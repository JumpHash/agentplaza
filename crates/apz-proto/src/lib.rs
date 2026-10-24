//! AgentPlaza wire types, shared by every process that speaks them.
//!
//! * [`chat`] — the provider-neutral chat model. The guest harness builds
//!   [`chat::ChatRequest`]s, `apid`'s gateway translates them to a provider's
//!   API, and the answer comes back as a [`chat::ChatResponse`].
//! * [`wire`] — the guest↔hub line protocol (one JSON object per line over
//!   vsock): v1 demo session, v2 keepalive session, v3 agent loop.
//! * [`line`] — the newline framing `wire` travels in, over any `Read`/`Write`.
//!
//! `chat` and `wire` do no I/O; `line` only reads and writes the stream it is
//! given.

#![forbid(unsafe_code)]

pub mod chat;
pub mod hooks;
pub mod line;
pub mod wire;
