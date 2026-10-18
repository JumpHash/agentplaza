//! The hub's MCP server, run through its router in-process.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use apz_http::{Request, Response, Router};
use apz_plaza::ConnectedAgent;
use apz_plaza::events::EventBus;
use apz_plaza::tasks::{CreateTask, TaskLedger};
use serde_json::{Value, json};

use super::audit::MemoryAudit;
use super::auth::{MemoryTokens, NewToken, Scope, TokenRecord, sha256_hex};
use super::tools::{Ctx, Plaza, REGISTRY, Tool, observe, visible};
use super::{Host, HubMcp, routes};
use crate::prompts::store_mem::MemoryPrompts;
use crate::prompts::{NewPrompt, Prompts};
use crate::skills::Skills;
use apz_prompt::Kind;

const ORIGIN: &str = "https://hub.test";
const T0: u64 = 1_000_000;
const MINUTE: u64 = 60_000;

struct FakePlaza {
    prompts: Prompts,
    /// Only the skill tests build a library; the others never reach the skill tools.
    skills: Option<Skills>,
}

fn fake_plaza() -> FakePlaza {
    FakePlaza {
        prompts: Prompts::new(Arc::new(MemoryPrompts::in_memory())),
        skills: None,
    }
}

/// Adds a prompt with a starter body (publishable) to the fake plaza's library, unpublished.
fn seed_prompt(plaza: &FakePlaza, id: &str) {
    let new = NewPrompt {
        id: id.into(),
        kind: Kind::System,
        title: id.into(),
        description: String::new(),
        tags: Vec::new(),
        binding: Some("role:coder".into()),
    };
    plaza
        .prompts
        .create(
            &new,
            apz_prompt::template(Kind::System).to_string(),
            "seed".into(),
            "owner",
            T0,
        )
        .expect("a prompt");
}

impl Plaza for FakePlaza {
    fn prompts(&self) -> &Prompts {
        &self.prompts
    }

    fn skills(&self) -> &Skills {
        self.skills.as_ref().expect("a skill library")
    }

    fn create_task(&self, input: &Value) -> Result<Value, String> {
        if input["goal"] == "" {
            Err("goal must be a non-empty string".into())
        } else if input["project"].is_null() {
            Err("a task belongs to a project: name it with `project`".into())
        } else {
            Ok(
                json!({ "id": "t2", "goal": input["goal"], "after": input["after"], "project": input["project"] }),
            )
        }
    }

    fn search_library(&self, query: &str, _limit: usize) -> Vec<Value> {
        if query == "weather" {
            vec![json!({ "name": "weather", "version": 1, "state": "published" })]
        } else {
            Vec::new()
        }
    }

    fn restart_agent(&self, cid: u64) -> Result<Value, String> {
        if cid == 1 {
            Ok(json!({ "ok": true, "cid": cid }))
        } else {
            Err(format!("no agent {cid}"))
        }
    }

    fn record_decision(
        &self,
        approval_id: &str,
        decision: &str,
        _note: &str,
    ) -> Result<Value, String> {
        Ok(
            json!({ "recorded": true, "decided": false, "approval_id": approval_id, "decision": decision }),
        )
    }

    fn status(&self) -> Value {
        json!({
            "agents": { "total": 2, "idle": 1, "working": 1, "offline": 0 },
            "tasks": { "open": 3 },
            "default_model": "model-a",
            "orchestrator_model": "model-b",
            "noded": true,
            "hub": true,
        })
    }

    fn agents(&self) -> Vec<Value> {
        vec![
            json!({ "cid": 1, "name": "Ada", "role": "coder", "tier": "dev", "status": "idle", "node": "n0", "model": "m", "work": null }),
            json!({ "cid": 2, "name": "Bo", "role": "writer", "tier": "dev", "status": "working", "node": "n0", "model": "m", "work": null }),
        ]
    }

    fn agent(&self, cid: u64) -> Option<Value> {
        self.agents()
            .into_iter()
            .find(|agent| agent["cid"].as_u64() == Some(cid))
    }

    fn transcript(&self, cid: u64, _after: u64, _limit: usize) -> Vec<Value> {
        vec![
            json!({ "seq": 1, "at_ms": 0, "cid": cid, "work": 1, "kind": "tool_result", "data": "x".repeat(5000) }),
        ]
    }

    fn tasks(&self) -> Vec<Value> {
        // The ledger's own rows: t1 open, t2 claimed by cid 1 (a coder) and naming project pr2, t3 by
        // cid 2 (no role, so cid:2).
        let ledger = TaskLedger::new(Arc::new(EventBus::default()));
        let claimants = [
            (None, None),
            (Some((1_u64, Some("coder"))), Some("pr2")),
            (Some((2, None)), None),
        ];
        claimants
            .into_iter()
            .map(|(claimant, project)| {
                let task = ledger
                    .create(&CreateTask {
                        goal: "Ship".into(),
                        acceptance: None,
                        budget: None,
                        role: None,
                        after: Vec::new(),
                        project: project.map(String::from),
                    })
                    .expect("a task");
                if let Some((cid, role)) = claimant {
                    let agent = ConnectedAgent {
                        cid,
                        role: role.map(String::from),
                        run: "r1".into(),
                        vm: 0,
                    };
                    task.claim(&agent).expect("a claim");
                }
                serde_json::to_value(task.view()).expect("a task row")
            })
            .collect()
    }

    fn task(&self, id: &str) -> Option<Value> {
        self.tasks().into_iter().find(|task| task["id"] == id)
    }

    fn board(&self, _limit: usize) -> Value {
        json!({ "posts": [] })
    }

    fn projects(&self) -> Vec<Value> {
        // One project, in the project store's own shape, that lists t1 and t3.
        let project: crate::projects::Project = serde_json::from_value(json!({
            "id": "pr1", "title": "Site", "goal": "a site", "kind": "code", "stage": "brief",
            "lead_cid": 1, "tasks": ["t1", "t3"], "created_ms": T0, "updated_ms": T0,
        }))
        .expect("a project");
        vec![serde_json::to_value(project).expect("a project row")]
    }

    fn project(&self, id: &str) -> Option<Value> {
        self.projects()
            .into_iter()
            .find(|project| project["id"] == id)
    }

    fn project_graph(&self, id: &str) -> Option<Value> {
        // pr1's plan: t1 first, then t2 after it.
        if id != "pr1" {
            return None;
        }
        let ledger = TaskLedger::new(Arc::new(EventBus::default()));
        for after in [Vec::new(), vec!["t1".to_string()]] {
            ledger
                .create(&CreateTask {
                    goal: "Ship".into(),
                    acceptance: None,
                    budget: None,
                    role: Some("coder".into()),
                    after,
                    project: Some("pr1".into()),
                })
                .expect("a task");
        }
        let tasks = ledger.list();
        let views: Vec<_> = tasks.iter().map(|task| task.view()).collect();
        let graph = crate::task_graph::build("pr1", "implementation", &views, |_| None, |_| None);
        serde_json::to_value(graph).ok()
    }

    fn approvals(&self) -> Vec<Value> {
        vec![json!({ "id": "a1", "state": "pending", "kind": "tool" })]
    }

    fn machines(&self) -> Vec<Value> {
        vec![json!({ "name": "n0", "kind": "linux", "status": "up" })]
    }

    fn treasury(&self, _journal_limit: usize) -> Value {
        json!({ "view": {}, "journal": {}, "plaza_budget": 0 })
    }

    fn usage(&self, _from_ms: u64, _to_ms: u64) -> Value {
        json!({ "totals": { "calls": 0 } })
    }

    fn laws(&self) -> Vec<Value> {
        vec![json!({ "id": "L1", "tier": "A" })]
    }

    fn message_orchestrator(
        &self,
        text: &str,
        conversation: Option<&str>,
    ) -> Result<Value, String> {
        Ok(json!({ "work": 7, "cid": 1, "text": text, "conversation": conversation }))
    }

    fn tell_agent(&self, cid: u64, text: &str) -> Result<Value, String> {
        Ok(json!({ "cid": cid, "text": text }))
    }

    fn stop_agent(&self, cid: u64, reason: &str) -> Result<Value, String> {
        Ok(json!({ "cid": cid, "reason": reason }))
    }
}

/// A test-only tool that needs the operate scope, to check the scope gate.
struct FakeOperate;

impl Tool for FakeOperate {
    fn name(&self) -> &'static str {
        "fake_operate"
    }

    fn scope(&self) -> Scope {
        Scope::Operate
    }

    fn description(&self) -> &'static str {
        "test only"
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    fn output_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    fn call(&self, _ctx: &Ctx<'_>, _args: &Value) -> Result<Value, String> {
        Ok(json!({}))
    }

    fn summary(&self, _out: &Value) -> String {
        String::new()
    }
}

struct TestHost {
    hub: HubMcp,
    plaza: FakePlaza,
    clock: Arc<AtomicU64>,
}

impl Host for TestHost {
    fn hub_mcp(&self) -> &HubMcp {
        &self.hub
    }

    fn plaza(&self) -> &dyn Plaza {
        &self.plaza
    }
}

struct Harness {
    host: TestHost,
    router: Router<TestHost>,
}

fn harness(enabled: bool) -> Harness {
    let clock = Arc::new(AtomicU64::new(T0));
    let reading = Arc::clone(&clock);
    let hub = HubMcp::new(
        enabled,
        vec![ORIGIN.to_string()],
        Box::new(MemoryTokens::default()),
        Box::new(MemoryAudit::default()),
        Box::new(move || reading.load(Ordering::SeqCst)),
    );
    Harness {
        host: TestHost {
            hub,
            plaza: fake_plaza(),
            clock,
        },
        router: routes(Router::new()),
    }
}

fn header(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

fn bearer(token: &str) -> (String, String) {
    header("authorization", &format!("Bearer {token}"))
}

fn body_of(resp: &Response) -> Value {
    serde_json::from_slice(resp.bytes()).unwrap_or(Value::Null)
}

fn reply_header(resp: &Response, name: &str) -> Option<String> {
    resp.headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

fn request_body(id: u64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn tool_call(name: &str, arguments: Value) -> Value {
    request_body(
        7,
        "tools/call",
        json!({ "name": name, "arguments": arguments }),
    )
}

fn listed_names(h: &Harness, token: &str, session: &str) -> Vec<String> {
    let body = body_of(&h.call(
        token,
        Some(session),
        request_body(3, "tools/list", json!({})),
    ));
    body["result"]["tools"]
        .as_array()
        .expect("a tools array")
        .iter()
        .map(|tool| tool["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

impl Harness {
    fn now(&self) -> u64 {
        self.host.clock.load(Ordering::SeqCst)
    }

    fn advance(&self, ms: u64) {
        self.host.clock.fetch_add(ms, Ordering::SeqCst);
    }

    fn mint(
        &self,
        scope: Scope,
        allowlist: &[&str],
        expires_ms: Option<u64>,
    ) -> (String, TokenRecord) {
        let allowlist = allowlist.iter().map(|name| name.to_string()).collect();
        let new = NewToken {
            name: "test".into(),
            scope,
            allowlist,
            expires_ms,
        };
        self.host
            .hub
            .tokens
            .create(new, self.now())
            .expect("a token")
    }

    fn raw(&self, method: &str, headers: Vec<(String, String)>, body: Vec<u8>) -> Response {
        let req = Request {
            method: method.into(),
            path: "/mcp".into(),
            headers,
            body,
            peer: Some(std::net::Ipv4Addr::LOCALHOST.into()),
            ..Request::default()
        };
        self.router.handle(&self.host, &req)
    }

    /// A POST with a bearer token and, when given, the session header.
    fn call(&self, token: &str, session: Option<&str>, body: Value) -> Response {
        let mut headers = vec![bearer(token)];
        if let Some(session) = session {
            headers.push(header("mcp-session-id", session));
        }
        self.raw("POST", headers, serde_json::to_vec(&body).expect("json"))
    }

    /// Initializes a session for `token` and returns its ID.
    fn session(&self, token: &str) -> String {
        let resp = self.call(
            token,
            None,
            request_body(1, "initialize", json!({ "protocolVersion": "2025-06-18" })),
        );
        assert_eq!(resp.status, 200);
        reply_header(&resp, "Mcp-Session-Id").expect("a session header")
    }
}

#[test]
fn initialize_negotiates_the_version_and_issues_a_session() {
    let h = harness(true);
    let (token, _) = h.mint(Scope::Observe, &[], None);
    let init = request_body(
        1,
        "initialize",
        json!({ "protocolVersion": "2025-06-18", "capabilities": {} }),
    );
    let resp = h.call(&token, None, init);
    assert_eq!(resp.status, 200);
    let body = body_of(&resp);
    assert_eq!(body["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(
        body["result"]["capabilities"]["tools"]["listChanged"],
        false
    );
    assert!(reply_header(&resp, "Mcp-Session-Id").is_some_and(|id| !id.is_empty()));
}

#[test]
fn an_unknown_version_gets_the_newest_and_a_known_older_one_is_kept() {
    let h = harness(true);
    let (token, _) = h.mint(Scope::Observe, &[], None);
    for (asked, answered) in [("1999-01-01", "2025-06-18"), ("2025-03-26", "2025-03-26")] {
        let resp = h.call(
            &token,
            None,
            request_body(1, "initialize", json!({ "protocolVersion": asked })),
        );
        assert_eq!(body_of(&resp)["result"]["protocolVersion"], answered);
    }
}

#[test]
fn notifications_get_no_body_ping_answers_and_batches_answer_per_message() {
    let h = harness(true);
    let (token, _) = h.mint(Scope::Observe, &[], None);
    let session = h.session(&token);
    let note = h.call(
        &token,
        Some(&session),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    );
    assert_eq!((note.status, note.bytes().len()), (202, 0));
    let ping = body_of(&h.call(&token, Some(&session), request_body(2, "ping", json!({}))));
    assert_eq!(ping["result"], json!({}));
    let batch = h.call(
        &token,
        Some(&session),
        json!([
            request_body(4, "ping", json!({})),
            request_body(5, "tools/list", json!({})),
            { "jsonrpc": "2.0", "method": "notifications/initialized" }
        ]),
    );
    assert_eq!(body_of(&batch).as_array().map(Vec::len), Some(2));
    let garbage = h.raw(
        "POST",
        vec![bearer(&token), header("mcp-session-id", &session)],
        b"{nope".to_vec(),
    );
    assert_eq!(body_of(&garbage)["error"]["code"], -32700);
}

#[test]
fn tools_list_shows_what_the_scope_and_the_allowlist_permit() {
    let h = harness(true);
    let (reader, _) = h.mint(Scope::Observe, &[], None);
    let (operator, _) = h.mint(Scope::Operate, &[], None);
    let (narrow, _) = h.mint(Scope::Observe, &["get_agent", "not_a_tool"], None);
    let s_reader = h.session(&reader);
    let s_operator = h.session(&operator);
    let s_narrow = h.session(&narrow);
    let observe_tools = [
        "plaza_status",
        "list_agents",
        "get_agent",
        "get_transcript",
        "list_tasks",
        "get_task",
        "list_board",
        "list_projects",
        "get_project",
        "project_graph",
        "list_approvals",
        "list_machines",
        "treasury_summary",
        "usage_summary",
        "list_laws",
        "search_library",
        "prompts_list",
        "prompt_get",
        "skills_list",
        "skill_get",
        "station_list",
    ];
    let operate_tools = [
        "message_orchestrator",
        "tell_agent",
        "stop_agent",
        "restart_agent",
        "create_task",
        "prompt_propose",
        "skill_propose",
        "station_lease",
        "station_release",
        "station_screenshot",
        "station_click",
        "station_type",
        "station_key",
        "station_scroll",
        "station_drag",
        "station_wait",
        "station_ui_tree",
        "station_ui_act",
        "station_shell",
        "station_run",
        "station_files",
        "station_app_list",
        "station_app_launch",
        "app_script",
    ];
    let all: Vec<&str> = observe_tools
        .iter()
        .chain(operate_tools.iter())
        .copied()
        .collect();
    assert_eq!(listed_names(&h, &reader, &s_reader), observe_tools);
    assert_eq!(listed_names(&h, &operator, &s_operator), all);
    assert_eq!(listed_names(&h, &narrow, &s_narrow), ["get_agent"]);

    // The scope gate, on a registry that holds an operate tool.
    static REGISTRY: [&dyn Tool; 2] = [&FakeOperate, &observe::PlazaStatus];
    let token_of = |scope| TokenRecord {
        id: "t".into(),
        name: "t".into(),
        scope,
        allowlist: vec![],
        created_ms: 0,
        expires_ms: None,
        last_used_ms: None,
        calls: 0,
        revoked_at_ms: None,
    };
    let names = |scope| -> Vec<&'static str> {
        visible(&REGISTRY, &token_of(scope))
            .iter()
            .map(|tool| tool.name())
            .collect()
    };
    assert_eq!(names(Scope::Observe), vec!["plaza_status"]);
    assert_eq!(names(Scope::Operate), vec!["fake_operate", "plaza_status"]);
}

#[test]
fn bad_missing_revoked_and_expired_tokens_get_401_with_a_challenge() {
    let h = harness(true);
    let (live, _) = h.mint(Scope::Observe, &[], None);
    let (revoked, record) = h.mint(Scope::Observe, &[], None);
    h.host.hub.tokens.revoke(&record.id, h.now());
    let (expiring, _) = h.mint(Scope::Observe, &[], Some(h.now() + 1_000));
    h.advance(2_000);
    let init = request_body(1, "initialize", json!({ "protocolVersion": "2025-06-18" }));
    let missing = h.raw("POST", vec![], serde_json::to_vec(&init).expect("json"));
    let bad = h.call("apz_mcp_not_a_token", None, init.clone());
    let gone = h.call(&revoked, None, init.clone());
    let late = h.call(&expiring, None, init.clone());
    for resp in [&missing, &bad, &gone, &late] {
        assert_eq!(resp.status, 401);
        assert_eq!(
            reply_header(resp, "WWW-Authenticate").as_deref(),
            Some("Bearer")
        );
    }
    assert_eq!(h.call(&live, None, init).status, 200);
}

#[test]
fn get_is_405_and_a_disabled_hub_answers_404() {
    let on = harness(true);
    let (token, _) = on.mint(Scope::Observe, &[], None);
    let get = on.raw("GET", vec![bearer(&token)], Vec::new());
    assert_eq!(get.status, 405);
    assert_eq!(reply_header(&get, "Allow").as_deref(), Some("POST"));

    let off = harness(false);
    let (token, _) = off.mint(Scope::Observe, &[], None);
    let init = request_body(1, "initialize", json!({ "protocolVersion": "2025-06-18" }));
    assert_eq!(off.call(&token, None, init).status, 404);
    assert_eq!(off.raw("GET", vec![bearer(&token)], Vec::new()).status, 404);
}

#[test]
fn prompts_list_and_get_read_the_library() {
    let h = harness(true);
    seed_prompt(&h.host.plaza, "coder-system");
    let (token, _) = h.mint(Scope::Observe, &[], None);
    let session = h.session(&token);
    let listed = body_of(&h.call(&token, Some(&session), tool_call("prompts_list", json!({}))));
    let rows = &listed["result"]["structuredContent"]["prompts"];
    assert_eq!(rows.as_array().map(Vec::len), Some(1));
    assert_eq!(rows[0]["id"], "coder-system");
    assert_eq!(rows[0]["kind"], "system");
    assert_eq!(rows[0]["active_version"], Value::Null);
    assert_eq!(rows[0]["latest_version"], 1);
    assert_eq!(rows[0]["score"], Value::Null);
    let fragments = body_of(&h.call(
        &token,
        Some(&session),
        tool_call("prompts_list", json!({ "kind": "fragment" })),
    ));
    assert_eq!(
        fragments["result"]["structuredContent"]["prompts"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );
    let bad_kind = body_of(&h.call(
        &token,
        Some(&session),
        tool_call("prompts_list", json!({ "kind": "poem" })),
    ));
    assert_eq!(bad_kind["result"]["isError"], true);
    // Nothing is live, so the latest version is read.
    let got = body_of(&h.call(
        &token,
        Some(&session),
        tool_call("prompt_get", json!({ "id": "coder-system" })),
    ));
    assert_eq!(got["result"]["structuredContent"]["version"], 1);
    assert_eq!(
        got["result"]["structuredContent"]["body"],
        apz_prompt::template(Kind::System)
    );
    let missing = body_of(&h.call(
        &token,
        Some(&session),
        tool_call("prompt_get", json!({ "id": "nope" })),
    ));
    assert_eq!(missing["result"]["isError"], true);
}

#[test]
fn prompt_propose_is_operate_only_and_never_publishes() {
    let h = harness(true);
    seed_prompt(&h.host.plaza, "coder-system");
    let (reader, _) = h.mint(Scope::Observe, &[], None);
    let (operator, _) = h.mint(Scope::Operate, &[], None);
    let reader_session = h.session(&reader);
    let operator_session = h.session(&operator);
    let args =
        json!({ "id": "coder-system", "body": "a new body", "note": "try", "base_version": 1 });
    let refused = body_of(&h.call(
        &reader,
        Some(&reader_session),
        tool_call("prompt_propose", args.clone()),
    ));
    assert!(
        refused["error"].is_object(),
        "an observe token must not reach prompt_propose"
    );
    let proposed = body_of(&h.call(
        &operator,
        Some(&operator_session),
        tool_call("prompt_propose", args),
    ));
    assert_eq!(proposed["result"]["isError"], false);
    assert_eq!(proposed["result"]["structuredContent"]["version"], 2);
    assert_eq!(proposed["result"]["structuredContent"]["created"], true);
    let same = json!({ "id": "coder-system", "body": "a new body", "base_version": 2 });
    let unchanged = body_of(&h.call(
        &operator,
        Some(&operator_session),
        tool_call("prompt_propose", same),
    ));
    assert_eq!(unchanged["result"]["structuredContent"]["created"], false);
    assert_eq!(unchanged["result"]["structuredContent"]["version"], 2);
    let stored = h
        .host
        .plaza
        .prompts
        .store()
        .get("coder-system")
        .expect("a read")
        .expect("a prompt");
    assert_eq!(stored.active_version, None, "a proposal must not publish");
}

#[test]
fn prompt_publish_is_govern_only_and_refuses_a_version_with_errors() {
    let h = harness(true);
    seed_prompt(&h.host.plaza, "coder-system");
    let broken = h
        .host
        .plaza
        .prompts
        .add_version(
            "coder-system",
            1,
            "```\nbroken".into(),
            "broken".into(),
            "owner",
            T0,
        )
        .expect("a version")
        .0;
    let (operator, _) = h.mint(Scope::Operate, &[], None);
    let (governor, _) = h.mint(Scope::Govern, &[], None);
    let operator_session = h.session(&operator);
    let governor_session = h.session(&governor);
    let by_operator = body_of(&h.call(
        &operator,
        Some(&operator_session),
        tool_call(
            "prompt_publish",
            json!({ "id": "coder-system", "version": 1 }),
        ),
    ));
    assert!(
        by_operator["error"].is_object(),
        "an operate token must not reach prompt_publish"
    );
    let refused = body_of(&h.call(
        &governor,
        Some(&governor_session),
        tool_call(
            "prompt_publish",
            json!({ "id": "coder-system", "version": broken.version }),
        ),
    ));
    assert_eq!(
        refused["result"]["isError"], true,
        "a version with errors must not publish"
    );
    let published = body_of(&h.call(
        &governor,
        Some(&governor_session),
        tool_call(
            "prompt_publish",
            json!({ "id": "coder-system", "version": 1 }),
        ),
    ));
    assert_eq!(published["result"]["isError"], false);
    assert_eq!(
        published["result"]["structuredContent"]["active_version"],
        1
    );
}

#[test]
fn plaza_status_answers_with_structured_content_and_text() {
    let h = harness(true);
    let (token, _) = h.mint(Scope::Observe, &[], None);
    let session = h.session(&token);
    let body = body_of(&h.call(&token, Some(&session), tool_call("plaza_status", json!({}))));
    let result = &body["result"];
    assert_eq!(result["isError"], false);
    assert_eq!(result["structuredContent"]["agents"]["total"], 2);
    assert_eq!(result["content"][0]["type"], "text");
    assert!(
        result["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("2 agents"))
    );
}

#[test]
fn list_agents_and_get_agent_read_the_roster_and_refuse_bad_input() {
    let h = harness(true);
    let (token, _) = h.mint(Scope::Observe, &[], None);
    let session = h.session(&token);
    let listed = body_of(&h.call(
        &token,
        Some(&session),
        tool_call("list_agents", json!({ "role": "coder" })),
    ));
    assert_eq!(
        listed["result"]["structuredContent"]["agents"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        listed["result"]["structuredContent"]["agents"][0]["name"],
        "Ada"
    );
    let one = body_of(&h.call(
        &token,
        Some(&session),
        tool_call("get_agent", json!({ "cid": 2 })),
    ));
    assert_eq!(one["result"]["structuredContent"]["name"], "Bo");
    let missing = body_of(&h.call(
        &token,
        Some(&session),
        tool_call("get_agent", json!({ "cid": 9 })),
    ));
    assert_eq!(missing["result"]["isError"], true);
    let too_many = body_of(&h.call(
        &token,
        Some(&session),
        tool_call("list_agents", json!({ "limit": 500 })),
    ));
    assert_eq!(too_many["result"]["isError"], true);
}

#[test]
fn the_observe_limit_is_60_a_minute_then_32029() {
    let h = harness(true);
    let (token, _) = h.mint(Scope::Observe, &[], None);
    let session = h.session(&token);
    for _ in 0..60 {
        let body = body_of(&h.call(&token, Some(&session), tool_call("plaza_status", json!({}))));
        assert_eq!(body["result"]["isError"], false);
    }
    let limited = body_of(&h.call(&token, Some(&session), tool_call("plaza_status", json!({}))));
    assert_eq!(limited["error"]["code"], -32029);
    assert_eq!(limited["error"]["data"]["retryAfter"], 60_000);
    h.advance(MINUTE);
    let again = body_of(&h.call(&token, Some(&session), tool_call("plaza_status", json!({}))));
    assert_eq!(again["result"]["isError"], false);
}

#[test]
fn every_tool_call_writes_an_audit_row() {
    let h = harness(true);
    let (token, record) = h.mint(Scope::Observe, &[], None);
    let session = h.session(&token);
    h.call(&token, Some(&session), tool_call("plaza_status", json!({})));
    h.call(
        &token,
        Some(&session),
        tool_call("get_agent", json!({ "cid": 9 })),
    );
    let rows = h.host.hub.audit.list();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].token_id.as_deref(), Some(record.id.as_str()));
    assert_eq!((rows[0].tool.as_str(), rows[0].ok), ("plaza_status", true));
    assert_eq!(rows[0].args_sha256, sha256_hex(b"{}"));
    assert_eq!(rows[1].tool, "get_agent");
    assert_eq!(
        (rows[1].ok, rows[1].error.as_deref()),
        (false, Some("tool_error"))
    );
    assert_eq!(
        rows[1].args_sha256,
        sha256_hex(json!({ "cid": 9 }).to_string().as_bytes())
    );
}

#[test]
fn a_session_expires_after_an_hour_idle_and_is_bound_to_its_token() {
    let h = harness(true);
    let (token, _) = h.mint(Scope::Observe, &[], None);
    let (other, _) = h.mint(Scope::Observe, &[], None);
    let session = h.session(&token);
    h.advance(59 * MINUTE);
    assert_eq!(
        h.call(&token, Some(&session), request_body(4, "ping", json!({})))
            .status,
        200
    );
    assert_eq!(
        h.call(&other, Some(&session), request_body(5, "ping", json!({})))
            .status,
        404
    );
    h.advance(61 * MINUTE);
    assert_eq!(
        h.call(&token, Some(&session), request_body(6, "ping", json!({})))
            .status,
        404
    );
    assert_eq!(
        h.call(
            &token,
            Some("no-such-session"),
            request_body(7, "ping", json!({}))
        )
        .status,
        404
    );
}

#[test]
fn the_registry_never_offers_a_tool_that_reaches_the_never_list() {
    const BANNED: [&str; 17] = [
        "vault",
        "secret",
        "key",
        "token",
        "join",
        "mint",
        "fine",
        "transfer",
        "payment",
        "law_edit",
        "override",
        "sanction",
        "stop_all",
        "resume",
        "promote",
        "retire_model",
        "setting",
    ];
    // `station_key` presses a keyboard key on a workstation (the word is in its name by meaning, not as a credential).
    const ALLOWED: [&str; 3] = ["propose_law_change", "stop_agent", "station_key"];
    for tool in REGISTRY.iter() {
        let name = tool.name();
        if ALLOWED.contains(&name) {
            continue;
        }
        for word in BANNED {
            assert!(
                !name.contains(word),
                "{name} reaches the never list through {word}"
            );
        }
    }
}

#[test]
fn a_token_cannot_call_a_tool_above_its_scope() {
    let h = harness(true);
    let (observe, _) = h.mint(Scope::Observe, &[], None);
    let (operate, _) = h.mint(Scope::Operate, &[], None);
    let observe_session = h.session(&observe);
    let operate_session = h.session(&operate);
    // Scope is checked before a tool runs, so these refusals never reach the service behind the tool.
    let cases = [
        (
            &observe,
            &observe_session,
            "tell_agent",
            json!({ "cid": 1, "text": "hi" }),
        ),
        (
            &observe,
            &observe_session,
            "skill_propose",
            json!({ "id": "pdf-forms", "skill_md": "# PDF forms\n", "files": [], "base_version": 1 }),
        ),
        (
            &operate,
            &operate_session,
            "request_decision",
            json!({ "approval_id": "a1", "decision": "deny", "note": "" }),
        ),
        (
            &operate,
            &operate_session,
            "skill_publish",
            json!({ "id": "pdf-forms", "version": 1 }),
        ),
    ];
    for (token, session, tool, args) in cases {
        let body = body_of(&h.call(token, Some(session.as_str()), tool_call(tool, args)));
        assert!(
            body.get("error").is_some() || body["result"]["isError"] == true,
            "{tool}: {body}"
        );
        assert!(
            body["result"]["structuredContent"].is_null(),
            "{tool}: {body}"
        );
    }
}

#[test]
fn one_happy_call_per_scope_reaches_its_service() {
    let h = harness(true);
    let (observe, _) = h.mint(Scope::Observe, &[], None);
    let session = h.session(&observe);
    let tasks = body_of(&h.call(
        &observe,
        Some(&session),
        tool_call("list_tasks", json!({ "status": "open" })),
    ));
    assert_eq!(tasks["result"]["isError"], false, "{tasks}");
    assert_eq!(tasks["result"]["structuredContent"]["tasks"][0]["id"], "t1");

    let (operate, _) = h.mint(Scope::Operate, &[], None);
    let session = h.session(&operate);
    let told = body_of(&h.call(
        &operate,
        Some(&session),
        tool_call("tell_agent", json!({ "cid": 1, "text": "look at t1" })),
    ));
    assert_eq!(told["result"]["isError"], false, "{told}");
    assert_eq!(told["result"]["structuredContent"]["text"], "look at t1");
}

#[test]
fn list_tasks_filters_by_state_project_and_claimant() {
    let h = harness(true);
    let (token, _) = h.mint(Scope::Observe, &[], None);
    let session = h.session(&token);
    let ids = |args: Value| -> Vec<String> {
        let body = body_of(&h.call(&token, Some(&session), tool_call("list_tasks", args)));
        assert_eq!(body["result"]["isError"], false, "{body}");
        body["result"]["structuredContent"]["tasks"]
            .as_array()
            .expect("a tasks array")
            .iter()
            .map(|task| task["id"].as_str().unwrap_or_default().to_string())
            .collect()
    };
    assert_eq!(ids(json!({})), ["t1", "t2", "t3"]);
    assert_eq!(ids(json!({ "status": "open" })), ["t1"]);
    assert_eq!(ids(json!({ "status": "claimed" })), ["t2", "t3"]);
    assert_eq!(ids(json!({ "project": "pr1" })), ["t1", "t3"]);
    assert_eq!(
        ids(json!({ "project": "pr2" })),
        ["t2"],
        "a task that names its project is that project's"
    );
    assert!(
        ids(json!({ "project": "pr9" })).is_empty(),
        "a project that does not exist has no tasks"
    );
    assert_eq!(ids(json!({ "assignee": 1 })), ["t2"]);
    assert_eq!(ids(json!({ "assignee": "coder" })), ["t2"]);
    assert_eq!(ids(json!({ "assignee": "cid:2" })), ["t3"]);
    assert_eq!(
        ids(json!({ "status": "claimed", "project": "pr1", "assignee": 2 })),
        ["t3"]
    );
}

#[test]
fn get_task_answers_the_task_and_says_its_state() {
    let h = harness(true);
    let (token, _) = h.mint(Scope::Observe, &[], None);
    let session = h.session(&token);
    let body = body_of(&h.call(
        &token,
        Some(&session),
        tool_call("get_task", json!({ "id": "t2" })),
    ));
    assert_eq!(
        body["result"]["structuredContent"]["agent_cid"], 1,
        "{body}"
    );
    assert_eq!(body["result"]["content"][0]["text"], "Task t2: claimed.");
}

#[test]
fn a_tool_result_in_a_transcript_is_cut_to_4_kb() {
    let h = harness(true);
    let (token, _) = h.mint(Scope::Observe, &[], None);
    let session = h.session(&token);
    let body = body_of(&h.call(
        &token,
        Some(&session),
        tool_call("get_transcript", json!({ "cid": 1 })),
    ));
    assert_eq!(body["result"]["isError"], false, "{body}");
    let entry = &body["result"]["structuredContent"]["entries"][0];
    assert_eq!(entry["data"]["truncated"], true);
    assert!(
        entry["data"]["preview"]
            .as_str()
            .is_some_and(|text| text.len() == 4096)
    );
}

#[test]
fn search_library_finds_a_package_by_its_text() {
    let h = harness(true);
    let (observe, _) = h.mint(Scope::Observe, &[], None);
    let session = h.session(&observe);
    let found = body_of(&h.call(
        &observe,
        Some(&session),
        tool_call("search_library", json!({ "query": "weather" })),
    ));
    assert_eq!(found["result"]["isError"], false, "{found}");
    assert_eq!(
        found["result"]["structuredContent"]["packages"][0]["name"],
        "weather"
    );
    let none = body_of(&h.call(
        &observe,
        Some(&session),
        tool_call("search_library", json!({ "query": "zzz" })),
    ));
    assert_eq!(none["result"]["structuredContent"]["packages"], json!([]));
}

#[test]
fn restart_agent_is_an_operate_tool_and_refuses_an_unknown_agent() {
    let h = harness(true);
    let (operate, _) = h.mint(Scope::Operate, &[], None);
    let session = h.session(&operate);
    let ok = body_of(&h.call(
        &operate,
        Some(&session),
        tool_call("restart_agent", json!({ "cid": 1 })),
    ));
    assert_eq!(ok["result"]["isError"], false, "{ok}");
    assert_eq!(ok["result"]["structuredContent"]["ok"], true);
    let refused = body_of(&h.call(
        &operate,
        Some(&session),
        tool_call("restart_agent", json!({ "cid": 9 })),
    ));
    assert_eq!(refused["result"]["isError"], true, "{refused}");
}

#[test]
fn request_decision_records_a_recommendation_and_never_decides() {
    let h = harness(true);
    let (govern, _) = h.mint(Scope::Govern, &[], None);
    let session = h.session(&govern);
    let body = body_of(&h.call(
        &govern,
        Some(&session),
        tool_call(
            "request_decision",
            json!({ "approval_id": "a1", "decision": "approve", "note": "looks fine" }),
        ),
    ));
    assert_eq!(body["result"]["isError"], false, "{body}");
    assert_eq!(body["result"]["structuredContent"]["recorded"], true);
    assert_eq!(body["result"]["structuredContent"]["decided"], false);
    let bad = body_of(&h.call(
        &govern,
        Some(&session),
        tool_call(
            "request_decision",
            json!({ "approval_id": "a1", "decision": "maybe", "note": "" }),
        ),
    ));
    assert_eq!(bad["result"]["isError"], true, "{bad}");
}

#[test]
fn create_task_files_a_task_in_a_project_and_refuses_an_empty_title() {
    let h = harness(true);
    let (operate, _) = h.mint(Scope::Operate, &[], None);
    let session = h.session(&operate);
    let made = body_of(&h.call(
        &operate,
        Some(&session),
        tool_call(
            "create_task",
            json!({ "title": "Ship it", "project": "pr1" }),
        ),
    ));
    assert_eq!(made["result"]["isError"], false, "{made}");
    assert_eq!(made["result"]["structuredContent"]["id"], "t2");
    let refused = body_of(&h.call(
        &operate,
        Some(&session),
        tool_call("create_task", json!({ "title": "", "project": "pr1" })),
    ));
    assert_eq!(refused["result"]["isError"], true, "{refused}");
    // Every task belongs to a project.
    let loose = body_of(&h.call(
        &operate,
        Some(&session),
        tool_call("create_task", json!({ "title": "Ship it" })),
    ));
    assert_eq!(loose["result"]["isError"], true, "{loose}");
    // The links and the project go to the plaza as the console's New task sends them.
    let linked = body_of(&h.call(
        &operate,
        Some(&session),
        tool_call(
            "create_task",
            json!({ "title": "Ship it", "after": ["t1"], "project": "pr1" }),
        ),
    ));
    assert_eq!(linked["result"]["isError"], false, "{linked}");
    assert_eq!(
        linked["result"]["structuredContent"]["after"],
        json!(["t1"])
    );
    assert_eq!(linked["result"]["structuredContent"]["project"], "pr1");
}

#[test]
fn project_graph_lays_out_a_projects_tasks_and_refuses_an_unknown_project() {
    let h = harness(true);
    let (token, _) = h.mint(Scope::Observe, &[], None);
    let session = h.session(&token);
    let body = body_of(&h.call(
        &token,
        Some(&session),
        tool_call("project_graph", json!({ "project": "pr1" })),
    ));
    assert_eq!(body["result"]["isError"], false, "{body}");
    let graph = &body["result"]["structuredContent"];
    assert_eq!(graph["project"], "pr1");
    assert_eq!(graph["edges"], json!([["t1", "t2"]]));
    assert_eq!(graph["frontier"], json!(["t1"]));
    assert_eq!(graph["longest_chain"], json!(["t1", "t2"]));
    assert_eq!(graph["tasks"][1]["depth"], 1);
    assert_eq!(
        graph["waiting"],
        json!([{ "role": "coder", "tasks": ["t1"] }])
    );
    assert_eq!(
        body["result"]["content"][0]["text"],
        "2 tasks: 1 being worked on or ready, 1 waiting for an agent, 2 in the longest chain left."
    );

    let unknown = body_of(&h.call(
        &token,
        Some(&session),
        tool_call("project_graph", json!({ "project": "pr9" })),
    ));
    assert_eq!(unknown["result"]["isError"], true, "{unknown}");
}

/// Open mode, the listener's TLS, and the peer rules (docs/design/hub-mcp.md, "Auth modes").
mod open_modes {
    use super::*;
    use crate::hub_mcp::{AuthMode, Bind};

    /// A harness on a listener that is TLS or not, with the given settings and the hub's own default origins.
    fn built(tls: bool, settings: crate::hub_mcp::Settings) -> Harness {
        let clock = Arc::new(AtomicU64::new(T0));
        let reading = Arc::clone(&clock);
        let defaults = crate::hub_mcp::default_origins(19080, "https://hub.test", tls);
        let hub = HubMcp::new(
            false,
            vec![],
            Box::new(MemoryTokens::default()),
            Box::new(MemoryAudit::default()),
            Box::new(move || reading.load(Ordering::SeqCst)),
        )
        .listening(tls, defaults);
        hub.apply(&settings);
        Harness {
            host: TestHost {
                hub,
                plaza: fake_plaza(),
                clock,
            },
            router: routes(Router::new()),
        }
    }

    fn settings(
        enabled: bool,
        bind: Bind,
        mode: AuthMode,
        scope: Scope,
    ) -> crate::hub_mcp::Settings {
        crate::hub_mcp::Settings {
            enabled,
            bind,
            allowed_origins: vec![],
            auth_mode: mode,
            open_scope: scope,
        }
    }

    /// A POST from `from`, with the given headers, through the router.
    fn send(h: &Harness, from: &str, headers: Vec<(String, String)>, body: Value) -> Response {
        let peer: std::net::IpAddr = from.parse().expect("an address");
        let body = serde_json::to_vec(&body).expect("json");
        let req = Request {
            method: "POST".into(),
            path: "/mcp".into(),
            headers,
            body,
            peer: Some(peer),
            ..Request::default()
        };
        h.router.handle(&h.host, &req)
    }

    /// Initializes an anonymous session from `from` and returns its ID.
    fn anonymous_session(h: &Harness, from: &str) -> String {
        let init = request_body(1, "initialize", json!({ "protocolVersion": "2025-06-18" }));
        let resp = send(h, from, vec![], init);
        assert_eq!(resp.status, 200);
        reply_header(&resp, "Mcp-Session-Id").expect("a session header")
    }

    #[test]
    fn open_mode_serves_anonymous_callers_within_the_open_scope() {
        // An anonymous caller lists and observes without a token.
        {
            let h = built(
                false,
                settings(true, Bind::Local, AuthMode::Open, Scope::Observe),
            );
            let session = anonymous_session(&h, "127.0.0.1");
            let list = send(
                &h,
                "127.0.0.1",
                vec![header("mcp-session-id", &session)],
                request_body(2, "tools/list", json!({})),
            );
            let listed = body_of(&list)["result"]["tools"].to_string();
            assert!(listed.contains("plaza_status") && !listed.contains("create_task"));
            let call = send(
                &h,
                "127.0.0.1",
                vec![header("mcp-session-id", &session)],
                tool_call("plaza_status", json!({})),
            );
            assert_eq!(body_of(&call)["result"]["isError"], json!(false));
        }
        // An anonymous caller is refused a tool above the open scope, and the refusal is audited.
        {
            let h = built(
                false,
                settings(true, Bind::Local, AuthMode::Open, Scope::Observe),
            );
            let session = anonymous_session(&h, "127.0.0.1");
            let resp = send(
                &h,
                "127.0.0.1",
                vec![header("mcp-session-id", &session)],
                tool_call("create_task", json!({ "title": "x" })),
            );
            assert_eq!(body_of(&resp)["error"]["code"], json!(-32602));
            let rows = h.host.hub.audit.list();
            assert_eq!(rows[0].error.as_deref(), Some("unknown_tool"));
            assert_eq!(rows[0].token_id, None);
        }
        // A token gets its own scope in open mode.
        {
            let h = built(
                false,
                settings(true, Bind::Local, AuthMode::Open, Scope::Observe),
            );
            let (token, _) = h.mint(Scope::Operate, &[], None);
            let session = h.session(&token);
            let resp = h.call(
                &token,
                Some(&session),
                tool_call(
                    "create_task",
                    json!({ "title": "from a token", "project": "pr1" }),
                ),
            );
            assert_eq!(body_of(&resp)["result"]["isError"], json!(false));
        }
    }

    #[test]
    fn open_over_remote_needs_the_confirmation_and_remote_needs_tls() {
        let db = crate::db::Db::connect(&std::collections::BTreeMap::new());
        let body = |bind: &str, mode: &str, confirm: bool| crate::hub_mcp::owner::SettingsBody {
            enabled: true,
            bind: bind.into(),
            allowed_origins: vec![],
            auth_mode: mode.into(),
            open_scope: "observe".into(),
            confirm_open_remote: confirm,
        };
        let tls = built(
            true,
            settings(false, Bind::Local, AuthMode::Secure, Scope::Observe),
        );
        let ctx = crate::hub_mcp::owner::Ctx {
            hub: &tls.host.hub,
            db: &db,
            public_url: "https://hub.test",
        };
        assert_eq!(
            crate::hub_mcp::owner::apply_settings(&ctx, body("remote", "open", false)).status,
            400
        );
        assert_eq!(
            tls.host.hub.settings().auth_mode,
            AuthMode::Secure,
            "a refused body changes nothing"
        );
        assert_eq!(
            crate::hub_mcp::owner::apply_settings(&ctx, body("remote", "open", true)).status,
            200
        );
        assert_eq!(tls.host.hub.settings().auth_mode, AuthMode::Open);

        let plain = built(
            false,
            settings(false, Bind::Local, AuthMode::Secure, Scope::Observe),
        );
        let ctx = crate::hub_mcp::owner::Ctx {
            hub: &plain.host.hub,
            db: &db,
            public_url: "https://hub.test",
        };
        let refused = crate::hub_mcp::owner::apply_settings(&ctx, body("remote", "secure", false));
        assert_eq!(refused.status, 400);
        assert!(String::from_utf8_lossy(refused.bytes()).contains("not serving TLS"));
    }

    #[test]
    fn a_non_loopback_peer_gets_404_under_local_and_remote_needs_tls() {
        let init = || request_body(1, "initialize", json!({ "protocolVersion": "2025-06-18" }));
        let local = built(
            true,
            settings(true, Bind::Local, AuthMode::Secure, Scope::Observe),
        );
        let (token, _) = local.mint(Scope::Observe, &[], None);
        assert_eq!(
            send(&local, "203.0.113.7", vec![bearer(&token)], init()).status,
            404
        );
        assert_eq!(
            send(&local, "127.0.0.1", vec![bearer(&token)], init()).status,
            200
        );

        let plain = built(
            false,
            settings(true, Bind::Remote, AuthMode::Secure, Scope::Observe),
        );
        let (token, _) = plain.mint(Scope::Observe, &[], None);
        assert_eq!(
            send(&plain, "203.0.113.7", vec![bearer(&token)], init()).status,
            404,
            "remote over a plain connection"
        );

        let tls = built(
            true,
            settings(true, Bind::Remote, AuthMode::Secure, Scope::Observe),
        );
        let (token, _) = tls.mint(Scope::Observe, &[], None);
        assert_eq!(
            send(&tls, "203.0.113.7", vec![bearer(&token)], init()).status,
            200
        );
    }

    #[test]
    fn origin_check_refuses_foreign_and_admits_the_hubs_own() {
        // A foreign origin is refused with 403; the hub's own origin is allowed.
        {
            let h = harness(true);
            let (token, _) = h.mint(Scope::Observe, &[], None);
            let init = request_body(1, "initialize", json!({ "protocolVersion": "2025-06-18" }));
            let foreign = h.raw(
                "POST",
                vec![bearer(&token), header("origin", "https://evil.example")],
                serde_json::to_vec(&init).expect("json"),
            );
            assert_eq!(foreign.status, 403);
            let own = h.raw(
                "POST",
                vec![bearer(&token), header("origin", ORIGIN)],
                serde_json::to_vec(&init).expect("json"),
            );
            assert_eq!(own.status, 200);
        }
        // The hub's own origins are allowed whatever the owner's list says.
        {
            let h = built(
                false,
                settings(true, Bind::Local, AuthMode::Secure, Scope::Observe),
            );
            let (token, _) = h.mint(Scope::Observe, &[], None);
            let init = serde_json::to_vec(&request_body(
                1,
                "initialize",
                json!({ "protocolVersion": "2025-06-18" }),
            ))
            .expect("json");
            assert_eq!(
                h.raw(
                    "POST",
                    vec![bearer(&token), header("origin", "http://127.0.0.1:19080")],
                    init.clone()
                )
                .status,
                200
            );
            assert_eq!(
                h.raw(
                    "POST",
                    vec![bearer(&token), header("origin", "https://hub.test")],
                    init.clone()
                )
                .status,
                200
            );
            assert_eq!(
                h.raw(
                    "POST",
                    vec![bearer(&token), header("origin", "https://evil.example")],
                    init
                )
                .status,
                403
            );
            assert!(
                h.host
                    .hub
                    .effective_origins()
                    .contains(&"http://[::1]:19080".to_string())
            );
        }
    }
    #[test]
    fn an_anonymous_call_is_audited_with_its_ip_and_no_token() {
        let h = built(
            true,
            settings(true, Bind::Remote, AuthMode::Open, Scope::Observe),
        );
        let session = anonymous_session(&h, "203.0.113.7");
        let resp = send(
            &h,
            "203.0.113.7",
            vec![header("mcp-session-id", &session)],
            tool_call("plaza_status", json!({})),
        );
        assert_eq!(body_of(&resp)["result"]["isError"], json!(false));
        let rows = h.host.hub.audit.list();
        let last = rows.last().expect("a row");
        assert_eq!(last.token_id, None);
        assert_eq!(last.client_ip.as_deref(), Some("203.0.113.7"));
        assert_eq!(last.tool, "plaza_status");
    }
}
