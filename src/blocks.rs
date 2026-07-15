//! Locating and rewriting ```mermaid fences in RAW Markdown text.
//!
//! This is deliberately separate from the parser/AST: a host editor
//! (flowmaid desktop) opens one fence as an editable tab and saves the
//! edit back INTO its fence, so it needs exact BYTE RANGES in the
//! original source — something the layout AST (which drops source
//! spans) cannot give. A small line scanner does the job with no
//! dependency, matching the zero-dep ethos.
//!
//! Only UNINDENTED fences are reported. An indented fence (inside a
//! list item) would need its body re-indented on write-back, which
//! [`splice`] does not attempt — reporting it would let a save corrupt
//! the document.

use std::ops::Range;

/// Is `lang` a mermaid info string? Matches the layout stage.
fn is_mermaid(lang: &str) -> bool {
    lang == "mermaid" || lang == "mmd"
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

/// Every ```mermaid / ~~~mmd fenced block in `md`, in document order.
/// Each entry is `(inner source, byte range of the WHOLE fence)` — the
/// range spans the opening fence line through the closing fence line,
/// so [`splice`] can rewrite just the body while preserving both
/// markers. An unterminated fence is skipped (there is nothing to
/// write back into).
pub fn mermaid_blocks(md: &str) -> Vec<(String, Range<usize>)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < md.len() {
        let line_end = md[i..].find('\n').map_or(md.len(), |k| i + k);
        if let Some((fence_char, fence_run, lang)) = open_fence(&md[i..line_end]) {
            // EVERY fenced block consumes its body up to the matching
            // close — including non-mermaid ones — so a ```mermaid line
            // nested inside e.g. a ```text block is not mistaken for a
            // real block. Only mermaid fences are recorded.
            let body_start = (line_end + 1).min(md.len());
            let mut j = body_start;
            let mut close_start = None;
            while j < md.len() {
                let le = md[j..].find('\n').map_or(md.len(), |k| j + k);
                if is_close_fence(&md[j..le], fence_char, fence_run) {
                    close_start = Some((j, le));
                    break;
                }
                j = le + 1;
            }
            let Some((cs, ce)) = close_start else {
                // Unterminated fence: everything after is its body.
                break;
            };
            if is_mermaid(&lang) {
                let inner = md[body_start..cs].strip_suffix('\n').unwrap_or(&md[body_start..cs]);
                out.push((inner.to_string(), i..ce));
            }
            i = (ce + 1).min(md.len());
            continue;
        }
        i = line_end + 1;
    }
    out
}

/// Replace the body of the `index`-th mermaid block with `src`, keeping
/// its opening and closing fence lines verbatim (fence char, length,
/// and info string all preserved). Returns `None` if that block no
/// longer exists or its fence lines are not both unindented markers
/// (e.g. the source changed under us, or the fence is inside a list).
pub fn splice(md: &str, index: usize, src: &str) -> Option<String> {
    let (_, range) = mermaid_blocks(md).into_iter().nth(index)?;
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
