//! Document model: the AST produced by the hand-written parser.
//!
//! The model is deliberately FLAT where it can be: inline content is
//! a sequence of [`Inline`] runs with resolved style flags (no
//! nesting), which keeps layout and every writer trivial.

/// A parsed Markdown document.
#[derive(Debug, Default, PartialEq)]
pub struct Doc {
    pub blocks: Vec<Block>,
}

/// One block-level element, in source order.
#[derive(Debug, PartialEq)]
pub enum Block {
    /// `#`..`######` — ATX heading, level 1..=6.
    Heading { level: u8, content: Vec<Inline> },
    Paragraph(Vec<Inline>),
    /// Fenced code (``` or ~~~). `lang` is the info string's first
    /// word, lowercased ("" when absent). Mermaid blocks are plain
    /// `Code` here — the LAYOUT stage turns them into diagrams.
    Code { lang: String, source: String },
    /// `>` block quote; contains full blocks recursively.
    Quote(Vec<Block>),
    List(List),
    Table(Table),
    /// `---` / `***` / `___` thematic break.
    Rule,
    /// A raw-HTML block, kept verbatim (rendered as code — this
    /// engine does not interpret HTML).
    Html(String),
}

/// `-`/`*`/`+` (unordered, `start == None`) or `1.`/`1)` (ordered,
/// `start == Some(first number)`).
#[derive(Debug, PartialEq)]
pub struct List {
    pub start: Option<u64>,
    pub items: Vec<ListItem>,
}

/// One list item. `checked` is the GFM task marker (`[ ]`/`[x]`);
/// `blocks` holds the item's content (first block is usually a
/// Paragraph; nested Lists and Code blocks live here too).
#[derive(Debug, PartialEq)]
pub struct ListItem {
    pub checked: Option<bool>,
    pub blocks: Vec<Block>,
}

/// GFM table. `rows[0]` is the header row; each cell is inline
/// content. All rows are padded/truncated to the header's width by
/// the parser, so consumers may assume a rectangle.
#[derive(Debug, PartialEq)]
pub struct Table {
    pub rows: Vec<Vec<Vec<Inline>>>,
}

/// One styled text run. Styles are RESOLVED (no nesting): the parser
/// flattens `**bold *italic***` into runs with both flags set. A
/// hard line break is a literal `\n` inside `text`.
#[derive(Debug, Default, PartialEq, Clone)]
pub struct Inline {
    pub text: String,
    pub strong: bool,
    pub em: bool,
    pub code: bool,
    pub strike: bool,
    /// Some(url) = this run is (part of) a link.
    pub link: Option<String>,
    /// Some(src) = this run is an image `![alt](src)`; `text` holds
    /// the alt text. The engine reserves a placeholder box (it can't
    /// decode pixels) and passes the src through to the consumer /
    /// SVG `<image>` / HTML `<img>`.
    pub image: Option<String>,
}

impl Inline {
    /// Plain text run with no styling — the common case.
    pub fn plain(text: impl Into<String>) -> Inline {
        Inline {
            text: text.into(),
            ..Inline::default()
        }
    }
}
