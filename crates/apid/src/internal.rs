//! The hub's internal endpoints: hello, every v3 guest line, gone.
//!
//! OWNER: slice "loop". `handle` is the agent loop's host half.

use apz_http::{Request, Response, Router};
use apz_proto::chat::{ChatRequest, Message, Role};
use apz_proto::wire::{cap_output, Decision, GuestMessage, HostMessage};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agents::{model_of, profile, Agent};
use crate::laws::Laws;
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

/// One egress decision and its outcome, reported by the hub's egress proxy.
#[derive(Deserialize)]
struct Egress {
    host: String,
    port: u16,
    allowed: bool,
    bytes_up: u64,
    bytes_down: u64,
    ms: u64,
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
        .post("/internal/v1/agents/{cid}/egress", |app, req, p| {
            authorized(app, req, p.get("cid"), |cid| {
                let egress: Egress = req.json().map_err(|e| Response::error(400, &e.to_string()))?;
                app.transcripts.record(
                    &app.bus,
                    cid,
                    0,
                    "egress",
                    json!({
                        "host": egress.host,
                        "port": egress.port,
                        "allowed": egress.allowed,
                        "bytes_up": egress.bytes_up,
                        "bytes_down": egress.bytes_down,
                        "ms": egress.ms,
                    }),
                );
                Ok(Response::json(200, &json!({})))
            })
        })
        // An agent's decision (from its VM: `POST http://plaza.gateway/decisions`, which vsockd
        // forwards here with the agent's CID).
        .post("/internal/v1/agents/{cid}/gateway/decisions", |app, req, p| {
            authorized(app, req, p.get("cid"), |cid| Ok(crate::decisions::agent_decision(app, cid, req)))
        })
}

/// Checks who is calling and the CID, then runs `f`. The local gateway presents the internal
/// token; a joined node's apz-link signs the request with its node key, and may speak only for
/// the CIDs in that node's range.
pub(crate) fn authorized(
    app: &App,
    req: &Request,
    cid: &str,
    f: impl FnOnce(u64) -> Result<Response, Response>,
) -> Response {
    let Ok(cid) = cid.parse::<u64>() else {
        return Response::error(400, "the CID must be a number");
    };
    if req.header("x-apz-node").is_some() {
        let node = match app.nodes.authenticate(req, apz_plaza::now_ms()) {
            Ok(node) => node,
            Err(_) => return Response::error(403, "bad node signature"),
        };
        if cid < node.cid_base || cid >= node.cid_base + node.cid_count {
            return Response::error(403, "that agent is not on this node");
        }
    } else if !req.header("x-apz-internal").is_some_and(|token| apz_trust::ct_eq(token.as_bytes(), app.token.as_bytes())) {
        return Response::error(403, "missing or wrong internal token");
    }
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
                program: None,
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
            // One credit per tool report; task work is cancelled if its escrow runs out.
            let _ = meter(app, cid, work, 1, "tool report");
            if crate::library::take_package_call(cid, &call) && ok {
                if let Err(err) = crate::library::accept(app, cid, &output) {
                    app.transcripts.record(&app.bus, cid, work, "error", json!({ "message": format!("tool_package: {err}") }));
                }
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
            // The agent's journal: one line per finished piece of work (episodic memory, AP-062).
            if let Some(item) = app.work.current(cid).filter(|item| item.id == work) {
                let clip = |text: &str, n: usize| -> String {
                    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    if one.chars().count() > n { format!("{}…", one.chars().take(n).collect::<String>()) } else { one }
                };
                let line = format!(
                    "- {:?} {:?}: {} → {}",
                    item.kind, status, clip(&item.prompt, 100), clip(&summary, 160)
                ).to_lowercase();
                let _ = app.memory.journal(cid, &line, apz_plaza::now_ms());
            }
            app.work.complete(app, cid, work, status, &summary);
            crate::harness_control::work_ended(app, cid, work, &summary);
            None
        }
        GuestMessage::Compressed { work, before, after, folded, reason } => {
            app.transcripts.record(
                &app.bus,
                cid,
                work,
                "compressed",
                json!({ "before": before, "after": after, "folded": folded, "reason": reason }),
            );
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
    // Held by STOP ALL: no agent thinks until the owner resumes, not even mid-turn.
    if app.control.paused() {
        let e = "the plaza is held (STOP ALL); waiting for the owner to resume".to_string();
        app.transcripts.record(&app.bus, cid, work, "error", json!({ "message": e }));
        return HostMessage::ModelResult { req, ok: false, response: None, error: Some(e) };
    }
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
            let usage = &resp.usage;
            let task = app.work.current(cid).and_then(|item| item.origin.task);
            let tokens = crate::prices::Tokens {
                input: usage.input_tokens,
                output: usage.output_tokens,
                cache_read: usage.cache_read_tokens,
                cache_write: usage.cache_write_tokens,
            };
            crate::usage::meter(app, cid, &model, tokens, None, task, "harness");
            let tokens = settings
                .prices
                .get(&model)
                .map(|p| {
                    (resp.usage.input_tokens as f64 * p.input + resp.usage.output_tokens as f64 * p.output)
                        / 1_000_000.0
                })
                .unwrap_or(0.0);
            // One credit per model call on top of the tokens, rounded up to whole credits.
            let credits = (tokens + 1.0).ceil() as i64;
            app.agents.update(cid, |a| a.usage.add(&resp.usage));
            if let Err(message) = meter(app, cid, work, credits, &format!("model {model}")) {
                return HostMessage::ModelResult { req, ok: false, response: None, error: Some(message) };
            }
            HostMessage::ModelResult { req, ok: true, response: Some(resp), error: None }
        }
        Err(e) => {
            let message = e.to_string();
            app.transcripts.record(&app.bus, cid, work, "error", json!({ "message": message }));
            HostMessage::ModelResult { req, ok: false, response: None, error: Some(message) }
        }
    }
}

/// Charges `credits` of metered spend to the work's task escrow. Only hired work
/// (a task) is charged: agents talking, reaching out, naming themselves or
/// debating motions cost the plaza's budget nothing (their tokens are still
/// counted in their usage). An exhausted escrow is recorded and the task's work
/// is cancelled.
pub(crate) fn meter(app: &App, cid: u64, work: u64, credits: i64, memo: &str) -> Result<(), String> {
    let Some(task) = app.work.current(cid).and_then(|item| item.origin.task) else {
        return Ok(());
    };
    let task = Some(task);
    match app.plaza.treasury.charge_spend(task.as_deref(), credits, memo) {
        Ok(_) => {
            if let Some(task) = task.as_deref() {
                app.work.add_spend(task, credits);
            }
            Ok(())
        }
        Err(_) => {
            let message = "budget exhausted";
            app.transcripts.record(&app.bus, cid, work, "error", json!({ "message": message }));
            if task.is_some() {
                app.work.cancel(app, cid, message);
            }
            Err(message.to_string())
        }
    }
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
    // The plaza's laws and the grant come first (see `gate_call`).
    let verdict = gate_call(&app.laws, cid, &agent.role, &granted, name, input);
    // The owner's permission mode next — only for calls that would otherwise run, so a refused
    // or ungranted call never waits in the approvals queue.
    let gate = if verdict.is_ok() { crate::approvals::gate(app, cid, name, input) } else { None };
    let mut program = None;
    let (decision, output, is_error) = match verdict {
        _ if gate.is_some() => (Decision::Deny, gate.unwrap_or_default(), true),
        Err(reason) => (Decision::Deny, reason, true),
        // An agent-made tool from the library: allowed with its program, which the guest runs.
        Ok(Runs::Guest) if name.starts_with(crate::library::PREFIX) => match crate::library::program(app, cid, name) {
            Some(found) => {
                program = Some(found);
                (Decision::Allow, String::new(), false)
            }
            None => (Decision::Deny, "no such library tool".to_string(), true),
        },
        Ok(Runs::Guest) => {
            if name == "tool_package" {
                crate::library::remember_package_call(cid, &call);
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
    HostMessage::ToolDecision { call, decision, output, is_error, program }
}

/// The law gate and the grant for one tool request, decided before anything runs: `Ok(where)` when
/// the call may run; `Err(reason)` when a law refuses it (a forbid is also recorded on the agent)
/// or no law permits it, or the agent's grant does not cover it.
pub(crate) fn gate_call(
    laws: &Laws,
    cid: u64,
    role: &str,
    granted: &[String],
    name: &str,
    input: &Value,
) -> Result<Runs, String> {
    if let Some(refused) = laws.refusal(cid, role, name, input) {
        return Err(refused);
    }
    decide(granted, name, runs(name)).map_err(str::to_string)
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

/// The law gate on the tool-request path: `gate_call`, which `tool_request` runs before anything
/// else, with the real laws and the real offer list. The App adds only the transcript, the owner's
/// approvals and the host run after this gate, so no App (and no network) is needed here.
#[cfg(test)]
mod law_gate_tests {
    use super::*;
    use crate::laws::{Laws, LAW_FILES};

    /// A rule an owner might add: no reading files, for any agent.
    const OWNER_RULE: &str = r#"law "O-001" "No reading files" {
  tier statute
  enforcement preventive
  severity info
  statement "Test rule: no agent may read a file."
  forbid tool.call
  principal is Agent
  resource is Tool
  when resource.name == "read_file"
}
"#;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("apid-gate-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::fs::remove_file(dir.join("violations.json"));
        dir
    }

    fn compiled_in() -> Vec<(String, String)> {
        LAW_FILES.iter().map(|(name, text)| (name.to_string(), text.to_string())).collect()
    }

    #[test]
    fn a_forbidden_call_is_refused_with_its_law_and_recorded() {
        let laws = Laws::load(&scratch("forbidden"));
        let granted = crate::tools::offered_names("builder");
        let input = json!({"command": "curl -H 'Authorization: sk-proj-abcdefghijklmnopqrstuvwxyz0123' x"});
        let reason = gate_call(&laws, 7, "builder", &granted, "bash", &input).unwrap_err();
        assert!(reason.starts_with("refused by law ") && reason.contains("C-002b"), "{reason}");
        assert!(laws.all_violations().iter().any(|(cid, v)| *cid == 7 && v.law == "C-002b"));
        assert!(!laws.recent(7, apz_plaza::now_ms()).is_empty());
    }

    #[test]
    fn a_permitted_call_runs_where_its_tool_runs() {
        let laws = Laws::load(&scratch("permitted"));
        let granted = crate::tools::offered_names("builder");
        assert_eq!(gate_call(&laws, 7, "builder", &granted, "read_file", &json!({"path": "a.md"})), Ok(Runs::Guest));
        assert_eq!(gate_call(&laws, 7, "builder", &granted, "plaza_tasks", &json!({})), Ok(Runs::Host));
        assert!(laws.all_violations().is_empty());
    }

    #[test]
    fn an_unknown_tool_is_denied_by_default_and_not_recorded() {
        let laws = Laws::load(&scratch("unknown"));
        let granted = vec!["launch_missiles".to_string()];
        let reason = gate_call(&laws, 7, "builder", &granted, "launch_missiles", &json!({})).unwrap_err();
        assert_eq!(reason, "refused: no law permits launch_missiles for the builder role");
        assert!(laws.all_violations().is_empty());
    }

    #[test]
    fn a_tool_the_laws_withhold_is_neither_offered_nor_run_even_if_granted() {
        let laws = Laws::load(&scratch("withheld"));
        // Posting a task is the orchestrator's (T-003): a builder is not offered it.
        assert!(!crate::tools::offered_names("builder").iter().any(|name| name == "plaza_post_task"));
        // The model may still call it by name; the gate refuses it by default and does not record it.
        let granted = vec!["plaza_post_task".to_string()];
        let reason = gate_call(&laws, 7, "builder", &granted, "plaza_post_task", &json!({})).unwrap_err();
        assert!(reason.contains("no law permits plaza_post_task"), "{reason}");
        assert!(laws.all_violations().is_empty());
    }

    #[test]
    fn a_rule_added_takes_effect_and_a_rule_removed_stops_applying() {
        let granted = crate::tools::offered_names("builder");
        let input = json!({"path": "notes.md"});
        let mut with_rule = compiled_in();
        with_rule.push(("owner.jhrl".to_string(), OWNER_RULE.to_string()));
        let added = Laws::from_files(&scratch("rule-added"), with_rule);
        let reason = gate_call(&added, 7, "builder", &granted, "read_file", &input).unwrap_err();
        assert!(reason.contains("O-001"), "{reason}");
        let removed = Laws::from_files(&scratch("rule-removed"), compiled_in());
        assert_eq!(gate_call(&removed, 7, "builder", &granted, "read_file", &input), Ok(Runs::Guest));
    }
}
