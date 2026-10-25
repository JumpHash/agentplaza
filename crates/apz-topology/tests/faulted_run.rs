//! A topology's runner on this machine with real processes (PLAN 22.8, no root): `cache` and `api` are `apz-topo-echo`
//! processes on 127.0.0.1. `api` answers HTTP 200 only when its call to `cache` answers 200; the call goes by name through
//! `cache`'s fault proxy (an `apz-fault` process the runner started), so a partition of `cache` fails the probes of `api`.
//! The scenarios kill `api` by its pid file and restart it, and fill `cache`'s declared volume.
//! The kill is `kill -9` and the fill is a Unix file write, so the file runs on Unix only.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use apz_topology::plan::plan;
use apz_topology::resilience::Ran;
use apz_topology::run::{Addressing, Observed, start_observed};
use apz_topology::scenarios;
use apz_topology::spec;
use apz_topology::steps::{Commands, Guest, Processes, System};

const ECHO: &str = env!("CARGO_BIN_EXE_apz-topo-echo");
const FAULT: &str = env!("CARGO_BIN_EXE_apz-fault");

const TOPOLOGY: &str = r#"
name = "faulted"

[[service]]
name = "cache"
artifact = "echo"
ports = [7379]
volumes = [{ name = "cache-data", target = "/data", size_mib = 1 }]
env = { APZ_TOPO_HTTP = "1" }

[[service]]
name = "api"
artifact = "echo"
ports = [18080]
depends_on = ["cache"]
env = { APZ_TOPO_HTTP = "1", APZ_TOPO_CALL = "cache:7379" }

[[resilience]]
id = "cache-partition"
action = "fault"
target = "cache"
fault = { kind = "partition" }
at_secs = 0
for_secs = 1
recover_within_secs = 2
probe = { service = "api", path = "/" }
expect = { max_error_percent = 5.0 }

[[resilience]]
id = "api-kill"
action = "kill"
target = "api"
at_secs = 0
for_secs = 1
recover_within_secs = 4
probe = { service = "api", path = "/" }
expect = { max_error_percent = 100.0 }

[[resilience]]
id = "cache-disk-full"
action = "fill_disk"
target = "cache"
volume = "cache-data"
at_secs = 0
for_secs = 1
recover_within_secs = 1
probe = { service = "api", path = "/" }
expect = { max_error_percent = 100.0 }
"#;

#[test]
fn a_routed_dependency_is_partitioned_killed_restarted_and_filled_on_real_processes() {
    let topology = spec::parse(TOPOLOGY).unwrap();
    let artifacts = BTreeMap::from([("echo".to_string(), ECHO.to_string())]);
    let planned = plan(&topology, &artifacts).unwrap();
    let observed = Observed {
        fault_binary: Some(PathBuf::from(FAULT)),
        ..Observed::default()
    };
    let running = start_observed(&planned, Addressing::Local, &BTreeMap::new(), &observed).unwrap();
    running.wait_listening(Duration::from_secs(30)).unwrap();
    let urls = running.urls();
    assert_eq!(urls["api"], "http://127.0.0.1:18080");

    let mut guest = Guest {
        host: System {
            path: std::env::var("PATH").unwrap_or_default(),
        },
        runner: Commands,
        procs: running,
        device: planned.bridge.clone(),
    };
    let api_pid = std::fs::read_to_string(guest.procs.pid_file("api").unwrap()).unwrap();
    let fill_dir = Processes::volume(&guest.procs, "cache", "cache-data")
        .unwrap()
        .0;

    let outcomes = scenarios::run_with(&topology.resilience, &urls, &mut guest);
    let measured = |id: &str| match &outcomes.iter().find(|o| o.id == id).unwrap().ran {
        Ran::Measured(m) => m.clone(),
        other => panic!("{id} did not run: {other:?}"),
    };

    // The partition reached the call that goes through cache's proxy: api answered 503 through the window, and it
    // answered 200 again after the heal.
    let partition = measured("cache-partition");
    assert!(partition.error_percent > 50.0, "{partition:?}");
    assert!(partition.recovered_secs.is_some(), "{partition:?}");

    // api was killed by its pid file and started again: its probes failed while it was down, it came back, and its pid
    // changed.
    let kill = measured("api-kill");
    assert!(kill.error_percent > 50.0, "{kill:?}");
    assert!(kill.recovered_secs.is_some(), "{kill:?}");
    let api_pid_after = std::fs::read_to_string(guest.procs.pid_file("api").unwrap()).unwrap();
    assert_ne!(api_pid, api_pid_after, "a new process was started");

    // The disk fill wrote its file into cache's volume during the hold, and removed it after.
    let fill = measured("cache-disk-full");
    assert!(fill.requests > 0, "{fill:?}");
    assert!(!fill_dir.join(apz_topology::steps::FILL_FILE).exists());
}

#[test]
fn a_fault_without_the_proxy_binary_is_refused_before_anything_starts() {
    let topology = spec::parse(TOPOLOGY).unwrap();
    let artifacts = BTreeMap::from([("echo".to_string(), ECHO.to_string())]);
    let planned = plan(&topology, &artifacts).unwrap();
    let why = start_observed(
        &planned,
        Addressing::Local,
        &BTreeMap::new(),
        &Observed::default(),
    )
    .err()
    .expect("the run is refused");
    assert!(why.contains("apz-fault"), "{why}");
}
