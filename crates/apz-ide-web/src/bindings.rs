//! The wasm-bindgen exports: each is a [`crate::model`] function with its refusal thrown as a JsError.

use wasm_bindgen::prelude::*;

use crate::model;

fn fail(message: String) -> JsError {
    JsError::new(&message)
}

/// Parses `git diff` or `diff -u` text. Returns the ChangeSet JSON with a `stats` field added.
#[wasm_bindgen]
pub fn parse_unified(text: &str) -> Result<String, JsError> {
    model::parse_unified(text).map_err(fail)
}

/// Cleans a relative file path (apz-ide's `check_path`). Returns the cleaned path.
#[wasm_bindgen]
pub fn check_path(p: &str) -> Result<String, JsError> {
    model::check_path(p).map_err(fail)
}

/// Cleans a path the IDE may read: valid and not secret.
#[wasm_bindgen]
pub fn check_read(p: &str) -> Result<String, JsError> {
    model::check_read(p).map_err(fail)
}

/// Cleans a path the IDE may write: readable and not under `.plaza/`.
#[wasm_bindgen]
pub fn check_write(p: &str) -> Result<String, JsError> {
    model::check_write(p).map_err(fail)
}

/// Cleans a directory path. `""` is the root.
#[wasm_bindgen]
pub fn check_dir(p: &str) -> Result<String, JsError> {
    model::check_dir(p).map_err(fail)
}

/// Whether a path is on the secret deny list.
#[wasm_bindgen]
pub fn is_secret(p: &str) -> bool {
    model::is_secret(p)
}

/// The IdeState before any action, as JSON.
#[wasm_bindgen]
pub fn initial_state() -> String {
    model::initial_state()
}

/// Applies one Action (JSON) to an IdeState (JSON). Returns the next IdeState as JSON.
#[wasm_bindgen]
pub fn reduce(state_json: &str, action_json: &str) -> Result<String, JsError> {
    model::reduce(state_json, action_json).map_err(fail)
}

/// The direct children of `dir` among an Entry array (JSON), in display order. Returns an Entry array as JSON.
#[wasm_bindgen]
pub fn children(entries_json: &str, dir: &str) -> Result<String, JsError> {
    model::children(entries_json, dir).map_err(fail)
}

/// The entries a ChangeSet (JSON) touches, with their ancestor directories. Returns an Entry array as JSON.
#[wasm_bindgen]
pub fn changed_filter(entries_json: &str, changes_json: &str) -> Result<String, JsError> {
    model::changed_filter(entries_json, changes_json).map_err(fail)
}

/// Parses a workspace id (`agent:<cid>` or `repo:<name>@<ref>`). Returns its JSON form: the quoted canonical string.
#[wasm_bindgen]
pub fn parse_workspace_id(s: &str) -> Result<String, JsError> {
    model::parse_workspace_id(s).map_err(fail)
}

/// The canonical text of a workspace id given as JSON (the form `parse_workspace_id` returns).
#[wasm_bindgen]
pub fn format_workspace_id(json: &str) -> Result<String, JsError> {
    model::format_workspace_id(json).map_err(fail)
}
