//! The Tools screen (parity with the web console's `tools` view): the connectors the plaza lends
//! to agents (configure, enable, test), the connectors agents proposed (review), the tools agents
//! built into the library (files, publish, reject, remove) and the tools found by agents (search,
//! flag, remove). A key is written once here and kept in the plaza's vault: the inputs are secret,
//! and a stored value is never sent back.

use std::collections::{BTreeMap, HashSet};

use iced::font::Weight;
use iced::widget::text::Wrapping;
use iced::widget::{Space, Stack, column, container, pick_list, row, text, text_input};
use iced::{Alignment, Border, Color, Element, Font, Length, Task, Theme};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::Id;
use crate::console::{Ctx, client, ui};
use crate::theme;

const DISC_PAGE: usize = 10;
const DISC_KINDS: [&str; 5] = ["cli", "library", "service", "api", "technique"];
const SECTIONS: [&str; 4] = ["Enabled", "Configured", "Needs a key", "Coming soon"];

const SANS_SEMIBOLD: Font = Font {
    weight: Weight::Semibold,
    ..theme::SANS
};

// ---- The hub's shapes (as apps/web/src/api.ts types them) -----------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Connector {
    pub id: String,
    pub label: String,
    pub brand: String,
    pub kind: String,
    pub description: String,
    pub docs_url: String,
    pub configured: bool,
    pub enabled: bool,
    pub fields: Vec<ConnField>,
    pub roles: Vec<String>,
    pub actions: Vec<ConnAction>,
    pub cli: Option<CliInfo>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ConnField {
    pub name: String,
    pub label: String,
    pub env: String,
    pub hint: String,
    pub secret: bool,
    pub required: bool,
    pub set: bool,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ConnAction {
    pub name: String,
    pub risk: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CliInfo {
    pub binary: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ConnTest {
    pub ok: bool,
    pub latency_ms: f64,
    pub detail: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Proposal {
    pub id: String,
    pub proposer: String,
    pub state: String,
    pub note: Option<String>,
    pub checks: Vec<String>,
    pub spec: ProposalSpec,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProposalSpec {
    pub id: String,
    pub label: String,
    pub category: String,
    pub description: String,
    pub docs_url: String,
    pub base_url: String,
    pub auth: ProposalAuth,
    pub fields: Vec<ProposalField>,
    pub actions: Vec<ProposalAction>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProposalAuth {
    pub kind: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProposalField {
    pub label: String,
    pub env: String,
    pub secret: bool,
    pub required: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProposalAction {
    pub name: String,
    pub description: String,
    pub risk: String,
    pub method: String,
    pub path: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LibraryTool {
    pub name: String,
    pub version: u64,
    pub author: String,
    pub description: String,
    pub command: Vec<String>,
    pub requires: Vec<String>,
    pub state: String,
    pub files: Vec<LibFile>,
    pub calls: u64,
    pub review_note: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LibFile {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FileContent {
    pub content: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Discovery {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub what_for: String,
    pub how: String,
    pub verdict: String,
    pub url: Option<String>,
    pub tags: Vec<String>,
    pub by: String,
    pub endorsed_by: Vec<Value>,
    pub flagged: Option<String>,
}

// ---- State and messages ---------------------------------------------------------------------

/// The dialog that is open over the screen.
#[derive(Debug, Clone)]
pub enum Modal {
    /// One connector's credentials (write-only), roles and enabled flag.
    Configure {
        id: String,
        values: BTreeMap<String, String>,
        roles: String,
        enabled: bool,
    },
    /// A connector proposal's full spec, with approve or reject.
    Review { id: String, note: String },
    /// A library tool's files; `shown` is the file open in the viewer (path, content).
    Files {
        name: String,
        version: u64,
        shown: Option<(String, String)>,
    },
    /// Reject a library tool, with an optional note for its author.
    RejectTool {
        name: String,
        version: u64,
        note: String,
    },
    /// Remove a library tool version.
    RemoveTool { name: String, version: u64 },
    /// Flag a found tool with a reason.
    FlagFound {
        id: String,
        name: String,
        reason: String,
    },
    /// Remove a found tool (the web's confirm).
    RemoveFound { id: String, name: String },
}

#[derive(Debug, Default)]
pub struct State {
    /// The connectors as last loaded; None until the first load.
    pub connectors: Option<Vec<Connector>>,
    pub proposals: Vec<Proposal>,
    pub library: Vec<LibraryTool>,
    pub toolbox: Vec<Discovery>,
    /// A load error, shown above the sections.
    pub error: Option<String>,
    /// The last outcome (text, is an error), shown above the sections.
    pub notice: Option<(String, bool)>,
    pub show_decided: bool,
    pub expanded: HashSet<String>,
    pub query: String,
    /// The found-tools kind filter; empty = all kinds.
    pub kind: String,
    /// The found-tools page, 1-based (0 reads as 1).
    pub page: usize,
    pub modal: Option<Modal>,
    /// The open dialog's error.
    pub modal_error: Option<String>,
    /// A write is on its way to the hub.
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    ConnectorsLoaded(Result<Vec<Connector>, String>),
    ProposalsLoaded(Result<Vec<Proposal>, String>),
    LibraryLoaded(Result<Vec<LibraryTool>, String>),
    ToolboxLoaded(Result<Vec<Discovery>, String>),
    /// A write finished: its outcome text, or the hub's error.
    Done(Result<String, String>),
    SetEnabled(String, bool),
    Test(String),
    Tested(Result<ConnTest, String>),
    Configure(String),
    CfgField(String, String),
    CfgRoles(String),
    CfgToggle,
    CfgClear(String),
    CfgSave,
    ToggleDecided,
    Review(String),
    ReviewNote(String),
    Decide(bool),
    OpenFiles(String, u64),
    ShowFile(String),
    FileLoaded(String, Result<String, String>),
    Publish(String, u64),
    OpenReject(String, u64),
    RejectNote(String),
    RejectTool,
    OpenRemove(String, u64),
    RemoveTool,
    Expand(String),
    Query(String),
    FilterKind(String),
    Page(isize),
    OpenFlag(String),
    FlagReason(String),
    FlagSubmit,
    Unflag(String),
    OpenRemoveFound(String, String),
    RemoveFound,
    Close,
    /// Opens a connector's or proposal's docs link in the browser.
    OpenDocs(String),
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

// ---- Loading and writing --------------------------------------------------------------------

/// Loads the screen's data (called when it opens and on every poll tick).
pub fn load(_state: &mut State, ctx: &Ctx) -> Task<Msg> {
    Task::batch([
        fetch::<Vec<Connector>>(ctx, "/v1/connectors", Msg::ConnectorsLoaded),
        fetch::<Vec<Proposal>>(ctx, "/v1/connectors/proposals", Msg::ProposalsLoaded),
        fetch::<Vec<LibraryTool>>(ctx, "/v1/library", Msg::LibraryLoaded),
        fetch::<Vec<Discovery>>(ctx, "/v1/toolbox", Msg::ToolboxLoaded),
    ])
}

fn fetch<T>(ctx: &Ctx, path: &str, map: fn(Result<T, String>) -> Msg) -> Task<Msg>
where
    T: DeserializeOwned + Send + 'static,
{
    let client = ctx.client.clone();
    let path = path.to_string();
    client::task(move || client.get::<T>(&path), map)
}

/// One write to the hub; on success the screen reloads and the dialog closes, showing `done`.
fn act(
    ctx: &Ctx,
    done: String,
    run: impl FnOnce(&client::Client) -> Result<(), String> + Send + 'static,
) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(move || run(&client).map(|()| done), Msg::Done)
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::ConnectorsLoaded(Ok(list)) => {
            state.connectors = Some(list);
            state.error = None;
            Task::none()
        }
        Msg::ProposalsLoaded(Ok(list)) => {
            state.proposals = list;
            state.error = None;
            Task::none()
        }
        Msg::LibraryLoaded(Ok(list)) => {
            state.library = list;
            state.error = None;
            Task::none()
        }
        Msg::ToolboxLoaded(Ok(list)) => {
            state.toolbox = list;
            state.error = None;
            Task::none()
        }
        Msg::ConnectorsLoaded(Err(e))
        | Msg::ProposalsLoaded(Err(e))
        | Msg::LibraryLoaded(Err(e))
        | Msg::ToolboxLoaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::Done(Ok(label)) => {
            state.busy = false;
            state.modal = None;
            state.modal_error = None;
            state.notice = Some((label, false));
            load(state, ctx)
        }
        Msg::Done(Err(e)) => {
            state.busy = false;
            if state.modal.is_some() {
                state.modal_error = Some(e);
            } else {
                state.notice = Some((e, true));
            }
            Task::none()
        }
        Msg::SetEnabled(id, on) => {
            let label = format!(
                "{} {}",
                connector_label(state, &id),
                if on { "enabled" } else { "disabled" }
            );
            let path = format!("/v1/connectors/{id}");
            act(ctx, label, move |c| {
                c.call("PUT", &path, Some(&json!({ "enabled": on })))
                    .map(|_| ())
            })
        }
        Msg::Test(id) => {
            let path = format!("/v1/connectors/{id}/test");
            let client = ctx.client.clone();
            client::task(
                move || client.post::<ConnTest>(&path, &json!({})),
                Msg::Tested,
            )
        }
        Msg::Tested(Ok(report)) => {
            state.notice = Some(if report.ok {
                (
                    format!("OK · {} ms · {}", report.latency_ms, report.detail),
                    false,
                )
            } else if report.detail.is_empty() {
                ("Test failed".to_string(), true)
            } else {
                (report.detail, true)
            });
            Task::none()
        }
        Msg::Tested(Err(e)) => {
            state.notice = Some((e, true));
            Task::none()
        }
        Msg::Configure(id) => {
            let Some(c) = connector(state, &id) else {
                return Task::none();
            };
            let values: BTreeMap<String, String> = c
                .fields
                .iter()
                .map(|f| (f.name.clone(), String::new()))
                .collect();
            let modal = Modal::Configure {
                id: id.clone(),
                values,
                roles: c.roles.join(", "),
                enabled: c.enabled,
            };
            state.modal = Some(modal);
            state.modal_error = None;
            Task::none()
        }
        Msg::CfgField(name, value) => {
            if let Some(Modal::Configure { values, .. }) = &mut state.modal {
                values.insert(name, value);
            }
            Task::none()
        }
        Msg::CfgRoles(text) => {
            if let Some(Modal::Configure { roles, .. }) = &mut state.modal {
                *roles = text;
            }
            Task::none()
        }
        Msg::CfgToggle => {
            if let Some(Modal::Configure { enabled, .. }) = &mut state.modal {
                *enabled = !*enabled;
            }
            Task::none()
        }
        Msg::CfgClear(name) => {
            if state.busy {
                return Task::none();
            }
            let Some(Modal::Configure { id, .. }) = state.modal.clone() else {
                return Task::none();
            };
            let label = connector(state, &id)
                .and_then(|c| c.fields.iter().find(|f| f.name == name))
                .map_or_else(|| name.clone(), |f| f.label.clone());
            state.busy = true;
            state.modal_error = None;
            let path = format!("/v1/connectors/{id}/secret?field={}", encode(&name));
            act(ctx, format!("{label} removed"), move |c| {
                c.delete(&path).map(|_| ())
            })
        }
        Msg::CfgSave => {
            if state.busy {
                return Task::none();
            }
            let Some(Modal::Configure {
                id,
                values,
                roles,
                enabled,
            }) = state.modal.clone()
            else {
                return Task::none();
            };
            let Some(c) = connector(state, &id) else {
                return Task::none();
            };
            // Each field typed into is written; an empty field keeps what is stored.
            let secrets: Vec<(String, String)> = c
                .fields
                .iter()
                .filter_map(|f| {
                    let v = values.get(&f.name).map_or("", |s| s.trim());
                    (!v.is_empty()).then(|| (f.name.clone(), v.to_string()))
                })
                .collect();
            let roles: Vec<String> = roles
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            state.busy = true;
            state.modal_error = None;
            let secret_path = format!("/v1/connectors/{id}/secret");
            let path = format!("/v1/connectors/{id}");
            act(ctx, "Saved".to_string(), move |client| {
                for (field, value) in secrets {
                    client
                        .call(
                            "PUT",
                            &secret_path,
                            Some(&json!({ "field": field, "value": value })),
                        )
                        .map(|_| ())?;
                }
                client
                    .call(
                        "PUT",
                        &path,
                        Some(&json!({ "enabled": enabled, "roles": roles })),
                    )
                    .map(|_| ())
            })
        }
        Msg::ToggleDecided => {
            state.show_decided = !state.show_decided;
            Task::none()
        }
        Msg::Review(id) => {
            state.modal = Some(Modal::Review {
                id,
                note: String::new(),
            });
            state.modal_error = None;
            Task::none()
        }
        Msg::ReviewNote(text) => {
            if let Some(Modal::Review { note, .. }) = &mut state.modal {
                *note = text;
            }
            Task::none()
        }
        Msg::Decide(approve) => {
            if state.busy {
                return Task::none();
            }
            let Some(Modal::Review { id, note }) = state.modal.clone() else {
                return Task::none();
            };
            let blocked = state
                .proposals
                .iter()
                .find(|p| p.id == id)
                .is_some_and(|p| !p.checks.is_empty());
            if approve && blocked {
                return Task::none();
            }
            let label = proposal_label(state, &id);
            let note = note.trim().to_string();
            let mut body = json!({ "approve": approve });
            if !approve && !note.is_empty() {
                body["note"] = json!(note);
            }
            state.busy = true;
            state.modal_error = None;
            let path = format!("/v1/connectors/proposals/{id}/review");
            let outcome = format!("{label} {}", if approve { "approved" } else { "rejected" });
            act(ctx, outcome, move |client| {
                client.call("POST", &path, Some(&body)).map(|_| ())
            })
        }
        Msg::OpenFiles(name, version) => {
            state.modal = Some(Modal::Files {
                name,
                version,
                shown: None,
            });
            state.modal_error = None;
            Task::none()
        }
        Msg::ShowFile(path) => {
            let Some(Modal::Files {
                name,
                version,
                shown,
            }) = &mut state.modal
            else {
                return Task::none();
            };
            *shown = Some((path.clone(), "Loading…".to_string()));
            let url = format!("/v1/library/{name}/{version}/files?path={}", encode(&path));
            let client = ctx.client.clone();
            let p = path.clone();
            client::task(
                move || client.get::<FileContent>(&url).map(|f| f.content),
                move |r| Msg::FileLoaded(p.clone(), r),
            )
        }
        Msg::FileLoaded(path, result) => {
            let content = match result {
                Ok(text) => text,
                Err(_) => "Could not read this file.".to_string(),
            };
            if let Some(Modal::Files { shown, .. }) = &mut state.modal {
                *shown = Some((path, content));
            }
            Task::none()
        }
        Msg::Publish(name, version) => {
            let outcome = format!("x_{name} published");
            let path = format!("/v1/library/{name}/{version}/review");
            act(ctx, outcome, move |client| {
                client
                    .call("POST", &path, Some(&json!({ "approve": true })))
                    .map(|_| ())
            })
        }
        Msg::OpenReject(name, version) => {
            state.modal = Some(Modal::RejectTool {
                name,
                version,
                note: String::new(),
            });
            state.modal_error = None;
            Task::none()
        }
        Msg::RejectNote(text) => {
            if let Some(Modal::RejectTool { note, .. }) = &mut state.modal {
                *note = text;
            }
            Task::none()
        }
        Msg::RejectTool => {
            if state.busy {
                return Task::none();
            }
            let Some(Modal::RejectTool {
                name,
                version,
                note,
            }) = state.modal.clone()
            else {
                return Task::none();
            };
            let note = note.trim().to_string();
            let mut body = json!({ "approve": false });
            if !note.is_empty() {
                body["note"] = json!(note);
            }
            state.busy = true;
            state.modal_error = None;
            let outcome = format!("x_{name} rejected");
            let path = format!("/v1/library/{name}/{version}/review");
            act(ctx, outcome, move |client| {
                client.call("POST", &path, Some(&body)).map(|_| ())
            })
        }
        Msg::OpenRemove(name, version) => {
            state.modal = Some(Modal::RemoveTool { name, version });
            state.modal_error = None;
            Task::none()
        }
        Msg::RemoveTool => {
            if state.busy {
                return Task::none();
            }
            let Some(Modal::RemoveTool { name, version }) = state.modal.clone() else {
                return Task::none();
            };
            state.busy = true;
            state.modal_error = None;
            let outcome = format!("x_{name} removed");
            let path = format!("/v1/library/{name}/{version}");
            act(ctx, outcome, move |client| client.delete(&path).map(|_| ()))
        }
        Msg::Expand(id) => {
            if !state.expanded.remove(&id) {
                state.expanded.insert(id);
            }
            Task::none()
        }
        Msg::Query(text) => {
            state.query = text;
            state.page = 1;
            Task::none()
        }
        Msg::FilterKind(kind) => {
            state.kind = kind;
            state.page = 1;
            Task::none()
        }
        Msg::Page(delta) => {
            state.page = (state.page.max(1) as isize + delta).max(1) as usize;
            Task::none()
        }
        Msg::OpenFlag(id) => {
            let name = found_name(state, &id);
            state.modal = Some(Modal::FlagFound {
                id,
                name,
                reason: String::new(),
            });
            state.modal_error = None;
            Task::none()
        }
        Msg::FlagReason(text) => {
            if let Some(Modal::FlagFound { reason, .. }) = &mut state.modal {
                *reason = text;
            }
            Task::none()
        }
        Msg::FlagSubmit => {
            if state.busy {
                return Task::none();
            }
            let Some(Modal::FlagFound { id, name, reason }) = state.modal.clone() else {
                return Task::none();
            };
            let reason = reason.trim().to_string();
            if reason.is_empty() {
                state.modal_error = Some("Write a reason first".to_string());
                return Task::none();
            }
            state.busy = true;
            state.modal_error = None;
            let outcome = format!("{name} flagged");
            let path = format!("/v1/toolbox/{id}/flag");
            act(ctx, outcome, move |client| {
                client
                    .post::<Value>(&path, &json!({ "reason": reason }))
                    .map(|_| ())
            })
        }
        Msg::Unflag(id) => {
            let outcome = format!("{} unflagged", found_name(state, &id));
            let path = format!("/v1/toolbox/{id}/flag");
            act(ctx, outcome, move |client| {
                client
                    .post::<Value>(&path, &json!({ "reason": null }))
                    .map(|_| ())
            })
        }
        Msg::OpenRemoveFound(id, name) => {
            state.modal = Some(Modal::RemoveFound { id, name });
            state.modal_error = None;
            Task::none()
        }
        Msg::RemoveFound => {
            if state.busy {
                return Task::none();
            }
            let Some(Modal::RemoveFound { id, name }) = state.modal.clone() else {
                return Task::none();
            };
            state.busy = true;
            state.modal_error = None;
            state.expanded.remove(&id);
            let outcome = format!("{name} removed");
            let path = format!("/v1/toolbox/{id}");
            act(ctx, outcome, move |client| client.delete(&path).map(|_| ()))
        }
        Msg::OpenDocs(url) => {
            if let Err(e) = ui::open_url(&url) {
                state.error = Some(e);
            }
            Task::none()
        }
        Msg::Close => {
            state.modal = None;
            state.modal_error = None;
            Task::none()
        }
    }
}

// ---- Helpers --------------------------------------------------------------------------------

fn connector<'s>(state: &'s State, id: &str) -> Option<&'s Connector> {
    state.connectors.as_ref()?.iter().find(|c| c.id == id)
}

fn connector_label(state: &State, id: &str) -> String {
    connector(state, id).map_or_else(|| id.to_string(), |c| c.label.clone())
}

fn proposal_label(state: &State, id: &str) -> String {
    state
        .proposals
        .iter()
        .find(|p| p.id == id)
        .map_or_else(|| id.to_string(), |p| p.spec.label.clone())
}

fn found_name(state: &State, id: &str) -> String {
    state
        .toolbox
        .iter()
        .find(|d| d.id == id)
        .map_or_else(|| id.to_string(), |d| d.name.clone())
}

fn gate(busy: bool, msg: Msg) -> Option<Msg> {
    if busy { None } else { Some(msg) }
}

/// Percent-encodes a query value: every byte outside the unreserved set.
pub(super) fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A connector is Enabled, Configured, Needs a key (a required field has no value), or Coming soon (no actions yet).
fn section_of(c: &Connector) -> &'static str {
    if c.actions.is_empty() {
        return "Coming soon";
    }
    if !c.configured || c.fields.iter().any(|f| f.required && !f.set) {
        return "Needs a key";
    }
    if c.enabled { "Enabled" } else { "Configured" }
}

fn section_status(section: &str) -> (&'static str, ui::Tone) {
    match section {
        "Enabled" => ("Enabled", ui::Tone::Ok),
        "Configured" => ("Key set", ui::Tone::Info),
        "Needs a key" => ("Needs a key", ui::Tone::Warn),
        _ => ("Coming soon", ui::Tone::Muted),
    }
}

/// Every required field is read from the environment, so the vault holds none of the key.
fn from_env_only(c: &Connector) -> bool {
    let required: Vec<&ConnField> = c.fields.iter().filter(|f| f.required).collect();
    !required.is_empty() && required.iter().all(|f| f.source.as_deref() == Some("env"))
}

fn source_line(f: &ConnField) -> String {
    match f.source.as_deref() {
        Some("vault") => "Set in the vault".to_string(),
        Some("env") => format!("Read from ${}", f.env),
        Some("sealed") => "Vault sealed".to_string(),
        Some("out_of_scope") => "Not available on this node".to_string(),
        _ => format!("Not set — or set ${} in apid's keys file", f.env),
    }
}

fn risk_tone(risk: &str) -> ui::Tone {
    match risk {
        "read" => ui::Tone::Ok,
        "write" => ui::Tone::Info,
        "irreversible" => ui::Tone::Danger,
        _ => ui::Tone::Muted,
    }
}

fn auth_label(auth: &ProposalAuth) -> String {
    match &auth.name {
        Some(name) if !name.is_empty() => format!("{} {}", auth.kind, name),
        _ => auth.kind.clone(),
    }
}

fn matches(d: &Discovery, q: &str) -> bool {
    q.is_empty()
        || [d.name.as_str(), d.what_for.as_str(), d.by.as_str()]
            .iter()
            .any(|s| s.to_lowercase().contains(q))
        || d.tags.iter().any(|t| t.to_lowercase().contains(q))
}

fn boxed(_: &Theme) -> container::Style {
    container::Style {
        border: Border {
            color: crate::theme::palette().rule,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

/// A card (the web's `.prov-card`): a boxed frame, 16px sides, 12px below.
fn frame<'a>(content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    container(content)
        .padding(iced::Padding {
            top: 16.0,
            right: 16.0,
            bottom: 12.0,
            left: 16.0,
        })
        .width(Length::Fill)
        .style(boxed)
        .into()
}

/// A small boxed tag (the web's tool-chip).
fn chip<'a>(content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    container(content).padding([2, 8]).style(boxed).into()
}

/// A 1px line in `color`, full width (the web's rules).
fn rule_line<'a>(color: Color) -> Element<'a, Msg> {
    container(Space::new().height(1.0))
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(color.into()),
            ..Default::default()
        })
        .into()
}

/// Items spread across the line with equal gaps (the web's `justify-content: space-between`).
fn between<'a>(items: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let last = items.len().saturating_sub(1);
    let mut line = row(Vec::new())
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    for (i, item) in items.into_iter().enumerate() {
        line = line.push(item);
        if i < last {
            line = line.push(Space::new().width(Length::Fill));
        }
    }
    line.into()
}

/// A card's actions (the web's `.prov-actions`): a faint rule, then the buttons from the left.
fn card_footer<'a>(items: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    column![
        ui::hairline(),
        row(items).spacing(8).align_y(Alignment::Center)
    ]
    .spacing(10)
    .into()
}

/// A card's head: the logo (or none), then a serif title over a one-line faint line, cut at the edge.
fn card_head<'a>(
    logo: Option<Element<'a, Msg>>,
    title: Element<'a, Msg>,
    sub: String,
) -> Element<'a, Msg> {
    let sub = container(ui::faint(sub).wrapping(Wrapping::None))
        .width(Length::Fill)
        .height(Length::Fixed(18.0))
        .clip(true);
    let block = column![title, sub].spacing(2).width(Length::Fill);
    match logo {
        Some(logo) => row![logo, block]
            .spacing(12)
            .align_y(Alignment::Center)
            .into(),
        None => block.into(),
    }
}

/// A small square dot in a tone (the web's risk dot).
fn dot<'a>(tone: ui::Tone) -> Element<'a, Msg> {
    let color = tone.color();
    container(
        Space::new()
            .width(Length::Fixed(7.0))
            .height(Length::Fixed(7.0)),
    )
    .style(move |_: &Theme| container::Style {
        background: Some(color.into()),
        ..Default::default()
    })
    .into()
}

/// A section title (the web's `.prov-group-title`): small mono caps over a rule.
fn group_title<'a>(title: &str) -> Element<'a, Msg> {
    column![
        ui::sans(title.to_string(), 11.0, crate::theme::palette().ink),
        rule_line(crate::theme::palette().rule)
    ]
    .spacing(6)
    .width(Length::Fill)
    .into()
}

/// A connector's logo box (the web's 48px `.logo.lg`): its brand mark, else its initials.
fn logo<'a>(c: &Connector) -> Element<'a, Msg> {
    let inner: Element<'a, Msg> = if !c.brand.is_empty() {
        ui::brand_logo(&c.brand, 26.0)
    } else {
        let letters: String = c
            .label
            .split_whitespace()
            .filter_map(|w| w.chars().next())
            .take(2)
            .collect::<String>()
            .to_uppercase();
        text(letters)
            .font(ui::SANS_BOLD)
            .size(13.0)
            .color(crate::theme::palette().ink)
            .into()
    };
    container(inner)
        .width(Length::Fixed(48.0))
        .height(Length::Fixed(48.0))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(boxed)
        .into()
}

/// A card's title in the web's serif semibold, one line.
fn card_title<'a>(label: &str) -> Element<'a, Msg> {
    text(label.to_string())
        .font(SANS_SEMIBOLD)
        .size(16.5)
        .color(crate::theme::palette().ink)
        .wrapping(Wrapping::None)
        .into()
}

fn checks_box<'a>(checks: &[String]) -> Element<'a, Msg> {
    let mut col = column![ui::badge("Checks failed · approve is off", ui::Tone::Warn)].spacing(4);
    for check in checks {
        col = col.push(ui::faint(format!("· {check}")));
    }
    col.into()
}

fn gated_row<'a>(items: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let mut line = row![Space::new().width(Length::Fill)]
        .spacing(8)
        .align_y(Alignment::Center);
    for item in items {
        line = line.push(item);
    }
    line.into()
}

// ---- Connectors -----------------------------------------------------------------------------

fn connector_card<'a>(c: &Connector) -> Element<'a, Msg> {
    let section = section_of(c);
    let (text, tone) = section_status(section);
    let needs_key = section == "Needs a key";
    let cli = c.kind == "cli";

    let mut meta = vec![ui::badge(
        if cli { "CLI" } else { "API" },
        if cli {
            ui::Tone::Signal
        } else {
            ui::Tone::Muted
        },
    )];
    if section != "Coming soon" && from_env_only(c) {
        meta.push(ui::badge("Key from env", ui::Tone::Info));
    }
    meta.push(ui::badge(text, tone));

    let mut body = column![
        card_head(Some(logo(c)), card_title(&c.label), c.description.clone()),
        between(meta)
    ]
    .spacing(12);

    if !c.actions.is_empty() {
        let chips: Vec<Element<'a, Msg>> = c
            .actions
            .iter()
            .map(|a| {
                chip(
                    row![
                        dot(risk_tone(&a.risk)),
                        ui::mono(a.name.clone(), 11.0, crate::theme::palette().ink_2)
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                )
            })
            .collect();
        body = body.push(row(chips).spacing(8));
    }

    if section != "Coming soon" {
        let mut footer = vec![ui::btn(
            "Configure",
            None,
            if needs_key {
                ui::Kind::Primary
            } else {
                ui::Kind::Secondary
            },
            Some(Msg::Configure(c.id.clone())),
        )];
        if c.configured {
            footer.push(ui::btn(
                "Test",
                None,
                ui::Kind::Secondary,
                Some(Msg::Test(c.id.clone())),
            ));
            footer.push(ui::btn(
                if c.enabled { "Disable" } else { "Enable" },
                None,
                ui::Kind::Ghost,
                Some(Msg::SetEnabled(c.id.clone(), !c.enabled)),
            ));
        }
        body = body.push(card_footer(footer));
    }
    frame(body)
}

fn connectors_section<'a>(state: &State) -> Element<'a, Msg> {
    let mut col = column![].spacing(22).width(Length::Fill);
    match &state.connectors {
        None => return ui::loading(),
        Some(list) if list.is_empty() => return ui::empty("models", "No tools yet", ""),
        Some(list) => {
            for section in SECTIONS {
                let cards: Vec<Element<'a, Msg>> = list
                    .iter()
                    .filter(|c| section_of(c) == section)
                    .map(connector_card)
                    .collect();
                if !cards.is_empty() {
                    col = col.push(column![group_title(section), ui::card_grid(cards)].spacing(12));
                }
            }
        }
    }
    col.into()
}

fn configure_body<'a>(
    state: &State,
    id: &str,
    values: &BTreeMap<String, String>,
    roles: &str,
    enabled: bool,
) -> Element<'a, Msg> {
    let Some(c) = connector(state, id) else {
        return ui::faint("This tool is gone.").into();
    };
    let mut col = column![].spacing(12).width(Length::Fill);
    if let Some(cli) = &c.cli {
        col = col.push(ui::faint(format!(
            "Installs {} in the agent's VM, pointed at the plaza's gateway — no key inside the VM.",
            cli.binary
        )));
    }
    col = col.push(ui::faint(
        "Write-only: a stored value is never shown again. Empty fields keep what is stored.",
    ));
    for f in &c.fields {
        let value = values.get(&f.name).map_or("", String::as_str);
        let name = f.name.clone();
        let placeholder = if f.hint.is_empty() {
            f.label.clone()
        } else {
            f.hint.clone()
        };
        let input = text_input(&placeholder, value)
            .on_input(move |v| Msg::CfgField(name.clone(), v))
            .on_submit(Msg::CfgSave)
            .secure(f.secret)
            .padding(8)
            .style(theme::input);
        let mut field = column![
            ui::label(&format!(
                "{} · {}",
                f.label,
                if f.required { "required" } else { "optional" }
            )),
            input,
            ui::faint(source_line(f)),
        ]
        .spacing(6);
        if f.source.as_deref() == Some("vault") {
            field = field.push(gated_row(vec![ui::btn(
                "Remove",
                None,
                ui::Kind::Ghost,
                gate(state.busy, Msg::CfgClear(f.name.clone())),
            )]));
        }
        col = col.push(field);
    }
    let roles_input = text_input("all roles", roles)
        .on_input(Msg::CfgRoles)
        .on_submit(Msg::CfgSave)
        .padding(8)
        .style(theme::input);
    col = col.push(
        column![
            ui::label("Roles"),
            roles_input,
            ui::faint("Comma-separated; empty = all roles")
        ]
        .spacing(6),
    );
    col = col.push(ui::btn(
        if enabled {
            "Enabled: on"
        } else {
            "Enabled: off"
        },
        None,
        ui::Kind::Secondary,
        Some(Msg::CfgToggle),
    ));
    if !c.docs_url.is_empty() {
        col = col.push(ui::link("Docs", Msg::OpenDocs(c.docs_url.clone()), false));
    }
    col = col.push(gated_row(vec![
        ui::btn("Close", None, ui::Kind::Ghost, Some(Msg::Close)),
        ui::btn(
            "Save",
            None,
            ui::Kind::Primary,
            gate(state.busy, Msg::CfgSave),
        ),
    ]));
    col.into()
}

// ---- Proposed connectors --------------------------------------------------------------------

fn proposal_card<'a>(p: &Proposal) -> Element<'a, Msg> {
    let s = &p.spec;
    let head = column![
        card_title(&s.label),
        ui::mono(s.id.clone(), 11.5, crate::theme::palette().ink_3).wrapping(Wrapping::None),
    ]
    .spacing(2)
    .width(Length::Fill);
    let mut body = column![
        head,
        between(vec![
            ui::faint(format!("proposed by {}", p.proposer)).into(),
            ui::badge(&s.category, ui::Tone::Signal),
            ui::badge(&auth_label(&s.auth), ui::Tone::Muted),
        ]),
        ui::faint(s.description.clone()),
        ui::mono(s.base_url.clone(), 11.0, crate::theme::palette().ink_2).wrapping(Wrapping::None),
    ]
    .spacing(12);

    if !s.fields.is_empty() {
        let chips: Vec<Element<'a, Msg>> = s
            .fields
            .iter()
            .map(|f| {
                chip(ui::sans(
                    format!("{}{}", f.label, if f.required { " *" } else { "" }),
                    11.0,
                    crate::theme::palette().ink_2,
                ))
            })
            .collect();
        body = body.push(row(chips).spacing(6));
    }
    if !s.actions.is_empty() {
        let chips: Vec<Element<'a, Msg>> = s
            .actions
            .iter()
            .map(|a| {
                chip(
                    row![
                        dot(risk_tone(&a.risk)),
                        ui::mono(a.name.clone(), 11.0, crate::theme::palette().ink_2),
                        ui::mono(
                            format!(" {} {}", a.method, a.path),
                            11.0,
                            crate::theme::palette().ink_3
                        ),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                )
            })
            .collect();
        body = body.push(row(chips).spacing(6));
    }
    if !p.checks.is_empty() {
        body = body.push(checks_box(&p.checks));
    }
    body = body.push(card_footer(vec![ui::btn(
        "Review",
        None,
        ui::Kind::Primary,
        Some(Msg::Review(p.id.clone())),
    )]));
    frame(body)
}

fn decided_row<'a>(p: &Proposal) -> Element<'a, Msg> {
    let (text, tone) = match p.state.as_str() {
        "approved" => ("Approved", ui::Tone::Ok),
        "rejected" => ("Rejected", ui::Tone::Danger),
        _ => ("Pending", ui::Tone::Warn),
    };
    let mut line = row![
        ui::mono(p.spec.id.clone(), 12.0, crate::theme::palette().ink),
        ui::badge(text, tone),
        ui::faint(format!("{} · proposed by {}", p.spec.label, p.proposer)),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    if let Some(note) = p.note.as_ref().filter(|n| !n.is_empty()) {
        line = line.push(ui::faint(format!("· {note}")));
    }
    line.into()
}

fn proposals_section<'a>(state: &State) -> Element<'a, Msg> {
    let pending: Vec<&Proposal> = state
        .proposals
        .iter()
        .filter(|p| p.state == "pending")
        .collect();
    let decided: Vec<&Proposal> = state
        .proposals
        .iter()
        .filter(|p| p.state != "pending")
        .collect();
    if pending.is_empty() && decided.is_empty() {
        return Space::new().into();
    }
    let mut col = column![group_title("Proposed connectors")]
        .spacing(12)
        .width(Length::Fill);
    if pending.is_empty() {
        col = col.push(ui::faint("Nothing waiting for your review."));
    } else {
        let cards: Vec<Element<'a, Msg>> = pending.iter().map(|p| proposal_card(p)).collect();
        col = col.push(ui::card_grid(cards));
    }
    if !decided.is_empty() {
        let label = if state.show_decided {
            "Hide decided".to_string()
        } else {
            format!("Decided ({})", decided.len())
        };
        col = col.push(ui::btn(
            &label,
            None,
            ui::Kind::Ghost,
            Some(Msg::ToggleDecided),
        ));
        if state.show_decided {
            let rows: Vec<Element<'a, Msg>> = decided.iter().map(|p| decided_row(p)).collect();
            col = col.push(column(rows).spacing(6));
        }
    }
    col.into()
}

fn review_body<'a>(state: &State, id: &str, note: &str) -> Element<'a, Msg> {
    let Some(p) = state.proposals.iter().find(|p| p.id == id) else {
        return ui::faint("This proposal is gone.").into();
    };
    let s = &p.spec;
    let blocked = !p.checks.is_empty();
    let mut col = column![
        ui::mono(
            format!("{} · {}", s.id, s.category),
            12.0,
            crate::theme::palette().ink
        ),
        ui::faint(format!(
            "Proposed by {} · auth {}",
            p.proposer,
            auth_label(&s.auth)
        )),
        ui::faint(s.description.clone()),
        ui::mono(s.base_url.clone(), 11.0, crate::theme::palette().ink_2),
    ]
    .spacing(10)
    .width(Length::Fill);
    if s.docs_url.starts_with("http://") || s.docs_url.starts_with("https://") {
        col = col.push(ui::link("Docs", Msg::OpenDocs(s.docs_url.clone()), false));
    }
    if !s.fields.is_empty() {
        let summary = s
            .fields
            .iter()
            .map(|f| {
                format!(
                    "{}{} · ${}{}",
                    f.label,
                    if f.required { " (required)" } else { "" },
                    f.env,
                    if f.secret { " · secret" } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        col = col.push(ui::faint(format!("Fields: {summary}")));
    }
    if blocked {
        col = col.push(checks_box(&p.checks));
    }
    for a in &s.actions {
        let url = format!("{}{}", s.base_url.trim_end_matches('/'), a.path);
        let schema = serde_json::to_string_pretty(&a.input_schema).unwrap_or_default();
        col = col.push(
            column![
                row![
                    dot(risk_tone(&a.risk)),
                    ui::mono(a.name.clone(), 12.0, crate::theme::palette().ink),
                    ui::faint(a.risk.clone())
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                ui::mono(
                    format!("{} {}", a.method, url),
                    11.0,
                    crate::theme::palette().ink_2
                ),
                ui::faint(a.description.clone()),
                container(ui::mono(schema, 11.0, crate::theme::palette().ink_2))
                    .padding(10)
                    .width(Length::Fill)
                    .style(boxed),
            ]
            .spacing(6),
        );
    }
    let note_input = text_input("Why it is rejected (optional)", note)
        .on_input(Msg::ReviewNote)
        .padding(8)
        .style(theme::input);
    col = col.push(
        column![
            ui::label("Note for the proposer"),
            note_input,
            ui::faint("Optional. Only sent when rejecting.")
        ]
        .spacing(6),
    );
    col = col.push(gated_row(vec![
        ui::btn(
            "Reject",
            None,
            ui::Kind::Ghost,
            gate(state.busy, Msg::Decide(false)),
        ),
        ui::btn(
            "Approve",
            None,
            ui::Kind::Primary,
            if blocked {
                None
            } else {
                gate(state.busy, Msg::Decide(true))
            },
        ),
    ]));
    col.into()
}

// ---- Made by agents -------------------------------------------------------------------------

fn library_card<'a>(t: &LibraryTool) -> Element<'a, Msg> {
    let (text, tone) = match t.state.as_str() {
        "pending" => ("Pending review", ui::Tone::Warn),
        "published" => ("Published", ui::Tone::Ok),
        "rejected" => ("Rejected", ui::Tone::Danger),
        _ => ("Private", ui::Tone::Muted),
    };
    let mut actions = vec![ui::btn(
        "Files",
        None,
        ui::Kind::Secondary,
        Some(Msg::OpenFiles(t.name.clone(), t.version)),
    )];
    if t.state == "pending" {
        actions.push(ui::btn(
            "Publish",
            None,
            ui::Kind::Primary,
            Some(Msg::Publish(t.name.clone(), t.version)),
        ));
        actions.push(ui::btn(
            "Reject",
            None,
            ui::Kind::Ghost,
            Some(Msg::OpenReject(t.name.clone(), t.version)),
        ));
    }
    actions.push(ui::btn(
        "Remove",
        None,
        ui::Kind::Ghost,
        Some(Msg::OpenRemove(t.name.clone(), t.version)),
    ));

    let title = iced::widget::text(format!("x_{}", t.name))
        .font(ui::SANS_SEMIBOLD)
        .size(16.5)
        .color(crate::theme::palette().ink)
        .wrapping(Wrapping::None);
    let mut body = column![
        card_head(None, title.into(), t.description.clone()),
        between(vec![
            ui::faint(format!("by {} · v{}", t.author, t.version)).into(),
            ui::badge(text, tone),
            ui::faint(format!(
                "{} {}",
                t.calls,
                if t.calls == 1 { "call" } else { "calls" }
            ))
            .into(),
        ]),
        ui::mono(t.command.join(" "), 11.0, crate::theme::palette().ink_2),
    ]
    .spacing(12);
    if !t.requires.is_empty() {
        let reqs: Vec<Element<'a, Msg>> = t
            .requires
            .iter()
            .map(|r| chip(ui::sans(r.clone(), 11.0, crate::theme::palette().ink_2)))
            .collect();
        body = body.push(row(reqs).spacing(8));
    }
    if let Some(note) = &t.review_note {
        body = body.push(ui::faint(format!("Note: {note}")));
    }
    body = body.push(card_footer(actions));
    frame(body)
}

fn library_section<'a>(state: &State) -> Element<'a, Msg> {
    let mut col = column![group_title("Made by agents")]
        .spacing(14)
        .width(Length::Fill);
    if state.library.is_empty() {
        col = col.push(ui::empty(
            "models",
            "Agents can build tools in their VM and package them with tool_package; they show up here.",
            "",
        ));
        return col.into();
    }
    let pending: Vec<Element<'a, Msg>> = state
        .library
        .iter()
        .filter(|t| t.state == "pending")
        .map(library_card)
        .collect();
    let rest: Vec<Element<'a, Msg>> = state
        .library
        .iter()
        .filter(|t| t.state != "pending")
        .map(library_card)
        .collect();
    if !pending.is_empty() {
        col = col.push(
            column![
                group_title("Waiting for your review"),
                ui::card_grid(pending)
            ]
            .spacing(10),
        );
    }
    if !rest.is_empty() {
        col = col.push(column![group_title("Library"), ui::card_grid(rest)].spacing(10));
    }
    col.into()
}

fn files_body<'a>(
    state: &State,
    name: &str,
    version: u64,
    shown: &Option<(String, String)>,
) -> Element<'a, Msg> {
    let files = state
        .library
        .iter()
        .find(|t| t.name == name && t.version == version)
        .map(|t| t.files.clone())
        .unwrap_or_default();
    let list: Element<'a, Msg> = if files.is_empty() {
        ui::faint("No files.").into()
    } else {
        let buttons: Vec<Element<'a, Msg>> = files
            .iter()
            .map(|f| {
                ui::btn(
                    &format!("{} · {} B", f.path, f.size),
                    None,
                    ui::Kind::Ghost,
                    Some(Msg::ShowFile(f.path.clone())),
                )
            })
            .collect();
        column(buttons).spacing(6).into()
    };
    let content = match shown {
        Some((_, text)) => text.clone(),
        None => "Pick a file to see its content.".to_string(),
    };
    let viewer = container(ui::sans(content, 12.0, crate::theme::palette().ink_2))
        .padding(12)
        .width(Length::Fill)
        .style(boxed);
    column![list, viewer].spacing(12).into()
}

fn reject_body<'a>(state: &State, note: &str) -> Element<'a, Msg> {
    let note_input = text_input("Why it is rejected (short)", note)
        .on_input(Msg::RejectNote)
        .padding(8)
        .style(theme::input);
    column![
        column![
            ui::label("Note for the author"),
            note_input,
            ui::faint("Optional. The author sees it.")
        ]
        .spacing(6),
        gated_row(vec![ui::btn(
            "Reject",
            None,
            ui::Kind::Primary,
            gate(state.busy, Msg::RejectTool)
        )]),
    ]
    .spacing(14)
    .into()
}

fn remove_tool_body<'a>(state: &State, version: u64) -> Element<'a, Msg> {
    column![
        ui::faint(format!(
            "Removes v{version} and its files from the library. This cannot be undone."
        )),
        gated_row(vec![
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Close)),
            ui::btn(
                "Remove",
                None,
                ui::Kind::Primary,
                gate(state.busy, Msg::RemoveTool)
            ),
        ]),
    ]
    .spacing(14)
    .into()
}

// ---- Found by agents ------------------------------------------------------------------------

/// A kind choice in the found-list picker; an empty `value` is all kinds.
#[derive(Debug, Clone, PartialEq)]
struct KindChoice {
    value: String,
    label: String,
}

impl std::fmt::Display for KindChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// One found tool, as a row over a rule (the web's `.disc-row`).
fn found_row<'a>(d: &Discovery, expanded: bool) -> Element<'a, Msg> {
    let mut name = row![
        text(d.name.clone())
            .font(SANS_SEMIBOLD)
            .size(15.0)
            .color(crate::theme::palette().ink)
            .wrapping(Wrapping::None),
        chip(ui::sans(
            d.kind.clone(),
            11.0,
            crate::theme::palette().ink_2
        )),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if d.flagged.is_some() {
        name = name.push(ui::badge("flagged", ui::Tone::Danger));
    }
    let flag = if d.flagged.is_some() {
        ui::btn(
            "Unflag",
            None,
            ui::Kind::Ghost,
            Some(Msg::Unflag(d.id.clone())),
        )
    } else {
        ui::btn(
            "Flag",
            None,
            ui::Kind::Ghost,
            Some(Msg::OpenFlag(d.id.clone())),
        )
    };
    let actions = row![
        ui::btn(
            "Details",
            None,
            ui::Kind::Ghost,
            Some(Msg::Expand(d.id.clone()))
        ),
        flag,
        ui::btn(
            "Remove",
            None,
            ui::Kind::Danger,
            Some(Msg::OpenRemoveFound(d.id.clone(), d.name.clone()))
        ),
    ]
    .spacing(8);
    let what = container(
        text(d.what_for.clone())
            .font(theme::SANS)
            .size(14.0)
            .color(crate::theme::palette().ink_2)
            .wrapping(Wrapping::None),
    )
    .width(Length::Fill)
    .height(Length::Fixed(18.0))
    .clip(true);
    let by = ui::sans(
        format!("by {} · {} endorsements", d.by, d.endorsed_by.len()),
        11.0,
        crate::theme::palette().ink_3,
    )
    .wrapping(Wrapping::None);
    let summary = column![name, row![what, by].spacing(8).align_y(Alignment::Center)]
        .spacing(4)
        .width(Length::Fill);
    let mut body = column![
        row![summary, actions]
            .spacing(12)
            .align_y(Alignment::Center)
    ]
    .spacing(10);

    if expanded {
        let mut details = column![
            container(ui::sans(d.how.clone(), 12.0, crate::theme::palette().ink_2))
                .padding(10)
                .width(Length::Fill)
                .style(boxed),
            ui::prose(d.verdict.clone()),
        ]
        .spacing(8);
        if !d.tags.is_empty() {
            let tags: Vec<Element<'a, Msg>> = d
                .tags
                .iter()
                .map(|t| chip(ui::sans(t.clone(), 11.0, crate::theme::palette().ink_2)))
                .collect();
            details = details.push(row(tags).spacing(8));
        }
        // Agents write these URLs, so only web links are shown (as the proposal review does).
        if let Some(url) = d
            .url
            .as_ref()
            .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
        {
            details = details.push(ui::mono(url.clone(), 11.0, crate::theme::palette().ink_2));
        }
        body = body.push(details);
    }
    column![
        container(body)
            .padding(iced::Padding::from([10.0, 0.0]))
            .width(Length::Fill),
        rule_line(crate::theme::palette().rule)
    ]
    .width(Length::Fill)
    .into()
}

/// The pager, right-aligned (the web's `.disc-pager`).
fn pager<'a>(page: usize, pages: usize) -> Element<'a, Msg> {
    row![
        Space::new().width(Length::Fill),
        ui::btn(
            "Previous",
            None,
            ui::Kind::Ghost,
            (page > 1).then_some(Msg::Page(-1))
        ),
        ui::sans(
            format!("page {page} of {pages}"),
            11.0,
            crate::theme::palette().ink_3
        ),
        ui::btn(
            "Next",
            None,
            ui::Kind::Ghost,
            (page < pages).then_some(Msg::Page(1))
        ),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .into()
}

fn found_section<'a>(state: &State) -> Element<'a, Msg> {
    let q = state.query.trim().to_lowercase();
    let rows: Vec<&Discovery> = state
        .toolbox
        .iter()
        .filter(|d| (state.kind.is_empty() || d.kind == state.kind) && matches(d, &q))
        .collect();
    let pages = rows.len().div_ceil(DISC_PAGE).max(1);
    let page = state.page.clamp(1, pages);

    // The web's toolbar: one line of search, the kind picker and the count.
    let search = text_input("Search name, use, tags, by", &state.query)
        .on_input(Msg::Query)
        .font(theme::SANS)
        .size(13.0)
        .padding([6, 8])
        .style(theme::input);
    let mut choices = vec![KindChoice {
        value: String::new(),
        label: "All kinds".to_string(),
    }];
    choices.extend(DISC_KINDS.iter().map(|k| KindChoice {
        value: k.to_string(),
        label: k.to_string(),
    }));
    let selected = choices.iter().find(|c| c.value == state.kind).cloned();
    let kind = pick_list(choices, selected, |c: KindChoice| Msg::FilterKind(c.value));
    let toolbar = row![
        container(search).width(Length::Fill),
        kind,
        ui::sans(
            format!("{} found", state.toolbox.len()),
            11.0,
            crate::theme::palette().ink_3
        )
        .wrapping(Wrapping::None),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let list: Element<'a, Msg> = if state.toolbox.is_empty() {
        ui::empty(
            "models",
            "Nothing yet",
            "Agents record what they find with plaza_toolbox_share.",
        )
    } else if rows.is_empty() {
        ui::faint("Nothing matches that search.").into()
    } else {
        let start = (page - 1) * DISC_PAGE;
        let items: Vec<Element<'a, Msg>> = rows
            .iter()
            .skip(start)
            .take(DISC_PAGE)
            .map(|d| found_row(d, state.expanded.contains(&d.id)))
            .collect();
        column![column(items).width(Length::Fill), pager(page, pages)]
            .spacing(10)
            .width(Length::Fill)
            .into()
    };

    column![group_title("Found by agents"), ui::faint("Software, services and techniques agents found on the internet, tried in their VMs and recommend to each other."), toolbar, list]
        .spacing(12)
        .into()
}

// ---- Dialogs --------------------------------------------------------------------------------

/// The open dialog over the screen, as a scrim with a sheet centred on it.
fn modal_layer<'a>(state: &State) -> Option<Element<'a, Msg>> {
    let (title, body) = match state.modal.as_ref()? {
        Modal::Configure {
            id,
            values,
            roles,
            enabled,
        } => (
            format!("Configure {}", connector_label(state, id)),
            configure_body(state, id, values, roles, *enabled),
        ),
        Modal::Review { id, note } => (
            format!("Review {}", proposal_label(state, id)),
            review_body(state, id, note),
        ),
        Modal::Files {
            name,
            version,
            shown,
        } => (
            format!("x_{name} · v{version}"),
            files_body(state, name, *version, shown),
        ),
        Modal::RejectTool { name, note, .. } => {
            (format!("Reject x_{name}"), reject_body(state, note))
        }
        Modal::RemoveTool { name, version } => (
            format!("Remove x_{name}?"),
            remove_tool_body(state, *version),
        ),
        Modal::FlagFound { name, reason, .. } => (
            format!("Flag {name}"),
            column![
                column![
                    ui::label("Reason"),
                    text_input("What is wrong with it?", reason)
                        .on_input(Msg::FlagReason)
                        .on_submit(Msg::FlagSubmit)
                        .padding(8)
                        .style(theme::input),
                    ui::faint("Shown next to the entry."),
                ]
                .spacing(6),
                gated_row(vec![
                    ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Close)),
                    ui::btn(
                        "Save",
                        None,
                        ui::Kind::Primary,
                        gate(state.busy, Msg::FlagSubmit)
                    ),
                ]),
            ]
            .spacing(14)
            .into(),
        ),
        Modal::RemoveFound { name, .. } => (
            "Remove from found tools".to_string(),
            column![
                ui::faint(format!("Remove {name} from the found tools?")),
                gated_row(vec![
                    ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Close)),
                    ui::btn(
                        "Remove",
                        None,
                        ui::Kind::Danger,
                        gate(state.busy, Msg::RemoveFound)
                    ),
                ]),
            ]
            .spacing(14)
            .into(),
        ),
    };
    let mut col = column![ui::sans(title, 16.0, crate::theme::palette().ink), body].spacing(14);
    if let Some(error) = &state.modal_error {
        col = col.push(ui::error_line(error));
    }
    let card = container(col)
        .padding(20)
        .max_width(620.0)
        .style(theme::sheet);
    Some(
        container(card)
            .width(Length::Fill)
            .height(Length::Fill)
            .center(Length::Fill)
            .style(theme::scrim)
            .into(),
    )
}

// ---- The screen -----------------------------------------------------------------------------

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = vec![
        ui::faint("Keys stay in the plaza's vault; agents reach a tool only through the plaza.")
            .into(),
    ];
    if let Some((text, is_error)) = &state.notice {
        parts.push(if *is_error {
            ui::error_line(text)
        } else {
            ui::faint(text.clone()).into()
        });
    }
    if let Some(error) = &state.error {
        parts.push(ui::error_line(error));
    }
    parts.push(connectors_section(state));
    parts.push(proposals_section(state));
    parts.push(library_section(state));
    parts.push(found_section(state));
    let body: Element<'a, Msg> = column(parts).spacing(22).width(Length::Fill).into();
    let body: Element<'a, Msg> = match modal_layer(state) {
        Some(layer) => Stack::with_children(vec![body, layer]).into(),
        None => body,
    };
    // The page scrolls as the web does: a tall list is not squeezed into the window.
    ui::scroll(ui::sheet(ui::view_head("Tools", vec![]), body))
        .height(Length::Fill)
        .into()
}
