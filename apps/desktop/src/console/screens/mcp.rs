//! The MCP screen (parity with the web console's `mcp` view): the servers the plaza runs for agents
//! (start, stop, test, remove), one server's tools, its config summary and logs, and the catalogue of
//! presets. Adding and editing wait for the wizard (parity item 5); "This hub as a server" waits for
//! parity item 6. A server's env settings are shown by name only: the values are a vault secret or a
//! literal, and neither is read here.

use std::collections::BTreeMap;

use iced::widget::{column, row, scrollable};
use iced::{Alignment, Element, Length, Task};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

use super::Id;
use crate::console::{client, ui, Ctx};

const ADD_NOTICE: &str = "Adding and editing servers is in the next update; use the web console meanwhile.";
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

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Tool {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LogLine {
    pub stream: String,
    pub line: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Preset {
    pub name: String,
    pub description: String,
    pub transport: String,
    pub note: String,
}

/// The hub's answer to a test: `ok: false` comes back as a 200 with `error`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TestOutcome {
    pub ok: bool,
    pub ms: f64,
    pub error: Option<String>,
    pub tools: Vec<Value>,
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
    server.last_error.as_deref().and_then(|e| e.lines().next()).filter(|line| !line.trim().is_empty())
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
    /// The server whose detail is open.
    pub selected: Option<String>,
    pub tools: Option<Vec<Tool>>,
    pub logs: Option<Vec<LogLine>>,
    /// The open detail's error.
    pub detail_error: Option<String>,
    /// The server waiting for its remove confirmation.
    pub confirm_remove: Option<String>,
    /// A write is on its way to the hub.
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    ServersLoaded(Result<Vec<Server>, String>),
    CatalogueLoaded(Result<Vec<Preset>, String>),
    Select(String),
    ToolsLoaded(String, Result<Vec<Tool>, String>),
    LogsLoaded(String, Result<Vec<LogLine>, String>),
    Refresh,
    Close,
    Start(String),
    Stop(String),
    Test(String),
    Tested(Result<TestOutcome, String>),
    /// A start, stop or remove finished: its outcome text, or the hub's error.
    Done(Result<String, String>),
    AskRemove(String),
    CancelRemove,
    Remove(String),
    AddNotice,
}

/// This screen opens no other screen yet: the hub-as-a-server tab comes with parity item 6.
pub fn go(_msg: &Msg) -> Option<Id> {
    None
}

// ---- Loading and writing --------------------------------------------------------------------

/// Loads the screen's data (called when it opens and on every poll tick). The open server's tools
/// reload with it; its logs load on demand.
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let mut tasks = vec![servers_task(ctx), fetch(ctx, "/v1/mcp/catalog".to_string(), Msg::CatalogueLoaded)];
    if let Some(id) = state.selected.clone() {
        tasks.push(tools_task(ctx, &id));
    }
    Task::batch(tasks)
}

fn fetch<T>(ctx: &Ctx, path: String, map: impl Fn(Result<T, String>) -> Msg + Send + 'static) -> Task<Msg>
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
    fetch(ctx, format!("/v1/mcp/servers/{id}/tools"), move |res| Msg::ToolsLoaded(owned.clone(), res))
}

fn logs_task(ctx: &Ctx, id: &str) -> Task<Msg> {
    let owned = id.to_string();
    fetch(ctx, format!("/v1/mcp/servers/{id}/logs?limit={LOG_LINES}"), move |res| Msg::LogsLoaded(owned.clone(), res))
}

/// The open detail's tools and logs, reloaded.
fn detail_tasks(state: &State, ctx: &Ctx) -> Task<Msg> {
    match &state.selected {
        Some(id) => Task::batch([tools_task(ctx, id), logs_task(ctx, id)]),
        None => Task::none(),
    }
}

/// One write to the hub; on success the screen reports `done`.
fn act(ctx: &Ctx, done: String, run: impl FnOnce(&client::Client) -> Result<(), String> + Send + 'static) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(move || run(&client).map(|()| done), Msg::Done)
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
        Msg::Select(id) => {
            state.selected = Some(id.clone());
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
        Msg::Close => {
            state.selected = None;
            state.tools = None;
            state.logs = None;
            state.detail_error = None;
            state.confirm_remove = None;
            Task::none()
        }
        Msg::Start(id) => {
            state.busy = true;
            let done = format!("Started {id}.");
            act(ctx, done, move |c| c.post::<Value>(&format!("/v1/mcp/servers/{id}/start"), &json!({})).map(|_| ()))
        }
        Msg::Stop(id) => {
            state.busy = true;
            let done = format!("Stopped {id}.");
            act(ctx, done, move |c| c.post::<Value>(&format!("/v1/mcp/servers/{id}/stop"), &json!({})).map(|_| ()))
        }
        Msg::Test(id) => {
            state.busy = true;
            let client = ctx.client.clone();
            client::task(move || client.post::<TestOutcome>(&format!("/v1/mcp/servers/{id}/test"), &json!({})), Msg::Tested)
        }
        Msg::Tested(Ok(outcome)) => {
            state.busy = false;
            state.notice = Some(if outcome.ok {
                (format!("Test passed: {} tools in {:.0} ms.", outcome.tools.len(), outcome.ms), false)
            } else {
                (outcome.error.unwrap_or_else(|| "Test failed.".to_string()), true)
            });
            Task::batch([servers_task(ctx), detail_tasks(state, ctx)])
        }
        Msg::Tested(Err(e)) => {
            state.busy = false;
            state.notice = Some((e, true));
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
            Task::none()
        }
        Msg::CancelRemove => {
            state.confirm_remove = None;
            Task::none()
        }
        Msg::Remove(id) => {
            state.confirm_remove = None;
            if state.selected.as_deref() == Some(id.as_str()) {
                state.selected = None;
                state.tools = None;
                state.logs = None;
                state.detail_error = None;
            }
            state.busy = true;
            let done = format!("Removed {id}.");
            act(ctx, done, move |c| c.delete(&format!("/v1/mcp/servers/{id}")).map(|_| ()))
        }
        Msg::AddNotice => {
            state.notice = Some((ADD_NOTICE.to_string(), false));
            Task::none()
        }
    }
}

// ---- The view -------------------------------------------------------------------------------

fn gate(busy: bool, msg: Msg) -> Option<Msg> {
    if busy {
        None
    } else {
        Some(msg)
    }
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = vec![tab_bar(), ui::faint("Servers the plaza runs for agents. Keys stay in the vault; only the names of a server's env settings are shown here.").into()];
    if let Some((text, is_error)) = &state.notice {
        parts.push(if *is_error { ui::error_line(text) } else { ui::faint(text.clone()).into() });
    }
    if let Some(error) = &state.error {
        parts.push(ui::error_line(error));
    }
    parts.push(servers_section(state));
    if let (Some(id), Some(_)) = (state.selected.as_deref(), state.servers.as_ref()) {
        parts.push(detail_section(state, id));
    }
    parts.push(catalogue_section(state));
    let body: Element<'a, Msg> = column(parts).spacing(22).width(Length::Fill).into();
    // The page scrolls as the web does: a tall list is not squeezed into the window.
    scrollable(ui::sheet(ui::view_head("MCP", vec![]), body)).height(Length::Fill).into()
}

/// The tab strip. The hub tab is disabled until parity item 6 lands.
fn tab_bar<'a>() -> Element<'a, Msg> {
    let items: Vec<Element<'a, Msg>> = vec![
        ui::btn_sm::<Msg>("Servers", None, ui::Kind::Secondary, None),
        ui::btn_sm::<Msg>("This hub as a server", None, ui::Kind::Ghost, None),
        ui::faint("Next update.").into(),
    ];
    row(items).spacing(8).align_y(Alignment::Center).into()
}

fn servers_section<'a>(state: &'a State) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = match &state.servers {
        None => ui::loading(),
        Some(list) if list.is_empty() => ui::empty("terminal", "No servers yet", "Add one from the catalogue below, or in the web console."),
        Some(list) => column(list.iter().map(|s| server_row(state.busy, s)).collect::<Vec<Element<'a, Msg>>>()).spacing(18).into(),
    };
    ui::card("Servers", body)
}

fn server_row<'a>(busy: bool, server: &Server) -> Element<'a, Msg> {
    let status = Status::from_wire(&server.status);
    let mut head: Vec<Element<'a, Msg>> = vec![ui::link(&server.name, Msg::Select(server.id.clone()), false), ui::badge(status.label(), status.tone())];
    if !server.enabled {
        head.push(ui::badge("Disabled", ui::Tone::Muted));
    }
    let facts: Element<'a, Msg> = ui::faint(format!("{} · {} tools", transport_text(&server.transport), server.tools_count)).into();
    let mut lines: Vec<Element<'a, Msg>> = vec![row(head).spacing(10).align_y(Alignment::Center).into(), facts];
    if let Some(error) = row_error(server) {
        lines.push(ui::error_line(error));
    }
    let id = server.id.clone();
    let actions: Vec<Element<'a, Msg>> = vec![
        ui::btn_sm("Start", None, ui::Kind::Secondary, gate(busy, Msg::Start(id.clone()))),
        ui::btn_sm("Stop", None, ui::Kind::Secondary, gate(busy, Msg::Stop(id.clone()))),
        ui::btn_sm("Test", None, ui::Kind::Secondary, gate(busy, Msg::Test(id))),
    ];
    lines.push(row(actions).spacing(8).into());
    column(lines).spacing(8).into()
}

fn detail_section<'a>(state: &'a State, id: &'a str) -> Element<'a, Msg> {
    let Some(server) = state.servers.as_ref().and_then(|list| list.iter().find(|s| s.id == id)) else {
        return ui::card("Server", ui::empty("terminal", "Not listed", "This server is no longer on the hub."));
    };
    let mut parts: Vec<Element<'a, Msg>> = vec![ui::faint(transport_text(&server.transport)).into()];
    if !server.transport.command.is_empty() {
        parts.push(ui::mono(format!("$ {}", server.transport.command.join(" ")), 12.0, ui::INK_2).into());
    }
    parts.push(ui::faint(format!("Env: {}", env_names(server))).into());
    if let Some(error) = &state.detail_error {
        parts.push(ui::error_line(error));
    }
    parts.push(ui::label("Tools").into());
    let tools: Element<'a, Msg> = match &state.tools {
        None => ui::loading(),
        Some(list) if list.is_empty() => ui::faint("This server has no tools.").into(),
        Some(list) => column(list.iter().map(tool_row).collect::<Vec<Element<'a, Msg>>>()).spacing(10).into(),
    };
    parts.push(tools);
    parts.push(ui::label("Logs").into());
    let logs: Element<'a, Msg> = match &state.logs {
        None => ui::loading(),
        Some(lines) if lines.is_empty() => ui::faint("No log lines yet.").into(),
        Some(lines) => column(lines.iter().map(|l| ui::mono(format!("{} {}", l.stream, l.line), 11.0, ui::INK_2).into()).collect::<Vec<Element<'a, Msg>>>()).spacing(2).into(),
    };
    parts.push(scrollable(logs).height(Length::Fixed(240.0)).into());
    let actions: Vec<Element<'a, Msg>> = vec![
        ui::btn_sm("Refresh", None, ui::Kind::Secondary, gate(state.busy, Msg::Refresh)),
        ui::btn_sm("Remove", None, ui::Kind::Danger, gate(state.busy, Msg::AskRemove(id.to_string()))),
        ui::link("Close", Msg::Close, false),
    ];
    parts.push(row(actions).spacing(12).align_y(Alignment::Center).into());
    if state.confirm_remove.as_deref() == Some(id) {
        let confirm: Vec<Element<'a, Msg>> = vec![
            ui::faint(format!("Remove {}? Its config is deleted and agents lose its tools.", server.name)).into(),
            ui::btn_sm("Cancel", None, ui::Kind::Ghost, Some(Msg::CancelRemove)),
            ui::btn_sm("Remove", None, ui::Kind::Danger, gate(state.busy, Msg::Remove(id.to_string()))),
        ];
        parts.push(row(confirm).spacing(10).align_y(Alignment::Center).into());
    }
    ui::card(&format!("Server · {}", server.name), column(parts).spacing(14).width(Length::Fill).into())
}

fn tool_row<'a>(tool: &Tool) -> Element<'a, Msg> {
    let parts: Vec<Element<'a, Msg>> = vec![ui::mono(tool.name.clone(), 12.0, ui::INK).into(), ui::prose(tool.description.clone()).into()];
    column(parts).spacing(3).into()
}

fn catalogue_section<'a>(state: &'a State) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = if state.catalogue.is_empty() {
        ui::empty("terminal", "No presets", "The catalogue is empty on this hub.")
    } else {
        column(state.catalogue.iter().map(preset_row).collect::<Vec<Element<'a, Msg>>>()).spacing(14).into()
    };
    ui::card("Catalogue", body)
}

fn preset_row<'a>(preset: &Preset) -> Element<'a, Msg> {
    let facts: Vec<String> = [preset.transport.clone(), preset.note.clone()].into_iter().filter(|s| !s.is_empty()).collect();
    let info: Vec<Element<'a, Msg>> = vec![
        ui::mono(preset.name.clone(), 12.0, ui::INK).into(),
        ui::prose(preset.description.clone()).into(),
        ui::faint(facts.join(" · ")).into(),
    ];
    let items: Vec<Element<'a, Msg>> = vec![
        column(info).spacing(4).width(Length::Fill).into(),
        ui::btn_sm("Add", None, ui::Kind::Secondary, Some(Msg::AddNotice)),
    ];
    row(items).spacing(12).align_y(Alignment::Center).into()
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

    const TOOLS: &str = r#"[{"name":"read_file","tool":"read_file","description":"Read a file","input_schema":{},"annotations":null,"risk":"read","allowed":true}]"#;

    const LOGS: &str = r#"[{"ts_ms":1,"stream":"stderr","line":"boom"}]"#;

    const TEST_FAILED: &str = r#"{"ok":false,"ms":12.5,"error":"exit 1","stderr":[]}"#;

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
    }

    #[test]
    fn mcp_env_shows_names_never_values() {
        let list: Vec<Server> = serde_json::from_str(SERVERS).unwrap();
        assert_eq!(env_names(&list[0]), "ROOT, TOKEN");
        assert!(!env_names(&list[0]).contains("fs_token"));
        assert_eq!(env_names(&list[1]), "none");
        assert_eq!(transport_text(&list[0].transport), "container · ghcr.io/x/fs:1");
        assert_eq!(transport_text(&list[1].transport), "remote · https://mcp.example.com/sse");
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
    fn mcp_row_error_shows_the_first_line_only_when_not_running() {
        let list: Vec<Server> = serde_json::from_str(SERVERS).unwrap();
        assert_eq!(row_error(&list[0]), Some("exit 1"));
        assert_eq!(row_error(&list[1]), None);
    }

    #[test]
    fn mcp_tools_logs_and_test_parse() {
        let tools: Vec<Tool> = serde_json::from_str(TOOLS).unwrap();
        assert_eq!(tools[0].name, "read_file");
        assert_eq!(tools[0].description, "Read a file");
        let logs: Vec<LogLine> = serde_json::from_str(LOGS).unwrap();
        assert_eq!((logs[0].stream.as_str(), logs[0].line.as_str()), ("stderr", "boom"));
        let outcome: TestOutcome = serde_json::from_str(TEST_FAILED).unwrap();
        assert!(!outcome.ok);
        assert_eq!(outcome.error.as_deref(), Some("exit 1"));
        assert!(outcome.tools.is_empty());
    }

    #[test]
    fn mcp_catalogue_parses() {
        let presets: Vec<Preset> = serde_json::from_str(CATALOGUE).unwrap();
        assert_eq!(presets[0].name, "Files");
        assert_eq!(presets[0].transport, "stdio");
        assert_eq!(presets[0].note, "Needs a volume");
    }
}
