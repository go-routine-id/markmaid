//! Hand-written GFM-subset parser: `&str -> Doc`. Implemented by the
//! parser build agent against the contract in `model.rs`.

use crate::model::Doc;

/// Parse Markdown into a [`Doc`]. Infallible by design.
pub fn parse(_source: &str) -> Doc {
    todo!("parser agent")
}
