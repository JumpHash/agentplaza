//! The metrics tables in Postgres (sql/migrations/0014_metrics.sql), over the store's own connection so metric
//! writes never wait behind the hub's state writes (and the other way round).
//!
//! * `metric_chunk_10s (day, series, start_ms, points bytea)` — one row per series per 10 minutes, the points
//!   `apz_metrics::chunk`-encoded (value = a counter's increase or a gauge's last reading; histograms are not
//!   kept at 10 s). The open 10-minute chunk of each series is held in memory and written when it closes.
//! * `metric_1m (day, series, hour_ms, aggs bytea)` — one row per series per hour: the hour's 1-minute
//!   aggregates as JSON, deflate-compressed; upserted every 10 minutes and when the hour closes.
//! * `metric_1h (day, series, day_ms, aggs bytea)` — one row per series per day, the same way.
//! * `metric_events`, `model_calls`, `trace_spans` (one row per kept trace, spans as jsonb), `metric_kv`.
//!
//! Every table except `metric_kv` is partitioned by day (`day` = UTC date); partitions are made on demand and
//! retention drops whole partitions. Writes are batched multi-row statements on [`Store::flush`] (every 10 s).

use std::collections::HashMap;
use std::sync::Mutex;

use apz_metrics::agg::{Agg, Closed, Tier};
use apz_metrics::trace::Trace;
use apz_metrics::{Event, Point, SeriesKey};

use super::settings::Retention;
use super::store::{CallRecord, Store};

#[derive(Default)]
struct Pending {
    /// The open 10-minute chunk of each series: (chunk start, points).
    chunks: HashMap<SeriesKey, (u64, Vec<Point>)>,
    /// Closed chunks waiting to be written.
    chunks_out: Vec<(SeriesKey, u64, Vec<Point>)>,
    /// 1-minute and 1-hour windows of the open hour / day per series, re-upserted on flush.
    minutes: HashMap<SeriesKey, (u64, Vec<(u64, Agg)>)>,
    hours: HashMap<SeriesKey, (u64, Vec<(u64, Agg)>)>,
    dirty_minutes: Vec<SeriesKey>,
    dirty_hours: Vec<SeriesKey>,
    events: Vec<Event>,
    calls: Vec<CallRecord>,
    traces: Vec<Trace>,
}

pub struct PgStore {
    url: String,
    client: Mutex<postgres::Client>,
    pending: Mutex<Pending>,
    /// Day partitions known to exist, per table.
    partitions: Mutex<std::collections::BTreeSet<(String, String)>>,
}

impl PgStore {
    /// Opens the store's own connection; `None` (logged) when it cannot connect.
    pub fn connect(url: &str) -> Option<PgStore> {
        match postgres::Client::connect(url, postgres::NoTls) {
            Ok(client) => Some(PgStore {
                url: url.to_string(),
                client: Mutex::new(client),
                pending: Mutex::new(Pending::default()),
                partitions: Mutex::new(Default::default()),
            }),
            Err(e) => {
                eprintln!("metrics: database connect failed ({e}); keeping metrics in memory");
                None
            }
        }
    }
}

impl Store for PgStore {
    fn put(&self, closed: &[Closed]) {
        let _ = (
            closed,
            &self.pending,
            &self.url,
            &self.client,
            &self.partitions,
        );
        todo!()
    }
    fn rows(
        &self,
        tier: Tier,
        metric: &str,
        from_ms: u64,
        to_ms: u64,
        keep: &dyn Fn(&SeriesKey) -> bool,
    ) -> Vec<(SeriesKey, Vec<(u64, Agg)>)> {
        let _ = (tier, metric, from_ms, to_ms, keep);
        todo!()
    }
    fn put_events(&self, events: &[Event]) {
        let _ = events;
        todo!()
    }
    fn events(
        &self,
        from_ms: u64,
        to_ms: u64,
        keep: &dyn Fn(&Event) -> bool,
        limit: usize,
    ) -> Vec<Event> {
        let _ = (from_ms, to_ms, keep, limit);
        todo!()
    }
    fn put_calls(&self, calls: &[CallRecord]) {
        let _ = calls;
        todo!()
    }
    fn calls(
        &self,
        before_ms: u64,
        limit: usize,
        keep: &dyn Fn(&CallRecord) -> bool,
    ) -> Vec<CallRecord> {
        let _ = (before_ms, limit, keep);
        todo!()
    }
    fn put_trace(&self, trace: &Trace) {
        let _ = trace;
        todo!()
    }
    fn traces(
        &self,
        from_ms: u64,
        to_ms: u64,
        keep: &dyn Fn(&Trace) -> bool,
        limit: usize,
    ) -> Vec<Trace> {
        let _ = (from_ms, to_ms, keep, limit);
        todo!()
    }
    fn trace(&self, id: &str) -> Option<Trace> {
        let _ = id;
        todo!()
    }
    fn kv_get(&self, key: &str) -> Option<String> {
        let _ = key;
        todo!()
    }
    fn kv_put(&self, key: &str, json: &str) {
        let _ = (key, json);
        todo!()
    }
    fn prune(&self, now_ms: u64, retention: &Retention) {
        let _ = (now_ms, retention);
        todo!()
    }
    fn sizes(&self) -> Vec<(String, u64)> {
        todo!()
    }
    fn flush(&self) {
        todo!()
    }
    fn kind(&self) -> &'static str {
        "postgres"
    }
}
