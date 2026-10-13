//! The Nodes screen (parity with the web console's `nodes` view): the machines in the plaza
//! (drain, resume, revoke), the join codes that bring new ones in (each token shown once), and
//! this hub's identity with the owner-token rotation. Reloads on the screen's poll.

use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::text::Wrapping;
use iced::widget::{checkbox, column, container, row, text_input, Space};
use iced::{Alignment, Border, Element, Length, Task, Theme};
use serde::Deserialize;
use serde_json::{json, Value};

use super::Id;
use crate::console::services::{self, Mode};
use crate::console::{client, ui, Ctx};

/// The hub's own node: it is not drained or revoked from here.
const HUB_NODE: &str = "n0";

/// A machine's reported hardware.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Machine {
    pub cpus: Option<u64>,
    pub memory_mib: Option<u64>,
    pub host: Option<String>,
}

/// A machine in the plaza (`GET /v1/nodes`).
#[derive(Debug, Clone, Deserialize)]
pub struct NodeView {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub cid_base: u64,
    #[serde(default)]
    pub cid_count: u64,
    #[serde(default)]
    pub last_seen_ms: f64,
    #[serde(default)]
    pub machine: Option<Machine>,
    #[serde(default)]
    pub draining: bool,
    #[serde(default)]
    pub revoked: bool,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub online: bool,
    #[serde(default)]
    pub agents: u64,
}

/// A join code (`GET /v1/nodes/codes`); its token is never listed.
#[derive(Debug, Clone, Deserialize)]
pub struct JoinCode {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub created_ms: f64,
    #[serde(default)]
    pub expires_ms: f64,
    /// active, used, expired or revoked.
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub used_by: Option<String>,
}

/// A join code as issued: the token, shown once.
#[derive(Debug, Clone, Deserialize)]
pub struct IssuedCode {
    pub token: String,
    pub expires_ms: f64,
}

/// What the hub says about itself (`GET /v1/hub/identity`).
#[derive(Debug, Clone, Deserialize)]
pub struct HubIdentity {
    pub name: String,
    pub version: String,
    pub url: String,
    pub fingerprint: Option<String>,
}

/// This machine's link to a network, from the hub (`GET /v1/node-link`, the web's Join card).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct NodeLink {
    #[serde(default)]
    pub joined: bool,
    #[serde(default)]
    pub hub: Option<String>,
    #[serde(default)]
    pub node_id: Option<String>,
    #[serde(default)]
    pub cids: Option<String>,
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub pid: Option<u32>,
}

/// A new owner token, shown once.
#[derive(Debug, Clone, Deserialize)]
pub struct RotatedToken {
    pub token: String,
}

/// A plaza machine's hardware, GPUs and runs (`GET /v1/machines`, the web's machine list).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PlazaMachine {
    pub id: String,
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub arch: String,
    #[serde(default)]
    pub backend: String,
    #[serde(default)]
    pub irqchip: String,
    #[serde(default)]
    pub vsock_mode: String,
    #[serde(default)]
    pub cpus: u64,
    #[serde(default)]
    pub memory_mib: u64,
    #[serde(default)]
    pub gpus: Vec<Gpu>,
    #[serde(default)]
    pub runs: Vec<Run>,
}

/// A GPU a machine reported.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Gpu {
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub memory_mib: Option<u64>,
}

/// A run on a machine: its kind (role), state, VMs, and when it started (ms since the epoch).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Run {
    pub id: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub created_at: f64,
    #[serde(default)]
    pub vms: Vec<Value>,
}

/// One of the list loads: pending until the first answer, failed when the last request did.
#[derive(Debug, Default)]
pub enum Load<T> {
    #[default]
    Pending,
    Ready(T),
    Failed,
}

fn settle<T>(result: Result<T, String>) -> Load<T> {
    match result {
        Ok(value) => Load::Ready(value),
        Err(_) => Load::Failed,
    }
}

/// The dialog that is open, drawn as a card above the lists.
#[derive(Debug, Clone)]
pub enum Dialog {
    /// Add a node: the label and how long the code is valid, then the token once issued.
    Add { label: String, minutes: u64, issued: Option<IssuedCode> },
    ConfirmRevoke { id: String, name: String },
    ConfirmRotate,
    /// The new owner token, shown once.
    Rotated(String),
}

#[derive(Debug, Default)]
pub struct State {
    pub nodes: Load<Vec<NodeView>>,
    pub codes: Load<Vec<JoinCode>>,
    pub hub: Load<HubIdentity>,
    /// The last action's error, and its confirmation.
    pub error: Option<String>,
    pub notice: Option<String>,
    /// The open dialog, if any.
    pub dialog: Option<Dialog>,
    /// This machine's link to a network, from the hub (loaded on open and on the poll).
    pub node: Load<NodeLink>,
    /// The join code being typed; the node's action running; its progress lines and error.
    pub code: String,
    pub node_busy: bool,
    pub progress: Vec<String>,
    pub node_error: Option<String>,
    /// The leave confirmation is open, and whether the network is forgotten too.
    pub leaving: bool,
    pub forget: bool,
    /// The plaza's machines, and the node whose detail is open.
    pub machines: Load<Vec<PlazaMachine>>,
    pub selected: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    NodesLoaded(Result<Vec<NodeView>, String>),
    CodesLoaded(Result<Vec<JoinCode>, String>),
    HubLoaded(Result<HubIdentity, String>),
    /// Drain (`on`) or resume a machine.
    Drain { id: String, name: String, on: bool },
    AskRevoke { id: String, name: String },
    Revoke { id: String, name: String },
    RevokeCode(String),
    /// An action finished: its result and the line to confirm it with.
    Acted(Result<Value, String>, String),
    AskRotate,
    Rotate,
    Rotated(Result<RotatedToken, String>),
    OpenAdd,
    LabelEdit(String),
    PickMinutes(u64),
    CreateCode,
    Issued(Result<IssuedCode, String>),
    /// Copy a value to the clipboard.
    Copy(String),
    Close,
    /// This machine's network link, from the hub (`GET /v1/node-link`).
    NodeLinkLoaded(Result<NodeLink, String>),
    CodeEdit(String),
    /// Join the network the code names, then start the node.
    Join,
    /// The join finished: the link the hub answers with, or its error.
    Joined(Result<NodeLink, String>),
    NodeStart,
    NodeStop,
    /// The node service started or stopped: its progress lines, or its error.
    NodeRan(Result<Vec<String>, String>),
    AskLeave,
    CancelLeave,
    ToggleForget(bool),
    Leave,
    Left(Result<NodeLink, String>),
    MachinesLoaded(Result<Vec<PlazaMachine>, String>),
    /// Open a node's detail, or close it.
    Select(String),
    Deselect,
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// Loads the screen's data (called when it opens and on every poll tick).
pub fn load(_state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let (nodes, codes, hub, machines, link) = (ctx.client.clone(), ctx.client.clone(), ctx.client.clone(), ctx.client.clone(), ctx.client.clone());
    Task::batch([
        client::task(move || nodes.get::<Vec<NodeView>>("/v1/nodes"), Msg::NodesLoaded),
        client::task(move || codes.get::<Vec<JoinCode>>("/v1/nodes/codes"), Msg::CodesLoaded),
        client::task(move || hub.get::<HubIdentity>("/v1/hub/identity"), Msg::HubLoaded),
        client::task(move || machines.get::<Vec<PlazaMachine>>("/v1/machines"), Msg::MachinesLoaded),
        client::task(move || link.get::<NodeLink>("/v1/node-link"), Msg::NodeLinkLoaded),
    ])
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::NodesLoaded(result) => {
            state.nodes = settle(result);
            Task::none()
        }
        Msg::CodesLoaded(result) => {
            state.codes = settle(result);
            Task::none()
        }
        Msg::HubLoaded(result) => {
            state.hub = settle(result);
            Task::none()
        }
        Msg::Drain { id, name, on } => {
            let notice = if on { format!("{name} is draining") } else { format!("{name} is back in service") };
            let path = format!("/v1/nodes/{id}/drain");
            let body = json!({ "on": on });
            let client = ctx.client.clone();
            client::task(move || client.call("POST", &path, Some(&body)), move |r| Msg::Acted(r, notice.clone()))
        }
        Msg::AskRevoke { id, name } => {
            state.dialog = Some(Dialog::ConfirmRevoke { id, name });
            state.error = None;
            Task::none()
        }
        Msg::Revoke { id, name } => {
            state.dialog = None;
            let path = format!("/v1/nodes/{id}/revoke");
            let notice = format!("{name} revoked");
            let client = ctx.client.clone();
            client::task(move || client.call("POST", &path, None), move |r| Msg::Acted(r, notice.clone()))
        }
        Msg::RevokeCode(id) => {
            let path = format!("/v1/nodes/codes/{id}");
            let client = ctx.client.clone();
            client::task(move || client.call("DELETE", &path, None), |r| Msg::Acted(r, "Join code revoked".to_string()))
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
        Msg::AskRotate => {
            state.dialog = Some(Dialog::ConfirmRotate);
            state.error = None;
            Task::none()
        }
        Msg::Rotate => {
            let client = ctx.client.clone();
            client::task(
                move || {
                    client
                        .call("POST", "/v1/hub/owner-token/rotate", None)
                        .and_then(|v| serde_json::from_value::<RotatedToken>(v).map_err(|e| e.to_string()))
                },
                Msg::Rotated,
            )
        }
        Msg::Rotated(Ok(made)) => {
            state.error = None;
            state.dialog = Some(Dialog::Rotated(made.token));
            load(state, ctx)
        }
        Msg::Rotated(Err(error)) => {
            state.error = Some(error);
            Task::none()
        }
        Msg::OpenAdd => {
            state.dialog = Some(Dialog::Add { label: String::new(), minutes: 30, issued: None });
            state.error = None;
            Task::none()
        }
        Msg::LabelEdit(value) => {
            if let Some(Dialog::Add { label, .. }) = &mut state.dialog {
                *label = value.chars().take(60).collect();
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
            let (name, minutes) = match &state.dialog {
                Some(Dialog::Add { label, minutes, .. }) => (label.trim().to_string(), *minutes),
                _ => return Task::none(),
            };
            if name.is_empty() {
                state.error = Some("Give the machine a label first.".to_string());
                return Task::none();
            }
            let body = json!({ "label": name, "ttl_minutes": minutes });
            let client = ctx.client.clone();
            client::task(move || client.post::<IssuedCode>("/v1/nodes/codes", &body), Msg::Issued)
        }
        Msg::Issued(Ok(made)) => {
            state.error = None;
            if let Some(Dialog::Add { issued, .. }) = &mut state.dialog {
                *issued = Some(made);
            }
            load(state, ctx)
        }
        Msg::Issued(Err(error)) => {
            state.error = Some(error);
            Task::none()
        }
        Msg::Copy(value) => {
            state.notice = Some("Copied".to_string());
            iced::clipboard::write(value)
        }
        Msg::Close => {
            state.dialog = None;
            state.error = None;
            Task::none()
        }
        Msg::NodeLinkLoaded(result) => {
            state.node = settle(result);
            Task::none()
        }
        Msg::CodeEdit(value) => {
            state.code = value;
            Task::none()
        }
        Msg::Join => {
            let token = state.code.trim().to_string();
            if state.node_busy {
                return Task::none();
            }
            if token.is_empty() {
                state.node_error = Some("Paste the join code first.".to_string());
                return Task::none();
            }
            state.node_busy = true;
            state.node_error = None;
            state.progress = vec!["Joining the network…".to_string()];
            let client = ctx.client.clone();
            let body = json!({ "code": token });
            client::task(move || client.post::<NodeLink>("/v1/node-link/join", &body), Msg::Joined)
        }
        Msg::Joined(result) => {
            state.node_busy = false;
            match result {
                Ok(link) => {
                    state.code.clear();
                    state.progress = vec![format!("joined as node {}", link.node_id.as_deref().unwrap_or("?"))];
                    state.node = Load::Ready(link);
                }
                Err(error) => {
                    state.progress.clear();
                    state.node_error = Some(error);
                }
            }
            load(state, ctx)
        }
        Msg::NodeStart => node_run(state, true),
        Msg::NodeStop => node_run(state, false),
        Msg::NodeRan(result) => {
            state.node_busy = false;
            match result {
                Ok(lines) => state.progress = lines,
                Err(error) => state.node_error = Some(error),
            }
            load(state, ctx)
        }
        Msg::AskLeave => {
            state.leaving = true;
            state.forget = false;
            state.node_error = None;
            Task::none()
        }
        Msg::CancelLeave => {
            state.leaving = false;
            state.forget = false;
            Task::none()
        }
        Msg::ToggleForget(on) => {
            state.forget = on;
            Task::none()
        }
        Msg::Leave => {
            if state.node_busy {
                return Task::none();
            }
            state.node_busy = true;
            state.node_error = None;
            let body = json!({ "forget": state.forget });
            let client = ctx.client.clone();
            client::task(move || client.post::<NodeLink>("/v1/node-link/leave", &body), Msg::Left)
        }
        Msg::MachinesLoaded(result) => {
            state.machines = settle(result);
            Task::none()
        }
        Msg::Select(id) => {
            state.selected = Some(id);
            Task::none()
        }
        Msg::Deselect => {
            state.selected = None;
            Task::none()
        }
        Msg::Left(result) => {
            state.node_busy = false;
            let forgot = state.forget;
            state.leaving = false;
            state.forget = false;
            match result {
                Ok(link) => {
                    state.node = Load::Ready(link);
                    state.progress = vec![if forgot {
                        "Left the network and forgot it.".to_string()
                    } else {
                        "Left the network. Its node is stopped.".to_string()
                    }];
                }
                Err(error) => state.node_error = Some(error),
            }
            load(state, ctx)
        }
    }
}

/// Starts or stops the node service in a task.
fn node_run(state: &mut State, start: bool) -> Task<Msg> {
    if state.node_busy {
        return Task::none();
    }
    state.node_busy = true;
    state.node_error = None;
    state.progress = vec![if start { "Starting the node…" } else { "Stopping the node…" }.to_string()];
    Task::perform(
        async move {
            if start {
                services::start(Mode::Node)
            } else {
                services::stop(Mode::Node)
            }
        },
        Msg::NodeRan,
    )
}

pub fn view<'a>(state: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    let local = ctx.client.base() == crate::console::boot::LOCAL_HUB;
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(error) = &state.error {
        parts.push(ui::error_line(error.as_str()));
    }
    if let Some(notice) = &state.notice {
        parts.push(ui::faint(notice.as_str()).into());
    }
    if let Some(dialog) = &state.dialog {
        parts.push(dialog_view(dialog));
    }
    parts.push(ui::card("Machines in this plaza", nodes_view(&state.nodes)));
    if let Some(detail) = detail_view(state) {
        parts.push(detail);
    }
    parts.push(network_view(state, local));
    parts.push(ui::card("Join codes", codes_view(&state.codes)));
    parts.push(ui::card("This hub", hub_view(&state.hub)));

    let head = ui::view_head("Nodes", vec![ui::btn("Add a node", None, ui::Kind::Primary, Some(Msg::OpenAdd))]);
    ui::sheet(head, column(parts).spacing(22).width(Length::Fill).into())
}

/// The machines' table, or its empty or failed state.
fn nodes_view<'a>(nodes: &'a Load<Vec<NodeView>>) -> Element<'a, Msg> {
    match nodes {
        Load::Pending => ui::loading(),
        Load::Failed => ui::empty("alert", "Could not load the machines.", ""),
        Load::Ready(list) if list.is_empty() => ui::empty("link", "No machines yet", "Add a node here to bring a machine into this network, or join this hub to another network below."),
        Load::Ready(list) => table(
            &[(3, "Name"), (2, "State"), (2, "Address"), (1, "Agents"), (1, "CIDs"), (3, "Machine"), (2, "Last seen"), (3, "")],
            list.iter().map(node_row).collect(),
        ),
    }
}

fn node_row<'a>(n: &'a NodeView) -> Element<'a, Msg> {
    let (state, tone) = node_state(n);
    cells(vec![
        (3, column![ui::link(n.name.as_str(), Msg::Select(n.id.clone()), false), ui::faint(n.id.as_str())].spacing(2).into()),
        (2, ui::badge(state, tone)),
        (2, ui::mono(if n.address.is_empty() { "—" } else { n.address.as_str() }, 12.0, crate::theme::palette().ink).into()),
        (1, ui::mono(n.agents.to_string(), 12.0, crate::theme::palette().ink).into()),
        (1, ui::mono(cids(n), 12.0, crate::theme::palette().ink).into()),
        (3, ui::faint(machine_text(&n.machine)).into()),
        (2, ui::faint(ago(n.last_seen_ms)).into()),
        (3, node_actions(n)),
    ])
}

fn node_actions<'a>(n: &'a NodeView) -> Element<'a, Msg> {
    if n.id == HUB_NODE {
        return ui::faint("this hub").into();
    }
    if n.revoked {
        return Space::new().width(Length::Fill).into();
    }
    row![
        ui::btn(
            if n.draining { "Resume" } else { "Drain" },
            None,
            ui::Kind::Secondary,
            Some(Msg::Drain { id: n.id.clone(), name: n.name.clone(), on: !n.draining }),
        ),
        ui::btn("Revoke", None, ui::Kind::Danger, Some(Msg::AskRevoke { id: n.id.clone(), name: n.name.clone() })),
    ]
    .spacing(8)
    .into()
}

/// The join codes, newest first, or the empty or failed state.
fn codes_view<'a>(codes: &'a Load<Vec<JoinCode>>) -> Element<'a, Msg> {
    match codes {
        Load::Pending => ui::loading(),
        Load::Failed => ui::empty("alert", "Could not load the join codes.", ""),
        Load::Ready(list) if list.is_empty() => {
            ui::empty("link", "No join codes yet", "Codes made with \"Add a node\" are listed here.")
        }
        Load::Ready(list) => {
            let mut sorted: Vec<&JoinCode> = list.iter().collect();
            sorted.sort_by(|a, b| b.created_ms.total_cmp(&a.created_ms));
            table(
                &[(3, "Label"), (2, "ID"), (2, "Created"), (2, "Expires"), (3, "State"), (1, "")],
                sorted.into_iter().map(code_row).collect(),
            )
        }
    }
}

fn code_row<'a>(c: &'a JoinCode) -> Element<'a, Msg> {
    let (label, tone) = code_state(&c.state);
    let state_cell: Element<'a, Msg> = match &c.used_by {
        Some(by) => row![ui::badge(&label, tone), ui::faint(format!(" by {by}"))]
            .spacing(6)
            .align_y(Alignment::Center)
            .into(),
        None => ui::badge(&label, tone),
    };
    let active = c.state == "active";
    let expires = if active { format!("in {}", countdown(c.expires_ms - now_ms())) } else { ago(c.expires_ms) };
    let action: Element<'a, Msg> = if active {
        ui::btn("Revoke", None, ui::Kind::Danger, Some(Msg::RevokeCode(c.id.clone())))
    } else {
        Space::new().width(Length::Fill).into()
    };
    cells(vec![
        (3, ui::mono(c.label.as_str(), 13.0, crate::theme::palette().ink).into()),
        (2, ui::mono(c.id.as_str(), 11.0, crate::theme::palette().ink_3).into()),
        (2, ui::faint(ago(c.created_ms)).into()),
        (2, ui::mono(expires, 12.0, crate::theme::palette().ink).into()),
        (3, state_cell),
        (1, action),
    ])
}

/// The hub's identity and the owner-token rotation.
fn hub_view<'a>(hub: &'a Load<HubIdentity>) -> Element<'a, Msg> {
    match hub {
        Load::Pending => ui::loading(),
        Load::Failed => ui::empty("alert", "Could not load this hub.", ""),
        Load::Ready(id) => {
            let fingerprint: Element<'a, Msg> = match &id.fingerprint {
                Some(fp) => ui::mono(fp.as_str(), 11.0, crate::theme::palette().ink).into(),
                None => ui::faint("no TLS (loopback only)").into(),
            };
            column![
                fact("Name", ui::mono(id.name.as_str(), 13.0, crate::theme::palette().ink).into()),
                fact("URL", ui::mono(id.url.as_str(), 12.0, crate::theme::palette().ink).into()),
                fact("Version", ui::mono(id.version.as_str(), 12.0, crate::theme::palette().ink).into()),
                fact("TLS fingerprint", fingerprint),
                ui::btn("Rotate owner token", None, ui::Kind::Danger, Some(Msg::AskRotate)),
            ]
            .spacing(12)
            .into()
        }
    }
}

/// The open dialog, as a card.
fn dialog_view<'a>(dialog: &'a Dialog) -> Element<'a, Msg> {
    match dialog {
        Dialog::Add { label, minutes, issued: None } => ui::card("Add a node", add_form(label.as_str(), *minutes)),
        Dialog::Add { issued: Some(made), .. } => ui::card("Add a node", issued_view(made)),
        Dialog::ConfirmRevoke { id, name } => ui::card(
            &format!("Revoke {name}?"),
            column![
                ui::faint("Its signatures stop working at once and its agents go offline."),
                row![
                    ui::btn("Cancel", None, ui::Kind::Secondary, Some(Msg::Close)),
                    ui::btn("Revoke", None, ui::Kind::Danger, Some(Msg::Revoke { id: id.clone(), name: name.clone() })),
                ]
                .spacing(8),
            ]
            .spacing(12)
            .into(),
        ),
        Dialog::ConfirmRotate => ui::card(
            "Rotate the owner token?",
            column![
                ui::faint("The current owner token stops working at once."),
                row![
                    ui::btn("Cancel", None, ui::Kind::Secondary, Some(Msg::Close)),
                    ui::btn("Rotate", None, ui::Kind::Danger, Some(Msg::Rotate)),
                ]
                .spacing(8),
            ]
            .spacing(12)
            .into(),
        ),
        Dialog::Rotated(token) => ui::card(
            "New owner token",
            column![
                ui::prose("The new owner token, shown once:"),
                copy_box(token),
                ui::faint("Every signed-in browser must sign in again."),
                ui::btn("Close", None, ui::Kind::Secondary, Some(Msg::Close)),
            ]
            .spacing(12)
            .into(),
        ),
    }
}

/// The label and validity of a new join code.
fn add_form<'a>(label: &'a str, minutes: u64) -> Element<'a, Msg> {
    let choice = |m: u64| {
        let kind = if m == minutes { ui::Kind::Primary } else { ui::Kind::Secondary };
        ui::btn(&format!("{m} minutes"), None, kind, Some(Msg::PickMinutes(m)))
    };
    column![
        ui::label("Label"),
        text_input("studio-pc", label).on_input(Msg::LabelEdit).on_submit(Msg::CreateCode).padding(8.0),
        ui::faint("What the machine is called here."),
        ui::label("Valid for"),
        row![choice(15), choice(30), choice(60)].spacing(8),
        ui::faint("How long the code can be used to join."),
        ui::btn("Create join code", None, ui::Kind::Primary, Some(Msg::CreateCode)),
    ]
    .spacing(10)
    .into()
}

/// The token just issued, shown once, with the command to run on the new machine.
fn issued_view<'a>(made: &'a IssuedCode) -> Element<'a, Msg> {
    column![
        ui::faint("Shown once. Single use. Anyone with it can join until it expires.").color(crate::theme::palette().warn),
        copy_box(&made.token),
        row![
            ui::faint("Expires in "),
            ui::mono(countdown(made.expires_ms - now_ms()), 12.0, crate::theme::palette().ink),
        ]
        .spacing(4)
        .align_y(Alignment::Center),
        ui::faint("On the new machine, run:"),
        copy_box(&format!("apz-link join {}", made.token)),
        ui::faint("or paste it into the desktop app's Join a hub."),
        ui::btn("Close", None, ui::Kind::Secondary, Some(Msg::Close)),
    ]
    .spacing(10)
    .into()
}

/// A value to copy: a mono box with a Copy button.
fn copy_box<'a>(value: &str) -> Element<'a, Msg> {
    container(
        row![
            ui::mono(value.to_string(), 12.0, crate::theme::palette().ink).width(Length::Fill),
            ui::btn("Copy", None, ui::Kind::Ghost, Some(Msg::Copy(value.to_string()))),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .padding([8, 12])
    .width(Length::Fill)
    .style(boxed)
    .into()
}

/// A labelled line of the hub's identity.
fn fact<'a>(name: &str, value: Element<'a, Msg>) -> Element<'a, Msg> {
    row![container(ui::label(name)).width(Length::Fixed(150.0)), value]
        .spacing(12)
        .align_y(Alignment::Center)
        .into()
}

/// A table: a mono header row, then the rows, each under a hairline. Columns are proportional.
fn table<'a>(head: &[(u16, &str)], rows: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let heads: Vec<(u16, Element<'a, Msg>)> = head.iter().map(|(p, l)| (*p, ui::label(*l).into())).collect();
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

/// The "Join a network" card: the join form while this machine is no node, else the network it
/// joined, with its services and Leave. Progress and errors show under either.
fn network_view<'a>(state: &'a State, local: bool) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = vec![ui::prose(
        "Make this hub's machine a node of another network: it lends its machine to that network's agents. Its own plaza keeps running.",
    )
    .into()];
    parts.push(match &state.node {
        Load::Pending => ui::loading(),
        Load::Failed => ui::empty("alert", "Could not load the network link.", ""),
        Load::Ready(node) if node.joined => joined_view(node, state, local),
        Load::Ready(_) => join_form(state),
    });
    for line in &state.progress {
        parts.push(ui::mono(line.as_str(), 11.0, crate::theme::palette().ink_3).into());
    }
    if let Some(error) = &state.node_error {
        parts.push(ui::error_line(error.as_str()));
    }
    ui::card("This machine", column(parts).spacing(12).width(Length::Fill).into())
}

/// The join form: the code (monospace) and the Join button.
fn join_form<'a>(state: &'a State) -> Element<'a, Msg> {
    let busy = state.node_busy;
    column![
        ui::label("Join code"),
        text_input("Paste the join code", &state.code)
            .font(crate::theme::MONO)
            .style(crate::theme::input)
            .padding(8.0)
            .on_input(Msg::CodeEdit)
            .on_submit(Msg::Join),
        ui::faint("Get a code from the other network's Nodes screen (Add a node)."),
        ui::btn(if busy { "Joining…" } else { "Join" }, None, ui::Kind::Primary, (!busy).then_some(Msg::Join)),
    ]
    .spacing(10)
    .into()
}

/// The joined network: its hub, node and CIDs, the node service's state with Start or Stop, and
/// Leave (confirmed, with the choice to forget the network).
fn joined_view<'a>(node: &'a NodeLink, state: &'a State, local: bool) -> Element<'a, Msg> {
    let busy = state.node_busy;
    let (badge_text, tone) = if node.running { ("running", ui::Tone::Ok) } else { ("stopped", ui::Tone::Muted) };
    let toggle: Element<'a, Msg> = if !local {
        ui::faint("Start and stop run on the node's own machine.").into()
    } else if node.running {
        ui::btn("Stop", None, ui::Kind::Secondary, (!busy).then_some(Msg::NodeStop))
    } else {
        ui::btn("Start", None, ui::Kind::Secondary, (!busy).then_some(Msg::NodeStart))
    };
    let leave: Element<'a, Msg> = if state.leaving {
        column![
            ui::faint("This node stops lending its machine to that network, and its agents there go offline."),
            checkbox(state.forget).label("Forget this network: remove the saved hub, so joining again needs a new code.").on_toggle(Msg::ToggleForget),
            row![
                ui::btn("Cancel", None, ui::Kind::Secondary, Some(Msg::CancelLeave)),
                ui::btn("Leave", None, ui::Kind::Danger, (!busy).then_some(Msg::Leave)),
            ]
            .spacing(8),
        ]
        .spacing(10)
        .into()
    } else {
        ui::btn("Leave", None, ui::Kind::Danger, (!busy).then_some(Msg::AskLeave))
    };
    let mut status = row![ui::badge(badge_text, tone)].spacing(10).align_y(Alignment::Center);
    if let Some(pid) = node.pid {
        status = status.push(ui::faint(format!("pid {pid}")).wrapping(Wrapping::None));
    }
    let service: Element<'a, Msg> = status.push(toggle).into();
    column![
        fact("Hub", ui::mono(or_dash(node.hub.as_deref().unwrap_or("")), 12.0, crate::theme::palette().ink).into()),
        fact("This node", ui::mono(or_dash(node.node_id.as_deref().unwrap_or("")), 12.0, crate::theme::palette().ink).into()),
        fact("CIDs", ui::mono(or_dash(node.cids.as_deref().unwrap_or("")), 12.0, crate::theme::palette().ink).into()),
        fact("Status", service),
        leave,
    ]
    .spacing(12)
    .into()
}

/// The selected node's detail under the machines table: its machine's facts and its runs.
fn detail_view<'a>(state: &'a State) -> Option<Element<'a, Msg>> {
    let id = state.selected.as_ref()?;
    let Load::Ready(nodes) = &state.nodes else {
        return None;
    };
    let node = nodes.iter().find(|n| &n.id == id)?;
    let body: Element<'a, Msg> = match &state.machines {
        Load::Pending => ui::loading(),
        Load::Failed => ui::empty("alert", "Could not load this machine.", ""),
        Load::Ready(list) => match list.iter().find(|m| &m.id == id) {
            Some(machine) => column![machine_facts(node, machine), runs_table(&machine.runs)].spacing(16).width(Length::Fill).into(),
            None => ui::faint("This machine reports no facts.").into(),
        },
    };
    let close = ui::btn("Close", None, ui::Kind::Secondary, Some(Msg::Deselect));
    Some(ui::card(&node.name, column![body, close].spacing(16).width(Length::Fill).into()))
}

/// The machine's facts, laid out like the web's detail: CPU, memory, GPUs, arch and backend, then the rest.
fn machine_facts<'a>(node: &'a NodeView, m: &'a PlazaMachine) -> Element<'a, Msg> {
    let cpus = if m.cpus > 0 { format!("{} cores", m.cpus) } else { "?".to_string() };
    let memory = if m.memory_mib > 0 { format!("{:.1} GiB", m.memory_mib as f64 / 1024.0) } else { "?".to_string() };
    let gpus = if m.gpus.is_empty() { "None".to_string() } else { m.gpus.iter().map(gpu_text).collect::<Vec<_>>().join(", ") };
    let host = node.machine.as_ref().and_then(|x| x.host.as_deref()).unwrap_or("?");
    column![
        fact("CPU", ui::mono(cpus, 12.0, crate::theme::palette().ink).into()),
        fact("Memory", ui::mono(memory, 12.0, crate::theme::palette().ink).into()),
        fact("GPUs", ui::mono(gpus, 12.0, crate::theme::palette().ink).into()),
        fact("Arch", ui::mono(or_dash(&m.arch), 12.0, crate::theme::palette().ink).into()),
        fact("Backend", ui::mono(or_dash(&m.backend), 12.0, crate::theme::palette().ink).into()),
        fact("Host", ui::mono(host, 12.0, crate::theme::palette().ink).into()),
        fact("Class", ui::mono(or_dash(&m.class), 12.0, crate::theme::palette().ink).into()),
        fact("Irqchip", ui::mono(or_dash(&m.irqchip), 12.0, crate::theme::palette().ink).into()),
        fact("vsock", ui::mono(or_dash(&m.vsock_mode), 12.0, crate::theme::palette().ink).into()),
    ]
    .spacing(12)
    .into()
}

/// A GPU as the web's list shows it: its vendor and name, with its memory when it reports one.
fn gpu_text(g: &Gpu) -> String {
    let name = format!("{} {}", g.vendor, g.name).trim().to_string();
    match g.memory_mib {
        Some(mib) if mib > 0 => format!("{name} ({:.0} GiB)", mib as f64 / 1024.0),
        _ => name,
    }
}

/// The machine's runs: id, kind, state, its VMs, and when it started; a faint line when it has none.
fn runs_table<'a>(runs: &'a [Run]) -> Element<'a, Msg> {
    if runs.is_empty() {
        return ui::faint("No runs.").into();
    }
    table(
        &[(3, "ID"), (2, "Kind"), (2, "State"), (1, "VMs"), (3, "Started")],
        runs.iter()
            .map(|r| {
                cells(vec![
                    (3, ui::mono(r.id.as_str(), 12.0, crate::theme::palette().ink).into()),
                    (2, ui::faint(r.role.as_deref().unwrap_or("fleet")).into()),
                    (2, ui::badge(r.state.as_str(), run_tone(&r.state))),
                    (1, ui::mono(r.vms.len().to_string(), 12.0, crate::theme::palette().ink).into()),
                    (3, ui::faint(ago(r.created_at)).into()),
                ])
            })
            .collect(),
    )
}

/// A run's state tone: running is ok, failed is a danger, anything else is muted.
fn run_tone(state: &str) -> ui::Tone {
    match state {
        "running" => ui::Tone::Ok,
        "failed" => ui::Tone::Danger,
        _ => ui::Tone::Muted,
    }
}

/// A value, or a dash when there is none.
fn or_dash(value: &str) -> &str {
    if value.is_empty() {
        "—"
    } else {
        value
    }
}

/// The state a node shows, most urgent first.
fn node_state(n: &NodeView) -> (&'static str, ui::Tone) {
    if n.revoked {
        ("revoked", ui::Tone::Danger)
    } else if n.draining {
        ("draining", ui::Tone::Warn)
    } else if n.online {
        ("online", ui::Tone::Ok)
    } else {
        ("offline", ui::Tone::Muted)
    }
}

/// A join code's label and tone.
fn code_state(state: &str) -> (String, ui::Tone) {
    let (label, tone) = match state {
        "active" => ("Active", ui::Tone::Ok),
        "used" => ("Used", ui::Tone::Info),
        "revoked" => ("Revoked", ui::Tone::Danger),
        "expired" => ("Expired", ui::Tone::Muted),
        other => return (other.to_string(), ui::Tone::Muted),
    };
    (label.to_string(), tone)
}

/// The CID range a node's agents hold, first to last.
fn cids(n: &NodeView) -> String {
    if n.cid_count > 0 {
        format!("{}–{}", n.cid_base, n.cid_base + n.cid_count - 1)
    } else {
        "—".to_string()
    }
}

/// cpus · memory GiB · host, as the node reported them.
fn machine_text(machine: &Option<Machine>) -> String {
    let Some(m) = machine else {
        return "—".to_string();
    };
    let cpus = m.cpus.map_or("?".to_string(), |c| c.to_string());
    let memory = match m.memory_mib {
        Some(mib) if mib > 0 => format!("{:.1} GiB", mib as f64 / 1024.0),
        _ => "?".to_string(),
    };
    format!("{cpus} cpus · {memory} · {}", m.host.as_deref().unwrap_or("?"))
}

/// mm:ss left, 00:00 once passed.
fn countdown(left_ms: f64) -> String {
    let s = (left_ms / 1000.0).floor().max(0.0) as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// A relative time since `ms` (the web's `ago`); "—" for no time.
fn ago(ms: f64) -> String {
    if ms <= 0.0 {
        return "—".to_string();
    }
    let s = ((now_ms() - ms) / 1000.0).round().max(0.0) as u64;
    match s {
        0..=4 => "just now".to_string(),
        5..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

/// Milliseconds since the Unix epoch.
fn now_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

/// A card's frame: a 1px rule border, square corners.
fn boxed(_: &Theme) -> container::Style {
    container::Style {
        border: Border { color: crate::theme::palette().rule, width: 1.0, radius: 0.0.into() },
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_json_parses_facts_gpus_and_runs() {
        let json = r#"{"id":"n1","class":"desktop","arch":"x86_64","backend":"kvm","irqchip":"split","vsock_mode":"vhost","cpus":16,"memory_mib":65536,"gpus":[{"vendor":"nvidia","name":"RTX 4090","memory_mib":24576}],"runs":[{"id":"r1","state":"running","vcpus":1,"memory_mib":256,"role":"worker","created_at":1700000000000.0,"vms":[{"index":0,"state":"running"}]}]}"#;
        let m: PlazaMachine = serde_json::from_str(json).expect("machine JSON parses");
        assert_eq!(m.id, "n1");
        assert_eq!((m.cpus, m.memory_mib), (16, 65536));
        assert_eq!(m.gpus[0].name, "RTX 4090");
        assert_eq!(m.gpus[0].memory_mib, Some(24576));
        assert_eq!(gpu_text(&m.gpus[0]), "nvidia RTX 4090 (24 GiB)");
        assert_eq!(m.runs[0].role.as_deref(), Some("worker"));
        assert_eq!(m.runs[0].vms.len(), 1);
    }

    #[test]
    fn machine_json_tolerates_a_partial_reply() {
        let m: PlazaMachine = serde_json::from_str(r#"{"id":"n0"}"#).expect("partial machine JSON parses");
        assert!(m.gpus.is_empty() && m.runs.is_empty());
        assert_eq!(m.cpus, 0);
    }
}
