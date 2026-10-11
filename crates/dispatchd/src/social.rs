//! The social pass: the plaza's board, read and spoken to by the dispatcher.
//!
//! `docs/agents/social.md`'s *drivers* section is this module. The dispatcher's
//! pass claims and drives — it is how work gets done. The social pass is how
//! the plaza *talks*: one idle connected agent per pass reads the board, and,
//! when the board names work its role can serve, says so. Templates, not prose:
//! every word of a note is the task's own (its id, its goal, its acceptance) or
//! a fact the dispatcher holds (the agent's role and CID, the grants the codex
//! gave the role) — the driver never invents a claim.
//!
//! # What one social pass does
//!
//! In order, for **one** agent:
//!
//! 1. **pick** — the connected register, cid-ascending, walked round-robin from
//!    where the last social pass left off ([`SocialPass::cursor`]). The first
//!    agent that is not **busy** is the one: busy means holding a claim in
//!    hand, which the dispatcher knows from the ledger's un-ended claims and
//!    its own retry state. When every connected agent holds a claim, or none is
//!    connected, the pass acts for nobody and says so.
//! 2. **read the board** — `POST /api/v0/board/read {agent_cid, up_to}` with
//!    `up_to` the newest post the board holds, which drops the agent's unread
//!    count to zero. An agent **already at the newest post** (its unread count
//!    is zero) is *not* posted for: the cursor is monotonic, and replaying a
//!    cursor it already holds is the refused go-backwards. That case is skipped
//!    silently — no request at all, and no journal noise.
//! 3. **the interest note** — the board's posts are walked newest-first, and
//!    the first whose `task` names a ledger row that is **open** and whose role
//!    is **exactly this agent's role**, and that the agent has **not already
//!    signalled for** ([`already_signalled`]), is posted as `subject:
//!    "available for <task>"` with the templated body ([`interest_note`]) and
//!    `posted_by` naming the driver ([`DRIVER`]). The post carries the task
//!    link, so the board — where discovery happens — points back at the work.
//! 4. **the award is noded's own** — when a task is awarded, the node posts the
//!    hiring note and links the contacts both ways. The dispatcher does not
//!    duplicate it, and this pass never touches a contact (the talk step below
//!    is the one thing here that speaks).
//!
//! # The talk step: the finisher tells the awarder
//!
//! The social pass makes work *discoverable*; the talk step says when it is
//! done. When the dispatcher finishes a task it drove — the confirmed `done`
//! and `failed` transitions in [`crate::dispatch`] — the agent that did the
//! work **says so to the task's awarder**, when the board names one, through
//! the same `say` the contract's `POST /api/v0/agents/{cid}/say` serves.
//!
//! **The awarder, from the board.** The node posts an award's hiring note
//! itself (`noded`'s `Social::seed_award`): a post whose `subject` is exactly
//! `hiring for <task>`, whose `task` link names that task, and whose
//! `agent_cid` is the **awarder** — the employer the award's caller named. That
//! post is the one source this step reads for "who hired this": it is already
//! the plaza's record of the hire, and the board must be read anyway to reach
//! it. [`awarder_of`] takes the newest such post. No awarder, no word: the step
//! skips, logs once and makes no `say`. An awarder that is the finisher itself
//! is skipped the same way — nothing says anything to itself.
//!
//! **The template.** One line, every word of it the task's own:
//!
//! ```text
//! done:   <task> — <the run's first artifact, or "no artifact note">
//! failed: <task> — <the dispatcher's own failure note>
//! ```
//!
//! Nothing is invented. The `done` line names the first artifact the run's
//! report carries — a read-only run wrote none, and the line says so rather
//! than claiming one — and the `failed` line is the same note the task was
//! finished with: the agent's own refusal, or the dispatcher's evidence after
//! the retry bound. [`talk_line`] composes it and cuts it to the contract's
//! 2 KiB `text` cap ([`MAX_SAY_TEXT`]), so the node never has to refuse it for
//! length.
//!
//! **The dedup.** At most one `say` per task. [`SocialPass::talk`] remembers
//! every task id it has spoken about and refuses a second word about the same
//! one. Across a restart the durable dedup is the **ledger's own terminal
//! state**: the word is emitted only as part of a transition the ledger
//! confirmed, and the ledger refuses `done` → `done` — so a restarted
//! dispatcher reads the task as terminal, skips it, and never reaches the
//! transition again. The conversation record is deliberately *not* the source:
//! the [`Social`] seam this crate holds carries no conversations accessor, and
//! the node's own conversation view is a bounded, newest-first **window**
//! (`truncated` is a field of it) — a window cannot prove the *absence* of an
//! earlier word, while a terminal row can. The in-memory set is therefore the
//! within-run guard, bounded to [`MAX_REMEMBERED`] ids; what it forgets, the
//! ledger already holds.
//!
//! **The bound.** The talk step is not the pass: it makes at most **two**
//! loopback requests per confirmed terminal transition — one `GET
//! /api/v0/board` (to recover the awarder) and at most one `POST
//! /api/v0/agents/{cid}/say` — both under the same [`crate::http::TIMEOUT`]
//! every other call rides, and it never retries. A task reaches a terminal
//! transition at most once, so the step runs at most once per task.
//!
//! # The dedup rule (stated, not implied)
//!
//! An agent has already signalled for a task when either the board carries a
//! post it authored that names the task (the interest note it posted, or any
//! earlier word about it), or the ledger records its CID as that task's
//! claimant — past or present. The board side is read from the newest
//! [`crate::views::Board`] the node returns (its own 256-post window); the
//! ledger side is read from the same task list the claim walk uses. Together
//! they are: *it already posted about that task, or it holds or held the
//! claim.*
//!
//! # The bound (stated, not implied)
//!
//! One social pass acts for **at most one agent** — the round-robin picks one
//! and the pass is over — and makes **at most three** loopback requests, all of
//! them for that one agent: one `GET /api/v0/board`, at most one
//! `POST /api/v0/board/read`, and at most one `POST /api/v0/board`. It never
//! fans out over the register, and no call is retried. So the plaza's social
//! traffic is at most three requests for one agent per social pass — never a
//! request per agent, and never more than three.
//!
//! # Cadence, and how it meets the claim loop
//!
//! [`SOCIAL_EVERY`] is how many dispatch passes go by between social passes:
//! the dispatcher runs one social pass every `SOCIAL_EVERY`-th pass — at the
//! default `--interval-secs 5` that is roughly one every 25 s, on the same loop
//! the claims run on but slower. It runs **first**, over the same snapshot the
//! claim walk is about to work from and before any claim is taken, so it never
//! observes an agent this pass is mid-claim with; an agent the ledger already
//! records as a claimant (a deferred retry, a claim left by an earlier pass,
//! another dispatcher's) is skipped. Every social failure — a board that will
//! not read, a refused cursor, a refused post — is logged here and
//! **swallowed**: the pass moves on to the claims. Each of the three calls is
//! bounded by the same loopback deadline every other dispatchd call uses
//! ([`crate::http::TIMEOUT`], 5 s), so a social pass can hold the claim walk up
//! by at most about fifteen seconds, and only on the every-[`SOCIAL_EVERY`]-th
//! pass: a social call can never stop, delay past its own bounded timeout, or
//! otherwise perturb the claim workflow.

use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;

use crate::dispatch::{Logger, Social};
use crate::roles::{self, Manifest};
use crate::views::{Agent, Board, NewPost, Outcome, Post, Task};

/// How many dispatch passes go by between social passes. One social pass every
/// [`SOCIAL_EVERY`]-th pass — the daemon's own loop, but slower. At the default
/// `--interval-secs 5` that is about one social pass every 25 s; the first
/// pass of a run is one, so a daemon's first tick already reads the board.
pub const SOCIAL_EVERY: u64 = 5;

/// The `posted_by` every post this driver makes carries: the dispatcher, acting
/// for the agent. The contract's `posted_by` names the driver, never the agent
/// — the agent is the author.
pub const DRIVER: &str = "dispatchd";

/// The most an interest note's body may occupy, in bytes: the board's own
/// `body` cap (4 KiB). A composed note is cut to it on a character boundary
/// rather than sent to be refused.
pub const MAX_BODY_BYTES: usize = 4096;

/// The most the talk step's line may occupy, in bytes: the contract's `say`
/// `text` cap (2 KiB). Like a note, a composed line is cut to it on a
/// character boundary rather than sent to be refused.
pub const MAX_SAY_TEXT: usize = 2048;

/// The `subject` prefix the node gives an award's hiring note: the post whose
/// subject is this plus a task's id, and which links that task, is the board's
/// record of who hired whom.
pub const HIRING_NOTE: &str = "hiring for ";

/// How many task ids the talk step remembers having spoken about. One id per
/// distinct task is a handful of bytes, so the bound is not about weight but
/// about being a bound at all: past it the oldest is forgotten, and what the
/// memory drops the ledger's terminal row still holds ([`SocialPass::talk`]).
pub const MAX_REMEMBERED: usize = 1024;

/// The drive of one social pass: the agent it acted for, and which of the two
/// things it did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SocialReport {
    /// The agent the pass acted for, when it found an idle one.
    pub acted_for: Option<u64>,
    /// Whether the pass advanced the agent's read cursor.
    pub read: bool,
    /// Whether the pass posted an interest note.
    pub posted: bool,
}

/// The interest note a driver posts for an agent: the board's `subject` and
/// `body`, composed from the task's own words and the agent's own facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// `available for <task>` — the contract's own subject.
    pub subject: String,
    /// The template: role, cid, the task's id and goal, its acceptance, and
    /// the harness tools the role's grants cover.
    pub body: String,
}

/// The social pass, held by the dispatcher for the life of a run: the node's
/// social surface, the dispatcher's voice, and the round-robin position.
pub struct SocialPass {
    node: Arc<dyn Social + Send + Sync>,
    log: Arc<dyn Logger + Send + Sync>,
    /// Where the round-robin over the connected register stands: the index
    /// (into the cid-ascending register) the *next* social pass starts at.
    cursor: usize,
    /// The tasks the talk step has already spoken about, oldest first — the
    /// dedup's within-run memory, bounded to [`MAX_REMEMBERED`] ids. The
    /// durable dedup is the ledger's own terminal state ([`SocialPass::talk`]).
    said: VecDeque<String>,
}

impl SocialPass {
    /// A social pass over `node`, logging to `log`, starting its round-robin
    /// at the first connected agent.
    pub fn new(node: Arc<dyn Social + Send + Sync>, log: Arc<dyn Logger + Send + Sync>) -> Self {
        Self {
            node,
            log,
            cursor: 0,
            said: VecDeque::new(),
        }
    }

    /// Where the round-robin stands: the register index the next pass starts
    /// at. Exposed so a test can prove the walk is a walk.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// How many tasks the talk step remembers having spoken about — the bound
    /// ([`MAX_REMEMBERED`]), made observable.
    pub fn spoken(&self) -> usize {
        self.said.len()
    }

    /// The talk step: the agent that finished `task` says so to the task's
    /// awarder, when the board names one.
    ///
    /// Called by the dispatcher on a confirmed terminal transition. The order
    /// is: the within-run dedup, then the board (for the awarder), then at most
    /// one `say`. Every way of not speaking — already spoken, a board that will
    /// not read, no hiring note for the task, an awarder that is the finisher
    /// itself, a refused delivery — is one log line and no more; none of them
    /// is an error the caller has to handle, which is what keeps the claim loop
    /// unblocked. Answers whether a `say` was *made*, not whether it was
    /// delivered (the node's own row records that).
    pub fn talk(
        &mut self,
        task: &Task,
        cid: u64,
        outcome: Outcome,
        artifact: Option<&str>,
        note: &str,
    ) -> bool {
        if self.said.iter().any(|said| said == &task.id) {
            self.log.line(&format!(
                "dispatchd: social: agent cid {cid} has already told the awarder about task {} ({outcome}); not saying it again",
                task.id
            ));
            return false;
        }
        let board = match self.node.board() {
            Ok(board) => board,
            Err(reason) => {
                self.log.line(&format!(
                    "dispatchd: social: agent cid {cid}: the board could not be read to name task {}'s awarder: {reason}; nothing said",
                    task.id
                ));
                return false;
            }
        };
        let Some(awarder) = awarder_of(&board, &task.id) else {
            self.log.line(&format!(
                "dispatchd: social: agent cid {cid} finished task {} ({outcome}) but the board names no awarder for it; nothing said",
                task.id
            ));
            return false;
        };
        if awarder == cid {
            self.log.line(&format!(
                "dispatchd: social: agent cid {cid} finished task {} ({outcome}) and the board's awarder is that same agent; nothing said",
                task.id
            ));
            return false;
        }
        // The one word for this task, whether or not the node takes it: the
        // memory holds the attempt, so a second transition for the same task
        // can never be a second `say`.
        self.remember(task.id.clone());
        let text = talk_line(&task.id, outcome, artifact, note);
        match self.node.say(cid, awarder, &text) {
            Ok(()) => {
                // The node took the exchange: it is on the record, and whether
                // the peer's session took the call is the node's own row to
                // say (`delivered`/`delivery`), never this log line's claim.
                self.log.line(&format!(
                    "dispatchd: social: agent cid {cid} said to its awarder cid {awarder} about task {} ({outcome}): {text} (the node's conversation row carries the delivery)",
                    task.id
                ));
                true
            }
            Err(reason) => {
                self.log.line(&format!(
                    "dispatchd: social: agent cid {cid}: the word to awarder cid {awarder} about task {} was refused: {reason}; moving on",
                    task.id
                ));
                false
            }
        }
    }

    /// Records `id` as spoken about, forgetting the oldest once the memory is
    /// full: the within-run guard is bounded, and the ledger's terminal row is
    /// the durable one (`talk`'s own doc says why).
    fn remember(&mut self, id: String) {
        if self.said.len() == MAX_REMEMBERED {
            self.said.pop_front();
        }
        self.said.push_back(id);
    }

    /// Runs one social pass over the snapshot `tasks` and `agents` — the same
    /// pair the claim walk reads — skipping `busy` agents.
    ///
    /// Every failure is logged and swallowed; the report says what happened.
    pub fn run(
        &mut self,
        tasks: &[Task],
        agents: &[Agent],
        busy: &BTreeSet<u64>,
        roles: Option<&Manifest>,
    ) -> SocialReport {
        let mut report = SocialReport::default();

        // The register, cid-ascending and de-duplicated: the round-robin order
        // is a property of the plaza, not of the order a daemon happened to
        // list its rows in.
        let mut register: Vec<u64> = agents.iter().map(|agent| agent.cid).collect();
        register.sort_unstable();
        register.dedup();
        if register.is_empty() {
            self.log
                .line("dispatchd: social: no connected agent to act for; nothing to do");
            return report;
        }

        // Round-robin from where the last social pass left off, skipping the
        // agents with a claim in hand.
        let start = self.cursor % register.len();
        let picked = (0..register.len())
            .map(|offset| (start + offset) % register.len())
            .find(|&index| !busy.contains(&register[index]));
        let Some(index) = picked else {
            self.log
                .line("dispatchd: social: every connected agent holds a claim; nothing to do");
            return report;
        };
        let cid = register[index];
        self.cursor = (index + 1) % register.len();
        let role = agents
            .iter()
            .find(|agent| agent.cid == cid)
            .map(|agent| agent.role.clone())
            .unwrap_or_default();
        report.acted_for = Some(cid);

        let board = match self.node.board() {
            Ok(board) => board,
            Err(reason) => {
                self.log.line(&format!(
                    "dispatchd: social: agent cid {cid} ({role}): the board could not be read: {reason}; skipping"
                ));
                return report;
            }
        };

        // (a) Read the board: advance the cursor to the newest post, which
        // drops the agent's unread count to zero. An agent already at the
        // newest post is not posted for — the cursor is monotonic, and asking
        // it to hold still is the refused go-backwards — so that case is
        // skipped silently, with no request at all.
        match board.newest() {
            None => self.log.line(&format!(
                "dispatchd: social: agent cid {cid} ({role}): the board is empty; there is nothing to read"
            )),
            Some(newest) if board.unread_of(cid) == Some(0) => self.log.line(&format!(
                "dispatchd: social: agent cid {cid} ({role}) is already at the newest post {}; nothing to read",
                newest.id
            )),
            Some(newest) => match self.node.read_board(cid, &newest.id) {
                Ok(ack) => {
                    report.read = true;
                    self.log.line(&format!(
                        "dispatchd: social: agent cid {cid} ({role}) read the board to {}; {} unread left",
                        newest.id, ack.unread
                    ));
                }
                Err(reason) => self.log.line(&format!(
                    "dispatchd: social: agent cid {cid} ({role}): the board read was refused: {reason}; moving on"
                )),
            },
        }

        // (b) The interest note: the newest post that names an open task of
        // this agent's own role that it has not already spoken about.
        let candidate = board.posts.iter().find_map(|post| {
            let task_id = post.task.as_deref()?;
            let task = tasks.iter().find(|task| task.id == task_id)?;
            if !task.is_open() || task.roled() != Some(role.as_str()) {
                return None;
            }
            if already_signalled(&board, tasks, cid, &task.id) {
                return None;
            }
            Some(task)
        });
        let Some(task) = candidate else {
            self.log.line(&format!(
                "dispatchd: social: agent cid {cid} ({role}): no open task on the board it can serve; nothing posted"
            ));
            return report;
        };

        let note = interest_note(&role, cid, task, roles);
        let post = NewPost {
            agent_cid: cid,
            subject: note.subject.clone(),
            body: note.body,
            task: Some(task.id.clone()),
            posted_by: DRIVER.to_string(),
        };
        match self.node.post(&post) {
            Ok(record) => {
                report.posted = true;
                self.log.line(&format!(
                    "dispatchd: social: agent cid {cid} ({role}) posted {} about open task {} ({})",
                    record.subject, task.id, record.id
                ));
            }
            Err(reason) => self.log.line(&format!(
                "dispatchd: social: agent cid {cid} ({role}): the interest note for {} was refused: {reason}; moving on",
                task.id
            )),
        }
        report
    }
}

/// Whether the agent has already signalled for `task`: the dedup rule, in one
/// place.
///
/// True when either the board carries a post this agent authored that names
/// the task (its own earlier word about it — the interest note it posted), or
/// the ledger records its CID as that task's claimant, past or present (the
/// claim it holds or held).
pub fn already_signalled(board: &Board, tasks: &[Task], cid: u64, task: &str) -> bool {
    board.posted_about(cid, task)
        || tasks
            .iter()
            .any(|row| row.id == task && row.agent_cid == Some(cid))
}

/// The awarder the board names for `task`, when it names one: the `agent_cid`
/// of the newest **hiring note** for it.
///
/// An award's hiring note is the node's own post — `subject` exactly
/// `hiring for <task>`, `task` the task's id, `agent_cid` the awarder's guest
/// CID (`noded`'s `Social::seed_award`, which the award endpoint calls). It is
/// the one source the talk step reads for "who hired this": a post that merely
/// links the task (the interest note the social pass posts) or merely shares
/// the subject is not a hiring note, and is not read as one. The board is
/// newest-first, so the first match is the latest award of the task.
pub fn awarder_of(board: &Board, task: &str) -> Option<u64> {
    board
        .posts
        .iter()
        .find(|post| is_hiring_note(post, task))
        .map(|post| post.agent_cid)
}

/// Whether `post` is the board's hiring note for `task`: it names the task and
/// its subject is exactly the node's own `hiring for <task>`.
fn is_hiring_note(post: &Post, task: &str) -> bool {
    post.task.as_deref() == Some(task)
        && post
            .subject
            .strip_prefix(HIRING_NOTE)
            .is_some_and(|named| named == task)
}

/// The one line the talk step says: a template over the task's own facts.
///
/// ```text
/// done:   <task> — <the run's first artifact, or "no artifact note">
/// failed: <task> — <the dispatcher's own failure note>
/// ```
///
/// `artifact` is the first path the run's report carries, when it carries any:
/// a read-only run wrote nothing, and the line states that rather than naming
/// an artifact that does not exist. `note` is the same note the task was
/// finished with — the agent's own refusal, or the dispatcher's evidence after
/// the retry bound — never a paraphrase of it. The result is cut to the
/// contract's `text` cap, so the node never refuses the line for length.
pub fn talk_line(task: &str, outcome: Outcome, artifact: Option<&str>, note: &str) -> String {
    let line = match outcome {
        Outcome::Done => format!(
            "done: {task} — {}",
            artifact
                .filter(|artifact| !artifact.trim().is_empty())
                .unwrap_or("no artifact note")
        ),
        Outcome::Failed => format!("failed: {task} — {note}"),
    };
    bounded_bytes(&line, MAX_SAY_TEXT)
}

/// The interest note for `role`/`cid` serving `task`, under `roles` (the
/// grants, or `None` when the dispatcher holds no manifest).
///
/// The subject is the contract's `available for <task>`. The body is a
/// template, and every field in it is the task's own words or a fact about the
/// agent:
///
/// ```text
/// {role} (cid {cid}) can serve {task}: {goal}. acceptance: {acceptance}; tools: {tools}
/// ```
///
/// where `tools` is the harness tools the role's grants cover (the grant
/// table's order), and the clause is left out entirely when the dispatcher
/// holds no manifest — there are then no grants to name. A goal or acceptance
/// the task does not carry is stated as absent, never invented.
pub fn interest_note(role: &str, cid: u64, task: &Task, roles: Option<&Manifest>) -> Note {
    let goal = if task.goal.trim().is_empty() {
        "(no goal stated)".to_string()
    } else {
        task.goal.clone()
    };
    let acceptance = task
        .acceptance
        .as_deref()
        .filter(|acceptance| !acceptance.trim().is_empty())
        .unwrap_or("(none stated)");
    let mut body = format!(
        "{role} (cid {cid}) can serve {}: {goal}. acceptance: {acceptance}",
        task.id
    );
    if let Some(manifest) = roles {
        body.push_str(&format!("; tools: {}", covered_tools(manifest, role)));
    }
    Note {
        subject: format!("available for {}", task.id),
        body: bounded_bytes(&body, MAX_BODY_BYTES),
    }
}

/// The harness tools the role's grants cover, comma-joined — the tools this
/// agent may actually be driven with, which is what the board should know.
///
/// The diagnostics (`echo`, `uptime`) need no grant and are not what a role is
/// hired for, so they are left out; the order is the grant table's, so the
/// same role always composes the same line. A role the grants cover nothing
/// for states `none` rather than an empty list.
fn covered_tools(manifest: &Manifest, role: &str) -> String {
    let covered: Vec<&str> = roles::TOOL_GRANTS
        .iter()
        .filter(|(tool, sources)| !sources.is_empty() && manifest.grants(role, tool))
        .map(|(tool, _)| *tool)
        .collect();
    if covered.is_empty() {
        "none".to_string()
    } else {
        covered.join(", ")
    }
}

/// Cuts `text` to at most `max` bytes on a character boundary, appending `…`
/// when it cut — so a composed note can never be refused by the board's own
/// `body` cap.
fn bounded_bytes(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    // Leave room for the marker so the answer is at most `max` bytes.
    let mut end = max.saturating_sub('…'.len_utf8());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = text[..end].to_string();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::rc::Rc;

    use crate::dispatch::Dispatcher;
    use crate::testing::{FakeAgent, FakeLedger, FakeOperator, FakeSocial, Recording, agent, task};

    /// A social pass over `social`, with its log kept for assertions. The fake
    /// is shared through an `Arc` so the test can read what the pass did after
    /// it ran.
    fn social_pass(social: &Arc<FakeSocial>) -> (SocialPass, Arc<Recording>) {
        let log = Arc::new(Recording::default());
        let pass = SocialPass::new(
            Arc::clone(social) as Arc<dyn Social + Send + Sync>,
            Arc::clone(&log) as Arc<dyn Logger + Send + Sync>,
        );
        (pass, log)
    }

    /// A roles manifest from a role → tools table, as the codex's own does.
    fn manifest(roles: &[(&str, &[&str])]) -> Manifest {
        Manifest::parse(
            &serde_json::json!({
                "roles": roles
                    .iter()
                    .map(|(role, tools)| (
                        (*role).to_string(),
                        serde_json::json!({ "role": role, "tools": tools }),
                    ))
                    .collect::<serde_json::Map<String, serde_json::Value>>(),
            })
            .to_string(),
        )
        .expect("a well-formed test manifest")
    }

    /// An open task with a goal and an acceptance of its own.
    fn roled_task(id: &str, goal: &str, acceptance: Option<&str>, role: &str) -> Task {
        serde_json::from_value(serde_json::json!({
            "id": id, "goal": goal, "acceptance": acceptance, "role": role, "state": "open",
        }))
        .expect("a task")
    }

    /// A claimed task whose row records the claimant, as the ledger would.
    fn claimed_task(id: &str, role: &str, cid: u64) -> Task {
        let mut task = task(id, "claimed", Some(role));
        task.agent_cid = Some(cid);
        task
    }

    // ------------------------------------------------------------ eligibility

    /// The pass acts for the first **idle** agent, and an agent with a claim in
    /// hand is skipped: the builder holds a claim, so the librarian is the one.
    #[test]
    fn the_pass_acts_for_the_first_idle_agent_and_skips_a_claimant() {
        let tasks = vec![
            task("t1", "open", Some("builder")),
            claimed_task("t2", "builder", 300),
        ];
        let agents = vec![agent(300, "builder"), agent(310, "librarian")];
        // The busy set the dispatcher computes for this snapshot: the builder,
        // the claimant of the un-ended t2.
        let busy = BTreeSet::from([300]);

        let social = Arc::new(FakeSocial::new().with_post(
            320,
            "mayor",
            "hiring for t1",
            "do t1",
            Some("t1"),
        ));
        let (mut pass, log) = social_pass(&social);
        let report = pass.run(&tasks, &agents, &busy, None);

        assert_eq!(report.acted_for, Some(310));
        assert!(report.read, "the librarian read the board");
        assert_eq!(social.reads(), vec![(310, "p1".to_string())]);
        // The post about t1 is for the builder's role; the librarian cannot
        // serve it, so nothing was posted.
        assert!(social.written().is_empty());
        assert!(
            log.text()
                .contains("agent cid 310 (librarian): no open task on the board it can serve"),
            "{}",
            log.text()
        );
    }

    /// The register is walked round-robin, cid-ascending, one agent a pass —
    /// whatever order the daemon listed its rows in.
    #[test]
    fn the_register_is_walked_round_robin() {
        let agents = vec![agent(300, "builder"), agent(310, "librarian")];
        // The daemon lists them the other way round; the walk is still by CID.
        let shuffled = vec![agent(310, "librarian"), agent(300, "builder")];
        let social = Arc::new(FakeSocial::new());
        let (mut pass, _log) = social_pass(&social);
        let idle = BTreeSet::new();

        assert_eq!(pass.run(&[], &shuffled, &idle, None).acted_for, Some(300));
        assert_eq!(pass.cursor(), 1);
        assert_eq!(pass.run(&[], &agents, &idle, None).acted_for, Some(310));
        assert_eq!(pass.cursor(), 0);
        assert_eq!(pass.run(&[], &agents, &idle, None).acted_for, Some(300));
    }

    /// No connected agent, and every connected agent busy, are both no-ops —
    /// logged, and with no request made.
    #[test]
    fn no_idle_agent_is_no_action() {
        let social = Arc::new(FakeSocial::new());
        let (mut pass, log) = social_pass(&social);

        let empty = pass.run(&[], &[], &BTreeSet::new(), None);
        assert_eq!(empty.acted_for, None);
        assert_eq!(social.boards(), 0);
        assert!(
            log.text().contains("no connected agent to act for"),
            "{}",
            log.text()
        );

        let agents = vec![agent(300, "builder")];
        let busy = BTreeSet::from([300]);
        let skipped = pass.run(&[], &agents, &busy, None);
        assert_eq!(skipped.acted_for, None);
        assert_eq!(
            social.boards(),
            0,
            "a busy agent is not asked for the board"
        );
        assert!(
            log.text().contains("every connected agent holds a claim"),
            "{}",
            log.text()
        );
    }

    /// The register's duplicates and the walk's bounds: a CID listed twice is
    /// one agent, and the walk wraps.
    #[test]
    fn the_walk_deduplicates_the_register_and_wraps() {
        let agents = vec![
            agent(300, "builder"),
            agent(300, "builder"),
            agent(310, "librarian"),
        ];
        let social = Arc::new(FakeSocial::new());
        let (mut pass, _log) = social_pass(&social);
        let idle = BTreeSet::new();
        assert_eq!(pass.run(&[], &agents, &idle, None).acted_for, Some(300));
        assert_eq!(pass.run(&[], &agents, &idle, None).acted_for, Some(310));
        assert_eq!(pass.run(&[], &agents, &idle, None).acted_for, Some(300));
    }

    // ------------------------------------------------------------ the read

    /// The cursor advances to the **newest** post, which is the response's
    /// head — and the pass reports that it read.
    #[test]
    fn the_pass_advances_the_cursor_to_the_newest_post() {
        let social = Arc::new(
            FakeSocial::new()
                .with_post(320, "mayor", "hiring for t9", "do t9", Some("t9"))
                .with_post(300, "builder", "a note", "hello", None),
        );
        let (mut pass, _log) = social_pass(&social);
        let report = pass.run(&[], &[agent(300, "builder")], &BTreeSet::new(), None);

        assert!(report.read);
        assert_eq!(social.reads(), vec![(300, "p2".to_string())]);
    }

    /// An agent already at the newest post is not posted for at all: the cursor
    /// is monotonic, and asking it to hold still is the refused go-backwards.
    #[test]
    fn an_agent_already_at_the_newest_post_is_not_read_for() {
        let social = Arc::new(
            FakeSocial::new()
                .with_post(320, "mayor", "hiring for t9", "do t9", Some("t9"))
                .cursor_at_newest(300),
        );
        let (mut pass, log) = social_pass(&social);
        let report = pass.run(&[], &[agent(300, "builder")], &BTreeSet::new(), None);

        assert!(!report.read);
        assert!(
            social.reads().is_empty(),
            "no request: nothing to advance to"
        );
        assert!(
            log.text().contains(
                "agent cid 300 (builder) is already at the newest post p1; nothing to read"
            ),
            "{}",
            log.text()
        );
    }

    /// An empty board is a board: nothing to read, and nothing to serve.
    #[test]
    fn an_empty_board_is_nothing_to_read() {
        let social = Arc::new(FakeSocial::new());
        let (mut pass, log) = social_pass(&social);
        let report = pass.run(&[], &[agent(300, "builder")], &BTreeSet::new(), None);
        assert!(!report.read && !report.posted);
        assert!(social.reads().is_empty());
        assert!(
            log.text()
                .contains("the board is empty; there is nothing to read"),
            "{}",
            log.text()
        );
    }

    // ------------------------------------------------------------ the note

    /// The interest note is the task's own words and the agent's own facts,
    /// exactly: the contract's subject, and the templated body with the grants
    /// the role's codex declaration covers.
    #[test]
    fn the_interest_note_is_the_tasks_own_words() {
        let tasks = vec![roled_task(
            "t4",
            "draft the handbook",
            Some("sections 1-3 exist"),
            "builder",
        )];
        let agents = vec![agent(103, "builder")];
        let roles = manifest(&[(
            "builder",
            &[
                "bash",
                "read_file",
                "write_file",
                "edit_file",
                "list",
                "search",
            ],
        )]);
        let social = Arc::new(FakeSocial::new().with_post(
            320,
            "mayor",
            "hiring for t4",
            "do t4",
            Some("t4"),
        ));
        let (mut pass, log) = social_pass(&social);

        let report = pass.run(&tasks, &agents, &BTreeSet::new(), Some(&roles));
        assert!(report.posted);
        assert_eq!(
            social.written(),
            vec![NewPost {
                agent_cid: 103,
                subject: "available for t4".to_string(),
                body: "builder (cid 103) can serve t4: draft the handbook. acceptance: \
                       sections 1-3 exist; tools: read_file, write_file, edit_file, \
                       append_file, list, search"
                    .to_string(),
                task: Some("t4".to_string()),
                posted_by: "dispatchd".to_string(),
            }]
        );
        assert!(
            log.text()
                .contains("agent cid 103 (builder) posted available for t4 about open task t4"),
            "{}",
            log.text()
        );
    }

    /// The template states what the task does not carry, and names no tools
    /// when the dispatcher holds no manifest: the same shape either way.
    #[test]
    fn the_template_states_what_is_absent_rather_than_inventing_it() {
        let bare = roled_task("t1", "summarize the handbook", None, "librarian");
        let note = interest_note("librarian", 310, &bare, None);
        assert_eq!(note.subject, "available for t1");
        assert_eq!(
            note.body,
            "librarian (cid 310) can serve t1: summarize the handbook. acceptance: (none stated)"
        );

        // A librarian's grants cover `search` and nothing else.
        let roles = manifest(&[("librarian", &["read_memory", "search", "transcript_search"])]);
        let note = interest_note("librarian", 310, &bare, Some(&roles));
        assert_eq!(
            note.body,
            "librarian (cid 310) can serve t1: summarize the handbook. acceptance: (none stated); \
             tools: search"
        );

        // A role the manifest does not carry is covered by nothing.
        let note = interest_note("intruder", 400, &bare, Some(&roles));
        assert!(note.body.ends_with("; tools: none"), "{}", note.body);

        // A task with a blank goal says so rather than printing an empty one.
        let blank = roled_task("t2", "   ", Some("  "), "builder");
        let note = interest_note("builder", 300, &blank, None);
        assert_eq!(
            note.body,
            "builder (cid 300) can serve t2: (no goal stated). acceptance: (none stated)"
        );
    }

    /// A note too long for the board's own body cap is cut on a character
    /// boundary, not sent to be refused.
    #[test]
    fn a_note_is_bounded_to_the_boards_body_cap() {
        let goal = "é".repeat(MAX_BODY_BYTES);
        let long = roled_task("t1", &goal, None, "builder");
        let note = interest_note("builder", 300, &long, None);
        assert!(
            note.body.len() <= MAX_BODY_BYTES,
            "{} bytes",
            note.body.len()
        );
        assert!(note.body.ends_with('…'));
        assert!(note.body.is_char_boundary(note.body.len()));
    }

    // ------------------------------------------------------------ the dedup

    /// The dedup rule, both halves: the board's own record of the agent's word,
    /// and the ledger's record of its claim.
    #[test]
    fn the_dedup_rule_is_the_board_and_the_ledger() {
        let board: Board = serde_json::from_str(
            r#"{"posts":[{"id":"p1","agent_cid":300,"task":"t1","subject":"available for t1",
                          "body":"b","posted_by":"dispatchd"}]}"#,
        )
        .expect("a board");
        assert!(
            already_signalled(&board, &[], 300, "t1"),
            "the board's own record"
        );
        assert!(
            !already_signalled(&board, &[], 310, "t1"),
            "not another agent's"
        );
        assert!(
            !already_signalled(&board, &[], 300, "t2"),
            "not another task's"
        );

        let empty: Board = serde_json::from_str(r#"{"total":0}"#).expect("a board");
        let held = claimed_task("t2", "builder", 300);
        assert!(
            already_signalled(&empty, std::slice::from_ref(&held), 300, "t2"),
            "the ledger's record of the claim"
        );
        assert!(!already_signalled(
            &empty,
            std::slice::from_ref(&held),
            310,
            "t2"
        ));
    }

    /// An agent that already spoke about a task on the board is not posted for
    /// again — even when a later post names the same task.
    #[test]
    fn an_agent_that_already_spoke_about_a_task_is_not_posted_for_again() {
        let tasks = vec![roled_task("t1", "do t1", None, "builder")];
        let social = Arc::new(
            FakeSocial::new()
                .with_post(
                    300,
                    "builder",
                    "available for t1",
                    "builder can serve t1",
                    Some("t1"),
                )
                .with_post(320, "mayor", "hiring for t1", "do t1", Some("t1")),
        );
        let (mut pass, log) = social_pass(&social);
        let report = pass.run(&tasks, &[agent(300, "builder")], &BTreeSet::new(), None);

        assert!(report.read, "it still reads the board");
        assert!(!report.posted);
        assert!(social.written().is_empty());
        assert!(
            log.text()
                .contains("agent cid 300 (builder): no open task on the board it can serve"),
            "{}",
            log.text()
        );
    }

    /// A task the board names but the ledger has not opened — done, claimed, or
    /// not there at all — is not work to speak for.
    #[test]
    fn only_an_open_task_of_the_agents_role_is_spoken_for() {
        for state in ["done", "claimed", "failed"] {
            let tasks = vec![task("t1", state, Some("builder"))];
            let social = Arc::new(FakeSocial::new().with_post(
                320,
                "mayor",
                "hiring for t1",
                "do t1",
                Some("t1"),
            ));
            let (mut pass, _log) = social_pass(&social);
            let report = pass.run(&tasks, &[agent(300, "builder")], &BTreeSet::new(), None);
            assert!(!report.posted, "{state} is not open work");
        }

        // A board link the ledger does not hold at all.
        let social = Arc::new(FakeSocial::new().with_post(
            320,
            "mayor",
            "hiring for t9",
            "do t9",
            Some("t9"),
        ));
        let (mut pass, _log) = social_pass(&social);
        assert!(
            !pass
                .run(&[], &[agent(300, "builder")], &BTreeSet::new(), None)
                .posted
        );

        // The right task, the wrong role.
        let tasks = vec![roled_task("t1", "do t1", None, "librarian")];
        let social = Arc::new(FakeSocial::new().with_post(
            320,
            "mayor",
            "hiring for t1",
            "do t1",
            Some("t1"),
        ));
        let (mut pass, _log) = social_pass(&social);
        assert!(
            !pass
                .run(&tasks, &[agent(300, "builder")], &BTreeSet::new(), None)
                .posted
        );
    }

    /// The driver is named on the post, and the award's contacts and
    /// conversations are left to the node: the pass touches neither.
    #[test]
    fn the_pass_leaves_the_awards_contacts_and_conversations_to_the_node() {
        let tasks = vec![roled_task("t1", "do t1", None, "builder")];
        let social = Arc::new(FakeSocial::new().with_post(
            320,
            "mayor",
            "hiring for t1",
            "do t1",
            Some("t1"),
        ));
        let (mut pass, _log) = social_pass(&social);
        pass.run(&tasks, &[agent(300, "builder")], &BTreeSet::new(), None);

        assert_eq!(social.written()[0].posted_by, DRIVER);
        assert!(
            social.contacts().is_empty(),
            "the award's links are the node's"
        );
        assert!(social.said().is_empty(), "a conversation is not the pass's");
    }

    // ------------------------------------------- failure isolation and cadence

    /// A board that will not read is logged, swallowed, and the claim loop runs
    /// exactly as it would have.
    #[test]
    fn a_board_that_will_not_read_does_not_stop_the_claim_loop() {
        let ledger = Rc::new(
            FakeLedger::with_task(task("t1", "open", Some("builder"))).with_agent(300, "builder"),
        );
        let operator = Rc::new(FakeOperator::default().with_agent(300, FakeAgent::default()));
        let social = Arc::new(FakeSocial::new().board_failing("the board answered 500: boom"));
        let log = Arc::new(Recording::default());
        let mut dispatcher = Dispatcher::new(
            Rc::clone(&ledger),
            operator,
            Arc::clone(&log) as Arc<dyn Logger + Send + Sync>,
        )
        .with_social(Arc::clone(&social) as Arc<dyn Social + Send + Sync>);

        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!((report.claimed, report.done), (1, 1));
        assert_eq!(
            ledger.claims.borrow().as_slice(),
            &[("t1".to_string(), 300)]
        );
        assert!(
            log.text().contains(
                "dispatchd: social: agent cid 300 (builder): the board could not be read: \
                 the board answered 500: boom; skipping"
            ),
            "{}",
            log.text()
        );
    }

    /// A refused cursor advance is logged, and the pass still speaks for the
    /// agent: the read and the note are independent steps.
    #[test]
    fn a_refused_read_does_not_stop_the_interest_note() {
        let tasks = vec![roled_task("t1", "do t1", None, "builder")];
        let social = Arc::new(
            FakeSocial::new()
                .with_post(320, "mayor", "hiring for t1", "do t1", Some("t1"))
                .read_failing("the board read answered 409: behind the agent's read cursor"),
        );
        let (mut pass, log) = social_pass(&social);
        let report = pass.run(&tasks, &[agent(300, "builder")], &BTreeSet::new(), None);

        assert!(!report.read && report.posted);
        assert_eq!(social.written().len(), 1);
        assert!(
            log.text().contains("the board read was refused"),
            "{}",
            log.text()
        );
    }

    /// A refused post is logged and swallowed: the claim loop is untouched.
    #[test]
    fn a_refused_post_is_logged_and_the_claim_loop_runs_on() {
        let ledger = Rc::new(
            FakeLedger::with_task(task("t1", "open", Some("builder"))).with_agent(300, "builder"),
        );
        let operator = Rc::new(FakeOperator::default().with_agent(300, FakeAgent::default()));
        let social = Arc::new(
            FakeSocial::new()
                .with_post(320, "mayor", "hiring for t1", "do t1", Some("t1"))
                .post_failing("the post answered 409: the board is full (1024 posts)"),
        );
        let log = Arc::new(Recording::default());
        let mut dispatcher = Dispatcher::new(
            Rc::clone(&ledger),
            operator,
            Arc::clone(&log) as Arc<dyn Logger + Send + Sync>,
        )
        .with_social(Arc::clone(&social) as Arc<dyn Social + Send + Sync>);

        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!((report.claimed, report.done), (1, 1));
        assert_eq!(
            ledger.claims.borrow().as_slice(),
            &[("t1".to_string(), 300)]
        );
        assert!(
            log.text().contains(
                "dispatchd: social: agent cid 300 (builder): the interest note for t1 was \
                 refused: the post answered 409: the board is full (1024 posts); moving on"
            ),
            "{}",
            log.text()
        );
    }

    /// A social pass and the claim loop share a tick without fighting: the
    /// first idle agent reads the board and speaks for the open task, *then* the
    /// claim loop claims that same task for it.
    #[test]
    fn the_social_pass_never_claims_what_the_claim_loop_claims() {
        let ledger = Rc::new(
            FakeLedger::with_task(task("t1", "open", Some("builder")))
                .with_agent(300, "builder")
                .with_agent(310, "librarian"),
        );
        let operator = Rc::new(
            FakeOperator::default()
                .with_agent(300, FakeAgent::default())
                .with_agent(310, FakeAgent::default()),
        );
        let social = Arc::new(FakeSocial::new().with_post(
            320,
            "mayor",
            "hiring for t1",
            "do t1",
            Some("t1"),
        ));
        let log = Arc::new(Recording::default());
        let mut dispatcher = Dispatcher::new(
            Rc::clone(&ledger),
            operator,
            Arc::clone(&log) as Arc<dyn Logger + Send + Sync>,
        )
        .with_social(Arc::clone(&social) as Arc<dyn Social + Send + Sync>);

        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!((report.claimed, report.done), (1, 1));
        assert_eq!(social.reads(), vec![(300, "p1".to_string())]);
        assert_eq!(social.written().len(), 1);
        assert_eq!(social.written()[0].agent_cid, 300);
        assert_eq!(social.written()[0].task.as_deref(), Some("t1"));
        assert_eq!(
            ledger.claims.borrow().as_slice(),
            &[("t1".to_string(), 300)]
        );
    }

    /// The cadence: the social pass runs on the dispatcher's first pass and then
    /// once every [`SOCIAL_EVERY`] passes — and not once in between.
    #[test]
    fn the_social_pass_runs_on_its_own_slower_cadence() {
        let ledger = FakeLedger::default().with_agent(300, "builder");
        let social = Arc::new(FakeSocial::new());
        let log = Arc::new(Recording::default());
        let mut dispatcher = Dispatcher::new(
            ledger,
            FakeOperator::default(),
            Arc::clone(&log) as Arc<dyn Logger + Send + Sync>,
        )
        .with_social(Arc::clone(&social) as Arc<dyn Social + Send + Sync>);

        for pass in 1..=SOCIAL_EVERY {
            dispatcher.pass().expect("the pass runs");
            // The social passes so far: the run's first, then every fifth.
            let expected = 1 + (pass - 1) / SOCIAL_EVERY;
            assert_eq!(social.boards(), expected as usize, "after pass {pass}");
        }
        dispatcher.pass().expect("the pass runs");
        assert_eq!(
            social.boards(),
            2,
            "the sixth pass is the second social pass"
        );
    }

    /// A dispatcher without the social surface makes no social call at all: the
    /// claim-only dispatcher is exactly what it was.
    #[test]
    fn a_dispatcher_without_the_social_surface_makes_no_social_call() {
        let ledger = FakeLedger::default().with_agent(300, "builder");
        let social = Arc::new(FakeSocial::new());
        let log = Arc::new(Recording::default());
        let mut dispatcher = Dispatcher::new(
            ledger,
            FakeOperator::default(),
            Arc::clone(&log) as Arc<dyn Logger + Send + Sync>,
        );

        for _ in 0..SOCIAL_EVERY {
            dispatcher.pass().expect("the pass runs");
        }
        assert_eq!(social.boards(), 0);
        assert!(!log.text().contains("dispatchd: social:"));
    }

    // ------------------------------------------------------------ the talk step

    /// A board for the talk step's tests: the hiring note for `task` authored
    /// by `cid`.
    fn hire(social: FakeSocial, cid: u64, task: &str) -> FakeSocial {
        social.with_post(
            cid,
            "mayor",
            &format!("hiring for {task}"),
            "go",
            Some(task),
        )
    }

    /// The awarder is the author of the board's hiring note for the task, and
    /// only that: a post that merely links the task, or merely shares the
    /// subject, is not a hire.
    #[test]
    fn the_awarder_is_the_hiring_notes_author() {
        let board: Board = serde_json::from_str(
            r#"{"posts":[
                 {"id":"p3","agent_cid":320,"subject":"available for t1","task":"t1"},
                 {"id":"p2","agent_cid":330,"subject":"hiring for t1","task":null},
                 {"id":"p1","agent_cid":340,"subject":"hiring for t1","task":"t1"}
               ]}"#,
        )
        .expect("a board");
        assert_eq!(
            awarder_of(&board, "t1"),
            Some(340),
            "the hiring note's author"
        );
        assert_eq!(awarder_of(&board, "t2"), None, "no hire names another task");

        // Two hires for the same task: the board is newest-first, so the latest
        // award is the one the finisher answers.
        let board: Board = serde_json::from_str(
            r#"{"posts":[
                 {"id":"p2","agent_cid":321,"subject":"hiring for t1","task":"t1"},
                 {"id":"p1","agent_cid":320,"subject":"hiring for t1","task":"t1"}
               ]}"#,
        )
        .expect("a board");
        assert_eq!(awarder_of(&board, "t1"), Some(321));
    }

    /// The template, exactly: the task's id, the run's first artifact (or the
    /// admission that it left none), and the dispatcher's own failure note.
    #[test]
    fn the_talk_line_is_the_tasks_own_facts() {
        assert_eq!(
            talk_line(
                "t3",
                Outcome::Done,
                Some("build-notes.txt"),
                "artifacts: build-notes.txt (2 tool call(s))"
            ),
            "done: t3 — build-notes.txt"
        );
        assert_eq!(
            talk_line(
                "t1",
                Outcome::Done,
                None,
                "read-only: 3 match(es) (1 tool call(s))"
            ),
            "done: t1 — no artifact note",
            "a read-only run left no artifact, and the line says so"
        );
        assert_eq!(
            talk_line("t1", Outcome::Done, Some("  "), "x"),
            "done: t1 — no artifact note",
            "a blank artifact is absent, not an empty name"
        );
        assert_eq!(
            talk_line(
                "t7",
                Outcome::Failed,
                None,
                "tool write_file refused: boom (after 3 attempt(s))"
            ),
            "failed: t7 — tool write_file refused: boom (after 3 attempt(s))"
        );

        // A note past the contract's `text` cap is cut on a character boundary,
        // not sent to be refused.
        let long = talk_line("t1", Outcome::Failed, None, &"é".repeat(MAX_SAY_TEXT));
        assert!(long.len() <= MAX_SAY_TEXT, "{} bytes", long.len());
        assert!(long.ends_with('…'));
    }

    /// The talk step says once per task, in the template's own words, to the
    /// awarder the board names — and a second word about the same task makes no
    /// request at all.
    #[test]
    fn the_talk_step_says_once_to_the_awarder() {
        let social = Arc::new(hire(FakeSocial::new(), 320, "t1"));
        let (mut pass, log) = social_pass(&social);
        let task = roled_task("t1", "do t1", None, "builder");

        assert!(pass.talk(
            &task,
            300,
            Outcome::Done,
            Some("build-notes.txt"),
            "artifacts: build-notes.txt (2 tool call(s))"
        ));
        assert_eq!(
            social.said(),
            vec![(300, 320, "done: t1 — build-notes.txt".to_string())]
        );
        assert!(
            log.text().contains(
                "agent cid 300 said to its awarder cid 320 about task t1 (done): \
                 done: t1 — build-notes.txt (the node's conversation row carries the delivery)"
            ),
            "{}",
            log.text()
        );

        // The dedup: the second word is refused — by the task's id, whatever
        // outcome it would have carried — and not even the board is read for it.
        let boards = social.boards();
        assert!(!pass.talk(&task, 300, Outcome::Failed, None, "anything"));
        assert_eq!(social.said().len(), 1);
        assert_eq!(social.boards(), boards, "the dedup runs before the board");
        assert_eq!(pass.spoken(), 1);
        assert!(
            log.text().contains(
                "agent cid 300 has already told the awarder about task t1 (failed); \
                 not saying it again"
            ),
            "{}",
            log.text()
        );
    }

    /// No awarder on the board, and an awarder that is the finisher itself: both
    /// make no request and one log line.
    #[test]
    fn the_talk_step_skips_when_the_board_names_no_other_agent() {
        let task = roled_task("t1", "do t1", None, "builder");

        let social = Arc::new(FakeSocial::new());
        let (mut pass, log) = social_pass(&social);
        assert!(!pass.talk(
            &task,
            300,
            Outcome::Done,
            Some("a.txt"),
            "artifacts: a.txt (1 call)"
        ));
        assert!(social.said().is_empty());
        assert_eq!(pass.spoken(), 0);
        assert!(
            log.text().contains(
                "agent cid 300 finished task t1 (done) but the board names no awarder for it; \
                 nothing said"
            ),
            "{}",
            log.text()
        );

        let social = Arc::new(hire(FakeSocial::new(), 300, "t1"));
        let (mut pass, log) = social_pass(&social);
        assert!(!pass.talk(&task, 300, Outcome::Failed, None, "tool x refused: y"));
        assert!(social.said().is_empty());
        assert_eq!(pass.spoken(), 0);
        assert!(
            log.text()
                .contains("the board's awarder is that same agent; nothing said"),
            "{}",
            log.text()
        );
    }

    /// A `say` the node refuses is one log line and one remembered attempt: it
    /// makes no second request, and answers false.
    #[test]
    fn a_refused_say_is_logged_and_not_retried() {
        let social = Arc::new(
            hire(FakeSocial::new(), 320, "t1")
                .say_failing("the say answered 409: no live agent has guest CID 320"),
        );
        let (mut pass, log) = social_pass(&social);
        let task = roled_task("t1", "do t1", None, "builder");

        assert!(!pass.talk(
            &task,
            300,
            Outcome::Done,
            Some("a.txt"),
            "artifacts: a.txt (1 call)"
        ));
        assert!(social.said().is_empty(), "the node took nothing");
        assert_eq!(pass.spoken(), 1, "but the one word was attempted");
        assert!(
            log.text().contains(
                "the word to awarder cid 320 about task t1 was refused: \
                 the say answered 409: no live agent has guest CID 320; moving on"
            ),
            "{}",
            log.text()
        );
        assert!(!pass.talk(
            &task,
            300,
            Outcome::Done,
            Some("a.txt"),
            "artifacts: a.txt (1 call)"
        ));
        assert_eq!(pass.spoken(), 1, "the refusal is not retried");
    }

    /// A board that will not read at talk time says nothing — and nothing is
    /// remembered, because no word was ever composed.
    #[test]
    fn a_board_that_will_not_read_says_nothing() {
        let social = Arc::new(FakeSocial::new().board_failing("the board answered 500: boom"));
        let (mut pass, log) = social_pass(&social);
        let task = roled_task("t1", "do t1", None, "builder");

        assert!(!pass.talk(&task, 300, Outcome::Done, None, "x"));
        assert!(social.said().is_empty());
        assert_eq!(pass.spoken(), 0);
        assert!(
            log.text().contains(
                "the board could not be read to name task t1's awarder: \
                 the board answered 500: boom; nothing said"
            ),
            "{}",
            log.text()
        );
    }

    /// The talk step's memory is bounded: past [`MAX_REMEMBERED`] spoken tasks
    /// the oldest id is forgotten and the count stops growing.
    #[test]
    fn the_talk_steps_memory_is_bounded() {
        let mut social = FakeSocial::new();
        for seq in 1..=MAX_REMEMBERED + 1 {
            let id = format!("t{seq}");
            social = hire(social, 320, &id);
        }
        let social = Arc::new(social);
        let (mut pass, _log) = social_pass(&social);

        for seq in 1..=MAX_REMEMBERED + 1 {
            let task = roled_task(&format!("t{seq}"), "do it", None, "builder");
            assert!(pass.talk(
                &task,
                300,
                Outcome::Done,
                Some("a.txt"),
                "artifacts: a.txt (1 tool call(s))"
            ));
        }
        assert_eq!(pass.spoken(), MAX_REMEMBERED, "the memory is capped");
        assert_eq!(
            social.said().len(),
            MAX_REMEMBERED + 1,
            "every word was made"
        );
    }
}
