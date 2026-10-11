//! The hub's internal endpoints: hello, every v3 guest line, gone.
//!
//! OWNER: slice "loop". `handle` is the agent loop's host half.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use apz_http::{Request, Response, Router};
use apz_proto::chat::{ChatRequest, Message, Role};
use apz_proto::wire::{cap_output, Decision, GuestMessage, HostMessage};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agents::{model_of, profile, Agent};
use crate::settings::tier_of;
use crate::state::App;
use crate::tools::{run_host, runs, Runs};

/// How much of the last user turn the transcript keeps, in characters.
const LAST_USER_CHARS: usize = 2_000;
/// How much of a tool decision's output the transcript keeps, in bytes.
const DECISION_OUTPUT_BYTES: usize = 4_000;
/// How much of a tool report's output the transcript keeps, in bytes.
const REPORT_OUTPUT_BYTES: usize = 16_000;

#[derive(Deserialize)]
struct Hello {
    #[serde(default)]
    role: String,
    #[serde(default)]
    vm: u32,
}

#[derive(Deserialize)]
struct Gone {
    #[serde(default)]
    reason: String,
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .post("/internal/v1/agents/{cid}/hello", |app, req, p| {
            authorized(app, req, p.get("cid"), |cid| {
                let hello: Hello = req.json().map_err(|e| Response::error(400, &e.to_string()))?;
                let profile = crate::agents::hello(app, cid, &hello.role, hello.vm);
                Ok(Response::json(200, &json!({ "profile": profile })))
            })
        })
        .post("/internal/v1/agents/{cid}/message", |app, req, p| {
            authorized(app, req, p.get("cid"), |cid| {
                let message: GuestMessage = req.json().map_err(|e| Response::error(400, &e.to_string()))?;
                let reply = handle(app, cid, message);
                Ok(Response::json(200, &json!({ "reply": reply })))
            })
        })
        .post("/internal/v1/agents/{cid}/gone", |app, req, p| {
            authorized(app, req, p.get("cid"), |cid| {
                let gone: Gone = req.json().unwrap_or(Gone { reason: String::new() });
                crate::agents::gone(app, cid, &gone.reason);
                Ok(Response::json(200, &json!({})))
            })
        })
}

/// Checks the internal token and the CID, then runs `f`.
fn authorized(
    app: &App,
    req: &Request,
    cid: &str,
    f: impl FnOnce(u64) -> Result<Response, Response>,
) -> Response {
    if req.header("x-apz-internal") != Some(app.token.as_str()) {
        return Response::error(403, "missing or wrong internal token");
    }
    let Ok(cid) = cid.parse::<u64>() else {
        return Response::error(400, "the CID must be a number");
    };
    f(cid).unwrap_or_else(|response| response)
}

/// One v3 guest line from agent `cid`; the reply for the guest, if any.
// contract
pub fn handle(app: &App, cid: u64, message: GuestMessage) -> Option<HostMessage> {
    let Some(agent) = app.agents.get(cid) else {
        return match message {
            GuestMessage::ModelCall { req, .. } => Some(HostMessage::ModelResult {
                req,
                ok: false,
                response: None,
                error: Some("unknown agent".to_string()),
            }),
            GuestMessage::ToolRequest { call, .. } => Some(HostMessage::ToolDecision {
                call,
                decision: Decision::Deny,
                output: "unknown agent".to_string(),
                is_error: true,
            }),
            _ => None,
        };
    };
    match message {
        GuestMessage::ModelCall { req, work, request } => {
            Some(model_call(app, cid, &agent, req, work, request))
        }
        GuestMessage::ToolRequest { work, call, name, input } => {
            Some(tool_request(app, cid, &agent, work, call, &name, &input))
        }
        GuestMessage::ToolReport { work, call, ok, output } => {
            let shared = pending().lock().unwrap_or_else(PoisonError::into_inner).remove(&(cid, call.clone()));
            if let (Some((_, path)), true) = (shared, ok) {
                app.artifacts.add(&app.bus, cid, work, &path, output.clone());
            }
            app.transcripts.record(
                &app.bus,
                cid,
                work,
                "tool_result",
                json!({ "call": call, "ok": ok, "output": cap_output(&output, REPORT_OUTPUT_BYTES) }),
            );
            None
        }
        GuestMessage::WorkDone { work, status, summary } => {
            app.work.complete(app, cid, work, status, &summary);
            None
        }
        _ => None,
    }
}

/// Runs one model call through the gateway, streams its deltas, and records it.
fn model_call(
    app: &App,
    cid: u64,
    agent: &Agent,
    req: u64,
    work: u64,
    mut request: ChatRequest,
) -> HostMessage {
    let model = model_of(app, agent);
    let settings = app.settings.get();
    if let Err(e) = app.settings.allowed(tier_of(&agent.role), &model) {
        app.transcripts.record(&app.bus, cid, work, "error", json!({ "message": e }));
        return HostMessage::ModelResult { req, ok: false, response: None, error: Some(e) };
    }
    request.model = model.clone();
    if request.max_tokens == 0 {
        request.max_tokens = settings.max_tokens;
    }
    if request.effort.is_none() {
        request.effort = settings.effort.clone();
    }
    let tool_names: Vec<&str> = request.tools.iter().map(|t| t.name.as_str()).collect();
    app.transcripts.record(
        &app.bus,
        cid,
        work,
        "model_request",
        request_summary(&model, &request.messages, &tool_names),
    );

    let gateway = app.providers.gateway();
    let result = gateway.complete(&request, &mut |delta| {
        app.bus.publish("delta", &json!({ "cid": cid, "work": work, "req": req, "delta": delta }));
    });
    match result {
        Ok(resp) => {
            let full = serde_json::to_value(&resp).unwrap_or(Value::Null);
            app.transcripts.record(&app.bus, cid, work, "model_response", full);
            let cost = settings
                .prices
                .get(&model)
                .map(|p| {
                    (resp.usage.input_tokens as f64 * p.input + resp.usage.output_tokens as f64 * p.output)
                        / 1_000_000.0
                })
                .unwrap_or(0.0);
            app.agents.update(cid, |a| {
                a.usage.add(&resp.usage);
                a.credits += cost;
            });
            HostMessage::ModelResult { req, ok: true, response: Some(resp), error: None }
        }
        Err(e) => {
            let message = e.to_string();
            app.transcripts.record(&app.bus, cid, work, "error", json!({ "message": message }));
            HostMessage::ModelResult { req, ok: false, response: None, error: Some(message) }
        }
    }
}

/// Share requests allowed to run, keyed by (cid, call), until their report
/// arrives: (tool name, requested path).
type Pending = Mutex<HashMap<(u64, String), (String, String)>>;

fn pending() -> &'static Pending {
    static PENDING: OnceLock<Pending> = OnceLock::new();
    PENDING.get_or_init(Pending::default)
}

/// Decides and, for host tools, runs one tool request from the model.
fn tool_request(
    app: &App,
    cid: u64,
    agent: &Agent,
    work: u64,
    call: String,
    name: &str,
    input: &Value,
) -> HostMessage {
    app.transcripts.record(
        &app.bus,
        cid,
        work,
        "tool_request",
        json!({ "call": call, "name": name, "input": input }),
    );
    let granted: Vec<String> = profile(app, agent).tools.into_iter().map(|t| t.name).collect();
    let (decision, output, is_error) = match decide(&granted, name, runs(name)) {
        Err(reason) => (Decision::Deny, reason.to_string(), true),
        Ok(Runs::Guest) => {
            if name == "share_file" {
                let path = input.get("path").and_then(Value::as_str).unwrap_or_default().to_string();
                pending().lock().unwrap_or_else(PoisonError::into_inner).insert((cid, call.clone()), (name.to_string(), path));
            }
            (Decision::Allow, String::new(), false)
        }
        Ok(Runs::Host) => {
            let (is_error, output) = run_host(app, agent, work, name, input);
            (Decision::Done, output, is_error)
        }
    };
    app.transcripts.record(
        &app.bus,
        cid,
        work,
        "tool_decision",
        json!({
            "call": call,
            "decision": decision,
            "output": cap_output(&output, DECISION_OUTPUT_BYTES),
        }),
    );
    HostMessage::ToolDecision { call, decision, output, is_error }
}

/// The pure tool policy: `Ok(where)` when the tool may run, `Err(reason)` when it may not.
fn decide(granted: &[String], name: &str, runs: Option<Runs>) -> Result<Runs, &'static str> {
    if !granted.iter().any(|g| g == name) {
        return Err("not granted to this agent");
    }
    runs.ok_or("unknown tool")
}

/// The bounded summary of a model request kept in the transcript: counts, tool names
/// and the last user turn clipped to `LAST_USER_CHARS`, never the full body.
fn request_summary(model: &str, messages: &[Message], tools: &[&str]) -> Value {
    let last_user: String = messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, Role::User))
        .map(|m| m.text().chars().take(LAST_USER_CHARS).collect())
        .unwrap_or_default();
    json!({
        "model": model,
        "messages": messages.len(),
        "tools": tools,
        "last_user": last_user,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decide_denies_ungranted_allows_guest_and_denies_unknown() {
        let granted = vec!["read".to_string(), "calc".to_string()];
        // An ungranted tool is denied even when a runner exists for it.
        assert!(matches!(decide(&granted, "bash", Some(Runs::Guest)), Err("not granted to this agent")));
        // A granted guest tool may run in the VM.
        assert!(matches!(decide(&granted, "read", Some(Runs::Guest)), Ok(Runs::Guest)));
        // A granted host tool runs in apid.
        assert!(matches!(decide(&granted, "calc", Some(Runs::Host)), Ok(Runs::Host)));
        // A granted name that no runner knows is denied as unknown.
        assert!(matches!(decide(&granted, "read", None), Err("unknown tool")));
    }

    #[test]
    fn request_summary_counts_and_clips_the_last_user_turn() {
        let long = "x".repeat(5_000);
        let messages = vec![
            Message::user_text("first"),
            Message::user_text(long),
            Message::assistant(vec![]),
        ];
        let summary = request_summary("m", &messages, &["read", "calc"]);
        assert_eq!(summary["messages"], 3);
        assert_eq!(summary["tools"], json!(["read", "calc"]));
        assert_eq!(summary["last_user"].as_str().map(|s| s.chars().count()), Some(LAST_USER_CHARS));
    }

    #[test]
    fn request_summary_is_empty_without_a_user_turn() {
        let summary = request_summary("m", &[Message::assistant(vec![])], &[]);
        assert_eq!(summary["last_user"], "");
        assert_eq!(summary["messages"], 1);
    }
}
