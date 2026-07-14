//! Semantic HTML writer: `Doc` -> HTML fragment (no `<html>` shell,
//! no stylesheet — bring your own). Mermaid fences are inlined as
//! SVG `<figure>`s via flowmaid; a fence that fails to parse becomes
//! a line-numbered `<pre class="markmaid-error">` instead of
//! breaking the page.
//!
//! Honest divergences from CommonMark/GFM output:
//! - raw HTML blocks are ESCAPED and shown as code — this engine
//!   never interprets or passes through HTML;
//! - void tags are HTML5-style (`<hr>`, `<br>`), not XHTML
//!   (`<hr />`);
//! - list items are always rendered tight (a single-paragraph item
//!   gets no `<p>` wrapper) — the model does not track loose lists;
//! - a link whose text mixes styles may emit several adjacent
//!   `<a>` elements (the model stores one flat run per style).

use crate::model::{Block, Doc, Inline, List, Table};

/// Markdown -> semantic HTML fragment.
pub fn render_html(source: &str) -> String {
    html_of(&crate::parser::parse(source))
}

/// The actual writer, off a parsed [`Doc`].
fn html_of(doc: &Doc) -> String {
    let mut out = String::new();
    // Mermaid fences are numbered in document order so an error
    // message points at the right block.
    let mut mermaid_n = 0usize;
    for b in &doc.blocks {
        block_html(&mut out, b, &mut mermaid_n);
    }
    out
}

/// Escape a text node (`&`, `<`, `>`).
fn esc_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

/// Escape a double-quoted attribute value (`&`, `<`, `>`, `"`).
fn esc_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// Neutralise dangerous URL schemes so a rendered link can never
/// execute script. A URL is SAFE when it has no scheme (relative /
/// fragment) or its scheme is one of http/https/mailto/tel/ftp;
/// anything else (`javascript:`, `data:`, `vbscript:`, unknown) is
/// dropped to an inert empty href. The module's injection-safety
/// claim depends on this.
fn safe_href(url: &str) -> &str {
    let t = url.trim_start();
    if let Some(colon) = t.find(':') {
        let scheme = &t[..colon];
        // A real scheme is non-empty, has no path punctuation before
        // the ':', and is made of scheme chars.
        let is_scheme = !scheme.is_empty()
            && !scheme.contains(['/', '?', '#'])
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        if is_scheme
            && !matches!(
                scheme.to_ascii_lowercase().as_str(),
                "http" | "https" | "mailto" | "tel" | "ftp"
            )
        {
            return "";
        }
    }
    url
}

/// Image source scheme filter: allows http/https/relative and
/// `data:` (common for inline images), drops `javascript:` etc.
fn safe_img(url: &str) -> &str {
    let t = url.trim_start();
    if let Some(colon) = t.find(':') {
        let scheme = &t[..colon];
        let is_scheme = !scheme.is_empty()
            && !scheme.contains(['/', '?', '#'])
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        if is_scheme
            && !matches!(
                scheme.to_ascii_lowercase().as_str(),
                "http" | "https" | "data" | "ftp"
            )
        {
            return "";
        }
    }
    url
}

/// One styled run: nested tags outside-in `a > strong > em > del >
/// code`, text escaped, literal `\n` as a `<br>` hard break. An image
/// run becomes an `<img>` (inside any surrounding link).
fn inline_html(out: &mut String, run: &Inline) {
    let mut close: Vec<&str> = Vec::new();
    if let Some(url) = &run.link {
        out.push_str("<a href=\"");
        out.push_str(&esc_attr(safe_href(url)));
        out.push_str("\">");
        close.push("</a>");
    }
    if let Some(src) = &run.image {
        out.push_str("<img src=\"");
        out.push_str(&esc_attr(safe_img(src)));
        out.push_str("\" alt=\"");
        out.push_str(&esc_attr(&run.text));
        out.push_str("\">");
        for tag in close.iter().rev() {
            out.push_str(tag);
        }
        return;
    }
    if run.strong {
        out.push_str("<strong>");
        close.push("</strong>");
    }
    if run.em {
        out.push_str("<em>");
        close.push("</em>");
    }
    if run.strike {
        out.push_str("<del>");
        close.push("</del>");
    }
    if run.code {
        out.push_str("<code>");
        close.push("</code>");
    }
    for (i, part) in run.text.split('\n').enumerate() {
        if i > 0 {
            out.push_str("<br>");
        }
        out.push_str(&esc_text(part));
    }
    for tag in close.iter().rev() {
        out.push_str(tag);
    }
}

fn inlines_html(out: &mut String, inlines: &[Inline]) {
    for run in inlines {
        inline_html(out, run);
    }
}

fn is_mermaid(lang: &str) -> bool {
    lang == "mermaid" || lang == "mmd"
}

fn block_html(out: &mut String, b: &Block, mermaid_n: &mut usize) {
    match b {
        Block::Heading { level, content } => {
            let l = (*level).clamp(1, 6);
            out.push_str(&format!("<h{}>", l));
            inlines_html(out, content);
            out.push_str(&format!("</h{}>\n", l));
        }
        Block::Paragraph(content) => {
            out.push_str("<p>");
            inlines_html(out, content);
            out.push_str("</p>\n");
        }
        Block::Code { lang, source } if is_mermaid(lang) => {
            *mermaid_n += 1;
            match flowmaid::render_svg(source) {
                Ok(svg) => {
                    out.push_str("<figure class=\"markmaid-diagram\">");
                    out.push_str(&svg);
                    out.push_str("</figure>\n");
                }
                Err(e) => {
                    out.push_str(&format!(
                        "<pre class=\"markmaid-error\">mermaid block #{}: {}</pre>\n",
                        mermaid_n,
                        esc_text(&e.to_string()),
                    ));
                }
            }
        }
        Block::Code { lang, source } => {
            if lang.is_empty() {
                out.push_str("<pre><code>");
            } else {
                out.push_str(&format!("<pre><code class=\"language-{}\">", esc_attr(lang)));
            }
            out.push_str(&esc_text(source));
            if !source.is_empty() && !source.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("</code></pre>\n");
        }
        Block::Quote(blocks) => {
            out.push_str("<blockquote>\n");
            for b in blocks {
                block_html(out, b, mermaid_n);
            }
            out.push_str("</blockquote>\n");
        }
        Block::List(list) => list_html(out, list, mermaid_n),
        Block::Table(table) => table_html(out, table),
        Block::Rule => out.push_str("<hr>\n"),
        // Raw HTML is shown as escaped code, mirroring the layout
        // stage — this engine does not interpret HTML.
        Block::Html(source) => {
            out.push_str("<pre><code>");
            out.push_str(&esc_text(source));
            out.push_str("</code></pre>\n");
        }
    }
}

fn list_html(out: &mut String, list: &List, mermaid_n: &mut usize) {
    let close = match list.start {
        None => {
            out.push_str("<ul>\n");
            "</ul>\n"
        }
        Some(1) => {
            out.push_str("<ol>\n");
            "</ol>\n"
        }
        Some(n) => {
            out.push_str(&format!("<ol start=\"{}\">\n", n));
            "</ol>\n"
        }
    };
    for item in &list.items {
        out.push_str("<li>");
        if let Some(checked) = item.checked {
            out.push_str(if checked {
                "<input type=\"checkbox\" disabled checked> "
            } else {
                "<input type=\"checkbox\" disabled> "
            });
        }
        // Tight rendering: a lone paragraph stays inline in the
        // <li>; anything richer gets full block layout.
        match item.blocks.as_slice() {
            [Block::Paragraph(content)] => inlines_html(out, content),
            blocks => {
                out.push('\n');
                for b in blocks {
                    block_html(out, b, mermaid_n);
                }
            }
        }
        out.push_str("</li>\n");
    }
    out.push_str(close);
}

fn table_html(out: &mut String, table: &Table) {
    let Some((header, body)) = table.rows.split_first() else {
        return;
    };
    out.push_str("<table>\n<thead>\n<tr>");
    for cell in header {
        out.push_str("<th>");
        inlines_html(out, cell);
        out.push_str("</th>");
    }
    out.push_str("</tr>\n</thead>\n");
    if !body.is_empty() {
        out.push_str("<tbody>\n");
        for row in body {
            out.push_str("<tr>");
            for cell in row {
                out.push_str("<td>");
                inlines_html(out, cell);
                out.push_str("</td>");
            }
            out.push_str("</tr>\n");
        }
        out.push_str("</tbody>\n");
    }
    out.push_str("</table>\n");
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    // Tests drive `html_of` on hand-built Docs — never the parser.
    use super::*;
    use crate::model::{Block, Doc, Inline, List, ListItem, Table};

    fn doc(blocks: Vec<Block>) -> Doc {
        Doc { blocks }
    }

    // ── regressions from the bug hunt ──

    #[test]
    fn dangerous_url_schemes_are_neutralised() {
        for bad in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "  javascript:alert(1)",
            "data:text/html,<script>x</script>",
            "vbscript:msgbox(1)",
        ] {
            assert_eq!(safe_href(bad), "", "must drop: {bad}");
        }
        for ok in [
            "https://x.dev",
            "http://x.dev",
            "mailto:a@b.c",
            "tel:+1",
            "/relative",
            "#anchor",
            "./rel",
            "path/to",
        ] {
            assert_eq!(safe_href(ok), ok, "must keep: {ok}");
        }
        // End to end: the rendered anchor carries no script scheme.
        let d = doc(vec![Block::Paragraph(vec![Inline {
            text: "x".into(),
            link: Some("javascript:alert(1)".into()),
            ..Inline::default()
        }])]);
        let out = html_of(&d);
        assert!(out.contains("<a href=\"\">x</a>"), "got: {out}");
        assert!(!out.contains("javascript:"));
    }

    #[test]
    fn empty_text_link_keeps_its_anchor() {
        let d = doc(vec![Block::Paragraph(vec![Inline {
            text: String::new(),
            link: Some("https://x.dev".into()),
            ..Inline::default()
        }])]);
        assert!(html_of(&d).contains("<a href=\"https://x.dev\"></a>"));
    }

    fn styled(text: &str, f: impl Fn(&mut Inline)) -> Inline {
        let mut i = Inline::plain(text);
        f(&mut i);
        i
    }

    fn para(text: &str) -> Block {
        Block::Paragraph(vec![Inline::plain(text)])
    }

    #[test]
    fn headings_map_to_h1_through_h6() {
        let blocks: Vec<Block> = (1..=6)
            .map(|l| Block::Heading {
                level: l,
                content: vec![Inline::plain("T")],
            })
            .collect();
        let html = html_of(&doc(blocks));
        for l in 1..=6 {
            assert!(html.contains(&format!("<h{}>T</h{}>", l, l)));
        }
    }

    #[test]
    fn paragraph_with_inline_styles() {
        let html = html_of(&doc(vec![Block::Paragraph(vec![
            styled("bold", |i| i.strong = true),
            Inline::plain(" "),
            styled("it", |i| i.em = true),
            Inline::plain(" "),
            styled("a<b", |i| i.code = true),
            Inline::plain(" "),
            styled("gone", |i| i.strike = true),
        ])]));
        assert!(html.starts_with("<p>"));
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains("<em>it</em>"));
        assert!(html.contains("<code>a&lt;b</code>"));
        assert!(html.contains("<del>gone</del>"));
        assert!(html.trim_end().ends_with("</p>"));
    }

    #[test]
    fn combined_styles_nest_outside_in() {
        let html = html_of(&doc(vec![Block::Paragraph(vec![styled("x", |i| {
            i.strong = true;
            i.em = true;
        })])]));
        assert!(html.contains("<strong><em>x</em></strong>"));
    }

    #[test]
    fn link_href_and_text_are_escaped() {
        let html = html_of(&doc(vec![Block::Paragraph(vec![styled("Q&A", |i| {
            i.link = Some("https://e.com/?a=1&b=\"2\"".into())
        })])]));
        assert!(html.contains("<a href=\"https://e.com/?a=1&amp;b=&quot;2&quot;\">Q&amp;A</a>"));
    }

    #[test]
    fn code_block_carries_language_class() {
        let html = html_of(&doc(vec![Block::Code {
            lang: "rust".into(),
            source: "let a = 1 < 2;".into(),
        }]));
        assert!(html.contains("<pre><code class=\"language-rust\">let a = 1 &lt; 2;\n</code></pre>"));
    }

    #[test]
    fn code_block_without_language() {
        let html = html_of(&doc(vec![Block::Code {
            lang: "".into(),
            source: "plain".into(),
        }]));
        assert!(html.contains("<pre><code>plain\n</code></pre>"));
        assert!(!html.contains("language-"));
    }

    #[test]
    fn mermaid_block_becomes_svg_figure() {
        let html = html_of(&doc(vec![Block::Code {
            lang: "mermaid".into(),
            source: "flowchart TD\nA[Start] --> B[Done]".into(),
        }]));
        assert!(html.contains("<figure class=\"markmaid-diagram\"><svg"));
        assert!(html.contains("</figure>"));
        assert!(html.contains("Start"));
    }

    #[test]
    fn broken_mermaid_becomes_numbered_error() {
        let html = html_of(&doc(vec![
            Block::Code {
                lang: "mermaid".into(),
                source: "flowchart TD\nA --> B".into(),
            },
            Block::Code {
                lang: "mmd".into(),
                source: "gantt\ntitle nope".into(),
            },
        ]));
        assert!(html.contains("<figure class=\"markmaid-diagram\">"));
        assert!(html.contains("<pre class=\"markmaid-error\">mermaid block #2: line "));
        assert!(!html.contains("mermaid block #1:"));
    }

    #[test]
    fn unordered_and_ordered_lists() {
        let item = |text: &str| ListItem {
            checked: None,
            blocks: vec![para(text)],
        };
        let ul = html_of(&doc(vec![Block::List(List {
            start: None,
            items: vec![item("one")],
        })]));
        assert!(ul.contains("<ul>\n<li>one</li>\n</ul>"));

        let ol3 = html_of(&doc(vec![Block::List(List {
            start: Some(3),
            items: vec![item("three")],
        })]));
        assert!(ol3.contains("<ol start=\"3\">\n<li>three</li>\n</ol>"));

        let ol1 = html_of(&doc(vec![Block::List(List {
            start: Some(1),
            items: vec![item("one")],
        })]));
        assert!(ol1.contains("<ol>\n"));
        assert!(!ol1.contains("start="));
    }

    #[test]
    fn task_items_render_disabled_checkboxes() {
        let item = |checked, text: &str| ListItem {
            checked: Some(checked),
            blocks: vec![para(text)],
        };
        let html = html_of(&doc(vec![Block::List(List {
            start: None,
            items: vec![item(true, "done"), item(false, "todo")],
        })]));
        assert!(html.contains("<li><input type=\"checkbox\" disabled checked> done</li>"));
        assert!(html.contains("<li><input type=\"checkbox\" disabled> todo</li>"));
    }

    #[test]
    fn multi_block_list_item_gets_block_layout() {
        let html = html_of(&doc(vec![Block::List(List {
            start: None,
            items: vec![ListItem {
                checked: None,
                blocks: vec![
                    para("first"),
                    Block::List(List {
                        start: None,
                        items: vec![ListItem {
                            checked: None,
                            blocks: vec![para("inner")],
                        }],
                    }),
                ],
            }],
        })]));
        assert!(html.contains("<li>\n<p>first</p>\n<ul>\n<li>inner</li>\n</ul>\n</li>"));
    }

    #[test]
    fn blockquote_wraps_blocks() {
        let html = html_of(&doc(vec![Block::Quote(vec![para("a"), para("b")])]));
        assert!(html.contains("<blockquote>\n<p>a</p>\n<p>b</p>\n</blockquote>"));
    }

    #[test]
    fn table_has_thead_and_tbody() {
        let cell = |s: &str| vec![Inline::plain(s)];
        let html = html_of(&doc(vec![Block::Table(Table {
            rows: vec![
                vec![cell("Name"), cell("Qty")],
                vec![cell("apple"), cell("1")],
            ],
        })]));
        assert!(html.contains("<table>\n<thead>\n<tr><th>Name</th><th>Qty</th></tr>\n</thead>"));
        assert!(html.contains("<tbody>\n<tr><td>apple</td><td>1</td></tr>\n</tbody>\n</table>"));

        let header_only = html_of(&doc(vec![Block::Table(Table {
            rows: vec![vec![cell("only")]],
        })]));
        assert!(header_only.contains("<thead>"));
        assert!(!header_only.contains("<tbody>"));
    }

    #[test]
    fn rule_and_hard_break() {
        let html = html_of(&doc(vec![
            Block::Paragraph(vec![Inline::plain("a\nb")]),
            Block::Rule,
        ]));
        assert!(html.contains("<p>a<br>b</p>"));
        assert!(html.contains("<hr>\n"));
    }

    #[test]
    fn raw_html_block_is_escaped_as_code() {
        let html = html_of(&doc(vec![Block::Html("<div class=\"x\">hi</div>".into())]));
        assert!(html.contains("<pre><code>&lt;div class=\"x\"&gt;hi&lt;/div&gt;</code></pre>"));
        assert!(!html.contains("<div"));
    }

    #[test]
    fn text_nodes_escape_amp_lt_gt() {
        let html = html_of(&doc(vec![para("1 < 2 && 3 > 2")]));
        assert!(html.contains("<p>1 &lt; 2 &amp;&amp; 3 &gt; 2</p>"));
    }

    #[test]
    fn image_becomes_img_with_escaped_alt() {
        let html = html_of(&doc(vec![Block::Paragraph(vec![styled("A & B", |i| {
            i.image = Some("pic.png".into());
        })])]));
        assert!(html.contains("<img src=\"pic.png\" alt=\"A &amp; B\">"));
    }

    #[test]
    fn image_data_uri_is_allowed() {
        let html = html_of(&doc(vec![Block::Paragraph(vec![styled("", |i| {
            i.image = Some("data:image/png;base64,AAAA".into());
        })])]));
        assert!(html.contains("src=\"data:image/png;base64,AAAA\""));
    }

    #[test]
    fn image_javascript_src_is_neutralised() {
        let html = html_of(&doc(vec![Block::Paragraph(vec![styled("x", |i| {
            i.image = Some("javascript:alert(1)".into());
        })])]));
        assert!(html.contains("<img src=\"\""));
        assert!(!html.contains("javascript:"));
    }

    #[test]
    fn linked_image_nests_img_in_anchor() {
        let html = html_of(&doc(vec![Block::Paragraph(vec![styled("t", |i| {
            i.image = Some("t.png".into());
            i.link = Some("https://big.example/".into());
        })])]));
        assert!(html.contains("<a href=\"https://big.example/\"><img src=\"t.png\" alt=\"t\"></a>"));
    }
}
