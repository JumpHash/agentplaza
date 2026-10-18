//! The task graph layout (crates/apz-graph) for the browser. Its one export takes the graph and the
//! options as JSON text and returns the layout as JSON text; a refusal throws a JsError with the
//! message. The JSON layer lives in [`model`], which builds and tests on the native target; the
//! wasm-bindgen export lives in `bindings`, wasm only.
//!
//! The web's Work screen loads this as its own wasm module on first use, not with the console.

pub mod model;

#[cfg(target_arch = "wasm32")]
mod bindings;
