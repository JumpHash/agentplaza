//! The local daemons' latest answers (PLAN 30.4): what noded says of this machine and its load, and whether noded and
//! the hub daemon answer, refreshed every [`EVERY`] in the background, the three probes at once. The overview, the
//! fleet, the machines list and the local node's record read the latest round instead of asking on each request, so
//! a daemon that is slow or down never holds them. Only the first read after a start waits, for one round.

use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use serde_json::Value;

use crate::state::App;

/// How often the probes run.
pub const EVERY: Duration = Duration::from_secs(2);

/// One round of the probes.
#[derive(Debug, Clone)]
pub struct Latest {
    /// noded's `/v1/machine`, when it answered 200.
    pub machine: Option<Value>,
    /// noded's `/v1/machine/stats`, when it answered 200.
    pub stats: Option<Value>,
    /// Whether the hub daemon's `/healthz` answered 200.
    pub hub_up: bool,
}

impl Latest {
    /// Whether noded answered this round.
    pub fn noded_up(&self) -> bool {
        self.machine.is_some()
    }
}

static LATEST: RwLock<Option<Latest>> = RwLock::new(None);
static STARTED: OnceLock<()> = OnceLock::new();

fn answered(result: Result<(u16, Value), String>) -> Option<Value> {
    result
        .ok()
        .filter(|(code, _)| *code == 200)
        .map(|(_, body)| body)
}

/// One round: the three probes on their own threads, joined.
fn round(app: &App) -> Latest {
    std::thread::scope(|scope| {
        let machine = scope.spawn(|| answered(app.noded.get("/v1/machine")));
        let stats = scope.spawn(|| answered(app.noded.get("/v1/machine/stats")));
        let hub = scope.spawn(|| matches!(app.hub.get("/healthz"), Ok((200, _))));
        Latest {
            machine: machine.join().ok().flatten(),
            stats: stats.join().ok().flatten(),
            hub_up: hub.join().unwrap_or(false),
        }
    })
}

fn store(latest: Latest) {
    *LATEST
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(latest);
}

/// The latest round. The first call after a start runs a round itself (and starts the background prober); every
/// later call answers at once.
pub fn latest(app: &App) -> Latest {
    if let Some(latest) = LATEST
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
    {
        return latest;
    }
    let first = round(app);
    store(first.clone());
    start(app);
    first
}

/// Starts the background prober (once).
fn start(app: &App) {
    let Some(app) = app.handle().upgrade() else {
        return;
    };
    if STARTED.set(()).is_err() {
        return;
    }
    std::thread::Builder::new()
        .name("probes".into())
        .spawn(move || prober(app))
        .ok();
}

fn prober(app: Arc<App>) {
    loop {
        std::thread::sleep(EVERY);
        store(round(&app));
    }
}
