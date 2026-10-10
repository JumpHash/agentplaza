//! The pass: pick, claim, drive, finish.
//!
//! One pass reads the ledger and the fleet register once, then walks the
//! ledger oldest-first. For each task it decides one of four things, and the
//! log says which in the dispatcher's own voice:
//!
//! * **skip** — the task names no role, or the role it names has no CONNECTED
//!   agent, or its claim was refused. A task nobody is addressed to stays
//!   `open`; the dispatcher does not invent an agent for it.
//! * **claim** — the task is `open` and a connected agent of its role exists:
//!   bind it (`POST /tasks/{id}/claim`) and drive it.
//! * **drive** — run the role family's workflow ([`crate::workflow`]) through
//!   the hub's `POST /tell`, one tool call at a time, and finish the task
//!   `done` with its artifacts — or `failed` with the reason.
//! * **retry** — the agent could not be reached: leave the task claimed and
//!   try again on later passes, up to [`MAX_ATTEMPTS`], then finish it
//!   `failed` with the evidence.
//!
//! What is retried and what is not is a deliberate split. An agent that
//! **answered** — a tool refusal, a listing the workflow cannot read, a
//! read-back that did not match — has said something final, and the next pass
//! would hear the same thing: that task is finished `failed` at once, with the
//! agent's own words. An agent that **could not be reached** (not connected,
//! no answer in time, the call could not be made at all) is a condition that
//! may pass, so it is retried — bounded — before being given up on.
//!
//! The bound is stated rather than implied: **a pass drives every actionable
//! task, one at a time** — never two tool calls in flight, anywhere — and the
//! work it can find is bounded by the ledger's own cap (1024 tasks) and the
//! hub's session caps. `--once` is exactly one pass.

use std::collections::HashMap;
use std::sync::Arc;

use crate::views::{Agent, Outcome, Task, Tell};
use crate::workflow::{self, DriveError, Driver};

/// How many passes a task whose agent could not be reached is retried for
/// before it is finished `failed` with the evidence. Attempts are counted per
/// pass, so a dispatcher running every 5 s gives up after about 10 s of the
/// agent being unreachable — and a dispatcher that is stopped and started
/// again remembers nothing, which is honest: the task stays claimed, and its
/// record says so.
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

/// The dispatcher: the ledger and the hub, and the one bit of state that only
/// concerns retries.
pub struct Dispatcher<L: Ledger, O: Operator> {
    ledger: L,
    operator: O,
    log: Arc<dyn Logger + Send + Sync>,
    /// Retries, by task id. Nothing else is remembered: the ledger holds the
    /// tasks and their states, and this only bounds how many passes a task
    /// whose agent is unreachable is tried for.
    retries: HashMap<String, Attempt>,
    passes: u64,
}

impl<L: Ledger, O: Operator> Dispatcher<L, O> {
    /// A dispatcher over `ledger` and `operator`, logging to `log`.
    pub fn new(ledger: L, operator: O, log: Arc<dyn Logger + Send + Sync>) -> Self {
        Self {
            ledger,
            operator,
            log,
            retries: HashMap::new(),
            passes: 0,
        }
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
                    Some(cid) => self.drive(task, role, cid, &mut report),
                    None => {
                        // Claimed by nobody this dispatcher knows: another
                        // dispatcher's, or one claimed before a restart.
                        // Finishing someone else's claim is not ours to do.
                        report.skipped += 1;
                        log.line(&format!(
                            "dispatchd: task {} (role {role}) is claimed by another; skipped",
                            task.id
                        ));
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

    /// Starts an open task: find a connected agent of its role, claim the task
    /// for it, and drive it.
    fn start(&mut self, task: &Task, role: &str, agents: &[Agent], report: &mut Reporter) {
        let log = Arc::clone(&self.log);
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
                self.drive(task, role, cid, report);
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

    /// Drives one claimed task through its role family's workflow and finishes
    /// it, whichever way it goes.
    fn drive(&mut self, task: &Task, role: &str, cid: u64, report: &mut Reporter) {
        let log = Arc::clone(&self.log);
        let attempts = self.note_attempt(&task.id, cid);
        let workflow = workflow::workflow_for(role);
        // The driver borrows the hub for the length of the run; the ledger and
        // the retry map are separate fields, so finishing the task afterwards
        // needs no lock and no re-borrow.
        let outcome = {
            let mut driver = AgentDriver::new(&self.operator, cid, TELL_TIMEOUT_MS);
            workflow(task, &mut driver).map(|artifacts| (artifacts, driver.calls()))
        };
        match outcome {
            Ok((artifacts, calls)) => {
                let note = workflow::done_note(&artifacts, calls);
                match self.ledger.finish(&task.id, Outcome::Done, &note) {
                    Ok(()) => {
                        report.done += 1;
                        log.line(&format!(
                            "dispatchd: task {} (role {role}) -> agent cid {cid} finished done: {note}",
                            task.id
                        ));
                    }
                    Err(reason) => {
                        report.failed += 1;
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
                    report.deferred += 1;
                    log.line(&format!(
                        "dispatchd: task {} (role {role}) -> agent cid {cid}: {note} (attempt {attempts} of {MAX_ATTEMPTS}); leaving it claimed",
                        task.id
                    ));
                } else {
                    // Either the agent answered (and this is final), or the
                    // attempts are spent: the task ends with the evidence.
                    let note = if err.is_retryable() {
                        format!("{note} (after {attempts} attempt(s))")
                    } else {
                        note
                    };
                    match self.ledger.finish(&task.id, Outcome::Failed, &note) {
                        Ok(()) => {
                            report.failed += 1;
                            log.line(&format!(
                                "dispatchd: task {} (role {role}) -> agent cid {cid} finished failed: {note}",
                                task.id
                            ));
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

    use crate::testing::{FakeAgent, FakeLedger, FakeOperator, Recording, agent, task};
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

    /// A pass drives **every** actionable task, in turn: two roles, two tasks,
    /// both done — which is the shape `--once` has to satisfy.
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
                FakeAgent::default().with_file("handbook.md", "the handbook"),
            )
            .with_agent(320, FakeAgent::default());
        let (mut dispatcher, _log) = dispatcher(ledger, operator);

        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!(report.considered, 3);
        assert_eq!((report.claimed, report.done), (3, 3));

        // Each role ran its own family's script, on its own agent.
        assert_eq!(
            dispatcher.operator.sequence(),
            vec![
                (300, "write_file".to_string()),
                (300, "read_file".to_string()),
                (310, "list".to_string()),
                (310, "write_file".to_string()),
                (310, "read_file".to_string()),
                (320, "write_file".to_string()),
                (320, "read_file".to_string()),
            ]
        );
        assert_eq!(
            dispatcher
                .operator
                .agent_file(310, "index.txt")
                .expect("the librarian's index"),
            "task t2: do t2\nworkspace: 1 entry\nhandbook.md\tfile\t12"
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
                ("t2", Outcome::Done, "artifacts: index.txt (3 tool call(s))"),
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

    /// A claimed task this dispatcher does not own is left alone: another
    /// dispatcher's claim (or one made before a restart) is not ours to finish.
    #[test]
    fn a_claim_the_dispatcher_does_not_own_is_left_alone() {
        let mut claimed = task("t1", "claimed", Some("builder"));
        claimed.agent_cid = Some(300);
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
}
