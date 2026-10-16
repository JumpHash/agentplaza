//! The Rules view's API: the plaza's laws (JHRL) explained, with how often each was enforced,
//! and the infractions — the acts a law refused — per agent and plaza-wide.
//!
//! GET /v1/laws                      → [LawInfo + {refusals: n, agents: [{cid, name, count}], last_at_ms}]
//! GET /v1/infractions?limit=        → [Infraction + {law_title, statement, agent_name}], newest first
//! GET /v1/agents/{cid}/infractions  → the same, for one agent (all of them, not only 30 days)

use std::collections::BTreeMap;

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde_json::{Value, json};

use crate::db::Infraction;
use crate::laws::{Failure, Override};
use crate::state::App;

/// Rows read to count refusals per law, plaza-wide.
const ALL: i64 = 10_000;
/// The default and the largest `limit` of /v1/infractions.
const DEFAULT_LIMIT: i64 = 200;
const MAX_LIMIT: i64 = 10_000;

/// The Rules view's routes. Every one is owner-only: `hub_auth::gate` asks for the owner token of any
/// `/v1/` request that does not come from loopback.
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/laws", laws_route)
        .doc(RouteDoc::new(
            "List the laws and how often each refused a call",
        ))
        .get("/v1/laws/rules", rules_list_route)
        .doc(RouteDoc::new("List the owner's rules and overrides"))
        .post("/v1/laws/rules", rule_create_route)
        .doc(RouteDoc::new("Add a rule").status(201).errors(&[400]))
        .route("PUT", "/v1/laws/rules/{id}", rule_update_route)
        .doc(RouteDoc::new("Edit a rule").errors(&[400, 404]))
        .post("/v1/laws/rules/{id}/enable", rule_enable_route)
        .doc(RouteDoc::new("Enable a rule").errors(&[404]))
        .post("/v1/laws/rules/{id}/disable", rule_disable_route)
        .doc(RouteDoc::new("Disable a rule").errors(&[404]))
        .route("DELETE", "/v1/laws/rules/{id}", rule_delete_route)
        .doc(RouteDoc::new("Delete a rule").errors(&[404]))
        .post("/v1/laws/check", check_route)
        .doc(RouteDoc::new("Test a rule source without applying it").errors(&[400]))
        .route("PUT", "/v1/laws/overrides/{law}", override_route)
        .doc(RouteDoc::new("Switch a built-in law on or off").errors(&[400, 404, 409]))
        .get("/v1/laws/violations", violations_route)
        .doc(RouteDoc::new("List the recorded refusals of a law"))
        .get("/v1/infractions", infractions_route)
        .doc(RouteDoc::new("List the recent infractions"))
        .get("/v1/agents/{cid}/infractions", agent_infractions_route)
        .doc(RouteDoc::new("List one agent's infractions"))
}

/// The owner's rules and overrides, whether permits are strict, the constitutional laws switched off,
/// and the history of changes to constitutional laws.
fn rules_list_route(app: &App, _: &Request, _: &Params<'_>) -> Response {
    let set = app.laws.rules();
    Response::json(
        200,
        &json!({
            "rules": set.rules,
            "overrides": set.overrides,
            "strict_permits": app.laws.strict(),
            "constitutional_off": app.laws.constitutional_off(),
            "history": set.history,
        }),
    )
}

/// `POST /v1/laws/rules` with `{title, source, enabled}`: a new rule set, applied only if it compiles.
fn rule_create_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let body = match body_of(req) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let enabled = body.get("enabled").and_then(Value::as_bool).unwrap_or(true);
    match app.laws.add_rule(
        &text_field(&body, "title"),
        &text_field(&body, "source"),
        enabled,
        apz_plaza::now_ms(),
    ) {
        Ok(rule) => Response::json(201, &rule),
        Err(failure) => failure_response(failure),
    }
}

/// `PUT /v1/laws/rules/{id}` with `{title?, source?}`: an edited rule set, applied only if it compiles.
fn rule_update_route(app: &App, req: &Request, params: &Params<'_>) -> Response {
    let body = match body_of(req) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let source = body.get("source").and_then(Value::as_str);
    match app.laws.update_rule(
        params.get("id"),
        &text_field(&body, "title"),
        source,
        apz_plaza::now_ms(),
    ) {
        Ok(rule) => Response::json(200, &rule),
        Err(failure) => failure_response(failure),
    }
}

fn rule_enable_route(app: &App, _: &Request, params: &Params<'_>) -> Response {
    set_enabled(app, params.get("id"), true)
}

fn rule_disable_route(app: &App, _: &Request, params: &Params<'_>) -> Response {
    set_enabled(app, params.get("id"), false)
}

fn set_enabled(app: &App, id: &str, enabled: bool) -> Response {
    match app.laws.set_rule_enabled(id, enabled, apz_plaza::now_ms()) {
        Ok(rule) => Response::json(200, &rule),
        Err(failure) => failure_response(failure),
    }
}

fn rule_delete_route(app: &App, _: &Request, params: &Params<'_>) -> Response {
    match app.laws.delete_rule(params.get("id")) {
        Ok(()) => Response::json(200, &json!({ "deleted": params.get("id") })),
        Err(failure) => failure_response(failure),
    }
}

/// `POST /v1/laws/check` with `{source, id?}`: a dry run (`id` names the rule the source would replace). A source that does not compile comes back with
/// its diagnostics (status 200, `ok` false); one that does lists the laws it declares and the recent
/// calls they would have refused. Nothing is applied.
fn check_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let body = match body_of(req) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let replaces = body.get("id").and_then(Value::as_str);
    match app.laws.dry_run(&text_field(&body, "source"), replaces) {
        Ok(run) => Response::json(
            200,
            &json!({ "ok": true, "diagnostics": [], "laws": run.laws, "would_refuse": run.would_refuse }),
        ),
        Err(Failure::Diagnostics(list)) => Response::json(
            200,
            &json!({ "ok": false, "diagnostics": list, "laws": [], "would_refuse": [] }),
        ),
        Err(failure) => failure_response(failure),
    }
}

/// `PUT /v1/laws/overrides/{law}` with `{disabled, reason, confirm}`: disables a built-in statute, with the
/// reason on record, or enables it again (`disabled` false). A constitutional law is switched off only
/// with `confirm` set to its exact id (409 otherwise).
fn override_route(app: &App, req: &Request, params: &Params<'_>) -> Response {
    let body = match body_of(req) {
        Ok(body) => body,
        Err(response) => return response,
    };
    set_override_answer(
        app.laws,
        &app.bus,
        params.get("law"),
        &body,
        apz_plaza::now_ms(),
    )
}

/// Sets or clears one built-in override from a request body (`disabled`, `reason`, and `confirm`,
/// the typed law id a constitutional law needs to be switched off). A constitutional law's change
/// goes out on the event stream as `law`, with the laws now off.
fn set_override_answer(
    laws: &crate::laws::Laws,
    bus: &crate::events::Bus,
    law: &str,
    body: &Value,
    now_ms: u64,
) -> Response {
    let disabled = body
        .get("disabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    match laws.set_override(
        law,
        disabled,
        &text_field(body, "reason"),
        &text_field(body, "confirm"),
        now_ms,
    ) {
        Ok(made) => {
            let made: Option<Override> = made;
            announce_constitutional(laws, bus, law, now_ms);
            Response::json(200, &json!({ "law": law, "override": made }))
        }
        Err(failure) => failure_response(failure),
    }
}

/// Publishes a `law` event for a constitutional law, so every console shows the change and the
/// laws that are switched off now. Other tiers publish nothing.
fn announce_constitutional(
    laws: &crate::laws::Laws,
    bus: &crate::events::Bus,
    law: &str,
    now_ms: u64,
) {
    let catalog = laws.catalog();
    let Some(info) = catalog.iter().find(|info| info.id == law) else {
        return;
    };
    if info.tier != crate::laws::CONSTITUTION {
        return;
    }
    bus.publish(
        "law",
        &json!({
            "law": info.id,
            "title": info.title,
            "tier": info.tier,
            "enabled": info.enabled,
            "by": crate::laws::OWNER,
            "at_ms": now_ms,
            "constitutional_off": crate::laws::constitutional_off(&catalog),
        }),
    );
}

/// `GET /v1/laws/violations?law=&limit=`: the recorded refusals, newest first, of one law (or of all).
fn violations_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let law = req.query_param("law");
    let limit = req
        .query_param("limit")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT);
    let found: Vec<Infraction> = infractions_of(app, None, ALL)
        .into_iter()
        .filter(|infraction| law.is_none_or(|law| infraction.law == law))
        .take(usize::try_from(limit).unwrap_or(0))
        .collect();
    Response::json(200, &infraction_rows(app, found))
}

/// The request's JSON body as an object, or a 400 saying why it is not one.
fn body_of(req: &Request) -> Result<Value, Response> {
    match req.json::<Value>() {
        Ok(body) if body.is_object() => Ok(body),
        Ok(_) => Err(Response::json(
            400,
            &json!({ "error": "the body must be a JSON object" }),
        )),
        Err(e) => Err(Response::json(
            400,
            &json!({ "error": format!("bad JSON: {e}") }),
        )),
    }
}

fn text_field(body: &Value, key: &str) -> String {
    body.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A refused change: 404 for an unknown id, 400 otherwise with the reason (and the diagnostics, when
/// the source does not compile).
fn failure_response(failure: Failure) -> Response {
    match &failure {
        Failure::NotFound(_) => Response::json(404, &json!({ "error": failure.message() })),
        Failure::Invalid(_) => Response::json(400, &json!({ "error": failure.message() })),
        Failure::Unconfirmed(_) => Response::json(409, &json!({ "error": failure.message() })),
        Failure::Diagnostics(list) => Response::json(
            400,
            &json!({ "error": "the source does not compile", "diagnostics": list }),
        ),
    }
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
                // A constitutional law that is switched off is flagged in the list itself.
                map.insert("constitutional_off".into(), json!(law.tier == crate::laws::CONSTITUTION && !law.enabled));
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
    Response::json(
        200,
        &infraction_rows(app, infractions_of(app, Some(cid), i64::MAX)),
    )
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
        .map(|(cid, v)| Infraction {
            cid,
            law: v.law,
            at_ms: v.at_ms,
            tool: String::new(),
            detail: v.detail,
        })
        .collect();
    found.sort_by_key(|a| std::cmp::Reverse(a.at_ms));
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
            let law_title = law
                .map_or(infraction.law.as_str(), |law| law.title.as_str())
                .to_string();
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
        _ => app
            .names
            .get(cid)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("agent {cid}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Bus;
    use crate::laws::{Laws, OWNER};
    use apz_http::Body;

    const C: &str = "C-002b";

    fn laws(name: &str) -> Laws {
        let dir =
            std::env::temp_dir().join(format!("apid-rules-route-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Laws::load(&dir)
    }

    fn answer(laws: &Laws, bus: &Bus, law: &str, body: Value) -> Response {
        set_override_answer(laws, bus, law, &body, 5_000)
    }

    fn json_of(response: &Response) -> Value {
        match &response.body {
            Body::Bytes(bytes) => serde_json::from_slice(bytes).unwrap(),
            other => panic!("not a JSON body: {other:?}"),
        }
    }

    #[test]
    fn switching_a_law_off_depends_on_its_tier() {
        // A constitutional switch-off without the typed id is a 409 and applies nothing.
        {
            let laws = laws("no-confirm");
            let bus = Bus::default();
            let asked =
                format!("{C} is a constitutional law; to switch it off, send confirm: \"{C}\"");
            let none = answer(
                &laws,
                &bus,
                C,
                json!({"disabled": true, "reason": "testing"}),
            );
            assert_eq!(none.status, 409);
            assert_eq!(json_of(&none)["error"], asked);
            let mismatch = answer(
                &laws,
                &bus,
                C,
                json!({"disabled": true, "reason": "testing", "confirm": "C-004a"}),
            );
            assert_eq!(mismatch.status, 409);
            assert!(
                json_of(&mismatch)["error"]
                    .as_str()
                    .unwrap()
                    .ends_with(&asked)
            );
            // Refused changes apply nothing: the law still holds, and no change is on record.
            assert!(laws.constitutional_off().is_empty());
            assert!(laws.rules().history.is_empty());
        }
        // A statute of another tier switches off as before, with no event.
        {
            let laws = laws("statute");
            let bus = Bus::default();
            let events = bus.subscribe();
            let off = answer(
                &laws,
                &bus,
                "S-013d",
                json!({"disabled": true, "reason": "researchers may commit"}),
            );
            assert_eq!(off.status, 200);
            assert!(laws.rules().history.is_empty());
            assert!(events.try_recv().is_err(), "no event for a statute");
            assert!(laws.constitutional_off().is_empty());
        }
    }

    #[test]
    fn a_confirmed_switch_off_is_recorded_and_announced_and_switching_on_needs_nothing() {
        let laws = laws("confirmed");
        let bus = Bus::default();
        let events = bus.subscribe();
        let off = answer(
            &laws,
            &bus,
            C,
            json!({"disabled": true, "reason": "testing", "confirm": C}),
        );
        assert_eq!(off.status, 200);
        assert_eq!(laws.constitutional_off(), vec![C.to_string()]);
        let history = laws.rules().history;
        assert_eq!(history.len(), 1);
        assert_eq!(
            (
                history[0].law.as_str(),
                history[0].by.as_str(),
                history[0].at_ms,
                history[0].disabled
            ),
            (C, OWNER, 5_000, true)
        );
        assert!(!history[0].title.is_empty());
        let frame = events.try_recv().expect("a law event");
        assert_eq!(frame.kind, "law");
        let data: Value = serde_json::from_str(&frame.data).unwrap();
        assert_eq!(
            (data["law"].as_str(), data["enabled"].as_bool()),
            (Some(C), Some(false))
        );
        assert_eq!(data["constitutional_off"], json!([C]));
        // Switching it back on: no confirmation, recorded, and announced with nothing off.
        let on = answer(&laws, &bus, C, json!({"disabled": false}));
        assert_eq!(on.status, 200);
        assert!(laws.constitutional_off().is_empty());
        assert_eq!(laws.rules().history.len(), 2);
        let data: Value =
            serde_json::from_str(&events.try_recv().expect("a law event").data).unwrap();
        assert_eq!(
            (
                data["enabled"].as_bool(),
                data["constitutional_off"].as_array().map(Vec::len)
            ),
            (Some(true), Some(0))
        );
    }
}
