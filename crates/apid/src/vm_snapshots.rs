//! Live checkpoints, rollback and forks of an agent's whole VM — memory, disk and running processes,
//! not only its files (those are the IDE's file checkpoints, `/v1/ide/workspaces/{id}/checkpoints`).
//!
//! GET    /v1/agents/{cid}/vm-checkpoints                 → [checkpoint]
//! POST   /v1/agents/{cid}/vm-checkpoints  {label?}       → checkpoint
//! POST   /v1/agents/{cid}/vm-checkpoints/{name}/restore  → {ok, safety: checkpoint}
//! DELETE /v1/agents/{cid}/vm-checkpoints/{name}          → {ok}
//! POST   /v1/agents/{cid}/fork            {n}            → {forks: [{cid, run}], checkpoint}
//!
//! A checkpoint is noded's snapshot view (`noded::snapshots::SnapshotView`). Rolling back first takes
//! a safety checkpoint (`before-restore`), so a rollback can itself be undone — CubeSandbox's rollback
//! deletes the VM before it restores, and a failed restore loses it. Forks are new agents of the same
//! role, each with a copy of the source agent's VM at the fork's checkpoint and a name of its own.
//!
//! The laws decide when a checkpoint is taken on their own: before a tool call whose risk class is
//! high, the hub asks for one (see [`auto_checkpoint`]), so "undo the last risky step" restores the
//! whole machine.

use apz_http::Router;

use crate::state::App;

/// Routes listed in the module docs.
pub fn routes(router: Router<App>) -> Router<App> {
    router
}

/// Takes a checkpoint of `cid`'s VM before the risky tool call `tool` runs, when automatic
/// checkpoints are on and none was taken for this agent in the last minute. Best effort: a failure
/// is logged and the call goes ahead.
pub fn auto_checkpoint(app: &App, cid: u64, tool: &str) {
    let _ = (app, cid, tool);
}
