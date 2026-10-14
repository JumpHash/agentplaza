//! The IDE screen (`Id::Ide`): one workspace at a time, an agent's `/workspace` or a project repo as
//! the hub serves it. This module holds the state, the hub calls and the reducer; the tree, the
//! editor, the diff and the search are the sibling modules' views. Shared rules (paths, workspace
//! ids, the diff parser, the tab and base reducer) come from `apz-ide`, the same code the web runs.
//!
//! Saves send the base hash, so the hub refuses a write over a newer one (409): the tab then shows a
//! conflict, and the owner reloads or keeps theirs. Changes come from the hub as a unified diff.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::LazyLock;

use apz_ide::state::{self, Action, Base, IdeState, Panel};
use apz_ide::{Buffer, ChangeSet, EntryKind, TreePage, WorkspaceId, path as ide_path};
use iced::keyboard::{self, Key, Modifiers, key::Named};
use iced::widget::{Space, button, column, container, pick_list, row, text_editor, text_input};
use iced::{Alignment, Border, Element, Length, Padding, Subscription, Task, Theme};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Id;
use super::ide_diff;
use super::ide_editor;
use super::ide_panels;
use super::ide_tree::{self, Tree};
use crate::console::Ctx;
use crate::console::boot;
use crate::console::client::{self, Client};
use crate::console::events;
use crate::console::ui::{self, Kind, Tone};
use crate::theme;

/// The setting that keeps the last workspace the IDE opened (the top bar's button opens it).
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
    /// The changed file whose diff the Changes panel shows.
    pub diff_file: Option<String>,
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
    /// Shows a changed file's diff in the Changes panel.
    ShowDiff(String),
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
}

/// The screen's navigation. `Msg::Open` opens the IDE on a workspace (`Some(id)`) or on the picker (`None`);
/// other screens return it to open the IDE. The IDE leaves through `Back`.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Open(workspace) => Some(Id::Ide(workspace.clone())),
        _ => None,
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

fn writable(st: &State) -> bool {
    current_info(st).is_some_and(|w| w.writable)
}

/// Whether the agent of the workspace on screen is working (the hub's status says so).
fn working(st: &State) -> bool {
    current_info(st).is_some_and(|w| w.status.contains("work"))
}

fn choice(w: &WorkspaceInfo) -> Choice {
    let name = if w.name.is_empty() { w.id.clone() } else { w.name.clone() };
    let kind = if w.kind == "agent" { "agent" } else { "repo" };
    let mut label = format!("{kind} · {name}");
    if !w.status.is_empty() {
        label.push_str(&format!(" · {}", w.status));
    }
    Choice { id: w.id.clone(), label }
}

/// The badge's state: conflict beats an agent working, which beats saved, which beats idle.
pub fn state_badge(st: &State) -> (&'static str, Tone) {
    if st.tabs.values().any(|t| t.conflict.is_some()) {
        ("conflict", Tone::Danger)
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
        if st.ide.panel == Panel::Agent {
            tasks.push(ide_panels::show(&mut st.panels, Panel::Agent, &ctx.client, &ws).map(Msg::Panels));
        }
    }
    Task::batch(tasks)
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
    st.notice = None;
    st.panels = ide_panels::State::default();
    load_workspace(st, ctx)
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
                    apply_preselect(st, ctx)
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
            st.diff_file = Some(path);
            Task::none()
        }
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
            match st.ide.workspace.clone() {
                Some(ws) => ide_panels::show(&mut st.panels, panel, &ctx.client, &ws).map(Msg::Panels),
                None => Task::none(),
            }
        }
        Msg::Panels(msg) => match st.ide.workspace.clone() {
            Some(ws) => ide_panels::update(&mut st.panels, msg, &ctx.client, &ws).map(Msg::Panels),
            None => Task::none(),
        },
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
            if st.search.is_some() {
                st.search = None;
                Task::none()
            } else {
                st.search = Some(Search::default());
                st.quick = None;
                iced::widget::operation::focus(SEARCH_ID.clone())
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
    apply_preselect(st, ctx)
}

// ---- Views ------------------------------------------------------------------------------------

fn frame_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(theme::palette().sheet.into()),
        border: Border {
            color: theme::palette().rule,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

fn vrule() -> Element<'static, Msg> {
    container(Space::new())
        .width(Length::Fixed(1.0))
        .height(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(theme::palette().rule.into()),
            ..Default::default()
        })
        .into()
}

/// The whole screen: the head with the top strip, then the tree, the centre and the right panel.
pub fn view(st: &State) -> Element<'_, Msg> {
    let head = ui::view_head("IDE", head_actions(st));
    let mut page = column![head].spacing(12).height(Length::Fill);
    if let Some(line) = status_line(st) {
        page = page.push(line);
    }
    let body = row![
        container(left(st)).width(Length::Fixed(264.0)).height(Length::Fill),
        vrule(),
        container(centre(st)).width(Length::Fill).height(Length::Fill).padding(Padding {
            top: 0.0,
            right: 14.0,
            bottom: 0.0,
            left: 14.0
        }),
        vrule(),
        container(right(st)).width(Length::Fixed(340.0)).height(Length::Fill),
    ]
    .height(Length::Fill);
    container(page.push(body))
        .padding(Padding {
            top: 0.0,
            right: 28.0,
            bottom: 28.0,
            left: 28.0,
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .style(frame_style)
        .into()
}

/// The top strip: the workspace picker, the changed count, the state badge, Review and Save.
fn head_actions(st: &State) -> Vec<Element<'_, Msg>> {
    let choices: Vec<Choice> = st.workspaces.iter().map(choice).collect();
    let selected = st
        .ide
        .workspace
        .as_ref()
        .map(WorkspaceId::to_string)
        .and_then(|id| choices.iter().find(|c| c.id == id).cloned());
    let picker = pick_list(choices, selected, Msg::Pick)
        .placeholder("Choose a workspace")
        .width(Length::Fixed(280.0))
        .style(theme::pick_list);
    let mut actions: Vec<Element<'_, Msg>> = vec![picker.into()];
    if let Some(changes) = &st.changes {
        actions.push(ui::mono(format!("{} changed", changes.files.len()), 11.5, theme::palette().ink_2).into());
    }
    let (label, tone) = state_badge(st);
    actions.push(ui::badge(label, tone));
    let on_workspace = st.ide.workspace.is_some();
    let review_kind = if st.ide.review { Kind::Primary } else { Kind::Secondary };
    actions.push(ui::btn("Review", None, review_kind, on_workspace.then_some(Msg::ToggleReview)));
    let dirty = st.ide.active.as_ref().and_then(|p| st.tabs.get(p)).is_some_and(|t| t.buffer.dirty);
    let save_kind = if dirty { Kind::Primary } else { Kind::Secondary };
    actions.push(ui::btn("Save", Some("check"), save_kind, (dirty && writable(st)).then_some(Msg::Save)));
    actions
}

fn status_line(st: &State) -> Option<Element<'_, Msg>> {
    if let Some(error) = st.error.as_deref() {
        return Some(ui::error_line(error));
    }
    st.notice.as_deref().map(|notice| ui::faint(notice.to_string()).into())
}

fn left(st: &State) -> Element<'_, Msg> {
    if let Some(search) = &st.search {
        return search_panel(search);
    }
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
        ui::btn_sm("", Some("search"), Kind::Ghost, on_workspace.then_some(Msg::ToggleSearch)),
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

/// The right side: the panel tabs, then the shown panel. Checkpoints sit under Changes, what they compare with.
fn right(st: &State) -> Element<'_, Msg> {
    let agent = matches!(st.ide.workspace, Some(WorkspaceId::Agent(_)));
    let pending = st.panels.held.len();
    let approvals = if pending > 0 { format!("Approvals {pending}") } else { "Approvals".to_string() };
    let tabs = ui::tabs(
        &[
            (Panel::Changes, "Changes"),
            (Panel::Agent, "Agent"),
            (Panel::Approvals, approvals.as_str()),
            (Panel::Run, "Run"),
            (Panel::Terminal, "Terminal"),
        ],
        st.ide.panel,
        Msg::Panel,
    );
    let body: Element<'_, Msg> = match st.ide.panel {
        Panel::Changes => column![changes_panel(st), ui::hairline(), ui::label("Checkpoints"), checkpoints_panel(st)].spacing(14).into(),
        Panel::Agent => ide_panels::agent_view(&st.panels, agent).map(Msg::Panels),
        Panel::Approvals => ide_panels::approvals_view(&st.panels, agent).map(Msg::Panels),
        Panel::Run => ide_panels::run_view(&st.panels, agent).map(Msg::Panels),
        Panel::Terminal => ide_panels::terminal_view().map(Msg::Panels),
    };
    column![ui::scroll_x(tabs), ui::scroll(body).height(Length::Fill)]
        .spacing(12)
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
            if let Some(file) = st.diff_file.as_deref().and_then(|path| changes.files.iter().find(|f| f.path == path)) {
                let open = (file.status != apz_ide::Status::Deleted).then(|| Msg::OpenPath(file.path.clone()));
                col = col.push(ui::btn_sm("Open in editor", None, Kind::Ghost, open));
                col = col.push(ide_diff::file_block(file));
            }
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

/// The screen's keys and the hub's events. Ctrl+S saves, Ctrl+P opens quick open, Ctrl+Shift+F searches,
/// Escape closes what is open here or goes back.
pub fn subscription(ctx: &Ctx) -> Subscription<Msg> {
    let keys = iced::event::listen_with(|event, _status, _window| match event {
        iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => shortcut(&key, modifiers),
        _ => None,
    });
    Subscription::batch([keys, events::subscription(&ctx.client).map(Msg::Event)])
}

fn shortcut(key: &Key, modifiers: Modifiers) -> Option<Msg> {
    match key {
        Key::Named(Named::Escape) => Some(Msg::Escape),
        Key::Character(c) if modifiers.command() => match (c.to_lowercase().as_str(), modifiers.shift()) {
            ("s", false) => Some(Msg::Save),
            ("p", false) => Some(Msg::ToggleQuick),
            ("f", true) => Some(Msg::ToggleSearch),
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
        assert_eq!(state_badge(&st).0, "conflict");

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
    fn the_badge_reads_conflict_then_agent_then_saved_then_idle() {
        let mut st = screen_with_a_tab();
        assert_eq!(state_badge(&st).0, "idle");
        st.saved = true;
        assert_eq!(state_badge(&st).0, "saved");
        st.workspaces[0].status = "working".into();
        assert_eq!(state_badge(&st).0, "agent working");
        st.tabs.get_mut("a.txt").expect("open").conflicted(Some("h2".into()));
        assert_eq!(state_badge(&st).0, "conflict");
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
}
