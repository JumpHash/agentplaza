//! The hub as an ACP agent: editors (Zed, JetBrains IDEs, Neovim, any Agent Client Protocol client)
//! prompt a plaza agent, watch its work stream in, and answer its approvals, through
//! `agentplaza acp` on the owner's machine.
//!
//! `GET /v1/acp` upgrades to a WebSocket (subprotocol [`PROTOCOL`]); each text message is one
//! JSON-RPC 2.0 message, and the connection is an `apz_acp::rpc::Peer` with the hub as the agent.
//! It is an owner route (the hub's front door, `hub_auth`: loopback or the owner token), off until
//! the owner turns on `acp_enabled` in the settings. Each editor session is a thread of its own on
//! the agent (`acp-<id>`), so it never mixes with the owner's console chat.
//!
//! - [`store`]: the sessions, kept in Postgres with the rest of the plaza state.
//! - [`agent`]: the ACP methods, over the [`agent::Hub`] the plaza gives them ([`app_hub`]).
//! - [`feed`]: a session's work as ACP `session/update`s, live from the bus or replayed.
//!
//! An agent never reaches the owner's machine: the editor's MCP servers are not connected, the
//! hub never calls the editor's `fs/*` or `terminal/*`, and the agent works in its own VM.

pub mod agent;
pub mod app_hub;
pub mod feed;
pub mod store;

use apz_http::Router;

use crate::state::App;

/// The WebSocket subprotocol of `/v1/acp`.
pub const PROTOCOL: &str = "apz.acp.v1";

/// `GET /v1/acp` (the WebSocket) and `GET /v1/acp/sessions` (the console's list).
pub fn routes(router: Router<App>) -> Router<App> {
    router
}
