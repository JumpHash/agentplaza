//! Weft: AgentPlaza's agent-native version control.
//!
//! One binary: `weft serve` (the history server on the hub), `weft serve --cache` (a stateless object cache on
//! every node), and the `weft` CLI (agent VMs, check VMs, the owner's machine). The design is
//! `docs/design/weft.md`; its architecture page is `docs/design/assets/weft/architecture.html`.
//!
//! - [`log`]: the append-only, hash-chained log of signed statements: the truth.
//! - [`blobs`]: file contents and artifacts by BLAKE3 hash.
//! - [`state`]: the state rebuilt by replaying the log, and the rules for who may sign what.
//! - [`store`]: the three kept consistent.

pub mod blobs;
pub mod hash;
pub mod keys;
pub mod log;
pub mod model;
pub mod record;
pub mod state;
pub mod store;
