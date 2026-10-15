//! The ACP v1 wire types, re-exported from the pinned `agent-client-protocol-schema` crate. Only the
//! types come from it: the framing, ids and threads are [`crate::rpc`]'s.
//!
//! Method names are in `AGENT_METHOD_NAMES` and `CLIENT_METHOD_NAMES`. The types are `non_exhaustive`,
//! so build them with their `new` constructors and builder methods, or decode them from JSON.

pub use agent_client_protocol_schema::ProtocolVersion;
pub use agent_client_protocol_schema::v1::*;
