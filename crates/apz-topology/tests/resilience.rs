//! The cache-outage scenario end to end (PLAN 22.8): a small API calls its cache through a fault proxy that resets 30 % of
//! the connections. Without a retry the API fails its share of probes and the scenario fails the spec's 5 %; with a retry
//! the API answers every probe and the scenario passes. The run is the topology's own scenario, run by `run_with` against
//! an in-process proxy.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use apz_topology::fault::{Fault, Proxy, Steer};
use apz_topology::resilience::{Measured, Ran, judge};
use apz_topology::scenarios;
use apz_topology::slowserve::SlowServer;
use apz_topology::spec;
use apz_topology::steps::{Commands, Guest, InProcess, System};

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

/// A guest whose only processes are the given proxies, by service.
fn guest(proxies: BTreeMap<String, Vec<Box<dyn Steer>>>) -> Guest<System, Commands, InProcess> {
    Guest {
        host: System {
            path: String::new(),
        },
        runner: Commands,
        procs: InProcess { proxies },
        device: "apz0".into(),
    }
}

/// The scenario's measurements, with the API making `attempts` calls per probe.
fn outage(attempts: u32) -> (spec::Scenario, Measured) {
    let cache = SlowServer::start(0).unwrap();
    let proxy_addr = free_port();
    let proxy = Proxy::start(proxy_addr, cache.addr(), Fault::Healthy).unwrap();
    let api_addr = api(proxy_addr, attempts);
    let topology = spec::parse(TOPOLOGY).unwrap();
    let urls = BTreeMap::from([("api".to_string(), format!("http://{api_addr}"))]);
    let mut guest = guest(BTreeMap::from([(
        "cache".to_string(),
        vec![Box::new(proxy.handle()) as Box<dyn Steer>],
    )]));
    let outcomes = scenarios::run_with(&topology.resilience, &urls, &mut guest);
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
fn a_fault_on_a_dependency_with_no_proxy_is_not_run_with_its_reason() {
    let topology = spec::parse(TOPOLOGY).unwrap();
    let urls = BTreeMap::from([("api".to_string(), "http://127.0.0.1:9".to_string())]);
    let mut guest = guest(BTreeMap::new());
    let outcomes = scenarios::run_with(&topology.resilience, &urls, &mut guest);
    assert_eq!(
        outcomes[0].ran,
        Ran::NotRun("no fault proxy in front of cache".into())
    );
}

#[test]
fn a_kill_with_no_pid_file_in_the_guest_is_not_run_with_its_reason() {
    let topology = spec::parse(&TOPOLOGY.replace(
        "action = \"fault\"\ntarget = \"cache\"\nfault = { kind = \"reset\", percent = 30 }",
        "action = \"kill\"\ntarget = \"cache\"",
    ))
    .unwrap();
    let urls = BTreeMap::from([("api".to_string(), "http://127.0.0.1:9".to_string())]);
    let mut guest = guest(BTreeMap::new());
    let outcomes = scenarios::run_with(&topology.resilience, &urls, &mut guest);
    assert_eq!(
        outcomes[0].ran,
        Ran::NotRun("the guest keeps no pid file for cache".into())
    );
}
