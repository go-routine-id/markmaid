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

/// Stored closure type for [`Measure::Custom`].
pub type MeasureFn = std::rc::Rc<dyn Fn(&str, f64, bool, bool) -> f64>;

/// How the layout measures text width. [`Measure::Estimated`] is the
/// built-in metric (flowmaid's Helvetica table for proportional text
/// plus a flat advance for monospace) — no font files required, so the
/// SVG/HTML writers and any font-agnostic consumer keep working.
/// [`Measure::Custom`] lets an interactive consumer supply real font
/// metrics, so wrapping and inline-code chips align with the glyphs it
/// actually paints.
#[derive(Default, Clone)]
pub enum Measure {
    /// The default built-in metric.
    #[default]
    Estimated,
    /// Measure with a consumer-supplied closure over `(text, size,
    /// mono, em)` returning the advance width in layout pixels.
    Custom(MeasureFn),
}

impl std::fmt::Debug for Measure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Measure::Estimated => f.write_str("Estimated"),
            Measure::Custom(_) => f.write_str("Custom(..)"),
        }
    }
}

impl Measure {
    /// The default built-in metric.
    pub fn estimated() -> Self {
        Measure::Estimated
    }

    /// Measure with a consumer-supplied closure over `(text, size,
    /// mono, em)` returning the advance width in layout pixels.
    pub fn custom(f: impl Fn(&str, f64, bool, bool) -> f64 + 'static) -> Self {
        Measure::Custom(std::rc::Rc::new(f))
    }

    /// Width of `s` at `size`, for `mono` (inline code) and `em`
    /// (italic). `em` is ignored by the estimated metric (which never
    /// modelled italic), but passed to a custom metric.
    pub fn width(&self, s: &str, size: f64, mono: bool, em: bool) -> f64 {
        match self {
            Measure::Estimated => crate::layout::estimated_width(s, size, mono),
            Measure::Custom(f) => f(s, size, mono, em),
        }
    }
}

/// How to handle a table whose natural column widths exceed the
/// available viewport width.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TableOverflow {
    /// Shrink every column proportionally so the table still fits the
    /// width. This is the historical default and what SVG/HTML use.
    #[default]
    Shrink,
    /// Keep columns at their natural width and let the consumer clip or
    /// scroll horizontally. Interactive consumers (e.g. egui) opt into
    /// this.
    Natural,
}

/// Layout inputs. `width` is the full document width including the
/// outer margins; text wraps to fit it.
#[derive(Debug, Clone)]
pub struct LayoutOptions {
    pub width: f64,
    /// Base font size for body text (headings scale from this).
    pub base_size: f64,
    /// Text metric; defaults to the built-in estimate.
    pub measure: Measure,
    /// Table layout strategy when the table is wider than the viewport.
    pub table_overflow: TableOverflow,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        LayoutOptions {
            width: 720.0,
            base_size: 14.0,
            measure: Measure::Estimated,
            table_overflow: TableOverflow::Shrink,
        }
    }
}

/// A laid-out document: paint `items` in order. `links` are hit-test
/// zones for interactivity; `anchors` map headings to y offsets
/// (tables of contents, scroll-to-section); `tables` marks table item
/// ranges that a consumer may render inside a horizontal scroll area;
/// `code_blocks` carries verbatim source for copy/select UX.
#[derive(Debug, Default)]
pub struct DocScene {
    pub width: f64,
    pub height: f64,
    pub items: Vec<Item>,
    pub links: Vec<LinkZone>,
    pub anchors: Vec<Anchor>,
    pub tables: Vec<TableZone>,
    pub code_blocks: Vec<CodeBlockZone>,
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
    /// An `![alt](src)` image: a RESERVED placeholder box carrying the
    /// source. The engine cannot decode pixels (that needs an image
    /// crate, against the zero-dependency ethos), so a consumer with
    /// a decoder loads `src` and draws it into `(x, y, w, h)`; the
    /// SVG writer emits `<image href>` and HTML emits `<img>`. The box
    /// aspect is a placeholder — consumers may re-fit to intrinsic
    /// size once loaded.
    Image(ImageItem),
}

#[derive(Debug)]
pub struct ImageItem {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub src: String,
    pub alt: String,
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
    /// Which mermaid fence of the document this is, counting every
    /// mermaid block the parser produced in layout order — including
    /// ones that failed to parse and rendered as an error card instead
    /// of a diagram. This is the index into
    /// [`crate::blocks::mermaid_fences`]; COUNTING `Item::Diagram`s
    /// instead would drift the moment a diagram is mid-edit and broken.
    pub fence: usize,
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
    Mind(flowmaid::mindmap::MindScene),
    Journey(flowmaid::journey::JourneyScene),
    Git(flowmaid::gitgraph::GitScene),
    Arch(flowmaid::architecture::ArchScene),
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

/// A laid-out table that requires horizontal scrolling.
///
/// `x`/`y`/`h` are in document coordinates. `w` is the width the table
/// occupies on the page; `natural_w` is the sum of the column widths
/// plus padding, i.e. the full scroll content width. `items` indexes
/// into [`DocScene::items`] — everything belonging to this table.
#[derive(Debug)]
pub struct TableZone {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub natural_w: f64,
    pub h: f64,
    pub items: std::ops::Range<usize>,
}

/// A laid-out code block (fenced ``` or raw HTML) that a consumer may
/// render with copy/select interactivity. `source` is the original
/// verbatim text; `items` is the painted geometry range.
#[derive(Debug)]
pub struct CodeBlockZone {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub source: String,
    pub items: std::ops::Range<usize>,
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
    CodeHighlightBg,
    /// Syntax highlighting roles for fenced code blocks.
    CodeComment,
    CodeKeyword,
    CodeString,
    CodeNumber,
    CodeType,
    CodeFunction,
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
        ColorRole::CodeHighlightBg => "#dbe4ff",
        ColorRole::CodeComment => "#6a737d",
        ColorRole::CodeKeyword => "#d73a49",
        ColorRole::CodeString => "#22863a",
        ColorRole::CodeNumber => "#005cc5",
        ColorRole::CodeType => "#6f42c1",
        ColorRole::CodeFunction => "#8250df",
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
