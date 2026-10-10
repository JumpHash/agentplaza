//! The wire views the dispatcher reads and writes.
//!
//! Three shapes, all owned by other daemons — the node's ledger and the hub's
//! admin surface — and read here defensively: every field the dispatcher does
//! not strictly need has a default, and unknown fields are ignored, so a
//! daemon that grows a field does not break the dispatcher, and one that
//! omits an optional one does not either. What the dispatcher *does* need
//! (`id`, `state`, a task's `role`) is required, so an answer it cannot
//! actually read is an error rather than a task treated as something it may
//! not be.

use serde::Deserialize;

/// One task, as the ledger reports it (`GET /api/v0/tasks`).
///
/// The daemon's own `TaskView`, read; the fields the dispatcher never uses
/// (budget, verifier, timestamps) are kept so the view is the daemon's shape
/// rather than a narrowing of it that would silently drop what a later
/// workflow wants.
#[derive(Debug, Clone, Deserialize)]
pub struct Task {
    /// The task's id (`t<seq>`).
    pub id: String,
    /// What the task is for.
    #[serde(default)]
    pub goal: String,
    /// How it will be judged complete, when the creator said.
    #[serde(default)]
    pub acceptance: Option<String>,
    /// The task's budget, in credits.
    #[serde(default)]
    pub budget: u64,
    /// The role the task was created for, when it named one.
    #[serde(default)]
    pub role: Option<String>,
    /// The task's state (`open`, `claimed`, `done`, `failed`, `withdrawn`).
    pub state: String,
    /// How the task ended, when it has.
    #[serde(default)]
    pub outcome: Option<String>,
    /// The schema's `assignee_id`: the claiming agent's role, or `cid:<n>`.
    #[serde(default)]
    pub assignee_id: Option<String>,
    /// The claiming agent's guest CID, when it is claimed.
    #[serde(default)]
    pub agent_cid: Option<u64>,
    /// What the agent said when it finished.
    #[serde(default)]
    pub note: Option<String>,
    /// When the task was created, in milliseconds since the Unix epoch.
    #[serde(default)]
    pub created_at: u64,
    /// When the task last changed, in milliseconds since the Unix epoch.
    #[serde(default)]
    pub updated_at: u64,
}

impl Task {
    /// The role the task needs, when it names one.
    ///
    /// A role is matched *exactly* against the role the hub accepted from an
    /// agent: `builder` is not `Builder`, and a blank role is no role at all
    /// (the ledger refuses a blank one, so this is only the absent case).
    pub fn roled(&self) -> Option<&str> {
        self.role.as_deref().filter(|role| !role.is_empty())
    }

    /// Whether the task has ended: a terminal task is never dispatched again.
    pub fn is_terminal(&self) -> bool {
        matches!(self.state.as_str(), "done" | "failed" | "withdrawn")
    }

    /// Whether the task is claimable.
    pub fn is_open(&self) -> bool {
        self.state == "open"
    }

    /// Whether the task has been claimed by an agent.
    pub fn is_claimed(&self) -> bool {
        self.state == "claimed"
    }
}

/// One connected agent, as the hub's registry reports it (`GET /api/v0/hub`,
/// the hub's `/status` proxied by the node).
///
/// A row's *presence* is the liveness signal: the hub keeps a row only while
/// the session is alive and reaps one that stops beating, so an agent listed
/// here is connected.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Agent {
    /// The guest CID the agent connected as.
    pub cid: u64,
    /// The role the hub accepted at the handshake.
    #[serde(default)]
    pub role: String,
    /// When the session connected, in milliseconds since the Unix epoch.
    #[serde(default)]
    pub connected_at_ms: u64,
    /// When the agent last beat, in milliseconds since the Unix epoch.
    #[serde(default)]
    pub last_heartbeat_ms: u64,
    /// How many tool calls the session has served.
    #[serde(default)]
    pub calls: u64,
}

/// The way a task ends, as the ledger's finish endpoint spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The work is done.
    Done,
    /// The work could not be done.
    Failed,
}

impl Outcome {
    /// The word the finish endpoint takes.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What came of asking the hub to run one tool call on an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tell {
    /// The agent answered: its own `ok` and `output`.
    Answered {
        /// Whether the tool succeeded.
        ok: bool,
        /// The tool's output.
        output: String,
    },
    /// The hub waited and the agent did not answer. The session is untouched.
    TimedOut {
        /// How long the dispatcher was willing to wait.
        waited_ms: u64,
    },
    /// The hub serves the CID but has no live session for it.
    NotConnected,
    /// The hub does not serve the CID at all.
    UnknownCid,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A task is read from the ledger's own shape, and every field the
    /// dispatcher does not need may be absent.
    #[test]
    fn a_task_is_read_from_the_ledgers_shape() {
        let task: Task = serde_json::from_str(
            r#"{"id":"t3","goal":"draft the handbook","acceptance":"it exists","budget":3,
                "role":"builder","state":"open","outcome":null,"assignee_id":null,
                "verifier_id":null,"agent_cid":null,"note":null,"created_at":1,"updated_at":1}"#,
        )
        .expect("the ledger's own view reads");
        assert_eq!(task.id, "t3");
        assert_eq!(task.roled(), Some("builder"));
        assert!(task.is_open() && !task.is_claimed() && !task.is_terminal());

        // A minimal view: an id and a state are what the dispatcher must have.
        let task: Task =
            serde_json::from_str(r#"{"id":"t1","state":"claimed"}"#).expect("a minimal view reads");
        assert_eq!(task.goal, "");
        assert_eq!(task.roled(), None);
        assert!(task.is_claimed());

        // A task with no id or no state is not a task the dispatcher can act
        // on: it is an error, not a task with an empty id.
        assert!(serde_json::from_str::<Task>(r#"{"state":"open"}"#).is_err());
        assert!(serde_json::from_str::<Task>(r#"{"id":"t1"}"#).is_err());
    }

    /// The role rule: exact equality, and an empty role is no role.
    #[test]
    fn a_role_is_matched_exactly_and_a_blank_one_is_no_role() {
        let roled = |role: &str| {
            serde_json::from_str::<Task>(&format!(
                r#"{{"id":"t1","state":"open","role":"{role}"}}"#
            ))
            .expect("a task")
            .roled()
            .map(str::to_string)
        };
        assert_eq!(roled("builder"), Some("builder".to_string()));
        assert_eq!(roled("builder "), Some("builder ".to_string()));
        assert_eq!(roled("Builder"), Some("Builder".to_string()));
        assert_eq!(roled(""), None);
    }

    /// The terminal rule: the three states the ledger ends a task in are not
    /// dispatchable, and every state it can hold dispatches exactly one way.
    #[test]
    fn the_states_are_the_ledgers_and_the_terminals_are_terminal() {
        let state = |state: &str| {
            serde_json::from_str::<Task>(&format!(r#"{{"id":"t1","state":"{state}"}}"#))
                .expect("a task")
        };
        for terminal in ["done", "failed", "withdrawn"] {
            let task = state(terminal);
            assert!(task.is_terminal(), "{terminal} is terminal");
            assert!(!task.is_open() && !task.is_claimed());
        }
        assert!(state("open").is_open());
        assert!(state("claimed").is_claimed());
        // A state the daemon has not taught the dispatcher about is neither
        // claimable nor a retry: it is left alone.
        let unknown = state("verifying");
        assert!(!unknown.is_open() && !unknown.is_claimed() && !unknown.is_terminal());
    }

    /// An agent row is read from the hub's own shape, and its presence is the
    /// liveness signal.
    #[test]
    fn an_agent_is_read_from_the_hubs_shape() {
        let agent: Agent = serde_json::from_str(
            r#"{"cid":300,"role":"builder","connected_at_ms":1,"last_heartbeat_ms":2,"calls":4}"#,
        )
        .expect("the hub's own row reads");
        assert_eq!((agent.cid, agent.role.as_str()), (300, "builder"));

        // A row with only its CID reads too: the extra facts are the hub's.
        let agent: Agent = serde_json::from_str(r#"{"cid":1}"#).expect("a minimal row reads");
        assert_eq!(agent.role, "");
        assert!(serde_json::from_str::<Agent>(r#"{"role":"builder"}"#).is_err());
    }

    /// The outcome's spelling is the endpoint's.
    #[test]
    fn the_outcome_spelling_is_the_endpoints() {
        assert_eq!(Outcome::Done.as_str(), "done");
        assert_eq!(Outcome::Failed.as_str(), "failed");
    }
}
