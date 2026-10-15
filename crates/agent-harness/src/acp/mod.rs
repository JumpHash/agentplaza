//! Coding engines over ACP (the Agent Client Protocol): the harness is the ACP client, the engine's
//! adapter the ACP agent on a child process's stdio. See [`link`] for how an engine session reaches
//! the hub.
//!
//! - [`catalog`]: the engines, their pinned adapters and how each starts.
//! - [`run`]: `code_engine` (input, install, sessions store, summary).
//! - [`session`]: the process, the handshake, one prompt turn, the engine's client calls.
//! - [`gate`]: what the hub is asked before an engine acts; [`stream`]: the update stream.
//! - [`fs`], [`terminal`]: the client methods, confined to the workspace.

pub mod catalog;
pub mod fs;
pub mod gate;
pub mod hub;
pub mod link;
pub mod run;
pub mod session;
pub mod stream;
pub mod terminal;
