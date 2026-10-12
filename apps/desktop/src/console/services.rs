//! The local services this machine runs, started and stopped by the console: in local mode the
//! hub (apid), the agent gateway (vsockd) and the node (noded), plus Postgres / Forgejo (Docker
//! containers) and Garage when installed; as a node, apz-link + vsockd + noded.

/// One local service and its state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    /// "running" | "stopped" | "missing" (not installed).
    pub state: String,
    pub detail: String,
}

/// Which set of services a mode runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Hub and one node on this machine, loopback only.
    Local,
    /// A hub other machines join (TLS on, owner token, join codes).
    Hub,
    /// This machine is a node of another hub.
    Node,
}

/// The services of `mode` and their state now.
pub fn status(_mode: Mode) -> Vec<Service> {
    Vec::new() // contract: D2
}

/// Starts what `mode` needs that is not running; returns a line per service.
pub fn start(_mode: Mode) -> Result<Vec<String>, String> {
    Err("contract: D2".into())
}

/// Stops the services `mode` started.
pub fn stop(_mode: Mode) -> Result<Vec<String>, String> {
    Err("contract: D2".into())
}

/// The owner token a local hub wrote for this machine (`<state>/owner-token`), if any.
pub fn local_owner_token() -> Option<String> {
    None // contract: D2
}

/// Joins a hub as a node with a join token (runs `apz-link join`); returns the node id.
pub fn join(_token: &str) -> Result<String, String> {
    Err("contract: D2".into())
}
