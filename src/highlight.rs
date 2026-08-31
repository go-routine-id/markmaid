//! Optional tree-sitter based syntax highlighting for fenced code blocks.
//!
//! This module is only compiled when the `syntax-tree-sitter` feature is
//! enabled. `layout_code()` calls `code_spans()` through a `#[cfg]` gate.
//!
//! HONEST SCOPE: **Rust is the only grammar compiled in today.** Every
//! other info string returns `None` and renders as plain code — the
//! documented degrade, never an error. The `grammars` table below is
//! the whole list:
//! adding a language is one row there plus its `tree-sitter-*` crate in
//! `Cargo.toml`, and nothing else in this module changes.

use std::collections::HashMap;
use std::sync::OnceLock;

use tree_sitter::Language;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

use crate::scene::ColorRole;

/// Capture names that tree-sitter is allowed to emit. The order here
/// determines the index returned in `HighlightEvent::HighlightStart(idx)`.
const CAPTURES: &[&str] = &[
    "comment",
    "line_comment",
    "block_comment",
    "doc_comment",
    "keyword",
    "keyword.control",
    "keyword.function",
    "keyword.operator",
    "keyword.return",
    "keyword.import",
    "keyword.conditional",
    "keyword.repeat",
    "conditional",
    "repeat",
    "include",
    "exception",
    "string",
    "string.documentation",
    "string.regex",
    "string.special",
    "character",
    "character.special",
    "number",
    "integer",
    "float",
    "type",
    "type.builtin",
    "type.definition",
    "type.super",
    "namespace",
    "module",
    "function",
    "function.builtin",
    "function.call",
    "function.macro",
    "function.method",
    "function.method.call",
    "method",
    "constructor",
];

fn capture_to_role(name: &str) -> ColorRole {
    match name {
        "comment" | "line_comment" | "block_comment" | "doc_comment" => ColorRole::CodeComment,
        "keyword"
        | "keyword.control"
        | "keyword.function"
        | "keyword.operator"
        | "keyword.return"
        | "keyword.import"
        | "keyword.conditional"
        | "keyword.repeat"
        | "conditional"
        | "repeat"
        | "include"
        | "exception" => ColorRole::CodeKeyword,
        "string" | "string.documentation" | "string.regex" | "string.special" | "character"
        | "character.special" => ColorRole::CodeString,
        "number" | "integer" | "float" => ColorRole::CodeNumber,
        "type" | "type.builtin" | "type.definition" | "type.super" | "namespace" | "module" => {
            ColorRole::CodeType
        }
        "function"
        | "function.builtin"
        | "function.call"
        | "function.macro"
        | "function.method"
        | "function.method.call"
        | "method"
        | "constructor" => ColorRole::CodeFunction,
        _ => ColorRole::CodeText,
    }
}

/// The grammars compiled into this build, as
/// `(info string, grammar, highlights query)`. THE list — add a row
/// (and the matching `tree-sitter-*` dependency) to support a language.
/// A grammar whose query fails to compile is skipped, so one bad row
/// cannot take the whole highlighter down with it.
fn grammars() -> Vec<(&'static str, Language, &'static str)> {
    vec![(
        "rust",
        Language::new(tree_sitter_rust::LANGUAGE),
        tree_sitter_rust::HIGHLIGHTS_QUERY,
    )]
}

fn build_config(
    name: &'static str,
    language: Language,
    highlights: &str,
) -> Option<HighlightConfiguration> {
    let mut config = HighlightConfiguration::new(
        language,
        name,
        highlights,
        "", // injection queries not needed for block rendering
        "", // locals queries not needed
    )
    .ok()?;
    config.configure(CAPTURES);
    Some(config)
}

fn configs() -> &'static HashMap<&'static str, HighlightConfiguration> {
    static CONFIGS: OnceLock<HashMap<&str, HighlightConfiguration>> = OnceLock::new();
    CONFIGS.get_or_init(|| {
        let mut m = HashMap::new();
        for (name, language, highlights) in grammars() {
            if let Some(cfg) = build_config(name, language, highlights) {
                m.insert(name, cfg);
            }
        }
        m
    })
}

/// Info strings this build can highlight, sorted. A consumer can show
/// this instead of guessing which fences will get colour.
pub fn supported_languages() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = configs().keys().copied().collect();
    v.sort_unstable();
    v
}

pub(crate) fn code_spans(
    source: &str,
    lang: &str,
) -> Option<Vec<(std::ops::Range<usize>, ColorRole)>> {
    let config = configs().get(lang)?;

    let mut highlighter = Highlighter::new();
    let events = highlighter
        .highlight(config, source.as_bytes(), None, |_| None)
        .ok()?;

    let mut stack: Vec<usize> = Vec::new();
    let mut out: Vec<(std::ops::Range<usize>, ColorRole)> = Vec::new();

    for event in events {
        match event.ok()? {
            HighlightEvent::Source { start, end } => {
                if start == end {
                    continue;
                }
                let role = stack
                    .last()
                    .and_then(|idx| CAPTURES.get(*idx))
                    .map(|name| capture_to_role(name))
                    .unwrap_or(ColorRole::CodeText);
                out.push((start..end, role));
            }
            HighlightEvent::HighlightStart(h) => stack.push(h.0),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
        }
    }

    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_spans_returns_some_for_rust() {
        let spans = code_spans("fn main() {}", "rust").expect("rust should be supported");
        assert!(!spans.is_empty());
        assert!(
            spans.iter().any(|(_, r)| *r == ColorRole::CodeKeyword),
            "expected at least one keyword span"
        );
    }

    #[test]
    fn supported_languages_is_the_grammar_table() {
        // Honest scope: whatever GRAMMARS holds is exactly what gets
        // colour — today that is Rust alone.
        assert_eq!(supported_languages(), vec!["rust"]);
        for lang in supported_languages() {
            assert!(code_spans("", lang).is_some(), "{} configured", lang);
        }
    }

    #[test]
    fn code_spans_returns_none_for_unsupported_lang() {
        assert!(code_spans("x", "zig").is_none());
    }

    #[test]
    fn code_spans_handles_multiline_comments() {
        let source = "// line one\n// line two\nfn x() {}";
        let spans = code_spans(source, "rust").unwrap();
        assert!(spans.iter().any(|(range, role)| {
            *role == ColorRole::CodeComment && source[range.clone()].contains("line one")
        }));
    }
}
