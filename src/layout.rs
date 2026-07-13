//! Layout: `Doc -> DocScene` plus the built-in SVG writer.
//! Implemented by the layout build agent against `scene.rs`.

use crate::model::Doc;
use crate::scene::{DocScene, LayoutOptions};

pub fn layout(_doc: &Doc, _opts: &LayoutOptions) -> DocScene {
    todo!("layout agent")
}

pub(crate) fn doc_to_svg(_scene: &DocScene) -> String {
    todo!("layout agent")
}
