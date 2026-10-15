//! The hub's internal endpoints: hello, every v3 guest line, gone.
//!
//! OWNER: slice "loop". `handle` is the agent loop's host half.

use apz_gateway::Caller;
use apz_http::{Request, Response, Router};
use apz_proto::chat::{ChatRequest, Message, Role};
use apz_proto::wire::{CAP_ACP, Decision, GuestMessage, HostMessage, cap_output};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::agents::{Agent, model_of, profile};
use crate::approvals;
use crate::laws::Laws;
use crate::settings::tier_of;
use crate::state::App;
use crate::tools::{Runs, run_host, runs};

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
    /// The harness's features (`acp`); absent from a harness from before v4.
    #[serde(default)]
    caps: Vec<String>,
}

#[derive(Deserialize)]
struct Gone {
    #[serde(default)]
    reason: String,
}

/// The egress proxy's question before it dials: may agent `cid` connect to `host:port`, and should the
/// proxy intercept that connection for an egress credential rule?
#[derive(Deserialize)]
struct EgressCheck {
    cid: u64,
    host: String,
    #[serde(default = "crate::egress_inject::default_port")]
    port: u16,
}

/// The egress proxy asks for the leaf certificate of an intercepted host.
#[derive(Deserialize)]
struct EgressCert {
    cid: u64,
    host: String,
}

/// The egress proxy asks which header to add to one request on an intercepted host.
#[derive(Deserialize)]
struct EgressInject {
    cid: u64,
    #[serde(flatten)]
    ask: crate::egress_inject::Ask,
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
                let hello: Hello = req
                    .json()
                    .map_err(|e| Response::error(400, &e.to_string()))?;
                let profile = crate::agents::hello(app, cid, &hello.role, hello.vm, &hello.caps);
                Ok(Response::json(200, &json!({ "profile": profile })))
            })
        })
        .post("/internal/v1/agents/{cid}/message", |app, req, p| {
            authorized(app, req, p.get("cid"), |cid| {
                let message: GuestMessage = req
                    .json()
                    .map_err(|e| Response::error(400, &e.to_string()))?;
                let reply = handle(app, cid, message);
                Ok(Response::json(200, &json!({ "reply": reply })))
            })
        })
        .post("/internal/v1/agents/{cid}/gone", |app, req, p| {
            authorized(app, req, p.get("cid"), |cid| {
                let gone: Gone = req.json().unwrap_or(Gone {
                    reason: String::new(),
                });
                crate::agents::gone(app, cid, &gone.reason);
                Ok(Response::json(200, &json!({})))
            })
        })
        .post("/internal/v1/agents/{cid}/egress", |app, req, p| {
            authorized(app, req, p.get("cid"), |cid| {
                let egress: Egress = req
                    .json()
                    .map_err(|e| Response::error(400, &e.to_string()))?;
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
        // The egress proxy asks before it dials a host for an agent: the fetch blocklist and the laws.
        .post("/internal/egress/check", |app, req, _p| {
            let check: EgressCheck = match req.json() {
                Ok(check) => check,
                Err(e) => return Response::error(400, &e.to_string()),
            };
            authorized(app, req, &check.cid.to_string(), |cid| {
                Ok(Response::json(
                    200,
                    &egress_check(app, cid, &check.host, check.port),
                ))
            })
        })
        // The egress proxy's leaf certificate for an intercepted host (the hub signs it with the egress CA).
        .post("/internal/egress/cert", |app, req, _p| {
            let cert: EgressCert = match req.json() {
                Ok(cert) => cert,
                Err(e) => return Response::error(400, &e.to_string()),
            };
            authorized(app, req, &cert.cid.to_string(), |cid| {
                Ok(crate::egress_inject::cert_reply(app, cid, &cert.host))
            })
        })
        // The egress proxy's header for one request on an intercepted host: the secret is read here, never sent to the guest.
        .post("/internal/egress/inject", |app, req, _p| {
            let inject: EgressInject = match req.json() {
                Ok(inject) => inject,
                Err(e) => return Response::error(400, &e.to_string()),
            };
            authorized(app, req, &inject.cid.to_string(), |cid| {
                Ok(crate::egress_inject::inject_reply(app, cid, &inject.ask))
            })
        })
        // An agent's decision (from its VM: `POST http://plaza.gateway/decisions`, which vsockd
        // forwards here with the agent's CID).
        .post(
            "/internal/v1/agents/{cid}/gateway/decisions",
            |app, req, p| {
                authorized(app, req, p.get("cid"), |cid| {
                    Ok(crate::decisions::agent_decision(app, cid, req))
                })
            },
        )
        // vsockd's gateway asks for a sandbox's CID and envd access token by its id (`sb…`).
        .get("/internal/e2b/sandboxes/{id}", |app, req, p| {
            let presented = req
                .header("x-apz-internal")
                .is_some_and(|token| apz_trust::ct_eq(token.as_bytes(), app.token.as_bytes()));
            if !presented {
                return Response::error(403, "missing or wrong internal token");
            }
            match crate::e2b::lookup(app, p.get("id")) {
                Some((cid, access_token)) => {
                    Response::json(200, &json!({"cid": cid, "access_token": access_token}))
                }
                None => Response::error(404, "no such sandbox"),
            }
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
    } else if !req
        .header("x-apz-internal")
        .is_some_and(|token| apz_trust::ct_eq(token.as_bytes(), app.token.as_bytes()))
    {
        return Response::error(403, "missing or wrong internal token");
    }
    // A sandbox's CID has no agent session: the agent routes refuse it (its egress routes still answer).
    if req.path.starts_with("/internal/v1/") && crate::e2b::sandbox_by_cid(app, cid).is_some() {
        return Response::error(403, "a sandbox has no agent session");
    }
    f(cid).unwrap_or_else(|response| response)
}

/// Whether `host` is on the fetch blocklist: an entry `example.com` covers the host and its
/// subdomains, `*.example.com` only the subdomains. Case-insensitive; a trailing dot is ignored.
pub(crate) fn on_fetch_blocklist(block: &[String], host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let under = |suffix: &str| {
        host.strip_suffix(format!(".{suffix}").as_str())
            .is_some_and(|label| !label.is_empty())
    };
    !host.is_empty()
        && block.iter().any(|entry| {
            let entry = entry.trim().trim_end_matches('.').to_ascii_lowercase();
            match entry.strip_prefix("*.") {
                Some(suffix) => !suffix.is_empty() && under(suffix),
                None => !entry.is_empty() && (host == entry || under(&entry)),
            }
        })
}

/// The law ids in a refusal `Laws::refuse_op` made: `refused by law S-013d, C-002: titles`.
pub(crate) fn law_of(refusal: &str) -> Option<String> {
    let ids = refusal
        .strip_prefix("refused by law ")?
        .split(':')
        .next()?
        .trim();
    (!ids.is_empty()).then(|| ids.to_string())
}

/// What the egress proxy is told about one connection.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct EgressVerdict {
    pub allow: bool,
    pub law: Option<String>,
    pub reason: Option<String>,
}

/// The decision for a connection to `host`: a law refusal first (it names the law, and the laws
/// have recorded it), then the blocklist; anything else is allowed.
pub(crate) fn egress_verdict(
    blocked: bool,
    law_refusal: Option<String>,
    host: &str,
) -> EgressVerdict {
    if let Some(refusal) = law_refusal {
        return EgressVerdict {
            allow: false,
            law: law_of(&refusal),
            reason: Some(refusal),
        };
    }
    if blocked {
        return EgressVerdict {
            allow: false,
            law: None,
            reason: Some(format!("refused: {host} is on the fetch blocklist")),
        };
    }
    EgressVerdict {
        allow: true,
        law: None,
        reason: None,
    }
}

/// The answer to the egress proxy for agent `cid` connecting to `host`. The laws are asked for
/// every host, so a forbidden attempt is recorded as a violation even when the blocklist would
/// refuse it too. The laws only forbid here: a permit is not required (see the report on
/// `Laws::refuse_op`).
fn egress_check(app: &App, cid: u64, host: &str, port: u16) -> Value {
    // A sandbox's CID is a party of its own: the laws see the role `sandbox`, and it never intercepts.
    let sandbox = crate::e2b::sandbox_by_cid(app, cid);
    let role = match &sandbox {
        Some(_) => "sandbox".to_string(),
        None => app
            .agents
            .get(cid)
            .map(|agent| agent.role)
            .unwrap_or_default(),
    };
    let resource = jhrl::authz::Entity {
        ty: "Host".into(),
        id: host.into(),
    };
    // The host's group is not known here (the towns' host-groups document is not code), so a law
    // that names a group sees an empty one.
    let attrs =
        std::collections::BTreeMap::from([("group".to_string(), jhrl::value::Value::str(""))]);
    let refusal = app
        .laws
        .refuse_op(cid, &role, "egress.connect", resource, attrs, &Value::Null);
    let blocked = on_fetch_blocklist(&app.settings.get().fetch_block, host);
    let verdict = match &sandbox {
        Some(sandbox) => crate::e2b::sandbox_verdict(sandbox, blocked, refusal, host),
        None => egress_verdict(blocked, refusal, host),
    };
    // Whether the proxy terminates this connection's TLS to add a credential (see egress_inject).
    let intercept = sandbox.is_none() && crate::egress_inject::intercepts(app, cid, host, port);
    // The denyOut blocks vsockd checks the addresses a name resolves to against (see e2b_net).
    let deny_cidrs = match &sandbox {
        Some(sandbox) if verdict.allow => {
            crate::e2b_net::deny_blocks(&sandbox.allow_out, &sandbox.deny_out, host)
        }
        _ => Vec::new(),
    };
    let mut reply = json!({
        "allow": verdict.allow,
        "law": verdict.law,
        "reason": verdict.reason,
        "intercept": intercept,
    });
    if !deny_cidrs.is_empty() {
        reply["deny_cidrs"] = json!(deny_cidrs);
    }
    reply
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
                approval: None,
            }),
            _ => None,
        };
    };
    match message {
        GuestMessage::ModelCall { req, work, request } => {
            Some(model_call(app, cid, &agent, req, work, request))
        }
        GuestMessage::ToolRequest {
            work,
            call,
            name,
            input,
        } => Some(tool_request(app, cid, &agent, work, call, &name, &input)),
        GuestMessage::ToolReport {
            work,
            call,
            ok,
            output,
        } => {
            // One credit per tool report; task work is cancelled if its escrow runs out.
            let _ = meter(app, cid, work, 1, "tool report");
            if crate::library::take_package_call(cid, &call) && ok {
                if let Err(err) = crate::library::accept(app, cid, &output) {
                    app.transcripts.record(
                        &app.bus,
                        cid,
                        work,
                        "error",
                        json!({ "message": format!("tool_package: {err}") }),
                    );
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
        GuestMessage::EngineUpdate {
            work,
            call,
            engine,
            session,
            seq,
            update,
        } => {
            // Recorded as the feed decides (the text waits for the message's end); published live.
            let entries = app.engine_feed.absorb(cid, work, &call, &update);
            app.bus.publish(
                "engine",
                &json!({
                    "cid": cid,
                    "work": work,
                    "call": call,
                    "engine": engine,
                    "session": session,
                    "seq": seq,
                    "update": update,
                }),
            );
            for entry in entries {
                app.transcripts
                    .record(&app.bus, cid, work, "engine_update", entry);
            }
            None
        }
        GuestMessage::WorkDone {
            work,
            status,
            summary,
        } => {
            // The agent's journal: one line per finished piece of work (episodic memory, AP-062).
            if let Some(item) = app.work.current(cid).filter(|item| item.id == work) {
                let clip = |text: &str, n: usize| -> String {
                    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    if one.chars().count() > n {
                        format!("{}…", one.chars().take(n).collect::<String>())
                    } else {
                        one
                    }
                };
                let line = format!(
                    "- {:?} {:?}: {} → {}",
                    item.kind,
                    status,
                    clip(&item.prompt, 100),
                    clip(&summary, 160)
                )
                .to_lowercase();
                let _ = app.memory.journal(cid, &line, apz_plaza::now_ms());
            }
            // The engine calls under this work item end with it: their buffered text, plan and usage.
            for entry in app.engine_feed.finish(cid, work) {
                app.transcripts
                    .record(&app.bus, cid, work, "engine_update", entry);
            }
            app.work.complete(app, cid, work, status, &summary);
            crate::harness_control::work_ended(app, cid, work, &summary);
            None
        }
        GuestMessage::Compressed {
            work,
            before,
            after,
            folded,
            reason,
        } => {
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
        app.transcripts
            .record(&app.bus, cid, work, "error", json!({ "message": e }));
        return HostMessage::ModelResult {
            req,
            ok: false,
            response: None,
            error: Some(e),
        };
    }
    let model = model_of(app, agent);
    let settings = app.settings.get();
    if let Err(e) = app.settings.allowed(tier_of(&agent.role), &model) {
        app.transcripts
            .record(&app.bus, cid, work, "error", json!({ "message": e }));
        return HostMessage::ModelResult {
            req,
            ok: false,
            response: None,
            error: Some(e),
        };
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
    let caller = Caller::Agent {
        cid,
        node: crate::vault::node_for_cid(&app.nodes.list(), cid),
    };
    let result = gateway.complete_for(
        &request,
        &mut |delta| {
            app.bus.publish(
                "delta",
                &json!({ "cid": cid, "work": work, "req": req, "delta": delta }),
            );
        },
        &caller,
    );
    match result {
        Ok(resp) => {
            let full = serde_json::to_value(&resp).unwrap_or(Value::Null);
            app.transcripts
                .record(&app.bus, cid, work, "model_response", full);
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
                    (resp.usage.input_tokens as f64 * p.input
                        + resp.usage.output_tokens as f64 * p.output)
                        / 1_000_000.0
                })
                .unwrap_or(0.0);
            // One credit per model call on top of the tokens, rounded up to whole credits.
            let credits = (tokens + 1.0).ceil() as i64;
            app.agents.update(cid, |a| a.usage.add(&resp.usage));
            if let Err(message) = meter(app, cid, work, credits, &format!("model {model}")) {
                return HostMessage::ModelResult {
                    req,
                    ok: false,
                    response: None,
                    error: Some(message),
                };
            }
            HostMessage::ModelResult {
                req,
                ok: true,
                response: Some(resp),
                error: None,
            }
        }
        Err(e) => {
            let message = e.to_string();
            app.transcripts
                .record(&app.bus, cid, work, "error", json!({ "message": message }));
            HostMessage::ModelResult {
                req,
                ok: false,
                response: None,
                error: Some(message),
            }
        }
    }
}

/// Charges `credits` of a model call to the work's task, through the shared charge
/// ([`crate::payments::charge_model_call`]), which honours `payments.charge_models`.
/// Only hired work (a task) is charged: agents talking, reaching out, naming
/// themselves or debating motions cost the plaza's budget nothing (their tokens are
/// still counted in their usage). A shortfall is recorded and the task's work is
/// cancelled.
pub(crate) fn meter(
    app: &App,
    cid: u64,
    work: u64,
    credits: i64,
    memo: &str,
) -> Result<(), String> {
    let Some(task) = app.work.current(cid).and_then(|item| item.origin.task) else {
        return Ok(());
    };
    let charging = app.settings.get().payments.charge_models;
    let spend = |task: &str, credits: i64| app.work.add_spend(task, credits);
    let charge = crate::payments::charge_model_call(
        &app.plaza.treasury,
        charging,
        spend,
        cid,
        Some(&task),
        credits,
        memo,
    );
    match charge {
        Some(settled) if settled.charged < settled.wanted => {
            let message = "budget exhausted";
            app.transcripts
                .record(&app.bus, cid, work, "error", json!({ "message": message }));
            app.work.cancel(app, cid, message);
            Err(message.to_string())
        }
        _ => Ok(()),
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
    let granted: Vec<String> = profile(app, agent)
        .tools
        .into_iter()
        .map(|t| t.name)
        .collect();
    // The plaza's laws and the grant come first (see `gate_call`).
    let verdict = gate_call(&app.laws, cid, &agent.role, &granted, name, input)
        .and_then(|runs| crate::sanctions::suspended(app, cid, name).map_or(Ok(runs), Err));
    // The owner's permission mode next — only for calls that would otherwise run, so a refused
    // or ungranted call never waits in the approvals queue. An ACP engine's own request may wait
    // open for the owner (see `holds_for_owner`); the held call keeps its work's thread.
    let holds = holds_for_owner(&agent.caps, name);
    let held = approvals::Held {
        thread: app
            .work
            .current(cid)
            .filter(|item| item.id == work)
            .map(|item| item.thread),
        waiting: holds.then(|| approvals::Waiting {
            work,
            call: call.clone(),
        }),
    };
    let gate = if verdict.is_ok() {
        crate::approvals::gate(app, cid, name, input, &held)
    } else {
        approvals::Gate::Run
    };
    // An allowed call that may not be undone starts only after its VM is checkpointed (`vm_snapshots`).
    if verdict.is_ok() && matches!(gate, approvals::Gate::Run) {
        crate::vm_snapshots::auto_checkpoint(app, cid, name);
    }
    let mut program = None;
    let mut approval = None;
    let (decision, output, is_error) = match (gate, verdict) {
        (approvals::Gate::Refused(reason), _) => (Decision::Deny, reason, true),
        (approvals::Gate::Held { approval: id, .. }, _) if holds => {
            let output = format!("waiting for the owner's approval ({id})");
            approval = Some(id);
            (Decision::Pending, output, false)
        }
        (approvals::Gate::Held { refusal, .. }, _) => (Decision::Deny, refusal, true),
        (approvals::Gate::Run, Err(reason)) => (Decision::Deny, reason, true),
        // An agent-made tool from the library: allowed with its program, which the guest runs.
        (approvals::Gate::Run, Ok(Runs::Guest)) if name.starts_with(crate::library::PREFIX) => {
            match crate::library::program(app, cid, name) {
                Some(found) => {
                    program = Some(found);
                    (Decision::Allow, String::new(), false)
                }
                None => (Decision::Deny, "no such library tool".to_string(), true),
            }
        }
        (approvals::Gate::Run, Ok(Runs::Guest)) => {
            if name == "tool_package" {
                crate::library::remember_package_call(cid, &call);
            }
            // The engine's model is checked against the tier before the VM runs it: a bad one is a
            // tool error here, not a gateway refusal later.
            match name {
                "code_engine" => match crate::tools::code_engine_model(app, &agent.role, input)
                    .and_then(|_| crate::tools::code_engine_enabled(app, input))
                {
                    Ok(_) => (Decision::Allow, String::new(), false),
                    Err(reason) => (Decision::Deny, reason, true),
                },
                _ => (Decision::Allow, String::new(), false),
            }
        }
        (approvals::Gate::Run, Ok(Runs::Host)) => {
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
    HostMessage::ToolDecision {
        call,
        decision,
        output,
        is_error,
        program,
        approval,
    }
}

/// Whether a held call may wait open for the owner's decision: an ACP engine's own request
/// (`engine.*`) from a harness that advertised `acp`. Any other held call is refused, as it always
/// was, and its decision reaches the agent as a chat follow-up.
pub(crate) fn holds_for_owner(caps: &[String], name: &str) -> bool {
    name.starts_with("engine.") && caps.iter().any(|cap| cap == CAP_ACP)
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
    decide(granted, name, engine_runs(name).or_else(|| runs(name))).map_err(str::to_string)
}

/// The requests an ACP engine makes of its own (`engine.*`), sent as nested tool requests while
/// `code_engine` runs. They act in the agent's VM, so they run in the guest like `code_engine` does.
const ENGINE_TOOLS: &[&str] = &[
    "engine.read",
    "engine.edit",
    "engine.exec",
    "engine.fetch",
    "engine.other",
];

/// Where an engine request runs: the guest, for the five names the engine may make; `None` for any
/// other name under `engine.`.
fn engine_runs(name: &str) -> Option<Runs> {
    ENGINE_TOOLS.contains(&name).then_some(Runs::Guest)
}

/// The pure tool policy: `Ok(where)` when the tool may run, `Err(reason)` when it may not. An engine
/// request is granted with `code_engine` (it is the engine's own action inside that run), so an
/// unknown `engine.*` name is granted and then refused as unknown.
fn decide(granted: &[String], name: &str, runs: Option<Runs>) -> Result<Runs, &'static str> {
    let engine = name.starts_with("engine.") && granted.iter().any(|g| g == "code_engine");
    if !engine && !granted.iter().any(|g| g == name) {
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
    fn decide_policy_grants_only_granted_tools_and_runs_each_where_it_belongs() {
        {
            // decide_denies_ungranted_allows_guest_and_denies_unknown: an ungranted tool is denied,
            // a granted guest tool runs in the VM, a granted host tool runs in apid, and a granted
            // name no runner knows is denied as unknown.
            let granted = vec!["read".to_string(), "calc".to_string()];
            // An ungranted tool is denied even when a runner exists for it.
            assert!(matches!(
                decide(&granted, "bash", Some(Runs::Guest)),
                Err("not granted to this agent")
            ));
            // A granted guest tool may run in the VM.
            assert!(matches!(
                decide(&granted, "read", Some(Runs::Guest)),
                Ok(Runs::Guest)
            ));
            // A granted host tool runs in apid.
            assert!(matches!(
                decide(&granted, "calc", Some(Runs::Host)),
                Ok(Runs::Host)
            ));
            // A granted name that no runner knows is denied as unknown.
            assert!(matches!(
                decide(&granted, "read", None),
                Err("unknown tool")
            ));
        }
        {
            // an_engine_request_is_granted_with_code_engine_and_runs_in_the_guest: an engine.* name
            // is granted only with code_engine and then runs in the guest; any other engine.* name
            // is granted with code_engine and is unknown.
            let with_engine = vec!["read".to_string(), "code_engine".to_string()];
            let without = vec!["read".to_string()];
            for name in [
                "engine.read",
                "engine.edit",
                "engine.exec",
                "engine.fetch",
                "engine.other",
            ] {
                assert!(
                    matches!(
                        decide(&with_engine, name, engine_runs(name).or_else(|| runs(name))),
                        Ok(Runs::Guest)
                    ),
                    "{name} with code_engine"
                );
                assert!(
                    matches!(
                        decide(&without, name, engine_runs(name).or_else(|| runs(name))),
                        Err("not granted to this agent")
                    ),
                    "{name} without code_engine"
                );
            }
            // Anything else under `engine.` is granted with code_engine and then unknown.
            assert!(matches!(
                decide(&with_engine, "engine.bogus", engine_runs("engine.bogus")),
                Err("unknown tool")
            ));
            assert_eq!(engine_runs("engine.bogus"), None);
        }
    }

    #[test]
    fn request_summary_counts_clips_and_handles_a_missing_user_turn() {
        {
            // request_summary_counts_and_clips_the_last_user_turn: the counts and tool names are
            // kept, and the last user turn is clipped to LAST_USER_CHARS.
            let long = "x".repeat(5_000);
            let messages = vec![
                Message::user_text("first"),
                Message::user_text(long),
                Message::assistant(vec![]),
            ];
            let summary = request_summary("m", &messages, &["read", "calc"]);
            assert_eq!(summary["messages"], 3);
            assert_eq!(summary["tools"], json!(["read", "calc"]));
            assert_eq!(
                summary["last_user"].as_str().map(|s| s.chars().count()),
                Some(LAST_USER_CHARS)
            );
        }
        {
            // request_summary_is_empty_without_a_user_turn: with no user turn the last user text is
            // empty, and the message count is still kept.
            let summary = request_summary("m", &[Message::assistant(vec![])], &[]);
            assert_eq!(summary["last_user"], "");
            assert_eq!(summary["messages"], 1);
        }
    }
}

/// The egress check's decision: the blocklist's matching, the verdict, and the real laws' refusal.
#[cfg(test)]
mod egress_check_tests {
    use super::*;
    use crate::laws::{LAW_FILES, Laws};

    fn list(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|entry| entry.to_string()).collect()
    }

    #[test]
    fn the_fetch_blocklist_matches_domains_subdomains_and_wildcards() {
        {
            // the_blocklist_covers_a_domain_with_its_subdomains_and_a_wildcard_only_the_subdomains:
            // a domain blocks itself and its subdomains, and a wildcard blocks only the subdomains.
            let block = list(&["example.com", "*.pypi.org", "  Files.Example.NET "]);
            for host in [
                "example.com",
                "EXAMPLE.com",
                "sub.example.com",
                "example.com.",
                "files.example.net",
                "a.pypi.org",
                "x.y.pypi.org",
            ] {
                assert!(on_fetch_blocklist(&block, host), "{host} must be blocked");
            }
            for host in [
                "pypi.org",
                "evilpypi.org",
                "a.evilpypi.org",
                "example.com.evil.net",
                "notexample.com",
                "pypi.org.evil.com",
                "example.net",
                "",
            ] {
                assert!(
                    !on_fetch_blocklist(&block, host),
                    "{host} must not be blocked"
                );
            }
        }
        {
            // an_empty_blocklist_blocks_nothing: an empty list, or one holding only an empty entry,
            // blocks no host.
            assert!(!on_fetch_blocklist(&[], "example.com"));
            assert!(!on_fetch_blocklist(&list(&[""]), "example.com"));
        }
    }

    #[test]
    fn a_law_refusal_wins_and_names_its_law_and_a_blocked_host_is_named() {
        let refusal =
            "refused by law S-013d, C-002: Researchers never write to repositories".to_string();
        assert_eq!(
            egress_verdict(true, Some(refusal.clone()), "git.hub.local"),
            EgressVerdict {
                allow: false,
                law: Some("S-013d, C-002".into()),
                reason: Some(refusal)
            }
        );
        // A law refusal stands even for a host the blocklist doesn't name.
        assert!(!egress_verdict(false, Some("refused by law X: t".into()), "example.com").allow);
        assert_eq!(
            egress_verdict(false, None, "example.com"),
            EgressVerdict {
                allow: true,
                law: None,
                reason: None
            }
        );
        assert_eq!(
            egress_verdict(true, None, "evil.example"),
            EgressVerdict {
                allow: false,
                law: None,
                reason: Some("refused: evil.example is on the fetch blocklist".into())
            }
        );
    }

    #[test]
    fn a_researcher_at_the_hub_git_is_refused_by_the_real_law_and_recorded() {
        let dir = std::env::temp_dir().join(format!("apid-egress-check-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::fs::remove_file(dir.join("violations.json"));
        let files: Vec<(String, String)> = LAW_FILES
            .iter()
            .map(|(name, text)| (name.to_string(), text.to_string()))
            .collect();
        let laws = Laws::from_files(&dir, files);
        let resource = jhrl::authz::Entity {
            ty: "Host".into(),
            id: "git.hub.local".into(),
        };
        let attrs = std::collections::BTreeMap::from([(
            "group".to_string(),
            jhrl::value::Value::str("hub-git"),
        )]);
        let refusal = laws.refuse_op(
            7,
            "researcher",
            "egress.connect",
            resource,
            attrs,
            &Value::Null,
        );
        let verdict = egress_verdict(false, refusal, "git.hub.local");
        assert!(
            !verdict.allow
                && verdict
                    .law
                    .as_deref()
                    .unwrap_or_default()
                    .contains("S-013d"),
            "{verdict:?}"
        );
        assert!(
            laws.all_violations()
                .iter()
                .any(|(cid, violation)| *cid == 7 && violation.law == "S-013d")
        );
    }
}

/// The law gate on the tool-request path: `gate_call`, which `tool_request` runs before anything
/// else, with the real laws and the real offer list. The App adds only the transcript, the owner's
/// approvals and the host run after this gate, so no App (and no network) is needed here.
#[cfg(test)]
mod law_gate_tests {
    use super::*;
    use crate::laws::{LAW_FILES, Laws};

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
        LAW_FILES
            .iter()
            .map(|(name, text)| (name.to_string(), text.to_string()))
            .collect()
    }

    #[test]
    fn gate_call_follows_the_laws_for_each_kind_of_call() {
        {
            // a_forbidden_call_is_refused_with_its_law_and_recorded: a forbidden call is refused
            // with its law, and the refusal is recorded.
            let laws = Laws::load(&scratch("forbidden"));
            let granted = crate::tools::offered_names("builder");
            let input = json!({"command": "curl -H 'Authorization: sk-proj-abcdefghijklmnopqrstuvwxyz0123' x"});
            let reason = gate_call(&laws, 7, "builder", &granted, "bash", &input).unwrap_err();
            assert!(
                reason.starts_with("refused by law ") && reason.contains("C-002b"),
                "{reason}"
            );
            assert!(
                laws.all_violations()
                    .iter()
                    .any(|(cid, v)| *cid == 7 && v.law == "C-002b")
            );
            assert!(!laws.recent(7, apz_plaza::now_ms()).is_empty());
        }
        {
            // a_permitted_call_runs_where_its_tool_runs: a permitted call runs where its tool runs
            // (guest or host) and records no violation.
            let laws = Laws::load(&scratch("permitted"));
            let granted = crate::tools::offered_names("builder");
            assert_eq!(
                gate_call(
                    &laws,
                    7,
                    "builder",
                    &granted,
                    "read_file",
                    &json!({"path": "a.md"})
                ),
                Ok(Runs::Guest)
            );
            assert_eq!(
                gate_call(&laws, 7, "builder", &granted, "plaza_tasks", &json!({})),
                Ok(Runs::Host)
            );
            assert!(laws.all_violations().is_empty());
        }
        {
            // an_unknown_tool_is_denied_by_default_and_not_recorded: an unknown tool is denied by
            // default, and the denial is not recorded.
            let laws = Laws::load(&scratch("unknown"));
            let granted = vec!["launch_missiles".to_string()];
            let reason = gate_call(&laws, 7, "builder", &granted, "launch_missiles", &json!({}))
                .unwrap_err();
            assert_eq!(
                reason,
                "refused: no law permits launch_missiles for the builder role"
            );
            assert!(laws.all_violations().is_empty());
        }
        {
            // a_tool_the_laws_withhold_is_neither_offered_nor_run_even_if_granted: a tool the laws
            // withhold is not offered to the role, and the gate refuses it even when granted.
            let laws = Laws::load(&scratch("withheld"));
            // Posting a task is the orchestrator's (T-003): a builder is not offered it.
            assert!(
                !crate::tools::offered_names("builder")
                    .iter()
                    .any(|name| name == "plaza_post_task")
            );
            // The model may still call it by name; the gate refuses it by default and does not record it.
            let granted = vec!["plaza_post_task".to_string()];
            let reason = gate_call(&laws, 7, "builder", &granted, "plaza_post_task", &json!({}))
                .unwrap_err();
            assert!(
                reason.contains("no law permits plaza_post_task"),
                "{reason}"
            );
            assert!(laws.all_violations().is_empty());
        }
        {
            // a_rule_added_takes_effect_and_a_rule_removed_stops_applying: a rule added to the laws
            // refuses the call, and the same call runs once that rule is removed.
            let granted = crate::tools::offered_names("builder");
            let input = json!({"path": "notes.md"});
            let mut with_rule = compiled_in();
            with_rule.push(("owner.jhrl".to_string(), OWNER_RULE.to_string()));
            let added = Laws::from_files(&scratch("rule-added"), with_rule);
            let reason =
                gate_call(&added, 7, "builder", &granted, "read_file", &input).unwrap_err();
            assert!(reason.contains("O-001"), "{reason}");
            let removed = Laws::from_files(&scratch("rule-removed"), compiled_in());
            assert_eq!(
                gate_call(&removed, 7, "builder", &granted, "read_file", &input),
                Ok(Runs::Guest)
            );
        }
    }
}

#[cfg(test)]
mod hold_tests {
    use super::*;

    #[test]
    fn only_an_engine_request_from_an_acp_harness_is_held_for_the_owner() {
        let acp = vec![CAP_ACP.to_string()];
        assert!(holds_for_owner(&acp, "engine.edit"));
        assert!(holds_for_owner(&acp, "engine.exec"));
        // The harness did not advertise acp (an older one): refused as before.
        assert!(!holds_for_owner(&[], "engine.edit"));
        // Not an engine call, even from an ACP harness: the plaza's own tools behave as they did.
        assert!(!holds_for_owner(&acp, "write_file"));
        assert!(!holds_for_owner(&acp, "code_engine"));
    }
}
