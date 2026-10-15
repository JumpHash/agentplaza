//! The hub's metrics: what it records itself (model calls, tools, turns, agent states, its own API, database and
//! bus), what nodes send in their heartbeats, where it is kept, and the `/v1/metrics/*` routes the consoles read.
//!
//! * [`store`] — the [`store::Store`] trait: closed windows, events, model calls, kept traces and small saved
//!   values; [`mem`] keeps them in memory (and saved values in files) when there is no Postgres, [`pg`] in the
//!   partitioned metrics tables (sql/migrations/0014_metrics.sql) over its own connection.
//! * [`settings`] — the owner's choices: collection level, retention, budgets, outside endpoints.
//! * [`ingest`] — node batches: clock-offset correction, columns and histograms into the rollup, events, reports.
//! * [`collect`] — the hub's own collectors: API route timer, database timer, bus lag, socket count, queue and
//!   approval gauges, agent states, treasury, store sizes, and the metrics system's own cost.
//! * [`traces`] — per-agent span rings, the tail sampler, kept traces.
//! * [`alerting`] — rules, their evaluation as windows update, mute/acknowledge, history, notifications.
//! * [`live`] — scopes watched live (leases) and the 2 s frames on `/v1/ws` (`metrics` events).
//! * [`scope`] — the scope tree, the entities under a scope, and which series a scope covers.
//! * [`source`] — [`apz_metrics_view::source::Source`] over all of the above and the hub's roster.
//! * [`routes`] — `/v1/metrics/*`, the opt-in `/metrics` (OpenMetrics) and the CSV export.
//! * [`export`] — OpenMetrics text, CSV, and the opt-in OTLP trace export.
//!
//! Recording is cheap by construction: handles from the [`Registry`] are atomics; the scrape every 10 s (2 s while
//! a scope is live) moves what changed into the in-memory [`Rollup`], whose closed windows go to the store in
//! batches. Overview tiles and alert checks read the open windows, never a table.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use apz_metrics::level::Governor;
use apz_metrics::{Event, Registry, Rollup, SeriesKey};

use crate::state::App;

pub mod alerting;
pub mod collect;
pub mod export;
pub mod ingest;
pub mod live;
pub mod mem;
pub mod pg;
pub mod routes;
pub mod scope;
pub mod settings;
pub mod source;
pub mod source_misc;
pub mod store;
pub mod traces;

/// How often the hub scrapes its registry into the rollup (2 s while any scope is live).
pub const SCRAPE_MS: u64 = 10_000;
pub const LIVE_SCRAPE_MS: u64 = 2_000;

/// The latest of each node's link and its reports.
#[derive(Clone, Debug, Default)]
pub struct NodeLink {
    /// Hub time of the last heartbeat.
    pub last_beat_ms: u64,
    /// Node clock minus hub clock, smoothed.
    pub offset_ms: f64,
    pub rtt_ms: f64,
    /// Latest-only reports (`top_processes`, `gpus`, `volumes`).
    pub reports: serde_json::Value,
}

/// Everything metrics, on the [`App`].
pub struct Metrics {
    pub registry: Registry,
    pub rollup: Mutex<Rollup>,
    pub store: Box<dyn store::Store>,
    pub settings: settings::Holder,
    pub governor: Mutex<Governor>,
    pub traces: traces::Traces,
    pub alerts: alerting::Alerts,
    pub live: live::Leases,
    pub links: Mutex<HashMap<String, NodeLink>>,
    /// The latest raw point of each series (live frames read these).
    pub latest: Mutex<HashMap<SeriesKey, (u64, f64)>>,
}

impl Metrics {
    /// Metrics over Postgres (its own connection to `db_url`) when given, else in memory with saved values under
    /// `state_dir/metrics`.
    pub fn open(state_dir: &Path, db_url: Option<&str>, now_ms: u64) -> Metrics {
        let store: Box<dyn store::Store> = match db_url.and_then(pg::PgStore::connect) {
            Some(pg) => Box::new(pg),
            None => Box::new(mem::MemStore::new(Some(state_dir.join("metrics")))),
        };
        let settings = settings::Holder::load(store.as_ref());
        let level = settings.get().level;
        Metrics {
            registry: Registry::default(),
            rollup: Mutex::new(Rollup::new()),
            governor: Mutex::new(Governor::new(level, now_ms)),
            traces: traces::Traces::new(now_ms),
            alerts: alerting::Alerts::load(store.as_ref()),
            live: live::Leases::default(),
            links: Mutex::new(HashMap::new()),
            latest: Mutex::new(HashMap::new()),
            settings,
            store,
        }
    }

    /// Records an event (lifecycle, compaction, scale …): stored, and counted into the histograms it carries.
    pub fn event(&self, e: Event) {
        ingest::record_event(self, e);
    }

    /// Scrapes the registry into the rollup, writes closed windows, and checks alert rules on what changed.
    pub fn tick(&self, app: &App, now_ms: u64) {
        collect::tick(self, app, now_ms);
    }
}

/// Locks a mutex, ignoring poisoning (a panicked holder leaves usable data here).
pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Starts the metrics threads: the scrape tick (10 s, 2 s while live), hourly retention and store sizes.
pub fn spawn(app: Arc<App>) {
    collect::spawn(app);
}
