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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::mem;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use apz_metrics::agg::{Agg, Closed, Tier};
use apz_metrics::trace::Trace;
use apz_metrics::{Event, Point, SeriesKey, chunk};
use postgres::types::ToSql;
use serde::de::DeserializeOwned;

use super::settings::Retention;
use super::store::{CallRecord, Store};

const CHUNK_MS: u64 = 600_000;
const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 86_400_000;
/// Rows fetched per page by the readers that filter in Rust.
const PAGE: usize = 500;
/// Every metrics table, for the sizes (metric_kv is the only one that is not partitioned by day).
const TABLES: [&str; 7] = [
    "metric_chunk_10s",
    "metric_1m",
    "metric_1h",
    "metric_events",
    "model_calls",
    "trace_spans",
    "metric_kv",
];

const SQL_CHUNKS: &str = "INSERT INTO metric_chunk_10s (day, series, start_ms, points) \
    SELECT r.d::date, r.s, r.st, r.p FROM unnest($1::text[], $2::text[], $3::bigint[], $4::bytea[]) AS r(d, s, st, p) \
    ON CONFLICT (day, series, start_ms) DO UPDATE SET points = EXCLUDED.points";
const SQL_MINUTES: &str = "INSERT INTO metric_1m (day, series, hour_ms, aggs) \
    SELECT r.d::date, r.s, r.h, r.a FROM unnest($1::text[], $2::text[], $3::bigint[], $4::bytea[]) AS r(d, s, h, a) \
    ON CONFLICT (day, series, hour_ms) DO UPDATE SET aggs = EXCLUDED.aggs";
const SQL_HOURS: &str = "INSERT INTO metric_1h (day, series, day_ms, aggs) \
    SELECT r.d::date, r.s, r.h, r.a FROM unnest($1::text[], $2::text[], $3::bigint[], $4::bytea[]) AS r(d, s, h, a) \
    ON CONFLICT (day, series, day_ms) DO UPDATE SET aggs = EXCLUDED.aggs";
const SQL_EVENTS: &str = "INSERT INTO metric_events (day, t_ms, kind, entity, data) \
    SELECT r.d::date, r.t, r.k, r.e, r.j::jsonb FROM unnest($1::text[], $2::bigint[], $3::text[], $4::text[], $5::text[]) AS r(d, t, k, e, j)";
const SQL_CALLS: &str = "INSERT INTO model_calls (day, at_ms, cid, provider, model, data) \
    SELECT r.d::date, r.t, r.c, r.p, r.m, r.j::jsonb FROM unnest($1::text[], $2::bigint[], $3::bigint[], $4::text[], $5::text[], $6::text[]) AS r(d, t, c, p, m, j)";
const SQL_TRACES: &str = "INSERT INTO trace_spans (day, start_ms, id, cid, data, bytes) \
    SELECT r.d::date, r.t, r.i, r.c, r.j::jsonb, r.b FROM unnest($1::text[], $2::bigint[], $3::text[], $4::bigint[], $5::text[], $6::int[]) AS r(d, t, i, c, j, b)";

#[derive(Default)]
struct Pending {
    /// The open 10-minute chunk of each series: (chunk start, points).
    chunks: HashMap<SeriesKey, (u64, Vec<Point>)>,
    /// Closed chunks waiting to be written.
    chunks_out: Vec<(SeriesKey, u64, Vec<Point>)>,
    /// The 1-minute windows of each series by (series, hour start), re-upserted whole on flush.
    minutes: HashMap<(SeriesKey, u64), Vec<(u64, Agg)>>,
    /// The 1-hour windows of each series by (series, day start), re-upserted whole on flush.
    hours: HashMap<(SeriesKey, u64), Vec<(u64, Agg)>>,
    dirty_minutes: Vec<(SeriesKey, u64)>,
    dirty_hours: Vec<(SeriesKey, u64)>,
    events: Vec<Event>,
    calls: Vec<CallRecord>,
    traces: Vec<Trace>,
}

/// What one flush writes, taken out of the pending state (and put back if the write fails).
struct Batch {
    chunks: Vec<(SeriesKey, u64, Vec<Point>)>,
    minutes: Vec<(SeriesKey, u64, Vec<(u64, Agg)>)>,
    hours: Vec<(SeriesKey, u64, Vec<(u64, Agg)>)>,
    events: Vec<Event>,
    calls: Vec<CallRecord>,
    traces: Vec<Trace>,
    dirty_minutes: Vec<(SeriesKey, u64)>,
    dirty_hours: Vec<(SeriesKey, u64)>,
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

    /// Runs `op` on the connection; on a closed connection, reconnects once and retries.
    fn run<T>(
        &self,
        mut op: impl FnMut(&mut postgres::Client) -> Result<T, postgres::Error>,
    ) -> Result<T, String> {
        let mut client = guard(&self.client);
        match op(&mut *client) {
            Ok(value) => Ok(value),
            Err(e) if e.is_closed() => {
                *client = postgres::Client::connect(&self.url, postgres::NoTls)
                    .map_err(|e| format!("reconnect failed: {e}"))?;
                op(&mut *client).map_err(|e| e.to_string())
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// Makes sure the partition of `table` for `day` exists. Done outside any transaction, so a rolled-back
    /// write never leaves a partition marked as known when it does not exist.
    fn ensure(&self, table: &str, day: i64) -> Result<(), String> {
        let known = (table.to_string(), date_text(day));
        if guard(&self.partitions).contains(&known) {
            return Ok(());
        }
        let sql = format!(
            "CREATE TABLE IF NOT EXISTS {} PARTITION OF {table} FOR VALUES FROM ('{}') TO ('{}')",
            partition_name(table, day),
            date_text(day),
            date_text(day + 1)
        );
        self.run(|c| c.batch_execute(&sql))?;
        guard(&self.partitions).insert(known);
        Ok(())
    }

    /// Runs a read that returns JSON rows (the `data` column) and keeps the ones `keep` accepts, newest first,
    /// fetching pages until `limit` rows are kept or the table is exhausted. `bounds` are the query's own
    /// parameters; the page's LIMIT and OFFSET are the two after them.
    fn scan<T: DeserializeOwned>(
        &self,
        sql: &str,
        bounds: &[&(dyn ToSql + Sync)],
        limit: usize,
        keep: &dyn Fn(&T) -> bool,
    ) -> Vec<T> {
        let mut out = Vec::new();
        let mut offset: i64 = 0;
        let page = PAGE as i64;
        while out.len() < limit {
            let rows = match self.run(|c| {
                let mut params: Vec<&(dyn ToSql + Sync)> = bounds.to_vec();
                params.push(&page);
                params.push(&offset);
                c.query(sql, &params)
            }) {
                Ok(rows) => rows,
                Err(e) => {
                    eprintln!("metrics: read failed ({e})");
                    break;
                }
            };
            let fetched = rows.len();
            for row in rows {
                let Ok(item) = serde_json::from_value::<T>(row.get::<_, serde_json::Value>(0))
                else {
                    continue;
                };
                if keep(&item) {
                    out.push(item);
                    if out.len() == limit {
                        break;
                    }
                }
            }
            if fetched < PAGE {
                break;
            }
            offset += fetched as i64;
        }
        out
    }

    /// The stored rows of `table` for `metric` whose `col` lies in `[from_ms - span, to_ms)`: (series, payload).
    #[allow(clippy::too_many_arguments)]
    fn stored_rows(
        &self,
        table: &str,
        col: &str,
        payload: &str,
        span: u64,
        metric: &str,
        from_ms: u64,
        to_ms: u64,
    ) -> Vec<(String, Vec<u8>)> {
        let sql = format!(
            "SELECT series, {payload} FROM {table} WHERE day BETWEEN $1::text::date AND $2::text::date \
             AND {col} >= $3 AND {col} < $4 AND series LIKE $5"
        );
        let lo_ms = from_ms.saturating_sub(span);
        let lo_day = date_text(day_of(lo_ms));
        let hi_day = date_text(day_of(to_ms.saturating_sub(1)));
        let (lo, hi) = (lo_ms as i64, to_ms as i64);
        let pattern = like_pattern(metric);
        match self.run(|c| c.query(sql.as_str(), &[&lo_day, &hi_day, &lo, &hi, &pattern])) {
            Ok(rows) => rows
                .iter()
                .map(|row| (row.get::<_, String>(0), row.get::<_, Vec<u8>>(1)))
                .collect(),
            Err(e) => {
                eprintln!("metrics: reading {table} failed ({e})");
                Vec::new()
            }
        }
    }

    /// Takes everything a flush writes out of the pending state.
    fn take_batch(&self) -> Batch {
        let mut p = guard(&self.pending);
        let dirty_minutes = take_dirty(&mut p.dirty_minutes);
        let dirty_hours = take_dirty(&mut p.dirty_hours);
        let minutes = dirty_minutes
            .iter()
            .filter_map(|(k, h)| {
                p.minutes
                    .get(&(k.clone(), *h))
                    .map(|ws| (k.clone(), *h, ws.clone()))
            })
            .collect();
        let hours = dirty_hours
            .iter()
            .filter_map(|(k, d)| {
                p.hours
                    .get(&(k.clone(), *d))
                    .map(|ws| (k.clone(), *d, ws.clone()))
            })
            .collect();
        Batch {
            chunks: mem::take(&mut p.chunks_out),
            minutes,
            hours,
            events: mem::take(&mut p.events),
            calls: mem::take(&mut p.calls),
            traces: mem::take(&mut p.traces),
            dirty_minutes,
            dirty_hours,
        }
    }

    /// Puts a failed batch back in front of what was pending since.
    fn restore(&self, batch: Batch) {
        let mut p = guard(&self.pending);
        prepend(&mut p.chunks_out, batch.chunks);
        prepend(&mut p.events, batch.events);
        prepend(&mut p.calls, batch.calls);
        prepend(&mut p.traces, batch.traces);
        p.dirty_minutes.extend(batch.dirty_minutes);
        p.dirty_hours.extend(batch.dirty_hours);
    }

    /// Writes a batch in one transaction, after making the partitions it needs.
    fn write(&self, b: &Batch) -> Result<(), String> {
        for (_, start, _) in &b.chunks {
            self.ensure("metric_chunk_10s", day_of(*start))?;
        }
        for (_, hour, _) in &b.minutes {
            self.ensure("metric_1m", day_of(*hour))?;
        }
        for (_, day, _) in &b.hours {
            self.ensure("metric_1h", day_of(*day))?;
        }
        for e in &b.events {
            self.ensure("metric_events", day_of(e.t_ms))?;
        }
        for c in &b.calls {
            self.ensure("model_calls", day_of(c.at_ms))?;
        }
        for t in &b.traces {
            self.ensure("trace_spans", day_of(t.start_ms))?;
        }

        let chunk_days: Vec<String> = b
            .chunks
            .iter()
            .map(|(_, s, _)| date_text(day_of(*s)))
            .collect();
        let chunk_series: Vec<String> = b.chunks.iter().map(|(k, _, _)| k.id()).collect();
        let chunk_starts: Vec<i64> = b.chunks.iter().map(|(_, s, _)| *s as i64).collect();
        // The values are quantized first, as the chunk format is built for: the low mantissa bits are dropped.
        let chunk_points: Vec<Vec<u8>> = b
            .chunks
            .iter()
            .map(|(_, _, p)| {
                let quantized: Vec<(u64, f64)> =
                    p.iter().map(|&(t, v)| (t, chunk::quantize(v))).collect();
                chunk::encode(&quantized)
            })
            .collect();

        let minute_days: Vec<String> = b
            .minutes
            .iter()
            .map(|(_, h, _)| date_text(day_of(*h)))
            .collect();
        let minute_series: Vec<String> = b.minutes.iter().map(|(k, _, _)| k.id()).collect();
        let minute_hours: Vec<i64> = b.minutes.iter().map(|(_, h, _)| *h as i64).collect();
        let minute_aggs: Vec<Vec<u8>> = b
            .minutes
            .iter()
            .map(|(_, _, ws)| pack_windows(ws))
            .collect();

        let hour_days: Vec<String> = b
            .hours
            .iter()
            .map(|(_, d, _)| date_text(day_of(*d)))
            .collect();
        let hour_series: Vec<String> = b.hours.iter().map(|(k, _, _)| k.id()).collect();
        let hour_starts: Vec<i64> = b.hours.iter().map(|(_, d, _)| *d as i64).collect();
        let hour_aggs: Vec<Vec<u8>> = b.hours.iter().map(|(_, _, ws)| pack_windows(ws)).collect();

        let event_days: Vec<String> = b.events.iter().map(|e| date_text(day_of(e.t_ms))).collect();
        let event_times: Vec<i64> = b.events.iter().map(|e| e.t_ms as i64).collect();
        let event_kinds: Vec<String> = b.events.iter().map(|e| e.kind.clone()).collect();
        let event_entities: Vec<String> = b.events.iter().map(|e| e.entity.clone()).collect();
        let event_data: Vec<String> = b
            .events
            .iter()
            .map(|e| serde_json::to_string(e).expect("an event serializes to JSON"))
            .collect();

        let call_days: Vec<String> = b.calls.iter().map(|c| date_text(day_of(c.at_ms))).collect();
        let call_times: Vec<i64> = b.calls.iter().map(|c| c.at_ms as i64).collect();
        let call_cids: Vec<i64> = b.calls.iter().map(|c| c.cid as i64).collect();
        let call_providers: Vec<String> = b.calls.iter().map(|c| c.provider.clone()).collect();
        let call_models: Vec<String> = b.calls.iter().map(|c| c.model.clone()).collect();
        let call_data: Vec<String> = b
            .calls
            .iter()
            .map(|c| serde_json::to_string(c).expect("a call serializes to JSON"))
            .collect();

        let trace_days: Vec<String> = b
            .traces
            .iter()
            .map(|t| date_text(day_of(t.start_ms)))
            .collect();
        let trace_starts: Vec<i64> = b.traces.iter().map(|t| t.start_ms as i64).collect();
        let trace_ids: Vec<String> = b.traces.iter().map(|t| t.id.clone()).collect();
        let trace_cids: Vec<i64> = b.traces.iter().map(|t| t.cid as i64).collect();
        let trace_data: Vec<String> = b
            .traces
            .iter()
            .map(|t| serde_json::to_string(t).expect("a trace serializes to JSON"))
            .collect();
        let trace_bytes: Vec<i32> = trace_data
            .iter()
            .map(|d| i32::try_from(d.len()).unwrap_or(i32::MAX))
            .collect();

        self.run(|c| {
            let mut tx = c.transaction()?;
            if !b.chunks.is_empty() {
                tx.execute(
                    SQL_CHUNKS,
                    &[&chunk_days, &chunk_series, &chunk_starts, &chunk_points],
                )?;
            }
            if !b.minutes.is_empty() {
                tx.execute(
                    SQL_MINUTES,
                    &[&minute_days, &minute_series, &minute_hours, &minute_aggs],
                )?;
            }
            if !b.hours.is_empty() {
                tx.execute(
                    SQL_HOURS,
                    &[&hour_days, &hour_series, &hour_starts, &hour_aggs],
                )?;
            }
            if !b.events.is_empty() {
                tx.execute(
                    SQL_EVENTS,
                    &[
                        &event_days,
                        &event_times,
                        &event_kinds,
                        &event_entities,
                        &event_data,
                    ],
                )?;
            }
            if !b.calls.is_empty() {
                tx.execute(
                    SQL_CALLS,
                    &[
                        &call_days,
                        &call_times,
                        &call_cids,
                        &call_providers,
                        &call_models,
                        &call_data,
                    ],
                )?;
            }
            if !b.traces.is_empty() {
                tx.execute(
                    SQL_TRACES,
                    &[
                        &trace_days,
                        &trace_starts,
                        &trace_ids,
                        &trace_cids,
                        &trace_data,
                        &trace_bytes,
                    ],
                )?;
            }
            tx.commit()
        })
    }

    /// Drops the in-memory minute and hour windows of periods that are over, once they are written. An entry
    /// that is dirty again stays, as does a period of the current hour or day.
    fn drop_old_windows(&self, now: u64) {
        let (hour, day) = (now - now % HOUR_MS, now - now % DAY_MS);
        let mut p = guard(&self.pending);
        let dirty: HashSet<(SeriesKey, u64)> = p.dirty_minutes.iter().cloned().collect();
        p.minutes.retain(|id, _| id.1 >= hour || dirty.contains(id));
        let dirty: HashSet<(SeriesKey, u64)> = p.dirty_hours.iter().cloned().collect();
        p.hours.retain(|id, _| id.1 >= day || dirty.contains(id));
    }

    /// Drops the partitions of `table` for days before `cutoff_day`.
    fn drop_partitions_before(&self, table: &str, cutoff_day: i64) {
        let names: Vec<String> = match self.run(|c| {
            c.query(
                "SELECT c.relname::text FROM pg_inherits i \
                 JOIN pg_class c ON c.oid = i.inhrelid JOIN pg_class p ON p.oid = i.inhparent \
                 WHERE p.relname::text = $1",
                &[&table.to_string()],
            )
        }) {
            Ok(rows) => rows.iter().map(|row| row.get::<_, String>(0)).collect(),
            Err(e) => {
                eprintln!("metrics: listing {table} partitions failed ({e})");
                return;
            }
        };
        let prefix = format!("{table}_");
        let cutoff = yyyymmdd(cutoff_day);
        for name in names {
            let Some(day) = name
                .strip_prefix(&prefix)
                .filter(|s| s.len() == 8)
                .and_then(|s| s.parse::<i64>().ok())
            else {
                continue;
            };
            if day >= cutoff {
                continue;
            }
            if let Err(e) = self.run(|c| c.batch_execute(&format!("DROP TABLE IF EXISTS {name}"))) {
                eprintln!("metrics: dropping {name} failed ({e})");
            }
        }
        let cutoff_text = date_text(cutoff_day);
        guard(&self.partitions).retain(|(t, d)| t != table || *d >= cutoff_text);
    }
}

impl PgStore {
    /// Drops the oldest days of kept traces while all of them together are larger than `cap` bytes; today's
    /// partition is always kept.
    fn cap_traces(&self, cap: u64) {
        let rows = match self.run(|c| {
            c.query(
                "SELECT c.relname::text, pg_total_relation_size(c.oid)::bigint FROM pg_inherits i \
                 JOIN pg_class c ON c.oid = i.inhrelid JOIN pg_class p ON p.oid = i.inhparent \
                 WHERE p.relname::text = 'trace_spans'",
                &[],
            )
        }) {
            Ok(rows) => rows,
            Err(e) => {
                eprintln!("metrics: sizing trace_spans partitions failed ({e})");
                return;
            }
        };
        let days: Vec<(i64, u64)> = rows
            .iter()
            .filter_map(|row| {
                let name: String = row.get(0);
                let day = name.strip_prefix("trace_spans_")?.parse::<i64>().ok()?;
                Some((day, row.get::<_, i64>(1).max(0) as u64))
            })
            .collect();
        for day in over_cap(days, cap) {
            let name = format!("trace_spans_{day}");
            if let Err(e) = self.run(|c| c.batch_execute(&format!("DROP TABLE IF EXISTS {name}"))) {
                eprintln!("metrics: dropping {name} failed ({e})");
                continue;
            }
            let text = format!("{:04}-{:02}-{:02}", day / 10_000, day / 100 % 100, day % 100);
            guard(&self.partitions).retain(|(t, d)| t != "trace_spans" || *d != text);
        }
    }
}

/// The days (`YYYYMMDD`) to drop, oldest first, so that the remaining partitions fit in `cap` bytes; the newest is
/// never dropped.
fn over_cap(mut days: Vec<(i64, u64)>, cap: u64) -> Vec<i64> {
    days.sort_unstable();
    let mut total: u64 = days.iter().map(|(_, bytes)| bytes).sum();
    let mut drop = Vec::new();
    for &(day, bytes) in days.iter().take(days.len().saturating_sub(1)) {
        if total <= cap {
            break;
        }
        total -= bytes;
        drop.push(day);
    }
    drop
}

impl Store for PgStore {
    fn put(&self, closed: &[Closed]) {
        let now = now_ms();
        let (now_hour, now_day) = (now - now % HOUR_MS, now - now % DAY_MS);
        let mut p = guard(&self.pending);
        for window in closed {
            let key = &window.key;
            match window.tier {
                Tier::Raw10s => {
                    if window.agg.hist.is_some() {
                        continue;
                    }
                    let chunk = window.start_ms - window.start_ms % CHUNK_MS;
                    let point = (window.start_ms, window.agg.last);
                    match p.chunks.get_mut(key) {
                        Some((start, points)) if *start == chunk => points.push(point),
                        // A window of a chunk already closed. Windows close in time order, so this is rare.
                        Some((start, _)) if *start > chunk => {}
                        _ => {
                            if let Some((start, points)) =
                                p.chunks.insert(key.clone(), (chunk, vec![point]))
                            {
                                p.chunks_out.push((key.clone(), start, points));
                            }
                        }
                    }
                }
                Tier::Min1 => {
                    let hour = window.start_ms - window.start_ms % HOUR_MS;
                    let id = (key.clone(), hour);
                    // An hour that is over with no entry left would be written as a row of its late windows
                    // only, replacing the stored row. Such a window is dropped.
                    if hour < now_hour && !p.minutes.contains_key(&id) {
                        continue;
                    }
                    upsert_window(
                        p.minutes.entry(id.clone()).or_default(),
                        window.start_ms,
                        &window.agg,
                    );
                    p.dirty_minutes.push(id);
                }
                Tier::Hour1 => {
                    let day = window.start_ms - window.start_ms % DAY_MS;
                    let id = (key.clone(), day);
                    if day < now_day && !p.hours.contains_key(&id) {
                        continue;
                    }
                    upsert_window(
                        p.hours.entry(id.clone()).or_default(),
                        window.start_ms,
                        &window.agg,
                    );
                    p.dirty_hours.push(id);
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
        let inside = |start: u64| start >= from_ms && start < to_ms;
        let mut found: BTreeMap<SeriesKey, BTreeMap<u64, Agg>> = BTreeMap::new();
        match tier {
            Tier::Raw10s => {
                for (series, bytes) in self.stored_rows(
                    "metric_chunk_10s",
                    "start_ms",
                    "points",
                    CHUNK_MS,
                    metric,
                    from_ms,
                    to_ms,
                ) {
                    let Some(key) = SeriesKey::parse(&series).filter(|k| keep(k)) else {
                        continue;
                    };
                    let Ok(points) = chunk::decode(&bytes) else {
                        eprintln!("metrics: a stored chunk of {series} does not decode");
                        continue;
                    };
                    for (t, v) in points {
                        if inside(t) {
                            found
                                .entry(key.clone())
                                .or_default()
                                .insert(t, point_agg(v));
                        }
                    }
                }
                // Points not yet written: the closed chunks waiting for a flush and the open chunks.
                let pending: Vec<(SeriesKey, Vec<Point>)> = {
                    let p = guard(&self.pending);
                    p.chunks
                        .iter()
                        .map(|(k, (_, points))| (k.clone(), points.clone()))
                        .chain(
                            p.chunks_out
                                .iter()
                                .map(|(k, _, points)| (k.clone(), points.clone())),
                        )
                        .collect()
                };
                for (key, points) in pending {
                    if key.metric != metric || !keep(&key) {
                        continue;
                    }
                    for (t, v) in points {
                        if inside(t) {
                            found
                                .entry(key.clone())
                                .or_default()
                                .insert(t, point_agg(v));
                        }
                    }
                }
            }
            Tier::Min1 | Tier::Hour1 => {
                let (table, col, span) = if tier == Tier::Min1 {
                    ("metric_1m", "hour_ms", HOUR_MS)
                } else {
                    ("metric_1h", "day_ms", DAY_MS)
                };
                for (series, bytes) in
                    self.stored_rows(table, col, "aggs", span, metric, from_ms, to_ms)
                {
                    let Some(key) = SeriesKey::parse(&series).filter(|k| keep(k)) else {
                        continue;
                    };
                    let Some(windows) = unpack_windows(&bytes) else {
                        eprintln!("metrics: a stored row of {series} does not unpack");
                        continue;
                    };
                    for (start, agg) in windows {
                        if inside(start) {
                            found.entry(key.clone()).or_default().insert(start, agg);
                        }
                    }
                }
                let pending: Vec<(SeriesKey, Vec<(u64, Agg)>)> = {
                    let p = guard(&self.pending);
                    let map = if tier == Tier::Min1 {
                        &p.minutes
                    } else {
                        &p.hours
                    };
                    map.iter()
                        .map(|((k, _), ws)| (k.clone(), ws.clone()))
                        .collect()
                };
                for (key, windows) in pending {
                    if key.metric != metric || !keep(&key) {
                        continue;
                    }
                    for (start, agg) in windows {
                        if inside(start) {
                            found.entry(key.clone()).or_default().insert(start, agg);
                        }
                    }
                }
            }
        }
        found
            .into_iter()
            .map(|(key, windows)| (key, windows.into_iter().collect()))
            .collect()
    }

    fn put_events(&self, events: &[Event]) {
        guard(&self.pending).events.extend(events.iter().cloned());
    }

    fn events(
        &self,
        from_ms: u64,
        to_ms: u64,
        keep: &dyn Fn(&Event) -> bool,
        limit: usize,
    ) -> Vec<Event> {
        let days = [
            date_text(day_of(from_ms)),
            date_text(day_of(to_ms.saturating_sub(1))),
        ];
        let (lo, hi) = (from_ms as i64, to_ms as i64);
        self.scan(
            "SELECT data FROM metric_events WHERE day BETWEEN $1::text::date AND $2::text::date \
             AND t_ms >= $3 AND t_ms < $4 ORDER BY t_ms DESC LIMIT $5 OFFSET $6",
            &[&days[0], &days[1], &lo, &hi],
            limit,
            keep,
        )
    }

    fn put_calls(&self, calls: &[CallRecord]) {
        guard(&self.pending).calls.extend(calls.iter().cloned());
    }

    fn calls(
        &self,
        before_ms: u64,
        limit: usize,
        keep: &dyn Fn(&CallRecord) -> bool,
    ) -> Vec<CallRecord> {
        let before = before_ms as i64;
        self.scan(
            "SELECT data FROM model_calls WHERE at_ms < $1 ORDER BY at_ms DESC LIMIT $2 OFFSET $3",
            &[&before],
            limit,
            keep,
        )
    }

    fn put_trace(&self, trace: &Trace) {
        guard(&self.pending).traces.push(trace.clone());
    }

    fn traces(
        &self,
        from_ms: u64,
        to_ms: u64,
        keep: &dyn Fn(&Trace) -> bool,
        limit: usize,
    ) -> Vec<Trace> {
        let days = [
            date_text(day_of(from_ms)),
            date_text(day_of(to_ms.saturating_sub(1))),
        ];
        let (lo, hi) = (from_ms as i64, to_ms as i64);
        self.scan(
            "SELECT data FROM trace_spans WHERE day BETWEEN $1::text::date AND $2::text::date \
             AND start_ms >= $3 AND start_ms < $4 ORDER BY start_ms DESC LIMIT $5 OFFSET $6",
            &[&days[0], &days[1], &lo, &hi],
            limit,
            keep,
        )
    }

    fn trace(&self, id: &str) -> Option<Trace> {
        match self.run(|c| {
            c.query_opt(
                "SELECT data FROM trace_spans WHERE id = $1 LIMIT 1",
                &[&id.to_string()],
            )
        }) {
            Ok(Some(row)) => serde_json::from_value(row.get::<_, serde_json::Value>(0)).ok(),
            Ok(None) => None,
            Err(e) => {
                eprintln!("metrics: reading trace {id} failed ({e})");
                None
            }
        }
    }

    fn kv_get(&self, key: &str) -> Option<String> {
        match self.run(|c| {
            c.query_opt(
                "SELECT value FROM metric_kv WHERE key = $1",
                &[&key.to_string()],
            )
        }) {
            Ok(row) => row.map(|r| r.get::<_, String>(0)),
            Err(e) => {
                eprintln!("metrics: reading saved value {key} failed ({e})");
                None
            }
        }
    }

    fn kv_put(&self, key: &str, json: &str) {
        let (key, json, at) = (key.to_string(), json.to_string(), now_ms() as i64);
        if let Err(e) = self.run(|c| {
            c.execute(
                "INSERT INTO metric_kv (key, value, at_ms) VALUES ($1, $2, $3) \
                 ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, at_ms = EXCLUDED.at_ms",
                &[&key, &json, &at],
            )
        }) {
            eprintln!("metrics: saving {key} failed ({e})");
        }
    }

    fn prune(&self, now_ms: u64, retention: &Retention) {
        let today = day_of(now_ms);
        let days = |n: u32| today - i64::from(n);
        let cutoffs = [
            (
                "metric_chunk_10s",
                day_of(now_ms.saturating_sub(u64::from(retention.chunks_hours) * HOUR_MS)),
            ),
            ("metric_1m", days(retention.minute_days)),
            ("metric_1h", days(retention.hour_days)),
            ("model_calls", days(retention.calls_days)),
            ("trace_spans", days(retention.traces_days)),
            ("metric_events", days(retention.events_days)),
        ];
        for (table, cutoff) in cutoffs {
            self.drop_partitions_before(table, cutoff);
        }
        self.cap_traces(u64::from(retention.traces_cap_mb) * 1024 * 1024);
    }

    fn sizes(&self) -> Vec<(String, u64)> {
        TABLES
            .iter()
            .map(|table| {
                let name = table.to_string();
                let pattern = format!("^{table}_[0-9]{{8}}$");
                let bytes = match self.run(|c| {
                    c.query_one(
                        "SELECT COALESCE(SUM(pg_total_relation_size(c.oid)), 0)::bigint FROM pg_class c \
                         WHERE c.relname::text = $1 OR c.relname::text ~ $2",
                        &[&name, &pattern],
                    )
                }) {
                    Ok(row) => row.get::<_, i64>(0),
                    Err(e) => {
                        eprintln!("metrics: sizing {name} failed ({e})");
                        0
                    }
                };
                (name, bytes.max(0) as u64)
            })
            .collect()
    }

    fn flush(&self) {
        let now = now_ms();
        let batch = self.take_batch();
        if let Err(e) = self.write(&batch) {
            eprintln!("metrics: flush failed ({e}); the rows wait for the next flush");
            self.restore(batch);
            return;
        }
        self.drop_old_windows(now);
    }

    fn kind(&self) -> &'static str {
        "postgres"
    }
}

/// Locks a mutex, taking the value over a poisoned lock.
fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The day number (days since 1970-01-01, UTC) of a time in milliseconds.
fn day_of(ms: u64) -> i64 {
    (ms / DAY_MS) as i64
}

/// The calendar date of a day number (days since 1970-01-01), proleptic Gregorian.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe as i64 + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// `YYYY-MM-DD` for a day number.
fn date_text(day: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `YYYYMMDD` as a number, for comparing a partition's day with a cutoff.
fn yyyymmdd(day: i64) -> i64 {
    let (y, m, d) = civil_from_days(day);
    y * 10_000 + i64::from(m) * 100 + i64::from(d)
}

/// The partition of `table` for `day`: `table_yyyymmdd`.
fn partition_name(table: &str, day: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{table}_{y:04}{m:02}{d:02}")
}

/// The LIKE pattern of the series ids of `metric` (`metric|...`), with the pattern's own wildcards escaped.
fn like_pattern(metric: &str) -> String {
    let mut out = String::with_capacity(metric.len() + 2);
    for ch in metric.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push_str("|%");
    out
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

/// Replaces the window that starts at `start`, or adds it.
fn upsert_window(windows: &mut Vec<(u64, Agg)>, start: u64, agg: &Agg) {
    match windows.iter_mut().find(|w| w.0 == start) {
        Some(existing) => existing.1 = agg.clone(),
        None => windows.push((start, agg.clone())),
    }
}

/// The windows as deflate-compressed JSON, sorted by start.
fn pack_windows(windows: &[(u64, Agg)]) -> Vec<u8> {
    let mut sorted = windows.to_vec();
    sorted.sort_by_key(|w| w.0);
    let json = serde_json::to_vec(&sorted).expect("windows serialize to JSON");
    let mut encoder =
        flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&json)
        .expect("writing to a Vec cannot fail");
    encoder.finish().expect("writing to a Vec cannot fail")
}

/// The windows of [`pack_windows`]; `None` when the bytes are not a packed row.
fn unpack_windows(bytes: &[u8]) -> Option<Vec<(u64, Agg)>> {
    let mut json = Vec::new();
    flate2::read::DeflateDecoder::new(bytes)
        .read_to_end(&mut json)
        .ok()?;
    serde_json::from_slice(&json).ok()
}

/// Takes the dirty keys out, sorted and without repeats.
fn take_dirty(keys: &mut Vec<(SeriesKey, u64)>) -> Vec<(SeriesKey, u64)> {
    let mut taken = mem::take(keys);
    taken.sort();
    taken.dedup();
    taken
}

/// Puts `older` in front of `queue`.
fn prepend<T>(queue: &mut Vec<T>, mut older: Vec<T>) {
    older.append(queue);
    *queue = older;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_traces_over_their_cap_lose_the_oldest_days_but_never_today() {
        let days = vec![(20261016, 300), (20261014, 500), (20261015, 400)];
        assert_eq!(over_cap(days.clone(), 1_200), Vec::<i64>::new());
        assert_eq!(over_cap(days.clone(), 800), vec![20261014]);
        assert_eq!(over_cap(days.clone(), 300), vec![20261014, 20261015]);
        // Even a cap today alone exceeds keeps today.
        assert_eq!(over_cap(days, 10), vec![20261014, 20261015]);
    }

    fn window(count: u64, value: f64) -> Agg {
        Agg {
            count,
            sum: value,
            min: value,
            max: value,
            last: value,
            hist: None,
        }
    }

    #[test]
    fn a_minute_row_survives_its_packing_round_trip() {
        let windows = vec![(120_000, window(2, 5.0)), (60_000, window(1, 1.5))];
        let unpacked = unpack_windows(&pack_windows(&windows)).expect("a packed row unpacks");
        assert_eq!(
            unpacked,
            vec![(60_000, window(1, 1.5)), (120_000, window(2, 5.0))]
        );
    }

    #[test]
    fn a_day_partition_is_named_after_its_date() {
        let day = 20_742; // 2026-10-16
        assert_eq!(date_text(day), "2026-10-16");
        assert_eq!(partition_name("metric_1m", day), "metric_1m_20261016");
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }
}
