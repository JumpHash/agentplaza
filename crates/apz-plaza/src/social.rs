// This crate is safe Rust only: `forbid(unsafe_code)` makes any future
// `unsafe` block a compile error (enforced at the crate root in lib.rs).

//! The social layer v0: the public board with its per-agent read cursors, the
//! contact lists, and the conversations between agents — `docs/agents/social.md`
//! over what a node of tool-server agents can honestly hold.
//!
//! # The honest constraint
//!
//! The port's agents are tool-servers, not minds: the harness serves the tool
//! calls the hub drives, and nothing sits behind an agent yet. So v0 implements
//! the plaza's social **state** faithfully — who posted what, who has read it,
//! who knows whom, who is talking to whom, and how recently — and names the
//! driver that acted for the agent in [`PostView::posted_by`]. Nothing here
//! invents an agent's opinion: a post's subject and body are the caller's own
//! words, and an award's hiring note is a template over the task's own fields.
//! When a cognition layer lands, these surfaces carry it unchanged.
//!
//! # The board
//!
//! [`PostView`] is the contract's `{id: p<seq>, agent_cid, role, subject, body,
//! task?, posted_by, created_at}`. A post is recorded **for a connected agent**
//! (the agent speaks; the caller is the driver acting for it), the author's
//! `role` is its identity in the ledger's own terms ([`ConnectedAgent::
//! assignee_id`] — its role, or `cid:<n>` for a VM the node booted without
//! one), and a post may name a `task` so the board is where discovery happens.
//! The board keeps the owner's retention of posts ([`Prune`]), dropping the
//! oldest, newest first in the response and bounded to [`BOARD_LIMIT`] there,
//! with a per-agent unread count beside it ([`BoardView::unread`]).
//!
//! # The read cursors
//!
//! Every connected agent has a cursor: the number of the last post it has read.
//! [`Social::read`] advances it, refuses a cursor past the newest post, refuses
//! one behind the agent's own (a cursor is monotonic, like a fencing token —
//! never rolled back), and is **idempotent**: replaying the same `up_to` changes
//! nothing and publishes nothing. Unread counts are derived from the cursors
//! against the board, never stored, so "who has read what" is observable and
//! can never disagree with the posts.
//!
//! # The contacts
//!
//! [`ContactView`] is `{peer_cid, role, since, kind}`. The list is the agent's
//! own; an explicit add ([`Social::add_contact`]) records `met` and is
//! idempotent — a peer already on the list keeps the kind it was first recorded
//! with, so an award's `hired` is never downgraded by a later handshake. An
//! agent is never its own contact, and the list is bounded per agent to
//! [`MAX_CONTACTS_PER_AGENT`].
//!
//! # The conversations
//!
//! [`ConversationView`] is one exchange: `{id: c<seq>, from_cid, to_cid,
//! from_role, to_role, text, created_at}` plus the delivery line, which v0
//! **decides and states**: `delivered: bool` and `delivery: Option<String>` —
//! `null` when the peer's session took the call, and otherwise the reason, in
//! the hub's own words (a refusal, a timeout, an unreachable admin) or this
//! daemon's honest statement that there is nowhere to send it. A failed
//! delivery is recorded, never hidden, and never fails the exchange: the record
//! is the point, the delivery is the attempt.
//!
//! The delivery itself is the contract's: the `conversation` tool call, with
//! `{from_role, from_cid, text}`, driven to the peer's session through the
//! hub ([`crate::hub::tell`]) — the same client the artifact
//! read-through and the operator's tool drive use. **The hub is where the
//! policy is**: a role the manifest does not grant `conversation` is the hub's
//! own 403, surfaced verbatim as the row's reason. The tool is asked for under
//! its contract name; whether the fleet serves it yet is the fleet's answer to
//! give, and it is recorded, not assumed.
//!
//! # Active pairs (the floor's proximity)
//!
//! A pair is **active** while it has an exchange within [`ACTIVE_SECS`]: the
//! front end draws two robots adjacent on exactly that, and derives it from
//! [`ConversationsView::pairs`] rather than from N² requests. The window is
//! exported in the view (`active_secs`) so nothing has to hard-code it.
//!
//! # Bounds (each stated, none silent)
//!
//! | what | bound |
//! |---|---|
//! | posts the board holds | the owner's retention ([`Prune`]: a count and an age), the oldest dropped; [`MAX_POSTS`] is the absolute ceiling, also oldest-first |
//! | a post's `subject` | [`MAX_SUBJECT`] (256 bytes) — 400 |
//! | a post's `body` | [`MAX_BODY`] (4096 bytes) — 400 |
//! | a post's `posted_by` | [`MAX_POSTED_BY`] (64 bytes) — 400 |
//! | the board response | [`BOARD_LIMIT`] (256) newest posts, `truncated` says so |
//! | contacts one agent holds | [`MAX_CONTACTS_PER_AGENT`] (256) — a fresh contact past it is refused, 409 |
//! | a conversation's `text` | [`MAX_SAY_TEXT`] (2048 bytes) — 400 |
//! | exchanges the record holds | the owner's retention ([`Prune`]: a count and an age), the oldest dropped; [`MAX_CONVERSATIONS`] is the absolute ceiling, also oldest-first |
//! | a conversations response | a page of at most [`CONVERSATIONS_LIMIT`] (256) newest, `next_before` names the next older page |
//! | pair summaries | [`MAX_PAIRS`] (256) with the newest activity, `pairs_truncated` says so |
//!
//! # What v0 narrows (stated, not hidden)
//!
//! * **The list is in this process.** Like the ledger, the treasury and the
//!   agora, the social state is held in memory and apid snapshots it, so it
//!   survives a restart; the schema's tables are a later slice. The posts, cursors, contacts and
//!   conversations are all *derived* views over it — nothing here is a second
//!   copy of a fact held elsewhere.
//! * **A contact is readable while the daemon knows the agent.** `GET
//!   /v1/agents/{cid}/contacts` answers for any CID whose list this store
//!   holds (§ the record is not thrown away when an agent stops), and for a
//!   connected agent whose list is empty; a CID the store has never heard of
//!   *and* the register does not show is the endpoint's 404.
//! * **An agent does not speak to itself.** `say` refuses `from == to`, as the
//!   contact list refuses a self-contact: a conversation is between two agents,
//!   and a pair of one would make the floor's adjacency meaningless.
//! * **A post's `task` link is checked against the ledger** at the endpoint: the
//!   board is where discovery happens, so it must not carry a link to work that
//!   does not exist.
//! * **The drivers are not here.** `social.md`'s social pass (the dispatcher
//!   reading the board, posting interest notes) is `dispatchd`'s; this module
//!   is the state and the view the drivers act on, and the award integration
//!   ([`Social::seed_award`]) is noded's own, because the award is noded's.
//! * **`since` is when the daemon recorded the link**, not a fact about the two
//!   agents' history: v0 has no history to record.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::events::{Event, EventBus};
use crate::{ConnectedAgent, now_ms};
use crate::tasks::{self, Task};

/// The absolute ceiling on posts the board holds. The owner's retention
/// ([`Prune::posts_max`]) is the working cap, and sits below this; a post past
/// the ceiling drops the oldest rather than being refused, so the board never
/// stops talking.
pub const MAX_POSTS: usize = 100_000;

/// The most a post's `subject` may occupy, in bytes.
pub const MAX_SUBJECT: usize = 256;

/// The most a post's `body` may occupy, in bytes. This is the contract's 4 KiB
/// body cap.
pub const MAX_BODY: usize = 4096;

/// The most a post's `posted_by` may occupy, in bytes (the driver's name).
pub const MAX_POSTED_BY: usize = 64;

/// The most posts a board *response* carries. The board itself may hold
/// [`MAX_POSTS`]; the endpoint returns the newest [`BOARD_LIMIT`] of them, so a
/// response is bounded — the bounding `treasury::JOURNAL_LIMIT` gives the
/// journal, with the same `total`/`limit`/`truncated` line beside it.
pub const BOARD_LIMIT: usize = 256;

/// The most contacts one agent holds. The list is the agent's own and small by
/// nature (a plaza of peers, not a directory), so v0 caps it and refuses a
/// fresh contact past the cap rather than growing without bound.
pub const MAX_CONTACTS_PER_AGENT: usize = 256;

/// The most a conversation's `text` may occupy, in bytes. This is the
/// contract's 2 KiB text cap.
pub const MAX_SAY_TEXT: usize = 2048;

/// The absolute ceiling on exchanges the record holds. The owner's retention
/// ([`Prune::conversations_max`]) is the working cap; past the ceiling the
/// oldest exchange is dropped, never the new one refused.
pub const MAX_CONVERSATIONS: usize = 100_000;

/// The most conversations a page carries (the record may hold
/// [`MAX_CONVERSATIONS`]).
pub const CONVERSATIONS_LIMIT: usize = 256;

/// The page a conversations read carries when its caller names no limit.
pub const CONVERSATIONS_PAGE: usize = 50;

/// The most bytes of a search (`q`) a conversations read takes.
pub const MAX_QUERY: usize = 200;

/// The most pair summaries a conversations response carries. Pairs are folded
/// from the record, so there are at most `MAX_CONVERSATIONS` of them; the
/// response keeps the [`MAX_PAIRS`] most recently active and says so
/// ([`ConversationsView::pairs_truncated`]).
pub const MAX_PAIRS: usize = 256;

/// How long a pair's conversation stays **active** after its last exchange, in
/// seconds. The floor's proximity derives from exactly this (the front end
/// draws two robots close while their pair is active), and it is exported in
/// every view that reports activity so no client hard-codes it.
pub const ACTIVE_SECS: u64 = 120;

/// The `posted_by` a post gets when its caller names no driver: the contract's
/// own word for the caller that acts for an agent.
pub const DEFAULT_POSTED_BY: &str = "driver";

/// The tool a conversation is delivered as, through the hub —
/// the contract's name (`conversation` with `{from_role, from_cid, text}`).
pub const CONVERSATION_TOOL: &str = "conversation";

/// The kind an agent's own handshake records.
pub const MET: &str = "met";

/// The kind the *awarder* records when it hires: the schema's `hired`.
pub const HIRED: &str = "hired";

/// The kind the *contractor* records when it is hired: the schema's
/// `was-hired`.
pub const WAS_HIRED: &str = "was-hired";

/// The `POST /v1/board` body.
#[derive(Debug, Clone, Deserialize)]
pub struct PostRequest {
    /// The guest CID of the agent the post is for — the author.
    pub agent_cid: u64,
    /// What the post is about.
    pub subject: String,
    /// The post itself.
    pub body: String,
    /// The task the post is about, when it names one: the board's link into
    /// the ledger, and what a hiring note carries.
    #[serde(default)]
    pub task: Option<String>,
    /// The driver that acted for the agent. Absent is [`DEFAULT_POSTED_BY`]:
    /// the caller is the driver.
    #[serde(default)]
    pub posted_by: Option<String>,
}

/// A post request's fields, checked, with the driver resolved.
///
/// Built only by [`PostRequest::checked`] (or by this module's own note
/// template, through it), so the caps hold for every post the board takes —
/// whether it came off the wire or from an award.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedPost {
    /// What the post is about.
    pub subject: String,
    /// The post itself.
    pub body: String,
    /// The task the post is about, when it names one.
    pub task: Option<String>,
    /// The driver that acted for the agent.
    pub posted_by: String,
}

impl PostRequest {
    /// Checks the request's text fields against the board's caps and resolves
    /// the driver, or says why the request is outside the endpoint's contract.
    pub fn checked(&self) -> Result<CheckedPost, String> {
        Ok(CheckedPost {
            subject: tasks::required_text("subject", &self.subject, MAX_SUBJECT)?,
            body: tasks::required_text("body", &self.body, MAX_BODY)?,
            // The link is checked for what this transport cannot carry, and for
            // nothing else: whether the task *exists* is the ledger's to say,
            // and the endpoint asks it.
            task: tasks::optional_text("task", &self.task, MAX_ROLE_LINK)?,
            posted_by: match &self.posted_by {
                Some(driver) => tasks::required_text("posted_by", driver, MAX_POSTED_BY)?,
                None => DEFAULT_POSTED_BY.to_string(),
            },
        })
    }
}

/// The most a post's `task` link may occupy, in bytes. A task id is `t<seq>`;
/// this is the same bound the ledger gives a role, and it is only a transport
/// bound (see [`PostRequest::checked`]).
const MAX_ROLE_LINK: usize = 64;

/// The `POST /v1/board/read` body.
#[derive(Debug, Clone, Deserialize)]
pub struct ReadRequest {
    /// The guest CID of the agent whose cursor advances.
    pub agent_cid: u64,
    /// The post to advance the cursor to (`p<seq>`).
    pub up_to: String,
}

/// The `POST /v1/agents/{cid}/contacts` body.
#[derive(Debug, Clone, Deserialize)]
pub struct ContactRequest {
    /// The guest CID of the peer to add.
    pub peer_cid: u64,
}

/// The `POST /v1/agents/{cid}/say` body.
#[derive(Debug, Clone, Deserialize)]
pub struct SayRequest {
    /// The guest CID of the agent spoken to.
    pub to_cid: u64,
    /// What is said.
    pub text: String,
}

impl SayRequest {
    /// Checks the text against the contract's cap and its floor, or says why
    /// the request is outside the endpoint's contract.
    pub fn checked(&self) -> Result<(), String> {
        tasks::required_text("text", &self.text, MAX_SAY_TEXT).map(|_| ())
    }
}

/// Whether the register shows an agent with a CID right now.
///
/// Which CIDs are connected is the *register's* to say, so it says it, and the
/// store is told: a store that guessed at liveness would be a second, weaker
/// copy of the register's rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// The register shows a live agent with this CID.
    Live,
    /// The register shows none.
    Absent,
}

/// Why the social layer refused a request.
///
/// Three shapes, one per status: a request outside the endpoint's contract
/// ([`Invalid`](SocialError::Invalid), a 400), a conflict with the state the
/// store holds ([`Conflict`](SocialError::Conflict), a 409), and a request that
/// names nothing the daemon knows ([`Unknown`](SocialError::Unknown), a 404).
/// The message is written where the rule lives, so a refusal names the rule it
/// broke rather than a code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocialError {
    /// The request is outside the endpoint's contract.
    Invalid(String),
    /// The request conflicts with the state the store holds.
    Conflict(String),
    /// The request names nothing the daemon knows.
    Unknown(String),
}

impl SocialError {
    /// The HTTP status this refusal is reported as.
    pub fn status(&self) -> u16 {
        match self {
            SocialError::Invalid(_) => 400,
            SocialError::Conflict(_) => 409,
            SocialError::Unknown(_) => 404,
        }
    }

    /// The refusal's message.
    pub fn message(&self) -> &str {
        match self {
            SocialError::Invalid(message)
            | SocialError::Conflict(message)
            | SocialError::Unknown(message) => message,
        }
    }
}

impl std::fmt::Display for SocialError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for SocialError {}

/// One post as the API reports it: the contract's `board.posts` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostView {
    /// The post's id (`p<seq>`), monotonic and never reused.
    pub id: String,
    /// The guest CID of the agent the post is for.
    pub agent_cid: u64,
    /// The author's identity in the ledger's own terms: its role, or
    /// `cid:<n>` for a VM the node booted without one. Always present.
    pub role: String,
    /// What the post is about.
    pub subject: String,
    /// The post itself.
    pub body: String,
    /// The task the post is about, or `null`. Always present.
    pub task: Option<String>,
    /// The driver that acted for the agent (the caller, or the award
    /// workflow). Always present.
    pub posted_by: String,
    /// When the post was recorded, in milliseconds since the Unix epoch.
    pub created_at: u64,
    /// The post's comments, oldest first. A post keeps its newest
    /// [`Social::MAX_COMMENTS`]; the oldest is dropped past that.
    pub comments: Vec<CommentView>,
    /// Ids of the files shown with the post (the plaza's file store). Always present.
    #[serde(default)]
    pub attachments: Vec<String>,
}

/// One comment on a board post's thread, as the API reports it: the contract's
/// `{id: c<seq>, post, agent_cid, author, body, created_at}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentView {
    /// The comment's id (`c<seq>`), on its own counter, monotonic and never reused.
    pub id: String,
    /// The post the comment is on (`p<seq>`).
    pub post: String,
    /// The commenter's guest CID, or `None` when the owner commented.
    pub agent_cid: Option<u64>,
    /// The commenter's name: the agent's role, or `owner`.
    pub author: String,
    /// The comment itself, at most [`Social::MAX_COMMENT_BODY`] characters.
    pub body: String,
    /// When the comment was recorded, in milliseconds since the Unix epoch.
    pub created_at: u64,
    /// Ids of the files shown with the comment. Always present.
    #[serde(default)]
    pub attachments: Vec<String>,
}

/// One agent's unread count: how many posts the board holds that the agent's
/// cursor has not reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnreadView {
    /// The agent's guest CID.
    pub agent_cid: u64,
    /// How many posts the agent has not read.
    pub count: usize,
}

/// The board endpoint's body: the posts, with the bounding line the journal
/// endpoint carries and every agent's unread count.
#[derive(Debug, Serialize)]
pub struct BoardView {
    /// Up to the endpoint's limit of posts, newest first.
    pub posts: Vec<PostView>,
    /// How many posts the board holds.
    pub total: usize,
    /// The limit the response was cut to.
    pub limit: usize,
    /// Whether the board holds more posts than the response carries.
    pub truncated: bool,
    /// Every agent the daemon knows — the connected ones, and any that holds a
    /// read cursor — with its unread count, in ascending CID order.
    pub unread: Vec<UnreadView>,
}

/// The read endpoint's body: the cursor's new place, and what it left unread.
#[derive(Debug, Serialize)]
pub struct ReadView {
    /// The agent whose cursor advanced.
    pub agent_cid: u64,
    /// The post the cursor now sits at.
    pub up_to: String,
    /// How many posts remain unread after the advance.
    pub unread: usize,
}

/// One contact as the API reports it: the contract's `{peer_cid, role, since,
/// kind}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContactView {
    /// The peer's guest CID.
    pub peer_cid: u64,
    /// The peer's identity in the ledger's own terms (its role, or `cid:<n>`),
    /// as it was when the link was recorded.
    pub role: String,
    /// When the daemon recorded the link, in milliseconds since the Unix epoch.
    pub since: u64,
    /// `met`, `hired` or `was-hired`.
    pub kind: &'static str,
}

/// The contacts endpoint's body: one agent's list, with the bounding line.
#[derive(Debug, Serialize)]
pub struct ContactsView {
    /// The agent whose list this is.
    pub agent_cid: u64,
    /// The agent's contacts, in the order they were recorded.
    pub contacts: Vec<ContactView>,
    /// How many contacts the agent holds.
    pub total: usize,
    /// The limit the response was cut to — the store's own per-agent cap.
    pub limit: usize,
    /// Whether the list is longer than the response carries. The store caps a
    /// list at its limit, so this is `false` for every list it holds: the
    /// field says so rather than leaving a reader to infer it.
    pub truncated: bool,
}

/// One exchange as the API reports it: the contract's `{id: c<seq>, from_cid,
/// to_cid, text, created_at}`, plus the roles the pair spoke as and the
/// delivery line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationView {
    /// The exchange's id (`c<seq>`), monotonic and never reused.
    pub id: String,
    /// The guest CID of the agent that spoke.
    pub from_cid: u64,
    /// The guest CID of the agent spoken to.
    pub to_cid: u64,
    /// The speaker's identity in the ledger's own terms.
    pub from_role: String,
    /// The listener's identity in the ledger's own terms.
    pub to_role: String,
    /// What was said.
    pub text: String,
    /// When the exchange was recorded, in milliseconds since the Unix epoch.
    pub created_at: u64,
    /// Whether the peer's session took the call.
    pub delivered: bool,
    /// Why it did not, when it did not: the hub's own words, or this daemon's
    /// statement that there is nowhere to send the call. `null` when delivered.
    pub delivery: Option<String>,
    /// Ids of the files sent with the message. Always present.
    #[serde(default)]
    pub attachments: Vec<String>,
}

/// One pair's conversation as the view folds it: who, how many exchanges, how
/// recently, and whether that recency makes them active.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PairView {
    /// The lower of the pair's two guest CIDs.
    pub a: u64,
    /// The higher of the pair's two guest CIDs.
    pub b: u64,
    /// Whether the pair has an exchange within [`ACTIVE_SECS`] — the floor's
    /// adjacency.
    pub active: bool,
    /// How many exchanges the pair has had (in the bounded record).
    pub count: usize,
    /// When the pair last spoke, in milliseconds since the Unix epoch.
    pub last_at: u64,
}

/// The conversations endpoint's body: the exchange record, newest first,
/// bounded, with the pair summaries the floor derives proximity from.
#[derive(Debug, Serialize)]
pub struct ConversationsView {
    /// Up to the endpoint's limit of exchanges, newest first.
    pub conversations: Vec<ConversationView>,
    /// One row per pair that has spoken, most recently active first.
    pub pairs: Vec<PairView>,
    /// The window [`PairView::active`] is measured over, in seconds.
    pub active_secs: u64,
    /// How many exchanges the record holds.
    pub total: usize,
    /// The limit the page was cut to.
    pub limit: usize,
    /// Whether older exchanges matching the query remain beyond this page
    /// (`next_before` is then set).
    pub truncated: bool,
    /// The `before` to pass for the next older page, or `None` when this page
    /// is the last. It is the id of the oldest exchange on this page, numbered.
    pub next_before: Option<u64>,
    /// How many pairs have spoken.
    pub pairs_total: usize,
    /// Whether more pairs have spoken than the response carries.
    pub pairs_truncated: bool,
}

/// One pair's thread: the exchanges between two agents, oldest first.
#[derive(Debug, Serialize)]
pub struct ThreadView {
    /// The first guest CID of the pair, as the path named it.
    pub a: u64,
    /// The second guest CID of the pair, as the path named it.
    pub b: u64,
    /// Whether the pair has an exchange within [`ACTIVE_SECS`].
    pub active: bool,
    /// The window [`ThreadView::active`] is measured over, in seconds.
    pub active_secs: u64,
    /// The page's exchanges, oldest first.
    pub messages: Vec<ConversationView>,
    /// How many exchanges the pair has had (in the retained record).
    pub total: usize,
    /// The limit the page was cut to.
    pub limit: usize,
    /// Whether older exchanges matching the query remain beyond this page.
    pub truncated: bool,
    /// The `before` for the next older page, or `None` when this page is the last.
    pub next_before: Option<u64>,
}

/// The owner's retention, as the store applies it: how many exchanges and
/// posts to keep, and how old one may be before it goes. Ages are in
/// milliseconds; `None` is no age limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prune {
    /// The most exchanges to keep (the oldest go first).
    pub conversations_max: usize,
    /// Exchanges older than this many milliseconds go, or `None` to keep them
    /// whatever their age.
    pub conversations_older_than_ms: Option<u64>,
    /// The most posts to keep (the oldest go first).
    pub posts_max: usize,
    /// Posts older than this many milliseconds go, or `None` to keep them
    /// whatever their age.
    pub posts_older_than_ms: Option<u64>,
}

/// What a [`Social::prune`] removed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Pruned {
    /// Exchanges removed from the record.
    pub conversations: usize,
    /// Posts removed from the board.
    pub posts: usize,
}

/// A conversations read's paging and search. The default is the first page of
/// [`CONVERSATIONS_PAGE`] with no search; apid fills it from the query string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationQuery {
    /// The search, matched case-insensitively against each exchange's text and
    /// its two roles. `None` is no filter.
    pub text: Option<String>,
    /// Guest CIDs whose exchanges match the search whatever they said (the
    /// caller maps agent names to CIDs). Only read when `text` is set.
    pub cids: Vec<u64>,
    /// Only exchanges with a numbered id below this (the `before` cursor).
    pub before: Option<u64>,
    /// The most exchanges the page carries, clamped to [`CONVERSATIONS_LIMIT`].
    pub limit: usize,
}

impl Default for ConversationQuery {
    fn default() -> Self {
        Self {
            text: None,
            cids: Vec::new(),
            before: None,
            limit: CONVERSATIONS_PAGE,
        }
    }
}

/// What an award's social seeding did.
///
/// Both halves are **best-effort**: an award stands whatever the board or a
/// contact list makes of it, so the caller reports what happened rather than
/// failing the award (see [`Social::seed_award`]).
#[derive(Debug)]
pub struct AwardSeed {
    /// The hiring note the award posted, or why it could not be posted.
    pub note: Result<PostView, SocialError>,
    /// Whether both contact links (`hired` for the awarder, `was-hired` for the
    /// contractor) were recorded.
    pub linked: bool,
}

/// The social layer's state: the board, the cursors, the contacts and the
/// conversations, and the bus every change is published to.
pub struct Social {
    events: Arc<EventBus>,
    inner: Mutex<SocialInner>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SocialInner {
    /// The board, oldest first. The oldest posts are dropped past the retention
    /// (never the newest), so the held ids run unbroken up to `posts_seq`.
    posts: Vec<PostView>,
    /// The number of the newest post (`0` while the board is empty).
    posts_seq: u64,
    /// Each agent's read cursor: the number of the last post it has read.
    cursors: BTreeMap<u64, u64>,
    /// Each agent's contact list, in the order its links were recorded.
    contacts: BTreeMap<u64, Vec<ContactView>>,
    /// The exchange record, oldest first. An id is assigned under the same
    /// lock as the row is filed, so the record is always in id order.
    conversations: Vec<ConversationView>,
    /// The number of the newest exchange (`0` while none has been recorded).
    conv_seq: u64,
    /// The number of the newest comment (`0` while none has been recorded).
    comment_seq: u64,
}

impl Social {
    /// A social layer holding no posts, cursors, contacts or conversations,
    /// that publishes every change it takes to `events`.
    pub fn new(events: Arc<EventBus>) -> Self {
        Self {
            events,
            inner: Mutex::new(SocialInner::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, SocialInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The board: the newest posts (bounded to `limit`), the record's size, and
    /// every known agent's unread count.
    ///
    /// `connected` is the register's answer — the CIDs it shows live right now.
    /// The unread list carries those agents *and* any agent that holds a cursor,
    /// because an unread count is a fact about the board and the cursor, and
    /// dropping it when an agent stops would make the count depend on liveness
    /// it has nothing to do with.
    pub fn board(&self, connected: &[u64], limit: usize) -> BoardView {
        let inner = self.lock();
        let total = inner.posts.len();
        let posts = inner.posts.iter().rev().take(limit).cloned().collect();
        let mut agents: Vec<u64> = connected.to_vec();
        for cid in inner.cursors.keys() {
            if !agents.contains(cid) {
                agents.push(*cid);
            }
        }
        agents.sort_unstable();
        let unread = agents
            .into_iter()
            .map(|agent_cid| UnreadView {
                agent_cid,
                count: unread_of(&inner, inner.cursors.get(&agent_cid).copied().unwrap_or(0)),
            })
            .collect();
        BoardView {
            posts,
            total,
            limit,
            truncated: total > limit,
            unread,
        }
    }

    /// Records `post` for `author` on the board. Past the ceiling
    /// ([`MAX_POSTS`]) the oldest post is dropped to make room, so a post is
    /// never refused for room; the owner's retention is applied by
    /// [`Social::prune`].
    ///
    /// The author's connectedness is the caller's to have established — the
    /// HTTP edge checks the register before it gets here, exactly as the claim
    /// and the bid do — and the post's own caps were checked when it was built
    /// ([`PostRequest::checked`]).
    pub fn post(
        &self,
        author: &ConnectedAgent,
        post: CheckedPost,
    ) -> Result<PostView, SocialError> {
        let mut inner = self.lock();
        drop_oldest(&mut inner.posts, MAX_POSTS - 1, None, |_| 0);
        inner.posts_seq += 1;
        let post = PostView {
            id: format!("p{}", inner.posts_seq),
            agent_cid: author.cid,
            role: author.assignee_id(),
            subject: post.subject,
            body: post.body,
            task: post.task,
            posted_by: post.posted_by,
            created_at: now_ms(),
            comments: Vec::new(),
            attachments: Vec::new(),
        };
        inner.posts.push(post.clone());
        // Published under the lock, so a stream sees the board in post order —
        // the ledger and the agora publish their transitions the same way.
        self.publish_board("post", Some(&post), author, None);
        Ok(post)
    }

    /// The most comments a post keeps; the oldest is dropped past it.
    pub const MAX_COMMENTS: usize = 200;
    /// The most characters a comment body may hold.
    pub const MAX_COMMENT_BODY: usize = 1000;

    /// Records a comment on post `post_id`, by `author` (`agent_cid` is `None`
    /// for the owner), and publishes it on the bus.
    ///
    /// The post must exist ([`SocialError::Unknown`]); the trimmed body must
    /// hold 1 to [`Social::MAX_COMMENT_BODY`] characters
    /// ([`SocialError::Invalid`]).
    pub fn comment(
        &self,
        post_id: &str,
        agent_cid: Option<u64>,
        author: &str,
        body: &str,
    ) -> Result<CommentView, SocialError> {
        let body = body.trim();
        if body.is_empty() {
            return Err(SocialError::Invalid("a comment must not be empty".to_string()));
        }
        if body.chars().count() > Self::MAX_COMMENT_BODY {
            return Err(SocialError::Invalid(format!(
                "a comment must be at most {} characters",
                Self::MAX_COMMENT_BODY
            )));
        }
        let mut inner = self.lock();
        let index = inner
            .posts
            .iter()
            .position(|post| post.id == post_id)
            .ok_or_else(|| SocialError::Unknown(format!("no post {post_id}")))?;
        inner.comment_seq += 1;
        let comment = CommentView {
            id: format!("c{}", inner.comment_seq),
            post: post_id.to_string(),
            agent_cid,
            author: author.to_string(),
            body: body.to_string(),
            created_at: now_ms(),
            attachments: Vec::new(),
        };
        let thread = &mut inner.posts[index].comments;
        if thread.len() >= Self::MAX_COMMENTS {
            thread.remove(0);
        }
        thread.push(comment.clone());
        // The frame carries the post and the comment, so a stream reads a thread
        // in the order it was made, under the same lock the thread was filed under.
        #[derive(Serialize)]
        struct Frame<'a> {
            post: &'a str,
            comment: &'a CommentView,
        }
        self.events.publish(Event::Board {
            kind: "comment",
            post: serde_json::to_string(&Frame {
                post: post_id,
                comment: &comment,
            })
            .ok(),
            agent_cid: agent_cid.unwrap_or(0),
            role: author.to_string(),
            read_up_to: None,
        });
        Ok(comment)
    }

    /// Advances `agent`'s read cursor to `up_to`, or says why it will not.
    ///
    /// The rules, each with its own refusal: `up_to` must name a post
    /// ([`SocialError::Invalid`]); it must be at or before the newest post on
    /// the board, and at or after the agent's own cursor — a cursor is
    /// monotonic, so it never rolls back ([`SocialError::Conflict`]). Replaying
    /// the cursor it already sits at is **idempotent**: the answer is the same
    /// and nothing is published, because nothing changed.
    /// Sets the files shown with a post (`p<seq>`), a comment (`c<seq>` on
    /// some post's thread) or a message (`c<seq>` in the exchange record).
    /// `on` picks which: "post", "comment" or "message".
    pub fn attach(&self, on: &str, id: &str, files: &[String]) -> Result<(), SocialError> {
        let mut inner = self.lock();
        let slot = match on {
            "post" => inner.posts.iter_mut().find(|p| p.id == id).map(|p| &mut p.attachments),
            "comment" => inner
                .posts
                .iter_mut()
                .flat_map(|p| p.comments.iter_mut())
                .find(|c| c.id == id)
                .map(|c| &mut c.attachments),
            "message" => inner.conversations.iter_mut().find(|c| c.id == id).map(|c| &mut c.attachments),
            _ => return Err(SocialError::Invalid(format!("cannot attach files to a {on}"))),
        };
        let slot = slot.ok_or_else(|| SocialError::Unknown(format!("no {on} {id}")))?;
        *slot = files.to_vec();
        if on != "message" {
            // A board view refetches on any board frame, so the files show without a reload.
            self.events.publish(Event::Board { kind: "attach", post: None, agent_cid: 0, role: String::new(), read_up_to: None });
        }
        Ok(())
    }

    pub fn read(&self, agent: &ConnectedAgent, up_to: &str) -> Result<ReadView, SocialError> {
        let seq = post_seq(up_to).ok_or_else(|| {
            SocialError::Invalid(format!("up_to must name a post (p<seq>), not {up_to}"))
        })?;
        if seq == 0 {
            return Err(SocialError::Invalid(
                "up_to must name a post: the first post is p1".to_string(),
            ));
        }
        let mut inner = self.lock();
        if inner.posts_seq == 0 {
            return Err(SocialError::Conflict(
                "the board is empty: there is no post to read up to".to_string(),
            ));
        }
        if seq > inner.posts_seq {
            return Err(SocialError::Conflict(format!(
                "up_to {up_to} is past the newest post p{}",
                inner.posts_seq
            )));
        }
        let held = inner.cursors.get(&agent.cid).copied().unwrap_or(0);
        if seq < held {
            return Err(SocialError::Conflict(format!(
                "up_to {up_to} is behind the agent's read cursor p{held}"
            )));
        }
        if seq > held {
            inner.cursors.insert(agent.cid, seq);
            self.publish_board("read", None, agent, Some(up_to));
        }
        Ok(ReadView {
            agent_cid: agent.cid,
            up_to: up_to.to_string(),
            unread: unread_of(&inner, seq),
        })
    }

    /// `agent`'s contact list, or the refusal that this daemon knows no such
    /// agent.
    ///
    /// A list this store holds is answered whatever the agent's liveness — the
    /// record is not thrown away when a VM stops — and a connected agent with
    /// no contacts yet is answered with an empty list. Only a CID the store has
    /// never heard of *and* the register does not show is unknown.
    pub fn contacts(&self, cid: u64, presence: Presence) -> Result<ContactsView, SocialError> {
        let inner = self.lock();
        let Some(contacts) = inner.contacts.get(&cid) else {
            if presence == Presence::Live {
                return Ok(ContactsView {
                    agent_cid: cid,
                    contacts: Vec::new(),
                    total: 0,
                    limit: MAX_CONTACTS_PER_AGENT,
                    truncated: false,
                });
            }
            return Err(SocialError::Unknown(format!(
                "no agent with guest CID {cid}"
            )));
        };
        Ok(ContactsView {
            agent_cid: cid,
            contacts: contacts.clone(),
            total: contacts.len(),
            limit: MAX_CONTACTS_PER_AGENT,
            truncated: contacts.len() > MAX_CONTACTS_PER_AGENT,
        })
    }

    /// Adds `peer` to `agent`'s contacts as a handshake ([`MET`]), or returns
    /// the link the pair already has.
    ///
    /// Idempotent, and deliberately conservative: the *kind* a link was first
    /// recorded with is kept, so an award's `hired` survives a later handshake,
    /// and a second identical add is not a second link. An agent is never its
    /// own contact, and a list at its cap refuses a fresh contact.
    pub fn add_contact(
        &self,
        agent: &ConnectedAgent,
        peer: &ConnectedAgent,
    ) -> Result<ContactView, SocialError> {
        self.link(agent, peer, MET)
    }

    /// Records one exchange: `from` speaks `text` to `to`; `deliver` hands it
    /// to `to` (apid queues it as work) and says whether it was taken and, if
    /// not, why.
    ///
    /// The row is written **after** the delivery attempt, so it is never a
    /// placeholder: one lock checks the record's room, the delivery runs
    /// outside the lock (a hub that answers slowly must not hold the board for
    /// every other reader), and a second lock files the row with its outcome.
    /// A refused delivery is a recorded fact, not an error: the exchange is the
    /// record of what an agent said, and it exists whether or not the peer's
    /// session was there to hear it.
    pub fn say(
        &self,
        from: &ConnectedAgent,
        to: &ConnectedAgent,
        text: &str,
        deliver: &dyn Fn(&ConnectedAgent, &ConnectedAgent, &str) -> (bool, Option<String>),
    ) -> Result<ConversationView, SocialError> {
        if from.cid == to.cid {
            return Err(SocialError::Conflict(format!(
                "guest CID {} cannot speak to itself",
                from.cid
            )));
        }
        let (delivered, delivery) = deliver(from, to, text);
        let mut inner = self.lock();
        // Past the ceiling the oldest exchange is dropped to make room, under
        // the same lock the row is filed in, so the record never refuses one.
        drop_oldest(&mut inner.conversations, MAX_CONVERSATIONS - 1, None, |_| 0);
        inner.conv_seq += 1;
        let row = ConversationView {
            id: format!("c{}", inner.conv_seq),
            from_cid: from.cid,
            to_cid: to.cid,
            from_role: from.assignee_id(),
            to_role: to.assignee_id(),
            text: text.to_string(),
            created_at: now_ms(),
            delivered,
            delivery,
            attachments: Vec::new(),
        };
        inner.conversations.push(row.clone());
        self.events.publish(Event::Conversation {
            id: row.id.clone(),
            from_cid: row.from_cid,
            to_cid: row.to_cid,
            delivered: row.delivered,
        });
        Ok(row)
    }

    /// The exchange record, newest first and bounded, with the pair summaries
    /// the floor derives proximity from.
    pub fn conversations(&self, query: &ConversationQuery) -> ConversationsView {
        let inner = self.lock();
        let limit = query.limit.clamp(1, CONVERSATIONS_LIMIT);
        let needle = search_needle(query);
        let (page, next_before) = page_of(
            inner.conversations.iter().rev(),
            query,
            needle.as_deref(),
            limit,
        );
        let (pairs, pairs_total) = pairs_of(&inner.conversations, now_ms());
        ConversationsView {
            conversations: page,
            pairs,
            active_secs: ACTIVE_SECS,
            total: inner.conversations.len(),
            limit,
            truncated: next_before.is_some(),
            next_before,
            pairs_total,
            pairs_truncated: pairs_total > MAX_PAIRS,
        }
    }

    /// Drops the oldest exchanges and posts past the owner's retention: the
    /// count caps first, then anything older than the age limits. Ids are never
    /// reused, so a dropped row's number is simply gone. Returns what it removed.
    ///
    /// Pair summaries and unread counts are derived from what remains, so they
    /// follow the prune without further work.
    pub fn prune(&self, policy: Prune) -> Pruned {
        let now = now_ms();
        let mut inner = self.lock();
        let conversations = drop_oldest(
            &mut inner.conversations,
            policy.conversations_max,
            policy.conversations_older_than_ms.map(|age| now.saturating_sub(age)),
            |row| row.created_at,
        );
        let posts = drop_oldest(
            &mut inner.posts,
            policy.posts_max,
            policy.posts_older_than_ms.map(|age| now.saturating_sub(age)),
            |post| post.created_at,
        );
        Pruned { conversations, posts }
    }

    /// One pair's thread: every exchange between the two CIDs, oldest first and
    /// bounded to the newest `limit` of them.
    ///
    /// The pair is derived from the record, never stored: `a` and `b` are read
    /// as the unordered pair the path named, so a thread is the same whichever
    /// order it is asked in, and a pair that has never spoken is an empty
    /// thread rather than a refusal.
    pub fn thread(&self, a: u64, b: u64, query: &ConversationQuery) -> ThreadView {
        let inner = self.lock();
        let now = now_ms();
        let limit = query.limit.clamp(1, CONVERSATIONS_LIMIT);
        let needle = search_needle(query);
        let rows: Vec<&ConversationView> = inner
            .conversations
            .iter()
            .filter(|row| {
                (row.from_cid == a && row.to_cid == b) || (row.from_cid == b && row.to_cid == a)
            })
            .collect();
        let total = rows.len();
        // The newest matching page, put back in oldest-first order for the reader.
        let (mut messages, next_before) =
            page_of(rows.iter().rev().copied(), query, needle.as_deref(), limit);
        messages.reverse();
        let active = rows
            .last()
            .is_some_and(|row| is_active(row.created_at, now));
        ThreadView {
            a,
            b,
            active,
            active_secs: ACTIVE_SECS,
            messages,
            total,
            limit,
            truncated: next_before.is_some(),
            next_before,
        }
    }

    /// An award's social seeding: the hiring note the award posts, and the two
    /// contact links it records.
    ///
    /// This is `social.md`'s integration — "an award posts the hiring note
    /// automatically, so the board is where discovery happens and the agora is
    /// where it concludes" — and it is deliberately **incapable of failing the
    /// award**: both halves are best-effort and reported ([`AwardSeed`]), so an
    /// award binds its task whether or not the board had room for a note. An
    /// award an agent makes to itself posts the note and links nothing, because
    /// an agent is never its own contact.
    pub fn seed_award(
        &self,
        task: &Task,
        awarder: &ConnectedAgent,
        contractor: &ConnectedAgent,
    ) -> AwardSeed {
        let note = PostRequest {
            agent_cid: awarder.cid,
            subject: clamp(&format!("hiring for {}", task.id()), MAX_SUBJECT),
            body: note_body(task),
            task: Some(task.id().to_string()),
            posted_by: Some(clamp(&awarder.assignee_id(), MAX_POSTED_BY)),
        }
        .checked()
        .map_err(SocialError::Invalid)
        .and_then(|post| self.post(awarder, post));
        if let Err(refused) = &note {
            eprintln!("noded: the award on {} posted no hiring note: {refused}", task.id());
        }
        let mut linked = true;
        if awarder.cid == contractor.cid {
            eprintln!(
                "noded: the award on {} is to guest CID {} itself; contacts not linked",
                task.id(),
                awarder.cid
            );
            linked = false;
        } else {
            for (one, other, kind) in [
                (awarder, contractor, HIRED),
                (contractor, awarder, WAS_HIRED),
            ] {
                if let Err(refused) = self.link(one, other, kind) {
                    eprintln!("noded: the award on {} recorded no {kind} contact: {refused}", task.id());
                    linked = false;
                }
            }
        }
        AwardSeed { note, linked }
    }

    /// Records `peer` on `agent`'s list with `kind`, or returns the link the
    /// pair already has (whatever kind it was recorded with).
    fn link(
        &self,
        agent: &ConnectedAgent,
        peer: &ConnectedAgent,
        kind: &'static str,
    ) -> Result<ContactView, SocialError> {
        if agent.cid == peer.cid {
            return Err(SocialError::Conflict(format!(
                "guest CID {} cannot be its own contact",
                agent.cid
            )));
        }
        let mut inner = self.lock();
        if let Some(held) = inner
            .contacts
            .get(&agent.cid)
            .and_then(|list| list.iter().find(|contact| contact.peer_cid == peer.cid))
        {
            return Ok(held.clone());
        }
        let held = inner.contacts.get(&agent.cid).map_or(0, Vec::len);
        if held >= MAX_CONTACTS_PER_AGENT {
            return Err(SocialError::Conflict(format!(
                "agent {} holds its cap of {MAX_CONTACTS_PER_AGENT} contacts",
                agent.cid
            )));
        }
        let contact = ContactView {
            peer_cid: peer.cid,
            role: peer.assignee_id(),
            since: now_ms(),
            kind,
        };
        inner
            .contacts
            .entry(agent.cid)
            .or_default()
            .push(contact.clone());
        Ok(contact)
    }

    /// Publishes one `board` event: a post, or a cursor's advance. The post is
    /// carried as the JSON object the board endpoint writes it as, so a frame
    /// and a poll never disagree about a post's shape.
    fn publish_board(
        &self,
        kind: &'static str,
        post: Option<&PostView>,
        agent: &ConnectedAgent,
        read_up_to: Option<&str>,
    ) {
        // A post is plain JSON scalars and strings, so this cannot fail; the
        // `null` fallback is the shape a frame carries when it somehow could,
        // and is not a panic on a request path.
        let encoded = post.and_then(|post| serde_json::to_string(post).ok());
        self.events.publish(Event::Board {
            kind,
            post: encoded,
            agent_cid: agent.cid,
            role: agent.assignee_id(),
            read_up_to: read_up_to.map(str::to_string),
        });
    }
}

/// Drops the oldest rows (`rows` is oldest first) until at most `max` remain,
/// and every row older than `cutoff` with them when a cutoff is given. Returns
/// how many went.
fn drop_oldest<T>(
    rows: &mut Vec<T>,
    max: usize,
    cutoff: Option<u64>,
    at: impl Fn(&T) -> u64,
) -> usize {
    let mut drop = rows.len().saturating_sub(max);
    if let Some(cutoff) = cutoff {
        drop = drop.max(rows.iter().take_while(|row| at(row) < cutoff).count());
    }
    rows.drain(..drop);
    drop
}

/// How many posts a cursor at `cursor` has not reached. Posts are dropped from
/// the front only, so the held posts are the newest ones and the count is the
/// held posts after the cursor, never more than the board holds.
fn unread_of(inner: &SocialInner, cursor: u64) -> usize {
    to_usize(inner.posts_seq.saturating_sub(cursor)).min(inner.posts.len())
}

/// A conversation's numbered id (`c<seq>`), or `0` for one that is not.
fn conv_seq(row: &ConversationView) -> u64 {
    row.id.strip_prefix('c').and_then(|n| n.parse().ok()).unwrap_or(0)
}

/// The search as the matcher reads it: lowercased, or `None` for no filter.
fn search_needle(query: &ConversationQuery) -> Option<String> {
    query.text.as_deref().map(str::to_lowercase)
}

/// Whether an exchange matches the search: its text or either role contains the
/// lowercased `needle`, or either party is one of `cids` (the names the caller
/// matched). No needle matches everything.
fn matches(row: &ConversationView, needle: Option<&str>, cids: &[u64]) -> bool {
    let Some(needle) = needle else {
        return true;
    };
    row.text.to_lowercase().contains(needle)
        || row.from_role.to_lowercase().contains(needle)
        || row.to_role.to_lowercase().contains(needle)
        || cids.contains(&row.from_cid)
        || cids.contains(&row.to_cid)
}

/// One page of `rows`, which arrive newest first: the matching rows below the
/// query's `before`, at most `limit`, newest first. The second value is the
/// `before` for the next older page (the page's oldest id) when older matches
/// remain, and `None` when this page is the last.
fn page_of<'a>(
    rows: impl Iterator<Item = &'a ConversationView>,
    query: &ConversationQuery,
    needle: Option<&str>,
    limit: usize,
) -> (Vec<ConversationView>, Option<u64>) {
    let mut page: Vec<ConversationView> = Vec::new();
    for row in rows {
        if query.before.is_some_and(|before| conv_seq(row) >= before) {
            continue;
        }
        if !matches(row, needle, &query.cids) {
            continue;
        }
        if page.len() == limit {
            // A match past the page: the page is not the last one.
            let next = page.last().map(conv_seq);
            return (page, next);
        }
        page.push(row.clone());
    }
    (page, None)
}

/// The hiring note's body: a template over the task's own fields — what the
/// call wants, and how it will be judged. Clamped to the board's cap, because
/// a task's goal may be as long as the ledger allows.
fn note_body(task: &Task) -> String {
    let mut body = format!("{} — {}", task.role().unwrap_or("any role"), task.goal());
    if let Some(acceptance) = task.acceptance() {
        body.push_str(&format!(" (acceptance: {acceptance})"));
    }
    clamp(&body, MAX_BODY)
}

/// `value`, cut to at most `max` bytes on a character boundary. Only ever
/// shrinks a template this module composes (see [`note_body`]).
fn clamp(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_string();
    }
    let mut end = max;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

/// Whether a pair's last exchange at `last_at` is recent enough at `now` for
/// the pair to be active: an exchange within [`ACTIVE_SECS`], inclusive of the
/// boundary.
fn is_active(last_at: u64, now: u64) -> bool {
    now.saturating_sub(last_at) <= ACTIVE_SECS * 1000
}

/// The pair summaries over `rows`: one row per pair that has spoken, most
/// recently active first, bounded to [`MAX_PAIRS`]. Returns the rows and how
/// many pairs the record holds, so a reader can see a window was taken.
fn pairs_of(rows: &[ConversationView], now: u64) -> (Vec<PairView>, usize) {
    let mut folded: BTreeMap<(u64, u64), (usize, u64)> = BTreeMap::new();
    for row in rows {
        let key = if row.from_cid <= row.to_cid {
            (row.from_cid, row.to_cid)
        } else {
            (row.to_cid, row.from_cid)
        };
        let entry = folded.entry(key).or_insert((0, 0));
        entry.0 += 1;
        entry.1 = entry.1.max(row.created_at);
    }
    let total = folded.len();
    let mut pairs: Vec<PairView> = folded
        .into_iter()
        .map(|((a, b), (count, last_at))| PairView {
            a,
            b,
            active: is_active(last_at, now),
            count,
            last_at,
        })
        .collect();
    // Newest activity first, and the pair's own CIDs break a tie so the order
    // is stable across reads of an unchanged record.
    pairs.sort_by(|one, other| {
        other
            .last_at
            .cmp(&one.last_at)
            .then_with(|| (one.a, one.b).cmp(&(other.a, other.b)))
    });
    pairs.truncate(MAX_PAIRS);
    (pairs, total)
}

/// The post number an id names, or `None` when the id is not `p<seq>`.
fn post_seq(id: &str) -> Option<u64> {
    id.strip_prefix('p')?.parse().ok()
}

/// A count as a `usize`: the record's own sizes are far inside it, so this can
/// only saturate on a bound the store itself refuses to reach.
fn to_usize(count: u64) -> usize {
    usize::try_from(count).unwrap_or(usize::MAX)
}

/// The stub admin surface the delivery tests use is a plain `TcpListener`, the
/// same shape `tests/agent_tell.rs` drives the daemon against: the delivery is
/// asserted from both ends — what the daemon asked for, and what the row says
/// came back.

impl Social {
    /// The whole state as JSON, for Postgres: the board with its comments and
    /// attachments, the cursors, the contacts and the exchange record. The event
    /// bus is not part of it.
    pub fn snapshot(&self) -> String {
        let inner = self.lock();
        serde_json::to_string(&*inner).expect("the social state serializes")
    }

    /// Replaces the state with `json`, a [`Social::snapshot`]. The snapshot must
    /// hold to the invariants a live operation keeps; a refusal changes nothing,
    /// and a restore publishes nothing.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let restored: SocialInner = serde_json::from_str(json)
            .map_err(|err| format!("the snapshot does not parse: {err}"))?;
        check_snapshot(&restored)?;
        *self.lock() = restored;
        Ok(())
    }
}

/// Refuses a snapshot no sequence of live operations could have produced: the
/// board's ids contiguous up to its counter, comment and
/// exchange ids unique and within their counters, every comment filed under the
/// post it names, and no cursor past the board (its unread arithmetic would
/// underflow).
fn check_snapshot(state: &SocialInner) -> Result<(), String> {
    let seq_of = |id: &str, prefix: char| id.strip_prefix(prefix).and_then(|n| n.parse::<u64>().ok());
    let held = state.posts.len() as u64;
    if held > state.posts_seq {
        return Err(format!(
            "the board holds {held} posts but its counter is {}",
            state.posts_seq
        ));
    }
    // The held posts are the newest: their ids run on, unbroken, to the counter.
    let first = state.posts_seq - held;
    let mut comment_seqs = std::collections::BTreeSet::new();
    for (index, post) in state.posts.iter().enumerate() {
        if post.id != format!("p{}", first + index as u64 + 1) {
            return Err(format!("post {} is out of place at position {}", post.id, index + 1));
        }
        if post.comments.len() > Social::MAX_COMMENTS {
            return Err(format!("post {} holds more than {} comments", post.id, Social::MAX_COMMENTS));
        }
        for comment in &post.comments {
            let seq = seq_of(&comment.id, 'c')
                .ok_or_else(|| format!("comment id {:?} is malformed", comment.id))?;
            if seq == 0 || seq > state.comment_seq {
                return Err(format!("comment {} is past the comment counter {}", comment.id, state.comment_seq));
            }
            if !comment_seqs.insert(seq) {
                return Err(format!("comment id {} appears twice", comment.id));
            }
            if comment.post != post.id {
                return Err(format!("comment {} is filed under {} but names {}", comment.id, post.id, comment.post));
            }
        }
    }
    let mut last = 0;
    for exchange in &state.conversations {
        let seq = seq_of(&exchange.id, 'c')
            .ok_or_else(|| format!("exchange id {:?} is malformed", exchange.id))?;
        if seq <= last || seq > state.conv_seq {
            return Err(format!(
                "exchange {} is out of order or past the exchange counter {}",
                exchange.id, state.conv_seq
            ));
        }
        last = seq;
    }
    if let Some((cid, cursor)) = state.cursors.iter().find(|(_, cursor)| **cursor > state.posts_seq) {
        return Err(format!("agent {cid}'s cursor {cursor} is past the board"));
    }
    Ok(())
}

/// A contact as a snapshot carries it: its kind is text until it is checked
/// against the three kinds a link can have (see [`ContactView`]).
#[derive(Deserialize)]
struct ContactRow {
    peer_cid: u64,
    role: String,
    since: u64,
    kind: String,
}

impl std::convert::TryFrom<ContactRow> for ContactView {
    type Error = String;

    fn try_from(row: ContactRow) -> Result<Self, String> {
        let kind = match row.kind.as_str() {
            MET => MET,
            HIRED => HIRED,
            WAS_HIRED => WAS_HIRED,
            other => return Err(format!("unknown contact kind {other:?}")),
        };
        Ok(ContactView {
            peer_cid: row.peer_cid,
            role: row.role,
            since: row.since,
            kind,
        })
    }
}

impl<'de> Deserialize<'de> for ContactView {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let row = ContactRow::deserialize(deserializer)?;
        ContactView::try_from(row).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::{CreateTask, TaskLedger};
    use std::sync::mpsc::Receiver;
    use std::time::Duration;

    /// A connected agent with a role, as the register would report one.
    fn agent(cid: u64, role: &str) -> ConnectedAgent {
        ConnectedAgent {
            cid,
            role: Some(role.to_string()),
            run: "r1".to_string(),
            vm: 0,
        }
    }

    /// A social layer and the receiver its bus publishes to.
    fn lab() -> (Social, Receiver<Event>) {
        let events = EventBus::new();
        let notes = events.subscribe();
        (Social::new(events), notes)
    }

    /// A post request for `cid`, with the fields a test varies.
    fn post(cid: u64, subject: &str, body: &str) -> PostRequest {
        PostRequest {
            agent_cid: cid,
            subject: subject.to_string(),
            body: body.to_string(),
            task: None,
            posted_by: None,
        }
    }

    /// Posts `subject` for `auth` and returns the row.
    fn posted(social: &Social, auth: &ConnectedAgent, subject: &str) -> PostView {
        let request = post(auth.cid, subject, "the body");
        social
            .post(auth, request.checked().expect("a post inside the caps"))
            .expect("room on the board")
    }

    #[test]
    fn a_snapshot_restores_the_board_the_record_and_the_contacts_and_refuses_garbage() {
        let (social, _notes) = lab();
        let builder = agent(100, "builder");
        let scout = agent(101, "scout");
        let first = posted(&social, &builder, "hiring?");
        posted(&social, &scout, "a second post");
        social
            .comment(&first.id, Some(101), "scout", "I can help.")
            .expect("a comment on a post");
        social
            .comment(&first.id, None, "owner", "Thanks.")
            .expect("an owner comment");
        social
            .attach("post", &first.id, &["f1".to_string()])
            .expect("a file on a post");
        social
            .say(&builder, &scout, "are you free?", &|_, _, _| (true, None))
            .expect("an exchange");
        social
            .say(&scout, &builder, "yes", &|_, _, _| (false, Some("offline".to_string())))
            .expect("a reply");
        social.add_contact(&builder, &scout).expect("a contact link");
        let json = social.snapshot();

        let (restored, notes) = lab();
        restored.restore(&json).expect("a snapshot restores");
        assert!(notes.try_recv().is_err(), "a restore publishes nothing");
        assert_eq!(
            serde_json::to_string(&restored.board(&[], 50)).unwrap(),
            serde_json::to_string(&social.board(&[], 50)).unwrap()
        );
        assert_eq!(
            serde_json::to_string(&restored.conversations(&ConversationQuery::default())).unwrap(),
            serde_json::to_string(&social.conversations(&ConversationQuery::default())).unwrap()
        );
        assert_eq!(restored.snapshot(), json, "the snapshot round-trips");

        let (fresh, _notes) = lab();
        assert!(fresh.restore("not json").is_err());
        let mut broken: serde_json::Value = serde_json::from_str(&json).unwrap();
        broken["posts_seq"] = serde_json::json!(1);
        assert!(fresh.restore(&broken.to_string()).is_err());
        assert_eq!(fresh.board(&[], 50).total, 0, "a refusal changes nothing");
    }

    #[test]
    fn a_comment_joins_its_post_and_the_owner_and_agents_may_both_comment() {
        let (social, _notes) = lab();
        let auth = agent(100, "builder");
        let row = posted(&social, &auth, "hiring?");
        assert!(row.comments.is_empty());
        let first = social
            .comment(&row.id, Some(101), "scout", "  I can help.  ")
            .expect("a comment on a post");
        assert_eq!(first.id, "c1");
        assert_eq!(first.post, "p1");
        assert_eq!(first.agent_cid, Some(101));
        assert_eq!(first.author, "scout");
        assert_eq!(first.body, "I can help.");
        let owner = social
            .comment("p1", None, "owner", "Go on.")
            .expect("the owner comments too");
        assert_eq!(owner.id, "c2");
        assert_eq!(owner.agent_cid, None);
        // An unknown post and a body outside 1..=1000 characters are refused.
        assert_eq!(
            social.comment("p9", None, "owner", "hello").unwrap_err(),
            SocialError::Unknown("no post p9".to_string())
        );
        assert_eq!(
            social.comment("p1", None, "owner", "   ").unwrap_err(),
            SocialError::Invalid("a comment must not be empty".to_string())
        );
        assert!(matches!(
            social.comment("p1", None, "owner", &"x".repeat(Social::MAX_COMMENT_BODY + 1)),
            Err(SocialError::Invalid(_))
        ));
    }

    #[test]
    fn the_board_carries_each_posts_thread_oldest_first() {
        let (social, _notes) = lab();
        let auth = agent(100, "builder");
        posted(&social, &auth, "first");
        posted(&social, &auth, "second");
        social.comment("p1", Some(101), "scout", "one").expect("a comment");
        social.comment("p2", None, "owner", "two").expect("a comment");
        social.comment("p1", None, "owner", "three").expect("a comment");
        let board = social.board(&[], 10);
        assert_eq!(board.posts[0].id, "p2");
        assert_eq!(board.posts[1].id, "p1");
        assert_eq!(board.posts[0].comments[0].body, "two");
        let bodies: Vec<&str> = board.posts[1]
            .comments
            .iter()
            .map(|comment| comment.body.as_str())
            .collect();
        assert_eq!(bodies, ["one", "three"]);
    }

    #[test]
    fn a_post_keeps_its_newest_comments_and_drops_the_oldest() {
        let (social, _notes) = lab();
        let auth = agent(100, "builder");
        posted(&social, &auth, "busy");
        for n in 0..=Social::MAX_COMMENTS {
            social
                .comment("p1", None, "owner", &format!("comment {n}"))
                .expect("a comment under the body cap is never refused");
        }
        let board = social.board(&[], 1);
        let thread = &board.posts[0].comments;
        assert_eq!(thread.len(), Social::MAX_COMMENTS);
        assert_eq!(thread[0].body, "comment 1");
        assert_eq!(thread[Social::MAX_COMMENTS - 1].body, format!("comment {}", Social::MAX_COMMENTS));
    }

    #[test]
    fn a_post_carries_the_caps_the_driver_and_the_authors_role() {
        // Every field is checked where the request is read, so a refusal names
        // the field: none of these reaches the board.
        assert_eq!(
            post(100, "   ", "body").checked().unwrap_err(),
            "subject must not be empty"
        );
        assert_eq!(
            post(100, "subject", "").checked().unwrap_err(),
            "body must not be empty"
        );
        assert_eq!(
            post(100, &"x".repeat(MAX_SUBJECT + 1), "body")
                .checked()
                .unwrap_err(),
            format!("subject must be at most {MAX_SUBJECT} bytes")
        );
        assert_eq!(
            post(100, "subject", &"x".repeat(MAX_BODY + 1))
                .checked()
                .unwrap_err(),
            format!("body must be at most {MAX_BODY} bytes")
        );
        let mut named = post(100, "subject", "body");
        named.posted_by = Some("x".repeat(MAX_POSTED_BY + 1));
        assert_eq!(
            named.checked().unwrap_err(),
            format!("posted_by must be at most {MAX_POSTED_BY} bytes")
        );
        // The caps are inclusive, not exclusive.
        assert!(
            post(100, &"x".repeat(MAX_SUBJECT), &"x".repeat(MAX_BODY))
                .checked()
                .is_ok()
        );

        let (social, notes) = lab();
        let auth = agent(100, "builder");
        let row = posted(&social, &auth, "hiring?");
        assert_eq!(row.id, "p1");
        assert_eq!(row.agent_cid, 100);
        assert_eq!(row.role, "builder");
        assert_eq!(row.subject, "hiring?");
        assert_eq!(row.task, None);
        assert_eq!(row.posted_by, DEFAULT_POSTED_BY);
        // A roleless VM is named by its CID, the ledger's own spelling.
        let roleless = ConnectedAgent {
            cid: 101,
            role: None,
            run: "r1".to_string(),
            vm: 1,
        };
        assert_eq!(posted(&social, &roleless, "hi").role, "cid:101");

        // Every post on the bus: the frame names the post, its author and its
        // role, and reads are not what this one is.
        assert_eq!(
            notes.recv_timeout(Duration::from_secs(1)),
            Ok(Event::Board {
                kind: "post",
                post: Some(serde_json::to_string(&row).expect("a serializable post")),
                agent_cid: 100,
                role: "builder".to_string(),
                read_up_to: None,
            })
        );
    }

    #[test]
    fn the_board_is_newest_first_and_bounded_to_the_window() {
        let (social, _notes) = lab();
        let auth = agent(100, "builder");
        for index in 1..=3 {
            posted(&social, &auth, &format!("post {index}"));
        }
        let board = social.board(&[100], BOARD_LIMIT);
        assert_eq!(board.total, 3);
        assert_eq!(board.limit, BOARD_LIMIT);
        assert!(!board.truncated);
        assert_eq!(
            board.posts.iter().map(|post| post.id.as_str()).collect::<Vec<_>>(),
            ["p3", "p2", "p1"]
        );

        // The window is the newest posts, and says it cut the rest.
        let window = social.board(&[100], 2);
        assert_eq!(
            window.posts.iter().map(|post| post.id.as_str()).collect::<Vec<_>>(),
            ["p3", "p2"]
        );
        assert!(window.truncated, "the window dropped p1");
        assert_eq!(window.total, 3);
    }

    #[test]
    fn an_unread_count_follows_the_agents_cursor() {
        let (social, _notes) = lab();
        let builder = agent(100, "builder");
        let librarian = agent(101, "librarian");
        for index in 1..=3 {
            posted(&social, &builder, &format!("post {index}"));
        }
        // Nobody has read: both connected agents are three posts behind, and a
        // cursor-holder that has since stopped is still reported.
        let board = social.board(&[101, 100], BOARD_LIMIT);
        assert_eq!(
            board.unread,
            vec![
                UnreadView { agent_cid: 100, count: 3 },
                UnreadView { agent_cid: 101, count: 3 },
            ],
            "ascending CID order, one row each"
        );

        let read = social.read(&librarian, "p2").expect("a cursor on the board");
        assert_eq!(read.unread, 1);
        assert_eq!(read.agent_cid, 101);
        assert_eq!(read.up_to, "p2");
        assert_eq!(
            social.board(&[100, 101], BOARD_LIMIT).unread,
            vec![
                UnreadView { agent_cid: 100, count: 3 },
                UnreadView { agent_cid: 101, count: 1 },
            ]
        );
        // The count follows the cursor, not the reader: a newer post is unread
        // again, and the agent that never read stays where it was.
        posted(&social, &builder, "post 4");
        assert_eq!(
            social.board(&[100, 101], BOARD_LIMIT).unread,
            vec![
                UnreadView { agent_cid: 100, count: 4 },
                UnreadView { agent_cid: 101, count: 2 },
            ]
        );
    }

    #[test]
    fn a_read_cursor_is_idempotent_monotonic_and_refuses_the_future() {
        let (social, notes) = lab();
        let builder = agent(100, "builder");
        let librarian = agent(101, "librarian");
        for index in 1..=2 {
            posted(&social, &builder, &format!("post {index}"));
        }
        while notes.try_recv().is_ok() {}

        // Past the newest post: refused, with the newest named.
        assert_eq!(
            social.read(&librarian, "p3").unwrap_err(),
            SocialError::Conflict("up_to p3 is past the newest post p2".to_string())
        );
        // A cursor is monotonic: once at p2, p1 is behind it.
        assert_eq!(social.read(&librarian, "p2").expect("on the board").unread, 0);
        assert_eq!(
            social.read(&librarian, "p1").unwrap_err(),
            SocialError::Conflict("up_to p1 is behind the agent's read cursor p2".to_string())
        );
        // Idempotent: the cursor it already sits at is the same answer, and
        // nothing is published because nothing changed.
        while notes.try_recv().is_ok() {}
        assert_eq!(social.read(&librarian, "p2").expect("the same cursor").unread, 0);
        assert!(
            notes.try_recv().is_err(),
            "a replay publishes nothing: the bus is quiet"
        );

        // And the refusal shapes that are not a cursor at all.
        assert_eq!(
            social.read(&librarian, "post-2").unwrap_err(),
            SocialError::Invalid("up_to must name a post (p<seq>), not post-2".to_string())
        );
        assert_eq!(
            social.read(&librarian, "p0").unwrap_err(),
            SocialError::Invalid("up_to must name a post: the first post is p1".to_string())
        );

        // An advance publishes the read frame, naming the reader and its role.
        let advanced = agent(102, "reviewer");
        let row = social.read(&advanced, "p1").expect("a cursor on the board");
        assert_eq!(row.unread, 1);
        match notes.recv_timeout(Duration::from_secs(1)) {
            Ok(Event::Board {
                kind,
                post,
                agent_cid,
                role,
                read_up_to,
            }) => {
                assert_eq!(kind, "read");
                assert_eq!(post, None, "a read carries no post");
                assert_eq!(agent_cid, 102);
                assert_eq!(role, "reviewer");
                assert_eq!(read_up_to.as_deref(), Some("p1"));
            }
            other => panic!("expected a read frame, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_board_refuses_a_cursor_before_one_post_exists() {
        let (social, _notes) = lab();
        assert_eq!(
            social.read(&agent(100, "builder"), "p1").unwrap_err(),
            SocialError::Conflict("the board is empty: there is no post to read up to".to_string())
        );
    }

    #[test]
    fn a_post_past_the_ceiling_drops_the_oldest_and_is_never_refused() {
        let (social, _notes) = lab();
        let auth = agent(100, "builder");
        posted(&social, &auth, "the one post");
        {
            // Stand the board at its ceiling with unbroken ids up to the counter.
            let mut inner = social.lock();
            let template = inner.posts[0].clone();
            inner.posts = (1..=MAX_POSTS as u64)
                .map(|seq| PostView { id: format!("p{seq}"), ..template.clone() })
                .collect();
            inner.posts_seq = MAX_POSTS as u64;
        }
        let next = posted(&social, &auth, "one more");
        assert_eq!(next.id, format!("p{}", MAX_POSTS + 1), "the id keeps counting");
        let inner = social.lock();
        assert_eq!(inner.posts.len(), MAX_POSTS, "still at the ceiling");
        assert_eq!(inner.posts.first().map(|p| p.id.as_str()), Some("p2"), "the oldest went");
        assert_eq!(inner.posts.last().map(|p| p.id.as_str()), Some(next.id.as_str()));
    }

    #[test]
    fn a_contact_is_one_way_idempotent_and_never_itself() {
        let (social, _notes) = lab();
        let builder = agent(100, "builder");
        let librarian = agent(101, "librarian");

        let added = social.add_contact(&builder, &librarian).expect("a peer");
        assert_eq!(added.peer_cid, 101);
        assert_eq!(added.role, "librarian");
        assert_eq!(added.kind, MET);
        assert!(added.since > 0);

        // The list is the agent's own: the peer's side is untouched.
        let peer_side = social.contacts(101, Presence::Live).expect("a known agent");
        assert_eq!(peer_side.total, 0);
        let mine = social.contacts(100, Presence::Live).expect("a known agent");
        assert_eq!(mine.total, 1);
        assert_eq!(mine.limit, MAX_CONTACTS_PER_AGENT);
        assert!(!mine.truncated);
        assert_eq!(mine.contacts, vec![added.clone()]);

        // Idempotent: the same add returns the same link, and the list does not
        // grow.
        assert_eq!(social.add_contact(&builder, &librarian), Ok(added.clone()));
        assert_eq!(
            social.contacts(100, Presence::Live).expect("known").total,
            1
        );
        // The kind a link was first recorded with is kept, whatever a later
        // handshake says: `hired` is not downgraded to `met`.
        let hired = social
            .link(&builder, &librarian, HIRED)
            .expect("the link already held");
        assert_eq!(hired.kind, MET, "the first kind stands");
        social
            .link(&builder, &librarian, HIRED)
            .expect("the held link");

        // An agent is never its own contact.
        assert_eq!(
            social.add_contact(&builder, &builder).unwrap_err(),
            SocialError::Conflict("guest CID 100 cannot be its own contact".to_string())
        );

        // An unknown CID is the endpoint's 404, and a live one with no list is
        // an empty list rather than not-found.
        assert_eq!(
            social.contacts(999, Presence::Absent).unwrap_err(),
            SocialError::Unknown("no agent with guest CID 999".to_string())
        );
        assert_eq!(
            social.contacts(999, Presence::Live).expect("a live agent").total,
            0
        );
        // A list this store holds outlives the agent's liveness: the record is
        // not thrown away when a VM stops.
        assert_eq!(social.contacts(100, Presence::Absent).expect("held").total, 1);
    }

    #[test]
    fn a_contact_list_refuses_a_contact_past_its_cap() {
        let (social, _notes) = lab();
        let builder = agent(100, "builder");
        {
            let mut inner = social.lock();
            // A list at its cap: the guard is the length, so this is the rule
            // itself and not a loop of 256 adds.
            inner.contacts.insert(
                100,
                (0..MAX_CONTACTS_PER_AGENT)
                    .map(|index| ContactView {
                        peer_cid: 1000 + index as u64,
                        role: "peer".to_string(),
                        since: 1,
                        kind: MET,
                    })
                    .collect(),
            );
        }
        assert_eq!(
            social.add_contact(&builder, &agent(101, "librarian")).unwrap_err(),
            SocialError::Conflict(format!(
                "agent 100 holds its cap of {MAX_CONTACTS_PER_AGENT} contacts"
            ))
        );
        // A peer already on the list still answers with its link at the cap:
        // the cap refuses a *fresh* contact, not a read of a held one.
        assert_eq!(
            social
                .add_contact(&builder, &agent(1000, "peer"))
                .expect("the held link")
                .peer_cid,
            1000
        );
    }

    #[test]
    fn an_award_posts_the_hiring_note_and_links_both_ways() {
        let events = EventBus::new();
        let notes = events.subscribe();
        let ledger = TaskLedger::new(Arc::clone(&events));
        let social = Social::new(Arc::clone(&events));
        let task = ledger
            .create(&CreateTask {
                goal: "ship the social layer".to_string(),
                acceptance: Some("the board shows the note".to_string()),
                budget: Some(500),
                role: Some("builder".to_string()),
            })
            .expect("a roled task inside the contract");
        let mayor = agent(100, "mayor");
        let builder = agent(101, "builder");

        let seed = social.seed_award(&task, &mayor, &builder);
        let note = seed.note.expect("the board had room");
        assert!(seed.linked);
        assert_eq!(note.subject, "hiring for t1");
        assert_eq!(note.task.as_deref(), Some("t1"));
        assert_eq!(note.agent_cid, 100, "the awarder authors the note");
        assert_eq!(note.role, "mayor");
        assert_eq!(note.posted_by, "mayor");
        assert_eq!(
            note.body,
            "builder — ship the social layer (acceptance: the board shows the note)",
            "the body is a template over the task's own fields"
        );

        // The links are both ways, with each side's own kind.
        let awarder = social.contacts(100, Presence::Live).expect("known");
        assert_eq!(awarder.contacts.len(), 1);
        assert_eq!(awarder.contacts[0].peer_cid, 101);
        assert_eq!(awarder.contacts[0].kind, HIRED);
        let contractor = social.contacts(101, Presence::Live).expect("known");
        assert_eq!(contractor.contacts.len(), 1);
        assert_eq!(contractor.contacts[0].peer_cid, 100);
        assert_eq!(contractor.contacts[0].kind, WAS_HIRED);
        assert_eq!(contractor.contacts[0].role, "mayor");

        // The note is a post like any other, and it is on the board's bus
        // beside the task's own creation.
        assert_eq!(social.board(&[100, 101], BOARD_LIMIT).posts[0].id, "p1");
        let mut seen = Vec::new();
        while let Ok(event) = notes.try_recv() {
            seen.push(event);
        }
        assert!(
            seen.iter().any(|event| matches!(
                event,
                Event::Board {
                    kind: "post",
                    post: Some(_),
                    ..
                }
            )),
            "the note is on the bus: {seen:?}"
        );

        // Seeding twice is idempotent: a second award of the same call cannot
        // post a second note (the call has one award), but the links hold and
        // the kinds do not flip.
        let again = social.seed_award(&task, &mayor, &builder);
        assert!(again.note.is_ok());
        assert!(again.linked);
        assert_eq!(
            social.contacts(100, Presence::Live).expect("known").contacts[0].kind,
            HIRED
        );
        assert_eq!(
            social.contacts(101, Presence::Live).expect("known").contacts[0].kind,
            WAS_HIRED
        );
    }

    #[test]
    fn an_award_an_agent_makes_to_itself_posts_a_note_and_links_nothing() {
        let events = EventBus::new();
        let ledger = TaskLedger::new(Arc::clone(&events));
        let social = Social::new(Arc::clone(&events));
        let task = ledger
            .create(&CreateTask {
                goal: "a solo job".to_string(),
                acceptance: None,
                budget: Some(1),
                role: Some("builder".to_string()),
            })
            .expect("a task");
        let builder = agent(101, "builder");

        let seed = social.seed_award(&task, &builder, &builder);
        assert!(seed.note.is_ok(), "the note still posts");
        assert!(!seed.linked, "an agent is never its own contact");
        assert_eq!(seed.note.expect("posted").body, "builder — a solo job");
        assert_eq!(
            social.contacts(101, Presence::Live).expect("known").total,
            0,
            "no self-contact, in either direction"
        );
    }

    #[test]
    fn the_hiring_notes_body_is_a_template_clamped_to_the_boards_cap() {
        let events = EventBus::new();
        let ledger = TaskLedger::new(Arc::clone(&events));
        let social = Social::new(Arc::clone(&events));
        // A goal at the ledger's own cap, with an acceptance clause beside it:
        // the composed note is longer than the board's body cap and must still
        // post, clamped on a character boundary.
        let task = ledger
            .create(&CreateTask {
                goal: "x".repeat(tasks::MAX_TEXT),
                acceptance: Some("the suite is green".to_string()),
                budget: Some(1),
                role: Some("builder".to_string()),
            })
            .expect("the ledger takes a goal at its cap");
        let seed = social.seed_award(&task, &agent(100, "mayor"), &agent(101, "builder"));
        let note = seed.note.expect("the note posts, clamped");
        assert_eq!(note.body.len(), MAX_BODY);
        assert!(
            note.body.is_char_boundary(note.body.len()),
            "the clamp cut on a character boundary"
        );
        assert_eq!(clamp("short", MAX_BODY), "short");
        assert_eq!(clamp(&"é".repeat(3), 3), "é", "a 2-byte char at 3 bytes cuts to one");
    }

    #[test]
    fn a_conversation_is_recorded_and_the_pairs_thread_is_oldest_first() {
        let (social, notes) = lab();
        let builder = agent(100, "builder");
        let librarian = agent(101, "librarian");
        let reviewer = agent(102, "reviewer");

        let first = social
            .say(&builder, &librarian, "do you have the plans?", &|_, _, _| (true, None))
            .expect("a room in the record");
        assert_eq!(first.id, "c1");
        assert_eq!(first.from_cid, 100);
        assert_eq!(first.to_cid, 101);
        assert_eq!(first.from_role, "builder");
        assert_eq!(first.to_role, "librarian");
        assert_eq!(first.text, "do you have the plans?");
        assert!(first.delivered);
        assert_eq!(first.delivery, None);
        let second = social
            .say(&librarian, &builder, "yes", &|_, _, _| (true, None))
            .expect("a room in the record");
        assert_eq!(second.id, "c2");
        social
            .say(&builder, &reviewer, "unrelated", &|_, _, _| (true, None))
            .expect("a room in the record");

        // The record is newest first, bounded.
        let all = social.conversations(&page(CONVERSATIONS_LIMIT));
        assert_eq!(all.total, 3);
        assert!(!all.truncated);
        assert_eq!(
            all.conversations.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            ["c3", "c2", "c1"]
        );
        let window = social.conversations(&page(1));
        assert!(window.truncated);
        assert_eq!(window.conversations.len(), 1);
        assert_eq!(window.conversations[0].id, "c3");
        assert_eq!(window.active_secs, ACTIVE_SECS);

        // A pair's thread is the pair's own, oldest first, whichever way the
        // path names the pair.
        let thread = social.thread(100, 101, &page(CONVERSATIONS_LIMIT));
        assert_eq!(thread.total, 2);
        assert_eq!(
            thread.messages.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            ["c1", "c2"]
        );
        assert!(thread.active, "an exchange just happened");
        assert_eq!(thread.active_secs, ACTIVE_SECS);
        assert!(!thread.truncated);
        assert_eq!(
            social.thread(101, 100, &page(CONVERSATIONS_LIMIT)).total,
            2,
            "the pair is unordered"
        );
        let bounded = social.thread(100, 101, &page(1));
        assert!(bounded.truncated);
        assert_eq!(
            bounded.messages.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            ["c2"],
            "the newest exchange is what the window keeps",
        );
        // A pair that has never spoken is an empty thread, not a refusal.
        let empty = social.thread(100, 999, &page(CONVERSATIONS_LIMIT));
        assert_eq!(empty.total, 0);
        assert!(!empty.active);
        assert!(empty.messages.is_empty());

        // Every exchange is on the bus.
        assert_eq!(
            notes.recv_timeout(Duration::from_secs(1)),
            Ok(Event::Conversation {
                id: "c1".to_string(),
                from_cid: 100,
                to_cid: 101,
                delivered: true,
            })
        );
        // And an agent does not speak to itself.
        assert_eq!(
            social.say(&builder, &builder, "hello me", &|_, _, _| (true, None)).unwrap_err(),
            SocialError::Conflict("guest CID 100 cannot speak to itself".to_string())
        );
    }

    #[test]
    fn the_pair_rows_fold_the_record_and_the_active_window_is_inclusive() {
        let row = |id: &str, from: u64, to: u64, at: u64| ConversationView {
            id: id.to_string(),
            from_cid: from,
            to_cid: to,
            from_role: "a".to_string(),
            to_role: "b".to_string(),
            text: "…".to_string(),
            created_at: at,
            delivered: false,
            delivery: None,
            attachments: Vec::new(),
        };
        // The window: an exchange within ACTIVE_SECS is active, and the
        // boundary itself counts.
        assert!(is_active(1_000_000, 1_000_000));
        assert!(is_active(1_000_000, 1_000_000 + ACTIVE_SECS * 1000));
        assert!(!is_active(1_000_000, 1_000_000 + ACTIVE_SECS * 1000 + 1));
        assert!(is_active(1_000_000, 999_000), "a clock that went back is not a gap");

        let rows = vec![
            row("c1", 101, 100, 1000),
            row("c2", 100, 101, 5000),
            row("c3", 102, 100, 9000),
            row("c4", 100, 102, 2000),
        ];
        let now = 10_000;
        let (pairs, total) = pairs_of(&rows, now);
        assert_eq!(total, 2, "two pairs, however many rows");
        // The pair that spoke last is first, and the pair is canonical (a < b).
        assert_eq!(
            (pairs[0].a, pairs[0].b, pairs[0].count, pairs[0].last_at),
            (100, 102, 2, 9000)
        );
        assert_eq!(
            (pairs[1].a, pairs[1].b, pairs[1].count, pairs[1].last_at),
            (100, 101, 2, 5000)
        );
        assert!(pairs.iter().all(|pair| pair.active));
        // A pair whose last exchange is past the window is not active, but it
        // is still a pair.
        let stale = pairs_of(&rows, 9000 + ACTIVE_SECS * 1000 + 1).0;
        assert_eq!(stale.len(), 2);
        assert!(stale.iter().all(|pair| !pair.active));

        // The window is bounded, and says how much it left out.
        let many: Vec<ConversationView> = (0..MAX_PAIRS + 5)
            .map(|index| row("c", 100, 200 + index as u64, index as u64))
            .collect();
        let (bounded, total) = pairs_of(&many, 0);
        assert_eq!(bounded.len(), MAX_PAIRS);
        assert_eq!(total, MAX_PAIRS + 5);
    }

    #[test]
    fn a_conversation_past_the_ceiling_drops_the_oldest_and_is_never_refused() {
        let (social, _notes) = lab();
        let builder = agent(100, "builder");
        let librarian = agent(101, "librarian");
        {
            let mut inner = social.lock();
            let template = ConversationView {
                id: String::new(),
                from_cid: 100,
                to_cid: 101,
                from_role: "builder".to_string(),
                to_role: "librarian".to_string(),
                text: "…".to_string(),
                created_at: 1,
                delivered: false,
                delivery: None,
                attachments: Vec::new(),
            };
            inner.conversations = (1..=MAX_CONVERSATIONS as u64)
                .map(|seq| ConversationView { id: format!("c{seq}"), ..template.clone() })
                .collect();
            inner.conv_seq = MAX_CONVERSATIONS as u64;
        }
        let row = said(&social, &builder, &librarian, "one more");
        assert_eq!(row.id, format!("c{}", MAX_CONVERSATIONS + 1));
        let view = social.conversations(&page(1));
        assert_eq!(view.total, MAX_CONVERSATIONS);
        assert_eq!(view.conversations[0].id, row.id);
        assert_eq!(social.lock().conversations.first().map(|r| r.id.as_str()), Some("c2"));
    }

    /// A query for one page of `limit` exchanges, no search.
    fn page(limit: usize) -> ConversationQuery {
        ConversationQuery { limit, ..ConversationQuery::default() }
    }

    /// Records one exchange from `from` to `to` that the peer took.
    fn said(social: &Social, from: &ConnectedAgent, to: &ConnectedAgent, text: &str) -> ConversationView {
        social
            .say(from, to, text, &|_, _, _| (true, None))
            .expect("an exchange is recorded")
    }

    /// A retention that keeps `conversations_max` exchanges and `posts_max` posts, with no age limit.
    fn keep(conversations_max: usize, posts_max: usize) -> Prune {
        Prune {
            conversations_max,
            conversations_older_than_ms: None,
            posts_max,
            posts_older_than_ms: None,
        }
    }

    #[test]
    fn prune_drops_the_oldest_exchanges_past_the_count_and_keeps_ids_monotonic() {
        let (social, _notes) = lab();
        let builder = agent(100, "builder");
        let librarian = agent(101, "librarian");
        for index in 1..=5 {
            said(&social, &builder, &librarian, &format!("message {index}"));
        }
        assert_eq!(social.prune(keep(3, 1024)), Pruned { conversations: 2, posts: 0 });
        let view = social.conversations(&page(10));
        assert_eq!(view.total, 3);
        assert_eq!(
            view.conversations.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            ["c5", "c4", "c3"]
        );
        // Pruning again with nothing to drop changes nothing.
        assert_eq!(social.prune(keep(3, 1024)), Pruned::default());
        // A fresh exchange takes the next number, never one a pruned row held.
        assert_eq!(said(&social, &builder, &librarian, "after").id, "c6");
    }

    #[test]
    fn prune_drops_exchanges_and_posts_older_than_the_age_limit() {
        let (social, _notes) = lab();
        let builder = agent(100, "builder");
        let librarian = agent(101, "librarian");
        said(&social, &builder, &librarian, "old");
        said(&social, &builder, &librarian, "fresh");
        let posted_old = posted(&social, &builder, "old post");
        posted(&social, &builder, "fresh post");
        {
            // The first exchange and the first post are a day old.
            let mut inner = social.lock();
            inner.conversations[0].created_at = now_ms() - 86_400_000;
            inner.posts[0].created_at = now_ms() - 86_400_000;
            assert_eq!(inner.posts[0].id, posted_old.id);
        }
        let hour = Prune {
            conversations_max: 100,
            conversations_older_than_ms: Some(3_600_000),
            posts_max: 100,
            posts_older_than_ms: Some(3_600_000),
        };
        assert_eq!(social.prune(hour), Pruned { conversations: 1, posts: 1 });
        let view = social.conversations(&page(10));
        assert_eq!(view.conversations.iter().map(|row| row.text.as_str()).collect::<Vec<_>>(), ["fresh"]);
        assert_eq!(social.board(&[], 10).posts.iter().map(|p| p.subject.as_str()).collect::<Vec<_>>(), ["fresh post"]);
    }

    #[test]
    fn the_board_keeps_unread_counts_correct_after_the_oldest_posts_go() {
        let (social, _notes) = lab();
        let auth = agent(100, "builder");
        let reader = agent(101, "librarian");
        for index in 1..=5 {
            posted(&social, &auth, &format!("post {index}"));
        }
        // The reader has read p1 and nothing else.
        social.read(&reader, "p1").expect("p1 is readable");
        assert_eq!(social.prune(keep(1024, 2)).posts, 3);
        // p1 to p3 are gone and p4, p5 are held. The reader's cursor sits at p1,
        // so both held posts are unread: the count is clamped to what remains,
        // not the three posts the raw arithmetic would name.
        let board = social.board(&[100, 101], 10);
        assert_eq!(board.total, 2);
        assert_eq!(board.unread.iter().find(|u| u.agent_cid == 101).map(|u| u.count), Some(2));
        // An author with no cursor also sees only the held posts as unread.
        assert_eq!(board.unread.iter().find(|u| u.agent_cid == 100).map(|u| u.count), Some(2));
        let next = social.read(&reader, "p5").expect("p5 is readable");
        assert_eq!(next.unread, 0);
    }

    #[test]
    fn a_conversations_page_walks_back_in_disjoint_consecutive_pages() {
        let (social, _notes) = lab();
        let builder = agent(100, "builder");
        let librarian = agent(101, "librarian");
        for index in 1..=5 {
            said(&social, &builder, &librarian, &format!("message {index}"));
        }
        let first = social.conversations(&page(2));
        assert_eq!(first.conversations.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["c5", "c4"]);
        assert_eq!(first.next_before, Some(4));
        assert!(first.truncated);

        let second = social.conversations(&ConversationQuery { before: first.next_before, ..page(2) });
        assert_eq!(second.conversations.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["c3", "c2"]);
        assert_eq!(second.next_before, Some(2));

        let last = social.conversations(&ConversationQuery { before: second.next_before, ..page(2) });
        assert_eq!(last.conversations.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["c1"]);
        assert_eq!(last.next_before, None);
        assert!(!last.truncated);

        // A thread pages the same way, each page oldest first.
        let thread = social.thread(101, 100, &page(2));
        assert_eq!(thread.messages.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["c4", "c5"]);
        assert_eq!(thread.next_before, Some(4));
        let older = social.thread(100, 101, &ConversationQuery { before: thread.next_before, ..page(2) });
        assert_eq!(older.messages.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["c2", "c3"]);
        assert_eq!(older.total, 5);
    }

    #[test]
    fn a_conversations_search_matches_text_roles_and_named_agents() {
        let (social, _notes) = lab();
        let builder = agent(100, "builder");
        let librarian = agent(101, "librarian");
        let reviewer = agent(102, "reviewer");
        said(&social, &builder, &librarian, "The Budget report is ready");
        said(&social, &builder, &librarian, "lunch?");
        said(&social, &librarian, &reviewer, "nothing to see");

        let by_text = ConversationQuery { text: Some("budget".into()), ..page(10) };
        assert_eq!(
            social.conversations(&by_text).conversations.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["c1"],
            "the match is case-insensitive"
        );
        let by_role = ConversationQuery { text: Some("reviewer".into()), ..page(10) };
        assert_eq!(social.conversations(&by_role).conversations.len(), 1, "a role is searched too");
        // A name the caller mapped to a CID matches every exchange that CID is in.
        let by_name = ConversationQuery { text: Some("zzz".into()), cids: vec![102], ..page(10) };
        assert_eq!(
            social.conversations(&by_name).conversations.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["c3"]
        );
        // Search narrows the thread too; the pair's total is still the whole pair.
        let thread = social.thread(100, 101, &ConversationQuery { text: Some("lunch".into()), ..page(10) });
        assert_eq!(thread.messages.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), ["lunch?"]);
        assert_eq!(thread.total, 2);
        assert!(thread.next_before.is_none());
    }

    #[test]
    fn a_say_outside_the_contract_is_refused_before_anything_is_recorded() {
        assert_eq!(
            SayRequest {
                to_cid: 101,
                text: "  ".to_string()
            }
            .checked()
            .unwrap_err(),
            "text must not be empty"
        );
        assert_eq!(
            SayRequest {
                to_cid: 101,
                text: "x".repeat(MAX_SAY_TEXT + 1)
            }
            .checked()
            .unwrap_err(),
            format!("text must be at most {MAX_SAY_TEXT} bytes")
        );
        assert!(
            SayRequest {
                to_cid: 101,
                text: "x".repeat(MAX_SAY_TEXT)
            }
            .checked()
            .is_ok(),
            "the cap is inclusive"
        );
    }

    #[test]
    fn a_post_the_board_could_not_serialize_is_still_a_frame() {
        // The frame carries the post's own JSON, so the two never disagree —
        // asserted here because the stream's shape depends on it.
        let post = PostView {
            id: "p1".to_string(),
            agent_cid: 100,
            role: "builder".to_string(),
            subject: "a \"quoted\" subject\nwith a newline".to_string(),
            body: "the body".to_string(),
            task: None,
            posted_by: DEFAULT_POSTED_BY.to_string(),
            created_at: 7,
            comments: Vec::new(),
            attachments: Vec::new(),
        };
        let encoded = serde_json::to_string(&post).expect("a serializable post");
        assert_eq!(
            encoded,
            r#"{"id":"p1","agent_cid":100,"role":"builder","subject":"a \"quoted\" subject\nwith a newline","body":"the body","task":null,"posted_by":"driver","created_at":7,"comments":[],"attachments":[]}"#
        );
        let frame = Event::Board {
            kind: "post",
            post: Some(encoded),
            agent_cid: 100,
            role: "builder".to_string(),
            read_up_to: None,
        }
        .frame();
        assert_eq!(frame.matches("data: ").count(), 1, "one data line: {frame}");
    }
}
