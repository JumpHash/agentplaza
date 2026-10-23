//! `mcp-bridge`: the guest end of one MCP server's stdio, run by init in a `plaza.mcp=1` VM. The logic is in
//! `mcp_bridge`; this file only wires the crate's modules together, since `vsockd` has no library target.

// Safe Rust except `sys`, the vsock connect boundary.
#![deny(unsafe_code)]

// The bridge needs one call from `sys` (`vsock_connect`); the rest of that file is for the daemon.
#[allow(dead_code)]
#[path = "../sys.rs"]
mod sys;

#[path = "../mcp_bridge.rs"]
mod mcp_bridge;

fn main() -> std::process::ExitCode {
    mcp_bridge::main()
}
