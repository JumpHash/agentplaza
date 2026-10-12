//! The Nodes screen (parity with the web console's `nodes` view): the machines in the plaza
//! (drain, resume, revoke), the join codes that bring new ones in (each token shown once), and
//! this hub's identity with the owner-token rotation. Reloads on the screen's poll.

use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{column, container, row, text_input, Space};
use iced::{Alignment, Border, Element, Length, Task, Theme};
use serde::Deserialize;
use serde_json::{json, Value};

use super::Id;
use crate::console::{client, ui, Ctx};
use crate::theme::WARN;

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

/// A new owner token, shown once.
#[derive(Debug, Clone, Deserialize)]
pub struct RotatedToken {
    pub token: String,
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
    let (nodes, codes, hub) = (ctx.client.clone(), ctx.client.clone(), ctx.client.clone());
    Task::batch([
        client::task(move || nodes.get::<Vec<NodeView>>("/v1/nodes"), Msg::NodesLoaded),
        client::task(move || codes.get::<Vec<JoinCode>>("/v1/nodes/codes"), Msg::CodesLoaded),
        client::task(move || hub.get::<HubIdentity>("/v1/hub/identity"), Msg::HubLoaded),
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
    }
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
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
        Load::Ready(list) if list.is_empty() => ui::empty("link", "No machines yet.", ""),
        Load::Ready(list) => table(
            &[(3, "Name"), (2, "State"), (2, "Address"), (1, "Agents"), (1, "CIDs"), (3, "Machine"), (2, "Last seen"), (3, "")],
            list.iter().map(node_row).collect(),
        ),
    }
}

fn node_row<'a>(n: &'a NodeView) -> Element<'a, Msg> {
    let (state, tone) = node_state(n);
    cells(vec![
        (3, column![ui::mono(n.name.as_str(), 13.0, ui::INK), ui::faint(n.id.as_str())].spacing(2).into()),
        (2, ui::badge(state, tone)),
        (2, ui::mono(if n.address.is_empty() { "—" } else { n.address.as_str() }, 12.0, ui::INK).into()),
        (1, ui::mono(n.agents.to_string(), 12.0, ui::INK).into()),
        (1, ui::mono(cids(n), 12.0, ui::INK).into()),
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
        (3, ui::mono(c.label.as_str(), 13.0, ui::INK).into()),
        (2, ui::mono(c.id.as_str(), 11.0, ui::INK_3).into()),
        (2, ui::faint(ago(c.created_ms)).into()),
        (2, ui::mono(expires, 12.0, ui::INK).into()),
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
                Some(fp) => ui::mono(fp.as_str(), 11.0, ui::INK).into(),
                None => ui::faint("no TLS (loopback only)").into(),
            };
            column![
                fact("Name", ui::mono(id.name.as_str(), 13.0, ui::INK).into()),
                fact("URL", ui::mono(id.url.as_str(), 12.0, ui::INK).into()),
                fact("Version", ui::mono(id.version.as_str(), 12.0, ui::INK).into()),
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
        ui::faint("Shown once. Single use. Anyone with it can join until it expires.").color(WARN),
        copy_box(&made.token),
        row![
            ui::faint("Expires in "),
            ui::mono(countdown(made.expires_ms - now_ms()), 12.0, ui::INK),
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
            ui::mono(value.to_string(), 12.0, ui::INK).width(Length::Fill),
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
        border: Border { color: ui::RULE, width: 1.0, radius: 0.0.into() },
        ..Default::default()
    }
}
