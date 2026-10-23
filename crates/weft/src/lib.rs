//! Weft: AgentPlaza's agent-native version control.
//!
//! One binary: `weft serve` (the history server on the hub), `weft serve --cache` (a stateless object cache on
//! every node), and the `weft` CLI (agent VMs, check VMs, the owner's machine). The design is
//! `docs/design/weft.md`.
//!
//! - [`log`]: the append-only, hash-chained log of signed statements: the truth.
//! - [`blobs`]: file contents and artifacts by BLAKE3 hash.
//! - [`state`]: the state rebuilt by replaying the log, and the rules for who may sign what.
//! - [`store`]: the three kept consistent.
//! - [`lang`], [`packsrc`]: language and format packs, loaded on demand.
//! - [`diff`], [`derived`]: diffs and the data derived from the log: symbols, touches, lineage, change trees.
//! - [`server`], [`wire`], [`client`]: `weft serve` and the protocol between it, node caches and the CLI.
//! - [`query`], [`verify`], [`map`]: queries over changes, verification, the project map.

pub mod blobs;
pub mod cache;
pub mod cli;
pub mod cli_git;
pub mod cli_more;
pub mod client;
pub mod connect;
pub mod derived;
pub mod diff;
pub mod export;
pub mod git;
pub mod gitwire;
pub mod hash;
pub mod import;
pub mod keys;
pub mod landing;
pub mod lang;
pub mod log;
pub mod map;
pub mod merge;
pub mod model;
pub mod packsrc;
pub mod query;
pub mod record;
pub mod release;
pub mod server;
pub mod sshsig;
pub mod state;
pub mod store;
pub mod trace;
pub mod verify;
pub mod wire;
pub mod workdir;
