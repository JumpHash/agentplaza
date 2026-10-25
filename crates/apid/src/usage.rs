//! The model-usage ledger: every model call an agent makes (through the harness or the gateway
//! proxy), with its tokens, cache hits and cost, so the owner can see what the plaza spends.
//!
//! The ledger is a stream in `plaza.db`, written through as each call is metered and read by the
//! queries in `sql/queries/usage_*.sql`:
//! - `usage_call`: one row per call (the newest kept: `settings.retention.usage_calls_max`);
//! - `usage_hour`: per-hour totals by (hour, model, cid), kept for `settings.retention.usage_days`
//!   (0 = forever). Series and summaries are read from the hours, so they cover the whole kept
//!   period even after the call rows are gone;
//! - `usage_work`: the totals of every work item that made a metered call, by (cid, work), capped at
//!   [`WORKS_MAX`] rows (the newest work items kept).
//!
//! The one component (`usage_state`) keeps only the counter the next call's id comes from.
//!
//! Cost: `cost_usd` is what the provider reported for the call (OpenRouter's `usage.cost`), when
//! it did; `est_usd` is the catalog's estimate ([`crate::prices`]) at the time of the call
//! (0 when no price was known). Totals sum `cost_usd` where present and `est_usd` otherwise
//! (`usd`), and also report both sums separately.
//!
//! GET /v1/usage/summary?from=&to=        → Summary (default: the last 30 days)
//! GET /v1/usage/series?from=&to=&bucket=hour|day&by=model|agent|role|provider → Series
//! GET /v1/usage/calls?before=&limit=&model=&cid=  → CallsPage (newest first; limit 1..=200, default 50)
//!
//! `from`/`to` are unix milliseconds; `before` is a call id.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use apz_http::{Request, Response, RouteDoc, Router};
use apz_store::rusqlite::{Connection, OptionalExtension, Row, params};
use apz_store::table::Value;
use apz_store::{RowSet, StoreError, Table, TableDef};
use serde::{Deserialize, Serialize};

use crate::prices::Tokens;
use crate::state::App;
use crate::store::sql;

pub const HOUR_MS: u64 = 60 * 60 * 1000;
pub const DAY_MS: u64 = 24 * HOUR_MS;
pub const CALLS_PAGE: usize = 50;
pub const CALLS_LIMIT: usize = 200;
/// The most points one series returns; a longer window starts later than asked.
pub const SERIES_MAX_POINTS: usize = 2000;
/// The most work items whose usage the ledger keeps.
pub const WORKS_MAX: usize = 20_000;

/// One model call (a row of `usage_call`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "usage_call")]
pub struct Call {
    /// Assigned by the ledger, increasing from 1.
    #[table(key)]
    pub id: u64,
    #[table(index)]
    pub at_ms: u64,
    pub cid: u64,
    pub role: String,
    /// The plaza model id, e.g. `anthropic/claude-opus-5-5`.
    pub model: String,
    /// The provider id that served it (the part before the first `/` when unknown).
    pub provider: String,
    #[serde(flatten)]
    #[table(flatten)]
    pub tokens: Tokens,
    /// What the provider reported the call cost, in US dollars.
    #[serde(default)]
    pub cost_usd: Option<f64>,
    /// The catalog's estimate, in US dollars.
    pub est_usd: f64,
    /// The task the call was charged to, if any.
    #[serde(default)]
    #[table(index)]
    pub task: Option<String>,
    /// The work item the call ran under, 0 for none. The gateway's engine calls carry theirs, so
    /// [`Ledger::work_totals`] can give a work's usage.
    #[serde(default)]
    pub work: u64,
    /// "harness" (the agent's own loop) or "gateway" (the proxy agents' tools call through).
    pub via: String,
    /// The rung of the model that served the call ("cheap", "mid" or "frontier"), from its output
    /// price; absent when no price is known and on records saved before rungs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rung: Option<String>,
    /// Whether the call escalated its task: an earlier call of the same task ran on a lower rung.
    /// Set when the call is filed.
    #[serde(default)]
    pub escalated: bool,
}

/// Totals over some calls.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct Totals {
    pub calls: u64,
    #[serde(flatten)]
    #[table(flatten)]
    pub tokens: Tokens,
    /// Sum of provider-reported costs.
    pub cost_usd: f64,
    /// Sum of estimates.
    pub est_usd: f64,
    /// Reported cost where known, the estimate otherwise.
    pub usd: f64,
    /// cache_read / (input + cache_read + cache_write); 0 when there was no input. Computed from the
    /// tokens when totals are read, so it is not stored.
    #[table(skip)]
    pub cache_hit_rate: f64,
}

/// One hour's totals for one model and one agent (a row of `usage_hour`, written as calls are metered).
#[derive(Debug, Clone, Default, PartialEq, apz_store::Table)]
#[table(name = "usage_hour")]
pub struct HourRow {
    #[table(key)]
    pub hour_ms: u64,
    #[table(key)]
    pub model: String,
    #[table(key)]
    pub cid: u64,
    pub provider: String,
    pub role: String,
    #[table(flatten)]
    pub totals: Totals,
}

/// One work item's usage: the calls made under it that carry a work id (a row of `usage_work`).
#[derive(Debug, Clone, Default, PartialEq, apz_store::Table)]
#[table(name = "usage_work")]
pub struct WorkRow {
    #[table(key)]
    pub cid: u64,
    #[table(key, index)]
    pub work: u64,
    #[table(flatten)]
    pub totals: Totals,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Group {
    /// The model id, the cid (as a string), the role or the provider.
    pub key: String,
    pub totals: Totals,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub from_ms: u64,
    pub to_ms: u64,
    pub totals: Totals,
    pub by_model: Vec<Group>,
    pub by_agent: Vec<Group>,
    pub by_role: Vec<Group>,
    pub by_provider: Vec<Group>,
    /// Models with calls but no known price (their estimate is 0).
    pub unpriced: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    /// The bucket's start, unix ms.
    pub at_ms: u64,
    /// Per key of the grouping.
    pub groups: BTreeMap<String, Totals>,
    pub totals: Totals,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Series {
    pub from_ms: u64,
    pub to_ms: u64,
    pub bucket_ms: u64,
    pub by: String,
    /// Every bucket in [from, to), empty ones included, oldest first.
    pub points: Vec<Point>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CallsPage {
    pub calls: Vec<Call>,
    pub total: usize,
    pub next_before: Option<u64>,
}

/// The usage component's one row: the id the next call gets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "usage_state")]
pub struct UsageState {
    pub next_id: u64,
}

/// The ledger: writes calls, hours and work items through `plaza.db` and reads them back from it. Its only
/// memory is the id counter, which the usage component saves.
pub struct Ledger {
    db: Arc<apz_store::Db>,
    /// The id the last call got. Held across each call's write, so ids and commits go in the same order.
    last_id: Mutex<u64>,
}

impl Ledger {
    /// A ledger over the hub's `plaza.db`; its counter starts at 0 until [`Ledger::load`].
    pub fn new(db: Arc<apz_store::Db>) -> Ledger {
        Ledger {
            db,
            last_id: Mutex::new(0),
        }
    }

    fn last(&self) -> MutexGuard<'_, u64> {
        self.last_id.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Files `call` (its `id` is assigned here, its `at_ms` kept), publishes `usage` on the bus with the
    /// stored call, and returns the id (0 when the store refused the call, which is logged).
    pub fn record(&self, app: &App, call: Call) -> u64 {
        match self.file(call) {
            Ok(stored) => {
                app.bus.publish("usage", &stored);
                stored.id
            }
            Err(e) => {
                eprintln!("apid: usage: a model call was not recorded: {e}");
                0
            }
        }
    }

    /// The storing half of [`Ledger::record`]: the call, its hour and its work item are written in one
    /// transaction, which also decides the escalation. No bus, so tests can file calls without an app.
    fn file(&self, call: Call) -> Result<Call, StoreError> {
        let mut last = self.last();
        let id = *last + 1;
        let call = Call { id, ..call };
        let written = self.db.write(move |conn| {
            let mut call = call;
            call.escalated = escalates(conn, &call)?;
            insert_call(conn, &call)?;
            add_to_hour(conn, &call)?;
            if call.work > 0 {
                add_to_work(conn, &call)?;
            }
            Ok(call)
        })?;
        *last = id;
        Ok(written.value)
    }

    /// The totals of the calls filed under work `work` of agent `cid` (zero when it made none).
    pub fn work_totals(&self, cid: u64, work: u64) -> Totals {
        let read = self.db.read(|conn| {
            let found = conn
                .prepare_cached(sql::USAGE_WORK_TOTALS)?
                .query_row(params![bound(cid), bound(work)], |r| totals_at(r, 0))
                .optional()?;
            Ok(found.unwrap_or_default())
        });
        read.unwrap_or_else(|e| {
            eprintln!("apid: usage: cannot read a work's totals: {e}");
            Totals::default()
        })
    }

    /// Totals over the hours overlapping [from, to), grouped four ways.
    pub fn summary(&self, from_ms: u64, to_ms: u64) -> Summary {
        let read = self.db.read(|conn| {
            let models = grouped(conn, "model", from_ms, to_ms)?;
            let mut totals = Totals::default();
            for group in models.values() {
                totals.merge(group);
            }
            let unpriced = models
                .iter()
                .filter(|(_, t)| {
                    t.est_usd == 0.0 && t.cost_usd == 0.0 && tokens_total(&t.tokens) > 0
                })
                .map(|(model, _)| model.clone())
                .collect();
            Ok(Summary {
                from_ms,
                to_ms,
                totals,
                by_model: groups(models),
                by_agent: groups(grouped(conn, "agent", from_ms, to_ms)?),
                by_role: groups(grouped(conn, "role", from_ms, to_ms)?),
                by_provider: groups(grouped(conn, "provider", from_ms, to_ms)?),
                unpriced,
            })
        });
        read.unwrap_or_else(|e| {
            eprintln!("apid: usage: cannot read the summary: {e}");
            Summary {
                from_ms,
                to_ms,
                ..Summary::default()
            }
        })
    }

    /// `by` is one of model, agent, role, provider; `bucket_ms` is HOUR_MS or DAY_MS.
    /// The window starts at `from` rounded down to a bucket, and every bucket up to `to` is
    /// emitted (empty ones too), at most [`SERIES_MAX_POINTS`]; a longer window starts later.
    pub fn series(&self, from_ms: u64, to_ms: u64, bucket_ms: u64, by: &str) -> Series {
        let bucket_ms = bucket_ms.max(1);
        let mut start = from_ms - from_ms % bucket_ms;
        let mut points = Vec::new();
        if to_ms > start {
            let buckets = (to_ms - start).div_ceil(bucket_ms);
            let max = SERIES_MAX_POINTS as u64;
            if buckets > max {
                start += (buckets - max) * bucket_ms;
            }
            let count = (to_ms - start).div_ceil(bucket_ms) as usize;
            points = (0..count)
                .map(|i| Point {
                    at_ms: start + i as u64 * bucket_ms,
                    ..Point::default()
                })
                .collect::<Vec<Point>>();
            let read = self.db.read(|conn| {
                let mut stmt = conn.prepare_cached(sql::USAGE_SERIES)?;
                let rows = stmt.query_map(
                    params![by, bound(start), bound(bucket_ms), bound(to_ms)],
                    |r| {
                        Ok((
                            r.get::<_, u64>(0)?,
                            r.get::<_, String>(1)?,
                            totals_at(r, 2)?,
                        ))
                    },
                )?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            });
            match read {
                Ok(rows) => {
                    for (idx, group, totals) in rows {
                        if let Some(point) =
                            usize::try_from(idx).ok().and_then(|i| points.get_mut(i))
                        {
                            point.totals.merge(&totals);
                            point.groups.entry(group).or_default().merge(&totals);
                        }
                    }
                }
                Err(e) => eprintln!("apid: usage: cannot read the series: {e}"),
            }
        }
        Series {
            from_ms: start,
            to_ms,
            bucket_ms,
            by: by.to_string(),
            points,
        }
    }

    /// The newest calls first, those with an id below `before` (when given), matching `model`
    /// and `cid` (when given). `limit` 0 means [`CALLS_PAGE`]; it is capped at [`CALLS_LIMIT`].
    /// `total` counts every retained call that matches the filters. `next_before` is the id of
    /// the last call returned when more matching calls remain.
    pub fn calls(
        &self,
        before: Option<u64>,
        limit: usize,
        model: Option<&str>,
        cid: Option<u64>,
    ) -> CallsPage {
        let limit = if limit == 0 {
            CALLS_PAGE
        } else {
            limit.min(CALLS_LIMIT)
        };
        let read = self.db.read(|conn| {
            let total: i64 = conn
                .prepare_cached(sql::USAGE_CALLS_COUNT)?
                .query_row(params![model, cid.map(bound)], |r| r.get(0))?;
            let mut stmt = conn.prepare_cached(sql::USAGE_CALLS_PAGE)?;
            let mut calls = stmt
                .query_map(
                    params![
                        model,
                        cid.map(bound),
                        before.map(bound),
                        bound(limit as u64 + 1)
                    ],
                    call_at,
                )?
                .collect::<Result<Vec<Call>, _>>()?;
            // One row past the page says whether more matching calls remain.
            let more = calls.len() > limit;
            calls.truncate(limit);
            let next_before = if more {
                calls.last().map(|c| c.id)
            } else {
                None
            };
            Ok(CallsPage {
                calls,
                total: usize::try_from(total).unwrap_or(0),
                next_before,
            })
        });
        read.unwrap_or_else(|e| {
            eprintln!("apid: usage: cannot read the calls: {e}");
            CallsPage::default()
        })
    }

    /// Drops call rows beyond `calls_max` (oldest first), hour rows older than `days` (0 = keep),
    /// relative to `now_ms`, and work rows beyond [`WORKS_MAX`] (the oldest work items first). Sent to
    /// the writer; the next read sees it once committed.
    pub fn prune(&self, now_ms: u64, calls_max: usize, days: u32) {
        let hours_before = (days > 0).then(|| now_ms.saturating_sub(u64::from(days) * DAY_MS));
        let sent = self.db.send(move |conn| {
            if calls_max == 0 {
                conn.execute(sql::USAGE_CALLS_CLEAR, [])?;
            } else {
                conn.execute(sql::USAGE_CALLS_PRUNE, [bound((calls_max - 1) as u64)])?;
            }
            if let Some(cutoff) = hours_before {
                conn.execute(sql::USAGE_HOURS_PRUNE, [bound(cutoff)])?;
            }
            conn.execute(sql::USAGE_WORKS_PRUNE, [bound(WORKS_MAX as u64 - 1)])?;
            Ok(())
        });
        if let Err(e) = sent {
            eprintln!("apid: usage: cannot prune the ledger: {e}");
        }
    }

    /// The counter as its row, as it is now (the saver writes what changed).
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        let next_id = *self.last();
        RowSet::one(def(), &UsageState { next_id })
    }

    /// Restores the counter at start, never below the id of the newest stored call.
    pub fn load(&self, conn: &Connection) -> Result<(), StoreError> {
        let stored =
            apz_store::table::load_one::<UsageState>(conn, def())?.map_or(0, |state| state.next_id);
        let newest: u64 = conn.query_row(sql::USAGE_NEWEST_ID, [], |r| r.get(0))?;
        *self.last() = stored.max(newest);
        Ok(())
    }
}

/// The counter's table (`usage_state`).
pub fn def() -> &'static TableDef {
    apz_store::root_def!(UsageState)
}

/// The calls' table (`usage_call`), written through.
pub fn call_def() -> &'static TableDef {
    apz_store::root_def!(Call)
}

/// The hours' table (`usage_hour`), written through.
pub fn hour_def() -> &'static TableDef {
    apz_store::root_def!(HourRow)
}

/// The work items' table (`usage_work`), written through.
pub fn work_def() -> &'static TableDef {
    apz_store::root_def!(WorkRow)
}

/// A count or an id as a SQL parameter (values past i64 are clamped; no row holds them).
fn bound(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Writes one call as a row of `usage_call`, keyed by its id (the table's `pos` takes the id too: a stream has no list
/// position of its own).
fn insert_call(conn: &Connection, call: &Call) -> Result<(), StoreError> {
    let def = call_def();
    let mut rows = RowSet::new();
    call.put(def, vec![Value::Integer(bound(call.id))], &mut rows)?;
    let mut changes = apz_store::table::Changes::default();
    changes.upserts = rows
        .tables
        .remove(&def.name)
        .unwrap_or_default()
        .into_iter()
        .map(|row| (def.name.clone(), row))
        .collect();
    apz_store::table::apply(conn, def, &changes)
}

/// The totals one call adds: its cost, its estimate and what it counts as (reported cost where known).
fn call_totals(call: &Call) -> (f64, f64, f64) {
    (
        call.cost_usd.unwrap_or(0.0),
        call.est_usd,
        call.cost_usd.unwrap_or(call.est_usd),
    )
}

/// Adds one call to its hour's row, made when it is new.
fn add_to_hour(conn: &Connection, call: &Call) -> Result<(), StoreError> {
    let hour = call.at_ms - call.at_ms % HOUR_MS;
    let t = call.tokens;
    let (cost, est, usd) = call_totals(call);
    conn.prepare_cached(sql::USAGE_HOUR_ADD)?.execute(params![
        hour,
        call.model,
        call.cid,
        call.provider,
        call.role,
        1u64,
        t.input,
        t.output,
        t.cache_read,
        t.cache_write,
        cost,
        est,
        usd
    ])?;
    Ok(())
}

/// Adds one call to its work item's row, made when it is new.
fn add_to_work(conn: &Connection, call: &Call) -> Result<(), StoreError> {
    let t = call.tokens;
    let (cost, est, usd) = call_totals(call);
    conn.prepare_cached(sql::USAGE_WORK_ADD)?.execute(params![
        call.cid,
        call.work,
        1u64,
        t.input,
        t.output,
        t.cache_read,
        t.cache_write,
        cost,
        est,
        usd
    ])?;
    Ok(())
}

/// Whether `call` escalates its task: an earlier retained call of the same task ran on a lower rung.
fn escalates(conn: &Connection, call: &Call) -> Result<bool, StoreError> {
    let (Some(task), Some(rank)) = (call.task.as_deref(), rung_rank(call.rung.as_deref())) else {
        return Ok(false);
    };
    Ok(conn
        .prepare_cached(sql::USAGE_ESCALATED)?
        .query_row(params![task, i64::from(rank)], |r| r.get(0))?)
}

/// The usage of the hours overlapping [from, to), by one grouping (`model`, `agent`, `role` or `provider`).
fn grouped(
    conn: &Connection,
    by: &str,
    from_ms: u64,
    to_ms: u64,
) -> Result<BTreeMap<String, Totals>, StoreError> {
    let mut stmt = conn.prepare_cached(sql::USAGE_SUMMARY)?;
    let rows = stmt.query_map(
        params![by, bound(from_ms), bound(to_ms), bound(HOUR_MS)],
        |r| Ok((r.get::<_, String>(0)?, totals_at(r, 1)?)),
    )?;
    Ok(rows.collect::<Result<BTreeMap<_, _>, _>>()?)
}

/// One call row of `usage_call`, in the column order of `usage_calls_page.sql`.
fn call_at(r: &Row<'_>) -> apz_store::rusqlite::Result<Call> {
    Ok(Call {
        id: r.get(0)?,
        at_ms: r.get(1)?,
        cid: r.get(2)?,
        role: r.get(3)?,
        model: r.get(4)?,
        provider: r.get(5)?,
        tokens: Tokens {
            input: r.get(6)?,
            output: r.get(7)?,
            cache_read: r.get(8)?,
            cache_write: r.get(9)?,
        },
        cost_usd: r.get(10)?,
        est_usd: r.get(11)?,
        task: r.get(12)?,
        work: r.get(13)?,
        via: r.get(14)?,
        rung: r.get(15)?,
        escalated: r.get(16)?,
    })
}

/// Totals read from eight columns starting at `at`: calls, input, output, cache read, cache write, reported cost,
/// estimate, usd (the order of the usage queries' sums).
fn totals_at(r: &Row<'_>, at: usize) -> apz_store::rusqlite::Result<Totals> {
    let tokens = Tokens {
        input: r.get(at + 1)?,
        output: r.get(at + 2)?,
        cache_read: r.get(at + 3)?,
        cache_write: r.get(at + 4)?,
    };
    Ok(Totals {
        calls: r.get(at)?,
        cache_hit_rate: hit_rate(&tokens),
        tokens,
        cost_usd: r.get(at + 5)?,
        est_usd: r.get(at + 6)?,
        usd: r.get(at + 7)?,
    })
}

impl Totals {
    /// Adds `other` to these totals and recomputes the cache hit rate.
    fn merge(&mut self, other: &Totals) {
        self.calls += other.calls;
        self.tokens.input += other.tokens.input;
        self.tokens.output += other.tokens.output;
        self.tokens.cache_read += other.tokens.cache_read;
        self.tokens.cache_write += other.tokens.cache_write;
        self.cost_usd += other.cost_usd;
        self.est_usd += other.est_usd;
        self.usd += other.usd;
        self.cache_hit_rate = hit_rate(&self.tokens);
    }
}

/// The order of the rungs: cheap < mid < frontier. `None` for no rung or an unknown one.
fn rung_rank(rung: Option<&str>) -> Option<u8> {
    match rung? {
        "cheap" => Some(0),
        "mid" => Some(1),
        "frontier" => Some(2),
        _ => None,
    }
}

/// The shadow models that run on the cascade: those whose card id starts with `rung` and that are
/// in state shadow.
pub fn shadow_rung_ids(
    models: impl IntoIterator<Item = (String, crate::foundry::registry::State)>,
) -> Vec<String> {
    use crate::foundry::registry::State;
    models
        .into_iter()
        .filter(|(id, state)| id.starts_with("rung") && *state == State::Shadow)
        .map(|(id, _)| id)
        .collect()
}

/// The log line for one shadow run on a call: whether the shadow agreed with the real rung.
fn comparison_line(id: &str, model: &str, rung: &str, result: &Result<bool, String>) -> String {
    match result {
        Ok(true) => format!("apid: shadow {id} agreed with the real rung {rung} on {model}"),
        Ok(false) => format!("apid: shadow {id} differed from the real rung {rung} on {model}"),
        Err(e) => format!("apid: shadow {id} did not run on {model}: {e}"),
    }
}

/// The shadow hook on the cascade: each shadow rung model runs on the call's features beside the
/// real rung decision. It runs on its own thread, so the call never waits for it, and it only reads
/// the call: the real rung stays as it was filed.
fn shadow_on_cascade(app: &App, call: &Call) {
    let Some(rung) = call.rung.clone() else {
        return;
    };
    let ids = app.models.derived_ids(|records| {
        shadow_rung_ids(
            records
                .iter()
                .map(|record| (record.card.id.clone(), record.state)),
        )
    });
    if ids.is_empty() {
        return;
    }
    let Some(handle) = app.handle().upgrade() else {
        return;
    };
    let features = serde_json::json!({
        "model": call.model,
        "task": call.task,
        "via": call.via,
        "input_tokens": call.tokens.input,
        "output_tokens": call.tokens.output,
        "cache_read_tokens": call.tokens.cache_read,
        "cache_write_tokens": call.tokens.cache_write,
        "est_usd": call.est_usd,
    });
    let real = serde_json::json!({ "label": rung });
    let cost = call.cost_usd.unwrap_or(call.est_usd);
    let model = call.model.clone();
    std::thread::spawn(move || {
        for id in ids {
            let result = crate::foundry::breeder::shadow_run(&handle, &id, &features, &real, cost);
            eprintln!("{}", comparison_line(&id, &model, &rung, &result));
        }
    });
}

/// The provider of a plaza model id: the part before the first `/` (`openrouter/x/y` is
/// `openrouter`), or the whole id when it has no `/`.
pub fn provider_of(model: &str) -> &str {
    model
        .split_once('/')
        .map_or(model, |(provider, _)| provider)
}

fn hit_rate(tokens: &Tokens) -> f64 {
    let seen = tokens.input + tokens.cache_read + tokens.cache_write;
    if seen == 0 {
        0.0
    } else {
        tokens.cache_read as f64 / seen as f64
    }
}

fn tokens_total(tokens: &Tokens) -> u64 {
    tokens.input + tokens.output + tokens.cache_read + tokens.cache_write
}

/// The groups, biggest spend first (then by key).
fn groups(by_key: BTreeMap<String, Totals>) -> Vec<Group> {
    let mut out: Vec<Group> = by_key
        .into_iter()
        .map(|(key, totals)| Group { key, totals })
        .collect();
    out.sort_by(|a, b| {
        b.totals
            .usd
            .total_cmp(&a.totals.usd)
            .then_with(|| a.key.cmp(&b.key))
    });
    out
}

/// What a metered call is charged to: the task it belongs to and the work item it ran under (0 for
/// none).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Charge {
    pub task: Option<String>,
    pub work: u64,
}

/// Builds a [`Call`] for one metered call (role and provider looked up from the roster and the
/// providers, the estimate from the price catalog and the manual prices) and records it.
pub fn meter(
    app: &App,
    cid: u64,
    model: &str,
    tokens: Tokens,
    cost_usd: Option<f64>,
    task: Option<String>,
    via: &str,
) {
    meter_charged(
        app,
        cid,
        model,
        tokens,
        cost_usd,
        Charge { task, work: 0 },
        via,
    );
}

/// [`meter`] for a call charged to a work item as well as its task.
pub fn meter_charged(
    app: &App,
    cid: u64,
    model: &str,
    tokens: Tokens,
    cost_usd: Option<f64>,
    charge: Charge,
    via: &str,
) {
    let settings = app.settings.get();
    let priced = app.prices.lookup(model, &settings.prices);
    let rung = crate::foundry::datasets::rung_for_price(priced.as_ref().map(|p| p.price.output))
        .map(str::to_string);
    let call = Call {
        id: 0,
        at_ms: apz_plaza::now_ms(),
        cid,
        role: app
            .agents
            .get(cid)
            .map(|agent| agent.role)
            .unwrap_or_default(),
        model: model.to_string(),
        provider: provider_of(model).to_string(),
        tokens,
        cost_usd,
        est_usd: priced.map_or(0.0, |p| crate::prices::estimate(&p.price, &tokens)),
        task: charge.task,
        work: charge.work,
        via: via.to_string(),
        rung,
        escalated: false,
    };
    shadow_on_cascade(app, &call);
    app.usage.record(app, call);
}

/// Applies the usage retention settings (the scheduler calls this every tick).
pub fn retain(app: &App) {
    let retention = app.settings.get().retention;
    app.usage.prune(
        apz_plaza::now_ms(),
        retention.usage_calls_max as usize,
        retention.usage_days,
    );
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/usage/summary", |app, req, _| summary_route(app, req))
        .doc(RouteDoc::new("Summarize usage over a time window").errors(&[400]))
        .get("/v1/usage/series", |app, req, _| series_route(app, req))
        .doc(RouteDoc::new("Show usage over time, bucketed").errors(&[400]))
        .get("/v1/usage/calls", |app, req, _| calls_route(app, req))
        .doc(RouteDoc::new("List the newest model calls").errors(&[400]))
}

fn summary_route(app: &App, req: &Request) -> Response {
    match window(req, 30) {
        Ok((from, to)) => Response::json(200, &app.usage.summary(from, to)),
        Err(response) => response,
    }
}

fn series_route(app: &App, req: &Request) -> Response {
    let (from, to) = match window(req, 7) {
        Ok(window) => window,
        Err(response) => return response,
    };
    let bucket = match req.query_param("bucket").unwrap_or("hour") {
        "hour" => HOUR_MS,
        "day" => DAY_MS,
        other => {
            return Response::error(400, &format!("bucket must be hour or day, not {other:?}"));
        }
    };
    let by = match req.query_param("by").unwrap_or("model") {
        by @ ("model" | "agent" | "role" | "provider") => by,
        other => {
            return Response::error(
                400,
                &format!("by must be model, agent, role or provider, not {other:?}"),
            );
        }
    };
    Response::json(200, &app.usage.series(from, to, bucket, by))
}

fn calls_route(app: &App, req: &Request) -> Response {
    let limit = match req.query_param("limit") {
        None => 0,
        Some(raw) => match raw.parse::<usize>() {
            Ok(n) if (1..=CALLS_LIMIT).contains(&n) => n,
            _ => {
                return Response::error(
                    400,
                    &format!("limit must be 1..={CALLS_LIMIT}, not {raw:?}"),
                );
            }
        },
    };
    let before = match param_u64(req, "before") {
        Ok(before) => before,
        Err(response) => return response,
    };
    let cid = match param_u64(req, "cid") {
        Ok(cid) => cid,
        Err(response) => return response,
    };
    let model = req
        .query_param("model")
        .map(crate::plaza::percent_decode)
        .filter(|m| !m.is_empty());
    Response::json(200, &app.usage.calls(before, limit, model.as_deref(), cid))
}

/// The `from`/`to` window of a request: `to` defaults to now, `from` to `days` before `to`.
fn window(req: &Request, days: u64) -> Result<(u64, u64), Response> {
    let to = param_u64(req, "to")?.unwrap_or_else(apz_plaza::now_ms);
    let from = param_u64(req, "from")?.unwrap_or(to.saturating_sub(days * DAY_MS));
    if from > to {
        return Err(Response::error(
            400,
            &format!("from ({from}) is after to ({to})"),
        ));
    }
    Ok((from, to))
}

fn param_u64(req: &Request, name: &str) -> Result<Option<u64>, Response> {
    match req.query_param(name) {
        None => Ok(None),
        Some(raw) => raw.parse::<u64>().map(Some).map_err(|_| {
            Response::error(400, &format!("{name} must be a whole number, not {raw:?}"))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::PLAZA;

    /// The start of an hour, far enough from zero that windows can reach back.
    const H: u64 = 1_000 * HOUR_MS;

    fn tok(input: u64, output: u64, cache_read: u64, cache_write: u64) -> Tokens {
        Tokens {
            input,
            output,
            cache_read,
            cache_write,
        }
    }

    fn call(
        at_ms: u64,
        cid: u64,
        model: &str,
        tokens: Tokens,
        cost_usd: Option<f64>,
        est_usd: f64,
    ) -> Call {
        Call {
            id: 0,
            at_ms,
            cid,
            role: "worker".into(),
            model: model.into(),
            provider: provider_of(model).into(),
            tokens,
            cost_usd,
            est_usd,
            task: None,
            work: 0,
            via: "harness".into(),
            rung: None,
            escalated: false,
        }
    }

    /// A ledger over a fresh plaza.db in a temporary folder. The ledger's tables are made here when the migration
    /// does not hold them yet (the migration is regenerated from these definitions by `apid schema`).
    fn fixture() -> (tempfile::TempDir, apz_store::Store, Ledger) {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::open(dir.path()).unwrap();
        let db = Arc::clone(store.db(PLAZA));
        db.write(|conn| {
            for table in [def(), call_def(), hour_def(), work_def()] {
                let exists: bool = conn.query_row(
                    "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                    [&table.name],
                    |r| r.get(0),
                )?;
                if !exists {
                    conn.execute_batch(&table.ddl())?;
                }
            }
            Ok(())
        })
        .unwrap();
        let ledger = Ledger::new(db);
        (dir, store, ledger)
    }

    #[test]
    fn a_works_calls_are_summed_per_cid_and_work_and_survive_pruning() {
        let (_dir, store, ledger) = fixture();
        let on_work = |cid, work, input| Call {
            work,
            ..call(H, cid, "a/x", tok(input, 1, 0, 0), None, 0.0)
        };
        ledger.file(on_work(1, 3, 10)).unwrap();
        ledger.file(on_work(1, 3, 5)).unwrap();
        ledger.file(on_work(1, 4, 7)).unwrap();
        ledger.file(on_work(2, 3, 100)).unwrap();
        ledger
            .file(call(H, 1, "a/x", tok(1000, 0, 0, 0), None, 0.0))
            .unwrap();

        let mine = ledger.work_totals(1, 3);
        assert_eq!(mine.calls, 2);
        assert_eq!(mine.tokens.input, 15);
        assert_eq!(mine.tokens.output, 2);
        assert_eq!(ledger.work_totals(1, 4).calls, 1);
        assert_eq!(ledger.work_totals(2, 3).tokens.input, 100);
        assert_eq!(ledger.work_totals(1, 9), Totals::default());

        // Pruning the calls keeps the work rows.
        ledger.prune(H + HOUR_MS, 0, 0);
        store.db(PLAZA).flush().unwrap();
        assert_eq!(ledger.work_totals(1, 3), mine);
    }

    #[test]
    fn summary_totals_the_hours_in_the_window() {
        let (_dir, _store, ledger) = fixture();
        ledger
            .file(call(
                H + 5,
                1,
                "anthropic/a",
                tok(100, 10, 300, 0),
                Some(0.5),
                0.4,
            ))
            .unwrap();
        ledger
            .file(call(
                H + 9 * 60_000,
                2,
                "anthropic/a",
                tok(100, 20, 0, 100),
                None,
                0.25,
            ))
            .unwrap();
        ledger
            .file(call(
                H + HOUR_MS + 1,
                1,
                "openai/b",
                tok(50, 5, 0, 0),
                None,
                0.0,
            ))
            .unwrap();
        ledger
            .file(call(
                H + HOUR_MS + 2,
                3,
                "free/c",
                tok(10, 1, 0, 0),
                None,
                0.0,
            ))
            .unwrap();

        let s = ledger.summary(H, H + 2 * HOUR_MS);
        assert_eq!(s.totals.calls, 4);
        assert_eq!(s.totals.tokens, tok(260, 36, 300, 100));
        assert!((s.totals.cost_usd - 0.5).abs() < 1e-12);
        assert!((s.totals.est_usd - 0.65).abs() < 1e-12);
        assert!((s.totals.usd - 0.75).abs() < 1e-12);
        assert!((s.totals.cache_hit_rate - 300.0 / 660.0).abs() < 1e-12);

        assert_eq!(s.by_model[0].key, "anthropic/a");
        assert_eq!(s.by_model[0].totals.calls, 2);
        assert_eq!(
            s.by_agent
                .iter()
                .map(|g| g.key.as_str())
                .collect::<Vec<_>>(),
            ["1", "2", "3"]
        );
        assert_eq!(s.by_provider.len(), 3);
        // Models with tokens but no price at all: openai/b and free/c.
        assert_eq!(s.unpriced, ["free/c", "openai/b"]);

        // Only the second hour overlaps this window.
        let later = ledger.summary(H + HOUR_MS, H + 2 * HOUR_MS);
        assert_eq!(later.totals.calls, 2);
        assert_eq!(later.totals.cache_hit_rate, 0.0);
    }

    #[test]
    fn series_emits_empty_buckets_and_caps_points() {
        let (_dir, _store, ledger) = fixture();
        ledger
            .file(call(H, 1, "a/x", tok(10, 0, 0, 0), None, 1.0))
            .unwrap();
        ledger
            .file(call(
                H + 2 * HOUR_MS + 5,
                2,
                "b/y",
                tok(10, 0, 0, 0),
                None,
                2.0,
            ))
            .unwrap();

        let s = ledger.series(H + 7 * 60_000, H + 4 * HOUR_MS, HOUR_MS, "model");
        assert_eq!(s.from_ms, H);
        assert_eq!(s.points.len(), 4);
        assert_eq!(s.points[0].totals.calls, 1);
        assert!(s.points[1].groups.is_empty());
        assert_eq!(s.points[1].totals.calls, 0);
        assert_eq!(s.points[2].groups["b/y"].calls, 1);
        assert_eq!(s.points[2].totals.est_usd, 2.0);
        assert_eq!(s.points[3].at_ms, H + 3 * HOUR_MS);

        let by_agent = ledger.series(H, H + HOUR_MS, HOUR_MS, "agent");
        assert_eq!(by_agent.points[0].groups.keys().collect::<Vec<_>>(), ["1"]);

        // A window of 3000 hours keeps the last 2000 of them.
        let long = ledger.series(0, 3000 * HOUR_MS, HOUR_MS, "model");
        assert_eq!(long.points.len(), SERIES_MAX_POINTS);
        assert_eq!(long.from_ms, 1000 * HOUR_MS);
        assert_eq!(long.points.last().map(|p| p.at_ms), Some(2999 * HOUR_MS));
    }

    #[test]
    fn calls_page_newest_first_with_filters() {
        let (_dir, _store, ledger) = fixture();
        for i in 0..5 {
            let cid = if i % 2 == 0 { 1 } else { 2 };
            ledger
                .file(call(H + i, cid, "a/x", tok(1, 0, 0, 0), None, 0.0))
                .unwrap();
        }
        let ids = |page: &CallsPage| page.calls.iter().map(|c| c.id).collect::<Vec<_>>();

        let first = ledger.calls(None, 2, None, None);
        assert_eq!(ids(&first), [5, 4]);
        assert_eq!(first.total, 5);
        assert_eq!(first.next_before, Some(4));

        let second = ledger.calls(first.next_before, 2, None, None);
        assert_eq!(ids(&second), [3, 2]);
        assert_eq!(second.next_before, Some(2));

        let last = ledger.calls(second.next_before, 2, None, None);
        assert_eq!(ids(&last), [1]);
        assert_eq!(last.next_before, None);

        let by_cid = ledger.calls(None, 50, None, Some(1));
        assert_eq!(ids(&by_cid), [5, 3, 1]);
        assert_eq!(by_cid.total, 3);

        let by_model = ledger.calls(Some(4), 1, Some("a/x"), None);
        assert_eq!(ids(&by_model), [3]);
        assert_eq!(by_model.next_before, Some(3));

        assert_eq!(
            ledger.calls(None, 50, Some("zzz/none"), None),
            CallsPage::default()
        );
        assert_eq!(ledger.calls(None, 0, None, None).calls.len(), 5);
    }

    #[test]
    fn prune_keeps_the_newest_work_rows_only() {
        let (_dir, store, ledger) = fixture();
        store
            .db(PLAZA)
            .write(|conn| {
                for work in 1..=(WORKS_MAX as u64 + 2) {
                    conn.execute(
                        sql::USAGE_WORK_ADD,
                        params![
                            7u64, work, 1u64, 0u64, 0u64, 0u64, 0u64, 0.0f64, 0.0f64, 0.0f64
                        ],
                    )?;
                }
                Ok(())
            })
            .unwrap();
        ledger.prune(H, usize::MAX, 0);
        store.db(PLAZA).flush().unwrap();
        let (count, oldest): (i64, i64) = store
            .db(PLAZA)
            .read(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*), MIN(\"work\") FROM \"usage_work\"",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?)
            })
            .unwrap();
        assert_eq!(count, WORKS_MAX as i64);
        assert_eq!(oldest, 3, "the works 1 and 2 are the oldest and go");
    }

    #[test]
    fn prune_drops_oldest_calls_and_hours_past_the_age() {
        let (_dir, store, ledger) = fixture();
        ledger
            .file(call(H, 1, "a/x", tok(1, 0, 0, 0), None, 1.0))
            .unwrap();
        ledger
            .file(call(H + 2 * DAY_MS, 1, "a/x", tok(1, 0, 0, 0), None, 1.0))
            .unwrap();
        ledger
            .file(call(
                H + 2 * DAY_MS + 1,
                2,
                "a/x",
                tok(1, 0, 0, 0),
                None,
                1.0,
            ))
            .unwrap();

        ledger.prune(H + 2 * DAY_MS + 2, 2, 0);
        store.db(PLAZA).flush().unwrap();
        let kept: Vec<u64> = ledger
            .calls(None, 50, None, None)
            .calls
            .iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(kept, [3, 2]);
        assert_eq!(ledger.summary(0, u64::MAX).totals.calls, 3);

        ledger.prune(H + 2 * DAY_MS, 2, 1);
        store.db(PLAZA).flush().unwrap();
        assert_eq!(ledger.summary(0, u64::MAX).totals.calls, 2);
    }

    #[test]
    fn the_counter_continues_from_the_stored_calls() {
        let (_dir, store, ledger) = fixture();
        ledger
            .file(call(H, 1, "a/x", tok(10, 2, 5, 1), Some(0.1), 0.2))
            .unwrap();
        ledger
            .file(call(H + HOUR_MS, 2, "b/y", tok(3, 0, 0, 0), None, 0.0))
            .unwrap();

        // A restart: a fresh ledger over the same store, loaded from it, numbers on from the stored calls.
        let restarted = Ledger::new(Arc::clone(store.db(PLAZA)));
        store.db(PLAZA).read(|conn| restarted.load(conn)).unwrap();
        assert_eq!(restarted.summary(0, u64::MAX), ledger.summary(0, u64::MAX));
        assert_eq!(
            restarted.calls(None, 50, None, None),
            ledger.calls(None, 50, None, None)
        );
        assert_eq!(
            restarted.series(H, H + 2 * HOUR_MS, HOUR_MS, "role"),
            ledger.series(H, H + 2 * HOUR_MS, HOUR_MS, "role")
        );

        let next = restarted
            .file(call(H + 3, 1, "a/x", tok(1, 0, 0, 0), None, 0.0))
            .unwrap();
        assert_eq!(next.id, 3);
        assert_eq!(restarted.summary(H, H + HOUR_MS).totals.calls, 2);
    }

    fn rung_call(task: Option<&str>, rung: Option<&str>) -> Call {
        Call {
            task: task.map(str::to_string),
            rung: rung.map(str::to_string),
            ..call(H, 1, "a/x", tok(1, 0, 0, 0), None, 0.0)
        }
    }

    #[test]
    fn a_call_records_its_rung_and_a_later_higher_rung_call_of_its_task_escalates() {
        let (_dir, _store, ledger) = fixture();
        let first = ledger.file(rung_call(Some("t1"), Some("cheap"))).unwrap();
        assert_eq!(
            (first.rung.as_deref(), first.escalated),
            (Some("cheap"), false)
        );
        let second = ledger
            .file(rung_call(Some("t1"), Some("frontier")))
            .unwrap();
        assert_eq!(
            (second.rung.as_deref(), second.escalated),
            (Some("frontier"), true)
        );
        // The task already had a cheaper call, so a later call on the frontier is still an escalation.
        assert!(
            ledger
                .file(rung_call(Some("t1"), Some("frontier")))
                .unwrap()
                .escalated
        );
        // Another task, no task, and a same-rung repeat with no lower call before are not.
        assert!(
            !ledger
                .file(rung_call(Some("t2"), Some("frontier")))
                .unwrap()
                .escalated
        );
        assert!(!ledger.file(rung_call(None, Some("mid"))).unwrap().escalated);
        assert!(
            !ledger
                .file(rung_call(Some("t3"), Some("mid")))
                .unwrap()
                .escalated
        );
        assert!(
            !ledger
                .file(rung_call(Some("t3"), Some("mid")))
                .unwrap()
                .escalated
        );
        // A lower rung after a higher one is not an escalation.
        ledger
            .file(rung_call(Some("t4"), Some("frontier")))
            .unwrap();
        assert!(
            !ledger
                .file(rung_call(Some("t4"), Some("cheap")))
                .unwrap()
                .escalated
        );

        let page = ledger.calls(None, 50, None, None);
        assert_eq!(
            page.calls
                .iter()
                .find(|c| c.id == second.id)
                .and_then(|c| c.rung.clone())
                .as_deref(),
            Some("frontier")
        );

        // A record saved before rungs still loads, with no rung and no escalation.
        let old = r#"{"id":1,"at_ms":5,"cid":1,"role":"worker","model":"a/x","provider":"a","input":1,"output":0,"cache_read":0,"cache_write":0,"est_usd":0.0,"via":"harness"}"#;
        let old: Call = serde_json::from_str(old).unwrap();
        assert_eq!((old.rung, old.escalated), (None, false));
    }

    #[test]
    fn the_shadow_hook_picks_only_shadow_rung_models_and_logs_each_comparison() {
        use crate::foundry::registry::State;
        let picked = shadow_rung_ids(vec![
            ("rung-a".to_string(), State::Shadow),
            ("rung-b".to_string(), State::Promoted),
            ("other".to_string(), State::Shadow),
        ]);
        assert_eq!(picked, ["rung-a"]);

        let agreed = comparison_line("rung-a", "a/x", "mid", &Ok(true));
        assert_eq!(
            agreed,
            "apid: shadow rung-a agreed with the real rung mid on a/x"
        );
        let differed = comparison_line("rung-a", "a/x", "mid", &Ok(false));
        assert!(
            differed.contains("differed from the real rung mid"),
            "{differed}"
        );
        let failed = comparison_line("rung-a", "a/x", "mid", &Err("no model".into()));
        assert!(failed.contains("did not run on a/x: no model"), "{failed}");
    }
}
