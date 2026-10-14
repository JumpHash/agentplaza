//! The IDE's right panels (Changes lives on the left): the agent's transcript with a steer box, the agent's held
//! approvals (a write's diff against the file now), a bounded run with polled output, and the terminal (a shell
//! in the agent's VM, on the side or docked under the editor). The screen (`ide.rs`) owns the workspace and
//! routes the messages; this module owns its panels' state, their hub calls and their views.
//!
//! The hub answers each call with the workspace it was asked for, so a late answer for a workspace
//! the owner has left is dropped (the same rule as the screen's `is_current`).

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use apz_ide::WorkspaceId;
use apz_ide::state::Panel;
use iced::widget::{Space, column, container, mouse_area, row, text_editor, text_input};
use iced::{Alignment, Element, Length, Task};
use serde::Deserialize;
use serde_json::{Value, json};

use super::ide::{self, FileView};
use super::ide_terminal::TerminalTabs;
use crate::console::client::{self, Client};
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

/// What a held write would change in its file.
#[derive(Debug, Clone, PartialEq)]
pub enum Preview {
    /// The file's current text is on its way.
    Loading,
    /// The file now against the text the write would leave. `note` says when there was no copy to compare with.
    Diff {
        path: String,
        lines: Vec<DiffLine>,
        note: Option<String>,
    },
    /// No diff can be drawn: an edit that does not match the file, or a file that is not text.
    Unavailable(String),
}

/// Whether `tool` writes a file the panel can preview.
pub fn is_write(tool: &str) -> bool {
    matches!(tool, "write_file" | "edit_file" | "append_file")
}

/// The file a held write changes, from its input.
fn write_path(held: &Held) -> Option<String> {
    held.input
        .get("path")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The text a held write leaves in its file, given the file's current text. None for a call that is not
/// a write; `Err` for an edit that does not apply to the file as it is now.
pub fn proposed(tool: &str, input: &Value, current: &str) -> Option<Result<String, String>> {
    let text = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_string);
    match tool {
        "write_file" => {
            Some(text("content").ok_or_else(|| "The write has no content.".to_string()))
        }
        "append_file" => Some(
            text("content")
                .map(|add| format!("{current}{add}"))
                .ok_or_else(|| "The append has no content.".to_string()),
        ),
        "edit_file" => Some(edit(current, text("old"), text("new"))),
        _ => None,
    }
}

/// The result of an edit: its old text must occur exactly once in the file.
fn edit(current: &str, old: Option<String>, new: Option<String>) -> Result<String, String> {
    let (Some(old), Some(new)) = (old, new) else {
        return Err("The edit has no old or new text.".to_string());
    };
    if old.is_empty() {
        return Err("The edit replaces empty text.".to_string());
    }
    match current.matches(&old).count() {
        0 => Err("The text to replace is not in the file.".to_string()),
        1 => Ok(current.replacen(&old, &new, 1)),
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

/// The held calls of agent `cid` that still wait, from `GET /v1/approvals`. Tool calls only: sanctions and
/// payments are decided on the Approvals screen.
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

/// The preview of a held write, from the file's copy the hub gave (`res`).
fn preview_of(held: &Held, res: Result<FileView, String>) -> Preview {
    let path = write_path(held).unwrap_or_default();
    let (current, note) = match res {
        Ok(file) => match file.text {
            Some(text) => (text, None),
            None => {
                return Preview::Unavailable("The file is not text, so it has no diff.".to_string());
            }
        },
        Err(e) => (
            String::new(),
            Some(format!(
                "No readable copy of the file ({e}); the diff shows the whole text as new."
            )),
        ),
    };
    match proposed(&held.tool, &held.input, &current) {
        Some(Ok(text)) => Preview::Diff {
            path,
            lines: line_diff(&current, &text),
            note,
        },
        Some(Err(e)) => Preview::Unavailable(e),
        None => Preview::Unavailable("This call does not write a file.".to_string()),
    }
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
    Steered {
        ws: String,
        res: Result<Value, String>,
    },
    Held {
        ws: String,
        res: Result<Value, String>,
    },
    /// The file a held write changes, as the hub reads it now.
    Current {
        ws: String,
        id: String,
        res: Result<FileView, String>,
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
        WorkspaceId::Repo { .. } => None,
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

fn file_task(client: &Client, ws: &WorkspaceId, id: String, path: String) -> Task<Msg> {
    let c = client.clone();
    let w = ws.clone();
    let key = ws.to_string();
    let shown_id = id.clone();
    client::task(
        move || ide::fetch_file(&c, &w, &path),
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
    match panel {
        Panel::Agent if !st.entries_loaded => transcript_task(client, ws, cid),
        Panel::Approvals => approvals_task(client, ws),
        _ => Task::none(),
    }
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
            if text.is_empty() {
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
            let path = format!("/v1/agents/{cid}/control");
            let body = json!({ "action": "steer", "text": text });
            client::task(
                move || c.post::<Value>(&path, &body),
                move |res| Msg::Steered {
                    ws: here.clone(),
                    res,
                },
            )
        }
        Msg::Steered { res, .. } => {
            st.steer_busy = false;
            match res {
                Ok(answer) => {
                    st.steer = text_editor::Content::new();
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
                    let jobs: Vec<(String, String)> = st
                        .held
                        .iter()
                        .filter(|h| is_write(&h.tool))
                        .filter_map(|h| write_path(h).map(|path| (h.id.clone(), path)))
                        .collect();
                    let mut tasks = Vec::new();
                    for (id, path) in jobs {
                        st.previews.insert(id.clone(), Preview::Loading);
                        tasks.push(file_task(client, ws, id, path));
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
        Some(Preview::Diff { path, lines, note }) => column![
            ui::mono(path.as_str(), 11.5, p.ink_2),
            diff_view(lines, note.as_deref())
        ]
        .spacing(6)
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

/// The Run panel: the command and its timeout, Run and Stop, the status, and the output so far.
pub fn run_view<'a>(st: &'a State, agent: bool) -> Element<'a, Msg> {
    if !agent {
        return ui::empty(
            "terminal",
            "Runs need an agent workspace",
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

/// The terminal's tabs. Repo workspaces have no VM, so they have no shell. The mouse area reports the
/// pointer, which is how the screen knows the terminal has the keys (see `ide::Msg::Pressed`).
pub fn terminal_body<'a>(tabs: Option<&'a TerminalTabs>, agent: bool) -> Element<'a, ide::Msg> {
    if !agent {
        return ui::faint("Terminals run in agent workspaces.").into();
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
    fn a_poll_that_moves_back_keeps_the_offset() {
        let mut run = running();
        run.apply(&poll("abc", 9, false));
        run.apply(&poll("", 4, false));
        assert_eq!(run.offset, 9);
    }

    #[test]
    fn a_timeout_is_reported_as_one() {
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

    #[test]
    fn a_stop_asked_once_and_ended_by_the_hub_reads_as_stopped() {
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
    fn only_a_running_run_can_be_stopped() {
        let mut idle = Run::default();
        assert!(!idle.stop_requested());
        let mut starting = Run::default();
        starting.start("x");
        assert!(!starting.stop_requested(), "no run id yet to stop");
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
    fn a_late_poll_after_the_run_ended_changes_nothing() {
        let mut run = running();
        run.apply(&Poll {
            exit_code: Some(0),
            ..poll("done\n", 5, true)
        });
        assert_eq!(run.apply(&poll("late", 9, false)), Step::Finished);
        assert_eq!(run.output, "done\n");
        assert_eq!(run.offset, 5);
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
    fn shown_keeps_the_newest_characters() {
        assert_eq!(shown("short"), "short");
        let long: String = "a".repeat(SHOWN_CHARS) + "tail";
        let kept = shown(&long);
        assert_eq!(kept.chars().count(), SHOWN_CHARS);
        assert!(kept.ends_with("tail"));
    }

    #[test]
    fn a_write_previews_its_content_and_an_edit_needs_one_match() {
        let write = json!({"path": "a.txt", "content": "new\n"});
        assert_eq!(
            proposed("write_file", &write, "old\n"),
            Some(Ok("new\n".to_string()))
        );
        let append = json!({"path": "a.txt", "content": "more\n"});
        assert_eq!(
            proposed("append_file", &append, "one\n"),
            Some(Ok("one\nmore\n".to_string()))
        );
        let edit_ok = json!({"path": "a.txt", "old": "two", "new": "2"});
        assert_eq!(
            proposed("edit_file", &edit_ok, "one two three\n"),
            Some(Ok("one 2 three\n".to_string()))
        );
        assert!(
            proposed("edit_file", &edit_ok, "nothing here\n")
                .unwrap()
                .is_err()
        );
        assert!(
            proposed(
                "edit_file",
                &json!({"path": "a", "old": "a", "new": "b"}),
                "a a\n"
            )
            .unwrap()
            .is_err()
        );
        assert_eq!(
            proposed("bash", &json!({}), ""),
            None,
            "a call that is not a write has no preview"
        );
    }

    #[test]
    fn a_line_diff_marks_removed_and_added_lines() {
        let lines = line_diff("a\nb\nc", "a\nx\nc");
        assert_eq!(
            lines,
            vec![
                DiffLine::Same("a".into()),
                DiffLine::Del("b".into()),
                DiffLine::Add("x".into()),
                DiffLine::Same("c".into()),
            ]
        );
        assert_eq!(line_diff("", "new"), vec![DiffLine::Add("new".into())]);
        assert_eq!(line_diff("gone", ""), vec![DiffLine::Del("gone".into())]);
    }

    #[test]
    fn the_pending_list_keeps_this_agents_tool_calls() {
        let list = json!([
            {"id": "a1", "cid": 7, "state": "pending", "tool": "write_file", "input": {"path": "x"}},
            {"id": "a2", "cid": 8, "state": "pending", "tool": "bash", "input": {}},
            {"id": "a3", "cid": 7, "state": "approved", "tool": "bash", "input": {}},
            {"id": "a4", "cid": 7, "state": "pending", "kind": "sanction", "tool": "", "input": {}},
        ]);
        let held = pending_for(&list, 7);
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].id, "a1");
        assert_eq!(held[0].tool, "write_file");
    }

    #[test]
    fn a_missing_copy_of_a_write_shows_the_whole_text_as_new() {
        let held = Held {
            id: "a1".into(),
            tool: "write_file".into(),
            input: json!({"path": "new.txt", "content": "hi\n"}),
        };
        match preview_of(&held, Err("HTTP 404".into())) {
            Preview::Diff { path, lines, note } => {
                assert_eq!(path, "new.txt");
                assert_eq!(lines, vec![DiffLine::Add("hi".into())]);
                assert!(note.is_some());
            }
            other => panic!("expected a diff, got {other:?}"),
        }
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
