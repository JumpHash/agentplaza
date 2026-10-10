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
//! What it deliberately does *not* do: invent work for a role whose agent is
//! not connected (such a task stays `open` and is logged as skipped), guess at
//! a tool's answer (an agent that does not answer is reported as such), or
//! drive two tool calls at once (a pass is strictly sequential — one call in
//! flight, anywhere).
//!
//! It holds no state that matters: the retry counter is in memory and only
//! bounds how many passes a failing task is retried for. Everything durable —
//! the task's state, its assignee, its note — lives in the ledger, which is
//! the record of what happened.
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
#[cfg(test)]
mod testing;
pub mod views;
pub mod workflow;

pub use dispatch::{Dispatcher, Ledger, Logger, Operator, Reporter, StdoutLog};
pub use hub::Hub;
pub use noded::Noded;
pub use roles::Manifest;
pub use views::{Agent, Task, Tell};
pub use workflow::{DriveError, Driver};
