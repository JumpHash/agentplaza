//! The IDE screen (`Id::Ide`): one workspace at a time, an agent's `/workspace` or a project repo as
//! the hub serves it. This module holds the state, the hub calls and the reducer; the tree, the
//! editor, the diff and the search are the sibling modules' views. Shared rules (paths, workspace
//! ids, the diff parser, the tab and base reducer) come from `apz-ide`, the same code the web runs.
//!
//! Saves send the base hash, so the hub refuses a write over a newer one (409): the tab then shows a
//! conflict, and the owner reloads or keeps theirs. Changes come from the hub as a unified diff.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::sync::LazyLock;

use apz_ide::state::{self, Action, Base, IdeState, Panel};
use apz_ide::{Buffer, ChangeSet, EntryKind, TreePage, WorkspaceId, path as ide_path};
use iced::event::Status;
use iced::keyboard::{self, Key, Modifiers, key::Named};
use iced::widget::{Space, Stack, column, container, row, text_editor};
use iced::{Alignment, Element, Length, Padding, Subscription, Task, mouse};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Id;
use super::board;
use super::ide_chat::{self, ChatMsg};
use super::ide_chrome::{self, Drag, Edge, Layout};
use super::ide_conversation;
use super::ide_editor;
use super::ide_files;
use super::ide_model::{self, AgentRow, ProjectRow, TaskRow};
use super::ide_palette;
use super::ide_panels;
use super::ide_right;
use super::ide_sidebar;
use super::ide_terminal::{self, KeptTabs, TabsMsg, TerminalTabs};
use super::ide_tree::{self, Tree};
use crate::console::Ctx;
use crate::console::boot;
use crate::console::client::{self, Client};
use crate::console::events;
use crate::console::rich;
use crate::console::shell;
use crate::console::ui::{self, Kind, Tone};

/// The setting that keeps the last workspace the IDE opened (the console's Ctrl+Shift+E opens it).
pub(super) const REMEMBERED: &str = "ide_workspace";

/// The quick-open box and the search box: focused when they open.
pub(super) static QUICK_ID: LazyLock<iced::widget::Id> = LazyLock::new(iced::widget::Id::unique);
pub(super) static SEARCH_ID: LazyLock<iced::widget::Id> = LazyLock::new(iced::widget::Id::unique);

// ---- The hub's shapes (`/v1/ide/...`) ---------------------------------------------------------

/// A workspace as the hub lists it (`GET /v1/ide/workspaces`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceInfo {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub writable: bool,
    #[serde(default)]
    pub status: String,
    /// A workstation's OS (`macos`, `windows`, `linux`); absent for the other kinds.
    #[serde(default)]
    pub os: Option<String>,
}

/// A checkpoint of a workspace (`GET .../checkpoints`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Checkpoint {
    pub n: u64,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub created_ms: u64,
    #[serde(default)]
    pub files: u64,
}

/// A file as the hub reads it (`GET .../file`). `text` is `None` for a file that is not text.
#[derive(Debug, Clone, Deserialize)]
pub struct FileView {
    #[serde(default)]
    pub hash: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
}

/// The hub's answer to an accepted save (`PUT .../file`).
#[derive(Debug, Clone, Deserialize)]
pub struct Saved {
    pub hash: String,
}

/// Why a save did not go through. `Conflict` carries the hub's hash now (`None`: the file is gone).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError {
    Conflict(Option<String>),
    Failed(String),
}

/// One search hit (`GET .../search`).
#[derive(Debug, Clone, Deserialize)]
pub struct Match {
    pub path: String,
    pub line: usize,
    pub text: String,
}

/// The hub's search answer: its hits, and whether it stopped short.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchReply {
    pub matches: Vec<Match>,
    #[serde(default)]
    pub truncated: bool,
}

// ---- The screen's own values -----------------------------------------------------------------

/// A workspace in the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    pub label: String,
}

impl std::fmt::Display for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// The Changes base the segmented control offers: work start, or the latest checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseChoice {
    WorkStart,
    Checkpoint,
}

/// The hub's file moved on under an open tab. `server_hash` is its hash now (`None`: the file is gone).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub server_hash: Option<String>,
}

/// What a remote change means for one open file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remote {
    Same,
    Reload,
    Conflict,
}

/// One open file: its buffer (text, base hash, dirty), the editor's content, and a conflict when the hub moved on.
#[derive(Debug)]
pub struct Tab {
    pub buffer: Buffer,
    pub content: text_editor::Content,
    pub conflict: Option<Conflict>,
}

impl Tab {
    pub fn new(path: &str, hash: Option<String>, text: &str) -> Tab {
        Tab {
            buffer: Buffer::new(path, hash, text),
            content: text_editor::Content::with_text(text),
            conflict: None,
        }
    }

    /// Applies an editor action. An edit marks the buffer dirty and keeps its text in step; returns whether it was an edit.
    pub fn edit(&mut self, action: text_editor::Action) -> bool {
        let edit = action.is_edit();
        self.content.perform(action);
        if edit {
            self.buffer.edit(self.content.text());
        }
        edit
    }

    /// A save the hub accepted. `sent` is the text that went: if the owner typed meanwhile, the tab stays dirty.
    pub fn saved(&mut self, sent: &str, hash: String) {
        if self.buffer.text == sent {
            self.buffer.saved(hash);
        } else {
            self.buffer.base_hash = Some(hash);
        }
        self.conflict = None;
    }

    /// A 409: the hub's file has a newer write. The edits stay; nothing is overwritten.
    pub fn conflicted(&mut self, server_hash: Option<String>) {
        self.conflict = Some(Conflict { server_hash });
    }

    /// "Keep mine": the buffer adopts the hub's hash as its base, so the next save replaces the hub's file.
    pub fn keep_mine(&mut self) {
        if let Some(conflict) = self.conflict.take() {
            self.buffer.base_hash = conflict.server_hash;
        }
    }

    /// "Reload": the hub's text replaces the buffer, which is clean again.
    pub fn reload(&mut self, path: &str, hash: Option<String>, text: &str) {
        *self = Tab::new(path, hash, text);
    }

    /// The file's hash on the hub changed. A clean buffer reloads; an edited one becomes a conflict.
    pub fn remote(&mut self, server_hash: Option<&str>) -> Remote {
        if !self.buffer.is_conflict(server_hash) {
            self.conflict = None;
            Remote::Same
        } else if self.buffer.dirty {
            self.conflicted(server_hash.map(str::to_string));
            Remote::Conflict
        } else {
            Remote::Reload
        }
    }
}

/// A search in progress or answered.
#[derive(Debug, Clone, Default)]
pub struct Search {
    pub query: String,
    pub glob: String,
    pub results: Option<SearchReply>,
    pub error: Option<String>,
    pub busy: bool,
}

/// The centre's view of the selected agent (or workspace): its chat, its terminal, the editor, or the diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Centre {
    #[default]
    Chat,
    Terminal,
    Editor,
    Diff,
}

/// The right panel's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RightTab {
    #[default]
    Board,
    Files,
    Tasks,
    Approvals,
    Activity,
}

/// The terminal drawer's tabs: the terminal, or the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Drawer {
    #[default]
    Terminal,
    Run,
}

/// The roster the sidebar, the chat and the palette read: the agents, the projects, the tasks and the workstations'
/// accounts, as the hub lists them.
#[derive(Debug, Clone, Default)]
pub struct Roster {
    pub agents: Vec<AgentRow>,
    pub projects: Vec<ProjectRow>,
    pub tasks: Vec<TaskRow>,
    pub stations: Vec<ide_model::StationRow>,
}

/// What the console knows of the plaza, for the IDE's chrome: whether it is held (STOP ALL) and whether
/// the events link is up. The console passes it in; the IDE reads nothing else from the shell.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Plaza {
    pub held: bool,
    pub live: bool,
}

/// The IDE's state for its one screen.
#[derive(Debug, Default)]
pub struct State {
    pub workspaces: Vec<WorkspaceInfo>,
    /// The workspace list has loaded.
    pub loaded: bool,
    /// The workspace list is being fetched.
    pub listing: bool,
    /// A failure to list the workspaces.
    pub error: Option<String>,
    /// A line for the owner: a save, a refused action, a note from the hub.
    pub notice: Option<String>,
    /// The target the screen's id names (`Id::Ide(Some(..))`, see `Target`), opened once the list is in.
    pub preselect: Option<String>,
    /// The screen the IDE was opened from; Escape goes back there.
    pub origin: Option<Id>,
    /// The shared reducer's state: workspace, open tabs, active tab, panel, base, review, changed-only.
    pub ide: IdeState,
    pub tree: Tree,
    pub tabs: BTreeMap<String, Tab>,
    pub changes: Option<ChangeSet>,
    pub changes_error: Option<String>,
    pub changes_busy: bool,
    pub checkpoints: Vec<Checkpoint>,
    /// The right panel's state beyond Changes (the agent, its approvals, the run).
    pub panels: ide_panels::State,
    /// The chat's scroll: whether the reader is at the newest entry, and the entries that came while they were not.
    pub chat_scroll: ide_chat::Scroll,
    /// The rich content of the transcript: the hub's cards and images for its references (`console::rich`).
    pub cards: rich::Cards,
    /// The checkpoint awaiting a confirmed restore.
    pub restore: Option<u64>,
    /// The last save of this workspace is in and nothing has been edited since.
    pub saved: bool,
    /// Where the editor goes once a file opens (a search hit's line).
    pub goto: Option<(String, usize)>,
    /// Quick open's query, while it is open.
    pub quick: Option<String>,
    pub search: Option<Search>,
    /// The changed file whose diff the centre shows (in Diff, in place of the review), if any.
    pub diff_file: Option<String>,
    /// The centre's view, the right panel's tab, and the drawer's tab.
    pub centre: Centre,
    pub right: RightTab,
    pub drawer_tab: Drawer,
    /// The sidebar's roster, projects and tasks, and whether they have loaded.
    pub roster: Vec<AgentRow>,
    pub projects: Vec<ProjectRow>,
    pub tasks: Vec<TaskRow>,
    /// The workstations' accounts, for the warning on a workstation's row.
    pub stations: Vec<ide_model::StationRow>,
    pub roster_loaded: bool,
    pub roster_error: Option<String>,
    /// The sidebar's search field, and the groups folded away.
    pub sidebar_query: String,
    pub collapsed: BTreeSet<String>,
    /// The board the Board tab shows, its scope (the project's posts, or all), and when it was last read (epoch ms):
    /// posts after that are unread.
    pub board: board::State,
    pub board_project: bool,
    pub board_seen_ms: u64,
    /// The command palette, while it is open.
    pub palette: Option<ide_palette::Palette>,
    /// The runs of tool steps the chat has open, by their first entry.
    pub steps_open: BTreeSet<usize>,
    /// The agent's cancel was asked and not answered yet.
    pub cancelling: bool,
    /// The side panels' sizes and folds, kept between runs.
    pub layout: Layout,
    /// A panel edge being dragged.
    pub drag: Option<Drag>,
    /// The terminal tabs of the agent workspace on screen, made on first need (see `ide_terminal`).
    pub terminals: Option<TerminalTabs>,
    /// The terminal tabs whose socket thread has started since they were made: a second start for one means the
    /// IDE was left and came back (see `terminal_task`).
    pub terminal_links: BTreeSet<usize>,
    /// The pointer is over the terminal.
    pub terminal_hover: bool,
    /// The terminal has the keys: its last press landed on it, so the IDE's shortcuts stay out.
    pub terminal_focus: bool,
    /// The terminal tabs the IDE was left with, to attach to again when it is entered (see `left`).
    pub parked: Option<KeptTabs>,
    /// The IDE was left and not entered again: a run's answers are not read on meanwhile (see `left`).
    pub away: bool,
    /// A poll of the run is on its way to the hub, so the run has a poll asking for its output.
    pub poll_inflight: bool,
    /// The window's width in px (0 until it is known: the wide layout is drawn until then).
    pub width: f32,
    /// In a narrow window: the left sidebar drawn as a drawer over the centre, and the right panel as a sheet over it.
    pub left_drawer: bool,
    pub right_sheet: bool,
    /// The conversation with the orchestrator open in place of a workspace (`conv-<n>`), if any.
    pub conversation: Option<ide_conversation::Conversation>,
    /// The owner's conversations, newest first, as the hub lists them: the sidebar's first section.
    pub conversations: Vec<Value>,
    /// Project mode: the IDE opened under a project (the console's scope) is that project's.
    pub project: Option<String>,
}

/// Below this window width the right panel is a sheet over the centre, toggled from the top bar.
pub const RIGHT_SHEET_BELOW: f32 = 1100.0;
/// Below this window width the left sidebar is a drawer over the centre, toggled from the top bar.
pub const LEFT_DRAWER_BELOW: f32 = 800.0;

impl State {
    /// The window is narrow enough that the right panel is a sheet over the centre.
    pub fn right_is_sheet(&self) -> bool {
        self.width > 0.0 && self.width < RIGHT_SHEET_BELOW
    }

    /// The window is narrow enough that the left sidebar is a drawer over the centre.
    pub fn left_is_drawer(&self) -> bool {
        self.width > 0.0 && self.width < LEFT_DRAWER_BELOW
    }

    /// The widest a sheet or drawer may be: the window less a margin, so the centre still shows at its edge.
    pub fn sheet_width(&self) -> f32 {
        (self.width - 48.0).max(240.0)
    }
}

// The terminal tabs hold a socket and an emulator, which have no Debug of their own; the screen's state derives it.
impl std::fmt::Debug for TerminalTabs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TerminalTabs")
    }
}

/// The IDE's messages. Hub answers carry the workspace they were asked for, so a late answer for a
/// workspace the owner has left is dropped.
#[derive(Debug, Clone)]
pub enum Msg {
    /// Opens the IDE on a workspace id (`agent:<cid>` or `repo:<name>@<ref>`), or on the picker. Navigation: see `go`.
    Open(Option<String>),
    Listed(Result<Vec<WorkspaceInfo>, String>),
    Pick(Choice),
    Dir {
        ws: String,
        dir: String,
        first: bool,
        res: Result<TreePage, String>,
    },
    ToggleDir(String),
    MoreDir(String),
    ChangedOnly,
    /// Lists the workspace again: its tree, changes, checkpoints and open files.
    RefreshAll,
    OpenPath(String),
    /// Shows a changed file's diff in the centre.
    ShowDiff(String),
    /// Puts the editor back in the centre, in place of a diff.
    CloseDiff,
    /// Folds or unfolds a side panel (the header's toggles, Ctrl+B and Ctrl+J). In a narrow window the same toggles
    /// open and close the drawer or the sheet over the centre.
    ToggleLeft,
    ToggleRight,
    /// The window's width, when it changes or is first read: the narrow layouts follow it.
    Resized(f32),
    /// The centre's view, the right panel's tab, and the drawer's tab.
    Centre(Centre),
    Right(RightTab),
    DrawerTab(Drawer),
    /// Folds or unfolds the terminal drawer under the centre (Ctrl+`).
    ToggleDrawer,
    /// The sidebar: its search, a group folded or unfolded, and "+ New agent" (the Agents screen's wizard).
    SidebarQuery(String),
    ToggleGroup(String),
    NewAgent,
    /// "+ New conversation": the shell's New project dialog (the console opens it).
    NewProject,
    /// Opens a conversation with the orchestrator in place of the workspace.
    OpenConversation(String),
    Conversation(ide_conversation::Msg),
    /// The conversations, as the hub listed them.
    Conversations(Result<Value, String>),
    /// The roster, projects and tasks, as the hub answered.
    Roster(Result<Roster, String>),
    /// The Board tab's messages, and its scope (true: the project's posts).
    Board(board::Msg),
    BoardScope(bool),
    /// The chat's messages (see `ide_chat`).
    Chat(ChatMsg),
    /// The command palette: opened, its query typed, an item run by its place in the list, closed, and the
    /// cursor moved (by a step up or down).
    PaletteOpen,
    PaletteQuery(String),
    PaletteRun(usize),
    PaletteClose,
    PaletteMove(i32),
    /// A press on a panel's grab strip starts a drag; the screen follows the pointer until the button comes up.
    DragStart(Edge),
    DragMove(f32, f32),
    DragEnd,
    /// A double press on a grab strip: the panel goes back to its default width.
    ResetWidth(Edge),
    /// The shell's own actions the IDE's chrome offers (the theme switch, the held strip's Resume). The
    /// console hands them to the shell.
    Shell(shell::Msg),
    Opened {
        ws: String,
        path: String,
        res: Result<FileView, String>,
    },
    Activate(String),
    Close(String),
    Edit(String, text_editor::Action),
    Save,
    Saved {
        ws: String,
        path: String,
        sent: String,
        res: Result<Saved, SaveError>,
    },
    Reload(String),
    Reloaded {
        ws: String,
        path: String,
        res: Result<FileView, String>,
    },
    KeepMine(String),
    Remote {
        ws: String,
        path: String,
        res: Result<FileView, String>,
    },
    Changed {
        ws: String,
        base: Base,
        res: Result<ChangeSet, String>,
    },
    PickBase(BaseChoice),
    CompareWith(u64),
    /// The panels' own messages (the chat's composer, the approvals, the run).
    Panels(ide_panels::Msg),
    /// A rich-content click (a chip, a link, an Open button) or a load of its cards and images.
    Rich(rich::Ev),
    Checkpoints {
        ws: String,
        res: Result<Vec<Checkpoint>, String>,
    },
    AskRestore(u64),
    CancelRestore,
    Restore(u64),
    Restored {
        ws: String,
        res: Result<(), String>,
    },
    ToggleQuick,
    QuickQuery(String),
    QuickFirst,
    ToggleSearch,
    SearchQuery(String),
    SearchGlob(String),
    RunSearch,
    Searched {
        ws: String,
        res: Result<SearchReply, String>,
    },
    OpenMatch(String, usize),
    Escape,
    /// Handled by the console: back to the screen the IDE was opened from.
    Back,
    Event(events::Event),
    /// The terminal tabs' messages (see `ide_terminal`).
    Terminals(TabsMsg),
    /// The pointer came into (true) or left the terminal.
    TerminalHover(bool),
    /// A left press anywhere in the window: the terminal keeps the keys only when the press landed on it.
    Pressed,
    /// A key press the window's listener saw, and whether a focused widget took it. The shortcuts are decided
    /// in `update`, where the terminal's focus is known.
    Shortcut {
        key: Key,
        modifiers: Modifiers,
        status: Status,
    },
}

/// The screen's navigation. `Msg::Open` opens the IDE on a workspace (`Some(id)`) or on the picker (`None`);
/// other screens return it to open the IDE. The IDE leaves through `Back`.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Open(workspace) => Some(Id::Ide(workspace.clone())),
        // "+ New agent" opens the Agents screen, whose wizard spawns one.
        Msg::NewAgent => Some(Id::Agents),
        Msg::Rich(rich::Ev::Action(action)) => action.route(),
        // A link on a board post in the panel navigates as it does on the board screen.
        Msg::Board(board::Msg::Rich(rich::Ev::Action(action))) => action.route(),
        _ => None,
    }
}

/// Asks the hub for the cards of the transcript's references that it does not hold yet.
pub(super) fn prefetch_transcript(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    let texts = ide_chat::transcript_texts(&st.panels.entries);
    let rctx = ide_chat::rich_ctx(st);
    rich::prefetch(&mut st.cards, &ctx.client, &rctx, &texts).map(Msg::Rich)
}

/// Where the IDE's Back (its Console button, and Escape with nothing open) goes: the screen it was opened
/// from. No origin, or an origin that is the IDE itself, goes to the Overview.
pub fn back_target(origin: Option<Id>) -> Id {
    match origin {
        Some(Id::Ide(_)) | None => Id::Overview,
        Some(id) => id,
    }
}

// ---- The hub calls ----------------------------------------------------------------------------

/// Percent-encodes everything but the unreserved characters (for a path segment or a query value).
pub fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A hub route under one workspace: `/v1/ide/workspaces/{id}{rest}`, the id percent-encoded.
pub(super) fn route(ws: &WorkspaceId, rest: &str) -> String {
    format!("/v1/ide/workspaces/{}{rest}", enc(&ws.to_string()))
}

/// The roster the sidebar, the chat and the palette read: the agents, the projects and the tasks. A list the hub
/// does not answer as a list is an error; an entry that does not read is left out.
pub(super) fn fetch_roster(client: &Client) -> Result<Roster, String> {
    let agents = client.get::<Value>("/v1/agents")?;
    let projects = client.get::<Value>("/v1/projects")?;
    let tasks = client.get::<Value>("/v1/tasks")?;
    // The workstations' accounts only decorate their rows: a failed list leaves the rows without the warning.
    let stations = client.get::<Value>("/v1/stations").unwrap_or(Value::Null);
    let list = |v: &Value, key: Option<&str>| -> Vec<Value> {
        let items = match key {
            Some(key) => v.get(key).cloned().unwrap_or(Value::Null),
            None => v.clone(),
        };
        items.as_array().cloned().unwrap_or_default()
    };
    Ok(Roster {
        agents: list(&agents, None)
            .iter()
            .filter_map(ide_model::parse_agent)
            .collect(),
        projects: list(&projects, None)
            .iter()
            .filter_map(ide_model::parse_project)
            .collect(),
        tasks: list(&tasks, Some("tasks"))
            .iter()
            .filter_map(ide_model::parse_task)
            .collect(),
        stations: list(&stations, None)
            .iter()
            .filter_map(ide_model::parse_station)
            .collect(),
    })
}

// ---- What each workspace kind has --------------------------------------------------------------
// The same rules the web IDE keeps (ide/layout.ts). A workstation has no diff (so no Diff view, no changed-only filter and
// no checkpoints), no chat, and no project (so no Board, Tasks, Approvals or Activity): its files, its terminal and its runs
// are its own. An agent's and a repository's views are unchanged.

/// Whether a workspace has a diff: the Diff view, the changed-only filter and the checkpoints. Only a workstation has none.
pub(super) fn has_diff(ws: Option<&WorkspaceId>) -> bool {
    !matches!(ws, Some(WorkspaceId::Station(_)))
}

/// Whether a workspace has a shell: its terminal and its runs. An agent's and a workstation's do.
pub(super) fn has_shell(ws: Option<&WorkspaceId>) -> bool {
    matches!(ws, Some(WorkspaceId::Agent(_) | WorkspaceId::Station(_)))
}

/// The centre's views a workspace has: a workstation's terminal and editor; every other kind's four (no workspace
/// yet included, as before).
pub(super) fn centre_views(ws: Option<&WorkspaceId>) -> Vec<Centre> {
    match ws {
        Some(WorkspaceId::Station(_)) => vec![Centre::Terminal, Centre::Editor],
        _ => vec![Centre::Chat, Centre::Terminal, Centre::Editor, Centre::Diff],
    }
}

/// The right panel's tabs a workspace has: a workstation's files; every other kind's five.
pub(super) fn right_tabs(ws: Option<&WorkspaceId>) -> Vec<RightTab> {
    match ws {
        Some(WorkspaceId::Station(_)) => vec![RightTab::Files],
        _ => vec![
            RightTab::Board,
            RightTab::Files,
            RightTab::Tasks,
            RightTab::Approvals,
            RightTab::Activity,
        ],
    }
}

/// Moves the centre and the right panel off a view the workspace on screen does not have: Chat or Diff to the editor,
/// a tab to the files.
pub(super) fn settle_views(st: &mut State) {
    let ws = st.ide.workspace.as_ref();
    if !centre_views(ws).contains(&st.centre) {
        st.centre = Centre::Editor;
    }
    if !right_tabs(ws).contains(&st.right) {
        st.right = RightTab::Files;
    }
}

/// The agent the workspace on screen is, read from the roster.
pub(super) fn selected_agent(st: &State) -> Option<&AgentRow> {
    match st.ide.workspace.as_ref()? {
        WorkspaceId::Agent(cid) => st.roster.iter().find(|a| a.cid == *cid),
        WorkspaceId::Repo { .. } | WorkspaceId::Station(_) => None,
    }
}

/// The project the workspace on screen belongs to: the one its agent works on (else the first project that names
/// it), or the one whose repo it is.
pub(super) fn selected_project(st: &State) -> Option<&ProjectRow> {
    match st.ide.workspace.as_ref()? {
        WorkspaceId::Agent(cid) => {
            match st
                .roster
                .iter()
                .find(|a| a.cid == *cid)
                .and_then(|a| a.project.as_deref())
            {
                Some(project) => st.projects.iter().find(|p| p.id == project),
                None => st.projects.iter().find(|p| p.members.contains(cid)),
            }
        }
        WorkspaceId::Repo { name, .. } => st
            .projects
            .iter()
            .find(|p| p.repo.as_deref() == Some(name.as_str())),
        // A workstation belongs to no project.
        WorkspaceId::Station(_) => None,
    }
}

/// The breadcrumb's parts: the project's title (or "Plaza" when no project names the workspace), the agent's or
/// repo's name, and the branch (a repo's ref, or "workspace" for an agent's working copy). A conversation on screen
/// is "Conversations", its title, and the project it started, if it did.
pub(super) fn crumbs(st: &State) -> (String, String, String) {
    if let Some(conv) = &st.conversation {
        let data = conv.data.as_ref();
        let title = data
            .and_then(|c| c["title"].as_str())
            .unwrap_or(&conv.id)
            .to_string();
        let project = data
            .and_then(|c| c["project"].as_str())
            .map(crate::console::scope::title)
            .unwrap_or_default();
        return ("Conversations".to_string(), title, project);
    }
    let project = selected_project(st)
        .map(|p| p.title.clone())
        .filter(|t: &String| !t.is_empty())
        .unwrap_or_else(|| "Plaza".to_string());
    match st.ide.workspace.as_ref() {
        Some(WorkspaceId::Agent(_)) => {
            let name = selected_agent(st)
                .map(|a| a.name.clone())
                .unwrap_or_else(|| {
                    st.ide
                        .workspace
                        .as_ref()
                        .map(WorkspaceId::to_string)
                        .unwrap_or_default()
                });
            (project, name, "workspace".to_string())
        }
        Some(WorkspaceId::Repo { name, git_ref }) => (project, name.clone(), git_ref.clone()),
        Some(WorkspaceId::Station(id)) => {
            // A workstation is named as the hub lists it; its id stands in when it has no name.
            let key = WorkspaceId::Station(id.clone()).to_string();
            let listed = st.workspaces.iter().find(|w| w.id == key);
            let name = listed
                .map(|w| w.name.clone())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| id.clone());
            ("Workstations".to_string(), name, "work folder".to_string())
        }
        None => (project, "No workspace".to_string(), String::new()),
    }
}

/// Milliseconds since the epoch, now.
pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub(super) fn fetch_workspaces(client: &Client) -> Result<Vec<WorkspaceInfo>, String> {
    client.get("/v1/ide/workspaces")
}

pub(super) fn fetch_tree(
    client: &Client,
    ws: &WorkspaceId,
    dir: &str,
    cursor: Option<&str>,
) -> Result<TreePage, String> {
    let mut rest = format!("/tree?path={}", enc(dir));
    if let Some(cursor) = cursor {
        rest.push_str(&format!("&cursor={}", enc(cursor)));
    }
    client.get(&route(ws, &rest))
}

pub(super) fn fetch_file(
    client: &Client,
    ws: &WorkspaceId,
    path: &str,
) -> Result<FileView, String> {
    client.get(&route(ws, &format!("/file?path={}", enc(path))))
}

/// `PUT .../file` with the base hash. A 409 becomes `Conflict` with the hub's hash now.
pub(super) fn put_file(
    client: &Client,
    ws: &WorkspaceId,
    path: &str,
    text: &str,
    expected: Option<&str>,
) -> Result<Saved, SaveError> {
    let body = json!({ "path": path, "text": text, "expected_hash": expected });
    let (status, value) = apz_http::client::request_json(
        &client.target,
        "PUT",
        &route(ws, "/file"),
        Some(&body),
        client.timeout,
    )
    .map_err(|e| SaveError::Failed(e.to_string()))?;
    match status {
        200..=299 => serde_json::from_value(value).map_err(|e| SaveError::Failed(e.to_string())),
        409 => Err(SaveError::Conflict(
            value
                .get("current_hash")
                .and_then(Value::as_str)
                .map(str::to_string),
        )),
        _ => Err(SaveError::Failed(
            value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("HTTP {status}")),
        )),
    }
}

/// The base as the hub names it in `?base=`.
pub(super) fn base_param(base: &Base) -> String {
    match base {
        Base::WorkStart => "work_start".to_string(),
        Base::Checkpoint(n) => format!("checkpoint:{n}"),
        Base::Branch(name) => format!("branch:{name}"),
    }
}

pub(super) fn fetch_changes(
    client: &Client,
    ws: &WorkspaceId,
    base: &Base,
) -> Result<ChangeSet, String> {
    let value: Value = client.get(&route(
        ws,
        &format!("/changes?base={}", enc(&base_param(base))),
    ))?;
    let diff = value
        .get("diff")
        .and_then(Value::as_str)
        .unwrap_or_default();
    apz_ide::parse_unified(diff).map_err(|e| e.to_string())
}

/// The checkpoint list, as a bare array or under `checkpoints`.
pub(super) fn parse_checkpoints(value: Value) -> Result<Vec<Checkpoint>, String> {
    let list = if value.is_array() {
        value
    } else {
        value
            .get("checkpoints")
            .cloned()
            .unwrap_or_else(|| json!([]))
    };
    serde_json::from_value(list).map_err(|e| e.to_string())
}

pub(super) fn fetch_checkpoints(
    client: &Client,
    ws: &WorkspaceId,
) -> Result<Vec<Checkpoint>, String> {
    let value: Value = client.get(&route(ws, "/checkpoints"))?;
    parse_checkpoints(value)
}

pub(super) fn fetch_search(
    client: &Client,
    ws: &WorkspaceId,
    query: &str,
    glob: &str,
) -> Result<SearchReply, String> {
    let mut rest = format!("/search?q={}", enc(query));
    if !glob.is_empty() {
        rest.push_str(&format!("&glob={}", enc(glob)));
    }
    client.get(&route(ws, &rest))
}

// ---- Targets, the remembered workspace, the shortcut ------------------------------------------

/// Where the IDE opens, as an entry point names it: a workspace id (`agent:<cid>` or `repo:<name>@<ref>`),
/// then `?` and `&`-joined flags: `review` (review mode) and `open=<path>` (that file, percent-encoded).
/// A branch name cannot hold `?`, so the first `?` ends the id.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Target {
    pub workspace: String,
    pub review: bool,
    pub open: Option<String>,
}

impl Target {
    pub fn parse(raw: &str) -> Target {
        let (workspace, flags) = raw.split_once('?').unwrap_or((raw, ""));
        let mut target = Target {
            workspace: workspace.to_string(),
            ..Target::default()
        };
        for flag in flags.split('&') {
            if flag == "review" {
                target.review = true;
            } else if let Some(path) = flag.strip_prefix("open=") {
                target.open = Some(dec(path));
            }
        }
        target
    }
}

/// The target string for `workspace`, in review mode or not, with a file to open (see `Target`).
pub fn target(workspace: &str, review: bool, open: Option<&str>) -> String {
    let mut flags = Vec::new();
    if review {
        flags.push("review".to_string());
    }
    if let Some(path) = open {
        flags.push(format!("open={}", enc(path)));
    }
    if flags.is_empty() {
        workspace.to_string()
    } else {
        format!("{workspace}?{}", flags.join("&"))
    }
}

/// Reverses `enc`: `%XX` becomes its byte, anything else stays as it is.
pub(super) fn dec(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The workspace the IDE last opened, as the settings keep it (None before any, or when it does not parse).
pub fn remembered() -> Option<String> {
    if cfg!(test) {
        return None;
    }
    boot::saved_setting(REMEMBERED)?
        .as_str()
        .filter(|ws| WorkspaceId::from_str(ws).is_ok())
        .map(str::to_string)
}

/// Keeps `ws` as the last workspace (not from tests: they never write the owner's settings).
pub(super) fn remember(ws: &str) {
    if !cfg!(test) {
        let _ = boot::save_setting(REMEMBERED, json!(ws));
    }
}

/// Ctrl+Shift+E (Cmd+Shift+E on macOS) opens the IDE from anywhere. `captured` is whether a focused text
/// field took the key: then it is typing, and the shortcut stays out.
pub fn open_shortcut(key: &Key, modifiers: Modifiers, captured: bool) -> bool {
    !captured
        && modifiers.command()
        && modifiers.shift()
        && !modifiers.alt()
        && matches!(key, Key::Character(c) if c.as_str().eq_ignore_ascii_case("e"))
}

/// The line for a workspace the hub did not list.
pub(super) fn not_listed(id: &WorkspaceId) -> String {
    match id {
        WorkspaceId::Repo { .. } => {
            format!("{id} is not served yet: the hub lists agent workspaces only for now.")
        }
        WorkspaceId::Agent(_) => format!("Workspace {id} is not listed."),
        WorkspaceId::Station(_) => format!("{id} is not online, so its work folder is not served."),
    }
}

// ---- Helpers over the state -------------------------------------------------------------------

/// Applies a reducer action to the shared state.
pub(super) fn dispatch(st: &mut State, action: Action) {
    let ide = std::mem::take(&mut st.ide);
    st.ide = state::reduce(ide, action);
}

/// Whether `ws` (a hub id string) is the workspace on screen.
pub(super) fn is_current(st: &State, ws: &str) -> bool {
    st.ide
        .workspace
        .as_ref()
        .is_some_and(|id| id.to_string() == ws)
}

/// The workspace on screen as the hub listed it.
pub(super) fn current_info(st: &State) -> Option<&WorkspaceInfo> {
    let id = st.ide.workspace.as_ref()?.to_string();
    st.workspaces.iter().find(|w| w.id == id)
}

pub(super) fn writable(st: &State) -> bool {
    current_info(st).is_some_and(|w| w.writable)
}

/// Whether the agent of the workspace on screen is working (the hub's status says so).
pub(super) fn working(st: &State) -> bool {
    current_info(st).is_some_and(|w| w.status.contains("work"))
}

pub(super) fn choice(w: &WorkspaceInfo) -> Choice {
    let mut name = if w.name.is_empty() {
        w.id.clone()
    } else {
        w.name.clone()
    };
    // A repo workspace is shown on the branch it serves.
    if let Ok(WorkspaceId::Repo { git_ref, .. }) = WorkspaceId::from_str(&w.id) {
        name = format!("{name} @{git_ref}");
    }
    let kind = match w.kind.as_str() {
        "agent" => "agent",
        "station" => "workstation",
        _ => "repo",
    };
    let mut label = format!("{kind} · {name}");
    if !w.status.is_empty() {
        label.push_str(&format!(" · {}", w.status));
    }
    Choice {
        id: w.id.clone(),
        label,
    }
}

/// The badge's state: conflict beats a held plaza, which beats an agent working, which beats saved, which
/// beats idle.
pub fn state_badge(st: &State, held: bool) -> (&'static str, Tone) {
    if st.tabs.values().any(|t| t.conflict.is_some()) {
        ("conflict", Tone::Danger)
    } else if held {
        ("held", Tone::Danger)
    } else if working(st) {
        ("agent working", Tone::Signal)
    } else if st.saved && !st.tabs.values().any(|t| t.buffer.dirty) {
        ("saved", Tone::Ok)
    } else {
        ("idle", Tone::Muted)
    }
}

/// The loaded files whose names match the quick-open query.
pub(super) fn quick_hits(st: &State) -> Vec<String> {
    let paths: Vec<String> = st
        .tree
        .entries
        .iter()
        .filter(|e| e.kind == EntryKind::File)
        .map(|e| e.path.clone())
        .collect();
    ide_editor::matches(&paths, st.quick.as_deref().unwrap_or(""))
}

pub(super) fn goto_line(tab: &mut Tab, line: usize) {
    let target = line.saturating_sub(1);
    tab.content.move_to(text_editor::Cursor {
        position: text_editor::Position {
            line: target,
            column: 0,
        },
        selection: None,
    });
}

/// Moves the editor to a pending line once its file has opened.
pub(super) fn apply_goto(st: &mut State, path: &str) {
    if let Some((target, line)) = st.goto.clone()
        && target == path
        && let Some(tab) = st.tabs.get_mut(path)
    {
        goto_line(tab, line);
        st.goto = None;
    }
}

// ---- Tasks ------------------------------------------------------------------------------------

pub(super) fn tree_task(
    client: &Client,
    ws: WorkspaceId,
    dir: String,
    cursor: Option<String>,
) -> Task<Msg> {
    let first = cursor.is_none();
    let key = ws.to_string();
    let c = client.clone();
    let (d, shown) = (dir.clone(), dir);
    client::task(
        move || fetch_tree(&c, &ws, &d, cursor.as_deref()),
        move |res| Msg::Dir {
            ws: key.clone(),
            dir: shown.clone(),
            first,
            res,
        },
    )
}

/// Lists one folder (marking it on its way).
pub(super) fn list_dir(
    st: &mut State,
    ctx: &Ctx,
    dir: String,
    cursor: Option<String>,
) -> Task<Msg> {
    let Some(ws) = st.ide.workspace.clone() else {
        return Task::none();
    };
    st.tree.inflight.insert(dir.clone());
    tree_task(&ctx.client, ws, dir, cursor)
}

/// Lists every open folder on screen whose listing is missing.
pub(super) fn load_missing(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    if st.ide.filter_changed {
        return Task::none();
    }
    let dirs = ide_tree::missing(&st.tree);
    Task::batch(
        dirs.into_iter()
            .map(|dir| list_dir(st, ctx, dir, None))
            .collect::<Vec<_>>(),
    )
}

pub(super) fn changes_task(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    // A workstation has no diff: the hub answers its changes with 501, so they are not asked for.
    let Some(ws) = st.ide.workspace.clone().filter(|ws| has_diff(Some(ws))) else {
        return Task::none();
    };
    st.changes_busy = true;
    let c = ctx.client.clone();
    let base = st.ide.base.clone();
    let key = ws.to_string();
    let answered = base.clone();
    client::task(
        move || fetch_changes(&c, &ws, &base),
        move |res| Msg::Changed {
            ws: key.clone(),
            base: answered.clone(),
            res,
        },
    )
}

/// The changes, unless a fetch is already on its way (events come often; one fetch at a time).
pub(super) fn changes_throttled(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    if st.changes_busy {
        Task::none()
    } else {
        changes_task(st, ctx)
    }
}

pub(super) fn checkpoints_task(st: &State, ctx: &Ctx) -> Task<Msg> {
    // A workstation has no checkpoints: its work folder is not versioned by the hub.
    let Some(ws) = st.ide.workspace.clone().filter(|ws| has_diff(Some(ws))) else {
        return Task::none();
    };
    let c = ctx.client.clone();
    let key = ws.to_string();
    client::task(
        move || fetch_checkpoints(&c, &ws),
        move |res| Msg::Checkpoints {
            ws: key.clone(),
            res,
        },
    )
}

/// Fetches an open file's hub copy, to check it against the tab.
pub(super) fn remote_task(st: &State, ctx: &Ctx, path: &str) -> Task<Msg> {
    let Some(ws) = st.ide.workspace.clone() else {
        return Task::none();
    };
    let c = ctx.client.clone();
    let key = ws.to_string();
    let p = path.to_string();
    let shown = p.clone();
    client::task(
        move || fetch_file(&c, &ws, &p),
        move |res| Msg::Remote {
            ws: key.clone(),
            path: shown.clone(),
            res,
        },
    )
}

pub(super) fn refresh_open(st: &State, ctx: &Ctx) -> Vec<Task<Msg>> {
    st.tabs
        .keys()
        .map(|path| remote_task(st, ctx, path))
        .collect()
}

pub(super) fn relist(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    if st.listing {
        return Task::none();
    }
    st.listing = true;
    let c = ctx.client.clone();
    client::task(move || fetch_workspaces(&c), Msg::Listed)
}

/// The root folder, the changes and the checkpoints of the workspace on screen.
pub(super) fn load_workspace(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    let root = list_dir(st, ctx, String::new(), None);
    let changes = changes_task(st, ctx);
    let mut tasks = vec![root, changes, checkpoints_task(st, ctx)];
    if let Some(ws) = st.ide.workspace.clone() {
        // The agent's held calls count on the Approvals tab, so they load with the workspace; an Agent tab
        // that stayed open across the switch loads its transcript too.
        tasks.push(ide_panels::approvals_task(&ctx.client, &ws).map(Msg::Panels));
        if st.centre == Centre::Chat {
            tasks.push(
                ide_panels::show(&mut st.panels, Panel::Agent, &ctx.client, &ws).map(Msg::Panels),
            );
        }
    }
    ensure_terminals(st, ctx);
    Task::batch(tasks)
}

/// Asks the hub for the roster, the projects and the tasks. The sidebar, the chat's header and the palette read them.
pub(super) fn load_roster(ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    client::task(move || fetch_roster(&c), Msg::Roster)
}

/// Asks the hub for the board: the Board tab's posts, and the unread counts the sidebar shows.
pub(super) fn load_board(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    board::load(&mut st.board, ctx).map(Msg::Board)
}

/// Makes the terminal tabs of an agent's or a workstation's workspace on first need: when the Terminal tab is shown or
/// the terminal is docked under the editor. Nothing is made for a repo workspace, or before the terminal is wanted.
pub(super) fn ensure_terminals(st: &mut State, ctx: &Ctx) {
    let wanted = st.centre == Centre::Terminal || st.layout.drawer;
    if st.terminals.is_some() || !wanted || !has_shell(st.ide.workspace.as_ref()) {
        return;
    }
    if let Some(ws) = st.ide.workspace.as_ref() {
        st.terminals = Some(TerminalTabs::new(ctx.client.target.clone(), ws.to_string()));
    }
}

/// Makes the terminal tabs again with as many tabs as they had. The hub keeps each shell, and a fresh emulator
/// re-attaches to it from the shell's oldest output.
pub(super) fn reopen_terminals(st: &mut State, ctx: &Ctx) {
    let count = st.terminals.as_ref().map_or(1, TerminalTabs::len).max(1);
    st.terminals = None;
    st.terminal_links.clear();
    ensure_terminals(st, ctx);
    if let Some(tabs) = st.terminals.as_mut() {
        for _ in 1..count {
            tabs.add();
        }
    }
}

/// A terminal message. A socket thread that starts a second time for one tab means the IDE was left and came
/// back: the subscriptions stop when the screen is left, and the emulators would then replay the shell's output
/// on top of themselves. So the tabs are made again. Otherwise the tabs take the message, and the slots that
/// are gone are forgotten.
pub(super) fn terminal_task(st: &mut State, msg: TabsMsg, ctx: &Ctx) -> Task<Msg> {
    // No tabs on screen (the IDE was left): a message from a socket that is still ending takes no part.
    if st.terminals.is_none() {
        return Task::none();
    }
    if let TabsMsg::Tab(slot, ide_terminal::Msg::Link(ide_terminal::Link::Ready(_))) = &msg
        && !st.terminal_links.insert(*slot)
    {
        reopen_terminals(st, ctx);
        return Task::none();
    }
    let Some(tabs) = st.terminals.as_mut() else {
        return Task::none();
    };
    let task = tabs.update(msg).map(Msg::Terminals);
    let open: BTreeSet<usize> = (0..tabs.len())
        .filter_map(|index| tabs.get(index))
        .map(|tab| tab.slot())
        .collect();
    st.terminal_links.retain(|slot| open.contains(slot));
    task
}

/// Reloads everything the workspace on screen shows (after a restore).
pub(super) fn reload_all(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    st.tree.loaded.clear();
    st.tree.more.clear();
    st.tree.inflight.clear();
    let mut tasks = vec![load_workspace(st, ctx)];
    tasks.extend(refresh_open(st, ctx));
    Task::batch(tasks)
}

/// Switches the screen to `id`: its tabs, tree and changes are its own, so the old ones go. A conversation on screen
/// gives way to the workspace.
pub(super) fn select(st: &mut State, id: WorkspaceId, ctx: &Ctx) -> Task<Msg> {
    st.conversation = None;
    if st.ide.workspace.as_ref() == Some(&id) {
        return Task::none();
    }
    if st.tabs.values().any(|t| t.buffer.dirty) {
        st.notice = Some("Save the edited files before you switch workspace.".into());
        return Task::none();
    }
    dispatch(st, Action::SetWorkspace(id.clone()));
    settle_views(st);
    remember(&id.to_string());
    st.tabs.clear();
    st.tree = Tree::default();
    st.changes = None;
    st.changes_error = None;
    st.changes_busy = false;
    st.checkpoints.clear();
    st.restore = None;
    st.saved = false;
    st.goto = None;
    st.quick = None;
    st.search = None;
    st.diff_file = None;
    st.notice = None;
    st.steps_open.clear();
    st.cancelling = false;
    st.panels = ide_panels::State::default();
    st.chat_scroll = ide_chat::Scroll::default();
    st.terminals = None;
    st.terminal_links.clear();
    st.terminal_focus = false;
    // The shells the IDE kept were the old workspace's; they keep running at the hub, and their tabs are not made again.
    st.parked = None;
    st.poll_inflight = false;
    load_workspace(st, ctx)
}

/// Shows the search panel on the left, with its box focused. A quick open gives way to it.
pub(super) fn open_search(st: &mut State) -> Task<Msg> {
    st.quick = None;
    st.search.get_or_insert_with(Search::default);
    iced::widget::operation::focus(SEARCH_ID.clone())
}

/// Opens conversation `id` in place of the workspace (the chat with the orchestrator, and the draft beside it).
pub(super) fn open_conversation(st: &mut State, id: String, ctx: &Ctx) -> Task<Msg> {
    if st.conversation.as_ref().is_some_and(|c| c.id == id) {
        return Task::none();
    }
    let load = ide_conversation::load(&id, ctx).map(Msg::Conversation);
    st.conversation = Some(ide_conversation::Conversation::new(id));
    load
}

/// Asks the hub for the owner's conversations, for the sidebar.
pub(super) fn load_conversations(ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    client::task(
        move || c.get::<Value>("/v1/owner/conversations"),
        Msg::Conversations,
    )
}

/// Opens the target the screen's id named, once the list is in: its workspace, in review mode if asked,
/// with its file open if one is named. A conversation's id opens that conversation.
pub(super) fn apply_preselect(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    let Some(raw) = st.preselect.take() else {
        return Task::none();
    };
    if ide_conversation::is_conversation_id(&raw) {
        return open_conversation(st, raw, ctx);
    }
    let target = Target::parse(&raw);
    match WorkspaceId::from_str(&target.workspace) {
        Ok(id) if st.workspaces.iter().any(|w| w.id == id.to_string()) => {
            let mut tasks = vec![select(st, id.clone(), ctx)];
            // A select the owner's unsaved edits refused leaves the old workspace on screen: nothing more then.
            if st.ide.workspace.as_ref() == Some(&id) {
                // A review is the Diff view, which a workstation does not have: the editor shows instead.
                if target.review && has_diff(Some(&id)) {
                    st.centre = Centre::Diff;
                }
                if st.centre == Centre::Diff && st.changes.is_none() && !st.changes_busy {
                    tasks.push(changes_task(st, ctx));
                }
                if let Some(path) = target.open {
                    tasks.push(open_path(st, path, ctx));
                }
            }
            Task::batch(tasks)
        }
        Ok(id) => {
            st.notice = Some(not_listed(&id));
            Task::none()
        }
        Err(e) => {
            st.notice = Some(format!("Workspace id {}: {e}", target.workspace));
            Task::none()
        }
    }
}

/// Opens a file in a tab (or shows its tab). The hub's copy is read once; later changes come as events.
pub(super) fn open_path(st: &mut State, path: String, ctx: &Ctx) -> Task<Msg> {
    st.quick = None;
    // A file opened for the editor puts the editor in the centre, in place of the chat or a diff.
    st.diff_file = None;
    st.centre = Centre::Editor;
    if let Err(e) = ide_path::check_read(&path) {
        st.notice = Some(format!("{path}: {e}"));
        return Task::none();
    }
    if st.tabs.contains_key(&path) {
        dispatch(st, Action::Activate(path));
        return Task::none();
    }
    let Some(ws) = st.ide.workspace.clone() else {
        return Task::none();
    };
    let c = ctx.client.clone();
    let key = ws.to_string();
    let p = path.clone();
    client::task(
        move || fetch_file(&c, &ws, &p),
        move |res| Msg::Opened {
            ws: key.clone(),
            path: path.clone(),
            res,
        },
    )
}

/// The screen's messages: the state changes, and the tasks they start.
/// The chat's messages: a run of tool steps opens or folds, and the agent's current work is stopped (its cancel
/// route; the agent keeps its session).
pub(super) fn chat_task(st: &mut State, msg: ChatMsg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        ChatMsg::Steps(first) => {
            if !st.steps_open.remove(&first) {
                st.steps_open.insert(first);
            }
            Task::none()
        }
        ChatMsg::Cancel => {
            let Some(WorkspaceId::Agent(cid)) = st.ide.workspace.clone() else {
                return Task::none();
            };
            if st.cancelling {
                return Task::none();
            }
            st.cancelling = true;
            let c = ctx.client.clone();
            let path = format!("/v1/agents/{cid}/cancel");
            client::task(
                move || c.call("POST", &path, None).map(|_| ()),
                |res| Msg::Chat(ChatMsg::Cancelled(res)),
            )
        }
        ChatMsg::Cancelled(res) => {
            st.cancelling = false;
            if let Err(e) = res {
                st.notice = Some(format!("The work was not stopped: {e}"));
            }
            Task::none()
        }
        ChatMsg::Decide(..) | ChatMsg::Decided(_) => ide_chat::decision_task(st, msg, ctx),
        ChatMsg::Scrolled(_) | ChatMsg::JumpToNewest => {
            ide_chat::update_scroll(st, msg).unwrap_or_else(Task::none)
        }
    }
}

pub fn update(st: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    if let Msg::Rich(ev) = msg {
        return match ev {
            // A file of the workspace opens in the editor at its line, as a search hit does.
            rich::Ev::Action(rich::RichAction::OpenPath { path, line }) => update(
                st,
                Msg::OpenMatch(path, line.map_or(1, |l| l as usize)),
                ctx,
            ),
            ev => {
                rich::apply(&mut st.cards, ev);
                Task::none()
            }
        };
    }
    if ide_sidebar::handles(&msg) {
        return ide_sidebar::update(st, msg, ctx);
    }
    if ide_chat::handles(&msg) {
        return ide_chat::update(st, msg, ctx);
    }
    if ide_right::handles(&msg) {
        return ide_right::update(st, msg, ctx);
    }
    if ide_palette::handles(&msg) {
        return ide_palette::update(st, msg, ctx);
    }
    if ide_files::handles(&msg) {
        return ide_files::update(st, msg, ctx);
    }
    match msg {
        Msg::Listed(res) => {
            st.listing = false;
            match res {
                Ok(list) => {
                    st.workspaces = list;
                    st.loaded = true;
                    st.error = None;
                    let task = apply_preselect(st, ctx);
                    ensure_terminals(st, ctx);
                    task
                }
                Err(e) => {
                    st.error = Some(e);
                    Task::none()
                }
            }
        }
        Msg::Open(_) => Task::none(),
        Msg::ToggleLeft => {
            if st.left_is_drawer() {
                st.left_drawer = !st.left_drawer;
            } else {
                st.layout.toggle(Edge::Left);
                ide_chrome::save(&st.layout);
            }
            Task::none()
        }
        Msg::Resized(width) => {
            st.width = width;
            Task::none()
        }
        Msg::DrawerTab(tab) => {
            st.drawer_tab = tab;
            Task::none()
        }
        Msg::ToggleDrawer => {
            st.layout.toggle(Edge::Bottom);
            ide_chrome::save(&st.layout);
            ensure_terminals(st, ctx);
            Task::none()
        }
        Msg::ToggleRight => {
            if st.right_is_sheet() {
                st.right_sheet = !st.right_sheet;
            } else {
                st.layout.toggle(Edge::Right);
                ide_chrome::save(&st.layout);
            }
            Task::none()
        }
        Msg::DragStart(edge) => {
            st.drag = Some(Drag::begin(&st.layout, edge));
            Task::none()
        }
        Msg::DragMove(x, y) => {
            if let Some(drag) = st.drag.as_mut() {
                ide_chrome::drag_to(&mut st.layout, drag, x, y);
            }
            Task::none()
        }
        Msg::Terminals(msg) => terminal_task(st, msg, ctx),
        Msg::TerminalHover(over) => {
            st.terminal_hover = over;
            Task::none()
        }
        Msg::Pressed => {
            // A press on the terminal gives it the keys; a press anywhere else takes them back.
            st.terminal_focus = st.terminal_hover;
            Task::none()
        }
        Msg::Shortcut {
            key,
            modifiers,
            status,
        } => {
            // While the palette is open, the arrows move its cursor; the rest of the keys are the box's.
            if st.palette.is_some() {
                return match key {
                    Key::Named(Named::ArrowDown) => Task::done(Msg::PaletteMove(1)),
                    Key::Named(Named::ArrowUp) => Task::done(Msg::PaletteMove(-1)),
                    _ => shortcut(&key, modifiers, status).map_or_else(Task::none, Task::done),
                };
            }
            // While the terminal has the keys, its keys are its own: the IDE's shortcuts stay out, but the drawer's key.
            let drawer_key =
                matches!(&key, Key::Character(c) if c.as_str() == "`") && modifiers.command();
            if st.terminal_focus && !drawer_key {
                return Task::none();
            }
            shortcut(&key, modifiers, status).map_or_else(Task::none, Task::done)
        }
        Msg::DragEnd => {
            if st.drag.take().is_some() {
                ide_chrome::save(&st.layout);
            }
            Task::none()
        }
        Msg::ResetWidth(edge) => {
            st.drag = None;
            st.layout.reset_width(edge);
            ide_chrome::save(&st.layout);
            Task::none()
        }
        // The console hands these to the shell (see `console::update_app`); the IDE never sees them.
        Msg::Shell(_) => Task::none(),
        // Left: a poll's answer is dropped unread, so the run's polling stops until the IDE is entered (see `left`).
        Msg::Rich(_) => Task::none(),
        Msg::Escape => {
            if st.palette.is_some() {
                st.palette = None;
                Task::none()
            } else if st.right_sheet || st.left_drawer {
                // A sheet or drawer over the centre closes before anything else goes back.
                st.right_sheet = false;
                st.left_drawer = false;
                Task::none()
            } else if st.quick.is_some() {
                st.quick = None;
                Task::none()
            } else if st.search.is_some() {
                st.search = None;
                Task::none()
            } else {
                Task::done(Msg::Back)
            }
        }
        Msg::Back | Msg::NewProject => Task::none(),
        Msg::OpenConversation(id) => open_conversation(st, id, ctx),
        Msg::Conversation(msg) => match st.conversation.as_mut() {
            Some(conv) => ide_conversation::update(conv, msg, ctx).map(Msg::Conversation),
            None => Task::none(),
        },
        Msg::Conversations(res) => {
            if let Ok(list) = res {
                st.conversations = list.as_array().cloned().unwrap_or_default();
            }
            Task::none()
        }
        Msg::Event(event) => {
            // The roster follows the agents, the work, the projects and the tasks; the board follows its own events.
            let mut tasks = Vec::new();
            if matches!(event.kind.as_str(), "agent" | "work" | "project" | "task") {
                tasks.push(load_roster(ctx));
            }
            // A conversation changed: the sidebar's list, and the one on screen (an answer, or one streaming in).
            if event.kind == "owner_conversation" {
                tasks.push(load_conversations(ctx));
            }
            if let Some(conv) = st.conversation.as_mut() {
                ide_conversation::on_event(conv, &event.kind, &event.data);
            }
            if matches!(event.kind.as_str(), "board" | "board_paused") {
                tasks.push(load_board(st, ctx));
            }
            tasks.push(event_task(st, event, ctx));
            Task::batch(tasks)
        }
        // The areas' messages were handled above, by their `handles`.
        _ => Task::none(),
    }
}

/// A hub event: a file of the workspace changed, or the agent moved on.
pub(super) fn event_task(st: &mut State, event: events::Event, ctx: &Ctx) -> Task<Msg> {
    let Some(ws) = st.ide.workspace.clone() else {
        return Task::none();
    };
    let agent = match ws {
        WorkspaceId::Agent(cid) => Some(cid),
        WorkspaceId::Repo { .. } | WorkspaceId::Station(_) => None,
    };
    let about_us = agent.is_some_and(|cid| {
        event
            .data
            .get("cid")
            .and_then(Value::as_u64)
            .is_none_or(|c| c == cid)
    });
    match event.kind.as_str() {
        "ide" => {
            if event.data.get("workspace").and_then(Value::as_str) != Some(ws.to_string().as_str())
            {
                return Task::none();
            }
            let path = event
                .data
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let mut tasks = vec![changes_throttled(st, ctx)];
            if st.tabs.contains_key(&path) {
                tasks.push(remote_task(st, ctx, &path));
            }
            let folder = ide_tree::parent(&path).to_string();
            if !path.is_empty()
                && st.tree.loaded.contains(&folder)
                && !st.tree.inflight.contains(&folder)
            {
                tasks.push(list_dir(st, ctx, folder, None));
            }
            Task::batch(tasks)
        }
        "work" if about_us => {
            let mut tasks = vec![relist(st, ctx)];
            if agent.is_some() {
                // The agent may have made files: the root listing is asked again (its open folders follow).
                if !st.tree.inflight.contains("") {
                    tasks.push(list_dir(st, ctx, String::new(), None));
                }
                tasks.push(changes_throttled(st, ctx));
                tasks.extend(refresh_open(st, ctx));
            }
            Task::batch(tasks)
        }
        "agent" => relist(st, ctx),
        "transcript" if about_us && agent.is_some() => {
            ide_panels::push_entry(&mut st.panels, event.data.clone());
            changes_throttled(st, ctx)
        }
        "approval" if about_us && agent.is_some() => {
            ide_panels::approvals_task(&ctx.client, &ws).map(Msg::Panels)
        }
        _ => Task::none(),
    }
}

/// Opens the screen's workspace when the screen is asked to (the list is fetched first).
pub fn load(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    // The window's width is read now, so a narrow window opens with its drawer and sheet (later changes come by
    // the resize events in `subscription`).
    let width = iced::window::latest()
        .and_then(iced::window::size)
        .map(|size| Msg::Resized(size.width));
    // Project mode follows the console's scope.
    st.project = ctx.project.clone();
    let mut tasks = vec![
        width,
        load_roster(ctx),
        load_board(st, ctx),
        load_conversations(ctx),
    ];
    if let Some(conv) = &st.conversation {
        tasks.push(ide_conversation::load(&conv.id, ctx).map(Msg::Conversation));
    }
    if !st.loaded {
        if !st.listing {
            st.listing = true;
            let c = ctx.client.clone();
            tasks.push(client::task(move || fetch_workspaces(&c), Msg::Listed));
        }
        return Task::batch(tasks);
    }
    tasks.push(apply_preselect(st, ctx));
    ensure_terminals(st, ctx);
    Task::batch(tasks)
}

/// The screen is left for another one. The terminals detach, keeping each tab's shell, offset and screen for
/// `entered`; no shell ends. The run's polling stops with the IDE: its answers are dropped until it is entered.
pub fn left(st: &mut State) {
    st.away = true;
    st.terminal_hover = false;
    st.terminal_focus = false;
    st.terminal_links.clear();
    if let Some(tabs) = st.terminals.take() {
        st.parked = Some(tabs.detach());
    }
}

/// The screen is entered from another one. The terminals are made again once from what `left` kept, each
/// attaching to its shell at its offset; kept tabs of a workspace that is no longer on screen are dropped. A run
/// that was being polled is polled again, unless its poll is still on its way.
pub fn entered(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    st.away = false;
    if let Some(kept) = st.parked.take()
        && st
            .ide
            .workspace
            .as_ref()
            .is_some_and(|ws| ws.to_string() == kept.workspace())
    {
        st.terminals = Some(TerminalTabs::restore(ctx.client.target.clone(), kept));
        st.terminal_links.clear();
    }
    resume_poll(st, ctx)
}

/// Asks the hub for the run's output from where it was read to, unless a poll is already on its way. While
/// the run is active each answer asks for the next one (`ide_panels`), so this restarts a chain that `left` ended.
pub(super) fn resume_poll(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    if st.poll_inflight || !st.panels.run.active() {
        return Task::none();
    }
    let (Some(run), Some(ws)) = (st.panels.run.run.clone(), st.ide.workspace.clone()) else {
        return Task::none();
    };
    st.poll_inflight = true;
    let path = route(
        &ws,
        &format!("/runs/{}?offset={}", enc(&run), st.panels.run.offset),
    );
    let c = ctx.client.clone();
    let key = ws.to_string();
    client::task(
        move || c.get::<ide_panels::Poll>(&path),
        move |res| {
            Msg::Panels(ide_panels::Msg::Polled {
                ws: key.clone(),
                res,
            })
        },
    )
}

// ---- Views ------------------------------------------------------------------------------------

/// The IDE's whole window: the top bar, the held strip, the sidebar (projects and agents), the centre (the chat,
/// terminal, editor or diff of the selected agent, with the terminal drawer under it), the right panel, and the
/// status bar. The palette lies over all of it. The console's sidebar and top bar are not drawn around it.
///
/// A narrow window keeps the side panels out of the row: below `LEFT_DRAWER_BELOW` the left sidebar is a drawer, and
/// below `RIGHT_SHEET_BELOW` the right panel is a sheet, each laid over the centre (see `State::left_is_drawer`).
pub fn view(st: &State, plaza: Plaza) -> Element<'_, Msg> {
    let mut page = column![ide_chrome::header(st, plaza)].height(Length::Fill);
    if plaza.held {
        page = page.push(ide_chrome::held_strip());
    }
    if let Some(line) = status_line(st) {
        page = page.push(container(line).padding([4, 16]).width(Length::Fill));
    }
    let mut body = row![].height(Length::Fill);
    if st.layout.left_open && !st.left_is_drawer() {
        body = body.push(left_panel(st, st.layout.left));
        body = body.push(ide_chrome::handle(Edge::Left));
    }
    body = body.push(
        container(centre_column(st))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(Padding {
                top: 10.0,
                right: 14.0,
                bottom: 10.0,
                left: 14.0,
            }),
    );
    if st.layout.right_open && !st.right_is_sheet() {
        body = body.push(ide_chrome::handle(Edge::Right));
        body = body.push(right_panel(st, st.layout.right));
    }
    page = page.push(body);
    page = page.push(ide_chrome::status_bar(st, plaza.live));
    let base = container(page)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(ui::paper);
    let mut stack = Stack::new().push(base);
    if st.left_is_drawer() && st.left_drawer {
        stack = stack.push(ide_chrome::overlay(
            left_panel(st, st.layout.left.min(st.sheet_width())),
            Edge::Left,
        ));
    }
    if st.right_is_sheet() && st.right_sheet {
        stack = stack.push(ide_chrome::overlay(
            right_panel(st, st.layout.right.min(st.sheet_width())),
            Edge::Right,
        ));
    }
    if st.palette.is_some() {
        stack = stack.push(ide_palette::view(st));
    }
    stack.into()
}

/// The left side panel: the sidebar in its padding, on the panel's sheet.
pub(super) fn left_panel(st: &State, width: f32) -> Element<'_, Msg> {
    container(ide_sidebar::view(st))
        .width(Length::Fixed(width))
        .height(Length::Fill)
        .padding(Padding {
            top: 10.0,
            right: 10.0,
            bottom: 10.0,
            left: 12.0,
        })
        .style(ide_chrome::panel_style)
        .into()
}

/// The right side panel: its tabs and the shown tab, on the panel's sheet. Its tabs take the panel's width. With a
/// conversation on screen, the project it drafts.
pub(super) fn right_panel(st: &State, width: f32) -> Element<'_, Msg> {
    let body = match &st.conversation {
        Some(conv) => {
            let roster: Vec<(u64, String)> =
                st.roster.iter().map(|a| (a.cid, a.name.clone())).collect();
            ide_conversation::draft(conv, &roster, now_ms(), Msg::Conversation, Msg::Shell)
        }
        None => ide_right::view(st, width),
    };
    container(body)
        .width(Length::Fixed(width))
        .height(Length::Fill)
        .padding(Padding {
            top: 10.0,
            right: 12.0,
            bottom: 10.0,
            left: 10.0,
        })
        .style(ide_chrome::panel_style)
        .into()
}

/// The centre column: the selected agent's chat (or terminal, editor, diff), and under it the terminal drawer
/// when it is open and the centre is not the terminal itself. The drawer is an agent's or a workstation's: repo
/// workspaces have none.
pub(super) fn centre_column(st: &State) -> Element<'_, Msg> {
    // A conversation is the chat with the orchestrator; it has no terminal drawer.
    if let Some(conv) = &st.conversation {
        let who = conv
            .data
            .as_ref()
            .and_then(|c| c["cid"].as_u64())
            .and_then(|cid| st.roster.iter().find(|a| a.cid == cid))
            .map(|a| a.name.clone())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "The orchestrator".to_string());
        return ide_conversation::chat(conv, who, now_ms(), Msg::Conversation);
    }
    let centre = container(ide_chat::centre(st))
        .width(Length::Fill)
        .height(Length::Fill);
    if !st.layout.drawer || !has_shell(st.ide.workspace.as_ref()) || st.centre == Centre::Terminal {
        return centre.into();
    }
    column![
        centre,
        ide_chrome::handle(Edge::Bottom),
        container(drawer(st))
            .width(Length::Fill)
            .height(Length::Fixed(st.layout.terminal_height))
            .padding(Padding {
                top: 6.0,
                right: 0.0,
                bottom: 0.0,
                left: 0.0,
            }),
    ]
    .height(Length::Fill)
    .into()
}

/// The terminal drawer: its two tabs (Terminal, Run), its hide, and the tab's body.
pub(super) fn drawer(st: &State) -> Element<'_, Msg> {
    let shell = has_shell(st.ide.workspace.as_ref());
    let tabs = ui::tabs(
        &[(Drawer::Terminal, "Terminal"), (Drawer::Run, "Run")],
        st.drawer_tab,
        Msg::DrawerTab,
    );
    let head = row![
        tabs,
        Space::new().width(Length::Fill),
        ui::btn_sm(
            "Hide Ctrl+`",
            Some("x"),
            Kind::Ghost,
            Some(Msg::ToggleDrawer)
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let body: Element<'_, Msg> = match st.drawer_tab {
        Drawer::Terminal => ide_panels::terminal_body(st.terminals.as_ref(), shell),
        Drawer::Run => ui::scroll(ide_panels::run_view(&st.panels, shell).map(Msg::Panels))
            .height(Length::Fill)
            .into(),
    };
    column![head, body].spacing(8).height(Length::Fill).into()
}

pub(super) fn status_line(st: &State) -> Option<Element<'_, Msg>> {
    if let Some(error) = st.error.as_deref() {
        return Some(ui::error_line(error));
    }
    st.notice
        .as_deref()
        .map(|notice| ui::faint(notice.to_string()).into())
}

// ---- Subscription -----------------------------------------------------------------------------

/// The screen's keys, the hub's events, and while a panel edge is dragged, the pointer. Ctrl+S saves,
/// Ctrl+P opens quick open, Ctrl+Shift+F searches, Ctrl+B and Ctrl+J fold the side panels, Escape closes
/// what is open here or goes back.
pub fn subscription(st: &State, ctx: &Ctx) -> Subscription<Msg> {
    // Every key and every left press reaches `update` with whether a widget took it: the shortcuts are decided
    // there, where the terminal's focus is known.
    let keys = iced::event::listen_with(|event, status, _window| match event {
        iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            Some(Msg::Shortcut {
                key,
                modifiers,
                status,
            })
        }
        iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => Some(Msg::Pressed),
        _ => None,
    });
    let drag = if st.drag.is_some() {
        iced::event::listen_with(drag_event)
    } else {
        Subscription::none()
    };
    let terminals = st
        .terminals
        .as_ref()
        .map_or_else(Subscription::none, |tabs| {
            tabs.subscription().map(Msg::Terminals)
        });
    let resized = iced::window::resize_events().map(|(_, size)| Msg::Resized(size.width));
    Subscription::batch([
        keys,
        drag,
        events::subscription(&ctx.client).map(Msg::Event),
        terminals,
        resized,
    ])
}

/// The pointer while a panel edge is dragged: its moves, and the button coming up, from anywhere in the window.
pub(super) fn drag_event(event: iced::Event, _: Status, _: iced::window::Id) -> Option<Msg> {
    match event {
        iced::Event::Mouse(mouse::Event::CursorMoved { position }) => {
            Some(Msg::DragMove(position.x, position.y))
        }
        iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => Some(Msg::DragEnd),
        _ => None,
    }
}

/// The IDE's keys. Ctrl+B and Ctrl+J are for the panels, so they are taken only when no focused field
/// (the editor, a search box) took the key: typing does not fold a panel.
pub(super) fn shortcut(key: &Key, modifiers: Modifiers, status: Status) -> Option<Msg> {
    match key {
        Key::Named(Named::Escape) => Some(Msg::Escape),
        Key::Character(c) if modifiers.command() => {
            match (c.to_lowercase().as_str(), modifiers.shift()) {
                ("s", false) => Some(Msg::Save),
                ("p", false) => Some(Msg::ToggleQuick),
                ("k", false) => Some(Msg::PaletteOpen),
                ("`", false) => Some(Msg::ToggleDrawer),
                ("f", true) => Some(Msg::ToggleSearch),
                ("b", false) if status == Status::Ignored => Some(Msg::ToggleLeft),
                ("j", false) if status == Status::Ignored => Some(Msg::ToggleRight),
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
#[path = "ide_tests.rs"]
mod tests;
