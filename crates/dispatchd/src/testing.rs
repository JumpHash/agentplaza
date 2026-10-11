//! Test support: whole fakes of the two daemons and of an agent.
//!
//! Unit tests are built around three stand-ins, and they are shared because
//! the same fakes prove different things at different layers:
//!
//! * [`FakeAgent`] is a *whole agent*: a workspace in a map that answers the
//!   harness's tools the way the harness does, and records every call. It is a
//!   [`Driver`], so the workflow scripts are proven against it directly.
//! * [`FakeOperator`] is the hub: it runs each `tell` against a [`FakeAgent`]
//!   for that CID (or fails the way a down hub, a silent agent or an unknown
//!   CID does), and records every call. It is an [`Operator`], so the pass is
//!   proven against it.
//! * [`FakeLedger`] is the node: tasks in memory, the transitions the real
//!   ledger would make, and a record of every claim and finish.
//!
//! None of them is a mock of an internal interface: they are the *daemons*
//! faked, at the wire semantics each real client implements, which is what
//! makes a test they pass worth something.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::Mutex;

use serde_json::json;

use crate::dispatch::{Ledger, Logger, Operator, Social};
use crate::views::{Agent, Board, Contact, NewPost, Outcome, Post, ReadAck, Task, Tell, Unread};
use crate::workflow::{DriveError, Driver};

/// A whole agent, faked: a workspace in a map that answers the harness's tools
/// the way the harness does, and records every call it was asked to make.
#[derive(Default)]
pub struct FakeAgent {
    files: BTreeMap<String, String>,
    calls: Vec<(String, serde_json::Value)>,
    /// When set, the next call fails with this instead of answering: a whole
    /// class of agent failure, injected at one point.
    failure: Option<DriveError>,
    /// When set, `read_file` answers with this content whatever was written —
    /// the agent whose workspace did not take the write.
    corrupt_read: Option<String>,
    /// When set, every tool answers this text instead of doing its work — the
    /// agent whose answer is not the shape the workflow needs.
    answer: Option<String>,
}

impl FakeAgent {
    /// An agent whose next call fails with `error`.
    pub fn failing(error: DriveError) -> Self {
        Self {
            failure: Some(error),
            ..Self::default()
        }
    }

    /// An agent whose `read_file` never returns what was written.
    pub fn corrupting(content: &str) -> Self {
        Self {
            corrupt_read: Some(content.to_string()),
            ..Self::default()
        }
    }

    /// An agent whose every tool answers `text` instead of its own output.
    pub fn answering(text: &str) -> Self {
        Self {
            answer: Some(text.to_string()),
            ..Self::default()
        }
    }

    /// Puts a file in the agent's workspace before any call.
    pub fn with_file(mut self, path: &str, content: &str) -> Self {
        self.files.insert(path.to_string(), content.to_string());
        self
    }

    /// The names of the tools the agent was asked to run, in order.
    pub fn sequence(&self) -> Vec<&str> {
        self.calls.iter().map(|(name, _)| name.as_str()).collect()
    }

    /// The argument `key` of the `call`-th call, which the test asserts is
    /// there.
    pub fn arg(&self, call: usize, key: &str) -> &str {
        self.calls
            .get(call)
            .and_then(|(_, args)| args.get(key))
            .and_then(serde_json::Value::as_str)
            .expect("the call carries this argument")
    }

    /// The file `path` holds now, if it holds one.
    pub fn file(&self, path: &str) -> Option<&str> {
        self.files.get(path).map(String::as_str)
    }

    /// Every file the workspace holds, by path.
    pub fn files(&self) -> &BTreeMap<String, String> {
        &self.files
    }

    fn record(&mut self, name: &str, args: &serde_json::Value) {
        self.calls.push((name.to_string(), args.clone()));
    }
}

impl Driver for FakeAgent {
    fn call(&mut self, name: &str, args: serde_json::Value) -> Result<String, DriveError> {
        self.record(name, &args);
        if let Some(failure) = self.failure.clone() {
            return Err(failure);
        }
        if let Some(answer) = self.answer.clone() {
            return Ok(answer);
        }
        let path = args
            .get("path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        match name {
            "write_file" => {
                let content = args
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let created = self
                    .files
                    .insert(path.to_string(), content.to_string())
                    .is_none();
                Ok(json!({
                    "path": path,
                    "bytes": content.len(),
                    "created": created,
                })
                .to_string())
            }
            "read_file" => {
                let content = self
                    .corrupt_read
                    .clone()
                    .or_else(|| self.files.get(path).cloned());
                match content {
                    Some(content) => Ok(json!({
                        "path": path,
                        "size": content.len(),
                        "bytes": content.len(),
                        "truncated": false,
                        "content": content,
                    })
                    .to_string()),
                    None => Err(DriveError::Refused {
                        name: "read_file".to_string(),
                        output: format!("not found: {path}"),
                    }),
                }
            }
            "list" => {
                let entries: Vec<serde_json::Value> = self
                    .files
                    .iter()
                    .map(|(name, content)| {
                        json!({ "name": name, "kind": "file", "size": content.len() })
                    })
                    .collect();
                Ok(json!({
                    "path": ".",
                    "entries": entries,
                    "truncated": false,
                    "skipped": 0,
                })
                .to_string())
            }
            "search" => {
                // The harness's own shape: the pattern, the files it scanned
                // and one entry per matching line, sorted by file name (the
                // workspace is a BTreeMap, so the walk already is).
                let pattern = args
                    .get("pattern")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                if pattern.is_empty() {
                    return Err(DriveError::Refused {
                        name: "search".to_string(),
                        output: "invalid input: \"pattern\" must not be empty".to_string(),
                    });
                }
                let mut matches = Vec::new();
                let mut bytes = 0;
                for (name, content) in &self.files {
                    bytes += content.len();
                    for (index, line) in content.lines().enumerate() {
                        if line.contains(pattern) {
                            matches.push(json!({
                                "file": name,
                                "line": index + 1,
                                "text": line,
                            }));
                        }
                    }
                }
                Ok(json!({
                    "pattern": pattern,
                    "path": ".",
                    "matches": matches,
                    "files_scanned": self.files.len(),
                    "bytes_scanned": bytes,
                    "skipped": 0,
                    "truncated": false,
                })
                .to_string())
            }
            other => Err(DriveError::Refused {
                name: other.to_string(),
                output: format!("no such tool: {other}"),
            }),
        }
    }

    fn calls(&self) -> u64 {
        self.calls.len() as u64
    }
}

/// The hub, faked: one [`FakeAgent`] per CID, plus the ways a real hub fails.
#[derive(Default)]
pub struct FakeOperator {
    agents: RefCell<HashMap<u64, FakeAgent>>,
    /// When set, every `tell` fails at the transport instead of reaching an
    /// agent: a hub that is down.
    transport: RefCell<Option<String>>,
    /// When set, every `tell` answers this: an agent that never answers
    /// (a timeout) or a CID the hub does not serve.
    answer: RefCell<Option<Tell>>,
    /// Every `(cid, tool, args)` the dispatcher asked for, in order.
    pub calls: RefCell<Vec<(u64, String, serde_json::Value)>>,
}

impl FakeOperator {
    /// A hub that answers every `tell` with `answer`.
    pub fn answering(answer: Tell) -> Self {
        Self {
            answer: RefCell::new(Some(answer)),
            ..Self::default()
        }
    }

    /// A hub that cannot be reached at all.
    pub fn unreachable(reason: &str) -> Self {
        Self {
            transport: RefCell::new(Some(reason.to_string())),
            ..Self::default()
        }
    }

    /// The hub comes back after an outage: a transport failure (and any fixed
    /// answer) is cleared, so the next `tell` reaches the agents again. A test
    /// that restarts the dispatcher uses it to model a hub that was down for one
    /// dispatcher's pass and up for the next.
    pub fn recover(&self) {
        *self.transport.borrow_mut() = None;
        *self.answer.borrow_mut() = None;
    }

    /// Puts a whole agent (with a workspace) behind `cid`.
    pub fn with_agent(self, cid: u64, agent: FakeAgent) -> Self {
        self.agents.borrow_mut().insert(cid, agent);
        self
    }

    /// The tool names the dispatcher asked for, in order, with their CIDs.
    pub fn sequence(&self) -> Vec<(u64, String)> {
        self.calls
            .borrow()
            .iter()
            .map(|(cid, name, _)| (*cid, name.clone()))
            .collect()
    }

    /// The file `path` holds in the agent behind `cid`, if that agent holds it.
    pub fn agent_file(&self, cid: u64, path: &str) -> Option<String> {
        self.agents
            .borrow()
            .get(&cid)
            .and_then(|agent| agent.file(path).map(str::to_string))
    }
}

impl Operator for FakeOperator {
    fn tell(
        &self,
        cid: u64,
        name: &str,
        args: &serde_json::Value,
        _timeout_ms: u64,
    ) -> Result<Tell, String> {
        self.calls
            .borrow_mut()
            .push((cid, name.to_string(), args.clone()));
        if let Some(reason) = self.transport.borrow().clone() {
            return Err(reason);
        }
        if let Some(answer) = self.answer.borrow().clone() {
            return Ok(answer);
        }
        let mut agents = self.agents.borrow_mut();
        let agent = agents.entry(cid).or_default();
        match agent.call(name, args.clone()) {
            Ok(output) => Ok(Tell::Answered { ok: true, output }),
            Err(DriveError::Refused { output, .. }) => Ok(Tell::Answered { ok: false, output }),
            Err(other) => Err(other.note()),
        }
    }
}

/// Sharing: the ledger and the hub *outlive* any one dispatcher, so a test that
/// restarts the dispatcher shares the same fakes through an `Rc` — that is the
/// whole point of a restart: the daemons' state stays, the dispatcher's memory
/// does not. The blanket impls let the fakes be used the same way in tests that
/// do not share.
impl<L: Ledger> Ledger for Rc<L> {
    fn tasks(&self) -> Result<Vec<Task>, String> {
        (**self).tasks()
    }

    fn agents(&self) -> Result<Vec<Agent>, String> {
        (**self).agents()
    }

    fn claim(&self, id: &str, cid: u64) -> Result<(), String> {
        (**self).claim(id, cid)
    }

    fn finish(&self, id: &str, outcome: Outcome, note: &str) -> Result<(), String> {
        (**self).finish(id, outcome, note)
    }
}

impl<O: Operator> Operator for Rc<O> {
    fn tell(
        &self,
        cid: u64,
        name: &str,
        args: &serde_json::Value,
        timeout_ms: u64,
    ) -> Result<Tell, String> {
        (**self).tell(cid, name, args, timeout_ms)
    }
}

/// The node's ledger, faked: tasks in memory, the transitions the real ledger
/// would make, and a record of every claim and finish.
#[derive(Default)]
pub struct FakeLedger {
    /// The tasks, oldest first, as the ledger would list them.
    pub tasks: RefCell<Vec<Task>>,
    /// The connected agents, as the register would list them.
    pub agents: Vec<Agent>,
    /// Every `(task, cid)` claim the dispatcher asked for.
    pub claims: RefCell<Vec<(String, u64)>>,
    /// Every `(task, outcome, note)` finish the dispatcher asked for.
    pub finishes: RefCell<Vec<(String, Outcome, String)>>,
    /// When set, every claim is refused with this.
    pub claim_refusal: Option<String>,
    /// When set, every finish is refused with this.
    pub finish_refusal: Option<String>,
    /// When set, reading the ledger or the register fails: a node that is
    /// down.
    pub down: Option<String>,
}

impl FakeLedger {
    /// A ledger holding one task, with no agents connected.
    pub fn with_task(task: Task) -> Self {
        Self {
            tasks: RefCell::new(vec![task]),
            ..Self::default()
        }
    }

    /// A ledger holding `tasks`, oldest first.
    pub fn with_tasks(tasks: Vec<Task>) -> Self {
        Self {
            tasks: RefCell::new(tasks),
            ..Self::default()
        }
    }

    /// Connects an agent of `role` on `cid`.
    pub fn with_agent(mut self, cid: u64, role: &str) -> Self {
        self.agents.push(agent(cid, role));
        self
    }

    /// The task `id` as it now stands.
    pub fn task(&self, id: &str) -> Task {
        self.tasks
            .borrow()
            .iter()
            .find(|task| task.id == id)
            .cloned()
            .expect("the task is in the ledger")
    }
}

impl Ledger for FakeLedger {
    fn tasks(&self) -> Result<Vec<Task>, String> {
        if let Some(reason) = &self.down {
            return Err(reason.clone());
        }
        Ok(self.tasks.borrow().clone())
    }

    fn agents(&self) -> Result<Vec<Agent>, String> {
        if let Some(reason) = &self.down {
            return Err(reason.clone());
        }
        Ok(self.agents.clone())
    }

    fn claim(&self, id: &str, cid: u64) -> Result<(), String> {
        if let Some(reason) = &self.claim_refusal {
            return Err(reason.clone());
        }
        self.claims.borrow_mut().push((id.to_string(), cid));
        let mut tasks = self.tasks.borrow_mut();
        let task = tasks
            .iter_mut()
            .find(|task| task.id == id)
            .ok_or_else(|| format!("no task with id {id}"))?;
        if task.state != "open" {
            return Err(format!("a task may not go from {} to claimed", task.state));
        }
        task.state = "claimed".to_string();
        task.agent_cid = Some(cid);
        task.assignee_id = Some(
            self.agents
                .iter()
                .find(|agent| agent.cid == cid)
                .map_or_else(|| format!("cid:{cid}"), |agent| agent.role.clone()),
        );
        Ok(())
    }

    fn finish(&self, id: &str, outcome: Outcome, note: &str) -> Result<(), String> {
        if let Some(reason) = &self.finish_refusal {
            return Err(reason.clone());
        }
        self.finishes
            .borrow_mut()
            .push((id.to_string(), outcome, note.to_string()));
        let mut tasks = self.tasks.borrow_mut();
        let task = tasks
            .iter_mut()
            .find(|task| task.id == id)
            .ok_or_else(|| format!("no task with id {id}"))?;
        if task.state != "claimed" {
            return Err(format!(
                "a task may not go from {} to {outcome}",
                task.state
            ));
        }
        task.state = match outcome {
            Outcome::Done => "done",
            Outcome::Failed => "failed",
        }
        .to_string();
        task.outcome = Some(
            match outcome {
                Outcome::Done => "accepted",
                Outcome::Failed => "failed",
            }
            .to_string(),
        );
        task.note = Some(note.to_string());
        Ok(())
    }
}

/// The dispatcher's own voice, recorded for assertions.
#[derive(Default)]
pub struct Recording(Mutex<Vec<String>>);

impl Recording {
    /// Every line, in order.
    pub fn text(&self) -> String {
        self.0
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .join("\n")
    }
}

impl Logger for Recording {
    fn line(&self, text: &str) {
        self.0
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .push(text.to_string());
    }
}

/// The node's social surface, faked: a board in memory with its read cursors,
/// the node's own rules (a monotonic cursor, a board that refuses a cursor past
/// the newest post), and the ways a real node fails.
///
/// It is the *daemon* faked, not an internal seam: `board` derives the unread
/// counts the way `noded` does (from the cursors against the posts), `read`
/// enforces the monotonic cursor the node enforces, and every refusal is the
/// node's own `{error}`-shaped message — so a social pass that passes these
/// tests is a pass the real node would answer the same way.
#[derive(Default)]
pub struct FakeSocial {
    inner: Mutex<FakeSocialInner>,
}

#[derive(Default)]
struct FakeSocialInner {
    /// The posts the board holds, oldest first.
    posts: Vec<Post>,
    /// Each agent's read cursor: the number of the last post it read.
    cursors: BTreeMap<u64, u64>,
    /// Every `(cid, up_to)` cursor advance the pass asked for, in order.
    reads: Vec<(u64, String)>,
    /// How many times the pass read the board.
    boards: usize,
    /// Every post the pass asked the board to record, in order.
    written: Vec<NewPost>,
    /// Every `(cid, peer)` contact the pass asked for, in order.
    contacts: Vec<(u64, u64)>,
    /// Every `(from, to, text)` the pass said, in order.
    said: Vec<(u64, u64, String)>,
    /// When set, reading the board fails with this.
    board_failure: Option<String>,
    /// When set, every cursor advance is refused with this.
    read_failure: Option<String>,
    /// When set, every post is refused with this.
    post_failure: Option<String>,
}

impl FakeSocial {
    /// An empty board with every cursor at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Seeds a post on the board, authored by `cid` as `role`, and returns the
    /// board now holding it.
    pub fn with_post(
        self,
        cid: u64,
        role: &str,
        subject: &str,
        body: &str,
        task: Option<&str>,
    ) -> Self {
        let mut inner = self.lock();
        let seq = inner.posts.len() as u64 + 1;
        inner
            .posts
            .push(board_post(seq, cid, role, subject, body, task));
        drop(inner);
        self
    }

    /// Puts `cid`'s read cursor at the current newest post: the agent that has
    /// already read everything.
    pub fn cursor_at_newest(self, cid: u64) -> Self {
        let mut inner = self.lock();
        let newest = inner.posts.len() as u64;
        inner.cursors.insert(cid, newest);
        drop(inner);
        self
    }

    /// A board that will not read: a node that is down, or a 500.
    pub fn board_failing(self, reason: &str) -> Self {
        self.lock().board_failure = Some(reason.to_string());
        self
    }

    /// A node whose cursor advance is refused (a 409, say).
    pub fn read_failing(self, reason: &str) -> Self {
        self.lock().read_failure = Some(reason.to_string());
        self
    }

    /// A node whose post is refused (a 409 board full, a 400 outside the
    /// contract).
    pub fn post_failing(self, reason: &str) -> Self {
        self.lock().post_failure = Some(reason.to_string());
        self
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FakeSocialInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Every `(cid, up_to)` the pass asked the board to advance to.
    pub fn reads(&self) -> Vec<(u64, String)> {
        self.lock().reads.clone()
    }

    /// How many times the pass read the board: the cadence, made observable.
    pub fn boards(&self) -> usize {
        self.lock().boards
    }

    /// Every post the pass asked the board to record.
    pub fn written(&self) -> Vec<NewPost> {
        self.lock().written.clone()
    }

    /// Every contact the pass asked for — empty for a driver that leaves the
    /// award's links to the node.
    pub fn contacts(&self) -> Vec<(u64, u64)> {
        self.lock().contacts.clone()
    }

    /// Everything the pass said to a peer — empty for a driver that leaves the
    /// conversations to the agents.
    pub fn said(&self) -> Vec<(u64, u64, String)> {
        self.lock().said.clone()
    }
}

/// A post the fake's board holds, with the id the node would give it.
fn board_post(
    seq: u64,
    cid: u64,
    role: &str,
    subject: &str,
    body: &str,
    task: Option<&str>,
) -> Post {
    Post {
        id: format!("p{seq}"),
        agent_cid: cid,
        role: role.to_string(),
        subject: subject.to_string(),
        body: body.to_string(),
        task: task.map(str::to_string),
        posted_by: "operator".to_string(),
        created_at: seq,
    }
}

impl Social for FakeSocial {
    fn board(&self) -> Result<Board, String> {
        let mut inner = self.lock();
        inner.boards += 1;
        if let Some(reason) = &inner.board_failure {
            return Err(reason.clone());
        }
        let total = inner.posts.len();
        let newest = total as u64;
        let posts: Vec<Post> = inner.posts.iter().rev().cloned().collect();
        let unread = inner
            .cursors
            .iter()
            .map(|(cid, cursor)| Unread {
                agent_cid: *cid,
                count: (newest - cursor) as usize,
            })
            .collect();
        Ok(Board {
            posts,
            total,
            limit: 256,
            truncated: false,
            unread,
        })
    }

    fn read_board(&self, cid: u64, up_to: &str) -> Result<ReadAck, String> {
        let mut inner = self.lock();
        if let Some(reason) = &inner.read_failure {
            return Err(reason.clone());
        }
        inner.reads.push((cid, up_to.to_string()));
        let newest = inner.posts.len() as u64;
        let Some(seq) = up_to
            .strip_prefix('p')
            .and_then(|seq| seq.parse::<u64>().ok())
        else {
            return Err(format!(
                "the board read answered 400: up_to must name a post (p<seq>), not {up_to:?}"
            ));
        };
        if newest == 0 {
            return Err("the board read answered 409: the board is empty".to_string());
        }
        if seq > newest {
            return Err(format!(
                "the board read answered 409: up_to {up_to} is past the newest post p{newest}"
            ));
        }
        let held = inner.cursors.get(&cid).copied().unwrap_or(0);
        if seq < held {
            return Err(format!(
                "the board read answered 409: up_to {up_to} is behind the agent's read cursor p{held}"
            ));
        }
        inner.cursors.insert(cid, seq);
        Ok(ReadAck {
            agent_cid: cid,
            up_to: up_to.to_string(),
            unread: (newest - seq) as usize,
        })
    }

    fn post(&self, post: &NewPost) -> Result<Post, String> {
        let mut inner = self.lock();
        if let Some(reason) = &inner.post_failure {
            return Err(reason.clone());
        }
        inner.written.push(post.clone());
        let seq = inner.posts.len() as u64 + 1;
        let recorded = board_post(
            seq,
            post.agent_cid,
            "",
            &post.subject,
            &post.body,
            post.task.as_deref(),
        );
        inner.posts.push(recorded.clone());
        Ok(recorded)
    }

    fn contacts(&self, _cid: u64) -> Result<Vec<Contact>, String> {
        Ok(Vec::new())
    }

    fn add_contact(&self, cid: u64, peer_cid: u64) -> Result<(), String> {
        self.lock().contacts.push((cid, peer_cid));
        Ok(())
    }

    fn say(&self, cid: u64, to_cid: u64, text: &str) -> Result<(), String> {
        self.lock().said.push((cid, to_cid, text.to_string()));
        Ok(())
    }
}

/// An agent row as the hub's registry would report it.
pub fn agent(cid: u64, role: &str) -> Agent {
    Agent {
        cid,
        role: role.to_string(),
        connected_at_ms: 1,
        last_heartbeat_ms: 2,
        calls: 0,
    }
}

/// A task as the ledger would report it.
pub fn task(id: &str, state: &str, role: Option<&str>) -> Task {
    serde_json::from_value(json!({
        "id": id,
        "goal": format!("do {id}"),
        "role": role,
        "state": state,
    }))
    .expect("a task")
}

/// Reads one whole HTTP request off a test's socket: the head, then exactly
/// the body its `Content-Length` declares, however the segments fell.
///
/// A request written in one `write_all` may still be split by the network, and
/// a test that read only once would then assert on half a request.
pub fn read_http_request(stream: &mut std::net::TcpStream) -> String {
    use std::io::Read;

    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = stream.read(&mut chunk).expect("the request is read");
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&buffer);
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let declared = head
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if body.len() >= declared {
                break;
            }
        }
    }
    String::from_utf8_lossy(&buffer).into_owned()
}
