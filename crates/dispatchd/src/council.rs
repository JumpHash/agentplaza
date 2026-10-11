//! The council pass: the four seats actually vote, and the plaza proposes.
//!
//! `docs/agents/council.md` is the pinned contract and `crates/noded/src/
//! council.rs` is its implementation: four seats (Finance, Legal (Agents),
//! Incident, Mayor Office) propose motions and vote on them, and a motion's
//! state is the count of the seats' votes against its owner-gate. The state
//! and the view are the node's; **driving the seats is this dispatcher's** — the
//! way the social pass is — and this module is that driver. It invents no
//! prose and no policy: every word of a vote's reason and of a proposed motion
//! is the motion's or the task's own, and the rule that decides the word is a
//! function of the plaza's own facts.
//!
//! # What one council pass does
//!
//! On the daemon's existing loop but slower ([`COUNCIL_EVERY`]: every third
//! pass, against the claim walk's every pass), over the same `tasks`/`agents`
//! snapshot the claim walk is about to work from and **before any claim is
//! taken**, one pass:
//!
//! 1. **read** the motions (`GET /api/v0/motions`, bounded by the endpoint's
//!    own 256-motion window). A motions board that will not read is one log
//!    line and the pass is over: the claim loop is never blocked by the
//!    council.
//! 2. **propose** — when the ledger carries an `open`, roled task whose id **no
//!    motion already names** ([`propose_candidate`]), the pass proposes **one**
//!    `priority` motion, by the first connected seat ([`CouncilPass`]'s
//!    round-robin). The text is a template over the task's own words
//!    ([`proposal_text`], `priority: <task> — <goal>`); `needs_owner` is left
//!    off the body, so the daemon derives the gate itself.
//! 3. **vote** — the seats cast **one** vote per pass, over the `open` motions
//!    oldest-first: the first open motion with a connected seat that has not
//!    yet voted gets that seat's vote ([`CouncilPass::run`]'s rule), and the
//!    pass is over. The word is [`vote_of`]: **yes** when the motion's own text
//!    names, as a whole token, the id of a task that is `open` in this pass's
//!    ledger; **no** otherwise — and the reason is the template
//!    [`vote_reason`] cites (`the record: <kind> '<first 80 bytes of text>'`,
//!    plus `names open task <task>` when it votes yes).
//!
//! # The seats, mapped to the fleet's roles (kept in step by hand)
//!
//! [`SEATS`] is `crates/noded/src/council.rs`'s table, repeated here because
//! `dispatchd` cannot depend on `noded` (neither is a library) — the same
//! hand-kept sibling `crate::roles` is to `vsockd`'s grants, and
//! `crates/noded/src/http.rs` is to `vsockd`'s status surface. A seat is held
//! by a **connected agent** whose role is exactly the seat's role; judges and
//! prosecutors never sit (S-008a), so `legal-gov-judge` and
//! `legal-gov-prosecutor` are not in the table. An agent of a seat's role is
//! picked by lowest CID ([`seat_agent`]), the same determinism
//! [`crate::dispatch::pick_agent`] gives the claim walk.
//!
//! # The rules, stated (not implied)
//!
//! * **The cadence.** [`COUNCIL_EVERY`] is how many dispatch passes go by
//!   between council passes: one council pass every `COUNCIL_EVERY`-th pass
//!   (the run's third, sixth, ninth…), on the daemon's own loop but slower. At
//!   the default `--interval-secs 5` that is roughly one every 15 s.
//! * **The round-robin.** The four seats are walked in [`SEATS`] order from
//!   where the last council action left off ([`CouncilPass::seat_cursor`]): an
//!   action (a proposal or a vote) advances the cursor past the seat it used,
//!   so the seats take turns and no seat's agent is asked twice before the
//!   others have had a turn. A seat whose agent is not connected is skipped,
//!   without disturbing the order.
//! * **One seat per pass.** A pass casts at most one vote, and a vote that is
//!   refused ends the pass's voting — the driver never hammers a refusal.
//! * **The vote rule (over the motion's own facts).** A seat votes **yes** when
//!   the motion names an open task; **no** otherwise. The kind is *not*
//!   consulted for the word — the prompt's example narrows the rule to
//!   `priority`, but a gated motion that names open work is the council
//!   signalling the owner to ratify it, and the driver reads the criterion as
//!   the motion naming open work — so a `priority` motion naming open work
//!   carries, and a `law_proposal` naming open work carries to
//!   `awaiting_owner`. (The kind is cited in the reason, and the daemon derives
//!   the gate from it; neither is the driver's to decide.)
//! * **Both dedups.**
//!   * *Voting:* a seat that has already voted on a motion is **never**
//!     re-voted. The dedup is the motion's own `votes[]` — read from the same
//!     bounded motions view the pass just read — which names the seat, so the
//!     driver cannot flip a vote it already cast (the backend replaces, 200,
//!     and the newest reason would stand; the driver simply never asks).
//!   * *Proposing:* a task whose id a motion's text already names is **not**
//!     proposed again. The backend has no task link on a motion (only `text`),
//!     so the dedup is a **token match** of the task's id against every
//!     motion's `text` ([`names`]) — a motion whose text names the task is the
//!     record that it was proposed.
//! * **Failure isolation.** Every failure — a motions board that will not
//!   read, a proposal or a vote the daemon refuses (a 403 for a role that holds
//!   no seat, a 409 for a decided motion or a disconnected proposer, a 500) —
//!   is logged once and swallowed. The pass returns, and the claim walk runs
//!   exactly as it would have: the council never blocks the ledger.
//! * **The bound.** One council pass makes at most **three** loopback
//!   requests: one `GET /api/v0/motions`, at most one `POST /api/v0/motions`,
//!   and at most one `POST /api/v0/motions/{id}/vote` — each under the same
//!   [`crate::http::TIMEOUT`] every other dispatchd call rides, and none
//!   retried. A council pass can therefore hold the claim walk up by at most
//!   three bounded timeouts, and only on every [`COUNCIL_EVERY`]-th pass.

use std::sync::Arc;

use crate::dispatch::{Council, Logger};
use crate::views::{Agent, CastVote, Motion, Motions, NewMotion, Task};

/// How many dispatch passes go by between council passes. One council pass
/// every [`COUNCIL_EVERY`]-th pass — the daemon's own loop, but slower. At the
/// default `--interval-secs 5` that is about one council pass every 15 s; the
/// run's first pass is one (the third pass of the run is the first council
/// pass), so a daemon's council speaks early and then on its own slower beat.
pub const COUNCIL_EVERY: u64 = 3;

/// The council's four seats, in the order votes are cast and reported: their
/// roles in `docs/agents/initial-roles.json`.
///
/// The doctrine is `docs/codex-draft/statutes/S-008-council-seats.cedar`
/// (`@law("S-008")`) read with `docs/codex-draft/playbooks/council-procedure.md`
/// step 1: the heads of Finance, Legal (Agents) (the legislative counsel),
/// Incident response and Mayor Office. Legal (Governance) — the judges and the
/// prosecutors — never sits (S-008a), so it is not here. This is
/// `crates/noded/src/council.rs`'s `SEATS`, repeated because the two crates do
/// not depend on each other.
pub const SEATS: [&str; 4] = [
    "finance",
    "legal-agents-counsel",
    "incident",
    "mayor-office",
];

/// The kind the driver proposes: a fleet-wide priority, the one kind whose gate
/// is the council's majority (never owner-gated), so the seats' own vote
/// decides it.
pub const PRIORITY: &str = "priority";

/// The most a proposed motion's `text` may occupy, in bytes — the contract
/// schema's `maxLength` (`crates/noded/src/council.rs`'s `MAX_MOTION_TEXT`). A
/// composed text is cut to it on a character boundary rather than sent to be
/// refused.
pub const MAX_MOTION_TEXT: usize = 4000;

/// How much of a motion's own text a vote's reason cites, in bytes — the
/// "first 80 chars of text" the reason template names.
pub const REASON_SNIPPET: usize = 80;

/// The most a vote's `reason` may occupy, in bytes — the contract's cap
/// (`crates/noded/src/council.rs`'s `MAX_REASON`), which a composed reason is
/// cut to before it is sent.
pub const MAX_REASON: usize = 2000;

/// The word a seat casts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vote {
    /// In favour: the motion names open work.
    Yes,
    /// Against: the motion names no open work.
    No,
}

impl Vote {
    /// The word as the wire and the views spell it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Yes => "yes",
            Self::No => "no",
        }
    }
}

/// What one council pass did, so a run can be summarised and a test can assert
/// on it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CouncilReport {
    /// Whether the pass read the motions board.
    pub read: bool,
    /// The id of the motion the pass proposed, when it proposed one.
    pub proposed: Option<String>,
    /// The id of the motion the pass voted on, when it cast a vote.
    pub voted: Option<String>,
    /// The seat whose vote was cast, when one was.
    pub seat: Option<String>,
    /// The word the seat cast, when one was.
    pub word: Option<String>,
}

/// The council pass, held by the dispatcher for the life of a run: the node's
/// council surface, the dispatcher's voice, and the round-robin position.
pub struct CouncilPass {
    node: Arc<dyn Council + Send + Sync>,
    log: Arc<dyn Logger + Send + Sync>,
    /// Where the round-robin over the four seats stands: the [`SEATS`] index
    /// the *next* council action starts at.
    seat_cursor: usize,
}

impl CouncilPass {
    /// A council pass over `node`, logging to `log`, starting its round-robin
    /// at the first seat.
    pub fn new(node: Arc<dyn Council + Send + Sync>, log: Arc<dyn Logger + Send + Sync>) -> Self {
        Self {
            node,
            log,
            seat_cursor: 0,
        }
    }

    /// Where the seat round-robin stands: the [`SEATS`] index the next action
    /// starts at. Exposed so a test can prove the walk is a walk.
    pub fn seat_cursor(&self) -> usize {
        self.seat_cursor
    }

    /// Runs one council pass over the snapshot `tasks` and `agents` — the same
    /// pair the claim walk reads.
    ///
    /// Every failure is logged and swallowed; the report says what happened.
    pub fn run(&mut self, tasks: &[Task], agents: &[Agent]) -> CouncilReport {
        let mut report = CouncilReport::default();
        let motions = match self.node.motions() {
            Ok(motions) => motions,
            Err(reason) => {
                self.log.line(&format!(
                    "dispatchd: council: the motions could not be read: {reason}; skipping"
                ));
                return report;
            }
        };
        report.read = true;
        self.propose(&motions, tasks, agents, &mut report);
        self.vote(&motions, tasks, agents, &mut report);
        report
    }

    /// The propose step: one `priority` motion for the first open, roled task
    /// no motion already names, by the first connected seat in the round-robin.
    ///
    /// Nothing to propose is silent — an idle council is not news each pass. A
    /// refusal is one log line, and the cursor does *not* advance: the seat did
    /// not act, so the next connected seat gets its turn on the next pass.
    fn propose(
        &mut self,
        motions: &Motions,
        tasks: &[Task],
        agents: &[Agent],
        report: &mut CouncilReport,
    ) {
        let Some(task) = propose_candidate(motions, tasks) else {
            return;
        };
        let Some((seat, cid)) = self.pick_seat(agents, &[]) else {
            self.log.line(&format!(
                "dispatchd: council: task {} is open and unnamed but no seat is connected; nothing proposed",
                task.id
            ));
            return;
        };
        let motion = NewMotion {
            proposer_cid: cid,
            kind: PRIORITY.to_string(),
            text: proposal_text(task),
        };
        match self.node.propose(&motion) {
            Ok(record) => {
                report.proposed = Some(record.id.clone());
                self.seat_cursor = (seat + 1) % SEATS.len();
                self.log.line(&format!(
                    "dispatchd: council: {} (cid {cid}) proposed {} for open task {}: {}",
                    SEATS[seat], record.id, task.id, record.text
                ));
            }
            Err(reason) => self.log.line(&format!(
                "dispatchd: council: {} (cid {cid}): the motion for task {} was refused: {reason}; moving on",
                SEATS[seat], task.id
            )),
        }
    }

    /// The vote step: one seat's vote, on the first open motion that has a
    /// connected seat which has not yet voted.
    ///
    /// At most one vote is cast, and a refusal ends the voting: the driver does
    /// not hammer a refusal, and the pass is bounded to one vote either way.
    fn vote(
        &mut self,
        motions: &Motions,
        tasks: &[Task],
        agents: &[Agent],
        report: &mut CouncilReport,
    ) {
        let target = motions.oldest_first().find_map(|motion| {
            if !motion.is_open() {
                return None;
            }
            let voted: Vec<&str> = motion.votes.iter().map(|vote| vote.seat.as_str()).collect();
            let (seat, cid) = self.pick_seat(agents, &voted)?;
            Some((motion, seat, cid))
        });
        let (motion, seat, cid) = match target {
            Some(target) => target,
            None => {
                // Only worth a line when there *is* open work nobody can act
                // on: a council with nothing open says nothing.
                if let Some(open) = motions.oldest_first().find(|motion| motion.is_open()) {
                    self.log.line(&format!(
                        "dispatchd: council: motion {} is open but no connected, unvoted seat can act; nothing cast",
                        open.id
                    ));
                }
                return;
            }
        };
        let (word, named) = vote_of(motion, tasks);
        let cast = CastVote {
            agent_cid: cid,
            vote: word.as_str().to_string(),
            reason: vote_reason(motion, named),
        };
        match self.node.vote(&motion.id, &cast) {
            Ok(record) => {
                report.voted = Some(record.id.clone());
                report.seat = Some(SEATS[seat].to_string());
                report.word = Some(word.as_str().to_string());
                self.seat_cursor = (seat + 1) % SEATS.len();
                self.log.line(&format!(
                    "dispatchd: council: {} (cid {cid}) voted {} on motion {} ({} yes, {} no, {} abstain, {} voted of {}): {}",
                    SEATS[seat],
                    word.as_str(),
                    record.id,
                    record.tally.yes,
                    record.tally.no,
                    record.tally.abstain,
                    record.tally.voted,
                    record.tally.seats,
                    cast.reason,
                ));
            }
            Err(reason) => self.log.line(&format!(
                "dispatchd: council: {} (cid {cid}): the vote on motion {} was refused: {reason}; moving on",
                SEATS[seat], motion.id
            )),
        }
    }

    /// The next connected seat from the round-robin cursor, skipping the seats
    /// in `voted` and the seats whose agent is not connected.
    ///
    /// Answers the seat's [`SEATS`] index and the CID of the agent that holds
    /// it (the lowest CID of that role, [`seat_agent`]).
    fn pick_seat(&self, agents: &[Agent], voted: &[&str]) -> Option<(usize, u64)> {
        let seats = SEATS.len();
        (0..seats)
            .map(|offset| (self.seat_cursor + offset) % seats)
            .find_map(|index| {
                let role = SEATS[index];
                if voted.contains(&role) {
                    return None;
                }
                seat_agent(agents, role).map(|agent| (index, agent.cid))
            })
    }
}

/// The agent that holds `role`, when the register lists one: the lowest CID,
/// exactly as [`crate::dispatch::pick_agent`] picks for a claim, so the seat's
/// actor is deterministic.
pub fn seat_agent<'a>(agents: &'a [Agent], role: &str) -> Option<&'a Agent> {
    agents
        .iter()
        .filter(|agent| agent.role == role)
        .min_by_key(|agent| agent.cid)
}

/// Whether `text` names `id` as a whole token: the character before and the
/// character after an occurrence are not ASCII alphanumerics (or a string end).
///
/// This is the honest reading of "the text names the task". A bare `contains`
/// would read `t1` inside `t10`, `t100` or `note1` — a motion about one task
/// would silently count as naming another. The node's ids are ASCII (`t<seq>`,
/// `m<seq>`), so the boundary test is over bytes and the walk stays on
/// character boundaries (`id` is ASCII, so the byte after an occurrence is one).
pub fn names(text: &str, id: &str) -> bool {
    if id.is_empty() {
        return false;
    }
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(offset) = text[from..].find(id) {
        let at = from + offset;
        let end = at + id.len();
        let before = at == 0 || !is_id_byte(bytes[at - 1]);
        let after = end == bytes.len() || !is_id_byte(bytes[end]);
        if before && after {
            return true;
        }
        from = at + 1;
    }
    false
}

/// Whether a byte may be part of an id: an ASCII alphanumeric. The node's ids
/// are alphanumeric, so a separator (`:`, space, `—`, `-`) is a boundary.
fn is_id_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
}

/// The open task a motion names, when it names one: the first `open` task, in
/// the ledger's oldest-first order, whose id the motion's text names as a whole
/// token ([`names`]).
pub fn named_open_task<'a>(motion: &Motion, tasks: &'a [Task]) -> Option<&'a Task> {
    tasks
        .iter()
        .find(|task| task.is_open() && names(&motion.text, &task.id))
}

/// The word a seat casts on `motion`, and the open task the motion names (when
/// the word is yes).
///
/// The rule, over the motion's own facts: **yes** when the motion's text names,
/// as a whole token, the id of a task that is `open` in this pass's ledger; **no**
/// otherwise. The kind is not consulted (see the module note) — the criterion
/// is the motion naming open work.
pub fn vote_of<'a>(motion: &Motion, tasks: &'a [Task]) -> (Vote, Option<&'a Task>) {
    match named_open_task(motion, tasks) {
        Some(task) => (Vote::Yes, Some(task)),
        None => (Vote::No, None),
    }
}

/// The reason a seat's vote carries: a template over the motion's own facts,
/// never invented prose.
///
/// ```text
/// yes: the record: <kind> '<first 80 bytes of text>' names open task <task>
/// no:  the record: <kind> '<first 80 bytes of text>'
/// ```
///
/// The kind, the text snippet and (on a yes) the task's id are the motion's and
/// the ledger's own; nothing else is claimed. The result is cut to the
/// contract's `reason` cap, so the daemon never refuses it for length.
pub fn vote_reason(motion: &Motion, named: Option<&Task>) -> String {
    let snippet = clip(&motion.text, REASON_SNIPPET);
    let reason = match named {
        Some(task) => format!(
            "the record: {} '{snippet}' names open task {}",
            motion.kind, task.id
        ),
        None => format!("the record: {} '{snippet}'", motion.kind),
    };
    clip(&reason, MAX_REASON)
}

/// The text the driver proposes for `task`: a template over the task's own
/// words.
///
/// ```text
/// priority: <task> — <goal>
/// ```
///
/// The goal is the task's own; a task with no goal is stated as absent rather
/// than invented. The result is cut to the contract's `text` cap.
pub fn proposal_text(task: &Task) -> String {
    let goal = if task.goal.trim().is_empty() {
        "(no goal stated)".to_string()
    } else {
        task.goal.clone()
    };
    clip(
        &format!("{PRIORITY}: {} — {goal}", task.id),
        MAX_MOTION_TEXT,
    )
}

/// The task the driver proposes for: the first `open`, roled task, oldest
/// first, whose id **no motion's text names** — the propose step's dedup.
///
/// The backend has no task link on a motion, so "already proposed" can only be
/// read from the motions' own `text`: a motion whose text names the task's id
/// as a whole token ([`names`]) is the record that the plaza has spoken about
/// it. A task named by any motion — proposed here, or by a seat, whatever the
/// motion's kind or state — is not proposed again.
pub fn propose_candidate<'a>(motions: &Motions, tasks: &'a [Task]) -> Option<&'a Task> {
    tasks.iter().find(|task| {
        task.is_open()
            && task.roled().is_some()
            && !motions
                .motions
                .iter()
                .any(|motion| names(&motion.text, &task.id))
    })
}

/// Cuts `text` to at most `max` bytes on a character boundary, appending `…`
/// when it cut — so a composed reason or motion text can never be refused by
/// the daemon's own byte cap.
fn clip(text: &str, max: usize) -> String {
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
    use crate::testing::{FakeCouncil, FakeLedger, FakeOperator, Recording, agent, task};

    /// A council pass over `council`, with its log kept for assertions. The
    /// fake is shared through an `Arc` so the test can read what the pass did
    /// after it ran.
    fn council_pass(council: &Arc<FakeCouncil>) -> (CouncilPass, Arc<Recording>) {
        let log = Arc::new(Recording::default());
        let pass = CouncilPass::new(
            Arc::clone(council) as Arc<dyn Council + Send + Sync>,
            Arc::clone(&log) as Arc<dyn Logger + Send + Sync>,
        );
        (pass, log)
    }

    /// A motion as the daemon would report one, built by hand for the pure
    /// rule tests.
    fn motion(kind: &str, text: &str, state: &str, votes: &[(&str, &str)]) -> Motion {
        let votes: Vec<serde_json::Value> = votes
            .iter()
            .map(|(seat, word)| serde_json::json!({ "seat": seat, "vote": word, "reason": "r" }))
            .collect();
        serde_json::from_value(serde_json::json!({
            "id": "m1", "kind": kind, "text": text, "state": state, "votes": votes,
        }))
        .expect("a motion")
    }

    // ------------------------------------------------------- the naming rule

    /// The token rule: a task's id inside a longer id or word is not a naming.
    #[test]
    fn a_name_is_a_whole_token() {
        assert!(names("priority: t1 — do it", "t1"));
        assert!(names("t1", "t1"));
        assert!(names("take up t1.", "t1"));
        // Not a naming: the id is part of a longer token.
        assert!(!names("t10", "t1"));
        assert!(!names("t100 and t11", "t1"));
        assert!(!names("note1", "t1"));
        assert!(!names("at1b", "t1"));
        assert!(!names("", "t1"));
        assert!(!names("t1", ""));
        // Two occurrences, the first embedded: the second still counts.
        assert!(names("t10 t1", "t1"));
        // Unicode around the token is still a boundary (bytes, not chars).
        assert!(names("—t1—", "t1"));
    }

    // ----------------------------------------------------------- the vote rule

    /// The vote rule matrix: the word tracks whether the motion names an open
    /// task, and does **not** depend on the kind. Every kind, both ways.
    #[test]
    fn the_vote_rule_is_the_motion_naming_open_work() {
        let open = task("t1", "open", Some("builder"));
        let done = task("t1", "done", Some("builder"));
        for kind in [
            "priority",
            "budget_move",
            "town_proposal",
            "mayor_replacement",
            "law_proposal",
            "other",
        ] {
            let names_open = motion(kind, "priority: t1 — do it", "open", &[]);
            let (word, named) = vote_of(&names_open, std::slice::from_ref(&open));
            assert_eq!(word, Vote::Yes, "{kind} naming open work is a yes");
            assert_eq!(named.map(|task| task.id.as_str()), Some("t1"));

            // The same text, but the named task is not open: a no.
            let (word, named) = vote_of(&names_open, std::slice::from_ref(&done));
            assert_eq!(word, Vote::No, "{kind} naming no open task is a no");
            assert!(named.is_none());

            // A text that names no task at all: a no.
            let no_task = motion(kind, "priority: raise the fleet's morale", "open", &[]);
            let (word, named) = vote_of(&no_task, std::slice::from_ref(&open));
            assert_eq!(word, Vote::No, "{kind} naming no task is a no");
            assert!(named.is_none());
        }
    }

    /// The reason is the motion's own facts, templated — the kind, the first
    /// eighty bytes of the text, and, on a yes, the task it names.
    #[test]
    fn the_reason_is_the_motions_own_facts() {
        let open = task("t7", "open", Some("builder"));
        let yes = motion("priority", "priority: t7 — do t7", "open", &[]);
        let (_, named) = vote_of(&yes, std::slice::from_ref(&open));
        assert_eq!(
            vote_reason(&yes, named),
            "the record: priority 'priority: t7 — do t7' names open task t7"
        );

        let no = motion("budget_move", "move it", "open", &[]);
        let (_, named) = vote_of(&no, std::slice::from_ref(&open));
        assert_eq!(vote_reason(&no, named), "the record: budget_move 'move it'");

        // A text past eighty bytes is cut on a character boundary, and the
        // reason is cut to the contract's own cap.
        let long = motion("other", &"é".repeat(REASON_SNIPPET), "open", &[]);
        let reason = vote_reason(&long, None);
        assert!(reason.contains('…'), "{reason}");
        assert!(reason.len() <= MAX_REASON, "{} bytes", reason.len());
        assert!(reason.is_char_boundary(reason.len()));
    }

    /// The proposal template names the task and its goal, and states an absent
    /// goal rather than inventing one.
    #[test]
    fn the_proposal_template_is_the_tasks_own_words() {
        assert_eq!(
            proposal_text(&task("t3", "open", Some("builder"))),
            "priority: t3 — do t3"
        );
        let mut blank = task("t4", "open", Some("builder"));
        blank.goal = "   ".to_string();
        assert_eq!(proposal_text(&blank), "priority: t4 — (no goal stated)");
    }

    /// The propose candidate: the first open, roled task no motion names — and
    /// the dedup is a token match against every motion's text, whatever its
    /// state or kind.
    #[test]
    fn the_propose_candidate_skips_named_and_unroled_tasks() {
        let tasks = vec![
            task("t1", "open", Some("builder")),
            task("t2", "open", None),
            task("t3", "open", Some("builder")),
            task("t4", "done", Some("builder")),
        ];
        let empty: Motions = serde_json::from_str(r#"{"total":0}"#).expect("a motions view");
        assert_eq!(
            propose_candidate(&empty, &tasks).map(|task| task.id.as_str()),
            Some("t1"),
            "the first open, roled task"
        );

        // t1 named by a motion: the next open, roled task is t3.
        let motions: Motions = serde_json::from_str(
            r#"{"motions":[{"id":"m1","kind":"priority","text":"priority: t1 — do t1","state":"carried"}]}"#,
        )
        .expect("a motions view");
        assert_eq!(
            propose_candidate(&motions, &tasks).map(|task| task.id.as_str()),
            Some("t3")
        );

        // A look-alike id does not count as naming.
        let motions: Motions = serde_json::from_str(
            r#"{"motions":[{"id":"m1","kind":"priority","text":"priority: t10 — x","state":"open"}]}"#,
        )
        .expect("a motions view");
        assert_eq!(
            propose_candidate(&motions, &tasks).map(|task| task.id.as_str()),
            Some("t1")
        );
    }

    // ----------------------------------------------------- the pass mechanics

    /// The seats vote across passes, one seat per pass, in round-robin order,
    /// until the motion is carried — and the driver never votes a decided one.
    #[test]
    fn the_seats_vote_across_passes_one_seat_at_a_time() {
        let tasks = vec![task("t1", "open", Some("builder"))];
        let agents = vec![
            agent(105, "finance"),
            agent(106, "incident"),
            agent(107, "legal-agents-counsel"),
            agent(114, "mayor-office"),
        ];
        // A priority motion naming the open task: every seat votes yes, so it
        // carries once all four have.
        let council = Arc::new(
            FakeCouncil::new()
                .with_seat(105, "finance")
                .with_seat(107, "legal-agents-counsel")
                .with_seat(106, "incident")
                .with_seat(114, "mayor-office")
                .with_motion("m1", "priority", "priority: t1 — do t1", false, &[]),
        );
        let (mut pass, log) = council_pass(&council);

        // The seat order is the contract's: finance, legal-agents-counsel,
        // incident, mayor-office (the register's order does not matter).
        let expected = [
            "finance",
            "legal-agents-counsel",
            "incident",
            "mayor-office",
        ];
        for (round, seat) in expected.iter().enumerate() {
            let report = pass.run(&tasks, &agents);
            assert_eq!(report.voted.as_deref(), Some("m1"), "round {round}");
            assert_eq!(report.seat.as_deref(), Some(*seat), "round {round}");
            assert_eq!(report.word.as_deref(), Some("yes"));
            assert_eq!(pass.seat_cursor(), (round + 1) % SEATS.len());
        }
        let record = council.motion("m1").expect("the motion");
        assert_eq!(record.state, "carried");
        assert_eq!((record.tally.yes, record.tally.voted), (4, 4));
        assert_eq!(
            council
                .cast()
                .iter()
                .map(|(_, cast)| cast.vote.as_str())
                .collect::<Vec<_>>(),
            vec!["yes", "yes", "yes", "yes"]
        );
        assert!(
            log.text()
                .contains("voted yes on motion m1 (4 yes, 0 no, 0 abstain, 4 voted of 4)"),
            "{}",
            log.text()
        );

        // A decided motion takes no further vote: the next pass casts nothing.
        let report = pass.run(&tasks, &agents);
        assert_eq!(report.voted, None);
        assert_eq!(council.cast().len(), 4, "no fifth vote");
    }

    /// A gated kind naming open work carries to `awaiting_owner`: the same
    /// rule, and the daemon's derived gate turns the carried majority into a
    /// motion that waits for the owner.
    #[test]
    fn a_gated_kind_naming_open_work_carries_to_the_owner() {
        let tasks = vec![task("t1", "open", Some("builder"))];
        let agents = vec![
            agent(105, "finance"),
            agent(106, "incident"),
            agent(107, "legal-agents-counsel"),
            agent(114, "mayor-office"),
        ];
        let council = Arc::new(
            FakeCouncil::new()
                .with_seat(105, "finance")
                .with_seat(107, "legal-agents-counsel")
                .with_seat(106, "incident")
                .with_seat(114, "mayor-office")
                .with_motion("m1", "law_proposal", "a law about t1", true, &[]),
        );
        let (mut pass, _log) = council_pass(&council);
        for _ in 0..SEATS.len() {
            pass.run(&tasks, &agents);
        }
        let record = council.motion("m1").expect("the motion");
        assert_eq!(record.state, "awaiting_owner");
        assert_eq!((record.tally.yes, record.tally.voted), (4, 4));
    }

    /// A seat that has already voted is never re-voted: the motion's own
    /// `votes[]` is the dedup, and it is read from the same bounded view.
    #[test]
    fn a_seat_that_has_voted_is_never_re_voted() {
        let tasks = vec![task("t1", "open", Some("builder"))];
        let agents = vec![agent(105, "finance"), agent(106, "incident")];
        // Finance already voted no; the driver must not flip it. Only incident
        // is left to vote, and it votes yes.
        let council = Arc::new(
            FakeCouncil::new()
                .with_seat(105, "finance")
                .with_seat(106, "incident")
                .with_motion(
                    "m1",
                    "priority",
                    "priority: t1 — do t1",
                    false,
                    &[(
                        "finance",
                        105,
                        "no",
                        "the record: priority 'priority: t1 — do t1' names open task t1",
                    )],
                ),
        );
        let (mut pass, _log) = council_pass(&council);

        // The cursor starts at finance, which already voted: it is skipped for
        // incident. Only one vote is cast, and it is not finance's.
        let report = pass.run(&tasks, &agents);
        assert_eq!(report.seat.as_deref(), Some("incident"));
        assert_eq!(
            council
                .cast()
                .iter()
                .map(|(_, cast)| (cast.agent_cid, cast.vote.as_str()))
                .collect::<Vec<_>>(),
            vec![(106, "yes")],
            "finance's word stands"
        );
        assert!(council.motion("m1").unwrap().voted("finance"));
        // The next pass: no connected seat can vote (finance voted, incident
        // voted), so nothing is cast — no flip.
        let report = pass.run(&tasks, &agents);
        assert_eq!(report.voted, None);
        assert_eq!(council.cast().len(), 1);
    }

    /// The propose step: one priority motion for an unnamed open task, by the
    /// first connected seat, and the dedup means it is proposed exactly once.
    #[test]
    fn the_propose_step_proposes_once_and_dedups_its_own_text() {
        let tasks = vec![task("t1", "open", Some("builder"))];
        let agents = vec![agent(105, "finance"), agent(114, "mayor-office")];
        let council = Arc::new(
            FakeCouncil::new()
                .with_seat(105, "finance")
                .with_seat(114, "mayor-office"),
        );
        let (mut pass, log) = council_pass(&council);

        let report = pass.run(&tasks, &agents);
        assert_eq!(report.proposed.as_deref(), Some("m1"));
        assert_eq!(
            council.proposed(),
            vec![NewMotion {
                proposer_cid: 105,
                kind: "priority".to_string(),
                text: "priority: t1 — do t1".to_string(),
            }]
        );
        assert!(
            log.text()
                .contains("finance (cid 105) proposed m1 for open task t1: priority: t1 — do t1"),
            "{}",
            log.text()
        );

        // The motion now names t1, so the next passes do not propose again.
        let report = pass.run(&tasks, &agents);
        assert_eq!(report.proposed, None);
        assert_eq!(council.proposed().len(), 1);
    }

    /// The propose and the vote share one round-robin: the propose uses the
    /// first seat, and the vote (in the same pass, over the *old* read) the
    /// next.
    #[test]
    fn the_propose_and_the_vote_share_the_round_robin() {
        let tasks = vec![task("t1", "open", Some("builder"))];
        let agents = vec![agent(105, "finance"), agent(107, "legal-agents-counsel")];
        // An unrelated open motion exists so the vote step has something to act
        // on in the same pass the propose happens.
        let council = Arc::new(
            FakeCouncil::new()
                .with_motion("m1", "other", "raise morale", false, &[])
                .with_seat(105, "finance")
                .with_seat(107, "legal-agents-counsel"),
        );
        let (mut pass, _log) = council_pass(&council);

        let report = pass.run(&tasks, &agents);
        // The proposal went to finance (cursor 0), the vote to
        // legal-agents-counsel (cursor 1).
        assert_eq!(report.proposed.as_deref(), Some("m2"));
        assert_eq!(report.seat.as_deref(), Some("legal-agents-counsel"));
        assert_eq!(report.word.as_deref(), Some("no"), "m1 names no open task");
        assert_eq!(pass.seat_cursor(), 2);
    }

    /// No connected seat means no proposal and no vote: the pass logs and moves
    /// on, and makes no request beyond the read.
    #[test]
    fn no_connected_seat_is_no_action() {
        let tasks = vec![task("t1", "open", Some("builder"))];
        let council = Arc::new(FakeCouncil::new());
        let (mut pass, log) = council_pass(&council);

        let report = pass.run(&tasks, &[]);
        assert!(report.read);
        assert_eq!(report.proposed, None);
        assert_eq!(report.voted, None);
        assert!(council.proposed().is_empty());
        assert!(council.cast().is_empty());
        assert!(
            log.text()
                .contains("no seat is connected; nothing proposed")
        );
    }

    // ------------------------------------------ failure isolation and cadence

    /// A motions board that will not read is logged, swallowed, and the claim
    /// loop runs exactly as it would have.
    #[test]
    fn a_motions_board_that_will_not_read_does_not_stop_the_claim_loop() {
        let ledger = Rc::new(
            FakeLedger::with_task(task("t1", "open", Some("builder"))).with_agent(300, "builder"),
        );
        let council = Arc::new(FakeCouncil::new().read_failing("the motions answered 500: boom"));
        let log = Arc::new(Recording::default());
        let mut dispatcher = Dispatcher::new(
            Rc::clone(&ledger),
            FakeOperator::default(),
            Arc::clone(&log) as Arc<dyn Logger + Send + Sync>,
        )
        .with_council(Arc::clone(&council) as Arc<dyn Council + Send + Sync>);

        let report = dispatcher.pass().expect("the pass runs");
        assert_eq!((report.claimed, report.done), (1, 1), "the claim loop ran");
        assert!(
            log.text().contains(
                "dispatchd: council: the motions could not be read: \
                 the motions answered 500: boom; skipping"
            ),
            "{}",
            log.text()
        );
    }

    /// A refused proposal and a refused vote are logged once and swallowed: the
    /// claim loop runs and the pass result is untouched.
    #[test]
    fn a_refused_proposal_or_vote_does_not_stop_the_claim_loop() {
        // A refused proposal.
        let ledger = Rc::new(
            FakeLedger::with_task(task("t1", "open", Some("builder"))).with_agent(105, "finance"),
        );
        let council = Arc::new(
            FakeCouncil::new()
                .with_seat(105, "finance")
                .propose_failing("the proposal answered 409: no live agent has guest CID 105"),
        );
        let log = Arc::new(Recording::default());
        let mut dispatcher = Dispatcher::new(
            Rc::clone(&ledger),
            FakeOperator::default(),
            Arc::clone(&log) as Arc<dyn Logger + Send + Sync>,
        )
        .with_council(Arc::clone(&council) as Arc<dyn Council + Send + Sync>);
        let report = dispatcher.pass().expect("the pass runs");
        assert!(report.skipped >= 1, "the claim loop ran");
        assert!(
            log.text().contains("the motion for task t1 was refused"),
            "{}",
            log.text()
        );

        // A refused vote on an open motion.
        let council = Arc::new(
            FakeCouncil::new()
                .with_motion("m1", "priority", "priority: t9 — x", false, &[])
                .with_seat(105, "finance")
                .vote_failing("the vote answered 409: motion m1 is carried, not open"),
        );
        let (mut pass, log) = council_pass(&council);
        let report = pass.run(
            &[task("t9", "open", Some("builder"))],
            &[agent(105, "finance")],
        );
        assert_eq!(report.voted, None);
        assert!(
            log.text().contains("the vote on motion m1 was refused"),
            "{}",
            log.text()
        );
    }

    /// The cadence: the council pass runs on the dispatcher's third pass and
    /// then once every [`COUNCIL_EVERY`] passes — and not once in between.
    #[test]
    fn the_council_pass_runs_on_its_own_slower_cadence() {
        let ledger = FakeLedger::default();
        let council = Arc::new(FakeCouncil::new());
        let log = Arc::new(Recording::default());
        let mut dispatcher = Dispatcher::new(
            ledger,
            FakeOperator::default(),
            Arc::clone(&log) as Arc<dyn Logger + Send + Sync>,
        )
        .with_council(Arc::clone(&council) as Arc<dyn Council + Send + Sync>);

        for pass in 1..=(COUNCIL_EVERY * 2) {
            dispatcher.pass().expect("the pass runs");
            // The council passes so far: pass 1, then every third.
            let expected = pass.div_ceil(COUNCIL_EVERY);
            assert_eq!(council.reads(), expected as usize, "after pass {pass}");
        }
    }

    /// A dispatcher without the council surface makes no council call at all:
    /// the claim-only dispatcher is exactly what it was.
    #[test]
    fn a_dispatcher_without_the_council_surface_makes_no_council_call() {
        let ledger = FakeLedger::default().with_agent(300, "builder");
        let council = Arc::new(FakeCouncil::new());
        let log = Arc::new(Recording::default());
        let mut dispatcher = Dispatcher::new(
            ledger,
            FakeOperator::default(),
            Arc::clone(&log) as Arc<dyn Logger + Send + Sync>,
        );

        for _ in 0..COUNCIL_EVERY {
            dispatcher.pass().expect("the pass runs");
        }
        assert_eq!(council.reads(), 0);
        assert!(!log.text().contains("dispatchd: council:"));
    }
}
