//! The Workstations screen (parity with the web console's `workstations` view): the machines agents
//! operate through the hub. The list enrolls new ones with a join code (its token shown once, with the
//! command to run on the machine) and shows each one's state. A workstation's page shows its facts, its
//! leases (release), its settings (the takeover policy, recording, the work folder), its rename, drain,
//! revoke and audit. The live view and the recordings come in a later pass. Reloads on the screen's
//! poll and on the `station` and `lease_*` bus frames.

use std::collections::HashMap;

use iced::widget::{Space, checkbox, column, container, pick_list, row, text_input};
use iced::{Alignment, Border, Element, Length, Subscription, Task, Theme};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Id;
use super::lab_common::{ago, now_ms};
use crate::console::{Ctx, client, events, ui};
use crate::theme;

/// The longest name, and the longest join-code label, in characters (the hub's limit).
const MAX_NAME_CHARS: usize = 64;
/// The join code lifetimes the screen offers, in minutes; the hub caps a code at 60.
const TTL_CHOICES: [u64; 3] = [15, 30, 60];
/// The bounds the settings form checks before it sends (the hub stores any whole number in them).
const RETENTION_DAYS: (u64, u64) = (1, 3650);
const RECORD_FPS: (u64, u64) = (1, 30);
const WORK_FOLDER_MAX: usize = 512;
/// The audit rows a page asks for.
const AUDIT_ROWS: u64 = 50;

/// What happens when the owner takes over a pixel session (the hub's `Takeover`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Takeover {
    #[default]
    AgentWaits,
    ReleaseLease,
    KeepWorkingPointerPaused,
}

/// One takeover policy as the pick list offers it: its label, and the line the form shows under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TakeoverChoice {
    value: Takeover,
    label: &'static str,
    explain: &'static str,
}

impl std::fmt::Display for TakeoverChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label)
    }
}

const TAKEOVERS: [TakeoverChoice; 3] = [
    TakeoverChoice {
        value: Takeover::AgentWaits,
        label: "Agent waits",
        explain: "When you take over a screen session, the agent waits and resumes when you give the session back.",
    },
    TakeoverChoice {
        value: Takeover::ReleaseLease,
        label: "Release the lease",
        explain: "When you take over, the agent's lease ends and its task goes back to the queue.",
    },
    TakeoverChoice {
        value: Takeover::KeepWorkingPointerPaused,
        label: "Keep working, pause the pointer",
        explain: "When you take over, the agent keeps working through shells, files and the UI tree; only its pointer pauses.",
    },
];

fn takeover_choice(value: Takeover) -> TakeoverChoice {
    TAKEOVERS
        .into_iter()
        .find(|c| c.value == value)
        .unwrap_or(TAKEOVERS[0])
}

/// A workstation's settings, as the hub stores them and the settings PUT sends them.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub takeover: Takeover,
    pub record: bool,
    pub retention_days: u64,
    pub record_fps: u64,
    pub record_takeovers: bool,
    pub work_folder: String,
    pub snapshot_reset: bool,
}

/// What the daemon reports about its machine.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Facts {
    pub os: String,
    pub os_version: String,
    pub arch: String,
    pub hostname: String,
    pub form: String,
    pub cpus: u64,
    pub ram_mb: u64,
    pub gpus: Vec<Gpu>,
    pub displays: Vec<Display>,
    pub account: Account,
    pub apps: u64,
    pub virtual_displays: bool,
    pub ui_tree: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Gpu {
    pub name: String,
    pub vram_mb: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Display {
    pub w: u64,
    pub h: u64,
}

/// The OS account the agents work in: a separate agent account, or the owner's own (`mode` is `owner`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Account {
    pub mode: String,
    pub name: String,
}

/// A workstation (`GET /v1/stations`): its facts, settings and leases.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Station {
    pub id: String,
    pub name: String,
    pub status: String,
    pub draining: bool,
    pub revoked: bool,
    pub version: String,
    pub code: String,
    pub joined_ms: u64,
    pub last_seen_ms: u64,
    pub address: String,
    pub facts: Facts,
    pub settings: Settings,
    pub leases: Vec<Lease>,
}

/// A lease: an agent's hold on a workstation, queued until a place is free.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Lease {
    pub id: String,
    pub agent: u64,
    pub task: Option<String>,
    pub kind: String,
    pub started_ms: u64,
    pub state: String,
    pub position: Option<u64>,
}

/// A join code (`GET /v1/stations/codes`); its token is never listed.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Code {
    pub id: String,
    pub label: String,
    pub created_ms: u64,
    pub expires_ms: u64,
    pub state: String,
    pub used_by: Option<String>,
}

/// A join code as issued (`POST /v1/stations/codes`): the token, shown once.
#[derive(Debug, Clone, Deserialize)]
pub struct Issued {
    pub token: String,
    pub expires_ms: u64,
}

/// One audit row: when, who, which agent (if any), and the event.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct AuditRow {
    pub at_ms: u64,
    pub actor: String,
    pub agent: Option<u64>,
    pub kind: String,
}

/// An agent's CID and name (`GET /v1/agents`), for the leases and the audit.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AgentName {
    pub cid: u64,
    pub name: String,
}

/// The reply to a settings PUT: the stored settings, and whether they reached the workstation.
#[derive(Debug, Clone, Deserialize)]
pub struct SettingsAnswer {
    pub settings: Settings,
    #[serde(default)]
    pub pushed: bool,
    #[serde(default)]
    pub push_error: Option<String>,
}

/// One of the list loads: pending until the first answer, failed when the first request did.
#[derive(Debug, Default)]
pub enum Load<T> {
    #[default]
    Pending,
    Ready(T),
    Failed,
}

/// Takes a reply into a list. A failed refresh keeps what the screen shows; a first failure shows as one.
fn settle_into<T>(slot: &mut Load<T>, result: Result<T, String>) {
    match result {
        Ok(value) => *slot = Load::Ready(value),
        Err(_) if matches!(slot, Load::Pending) => *slot = Load::Failed,
        Err(_) => {}
    }
}

/// The settings form as the owner edits it: numbers as text, so a half-typed value is still a value.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SettingsForm {
    takeover: Takeover,
    record: bool,
    retention_days: String,
    record_fps: String,
    record_takeovers: bool,
    work_folder: String,
    snapshot_reset: bool,
}

impl From<&Settings> for SettingsForm {
    fn from(s: &Settings) -> SettingsForm {
        SettingsForm {
            takeover: s.takeover,
            record: s.record,
            retention_days: s.retention_days.to_string(),
            record_fps: s.record_fps.to_string(),
            record_takeovers: s.record_takeovers,
            work_folder: s.work_folder.clone(),
            snapshot_reset: s.snapshot_reset,
        }
    }
}

/// One edit of the settings form.
#[derive(Debug, Clone)]
pub enum FormEdit {
    Takeover(Takeover),
    Record(bool),
    Retention(String),
    Fps(String),
    RecordTakeovers(bool),
    WorkFolder(String),
    SnapshotReset(bool),
}

/// The dialog that is open, drawn as a card above the lists.
#[derive(Debug, Clone)]
pub enum Dialog {
    /// Add a workstation: the label and how long the code is valid, then the token once issued.
    Add {
        label: String,
        minutes: u64,
        issued: Option<Issued>,
    },
    /// Rename the open workstation; `problem` is the last refusal.
    Rename {
        name: String,
        problem: Option<String>,
    },
    ConfirmRevoke {
        id: String,
        name: String,
    },
    /// Release a lease of the open workstation: the agent's name for the words.
    ConfirmRelease {
        lease: String,
        who: String,
    },
}

#[derive(Debug, Default)]
pub struct State {
    pub stations: Load<Vec<Station>>,
    pub codes: Load<Vec<Code>>,
    /// The agents' names by CID.
    names: HashMap<u64, String>,
    /// The open workstation's audit, for the station it was asked for.
    audit: Load<Vec<AuditRow>>,
    /// The workstation whose page is open; None is the list.
    selected: Option<String>,
    dialog: Option<Dialog>,
    /// The last action's confirmation, and its error.
    notice: Option<String>,
    error: Option<String>,
    /// The settings as the owner edits them, and as the hub last had them: the form is dirty when they differ.
    form: SettingsForm,
    baseline: Option<SettingsForm>,
    form_error: Option<String>,
    saving: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    StationsLoaded(Result<Vec<Station>, String>),
    CodesLoaded(Result<Vec<Code>, String>),
    AgentsLoaded(Result<Vec<AgentName>, String>),
    AuditLoaded(String, Result<Vec<AuditRow>, String>),
    /// A bus frame: the station and lease frames reload the screen.
    Event(events::Event),
    Select(String),
    Deselect,
    OpenAdd,
    LabelEdit(String),
    PickMinutes(u64),
    CreateCode,
    CodeIssued(Result<Issued, String>),
    RevokeCode(String),
    /// Copy a value to the clipboard.
    Copy(String),
    AskRename,
    RenameEdit(String),
    SaveName,
    Renamed(Result<Value, String>),
    AskRevoke,
    Revoke,
    AskRelease(String),
    Release,
    /// Drain (`true`) or resume the open workstation.
    Drain(bool),
    /// An action finished: its result and the line to confirm it with.
    Acted(Result<Value, String>, String),
    Form(FormEdit),
    SaveSettings,
    /// The settings were saved for the workstation named (the page may have moved on).
    SettingsSaved(String, Result<SettingsAnswer, String>),
    CloseDialog,
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// Loads the screen's data (on open, on every poll tick, and on a bus frame): the workstations, the join
/// codes and the agents' names, and the open workstation's audit.
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let (stations, codes, agents) = (ctx.client.clone(), ctx.client.clone(), ctx.client.clone());
    let mut tasks = vec![
        client::task(
            move || stations.get::<Vec<Station>>("/v1/stations"),
            Msg::StationsLoaded,
        ),
        client::task(
            move || codes.get::<Vec<Code>>("/v1/stations/codes"),
            Msg::CodesLoaded,
        ),
        client::task(
            move || agents.get::<Vec<AgentName>>("/v1/agents"),
            Msg::AgentsLoaded,
        ),
    ];
    if let Some(id) = state.selected.clone() {
        let audit = ctx.client.clone();
        let path = format!("/v1/stations/audit?station={id}&limit={AUDIT_ROWS}");
        tasks.push(client::task(
            move || audit.get::<Vec<AuditRow>>(&path),
            move |r| Msg::AuditLoaded(id.clone(), r),
        ));
    }
    Task::batch(tasks)
}

/// The workstation whose page is open, once loaded.
fn current(state: &State) -> Option<&Station> {
    let id = state.selected.as_ref()?;
    match &state.stations {
        Load::Ready(list) => list.iter().find(|s| &s.id == id),
        _ => None,
    }
}

/// Takes the open workstation's settings into the form, unless the owner has edits that a refresh must not take.
fn sync_form(state: &mut State, station: &Station) {
    if state.baseline.as_ref().is_some_and(|b| *b != state.form) {
        return;
    }
    let form = SettingsForm::from(&station.settings);
    state.baseline = Some(form.clone());
    state.form = form;
}

/// Clears a page's dialogs, messages and form when another workstation (or the list) is opened.
fn reset_page(state: &mut State) {
    state.dialog = None;
    state.error = None;
    state.notice = None;
    state.form = SettingsForm::default();
    state.baseline = None;
    state.form_error = None;
    state.saving = false;
    state.audit = Load::Pending;
}

fn apply_edit(form: &mut SettingsForm, edit: FormEdit) {
    match edit {
        FormEdit::Takeover(value) => form.takeover = value,
        FormEdit::Record(on) => form.record = on,
        FormEdit::Retention(text) => form.retention_days = text,
        FormEdit::Fps(text) => form.record_fps = text,
        FormEdit::RecordTakeovers(on) => form.record_takeovers = on,
        FormEdit::WorkFolder(text) => form.work_folder = text,
        FormEdit::SnapshotReset(on) => form.snapshot_reset = on,
    }
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::StationsLoaded(result) => {
            settle_into(&mut state.stations, result);
            if let Some(station) = current(state).cloned() {
                sync_form(state, &station);
            }
            Task::none()
        }
        Msg::CodesLoaded(result) => {
            settle_into(&mut state.codes, result);
            Task::none()
        }
        Msg::AgentsLoaded(Ok(list)) => {
            state.names = list.into_iter().map(|a| (a.cid, a.name)).collect();
            Task::none()
        }
        Msg::AgentsLoaded(Err(_)) => Task::none(),
        Msg::AuditLoaded(id, result) => {
            if state.selected.as_deref() == Some(id.as_str()) {
                settle_into(&mut state.audit, result);
            }
            Task::none()
        }
        Msg::Event(event) => {
            if station_event(&event.kind) {
                load(state, ctx)
            } else {
                Task::none()
            }
        }
        Msg::Select(id) => {
            state.selected = Some(id);
            reset_page(state);
            load(state, ctx)
        }
        Msg::Deselect => {
            state.selected = None;
            reset_page(state);
            Task::none()
        }
        Msg::OpenAdd => {
            state.dialog = Some(Dialog::Add {
                label: String::new(),
                minutes: TTL_CHOICES[1],
                issued: None,
            });
            state.error = None;
            Task::none()
        }
        Msg::LabelEdit(value) => {
            if let Some(Dialog::Add { label, .. }) = &mut state.dialog {
                *label = value.chars().take(MAX_NAME_CHARS).collect();
            }
            Task::none()
        }
        Msg::PickMinutes(choice) => {
            if let Some(Dialog::Add { minutes, .. }) = &mut state.dialog {
                *minutes = choice;
            }
            Task::none()
        }
        Msg::CreateCode => {
            let (label, minutes) = match &state.dialog {
                Some(Dialog::Add {
                    label,
                    minutes,
                    issued: None,
                }) => (label.trim().to_string(), *minutes),
                _ => return Task::none(),
            };
            let body = json!({ "label": label, "ttl_minutes": minutes });
            let client = ctx.client.clone();
            client::task(
                move || client.post::<Issued>("/v1/stations/codes", &body),
                Msg::CodeIssued,
            )
        }
        Msg::CodeIssued(Ok(made)) => {
            state.error = None;
            if let Some(Dialog::Add { issued, .. }) = &mut state.dialog {
                *issued = Some(made);
            }
            load(state, ctx)
        }
        Msg::CodeIssued(Err(error)) => {
            state.error = Some(error);
            Task::none()
        }
        Msg::RevokeCode(id) => {
            let path = format!("/v1/stations/codes/{id}");
            let client = ctx.client.clone();
            client::task(
                move || client.call("DELETE", &path, None),
                |r| Msg::Acted(r, "Join code revoked".to_string()),
            )
        }
        Msg::Copy(value) => {
            state.notice = Some("Copied".to_string());
            iced::clipboard::write(value)
        }
        Msg::AskRename => {
            let Some(name) = current(state).map(|s| s.name.clone()) else {
                return Task::none();
            };
            state.dialog = Some(Dialog::Rename {
                name,
                problem: None,
            });
            state.error = None;
            Task::none()
        }
        Msg::RenameEdit(value) => {
            if let Some(Dialog::Rename { name, problem }) = &mut state.dialog {
                *name = value;
                *problem = None;
            }
            Task::none()
        }
        Msg::SaveName => {
            let Some(Dialog::Rename { name, .. }) = &state.dialog else {
                return Task::none();
            };
            let (name, Some(id)) = (name.trim().to_string(), state.selected.clone()) else {
                return Task::none();
            };
            if let Some(problem) = name_problem(&name) {
                if let Some(Dialog::Rename { problem: slot, .. }) = &mut state.dialog {
                    *slot = Some(problem);
                }
                return Task::none();
            }
            let path = format!("/v1/stations/{id}");
            let body = json!({ "name": name });
            let client = ctx.client.clone();
            client::task(
                move || client.call("PATCH", &path, Some(&body)),
                Msg::Renamed,
            )
        }
        Msg::Renamed(Ok(_)) => {
            state.dialog = None;
            state.error = None;
            state.notice = Some("Renamed".to_string());
            load(state, ctx)
        }
        Msg::Renamed(Err(error)) => {
            if let Some(Dialog::Rename { problem, .. }) = &mut state.dialog {
                *problem = Some(error);
            }
            Task::none()
        }
        Msg::AskRevoke => {
            let Some((id, name)) = current(state).map(|s| (s.id.clone(), s.name.clone())) else {
                return Task::none();
            };
            state.dialog = Some(Dialog::ConfirmRevoke { id, name });
            state.error = None;
            Task::none()
        }
        Msg::Revoke => {
            let Some(Dialog::ConfirmRevoke { id, name }) = state.dialog.clone() else {
                return Task::none();
            };
            state.dialog = None;
            let path = format!("/v1/stations/{id}/revoke");
            let notice = format!("{name} revoked");
            let client = ctx.client.clone();
            client::task(
                move || client.call("POST", &path, None),
                move |r| Msg::Acted(r, notice.clone()),
            )
        }
        Msg::AskRelease(lease) => {
            let Some(station) = current(state) else {
                return Task::none();
            };
            let Some(found) = station.leases.iter().find(|l| l.id == lease) else {
                return Task::none();
            };
            let who = agent_name(state, found.agent);
            state.dialog = Some(Dialog::ConfirmRelease { lease, who });
            state.error = None;
            Task::none()
        }
        Msg::Release => {
            let (Some(Dialog::ConfirmRelease { lease, .. }), Some(id)) =
                (state.dialog.clone(), state.selected.clone())
            else {
                return Task::none();
            };
            state.dialog = None;
            let path = format!("/v1/stations/{id}/leases/{lease}");
            let client = ctx.client.clone();
            client::task(
                move || client.call("DELETE", &path, None),
                |r| Msg::Acted(r, "Lease released".to_string()),
            )
        }
        Msg::Drain(on) => {
            let Some((id, name)) = current(state).map(|s| (s.id.clone(), s.name.clone())) else {
                return Task::none();
            };
            let (verb, notice) = if on {
                ("drain", format!("{name} is draining"))
            } else {
                ("undrain", format!("{name} is back in service"))
            };
            let path = format!("/v1/stations/{id}/{verb}");
            let client = ctx.client.clone();
            client::task(
                move || client.call("POST", &path, None),
                move |r| Msg::Acted(r, notice.clone()),
            )
        }
        Msg::Acted(result, notice) => {
            match result {
                Ok(_) => {
                    state.error = None;
                    state.notice = Some(notice);
                }
                Err(error) => {
                    state.notice = None;
                    state.error = Some(error);
                }
            }
            load(state, ctx)
        }
        Msg::Form(edit) => {
            state.form_error = None;
            apply_edit(&mut state.form, edit);
            Task::none()
        }
        Msg::SaveSettings => {
            if state.saving {
                return Task::none();
            }
            let Some(id) = state.selected.clone() else {
                return Task::none();
            };
            let settings = match settings_of(&state.form) {
                Ok(settings) => settings,
                Err(error) => {
                    state.form_error = Some(error);
                    return Task::none();
                }
            };
            state.form_error = None;
            state.saving = true;
            let body = serde_json::to_value(&settings).unwrap_or_default();
            let path = format!("/v1/stations/{id}/settings");
            let client = ctx.client.clone();
            let saved = id.clone();
            client::task(
                move || {
                    client.call("PUT", &path, Some(&body)).and_then(|v| {
                        serde_json::from_value::<SettingsAnswer>(v).map_err(|e| e.to_string())
                    })
                },
                move |r| Msg::SettingsSaved(saved.clone(), r),
            )
        }
        Msg::SettingsSaved(id, result) => {
            if state.selected.as_deref() != Some(id.as_str()) {
                return Task::none();
            }
            state.saving = false;
            match result {
                Ok(answer) => {
                    let form = SettingsForm::from(&answer.settings);
                    state.baseline = Some(form.clone());
                    state.form = form;
                    state.error = None;
                    if answer.pushed {
                        state.notice =
                            Some("Settings saved and sent to the workstation".to_string());
                    } else if let Some(problem) = answer.push_error {
                        state.notice = None;
                        state.error = Some(format!("Saved. Not sent yet: {problem}"));
                    } else {
                        state.notice =
                            Some("Saved. The workstation gets them when it connects.".to_string());
                    }
                }
                Err(error) => state.form_error = Some(error),
            }
            load(state, ctx)
        }
        Msg::CloseDialog => {
            state.dialog = None;
            state.error = None;
            Task::none()
        }
    }
}

/// The bus frames the screen reloads on: a workstation changed, or one of its leases did.
pub fn station_event(kind: &str) -> bool {
    kind == "station" || kind.starts_with("lease_")
}

/// The screen's bus subscription: the hub's events, which `update` filters.
pub fn subscription(ctx: &Ctx) -> Subscription<Msg> {
    events::subscription(&ctx.client).map(Msg::Event)
}

pub fn view(state: &State) -> Element<'_, Msg> {
    let mut parts: Vec<Element<'_, Msg>> = Vec::new();
    if let Some(error) = &state.error {
        parts.push(ui::error_line(error.as_str()));
    }
    if let Some(notice) = &state.notice {
        parts.push(ui::faint(notice.as_str()).into());
    }
    if let Some(dialog) = &state.dialog {
        parts.push(dialog_view(dialog));
    }
    match (&state.selected, &state.stations) {
        (None, _) => list_page(state, parts),
        (Some(id), Load::Ready(list)) => match list.iter().find(|s| &s.id == id) {
            Some(station) => station_page(state, station, parts),
            None => bare_page(parts, ui::empty("alert", "No such workstation.", "")),
        },
        (Some(_), Load::Failed) => bare_page(
            parts,
            ui::empty("alert", "Could not load this workstation.", ""),
        ),
        (Some(_), Load::Pending) => bare_page(parts, ui::loading()),
    }
}

// ---- The list: the workstations and the join codes ----

fn list_page<'a>(state: &'a State, mut parts: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let head = ui::view_head_with_intro(
        "Workstations",
        "Machines that agents operate through the hub: their screen, shell, files and every program installed there.",
        vec![ui::btn(
            "Add workstation",
            Some("plus"),
            ui::Kind::Primary,
            Some(Msg::OpenAdd),
        )],
    );
    parts.push(ui::card("Workstations", stations_view(state)));
    parts.push(ui::card("Join codes", codes_view(state)));
    ui::sheet(head, column(parts).spacing(22).width(Length::Fill).into())
}

fn stations_view(state: &State) -> Element<'_, Msg> {
    match &state.stations {
        Load::Pending => ui::loading(),
        Load::Failed => ui::empty("alert", "Could not load the workstations.", ""),
        Load::Ready(list) if list.is_empty() => ui::empty(
            "machine",
            "No workstations yet",
            "Add one here, then run the join command on the machine.",
        ),
        Load::Ready(list) => {
            let mut sorted: Vec<&Station> = list.iter().collect();
            sorted.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            table(
                &[
                    (3, "Name"),
                    (3, "GPUs"),
                    (2, "State"),
                    (2, "Leases"),
                    (2, "Last seen"),
                ],
                sorted.into_iter().map(station_row).collect(),
            )
        }
    }
}

fn station_row(s: &Station) -> Element<'_, Msg> {
    let (status, tone) = status_chip(&s.status);
    let (holding, waiting) = lease_counts(&s.leases);
    let mut badges: Vec<Element<'_, Msg>> = vec![ui::badge(&status, tone)];
    if let Some(text) = account_badge(&s.facts.account) {
        badges.push(ui::badge(&text, ui::Tone::Warn));
    }
    let gpus: Element<'_, Msg> = if s.facts.gpus.is_empty() {
        ui::faint("no GPU").into()
    } else {
        column(
            s.facts
                .gpus
                .iter()
                .map(|g| mono_text(gpu_line(g)))
                .collect::<Vec<_>>(),
        )
        .spacing(4)
        .into()
    };
    cells(vec![
        (
            3,
            column![
                ui::link(&s.name, Msg::Select(s.id.clone()), false),
                ui::faint(station_summary(&s.facts))
            ]
            .spacing(2)
            .into(),
        ),
        (3, gpus),
        (2, column(badges).spacing(6).into()),
        (
            2,
            ui::faint(format!("{holding} active · {waiting} queued")).into(),
        ),
        (2, ui::faint(ago(s.last_seen_ms)).into()),
    ])
}

fn codes_view(state: &State) -> Element<'_, Msg> {
    match &state.codes {
        Load::Pending => ui::loading(),
        Load::Failed => ui::empty("alert", "Could not load the join codes.", ""),
        Load::Ready(list) if list.is_empty() => ui::empty(
            "link",
            "No join codes yet",
            "Codes made with \"Add workstation\" are listed here.",
        ),
        Load::Ready(list) => {
            let mut sorted: Vec<&Code> = list.iter().collect();
            sorted.sort_by(|a, b| b.created_ms.cmp(&a.created_ms));
            table(
                &[
                    (3, "Label"),
                    (2, "ID"),
                    (2, "Created"),
                    (2, "Expires"),
                    (3, "State"),
                    (1, ""),
                ],
                sorted.into_iter().map(code_row).collect(),
            )
        }
    }
}

fn code_row(c: &Code) -> Element<'_, Msg> {
    let (label, tone) = code_chip(&c.state);
    let state_cell: Element<'_, Msg> = match &c.used_by {
        Some(by) => row![ui::badge(&label, tone), ui::faint(format!(" by {by}"))]
            .spacing(6)
            .align_y(Alignment::Center)
            .into(),
        None => ui::badge(&label, tone),
    };
    let active = c.state == "active";
    let expires = if active {
        format!("in {}", countdown(left_ms(c.expires_ms)))
    } else {
        ago(c.expires_ms)
    };
    let action: Element<'_, Msg> = if active {
        ui::btn_sm(
            "Revoke",
            None,
            ui::Kind::Danger,
            Some(Msg::RevokeCode(c.id.clone())),
        )
    } else {
        Space::new().width(Length::Fill).into()
    };
    let label_text = if c.label.is_empty() {
        "no label".to_string()
    } else {
        c.label.clone()
    };
    cells(vec![
        (3, mono_text(label_text)),
        (
            2,
            ui::mono(c.id.clone(), 11.0, theme::palette().ink_3).into(),
        ),
        (2, ui::faint(ago(c.created_ms)).into()),
        (2, mono_text(expires)),
        (3, state_cell),
        (1, action),
    ])
}

// ---- One workstation's page ----

fn station_page<'a>(
    state: &'a State,
    s: &'a Station,
    mut parts: Vec<Element<'a, Msg>>,
) -> Element<'a, Msg> {
    let head = ui::view_head_with_intro(&s.name, &station_summary(&s.facts), station_actions(s));
    let mut chips: Vec<Element<'a, Msg>> = vec![];
    let (status, tone) = status_chip(&s.status);
    chips.push(ui::badge(&status, tone));
    if let Some(text) = account_badge(&s.facts.account) {
        chips.push(ui::badge(&text, ui::Tone::Warn));
    }
    parts.insert(0, ui::link("All workstations", Msg::Deselect, false));
    parts.push(ui::chip_row(chips));
    parts.push(ui::card("Facts", facts_view(s)));
    parts.push(ui::card("Leases", leases_view(state, s)));
    parts.push(ui::card("Settings", settings_view(state, s)));
    parts.push(ui::card("Audit", audit_view(state)));
    ui::sheet(head, column(parts).spacing(16).width(Length::Fill).into())
}

/// A page with no workstation to show: the back link, the notices, then the body.
fn bare_page<'a>(mut parts: Vec<Element<'a, Msg>>, body: Element<'a, Msg>) -> Element<'a, Msg> {
    parts.insert(0, ui::link("All workstations", Msg::Deselect, false));
    parts.push(body);
    ui::sheet(
        ui::view_head("Workstation", Vec::new()),
        column(parts).spacing(16).width(Length::Fill).into(),
    )
}

fn station_actions(s: &Station) -> Vec<Element<'_, Msg>> {
    if s.revoked {
        return vec![ui::faint("Revoked: its link is closed.").into()];
    }
    vec![
        ui::btn("Rename", None, ui::Kind::Secondary, Some(Msg::AskRename)),
        ui::btn(
            if s.draining { "Resume" } else { "Drain" },
            None,
            ui::Kind::Secondary,
            Some(Msg::Drain(!s.draining)),
        ),
        ui::btn("Revoke", None, ui::Kind::Danger, Some(Msg::AskRevoke)),
    ]
}

fn facts_view(s: &Station) -> Element<'_, Msg> {
    let f = &s.facts;
    let gpus: Element<'_, Msg> = if f.gpus.is_empty() {
        mono_text("none")
    } else {
        column(
            f.gpus
                .iter()
                .map(|g| mono_text(gpu_line(g)))
                .collect::<Vec<_>>(),
        )
        .spacing(4)
        .into()
    };
    let system = [os_text(f), f.arch.clone()]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let rows: Vec<(&str, Element<'_, Msg>)> = vec![
        ("System", mono_text(dash(&system))),
        ("Machine", mono_text(dash(&form_label(&f.form)))),
        ("Hostname", mono_text(dash(&f.hostname))),
        (
            "CPUs",
            mono_text(if f.cpus > 0 {
                format!("{} cores", f.cpus)
            } else {
                dash("")
            }),
        ),
        ("Memory", mono_text(ram_text(f.ram_mb))),
        ("GPUs", gpus),
        ("Displays", mono_text(displays_text(&f.displays))),
        ("Account", mono_text(account_text(&f.account))),
        ("Apps found", mono_text(f.apps.to_string())),
        ("Virtual displays", mono_text(yes_no(f.virtual_displays))),
        ("UI tree", mono_text(yes_no(f.ui_tree))),
        ("Version", mono_text(dash(&s.version))),
        ("Address", mono_text(dash(&s.address))),
        ("Joined", mono_text(ago(s.joined_ms))),
        ("Last seen", mono_text(ago(s.last_seen_ms))),
        ("Join code", mono_text(dash(&s.code))),
    ];
    column(
        rows.into_iter()
            .map(|(name, value)| fact(name, value))
            .collect::<Vec<_>>(),
    )
    .spacing(12)
    .into()
}

fn leases_view<'a>(state: &'a State, s: &'a Station) -> Element<'a, Msg> {
    if s.leases.is_empty() {
        return ui::empty(
            "machine",
            "No leases",
            "An agent holds a lease while it works here. Leases show up as agents start.",
        );
    }
    let rows: Vec<Element<'a, Msg>> = s
        .leases
        .iter()
        .map(|l| {
            let (label, tone) = lease_chip(l);
            let task = match &l.task {
                Some(task) if !task.is_empty() => mono_text(task.clone()),
                _ => ui::faint("—").into(),
            };
            cells(vec![
                (3, mono_text(agent_name(state, l.agent))),
                (3, task),
                (2, ui::faint(lease_kind_label(&l.kind)).into()),
                (2, ui::badge(&label, tone)),
                (2, ui::faint(ago(l.started_ms)).into()),
                (
                    2,
                    ui::btn_sm(
                        "Release",
                        None,
                        ui::Kind::Danger,
                        Some(Msg::AskRelease(l.id.clone())),
                    ),
                ),
            ])
        })
        .collect();
    table(
        &[
            (3, "Agent"),
            (3, "Task"),
            (2, "Kind"),
            (2, "State"),
            (2, "Started"),
            (2, ""),
        ],
        rows,
    )
}

fn settings_view<'a>(state: &'a State, s: &'a Station) -> Element<'a, Msg> {
    let live = !s.revoked;
    let f = &state.form;
    let choice = takeover_choice(f.takeover);

    let policy = pick_list(TAKEOVERS.to_vec(), Some(choice), |c: TakeoverChoice| {
        Msg::Form(FormEdit::Takeover(c.value))
    })
    .style(theme::pick_list)
    .text_size(13.0)
    .width(Length::Fixed(300.0));

    let mut record = checkbox(f.record)
        .label("Record screen sessions")
        .text_size(13.0);
    if live {
        record = record.on_toggle(|on| Msg::Form(FormEdit::Record(on)));
    }
    let mut takeovers = checkbox(f.record_takeovers)
        .label("Record the owner's takeovers too")
        .text_size(13.0);
    if live {
        takeovers = takeovers.on_toggle(|on| Msg::Form(FormEdit::RecordTakeovers(on)));
    }
    let mut snapshot = checkbox(f.snapshot_reset)
        .label("Snapshot and reset (VMs)")
        .text_size(13.0);
    if live {
        snapshot = snapshot.on_toggle(|on| Msg::Form(FormEdit::SnapshotReset(on)));
    }

    let mut retention = text_input("", &f.retention_days)
        .font(theme::MONO)
        .style(theme::input)
        .padding(8.0)
        .width(Length::Fixed(120.0));
    if live {
        retention = retention.on_input(|t| Msg::Form(FormEdit::Retention(t)));
    }
    let mut fps = text_input("", &f.record_fps)
        .font(theme::MONO)
        .style(theme::input)
        .padding(8.0)
        .width(Length::Fixed(120.0));
    if live {
        fps = fps.on_input(|t| Msg::Form(FormEdit::Fps(t)));
    }
    let mut folder = text_input("~/AgentPlaza", &f.work_folder)
        .font(theme::MONO)
        .style(theme::input)
        .padding(8.0);
    if live {
        folder = folder.on_input(|t| Msg::Form(FormEdit::WorkFolder(t)));
    }

    let save = ui::btn(
        if state.saving {
            "Saving…"
        } else {
            "Save settings"
        },
        None,
        ui::Kind::Primary,
        (live && !state.saving).then_some(Msg::SaveSettings),
    );
    let mut body: Vec<Element<'a, Msg>> = vec![
        field("Takeover policy", policy.into(), takeover_text(f.takeover)),
        row![record, takeovers]
            .spacing(24)
            .align_y(Alignment::Center)
            .into(),
        row![
            field(
                "Keep recordings for (days)",
                retention.into(),
                &format!(
                    "{} to {}. Older recordings are deleted.",
                    RETENTION_DAYS.0, RETENTION_DAYS.1
                ),
            ),
            field(
                "Frames per second",
                fps.into(),
                &format!(
                    "{} to {}. What a recording keeps.",
                    RECORD_FPS.0, RECORD_FPS.1
                ),
            ),
        ]
        .spacing(16)
        .into(),
        field(
            "Work folder",
            folder.into(),
            "Under the agent's OS account. It syncs with the hub both ways.",
        ),
        snapshot.into(),
        row![save].into(),
    ];
    if let Some(error) = &state.form_error {
        body.push(ui::error_line(error.as_str()));
    }
    column(body).spacing(14).width(Length::Fill).into()
}

fn audit_view(state: &State) -> Element<'_, Msg> {
    match &state.audit {
        Load::Pending => ui::loading(),
        Load::Failed => ui::empty("alert", "Could not load the audit.", ""),
        Load::Ready(rows) if rows.is_empty() => ui::empty("clock", "No events yet", ""),
        Load::Ready(rows) => table(
            &[(2, "When"), (3, "Event"), (3, "Agent"), (2, "By")],
            rows.iter()
                .map(|r| {
                    let agent = r
                        .agent
                        .map(|cid| agent_name(state, cid))
                        .unwrap_or_else(|| "—".to_string());
                    cells(vec![
                        (2, ui::faint(ago(r.at_ms)).into()),
                        (3, mono_text(audit_label(&r.kind))),
                        (3, mono_text(agent)),
                        (2, mono_text(r.actor.clone())),
                    ])
                })
                .collect(),
        ),
    }
}

// ---- Dialogs ----

fn dialog_view(dialog: &Dialog) -> Element<'_, Msg> {
    match dialog {
        Dialog::Add {
            label,
            minutes,
            issued: None,
        } => ui::card("Add a workstation", add_form(label, *minutes)),
        Dialog::Add {
            issued: Some(made), ..
        } => ui::card("Add a workstation", issued_view(made)),
        Dialog::Rename { name, problem } => {
            ui::card("Rename workstation", rename_form(name, problem.as_deref()))
        }
        Dialog::ConfirmRevoke { name, .. } => ui::card(
            &format!("Revoke {name}?"),
            confirm(
                "Its link closes at once and its leases end. This cannot be undone.",
                "Revoke",
                Msg::Revoke,
            ),
        ),
        Dialog::ConfirmRelease { who, .. } => ui::card(
            "Release this lease?",
            confirm(
                &format!(
                    "{who} gives the machine back: its session here ends, or its place in the queue is given up."
                ),
                "Release",
                Msg::Release,
            ),
        ),
    }
}

/// A line of text with a Cancel and a danger action under it.
fn confirm<'a>(text: &str, action: &str, run: Msg) -> Element<'a, Msg> {
    column![
        ui::faint(text.to_string()),
        row![
            ui::btn("Cancel", None, ui::Kind::Secondary, Some(Msg::CloseDialog)),
            ui::btn(action, None, ui::Kind::Danger, Some(run)),
        ]
        .spacing(8),
    ]
    .spacing(12)
    .into()
}

/// The label and validity of a new join code.
fn add_form(label: &str, minutes: u64) -> Element<'_, Msg> {
    let choice = |m: u64| {
        let kind = if m == minutes {
            ui::Kind::Primary
        } else {
            ui::Kind::Secondary
        };
        ui::btn(
            &format!("{m} minutes"),
            None,
            kind,
            Some(Msg::PickMinutes(m)),
        )
    };
    column![
        field(
            "Label",
            text_input("studio-mac", label)
                .font(theme::MONO)
                .style(theme::input)
                .padding(8.0)
                .on_input(Msg::LabelEdit)
                .on_submit(Msg::CreateCode)
                .into(),
            "What the machine is called in this list until it names itself. Optional.",
        ),
        ui::label("Valid for"),
        row![
            choice(TTL_CHOICES[0]),
            choice(TTL_CHOICES[1]),
            choice(TTL_CHOICES[2])
        ]
        .spacing(8),
        ui::faint("How long the code can enroll a machine. The hub caps it at 60 minutes."),
        ui::btn(
            "Create join code",
            None,
            ui::Kind::Primary,
            Some(Msg::CreateCode)
        ),
    ]
    .spacing(10)
    .into()
}

/// The token just issued, shown once, with the command to run on the machine.
fn issued_view(made: &Issued) -> Element<'_, Msg> {
    column![
        ui::faint("Shown once. Single use. Anyone with it can enroll a machine until it expires.")
            .color(theme::palette().warn),
        copy_box(&made.token),
        row![
            ui::faint("Expires in "),
            ui::mono(
                countdown(left_ms(made.expires_ms)),
                12.0,
                theme::palette().ink
            ),
        ]
        .spacing(4)
        .align_y(Alignment::Center),
        ui::faint("On the machine, run:"),
        copy_box(&join_command(&made.token)),
        ui::faint("The machine appears in the list once it joins."),
        ui::btn("Close", None, ui::Kind::Secondary, Some(Msg::CloseDialog)),
    ]
    .spacing(10)
    .into()
}

fn rename_form<'a>(name: &'a str, problem: Option<&'a str>) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = vec![field(
        "Name",
        text_input("", name)
            .font(theme::MONO)
            .style(theme::input)
            .padding(8.0)
            .on_input(Msg::RenameEdit)
            .on_submit(Msg::SaveName)
            .into(),
        "Shown on the list and in the audit.",
    )];
    if let Some(problem) = problem {
        parts.push(ui::error_line(problem));
    }
    parts.push(
        row![
            ui::btn("Save name", None, ui::Kind::Primary, Some(Msg::SaveName)),
            ui::btn("Cancel", None, ui::Kind::Secondary, Some(Msg::CloseDialog)),
        ]
        .spacing(8)
        .into(),
    );
    column(parts).spacing(10).into()
}

// ---- Shared pieces ----

/// A value to copy: a mono box with a Copy button.
fn copy_box<'a>(value: &str) -> Element<'a, Msg> {
    container(
        row![
            ui::mono(value.to_string(), 12.0, theme::palette().ink).width(Length::Fill),
            ui::btn(
                "Copy",
                None,
                ui::Kind::Ghost,
                Some(Msg::Copy(value.to_string()))
            ),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .padding([8, 12])
    .width(Length::Fill)
    .style(boxed)
    .into()
}

/// A labelled field: its label, its control, and a hint under it.
fn field<'a>(label: &str, control: Element<'a, Msg>, hint: &str) -> Element<'a, Msg> {
    column![ui::label(label), control, ui::faint(hint.to_string())]
        .spacing(4)
        .into()
}

/// A labelled line of a card's facts.
fn fact<'a>(name: &str, value: Element<'a, Msg>) -> Element<'a, Msg> {
    row![
        container(ui::label(name)).width(Length::Fixed(150.0)),
        value
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into()
}

/// A mono value in ink, for facts and cells.
fn mono_text<'a>(value: impl Into<String>) -> Element<'a, Msg> {
    ui::mono(value.into(), 12.0, theme::palette().ink).into()
}

/// A table: a mono header row, then the rows, each under a hairline. Columns are proportional.
fn table<'a>(head: &[(u16, &str)], rows: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let heads: Vec<(u16, Element<'a, Msg>)> = head
        .iter()
        .map(|(p, l)| (*p, ui::label(l).into()))
        .collect();
    let mut lines: Vec<Element<'a, Msg>> = vec![cells(heads), ui::hairline()];
    for r in rows {
        lines.push(r);
        lines.push(ui::hairline());
    }
    column(lines).width(Length::Fill).into()
}

/// One table row: its cells in proportional columns.
fn cells<'a>(parts: Vec<(u16, Element<'a, Msg>)>) -> Element<'a, Msg> {
    let laid: Vec<Element<'a, Msg>> = parts
        .into_iter()
        .map(|(portion, cell)| container(cell).width(Length::FillPortion(portion)).into())
        .collect();
    row(laid).spacing(12).align_y(Alignment::Center).into()
}

/// A card's frame: a 1px rule border, square corners.
fn boxed(_: &Theme) -> container::Style {
    container::Style {
        border: Border {
            color: theme::palette().rule,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

/// The agent's name for a CID, or "agent N" when the list has none.
fn agent_name(state: &State, cid: u64) -> String {
    state
        .names
        .get(&cid)
        .cloned()
        .unwrap_or_else(|| format!("agent {cid}"))
}

// ---- View-model helpers (pure, tested below) ----

/// The status chip: the state the hub reports for a workstation.
pub fn status_chip(status: &str) -> (String, ui::Tone) {
    let tone = match status {
        "online" => ui::Tone::Ok,
        "draining" => ui::Tone::Warn,
        "revoked" => ui::Tone::Danger,
        _ => ui::Tone::Muted,
    };
    (status.to_string(), tone)
}

/// The account badge: the agents act as the owner, so it is flagged. None for a separate agent account.
pub fn account_badge(account: &Account) -> Option<String> {
    (account.mode == "owner").then(|| format!("agents act as {}", account.name))
}

/// The account in words, for the facts.
pub fn account_text(account: &Account) -> String {
    if account.mode == "owner" {
        format!(
            "the owner's own account ({}): agents act as the owner",
            account.name
        )
    } else {
        format!("a separate agent account ({})", account.name)
    }
}

/// An OS's name, then its version. The hub reports `macos`, `windows` or `linux`.
pub fn os_text(facts: &Facts) -> String {
    let name = match facts.os.as_str() {
        "macos" => "macOS",
        "windows" => "Windows",
        "linux" => "Linux",
        other => other,
    };
    [name, facts.os_version.as_str()]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The kind of machine in words.
pub fn form_label(form: &str) -> String {
    let label = match form {
        "laptop" => "Laptop",
        "desktop" => "Desktop",
        "mini" => "Mini",
        "server" => "Server",
        "vm" => "VM",
        other => other,
    };
    label.to_string()
}

/// Memory in gigabytes, one decimal under 10 GB; a dash for none.
pub fn ram_text(ram_mb: u64) -> String {
    if ram_mb == 0 {
        return "—".to_string();
    }
    let gib = ram_mb as f64 / 1024.0;
    if gib >= 10.0 {
        format!("{} GB RAM", gib.round() as u64)
    } else {
        format!("{gib:.1} GB RAM")
    }
}

/// A GPU's VRAM: gigabytes from 1024 MB, else megabytes.
fn vram_text(mb: u64) -> String {
    if mb >= 1024 {
        format!("{} GB", (mb as f64 / 1024.0).round() as u64)
    } else {
        format!("{mb} MB")
    }
}

/// One GPU in words: its name, then its memory (a GPU that shares system memory reports 0).
pub fn gpu_line(gpu: &Gpu) -> String {
    if gpu.vram_mb > 0 {
        format!("{} · {}", gpu.name, vram_text(gpu.vram_mb))
    } else {
        format!("{} · shared memory", gpu.name)
    }
}

/// The displays' sizes in points, or "none".
pub fn displays_text(displays: &[Display]) -> String {
    if displays.is_empty() {
        return "none".to_string();
    }
    displays
        .iter()
        .map(|d| format!("{}×{}", d.w, d.h))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A row's one-line summary: the OS, the kind of machine, the memory and the GPUs.
pub fn station_summary(facts: &Facts) -> String {
    let gpus = if facts.gpus.is_empty() {
        "no GPU".to_string()
    } else {
        facts
            .gpus
            .iter()
            .map(|g| g.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let form = form_label(&facts.form);
    [
        os_text(facts),
        if form.is_empty() { String::new() } else { form },
        if facts.ram_mb > 0 {
            ram_text(facts.ram_mb)
        } else {
            String::new()
        },
        gpus,
    ]
    .into_iter()
    .filter(|p| !p.is_empty())
    .collect::<Vec<_>>()
    .join(" · ")
}

/// Holding and waiting leases: those queued wait, the rest hold a place.
pub fn lease_counts(leases: &[Lease]) -> (usize, usize) {
    let waiting = leases.iter().filter(|l| l.state == "queued").count();
    (leases.len() - waiting, waiting)
}

/// A lease's state as a chip; a queued lease shows its place in the queue.
pub fn lease_chip(lease: &Lease) -> (String, ui::Tone) {
    match lease.state.as_str() {
        "queued" => match lease.position {
            Some(place) => (format!("queued #{place}"), ui::Tone::Warn),
            None => ("queued".to_string(), ui::Tone::Warn),
        },
        "opening" => ("opening".to_string(), ui::Tone::Info),
        _ => ("active".to_string(), ui::Tone::Ok),
    }
}

pub fn lease_kind_label(kind: &str) -> &'static str {
    if kind == "pixel" {
        "Pixel session"
    } else {
        "Shared"
    }
}

/// A join code's state as a chip.
pub fn code_chip(state: &str) -> (String, ui::Tone) {
    let (label, tone) = match state {
        "active" => ("Active", ui::Tone::Ok),
        "used" => ("Used", ui::Tone::Info),
        "expired" => ("Expired", ui::Tone::Muted),
        "revoked" => ("Revoked", ui::Tone::Danger),
        other => return (other.to_string(), ui::Tone::Muted),
    };
    (label.to_string(), tone)
}

/// The takeover policy's explanation, as the form shows it under the pick list.
pub fn takeover_text(value: Takeover) -> &'static str {
    takeover_choice(value).explain
}

/// An audit row's event in words; an unknown kind shows as its words.
pub fn audit_label(kind: &str) -> String {
    match kind {
        "join" => "Enrolled".to_string(),
        "join_refused" => "Join refused".to_string(),
        "code_revoke" => "Join code revoked".to_string(),
        "rename" => "Renamed".to_string(),
        "settings" => "Settings changed".to_string(),
        "drain" => "Draining".to_string(),
        "undrain" => "Back in service".to_string(),
        "revoke" => "Revoked".to_string(),
        "lease_granted" => "Lease granted".to_string(),
        "lease_failed" => "Lease failed".to_string(),
        "lease_ended" => "Lease ended".to_string(),
        "lease_released" => "Lease released".to_string(),
        other => other.replace('_', " "),
    }
}

/// The settings form's values as settings to send, or the first problem to show. The folder is trimmed.
pub fn settings_of(f: &SettingsForm) -> Result<Settings, String> {
    let days = whole_in(&f.retention_days, RETENTION_DAYS).ok_or_else(|| {
        format!(
            "Keep recordings for whole days, from {} to {}.",
            RETENTION_DAYS.0, RETENTION_DAYS.1
        )
    })?;
    let fps = whole_in(&f.record_fps, RECORD_FPS).ok_or_else(|| {
        format!(
            "Record whole frames per second, from {} to {}.",
            RECORD_FPS.0, RECORD_FPS.1
        )
    })?;
    let folder = f.work_folder.trim();
    if folder.is_empty() {
        return Err("The work folder cannot be empty.".to_string());
    }
    if folder.chars().count() > WORK_FOLDER_MAX || has_control(folder) {
        return Err(format!(
            "The work folder is a path of up to {WORK_FOLDER_MAX} characters."
        ));
    }
    Ok(Settings {
        takeover: f.takeover,
        record: f.record,
        retention_days: days,
        record_fps: fps,
        record_takeovers: f.record_takeovers,
        work_folder: folder.to_string(),
        snapshot_reset: f.snapshot_reset,
    })
}

/// A whole number from `text` within `(lo, hi)`, or None.
fn whole_in(text: &str, (lo, hi): (u64, u64)) -> Option<u64> {
    let t = text.trim();
    if t.is_empty() || !t.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: u64 = t.parse().ok()?;
    (lo..=hi).contains(&n).then_some(n)
}

fn has_control(text: &str) -> bool {
    text.chars().any(|c| (c as u32) < 32 || c as u32 == 127)
}

/// Why a name cannot be saved, or None. The same rule the hub applies.
pub fn name_problem(name: &str) -> Option<String> {
    let t = name.trim();
    if t.is_empty() {
        return Some("Give the workstation a name.".to_string());
    }
    if t.chars().count() > MAX_NAME_CHARS {
        return Some(format!("The name is up to {MAX_NAME_CHARS} characters."));
    }
    if has_control(t) {
        return Some("The name has no control characters.".to_string());
    }
    None
}

/// The command a person runs on the machine to enroll it.
pub fn join_command(token: &str) -> String {
    format!("agentplaza workstation join {token}")
}

/// mm:ss for `left_ms` milliseconds, 00:00 once it has passed.
pub fn countdown(left_ms: i64) -> String {
    let s = (left_ms / 1000).max(0);
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// Milliseconds from now until `expires_ms` (negative once it has passed).
fn left_ms(expires_ms: u64) -> i64 {
    expires_ms as i64 - now_ms() as i64
}

/// A value, or a dash when there is none.
fn dash(value: &str) -> String {
    if value.is_empty() {
        "—".to_string()
    } else {
        value.to_string()
    }
}

fn yes_no(on: bool) -> String {
    (if on { "yes" } else { "no" }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_settings() -> Settings {
        Settings {
            takeover: Takeover::KeepWorkingPointerPaused,
            record: true,
            retention_days: 30,
            record_fps: 5,
            record_takeovers: false,
            work_folder: "~/AgentPlaza".to_string(),
            snapshot_reset: true,
        }
    }

    fn sample_form() -> SettingsForm {
        SettingsForm::from(&sample_settings())
    }

    #[test]
    fn status_chip_names_each_state_and_keeps_an_unknown_one() {
        assert_eq!(status_chip("online"), ("online".to_string(), ui::Tone::Ok));
        assert_eq!(
            status_chip("offline"),
            ("offline".to_string(), ui::Tone::Muted)
        );
        assert_eq!(
            status_chip("draining"),
            ("draining".to_string(), ui::Tone::Warn)
        );
        assert_eq!(
            status_chip("revoked"),
            ("revoked".to_string(), ui::Tone::Danger)
        );
        assert_eq!(
            status_chip("mystery"),
            ("mystery".to_string(), ui::Tone::Muted)
        );
    }

    #[test]
    fn account_badge_flags_only_the_owner_account() {
        let owner = Account {
            mode: "owner".to_string(),
            name: "irfan".to_string(),
        };
        let agent = Account {
            mode: "agent_user".to_string(),
            name: "apz-agent".to_string(),
        };
        assert_eq!(
            account_badge(&owner).as_deref(),
            Some("agents act as irfan")
        );
        assert_eq!(account_badge(&agent), None);
        assert!(account_text(&owner).contains("agents act as the owner"));
        assert!(account_text(&agent).contains("a separate agent account (apz-agent)"));
    }

    #[test]
    fn settings_serialize_with_the_hub_keys_and_policy_names() {
        let value = serde_json::to_value(sample_settings()).expect("settings serialize");
        assert_eq!(
            value,
            json!({
                "takeover": "keep_working_pointer_paused",
                "record": true,
                "retention_days": 30,
                "record_fps": 5,
                "record_takeovers": false,
                "work_folder": "~/AgentPlaza",
                "snapshot_reset": true,
            })
        );
        assert_eq!(
            serde_json::to_value(Takeover::AgentWaits).unwrap(),
            json!("agent_waits")
        );
        assert_eq!(
            serde_json::to_value(Takeover::ReleaseLease).unwrap(),
            json!("release_lease")
        );
    }

    #[test]
    fn the_form_round_trips_through_the_settings() {
        let form = sample_form();
        assert_eq!(form.retention_days, "30");
        assert_eq!(form.record_fps, "5");
        assert_eq!(settings_of(&form), Ok(sample_settings()));
    }

    #[test]
    fn settings_of_trims_the_folder_and_checks_each_bound() {
        let mut form = sample_form();
        form.work_folder = "  ~/Work  ".to_string();
        form.retention_days = " 1 ".to_string();
        form.record_fps = "30".to_string();
        let settings = settings_of(&form).expect("in bounds");
        assert_eq!(settings.work_folder, "~/Work");
        assert_eq!((settings.retention_days, settings.record_fps), (1, 30));

        form.retention_days = "3650".to_string();
        assert!(settings_of(&form).is_ok());
        for bad in ["0", "3651", "abc", "", "-3", "2.5", "+4"] {
            form.retention_days = bad.to_string();
            assert!(
                settings_of(&form)
                    .unwrap_err()
                    .starts_with("Keep recordings for whole days"),
                "accepted {bad:?}"
            );
        }
        form.retention_days = "30".to_string();
        for bad in ["0", "31", "x"] {
            form.record_fps = bad.to_string();
            assert!(
                settings_of(&form)
                    .unwrap_err()
                    .starts_with("Record whole frames"),
                "accepted fps {bad:?}"
            );
        }
    }

    #[test]
    fn settings_of_refuses_an_empty_long_or_control_folder() {
        let mut form = sample_form();
        form.work_folder = "   ".to_string();
        assert_eq!(
            settings_of(&form),
            Err("The work folder cannot be empty.".to_string())
        );
        form.work_folder = "a".repeat(WORK_FOLDER_MAX + 1);
        assert!(
            settings_of(&form)
                .unwrap_err()
                .starts_with("The work folder is a path")
        );
        form.work_folder = "a".repeat(WORK_FOLDER_MAX);
        assert!(settings_of(&form).is_ok());
        form.work_folder = "~/A\u{7}".to_string();
        assert!(
            settings_of(&form)
                .unwrap_err()
                .starts_with("The work folder is a path")
        );
    }

    #[test]
    fn name_problem_checks_empty_length_and_control_characters() {
        assert_eq!(
            name_problem("  "),
            Some("Give the workstation a name.".to_string())
        );
        assert_eq!(
            name_problem(&"n".repeat(MAX_NAME_CHARS + 1)),
            Some(format!("The name is up to {MAX_NAME_CHARS} characters."))
        );
        assert_eq!(
            name_problem("studio\tmac"),
            Some("The name has no control characters.".to_string())
        );
        assert_eq!(name_problem("  studio-mac  "), None);
    }

    #[test]
    fn join_command_and_countdown_format() {
        assert_eq!(
            join_command("apzw1.abc"),
            "agentplaza workstation join apzw1.abc"
        );
        assert_eq!(countdown(0), "00:00");
        assert_eq!(countdown(-5_000), "00:00");
        assert_eq!(countdown(65_999), "01:05");
        assert_eq!(countdown(30 * 60 * 1000), "30:00");
    }

    #[test]
    fn summary_reads_os_form_memory_and_gpus() {
        let facts = Facts {
            os: "linux".to_string(),
            os_version: "Arch 2026".to_string(),
            form: "desktop".to_string(),
            ram_mb: 65_536,
            gpus: vec![Gpu {
                name: "RTX 4090".to_string(),
                vram_mb: 24_576,
            }],
            ..Facts::default()
        };
        assert_eq!(
            station_summary(&facts),
            "Linux Arch 2026 · Desktop · 64 GB RAM · RTX 4090"
        );
        let bare = Facts {
            os: "windows".to_string(),
            ..Facts::default()
        };
        assert_eq!(station_summary(&bare), "Windows · no GPU");
    }

    #[test]
    fn gpu_line_shows_vram_or_shared_memory() {
        assert_eq!(
            gpu_line(&Gpu {
                name: "RTX 4090".to_string(),
                vram_mb: 24_576
            }),
            "RTX 4090 · 24 GB"
        );
        assert_eq!(
            gpu_line(&Gpu {
                name: "Apple M3".to_string(),
                vram_mb: 0
            }),
            "Apple M3 · shared memory"
        );
        assert_eq!(
            gpu_line(&Gpu {
                name: "Old card".to_string(),
                vram_mb: 512
            }),
            "Old card · 512 MB"
        );
        assert_eq!(
            displays_text(&[Display { w: 2560, h: 1440 }, Display { w: 1920, h: 1080 }]),
            "2560×1440, 1920×1080"
        );
        assert_eq!(displays_text(&[]), "none");
    }

    #[test]
    fn lease_chip_names_the_place_in_the_queue() {
        let queued = Lease {
            state: "queued".to_string(),
            position: Some(2),
            ..Lease::default()
        };
        assert_eq!(
            lease_chip(&queued),
            ("queued #2".to_string(), ui::Tone::Warn)
        );
        let waiting = Lease {
            state: "queued".to_string(),
            ..Lease::default()
        };
        assert_eq!(lease_chip(&waiting), ("queued".to_string(), ui::Tone::Warn));
        let opening = Lease {
            state: "opening".to_string(),
            ..Lease::default()
        };
        assert_eq!(
            lease_chip(&opening),
            ("opening".to_string(), ui::Tone::Info)
        );
        let active = Lease {
            state: "active".to_string(),
            ..Lease::default()
        };
        assert_eq!(lease_chip(&active), ("active".to_string(), ui::Tone::Ok));
        assert_eq!(lease_counts(&[queued, active, waiting]), (1, 2));
        assert_eq!(lease_kind_label("pixel"), "Pixel session");
        assert_eq!(lease_kind_label("shared"), "Shared");
    }

    #[test]
    fn code_chip_and_audit_labels_name_the_states_and_events() {
        assert_eq!(code_chip("active"), ("Active".to_string(), ui::Tone::Ok));
        assert_eq!(code_chip("used"), ("Used".to_string(), ui::Tone::Info));
        assert_eq!(
            code_chip("expired"),
            ("Expired".to_string(), ui::Tone::Muted)
        );
        assert_eq!(
            code_chip("revoked"),
            ("Revoked".to_string(), ui::Tone::Danger)
        );
        assert_eq!(audit_label("join_refused"), "Join refused");
        assert_eq!(audit_label("lease_released"), "Lease released");
        assert_eq!(audit_label("some_new_event"), "some new event");
    }

    #[test]
    fn takeover_text_is_the_policy_explanation() {
        assert!(takeover_text(Takeover::ReleaseLease).contains("goes back to the queue"));
        assert!(
            takeover_text(Takeover::AgentWaits).contains("resumes when you give the session back")
        );
        assert_eq!(TAKEOVERS.len(), 3);
    }

    #[test]
    fn station_json_parses_a_hub_reply_and_tolerates_a_partial_one() {
        let full = r#"{
            "id": "st_1", "name": "studio-mac", "status": "draining", "online": true, "draining": true, "revoked": false,
            "version": "0.0.9", "code": "studio-mac", "joined_ms": 1700000000000, "last_seen_ms": 1700000100000,
            "address": "10.0.0.7",
            "facts": {"os": "macos", "os_version": "15.1", "arch": "arm64", "hostname": "studio", "form": "mini",
                      "cpus": 10, "ram_mb": 32768, "gpus": [{"vendor": "apple", "name": "M3 Max", "vram_mb": 0}],
                      "displays": [{"id": 1, "w": 3024, "h": 1964, "scale": 2.0}],
                      "account": {"mode": "owner", "name": "irfan"}, "apps": 42, "virtual_displays": true, "ui_tree": true},
            "settings": {"takeover": "release_lease", "record": false, "retention_days": 7, "record_fps": 2,
                         "record_takeovers": true, "work_folder": "~/AgentPlaza", "snapshot_reset": false},
            "leases": [{"id": "l1", "station": "st_1", "agent": 3, "task": "render", "kind": "pixel", "session": 9,
                        "started_ms": 1700000050000, "state": "queued", "position": 1}]
        }"#;
        let s: Station = serde_json::from_str(full).expect("station parses");
        assert_eq!(s.status, "draining");
        assert!(s.draining && !s.revoked);
        assert_eq!(s.facts.gpus[0].name, "M3 Max");
        assert_eq!(s.facts.displays[0].h, 1964);
        assert_eq!(
            account_badge(&s.facts.account).as_deref(),
            Some("agents act as irfan")
        );
        assert_eq!(s.settings.takeover, Takeover::ReleaseLease);
        assert_eq!(s.leases[0].position, Some(1));
        assert_eq!(s.leases[0].task.as_deref(), Some("render"));

        let partial: Station = serde_json::from_str(r#"{"id": "st_2", "name": "bare"}"#)
            .expect("partial station parses");
        assert_eq!(partial.name, "bare");
        assert!(partial.leases.is_empty() && partial.facts.gpus.is_empty());
        assert_eq!(partial.settings, Settings::default());
    }

    #[test]
    fn a_settings_answer_parses_with_and_without_a_push_error() {
        let pushed: SettingsAnswer = serde_json::from_value(
            json!({"settings": sample_settings(), "pushed": true, "push_error": null}),
        )
        .expect("answer parses");
        assert!(pushed.pushed && pushed.push_error.is_none());
        let held: SettingsAnswer = serde_json::from_value(
            json!({"settings": sample_settings(), "pushed": false, "push_error": "offline"}),
        )
        .expect("answer parses");
        assert_eq!(held.push_error.as_deref(), Some("offline"));
    }

    #[test]
    fn station_events_are_the_station_and_lease_frames() {
        assert!(station_event("station"));
        assert!(station_event("lease_granted"));
        assert!(station_event("lease_released"));
        assert!(station_event("lease_owner_input"));
        assert!(!station_event("agent"));
        assert!(!station_event("stationary"));
    }

    #[test]
    fn a_failed_refresh_keeps_what_the_list_shows() {
        let mut slot: Load<Vec<Code>> = Load::Pending;
        settle_into(&mut slot, Err("down".to_string()));
        assert!(matches!(slot, Load::Failed));
        settle_into(&mut slot, Ok(vec![Code::default()]));
        settle_into(&mut slot, Err("down".to_string()));
        assert!(matches!(&slot, Load::Ready(list) if list.len() == 1));
    }
}
