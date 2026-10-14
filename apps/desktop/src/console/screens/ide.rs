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
use apz_ide::{Buffer, ChangeSet, EntryKind, FileChange, TreePage, WorkspaceId, path as ide_path};
use iced::event::Status;
use iced::keyboard::{self, Key, Modifiers, key::Named};
use iced::widget::{Space, button, column, container, pick_list, row, text_editor, text_input};
use iced::{Alignment, Element, Length, Padding, Subscription, Task, mouse};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Id;
use super::ide_chrome::{self, Dock, Drag, Edge, Layout};
use super::ide_diff;
use super::ide_editor;
use super::ide_panels;
use super::ide_terminal::{self, KeptTabs, TabsMsg, TerminalTabs};
use super::ide_tree::{self, Tree};
use crate::console::Ctx;
use crate::console::boot;
use crate::console::client::{self, Client};
use crate::console::events;
use crate::console::shell;
use crate::console::ui::{self, Kind, Tone};
use crate::theme;

/// The setting that keeps the last workspace the IDE opened (the console's Ctrl+Shift+E opens it).
const REMEMBERED: &str = "ide_workspace";

/// The quick-open box and the search box: focused when they open.
static QUICK_ID: LazyLock<iced::widget::Id> = LazyLock::new(iced::widget::Id::unique);
static SEARCH_ID: LazyLock<iced::widget::Id> = LazyLock::new(iced::widget::Id::unique);

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

/// The left panel's view, picked on the activity rail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Side {
    #[default]
    Files,
    Search,
    Changes,
    Checkpoints,
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
    /// The checkpoint awaiting a confirmed restore.
    pub restore: Option<u64>,
    /// The last save of this workspace is in and nothing has been edited since.
    pub saved: bool,
    /// Where the editor goes once a file opens (a search hit's line).
    pub goto: Option<(String, usize)>,
    /// Quick open's query, while it is open.
    pub quick: Option<String>,
    pub search: Option<Search>,
    /// The changed file whose diff the centre shows (in place of the editor), if any.
    pub diff_file: Option<String>,
    /// The left panel's view, and whether the panel is folded (in `layout`).
    pub side: Side,
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
    /// Shows a left panel on the activity rail; a press on the open one folds the panel.
    Side(Side),
    /// Folds or unfolds a side panel (the header's toggles, Ctrl+B and Ctrl+J).
    ToggleLeft,
    ToggleRight,
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
    ToggleReview,
    /// Shows another panel of the right side (Changes, Agent, Approvals, Run, Terminal).
    Panel(Panel),
    /// The right panels' own messages (the agent, its approvals, the run).
    Panels(ide_panels::Msg),
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
    /// Moves the terminal between the side panel and the dock under the editor.
    ToggleTerminalDock,
}

/// The screen's navigation. `Msg::Open` opens the IDE on a workspace (`Some(id)`) or on the picker (`None`);
/// other screens return it to open the IDE. The IDE leaves through `Back`.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Open(workspace) => Some(Id::Ide(workspace.clone())),
        _ => None,
    }
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

fn fetch_workspaces(client: &Client) -> Result<Vec<WorkspaceInfo>, String> {
    client.get("/v1/ide/workspaces")
}

fn fetch_tree(client: &Client, ws: &WorkspaceId, dir: &str, cursor: Option<&str>) -> Result<TreePage, String> {
    let mut rest = format!("/tree?path={}", enc(dir));
    if let Some(cursor) = cursor {
        rest.push_str(&format!("&cursor={}", enc(cursor)));
    }
    client.get(&route(ws, &rest))
}

pub(super) fn fetch_file(client: &Client, ws: &WorkspaceId, path: &str) -> Result<FileView, String> {
    client.get(&route(ws, &format!("/file?path={}", enc(path))))
}

/// `PUT .../file` with the base hash. A 409 becomes `Conflict` with the hub's hash now.
fn put_file(client: &Client, ws: &WorkspaceId, path: &str, text: &str, expected: Option<&str>) -> Result<Saved, SaveError> {
    let body = json!({ "path": path, "text": text, "expected_hash": expected });
    let (status, value) = apz_http::client::request_json(&client.target, "PUT", &route(ws, "/file"), Some(&body), client.timeout)
        .map_err(|e| SaveError::Failed(e.to_string()))?;
    match status {
        200..=299 => serde_json::from_value(value).map_err(|e| SaveError::Failed(e.to_string())),
        409 => Err(SaveError::Conflict(value.get("current_hash").and_then(Value::as_str).map(str::to_string))),
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
fn base_param(base: &Base) -> String {
    match base {
        Base::WorkStart => "work_start".to_string(),
        Base::Checkpoint(n) => format!("checkpoint:{n}"),
        Base::Branch(name) => format!("branch:{name}"),
    }
}

fn fetch_changes(client: &Client, ws: &WorkspaceId, base: &Base) -> Result<ChangeSet, String> {
    let value: Value = client.get(&route(ws, &format!("/changes?base={}", enc(&base_param(base)))))?;
    let diff = value.get("diff").and_then(Value::as_str).unwrap_or_default();
    apz_ide::parse_unified(diff).map_err(|e| e.to_string())
}

/// The checkpoint list, as a bare array or under `checkpoints`.
fn parse_checkpoints(value: Value) -> Result<Vec<Checkpoint>, String> {
    let list = if value.is_array() {
        value
    } else {
        value.get("checkpoints").cloned().unwrap_or_else(|| json!([]))
    };
    serde_json::from_value(list).map_err(|e| e.to_string())
}

fn fetch_checkpoints(client: &Client, ws: &WorkspaceId) -> Result<Vec<Checkpoint>, String> {
    let value: Value = client.get(&route(ws, "/checkpoints"))?;
    parse_checkpoints(value)
}

fn fetch_search(client: &Client, ws: &WorkspaceId, query: &str, glob: &str) -> Result<SearchReply, String> {
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
fn dec(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
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
    boot::saved_setting(REMEMBERED)?.as_str().filter(|ws| WorkspaceId::from_str(ws).is_ok()).map(str::to_string)
}

/// Keeps `ws` as the last workspace (not from tests: they never write the owner's settings).
fn remember(ws: &str) {
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
fn not_listed(id: &WorkspaceId) -> String {
    match id {
        WorkspaceId::Repo { .. } => format!("{id} is not served yet: the hub lists agent workspaces only for now."),
        WorkspaceId::Agent(_) => format!("Workspace {id} is not listed."),
    }
}

// ---- Helpers over the state -------------------------------------------------------------------

/// Applies a reducer action to the shared state.
fn dispatch(st: &mut State, action: Action) {
    let ide = std::mem::take(&mut st.ide);
    st.ide = state::reduce(ide, action);
}

/// Whether `ws` (a hub id string) is the workspace on screen.
fn is_current(st: &State, ws: &str) -> bool {
    st.ide.workspace.as_ref().is_some_and(|id| id.to_string() == ws)
}

/// The workspace on screen as the hub listed it.
fn current_info(st: &State) -> Option<&WorkspaceInfo> {
    let id = st.ide.workspace.as_ref()?.to_string();
    st.workspaces.iter().find(|w| w.id == id)
}

pub(super) fn writable(st: &State) -> bool {
    current_info(st).is_some_and(|w| w.writable)
}

/// Whether the agent of the workspace on screen is working (the hub's status says so).
fn working(st: &State) -> bool {
    current_info(st).is_some_and(|w| w.status.contains("work"))
}

fn choice(w: &WorkspaceInfo) -> Choice {
    let mut name = if w.name.is_empty() { w.id.clone() } else { w.name.clone() };
    // A repo workspace is shown on the branch it serves.
    if let Ok(WorkspaceId::Repo { git_ref, .. }) = WorkspaceId::from_str(&w.id) {
        name = format!("{name} @{git_ref}");
    }
    let kind = if w.kind == "agent" { "agent" } else { "repo" };
    let mut label = format!("{kind} · {name}");
    if !w.status.is_empty() {
        label.push_str(&format!(" · {}", w.status));
    }
    Choice { id: w.id.clone(), label }
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
fn quick_hits(st: &State) -> Vec<String> {
    let paths: Vec<String> = st
        .tree
        .entries
        .iter()
        .filter(|e| e.kind == EntryKind::File)
        .map(|e| e.path.clone())
        .collect();
    ide_editor::matches(&paths, st.quick.as_deref().unwrap_or(""))
}

fn goto_line(tab: &mut Tab, line: usize) {
    let target = line.saturating_sub(1);
    tab.content.move_to(text_editor::Cursor {
        position: text_editor::Position { line: target, column: 0 },
        selection: None,
    });
}

/// Moves the editor to a pending line once its file has opened.
fn apply_goto(st: &mut State, path: &str) {
    if let Some((target, line)) = st.goto.clone()
        && target == path
        && let Some(tab) = st.tabs.get_mut(path)
    {
        goto_line(tab, line);
        st.goto = None;
    }
}

// ---- Tasks ------------------------------------------------------------------------------------

fn tree_task(client: &Client, ws: WorkspaceId, dir: String, cursor: Option<String>) -> Task<Msg> {
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
fn list_dir(st: &mut State, ctx: &Ctx, dir: String, cursor: Option<String>) -> Task<Msg> {
    let Some(ws) = st.ide.workspace.clone() else { return Task::none() };
    st.tree.inflight.insert(dir.clone());
    tree_task(&ctx.client, ws, dir, cursor)
}

/// Lists every open folder on screen whose listing is missing.
fn load_missing(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    if st.ide.filter_changed {
        return Task::none();
    }
    let dirs = ide_tree::missing(&st.tree);
    Task::batch(dirs.into_iter().map(|dir| list_dir(st, ctx, dir, None)).collect::<Vec<_>>())
}

fn changes_task(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    let Some(ws) = st.ide.workspace.clone() else { return Task::none() };
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
fn changes_throttled(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    if st.changes_busy { Task::none() } else { changes_task(st, ctx) }
}

fn checkpoints_task(st: &State, ctx: &Ctx) -> Task<Msg> {
    let Some(ws) = st.ide.workspace.clone() else { return Task::none() };
    let c = ctx.client.clone();
    let key = ws.to_string();
    client::task(move || fetch_checkpoints(&c, &ws), move |res| Msg::Checkpoints { ws: key.clone(), res })
}

/// Fetches an open file's hub copy, to check it against the tab.
fn remote_task(st: &State, ctx: &Ctx, path: &str) -> Task<Msg> {
    let Some(ws) = st.ide.workspace.clone() else { return Task::none() };
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

fn refresh_open(st: &State, ctx: &Ctx) -> Vec<Task<Msg>> {
    st.tabs.keys().map(|path| remote_task(st, ctx, path)).collect()
}

fn relist(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    if st.listing {
        return Task::none();
    }
    st.listing = true;
    let c = ctx.client.clone();
    client::task(move || fetch_workspaces(&c), Msg::Listed)
}

/// The root folder, the changes and the checkpoints of the workspace on screen.
fn load_workspace(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    let root = list_dir(st, ctx, String::new(), None);
    let changes = changes_task(st, ctx);
    let mut tasks = vec![root, changes, checkpoints_task(st, ctx)];
    if let Some(ws) = st.ide.workspace.clone() {
        // The agent's held calls count on the Approvals tab, so they load with the workspace; an Agent tab
        // that stayed open across the switch loads its transcript too.
        tasks.push(ide_panels::approvals_task(&ctx.client, &ws).map(Msg::Panels));
        if right_panel(st.ide.panel) == Panel::Agent {
            tasks.push(ide_panels::show(&mut st.panels, Panel::Agent, &ctx.client, &ws).map(Msg::Panels));
        }
    }
    ensure_terminals(st, ctx);
    Task::batch(tasks)
}

/// Whether the workspace on screen is an agent's: its terminal runs in the agent's VM.
fn on_agent(st: &State) -> bool {
    matches!(st.ide.workspace, Some(WorkspaceId::Agent(_)))
}

/// Makes the terminal tabs of an agent workspace on first need: when the Terminal tab is shown or the terminal
/// is docked under the editor. Nothing is made for a repo workspace, or before the terminal is wanted.
fn ensure_terminals(st: &mut State, ctx: &Ctx) {
    let wanted = right_panel(st.ide.panel) == Panel::Terminal || st.layout.dock == Dock::Bottom;
    if st.terminals.is_some() || !wanted || !on_agent(st) {
        return;
    }
    if let Some(ws) = st.ide.workspace.as_ref() {
        st.terminals = Some(TerminalTabs::new(ctx.client.target.clone(), ws.to_string()));
    }
}

/// Makes the terminal tabs again with as many tabs as they had. The hub keeps each shell, and a fresh emulator
/// re-attaches to it from the shell's oldest output.
fn reopen_terminals(st: &mut State, ctx: &Ctx) {
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
fn terminal_task(st: &mut State, msg: TabsMsg, ctx: &Ctx) -> Task<Msg> {
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
    let Some(tabs) = st.terminals.as_mut() else { return Task::none() };
    let task = tabs.update(msg).map(Msg::Terminals);
    let open: BTreeSet<usize> = (0..tabs.len()).filter_map(|index| tabs.get(index)).map(|tab| tab.slot()).collect();
    st.terminal_links.retain(|slot| open.contains(slot));
    task
}

/// Reloads everything the workspace on screen shows (after a restore).
fn reload_all(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    st.tree.loaded.clear();
    st.tree.more.clear();
    st.tree.inflight.clear();
    let mut tasks = vec![load_workspace(st, ctx)];
    tasks.extend(refresh_open(st, ctx));
    Task::batch(tasks)
}

/// Switches the screen to `id`: its tabs, tree and changes are its own, so the old ones go.
fn select(st: &mut State, id: WorkspaceId, ctx: &Ctx) -> Task<Msg> {
    if st.ide.workspace.as_ref() == Some(&id) {
        return Task::none();
    }
    if st.tabs.values().any(|t| t.buffer.dirty) {
        st.notice = Some("Save the edited files before you switch workspace.".into());
        return Task::none();
    }
    dispatch(st, Action::SetWorkspace(id.clone()));
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
    st.panels = ide_panels::State::default();
    st.terminals = None;
    st.terminal_links.clear();
    st.terminal_focus = false;
    // The shells the IDE kept were the old workspace's; they keep running at the hub, and their tabs are not made again.
    st.parked = None;
    st.poll_inflight = false;
    load_workspace(st, ctx)
}

/// Shows the search panel on the left, with its box focused. A quick open gives way to it.
fn open_search(st: &mut State) -> Task<Msg> {
    st.quick = None;
    st.search.get_or_insert_with(Search::default);
    iced::widget::operation::focus(SEARCH_ID.clone())
}

/// Opens the target the screen's id named, once the list is in: its workspace, in review mode if asked,
/// with its file open if one is named.
fn apply_preselect(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    let Some(raw) = st.preselect.take() else { return Task::none() };
    let target = Target::parse(&raw);
    match WorkspaceId::from_str(&target.workspace) {
        Ok(id) if st.workspaces.iter().any(|w| w.id == id.to_string()) => {
            let mut tasks = vec![select(st, id.clone(), ctx)];
            // A select the owner's unsaved edits refused leaves the old workspace on screen: nothing more then.
            if st.ide.workspace.as_ref() == Some(&id) {
                if target.review && !st.ide.review {
                    dispatch(st, Action::ToggleReview);
                }
                if st.ide.review && st.changes.is_none() && !st.changes_busy {
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
fn open_path(st: &mut State, path: String, ctx: &Ctx) -> Task<Msg> {
    st.quick = None;
    // A file opened for the editor puts the editor back in the centre, in place of a diff.
    st.diff_file = None;
    if let Err(e) = ide_path::check_read(&path) {
        st.notice = Some(format!("{path}: {e}"));
        return Task::none();
    }
    if st.tabs.contains_key(&path) {
        dispatch(st, Action::Activate(path));
        return Task::none();
    }
    let Some(ws) = st.ide.workspace.clone() else { return Task::none() };
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
pub fn update(st: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
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
        Msg::Pick(pick) => match WorkspaceId::from_str(&pick.id) {
            Ok(id) => select(st, id, ctx),
            Err(e) => {
                st.notice = Some(e.to_string());
                Task::none()
            }
        },
        Msg::Dir { ws, dir, first, res } => {
            if !is_current(st, &ws) {
                return Task::none();
            }
            match res {
                Ok(page) => {
                    ide_tree::apply_page(&mut st.tree, &dir, page, first);
                    load_missing(st, ctx)
                }
                Err(e) => {
                    st.tree.inflight.remove(&dir);
                    st.notice = Some(format!("Could not list {}: {e}", if dir.is_empty() { "the root" } else { &dir }));
                    Task::none()
                }
            }
        }
        Msg::ToggleDir(dir) => {
            ide_tree::toggle(&mut st.tree, &dir);
            load_missing(st, ctx)
        }
        Msg::MoreDir(dir) => {
            let Some(cursor) = st.tree.more.get(&dir).cloned() else {
                return Task::none();
            };
            list_dir(st, ctx, dir, Some(cursor))
        }
        Msg::Open(_) => Task::none(),
        Msg::RefreshAll => reload_all(st, ctx),
        Msg::ChangedOnly => {
            dispatch(st, Action::ToggleChangedFilter);
            load_missing(st, ctx)
        }
        Msg::OpenPath(path) => open_path(st, path, ctx),
        Msg::ShowDiff(path) => {
            // A diff is shown in the centre, where the review would be: the review gives way.
            if st.ide.review {
                dispatch(st, Action::ToggleReview);
            }
            st.quick = None;
            st.diff_file = Some(path);
            Task::none()
        }
        Msg::CloseDiff => {
            st.diff_file = None;
            Task::none()
        }
        Msg::Side(side) => {
            // A press on the open panel's icon folds it; another side opens its panel.
            let folds = st.side == side && st.layout.left_open;
            st.layout.left_open = !folds;
            st.side = side;
            if side != Side::Search {
                st.search = None;
            }
            ide_chrome::save(&st.layout);
            if side == Side::Search && !folds { open_search(st) } else { Task::none() }
        }
        Msg::ToggleLeft => {
            st.layout.toggle(Edge::Left);
            if st.layout.left_open && st.side == Side::Search {
                st.search.get_or_insert_with(Search::default);
            }
            ide_chrome::save(&st.layout);
            Task::none()
        }
        Msg::ToggleRight => {
            st.layout.toggle(Edge::Right);
            ide_chrome::save(&st.layout);
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
        Msg::ToggleTerminalDock => {
            st.layout.toggle(Edge::Bottom);
            let mut tasks = Vec::new();
            match st.layout.dock {
                Dock::Side => {
                    // Back on the side, the side panel shows, on its Terminal tab.
                    st.layout.right_open = true;
                    dispatch(st, Action::SetPanel(Panel::Terminal));
                }
                Dock::Bottom if right_panel(st.ide.panel) == Panel::Terminal => {
                    // Docked, the side panel's Terminal tab would only say where it went: the Agent shows instead.
                    dispatch(st, Action::SetPanel(Panel::Agent));
                    if let Some(ws) = st.ide.workspace.clone() {
                        tasks.push(ide_panels::show(&mut st.panels, Panel::Agent, &ctx.client, &ws).map(Msg::Panels));
                    }
                }
                Dock::Bottom => {}
            }
            ide_chrome::save(&st.layout);
            ensure_terminals(st, ctx);
            Task::batch(tasks)
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
        Msg::Shortcut { key, modifiers, status } => {
            // While the terminal has the keys, its keys are its own: the IDE's shortcuts stay out.
            if st.terminal_focus {
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
        Msg::Opened { ws, path, res } => {
            if !is_current(st, &ws) {
                return Task::none();
            }
            match res {
                Ok(file) => match file.text {
                    Some(text) => {
                        st.tabs.insert(path.clone(), Tab::new(&path, file.hash, &text));
                        dispatch(st, Action::Open(path.clone()));
                        st.notice = None;
                        apply_goto(st, &path);
                    }
                    None => st.notice = Some(format!("{path} is not a text file.")),
                },
                Err(e) => st.notice = Some(format!("{path}: {e}")),
            }
            Task::none()
        }
        Msg::Activate(path) => {
            st.diff_file = None;
            dispatch(st, Action::Activate(path));
            Task::none()
        }
        Msg::Close(path) => {
            if st.tabs.get(&path).is_some_and(|t| t.buffer.dirty) {
                st.notice = Some(format!("Save {path} before closing it."));
                return Task::none();
            }
            st.tabs.remove(&path);
            dispatch(st, Action::Close(path));
            Task::none()
        }
        Msg::Edit(path, action) => {
            if writable(st)
                && let Some(tab) = st.tabs.get_mut(&path)
                && tab.edit(action)
            {
                st.saved = false;
            }
            Task::none()
        }
        Msg::Save => {
            let Some(path) = st.ide.active.clone() else { return Task::none() };
            let Some(ws) = st.ide.workspace.clone() else { return Task::none() };
            let Some(tab) = st.tabs.get(&path) else { return Task::none() };
            if !tab.buffer.dirty {
                return Task::none();
            }
            if !writable(st) {
                st.notice = Some("This workspace is read-only.".into());
                return Task::none();
            }
            let sent = tab.buffer.text.clone();
            let expected = tab.buffer.base_hash.clone();
            st.notice = Some(format!("Saving {path}…"));
            let c = ctx.client.clone();
            let key = ws.to_string();
            let p = path.clone();
            let text = sent.clone();
            // `client::task` is for `String` errors; a save carries its own (`SaveError`), so it runs as a plain task.
            Task::perform(async move { put_file(&c, &ws, &p, &text, expected.as_deref()) }, move |res| Msg::Saved {
                ws: key.clone(),
                path: path.clone(),
                sent: sent.clone(),
                res,
            })
        }
        Msg::Saved { ws, path, sent, res } => {
            if !is_current(st, &ws) {
                return Task::none();
            }
            match res {
                Ok(saved) => {
                    if let Some(tab) = st.tabs.get_mut(&path) {
                        tab.saved(&sent, saved.hash);
                    }
                    st.saved = true;
                    st.notice = Some(format!("Saved {path}"));
                    Task::batch([changes_throttled(st, ctx), checkpoints_task(st, ctx)])
                }
                Err(SaveError::Conflict(current)) => {
                    if let Some(tab) = st.tabs.get_mut(&path) {
                        tab.conflicted(current);
                    }
                    st.notice = Some(format!("{path} changed on the hub since you opened it. Reload it or keep yours."));
                    Task::none()
                }
                Err(SaveError::Failed(e)) => {
                    st.notice = Some(format!("Save failed: {e}"));
                    Task::none()
                }
            }
        }
        Msg::Reload(path) => {
            let Some(ws) = st.ide.workspace.clone() else { return Task::none() };
            let c = ctx.client.clone();
            let key = ws.to_string();
            let p = path.clone();
            client::task(
                move || fetch_file(&c, &ws, &p),
                move |res| Msg::Reloaded {
                    ws: key.clone(),
                    path: path.clone(),
                    res,
                },
            )
        }
        Msg::Reloaded { ws, path, res } => {
            if !is_current(st, &ws) {
                return Task::none();
            }
            match res {
                Ok(file) => match (file.text, st.tabs.get_mut(&path)) {
                    (Some(text), Some(tab)) => {
                        tab.reload(&path, file.hash, &text);
                        st.notice = Some(format!("Reloaded {path}"));
                    }
                    (None, _) => st.notice = Some(format!("{path} is not a text file.")),
                    (Some(_), None) => {}
                },
                Err(e) => st.notice = Some(format!("Reload failed: {e}")),
            }
            Task::none()
        }
        Msg::KeepMine(path) => {
            if let Some(tab) = st.tabs.get_mut(&path) {
                tab.keep_mine();
                st.notice = Some(format!("Kept your version of {path}. Saving replaces the hub's file."));
            }
            Task::none()
        }
        Msg::Remote { ws, path, res } => {
            if is_current(st, &ws)
                && let Ok(file) = res
                && let Some(tab) = st.tabs.get_mut(&path)
            {
                match tab.remote(file.hash.as_deref()) {
                    Remote::Reload => {
                        if let Some(text) = &file.text {
                            tab.reload(&path, file.hash.clone(), text);
                        }
                    }
                    Remote::Same | Remote::Conflict => {}
                }
            }
            Task::none()
        }
        Msg::Changed { ws, base, res } => {
            st.changes_busy = false;
            if is_current(st, &ws) && base == st.ide.base {
                match res {
                    Ok(set) => {
                        st.changes = Some(set);
                        st.changes_error = None;
                    }
                    Err(e) => st.changes_error = Some(e),
                }
            }
            Task::none()
        }
        Msg::PickBase(choice) => match choice {
            BaseChoice::WorkStart => {
                dispatch(st, Action::SetBase(Base::WorkStart));
                changes_task(st, ctx)
            }
            BaseChoice::Checkpoint => match st.checkpoints.iter().map(|c| c.n).max() {
                Some(n) => {
                    dispatch(st, Action::SetBase(Base::Checkpoint(n)));
                    changes_task(st, ctx)
                }
                None => {
                    st.notice = Some("There is no checkpoint to compare with yet.".into());
                    Task::none()
                }
            },
        },
        Msg::CompareWith(n) => {
            dispatch(st, Action::SetBase(Base::Checkpoint(n)));
            changes_task(st, ctx)
        }
        Msg::ToggleReview => {
            dispatch(st, Action::ToggleReview);
            if st.ide.review && st.changes.is_none() {
                changes_task(st, ctx)
            } else {
                Task::none()
            }
        }
        Msg::Panel(panel) => {
            dispatch(st, Action::SetPanel(panel));
            ensure_terminals(st, ctx);
            match st.ide.workspace.clone() {
                Some(ws) => ide_panels::show(&mut st.panels, panel, &ctx.client, &ws).map(Msg::Panels),
                None => Task::none(),
            }
        }
        // Left: a poll's answer is dropped unread, so the run's polling stops until the IDE is entered (see `left`).
        Msg::Panels(ide_panels::Msg::Polled { .. }) if st.away => {
            st.poll_inflight = false;
            Task::none()
        }
        Msg::Panels(msg) => {
            let answers_run = matches!(msg, ide_panels::Msg::Started { .. } | ide_panels::Msg::Polled { .. });
            let task = match st.ide.workspace.clone() {
                Some(ws) => ide_panels::update(&mut st.panels, msg, &ctx.client, &ws).map(Msg::Panels),
                None => Task::none(),
            };
            if answers_run {
                // The run's chain goes on from here: ide_panels asks for the next poll while the run is active.
                st.poll_inflight = st.panels.run.active() && st.panels.run.run.is_some();
            }
            task
        }
        Msg::Checkpoints { ws, res } => {
            if is_current(st, &ws) {
                match res {
                    Ok(list) => st.checkpoints = list,
                    Err(e) => st.notice = Some(format!("Checkpoints: {e}")),
                }
            }
            Task::none()
        }
        Msg::AskRestore(n) => {
            st.restore = Some(n);
            Task::none()
        }
        Msg::CancelRestore => {
            st.restore = None;
            Task::none()
        }
        Msg::Restore(n) => {
            let Some(ws) = st.ide.workspace.clone() else { return Task::none() };
            st.notice = Some(format!("Restoring checkpoint #{n}…"));
            let c = ctx.client.clone();
            let key = ws.to_string();
            client::task(
                move || c.post::<Value>(&route(&ws, &format!("/checkpoints/{n}/restore")), &json!({})).map(|_| ()),
                move |res| Msg::Restored { ws: key.clone(), res },
            )
        }
        Msg::Restored { ws, res } => {
            if !is_current(st, &ws) {
                return Task::none();
            }
            st.restore = None;
            match res {
                Ok(()) => {
                    st.notice = Some("Restored the checkpoint.".into());
                    reload_all(st, ctx)
                }
                Err(e) => {
                    st.notice = Some(format!("Restore failed: {e}"));
                    Task::none()
                }
            }
        }
        Msg::ToggleQuick => {
            if st.quick.is_some() {
                st.quick = None;
                Task::none()
            } else {
                st.quick = Some(String::new());
                st.search = None;
                iced::widget::operation::focus(QUICK_ID.clone())
            }
        }
        Msg::QuickQuery(query) => {
            st.quick = Some(query);
            Task::none()
        }
        Msg::QuickFirst => match quick_hits(st).into_iter().next() {
            Some(first) => open_path(st, first, ctx),
            None => Task::none(),
        },
        Msg::ToggleSearch => {
            if st.side == Side::Search && st.search.is_some() {
                st.side = Side::Files;
                st.search = None;
                Task::none()
            } else {
                st.side = Side::Search;
                st.layout.left_open = true;
                ide_chrome::save(&st.layout);
                open_search(st)
            }
        }
        Msg::SearchQuery(query) => {
            if let Some(search) = st.search.as_mut() {
                search.query = query;
            }
            Task::none()
        }
        Msg::SearchGlob(glob) => {
            if let Some(search) = st.search.as_mut() {
                search.glob = glob;
            }
            Task::none()
        }
        Msg::RunSearch => {
            let Some(ws) = st.ide.workspace.clone() else { return Task::none() };
            let Some(search) = st.search.as_mut() else { return Task::none() };
            let query = search.query.trim().to_string();
            if query.is_empty() {
                return Task::none();
            }
            search.busy = true;
            search.error = None;
            let glob = search.glob.trim().to_string();
            let c = ctx.client.clone();
            let key = ws.to_string();
            client::task(
                move || fetch_search(&c, &ws, &query, &glob),
                move |res| Msg::Searched { ws: key.clone(), res },
            )
        }
        Msg::Searched { ws, res } => {
            if is_current(st, &ws)
                && let Some(search) = st.search.as_mut()
            {
                search.busy = false;
                match res {
                    Ok(reply) => {
                        search.results = Some(reply);
                        search.error = None;
                    }
                    Err(e) => search.error = Some(e),
                }
            }
            Task::none()
        }
        Msg::OpenMatch(path, line) => {
            st.diff_file = None;
            if st.tabs.contains_key(&path) {
                dispatch(st, Action::Activate(path.clone()));
                if let Some(tab) = st.tabs.get_mut(&path) {
                    goto_line(tab, line);
                }
                Task::none()
            } else {
                st.goto = Some((path.clone(), line));
                open_path(st, path, ctx)
            }
        }
        Msg::Escape => {
            if st.quick.is_some() {
                st.quick = None;
                Task::none()
            } else if st.search.is_some() {
                st.side = Side::Files;
                st.search = None;
                Task::none()
            } else {
                Task::done(Msg::Back)
            }
        }
        Msg::Back => Task::none(),
        Msg::Event(event) => event_task(st, event, ctx),
    }
}

/// A hub event: a file of the workspace changed, or the agent moved on.
fn event_task(st: &mut State, event: events::Event, ctx: &Ctx) -> Task<Msg> {
    let Some(ws) = st.ide.workspace.clone() else { return Task::none() };
    let agent = match ws {
        WorkspaceId::Agent(cid) => Some(cid),
        WorkspaceId::Repo { .. } => None,
    };
    let about_us = agent.is_some_and(|cid| event.data.get("cid").and_then(Value::as_u64).is_none_or(|c| c == cid));
    match event.kind.as_str() {
        "ide" => {
            if event.data.get("workspace").and_then(Value::as_str) != Some(ws.to_string().as_str()) {
                return Task::none();
            }
            let path = event.data.get("path").and_then(Value::as_str).unwrap_or_default().to_string();
            let mut tasks = vec![changes_throttled(st, ctx)];
            if st.tabs.contains_key(&path) {
                tasks.push(remote_task(st, ctx, &path));
            }
            let folder = ide_tree::parent(&path).to_string();
            if !path.is_empty() && st.tree.loaded.contains(&folder) && !st.tree.inflight.contains(&folder) {
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
        "approval" if about_us && agent.is_some() => ide_panels::approvals_task(&ctx.client, &ws).map(Msg::Panels),
        _ => Task::none(),
    }
}

/// Opens the screen's workspace when the screen is asked to (the list is fetched first).
pub fn load(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    if !st.loaded {
        if st.listing {
            return Task::none();
        }
        st.listing = true;
        let c = ctx.client.clone();
        return client::task(move || fetch_workspaces(&c), Msg::Listed);
    }
    let task = apply_preselect(st, ctx);
    ensure_terminals(st, ctx);
    task
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
        && st.ide.workspace.as_ref().is_some_and(|ws| ws.to_string() == kept.workspace())
    {
        st.terminals = Some(TerminalTabs::restore(ctx.client.target.clone(), kept));
        st.terminal_links.clear();
    }
    resume_poll(st, ctx)
}

/// Asks the hub for the run's output from where it was read to, unless a poll is already on its way. While
/// the run is active each answer asks for the next one (`ide_panels`), so this restarts a chain that `left` ended.
fn resume_poll(st: &mut State, ctx: &Ctx) -> Task<Msg> {
    if st.poll_inflight || !st.panels.run.active() {
        return Task::none();
    }
    let (Some(run), Some(ws)) = (st.panels.run.run.clone(), st.ide.workspace.clone()) else {
        return Task::none();
    };
    st.poll_inflight = true;
    let path = route(&ws, &format!("/runs/{}?offset={}", enc(&run), st.panels.run.offset));
    let c = ctx.client.clone();
    let key = ws.to_string();
    client::task(move || c.get::<ide_panels::Poll>(&path), move |res| Msg::Panels(ide_panels::Msg::Polled { ws: key.clone(), res }))
}

// ---- Views ------------------------------------------------------------------------------------

/// The IDE's whole window: the header, the held strip, the activity rail, the left panel, the centre,
/// the right panel and the status bar. The console's sidebar and top bar are not drawn around it.
pub fn view(st: &State, plaza: Plaza) -> Element<'_, Msg> {
    let mut page = column![ide_chrome::header(st, plaza)].height(Length::Fill);
    if plaza.held {
        page = page.push(ide_chrome::held_strip());
    }
    if let Some(line) = status_line(st) {
        page = page.push(container(line).padding([4, 16]).width(Length::Fill));
    }
    let mut body = row![ide_chrome::rail(st)].height(Length::Fill);
    if st.layout.left_open {
        body = body.push(
            container(left_panel(st))
                .width(Length::Fixed(st.layout.left))
                .height(Length::Fill)
                .padding(Padding {
                    top: 10.0,
                    right: 10.0,
                    bottom: 10.0,
                    left: 12.0,
                })
                .style(ide_chrome::panel_style),
        );
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
    if st.layout.right_open {
        body = body.push(ide_chrome::handle(Edge::Right));
        body = body.push(
            container(right(st))
                .width(Length::Fixed(st.layout.right))
                .height(Length::Fill)
                .padding(Padding {
                    top: 10.0,
                    right: 12.0,
                    bottom: 10.0,
                    left: 10.0,
                })
                .style(ide_chrome::panel_style),
        );
    }
    page = page.push(body);
    page = page.push(ide_chrome::status_bar(st, plaza.live));
    container(page).width(Length::Fill).height(Length::Fill).style(ui::paper).into()
}

/// The workspace picker: the workspaces the hub lists, the one on screen selected.
pub(super) fn picker(st: &State) -> Element<'_, Msg> {
    let choices: Vec<Choice> = st.workspaces.iter().map(choice).collect();
    let selected = st
        .ide
        .workspace
        .as_ref()
        .map(WorkspaceId::to_string)
        .and_then(|id| choices.iter().find(|c| c.id == id).cloned());
    pick_list(choices, selected, Msg::Pick)
        .placeholder("Choose a workspace")
        .width(Length::Fixed(260.0))
        .style(theme::pick_list)
        .into()
}

fn status_line(st: &State) -> Option<Element<'_, Msg>> {
    if let Some(error) = st.error.as_deref() {
        return Some(ui::error_line(error));
    }
    st.notice.as_deref().map(|notice| ui::faint(notice.to_string()).into())
}

/// The left panel: the view the activity rail picked.
fn left_panel(st: &State) -> Element<'_, Msg> {
    match (st.side, &st.search) {
        (Side::Search, Some(search)) => search_panel(search),
        (Side::Changes, _) => changes_side(st),
        (Side::Checkpoints, _) => checkpoints_side(st),
        _ => files_side(st),
    }
}

/// The Files view: the tree, its changed-only filter and its refresh.
fn files_side(st: &State) -> Element<'_, Msg> {
    let filter = st.ide.filter_changed;
    let on_workspace = st.ide.workspace.is_some();
    let header = row![
        ui::label("Files"),
        Space::new().width(Length::Fill),
        ui::btn_sm(
            "Changed",
            None,
            if filter { Kind::Primary } else { Kind::Secondary },
            on_workspace.then_some(Msg::ChangedOnly)
        ),
        ui::btn_sm("", Some("refresh"), Kind::Ghost, on_workspace.then_some(Msg::RefreshAll)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let rows = match (filter, &st.changes) {
        (true, Some(changes)) => ide_tree::rows(&ide_tree::changed(&st.tree, changes), true),
        _ => ide_tree::rows(&st.tree, false),
    };
    column![header, ui::scroll(ide_tree::view(rows, st.ide.active.clone())).height(Length::Fill)]
        .spacing(10)
        .height(Length::Fill)
        .into()
}

/// The Changes view: what has changed since the base, with each file a press away from its diff.
fn changes_side(st: &State) -> Element<'_, Msg> {
    column![ui::label("Changes"), ui::scroll(changes_panel(st)).height(Length::Fill)]
        .spacing(10)
        .height(Length::Fill)
        .into()
}

/// The Checkpoints view: the workspace's checkpoints, with Compare and Restore.
fn checkpoints_side(st: &State) -> Element<'_, Msg> {
    column![ui::label("Checkpoints"), ui::scroll(checkpoints_panel(st)).height(Length::Fill)]
        .spacing(10)
        .height(Length::Fill)
        .into()
}

fn search_panel(search: &Search) -> Element<'_, Msg> {
    let header = row![
        ui::label("Search"),
        Space::new().width(Length::Fill),
        ui::btn_sm("Close", Some("x"), Kind::Ghost, Some(Msg::ToggleSearch)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let query = text_input("Text to find", &search.query)
        .id(SEARCH_ID.clone())
        .on_input(Msg::SearchQuery)
        .on_submit(Msg::RunSearch)
        .style(theme::input)
        .padding(8);
    let glob = text_input("Glob, e.g. *.rs (optional)", &search.glob)
        .on_input(Msg::SearchGlob)
        .on_submit(Msg::RunSearch)
        .style(theme::input)
        .padding(8);
    let mut list = column![].spacing(2);
    if search.busy {
        list = list.push(ui::faint("Searching…"));
    }
    if let Some(error) = &search.error {
        list = list.push(ui::error_line(error));
    }
    if let Some(reply) = &search.results {
        if reply.matches.is_empty() {
            list = list.push(ui::faint("No matches."));
        }
        for m in &reply.matches {
            let hit = column![
                ui::mono(format!("{}:{}", m.path, m.line), 10.5, theme::palette().ink_2),
                ui::mono(m.text.trim().to_string(), 11.5, theme::palette().ink),
            ];
            list = list.push(
                button(hit)
                    .on_press(Msg::OpenMatch(m.path.clone(), m.line))
                    .padding([4, 6])
                    .width(Length::Fill)
                    .style(theme::register_row(false)),
            );
        }
        if reply.truncated {
            list = list.push(ui::faint("Results are cut short. Narrow the search or the glob."));
        }
    }
    column![header, query, glob, ui::scroll(list).height(Length::Fill)]
        .spacing(8)
        .height(Length::Fill)
        .into()
}

/// The centre column: the editor (or what replaces it), with the terminal under it when it is docked there.
fn centre_column(st: &State) -> Element<'_, Msg> {
    if st.layout.dock != Dock::Bottom {
        return centre(st);
    }
    let dock = container(ide_panels::terminal_bottom(st.terminals.as_ref(), on_agent(st)))
        .width(Length::Fill)
        .height(Length::Fixed(st.layout.terminal_height))
        .padding(Padding {
            top: 6.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        });
    column![container(centre(st)).width(Length::Fill).height(Length::Fill), ide_chrome::handle(Edge::Bottom), dock]
        .height(Length::Fill)
        .into()
}

fn centre(st: &State) -> Element<'_, Msg> {
    if st.ide.workspace.is_none() {
        if st.loaded && st.workspaces.is_empty() {
            return ui::empty(
                "projects",
                "Nothing to open yet",
                "Agents with a VM and projects with a repo show up here.",
            );
        }
        return ui::empty("projects", "No workspace open", "Choose an agent workspace or a project repo above.");
    }
    if let Some(query) = st.quick.as_deref() {
        return ide_editor::quick_open(query, quick_hits(st), QUICK_ID.clone());
    }
    if st.ide.review {
        return match &st.changes {
            Some(changes) => ide_diff::review(changes),
            None => ui::empty("git", "Nothing to review yet", "The changes are still loading."),
        };
    }
    // A changed file the rail's Changes list picked: its diff, in place of the editor.
    if let (Some(path), Some(changes)) = (st.diff_file.as_deref(), st.changes.as_ref())
        && let Some(file) = changes.files.iter().find(|f| f.path == path)
    {
        return diff_view(file);
    }
    if let Some(path) = st.ide.active.as_deref()
        && let Some(tab) = st.tabs.get(path)
    {
        return column![
            ide_editor::tab_strip(&st.ide.open, st.ide.active.as_deref(), &st.tabs),
            ide_editor::editor(path, tab, writable(st)),
        ]
        .spacing(8)
        .height(Length::Fill)
        .into();
    }
    ui::empty("file", "No file open", "Pick a file in the tree, or press Ctrl+P to find one by name.")
}

/// The right panel's own view. Changes live on the left now, so a Changes panel shows the Agent.
pub(super) fn right_panel(panel: Panel) -> Panel {
    match panel {
        Panel::Changes => Panel::Agent,
        other => other,
    }
}

/// The right side: the panel tabs, then the shown panel (the agent, its approvals, the run, the terminal).
fn right(st: &State) -> Element<'_, Msg> {
    let agent = matches!(st.ide.workspace, Some(WorkspaceId::Agent(_)));
    let pending = st.panels.held.len();
    let approvals = if pending > 0 { format!("Approvals {pending}") } else { "Approvals".to_string() };
    let shown = right_panel(st.ide.panel);
    let tabs = ui::tabs(
        &[
            (Panel::Agent, "Agent"),
            (Panel::Approvals, approvals.as_str()),
            (Panel::Run, "Run"),
            (Panel::Terminal, "Terminal"),
        ],
        shown,
        Msg::Panel,
    );
    let body: Element<'_, Msg> = match shown {
        Panel::Changes | Panel::Agent => ide_panels::agent_view(&st.panels, agent).map(Msg::Panels),
        Panel::Approvals => ide_panels::approvals_view(&st.panels, agent).map(Msg::Panels),
        Panel::Run => ide_panels::run_view(&st.panels, agent).map(Msg::Panels),
        Panel::Terminal => ide_panels::terminal_side(st.terminals.as_ref(), st.layout.dock, agent),
    };
    // The terminal fills its panel (a scroll would give its grid no height); the other panels scroll.
    let body: Element<'_, Msg> = if shown == Panel::Terminal {
        body
    } else {
        ui::scroll(body).height(Length::Fill).into()
    };
    column![ui::scroll_x(tabs), body].spacing(12).height(Length::Fill).into()
}

/// One changed file's diff in the centre: its path, Open in editor and Close, then its rows.
fn diff_view(file: &FileChange) -> Element<'static, Msg> {
    let open = (file.status != apz_ide::Status::Deleted).then(|| Msg::OpenPath(file.path.clone()));
    let head = row![
        ui::mono(file.path.clone(), 12.0, theme::palette().ink),
        Space::new().width(Length::Fill),
        ui::btn_sm("Open in editor", None, Kind::Ghost, open),
        ui::btn_sm("Close", Some("x"), Kind::Ghost, Some(Msg::CloseDiff)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    column![head, ui::scroll(ide_diff::file_block(file)).height(Length::Fill)]
        .spacing(10)
        .height(Length::Fill)
        .into()
}

fn changes_panel(st: &State) -> Element<'_, Msg> {
    let base = match st.ide.base {
        Base::Checkpoint(_) => BaseChoice::Checkpoint,
        _ => BaseChoice::WorkStart,
    };
    let seg = ui::seg(
        &[(BaseChoice::WorkStart, "Work start"), (BaseChoice::Checkpoint, "Checkpoint")],
        base,
        Msg::PickBase,
    );
    let mut col = column![ui::label("Changed since"), seg].spacing(10);
    if let Some(error) = &st.changes_error {
        col = col.push(ui::error_line(error));
    }
    match &st.changes {
        Some(changes) => {
            col = col.push(ui::mono(ide_diff::summary(changes), 11.0, theme::palette().ink_2));
            col = col.push(ide_diff::file_list(changes, st.diff_file.as_deref()));
        }
        None if st.changes_error.is_none() => col = col.push(ui::faint("Loading changes…")),
        None => {}
    }
    col.into()
}

fn checkpoints_panel(st: &State) -> Element<'_, Msg> {
    let mut list = column![].spacing(10);
    let mut points: Vec<&Checkpoint> = st.checkpoints.iter().collect();
    points.sort_by(|a, b| b.n.cmp(&a.n));
    if points.is_empty() {
        list = list.push(ui::faint("No checkpoints yet."));
    }
    let can_restore = writable(st);
    for point in points {
        let title = if point.label.is_empty() {
            format!("#{}", point.n)
        } else {
            format!("#{}  {}", point.n, point.label)
        };
        let actions = row![
            ui::btn_sm("Compare", None, Kind::Ghost, Some(Msg::CompareWith(point.n))),
            ui::btn_sm("Restore", None, Kind::Secondary, can_restore.then_some(Msg::AskRestore(point.n))),
        ]
        .spacing(6);
        list = list.push(
            column![
                ui::mono(title, 11.5, theme::palette().ink),
                ui::faint(format!("{} files", point.files)),
                actions
            ]
            .spacing(4),
        );
    }
    if let Some(n) = st.restore {
        let confirm = column![
            ui::mono(format!("Restore checkpoint #{n}?"), 11.5, theme::palette().ink),
            ui::faint("The workspace files go back to that point. Open editors reload."),
            row![
                ui::btn_sm("Restore", None, Kind::Danger, Some(Msg::Restore(n))),
                ui::btn_sm("Cancel", None, Kind::Ghost, Some(Msg::CancelRestore)),
            ]
            .spacing(6),
        ]
        .spacing(8);
        list = list.push(
            container(confirm)
                .padding([10, 12])
                .width(Length::Fill)
                .style(theme::notice(theme::Tone::Warn)),
        );
    }
    list.into()
}

// ---- Subscription -----------------------------------------------------------------------------

/// The screen's keys, the hub's events, and while a panel edge is dragged, the pointer. Ctrl+S saves,
/// Ctrl+P opens quick open, Ctrl+Shift+F searches, Ctrl+B and Ctrl+J fold the side panels, Escape closes
/// what is open here or goes back.
pub fn subscription(st: &State, ctx: &Ctx) -> Subscription<Msg> {
    // Every key and every left press reaches `update` with whether a widget took it: the shortcuts are decided
    // there, where the terminal's focus is known.
    let keys = iced::event::listen_with(|event, status, _window| match event {
        iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => Some(Msg::Shortcut { key, modifiers, status }),
        iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => Some(Msg::Pressed),
        _ => None,
    });
    let drag = if st.drag.is_some() { iced::event::listen_with(drag_event) } else { Subscription::none() };
    let terminals = st.terminals.as_ref().map_or_else(Subscription::none, |tabs| tabs.subscription().map(Msg::Terminals));
    Subscription::batch([keys, drag, events::subscription(&ctx.client).map(Msg::Event), terminals])
}

/// The pointer while a panel edge is dragged: its moves, and the button coming up, from anywhere in the window.
fn drag_event(event: iced::Event, _: Status, _: iced::window::Id) -> Option<Msg> {
    match event {
        iced::Event::Mouse(mouse::Event::CursorMoved { position }) => Some(Msg::DragMove(position.x, position.y)),
        iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => Some(Msg::DragEnd),
        _ => None,
    }
}

/// The IDE's keys. Ctrl+B and Ctrl+J are for the panels, so they are taken only when no focused field
/// (the editor, a search box) took the key: typing does not fold a panel.
fn shortcut(key: &Key, modifiers: Modifiers, status: Status) -> Option<Msg> {
    match key {
        Key::Named(Named::Escape) => Some(Msg::Escape),
        Key::Character(c) if modifiers.command() => match (c.to_lowercase().as_str(), modifiers.shift()) {
            ("s", false) => Some(Msg::Save),
            ("p", false) => Some(Msg::ToggleQuick),
            ("f", true) => Some(Msg::ToggleSearch),
            ("b", false) if status == Status::Ignored => Some(Msg::ToggleLeft),
            ("j", false) if status == Status::Ignored => Some(Msg::ToggleRight),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Ctx {
        Ctx {
            client: Client::new("http://127.0.0.1:9", None, None),
        }
    }

    fn info(cid: u64, writable: bool, status: &str) -> WorkspaceInfo {
        WorkspaceInfo {
            id: format!("agent:{cid}"),
            kind: "agent".into(),
            owner: cid.to_string(),
            name: "scout".into(),
            writable,
            status: status.into(),
        }
    }

    /// A screen on agent 7's workspace, with `a.txt` open at `h1`.
    fn screen_with_a_tab() -> State {
        let mut st = State {
            loaded: true,
            workspaces: vec![info(7, true, "idle")],
            ..State::default()
        };
        dispatch(&mut st, Action::SetWorkspace(WorkspaceId::Agent(7)));
        let file = FileView {
            hash: Some("h1".into()),
            text: Some("one\n".into()),
        };
        let _ = update(
            &mut st,
            Msg::Opened {
                ws: "agent:7".into(),
                path: "a.txt".into(),
                res: Ok(file),
            },
            &ctx(),
        );
        st
    }

    /// The owner's edit: the buffer changes as the editor would change it.
    fn type_into(st: &mut State, path: &str, text: &str) {
        st.tabs.get_mut(path).expect("open").buffer.edit(text);
    }

    #[test]
    fn opening_a_file_sets_a_tab_and_makes_it_active() {
        let mut st = State {
            loaded: true,
            workspaces: vec![info(7, true, "idle")],
            ..State::default()
        };
        dispatch(&mut st, Action::SetWorkspace(WorkspaceId::Agent(7)));
        let file = FileView {
            hash: Some("h1".into()),
            text: Some("fn main() {}\n".into()),
        };
        let _ = update(
            &mut st,
            Msg::Opened {
                ws: "agent:7".into(),
                path: "src/main.rs".into(),
                res: Ok(file),
            },
            &ctx(),
        );
        assert!(st.tabs.contains_key("src/main.rs"));
        assert_eq!(st.ide.open, ["src/main.rs"]);
        assert_eq!(st.ide.active.as_deref(), Some("src/main.rs"));
        assert!(!st.tabs["src/main.rs"].buffer.dirty);
    }

    #[test]
    fn a_file_answered_for_another_workspace_is_dropped() {
        let mut st = screen_with_a_tab();
        let file = FileView {
            hash: Some("x".into()),
            text: Some("late\n".into()),
        };
        let _ = update(
            &mut st,
            Msg::Opened {
                ws: "agent:9".into(),
                path: "b.txt".into(),
                res: Ok(file),
            },
            &ctx(),
        );
        assert!(!st.tabs.contains_key("b.txt"));
    }

    #[test]
    fn a_409_marks_the_tab_and_keep_mine_adopts_the_hub_hash_so_the_next_save_wins() {
        let mut st = screen_with_a_tab();
        type_into(&mut st, "a.txt", "two\n");
        let refused = Msg::Saved {
            ws: "agent:7".into(),
            path: "a.txt".into(),
            sent: "two\n".into(),
            res: Err(SaveError::Conflict(Some("h2".into()))),
        };
        let _ = update(&mut st, refused, &ctx());
        let tab = &st.tabs["a.txt"];
        assert_eq!(
            tab.conflict,
            Some(Conflict {
                server_hash: Some("h2".into())
            })
        );
        assert!(tab.buffer.dirty, "a refused save keeps the owner's edits");
        assert_eq!(state_badge(&st, false).0, "conflict");

        let _ = update(&mut st, Msg::KeepMine("a.txt".into()), &ctx());
        let tab = &st.tabs["a.txt"];
        assert_eq!(tab.conflict, None);
        assert_eq!(tab.buffer.base_hash.as_deref(), Some("h2"), "the next save sends the hub's hash");
        assert!(tab.buffer.dirty);
    }

    #[test]
    fn reload_after_a_conflict_takes_the_hub_text_and_is_clean() {
        let mut tab = Tab::new("a.txt", Some("h1".into()), "one\n");
        tab.buffer.edit("mine\n");
        tab.conflicted(Some("h2".into()));
        tab.reload("a.txt", Some("h2".into()), "theirs\n");
        assert_eq!(tab.conflict, None);
        assert_eq!(tab.buffer.text, "theirs\n");
        assert!(!tab.buffer.dirty);
        assert_eq!(tab.buffer.base_hash.as_deref(), Some("h2"));
    }

    #[test]
    fn a_saved_tab_is_clean_unless_the_owner_typed_during_the_save() {
        let mut tab = Tab::new("a.txt", Some("h1".into()), "one\n");
        tab.buffer.edit("two\n");
        tab.buffer.edit("three\n");
        tab.saved("two\n", "h2".into());
        assert!(tab.buffer.dirty, "the text moved on while the save was in flight");
        assert_eq!(tab.buffer.base_hash.as_deref(), Some("h2"));
        tab.saved("three\n", "h3".into());
        assert!(!tab.buffer.dirty);
    }

    #[test]
    fn a_remote_change_reloads_a_clean_tab_and_conflicts_an_edited_one() {
        let mut clean = Tab::new("a.txt", Some("h1".into()), "one\n");
        assert_eq!(clean.remote(Some("h1")), Remote::Same);
        assert_eq!(clean.remote(Some("h9")), Remote::Reload);

        let mut edited = Tab::new("a.txt", Some("h1".into()), "one\n");
        edited.buffer.edit("mine\n");
        assert_eq!(edited.remote(Some("h10")), Remote::Conflict);
        assert_eq!(
            edited.conflict,
            Some(Conflict {
                server_hash: Some("h10".into())
            })
        );
        assert_eq!(edited.buffer.text, "mine\n", "the owner's text stays");
        assert_eq!(edited.remote(Some("h1")), Remote::Same, "the hub moved back: the conflict goes");
        assert_eq!(edited.conflict, None);
    }

    #[test]
    fn a_remote_event_for_a_clean_open_file_reloads_it() {
        let mut st = screen_with_a_tab();
        let fresh = FileView {
            hash: Some("h9".into()),
            text: Some("nine\n".into()),
        };
        let _ = update(
            &mut st,
            Msg::Remote {
                ws: "agent:7".into(),
                path: "a.txt".into(),
                res: Ok(fresh),
            },
            &ctx(),
        );
        let tab = &st.tabs["a.txt"];
        assert_eq!(tab.buffer.text, "nine\n");
        assert_eq!(tab.buffer.base_hash.as_deref(), Some("h9"));
    }

    #[test]
    fn escape_closes_quick_open_before_it_goes_back() {
        let mut st = screen_with_a_tab();
        st.quick = Some("a".into());
        let _ = update(&mut st, Msg::Escape, &ctx());
        assert_eq!(st.quick, None, "the first Escape closes quick open");
    }

    #[test]
    fn switching_workspace_is_refused_while_an_edit_is_unsaved() {
        let mut st = screen_with_a_tab();
        st.workspaces.push(info(8, true, "idle"));
        type_into(&mut st, "a.txt", "two\n");
        let _ = update(&mut st, Msg::Pick(choice(&info(8, true, "idle"))), &ctx());
        assert_eq!(st.ide.workspace, Some(WorkspaceId::Agent(7)));
        assert!(st.notice.is_some());
    }

    #[test]
    fn the_hub_route_percent_encodes_the_workspace_id() {
        let repo = WorkspaceId::Repo {
            name: "plaza-web".into(),
            git_ref: "feat/x".into(),
        };
        assert_eq!(route(&repo, "/file"), "/v1/ide/workspaces/repo%3Aplaza-web%40feat%2Fx/file");
        assert_eq!(enc("a b/c"), "a%20b%2Fc");
        assert_eq!(enc("keep-._~"), "keep-._~");
    }

    #[test]
    fn checkpoints_come_as_a_bare_array_or_under_checkpoints() {
        let bare = json!([{"n": 1, "label": "start", "files": 2}]);
        let wrapped = json!({"checkpoints": [{"n": 1, "label": "start", "files": 2}]});
        assert_eq!(parse_checkpoints(bare).expect("bare"), parse_checkpoints(wrapped).expect("wrapped"));
        assert!(parse_checkpoints(json!({})).expect("empty").is_empty());
    }

    #[test]
    fn the_badge_reads_conflict_then_held_then_agent_then_saved_then_idle() {
        let mut st = screen_with_a_tab();
        assert_eq!(state_badge(&st, false).0, "idle");
        st.saved = true;
        assert_eq!(state_badge(&st, false).0, "saved");
        st.workspaces[0].status = "working".into();
        assert_eq!(state_badge(&st, false).0, "agent working");
        assert_eq!(state_badge(&st, true).0, "held", "a held plaza shows before the agent's work");
        st.tabs.get_mut("a.txt").expect("open").conflicted(Some("h2".into()));
        assert_eq!(state_badge(&st, true).0, "conflict");
    }

    #[test]
    fn the_picker_names_a_repo_by_its_branch_and_an_agent_by_its_status() {
        let repo = WorkspaceInfo {
            id: "repo:plaza@feat/x".into(),
            kind: "repo".into(),
            owner: String::new(),
            name: "plaza".into(),
            writable: true,
            status: String::new(),
        };
        assert_eq!(choice(&repo).label, "repo · plaza @feat/x");
        assert_eq!(choice(&info(7, true, "idle")).label, "agent · scout · idle");
    }

    #[test]
    fn back_goes_to_the_screen_the_ide_was_opened_from_and_never_to_the_ide() {
        assert_eq!(back_target(None), Id::Overview, "no origin: the Overview");
        assert_eq!(back_target(Some(Id::Agents)), Id::Agents);
        assert_eq!(back_target(Some(Id::Ide(Some("agent:7".into())))), Id::Overview, "an IDE origin would loop");
    }

    #[test]
    fn escape_closes_the_search_panel_before_it_goes_back() {
        let mut st = screen_with_a_tab();
        let _ = update(&mut st, Msg::Side(Side::Search), &ctx());
        assert_eq!(st.side, Side::Search);
        assert!(st.search.is_some());
        let _ = update(&mut st, Msg::Escape, &ctx());
        assert_eq!((st.side, st.search.is_none()), (Side::Files, true), "the first Escape closes the search");
    }

    #[test]
    fn a_press_on_the_open_rail_icon_folds_its_panel_and_another_side_opens_it() {
        let mut st = screen_with_a_tab();
        assert!(st.layout.left_open);
        let _ = update(&mut st, Msg::Side(Side::Files), &ctx());
        assert!(!st.layout.left_open, "the open side folds");
        let _ = update(&mut st, Msg::Side(Side::Changes), &ctx());
        assert!(st.layout.left_open && st.side == Side::Changes, "another side opens the panel on it");
    }

    #[test]
    fn a_changed_file_shows_its_diff_in_the_centre_and_opening_the_editor_clears_it() {
        let mut st = screen_with_a_tab();
        st.diff_file = Some("a.txt".into());
        let _ = update(&mut st, Msg::Activate("a.txt".into()), &ctx());
        assert_eq!(st.diff_file, None, "a tab shown puts the editor back");
        let _ = update(&mut st, Msg::ShowDiff("a.txt".into()), &ctx());
        assert_eq!(st.diff_file.as_deref(), Some("a.txt"));
        let _ = update(&mut st, Msg::CloseDiff, &ctx());
        assert_eq!(st.diff_file, None);
    }

    #[test]
    fn a_drag_on_an_edge_changes_its_width_and_a_double_press_resets_it() {
        let mut st = screen_with_a_tab();
        let _ = update(&mut st, Msg::DragStart(Edge::Left), &ctx());
        let _ = update(&mut st, Msg::DragMove(100.0, 0.0), &ctx());
        let _ = update(&mut st, Msg::DragMove(160.0, 0.0), &ctx());
        assert_eq!(st.layout.left, 264.0 + 60.0);
        let _ = update(&mut st, Msg::DragEnd, &ctx());
        assert!(st.drag.is_none(), "the button came up: the drag ends");
        let _ = update(&mut st, Msg::ResetWidth(Edge::Left), &ctx());
        assert_eq!(st.layout.left, 264.0);
    }

    #[test]
    fn the_terminal_takes_the_keys_while_it_has_focus() {
        let mut st = screen_with_a_tab();
        type_into(&mut st, "a.txt", "two\n");
        st.saved = false;
        let save_key = |st: &mut State| {
            let key = Key::Character("s".into());
            let _ = update(
                st,
                Msg::Shortcut {
                    key,
                    modifiers: Modifiers::CTRL,
                    status: Status::Ignored,
                },
                &ctx(),
            );
        };
        // The terminal has the keys: Ctrl+S is the shell's, so nothing is saved.
        st.terminal_hover = true;
        let _ = update(&mut st, Msg::Pressed, &ctx());
        assert!(st.terminal_focus);
        save_key(&mut st);
        assert_eq!(st.notice, None, "the terminal's Ctrl+S is not the IDE's save");
        // A press elsewhere (the editor) gives the keys back: Ctrl+S is the IDE's save again.
        st.terminal_hover = false;
        let _ = update(&mut st, Msg::Pressed, &ctx());
        assert!(!st.terminal_focus);
        assert!(matches!(shortcut(&Key::Character("s".into()), Modifiers::CTRL, Status::Ignored), Some(Msg::Save)));
    }

    #[test]
    fn the_terminal_dock_moves_the_terminal_and_makes_its_tabs_once() {
        let mut st = screen_with_a_tab();
        assert_eq!(st.layout.dock, Dock::Side);
        assert!(st.terminals.is_none(), "no shell until the terminal is wanted");
        let _ = update(&mut st, Msg::ToggleTerminalDock, &ctx());
        assert_eq!(st.layout.dock, Dock::Bottom);
        assert_eq!(st.terminals.as_ref().map(TerminalTabs::len), Some(1), "docking makes the first tab");
        let _ = update(&mut st, Msg::ToggleTerminalDock, &ctx());
        assert_eq!(st.layout.dock, Dock::Side);
        assert_eq!(st.ide.panel, Panel::Terminal, "back on the side, the side panel shows the terminal");
        assert!(st.layout.right_open);
        assert_eq!(st.terminals.as_ref().map(TerminalTabs::len), Some(1), "the same tabs go on");
    }

    #[test]
    fn a_repo_workspace_makes_no_terminal() {
        let mut st = State {
            loaded: true,
            ..State::default()
        };
        dispatch(&mut st, Action::SetWorkspace(WorkspaceId::Repo {
            name: "plaza".into(),
            git_ref: "main".into(),
        }));
        let _ = update(&mut st, Msg::Panel(Panel::Terminal), &ctx());
        let _ = update(&mut st, Msg::ToggleTerminalDock, &ctx());
        assert!(st.terminals.is_none(), "repo workspaces run no shell");
    }

    #[test]
    fn remaking_the_terminal_tabs_keeps_their_count_and_forgets_their_starts() {
        let mut st = screen_with_a_tab();
        let _ = update(&mut st, Msg::Panel(Panel::Terminal), &ctx());
        let _ = update(&mut st, Msg::Terminals(TabsMsg::Add), &ctx());
        assert_eq!(st.terminals.as_ref().map(TerminalTabs::len), Some(2));
        st.terminal_links.insert(0);
        reopen_terminals(&mut st, &ctx());
        assert_eq!(st.terminals.as_ref().map(TerminalTabs::len), Some(2), "the same two tabs are made again");
        assert!(st.terminal_links.is_empty(), "the new tabs have not started yet");
    }

    #[test]
    fn the_panel_keys_fold_the_side_panels_only_when_no_field_took_them() {
        let cmd = if cfg!(target_os = "macos") { Modifiers::LOGO } else { Modifiers::CTRL };
        let b = Key::Character("b".into());
        let j = Key::Character("j".into());
        assert!(matches!(shortcut(&b, cmd, Status::Ignored), Some(Msg::ToggleLeft)));
        assert!(matches!(shortcut(&j, cmd, Status::Ignored), Some(Msg::ToggleRight)));
        assert!(shortcut(&b, cmd, Status::Captured).is_none(), "a focused field keeps the key");
        assert!(shortcut(&b, cmd | Modifiers::SHIFT, Status::Ignored).is_none(), "Shift+B is not this key");
    }

    #[test]
    fn open_navigates_to_the_ide_on_the_given_workspace_or_the_picker() {
        assert_eq!(go(&Msg::Open(Some("agent:7".into()))), Some(Id::Ide(Some("agent:7".into()))));
        assert_eq!(go(&Msg::Open(None)), Some(Id::Ide(None)));
        assert_eq!(go(&Msg::Save), None, "other messages do not navigate");
    }

    #[test]
    fn a_tree_listing_for_the_current_workspace_fills_the_tree() {
        let mut st = screen_with_a_tab();
        let page = TreePage {
            entries: vec![apz_ide::Entry {
                path: "a.txt".into(),
                kind: EntryKind::File,
                size: 4,
                hash: Some("h1".into()),
                binary: false,
            }],
            next: None,
        };
        let _ = update(
            &mut st,
            Msg::Dir {
                ws: "agent:7".into(),
                dir: String::new(),
                first: true,
                res: Ok(page),
            },
            &ctx(),
        );
        assert!(st.tree.loaded.contains(""));
        assert_eq!(st.tree.entries.len(), 1);
    }

    #[test]
    fn the_open_shortcut_is_command_shift_e_and_stays_out_of_text_fields() {
        let cmd = if cfg!(target_os = "macos") { Modifiers::LOGO } else { Modifiers::CTRL };
        let e = Key::Character("e".into());
        assert!(open_shortcut(&e, cmd | Modifiers::SHIFT, false));
        assert!(open_shortcut(&Key::Character("E".into()), cmd | Modifiers::SHIFT, false), "Shift makes the key read as E");
        assert!(!open_shortcut(&e, cmd | Modifiers::SHIFT, true), "a focused text field keeps the key");
        assert!(!open_shortcut(&e, cmd, false), "without Shift it is not this key");
        assert!(!open_shortcut(&e, cmd | Modifiers::SHIFT | Modifiers::ALT, false));
        assert!(!open_shortcut(&Key::Character("f".into()), cmd | Modifiers::SHIFT, false));
        assert!(!open_shortcut(&e, Modifiers::SHIFT, false), "Shift and E alone type a letter");
    }

    #[test]
    fn a_target_names_the_workspace_and_its_flags() {
        assert_eq!(target("agent:7", false, None), "agent:7");
        assert_eq!(target("repo:plaza@main", true, None), "repo:plaza@main?review");
        let linked = target("repo:plaza@feat/x", false, Some("src/a b.rs"));
        assert_eq!(linked, "repo:plaza@feat/x?open=src%2Fa%20b.rs");
        assert_eq!(
            Target::parse(&linked),
            Target {
                workspace: "repo:plaza@feat/x".into(),
                review: false,
                open: Some("src/a b.rs".into()),
            }
        );
        let both = Target::parse(&target("agent:7", true, Some("x.rs")));
        assert!(both.review);
        assert_eq!(both.open.as_deref(), Some("x.rs"));
        assert_eq!(Target::parse("agent:7"), Target { workspace: "agent:7".into(), ..Target::default() });
        assert_eq!(dec("100%"), "100%", "a stray percent sign stays as it is");
    }

    /// The terminals on screen: each tab's slot, shell, offset and identity.
    fn tabs_held(st: &State) -> Vec<(usize, Option<String>, u64, u64)> {
        let Some(tabs) = st.terminals.as_ref() else { return Vec::new() };
        (0..tabs.len())
            .filter_map(|index| tabs.get(index))
            .map(|tab| (tab.slot(), tab.term().map(str::to_string), tab.offset(), tab.id()))
            .collect()
    }

    /// A tab's message from the socket, by its slot.
    fn from_tab(slot: usize, link: ide_terminal::Link) -> Msg {
        Msg::Terminals(TabsMsg::Tab(slot, ide_terminal::Msg::Link(link)))
    }

    #[test]
    fn leaving_and_entering_makes_the_terminals_once_at_their_offsets() {
        let mut st = screen_with_a_tab();
        let _ = update(&mut st, Msg::Panel(Panel::Terminal), &ctx());
        let _ = update(&mut st, Msg::Terminals(TabsMsg::Add), &ctx());
        let _ = update(&mut st, from_tab(0, ide_terminal::Link::Opened("term-a".into())), &ctx());
        let _ = update(&mut st, from_tab(0, ide_terminal::Link::Output(b"hello".to_vec(), 5)), &ctx());
        let _ = update(&mut st, from_tab(1, ide_terminal::Link::Opened("term-b".into())), &ctx());
        let _ = update(&mut st, from_tab(1, ide_terminal::Link::Output(b"world!".to_vec(), 6)), &ctx());
        let shown = tabs_held(&st);
        let kept = |held: &[(usize, Option<String>, u64, u64)]| held.iter().map(|t| (t.0, t.1.clone(), t.2)).collect::<Vec<_>>();
        assert_eq!(kept(&shown), vec![(0, Some("term-a".to_string()), 5), (1, Some("term-b".to_string()), 6)]);

        left(&mut st);
        assert!(st.terminals.is_none(), "the tabs leave the screen");
        assert!(st.parked.is_some(), "the shells and their offsets are kept");
        // A message from a socket that is still ending takes no part while the IDE is away.
        let _ = update(&mut st, from_tab(0, ide_terminal::Link::Output(b"late".to_vec(), 9)), &ctx());
        assert!(st.terminals.is_none());

        let _ = entered(&mut st, &ctx());
        let back = tabs_held(&st);
        assert_eq!(kept(&back), kept(&shown), "the same shells at the same offsets");
        assert!(back.iter().zip(&shown).all(|(new, old)| new.3 != old.3), "the tabs are made again, once");
        assert!(st.parked.is_none(), "what was kept is used");

        // Entered again with no leave in between, and loaded again: nothing is made again.
        let _ = entered(&mut st, &ctx());
        let _ = load(&mut st, &ctx());
        assert_eq!(tabs_held(&st), back);
    }

    #[test]
    fn a_run_is_not_read_on_while_the_ide_is_away_and_is_polled_again_on_entry() {
        let mut st = screen_with_a_tab();
        st.panels.run = ide_panels::Run {
            phase: ide_panels::Phase::Running,
            run: Some("run-1".into()),
            ..ide_panels::Run::default()
        };
        st.poll_inflight = true;
        left(&mut st);
        let answer = Msg::Panels(ide_panels::Msg::Polled {
            ws: "agent:7".into(),
            res: Ok(ide_panels::Poll {
                output: "late\n".into(),
                next: 5,
                done: false,
                exit_code: None,
                timed_out: false,
            }),
        });
        let _ = update(&mut st, answer, &ctx());
        assert!(!st.poll_inflight, "the answer ends the poll while the IDE is away");
        assert!(st.panels.run.output.is_empty(), "the run is not read on while the IDE is away");
        assert_eq!(st.panels.run.offset, 0);
        let _ = entered(&mut st, &ctx());
        assert!(st.poll_inflight, "entering asks the run for its output again");
        assert!(st.panels.run.output.is_empty(), "the output comes with that poll's answer");
    }
}
