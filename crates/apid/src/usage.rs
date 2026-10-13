//! The model-usage ledger: every model call an agent makes (through the harness or the gateway
//! proxy), with its tokens, cache hits and cost, so the owner can see what the plaza spends.
//! Saved in Postgres ("usage").
//!
//! Two records, both bounded:
//! - `calls`: the newest calls, one row each (at most `settings.retention.usage_calls_max`);
//! - `hours`: per-hour totals by (hour, model, cid), kept for `settings.retention.usage_days`
//!   (0 = forever). Series and summaries are computed from the hours, so they cover the whole
//!   kept period even after the call rows are gone.
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

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Mutex, MutexGuard, PoisonError};

use apz_http::{Request, Response, Router};
use serde::{Deserialize, Serialize};

use crate::prices::Tokens;
use crate::state::App;

pub const HOUR_MS: u64 = 60 * 60 * 1000;
pub const DAY_MS: u64 = 24 * HOUR_MS;
pub const CALLS_PAGE: usize = 50;
pub const CALLS_LIMIT: usize = 200;
/// The most points one series returns; a longer window starts later than asked.
pub const SERIES_MAX_POINTS: usize = 2000;

/// One model call.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Call {
    /// Assigned by the ledger, increasing from 1.
    pub id: u64,
    pub at_ms: u64,
    pub cid: u64,
    pub role: String,
    /// The plaza model id, e.g. `anthropic/claude-opus-5-5`.
    pub model: String,
    /// The provider id that served it (the part before the first `/` when unknown).
    pub provider: String,
    #[serde(flatten)]
    pub tokens: Tokens,
    /// What the provider reported the call cost, in US dollars.
    #[serde(default)]
    pub cost_usd: Option<f64>,
    /// The catalog's estimate, in US dollars.
    pub est_usd: f64,
    /// The task the call was charged to, if any.
    #[serde(default)]
    pub task: Option<String>,
    /// "harness" (the agent's own loop) or "gateway" (the proxy agents' tools call through).
    pub via: String,
}

/// Totals over some calls.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Totals {
    pub calls: u64,
    #[serde(flatten)]
    pub tokens: Tokens,
    /// Sum of provider-reported costs.
    pub cost_usd: f64,
    /// Sum of estimates.
    pub est_usd: f64,
    /// Reported cost where known, the estimate otherwise.
    pub usd: f64,
    /// cache_read / (input + cache_read + cache_write); 0 when there was no input.
    pub cache_hit_rate: f64,
}

/// One hour's totals for one model and one agent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HourRow {
    pub hour_ms: u64,
    pub model: String,
    pub provider: String,
    pub cid: u64,
    pub role: String,
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

/// An hour row's key: (hour start, model, cid).
pub type HourKey = (u64, String, u64);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LedgerState {
    pub next_id: u64,
    pub calls: VecDeque<Call>,
    #[serde(with = "hour_list", default)]
    pub hours: BTreeMap<HourKey, HourRow>,
}

/// The hour rows live in a map internally and are saved as a list.
mod hour_list {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Deserializer, Serializer};

    use super::{HourKey, HourRow};

    pub fn serialize<S: Serializer>(hours: &BTreeMap<HourKey, HourRow>, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(hours.values())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeMap<HourKey, HourRow>, D::Error> {
        let rows = Vec::<HourRow>::deserialize(d)?;
        Ok(rows.into_iter().map(|row| ((row.hour_ms, row.model.clone(), row.cid), row)).collect())
    }
}

#[derive(Default)]
pub struct Ledger {
    state: Mutex<LedgerState>,
}

impl Ledger {
    fn lock(&self) -> MutexGuard<'_, LedgerState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Files `call` (its `id` is assigned here, its `at_ms` kept) into the call rows and its hour,
    /// publishes `usage` on the bus with the stored call, and returns the id.
    pub fn record(&self, app: &App, call: Call) -> u64 {
        let stored = self.file(call);
        app.bus.publish("usage", &stored);
        stored.id
    }

    /// The storing half of [`Ledger::record`]: no bus, so tests can file calls without an app.
    fn file(&self, mut call: Call) -> Call {
        let mut state = self.lock();
        state.next_id += 1;
        call.id = state.next_id;
        let hour_ms = call.at_ms - call.at_ms % HOUR_MS;
        let row = state.hours.entry((hour_ms, call.model.clone(), call.cid)).or_insert_with(|| HourRow {
            hour_ms,
            model: call.model.clone(),
            provider: call.provider.clone(),
            cid: call.cid,
            role: call.role.clone(),
            totals: Totals::default(),
        });
        row.totals.add_call(&call);
        state.calls.push_back(call.clone());
        call
    }

    /// Totals over the hours overlapping [from, to), grouped four ways.
    pub fn summary(&self, from_ms: u64, to_ms: u64) -> Summary {
        let state = self.lock();
        let mut total = Totals::default();
        let mut models: BTreeMap<String, Totals> = BTreeMap::new();
        let mut agents: BTreeMap<String, Totals> = BTreeMap::new();
        let mut roles: BTreeMap<String, Totals> = BTreeMap::new();
        let mut providers: BTreeMap<String, Totals> = BTreeMap::new();
        for row in state.hours.values().filter(|row| overlaps(row.hour_ms, from_ms, to_ms)) {
            total.merge(&row.totals);
            models.entry(row.model.clone()).or_default().merge(&row.totals);
            agents.entry(row.cid.to_string()).or_default().merge(&row.totals);
            roles.entry(row.role.clone()).or_default().merge(&row.totals);
            providers.entry(row.provider.clone()).or_default().merge(&row.totals);
        }
        let unpriced = models
            .iter()
            .filter(|(_, t)| t.est_usd == 0.0 && t.cost_usd == 0.0 && tokens_total(&t.tokens) > 0)
            .map(|(model, _)| model.clone())
            .collect();
        Summary {
            from_ms,
            to_ms,
            totals: total,
            by_model: groups(models),
            by_agent: groups(agents),
            by_role: groups(roles),
            by_provider: groups(providers),
            unpriced,
        }
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
                .map(|i| Point { at_ms: start + i as u64 * bucket_ms, ..Point::default() })
                .collect::<Vec<Point>>();
            let state = self.lock();
            for row in state.hours.values().filter(|row| row.hour_ms >= start && row.hour_ms < to_ms) {
                let point = &mut points[((row.hour_ms - start) / bucket_ms) as usize];
                point.totals.merge(&row.totals);
                point.groups.entry(group_of(row, by)).or_default().merge(&row.totals);
            }
        }
        Series { from_ms: start, to_ms, bucket_ms, by: by.to_string(), points }
    }

    /// The newest calls first, those with an id below `before` (when given), matching `model`
    /// and `cid` (when given). `limit` 0 means [`CALLS_PAGE`]; it is capped at [`CALLS_LIMIT`].
    /// `total` counts every retained call that matches the filters. `next_before` is the id of
    /// the last call returned when more matching calls remain.
    pub fn calls(&self, before: Option<u64>, limit: usize, model: Option<&str>, cid: Option<u64>) -> CallsPage {
        let limit = if limit == 0 { CALLS_PAGE } else { limit.min(CALLS_LIMIT) };
        let state = self.lock();
        let matches = |call: &&Call| {
            model.is_none_or(|m| call.model == m) && cid.is_none_or(|id| call.cid == id)
        };
        let total = state.calls.iter().filter(matches).count();
        let mut calls = Vec::new();
        let mut more = false;
        for call in state.calls.iter().rev().filter(matches).filter(|c| before.is_none_or(|b| c.id < b)) {
            if calls.len() == limit {
                more = true;
                break;
            }
            calls.push(call.clone());
        }
        let next_before = if more { calls.last().map(|c| c.id) } else { None };
        CallsPage { calls, total, next_before }
    }

    /// Drops call rows beyond `calls_max` (oldest first) and hour rows older than `days`
    /// (0 = keep), relative to `now_ms`.
    pub fn prune(&self, now_ms: u64, calls_max: usize, days: u32) {
        let mut state = self.lock();
        while state.calls.len() > calls_max {
            state.calls.pop_front();
        }
        if days > 0 {
            let cutoff = now_ms.saturating_sub(u64::from(days) * DAY_MS);
            state.hours.retain(|key, _| key.0 >= cutoff);
        }
    }

    pub fn snapshot(&self) -> String {
        self.state.lock().ok().and_then(|s| serde_json::to_string(&*s).ok()).unwrap_or_default()
    }

    pub fn restore(&self, json: &str) -> Result<(), String> {
        let state: LedgerState = serde_json::from_str(json).map_err(|e| e.to_string())?;
        *self.state.lock().map_err(|e| e.to_string())? = state;
        Ok(())
    }
}

impl Totals {
    /// Adds one call to these totals.
    fn add_call(&mut self, call: &Call) {
        self.merge(&Totals {
            calls: 1,
            tokens: call.tokens,
            cost_usd: call.cost_usd.unwrap_or(0.0),
            est_usd: call.est_usd,
            usd: call.cost_usd.unwrap_or(call.est_usd),
            cache_hit_rate: 0.0,
        });
    }

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

/// Whether the hour starting at `hour_ms` overlaps [from, to).
fn overlaps(hour_ms: u64, from_ms: u64, to_ms: u64) -> bool {
    hour_ms < to_ms && hour_ms + HOUR_MS > from_ms
}

/// The key a row falls under for a series' or summary's grouping.
fn group_of(row: &HourRow, by: &str) -> String {
    match by {
        "agent" => row.cid.to_string(),
        "role" => row.role.clone(),
        "provider" => row.provider.clone(),
        _ => row.model.clone(),
    }
}

/// The groups, biggest spend first (then by key).
fn groups(by_key: BTreeMap<String, Totals>) -> Vec<Group> {
    let mut out: Vec<Group> = by_key.into_iter().map(|(key, totals)| Group { key, totals }).collect();
    out.sort_by(|a, b| b.totals.usd.total_cmp(&a.totals.usd).then_with(|| a.key.cmp(&b.key)));
    out
}

/// Builds a [`Call`] for one metered call (role and provider looked up from the roster and the
/// providers, the estimate from the price catalog and the manual prices) and records it.
pub fn meter(app: &App, cid: u64, model: &str, tokens: Tokens, cost_usd: Option<f64>, task: Option<String>, via: &str) {
    let settings = app.settings.get();
    let priced = app.prices.lookup(model, &settings.prices);
    let call = Call {
        id: 0,
        at_ms: apz_plaza::now_ms(),
        cid,
        role: app.agents.get(cid).map(|agent| agent.role).unwrap_or_default(),
        model: model.to_string(),
        provider: model.split_once('/').map_or(model, |(provider, _)| provider).to_string(),
        tokens,
        cost_usd,
        est_usd: priced.map_or(0.0, |p| crate::prices::estimate(&p.price, &tokens)),
        task,
        via: via.to_string(),
    };
    app.usage.record(app, call);
}

/// Applies the usage retention settings (the scheduler calls this every tick).
pub fn retain(app: &App) {
    let retention = app.settings.get().retention;
    app.usage.prune(apz_plaza::now_ms(), retention.usage_calls_max as usize, retention.usage_days);
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/usage/summary", |app, req, _| summary_route(app, req))
        .get("/v1/usage/series", |app, req, _| series_route(app, req))
        .get("/v1/usage/calls", |app, req, _| calls_route(app, req))
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
        other => return Response::error(400, &format!("bucket must be hour or day, not {other:?}")),
    };
    let by = match req.query_param("by").unwrap_or("model") {
        by @ ("model" | "agent" | "role" | "provider") => by,
        other => return Response::error(400, &format!("by must be model, agent, role or provider, not {other:?}")),
    };
    Response::json(200, &app.usage.series(from, to, bucket, by))
}

fn calls_route(app: &App, req: &Request) -> Response {
    let limit = match req.query_param("limit") {
        None => 0,
        Some(raw) => match raw.parse::<usize>() {
            Ok(n) if (1..=CALLS_LIMIT).contains(&n) => n,
            _ => return Response::error(400, &format!("limit must be 1..={CALLS_LIMIT}, not {raw:?}")),
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
    let model = req.query_param("model").map(crate::plaza::percent_decode).filter(|m| !m.is_empty());
    Response::json(200, &app.usage.calls(before, limit, model.as_deref(), cid))
}

/// The `from`/`to` window of a request: `to` defaults to now, `from` to `days` before `to`.
fn window(req: &Request, days: u64) -> Result<(u64, u64), Response> {
    let to = param_u64(req, "to")?.unwrap_or_else(apz_plaza::now_ms);
    let from = param_u64(req, "from")?.unwrap_or(to.saturating_sub(days * DAY_MS));
    if from > to {
        return Err(Response::error(400, &format!("from ({from}) is after to ({to})")));
    }
    Ok((from, to))
}

fn param_u64(req: &Request, name: &str) -> Result<Option<u64>, Response> {
    match req.query_param(name) {
        None => Ok(None),
        Some(raw) => raw
            .parse::<u64>()
            .map(Some)
            .map_err(|_| Response::error(400, &format!("{name} must be a whole number, not {raw:?}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The start of an hour, far enough from zero that windows can reach back.
    const H: u64 = 1_000 * HOUR_MS;

    fn tok(input: u64, output: u64, cache_read: u64, cache_write: u64) -> Tokens {
        Tokens { input, output, cache_read, cache_write }
    }

    fn call(at_ms: u64, cid: u64, model: &str, tokens: Tokens, cost_usd: Option<f64>, est_usd: f64) -> Call {
        Call {
            id: 0,
            at_ms,
            cid,
            role: "worker".into(),
            model: model.into(),
            provider: model.split_once('/').map_or(model, |(p, _)| p).into(),
            tokens,
            cost_usd,
            est_usd,
            task: None,
            via: "harness".into(),
        }
    }

    #[test]
    fn summary_totals_the_hours_in_the_window() {
        let ledger = Ledger::default();
        ledger.file(call(H + 5, 1, "anthropic/a", tok(100, 10, 300, 0), Some(0.5), 0.4));
        ledger.file(call(H + 9 * 60_000, 2, "anthropic/a", tok(100, 20, 0, 100), None, 0.25));
        ledger.file(call(H + HOUR_MS + 1, 1, "openai/b", tok(50, 5, 0, 0), None, 0.0));
        ledger.file(call(H + HOUR_MS + 2, 3, "free/c", tok(10, 1, 0, 0), None, 0.0));

        let s = ledger.summary(H, H + 2 * HOUR_MS);
        assert_eq!(s.totals.calls, 4);
        assert_eq!(s.totals.tokens, tok(260, 36, 300, 100));
        assert!((s.totals.cost_usd - 0.5).abs() < 1e-12);
        assert!((s.totals.est_usd - 0.65).abs() < 1e-12);
        assert!((s.totals.usd - 0.75).abs() < 1e-12);
        assert!((s.totals.cache_hit_rate - 300.0 / 660.0).abs() < 1e-12);

        assert_eq!(s.by_model[0].key, "anthropic/a");
        assert_eq!(s.by_model[0].totals.calls, 2);
        assert_eq!(s.by_agent.iter().map(|g| g.key.as_str()).collect::<Vec<_>>(), ["1", "2", "3"]);
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
        let ledger = Ledger::default();
        ledger.file(call(H, 1, "a/x", tok(10, 0, 0, 0), None, 1.0));
        ledger.file(call(H + 2 * HOUR_MS + 5, 2, "b/y", tok(10, 0, 0, 0), None, 2.0));

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
        let ledger = Ledger::default();
        for i in 0..5 {
            let cid = if i % 2 == 0 { 1 } else { 2 };
            ledger.file(call(H + i, cid, "a/x", tok(1, 0, 0, 0), None, 0.0));
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

        assert_eq!(ledger.calls(None, 50, Some("zzz/none"), None), CallsPage::default());
        assert_eq!(ledger.calls(None, 0, None, None).calls.len(), 5);
    }

    #[test]
    fn prune_drops_oldest_calls_and_hours_past_the_age() {
        let ledger = Ledger::default();
        ledger.file(call(H, 1, "a/x", tok(1, 0, 0, 0), None, 1.0));
        ledger.file(call(H + 2 * DAY_MS, 1, "a/x", tok(1, 0, 0, 0), None, 1.0));
        ledger.file(call(H + 2 * DAY_MS + 1, 2, "a/x", tok(1, 0, 0, 0), None, 1.0));

        ledger.prune(H + 2 * DAY_MS + 2, 2, 0);
        let kept: Vec<u64> = ledger.calls(None, 50, None, None).calls.iter().map(|c| c.id).collect();
        assert_eq!(kept, [3, 2]);
        assert_eq!(ledger.summary(0, u64::MAX).totals.calls, 3);

        ledger.prune(H + 2 * DAY_MS, 2, 1);
        assert_eq!(ledger.summary(0, u64::MAX).totals.calls, 2);
    }

    #[test]
    fn snapshot_round_trips_and_numbering_continues() {
        let ledger = Ledger::default();
        ledger.file(call(H, 1, "a/x", tok(10, 2, 5, 1), Some(0.1), 0.2));
        ledger.file(call(H + HOUR_MS, 2, "b/y", tok(3, 0, 0, 0), None, 0.0));

        let restored = Ledger::default();
        restored.restore(&ledger.snapshot()).unwrap();
        assert_eq!(restored.summary(0, u64::MAX), ledger.summary(0, u64::MAX));
        assert_eq!(restored.calls(None, 50, None, None), ledger.calls(None, 50, None, None));
        assert_eq!(restored.series(H, H + 2 * HOUR_MS, HOUR_MS, "role"), ledger.series(H, H + 2 * HOUR_MS, HOUR_MS, "role"));

        let next = restored.file(call(H + 3, 1, "a/x", tok(1, 0, 0, 0), None, 0.0));
        assert_eq!(next.id, 3);
        assert_eq!(restored.summary(H, H + HOUR_MS).totals.calls, 2);
    }
}
