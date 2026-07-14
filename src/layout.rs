//! Layout: `Doc` -> [`DocScene`] geometry, plus the built-in SVG
//! writer behind [`crate::scene::to_svg`].
//!
//! Text metrics are ESTIMATED the flowmaid way (a per-character
//! width table, no font files): close enough for pleasant reading
//! layouts, not for pixel-perfect typography. Honest notes on where
//! this stage diverges from a real renderer:
//! - code blocks never wrap: long source lines overrun the code
//!   card (and possibly the page);
//! - table cells are single-line and never truncated; a table wider
//!   than the column keeps its frame inside the page by narrowing
//!   every column proportionally, so cell text may overflow;
//! - an unbreakable word wider than the column is hard-broken at
//!   character level (real renderers overflow instead).

use crate::model::{Block, Doc, Inline, List, Table};
use crate::scene::{
    role_color, Anchor, ColorRole, DiagramItem, DiagramView, DocScene, Item, LayoutOptions,
    LineItem, LinkZone, RectItem, TextRun,
};
use flowmaid::model::Document;

// ── Layout constants (tweak here) ──────────────────────────────────
/// Outer page margin, all four sides.
const MARGIN: f64 = 24.0;
/// Requested widths are clamped into this finite, drawable range;
/// a non-finite width falls back to the default.
const MIN_DOC_WIDTH: f64 = 2.0 * MARGIN + 32.0;
const MAX_DOC_WIDTH: f64 = 100_000.0;
const DEFAULT_DOC_WIDTH: f64 = 720.0;
/// Minimum content width kept inside a list item, so deep nesting
/// stops indenting instead of marching off the right edge.
const MIN_LIST_CONTENT: f64 = 48.0;
/// Heading size multipliers for levels 1..=6 (x base size).
const HEADING_SCALE: [f64; 6] = [1.7, 1.45, 1.25, 1.1, 1.0, 0.95];
/// Space above a heading: levels 1-2 / levels 3+.
const HEAD_SPACE_MAJOR: f64 = 14.0;
const HEAD_SPACE_MINOR: f64 = 10.0;
/// Space below any heading.
const HEAD_SPACE_BELOW: f64 = 6.0;
/// Gap between heading text and its underline (levels 1-2 only).
const HEAD_RULE_GAP: f64 = 3.0;
/// Space after every non-heading block.
const BLOCK_SPACE: f64 = 8.0;
/// Space above and below a thematic break.
const RULE_SPACE: f64 = 10.0;
/// Line-box height as a multiple of the font size.
const LINE_HEIGHT: f64 = 1.5;
/// Monospace advance as a fraction of the font size.
const MONO_ADVANCE: f64 = 0.62;
/// flowmaid's proportional width table is calibrated at this size.
const FLOWMAID_CALIBRATION: f64 = 13.0;
/// Indent per list nesting level; vertical gap between items.
const LIST_INDENT: f64 = 18.0;
const LIST_GAP: f64 = 3.0;
/// GFM task checkbox: outer box and inner "checked" fill.
const CHECKBOX_SIZE: f64 = 13.0;
const CHECKBOX_FILL: f64 = 7.0;
/// Inline code chip: padding around the fragment, corner rounding.
const CHIP_PAD: f64 = 3.0;
const CHIP_ROUND: f64 = 3.0;
/// Block quote: horizontal content inset, vertical padding, accent
/// bar width.
const QUOTE_INSET: f64 = 14.0;
const QUOTE_PAD: f64 = 8.0;
const QUOTE_BAR: f64 = 4.0;
/// Code block interior padding (all sides).
const CODE_PAD: f64 = 12.0;
/// Table: horizontal cell padding (both sides combined), minimum
/// column width, vertical cell padding (both sides combined).
const CELL_PAD_X: f64 = 24.0;
const CELL_MIN_W: f64 = 40.0;
const CELL_PAD_Y: f64 = 8.0;
/// Diagram card: padding around the scaled mermaid scene.
const DIAGRAM_PAD: f64 = 12.0;

// ── Text metrics ────────────────────────────────────────────────────

/// Estimated width of `s` at `size` px. Proportional text reuses
/// flowmaid's per-character table (calibrated at 13 px); monospace
/// is a flat advance per char. Both are additive per character, so
/// a fragment re-measures to exactly the sum of its pieces.
fn text_w(s: &str, size: f64, mono: bool) -> f64 {
    if mono {
        MONO_ADVANCE * size * s.chars().count() as f64
    } else {
        flowmaid::layout::text_width(s) * size / FLOWMAID_CALIBRATION
    }
}

/// Height of one line box at `size` px.
fn line_h(size: f64) -> f64 {
    LINE_HEIGHT * size
}

// ── Inline wrapping ─────────────────────────────────────────────────

/// Resolved style of one fragment (an [`Inline`] minus its text).
#[derive(Debug, Clone, PartialEq)]
struct RunStyle {
    strong: bool,
    em: bool,
    code: bool,
    strike: bool,
    link: Option<String>,
}

impl RunStyle {
    fn of(run: &Inline) -> RunStyle {
        RunStyle {
            strong: run.strong,
            em: run.em,
            code: run.code,
            strike: run.strike,
            link: run.link.clone(),
        }
    }
}

/// One wrap token: an unbreakable word (possibly spanning several
/// styles when runs abut without whitespace, e.g. `fo**o**`) or a
/// hard line break.
enum Tok {
    Word(Vec<(String, RunStyle)>),
    Break,
}

/// Split inline runs into whitespace-delimited word tokens. Any
/// whitespace separates words (and collapses); a literal `\n` is a
/// hard break.
fn tokenize(inlines: &[Inline]) -> Vec<Tok> {
    let mut toks = Vec::new();
    let mut word: Vec<(String, RunStyle)> = Vec::new();
    for run in inlines {
        let st = RunStyle::of(run);
        for ch in run.text.chars() {
            if ch == '\n' {
                if !word.is_empty() {
                    toks.push(Tok::Word(std::mem::take(&mut word)));
                }
                toks.push(Tok::Break);
            } else if ch.is_whitespace() {
                if !word.is_empty() {
                    toks.push(Tok::Word(std::mem::take(&mut word)));
                }
            } else if let Some(last) = word.last_mut().filter(|p| p.1 == st) {
                last.0.push(ch);
            } else {
                word.push((ch.to_string(), st.clone()));
            }
        }
    }
    if !word.is_empty() {
        toks.push(Tok::Word(word));
    }
    toks
}

/// A placed same-style fragment of one line. `x` is absolute,
/// `w` its estimated width.
#[derive(Debug, Default, Clone)]
struct Frag {
    x: f64,
    w: f64,
    text: String,
    style: Option<RunStyle>,
}

/// Append a styled piece at the wrap cursor, merging into the
/// previous fragment when the style matches and there is no gap —
/// this is what yields ONE TextRun per contiguous same-style
/// stretch of a line.
fn push_piece(line: &mut Vec<Frag>, cx: &mut f64, text: &str, style: &RunStyle, size: f64) {
    let pw = text_w(text, size, style.code);
    if let Some(last) = line.last_mut() {
        if last.style.as_ref() == Some(style) && (last.x + last.w - *cx).abs() < 1e-9 {
            last.text.push_str(text);
            last.w += pw;
            *cx += pw;
            return;
        }
    }
    line.push(Frag {
        x: *cx,
        w: pw,
        text: text.to_string(),
        style: Some(style.clone()),
    });
    *cx += pw;
}

/// Greedy word wrap of `toks` into the column `[x, x + w)`. A word
/// wider than the whole column is hard-broken at character level so
/// no fragment ever escapes the column.
fn wrap_frags(toks: &[Tok], x: f64, w: f64, size: f64) -> Vec<Vec<Frag>> {
    let right = x + w;
    let mut lines: Vec<Vec<Frag>> = Vec::new();
    let mut line: Vec<Frag> = Vec::new();
    let mut cx = x;
    for tok in toks {
        let pieces = match tok {
            Tok::Break => {
                lines.push(std::mem::take(&mut line));
                cx = x;
                continue;
            }
            Tok::Word(pieces) => pieces,
        };
        let ww: f64 = pieces.iter().map(|(t, st)| text_w(t, size, st.code)).sum();
        if let Some(prev) = line.last() {
            // Mid-line: a space joins this word to the previous one.
            // Same style on both sides -> the space lives inside the
            // fragment (measured in that style); otherwise it is a
            // plain-width gap between fragments.
            let prev_style = prev.style.clone().expect("placed frag has style");
            let same = prev_style == pieces[0].1;
            let sw = if same {
                text_w(" ", size, prev_style.code)
            } else {
                text_w(" ", size, false)
            };
            if cx + sw + ww <= right + 1e-6 {
                if same {
                    push_piece(&mut line, &mut cx, " ", &prev_style, size);
                } else {
                    cx += sw;
                }
                for (t, st) in pieces {
                    push_piece(&mut line, &mut cx, t, st, size);
                }
                continue;
            }
            lines.push(std::mem::take(&mut line));
            cx = x;
        }
        // At line start.
        if ww <= w + 1e-6 {
            for (t, st) in pieces {
                push_piece(&mut line, &mut cx, t, st, size);
            }
        } else {
            // Overlong word: break at character level.
            for (t, st) in pieces {
                for ch in t.chars() {
                    let cw = text_w(ch.encode_utf8(&mut [0u8; 4]), size, st.code);
                    if cx + cw > right + 1e-6 && !line.is_empty() {
                        lines.push(std::mem::take(&mut line));
                        cx = x;
                    }
                    push_piece(&mut line, &mut cx, ch.encode_utf8(&mut [0u8; 4]), st, size);
                }
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Paint one wrapped line at line-box top `y`, shifted by `dx`
/// (used by table cells, whose fragments are measured at origin 0).
/// Code chips are emitted before every text run so a chip never
/// covers a neighbouring fragment. Style mapping: code -> mono +
/// CodeBg chip, link -> Link role + underline + LinkZone, strong ->
/// Strong role + bold, em -> italic, strike passes through.
fn emit_line(
    scene: &mut DocScene,
    frags: Vec<Frag>,
    dx: f64,
    y: f64,
    size: f64,
    base_role: ColorRole,
    force_strong: bool,
) {
    for f in &frags {
        let st = f.style.as_ref().expect("placed frag has style");
        if st.code {
            // Chip around the estimated glyph box (cap top ~0.1 em
            // above line top, descender bottom ~1.0 em).
            scene.items.push(Item::Rect(RectItem {
                x: dx + f.x - CHIP_PAD,
                y: y + 0.1 * size - CHIP_PAD,
                w: f.w + 2.0 * CHIP_PAD,
                h: 0.9 * size + 2.0 * CHIP_PAD,
                rounding: CHIP_ROUND,
                fill: Some(ColorRole::CodeBg),
                stroke: None,
            }));
        }
    }
    for f in frags {
        let st = f.style.expect("placed frag has style");
        let role = if st.link.is_some() {
            ColorRole::Link
        } else if st.code {
            ColorRole::CodeText
        } else if st.strong || force_strong {
            ColorRole::Strong
        } else {
            base_role
        };
        if let Some(url) = &st.link {
            scene.links.push(LinkZone {
                x: dx + f.x,
                y,
                w: f.w,
                h: line_h(size),
                url: url.clone(),
            });
        }
        scene.items.push(Item::Text(TextRun {
            x: dx + f.x,
            y,
            size,
            mono: st.code,
            strong: st.strong || force_strong,
            em: st.em,
            strike: st.strike,
            underline: st.link.is_some(),
            role,
            text: f.text,
        }));
    }
}

/// Wrap + paint inline content into the column starting at `y`;
/// returns the y below the last line.
#[allow(clippy::too_many_arguments)]
fn layout_inlines(
    scene: &mut DocScene,
    inlines: &[Inline],
    x: f64,
    w: f64,
    y: f64,
    size: f64,
    base_role: ColorRole,
    force_strong: bool,
) -> f64 {
    let toks = tokenize(inlines);
    let mut y = y;
    for line in wrap_frags(&toks, x, w, size) {
        emit_line(scene, line, 0.0, y, size, base_role, force_strong);
        y += line_h(size);
    }
    y
}

/// Inline content as a single unwrapped line of fragments measured
/// at origin 0 — table cells. Hard breaks become spaces.
fn line_frags(inlines: &[Inline], size: f64) -> Vec<Frag> {
    let clean: Vec<Inline> = inlines
        .iter()
        .map(|r| Inline {
            text: r.text.replace('\n', " "),
            ..r.clone()
        })
        .collect();
    let toks = tokenize(&clean);
    wrap_frags(&toks, 0.0, f64::INFINITY, size)
        .pop()
        .unwrap_or_default()
}

/// Concatenated plain text of inline content (anchor labels).
fn plain_text(inlines: &[Inline]) -> String {
    inlines
        .iter()
        .map(|r| r.text.replace('\n', " "))
        .collect()
}

// ── Block layout ────────────────────────────────────────────────────

/// Lay a parsed document out. `opts.width` is the full page width;
/// content occupies the column between the outer margins.
pub fn layout(doc: &Doc, opts: &LayoutOptions) -> DocScene {
    let mut scene = DocScene::default();
    // Sanitise the requested width: a non-finite width (inf/NaN)
    // would poison every coordinate and emit an invalid SVG, and a
    // sub-page width leaves no room to draw (bug hunt). Clamp to a
    // finite, drawable range; fall back to a sane default when the
    // caller passes garbage.
    let width = if opts.width.is_finite() {
        opts.width.clamp(MIN_DOC_WIDTH, MAX_DOC_WIDTH)
    } else {
        DEFAULT_DOC_WIDTH
    };
    scene.width = width;
    let col = width - 2.0 * MARGIN;
    let end = layout_blocks(&mut scene, &doc.blocks, MARGIN, col, MARGIN, opts.base_size);
    scene.height = end + MARGIN;
    scene
}

/// Extra space a block asks for above itself (applied between
/// blocks only — the first block of a container starts flush).
fn space_before(b: &Block) -> f64 {
    match b {
        Block::Heading { level, .. } if *level >= 3 => HEAD_SPACE_MINOR,
        Block::Heading { .. } => HEAD_SPACE_MAJOR,
        Block::Rule => RULE_SPACE,
        _ => 0.0,
    }
}

/// Space a block leaves below itself (not applied after the last
/// block — containers pad their own bottoms).
fn space_after(b: &Block) -> f64 {
    match b {
        Block::Heading { .. } => HEAD_SPACE_BELOW,
        Block::Rule => RULE_SPACE,
        _ => BLOCK_SPACE,
    }
}

/// Lay out a run of blocks top-down in the column `[x, x + w)`.
/// Returns the content-end y (no trailing spacing).
fn layout_blocks(
    scene: &mut DocScene,
    blocks: &[Block],
    x: f64,
    w: f64,
    mut y: f64,
    base: f64,
) -> f64 {
    for (i, b) in blocks.iter().enumerate() {
        if i > 0 {
            y += space_before(b);
        }
        y = layout_block(scene, b, x, w, y, base);
        if i + 1 < blocks.len() {
            y += space_after(b);
        }
    }
    y
}

fn is_mermaid(lang: &str) -> bool {
    lang == "mermaid" || lang == "mmd"
}

fn layout_block(scene: &mut DocScene, b: &Block, x: f64, w: f64, y: f64, base: f64) -> f64 {
    match b {
        Block::Heading { level, content } => layout_heading(scene, *level, content, x, w, y, base),
        Block::Paragraph(inls) => layout_inlines(scene, inls, x, w, y, base, ColorRole::Text, false),
        Block::Code { lang, source } if is_mermaid(lang) => {
            layout_mermaid(scene, source, x, w, y, base)
        }
        Block::Code { source, .. } => layout_code(scene, source, x, w, y, base),
        // Raw HTML is shown verbatim as code — never interpreted.
        Block::Html(source) => layout_code(scene, source, x, w, y, base),
        Block::Quote(blocks) => layout_quote(scene, blocks, x, w, y, base),
        Block::List(list) => layout_list(scene, list, x, w, y, base),
        Block::Table(table) => layout_table(scene, table, x, w, y, base),
        Block::Rule => {
            scene.items.push(Item::Line(LineItem {
                x1: x,
                y1: y,
                x2: x + w,
                y2: y,
                role: ColorRole::Border,
            }));
            y
        }
    }
}

fn layout_heading(
    scene: &mut DocScene,
    level: u8,
    content: &[Inline],
    x: f64,
    w: f64,
    y: f64,
    base: f64,
) -> f64 {
    let level = level.clamp(1, 6);
    let size = base * HEADING_SCALE[(level - 1) as usize];
    scene.anchors.push(Anchor {
        level,
        text: plain_text(content),
        y,
    });
    let mut end = layout_inlines(scene, content, x, w, y, size, ColorRole::Strong, true);
    if level <= 2 {
        end += HEAD_RULE_GAP;
        scene.items.push(Item::Line(LineItem {
            x1: x,
            y1: end,
            x2: x + w,
            y2: end,
            role: ColorRole::Border,
        }));
    }
    end
}

/// Verbatim code card: one mono run per source line, no wrapping.
fn layout_code(scene: &mut DocScene, source: &str, x: f64, w: f64, y: f64, base: f64) -> f64 {
    let lines: Vec<&str> = source.lines().collect();
    let lh = line_h(base);
    let h = lines.len() as f64 * lh + 2.0 * CODE_PAD;
    scene.items.push(Item::Rect(RectItem {
        x,
        y,
        w,
        h,
        rounding: 6.0,
        fill: Some(ColorRole::CodeBg),
        stroke: None,
    }));
    for (i, line) in lines.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        scene.items.push(Item::Text(TextRun {
            x: x + CODE_PAD,
            y: y + CODE_PAD + i as f64 * lh,
            size: base,
            mono: true,
            strong: false,
            em: false,
            strike: false,
            underline: false,
            role: ColorRole::CodeText,
            text: (*line).to_string(),
        }));
    }
    y + h
}

/// Block quote: content inset on a QuoteBg card with a Link-colored
/// accent bar down the left edge. The card rects are inserted
/// behind the already-laid-out content.
fn layout_quote(scene: &mut DocScene, blocks: &[Block], x: f64, w: f64, y: f64, base: f64) -> f64 {
    let idx = scene.items.len();
    let inner_end = layout_blocks(
        scene,
        blocks,
        x + QUOTE_INSET,
        (w - 2.0 * QUOTE_INSET).max(1.0),
        y + QUOTE_PAD,
        base,
    );
    let h = (inner_end + QUOTE_PAD) - y;
    scene.items.insert(
        idx,
        Item::Rect(RectItem {
            x,
            y,
            w,
            h,
            rounding: 6.0,
            fill: Some(ColorRole::QuoteBg),
            stroke: None,
        }),
    );
    scene.items.insert(
        idx + 1,
        Item::Rect(RectItem {
            x,
            y,
            w: QUOTE_BAR,
            h,
            rounding: 2.0,
            fill: Some(ColorRole::Link),
            stroke: None,
        }),
    );
    y + h
}

fn layout_list(scene: &mut DocScene, list: &List, x: f64, w: f64, y: f64, base: f64) -> f64 {
    let lh = line_h(base);
    let mut y = y;
    for (i, item) in list.items.iter().enumerate() {
        if i > 0 {
            y += LIST_GAP;
        }
        match (item.checked, list.start) {
            (Some(checked), _) => {
                // GFM task marker: outline box, filled inner square
                // when checked, vertically centred on the first line.
                let by = y + (lh - CHECKBOX_SIZE) / 2.0;
                scene.items.push(Item::Rect(RectItem {
                    x,
                    y: by,
                    w: CHECKBOX_SIZE,
                    h: CHECKBOX_SIZE,
                    rounding: 3.0,
                    fill: None,
                    stroke: Some(ColorRole::Border),
                }));
                if checked {
                    let inset = (CHECKBOX_SIZE - CHECKBOX_FILL) / 2.0;
                    scene.items.push(Item::Rect(RectItem {
                        x: x + inset,
                        y: by + inset,
                        w: CHECKBOX_FILL,
                        h: CHECKBOX_FILL,
                        rounding: 1.5,
                        fill: Some(ColorRole::Text),
                        stroke: None,
                    }));
                }
            }
            (None, start) => {
                let marker = match start {
                    Some(n) => format!("{}.", n + i as u64),
                    None => "\u{2022}".to_string(),
                };
                scene.items.push(Item::Text(TextRun {
                    x,
                    y,
                    size: base,
                    mono: false,
                    strong: false,
                    em: false,
                    strike: false,
                    underline: false,
                    role: ColorRole::Text,
                    text: marker,
                }));
            }
        }
        // Cap the indent so a deeply nested item never pushes its
        // marker/content past the right edge: once the column is
        // narrow, stop indenting (bug hunt).
        let indent = LIST_INDENT.min((w - MIN_LIST_CONTENT).max(0.0));
        let end = layout_blocks(
            scene,
            &item.blocks,
            x + indent,
            (w - indent).max(MIN_LIST_CONTENT),
            y,
            base,
        );
        // Never end above the marker's own line.
        y = end.max(y + lh);
    }
    y
}

fn layout_table(scene: &mut DocScene, table: &Table, x: f64, w: f64, y: f64, base: f64) -> f64 {
    let rows = &table.rows;
    if rows.is_empty() || rows[0].is_empty() {
        return y;
    }
    let ncols = rows[0].len();
    // Measure: every cell as one unwrapped line.
    let mut cells: Vec<Vec<Vec<Frag>>> = Vec::with_capacity(rows.len());
    let mut colw = vec![CELL_MIN_W; ncols];
    for row in rows {
        let mut frow = Vec::with_capacity(ncols);
        for c in 0..ncols {
            let frags = row.get(c).map(|cell| line_frags(cell, base)).unwrap_or_default();
            let tw = frags.last().map(|f| f.x + f.w).unwrap_or(0.0);
            colw[c] = colw[c].max(tw + CELL_PAD_X);
            frow.push(frags);
        }
        cells.push(frow);
    }
    // Keep the frame on the page: narrow all columns proportionally
    // when the natural width exceeds the column (cells may overflow).
    let natural: f64 = colw.iter().sum();
    if natural > w {
        let f = w / natural;
        for cw in &mut colw {
            *cw *= f;
        }
    }
    let total_w: f64 = colw.iter().sum();
    let row_h = line_h(base) + CELL_PAD_Y;
    let total_h = rows.len() as f64 * row_h;

    // Stripes: header strip + every odd body row.
    for r in 0..rows.len() {
        if r == 0 || (r - 1) % 2 == 1 {
            scene.items.push(Item::Rect(RectItem {
                x,
                y: y + r as f64 * row_h,
                w: total_w,
                h: row_h,
                rounding: 0.0,
                fill: Some(ColorRole::TableStripeBg),
                stroke: None,
            }));
        }
    }
    // Grid: frame plus every row/column separator.
    for r in 0..=rows.len() {
        let ly = y + r as f64 * row_h;
        scene.items.push(Item::Line(LineItem {
            x1: x,
            y1: ly,
            x2: x + total_w,
            y2: ly,
            role: ColorRole::Border,
        }));
    }
    let mut cx = x;
    for c in 0..=ncols {
        scene.items.push(Item::Line(LineItem {
            x1: cx,
            y1: y,
            x2: cx,
            y2: y + total_h,
            role: ColorRole::Border,
        }));
        if c < ncols {
            cx += colw[c];
        }
    }
    // Cell text: header bold, everything single-line.
    for (r, frow) in cells.into_iter().enumerate() {
        let ty = y + r as f64 * row_h + CELL_PAD_Y / 2.0;
        let (role, strong) = if r == 0 {
            (ColorRole::Strong, true)
        } else {
            (ColorRole::Text, false)
        };
        let mut cx = x;
        for (c, frags) in frow.into_iter().enumerate() {
            emit_line(scene, frags, cx + CELL_PAD_X / 2.0, ty, base, role, strong);
            cx += colw[c];
        }
    }
    y + total_h
}

// ── Mermaid blocks ──────────────────────────────────────────────────

/// A ```mermaid fence: run the flowmaid engine and embed the scene
/// on a white card, scaled down (never up) to fit the column. A
/// parse error becomes a red card with the line-numbered message —
/// the document keeps rendering.
fn layout_mermaid(scene: &mut DocScene, source: &str, x: f64, w: f64, y: f64, base: f64) -> f64 {
    let parsed = match flowmaid::parser::parse_document(source) {
        Ok(p) => p,
        Err(e) => return layout_mermaid_error(scene, &e.to_string(), x, w, y, base),
    };
    let (size, view) = match parsed {
        Document::Flowchart(g) | Document::State(g) => {
            let sc = flowmaid::scene::scene(&g);
            ((sc.width, sc.height), DiagramView::Flow(sc))
        }
        Document::Er(d) => {
            let es = flowmaid::er::scene(&d);
            ((es.scene.width, es.scene.height), DiagramView::Er(es))
        }
        Document::Class(d) => {
            let cs = flowmaid::class::scene(&d);
            ((cs.scene.width, cs.scene.height), DiagramView::Class(cs))
        }
        Document::Sequence(d) => {
            let ss = flowmaid::seq::scene(&d);
            ((ss.width, ss.height), DiagramView::Seq(ss))
        }
        Document::Pie(d) => {
            let ps = flowmaid::pie::scene(&d);
            ((ps.width, ps.height), DiagramView::Pie(ps))
        }
    };
    // Fit-to-width, shrinking as far as needed (no lower clamp — a
    // very wide diagram must still fit inside its card, not overflow;
    // bug hunt). Guard the divide against a zero-width scene.
    let avail = (w - 2.0 * DIAGRAM_PAD).max(1.0);
    let scale = if size.0 > 0.0 {
        (avail / size.0).min(1.0)
    } else {
        1.0
    };
    let (cw, ch) = (
        size.0 * scale + 2.0 * DIAGRAM_PAD,
        size.1 * scale + 2.0 * DIAGRAM_PAD,
    );
    scene.items.push(Item::Rect(RectItem {
        x,
        y,
        w: cw,
        h: ch,
        rounding: 8.0,
        fill: Some(ColorRole::DiagramBg),
        stroke: Some(ColorRole::Border),
    }));
    scene.items.push(Item::Diagram(DiagramItem {
        x: x + DIAGRAM_PAD,
        y: y + DIAGRAM_PAD,
        scale,
        size,
        view: Box::new(view),
    }));
    y + ch
}

fn layout_mermaid_error(
    scene: &mut DocScene,
    message: &str,
    x: f64,
    w: f64,
    y: f64,
    base: f64,
) -> f64 {
    let msg = format!("mermaid: {}", message);
    let lh = line_h(base);
    // Naive character wrap at the column (mono metric).
    let max_chars = (((w - 2.0 * CODE_PAD) / (MONO_ADVANCE * base)) as usize).max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for (i, ch) in msg.chars().enumerate() {
        if i > 0 && i % max_chars == 0 {
            lines.push(std::mem::take(&mut cur));
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    let h = lines.len() as f64 * lh + 2.0 * CODE_PAD;
    scene.items.push(Item::Rect(RectItem {
        x,
        y,
        w,
        h,
        rounding: 6.0,
        fill: Some(ColorRole::ErrorBg),
        stroke: None,
    }));
    for (i, line) in lines.into_iter().enumerate() {
        scene.items.push(Item::Text(TextRun {
            x: x + CODE_PAD,
            y: y + CODE_PAD + i as f64 * lh,
            size: base,
            mono: true,
            strong: false,
            em: false,
            strike: false,
            underline: false,
            role: ColorRole::ErrorText,
            text: line,
        }));
    }
    y + h
}

// ── SVG writer ──────────────────────────────────────────────────────

/// XML-escape text content (`&`, `<`, `>`).
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

/// Serialise a [`DocScene`] to a standalone SVG document using the
/// default palette. Inline diagrams are embedded as nested `<svg>`
/// elements straight from the flowmaid writers.
pub(crate) fn doc_to_svg(scene: &DocScene) -> String {
    let mut s = String::new();
    // Defensive: the writer never emits a zero/negative/non-finite
    // dimension even if handed a hand-built scene — a conformant
    // rasteriser (rsvg) refuses "no dimensions" (bug hunt). layout()
    // already guarantees a sane width; this is belt-and-braces.
    let w = if scene.width.is_finite() { scene.width.max(1.0) } else { 1.0 };
    let h = if scene.height.is_finite() { scene.height.max(1.0) } else { 1.0 };
    s.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.1}\" height=\"{h:.1}\" \
         viewBox=\"0 0 {w:.1} {h:.1}\" font-family=\"Helvetica, Arial, sans-serif\">\n",
    ));
    s.push_str(&format!(
        "<rect width=\"{w:.1}\" height=\"{h:.1}\" fill=\"#ffffff\"/>\n",
    ));
    for item in &scene.items {
        match item {
            Item::Rect(r) => {
                s.push_str(&format!(
                    "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" \
                     rx=\"{:.1}\" fill=\"{}\"",
                    r.x,
                    r.y,
                    r.w,
                    r.h,
                    r.rounding,
                    r.fill.map(role_color).unwrap_or("none"),
                ));
                if let Some(st) = r.stroke {
                    s.push_str(&format!(" stroke=\"{}\" stroke-width=\"1\"", role_color(st)));
                }
                s.push_str("/>\n");
            }
            Item::Line(l) => {
                s.push_str(&format!(
                    "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" \
                     stroke=\"{}\" stroke-width=\"1\"/>\n",
                    l.x1,
                    l.y1,
                    l.x2,
                    l.y2,
                    role_color(l.role),
                ));
            }
            Item::Text(t) => {
                let mut attrs = String::new();
                if t.mono {
                    attrs.push_str(" font-family=\"ui-monospace, Menlo, monospace\"");
                }
                if t.strong {
                    attrs.push_str(" font-weight=\"700\"");
                }
                if t.em {
                    attrs.push_str(" font-style=\"italic\"");
                }
                match (t.underline, t.strike) {
                    (true, true) => attrs.push_str(" text-decoration=\"underline line-through\""),
                    (true, false) => attrs.push_str(" text-decoration=\"underline\""),
                    (false, true) => attrs.push_str(" text-decoration=\"line-through\""),
                    (false, false) => {}
                }
                s.push_str(&format!(
                    "<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"{:.1}\"{} fill=\"{}\">{}</text>\n",
                    t.x,
                    t.y + 0.80 * t.size,
                    t.size,
                    attrs,
                    role_color(t.role),
                    escape(&t.text),
                ));
            }
            Item::Diagram(d) => {
                let full = match &*d.view {
                    DiagramView::Flow(sc) => flowmaid::scene::to_svg(sc),
                    DiagramView::Er(es) => flowmaid::er::to_svg(es),
                    DiagramView::Class(cs) => flowmaid::class::to_svg(cs),
                    DiagramView::Seq(ss) => flowmaid::seq::to_svg(ss),
                    DiagramView::Pie(ps) => flowmaid::pie::to_svg(ps),
                };
                // Strip the writer's outer <svg> element and re-wrap
                // as a nested <svg>: the viewBox scales the content
                // automatically. font-size 14 restores the root
                // default flowmaid texts inherit.
                let start = full.find('>').map_or(0, |i| i + 1);
                let end = full.rfind("</svg>").unwrap_or(full.len());
                s.push_str(&format!(
                    "<svg x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" \
                     viewBox=\"0 0 {:.1} {:.1}\" font-size=\"14\">\n",
                    d.x,
                    d.y,
                    d.size.0 * d.scale,
                    d.size.1 * d.scale,
                    d.size.0,
                    d.size.1,
                ));
                s.push_str(&full[start..end]);
                s.push_str("</svg>\n");
            }
        }
    }
    s.push_str("</svg>\n");
    s
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Block, Doc, Inline, List, ListItem, Table};

    fn opts(width: f64) -> LayoutOptions {
        LayoutOptions {
            width,
            base_size: 14.0,
        }
    }

    fn doc(blocks: Vec<Block>) -> Doc {
        Doc { blocks }
    }

    fn texts(scene: &DocScene) -> Vec<&TextRun> {
        scene
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Text(t) => Some(t),
                _ => None,
            })
            .collect()
    }

    fn rects(scene: &DocScene) -> Vec<&RectItem> {
        scene
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Rect(r) => Some(r),
                _ => None,
            })
            .collect()
    }

    fn glines(scene: &DocScene) -> Vec<&LineItem> {
        scene
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Line(l) => Some(l),
                _ => None,
            })
            .collect()
    }

    fn styled(text: &str, f: impl Fn(&mut Inline)) -> Inline {
        let mut i = Inline::plain(text);
        f(&mut i);
        i
    }

    fn para(text: &str) -> Block {
        Block::Paragraph(vec![Inline::plain(text)])
    }

    /// The frozen geometric invariant: every sized item stays inside
    /// `[0, width]` (half-pixel tolerance).
    fn assert_within(scene: &DocScene) {
        for item in &scene.items {
            match item {
                Item::Rect(r) => {
                    assert!(r.x >= -0.5, "rect x {} < 0", r.x);
                    assert!(
                        r.x + r.w <= scene.width + 0.5,
                        "rect right {} > width {}",
                        r.x + r.w,
                        scene.width
                    );
                }
                Item::Line(l) => {
                    for v in [l.x1, l.x2] {
                        assert!((-0.5..=scene.width + 0.5).contains(&v), "line x {}", v);
                    }
                }
                Item::Diagram(d) => {
                    assert!(d.x >= -0.5);
                    assert!(d.x + d.size.0 * d.scale <= scene.width + 0.5);
                }
                Item::Text(_) => {}
            }
        }
    }

    // ── regressions from the bug hunt ──

    #[test]
    fn non_finite_width_is_sanitised() {
        for w in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            let sc = layout(&doc(vec![Block::Rule, para("hi")]), &opts(w));
            assert!(sc.width.is_finite() && sc.width > 0.0, "width {}", sc.width);
            assert_within(&sc);
            let svg = crate::scene::to_svg(&sc);
            assert!(!svg.contains("inf") && !svg.contains("NaN"), "{svg}");
        }
    }

    #[test]
    fn tiny_and_zero_width_stay_drawable() {
        for w in [0.0, 0.4, -50.0, 10.0] {
            let sc = layout(&doc(vec![para("hello world")]), &opts(w));
            assert!(sc.width >= MIN_DOC_WIDTH - 1e-9);
            let svg = crate::scene::to_svg(&sc);
            // Header width is positive and not rounded to zero.
            assert!(svg.contains("width=\"") && !svg.contains("width=\"0"), "{}", &svg[..80]);
        }
    }

    #[test]
    fn deeply_nested_list_marker_stays_on_page() {
        // 20 levels deep, each a single item wrapping the next.
        fn nest(depth: usize) -> Block {
            let inner: Vec<Block> = if depth == 0 {
                vec![para("word")]
            } else {
                vec![para("x"), nest(depth - 1)]
            };
            Block::List(List {
                start: None,
                items: vec![ListItem { checked: None, blocks: inner }],
            })
        }
        let sc = layout(&doc(vec![nest(20)]), &opts(300.0));
        for t in texts(&sc) {
            let w = text_w(&t.text, t.size, t.mono);
            assert!(
                t.x + w <= sc.width + 0.5,
                "marker/text '{}' at x={} overflows width {}",
                t.text,
                t.x,
                sc.width
            );
        }
    }

    #[test]
    fn very_wide_diagram_fits_its_card() {
        // A flowchart of many nodes in one row is very wide; it must
        // shrink to fit, not overflow at a 0.01 floor.
        let mut src = String::from("flowchart LR\n");
        for i in 0..40 {
            src.push_str(&format!("N{i}[Node number {i} label]-->N{}\n", i + 1));
        }
        let block = Block::Code { lang: "mermaid".into(), source: src };
        let sc = layout(&doc(vec![block]), &opts(720.0));
        let d = sc
            .items
            .iter()
            .find_map(|i| match i {
                Item::Diagram(d) => Some(d),
                _ => None,
            })
            .expect("diagram");
        assert!(d.scale > 0.0 && d.scale <= 1.0);
        assert!(d.x + d.size.0 * d.scale <= sc.width + 0.5, "diagram overflows");
        assert_within(&sc);
    }

    #[test]
    fn wrap_produces_multiple_lines_inside_column() {
        let text = "the quick brown fox jumps over the lazy dog again and again until it wraps";
        let sc = layout(&doc(vec![para(text)]), &opts(240.0));
        let ts = texts(&sc);
        let mut ys: Vec<i64> = ts.iter().map(|t| (t.y * 10.0) as i64).collect();
        ys.sort();
        ys.dedup();
        assert!(ys.len() >= 3, "expected several lines, got {}", ys.len());
        for t in &ts {
            assert!(t.x >= MARGIN - 1e-6);
            let w = text_w(&t.text, t.size, t.mono);
            assert!(
                t.x + w <= 240.0 - MARGIN + 0.5,
                "run '{}' overflows: {} > {}",
                t.text,
                t.x + w,
                240.0 - MARGIN
            );
        }
    }

    #[test]
    fn heading_records_anchor_and_underline() {
        let sc = layout(
            &doc(vec![Block::Heading {
                level: 2,
                content: vec![Inline::plain("Setup guide")],
            }]),
            &opts(400.0),
        );
        assert_eq!(sc.anchors.len(), 1);
        assert_eq!(sc.anchors[0].level, 2);
        assert_eq!(sc.anchors[0].text, "Setup guide");
        assert!((sc.anchors[0].y - MARGIN).abs() < 1e-9);
        let run = &texts(&sc)[0];
        assert!((run.size - 14.0 * 1.45).abs() < 1e-9);
        assert_eq!(run.role, ColorRole::Strong);
        assert!(run.strong);
        // Level 2 gets a full-column underline.
        let ls = glines(&sc);
        assert_eq!(ls.len(), 1);
        assert_eq!(ls[0].role, ColorRole::Border);
        assert!((ls[0].x1 - MARGIN).abs() < 1e-9 && (ls[0].x2 - 376.0).abs() < 1e-9);
        assert!(ls[0].y1 > run.y);

        // Level 3: no underline, smaller size.
        let sc3 = layout(
            &doc(vec![Block::Heading {
                level: 3,
                content: vec![Inline::plain("Minor")],
            }]),
            &opts(400.0),
        );
        assert!(glines(&sc3).is_empty());
        assert_eq!(sc3.anchors[0].level, 3);
    }

    #[test]
    fn heading_sizes_follow_scale_table() {
        let blocks: Vec<Block> = (1..=6)
            .map(|l| Block::Heading {
                level: l,
                content: vec![Inline::plain("h")],
            })
            .collect();
        let sc = layout(&doc(blocks), &opts(400.0));
        let sizes: Vec<f64> = texts(&sc).iter().map(|t| t.size).collect();
        for (i, s) in sizes.iter().enumerate() {
            assert!((s - 14.0 * HEADING_SCALE[i]).abs() < 1e-9, "level {}", i + 1);
        }
    }

    #[test]
    fn heading_spacing_after_paragraph() {
        let sc = layout(
            &doc(vec![
                para("intro"),
                Block::Heading {
                    level: 1,
                    content: vec![Inline::plain("Title")],
                },
            ]),
            &opts(400.0),
        );
        // 24 margin + 21 para line + 8 after + 14 above = 67.
        assert!((sc.anchors[0].y - 67.0).abs() < 1e-9);
    }

    #[test]
    fn link_zone_aligns_with_its_run() {
        let sc = layout(
            &doc(vec![Block::Paragraph(vec![
                Inline::plain("see "),
                styled("docs", |i| i.link = Some("https://example.com".into())),
            ])]),
            &opts(400.0),
        );
        assert_eq!(sc.links.len(), 1);
        let z = &sc.links[0];
        assert_eq!(z.url, "https://example.com");
        let run = texts(&sc)
            .into_iter()
            .find(|t| t.text == "docs")
            .expect("link run");
        assert!(run.underline);
        assert_eq!(run.role, ColorRole::Link);
        assert!((z.x - run.x).abs() < 1e-9);
        assert!((z.y - run.y).abs() < 1e-9);
        assert!((z.w - text_w("docs", 14.0, false)).abs() < 1e-6);
        assert!((z.h - 21.0).abs() < 1e-9);
    }

    #[test]
    fn inline_code_gets_mono_run_and_chip() {
        let sc = layout(
            &doc(vec![Block::Paragraph(vec![
                Inline::plain("run "),
                styled("cargo test", |i| i.code = true),
            ])]),
            &opts(400.0),
        );
        let run = texts(&sc)
            .into_iter()
            .find(|t| t.text == "cargo test")
            .expect("one merged mono fragment");
        assert!(run.mono);
        assert_eq!(run.role, ColorRole::CodeText);
        let chip = rects(&sc)
            .into_iter()
            .find(|r| r.fill == Some(ColorRole::CodeBg))
            .expect("chip");
        assert!((chip.x - (run.x - CHIP_PAD)).abs() < 1e-9);
        let w = text_w("cargo test", 14.0, true);
        assert!((chip.w - (w + 2.0 * CHIP_PAD)).abs() < 1e-6);
        assert!((chip.rounding - CHIP_ROUND).abs() < 1e-9);
    }

    #[test]
    fn code_block_rect_contains_all_runs() {
        let sc = layout(
            &doc(vec![Block::Code {
                lang: "rust".into(),
                source: "fn main() {\n    println!(\"hi\");\n}".into(),
            }]),
            &opts(400.0),
        );
        let card = rects(&sc)[0];
        assert_eq!(card.fill, Some(ColorRole::CodeBg));
        assert!((card.h - (3.0 * 21.0 + 24.0)).abs() < 1e-9);
        let ts = texts(&sc);
        assert_eq!(ts.len(), 3);
        for t in ts {
            assert!(t.mono);
            assert_eq!(t.role, ColorRole::CodeText);
            assert!((t.x - (card.x + CODE_PAD)).abs() < 1e-9);
            assert!(t.y >= card.y && t.y + 21.0 <= card.y + card.h + 1e-6);
        }
    }

    #[test]
    fn table_emits_rectangular_grid_and_strong_header() {
        let cell = |s: &str| vec![Inline::plain(s)];
        let sc = layout(
            &doc(vec![Block::Table(Table {
                rows: vec![
                    vec![cell("Name"), cell("Qty")],
                    vec![cell("apple"), cell("1")],
                    vec![cell("pear"), cell("2")],
                ],
            })]),
            &opts(400.0),
        );
        let ls = glines(&sc);
        let horizontal: Vec<_> = ls.iter().filter(|l| l.y1 == l.y2).collect();
        let vertical: Vec<_> = ls.iter().filter(|l| l.x1 == l.x2).collect();
        assert_eq!(horizontal.len(), 4); // 3 rows -> 4 boundaries
        assert_eq!(vertical.len(), 3); // 2 cols -> 3 boundaries
        for l in &ls {
            assert_eq!(l.role, ColorRole::Border);
        }
        // Rows are 29 apart (21 line + 8 padding).
        let mut hys: Vec<f64> = horizontal.iter().map(|l| l.y1).collect();
        hys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!((hys[1] - hys[0] - 29.0).abs() < 1e-9);
        // Header strong + stripes: header strip and the 2nd body row.
        let header = texts(&sc).into_iter().find(|t| t.text == "Name").unwrap();
        assert!(header.strong);
        assert_eq!(header.role, ColorRole::Strong);
        let body = texts(&sc).into_iter().find(|t| t.text == "apple").unwrap();
        assert!(!body.strong);
        let stripes: Vec<_> = rects(&sc)
            .into_iter()
            .filter(|r| r.fill == Some(ColorRole::TableStripeBg))
            .collect();
        assert_eq!(stripes.len(), 2);
        assert_within(&sc);
    }

    #[test]
    fn task_checkbox_inner_fill_only_when_checked() {
        let item = |checked, text: &str| ListItem {
            checked: Some(checked),
            blocks: vec![para(text)],
        };
        let sc = layout(
            &doc(vec![Block::List(List {
                start: None,
                items: vec![item(true, "done"), item(false, "todo")],
            })]),
            &opts(400.0),
        );
        let boxes: Vec<_> = rects(&sc)
            .into_iter()
            .filter(|r| r.stroke == Some(ColorRole::Border) && r.w == CHECKBOX_SIZE)
            .collect();
        assert_eq!(boxes.len(), 2);
        for b in &boxes {
            assert_eq!(b.h, CHECKBOX_SIZE);
            assert!((b.rounding - 3.0).abs() < 1e-9);
        }
        let fills: Vec<_> = rects(&sc)
            .into_iter()
            .filter(|r| r.fill == Some(ColorRole::Text) && r.w == CHECKBOX_FILL)
            .collect();
        assert_eq!(fills.len(), 1);
        // Inner fill centred in the first (checked) box.
        assert!((fills[0].x - (boxes[0].x + 3.0)).abs() < 1e-9);
        // Item text starts one indent in.
        let t = texts(&sc).into_iter().find(|t| t.text == "done").unwrap();
        assert!((t.x - (MARGIN + LIST_INDENT)).abs() < 1e-9);
    }

    #[test]
    fn ordered_markers_and_nested_indent() {
        let sc = layout(
            &doc(vec![Block::List(List {
                start: Some(3),
                items: vec![
                    ListItem {
                        checked: None,
                        blocks: vec![para("first")],
                    },
                    ListItem {
                        checked: None,
                        blocks: vec![
                            para("second"),
                            Block::List(List {
                                start: None,
                                items: vec![ListItem {
                                    checked: None,
                                    blocks: vec![para("inner")],
                                }],
                            }),
                        ],
                    },
                ],
            })]),
            &opts(400.0),
        );
        let ts = texts(&sc);
        let find = |s: &str| ts.iter().find(|t| t.text == s).unwrap();
        assert!((find("3.").x - MARGIN).abs() < 1e-9);
        assert!((find("4.").x - MARGIN).abs() < 1e-9);
        assert!((find("first").x - (MARGIN + LIST_INDENT)).abs() < 1e-9);
        assert!((find("\u{2022}").x - (MARGIN + LIST_INDENT)).abs() < 1e-9);
        assert!((find("inner").x - (MARGIN + 2.0 * LIST_INDENT)).abs() < 1e-9);
    }

    #[test]
    fn list_items_are_three_pixels_apart() {
        let item = |text: &str| ListItem {
            checked: None,
            blocks: vec![para(text)],
        };
        let sc = layout(
            &doc(vec![Block::List(List {
                start: None,
                items: vec![item("a"), item("b")],
            })]),
            &opts(400.0),
        );
        let ts = texts(&sc);
        let a = ts.iter().find(|t| t.text == "a").unwrap();
        let b = ts.iter().find(|t| t.text == "b").unwrap();
        assert!((b.y - a.y - (21.0 + LIST_GAP)).abs() < 1e-9);
    }

    #[test]
    fn quote_content_is_inset_on_a_card() {
        let sc = layout(
            &doc(vec![Block::Quote(vec![para("quoted words")])]),
            &opts(400.0),
        );
        let rs = rects(&sc);
        let bg = rs
            .iter()
            .find(|r| r.fill == Some(ColorRole::QuoteBg))
            .expect("bg");
        let bar = rs
            .iter()
            .find(|r| r.fill == Some(ColorRole::Link))
            .expect("accent");
        assert!((bg.x - MARGIN).abs() < 1e-9);
        assert!((bg.w - 352.0).abs() < 1e-9);
        assert!((bg.rounding - 6.0).abs() < 1e-9);
        assert_eq!(bar.w, QUOTE_BAR);
        assert!((bar.h - bg.h).abs() < 1e-9);
        assert!((bar.y - bg.y).abs() < 1e-9);
        let t = &texts(&sc)[0];
        assert!((t.x - (MARGIN + QUOTE_INSET)).abs() < 1e-9);
        assert!((t.y - (bg.y + QUOTE_PAD)).abs() < 1e-9);
        // Bg h = content line + 2 * padding.
        assert!((bg.h - (21.0 + 2.0 * QUOTE_PAD)).abs() < 1e-9);
    }

    #[test]
    fn mermaid_flowchart_becomes_a_scaled_diagram() {
        let sc = layout(
            &doc(vec![Block::Code {
                lang: "mermaid".into(),
                source: "flowchart TD\nA[Start] --> B[Done]".into(),
            }]),
            &opts(400.0),
        );
        let d = sc
            .items
            .iter()
            .find_map(|i| match i {
                Item::Diagram(d) => Some(d),
                _ => None,
            })
            .expect("diagram item");
        assert!(d.scale <= 1.0 && d.scale > 0.0);
        assert!(matches!(&*d.view, DiagramView::Flow(_)));
        assert!((d.x - (MARGIN + DIAGRAM_PAD)).abs() < 1e-9);
        let card = rects(&sc)
            .into_iter()
            .find(|r| r.fill == Some(ColorRole::DiagramBg))
            .expect("card");
        assert_eq!(card.stroke, Some(ColorRole::Border));
        assert!((card.w - (d.size.0 * d.scale + 2.0 * DIAGRAM_PAD)).abs() < 1e-6);
        assert_within(&sc);

        let svg = doc_to_svg(&sc);
        assert!(svg.contains("<svg x="), "nested svg embed missing");
        assert!(svg.contains("Start"));
        assert!(!svg.to_lowercase().contains("todo"));
    }

    #[test]
    fn unsupported_mermaid_becomes_error_card() {
        let sc = layout(
            &doc(vec![Block::Code {
                lang: "mmd".into(),
                source: "gantt\ntitle nope".into(),
            }]),
            &opts(400.0),
        );
        let bg = rects(&sc)
            .into_iter()
            .find(|r| r.fill == Some(ColorRole::ErrorBg))
            .expect("error bg");
        assert!((bg.x - MARGIN).abs() < 1e-9);
        let ts = texts(&sc);
        assert!(!ts.is_empty());
        assert!(ts[0].mono);
        assert_eq!(ts[0].role, ColorRole::ErrorText);
        assert!(
            ts[0].text.starts_with("mermaid: line "),
            "got '{}'",
            ts[0].text
        );
        assert_within(&sc);
    }

    #[test]
    fn hard_break_forces_a_new_line() {
        let sc = layout(
            &doc(vec![Block::Paragraph(vec![Inline::plain(
                "line one\nline two",
            )])]),
            &opts(400.0),
        );
        let ts = texts(&sc);
        assert_eq!(ts.len(), 2);
        assert_eq!(ts[0].text, "line one");
        assert_eq!(ts[1].text, "line two");
        assert!((ts[1].y - ts[0].y - 21.0).abs() < 1e-9);
    }

    #[test]
    fn rule_is_a_full_column_border_line() {
        let sc = layout(&doc(vec![Block::Rule]), &opts(400.0));
        let ls = glines(&sc);
        assert_eq!(ls.len(), 1);
        assert_eq!(ls[0].role, ColorRole::Border);
        assert!((ls[0].x1 - MARGIN).abs() < 1e-9);
        assert!((ls[0].x2 - 376.0).abs() < 1e-9);
    }

    #[test]
    fn overlong_word_is_hard_broken_to_fit() {
        let sc = layout(&doc(vec![para(&"a".repeat(120))]), &opts(200.0));
        let ts = texts(&sc);
        assert!(ts.len() >= 2);
        for t in ts {
            assert!((t.x - MARGIN).abs() < 1e-9);
            assert!(t.x + text_w(&t.text, t.size, t.mono) <= 200.0 - MARGIN + 0.5);
        }
    }

    #[test]
    fn empty_doc_is_just_margins() {
        let sc = layout(&Doc::default(), &opts(400.0));
        assert!(sc.items.is_empty());
        assert!((sc.height - 2.0 * MARGIN).abs() < 1e-9);
        assert!((sc.width - 400.0).abs() < 1e-9);
    }

    #[test]
    fn metrics_are_additive() {
        assert!((text_w("abc", 14.0, true) - 0.62 * 14.0 * 3.0).abs() < 1e-9);
        let ab = text_w("ab", 14.0, false);
        assert!((ab - text_w("a", 14.0, false) - text_w("b", 14.0, false)).abs() < 1e-9);
        assert_eq!(line_h(14.0), 21.0);
    }

    #[test]
    fn svg_escapes_text_and_code() {
        let sc = layout(
            &doc(vec![
                para("a<b & c>d"),
                Block::Code {
                    lang: "".into(),
                    source: "if x < 1 && y > 2 {}".into(),
                },
            ]),
            &opts(400.0),
        );
        let svg = doc_to_svg(&sc);
        assert!(svg.contains("a&lt;b"));
        assert!(svg.contains("&amp;"));
        assert!(svg.contains("c&gt;d"));
        assert!(svg.contains("x &lt; 1 &amp;&amp; y &gt; 2"));
        assert!(!svg.contains("c>d</text>"));
    }

    #[test]
    fn svg_document_shape() {
        let sc = layout(&doc(vec![para("hi")]), &opts(300.0));
        let svg = doc_to_svg(&sc);
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"300.0\""));
        assert!(svg.contains("viewBox=\"0 0 300.0"));
        assert!(svg.contains("font-family=\"Helvetica, Arial, sans-serif\""));
        assert!(svg.contains("fill=\"#ffffff\"/>"));
        assert!(svg.trim_end().ends_with("</svg>"));
        // Baseline = top + 0.8 * size: 24 + 11.2.
        assert!(svg.contains("y=\"35.2\""));
    }

    #[test]
    fn mixed_document_stays_inside_the_page() {
        let cell = |s: &str| vec![Inline::plain(s)];
        let d = doc(vec![
            Block::Heading {
                level: 1,
                content: vec![Inline::plain("Everything at once")],
            },
            Block::Paragraph(vec![
                Inline::plain("Body with "),
                styled("bold", |i| i.strong = true),
                Inline::plain(" and "),
                styled("code", |i| i.code = true),
                Inline::plain(" and a "),
                styled("link", |i| i.link = Some("https://x.dev".into())),
                Inline::plain(" run that is long enough to wrap over several lines here."),
            ]),
            Block::List(List {
                start: Some(1),
                items: vec![
                    ListItem {
                        checked: None,
                        blocks: vec![para("plain item")],
                    },
                    ListItem {
                        checked: Some(true),
                        blocks: vec![para("done task")],
                    },
                ],
            }),
            Block::Quote(vec![para("a quoted line")]),
            Block::Code {
                lang: "sh".into(),
                source: "cargo build".into(),
            },
            Block::Table(Table {
                rows: vec![vec![cell("k"), cell("v")], vec![cell("a"), cell("b")]],
            }),
            Block::Rule,
            Block::Html("<b>raw</b>".into()),
            Block::Code {
                lang: "mermaid".into(),
                source: "flowchart LR\nA --> B".into(),
            },
        ]);
        let sc = layout(&d, &opts(360.0));
        assert_within(&sc);
        for t in texts(&sc) {
            assert!(t.x >= -1e-6);
            assert!(
                t.x + text_w(&t.text, t.size, t.mono) <= sc.width + 0.5,
                "run '{}' escapes the page",
                t.text
            );
        }
        assert_eq!(sc.anchors.len(), 1);
        assert_eq!(sc.links.len(), 1);
        assert!(sc.height > 2.0 * MARGIN);
        let svg = doc_to_svg(&sc);
        assert!(svg.contains("<svg x="));
        assert!(svg.contains("&lt;b&gt;raw&lt;/b&gt;"));
    }

    #[test]
    fn wide_table_is_narrowed_to_fit() {
        let cell = |s: &str| vec![Inline::plain(s)];
        let long = "a very long header cell that will not fit";
        let sc = layout(
            &doc(vec![Block::Table(Table {
                rows: vec![
                    vec![cell(long), cell(long), cell(long)],
                    vec![cell("x"), cell("y"), cell("z")],
                ],
            })]),
            &opts(300.0),
        );
        assert_within(&sc);
    }
}
