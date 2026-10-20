//! The theme engine (crates/apz-theme) for the browser. Its exports take theme files, VS Code themes and
//! `.vsix` archives as JSON text or bytes, and return JSON text for every outcome: a refusal comes back as
//! `{"error": "..."}` rather than a thrown exception. The JSON layer lives in [`model`], which builds and
//! tests on the native target; the wasm-bindgen exports live in `bindings`, wasm only.
//!
//! The console's Appearance screen loads this as its own wasm module on first use, not with the console.

pub mod model;

#[cfg(target_arch = "wasm32")]
mod bindings;
