//! The MCP screen (parity with the web console's `mcp` view): the servers the plaza runs for agents as a table
//! (start or stop, logs, test, edit, remove), the open server's tools and logs, the test result, the remove
//! confirmation, and the catalogue of presets. Adding and editing wait for the wizard. "This hub as a server" is
//! the second tab, in mcp_hub.rs. A server's env settings are shown by name only: the values are a vault secret or
//! a literal, and neither is read here.

use std::collections::BTreeMap;

use iced::widget::{Space, button, checkbox, column, container, row, text};
use iced::{Alignment, Background, Border, Element, Length, Task, Theme};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::Id;
use super::mcp_hub;
use super::mcp_wizard;
use crate::console::{Ctx, client, ui};

const LOG_LINES: usize = 100;

// ---- The hub's shapes (as apps/web/src/api.ts types them) -----------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Server {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub transport: Transport,
    /// Env settings by name. The values are never shown.
    pub env: BTreeMap<String, Value>,
    pub status: String,
    pub tools_count: u64,
    pub last_error: Option<String>,
    /// When a client last used the server, in milliseconds since the epoch; 0 for never.
    pub last_used_ms: u64,
    /// Persistent storage: a container server may keep a volume between runs.
    pub storage: Storage,
}

/// The storage a server keeps between runs (the hub's `storage`).
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Storage {
    pub volume: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Transport {
    /// `stdio` (a container) or `http` (a remote URL).
    pub kind: String,
    pub image: String,
    pub command: Vec<String>,
    pub url: String,
}

/// One tool: its name, risk, what its MCP annotations say, and whether this hub offers it to agents.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub risk: String,
    pub annotations: Value,
    /// Absent in a test's tools: they are not yet filtered by the hub's allowances.
    pub allowed: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LogLine {
    pub ts_ms: u64,
    pub stream: String,
    pub line: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub description: String,
    pub transport: String,
    pub image: Option<String>,
    pub command: Vec<String>,
    pub url: Option<String>,
    pub env: Vec<mcp_wizard::Spec>,
    pub headers: Vec<mcp_wizard::Spec>,
    pub status: String,
    pub note: String,
}

/// The server a test reached: its name, version and protocol.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
    pub protocol: String,
}

/// The hub's answer to a test: `ok: false` comes back as a 200 with `error` and the stderr tail.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TestOutcome {
    pub ok: bool,
    pub ms: f64,
    pub error: Option<String>,
    pub server_info: Option<ServerInfo>,
    pub tools: Vec<Tool>,
    pub stderr: Vec<LogLine>,
}

/// The path a server's delete takes. `purge` also deletes its storage volume (`?purge=1`).
pub fn delete_path(id: &str, purge: bool) -> String {
    if purge {
        format!("/v1/mcp/servers/{id}?purge=1")
    } else {
        format!("/v1/mcp/servers/{id}")
    }
}

/// A server's status word, as the hub reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Stopped,
    Starting,
    Running,
    Error,
}

impl Status {
    /// An unknown word reads as stopped, as the web does.
    pub fn from_wire(word: &str) -> Status {
        match word {
            "starting" => Status::Starting,
            "running" => Status::Running,
            "error" => Status::Error,
            _ => Status::Stopped,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Status::Stopped => "Stopped",
            Status::Starting => "Starting",
            Status::Running => "Running",
            Status::Error => "Error",
        }
    }

    fn tone(self) -> ui::Tone {
        match self {
            Status::Stopped => ui::Tone::Muted,
            Status::Starting => ui::Tone::Info,
            Status::Running => ui::Tone::Ok,
            Status::Error => ui::Tone::Danger,
        }
    }
}

/// The transport as the web reads it.
pub fn transport_text(transport: &Transport) -> String {
    if transport.kind == "http" {
        format!("remote · {}", transport.url)
    } else {
        format!("container · {}", transport.image)
    }
}

/// The names of a server's env settings, sorted. The values stay in the vault.
pub fn env_names(server: &Server) -> String {
    if server.env.is_empty() {
        "none".to_string()
    } else {
        server.env.keys().cloned().collect::<Vec<_>>().join(", ")
    }
}

/// The error a row shows: a server that is not running shows the first line of its last error, as the web does.
pub fn row_error(server: &Server) -> Option<&str> {
    if Status::from_wire(&server.status) == Status::Running {
        return None;
    }
    server
        .last_error
        .as_deref()
        .and_then(|e| e.lines().next())
        .filter(|line| !line.trim().is_empty())
}

/// A tool's risk, as the web's badge tones it: read is ok, write is info, irreversible is danger.
fn risk_tone(risk: &str) -> ui::Tone {
    match risk {
        "read" => ui::Tone::Ok,
        "write" => ui::Tone::Info,
        "irreversible" => ui::Tone::Danger,
        _ => ui::Tone::Muted,
    }
}

/// The names of a tool's annotations that are set to true, in order (the chips the web draws).
fn annotation_names(a: &Value) -> Vec<String> {
    a.as_object()
        .map(|map| {
            map.iter()
                .filter(|(_, v)| v.as_bool() == Some(true))
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// A log line's clock as HH:MM:SS.mmm (UTC, as the hub's other screens read time here).
fn clock(ms: u64) -> String {
    let day = ms % 86_400_000;
    let h = day / 3_600_000;
    let m = (day / 60_000) % 60;
    let s = (day / 1_000) % 60;
    format!("{h:02}:{m:02}:{s:02}.{:03}", day % 1_000)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---- State and messages ---------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct State {
    /// The servers as last loaded; None until the first load.
    pub servers: Option<Vec<Server>>,
    pub catalogue: Vec<Preset>,
    /// A load error, shown above the sections.
    pub error: Option<String>,
    /// The last outcome (text, is an error).
    pub notice: Option<(String, bool)>,
    /// The server whose tools or logs are open.
    pub selected: Option<String>,
    /// Which of the open server's panels shows: its tools, or its logs.
    pub pane: Pane,
    pub tools: Option<Vec<Tool>>,
    pub logs: Option<Vec<LogLine>>,
    /// The open panel's error.
    pub detail_error: Option<String>,
    /// The last test: the server's id and its outcome, shown until closed.
    pub test: Option<(String, TestOutcome)>,
    /// The server waiting for its remove confirmation.
    pub confirm_remove: Option<String>,
    /// The remove confirmation's box: also delete the server's storage volume.
    pub purge: bool,
    /// A write is on its way to the hub.
    pub busy: bool,
    /// The tab open: the servers, or this hub as a server.
    pub tab: Tab,
    pub hub: mcp_hub::State,
    /// The add or edit wizard, open over the screen.
    pub wizard: Option<mcp_wizard::State>,
}

/// The tabs of the screen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Servers,
    Hub,
}

/// The panels a server opens over the table: its tools (the web's Tools dialog) or its logs (the web's drawer).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Pane {
    #[default]
    Tools,
    Logs,
}

#[derive(Debug, Clone)]
pub enum Msg {
    ServersLoaded(Result<Vec<Server>, String>),
    CatalogueLoaded(Result<Vec<Preset>, String>),
    Open(String, Pane),
    ToolsLoaded(String, Result<Vec<Tool>, String>),
    LogsLoaded(String, Result<Vec<LogLine>, String>),
    Refresh,
    /// "Try again" after a failed load: the screen's load runs once more.
    Retry,
    /// Closes the open server's panel.
    Close,
    Start(String),
    Stop(String),
    Test(String),
    /// A test finished: the server's id, and the hub's outcome or the request's error.
    Tested(String, Result<TestOutcome, String>),
    CloseTest,
    /// A start, stop or remove finished: its outcome text, or the hub's error.
    Done(Result<String, String>),
    AskRemove(String),
    CancelRemove,
    SetPurge(bool),
    Remove(String),
    OpenAdd(Option<String>),
    OpenEdit(String),
    Edited(Result<Value, String>),
    Wiz(mcp_wizard::Msg),
    Tab(Tab),
    Hub(mcp_hub::Msg),
}

/// This screen opens no other screen yet.
pub fn go(_msg: &Msg) -> Option<Id> {
    None
}

// ---- Loading and writing --------------------------------------------------------------------

/// Loads the screen's data (called when it opens and on every poll tick). The open server's tools
/// reload with it; its logs load on demand.
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    if state.tab == Tab::Hub {
        return mcp_hub::load(&state.hub, ctx).map(Msg::Hub);
    }
    let mut tasks = vec![
        servers_task(ctx),
        fetch(ctx, "/v1/mcp/catalog".to_string(), Msg::CatalogueLoaded),
    ];
    if let Some(id) = state.selected.clone() {
        tasks.push(tools_task(ctx, &id));
    }
    Task::batch(tasks)
}

fn fetch<T>(
    ctx: &Ctx,
    path: String,
    map: impl Fn(Result<T, String>) -> Msg + Send + 'static,
) -> Task<Msg>
where
    T: DeserializeOwned + Send + 'static,
{
    let client = ctx.client.clone();
    client::task(move || client.get::<T>(&path), map)
}

fn servers_task(ctx: &Ctx) -> Task<Msg> {
    fetch(ctx, "/v1/mcp/servers".to_string(), Msg::ServersLoaded)
}

fn tools_task(ctx: &Ctx, id: &str) -> Task<Msg> {
    let owned = id.to_string();
    fetch(ctx, format!("/v1/mcp/servers/{id}/tools"), move |res| {
        Msg::ToolsLoaded(owned.clone(), res)
    })
}

fn logs_task(ctx: &Ctx, id: &str) -> Task<Msg> {
    let owned = id.to_string();
    fetch(
        ctx,
        format!("/v1/mcp/servers/{id}/logs?limit={LOG_LINES}"),
        move |res| Msg::LogsLoaded(owned.clone(), res),
    )
}

/// The open panel's tools and logs, reloaded.
fn detail_tasks(state: &State, ctx: &Ctx) -> Task<Msg> {
    match &state.selected {
        Some(id) => Task::batch([tools_task(ctx, id), logs_task(ctx, id)]),
        None => Task::none(),
    }
}

/// One write to the hub; on success the screen reports `done`.
fn act(
    ctx: &Ctx,
    done: String,
    run: impl FnOnce(&client::Client) -> Result<(), String> + Send + 'static,
) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(move || run(&client).map(|()| done), Msg::Done)
}

/// A server's name for a message: the name it was listed under, or its id when it is no longer listed.
fn name_of(state: &State, id: &str) -> String {
    state
        .servers
        .as_ref()
        .and_then(|list| list.iter().find(|s| s.id == id))
        .map(|s| s.name.clone())
        .unwrap_or_else(|| id.to_string())
}

fn close_panel(state: &mut State) {
    state.selected = None;
    state.tools = None;
    state.logs = None;
    state.detail_error = None;
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::ServersLoaded(Ok(list)) => {
            state.servers = Some(list);
            state.error = None;
            Task::none()
        }
        Msg::CatalogueLoaded(Ok(list)) => {
            state.catalogue = list;
            state.error = None;
            Task::none()
        }
        Msg::ServersLoaded(Err(e)) | Msg::CatalogueLoaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::Open(id, pane) => {
            state.selected = Some(id.clone());
            state.pane = pane;
            state.tools = None;
            state.logs = None;
            state.detail_error = None;
            state.confirm_remove = None;
            Task::batch([tools_task(ctx, &id), logs_task(ctx, &id)])
        }
        Msg::ToolsLoaded(id, res) => {
            if state.selected.as_deref() == Some(id.as_str()) {
                match res {
                    Ok(list) => state.tools = Some(list),
                    Err(e) => state.detail_error = Some(e),
                }
            }
            Task::none()
        }
        Msg::LogsLoaded(id, res) => {
            if state.selected.as_deref() == Some(id.as_str()) {
                match res {
                    Ok(list) => state.logs = Some(list),
                    Err(e) => state.detail_error = Some(e),
                }
            }
            Task::none()
        }
        Msg::Refresh => detail_tasks(state, ctx),
        Msg::Retry => load(state, ctx),
        Msg::Close => {
            close_panel(state);
            state.confirm_remove = None;
            Task::none()
        }
        Msg::Start(id) => {
            state.busy = true;
            let done = format!("{} started", name_of(state, &id));
            act(ctx, done, move |c| {
                c.post::<Value>(&format!("/v1/mcp/servers/{id}/start"), &json!({}))
                    .map(|_| ())
            })
        }
        Msg::Stop(id) => {
            state.busy = true;
            let done = format!("{} stopped", name_of(state, &id));
            act(ctx, done, move |c| {
                c.post::<Value>(&format!("/v1/mcp/servers/{id}/stop"), &json!({}))
                    .map(|_| ())
            })
        }
        Msg::Test(id) => {
            state.busy = true;
            state.test = None;
            let client = ctx.client.clone();
            let for_msg = id.clone();
            client::task(
                move || {
                    client.post::<TestOutcome>(&format!("/v1/mcp/servers/{id}/test"), &json!({}))
                },
                move |res| Msg::Tested(for_msg.clone(), res),
            )
        }
        Msg::Tested(id, Ok(outcome)) => {
            state.busy = false;
            state.notice = None;
            state.test = Some((id, outcome));
            Task::batch([servers_task(ctx), detail_tasks(state, ctx)])
        }
        Msg::Tested(_, Err(e)) => {
            state.busy = false;
            state.notice = Some((e, true));
            Task::none()
        }
        Msg::CloseTest => {
            state.test = None;
            Task::none()
        }
        Msg::Done(Ok(label)) => {
            state.busy = false;
            state.notice = Some((label, false));
            Task::batch([servers_task(ctx), detail_tasks(state, ctx)])
        }
        Msg::Done(Err(e)) => {
            state.busy = false;
            state.notice = Some((e, true));
            Task::none()
        }
        Msg::AskRemove(id) => {
            state.confirm_remove = Some(id);
            state.purge = false;
            Task::none()
        }
        Msg::CancelRemove => {
            state.confirm_remove = None;
            state.purge = false;
            Task::none()
        }
        Msg::SetPurge(on) => {
            state.purge = on;
            Task::none()
        }
        Msg::Remove(id) => {
            state.confirm_remove = None;
            let purge = std::mem::take(&mut state.purge);
            if state.selected.as_deref() == Some(id.as_str()) {
                close_panel(state);
            }
            state.busy = true;
            let done = format!("{} deleted", name_of(state, &id));
            act(ctx, done, move |c| {
                c.delete(&delete_path(&id, purge)).map(|_| ())
            })
        }
        Msg::OpenAdd(preset) => {
            state.wizard = Some(mcp_wizard::State::new(
                None,
                preset.as_deref(),
                &state.catalogue,
            ));
            mcp_wizard::start(ctx).map(Msg::Wiz)
        }
        Msg::OpenEdit(id) => {
            let client = ctx.client.clone();
            client::task(
                move || client.get::<Value>(&format!("/v1/mcp/servers/{id}")),
                Msg::Edited,
            )
        }
        Msg::Edited(Ok(view)) => {
            state.wizard = Some(mcp_wizard::State::new(Some(&view), None, &state.catalogue));
            mcp_wizard::start(ctx).map(Msg::Wiz)
        }
        Msg::Edited(Err(e)) => {
            state.notice = Some((e, true));
            Task::none()
        }
        Msg::Wiz(m) => {
            let Some(wiz) = state.wizard.as_mut() else {
                return Task::none();
            };
            match mcp_wizard::update(wiz, m, ctx) {
                mcp_wizard::Outcome::Stay(task) => task.map(Msg::Wiz),
                mcp_wizard::Outcome::Closed(label) => {
                    state.wizard = None;
                    if !label.is_empty() {
                        state.notice = Some((label, false));
                    }
                    servers_task(ctx)
                }
            }
        }
        Msg::Tab(tab) => {
            mcp_hub::leave(&mut state.hub);
            state.tab = tab;
            load(state, ctx)
        }
        Msg::Hub(m) => mcp_hub::update(&mut state.hub, m, ctx).map(Msg::Hub),
    }
}

// ---- The view -------------------------------------------------------------------------------

fn gate(busy: bool, msg: Msg) -> Option<Msg> {
    if busy { None } else { Some(msg) }
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = match &state.wizard {
        Some(wiz) => mcp_wizard::view(wiz).map(Msg::Wiz),
        None => {
            let content: Element<'a, Msg> = match state.tab {
                Tab::Servers => servers_view(state),
                Tab::Hub => mcp_hub::view(&state.hub).map(Msg::Hub),
            };
            column![tab_bar(state.tab), content]
                .spacing(16)
                .width(Length::Fill)
                .into()
        }
    };
    // The page scrolls as the web does: a tall list is not squeezed into the window.
    ui::scroll(ui::sheet(ui::view_head("MCP servers", vec![]), body))
        .height(Length::Fill)
        .into()
}

/// The servers tab: the notices, the open test or remove confirmation, the open panel, the servers and the catalogue.
fn servers_view(state: &State) -> Element<'_, Msg> {
    let mut parts: Vec<Element<'_, Msg>> = Vec::new();
    if let Some((text, is_error)) = &state.notice {
        parts.push(if *is_error {
            ui::error_line(text)
        } else {
            ui::faint(text.clone()).into()
        });
    }
    if let (Some(error), Some(_)) = (&state.error, &state.servers) {
        parts.push(ui::error_line(error));
    }
    if let Some((id, outcome)) = &state.test {
        parts.push(test_card(state, id, outcome));
    }
    if let (Some(id), Some(_)) = (state.confirm_remove.as_deref(), state.servers.as_ref()) {
        parts.push(remove_card(state, id));
    }
    if let (Some(id), Some(_)) = (state.selected.as_deref(), state.servers.as_ref()) {
        parts.push(pane_card(state, id));
    }
    parts.push(servers_section(state));
    parts.push(catalogue_section(state));
    column(parts).spacing(16).width(Length::Fill).into()
}

/// The tab strip, as the web's `.tabs`: the open tab is ink, the other pressable.
fn tab_bar<'a>(tab: Tab) -> Element<'a, Msg> {
    ui::tabs(
        &[
            (Tab::Servers, "MCP servers"),
            (Tab::Hub, "This hub as a server"),
        ],
        tab,
        Msg::Tab,
    )
}

fn servers_section<'a>(state: &'a State) -> Element<'a, Msg> {
    let add = || {
        ui::btn(
            "Add server",
            Some("plus"),
            ui::Kind::Primary,
            Some(Msg::OpenAdd(None)),
        )
    };
    match &state.servers {
        None => match &state.error {
            Some(error) => column![
                ui::empty("triangle-alert", "Could not load MCP servers", error),
                ui::btn_sm("Try again", None, ui::Kind::Ghost, Some(Msg::Retry)),
            ]
            .spacing(12)
            .align_x(Alignment::Center)
            .width(Length::Fill)
            .into(),
            None => ui::faint("Loading MCP servers...").into(),
        },
        Some(list) if list.is_empty() => column![
            ui::empty(
                "server",
                "No MCP servers",
                "Add one to give agents outside tools. The hub runs each server, and agents never see its credentials.",
            ),
            container(add()).width(Length::Fill).center_x(Length::Fill),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into(),
        Some(list) => column![row![add()], table(state, list)]
            .spacing(16)
            .width(Length::Fill)
            .into(),
    }
}

/// The table's column widths, in portions; the actions column takes what its buttons need.
const COL_SERVER: u16 = 3;
const COL_TRANSPORT: u16 = 4;
const COL_STATUS: u16 = 3;
const COL_TOOLS: u16 = 1;
const COL_USED: u16 = 2;

/// The servers as the web's table: a head over a strong rule, a rule between rows, in a raised card.
fn table<'a>(state: &'a State, list: &'a [Server]) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let now = now_ms();
    let head = row![
        cell(COL_SERVER, ui::label("Server")),
        cell(COL_TRANSPORT, ui::label("Transport")),
        cell(COL_STATUS, ui::label("Status")),
        cell(COL_TOOLS, ui::label("Tools")),
        cell(COL_USED, ui::label("Last used")),
        container(ui::label("Actions")).padding([9, 12]),
    ];
    let mut lines: Vec<Element<'a, Msg>> = vec![head.into(), rule(p.rule_strong)];
    let n = list.len();
    for (slot, s) in list.iter().enumerate() {
        lines.push(server_row(state.busy, s, now));
        if slot + 1 < n {
            lines.push(rule(p.rule));
        }
    }
    container(ui::scroll_x(column(lines).width(Length::Fill)).width(Length::Fill))
        .width(Length::Fill)
        .style(ui::card_style)
        .into()
}

/// A table cell at a portion's width, padded as the web's `.tbl td`.
fn cell<'a>(portion: u16, content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    container(content)
        .width(Length::FillPortion(portion))
        .padding([9, 12])
        .into()
}

/// A rule the width of its line, in the given colour.
fn rule<'a>(color: iced::Color) -> Element<'a, Msg> {
    container(Space::new().height(1.0))
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Color(color)),
            ..Default::default()
        })
        .into()
}

/// One server's row: its name and id, its transport, its status and error, its tool count, when it was used,
/// and the row's actions (start or stop, logs, test, edit, remove).
fn server_row<'a>(busy: bool, server: &'a Server, now: u64) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let status = Status::from_wire(&server.status);
    let id = server.id.clone();

    let name = column![
        ui::sans(server.name.clone(), 13.0, p.ink),
        ui::mono(server.id.clone(), 11.0, p.ink_3),
    ]
    .spacing(2);

    let mut status_cells: Vec<Element<'a, Msg>> = vec![ui::badge(status.label(), status.tone())];
    if !server.enabled {
        status_cells.push(ui::badge("Disabled", ui::Tone::Muted));
    }
    let mut status_col: Vec<Element<'a, Msg>> = vec![
        row(status_cells)
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
    ];
    if let Some(error) = row_error(server) {
        status_col.push(ui::sans(error.to_string(), 12.0, p.crit).into());
        status_col.push(ui::btn_sm(
            "View logs",
            Some("file-text"),
            ui::Kind::Ghost,
            Some(Msg::Open(id.clone(), Pane::Logs)),
        ));
    }

    let used = if server.last_used_ms == 0 {
        "never".to_string()
    } else {
        mcp_hub::ago(now, server.last_used_ms)
    };

    let start_stop: Element<'a, Msg> = if status == Status::Running {
        ui::btn_sm(
            "Stop",
            Some("square"),
            ui::Kind::Ghost,
            gate(busy, Msg::Stop(id.clone())),
        )
    } else {
        ui::btn_sm(
            "Start",
            Some("play"),
            ui::Kind::Ghost,
            gate(busy, Msg::Start(id.clone())),
        )
    };
    let actions: Vec<Element<'a, Msg>> = vec![
        start_stop,
        icon_button("file-text", Some(Msg::Open(id.clone(), Pane::Logs))),
        icon_button("check", gate(busy, Msg::Test(id.clone()))),
        icon_button("pencil", gate(busy, Msg::OpenEdit(id.clone()))),
        icon_button("trash-2", gate(busy, Msg::AskRemove(id.clone()))),
    ];

    row![
        cell(COL_SERVER, name),
        cell(
            COL_TRANSPORT,
            ui::mono(
                ui::ellipsize(&transport_text(&server.transport), 56),
                12.0,
                p.ink_2
            ),
        ),
        cell(COL_STATUS, column(status_col).spacing(6)),
        cell(
            COL_TOOLS,
            ui::btn_sm(
                &server.tools_count.to_string(),
                None,
                ui::Kind::Ghost,
                Some(Msg::Open(id.clone(), Pane::Tools)),
            ),
        ),
        cell(COL_USED, ui::faint(used)),
        container(row(actions).spacing(4).align_y(Alignment::Center)).padding([9, 12]),
    ]
    .align_y(Alignment::Center)
    .into()
}

/// A square icon button with no label, as the web's `.iconbtn`: the glyph in ink-2, a hover ground.
fn icon_button<'a>(name: &str, msg: Option<Msg>) -> Element<'a, Msg> {
    let glyph =
        container(ui::icon::<Msg>(name, 14.0, crate::theme::palette().ink_2)).center(Length::Fill);
    button(glyph)
        .on_press_maybe(msg)
        .padding(0)
        .width(Length::Fixed(28.0))
        .height(Length::Fixed(28.0))
        .style(|_: &Theme, status: button::Status| button::Style {
            background: matches!(status, button::Status::Hovered | button::Status::Pressed)
                .then_some(Background::Color(crate::theme::palette().hover)),
            text_color: crate::theme::palette().ink,
            border: Border::default().rounded(crate::theme::skin().radius),
            ..Default::default()
        })
        .into()
}

/// The panel over the table for the open server: its tools (the web's Tools dialog) or its logs (the drawer).
fn pane_card<'a>(state: &'a State, id: &'a str) -> Element<'a, Msg> {
    let name = name_of(state, id);
    match state.pane {
        Pane::Tools => {
            let body: Element<'a, Msg> = match (&state.detail_error, &state.tools) {
                (Some(error), _) => ui::error_line(error),
                (None, None) => ui::faint("Loading the tools").into(),
                (None, Some(list)) if list.is_empty() => {
                    ui::faint("This server offers no tools.").into()
                }
                (None, Some(list)) => {
                    column(list.iter().map(tool_row).collect::<Vec<Element<'a, Msg>>>())
                        .spacing(0)
                        .width(Length::Fill)
                        .into()
                }
            };
            ui::card_with(
                &format!("Tools of {name}"),
                vec![ui::btn_sm("Close", None, ui::Kind::Ghost, Some(Msg::Close))],
                body,
            )
        }
        Pane::Logs => {
            let status = state
                .servers
                .as_ref()
                .and_then(|list| list.iter().find(|s| s.id == id))
                .map(|s| Status::from_wire(&s.status))
                .unwrap_or(Status::Stopped);
            let body: Element<'a, Msg> = match (&state.detail_error, &state.logs) {
                (Some(error), _) => ui::error_line(error),
                (None, None) => ui::faint("Loading").into(),
                (None, Some(lines)) if lines.is_empty() => {
                    ui::empty("file-text", "No output yet", "")
                }
                (None, Some(lines)) => ui::scroll(
                    column(
                        lines
                            .iter()
                            .map(log_line)
                            .collect::<Vec<Element<'a, Msg>>>(),
                    )
                    .spacing(4)
                    .width(Length::Fill),
                )
                .height(Length::Fixed(320.0))
                .into(),
            };
            let actions: Vec<Element<'a, Msg>> = vec![
                ui::badge(status.label(), status.tone()),
                icon_button("refresh-cw", Some(Msg::Refresh)),
                icon_button("x", Some(Msg::Close)),
            ];
            ui::card_with(&format!("Logs for {name}"), actions, body)
        }
    }
}

/// One tool: its name and risk, the annotation chips, and its description. A tool the hub does not offer says so.
fn tool_row<'a>(tool: &Tool) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let mut head: Vec<Element<'a, Msg>> = vec![
        ui::mono(tool.name.clone(), 12.0, p.ink).into(),
        ui::badge(&tool.risk, risk_tone(&tool.risk)),
    ];
    if tool.allowed == Some(false) {
        head.push(ui::badge("Not offered", ui::Tone::Muted));
    }
    let chips: Vec<Element<'a, Msg>> = annotation_names(&tool.annotations)
        .into_iter()
        .map(chip)
        .collect();
    let mut lines: Vec<Element<'a, Msg>> =
        vec![row(head).spacing(8).align_y(Alignment::Center).into()];
    if !chips.is_empty() {
        lines.push(row(chips).spacing(6).align_y(Alignment::Center).into());
    }
    lines.push(ui::prose(ui::ellipsize(&tool.description, 160)).into());
    container(column(lines).spacing(6).width(Length::Fill))
        .padding([10, 0])
        .width(Length::Fill)
        .into()
}

/// An annotation as a pill chip: the name in ink-2 on the sheet ground, outlined.
fn chip<'a>(label: String) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    container(text(label).size(11.5).color(p.ink_2))
        .padding([2, 7])
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Color(p.sheet_2)),
            border: Border {
                color: p.rule_strong,
                width: 1.0,
                radius: 999.0.into(),
            },
            ..Default::default()
        })
        .into()
}

/// One log line: its clock, its stream as a badge, and its text; stderr lines are in crit.
fn log_line<'a>(l: &LogLine) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let stderr = l.stream == "stderr";
    let line_color = if stderr { p.crit } else { p.ink_2 };
    row![
        ui::mono(clock(l.ts_ms), 11.0, p.ink_3),
        ui::badge(
            &l.stream,
            if stderr {
                ui::Tone::Danger
            } else {
                ui::Tone::Muted
            }
        ),
        ui::mono(l.line.clone(), 12.0, line_color),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

/// The result of the last test, as the web's Test dialog: the error and stderr, or the server and its tools.
fn test_card<'a>(state: &'a State, id: &'a str, outcome: &'a TestOutcome) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let name = name_of(state, id);
    let body: Element<'a, Msg> = if !outcome.ok {
        let mut parts: Vec<Element<'a, Msg>> = vec![
            ui::sans(
                outcome
                    .error
                    .clone()
                    .unwrap_or_else(|| "The server did not start.".to_string()),
                13.0,
                p.crit,
            )
            .into(),
        ];
        if !outcome.stderr.is_empty() {
            parts.push(
                column(
                    outcome
                        .stderr
                        .iter()
                        .map(|l| ui::mono(l.line.clone(), 11.0, p.ink_2).into())
                        .collect::<Vec<Element<'a, Msg>>>(),
                )
                .spacing(2)
                .into(),
            );
        }
        column(parts).spacing(8).width(Length::Fill).into()
    } else {
        let mut parts: Vec<Element<'a, Msg>> = Vec::new();
        if let Some(info) = &outcome.server_info {
            parts.push(ui::faint(format!("Server · {}", info.name)).into());
            parts.push(ui::faint(format!("Version · {}", info.version)).into());
            parts.push(ui::faint(format!("Protocol · {}", info.protocol)).into());
        }
        parts.push(ui::label(&format!("Tools ({})", outcome.tools.len())).into());
        if outcome.tools.is_empty() {
            parts.push(ui::faint("This server offers no tools.").into());
        } else {
            parts.push(
                column(
                    outcome
                        .tools
                        .iter()
                        .map(tool_row)
                        .collect::<Vec<Element<'a, Msg>>>(),
                )
                .spacing(0)
                .into(),
            );
        }
        column(parts).spacing(8).width(Length::Fill).into()
    };
    let body: Element<'a, Msg> = if state.busy {
        ui::faint("Testing").into()
    } else {
        body
    };
    ui::card_with(
        &format!("Test {name}"),
        vec![ui::btn_sm(
            "Close",
            None,
            ui::Kind::Ghost,
            Some(Msg::CloseTest),
        )],
        body,
    )
}

/// The remove confirmation as the web's Delete dialog: what goes, and the storage volume only when ticked.
fn remove_card<'a>(state: &'a State, id: &'a str) -> Element<'a, Msg> {
    let server = state
        .servers
        .as_ref()
        .and_then(|list| list.iter().find(|s| s.id == id));
    let name = server
        .map(|s| s.name.clone())
        .unwrap_or_else(|| id.to_string());
    let mut parts: Vec<Element<'a, Msg>> = vec![
        ui::faint(format!(
            "This stops {name} and removes it. Its vault secrets stay, but lose the mcp:{id} binding."
        ))
        .into(),
    ];
    if server.is_some_and(|s| s.storage.volume) {
        parts.push(
            checkbox(state.purge)
                .label("Also delete its storage volume")
                .on_toggle(Msg::SetPurge)
                .into(),
        );
        parts.push(
            ui::faint("The volume keeps what the server stored between runs. It is deleted only when this is ticked.")
                .into(),
        );
    }
    parts.push(
        row![
            ui::btn_sm("Cancel", None, ui::Kind::Ghost, Some(Msg::CancelRemove)),
            ui::btn_sm(
                "Delete",
                None,
                ui::Kind::Danger,
                gate(state.busy, Msg::Remove(id.to_string())),
            ),
        ]
        .spacing(10)
        .align_y(Alignment::Center)
        .into(),
    );
    ui::card(
        &format!("Delete {name}"),
        column(parts).spacing(10).width(Length::Fill).into(),
    )
}

fn catalogue_section<'a>(state: &'a State) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = if state.catalogue.is_empty() {
        ui::empty(
            "terminal",
            "No presets",
            "The catalogue is empty on this hub.",
        )
    } else {
        let n = state.catalogue.len();
        let mut lines: Vec<Element<'a, Msg>> = Vec::new();
        for (slot, preset) in state.catalogue.iter().enumerate() {
            lines.push(preset_row(preset));
            if slot + 1 < n {
                lines.push(rule(crate::theme::palette().rule));
            }
        }
        column(lines).spacing(12).width(Length::Fill).into()
    };
    ui::card_with(
        "Catalogue",
        vec![ui::btn_sm(
            "Custom server",
            None,
            ui::Kind::Ghost,
            Some(Msg::OpenAdd(None)),
        )],
        body,
    )
}

fn preset_row<'a>(preset: &Preset) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let facts: Vec<String> = [preset.transport.clone(), preset.note.clone()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    let info: Vec<Element<'a, Msg>> = vec![
        ui::mono(preset.name.clone(), 13.0, p.ink).into(),
        ui::prose(preset.description.clone()).into(),
        ui::faint(facts.join(" · ")).into(),
    ];
    row![
        column(info).spacing(2).width(Length::Fill),
        ui::btn_sm(
            "Add",
            None,
            ui::Kind::Ghost,
            Some(Msg::OpenAdd(Some(preset.id.clone()))),
        ),
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVERS: &str = r#"[
        {"id":"fs","name":"Files","enabled":true,
         "transport":{"kind":"stdio","image":"ghcr.io/x/fs:1","command":["serve","/data"]},
         "env":{"TOKEN":{"secret":"fs_token"},"ROOT":{"value":"/data"}},
         "headers":{},"status":"error","tools_count":0,"last_error":"exit 1\nno such file","last_used_ms":0,"server_info":null},
        {"id":"web","name":"Search","enabled":false,
         "transport":{"kind":"http","url":"https://mcp.example.com/sse"},
         "env":{},"headers":{},"status":"running","tools_count":4,"last_error":null}
    ]"#;

    const TOOLS: &str = r#"[{"name":"read_file","tool":"read_file","description":"Read a file","input_schema":{},"annotations":{"readOnlyHint":true,"destructiveHint":false},"risk":"read","allowed":true}]"#;

    const LOGS: &str = r#"[{"ts_ms":1,"stream":"stderr","line":"boom"}]"#;

    const TEST_FAILED: &str = r#"{"ok":false,"ms":12.5,"error":"exit 1","stderr":[{"ts_ms":1,"stream":"stderr","line":"no such file"}]}"#;

    const CATALOGUE: &str = r#"[{"id":"fs","name":"Files","description":"Read and write files","transport":"stdio","image":"x","command":[],"url":null,"env":[],"headers":[],"docs":"","status":"","note":"Needs a volume"}]"#;

    #[test]
    fn mcp_servers_parse_into_the_screen_types() {
        let list: Vec<Server> = serde_json::from_str(SERVERS).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "Files");
        assert_eq!(list[0].transport.kind, "stdio");
        assert_eq!(list[0].transport.command.join(" "), "serve /data");
        assert_eq!(list[1].transport.url, "https://mcp.example.com/sse");
        assert!(!list[1].enabled);
        assert_eq!(list[1].tools_count, 4);
        assert_eq!(list[1].last_error, None);
        assert_eq!(list[0].last_used_ms, 0);
    }

    #[test]
    fn mcp_env_shows_names_never_values() {
        let list: Vec<Server> = serde_json::from_str(SERVERS).unwrap();
        assert_eq!(env_names(&list[0]), "ROOT, TOKEN");
        assert!(!env_names(&list[0]).contains("fs_token"));
        assert_eq!(env_names(&list[1]), "none");
        assert_eq!(
            transport_text(&list[0].transport),
            "container · ghcr.io/x/fs:1"
        );
        assert_eq!(
            transport_text(&list[1].transport),
            "remote · https://mcp.example.com/sse"
        );
    }

    #[test]
    fn mcp_status_maps_from_the_hub_word() {
        assert_eq!(Status::from_wire("running"), Status::Running);
        assert_eq!(Status::from_wire("starting"), Status::Starting);
        assert_eq!(Status::from_wire("error"), Status::Error);
        assert_eq!(Status::from_wire("stopped"), Status::Stopped);
        assert_eq!(Status::from_wire("something new"), Status::Stopped);
        assert_eq!(Status::Running.label(), "Running");
        assert_eq!(Status::Running.tone(), ui::Tone::Ok);
        assert_eq!(Status::Error.tone(), ui::Tone::Danger);
    }

    #[test]
    fn mcp_tools_logs_and_test_parse() {
        let tools: Vec<Tool> = serde_json::from_str(TOOLS).unwrap();
        assert_eq!(tools[0].name, "read_file");
        assert_eq!(tools[0].description, "Read a file");
        assert_eq!(tools[0].risk, "read");
        assert_eq!(tools[0].allowed, Some(true));
        assert_eq!(
            annotation_names(&tools[0].annotations),
            vec!["readOnlyHint"]
        );
        let logs: Vec<LogLine> = serde_json::from_str(LOGS).unwrap();
        assert_eq!(
            (logs[0].stream.as_str(), logs[0].line.as_str()),
            ("stderr", "boom")
        );
        let outcome: TestOutcome = serde_json::from_str(TEST_FAILED).unwrap();
        assert!(!outcome.ok);
        assert_eq!(outcome.error.as_deref(), Some("exit 1"));
        assert!(outcome.tools.is_empty());
        assert_eq!(outcome.stderr.len(), 1);
        assert_eq!(
            (
                outcome.stderr[0].stream.as_str(),
                outcome.stderr[0].line.as_str()
            ),
            ("stderr", "no such file")
        );
    }

    #[test]
    fn mcp_test_failure_without_stderr_parses_empty() {
        let outcome: TestOutcome =
            serde_json::from_str(r#"{"ok":false,"ms":1.0,"error":"exit 1"}"#).unwrap();
        assert!(outcome.stderr.is_empty());
    }

    #[test]
    fn mcp_server_storage_parses_and_defaults_off() {
        let list: Vec<Server> = serde_json::from_str(SERVERS).unwrap();
        assert!(!list[0].storage.volume, "no storage key means no volume");
        let with: Server =
            serde_json::from_str(r#"{"id":"fs","name":"Files","storage":{"volume":true}}"#)
                .unwrap();
        assert!(with.storage.volume);
    }

    #[test]
    fn mcp_delete_path_purges_only_when_asked() {
        assert_eq!(delete_path("fs", false), "/v1/mcp/servers/fs");
        assert_eq!(delete_path("fs", true), "/v1/mcp/servers/fs?purge=1");
    }

    #[test]
    fn mcp_catalogue_parses() {
        let presets: Vec<Preset> = serde_json::from_str(CATALOGUE).unwrap();
        assert_eq!(presets[0].name, "Files");
        assert_eq!(presets[0].transport, "stdio");
        assert_eq!(presets[0].note, "Needs a volume");
    }

    #[test]
    fn mcp_log_clock_reads_hours_minutes_seconds_and_millis() {
        // 01:02:03.004 UTC on any day.
        assert_eq!(clock(3_723_004), "01:02:03.004");
        assert_eq!(clock(86_400_000 + 5), "00:00:00.005");
    }
}
