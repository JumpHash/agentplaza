//! The E2B API screen (parity with the web console's E2B view): the owner's keys for the hub's E2B-compatible
//! sandbox API (`crates/apid/src/e2b.rs`), the SDK setup lines for a new key, and the sandboxes running now, with
//! pause, resume and end. A key is shown once, when it is made; the hub keeps only its hash. The sandboxes reload
//! every ten seconds while the screen is open, through the screen's own subscription.

use std::time::{Duration, Instant};

use iced::widget::{column, row, text_input};
use iced::{Alignment, Element, Length, Subscription, Task};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Id;
use super::agent;
use super::nodes::Load;
use crate::console::{Ctx, client, ui};

/// The sandboxes reload this often while the screen is open.
const POLL: Duration = Duration::from_secs(10);
/// The sandbox API's port on the hub's own machine, which the SDK's sandbox traffic goes to.
const SANDBOX_URL: &str = "http://127.0.0.1:49982";

/// A key, as `GET /v1/e2b/keys` lists it. The key itself is never listed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Key {
    pub id: String,
    pub label: String,
    pub created_ms: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct KeyList {
    keys: Vec<Key>,
}

/// A key as made (`POST /v1/e2b/keys`): the key, shown once. The rest of the answer is not read.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Issued {
    pub label: String,
    pub key: String,
}

/// A sandbox, as `GET /v1/e2b/sandboxes` lists it (the E2B `ListedSandbox`, with the owner's `cid` and flags).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Sandbox {
    #[serde(rename = "sandboxID")]
    pub sandbox_id: String,
    #[serde(rename = "templateID")]
    pub template_id: String,
    /// `running` or `paused`.
    pub state: String,
    /// RFC 3339 instants.
    #[serde(rename = "startedAt")]
    pub started_at: String,
    #[serde(rename = "endAt")]
    pub end_at: String,
    #[serde(rename = "cpuCount")]
    pub cpu_count: u64,
    #[serde(rename = "memoryMB")]
    pub memory_mb: u64,
    pub cid: Option<u64>,
    #[serde(rename = "allowInternetAccess")]
    pub allow_internet_access: bool,
    #[serde(rename = "autoPause")]
    pub auto_pause: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SandboxList {
    sandboxes: Vec<Sandbox>,
}

/// A template, as `GET /v1/e2b/templates` lists it: its builds, newest first.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Template {
    #[serde(rename = "templateID")]
    pub template_id: String,
    pub name: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    pub builds: Vec<Build>,
}

/// One build of a template. `status` is `waiting`, `building`, `ready` or `error`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Build {
    #[serde(rename = "buildID")]
    pub build_id: String,
    pub status: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "finishedAt")]
    pub finished_at: Option<String>,
    #[serde(rename = "cpuCount")]
    pub cpu_count: u64,
    #[serde(rename = "memoryMB")]
    pub memory_mb: u64,
    pub tags: Vec<String>,
    /// Why the build failed: the message, and the step it failed on. Null unless it failed.
    pub reason: Option<Reason>,
}

/// A failed build's reason.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Reason {
    pub message: String,
    pub step: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct TemplateList {
    templates: Vec<Template>,
}

/// The three lines that point an E2B SDK at this hub and this key, as the web's setup shows them. `hub` is the hub's
/// base URL.
pub fn setup_text(hub: &str, key: &str) -> String {
    format!(
        "E2B_API_URL={}/e2b\nE2B_API_KEY={key}\nE2B_SANDBOX_URL={SANDBOX_URL}",
        hub.trim_end_matches('/')
    )
}

/// A sandbox's facts under its id: its template, the agent it runs for, and its network and pause settings.
pub fn sandbox_facts(sandbox: &Sandbox) -> String {
    let mut parts = vec![sandbox.template_id.clone()];
    if let Some(cid) = sandbox.cid {
        parts.push(format!("agent {cid}"));
    }
    if sandbox.auto_pause {
        parts.push("auto-pause".to_string());
    }
    parts.push(
        if sandbox.allow_internet_access {
            "internet on"
        } else {
            "no internet"
        }
        .to_string(),
    );
    parts.join(" · ")
}

/// The badge for a sandbox's state: its words and its tone.
pub fn state_badge(state: &str) -> (String, ui::Tone) {
    let tone = match state {
        "running" => ui::Tone::Ok,
        "paused" => ui::Tone::Warn,
        _ => ui::Tone::Muted,
    };
    (state.to_string(), tone)
}

/// An RFC 3339 instant as `YYYY-MM-DD HH:MM` (UTC, as the hub writes it); the text as given when it is not one.
pub fn short_time(rfc3339: &str) -> String {
    match rfc3339.get(..16) {
        Some(head) if head.as_bytes().get(10) == Some(&b'T') => head.replace('T', " "),
        _ => rfc3339.to_string(),
    }
}

/// The badge for a build's status: its words and its tone.
pub fn build_badge(status: &str) -> (String, ui::Tone) {
    let tone = match status {
        "ready" => ui::Tone::Ok,
        "building" | "waiting" => ui::Tone::Warn,
        "error" => ui::Tone::Danger,
        _ => ui::Tone::Muted,
    };
    (status.to_string(), tone)
}

/// The tags of a template's ready builds, each once, in the order the builds list them.
pub fn ready_tags(builds: &[Build]) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for build in builds.iter().filter(|build| build.status == "ready") {
        for tag in &build.tags {
            if !tags.contains(tag) {
                tags.push(tag.clone());
            }
        }
    }
    tags
}

/// A template's id, shortened to its first twelve characters for the list.
pub fn short_id(id: &str) -> String {
    id.chars().take(12).collect()
}

#[derive(Debug, Default)]
pub struct State {
    pub keys: Load<Vec<Key>>,
    pub sandboxes: Load<Vec<Sandbox>>,
    pub templates: Load<Vec<Template>>,
    /// When the sandboxes last loaded. The screen's own poll reloads them every `POLL`; a load on the shell's tick
    /// reloads them only when that is due.
    pub sandboxes_at: Option<Instant>,
    /// The label typed for the next key.
    pub label: String,
    /// The key just made, shown once. Leaving the screen drops it.
    pub issued: Option<Issued>,
    /// A write is on its way to the hub.
    pub busy: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
    /// The key waiting for its revoke confirmation, and the sandbox waiting for its end confirmation.
    pub confirm_revoke: Option<String>,
    pub confirm_end: Option<String>,
    /// The template waiting for its delete confirmation.
    pub confirm_delete_template: Option<String>,
}

/// Drops the new key from the page. Called when the owner leaves the screen.
pub fn leave(state: &mut State) {
    state.issued = None;
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    KeysLoaded(Result<Vec<Key>, String>),
    SandboxesLoaded(Result<Vec<Sandbox>, String>),
    TemplatesLoaded(Result<Vec<Template>, String>),
    /// The screen's ten-second tick: the sandboxes reload.
    Poll,
    Label(String),
    Create,
    Created(Result<Issued, String>),
    /// Drops the key from the page once it is copied.
    DismissKey,
    /// Copies a value to the clipboard.
    Copy(String),
    AskRevoke(String),
    CancelRevoke,
    Revoke(String),
    AskEnd(String),
    CancelEnd,
    End(String),
    Pause(String),
    Resume(String),
    AskDeleteTemplate(String),
    CancelDeleteTemplate,
    /// Deletes a template (its DELETE, then the reload that `Acted` runs).
    DeleteTemplate(String),
    /// A write finished: its result and the line to confirm it with.
    Acted(Result<(), String>, String),
}

pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

fn settle<T>(result: Result<T, String>) -> Load<T> {
    match result {
        Ok(value) => Load::Ready(value),
        Err(_) => Load::Failed,
    }
}

fn keys_task(ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(
        move || client.get::<KeyList>("/v1/e2b/keys").map(|list| list.keys),
        Msg::KeysLoaded,
    )
}

fn sandboxes_task(ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(
        move || {
            client
                .get::<SandboxList>("/v1/e2b/sandboxes")
                .map(|list| list.sandboxes)
        },
        Msg::SandboxesLoaded,
    )
}

fn templates_task(ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(
        move || {
            client
                .get::<TemplateList>("/v1/e2b/templates")
                .map(|list| list.templates)
        },
        Msg::TemplatesLoaded,
    )
}

/// Loads the keys and the templates, and the sandboxes when they are due (called when the screen opens and on the
/// shell's tick).
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let mut tasks = vec![keys_task(ctx), templates_task(ctx)];
    if state.sandboxes_at.is_none_or(|at| at.elapsed() >= POLL) {
        tasks.push(sandboxes_task(ctx));
    }
    Task::batch(tasks)
}

/// The sandboxes' ten-second poll, while the screen is open.
pub fn subscription() -> Subscription<Msg> {
    iced::time::every(POLL).map(|_| Msg::Poll)
}

/// One write to the hub: it is refused while another runs. Its confirmation is `notice`.
fn act(
    state: &mut State,
    ctx: &Ctx,
    method: &'static str,
    path: String,
    body: Option<Value>,
    notice: &'static str,
) -> Task<Msg> {
    if state.busy {
        return Task::none();
    }
    state.busy = true;
    state.confirm_revoke = None;
    state.confirm_end = None;
    state.confirm_delete_template = None;
    let client = ctx.client.clone();
    client::task(
        move || client.call(method, &path, body.as_ref()).map(|_| ()),
        move |result| Msg::Acted(result, notice.to_string()),
    )
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::KeysLoaded(result) => {
            state.keys = settle(result);
            Task::none()
        }
        Msg::SandboxesLoaded(result) => {
            if result.is_ok() {
                state.sandboxes_at = Some(Instant::now());
            }
            state.sandboxes = settle(result);
            Task::none()
        }
        Msg::TemplatesLoaded(result) => {
            state.templates = settle(result);
            Task::none()
        }
        Msg::Poll => Task::batch(vec![sandboxes_task(ctx), templates_task(ctx)]),
        Msg::Label(text) => {
            state.label = text.chars().take(60).collect();
            Task::none()
        }
        Msg::Create => {
            if state.busy {
                return Task::none();
            }
            let label = state.label.trim().to_string();
            if label.is_empty() {
                state.error = Some("Give the key a label first.".to_string());
                return Task::none();
            }
            state.busy = true;
            state.error = None;
            let body = json!({ "label": label });
            let client = ctx.client.clone();
            client::task(
                move || client.post::<Issued>("/v1/e2b/keys", &body),
                Msg::Created,
            )
        }
        Msg::Created(Ok(made)) => {
            state.busy = false;
            state.label.clear();
            state.error = None;
            state.issued = Some(made);
            load(state, ctx)
        }
        Msg::Created(Err(error)) => {
            state.busy = false;
            state.error = Some(error);
            Task::none()
        }
        Msg::DismissKey => {
            state.issued = None;
            Task::none()
        }
        Msg::Copy(value) => {
            state.notice = Some("Copied".to_string());
            iced::clipboard::write(value)
        }
        Msg::AskRevoke(id) => {
            state.confirm_revoke = Some(id);
            state.error = None;
            Task::none()
        }
        Msg::CancelRevoke => {
            state.confirm_revoke = None;
            Task::none()
        }
        Msg::Revoke(id) => act(
            state,
            ctx,
            "DELETE",
            format!("/v1/e2b/keys/{id}"),
            None,
            "Key revoked",
        ),
        Msg::AskEnd(id) => {
            state.confirm_end = Some(id);
            state.error = None;
            Task::none()
        }
        Msg::CancelEnd => {
            state.confirm_end = None;
            Task::none()
        }
        Msg::End(id) => act(
            state,
            ctx,
            "DELETE",
            format!("/v1/e2b/sandboxes/{id}"),
            None,
            "Sandbox ended",
        ),
        Msg::Pause(id) => act(
            state,
            ctx,
            "POST",
            format!("/v1/e2b/sandboxes/{id}/pause"),
            None,
            "Sandbox paused",
        ),
        Msg::Resume(id) => act(
            state,
            ctx,
            "POST",
            format!("/v1/e2b/sandboxes/{id}/resume"),
            Some(json!({})),
            "Sandbox resumed",
        ),
        Msg::AskDeleteTemplate(id) => {
            state.confirm_delete_template = Some(id);
            state.error = None;
            Task::none()
        }
        Msg::CancelDeleteTemplate => {
            state.confirm_delete_template = None;
            Task::none()
        }
        Msg::DeleteTemplate(id) => act(
            state,
            ctx,
            "DELETE",
            format!("/v1/e2b/templates/{id}"),
            None,
            "Template deleted",
        ),
        Msg::Acted(result, notice) => {
            state.busy = false;
            match result {
                Ok(()) => {
                    state.error = None;
                    state.notice = Some(notice);
                }
                Err(error) => {
                    state.notice = None;
                    state.error = Some(error);
                }
            }
            // The sandboxes change with the write, so they are read again now, not on the next poll.
            state.sandboxes_at = None;
            load(state, ctx)
        }
    }
}

// ---- The view ---------------------------------------------------------------------------------------------

/// The button's message, unless a write is running.
fn gate(busy: bool, msg: Msg) -> Option<Msg> {
    if busy { None } else { Some(msg) }
}

/// The screen. `hub` is the hub's base URL, which the setup lines name.
pub fn view<'a>(state: &'a State, hub: &str) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = vec![
        ui::faint("Keys let an E2B SDK create and drive sandboxes on this hub. The sandboxes below are the ones running now.")
            .into(),
    ];
    if let Some(error) = &state.error {
        parts.push(ui::error_line(error));
    }
    if let Some(notice) = &state.notice {
        parts.push(ui::faint(notice.clone()).into());
    }
    if let Some(made) = &state.issued {
        parts.push(ui::card("New key", issued_view(made, hub)));
    }
    parts.push(ui::card("Keys", keys_card(state)));
    parts.push(ui::card("Sandboxes", sandboxes_view(state)));
    parts.push(ui::card("Templates", templates_view(state)));
    let head = ui::view_head("E2B API", vec![]);
    ui::sheet(head, column(parts).spacing(22).width(Length::Fill).into())
}

/// The key just made: the key with its copy, the warning that it is shown once, and the SDK setup lines.
fn issued_view<'a>(made: &Issued, hub: &str) -> Element<'a, Msg> {
    column![
        ui::faint(format!("Key for {}", made.label)),
        ui::faint(
            "Shown once. Copy it now: the hub keeps only its hash, so it cannot be shown again."
        )
        .color(crate::theme::palette().warn),
        copy_box(&made.key),
        ui::label("Point an E2B SDK at this hub"),
        copy_box(&setup_text(hub, &made.key)),
        ui::btn("Close", None, ui::Kind::Secondary, Some(Msg::DismissKey)),
    ]
    .spacing(10)
    .width(Length::Fill)
    .into()
}

/// A value to copy: its text, and a Copy button beside it.
fn copy_box<'a>(value: &str) -> Element<'a, Msg> {
    row![
        ui::mono(value.to_string(), 12.0, crate::theme::palette().ink).width(Length::Fill),
        ui::btn_sm(
            "Copy",
            None,
            ui::Kind::Ghost,
            Some(Msg::Copy(value.to_string()))
        ),
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into()
}

fn keys_card(state: &State) -> Element<'_, Msg> {
    let form = row![
        text_input("ci-runner", &state.label)
            .on_input(Msg::Label)
            .on_submit(Msg::Create)
            .padding(8.0)
            .style(crate::theme::input)
            .width(Length::Fill),
        ui::btn(
            "Create key",
            None,
            ui::Kind::Primary,
            gate(state.busy, Msg::Create)
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let list: Element<'_, Msg> = match &state.keys {
        Load::Pending => ui::loading(),
        Load::Failed => ui::empty("alert", "Could not load the keys.", ""),
        Load::Ready(list) if list.is_empty() => ui::empty(
            "key",
            "No keys yet",
            "Make one above, then point an E2B SDK at this hub with it.",
        ),
        Load::Ready(list) => column(
            list.iter()
                .map(|key| key_row(state, key))
                .collect::<Vec<_>>(),
        )
        .spacing(14)
        .width(Length::Fill)
        .into(),
    };
    ui::card(
        "Keys",
        column![
            form,
            ui::faint("The label names the key here. Revoking a key ends the SDK calls that use it at once."),
            list,
        ]
        .spacing(14)
        .width(Length::Fill)
        .into(),
    )
}

fn key_row<'a>(state: &State, key: &Key) -> Element<'a, Msg> {
    let label = if key.label.is_empty() {
        "(no label)".to_string()
    } else {
        key.label.clone()
    };
    let facts = format!("{} · created {}", key.id, agent::ago(key.created_ms));
    let action: Element<'a, Msg> = if state.confirm_revoke.as_deref() == Some(key.id.as_str()) {
        let confirm: Vec<Element<'a, Msg>> = vec![
            ui::faint(format!(
                "Revoke {label}? SDK calls that use it stop at once. This cannot be undone."
            ))
            .into(),
            ui::btn_sm("Cancel", None, ui::Kind::Ghost, Some(Msg::CancelRevoke)),
            ui::btn_sm(
                "Revoke",
                None,
                ui::Kind::Danger,
                gate(state.busy, Msg::Revoke(key.id.clone())),
            ),
        ];
        row(confirm).spacing(10).align_y(Alignment::Center).into()
    } else {
        ui::btn_sm(
            "Revoke",
            None,
            ui::Kind::Danger,
            gate(state.busy, Msg::AskRevoke(key.id.clone())),
        )
    };
    column![
        row![
            ui::mono(label, 12.0, crate::theme::palette().ink),
            ui::faint(facts),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
        action,
    ]
    .spacing(8)
    .into()
}

/// The sandboxes table, or its empty or failed state.
fn sandboxes_view(state: &State) -> Element<'_, Msg> {
    match &state.sandboxes {
        Load::Pending => ui::loading(),
        Load::Failed => ui::empty("alert", "Could not load the sandboxes.", ""),
        Load::Ready(list) if list.is_empty() => ui::empty(
            "square-code",
            "No sandboxes running",
            "The sandboxes an SDK creates on this hub are listed here.",
        ),
        Load::Ready(list) => table(
            &[
                (3, "Sandbox"),
                (2, "State"),
                (2, "Resources"),
                (2, "Started"),
                (2, "Ends"),
                (3, ""),
            ],
            list.iter()
                .map(|sandbox| sandbox_row(state, sandbox))
                .collect(),
        ),
    }
}

fn sandbox_row<'a>(state: &State, sandbox: &Sandbox) -> Element<'a, Msg> {
    let (label, tone) = state_badge(&sandbox.state);
    cells(vec![
        (
            3,
            column![
                ui::mono(
                    sandbox.sandbox_id.clone(),
                    12.0,
                    crate::theme::palette().ink
                ),
                ui::faint(sandbox_facts(sandbox)),
            ]
            .spacing(2)
            .into(),
        ),
        (2, ui::badge(&label, tone)),
        (
            2,
            ui::sans(
                format!("{} CPU · {} MB", sandbox.cpu_count, sandbox.memory_mb),
                12.0,
                crate::theme::palette().ink,
            )
            .into(),
        ),
        (2, ui::faint(short_time(&sandbox.started_at)).into()),
        (2, ui::faint(short_time(&sandbox.end_at)).into()),
        (3, sandbox_actions(state, sandbox)),
    ])
}

fn sandbox_actions<'a>(state: &State, sandbox: &Sandbox) -> Element<'a, Msg> {
    let id = sandbox.sandbox_id.clone();
    if state.confirm_end.as_deref() == Some(id.as_str()) {
        let confirm: Vec<Element<'a, Msg>> = vec![
            ui::faint("End it? It stops at once and cannot be resumed.").into(),
            ui::btn_sm("Cancel", None, ui::Kind::Ghost, Some(Msg::CancelEnd)),
            ui::btn_sm(
                "End",
                None,
                ui::Kind::Danger,
                gate(state.busy, Msg::End(id)),
            ),
        ];
        return row(confirm).spacing(8).align_y(Alignment::Center).into();
    }
    let mut buttons: Vec<Element<'a, Msg>> = Vec::new();
    match sandbox.state.as_str() {
        "running" => buttons.push(ui::btn_sm(
            "Pause",
            None,
            ui::Kind::Secondary,
            gate(state.busy, Msg::Pause(id.clone())),
        )),
        "paused" => buttons.push(ui::btn_sm(
            "Resume",
            None,
            ui::Kind::Secondary,
            gate(state.busy, Msg::Resume(id.clone())),
        )),
        _ => {}
    }
    buttons.push(ui::btn_sm(
        "End",
        None,
        ui::Kind::Danger,
        gate(state.busy, Msg::AskEnd(id)),
    ));
    row(buttons).spacing(8).align_y(Alignment::Center).into()
}

/// The templates table, or its empty or failed state.
fn templates_view(state: &State) -> Element<'_, Msg> {
    match &state.templates {
        Load::Pending => ui::loading(),
        Load::Failed => ui::empty("alert", "Could not load the templates.", ""),
        Load::Ready(list) if list.is_empty() => ui::empty(
            "square-code",
            "No templates yet",
            "Templates are built with the E2B SDK's Template.build, then listed here.",
        ),
        Load::Ready(list) => table(
            &[
                (3, "Template"),
                (3, "Newest build"),
                (2, "Tags"),
                (2, "Resources"),
                (2, "Updated"),
                (2, ""),
            ],
            list.iter()
                .map(|template| template_row(state, template))
                .collect(),
        ),
    }
}

fn template_row<'a>(state: &State, template: &Template) -> Element<'a, Msg> {
    let name = if template.name.is_empty() {
        "(no name)".to_string()
    } else {
        template.name.clone()
    };
    let newest = template.builds.first();
    let build: Element<'a, Msg> = match newest {
        None => ui::faint("No build yet").into(),
        Some(build) => {
            let (label, tone) = build_badge(&build.status);
            let mut parts: Vec<Element<'a, Msg>> = vec![ui::badge(&label, tone)];
            if let Some(reason) = build.reason.as_ref().filter(|_| build.status == "error") {
                let text = if reason.step.is_empty() {
                    reason.message.clone()
                } else {
                    format!("{}: {}", reason.step, reason.message)
                };
                parts.push(ui::faint(text).into());
            }
            column(parts).spacing(4).into()
        }
    };
    let resources: Element<'a, Msg> = match newest {
        Some(build) => ui::sans(
            format!("{} CPU · {} MB", build.cpu_count, build.memory_mb),
            12.0,
            crate::theme::palette().ink,
        )
        .into(),
        None => ui::faint("none").into(),
    };
    let tags = ready_tags(&template.builds);
    let tag_text = if tags.is_empty() {
        "none".to_string()
    } else {
        tags.join(", ")
    };
    cells(vec![
        (
            3,
            column![
                ui::mono(name, 12.0, crate::theme::palette().ink),
                ui::faint(short_id(&template.template_id)),
            ]
            .spacing(2)
            .into(),
        ),
        (3, build),
        (2, ui::faint(tag_text).into()),
        (2, resources),
        (2, ui::faint(short_time(&template.updated_at)).into()),
        (2, template_actions(state, &template.template_id)),
    ])
}

/// A template's Delete button, which asks first: the confirm line with its Cancel and Delete.
fn template_actions<'a>(state: &State, template_id: &str) -> Element<'a, Msg> {
    let id = template_id.to_string();
    if state.confirm_delete_template.as_deref() == Some(id.as_str()) {
        let confirm: Vec<Element<'a, Msg>> = vec![
            ui::faint("Delete it? This cannot be undone.").into(),
            ui::btn_sm("Cancel", None, ui::Kind::Ghost, Some(Msg::CancelDeleteTemplate)),
            ui::btn_sm(
                "Delete",
                None,
                ui::Kind::Danger,
                gate(state.busy, Msg::DeleteTemplate(id)),
            ),
        ];
        return row(confirm).spacing(8).align_y(Alignment::Center).into();
    }
    ui::btn_sm(
        "Delete",
        None,
        ui::Kind::Danger,
        gate(state.busy, Msg::AskDeleteTemplate(id)),
    )
}

/// A table: a mono header row, then the rows, each under a hairline. Columns are proportional.
fn table<'a>(head: &[(u16, &str)], rows: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let heads: Vec<(u16, Element<'a, Msg>)> = head
        .iter()
        .map(|(portion, label)| (*portion, ui::label(label).into()))
        .collect();
    let mut lines: Vec<Element<'a, Msg>> = vec![cells(heads), ui::hairline()];
    for line in rows {
        lines.push(line);
        lines.push(ui::hairline());
    }
    column(lines).width(Length::Fill).into()
}

/// One table row: its cells in proportional columns.
fn cells<'a>(parts: Vec<(u16, Element<'a, Msg>)>) -> Element<'a, Msg> {
    let laid: Vec<Element<'a, Msg>> = parts
        .into_iter()
        .map(|(portion, cell)| {
            iced::widget::container(cell)
                .width(Length::FillPortion(portion))
                .into()
        })
        .collect();
    row(laid).spacing(12).align_y(Alignment::Center).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e2b_keys_parse_from_the_owner_api() {
        let list: KeyList =
            serde_json::from_str(r#"{"keys":[{"id":"e2b_1","label":"ci","created_ms":5}]}"#)
                .expect("the keys parse");
        assert_eq!(list.keys[0].label, "ci");
        assert_eq!(list.keys[0].created_ms, 5);
    }

    #[test]
    fn e2b_key_as_made_carries_the_key_once() {
        let made: Issued = serde_json::from_str(
            r#"{"id":"e2b_2","label":"ci","key":"e2b_abc123","created_ms":6}"#,
        )
        .expect("the issued key parses");
        assert_eq!(made.key, "e2b_abc123");
        assert_eq!(made.label, "ci");
    }

    #[test]
    fn e2b_sandbox_facts_name_the_agent_and_the_settings() {
        let sandbox = Sandbox {
            template_id: "base".to_string(),
            cid: Some(7),
            auto_pause: true,
            allow_internet_access: false,
            ..Sandbox::default()
        };
        assert_eq!(
            sandbox_facts(&sandbox),
            "base · agent 7 · auto-pause · no internet"
        );
        let bare = Sandbox {
            template_id: "base".to_string(),
            allow_internet_access: true,
            ..Sandbox::default()
        };
        assert_eq!(sandbox_facts(&bare), "base · internet on");
    }

    #[test]
    fn e2b_sandboxes_parse_with_the_owner_fields() {
        let list: SandboxList = serde_json::from_str(
            r#"{"sandboxes":[{"sandboxID":"sb_1","templateID":"base","clientID":"c","state":"paused",
                "startedAt":"2026-10-15T12:34:56Z","endAt":"2026-10-15T13:34:56Z","cpuCount":2,
                "memoryMB":512,"diskSizeMB":0,"metadata":{"k":"v"},"cid":7,"run":"run_1",
                "allowInternetAccess":false,"autoPause":true}]}"#,
        )
        .expect("the sandboxes parse");
        let sandbox = &list.sandboxes[0];
        assert_eq!(sandbox.sandbox_id, "sb_1");
        assert_eq!(sandbox.template_id, "base");
        assert_eq!(sandbox.state, "paused");
        assert_eq!(sandbox.cpu_count, 2);
        assert_eq!(sandbox.memory_mb, 512);
        assert_eq!(sandbox.cid, Some(7));
        assert!(sandbox.auto_pause);
        assert!(!sandbox.allow_internet_access);
    }

    #[test]
    fn e2b_sandbox_without_a_cid_still_parses() {
        let list: SandboxList =
            serde_json::from_str(r#"{"sandboxes":[{"sandboxID":"sb_2","cid":null}]}"#)
                .expect("a sandbox with a null cid parses");
        assert_eq!(list.sandboxes[0].cid, None);
    }

    #[test]
    fn e2b_setup_lines_point_an_sdk_at_the_hub_and_the_key() {
        assert_eq!(
            setup_text("https://hub.example.test", "e2b_key"),
            "E2B_API_URL=https://hub.example.test/e2b\nE2B_API_KEY=e2b_key\nE2B_SANDBOX_URL=http://127.0.0.1:49982"
        );
        assert_eq!(
            setup_text("https://hub.example.test/", "k").lines().next(),
            Some("E2B_API_URL=https://hub.example.test/e2b")
        );
    }

    #[test]
    fn e2b_state_badges_read_running_and_paused() {
        assert_eq!(
            state_badge("running"),
            ("running".to_string(), ui::Tone::Ok)
        );
        assert_eq!(
            state_badge("paused"),
            ("paused".to_string(), ui::Tone::Warn)
        );
        assert_eq!(state_badge("starting").1, ui::Tone::Muted);
    }

    #[test]
    fn e2b_times_read_as_minutes_in_utc() {
        assert_eq!(short_time("2026-10-15T12:34:56Z"), "2026-10-15 12:34");
        assert_eq!(short_time(""), "");
        assert_eq!(short_time("soon"), "soon");
    }

    #[test]
    fn e2b_templates_parse_with_their_builds() {
        let list: TemplateList = serde_json::from_str(
            r#"{"templates":[{"templateID":"tpl_1","name":"base","createdAt":"2026-10-14T09:00:00Z",
                "updatedAt":"2026-10-15T12:34:56Z","builds":[
                {"buildID":"b_2","status":"error","createdAt":"2026-10-15T12:00:00Z",
                 "finishedAt":"2026-10-15T12:05:00Z","cpuCount":2,"memoryMB":512,"tags":["latest"],
                 "reason":{"message":"out of disk","step":"RUN apt-get"}},
                {"buildID":"b_1","status":"ready","createdAt":"2026-10-14T09:00:00Z",
                 "finishedAt":null,"cpuCount":1,"memoryMB":256,"tags":["v1","latest"],"reason":null}]}]}"#,
        )
        .expect("the templates parse");
        let template = &list.templates[0];
        assert_eq!(template.template_id, "tpl_1");
        assert_eq!(template.name, "base");
        assert_eq!(template.builds.len(), 2);
        let failed = &template.builds[0];
        assert_eq!(failed.status, "error");
        assert_eq!(failed.finished_at.as_deref(), Some("2026-10-15T12:05:00Z"));
        assert_eq!(
            failed.reason.as_ref().map(|reason| reason.step.as_str()),
            Some("RUN apt-get")
        );
        assert_eq!(failed.memory_mb, 512);
        let ready = &template.builds[1];
        assert!(ready.finished_at.is_none());
        assert!(ready.reason.is_none());
    }

    #[test]
    fn e2b_templates_parse_without_reason_or_finish_time() {
        let list: TemplateList = serde_json::from_str(
            r#"{"templates":[{"templateID":"tpl_2","name":"slim","builds":[{"buildID":"b_3","status":"building","tags":[]}]}]}"#,
        )
        .expect("a template without the optional fields parses");
        let build = &list.templates[0].builds[0];
        assert_eq!(build.status, "building");
        assert!(build.finished_at.is_none());
        assert!(build.reason.is_none());
        assert_eq!(list.templates[0].updated_at, "");
    }

    #[test]
    fn e2b_ready_builds_name_the_tags_once() {
        let build = |status: &str, tags: &[&str]| Build {
            status: status.to_string(),
            tags: tags.iter().map(|tag| tag.to_string()).collect(),
            ..Build::default()
        };
        let builds = vec![
            build("ready", &["latest", "v2"]),
            build("error", &["broken"]),
            build("ready", &["v2", "v1"]),
            build("building", &["next"]),
        ];
        assert_eq!(ready_tags(&builds), vec!["latest", "v2", "v1"]);
        assert!(ready_tags(&[]).is_empty());
    }

    #[test]
    fn e2b_leaving_the_screen_drops_the_key() {
        let mut state = State {
            issued: Some(Issued {
                label: "ci".to_string(),
                key: "e2b_secret".to_string(),
            }),
            ..Default::default()
        };
        leave(&mut state);
        assert!(state.issued.is_none());
    }
}
