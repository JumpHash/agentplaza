// This crate is safe Rust only: `forbid(unsafe_code)` makes any future
// `unsafe` block a compile error (enforced at the crate root in lib.rs).

//! The board: the public posts that agents and the owner write, the comments on them,
//! and each connected agent's read cursor over it.
//!
//! # The board
//!
//! [`PostView`] is the contract's `{id: p<seq>, agent_cid, role, subject, body,
//! task?, posted_by, created_at}`. A post is recorded **for a connected agent**
//! (the agent speaks; the caller is the driver acting for it), the author's
//! `role` is its identity in the ledger's own terms ([`ConnectedAgent::
//! assignee_id`] — its role, or `cid:<n>` for a VM the node booted without
//! one), and a post may name a `task`, linking the board into the ledger. The
//! board keeps the owner's retention of posts ([`Prune`]), dropping the oldest,
//! newest first in the response and bounded to [`BOARD_LIMIT`] there, with a
//! per-agent unread count beside it ([`BoardView::unread`]).
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
//! Direct messages between agents are not recorded here: they are delivered as
//! work items, and nothing about them is kept on the board.
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
//!
//! # What v0 narrows (stated, not hidden)
//!
//! * **The board is in plaza.db.** It is held in memory, and its rows (the
//!   `social` tables) are saved by the store's saver, so it survives a restart.
//! * **The drivers are not here.** The board is the state and the view the
//!   drivers act on; who posts for an agent is the caller's to say.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};
use serde::{Deserialize, Serialize};

use crate::events::{Event, EventBus};
use crate::tasks;
use crate::{ConnectedAgent, now_ms};

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
/// response is bounded, with a `total`/`limit`/`truncated` line beside it.
pub const BOARD_LIMIT: usize = 256;

/// The `posted_by` a post gets when its caller names no driver: the contract's
/// own word for the caller that acts for an agent.
pub const DEFAULT_POSTED_BY: &str = "driver";

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
    /// the ledger.
    #[serde(default)]
    pub task: Option<String>,
    /// The driver that acted for the agent. Absent is [`DEFAULT_POSTED_BY`]:
    /// the caller is the driver.
    #[serde(default)]
    pub posted_by: Option<String>,
}

/// A post request's fields, checked, with the driver resolved.
///
/// Built only by [`PostRequest::checked`] (or by the owner's post path, through
/// its own checks), so the caps hold for every post the board takes.
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
    /// The project the post belongs to, or `None` for the whole board. It is not on the wire: the
    /// hub sets it to the author's project.
    pub project: Option<String>,
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
            project: None,
        })
    }
}

/// The most a post's `task` link may occupy, in bytes. A task id is `t<seq>`;
/// this is the same bound the ledger gives a role, and it is only a transport
/// bound (see [`PostRequest::checked`]).
const MAX_ROLE_LINK: usize = 64;

/// The `POST /v1/board/posts` body: the owner's post, for the whole board or one project.
#[derive(Debug, Clone, Deserialize)]
pub struct OwnerPostRequest {
    /// What the post is about. Absent is the first characters of the body.
    #[serde(default)]
    pub subject: Option<String>,
    /// The post itself.
    pub body: String,
    /// The project the post is for. Absent is the whole board.
    #[serde(default)]
    pub project: Option<String>,
    /// The agents the post mentions, by name or role, and `all` for everyone on the project
    /// (or every agent, when the post has no project). A leading `@` is dropped.
    #[serde(default)]
    pub mentions: Vec<String>,
}

/// An owner's post, checked, with its mentions normalized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedOwnerPost {
    /// What the post is about.
    pub subject: String,
    /// The post itself.
    pub body: String,
    /// The project the post is for, or `None` for the whole board.
    pub project: Option<String>,
    /// The mentioned names, without their `@`, first mention first, no repeats (case-insensitive).
    pub mentions: Vec<String>,
}

impl OwnerPostRequest {
    /// Checks the post against the board's caps, or says why the request is outside the endpoint's
    /// contract. The project's existence is the caller's to check.
    pub fn checked(&self) -> Result<CheckedOwnerPost, String> {
        let body = tasks::required_text("body", &self.body, MAX_BODY)?;
        let subject = match self
            .subject
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(subject) => tasks::required_text("subject", subject, MAX_SUBJECT)?,
            None => body.trim().chars().take(60).collect(),
        };
        let project = tasks::optional_text("project", &self.project, MAX_ROLE_LINK)?;
        if self.mentions.len() > MAX_MENTIONS {
            return Err(format!("a post mentions at most {MAX_MENTIONS} names"));
        }
        let mut mentions: Vec<String> = Vec::new();
        for raw in &self.mentions {
            let name =
                tasks::required_text("mention", raw.trim().trim_start_matches('@'), MAX_MENTION)?;
            if !mentions.iter().any(|seen| seen.eq_ignore_ascii_case(&name)) {
                mentions.push(name);
            }
        }
        Ok(CheckedOwnerPost {
            subject,
            body,
            project,
            mentions,
        })
    }
}

/// The `POST /v1/board/read` body.
#[derive(Debug, Clone, Deserialize)]
pub struct ReadRequest {
    /// The guest CID of the agent whose cursor advances.
    pub agent_cid: u64,
    /// The post to advance the cursor to (`p<seq>`).
    pub up_to: String,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
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
    /// The driver that acted for the agent (the caller). Always present.
    pub posted_by: String,
    /// When the post was recorded, in milliseconds since the Unix epoch.
    pub created_at: u64,
    /// The post's comments, oldest first. A post keeps its newest
    /// [`Social::MAX_COMMENTS`]; the oldest is dropped past that.
    #[table(child = "social_post_comment")]
    pub comments: Vec<CommentView>,
    /// Ids of the files shown with the post (the plaza's file store). Always present.
    #[serde(default)]
    #[table(list = "social_post_attachment")]
    pub attachments: Vec<String>,
    /// Whether the owner wrote the post. Its `agent_cid` is `0` and its `role` is [`OWNER_ROLE`].
    #[serde(default)]
    pub owner: bool,
    /// The project the post is for, or `null` for the whole board.
    #[serde(default)]
    pub project: Option<String>,
    /// The names the owner mentioned (`all` for everyone), as written. Empty for an agent's post.
    #[serde(default)]
    #[table(list = "social_post_mention")]
    pub mentions: Vec<String>,
}

/// The role and driver an owner's post carries.
pub const OWNER_ROLE: &str = "owner";

/// The most names one owner post may mention.
pub const MAX_MENTIONS: usize = 16;

/// The most bytes one mentioned name may occupy.
pub const MAX_MENTION: usize = 64;

/// One comment on a board post's thread, as the API reports it: the contract's
/// `{id: c<seq>, post, agent_cid, author, body, created_at}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
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
    #[table(list = "social_comment_attachment")]
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

/// The owner's retention, as the store applies it: how many posts to keep, and how old one may
/// be before it goes. Ages are in milliseconds; `None` is no age limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prune {
    /// The most posts to keep (the oldest go first).
    pub posts_max: usize,
    /// Posts older than this many milliseconds go, or `None` to keep them whatever their age.
    pub posts_older_than_ms: Option<u64>,
}

/// What a [`Social::prune`] removed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Pruned {
    /// Posts removed from the board.
    pub posts: usize,
}

/// The board's state: the posts, their comments and the read cursors, and the bus every change
/// is published to.
pub struct Social {
    events: Arc<EventBus>,
    inner: Mutex<SocialInner>,
}

#[derive(Debug, Default, Serialize, Deserialize, apz_store::Table)]
#[table(name = "social")]
struct SocialInner {
    /// The board, oldest first. The oldest posts are dropped past the retention
    /// (never the newest), so the held ids run unbroken up to `posts_seq`.
    #[table(child = "social_post")]
    posts: Vec<PostView>,
    /// The number of the newest post (`0` while the board is empty).
    posts_seq: u64,
    /// Each agent's read cursor: the number of the last post it has read.
    #[table(map_value = "social_cursor")]
    cursors: BTreeMap<u64, u64>,
    /// The number of the newest comment (`0` while none has been recorded).
    comment_seq: u64,
}

impl Social {
    /// A board holding no posts or cursors, that publishes every change it takes to `events`.
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
        self.board_for(connected, limit, |_| true)
    }

    /// [`Social::board`] over the posts `keep` accepts: the newest `limit` of them, and their count.
    /// The unread counts are the board's own: a cursor counts every post, kept or not.
    pub fn board_for(
        &self,
        connected: &[u64],
        limit: usize,
        keep: impl Fn(&PostView) -> bool,
    ) -> BoardView {
        let inner = self.lock();
        let kept: Vec<&PostView> = inner.posts.iter().filter(|post| keep(post)).collect();
        let total = kept.len();
        let posts = kept
            .iter()
            .rev()
            .take(limit)
            .map(|post| (*post).clone())
            .collect();
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
    /// does — and the post's own caps were checked when it was built
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
            owner: false,
            project: post.project,
            mentions: Vec::new(),
        };
        inner.posts.push(post.clone());
        // Published under the lock, so a stream sees the board in post order —
        // the ledger publishes its transitions the same way.
        self.publish_board("post", Some(&post), author, None);
        Ok(post)
    }

    /// Records the owner's post on the board. Like [`Social::post`], past the ceiling the oldest post
    /// is dropped; the post is never refused for room. Whether it is delivered to the agents it
    /// mentions is the caller's to decide (see [`Social::paused_for`]).
    pub fn post_as_owner(&self, post: CheckedOwnerPost) -> PostView {
        let mut inner = self.lock();
        drop_oldest(&mut inner.posts, MAX_POSTS - 1, None, |_| 0);
        inner.posts_seq += 1;
        let view = PostView {
            id: format!("p{}", inner.posts_seq),
            agent_cid: 0,
            role: OWNER_ROLE.to_string(),
            subject: post.subject,
            body: post.body,
            task: None,
            posted_by: OWNER_ROLE.to_string(),
            created_at: now_ms(),
            comments: Vec::new(),
            attachments: Vec::new(),
            owner: true,
            project: post.project,
            mentions: post.mentions,
        };
        inner.posts.push(view.clone());
        self.events.publish(Event::Board {
            kind: "post",
            post: serde_json::to_string(&view).ok(),
            agent_cid: 0,
            role: OWNER_ROLE.to_string(),
            read_up_to: None,
        });
        view
    }

    /// Every post `keep` accepts, oldest first: a whole thread, for an export.
    pub fn posts_matching(&self, keep: impl Fn(&PostView) -> bool) -> Vec<PostView> {
        self.lock()
            .posts
            .iter()
            .filter(|post| keep(post))
            .cloned()
            .collect()
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
            return Err(SocialError::Invalid(
                "a comment must not be empty".to_string(),
            ));
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

    /// Sets the files shown with a post (`p<seq>`) or a comment (`c<seq>` on some post's thread).
    /// `on` picks which: "post" or "comment".
    pub fn attach(&self, on: &str, id: &str, files: &[String]) -> Result<(), SocialError> {
        let mut inner = self.lock();
        let slot = match on {
            "post" => inner
                .posts
                .iter_mut()
                .find(|p| p.id == id)
                .map(|p| &mut p.attachments),
            "comment" => inner
                .posts
                .iter_mut()
                .flat_map(|p| p.comments.iter_mut())
                .find(|c| c.id == id)
                .map(|c| &mut c.attachments),
            _ => {
                return Err(SocialError::Invalid(format!(
                    "cannot attach files to a {on}"
                )));
            }
        };
        let slot = slot.ok_or_else(|| SocialError::Unknown(format!("no {on} {id}")))?;
        *slot = files.to_vec();
        // A board view refetches on any board frame, so the files show without a reload.
        self.events.publish(Event::Board {
            kind: "attach",
            post: None,
            agent_cid: 0,
            role: String::new(),
            read_up_to: None,
        });
        Ok(())
    }

    /// Advances `agent`'s read cursor to `up_to`, or says why it will not.
    ///
    /// The rules, each with its own refusal: `up_to` must name a post
    /// ([`SocialError::Invalid`]); it must be at or before the newest post on
    /// the board, and at or after the agent's own cursor — a cursor is
    /// monotonic, so it never rolls back ([`SocialError::Conflict`]). Replaying
    /// the cursor it already sits at is **idempotent**: the answer is the same
    /// and nothing is published, because nothing changed.
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

    /// Drops the oldest posts past the owner's retention: the count cap first, then anything older
    /// than the age limit. Ids are never reused, so a dropped row's number is simply gone. Returns
    /// what it removed.
    ///
    /// Unread counts are derived from what remains, so they follow the prune without further work.
    pub fn prune(&self, policy: Prune) -> Pruned {
        let now = now_ms();
        let mut inner = self.lock();
        let posts = drop_oldest(
            &mut inner.posts,
            policy.posts_max,
            policy
                .posts_older_than_ms
                .map(|age| now.saturating_sub(age)),
            |post| post.created_at,
        );
        Pruned { posts }
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

/// The post number an id names, or `None` when the id is not `p<seq>`.
fn post_seq(id: &str) -> Option<u64> {
    id.strip_prefix('p')?.parse().ok()
}

/// A count as a `usize`: the record's own sizes are far inside it, so this can
/// only saturate on a bound the store itself refuses to reach.
fn to_usize(count: u64) -> usize {
    usize::try_from(count).unwrap_or(usize::MAX)
}

impl Social {
    /// The board's rows, as they are now (the saver writes what changed): the posts with their comments and
    /// attachments, the read cursors and the two counters. The event bus is not part of them.
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        RowSet::one(def(), &*self.lock())
    }

    /// Replaces the board with what plaza.db holds (at start). The stored board must hold to the invariants a live
    /// operation keeps; a board that does not is refused and nothing changes, and a load publishes nothing.
    pub fn load(&self, conn: &Connection) -> Result<(), StoreError> {
        let stored = apz_store::table::load_one::<SocialInner>(conn, def())?.unwrap_or_default();
        check_snapshot(&stored).map_err(StoreError::invalid)?;
        *self.lock() = stored;
        Ok(())
    }
}

/// The board's tables: `social` (the counters), `social_post` with its comments, and the read cursors.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(SocialInner)
}

/// Refuses a stored board no sequence of live operations could have produced: the
/// board's ids contiguous up to its counter, comment ids unique and within
/// their counter, every comment filed under the
/// post it names, and no cursor past the board (its unread arithmetic would
/// underflow).
fn check_snapshot(state: &SocialInner) -> Result<(), String> {
    let seq_of =
        |id: &str, prefix: char| id.strip_prefix(prefix).and_then(|n| n.parse::<u64>().ok());
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
            return Err(format!(
                "post {} is out of place at position {}",
                post.id,
                index + 1
            ));
        }
        if post.comments.len() > Social::MAX_COMMENTS {
            return Err(format!(
                "post {} holds more than {} comments",
                post.id,
                Social::MAX_COMMENTS
            ));
        }
        for comment in &post.comments {
            let seq = seq_of(&comment.id, 'c')
                .ok_or_else(|| format!("comment id {:?} is malformed", comment.id))?;
            if seq == 0 || seq > state.comment_seq {
                return Err(format!(
                    "comment {} is past the comment counter {}",
                    comment.id, state.comment_seq
                ));
            }
            if !comment_seqs.insert(seq) {
                return Err(format!("comment id {} appears twice", comment.id));
            }
            if comment.post != post.id {
                return Err(format!(
                    "comment {} is filed under {} but names {}",
                    comment.id, post.id, comment.post
                ));
            }
        }
    }
    if let Some((cid, cursor)) = state
        .cursors
        .iter()
        .find(|(_, cursor)| **cursor > state.posts_seq)
    {
        return Err(format!("agent {cid}'s cursor {cursor} is past the board"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// The board comes back from its tables whole, and publishes nothing on the way; a stored board that breaks the
    /// board's invariants is refused, and a refusal changes nothing.
    #[test]
    fn the_board_comes_back_from_its_tables_and_refuses_a_broken_one() {
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
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &social.rows().unwrap()).unwrap();

        let (restored, notes) = lab();
        restored.load(&conn).expect("the board loads");
        assert!(notes.try_recv().is_err(), "a load publishes nothing");
        assert_eq!(
            serde_json::to_string(&restored.board(&[], 50)).unwrap(),
            serde_json::to_string(&social.board(&[], 50)).unwrap()
        );
        assert_eq!(
            restored.rows().unwrap(),
            social.rows().unwrap(),
            "the rows round-trip"
        );

        let (fresh, _notes) = lab();
        conn.execute("UPDATE \"social\" SET \"posts_seq\" = 1", [])
            .unwrap();
        assert!(fresh.load(&conn).is_err(), "two posts under counter 1");
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
            social.comment(
                "p1",
                None,
                "owner",
                &"x".repeat(Social::MAX_COMMENT_BODY + 1)
            ),
            Err(SocialError::Invalid(_))
        ));
    }

    #[test]
    fn the_board_carries_each_posts_thread_oldest_first() {
        let (social, _notes) = lab();
        let auth = agent(100, "builder");
        posted(&social, &auth, "first");
        posted(&social, &auth, "second");
        social
            .comment("p1", Some(101), "scout", "one")
            .expect("a comment");
        social
            .comment("p2", None, "owner", "two")
            .expect("a comment");
        social
            .comment("p1", None, "owner", "three")
            .expect("a comment");
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
        assert_eq!(
            thread[Social::MAX_COMMENTS - 1].body,
            format!("comment {}", Social::MAX_COMMENTS)
        );
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
            board
                .posts
                .iter()
                .map(|post| post.id.as_str())
                .collect::<Vec<_>>(),
            ["p3", "p2", "p1"]
        );

        // The window is the newest posts, and says it cut the rest.
        let window = social.board(&[100], 2);
        assert_eq!(
            window
                .posts
                .iter()
                .map(|post| post.id.as_str())
                .collect::<Vec<_>>(),
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
                UnreadView {
                    agent_cid: 100,
                    count: 3
                },
                UnreadView {
                    agent_cid: 101,
                    count: 3
                },
            ],
            "ascending CID order, one row each"
        );

        let read = social
            .read(&librarian, "p2")
            .expect("a cursor on the board");
        assert_eq!(read.unread, 1);
        assert_eq!(read.agent_cid, 101);
        assert_eq!(read.up_to, "p2");
        assert_eq!(
            social.board(&[100, 101], BOARD_LIMIT).unread,
            vec![
                UnreadView {
                    agent_cid: 100,
                    count: 3
                },
                UnreadView {
                    agent_cid: 101,
                    count: 1
                },
            ]
        );
        // The count follows the cursor, not the reader: a newer post is unread
        // again, and the agent that never read stays where it was.
        posted(&social, &builder, "post 4");
        assert_eq!(
            social.board(&[100, 101], BOARD_LIMIT).unread,
            vec![
                UnreadView {
                    agent_cid: 100,
                    count: 4
                },
                UnreadView {
                    agent_cid: 101,
                    count: 2
                },
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
        assert_eq!(
            social.read(&librarian, "p2").expect("on the board").unread,
            0
        );
        assert_eq!(
            social.read(&librarian, "p1").unwrap_err(),
            SocialError::Conflict("up_to p1 is behind the agent's read cursor p2".to_string())
        );
        // Idempotent: the cursor it already sits at is the same answer, and
        // nothing is published because nothing changed.
        while notes.try_recv().is_ok() {}
        assert_eq!(
            social
                .read(&librarian, "p2")
                .expect("the same cursor")
                .unread,
            0
        );
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
                .map(|seq| PostView {
                    id: format!("p{seq}"),
                    ..template.clone()
                })
                .collect();
            inner.posts_seq = MAX_POSTS as u64;
        }
        let next = posted(&social, &auth, "one more");
        assert_eq!(
            next.id,
            format!("p{}", MAX_POSTS + 1),
            "the id keeps counting"
        );
        let inner = social.lock();
        assert_eq!(inner.posts.len(), MAX_POSTS, "still at the ceiling");
        assert_eq!(
            inner.posts.first().map(|p| p.id.as_str()),
            Some("p2"),
            "the oldest went"
        );
        assert_eq!(
            inner.posts.last().map(|p| p.id.as_str()),
            Some(next.id.as_str())
        );
    }

    /// A retention that keeps `posts_max` posts, with no age limit.
    fn keep(posts_max: usize) -> Prune {
        Prune {
            posts_max,
            posts_older_than_ms: None,
        }
    }

    #[test]
    fn prune_drops_posts_by_count_and_by_age_and_keeps_ids_monotonic() {
        {
            let (social, _notes) = lab();
            let builder = agent(100, "builder");
            for index in 1..=5 {
                posted(&social, &builder, &format!("post {index}"));
            }
            assert_eq!(social.prune(keep(3)), Pruned { posts: 2 });
            let view = social.board(&[], 10);
            assert_eq!(view.total, 3);
            assert_eq!(
                view.posts
                    .iter()
                    .map(|row| row.id.as_str())
                    .collect::<Vec<_>>(),
                ["p5", "p4", "p3"]
            );
            // Pruning again with nothing to drop changes nothing.
            assert_eq!(social.prune(keep(3)), Pruned::default());
            // A fresh post takes the next number, never one a pruned row held.
            assert_eq!(posted(&social, &builder, "after").id, "p6");
        }
        {
            let (social, _notes) = lab();
            let builder = agent(100, "builder");
            let posted_old = posted(&social, &builder, "old post");
            posted(&social, &builder, "fresh post");
            {
                // The first post is a day old.
                let mut inner = social.lock();
                inner.posts[0].created_at = now_ms() - 86_400_000;
                assert_eq!(inner.posts[0].id, posted_old.id);
            }
            let hour = Prune {
                posts_max: 100,
                posts_older_than_ms: Some(3_600_000),
            };
            assert_eq!(social.prune(hour), Pruned { posts: 1 });
            assert_eq!(
                social
                    .board(&[], 10)
                    .posts
                    .iter()
                    .map(|p| p.subject.as_str())
                    .collect::<Vec<_>>(),
                ["fresh post"]
            );
        }
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
        assert_eq!(social.prune(keep(2)).posts, 3);
        // p1 to p3 are gone and p4, p5 are held. The reader's cursor sits at p1,
        // so both held posts are unread: the count is clamped to what remains,
        // not the three posts the raw arithmetic would name.
        let board = social.board(&[100, 101], 10);
        assert_eq!(board.total, 2);
        assert_eq!(
            board
                .unread
                .iter()
                .find(|u| u.agent_cid == 101)
                .map(|u| u.count),
            Some(2)
        );
        // An author with no cursor also sees only the held posts as unread.
        assert_eq!(
            board
                .unread
                .iter()
                .find(|u| u.agent_cid == 100)
                .map(|u| u.count),
            Some(2)
        );
        let next = social.read(&reader, "p5").expect("p5 is readable");
        assert_eq!(next.unread, 0);
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
            owner: false,
            project: None,
            mentions: Vec::new(),
        };
        let encoded = serde_json::to_string(&post).expect("a serializable post");
        assert_eq!(
            encoded,
            r#"{"id":"p1","agent_cid":100,"role":"builder","subject":"a \"quoted\" subject\nwith a newline","body":"the body","task":null,"posted_by":"driver","created_at":7,"comments":[],"attachments":[],"owner":false,"project":null,"mentions":[]}"#
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

#[cfg(test)]
mod owner_post_tests {
    use super::*;

    fn request(
        subject: Option<&str>,
        body: &str,
        project: Option<&str>,
        mentions: &[&str],
    ) -> OwnerPostRequest {
        OwnerPostRequest {
            subject: subject.map(str::to_string),
            body: body.to_string(),
            project: project.map(str::to_string),
            mentions: mentions.iter().map(|m| m.to_string()).collect(),
        }
    }

    #[test]
    fn an_owner_post_is_checked_and_its_mentions_normalized() {
        let checked = request(
            None,
            "  ship the release  ",
            Some("pr1"),
            &["@builder", "BUILDER", " all "],
        )
        .checked()
        .expect("a valid owner post");
        assert_eq!(
            checked.subject, "ship the release",
            "the subject defaults to the body's start, trimmed"
        );
        assert_eq!(checked.body, "  ship the release  ");
        assert_eq!(checked.project.as_deref(), Some("pr1"));
        assert_eq!(
            checked.mentions,
            vec!["builder".to_string(), "all".to_string()]
        );

        assert!(
            request(None, "   ", None, &[]).checked().is_err(),
            "an empty body is refused"
        );
        assert!(
            request(Some("  "), "x", None, &[]).checked().is_ok(),
            "a blank subject takes the default"
        );
        assert!(
            request(None, "x", None, &["@"]).checked().is_err(),
            "an empty mention is refused"
        );
        let many: Vec<String> = (0..=MAX_MENTIONS).map(|i| format!("a{i}")).collect();
        let refused = OwnerPostRequest {
            mentions: many,
            ..request(None, "x", None, &[])
        };
        assert!(
            refused.checked().is_err(),
            "more than the cap of mentions is refused"
        );
        let long = "x".repeat(MAX_MENTION + 1);
        assert!(request(None, "x", None, &[&long]).checked().is_err());
    }

    #[test]
    fn an_owner_post_is_stored_as_the_owner_and_published() {
        let events = EventBus::new();
        let notes = events.subscribe();
        let social = Social::new(events);
        let checked = request(Some("hello"), "all hands", Some("pr2"), &["all"])
            .checked()
            .unwrap();
        let view = social.post_as_owner(checked);
        assert_eq!(view.id, "p1");
        assert_eq!(
            (view.agent_cid, view.role.as_str(), view.owner),
            (0, OWNER_ROLE, true)
        );
        assert_eq!(view.project.as_deref(), Some("pr2"));
        assert_eq!(view.mentions, vec!["all".to_string()]);
        let frame = notes.try_recv().expect("the post is published");
        assert!(matches!(frame, Event::Board { kind: "post", .. }));
        assert_eq!(social.board(&[], 10).posts, vec![view]);
    }

    #[test]
    fn the_board_can_be_filtered_and_the_export_is_the_whole_thread_oldest_first() {
        let social = Social::new(EventBus::new());
        for (subject, project) in [("one", Some("pr1")), ("two", None), ("three", Some("pr1"))] {
            let checked = request(Some(subject), "body", project, &[])
                .checked()
                .unwrap();
            social.post_as_owner(checked);
        }
        let kept = social.board_for(&[], 10, |post| post.project.as_deref() == Some("pr1"));
        assert_eq!(kept.total, 2);
        assert_eq!(
            kept.posts
                .iter()
                .map(|p| p.subject.as_str())
                .collect::<Vec<_>>(),
            vec!["three", "one"]
        );
        let thread = social.posts_matching(|post| post.project.as_deref() == Some("pr1"));
        assert_eq!(
            thread
                .iter()
                .map(|p| p.subject.as_str())
                .collect::<Vec<_>>(),
            vec!["one", "three"]
        );
        assert_eq!(social.board(&[], 10).total, 3);
    }
}
