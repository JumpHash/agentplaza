//! The prompt editor (docs/design/prompt-library.md, "Screens", the web's prompt editor at 1:1): the markdown source beside
//! a live preview, the findings, score and token estimate checked locally with `apz_prompt` as you type, Save version,
//! Publish, Unpublish and Archive, and the Versions (view, compare, publish, restore), History and Rules tabs. `prompts`
//! owns it: `open` starts it for one prompt and `Msg::Back` returns to the list. The hub's JSON is read through
//! `apz_http` with the status kept, so a 409 (a newer version was saved) is told apart from other failures.

use std::cmp::Reverse;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use apz_prompt::{Finding, Kind, Report, Severity};
use iced::widget::{column, container, row, text_editor, text_input};
use iced::{Alignment, Element, Font, Length, Task};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::console::{client, ui, Ctx};
use crate::theme::palette;

use super::mcp_hub;

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

const TABS: [(Tab, &str); 4] = [(Tab::Edit, "Edit"), (Tab::Versions, "Versions"), (Tab::History, "History"), (Tab::Rules, "Rules")];

#[derive(Debug, Clone)]
pub enum Msg {
    /// Back to the list (handled by `prompts`, which asks first when the draft has unsaved changes).
    Back,
    /// Leave without saving (handled by `prompts`), after the confirmation.
    Discard,
    /// Keep editing after the confirmation.
    KeepEditing,
    Tab(Tab),
    Loaded(Result<(PromptDetail, PromptVersion), String>),
    Refreshed(Result<PromptDetail, String>),
    Edit(text_editor::Action),
    Note(String),
    Save,
    Saved(Result<AddVersion, String>),
    /// Reload the prompt after a 409, which discards the draft.
    ReloadLatest,
    /// A publish, unpublish, archive or restore went through (its notice), or failed.
    Acted(Result<String, String>),
    Publish(u32),
    Unpublish,
    ToggleArchive,
    View(u32),
    Viewed(Result<PromptVersion, String>),
    CloseView,
    Pick(u32),
    ClearPicks,
    Compare,
    Compared(Result<PromptDiff, String>),
    RestoreVersion(u32),
    Restored(Result<PromptVersion, String>),
    HistoryLoaded(Result<Vec<PromptEvent>, String>),
    /// Save the latest version as a Markdown file: the save dialog first, then the body.
    Download,
    DownloadPicked(Option<PathBuf>),
    Downloaded(Result<(PromptVersion, PathBuf), String>),
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
}

/// The editor for prompt `id`, and the task that loads it.
pub fn open(id: String, ctx: &Ctx) -> (State, Task<Msg>) {
    let task = load(&id, ctx);
    (State { id, tab: Tab::Edit, loaded: None, error: None, notice: None, conflict: false, leaving: false }, task)
}

/// Whether the draft differs from the body last loaded or saved.
pub fn is_dirty(state: &State) -> bool {
    state.loaded.as_ref().is_some_and(dirty)
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
            let wants_history = tab == Tab::History && state.loaded.as_ref().is_some_and(|l| l.events.is_none());
            if wants_history {
                history(&state.id, ctx)
            } else {
                Task::none()
            }
        }
        Msg::Loaded(Ok((detail, latest))) => {
            let kind = Kind::parse(&detail.prompt.kind).unwrap_or(Kind::Fragment);
            state.loaded = Some(Loaded {
                report: apz_prompt::check(kind, &latest.body),
                source: text_editor::Content::with_text(&latest.body),
                saved: source_text(&text_editor::Content::with_text(&latest.body)),
                pending: String::new(),
                kind,
                summary: detail.prompt,
                versions: sorted(detail.versions),
                note: String::new(),
                viewing: None,
                picked: Vec::new(),
                compare: None,
                events: None,
            });
            state.error = None;
            Task::none()
        }
        Msg::Loaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::Refreshed(Ok(detail)) => {
            if let Some(l) = state.loaded.as_mut() {
                l.summary = detail.prompt;
                l.versions = sorted(detail.versions);
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
            state.notice = Some(if added.created {
                (format!("Saved as v{}", added.version.version), false)
            } else {
                ("No changes: the body is the same as the latest version.".to_string(), false)
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
            refresh(&state.id, ctx)
        }
        Msg::Acted(Err(e)) => {
            state.notice = Some((e, true));
            Task::none()
        }
        Msg::Publish(version) => act(&state.id, ctx, "publish", Some(json!({ "version": version })), format!("Published v{version}")),
        Msg::Unpublish => act(&state.id, ctx, "unpublish", None, "Unpublished".to_string()),
        Msg::ToggleArchive => {
            let archived = state.loaded.as_ref().is_some_and(|l| l.summary.archived_ms.is_some());
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
                if let Some(at) = l.picked.iter().position(|&p| p == version) {
                    l.picked.remove(at);
                } else {
                    if l.picked.len() == 2 {
                        l.picked.remove(0);
                    }
                    l.picked.push(version);
                }
            }
            Task::none()
        }
        Msg::ClearPicks => {
            if let Some(l) = state.loaded.as_mut() {
                l.picked.clear();
                l.compare = None;
            }
            Task::none()
        }
        Msg::Compare => match state.loaded.as_ref() {
            Some(l) if l.picked.len() == 2 => {
                let (from, to) = (l.picked[0].min(l.picked[1]), l.picked[0].max(l.picked[1]));
                let (c, id) = (ctx.client.clone(), state.id.clone());
                client::task(move || get::<PromptDiff>(&c, &format!("{}/diff?from={from}&to={to}", prompt_path(&id))), Msg::Compared)
            }
            _ => Task::none(),
        },
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
            state.notice = Some((format!("v{} is in the editor. Save it to make it the latest version.", version.version), false));
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
                let name = if matches!(l.kind, Kind::AgentsMd) { "AGENTS.md".to_string() } else { format!("{}-v{}.md", state.id, l.summary.latest_version) };
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
                let (c, id, version) = (ctx.client.clone(), state.id.clone(), l.summary.latest_version);
                client::task(move || get::<PromptVersion>(&c, &format!("{}/versions/{version}", prompt_path(&id))).map(|v| (v, path)), Msg::Downloaded)
            }
            None => Task::none(),
        },
        Msg::Downloaded(Ok((version, path))) => {
            state.notice = Some(match std::fs::write(&path, version.body.as_bytes()) {
                Ok(()) => (format!("Saved v{} to {}", version.version, path.display()), false),
                Err(e) => (format!("The file could not be written: {e}"), true),
            });
            Task::none()
        }
        Msg::Downloaded(Err(e)) => {
            state.notice = Some((e, true));
            Task::none()
        }
    }
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let Some(l) = state.loaded.as_ref() else {
        let status: Element<'a, Msg> = match &state.error {
            Some(e) => ui::error_line(e),
            None => ui::loading(),
        };
        return column![ui::btn_sm("Back", None, ui::Kind::Secondary, Some(Msg::Back)), status].spacing(16).into();
    };
    let mut parts: Vec<Element<'a, Msg>> = vec![header(l)];
    if let Some((text, is_error)) = &state.notice {
        parts.push(if *is_error { ui::error_line(text) } else { ui::faint(text.clone()).into() });
    }
    if state.conflict {
        parts.push(ui::btn_sm("Reload latest", None, ui::Kind::Secondary, Some(Msg::ReloadLatest)));
    }
    if state.leaving {
        parts.push(
            row![
                ui::faint("Discard your changes?"),
                ui::btn_sm("Discard", None, ui::Kind::Secondary, Some(Msg::Discard)),
                ui::btn_sm("Keep editing", None, ui::Kind::Ghost, Some(Msg::KeepEditing)),
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    parts.push(ui::tabs(&TABS, state.tab, Msg::Tab));
    parts.push(match state.tab {
        Tab::Edit => edit_tab(l),
        Tab::Versions => versions_tab(l),
        Tab::History => history_tab(l),
        Tab::Rules => rules_tab(l.kind),
    });
    column(parts).spacing(18).width(Length::Fill).into()
}

// ---- Hub calls -----------------------------------------------------------------------------------------------------

/// The answer to one call: the HTTP status and the JSON body.
type Answer = (u16, Value);

fn prompt_path(id: &str) -> String {
    format!("/v1/prompts/{id}")
}

fn send(c: &client::Client, method: &str, path: &str, body: Option<&Value>) -> Result<Answer, String> {
    apz_http::client::request_json(&c.target, method, path, body, c.timeout).map_err(|e| e.to_string())
}

/// A 2xx answer as `T`; anything else as the hub's `error` text (or the status).
fn expect<T: DeserializeOwned>((status, value): Answer) -> Result<T, String> {
    if (200..300).contains(&status) {
        serde_json::from_value(value).map_err(|e| e.to_string())
    } else {
        Err(value.get("error").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("HTTP {status}")))
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
            let latest: PromptVersion = get(&c, &format!("{}/versions/{}", prompt_path(&id), detail.prompt.latest_version))?;
            Ok((detail, latest))
        },
        Msg::Loaded,
    )
}

fn refresh(id: &str, ctx: &Ctx) -> Task<Msg> {
    let (c, id) = (ctx.client.clone(), id.to_string());
    client::task(move || get::<PromptDetail>(&c, &prompt_path(&id)), Msg::Refreshed)
}

fn history(id: &str, ctx: &Ctx) -> Task<Msg> {
    let (c, id) = (ctx.client.clone(), id.to_string());
    client::task(move || get::<Events>(&c, &format!("{}/history", prompt_path(&id))).map(|list| list.events), Msg::HistoryLoaded)
}

fn version_task(id: &str, ctx: &Ctx, version: u32, map: fn(Result<PromptVersion, String>) -> Msg) -> Task<Msg> {
    let (c, id) = (ctx.client.clone(), id.to_string());
    client::task(move || get::<PromptVersion>(&c, &format!("{}/versions/{version}", prompt_path(&id))), map)
}

/// Save the draft as a new version on top of the latest one. 409 means someone saved a newer version first.
fn save(id: &str, l: &Loaded, ctx: &Ctx) -> Task<Msg> {
    let (c, id) = (ctx.client.clone(), id.to_string());
    let body = source_text(&l.source);
    let note = l.note.trim().to_string();
    let base = l.summary.latest_version;
    client::task(
        move || {
            let answer = send(&c, "POST", &format!("{}/versions", prompt_path(&id)), Some(&json!({ "body": body, "note": note, "base_version": base })))?;
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
    client::task(move || send(&c, "POST", &path, body.as_ref()).and_then(expect::<Value>).map(|_| done), Msg::Acted)
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
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
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

fn num(n: Option<u32>) -> String {
    n.map_or_else(String::new, |n| n.to_string())
}

// ---- Views ---------------------------------------------------------------------------------------------------------

fn header(l: &Loaded) -> Element<'_, Msg> {
    let s = &l.summary;
    let archived = s.archived_ms.is_some();
    let actions: Vec<Element<'_, Msg>> = vec![
        ui::btn_sm("Back", None, ui::Kind::Ghost, Some(Msg::Back)),
        ui::btn_sm("Save version", None, ui::Kind::Primary, Some(Msg::Save)),
        ui::btn_sm("Download .md", None, ui::Kind::Secondary, Some(Msg::Download)),
        ui::btn_sm("Publish", None, ui::Kind::Secondary, Some(Msg::Publish(s.latest_version))),
        ui::btn_sm("Unpublish", None, ui::Kind::Secondary, s.active_version.map(|_| Msg::Unpublish)),
        ui::btn_sm(if archived { "Restore" } else { "Archive" }, None, ui::Kind::Secondary, Some(Msg::ToggleArchive)),
    ];
    let live = match (s.active_version, s.score) {
        (Some(v), Some(score)) => format!("live v{v} · score {score}"),
        (Some(v), None) => format!("live v{v}"),
        _ => "not published".to_string(),
    };
    let binding = s.binding.clone().unwrap_or_else(|| "no binding".to_string());
    let mut meta: Vec<Element<'_, Msg>> = vec![
        ui::badge(l.kind.as_str(), ui::Tone::Muted),
        ui::faint(binding).into(),
        ui::faint(live).into(),
        ui::badge(&format!("score {}", l.report.score), score_tone(l.report.score)),
    ];
    if dirty(l) {
        meta.push(ui::badge("Unsaved", ui::Tone::Warn));
    }
    if archived {
        meta.push(ui::badge("archived", ui::Tone::Warn));
    }
    let mut parts: Vec<Element<'_, Msg>> = vec![ui::view_head(&s.title, actions), row(meta).spacing(12).align_y(Alignment::Center).into()];
    if !s.description.is_empty() {
        parts.push(ui::faint(s.description.clone()).into());
    }
    if !s.tags.is_empty() {
        parts.push(ui::faint(s.tags.join(" · ")).into());
    }
    column(parts).spacing(8).into()
}

fn finding_row(f: &Finding) -> Element<'static, Msg> {
    let place = if f.line == 0 { "document".to_string() } else { format!("line {}", f.line) };
    row![
        ui::badge(f.severity.as_str(), severity_tone(f.severity)),
        ui::mono(place, 12.0, palette().ink_3),
        ui::prose(f.message.clone()),
        ui::mono(f.rule.clone(), 11.0, palette().ink_3),
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into()
}

fn edit_tab(l: &Loaded) -> Element<'_, Msg> {
    let body = source_text(&l.source);
    let editor = text_editor(&l.source).on_action(Msg::Edit).font(Font::MONOSPACE).height(Length::Fixed(420.0));
    let preview = ui::scroll(ui::markdown::<Msg>(&body)).height(Length::Fixed(420.0));
    let panes = row![container(editor).width(Length::FillPortion(1)), container(preview).width(Length::FillPortion(1))].spacing(16);

    let r = &l.report;
    let mut findings: Vec<Element<'_, Msg>> = Vec::new();
    if r.findings.is_empty() {
        findings.push(ui::faint("No findings: every check passes.").into());
    }
    findings.extend(r.findings.iter().map(finding_row));

    let counts = ui::faint(format!("{} errors · {} warnings · {} infos · score {}", r.errors, r.warnings, r.infos, r.score));
    let stats = ui::faint(format!(
        "{} words · {} lines · {} headings · ~{} tokens",
        r.stats.words, r.stats.lines, r.stats.headings, r.stats.est_tokens
    ));
    column![
        panes,
        text_input("Note for this version", &l.note).on_input(Msg::Note).padding(8).style(crate::theme::input),
        counts,
        stats,
        column(findings).spacing(8),
    ]
    .spacing(12)
    .into()
}

fn versions_tab(l: &Loaded) -> Element<'_, Msg> {
    let mut parts: Vec<Element<'_, Msg>> = Vec::new();
    if let Some(v) = &l.viewing {
        parts.push(viewing(v));
    }
    if let Some(d) = &l.compare {
        parts.push(compare(d));
    }
    let picked = l.picked.len();
    parts.push(
        row![
            ui::faint(format!("{picked} of 2 picked to compare")),
            ui::btn_sm("Compare", None, ui::Kind::Secondary, (picked == 2).then_some(Msg::Compare)),
            ui::btn_sm("Clear", None, ui::Kind::Ghost, (picked > 0).then_some(Msg::ClearPicks)),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into(),
    );
    for v in &l.versions {
        parts.push(ui::hairline());
        parts.push(version_row(l, v));
    }
    column(parts).spacing(12).into()
}

fn version_row<'a>(l: &'a Loaded, v: &'a PromptVersion) -> Element<'a, Msg> {
    let n = v.version;
    let mut marks: Vec<&str> = Vec::new();
    if l.summary.active_version == Some(n) {
        marks.push("live");
    }
    if l.summary.best_version == Some(n) {
        marks.push("best");
    }
    if v.rules_version < apz_prompt::RULES_VERSION {
        marks.push("stale");
    }
    let mut head: Vec<Element<'a, Msg>> = vec![ui::mono(format!("v{n}"), 13.0, palette().ink).into(), ui::badge(&format!("score {}", v.score), score_tone(v.score))];
    for mark in marks {
        head.push(ui::badge(mark, ui::Tone::Signal));
    }
    let mut body: Vec<Element<'a, Msg>> = vec![row(head).spacing(10).align_y(Alignment::Center).into()];
    if !v.note.is_empty() {
        body.push(ui::prose(v.note.clone()).into());
    }
    body.push(
        ui::faint(format!(
            "{} · {} · {} errors, {} warnings, {} infos",
            v.author,
            mcp_hub::ago(now_ms(), v.created_ms),
            v.errors,
            v.warnings,
            v.infos
        ))
        .into(),
    );
    let pick: Element<'a, Msg> = if l.picked.contains(&n) {
        ui::btn_sm("Unpick", None, ui::Kind::Ghost, Some(Msg::Pick(n)))
    } else {
        ui::btn_sm("Compare", None, ui::Kind::Ghost, Some(Msg::Pick(n)))
    };
    body.push(
        row![
            pick,
            ui::btn_sm("View", None, ui::Kind::Ghost, Some(Msg::View(n))),
            ui::btn_sm("Publish", None, ui::Kind::Secondary, Some(Msg::Publish(n))),
            ui::btn_sm("Restore as new", None, ui::Kind::Secondary, Some(Msg::RestoreVersion(n))),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into(),
    );
    column(body).spacing(8).into()
}

fn viewing(v: &PromptVersion) -> Element<'_, Msg> {
    let stale = if v.stale { " · stale" } else { "" };
    let mut parts: Vec<Element<'_, Msg>> = vec![
        row![
            ui::mono(format!("v{} · {} findings{stale}", v.version, v.findings.len()), 12.0, palette().ink_3),
            ui::btn_sm("Close", None, ui::Kind::Ghost, Some(Msg::CloseView)),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into(),
    ];
    parts.extend(v.findings.iter().map(finding_row));
    parts.push(ui::markdown::<Msg>(&v.body));
    column(parts).spacing(10).into()
}

fn compare(d: &PromptDiff) -> Element<'_, Msg> {
    let side = |v: &PromptVersion| {
        ui::mono(
            format!("v{} · score {} · {} errors, {} warnings", v.version, v.score, v.errors, v.warnings),
            12.0,
            palette().ink,
        )
    };
    let mut lines: Vec<Element<'_, Msg>> = Vec::new();
    for line in &d.lines {
        let (mark, colour) = match line.op.as_str() {
            "add" => ("+", ui::Tone::Ok.color()),
            "del" => ("−", ui::Tone::Danger.color()),
            _ => (" ", ui::Tone::Muted.color()),
        };
        lines.push(ui::mono(format!("{:>4} {:>4} {mark} {}", num(line.old), num(line.new), line.text), 12.0, colour).into());
    }
    column![
        row![side(&d.from), side(&d.to)].spacing(24),
        ui::scroll(column(lines)).height(Length::Fixed(360.0)),
    ]
    .spacing(12)
    .into()
}

fn history_tab(l: &Loaded) -> Element<'_, Msg> {
    let Some(events) = l.events.as_ref() else {
        return ui::loading();
    };
    if events.is_empty() {
        return ui::faint("No history yet.").into();
    }
    let now = now_ms();
    let rows: Vec<Element<'_, Msg>> = events
        .iter()
        .map(|e| {
            let version = e.version.map_or_else(String::new, |v| format!("v{v}"));
            row![
                ui::mono(e.action.clone(), 12.0, palette().ink),
                ui::faint(version),
                ui::faint(e.actor.clone()),
                ui::faint(mcp_hub::ago(now, e.at_ms)),
            ]
            .spacing(16)
            .align_y(Alignment::Center)
            .into()
        })
        .collect();
    column(rows).spacing(10).into()
}

/// The rules that apply to this kind, grouped by their usual severity, with the why under each.
fn rules_tab(kind: Kind) -> Element<'static, Msg> {
    let rules = apz_prompt::rules();
    let mut parts: Vec<Element<'static, Msg>> = vec![
        ui::faint(format!("The rules for {} prompts (rules version {}).", kind.as_str(), apz_prompt::RULES_VERSION)).into(),
    ];
    for severity in [Severity::Error, Severity::Warning, Severity::Info] {
        parts.push(ui::label(severity.as_str()).into());
        for rule in rules.iter().filter(|r| r.severity == severity && r.applies_to(kind)) {
            parts.push(
                column![ui::mono(format!("{} · {}", rule.title, rule.id), 13.0, palette().ink), ui::prose(rule.why)]
                    .spacing(4)
                    .into(),
            );
        }
    }
    column(parts).spacing(14).into()
}
