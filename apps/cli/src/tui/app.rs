//! The terminal UI's state, loop and threads. The poller fetches the hub every 3 s (the agents and
//! the plaza's state, plus the visible tab's endpoint); the event thread holds the `/v1/ws` socket.
//! Both send to the UI thread, which owns all the state and draws about 20 times a second.

use std::collections::{HashMap, VecDeque};
use std::collections::vec_deque;
use std::io::{self, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use apz_http::client::Target;
use apz_http::ws;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Color;
use serde_json::{Value, json};

use super::floor::FloorView;
use super::screens::{self, clip};
use crate::ctx::{Client, seg};

const FRAME: Duration = Duration::from_millis(50);
const POLL_EVERY: Duration = Duration::from_secs(3);
const EVENT_LOG_LINES: usize = 200;

/// The tabs, in the order of their number keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Floor,
    Overview,
    Agents,
    Tasks,
    Approvals,
    Board,
    Conversations,
    Nodes,
}

impl Tab {
    pub const ALL: [Tab; 8] = [
        Tab::Floor,
        Tab::Overview,
        Tab::Agents,
        Tab::Tasks,
        Tab::Approvals,
        Tab::Board,
        Tab::Conversations,
        Tab::Nodes,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Floor => "Floor",
            Tab::Overview => "Overview",
            Tab::Agents => "Agents",
            Tab::Tasks => "Tasks",
            Tab::Approvals => "Approvals",
            Tab::Board => "Board",
            Tab::Conversations => "Conversations",
            Tab::Nodes => "Nodes",
        }
    }

    /// The hub endpoint this tab reads (the floor reads the agents list too).
    pub fn path(self) -> &'static str {
        match self {
            Tab::Floor | Tab::Agents => "/v1/agents",
            Tab::Overview => "/v1/overview",
            Tab::Tasks => "/v1/tasks",
            Tab::Approvals => "/v1/approvals",
            Tab::Board => "/v1/board",
            Tab::Conversations => "/v1/conversations",
            Tab::Nodes => "/v1/nodes",
        }
    }

    fn index(self) -> usize {
        Tab::ALL.iter().position(|tab| *tab == self).unwrap_or(0)
    }

    pub fn next(self) -> Tab {
        Tab::ALL[(self.index() + 1) % Tab::ALL.len()]
    }

    pub fn prev(self) -> Tab {
        Tab::ALL[(self.index() + Tab::ALL.len() - 1) % Tab::ALL.len()]
    }
}

/// The colours: signal orange and paper for the selection and HELD, with 16-colour fallbacks for
/// terminals that do not advertise truecolour.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub accent: Color,
    pub paper: Color,
    pub ok: Color,
    pub muted: Color,
}

impl Palette {
    pub fn detect() -> Palette {
        let truecolor = std::env::var("COLORTERM")
            .map(|v| v.contains("truecolor") || v.contains("24bit"))
            .unwrap_or(false);
        if truecolor {
            Palette {
                accent: Color::Rgb(0xe8, 0x59, 0x0c),
                paper: Color::Rgb(0xf4, 0xf1, 0xea),
                ok: Color::Rgb(0x2f, 0x9e, 0x44),
                muted: Color::Rgb(0x86, 0x8e, 0x96),
            }
        } else {
            Palette { accent: Color::Red, paper: Color::White, ok: Color::Green, muted: Color::DarkGray }
        }
    }
}

/// A question waiting for `y` (or `n` / Esc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    StopAll,
    Resume,
    Approve(String),
    Deny(String),
}

impl Confirm {
    pub fn question(&self) -> String {
        match self {
            Confirm::StopAll => "Stop all agents now?".to_string(),
            Confirm::Resume => "Resume the plaza?".to_string(),
            Confirm::Approve(id) => format!("Approve {}?", short(id)),
            Confirm::Deny(id) => format!("Deny {}?", short(id)),
        }
    }
}

/// The newest live events, oldest first; the oldest are dropped past the capacity.
pub struct EventLog {
    lines: VecDeque<String>,
    cap: usize,
}

impl EventLog {
    pub fn new(cap: usize) -> EventLog {
        EventLog { lines: VecDeque::new(), cap }
    }

    pub fn push(&mut self, line: String) {
        if self.cap == 0 {
            return;
        }
        if self.lines.len() == self.cap {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn iter(&self) -> vec_deque::Iter<'_, String> {
        self.lines.iter()
    }
}

/// What the background threads send the UI.
enum Msg {
    Fetched(&'static str, Result<Value, String>),
    Link(bool),
    Event(String, Value),
    Done(String, Result<Value, String>),
}

pub struct App {
    pub client: Client,
    pub palette: Palette,
    pub tab: Tab,
    /// The last good answer of each endpoint, by path.
    pub data: HashMap<&'static str, Value>,
    /// The last fetch error (the hub is down or unreachable), shown in the footer.
    pub error: Option<String>,
    pub notice: Option<String>,
    /// Whether the events socket is up.
    pub link: bool,
    pub floor: FloorView,
    pub log: EventLog,
    pub selected: usize,
    /// Agent details open under the table.
    pub detail: bool,
    /// A message being typed to an agent: its cid and the text so far.
    pub draft: Option<(i64, String)>,
    pub confirm: Option<Confirm>,
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    watch: Sender<&'static str>,
}

impl App {
    pub fn new(client: Client) -> App {
        let (tx, rx) = mpsc::channel();
        let (watch, watched) = mpsc::channel();
        let poll_client = client.clone();
        let poll_tx = tx.clone();
        thread::spawn(move || poll(poll_client, watched, poll_tx));
        let target = client.target.clone();
        let event_tx = tx.clone();
        thread::spawn(move || follow_events(target, event_tx));
        App {
            client,
            palette: Palette::detect(),
            tab: Tab::Floor,
            data: HashMap::new(),
            error: None,
            notice: None,
            link: false,
            floor: FloorView::new(),
            log: EventLog::new(EVENT_LOG_LINES),
            selected: 0,
            detail: false,
            draft: None,
            confirm: None,
            rx,
            tx,
            watch,
        }
    }

    /// The agents list (`GET /v1/agents`).
    pub fn agents(&self) -> &[Value] {
        self.array("/v1/agents")
    }

    /// An endpoint's answer when it is an array.
    pub fn array(&self, path: &str) -> &[Value] {
        self.data.get(path).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
    }

    /// An endpoint's answer's array under `key` (`{"tasks": [...]}` and the like).
    pub fn wrapped(&self, path: &str, key: &str) -> &[Value] {
        self.data
            .get(path)
            .and_then(|v| v.get(key))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn paused(&self) -> bool {
        self.data.get("/v1/plaza/state").and_then(|v| v.get("paused")).and_then(Value::as_bool).unwrap_or(false)
    }

    /// The rows of the visible tab's list (0 for the tabs without one).
    fn rows(&self) -> usize {
        match self.tab {
            Tab::Floor | Tab::Overview => 0,
            Tab::Agents => self.agents().len(),
            Tab::Tasks => self.wrapped("/v1/tasks", "tasks").len(),
            Tab::Approvals => self.array("/v1/approvals").len(),
            Tab::Board => self.wrapped("/v1/board", "posts").len(),
            Tab::Conversations => self.wrapped("/v1/conversations", "conversations").len(),
            Tab::Nodes => self.array("/v1/nodes").len(),
        }
    }

    /// Takes what the threads sent since the last frame.
    pub fn drain(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Fetched(path, Ok(value)) => {
                    if path == "/v1/agents" {
                        self.floor.on_agents(&value);
                    }
                    self.data.insert(path, value);
                    self.error = None;
                }
                Msg::Fetched(path, Err(e)) => self.error = Some(format!("{path}: {e}")),
                Msg::Link(up) => self.link = up,
                Msg::Event(kind, data) => {
                    self.floor.on_event(&kind, &data);
                    self.log.push(event_line(&kind, &data));
                }
                Msg::Done(label, result) => {
                    self.notice = Some(match result {
                        Ok(_) => format!("{label}: done"),
                        Err(e) => format!("{label}: {e}"),
                    });
                    let _ = self.watch.send(self.tab.path());
                }
            }
        }
    }

    pub fn tick(&mut self, dt: f32) {
        self.floor.tick(dt);
    }

    pub fn draw(&self, frame: &mut Frame) {
        let [header, tabs, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        screens::header(frame, header, self);
        screens::tab_bar(frame, tabs, self);
        match self.tab {
            Tab::Floor => {
                let [floor, events] = Layout::vertical([Constraint::Min(0), Constraint::Length(7)]).areas(body);
                self.floor.render(frame, floor);
                screens::event_log(frame, events, self);
            }
            Tab::Overview => screens::overview(frame, body, self),
            Tab::Agents => screens::agents(frame, body, self),
            Tab::Tasks => screens::tasks(frame, body, self),
            Tab::Approvals => screens::approvals(frame, body, self),
            Tab::Board => screens::board(frame, body, self),
            Tab::Conversations => screens::conversations(frame, body, self),
            Tab::Nodes => screens::nodes(frame, body, self),
        }
        screens::footer(frame, footer, self);
    }

    /// Handles one key press; true when the UI should quit.
    pub fn on_key(&mut self, key: KeyEvent) -> bool {
        if key.kind != KeyEventKind::Press {
            return false;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if self.draft.is_none() {
                return true;
            }
            self.draft = None;
            return false;
        }
        self.notice = None;
        if self.draft.is_some() {
            self.on_draft_key(key.code);
            return false;
        }
        if let Some(confirm) = self.confirm.take() {
            match key.code {
                KeyCode::Char('y' | 'Y') => self.run(confirm),
                KeyCode::Char('n' | 'N') | KeyCode::Esc => {}
                _ => self.confirm = Some(confirm),
            }
            return false;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Char(c @ '1'..='8') => self.select_tab(Tab::ALL[usize::from(c as u8 - b'1')]),
            KeyCode::Tab => self.select_tab(self.tab.next()),
            KeyCode::BackTab => self.select_tab(self.tab.prev()),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Enter => {
                if self.tab == Tab::Agents && self.rows() > 0 {
                    self.detail = !self.detail;
                }
            }
            KeyCode::Char('t') => self.start_draft(),
            KeyCode::Char('a') => self.ask_decision(true),
            KeyCode::Char('d') => self.ask_decision(false),
            KeyCode::Char('s') => {
                self.confirm = Some(if self.paused() { Confirm::Resume } else { Confirm::StopAll });
            }
            KeyCode::Char('r') => {
                let _ = self.watch.send(self.tab.path());
                self.notice = Some("refreshing".to_string());
            }
            _ => {}
        }
        false
    }

    fn select_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.selected = 0;
        self.detail = false;
        let _ = self.watch.send(tab.path());
    }

    fn move_selection(&mut self, delta: isize) {
        let rows = self.rows();
        if rows == 0 {
            self.selected = 0;
            return;
        }
        let next = self.selected as isize + delta;
        self.selected = next.clamp(0, rows as isize - 1) as usize;
    }

    fn selected_agent_cid(&self) -> Option<i64> {
        self.agents().get(self.selected).and_then(|a| a.get("cid")).and_then(Value::as_i64)
    }

    fn start_draft(&mut self) {
        if self.tab != Tab::Agents {
            return;
        }
        if let Some(cid) = self.selected_agent_cid() {
            self.draft = Some((cid, String::new()));
        }
    }

    fn on_draft_key(&mut self, code: KeyCode) {
        let Some((cid, mut text)) = self.draft.take() else { return };
        match code {
            KeyCode::Esc => {}
            KeyCode::Enter => {
                let text = text.trim().to_string();
                if !text.is_empty() {
                    self.run_job(format!("message to agent {cid}"), move |c| {
                        c.post(&format!("/v1/agents/{cid}/chat"), &json!({ "text": text }))
                    });
                }
            }
            KeyCode::Backspace => {
                text.pop();
                self.draft = Some((cid, text));
            }
            KeyCode::Char(c) => {
                text.push(c);
                self.draft = Some((cid, text));
            }
            _ => self.draft = Some((cid, text)),
        }
    }

    fn ask_decision(&mut self, approve: bool) {
        if self.tab != Tab::Approvals {
            return;
        }
        let Some(item) = self.array("/v1/approvals").get(self.selected) else { return };
        let id = item.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        if item.get("state").and_then(Value::as_str) != Some("pending") {
            self.notice = Some("that approval is already decided".to_string());
            return;
        }
        self.confirm = Some(if approve { Confirm::Approve(id) } else { Confirm::Deny(id) });
    }

    fn run(&mut self, confirm: Confirm) {
        match confirm {
            Confirm::StopAll => self.run_job("stop all".to_string(), |c| {
                c.post("/v1/plaza/stop", &json!({ "reason": "stopped from the terminal" }))
            }),
            Confirm::Resume => self.run_job("resume".to_string(), |c| c.post("/v1/plaza/resume", &json!({}))),
            Confirm::Approve(id) => self.decide(id, true),
            Confirm::Deny(id) => self.decide(id, false),
        }
    }

    fn decide(&mut self, id: String, approve: bool) {
        let label = format!("{} {}", if approve { "approve" } else { "deny" }, short(&id));
        let path = format!("/v1/approvals/{}", seg(&id));
        self.run_job(label, move |c| c.post(&path, &json!({ "approve": approve })));
    }

    /// Runs one hub call on its own thread; its result comes back as a notice.
    fn run_job<F>(&mut self, label: String, job: F)
    where
        F: FnOnce(&Client) -> Result<Value, String> + Send + 'static,
    {
        let client = self.client.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = job(&client);
            let _ = tx.send(Msg::Done(label, result));
        });
    }
}

/// The poller: the agents, the plaza's state and the visible tab's endpoint, every 3 s, or at once
/// when the UI names another endpoint (or asks to refresh). Ends when the UI is gone.
fn poll(mut client: Client, watched: Receiver<&'static str>, tx: Sender<Msg>) {
    client.timeout = Duration::from_secs(5);
    let mut visible: &'static str = "/v1/agents";
    loop {
        let mut paths = vec!["/v1/hub/identity", "/v1/agents", "/v1/plaza/state"];
        if !paths.contains(&visible) {
            paths.push(visible);
        }
        for path in paths {
            if tx.send(Msg::Fetched(path, client.get(path))).is_err() {
                return;
            }
        }
        match watched.recv_timeout(POLL_EVERY) {
            Ok(path) => visible = path,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// The events socket: frames `{"event": kind, "data": ...}`; reconnects with a backoff up to 15 s.
fn follow_events(target: Target, tx: Sender<Msg>) {
    let mut wait = Duration::from_millis(500);
    loop {
        if let Ok(mut socket) =
            ws::connect(&target, "/v1/ws", Some("apz.events.v1"), Duration::from_secs(10), ws::Limits::default())
        {
            wait = Duration::from_millis(500);
            if tx.send(Msg::Link(true)).is_err() {
                return;
            }
            loop {
                match socket.recv(Duration::from_secs(1)) {
                    Ok(Some(ws::Message::Text(text))) => {
                        if let Some((kind, data)) = parse_frame(&text) {
                            if tx.send(Msg::Event(kind, data)).is_err() {
                                return;
                            }
                        }
                    }
                    Ok(Some(ws::Message::Binary(_))) | Ok(None) => {}
                    Err(_) => break,
                }
            }
        }
        if tx.send(Msg::Link(false)).is_err() {
            return;
        }
        thread::sleep(wait);
        wait = (wait * 2).min(Duration::from_secs(15));
    }
}

fn parse_frame(text: &str) -> Option<(String, Value)> {
    let frame: Value = serde_json::from_str(text).ok()?;
    let kind = frame.get("event")?.as_str()?.to_string();
    Some((kind, frame.get("data").cloned().unwrap_or(Value::Null)))
}

fn event_line(kind: &str, data: &Value) -> String {
    format!("{kind}  {}", clip(&data.to_string(), 120))
}

fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Restores the terminal (alternate screen off, cooked mode) on the way out, and on a panic.
struct Screen;

impl Screen {
    fn enter() -> Result<Screen, String> {
        terminal::enable_raw_mode().map_err(|e| e.to_string())?;
        if let Err(e) = execute!(io::stdout(), EnterAlternateScreen) {
            restore();
            return Err(e.to_string());
        }
        let before = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            before(info);
        }));
        Ok(Screen)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        restore();
    }
}

fn restore() {
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();
    let _ = io::stdout().flush();
}

/// The terminal UI until the user quits.
pub fn run(client: Client) -> Result<(), String> {
    let _screen = Screen::enter()?;
    let mut term = Terminal::new(CrosstermBackend::new(io::stdout())).map_err(|e| e.to_string())?;
    let mut app = App::new(client);
    let mut last = Instant::now();
    loop {
        app.drain();
        term.draw(|frame| app.draw(frame)).map_err(|e| e.to_string())?;
        if event::poll(FRAME).map_err(|e| e.to_string())? {
            if let Event::Key(key) = event::read().map_err(|e| e.to_string())? {
                if app.on_key(key) {
                    return Ok(());
                }
            }
        }
        let now = Instant::now();
        app.tick(now.duration_since(last).as_secs_f32());
        last = now;
    }
}

/// `APZ_TUI_SNAPSHOT=<file>`: runs the app off-screen for `secs` seconds against the hub (polls and
/// live events as usual), then writes every tab as plain text, `width`×`height`, into `path` —
/// the terminal UI's equivalent of the desktop app's `APZ_SNAPSHOT`.
pub fn snapshot(client: Client, path: &std::path::Path, width: u16, height: u16, secs: f32) -> Result<(), String> {
    let mut app = App::new(client);
    let started = Instant::now();
    let mut last = started;
    while started.elapsed().as_secs_f32() < secs {
        app.drain();
        thread::sleep(Duration::from_millis(50));
        let now = Instant::now();
        app.tick(now.duration_since(last).as_secs_f32());
        last = now;
    }
    let mut out = String::new();
    for (i, tab) in Tab::ALL.iter().enumerate() {
        let key = char::from_digit(i as u32 + 1, 10).unwrap_or('1');
        app.on_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
        // The tab's own data arrives with its first poll.
        let until = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < until {
            app.drain();
            thread::sleep(Duration::from_millis(50));
            app.tick(0.05);
        }
        let mut term = Terminal::new(ratatui::backend::TestBackend::new(width, height)).map_err(|e| e.to_string())?;
        term.draw(|frame| app.draw(frame)).map_err(|e| e.to_string())?;
        let buffer = term.backend().buffer();
        out.push_str(&format!("──── tab {} {:?} ────\n", i + 1, tab));
        for y in 0..buffer.area.height {
            let line: String = (0..buffer.area.width).map(|x| buffer[(x, y)].symbol().to_string()).collect();
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    std::fs::write(path, out).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_cycle_both_ways() {
        assert_eq!(Tab::ALL.len(), 8);
        assert_eq!(Tab::Floor.prev(), Tab::Nodes);
        assert_eq!(Tab::Nodes.next(), Tab::Floor);
        assert_eq!(Tab::Overview.next().next(), Tab::Tasks);
        assert_eq!(Tab::Floor.next().prev(), Tab::Floor);
    }

    #[test]
    fn event_log_keeps_the_newest_lines() {
        let mut log = EventLog::new(2);
        for line in ["a", "b", "c"] {
            log.push(line.to_string());
        }
        let kept: Vec<&str> = log.iter().map(String::as_str).collect();
        assert_eq!(kept, ["b", "c"]);
        assert_eq!(log.len(), 2);

        let mut none = EventLog::new(0);
        none.push("x".to_string());
        assert_eq!(none.len(), 0);
    }
}
