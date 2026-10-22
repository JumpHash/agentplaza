//! The terminal UI's state, loop and threads. The poller fetches the hub every 3 s (the agents and
//! the plaza's state, plus the visible tab's endpoint); the event thread holds the `/v1/ws` socket.
//! Both send to the UI thread, which owns all the state and draws about 20 times a second. The
//! projects (for the switcher) are read at the start, on a `project` event and on a refresh.

use std::collections::HashMap;
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

use super::cmdk;
use super::context_bar;
use super::onboarding::{self, DepsKey, Facts, Onboarding, Stage, Step, TOUR};
use super::scope;
use super::screens;
use super::style;
use super::workspace::{self, Effect, Got, Want, Workspace};
use super::workspace_view;
use crate::cmd;
use crate::cmd::stations::control;
use crate::ctx::{Client, Config, seg};

// The Machines, Workstations and Chat tabs' modules sit beside this file (the tui module lists its own modules).
#[path = "chat.rs"]
mod chat;
#[path = "machines.rs"]
mod machines;
#[path = "providers.rs"]
mod providers;
#[path = "workstations.rs"]
mod workstations;

use providers::Action as ProvidersAction;

const FRAME: Duration = Duration::from_millis(50);
const POLL_EVERY: Duration = Duration::from_secs(3);
/// How many recent recordings the Workstations tab lists for the selected workstation.
const RECENT_RECORDINGS: u32 = 5;
/// How long Start may take: it opens the project, posts its tasks and asks the nodes for the new agents.
const START_TIMEOUT: Duration = Duration::from_secs(120);

/// The tabs, in the order of the tab bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Agents,
    Tasks,
    Approvals,
    Board,
    Nodes,
    Machines,
    Workspace,
    Workstations,
    Chat,
    Providers,
}

impl Tab {
    pub const ALL: [Tab; 11] = [
        Tab::Overview,
        Tab::Agents,
        Tab::Tasks,
        Tab::Approvals,
        Tab::Board,
        Tab::Nodes,
        Tab::Machines,
        Tab::Workspace,
        Tab::Workstations,
        Tab::Chat,
        Tab::Providers,
    ];

    /// The tab a key shows: 1 to 8 for the first eight tabs, then `w` for the Workstations, `c` for the Chat and `m`
    /// for the Providers (the models).
    pub fn from_key(key: char) -> Option<Tab> {
        match key {
            '1'..='8' => Tab::ALL.get(usize::from(key as u8 - b'1')).copied(),
            'w' => Some(Tab::Workstations),
            'c' => Some(Tab::Chat),
            'm' => Some(Tab::Providers),
            _ => None,
        }
    }

    /// The key that shows this tab (see `from_key`).
    pub fn key(self) -> char {
        match self.index() {
            i @ 0..=7 => char::from(b'1' + i as u8),
            8 => 'w',
            9 => 'c',
            _ => 'm',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Agents => "Agents",
            Tab::Tasks => "Tasks",
            Tab::Approvals => "Approvals",
            Tab::Board => "Board",
            Tab::Nodes => "Nodes",
            Tab::Machines => "Machines",
            Tab::Workstations => "Workstations",
            Tab::Workspace => "Workspace",
            Tab::Chat => "Chat",
            Tab::Providers => "Providers",
        }
    }

    /// The hub endpoint this tab reads.
    pub fn path(self) -> &'static str {
        match self {
            Tab::Agents => "/v1/agents",
            Tab::Overview => "/v1/overview",
            Tab::Tasks => "/v1/tasks",
            Tab::Approvals => "/v1/approvals",
            Tab::Board => "/v1/board",
            Tab::Nodes => "/v1/nodes",
            Tab::Machines => "/v1/machines",
            Tab::Workstations => "/v1/stations",
            // The workspace reads the roster on every poll; its other lists come on the events.
            Tab::Workspace => "/v1/agents",
            Tab::Chat => chat::PATH,
            Tab::Providers => providers::PATH,
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

/// The colours of the Loom theme (dark): signal orange for the selection, the badges and the cursor,
/// with 16-colour fallbacks for terminals that do not advertise truecolour, and no colour at all under
/// `NO_COLOR` (emphasis is then bold or reversed; see `style::chip`). The four fields are what most
/// screens use directly; the rest of the theme comes from the methods below, which follow the mode
/// the accent says (truecolour, 16 colours, or none).
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// The signal orange: the selection, the HELD badge, the brand.
    pub accent: Color,
    /// Light text on the default background (see `on_accent` for the dark text on the accent).
    pub paper: Color,
    pub ok: Color,
    /// Ink 3: secondary text and hints.
    pub muted: Color,
}

/// How a palette draws: from its accent, which `detect` sets per mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Truecolor,
    Ansi,
    Mono,
}

impl Palette {
    pub fn detect() -> Palette {
        if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
            return Palette {
                accent: Color::Reset,
                paper: Color::Reset,
                ok: Color::Reset,
                muted: Color::Reset,
            };
        }
        let truecolor = std::env::var("COLORTERM")
            .map(|v| v.contains("truecolor") || v.contains("24bit"))
            .unwrap_or(false);
        if truecolor {
            Palette {
                accent: Color::Rgb(0xe8, 0x79, 0x4a),
                paper: Color::Rgb(0xf4, 0xf1, 0xea),
                ok: Color::Rgb(0x5c, 0xcf, 0x8c),
                muted: Color::Rgb(0x95, 0x8f, 0x83),
            }
        } else {
            Palette {
                accent: Color::Red,
                paper: Color::White,
                ok: Color::Green,
                muted: Color::DarkGray,
            }
        }
    }

    fn mode(&self) -> Mode {
        match self.accent {
            Color::Rgb(..) => Mode::Truecolor,
            Color::Reset => Mode::Mono,
            _ => Mode::Ansi,
        }
    }

    /// One token: its truecolour value, its 16-colour fallback, or the terminal's own colour.
    fn token(&self, rgb: Color, ansi: Color) -> Color {
        match self.mode() {
            Mode::Truecolor => rgb,
            Mode::Ansi => ansi,
            Mode::Mono => Color::Reset,
        }
    }

    // The theme's tokens. The ones the body screens do not use yet are allowed unused until they do.
    pub fn ink(&self) -> Color {
        self.token(Color::Rgb(0xec, 0xe8, 0xdf), Color::White)
    }

    pub fn ink2(&self) -> Color {
        self.token(Color::Rgb(0xaa, 0xa4, 0x98), Color::Gray)
    }

    #[allow(dead_code)]
    pub fn rule(&self) -> Color {
        self.token(Color::Rgb(0x2a, 0x28, 0x24), Color::DarkGray)
    }

    pub fn rule_strong(&self) -> Color {
        self.token(Color::Rgb(0x3a, 0x37, 0x31), Color::DarkGray)
    }

    /// The header and status rows.
    pub fn raised(&self) -> Color {
        self.token(Color::Rgb(0x1c, 0x1b, 0x18), Color::Reset)
    }

    pub fn sheet(&self) -> Color {
        self.token(Color::Rgb(0x21, 0x1f, 0x1c), Color::Reset)
    }

    pub fn accent_soft(&self) -> Color {
        self.token(Color::Rgb(0x3a, 0x24, 0x18), Color::Reset)
    }

    /// Dark ink on the accent: the selected tab, the HELD badge, the mode chips.
    pub fn on_accent(&self) -> Color {
        self.token(Color::Rgb(0x1a, 0x0e, 0x07), Color::Black)
    }

    #[allow(dead_code)]
    pub fn warn(&self) -> Color {
        self.token(Color::Rgb(0xe8, 0xb2, 0x57), Color::Yellow)
    }

    pub fn crit(&self) -> Color {
        self.token(Color::Rgb(0xf0, 0x7a, 0x6f), Color::LightRed)
    }

    #[allow(dead_code)]
    pub fn info(&self) -> Color {
        self.token(Color::Rgb(0x86, 0xa8, 0xf0), Color::Blue)
    }

    #[allow(dead_code)]
    pub fn review(&self) -> Color {
        self.token(Color::Rgb(0xb3, 0x9a, 0xf0), Color::Magenta)
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
    /// Start the project a conversation drafted, at the draft revision on screen (the Chat tab's `S`).
    Start {
        conversation: String,
        revision: u64,
        title: String,
    },
}

impl Confirm {
    pub fn question(&self) -> String {
        match self {
            Confirm::StopAll => "Stop all agents now?".to_string(),
            Confirm::Resume => "Resume the plaza?".to_string(),
            Confirm::CancelWork(cid) => format!("Stop agent {cid}'s work now?"),
            Confirm::Approve(id) => format!("Approve {}?", short(id)),
            Confirm::Deny(id) => format!("Deny {}?", short(id)),
            Confirm::Start {
                conversation,
                revision,
                title,
            } => format!("Start project \"{title}\" from {conversation}'s draft r{revision}?"),
        }
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
    /// What the takeover panel (by its number) reports: a line, a refused command, or that it is over.
    Desk(u64, control::Report),
    Link(bool),
    Event(String, Value),
    Done(String, Result<Value, String>),
    /// An answer to a request of the workspace tab.
    Workspace(Got),
    /// A conversation the Chat tab read, by its id.
    Conversation(String, Result<Value, String>),
    /// The engines' update dry run the Providers tab asked for.
    Plan(Result<Value, String>),
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
    /// The animation clock: seconds since the UI started. `tick` advances it, and everything animated
    /// reads it (the recorder drives it with fixed steps).
    pub clock: f32,
    /// When the notice on screen appeared, on the clock: its toast hides a few seconds after.
    pub notice_at: f32,
    /// The notice the clock last saw, so that a new notice is told from the same text again.
    notice_seen: Option<String>,
    /// Whether the events socket is up.
    pub link: bool,
    pub selected: usize,
    /// Agent details open under the table.
    pub detail: bool,
    /// A message being typed to an agent: its cid and the text so far.
    pub draft: Option<(i64, String)>,
    pub confirm: Option<Confirm>,
    /// The first-start setup or the tour, while it runs (it takes every key).
    pub onboarding: Option<Onboarding>,
    /// The command palette, while it is open (Ctrl+K): it takes the keys.
    pub cmdk: Option<cmdk::CommandPalette>,
    /// The sync state of each workstation the Workstations tab has read, by id.
    pub station_sync: HashMap<String, Value>,
    /// The recent recordings of each workstation the Workstations tab has read, by id.
    pub station_recent: HashMap<String, Value>,
    /// The takeover panel of the Workstations tab, while one is open (`T` opens it, Esc gives the session back).
    pub desk: Option<workstations::Panel>,
    /// The number the next takeover panel gets: a report of an earlier panel is not shown in a later one.
    desk_seq: u64,
    /// The Workspace tab's state (the agents by project, the chat, the board).
    pub ws: Workspace,
    /// The project the Agents and Tasks tabs show: None for all of them (`p` and `P` switch it).
    pub scope: Option<String>,
    /// The Chat tab's state: the conversations' views and the answers being written.
    pub chat: chat::ChatTab,
    /// A message to the orchestrator being written in the footer (the Chat tab's `t` and `n`).
    pub compose: Option<chat::Compose>,
    /// The Providers tab's state: the selection, the search, the update plan and the add form.
    pub providers: providers::Screen,
    /// A read of the projects is under way, and whether another was asked for meanwhile.
    projects_reading: bool,
    projects_again: bool,
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
        let mut app = App {
            client,
            palette: Palette::detect(),
            tab: Tab::Overview,
            data: HashMap::new(),
            error: None,
            notice: None,
            clock: 0.0,
            notice_at: 0.0,
            notice_seen: None,
            link: false,
            selected: 0,
            detail: false,
            draft: None,
            confirm: None,
            onboarding: None,
            cmdk: None,
            station_sync: HashMap::new(),
            station_recent: HashMap::new(),
            desk: None,
            desk_seq: 0,
            ws: Workspace::default(),
            scope: None,
            chat: chat::ChatTab::default(),
            compose: None,
            providers: providers::Screen::default(),
            projects_reading: false,
            projects_again: false,
            rx,
            tx,
            watch,
            station_watch,
            station_watched: None,
        };
        app.fetch_projects();
        app
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
        if let Some(Stage::Tour(i)) = self.onboarding.as_ref().map(|ob| ob.stage)
            && let Some(tab) = TOUR[i].tab
        {
            self.select_tab(tab);
        }
    }

    fn on_onboarding_key(&mut self, code: KeyCode) {
        let Some(input) = onboarding::input_for(code) else {
            return;
        };
        let Some(ob) = self.onboarding.as_mut() else {
            return;
        };
        match ob.deps_key(input) {
            DepsKey::Handled => return,
            DepsKey::Run(needs) => {
                self.run_fixes(needs);
                return;
            }
            DepsKey::Pass => {}
        }
        let Some(stage) = self.onboarding.as_ref().map(|ob| ob.stage) else {
            return;
        };
        let Step { next, mark } = onboarding::advance(stage, input);
        match next {
            Some(to) => {
                if let Some(ob) = self.onboarding.as_mut() {
                    ob.stage = to;
                }
            }
            None => self.onboarding = None,
        }
        if let Some(mark) = mark
            && let Err(e) = onboarding::persist(&Config::path(), mark)
        {
            self.notice = Some(format!("could not save the setup: {e}"));
        }
        if let Some(to @ Stage::Tour(_)) = next
            && to != stage
        {
            self.show_tour_step();
        }
    }

    /// The agents list (`GET /v1/agents`).
    pub fn agents(&self) -> &[Value] {
        self.array("/v1/agents")
    }

    /// The projects (`GET /v1/projects`): what the switcher offers.
    pub fn projects(&self) -> &[Value] {
        self.array("/v1/projects")
    }

    /// The agents the Agents tab lists: every one, or the selected project's.
    pub fn shown_agents(&self) -> Vec<&Value> {
        let scope = self.scope.as_deref();
        self.agents()
            .iter()
            .filter(|a| scope::agent_in(a, scope))
            .collect()
    }

    /// The tasks the Tasks tab lists: every one, or the selected project's.
    pub fn shown_tasks(&self) -> Vec<&Value> {
        let scope = self.scope.as_deref();
        let project = scope::project_of(scope, self.projects());
        self.wrapped("/v1/tasks", "tasks")
            .iter()
            .filter(|t| scope::task_in(t, scope, project))
            .collect()
    }

    /// An endpoint's answer when it is an array.
    pub fn array(&self, path: &str) -> &[Value] {
        self.data
            .get(path)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
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
        self.data
            .get("/v1/plaza/state")
            .and_then(|v| v.get("paused"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// The rows of the visible tab's list (0 for the tabs without one).
    fn rows(&self) -> usize {
        match self.tab {
            Tab::Overview => 0,
            Tab::Agents => self.shown_agents().len(),
            Tab::Tasks => self.shown_tasks().len(),
            Tab::Approvals => self.array("/v1/approvals").len(),
            Tab::Board => self.wrapped("/v1/board", "posts").len(),
            Tab::Nodes => self.array("/v1/nodes").len(),
            Tab::Machines => self.array("/v1/machines").len(),
            Tab::Workstations => self.array("/v1/stations").len(),
            Tab::Workspace => 0,
            Tab::Chat => self.array(chat::PATH).len(),
            Tab::Providers => 0,
        }
    }

    /// Takes what the threads sent since the last frame.
    pub fn drain(&mut self) {
        let messages: Vec<Msg> = self.rx.try_iter().collect();
        let mut roster_changed = false;
        for msg in messages {
            match msg {
                Msg::Fetched(path, result) if path == "/v1/projects" => {
                    self.projects_reading = false;
                    match result {
                        Ok(value) => {
                            self.data.insert(path, value);
                            self.error = None;
                        }
                        Err(e) => self.error = Some(format!("{path}: {e}")),
                    }
                    if std::mem::take(&mut self.projects_again) {
                        self.fetch_projects();
                    }
                }
                Msg::Fetched(path, Ok(value)) => {
                    if path == "/v1/agents" {
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
                Msg::Desk(number, report) => self.on_desk_report(number, report),
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
                    if up {
                        self.fetch_projects();
                    }
                    if up && self.tab == Tab::Workspace {
                        // The events are back: what they carried while they were down is fetched again. Only on the
                        // tab: its look marks what it asked for, so the asks must be made.
                        let effects = self.ws.enter();
                        self.execute(effects);
                    }
                }
                Msg::Event(kind, data) => {
                    match kind.as_str() {
                        "project" => self.fetch_projects(),
                        "delta" => self.chat.on_delta(&data),
                        "owner_conversation" => {
                            self.chat.on_view(data.clone());
                            // The list's summary changed with it.
                            if self.tab == Tab::Chat {
                                let _ = self.watch.send(chat::PATH);
                            }
                        }
                        _ => {}
                    }
                    // The Workstations tab reads the stations once more when one changes (not per owner input), and
                    // the selected one's sync and recordings with them.
                    if self.tab == Tab::Workstations && is_station_change(&kind) {
                        let _ = self.watch.send(Tab::Workstations.path());
                        let _ = self.station_watch.send(self.station_watched.clone());
                    }
                    // The Providers tab reads its other endpoints again when a provider or an engine changes.
                    if self.tab == Tab::Providers && providers::is_change(&kind) {
                        self.fetch_providers();
                    }
                    let effects = self.ws.on_event(&kind, &data);
                    self.on_workspace(effects);
                }
                Msg::Workspace(got) => {
                    roster_changed = true;
                    let effects = self.ws.receive(got);
                    self.on_workspace(effects);
                }
                Msg::Conversation(_, Ok(view)) => self.chat.on_view(view),
                Msg::Conversation(id, Err(e)) => self.notice = Some(format!("{id}: {e}")),
                Msg::Plan(result) => {
                    if let Some(note) = self.providers.on_plan(result) {
                        self.notice = Some(note);
                    }
                }
                Msg::Done(label, result) => {
                    if self.tab == Tab::Providers {
                        self.providers
                            .on_done(&label, result.as_ref().map(|_| ()).map_err(Clone::clone));
                    }
                    self.notice = Some(match result {
                        Ok(_) => format!("{label}: done"),
                        Err(e) => format!("{label}: {e}"),
                    });
                    if self.tab == Tab::Providers {
                        self.fetch_providers();
                    }
                    let _ = self.watch.send(self.tab.path());
                    let effects = self.ws.refresh_board();
                    self.on_workspace(effects);
                }
            }
        }
        self.follow_selected_station();
        self.follow_conversation();
        if roster_changed && self.tab == Tab::Workspace {
            let effects = self.ws.sync();
            self.execute(effects);
        }
    }

    /// Points the detail poller at the workstation the Workstations tab selects (and at none, off that tab).
    fn follow_selected_station(&mut self) {
        let wanted = match self.tab {
            Tab::Workstations => self
                .array("/v1/stations")
                .get(self.selected)
                .and_then(|s| s.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string),
            _ => None,
        };
        if wanted != self.station_watched {
            let _ = self.station_watch.send(wanted.clone());
            self.station_watched = wanted;
        }
    }

    /// Reads the conversation the Chat tab selects when its view is missing or older than the list says.
    fn follow_conversation(&mut self) {
        if self.tab != Tab::Chat {
            return;
        }
        let Some(id) = self
            .array(chat::PATH)
            .get(self.selected)
            .and_then(|summary| self.chat.wants(summary))
        else {
            return;
        };
        self.chat.reading(&id);
        let client = self.client.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = client.get(&format!("{}/{}", chat::PATH, seg(&id)));
            let _ = tx.send(Msg::Conversation(id, result));
        });
    }

    /// Reads the projects on a thread (the answer arrives as `Msg::Fetched`). One read at a time: a read asked for
    /// while one is under way follows it.
    fn fetch_projects(&mut self) {
        if self.projects_reading {
            self.projects_again = true;
            return;
        }
        self.projects_reading = true;
        let client = self.client.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let _ = tx.send(Msg::Fetched("/v1/projects", client.get("/v1/projects")));
        });
    }

    /// Steps the project the Agents and Tasks tabs show through All projects and each open project.
    fn step_scope(&mut self, delta: isize) {
        let choices = scope::choices(self.projects());
        self.scope = scope::step(self.scope.as_deref(), &choices, delta);
        if matches!(self.tab, Tab::Agents | Tab::Tasks) {
            self.selected = 0;
            self.detail = false;
        }
        self.notice = Some(format!(
            "showing {}",
            scope::label(self.scope.as_deref(), self.projects())
        ));
    }

    /// Runs the workspace's effects while its tab is on screen. Off it the state is kept, and the next look at the tab
    /// asks again (`Workspace::enter`).
    fn on_workspace(&mut self, effects: Vec<Effect>) {
        if self.tab == Tab::Workspace {
            self.execute(effects);
        } else {
            // The context bar of the Chat tab's conversation is kept current by the same events.
            let context: Vec<Effect> = effects
                .into_iter()
                .filter(|e| matches!(e, Effect::Context(_)))
                .collect();
            self.execute(context);
        }
    }

    pub fn tick(&mut self, dt: f32) {
        self.clock += dt;
        let effects = self.ws.poll(dt, self.shown_owner());
        self.execute(effects);
        if self.notice != self.notice_seen {
            if self.notice.is_some() {
                self.notice_at = self.clock;
            }
            self.notice_seen = self.notice.clone();
        }
    }

    /// The chat on screen whose context the context bar shows: the Workspace's selected agent, or the Chat tab's
    /// conversation.
    fn shown_owner(&self) -> Option<context_bar::Owner> {
        match self.tab {
            Tab::Workspace => self.ws.selected_cid().map(context_bar::Owner::Agent),
            Tab::Chat => chat::selected(self).map(|(owner, _)| owner),
            _ => None,
        }
    }

    /// Runs a context or model request on its own thread; the answer comes back as `Msg::Workspace`.
    fn context_ask(&self, ask: context_bar::Ask) {
        let client = self.client.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let _ = tx.send(Msg::Workspace(context_bar::answer(&client, ask)));
        });
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
            Tab::Overview => screens::overview(frame, body, self),
            Tab::Agents => screens::agents(frame, body, self),
            Tab::Tasks => screens::tasks(frame, body, self),
            Tab::Approvals => screens::approvals(frame, body, self),
            Tab::Board => screens::board(frame, body, self),
            Tab::Nodes => screens::nodes(frame, body, self),
            Tab::Machines => machines::render(frame, body, self),
            Tab::Workstations => workstations::render(frame, body, self),
            Tab::Workspace => {
                workspace_view::render(frame, body, &mut self.ws, self.palette, self.clock)
            }
            Tab::Chat => chat::render(frame, body, self),
            Tab::Providers => {
                providers::render(
                    frame,
                    body,
                    &self.providers,
                    &self.data,
                    self.palette,
                    self.clock,
                );
            }
        }
        if let Some(ob) = &self.onboarding {
            ob.draw(frame, body, self.palette, self.client.base(), self.clock);
        }
        if let Some(notice) = &self.notice {
            let age = self.clock - self.notice_at;
            if age < style::TOAST_SECS {
                style::toast(frame, body, notice, age, &self.palette);
            }
        }
        if let Some(open) = &self.cmdk {
            open.draw(frame, body, self.palette, self.clock);
        }
        screens::footer(frame, footer, self);
    }

    /// Handles one key press; true when the UI should quit.
    pub fn on_key(&mut self, key: KeyEvent) -> bool {
        if key.kind != KeyEventKind::Press {
            return false;
        }
        // The command palette takes the keys while it is open. Ctrl+K opens it only where no other
        // screen takes the keys (the setup, a takeover, a draft, a question, a typing workspace).
        if let Some(open) = self.cmdk.as_mut() {
            match open.on_key(key) {
                cmdk::Step::Stay => {}
                cmdk::Step::Close => self.cmdk = None,
                cmdk::Step::Run(action) => {
                    self.cmdk = None;
                    return self.run_command(action);
                }
            }
            return false;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('k')
            && self.onboarding.is_none()
            && self.desk.is_none()
            && self.draft.is_none()
            && self.compose.is_none()
            && self.confirm.is_none()
            && !(self.tab == Tab::Workspace && self.ws.captures_keys())
        {
            self.cmdk = Some(cmdk::CommandPalette::new(self.clock));
            return false;
        }
        let typing = self.tab == Tab::Workspace && self.ws.captures_keys();
        // With a takeover panel open, Ctrl+C gives the session back rather than quitting (the panel takes it below).
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('c')
            && !typing
            && self.desk.is_none()
        {
            if self.draft.is_none() && self.compose.is_none() {
                return true;
            }
            self.draft = None;
            self.compose = None;
            return false;
        }
        self.notice = None;
        self.notice_seen = None;
        if self.onboarding.is_some() {
            self.on_onboarding_key(key.code);
            return false;
        }
        if self.desk.is_some() {
            self.on_desk_key(key);
            return false;
        }
        if self.draft.is_some() {
            self.on_draft_key(key.code);
            return false;
        }
        if self.compose.is_some() {
            self.on_compose_key(key.code);
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
        if self.tab == Tab::Providers
            && let Some(action) = self.providers.on_key(key, &self.data)
        {
            self.providers_act(action);
            return false;
        }
        if self.tab == Tab::Chat
            && let Some(ws_key) = workspace_key(key)
        {
            // The context bar's `i` and `M` (and the picker's keys while it is open) on the conversation.
            let shown = chat::selected(self);
            if let Some(effects) = self.ws.context_key(ws_key, shown) {
                self.execute(effects);
                return false;
            }
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
            KeyCode::Char('c') => self.select_tab(Tab::Chat),
            KeyCode::Char('m') => self.select_tab(Tab::Providers),
            KeyCode::Char('p') => self.step_scope(1),
            KeyCode::Char('P') => self.step_scope(-1),
            KeyCode::Char('n') => self.start_compose(true),
            KeyCode::Char('S') => self.ask_start(),
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
            KeyCode::Char('T') => self.open_desk(),
            KeyCode::Char('?') => self.begin(Stage::Tour(0)),
            KeyCode::Char('a') => self.ask_decision(true),
            KeyCode::Char('d') => self.ask_decision(false),
            KeyCode::Char('s') => {
                self.confirm = Some(if self.paused() {
                    Confirm::Resume
                } else {
                    Confirm::StopAll
                });
            }
            KeyCode::Char('r') => {
                let _ = self.watch.send(self.tab.path());
                self.fetch_projects();
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

    /// Runs a command the palette chose, with the methods its key calls. True when the UI should quit.
    fn run_command(&mut self, action: cmdk::Action) -> bool {
        match action {
            cmdk::Action::GoTo(tab) => self.select_tab(tab),
            cmdk::Action::NewConversation => {
                self.select_tab(Tab::Chat);
                self.start_compose(true);
            }
            cmdk::Action::MessageAgent => {
                if self.tab != Tab::Agents {
                    self.select_tab(Tab::Agents);
                }
                self.start_draft();
            }
            cmdk::Action::NextProject => self.step_scope(1),
            cmdk::Action::PreviousProject => self.step_scope(-1),
            cmdk::Action::Refresh => {
                let _ = self.watch.send(self.tab.path());
                self.fetch_projects();
                if self.tab == Tab::Workspace {
                    let effects = self.ws.enter();
                    self.execute(effects);
                }
                self.notice = Some("refreshing".to_string());
            }
            cmdk::Action::StopAll => self.confirm = Some(Confirm::StopAll),
            cmdk::Action::TakeTour => self.begin(Stage::Tour(0)),
            cmdk::Action::Quit => return true,
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
        if tab == Tab::Providers {
            self.fetch_providers();
        }
    }

    /// Does what the Providers tab's key asked: a hub call on a thread (its notice follows), the update dry run, a
    /// refresh, or a line for the footer.
    fn providers_act(&mut self, action: ProvidersAction) {
        match action {
            ProvidersAction::None => {}
            ProvidersAction::Note(text) => self.notice = Some(text),
            ProvidersAction::Run { label, calls } => {
                self.run_job(label, move |c| providers::run_calls(c, &calls));
            }
            ProvidersAction::Plan => {
                let client = self.client.clone();
                let tx = self.tx.clone();
                thread::spawn(move || {
                    let _ = tx.send(Msg::Plan(providers::dry_run(&client)));
                });
            }
            ProvidersAction::Refresh => {
                self.fetch_providers();
                let _ = self.watch.send(providers::PATH);
                self.notice = Some("refreshing".to_string());
            }
        }
    }

    /// Reads what the Providers tab shows besides the poller's endpoint: each one on its own thread, as `Fetched`.
    fn fetch_providers(&self) {
        for path in providers::READS {
            let client = self.client.clone();
            let tx = self.tx.clone();
            thread::spawn(move || {
                let _ = tx.send(Msg::Fetched(path, client.get(path)));
            });
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
        self.shown_agents()
            .get(self.selected)
            .and_then(|a| a.get("cid"))
            .and_then(Value::as_i64)
    }

    fn start_draft(&mut self) {
        if self.tab == Tab::Chat {
            self.start_compose(false);
            return;
        }
        if self.tab != Tab::Agents {
            return;
        }
        if let Some(cid) = self.selected_agent_cid() {
            self.draft = Some((cid, String::new()));
        }
    }

    /// The id of the conversation the Chat tab selects.
    fn selected_conversation(&self) -> Option<String> {
        self.array(chat::PATH)
            .get(self.selected)
            .and_then(|c| c.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// Opens the footer's composer on the Chat tab: for a new conversation, or for the selected one.
    fn start_compose(&mut self, new: bool) {
        if self.tab != Tab::Chat {
            return;
        }
        if new {
            self.compose = Some(chat::Compose::New(String::new()));
            return;
        }
        match self.selected_conversation() {
            Some(id) => self.compose = Some(chat::Compose::To(id, String::new())),
            None => self.notice = Some("no conversation yet: n opens one".to_string()),
        }
    }

    /// The composer takes the keys: Enter sends the text (the first message of a new conversation, which lists first),
    /// Esc drops it.
    fn on_compose_key(&mut self, code: KeyCode) {
        let Some(mut compose) = self.compose.take() else {
            return;
        };
        match code {
            KeyCode::Esc => {}
            KeyCode::Enter => {
                let text = compose.text().trim().to_string();
                if text.is_empty() {
                    self.compose = Some(compose);
                    return;
                }
                match compose {
                    chat::Compose::To(id, _) => {
                        self.run_job(format!("message to {id}"), move |c| {
                            c.post(
                                &format!("{}/{}/messages", chat::PATH, seg(&id)),
                                &json!({ "text": text }),
                            )
                        });
                    }
                    chat::Compose::New(_) => {
                        // The newest conversation lists first: it is the one selected once it is open.
                        self.selected = 0;
                        self.run_job("new conversation".to_string(), move |c| {
                            c.post(chat::PATH, &json!({ "text": text }))
                        });
                    }
                }
            }
            KeyCode::Backspace => {
                compose.text_mut().pop();
                self.compose = Some(compose);
            }
            KeyCode::Char(c) => {
                if compose.text().chars().count() < workspace::COMPOSE_MAX {
                    compose.text_mut().push(c);
                }
                self.compose = Some(compose);
            }
            _ => self.compose = Some(compose),
        }
    }

    /// Asks to start the selected conversation's draft at the revision on screen; a draft something still stops, or
    /// one started already, says so instead.
    fn ask_start(&mut self) {
        if self.tab != Tab::Chat {
            return;
        }
        let Some(id) = self.selected_conversation() else {
            return;
        };
        let view = self.chat.views.get(&id);
        let Some(draft) = view.and_then(|v| v.get("draft")).filter(|d| d.is_object()) else {
            self.notice = Some(format!("{id} has no project draft yet"));
            return;
        };
        if let Some(project) = view.and_then(|v| v.get("project")).and_then(Value::as_str) {
            self.notice = Some(format!("{id} started project {project} already"));
            return;
        }
        if !cmd::chat::startable(draft) {
            let problems: Vec<&str> = draft
                .get("problems")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            self.notice = Some(format!(
                "the draft cannot start yet: {}",
                problems.join("; ")
            ));
            return;
        }
        self.confirm = Some(Confirm::Start {
            conversation: id,
            revision: draft.get("revision").and_then(Value::as_u64).unwrap_or(0),
            title: draft
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        });
    }

    fn on_draft_key(&mut self, code: KeyCode) {
        let Some((cid, mut text)) = self.draft.take() else {
            return;
        };
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

    /// Opens the takeover panel on the selected workstation: a thread takes its shared session and does the panel's
    /// lines. Only on the Workstations tab.
    fn open_desk(&mut self) {
        if self.tab != Tab::Workstations {
            return;
        }
        let Some(station) = self
            .array("/v1/stations")
            .get(self.selected)
            .and_then(|s| s.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            return;
        };
        self.desk_seq += 1;
        let number = self.desk_seq;
        let (orders, rx) = mpsc::channel();
        let target = self.client.target.clone();
        let tx = self.tx.clone();
        let taking = station.clone();
        thread::spawn(move || {
            let say = |report: control::Report| {
                let _ = tx.send(Msg::Desk(number, report));
            };
            control::run_panel(
                &target,
                &taking,
                cmd::stations::screen::DEFAULT_SESSION,
                &rx,
                &say,
            );
        });
        self.desk = Some(workstations::Panel::new(number, station, orders));
    }

    /// A report of the takeover panel with that number. Its end closes the panel, unless it is already closed.
    fn on_desk_report(&mut self, number: u64, report: control::Report) {
        if self
            .desk
            .as_ref()
            .is_none_or(|panel| panel.number != number)
        {
            return;
        }
        match report {
            control::Report::Line(text) => self.desk_push(text),
            control::Report::Refused(text) => self.desk_push(format!("refused: {text}")),
            control::Report::Ended => {
                self.desk = None;
                self.notice = Some("the takeover is over".to_string());
            }
        }
    }

    fn desk_push(&mut self, text: String) {
        if let Some(panel) = self.desk.as_mut() {
            panel.push(text);
        }
    }

    /// The takeover panel takes the keys: the line being typed; Enter sends it (`give-back` gives the session back);
    /// Esc or Ctrl+C gives the session back.
    fn on_desk_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Esc || (ctrl && key.code == KeyCode::Char('c')) {
            self.close_desk();
            return;
        }
        let Some(panel) = self.desk.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Enter => {
                let line = std::mem::take(&mut panel.line);
                let text = line.trim().to_string();
                if text == "give-back" || text == "give_back" {
                    self.close_desk();
                } else if !text.is_empty() {
                    panel.push(format!("> {text}"));
                    let _ = panel.orders.send(control::Order::Line(line));
                }
            }
            KeyCode::Backspace => {
                panel.line.pop();
            }
            KeyCode::Char(c) if !ctrl => panel.line.push(c),
            _ => {}
        }
    }

    /// Gives the session back: the takeover's thread does it, and its end is ignored once the panel is gone.
    fn close_desk(&mut self) {
        if let Some(panel) = self.desk.take() {
            let _ = panel.orders.send(control::Order::GiveBack);
            self.notice = Some(format!("giving {} back", panel.station));
        }
    }

    fn ask_decision(&mut self, approve: bool) {
        if self.tab != Tab::Approvals {
            return;
        }
        let Some(item) = self.array("/v1/approvals").get(self.selected) else {
            return;
        };
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if item.get("state").and_then(Value::as_str) != Some("pending") {
            self.notice = Some("that approval is already decided".to_string());
            return;
        }
        self.confirm = Some(if approve {
            Confirm::Approve(id)
        } else {
            Confirm::Deny(id)
        });
    }

    fn run(&mut self, confirm: Confirm) {
        match confirm {
            Confirm::StopAll => self.run_job("stop all".to_string(), |c| {
                c.post(
                    "/v1/plaza/stop",
                    &json!({ "reason": "stopped from the terminal" }),
                )
            }),
            Confirm::Resume => self.run_job("resume".to_string(), |c| {
                c.post("/v1/plaza/resume", &json!({}))
            }),
            Confirm::Approve(id) => self.decide(id, true),
            Confirm::Deny(id) => self.decide(id, false),
            Confirm::CancelWork(cid) => self
                .run_job(format!("stop agent {cid}'s work"), move |c| {
                    c.post(&format!("/v1/agents/{cid}/cancel"), &json!({}))
                }),
            Confirm::Start {
                conversation,
                revision,
                ..
            } => self.run_job(format!("start {conversation}"), move |c| {
                let mut client = c.clone();
                client.timeout = START_TIMEOUT;
                client.post(
                    &format!("{}/{}/start", chat::PATH, seg(&conversation)),
                    &json!({ "revision": revision }),
                )
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
                Effect::Steer { cid, text } => self
                    .run_job(format!("message to agent {cid}"), move |c| {
                        c.post(&format!("/v1/agents/{cid}/chat"), &json!({ "text": text }))
                    }),
                Effect::Post { project, body } => {
                    let mentions = cmd::board::mentions_in(&body);
                    self.run_job("post to the board".to_string(), move |c| {
                        c.post(
                            "/v1/board/posts",
                            &json!({ "body": body, "project": project, "mentions": mentions }),
                        )
                    });
                }
                Effect::Pause { project, on } => {
                    let label = format!("{} the board", if on { "pause" } else { "resume" });
                    self.run_job(label, move |c| {
                        c.post("/v1/board/pause", &json!({ "on": on, "project": project }))
                    });
                }
                Effect::Export { project } => self.export(project),
                Effect::AskStop(cid) => self.confirm = Some(Confirm::CancelWork(cid)),
                Effect::Context(ask) => self.context_ask(ask),
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
                    Got::Board {
                        project,
                        result: client.get(&path),
                    }
                }
                Want::Transcript { cid, after } => {
                    let path = match after {
                        Some(seq) => format!("/v1/agents/{cid}/transcript?after={seq}&limit=1000"),
                        None => {
                            format!("/v1/agents/{cid}/transcript?last={}", workspace::FIRST_LOAD)
                        }
                    };
                    Got::Transcript {
                        cid,
                        result: client.get(&path),
                    }
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
            Some(id) => id
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect(),
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
        let label = format!(
            "{} {}",
            if approve { "approve" } else { "deny" },
            short(&id)
        );
        let path = format!("/v1/approvals/{}", seg(&id));
        self.run_job(label, move |c| {
            c.post(&path, &json!({ "approve": approve }))
        });
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
        // The overview counts the approvals waiting.
        if visible == "/v1/overview" {
            paths.push("/v1/approvals");
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
            let recent = client.get(&format!(
                "/v1/stations/recordings?station={}&limit={RECENT_RECORDINGS}",
                seg(&id)
            ));
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
        if let Ok(mut socket) = ws::connect(
            &target,
            "/v1/ws",
            Some("apz.events.v1"),
            Duration::from_secs(10),
            ws::Limits::default(),
        ) {
            wait = Duration::from_millis(500);
            if tx.send(Msg::Link(true)).is_err() {
                return;
            }
            loop {
                match socket.recv(Duration::from_secs(1)) {
                    Ok(Some(ws::Message::Text(text))) => {
                        if let Some((kind, data)) = parse_frame(&text)
                            && tx.send(Msg::Event(kind, data)).is_err()
                        {
                            return;
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
        term.draw(|frame| app.draw(frame))
            .map_err(|e| e.to_string())?;
        if event::poll(FRAME).map_err(|e| e.to_string())?
            && let Event::Key(key) = event::read().map_err(|e| e.to_string())?
            && app.on_key(key)
        {
            return Ok(());
        }
        let now = Instant::now();
        app.tick(now.duration_since(last).as_secs_f32());
        last = now;
    }
}

/// `APZ_TUI_SNAPSHOT=<file>`: runs the app off-screen for `secs` seconds against the hub (polls and
/// live events as usual), then writes every tab as plain text, `width`×`height`, into `path` —
/// the terminal UI's equivalent of the desktop app's `APZ_SNAPSHOT`.
pub fn snapshot(
    client: Client,
    path: &std::path::Path,
    width: u16,
    height: u16,
    secs: f32,
) -> Result<(), String> {
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
        let mut term = Terminal::new(ratatui::backend::TestBackend::new(width, height))
            .map_err(|e| e.to_string())?;
        term.draw(|frame| app.draw(frame))
            .map_err(|e| e.to_string())?;
        let buffer = term.backend().buffer();
        out.push_str(&format!("──── tab {} {:?} ────\n", i + 1, tab));
        for y in 0..buffer.area.height {
            let line: String = (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect();
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
        assert_eq!(Tab::Overview.prev(), Tab::Providers);
        assert_eq!(Tab::Workspace.next(), Tab::Workstations);
        assert_eq!(Tab::Workstations.next(), Tab::Chat);
        assert_eq!(Tab::Chat.next(), Tab::Providers);
        assert_eq!(Tab::Providers.next(), Tab::Overview);
        assert_eq!(Tab::Machines.next(), Tab::Workspace);
        assert_eq!(Tab::Nodes.next(), Tab::Machines);
        assert_eq!(Tab::Workstations.path(), "/v1/stations");
        assert_eq!(Tab::Overview.next().next(), Tab::Tasks);
        assert_eq!(Tab::Overview.next().prev(), Tab::Overview);
    }

    /// An app whose hub never answers (nothing listens on the port): the tests feed it the hub's data themselves.
    fn offline_app() -> App {
        App::new(Client::new("http://127.0.0.1:9", None, None))
    }

    fn press(app: &mut App, code: KeyCode) -> bool {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn the_switcher_filters_the_agents_and_tasks_tabs() {
        let mut app = offline_app();
        app.data.insert(
            "/v1/projects",
            json!([
                { "id": "pr2", "title": "Old", "stage": "implementation", "archived": true, "tasks": [] },
                { "id": "pr3", "title": "Release 0.9", "stage": "implementation", "tasks": ["t9"] }
            ]),
        );
        app.data.insert(
            "/v1/agents",
            json!([{ "cid": 8, "project": null }, { "cid": 7, "project": "pr3" }]),
        );
        app.data.insert(
            "/v1/tasks",
            json!({ "tasks": [
                { "id": "t1", "project": "pr3" },
                { "id": "t2", "project": null },
                { "id": "t9", "project": null }
            ]}),
        );
        app.select_tab(Tab::Agents);
        app.selected = 1;
        press(&mut app, KeyCode::Char('p'));
        assert_eq!(
            app.scope.as_deref(),
            Some("pr3"),
            "the archived pr2 is not offered"
        );
        assert_eq!(app.selected, 0, "the selection starts over");
        assert_eq!(app.selected_agent_cid(), Some(7));
        assert_eq!(app.rows(), 1);
        let tasks: Vec<&Value> = app.shown_tasks().iter().map(|t| &t["id"]).collect();
        assert_eq!(tasks, [&json!("t1"), &json!("t9")]);
        press(&mut app, KeyCode::Char('p'));
        assert_eq!(app.scope, None, "past the last project is All again");
        assert_eq!(app.shown_agents().len(), 2);
        press(&mut app, KeyCode::Char('P'));
        assert_eq!(app.scope.as_deref(), Some("pr3"), "P steps back");
    }

    #[test]
    fn the_chat_tab_writes_to_a_conversation_and_asks_before_it_starts_one() {
        let mut app = offline_app();
        press(&mut app, KeyCode::Char('c'));
        assert_eq!(app.tab, Tab::Chat);
        press(&mut app, KeyCode::Char('t'));
        assert!(app.compose.is_none());
        assert_eq!(
            app.notice.as_deref(),
            Some("no conversation yet: n opens one")
        );
        app.data.insert(
            chat::PATH,
            json!([{ "id": "conv-3", "title": "Release 0.9", "updated_ms": 5, "pending": 0 }]),
        );
        app.chat.on_view(json!({
            "id": "conv-3", "cid": 3, "updated_ms": 5, "messages": [],
            "draft": { "revision": 2, "title": "Release 0.9", "problems": [] }
        }));
        press(&mut app, KeyCode::Char('t'));
        for c in "go q".chars() {
            assert!(!press(&mut app, KeyCode::Char(c)), "q is text while typing");
        }
        assert_eq!(
            app.compose,
            Some(chat::Compose::To("conv-3".into(), "go q".into()))
        );
        press(&mut app, KeyCode::Esc);
        assert!(app.compose.is_none(), "Esc drops the text");
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.compose, Some(chat::Compose::New(String::new())));
        press(&mut app, KeyCode::Enter);
        assert!(app.compose.is_some(), "an empty message is not sent");
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('S'));
        assert_eq!(
            app.confirm,
            Some(Confirm::Start {
                conversation: "conv-3".into(),
                revision: 2,
                title: "Release 0.9".into()
            })
        );
        press(&mut app, KeyCode::Char('n'));
        assert!(app.confirm.is_none(), "n answers the question");
        // A draft something still stops is not asked about.
        app.chat.on_view(json!({
            "id": "conv-3", "cid": 3, "updated_ms": 6, "messages": [],
            "draft": { "revision": 3, "title": "Release 0.9", "problems": ["the draft has no tasks yet"] }
        }));
        press(&mut app, KeyCode::Char('S'));
        assert!(app.confirm.is_none());
        assert_eq!(
            app.notice.as_deref(),
            Some("the draft cannot start yet: the draft has no tasks yet")
        );
    }

    #[test]
    fn every_tab_has_a_key_that_shows_it() {
        for tab in Tab::ALL {
            assert_eq!(Tab::from_key(tab.key()), Some(tab), "{tab:?}");
        }
        assert_eq!(Tab::Chat.key(), 'c');
        assert_eq!(Tab::Providers.key(), 'm');
        assert_eq!(Tab::from_key('m'), Some(Tab::Providers));
        assert_eq!(Tab::Providers.path(), "/v1/providers");
        assert_eq!(Tab::Workstations.key(), 'w');
        assert_eq!(Tab::Chat.path(), "/v1/owner/conversations");
    }

    #[test]
    fn the_takeover_and_sync_events_refresh_the_workstations_tab() {
        for kind in [
            "station_takeover",
            "station_give_back",
            "station_sync_conflict",
            "station_sync_paused",
            "lease_granted",
        ] {
            assert!(is_station_change(kind), "{kind} changes the tab");
        }
        assert!(
            !is_station_change("station_sync"),
            "a pass that moved files is read on the next poll"
        );
        assert!(!is_station_change("owner_input"));
    }
}
