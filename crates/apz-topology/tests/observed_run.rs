//! A fake topology run, observed (PLAN 22.7): its services are stand-ins that write log lines, the real collector
//! process (`apz-topo-collect`) serves their OTLP and ships their lines, and a fake hub receiver stands in for the
//! hub's OTLP receiver. No container runtime and no VM: the processes are the real ones.
//! The stand-ins are `/bin/sh` scripts, so the file runs on Unix only.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::io::{BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Child;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use apz_topology::assert::{self, Evidence};
use apz_topology::collector::{self, Config, Request, read_request};
use apz_topology::observe::{self, LogLine};
use apz_topology::plan::plan;
use apz_topology::run::{Addressing, Observed, Standin, start_observed};
use apz_topology::spec::{self, Assertion};
use serde_json::{Value, json};

const TOPOLOGY: &str = "name = \"fake-run\"\n[[service]]\nname = \"api\"\nimage = \"api:1\"\n[[service]]\nname = \"db\"\nimage = \"db:1\"\n";
const TOKEN: &str = "run-token";
const WAIT: Duration = Duration::from_secs(20);

/// A fake hub receiver: it records every request it gets and answers 200. Returns its run's base URL and the requests.
fn receiver() -> (String, Arc<Mutex<Vec<Request>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let log = Arc::clone(&log);
            thread::spawn(move || {
                let mut stream = stream;
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                if let Ok(request) = read_request(&mut reader) {
                    log.lock().unwrap().push(request);
                }
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                );
            });
        }
    });
    (format!("http://{addr}/v1/test-lab/runs/q1/otlp"), seen)
}

/// A free loopback address (the port is free when it is asked for; the collector binds it next).
fn free_addr() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

/// POSTs `body` to `addr` at `path`, and returns the status the answer carries.
fn post(addr: &str, path: &str, body: &str) -> u16 {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    answer.split_whitespace().nth(1).unwrap().parse().unwrap()
}

/// A child process that is killed when the test ends.
struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("apz-observed-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn standin(script: &str) -> Standin {
    Standin {
        argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
        env: BTreeMap::new(),
    }
}

fn collector_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_apz-topo-collect"))
}

/// The OTLP traces request a service sends for one request it served.
fn traces_body() -> Value {
    json!({"resourceSpans": [{
        "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "api-1"}}]},
        "scopeSpans": [{"spans": [{
            "traceId": "0af7651916cd43dd8448eb211c80319c",
            "spanId": "b7ad6b7169203331",
            "name": "GET /items",
            "kind": 2,
            "startTimeUnixNano": "1700000000000000000",
            "endTimeUnixNano": "1700000000012000000",
            "status": {"code": 1},
        }]}],
    }]})
}

/// The OTLP metrics request a service sends: its cumulative request count.
fn metrics_body() -> Value {
    json!({"resourceMetrics": [{
        "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "api-1"}}]},
        "scopeMetrics": [{"metrics": [{"name": "http.server.requests", "sum": {
            "isMonotonic": true, "aggregationTemporality": 2,
            "dataPoints": [{"timeUnixNano": "1700000000000000000", "asInt": "4"}],
        }}]}],
    }]})
}

#[test]
fn a_seeded_error_line_is_shipped_and_fails_its_assertion_with_the_line_cited() {
    let logs = scratch("errors").join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    let (upstream, seen) = receiver();
    let listen = free_addr().to_string();

    // The lab VM's collector, a process, with its config.
    let config = Config {
        listen: listen.clone(),
        upstream,
        token: TOKEN.into(),
        logs_dir: logs.clone(),
        interval_ms: 100,
    };
    let config_path = logs.parent().unwrap().join("collect.json");
    std::fs::write(&config_path, serde_json::to_string(&config).unwrap()).unwrap();
    let _collector =
        Killed(collector::spawn(&collector_bin(), &config_path, &listen, WAIT).unwrap());

    // The topology's services: stand-ins that write their output, and export OTLP to the collector.
    let topology = spec::parse(TOPOLOGY).unwrap();
    let plan = plan(&topology, &BTreeMap::new()).unwrap();
    let standins = BTreeMap::from([
        (
            "api".to_string(),
            standin("echo 'INFO started'; echo 'ERROR seeded failure in the cache' >&2; sleep 60"),
        ),
        ("db".to_string(), standin("echo 'INFO ready'; sleep 60")),
    ]);
    let observed = Observed {
        logs_dir: Some(logs.clone()),
        otlp: Some(format!("http://{listen}")),
        fault_binary: None,
    };
    let running = start_observed(&plan, Addressing::Loopback, &standins, &observed).unwrap();
    assert_eq!(running.log_dir(), logs.as_path());

    // The api service exports its traces and metrics (JSON) to the collector, which relays them as they are.
    assert_eq!(post(&listen, "/v1/traces", &traces_body().to_string()), 200);
    assert_eq!(
        post(&listen, "/v1/metrics", &metrics_body().to_string()),
        200
    );

    // Its log lines reach the receiver, once each, with the token the run was given.
    let deadline = Instant::now() + WAIT;
    let shipped = loop {
        let found = seen.lock().unwrap().iter().any(|r| {
            r.path.ends_with("/logs") && String::from_utf8_lossy(&r.body).contains("seeded failure")
        });
        if found {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        thread::sleep(Duration::from_millis(50));
    };
    assert!(shipped, "the seeded ERROR line never reached the receiver");

    let requests = seen.lock().unwrap();
    let relayed = |suffix: &str| requests.iter().find(|r| r.path.ends_with(suffix)).cloned();
    let traces = relayed("/traces").expect("the traces were relayed");
    assert_eq!(
        serde_json::from_slice::<Value>(&traces.body).unwrap(),
        traces_body()
    );
    assert_eq!(traces.token, TOKEN);
    assert_eq!(relayed("/metrics").unwrap().token, TOKEN);

    let mut lines: Vec<LogLine> = Vec::new();
    for request in requests.iter().filter(|r| r.path.ends_with("/logs")) {
        lines.extend(observe::parse_logs(&serde_json::from_slice(&request.body).unwrap()).unwrap());
    }
    drop(requests);
    let seeded: Vec<&LogLine> = lines
        .iter()
        .filter(|l| l.body.contains("seeded failure"))
        .collect();
    assert_eq!(seeded.len(), 1, "a line is shipped once");
    assert_eq!(seeded[0].severity, "ERROR");
    assert_eq!(seeded[0].service, "api-1");

    // The runner kept the containers' output in their log files, where the collector read it.
    let on_disk = std::fs::read_to_string(running.log_dir().join("api-1.log")).unwrap();
    assert!(
        on_disk.contains("ERROR seeded failure in the cache"),
        "{on_disk}"
    );

    // The topology's assertion over what was observed fails, and cites the line.
    let evidence = Evidence {
        started_ms: 0,
        logs: lines,
        ..Evidence::default()
    };
    let verdicts = assert::check(
        &[Assertion::NoErrors {
            id: None,
            allow: Vec::new(),
        }],
        &evidence,
    );
    assert!(!verdicts[0].passed);
    assert_eq!(verdicts[0].cites.len(), 1);
    assert!(
        verdicts[0].cites[0].contains("seeded failure in the cache"),
        "{:?}",
        verdicts[0].cites
    );
}

#[test]
fn a_relay_the_hub_refuses_answers_502_so_the_exporter_retries() {
    let logs = scratch("refused");
    let listen = free_addr().to_string();
    let config = Config {
        listen: listen.clone(),
        // Nothing listens on port 1: the relay is refused.
        upstream: "http://127.0.0.1:1/v1/test-lab/runs/q1/otlp".into(),
        token: TOKEN.into(),
        logs_dir: logs.clone(),
        interval_ms: 100,
    };
    let config_path = logs.join("collect.json");
    std::fs::write(&config_path, serde_json::to_string(&config).unwrap()).unwrap();
    let _collector =
        Killed(collector::spawn(&collector_bin(), &config_path, &listen, WAIT).unwrap());
    assert_eq!(post(&listen, "/v1/traces", &traces_body().to_string()), 502);
    assert_eq!(post(&listen, "/v1/unknown", "{}"), 404);
    let _ = std::fs::remove_dir_all(&logs);
}
