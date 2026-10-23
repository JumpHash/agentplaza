//! The sources of the hub's cached answers, by name (PLAN 30.4). Each is a key of the hub's `apz_cache::Generations`
//! (`App::generations`): the store that owns the source bumps its key under the write's own lock, so a cache built before
//! the write never stores over it. A source is `role:<id>` or `project:<id>` for one record, or a name for a whole store.
//! The skills' keys (`skill:<id>`, `skills`) are in `skills`.

use apz_cache::Generations;

/// The owner's settings (`settings.rs`).
pub const SETTINGS: &str = "settings";
/// The connectors: their settings, their proposals and the catalog of approved ones (`connectors.rs`).
pub const CONNECTORS: &str = "connectors";
/// The vault: which secrets exist and whether it is sealed (`vault/mod.rs`); the connectors' fields read it.
pub const VAULT: &str = "vault";
/// The MCP servers and their tool listings (`mcp/config.rs`).
pub const MCP: &str = "mcp";
/// The engine instances the owner added (`engine_instances.rs`).
pub const ENGINES: &str = "engines";

/// A custom role (`roles_custom.rs`).
pub fn role(id: &str) -> String {
    format!("role:{id}")
}

/// A project: its record, its maintenance mode and its environments (`projects.rs`, `environments.rs`).
pub fn project(id: &str) -> String {
    format!("project:{id}")
}

/// Bumps the source `key` (a key of the hub's generations).
pub fn bump(gens: &Generations<String>, key: &str) {
    gens.bump(&key.to_string());
}
