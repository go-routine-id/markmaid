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
//! - [`mermaid_fences`] reports EVERY mermaid fence in document order,
//!   nested ones included, so entry `i` lines up with the `i`-th
//!   [`crate::Item::Diagram`] the layout stage emits. A fence that
//!   cannot be written back safely carries `range: None`.
//! - [`mermaid_blocks`] reports only the WRITEABLE subset (both fence
//!   markers at column 0, no blockquote prefix). Its indices are dense,
//!   so they do NOT line up with the rendered diagrams once a document
//!   nests a fence inside a list item or a quote.
//!
//! A consumer that pairs an on-screen diagram with its source (the
//! flowmaid desktop editor does) must index [`mermaid_fences`] and save
//! through [`splice_fence`]. [`mermaid_blocks`]/[`splice`] stay for
//! callers that only ever handle flat documents.
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
                s = rest;
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

/// One mermaid fence found in the raw source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MermaidFence {
    /// The fence body as the diagram engine receives it: a fence nested
    /// in a list item or a quote has that prefix removed, so this is the
    /// same text the layout stage laid out.
    pub source: String,
    /// Byte range of the WHOLE fence (opening marker line through
    /// closing marker line) when it can be written back with
    /// [`splice_fence`]; `None` for a nested fence, whose body would
    /// need re-indenting on write-back.
    pub range: Option<Range<usize>>,
}

/// Every ```mermaid / ~~~mmd fence in `md`, in document order —
/// INCLUDING ones nested in a list item or a blockquote.
///
/// Entry `i` corresponds to the `i`-th diagram the layout stage emits,
/// which is what makes this the right list to index when pairing an
/// on-screen diagram with its source. An unterminated fence ends the
/// scan (there is nothing to write back into).
pub fn mermaid_fences(md: &str) -> Vec<MermaidFence> {
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
                // Unterminated fence: everything after is its body.
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
                out.push(MermaidFence {
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
