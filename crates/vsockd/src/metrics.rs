//! vsockd's metrics, on `GET /v1/metrics` in the Prometheus text format (behind the internal token, like every route
//! but `/healthz`). `apz_vsockd_session_wakeups_total` counts how often keepalive sessions wake: divided by the open
//! sessions it is the idle cost per agent, which the event-driven wait keeps near one wake per heartbeat.

use apz_metrics::prom::{self, Counter, Gauge, Histogram, Metric};

/// Keepalive and one-shot sessions open now.
pub static SESSIONS: Gauge = Gauge::new("apz_vsockd_sessions", "Guest sessions open");
/// Connections refused because the session cap was reached.
pub static SESSIONS_REFUSED: Counter = Counter::new(
    "apz_vsockd_sessions_refused_total",
    "Guest connections refused at the session cap",
);
/// Times a keepalive session thread woke up (a guest line, a hub command, an apid answer, or a timeout).
pub static SESSION_WAKEUPS: Counter = Counter::new(
    "apz_vsockd_session_wakeups_total",
    "Times a keepalive session woke up",
);
/// A guest's hello: apid's profile lookup.
pub static HELLO: Histogram = Histogram::new(
    "apz_vsockd_hello_seconds",
    "A guest's hello: apid's profile lookup",
);

/// The Prometheus text of every vsockd metric.
pub fn render() -> String {
    prom::render(&[
        &SESSIONS as &dyn Metric,
        &SESSIONS_REFUSED,
        &SESSION_WAKEUPS,
        &HELLO,
    ])
}
