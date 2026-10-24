//! The cache-outage scenario end to end (PLAN 22.8): a small API calls its cache through a fault proxy that resets 30 % of
//! the connections. Without a retry the API fails its share of probes and the scenario fails the spec's 5 %; with a retry
//! the API answers every probe and the scenario passes. The run is the topology's own scenario, run by `run_all`.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use apz_topology::resilience::{Measured, Ran, judge};
use apz_topology::scenarios::{self, Config, ProxyConfig};
use apz_topology::slowserve::SlowServer;
use apz_topology::spec;

const TOPOLOGY: &str = r#"
name = "web-stack"

[[service]]
name = "api"
image = "api:1"

[[service]]
name = "cache"
image = "redis:7"

[[resilience]]
id = "cache-outage"
action = "fault"
target = "cache"
fault = { kind = "reset", percent = 30 }
at_secs = 0
for_secs = 1
recover_within_secs = 3
probe = { service = "api", path = "/items" }
expect = { max_error_percent = 5.0, max_p95_ms = 2000 }
"#;

/// A loopback address that is free now.
fn free_port() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

/// One GET to the cache: true when it answers 200 (a reset or a closed connection is a failure).
fn fetch(cache: SocketAddr) -> bool {
    let Ok(mut stream) = TcpStream::connect(cache) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    if stream
        .write_all(b"GET /key HTTP/1.1\r\nHost: cache\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut reply = Vec::new();
    let _ = stream.read_to_end(&mut reply);
    reply.starts_with(b"HTTP/1.1 200")
}

/// The API stand-in: answers 200 when one of its `attempts` calls to the cache succeeds, 503 otherwise.
fn api(cache: SocketAddr, attempts: u32) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            thread::spawn(move || {
                let mut head = [0u8; 2048];
                let _ = stream.read(&mut head);
                let ok = (0..attempts).any(|_| fetch(cache));
                let status = if ok {
                    "200 OK"
                } else {
                    "503 Service Unavailable"
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
            });
        }
    });
    addr
}

/// The scenario's measurements, with the API making `attempts` calls per probe.
fn outage(attempts: u32) -> (spec::Scenario, Measured) {
    let cache = SlowServer::start(0).unwrap();
    let proxy = free_port();
    let api_addr = api(proxy, attempts);
    let topology = spec::parse(TOPOLOGY).unwrap();
    let config = Config {
        proxies: BTreeMap::from([(
            "cache".to_string(),
            ProxyConfig {
                listen: proxy.to_string(),
                upstream: cache.addr().to_string(),
            },
        )]),
        urls: BTreeMap::from([("api".to_string(), format!("http://{api_addr}"))]),
        services: BTreeMap::new(),
        volumes: Vec::new(),
        device: "apz0".into(),
        collector: None,
    };
    let outcomes = scenarios::run_all(&topology, &config).expect("the scenario runs");
    let measured = match &outcomes[0].ran {
        Ran::Measured(measured) => measured.clone(),
        other => panic!("the scenario did not run: {other:?}"),
    };
    (topology.resilience[0].clone(), measured)
}

#[test]
fn a_missing_retry_fails_the_cache_outage_scenario() {
    let (scenario, measured) = outage(1);
    let judgment = judge(&scenario, &measured);
    assert!(!judgment.passed, "{judgment:?} {measured:?}");
    assert!(
        measured.error_percent > 5.0,
        "the reset share reaches the API: {measured:?}"
    );
    assert!(
        judgment.detail.contains("the spec allows 5.0 %"),
        "{}",
        judgment.detail
    );
    assert!(
        judgment.detail.contains("Not as the spec says"),
        "{}",
        judgment.detail
    );
}

#[test]
fn a_retry_passes_the_same_cache_outage_scenario() {
    let (scenario, measured) = outage(3);
    let judgment = judge(&scenario, &measured);
    assert!(judgment.passed, "{judgment:?} {measured:?}");
    assert_eq!(measured.error_percent, 0.0, "{measured:?}");
    assert!(measured.recovered_secs.is_some(), "{measured:?}");
}

#[test]
fn a_fault_on_a_dependency_with_no_proxy_is_refused_before_anything_starts() {
    let topology = spec::parse(TOPOLOGY).unwrap();
    let config = Config {
        proxies: BTreeMap::new(),
        urls: BTreeMap::new(),
        services: BTreeMap::new(),
        volumes: Vec::new(),
        device: "apz0".into(),
        collector: None,
    };
    let why = scenarios::run_all(&topology, &config).unwrap_err();
    assert!(
        why.contains("cache-outage") || why.contains("cache"),
        "{why}"
    );
}

#[test]
fn a_kill_the_guest_cannot_do_is_not_run_with_its_reason() {
    let topology = spec::parse(&TOPOLOGY.replace(
        "action = \"fault\"\ntarget = \"cache\"\nfault = { kind = \"reset\", percent = 30 }",
        "action = \"kill\"\ntarget = \"cache\"",
    ))
    .unwrap();
    let config = Config {
        proxies: BTreeMap::new(),
        urls: BTreeMap::from([("api".to_string(), "http://127.0.0.1:9".to_string())]),
        services: BTreeMap::new(),
        volumes: Vec::new(),
        device: "apz0".into(),
        collector: None,
    };
    let outcomes = scenarios::run_all(&topology, &config).unwrap();
    assert_eq!(
        outcomes[0].ran,
        Ran::NotRun("the guest knows no process of cache".into())
    );
}
