//! Helpers (PLAN 7.3): a session starts a helper for a bounded job, and the helper runs in the session's own VM as a
//! second agent loop with its own conversation and its own work-id space. The session never waits on the job unless it
//! calls `await_helper`; the helper's report is its final answer, capped, not its transcript.
//!
//! A helper's model calls go to the hub like the session's, tagged with the helper's id (`ModelCall.helper`), so the
//! hub holds them on the session's budget. A helper runs only the tools its job is given (read, find, search and the
//! code-intelligence tools; with `edits`, also edit, run and run_tests), and only those the VM's policy allows. It is
//! never offered `spawn_helper` or `await_helper`: depth is one. A helper that edits works in its own git worktree
//! under `.helpers/<id>` on the branch `helper/<id>`; the session merges what it keeps.
//!
//! The helpers of a fork run on threads of the agent's scope. At most [`cap_for`] of them run at once; the rest wait
//! in the order they were started. A helper ends with its session's work item: one still running is stopped.

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, Scope};
use std::time::{Duration, Instant};

use apz_proto::chat::{
    Block, ChatRequest, ChatResponse, Message, Role, SystemPart, ToolSpec, blocks_text,
};
use apz_proto::wire::{
    AgentProfile, GuestMessage, HostMessage, MAX_TOOL_OUTPUT, MODEL_PAUSED, PolicySnapshot,
    cap_output,
};
use serde_json::Value;

use crate::agent::{self, paused_wait, run_guest_tool};
use crate::files::Workspace;
use crate::lane::{Shared, model_key};

/// The directory under the workspace where helpers' worktrees are made.
pub const HELPERS_DIR: &str = ".helpers";
/// The tools of a helper that does not edit: reads and code intelligence, never a command or a write.
const READ_TOOLS: [&str; 8] = [
    "read",
    "find",
    "search",
    "symbols",
    "definition",
    "references",
    "diagnostics",
    "outline",
];
/// The tools a helper that edits adds to [`READ_TOOLS`].
const EDIT_TOOLS: [&str; 3] = ["edit", "run", "run_tests"];
/// The tools a helper is never offered, and refuses if it asks: depth is one.
const HELPER_TOOLS: [&str; 2] = ["spawn_helper", "await_helper"];
/// The report's size when the job names none: about 2 000 tokens (4 bytes a token).
const DEFAULT_REPORT_TOKENS: u64 = 2_000;
/// The largest report a job may ask for.
const MAX_REPORT_TOKENS: u64 = 8_000;
const BYTES_PER_TOKEN: u64 = 4;
/// How long `await_helper` waits when it names no time.
const DEFAULT_AWAIT: Duration = Duration::from_secs(300);
/// The longest `await_helper` may wait in one call.
const MAX_AWAIT: Duration = Duration::from_secs(3_600);
/// How often a helper's wait checks whether it was stopped.
const POLL: Duration = Duration::from_millis(20);
/// The base of the request ids helpers use. The agent loop counts its own from 1, so the two never meet.
const REQ_BASE: u64 = 1 << 40;
/// The base of the work ids a helper's local tools run under (their read tracking is per work item).
const WORK_BASE: u64 = 1 << 62;
/// The most helpers a fork runs at once: half its CPUs, at least one.
pub fn cap_for(parallelism: usize) -> usize {
    (parallelism / 2).max(1)
}

/// How a helper ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Done,
    Failed,
    Stopped,
}

/// What a helper reports when it ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub status: Status,
    pub text: String,
    /// The branch its edits were committed to, when it edited.
    pub branch: Option<String>,
    /// The diff stat of that branch against the commit the fork was at when the helper started.
    pub stat: Option<String>,
}

impl Report {
    fn new(status: Status, text: impl Into<String>) -> Report {
        Report {
            status,
            text: text.into(),
            branch: None,
            stat: None,
        }
    }
}

/// What the dispatcher is told. `Start` carries everything a helper needs, so it runs on a thread of its own.
pub enum Event {
    Start(Box<Start>),
    /// A helper's thread ended.
    Ended,
    /// The agent loop is done: queued helpers never start.
    Close,
}

/// One helper, as started.
pub struct Start {
    pub id: u64,
    pub job: String,
    pub edits: bool,
    /// The report's size in bytes.
    pub report_bytes: usize,
    /// The parent's work item the helper's calls are reported under.
    pub work: u64,
    /// The VM's policy snapshot: the helper's local tools run only if it allows them.
    pub policy: Option<PolicySnapshot>,
    pub system_parts: Vec<SystemPart>,
    /// The tools the helper is offered: the parent's profile tools that its job allows.
    pub tools: Vec<ToolSpec>,
    pub max_turns: u32,
}

#[derive(Default)]
struct State {
    /// The last helper id given.
    last: u64,
    /// The next request id a helper's model call takes.
    next_req: u64,
    helpers: BTreeMap<u64, Record>,
}

struct Record {
    work: u64,
    cancelled: bool,
    phase: Phase,
}

enum Phase {
    Queued,
    Running,
    Ended(Report),
}

/// The helpers of one agent connection: started, awaited and stopped here. Shared between the agent loop and the
/// threads that run its tools (and the plaza MCP server, which reaches it through [`Board::spawn`] and
/// [`Board::await_report`]).
pub struct Board {
    state: Mutex<State>,
    changed: Condvar,
    events: Mutex<Sender<Event>>,
    cap: usize,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Board {
    /// A board that runs at most `cap` helpers at once, and the receiver its dispatcher reads.
    pub fn new(cap: usize) -> (Arc<Board>, Receiver<Event>) {
        let (events, rx) = mpsc::channel();
        let state = State {
            next_req: REQ_BASE,
            ..State::default()
        };
        let board = Board {
            state: Mutex::new(state),
            changed: Condvar::new(),
            events: Mutex::new(events),
            cap: cap.max(1),
        };
        (Arc::new(board), rx)
    }

    fn send(&self, event: Event) -> Result<(), String> {
        locked(&self.events)
            .send(event)
            .map_err(|_| "helpers are not running".to_string())
    }

    /// Starts a helper for `input` (`job`, and optionally `edits`, `model`, `max_report_tokens`) in the work item `work`
    /// of a VM running `profile`. Returns at once with the helper's id.
    pub fn spawn(
        &self,
        work: u64,
        profile: &AgentProfile,
        policy: Option<PolicySnapshot>,
        input: &Value,
    ) -> Result<String, String> {
        let job = input["job"]
            .as_str()
            .map(str::trim)
            .filter(|job| !job.is_empty())
            .ok_or("INVALID_INPUT: spawn_helper needs a job")?
            .to_string();
        let edits = match input.get("edits") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(edits)) => *edits,
            Some(_) => return Err("INVALID_INPUT: edits must be true or false".into()),
        };
        if let Some(model) = input.get("model").and_then(Value::as_str)
            && model != profile.model
        {
            return Err(format!(
                "a helper runs on the model its step allows: {}",
                profile.model
            ));
        }
        let tokens = match input.get("max_report_tokens") {
            None | Some(Value::Null) => DEFAULT_REPORT_TOKENS,
            Some(value) => value
                .as_u64()
                .filter(|tokens| (1..=MAX_REPORT_TOKENS).contains(tokens))
                .ok_or(format!(
                    "INVALID_INPUT: max_report_tokens must be between 1 and {MAX_REPORT_TOKENS}"
                ))?,
        };
        let allowed: Vec<&str> = if edits {
            READ_TOOLS
                .iter()
                .chain(EDIT_TOOLS.iter())
                .copied()
                .collect()
        } else {
            READ_TOOLS.to_vec()
        };
        let tools: Vec<ToolSpec> = profile
            .tools
            .iter()
            .filter(|tool| allowed.contains(&tool.name.as_str()))
            .map(|tool| ToolSpec {
                deferred: false,
                ..tool.clone()
            })
            .collect();
        let id = {
            let mut state = locked(&self.state);
            state.last += 1;
            let id = state.last;
            state.helpers.insert(
                id,
                Record {
                    work,
                    cancelled: false,
                    phase: Phase::Queued,
                },
            );
            id
        };
        self.send(Event::Start(Box::new(Start {
            id,
            job,
            edits,
            report_bytes: (tokens * BYTES_PER_TOKEN) as usize,
            work,
            policy,
            system_parts: profile.system_parts.clone(),
            tools,
            max_turns: profile.max_turns,
        })))?;
        let isolation = if edits {
            format!(
                "It edits its own worktree on branch helper/h{id}; merge what you keep with run."
            )
        } else {
            "It only reads.".to_string()
        };
        Ok(format!(
            "started helper h{id}; it runs alongside you. {isolation} await_helper with id h{id} gives its report."
        ))
    }

    /// The report of the helper `input` names (`id`, and optionally `timeout_s`): at once if it ended, else after the
    /// wait, or "still running" if it has not ended by then.
    pub fn await_report(&self, input: &Value) -> Result<String, String> {
        let id = helper_id(input).ok_or("INVALID_INPUT: await_helper needs the id of a helper")?;
        let timeout = match input.get("timeout_s") {
            None | Some(Value::Null) => DEFAULT_AWAIT,
            Some(value) => value
                .as_u64()
                .map(|secs| Duration::from_secs(secs).min(MAX_AWAIT))
                .ok_or("INVALID_INPUT: timeout_s must be a number of seconds")?,
        };
        let deadline = Instant::now() + timeout;
        let mut state = locked(&self.state);
        loop {
            let record = state
                .helpers
                .get(&id)
                .ok_or_else(|| format!("no helper h{id}"))?;
            if let Phase::Ended(report) = &record.phase {
                return Ok(render(id, report));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(format!("helper h{id} is still running"));
            }
            state = self
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Stops the helpers of work item `work` that have not ended.
    pub fn cancel_work(&self, work: u64) {
        let mut state = locked(&self.state);
        for record in state.helpers.values_mut() {
            if record.work == work && !matches!(record.phase, Phase::Ended(_)) {
                record.cancelled = true;
            }
        }
    }

    /// Ends the dispatcher: the agent loop is done.
    pub fn close(&self) {
        let _ = self.send(Event::Close);
    }

    fn is_cancelled(&self, id: u64) -> bool {
        locked(&self.state)
            .helpers
            .get(&id)
            .is_none_or(|record| record.cancelled)
    }

    fn mark_running(&self, id: u64) {
        if let Some(record) = locked(&self.state).helpers.get_mut(&id) {
            record.phase = Phase::Running;
        }
    }

    fn next_req(&self) -> u64 {
        let mut state = locked(&self.state);
        state.next_req += 1;
        state.next_req
    }

    /// Ends helper `id` with `report`: its thread calls this, and so does a test that stands in for the dispatcher.
    pub(crate) fn finish(&self, id: u64, report: Report) {
        if let Some(record) = locked(&self.state).helpers.get_mut(&id) {
            record.phase = Phase::Ended(report);
        }
        self.changed.notify_all();
        let _ = self.send(Event::Ended);
    }
}

/// Runs `spawn_helper` or `await_helper` for work item `work` on `board`, as the agent's own loop does, and any
/// other name is not a helper tool. The agent's loop and the plaza's MCP server (for a vendor engine, through the hub's
/// gate) both answer through here, so a helper is started and awaited the same way wherever it is asked for.
pub fn run_tool(
    board: &Board,
    work: u64,
    profile: Option<&AgentProfile>,
    policy: Option<PolicySnapshot>,
    name: &str,
    input: &Value,
) -> Result<String, String> {
    match (name, profile) {
        ("spawn_helper", Some(profile)) => board.spawn(work, profile, policy, input),
        ("spawn_helper", None) => Err("this agent has no model".to_string()),
        ("await_helper", _) => board.await_report(input),
        _ => Err(format!("{name} is not a helper tool")),
    }
}

/// The id of the helper `input` names: `h<n>`, or a bare number.
fn helper_id(input: &Value) -> Option<u64> {
    match input.get("id")? {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.strip_prefix('h').unwrap_or(text.as_str()).parse().ok(),
        _ => None,
    }
}

/// A helper's report as the session reads it.
fn render(id: u64, report: &Report) -> String {
    let status = match report.status {
        Status::Done => "ended",
        Status::Failed => "failed",
        Status::Stopped => "stopped",
    };
    let mut out = format!("helper h{id} {status}: {}", report.text);
    if let Some(branch) = &report.branch {
        out.push_str(&format!("\nbranch: {branch}"));
    }
    if let Some(stat) = &report.stat {
        out.push_str(&format!("\n{stat}"));
    }
    out
}

/// Runs the helpers the board starts, at most its cap at once, until the board is closed. Each helper runs on a thread
/// of `scope`; a helper's thread ends with its report, and the next queued helper starts then.
pub fn dispatch<'s, 'e: 's, 'l, S: Write + Send, W: Write + Send>(
    scope: &'s Scope<'s, 'e>,
    events: Receiver<Event>,
    board: Arc<Board>,
    shared: &'s Shared<'l, S, W>,
    workspace: &'s Workspace,
) {
    let mut queue: VecDeque<Box<Start>> = VecDeque::new();
    let mut running = 0usize;
    for event in events {
        match event {
            Event::Start(start) => queue.push_back(start),
            Event::Ended => running = running.saturating_sub(1),
            Event::Close => break,
        }
        while running < board.cap {
            let Some(start) = queue.pop_front() else {
                break;
            };
            running += 1;
            let board = Arc::clone(&board);
            scope.spawn(move || {
                let id = start.id;
                let report = if board.is_cancelled(id) {
                    Report::new(Status::Stopped, "stopped before it started")
                } else {
                    board.mark_running(id);
                    run_helper(shared, workspace, &board, *start)
                };
                shared.note(&format!("agent-harness: helper h{id} {:?}", report.status));
                board.finish(id, report);
            });
        }
    }
    // Closed: the helpers that never started end stopped.
    for start in queue {
        board.finish(
            start.id,
            Report::new(Status::Stopped, "the agent stopped before it started"),
        );
    }
}

/// Runs one helper to its report: its own worktree when it edits, its conversation, and its local tools. The report's
/// text is capped to the size the job asked for.
fn run_helper<S: Write + Send, W: Write + Send>(
    shared: &Shared<'_, S, W>,
    workspace: &Workspace,
    board: &Board,
    start: Start,
) -> Report {
    let mut report = if start.edits {
        edit_report(shared, workspace, board, &start)
    } else {
        converse(shared, workspace, board, &start)
    };
    report.text = cap_output(&report.text, start.report_bytes);
    report
}

/// A helper that edits: its conversation in its own worktree, then its edits committed on its branch.
fn edit_report<S: Write + Send, W: Write + Send>(
    shared: &Shared<'_, S, W>,
    workspace: &Workspace,
    board: &Board,
    start: &Start,
) -> Report {
    let made = match worktree(workspace, start.id) {
        Ok(made) => made,
        Err(err) => {
            return Report::new(Status::Failed, format!("its worktree was not made: {err}"));
        }
    };
    let mut report = converse(shared, &Workspace::at(&made.dir), board, start);
    if let Err(err) = commit(&made.dir, &start.job) {
        report.status = Status::Failed;
        report.text = format!("its edits were not committed: {err}");
    }
    report.stat = diff_stat(workspace.dir(), &made.base, &made.branch).ok();
    report.branch = Some(made.branch);
    report
}

/// The helper's model loop: call the model, run the tools it asks for, repeat until it answers without one.
fn converse<S: Write + Send, W: Write + Send>(
    shared: &Shared<'_, S, W>,
    ws: &Workspace,
    board: &Board,
    start: &Start,
) -> Report {
    let prompt = format!(
        "{}\n\nYou are a helper: the session that started you reads only your final answer. Keep it under {} tokens and say what you found or changed.",
        start.job,
        start.report_bytes as u64 / BYTES_PER_TOKEN
    );
    let mut messages = vec![Message::user_text(prompt)];
    let mut turns = 0u32;
    let mut paused = 0u32;
    let mut last_text = String::new();
    loop {
        if board.is_cancelled(start.id) || shared.stopping() {
            return Report::new(Status::Stopped, "the work item ended");
        }
        if turns >= start.max_turns {
            return Report::new(
                Status::Failed,
                format!("turn budget reached; last said: {last_text}"),
            );
        }
        turns += 1;
        let req = board.next_req();
        let request = ChatRequest {
            model: String::new(),
            system: None,
            system_parts: start.system_parts.clone(),
            messages: messages.clone(),
            tools: start.tools.clone(),
            cache_last_message: true,
            max_tokens: 0,
            temperature: None,
            effort: None,
        };
        let answer = match model_call(shared, board, start, req, request) {
            Ok(answer) => answer,
            Err(stopped) => return stopped,
        };
        let response = match answer {
            Ok(response) => {
                paused = 0;
                response
            }
            // The hub holds the session's model calls (its budget, a limit, the plaza is held): wait and ask again,
            // without spending a turn, until a cancel or a stop.
            Err(reason) if reason.starts_with(MODEL_PAUSED) => {
                turns -= 1;
                let wait = paused_wait(paused);
                paused += 1;
                let until = Instant::now() + wait;
                while Instant::now() < until && !board.is_cancelled(start.id) && !shared.stopping()
                {
                    thread::sleep(POLL);
                }
                continue;
            }
            Err(reason) => return Report::new(Status::Failed, reason),
        };
        let ChatResponse { content, .. } = response;
        let said = blocks_text(&content);
        if !said.trim().is_empty() {
            last_text = said.clone();
        }
        let uses: Vec<(String, String, Value)> = content
            .iter()
            .filter_map(|block| match block {
                Block::ToolUse {
                    id, name, input, ..
                } => Some((id.clone(), name.clone(), input.clone())),
                _ => None,
            })
            .collect();
        messages.push(Message::assistant(content));
        if uses.is_empty() {
            return Report::new(Status::Done, said);
        }
        let results: Vec<Block> = uses
            .into_iter()
            .map(|(id, name, input)| {
                let (content, is_error) = helper_tool(shared, ws, start, &id, &name, &input);
                Block::ToolResult {
                    tool_use_id: id,
                    content,
                    is_error,
                }
            })
            .collect();
        messages.push(Message {
            role: Role::User,
            content: results,
        });
    }
}

/// One model call of a helper: sent to the hub tagged with the helper, and answered under its request id. The
/// outer `Err` is the helper ended (stopped, or the connection is gone); the inner one is the hub's answer.
fn model_call<S: Write + Send, W: Write + Send>(
    shared: &Shared<'_, S, W>,
    board: &Board,
    start: &Start,
    req: u64,
    request: ChatRequest,
) -> Result<Result<ChatResponse, String>, Report> {
    // Registered before the call goes out, so the answer cannot arrive before anyone waits for it.
    let answers = shared.nested().register(&model_key(req));
    let sent = shared.say(&GuestMessage::ModelCall {
        req,
        work: start.work,
        request,
        helper: Some(start.id),
    });
    if sent.is_err() {
        shared.nested().unregister(&model_key(req));
        return Err(Report::new(Status::Failed, "the host connection is gone"));
    }
    loop {
        match answers.recv_timeout(POLL) {
            Ok(HostMessage::ModelResult {
                ok,
                response,
                error,
                ..
            }) => {
                shared.nested().unregister(&model_key(req));
                return Ok(match (ok, response) {
                    (true, Some(response)) => Ok(response),
                    (true, None) => Err("the host sent no answer".to_string()),
                    (false, _) => Err(error.unwrap_or_else(|| "the model call failed".into())),
                });
            }
            // The answer streams as deltas first; the result that ends it is the one that counts.
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {
                if shared.stopping() || board.is_cancelled(start.id) {
                    shared.nested().unregister(&model_key(req));
                    return Err(Report::new(Status::Stopped, "the work item ended"));
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(Report::new(Status::Failed, "the host connection is gone"));
            }
        }
    }
}

/// One tool call of a helper: refused if it is a helper tool, not offered to it, or not allowed by the VM's policy;
/// otherwise run here and reported to the hub under the policy. `(result, is_error)`.
fn helper_tool<S: Write + Send, W: Write + Send>(
    shared: &Shared<'_, S, W>,
    ws: &Workspace,
    start: &Start,
    call: &str,
    name: &str,
    input: &Value,
) -> (String, bool) {
    if HELPER_TOOLS.contains(&name) {
        return ("a helper cannot start or await helpers".into(), true);
    }
    if let Some(reason) = agent::invalid_input(input, None) {
        return (format!("INVALID_INPUT: {reason}"), true);
    }
    if !start.tools.iter().any(|tool| tool.name == name) {
        return (format!("{name} is not offered to a helper"), true);
    }
    let Some(policy) = start
        .policy
        .as_ref()
        .filter(|policy| policy.allow.iter().any(|tool| tool == name))
    else {
        return (format!("{name} is not allowed in this VM"), true);
    };
    let started = Instant::now();
    let outcome = run_guest_tool(ws, WORK_BASE + start.id, name, input);
    let ms = started.elapsed().as_millis() as u64;
    let (ok, output) = match outcome {
        Ok(output) => (true, output),
        Err(err) => (false, err),
    };
    let output = cap_output(&output, MAX_TOOL_OUTPUT);
    let record = (policy.id.clone(), name.to_string(), input.clone());
    let _ = shared.say(&agent::tool_report(
        start.work,
        call,
        ok,
        output.clone(),
        Some(&record),
        ms,
        None,
    ));
    (output, !ok)
}

/// A helper's worktree: where it is, its branch, and the commit it started from.
struct Made {
    dir: PathBuf,
    branch: String,
    base: String,
}

/// Makes the worktree of helper `id` under the workspace's `.helpers` on branch `helper/h<id>`.
fn worktree(workspace: &Workspace, id: u64) -> Result<Made, String> {
    let root = workspace.dir();
    let base = git(root, &["rev-parse", "HEAD"])?.trim().to_string();
    let branch = format!("helper/h{id}");
    let dir = root.join(HELPERS_DIR).join(format!("h{id}"));
    let dir_arg = dir.to_string_lossy().into_owned();
    git(root, &["worktree", "add", "-b", &branch, &dir_arg])?;
    exclude(root);
    Ok(Made { dir, branch, base })
}

/// Keeps the helpers' worktrees out of the workspace's own status.
fn exclude(root: &Path) {
    let info = root.join(".git").join("info");
    if !info.is_dir() {
        return;
    }
    let path = info.join("exclude");
    let current = fs::read_to_string(&path).unwrap_or_default();
    if !current.lines().any(|line| line.trim() == HELPERS_DIR) {
        let _ = fs::write(&path, format!("{current}{HELPERS_DIR}/\n"));
    }
}

/// Commits the helper's edits on its branch, when there are any.
fn commit(dir: &Path, job: &str) -> Result<(), String> {
    if git(dir, &["status", "--porcelain"])?.trim().is_empty() {
        return Ok(());
    }
    git(dir, &["add", "-A"])?;
    let subject: String = job.lines().next().unwrap_or("").chars().take(60).collect();
    git(
        dir,
        &[
            "-c",
            "user.name=apz-helper",
            "-c",
            "user.email=helper@plaza.invalid",
            "commit",
            "-m",
            &format!("helper: {subject}"),
        ],
    )
    .map(|_| ())
}

/// The diff stat of `branch` against `base`, as git prints it.
fn diff_stat(root: &Path, base: &str, branch: &str) -> Result<String, String> {
    let stat = git(root, &["diff", "--stat", &format!("{base}..{branch}")])?;
    Ok(cap_output(stat.trim_end(), 1024))
}

/// Runs git in `dir`: its output, or its error text.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|err| err.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profile(max_turns: u32) -> AgentProfile {
        serde_json::from_value(json!({
            "name": "ada", "role": "builder", "model": "anthropic/x",
            "system_parts": [{"text": "be brief"}],
            "tools": [
                {"name": "read", "description": "read", "input_schema": {"type": "object"}},
                {"name": "edit", "description": "edit", "input_schema": {"type": "object"}},
                {"name": "spawn_helper", "description": "spawn", "input_schema": {"type": "object"}},
            ],
            "max_turns": max_turns,
        }))
        .expect("a profile")
    }

    #[test]
    fn the_cap_is_half_the_cpus_and_never_less_than_one() {
        assert_eq!(cap_for(1), 1);
        assert_eq!(cap_for(3), 1);
        assert_eq!(cap_for(4), 2);
        assert_eq!(cap_for(10), 5);
    }

    #[test]
    fn a_spawn_names_its_job_and_refuses_a_model_its_step_does_not_allow() {
        let (board, _events) = Board::new(1);
        let p = profile(4);
        let text = board
            .spawn(7, &p, None, &json!({"job": "find the parser"}))
            .expect("a job starts");
        assert!(text.starts_with("started helper h1"), "{text}");
        let err = board
            .spawn(7, &p, None, &json!({"job": "x", "model": "other/y"}))
            .expect_err("another model is refused");
        assert!(err.contains("anthropic/x"), "{err}");
        assert!(
            board.spawn(7, &p, None, &json!({})).is_err(),
            "a job is needed"
        );
        assert!(
            board
                .spawn(7, &p, None, &json!({"job": "x", "max_report_tokens": 0}))
                .is_err()
        );
    }

    #[test]
    fn a_helper_is_offered_reads_only_unless_it_edits_and_never_the_helper_tools() {
        let (board, events) = Board::new(1);
        let p = profile(4);
        board.spawn(7, &p, None, &json!({"job": "read"})).unwrap();
        board
            .spawn(7, &p, None, &json!({"job": "edit", "edits": true}))
            .unwrap();
        let mut seen = Vec::new();
        for event in events.try_iter() {
            if let Event::Start(start) = event {
                seen.push((
                    start.edits,
                    start
                        .tools
                        .iter()
                        .map(|t| t.name.clone())
                        .collect::<Vec<_>>(),
                ));
            }
        }
        assert_eq!(seen[0], (false, vec!["read".to_string()]));
        assert_eq!(
            seen[1],
            (true, vec!["read".to_string(), "edit".to_string()])
        );
    }

    #[test]
    fn an_ended_helper_is_reported_and_an_unknown_one_is_an_error() {
        let (board, _events) = Board::new(1);
        let p = profile(4);
        board.spawn(7, &p, None, &json!({"job": "x"})).unwrap();
        board.finish(1, Report::new(Status::Done, "found it"));
        let text = board.await_report(&json!({"id": "h1"})).expect("it ended");
        assert_eq!(text, "helper h1 ended: found it");
        assert!(board.await_report(&json!({"id": "h9"})).is_err());
    }

    #[test]
    fn an_await_past_its_time_says_the_helper_is_still_running() {
        let (board, _events) = Board::new(1);
        let p = profile(4);
        board.spawn(7, &p, None, &json!({"job": "x"})).unwrap();
        let text = board
            .await_report(&json!({"id": "h1", "timeout_s": 0}))
            .expect("the wait ends");
        assert_eq!(text, "helper h1 is still running");
    }

    #[test]
    fn cancelling_a_work_item_stops_only_its_unended_helpers() {
        let (board, _events) = Board::new(1);
        let p = profile(4);
        board.spawn(7, &p, None, &json!({"job": "a"})).unwrap();
        board.spawn(8, &p, None, &json!({"job": "b"})).unwrap();
        board.cancel_work(7);
        assert!(board.is_cancelled(1));
        assert!(!board.is_cancelled(2));
    }
}

#[cfg(test)]
mod serve_tests {
    use super::*;
    use crate::session::testing::pair;
    use apz_proto::line::{Lines, Recv};
    use serde_json::json;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    const HEARTBEAT: Duration = Duration::from_secs(60);

    fn scratch(name: &str) -> Workspace {
        let dir = std::env::temp_dir().join(format!("apz-helpers-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch workspace");
        Workspace::at(dir)
    }

    fn profile(tools: Value) -> AgentProfile {
        serde_json::from_value(json!({
            "name": "ada", "role": "builder", "model": "anthropic/x",
            "system_parts": [{"text": "be brief"}],
            "tools": tools, "max_turns": 6,
        }))
        .expect("a profile")
    }

    fn tool(name: &str) -> Value {
        json!({"name": name, "description": name, "input_schema": {"type": "object"}})
    }

    /// The loop's own tools, which a helper test offers.
    fn tools(extra: &[&str]) -> Value {
        let mut all = vec![tool("read"), tool("spawn_helper"), tool("await_helper")];
        all.extend(extra.iter().map(|name| tool(name)));
        Value::Array(all)
    }

    fn policy_line(allow: &[&str]) -> String {
        json!({"type": "policy", "policy": {"id": "pol-1", "issued_ms": 1, "allow": allow, "signature": "s"}})
            .to_string()
    }

    fn reply(req: u64, content: Value, stop: &str) -> String {
        json!({
            "type": "model_result", "req": req, "ok": true,
            "response": {"model": "anthropic/x", "content": content, "stop": stop,
                         "usage": {"input_tokens": 1, "output_tokens": 1}},
        })
        .to_string()
    }

    fn text(text: &str) -> Value {
        json!({"type": "text", "text": text})
    }

    fn call(id: &str, name: &str, input: Value) -> Value {
        json!({"type": "tool_use", "id": id, "name": name, "input": input})
    }

    /// Reads the host's model calls: the parent's (no `helper`) and each helper's, in arrival order, kept until asked
    /// for, so the script can answer them in any order.
    struct Calls {
        host: Lines<UnixStream>,
        pending: Vec<Value>,
    }

    impl Calls {
        fn read_one(&mut self, wait: Option<Duration>) -> Option<Value> {
            self.host
                .get_ref()
                .set_read_timeout(wait.or(Some(Duration::from_secs(10))))
                .expect("a read timeout");
            loop {
                match self.host.recv().expect("a line from the guest") {
                    Recv::Line(line) => {
                        let value: Value = serde_json::from_str(&line).expect("a JSON line");
                        if value["type"] == "model_call" {
                            return Some(value);
                        }
                    }
                    Recv::Idle => return None,
                    Recv::Closed => panic!("the guest closed"),
                }
            }
        }

        /// The next model call `want` accepts; the others wait in `pending`.
        fn want(&mut self, want: impl Fn(&Value) -> bool) -> Value {
            if let Some(at) = self.pending.iter().position(&want) {
                return self.pending.remove(at);
            }
            loop {
                let call = self.read_one(None).expect("a model call");
                if want(&call) {
                    return call;
                }
                self.pending.push(call);
            }
        }

        /// Whether a call `want` accepts arrives within `wait` (none does, once it is waited for).
        fn arrives(&mut self, want: impl Fn(&Value) -> bool, wait: Duration) -> bool {
            if self.pending.iter().any(&want) {
                return true;
            }
            let until = std::time::Instant::now() + wait;
            while std::time::Instant::now() < until {
                match self.read_one(Some(wait)) {
                    Some(call) => {
                        let hit = want(&call);
                        self.pending.push(call);
                        if hit {
                            return true;
                        }
                    }
                    None => break,
                }
            }
            false
        }

        fn send(&mut self, line: &str) {
            self.host.send(line).expect("the guest reads");
        }

        fn stop(mut self) {
            self.send(r#"{"type":"stop","reason":"test over"}"#);
            loop {
                match self.host.recv().expect("the guest's bye") {
                    Recv::Line(line) if line.contains("\"bye\"") => return,
                    Recv::Line(_) | Recv::Idle => {}
                    Recv::Closed => panic!("the guest closed before its bye"),
                }
            }
        }
    }

    fn parent(call: &Value) -> bool {
        call.get("helper").is_none_or(Value::is_null)
    }

    fn helper_of(call: &Value, id: u64) -> bool {
        call["helper"].as_u64() == Some(id)
    }

    /// Serves one work item on a VM whose profile is `profile`, with `cap` helpers at once, against `script`.
    fn serve_with(
        workspace: &Workspace,
        profile: AgentProfile,
        cap: usize,
        script: impl FnOnce(Calls) + Send + 'static,
    ) -> u64 {
        let (mut guest, host) = pair();
        let hub = thread::spawn(move || {
            script(Calls {
                host,
                pending: Vec::new(),
            })
        });
        let mut log = Vec::new();
        let result = agent::serve_capped(
            &mut guest,
            &mut log,
            workspace,
            Some(profile),
            HEARTBEAT,
            cap,
        );
        drop(guest);
        hub.join().expect("the scripted hub finished");
        result.expect("the hub stops the agent")
    }

    /// The text of each tool result in `messages`, by its `tool_use_id`'s order.
    fn results(call: &Value) -> Vec<(bool, String)> {
        call["request"]["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|message| message["content"].as_array().cloned().unwrap_or_default())
            .filter(|block| block["type"] == "tool_result")
            .map(|block| {
                (
                    block["is_error"].as_bool().unwrap_or(false),
                    block["content"].as_str().unwrap_or("").to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn two_helpers_run_at_once_and_a_third_waits_for_one_of_them() {
        let workspace = scratch("cap");
        let profile = profile(tools(&[]));
        serve_with(&workspace, profile, 2, |mut calls| {
            calls.send(&policy_line(&["spawn_helper", "await_helper", "read"]));
            calls.send(
                &json!({"type": "work", "work": 1, "kind": "task", "thread": "t", "prompt": "go"})
                    .to_string(),
            );
            let first = calls.want(parent);
            assert_eq!(first["work"], 1, "{first}");
            calls.send(&reply(
                first["req"].as_u64().unwrap(),
                json!([
                    call("c1", "spawn_helper", json!({"job": "one"})),
                    call("c2", "spawn_helper", json!({"job": "two"})),
                    call("c3", "spawn_helper", json!({"job": "three"})),
                ]),
                "tool_use",
            ));
            let second = calls.want(parent);
            calls.send(&reply(
                second["req"].as_u64().unwrap(),
                json!([
                    call("a1", "await_helper", json!({"id": "h1", "timeout_s": 30})),
                    call("a2", "await_helper", json!({"id": "h2", "timeout_s": 30})),
                    call("a3", "await_helper", json!({"id": "h3", "timeout_s": 30})),
                ]),
                "tool_use",
            ));
            let started = calls.want(|call| call["helper"].as_u64().is_some());
            let other = calls.want(|call| call["helper"].as_u64().is_some());
            let mut ids = [
                started["helper"].as_u64().unwrap(),
                other["helper"].as_u64().unwrap(),
            ];
            ids.sort_unstable();
            assert_eq!(ids, [1, 2], "the first two helpers start");
            assert!(
                !calls.arrives(
                    |call| call["helper"].as_u64().is_some(),
                    Duration::from_millis(300)
                ),
                "a third helper waits while two run"
            );
            calls.send(&reply(
                started["req"].as_u64().unwrap(),
                json!([text(&format!("report for h{}", started["helper"]))]),
                "end_turn",
            ));
            let third = calls.want(|call| helper_of(call, 3));
            calls.send(&reply(
                other["req"].as_u64().unwrap(),
                json!([text(&format!("report for h{}", other["helper"]))]),
                "end_turn",
            ));
            calls.send(&reply(
                third["req"].as_u64().unwrap(),
                json!([text("report for h3")]),
                "end_turn",
            ));
            let last = calls.want(parent);
            let answered = results(&last);
            assert!(
                answered
                    .iter()
                    .any(|(_, text)| text.starts_with("helper h1 ended: report for h1")),
                "{answered:?}"
            );
            calls.send(&reply(
                last["req"].as_u64().unwrap(),
                json!([text("all done")]),
                "end_turn",
            ));
            calls.stop();
        });
    }

    #[test]
    fn a_helper_cannot_start_a_helper_and_its_report_is_capped() {
        let workspace = scratch("depth");
        let profile = profile(tools(&[]));
        serve_with(&workspace, profile, 1, |mut calls| {
            calls.send(&policy_line(&["spawn_helper", "await_helper", "read"]));
            calls.send(
                &json!({"type": "work", "work": 1, "kind": "task", "thread": "t", "prompt": "go"})
                    .to_string(),
            );
            let first = calls.want(parent);
            calls.send(&reply(
                first["req"].as_u64().unwrap(),
                json!([call(
                    "c1",
                    "spawn_helper",
                    json!({"job": "look", "max_report_tokens": 10})
                )]),
                "tool_use",
            ));
            let offered = calls.want(|call| helper_of(call, 1));
            let names: Vec<&str> = offered["request"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| tool["name"].as_str().unwrap())
                .collect();
            assert_eq!(
                names,
                ["read"],
                "a helper is offered reads, never the helper tools"
            );
            calls.send(&reply(
                offered["req"].as_u64().unwrap(),
                json!([call("n1", "spawn_helper", json!({"job": "deeper"}))]),
                "tool_use",
            ));
            let after = calls.want(|call| helper_of(call, 1));
            let refused = results(&after);
            assert!(
                refused
                    .iter()
                    .any(|(is_error, text)| *is_error
                        && text.contains("cannot start or await helpers")),
                "{refused:?}"
            );
            calls.send(&reply(
                after["req"].as_u64().unwrap(),
                json!([text(&"x".repeat(400))]),
                "end_turn",
            ));
            let second = calls.want(parent);
            calls.send(&reply(
                second["req"].as_u64().unwrap(),
                json!([call("a1", "await_helper", json!({"id": "h1"}))]),
                "tool_use",
            ));
            let last = calls.want(parent);
            let awaited = results(&last);
            let report = awaited
                .last()
                .map(|(_, text)| text.clone())
                .unwrap_or_default();
            assert!(report.starts_with("helper h1 ended: "), "{report}");
            assert!(
                report.len() < 120,
                "a report of 10 tokens is cut short: {}",
                report.len()
            );
            calls.send(&reply(
                last["req"].as_u64().unwrap(),
                json!([text("done")]),
                "end_turn",
            ));
            calls.stop();
        });
    }

    #[test]
    fn an_editing_helper_commits_to_its_branch_and_the_parent_tree_is_untouched() {
        let workspace = scratch("edits");
        let root = workspace.dir().to_path_buf();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q"]);
        fs::write(root.join("README.md"), "base\n").unwrap();
        git(&["add", "-A"]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "base",
        ]);
        let profile = profile(tools(&["edit"]));
        serve_with(&workspace, profile, 1, |mut calls| {
            calls.send(&policy_line(&[
                "spawn_helper",
                "await_helper",
                "read",
                "edit",
            ]));
            calls.send(
                &json!({"type": "work", "work": 1, "kind": "task", "thread": "t", "prompt": "go"})
                    .to_string(),
            );
            let first = calls.want(parent);
            calls.send(&reply(
                first["req"].as_u64().unwrap(),
                json!([call(
                    "c1",
                    "spawn_helper",
                    json!({"job": "write it", "edits": true})
                )]),
                "tool_use",
            ));
            let edit = calls.want(|call| helper_of(call, 1));
            calls.send(&reply(
                edit["req"].as_u64().unwrap(),
                json!([call(
                    "e1",
                    "edit",
                    json!({"create": [{"path": "helper.txt", "content": "from the helper\n"}]})
                )]),
                "tool_use",
            ));
            let done = calls.want(|call| helper_of(call, 1));
            calls.send(&reply(
                done["req"].as_u64().unwrap(),
                json!([text("wrote it")]),
                "end_turn",
            ));
            let second = calls.want(parent);
            calls.send(&reply(
                second["req"].as_u64().unwrap(),
                json!([call("a1", "await_helper", json!({"id": "h1"}))]),
                "tool_use",
            ));
            let last = calls.want(parent);
            let awaited = results(&last);
            let report = awaited
                .last()
                .map(|(_, text)| text.clone())
                .unwrap_or_default();
            assert!(report.contains("branch: helper/h1"), "{report}");
            calls.send(&reply(
                last["req"].as_u64().unwrap(),
                json!([text("done")]),
                "end_turn",
            ));
            calls.stop();
        });
        assert!(
            !root.join("helper.txt").exists(),
            "the parent's tree is untouched"
        );
        let out = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["show", "helper/h1:helper.txt"])
            .output()
            .expect("git");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "from the helper\n");
    }
}
