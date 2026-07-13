//! Render the demo document to SVG and HTML:
//! `cargo run --example render` writes demo.svg + demo.html next to
//! the manifest.

fn main() {
    let md = include_str!("demo.md");
    std::fs::write("demo.svg", markmaid::render_svg(md, 760.0)).unwrap();
    std::fs::write("demo.html", markmaid::render_html(md)).unwrap();
    eprintln!("wrote demo.svg + demo.html");
}
