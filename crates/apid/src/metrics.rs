//! The hub's metrics, on `GET /v1/metrics` in the Prometheus text format (behind the owner's auth, like every route).
//! Wakes and sleeps are timed end to end here; the node times its own part (noded's `/v1/metrics`), and each wake also
//! writes one `timing op=wake` line on stderr (see `crate::sleep`).

use apz_http::{Response, Router};
use apz_metrics::{Counter, Gauge, Histogram, Metric};

use crate::agents::Status;
use crate::state::App;

/// Wakes the hub started (one noded restore each).
pub static WAKES: Counter = Counter::new("apz_hub_wakes_total", "Agent wakes the hub started");
/// Wakes refused because the wake queue was full.
pub static WAKES_REFUSED: Counter = Counter::new(
    "apz_hub_wakes_refused_total",
    "Agent wakes refused: the wake queue was full",
);
/// Wakes queued or running.
pub static WAKES_IN_FLIGHT: Gauge =
    Gauge::new("apz_hub_wakes_in_flight", "Agent wakes queued or running");
/// From the wake request to the agent's hello.
pub static WAKE: Histogram = Histogram::new(
    "apz_hub_wake_seconds",
    "A wake, from the request to the agent's hello",
);
/// The node's part of a wake: the restore call.
pub static WAKE_NODE: Histogram = Histogram::new(
    "apz_hub_wake_node_seconds",
    "The node's part of a wake: its restore call",
);
/// From the node's answer to the agent's hello: the guest reconnecting.
pub static WAKE_RECONNECT: Histogram = Histogram::new(
    "apz_hub_wake_reconnect_seconds",
    "From the node's answer to the agent's hello: the guest reconnecting",
);
/// Sleeps the hub started.
pub static SLEEPS: Counter = Counter::new("apz_hub_sleeps_total", "Agent sleeps the hub started");
/// A sleep: the node's snapshot and stop.
pub static SLEEP: Histogram = Histogram::new(
    "apz_hub_sleep_seconds",
    "A sleep: the node's snapshot and stop",
);

/// `GET /v1/metrics`.
pub fn routes(router: Router<App>) -> Router<App> {
    router.get("/v1/metrics", |app, _, _| {
        let mut text = apz_metrics::render(&[
            &WAKES as &dyn Metric,
            &WAKES_REFUSED,
            &WAKES_IN_FLIGHT,
            &WAKE,
            &WAKE_NODE,
            &WAKE_RECONNECT,
            &SLEEPS,
            &SLEEP,
        ]);
        text.push_str(&agents_by_status(app));
        text.push_str(&wakes_queued());
        Response::new(200, "text/plain; version=0.0.4", text)
    })
}

/// `apz_hub_wakes_queued{lane}`: the wakes waiting for a worker, by lane, counted at scrape time.
fn wakes_queued() -> String {
    let (interactive, background) = crate::sleep::queued_by_lane();
    format!(
        "# HELP apz_hub_wakes_queued Agent wakes waiting for a worker, by lane\n# TYPE apz_hub_wakes_queued gauge\n\
         apz_hub_wakes_queued{{lane=\"interactive\"}} {interactive}\napz_hub_wakes_queued{{lane=\"background\"}} {background}\n"
    )
}

/// `apz_hub_agents{status="..."}`: the roster by status, counted at scrape time.
fn agents_by_status(app: &App) -> String {
    let (mut idle, mut working, mut asleep, mut offline) = (0u64, 0u64, 0u64, 0u64);
    for agent in app.agents.list() {
        match agent.status {
            Status::Idle => idle += 1,
            Status::Working => working += 1,
            Status::Asleep => asleep += 1,
            Status::Offline => offline += 1,
        }
    }
    format!(
        "# HELP apz_hub_agents Agents on the roster, by status\n# TYPE apz_hub_agents gauge\n\
         apz_hub_agents{{status=\"idle\"}} {idle}\napz_hub_agents{{status=\"working\"}} {working}\n\
         apz_hub_agents{{status=\"asleep\"}} {asleep}\napz_hub_agents{{status=\"offline\"}} {offline}\n"
    )
}
