// Layout code walks parallel arrays (column widths, row heights) by
// index on purpose — iterator zips would obscure the math, not
// clarify it. Same stance as flowmaid.
#![allow(clippy::needless_range_loop)]

//! markmaid — a framework-agnostic Markdown rendering engine.
//!
//! Sister crate to [flowmaid](https://crates.io/crates/flowmaid) and
//! built on the same philosophy: a hand-written parser (a documented
//! GFM subset — no external markdown crate), a layout stage that
//! produces FINAL geometry as plain data, and writers/painters that
//! only draw primitives. One geometry source, any GUI toolkit:
//!
//! ```text
//! markdown text ──parse──▶ Doc (AST) ──layout──▶ DocScene ──▶ SVG writer (built in)
//!                                                    │        egui / iced / GTK painter
//!                                                    │        web canvas
//!                                                    └─ inline ```mermaid blocks are
//!                                                       flowmaid scenes, first-class
//! ```
//!
//! ```
//! let svg = markmaid::render_svg("# Hello\n\nSome **bold** text.", 640.0);
//! assert!(svg.starts_with("<svg"));
//! ```
//!
//! The only dependency is `flowmaid` itself (zero-dependency, same
//! family) — mermaid fences become embedded diagram geometry.

pub mod html;
pub mod layout;
pub mod model;
pub mod parser;
pub mod scene;

pub use model::{Block, Doc, Inline};
pub use scene::{
    ColorRole, DiagramItem, DiagramView, DocScene, ImageItem, Item, LayoutOptions, LineItem,
    LinkZone, RectItem, TextRun,
};

/// Parse Markdown (GFM subset) into a [`Doc`]. Never fails: unknown
/// constructs degrade to paragraphs or verbatim blocks.
pub fn parse(source: &str) -> Doc {
    parser::parse(source)
}

/// Lay a parsed document out into framework-neutral geometry.
pub fn layout(doc: &Doc, opts: &LayoutOptions) -> DocScene {
    layout::layout(doc, opts)
}

/// Markdown → standalone SVG document (default palette).
pub fn render_svg(source: &str, width: f64) -> String {
    scene::render_svg(source, width)
}

/// Markdown → semantic HTML fragment. Mermaid blocks are inlined as
/// SVG `<figure>`s; a block that fails to parse becomes a
/// line-numbered `<pre class="markmaid-error">` instead of breaking
/// the page.
pub fn render_html(source: &str) -> String {
    html::render_html(source)
}
