//! The metrics store in `metrics.db` (`store::METRICS`): typed tables for the 10-second points, the 1-minute and
//! 1-hour windows (with their histogram buckets), events, model calls and kept traces, and the saved values
//! (settings, alert state, dashboards), one row each.
//!
//! What closes or is recorded waits in memory and is written in one batch per [`Store::flush`] (every 10 s); reads
//! see what is waiting as well. Retention deletes by age, table by table; kept traces are also capped in bytes,
//! oldest first, and today's are never dropped.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use apz_metrics::agg::{Agg, Closed, Tier};
use apz_metrics::hist::HistSnapshot;
use apz_metrics::trace::{Keep, Span, SpanKind, Trace};
use apz_metrics::{EntityKind, Event, SeriesKey};
use apz_metrics_view::model::Saved as Dashboards;
use apz_store::rusqlite::{Connection, params};
use apz_store::table::{self, Value};
use apz_store::{Column, Db, RowSet, StoreError, Table, TableDef};

use super::alerting::Stored;
use super::settings::{MetricsSettings, Retention};
use super::store::{CallRecord, Store};
use crate::store::sql;

const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 86_400_000;
/// Rows read per page by the readers that filter what they read (a caller's filter can drop a row).
const PAGE: i64 = 500;

/// The stream tables, each with the children that go with it, for the stores table and "Metrics cost".
const SIZED: [(&str, &[&str]); 6] = [
    ("metric_sample", &["metric_sample"]),
    ("metric_agg_1m", &["metric_agg_1m", "metric_agg_1m_bucket"]),
    ("metric_agg_1h", &["metric_agg_1h", "metric_agg_1h_bucket"]),
    ("metric_event", &["metric_event", "metric_event_part"]),
    ("model_call", &["model_call", "model_call_context"]),
    ("trace", &["trace", "trace_span", "trace_span_attr"]),
];

/// The statements of one window tier (1-minute or 1-hour).
#[derive(Clone, Copy)]
struct WindowSql {
    put: &'static str,
    clear: &'static str,
    bucket_put: &'static str,
    range: &'static str,
    buckets: &'static str,
}

const MINUTE: WindowSql = WindowSql {
    put: sql::METRIC_AGG_1M_PUT,
    clear: sql::METRIC_AGG_1M_BUCKET_CLEAR,
    bucket_put: sql::METRIC_AGG_1M_BUCKET_PUT,
    range: sql::METRIC_AGG_1M_RANGE,
    buckets: sql::METRIC_AGG_1M_BUCKET_RANGE,
};

const HOUR: WindowSql = WindowSql {
    put: sql::METRIC_AGG_1H_PUT,
    clear: sql::METRIC_AGG_1H_BUCKET_CLEAR,
    bucket_put: sql::METRIC_AGG_1H_BUCKET_PUT,
    range: sql::METRIC_AGG_1H_RANGE,
    buckets: sql::METRIC_AGG_1H_BUCKET_RANGE,
};

/// What is waiting for the next flush. Reads see it as well as the stored rows.
#[derive(Default)]
struct Pending {
    /// Closed 10-second windows without a histogram, in arrival order: (series, window start ms, value).
    samples: Vec<(SeriesKey, u64, f64)>,
    /// Closed 1-minute windows by (series, start): a window that closes again replaces the one waiting.
    minutes: HashMap<(SeriesKey, u64), Agg>,
    /// Closed 1-hour windows by (series, start).
    hours: HashMap<(SeriesKey, u64), Agg>,
    events: Vec<Event>,
    calls: Vec<CallRecord>,
    traces: Vec<Trace>,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.samples.is_empty()
            && self.minutes.is_empty()
            && self.hours.is_empty()
            && self.events.is_empty()
            && self.calls.is_empty()
            && self.traces.is_empty()
    }
}

/// The metrics store over `metrics.db`.
pub struct SqliteStore {
    db: Arc<Db>,
    pending: Mutex<Pending>,
}

impl SqliteStore {
    /// The store over the metrics file (`store::METRICS`).
    pub fn new(db: Arc<Db>) -> SqliteStore {
        SqliteStore {
            db,
            pending: Mutex::new(Pending::default()),
        }
    }

    /// Puts a batch that failed to write back in front of what arrived since.
    fn restore(&self, old: Pending) {
        let mut p = super::lock(&self.pending);
        let Pending {
            samples,
            minutes,
            hours,
            events,
            calls,
            traces,
        } = old;
        prepend(&mut p.samples, samples);
        prepend(&mut p.events, events);
        prepend(&mut p.calls, calls);
        prepend(&mut p.traces, traces);
        // A window that closed again since the failed write is the newer one.
        for (id, agg) in minutes {
            p.minutes.entry(id).or_insert(agg);
        }
        for (id, agg) in hours {
            p.hours.entry(id).or_insert(agg);
        }
    }

    /// The stored 10-second points of the metric in `[from_ms, to_ms)`.
    fn stored_samples(
        &self,
        metric: &str,
        from_ms: u64,
        to_ms: u64,
    ) -> Result<Vec<(SeriesKey, u64, Agg)>, StoreError> {
        let (from, to) = (int(from_ms), int(to_ms));
        self.db.read(|conn| {
            let mut stmt = conn.prepare_cached(sql::METRIC_SAMPLE_RANGE)?;
            let rows = stmt.query_map(params![metric, from, to], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, f64>(2)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (series, t_ms, value) = row?;
                if let Some(key) = SeriesKey::parse(&series) {
                    out.push((key, uint(t_ms), point_agg(value)));
                }
            }
            Ok(out)
        })
    }

    /// The stored windows of one tier of the metric in `[from_ms, to_ms)`, with their histogram buckets.
    fn stored_windows(
        &self,
        tier: WindowSql,
        metric: &str,
        from_ms: u64,
        to_ms: u64,
    ) -> Result<Vec<(SeriesKey, u64, Agg)>, StoreError> {
        let (from, to) = (int(from_ms), int(to_ms));
        self.db.read(|conn| {
            let mut buckets: HashMap<(String, i64), Vec<(u16, u64)>> = HashMap::new();
            let mut stmt = conn.prepare_cached(tier.buckets)?;
            let rows = stmt.query_map(params![metric, from, to], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })?;
            for row in rows {
                let (series, start, bucket, count) = row?;
                let bucket = u16::try_from(bucket).unwrap_or(u16::MAX);
                buckets
                    .entry((series, start))
                    .or_default()
                    .push((bucket, uint(count)));
            }
            let mut out = Vec::new();
            let mut stmt = conn.prepare_cached(tier.range)?;
            let rows = stmt.query_map(params![metric, from, to], |r| {
                Ok(WindowRow {
                    series: r.get(0)?,
                    start: r.get(1)?,
                    count: r.get(2)?,
                    sum: r.get(3)?,
                    min: r.get(4)?,
                    max: r.get(5)?,
                    last: r.get(6)?,
                    hist_count: r.get(7)?,
                    hist_sum: r.get(8)?,
                    hist_min: r.get(9)?,
                    hist_max: r.get(10)?,
                })
            })?;
            for row in rows {
                let row = row?;
                let hist = row.hist_count.map(|count| HistSnapshot {
                    buckets: buckets
                        .remove(&(row.series.clone(), row.start))
                        .unwrap_or_default(),
                    count: uint(count),
                    sum: row.hist_sum.unwrap_or(0.0),
                    min: row.hist_min.unwrap_or(0.0),
                    max: row.hist_max.unwrap_or(0.0),
                });
                let agg = Agg {
                    count: uint(row.count),
                    sum: row.sum,
                    min: row.min,
                    max: row.max,
                    last: row.last,
                    hist,
                };
                if let Some(key) = SeriesKey::parse(&row.series) {
                    out.push((key, uint(row.start), agg));
                }
            }
            Ok(out)
        })
    }

    /// The stored events in `[from_ms, to_ms)` that `keep` accepts, newest first, at most `limit`.
    fn stored_events(
        &self,
        from_ms: u64,
        to_ms: u64,
        keep: &dyn Fn(&Event) -> bool,
        limit: usize,
    ) -> Result<Vec<Event>, StoreError> {
        let (from, to) = (int(from_ms), int(to_ms));
        self.db.read(|conn| {
            let mut out = Vec::new();
            let mut offset = 0;
            loop {
                let page = event_page(conn, from, to, offset)?;
                let fetched = index(page.len());
                for event in page {
                    if keep(&event) {
                        out.push(event);
                        if out.len() == limit {
                            return Ok(out);
                        }
                    }
                }
                if fetched < PAGE {
                    return Ok(out);
                }
                offset += fetched;
            }
        })
    }

    /// The stored model calls made before `before_ms` that `keep` accepts, newest first, at most `limit`.
    fn stored_calls(
        &self,
        before_ms: u64,
        keep: &dyn Fn(&CallRecord) -> bool,
        limit: usize,
    ) -> Result<Vec<CallRecord>, StoreError> {
        let before = int(before_ms);
        self.db.read(|conn| {
            let mut out = Vec::new();
            let mut offset = 0;
            loop {
                let page = call_page(conn, before, offset)?;
                let fetched = index(page.len());
                for call in page {
                    if keep(&call) {
                        out.push(call);
                        if out.len() == limit {
                            return Ok(out);
                        }
                    }
                }
                if fetched < PAGE {
                    return Ok(out);
                }
                offset += fetched;
            }
        })
    }

    /// The stored kept traces starting in `[from_ms, to_ms)` that `keep` accepts, newest first, at most `limit`.
    fn stored_traces(
        &self,
        from_ms: u64,
        to_ms: u64,
        keep: &dyn Fn(&Trace) -> bool,
        limit: usize,
    ) -> Result<Vec<Trace>, StoreError> {
        let (from, to) = (int(from_ms), int(to_ms));
        self.db.read(|conn| {
            let mut out = Vec::new();
            let mut offset = 0;
            loop {
                let page = trace_page(conn, from, to, offset)?;
                let fetched = index(page.len());
                for trace in page {
                    if keep(&trace) {
                        out.push(trace);
                        if out.len() == limit {
                            return Ok(out);
                        }
                    }
                }
                if fetched < PAGE {
                    return Ok(out);
                }
                offset += fetched;
            }
        })
    }

    /// Reads one saved value (a single row of its table).
    fn load<T: Table>(&self, def: &'static TableDef, what: &str) -> Option<T> {
        match self.db.read(|conn| table::load_one::<T>(conn, def)) {
            Ok(value) => value,
            Err(e) => {
                eprintln!("metrics: reading {what} failed ({e})");
                None
            }
        }
    }

    /// Writes one saved value: its table holds exactly its row, from now on. Waits for the commit.
    fn save<T: Table>(&self, def: &'static TableDef, value: &T, what: &str) {
        let rows = match RowSet::one(def, value) {
            Ok(rows) => rows,
            Err(e) => {
                eprintln!("metrics: saving {what} failed ({e})");
                return;
            }
        };
        if let Err(e) = self.db.write(move |conn| table::replace(conn, def, &rows)) {
            eprintln!("metrics: saving {what} failed ({e})");
        }
    }
}

impl Store for SqliteStore {
    fn put(&self, closed: &[Closed]) {
        let mut p = super::lock(&self.pending);
        for window in closed {
            match window.tier {
                // Histograms are kept at 1 minute and 1 hour; a 10-second window keeps its last reading.
                Tier::Raw10s if window.agg.hist.is_none() => {
                    p.samples
                        .push((window.key.clone(), window.start_ms, window.agg.last));
                }
                Tier::Raw10s => {}
                Tier::Min1 => {
                    p.minutes
                        .insert((window.key.clone(), window.start_ms), window.agg.clone());
                }
                Tier::Hour1 => {
                    p.hours
                        .insert((window.key.clone(), window.start_ms), window.agg.clone());
                }
            }
        }
    }

    fn rows(
        &self,
        tier: Tier,
        metric: &str,
        from_ms: u64,
        to_ms: u64,
        keep: &dyn Fn(&SeriesKey) -> bool,
    ) -> Vec<(SeriesKey, Vec<(u64, Agg)>)> {
        let stored = match tier {
            Tier::Raw10s => self.stored_samples(metric, from_ms, to_ms),
            Tier::Min1 => self.stored_windows(MINUTE, metric, from_ms, to_ms),
            Tier::Hour1 => self.stored_windows(HOUR, metric, from_ms, to_ms),
        };
        let stored = stored.unwrap_or_else(|e| {
            eprintln!("metrics: reading {} failed ({e})", tier.words());
            Vec::new()
        });
        let mut found: BTreeMap<SeriesKey, BTreeMap<u64, Agg>> = BTreeMap::new();
        for (key, start, agg) in stored {
            if keep(&key) {
                found.entry(key).or_default().insert(start, agg);
            }
        }
        // What is waiting replaces the stored windows of the same start.
        let inside = |start: u64| start >= from_ms && start < to_ms;
        let mut waiting: Vec<(SeriesKey, u64, Agg)> = Vec::new();
        {
            let p = super::lock(&self.pending);
            match tier {
                Tier::Raw10s => {
                    for (key, start, value) in &p.samples {
                        if key.metric == metric && inside(*start) && keep(key) {
                            waiting.push((key.clone(), *start, point_agg(*value)));
                        }
                    }
                }
                Tier::Min1 | Tier::Hour1 => {
                    let map = if tier == Tier::Min1 {
                        &p.minutes
                    } else {
                        &p.hours
                    };
                    for ((key, start), agg) in map {
                        if key.metric == metric && inside(*start) && keep(key) {
                            waiting.push((key.clone(), *start, agg.clone()));
                        }
                    }
                }
            }
        }
        for (key, start, agg) in waiting {
            found.entry(key).or_default().insert(start, agg);
        }
        found
            .into_iter()
            .map(|(key, windows)| (key, windows.into_iter().collect()))
            .collect()
    }

    fn put_events(&self, events: &[Event]) {
        super::lock(&self.pending)
            .events
            .extend(events.iter().cloned());
    }

    fn events(
        &self,
        from_ms: u64,
        to_ms: u64,
        keep: &dyn Fn(&Event) -> bool,
        limit: usize,
    ) -> Vec<Event> {
        if limit == 0 {
            return Vec::new();
        }
        let mut out = self
            .stored_events(from_ms, to_ms, keep, limit)
            .unwrap_or_else(|e| {
                eprintln!("metrics: reading events failed ({e})");
                Vec::new()
            });
        {
            let p = super::lock(&self.pending);
            for event in &p.events {
                if event.t_ms >= from_ms && event.t_ms < to_ms && keep(event) {
                    out.push(event.clone());
                }
            }
        }
        out.sort_by_key(|e| Reverse(e.t_ms));
        out.truncate(limit);
        out
    }

    fn put_calls(&self, calls: &[CallRecord]) {
        super::lock(&self.pending)
            .calls
            .extend(calls.iter().cloned());
    }

    fn calls(
        &self,
        before_ms: u64,
        limit: usize,
        keep: &dyn Fn(&CallRecord) -> bool,
    ) -> Vec<CallRecord> {
        if limit == 0 {
            return Vec::new();
        }
        let mut out = self
            .stored_calls(before_ms, keep, limit)
            .unwrap_or_else(|e| {
                eprintln!("metrics: reading model calls failed ({e})");
                Vec::new()
            });
        {
            let p = super::lock(&self.pending);
            for call in &p.calls {
                if call.at_ms < before_ms && keep(call) {
                    out.push(call.clone());
                }
            }
        }
        out.sort_by_key(|c| Reverse(c.at_ms));
        out.truncate(limit);
        out
    }

    fn put_trace(&self, trace: &Trace) {
        super::lock(&self.pending).traces.push(trace.clone());
    }

    fn traces(
        &self,
        from_ms: u64,
        to_ms: u64,
        keep: &dyn Fn(&Trace) -> bool,
        limit: usize,
    ) -> Vec<Trace> {
        if limit == 0 {
            return Vec::new();
        }
        let mut out = self
            .stored_traces(from_ms, to_ms, keep, limit)
            .unwrap_or_else(|e| {
                eprintln!("metrics: reading traces failed ({e})");
                Vec::new()
            });
        {
            let p = super::lock(&self.pending);
            for trace in &p.traces {
                if trace.start_ms >= from_ms && trace.start_ms < to_ms && keep(trace) {
                    out.push(trace.clone());
                }
            }
        }
        out.sort_by_key(|t| Reverse(t.start_ms));
        out.truncate(limit);
        out
    }

    fn trace(&self, id: &str) -> Option<Trace> {
        {
            let p = super::lock(&self.pending);
            if let Some(found) = p.traces.iter().rev().find(|t| t.id == id) {
                return Some(found.clone());
            }
        }
        match self.db.read(|conn| trace_by_id(conn, id)) {
            Ok(found) => found,
            Err(e) => {
                eprintln!("metrics: reading trace {id} failed ({e})");
                None
            }
        }
    }

    fn settings(&self) -> Option<MetricsSettings> {
        self.load(settings_def(), "the settings")
    }

    fn put_settings(&self, settings: &MetricsSettings) {
        self.save(settings_def(), settings, "the settings");
    }

    fn alerts(&self) -> Option<Stored> {
        self.load(alerts_def(), "the alerts")
    }

    fn put_alerts(&self, stored: &Stored) {
        self.save(alerts_def(), stored, "the alerts");
    }

    fn dashboards(&self) -> Option<Dashboards> {
        self.load(dashboards_def(), "the dashboards")
    }

    fn put_dashboards(&self, saved: &Dashboards) {
        self.save(dashboards_def(), saved, "the dashboards");
    }

    fn prune(&self, now_ms: u64, retention: &Retention) {
        let today = now_ms - now_ms % DAY_MS;
        let days_back = |days: u32| today.saturating_sub(u64::from(days) * DAY_MS);
        let cuts = [
            (
                sql::METRIC_SAMPLE_PRUNE,
                now_ms.saturating_sub(u64::from(retention.chunks_hours) * HOUR_MS),
            ),
            (sql::METRIC_AGG_1M_PRUNE, days_back(retention.minute_days)),
            (sql::METRIC_AGG_1H_PRUNE, days_back(retention.hour_days)),
            (sql::METRIC_EVENT_PRUNE, days_back(retention.events_days)),
            (sql::METRIC_CALL_PRUNE, days_back(retention.calls_days)),
            (sql::METRIC_TRACE_PRUNE, days_back(retention.traces_days)),
        ];
        let cap = u64::from(retention.traces_cap_mb) * 1024 * 1024;
        let outcome = self.db.write(move |conn| {
            for (statement, cut) in cuts {
                conn.prepare_cached(statement)?.execute([int(cut)])?;
            }
            cap_traces(conn, cap, int(today))
        });
        if let Err(e) = outcome {
            eprintln!("metrics: pruning failed ({e})");
        }
    }

    fn sizes(&self) -> Vec<(String, u64)> {
        self.db
            .read(|conn| {
                let mut out = Vec::with_capacity(SIZED.len());
                for (name, tables) in SIZED {
                    let mut bytes = 0;
                    for table in tables {
                        let on_disk: i64 =
                            conn.query_row(sql::METRIC_SIZE, params![table], |r| r.get(0))?;
                        bytes += uint(on_disk);
                    }
                    out.push((name.to_string(), bytes));
                }
                Ok(out)
            })
            .unwrap_or_else(|e| {
                eprintln!("metrics: sizing the tables failed ({e})");
                Vec::new()
            })
    }

    fn flush(&self) {
        let batch = std::mem::take(&mut *super::lock(&self.pending));
        if batch.is_empty() {
            return;
        }
        let shared = Arc::new(batch);
        let job = Arc::clone(&shared);
        if let Err(e) = self.db.write(move |conn| write_batch(conn, &job)) {
            eprintln!("metrics: flush failed ({e}); the rows wait for the next flush");
            if let Ok(batch) = Arc::try_unwrap(shared) {
                self.restore(batch);
            }
        }
    }
}

/// The saved values' tables, one row each: the settings, the alert state, the dashboards.
fn settings_def() -> &'static TableDef {
    apz_store::root_def!(MetricsSettings)
}

fn alerts_def() -> &'static TableDef {
    apz_store::root_def!(Stored)
}

fn dashboards_def() -> &'static TableDef {
    apz_store::root_def!(Dashboards)
}

/// Every saved value's definition (the migration must hold each one exactly).
#[cfg(test)]
fn saved_defs() -> Vec<&'static TableDef> {
    vec![settings_def(), alerts_def(), dashboards_def()]
}

/// A window row as the stored-windows query returns it.
struct WindowRow {
    series: String,
    start: i64,
    count: i64,
    sum: f64,
    min: f64,
    max: f64,
    last: f64,
    hist_count: Option<i64>,
    hist_sum: Option<f64>,
    hist_min: Option<f64>,
    hist_max: Option<f64>,
}

/// Writes one closed window and its histogram buckets (the buckets are replaced with the window).
fn write_window(
    conn: &Connection,
    sqls: WindowSql,
    key: &SeriesKey,
    start: u64,
    agg: &Agg,
) -> Result<(), StoreError> {
    let (metric, series) = (key.metric.as_str(), key.id());
    let (hist_count, hist_sum, hist_min, hist_max) = match &agg.hist {
        Some(h) => (Some(int(h.count)), Some(h.sum), Some(h.min), Some(h.max)),
        None => (None, None, None, None),
    };
    conn.prepare_cached(sqls.put)?.execute(params![
        metric,
        int(start),
        series.as_str(),
        int(agg.count),
        agg.sum,
        agg.min,
        agg.max,
        agg.last,
        hist_count,
        hist_sum,
        hist_min,
        hist_max
    ])?;
    conn.prepare_cached(sqls.clear)?
        .execute(params![metric, int(start), series.as_str()])?;
    if let Some(h) = &agg.hist {
        for (bucket, count) in &h.buckets {
            conn.prepare_cached(sqls.bucket_put)?.execute(params![
                metric,
                int(start),
                series.as_str(),
                i64::from(*bucket),
                int(*count)
            ])?;
        }
    }
    Ok(())
}

/// Writes everything waiting in one transaction (the writer's savepoint): a failure writes none of it.
fn write_batch(conn: &Connection, b: &Pending) -> Result<(), StoreError> {
    for (key, t_ms, value) in &b.samples {
        conn.prepare_cached(sql::METRIC_SAMPLE_PUT)?
            .execute(params![key.metric.as_str(), int(*t_ms), key.id(), *value])?;
    }
    for ((key, start), agg) in &b.minutes {
        write_window(conn, MINUTE, key, *start, agg)?;
    }
    for ((key, start), agg) in &b.hours {
        write_window(conn, HOUR, key, *start, agg)?;
    }
    for event in &b.events {
        let entity_kind = Column::to_value(&event.entity_kind)?;
        conn.prepare_cached(sql::METRIC_EVENT_PUT)?
            .execute(params![
                int(event.t_ms),
                event.kind.as_str(),
                entity_kind,
                event.entity.as_str(),
                event.duration_ms,
                event.detail.as_str()
            ])?;
        let id = conn.last_insert_rowid();
        for (pos, (name, value)) in event.parts.iter().enumerate() {
            conn.prepare_cached(sql::METRIC_EVENT_PART_PUT)?
                .execute(params![id, index(pos), name.as_str(), *value])?;
        }
    }
    for call in &b.calls {
        conn.prepare_cached(sql::METRIC_CALL_PUT)?.execute(params![
            int(call.at_ms),
            int(call.cid),
            call.role.as_str(),
            call.run.as_str(),
            call.node.as_str(),
            call.provider.as_str(),
            call.model.as_str(),
            call.engine.as_str(),
            int(call.tokens_in),
            int(call.cache_read),
            int(call.cache_write),
            int(call.tokens_out),
            call.cost_usd,
            call.ttft_ms,
            call.duration_ms,
            i64::from(call.status),
            i64::from(call.retries),
            call.task.as_deref(),
            call.trace.as_deref()
        ])?;
        let id = conn.last_insert_rowid();
        for (pos, (part, tokens)) in call.context.iter().enumerate() {
            conn.prepare_cached(sql::METRIC_CALL_CONTEXT_PUT)?
                .execute(params![id, index(pos), part.as_str(), int(*tokens)])?;
        }
    }
    for trace in &b.traces {
        let kept = match trace.kept {
            Some(kept) => Column::to_value(&kept)?,
            None => Value::Null,
        };
        conn.prepare_cached(sql::METRIC_TRACE_PUT)?
            .execute(params![
                trace.id.as_str(),
                int(trace.cid),
                trace.role.as_str(),
                int(trace.start_ms),
                trace.dur_ms,
                trace.ok,
                kept,
                int(trace_bytes(trace))
            ])?;
        let seq = conn.last_insert_rowid();
        for (pos, span) in trace.spans.iter().enumerate() {
            let kind = Column::to_value(&span.kind)?;
            conn.prepare_cached(sql::METRIC_TRACE_SPAN_PUT)?
                .execute(params![
                    seq,
                    index(pos),
                    i64::from(span.id),
                    span.parent.map(i64::from),
                    kind,
                    span.name.as_str(),
                    int(span.start_ms),
                    span.dur_ms,
                    span.first_token_ms,
                    span.ok
                ])?;
            for (attr, (name, value)) in span.attrs.iter().enumerate() {
                conn.prepare_cached(sql::METRIC_TRACE_ATTR_PUT)?
                    .execute(params![
                        seq,
                        index(pos),
                        index(attr),
                        name.as_str(),
                        value.as_str()
                    ])?;
            }
        }
    }
    Ok(())
}

/// One page of stored events starting at `offset`, newest first, each with its parts.
fn event_page(
    conn: &Connection,
    from: i64,
    to: i64,
    offset: i64,
) -> Result<Vec<Event>, StoreError> {
    type Head = (i64, i64, String, Value, String, Option<f64>, String);
    let heads: Vec<Head> = {
        let mut stmt = conn.prepare_cached(sql::METRIC_EVENT_PAGE)?;
        let rows = stmt.query_map(params![from, to, PAGE, offset], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })?;
        rows.collect::<Result<_, _>>()?
    };
    let mut out = Vec::with_capacity(heads.len());
    for (id, t_ms, kind, entity_kind, entity, duration_ms, detail) in heads {
        let Ok(entity_kind) = EntityKind::from_value(entity_kind) else {
            continue;
        };
        let parts: Vec<(String, f64)> = {
            let mut stmt = conn.prepare_cached(sql::METRIC_EVENT_PARTS)?;
            let rows = stmt.query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<Result<_, _>>()?
        };
        out.push(Event {
            t_ms: uint(t_ms),
            kind,
            entity_kind,
            entity,
            duration_ms,
            detail,
            parts,
        });
    }
    Ok(out)
}

/// One page of model calls made before `before`, newest first, each with its context split.
fn call_page(conn: &Connection, before: i64, offset: i64) -> Result<Vec<CallRecord>, StoreError> {
    let heads: Vec<(i64, CallRecord)> = {
        let mut stmt = conn.prepare_cached(sql::METRIC_CALL_PAGE)?;
        let rows = stmt.query_map(params![before, PAGE, offset], |r| {
            let call = CallRecord {
                at_ms: uint(r.get(1)?),
                cid: uint(r.get(2)?),
                role: r.get(3)?,
                run: r.get(4)?,
                node: r.get(5)?,
                provider: r.get(6)?,
                model: r.get(7)?,
                engine: r.get(8)?,
                tokens_in: uint(r.get(9)?),
                cache_read: uint(r.get(10)?),
                cache_write: uint(r.get(11)?),
                tokens_out: uint(r.get(12)?),
                cost_usd: r.get(13)?,
                ttft_ms: r.get(14)?,
                duration_ms: r.get(15)?,
                status: u16::try_from(r.get::<_, i64>(16)?).unwrap_or(u16::MAX),
                retries: u32::try_from(r.get::<_, i64>(17)?).unwrap_or(u32::MAX),
                task: r.get(18)?,
                context: Vec::new(),
                trace: r.get(19)?,
            };
            Ok((r.get(0)?, call))
        })?;
        rows.collect::<Result<_, _>>()?
    };
    let mut out = Vec::with_capacity(heads.len());
    for (id, mut call) in heads {
        let mut stmt = conn.prepare_cached(sql::METRIC_CALL_CONTEXT)?;
        let rows = stmt.query_map(params![id], |r| {
            Ok((r.get::<_, String>(0)?, uint(r.get::<_, i64>(1)?)))
        })?;
        call.context = rows.collect::<Result<_, _>>()?;
        out.push(call);
    }
    Ok(out)
}

/// The header columns of one kept trace, as the trace queries return them.
type TraceHead = (i64, String, i64, String, i64, f64, bool, Value);

/// One page of kept traces starting in `[from, to)`, newest first, each with its spans.
fn trace_page(
    conn: &Connection,
    from: i64,
    to: i64,
    offset: i64,
) -> Result<Vec<Trace>, StoreError> {
    let heads: Vec<TraceHead> = {
        let mut stmt = conn.prepare_cached(sql::METRIC_TRACE_PAGE)?;
        let rows = stmt.query_map(params![from, to, PAGE, offset], trace_head_row)?;
        rows.collect::<Result<_, _>>()?
    };
    heads
        .into_iter()
        .map(|head| {
            let seq = head.0;
            let mut trace = trace_of_head(head)?;
            trace.spans = trace_spans(conn, seq)?;
            Ok(trace)
        })
        .collect()
}

/// The newest kept trace with this id, with its spans.
fn trace_by_id(conn: &Connection, id: &str) -> Result<Option<Trace>, StoreError> {
    let head = {
        let mut stmt = conn.prepare_cached(sql::METRIC_TRACE_BY_ID)?;
        let mut rows = stmt.query_map(params![id], trace_head_row)?;
        match rows.next() {
            Some(row) => row?,
            None => return Ok(None),
        }
    };
    let seq = head.0;
    let mut trace = trace_of_head(head)?;
    trace.spans = trace_spans(conn, seq)?;
    Ok(Some(trace))
}

fn trace_head_row(r: &apz_store::rusqlite::Row<'_>) -> apz_store::rusqlite::Result<TraceHead> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
    ))
}

/// A kept trace from its header columns (its spans and attributes are read separately).
fn trace_of_head(head: TraceHead) -> Result<Trace, StoreError> {
    let (_, id, cid, role, start_ms, dur_ms, ok, kept) = head;
    Ok(Trace {
        id,
        cid: uint(cid),
        role,
        start_ms: uint(start_ms),
        dur_ms,
        ok,
        spans: Vec::new(),
        kept: Option::<Keep>::from_value(kept)?,
    })
}

/// The spans of one kept trace, in order, with their attributes.
fn trace_spans(conn: &Connection, seq: i64) -> Result<Vec<Span>, StoreError> {
    type SpanHead = (
        i64,
        i64,
        Option<i64>,
        Value,
        String,
        i64,
        f64,
        Option<f64>,
        bool,
    );
    let heads: Vec<SpanHead> = {
        let mut stmt = conn.prepare_cached(sql::METRIC_TRACE_SPANS)?;
        let rows = stmt.query_map(params![seq], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
            ))
        })?;
        rows.collect::<Result<_, _>>()?
    };
    let mut spans = Vec::with_capacity(heads.len());
    for (_, span_id, parent, kind, name, start_ms, dur_ms, first_token_ms, ok) in heads {
        spans.push(Span {
            id: u32::try_from(span_id).unwrap_or(u32::MAX),
            parent: parent.map(|p| u32::try_from(p).unwrap_or(u32::MAX)),
            kind: SpanKind::from_value(kind)?,
            name,
            start_ms: uint(start_ms),
            dur_ms,
            first_token_ms,
            ok,
            attrs: Vec::new(),
        });
    }
    let attrs: Vec<(i64, String, String)> = {
        let mut stmt = conn.prepare_cached(sql::METRIC_TRACE_ATTRS)?;
        let rows = stmt.query_map(params![seq], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    for (span, name, value) in attrs {
        // An attribute belongs to the span at its position (spans are stored in order, from 0).
        if let Some(s) = usize::try_from(span).ok().and_then(|i| spans.get_mut(i)) {
            s.attrs.push((name, value));
        }
    }
    Ok(spans)
}

/// Drops the oldest kept traces (those that start before today) while all kept traces together take more than
/// `cap` bytes.
fn cap_traces(conn: &Connection, cap: u64, today: i64) -> Result<(), StoreError> {
    let total = uint(conn.query_row(sql::METRIC_TRACE_BYTES, [], |r| r.get::<_, i64>(0))?);
    if total <= cap {
        return Ok(());
    }
    let oldest: Vec<(i64, u64)> = {
        let mut stmt = conn.prepare_cached(sql::METRIC_TRACE_OLDEST)?;
        let rows = stmt.query_map(params![today], |r| {
            Ok((r.get::<_, i64>(0)?, uint(r.get::<_, i64>(1)?)))
        })?;
        rows.collect::<Result<_, _>>()?
    };
    let bytes: Vec<u64> = oldest.iter().map(|(_, b)| *b).collect();
    for &(seq, _) in oldest.iter().take(drop_for_cap(total, cap, &bytes)) {
        conn.prepare_cached(sql::METRIC_TRACE_DELETE)?
            .execute(params![seq])?;
    }
    Ok(())
}

/// How many of the oldest kept traces to drop, given their bytes oldest first, so that what is left fits in `cap`.
/// `total` is every kept trace's bytes together (today's included, which are never on the list).
fn drop_for_cap(total: u64, cap: u64, oldest: &[u64]) -> usize {
    let mut left = total;
    let mut dropped = 0;
    for bytes in oldest {
        if left <= cap {
            break;
        }
        left = left.saturating_sub(*bytes);
        dropped += 1;
    }
    dropped
}

/// The rough bytes a kept trace takes: its text and a fixed share per span and attribute.
fn trace_bytes(trace: &Trace) -> u64 {
    let spans: u64 = trace
        .spans
        .iter()
        .map(|span| {
            let attrs: u64 = span
                .attrs
                .iter()
                .map(|(k, v)| 16 + (k.len() + v.len()) as u64)
                .sum();
            64 + span.name.len() as u64 + attrs
        })
        .sum();
    96 + trace.id.len() as u64 + trace.role.len() as u64 + spans
}

/// One point as a window: a single reading.
fn point_agg(value: f64) -> Agg {
    Agg {
        count: 1,
        sum: value,
        min: value,
        max: value,
        last: value,
        hist: None,
    }
}

/// A count or a time in the store's integer columns (values past `i64::MAX` are kept at the top).
fn int(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// A stored integer as the count or time it is (a negative value, never written, reads as 0).
fn uint(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

/// A position in a list, as the store keeps it.
fn index(pos: usize) -> i64 {
    i64::try_from(pos).unwrap_or(i64::MAX)
}

/// Puts `older` in front of `queue`.
fn prepend<T>(queue: &mut Vec<T>, mut older: Vec<T>) {
    older.append(queue);
    *queue = older;
}

/// A store in a fresh folder under the system temp dir, for tests.
#[cfg(test)]
pub fn scratch_store(name: &str) -> apz_store::Store {
    let dir = std::env::temp_dir().join(format!("apid-metrics-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    crate::store::open(&dir).expect("the store opens in a fresh folder")
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_metrics::EntityKind;
    use apz_metrics::alerts::starter_rules;
    use apz_metrics::trace::SpanKind;

    fn metrics(files: &apz_store::Store) -> SqliteStore {
        SqliteStore::new(Arc::clone(files.db(crate::store::METRICS)))
    }

    fn window(key: &SeriesKey, tier: Tier, start_ms: u64, agg: Agg) -> Closed {
        Closed {
            key: key.clone(),
            tier,
            start_ms,
            agg,
        }
    }

    fn cpu(value: f64) -> Agg {
        Agg {
            count: 1,
            sum: value,
            min: value,
            max: value,
            last: value,
            hist: None,
        }
    }

    fn event(t_ms: u64) -> Event {
        Event {
            t_ms,
            kind: "boot".to_string(),
            entity_kind: EntityKind::Vm,
            entity: "vm-1".to_string(),
            duration_ms: None,
            detail: String::new(),
            parts: Vec::new(),
        }
    }

    fn call(at_ms: u64) -> CallRecord {
        CallRecord {
            at_ms,
            cid: 109,
            role: "builder".into(),
            run: "r1".into(),
            node: "n0".into(),
            provider: "anthropic".into(),
            model: "opus".into(),
            engine: "plaza harness".into(),
            tokens_in: 1200,
            cache_read: 800,
            cache_write: 40,
            tokens_out: 300,
            cost_usd: 0.25,
            ttft_ms: Some(410.5),
            duration_ms: Some(2300.0),
            status: 200,
            retries: 1,
            task: Some("t-9".into()),
            context: vec![("system".into(), 500), ("tools".into(), 300)],
            trace: Some("tr-0badf00d".into()),
        }
    }

    fn span(id: u32, parent: Option<u32>, kind: SpanKind) -> Span {
        Span {
            id,
            parent,
            kind,
            name: format!("span {id}"),
            start_ms: 100 + u64::from(id),
            dur_ms: 40.5,
            first_token_ms: (kind == SpanKind::Model).then_some(12.0),
            ok: id != 3,
            attrs: vec![
                ("model".into(), "opus".into()),
                ("outcome".into(), "ok".into()),
            ],
        }
    }

    fn turn(start_ms: u64, ok: bool, kept: Option<Keep>) -> Trace {
        Trace {
            id: "tr-1a2b3c4d".into(),
            cid: 109,
            role: "builder".into(),
            start_ms,
            dur_ms: 950.0,
            ok,
            spans: vec![
                span(1, None, SpanKind::Model),
                span(2, Some(1), SpanKind::Tool),
                span(3, Some(1), SpanKind::HubMcp),
            ],
            kept,
        }
    }

    #[test]
    fn every_stream_row_reads_back_as_it_was_written() {
        let files = scratch_store("round-trip");
        let store = metrics(&files);
        let key = SeriesKey::new("vm.cpu_pct", EntityKind::Vm, "r1/0");
        let hist = HistSnapshot {
            buckets: vec![(3, 2), (9, 1)],
            count: 3,
            sum: 12.5,
            min: 1.0,
            max: 9.0,
        };
        let histogram = Agg {
            count: 3,
            sum: 12.5,
            min: 1.0,
            max: 9.0,
            last: 9.0,
            hist: Some(hist),
        };
        let plain = cpu(0.5);
        store.put(&[
            window(&key, Tier::Raw10s, 10_000, cpu(0.25)),
            window(&key, Tier::Min1, 60_000, histogram.clone()),
            window(&key, Tier::Min1, 120_000, plain.clone()),
            window(&key, Tier::Hour1, 3_600_000, histogram.clone()),
        ]);
        let bare = Event {
            t_ms: 6_000,
            kind: "exit".into(),
            entity_kind: EntityKind::Global,
            entity: "n0".into(),
            duration_ms: None,
            detail: String::new(),
            parts: Vec::new(),
        };
        let wake = Event {
            t_ms: 5_000,
            kind: "wake".into(),
            entity_kind: EntityKind::Vm,
            entity: "r12/3".into(),
            duration_ms: Some(412.0),
            detail: "ready in 412 ms".into(),
            parts: vec![("restore_ms".into(), 160.0), ("ready_ms".into(), 252.0)],
        };
        store.put_events(&[wake.clone(), bare.clone()]);
        let rich = call(7_000);
        let sparse = CallRecord {
            at_ms: 8_000,
            cid: 110,
            ..CallRecord::default()
        };
        store.put_calls(&[rich.clone(), sparse.clone()]);
        let kept = turn(9_000, false, Some(Keep::Error));
        let open_turn = Trace {
            spans: Vec::new(),
            ..turn(9_500, true, None)
        };
        store.put_trace(&kept);
        store.put_trace(&open_turn);
        store.flush();

        let minutes = store.rows(Tier::Min1, "vm.cpu_pct", 0, u64::MAX / 2, &|_| true);
        assert_eq!(
            minutes,
            vec![(
                key.clone(),
                vec![(60_000, histogram.clone()), (120_000, plain)]
            )]
        );
        let hours = store.rows(Tier::Hour1, "vm.cpu_pct", 0, u64::MAX / 2, &|_| true);
        assert_eq!(hours, vec![(key.clone(), vec![(3_600_000, histogram)])]);
        let samples = store.rows(Tier::Raw10s, "vm.cpu_pct", 0, u64::MAX / 2, &|_| true);
        assert_eq!(samples, vec![(key, vec![(10_000, cpu(0.25))])]);
        assert_eq!(
            store.events(0, 10_000, &|_| true, 10),
            vec![bare, wake],
            "newest first, parts in order"
        );
        assert_eq!(store.calls(10_000, 10, &|_| true), vec![sparse, rich]);
        assert_eq!(store.trace("tr-1a2b3c4d"), Some(open_turn.clone()));
        assert_eq!(
            store.traces(0, 20_000, &|_| true, 10),
            vec![open_turn, kept]
        );
    }

    #[test]
    fn reads_see_what_is_waiting_before_a_flush_and_after_it() {
        let files = scratch_store("waiting");
        let store = metrics(&files);
        let key = SeriesKey::new("vm.cpu_pct", EntityKind::Vm, "r1/0");
        store.put(&[window(&key, Tier::Min1, 60_000, cpu(2.0))]);
        store.put_events(&[event(5_000)]);
        store.put_trace(&turn(4_000, true, None));
        let check = |store: &SqliteStore| {
            assert_eq!(
                store
                    .rows(Tier::Min1, "vm.cpu_pct", 0, 100_000, &|_| true)
                    .len(),
                1
            );
            assert_eq!(store.events(0, 10_000, &|_| true, 10).len(), 1);
            assert!(store.trace("tr-1a2b3c4d").is_some());
        };
        check(&store);
        store.flush();
        check(&store);
    }

    #[test]
    fn an_hours_minute_windows_read_back_as_written() {
        let files = scratch_store("minutes");
        let store = metrics(&files);
        let key = SeriesKey::new("vm.cpu_pct", EntityKind::Vm, "r1/0");
        let hour = 1_800_000_000_000 - 1_800_000_000_000 % HOUR_MS;
        // Six 10-minute blocks of 1-minute windows, as the rollup closes them.
        for w in 0..60 {
            store.put(&[window(&key, Tier::Min1, hour + w * 60_000, cpu(w as f64))]);
        }
        store.flush();
        let read = store.rows(Tier::Min1, "vm.cpu_pct", hour, hour + HOUR_MS, &|_| true);
        assert_eq!(read.len(), 1);
        let windows = &read[0].1;
        assert_eq!(windows.len(), 60);
        assert!(windows.iter().enumerate().all(|(w, (start, agg))| {
            *start == hour + w as u64 * 60_000 && agg.last == w as f64
        }));
    }

    #[test]
    fn a_rewritten_window_replaces_its_histogram_buckets() {
        let files = scratch_store("rewrite");
        let store = metrics(&files);
        let key = SeriesKey::new("vm.cpu_pct", EntityKind::Vm, "r1/0");
        let with = |buckets: Vec<(u16, u64)>| Agg {
            hist: Some(HistSnapshot {
                count: buckets.iter().map(|b| b.1).sum(),
                buckets,
                sum: 1.0,
                min: 1.0,
                max: 1.0,
            }),
            ..cpu(1.0)
        };
        store.put(&[window(&key, Tier::Min1, 60_000, with(vec![(1, 1), (2, 4)]))]);
        store.flush();
        store.put(&[window(&key, Tier::Min1, 60_000, with(vec![(7, 2)]))]);
        store.flush();
        let read = store.rows(Tier::Min1, "vm.cpu_pct", 0, 120_000, &|_| true);
        assert_eq!(read[0].1, vec![(60_000, with(vec![(7, 2)]))]);
    }

    #[test]
    fn a_series_prefix_selects_only_its_own_metric() {
        let files = scratch_store("prefix");
        let store = metrics(&files);
        let cpu_key = SeriesKey::new("cpu", EntityKind::Vm, "vm-1");
        let other = SeriesKey::new("cpu_pct", EntityKind::Vm, "vm-1");
        store.put(&[
            window(&cpu_key, Tier::Raw10s, 10_000, cpu(1.0)),
            window(&other, Tier::Raw10s, 10_000, cpu(2.0)),
        ]);
        store.flush();
        let rows = store.rows(Tier::Raw10s, "cpu", 0, 100_000, &|_| true);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, cpu_key);
    }

    #[test]
    fn retention_drops_what_is_past_its_days() {
        let files = scratch_store("retention");
        let store = metrics(&files);
        let now = 1_000 * DAY_MS;
        let key = SeriesKey::new("cpu", EntityKind::Vm, "vm-1");
        store.put(&[
            window(&key, Tier::Min1, now - 20 * DAY_MS, cpu(1.0)),
            window(&key, Tier::Min1, now - 2 * DAY_MS, cpu(2.0)),
            window(&key, Tier::Hour1, now - 500 * DAY_MS, cpu(3.0)),
            window(&key, Tier::Hour1, now - 10 * DAY_MS, cpu(4.0)),
            window(&key, Tier::Raw10s, now - 3 * DAY_MS, cpu(5.0)),
            window(&key, Tier::Raw10s, now - HOUR_MS, cpu(6.0)),
        ]);
        store.put_events(&[event(now - 100 * DAY_MS), event(now - HOUR_MS)]);
        store.put_calls(&[call(now - 100 * DAY_MS), call(now - HOUR_MS)]);
        store.put_trace(&turn(now - 30 * DAY_MS, true, None));
        store.put_trace(&turn(now - HOUR_MS, true, None));
        store.flush();

        store.prune(now, &Retention::default());

        let starts = |tier: Tier| -> Vec<u64> {
            store
                .rows(tier, "cpu", 0, now, &|_| true)
                .iter()
                .flat_map(|(_, windows)| windows.iter().map(|(start, _)| *start))
                .collect()
        };
        assert_eq!(starts(Tier::Min1), vec![now - 2 * DAY_MS]);
        assert_eq!(starts(Tier::Hour1), vec![now - 10 * DAY_MS]);
        assert_eq!(starts(Tier::Raw10s), vec![now - HOUR_MS]);
        assert_eq!(store.events(0, now + 1, &|_| true, 10).len(), 1);
        assert_eq!(store.calls(now + 1, 10, &|_| true).len(), 1);
        assert_eq!(store.traces(0, now + 1, &|_| true, 10).len(), 1);
    }

    #[test]
    fn the_trace_cap_drops_the_oldest_traces_and_keeps_todays() {
        let files = scratch_store("cap");
        let store = metrics(&files);
        // Noon, so that an hour ago is still today.
        let now = 1_000 * DAY_MS + 12 * HOUR_MS;
        store.put_trace(&turn(now - 2 * DAY_MS, true, None));
        store.put_trace(&turn(now - HOUR_MS, true, None));
        store.flush();
        let cap_zero = Retention {
            traces_cap_mb: 0,
            ..Retention::default()
        };
        store.prune(now, &cap_zero);
        let left = store.traces(0, now + 1, &|_| true, 10);
        assert_eq!(left.len(), 1, "the old trace goes, today's stays");
        assert_eq!(left[0].start_ms, now - HOUR_MS);
    }

    #[test]
    fn kept_traces_over_their_cap_lose_the_oldest_but_never_today() {
        // Today's 300 bytes are part of the total and are never on the list of droppable traces.
        let oldest = [500, 400];
        assert_eq!(drop_for_cap(1_200, 1_200, &oldest), 0);
        assert_eq!(drop_for_cap(1_200, 800, &oldest), 1);
        assert_eq!(drop_for_cap(1_200, 300, &oldest), 2);
        assert_eq!(drop_for_cap(1_200, 10, &oldest), 2);
    }

    #[test]
    fn a_batch_that_failed_is_written_by_the_next_flush() {
        let files = scratch_store("restore");
        let store = metrics(&files);
        store.restore(Pending {
            events: vec![event(1_000)],
            ..Pending::default()
        });
        store.put_events(&[event(2_000)]);
        store.flush();
        let times: Vec<u64> = store
            .events(0, 10_000, &|_| true, 10)
            .iter()
            .map(|e| e.t_ms)
            .collect();
        assert_eq!(times, vec![2_000, 1_000]);
    }

    #[test]
    fn saved_values_survive_a_reopen_and_their_tables_are_in_the_migration() {
        let dir = std::env::temp_dir().join(format!("apid-metrics-saved-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let settings = MetricsSettings {
            level: apz_metrics::Level::Basic,
            budgets: vec![super::super::settings::BudgetCap {
                scope: "role:builder".into(),
                cap_usd: 12.5,
            }],
            monthly_budget_usd: Some(300.0),
            basis_days: 30,
            outside: super::super::settings::Outside {
                openmetrics: true,
                openmetrics_token: "tok".into(),
                ..Default::default()
            },
            ..MetricsSettings::default()
        };
        let dashboards = Dashboards {
            scopes: vec!["node:n1".into()],
            pins: vec![apz_metrics_view::model::Pin {
                tab: "overview".into(),
                scope: "global".into(),
                panel: "ttft".into(),
            }],
            recent: vec!["node:n1".into()],
            starter: false,
        };
        let stored = Stored {
            rules: starter_rules(),
            states: [(
                "rule-1\u{1f}global".to_string(),
                apz_metrics::alerts::State::Ok,
            )]
            .into_iter()
            .collect(),
            muted: vec![super::super::alerting::Mute {
                rule: "rule-1".into(),
                until_ms: 9,
            }],
            acked: vec![super::super::alerting::Ack {
                rule: "rule-1".into(),
                subject: "global".into(),
            }],
            history: vec![super::super::alerting::HistoryRow {
                at_ms: 5,
                rule: "rule-1".into(),
                subject: "global".into(),
                what: "fired".into(),
                value: 2.5,
                lasted_ms: Some(60_000),
            }],
        };
        {
            let files = crate::store::open(&dir).expect("the store opens");
            let store = metrics(&files);
            store.put_settings(&settings);
            store.put_dashboards(&dashboards);
            store.put_alerts(&stored);
            assert_eq!(store.settings(), Some(settings.clone()));
        }
        let files = crate::store::open(&dir).expect("the store reopens");
        let store = metrics(&files);
        assert_eq!(store.settings(), Some(settings));
        assert_eq!(store.dashboards(), Some(dashboards));
        assert_eq!(store.alerts(), Some(stored));

        // Every saved value's table is in the migrated file exactly as its definition says.
        let conn = Connection::open_in_memory().unwrap();
        for m in crate::store::FILES
            .iter()
            .find(|f| f.name == crate::store::METRICS)
            .unwrap()
            .migrations
        {
            conn.execute_batch(m.sql).unwrap();
        }
        assert_eq!(
            apz_store::lint::schema(&conn).unwrap(),
            Vec::<String>::new()
        );
        for def in saved_defs() {
            def.validate().unwrap();
            let want = Connection::open_in_memory().unwrap();
            want.execute_batch(&def.ddl()).unwrap();
            for table in def.walk() {
                let have = table_columns(&conn, &table.name);
                assert!(!have.is_empty(), "{} is not in the migration", table.name);
                assert_eq!(have, table_columns(&want, &table.name), "{}", table.name);
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The columns of a table: (name, type, not null, primary key position).
    fn table_columns(conn: &Connection, table: &str) -> Vec<(String, String, bool, i64)> {
        let mut stmt = conn
            .prepare("SELECT name, type, \"notnull\", pk FROM pragma_table_info(?1)")
            .unwrap();
        stmt.query_map([table], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? == 1, r.get(3)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
    }

    #[test]
    fn the_sizes_report_each_stream_table_from_disk() {
        let files = scratch_store("sizes");
        let store = metrics(&files);
        let key = SeriesKey::new("vm.cpu_pct", EntityKind::Vm, "r1/0");
        store.put(&[window(&key, Tier::Raw10s, 10_000, cpu(1.0))]);
        store.flush();
        let sizes = store.sizes();
        assert_eq!(sizes.len(), SIZED.len());
        let sample = sizes.iter().find(|(name, _)| name == "metric_sample");
        assert!(sample.is_some_and(|(_, bytes)| *bytes > 0), "{sizes:?}");
    }
}
