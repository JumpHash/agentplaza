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
    pub fn set(&self, state: PlazaState) {
        if let Ok(mut guard) = self.state.lock() {
            *guard = state;
        }
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

#[derive(Deserialize)]
struct StopBody {
    reason: Option<String>,
}

/// Cancels every agent's running work and holds the plaza.
pub fn stop_all(app: &App, by: &str, reason: &str) -> PlazaState {
    // Hold first, so work that the cancels finish cannot dispatch the next item.
    let mut state = PlazaState {
        paused: true,
        since_ms: apz_plaza::now_ms(),
        by: by.into(),
        reason: reason.into(),
        cancelled: 0,
    };
    app.control.set(state.clone());
    // Every E2B sandbox ends too; the hold just set refuses new ones (see e2b).
    crate::e2b::stop_all(app);
    for agent in app.agents.list() {
        if app.work.current(agent.cid).is_some()
            && app.work.cancel(app, agent.cid, "STOP ALL by the owner")
        {
            state.cancelled += 1;
        }
    }
    app.control.set(state.clone());
    // Every workstation freezes: input stops, pixel sessions end, shells and runs stop (docs/design/workstations.md).
    crate::stations::freeze_all(app, true, by);
    app.bus.publish("plaza", &state);
    state
}

/// Releases the hold; queued work dispatches again, and the workstations thaw and grant their queues.
pub fn resume(app: &App, by: &str) -> PlazaState {
    let previous = app.control.state();
    let state = PlazaState {
        paused: false,
        since_ms: apz_plaza::now_ms(),
        by: by.into(),
        reason: String::new(),
        cancelled: previous.cancelled,
    };
    app.control.set(state.clone());
    crate::stations::freeze_all(app, false, by);
    app.bus.publish("plaza", &state);
    app.work.dispatch_idle(app);
    state
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/plaza/state", |app, _, _| {
            Response::json(200, &app.control.state())
        })
        .post("/v1/plaza/stop", stop_route)
        .post("/v1/plaza/resume", resume_route)
}

/// Body `{reason?}` is optional; the owner acts.
fn stop_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let reason = if req.body.is_empty() {
        String::new()
    } else {
        match req.json::<StopBody>() {
            Ok(body) => body.reason.unwrap_or_default(),
            Err(e) => return Response::error(400, &format!("bad body: {e}")),
        }
    };
    Response::json(200, &stop_all(app, "owner", &reason))
}

fn resume_route(app: &App, _req: &Request, _: &Params<'_>) -> Response {
    Response::json(200, &resume(app, "owner"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hold_survives_snapshot_and_restore() {
        let control = Control::default();
        assert!(!control.paused());
        control.set(PlazaState {
            paused: true,
            since_ms: 5,
            by: "owner".into(),
            reason: "bad loop".into(),
            cancelled: 2,
        });
        let json = control.snapshot();

        let restored = Control::default();
        restored.restore(&json).unwrap();
        assert!(restored.paused());
        assert_eq!(restored.state().cancelled, 2);
        assert_eq!(restored.state().reason, "bad loop");

        assert!(restored.restore("not json").is_err());
        assert!(restored.paused(), "a refused restore changes nothing");
    }
}
