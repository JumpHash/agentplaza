//! The one small HTTP/1.1 every AgentPlaza daemon speaks.
//!
//! * [`Server`] — a threaded loopback server: one thread per connection, one
//!   request per connection (`Connection: close`), bounded heads and bodies,
//!   read/write timeouts, no chunked bodies.
//! * [`Router`] — method + path-pattern dispatch (`/v1/agents/{cid}/tell`).
//! * [`Response::sse`] / [`Sse`] — a server-sent event stream.
//! * [`client`] — a blocking client for `127.0.0.1:port`, including an SSE
//!   reader so one daemon can relay another's event stream.
//!
//! Every daemon binds loopback only, and the server refuses (403) a request
//! whose `Host` names anything but `127.0.0.1` or `localhost`: without that, a
//! page on any hostname that resolves to 127.0.0.1 (DNS rebinding) could post
//! simple requests here. A request without `Host` (a raw probe) is served.

#![forbid(unsafe_code)]

pub mod client;
mod message;
mod router;
mod server;
pub mod ws;

pub use message::{Body, JSON, Limits, Request, Response, Sse, host_is_loopback};
pub use router::{Params, Router};
pub use server::{Handle, RELAY_PREFACE, Server};
