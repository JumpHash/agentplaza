//! MCP tests: the runtime against the fixture server (a process over stdio, and HTTP), the vault scoping,
//! access and risk, and the laws. The fixture is the `mcp_fixture` binary (src/bin, built with apid).

use std::collections::{BTreeMap, BTreeSet, HashMap};
#[cfg(not(windows))]
use std::io::Read;
use std::io::{BufRead, BufReader};
use std::path::Path;
#[cfg(not(windows))]
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;

use super::client::ToolInfo;
use super::config::{
    Access, Limits, McpServer, Package, Pick, Risk, Setting, ToolPolicy, Transport, TransportKind,
};
use super::egress::Egress;
use super::http::check_endpoint;
use super::loopback::{
    Helper, KIND, Machine, Need, approval_for, parse_helper, revoke_for_delete, settle,
};
use super::runtime::{Ctx, Runtime, Status};
use super::runtimes::{Fetch, sha256_hex};
use super::stdio::{Child as ServerChild, SandboxLauncher};
use super::vm::{VmHost, VmRequest};
use super::{
    McpSecrets, OnBehalf, Secret, allowed, auto_risk, forget_risks, lookup, publish_risks,
    risk_for, wire_names,
};

const T0: u64 = 1_000_000;

/// The fixture binary, next to the test binary (target/debug/mcp_fixture, or mcp_fixture.exe on Windows).
fn fixture() -> String {
    let exe = std::env::current_exe().expect("the test binary");
    let debug = exe.parent().and_then(|p| p.parent()).expect("target/debug");
    let name = format!("mcp_fixture{}", std::env::consts::EXE_SUFFIX);
    debug.join(name).to_string_lossy().into_owned()
}

/// The URL the fixture is pinned at in the tests. `FixtureFetch` serves the built binary for it, so no test
/// touches the network.
const FIXTURE_URL: &str = "https://fixture.invalid/mcp_fixture";

/// Serves the fixture binary for its URL, and nothing else.
struct FixtureFetch;

impl Fetch for FixtureFetch {
    fn get(&self, url: &str, _limit: u64) -> Result<Vec<u8>, String> {
        if url == FIXTURE_URL {
            std::fs::read(fixture()).map_err(|e| e.to_string())
        } else {
            Err(format!("no fake download for {url}"))
        }
    }
}

/// The fixture as a pinned binary package: its URL, the SHA-256 of the built file, and the file to run.
fn fixture_package() -> Package {
    let bytes =
        std::fs::read(fixture()).expect("build the mcp_fixture binary: cargo build -p apid");
    Package::Binary {
        url: FIXTURE_URL.into(),
        sha256: sha256_hex(&bytes),
        entry: format!("mcp_fixture{}", std::env::consts::EXE_SUFFIX),
    }
}

/// A runtime for one test: its own state directory, the fixture served by `FixtureFetch`, the OS sandbox, and no
/// proxy (the proxy has its own tests).
fn runtime() -> Runtime {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "apz-mcp-tests-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    Runtime::new(dir, Arc::new(FixtureFetch), Arc::new(SandboxLauncher), None)
}

/// A microVM host for tests: available or not as given, and every launch is recorded and refused with `refusal`.
struct FakeVm {
    available: Result<(), String>,
    launched: Mutex<Vec<VmRequest>>,
    refusal: String,
}

impl VmHost for FakeVm {
    fn available(&self) -> Result<(), String> {
        self.available.clone()
    }

    fn launch(&self, request: &VmRequest) -> Result<Box<dyn ServerChild>, String> {
        self.launched.lock().unwrap().push(request.clone());
        Err(self.refusal.clone())
    }

    fn purge(&self, _server: &str) -> Result<(), String> {
        Ok(())
    }
}

/// A runtime for one test with `vm` as its microVM host, and the proxy on a free loopback port when `proxied`.
fn runtime_with_vm(vm: Arc<FakeVm>, proxied: bool) -> Runtime {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "apz-mcp-vm-tests-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let egress = proxied.then(|| Egress::new(port));
    Runtime::new(
        dir,
        Arc::new(FixtureFetch),
        Arc::new(SandboxLauncher),
        egress,
    )
    .with_vm(vm)
}

/// A vault for tests: entries by name, each bound to one server, optionally scoped to one node. Agents
/// map to a node.
#[derive(Default)]
struct Vault {
    /// (name, value, server id, node scope)
    entries: Vec<(String, String, String, Option<String>)>,
    /// agent cid to its node
    nodes: HashMap<u64, String>,
}

impl McpSecrets for Vault {
    fn resolve(&self, name: &str, server: &str, behalf: &OnBehalf) -> Result<Secret, String> {
        let caller = match behalf {
            OnBehalf::Agent { cid } => self.nodes.get(cid).cloned(),
            OnBehalf::Owner { node } => node.clone(),
        };
        let mut found: Option<&(String, String, String, Option<String>)> = None;
        for entry in self.entries.iter().filter(|e| e.0 == name && e.2 == server) {
            match &entry.3 {
                None => found = found.or(Some(entry)),
                Some(scope) if caller.as_deref() == Some(scope.as_str()) => found = Some(entry),
                Some(_) => {}
            }
        }
        match found {
            Some(entry) => Ok(Secret(entry.1.clone())),
            None => Err(format!(
                "no secret {name} is bound to mcp:{server} for this caller"
            )),
        }
    }
}

fn owner(node: &str) -> OnBehalf {
    OnBehalf::Owner {
        node: Some(node.into()),
    }
}

fn ctx<'a>(vault: &'a dyn McpSecrets, behalf: OnBehalf, node: &str, now: u64) -> Ctx<'a> {
    Ctx {
        secrets: vault,
        behalf,
        node: node.into(),
        blocklist: Vec::new(),
        now,
    }
}

/// A stdio server that runs the fixture.
fn stdio_server(id: &str, env: &[(&str, Setting)]) -> McpServer {
    McpServer {
        id: id.into(),
        name: format!("{id} fixture"),
        enabled: true,
        transport: Transport {
            package: Some(fixture_package()),
            args: vec!["stdio".into()],
            ..Transport::default()
        },
        env: env
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
        headers: BTreeMap::new(),
        tools: ToolPolicy::default(),
        access: Access {
            roles: Pick::all(),
            nodes: Pick::all(),
            projects: Pick::all(),
        },
        limits: Limits::default(),
        allow_private: false,
        storage: Default::default(),
        created_ms: 0,
        updated_ms: 0,
        offered: Vec::new(),
    }
}

fn with_timeout(mut server: McpServer, seconds: u64) -> McpServer {
    server.limits.timeout_s = seconds;
    server
}

fn value(text: &str) -> Setting {
    Setting::Value { value: text.into() }
}

#[test]
fn the_handshake_and_the_paged_listing_find_all_nine_tools() {
    let runtime = runtime();
    let vault = Vault::default();
    let server = stdio_server("fixture", &[]);
    let probe = runtime
        .probe(&server, &ctx(&vault, owner("n0"), "n0", T0))
        .unwrap();
    assert_eq!(probe.server_info.name, "fixture");
    assert_eq!(probe.server_info.protocol, "2025-06-18");
    let names: Vec<&str> = probe.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "echo",
            "fail",
            "slow",
            "big",
            "leak_env",
            "leak_header",
            "image",
            "crash",
            "delete_all"
        ]
    );
    assert_eq!(probe.resources.len(), 1);
    assert_eq!(probe.prompts.len(), 1);
    runtime.stop("fixture");
}

#[test]
fn a_call_returns_its_text_with_escapes_intact() {
    let runtime = runtime();
    let vault = Vault::default();
    let server = stdio_server("fixture", &[]);
    let out = runtime
        .call(
            &server,
            &ctx(&vault, owner("n0"), "n0", T0),
            "echo",
            json!({"text": "hi \"there\""}),
        )
        .unwrap();
    assert_eq!(out.text, "hi \"there\"");
    assert!(!out.is_error);
    assert!(
        out.tools.is_some(),
        "the first call starts the instance and lists its tools"
    );
    let again = runtime
        .call(
            &server,
            &ctx(&vault, owner("n0"), "n0", T0 + 1),
            "echo",
            json!({"text": "x"}),
        )
        .unwrap();
    assert!(
        again.tools.is_none(),
        "a running instance is not listed again"
    );
    runtime.stop("fixture");
}

#[test]
fn a_tool_error_is_an_error_result_not_a_failed_call() {
    let runtime = runtime();
    let vault = Vault::default();
    let server = stdio_server("fixture", &[]);
    let out = runtime
        .call(
            &server,
            &ctx(&vault, owner("n0"), "n0", T0),
            "fail",
            json!({}),
        )
        .unwrap();
    assert!(out.is_error);
    assert_eq!(out.text, "fixture failure");
    runtime.stop("fixture");
}

#[test]
fn a_slow_call_times_out_and_the_next_call_starts_again() {
    let runtime = runtime();
    let vault = Vault::default();
    let server = with_timeout(stdio_server("fixture", &[]), 1);
    let err = runtime
        .call(
            &server,
            &ctx(&vault, owner("n0"), "n0", T0),
            "slow",
            json!({"ms": 5000}),
        )
        .unwrap_err();
    assert!(err.contains("timed out after 1 s"), "{err}");
    assert_eq!(runtime.summary("fixture").status, Status::Stopped);
    let out = runtime
        .call(
            &server,
            &ctx(&vault, owner("n0"), "n0", T0 + 10),
            "echo",
            json!({"text": "back"}),
        )
        .unwrap();
    assert_eq!(out.text, "back");
    runtime.stop("fixture");
}

#[test]
fn a_large_output_is_capped_at_the_server_limit() {
    let runtime = runtime();
    let vault = Vault::default();
    let mut server = stdio_server("fixture", &[]);
    server.limits.max_output_kb = 1;
    let out = runtime
        .call(
            &server,
            &ctx(&vault, owner("n0"), "n0", T0),
            "big",
            json!({"bytes": 5000}),
        )
        .unwrap();
    assert!(
        out.text.ends_with("[truncated at 1 KiB]"),
        "{}",
        &out.text[out.text.len().saturating_sub(60)..]
    );
    assert!(out.text.len() < 1100);
    runtime.stop("fixture");
}

#[test]
fn a_secret_echoed_by_the_server_never_comes_back() {
    let runtime = runtime();
    let mut vault = Vault::default();
    vault.entries.push((
        "TEST_TOKEN".into(),
        "SENTINEL-SECRET-7731".into(),
        "fixture".into(),
        None,
    ));
    let server = stdio_server(
        "fixture",
        &[(
            "SENTINEL_VAR",
            Setting::Secret {
                secret: "TEST_TOKEN".into(),
                format: None,
            },
        )],
    );
    let out = runtime
        .call(
            &server,
            &ctx(&vault, owner("n0"), "n0", T0),
            "leak_env",
            json!({"name": "SENTINEL_VAR"}),
        )
        .unwrap();
    assert_eq!(
        out.text,
        super::runtime::REDACTED,
        "the value reached the child and was scrubbed"
    );
    let encoded = runtime
        .call(
            &server,
            &ctx(&vault, owner("n0"), "n0", T0 + 1),
            "echo",
            json!({"text": "SENTINEL-SECRET-7731"}),
        )
        .unwrap();
    assert!(!encoded.text.contains("SENTINEL"));
    runtime.stop("fixture");
}

#[test]
fn vault_scopes_secrets_to_their_server_and_node() {
    // A server cannot resolve a secret bound to another server.
    {
        let mut vault = Vault::default();
        vault.entries.push((
            "GITHUB_TOKEN".into(),
            "gh-value".into(),
            "github".into(),
            None,
        ));
        assert!(
            vault
                .resolve("GITHUB_TOKEN", "github", &owner("n0"))
                .is_ok()
        );
        assert!(
            vault
                .resolve("GITHUB_TOKEN", "linear", &owner("n0"))
                .is_err()
        );
    }
    // Node scope applies through the calling agent.
    {
        let mut vault = Vault::default();
        vault.entries.push((
            "KEY".into(),
            "node-one".into(),
            "fixture".into(),
            Some("n1".into()),
        ));
        vault.nodes.insert(7, "n1".into());
        vault.nodes.insert(8, "n2".into());
        assert!(
            vault
                .resolve("KEY", "fixture", &OnBehalf::Agent { cid: 7 })
                .is_ok()
        );
        assert!(
            vault
                .resolve("KEY", "fixture", &OnBehalf::Agent { cid: 8 })
                .is_err()
        );
    }
}

#[test]
fn a_server_that_exits_at_start_backs_off_and_gives_up() {
    let runtime = runtime();
    let vault = Vault::default();
    let server = stdio_server("crashy", &[("FIXTURE_MODE", value("exit_at_start"))]);
    // Failures at t=0,1s,3s,7s,15s (the backoff gaps are 1, 2, 4, 8 s); the fifth gives up.
    let mut now = T0;
    for expected_wait in [1u64, 2, 4, 8] {
        assert!(
            runtime
                .call(
                    &server,
                    &ctx(&vault, owner("n0"), "n0", now),
                    "echo",
                    json!({"text": "x"})
                )
                .is_err()
        );
        let early = runtime
            .call(
                &server,
                &ctx(&vault, owner("n0"), "n0", now + 500),
                "echo",
                json!({"text": "x"}),
            )
            .unwrap_err();
        assert!(early.contains("restarts after a failure"), "{early}");
        now += expected_wait * 1000;
    }
    // The fifth start fails and gives up; the call after it says so.
    assert!(
        runtime
            .call(
                &server,
                &ctx(&vault, owner("n0"), "n0", now),
                "echo",
                json!({"text": "x"})
            )
            .is_err()
    );
    let last = runtime
        .call(
            &server,
            &ctx(&vault, owner("n0"), "n0", now + 1),
            "echo",
            json!({"text": "x"}),
        )
        .unwrap_err();
    assert!(last.contains("gave up after 5 failures"), "{last}");
    assert_eq!(runtime.summary("crashy").status, Status::Error);
    // An owner's start clears the failures, so the next call tries again.
    runtime.reset("crashy");
    let retry = runtime
        .call(
            &server,
            &ctx(&vault, owner("n0"), "n0", now + 1),
            "echo",
            json!({"text": "x"}),
        )
        .unwrap_err();
    assert!(!retry.contains("gave up"), "{retry}");
}

#[test]
fn an_idle_instance_stops_and_the_next_call_starts_it() {
    let runtime = runtime();
    let vault = Vault::default();
    let mut server = stdio_server("fixture", &[]);
    server.limits.idle_stop_min = 1;
    runtime
        .call(
            &server,
            &ctx(&vault, owner("n0"), "n0", T0),
            "echo",
            json!({"text": "a"}),
        )
        .unwrap();
    assert_eq!(runtime.summary("fixture").status, Status::Running);
    runtime.reap_idle(T0 + 59_000);
    assert_eq!(
        runtime.summary("fixture").status,
        Status::Running,
        "not idle long enough"
    );
    runtime.reap_idle(T0 + 60_000);
    assert_eq!(runtime.summary("fixture").status, Status::Stopped);
    let out = runtime
        .call(
            &server,
            &ctx(&vault, owner("n0"), "n0", T0 + 61_000),
            "echo",
            json!({"text": "b"}),
        )
        .unwrap();
    assert_eq!(out.text, "b");
    runtime.stop("fixture");
}

/// The fixture as an HTTP server on a free port, killed when dropped.
struct HttpFixture {
    child: Child,
    port: u16,
}

impl HttpFixture {
    fn start() -> HttpFixture {
        let mut child = apz_os::runner::command(std::path::Path::new(&fixture()))
            .args(["http", "0"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("the fixture");
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        HttpFixture {
            child,
            port: line.trim().parse().expect("the port"),
        }
    }
}

impl Drop for HttpFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn http_server(id: &str, url: String) -> McpServer {
    let mut server = stdio_server(id, &[]);
    server.transport = Transport {
        kind: TransportKind::Http,
        url: Some(url),
        ..Transport::default()
    };
    server.allow_private = true;
    server
}

#[test]
fn the_http_transport_reads_json_and_event_stream_replies_and_keeps_its_session() {
    let fixture = HttpFixture::start();
    let runtime = runtime();
    let vault = Vault::default();
    for path in ["mcp", "mcp/sse"] {
        let server = http_server(
            "remote",
            format!("http://127.0.0.1:{}/{path}", fixture.port),
        );
        let probe = runtime
            .probe(&server, &ctx(&vault, owner("n0"), "n0", T0))
            .unwrap();
        assert_eq!(probe.tools.len(), 9, "{path}");
        let out = runtime
            .call(
                &server,
                &ctx(&vault, owner("n0"), "n0", T0 + 1),
                "echo",
                json!({"text": path}),
            )
            .unwrap();
        assert_eq!(out.text, path);
        runtime.stop("remote");
    }
}

#[test]
fn an_http_bearer_secret_reaches_the_server_and_is_scrubbed_from_its_reply() {
    let fixture = HttpFixture::start();
    let runtime = runtime();
    let mut vault = Vault::default();
    vault.entries.push((
        "LINEAR_KEY".into(),
        "lin-SENTINEL-99".into(),
        "remote".into(),
        None,
    ));
    let mut server = http_server("remote", format!("http://127.0.0.1:{}/mcp", fixture.port));
    server.headers.insert(
        "Authorization".into(),
        Setting::Secret {
            secret: "LINEAR_KEY".into(),
            format: Some("Bearer {}".into()),
        },
    );
    let out = runtime
        .call(
            &server,
            &ctx(&vault, owner("n0"), "n0", T0),
            "leak_header",
            json!({"name": "authorization"}),
        )
        .unwrap();
    assert_eq!(out.text, format!("Bearer {}", super::runtime::REDACTED));
    runtime.stop("remote");
}

#[test]
fn a_private_http_endpoint_is_refused_unless_allowed() {
    assert!(check_endpoint("http://127.0.0.1:9/mcp", false, &[]).is_err());
    assert!(check_endpoint("https://192.168.1.10/mcp", false, &[]).is_err());
    assert!(check_endpoint("http://127.0.0.1:9/mcp", true, &[]).is_ok());
    let runtime = runtime();
    let vault = Vault::default();
    let mut server = stdio_server("remote", &[]);
    server.transport = Transport {
        kind: TransportKind::Http,
        url: Some("http://127.0.0.1:9/mcp".into()),
        ..Transport::default()
    };
    let err = runtime
        .call(
            &server,
            &ctx(&vault, owner("n0"), "n0", T0),
            "echo",
            json!({}),
        )
        .unwrap_err();
    assert!(err.contains("refused"), "{err}");
}

#[test]
fn access_admits_the_listed_roles_and_nodes_only() {
    let mut server = stdio_server("fixture", &[]);
    server.access = Access {
        roles: Pick::only(vec!["builder".into()]),
        nodes: Pick::only(vec!["n1".into()]),
        projects: Pick::all(),
    };
    assert!(allowed(&server, "builder", Some("n1"), Some("pr1")));
    assert!(!allowed(&server, "reviewer", Some("n1"), Some("pr1")));
    assert!(!allowed(&server, "builder", Some("n2"), Some("pr1")));
    assert!(
        !allowed(&server, "builder", None, Some("pr1")),
        "a caller with no known node is refused"
    );
    server.access = Access::default();
    assert!(
        !allowed(&server, "builder", Some("n1"), Some("pr1")),
        "an empty access list admits nobody"
    );
    server.access = Access {
        roles: Pick::all(),
        nodes: Pick::all(),
        projects: Pick::all(),
    };
    server.enabled = false;
    assert!(
        !allowed(&server, "builder", Some("n1"), Some("pr1")),
        "a disabled server is offered to nobody"
    );
}

#[test]
fn access_by_project_lets_in_that_projects_agents_only() {
    let mut server = stdio_server("fixture", &[]);
    server.access = Access {
        roles: Pick::all(),
        nodes: Pick::all(),
        projects: Pick::only(vec!["pr4".into()]),
    };
    assert!(allowed(&server, "builder", Some("n1"), Some("pr4")));
    assert!(!allowed(&server, "builder", Some("n1"), Some("pr5")));
    assert!(
        !allowed(&server, "orchestrator", Some("n1"), None),
        "a server kept to projects is not the plaza-wide agents'"
    );
    // A server from before projects names none, and stays open to every project.
    let old: Access =
        serde_json::from_value(serde_json::json!({"roles": "all", "nodes": "all"})).unwrap();
    assert_eq!(old.projects, Pick::all());
    server.access = old;
    assert!(allowed(&server, "orchestrator", Some("n1"), None));
}

fn tool(name: &str, annotations: serde_json::Value) -> ToolInfo {
    ToolInfo {
        name: name.into(),
        description: String::new(),
        input_schema: json!({"type": "object"}),
        annotations: serde_json::from_value(annotations).unwrap(),
    }
}

#[test]
fn risk_takes_the_owners_override_then_the_annotations() {
    let mut server = stdio_server("fixture", &[]);
    assert_eq!(
        risk_for(&server, &tool("a", json!({"readOnlyHint": true}))),
        Risk::Read
    );
    assert_eq!(
        risk_for(&server, &tool("b", json!({"destructiveHint": true}))),
        Risk::Irreversible
    );
    assert_eq!(risk_for(&server, &tool("c", json!({}))), Risk::Write);
    server.tools.risk.insert("b".into(), Risk::Write);
    assert_eq!(
        risk_for(&server, &tool("b", json!({"destructiveHint": true}))),
        Risk::Write
    );
}

#[test]
fn names_are_sanitised_and_kept_within_sixty_four_characters() {
    let plain = tool("search_issues", json!({}));
    assert_eq!(
        wire_names("github", std::slice::from_ref(&plain)),
        ["mcp__github__search_issues"]
    );
    let dotted = tool("search.issues", json!({}));
    assert_eq!(
        wire_names("github", &[dotted]),
        ["mcp__github__search_issues"]
    );
    let long = tool(&"x".repeat(80), json!({}));
    let name = wire_names("github", std::slice::from_ref(&long)).remove(0);
    assert_eq!(name.len(), 64);
    assert!(name.starts_with("mcp__github__xxx"));
    assert!(
        name.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    );
    let (_, hex) = name.rsplit_once('_').unwrap();
    assert_eq!(hex.len(), 6);
    assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(
        wire_names("github", &[long]),
        vec![name.clone()],
        "the suffix is deterministic"
    );
    let a = tool(&format!("{}a", "y".repeat(70)), json!({}));
    let b = tool(&format!("{}b", "y".repeat(70)), json!({}));
    let names = wire_names("github", &[a, b]);
    assert_ne!(
        names[0], names[1],
        "two long names with one suffix stay distinct"
    );
    assert!(names.iter().all(|n| n.len() == 64));
}

#[test]
fn colliding_names_get_distinct_wire_names_that_map_back_to_their_tools() {
    let listed = vec![
        tool("a.b", json!({"readOnlyHint": true})),
        tool("a_b", json!({"destructiveHint": true})),
    ];
    let wires = wire_names("collide", &listed);
    assert_eq!(wires[0], "mcp__collide__a_b");
    assert_ne!(wires[0], wires[1]);
    assert!(wires[1].starts_with("mcp__collide__a_b_"), "{}", wires[1]);
    let servers = BTreeMap::from([("collide".to_string(), stdio_server("collide", &[]))]);
    publish_risks(&servers, &BTreeMap::from([("collide".to_string(), listed)]));
    let first = lookup(&wires[0]).expect("registered");
    assert_eq!(
        (first.server.as_str(), first.tool.as_str(), first.risk),
        ("collide", "a.b", "read")
    );
    let second = lookup(&wires[1]).expect("registered");
    assert_eq!((second.tool.as_str(), second.risk), ("a_b", "irreversible"));
    forget_risks("collide");
    assert!(lookup(&wires[0]).is_none());
}

#[test]
fn the_laws_permit_an_enabled_read_tool_and_hold_an_irreversible_one() {
    let laws = crate::laws::Laws::builtin();
    let listed = vec![
        tool("search", json!({"readOnlyHint": true})),
        tool("wipe", json!({"destructiveHint": true})),
    ];
    let wires = wire_names("lawtest", &listed);
    publish_risks(
        &BTreeMap::from([("lawtest".to_string(), stdio_server("lawtest", &[]))]),
        &BTreeMap::from([("lawtest".to_string(), listed)]),
    );
    assert!(
        laws.permits("builder", &wires[0]),
        "an enabled read tool is permitted"
    );
    assert!(
        !laws.permits("builder", &wires[1]),
        "C-004a holds an irreversible tool without an owner approval"
    );
}

#[test]
fn the_fixture_binary_is_built_next_to_the_tests() {
    assert!(
        std::path::Path::new(&fixture()).exists(),
        "build the mcp_fixture binary: cargo build -p apid"
    );
}

#[test]
fn tool_views_show_the_agent_name_the_annotation_risk_and_the_effective_risk() {
    let mut server = stdio_server("fixture", &[]);
    server.tools.risk.insert("wipe".into(), Risk::Write);
    let wipe = tool("wipe", json!({"destructiveHint": true}));
    assert_eq!(
        auto_risk(&wipe),
        Risk::Irreversible,
        "the annotations alone say irreversible"
    );
    let rows = super::routes::tool_rows(&server, std::slice::from_ref(&wipe));
    assert_eq!(rows[0]["agent_name"], "mcp__fixture__wipe");
    assert_eq!(
        rows[0]["auto_risk"], "irreversible",
        "before the owner's override"
    );
    assert_eq!(
        rows[0]["risk"], "write",
        "the effective risk takes the override"
    );
    let plain = super::routes::tool_rows(
        &stdio_server("fixture", &[]),
        &[tool("echo", json!({"readOnlyHint": true}))],
    );
    assert_eq!(
        (plain[0]["auto_risk"].as_str(), plain[0]["risk"].as_str()),
        (Some("read"), Some("read"))
    );
}

#[test]
fn a_start_that_fails_before_launch_leaves_its_reason_on_the_server_view() {
    // The package cannot be downloaded: the start fails before anything runs, and the reason stays on the server.
    let runtime = runtime();
    let vault = Vault::default();
    let mut server = stdio_server("nodownload", &[]);
    server.transport.package = Some(Package::Binary {
        url: "https://nowhere.invalid/server".into(),
        sha256: "0".repeat(64),
        entry: "server".into(),
    });
    assert!(
        runtime
            .probe(&server, &ctx(&vault, owner("n0"), "n0", T0))
            .is_err()
    );
    let summary = runtime.summary("nodownload");
    assert_eq!(summary.status, Status::Error);
    assert!(
        summary
            .last_error
            .unwrap_or_default()
            .contains("nowhere.invalid")
    );
}

// The sandboxed server reaches the proxy only through its container's loopback exemption, which on Windows an
// administrator grants through UAC. The CI runner cannot click that prompt, so these run off Windows only. The
// fake-machine tests at the end of this file run everywhere.
#[test]
#[cfg(not(windows))]
fn the_sandboxed_fixture_reaches_the_proxy_port_and_nothing_else() {
    // The proxy's port is the only loopback port a sandboxed server may connect to: another local listener, and an
    // address beyond the loopback, are refused by the sandbox.
    let egress = Egress::new(0);
    let lease = egress.attach("probe", false, &[]).unwrap();
    let other = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let scratch = std::env::temp_dir().join(format!("apz-mcp-sandbox-{}", std::process::id()));
    let reach = |target: String| -> String {
        let spec = apz_sandbox::Spec {
            program: PathBuf::from(fixture()),
            args: vec!["connect".into(), target],
            env: Vec::new(),
            workdir: scratch.clone(),
            read_only: Vec::new(),
            read_write: Vec::new(),
            network: apz_sandbox::Network::LoopbackPort(lease.port()),
            limits: apz_sandbox::Limits::default(),
        };
        let mut child =
            apz_sandbox::spawn(&spec, Stdio::null(), Stdio::piped(), Stdio::piped()).unwrap();
        let mut out = String::new();
        child.stdout().unwrap().read_to_string(&mut out).unwrap();
        let mut err = String::new();
        child.stderr().unwrap().read_to_string(&mut err).unwrap();
        child.wait().unwrap();
        format!("{out}{err}")
    };
    let proxy = reach(format!("127.0.0.1:{}", lease.port()));
    assert!(proxy.starts_with("connected"), "{proxy}");
    let local = reach(format!("127.0.0.1:{}", other.local_addr().unwrap().port()));
    assert!(local.starts_with("refused"), "{local}");
    let beyond = reach("192.0.2.1:443".into());
    assert!(beyond.starts_with("refused"), "{beyond}");
}

#[test]
fn a_server_runs_in_a_microvm_with_its_directories_as_volumes_and_the_proxy_on_the_guest_port() {
    let vm = Arc::new(FakeVm {
        available: Ok(()),
        launched: Mutex::new(Vec::new()),
        refusal: "the fake node refused the boot".into(),
    });
    let runtime = runtime_with_vm(Arc::clone(&vm), true);
    let vault = Vault::default();
    let server = stdio_server("fixture", &[]);
    let refused = runtime
        .probe(&server, &ctx(&vault, owner("n0"), "n0", T0))
        .unwrap_err();
    assert!(
        refused.contains("the fake node refused the boot"),
        "{refused}"
    );
    assert_eq!(runtime.summary("fixture").launcher, "vm");
    let launched = vm.launched.lock().unwrap();
    assert_eq!(launched.len(), 1);
    let request = &launched[0];
    assert!(
        request.program.starts_with("/data/ro-"),
        "{}",
        request.program
    );
    assert!(
        request
            .program
            .ends_with(&format!("/mcp_fixture{}", std::env::consts::EXE_SUFFIX)),
        "{}",
        request.program
    );
    assert_eq!(request.args, ["stdio"]);
    assert_eq!(request.cwd, "/workspace");
    assert_ne!(request.egress_port, 0);
    assert!(request.env["HTTPS_PROXY"].ends_with("@127.0.0.1:3128"));
    assert!(request.mounts.iter().all(|m| m.name.starts_with("ro-")));
}

// The egress proxy needs a loopback port, which the Windows AppContainer cannot reach (no loopback exemption).
#[test]
#[cfg(not(windows))]
fn a_node_that_cannot_boot_a_microvm_runs_the_server_in_the_sandbox_and_says_why() {
    let vm = Arc::new(FakeVm {
        available: Err("no /dev/kvm on this host".into()),
        launched: Mutex::new(Vec::new()),
        refusal: String::new(),
    });
    let runtime = runtime_with_vm(Arc::clone(&vm), true);
    let vault = Vault::default();
    let server = stdio_server("fixture", &[]);
    let probe = runtime
        .probe(&server, &ctx(&vault, owner("n0"), "n0", T0))
        .unwrap();
    assert_eq!(probe.server_info.name, "fixture");
    assert_eq!(
        runtime.summary("fixture").launcher,
        "sandbox: no /dev/kvm on this host"
    );
    assert!(vm.launched.lock().unwrap().is_empty());
    runtime.stop("fixture");
}

// The egress proxy needs a loopback port, which the Windows AppContainer cannot reach (no loopback exemption).
#[test]
#[cfg(not(windows))]
fn a_server_on_another_node_or_without_a_proxy_is_not_sent_to_a_microvm() {
    let vm = Arc::new(FakeVm {
        available: Ok(()),
        launched: Mutex::new(Vec::new()),
        refusal: "never".into(),
    });
    let vault = Vault::default();
    let server = stdio_server("fixture", &[]);
    let other_node = runtime_with_vm(Arc::clone(&vm), true);
    other_node
        .probe(&server, &ctx(&vault, owner("n7"), "n7", T0))
        .unwrap();
    assert_eq!(
        other_node.summary("fixture").launcher,
        "sandbox: node n7 is not this hub's node"
    );
    other_node.stop("fixture");
    let unproxied = runtime_with_vm(Arc::clone(&vm), false);
    unproxied
        .probe(&server, &ctx(&vault, owner("n0"), "n0", T0))
        .unwrap();
    assert_eq!(
        unproxied.summary("fixture").launcher,
        "sandbox: the server has no egress proxy"
    );
    unproxied.stop("fixture");
    assert!(vm.launched.lock().unwrap().is_empty());
}

/// Windows' loopback check, faked: a container is exempt when it is in `exempt`. Each grant and revoke is recorded
/// and fails with `refusal` when one is given (a declined prompt, say). `desktop` says whether the prompt can show.
struct FakeLoopback {
    exempt: Mutex<BTreeSet<String>>,
    desktop: bool,
    refusal: Option<String>,
    /// The helper's runs, in order: `grant <sid> <port>` or `revoke <sid>`.
    elevated: Mutex<Vec<String>>,
}

/// The container of every fake server.
const SID: &str = "S-1-15-2-1111";

impl FakeLoopback {
    fn new(desktop: bool, refusal: Option<&str>) -> Arc<FakeLoopback> {
        Arc::new(FakeLoopback {
            exempt: Mutex::new(BTreeSet::new()),
            desktop,
            refusal: refusal.map(str::to_string),
            elevated: Mutex::new(Vec::new()),
        })
    }

    fn elevated(&self) -> Vec<String> {
        self.elevated.lock().unwrap().clone()
    }
}

impl Machine for FakeLoopback {
    fn container_sid(&self, _workdir: &Path) -> Result<String, String> {
        Ok(SID.to_string())
    }

    fn is_exempt(&self, sid: &str) -> Result<bool, String> {
        Ok(self.exempt.lock().unwrap().contains(sid))
    }

    fn has_desktop(&self) -> bool {
        self.desktop
    }

    fn grant(&self, sid: &str, port: u16) -> Result<(), String> {
        self.elevated
            .lock()
            .unwrap()
            .push(format!("grant {sid} {port}"));
        if let Some(why) = &self.refusal {
            return Err(why.clone());
        }
        self.exempt.lock().unwrap().insert(sid.to_string());
        Ok(())
    }

    fn revoke(&self, sid: &str) -> Result<(), String> {
        self.elevated.lock().unwrap().push(format!("revoke {sid}"));
        if let Some(why) = &self.refusal {
            return Err(why.clone());
        }
        self.exempt.lock().unwrap().remove(sid);
        Ok(())
    }
}

/// A runtime whose stdio servers reach a proxy on a free port, with `machine` checking their containers. Returns the
/// port too.
fn loopback_runtime(machine: Arc<FakeLoopback>) -> (Runtime, u16) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "apz-mcp-loopback-tests-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let machine: Arc<dyn Machine> = machine;
    let runtime = Runtime::new(
        dir,
        Arc::new(FixtureFetch),
        Arc::new(SandboxLauncher),
        Some(Egress::new(port)),
    )
    .with_loopback(Some(machine));
    (runtime, port)
}

/// Starts `server` at `now`, which must be refused: returns the refusal.
fn refused_start(runtime: &Runtime, server: &str, now: u64) -> String {
    let vault = Vault::default();
    let Err(why) = runtime.probe(
        &stdio_server(server, &[]),
        &ctx(&vault, owner("n0"), "n0", now),
    ) else {
        panic!("a start without the loopback exemption must be refused");
    };
    why
}

#[test]
fn a_sandboxed_server_is_refused_until_its_container_is_exempt_from_loopback() {
    let fake = FakeLoopback::new(true, None);
    let (runtime, port) = loopback_runtime(fake.clone());
    let why = refused_start(&runtime, "gated", T0);
    assert!(why.contains("not exempt from loopback"), "{why}");
    assert_eq!(
        runtime.loopback_need("gated"),
        Some(Need {
            sid: SID.into(),
            port
        })
    );
    assert!(fake.elevated().is_empty());
    assert_eq!(runtime.summary("gated").status, Status::Error);
}

#[test]
fn one_pending_approval_serves_every_refusal_of_a_server() {
    let fake = FakeLoopback::new(true, None);
    let (runtime, _) = loopback_runtime(fake);
    refused_start(&runtime, "asks", T0);
    let need = runtime.loopback_need("asks").unwrap();
    let store = crate::approvals::Store::default();
    let first = approval_for(&store.list(), "asks", &need, |subject, detail| {
        store.submit(0, KIND, subject, detail)
    });
    let again = approval_for(&store.list(), "asks", &need, |subject, detail| {
        store.submit(0, KIND, subject, detail)
    });
    assert_eq!(first.id, again.id);
    assert_eq!(store.list().len(), 1);
    assert_eq!(first.kind, KIND);
    assert_eq!(first.detail["server"], json!("asks"));
    assert_eq!(first.detail["port"], json!(need.port));
    approval_for(&store.list(), "other", &need, |subject, detail| {
        store.submit(0, KIND, subject, detail)
    });
    assert_eq!(store.list().len(), 2);
}

#[test]
#[cfg(not(windows))]
fn an_approved_exemption_lets_the_next_start_run() {
    // On Windows the real check runs in the spawn too, which needs the exemption an administrator grants: the CI
    // runner cannot, so this runs off Windows only.
    let fake = FakeLoopback::new(true, None);
    let (runtime, port) = loopback_runtime(fake.clone());
    refused_start(&runtime, "approved", T0);
    let need = runtime.loopback_need("approved").unwrap();
    let store = crate::approvals::Store::default();
    let item = approval_for(&store.list(), "approved", &need, |subject, detail| {
        store.submit(0, KIND, subject, detail)
    });
    assert!(store.decide(&item.id, true, None).is_ok());
    settle(&runtime, "approved", &need, true).unwrap();
    assert_eq!(fake.elevated(), vec![format!("grant {SID} {port}")]);
    let vault = Vault::default();
    let started = runtime.probe(
        &stdio_server("approved", &[]),
        &ctx(&vault, owner("n0"), "n0", T0 + 120_000),
    );
    assert!(started.is_ok(), "{}", started.err().unwrap_or_default());
    assert_eq!(runtime.loopback_need("approved"), None);
    runtime.stop("approved");
}

#[test]
fn a_denied_approval_grants_nothing_and_the_server_stays_refused() {
    let fake = FakeLoopback::new(true, None);
    let (runtime, _) = loopback_runtime(fake.clone());
    refused_start(&runtime, "denied", T0);
    let need = runtime.loopback_need("denied").unwrap();
    settle(&runtime, "denied", &need, false).unwrap();
    assert!(fake.elevated().is_empty());
    let why = refused_start(&runtime, "denied", T0 + 120_000);
    assert!(why.contains("not exempt"), "{why}");
}

#[test]
fn a_declined_prompt_leaves_the_server_refused_with_that_reason() {
    let fake = FakeLoopback::new(
        true,
        Some("sandbox I/O error: the administrator prompt was declined"),
    );
    let (runtime, _) = loopback_runtime(fake.clone());
    refused_start(&runtime, "declined", T0);
    let need = runtime.loopback_need("declined").unwrap();
    let why = settle(&runtime, "declined", &need, true).unwrap_err();
    assert!(why.contains("declined"), "{why}");
    assert!(
        runtime
            .summary("declined")
            .last_error
            .unwrap_or_default()
            .contains("declined")
    );
    assert_eq!(fake.elevated().len(), 1);
    let again = refused_start(&runtime, "declined", T0 + 120_000);
    assert!(again.contains("not exempt"), "{again}");
}

#[test]
fn without_a_desktop_the_refusal_stays_and_the_command_is_printed() {
    let fake = FakeLoopback::new(false, None);
    let (runtime, port) = loopback_runtime(fake.clone());
    refused_start(&runtime, "headless", T0);
    let need = runtime.loopback_need("headless").unwrap();
    let why = settle(&runtime, "headless", &need, true).unwrap_err();
    assert!(
        why.contains(&format!(
            "apid windows-loopback grant --sid {SID} --port {port}"
        )),
        "{why}"
    );
    assert!(fake.elevated().is_empty());
}

#[test]
fn deleting_an_exempt_server_takes_its_container_exemption_away() {
    let fake = FakeLoopback::new(true, None);
    fake.exempt.lock().unwrap().insert(SID.into());
    let (runtime, _) = loopback_runtime(fake.clone());
    assert_eq!(revoke_for_delete(&runtime, Path::new("unused")), None);
    assert_eq!(fake.elevated(), vec![format!("revoke {SID}")]);
    assert!(!fake.exempt.lock().unwrap().contains(SID));
}

#[test]
fn deleting_a_server_that_is_not_exempt_runs_nothing() {
    let fake = FakeLoopback::new(true, None);
    let (runtime, _) = loopback_runtime(fake.clone());
    assert_eq!(revoke_for_delete(&runtime, Path::new("unused")), None);
    assert!(fake.elevated().is_empty());
}

#[test]
fn a_revoke_that_fails_on_delete_is_a_note_with_the_command() {
    let fake = FakeLoopback::new(true, Some("the administrator prompt was declined"));
    fake.exempt.lock().unwrap().insert(SID.into());
    let (runtime, _) = loopback_runtime(fake);
    let note = revoke_for_delete(&runtime, Path::new("unused")).unwrap();
    assert!(
        note.contains(&format!("apid windows-loopback revoke --sid {SID}")),
        "{note}"
    );
}

#[test]
fn a_server_without_the_proxy_is_not_checked_for_the_exemption() {
    let fake = FakeLoopback::new(true, None);
    let machine: Arc<dyn Machine> = fake.clone();
    let runtime = runtime().with_loopback(Some(machine));
    let vault = Vault::default();
    let started = runtime.probe(
        &stdio_server("direct", &[]),
        &ctx(&vault, owner("n0"), "n0", T0),
    );
    assert!(started.is_ok(), "{}", started.err().unwrap_or_default());
    assert!(runtime.loopback_need("direct").is_none());
    assert!(fake.elevated().is_empty());
    runtime.stop("direct");
}

#[test]
fn the_helper_takes_one_grant_or_revoke_of_an_appcontainer() {
    let args = |text: &str| text.split(' ').map(str::to_string).collect::<Vec<_>>();
    assert_eq!(
        parse_helper(&args(&format!("grant --sid {SID} --port 19128"))),
        Ok(Helper::Grant {
            sid: SID.into(),
            port: 19128
        })
    );
    assert_eq!(
        parse_helper(&args(&format!("revoke --sid {SID}"))),
        Ok(Helper::Revoke { sid: SID.into() })
    );
    for bad in [
        "grant --sid S-1-5-18 --port 19128",
        "grant --sid S-1-15-2-1",
        "grant --sid S-1-15-2-1 --port 0",
        "grant --sid S-1-15-2-1 --port 70000",
        "delete --sid S-1-15-2-1",
        "revoke --sid S-1-15-2-1 --all",
    ] {
        assert!(parse_helper(&args(bad)).is_err(), "{bad}");
    }
}
