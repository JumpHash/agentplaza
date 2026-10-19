//! The E2B API screen (parity with the web console's E2B view): the owner's keys for the hub's E2B-compatible
//! sandbox API (`crates/apid/src/e2b.rs`), the SDK setup lines for a new key, and the sandboxes running now, with
//! pause, resume and end. A key is shown once, when it is made; the hub keeps only its hash. The sandboxes reload
//! every ten seconds while the screen is open, through the screen's own subscription.

use std::time::{Duration, Instant};

use iced::alignment::{Horizontal, Vertical};
use iced::widget::{Space, column, container, row, text, text_input};
use iced::{Alignment, Border, Color, Element, Length, Padding, Subscription, Task, Theme};
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

/// An RFC 3339 instant as `YYYY-MM-DD HH:MM` (UTC, as the hub writes it); the text as given when it is not one.
pub fn short_time(rfc3339: &str) -> String {
    match rfc3339.get(..16) {
        Some(head) if head.as_bytes().get(10) == Some(&b'T') => head.replace('T', " "),
        _ => rfc3339.to_string(),
    }
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

/// The screen, as the web's E2B API page: the keys (with the key just made, and the form for a new one), the setup
/// an SDK client reads, then the sandboxes and the templates. `hub` is the hub's base URL, which the setup lines name.
pub fn view<'a>(state: &'a State, hub: &str) -> Element<'a, Msg> {
    let head = ui::view_head("E2B API", Vec::new());
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(error) = &state.error {
        parts.push(ui::error_line(error));
    }
    if let Some(notice) = &state.notice {
        parts.push(ui::faint(notice.clone()).into());
    }
    parts.push(ui::card("Keys", keys_card(state)));
    parts.push(ui::card("Setup for SDK clients", setup_view(state, hub)));
    parts.push(flush_card("Sandboxes", sandboxes_body(state)));
    parts.push(flush_card("Templates", templates_body(state)));
    ui::sheet(head, column(parts).spacing(16).width(Length::Fill).into())
}

/// The Keys card: the table of keys (with the revoke confirmation under it when one is asked), the key just made,
/// and the form for a new key.
fn keys_card(state: &State) -> Element<'_, Msg> {
    let p = crate::theme::palette();
    let mut parts: Vec<Element<'_, Msg>> = Vec::new();
    match &state.keys {
        Load::Pending => parts.push(ui::loading()),
        Load::Failed => parts.push(ui::empty("alert", "Could not load the keys", "")),
        Load::Ready(list) if list.is_empty() => parts.push(ui::empty(
            "key",
            "No keys",
            "Make a key below for an SDK client.",
        )),
        Load::Ready(list) => {
            let cols = [
                Col {
                    head: "Label",
                    share: Some(3),
                    right: false,
                },
                Col {
                    head: "Created",
                    share: Some(2),
                    right: false,
                },
                Col {
                    head: "",
                    share: None,
                    right: true,
                },
            ];
            let rows: Vec<Vec<Element<'_, Msg>>> = list
                .iter()
                .map(|k| {
                    let label = if k.label.is_empty() {
                        "(no label)".to_string()
                    } else {
                        k.label.clone()
                    };
                    vec![
                        ui::sans(label, 13.0, p.ink).into(),
                        ui::faint(agent::ago(k.created_ms)).into(),
                        ui::btn_sm(
                            "Revoke",
                            Some("trash"),
                            ui::Kind::Danger,
                            gate(state.busy, Msg::AskRevoke(k.id.clone())),
                        ),
                    ]
                })
                .collect();
            parts.push(table(0.0, &cols, rows));
            if let Some(key) = state
                .confirm_revoke
                .as_deref()
                .and_then(|id| list.iter().find(|k| k.id == id))
            {
                parts.push(confirm(
                    &format!("Revoke {}", key.label),
                    ui::faint("Clients that use this key stop working. This cannot be undone.")
                        .into(),
                    Msg::CancelRevoke,
                    "Revoke",
                    gate(state.busy, Msg::Revoke(key.id.clone())),
                ));
            }
        }
    }
    if let Some(made) = &state.issued {
        parts.push(reveal_box(made));
    }
    let form = row![
        text("New key")
            .font(crate::theme::SANS_SEMIBOLD)
            .size(13.0)
            .color(p.ink),
        column![
            ui::label("Label"),
            text_input("laptop-sdk", &state.label)
                .on_input(Msg::Label)
                .on_submit(Msg::Create)
                .padding(8.0)
                .style(crate::theme::input)
                .width(Length::Fill),
            ui::faint("What this client is, such as ci or laptop-sdk."),
        ]
        .spacing(6)
        .width(Length::Fill),
        ui::btn(
            "Create key",
            Some("plus"),
            ui::Kind::Primary,
            gate(state.busy, Msg::Create),
        ),
    ]
    .spacing(16)
    .align_y(Alignment::End);
    parts.push(form.into());
    column(parts).spacing(16).width(Length::Fill).into()
}

/// The key just made, shown once: its label, the key in a box with Copy and Hide. Hide drops it from the page.
fn reveal_box<'a>(made: &Issued) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let fill = p.signal_soft;
    let radius = crate::theme::skin().radius;
    container(
        column![
            ui::sans(format!("Key for {}", made.label), 13.0, p.ink),
            env_box(made.key.clone()),
            row![
                ui::btn_sm(
                    "Copy key",
                    None,
                    ui::Kind::Ghost,
                    Some(Msg::Copy(made.key.clone()))
                ),
                ui::btn_sm("Hide", None, ui::Kind::Ghost, Some(Msg::DismissKey)),
            ]
            .spacing(8),
        ]
        .spacing(12),
    )
    .padding(16)
    .width(Length::Fill)
    .style(move |_: &Theme| container::Style {
        background: Some(fill.into()),
        border: Border {
            radius: radius.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .into()
}

/// The mono box the web's `.e2b-env` draws: the text on the sheet's second ground, broken where it does not fit.
fn env_box<'a>(value: String) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let fill = p.sheet_2;
    let radius = crate::theme::skin().radius;
    container(ui::mono(value, 12.0, p.ink).wrapping(text::Wrapping::Glyph))
        .padding(Padding {
            top: 8.0,
            right: 12.0,
            bottom: 8.0,
            left: 12.0,
        })
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(fill.into()),
            border: Border {
                radius: radius.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

/// The setup an SDK client reads: the three lines with the key (or its placeholder), a Copy, and the note on the key.
fn setup_view<'a>(state: &'a State, hub: &str) -> Element<'a, Msg> {
    let key = state
        .issued
        .as_ref()
        .map_or("<your key>", |made| made.key.as_str());
    let setup = setup_text(hub, key);
    let note = if state.issued.is_some() {
        "The key above is in this setup. It lives only on this screen, so copy it into the client before you leave."
    } else {
        "Put a key in E2B_API_KEY. Make one above; it is shown once."
    };
    column![
        env_box(setup.clone()),
        ui::faint("E2B_SANDBOX_URL is the hub machine's own local sandbox gateway, so it is the same on every client that runs there. Use it as written."),
        row![ui::btn_sm("Copy", None, ui::Kind::Ghost, Some(Msg::Copy(setup)))],
        ui::faint(note),
    ]
    .spacing(12)
    .width(Length::Fill)
    .into()
}

/// The sandboxes' table (or its empty or failed state), with the end confirmation above it when one is asked.
fn sandboxes_body(state: &State) -> Element<'_, Msg> {
    let p = crate::theme::palette();
    match &state.sandboxes {
        Load::Pending => ui::loading(),
        Load::Failed => ui::empty("alert", "Could not load the sandboxes", ""),
        Load::Ready(list) if list.is_empty() => ui::empty(
            "harness",
            "No sandboxes",
            "The gateway runs none right now. A client that makes one shows it here.",
        ),
        Load::Ready(list) => {
            let cols = [
                Col {
                    head: "Sandbox",
                    share: Some(3),
                    right: false,
                },
                Col {
                    head: "Template",
                    share: Some(2),
                    right: false,
                },
                Col {
                    head: "State",
                    share: Some(2),
                    right: false,
                },
                Col {
                    head: "Started",
                    share: Some(2),
                    right: false,
                },
                Col {
                    head: "Ends",
                    share: Some(2),
                    right: false,
                },
                Col {
                    head: "CPU",
                    share: Some(1),
                    right: false,
                },
                Col {
                    head: "Memory",
                    share: Some(1),
                    right: false,
                },
                Col {
                    head: "CID",
                    share: Some(1),
                    right: false,
                },
                Col {
                    head: "",
                    share: None,
                    right: true,
                },
            ];
            let rows: Vec<Vec<Element<'_, Msg>>> = list
                .iter()
                .map(|s| {
                    let tone = if s.state == "running" {
                        ui::Tone::Ok
                    } else {
                        ui::Tone::Muted
                    };
                    vec![
                        ui::mono(s.sandbox_id.clone(), 12.0, p.ink).into(),
                        ui::sans(s.template_id.clone(), 13.0, p.ink_2).into(),
                        ui::badge(&s.state, tone),
                        ui::faint(short_time(&s.started_at)).into(),
                        ui::faint(short_time(&s.end_at)).into(),
                        ui::sans(format!("{} vCPU", s.cpu_count), 13.0, p.ink_2).into(),
                        ui::sans(memory_text(s.memory_mb), 13.0, p.ink_2).into(),
                        ui::sans(
                            s.cid.map_or("—".to_string(), |cid| cid.to_string()),
                            13.0,
                            p.ink_2,
                        )
                        .into(),
                        sandbox_actions(state, s),
                    ]
                })
                .collect();
            let mut parts: Vec<Element<'_, Msg>> = Vec::new();
            if let Some(s) = state
                .confirm_end
                .as_deref()
                .and_then(|id| list.iter().find(|s| s.sandbox_id == id))
            {
                parts.push(
                    container(confirm(
                        "End this sandbox",
                        column![
                            ui::faint("Ends the sandbox. Whatever it holds is lost, and it cannot be resumed."),
                            ui::mono(s.sandbox_id.clone(), 12.0, p.ink_2),
                        ]
                        .spacing(8)
                        .into(),
                        Msg::CancelEnd,
                        "End",
                        gate(state.busy, Msg::End(s.sandbox_id.clone())),
                    ))
                    .padding(16)
                    .width(Length::Fill)
                    .into(),
                );
            }
            parts.push(table(16.0, &cols, rows));
            column(parts).width(Length::Fill).into()
        }
    }
}

/// A sandbox's actions: a running one pauses, a paused one resumes, and either can be ended.
fn sandbox_actions<'a>(state: &State, sandbox: &Sandbox) -> Element<'a, Msg> {
    let id = sandbox.sandbox_id.clone();
    let mut buttons: Vec<Element<'a, Msg>> = Vec::new();
    match sandbox.state.as_str() {
        "running" => buttons.push(ui::btn_sm(
            "Pause",
            None,
            ui::Kind::Ghost,
            gate(state.busy, Msg::Pause(id.clone())),
        )),
        "paused" => buttons.push(ui::btn_sm(
            "Resume",
            Some("play"),
            ui::Kind::Ghost,
            gate(state.busy, Msg::Resume(id.clone())),
        )),
        _ => {}
    }
    buttons.push(ui::btn_sm(
        "End",
        Some("stop"),
        ui::Kind::Danger,
        gate(state.busy, Msg::AskEnd(id)),
    ));
    row(buttons).spacing(8).align_y(Alignment::Center).into()
}

/// The templates' table (or its empty or failed state), with the delete confirmation above it when one is asked.
fn templates_body(state: &State) -> Element<'_, Msg> {
    let p = crate::theme::palette();
    match &state.templates {
        Load::Pending => ui::loading(),
        Load::Failed => ui::empty("alert", "Could not load the templates", ""),
        Load::Ready(list) if list.is_empty() => ui::empty(
            "harness",
            "No templates",
            "Templates are built with the E2B SDK's Template.build.",
        ),
        Load::Ready(list) => {
            let cols = [
                Col {
                    head: "Template",
                    share: Some(3),
                    right: false,
                },
                Col {
                    head: "Build",
                    share: Some(2),
                    right: false,
                },
                Col {
                    head: "Tags",
                    share: Some(2),
                    right: false,
                },
                Col {
                    head: "Size",
                    share: Some(2),
                    right: false,
                },
                Col {
                    head: "Updated",
                    share: Some(2),
                    right: false,
                },
                Col {
                    head: "Failure",
                    share: Some(3),
                    right: false,
                },
                Col {
                    head: "",
                    share: None,
                    right: true,
                },
            ];
            let rows: Vec<Vec<Element<'_, Msg>>> = list
                .iter()
                .map(|t| {
                    let name = if t.name.is_empty() {
                        "(no name)".to_string()
                    } else {
                        t.name.clone()
                    };
                    let latest = t.builds.first();
                    let build: Element<'_, Msg> = match latest {
                        Some(b) => ui::badge(&b.status, build_tone(&b.status)),
                        None => ui::faint("—").into(),
                    };
                    let size = latest.map_or("—".to_string(), |b| {
                        format!("{} vCPU, {}", b.cpu_count, memory_text(b.memory_mb))
                    });
                    let failure =
                        latest
                            .filter(|b| b.status == "error")
                            .map_or("—".to_string(), |b| {
                                b.reason
                                    .as_ref()
                                    .map_or("The build failed.".to_string(), |r| r.message.clone())
                            });
                    let tags = ready_tags(&t.builds);
                    let tag_text = if tags.is_empty() {
                        "—".to_string()
                    } else {
                        tags.join(", ")
                    };
                    vec![
                        row![
                            ui::sans(name, 13.0, p.ink),
                            ui::mono(short_id(&t.template_id), 12.0, p.ink_3),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .into(),
                        build,
                        ui::sans(tag_text, 13.0, p.ink_2).into(),
                        ui::sans(size, 13.0, p.ink_2).into(),
                        ui::faint(short_time(&t.updated_at)).into(),
                        ui::faint(failure).into(),
                        ui::btn_sm(
                            "Delete",
                            Some("trash"),
                            ui::Kind::Danger,
                            gate(state.busy, Msg::AskDeleteTemplate(t.template_id.clone())),
                        ),
                    ]
                })
                .collect();
            let mut parts: Vec<Element<'_, Msg>> = Vec::new();
            if let Some(t) = state
                .confirm_delete_template
                .as_deref()
                .and_then(|id| list.iter().find(|t| t.template_id == id))
            {
                parts.push(
                    container(confirm(
                        &format!("Delete {}", t.name),
                        column![
                            ui::faint("Deletes the template and all its builds. This cannot be undone. A build that is running, or one that starts from it, blocks the delete until it finishes."),
                            ui::mono(t.template_id.clone(), 12.0, p.ink_2),
                        ]
                        .spacing(8)
                        .into(),
                        Msg::CancelDeleteTemplate,
                        "Delete",
                        gate(state.busy, Msg::DeleteTemplate(t.template_id.clone())),
                    ))
                    .padding(16)
                    .width(Length::Fill)
                    .into(),
                );
            }
            parts.push(table(16.0, &cols, rows));
            column(parts).width(Length::Fill).into()
        }
    }
}

/// A build's badge tone, as the web's: waiting is quiet, building is info, ready is ok, error is danger.
fn build_tone(status: &str) -> ui::Tone {
    match status {
        "ready" => ui::Tone::Ok,
        "building" => ui::Tone::Info,
        "error" => ui::Tone::Danger,
        _ => ui::Tone::Muted,
    }
}

/// A confirmation as the web's dialog: a card with its text and Cancel, and the action in danger.
fn confirm<'a>(
    title: &str,
    body: Element<'a, Msg>,
    cancel: Msg,
    action_label: &str,
    action: Option<Msg>,
) -> Element<'a, Msg> {
    ui::card(
        title,
        column![
            body,
            row![
                Space::new().width(Length::Fill),
                ui::btn("Cancel", None, ui::Kind::Ghost, Some(cancel)),
                ui::btn(action_label, None, ui::Kind::Danger, action),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(12)
        .into(),
    )
}

/// A memory size as the web's table reads it: MB below a gibibyte, GB from there (one decimal).
fn memory_text(mb: u64) -> String {
    if mb >= 1024 {
        format!("{:.1} GB", mb as f64 / 1024.0)
    } else {
        format!("{mb} MB")
    }
}

/// The card's rule across, in the palette's rule colour.
fn rule<'a>(color: Color) -> Element<'a, Msg> {
    container(Space::new().height(1.0))
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(color.into()),
            ..Default::default()
        })
        .into()
}

/// The web's `.card.flush`: the title in the card head over a rule, the body flush to the card's edges.
fn flush_card<'a>(title: &str, body: Element<'a, Msg>) -> Element<'a, Msg> {
    let top = container(row![
        ui::card_title(title),
        Space::new().width(Length::Fill)
    ])
    .padding([8, 16])
    .height(Length::Shrink)
    .width(Length::Fill)
    .center_y(Length::Fixed(44.0));
    container(column![top, rule(crate::theme::palette().rule), body])
        .width(Length::Fill)
        .style(ui::card_style)
        .clip(true)
        .into()
}

/// A column of a table: its head, its share of the row (None: as wide as its content), and whether it reads right.
#[derive(Clone, Copy)]
struct Col {
    head: &'static str,
    share: Option<u16>,
    right: bool,
}

/// One cell: the first and last columns sit at the card's edges (`edge`), the others carry 12 either side.
fn cell<'a>(
    cols: &[Col],
    i: usize,
    content: Element<'a, Msg>,
    edge: f32,
    pad_y: f32,
) -> Element<'a, Msg> {
    let col = cols[i];
    let left = if i == 0 { edge } else { 12.0 };
    let right = if i + 1 == cols.len() { edge } else { 12.0 };
    let inner = container(content)
        .padding(Padding {
            top: pad_y,
            right,
            bottom: pad_y,
            left,
        })
        .align_x(if col.right {
            Horizontal::Right
        } else {
            Horizontal::Left
        })
        .align_y(Vertical::Center);
    match col.share {
        Some(share) => inner.width(Length::FillPortion(share)).into(),
        None => inner.width(Length::Shrink).into(),
    }
}

/// The web's `.tbl`: the head in the label voice over a strong rule, a row per item with a rule between.
fn table<'a>(edge: f32, cols: &[Col], rows: Vec<Vec<Element<'a, Msg>>>) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let heads: Vec<Element<'a, Msg>> = cols
        .iter()
        .enumerate()
        .map(|(i, c)| cell(cols, i, ui::label(c.head).into(), edge, 8.0))
        .collect();
    let mut lines: Vec<Element<'a, Msg>> =
        vec![row(heads).width(Length::Fill).into(), rule(p.rule_strong)];
    for (n, cells) in rows.into_iter().enumerate() {
        if n > 0 {
            lines.push(rule(p.rule));
        }
        let line: Vec<Element<'a, Msg>> = cells
            .into_iter()
            .enumerate()
            .map(|(i, content)| cell(cols, i, content, edge, 9.0))
            .collect();
        lines.push(
            row(line)
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .into(),
        );
    }
    column(lines).width(Length::Fill).into()
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
    fn e2b_sandboxes_parse_with_the_owner_fields_and_without_a_cid() {
        {
            // e2b_sandboxes_parse_with_the_owner_fields
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
        }
        {
            // e2b_sandbox_without_a_cid_still_parses
            let list: SandboxList =
                serde_json::from_str(r#"{"sandboxes":[{"sandboxID":"sb_2","cid":null}]}"#)
                    .expect("a sandbox with a null cid parses");
            assert_eq!(list.sandboxes[0].cid, None);
        }
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
    fn e2b_templates_parse_with_their_builds_and_without_optional_fields() {
        {
            // e2b_templates_parse_with_their_builds
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
        {
            // e2b_templates_parse_without_reason_or_finish_time
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
