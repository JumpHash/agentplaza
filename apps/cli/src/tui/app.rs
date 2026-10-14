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
use super::onboarding::{self, DepsKey, Facts, Onboarding, Stage, Step, TOUR};
use super::screens;
use super::workspace::{self, Effect, Got, Want, Workspace};
use super::workspace_view;
use crate::cmd;
use crate::ctx::{Client, Config, seg};

// The Machines and Workstations tabs' modules sit beside this file (the tui module lists its own modules).
#[path = "machines.rs"]
mod machines;
#[path = "workstations.rs"]
mod workstations;

const FRAME: Duration = Duration::from_millis(50);
const POLL_EVERY: Duration = Duration::from_secs(3);
const EVENT_LOG_LINES: usize = 200;
/// How many recent recordings the Workstations tab lists for the selected workstation.
const RECENT_RECORDINGS: u32 = 5;

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
    Machines,
    Workstations,
    Workspace,
}

impl Tab {
    pub const ALL: [Tab; 11] = [
        Tab::Floor,
        Tab::Overview,
        Tab::Agents,
        Tab::Tasks,
        Tab::Approvals,
        Tab::Board,
        Tab::Conversations,
        Tab::Nodes,
        Tab::Machines,
        Tab::Workspace,
        Tab::Workstations,
    ];

    /// The tab a number key shows: 1 to 9, then 0 for the tenth.
    pub fn from_key(key: char) -> Option<Tab> {
        match key {
            '1'..='9' => Tab::ALL.get(usize::from(key as u8 - b'1')).copied(),
            '0' => Tab::ALL.get(9).copied(),
            'w' => Some(Tab::Workstations),
            _ => None,
        }
    }

    /// The number key that shows this tab (see `from_key`).
    pub fn key(self) -> char {
        match self.index() {
            i @ 0..=8 => char::from(b'1' + i as u8),
            9 => '0',
            _ => 'w',
        }
    }

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
            Tab::Machines => "Machines",
            Tab::Workstations => "Workstations",
            Tab::Workspace => "Workspace",
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
            Tab::Machines => "/v1/machines",
            Tab::Workstations => "/v1/stations",
            // The workspace reads the roster on every poll; its other lists come on the events.
            Tab::Workspace => "/v1/agents",
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
    /// Stop the running work of an agent (the workspace's Ctrl+C in a note).
    CancelWork(u64),
}

impl Confirm {
    pub fn question(&self) -> String {
        match self {
            Confirm::StopAll => "Stop all agents now?".to_string(),
            Confirm::Resume => "Resume the plaza?".to_string(),
            Confirm::CancelWork(cid) => format!("Stop agent {cid}'s work now?"),
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
    Facts(Facts),
    /// This machine's checks, read or read again after a fix.
    Deps(Vec<apz_setup::Check>),
    /// A line of a running fix's output.
    FixLine(String),
    /// The fixes ended: nothing more to run, or the one that stopped them.
    FixDone(Result<(), String>),
    Fetched(&'static str, Result<Value, String>),
    /// The selected workstation's sync state (`"sync"`) or its recent recordings (`"recordings"`).
    Station(String, &'static str, Result<Value, String>),
    Link(bool),
    Event(String, Value),
    Done(String, Result<Value, String>),
    /// An answer to a request of the workspace tab.
    Workspace(Got),
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
    /// The first-start setup or the tour, while it runs (it takes every key).
    pub onboarding: Option<Onboarding>,
    /// The sync state of each workstation the Workstations tab has read, by id.
    pub station_sync: HashMap<String, Value>,
    /// The recent recordings of each workstation the Workstations tab has read, by id.
    pub station_recent: HashMap<String, Value>,
    /// The Workspace tab's state (the agents by project, the chat, the board).
    pub ws: Workspace,
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    watch: Sender<&'static str>,
    /// The workstation the detail poller follows (None: none, while another tab shows).
    station_watch: Sender<Option<String>>,
    station_watched: Option<String>,
}

impl App {
    pub fn new(client: Client) -> App {
        let (tx, rx) = mpsc::channel();
        let (watch, watched) = mpsc::channel();
        let poll_client = client.clone();
        let poll_tx = tx.clone();
        thread::spawn(move || poll(poll_client, watched, poll_tx));
        let (station_watch, station_watched_rx) = mpsc::channel();
        let station_client = client.clone();
        let station_tx = tx.clone();
        thread::spawn(move || follow_station(station_client, station_watched_rx, station_tx));
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
            onboarding: None,
            station_sync: HashMap::new(),
            station_recent: HashMap::new(),
            ws: Workspace::default(),
            rx,
            tx,
            watch,
            station_watch,
            station_watched: None,
        }
    }

    /// Starts the setup or the tour.
    pub fn begin(&mut self, stage: Stage) {
        self.onboarding = Some(Onboarding::new(stage));
        match stage {
            Stage::Setup(_) => {
                self.gather_facts();
                self.gather_deps();
            }
            Stage::Tour(_) => self.show_tour_step(),
            Stage::Ask => {}
        }
    }

    /// Reads the hub and the model keys on a thread; the answer arrives as `Msg::Facts`.
    fn gather_facts(&self) {
        let client = self.client.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let _ = tx.send(Msg::Facts(onboarding::gather(&client)));
        });
    }

    /// Reads this machine's checks on a thread; the answer arrives as `Msg::Deps`.
    fn gather_deps(&self) {
        let tx = self.tx.clone();
        thread::spawn(move || {
            let _ = tx.send(Msg::Deps(apz_setup::check_all(apz_setup::Mode::Local)));
        });
    }

    /// Runs the confirmed fixes in order on a thread: their output arrives as `Msg::FixLine`, the
    /// checks again as `Msg::Deps`, and the end as `Msg::FixDone`. A fix that fails stops the rest.
    fn run_fixes(&self, needs: Vec<apz_setup::Need>) {
        let tx = self.tx.clone();
        thread::spawn(move || {
            let mut result = Ok(());
            for need in needs {
                let lines = tx.clone();
                let fixed = apz_setup::fix(need, &mut |line| {
                    let _ = lines.send(Msg::FixLine(line));
                });
                if let Err(e) = fixed {
                    result = Err(format!("{}: {e}", need.name()));
                    break;
                }
            }
            let _ = tx.send(Msg::Deps(apz_setup::check_all(apz_setup::Mode::Local)));
            let _ = tx.send(Msg::FixDone(result));
        });
    }

    /// Switches to the tab the tour's current step shows (a step without one keeps the current tab).
    fn show_tour_step(&mut self) {
        if let Some(Stage::Tour(i)) = self.onboarding.as_ref().map(|ob| ob.stage) {
            if let Some(tab) = TOUR[i].tab {
                self.select_tab(tab);
            }
        }
    }

    fn on_onboarding_key(&mut self, code: KeyCode) {
        let Some(input) = onboarding::input_for(code) else { return };
        let Some(ob) = self.onboarding.as_mut() else { return };
        match ob.deps_key(input) {
            DepsKey::Handled => return,
            DepsKey::Run(needs) => {
                self.run_fixes(needs);
                return;
            }
            DepsKey::Pass => {}
        }
        let Some(stage) = self.onboarding.as_ref().map(|ob| ob.stage) else { return };
        let Step { next, mark } = onboarding::advance(stage, input);
        match next {
            Some(to) => {
                if let Some(ob) = self.onboarding.as_mut() {
                    ob.stage = to;
                }
            }
            None => self.onboarding = None,
        }
        if let Some(mark) = mark {
            if let Err(e) = onboarding::persist(&Config::path(), mark) {
                self.notice = Some(format!("could not save the setup: {e}"));
            }
        }
        if let Some(to @ Stage::Tour(_)) = next {
            if to != stage {
                self.show_tour_step();
            }
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

    /// The classifier's context for the agent text on screen: the agents the app has polled, so `@Name` is a chip.
    pub fn rich_ctx(&self) -> apz_rich::Ctx {
        crate::rich::from_agents(self.array("/v1/agents"), None)
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
            Tab::Machines => self.array("/v1/machines").len(),
            Tab::Workstations => self.array("/v1/stations").len(),
            Tab::Workspace => 0,
        }
    }

    /// Takes what the threads sent since the last frame.
    pub fn drain(&mut self) {
        let messages: Vec<Msg> = self.rx.try_iter().collect();
        let mut roster_changed = false;
        for msg in messages {
            match msg {
                Msg::Fetched(path, Ok(value)) => {
                    if path == "/v1/agents" {
                        self.floor.on_agents(&value);
                        self.ws.set_agents(&value);
                        roster_changed = true;
                    }
                    self.data.insert(path, value);
                    self.error = None;
                }
                Msg::Fetched(path, Err(e)) => self.error = Some(format!("{path}: {e}")),
                Msg::Station(id, "sync", Ok(value)) => {
                    self.station_sync.insert(id, value);
                }
                Msg::Station(id, _, Ok(value)) => {
                    self.station_recent.insert(id, value);
                }
                Msg::Station(id, _, Err(e)) => self.error = Some(format!("/v1/stations/{id}: {e}")),
                Msg::Facts(facts) => {
                    if let Some(ob) = self.onboarding.as_mut() {
                        ob.facts = Some(facts);
                    }
                }
                Msg::Deps(checks) => {
                    if let Some(ob) = self.onboarding.as_mut() {
                        ob.deps.checks = Some(checks);
                    }
                }
                Msg::FixLine(line) => {
                    if let Some(ob) = self.onboarding.as_mut() {
                        ob.push_log(line);
                    }
                }
                Msg::FixDone(result) => {
                    if let Some(ob) = self.onboarding.as_mut() {
                        ob.deps.running = false;
                        let done = match result {
                            Ok(()) => "the fixes are done".to_string(),
                            Err(e) => format!("the fixes stopped: {e}"),
                        };
                        ob.push_log(done);
                    }
                }
                Msg::Link(up) => {
                    self.link = up;
                    if up && self.tab == Tab::Workspace {
                        // The events are back: what they carried while they were down is fetched again. Only on the
                        // tab: its look marks what it asked for, so the asks must be made.
                        let effects = self.ws.enter();
                        self.execute(effects);
                    }
                }
                Msg::Event(kind, data) => {
                    self.floor.on_event(&kind, &data);
                    if kind != "serial" {
                        self.log.push(event_line(&kind, &data));
                    }
                    // The Workstations tab reads the stations once more when one changes (not per owner input), and
                    // the selected one's sync and recordings with them.
                    if self.tab == Tab::Workstations && is_station_change(&kind) {
                        let _ = self.watch.send(Tab::Workstations.path());
                        let _ = self.station_watch.send(self.station_watched.clone());
                    }
                    let effects = self.ws.on_event(&kind, &data);
                    self.on_workspace(effects);
                }
                Msg::Workspace(got) => {
                    roster_changed = true;
                    let effects = self.ws.receive(got);
                    self.on_workspace(effects);
                }
                Msg::Done(label, result) => {
                    self.notice = Some(match result {
                        Ok(_) => format!("{label}: done"),
                        Err(e) => format!("{label}: {e}"),
                    });
                    let _ = self.watch.send(self.tab.path());
                    let effects = self.ws.refresh_board();
                    self.on_workspace(effects);
                }
            }
        }
        self.follow_selected_station();
        if roster_changed && self.tab == Tab::Workspace {
            let effects = self.ws.sync();
            self.execute(effects);
        }
    }

    /// Points the detail poller at the workstation the Workstations tab selects (and at none, off that tab).
    fn follow_selected_station(&mut self) {
        let wanted = match self.tab {
            Tab::Workstations => self.array("/v1/stations").get(self.selected).and_then(|s| s.get("id")).and_then(Value::as_str).map(str::to_string),
            _ => None,
        };
        if wanted != self.station_watched {
            let _ = self.station_watch.send(wanted.clone());
            self.station_watched = wanted;
        }
    }

    /// Runs the workspace's effects while its tab is on screen. Off it the state is kept, and the next look at the tab
    /// asks again (`Workspace::enter`).
    fn on_workspace(&mut self, effects: Vec<Effect>) {
        if self.tab == Tab::Workspace {
            self.execute(effects);
        }
    }

    pub fn tick(&mut self, dt: f32) {
        self.floor.tick(dt);
    }

    pub fn draw(&mut self, frame: &mut Frame) {
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
            Tab::Machines => machines::render(frame, body, self),
            Tab::Workstations => workstations::render(frame, body, self),
            Tab::Workspace => workspace_view::render(frame, body, &mut self.ws, self.palette),
        }
        if let Some(ob) = &self.onboarding {
            ob.draw(frame, body, self.palette, self.client.base());
        }
        screens::footer(frame, footer, self);
    }

    /// Handles one key press; true when the UI should quit.
    pub fn on_key(&mut self, key: KeyEvent) -> bool {
        if key.kind != KeyEventKind::Press {
            return false;
        }
        let typing = self.tab == Tab::Workspace && self.ws.captures_keys();
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') && !typing {
            if self.draft.is_none() {
                return true;
            }
            self.draft = None;
            return false;
        }
        self.notice = None;
        if self.onboarding.is_some() {
            self.on_onboarding_key(key.code);
            return false;
        }
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
        if self.tab == Tab::Workspace
            && let Some(ws_key) = workspace_key(key)
            && let Some(effects) = self.ws.key(ws_key)
        {
            self.execute(effects);
            return false;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Char(c @ '0'..='9') => {
                if let Some(tab) = Tab::from_key(c) {
                    self.select_tab(tab);
                }
            }
            KeyCode::Char('w') => self.select_tab(Tab::Workstations),
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
            KeyCode::Char('?') => self.begin(Stage::Tour(0)),
            KeyCode::Char('a') => self.ask_decision(true),
            KeyCode::Char('d') => self.ask_decision(false),
            KeyCode::Char('s') => {
                self.confirm = Some(if self.paused() { Confirm::Resume } else { Confirm::StopAll });
            }
            KeyCode::Char('r') => {
                let _ = self.watch.send(self.tab.path());
                if self.tab == Tab::Workspace {
                    let effects = self.ws.enter();
                    self.execute(effects);
                }
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
        if tab == Tab::Workspace {
            let effects = self.ws.enter();
            self.execute(effects);
        }
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
            Confirm::CancelWork(cid) => self.run_job(format!("stop agent {cid}'s work"), move |c| {
                c.post(&format!("/v1/agents/{cid}/cancel"), &json!({}))
            }),
        }
    }

    /// Runs the workspace's effects: fetches on threads (their answers come back as `Msg::Workspace`), and the writes
    /// as jobs (their results come back as notices).
    fn execute(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Fetch(want) => self.fetch(want),
                Effect::Agents => self.fetch_agents(),
                Effect::Steer { cid, text } => self.run_job(format!("message to agent {cid}"), move |c| {
                    c.post(&format!("/v1/agents/{cid}/chat"), &json!({ "text": text }))
                }),
                Effect::Post { project, body } => {
                    let mentions = cmd::board::mentions_in(&body);
                    self.run_job("post to the board".to_string(), move |c| {
                        c.post("/v1/board/posts", &json!({ "body": body, "project": project, "mentions": mentions }))
                    });
                }
                Effect::Pause { project, on } => {
                    let label = format!("{} the board", if on { "pause" } else { "resume" });
                    self.run_job(label, move |c| c.post("/v1/board/pause", &json!({ "on": on, "project": project })));
                }
                Effect::Export { project } => self.export(project),
                Effect::AskStop(cid) => self.confirm = Some(Confirm::CancelWork(cid)),
                Effect::Notice(text) => self.notice = Some(text),
            }
        }
    }

    /// One answer-fetch of the workspace, on its own thread.
    fn fetch(&self, want: Want) {
        let client = self.client.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let got = match want {
                Want::Projects => Got::Projects(client.get("/v1/projects")),
                Want::Tasks => Got::Tasks(client.get("/v1/tasks")),
                Want::Board(project) => {
                    let path = match &project {
                        Some(id) => format!("/v1/board?project={}", seg(id)),
                        None => "/v1/board".to_string(),
                    };
                    Got::Board { project, result: client.get(&path) }
                }
                Want::Transcript { cid, after } => {
                    let path = match after {
                        Some(seq) => format!("/v1/agents/{cid}/transcript?after={seq}&limit=1000"),
                        None => format!("/v1/agents/{cid}/transcript?last={}", workspace::FIRST_LOAD),
                    };
                    Got::Transcript { cid, result: client.get(&path) }
                }
            };
            let _ = tx.send(Msg::Workspace(got));
        });
    }

    /// The agents roster again, as the poll reads it.
    fn fetch_agents(&self) {
        let client = self.client.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let _ = tx.send(Msg::Fetched("/v1/agents", client.get("/v1/agents")));
        });
    }

    /// The board (all of it, or the project's) as a Markdown file in the working directory: `E` in the workspace.
    fn export(&mut self, project: Option<String>) {
        let scope: String = match project.as_deref() {
            Some(id) => id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect(),
            None => "all".to_string(),
        };
        let name = format!("agentplaza-board-{scope}.md");
        let path = std::env::current_dir().unwrap_or_default().join(&name);
        self.run_job(format!("export to {name}"), move |c| {
            let mut route = "/v1/board/export?format=md".to_string();
            if let Some(id) = &project {
                route.push_str(&format!("&project={}", seg(id)));
            }
            let reply = apz_http::client::request(&c.target, "GET", &route, None, c.timeout)
                .map_err(|e| format!("{route}: {e}"))?;
            if !(200..300).contains(&reply.status) {
                return Err(format!("{route}: HTTP {}", reply.status));
            }
            cmd::skills::write_out(&path, &reply.body, false)?;
            Ok(json!({ "file": path.display().to_string(), "bytes": reply.body.len() }))
        });
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

/// A terminal key as the workspace reads it; None for the keys it does not use (Ctrl+letters other than Ctrl+C).
fn workspace_key(key: KeyEvent) -> Option<workspace::Key> {
    use workspace::Key as W;
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    Some(match key.code {
        KeyCode::Char('c') if ctrl => W::CtrlC,
        KeyCode::Char(_) if ctrl => return None,
        KeyCode::Char(c) => W::Char(c),
        KeyCode::Up => W::Up,
        KeyCode::Down => W::Down,
        KeyCode::PageUp => W::PageUp,
        KeyCode::PageDown => W::PageDown,
        KeyCode::End => W::End,
        KeyCode::Enter => W::Enter,
        KeyCode::Tab => W::Tab,
        KeyCode::BackTab => W::BackTab,
        KeyCode::Esc => W::Esc,
        KeyCode::Backspace => W::Backspace,
        _ => return None,
    })
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

/// The workstation detail's poller: the followed workstation's sync state and its recent recordings, every 3 s, or at
/// once when the UI names a workstation (the same one again asks for a refresh). None stops it until one is named.
fn follow_station(mut client: Client, watched: Receiver<Option<String>>, tx: Sender<Msg>) {
    client.timeout = Duration::from_secs(5);
    let mut current: Option<String> = None;
    loop {
        if let Some(id) = current.clone() {
            let sync = client.get(&format!("/v1/stations/{}/sync", seg(&id)));
            let recent = client.get(&format!("/v1/stations/recordings?station={}&limit={RECENT_RECORDINGS}", seg(&id)));
            if tx.send(Msg::Station(id.clone(), "sync", sync)).is_err() {
                return;
            }
            if tx.send(Msg::Station(id, "recordings", recent)).is_err() {
                return;
            }
        }
        match watched.recv_timeout(POLL_EVERY) {
            Ok(next) => current = next,
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

/// The hub events that change what the Workstations tab shows: a station's own change, a lease opened, queued,
/// granted, released, ended or failed, the owner's takeover or give back, and a sync's conflict or pause. The owner's
/// input during a takeover is left out (it is frequent).
fn is_station_change(kind: &str) -> bool {
    matches!(
        kind,
        "station"
            | "station_takeover"
            | "station_give_back"
            | "station_sync_conflict"
            | "station_sync_paused"
            | "lease_waiting"
            | "lease_granted"
            | "lease_released"
            | "lease_ended"
            | "lease_failed"
    )
}

fn parse_frame(text: &str) -> Option<(String, Value)> {
    let frame: Value = serde_json::from_str(text).ok()?;
    let kind = frame.get("event")?.as_str()?.to_string();
    Some((kind, frame.get("data").cloned().unwrap_or(Value::Null)))
}

/// One log line: the kind, then the data's top-level scalar fields as `key=value` (anything else as
/// its one-line text), with line breaks collapsed to spaces.
fn event_line(kind: &str, data: &Value) -> String {
    let body = match data.as_object() {
        Some(map) => map
            .iter()
            .filter(|(_, v)| !v.is_object() && !v.is_array())
            .map(|(k, v)| format!("{k}={}", screens::show(Some(v))))
            .collect::<Vec<_>>()
            .join(" "),
        None => screens::show(Some(data)),
    };
    format!("{kind}  {body}").replace(['\r', '\n'], " ")
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
pub fn run(client: Client, start: Option<Stage>) -> Result<(), String> {
    let _screen = Screen::enter()?;
    let mut term = Terminal::new(CrosstermBackend::new(io::stdout())).map_err(|e| e.to_string())?;
    let mut app = App::new(client);
    if let Some(stage) = start {
        app.begin(stage);
    }
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
        app.on_key(KeyEvent::new(KeyCode::Char(tab.key()), KeyModifiers::NONE));
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
        assert_eq!(Tab::ALL.len(), 11);
        assert_eq!(Tab::Floor.prev(), Tab::Workstations);
        assert_eq!(Tab::Workspace.next(), Tab::Workstations);
        assert_eq!(Tab::Workstations.next(), Tab::Floor);
        assert_eq!(Tab::Machines.next(), Tab::Workspace);
        assert_eq!(Tab::Nodes.next(), Tab::Machines);
        assert_eq!(Tab::Workstations.path(), "/v1/stations");
        assert_eq!(Tab::Overview.next().next(), Tab::Tasks);
        assert_eq!(Tab::Floor.next().prev(), Tab::Floor);
    }

    #[test]
    fn the_takeover_and_sync_events_refresh_the_workstations_tab() {
        for kind in ["station_takeover", "station_give_back", "station_sync_conflict", "station_sync_paused", "lease_granted"] {
            assert!(is_station_change(kind), "{kind} changes the tab");
        }
        assert!(!is_station_change("station_sync"), "a pass that moved files is read on the next poll");
        assert!(!is_station_change("owner_input"));
    }

    #[test]
    fn number_keys_show_the_tabs_and_zero_shows_the_tenth() {
        assert_eq!(Tab::from_key('1'), Some(Tab::Floor));
        assert_eq!(Tab::from_key('9'), Some(Tab::Machines));
        assert_eq!(Tab::from_key('0'), Some(Tab::Workspace));
        assert_eq!(Tab::from_key('x'), None);
        assert_eq!(Tab::from_key('w'), Some(Tab::Workstations));
        for tab in Tab::ALL {
            assert_eq!(Tab::from_key(tab.key()), Some(tab));
        }
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
