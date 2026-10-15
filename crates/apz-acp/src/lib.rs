//! Agent Client Protocol (ACP) support: the protocol's types from the pinned schema crate, blocking
//! line-delimited JSON-RPC over any byte stream, the mapping between plaza and ACP concepts, and the
//! test support (a fake agent binary and a fake client). The harness uses it as an ACP client (engines
//! inside the VM), and the hub as an ACP agent (editors).

#![deny(unsafe_code)]

pub mod compat;
pub mod map;
pub mod rpc;
pub mod testing;
pub mod types;
