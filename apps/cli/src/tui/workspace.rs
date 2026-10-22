//! The Workspace tab's model (docs/design/workspace.md, section 2): the agents grouped by project the way the IDE groups
//! them, the selected agent's transcript and its composer, the project's board, and the rules for the filter, the focus
//! and the narrow layout. Pure: no terminal and no hub. The hub's answers come in as JSON, and the work they call for goes
//! out as `Effect`s that `app` runs, so these rules are tested without either. The drawing is in `workspace_view`.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::context_bar::{self, Ask, Context, Owner, Picker, Stroke as PickerStroke};

/// Below this many columns the tab shows one pane at a time.
pub const NARROW_BELOW: u16 = 120;
/// The most characters a composer holds: the hub's limit for a chat text.
pub const COMPOSE_MAX: usize = 20_000;
/// The entries a transcript keeps in memory; the oldest go first.
const KEEP: usize = 1_000;
/// The entries the first load of a transcript asks for.
pub const FIRST_LOAD: usize = 300;
/// The rows a page moves the agents list by.
const PAGE_ROWS: isize = 5;
/// A move past the end of the list (End in the agents pane).
const FAR: isize = 100_000;
/// The title of the group of the agents in no project. It sorts last.
pub const NO_PROJECT: &str = "No project";

static NULL: Value = Value::Null;

pub(super) fn str_of<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

pub(super) fn data_of(entry: &Value) -> &Value {
    entry.get("data").unwrap_or(&NULL)
}

pub(super) fn kind_of(entry: &Value) -> &str {
    str_of(entry, "kind").unwrap_or("")
}

fn cid_of(agent: &Value) -> Option<u64> {
    agent.get("cid").and_then(Value::as_u64)
}

pub(super) fn seq_of(entry: &Value) -> u64 {
    entry.get("seq").and_then(Value::as_u64).unwrap_or(0)
}

/// An agent by cid, from a roster (`GET /v1/agents`).
pub fn agent_of(agents: &[Value], cid: u64) -> Option<&Value> {
    agents.iter().find(|agent| cid_of(agent) == Some(cid))
}

/// The agent's current task: the prompt of the work it is doing now.
pub fn task_of(agent: &Value) -> Option<&str> {
    agent
        .pointer("/work/prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|task| !task.is_empty())
}

/// The lists the tab reads from the hub: the array itself, or the array under `key` (`{"tasks": [...]}`).
fn listed(value: &Value, key: &str) -> Vec<Value> {
    let list = value
        .as_array()
        .or_else(|| value.get(key).and_then(Value::as_array));
    list.cloned().unwrap_or_default()
}

/// The three panes of the tab.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Pane {
    #[default]
    Agents,
    Chat,
    Board,
}

impl Pane {
    pub const ALL: [Pane; 3] = [Pane::Agents, Pane::Chat, Pane::Board];

    pub fn title(self) -> &'static str {
        match self {
            Pane::Agents => "Agents",
            Pane::Chat => "Chat",
            Pane::Board => "Board",
        }
    }

    fn index(self) -> usize {
        Pane::ALL.iter().position(|pane| *pane == self).unwrap_or(0)
    }

    pub fn next(self) -> Pane {
        Pane::ALL[(self.index() + 1) % Pane::ALL.len()]
    }

    pub fn prev(self) -> Pane {
        Pane::ALL[(self.index() + Pane::ALL.len() - 1) % Pane::ALL.len()]
    }
}

/// The panes a width shows: all three side by side, or only the focused one below `NARROW_BELOW` columns.
pub fn visible(width: u16, focus: Pane) -> Vec<Pane> {
    if width < NARROW_BELOW {
        vec![focus]
    } else {
        Pane::ALL.to_vec()
    }
}

/// An agent's status mark: working, idle, sleeping (offline), done or failed (its last outcome, while it is idle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    Working,
    Idle,
    Sleeping,
    Done,
    Failed,
}

impl Glyph {
    pub fn mark(self) -> char {
        match self {
            Glyph::Working => '●',
            Glyph::Idle => '○',
            Glyph::Sleeping => '☾',
            Glyph::Done => '✓',
            Glyph::Failed => '✗',
        }
    }
}

/// The mark of an agent from its status and the outcome of its last finished work (`done`, `failed`, `error`).
pub fn glyph(status: &str, outcome: Option<&str>) -> Glyph {
    match status {
        "working" => Glyph::Working,
        "offline" | "asleep" => Glyph::Sleeping,
        _ => match outcome {
            Some("done") => Glyph::Done,
            Some("failed" | "error") => Glyph::Failed,
            _ => Glyph::Idle,
        },
    }
}

/// One group of the agents list: a project (or none), and the agents in it, by label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub project: Option<String>,
    pub title: String,
    pub cids: Vec<u64>,
}

/// Whether an agent matches the filter: its role or its cid contains it (case-insensitive). An empty filter matches
/// every agent.
pub fn matches(agent: &Value, filter: &str) -> bool {
    let needle = filter.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }
    let by_text = ["role_label", "role"]
        .iter()
        .any(|key| str_of(agent, key).is_some_and(|text| text.to_lowercase().contains(&needle)));
    by_text || cid_of(agent).is_some_and(|cid| cid.to_string().contains(&needle))
}

/// The cids sorted by their label: the role, then the cid (agents have no names).
fn by_label(agents: &[Value], mut cids: Vec<u64>) -> Vec<u64> {
    let key = |cid: u64| {
        let role = agent_of(agents, cid)
            .and_then(|agent| str_of(agent, "role"))
            .unwrap_or("");
        (role.to_lowercase(), cid)
    };
    cids.sort_by_key(|&cid| key(cid));
    cids
}

/// The agents grouped by project, as the IDE groups them (`apps/web/src/ide/groups.ts`): an agent whose live session is
/// on a project sits under it, and only it; one with no project (the orchestrator, or an agent with no live session)
/// sits under every project it leads and every project holding a task it is assigned to (claimed, or working on now).
/// Projects come most recently active first; the agents in none go to `NO_PROJECT`, last; empty groups are left out.
pub fn groups(agents: &[Value], projects: &[Value], tasks: &[Value], filter: &str) -> Vec<Group> {
    let mut sorted: Vec<&Value> = projects
        .iter()
        .filter(|p| str_of(p, "id").is_some())
        .collect();
    sorted.sort_by_key(|p| {
        std::cmp::Reverse(p.get("updated_ms").and_then(Value::as_u64).unwrap_or(0))
    });
    let assignees: HashMap<&str, u64> = tasks
        .iter()
        .filter_map(|task| Some((str_of(task, "id")?, task.get("agent_cid")?.as_u64()?)))
        .collect();
    let mut lists: Vec<Vec<u64>> = vec![Vec::new(); sorted.len()];
    let mut loose: Vec<u64> = Vec::new();
    for agent in agents {
        let Some(cid) = cid_of(agent) else { continue };
        if !matches(agent, filter) {
            continue;
        }
        let origin = agent.pointer("/work/origin/task").and_then(Value::as_str);
        let on_project = str_of(agent, "project").filter(|p| !p.is_empty());
        let mut placed = false;
        for (list, project) in lists.iter_mut().zip(&sorted) {
            let leads = project.get("lead_cid").and_then(Value::as_u64) == Some(cid);
            let holds = project
                .get("tasks")
                .and_then(Value::as_array)
                .is_some_and(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .any(|id| origin == Some(id) || assignees.get(id) == Some(&cid))
                });
            let in_it = match on_project {
                Some(id) => str_of(project, "id") == Some(id),
                None => leads || holds,
            };
            if in_it {
                list.push(cid);
                placed = true;
            }
        }
        if !placed {
            loose.push(cid);
        }
    }
    let mut out = Vec::new();
    for (list, project) in lists.into_iter().zip(&sorted) {
        if list.is_empty() {
            continue;
        }
        let id = str_of(project, "id").unwrap_or("").to_string();
        let title = str_of(project, "title")
            .filter(|t| !t.is_empty())
            .unwrap_or(&id)
            .to_string();
        out.push(Group {
            project: Some(id),
            title,
            cids: by_label(agents, list),
        });
    }
    if !loose.is_empty() {
        out.push(Group {
            project: None,
            title: NO_PROJECT.to_string(),
            cids: by_label(agents, loose),
        });
    }
    out
}

/// A row of the agents pane: a group's title, or one agent under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Header { title: String, count: usize },
    Agent { cid: u64, project: Option<String> },
}

/// The rows of the groups, in order: each title, then its agents.
pub fn rows(groups: &[Group]) -> Vec<Row> {
    let mut out = Vec::new();
    for group in groups {
        out.push(Row::Header {
            title: group.title.clone(),
            count: group.cids.len(),
        });
        out.extend(group.cids.iter().map(|&cid| Row::Agent {
            cid,
            project: group.project.clone(),
        }));
    }
    out
}

/// One block of the chat: a transcript entry, or a run of tool steps that folds into one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Entry(usize),
    /// The entries `first..end` (indexes into the transcript), and the line that sums them up.
    Steps {
        first: usize,
        end: usize,
        label: String,
    },
}

/// Whether an entry is part of a tool step: a call, its decision or its result, or a model turn that only asks for tools.
pub fn is_step(entry: &Value) -> bool {
    match kind_of(entry) {
        "tool_request" | "tool_decision" | "tool_result" => true,
        "model_response" => data_of(entry)
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|blocks| {
                !blocks.is_empty() && blocks.iter().all(|b| str_of(b, "type") == Some("tool_use"))
            }),
        _ => false,
    }
}

fn tool_uses(entry: &Value) -> Vec<String> {
    data_of(entry)
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| str_of(block, "type") == Some("tool_use"))
        .filter_map(|block| str_of(block, "name").map(str::to_string))
        .collect()
}

/// "3 steps · bash": the count and the last step. One step reads "1 step".
pub fn steps_label(count: usize, last: Option<&str>) -> String {
    let noun = if count == 1 { "step" } else { "steps" };
    match last {
        Some(name) => format!("{count} {noun} · {name}"),
        None => format!("{count} {noun}"),
    }
}

/// The blocks of a transcript: each entry as it is, except that a run of tool steps becomes one `Steps` block. A run is
/// counted by the calls its model turns make; a run that names no tool is counted by its entries.
pub fn items(entries: &[Value]) -> Vec<Item> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < entries.len() {
        if !is_step(&entries[i]) {
            out.push(Item::Entry(i));
            i += 1;
            continue;
        }
        let first = i;
        let mut calls: Vec<String> = Vec::new();
        let mut requests: Vec<String> = Vec::new();
        while i < entries.len() && is_step(&entries[i]) {
            match kind_of(&entries[i]) {
                "model_response" => calls.extend(tool_uses(&entries[i])),
                "tool_request" => {
                    requests.extend(str_of(data_of(&entries[i]), "name").map(str::to_string))
                }
                _ => {}
            }
            i += 1;
        }
        let names = if calls.is_empty() { requests } else { calls };
        let count = if names.is_empty() {
            i - first
        } else {
            names.len()
        };
        out.push(Item::Steps {
            first,
            end: i,
            label: steps_label(count, names.last().map(String::as_str)),
        });
    }
    out
}

/// The seq of the first entry of a block: the block's identity while the transcript grows.
pub fn first_seq(entries: &[Value], item: &Item) -> u64 {
    match item {
        Item::Entry(i) => seq_of(&entries[*i]),
        Item::Steps { first, .. } => seq_of(&entries[*first]),
    }
}

/// The outcome of the last finished work in a transcript (its `work_end` status), if it has one.
pub fn outcome_of(entries: &[Value]) -> Option<String> {
    entries
        .iter()
        .rev()
        .find(|entry| kind_of(entry) == "work_end")
        .and_then(|entry| str_of(data_of(entry), "status"))
        .map(str::to_string)
}

/// How a list is scrolled: the top line, and whether it follows the end (new lines show at once). Each draw sizes it with
/// `settle`, so the key handlers can move it by the size the last draw had.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct View {
    pub top: usize,
    pub follow: bool,
    pub height: usize,
    pub total: usize,
}

impl Default for View {
    fn default() -> Self {
        View {
            top: 0,
            follow: true,
            height: 0,
            total: 0,
        }
    }
}

impl View {
    fn bottom(&self) -> usize {
        self.total.saturating_sub(self.height)
    }

    /// Sizes the view for `total` lines in `height` rows and returns its top line. While following, the end is shown;
    /// otherwise the reader's place is kept, and moved only as far as needed to show `keep` (the lines of the cursor).
    pub fn settle(&mut self, total: usize, height: usize, keep: Option<(usize, usize)>) -> usize {
        self.total = total;
        self.height = height;
        let bottom = self.bottom();
        if self.follow {
            self.top = bottom;
        }
        if let Some((start, end)) = keep
            && height > 0
        {
            let end = end.min(start + height);
            if start < self.top {
                self.top = start;
            } else if end > self.top + height {
                self.top = end - height;
            }
        }
        self.top = self.top.min(bottom);
        self.follow = self.top >= bottom;
        self.top
    }

    /// Moves the view by `delta` lines, as the last `settle` sized it.
    pub fn scroll(&mut self, delta: isize) {
        let bottom = self.bottom() as isize;
        self.top = (self.top as isize + delta).clamp(0, bottom) as usize;
        self.follow = self.top as isize >= bottom;
    }

    /// The lines a page moves by.
    pub fn page(&self) -> isize {
        self.height.max(2) as isize - 1
    }

    /// Goes to the end, and follows it.
    pub fn end(&mut self) {
        self.follow = true;
        self.top = self.bottom();
    }
}

/// One agent's chat: the entries in seq order, whether the hub's history has been loaded, the step runs that are open
/// (by their first seq), the cursor, and the view.
#[derive(Debug, Clone, Default)]
pub struct Transcript {
    pub entries: Vec<Value>,
    pub loaded: bool,
    pub requested: bool,
    pub open: HashSet<u64>,
    /// The first seq of the block under the cursor; None is the newest block.
    pub cursor: Option<u64>,
    /// Set by a cursor move: the next draw keeps the cursor's block on screen.
    pub reveal: bool,
    pub view: View,
}

impl Transcript {
    pub fn last_seq(&self) -> u64 {
        self.entries.last().map_or(0, seq_of)
    }

    /// Puts an entry in its place by seq; an entry already held is skipped. True when the entry is past a gap: entries
    /// before it were missed, and the caller asks for them.
    pub fn merge(&mut self, entry: Value) -> bool {
        let seq = seq_of(&entry);
        if seq == 0 {
            return false;
        }
        let gap = !self.entries.is_empty() && seq > self.last_seq() + 1;
        if let Err(at) = self.entries.binary_search_by_key(&seq, seq_of) {
            self.entries.insert(at, entry);
        }
        if self.entries.len() > KEEP {
            let extra = self.entries.len() - KEEP;
            self.entries.drain(..extra);
        }
        gap
    }

    /// The index of the block under the cursor (the newest when the cursor is not set).
    pub fn cursor_item(&self, list: &[Item]) -> Option<usize> {
        let found = self.cursor.and_then(|seq| {
            list.iter()
                .position(|item| first_seq(&self.entries, item) == seq)
        });
        found.or_else(|| list.len().checked_sub(1))
    }

    /// The seq the cursor is on, for drawing: the explicit cursor, else the newest block.
    pub fn cursor_seq(&self) -> Option<u64> {
        let list = items(&self.entries);
        self.cursor_item(&list)
            .map(|index| first_seq(&self.entries, &list[index]))
    }

    /// Moves the cursor by `delta` blocks, and asks the next draw to show it.
    pub fn move_cursor(&mut self, delta: isize) {
        let list = items(&self.entries);
        let Some(current) = self.cursor_item(&list) else {
            return;
        };
        let next = (current as isize + delta).clamp(0, list.len() as isize - 1) as usize;
        self.cursor = Some(first_seq(&self.entries, &list[next]));
        self.reveal = true;
    }

    /// Opens or folds the steps under the cursor (other blocks do nothing).
    pub fn toggle(&mut self) {
        let list = items(&self.entries);
        let Some(index) = self.cursor_item(&list) else {
            return;
        };
        if let Item::Steps { .. } = &list[index] {
            let seq = first_seq(&self.entries, &list[index]);
            if !self.open.remove(&seq) {
                self.open.insert(seq);
            }
        }
    }

    /// Puts the cursor back on the newest block, and follows the end.
    pub fn follow_newest(&mut self) {
        self.cursor = None;
        self.view.end();
    }
}

/// The selected agent and the project whose board the chat shows (the agent's group it is listed under).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub cid: u64,
    pub project: Option<String>,
}

/// Where a composed text goes: a steer note to an agent, or a post on a board (of a project, or the whole board).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Steer(u64),
    Board(Option<String>),
}

/// A text being written: a steer note or a board post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composer {
    pub target: Target,
    pub text: String,
}

/// The keys the tab reads. The app maps terminal keys to these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    PageUp,
    PageDown,
    End,
    Enter,
    Tab,
    BackTab,
    Esc,
    Backspace,
    CtrlC,
    Char(char),
}

/// What a key does to a composer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stroke {
    Keep,
    /// Esc: the text is dropped and the composer closes.
    Close,
    /// Enter with text: the text goes to its target, and the composer closes.
    Send(Target, String),
    /// Ctrl+C in a steer note: stop the agent's work, after a confirm.
    AskStop(u64),
}

impl Composer {
    pub fn new(target: Target) -> Composer {
        Composer {
            target,
            text: String::new(),
        }
    }

    pub fn key(&mut self, key: Key) -> Stroke {
        match key {
            Key::Esc => Stroke::Close,
            Key::Enter => {
                let body = self.text.trim().to_string();
                if body.is_empty() {
                    Stroke::Keep
                } else {
                    Stroke::Send(self.target.clone(), body)
                }
            }
            Key::Backspace => {
                self.text.pop();
                Stroke::Keep
            }
            Key::CtrlC => match self.target {
                Target::Steer(cid) => Stroke::AskStop(cid),
                Target::Board(_) => Stroke::Keep,
            },
            Key::Char(c) => {
                if !c.is_control() && self.text.chars().count() < COMPOSE_MAX {
                    self.text.push(c);
                }
                Stroke::Keep
            }
            _ => Stroke::Keep,
        }
    }
}

/// What the tab asks the hub for. The answer comes back as a `Got`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Want {
    Projects,
    Tasks,
    Board(Option<String>),
    /// The newest entries of an agent (`after` None), or the entries after `after`.
    Transcript {
        cid: u64,
        after: Option<u64>,
    },
}

/// An answer to a `Want`.
#[derive(Debug)]
pub enum Got {
    Projects(Result<Value, String>),
    Tasks(Result<Value, String>),
    Board {
        project: Option<String>,
        result: Result<Value, String>,
    },
    Transcript {
        cid: u64,
        result: Result<Value, String>,
    },
    /// A chat's context (`GET .../context`), by its owner.
    Context {
        owner: Owner,
        result: Result<Value, String>,
    },
    /// The model list (`GET /v1/models`) the picker reads.
    Models(Result<Value, String>),
    /// A model choice the hub took or refused (`PATCH`), for the chat it was made for.
    Saved {
        owner: Owner,
        result: Result<Value, String>,
    },
}

/// Work the app does for the tab: a fetch, a request to the agents list, a write to the hub, or a notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Fetch(Want),
    /// The agents roster again (`GET /v1/agents`).
    Agents,
    Steer {
        cid: u64,
        text: String,
    },
    Post {
        project: Option<String>,
        body: String,
    },
    Pause {
        project: Option<String>,
        on: bool,
    },
    Export {
        project: Option<String>,
    },
    /// Stop the agent's work, after a confirm.
    AskStop(u64),
    /// A request for the context bar or the model picker (see `context_bar::Ask`).
    Context(Ask),
    Notice(String),
}

/// A project's board as the hub answered it (`GET /v1/board`, with the project as the scope).
#[derive(Debug, Clone)]
pub struct BoardView {
    pub project: Option<String>,
    pub value: Value,
}

/// The tab's state. Everything the hub sent is kept here, and the drawing reads it.
#[derive(Debug, Default)]
pub struct Workspace {
    pub agents: Vec<Value>,
    pub projects: Vec<Value>,
    pub tasks: Vec<Value>,
    pub board: Option<BoardView>,
    /// The project of the last board request (so the same board is not asked for twice).
    pub board_requested: Option<Option<String>>,
    pub board_view: View,
    pub agents_view: View,
    pub transcripts: HashMap<u64, Transcript>,
    /// The last finished work's outcome of each agent (`done`, `failed`).
    pub outcomes: HashMap<u64, String>,
    pub focus: Pane,
    pub selected: Option<Selection>,
    /// The filter applied to the agents list.
    pub filter: String,
    /// The filter while it is typed (it applies as it is typed).
    pub filtering: Option<String>,
    pub composer: Option<Composer>,
    /// Each chat's context as the hub last answered it, by its owner (the agent's, or the conversation's).
    pub contexts: HashMap<Owner, Context>,
    /// Seconds since each chat's context was asked for, for the chat on screen (absent: never asked).
    pub context_since: HashMap<Owner, f32>,
    /// Whether the breakdown panel is open (`i`).
    pub breakdown: bool,
    /// The model and effort picker (`M`), open until Esc or a save.
    pub picker: Option<Picker>,
}

impl Workspace {
    /// The roster as the hub sent it (`GET /v1/agents`).
    pub fn set_agents(&mut self, value: &Value) {
        self.agents = listed(value, "agents");
    }

    /// Whether a composer, the filter or the picker is taking the keys (the app's own keys wait for it).
    pub fn captures_keys(&self) -> bool {
        self.filtering.is_some() || self.composer.is_some() || self.picker.is_some()
    }

    /// The model the roster lists for an agent, as the picker opens with it.
    fn agent_model(&self, cid: u64) -> Option<String> {
        context_bar::model_of(agent_of(&self.agents, cid).and_then(|a| a.get("model")))
    }

    /// Asks for the context of the chat on screen, once it has not been asked for in the last `EVERY` seconds (or never).
    /// `dt` is the seconds since the last frame.
    pub fn poll(&mut self, dt: f32, shown: Option<Owner>) -> Vec<Effect> {
        let Some(owner) = shown else {
            return Vec::new();
        };
        let elapsed = match self.context_since.get_mut(&owner) {
            Some(since) => {
                *since += dt;
                *since
            }
            None => f32::INFINITY,
        };
        if elapsed < context_bar::EVERY {
            return Vec::new();
        }
        self.context_since.insert(owner.clone(), 0.0);
        vec![Effect::Context(Ask::Fetch(owner))]
    }

    /// The breakdown's `i` and the picker's `M` for the chat on screen (`shown`: its owner and its model), and the keys of
    /// an open picker. None for the other keys, which the app's own keys then take.
    pub fn context_key(
        &mut self,
        key: Key,
        shown: Option<(Owner, Option<String>)>,
    ) -> Option<Vec<Effect>> {
        if self.picker.is_some() {
            return Some(self.picker_key(key));
        }
        match key {
            Key::Char('i') => {
                self.breakdown = !self.breakdown;
                Some(Vec::new())
            }
            Key::Char('M') => Some(match shown {
                Some((owner, model)) => {
                    self.picker = Some(Picker::open(owner, model.as_deref()));
                    vec![Effect::Context(Ask::Models)]
                }
                None => vec![Effect::Notice("no chat to pick a model for".to_string())],
            }),
            _ => None,
        }
    }

    fn picker_key(&mut self, key: Key) -> Vec<Effect> {
        let Some(picker) = self.picker.as_mut() else {
            return Vec::new();
        };
        match picker.key(key) {
            PickerStroke::Keep => Vec::new(),
            PickerStroke::Close => {
                self.picker = None;
                Vec::new()
            }
            PickerStroke::Save { owner, model } => {
                vec![Effect::Context(Ask::Save { owner, model })]
            }
        }
    }

    fn active_filter(&self) -> &str {
        self.filtering.as_deref().unwrap_or(&self.filter)
    }

    /// The rows of the agents pane now.
    pub fn rows(&self) -> Vec<Row> {
        rows(&groups(
            &self.agents,
            &self.projects,
            &self.tasks,
            self.active_filter(),
        ))
    }

    fn selectable(&self) -> Vec<Selection> {
        self.rows()
            .into_iter()
            .filter_map(|row| match row {
                Row::Agent { cid, project } => Some(Selection { cid, project }),
                Row::Header { .. } => None,
            })
            .collect()
    }

    pub fn selected_cid(&self) -> Option<u64> {
        self.selected.as_ref().map(|s| s.cid)
    }

    /// The project of the selected agent's group: whose board the pane shows.
    pub fn want_project(&self) -> Option<String> {
        self.selected.as_ref().and_then(|s| s.project.clone())
    }

    pub fn board_paused(&self) -> bool {
        self.board
            .as_ref()
            .filter(|b| b.project == self.want_project())
            .and_then(|b| b.value.get("paused"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// Keeps the selection on a row that is listed: the same agent in the same group, else the agent in another group,
    /// else the first agent. Asks for what the selection needs (its transcript, the board of its project).
    pub fn sync(&mut self) -> Vec<Effect> {
        let choices = self.selectable();
        let previous = self.selected.clone();
        let next = previous
            .clone()
            .filter(|s| choices.contains(s))
            .or_else(|| {
                previous
                    .as_ref()
                    .and_then(|p| choices.iter().find(|c| c.cid == p.cid).cloned())
            })
            .or_else(|| choices.first().cloned());
        let moved = next.as_ref().map(|s| s.cid) != previous.as_ref().map(|s| s.cid);
        self.selected = next;
        let mut effects = Vec::new();
        if let Some(cid) = self.selected_cid() {
            let transcript = self.transcripts.entry(cid).or_default();
            if !transcript.loaded {
                if !transcript.requested {
                    transcript.requested = true;
                    effects.push(Effect::Fetch(Want::Transcript { cid, after: None }));
                }
            } else if moved {
                effects.push(Effect::Fetch(Want::Transcript {
                    cid,
                    after: Some(transcript.last_seq()),
                }));
            }
        }
        if self.selected.is_some() {
            let want = self.want_project();
            let shown = self.board.as_ref().map(|b| b.project.clone());
            if shown != Some(want.clone()) && self.board_requested != Some(want.clone()) {
                self.board_requested = Some(want.clone());
                effects.push(Effect::Fetch(Want::Board(want)));
            }
        }
        effects
    }

    /// What a fresh look at the tab asks for: the projects, the tasks, the board, and the newer entries of the selected
    /// transcript.
    pub fn enter(&mut self) -> Vec<Effect> {
        let mut effects = vec![Effect::Fetch(Want::Projects), Effect::Fetch(Want::Tasks)];
        effects.extend(self.refresh_board());
        if let Some(cid) = self.selected_cid()
            && let Some(transcript) = self.transcripts.get(&cid).filter(|t| t.loaded)
        {
            effects.push(Effect::Fetch(Want::Transcript {
                cid,
                after: Some(transcript.last_seq()),
            }));
        }
        effects.extend(self.sync());
        effects
    }

    /// Asks for the board of the selected agent's project again (after a post, a pause, or a board event).
    pub fn refresh_board(&mut self) -> Vec<Effect> {
        let want = self.want_project();
        self.board_requested = Some(want.clone());
        vec![Effect::Fetch(Want::Board(want))]
    }

    /// Takes an answer from the hub. A failed answer becomes a notice, and its request may be made again later.
    pub fn receive(&mut self, got: Got) -> Vec<Effect> {
        match got {
            Got::Projects(Ok(value)) => self.projects = listed(&value, "projects"),
            Got::Tasks(Ok(value)) => self.tasks = listed(&value, "tasks"),
            Got::Board {
                project,
                result: Ok(value),
            } => {
                if self.board_requested.as_ref() == Some(&project) {
                    self.board = Some(BoardView { project, value });
                }
            }
            Got::Transcript {
                cid,
                result: Ok(value),
            } => {
                let transcript = self.transcripts.entry(cid).or_default();
                transcript.requested = false;
                transcript.loaded = true;
                for entry in value.as_array().into_iter().flatten() {
                    transcript.merge(entry.clone());
                }
                if let Some(outcome) = outcome_of(&transcript.entries) {
                    self.outcomes.insert(cid, outcome);
                }
            }
            Got::Context {
                owner,
                result: Ok(value),
            } => match Context::parse(&value) {
                Some(context) => {
                    self.contexts.insert(owner, context);
                }
                None => {
                    return vec![Effect::Notice(
                        "context: the hub's answer has no used".to_string(),
                    )];
                }
            },
            Got::Context {
                result: Err(error), ..
            } => return vec![Effect::Notice(format!("context: {error}"))],
            Got::Models(Ok(value)) => {
                if let Some(picker) = self.picker.as_mut() {
                    picker.load(&value);
                }
            }
            Got::Models(Err(error)) => {
                if let Some(picker) = self.picker.as_mut() {
                    picker.fail(&error);
                }
            }
            Got::Saved {
                owner,
                result: Ok(_),
            } => {
                let mut effects = vec![Effect::Notice("model saved".to_string())];
                if self.picker.as_ref().is_some_and(|p| p.owner == owner) {
                    self.picker = None;
                }
                self.context_since.insert(owner.clone(), 0.0);
                effects.push(Effect::Context(Ask::Fetch(owner)));
                return effects;
            }
            Got::Saved {
                owner,
                result: Err(error),
            } => match self.picker.as_mut().filter(|p| p.owner == owner) {
                Some(picker) => picker.note = Some(error),
                None => return vec![Effect::Notice(format!("model: {error}"))],
            },
            Got::Projects(Err(error)) => return vec![Effect::Notice(format!("projects: {error}"))],
            Got::Tasks(Err(error)) => return vec![Effect::Notice(format!("tasks: {error}"))],
            Got::Board {
                result: Err(error), ..
            } => return vec![Effect::Notice(format!("board: {error}"))],
            Got::Transcript {
                cid,
                result: Err(error),
            } => {
                if let Some(transcript) = self.transcripts.get_mut(&cid) {
                    transcript.requested = false;
                }
                return vec![Effect::Notice(format!("transcript: {error}"))];
            }
        }
        Vec::new()
    }

    /// What a hub event changes: a transcript entry goes into its place (and a gap asks for what was missed), a finished
    /// work sets its agent's outcome, and the lists the event names are asked for again.
    pub fn on_event(&mut self, kind: &str, data: &Value) -> Vec<Effect> {
        match kind {
            "transcript" => {
                let Some(cid) = data.get("cid").and_then(Value::as_u64) else {
                    return Vec::new();
                };
                let Some(transcript) = self.transcripts.get_mut(&cid) else {
                    return Vec::new();
                };
                let before = transcript.last_seq();
                let loaded = transcript.loaded;
                let gap = transcript.merge(data.clone());
                if gap && loaded && self.selected_cid() == Some(cid) {
                    return vec![Effect::Fetch(Want::Transcript {
                        cid,
                        after: Some(before),
                    })];
                }
                Vec::new()
            }
            "work" => {
                if data.get("state").and_then(Value::as_str) == Some("ended")
                    && let (Some(cid), Some(status)) = (
                        data.get("cid").and_then(Value::as_u64),
                        str_of(data, "status"),
                    )
                {
                    self.outcomes.insert(cid, status.to_string());
                }
                vec![Effect::Agents]
            }
            "agent" => vec![Effect::Agents],
            "context" => {
                // The events come once a second an agent at most: each chat of that agent (or of the orchestrator it
                // is) is read again.
                let Some(cid) = data.get("cid").and_then(Value::as_u64) else {
                    return Vec::new();
                };
                let owners: Vec<Owner> = self
                    .contexts
                    .iter()
                    .filter(|(owner, context)| match owner {
                        Owner::Agent(own) => *own == cid,
                        Owner::Conversation(_) => context.cid == cid,
                    })
                    .map(|(owner, _)| owner.clone())
                    .collect();
                owners
                    .into_iter()
                    .map(|owner| {
                        self.context_since.insert(owner.clone(), 0.0);
                        Effect::Context(Ask::Fetch(owner))
                    })
                    .collect()
            }
            "project" => vec![Effect::Fetch(Want::Projects)],
            "task" => vec![Effect::Fetch(Want::Tasks)],
            "board" | "board_paused" => self.refresh_board(),
            _ => Vec::new(),
        }
    }

    /// One key on the tab. None when the key is not the tab's: the app's own keys then act (the tabs, quit, the stop switch).
    pub fn key(&mut self, key: Key) -> Option<Vec<Effect>> {
        if self.filtering.is_some() {
            return Some(self.filter_key(key));
        }
        if let Some(composer) = self.composer.as_mut() {
            let stroke = composer.key(key);
            return Some(self.stroke(stroke));
        }
        if self.picker.is_some() || matches!(key, Key::Char('i' | 'M')) {
            let shown = self
                .selected_cid()
                .map(|cid| (Owner::Agent(cid), self.agent_model(cid)));
            return self.context_key(key, shown);
        }
        let effects = match key {
            Key::Tab => {
                self.focus = self.focus.next();
                Vec::new()
            }
            Key::BackTab => {
                self.focus = self.focus.prev();
                Vec::new()
            }
            Key::Char('/') => {
                self.focus = Pane::Agents;
                self.filtering = Some(self.filter.clone());
                self.sync()
            }
            Key::Char('t') => match self.selected_cid() {
                Some(cid) => {
                    self.composer = Some(Composer::new(Target::Steer(cid)));
                    Vec::new()
                }
                None => vec![Effect::Notice("no agent to write to".to_string())],
            },
            Key::Char('p') => {
                self.composer = Some(Composer::new(Target::Board(self.want_project())));
                Vec::new()
            }
            Key::Char('P') => {
                let on = !self.board_paused();
                vec![Effect::Pause {
                    project: self.want_project(),
                    on,
                }]
            }
            Key::Char('E') => vec![Effect::Export {
                project: self.want_project(),
            }],
            Key::Char('o') => match self.selected_cid() {
                Some(cid) => vec![Effect::Notice(format!(
                    "open the IDE with: agentplaza ide ls agent:{cid}"
                ))],
                None => Vec::new(),
            },
            Key::Up | Key::Char('k') => self.step(-1),
            Key::Down | Key::Char('j') => self.step(1),
            Key::PageUp => self.page(-1),
            Key::PageDown => self.page(1),
            Key::End => self.newest(),
            Key::Enter => self.enter_pane(),
            _ => return None,
        };
        Some(effects)
    }

    fn stroke(&mut self, stroke: Stroke) -> Vec<Effect> {
        match stroke {
            Stroke::Keep => Vec::new(),
            Stroke::Close => {
                self.composer = None;
                Vec::new()
            }
            Stroke::Send(target, text) => {
                self.composer = None;
                vec![match target {
                    Target::Steer(cid) => Effect::Steer { cid, text },
                    Target::Board(project) => Effect::Post {
                        project,
                        body: text,
                    },
                }]
            }
            Stroke::AskStop(cid) => vec![Effect::AskStop(cid)],
        }
    }

    fn filter_key(&mut self, key: Key) -> Vec<Effect> {
        match key {
            Key::Up => return self.step(-1),
            Key::Down => return self.step(1),
            Key::Enter => {
                self.filter = self.filtering.take().unwrap_or_default().trim().to_string()
            }
            Key::Esc | Key::CtrlC => {
                self.filtering = None;
                self.filter.clear();
            }
            Key::Backspace => {
                if let Some(text) = self.filtering.as_mut() {
                    text.pop();
                }
            }
            Key::Char(c) if !c.is_control() => {
                if let Some(text) = self.filtering.as_mut() {
                    text.push(c);
                }
            }
            _ => {}
        }
        self.sync()
    }

    /// Moves within the focused pane: the agents list's selection, the chat's cursor, or the board's scroll.
    fn step(&mut self, delta: isize) -> Vec<Effect> {
        match self.focus {
            Pane::Agents => {
                let choices = self.selectable();
                let Some(index) = choices
                    .iter()
                    .position(|c| self.selected.as_ref() == Some(c))
                else {
                    return self.sync();
                };
                let next = (index as isize + delta).clamp(0, choices.len() as isize - 1) as usize;
                self.selected = Some(choices[next].clone());
                self.sync()
            }
            Pane::Chat => {
                if let Some(cid) = self.selected_cid() {
                    self.transcripts.entry(cid).or_default().move_cursor(delta);
                }
                Vec::new()
            }
            Pane::Board => {
                self.board_view.scroll(delta);
                Vec::new()
            }
        }
    }

    fn page(&mut self, pages: isize) -> Vec<Effect> {
        match self.focus {
            Pane::Agents => self.step(pages * PAGE_ROWS),
            Pane::Chat => {
                if let Some(cid) = self.selected_cid() {
                    let view = &mut self.transcripts.entry(cid).or_default().view;
                    let by = view.page() * pages;
                    view.scroll(by);
                }
                Vec::new()
            }
            Pane::Board => {
                let by = self.board_view.page() * pages;
                self.board_view.scroll(by);
                Vec::new()
            }
        }
    }

    fn newest(&mut self) -> Vec<Effect> {
        match self.focus {
            Pane::Agents => return self.step(FAR),
            Pane::Chat => {
                if let Some(cid) = self.selected_cid() {
                    self.transcripts.entry(cid).or_default().follow_newest();
                }
            }
            Pane::Board => self.board_view.end(),
        }
        Vec::new()
    }

    fn enter_pane(&mut self) -> Vec<Effect> {
        match self.focus {
            Pane::Agents => self.focus = Pane::Chat,
            Pane::Chat => {
                if let Some(cid) = self.selected_cid() {
                    self.transcripts.entry(cid).or_default().toggle();
                }
            }
            Pane::Board => {}
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn agent(cid: u64, status: &str, task: Option<(&str, Option<&str>)>) -> Value {
        let work = match task {
            Some((prompt, origin)) => {
                json!({"kind": "chat", "prompt": prompt, "origin": {"task": origin}})
            }
            None => Value::Null,
        };
        json!({"cid": cid, "role": "dev", "role_label": "Developer", "status": status, "work": work})
    }

    fn entry(seq: u64, kind: &str, data: Value) -> Value {
        json!({"seq": seq, "cid": 1, "work": 1, "kind": kind, "data": data})
    }

    #[test]
    fn a_transcript_keeps_seq_order_skips_repeats_and_reports_gaps() {
        let mut t = Transcript::default();
        assert!(!t.merge(entry(2, "work_start", json!({"prompt": "b"}))));
        assert!(!t.merge(entry(3, "work_start", json!({"prompt": "c"}))));
        assert!(
            !t.merge(entry(3, "work_start", json!({"prompt": "c"}))),
            "a repeat is skipped"
        );
        assert!(
            !t.merge(entry(1, "work_start", json!({"prompt": "a"}))),
            "a late older entry is placed first"
        );
        let seqs: Vec<u64> = t.entries.iter().map(seq_of).collect();
        assert_eq!(seqs, vec![1, 2, 3]);
        assert!(
            t.merge(entry(6, "work_start", json!({"prompt": "f"}))),
            "4 and 5 were missed"
        );
        assert_eq!(t.last_seq(), 6);
    }

    #[test]
    fn an_agent_on_a_project_sits_under_it_alone_and_the_rest_under_what_they_lead_or_hold() {
        let mut ada = agent(1, "idle", None);
        ada["project"] = json!("pr2");
        let lead = agent(9, "idle", None);
        let projects = vec![
            json!({"id": "pr1", "title": "Docs", "lead_cid": 1, "tasks": ["t1"], "updated_ms": 2}),
            json!({"id": "pr2", "title": "Release", "lead_cid": 9, "tasks": [], "updated_ms": 1}),
        ];
        let tasks = vec![json!({"id": "t1", "agent_cid": 1})];
        let g = groups(&[ada, lead], &projects, &tasks, "");
        let titles: Vec<(&str, Vec<u64>)> = g
            .iter()
            .map(|g| (g.title.as_str(), g.cids.clone()))
            .collect();
        assert_eq!(
            titles,
            vec![("Release", vec![1, 9])],
            "Ada works on pr2 though she leads pr1 and holds its task; the orchestrator is under what it leads"
        );
    }

    #[test]
    fn the_composer_sends_trimmed_text_and_an_empty_one_waits() {
        let mut composer = Composer::new(Target::Steer(7));
        assert_eq!(composer.key(Key::Enter), Stroke::Keep);
        for c in " hi ".chars() {
            composer.key(Key::Char(c));
        }
        assert_eq!(
            composer.key(Key::Enter),
            Stroke::Send(Target::Steer(7), "hi".to_string())
        );
    }

    #[test]
    fn selecting_an_agent_asks_for_its_transcript_once_and_its_projects_board() {
        let mut ws = Workspace {
            agents: vec![agent(1, "idle", None), agent(2, "idle", None)],
            ..Workspace::default()
        };
        let first = ws.sync();
        assert!(first.contains(&Effect::Fetch(Want::Transcript {
            cid: 1,
            after: None
        })));
        assert!(first.contains(&Effect::Fetch(Want::Board(None))));
        assert!(ws.sync().is_empty(), "nothing new to ask for");
        let moved = ws.key(Key::Down).unwrap_or_default();
        assert_eq!(ws.selected_cid(), Some(2));
        assert!(moved.contains(&Effect::Fetch(Want::Transcript {
            cid: 2,
            after: None
        })));
    }

    #[test]
    fn a_loaded_transcript_sets_the_agents_last_outcome() {
        let mut ws = Workspace::default();
        let notice = ws.receive(Got::Transcript {
            cid: 1,
            result: Ok(json!([
                entry(1, "work_start", json!({"prompt": "go"})),
                entry(2, "work_end", json!({"status": "failed"}))
            ])),
        });
        assert!(notice.is_empty());
        assert_eq!(ws.outcomes.get(&1).map(String::as_str), Some("failed"));
        assert!(ws.transcripts[&1].loaded);
    }

    #[test]
    fn a_transcript_event_past_a_gap_asks_for_what_was_missed() {
        let mut ws = Workspace {
            agents: vec![agent(1, "idle", None)],
            ..Workspace::default()
        };
        ws.sync();
        ws.receive(Got::Transcript {
            cid: 1,
            result: Ok(json!([
                entry(1, "work_start", json!({"prompt": "a"})),
                entry(2, "work_start", json!({"prompt": "b"}))
            ])),
        });
        let event =
            json!({"seq": 5, "cid": 1, "work": 1, "kind": "work_start", "data": {"prompt": "e"}});
        let effects = ws.on_event("transcript", &event);
        assert_eq!(
            effects,
            vec![Effect::Fetch(Want::Transcript {
                cid: 1,
                after: Some(2)
            })]
        );
    }

    #[test]
    fn a_finished_work_event_sets_the_outcome_and_asks_for_the_roster() {
        let mut ws = Workspace::default();
        let effects = ws.on_event(
            "work",
            &json!({"cid": 3, "work": 9, "state": "ended", "status": "done"}),
        );
        assert!(effects.contains(&Effect::Agents));
        assert_eq!(ws.outcomes.get(&3).map(String::as_str), Some("done"));
        assert_eq!(
            ws.on_event("board", &json!({})),
            vec![Effect::Fetch(Want::Board(None))]
        );
    }

    fn model_listing() -> Value {
        json!({"models": [
            {"provider": "anthropic", "id": "claude-a", "favourite": true, "context": 200000},
            {"provider": "openai", "id": "gpt-x", "favourite": false, "context": 128000},
        ]})
    }

    fn context_answer(cid: u64, used: u64) -> Value {
        json!({"cid": cid, "work": 9, "model": "anthropic/claude-a", "window": 200000, "used": used,
            "parts": [{"part": "system", "tokens": used}], "tools": [], "calls": 1, "compactions": 0})
    }

    #[test]
    fn i_opens_the_breakdown_and_t_writes_the_steer_note() {
        let mut ws = Workspace {
            agents: vec![agent(1, "idle", None)],
            ..Workspace::default()
        };
        ws.sync();
        assert_eq!(ws.key(Key::Char('i')), Some(Vec::new()));
        assert!(ws.breakdown);
        ws.key(Key::Char('i'));
        assert!(!ws.breakdown, "i toggles it");
        ws.key(Key::Char('t'));
        assert_eq!(
            ws.composer.as_ref().map(|c| c.target.clone()),
            Some(Target::Steer(1))
        );
    }

    #[test]
    fn m_opens_the_picker_asks_for_the_models_and_a_save_closes_it() {
        let mut ws = Workspace {
            agents: vec![agent(1, "idle", None)],
            ..Workspace::default()
        };
        ws.sync();
        let effects = ws.key(Key::Char('M')).unwrap_or_default();
        assert_eq!(effects, vec![Effect::Context(Ask::Models)]);
        assert!(ws.captures_keys(), "the picker takes the keys");
        assert_eq!(
            ws.key(Key::Char('q')),
            Some(Vec::new()),
            "q does not quit under the picker"
        );
        let got = ws.receive(Got::Models(Ok(model_listing())));
        assert!(got.is_empty());
        ws.key(Key::Down);
        let save = ws.key(Key::Enter).unwrap_or_default();
        assert_eq!(
            save,
            vec![Effect::Context(Ask::Save {
                owner: Owner::Agent(1),
                model: Some("anthropic/claude-a".into()),
            })]
        );
        let done = ws.receive(Got::Saved {
            owner: Owner::Agent(1),
            result: Ok(json!({})),
        });
        assert!(ws.picker.is_none(), "a saved choice closes the picker");
        assert_eq!(done[0], Effect::Notice("model saved".to_string()));
        assert_eq!(done[1], Effect::Context(Ask::Fetch(Owner::Agent(1))));
    }

    #[test]
    fn a_refused_model_keeps_the_picker_open_with_the_hubs_message() {
        let mut ws = Workspace {
            agents: vec![agent(1, "idle", None)],
            ..Workspace::default()
        };
        ws.sync();
        ws.key(Key::Char('M'));
        ws.receive(Got::Models(Ok(model_listing())));
        ws.receive(Got::Saved {
            owner: Owner::Agent(1),
            result: Err("the tier policy does not allow openai/gpt-x".into()),
        });
        let picker = ws.picker.as_ref().expect("still open");
        assert_eq!(
            picker.note.as_deref(),
            Some("the tier policy does not allow openai/gpt-x")
        );
    }

    #[test]
    fn a_failed_model_list_offers_the_default_and_says_why() {
        let mut ws = Workspace {
            agents: vec![agent(1, "idle", None)],
            ..Workspace::default()
        };
        ws.sync();
        ws.key(Key::Char('M'));
        ws.receive(Got::Models(Err("HTTP 503".into())));
        let picker = ws.picker.as_ref().unwrap();
        assert_eq!(picker.chosen(), Some(None));
        assert_eq!(picker.note.as_deref(), Some("models: HTTP 503"));
    }

    #[test]
    fn a_context_event_asks_for_the_chats_of_its_agent_and_the_context_is_kept() {
        let mut ws = Workspace::default();
        assert!(
            ws.receive(Got::Context {
                owner: Owner::Agent(1),
                result: Ok(context_answer(1, 41_200))
            })
            .is_empty()
        );
        assert_eq!(ws.contexts[&Owner::Agent(1)].used, 41_200);
        let effects = ws.on_event(
            "context",
            &json!({"cid": 1, "work": 9, "used": 1, "window": 200000}),
        );
        assert_eq!(effects, vec![Effect::Context(Ask::Fetch(Owner::Agent(1)))]);
        assert!(
            ws.on_event("context", &json!({"cid": 2})).is_empty(),
            "another agent's"
        );
        let failed = ws.receive(Got::Context {
            owner: Owner::Agent(1),
            result: Err("HTTP 404".into()),
        });
        assert_eq!(
            failed,
            vec![Effect::Notice("context: HTTP 404".to_string())]
        );
    }

    #[test]
    fn the_shown_chats_context_is_asked_for_at_once_and_then_every_ten_seconds() {
        let mut ws = Workspace::default();
        let shown = Some(Owner::Agent(1));
        assert_eq!(
            ws.poll(0.5, shown.clone()),
            vec![Effect::Context(Ask::Fetch(Owner::Agent(1)))],
            "a chat never asked for is asked for now"
        );
        assert!(ws.poll(5.0, shown.clone()).is_empty());
        assert!(
            ws.poll(4.0, shown.clone()).is_empty(),
            "nine and a half seconds is not ten"
        );
        assert_eq!(
            ws.poll(1.0, shown.clone()),
            vec![Effect::Context(Ask::Fetch(Owner::Agent(1)))]
        );
        assert!(
            ws.poll(30.0, None).is_empty(),
            "nothing on screen, nothing asked"
        );
    }

    #[test]
    fn a_chat_with_no_picker_open_leaves_the_other_keys_to_the_app() {
        let mut ws = Workspace::default();
        assert_eq!(ws.context_key(Key::Char('x'), None), None);
        assert_eq!(
            ws.context_key(Key::Char('M'), None),
            Some(vec![Effect::Notice(
                "no chat to pick a model for".to_string()
            )])
        );
    }
}
