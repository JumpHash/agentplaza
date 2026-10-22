//! The skill editor (docs/design/skill-library.md, "Screens"): the file tree, SKILL.md and the text files in editors with a
//! live markdown preview, findings checked locally with `apz_skill` as you type (each one jumps to its file and line),
//! Save version, Publish, Unpublish and Archive, Export, Details (title and tags), Bindings, the GitHub source with Check
//! for updates and Update, and the Versions (view, compare, publish, restore as new), History and Rules tabs. Every file
//! can be saved as a file on this computer. `skills` owns it: `open` starts it for one skill and `Msg::Back` returns to
//! the list. Hub answers go through `apz_http` with the status kept, so a 409 (a newer version was saved) is told apart
//! from other failures.

use std::cmp::Reverse;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use apz_skill::{FileInfo, Finding, Report, Severity};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use iced::widget::{
    Space, button, checkbox, column, container, row, text, text_editor, text_input,
};
use iced::{Alignment, Border, Element, Font, Length, Task, Theme};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::console::{Ctx, client, ui};
use crate::theme::palette;

use super::mcp_hub;
use super::prompt_editor;
use super::skills::{Role, field};

const SKILL_MD: &str = "SKILL.md";
/// The binding that means every agent on the plaza.
const EVERY_AGENT: &str = "plaza";
/// apz_skill reads at most this much text per file.
const TEXT_LIMIT: usize = 1 << 20;
/// The save's error text when the hub answers 409: a newer version was saved first.
const CONFLICT: &str = "a newer version was saved; reload";
/// The hub's limits on a skill's title and tags (apid's skills module).
const MAX_TITLE: usize = 120;
const MAX_TAGS: usize = 16;
const MAX_TAG: usize = 32;

// ---- The hub's JSON (apps/web/src/api.ts, "The skill library") ----------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Source {
    Built,
    Upload,
    Github { repo: String, commit: String },
}

/// One skill as the list and the header show it.
#[derive(Debug, Clone, Deserialize)]
pub struct Summary {
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    bindings: Vec<String>,
    #[serde(default)]
    active_version: Option<u32>,
    latest_version: u32,
    #[serde(default)]
    best_version: Option<u32>,
    #[serde(default)]
    score: Option<u8>,
    source: Source,
    #[serde(default)]
    archived_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileMeta {
    path: String,
    hash: String,
    size: u64,
    executable: bool,
}

/// One version as the versions list shows it.
#[derive(Debug, Clone, Deserialize)]
pub struct VersionMeta {
    version: u32,
    #[serde(default)]
    note: String,
    #[serde(default)]
    author: String,
    created_ms: u64,
    score: u8,
    errors: u32,
    warnings: u32,
    infos: u32,
    rules_version: u32,
}

/// A whole version: the SKILL.md, the files and the findings.
#[derive(Debug, Clone, Deserialize)]
pub struct Version {
    version: u32,
    score: u8,
    #[serde(default)]
    skill_md: String,
    #[serde(default)]
    files: Vec<FileMeta>,
    #[serde(default)]
    stale: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Detail {
    skill: Summary,
    versions: Vec<VersionMeta>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DiffLine {
    op: String,
    old: Option<u32>,
    new: Option<u32>,
    text: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FilesDiff {
    added: Vec<String>,
    removed: Vec<String>,
    changed: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Diff {
    from: VersionMeta,
    to: VersionMeta,
    skill_md: Vec<DiffLine>,
    files: FilesDiff,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GithubCheck {
    current: String,
    latest: String,
    update_available: bool,
}

/// A save, a GitHub update: the version and whether it was new.
#[derive(Debug, Clone, Deserialize)]
pub struct Added {
    version: VersionMeta,
    created: bool,
}

/// One entry of the skill's history (the hub's `GET /v1/skills/{id}/history`).
#[derive(Debug, Clone, Deserialize)]
pub struct SkillEvent {
    action: String,
    version: Option<u32>,
    actor: String,
    at_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct Events {
    events: Vec<SkillEvent>,
}

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
    /// Back to the list (handled by `skills`, which asks first when the draft has unsaved changes).
    Back,
    /// Leave without saving (handled by `skills`), after the confirmation.
    Discard,
    /// Keep editing after the confirmation.
    KeepEditing,
    Tab(Tab),
    Loaded(Result<(Detail, Version, Vec<Vec<u8>>), String>),
    Refreshed(Result<Detail, String>),
    Select(String),
    /// Select a file and show one of its lines (a finding's place).
    Jump(String, u32),
    EditSkill(text_editor::Action),
    EditFile(text_editor::Action),
    Note(String),
    NewPath(String),
    AddFile,
    PickFromDisk,
    Picked(Option<PathBuf>),
    RenameTo(String),
    Rename,
    Remove,
    ToggleExec,
    /// Save the selected file as a file on this computer: the save dialog, then the bytes.
    SaveAs,
    /// A saved text file's bytes, fetched for its save: its name and bytes, or the error that stopped it.
    SaveFetched(Result<(String, Vec<u8>), String>),
    SaveAsPicked(Option<(PathBuf, Vec<u8>)>),
    /// Open the Details form (title and tags) with the skill's current values, or close it without saving.
    OpenDetails,
    CloseDetails,
    DetailTitle(String),
    DetailTags(String),
    /// Save the Details form: a PUT of the title and tags, then the skill reloads.
    SaveDetails,
    DetailsSaved(Result<(), String>),
    /// A text outcome from an export or a GitHub check (its notice).
    Notice(Result<String, String>),
    Save,
    Saved(Result<Added, String>),
    /// Reload the latest version after a 409, which discards the draft.
    ReloadLatest,
    /// A binding box ticked (true) or cleared (false): `plaza` for every agent, `role:<id>` for a role.
    ToggleBinding(String, bool),
    /// The roles list for the bindings picker, or the error that stopped it.
    Roles(Result<Vec<Role>, String>),
    SaveBindings,
    /// A publish, unpublish, archive, restore or bindings save went through (its notice), or failed.
    Acted(Result<String, String>),
    /// Open the publish form (the version chosen starts as the latest), or close it.
    OpenPublish,
    ClosePublish,
    PublishPick(u32),
    Publish(u32),
    Unpublish,
    ToggleArchive,
    Export,
    ExportPicked(Option<PathBuf>),
    View(u32),
    Viewed(Result<Version, String>),
    CloseView,
    /// Pick a version to compare; the second pick compares the two.
    Pick(u32),
    ClearPicks,
    Compared(Result<Diff, String>),
    RestoreVersion(u32),
    Restored(Result<(Version, Vec<Vec<u8>>), String>),
    CheckGithub,
    GithubChecked(Result<GithubCheck, String>),
    UpdateGithub,
    GithubUpdated(Result<Added, String>),
    HistoryLoaded(Result<Vec<SkillEvent>, String>),
}

/// The body of one file in the draft: an editor for text, the bytes for anything else.
#[derive(Debug)]
enum Body {
    Text {
        content: text_editor::Content,
        saved: String,
    },
    Binary(Vec<u8>),
}

/// One file of the draft. `saved` is the saved file's (hash, size); None for a file this draft adds or restores.
#[derive(Debug)]
pub struct Entry {
    path: String,
    executable: bool,
    saved: Option<(String, u64)>,
    body: Body,
}

impl Entry {
    /// The text, for a text file.
    fn text(&self) -> Option<String> {
        match &self.body {
            Body::Text { content, .. } => Some(source_text(content)),
            Body::Binary(_) => None,
        }
    }

    fn bytes(&self) -> Vec<u8> {
        match &self.body {
            Body::Text { content, .. } => source_text(content).into_bytes(),
            Body::Binary(bytes) => bytes.clone(),
        }
    }

    fn size(&self) -> u64 {
        self.bytes().len() as u64
    }

    /// The text differs from the text last loaded or saved.
    fn text_changed(&self) -> bool {
        match &self.body {
            Body::Text { content, saved } => source_text(content) != *saved,
            Body::Binary(_) => false,
        }
    }

    fn changed(&self) -> bool {
        self.saved.is_none() || self.text_changed()
    }
}

/// The Details form's draft: the title, and the tags typed comma separated.
#[derive(Debug, Clone, Default)]
pub struct SkillDetails {
    title: String,
    tags: String,
}

impl SkillDetails {
    fn of(summary: &Summary) -> Self {
        SkillDetails {
            title: summary.title.clone(),
            tags: summary.tags.join(", "),
        }
    }
}

/// The skill as loaded: the draft, the checks on it, and the versions' tabs' state.
#[derive(Debug)]
pub struct Loaded {
    summary: Summary,
    /// Newest first.
    versions: Vec<VersionMeta>,
    skill_md: text_editor::Content,
    /// SKILL.md as loaded or saved, normalised as the draft is.
    skill_saved: String,
    entries: Vec<Entry>,
    /// The file shown: SKILL.md or a path in `entries`.
    selected: String,
    /// A file was added, removed, renamed or made executable since the last load or save.
    structure_changed: bool,
    report: Report,
    note: String,
    new_path: String,
    rename_to: String,
    /// The draft's bindings: `plaza` for every agent, `role:<id>` for a role.
    bindings: Vec<String>,
    /// A finding's line, shown under the editor.
    jump: Option<u32>,
    github: Option<GithubCheck>,
    viewing: Option<Version>,
    /// The versions picked to compare (at most two).
    picked: Vec<u32>,
    compare: Option<Diff>,
    /// None until the History tab was opened.
    events: Option<Vec<SkillEvent>>,
    /// The Details form's draft while the form is open.
    details: Option<SkillDetails>,
    /// The version the publish form has chosen, while the form is open.
    publish: Option<u32>,
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
    /// The roles for the bindings picker, from GET /v1/roles.
    roles: Vec<Role>,
    /// The hub changed this skill while the draft had unsaved edits: offer a reload instead of replacing the draft.
    changed: bool,
}

/// The editor for skill `id`, and the task that loads it.
pub fn open(id: String, ctx: &Ctx) -> (State, Task<Msg>) {
    let task = Task::batch([load(&id, ctx), roles_task(ctx)]);
    (
        State {
            id,
            tab: Tab::Edit,
            loaded: None,
            error: None,
            notice: None,
            conflict: false,
            leaving: false,
            roles: Vec::new(),
            changed: false,
        },
        task,
    )
}

/// The hub changed this skill: reload it when the draft has no unsaved edits, otherwise say so and keep the draft. An
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

/// The roles list for the bindings picker.
fn roles_task(ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    client::task(move || get::<Vec<Role>>(&c, "/v1/roles"), Msg::Roles)
}

/// Whether the draft differs from the version last loaded or saved.
pub fn is_dirty(state: &State) -> bool {
    state.loaded.as_ref().is_some_and(Loaded::dirty)
}

/// Ask before leaving: the view shows Discard and Keep editing until one is chosen.
pub fn ask_leave(state: &mut State) {
    state.leaving = true;
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
            match state.loaded.as_ref() {
                Some(l) if tab == Tab::History && l.events.is_none() => history(&state.id, ctx),
                _ => Task::none(),
            }
        }
        Msg::Loaded(Ok((detail, version, files))) => {
            state.changed = false;
            let keep = state.loaded.as_ref().map(|l| l.selected.clone());
            let mut loaded = build(detail, version, files);
            if let Some(sel) = keep.filter(|s| loaded.has_path(s)) {
                loaded.selected = sel;
            }
            state.loaded = Some(loaded);
            state.error = None;
            if state.tab == Tab::History {
                history(&state.id, ctx)
            } else {
                Task::none()
            }
        }
        Msg::Loaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::Refreshed(Ok(detail)) => {
            if let Some(l) = state.loaded.as_mut() {
                l.summary = detail.skill;
                l.versions = sorted(detail.versions);
            }
            Task::none()
        }
        Msg::Refreshed(Err(e)) => {
            state.notice = Some((e, true));
            Task::none()
        }
        Msg::Select(path) => {
            if let Some(l) = state.loaded.as_mut() {
                l.selected = path;
                l.jump = None;
            }
            Task::none()
        }
        Msg::Jump(path, line) => {
            if let Some(l) = state.loaded.as_mut()
                && l.has_path(&path)
            {
                l.selected = path;
                l.jump = (line > 0).then_some(line);
            }
            Task::none()
        }
        Msg::EditSkill(action) => {
            if let Some(l) = state.loaded.as_mut() {
                l.skill_md.perform(action);
                l.recheck();
            }
            Task::none()
        }
        Msg::EditFile(action) => {
            if let Some(l) = state.loaded.as_mut() {
                let edited = match l.selected_entry_mut() {
                    Some(Entry {
                        body: Body::Text { content, .. },
                        ..
                    }) => {
                        content.perform(action);
                        true
                    }
                    _ => false,
                };
                if edited {
                    l.recheck();
                }
            }
            Task::none()
        }
        Msg::Note(note) => {
            if let Some(l) = state.loaded.as_mut() {
                l.note = note;
            }
            Task::none()
        }
        Msg::NewPath(path) => {
            if let Some(l) = state.loaded.as_mut() {
                l.new_path = path;
            }
            Task::none()
        }
        Msg::AddFile => {
            if let Some(l) = state.loaded.as_mut() {
                let path = l.new_path.trim().to_string();
                let body = Body::Text {
                    content: text_editor::Content::new(),
                    saved: String::new(),
                };
                match add(l, path, body) {
                    Ok(()) => l.new_path.clear(),
                    Err(e) => state.notice = Some((e, true)),
                }
            }
            Task::none()
        }
        Msg::PickFromDisk => Task::perform(
            async {
                rfd::AsyncFileDialog::new()
                    .set_title("Add a file to the skill")
                    .pick_file()
                    .await
                    .map(|file| file.path().to_path_buf())
            },
            Msg::Picked,
        ),
        Msg::Picked(None) => Task::none(),
        Msg::Picked(Some(path)) => {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string);
            match (name, std::fs::read(&path)) {
                (Some(name), Ok(bytes)) => {
                    if let Some(l) = state.loaded.as_mut()
                        && let Err(e) = add(l, name, body_of(bytes))
                    {
                        state.notice = Some((e, true));
                    }
                }
                (_, Err(e)) => {
                    state.notice = Some((format!("The file could not be read: {e}"), true))
                }
                (None, _) => {
                    state.notice = Some(("The file name is not valid UTF-8.".to_string(), true))
                }
            }
            Task::none()
        }
        Msg::RenameTo(to) => {
            if let Some(l) = state.loaded.as_mut() {
                l.rename_to = to;
            }
            Task::none()
        }
        Msg::Rename => {
            if let Some(l) = state.loaded.as_mut()
                && let Err(e) = rename(l)
            {
                state.notice = Some((e, true));
            }
            Task::none()
        }
        Msg::Remove => {
            if let Some(l) = state.loaded.as_mut() {
                let selected = l.selected.clone();
                if selected == SKILL_MD {
                    state.notice = Some(("SKILL.md cannot be removed.".to_string(), true));
                } else {
                    l.entries.retain(|e| e.path != selected);
                    l.selected = SKILL_MD.to_string();
                    l.structure_changed = true;
                    l.recheck();
                }
            }
            Task::none()
        }
        Msg::ToggleExec => {
            if let Some(l) = state.loaded.as_mut() {
                let toggled = l
                    .selected_entry_mut()
                    .map(|e| e.executable = !e.executable)
                    .is_some();
                if toggled {
                    l.structure_changed = true;
                }
            }
            Task::none()
        }
        Msg::SaveAs => {
            let Some(l) = state.loaded.as_ref() else {
                return Task::none();
            };
            if l.selected == SKILL_MD {
                return save_dialog(file_name(SKILL_MD), source_text(&l.skill_md).into_bytes());
            }
            match l.selected_entry() {
                // A saved text file is written as its saved version, from the hub.
                Some(e) if e.saved.is_some() && matches!(e.body, Body::Text { .. }) => {
                    let (c, id, version, path) = (
                        ctx.client.clone(),
                        state.id.clone(),
                        l.summary.latest_version,
                        e.path.clone(),
                    );
                    let name = file_name(&path);
                    client::task(
                        move || {
                            raw(&c, "GET", &file_path(&id, version, &path), None)
                                .map(|bytes| (name, bytes))
                        },
                        Msg::SaveFetched,
                    )
                }
                Some(e) => save_dialog(file_name(&e.path), e.bytes()),
                None => Task::none(),
            }
        }
        Msg::SaveFetched(Ok((name, bytes))) => save_dialog(name, bytes),
        Msg::SaveFetched(Err(e)) => {
            state.notice = Some((e, true));
            Task::none()
        }
        Msg::SaveAsPicked(None) => Task::none(),
        Msg::SaveAsPicked(Some((path, bytes))) => {
            state.notice = Some(match std::fs::write(&path, &bytes) {
                Ok(()) => (format!("Saved to {}", path.display()), false),
                Err(e) => (format!("The file could not be written: {e}"), true),
            });
            Task::none()
        }
        Msg::Notice(Ok(text)) => {
            state.notice = Some((text, false));
            Task::none()
        }
        Msg::Notice(Err(e)) => {
            state.notice = Some((e, true));
            Task::none()
        }
        Msg::Save => match state.loaded.as_ref() {
            Some(l) => save(&state.id, l, ctx),
            None => Task::none(),
        },
        Msg::Saved(Ok(added)) => {
            state.conflict = false;
            state.changed = false;
            state.notice = Some(if added.created {
                (format!("Saved as v{}", added.version.version), false)
            } else {
                (
                    "No changes: the files are the same as the latest version.".to_string(),
                    false,
                )
            });
            load(&state.id, ctx)
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
        Msg::ToggleBinding(binding, on) => {
            if let Some(l) = state.loaded.as_mut() {
                l.bindings.retain(|b| *b != binding);
                if on {
                    l.bindings.push(binding);
                }
            }
            Task::none()
        }
        Msg::Roles(Ok(roles)) => {
            state.roles = roles;
            Task::none()
        }
        Msg::Roles(Err(e)) => {
            state.notice = Some((e, true));
            Task::none()
        }
        Msg::SaveBindings => match state.loaded.as_ref() {
            Some(l) => {
                let (c, id, bindings) = (ctx.client.clone(), state.id.clone(), l.bindings.clone());
                client::task(
                    move || {
                        send(
                            &c,
                            "PUT",
                            &format!("{}/bindings", skill_path(&id)),
                            Some(&json!({ "bindings": bindings })),
                        )
                        .and_then(expect::<Value>)
                        .map(|_| "Bindings saved".to_string())
                    },
                    Msg::Acted,
                )
            }
            None => Task::none(),
        },
        Msg::Acted(Ok(text)) => {
            state.notice = Some((text, false));
            state.changed = false;
            if let Some(l) = state.loaded.as_mut() {
                l.events = None;
            }
            let events = if state.tab == Tab::History {
                history(&state.id, ctx)
            } else {
                Task::none()
            };
            Task::batch([refresh(&state.id, ctx), events])
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
        Msg::Export => match state.loaded.as_ref() {
            Some(l) => {
                let name = format!("{}-v{}.zip", state.id, l.summary.latest_version);
                Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .set_title("Export the skill as a zip")
                            .set_file_name(name)
                            .add_filter("Zip", &["zip"])
                            .save_file()
                            .await
                            .map(|file| file.path().to_path_buf())
                    },
                    Msg::ExportPicked,
                )
            }
            None => Task::none(),
        },
        Msg::ExportPicked(None) => Task::none(),
        Msg::ExportPicked(Some(path)) => match state.loaded.as_ref() {
            Some(l) => {
                let (c, id, version) = (
                    ctx.client.clone(),
                    state.id.clone(),
                    l.summary.latest_version,
                );
                client::task(
                    move || {
                        let bytes = raw(
                            &c,
                            "GET",
                            &format!("{}/versions/{version}/export", skill_path(&id)),
                            None,
                        )?;
                        std::fs::write(&path, &bytes)
                            .map_err(|e| format!("The file could not be written: {e}"))?;
                        Ok(format!("Exported v{version} to {}", path.display()))
                    },
                    Msg::Notice,
                )
            }
            None => Task::none(),
        },
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
        Msg::RestoreVersion(version) => {
            let (c, id) = (ctx.client.clone(), state.id.clone());
            client::task(move || fetch_version(&c, &id, version), Msg::Restored)
        }
        Msg::Restored(Ok((version, files))) => {
            if let Some(l) = state.loaded.as_mut() {
                l.entries = version
                    .files
                    .iter()
                    .zip(files)
                    .map(|(f, bytes)| Entry {
                        path: f.path.clone(),
                        executable: f.executable,
                        saved: None,
                        body: body_of(bytes),
                    })
                    .collect();
                l.skill_md = text_editor::Content::with_text(&version.skill_md);
                l.selected = SKILL_MD.to_string();
                l.jump = None;
                l.structure_changed = true;
                l.note = format!("Restored from v{}", version.version);
                l.recheck();
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
        Msg::CheckGithub => {
            let (c, id) = (ctx.client.clone(), state.id.clone());
            client::task(
                move || post::<GithubCheck>(&c, &format!("{}/github/check", skill_path(&id)), None),
                Msg::GithubChecked,
            )
        }
        Msg::GithubChecked(Ok(check)) => {
            let text = if check.update_available {
                format!(
                    "Update available: {} (this is {})",
                    check.latest, check.current
                )
            } else {
                format!("Up to date at {}", check.current)
            };
            if let Some(l) = state.loaded.as_mut() {
                l.github = Some(check);
            }
            state.notice = Some((text, false));
            Task::none()
        }
        Msg::GithubChecked(Err(e)) => {
            state.notice = Some((e, true));
            Task::none()
        }
        Msg::UpdateGithub => {
            let (c, id) = (ctx.client.clone(), state.id.clone());
            client::task(
                move || post::<Added>(&c, &format!("{}/github/update", skill_path(&id)), None),
                Msg::GithubUpdated,
            )
        }
        Msg::GithubUpdated(Ok(added)) => {
            state.notice = Some((
                if added.created {
                    format!("Updated to v{} from GitHub", added.version.version)
                } else {
                    "No changes: GitHub has the same files as the latest version.".to_string()
                },
                false,
            ));
            load(&state.id, ctx)
        }
        Msg::GithubUpdated(Err(e)) => {
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
        Msg::OpenDetails => {
            if let Some(l) = state.loaded.as_mut() {
                l.details = Some(SkillDetails::of(&l.summary));
            }
            Task::none()
        }
        Msg::CloseDetails => {
            if let Some(l) = state.loaded.as_mut() {
                l.details = None;
            }
            Task::none()
        }
        Msg::DetailTitle(title) => {
            if let Some(d) = state.loaded.as_mut().and_then(|l| l.details.as_mut()) {
                d.title = title;
            }
            Task::none()
        }
        Msg::DetailTags(tags) => {
            if let Some(d) = state.loaded.as_mut().and_then(|l| l.details.as_mut()) {
                d.tags = tags;
            }
            Task::none()
        }
        Msg::SaveDetails => {
            let body = state
                .loaded
                .as_ref()
                .and_then(|l| l.details.as_ref().map(meta_body));
            match body {
                Some(Ok(body)) => {
                    let (c, id) = (ctx.client.clone(), state.id.clone());
                    client::task(
                        move || {
                            send(&c, "PUT", &skill_path(&id), Some(&body))
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

/// Save one file's bytes as a file on this computer: the save dialog, then the write (SaveAsPicked).
fn save_dialog(name: String, bytes: Vec<u8>) -> Task<Msg> {
    Task::perform(
        async move {
            rfd::AsyncFileDialog::new()
                .set_title("Save the file")
                .set_file_name(name)
                .save_file()
                .await
                .map(|file| (file.path().to_path_buf(), bytes))
        },
        Msg::SaveAsPicked,
    )
}

// ---- Hub calls -----------------------------------------------------------------------------------------------------

/// The answer to one call: the HTTP status and the JSON body.
type Answer = (u16, Value);

fn skill_path(id: &str) -> String {
    format!("/v1/skills/{}", percent(id))
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

fn post<T: DeserializeOwned>(
    c: &client::Client,
    path: &str,
    body: Option<&Value>,
) -> Result<T, String> {
    expect(send(c, "POST", path, body)?)
}

/// The bytes of one call, for a file or an export. Anything but a 2xx is an error.
fn raw(
    c: &client::Client,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Vec<u8>, String> {
    let bytes = body.map(|b| b.to_string().into_bytes());
    let reply = apz_http::client::request(&c.target, method, path, bytes.as_deref(), c.timeout)
        .map_err(|e| e.to_string())?;
    if (200..300).contains(&reply.status) {
        Ok(reply.body)
    } else {
        Err(format!("HTTP {}", reply.status))
    }
}

/// A version and every one of its files' bytes, in the version's file order.
fn fetch_version(
    c: &client::Client,
    id: &str,
    version: u32,
) -> Result<(Version, Vec<Vec<u8>>), String> {
    let v: Version = get(c, &format!("{}/versions/{version}", skill_path(id)))?;
    let mut files = Vec::with_capacity(v.files.len());
    for f in &v.files {
        files.push(raw(c, "GET", &file_path(id, version, &f.path), None)?);
    }
    Ok((v, files))
}

/// The path of one file of a version: each segment percent-encoded, the slashes kept.
fn file_path(id: &str, version: u32, path: &str) -> String {
    format!(
        "{}/versions/{version}/files/{}",
        skill_path(id),
        path.split('/').map(percent).collect::<Vec<_>>().join("/")
    )
}

fn load(id: &str, ctx: &Ctx) -> Task<Msg> {
    let (c, id) = (ctx.client.clone(), id.to_string());
    client::task(
        move || {
            let detail: Detail = get(&c, &skill_path(&id))?;
            let (version, files) = fetch_version(&c, &id, detail.skill.latest_version)?;
            Ok((detail, version, files))
        },
        Msg::Loaded,
    )
}

/// The two picked versions' diff, from the hub (the lower number first).
fn compare_task(state: &State, ctx: &Ctx) -> Task<Msg> {
    let Some(l) = state.loaded.as_ref() else {
        return Task::none();
    };
    let (from, to) = (l.picked[0].min(l.picked[1]), l.picked[0].max(l.picked[1]));
    let (c, id) = (ctx.client.clone(), state.id.clone());
    client::task(
        move || get::<Diff>(&c, &format!("{}/diff?from={from}&to={to}", skill_path(&id))),
        Msg::Compared,
    )
}

fn refresh(id: &str, ctx: &Ctx) -> Task<Msg> {
    let (c, id) = (ctx.client.clone(), id.to_string());
    client::task(move || get::<Detail>(&c, &skill_path(&id)), Msg::Refreshed)
}

fn version_task(
    id: &str,
    ctx: &Ctx,
    version: u32,
    map: fn(Result<Version, String>) -> Msg,
) -> Task<Msg> {
    let (c, id) = (ctx.client.clone(), id.to_string());
    client::task(
        move || get::<Version>(&c, &format!("{}/versions/{version}", skill_path(&id))),
        map,
    )
}

fn history(id: &str, ctx: &Ctx) -> Task<Msg> {
    let (c, id) = (ctx.client.clone(), id.to_string());
    client::task(
        move || get::<Events>(&c, &format!("{}/history", skill_path(&id))).map(|list| list.events),
        Msg::HistoryLoaded,
    )
}

/// The draft as a new version on top of the latest one. Unchanged files go by their hash, the rest by their bytes.
fn save(id: &str, l: &Loaded, ctx: &Ctx) -> Task<Msg> {
    let (c, id) = (ctx.client.clone(), id.to_string());
    let skill_md = source_text(&l.skill_md);
    let files: Vec<Value> = l.entries.iter().map(file_in).collect();
    let note = l.note.trim().to_string();
    let base = l.summary.latest_version;
    client::task(
        move || {
            let body =
                json!({ "skill_md": skill_md, "files": files, "note": note, "base_version": base });
            let answer = send(
                &c,
                "POST",
                &format!("{}/versions", skill_path(&id)),
                Some(&body),
            )?;
            if answer.0 == 409 {
                return Err(CONFLICT.to_string());
            }
            expect::<Added>(answer)
        },
        Msg::Saved,
    )
}

/// A POST to one of the skill's action routes (publish, unpublish, archive, restore); `done` is the notice on success.
fn act(id: &str, ctx: &Ctx, action: &str, body: Option<Value>, done: String) -> Task<Msg> {
    let (c, path) = (ctx.client.clone(), format!("{}/{action}", skill_path(id)));
    client::task(
        move || {
            send(&c, "POST", &path, body.as_ref())
                .and_then(expect::<Value>)
                .map(|_| done)
        },
        Msg::Acted,
    )
}

// ---- The draft -----------------------------------------------------------------------------------------------------

impl Loaded {
    fn has_path(&self, path: &str) -> bool {
        path == SKILL_MD || self.entries.iter().any(|e| e.path == path)
    }

    fn selected_entry(&self) -> Option<&Entry> {
        self.entries.iter().find(|e| e.path == self.selected)
    }

    fn selected_entry_mut(&mut self) -> Option<&mut Entry> {
        let selected = self.selected.clone();
        self.entries.iter_mut().find(|e| e.path == selected)
    }

    fn skill_changed(&self) -> bool {
        source_text(&self.skill_md) != self.skill_saved
    }

    fn dirty(&self) -> bool {
        self.structure_changed || self.skill_changed() || self.entries.iter().any(Entry::changed)
    }

    /// Runs the checks on the draft, as apz_skill sees it.
    fn recheck(&mut self) {
        let skill_md = source_text(&self.skill_md);
        let files: Vec<FileInfo> = self
            .entries
            .iter()
            .map(|e| FileInfo {
                path: e.path.clone(),
                size: e.size(),
                executable: e.executable,
                text: e.text(),
            })
            .collect();
        self.report = apz_skill::check(&skill_md, &files);
    }
}

/// The skill as loaded from the hub: the latest version's files, checked.
fn build(detail: Detail, version: Version, files: Vec<Vec<u8>>) -> Loaded {
    let entries = version
        .files
        .iter()
        .zip(files)
        .map(|(f, bytes)| Entry {
            path: f.path.clone(),
            executable: f.executable,
            saved: Some((f.hash.clone(), f.size)),
            body: body_of(bytes),
        })
        .collect();
    let mut l = Loaded {
        bindings: detail.skill.bindings.clone(),
        summary: detail.skill,
        versions: sorted(detail.versions),
        skill_md: text_editor::Content::with_text(&version.skill_md),
        skill_saved: normalised(&version.skill_md),
        entries,
        selected: SKILL_MD.to_string(),
        structure_changed: false,
        report: Report {
            rules_version: 0,
            findings: Vec::new(),
            score: 0,
            errors: 0,
            warnings: 0,
            infos: 0,
            stats: Default::default(),
            name: None,
            description: None,
        },
        note: String::new(),
        new_path: String::new(),
        rename_to: String::new(),
        jump: None,
        github: None,
        viewing: None,
        picked: Vec::new(),
        compare: None,
        events: None,
        details: None,
        publish: None,
    };
    l.recheck();
    l
}

/// A file's body: an editor when it is UTF-8 text the checks read, the bytes otherwise.
fn body_of(bytes: Vec<u8>) -> Body {
    match String::from_utf8(bytes) {
        Ok(text) if text.len() <= TEXT_LIMIT => Body::Text {
            content: text_editor::Content::with_text(&text),
            saved: normalised(&text),
        },
        Ok(text) => Body::Binary(text.into_bytes()),
        Err(e) => Body::Binary(e.into_bytes()),
    }
}

/// The JSON of one file in a save: its hash when unchanged, its bytes (base64) otherwise.
fn file_in(e: &Entry) -> Value {
    match &e.saved {
        Some((hash, _)) if !e.text_changed() => {
            json!({ "path": e.path, "executable": e.executable, "hash": hash })
        }
        _ => {
            json!({ "path": e.path, "executable": e.executable, "content_b64": STANDARD.encode(e.bytes()) })
        }
    }
}

/// Adds a file to the draft and selects it.
fn add(l: &mut Loaded, path: String, body: Body) -> Result<(), String> {
    if !valid_path(&path) {
        return Err(format!(
            "{path:?} is not a valid file path: relative, no .., no empty part, and not SKILL.md"
        ));
    }
    if l.has_path(&path) {
        return Err(format!("{path} is already in the skill"));
    }
    l.entries.push(Entry {
        path: path.clone(),
        executable: false,
        saved: None,
        body,
    });
    l.entries.sort_by(|a, b| a.path.cmp(&b.path));
    l.selected = path;
    l.structure_changed = true;
    l.recheck();
    Ok(())
}

fn rename(l: &mut Loaded) -> Result<(), String> {
    let to = l.rename_to.trim().to_string();
    let selected = l.selected.clone();
    if selected == SKILL_MD {
        return Err("SKILL.md keeps its name.".to_string());
    }
    if !valid_path(&to) {
        return Err(format!(
            "{to:?} is not a valid file path: relative, no .., no empty part, and not SKILL.md"
        ));
    }
    if l.has_path(&to) {
        return Err(format!("{to} is already in the skill"));
    }
    if let Some(e) = l.entries.iter_mut().find(|e| e.path == selected) {
        e.path = to.clone();
    }
    l.entries.sort_by(|a, b| a.path.cmp(&b.path));
    l.selected = to;
    l.rename_to.clear();
    l.structure_changed = true;
    l.recheck();
    Ok(())
}

/// A file path the server accepts (its `safe_path`): relative, `/`-separated, no `..`, no empty part, not SKILL.md.
fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 300
        && !path.starts_with('/')
        && !path.contains('\\')
        && path != SKILL_MD
        && path
            .split('/')
            .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

/// The draft's text, without the trailing newline the editor adds.
fn source_text(source: &text_editor::Content) -> String {
    let text = source.text();
    text.strip_suffix('\n').map(str::to_string).unwrap_or(text)
}

/// Text as the editor keeps it, so a text compares equal to the draft until it is edited.
fn normalised(text: &str) -> String {
    source_text(&text_editor::Content::with_text(text))
}

/// The PUT body for a skill's title and tags from the Details form, checked against the hub's limits first.
fn meta_body(d: &SkillDetails) -> Result<Value, String> {
    let title = d.title.trim();
    if title.is_empty() || title.chars().count() > MAX_TITLE {
        return Err(format!("The title must be 1 to {MAX_TITLE} characters."));
    }
    let tags = prompt_editor::parse_tags(&d.tags);
    if tags.len() > MAX_TAGS || tags.iter().any(|t| t.chars().count() > MAX_TAG) {
        return Err(format!(
            "At most {MAX_TAGS} tags of 1 to {MAX_TAG} characters."
        ));
    }
    Ok(json!({ "title": title, "tags": tags }))
}

fn sorted(mut versions: Vec<VersionMeta>) -> Vec<VersionMeta> {
    versions.sort_by_key(|v| Reverse(v.version));
    versions
}

fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// A percent-encoded path segment (RFC 3986 unreserved characters pass through).
fn percent(segment: &str) -> String {
    segment
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn is_markdown(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".md")
}

fn selected_text(l: &Loaded) -> Option<String> {
    if l.selected == SKILL_MD {
        Some(source_text(&l.skill_md))
    } else {
        l.selected_entry().and_then(Entry::text)
    }
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

fn num(n: Option<u32>) -> String {
    n.map_or_else(String::new, |n| n.to_string())
}

// ---- Views ---------------------------------------------------------------------------------------------------------

/// A size as the web's editor shows it: bytes, KB or MB.
fn size_label(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

/// The source's name, as the header's badge shows it.
fn source_name(source: &Source) -> &'static str {
    match source {
        Source::Built => "Built in",
        Source::Upload => "Uploaded",
        Source::Github { .. } => "GitHub",
    }
}

/// A binding's name: Every agent for `plaza`, Role x for `role:x`.
fn binding_name(binding: &str) -> String {
    if binding == EVERY_AGENT {
        "Every agent".to_string()
    } else if let Some(role) = binding.strip_prefix("role:") {
        format!("Role {role}")
    } else {
        binding.to_string()
    }
}

/// SKILL.md's size in bytes, as the draft holds it.
fn skill_md_size(l: &Loaded) -> u64 {
    source_text(&l.skill_md).len() as u64
}

/// The page head: the title with the skill's actions, the description, the status badges (source, live, latest, score,
/// bindings, tags, archived, unsaved, the hub's change) and the GitHub source line.
fn header<'a>(state: &'a State, l: &'a Loaded) -> Element<'a, Msg> {
    let s = &l.summary;
    let archived = s.archived_ms.is_some();
    let editing = l.details.is_some();
    let mut actions: Vec<Element<'a, Msg>> = vec![
        ui::btn_sm("Back to skills", None, ui::Kind::Ghost, Some(Msg::Back)),
        ui::btn_sm("Save version", None, ui::Kind::Primary, Some(Msg::Save)),
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
        ui::btn_sm("Export", None, ui::Kind::Ghost, Some(Msg::Export)),
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
        "Untitled skill"
    } else {
        s.title.as_str()
    };
    let mut bar: Vec<Element<'a, Msg>> = vec![
        ui::badge(source_name(&s.source), ui::Tone::Muted),
        match s.active_version {
            Some(v) => ui::badge(&format!("Live v{v}"), ui::Tone::Ok),
            None => ui::badge("Not published", ui::Tone::Muted),
        },
        ui::badge(&format!("Latest v{}", s.latest_version), ui::Tone::Muted),
    ];
    if let Some(score) = s.score {
        bar.push(ui::badge(&format!("Score {score}"), score_tone(score)));
    }
    for binding in &s.bindings {
        bar.push(ui::badge::<Msg>(&binding_name(binding), ui::Tone::Signal));
    }
    for tag in &s.tags {
        bar.push(ui::badge::<Msg>(tag, ui::Tone::Muted));
    }
    if archived {
        bar.push(ui::badge("Archived", ui::Tone::Muted));
    }
    if l.dirty() {
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
    let mut parts: Vec<Element<'a, Msg>> = vec![ui::view_head(title, actions)];
    if !s.description.is_empty() {
        parts.push(ui::faint(s.description.clone()).into());
    }
    parts.push(row(bar).spacing(8).align_y(Alignment::Center).wrap().into());
    if let Some(line) = source_line(l) {
        parts.push(line);
    }
    column(parts).spacing(8).into()
}

/// The GitHub source: the repo at its commit, Check for updates, and the result (an update to take, or up to date).
fn source_line(l: &Loaded) -> Option<Element<'_, Msg>> {
    let Source::Github { repo, commit } = &l.summary.source else {
        return None;
    };
    let short: String = commit.chars().take(7).collect();
    let mut parts: Vec<Element<'_, Msg>> = vec![
        ui::mono(format!("{repo}@{short}"), 12.5, palette().ink_2).into(),
        ui::btn_sm(
            "Check for updates",
            None,
            ui::Kind::Secondary,
            Some(Msg::CheckGithub),
        ),
    ];
    if let Some(check) = l.github.as_ref() {
        if check.update_available {
            let latest: String = check.latest.chars().take(7).collect();
            parts.push(ui::faint(format!("New commit {latest}")).into());
            parts.push(ui::btn_sm(
                "Update",
                None,
                ui::Kind::Primary,
                Some(Msg::UpdateGithub),
            ));
        } else {
            parts.push(ui::faint("Up to date").into());
        }
    }
    Some(row(parts).spacing(10).align_y(Alignment::Center).into())
}

/// The Details form: the title and the tags, saved as the skill's metadata (its files and versions do not change).
fn details_form(d: &SkillDetails) -> Element<'_, Msg> {
    let body = column![
        field(
            "Title",
            text_input("Shown in the list and the header.", &d.title)
                .on_input(Msg::DetailTitle)
                .padding(8)
                .style(crate::theme::input)
                .into(),
        ),
        field(
            "Tags",
            text_input("comma, separated", &d.tags)
                .on_input(Msg::DetailTags)
                .padding(8)
                .style(crate::theme::input)
                .into(),
        ),
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
        ui::faint("Publishing an older version rolls the skill back. Versions with errors cannot be published."),
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

/// The Edit tab: the file tree beside the selected file, its findings, then the note and the bindings.
fn edit_tab<'a>(l: &'a Loaded, roles: &'a [Role]) -> Element<'a, Msg> {
    let tree = container(file_card(l)).width(Length::Fixed(260.0));
    let editor = container(column![selected_pane(l), findings(l)].spacing(12)).width(Length::Fill);
    column![
        row![tree, editor].spacing(16).align_y(Alignment::Start),
        field(
            "Note",
            text_input("What changed, in a few words", &l.note)
                .on_input(Msg::Note)
                .padding(8)
                .style(crate::theme::input)
                .into(),
        ),
        bindings_card(l, roles),
    ]
    .spacing(16)
    .into()
}

/// The file tree, as the web's Files card: SKILL.md, then the files by path, each with its marks and size; the tools to add
/// a file, and for the selected one, rename, executable and remove.
fn file_card(l: &Loaded) -> Element<'_, Msg> {
    let mut items: Vec<Element<'_, Msg>> = vec![file_item(
        SKILL_MD,
        l.selected == SKILL_MD,
        l.skill_changed(),
        false,
        skill_md_size(l),
    )];
    let mut entries: Vec<&Entry> = l.entries.iter().collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    for e in entries {
        items.push(file_item(
            &e.path,
            l.selected == e.path,
            e.changed(),
            e.executable,
            e.size(),
        ));
    }
    let mut tools: Vec<Element<'_, Msg>> = vec![
        text_input("new file path, e.g. scripts/run.py", &l.new_path)
            .on_input(Msg::NewPath)
            .on_submit(Msg::AddFile)
            .padding(8)
            .style(crate::theme::input)
            .into(),
        row![
            ui::btn_sm("Add file", None, ui::Kind::Secondary, Some(Msg::AddFile)),
            ui::btn_sm("Upload", None, ui::Kind::Secondary, Some(Msg::PickFromDisk)),
        ]
        .spacing(8)
        .into(),
    ];
    if let Some(e) = l.selected_entry() {
        tools.push(
            text_input("rename to", &l.rename_to)
                .on_input(Msg::RenameTo)
                .on_submit(Msg::Rename)
                .padding(8)
                .style(crate::theme::input)
                .into(),
        );
        tools.push(
            row![
                ui::btn_sm("Rename", None, ui::Kind::Secondary, Some(Msg::Rename)),
                ui::btn_sm(
                    if e.executable {
                        "Not executable"
                    } else {
                        "Make executable"
                    },
                    None,
                    ui::Kind::Secondary,
                    Some(Msg::ToggleExec),
                ),
                ui::btn_sm("Remove", None, ui::Kind::Ghost, Some(Msg::Remove)),
            ]
            .spacing(8)
            .wrap()
            .into(),
        );
    }
    ui::card(
        "Files",
        column![column(items).spacing(2), column(tools).spacing(8)]
            .spacing(14)
            .into(),
    )
}

/// One file in the tree: its path in mono, and its marks and size beneath; the selected one is shaded.
fn file_item(
    path: &str,
    active: bool,
    changed: bool,
    executable: bool,
    size: u64,
) -> Element<'static, Msg> {
    let mut meta: Vec<Element<'static, Msg>> = Vec::new();
    if executable {
        meta.push(ui::badge("exec", ui::Tone::Muted));
    }
    if changed {
        meta.push(ui::badge("changed", ui::Tone::Warn));
    }
    meta.push(ui::faint(size_label(size)).into());
    let body = column![
        ui::mono(path.to_string(), 13.0, palette().ink),
        row(meta).spacing(6).align_y(Alignment::Center),
    ]
    .spacing(2);
    button(body)
        .on_press(Msg::Select(path.to_string()))
        .width(Length::Fill)
        .padding([6, 8])
        .style(move |_: &Theme, status: button::Status| {
            let p = palette();
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: if active {
                    Some(p.sheet_2.into())
                } else if hovered {
                    Some(p.hover.into())
                } else {
                    None
                },
                text_color: p.ink,
                border: Border {
                    radius: crate::theme::skin().radius.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .into()
}

/// The selected file's head (its path, the marks, its size and Save as), then its editor or its binary note.
fn selected_pane(l: &Loaded) -> Element<'_, Msg> {
    let mut parts: Vec<Element<'_, Msg>> = Vec::new();
    if l.selected == SKILL_MD {
        parts.push(file_head(
            SKILL_MD,
            skill_md_size(l),
            vec![badge_skill()],
            false,
        ));
        parts.push(text_pane(&l.skill_md, true, Msg::EditSkill));
    } else if let Some(e) = l.selected_entry() {
        let mut marks: Vec<Element<'_, Msg>> = Vec::new();
        if e.saved.is_none() {
            marks.push(ui::badge("new", ui::Tone::Signal));
        }
        if e.executable {
            marks.push(ui::badge("executable", ui::Tone::Muted));
        }
        parts.push(file_head(&e.path, e.size(), marks, true));
        match &e.body {
            Body::Text { content, .. } => {
                if e.saved.is_some() && e.text_changed() {
                    parts.push(
                        ui::faint("Save as writes the saved version: unsaved edits are not in it.")
                            .into(),
                    );
                }
                parts.push(text_pane(content, is_markdown(&e.path), Msg::EditFile));
            }
            Body::Binary(bytes) => parts.push(binary_pane(&e.path, bytes.len() as u64)),
        }
    } else {
        parts.push(ui::faint("Pick a file in the tree.").into());
    }
    if let Some(line) = l.jump
        && let Some(text) = selected_text(l).and_then(|t| {
            t.lines()
                .nth(line.saturating_sub(1) as usize)
                .map(str::to_string)
        })
    {
        parts.push(ui::mono(format!("line {line}: {text}"), 12.0, palette().ink_3).into());
    }
    column(parts).spacing(10).width(Length::Fill).into()
}

/// The Skill badge on SKILL.md's head.
fn badge_skill() -> Element<'static, Msg> {
    ui::badge("Skill", ui::Tone::Signal)
}

/// A file's head: its path in mono, its marks, its size, and Save as at the right.
fn file_head<'a>(
    path: &str,
    size: u64,
    marks: Vec<Element<'a, Msg>>,
    save_as: bool,
) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> =
        vec![ui::mono(path.to_string(), 15.0, palette().ink).into()];
    items.extend(marks);
    items.push(ui::faint(size_label(size)).into());
    items.push(Space::new().width(Length::Fill).into());
    if save_as {
        items.push(ui::btn_sm(
            "Save as",
            None,
            ui::Kind::Secondary,
            Some(Msg::SaveAs),
        ));
    }
    row(items).spacing(8).align_y(Alignment::Center).into()
}

/// A text file's source beside its live markdown preview for markdown files, as the web's two panes.
fn text_pane<'a>(
    content: &'a text_editor::Content,
    markdown: bool,
    on_edit: fn(text_editor::Action) -> Msg,
) -> Element<'a, Msg> {
    let editor = text_editor(content)
        .style(crate::theme::editor)
        .on_action(on_edit)
        .font(Font::MONOSPACE)
        .height(Length::Fixed(360.0));
    let source = column![
        ui::faint("Source"),
        container(editor)
            .width(Length::Fill)
            .style(prompt_editor::well_style),
    ]
    .spacing(6)
    .width(Length::FillPortion(1));
    if !markdown {
        return source.into();
    }
    let preview = column![
        ui::faint("Preview"),
        container(
            ui::scroll(ui::markdown::<Msg>(&source_text(content))).height(Length::Fixed(360.0)),
        )
        .padding(12)
        .width(Length::Fill)
        .style(prompt_editor::well_style),
    ]
    .spacing(6)
    .width(Length::FillPortion(1));
    row![source, preview].spacing(12).into()
}

/// A binary file's note: its size, and Save as to keep it on this computer.
fn binary_pane(path: &str, size: u64) -> Element<'static, Msg> {
    container(
        column![
            ui::sans(
                format!(
                    "{path} is a binary file, {}. It is not shown in the editor.",
                    size_label(size)
                ),
                13.0,
                palette().ink_2,
            ),
            ui::btn_sm("Save as", None, ui::Kind::Secondary, Some(Msg::SaveAs)),
        ]
        .spacing(8),
    )
    .padding(16)
    .width(Length::Fill)
    .style(prompt_editor::well_style)
    .into()
}

/// The checks on the draft: the banner for code, the score and counts, then the findings grouped by file (SKILL.md first).
fn findings(l: &Loaded) -> Element<'_, Msg> {
    let r = &l.report;
    let mut summary: Vec<Element<'_, Msg>> = Vec::new();
    if r.findings.iter().any(|f| f.rule == "script") {
        summary.push(
            container(ui::sans(
                "This skill contains code. Agents may run it in their own VMs. Read it before publishing.",
                13.0,
                ui::Tone::Warn.color(),
            ))
            .padding([8, 12])
            .width(Length::Fill)
            .style(|_: &Theme| container::Style {
                background: Some(palette().warn_soft.into()),
                border: Border {
                    radius: crate::theme::skin().radius.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into(),
        );
    }
    summary.push(
        row![
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
                "{} files, {} body lines, about {} tokens",
                r.stats.files, r.stats.body_lines, r.stats.est_tokens
            )),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .wrap()
        .into(),
    );
    let mut groups: Vec<Element<'_, Msg>> = Vec::new();
    if r.findings.is_empty() {
        groups.push(ui::faint("No findings. This skill passes every check.").into());
    }
    let mut paths: Vec<&str> = Vec::new();
    for f in &r.findings {
        if !paths.contains(&f.path.as_str()) {
            paths.push(&f.path);
        }
    }
    paths.sort_by(|a, b| match (*a == SKILL_MD, *b == SKILL_MD) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.cmp(b),
    });
    for path in paths {
        groups.push(ui::label(path).into());
        groups.extend(
            r.findings
                .iter()
                .filter(|f| f.path == path)
                .map(finding_row),
        );
    }
    column![
        column(summary).spacing(10),
        ui::card("Findings", column(groups).spacing(6).into()),
    ]
    .spacing(12)
    .into()
}

/// One finding: its severity in the tone, the place (a line jumps to it), the message and the file and rule, in a
/// ruled box.
fn finding_row(f: &Finding) -> Element<'static, Msg> {
    let (name, tone) = match f.severity {
        Severity::Error => ("Error", ui::Tone::Danger),
        Severity::Warning => ("Warning", ui::Tone::Warn),
        Severity::Info => ("Info", ui::Tone::Info),
    };
    let place: Element<'static, Msg> = if f.line == 0 {
        ui::faint("Whole file").into()
    } else {
        ui::link(
            &format!("Line {}", f.line),
            Msg::Jump(f.path.clone(), f.line),
            false,
        )
    };
    prompt_editor::boxed(
        row![
            ui::sans(name, 13.0, tone.color()),
            place,
            container(ui::prose(f.message.clone())).width(Length::Fill),
            ui::mono(format!("{}: {}", f.path, f.rule), 12.0, palette().ink_3),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into(),
    )
}

/// The bindings picker, the same as New skill and the import preview: an Every agent box, then a box per role, by label.
fn bindings_card<'a>(l: &'a Loaded, roles: &'a [Role]) -> Element<'a, Msg> {
    let every = l.bindings.iter().any(|b| b.as_str() == EVERY_AGENT);
    let mut boxes: Vec<Element<'a, Msg>> = vec![
        checkbox(every)
            .label("Every agent")
            .on_toggle(|on| Msg::ToggleBinding(EVERY_AGENT.to_string(), on))
            .into(),
    ];
    let mut by_label: Vec<&Role> = roles.iter().collect();
    by_label.sort_by(|a, b| a.title.cmp(&b.title));
    for r in by_label {
        let binding = format!("role:{}", r.id);
        let on = l.bindings.contains(&binding);
        boxes.push(
            checkbox(on)
                .label(format!("Role: {}", r.title))
                .on_toggle(move |on| Msg::ToggleBinding(binding.clone(), on))
                .into(),
        );
    }
    ui::card_with(
        "Bindings",
        vec![ui::btn_sm(
            "Save bindings",
            None,
            ui::Kind::Primary,
            Some(Msg::SaveBindings),
        )],
        column(boxes).spacing(8).into(),
    )
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
        parts.push(prompt_editor::table(
            &[
                "Version",
                "Note",
                "Author",
                "Created",
                "Score",
                "Errors / warnings / infos",
                "",
            ],
            &[2, 3, 2, 2, 1, 2, 0],
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

/// One version's cells: its number with the live, best and stale marks; its note, author and age; its score and counts;
/// and its actions (view, compare or unpick, publish or roll back, restore as new).
fn version_cells<'a>(l: &'a Loaded, v: &'a VersionMeta) -> Vec<Element<'a, Msg>> {
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
    if v.rules_version < apz_skill::RULES_VERSION {
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
        ui::sans(
            format!("{} / {} / {}", v.errors, v.warnings, v.infos),
            13.0,
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

/// A version read only, in a card: its badges, its markdown, its other files (or none), and Close.
fn viewing(v: &Version) -> Element<'_, Msg> {
    let mut badges: Vec<Element<'_, Msg>> = Vec::new();
    if v.stale {
        badges.push(ui::badge("stale", ui::Tone::Warn));
    }
    badges.push(ui::badge(
        &format!("Score {}", v.score),
        score_tone(v.score),
    ));
    badges.push(ui::btn_sm(
        "Close",
        None,
        ui::Kind::Ghost,
        Some(Msg::CloseView),
    ));
    let mut body: Vec<Element<'_, Msg>> = vec![ui::markdown::<Msg>(&v.skill_md)];
    if v.files.is_empty() {
        body.push(ui::faint("No other files.").into());
    } else {
        for f in &v.files {
            body.push(
                row![
                    ui::mono(f.path.clone(), 12.5, palette().ink),
                    ui::faint(size_label(f.size)),
                ]
                .spacing(12)
                .align_y(Alignment::Center)
                .into(),
            );
        }
    }
    ui::card_with(
        &format!("v{}, read only", v.version),
        badges,
        column(body).spacing(12).into(),
    )
}

/// The comparison of two picked versions, in a card: each side's score and counts, the files added, removed and changed,
/// and the SKILL.md line diff with its numbers, then Close.
fn compare(d: &Diff) -> Element<'_, Msg> {
    let side = |v: &VersionMeta| -> Element<'static, Msg> {
        column![
            ui::sans(format!("v{}", v.version), 14.0, palette().ink),
            row![
                ui::badge(&format!("Score {}", v.score), score_tone(v.score)),
                ui::badge(
                    &format!("{} errors", v.errors),
                    if v.errors > 0 {
                        ui::Tone::Danger
                    } else {
                        ui::Tone::Muted
                    }
                ),
                ui::badge(
                    &format!("{} warnings", v.warnings),
                    if v.warnings > 0 {
                        ui::Tone::Warn
                    } else {
                        ui::Tone::Muted
                    }
                ),
                ui::badge(
                    &format!("{} infos", v.infos),
                    if v.infos > 0 {
                        ui::Tone::Info
                    } else {
                        ui::Tone::Muted
                    }
                ),
            ]
            .spacing(6)
            .wrap(),
        ]
        .spacing(6)
        .width(Length::Fill)
        .into()
    };
    let list = |label: &str, items: &[String]| -> Element<'static, Msg> {
        let mut rows: Vec<Element<'static, Msg>> =
            vec![ui::sans(format!("{label} ({})", items.len()), 13.0, palette().ink).into()];
        if items.is_empty() {
            rows.push(ui::faint("None").into());
        }
        rows.extend(
            items
                .iter()
                .map(|p| ui::mono(p.clone(), 12.0, palette().ink_2).into()),
        );
        column(rows).spacing(4).width(Length::Fill).into()
    };
    let lines: Vec<Element<'_, Msg>> = d
        .skill_md
        .iter()
        .map(|line| {
            let (sign, ink, ground) = match line.op.as_str() {
                "add" => ("+", palette().ok, Some(palette().ok_soft)),
                "del" => ("−", palette().crit, Some(palette().crit_soft)),
                _ => (" ", palette().ink_2, None),
            };
            container(
                row![
                    ui::mono(format!("{:>4}", num(line.old)), 12.0, palette().ink_3),
                    ui::mono(format!("{:>4}", num(line.new)), 12.0, palette().ink_3),
                    ui::mono(sign.to_string(), 12.0, ink),
                    ui::mono(line.text.clone(), 12.0, ink),
                ]
                .spacing(8),
            )
            .padding([1, 10])
            .width(Length::Fill)
            .style(move |_: &Theme| container::Style {
                background: ground.map(iced::Background::Color),
                ..Default::default()
            })
            .into()
        })
        .collect();
    ui::card_with(
        &format!("Compare v{} and v{}", d.from.version, d.to.version),
        vec![ui::btn_sm(
            "Close",
            None,
            ui::Kind::Ghost,
            Some(Msg::ClearPicks),
        )],
        column![
            row![side(&d.from), side(&d.to)].spacing(24),
            row![
                list("Added", &d.files.added),
                list("Removed", &d.files.removed),
                list("Changed", &d.files.changed),
            ]
            .spacing(24),
            container(ui::scroll(column(lines).width(Length::Fill)).height(Length::Fixed(360.0)))
                .width(Length::Fill)
                .style(prompt_editor::well_style),
        ]
        .spacing(16)
        .into(),
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
    prompt_editor::table(&["Action", "Version", "Actor", "When"], &[2, 1, 2, 2], rows)
}

/// The Rules tab: the checks every skill is scored against, grouped by severity, each with its id and why.
fn rules_tab() -> Element<'static, Msg> {
    let rules = apz_skill::rules();
    let mut parts: Vec<Element<'static, Msg>> = vec![
        ui::faint("The checks every skill is scored against. Errors block publishing. Warnings and infos lower the score.")
            .into(),
    ];
    for (severity, name) in [
        (Severity::Error, "Errors"),
        (Severity::Warning, "Warnings"),
        (Severity::Info, "Infos"),
    ] {
        let list: Vec<&apz_skill::Rule> = rules.iter().filter(|r| r.severity == severity).collect();
        let mut group: Vec<Element<'static, Msg>> =
            vec![ui::sans(format!("{name} ({})", list.len()), 15.0, palette().ink).into()];
        if list.is_empty() {
            group.push(ui::faint("None.").into());
        }
        for rule in list {
            group.push(
                column![
                    row![
                        ui::sans(rule.title.to_string(), 14.0, palette().ink),
                        ui::mono(rule.id.to_string(), 11.0, palette().ink_3),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                    ui::prose(rule.why.to_string()),
                ]
                .spacing(4)
                .into(),
            );
        }
        parts.push(column(group).spacing(10).into());
    }
    column(parts).spacing(18).into()
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let Some(l) = state.loaded.as_ref() else {
        let status: Element<'a, Msg> = match &state.error {
            Some(e) => ui::error_line(e),
            None => ui::loading(),
        };
        return ui::sheet(
            ui::view_head(
                "Skill",
                vec![ui::btn_sm(
                    "Back to skills",
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
        body.push(details_form(d));
    }
    body.push(ui::tabs(&TABS, state.tab, Msg::Tab));
    body.push(match state.tab {
        Tab::Edit => edit_tab(l, &state.roles),
        Tab::Versions => versions_tab(l),
        Tab::History => history_tab(l),
        Tab::Rules => rules_tab(),
    });
    ui::sheet(
        header(state, l),
        column(body).spacing(16).width(Length::Fill).into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn details(title: &str, tags: &str) -> SkillDetails {
        SkillDetails {
            title: title.to_string(),
            tags: tags.to_string(),
        }
    }

    #[test]
    fn the_meta_body_carries_the_title_and_the_tags() {
        let body = meta_body(&details("  Review  ", " code, , safety ")).unwrap();
        assert_eq!(
            body,
            json!({ "title": "Review", "tags": ["code", "safety"] })
        );
    }

    #[test]
    fn a_file_path_keeps_its_slashes_and_encodes_each_segment() {
        assert_eq!(
            file_path("demo", 3, "scripts/run me.py"),
            "/v1/skills/demo/versions/3/files/scripts/run%20me.py"
        );
    }
}
