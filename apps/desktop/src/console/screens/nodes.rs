//! The Nodes screen, drawn as the web console's `nodes` view (the Loom/Graphite prototype): the machines in
//! the plaza (each one's picture, state and facts, opened into its tallies, runs, VMs and technical details;
//! stop, resume, move, revoke), the join card that lends this machine to another network, the join codes that
//! bring new ones in (each token shown once), and this hub's identity with the owner-token rotation. Reloads
//! on the screen's poll.

use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::text::Wrapping;
use iced::widget::{
    Space, Stack, checkbox, column, container, mouse_area, row, svg, text, text_input,
};
use iced::{Alignment, Border, Color, Element, Length, Task, Theme};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Id;
use crate::console::services::{self, Mode};
use crate::console::{Ctx, client, roles, ui};
use crate::theme;

/// The hub's own node: it can be stopped and resumed from here, but not revoked.
const HUB_NODE: &str = "n0";

/// A machine's reported hardware.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Machine {
    pub cpus: Option<u64>,
    pub memory_mib: Option<u64>,
    pub host: Option<String>,
    pub version: Option<String>,
}

/// An agent that cannot leave a stopping machine yet, and why.
#[derive(Debug, Clone, Deserialize)]
pub struct Waiting {
    #[serde(default)]
    pub cid: u64,
    #[serde(default)]
    pub reason: String,
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
    /// "active", "stopping" or "stopped"; empty from an older hub.
    #[serde(default)]
    pub state: String,
    /// Agents still to move off a stopping machine.
    #[serde(default)]
    pub left: u64,
    /// Agents that cannot leave yet, and why.
    #[serde(default)]
    pub waiting: Vec<Waiting>,
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

/// A plaza machine's hardware, GPUs, runs and live stats (`GET /v1/machines`, the web's machine list).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PlazaMachine {
    pub id: String,
    /// `server`, `workstation`, `laptop`, `edge` or `phone`.
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub model: Option<String>,
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
    pub noded_port: Option<u64>,
    #[serde(default)]
    pub cpus: u64,
    #[serde(default)]
    pub memory_mib: u64,
    #[serde(default)]
    pub gpus: Vec<Gpu>,
    #[serde(default)]
    pub runs: Vec<Run>,
    #[serde(default)]
    pub stats: Option<Stats>,
}

/// A machine's live stats: its CPU now and over the last samples, its memory, load and uptime, and the VMs its
/// agents run (the web's `stats`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Stats {
    #[serde(default)]
    pub cpu_percent: f64,
    #[serde(default)]
    pub load: Vec<f64>,
    #[serde(default)]
    pub mem_total_mib: f64,
    #[serde(default)]
    pub mem_used_mib: f64,
    #[serde(default)]
    pub uptime_s: f64,
    #[serde(default)]
    pub vms_running: u64,
    #[serde(default)]
    pub guest_mem_mib: f64,
    #[serde(default)]
    pub history: Vec<Sample>,
}

/// One CPU sample of a machine's history.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Sample {
    #[serde(default)]
    pub cpu_percent: f64,
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
    pub vms: Vec<Vm>,
}

/// A VM of a run: its index in the run, its state, and its guest CID.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Vm {
    #[serde(default)]
    pub index: u64,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub guest_cid: Option<u64>,
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
    Add {
        label: String,
        minutes: u64,
        issued: Option<IssuedCode>,
    },
    /// Stop a machine with `agents` on it: how it stops is chosen in the dialog.
    ConfirmStop {
        id: String,
        name: String,
        agents: u64,
    },
    ConfirmRevoke {
        id: String,
        name: String,
    },
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
    /// The plaza's machines, and the node whose detail is open. Until the owner opens or closes one, the hub's
    /// detail is the open one (as the web's).
    pub machines: Load<Vec<PlazaMachine>>,
    pub selected: Option<String>,
    pub touched: bool,
    /// The runs whose VMs are open under the machine's runs table.
    pub open_runs: Vec<String>,
    /// The technical details of the open machine are shown.
    pub tech_open: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    NodesLoaded(Result<Vec<NodeView>, String>),
    CodesLoaded(Result<Vec<JoinCode>, String>),
    HubLoaded(Result<HubIdentity, String>),
    /// Stop a machine: `now` interrupts its turns in progress, otherwise they finish first.
    Stop {
        id: String,
        name: String,
        now: bool,
    },
    /// Resume a stopped or stopping machine, and move agents back onto it.
    Resume {
        id: String,
        name: String,
    },
    /// Open the stop confirmation for a machine with `agents` on it.
    AskStop {
        id: String,
        name: String,
        agents: u64,
    },
    /// Move agents to spread the load over the machines in service.
    Rebalance,
    /// The rebalance's reply: the moves it made, or its error.
    Rebalanced(Result<Value, String>),
    AskRevoke {
        id: String,
        name: String,
    },
    Revoke {
        id: String,
        name: String,
    },
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
    /// Open a run's VMs, or close them.
    ToggleRun(String),
    /// Show a machine's technical details, or hide them.
    ToggleTech,
    /// Kill a run on its machine.
    KillRun(String),
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
    let (nodes, codes, hub, machines, link) = (
        ctx.client.clone(),
        ctx.client.clone(),
        ctx.client.clone(),
        ctx.client.clone(),
        ctx.client.clone(),
    );
    Task::batch([
        client::task(
            move || nodes.get::<Vec<NodeView>>("/v1/nodes"),
            Msg::NodesLoaded,
        ),
        client::task(
            move || codes.get::<Vec<JoinCode>>("/v1/nodes/codes"),
            Msg::CodesLoaded,
        ),
        client::task(
            move || hub.get::<HubIdentity>("/v1/hub/identity"),
            Msg::HubLoaded,
        ),
        client::task(
            move || machines.get::<Vec<PlazaMachine>>("/v1/machines"),
            Msg::MachinesLoaded,
        ),
        client::task(
            move || link.get::<NodeLink>("/v1/node-link"),
            Msg::NodeLinkLoaded,
        ),
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
        Msg::Stop { id, name, now } => {
            state.dialog = None;
            let notice = if now {
                format!("{name} is stopping: its agents move now")
            } else {
                format!("{name} is stopping: its agents move as their turns end")
            };
            let path = format!("/v1/nodes/{id}/stop");
            let body = json!({ "now": now });
            let client = ctx.client.clone();
            client::task(
                move || client.call("POST", &path, Some(&body)),
                move |r| Msg::Acted(r, notice.clone()),
            )
        }
        Msg::Resume { id, name } => {
            let notice = format!("{name} is back in service: agents move onto it once it reports");
            let path = format!("/v1/nodes/{id}/resume");
            let body = json!({ "rebalance": true });
            let client = ctx.client.clone();
            client::task(
                move || client.call("POST", &path, Some(&body)),
                move |r| Msg::Acted(r, notice.clone()),
            )
        }
        Msg::AskStop { id, name, agents } => {
            state.dialog = Some(Dialog::ConfirmStop { id, name, agents });
            state.error = None;
            Task::none()
        }
        Msg::Rebalance => {
            let body = json!({ "dry_run": false });
            let client = ctx.client.clone();
            client::task(
                move || client.call("POST", "/v1/nodes/rebalance", Some(&body)),
                Msg::Rebalanced,
            )
        }
        Msg::Rebalanced(result) => {
            let moved = result.as_ref().map_or(0, moved_count);
            update(state, Msg::Acted(result, rebalance_notice(moved)), ctx)
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
            client::task(
                move || client.call("POST", &path, None),
                move |r| Msg::Acted(r, notice.clone()),
            )
        }
        Msg::RevokeCode(id) => {
            let path = format!("/v1/nodes/codes/{id}");
            let client = ctx.client.clone();
            client::task(
                move || client.call("DELETE", &path, None),
                |r| Msg::Acted(r, "Join code revoked".to_string()),
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
                        .and_then(|v| {
                            serde_json::from_value::<RotatedToken>(v).map_err(|e| e.to_string())
                        })
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
            state.dialog = Some(Dialog::Add {
                label: String::new(),
                minutes: 30,
                issued: None,
            });
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
            client::task(
                move || client.post::<IssuedCode>("/v1/nodes/codes", &body),
                Msg::Issued,
            )
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
            client::task(
                move || client.post::<NodeLink>("/v1/node-link/join", &body),
                Msg::Joined,
            )
        }
        Msg::Joined(result) => {
            state.node_busy = false;
            match result {
                Ok(link) => {
                    state.code.clear();
                    state.progress = vec![format!(
                        "joined as node {}",
                        link.node_id.as_deref().unwrap_or("?")
                    )];
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
            client::task(
                move || client.post::<NodeLink>("/v1/node-link/leave", &body),
                Msg::Left,
            )
        }
        Msg::MachinesLoaded(result) => {
            state.machines = settle(result);
            Task::none()
        }
        Msg::Select(id) => {
            // A press on the open node closes it; a press on another opens that one.
            let open = if state.touched {
                state.selected.clone()
            } else {
                Some(HUB_NODE.to_string())
            };
            state.selected = if open.as_deref() == Some(id.as_str()) {
                None
            } else {
                Some(id)
            };
            state.touched = true;
            state.tech_open = false;
            Task::none()
        }
        Msg::Deselect => {
            state.selected = None;
            state.touched = true;
            Task::none()
        }
        Msg::ToggleRun(id) => {
            if let Some(at) = state.open_runs.iter().position(|open| *open == id) {
                state.open_runs.remove(at);
            } else {
                state.open_runs.push(id);
            }
            Task::none()
        }
        Msg::ToggleTech => {
            state.tech_open = !state.tech_open;
            Task::none()
        }
        Msg::KillRun(id) => {
            let path = format!("/v1/runs/{id}");
            let notice = format!("Killed run {id}");
            let client = ctx.client.clone();
            client::task(
                move || client.call("DELETE", &path, None),
                move |r| Msg::Acted(r, notice.clone()),
            )
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
    state.progress = vec![
        if start {
            "Starting the node…"
        } else {
            "Stopping the node…"
        }
        .to_string(),
    ];
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

// ---- View --------------------------------------------------------------------------------------

/// The screen: the machines in the plaza, the join card, the join codes and this hub, with the open dialog over them.
/// As the web's: the open node's detail sits under its own block.
pub fn view<'a>(state: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    let local = ctx.client.base() == crate::console::boot::LOCAL_HUB;
    let dialog = dialog_layer(state);
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if dialog.is_none()
        && let Some(error) = &state.error
    {
        parts.push(ui::error_line(error.as_str()));
    }
    if let Some(notice) = &state.notice {
        parts.push(ui::faint(notice.as_str()).into());
    }
    parts.push(machines_card(state));
    parts.push(join_card(state, local));
    parts.push(codes_card(&state.codes));
    parts.push(hub_card(&state.hub));

    let head = ui::view_head(
        "Nodes",
        vec![
            ui::btn("Rebalance", None, ui::Kind::Secondary, Some(Msg::Rebalance)),
            ui::btn("Add a node", None, ui::Kind::Primary, Some(Msg::OpenAdd)),
        ],
    );
    let body: Element<'a, Msg> = column(parts).spacing(16).width(Length::Fill).into();
    let page = ui::scroll(ui::sheet(head, body)).height(Length::Fill);
    match dialog {
        Some(layer) => Stack::with_children(vec![page.into(), layer]).into(),
        None => page.into(),
    }
}

/// The machine of a node, from the plaza's machine list (joined by id).
fn machine_of<'a>(state: &'a State, id: &str) -> Option<&'a PlazaMachine> {
    match &state.machines {
        Load::Ready(list) => list.iter().find(|m| m.id == id),
        _ => None,
    }
}

/// Whether a node's detail is open: this hub's by default, else the one the owner opened (a press closes it).
fn is_open(state: &State, id: &str) -> bool {
    if state.touched {
        state.selected.as_deref() == Some(id)
    } else {
        id == HUB_NODE
    }
}

/// The plaza's machines: one block per node, its detail under the block when open.
fn machines_card<'a>(state: &'a State) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = match &state.nodes {
        Load::Pending => padded(ui::loading()),
        Load::Failed => padded(ui::prose("Could not load the machines.")),
        Load::Ready(list) if list.is_empty() => ui::empty(
            "machine",
            "No machines yet",
            "Add a node here to bring a machine into this network, or join this hub to another network below.",
        ),
        Load::Ready(list) => {
            let mut blocks: Vec<Element<'a, Msg>> = Vec::new();
            for (i, node) in list.iter().enumerate() {
                if i > 0 {
                    blocks.push(ui::hairline());
                }
                blocks.push(node_block(state, node));
            }
            column(blocks).width(Length::Fill).into()
        }
    };
    flush_card("Machines in this plaza", body)
}

/// One node's block: its picture, name, state and facts. A press on it opens or closes its detail.
fn node_block<'a>(state: &'a State, n: &'a NodeView) -> Element<'a, Msg> {
    let m = machine_of(state, &n.id);
    let (state_text, tone) = node_state(n);
    let live = match m {
        Some(m) => machine_live(m),
        None => n.online,
    };
    let kind = m.map(|m| m.kind.as_str()).unwrap_or("workstation");
    let sub = match m {
        Some(m) => [kind_label(&m.kind), m.model.as_deref().unwrap_or("")]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · "),
        None => n.id.clone(),
    };
    let facts: Vec<(&'static str, String)> = vec![
        ("Address", or_dash(&n.address).to_string()),
        ("Agents", n.agents.to_string()),
        ("CIDs", cids(n)),
        ("Machine", machine_text(&n.machine)),
        ("Last seen", ago(n.last_seen_ms)),
    ];
    let p = theme::palette();
    let picture = container(svg_view(art_svg(kind, live), Length::Fill, Length::Fill))
        .width(Length::Fixed(64.0))
        .height(Length::Fixed(48.0))
        .padding(2)
        .style(move |_: &Theme| container::Style {
            background: Some(p.sheet_2.into()),
            border: Border {
                color: if live { p.ok } else { p.rule_strong },
                width: 1.0,
                radius: theme::skin().radius.into(),
            },
            ..Default::default()
        });
    let head = row![
        picture,
        column![
            row![
                text(n.name.clone())
                    .font(theme::SANS_SEMIBOLD)
                    .size(theme::TYPE_UI)
                    .color(p.ink),
                ui::badge_dot(&state_text, tone),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            ui::faint(sub),
            fact_grid(facts),
        ]
        .spacing(4)
        .width(Length::Fill),
    ]
    .spacing(16)
    .align_y(Alignment::Start);
    let mut items: Vec<Element<'a, Msg>> =
        vec![mouse_area(head).on_press(Msg::Select(n.id.clone())).into()];
    if is_open(state, &n.id) {
        items.push(node_detail(state, n, m));
    }
    container(column(items).spacing(16).width(Length::Fill))
        .padding(16)
        .width(Length::Fill)
        .into()
}

/// Whether a machine has a VM running: the web's live test on its runs' VM states.
fn machine_live(m: &PlazaMachine) -> bool {
    m.runs
        .iter()
        .any(|r| r.vms.iter().any(|v| is_live_word(&v.state)))
}

/// A run or VM state that counts as live.
fn is_live_word(s: &str) -> bool {
    let s = s.to_ascii_lowercase();
    ["run", "ready", "up", "active"]
        .iter()
        .any(|w| s.contains(w))
}

/// A run or VM state's tone, as the web's: live is ok, starting is info, failure is danger, the rest muted.
fn tone_of(s: &str) -> ui::Tone {
    let s = s.to_ascii_lowercase();
    if is_live_word(&s) {
        ui::Tone::Ok
    } else if ["start", "boot", "pend", "creat", "launch"]
        .iter()
        .any(|w| s.contains(w))
    {
        ui::Tone::Info
    } else if ["fail", "err", "crash", "kill", "dead"]
        .iter()
        .any(|w| s.contains(w))
    {
        ui::Tone::Danger
    } else {
        ui::Tone::Muted
    }
}

/// The kind of machine, as the web names it.
fn kind_label(kind: &str) -> &'static str {
    match kind {
        "server" => "Server",
        "workstation" => "Workstation",
        "laptop" => "Laptop",
        "edge" => "Edge",
        "phone" => "Phone",
        _ => "Machine",
    }
}

/// The node's open detail: the machine's picture, its sentence, tallies, runs and VMs, the technical details, the
/// node's facts, what waits to move, and its actions.
fn node_detail<'a>(
    state: &'a State,
    n: &'a NodeView,
    m: Option<&'a PlazaMachine>,
) -> Element<'a, Msg> {
    let p = theme::palette();
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(m) = m {
        let live = machine_live(m);
        parts.push(art_view(&m.kind, live));
        parts.push(ui::sans(sentence(m), theme::TYPE_UI, p.ink_2).into());
        match &m.stats {
            Some(stats) => {
                parts.push(tallies(stats));
                parts.push(ui::faint(format!("Up {}", uptime(stats.uptime_s))).into());
            }
            None => parts.push(ui::faint("No live stats.").into()),
        }
        parts.push(ui::label("Runs").into());
        parts.push(runs_table(&m.runs));
        for run in m.runs.iter().filter(|r| state.open_runs.contains(&r.id)) {
            parts.push(vms_box(run));
        }
        parts.push(ui::disclosure(
            "Technical details".to_string(),
            p.ink_3,
            state.tech_open,
            Msg::ToggleTech,
        ));
        if state.tech_open {
            parts.push(
                column![
                    fact_row("Class", ui::mono(or_dash(&m.class), 12.0, p.ink).into()),
                    fact_row("Arch", ui::mono(or_dash(&m.arch), 12.0, p.ink).into()),
                    fact_row("Backend", ui::mono(or_dash(&m.backend), 12.0, p.ink).into()),
                    fact_row("Irqchip", ui::mono(or_dash(&m.irqchip), 12.0, p.ink).into()),
                    fact_row(
                        "vsock",
                        ui::mono(or_dash(&m.vsock_mode), 12.0, p.ink).into()
                    ),
                    fact_row(
                        "noded port",
                        ui::mono(
                            m.noded_port
                                .map_or("—".to_string(), |port| port.to_string()),
                            12.0,
                            p.ink,
                        )
                        .into(),
                    ),
                ]
                .spacing(8)
                .into(),
            );
        }
    }
    let mc = n.machine.as_ref();
    parts.push(
        column![
            fact_row("Address", ui::mono(or_dash(&n.address), 12.0, p.ink).into()),
            fact_row("CIDs", ui::mono(cids(n), 12.0, p.ink).into()),
            fact_row(
                "Last seen",
                ui::sans(ago(n.last_seen_ms), 12.0, p.ink).into()
            ),
            fact_row("Agents", ui::sans(n.agents.to_string(), 12.0, p.ink).into()),
            fact_row(
                "Host",
                ui::mono(
                    mc.and_then(|m| m.host.as_deref()).unwrap_or("—"),
                    12.0,
                    p.ink
                )
                .into(),
            ),
            fact_row(
                "Version",
                ui::mono(
                    mc.and_then(|m| m.version.as_deref()).unwrap_or("—"),
                    12.0,
                    p.ink
                )
                .into(),
            ),
        ]
        .spacing(8)
        .into(),
    );
    if !n.waiting.is_empty() {
        let lines: Vec<Element<'a, Msg>> = n
            .waiting
            .iter()
            .map(|w| ui::sans(waiting_line(w), 12.0, p.ink).into())
            .collect();
        parts.push(
            column![ui::label("Waiting to move"), column(lines).spacing(6)]
                .spacing(8)
                .into(),
        );
    }
    parts.push(
        row(node_actions(n))
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
    );
    column(parts).spacing(16).width(Length::Fill).into()
}

/// The node's buttons by its state, as the web's: Resume or Stop, Move now while it stops, Revoke, or the hub's note.
fn node_actions<'a>(n: &'a NodeView) -> Vec<Element<'a, Msg>> {
    if n.revoked {
        return Vec::new();
    }
    let (id, name) = (n.id.clone(), n.name.clone());
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    match service_state(n) {
        "stopping" => {
            parts.push(ui::btn(
                "Resume",
                None,
                ui::Kind::Secondary,
                Some(Msg::Resume {
                    id: id.clone(),
                    name: name.clone(),
                }),
            ));
            parts.push(ui::btn(
                "Move now",
                None,
                ui::Kind::Secondary,
                Some(Msg::Stop {
                    id: id.clone(),
                    name: name.clone(),
                    now: true,
                }),
            ));
        }
        "stopped" => parts.push(ui::btn(
            "Resume",
            None,
            ui::Kind::Secondary,
            Some(Msg::Resume {
                id: id.clone(),
                name: name.clone(),
            }),
        )),
        _ => parts.push(ui::btn(
            "Stop",
            None,
            ui::Kind::Secondary,
            Some(Msg::AskStop {
                id: id.clone(),
                name: name.clone(),
                agents: n.agents,
            }),
        )),
    }
    if n.id == HUB_NODE {
        parts.push(ui::faint("this hub").into());
    } else {
        parts.push(ui::btn(
            "Revoke",
            None,
            ui::Kind::Secondary,
            Some(Msg::AskRevoke { id, name }),
        ));
    }
    parts
}

/// The four tallies of a machine: its CPU (a sparkline of the last samples, or a bar), its memory, its load and the
/// VMs its agents run, each with its caption.
fn tallies<'a>(s: &Stats) -> Element<'a, Msg> {
    let cpu_visual = if s.history.is_empty() {
        bar(s.cpu_percent)
    } else {
        spark(&s.history)
    };
    let mem_pct = 100.0 * s.mem_used_mib / s.mem_total_mib.max(1.0);
    let load = if s.load.is_empty() {
        "–".to_string()
    } else {
        s.load
            .iter()
            .map(|x| fx1(*x))
            .collect::<Vec<_>>()
            .join(" · ")
    };
    row![
        tally(
            "CPU",
            format!("{}%", s.cpu_percent.round() as i64),
            String::new(),
            Some(cpu_visual)
        ),
        tally(
            "Memory",
            format!("{} / {}", gib(s.mem_used_mib), gib(s.mem_total_mib)),
            "GiB used of GiB".to_string(),
            Some(bar(mem_pct)),
        ),
        tally("Load", load, "1 / 5 / 15 min".to_string(), None),
        tally(
            "Agents' VMs",
            s.vms_running.to_string(),
            format!("{} GiB reserved", gib(s.guest_mem_mib)),
            None,
        ),
    ]
    .spacing(16)
    .width(Length::Fill)
    .into()
}

/// One tally: its label, figure, visual and caption.
fn tally<'a>(
    label: &str,
    fig: String,
    caption: String,
    visual: Option<Element<'a, Msg>>,
) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = vec![
        ui::label(label).into(),
        ui::mono(fig, 18.0, theme::palette().ink).into(),
    ];
    if let Some(visual) = visual {
        items.push(visual);
    }
    if !caption.is_empty() {
        items.push(ui::faint(caption).into());
    }
    column(items)
        .spacing(4)
        .width(Length::FillPortion(1))
        .into()
}

/// A bar filled to `pct` per cent of its width, as the web's `.nd-bar`.
fn bar<'a>(pct: f64) -> Element<'a, Msg> {
    let p = pct.clamp(0.0, 100.0).round() as u16;
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if p > 0 {
        parts.push(
            container(Space::new().height(Length::Fixed(6.0)))
                .width(Length::FillPortion(p))
                .style(|_: &Theme| container::Style {
                    background: Some(theme::palette().signal.into()),
                    ..Default::default()
                })
                .into(),
        );
    }
    if p < 100 {
        parts.push(
            Space::new()
                .width(Length::FillPortion(100 - p))
                .height(Length::Fixed(6.0))
                .into(),
        );
    }
    container(row(parts).width(Length::Fill))
        .height(Length::Fixed(6.0))
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(theme::palette().sheet_2.into()),
            border: Border::default().rounded(3.0),
            ..Default::default()
        })
        .into()
}

/// The CPU history as a sparkline of the last 60 samples on a ruled baseline (the web's `mt-spark`).
fn spark<'a>(history: &[Sample]) -> Element<'a, Msg> {
    let values: Vec<f64> = history
        .iter()
        .map(|s| s.cpu_percent)
        .filter(|v| v.is_finite())
        .collect();
    let values = &values[values.len().saturating_sub(60)..];
    let (w, h) = (160.0_f64, 48.0_f64);
    let p = theme::palette();
    let points = values
        .iter()
        .enumerate()
        .map(|(i, y)| {
            let x = if values.len() == 1 {
                w / 2.0
            } else {
                i as f64 * w / (values.len() - 1) as f64
            };
            let yy = h - 4.0 - (y.clamp(0.0, 100.0) / 100.0) * (h - 8.0);
            format!("{x:.1},{yy:.1}")
        })
        .collect::<Vec<_>>()
        .join(" ");
    let markup = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" preserveAspectRatio="none"><line x1="0" y1="{b}" x2="{w}" y2="{b}" stroke="{rule}" stroke-width="1"/><polyline points="{points}" fill="none" stroke="{ink}" stroke-width="1.5"/></svg>"#,
        b = h - 4.0,
        rule = hex(p.rule_strong),
        ink = hex(p.ink_2),
    );
    svg_view(markup, Length::Fill, Length::Fixed(48.0))
}

/// The runs table: a row per run (its id, role, state, VMs and start, and Kill); a press opens its VMs.
fn runs_table<'a>(runs: &'a [Run]) -> Element<'a, Msg> {
    if runs.is_empty() {
        return ui::faint("No runs.").into();
    }
    let p = theme::palette();
    let head: [(u16, &str); 6] = [
        (3, "Run"),
        (2, "Role"),
        (2, "State"),
        (1, "VMs"),
        (2, "Started"),
        (1, ""),
    ];
    let rows: Vec<Element<'a, Msg>> = runs
        .iter()
        .map(|r| {
            let line: Element<'a, Msg> = cells(vec![
                (3, ui::mono(r.id.clone(), 12.0, p.ink).into()),
                (
                    2,
                    ui::faint(r.role.clone().unwrap_or_else(|| "fleet".to_string())).into(),
                ),
                (2, ui::badge(&r.state, tone_of(&r.state))),
                (1, ui::sans(r.vms.len().to_string(), 12.0, p.ink).into()),
                (2, ui::faint(ago(r.created_at)).into()),
                (
                    1,
                    ui::btn_sm(
                        "Kill",
                        Some("stop"),
                        ui::Kind::Danger,
                        Some(Msg::KillRun(r.id.clone())),
                    ),
                ),
            ]);
            mouse_area(line)
                .on_press(Msg::ToggleRun(r.id.clone()))
                .into()
        })
        .collect();
    table(&head, rows)
}

/// A run's VMs, under its row: each VM's index, state and guest CID.
fn vms_box<'a>(run: &'a Run) -> Element<'a, Msg> {
    let p = theme::palette();
    let mut lines: Vec<Element<'a, Msg>> = vec![ui::label(&run.id).into()];
    if run.vms.is_empty() {
        lines.push(ui::faint("No VMs.").into());
    }
    for vm in &run.vms {
        lines.push(
            row![
                ui::sans(format!("VM {}", vm.index), 12.0, p.ink),
                ui::badge(&vm.state, tone_of(&vm.state)),
                ui::faint(format!(
                    "guest CID {}",
                    vm.guest_cid.map_or("—".to_string(), |c| c.to_string())
                )),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    container(column(lines).spacing(6))
        .padding([8, 12])
        .width(Length::Fill)
        .style(theme::boxed)
        .into()
}

/// The join card: this machine's network link, its state and Leave when it is joined; the join form when not.
fn join_card<'a>(state: &'a State, local: bool) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = vec![ui::prose(
        "Make this hub's machine a node of another network: it lends its machine to that network's agents. Its own plaza keeps running.",
    )
    .into()];
    parts.push(match &state.node {
        Load::Pending => ui::loading(),
        Load::Failed => ui::prose("Could not load the network link.").into(),
        Load::Ready(node) if node.joined => joined_view(node, state, local),
        Load::Ready(_) => join_form(state),
    });
    for line in &state.progress {
        parts.push(ui::mono(line.as_str(), 11.0, theme::palette().ink_3).into());
    }
    if let Some(error) = &state.node_error {
        parts.push(ui::error_line(error.as_str()));
    }
    ui::card(
        "Join another network",
        column(parts).spacing(12).width(Length::Fill).into(),
    )
}

/// The join form: the code, its hint and the Join button.
fn join_form<'a>(state: &'a State) -> Element<'a, Msg> {
    let busy = state.node_busy;
    let code = text_input("Paste the join code", &state.code)
        .font(theme::MONO)
        .style(theme::input)
        .padding(8.0)
        .on_input(Msg::CodeEdit)
        .on_submit(Msg::Join);
    column![
        field(
            "Join code",
            code.into(),
            Some("Get a code from the other network's Nodes screen (Add a node)."),
        ),
        row![ui::btn(
            if busy { "Joining…" } else { "Join" },
            None,
            ui::Kind::Primary,
            (!busy).then_some(Msg::Join),
        )],
    ]
    .spacing(10)
    .into()
}

/// The joined network: its hub, this node's id and CIDs, the node service's state with Start or Stop (on this
/// machine), and Leave (confirmed, with the choice to forget the network).
fn joined_view<'a>(node: &'a NodeLink, state: &'a State, local: bool) -> Element<'a, Msg> {
    let busy = state.node_busy;
    let p = theme::palette();
    let (badge_text, tone) = if node.running {
        ("running", ui::Tone::Ok)
    } else {
        ("stopped", ui::Tone::Muted)
    };
    let toggle: Element<'a, Msg> = if !local {
        ui::faint("Start and stop run on the node's own machine.").into()
    } else if node.running {
        ui::btn(
            "Stop",
            None,
            ui::Kind::Secondary,
            (!busy).then_some(Msg::NodeStop),
        )
    } else {
        ui::btn(
            "Start",
            None,
            ui::Kind::Secondary,
            (!busy).then_some(Msg::NodeStart),
        )
    };
    let mut status = row![ui::badge(badge_text, tone)]
        .spacing(10)
        .align_y(Alignment::Center);
    if let Some(pid) = node.pid {
        status = status.push(ui::faint(format!("pid {pid}")).wrapping(Wrapping::None));
    }
    status = status.push(toggle);
    column![
        fact_row(
            "Hub",
            ui::mono(or_dash(node.hub.as_deref().unwrap_or("")), 12.0, p.ink).into(),
        ),
        fact_row(
            "This node",
            ui::mono(or_dash(node.node_id.as_deref().unwrap_or("")), 12.0, p.ink).into(),
        ),
        fact_row(
            "CIDs",
            ui::mono(or_dash(node.cids.as_deref().unwrap_or("")), 12.0, p.ink).into(),
        ),
        fact_row("Status", status.into()),
        row![ui::btn(
            "Leave",
            None,
            ui::Kind::Secondary,
            (!busy).then_some(Msg::AskLeave)
        )],
    ]
    .spacing(12)
    .into()
}

/// The join codes, newest first, as a flush table: the label, id, when made, when it ends, its state and Revoke.
fn codes_card<'a>(codes: &'a Load<Vec<JoinCode>>) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = match codes {
        Load::Pending => padded(ui::loading()),
        Load::Failed => padded(ui::prose("Could not load the join codes.")),
        Load::Ready(list) if list.is_empty() => ui::empty(
            "key",
            "No join codes yet",
            "Codes made with \"Add a node\" are listed here.",
        ),
        Load::Ready(list) => {
            let mut sorted: Vec<&JoinCode> = list.iter().collect();
            sorted.sort_by(|a, b| b.created_ms.total_cmp(&a.created_ms));
            let head: [(u16, &str); 6] = [
                (3, "Label"),
                (2, "ID"),
                (2, "Created"),
                (2, "Expires"),
                (3, "State"),
                (1, ""),
            ];
            table(&head, sorted.into_iter().map(code_row).collect())
        }
    };
    flush_card("Join codes", body)
}

fn code_row<'a>(c: &'a JoinCode) -> Element<'a, Msg> {
    let p = theme::palette();
    let (label, tone) = code_state(&c.state);
    let state_cell: Element<'a, Msg> = match &c.used_by {
        Some(by) => row![ui::badge(&label, tone), ui::faint(format!(" by {by}"))]
            .spacing(6)
            .align_y(Alignment::Center)
            .into(),
        None => ui::badge(&label, tone),
    };
    let active = c.state == "active";
    let expires: Element<'a, Msg> = if active {
        row![
            ui::faint("in "),
            ui::sans(countdown(c.expires_ms - now_ms()), 12.0, p.ink),
        ]
        .into()
    } else {
        ui::faint(ago(c.expires_ms)).into()
    };
    let action: Element<'a, Msg> = if active {
        ui::btn_sm(
            "Revoke",
            None,
            ui::Kind::Secondary,
            Some(Msg::RevokeCode(c.id.clone())),
        )
    } else {
        Space::new().width(Length::Fill).into()
    };
    cells(vec![
        (
            3,
            text(c.label.clone())
                .font(theme::SANS_SEMIBOLD)
                .size(theme::TYPE_UI)
                .color(p.ink)
                .into(),
        ),
        (2, ui::mono(c.id.clone(), 11.0, p.ink_3).into()),
        (2, ui::faint(ago(c.created_ms)).into()),
        (2, expires),
        (3, state_cell),
        (1, action),
    ])
}

/// This hub's identity and the owner-token rotation.
fn hub_card<'a>(hub: &'a Load<HubIdentity>) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = match hub {
        Load::Pending => ui::loading(),
        Load::Failed => ui::prose("Could not load this hub.").into(),
        Load::Ready(id) => {
            let p = theme::palette();
            let fingerprint: String = id
                .fingerprint
                .clone()
                .unwrap_or_else(|| "no TLS (loopback only)".to_string());
            column![
                fact_row("Name", ui::mono(id.name.clone(), 13.0, p.ink).into()),
                fact_row("URL", ui::mono(id.url.clone(), 12.0, p.ink).into()),
                fact_row("Version", ui::mono(id.version.clone(), 12.0, p.ink).into()),
                fact_row("TLS fingerprint", ui::mono(fingerprint, 11.0, p.ink).into()),
                row![ui::btn(
                    "Rotate owner token",
                    None,
                    ui::Kind::Danger,
                    Some(Msg::AskRotate)
                )],
            ]
            .spacing(12)
            .into()
        }
    };
    ui::card("This hub", body)
}

// ---- Dialogs ------------------------------------------------------------------------------------

/// The dialog over the screen: the leave confirmation, or the open dialog.
fn dialog_layer<'a>(state: &'a State) -> Option<Element<'a, Msg>> {
    if state.leaving {
        return Some(leave_layer(state));
    }
    state
        .dialog
        .as_ref()
        .map(|dialog| dialog_view(state, dialog))
}

/// A dialog, as the web's modal: a sheet on a scrim, its title, body and actions. The screen's error shows in it.
fn layer<'a>(
    title: &str,
    mut body: Vec<Element<'a, Msg>>,
    actions: Vec<Element<'a, Msg>>,
) -> Element<'a, Msg> {
    let p = theme::palette();
    let mut foot: Vec<Element<'a, Msg>> = vec![Space::new().width(Length::Fill).into()];
    foot.extend(actions);
    body.push(row(foot).spacing(8).align_y(Alignment::Center).into());
    let sheet = container(
        column![
            text(title.to_string())
                .font(theme::SANS_SEMIBOLD)
                .size(theme::TYPE_SECTION)
                .color(p.ink),
            column(body).spacing(14).width(Length::Fill),
        ]
        .spacing(16)
        .width(Length::Fill),
    )
    .padding(20)
    .width(Length::Fixed(520.0))
    .style(theme::pop);
    container(sheet)
        .width(Length::Fill)
        .height(Length::Fill)
        .center(Length::Fill)
        .style(theme::scrim)
        .into()
}

/// The screen's error, for a dialog to show (empty when there is none).
fn dialog_error<'a>(state: &'a State) -> Option<Element<'a, Msg>> {
    state.error.as_deref().map(ui::error_line)
}

/// The open dialog: add a node (its label and validity, then the token once issued), the stop and revoke
/// confirmations, the owner token's rotation and the new token.
fn dialog_view<'a>(state: &'a State, dialog: &'a Dialog) -> Element<'a, Msg> {
    let error = dialog_error(state);
    match dialog {
        Dialog::Add {
            label,
            minutes,
            issued: None,
        } => {
            let label_input = text_input("studio-pc", label)
                .on_input(Msg::LabelEdit)
                .on_submit(Msg::CreateCode)
                .padding(8.0)
                .style(theme::input);
            let mut body: Vec<Element<'a, Msg>> = vec![
                field(
                    "Label",
                    label_input.into(),
                    Some("What the machine is called here."),
                ),
                field(
                    "Valid for",
                    ui::seg(
                        &[
                            (15u64, "15 minutes"),
                            (30u64, "30 minutes"),
                            (60u64, "60 minutes"),
                        ],
                        *minutes,
                        Msg::PickMinutes,
                    ),
                    Some("How long the code can be used to join."),
                ),
            ];
            body.extend(error);
            layer(
                "Add a node",
                body,
                vec![
                    ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Close)),
                    ui::btn(
                        "Create join code",
                        None,
                        ui::Kind::Primary,
                        Some(Msg::CreateCode),
                    ),
                ],
            )
        }
        Dialog::Add {
            issued: Some(made), ..
        } => {
            let p = theme::palette();
            let mut body: Vec<Element<'a, Msg>> = vec![
                ui::faint("Shown once. Single use. Anyone with it can join until it expires.")
                    .color(p.warn)
                    .into(),
                copy_box(&made.token),
                row![
                    ui::faint("Expires in "),
                    ui::sans(countdown(made.expires_ms - now_ms()), 12.0, p.ink),
                ]
                .spacing(4)
                .align_y(Alignment::Center)
                .into(),
                ui::faint("On the new machine, run:").into(),
                copy_box(&format!("apz-link join {}", made.token)),
                ui::faint("or paste it into the desktop app's Join a hub.").into(),
            ];
            body.extend(error);
            layer(
                "Add a node",
                body,
                vec![ui::btn(
                    "Close",
                    None,
                    ui::Kind::Secondary,
                    Some(Msg::Close),
                )],
            )
        }
        Dialog::ConfirmStop { id, name, agents } => {
            let mut body: Vec<Element<'a, Msg>> = vec![
                ui::prose(agents_move_line(*agents)).into(),
                ui::prose(if id == HUB_NODE {
                    "This is the hub’s own machine: the hub keeps running, only its agents leave."
                } else {
                    "Once it is stopped, its machine can be turned off. Resume brings it back, and agents move onto it again."
                })
                .into(),
                ui::faint("Stop now interrupts the turns in progress; each one resumes on its agent’s new machine. Otherwise an agent that is working moves when its turn ends.").into(),
            ];
            body.extend(error);
            layer(
                &format!("Stop {name}?"),
                body,
                vec![
                    ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Close)),
                    ui::btn(
                        "Stop after their turns",
                        None,
                        ui::Kind::Secondary,
                        Some(Msg::Stop {
                            id: id.clone(),
                            name: name.clone(),
                            now: false,
                        }),
                    ),
                    ui::btn(
                        "Stop now",
                        None,
                        ui::Kind::Danger,
                        Some(Msg::Stop {
                            id: id.clone(),
                            name: name.clone(),
                            now: true,
                        }),
                    ),
                ],
            )
        }
        Dialog::ConfirmRevoke { id, name } => {
            let mut body: Vec<Element<'a, Msg>> = vec![
                ui::prose("Its signatures stop working at once and its agents go offline.").into(),
            ];
            body.extend(error);
            layer(
                &format!("Revoke {name}?"),
                body,
                vec![
                    ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Close)),
                    ui::btn(
                        "Revoke",
                        None,
                        ui::Kind::Danger,
                        Some(Msg::Revoke {
                            id: id.clone(),
                            name: name.clone(),
                        }),
                    ),
                ],
            )
        }
        Dialog::ConfirmRotate => {
            let mut body: Vec<Element<'a, Msg>> =
                vec![ui::prose("The current owner token stops working at once.").into()];
            body.extend(error);
            layer(
                "Rotate the owner token?",
                body,
                vec![
                    ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::Close)),
                    ui::btn("Rotate", None, ui::Kind::Danger, Some(Msg::Rotate)),
                ],
            )
        }
        Dialog::Rotated(token) => {
            let mut body: Vec<Element<'a, Msg>> = vec![
                ui::prose("The new owner token, shown once:").into(),
                copy_box(token),
                ui::faint("Every signed-in browser must sign in again.").into(),
            ];
            body.extend(error);
            layer(
                "Rotate the owner token?",
                body,
                vec![ui::btn(
                    "Close",
                    None,
                    ui::Kind::Secondary,
                    Some(Msg::Close),
                )],
            )
        }
    }
}

/// The leave confirmation: this node stops lending its machine, and the choice to forget the network too.
fn leave_layer<'a>(state: &'a State) -> Element<'a, Msg> {
    let forget = checkbox(state.forget)
        .label("Forget this network: remove the saved hub, so joining again needs a new code.")
        .on_toggle(Msg::ToggleForget);
    let mut body: Vec<Element<'a, Msg>> = vec![
        ui::prose(
            "This node stops lending its machine to that network, and its agents there go offline.",
        )
        .into(),
        forget.into(),
    ];
    body.extend(dialog_error(state));
    layer(
        "Leave this network?",
        body,
        vec![
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CancelLeave)),
            ui::btn(
                "Leave",
                None,
                ui::Kind::Danger,
                (!state.node_busy).then_some(Msg::Leave),
            ),
        ],
    )
}

// ---- Shared pieces -------------------------------------------------------------------------------

/// A plain card body's padding of 16 around one element.
fn padded<'a>(content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    container(content).padding(16).width(Length::Fill).into()
}

/// A machine's one-line sentence, as the web's: its kind, model, chip and cores, memory, then its GPUs.
fn sentence(m: &PlazaMachine) -> String {
    let cores = if m.cpus > 0 {
        let arch = if m.arch.is_empty() {
            String::new()
        } else {
            format!(" {}", m.arch)
        };
        format!("{}-core{arch}", m.cpus)
    } else {
        m.arch.clone()
    };
    let backend = if m.backend.is_empty() {
        String::new()
    } else {
        format!(" on {}", m.backend.to_ascii_uppercase())
    };
    let memory = if m.memory_mib > 0 {
        format!(
            " · {} GiB memory",
            (m.memory_mib as f64 / 1024.0).round() as u64
        )
    } else {
        String::new()
    };
    let chip = format!("{cores}{backend}{memory}");
    let mut head: Vec<&str> = vec![kind_label(&m.kind)];
    if let Some(model) = m.model.as_deref().filter(|s| !s.is_empty()) {
        head.push(model);
    }
    head.push(chip.as_str());
    let head = head
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    if m.gpus.is_empty() {
        head
    } else {
        let names: Vec<&str> = m.gpus.iter().map(|g| g.name.as_str()).collect();
        format!("{head} · {}", names.join(", "))
    }
}

/// A labelled line: its label in the label voice, its control, then a hint line when there is one.
fn field<'a>(label: &str, control: Element<'a, Msg>, hint: Option<&str>) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = vec![ui::label(label).into(), control];
    if let Some(hint) = hint {
        items.push(ui::faint(hint.to_string()).into());
    }
    column(items).spacing(6).width(Length::Fill).into()
}

/// A labelled fact of the hub's or a node's details: the label at a fixed width, the value beside it.
fn fact_row<'a>(name: &str, value: Element<'a, Msg>) -> Element<'a, Msg> {
    row![
        container(ui::label(name)).width(Length::Fixed(150.0)),
        value
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into()
}

/// A node's facts in a grid of three to a row, as the web's `.nd-grid`: each label and its value.
fn fact_grid<'a>(facts: Vec<(&'static str, String)>) -> Element<'a, Msg> {
    let mut lines: Vec<Element<'a, Msg>> = Vec::new();
    for chunk in facts.chunks(3) {
        let mut cells_row: Vec<Element<'a, Msg>> = chunk
            .iter()
            .map(|(k, v)| {
                container(
                    row![
                        ui::faint(*k),
                        Space::new().width(Length::Fill),
                        ui::sans(v.clone(), 12.0, theme::palette().ink),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                )
                .width(Length::FillPortion(1))
                .into()
            })
            .collect();
        while cells_row.len() < 3 {
            cells_row.push(Space::new().width(Length::FillPortion(1)).into());
        }
        lines.push(row(cells_row).spacing(16).into());
    }
    column(lines).spacing(8).width(Length::Fill).into()
}

/// A card whose body is flush (a table or a list under its head), as the web's flush card.
fn flush_card<'a>(title: &str, body: Element<'a, Msg>) -> Element<'a, Msg> {
    let head = container(ui::card_title(title))
        .padding([8, 16])
        .width(Length::Fill)
        .center_y(Length::Fixed(44.0));
    let rule = container(Space::new().height(Length::Fixed(1.0)))
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(theme::palette().rule.into()),
            ..Default::default()
        });
    container(column![head, rule, body])
        .width(Length::Fill)
        .style(ui::card_style)
        .into()
}

/// A table: a head row in the label voice over a strong rule, each row under a faint rule. Columns are proportional.
fn table<'a>(head: &[(u16, &str)], rows: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let heads: Vec<(u16, Element<'a, Msg>)> = head
        .iter()
        .map(|(portion, label)| (*portion, ui::label(label).into()))
        .collect();
    let strong = container(Space::new().height(Length::Fixed(1.0)))
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(theme::palette().rule_strong.into()),
            ..Default::default()
        });
    let mut lines: Vec<Element<'a, Msg>> = vec![cells(heads), strong.into()];
    for line in rows {
        lines.push(line);
        lines.push(ui::hairline());
    }
    column(lines).width(Length::Fill).into()
}

/// One table row: its cells in proportional columns, padded 9 by 12.
fn cells<'a>(parts: Vec<(u16, Element<'a, Msg>)>) -> Element<'a, Msg> {
    let laid: Vec<Element<'a, Msg>> = parts
        .into_iter()
        .map(|(portion, cell)| container(cell).width(Length::FillPortion(portion)).into())
        .collect();
    container(row(laid).spacing(12).align_y(Alignment::Center))
        .padding([9, 12])
        .width(Length::Fill)
        .into()
}

/// A value to copy: a mono box with a Copy button.
fn copy_box<'a>(value: &str) -> Element<'a, Msg> {
    container(
        row![
            ui::mono(value.to_string(), 12.0, theme::palette().ink).width(Length::Fill),
            ui::btn_sm(
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
    .style(theme::boxed)
    .into()
}

/// A machine's picture as an SVG widget of the given size.
fn svg_view<'a>(markup: String, width: Length, height: Length) -> Element<'a, Msg> {
    svg(svg::Handle::from_memory(markup.into_bytes()))
        .width(width)
        .height(height)
        .into()
}

/// The machine's art, large, in the view box of the node's detail.
fn art_view<'a>(kind: &str, live: bool) -> Element<'a, Msg> {
    container(svg_view(
        art_svg(kind, live),
        Length::Fill,
        Length::Fixed(260.0),
    ))
    .width(Length::Fill)
    .height(Length::Fixed(360.0))
    .center_x(Length::Fill)
    .center_y(Length::Fixed(360.0))
    .style(|_: &Theme| container::Style {
        background: Some(theme::palette().sheet_2.into()),
        border: Border {
            color: theme::palette().rule,
            width: 1.0,
            radius: theme::skin().radius_lg.into(),
        },
        ..Default::default()
    })
    .into()
}

/// The machine's line art: a chassis sized by its kind, LEDs lit while a VM runs, a vent line.
fn art_svg(kind: &str, active: bool) -> String {
    let p = theme::palette();
    let (w, hg) = match kind {
        "edge" => (44.0, 44.0),
        "server" => (56.0, 96.0),
        "laptop" => (96.0, 64.0),
        _ => (64.0, 96.0),
    };
    let x = (160.0 - w) / 2.0;
    let y = (120.0 - hg) / 2.0;
    let led = if active {
        hex(p.ok)
    } else {
        hex(p.rule_strong)
    };
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 160 120"><rect x="{x}" y="{y}" width="{w}" height="{hg}" rx="4" fill="{chassis}" stroke="{stroke}" stroke-width="1.5"/><circle cx="{c1}" cy="{cy}" r="3" fill="{led}"/><circle cx="{c2}" cy="{cy}" r="3" fill="{led}"/><line x1="{vx1}" y1="{vy}" x2="{vx2}" y2="{vy}" stroke="{vent}" stroke-width="1.5"/></svg>"#,
        chassis = hex(p.sheet_2),
        stroke = hex(p.ink_3),
        vent = hex(p.rule_strong),
        c1 = x + 10.0,
        c2 = x + 20.0,
        cy = y + 10.0,
        vx1 = x + 8.0,
        vx2 = x + w - 8.0,
        vy = y + hg - 16.0,
    )
}

/// A colour as the SVG's `#rrggbb`.
fn hex(c: Color) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c.r), b(c.g), b(c.b))
}

/// A value, or a dash when there is none.
fn or_dash(value: &str) -> &str {
    if value.is_empty() { "—" } else { value }
}

/// Where a node is in its stop: "stopping", "stopped" or "active". An older hub sends no state,
/// so there a draining node reads as stopped.
fn service_state(n: &NodeView) -> &str {
    match n.state.as_str() {
        "" if n.draining => "stopped",
        "" => "active",
        state => state,
    }
}

/// The state a node shows, most urgent first.
fn node_state(n: &NodeView) -> (String, ui::Tone) {
    if n.revoked {
        return ("revoked".to_string(), ui::Tone::Danger);
    }
    match service_state(n) {
        "stopping" => (
            format!("stopping, {} to move", agents_text(n.left)),
            ui::Tone::Warn,
        ),
        "stopped" => ("stopped".to_string(), ui::Tone::Info),
        _ if n.online => ("online".to_string(), ui::Tone::Ok),
        _ => ("offline".to_string(), ui::Tone::Muted),
    }
}

/// A count of agents with its noun: "1 agent", "3 agents".
fn agents_text(count: u64) -> String {
    if count == 1 {
        "1 agent".to_string()
    } else {
        format!("{count} agents")
    }
}

/// The stop dialog's line on the agents that move off the node.
fn agents_move_line(agents: u64) -> String {
    let tail = "to the other machines in service, one at a time, and no new agent starts on it.";
    match agents {
        0 => "No new agent starts on it.".to_string(),
        1 => format!("Its agent moves {tail}"),
        n => format!("Its {n} agents move {tail}"),
    }
}

/// A waiting agent's line: its label, and why it cannot leave.
fn waiting_line(w: &Waiting) -> String {
    format!("{}: {}", roles::agent_label("", w.cid), w.reason)
}

/// How many agents a rebalance moved, from the hub's reply.
fn moved_count(reply: &Value) -> usize {
    reply["moves"].as_array().map_or(0, Vec::len)
}

/// The notice a rebalance leaves, by how many agents it moved.
fn rebalance_notice(moved: usize) -> String {
    match moved {
        0 => "The agents are already spread over the machines.".to_string(),
        1 => "Moving 1 agent to spread the load.".to_string(),
        n => format!("Moving {n} agents to spread the load."),
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
    format!(
        "{cpus} cpus · {memory} · {}",
        m.host.as_deref().unwrap_or("?")
    )
}

/// mm:ss left, 00:00 once passed.
fn countdown(left_ms: f64) -> String {
    let s = (left_ms / 1000.0).floor().max(0.0) as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// Gibibytes from mebibytes, to one decimal; "?" for a figure the node did not report.
fn gib(mib: f64) -> String {
    if mib.is_finite() {
        format!("{:.1}", mib / 1024.0)
    } else {
        "?".to_string()
    }
}

/// One decimal, or a dash for a figure the node did not report.
fn fx1(n: f64) -> String {
    if n.is_finite() {
        format!("{n:.1}")
    } else {
        "–".to_string()
    }
}

/// A machine's uptime as the web words it: "42s", "17m", "3h 5m", "2d 4h".
fn uptime(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s < 60 {
        return format!("{s}s");
    }
    let mins = s / 60;
    if mins < 60 {
        return format!("{mins}m");
    }
    let hours = mins / 60;
    if hours < 48 {
        return format!("{hours}h {}m", mins % 60);
    }
    format!("{}d {}h", hours / 24, hours % 24)
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
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
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
        assert_eq!(m.gpus[0].vendor, "nvidia");
        assert_eq!(m.runs[0].role.as_deref(), Some("worker"));
        assert_eq!(m.runs[0].vms.len(), 1);
    }

    #[test]
    fn machine_json_tolerates_a_partial_reply() {
        let m: PlazaMachine =
            serde_json::from_str(r#"{"id":"n0"}"#).expect("partial machine JSON parses");
        assert!(m.gpus.is_empty() && m.runs.is_empty());
        assert_eq!(m.cpus, 0);
    }

    fn node(json: &str) -> NodeView {
        serde_json::from_str(json).expect("node JSON parses")
    }

    #[test]
    fn node_json_reads_the_stop_fields_and_tolerates_an_older_hub() {
        let n = node(
            r#"{"id":"n1","state":"stopping","left":2,"draining":true,"waiting":[{"cid":7,"reason":"no room"}]}"#,
        );
        assert_eq!(
            (n.state.as_str(), n.left, n.waiting.len()),
            ("stopping", 2, 1)
        );
        assert_eq!(n.waiting[0].cid, 7);
        let old = node(r#"{"id":"n0"}"#);
        assert!(old.state.is_empty() && old.waiting.is_empty());
        assert_eq!(service_state(&old), "active");
    }

    #[test]
    fn node_state_label_follows_the_stop_state() {
        let one = node(r#"{"id":"n1","state":"stopping","left":1,"draining":true}"#);
        assert_eq!(
            node_state(&one),
            ("stopping, 1 agent to move".to_string(), ui::Tone::Warn)
        );
        let three = node(r#"{"id":"n1","state":"stopping","left":3,"draining":true}"#);
        assert_eq!(node_state(&three).0, "stopping, 3 agents to move");
        let stopped = node(r#"{"id":"n1","state":"stopped","draining":true}"#);
        assert_eq!(
            node_state(&stopped),
            ("stopped".to_string(), ui::Tone::Info)
        );
        let draining = node(r#"{"id":"n1","draining":true}"#);
        assert_eq!(node_state(&draining).0, "stopped");
        let revoked = node(r#"{"id":"n1","revoked":true,"state":"stopped"}"#);
        assert_eq!(
            node_state(&revoked),
            ("revoked".to_string(), ui::Tone::Danger)
        );
        let online = node(r#"{"id":"n1","online":true}"#);
        assert_eq!(node_state(&online), ("online".to_string(), ui::Tone::Ok));
        let offline = node(r#"{"id":"n1"}"#);
        assert_eq!(
            node_state(&offline),
            ("offline".to_string(), ui::Tone::Muted)
        );
    }

    #[test]
    fn waiting_line_labels_the_agent_and_its_reason() {
        let n = node(
            r#"{"id":"n1","state":"stopping","waiting":[{"cid":42,"reason":"no machine has room"}]}"#,
        );
        assert_eq!(
            waiting_line(&n.waiting[0]),
            "agent #42: no machine has room"
        );
    }

    #[test]
    fn stop_dialog_counts_the_agents_that_move() {
        assert_eq!(agents_move_line(0), "No new agent starts on it.");
        assert!(agents_move_line(1).starts_with("Its agent moves to the other machines"));
        assert!(agents_move_line(4).starts_with("Its 4 agents move to the other machines"));
    }

    #[test]
    fn rebalance_notice_reports_the_moves() {
        assert_eq!(
            rebalance_notice(0),
            "The agents are already spread over the machines."
        );
        assert_eq!(rebalance_notice(1), "Moving 1 agent to spread the load.");
        assert_eq!(rebalance_notice(3), "Moving 3 agents to spread the load.");
        let reply = json!({"moves": [{"cid": 1, "from": "n1", "to": "n2"}], "started": true});
        assert_eq!(moved_count(&reply), 1);
        assert_eq!(moved_count(&json!({"started": false})), 0);
    }
}
