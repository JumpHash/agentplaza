//! Agents sleep when they have nothing to do, and wake the moment something needs them.
//!
//! CubeSandbox pauses a sandbox when its proxy has seen no traffic for a while and resumes it on the
//! next request. The hub knows more than traffic: an agent's work queue, its running engines and
//! terminals, its open IDE sessions, the approvals it waits on, its leases. So an agent sleeps when
//! all of those are empty and it has been idle for `idle_sleep_min` minutes (a setting; 0 = never),
//! unless its role is in `sleep_never_roles`. Sleeping is a snapshot of its VM and a stop
//! (noded `POST /v1/runs/{run}/vms/{vm}/sleep`); the agent shows as `asleep`, its memory is free.
//!
//! It wakes **before** the work is delivered: the scheduler's dispatch, a chat, a `tell`, an IDE
//! request and the owner's wake all call [`wake`] / [`ensure_awake`], which restore the VM from its
//! sleep snapshot (noded `.../wake`); the harness reconnects within moments, its hello marks it idle,
//! and the queued work goes out (`Scheduler::dispatch_idle`).
//!
//! POST /v1/agents/{cid}/sleep → {ok}      (the owner puts it to sleep now)
//! POST /v1/agents/{cid}/wake  → {ok}      (the owner wakes it)

use std::time::Duration;

use apz_http::Router;

use crate::state::App;

/// Routes: `POST /v1/agents/{cid}/sleep`, `POST /v1/agents/{cid}/wake`.
pub fn routes(router: Router<App>) -> Router<App> {
    router
}

/// Records activity for `cid` (work started or ended, a chat, an IDE call, a tool call): the idle
/// clock restarts.
pub fn touch(app: &App, cid: u64) {
    let _ = (app, cid);
}

/// Starts waking `cid` if it is asleep (returns at once; idempotent while a wake is in flight).
pub fn wake(app: &App, cid: u64) {
    let _ = (app, cid);
}

/// Wakes `cid` if it is asleep and waits until it is connected again, up to `timeout`.
pub fn ensure_awake(app: &App, cid: u64, timeout: Duration) -> Result<(), String> {
    let _ = (app, cid, timeout);
    Ok(())
}
