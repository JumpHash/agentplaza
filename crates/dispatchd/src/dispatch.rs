//! The pass: pick, claim, drive, finish.
//!
//! One pass reads the ledger and the fleet register once, then walks the
//! ledger oldest-first. For each task it decides one of five things, and the
//! log says which in the dispatcher's own voice:
//!
//! * **skip** — the task names no role, or the role it names has no CONNECTED
//!   agent, or the role's grants do not cover any script this dispatcher may
//!   drive ([`crate::workflow::plan_for`]), or its claim was refused. A task
//!   nobody is addressed to, or a role nothing may be driven for, stays `open`;
//!   the dispatcher does not invent an agent, and does not knowingly attempt a
//!   call the hub would refuse.
//! * **claim** — the task is `open`, a connected agent of its role exists, and
//!   its role can be driven: bind it (`POST /tasks/{id}/claim`) and drive it.
//! * **adopt** — the task is `claimed` and this dispatcher holds no retry
//!   state for it, which is what a task it claimed *before a restart* looks
//!   like (the retry map is in memory). When the ledger's own `agent_cid` names
//!   a CONNECTED agent of exactly the task's role, the claim is this fleet's own
//!   stranded work: re-decide the plan and drive it with a fresh attempt budget.
//!   A claim whose agent is not connected, or is connected as some other role,
//!   is not ours to finish and is left exactly as it is. See
//!   [`Dispatcher::adopt`].
//! * **drive** — run the planned workflow ([`crate::workflow`]) through
//!   the hub's `POST /tell`, one tool call at a time, and finish the task
//!   `done` with its artifacts (or, for a read-only run, with the line the
//!   script read out) — or `failed` with the reason. A **confirmed** finish is
//!   also the talk step ([`crate::social`]): the agent that did the work says
//!   so to the task's awarder, when the board names one, in one line composed
//!   from the task's own facts.
//! * **retry** — the agent could not be reached: leave the task claimed and
//!   try again on later passes, up to [`MAX_ATTEMPTS`], then finish it
//!   `failed` with the evidence.
//!
//! What is retried and what is not is a deliberate split. An agent that
//! **answered** — a tool refusal, an answer the workflow cannot read, a
//! read-back that did not match — has said something final, and the next pass
//! would hear the same thing: that task is finished `failed` at once, with the
//! agent's own words. An agent that **could not be reached** (not connected,
//! no answer in time, the call could not be made at all) is a condition that
//! may pass, so it is retried — bounded — before being given up on. A hub that
//! **refused** the call on the role's grants is neither: it is the policy's
//! answer, final in the same way an agent's refusal is, and `403` is
//! recognised as exactly that ([`Tell::Refused`]) rather than read as a
//! transport failure.
//!
//! The bound is stated rather than implied: **a pass drives every actionable
//! task, one at a time** — never two tool calls in flight, anywhere — and the
//! work it can find is bounded by the ledger's own cap (1024 tasks) and the
//! hub's session caps. `--once` is exactly one pass.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use crate::roles::Manifest;
use crate::social::SocialPass;
use crate::views::{Agent, Board, Contact, NewPost, Outcome, Post, ReadAck, Task, Tell};
use crate::workflow::{self, DriveError, Driver, Plan};

/// How many passes a task whose agent could not be reached is retried for
/// before it is finished `failed` with the evidence. Attempts are counted per
/// pass, so a dispatcher running every 5 s gives up after about 10 s of the
/// agent being unreachable — and a dispatcher that is stopped and started
/// again remembers nothing: a claim its ledger record still backs is adopted
/// with a fresh budget ([`Dispatcher::adopt`]), and the bound starts over.
pub const MAX_ATTEMPTS: u32 = 3;

/// How long the dispatcher is willing to wait for one tool call. It is the
/// hub's own maximum, so a call is never cut off by the client before the hub
/// has given up: the answer that comes back is the hub's verdict, not a race
/// between two timeouts.
pub const TELL_TIMEOUT_MS: u64 = 5_000;

/// The ledger and the fleet register: the node daemon.
///
/// Both live behind one trait because they are one daemon's answer to two
/// questions — what is there to do, and who can do it — and because a pass
/// must read them together to decide anything.
pub trait Ledger {
    /// Every task, oldest first.
    fn tasks(&self) -> Result<Vec<Task>, String>;
    /// Every connected agent (the hub's registry, as the node proxies it).
    fn agents(&self) -> Result<Vec<Agent>, String>;
    /// Binds an open task to a live agent.
    fn claim(&self, id: &str, cid: u64) -> Result<(), String>;
    /// Ends a claimed task.
    fn finish(&self, id: &str, outcome: Outcome, note: &str) -> Result<(), String>;
}

/// The hub's admin surface: one tool call into one connected agent's session.
pub trait Operator {
    /// Runs one tool call on the agent connected as `cid`, waiting at most
    /// `timeout_ms`, and answers what the hub made of it.
    fn tell(
        &self,
        cid: u64,
        name: &str,
        args: &serde_json::Value,
        timeout_ms: u64,
    ) -> Result<Tell, String>;
}

/// The plaza's social surface: the node's board, its read cursors, the contact
/// lists and the peer conversations (`docs/agents/social.md`).
///
/// It is one trait because it is one daemon's answer to one question — what is
/// the plaza saying — and because the social pass reads and writes it through
/// this seam alone. The pass itself uses the board and the read cursor (the
/// interest note is a post); the contact and conversation calls are the rest of
/// the contract, kept here so the client is the whole surface rather than the
/// half the first driver happens to need. `move` and `hibernate` are the
/// dispatcher's business nowhere: an award seeds a contact itself, in the node
/// ([`crate::social`] says so), so the driver never duplicates it.
pub trait Social {
    /// The board: the newest posts, and every agent's unread count.
    fn board(&self) -> Result<Board, String>;
    /// Advances `cid`'s read cursor to `up_to`, and answers what it left
    /// unread. The cursor is monotonic: a cursor at or behind the one the agent
    /// already holds is refused rather than rolled back.
    fn read_board(&self, cid: u64, up_to: &str) -> Result<ReadAck, String>;
    /// Records `post` for a connected agent and answers the post the board now
    /// holds.
    fn post(&self, post: &NewPost) -> Result<Post, String>;
    /// The agent's own contact list.
    fn contacts(&self, cid: u64) -> Result<Vec<Contact>, String>;
    /// Adds one peer to the agent's list. Idempotent: a peer already on the
    /// list keeps the kind it was first recorded with.
    fn add_contact(&self, cid: u64, peer_cid: u64) -> Result<(), String>;
    /// The agent speaks to a peer: the exchange is recorded and delivered to
    /// the peer's session when it has one.
    fn say(&self, cid: u64, to_cid: u64, text: &str) -> Result<(), String>;
}

/// Where the dispatcher's own voice goes.
pub trait Logger {
    /// Writes one line.
    fn line(&self, text: &str);
}

/// The [`Logger`] that writes to this process's standard output, one whole
/// line at a time.
pub struct StdoutLog;

impl Logger for StdoutLog {
    fn line(&self, text: &str) {
        println!("{text}");
    }
}

/// What one pass did, so a run can be summarised and a test can assert on it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Reporter {
    /// Open, roled tasks the pass considered.
    pub considered: usize,
    /// Tasks it claimed (and then drove).
    pub claimed: usize,
    /// Tasks it drove to completion.
    pub done: usize,
    /// Tasks it finished `failed`.
    pub failed: usize,
    /// Tasks it left alone, for a reason it logged.
    pub skipped: usize,
    /// Tasks left claimed, to try again on a later pass.
    pub deferred: usize,
}

impl Reporter {
    /// The one-line tally a pass ends with.
    pub fn summary(&self) -> String {
        format!(
            "{} considered, {} claimed, {} done, {} failed, {} deferred, {} skipped",
            self.considered, self.claimed, self.done, self.failed, self.deferred, self.skipped
        )
    }
}

/// One task's in-memory retry state: the agent it was claimed for and how many
/// passes have tried to drive it.
#[derive(Debug, Clone, Copy)]
struct Attempt {
    cid: u64,
    attempts: u32,
}

/// The dispatcher: the ledger and the hub, the codex's grants it was started
/// with, and the one bit of state that only concerns retries.
pub struct Dispatcher<L: Ledger, O: Operator> {
    ledger: L,
    operator: O,
    log: Arc<dyn Logger + Send + Sync>,
    /// The codex's tool grants (`--roles`), when a manifest was named: what
    /// each role may be driven with, read so the dispatcher does not knowingly
    /// attempt a call the hub would refuse (see
    /// [`crate::workflow::plan_for`]). `None` is the dispatcher without a
    /// manifest: every role drives its family's script.
    roles: Option<Manifest>,
    /// Retries, by task id. Nothing else is remembered: the ledger holds the
    /// tasks and their states, and this only bounds how many passes a task
    /// whose agent is unreachable is tried for.
    retries: HashMap<String, Attempt>,
    /// The social pass ([`crate::social`]), when the dispatcher was given the
    /// node's social surface. `None` is a dispatcher that only claims and
    /// drives: a test, or a run for which the board is off.
    social: Option<SocialPass>,
    passes: u64,
}

impl<L: Ledger, O: Operator> Dispatcher<L, O> {
    /// A dispatcher over `ledger` and `operator`, logging to `log`, with no
    /// grant policy: every role drives its family's script.
    pub fn new(ledger: L, operator: O, log: Arc<dyn Logger + Send + Sync>) -> Self {
        Self::with_roles(ledger, operator, log, None)
    }

    /// A dispatcher that holds `roles` as the grants it plans against.
    pub fn with_roles(
        ledger: L,
        operator: O,
        log: Arc<dyn Logger + Send + Sync>,
        roles: Option<Manifest>,
    ) -> Self {
        Self {
            ledger,
            operator,
            log,
            roles,
            retries: HashMap::new(),
            social: None,
            passes: 0,
        }
    }

    /// Gives the dispatcher the plaza's social surface, so its pass also runs
    /// the social pass ([`crate::social`]).
    ///
    /// The social pass is the dispatcher's social behaviour: one idle connected
    /// agent per social pass reads the board and, when the board names work its
    /// role can serve, says so. It is wired here rather than required by
    /// [`Dispatcher::new`] so a dispatcher built for driving alone — every test
    /// that only cares about claims — stays exactly what it was.
    pub fn with_social(mut self, node: Arc<dyn Social + Send + Sync>) -> Self {
        let log = Arc::clone(&self.log);
        self.social = Some(SocialPass::new(node, log));
        self
    }

    /// How many passes have been run: the number the log's lines carry.
    pub fn passes(&self) -> u64 {
        self.passes
    }

    /// Runs one pass: read, decide, claim, drive, finish.
    ///
    /// An unreachable ledger (or register) is the pass's own failure and is
    /// reported as such: the caller logs it and, in the daemon's loop, tries
    /// again on the next interval. Nothing is claimed and nothing is finished
    /// on a pass that could not read the ledger.
    pub fn pass(&mut self) -> Result<Reporter, String> {
        self.passes += 1;
        let log = Arc::clone(&self.log);
        let tasks = self.ledger.tasks()?;
        let agents = self.ledger.agents()?;
        let mut report = Reporter::default();
        log.line(&format!(
            "dispatchd: pass {}: {} task(s) in the ledger, {} agent(s) connected",
            self.passes,
            tasks.len(),
            agents.len()
        ));

        // The social pass runs first, and on the pass's own cadence but slower
        // ([`crate::social::SOCIAL_EVERY`]): over the same snapshot the claim
        // walk is about to work from, before any claim is taken — so the agent
        // it acts for is one the register shows idle, never one this pass is
        // mid-claim with. Every social failure is logged inside and swallowed
        // here: the claim walk is untouched by it.
        if let Some(social) = &mut self.social
            && self.passes % crate::social::SOCIAL_EVERY == 1
        {
            let busy = busy_agents(&self.retries, &tasks);
            social.run(&tasks, &agents, &busy, self.roles.as_ref());
        }

        for task in &tasks {
            if task.is_terminal() {
                // It ended (whoever ended it): it is not ours any more, and
                // neither is its retry state.
                self.retries.remove(&task.id);
                continue;
            }
            let Some(role) = task.roled() else {
                report.skipped += 1;
                log.line(&format!(
                    "dispatchd: task {} names no role; skipped",
                    task.id
                ));
                continue;
            };
            if task.is_open() {
                report.considered += 1;
                self.start(task, role, &agents, &mut report);
            } else if task.is_claimed() {
                match self.retries.get(&task.id).map(|attempt| attempt.cid) {
                    Some(cid) => {
                        // A task this dispatcher claimed on an earlier pass: an
                        // interruptible wait stopped before the tool result
                        // landed. The plan is re-decided from the same manifest
                        // it was claimed under, so a resumed retry drives exactly
                        // what the claim was made for. (A `Skipped` plan here
                        // means a claim made under an earlier manifest; it is
                        // reported, never driven into a refusal.)
                        match workflow::plan_for(role, self.roles.as_ref()) {
                            Plan::Drive(family) => {
                                self.drive(task, role, family, cid, &agents, &mut report)
                            }
                            Plan::Skipped(reason) => {
                                report.skipped += 1;
                                log.line(&format!(
                                    "dispatchd: task {} (role {role}) is claimed but {reason}; skipped",
                                    task.id
                                ));
                            }
                        }
                    }
                    None => {
                        // No retry state: either a task this dispatcher claimed
                        // before it was stopped and started again (the map is in
                        // memory), or a claim that was never ours. The ledger's
                        // own `agent_cid` tells them apart — see `adopt`.
                        self.adopt(task, role, &agents, &mut report);
                    }
                }
            } else {
                // A state the dispatcher has no rule for (the ledger's larger
                // machine, a later slice) is left alone: it is the ledger's to
                // move, not the dispatcher's to guess at.
                report.skipped += 1;
                log.line(&format!(
                    "dispatchd: task {} (role {role}) is {}; skipped",
                    task.id, task.state
                ));
            }
        }

        log.line(&format!(
            "dispatchd: pass {}: {}",
            self.passes,
            report.summary()
        ));
        Ok(report)
    }

    /// Starts an open task: decide what may be driven for its role, find a
    /// connected agent of that role, claim the task for it, and drive it.
    ///
    /// The plan comes first, before any agent is looked for and before the task
    /// is claimed: a role whose grants cover no script this dispatcher may drive
    /// is *not claimed* — claiming a task only to leave it would take it out of
    /// the hands of a dispatcher whose manifest does allow it, and the ledger
    /// has no way back to `open`.
    fn start(&mut self, task: &Task, role: &str, agents: &[Agent], report: &mut Reporter) {
        let log = Arc::clone(&self.log);
        let family = match workflow::plan_for(role, self.roles.as_ref()) {
            Plan::Drive(family) => family,
            Plan::Skipped(reason) => {
                report.skipped += 1;
                log.line(&format!(
                    "dispatchd: task {} (role {role}): {reason}; skipped",
                    task.id
                ));
                return;
            }
        };
        let Some(agent) = pick_agent(agents, role) else {
            report.skipped += 1;
            log.line(&format!(
                "dispatchd: task {} (role {role}): no connected agent of that role; skipped",
                task.id
            ));
            return;
        };
        let cid = agent.cid;
        match self.ledger.claim(&task.id, cid) {
            Ok(()) => {
                report.claimed += 1;
                log.line(&format!(
                    "dispatchd: task {} (role {role}) -> agent cid {cid}: claimed",
                    task.id
                ));
                self.retries
                    .insert(task.id.clone(), Attempt { cid, attempts: 0 });
                self.drive(task, role, family, cid, agents, report);
            }
            Err(reason) => {
                report.skipped += 1;
                log.line(&format!(
                    "dispatchd: task {} (role {role}) -> agent cid {cid}: claim refused: {reason}",
                    task.id
                ));
            }
        }
    }

    /// Adopts a `claimed` task this dispatcher holds no retry state for.
    ///
    /// The retry map is in memory, so a dispatcher that is stopped and started
    /// again remembers nothing — and a task it had claimed then looks like a
    /// stranger's. It is not: the ledger's own `agent_cid` names the holder, and
    /// when the register this pass read still lists that CID connected as
    /// *exactly* the task's role, the claim is this fleet's own stranded work.
    /// Adoption re-decides the plan from the manifest in hand and drives the task
    /// with a fresh attempt budget ([`MAX_ATTEMPTS`]); the ledger's row is left
    /// untouched until that drive finishes it, so a re-read is always consistent
    /// with itself.
    ///
    /// A claim whose agent is not connected, or is connected as some other role,
    /// is not ours to finish: it is left exactly as it is, and the log says why.
    fn adopt(&mut self, task: &Task, role: &str, agents: &[Agent], report: &mut Reporter) {
        let log = Arc::clone(&self.log);
        let stranded = task
            .agent_cid
            .filter(|cid| agent_of_role(agents, *cid, role).is_some());
        let Some(cid) = stranded else {
            report.skipped += 1;
            log.line(&format!(
                "dispatchd: task {} (role {role}) is claimed by another; skipped",
                task.id
            ));
            return;
        };
        let family = match workflow::plan_for(role, self.roles.as_ref()) {
            Plan::Drive(family) => family,
            Plan::Skipped(reason) => {
                report.skipped += 1;
                log.line(&format!(
                    "dispatchd: task {} (role {role}) is claimed but {reason}; skipped",
                    task.id
                ));
                return;
            }
        };
        log.line(&format!(
            "dispatchd: adopting stranded claim {} (agent cid {cid})",
            task.id
        ));
        self.drive(task, role, family, cid, agents, report);
    }

    /// Drives one claimed task through its planned family's script and finishes
    /// it, whichever way it goes.
    ///
    /// The agent is re-verified against the register first, every time: a guest
    /// CID is not a stable identity — a VM that died can be replaced by a new run
    /// that takes the same CID — so the CID a claim was made for is not on its own
    /// proof that the agent answering it is still the one of the task's role.
    /// `agents` is the register this pass read; a CID that is gone, or that is now
    /// connected as some other role, does not get driven: the failure is retryable
    /// and names the divergence, and the bounded retry path below either sees the
    /// register agree again or ends the task with that evidence.
    fn drive(
        &mut self,
        task: &Task,
        role: &str,
        family: &'static workflow::Family,
        cid: u64,
        agents: &[Agent],
        tally: &mut Reporter,
    ) {
        let log = Arc::clone(&self.log);
        let attempts = self.note_attempt(&task.id, cid);
        let outcome = if agent_of_role(agents, cid, role).is_some() {
            // The driver borrows the hub for the length of the run; the ledger
            // and the retry map are separate fields, so finishing the task
            // afterwards needs no lock and no re-borrow.
            let mut driver = AgentDriver::new(&self.operator, cid, TELL_TIMEOUT_MS);
            (family.workflow)(task, &mut driver).map(|report| (report, driver.calls()))
        } else {
            Err(divergent_agent(family, cid, role))
        };
        match outcome {
            Ok((report, calls)) => {
                // A run that wrote files names them; a read-only run carries the
                // line its script read out, and that line is the note. Either
                // way the note is what the run actually did.
                let note = workflow::done_note(&report, calls);
                // The talk step's `done` line names the run's first artifact, so
                // the report is read for it before the finish consumes nothing
                // of it — a run that left none (the read-only script) says so.
                let artifact = report.artifacts().first().cloned();
                let finished = self.ledger.finish(&task.id, Outcome::Done, &note);
                match finished {
                    Ok(()) => {
                        tally.done += 1;
                        log.line(&format!(
                            "dispatchd: task {} (role {role}) -> agent cid {cid} finished done: {note}",
                            task.id
                        ));
                        // The transition is durable: now the finisher tells its
                        // awarder (the talk step, bounded and logged).
                        self.announce(task, cid, Outcome::Done, artifact.as_deref(), &note);
                    }
                    Err(reason) => {
                        tally.failed += 1;
                        log.line(&format!(
                            "dispatchd: task {} (role {role}) -> agent cid {cid}: the ledger refused to finish it done: {reason}",
                            task.id
                        ));
                    }
                }
            }
            Err(err) => {
                let note = err.note();
                if err.is_retryable() && attempts < MAX_ATTEMPTS {
                    tally.deferred += 1;
                    log.line(&format!(
                        "dispatchd: task {} (role {role}) -> agent cid {cid}: {note} (attempt {attempts} of {MAX_ATTEMPTS}); leaving it claimed",
                        task.id
                    ));
                } else {
                    // Either the agent answered (and this is final), the grants
                    // refused it (final the same way), or the attempts are
                    // spent: the task ends with the evidence.
                    let note = if err.is_retryable() {
                        format!("{note} (after {attempts} attempt(s))")
                    } else {
                        note
                    };
                    let finished = self.ledger.finish(&task.id, Outcome::Failed, &note);
                    match finished {
                        Ok(()) => {
                            tally.failed += 1;
                            log.line(&format!(
                                "dispatchd: task {} (role {role}) -> agent cid {cid} finished failed: {note}",
                                task.id
                            ));
                            self.announce(task, cid, Outcome::Failed, None, &note);
                        }
                        Err(reason) => {
                            log.line(&format!(
                                "dispatchd: task {} (role {role}) -> agent cid {cid}: the ledger refused to finish it failed: {reason}",
                                task.id
                            ));
                        }
                    }
                }
            }
        }
    }

    /// The talk step: the agent that finished `task` says so to the task's
    /// awarder, when the board names one ([`crate::social::SocialPass::talk`]).
    ///
    /// A dispatcher without the social surface says nothing: it holds no seam
    /// to say it through, and it is exactly the claim-and-drive dispatcher it
    /// was. With the surface, every failure inside the step is logged there and
    /// swallowed — the claim loop does not see it, and never waits past the
    /// step's own bounded calls.
    fn announce(
        &mut self,
        task: &Task,
        cid: u64,
        outcome: Outcome,
        artifact: Option<&str>,
        note: &str,
    ) {
        if let Some(social) = &mut self.social {
            social.talk(task, cid, outcome, artifact, note);
        }
    }

    /// Counts one attempt at driving `id`, and answers which attempt this is.
    fn note_attempt(&mut self, id: &str, cid: u64) -> u32 {
        let attempt = self
            .retries
            .entry(id.to_string())
            .or_insert(Attempt { cid, attempts: 0 });
        attempt.cid = cid;
        attempt.attempts += 1;
        attempt.attempts
    }
}

/// The agent a task of `role` should go to: the connected agent of that role
/// with the lowest CID.
///
/// Lowest CID rather than "any": a fleet's assignment is then deterministic,
/// so the same pass over the same ledger and register twice drives the same
/// agents — which is what makes a dispatcher restart safe to reason about.
/// Role matching is exact equality (the role the hub accepted at the
/// handshake), never a prefix or a case fold.
pub fn pick_agent<'a>(agents: &'a [Agent], role: &str) -> Option<&'a Agent> {
    agents
        .iter()
        .filter(|agent| agent.role == role)
        .min_by_key(|agent| agent.cid)
}

/// The guest CIDs that are **busy** — an agent with a claim in hand — as the
/// social pass must read that.
///
/// Two sources, and both are facts the dispatcher already holds: the ledger
/// rows this pass read whose claimant has not ended (a claim left by an earlier
/// pass, a deferred retry, or another dispatcher's), and this dispatcher's own
/// retry state (the claim it took and is still driving). An agent in this set
/// is skipped by the social pass, so the board is never spoken to for an agent
/// that is mid-claim.
fn busy_agents(retries: &HashMap<String, Attempt>, tasks: &[Task]) -> BTreeSet<u64> {
    let mut busy: BTreeSet<u64> = retries.values().map(|attempt| attempt.cid).collect();
    for task in tasks {
        if !task.is_terminal()
            && let Some(cid) = task.agent_cid
        {
            busy.insert(cid);
        }
    }
    busy
}

/// The agent at `cid` connected as *exactly* `role`, when the register lists
/// one.
///
/// This is the one check that answers "is this CID still the agent a claim was
/// made for?" A guest CID is not a stable identity — a VM that died can be
/// replaced by a new run that takes the same CID — so the register read at the
/// top of the pass is the only thing that says so. Both the retry path (before
/// it drives) and the adoption path (before it adopts) ask this same question.
fn agent_of_role<'a>(agents: &'a [Agent], cid: u64, role: &str) -> Option<&'a Agent> {
    agents
        .iter()
        .find(|agent| agent.cid == cid && agent.role == role)
}

/// The retryable failure a claimed task hits when the register no longer lists
/// its agent at that CID with that role: a replaced VM, a reused guest CID, a
/// vanished session. The reason names the divergence, and it is the bounded
/// retry path — not an immediate finish — that decides what to do about it,
/// because the register may yet agree again.
fn divergent_agent(family: &workflow::Family, cid: u64, role: &str) -> DriveError {
    DriveError::Unreachable {
        // The first call the plan would have driven: the tool the run stopped
        // short of. Every table family lists at least one, so this never falls
        // to the family's own name.
        name: family
            .tools
            .first()
            .copied()
            .unwrap_or(family.name)
            .to_string(),
        reason: format!("the agent of cid {cid} is no longer {role}"),
    }
}

/// The [`Driver`] a production pass uses: every call goes to the hub as one
/// `POST /tell`, and the agent's own answer is what comes back.
///
/// The mapping from the hub's verdict to a [`DriveError`] is where the retry
/// policy is decided: an answer (either way) is final, and anything that
/// stopped the answer from arriving is not.
struct AgentDriver<'a, O: Operator> {
    operator: &'a O,
    cid: u64,
    timeout_ms: u64,
    calls: u64,
}

impl<'a, O: Operator> AgentDriver<'a, O> {
    fn new(operator: &'a O, cid: u64, timeout_ms: u64) -> Self {
        Self {
            operator,
            cid,
            timeout_ms,
            calls: 0,
        }
    }
}

impl<O: Operator> Driver for AgentDriver<'_, O> {
    fn call(&mut self, name: &str, args: serde_json::Value) -> Result<String, DriveError> {
        self.calls += 1;
        let unavailable = |reason: String| DriveError::Unreachable {
            name: name.to_string(),
            reason,
        };
        match self.operator.tell(self.cid, name, &args, self.timeout_ms) {
            Ok(Tell::Answered { ok: true, output }) => Ok(output),
            Ok(Tell::Answered { ok: false, output }) => Err(DriveError::Refused {
                name: name.to_string(),
                output,
            }),
            // The hub's grants refused it: the policy's own answer, and final —
            // another pass would be refused just the same. The note is `refused`
            // rather than `unavailable`, and it is not retried.
            Ok(Tell::Refused { reason }) => Err(DriveError::Refused {
                name: name.to_string(),
                output: reason,
            }),
            Ok(Tell::TimedOut { waited_ms }) => Err(unavailable(format!(
                "the agent did not answer within {waited_ms} ms"
            ))),
            Ok(Tell::NotConnected) => Err(unavailable(format!(
                "agent cid {} is not connected",
                self.cid
            ))),
            Ok(Tell::UnknownCid) => Err(unavailable(format!(
                "the hub serves no agent with guest CID {}",
                self.cid
            ))),
            Err(reason) => Err(unavailable(reason)),
        }
    }

    fn calls(&self) -> u64 {
        self.calls
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::rc::Rc;

    use crate::testing::{FakeAgent, FakeLedger, FakeOperator, FakeSocial, Recording, agent, task};
    use crate::views::Tell;

    /// A dispatcher over the fakes, with its log kept for assertions.
    fn dispatcher(
        ledger: FakeLedger,
        operator: FakeOperator,
    ) -> (Dispatcher<FakeLedger, FakeOperator>, Arc<Recording>) {
        let log = Arc::new(Recording::default());
        let dispatcher = Dispatcher::new(
            ledger,
            operator,
            Arc::clone(&log) as Arc<dyn Logger + Send + Sync>,
        );
        (dispatcher, log)
    }

    /// A dispatcher holding the codex's grants, so its plans are the ones a
    /// `--roles` run would make.
    fn dispatcher_with_roles(
        ledger: FakeLedger,
        operator: FakeOperator,
        roles: &[(&str, &[&str])],
    ) -> (Dispatcher<FakeLedger, FakeOperator>, Arc<Recording>) {
        let log = Arc::new(Recording::default());
        let manifest = crate::roles::Manifest::parse(
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
        .expect("a well-formed test manifest");
        let dispatcher = Dispatcher::with_roles(
            ledger,
            operator,
            Arc::clone(&log) as Arc<dyn Logger + Send + Sync>,
            Some(manifest),
        );
        (dispatcher, log)
    }

    /// The codex's own grants for the roles these tests drive.
    const GRANTS: &[(&str, &[&str])] = &[
        (
            "builder",
            &[
                "bash",
                "read_file",
                "write_file",
                "edit_file",
                "list",
                "search",
            ],
        ),
        ("librarian", &["read_memory", "search", "transcript_search"]),
        ("architect", &["bash", "read_file", "list", "search"]),
        (
            "mayor",
            &[
                "read_file",
                "list",
                "search",
                "transcript_search",
                "assign_task",
                "spawn_agent",
                "hibernate_agent",
                "approve_plan",
                "approve_merge",
                "escalate",
                "send_message",
            ],
        ),
        (
            "legal-gov-judge",
            &["read_case_record", "search_precedent", "issue_ruling"],
        ),
    ];

    /// With the grants loaded, a pass still drives a builder to write and a
    /// librarian to read — the librarian through the read-only script, because
    /// that is all its grants (and the harness) allow.
    #[test]
    fn a_grants_aware_pass_drives_the_read_only_script_for_a_librarian() {
        let ledger = FakeLedger::with_tasks(vec![
            task("t1", "open", Some("builder")),
            task("t2", "open", Some("librarian")),
        ])
        .with_agent(300, "builder")
        .with_agent(310, "librarian");
        let operator = FakeOperator::default()
            .with_agent(300, FakeAgent::default())
            .with_agent(
                310,
                FakeAgent::default().with_file("handbook.md", "do the plaza's work\n"),
            );
        let (mut dispatcher, log) = dispatcher_with_roles(ledger, operator, GRANTS);

        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!((report.claimed, report.done), (2, 2));

        // The builder writes; the librarian only searches. Both complete.
        assert_eq!(
            dispatcher.operator.sequence(),
            vec![
                (300, "write_file".to_string()),
                (300, "read_file".to_string()),
                (310, "search".to_string()),
            ]
        );
        let finishes = dispatcher.ledger.finishes.borrow();
        assert_eq!(finishes[0].1, Outcome::Done);
        assert_eq!(finishes[0].2, "artifacts: build-notes.txt (2 tool call(s))");
        assert_eq!(finishes[1].1, Outcome::Done);
        assert!(
            finishes[1].2.starts_with("read-only: search"),
            "{}",
            finishes[1].2
        );
        drop(finishes);
        assert!(log.text().contains("finished done"), "log: {}", log.text());
        // Nothing was skipped: both roles could be driven.
        assert_eq!(report.skipped, 0, "{}", log.text());
    }

    /// A role that may read and search but not write (and has no shell) falls
    /// back to the read-only script, exactly as a librarian does — the plan is
    /// the grants, not the family name.
    #[test]
    fn a_role_without_a_write_grant_runs_the_read_only_script() {
        let ledger =
            FakeLedger::with_task(task("t1", "open", Some("mayor"))).with_agent(320, "mayor");
        let operator = FakeOperator::default()
            .with_agent(320, FakeAgent::default().with_file("design.md", "do it\n"));
        let (mut dispatcher, _log) = dispatcher_with_roles(ledger, operator, GRANTS);

        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!((report.claimed, report.done), (1, 1));
        assert_eq!(
            dispatcher.operator.sequence(),
            vec![(320, "search".to_string())],
            "a mayor may not write, so it does not"
        );
        assert!(
            dispatcher
                .ledger
                .finishes
                .borrow()
                .iter()
                .all(|(_, outcome, note)| *outcome == Outcome::Done
                    && note.starts_with("read-only:"))
        );
    }

    /// A role whose grants cover no tool this dispatcher may drive is never
    /// claimed and never driven: the task stays `open`, and the log says why.
    #[test]
    fn a_role_the_dispatcher_may_not_drive_is_left_open() {
        let ledger = FakeLedger::with_tasks(vec![
            task("t1", "open", Some("builder")),
            task("t2", "open", Some("legal-gov-judge")),
            task("t3", "open", Some("intruder")),
        ])
        .with_agent(300, "builder")
        .with_agent(310, "legal-gov-judge")
        .with_agent(320, "intruder");
        let operator = FakeOperator::default().with_agent(300, FakeAgent::default());
        let (mut dispatcher, log) = dispatcher_with_roles(ledger, operator, GRANTS);

        let report = dispatcher.pass().expect("the pass runs");
        // Only the builder was driveable.
        assert_eq!(report.considered, 3);
        assert_eq!((report.claimed, report.done, report.skipped), (1, 1, 2));

        // The two undriveable tasks are untouched — not claimed, not driven,
        // not finished — so a dispatcher whose manifest allows them can take
        // them later.
        assert_eq!(
            dispatcher.ledger.claims.borrow().as_slice(),
            &[("t1".to_string(), 300)],
            "only the builder's task was claimed, for the builder's agent"
        );
        assert_eq!(dispatcher.ledger.task("t2").state, "open");
        assert_eq!(dispatcher.ledger.task("t3").state, "open");
        assert!(
            dispatcher
                .operator
                .calls
                .borrow()
                .iter()
                .all(|(cid, _, _)| *cid == 300),
            "only the builder's agent was driven"
        );
        let text = log.text();
        assert!(
            text.contains(
                "dispatchd: task t2 (role legal-gov-judge): the role grants no tool this dispatcher may drive; skipped"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "dispatchd: task t3 (role intruder): the role is not in the manifest; skipped"
            ),
            "{text}"
        );
    }

    /// A hub that refuses a call on the role's grants is final, not a retry: the
    /// task ends `failed` at once with the hub's own words. This is the
    /// backstop's echo — a dispatcher whose manifest disagrees with the hub's
    /// policy, or a hub started with a different manifest, still ends honestly.
    #[test]
    fn a_hub_refusal_finishes_the_task_at_once() {
        let ledger =
            FakeLedger::with_task(task("t1", "open", Some("builder"))).with_agent(300, "builder");
        let operator = FakeOperator::answering(Tell::Refused {
            reason: "role builder does not grant write_file".to_string(),
        });
        let (mut dispatcher, log) = dispatcher(ledger, operator);

        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!(
            report,
            Reporter {
                considered: 1,
                claimed: 1,
                done: 0,
                failed: 1,
                skipped: 0,
                deferred: 0,
            },
            "a refusal is final, not deferred"
        );
        let finishes = dispatcher.ledger.finishes.borrow();
        assert_eq!(finishes.len(), 1);
        assert_eq!(finishes[0].1, Outcome::Failed);
        assert_eq!(
            finishes[0].2,
            "tool write_file refused: role builder does not grant write_file"
        );
        drop(finishes);
        assert_eq!(dispatcher.ledger.task("t1").state, "failed");
        assert!(
            log.text()
                .contains("finished failed: tool write_file refused:"),
            "{}",
            log.text()
        );
    }

    /// The whole loop, once: an open roled task whose agent is connected is
    /// claimed for that agent, driven through its family's script, and
    /// finished `done` with the artifacts its workspace now holds.
    #[test]
    fn a_pass_claims_drives_and_finishes_a_roled_task() {
        let ledger =
            FakeLedger::with_task(task("t3", "open", Some("builder"))).with_agent(300, "builder");
        let operator = FakeOperator::default().with_agent(300, FakeAgent::default());
        let (mut dispatcher, log) = dispatcher(ledger, operator);

        let report = dispatcher.pass().expect("the pass reads both daemons");
        assert_eq!(
            report,
            Reporter {
                considered: 1,
                claimed: 1,
                done: 1,
                failed: 0,
                skipped: 0,
                deferred: 0,
            }
        );

        // The agent was driven by the builder's script, on its own CID.
        assert_eq!(
            dispatcher.operator.sequence(),
            vec![
                (300, "write_file".to_string()),
                (300, "read_file".to_string()),
            ]
        );

        // The task ended `done`, with a note naming its artifact and the calls
        // it took.
        let finishes = dispatcher.ledger.finishes.borrow();
        assert_eq!(finishes.len(), 1);
        assert_eq!(finishes[0].0, "t3");
        assert_eq!(finishes[0].1, Outcome::Done);
        assert_eq!(finishes[0].2, "artifacts: build-notes.txt (2 tool call(s))");
        drop(finishes);

        // And the artifact really is in the agent's workspace.
        let agent = dispatcher
            .operator
            .agent_file(300, "build-notes.txt")
            .expect("the round trip left the artifact");
        assert_eq!(agent, "task t3: do t3");

        // The log carries every step in the dispatcher's own voice.
        let text = log.text();
        for expected in [
            "dispatchd: pass 1: 1 task(s) in the ledger, 1 agent(s) connected",
            "dispatchd: task t3 (role builder) -> agent cid 300: claimed",
            "dispatchd: task t3 (role builder) -> agent cid 300 finished done: artifacts: build-notes.txt (2 tool call(s))",
            "dispatchd: pass 1: 1 considered, 1 claimed, 1 done, 0 failed, 0 deferred, 0 skipped",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
        }
    }

    /// A pass drives **every** actionable task, in turn: three roles, three
    /// tasks, all done — which is the shape `--once` has to satisfy.
    ///
    /// With no manifest there is no grant policy, so each role drives its own
    /// family's script — and the librarian's script is read-only, because the
    /// codex's librarian grants (and the harness's tools) leave it nothing to
    /// write with.
    #[test]
    fn a_pass_dispatches_every_actionable_task_in_turn() {
        let ledger = FakeLedger::with_tasks(vec![
            task("t1", "open", Some("builder")),
            task("t2", "open", Some("librarian")),
            task("t3", "open", Some("architect")),
        ])
        .with_agent(300, "builder")
        .with_agent(310, "librarian")
        .with_agent(320, "architect");
        let operator = FakeOperator::default()
            .with_agent(300, FakeAgent::default())
            .with_agent(
                310,
                FakeAgent::default().with_file("handbook.md", "do the plaza's work\n"),
            )
            .with_agent(320, FakeAgent::default());
        let (mut dispatcher, _log) = dispatcher(ledger, operator);

        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!(report.considered, 3);
        assert_eq!((report.claimed, report.done), (3, 3));

        // Each role ran its own family's script, on its own agent. The
        // librarian's is one read (a search) and no write at all.
        assert_eq!(
            dispatcher.operator.sequence(),
            vec![
                (300, "write_file".to_string()),
                (300, "read_file".to_string()),
                (310, "search".to_string()),
                (320, "write_file".to_string()),
                (320, "read_file".to_string()),
            ]
        );
        assert_eq!(
            dispatcher
                .operator
                .agent_file(310, "handbook.md")
                .expect("the librarian's workspace is untouched"),
            "do the plaza's work\n",
            "a read-only run writes nothing"
        );
        assert_eq!(
            dispatcher
                .operator
                .agent_file(320, "notes.txt")
                .expect("the generic artifact"),
            "task t3: do t3"
        );
        let finishes = dispatcher.ledger.finishes.borrow();
        assert_eq!(
            finishes
                .iter()
                .map(|(id, outcome, note)| (id.as_str(), *outcome, note.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (
                    "t1",
                    Outcome::Done,
                    "artifacts: build-notes.txt (2 tool call(s))"
                ),
                (
                    "t2",
                    Outcome::Done,
                    "read-only: search \"do\" matched 1 line(s) in 1 file(s) scanned; \
                     first: handbook.md:1: do the plaza's work (1 tool call(s))"
                ),
                ("t3", Outcome::Done, "artifacts: notes.txt (2 tool call(s))"),
            ]
        );
    }

    /// A task whose role has no CONNECTED agent is left `open` and logged as
    /// skipped: the dispatcher never invents an agent for it, and never blames
    /// the task for the fleet's shape.
    #[test]
    fn a_task_whose_role_is_not_connected_stays_open() {
        let ledger =
            FakeLedger::with_task(task("t1", "open", Some("builder"))).with_agent(310, "librarian");
        let (mut dispatcher, log) = dispatcher(ledger, FakeOperator::default());

        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!(report.considered, 1);
        assert_eq!((report.claimed, report.skipped), (0, 1));
        // The task is untouched: no claim was even asked for.
        assert!(dispatcher.ledger.claims.borrow().is_empty());
        assert_eq!(dispatcher.ledger.task("t1").state, "open");
        assert!(
            log.text().contains(
                "dispatchd: task t1 (role builder): no connected agent of that role; skipped"
            ),
            "log: {}",
            log.text()
        );

        // And it is picked up on the pass where the agent *is* connected.
        dispatcher.ledger.agents.push(agent(300, "builder"));
        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!((report.claimed, report.done), (1, 1));
        assert_eq!(dispatcher.ledger.task("t1").state, "done");
    }

    /// A task that names no role is never claimed: "for each open task with a
    /// role" is taken literally.
    #[test]
    fn a_task_with_no_role_is_never_claimed() {
        let ledger =
            FakeLedger::with_tasks(vec![task("t1", "open", None), task("t2", "open", Some(""))])
                .with_agent(300, "builder");
        let (mut dispatcher, log) = dispatcher(ledger, FakeOperator::default());

        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!(report.considered, 0);
        assert_eq!(report.skipped, 2);
        assert!(dispatcher.ledger.claims.borrow().is_empty());
        assert!(dispatcher.ledger.finishes.borrow().is_empty());
        assert_eq!(log.text().matches("names no role; skipped").count(), 2);
    }

    /// A terminal task is never touched again, whatever it names.
    #[test]
    fn a_terminal_task_is_never_touched() {
        let ledger = FakeLedger::with_tasks(vec![
            task("t1", "done", Some("builder")),
            task("t2", "failed", Some("builder")),
            task("t3", "withdrawn", Some("builder")),
        ])
        .with_agent(300, "builder");
        let operator = FakeOperator::default().with_agent(300, FakeAgent::default());
        let (mut dispatcher, _log) = dispatcher(ledger, operator);

        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!(report, Reporter::default());
        assert!(dispatcher.operator.calls.borrow().is_empty());
        assert!(dispatcher.ledger.claims.borrow().is_empty());
        assert!(dispatcher.ledger.finishes.borrow().is_empty());
    }

    /// A state the dispatcher has no rule for is left alone: not claimed, not
    /// failed, just logged.
    #[test]
    fn a_state_the_dispatcher_has_no_rule_for_is_left_alone() {
        let ledger = FakeLedger::with_task(task("t1", "verifying", Some("builder")))
            .with_agent(300, "builder");
        let (mut dispatcher, log) = dispatcher(ledger, FakeOperator::default());
        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!(report.skipped, 1);
        assert_eq!(report.considered, 0);
        assert!(dispatcher.ledger.claims.borrow().is_empty());
        assert!(
            log.text()
                .contains("dispatchd: task t1 (role builder) is verifying; skipped"),
            "log: {}",
            log.text()
        );
    }

    /// An agent that answers and refuses is final: the task is finished
    /// `failed` at once, with the agent's own words — no retry for an answer.
    #[test]
    fn a_tool_refusal_finishes_the_task_failed_at_once() {
        let ledger =
            FakeLedger::with_task(task("t1", "open", Some("builder"))).with_agent(300, "builder");
        let operator = FakeOperator::default().with_agent(
            300,
            FakeAgent::failing(DriveError::Refused {
                name: "write_file".to_string(),
                output: "denied: path \"../escape.txt\" contains \"..\"".to_string(),
            }),
        );
        let (mut dispatcher, log) = dispatcher(ledger, operator);

        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!((report.done, report.failed, report.deferred), (0, 1, 0));
        let finishes = dispatcher.ledger.finishes.borrow();
        assert_eq!(finishes.len(), 1);
        assert_eq!(finishes[0].1, Outcome::Failed);
        assert_eq!(
            finishes[0].2,
            "tool write_file refused: denied: path \"../escape.txt\" contains \"..\""
        );
        drop(finishes);
        assert_eq!(dispatcher.ledger.task("t1").state, "failed");
        assert!(
            log.text()
                .contains("finished failed: tool write_file refused:"),
            "log: {}",
            log.text()
        );
    }

    /// An agent that cannot be reached is retried for a bounded number of
    /// passes, and then the task is finished `failed` with the evidence — the
    /// task is never left hanging without a record.
    #[test]
    fn an_unreachable_agent_is_retried_for_a_bounded_number_of_passes() {
        let ledger =
            FakeLedger::with_task(task("t1", "open", Some("builder"))).with_agent(300, "builder");
        let operator =
            FakeOperator::unreachable("cannot connect to http://127.0.0.1:19080: refused");
        let (mut dispatcher, log) = dispatcher(ledger, operator);

        // Passes 1 and 2: claimed, tried, deferred — still claimed.
        for pass in 1..MAX_ATTEMPTS {
            let report = dispatcher.pass().expect("the pass runs");
            assert_eq!(report.deferred, 1, "pass {pass}");
            assert_eq!(report.failed, 0, "pass {pass}");
            assert_eq!(dispatcher.ledger.task("t1").state, "claimed");
            assert!(dispatcher.ledger.finishes.borrow().is_empty());
        }
        assert_eq!(dispatcher.ledger.claims.borrow().len(), 1, "claimed once");

        // The last attempt: the task ends `failed`, with how many attempts it
        // took and why.
        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!((report.failed, report.deferred), (1, 0));
        let finishes = dispatcher.ledger.finishes.borrow();
        assert_eq!(finishes.len(), 1);
        assert_eq!(finishes[0].1, Outcome::Failed);
        assert_eq!(
            finishes[0].2,
            "tool write_file unavailable: cannot connect to http://127.0.0.1:19080: refused (after 3 attempt(s))"
        );
        drop(finishes);
        assert_eq!(dispatcher.ledger.task("t1").state, "failed");

        let text = log.text();
        assert!(
            text.contains("(attempt 1 of 3); leaving it claimed"),
            "{text}"
        );
        assert!(
            text.contains("(attempt 2 of 3); leaving it claimed"),
            "{text}"
        );
        assert!(
            text.contains("finished failed: tool write_file unavailable"),
            "{text}"
        );
    }

    /// A timeout and a "not connected" are the same class as an unreachable
    /// hub: retryable, and reported in the hub's own words.
    #[test]
    fn a_timeout_and_a_disconnected_agent_are_both_retryable() {
        for answer in [
            Tell::TimedOut { waited_ms: 5_000 },
            Tell::NotConnected,
            Tell::UnknownCid,
        ] {
            let ledger = FakeLedger::with_task(task("t1", "open", Some("builder")))
                .with_agent(300, "builder");
            let (mut dispatcher, log) = dispatcher(ledger, FakeOperator::answering(answer.clone()));
            let report = dispatcher.pass().expect("the pass runs");
            assert_eq!(report.deferred, 1, "{answer:?}");
            assert_eq!(dispatcher.ledger.task("t1").state, "claimed", "{answer:?}");
            assert!(
                log.text().contains("leaving it claimed"),
                "{answer:?}: {}",
                log.text()
            );
        }
    }

    /// A claimed task whose claimant is not a connected agent of the task's role
    /// is left alone — untouched, still claimed. A builder *is* connected here,
    /// but the claim names a CID nothing serves, so it is not this fleet's to
    /// finish (only a claim the register still backs is adopted; see
    /// `a_claimed_task_is_adopted_after_a_restart`).
    #[test]
    fn a_claim_the_dispatcher_does_not_own_is_left_alone() {
        let mut claimed = task("t1", "claimed", Some("builder"));
        claimed.agent_cid = Some(999);
        let ledger = FakeLedger::with_task(claimed).with_agent(300, "builder");
        let operator = FakeOperator::default().with_agent(300, FakeAgent::default());
        let (mut dispatcher, log) = dispatcher(ledger, operator);

        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!(report.skipped, 1);
        assert!(dispatcher.operator.calls.borrow().is_empty());
        assert!(dispatcher.ledger.finishes.borrow().is_empty());
        assert_eq!(dispatcher.ledger.task("t1").state, "claimed");
        assert!(
            log.text().contains("is claimed by another; skipped"),
            "log: {}",
            log.text()
        );
    }

    /// A dispatcher that is stopped and started again adopts the claim it left
    /// behind. The ledger and the hub are the same daemons; only the dispatcher
    /// — and its in-memory retry map — is new. A claimed task whose agent is
    /// still connected as the task's role is this fleet's own stranded work: it is
    /// driven to done (with the adoption logged) instead of being parked forever
    /// as "claimed by another".
    #[test]
    fn a_claimed_task_is_adopted_after_a_restart() {
        let ledger = Rc::new(
            FakeLedger::with_task(task("t1", "open", Some("builder"))).with_agent(300, "builder"),
        );
        let operator = Rc::new(
            FakeOperator::unreachable("cannot connect to http://127.0.0.1:19080: refused")
                .with_agent(300, FakeAgent::default()),
        );
        let log = Arc::new(Recording::default());

        // The first dispatcher claims t1 and, with the hub down, defers it — the
        // claim the ledger records.
        let mut first = Dispatcher::new(
            Rc::clone(&ledger),
            Rc::clone(&operator),
            Arc::clone(&log) as Arc<dyn Logger + Send + Sync>,
        );
        let report = first.pass().expect("the pass runs");
        assert_eq!((report.claimed, report.deferred), (1, 1));
        assert_eq!(ledger.task("t1").state, "claimed");
        assert_eq!(ledger.task("t1").agent_cid, Some(300));
        drop(first); // the dispatcher is gone; a restart leaves the ledger as it is.

        // The hub comes back, and a *fresh* dispatcher — its retry map empty —
        // runs over the same daemons.
        operator.recover();
        let mut second = Dispatcher::new(
            Rc::clone(&ledger),
            Rc::clone(&operator),
            Arc::clone(&log) as Arc<dyn Logger + Send + Sync>,
        );
        let report = second.pass().expect("the pass runs");

        // It adopted the stranded claim: not claimed again, driven to done.
        assert_eq!((report.claimed, report.done, report.skipped), (0, 1, 0));
        // The full sequence: the first dispatcher's one attempt (refused at the
        // transport), then the adopted run's write and read-back.
        assert_eq!(
            operator.sequence(),
            vec![
                (300, "write_file".to_string()),
                (300, "write_file".to_string()),
                (300, "read_file".to_string()),
            ]
        );
        assert_eq!(ledger.task("t1").state, "done");
        assert_eq!(
            operator.agent_file(300, "build-notes.txt").as_deref(),
            Some("task t1: do t1")
        );
        // The claim was made once, by the first dispatcher: adoption never
        // re-claims.
        assert_eq!(
            ledger.claims.borrow().as_slice(),
            &[("t1".to_string(), 300)]
        );

        let text = log.text();
        assert!(
            text.contains("dispatchd: adopting stranded claim t1 (agent cid 300)"),
            "{text}"
        );
        assert!(
            text.contains("dispatchd: task t1 (role builder) -> agent cid 300 finished done"),
            "{text}"
        );
    }

    /// A stranded claim the register does not back is left exactly as it is: the
    /// fleet cannot tell it is its own, so it does not touch it. Two shapes, both
    /// untouched — the agent gone, and the CID now connected as another role (a
    /// replaced VM that took the guest CID).
    #[test]
    fn a_stranded_claim_the_register_does_not_back_is_left_alone() {
        for (name, agents, cid) in [
            ("agent gone", vec![agent(400, "builder")], 300u64),
            ("role changed", vec![agent(300, "librarian")], 300u64),
            ("no agent recorded", vec![agent(300, "builder")], 0u64),
        ] {
            let mut claimed = task("t1", "claimed", Some("builder"));
            // `cid == 0` models a ledger that recorded no claiming agent at all:
            // the field is absent, which no connected agent can match.
            claimed.agent_cid = (cid != 0).then_some(cid);
            let mut ledger = FakeLedger::with_task(claimed);
            ledger.agents = agents;
            let operator = FakeOperator::default().with_agent(300, FakeAgent::default());
            let (mut dispatcher, log) = dispatcher(ledger, operator);

            let report = dispatcher.pass().expect("the pass runs");
            assert_eq!(report.skipped, 1, "{name}");
            assert!(dispatcher.operator.calls.borrow().is_empty(), "{name}");
            assert!(dispatcher.ledger.claims.borrow().is_empty(), "{name}");
            assert!(dispatcher.ledger.finishes.borrow().is_empty(), "{name}");
            assert_eq!(dispatcher.ledger.task("t1").state, "claimed", "{name}");
            assert!(
                log.text().contains("is claimed by another; skipped"),
                "{name}: {}",
                log.text()
            );
        }
    }

    /// A retry never drives a stale CID: when a task's agent VM dies and a new run
    /// takes the same guest CID, the register now lists that CID as a different
    /// role — and the next pass must not drive whatever took the CID (with no
    /// manifest the hub would admit it, and the task would be marked done on the
    /// impostor). The divergence is retryable and bounded: the task ends failed
    /// naming it, and the impostor's hub is never asked to do anything.
    #[test]
    fn a_retry_never_drives_a_stale_cid() {
        // The task is claimed, with retry state for cid 300 (the hub was down).
        let ledger =
            FakeLedger::with_task(task("t1", "open", Some("builder"))).with_agent(300, "builder");
        let operator =
            FakeOperator::unreachable("cannot connect to http://127.0.0.1:19080: refused")
                .with_agent(300, FakeAgent::default());
        let (mut dispatcher, log) = dispatcher(ledger, operator);
        let first = dispatcher.pass().expect("the pass runs");
        assert_eq!((first.claimed, first.deferred), (1, 1));
        let calls_before = dispatcher.operator.calls.borrow().len();

        // The VM is replaced: cid 300 is now the impostor's, connected as another
        // role. The hub itself is up and would happily run the call.
        dispatcher.ledger.agents = vec![agent(300, "intruder")];
        dispatcher.operator.recover();

        // One more deferred pass, then the bound is spent and the task ends
        // failed — with the divergence named.
        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!((report.failed, report.deferred), (0, 1));
        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!((report.failed, report.deferred), (1, 0));

        let finishes = dispatcher.ledger.finishes.borrow();
        assert_eq!(finishes.len(), 1);
        assert_eq!(finishes[0].1, Outcome::Failed);
        assert_eq!(
            finishes[0].2,
            "tool write_file unavailable: the agent of cid 300 is no longer builder (after 3 attempt(s))"
        );
        drop(finishes);
        assert_eq!(dispatcher.ledger.task("t1").state, "failed");
        // The impostor's hub was never driven: the only call it ever saw was the
        // first pass's, before the VM was replaced.
        assert_eq!(dispatcher.operator.calls.borrow().len(), calls_before);
        assert!(
            log.text()
                .contains("tool write_file unavailable: the agent of cid 300 is no longer builder"),
            "{}",
            log.text()
        );
    }

    /// The other shape of a stale CID: the agent's session vanished entirely (its
    /// VM died and nothing took the CID). The retry re-verifies the register,
    /// finds no agent at that CID, and ends the task failed naming the divergence
    /// — it never drives a CID the register no longer holds.
    #[test]
    fn a_retry_never_drives_a_cid_whose_agent_vanished() {
        let ledger =
            FakeLedger::with_task(task("t1", "open", Some("builder"))).with_agent(300, "builder");
        let operator =
            FakeOperator::unreachable("cannot connect to http://127.0.0.1:19080: refused");
        let (mut dispatcher, log) = dispatcher(ledger, operator);
        let first = dispatcher.pass().expect("the pass runs");
        assert_eq!((first.claimed, first.deferred), (1, 1));
        let calls_before = dispatcher.operator.calls.borrow().len();

        // The agent is gone from the register: nothing is connected at cid 300.
        dispatcher.ledger.agents.clear();

        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!((report.failed, report.deferred), (0, 1));
        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!((report.failed, report.deferred), (1, 0));

        let finishes = dispatcher.ledger.finishes.borrow();
        assert_eq!(
            finishes[0].2,
            "tool write_file unavailable: the agent of cid 300 is no longer builder (after 3 attempt(s))"
        );
        drop(finishes);
        assert_eq!(dispatcher.ledger.task("t1").state, "failed");
        assert_eq!(dispatcher.operator.calls.borrow().len(), calls_before);
        assert!(
            log.text().contains("is no longer builder"),
            "{}",
            log.text()
        );
    }

    /// Adoption re-arms the retry bound but does not remove it: a task adopted
    /// after a restart is still given up on after [`MAX_ATTEMPTS`] passes, so a
    /// flapping fleet cannot keep a task claimed forever. Being adopted *twice*
    /// (two restarts) is no different — the second adoption spends its own budget
    /// and ends the task.
    #[test]
    fn the_adoption_cannot_retry_forever() {
        fn over(
            ledger: &Rc<FakeLedger>,
            operator: &Rc<FakeOperator>,
            log: &Arc<Recording>,
        ) -> Dispatcher<Rc<FakeLedger>, Rc<FakeOperator>> {
            Dispatcher::new(
                Rc::clone(ledger),
                Rc::clone(operator),
                Arc::clone(log) as Arc<dyn Logger + Send + Sync>,
            )
        }

        let ledger = Rc::new(
            FakeLedger::with_task(task("t1", "open", Some("builder"))).with_agent(300, "builder"),
        );
        let operator = Rc::new(FakeOperator::unreachable(
            "cannot connect to http://127.0.0.1:19080: refused",
        ));
        let log = Arc::new(Recording::default());

        // First dispatcher claims the task, then is killed with the hub still down.
        let mut first = over(&ledger, &operator, &log);
        assert_eq!(first.pass().expect("the pass runs").deferred, 1);
        drop(first);

        // A second dispatcher adopts it and, before it can finish, is killed too:
        // the claim survives for a third.
        let mut second = over(&ledger, &operator, &log);
        for _ in 0..2 {
            assert_eq!(second.pass().expect("the pass runs").deferred, 1);
        }
        drop(second);

        // The third adopts it again and spends the whole budget: the task ends
        // failed. Adopted twice, but not retried forever.
        let mut third = over(&ledger, &operator, &log);
        for pass in 1..=MAX_ATTEMPTS {
            let report = third.pass().expect("the pass runs");
            let expected = if pass < MAX_ATTEMPTS { (0, 1) } else { (1, 0) };
            assert_eq!((report.failed, report.deferred), expected, "pass {pass}");
        }
        assert_eq!(ledger.task("t1").state, "failed");

        let text = log.text();
        assert_eq!(
            text.matches("dispatchd: adopting stranded claim t1 (agent cid 300)")
                .count(),
            2,
            "adopted exactly twice:\n{text}"
        );
    }

    /// A claim the ledger refuses leaves the task alone and is logged: the
    /// dispatcher drives nothing it does not hold.
    #[test]
    fn a_refused_claim_leaves_the_task_alone() {
        let mut ledger =
            FakeLedger::with_task(task("t1", "open", Some("builder"))).with_agent(300, "builder");
        ledger.claim_refusal = Some("a task may not go from claimed to claimed".to_string());
        let operator = FakeOperator::default().with_agent(300, FakeAgent::default());
        let (mut dispatcher, log) = dispatcher(ledger, operator);

        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!((report.claimed, report.skipped), (0, 1));
        assert!(dispatcher.operator.calls.borrow().is_empty());
        assert!(
            log.text()
                .contains("claim refused: a task may not go from claimed to claimed"),
            "log: {}",
            log.text()
        );
    }

    /// A ledger that cannot be read fails the pass, and nothing is touched: a
    /// pass that cannot see the world must not act on a guess.
    #[test]
    fn a_pass_that_cannot_read_the_ledger_touches_nothing() {
        let mut ledger =
            FakeLedger::with_task(task("t1", "open", Some("builder"))).with_agent(300, "builder");
        ledger.down = Some("cannot connect to http://127.0.0.1:8899: refused".to_string());
        let operator = FakeOperator::default().with_agent(300, FakeAgent::default());
        let (mut dispatcher, _log) = dispatcher(ledger, operator);

        let err = dispatcher.pass().expect_err("the ledger is down");
        assert!(err.contains("cannot connect"), "{err}");
        assert!(dispatcher.operator.calls.borrow().is_empty());
        assert!(dispatcher.ledger.claims.borrow().is_empty());
        assert_eq!(dispatcher.passes(), 1);
    }

    /// A finish the ledger refuses is reported, not hidden: the dispatcher
    /// believes the ledger's state machine over its own bookkeeping, and says
    /// so.
    #[test]
    fn a_finish_the_ledger_refuses_is_reported() {
        let mut ledger =
            FakeLedger::with_task(task("t1", "open", Some("builder"))).with_agent(300, "builder");
        ledger.finish_refusal = Some("a task may not go from done to done".to_string());
        let operator = FakeOperator::default().with_agent(300, FakeAgent::default());
        let (mut dispatcher, log) = dispatcher(ledger, operator);

        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!((report.claimed, report.done, report.failed), (1, 0, 1));
        assert!(
            log.text().contains(
                "the ledger refused to finish it done: a task may not go from done to done"
            ),
            "log: {}",
            log.text()
        );
    }

    /// The agent pick: the connected agent of the role with the lowest CID,
    /// exactly the role, and nothing when the role has no agent.
    #[test]
    fn the_agent_pick_is_the_lowest_cid_of_that_role() {
        let agents = vec![
            agent(320, "librarian"),
            agent(310, "builder"),
            agent(305, "builder"),
            agent(300, "builder "),
        ];
        assert_eq!(pick_agent(&agents, "builder").map(|a| a.cid), Some(305));
        assert_eq!(pick_agent(&agents, "librarian").map(|a| a.cid), Some(320));
        assert_eq!(pick_agent(&agents, "Builder"), None);
        assert_eq!(pick_agent(&agents, "build"), None);
        assert_eq!(pick_agent(&[], "builder"), None);
    }

    /// The reporter's summary is the log line a pass ends with.
    #[test]
    fn the_summary_is_the_passes_tally() {
        let report = Reporter {
            considered: 3,
            claimed: 2,
            done: 1,
            failed: 1,
            skipped: 4,
            deferred: 1,
        };
        assert_eq!(
            report.summary(),
            "3 considered, 2 claimed, 1 done, 1 failed, 1 deferred, 4 skipped"
        );
        assert_eq!(
            Reporter::default().summary(),
            "0 considered, 0 claimed, 0 done, 0 failed, 0 deferred, 0 skipped"
        );
    }

    /// The busy set the social pass skips: the ledger's un-ended claims and the
    /// dispatcher's own in-memory retries — the agents that hold a claim.
    #[test]
    fn the_busy_set_is_the_ledgers_claims_and_the_dispatchers_retries() {
        let mut claimed = task("t2", "claimed", Some("builder"));
        claimed.agent_cid = Some(300);
        let mut other = task("t4", "claimed", Some("librarian"));
        other.agent_cid = Some(310);
        let tasks = vec![
            task("t1", "open", Some("builder")),
            claimed,
            task("t3", "done", Some("builder")),
            other,
        ];
        let retries = HashMap::from([(
            "t9".to_string(),
            Attempt {
                cid: 320,
                attempts: 1,
            },
        )]);
        assert_eq!(
            busy_agents(&retries, &tasks)
                .into_iter()
                .collect::<Vec<_>>(),
            vec![300, 310, 320]
        );

        // A terminal task's claimant is not busy: its claim is over.
        assert!(busy_agents(&HashMap::new(), &[task("t3", "done", Some("builder"))]).is_empty());
        // And an open task has no claimant to be busy.
        assert!(busy_agents(&HashMap::new(), &[task("t1", "open", Some("builder"))]).is_empty());
    }

    // ------------------------------------------------------------ the talk step

    /// A dispatcher holding the plaza's social surface, so its confirmed
    /// finishes also run the talk step. Generic over the ledger and operator so
    /// a test that restarts the dispatcher shares the fakes through an `Rc`.
    fn dispatcher_with_social<L: Ledger, O: Operator>(
        ledger: L,
        operator: O,
        social: &Arc<FakeSocial>,
    ) -> (Dispatcher<L, O>, Arc<Recording>) {
        let log = Arc::new(Recording::default());
        let dispatcher = Dispatcher::new(
            ledger,
            operator,
            Arc::clone(&log) as Arc<dyn Logger + Send + Sync>,
        )
        .with_social(Arc::clone(social) as Arc<dyn Social + Send + Sync>);
        (dispatcher, log)
    }

    /// A board carrying the award's hiring note for `task`, authored by `cid`.
    fn hired(social: FakeSocial, cid: u64, task: &str) -> FakeSocial {
        social.with_post(
            cid,
            "mayor",
            &format!("hiring for {task}"),
            "go",
            Some(task),
        )
    }

    /// The talk step, end to end: the builder the dispatcher drives to `done`
    /// says the task's own facts to the author of the board's hiring note for
    /// it — once, and never again on a later pass.
    #[test]
    fn a_task_driven_to_done_is_announced_to_its_awarder() {
        let ledger =
            FakeLedger::with_task(task("t1", "open", Some("builder"))).with_agent(300, "builder");
        let operator = FakeOperator::default().with_agent(300, FakeAgent::default());
        let social = Arc::new(hired(FakeSocial::new(), 320, "t1"));
        let (mut dispatcher, log) = dispatcher_with_social(ledger, operator, &social);

        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!((report.claimed, report.done), (1, 1));
        assert_eq!(
            social.said(),
            vec![(300, 320, "done: t1 — build-notes.txt".to_string())],
            "the builder told the awarder the task's own artifact"
        );
        assert!(
            log.text().contains(
                "dispatchd: social: agent cid 300 said to its awarder cid 320 about task t1 \
                 (done): done: t1 — build-notes.txt (the node's conversation row carries the \
                 delivery)"
            ),
            "{}",
            log.text()
        );

        // The task is terminal now: a later pass reads it as such, skips it, and
        // never reaches a transition to announce. One say per task, and the
        // ledger's own terminal row is what bounds a restarted dispatcher.
        dispatcher.pass().expect("the pass runs");
        assert_eq!(social.said().len(), 1, "one say per task");
    }

    /// A task the dispatcher drives to `failed` says the dispatcher's own
    /// failure note — the same line the ledger recorded, not a paraphrase.
    #[test]
    fn a_task_driven_to_failure_is_announced_with_the_failure_note() {
        let ledger =
            FakeLedger::with_task(task("t1", "open", Some("builder"))).with_agent(300, "builder");
        let operator = FakeOperator::default().with_agent(
            300,
            FakeAgent::failing(DriveError::Refused {
                name: "write_file".to_string(),
                output: "the disk is full".to_string(),
            }),
        );
        let social = Arc::new(hired(FakeSocial::new(), 320, "t1"));
        let (mut dispatcher, _log) = dispatcher_with_social(ledger, operator, &social);

        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!((report.done, report.failed), (0, 1));
        assert_eq!(
            social.said(),
            vec![(
                300,
                320,
                "failed: t1 — tool write_file refused: the disk is full".to_string()
            )]
        );
        // The record and the word are the same note.
        let finishes = dispatcher.ledger.finishes.borrow();
        assert_eq!(finishes[0].2, "tool write_file refused: the disk is full");
    }

    /// A task finished `failed` after the retry bound is spent says the note it
    /// was finished with, suffix and all.
    #[test]
    fn a_failure_after_the_retry_bound_says_the_whole_note() {
        let ledger =
            FakeLedger::with_task(task("t1", "open", Some("builder"))).with_agent(300, "builder");
        let operator = FakeOperator::unreachable("cannot connect: refused");
        let social = Arc::new(hired(FakeSocial::new(), 320, "t1"));
        let (mut dispatcher, _log) = dispatcher_with_social(ledger, operator, &social);

        for _ in 0..MAX_ATTEMPTS {
            dispatcher.pass().expect("the pass runs");
        }
        assert_eq!(
            social.said(),
            vec![(
                300,
                320,
                "failed: t1 — tool write_file unavailable: cannot connect: refused \
                 (after 3 attempt(s))"
                    .to_string()
            )]
        );
    }

    /// A `say` the node refuses changes nothing the pass reports: the task was
    /// finished, the word was attempted once, and the refusal is a log line —
    /// never a claim loop that stopped.
    #[test]
    fn a_refused_say_does_not_affect_the_pass() {
        let ledger =
            FakeLedger::with_task(task("t1", "open", Some("builder"))).with_agent(300, "builder");
        let operator = FakeOperator::default().with_agent(300, FakeAgent::default());
        let social = Arc::new(
            hired(FakeSocial::new(), 320, "t1")
                .say_failing("the say answered 409: no live agent has guest CID 320"),
        );
        let (mut dispatcher, log) = dispatcher_with_social(ledger, operator, &social);

        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!((report.claimed, report.done, report.failed), (1, 1, 0));
        assert_eq!(dispatcher.ledger.task("t1").state, "done");
        assert!(social.said().is_empty(), "the node took nothing");
        assert!(
            log.text().contains(
                "dispatchd: social: agent cid 300: the word to awarder cid 320 about task t1 was \
                 refused: the say answered 409: no live agent has guest CID 320; moving on"
            ),
            "{}",
            log.text()
        );
    }

    /// A board with no hiring note for the task: the finish is untouched and
    /// nothing is said — the skip is logged once.
    #[test]
    fn a_finish_with_no_awarder_on_the_board_is_silent() {
        let ledger =
            FakeLedger::with_task(task("t1", "open", Some("builder"))).with_agent(300, "builder");
        let operator = FakeOperator::default().with_agent(300, FakeAgent::default());
        let social = Arc::new(FakeSocial::new());
        let (mut dispatcher, log) = dispatcher_with_social(ledger, operator, &social);

        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!((report.claimed, report.done), (1, 1));
        assert!(social.said().is_empty());
        assert!(
            log.text().contains(
                "dispatchd: social: agent cid 300 finished task t1 (done) but the board names no \
                 awarder for it; nothing said"
            ),
            "{}",
            log.text()
        );
    }

    /// A dispatcher without the social surface says nothing: the claim-and-drive
    /// dispatcher is exactly what it was, and not one social call is made.
    #[test]
    fn a_dispatcher_without_the_social_surface_says_nothing() {
        let ledger =
            FakeLedger::with_task(task("t1", "open", Some("builder"))).with_agent(300, "builder");
        let operator = FakeOperator::default().with_agent(300, FakeAgent::default());
        let social = Arc::new(hired(FakeSocial::new(), 320, "t1"));
        let (mut dispatcher, log) = dispatcher(ledger, operator);

        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!((report.claimed, report.done), (1, 1));
        assert!(social.said().is_empty());
        assert_eq!(social.boards(), 0, "not even the awarder's board is read");
        assert!(!log.text().contains("dispatchd: social:"));
    }

    /// A re-adoption speaks once, not twice. The first dispatcher claims the
    /// task and is stopped mid-drive (its retry state is in memory and dies
    /// with it); the second adopts the stranded claim, drives it to done, and
    /// says the one word — and the passes after that read the task as terminal
    /// and say nothing, because the ledger's own row is the durable dedup.
    #[test]
    fn a_re_adopted_claim_is_announced_once() {
        let ledger = Rc::new(
            FakeLedger::with_task(task("t1", "open", Some("builder"))).with_agent(300, "builder"),
        );
        let operator = Rc::new(
            FakeOperator::unreachable("cannot connect: refused")
                .with_agent(300, FakeAgent::default()),
        );
        let social = Arc::new(hired(FakeSocial::new(), 320, "t1"));

        // The first dispatcher claims t1 and cannot reach the agent: the claim
        // is left behind, nothing is said.
        let (mut first, _log) =
            dispatcher_with_social(Rc::clone(&ledger), Rc::clone(&operator), &social);
        assert_eq!(first.pass().expect("the pass runs").deferred, 1);
        assert_eq!(ledger.task("t1").state, "claimed");
        assert!(social.said().is_empty(), "nothing was finished");

        // The agent comes back; a second dispatcher adopts the stranded claim.
        operator.recover();
        let (mut second, _log) =
            dispatcher_with_social(Rc::clone(&ledger), Rc::clone(&operator), &social);
        let report = second.pass().expect("the pass runs");
        assert_eq!(
            (report.claimed, report.done),
            (0, 1),
            "adopted, not re-claimed"
        );
        assert_eq!(
            social.said(),
            vec![(300, 320, "done: t1 — build-notes.txt".to_string())]
        );
        assert!(ledger.claims.borrow().len() == 1, "the claim was made once");

        // And the passes after it: the task is terminal, so no second word.
        for _ in 0..2 {
            second.pass().expect("the pass runs");
        }
        assert_eq!(social.said().len(), 1);
    }

    /// A finish the ledger refuses is not a transition: nothing is said about a
    /// task that is not (yet) over.
    #[test]
    fn a_refused_finish_says_nothing() {
        let ledger =
            FakeLedger::with_task(task("t1", "open", Some("builder"))).with_agent(300, "builder");
        let operator = FakeOperator::default().with_agent(300, FakeAgent::default());
        let social = Arc::new(hired(FakeSocial::new(), 320, "t1"));
        let (mut dispatcher, log) = dispatcher_with_social(ledger, operator, &social);
        dispatcher.ledger.finish_refusal = Some("a task may not go from done to done".to_string());

        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!((report.done, report.failed), (0, 1));
        assert!(
            social.said().is_empty(),
            "a task the ledger did not finish has nothing to say"
        );
        // The pass's one board read is the social pass's own (this is pass 1);
        // the refused finish asks for none.
        assert_eq!(social.boards(), 1);
        assert!(
            log.text().contains("the ledger refused to finish it done"),
            "{}",
            log.text()
        );
    }
}
