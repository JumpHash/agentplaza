//! The prompt editor (docs/design/prompt-library.md, "Screens", the web's prompt editor at 1:1): the markdown source beside
//! a live preview, the findings, score and token estimate checked locally with `apz_prompt` as you type (each finding with
//! a line jumps the source to it), Save version, Publish, Unpublish and Archive, the Details form (title, description, tags
//! and binding, edited in place), and the Versions (view, compare, publish, restore), History and Rules tabs. `prompts`
//! owns it: `open` starts it for one prompt and `Msg::Back` returns to the list. The hub's JSON is read through
//! `apz_http` with the status kept, so a 409 (a newer version was saved) is told apart from other failures.

use std::cmp::Reverse;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use apz_prompt::{Finding, Kind, Report, Severity};
use iced::widget::text_editor::{Cursor, Position};
use iced::widget::{
    Space, button, checkbox, column, container, operation, row, text, text_editor, text_input,
};
use iced::{Alignment, Background, Border, Color, Element, Font, Length, Task, Theme};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::console::{Ctx, client, ui};
use crate::theme::palette;

use super::{mcp_hub, prompts};

// ---- The hub's JSON (apps/web/src/api.ts, "The prompt library") --------------------------------------------------

/// One prompt as the list shows it. `score` is the live version's, `None` when nothing is published.
#[derive(Debug, Clone, Deserialize)]
pub struct PromptSummary {
    pub title: String,
    pub kind: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub binding: Option<String>,
    #[serde(default)]
    pub active_version: Option<u32>,
    pub latest_version: u32,
    #[serde(default)]
    pub best_version: Option<u32>,
    #[serde(default)]
    pub score: Option<u8>,
    #[serde(default)]
    pub archived_ms: Option<u64>,
}

/// One version. The prompt's list leaves out `body`, `findings` and `stale`; a single version carries them all.
#[derive(Debug, Clone, Deserialize)]
pub struct PromptVersion {
    pub version: u32,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub author: String,
    pub created_ms: u64,
    pub score: u8,
    pub errors: u32,
    pub warnings: u32,
    pub infos: u32,
    pub rules_version: u32,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub findings: Vec<Finding>,
    #[serde(default)]
    pub stale: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PromptDetail {
    pub prompt: PromptSummary,
    pub versions: Vec<PromptVersion>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PromptDiffLine {
    pub op: String,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PromptDiff {
    pub from: PromptVersion,
    pub to: PromptVersion,
    pub lines: Vec<PromptDiffLine>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PromptEvent {
    pub action: String,
    pub version: Option<u32>,
    pub actor: String,
    pub at_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AddVersion {
    pub version: PromptVersion,
    pub created: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct Events {
    events: Vec<PromptEvent>,
}

/// The save's error text when the hub answers 409: a newer version was saved first.
const CONFLICT: &str = "a newer version was saved; reload";

// ---- The screen ----------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Edit,
    Versions,
    History,
    Rules,
}

const TABS: [(Tab, &str); 4] = [
    (Tab::Edit, "Edit"),
    (Tab::Versions, "Versions"),
    (Tab::History, "History"),
    (Tab::Rules, "Rules"),
];

#[derive(Debug, Clone)]
pub enum Msg {
    /// Back to the list (handled by `prompts`, which asks first when the draft has unsaved changes).
    Back,
    /// Leave without saving (handled by `prompts`), after the confirmation.
    Discard,
    /// Keep editing after the confirmation.
    KeepEditing,
    Tab(Tab),
    /// The prompt, its latest version and its live version (the latest when none is live).
    Loaded(Result<(PromptDetail, PromptVersion, PromptVersion), String>),
    Refreshed(Result<(PromptDetail, PromptVersion), String>),
    Edit(text_editor::Action),
    Note(String),
    Save,
    Saved(Result<AddVersion, String>),
    /// Reload the prompt after a 409, which discards the draft.
    ReloadLatest,
    /// A publish, unpublish, archive or restore went through (its notice), or failed.
    Acted(Result<String, String>),
    /// Open the publish form (the version chosen starts as the latest).
    OpenPublish,
    ClosePublish,
    PublishPick(u32),
    Publish(u32),
    Unpublish,
    ToggleArchive,
    View(u32),
    Viewed(Result<PromptVersion, String>),
    CloseView,
    /// Pick a version to compare; the second pick compares the two.
    Pick(u32),
    ClearPicks,
    Compared(Result<PromptDiff, String>),
    RestoreVersion(u32),
    Restored(Result<PromptVersion, String>),
    HistoryLoaded(Result<Vec<PromptEvent>, String>),
    /// Save the latest version as a Markdown file: the save dialog first, then the body.
    Download,
    DownloadPicked(Option<PathBuf>),
    Downloaded(Result<(PromptVersion, PathBuf), String>),
    /// Move the source to a finding's line (1-based; 0 is the whole document and moves nothing).
    Jump(u32),
    /// Open the Details form with the prompt's current metadata, or close it without saving.
    OpenDetails,
    CloseDetails,
    DetailText(Field, String),
    DetailEvery(bool),
    /// Save the Details form: a PUT of the metadata, then the prompt reloads.
    SaveDetails,
    DetailsSaved(Result<(), String>),
}

/// A Details form field that takes text.
#[derive(Debug, Clone, Copy)]
pub enum Field {
    Title,
    Description,
    Tags,
    Role,
}

/// The Details form's draft, as the create form takes the same fields: tags typed comma separated, and the binding as
/// a role (system prompts) or the every-agent box (AGENTS.md).
#[derive(Debug, Clone, Default)]
pub struct Details {
    title: String,
    description: String,
    tags: String,
    role: String,
    every: bool,
}

impl Details {
    /// The form's draft from the prompt's current metadata.
    fn of(summary: &PromptSummary) -> Self {
        let (role, every) = match summary.binding.as_deref() {
            Some("plaza") => (String::new(), true),
            Some(binding) => (
                binding
                    .strip_prefix("role:")
                    .unwrap_or_default()
                    .to_string(),
                false,
            ),
            None => (String::new(), false),
        };
        Details {
            title: summary.title.clone(),
            description: summary.description.clone(),
            tags: summary.tags.join(", "),
            role,
            every,
        }
    }
}

/// The prompt as loaded: the editor's draft, the checks on it, and the versions' tabs' state.
#[derive(Debug)]
struct Loaded {
    summary: PromptSummary,
    /// Newest first.
    versions: Vec<PromptVersion>,
    kind: Kind,
    source: text_editor::Content,
    /// The body last loaded or saved, normalised as the draft is, so the draft differs from it only by an edit.
    saved: String,
    /// The live version's body: what the draft is compared with on the Edit tab.
    live: String,
    /// The version the publish form has chosen, while the form is open.
    publish: Option<u32>,
    /// The draft being saved, until the hub answers.
    pending: String,
    note: String,
    report: Report,
    viewing: Option<PromptVersion>,
    /// The versions picked to compare (at most two).
    picked: Vec<u32>,
    compare: Option<PromptDiff>,
    /// None until the History tab was opened.
    events: Option<Vec<PromptEvent>>,
    /// The Details form's draft while the form is open.
    details: Option<Details>,
}

#[derive(Debug)]
pub struct State {
    pub id: String,
    tab: Tab,
    loaded: Option<Loaded>,
    error: Option<String>,
    /// The last outcome: (text, is_error).
    notice: Option<(String, bool)>,
    /// A save was refused with 409: offer a reload of the latest version.
    conflict: bool,
    /// Back was pressed with unsaved changes: asking whether to discard them.
    leaving: bool,
    /// The hub changed this prompt while the draft had unsaved edits: offer a reload instead of replacing the draft.
    changed: bool,
}

/// The editor for prompt `id`, and the task that loads it.
pub fn open(id: String, ctx: &Ctx) -> (State, Task<Msg>) {
    let task = load(&id, ctx);
    (
        State {
            id,
            tab: Tab::Edit,
            loaded: None,
            error: None,
            notice: None,
            conflict: false,
            leaving: false,
            changed: false,
        },
        task,
    )
}

/// Whether the draft differs from the body last loaded or saved.
pub fn is_dirty(state: &State) -> bool {
    state.loaded.as_ref().is_some_and(dirty)
}

/// Ask before leaving: the view shows Discard and Keep editing until one is chosen.
pub fn ask_leave(state: &mut State) {
    state.leaving = true;
}

/// The hub changed this prompt: reload it when the draft has no unsaved edits, otherwise say so and keep the draft. An
/// open Details form counts as unsaved, since a reload would close it.
pub fn hub_changed(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let details_open = state.loaded.as_ref().is_some_and(|l| l.details.is_some());
    if is_dirty(state) || details_open {
        state.changed = true;
        Task::none()
    } else {
        load(&state.id, ctx)
    }
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Back | Msg::Discard => Task::none(),
        Msg::KeepEditing => {
            state.leaving = false;
            Task::none()
        }
        Msg::ReloadLatest => {
            state.conflict = false;
            state.notice = None;
            load(&state.id, ctx)
        }
        Msg::Tab(tab) => {
            state.tab = tab;
            let wants_history =
                tab == Tab::History && state.loaded.as_ref().is_some_and(|l| l.events.is_none());
            if wants_history {
                history(&state.id, ctx)
            } else {
                Task::none()
            }
        }
        Msg::Loaded(Ok((detail, latest, live))) => {
            let kind = Kind::parse(&detail.prompt.kind).unwrap_or(Kind::Fragment);
            state.changed = false;
            state.loaded = Some(Loaded {
                report: apz_prompt::check(kind, &latest.body),
                source: text_editor::Content::with_text(&latest.body),
                saved: source_text(&text_editor::Content::with_text(&latest.body)),
                live: live.body,
                publish: None,
                pending: String::new(),
                kind,
                summary: detail.prompt,
                versions: sorted(detail.versions),
                note: String::new(),
                viewing: None,
                picked: Vec::new(),
                compare: None,
                events: None,
                details: None,
            });
            state.error = None;
            Task::none()
        }
        Msg::Loaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::Refreshed(Ok((detail, live))) => {
            if let Some(l) = state.loaded.as_mut() {
                l.summary = detail.prompt;
                l.versions = sorted(detail.versions);
                l.live = live.body;
            }
            Task::none()
        }
        Msg::Refreshed(Err(e)) => {
            state.notice = Some((e, true));
            Task::none()
        }
        Msg::Edit(action) => {
            if let Some(l) = state.loaded.as_mut() {
                l.source.perform(action);
                l.report = apz_prompt::check(l.kind, &source_text(&l.source));
            }
            Task::none()
        }
        Msg::Note(note) => {
            if let Some(l) = state.loaded.as_mut() {
                l.note = note;
            }
            Task::none()
        }
        Msg::Save => match state.loaded.as_mut() {
            Some(l) => {
                l.pending = source_text(&l.source);
                save(&state.id, l, ctx)
            }
            None => Task::none(),
        },
        Msg::Saved(Ok(added)) => {
            state.conflict = false;
            state.changed = false;
            state.notice = Some(if added.created {
                (format!("Saved as v{}", added.version.version), false)
            } else {
                (
                    "No changes: the body is the same as the latest version.".to_string(),
                    false,
                )
            });
            if let Some(l) = state.loaded.as_mut() {
                l.saved = std::mem::take(&mut l.pending);
                if added.created {
                    l.note.clear();
                }
            }
            refresh(&state.id, ctx)
        }
        Msg::Saved(Err(e)) => {
            state.conflict = e == CONFLICT;
            state.notice = Some(if state.conflict {
                ("A newer version was saved. Reload the latest to see it; the draft is replaced.".to_string(), true)
            } else {
                (e, true)
            });
            Task::none()
        }
        Msg::Acted(Ok(text)) => {
            state.notice = Some((text, false));
            state.changed = false;
            refresh(&state.id, ctx)
        }
        Msg::Acted(Err(e)) => {
            state.notice = Some((e, true));
            Task::none()
        }
        Msg::OpenPublish => {
            if let Some(l) = state.loaded.as_mut() {
                l.publish = Some(l.summary.latest_version);
            }
            Task::none()
        }
        Msg::ClosePublish => {
            if let Some(l) = state.loaded.as_mut() {
                l.publish = None;
            }
            Task::none()
        }
        Msg::PublishPick(version) => {
            if let Some(l) = state.loaded.as_mut() {
                l.publish = Some(version);
            }
            Task::none()
        }
        Msg::Publish(version) => {
            if let Some(l) = state.loaded.as_mut() {
                l.publish = None;
            }
            act(
                &state.id,
                ctx,
                "publish",
                Some(json!({ "version": version })),
                format!("Published v{version}"),
            )
        }
        Msg::Unpublish => act(&state.id, ctx, "unpublish", None, "Unpublished".to_string()),
        Msg::ToggleArchive => {
            let archived = state
                .loaded
                .as_ref()
                .is_some_and(|l| l.summary.archived_ms.is_some());
            if archived {
                act(&state.id, ctx, "restore", None, "Restored".to_string())
            } else {
                act(&state.id, ctx, "archive", None, "Archived".to_string())
            }
        }
        Msg::View(version) => version_task(&state.id, ctx, version, Msg::Viewed),
        Msg::Viewed(Ok(version)) => {
            if let Some(l) = state.loaded.as_mut() {
                l.viewing = Some(version);
            }
            Task::none()
        }
        Msg::Viewed(Err(e)) => {
            state.notice = Some((e, true));
            Task::none()
        }
        Msg::CloseView => {
            if let Some(l) = state.loaded.as_mut() {
                l.viewing = None;
            }
            Task::none()
        }
        Msg::Pick(version) => {
            if let Some(l) = state.loaded.as_mut() {
                l.compare = None;
                if let Some(at) = l.picked.iter().position(|&p| p == version) {
                    l.picked.remove(at);
                } else {
                    if l.picked.len() == 2 {
                        l.picked.remove(0);
                    }
                    l.picked.push(version);
                }
            }
            // The second pick compares the two, as the web does.
            if state.loaded.as_ref().is_some_and(|l| l.picked.len() == 2) {
                compare_task(state, ctx)
            } else {
                Task::none()
            }
        }
        Msg::ClearPicks => {
            if let Some(l) = state.loaded.as_mut() {
                l.picked.clear();
                l.compare = None;
            }
            Task::none()
        }
        Msg::Compared(Ok(diff)) => {
            if let Some(l) = state.loaded.as_mut() {
                l.compare = Some(diff);
            }
            Task::none()
        }
        Msg::Compared(Err(e)) => {
            state.notice = Some((e, true));
            Task::none()
        }
        Msg::RestoreVersion(version) => version_task(&state.id, ctx, version, Msg::Restored),
        Msg::Restored(Ok(version)) => {
            if let Some(l) = state.loaded.as_mut() {
                l.source = text_editor::Content::with_text(&version.body);
                l.report = apz_prompt::check(l.kind, &version.body);
                l.note = format!("Restored from v{}", version.version);
            }
            state.tab = Tab::Edit;
            state.notice = Some((
                format!(
                    "v{} is in the editor. Save it to make it the latest version.",
                    version.version
                ),
                false,
            ));
            Task::none()
        }
        Msg::Restored(Err(e)) => {
            state.notice = Some((e, true));
            Task::none()
        }
        Msg::HistoryLoaded(Ok(events)) => {
            if let Some(l) = state.loaded.as_mut() {
                l.events = Some(events);
            }
            Task::none()
        }
        Msg::HistoryLoaded(Err(e)) => {
            state.notice = Some((e, true));
            Task::none()
        }
        Msg::Download => match state.loaded.as_ref() {
            Some(l) => {
                let name = if matches!(l.kind, Kind::AgentsMd) {
                    "AGENTS.md".to_string()
                } else {
                    format!("{}-v{}.md", state.id, l.summary.latest_version)
                };
                Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .set_title("Save the prompt as Markdown")
                            .set_file_name(name)
                            .add_filter("Markdown", &["md"])
                            .save_file()
                            .await
                            .map(|file| file.path().to_path_buf())
                    },
                    Msg::DownloadPicked,
                )
            }
            None => Task::none(),
        },
        Msg::DownloadPicked(None) => Task::none(),
        Msg::DownloadPicked(Some(path)) => match state.loaded.as_ref() {
            Some(l) => {
                let (c, id, version) = (
                    ctx.client.clone(),
                    state.id.clone(),
                    l.summary.latest_version,
                );
                client::task(
                    move || {
                        get::<PromptVersion>(
                            &c,
                            &format!("{}/versions/{version}", prompt_path(&id)),
                        )
                        .map(|v| (v, path))
                    },
                    Msg::Downloaded,
                )
            }
            None => Task::none(),
        },
        Msg::Downloaded(Ok((version, path))) => {
            state.notice = Some(match std::fs::write(&path, version.body.as_bytes()) {
                Ok(()) => (
                    format!("Saved v{} to {}", version.version, path.display()),
                    false,
                ),
                Err(e) => (format!("The file could not be written: {e}"), true),
            });
            Task::none()
        }
        Msg::Downloaded(Err(e)) => {
            state.notice = Some((e, true));
            Task::none()
        }
        Msg::Jump(line) => {
            let cursor = state
                .loaded
                .as_ref()
                .and_then(|l| finding_cursor(&source_text(&l.source), line));
            if let (Some(l), Some(cursor)) = (state.loaded.as_mut(), cursor) {
                l.source.move_to(cursor);
            }
            operation::focus(source_id())
        }
        Msg::OpenDetails => {
            if let Some(l) = state.loaded.as_mut() {
                l.details = Some(Details::of(&l.summary));
            }
            Task::none()
        }
        Msg::CloseDetails => {
            if let Some(l) = state.loaded.as_mut() {
                l.details = None;
            }
            Task::none()
        }
        Msg::DetailText(field, text) => {
            if let Some(d) = state.loaded.as_mut().and_then(|l| l.details.as_mut()) {
                match field {
                    Field::Title => d.title = text,
                    Field::Description => d.description = text,
                    Field::Tags => d.tags = text,
                    Field::Role => d.role = text,
                }
            }
            Task::none()
        }
        Msg::DetailEvery(on) => {
            if let Some(d) = state.loaded.as_mut().and_then(|l| l.details.as_mut()) {
                d.every = on;
            }
            Task::none()
        }
        Msg::SaveDetails => {
            let body = state
                .loaded
                .as_ref()
                .and_then(|l| l.details.as_ref().map(|d| meta_body(l.kind, d)));
            match body {
                Some(Ok(body)) => {
                    let (c, id) = (ctx.client.clone(), state.id.clone());
                    client::task(
                        move || {
                            send(&c, "PUT", &prompt_path(&id), Some(&body))
                                .and_then(expect::<Value>)
                                .map(|_| ())
                        },
                        Msg::DetailsSaved,
                    )
                }
                Some(Err(e)) => {
                    state.notice = Some((e, true));
                    Task::none()
                }
                None => Task::none(),
            }
        }
        Msg::DetailsSaved(Ok(())) => {
            state.notice = Some(("Details saved".to_string(), false));
            state.changed = false;
            if let Some(l) = state.loaded.as_mut() {
                l.details = None;
            }
            refresh(&state.id, ctx)
        }
        Msg::DetailsSaved(Err(e)) => {
            state.notice = Some((e, true));
            Task::none()
        }
    }
}

// ---- Hub calls -----------------------------------------------------------------------------------------------------

/// The answer to one call: the HTTP status and the JSON body.
type Answer = (u16, Value);

fn prompt_path(id: &str) -> String {
    format!("/v1/prompts/{id}")
}

fn send(
    c: &client::Client,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Answer, String> {
    apz_http::client::request_json(&c.target, method, path, body, c.timeout)
        .map_err(|e| e.to_string())
}

/// A 2xx answer as `T`; anything else as the hub's `error` text (or the status).
fn expect<T: DeserializeOwned>((status, value): Answer) -> Result<T, String> {
    if (200..300).contains(&status) {
        serde_json::from_value(value).map_err(|e| e.to_string())
    } else {
        Err(value
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("HTTP {status}")))
    }
}

fn get<T: DeserializeOwned>(c: &client::Client, path: &str) -> Result<T, String> {
    expect(send(c, "GET", path, None)?)
}

fn load(id: &str, ctx: &Ctx) -> Task<Msg> {
    let (c, id) = (ctx.client.clone(), id.to_string());
    client::task(
        move || {
            let detail: PromptDetail = get(&c, &prompt_path(&id))?;
            let latest: PromptVersion = get(
                &c,
                &format!(
                    "{}/versions/{}",
                    prompt_path(&id),
                    detail.prompt.latest_version
                ),
            )?;
            let live = match detail.prompt.active_version {
                Some(n) if n != latest.version => live_version(&c, &id, &detail)?,
                _ => latest.clone(),
            };
            Ok((detail, latest, live))
        },
        Msg::Loaded,
    )
}

/// The live version's body, from the hub: the published version, or the latest when none is published.
fn live_version(
    c: &client::Client,
    id: &str,
    detail: &PromptDetail,
) -> Result<PromptVersion, String> {
    let n = detail
        .prompt
        .active_version
        .unwrap_or(detail.prompt.latest_version);
    get::<PromptVersion>(c, &format!("{}/versions/{n}", prompt_path(id)))
}

fn refresh(id: &str, ctx: &Ctx) -> Task<Msg> {
    let (c, id) = (ctx.client.clone(), id.to_string());
    client::task(
        move || {
            let detail: PromptDetail = get(&c, &prompt_path(&id))?;
            let live = live_version(&c, &id, &detail)?;
            Ok((detail, live))
        },
        Msg::Refreshed,
    )
}

/// The two picked versions' line diff, from the hub (the lower number first).
fn compare_task(state: &State, ctx: &Ctx) -> Task<Msg> {
    let Some(l) = state.loaded.as_ref() else {
        return Task::none();
    };
    let (from, to) = (l.picked[0].min(l.picked[1]), l.picked[0].max(l.picked[1]));
    let (c, id) = (ctx.client.clone(), state.id.clone());
    client::task(
        move || {
            get::<PromptDiff>(
                &c,
                &format!("{}/diff?from={from}&to={to}", prompt_path(&id)),
            )
        },
        Msg::Compared,
    )
}

fn history(id: &str, ctx: &Ctx) -> Task<Msg> {
    let (c, id) = (ctx.client.clone(), id.to_string());
    client::task(
        move || get::<Events>(&c, &format!("{}/history", prompt_path(&id))).map(|list| list.events),
        Msg::HistoryLoaded,
    )
}

fn version_task(
    id: &str,
    ctx: &Ctx,
    version: u32,
    map: fn(Result<PromptVersion, String>) -> Msg,
) -> Task<Msg> {
    let (c, id) = (ctx.client.clone(), id.to_string());
    client::task(
        move || get::<PromptVersion>(&c, &format!("{}/versions/{version}", prompt_path(&id))),
        map,
    )
}

/// Save the draft as a new version on top of the latest one. 409 means someone saved a newer version first.
fn save(id: &str, l: &Loaded, ctx: &Ctx) -> Task<Msg> {
    let (c, id) = (ctx.client.clone(), id.to_string());
    let body = source_text(&l.source);
    let note = l.note.trim().to_string();
    let base = l.summary.latest_version;
    client::task(
        move || {
            let answer = send(
                &c,
                "POST",
                &format!("{}/versions", prompt_path(&id)),
                Some(&json!({ "body": body, "note": note, "base_version": base })),
            )?;
            if answer.0 == 409 {
                return Err(CONFLICT.to_string());
            }
            expect::<AddVersion>(answer)
        },
        Msg::Saved,
    )
}

/// A POST to one of the prompt's action routes (publish, unpublish, archive, restore); `done` is the notice on success.
fn act(id: &str, ctx: &Ctx, action: &str, body: Option<Value>, done: String) -> Task<Msg> {
    let (c, path) = (ctx.client.clone(), format!("{}/{action}", prompt_path(id)));
    client::task(
        move || {
            send(&c, "POST", &path, body.as_ref())
                .and_then(expect::<Value>)
                .map(|_| done)
        },
        Msg::Acted,
    )
}

// ---- Helpers -------------------------------------------------------------------------------------------------------

/// The draft's text, without the trailing newline the editor adds.
fn source_text(source: &text_editor::Content) -> String {
    let text = source.text();
    text.strip_suffix('\n').map(str::to_string).unwrap_or(text)
}

/// The draft differs from the body last loaded or saved.
fn dirty(l: &Loaded) -> bool {
    source_text(&l.source) != l.saved
}

fn sorted(mut versions: Vec<PromptVersion>) -> Vec<PromptVersion> {
    versions.sort_by_key(|v| Reverse(v.version));
    versions
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn score_tone(score: u8) -> ui::Tone {
    match score {
        90..=100 => ui::Tone::Ok,
        70..=89 => ui::Tone::Warn,
        _ => ui::Tone::Danger,
    }
}

fn severity_tone(severity: Severity) -> ui::Tone {
    match severity {
        Severity::Error => ui::Tone::Danger,
        Severity::Warning => ui::Tone::Warn,
        Severity::Info => ui::Tone::Info,
    }
}

/// The id of the source editor, so a finding's jump can focus it.
fn source_id() -> iced::widget::Id {
    iced::widget::Id::new("prompt-source")
}

/// The cursor that selects a finding's line (1-based) in `text`, from the line's start to its end. A line past the end
/// selects the last line. Line 0 is the whole document, which has no line to select.
pub fn finding_cursor(text: &str, line: u32) -> Option<Cursor> {
    if line == 0 {
        return None;
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let at = (line as usize).min(lines.len()) - 1;
    let end = lines[at].len();
    Some(Cursor {
        position: Position {
            line: at,
            column: end,
        },
        selection: Some(Position {
            line: at,
            column: 0,
        }),
    })
}

/// The tags typed comma separated: each trimmed, the empty ones dropped (the create form's rule).
pub fn parse_tags(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect()
}

/// The binding a prompt of `kind` takes from its form: a role for a system prompt, every agent for AGENTS.md, none for a
/// fragment (the hub binds no fragment).
pub fn binding_for(kind: Kind, role: &str, every: bool) -> Option<String> {
    match kind {
        Kind::System => {
            let role = role.trim();
            (!role.is_empty()).then(|| format!("role:{role}"))
        }
        Kind::AgentsMd => every.then(|| "plaza".to_string()),
        Kind::Fragment => None,
    }
}

/// The PUT body for a prompt's metadata, from the Details form. A blank title is refused here, as the hub would.
pub fn meta_body(kind: Kind, d: &Details) -> Result<Value, String> {
    if d.title.trim().is_empty() {
        return Err("Give the prompt a title.".to_string());
    }
    Ok(json!({
        "title": d.title.trim(),
        "description": d.description.trim(),
        "tags": parse_tags(&d.tags),
        "binding": binding_for(kind, &d.role, d.every),
    }))
}

// ---- Views ---------------------------------------------------------------------------------------------------------

/// The kind's name, as the editor writes it (the web's KIND_NAME).
fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::System => "System prompt",
        Kind::AgentsMd => "AGENTS.md",
        Kind::Fragment => "Fragment",
    }
}

/// One line of a diff: unchanged, added or removed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DiffOp {
    Same,
    Add,
    Del,
}

fn diff_op(op: &str) -> DiffOp {
    match op {
        "add" => DiffOp::Add,
        "del" => DiffOp::Del,
        _ => DiffOp::Same,
    }
}

/// The line diff of `a` against `b` (their longest common subsequence of lines), in order, as the web's editor draws it.
fn line_diff(a: &str, b: &str) -> Vec<(DiffOp, String)> {
    let a_lines: Vec<&str> = a.split('\n').collect();
    let b_lines: Vec<&str> = b.split('\n').collect();
    let (n, m) = (a_lines.len(), b_lines.len());
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a_lines[i] == b_lines[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::with_capacity(n + m);
    while i < n && j < m {
        if a_lines[i] == b_lines[j] {
            out.push((DiffOp::Same, a_lines[i].to_string()));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            out.push((DiffOp::Del, a_lines[i].to_string()));
            i += 1;
        } else {
            out.push((DiffOp::Add, b_lines[j].to_string()));
            j += 1;
        }
    }
    out.extend(a_lines[i..].iter().map(|l| (DiffOp::Del, l.to_string())));
    out.extend(b_lines[j..].iter().map(|l| (DiffOp::Add, l.to_string())));
    out
}

/// A bordered well on the paper ground, the web's `.diff` and preview boxes: a rule and the skin's radius.
pub(super) fn well_style(_: &Theme) -> container::Style {
    let p = palette();
    container::Style {
        text_color: Some(p.ink),
        background: Some(p.paper.into()),
        border: Border {
            color: p.rule,
            width: 1.0,
            radius: crate::theme::skin().radius.into(),
        },
        ..Default::default()
    }
}

/// A card's surface with no head: the web's `.card` holding only a body, padded 16.
fn plain_card<'a>(body: Element<'a, Msg>) -> Element<'a, Msg> {
    container(body)
        .padding(16)
        .width(Length::Fill)
        .style(ui::card_style)
        .into()
}

/// The diff's rows: the sign, then the line; an added line on the ok ground, a removed one on the crit ground. `cap`
/// bounds the box's height (the compare panel scrolls); without it the box grows with its lines.
fn diff_box<'a>(rows: Vec<(DiffOp, String)>, cap: Option<f32>) -> Element<'a, Msg> {
    let p = palette();
    let lines: Vec<Element<'a, Msg>> = rows
        .into_iter()
        .map(|(op, line)| {
            let (sign, ink, ground) = match op {
                DiffOp::Same => (' ', p.ink_2, None),
                DiffOp::Add => ('+', p.ok, Some(p.ok_soft)),
                DiffOp::Del => ('−', p.crit, Some(p.crit_soft)),
            };
            container(ui::mono(format!("{sign} {line}"), 12.5, ink))
                .padding([1, 10])
                .width(Length::Fill)
                .style(move |_: &Theme| container::Style {
                    background: ground.map(Background::Color),
                    ..Default::default()
                })
                .into()
        })
        .collect();
    let body: Element<'a, Msg> = match cap {
        Some(height) => ui::scroll(column(lines).width(Length::Fill))
            .height(Length::Fixed(height))
            .into(),
        None => column(lines).width(Length::Fill).into(),
    };
    container(body)
        .padding([6, 0])
        .width(Length::Fill)
        .style(well_style)
        .into()
}

/// A hairline across, in `color`.
pub(super) fn rule<'a, M: 'a>(color: Color) -> Element<'a, M> {
    container(Space::new().height(1.0))
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(color.into()),
            ..Default::default()
        })
        .into()
}

/// A table as the web's `.tbl`: a head row in the label voice over a strong rule, then a rule under each row, in a card's
/// surface. Each column takes its portion of the row; a width of 0 is the width of its content (the actions).
pub(super) fn table<'a, M: 'a>(
    heads: &[&str],
    widths: &[u16],
    rows: Vec<Vec<Element<'a, M>>>,
) -> Element<'a, M> {
    let p = palette();
    let cell = |w: u16, content: Element<'a, M>| -> Element<'a, M> {
        let c = container(content).padding([9, 12]);
        if w == 0 {
            c.into()
        } else {
            c.width(Length::FillPortion(w)).into()
        }
    };
    let head_cells: Vec<Element<'a, M>> = heads
        .iter()
        .zip(widths)
        .map(|(name, &w)| cell(w, ui::label(name).into()))
        .collect();
    let mut lines: Vec<Element<'a, M>> = vec![
        row(head_cells)
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .into(),
        rule(p.rule_strong),
    ];
    for cells in rows {
        let row_cells: Vec<Element<'a, M>> = cells
            .into_iter()
            .zip(widths)
            .map(|(content, &w)| cell(w, content))
            .collect();
        lines.push(
            row(row_cells)
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .into(),
        );
        lines.push(rule(p.rule));
    }
    container(column(lines))
        .width(Length::Fill)
        .style(ui::card_style)
        .into()
}

/// The Edit, Versions, History and Rules tabs, as the web's `.tabs`: the Versions count after its label.
fn tab_strip<'a>(current: Tab, versions: usize) -> Element<'a, Msg> {
    let p = palette();
    let mut strip = row![].spacing(22);
    for (tab, name) in TABS {
        let on = tab == current;
        let mut label = row![text(name).font(ui::SANS_MEDIUM).size(13.0)]
            .spacing(6)
            .align_y(Alignment::Center);
        if tab == Tab::Versions {
            label = label.push(ui::count(versions as u32));
        }
        let body = column![
            container(label).center_y(Length::Fixed(34.0)),
            container(Space::new().height(2.0))
                .width(Length::Fill)
                .style(move |_: &Theme| container::Style {
                    background: on.then(|| p.action.into()),
                    ..Default::default()
                }),
        ];
        let cell = button(body)
            .padding(0.0)
            .style(move |_: &Theme, status: button::Status| {
                let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
                button::Style {
                    background: None,
                    text_color: if on || hovered { p.ink } else { p.ink_3 },
                    border: Border::default(),
                    ..Default::default()
                }
            })
            .on_press(Msg::Tab(tab));
        strip = strip.push(cell);
    }
    column![strip, ui::hairline()].into()
}

/// The page head: the title with the prompt's actions, the kind and id line, and the status badges (live, latest, score,
/// binding, archived, unsaved), with the hub's change notice when the draft is kept.
fn header<'a>(state: &'a State, l: &'a Loaded) -> Element<'a, Msg> {
    let s = &l.summary;
    let archived = s.archived_ms.is_some();
    let editing = l.details.is_some();
    let mut actions: Vec<Element<'a, Msg>> = vec![
        ui::btn_sm("Back to prompts", None, ui::Kind::Ghost, Some(Msg::Back)),
        ui::btn_sm("Download .md", None, ui::Kind::Ghost, Some(Msg::Download)),
        ui::btn_sm(
            if editing {
                "Close details"
            } else {
                "Details…"
            },
            None,
            ui::Kind::Ghost,
            Some(if editing {
                Msg::CloseDetails
            } else {
                Msg::OpenDetails
            }),
        ),
        ui::btn_sm(
            "Publish…",
            None,
            ui::Kind::Ghost,
            Some(if l.publish.is_some() {
                Msg::ClosePublish
            } else {
                Msg::OpenPublish
            }),
        ),
    ];
    if s.active_version.is_some() {
        actions.push(ui::btn_sm(
            "Unpublish",
            None,
            ui::Kind::Ghost,
            Some(Msg::Unpublish),
        ));
    }
    actions.push(ui::btn_sm(
        if archived { "Restore" } else { "Archive" },
        None,
        ui::Kind::Ghost,
        Some(Msg::ToggleArchive),
    ));
    let title = if s.title.is_empty() {
        "Untitled prompt"
    } else {
        s.title.as_str()
    };
    let mut bar: Vec<Element<'a, Msg>> = vec![match s.active_version {
        Some(v) => ui::badge(&format!("Live v{v}"), ui::Tone::Ok),
        None => ui::badge("Not published", ui::Tone::Muted),
    }];
    bar.push(ui::badge(
        &format!("Latest v{}", s.latest_version),
        ui::Tone::Muted,
    ));
    if let Some(score) = s.score {
        bar.push(ui::badge(&format!("Score {score}"), score_tone(score)));
    }
    if let Some(binding) = s.binding.as_deref() {
        bar.push(ui::badge(&format!("Binds {binding}"), ui::Tone::Signal));
    }
    if archived {
        bar.push(ui::badge("Archived", ui::Tone::Muted));
    }
    if dirty(l) {
        bar.push(ui::badge("Unsaved changes", ui::Tone::Warn));
    }
    if state.changed && !state.conflict {
        bar.push(ui::badge("Changed on the hub", ui::Tone::Warn));
        bar.push(ui::btn_sm(
            "Reload",
            None,
            ui::Kind::Ghost,
            Some(Msg::ReloadLatest),
        ));
    }
    column![
        ui::view_head(title, actions),
        ui::faint(format!("{} · {}", kind_name(l.kind), state.id)),
        row(bar).spacing(8).align_y(Alignment::Center).wrap(),
    ]
    .spacing(8)
    .into()
}

/// One finding: its severity in the tone, the place (a line moves the source to it), the message and the rule's id, in
/// a ruled box. With `jump`, a finding with a line is a link that moves the source to that line.
fn finding_row(f: &Finding, jump: bool) -> Element<'static, Msg> {
    let (name, tone) = match f.severity {
        Severity::Error => ("Error", ui::Tone::Danger),
        Severity::Warning => ("Warning", ui::Tone::Warn),
        Severity::Info => ("Info", ui::Tone::Info),
    };
    let place: Element<'static, Msg> = if f.line == 0 {
        ui::faint("Whole document").into()
    } else if jump {
        ui::link(&format!("Line {}", f.line), Msg::Jump(f.line), false)
    } else {
        ui::faint(format!("Line {}", f.line)).into()
    };
    let content = row![
        ui::sans(name, 13.0, tone.color()),
        place,
        container(ui::prose(f.message.clone())).width(Length::Fill),
        ui::mono(f.rule.clone(), 12.0, palette().ink_3),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    boxed(content.into())
}

/// A row in a ruled box on the sheet, the web's `.pe-finding`: its padding, a rule and the skin's radius.
pub(super) fn boxed<'a, M: 'a>(content: Element<'a, M>) -> Element<'a, M> {
    container(content)
        .padding([8, 12])
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(palette().sheet.into()),
            border: Border {
                color: palette().rule,
                width: 1.0,
                radius: crate::theme::skin().radius.into(),
            },
            ..Default::default()
        })
        .into()
}

/// The Details form: the create form's fields for the prompt's metadata (its kind does not change).
fn details_form(kind: Kind, d: &Details) -> Element<'_, Msg> {
    let binding: Element<'_, Msg> = match kind {
        Kind::System => text_input("e.g. planner", &d.role)
            .on_input(|v| Msg::DetailText(Field::Role, v))
            .padding(8)
            .style(crate::theme::input)
            .into(),
        Kind::AgentsMd => checkbox(d.every)
            .label("Apply to every agent")
            .on_toggle(Msg::DetailEvery)
            .into(),
        Kind::Fragment => ui::faint("Fragments are not bound to a role or to the plaza.").into(),
    };
    let body = column![
        prompts::field(
            "Title",
            text_input("What the prompt is for", &d.title)
                .on_input(|v| Msg::DetailText(Field::Title, v))
                .padding(8)
                .style(crate::theme::input)
                .into(),
        ),
        prompts::field(
            "Description",
            text_input("A line on what it is for", &d.description)
                .on_input(|v| Msg::DetailText(Field::Description, v))
                .padding(8)
                .style(crate::theme::input)
                .into(),
        ),
        prompts::field(
            "Tags",
            text_input("comma, separated", &d.tags)
                .on_input(|v| Msg::DetailText(Field::Tags, v))
                .padding(8)
                .style(crate::theme::input)
                .into(),
        ),
        prompts::field("Binding", binding),
        row![
            ui::btn(
                "Save details",
                None,
                ui::Kind::Primary,
                Some(Msg::SaveDetails)
            ),
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CloseDetails)),
        ]
        .spacing(8),
    ]
    .spacing(12);
    ui::card("Details", body.into())
}

/// The publish form: the version to publish (the latest at first), as the web's Publish dialog.
fn publish_form(l: &Loaded) -> Option<Element<'_, Msg>> {
    let chosen = l.publish?;
    let labels: Vec<(u32, String)> = l
        .versions
        .iter()
        .map(|v| {
            let live = if l.summary.active_version == Some(v.version) {
                " (live)"
            } else {
                ""
            };
            (
                v.version,
                format!("v{}, score {}{live}", v.version, v.score),
            )
        })
        .collect();
    let options: Vec<(u32, &str)> = labels.iter().map(|(n, s)| (*n, s.as_str())).collect();
    let body = column![
        ui::faint("Publishing an older version rolls the prompt back. Versions with errors cannot be published."),
        ui::seg(&options, chosen, Msg::PublishPick),
        row![
            ui::btn("Publish", None, ui::Kind::Primary, Some(Msg::Publish(chosen))),
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::ClosePublish)),
        ]
        .spacing(8),
    ]
    .spacing(12);
    Some(ui::card("Publish", body.into()))
}

/// The Edit tab: the source beside the diff against the live version, with the note and the save; the checks and the
/// preview under them.
fn edit_tab(l: &Loaded) -> Element<'_, Msg> {
    let body = source_text(&l.source);
    let source = text_editor(&l.source)
        .style(crate::theme::editor)
        .id(source_id())
        .on_action(Msg::Edit)
        .font(Font::MONOSPACE)
        .height(Length::Fixed(360.0));
    let save_label = format!("Save as version {}", l.summary.latest_version + 1);
    let save_row = row![
        container(prompts::field(
            "Note",
            text_input("What changed, in a few words", &l.note)
                .on_input(Msg::Note)
                .padding(8)
                .style(crate::theme::input)
                .into(),
        ))
        .width(Length::Fixed(260.0)),
        ui::btn(&save_label, None, ui::Kind::Primary, Some(Msg::Save)),
    ]
    .spacing(8)
    .align_y(Alignment::End);
    let panes = row![
        container(plain_card(column![source, save_row].spacing(12).into(),))
            .width(Length::FillPortion(1)),
        container(ui::card(
            "Against the live version",
            diff_box(line_diff(&l.live, &body), None),
        ))
        .width(Length::FillPortion(1)),
    ]
    .spacing(16)
    .align_y(Alignment::Start);

    let r = &l.report;
    let summary = row![
        ui::badge(&format!("Score {}", r.score), score_tone(r.score)),
        ui::badge(
            &format!("{} errors", r.errors),
            if r.errors > 0 {
                ui::Tone::Danger
            } else {
                ui::Tone::Muted
            }
        ),
        ui::badge(
            &format!("{} warnings", r.warnings),
            if r.warnings > 0 {
                ui::Tone::Warn
            } else {
                ui::Tone::Muted
            }
        ),
        ui::badge(
            &format!("{} infos", r.infos),
            if r.infos > 0 {
                ui::Tone::Info
            } else {
                ui::Tone::Muted
            }
        ),
        ui::faint(format!(
            "{} words, about {} tokens, {} lines, {} headings",
            r.stats.words, r.stats.est_tokens, r.stats.lines, r.stats.headings
        )),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .wrap();
    let mut findings: Vec<Element<'_, Msg>> = Vec::new();
    if r.findings.is_empty() {
        findings.push(ui::faint("No findings. This prompt passes every check.").into());
    }
    findings.extend(r.findings.iter().map(|f| finding_row(f, true)));
    let checks = ui::card(
        "Checks",
        column![summary, column(findings).spacing(6)]
            .spacing(12)
            .into(),
    );
    let preview = ui::card(
        "Preview",
        container(ui::scroll(ui::markdown::<Msg>(&body)).height(Length::Fixed(320.0)))
            .padding(12)
            .width(Length::Fill)
            .style(well_style)
            .into(),
    );
    column![panes, column![checks, preview].spacing(16)]
        .spacing(16)
        .into()
}

/// The Versions tab: the viewed version, the table of versions with their actions, the pick bar and the comparison.
fn versions_tab(l: &Loaded) -> Element<'_, Msg> {
    let mut parts: Vec<Element<'_, Msg>> = Vec::new();
    if let Some(v) = &l.viewing {
        parts.push(viewing(v));
    }
    if l.versions.is_empty() {
        parts.push(ui::faint("No versions yet.").into());
    } else {
        let rows: Vec<Vec<Element<'_, Msg>>> =
            l.versions.iter().map(|v| version_cells(l, v)).collect();
        parts.push(table(
            &[
                "Version",
                "Note",
                "Author",
                "Created",
                "Score",
                "E / W / I",
                "",
            ],
            &[2, 3, 2, 2, 1, 1, 0],
            rows,
        ));
    }
    if !l.picked.is_empty() {
        parts.push(pick_bar(l));
    }
    if let Some(d) = &l.compare {
        parts.push(compare(d));
    }
    column(parts).spacing(16).into()
}

/// One version's cells: its number with the live, best and stale marks; its note, author and age; its score and findings
/// counts; and its actions (view, compare or unpick, publish or roll back, restore as new).
fn version_cells<'a>(l: &'a Loaded, v: &'a PromptVersion) -> Vec<Element<'a, Msg>> {
    let n = v.version;
    let live = l.summary.active_version;
    let mut marks: Vec<Element<'a, Msg>> = vec![
        text(format!("v{n}"))
            .font(ui::SANS_SEMIBOLD)
            .size(13.0)
            .color(palette().ink)
            .into(),
    ];
    if live == Some(n) {
        marks.push(ui::badge("Live", ui::Tone::Ok));
    }
    if l.summary.best_version == Some(n) {
        marks.push(ui::badge("Best", ui::Tone::Signal));
    }
    if v.rules_version < apz_prompt::RULES_VERSION {
        marks.push(ui::badge("stale", ui::Tone::Warn));
    }
    let publish_label = if live == Some(n) {
        "Live"
    } else if live.is_some_and(|x| n < x) {
        "Roll back"
    } else {
        "Publish"
    };
    let picked = l.picked.contains(&n);
    let note = if v.note.is_empty() {
        "—".to_string()
    } else {
        v.note.clone()
    };
    vec![
        row(marks).spacing(6).align_y(Alignment::Center).into(),
        ui::sans(note, 13.0, palette().ink_2).into(),
        ui::sans(v.author.clone(), 13.0, palette().ink_2).into(),
        ui::faint(mcp_hub::ago(now_ms(), v.created_ms)).into(),
        ui::badge(&v.score.to_string(), score_tone(v.score)),
        ui::mono(
            format!("{} / {} / {}", v.errors, v.warnings, v.infos),
            12.0,
            palette().ink_2,
        )
        .into(),
        row![
            ui::btn_sm("View", None, ui::Kind::Ghost, Some(Msg::View(n))),
            ui::btn_sm(
                if picked { "Unpick" } else { "Compare" },
                None,
                ui::Kind::Ghost,
                Some(Msg::Pick(n))
            ),
            ui::btn_sm(
                publish_label,
                None,
                ui::Kind::Ghost,
                (live != Some(n)).then_some(Msg::Publish(n))
            ),
            ui::btn_sm(
                "Restore as new",
                None,
                ui::Kind::Ghost,
                Some(Msg::RestoreVersion(n))
            ),
        ]
        .spacing(4)
        .align_y(Alignment::Center)
        .into(),
    ]
}

/// The pick bar: the version picked so far, and Clear.
fn pick_bar(l: &Loaded) -> Element<'_, Msg> {
    let note = if l.picked.len() == 1 {
        format!("v{} picked. Pick one more version to compare.", l.picked[0])
    } else {
        String::new()
    };
    row![
        ui::faint(note),
        ui::btn_sm("Clear", None, ui::Kind::Ghost, Some(Msg::ClearPicks)),
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into()
}

/// A version read only, in a card: its body in a scrolling well, with Close.
fn viewing(v: &PromptVersion) -> Element<'_, Msg> {
    ui::card_with(
        &format!("v{}, read only", v.version),
        vec![ui::btn_sm(
            "Close",
            None,
            ui::Kind::Ghost,
            Some(Msg::CloseView),
        )],
        container(
            ui::scroll(ui::mono(v.body.clone(), 12.5, palette().ink_2))
                .height(Length::Fixed(320.0)),
        )
        .padding(12)
        .width(Length::Fill)
        .style(well_style)
        .into(),
    )
}

/// The comparison of two picked versions, in a card: the line diff, with Close.
fn compare(d: &PromptDiff) -> Element<'_, Msg> {
    let rows: Vec<(DiffOp, String)> = d
        .lines
        .iter()
        .map(|l| (diff_op(&l.op), l.text.clone()))
        .collect();
    ui::card_with(
        &format!("Compare v{} and v{}", d.from.version, d.to.version),
        vec![ui::btn_sm(
            "Close",
            None,
            ui::Kind::Ghost,
            Some(Msg::ClearPicks),
        )],
        diff_box(rows, Some(360.0)),
    )
}

/// The History tab: each event's action, version, actor and age.
fn history_tab(l: &Loaded) -> Element<'_, Msg> {
    let Some(events) = l.events.as_ref() else {
        return ui::loading();
    };
    if events.is_empty() {
        return ui::faint("No history yet.").into();
    }
    let now = now_ms();
    let rows: Vec<Vec<Element<'_, Msg>>> = events
        .iter()
        .map(|e| {
            vec![
                ui::badge(&e.action, ui::Tone::Muted),
                ui::sans(
                    e.version
                        .map_or_else(|| "—".to_string(), |v| format!("v{v}")),
                    13.0,
                    palette().ink_2,
                )
                .into(),
                ui::sans(e.actor.clone(), 13.0, palette().ink_2).into(),
                ui::faint(mcp_hub::ago(now, e.at_ms)).into(),
            ]
        })
        .collect();
    table(&["Action", "Version", "Actor", "When"], &[2, 1, 2, 2], rows)
}

/// The Rules tab: the checks that apply to this kind, grouped by their severity, each with the why under it.
fn rules_tab(kind: Kind) -> Element<'static, Msg> {
    let rules = apz_prompt::rules();
    let mut parts: Vec<Element<'static, Msg>> = vec![
        ui::faint(format!(
            "The checks that apply to {}. Errors block publishing and cost 25 points each; warnings cost 8 and infos 2.",
            kind_name(kind)
        ))
        .into(),
    ];
    let mut items: Vec<Element<'static, Msg>> = Vec::new();
    for severity in [Severity::Error, Severity::Warning, Severity::Info] {
        for rule in rules
            .iter()
            .filter(|r| r.severity == severity && r.applies_to(kind))
        {
            items.push(
                row![
                    ui::badge(severity.as_str(), severity_tone(severity)),
                    column![
                        ui::sans(rule.title.to_string(), 13.0, palette().ink),
                        ui::faint(rule.why.to_string()),
                    ]
                    .spacing(2),
                ]
                .spacing(12)
                .align_y(Alignment::Start)
                .into(),
            );
        }
    }
    parts.push(column(items).spacing(14).into());
    column(parts).spacing(14).into()
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let Some(l) = state.loaded.as_ref() else {
        let status: Element<'a, Msg> = match &state.error {
            Some(e) => ui::error_line(e),
            None => ui::loading(),
        };
        return ui::sheet(
            ui::view_head(
                "Prompt",
                vec![ui::btn_sm(
                    "Back to prompts",
                    None,
                    ui::Kind::Ghost,
                    Some(Msg::Back),
                )],
            ),
            status,
        );
    };
    let mut body: Vec<Element<'a, Msg>> = Vec::new();
    if let Some((text, is_error)) = &state.notice {
        body.push(if *is_error {
            ui::error_line(text)
        } else {
            ui::faint(text.clone()).into()
        });
    }
    if state.conflict {
        body.push(ui::btn_sm(
            "Reload latest",
            None,
            ui::Kind::Secondary,
            Some(Msg::ReloadLatest),
        ));
    }
    if state.leaving {
        body.push(
            row![
                ui::faint("Discard your changes?"),
                ui::btn_sm("Discard", None, ui::Kind::Secondary, Some(Msg::Discard)),
                ui::btn_sm(
                    "Keep editing",
                    None,
                    ui::Kind::Ghost,
                    Some(Msg::KeepEditing)
                ),
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    if let Some(form) = publish_form(l) {
        body.push(form);
    }
    if let Some(d) = l.details.as_ref() {
        body.push(details_form(l.kind, d));
    }
    body.push(tab_strip(state.tab, l.versions.len()));
    body.push(match state.tab {
        Tab::Edit => edit_tab(l),
        Tab::Versions => versions_tab(l),
        Tab::History => history_tab(l),
        Tab::Rules => rules_tab(l.kind),
    });
    ui::sheet(header(state, l), column(body).spacing(16).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_finding_moves_the_cursor_to_its_line() {
        // A finding on a line selects that line.
        {
            let text = "# Title\nfirst line\nsecond";
            let c = finding_cursor(text, 2).expect("line 2 has a cursor");
            assert_eq!(
                c.position,
                Position {
                    line: 1,
                    column: "first line".len()
                }
            );
            assert_eq!(c.selection, Some(Position { line: 1, column: 0 }));
        }
        // A finding past the end selects the last line.
        {
            let c = finding_cursor("a\nbb", 9).expect("clamped to the last line");
            assert_eq!(c.position, Position { line: 1, column: 2 });
        }
        // A document finding has no line to jump to.
        {
            assert_eq!(finding_cursor("anything", 0), None);
        }
    }

    #[test]
    fn the_binding_follows_the_kind() {
        assert_eq!(
            binding_for(Kind::System, " builder ", false),
            Some("role:builder".to_string())
        );
        assert_eq!(binding_for(Kind::System, "  ", true), None);
        assert_eq!(
            binding_for(Kind::AgentsMd, "", true),
            Some("plaza".to_string())
        );
        assert_eq!(binding_for(Kind::AgentsMd, "builder", false), None);
        assert_eq!(binding_for(Kind::Fragment, "builder", true), None);
    }

    #[test]
    fn the_meta_body_is_built_from_the_form_fields() {
        // The form fields are carried trimmed.
        {
            let d = Details {
                title: "  Builder  ".to_string(),
                description: " Builds things ".to_string(),
                tags: "review, safety".to_string(),
                role: "builder".to_string(),
                every: false,
            };
            assert_eq!(
                meta_body(Kind::System, &d).unwrap(),
                json!({ "title": "Builder", "description": "Builds things", "tags": ["review", "safety"], "binding": "role:builder" })
            );
        }
        // A fragment sends no binding.
        {
            let d = Details {
                title: "Note".to_string(),
                role: "builder".to_string(),
                every: true,
                ..Details::default()
            };
            let body = meta_body(Kind::Fragment, &d).unwrap();
            assert_eq!(body["binding"], Value::Null);
        }
    }
}
