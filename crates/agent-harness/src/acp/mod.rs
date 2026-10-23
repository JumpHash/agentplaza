//! Coding engines over ACP (the Agent Client Protocol): the harness is the ACP client, the engine's
//! adapter the ACP agent on a child process's stdio. See [`link`] for how an engine session reaches
//! the hub.
//!
//! - [`catalog`]: the engines, their pinned adapters and how each starts; [`instance`]: the engine a call runs,
//!   the catalogue's or the owner's spec.
//! - [`run`]: `code_engine` (input, install, sessions store, summary).
//! - [`session`]: the process, the handshake, one prompt turn, the engine's client calls.
//! - [`gate`]: what the hub is asked before an engine acts; [`stream`]: the update stream.
//! - [`mcp`]: the plaza's own tools, offered to the engine through an MCP server of this binary.
//! - [`fs`], [`terminal`]: the client methods, confined to the workspace.
//! - [`layout`]: who owns the engine's folders (the engine user writes its job folder and its home).

pub mod adapter;
pub mod catalog;
pub mod fs;
pub mod gate;
pub mod hub;
pub mod instance;
pub mod layout;
pub mod link;
pub mod mcp;
pub mod run;
pub mod session;
pub mod shim;
pub mod stream;
pub mod terminal;
