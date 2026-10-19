//! The wasm-bindgen exports: each one calls [`crate::model`], which returns JSON text for every outcome.

use wasm_bindgen::prelude::*;

use crate::model;

/// Resolves a theme file for a skin (its id) and a mode (`light` or `dark`). Returns
/// `{"resolved": Resolved, "css": string}`, or `{"error": string}`.
#[wasm_bindgen]
pub fn resolve(file: &str, skin: &str, mode: &str) -> String {
    model::resolve(file, skin, mode)
}

/// Checks a theme file. Returns `{"ok": bool, "problems": [string]}`; a file that does not parse is a problem, not
/// an error.
#[wasm_bindgen]
pub fn validate(file: &str) -> String {
    model::validate(file)
}

/// The contrast rule of every check, against the resolved tokens in a skin and mode. Returns an array of
/// `{token, against, ratio, min, ok}`, or `{"error": string}`.
#[wasm_bindgen]
pub fn contrast(file: &str, skin: &str, mode: &str) -> String {
    model::contrast(file, skin, mode)
}

/// The theme file with `token` set in the palette of the mode it resolves in, to the nearest colour that passes its
/// contrast rule. Returns the theme file as JSON, or `{"error": string}`.
#[wasm_bindgen]
pub fn fix(file: &str, skin: &str, mode: &str, token: &str) -> String {
    model::fix(file, skin, mode, token)
}

/// Imports a VS Code theme (JSON with comments, one file, no include chain). Returns
/// `{"file": ThemeFile, "name": string, "mode": string, "report": Report}`, or `{"error": string}`.
#[wasm_bindgen]
pub fn import_vscode(text: &str, skin: &str) -> String {
    model::import_vscode(text, skin)
}

/// The colour themes in a `.vsix` archive (bytes). Returns an array of `{label, ui_theme, path}`, or
/// `{"error": string}`.
#[wasm_bindgen]
pub fn vsix_themes(bytes: &[u8]) -> String {
    model::vsix_themes(bytes)
}

/// Imports one theme from a `.vsix` archive (bytes): `pick` names the theme's path when the archive has several.
/// Returns the same shape as [`import_vscode`].
#[wasm_bindgen]
pub fn import_vsix(bytes: &[u8], pick: Option<String>, skin: &str) -> String {
    model::import_vsix(bytes, pick.as_deref(), skin)
}

/// The backdrop's uniform block for a background spec (JSON) on a token map (JSON), as a Float32Array of 40 floats in
/// the shader's order. `px` and `py` are the pointer in css px, read when `has_pointer` is true. A refusal throws its
/// message.
#[wasm_bindgen]
pub fn backdrop_uniforms(
    bg: &str,
    tokens: &str,
    width: f32,
    height: f32,
    scale: f32,
    time: f32,
    px: f32,
    py: f32,
    has_pointer: bool,
) -> Result<Box<[f32]>, String> {
    model::backdrop_uniforms(bg, tokens, width, height, scale, time, px, py, has_pointer)
        .map(Vec::into_boxed_slice)
}

/// Whether a background spec (JSON) moves. A refusal throws its message.
#[wasm_bindgen]
pub fn backdrop_animates(bg: &str) -> Result<bool, String> {
    model::backdrop_animates(bg)
}
