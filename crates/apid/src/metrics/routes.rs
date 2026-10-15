//! `/v1/metrics/*` (owner only, like everything behind the hub's gate; agents never reach it) and the opt-in
//! `/metrics` (OpenMetrics, behind its own token).
//!
//! * `GET /v1/metrics/catalog` — the catalog (`apz_metrics::CATALOG`).
//! * `GET /v1/metrics/scopes` — the scope tree.
//! * `GET /v1/metrics/view?tab&scope&range&shift&zoom&compare&group&trace&filter&width&tz&fixture` — a composed
//!   [`apz_metrics_view::View`] (the fixture when `fixture=1`).
//! * `GET /v1/metrics/around?…same…&t=` — "what changed around then" ([`apz_metrics_view::Around`]).
//! * `POST /v1/metrics/query` — one [`apz_metrics_view::source::Read`]-shaped read, as grouped series (CLI, tests).
//! * `GET /v1/metrics/events?scope&from&to&kinds&limit`, `GET /v1/metrics/calls?scope&before&limit`,
//!   `GET /v1/metrics/traces?scope&from&to`, `GET /v1/metrics/traces/{id}`.
//! * `GET /v1/metrics/export?…view query…` — the view's chart series as CSV.
//! * `POST /v1/metrics/live` — a [`apz_metrics_view::LiveLease`]; `POST /v1/metrics/open/{cid}` — an agent is open.
//! * `GET`/`PUT /v1/metrics/settings`; `GET`/`PUT /v1/metrics/dashboards` ([`apz_metrics_view::Saved`]).
//! * `GET /v1/metrics/alerts`; `PUT /v1/metrics/alerts/rules` (a rule, upsert); `DELETE
//!   /v1/metrics/alerts/rules/{id}`; `POST /v1/metrics/alerts/{id}/mute` `{until_ms}`; `POST
//!   /v1/metrics/alerts/{id}/ack` `{subject}`; `GET /v1/metrics/slos`; `GET /v1/metrics/forecast`.
//! * `POST /v1/nodes/{id}/metrics` lives in nodes.rs (signed by the node); this module ingests it.
//! * `GET /metrics` — OpenMetrics text with `Authorization: Bearer <openmetrics token>`, 404 when off.

use apz_http::Router;

use crate::state::App;

pub fn routes(router: Router<App>) -> Router<App> {
    router
}
