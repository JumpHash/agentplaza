//! `/v1/metrics/*` (owner only, like everything behind the hub's gate; agents never reach it) and the opt-in
//! `/metrics` (OpenMetrics, behind its own token).
//!
//! * `GET /v1/metrics/catalog` — the catalog (`apz_metrics::CATALOG`).
//! * `GET /v1/metrics/scopes` — the scope tree.
//! * `GET /v1/metrics/view?tab&scope&range&shift&zoom&compare&group&trace&filter&width&tz&fixture` — a composed
//!   [`apz_metrics_view::View`] (the fixture when `fixture=1`).
//! * `GET /v1/metrics/around?…same…&t=` — "what changed around then" ([`apz_metrics_view::Around`]).
//! * `POST /v1/metrics/query` — one [`apz_metrics_view::source::Read`]-shaped read, as grouped series (CLI, tests).
//! * `GET /v1/metrics/calls/page?scope&before&tz&fixture` — the call log's next page, composed (a `Table`).
//! * `GET /v1/metrics/events?scope&from&to&kinds&limit`, `GET /v1/metrics/calls?scope&before&limit`,
//!   `GET /v1/metrics/traces?scope&from&to`, `GET /v1/metrics/traces/{id}`.
//! * `GET /v1/metrics/export?…view query…` — the view's chart series as CSV.
//! * `POST /v1/metrics/live` — a [`apz_metrics_view::LiveLease`]; `POST /v1/metrics/open/{cid}` — an agent is open.
//! * `GET`/`PUT /v1/metrics/settings`; `GET`/`PUT /v1/metrics/dashboards` ([`apz_metrics_view::Saved`]).
//! * `GET /v1/metrics/alerts`; `PUT /v1/metrics/alerts/rules` (a rule, upsert); `DELETE
//!   /v1/metrics/alerts/rules/{id}`; `POST /v1/metrics/alerts/{id}/mute` `{until_ms}`; `POST
//!   /v1/metrics/alerts/{id}/ack` `{subject}`; `POST /v1/metrics/alerts/{id}/on` `{on}`; `GET /v1/metrics/slos`;
//!   `GET /v1/metrics/forecast`.
//! * `POST /v1/nodes/{id}/metrics` lives in nodes.rs (signed by the node); this module ingests it.
//! * `GET /metrics` — OpenMetrics text with `Authorization: Bearer <openmetrics token>`, 404 when off.

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_metrics::CATALOG;
use apz_metrics::alerts::Rule;
use apz_metrics::query::AggFn;
use apz_metrics_view::compose::around::around;
use apz_metrics_view::compose::compose;
use apz_metrics_view::fixture::Fixture;
use apz_metrics_view::source::{GroupBy, Read, Source};
use apz_metrics_view::tabs::TABS;
use apz_metrics_view::{LiveLease, RANGES, Saved, ViewQuery};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::str::FromStr;

use super::export;
use super::settings::MetricsSettings;
use super::source::LiveSource;
use crate::state::App;

const DAY_MS: u64 = 86_400_000;

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/metrics/catalog", |_, _, _| {
            Response::json(200, &CATALOG)
        })
        .doc(RouteDoc::new("Read the metric catalog"))
        .get("/v1/metrics/scopes", |app, req, _| scopes_route(app, req))
        .doc(RouteDoc::new("List the metric scopes"))
        .get("/v1/metrics/view", |app, req, _| view_route(app, req))
        .doc(RouteDoc::new("Compose a metrics view").errors(&[400]))
        .get("/v1/metrics/around", |app, req, _| around_route(app, req))
        .doc(RouteDoc::new("Show what changed around a time").errors(&[400]))
        .route("POST", "/v1/metrics/query", |app, req, _| {
            query_route(app, req)
        })
        .doc(RouteDoc::new("Read metric series grouped by a key").errors(&[400]))
        .get("/v1/metrics/events", |app, req, _| events_route(app, req))
        .doc(RouteDoc::new("List the metric events").errors(&[400]))
        .get("/v1/metrics/calls", |app, req, _| calls_route(app, req))
        .doc(RouteDoc::new("List the agents' calls").errors(&[400]))
        .get("/v1/metrics/calls/page", |app, req, _| {
            calls_page_route(app, req)
        })
        .doc(RouteDoc::new("Load the next page of calls").errors(&[400]))
        .get("/v1/metrics/traces", |app, req, _| traces_route(app, req))
        .doc(RouteDoc::new("List the kept traces").errors(&[400]))
        .get("/v1/metrics/traces/{id}", |app, req, params| {
            trace_route(app, req, params)
        })
        .doc(RouteDoc::new("Show a kept trace").errors(&[404]))
        .get("/v1/metrics/export", |app, req, _| export_route(app, req))
        .doc(RouteDoc::new("Export a view as CSV").errors(&[400]))
        .route("POST", "/v1/metrics/live", |app, req, _| {
            live_route(app, req)
        })
        .doc(RouteDoc::new("Renew the live metrics lease").errors(&[400]))
        .route("POST", "/v1/metrics/open/{cid}", |app, _, params| {
            open_route(app, params)
        })
        .doc(RouteDoc::new("Mark an agent as open in the traces"))
        .get("/v1/metrics/settings", |app, _, _| {
            Response::json(200, &app.metrics.settings.get())
        })
        .doc(RouteDoc::new("Read the metrics settings"))
        .route("PUT", "/v1/metrics/settings", |app, req, _| {
            put_settings_route(app, req)
        })
        .doc(RouteDoc::new("Save the metrics settings").errors(&[400, 500]))
        .get("/v1/metrics/dashboards", |app, _, _| {
            dashboards_get_route(app)
        })
        .doc(RouteDoc::new("Read the saved dashboards"))
        .route("PUT", "/v1/metrics/dashboards", |app, req, _| {
            dashboards_put_route(app, req)
        })
        .doc(RouteDoc::new("Save the dashboards").errors(&[400]))
        .get("/v1/metrics/alerts", |app, _, _| alerts_route(app))
        .doc(RouteDoc::new("List the alerts and their state"))
        .route("PUT", "/v1/metrics/alerts/rules", |app, req, _| {
            put_rule_route(app, req)
        })
        .doc(RouteDoc::new("Save an alert rule").errors(&[400]))
        .route(
            "DELETE",
            "/v1/metrics/alerts/rules/{id}",
            |app, _, params| delete_rule_route(app, params),
        )
        .doc(RouteDoc::new("Delete an alert rule").errors(&[400]))
        .route("POST", "/v1/metrics/alerts/{id}/mute", mute_route)
        .doc(RouteDoc::new("Mute an alert until a time").errors(&[400]))
        .route("POST", "/v1/metrics/alerts/{id}/ack", |app, req, params| {
            ack_route(app, req, params)
        })
        .doc(RouteDoc::new("Acknowledge an alert").errors(&[400]))
        .route("POST", "/v1/metrics/alerts/{id}/on", |app, req, params| {
            rule_on_route(app, req, params)
        })
        .doc(RouteDoc::new("Switch an alert rule on or off").errors(&[400, 404]))
        .get("/v1/metrics/slos", |app, _, _| slos_route(app))
        .doc(RouteDoc::new("List the service-level objectives"))
        .get("/v1/metrics/forecast", |app, req, _| {
            forecast_route(app, req)
        })
        .doc(RouteDoc::new("Show the capacity forecast").errors(&[400, 404]))
        .get("/metrics", |app, req, _| metrics_route(app, req))
        .doc(RouteDoc::new("Scrape the metrics in OpenMetrics").errors(&[404]))
}

/// The source a read goes to: the built-in fixture when `fixture` is set, else the hub at this moment.
fn with_source(app: &App, fixture: bool, read: impl FnOnce(&dyn Source) -> Response) -> Response {
    let now = apz_plaza::now_ms();
    if fixture {
        read(&Fixture::new(now))
    } else {
        read(&LiveSource {
            app,
            m: &app.metrics,
            now_ms: now,
        })
    }
}

fn scopes_route(app: &App, req: &Request) -> Response {
    with_source(app, fixture_requested(&req.query), |src| {
        Response::json(200, &src.scopes())
    })
}

fn view_route(app: &App, req: &Request) -> Response {
    match parse_view(&req.query) {
        Ok(query) => with_source(app, query.fixture, |src| view_response(&query, src)),
        Err(message) => Response::error(400, &message),
    }
}

/// The composed view as JSON. Split out so a test can compose from the fixture without an app.
fn view_response(query: &ViewQuery, src: &dyn Source) -> Response {
    Response::json(200, &compose(query, src))
}

fn around_route(app: &App, req: &Request) -> Response {
    let pairs = query_pairs(&req.query);
    let t_ms = match number(&pairs, "t", apz_plaza::now_ms()) {
        Ok(t_ms) => t_ms,
        Err(message) => return Response::error(400, &message),
    };
    match parse_view(&req.query) {
        Ok(query) => with_source(app, query.fixture, |src| {
            Response::json(200, &around(&query, t_ms, src))
        }),
        Err(message) => Response::error(400, &message),
    }
}

fn export_route(app: &App, req: &Request) -> Response {
    match parse_view(&req.query) {
        Ok(query) => with_source(app, query.fixture, |src| {
            let view = compose(&query, src);
            Response::new(200, "text/csv; charset=utf-8", export::csv(&view))
        }),
        Err(message) => Response::error(400, &message),
    }
}

#[derive(Deserialize)]
struct QueryBody {
    metric: String,
    scope: String,
    from_ms: u64,
    to_ms: u64,
    step_ms: u64,
    agg: AggFn,
    group: String,
    #[serde(default)]
    top: Option<usize>,
}

fn query_route(app: &App, req: &Request) -> Response {
    let body: QueryBody = match json_body(req) {
        Ok(body) => body,
        Err(response) => return response,
    };
    if body.step_ms == 0 || body.to_ms < body.from_ms {
        return Response::error(
            400,
            "step_ms must be above zero, and to_ms must not be before from_ms",
        );
    }
    let group = match parse_group(&body.group) {
        Ok(group) => group,
        Err(message) => return Response::error(400, &message),
    };
    let read = Read {
        metric: body.metric,
        scope: body.scope,
        from_ms: body.from_ms,
        to_ms: body.to_ms,
        step_ms: body.step_ms,
        agg: body.agg,
        group,
        top: body.top,
    };
    with_source(app, fixture_requested(&req.query), |src| {
        Response::json(200, &json!({ "groups": src.series(&read) }))
    })
}

fn events_route(app: &App, req: &Request) -> Response {
    let pairs = query_pairs(&req.query);
    let now = apz_plaza::now_ms();
    let scope = value_of(&pairs, "scope").unwrap_or("global");
    let parsed = (
        number(&pairs, "from", now.saturating_sub(DAY_MS)),
        number(&pairs, "to", now),
        number(&pairs, "limit", 200_usize),
    );
    let (from_ms, to_ms, limit) = match parsed {
        (Ok(from), Ok(to), Ok(limit)) => (from, to, limit.clamp(1, 1000)),
        (Err(message), _, _) | (_, Err(message), _) | (_, _, Err(message)) => {
            return Response::error(400, &message);
        }
    };
    let kinds: Vec<&str> = value_of(&pairs, "kinds")
        .map(|raw| raw.split(',').filter(|kind| !kind.is_empty()).collect())
        .unwrap_or_default();
    with_source(app, fixture_requested(&req.query), |src| {
        Response::json(200, &src.events(scope, from_ms, to_ms, &kinds, limit))
    })
}

/// The call log's next page as a composed table (`scope`, `before`, `tz`, `fixture`), for "Load older".
fn calls_page_route(app: &App, req: &Request) -> Response {
    let pairs = query_pairs(&req.query);
    let before_ms = match number(&pairs, "before", apz_plaza::now_ms()) {
        Ok(before) => before,
        Err(message) => return Response::error(400, &message),
    };
    match parse_view(&req.query) {
        Ok(query) => with_source(app, query.fixture, |src| {
            Response::json(
                200,
                &apz_metrics_view::compose::tokens::calls_page(&query, src, before_ms),
            )
        }),
        Err(message) => Response::error(400, &message),
    }
}

fn calls_route(app: &App, req: &Request) -> Response {
    let pairs = query_pairs(&req.query);
    let scope = value_of(&pairs, "scope").unwrap_or("global");
    let parsed = (
        number(&pairs, "before", apz_plaza::now_ms()),
        number(&pairs, "limit", 50_usize),
    );
    let (before_ms, limit) = match parsed {
        (Ok(before), Ok(limit)) => (before, limit.clamp(1, 500)),
        (Err(message), _) | (_, Err(message)) => return Response::error(400, &message),
    };
    with_source(app, fixture_requested(&req.query), |src| {
        Response::json(200, &src.calls(scope, before_ms, limit))
    })
}

fn traces_route(app: &App, req: &Request) -> Response {
    let pairs = query_pairs(&req.query);
    let now = apz_plaza::now_ms();
    let scope = value_of(&pairs, "scope").unwrap_or("global");
    let parsed = (
        number(&pairs, "from", now.saturating_sub(DAY_MS)),
        number(&pairs, "to", now),
        number(&pairs, "limit", 100_usize),
    );
    let (from_ms, to_ms, limit) = match parsed {
        (Ok(from), Ok(to), Ok(limit)) => (from, to, limit.clamp(1, 500)),
        (Err(message), _, _) | (_, Err(message), _) | (_, _, Err(message)) => {
            return Response::error(400, &message);
        }
    };
    with_source(app, fixture_requested(&req.query), |src| {
        Response::json(200, &src.traces(scope, from_ms, to_ms, limit))
    })
}

fn trace_route(app: &App, req: &Request, params: &Params<'_>) -> Response {
    let id = params.get("id");
    with_source(app, fixture_requested(&req.query), |src| {
        match src.trace(id) {
            Some(trace) => Response::json(200, &trace),
            None => Response::error(404, &format!("no kept trace {id:?}")),
        }
    })
}

#[derive(Deserialize)]
struct MuteBody {
    until_ms: u64,
}

#[derive(Deserialize)]
struct AckBody {
    subject: String,
}

fn open_route(app: &App, params: &Params<'_>) -> Response {
    let cid = match params.get("cid").parse::<u64>() {
        Ok(cid) => cid,
        Err(_) => return Response::error(400, "cid must be an agent's number"),
    };
    // An agent stays open in the traces for half a minute after the console asks.
    app.metrics
        .traces
        .mark_open(cid, apz_plaza::now_ms() + 30_000);
    Response::json(200, &json!({ "ok": true }))
}

fn live_route(app: &App, req: &Request) -> Response {
    let lease: LiveLease = match json_body(req) {
        Ok(lease) => lease,
        Err(response) => return response,
    };
    app.metrics.live.lease(lease, apz_plaza::now_ms());
    Response::json(200, &json!({ "ok": true }))
}

fn put_settings_route(app: &App, req: &Request) -> Response {
    let new: MetricsSettings = match json_body(req) {
        Ok(new) => new,
        Err(response) => return response,
    };
    let was_on = app.metrics.settings.get().outside.openmetrics;
    let saved = app.metrics.settings.put(app.metrics.store.as_ref(), new);
    let mut value = match serde_json::to_value(&saved) {
        Ok(value) => value,
        Err(error) => return Response::error(500, &error.to_string()),
    };
    // The token is never sent again, except once: when OpenMetrics was just switched on.
    if saved.outside.openmetrics && !was_on {
        value["openmetrics_token"] = json!(saved.outside.openmetrics_token);
    }
    Response::json(200, &value)
}

fn dashboards_get_route(app: &App) -> Response {
    let saved = app
        .metrics
        .store
        .kv_get("dashboards")
        .and_then(|text| serde_json::from_str::<Saved>(&text).ok())
        .unwrap_or(Saved {
            starter: true,
            ..Saved::default()
        });
    Response::json(200, &saved)
}

fn dashboards_put_route(app: &App, req: &Request) -> Response {
    let mut saved: Saved = match json_body(req) {
        Ok(saved) => saved,
        Err(response) => return response,
    };
    // Once the owner saves anything, the starter pins stop being shown.
    saved.starter = false;
    let text = serde_json::to_string(&saved).unwrap_or_default();
    app.metrics.store.kv_put("dashboards", &text);
    Response::json(200, &saved)
}

fn alerts_route(app: &App) -> Response {
    let state = app
        .metrics
        .alerts
        .state(&app.metrics, app, apz_plaza::now_ms());
    Response::json(200, &state)
}

fn slos_route(app: &App) -> Response {
    let state = app
        .metrics
        .alerts
        .state(&app.metrics, app, apz_plaza::now_ms());
    Response::json(200, &state.slos)
}

fn put_rule_route(app: &App, req: &Request) -> Response {
    let rule: Rule = match json_body(req) {
        Ok(rule) => rule,
        Err(response) => return response,
    };
    match app
        .metrics
        .alerts
        .put_rule(app.metrics.store.as_ref(), rule)
    {
        Ok(rule) => Response::json(200, &json!({ "ok": true, "rule": rule })),
        Err(message) => Response::error(400, &message),
    }
}

fn delete_rule_route(app: &App, params: &Params<'_>) -> Response {
    let outcome = app
        .metrics
        .alerts
        .delete(app.metrics.store.as_ref(), params.get("id"));
    outcome_response(outcome)
}

fn mute_route(app: &App, req: &Request, params: &Params<'_>) -> Response {
    let body: MuteBody = match json_body(req) {
        Ok(body) => body,
        Err(response) => return response,
    };
    app.metrics.alerts.mute(
        app.metrics.store.as_ref(),
        params.get("id"),
        body.until_ms,
        apz_plaza::now_ms(),
    );
    Response::json(200, &json!({ "ok": true }))
}

#[derive(Deserialize)]
struct OnBody {
    on: bool,
}

/// Switches a rule (a starter or the owner's) on or off.
fn rule_on_route(app: &App, req: &Request, params: &Params<'_>) -> Response {
    let body: OnBody = match json_body(req) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let id = params.get("id");
    let Some(mut rule) = app.metrics.alerts.rules().into_iter().find(|r| r.id == id) else {
        return Response::error(404, "no such rule");
    };
    rule.on = body.on;
    match app
        .metrics
        .alerts
        .put_rule(app.metrics.store.as_ref(), rule)
    {
        Ok(rule) => Response::json(200, &json!({ "ok": true, "rule": rule })),
        Err(message) => Response::error(400, &message),
    }
}

fn ack_route(app: &App, req: &Request, params: &Params<'_>) -> Response {
    let body: AckBody = match json_body(req) {
        Ok(body) => body,
        Err(response) => return response,
    };
    app.metrics.alerts.ack(
        app.metrics.store.as_ref(),
        params.get("id"),
        &body.subject,
        apz_plaza::now_ms(),
    );
    Response::json(200, &json!({ "ok": true }))
}

/// The forecast is the capacity tab's `forecast` panel, so the route composes that tab and returns the panel.
fn forecast_route(app: &App, req: &Request) -> Response {
    let mut query = match parse_view(&req.query) {
        Ok(query) => query,
        Err(message) => return Response::error(400, &message),
    };
    query.tab = "capacity".into();
    with_source(app, query.fixture, |src| {
        let view = compose(&query, src);
        match view.panels.into_iter().find(|panel| panel.id == "forecast") {
            Some(panel) => Response::json(200, &panel),
            None => Response::error(404, "the capacity tab has no forecast panel"),
        }
    })
}

/// `/metrics`: 404 unless OpenMetrics is switched on, 401 without the token, else the OpenMetrics text.
fn metrics_route(app: &App, req: &Request) -> Response {
    let outside = app.metrics.settings.get().outside;
    if !outside.openmetrics {
        return Response::error(404, "not found");
    }
    let presented = req
        .header("authorization")
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    let allowed = !presented.is_empty()
        && apz_trust::ct_eq(presented.as_bytes(), outside.openmetrics_token.as_bytes());
    if !allowed {
        return Response::error(401, "the OpenMetrics token is missing or wrong");
    }
    Response::new(
        200,
        "text/plain; version=0.0.4",
        export::openmetrics(&app.metrics),
    )
}

/// Turns a store-free outcome into `{ok: true}`, or a 400 with its message.
fn outcome_response(outcome: Result<(), String>) -> Response {
    match outcome {
        Ok(()) => Response::json(200, &json!({ "ok": true })),
        Err(message) => Response::error(400, &message),
    }
}

/// The request body as `T`, or a 400 naming what did not parse.
fn json_body<T: DeserializeOwned>(req: &Request) -> Result<T, Response> {
    req.json::<T>()
        .map_err(|error| Response::error(400, &format!("the body is not valid: {error}")))
}

/// Parses the view's query string into a [`ViewQuery`]. Defaults: tab `overview`, scope `global`, range `24h`,
/// compare `prev`, group `provider`, width 900, tz 0, no zoom, no fixture.
pub fn parse_view(query: &str) -> Result<ViewQuery, String> {
    let pairs = query_pairs(query);
    let tab = value_of(&pairs, "tab").unwrap_or("overview");
    if !TABS.iter().any(|(id, _, _)| *id == tab) {
        return Err(format!("tab must be one of the view's tabs, not {tab:?}"));
    }
    let range = value_of(&pairs, "range").unwrap_or("24h");
    if !RANGES.iter().any(|(name, _)| *name == range) {
        return Err(format!(
            "range must be one of 15m, 1h, 6h, 24h, 7d, 30d, 90d, not {range:?}"
        ));
    }
    let scope = value_of(&pairs, "scope").unwrap_or("global");
    let mut view = ViewQuery::new(tab, scope, range);
    view.shift = number(&pairs, "shift", view.shift)?;
    view.zoom = match value_of(&pairs, "zoom") {
        Some(raw) => Some(parse_zoom(raw)?),
        None => None,
    };
    if let Some(compare) = value_of(&pairs, "compare") {
        view.compare = compare.to_string();
    }
    if let Some(group) = value_of(&pairs, "group") {
        view.group = group.to_string();
    }
    view.trace = value_of(&pairs, "trace").map(str::to_string);
    view.filter = value_of(&pairs, "filter").map(str::to_string);
    view.width = number(&pairs, "width", view.width)?;
    if !(1..=4000).contains(&view.width) {
        return Err(format!("width must be 1..=4000, not {}", view.width));
    }
    view.tz = number(&pairs, "tz", view.tz)?;
    view.fixture = matches!(value_of(&pairs, "fixture"), Some("1" | "true"));
    Ok(view)
}

/// `zoom=a,b`: two ascending fractions of the range, from 0 to 1.
fn parse_zoom(raw: &str) -> Result<(f64, f64), String> {
    let refusal = || format!("zoom must be two fractions a,b with 0 <= a < b <= 1, not {raw:?}");
    let (start, end) = raw.split_once(',').ok_or_else(refusal)?;
    let start: f64 = start.trim().parse().map_err(|_| refusal())?;
    let end: f64 = end.trim().parse().map_err(|_| refusal())?;
    if (0.0..=1.0).contains(&start) && (0.0..=1.0).contains(&end) && start < end {
        Ok((start, end))
    } else {
        Err(refusal())
    }
}

/// `group=all|entity|label:<label>|prop:<property>` of a [`Read`].
fn parse_group(raw: &str) -> Result<GroupBy, String> {
    let refusal =
        || format!("group must be all, entity, label:<label> or prop:<property>, not {raw:?}");
    match raw {
        "all" => Ok(GroupBy::All),
        "entity" => Ok(GroupBy::Entity),
        _ => {
            if let Some(label) = raw.strip_prefix("label:").filter(|label| !label.is_empty()) {
                Ok(GroupBy::Label(label.to_string()))
            } else if let Some(property) = raw.strip_prefix("prop:").filter(|name| !name.is_empty())
            {
                Ok(GroupBy::AgentProp(property.to_string()))
            } else {
                Err(refusal())
            }
        }
    }
}

fn fixture_requested(query: &str) -> bool {
    matches!(value_of(&query_pairs(query), "fixture"), Some("1" | "true"))
}

/// The query string as decoded (name, value) pairs in order; a bare name has an empty value.
fn query_pairs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(name), decode(value))
        })
        .collect()
}

/// The first value of `name`; an empty value counts as absent.
fn value_of<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
        .filter(|value| !value.is_empty())
}

/// A number parameter, or `default` when absent.
fn number<T: FromStr>(pairs: &[(String, String)], name: &str, default: T) -> Result<T, String> {
    match value_of(pairs, name) {
        None => Ok(default),
        Some(raw) => raw
            .parse::<T>()
            .map_err(|_| format!("{name} must be a number, not {raw:?}")),
    }
}

/// Percent-decodes a query component, with `+` as a space.
fn decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' => {
                let escaped = raw
                    .get(index + 1..index + 3)
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok());
                match escaped {
                    Some(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    None => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_with_no_parameters_takes_the_documented_defaults() {
        let view = parse_view("").unwrap();
        assert_eq!(view.tab, "overview");
        assert_eq!(view.scope, "global");
        assert_eq!(view.range, "24h");
        assert_eq!(view.compare, "prev");
        assert_eq!(view.group, "provider");
        assert_eq!(view.width, 900);
        assert_eq!(view.tz, 0);
        assert_eq!(view.shift, 0);
        assert_eq!(view.zoom, None);
        assert!(!view.fixture);
    }

    #[test]
    fn a_zoom_is_two_ascending_fractions_of_the_range() {
        let view = parse_view("zoom=0.25,0.75").unwrap();
        assert_eq!(view.zoom, Some((0.25, 0.75)));
        assert!(parse_view("zoom=0.8,0.2").is_err());
        assert!(parse_view("zoom=0.5,1.5").is_err());
        assert!(parse_view("zoom=half").is_err());
    }

    #[test]
    fn a_fixture_view_composes_the_overview_from_the_built_in_data() {
        let query = parse_view("tab=overview&fixture=1").unwrap();
        assert!(query.fixture);
        let response = view_response(&query, &Fixture::new(apz_plaza::now_ms()));
        let value: serde_json::Value = serde_json::from_slice(response.bytes()).unwrap();
        assert_eq!(value["tab"], "overview");
        assert!(
            value["panels"]
                .as_array()
                .is_some_and(|panels| !panels.is_empty())
        );
    }

    #[test]
    fn an_escaped_scope_is_decoded_before_it_is_read() {
        let view = parse_view("scope=provider%3Aanthropic&range=7d").unwrap();
        assert_eq!(view.scope, "provider:anthropic");
        assert_eq!(view.range, "7d");
    }

    #[test]
    fn an_unknown_tab_or_range_is_refused() {
        assert!(parse_view("tab=nowhere").is_err());
        assert!(parse_view("range=2w").is_err());
    }

    #[test]
    fn a_group_parameter_names_its_grouping_or_is_refused() {
        assert_eq!(parse_group("all"), Ok(GroupBy::All));
        assert_eq!(parse_group("entity"), Ok(GroupBy::Entity));
        assert_eq!(
            parse_group("label:model"),
            Ok(GroupBy::Label("model".into()))
        );
        assert_eq!(
            parse_group("prop:role"),
            Ok(GroupBy::AgentProp("role".into()))
        );
        assert!(parse_group("label:").is_err());
        assert!(parse_group("by-role").is_err());
    }
}
