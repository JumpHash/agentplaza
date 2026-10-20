//! The owner's appearance (docs/design/themes.md): one global choice of theme, skin, background and mode, plus the
//! owner's saved themes, served at `GET`/`PUT /v1/appearance`. Both consoles apply it; each may hold a per-device
//! override of its own.
//!
//! GET    /v1/appearance                 → the document: the choice and the saved themes
//! PUT    /v1/appearance {…}             → saves the choice (checked against the saved themes)
//! PUT    /v1/appearance/themes/{id}     → saves a theme the owner made (built-in ids are refused)
//! DELETE /v1/appearance/themes/{id}     → deletes a saved theme; a choice that used it goes back to the default
//!
//! Kept in `appearance.json` under the state dir, and with the plaza state (component `appearance`).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_theme::appearance::{Appearance, Document};
use apz_theme::{ThemeFile, builtin};
use serde_json::json;

use crate::state::App;

/// The most saved themes the owner may keep.
const MAX_THEMES: usize = 64;

/// The appearance document, persisted at `<state-dir>/appearance.json` and as the `appearance` component.
pub struct Store {
    path: PathBuf,
    inner: Mutex<Document>,
}

impl Store {
    /// Loads `appearance.json`; a missing or unparsable file (or one that fails the checks) gives the default.
    pub fn load(state_dir: &Path) -> Store {
        let path = state_dir.join("appearance.json");
        let doc = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Document>(&bytes) {
                Ok(doc) => match check(&doc) {
                    Ok(()) => doc,
                    Err(e) => {
                        eprintln!(
                            "appearance: {} fails its checks ({e}); using the default",
                            path.display()
                        );
                        Document::default()
                    }
                },
                Err(e) => {
                    eprintln!(
                        "appearance: {} is not valid appearance JSON ({e}); using the default",
                        path.display()
                    );
                    Document::default()
                }
            },
            Err(_) => Document::default(),
        };
        Store {
            path,
            inner: Mutex::new(doc),
        }
    }

    /// The whole document: the choice and the saved themes.
    pub fn get(&self) -> Document {
        self.lock().clone()
    }

    /// Saves the choice after checking it against the saved themes. Returns the saved document.
    pub fn put_choice(&self, choice: Appearance) -> Result<Document, String> {
        let mut guard = self.lock();
        let problems = choice.problems(&guard.themes);
        if !problems.is_empty() {
            return Err(problems.join("; "));
        }
        let mut next = guard.clone();
        next.choice = choice;
        self.write(&mut guard, next)
    }

    /// Saves a theme the owner made, replacing the saved theme of the same id. Built-in ids are refused.
    pub fn save_theme(&self, theme: ThemeFile) -> Result<Document, String> {
        if builtin::is_builtin(&theme.id) {
            return Err(format!(
                "{:?} is a built-in theme and cannot be replaced",
                theme.id
            ));
        }
        let problems = theme.problems();
        if !problems.is_empty() {
            return Err(problems.join("; "));
        }
        let mut guard = self.lock();
        let mut next = guard.clone();
        match next.themes.iter().position(|t| t.id == theme.id) {
            Some(i) => next.themes[i] = theme,
            None if next.themes.len() >= MAX_THEMES => {
                return Err(format!("at most {MAX_THEMES} saved themes"));
            }
            None => next.themes.push(theme),
        }
        self.write(&mut guard, next)
    }

    /// Deletes a saved theme. A choice that used it goes back to the default theme.
    pub fn delete_theme(&self, id: &str) -> Result<Document, String> {
        if builtin::is_builtin(id) {
            return Err(format!("{id:?} is a built-in theme and cannot be deleted"));
        }
        let mut guard = self.lock();
        if !guard.themes.iter().any(|t| t.id == id) {
            return Err(format!("no saved theme {id:?}"));
        }
        let mut next = guard.clone();
        next.themes.retain(|t| t.id != id);
        if next.choice.theme == id {
            next.choice.theme = builtin::DEFAULT_THEME.into();
        }
        self.write(&mut guard, next)
    }

    /// The whole document as JSON, for Postgres (`crate::db`).
    pub fn snapshot(&self) -> String {
        serde_json::to_string_pretty(&self.get()).unwrap_or_default()
    }

    /// Replaces the document with a [`Store::snapshot`]; a refusal changes nothing.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let doc: Document =
            serde_json::from_str(json).map_err(|e| format!("bad appearance snapshot: {e}"))?;
        check(&doc)?;
        let mut guard = self.lock();
        self.write(&mut guard, doc).map(|_| ())
    }

    fn lock(&self) -> MutexGuard<'_, Document> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Writes `appearance.json` atomically and swaps the in-memory document. `guard` is the locked document.
    fn write(&self, guard: &mut Document, next: Document) -> Result<Document, String> {
        let bytes = serde_json::to_vec_pretty(&next).map_err(|e| e.to_string())?;
        let tmp = self.path.with_file_name("appearance.json.tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| format!("write appearance: {e}"))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("replace appearance: {e}"))?;
        *guard = next.clone();
        Ok(next)
    }
}

/// The checks every saved document passes (`load` and `restore`): valid unique saved themes, none named as a
/// built-in, a count within the limit, and a choice that is valid against them.
fn check(doc: &Document) -> Result<(), String> {
    if doc.themes.len() > MAX_THEMES {
        return Err(format!("at most {MAX_THEMES} saved themes"));
    }
    for (i, theme) in doc.themes.iter().enumerate() {
        if builtin::is_builtin(&theme.id) {
            return Err(format!("saved theme {:?} takes a built-in id", theme.id));
        }
        if doc.themes[..i].iter().any(|t| t.id == theme.id) {
            return Err(format!("saved theme {:?} appears twice", theme.id));
        }
        let problems = theme.problems();
        if !problems.is_empty() {
            return Err(format!(
                "saved theme {:?}: {}",
                theme.id,
                problems.join("; ")
            ));
        }
    }
    let problems = doc.choice.problems(&doc.themes);
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("; "))
    }
}

/// `GET`/`PUT /v1/appearance` and `PUT`/`DELETE /v1/appearance/themes/{id}`.
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/appearance", get_route)
        .doc(RouteDoc::new(
            "Read the owner's appearance and saved themes",
        ))
        .route("PUT", "/v1/appearance", put_choice_route)
        .doc(RouteDoc::new("Save the owner's appearance choice").errors(&[400]))
        .route("PUT", "/v1/appearance/themes/{id}", save_theme_route)
        .doc(RouteDoc::new("Save a theme the owner made").errors(&[400]))
        .route("DELETE", "/v1/appearance/themes/{id}", delete_theme_route)
        .doc(RouteDoc::new("Delete a saved theme").errors(&[400]))
}

fn get_route(app: &App, _req: &Request, _p: &Params<'_>) -> Response {
    Response::json(200, &app.appearance.get())
}

fn put_choice_route(app: &App, req: &Request, _p: &Params<'_>) -> Response {
    let choice: Appearance = match req.json() {
        Ok(choice) => choice,
        Err(e) => {
            return Response::json(
                400,
                &json!({ "error": format!("bad appearance JSON: {e}") }),
            );
        }
    };
    finish(app, app.appearance.put_choice(choice))
}

fn save_theme_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    let theme: ThemeFile = match req.json() {
        Ok(theme) => theme,
        Err(e) => return Response::json(400, &json!({ "error": format!("bad theme JSON: {e}") })),
    };
    if theme.id != id {
        return Response::json(
            400,
            &json!({ "error": format!("the theme's id {:?} is not {id:?}", theme.id) }),
        );
    }
    finish(app, app.appearance.save_theme(theme))
}

fn delete_theme_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    finish(app, app.appearance.delete_theme(p.get("id")))
}

/// Answers a change: the saved document is published on the bus so the other console updates live.
fn finish(app: &App, result: Result<Document, String>) -> Response {
    match result {
        Ok(doc) => {
            app.bus.publish("appearance", &doc);
            Response::json(200, &doc)
        }
        Err(msg) => Response::json(400, &json!({ "error": msg })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_theme::builtin::DEFAULT_THEME;

    /// An empty temporary state directory for one test.
    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("appearance-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary directory");
        dir
    }

    /// A saved theme with the given id, copied from the default built-in theme.
    fn theme(id: &str) -> ThemeFile {
        let mut t = builtin::builtin(DEFAULT_THEME).expect("the default theme is built in");
        t.id = id.into();
        t.name = format!("Theme {id}");
        t
    }

    fn choice(theme: &str) -> Appearance {
        Appearance {
            theme: theme.into(),
            ..Appearance::default()
        }
    }

    #[test]
    fn default_document_on_an_empty_dir() {
        let store = Store::load(&dir("empty"));
        assert_eq!(store.get(), Document::default());
    }

    #[test]
    fn put_choice_accepts_a_builtin_and_refuses_an_unknown_theme() {
        let store = Store::load(&dir("choice"));
        let saved = store
            .put_choice(choice(DEFAULT_THEME))
            .expect("the default theme is valid");
        assert_eq!(saved.choice.theme, DEFAULT_THEME);
        let err = store.put_choice(choice("no-such-theme")).unwrap_err();
        assert!(err.contains("unknown theme"), "{err}");
        assert_eq!(
            store.get().choice.theme,
            DEFAULT_THEME,
            "a refused choice changes nothing"
        );
    }

    #[test]
    fn a_saved_theme_can_be_chosen_and_deleting_it_resets_the_choice() {
        let store = Store::load(&dir("save"));
        store
            .save_theme(theme("my-theme"))
            .expect("the theme is valid");
        store
            .put_choice(choice("my-theme"))
            .expect("a saved theme can be chosen");
        let after = store
            .delete_theme("my-theme")
            .expect("the saved theme is deleted");
        assert!(after.themes.is_empty());
        assert_eq!(
            after.choice.theme, DEFAULT_THEME,
            "the choice goes back to the default"
        );
    }

    #[test]
    fn built_in_ids_are_refused() {
        let store = Store::load(&dir("builtin"));
        assert!(store.save_theme(theme(DEFAULT_THEME)).is_err());
        assert!(store.delete_theme(DEFAULT_THEME).is_err());
        assert_eq!(store.get(), Document::default());
    }

    #[test]
    fn the_document_round_trips_through_the_file() {
        let root = dir("file");
        let store = Store::load(&root);
        store
            .save_theme(theme("my-theme"))
            .expect("the theme is valid");
        store
            .put_choice(choice("my-theme"))
            .expect("the choice is valid");
        assert!(root.join("appearance.json").exists());
        let reloaded = Store::load(&root);
        assert_eq!(reloaded.get(), store.get());

        let other = Store::load(&dir("restore"));
        other
            .restore(&store.snapshot())
            .expect("the snapshot restores");
        assert_eq!(other.get(), store.get());
    }
}
