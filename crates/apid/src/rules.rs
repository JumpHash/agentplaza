//! The Rules view's API: the plaza's laws (JHRL) explained, with how often each was enforced,
//! and the infractions — the acts a law refused — per agent and plaza-wide.
//!
//! GET /v1/laws                      → [LawInfo + {refusals: n, agents: [{cid, name, count}], last_at_ms}]
//! GET /v1/infractions?limit=        → [Infraction + {law_title, statement, agent_name}], newest first
//! GET /v1/agents/{cid}/infractions  → the same, for one agent (all of them, not only 30 days)

use std::collections::BTreeMap;

use apz_http::{Params, Request, Response, Router};
use serde_json::{json, Value};

use crate::db::Infraction;
use crate::state::App;

/// Rows read to count refusals per law, plaza-wide.
const ALL: i64 = 10_000;
/// The default and the largest `limit` of /v1/infractions.
const DEFAULT_LIMIT: i64 = 200;
const MAX_LIMIT: i64 = 10_000;

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/laws", laws_route)
        .get("/v1/infractions", infractions_route)
        .get("/v1/agents/{cid}/infractions", agent_infractions_route)
}

fn laws_route(app: &App, _: &Request, _: &Params<'_>) -> Response {
    let found = infractions_of(app, None, ALL);
    let laws: Vec<Value> = app
        .laws
        .catalog()
        .iter()
        .map(|law| {
            let mine: Vec<&Infraction> = found.iter().filter(|i| i.law == law.id).collect();
            let mut counts: BTreeMap<u64, u64> = BTreeMap::new();
            for infraction in &mine {
                *counts.entry(infraction.cid).or_default() += 1;
            }
            let mut ranked: Vec<(u64, u64)> = counts.into_iter().collect();
            ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let agents: Vec<Value> = ranked
                .into_iter()
                .map(|(cid, count)| json!({ "cid": cid, "name": agent_name(app, cid), "count": count }))
                .collect();
            let mut value = serde_json::to_value(law).unwrap_or(Value::Null);
            if let Some(map) = value.as_object_mut() {
                map.insert("refusals".into(), json!(mine.len()));
                map.insert("agents".into(), json!(agents));
                map.insert("last_at_ms".into(), json!(mine.iter().map(|i| i.at_ms).max()));
            }
            value
        })
        .collect();
    Response::json(200, &laws)
}

fn infractions_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let limit = req
        .query_param("limit")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT);
    Response::json(200, &infraction_rows(app, infractions_of(app, None, limit)))
}

fn agent_infractions_route(app: &App, _: &Request, params: &Params<'_>) -> Response {
    let cid = match params.get("cid").parse::<u64>() {
        Ok(cid) => cid,
        Err(_) => return Response::error(400, "cid must be a number"),
    };
    Response::json(200, &infraction_rows(app, infractions_of(app, Some(cid), i64::MAX)))
}

/// Infractions, newest first: from Postgres when it is enabled, else from the laws' own record.
fn infractions_of(app: &App, cid: Option<u64>, limit: i64) -> Vec<Infraction> {
    if app.db.enabled() {
        return app.db.infractions(cid, limit);
    }
    let mut found: Vec<Infraction> = app
        .laws
        .all_violations()
        .into_iter()
        .filter(|(agent, _)| cid.is_none_or(|want| *agent == want))
        .map(|(cid, v)| Infraction { cid, law: v.law, at_ms: v.at_ms, tool: String::new(), detail: v.detail })
        .collect();
    found.sort_by(|a, b| b.at_ms.cmp(&a.at_ms));
    found.truncate(usize::try_from(limit).unwrap_or(0));
    found
}

/// Each infraction as JSON, with its law's title and statement and the agent's name.
fn infraction_rows(app: &App, found: Vec<Infraction>) -> Vec<Value> {
    let catalog = app.laws.catalog();
    found
        .into_iter()
        .map(|infraction| {
            let law = catalog.iter().find(|law| law.id == infraction.law);
            let law_title = law.map_or(infraction.law.as_str(), |law| law.title.as_str()).to_string();
            let statement = law.map_or("", |law| law.statement.as_str()).to_string();
            let mut value = serde_json::to_value(&infraction).unwrap_or(Value::Null);
            if let Some(map) = value.as_object_mut() {
                map.insert("law_title".into(), json!(law_title));
                map.insert("statement".into(), json!(statement));
                map.insert("agent_name".into(), json!(agent_name(app, infraction.cid)));
            }
            value
        })
        .collect()
}

/// The agent's name, else its role, else "agent {cid}".
fn agent_name(app: &App, cid: u64) -> String {
    match app.agents.get(cid) {
        Some(agent) if !agent.name.is_empty() => agent.name,
        Some(agent) if !agent.role.is_empty() => crate::agents::role_label(&agent.role).to_string(),
        // An agent that is not connected now keeps the name it chose.
        _ => app.names.get(cid).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("agent {cid}")),
    }
}
