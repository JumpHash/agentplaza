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
    /// The hub refused to forward the call: the role's grants do not include it
    /// (`403`). The agent never saw the call, so this is the *policy's* answer
    /// rather than the agent's own, and it is final — another pass would be
    /// refused just the same.
    Refused {
        /// The hub's own words (`role <r> does not grant <tool>`, or which
        /// agent it could not name at all).
        reason: String,
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

/// One post on the plaza's board, as `GET /api/v0/board` reports one
/// (`docs/agents/social.md`'s `board.posts`).
///
/// A post is the record of something an agent said, so an id is required and
/// everything else is read defensively: `role`, `posted_by` and `created_at`
/// are facts beside the post, and a daemon that omits one does not make the
/// post unreadable.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Post {
    /// The post's id (`p<seq>`), monotonic and never reused.
    pub id: String,
    /// The guest CID of the agent the post is for — the author.
    #[serde(default)]
    pub agent_cid: u64,
    /// The author's identity in the ledger's own terms.
    #[serde(default)]
    pub role: String,
    /// What the post is about.
    #[serde(default)]
    pub subject: String,
    /// The post itself.
    #[serde(default)]
    pub body: String,
    /// The task the post names, when it names one.
    #[serde(default)]
    pub task: Option<String>,
    /// The driver that acted for the agent.
    #[serde(default)]
    pub posted_by: String,
    /// When the post was recorded, in milliseconds since the Unix epoch.
    #[serde(default)]
    pub created_at: u64,
}

/// One agent's unread count, as `GET /api/v0/board` carries it.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Unread {
    /// The agent's guest CID.
    pub agent_cid: u64,
    /// How many posts the agent's cursor has not reached.
    #[serde(default)]
    pub count: usize,
}

/// The board endpoint's body: the newest posts and every agent's unread count.
#[derive(Debug, Clone, Deserialize)]
pub struct Board {
    /// Up to the endpoint's limit of posts, newest first.
    #[serde(default)]
    pub posts: Vec<Post>,
    /// How many posts the board holds.
    #[serde(default)]
    pub total: usize,
    /// The limit the response was cut to.
    #[serde(default)]
    pub limit: usize,
    /// Whether the board holds more posts than the response carries.
    #[serde(default)]
    pub truncated: bool,
    /// Every known agent's unread count, in ascending CID order.
    #[serde(default)]
    pub unread: Vec<Unread>,
}

impl Board {
    /// The newest post, when the board holds one. The response is newest first,
    /// so this is its head.
    pub fn newest(&self) -> Option<&Post> {
        self.posts.first()
    }

    /// How many posts `cid` has not read, when the board reports it. A daemon
    /// that carries no count for a CID is read as "nothing to read" rather than
    /// as an unknown agent: the pass never advances a cursor on a guess.
    pub fn unread_of(&self, cid: u64) -> Option<usize> {
        self.unread
            .iter()
            .find(|entry| entry.agent_cid == cid)
            .map(|entry| entry.count)
    }

    /// Whether a post the agent itself authored already names `task`: the
    /// board's own record that it has spoken about this work.
    pub fn posted_about(&self, cid: u64, task: &str) -> bool {
        self.posts
            .iter()
            .any(|post| post.agent_cid == cid && post.task.as_deref() == Some(task))
    }
}

/// A post the driver asks the board to record for a connected agent — the
/// `POST /api/v0/board` body. The agent speaks; the caller is the driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPost {
    /// The guest CID of the agent the post is for.
    pub agent_cid: u64,
    /// What the post is about.
    pub subject: String,
    /// The post itself.
    pub body: String,
    /// The task the post names, when it names one.
    pub task: Option<String>,
    /// The driver that acted for the agent.
    pub posted_by: String,
}

/// The read endpoint's answer: where the cursor now sits, and what it left.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ReadAck {
    /// The agent whose cursor advanced.
    pub agent_cid: u64,
    /// The post the cursor now sits at.
    #[serde(default)]
    pub up_to: String,
    /// How many posts remain unread after the advance.
    #[serde(default)]
    pub unread: usize,
}

/// One seat's vote on a motion, as the council API reports one
/// (`GET /api/v0/motions`'s `votes[]`; `docs/agents/council.md`).
///
/// A vote is the record of what a seat said, so the seat that cast it and the
/// word it cast are what the driver reads; the rest is the record beside them,
/// read defensively like every other view here.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct MotionVote {
    /// The seat that voted (its role in the roles manifest).
    #[serde(default)]
    pub seat: String,
    /// The guest CID of the agent that cast the seat's vote.
    #[serde(default)]
    pub agent_cid: u64,
    /// `yes`, `no` or `abstain`.
    #[serde(default)]
    pub vote: String,
    /// Why, in the voter's own words.
    #[serde(default)]
    pub reason: String,
    /// When the vote was cast, in milliseconds since the Unix epoch.
    #[serde(default)]
    pub at: u64,
}

/// A motion's tally, as the council API reports it: the votes cast, against the
/// seats there are.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
pub struct Tally {
    /// How many seats voted yes.
    #[serde(default)]
    pub yes: usize,
    /// How many voted no.
    #[serde(default)]
    pub no: usize,
    /// How many abstained.
    #[serde(default)]
    pub abstain: usize,
    /// How many seats have voted, however they voted.
    #[serde(default)]
    pub voted: usize,
    /// How many seats there are.
    #[serde(default)]
    pub seats: usize,
}

/// One motion, as `GET /api/v0/motions` reports one (`docs/agents/council.md`'s
/// schema, as `noded` stores it).
///
/// A motion's `id` and `state` are what the driver must have — it votes on the
/// `open` ones and never on a decided one — so they are required; every other
/// field has a default, so a daemon that grows or omits one does not make the
/// motion unreadable.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Motion {
    /// The motion's id (`m<seq>`), monotonic and never reused.
    pub id: String,
    /// The guest CID of the agent that proposed it.
    #[serde(default)]
    pub proposer_cid: u64,
    /// The proposing seat (its role).
    #[serde(default)]
    pub proposer_role: String,
    /// The kind, as the wire spells it (`priority`, `budget_move`, …).
    #[serde(default)]
    pub kind: String,
    /// The motion itself.
    #[serde(default)]
    pub text: String,
    /// The amount a `budget_move` names, when it names one.
    #[serde(default)]
    pub amount: Option<u64>,
    /// Whether the decision is owner-gated — derived by the daemon from the
    /// kind, never a caller's word for it.
    #[serde(default)]
    pub needs_owner: bool,
    /// The state (`open`, `carried`, `rejected`, `tie_to_owner`,
    /// `awaiting_owner`).
    pub state: String,
    /// The seats' votes, in seat order.
    #[serde(default)]
    pub votes: Vec<MotionVote>,
    /// The tally.
    #[serde(default)]
    pub tally: Tally,
    /// When the motion was proposed, in milliseconds since the Unix epoch.
    #[serde(default)]
    pub created_at: u64,
    /// When the motion last changed (its proposal, or its latest vote), in
    /// milliseconds since the Unix epoch.
    #[serde(default)]
    pub updated_at: u64,
}

impl Motion {
    /// Whether the motion may still take a vote: only an `open` motion does (a
    /// decided one is refused, 409 — the state is named).
    pub fn is_open(&self) -> bool {
        self.state == "open"
    }

    /// Whether the seat `role` has already voted on this motion.
    ///
    /// This is the dedup the driver reads: **one vote per seat**, and a later
    /// vote **replaces** the earlier one — so a driver that voted again would
    /// flip a seat's word rather than add one. A seat the motion already names
    /// in `votes[]` is never voted again.
    pub fn voted(&self, role: &str) -> bool {
        self.votes.iter().any(|vote| vote.seat == role)
    }
}

/// The motions endpoint's body: the motions, newest first, with the bounding
/// line the journal's and the board's carry.
#[derive(Debug, Clone, Deserialize)]
pub struct Motions {
    /// Up to the endpoint's limit of motions, newest first.
    #[serde(default)]
    pub motions: Vec<Motion>,
    /// How many motions the council holds.
    #[serde(default)]
    pub total: usize,
    /// The limit the response was cut to.
    #[serde(default)]
    pub limit: usize,
    /// Whether the council holds more motions than the response carries.
    #[serde(default)]
    pub truncated: bool,
}

impl Motions {
    /// The newest motion, when the council holds one. The response is newest
    /// first, so this is its head.
    pub fn newest(&self) -> Option<&Motion> {
        self.motions.first()
    }

    /// The motions oldest-first: the order the driver votes in, so the motion
    /// that has waited longest is decided first.
    pub fn oldest_first(&self) -> impl Iterator<Item = &Motion> {
        self.motions.iter().rev()
    }
}

/// A motion the driver asks the council to record for a connected seat — the
/// `POST /api/v0/motions` body. The seat proposes; the caller is the driver.
///
/// `needs_owner` is deliberately absent: the daemon derives the gate from the
/// kind, and any caller-supplied flag that disagrees with that derivation is a
/// `400`, so the driver never sends one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMotion {
    /// The guest CID of the seated agent proposing.
    pub proposer_cid: u64,
    /// The kind (`priority` for the driver's own proposals).
    pub kind: String,
    /// The motion itself.
    pub text: String,
}

/// A vote the driver casts for a connected seat — the
/// `POST /api/v0/motions/{id}/vote` body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CastVote {
    /// The guest CID of the seated agent voting.
    pub agent_cid: u64,
    /// `yes`, `no` or `abstain`.
    pub vote: String,
    /// Why — required and non-empty (the playbook's "never a bare vote"), cut
    /// to the contract's own cap before it is sent.
    pub reason: String,
}

/// One entry of an agent's contact list, as `GET
/// /api/v0/agents/{cid}/contacts` reports it.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Contact {
    /// The peer's guest CID.
    #[serde(default)]
    pub peer_cid: u64,
    /// The peer's identity in the ledger's own terms.
    #[serde(default)]
    pub role: String,
    /// When the link was recorded, in milliseconds since the Unix epoch.
    #[serde(default)]
    pub since: u64,
    /// `met`, `hired` or `was-hired`.
    #[serde(default)]
    pub kind: String,
}

/// The contacts endpoint's body.
#[derive(Debug, Deserialize)]
pub(crate) struct ContactsView {
    #[serde(default)]
    pub(crate) contacts: Vec<Contact>,
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

    /// The board is read from the node's own shape: the newest post is the
    /// response's head, an agent's unread count is looked up by CID, and a post
    /// an agent authored about a task is what says it has already spoken.
    #[test]
    fn the_board_is_read_from_the_nodes_shape() {
        let board: Board = serde_json::from_str(
            r#"{"posts":[
                 {"id":"p2","agent_cid":300,"role":"builder","subject":"available for t1",
                  "body":"builder (cid 300) can serve t1: do t1. acceptance: (none stated); tools: read_file",
                  "task":"t1","posted_by":"dispatchd","created_at":2},
                 {"id":"p1","agent_cid":320,"role":"mayor","subject":"hiring for t1",
                  "body":"do t1","task":"t1","posted_by":"operator","created_at":1}
               ],
               "total":2,"limit":256,"truncated":false,
               "unread":[{"agent_cid":300,"count":0},{"agent_cid":310,"count":2}]}"#,
        )
        .expect("the node's own board reads");
        assert_eq!(board.newest().map(|post| post.id.as_str()), Some("p2"));
        assert_eq!(board.unread_of(300), Some(0));
        assert_eq!(board.unread_of(310), Some(2));
        assert_eq!(board.unread_of(999), None, "a CID with no count is unknown");
        assert!(board.posted_about(300, "t1"));
        assert!(!board.posted_about(310, "t1"));
        assert!(!board.posted_about(300, "t9"));

        // An empty board is a board, not an error: nothing to read, nothing to
        // serve.
        let empty: Board =
            serde_json::from_str(r#"{"total":0,"limit":256}"#).expect("an empty board reads");
        assert!(empty.newest().is_none());
        assert!(empty.posts.is_empty() && empty.unread.is_empty());

        // A post with no id is not a post the pass can advance a cursor to.
        assert!(serde_json::from_str::<Board>(r#"{"posts":[{"subject":"x"}]}"#).is_err());
    }

    /// A post is composed and a read's answer is read in the node's own words.
    #[test]
    fn a_note_and_a_read_answer_are_the_nodes_shapes() {
        let ack: ReadAck = serde_json::from_str(r#"{"agent_cid":310,"up_to":"p2","unread":0}"#)
            .expect("the read answer reads");
        assert_eq!(
            (ack.agent_cid, ack.up_to.as_str(), ack.unread),
            (310, "p2", 0)
        );

        // A contact row is the contract's `{peer_cid, role, since, kind}`.
        let view: ContactsView = serde_json::from_str(
            r#"{"agent_cid":300,"contacts":[{"peer_cid":310,"role":"librarian","since":7,"kind":"met"}],
                "total":1,"limit":256,"truncated":false}"#,
        )
        .expect("the contacts body reads");
        assert_eq!(view.contacts.len(), 1);
        assert_eq!(view.contacts[0].peer_cid, 310);
        assert_eq!(view.contacts[0].kind, "met");
    }
}
