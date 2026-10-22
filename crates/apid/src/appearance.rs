//! The owner's appearance (docs/design/themes.md): one global choice of theme, skin, background and mode, plus the
//! owner's saved themes, served at `GET`/`PUT /v1/appearance`. Both consoles apply it; each may hold a per-device
//! override of its own.
//!
//! GET    /v1/appearance                 → the document: the choice and the saved themes
//! PUT    /v1/appearance {…}             → saves the choice (checked against the saved themes)
//! PUT    /v1/appearance/themes/{id}     → saves a theme the owner made (built-in ids are refused)
//! DELETE /v1/appearance/themes/{id}     → deletes a saved theme; a choice that used it goes back to the default
//!
//! Kept in the hub's store (component `appearance`): the document is one row of typed tables, with its choice and
//! its saved themes' backgrounds and palettes as child rows.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_store::{RowSet, StoreError, TableDef};
use apz_theme::appearance::{Appearance, Density, Document, ModeChoice};
use apz_theme::{Background, BackgroundChoice, Palettes, ThemeFile, builtin};
use serde_json::json;

use crate::state::App;

/// The most saved themes the owner may keep.
const MAX_THEMES: usize = 64;

/// The appearance document, kept in the hub's store (component `appearance`).
pub struct Store {
    inner: Mutex<Document>,
}

impl Store {
    /// An empty store holding the default document; [`Store::load`] fills it at start.
    pub fn new() -> Store {
        Store {
            inner: Mutex::new(Document::default()),
        }
    }

    /// Replaces the document with what the store holds (at start). A stored document that fails the checks, or cannot
    /// be read back, gives the default.
    pub fn load(&self, conn: &apz_store::rusqlite::Connection) -> Result<(), StoreError> {
        let stored: Option<Stored> = apz_store::table::load_one(conn, def())?;
        let doc = match stored.map(Document::try_from).transpose() {
            Ok(Some(doc)) => match check(&doc) {
                Ok(()) => doc,
                Err(e) => {
                    eprintln!(
                        "appearance: the stored document fails its checks ({e}); using the default"
                    );
                    Document::default()
                }
            },
            Ok(None) => Document::default(),
            Err(e) => {
                eprintln!(
                    "appearance: the stored document cannot be read ({e}); using the default"
                );
                Document::default()
            }
        };
        *self.lock() = doc;
        Ok(())
    }

    /// The document as rows of the appearance's tables (for the hub's store).
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        RowSet::one(def(), &Stored::from(&self.get()))
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
        Ok(install(&mut guard, next))
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
        Ok(install(&mut guard, next))
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
        Ok(install(&mut guard, next))
    }

    fn lock(&self) -> MutexGuard<'_, Document> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Installs `next` as the document (`guard` is the locked document) and returns it.
fn install(guard: &mut Document, next: Document) -> Document {
    *guard = next.clone();
    next
}

/// The checks every saved document passes (`load` and the saves): valid unique saved themes, none named as a
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

/// The appearance as the store holds it. A plain struct cannot hold child tables, and a background is a preset's id
/// or a spec, so the choice and each theme are child rows of this one, with their backgrounds and palettes as children
/// (see [`ChoiceRow`], [`ThemeRow`]). [`From`] and [`TryFrom`] convert at the boundary.
#[derive(Debug, Clone, apz_store::Table)]
#[table(name = "appearance")]
struct Stored {
    #[table(child)]
    choice: Option<ChoiceRow>,
    #[table(child = "appearance_theme")]
    themes: Vec<ThemeRow>,
}

/// The choice as the store holds it: a background is either a preset's id (`background_preset`) or a spec (a child).
#[derive(Debug, Clone, apz_store::Table)]
struct ChoiceRow {
    theme: String,
    skin: Option<String>,
    background_preset: Option<String>,
    #[table(child)]
    background: Option<Background>,
    mode: ModeChoice,
    density: Density,
    code_size: f32,
    chosen: bool,
}

/// A saved theme as the store holds it, its background as the choice's is, and its palettes as child rows.
#[derive(Debug, Clone, apz_store::Table)]
struct ThemeRow {
    schema: u32,
    id: String,
    name: String,
    about: String,
    skin: String,
    background_preset: Option<String>,
    #[table(child)]
    background: Option<Background>,
    #[table(child)]
    palette: Option<PaletteRow>,
    #[table(child)]
    overrides: Option<PaletteRow>,
    source: Option<String>,
}

/// One set of palettes: the light and the dark colours, each present or absent.
#[derive(Debug, Clone, apz_store::Table)]
struct PaletteRow {
    #[table(child)]
    light: Option<Colours>,
    #[table(child)]
    dark: Option<Colours>,
}

/// One palette's colours: token name to colour text.
#[derive(Debug, Clone, apz_store::Table)]
struct Colours {
    #[table(map_value)]
    entries: BTreeMap<String, String>,
}

/// The appearance's tables.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(Stored)
}

/// A background choice as its two stored parts: a preset's id, or a spec (at most one of them is set).
fn background_parts(choice: Option<&BackgroundChoice>) -> (Option<String>, Option<Background>) {
    match choice {
        None => (None, None),
        Some(BackgroundChoice::Preset(id)) => (Some(id.clone()), None),
        Some(BackgroundChoice::Spec(spec)) => (None, Some(spec.clone())),
    }
}

/// A background choice from its stored parts. Both parts set is a refusal.
fn background_of(
    preset: Option<String>,
    spec: Option<Background>,
) -> Result<Option<BackgroundChoice>, StoreError> {
    match (preset, spec) {
        (None, None) => Ok(None),
        (Some(id), None) => Ok(Some(BackgroundChoice::Preset(id))),
        (None, Some(spec)) => Ok(Some(BackgroundChoice::Spec(spec))),
        (Some(_), Some(_)) => Err(StoreError::invalid(
            "a background is a preset or a spec, not both",
        )),
    }
}

impl From<&Document> for Stored {
    fn from(doc: &Document) -> Self {
        Stored {
            choice: Some(ChoiceRow::from(&doc.choice)),
            themes: doc.themes.iter().map(ThemeRow::from).collect(),
        }
    }
}

impl TryFrom<Stored> for Document {
    type Error = StoreError;

    fn try_from(stored: Stored) -> Result<Self, StoreError> {
        Ok(Document {
            choice: match stored.choice {
                Some(choice) => Appearance::try_from(choice)?,
                None => Appearance::default(),
            },
            themes: stored
                .themes
                .into_iter()
                .map(ThemeFile::try_from)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<&Appearance> for ChoiceRow {
    fn from(a: &Appearance) -> Self {
        let (background_preset, background) = background_parts(a.background.as_ref());
        ChoiceRow {
            theme: a.theme.clone(),
            skin: a.skin.clone(),
            background_preset,
            background,
            mode: a.mode,
            density: a.density,
            code_size: a.code_size,
            chosen: a.chosen,
        }
    }
}

impl TryFrom<ChoiceRow> for Appearance {
    type Error = StoreError;

    fn try_from(row: ChoiceRow) -> Result<Self, StoreError> {
        Ok(Appearance {
            theme: row.theme,
            skin: row.skin,
            background: background_of(row.background_preset, row.background)?,
            mode: row.mode,
            density: row.density,
            code_size: row.code_size,
            chosen: row.chosen,
        })
    }
}

impl From<&ThemeFile> for ThemeRow {
    fn from(t: &ThemeFile) -> Self {
        let (background_preset, background) = background_parts(t.background.as_ref());
        ThemeRow {
            schema: t.schema,
            id: t.id.clone(),
            name: t.name.clone(),
            about: t.about.clone(),
            skin: t.skin.clone(),
            background_preset,
            background,
            palette: Some(PaletteRow::from(&t.palette)),
            overrides: Some(PaletteRow::from(&t.overrides)),
            source: t.source.clone(),
        }
    }
}

impl TryFrom<ThemeRow> for ThemeFile {
    type Error = StoreError;

    fn try_from(row: ThemeRow) -> Result<Self, StoreError> {
        Ok(ThemeFile {
            schema: row.schema,
            id: row.id,
            name: row.name,
            about: row.about,
            skin: row.skin,
            palette: row.palette.map(Palettes::from).unwrap_or_default(),
            background: background_of(row.background_preset, row.background)?,
            overrides: row.overrides.map(Palettes::from).unwrap_or_default(),
            source: row.source,
        })
    }
}

impl From<&Palettes> for PaletteRow {
    fn from(p: &Palettes) -> Self {
        PaletteRow {
            light: p.light.as_ref().map(|entries| Colours {
                entries: entries.clone(),
            }),
            dark: p.dark.as_ref().map(|entries| Colours {
                entries: entries.clone(),
            }),
        }
    }
}

impl From<PaletteRow> for Palettes {
    fn from(row: PaletteRow) -> Self {
        Palettes {
            light: row.light.map(|c| c.entries),
            dark: row.dark.map(|c| c.entries),
        }
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
    fn default_document_on_an_empty_store() {
        let store = Store::new();
        assert_eq!(store.get(), Document::default());
    }

    #[test]
    fn put_choice_accepts_a_builtin_and_refuses_an_unknown_theme() {
        let store = Store::new();
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
        let store = Store::new();
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
        let store = Store::new();
        assert!(store.save_theme(theme(DEFAULT_THEME)).is_err());
        assert!(store.delete_theme(DEFAULT_THEME).is_err());
        assert_eq!(store.get(), Document::default());
    }

    #[test]
    fn the_document_round_trips_through_the_store() {
        let store = Store::new();
        store
            .save_theme(theme("my-theme"))
            .expect("the theme is valid");
        store
            .put_choice(Appearance {
                background: Some(BackgroundChoice::Spec(Background::default())),
                ..choice("my-theme")
            })
            .expect("the choice is valid");
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &store.rows().unwrap()).unwrap();
        let reloaded = Store::new();
        reloaded.load(&conn).unwrap();
        assert_eq!(
            serde_json::to_value(reloaded.get()).unwrap(),
            serde_json::to_value(store.get()).unwrap()
        );
    }
}
