//! Hand-written GFM-subset parser: `&str -> Doc`. Pure std, no
//! regexes, infallible — anything unrecognised degrades to plain
//! paragraph text rather than an error.
//!
//! Supported: ATX headings, paragraphs (lazy continuation, hard
//! breaks via trailing `\` or two spaces), ``` / ~~~ fences with
//! info strings, nested blockquotes, unordered/ordered lists with
//! nesting and GFM task markers, GFM tables with `\|` escapes,
//! thematic breaks, verbatim raw-HTML blocks, and inline
//! strong/em/code/strike/links/autolinks with backslash escapes.
//!
//! Documented divergences from CommonMark: no setext headings
//! (`---` is always a thematic break), no reference links or link
//! titles, no footnotes, simplified emphasis flanking rules, lists
//! use a fixed content-column indent rule, and blockquotes require
//! the `>` prefix on every line (no lazy quote continuation).

use crate::model::{Block, Doc, Inline, List, ListItem, Table};

/// Parse Markdown into a [`Doc`]. Never fails.
pub fn parse(source: &str) -> Doc {
    // BOM courtesy, same as flowmaid's parsers.
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let lines: Vec<&str> = source.lines().collect();
    Doc {
        blocks: parse_blocks(&lines),
    }
}

// ── Block structure ─────────────────────────────────────────────

fn parse_blocks(lines: &[&str]) -> Vec<Block> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        let line = raw.trim_start();
        if line.is_empty() {
            i += 1;
            continue;
        }

        // Fenced code.
        if let Some((ch, open_len, lang)) = fence_open(line) {
            let mut body = Vec::new();
            let mut j = i + 1;
            while j < lines.len() && !fence_close(lines[j].trim_start(), ch, open_len) {
                body.push(lines[j]);
                j += 1;
            }
            out.push(Block::Code {
                lang,
                source: body.join("\n"),
            });
            i = if j < lines.len() { j + 1 } else { j };
            continue;
        }

        // ATX heading.
        if let Some((level, rest)) = atx_heading(line) {
            out.push(Block::Heading {
                level,
                content: parse_inlines(rest),
            });
            i += 1;
            continue;
        }

        // Thematic break (checked before lists: `- - -` is a rule).
        if is_rule(line) {
            out.push(Block::Rule);
            i += 1;
            continue;
        }

        // Blockquote: strictly `>`-prefixed lines, then recurse.
        if line.starts_with('>') {
            let mut inner = Vec::new();
            let mut j = i;
            while j < lines.len() {
                let l = lines[j].trim_start();
                if let Some(rest) = l.strip_prefix('>') {
                    inner.push(rest.strip_prefix(' ').unwrap_or(rest));
                    j += 1;
                } else {
                    break;
                }
            }
            out.push(Block::Quote(parse_blocks(&inner)));
            i = j;
            continue;
        }

        // Raw HTML block: kept verbatim until a blank line.
        if is_html_open(line) {
            let mut body = Vec::new();
            let mut j = i;
            while j < lines.len() && !lines[j].trim().is_empty() {
                body.push(lines[j]);
                j += 1;
            }
            out.push(Block::Html(body.join("\n")));
            i = j;
            continue;
        }

        // GFM table: header line + delimiter row.
        if line.contains('|')
            && i + 1 < lines.len()
            && is_table_delimiter(lines[i + 1].trim())
        {
            let header = split_row(line);
            let width = header.len().max(1);
            let mut rows = vec![pad_row(header, width)];
            let mut j = i + 2;
            while j < lines.len() {
                let l = lines[j].trim();
                if l.is_empty() || !l.contains('|') {
                    break;
                }
                rows.push(pad_row(split_row(l), width));
                j += 1;
            }
            out.push(Block::Table(Table { rows }));
            i = j;
            continue;
        }

        // List.
        if let Some(marker) = list_marker(raw) {
            let (list, consumed) = parse_list(&lines[i..], marker);
            out.push(Block::List(list));
            i += consumed;
            continue;
        }

        // Paragraph: gather until a blank line or another block form.
        let mut parts: Vec<String> = Vec::new();
        let mut j = i;
        while j < lines.len() {
            let l = lines[j].trim_start();
            if l.is_empty()
                || fence_open(l).is_some()
                || atx_heading(l).is_some()
                || is_rule(l)
                || l.starts_with('>')
                || is_html_open(l)
                || list_marker(lines[j]).is_some()
                || (l.contains('|')
                    && j + 1 < lines.len()
                    && is_table_delimiter(lines[j + 1].trim()))
            {
                break;
            }
            parts.push(hard_break_line(l));
            j += 1;
        }
        // Hard-break lines already end with '\n'; other joins get a
        // single space.
        let mut text = String::new();
        for (k, p) in parts.iter().enumerate() {
            if k > 0 && !text.ends_with('\n') {
                text.push(' ');
            }
            text.push_str(p);
        }
        out.push(Block::Paragraph(parse_inlines(text.trim_end())));
        i = j.max(i + 1);
    }
    out
}

/// Opening code fence (three-or-more backticks or tildes) →
/// (fence char, run length, lowercased first info word).
fn fence_open(line: &str) -> Option<(char, usize, String)> {
    let ch = line.chars().next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let run = line.chars().take_while(|&c| c == ch).count();
    if run < 3 {
        return None;
    }
    let info = line[run..].trim();
    // CommonMark: a backtick fence's info string may not contain `.
    if ch == '`' && info.contains('`') {
        return None;
    }
    let lang = info
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_lowercase();
    Some((ch, run, lang))
}

fn fence_close(line: &str, ch: char, open_len: usize) -> bool {
    let run = line.chars().take_while(|&c| c == ch).count();
    run >= open_len && line[run..].trim().is_empty()
}

/// `#{1..=6}` + space (or end of line) → (level, content) with any
/// standalone trailing `#` run stripped.
fn atx_heading(line: &str) -> Option<(u8, &str)> {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &line[hashes..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None; // '#x' is a paragraph
    }
    let mut content = rest.trim();
    let trailing = content.chars().rev().take_while(|&c| c == '#').count();
    if trailing > 0 {
        let cut = &content[..content.len() - trailing];
        if cut.is_empty() || cut.ends_with(' ') {
            content = cut.trim_end();
        }
    }
    Some((hashes as u8, content))
}

/// 3+ of the SAME `-` / `*` / `_`, spaces allowed, nothing else.
fn is_rule(line: &str) -> bool {
    let mut kind = None;
    let mut count = 0usize;
    for c in line.chars() {
        match c {
            ' ' | '\t' => {}
            '-' | '*' | '_' => {
                if kind.get_or_insert(c) != &c {
                    return false;
                }
                count += 1;
            }
            _ => return false,
        }
    }
    count >= 3
}

fn is_html_open(line: &str) -> bool {
    let mut chars = line.chars();
    if chars.next() != Some('<') {
        return false;
    }
    if !matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '!' || c == '/') {
        return false;
    }
    // `<https://…>` is an autolink, not an HTML block — otherwise a
    // line that is only an autolink is eaten verbatim as raw HTML and
    // any prose after it is swallowed too (bug hunt). A scheme URL is
    // recognised by `://` before the closing `>`.
    if let Some(close) = line.find('>') {
        if line[1..close].contains("://") {
            return false;
        }
    }
    true
}

/// A `| --- | :---: |` style delimiter row.
fn is_table_delimiter(line: &str) -> bool {
    if !line.contains('-') {
        return false;
    }
    line.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// Split a table row into cells on unescaped pipes; outer pipes
/// dropped; `\|` unescaped to a literal pipe.
fn split_row(line: &str) -> Vec<Vec<Inline>> {
    let line = line.trim();
    let line = line.strip_prefix('|').unwrap_or(line);
    let line = line.strip_suffix('|').unwrap_or(line);
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut esc = false;
    for c in line.chars() {
        match (esc, c) {
            (true, '|') => {
                cur.push('|');
                esc = false;
            }
            (true, other) => {
                cur.push('\\');
                cur.push(other);
                esc = false;
            }
            (false, '\\') => esc = true,
            (false, '|') => {
                cells.push(parse_inlines(cur.trim()));
                cur.clear();
            }
            (false, other) => cur.push(other),
        }
    }
    if esc {
        cur.push('\\');
    }
    cells.push(parse_inlines(cur.trim()));
    cells
}

fn pad_row(mut row: Vec<Vec<Inline>>, width: usize) -> Vec<Vec<Inline>> {
    row.truncate(width);
    while row.len() < width {
        row.push(Vec::new());
    }
    row
}

/// Leading indent in columns (a tab counts as 4).
fn indent_of(raw: &str) -> usize {
    let mut n = 0;
    for c in raw.chars() {
        match c {
            ' ' => n += 1,
            '\t' => n += 4,
            _ => break,
        }
    }
    n
}

struct Marker {
    indent: usize,
    /// Column where the item's content starts.
    content_col: usize,
    /// None = unordered; Some(n) = ordered starting at n.
    number: Option<u64>,
}

/// Detect a list marker on a raw (indented) line.
fn list_marker(raw: &str) -> Option<Marker> {
    let indent = indent_of(raw);
    let line = raw.trim_start();
    // Unordered: - * + followed by a space. (`- - -` rules are
    // caught before lists in parse_blocks.)
    for m in ['-', '*', '+'] {
        if let Some(rest) = line.strip_prefix(m) {
            if rest.starts_with(' ') {
                return Some(Marker {
                    indent,
                    content_col: indent + 2,
                    number: None,
                });
            }
        }
    }
    // Ordered: 1-9 digits + '.' or ')' + space.
    let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
    if (1..=9).contains(&digits) {
        let after = &line[digits..];
        if (after.starts_with('.') || after.starts_with(')')) && after[1..].starts_with(' ') {
            let n = line[..digits].parse().ok()?;
            return Some(Marker {
                indent,
                content_col: indent + digits + 2,
                number: Some(n),
            });
        }
    }
    None
}

/// Parse one list starting at `lines[0]` (which carries `first`).
/// Returns the list and how many lines were consumed.
fn parse_list(lines: &[&str], first: Marker) -> (List, usize) {
    let ordered = first.number.is_some();
    let start = first.number;
    let list_indent = first.indent;
    let mut items: Vec<ListItem> = Vec::new();
    let mut item_lines: Vec<String> = Vec::new();
    let mut item_checked: Option<bool> = None;
    let mut content_col = first.content_col;
    let mut i = 0;
    // Blank lines are held back until we know the list continues —
    // trailing blanks after the last item belong to the document.
    let mut pending_blanks = 0usize;

    let flush = |items: &mut Vec<ListItem>, buf: &mut Vec<String>, checked: &mut Option<bool>| {
        if !buf.is_empty() {
            let refs: Vec<&str> = buf.iter().map(String::as_str).collect();
            items.push(ListItem {
                checked: checked.take(),
                blocks: parse_blocks(&refs),
            });
            buf.clear();
        }
    };

    while i < lines.len() {
        let raw = lines[i];
        if raw.trim().is_empty() {
            pending_blanks += 1;
            i += 1;
            continue;
        }
        let ind = indent_of(raw);
        match list_marker(raw) {
            // A new sibling item at the list's indent level (same
            // ordered/unordered family — switching families ends the
            // list; a documented simplification).
            Some(m) if m.indent == list_indent && m.number.is_some() == ordered => {
                flush(&mut items, &mut item_lines, &mut item_checked);
                pending_blanks = 0;
                content_col = m.content_col;
                let content = &raw.trim_start()[(m.content_col - m.indent)..];
                let (checked, content) = task_marker(content);
                item_checked = checked;
                item_lines.push(content.to_string());
                i += 1;
            }
            // Anything indented to the content column belongs to the
            // current item (nested lists, code, more paragraphs).
            _ if ind >= content_col && !item_lines.is_empty() => {
                for _ in 0..pending_blanks {
                    item_lines.push(String::new());
                }
                pending_blanks = 0;
                item_lines.push(dedent(raw, content_col));
                i += 1;
            }
            _ => {
                i -= pending_blanks.min(i); // give trailing blanks back
                break;
            }
        }
    }
    flush(&mut items, &mut item_lines, &mut item_checked);
    (List { start, items }, i)
}

/// GFM task marker at the start of an item's content.
fn task_marker(content: &str) -> (Option<bool>, &str) {
    let b = content.as_bytes();
    if b.len() >= 4 && b[0] == b'[' && b[2] == b']' && b[3] == b' ' {
        match b[1] {
            b' ' => return (Some(false), &content[4..]),
            b'x' | b'X' => return (Some(true), &content[4..]),
            _ => {}
        }
    }
    (None, content)
}

/// Remove up to `cols` columns of leading indentation.
fn dedent(raw: &str, cols: usize) -> String {
    let mut taken = 0;
    let mut idx = 0;
    for (bi, c) in raw.char_indices() {
        if taken >= cols {
            break;
        }
        match c {
            ' ' => taken += 1,
            '\t' => taken += 4,
            _ => break,
        }
        idx = bi + c.len_utf8();
    }
    raw[idx..].to_string()
}

/// A paragraph line, with hard breaks (trailing `\` or 2+ spaces)
/// turned into a literal newline.
fn hard_break_line(line: &str) -> String {
    if let Some(cut) = line.strip_suffix('\\') {
        return format!("{}\n", cut.trim_end());
    }
    if line.ends_with("  ") {
        return format!("{}\n", line.trim_end());
    }
    line.trim_end().to_string()
}

// ── Inline structure ────────────────────────────────────────────

#[derive(Clone, Default)]
struct Style {
    strong: bool,
    em: bool,
    strike: bool,
    link: Option<String>,
}

/// Applies one emphasis delimiter's effect to a style.
type StyleFn = fn(&mut Style);

/// Parse inline content into flattened, style-resolved runs.
pub fn parse_inlines(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    inline_into(text, &Style::default(), &mut out);
    // Merge adjacent identical-style runs for compact output.
    // Empty-text runs are dropped EXCEPT links: `[](url)` must keep
    // its anchor and href (bug hunt / CommonMark).
    let mut merged: Vec<Inline> = Vec::new();
    for r in out {
        if r.text.is_empty() && r.link.is_none() {
            continue;
        }
        if let Some(last) = merged.last_mut() {
            if last.strong == r.strong
                && last.em == r.em
                && last.code == r.code
                && last.strike == r.strike
                && last.link == r.link
            {
                last.text.push_str(&r.text);
                continue;
            }
        }
        merged.push(r);
    }
    merged
}

fn push_text(out: &mut Vec<Inline>, buf: &mut String, st: &Style) {
    if buf.is_empty() {
        return;
    }
    out.push(Inline {
        text: std::mem::take(buf),
        strong: st.strong,
        em: st.em,
        code: false,
        strike: st.strike,
        link: st.link.clone(),
    });
}

fn inline_into(s: &str, st: &Style, out: &mut Vec<Inline>) {
    let b = s.as_bytes();
    let mut buf = String::new();
    let mut i = 0;
    while i < b.len() {
        let rest = &s[i..];

        // Backslash escapes ASCII punctuation.
        if b[i] == b'\\' && i + 1 < b.len() && b[i + 1].is_ascii_punctuation() {
            buf.push(b[i + 1] as char);
            i += 2;
            continue;
        }

        // Code span: N backticks .. N backticks, verbatim inside.
        if b[i] == b'`' {
            let n = rest.bytes().take_while(|&c| c == b'`').count();
            if let Some(end) = find_backtick_run(&s[i + n..], n) {
                let inner = strip_code_pad(&s[i + n..i + n + end]);
                push_text(out, &mut buf, st);
                out.push(Inline {
                    text: inner.to_string(),
                    strong: st.strong,
                    em: st.em,
                    code: true,
                    strike: st.strike,
                    link: st.link.clone(),
                });
                i += n + end + n;
                continue;
            }
        }

        // Strong / em / strike delimiters.
        let delim: Option<(&str, StyleFn)> = if rest.starts_with("**") {
            Some(("**", |st: &mut Style| st.strong = true))
        } else if rest.starts_with("__") {
            Some(("__", |st: &mut Style| st.strong = true))
        } else if rest.starts_with("~~") {
            Some(("~~", |st: &mut Style| st.strike = true))
        } else if rest.starts_with('*') {
            Some(("*", |st: &mut Style| st.em = true))
        } else if rest.starts_with('_') && word_boundary_before(s, i) {
            Some(("_", |st: &mut Style| st.em = true))
        } else {
            None
        };
        if let Some((d, apply)) = delim {
            if let Some(end) = find_emphasis_close(&s[i + d.len()..], d) {
                push_text(out, &mut buf, st);
                let mut inner_style = st.clone();
                apply(&mut inner_style);
                inline_into(&s[i + d.len()..i + d.len() + end], &inner_style, out);
                i += d.len() + end + d.len();
                continue;
            }
        }

        // Link: [text](url).
        if b[i] == b'[' {
            if let Some((text, url, len)) = parse_link(rest) {
                push_text(out, &mut buf, st);
                let inner_style = Style {
                    link: Some(url.clone()),
                    ..st.clone()
                };
                if text.is_empty() {
                    // `[](url)` keeps its anchor: emit an explicit
                    // empty-text link run (the recursion would emit
                    // nothing).
                    out.push(Inline {
                        text: String::new(),
                        strong: st.strong,
                        em: st.em,
                        code: false,
                        strike: st.strike,
                        link: Some(url),
                    });
                } else {
                    inline_into(text, &inner_style, out);
                }
                i += len;
                continue;
            }
        }
        // Autolink: <scheme://...>.
        if b[i] == b'<' {
            if let Some(close) = rest.find('>') {
                let url = &rest[1..close];
                if url.contains("://") && !url.contains(char::is_whitespace) {
                    push_text(out, &mut buf, st);
                    out.push(Inline {
                        text: url.to_string(),
                        strong: st.strong,
                        em: st.em,
                        code: false,
                        strike: st.strike,
                        link: Some(url.to_string()),
                    });
                    i += close + 1;
                    continue;
                }
            }
        }

        // Plain character (multi-byte safe).
        let c = rest.chars().next().unwrap();
        buf.push(c);
        i += c.len_utf8();
    }
    push_text(out, &mut buf, st);
}

/// Byte offset (within `s`) of the next run of EXACTLY `n` backticks.
fn find_backtick_run(s: &str, n: usize) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'`' {
            let run = s[i..].bytes().take_while(|&c| c == b'`').count();
            if run == n {
                return Some(i);
            }
            i += run;
        } else {
            i += 1;
        }
    }
    None
}

/// One space stripped from each side when both are present and the
/// content is not all spaces (CommonMark's code-span rule).
fn strip_code_pad(s: &str) -> &str {
    if s.len() >= 2 && s.starts_with(' ') && s.ends_with(' ') && s.chars().any(|c| c != ' ') {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// Offset of the closing delimiter: content non-empty, opener not
/// followed by whitespace, closer not preceded by whitespace.
fn find_emphasis_close(s: &str, delim: &str) -> Option<usize> {
    let first = s.chars().next()?;
    if first.is_whitespace() {
        return None;
    }
    let mut i = 0;
    while let Some(pos) = s[i..].find(delim) {
        let at = i + pos;
        if at == 0 {
            i = at + delim.len();
            continue;
        }
        let before = s[..at].chars().next_back().unwrap();
        if !before.is_whitespace() {
            // `_` must also close at a word boundary.
            if delim == "_"
                && s[at + 1..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphanumeric())
            {
                i = at + delim.len();
                continue;
            }
            return Some(at);
        }
        i = at + delim.len();
        if i >= s.len() {
            break;
        }
    }
    None
}

/// `_` only opens emphasis at a word boundary.
fn word_boundary_before(s: &str, i: usize) -> bool {
    s[..i]
        .chars()
        .next_back()
        .map_or(true, |c| !c.is_alphanumeric())
}

/// `[text](url)` at the start of `s` → (text, url, total length).
/// Brackets nest in the text; parens nest one level in the url.
fn parse_link(s: &str) -> Option<(&str, String, usize)> {
    let b = s.as_bytes();
    let mut depth = 0usize;
    let mut close = None;
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    if b.get(close + 1) != Some(&b'(') {
        return None;
    }
    let mut depth = 0usize;
    for (i, &c) in b.iter().enumerate().skip(close + 1) {
        match c {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    let url = s[close + 2..i].trim().to_string();
                    return Some((&s[1..close], url, i + 1));
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(inlines: &[Inline]) -> String {
        inlines.iter().map(|r| r.text.as_str()).collect()
    }

    // ── regressions from the bug hunt ──

    #[test]
    fn lone_autolink_is_a_link_not_an_html_block() {
        let d = parse("<https://example.com>");
        let Block::Paragraph(c) = &d.blocks[0] else {
            panic!("got {:?}", d.blocks[0]);
        };
        assert_eq!(c[0].link.as_deref(), Some("https://example.com"));
        // And prose after the autolink is not swallowed as HTML.
        let d = parse("<https://example.com>\nmore prose");
        assert!(matches!(d.blocks[0], Block::Paragraph(_)));
        assert!(!d.blocks.iter().any(|b| matches!(b, Block::Html(_))));
        // A real HTML tag line is still an HTML block.
        assert!(matches!(parse("<div>\nx\n</div>").blocks[0], Block::Html(_)));
    }

    #[test]
    fn empty_text_link_run_is_kept() {
        let r = parse_inlines("before [](https://x.dev) after");
        assert!(r
            .iter()
            .any(|x| x.text.is_empty() && x.link.as_deref() == Some("https://x.dev")));
    }

    #[test]
    fn empty_and_blank_inputs() {
        assert_eq!(parse("").blocks.len(), 0);
        assert_eq!(parse("  \n\n \t\n").blocks.len(), 0);
    }

    #[test]
    fn atx_headings_and_non_headings() {
        let d = parse("# One\n### Three ###\n#nospace\n\n####### seven");
        assert!(
            matches!(&d.blocks[0], Block::Heading { level: 1, content } if text_of(content) == "One")
        );
        assert!(
            matches!(&d.blocks[1], Block::Heading { level: 3, content } if text_of(content) == "Three")
        );
        assert!(matches!(&d.blocks[2], Block::Paragraph(c) if text_of(c).starts_with("#nospace")));
        assert!(matches!(&d.blocks[3], Block::Paragraph(_)), "7 hashes is not a heading");
    }

    #[test]
    fn paragraph_joining_and_hard_breaks() {
        let d = parse("line one\nline two");
        assert!(matches!(&d.blocks[0], Block::Paragraph(c) if text_of(c) == "line one line two"));
        let d = parse("a  \nb\\\nc");
        assert!(matches!(&d.blocks[0], Block::Paragraph(c) if text_of(c) == "a\nb\nc"));
    }

    #[test]
    fn fences_backtick_tilde_info_and_unclosed() {
        let d = parse("```rust ignore\nfn x() {}\n```\n~~~\nplain\n~~~\n```\nunclosed");
        assert!(
            matches!(&d.blocks[0], Block::Code { lang, source } if lang == "rust" && source == "fn x() {}")
        );
        assert!(
            matches!(&d.blocks[1], Block::Code { lang, source } if lang.is_empty() && source == "plain")
        );
        assert!(matches!(&d.blocks[2], Block::Code { source, .. } if source == "unclosed"));
    }

    #[test]
    fn rules_and_precedence_over_setext() {
        let d = parse("---\npara\n***\n* * *\n___");
        assert!(matches!(d.blocks[0], Block::Rule));
        assert!(matches!(d.blocks[2], Block::Rule));
        assert!(matches!(d.blocks[3], Block::Rule), "spaced rule");
        assert!(matches!(d.blocks[4], Block::Rule));
    }

    #[test]
    fn quotes_nest_and_contain_blocks() {
        let d = parse("> # Title\n> body\n> - item\n> > deeper");
        let Block::Quote(inner) = &d.blocks[0] else { panic!() };
        assert!(matches!(inner[0], Block::Heading { level: 1, .. }));
        assert!(matches!(inner[1], Block::Paragraph(_)));
        assert!(matches!(inner[2], Block::List(_)));
        assert!(matches!(inner[3], Block::Quote(_)));
    }

    #[test]
    fn unordered_list_with_tasks_and_nesting() {
        let d = parse("- [x] done\n- [ ] todo\n- plain\n  - nested\n    continued");
        let Block::List(l) = &d.blocks[0] else { panic!() };
        assert_eq!(l.start, None);
        assert_eq!(l.items.len(), 3);
        assert_eq!(l.items[0].checked, Some(true));
        assert_eq!(l.items[1].checked, Some(false));
        assert_eq!(l.items[2].checked, None);
        let nested = l.items[2]
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::List(n) => Some(n),
                _ => None,
            })
            .expect("nested list");
        let Block::Paragraph(c) = &nested.items[0].blocks[0] else { panic!() };
        assert_eq!(text_of(c), "nested continued");
    }

    #[test]
    fn ordered_list_start_and_multiblock_items() {
        let d = parse("3. three\n4. four\n\n   second paragraph\n5. five");
        let Block::List(l) = &d.blocks[0] else { panic!() };
        assert_eq!(l.start, Some(3));
        assert_eq!(l.items.len(), 3);
        assert_eq!(l.items[1].blocks.len(), 2, "item 4 holds two paragraphs");
    }

    #[test]
    fn list_followed_by_paragraph_after_blank() {
        let d = parse("- item\n\nplain paragraph");
        assert!(matches!(d.blocks[0], Block::List(_)));
        assert!(matches!(&d.blocks[1], Block::Paragraph(c) if text_of(c) == "plain paragraph"));
    }

    #[test]
    fn list_item_with_code_fence_inside() {
        let d = parse("- item\n  ```\n  code\n  ```");
        let Block::List(l) = &d.blocks[0] else { panic!() };
        assert!(l.items[0]
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Code { source, .. } if source == "code")));
    }

    #[test]
    fn tables_with_escapes_and_ragged_rows() {
        let d = parse("| A | B |\n|---|---|\n| 1 | a\\|b |\n| only |\n| x | y | extra |");
        let Block::Table(t) = &d.blocks[0] else { panic!() };
        assert_eq!(t.rows.len(), 4);
        assert!(t.rows.iter().all(|r| r.len() == 2), "rectangular");
        assert_eq!(text_of(&t.rows[1][1]), "a|b");
        assert_eq!(text_of(&t.rows[2][1]), "", "short row padded");
    }

    #[test]
    fn table_needs_delimiter_row() {
        let d = parse("a | b\nplain line");
        assert!(matches!(&d.blocks[0], Block::Paragraph(_)));
    }

    #[test]
    fn html_block_verbatim() {
        let d = parse("<div class=\"x\">\n<span>hi</span>\n</div>\n\nafter");
        assert!(matches!(&d.blocks[0], Block::Html(h) if h.contains("<span>hi</span>")));
        assert!(matches!(&d.blocks[1], Block::Paragraph(_)));
    }

    #[test]
    fn inline_strong_em_strike_flatten() {
        let r = parse_inlines("**bold *both* bold** and ~~gone~~ and __b__ _i_");
        assert!(r.iter().any(|x| x.strong && !x.em && x.text.contains("bold")));
        assert!(r.iter().any(|x| x.strong && x.em && x.text == "both"));
        assert!(r.iter().any(|x| x.strike && x.text == "gone"));
        assert!(r.iter().any(|x| x.strong && x.text == "b"));
        assert!(r.iter().any(|x| x.em && x.text == "i"));
    }

    #[test]
    fn underscore_not_intraword_and_unclosed_literal() {
        let r = parse_inlines("snake_case_name stays");
        assert_eq!(text_of(&r), "snake_case_name stays");
        assert!(r.iter().all(|x| !x.em));
        let r = parse_inlines("*unclosed");
        assert_eq!(text_of(&r), "*unclosed");
    }

    #[test]
    fn code_spans_protect_content() {
        let r = parse_inlines("use `let *x* = 1` here");
        let code: Vec<_> = r.iter().filter(|x| x.code).collect();
        assert_eq!(code.len(), 1);
        assert_eq!(code[0].text, "let *x* = 1");
        let r = parse_inlines("`` a`b `` end");
        assert_eq!(r.iter().find(|x| x.code).unwrap().text, "a`b");
    }

    #[test]
    fn links_and_autolinks() {
        let r = parse_inlines("see [the **docs**](https://d.rs/a(1)) or <https://x.dev>");
        assert!(r
            .iter()
            .any(|x| x.strong && x.text == "docs" && x.link.as_deref() == Some("https://d.rs/a(1)")));
        assert!(r.iter().any(|x| x.link.as_deref() == Some("https://x.dev")));
    }

    #[test]
    fn escapes_are_literal() {
        let r = parse_inlines("\\*not em\\* and \\[not link\\]");
        assert_eq!(text_of(&r), "*not em* and [not link]");
        assert!(r.iter().all(|x| !x.em && x.link.is_none()));
    }

    #[test]
    fn bom_and_crlf() {
        let d = parse("\u{feff}# Hi\r\n\r\ntext\r\n");
        assert!(matches!(d.blocks[0], Block::Heading { level: 1, .. }));
        assert!(matches!(&d.blocks[1], Block::Paragraph(c) if text_of(c) == "text"));
    }

    #[test]
    fn adjacent_same_style_runs_merge() {
        let r = parse_inlines("plain \\* still plain");
        assert_eq!(r.len(), 1);
    }
}
