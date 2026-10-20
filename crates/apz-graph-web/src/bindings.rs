//! The wasm-bindgen export: [`crate::model::layout`], with its refusal thrown as a JsError.

use wasm_bindgen::prelude::*;

use crate::model;

/// Lays out a graph (JSON: the read model of `GET /v1/projects/{id}/graph`, as it comes) with the
/// options (JSON, or empty for the desk sizes). Returns the layout as JSON.
#[wasm_bindgen]
pub fn layout(graph: &str, options: &str) -> Result<String, JsError> {
    model::layout(graph, options).map_err(|message| JsError::new(&message))
}

/// Lays out a graph's nodes and links with the force-directed layout (JSON: `nodes`, `edges`, `width`,
/// `height`, and optionally `iterations` and `node_size`; see [`crate::model::force_layout`]). Returns
/// each node's centre as JSON.
#[wasm_bindgen]
pub fn force_layout(input: &str) -> Result<String, JsError> {
    model::force_layout(input).map_err(|message| JsError::new(&message))
}
