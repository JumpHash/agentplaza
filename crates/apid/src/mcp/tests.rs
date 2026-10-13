//! MCP tests: the runtime against the fixture server (a process over stdio, and HTTP), the vault scoping,
//! access and risk, and the laws. The fixture is the `mcp_fixture` binary (src/bin, built with apid).

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::time::Duration;
use super::client::Transport as _;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

use serde_json::json;

use super::config::{Access, Limits, McpServer, Pick, Risk, Setting, ToolPolicy, Transport};
use super::http::check_endpoint;
use super::runtime::{Ctx, Runtime, Status};
use super::stdio::PlainLauncher;
use super::{allowed, auto_risk, lookup, publish_risks, risk_for, wire_names, forget_risks, McpSecrets, OnBehalf, Secret};
use super::client::ToolInfo;

const T0: u64 = 1_000_000;

/// The fixture binary, next to the test binary (target/debug/mcp_fixture).
fn fixture() -> String {
    let exe = std::env::current_exe().expect("the test binary");
    let debug = exe.parent().and_then(|p| p.parent()).expect("target/debug");
    debug.join("mcp_fixture").to_string_lossy().into_owned()
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
            None => Err(format!("no secret {name} is bound to mcp:{server} for this caller")),
        }
    }
}

fn owner(node: &str) -> OnBehalf {
    OnBehalf::Owner { node: Some(node.into()) }
}

fn ctx<'a>(vault: &'a dyn McpSecrets, behalf: OnBehalf, node: &str, now: u64) -> Ctx<'a> {
    Ctx { secrets: vault, behalf, node: node.into(), blocklist: Vec::new(), now }
}

/// A stdio server that runs the fixture.
fn stdio_server(id: &str, env: &[(&str, Setting)]) -> McpServer {
    McpServer {
        id: id.into(),
        name: format!("{id} fixture"),
        enabled: true,
        transport: Transport::Stdio { image: fixture(), command: vec!["stdio".into()], workdir: None },
        env: env.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
        headers: BTreeMap::new(),
        tools: ToolPolicy::default(),
        access: Access { roles: Pick::All, nodes: Pick::All },
        limits: Limits::default(),
        allow_private: false,
        storage: Default::default(),
        created_ms: 0,
        updated_ms: 0,
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
    let runtime = Runtime::new(Arc::new(PlainLauncher));
    let vault = Vault::default();
    let server = stdio_server("fixture", &[]);
    let probe = runtime.probe(&server, &ctx(&vault, owner("n0"), "n0", T0)).unwrap();
    assert_eq!(probe.server_info.name, "fixture");
    assert_eq!(probe.server_info.protocol, "2025-06-18");
    let names: Vec<&str> = probe.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["echo", "fail", "slow", "big", "leak_env", "leak_header", "image", "crash", "delete_all"]);
    assert_eq!(probe.resources.len(), 1);
    assert_eq!(probe.prompts.len(), 1);
    runtime.stop("fixture");
}

#[test]
fn a_call_returns_its_text_with_escapes_intact() {
    let runtime = Runtime::new(Arc::new(PlainLauncher));
    let vault = Vault::default();
    let server = stdio_server("fixture", &[]);
    let out = runtime.call(&server, &ctx(&vault, owner("n0"), "n0", T0), "echo", json!({"text": "hi \"there\""})).unwrap();
    assert_eq!(out.text, "hi \"there\"");
    assert!(!out.is_error);
    assert!(out.tools.is_some(), "the first call starts the instance and lists its tools");
    let again = runtime.call(&server, &ctx(&vault, owner("n0"), "n0", T0 + 1), "echo", json!({"text": "x"})).unwrap();
    assert!(again.tools.is_none(), "a running instance is not listed again");
    runtime.stop("fixture");
}

#[test]
fn a_tool_error_is_an_error_result_not_a_failed_call() {
    let runtime = Runtime::new(Arc::new(PlainLauncher));
    let vault = Vault::default();
    let server = stdio_server("fixture", &[]);
    let out = runtime.call(&server, &ctx(&vault, owner("n0"), "n0", T0), "fail", json!({})).unwrap();
    assert!(out.is_error);
    assert_eq!(out.text, "fixture failure");
    runtime.stop("fixture");
}

#[test]
fn a_slow_call_times_out_and_the_next_call_starts_again() {
    let runtime = Runtime::new(Arc::new(PlainLauncher));
    let vault = Vault::default();
    let server = with_timeout(stdio_server("fixture", &[]), 1);
    let err = runtime.call(&server, &ctx(&vault, owner("n0"), "n0", T0), "slow", json!({"ms": 5000})).unwrap_err();
    assert!(err.contains("timed out after 1 s"), "{err}");
    assert_eq!(runtime.summary("fixture").status, Status::Stopped);
    let out = runtime.call(&server, &ctx(&vault, owner("n0"), "n0", T0 + 10), "echo", json!({"text": "back"})).unwrap();
    assert_eq!(out.text, "back");
    runtime.stop("fixture");
}

#[test]
fn a_large_output_is_capped_at_the_server_limit() {
    let runtime = Runtime::new(Arc::new(PlainLauncher));
    let vault = Vault::default();
    let mut server = stdio_server("fixture", &[]);
    server.limits.max_output_kb = 1;
    let out = runtime.call(&server, &ctx(&vault, owner("n0"), "n0", T0), "big", json!({"bytes": 5000})).unwrap();
    assert!(out.text.ends_with("[truncated at 1 KiB]"), "{}", &out.text[out.text.len().saturating_sub(60)..]);
    assert!(out.text.len() < 1100);
    runtime.stop("fixture");
}

#[test]
fn a_secret_echoed_by_the_server_never_comes_back() {
    let runtime = Runtime::new(Arc::new(PlainLauncher));
    let mut vault = Vault::default();
    vault.entries.push(("TEST_TOKEN".into(), "SENTINEL-SECRET-7731".into(), "fixture".into(), None));
    let server = stdio_server("fixture", &[("SENTINEL_VAR", Setting::Secret { secret: "TEST_TOKEN".into(), format: None })]);
    let out = runtime.call(&server, &ctx(&vault, owner("n0"), "n0", T0), "leak_env", json!({"name": "SENTINEL_VAR"})).unwrap();
    assert_eq!(out.text, super::runtime::REDACTED, "the value reached the child and was scrubbed");
    let encoded = runtime.call(&server, &ctx(&vault, owner("n0"), "n0", T0 + 1), "echo", json!({"text": "SENTINEL-SECRET-7731"})).unwrap();
    assert!(!encoded.text.contains("SENTINEL"));
    runtime.stop("fixture");
}

#[test]
fn a_server_cannot_resolve_a_secret_bound_to_another_server() {
    let mut vault = Vault::default();
    vault.entries.push(("GITHUB_TOKEN".into(), "gh-value".into(), "github".into(), None));
    assert!(vault.resolve("GITHUB_TOKEN", "github", &owner("n0")).is_ok());
    assert!(vault.resolve("GITHUB_TOKEN", "linear", &owner("n0")).is_err());
}

#[test]
fn node_scope_applies_through_the_calling_agent() {
    let mut vault = Vault::default();
    vault.entries.push(("KEY".into(), "node-one".into(), "fixture".into(), Some("n1".into())));
    vault.nodes.insert(7, "n1".into());
    vault.nodes.insert(8, "n2".into());
    assert!(vault.resolve("KEY", "fixture", &OnBehalf::Agent { cid: 7 }).is_ok());
    assert!(vault.resolve("KEY", "fixture", &OnBehalf::Agent { cid: 8 }).is_err());
}

#[test]
fn a_server_that_exits_at_start_backs_off_and_gives_up() {
    let runtime = Runtime::new(Arc::new(PlainLauncher));
    let vault = Vault::default();
    let server = stdio_server("crashy", &[("FIXTURE_MODE", value("exit_at_start"))]);
    // Failures at t=0,1s,3s,7s,15s (the backoff gaps are 1, 2, 4, 8 s); the fifth gives up.
    let mut now = T0;
    for expected_wait in [1u64, 2, 4, 8] {
        assert!(runtime.call(&server, &ctx(&vault, owner("n0"), "n0", now), "echo", json!({"text": "x"})).is_err());
        let early = runtime.call(&server, &ctx(&vault, owner("n0"), "n0", now + 500), "echo", json!({"text": "x"})).unwrap_err();
        assert!(early.contains("restarts after a failure"), "{early}");
        now += expected_wait * 1000;
    }
    // The fifth start fails and gives up; the call after it says so.
    assert!(runtime.call(&server, &ctx(&vault, owner("n0"), "n0", now), "echo", json!({"text": "x"})).is_err());
    let last = runtime.call(&server, &ctx(&vault, owner("n0"), "n0", now + 1), "echo", json!({"text": "x"})).unwrap_err();
    assert!(last.contains("gave up after 5 failures"), "{last}");
    assert_eq!(runtime.summary("crashy").status, Status::Error);
    // An owner's start clears the failures, so the next call tries again.
    runtime.reset("crashy");
    let retry = runtime.call(&server, &ctx(&vault, owner("n0"), "n0", now + 1), "echo", json!({"text": "x"})).unwrap_err();
    assert!(!retry.contains("gave up"), "{retry}");
}

#[test]
fn an_idle_instance_stops_and_the_next_call_starts_it() {
    let runtime = Runtime::new(Arc::new(PlainLauncher));
    let vault = Vault::default();
    let mut server = stdio_server("fixture", &[]);
    server.limits.idle_stop_min = 1;
    runtime.call(&server, &ctx(&vault, owner("n0"), "n0", T0), "echo", json!({"text": "a"})).unwrap();
    assert_eq!(runtime.summary("fixture").status, Status::Running);
    runtime.reap_idle(T0 + 59_000);
    assert_eq!(runtime.summary("fixture").status, Status::Running, "not idle long enough");
    runtime.reap_idle(T0 + 60_000);
    assert_eq!(runtime.summary("fixture").status, Status::Stopped);
    let out = runtime.call(&server, &ctx(&vault, owner("n0"), "n0", T0 + 61_000), "echo", json!({"text": "b"})).unwrap();
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
        let mut child = Command::new(fixture()).args(["http", "0"]).stdout(Stdio::piped()).spawn().expect("the fixture");
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        HttpFixture { child, port: line.trim().parse().expect("the port") }
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
    server.transport = Transport::Http { url };
    server.allow_private = true;
    server
}

#[test]
fn the_http_transport_reads_json_and_event_stream_replies_and_keeps_its_session() {
    let fixture = HttpFixture::start();
    let runtime = Runtime::new(Arc::new(PlainLauncher));
    let vault = Vault::default();
    for path in ["mcp", "mcp/sse"] {
        let server = http_server("remote", format!("http://127.0.0.1:{}/{path}", fixture.port));
        let probe = runtime.probe(&server, &ctx(&vault, owner("n0"), "n0", T0)).unwrap();
        assert_eq!(probe.tools.len(), 9, "{path}");
        let out = runtime.call(&server, &ctx(&vault, owner("n0"), "n0", T0 + 1), "echo", json!({"text": path})).unwrap();
        assert_eq!(out.text, path);
        runtime.stop("remote");
    }
}

#[test]
fn an_http_bearer_secret_reaches_the_server_and_is_scrubbed_from_its_reply() {
    let fixture = HttpFixture::start();
    let runtime = Runtime::new(Arc::new(PlainLauncher));
    let mut vault = Vault::default();
    vault.entries.push(("LINEAR_KEY".into(), "lin-SENTINEL-99".into(), "remote".into(), None));
    let mut server = http_server("remote", format!("http://127.0.0.1:{}/mcp", fixture.port));
    server.headers.insert(
        "Authorization".into(),
        Setting::Secret { secret: "LINEAR_KEY".into(), format: Some("Bearer {}".into()) },
    );
    let out = runtime.call(&server, &ctx(&vault, owner("n0"), "n0", T0), "leak_header", json!({"name": "authorization"})).unwrap();
    assert_eq!(out.text, format!("Bearer {}", super::runtime::REDACTED));
    runtime.stop("remote");
}

#[test]
fn a_private_http_endpoint_is_refused_unless_allowed() {
    assert!(check_endpoint("http://127.0.0.1:9/mcp", false, &[]).is_err());
    assert!(check_endpoint("https://192.168.1.10/mcp", false, &[]).is_err());
    assert!(check_endpoint("http://127.0.0.1:9/mcp", true, &[]).is_ok());
    let runtime = Runtime::new(Arc::new(PlainLauncher));
    let vault = Vault::default();
    let mut server = stdio_server("remote", &[]);
    server.transport = Transport::Http { url: "http://127.0.0.1:9/mcp".into() };
    let err = runtime.call(&server, &ctx(&vault, owner("n0"), "n0", T0), "echo", json!({})).unwrap_err();
    assert!(err.contains("refused"), "{err}");
}

#[test]
fn access_admits_the_listed_roles_and_nodes_only() {
    let mut server = stdio_server("fixture", &[]);
    server.access = Access { roles: Pick::Only(vec!["builder".into()]), nodes: Pick::Only(vec!["n1".into()]) };
    assert!(allowed(&server, "builder", Some("n1")));
    assert!(!allowed(&server, "reviewer", Some("n1")));
    assert!(!allowed(&server, "builder", Some("n2")));
    assert!(!allowed(&server, "builder", None), "a caller with no known node is refused");
    server.access = Access::default();
    assert!(!allowed(&server, "builder", Some("n1")), "an empty access list admits nobody");
    server.access = Access { roles: Pick::All, nodes: Pick::All };
    server.enabled = false;
    assert!(!allowed(&server, "builder", Some("n1")), "a disabled server is offered to nobody");
}

fn tool(name: &str, annotations: serde_json::Value) -> ToolInfo {
    ToolInfo { name: name.into(), description: String::new(), input_schema: json!({"type": "object"}), annotations }
}

#[test]
fn risk_takes_the_owners_override_then_the_annotations() {
    let mut server = stdio_server("fixture", &[]);
    assert_eq!(risk_for(&server, &tool("a", json!({"readOnlyHint": true}))), Risk::Read);
    assert_eq!(risk_for(&server, &tool("b", json!({"destructiveHint": true}))), Risk::Irreversible);
    assert_eq!(risk_for(&server, &tool("c", json!({}))), Risk::Write);
    server.tools.risk.insert("b".into(), Risk::Write);
    assert_eq!(risk_for(&server, &tool("b", json!({"destructiveHint": true}))), Risk::Write);
}

#[test]
fn names_are_sanitised_and_kept_within_sixty_four_characters() {
    let plain = tool("search_issues", json!({}));
    assert_eq!(wire_names("github", std::slice::from_ref(&plain)), ["mcp__github__search_issues"]);
    let dotted = tool("search.issues", json!({}));
    assert_eq!(wire_names("github", &[dotted]), ["mcp__github__search_issues"]);
    let long = tool(&"x".repeat(80), json!({}));
    let name = wire_names("github", std::slice::from_ref(&long)).remove(0);
    assert_eq!(name.len(), 64);
    assert!(name.starts_with("mcp__github__xxx"));
    assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    let (_, hex) = name.rsplit_once('_').unwrap();
    assert_eq!(hex.len(), 6);
    assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(wire_names("github", &[long]), vec![name.clone()], "the suffix is deterministic");
    let a = tool(&format!("{}a", "y".repeat(70)), json!({}));
    let b = tool(&format!("{}b", "y".repeat(70)), json!({}));
    let names = wire_names("github", &[a, b]);
    assert_ne!(names[0], names[1], "two long names with one suffix stay distinct");
    assert!(names.iter().all(|n| n.len() == 64));
}

#[test]
fn colliding_names_get_distinct_wire_names_that_map_back_to_their_tools() {
    let listed = vec![tool("a.b", json!({"readOnlyHint": true})), tool("a_b", json!({"destructiveHint": true}))];
    let wires = wire_names("collide", &listed);
    assert_eq!(wires[0], "mcp__collide__a_b");
    assert_ne!(wires[0], wires[1]);
    assert!(wires[1].starts_with("mcp__collide__a_b_"), "{}", wires[1]);
    let servers = BTreeMap::from([("collide".to_string(), stdio_server("collide", &[]))]);
    publish_risks(&servers, &BTreeMap::from([("collide".to_string(), listed)]));
    let first = lookup(&wires[0]).expect("registered");
    assert_eq!((first.server.as_str(), first.tool.as_str(), first.risk), ("collide", "a.b", "read"));
    let second = lookup(&wires[1]).expect("registered");
    assert_eq!((second.tool.as_str(), second.risk), ("a_b", "irreversible"));
    forget_risks("collide");
    assert!(lookup(&wires[0]).is_none());
}

#[test]
fn the_laws_permit_an_enabled_read_tool_and_hold_an_irreversible_one() {
    let dir = std::env::temp_dir().join(format!("apz-mcp-laws-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let laws = crate::laws::Laws::load(&dir);
    let listed = vec![tool("search", json!({"readOnlyHint": true})), tool("wipe", json!({"destructiveHint": true}))];
    let wires = wire_names("lawtest", &listed);
    publish_risks(&BTreeMap::from([("lawtest".to_string(), stdio_server("lawtest", &[]))]), &BTreeMap::from([("lawtest".to_string(), listed)]));
    assert!(laws.permits("builder", &wires[0]), "an enabled read tool is permitted");
    assert!(!laws.permits("builder", &wires[1]), "C-004a holds an irreversible tool without an owner approval");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_fixture_binary_is_built_next_to_the_tests() {
    assert!(std::path::Path::new(&fixture()).exists(), "build the mcp_fixture binary: cargo build -p apid");
}

#[test]
fn tool_views_show_the_agent_name_the_annotation_risk_and_the_effective_risk() {
    let mut server = stdio_server("fixture", &[]);
    server.tools.risk.insert("wipe".into(), Risk::Write);
    let wipe = tool("wipe", json!({"destructiveHint": true}));
    assert_eq!(auto_risk(&wipe), Risk::Irreversible, "the annotations alone say irreversible");
    let rows = super::routes::tool_rows(&server, std::slice::from_ref(&wipe));
    assert_eq!(rows[0]["agent_name"], "mcp__fixture__wipe");
    assert_eq!(rows[0]["auto_risk"], "irreversible", "before the owner's override");
    assert_eq!(rows[0]["risk"], "write", "the effective risk takes the override");
    let plain = super::routes::tool_rows(&stdio_server("fixture", &[]), &[tool("echo", json!({"readOnlyHint": true}))]);
    assert_eq!((plain[0]["auto_risk"].as_str(), plain[0]["risk"].as_str()), (Some("read"), Some("read")));
}

/// A server that answers every POST with a 307 to `target`, until the test ends.
fn start_redirector(target: String) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let target = target.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; length];
                let _ = reader.read_exact(&mut body);
                let mut out = stream;
                let _ = write!(out, "HTTP/1.1 307 Temporary Redirect\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            });
        }
    });
    port
}

#[test]
fn an_allowed_redirect_hop_works_end_to_end_and_the_endpoint_moves_to_it() {
    let fixture = HttpFixture::start();
    let redirector = start_redirector(format!("http://127.0.0.1:{}/mcp", fixture.port));
    let policy = super::http::Policy { allow_private: true, blocklist: Vec::new() };
    let mut transport = super::http::HttpTransport::new(format!("http://127.0.0.1:{redirector}/mcp"), Vec::new(), Vec::new(), policy);
    let timeout = Duration::from_secs(10);
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}});
    let reply = transport.send(&init, timeout).unwrap();
    assert_eq!(reply[0]["result"]["serverInfo"]["name"], "fixture");
    transport.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}), timeout).unwrap();
    let listed = transport.send(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}), timeout).unwrap();
    assert_eq!(listed[0]["result"]["tools"].as_array().unwrap().len(), 3, "the first page, from the redirect target");
}

#[test]
fn a_redirect_to_loopback_is_refused_for_a_server_that_does_not_allow_private_hosts() {
    let fixture = HttpFixture::start();
    let redirector = start_redirector(format!("http://127.0.0.1:{}/mcp", fixture.port));
    let policy = super::http::Policy { allow_private: false, blocklist: Vec::new() };
    let mut transport = super::http::HttpTransport::new(format!("http://127.0.0.1:{redirector}/mcp"), Vec::new(), Vec::new(), policy);
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}});
    // The origin itself is loopback, so the first hop is refused before it is sent; that is the check.
    let err = transport.send(&init, Duration::from_secs(10)).unwrap_err();
    assert!(err.to_string().contains("refused"), "{err}");
}


#[test]
fn a_start_that_fails_before_launch_leaves_its_reason_on_the_server_view() {
    let docker = super::stdio::tests::MockDocker::new(Default::default(), |_: &[String]| Err(std::io::Error::from(std::io::ErrorKind::NotFound)));
    // Loopback binds without docker; the networks, which need docker, are what fails.
    let bind: super::egress::Bind = Box::new(|_: std::net::Ipv4Addr| std::net::TcpListener::bind("127.0.0.1:0"));
    let egress = super::egress::Egress::with_setup(super::egress::Setup {
        docker: Arc::new(docker),
        bind,
        port: super::egress::DEFAULT_PORT,
        state_dir: std::env::temp_dir(),
        relay_for: super::egress::relay_to(std::path::PathBuf::from("mcp_relay")),
        handshake: super::egress::Handshake::default(),
    });
    let runtime = Runtime::with_egress(Arc::new(PlainLauncher), egress);
    let vault = Vault::default();
    let server = stdio_server("nodocker", &[]);
    assert!(runtime.probe(&server, &ctx(&vault, owner("n0"), "n0", T0)).is_err());
    let summary = runtime.summary("nodocker");
    assert_eq!(summary.status, Status::Error);
    assert_eq!(summary.last_error.as_deref(), Some(super::stdio::NO_DOCKER));
}
