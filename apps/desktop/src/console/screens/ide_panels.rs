//! The IDE's right panels (Changes lives on the left): the agent's transcript with a steer box, the agent's held
//! approvals (a write's diff against the file now), a bounded run with polled output, and the terminal (a shell
//! in the agent's VM, on the side or docked under the editor). The screen (`ide.rs`) owns the workspace and
//! routes the messages; this module owns its panels' state, their hub calls and their views.
//!
//! The hub answers each call with the workspace it was asked for, so a late answer for a workspace
//! the owner has left is dropped (the same rule as the screen's `is_current`).

use crate::console::chat_tools;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

use apz_ide::WorkspaceId;
use apz_ide::state::Panel;
use iced::widget::{Space, button, column, container, mouse_area, row, text_editor, text_input};
use iced::{Alignment, Border, Element, Length, Task};
use serde::Deserialize;
use serde_json::{Value, json};

use super::ide::{self, FileView};
use super::ide_terminal::TerminalTabs;
use crate::console::client::{self, Client};
use crate::console::files::{self, Attachment};
use crate::console::ui::{self, Kind};
use crate::theme;

/// The transcript keeps this many entries, the newest; the hub's `last=` asks for the same first.
const KEEP_ENTRIES: usize = 400;
const TRANSCRIPT_LAST: usize = 200;
/// The steer note's cap, as the hub's control route enforces it.
pub const STEER_MAX: usize = 2000;
/// The wait between two polls of a run.
const POLL_EVERY: Duration = Duration::from_millis(500);
/// The output the Run panel shows: its newest characters (the whole output stays in memory).
const SHOWN_CHARS: usize = 20_000;
/// The diff lines the panel draws; a longer diff says how many lines it leaves out.
const DIFF_SHOWN: usize = 400;
/// The largest table a diff computes (old lines times new lines); past it the file shows as replaced.
const DIFF_MAX_CELLS: usize = 4_000_000;

// ---- The hub's shapes -------------------------------------------------------------------------

/// One poll of a run (`GET .../runs/{run}?offset=`): the output from the offset, and the run's state.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Poll {
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub next: u64,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub timed_out: bool,
}

// ---- Approvals --------------------------------------------------------------------------------

/// A held call of this agent, awaiting the owner.
#[derive(Debug, Clone, PartialEq)]
pub struct Held {
    pub id: String,
    pub tool: String,
    pub input: Value,
}

/// One line of a diff: unchanged, added, or removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLine {
    Same(String),
    Add(String),
    Del(String),
}

/// What a held edit would change in one of its files.
#[derive(Debug, Clone, PartialEq)]
pub struct FileDiff {
    pub path: String,
    pub lines: Vec<DiffLine>,
    /// Says when there was no copy of the file to compare with.
    pub note: Option<String>,
}

/// A file a held edit names, with the copy the hub gave of it: its path, and the read or why it failed.
pub type FileCopy = (String, Result<FileView, String>);

/// What a held write would change in its files.
#[derive(Debug, Clone, PartialEq)]
pub enum Preview {
    /// The files' current text is on its way.
    Loading,
    /// One diff per file the edit names, each against the text the edit would leave there.
    Diff { files: Vec<FileDiff> },
    /// No diff can be drawn: an edit that does not match its file, or a file that is not text.
    Unavailable(String),
}

/// Whether `tool` writes files the panel can preview: the `edit` tool.
pub fn is_write(tool: &str) -> bool {
    tool == "edit"
}

/// The files a held edit changes, in the order its call names them: its `create`
/// entries, then its `edits`.
fn write_paths(held: &Held) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for key in ["create", "edits"] {
        for entry in held
            .input
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(path) = entry.get("path").and_then(Value::as_str) else {
                continue;
            };
            if !paths.iter().any(|known| known == path) {
                paths.push(path.to_string());
            }
        }
    }
    paths
}

/// The text a held edit leaves in one of its files, given the file's current text. None for a call that
/// is not an edit; `Err` for an entry that does not apply to the file as it is now. A `create` replaces the
/// file's text with its whole content, and the `edits` for that path then apply in order.
pub fn proposed(
    tool: &str,
    input: &Value,
    path: &str,
    current: &str,
) -> Option<Result<String, String>> {
    if !is_write(tool) {
        return None;
    }
    let mut text = current.to_string();
    if let Some(create) = path_entries(input, "create", path).last() {
        match create.get("content").and_then(Value::as_str) {
            Some(content) => text = content.to_string(),
            None => return Some(Err("The write has no content.".to_string())),
        }
    }
    for entry in path_entries(input, "edits", path) {
        let part = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_string);
        let replace_all = entry
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        match edit(&text, part("old"), part("new"), replace_all) {
            Ok(next) => text = next,
            Err(reason) => return Some(Err(reason)),
        }
    }
    Some(Ok(text))
}

/// The entries of an edit's `key` list (`create` or `edits`) that name `path`, in call order.
fn path_entries<'a>(input: &'a Value, key: &str, path: &str) -> Vec<&'a Value> {
    input
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|entry| entry.get("path").and_then(Value::as_str) == Some(path))
                .collect()
        })
        .unwrap_or_default()
}

/// The result of an edit: its old text must occur exactly once in the file, unless the edit replaces all.
fn edit(
    current: &str,
    old: Option<String>,
    new: Option<String>,
    replace_all: bool,
) -> Result<String, String> {
    let (Some(old), Some(new)) = (old, new) else {
        return Err("The edit has no old or new text.".to_string());
    };
    if old.is_empty() {
        return Err("The edit replaces empty text.".to_string());
    }
    match current.matches(&old).count() {
        0 => Err("The text to replace is not in the file.".to_string()),
        1 => Ok(current.replacen(&old, &new, 1)),
        _ if replace_all => Ok(current.replace(&old, &new)),
        n => Err(format!(
            "The text to replace occurs {n} times; an edit needs exactly one match."
        )),
    }
}

/// A line diff of `old` against `new`, by the longest common subsequence of lines. A table too large
/// to compute shows the whole of `old` removed and `new` added.
pub fn line_diff(old: &str, new: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let (n, m) = (a.len(), b.len());
    if n.saturating_mul(m) > DIFF_MAX_CELLS {
        return a
            .iter()
            .map(|l| DiffLine::Del((*l).to_string()))
            .chain(b.iter().map(|l| DiffLine::Add((*l).to_string())))
            .collect();
    }
    // lcs[i][j]: the common lines of a[i..] and b[j..].
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::with_capacity(n.max(m));
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(DiffLine::Same(a[i].to_string()));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push(DiffLine::Del(a[i].to_string()));
            i += 1;
        } else {
            out.push(DiffLine::Add(b[j].to_string()));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| DiffLine::Del((*l).to_string())));
    out.extend(b[j..].iter().map(|l| DiffLine::Add((*l).to_string())));
    out
}

/// The held calls of agent `cid` that still wait, from `GET /v1/approvals`. Tool calls only: the other kinds (reviews,
/// shares) are decided on the Approvals screen.
fn pending_for(list: &Value, cid: u64) -> Vec<Held> {
    list.as_array()
        .map(|items| {
            items
                .iter()
                .filter(|a| a["cid"].as_u64() == Some(cid))
                .filter(|a| a["state"].as_str() == Some("pending"))
                .filter(|a| a["kind"].as_str().unwrap_or("tool") == "tool")
                .filter_map(|a| {
                    Some(Held {
                        id: a["id"].as_str()?.to_string(),
                        tool: a["tool"].as_str().unwrap_or_default().to_string(),
                        input: a["input"].clone(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The preview of a held edit, from the copies the hub gave of each file it names (`copies`).
fn preview_of(held: &Held, copies: Result<Vec<FileCopy>, String>) -> Preview {
    let copies = match copies {
        Ok(copies) => copies,
        Err(e) => return Preview::Unavailable(e),
    };
    let mut files = Vec::new();
    for (path, res) in copies {
        let (current, note) = match res {
            Ok(file) => match file.text {
                Some(text) => (text, None),
                None => {
                    return Preview::Unavailable(
                        "The file is not text, so it has no diff.".to_string(),
                    );
                }
            },
            Err(e) => (
                String::new(),
                Some(format!(
                    "No readable copy of the file ({e}); the diff shows the whole text as new."
                )),
            ),
        };
        match proposed(&held.tool, &held.input, &path, &current) {
            Some(Ok(text)) => files.push(FileDiff {
                path,
                lines: line_diff(&current, &text),
                note,
            }),
            Some(Err(e)) => return Preview::Unavailable(e),
            None => return Preview::Unavailable("This call does not write a file.".to_string()),
        }
    }
    if files.is_empty() {
        return Preview::Unavailable("This call does not write a file.".to_string());
    }
    Preview::Diff { files }
}

// ---- Run --------------------------------------------------------------------------------------

/// Where a run is. `Stopping` is a stop the owner asked for and the hub has not yet ended.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Idle,
    Starting,
    Running,
    Stopping,
    Done {
        exit_code: Option<i64>,
        timed_out: bool,
        stopped: bool,
    },
}

/// Whether polling goes on after a poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Again,
    Finished,
}

/// One bounded command in the workspace: its id, its output so far, and where its output was read to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Run {
    pub phase: Phase,
    /// The hub's run id, once the run has started.
    pub run: Option<String>,
    pub command: String,
    pub output: String,
    /// The byte offset the output was read to; the next poll asks from here.
    pub offset: u64,
    /// The last failure a start, a poll or a stop reported.
    pub error: Option<String>,
}

impl Run {
    /// Whether a run is on its way (starting, running or being stopped).
    pub fn active(&self) -> bool {
        matches!(
            self.phase,
            Phase::Starting | Phase::Running | Phase::Stopping
        )
    }

    /// Begins a run of `command`, clearing the last one. Refused (false) while a run is active.
    pub fn start(&mut self, command: &str) -> bool {
        if self.active() {
            return false;
        }
        *self = Run {
            phase: Phase::Starting,
            command: command.to_string(),
            ..Run::default()
        };
        true
    }

    /// The hub started the run under `run`.
    pub fn started(&mut self, run: String) {
        if self.phase == Phase::Starting {
            self.run = Some(run);
            self.phase = Phase::Running;
        }
    }

    /// The owner asked to stop. True when the stop should be sent (only a running run stops).
    pub fn stop_requested(&mut self) -> bool {
        if self.phase == Phase::Running {
            self.phase = Phase::Stopping;
            true
        } else {
            false
        }
    }

    /// The hub refused the stop: the run goes on as it was.
    pub fn stop_failed(&mut self, error: String) {
        if self.phase == Phase::Stopping {
            self.phase = Phase::Running;
        }
        self.error = Some(error);
    }

    /// The run could not start, or was lost while it ran. Polling ends; the run is no longer active.
    pub fn failed(&mut self, error: String) {
        if self.active() {
            self.phase = Phase::Idle;
        }
        self.error = Some(error);
    }

    /// Applies one poll: appends its output, moves the offset on, and ends the run when the hub says so.
    pub fn apply(&mut self, poll: &Poll) -> Step {
        if !self.active() {
            return Step::Finished;
        }
        self.output.push_str(&poll.output);
        self.offset = self.offset.max(poll.next);
        if poll.done {
            self.phase = Phase::Done {
                exit_code: poll.exit_code,
                timed_out: poll.timed_out,
                stopped: self.phase == Phase::Stopping,
            };
            Step::Finished
        } else {
            Step::Again
        }
    }

    /// The run's state as a line for the owner.
    pub fn status(&self) -> String {
        match &self.phase {
            Phase::Idle if self.command.is_empty() => "Nothing has run yet.".to_string(),
            Phase::Idle => "Not running.".to_string(),
            Phase::Starting => "Starting…".to_string(),
            Phase::Running => "Running…".to_string(),
            Phase::Stopping => "Stopping…".to_string(),
            Phase::Done {
                timed_out: true, ..
            } => "Timed out.".to_string(),
            Phase::Done { stopped: true, .. } => "Stopped.".to_string(),
            Phase::Done {
                exit_code: Some(code),
                ..
            } => format!("Exited with code {code}."),
            Phase::Done {
                exit_code: None, ..
            } => "Finished.".to_string(),
        }
    }
}

/// The run's id in the hub's answer to a start (a string, or a number).
fn run_id(value: &Value) -> Option<String> {
    let run = value.get("run")?;
    run.as_str()
        .map(str::to_string)
        .or_else(|| run.as_u64().map(|n| n.to_string()))
}

/// The timeout box: empty for the hub's default, else whole seconds.
fn parse_timeout(text: &str) -> Result<Option<u64>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    text.parse::<u64>()
        .map(Some)
        .map_err(|_| "The timeout is a whole number of seconds.".to_string())
}

/// The newest `SHOWN_CHARS` characters of a run's output.
pub fn shown(output: &str) -> &str {
    let count = output.chars().count();
    if count <= SHOWN_CHARS {
        return output;
    }
    output
        .char_indices()
        .nth(count - SHOWN_CHARS)
        .map(|(i, _)| &output[i..])
        .unwrap_or(output)
}

// ---- The panels' state ------------------------------------------------------------------------

/// The right panel's state for the workspace on screen. It resets when the workspace changes.
#[derive(Debug, Default)]
pub struct State {
    /// The agent's transcript, oldest first.
    pub entries: Vec<Value>,
    pub entries_loaded: bool,
    pub entries_error: Option<String>,
    pub steer: text_editor::Content,
    pub steer_busy: bool,
    pub steer_notice: Option<String>,
    /// The chat's model chip and context bar (the workspace's agent, loaded when its chat opens).
    pub chat_tools: chat_tools::ChatTools,
    /// The files the composer sends with its next note: uploaded, and how many are still going up.
    pub attachments: Vec<Attachment>,
    pub uploading: usize,
    /// The agent is working: the composer's text steers the work in flight; idle, it is a chat turn of its own (the
    /// IDE sets it before each send).
    pub agent_working: bool,
    pub held: Vec<Held>,
    pub approvals_loaded: bool,
    pub approvals_error: Option<String>,
    pub previews: BTreeMap<String, Preview>,
    pub busy: BTreeSet<String>,
    pub reasons: BTreeMap<String, String>,
    pub run: Run,
    pub command: String,
    pub timeout: String,
}

/// The panels' messages. Hub answers carry their workspace, so a late one is dropped.
#[derive(Debug, Clone)]
pub enum Msg {
    Transcript {
        ws: String,
        res: Result<Vec<Value>, String>,
    },
    Steer(text_editor::Action),
    SteerSend,
    /// The chat's model chip and context bar.
    ChatTools(chat_tools::ChatToolsMsg),
    Steered {
        ws: String,
        /// The attachments the note carried, so a sent one leaves the composer.
        sent: Vec<String>,
        res: Result<Value, String>,
    },
    /// The composer's paperclip: opens the file picker.
    Pick,
    /// Files to attach to the note: picked, or dropped on the window.
    Attach {
        ws: String,
        paths: Vec<PathBuf>,
    },
    Attached {
        ws: String,
        res: Result<Attachment, String>,
    },
    /// A file taken off the note, by its id.
    Detach(String),
    Held {
        ws: String,
        res: Result<Value, String>,
    },
    /// The files a held edit changes, as the hub reads them now.
    Current {
        ws: String,
        id: String,
        res: Result<Vec<FileCopy>, String>,
    },
    Reason(String, String),
    Decide {
        id: String,
        approve: bool,
    },
    Decided {
        ws: String,
        id: String,
        res: Result<Value, String>,
    },
    Command(String),
    Timeout(String),
    RunStart,
    Started {
        ws: String,
        res: Result<Value, String>,
    },
    Polled {
        ws: String,
        res: Result<Poll, String>,
    },
    RunStop,
    Stopped {
        ws: String,
        res: Result<Value, String>,
    },
}

impl Msg {
    /// The workspace a hub answer was asked for, if it carries one.
    fn workspace(&self) -> Option<&str> {
        match self {
            Msg::Transcript { ws, .. }
            | Msg::Steered { ws, .. }
            | Msg::Attach { ws, .. }
            | Msg::Attached { ws, .. }
            | Msg::Held { ws, .. }
            | Msg::Current { ws, .. }
            | Msg::Decided { ws, .. }
            | Msg::Started { ws, .. }
            | Msg::Polled { ws, .. }
            | Msg::Stopped { ws, .. } => Some(ws),
            _ => None,
        }
    }
}

/// The agent a workspace is, if it is an agent's (repo workspaces have no agent).
fn agent_of(ws: &WorkspaceId) -> Option<u64> {
    match ws {
        WorkspaceId::Agent(cid) => Some(*cid),
        WorkspaceId::Repo { .. } | WorkspaceId::Station(_) => None,
    }
}

// ---- Tasks ------------------------------------------------------------------------------------

fn transcript_task(client: &Client, ws: &WorkspaceId, cid: u64) -> Task<Msg> {
    let c = client.clone();
    let key = ws.to_string();
    client::task(
        move || {
            c.get::<Vec<Value>>(&format!(
                "/v1/agents/{cid}/transcript?last={TRANSCRIPT_LAST}"
            ))
        },
        move |res| Msg::Transcript {
            ws: key.clone(),
            res,
        },
    )
}

/// The system's file picker, for files to attach to a note or a message. Cancelled, it picks nothing.
pub async fn pick_files() -> Vec<PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_title("Attach files")
        .pick_files()
        .await
        .map(|files| files.iter().map(|f| f.path().to_path_buf()).collect())
        .unwrap_or_default()
}

/// The held calls of the workspace's agent (none for a repo workspace).
pub fn approvals_task(client: &Client, ws: &WorkspaceId) -> Task<Msg> {
    if agent_of(ws).is_none() {
        return Task::none();
    }
    let c = client.clone();
    let key = ws.to_string();
    client::task(
        move || c.get::<Value>("/v1/approvals"),
        move |res| Msg::Held {
            ws: key.clone(),
            res,
        },
    )
}

fn file_task(client: &Client, ws: &WorkspaceId, id: String, paths: Vec<String>) -> Task<Msg> {
    let c = client.clone();
    let w = ws.clone();
    let key = ws.to_string();
    let shown_id = id.clone();
    client::task(
        move || {
            Ok(paths
                .into_iter()
                .map(|path| {
                    let res = ide::fetch_file(&c, &w, &path);
                    (path, res)
                })
                .collect::<Vec<FileCopy>>())
        },
        move |res| Msg::Current {
            ws: key.clone(),
            id: shown_id.clone(),
            res,
        },
    )
}

/// One poll of a run, from `offset`. With `wait`, it first sleeps the poll interval.
fn poll_task(client: &Client, ws: &WorkspaceId, run: String, offset: u64, wait: bool) -> Task<Msg> {
    let path = ide::route(ws, &format!("/runs/{}?offset={offset}", ide::enc(&run)));
    let c = client.clone();
    let key = ws.to_string();
    client::task(
        move || {
            if wait {
                std::thread::sleep(POLL_EVERY);
            }
            c.get::<Poll>(&path)
        },
        move |res| Msg::Polled {
            ws: key.clone(),
            res,
        },
    )
}

/// Loads what an opened panel shows: the transcript once, the approvals each time they open.
pub fn show(st: &mut State, panel: Panel, client: &Client, ws: &WorkspaceId) -> Task<Msg> {
    let Some(cid) = agent_of(ws) else {
        return Task::none();
    };
    let tools = if matches!(panel, Panel::Agent) && !st.chat_tools.is_loaded() {
        chat_tools::load(client, chat_tools::Target::Agent(cid)).map(Msg::ChatTools)
    } else {
        Task::none()
    };
    let rest = match panel {
        Panel::Agent if !st.entries_loaded => transcript_task(client, ws, cid),
        Panel::Approvals => approvals_task(client, ws),
        _ => Task::none(),
    };
    Task::batch([tools, rest])
}

/// A transcript entry the hub published live (the workspace's agent only; the screen checks that).
pub fn push_entry(st: &mut State, entry: Value) {
    if !st.entries_loaded {
        // The load that is coming (or the one that answered before this entry) has it.
        return;
    }
    let seq = entry.get("seq").and_then(Value::as_u64).unwrap_or(0);
    let last = st
        .entries
        .last()
        .and_then(|e| e.get("seq"))
        .and_then(Value::as_u64);
    if last.is_some_and(|last| last >= seq) {
        return;
    }
    st.entries.push(entry);
    keep_newest(&mut st.entries);
}

fn keep_newest(entries: &mut Vec<Value>) {
    if entries.len() > KEEP_ENTRIES {
        let drop = entries.len() - KEEP_ENTRIES;
        entries.drain(..drop);
    }
}

/// The panels' messages: their state changes, and the tasks they start.
pub fn update(st: &mut State, msg: Msg, client: &Client, ws: &WorkspaceId) -> Task<Msg> {
    let here = ws.to_string();
    if msg.workspace().is_some_and(|w| w != here) {
        return Task::none();
    }
    // The run's inputs are the owner's own text, whatever the workspace.
    match msg {
        Msg::Command(text) => {
            st.command = text;
            return Task::none();
        }
        Msg::Timeout(text) => {
            st.timeout = text;
            return Task::none();
        }
        _ => {}
    }
    // Everything else talks to the workspace's agent; a repo workspace has none (its panels are not offered).
    let Some(cid) = agent_of(ws) else {
        return Task::none();
    };
    match msg {
        Msg::Transcript { res, .. } => {
            match res {
                Ok(list) => {
                    st.entries = list;
                    st.entries
                        .sort_by_key(|e| e.get("seq").and_then(Value::as_u64).unwrap_or(0));
                    keep_newest(&mut st.entries);
                    st.entries_loaded = true;
                    st.entries_error = None;
                }
                Err(e) => st.entries_error = Some(e),
            }
            Task::none()
        }
        Msg::Steer(action) => {
            st.steer.perform(action);
            Task::none()
        }
        Msg::SteerSend => {
            let text = st.steer.text().trim().to_string();
            if st.steer_busy {
                return Task::none();
            }
            if st.uploading > 0 {
                st.steer_notice = Some("Wait for the files to attach.".to_string());
                return Task::none();
            }
            if text.is_empty() && st.attachments.is_empty() {
                st.steer_notice = Some("Write a note first.".to_string());
                return Task::none();
            }
            if text.chars().count() > STEER_MAX {
                st.steer_notice = Some(format!("The note is over {STEER_MAX} characters."));
                return Task::none();
            }
            st.steer_busy = true;
            st.steer_notice = None;
            let c = client.clone();
            let sent: Vec<String> = st.attachments.iter().map(|a| a.id.clone()).collect();
            // Working, the text steers the work in flight; idle, it is a chat turn of its own (the hub says why when
            // the agent has no project, or its project is paused).
            let (path, body) = if st.agent_working {
                (
                    format!("/v1/agents/{cid}/control"),
                    json!({ "action": "steer", "text": text, "attachments": sent }),
                )
            } else {
                (
                    format!("/v1/agents/{cid}/chat"),
                    json!({ "text": text, "attachments": sent }),
                )
            };
            client::task(
                move || c.post::<Value>(&path, &body),
                move |res| Msg::Steered {
                    ws: here.clone(),
                    sent: sent.clone(),
                    res,
                },
            )
        }
        Msg::Steered { sent, res, .. } => {
            st.steer_busy = false;
            match res {
                Ok(answer) => {
                    st.steer = text_editor::Content::new();
                    st.attachments.retain(|a| !sent.contains(&a.id));
                    st.steer_notice = Some(
                        answer
                            .get("result")
                            .and_then(Value::as_str)
                            .unwrap_or("Note sent.")
                            .to_string(),
                    );
                }
                Err(e) => st.steer_notice = Some(format!("The note was not sent: {e}")),
            }
            st.chat_tools.refresh(client).map(Msg::ChatTools)
        }
        Msg::ChatTools(m) => chat_tools::update(&mut st.chat_tools, m, client).map(Msg::ChatTools),
        Msg::Pick => Task::perform(pick_files(), move |paths| Msg::Attach {
            ws: here.clone(),
            paths,
        }),
        Msg::Attach { paths, .. } => {
            st.uploading += paths.len();
            st.steer_notice = None;
            Task::batch(paths.into_iter().map(|path| {
                let c = client.clone();
                let key = here.clone();
                client::task(
                    move || files::attach(&c, &path),
                    move |res| Msg::Attached {
                        ws: key.clone(),
                        res,
                    },
                )
            }))
        }
        Msg::Attached { res, .. } => {
            st.uploading = st.uploading.saturating_sub(1);
            match res {
                Ok(file) => st.attachments.push(file),
                Err(e) => st.steer_notice = Some(format!("The file was not attached: {e}")),
            }
            Task::none()
        }
        Msg::Detach(id) => {
            st.attachments.retain(|a| a.id != id);
            Task::none()
        }
        Msg::Held { res, .. } => {
            st.approvals_loaded = true;
            match res {
                Ok(list) => {
                    st.held = pending_for(&list, cid);
                    st.approvals_error = None;
                    st.previews
                        .retain(|id, _| st.held.iter().any(|h| &h.id == id));
                    // Each held write reads its file now, for its diff.
                    let jobs: Vec<(String, Vec<String>)> = st
                        .held
                        .iter()
                        .filter(|h| is_write(&h.tool))
                        .map(|h| (h.id.clone(), write_paths(h)))
                        .filter(|(_, paths)| !paths.is_empty())
                        .collect();
                    let mut tasks = Vec::new();
                    for (id, paths) in jobs {
                        st.previews.insert(id.clone(), Preview::Loading);
                        tasks.push(file_task(client, ws, id, paths));
                    }
                    Task::batch(tasks)
                }
                Err(e) => {
                    st.approvals_error = Some(e);
                    Task::none()
                }
            }
        }
        Msg::Current { id, res, .. } => {
            if let Some(held) = st.held.iter().find(|h| h.id == id).cloned() {
                st.previews.insert(id, preview_of(&held, res));
            }
            Task::none()
        }
        Msg::Reason(id, text) => {
            st.reasons.insert(id, text);
            Task::none()
        }
        Msg::Decide { id, approve } => {
            if st.busy.contains(&id) {
                return Task::none();
            }
            st.busy.insert(id.clone());
            let mut body = json!({ "approve": approve });
            if let Some(reason) = st
                .reasons
                .get(&id)
                .map(|r| r.trim())
                .filter(|r| !r.is_empty())
            {
                body["reason"] = json!(reason);
            }
            let c = client.clone();
            let path = format!("/v1/approvals/{id}");
            let shown_id = id.clone();
            client::task(
                move || c.post::<Value>(&path, &body),
                move |res| Msg::Decided {
                    ws: here.clone(),
                    id: shown_id.clone(),
                    res,
                },
            )
        }
        Msg::Decided { id, res, .. } => {
            st.busy.remove(&id);
            match res {
                Ok(_) => {
                    st.reasons.remove(&id);
                    st.approvals_error = None;
                    approvals_task(client, ws)
                }
                Err(e) => {
                    st.approvals_error = Some(e);
                    Task::none()
                }
            }
        }
        // Handled above, before the workspace check.
        Msg::Command(_) | Msg::Timeout(_) => Task::none(),
        Msg::RunStart => {
            let command = st.command.trim().to_string();
            if command.is_empty() {
                st.run.error = Some("Type a command first.".to_string());
                return Task::none();
            }
            let timeout = match parse_timeout(&st.timeout) {
                Ok(timeout) => timeout,
                Err(e) => {
                    st.run.error = Some(e);
                    return Task::none();
                }
            };
            if !st.run.start(&command) {
                return Task::none();
            }
            let mut body = json!({ "command": command });
            if let Some(timeout) = timeout {
                body["timeout_s"] = json!(timeout);
            }
            let c = client.clone();
            let path = ide::route(ws, "/runs");
            client::task(
                move || c.post::<Value>(&path, &body),
                move |res| Msg::Started {
                    ws: here.clone(),
                    res,
                },
            )
        }
        Msg::Started { res, .. } => match res {
            Ok(answer) => match run_id(&answer) {
                Some(run) => {
                    st.run.started(run.clone());
                    poll_task(client, ws, run, 0, false)
                }
                None => {
                    st.run.failed("The hub did not name the run.".to_string());
                    Task::none()
                }
            },
            Err(e) => {
                st.run.failed(e);
                Task::none()
            }
        },
        Msg::Polled { res, .. } => match res {
            Ok(poll) => {
                let Some(run) = st.run.run.clone() else {
                    return Task::none();
                };
                match st.run.apply(&poll) {
                    Step::Again => poll_task(client, ws, run, st.run.offset, true),
                    Step::Finished => Task::none(),
                }
            }
            Err(e) => {
                st.run.failed(format!("The run could not be read: {e}"));
                Task::none()
            }
        },
        Msg::RunStop => {
            let Some(run) = st.run.run.clone() else {
                return Task::none();
            };
            if !st.run.stop_requested() {
                return Task::none();
            }
            let c = client.clone();
            let path = ide::route(ws, &format!("/runs/{}/stop", ide::enc(&run)));
            client::task(
                move || c.post::<Value>(&path, &json!({})),
                move |res| Msg::Stopped {
                    ws: here.clone(),
                    res,
                },
            )
        }
        Msg::Stopped { res, .. } => {
            if let Err(e) = res {
                st.run.stop_failed(format!("The stop was not sent: {e}"));
            }
            Task::none()
        }
    }
}

// ---- Views ------------------------------------------------------------------------------------

/// A line diff, drawn in mono: added lines in ok, removed in crit.
fn diff_view<'a>(lines: &'a [DiffLine], note: Option<&'a str>) -> Element<'a, Msg> {
    let p = theme::palette();
    let mut col = column![].spacing(0);
    if let Some(note) = note {
        col = col.push(ui::faint(note));
    }
    for line in lines.iter().take(DIFF_SHOWN) {
        let (mark, text, tone) = match line {
            DiffLine::Same(t) => (" ", t.as_str(), p.ink_2),
            DiffLine::Add(t) => ("+", t.as_str(), p.ok),
            DiffLine::Del(t) => ("-", t.as_str(), p.crit),
        };
        col = col.push(ui::mono(format!("{mark} {text}"), 11.5, tone));
    }
    if lines.len() > DIFF_SHOWN {
        col = col.push(ui::faint(format!(
            "{} more lines",
            lines.len() - DIFF_SHOWN
        )));
    }
    container(col)
        .padding(8)
        .width(Length::Fill)
        .style(ui::frame(None, p.rule))
        .into()
}

fn held_card<'a>(st: &'a State, held: &'a Held) -> Element<'a, Msg> {
    let p = theme::palette();
    let busy = st.busy.contains(&held.id);
    let detail: Element<'a, Msg> = match st.previews.get(&held.id) {
        Some(Preview::Loading) => ui::faint("Reading the file…").into(),
        Some(Preview::Diff { files }) => column(files.iter().map(|file| {
            column![
                ui::mono(file.path.as_str(), 11.5, p.ink_2),
                diff_view(&file.lines, file.note.as_deref())
            ]
            .spacing(6)
            .into()
        }))
        .spacing(10)
        .into(),
        Some(Preview::Unavailable(message)) => ui::faint(message.as_str()).into(),
        None => {
            let input = serde_json::to_string_pretty(&held.input).unwrap_or_default();
            ui::mono(input, 11.5, p.ink_2).into()
        }
    };
    let id = held.id.clone();
    let reason = text_input(
        "Reason (optional)",
        st.reasons.get(&held.id).map(String::as_str).unwrap_or(""),
    )
    .on_input(move |text| Msg::Reason(id.clone(), text))
    .size(13.0)
    .style(theme::input)
    .padding(6);
    let deny = ui::btn(
        "Deny",
        None,
        Kind::Danger,
        (!busy).then(|| Msg::Decide {
            id: held.id.clone(),
            approve: false,
        }),
    );
    let approve = ui::btn(
        "Approve",
        None,
        Kind::Primary,
        (!busy).then(|| Msg::Decide {
            id: held.id.clone(),
            approve: true,
        }),
    );
    let body = column![
        ui::mono(held.tool.as_str(), 12.0, p.ink),
        detail,
        reason,
        row![Space::new().width(Length::Fill), deny, approve]
            .spacing(8)
            .align_y(Alignment::Center),
    ]
    .spacing(10);
    container(body)
        .padding(12)
        .width(Length::Fill)
        .style(ui::frame(None, p.rule))
        .into()
}

/// The files a composer will send, as chips (name and size; × takes one off), and a line while some still go up.
/// None when there is nothing to show. `remove` makes the message for a file's id.
pub fn attachment_chips<'a, M: Clone + 'a>(
    files: &'a [Attachment],
    uploading: usize,
    remove: impl Fn(String) -> M + 'a,
) -> Option<Element<'a, M>> {
    if files.is_empty() && uploading == 0 {
        return None;
    }
    let p = theme::palette();
    let mut chips = row![].spacing(6).align_y(Alignment::Center);
    for file in files {
        let label = format!("{} · {}", file.name, files::size_text(file.size));
        let take = button(ui::icon("x", 11.0, p.ink_3))
            .padding(0)
            .style(move |_, _| button::Style {
                background: None,
                text_color: p.ink_3,
                ..Default::default()
            })
            .on_press(remove(file.id.clone()));
        chips = chips.push(
            container(
                row![ui::sans(label, 11.5, p.ink_2), take]
                    .spacing(6)
                    .align_y(Alignment::Center),
            )
            .padding([2, 8])
            .style(move |_| container::Style {
                background: Some(p.hover.into()),
                border: Border {
                    color: p.rule,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            }),
        );
    }
    if uploading > 0 {
        chips = chips.push(ui::faint(format!("Attaching {uploading}…")));
    }
    Some(ui::scroll_x(chips).into())
}

/// The Approvals panel: the agent's held calls, each with its input or its diff, and Approve or Deny.
pub fn approvals_view<'a>(st: &'a State, agent: bool) -> Element<'a, Msg> {
    if !agent {
        return ui::empty(
            "approval",
            "No approvals here",
            "Repo workspaces have no agent, so nothing is held for them.",
        );
    }
    let mut col = column![].spacing(12);
    if let Some(error) = &st.approvals_error {
        col = col.push(ui::error_line(error));
    }
    if !st.approvals_loaded {
        col = col.push(ui::faint("Loading approvals…"));
    } else if st.held.is_empty() {
        col = col.push(ui::faint("Nothing is held for this agent."));
    }
    for held in &st.held {
        col = col.push(held_card(st, held));
    }
    col.into()
}

/// The Run panel: the command and its timeout, Run and Stop, the status, and the output so far. `shell` is whether the
/// workspace has a shell (an agent's or a workstation's); a repository's runs come when the hub serves them.
pub fn run_view<'a>(st: &'a State, shell: bool) -> Element<'a, Msg> {
    if !shell {
        return ui::empty(
            "terminal",
            "Runs need an agent or a workstation",
            "Repo runs come when the hub serves repo workspaces.",
        );
    }
    let p = theme::palette();
    let active = st.run.active();
    let command = text_input("Command, e.g. cargo test", &st.command)
        .on_input(Msg::Command)
        .on_submit(Msg::RunStart)
        .style(theme::input)
        .padding(8);
    let timeout = text_input("Timeout in seconds (optional)", &st.timeout)
        .on_input(Msg::Timeout)
        .on_submit(Msg::RunStart)
        .style(theme::input)
        .padding(8);
    let can_run = !active && !st.command.trim().is_empty();
    let can_stop = st.run.phase == Phase::Running;
    let actions = row![
        ui::btn(
            "Run",
            Some("play"),
            Kind::Primary,
            can_run.then_some(Msg::RunStart)
        ),
        ui::btn(
            "Stop",
            Some("stop"),
            Kind::Danger,
            can_stop.then_some(Msg::RunStop)
        ),
    ]
    .spacing(8);
    let mut col = column![
        command,
        timeout,
        actions,
        ui::mono(st.run.status(), 11.5, p.ink_2)
    ]
    .spacing(10);
    if let Some(error) = &st.run.error {
        col = col.push(ui::error_line(error));
    }
    if st.run.output.is_empty() {
        col = col.push(ui::faint(if active {
            "Waiting for output…"
        } else {
            "No output yet."
        }));
    } else {
        let output = container(ui::mono(shown(&st.run.output), 11.5, p.ink))
            .padding(8)
            .width(Length::Fill)
            .style(ui::frame(None, p.rule));
        col = col.push(output);
    }
    col.into()
}

/// The terminal's tabs. A repo workspace has no shell (its VM is the agent's, and a workstation's is the station's).
/// The mouse area reports the pointer, which is how the screen knows the terminal has the keys (see
/// `ide::Msg::Pressed`).
pub fn terminal_body<'a>(tabs: Option<&'a TerminalTabs>, shell: bool) -> Element<'a, ide::Msg> {
    if !shell {
        return ui::faint("Terminals run in agent and workstation workspaces.").into();
    }
    match tabs {
        Some(tabs) => mouse_area(tabs.view().map(ide::Msg::Terminals))
            .on_enter(ide::Msg::TerminalHover(true))
            .on_exit(ide::Msg::TerminalHover(false))
            .into(),
        None => ui::faint("Starting the terminal…").into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poll(output: &str, next: u64, done: bool) -> Poll {
        Poll {
            output: output.to_string(),
            next,
            done,
            exit_code: None,
            timed_out: false,
        }
    }

    fn running() -> Run {
        let mut run = Run::default();
        assert!(run.start("cargo test"));
        run.started("r1".to_string());
        run
    }

    #[test]
    fn a_run_starts_once_and_runs_to_its_exit_code() {
        let mut run = Run::default();
        assert!(run.start("cargo test"));
        assert_eq!(run.phase, Phase::Starting);
        assert!(!run.start("again"), "a run is refused while one is active");
        run.started("r1".into());
        assert_eq!(run.run.as_deref(), Some("r1"));
        assert_eq!(run.apply(&poll("compiling\n", 10, false)), Step::Again);
        assert_eq!(run.offset, 10);
        let last = Poll {
            output: "ok\n".into(),
            next: 13,
            done: true,
            exit_code: Some(0),
            timed_out: false,
        };
        assert_eq!(run.apply(&last), Step::Finished);
        assert_eq!(run.output, "compiling\nok\n");
        assert_eq!(run.offset, 13);
        assert_eq!(
            run.phase,
            Phase::Done {
                exit_code: Some(0),
                timed_out: false,
                stopped: false
            }
        );
        assert_eq!(run.status(), "Exited with code 0.");
        assert!(run.start("next"), "a finished run lets the next one start");
        assert_eq!(run.output, "", "a new run starts with no output");
    }

    #[test]
    fn run_polls_keep_the_offset_and_the_end_state() {
        // Original: a_poll_that_moves_back_keeps_the_offset
        {
            let mut run = running();
            run.apply(&poll("abc", 9, false));
            run.apply(&poll("", 4, false));
            assert_eq!(run.offset, 9);
        }
        // Original: a_timeout_is_reported_as_one
        {
            let mut run = running();
            let timed = Poll {
                output: String::new(),
                next: 0,
                done: true,
                exit_code: None,
                timed_out: true,
            };
            assert_eq!(run.apply(&timed), Step::Finished);
            assert_eq!(run.status(), "Timed out.");
        }
        // Original: a_late_poll_after_the_run_ended_changes_nothing
        {
            let mut run = running();
            run.apply(&Poll {
                exit_code: Some(0),
                ..poll("done\n", 5, true)
            });
            assert_eq!(run.apply(&poll("late", 9, false)), Step::Finished);
            assert_eq!(run.output, "done\n");
            assert_eq!(run.offset, 5);
        }
    }

    #[test]
    fn a_stop_asked_once_and_ended_by_the_hub_reads_as_stopped() {
        // Only a running run can be stopped: no run yet, or one still starting, has no id to stop.
        assert!(!Run::default().stop_requested());
        let mut starting = Run::default();
        starting.start("x");
        assert!(!starting.stop_requested(), "no run id yet to stop");

        let mut run = running();
        assert!(run.stop_requested());
        assert!(!run.stop_requested(), "a second stop is not sent");
        assert_eq!(run.phase, Phase::Stopping);
        assert_eq!(
            run.apply(&Poll {
                exit_code: Some(-15),
                ..poll("", 0, true)
            }),
            Step::Finished
        );
        assert_eq!(run.status(), "Stopped.");
    }

    #[test]
    fn a_refused_stop_leaves_the_run_running() {
        let mut run = running();
        assert!(run.stop_requested());
        run.stop_failed("no session".into());
        assert_eq!(run.phase, Phase::Running);
        assert_eq!(run.error.as_deref(), Some("no session"));
        assert_eq!(
            run.apply(&poll("", 0, false)),
            Step::Again,
            "the run is still polled"
        );
    }

    #[test]
    fn a_failed_start_or_lost_run_is_idle_with_the_error() {
        let mut run = Run::default();
        run.start("cargo test");
        run.failed("HTTP 409".into());
        assert_eq!(run.phase, Phase::Idle);
        assert_eq!(run.error.as_deref(), Some("HTTP 409"));
        assert!(run.start("cargo test"), "the owner can try again");
        assert_eq!(run.error, None, "a new run clears the old error");
    }

    #[test]
    fn run_ids_come_as_text_or_as_numbers() {
        assert_eq!(run_id(&json!({"run": "r7"})).as_deref(), Some("r7"));
        assert_eq!(run_id(&json!({"run": 7})).as_deref(), Some("7"));
        assert_eq!(run_id(&json!({})), None);
    }

    #[test]
    fn the_timeout_is_empty_or_whole_seconds() {
        assert_eq!(parse_timeout("  "), Ok(None));
        assert_eq!(parse_timeout(" 90 "), Ok(Some(90)));
        assert!(parse_timeout("1.5").is_err());
        assert!(parse_timeout("-3").is_err());
    }

    #[test]
    fn a_write_previews_its_content_and_an_edit_needs_one_match() {
        let create = json!({"create": [{"path": "a.txt", "content": "new\n"}]});
        assert_eq!(
            proposed("edit", &create, "a.txt", "old\n"),
            Some(Ok("new\n".to_string()))
        );
        let edit_ok = json!({"edits": [{"path": "a.txt", "old": "two", "new": "2"}]});
        assert_eq!(
            proposed("edit", &edit_ok, "a.txt", "one two three\n"),
            Some(Ok("one 2 three\n".to_string()))
        );
        assert!(
            proposed("edit", &edit_ok, "a.txt", "nothing here\n")
                .unwrap()
                .is_err()
        );
        assert!(
            proposed(
                "edit",
                &json!({"edits": [{"path": "a", "old": "a", "new": "b"}]}),
                "a",
                "a a\n"
            )
            .unwrap()
            .is_err()
        );
        let all = json!({"edits": [{"path": "a", "old": "a", "new": "b", "replace_all": true}]});
        assert_eq!(
            proposed("edit", &all, "a", "a a\n"),
            Some(Ok("b b\n".to_string()))
        );
        assert_eq!(
            proposed("run", &json!({"command": "ls"}), "a.txt", ""),
            None,
            "a call that is not an edit has no preview"
        );
    }

    #[test]
    fn an_edit_previews_each_file_it_names() {
        let call = json!({
            "create": [{"path": "b.txt", "content": "x\n"}],
            "edits": [
                {"path": "a.txt", "old": "one", "new": "1"},
                {"path": "a.txt", "old": "two", "new": "2"},
            ],
        });
        let held = Held {
            id: "h1".to_string(),
            tool: "edit".to_string(),
            input: call,
        };
        assert_eq!(write_paths(&held), vec!["b.txt", "a.txt"]);
        assert_eq!(
            proposed("edit", &held.input, "a.txt", "one two\n"),
            Some(Ok("1 2\n".to_string()))
        );
        assert_eq!(
            proposed("edit", &held.input, "b.txt", "old\n"),
            Some(Ok("x\n".to_string()))
        );
    }

    #[test]
    fn the_pending_list_keeps_this_agents_tool_calls() {
        let list = json!([
            {"id": "a1", "cid": 7, "state": "pending", "tool": "edit", "input": {"create": [{"path": "x"}]}},
            {"id": "a2", "cid": 8, "state": "pending", "tool": "run", "input": {}},
            {"id": "a3", "cid": 7, "state": "approved", "tool": "run", "input": {}},
            {"id": "a4", "cid": 7, "state": "pending", "kind": "artifact.share", "tool": "", "input": {}},
        ]);
        let held = pending_for(&list, 7);
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].id, "a1");
        assert_eq!(held[0].tool, "edit");
    }

    #[test]
    fn push_entry_waits_for_the_load_and_skips_seen_entries() {
        let mut st = State::default();
        push_entry(&mut st, json!({"seq": 1, "kind": "work_start"}));
        assert!(
            st.entries.is_empty(),
            "nothing is kept before the transcript loads"
        );
        st.entries_loaded = true;
        st.entries.push(json!({"seq": 2, "kind": "model_response"}));
        push_entry(&mut st, json!({"seq": 2, "kind": "model_response"}));
        push_entry(&mut st, json!({"seq": 3, "kind": "error"}));
        assert_eq!(st.entries.len(), 2);
    }
}
