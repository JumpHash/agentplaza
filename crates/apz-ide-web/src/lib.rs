//! The IDE's shared model (crates/apz-ide) for the browser. Every export takes and returns JSON text (a workspace id is
//! its canonical string, JSON-quoted), and a refusal throws a JsError with the message. The JSON layer lives in
//! [`model`], which builds and tests on the native target; the wasm-bindgen exports live in `bindings`, wasm only.
//!
//! The web loads this as its own wasm module on first use (apps/web/src/ide/wasm.ts), not with the console.

pub mod model;

#[cfg(target_arch = "wasm32")]
mod bindings;
