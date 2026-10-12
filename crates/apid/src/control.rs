//! The plaza's master switch. STOP ALL cancels every agent's running work and holds the plaza:
//! nothing new is dispatched (queued work, loops, outreach, task assignment) until the owner
//! resumes. The agents' VMs stay up and their queues are kept. Saved in Postgres ("control").
//!
//! GET  /v1/plaza/state   → {paused, since_ms, by, reason, cancelled}
//! POST /v1/plaza/stop    body {reason?} → the state (cancels running work, pauses)
//! POST /v1/plaza/resume  → the state

use std::sync::Mutex;

use apz_http::{Params, Request, Response, Router};
use serde::{Deserialize, Serialize};

use crate::state::App;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlazaState {
    pub paused: bool,
    pub since_ms: u64,
    pub by: String,
    pub reason: String,
    /// How many running work items the last STOP ALL cancelled.
    pub cancelled: u32,
}

#[derive(Default)]
pub struct Control {
    state: Mutex<PlazaState>,
}

impl Control {
    pub fn state(&self) -> PlazaState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }
    /// True while the plaza is held: the scheduler, loops, outreach and task assignment check it.
    pub fn paused(&self) -> bool {
        self.state().paused
    }
    pub fn snapshot(&self) -> String {
        serde_json::to_string(&self.state()).unwrap_or_default()
    }
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let state: PlazaState = serde_json::from_str(json).map_err(|e| e.to_string())?;
        *self.state.lock().map_err(|e| e.to_string())? = state;
        Ok(())
    }
}

/// Cancels every agent's running work and holds the plaza.
pub fn stop_all(_app: &App, _by: &str, _reason: &str) -> PlazaState {
    PlazaState::default() // contract: B1
}

/// Releases the hold; queued work dispatches again.
pub fn resume(_app: &App, _by: &str) -> PlazaState {
    PlazaState::default() // contract: B1
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/plaza/state", |app, _, _| Response::json(200, &app.control.state()))
        .post("/v1/plaza/stop", stop_route)
        .post("/v1/plaza/resume", resume_route)
}

fn stop_route(_app: &App, _req: &Request, _: &Params<'_>) -> Response {
    Response::error(501, "contract: B1")
}

fn resume_route(_app: &App, _req: &Request, _: &Params<'_>) -> Response {
    Response::error(501, "contract: B1")
}
