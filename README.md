# markmaid

A framework-agnostic Markdown **rendering engine** in pure Rust — sister crate to [flowmaid](https://crates.io/crates/flowmaid), built on the same philosophy: hand-written parser, layout to plain positioned geometry, and painters that only draw primitives. No external dependencies (the only dependency *is* flowmaid, which itself has none).

```text
markdown text ──parse──▶ Doc (AST) ──layout──▶ DocScene ──▶ to_svg() (built in)
                                                  │         to_html() (built in)
                                                  │         egui / iced / GTK / canvas painter (~100 lines each)
                                                  └── ```mermaid blocks become embedded
                                                      flowmaid diagram scenes, first-class
```

Most Rust markdown crates stop at HTML, and every GUI toolkit grows its own renderer. markmaid takes the flowmaid route instead: **compute final geometry once, as data** — positioned text runs (with style flags and semantic color *roles*), rects, lines, link hit-zones, heading anchors — and let any consumer paint it. The same `DocScene` renders to SVG, to an egui canvas, or to anything with a `draw_text`/`draw_rect`.

## Usage

```rust
// One-call conveniences:
let svg  = markmaid::render_svg("# Hello\n\nSome **bold** text.", 720.0);
let html = markmaid::render_html("# Hello\n\n```mermaid\nflowchart TD\nA-->B\n```");

// Or the full pipeline, for interactive apps:
let doc   = markmaid::parse(source);
let scene = markmaid::layout(&doc, &markmaid::LayoutOptions { width: 720.0, ..Default::default() });
for item in &scene.items { /* paint Text / Rect / Line / Diagram */ }
for link in &scene.links { /* hit-test clicks, open urls */ }
```

- Colors are **roles** (`Text`, `Link`, `CodeBg`, …), not values — themes belong to the consumer; `role_color()` provides the default light palette (matching flowmaid's ink family).
- Text metrics are **estimated** (the same character-class table flowmaid uses), which is what makes zero-dependency layout possible. Long CJK runs or exotic fonts can wrap slightly differently than a consumer's real font — the same documented trade-off flowmaid has always had.
- ```` ```mermaid ```` fences are laid out by the flowmaid engine and embedded as `DiagramItem`s: consumers that already paint flowmaid scenes reuse those painters verbatim; the SVG writer nests the engine's own output, so exports are pixel-identical. A block that fails to parse renders as an error card with the engine's line-numbered message.

## Supported Markdown (honest subset)

ATX headings (`#`–`######`), paragraphs with lazy continuation and hard breaks (trailing `\` or two spaces), fenced code (``` and `~~~`, info strings), blockquotes (nested, containing any block), unordered/ordered lists with nesting and GFM task markers (`- [x]`), GFM tables (with `\|` escapes), thematic breaks, raw HTML blocks (kept verbatim, shown as code — never interpreted), and inline `**strong**`, `*em*`, `` `code` ``, `~~strike~~`, `[links](url)`, `<autolinks>`, backslash escapes.

Not supported (by design, documented): setext headings, reference-style links, link titles, footnotes, inline images (planned as geometry placeholders), and full CommonMark emphasis corner cases. The parser is infallible — anything unrecognised degrades to plain text, never an error.

## Status

Early. API may move before 1.0. Part of the [go-routine](https://go-routine-id.github.io/) open-source family.

## License

GPL-3.0-or-later — same as flowmaid. Full text in `LICENSE`.
