//! DocScene: the FRAMEWORK-NEUTRAL geometry of a laid-out document —
//! markmaid's equivalent of flowmaid's `Scene`. The layout stage
//! computes final positions once; every consumer (the built-in SVG
//! writer, an egui painter, a web canvas, iced, GTK, ...) just draws
//! the primitives in order.
//!
//! Conventions:
//! - Coordinates are in CSS-like pixels; `(0, 0)` is the top-left of
//!   the document, `y` grows downward.
//! - [`TextRun::y`] is the TOP of the run's line box; the run's font
//!   size is [`TextRun::size`] (baseline ≈ `y + 0.8 * size`).
//! - Colors are ROLES, not values — themes belong to the consumer.
//!   [`role_color`] provides the default light-paper palette that
//!   the SVG writer uses.

/// Layout inputs. `width` is the full document width including the
/// outer margins; text wraps to fit it.
#[derive(Debug, Clone)]
pub struct LayoutOptions {
    pub width: f64,
    /// Base font size for body text (headings scale from this).
    pub base_size: f64,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        LayoutOptions {
            width: 720.0,
            base_size: 14.0,
        }
    }
}

/// A laid-out document: paint `items` in order. `links` are hit-test
/// zones for interactivity; `anchors` map headings to y offsets
/// (tables of contents, scroll-to-section).
#[derive(Debug, Default)]
pub struct DocScene {
    pub width: f64,
    pub height: f64,
    pub items: Vec<Item>,
    pub links: Vec<LinkZone>,
    pub anchors: Vec<Anchor>,
}

/// One paint primitive.
#[derive(Debug)]
pub enum Item {
    Text(TextRun),
    Rect(RectItem),
    /// A horizontal or vertical line (table grid, thematic break).
    Line(LineItem),
    /// An inline mermaid diagram, laid out by the flowmaid engine.
    Diagram(DiagramItem),
}

/// Positioned styled text. Never contains `\n` — the layout stage
/// splits lines and wraps.
#[derive(Debug)]
pub struct TextRun {
    pub x: f64,
    /// Top of the line box.
    pub y: f64,
    pub size: f64,
    pub mono: bool,
    pub strong: bool,
    pub em: bool,
    pub strike: bool,
    pub underline: bool,
    pub role: ColorRole,
    pub text: String,
}

#[derive(Debug)]
pub struct RectItem {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub rounding: f64,
    pub fill: Option<ColorRole>,
    pub stroke: Option<ColorRole>,
}

#[derive(Debug)]
pub struct LineItem {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    pub role: ColorRole,
}

/// An embedded diagram: the engine scene plus where/how large to
/// paint it. `scale` fits the diagram to the column width (never
/// enlarges past 1.0). Consumers translate by `(x, y)` and scale
/// uniformly — exactly the `ts` closure pattern flowmaid painters
/// already use.
#[derive(Debug)]
pub struct DiagramItem {
    pub x: f64,
    pub y: f64,
    pub scale: f64,
    /// Unscaled engine-space size (width, height).
    pub size: (f64, f64),
    pub view: Box<DiagramView>,
}

/// The flowmaid geometry of one diagram, by type. Consumers that
/// already paint flowmaid scenes (desktop, web) reuse those painters
/// verbatim.
#[derive(Debug)]
pub enum DiagramView {
    /// Flowcharts and state diagrams (both live on `Scene`).
    Flow(flowmaid::scene::Scene),
    Er(flowmaid::er::ErScene),
    Class(flowmaid::class::ClassScene),
    Seq(flowmaid::seq::SeqScene),
    Pie(flowmaid::pie::PieScene),
}

/// Clickable region of a link, in document coordinates.
#[derive(Debug)]
pub struct LinkZone {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub url: String,
}

/// One heading's position — for TOCs and scroll-to-anchor.
#[derive(Debug)]
pub struct Anchor {
    pub level: u8,
    pub text: String,
    pub y: f64,
}

/// Semantic color slots. Consumers map these to their theme;
/// [`role_color`] is the default (light paper) palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorRole {
    Text,
    Strong,
    Muted,
    Link,
    CodeText,
    CodeBg,
    QuoteBg,
    Border,
    ErrorText,
    ErrorBg,
    /// Card behind an inline diagram (flowmaid scenes assume white).
    DiagramBg,
    /// Striped table rows / header strip.
    TableStripeBg,
}

/// Default light palette — the same ink/border family as flowmaid's
/// SVG output, so mixed documents look coherent.
pub fn role_color(role: ColorRole) -> &'static str {
    match role {
        ColorRole::Text => "#232840",
        ColorRole::Strong => "#111527",
        ColorRole::Muted => "#6a7086",
        ColorRole::Link => "#3563d9",
        ColorRole::CodeText => "#232840",
        ColorRole::CodeBg => "#eef1fb",
        ColorRole::QuoteBg => "#f4f6fc",
        ColorRole::Border => "#d5d9ec",
        ColorRole::ErrorText => "#c92a2a",
        ColorRole::ErrorBg => "#ffe3e3",
        ColorRole::DiagramBg => "#ffffff",
        ColorRole::TableStripeBg => "#f7f8fd",
    }
}

/// Serialise a laid-out document to standalone SVG using the default
/// palette. Inline diagrams are embedded as nested `<svg>` elements
/// produced by the flowmaid writers, so a document exports pixel-
/// identical to what interactive consumers paint.
pub fn to_svg(scene: &DocScene) -> String {
    crate::layout::doc_to_svg(scene)
}

/// Convenience: parse + layout + SVG in one call.
pub fn render_svg(source: &str, width: f64) -> String {
    let doc = crate::parser::parse(source);
    let scene = crate::layout::layout(&doc, &LayoutOptions { width, ..Default::default() });
    to_svg(&scene)
}
