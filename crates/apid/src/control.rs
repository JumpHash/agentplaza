//! The plaza's master switch. STOP ALL cancels every agent's running work and holds the plaza:
//! nothing new is dispatched (queued work, loops, task assignment) until the owner
//! resumes. The agents' VMs stay up and their queues are kept. Saved in plaza.db (the `control` table).
//!
//! GET  /v1/plaza/state   → {paused, since_ms, by, reason, cancelled}
//! POST /v1/plaza/stop    body {reason?} → the state (cancels running work, pauses)
//! POST /v1/plaza/resume  → the state

use std::sync::Mutex;

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};
use serde::{Deserialize, Serialize};

use crate::state::App;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "control")]
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
    /// True while the plaza is held: the scheduler, loops and task assignment check it.
    pub fn paused(&self) -> bool {
        self.state().paused
    }
    pub fn set(&self, state: PlazaState) {
        if let Ok(mut guard) = self.state.lock() {
            *guard = state;
        }
    }
    /// The state's row, as it is now (the saver writes what changed).
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        RowSet::one(def(), &self.state())
    }
    /// Replaces the state with what plaza.db holds (at start).
    pub fn load(&self, conn: &Connection) -> Result<(), StoreError> {
        let stored = apz_store::table::load_one::<PlazaState>(conn, def())?;
        self.set(stored.unwrap_or_default());
        Ok(())
    }
}

/// The control table: `control`, one row.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(PlazaState)
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
    for cid in app.agents.select(|a| Some(a.cid)) {
        if app.work.current(cid).is_some() && app.work.cancel(app, cid, "STOP ALL by the owner") {
            state.cancelled += 1;
        }
    }
    // Each node's vsockd keeps the egress decisions it allowed for a while, so a stopped agent's next
    // connection would still pass. They go now, on whichever node runs the agent; the paused check in
    // the egress answer refuses the re-ask. A remote node answers through its command queue, so this
    // runs off the owner's request. A node that cannot be reached keeps its decisions until they expire.
    let cids = app.agents.select(|a| Some(a.cid));
    let handle = app.handle();
    std::thread::spawn(move || {
        let Some(app) = handle.upgrade() else {
            return;
        };
        for cid in cids {
            let forget = format!("/v1/egress/forget/{cid}");
            if let Err(err) = crate::fleet::gateway(&app, cid, "POST", &forget, None) {
                eprintln!("apid: STOP ALL could not clear agent {cid}'s egress decisions: {err}");
            }
        }
    });
    app.control.set(state.clone());
    // Every workstation freezes: input stops, pixel sessions end, shells and runs stop (docs/design/workstations.md).
    crate::stations::freeze_all(app, true, by);
    // Weft takes no writes: no drafts, seals, landings, releases or exports.
    crate::weft_hub::hold(app, true);
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
    crate::weft_hub::hold(app, false);
    app.bus.publish("plaza", &state);
    app.work.dispatch_idle(app);
    state
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/plaza/state", |app, _, _| {
            Response::json(200, &app.control.state())
        })
        .doc(RouteDoc::new("Show whether the plaza is stopped"))
        .post("/v1/plaza/stop", stop_route)
        .doc(RouteDoc::new("Stop all work and hold the plaza").errors(&[400]))
        .post("/v1/plaza/resume", resume_route)
        .doc(RouteDoc::new("Resume the plaza after a stop"))
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
    fn hold_survives_a_round_trip_through_its_table() {
        let control = Control::default();
        assert!(!control.paused());
        control.set(PlazaState {
            paused: true,
            since_ms: 5,
            by: "owner".into(),
            reason: "bad loop".into(),
            cancelled: 2,
        });
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &control.rows().unwrap()).unwrap();

        let restored = Control::default();
        restored.load(&conn).unwrap();
        assert!(restored.paused());
        assert_eq!(restored.state().cancelled, 2);
        assert_eq!(restored.state().reason, "bad loop");
        assert_eq!(restored.state(), control.state());
    }
}
