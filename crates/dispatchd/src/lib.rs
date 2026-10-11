//! `dispatchd` — the fleet dispatcher: the loop that closes the plaza's
//! autonomy loop.
//!
//! ```text
//! dispatchd [--noded URL] [--hub URL] [--once] [--interval-secs N]
//! ```
//!
//! The ledger (`noded`, the node daemon) holds tasks and knows which agents
//! are connected; the hub (`vsockd`'s admin surface) is the one way to *make*
//! an agent do something. Between them sits a human, unless something drives
//! the loop. This is that something:
//!
//! 1. **read** the ledger (`GET /api/v0/tasks`) and the connected agents
//!    (`GET /api/v0/hub`, the hub's registry proxied by the node);
//! 2. **pick** an `open` task whose `role` matches a CONNECTED agent, and
//!    **claim** it for that agent (`POST /api/v0/tasks/{id}/claim`);
//! 3. **drive** the agent through the workflow its role family scripts
//!    ([`workflow`]) — real tool calls (`write_file`, `read_file`, `list`, …)
//!    sent into the agent's own session through the hub's `POST /tell`, each
//!    one answered by the agent's own `tool_result`;
//! 4. **finish** the task (`POST /api/v0/tasks/{id}/finish`) with the note
//!    naming the artifacts the agent's workspace now holds, or with the
//!    reason it could not.
//!
//! Every fifth pass is also a **social pass** ([`crate::social`]), run first,
//! over the same snapshot and before any claim: one idle connected agent reads
//! the plaza's board (`POST /api/v0/board/read`) and, when a post names an open
//! task its role can serve, posts a templated interest note (`POST
//! /api/v0/board`). It is bounded (one agent, at most three loopback requests)
//! and every failure in it is logged and swallowed, so the claim loop is never
//! blocked by the board.
//!
//! The **talk step** is the social behaviour's other half, and it runs not on a
//! cadence but on the event: when the dispatcher finishes a task it drove — the
//! confirmed `done` or `failed` transition — the agent that did the work says
//! so to the task's awarder, when the board names one (the author of the task's
//! hiring note), through `POST /api/v0/agents/{cid}/say`. The line is a
//! template over the task's own facts (`done: <task> — <first artifact or "no
//! artifact note">`, `failed: <task> — <the dispatcher's failure note>`), it is
//! bounded (at most two loopback calls per terminal transition, at most one
//! `say` per task) and every failure in it is logged and swallowed.
//!
//! What it deliberately does *not* do: invent work for a role whose agent is
//! not connected (such a task stays `open` and is logged as skipped), guess at
//! a tool's answer (an agent that does not answer is reported as such), drive
//! an agent whose CID is no longer connected as the task's role (a guest CID is
//! reused when a VM is replaced, so every drive re-verifies the agent against
//! the register), or drive two tool calls at once (a pass is strictly
//! sequential — one call in flight, anywhere). Nor does the social pass speak
//! for an agent with a claim in hand, invent a note's words, or duplicate the
//! award's own hiring note and contact links — those are the node's. (The talk
//! step speaks, but only the task's own facts, and only to the awarder the
//! board names.)
//!
//! It holds no state that matters: the retry counter is in memory and only
//! bounds how many passes a failing task is retried for, and a claim left
//! behind by a restart is adopted on a later pass whenever the ledger's own
//! `agent_cid` still names a connected agent of the task's role ([`dispatch`]).
//! Everything durable — the task's state, its assignee, its note — lives in the
//! ledger, which is the record of what happened.
//!
//! This crate is safe Rust only (`forbid(unsafe_code)`), uses no threads and
//! no unsafe, and depends on nothing beyond `serde`/`serde_json`: the HTTP it
//! speaks is the smallest HTTP/1.1 the two loopback daemons serve
//! ([`http`]) — the same pattern `noded`'s own hub client follows.

#![forbid(unsafe_code)]

pub mod dispatch;
pub mod http;
pub mod hub;
pub mod noded;
pub mod roles;
pub mod social;
#[cfg(test)]
mod testing;
pub mod views;
pub mod workflow;

pub use dispatch::{Dispatcher, Ledger, Logger, Operator, Reporter, Social, StdoutLog};
pub use hub::Hub;
pub use noded::Noded;
pub use roles::Manifest;
pub use social::{DRIVER, Note, SOCIAL_EVERY, SocialPass, SocialReport};
pub use views::{Agent, Board, Contact, NewPost, Post, ReadAck, Task, Tell};
pub use workflow::{DriveError, Driver};
