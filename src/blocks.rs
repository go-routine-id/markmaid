//! Locating and rewriting ```mermaid fences in RAW Markdown text.
//!
//! This is deliberately separate from the parser/AST: a host editor
//! (flowmaid desktop) opens one fence as an editable tab and saves the
//! edit back INTO its fence, so it needs exact BYTE RANGES in the
//! original source — something the layout AST (which drops source
//! spans) cannot give. A small line scanner does the job with no
//! dependency, matching the zero-dep ethos.
//!
//! Two indexings live here, and mixing them up is a real bug:
//!
//! - [`mermaid_fences`] enumerates the mermaid blocks THE PARSER sees,
//!   in the order the layout stage lays them out — nested ones and ones
//!   whose diagram fails to parse included. Entry `i` is the fence a
//!   [`crate::DiagramItem`] names in its `fence` field. A fence that
//!   cannot be written back safely carries `range: None`.
//! - [`mermaid_blocks`] reports only the WRITEABLE subset (both fence
//!   markers at column 0, no blockquote prefix). Its indices are dense,
//!   so they do NOT line up with the rendered diagrams once a document
//!   nests a fence, or once one of them fails to render.
//!
//! A consumer that pairs an on-screen diagram with its source (the
//! flowmaid desktop editor does) must take the ordinal from
//! [`crate::DiagramItem::fence`], index [`mermaid_fences`], and save
//! through [`splice_fence`]. [`mermaid_blocks`]/[`splice`] stay for
//! callers that only ever handle flat documents.
//!
//! Why the enumeration comes from the AST and not from this file's line
//! scanner: a second scanner cannot agree with the parser. It has no
//! notion of raw-HTML blocks (which swallow a fence whole), of a fence
//! left unterminated at end of input (which the parser still closes),
//! or of how a nested body is dedented. The scanner's only job here is
//! to OFFER a byte range, which is accepted only when its text matches
//! the block the parser produced — a mismatch degrades to
//! `range: None`, never to a write at the wrong offset.
//!
//! Why a nested fence is not writeable: its body would need
//! re-indenting on write-back, which [`splice`] does not attempt —
//! rewriting it blind would corrupt the document.

use std::ops::Range;

/// Is `lang` a mermaid info string? Matches the layout stage.
fn is_mermaid(lang: &str) -> bool {
    lang == "mermaid" || lang == "mmd"
}

/// What had to be peeled off a raw line to reach its markdown content:
/// blockquote markers and indentation columns. A `plain` line needed
/// neither, which is what makes its fence safe to write back.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Prefix {
    quotes: usize,
    cols: usize,
}

impl Prefix {
    fn is_plain(&self) -> bool {
        self.quotes == 0 && self.cols == 0
    }
}

/// Strip indentation and any blockquote `>` markers, returning what the
/// parser would see on this line. A tab counts as four columns, and
/// indentation restarts after each `>` — the same reading the block
/// parser uses, so both stages agree on what is a fence.
fn peel(line: &str) -> (Prefix, &str) {
    let mut s = line;
    let mut quotes = 0usize;
    let mut cols = 0usize;
    loop {
        let mut bytes = 0usize;
        let mut width = 0usize;
        for c in s.chars() {
            match c {
                ' ' => {
                    bytes += 1;
                    width += 1;
                }
                '\t' => {
                    bytes += 1;
                    width += 4;
                }
                _ => break,
            }
        }
        s = &s[bytes..];
        cols += width;
        match s.strip_prefix('>') {
            Some(rest) => {
                quotes += 1;
                cols = 0;
                // The one space after `>` is part of the marker, not
                // indentation — the block parser strips exactly one
                // too. Counting it as a column would dedent the body
                // one char too far.
                s = rest.strip_prefix(' ').unwrap_or(rest);
            }
            None => break,
        }
    }
    (Prefix { quotes, cols }, s)
}

/// Undo `prefix` on one body line, so a nested fence hands back the
/// same source the parser feeds to the diagram engine. Best effort: a
/// line indented LESS than its opening fence keeps what it has.
fn undent<'a>(line: &'a str, prefix: &Prefix) -> &'a str {
    let mut s = line;
    for _ in 0..prefix.quotes {
        let t = s.trim_start();
        match t.strip_prefix('>') {
            Some(rest) => s = rest.strip_prefix(' ').unwrap_or(rest),
            None => return s,
        }
    }
    let mut taken = 0usize;
    let mut idx = 0usize;
    for (bi, c) in s.char_indices() {
        if taken >= prefix.cols {
            break;
        }
        match c {
            ' ' => taken += 1,
            '\t' => taken += 4,
            _ => break,
        }
        idx = bi + c.len_utf8();
    }
    &s[idx..]
}

/// An opening fence at column 0: returns `(fence_char, run_len, lang)`
/// where `lang` is the lowercased first word of the info string.
fn open_fence(line: &str) -> Option<(u8, usize, String)> {
    let c = *line.as_bytes().first()?;
    if c != b'`' && c != b'~' {
        return None;
    }
    let run = line.bytes().take_while(|&x| x == c).count();
    if run < 3 {
        return None;
    }
    let lang = line[run..]
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    Some((c, run, lang))
}

/// Does `line` close a fence opened with `open_char` × `open_run`?
/// A closing fence is the same char, at least as long, at column 0,
/// with nothing but whitespace after it.
fn is_close_fence(line: &str, open_char: u8, open_run: usize) -> bool {
    if line.as_bytes().first() != Some(&open_char) {
        return false;
    }
    let run = line.bytes().take_while(|&x| x == open_char).count();
    run >= open_run && line[run..].trim().is_empty()
}

/// One mermaid fence of a document, in layout order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MermaidFence {
    /// The fence body exactly as the diagram engine receives it —
    /// taken from the parsed block, so a nested fence's dedent is the
    /// parser's, not this module's approximation of it.
    pub source: String,
    /// Byte range of the WHOLE fence (opening marker line through
    /// closing marker line) when it can be written back with
    /// [`splice_fence`]; `None` when no fence in the raw text could be
    /// matched to this block safely — a nested fence, one left
    /// unterminated, or one the scanner and the parser read differently.
    pub range: Option<Range<usize>>,
}

/// Mermaid block sources in the order [`crate::layout`] walks them:
/// document order, descending into quotes and list items exactly as
/// the layout stage does.
fn ast_sources(blocks: &[crate::model::Block], out: &mut Vec<String>) {
    use crate::model::Block;
    for b in blocks {
        match b {
            Block::Code { lang, source, .. } if is_mermaid(lang) => out.push(source.clone()),
            Block::Quote(inner) => ast_sources(inner, out),
            Block::List(list) => {
                for item in &list.items {
                    ast_sources(&item.blocks, out);
                }
            }
            _ => {}
        }
    }
}

/// A fence the raw-text scanner found: its body as the scanner reads
/// it, and the byte range it could be written back into.
struct Candidate {
    source: String,
    range: Option<Range<usize>>,
}

/// Scan the raw text for fences. This sees things the parser does not
/// (a fence inside a raw-HTML block) and misses things the parser has
/// (a fence left unterminated), so its output is only ever CANDIDATE
/// ranges — [`mermaid_fences`] decides which ones are real.
fn scan_fences(md: &str) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < md.len() {
        let line_end = md[i..].find('\n').map_or(md.len(), |k| i + k);
        let (prefix, logical) = peel(&md[i..line_end]);
        if let Some((fence_char, fence_run, lang)) = open_fence(logical) {
            // EVERY fenced block consumes its body up to the matching
            // close — including non-mermaid ones — so a ```mermaid line
            // nested inside e.g. a ```text block is not mistaken for a
            // real block. Only mermaid fences are recorded.
            let body_start = (line_end + 1).min(md.len());
            let mut j = body_start;
            let mut close = None;
            while j < md.len() {
                let le = md[j..].find('\n').map_or(md.len(), |k| j + k);
                if is_close_fence(peel(&md[j..le]).1, fence_char, fence_run) {
                    close = Some((j, le));
                    break;
                }
                j = le + 1;
            }
            let Some((cs, ce)) = close else {
                // Unterminated: there is no closing marker to preserve,
                // so nothing here can be written back into.
                break;
            };
            if is_mermaid(&lang) {
                let raw = &md[body_start..cs];
                let raw = raw.strip_suffix('\n').unwrap_or(raw);
                let source = if prefix.is_plain() {
                    raw.to_string()
                } else {
                    raw.split('\n')
                        .map(|l| undent(l, &prefix))
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                // Writeable only when BOTH markers sit plainly at column
                // 0: splice preserves the marker lines verbatim and does
                // not re-indent the body it writes between them.
                let writeable = prefix.is_plain() && peel(&md[cs..ce]).0.is_plain();
                out.push(Candidate {
                    source,
                    range: writeable.then_some(i..ce),
                });
            }
            i = (ce + 1).min(md.len());
            continue;
        }
        i = line_end + 1;
    }
    out
}

/// Every mermaid block of `md`, in the order the layout stage lays them
/// out — nested fences and ones whose diagram fails to parse included.
///
/// Entry `i` is the fence that [`crate::DiagramItem::fence`] names, which
/// is what makes this the right list to index when pairing an on-screen
/// diagram with its source. Each entry carries a write-back range only
/// when a fence in the raw text matches it exactly; otherwise `range` is
/// `None` and [`splice_fence`] refuses, rather than writing blind.
pub fn mermaid_fences(md: &str) -> Vec<MermaidFence> {
    let mut sources = Vec::new();
    ast_sources(&crate::parser::parse(md).blocks, &mut sources);

    // Walk both lists forward together. A candidate the parser never
    // produced (a fence inside a raw-HTML block) is stepped over; a
    // block the scanner never saw (an unterminated fence) simply gets
    // no range. The cursor only moves forward, so a range is never
    // reused or handed to an earlier block.
    let candidates = scan_fences(md);
    let mut cursor = 0usize;
    sources
        .into_iter()
        .map(|source| {
            let hit = candidates[cursor..]
                .iter()
                .position(|c| c.source == source)
                .map(|k| cursor + k);
            let range = match hit {
                Some(k) => {
                    cursor = k + 1;
                    candidates[k].range.clone()
                }
                None => None,
            };
            MermaidFence { source, range }
        })
        .collect()
}

/// The WRITEABLE mermaid fences of `md`, in document order: each entry
/// is `(inner source, byte range of the WHOLE fence)`, so [`splice`] can
/// rewrite just the body while preserving both markers.
///
/// Nested fences are omitted, which means these indices do NOT line up
/// with the rendered diagrams — use [`mermaid_fences`] when the index
/// has to match what is on screen.
pub fn mermaid_blocks(md: &str) -> Vec<(String, Range<usize>)> {
    mermaid_fences(md)
        .into_iter()
        .filter_map(|f| f.range.map(|r| (f.source, r)))
        .collect()
}

/// Replace the body of the `index`-th WRITEABLE mermaid block (see
/// [`mermaid_blocks`]) with `src`, keeping its opening and closing fence
/// lines verbatim. Returns `None` if that block no longer exists.
pub fn splice(md: &str, index: usize, src: &str) -> Option<String> {
    let (_, range) = mermaid_blocks(md).into_iter().nth(index)?;
    splice_range(md, range, src)
}

/// Replace the body of the `index`-th mermaid fence (see
/// [`mermaid_fences`] — the indexing that matches the rendered
/// diagrams) with `src`. Returns `None` when that fence no longer
/// exists or is not writeable, so a nested fence fails loudly instead
/// of corrupting the document.
pub fn splice_fence(md: &str, index: usize, src: &str) -> Option<String> {
    let range = mermaid_fences(md).into_iter().nth(index)?.range?;
    splice_range(md, range, src)
}

/// Rewrite the body between the fence markers spanned by `range`,
/// preserving both marker lines (fence char, length, info string).
fn splice_range(md: &str, range: Range<usize>, src: &str) -> Option<String> {
    let block = &md[range.clone()];
    let open_len = block.find('\n')?;
    let close_start = block.rfind('\n')?;
    let (open, close) = (&block[..open_len], &block[close_start + 1..]);
    let fence = |s: &str| s.starts_with("```") || s.starts_with("~~~");
    if !fence(open) || !fence(close) {
        return None;
    }
    let mut out = String::with_capacity(md.len() + src.len());
    out.push_str(&md[..range.start]);
    out.push_str(open);
    out.push('\n');
    out.push_str(src.trim_end_matches('\n'));
    out.push('\n');
    out.push_str(close);
    out.push_str(&md[range.end..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MD: &str = "# Judul\n\nteks\n\n```mermaid\nflowchart TD\nA-->B\n```\n\n\
                      antara\n\n```js\nconsole.log(1)\n```\n\n\
                      ~~~mermaid\npie\n\"x\" : 1\n~~~\n\npenutup\n";

    #[test]
    fn extracts_only_mermaid_fences_in_order() {
        let b = mermaid_blocks(MD);
        assert_eq!(b.len(), 2, "dua blok mermaid, fence js dilewati");
        assert_eq!(b[0].0, "flowchart TD\nA-->B");
        assert_eq!(b[1].0, "pie\n\"x\" : 1");
        // Range menutup seluruh fence, marker ikut.
        assert!(MD[b[0].1.clone()].starts_with("```mermaid"));
        assert!(MD[b[0].1.clone()].ends_with("```"));
    }

    #[test]
    fn splice_rewrites_one_block_only() {
        let out = splice(MD, 1, "pie\n\"y\" : 9").unwrap();
        assert!(out.contains("~~~mermaid\npie\n\"y\" : 9\n~~~"));
        assert!(out.contains("A-->B"), "blok #0 utuh");
        assert!(out.contains("console.log(1)") && out.contains("penutup"));
    }

    #[test]
    fn splice_missing_index_is_none() {
        assert!(splice(MD, 9, "x").is_none());
    }

    #[test]
    fn mermaid_nested_in_another_fence_is_not_reported() {
        // A ```mermaid example wrapped in a longer (````) fence must be
        // treated as that block's body, not a real diagram (else splice
        // corrupts it). Only the real top-level block is reported.
        let md = "````text\n```mermaid\nflowchart TD\nA-->B\n```\n````\n\n\
                  ```mermaid\npie\n\"x\" : 1\n```\n";
        let b = mermaid_blocks(md);
        assert_eq!(b.len(), 1, "only the real top-level mermaid block");
        assert_eq!(b[0].0, "pie\n\"x\" : 1");
    }

    #[test]
    fn mermaid_inside_bare_closed_text_fence_is_not_reported() {
        // ```text ... ```mermaid ... ``` — the bare ``` closes the text
        // block; the inner ```mermaid line was its body, not a diagram.
        let md = "```text\n```mermaid\nA-->B\n```\n";
        assert!(mermaid_blocks(md).is_empty());
    }

    #[test]
    fn fence_indices_line_up_with_rendered_diagrams() {
        // The regression this API exists for: a fence nested in a list
        // item still renders as a diagram, so a scanner that skips it
        // shifts every later index. A consumer pairing "diagram #0 on
        // screen" with "source #0" would then edit the WRONG block.
        let md = "- item\n  ```mermaid\n  flowchart TD\n  A-->B\n  ```\n\n\
                  ```mermaid\npie\n\"x\" : 1\n```\n";

        let scene = crate::layout(&crate::parse(md), &crate::LayoutOptions::default());
        let drawn = scene
            .items
            .iter()
            .filter(|i| matches!(i, crate::Item::Diagram(_)))
            .count();
        assert_eq!(drawn, 2, "both fences render");

        let fences = mermaid_fences(md);
        assert_eq!(fences.len(), drawn, "one entry per rendered diagram");
        // Nested body comes back dedented — exactly what was laid out.
        assert_eq!(fences[0].source, "flowchart TD\nA-->B");
        assert!(fences[0].range.is_none(), "nested fence is not writeable");
        assert_eq!(fences[1].source, "pie\n\"x\" : 1");
        assert!(fences[1].range.is_some());

        // The writeable-subset view is still one short — that is the
        // trap, now documented rather than silent.
        assert_eq!(mermaid_blocks(md).len(), 1);
    }

    /// How many diagrams the layout stage actually draws for `md`,
    /// and what ordinals it stamped on them.
    fn drawn(md: &str) -> Vec<usize> {
        let scene = crate::layout(&crate::parse(md), &crate::LayoutOptions::default());
        scene
            .items
            .iter()
            .filter_map(|i| match i {
                crate::Item::Diagram(d) => Some(d.fence),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_broken_diagram_still_holds_its_index() {
        // A fence that fails to parse renders as an error card, NOT an
        // Item::Diagram. Counting diagrams would therefore hand index 0
        // to the *second* fence and save an edit into the broken first
        // one — and mid-edit, broken is the normal state.
        let md = "```mermaid\nbukan diagram !!!\n```\n\n```mermaid\npie\n\"x\" : 1\n```\n";
        let fences = mermaid_fences(md);
        assert_eq!(fences.len(), 2, "both fences are mermaid blocks");
        assert_eq!(drawn(md), vec![1], "only the valid one draws, as fence #1");
        assert_eq!(fences[1].source, "pie\n\"x\" : 1");
        // Saving through the ordinal the scene reported hits the right
        // block, and leaves the broken one untouched.
        let out = splice_fence(md, 1, "pie\n\"z\" : 2").expect("writeable");
        assert!(out.contains("\"z\" : 2") && out.contains("bukan diagram !!!"));
    }

    #[test]
    fn fence_inside_a_raw_html_block_is_not_a_fence_at_all() {
        // The parser swallows `<div>` … up to the blank line as one
        // verbatim Html block, so there is no mermaid block here. The
        // line scanner sees one anyway; it must not become a phantom
        // index, and above all splice must not write into it.
        let md = "<div>\n```mermaid\npie\n\"x\" : 1\n```\n</div>\n";
        assert!(drawn(md).is_empty(), "nothing renders");
        assert!(mermaid_fences(md).is_empty(), "no phantom entry");
        assert!(
            splice_fence(md, 0, "pie\n\"z\" : 2").is_none(),
            "must never rewrite text inside a raw-HTML block"
        );
    }

    #[test]
    fn unterminated_last_fence_keeps_its_entry() {
        // The parser closes an unterminated fence at end of input and
        // lays it out, so it owns an index. There is no closing marker
        // to preserve, so it is simply not writeable — the save fails
        // loudly instead of silently landing on the wrong block.
        let md = "```mermaid\npie\n\"x\" : 1\n```\n\n```mermaid\nflowchart TD\nA-->B\n";
        let fences = mermaid_fences(md);
        assert_eq!(fences.len(), 2);
        assert_eq!(drawn(md), vec![0, 1], "both render");
        assert!(fences[0].range.is_some(), "the closed one is writeable");
        assert_eq!(fences[1].source, "flowchart TD\nA-->B");
        assert!(fences[1].range.is_none(), "no closing marker to preserve");
        assert!(splice_fence(md, 1, "flowchart TD\nC-->D").is_none());
    }

    #[test]
    fn source_is_the_parsers_text_not_the_scanners_guess() {
        // Indentation is semantic for mindmap: handing back a body
        // dedented one column too far would open a DIFFERENT diagram in
        // the editor than the one on screen. The source now comes from
        // the parsed block, so it matches by construction.
        let md = "> ```mermaid\n> mindmap\n>   root\n>     child\n> ```\n";
        let fences = mermaid_fences(md);
        assert_eq!(fences.len(), 1);
        assert_eq!(fences[0].source, "mindmap\n  root\n    child");
        assert!(fences[0].range.is_none(), "quoted fence is not writeable");
    }

    #[test]
    fn splice_fence_refuses_a_nested_fence() {
        let md = "- item\n  ```mermaid\n  A-->B\n  ```\n";
        assert_eq!(mermaid_fences(md).len(), 1);
        assert!(
            splice_fence(md, 0, "C-->D").is_none(),
            "a nested fence fails loudly instead of corrupting the file"
        );
    }

    #[test]
    fn splice_fence_matches_splice_on_a_flat_document() {
        // With no nesting the two indexings coincide, so an existing
        // caller sees no change in behaviour.
        assert_eq!(mermaid_fences(MD).len(), mermaid_blocks(MD).len());
        assert_eq!(splice_fence(MD, 1, "pie\n\"y\" : 9"), splice(MD, 1, "pie\n\"y\" : 9"));
        assert!(splice_fence(MD, 9, "x").is_none());
    }

    #[test]
    fn quoted_fence_is_reported_dedented_but_not_writeable() {
        let md = "> ```mermaid\n> flowchart TD\n> A-->B\n> ```\n";
        let fences = mermaid_fences(md);
        assert_eq!(fences.len(), 1);
        assert_eq!(fences[0].source, "flowchart TD\nA-->B");
        assert!(fences[0].range.is_none());
        assert!(mermaid_blocks(md).is_empty());
    }

    #[test]
    fn indented_fence_is_not_reported() {
        // Fence di dalam list item (ter-indentasi) tidak aman ditulis
        // balik, jadi tidak diekstrak.
        let md = "- item\n  ```mermaid\n  A-->B\n  ```\n";
        assert!(mermaid_blocks(md).is_empty());
    }

    #[test]
    fn unterminated_fence_is_skipped() {
        let md = "```mermaid\nflowchart TD\nA-->B\n";
        assert!(mermaid_blocks(md).is_empty());
    }

    #[test]
    fn empty_body_block() {
        let md = "```mermaid\n```\n";
        let b = mermaid_blocks(md);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].0, "");
    }
}
