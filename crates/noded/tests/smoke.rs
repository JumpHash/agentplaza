//! The one live test: a churn run over the real HTTP API, with real VMs.
//!
//! Gated on `APZ_NODED_SMOKE=1`, because it needs a hypervisor (`/dev/kvm` on
//! Linux, Hypervisor.framework on Apple Silicon) and the guest images at the
//! daemon's default paths (`/boot/vmlinuz-linux` and
//! `target/guest/initrd.x86_64.gz`, relative to the repository root). Without
//! the variable it does nothing, so `cargo test -p noded` runs anywhere.
//!
//! ```text
//! APZ_NODED_SMOKE=1 cargo test -p noded --test smoke -- --nocapture
//! ```

mod common;

use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use common::request;

/// How long the churn run is given to finish: the guests heartbeat every five
/// seconds and reboot after two beats.
const RUN_DEADLINE: Duration = Duration::from_secs(180);

/// The repository root. The daemon's default image paths are relative to the
/// working directory, and a test binary's working directory is the package
/// directory rather than the repository root, so they are resolved here.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn a_churn_run_boots_heartbeats_and_finishes() {
    if std::env::var("APZ_NODED_SMOKE").as_deref() != Ok("1") {
        eprintln!("skipping: set APZ_NODED_SMOKE=1 (needs a hypervisor and the guest images)");
        return;
    }
    let root = repo_root();
    let config = noded::Config {
        port: 0,
        kernel: PathBuf::from(noded::DEFAULT_KERNEL),
        initrd: root.join(noded::DEFAULT_INITRD),
        console_dir: root.join(noded::DEFAULT_CONSOLE_DIR),
        uds_dir: root.join(noded::DEFAULT_UDS_DIR),
        workspace: None,
    };
    let server = noded::Server::start(config).expect("the smoke daemon should bind");
    let port = server.port();

    let reply = request(
        port,
        "POST",
        "/api/v0/runs",
        Some(r#"{"n":2,"vcpus":2,"memory_mib":512,"beats":2}"#),
    );
    assert_eq!(reply.status, 201, "{}", reply.body);
    let created = reply.json();
    let id = created["id"].as_str().expect("an id").to_string();
    assert_eq!(created["state"], "running", "{}", reply.body);
    assert_eq!(
        created["vms"].as_array().expect("an array").len(),
        2,
        "{}",
        reply.body
    );
    assert!(created["ended_at"].is_null(), "{}", reply.body);
    eprintln!("smoke: {id} is running");

    // A GET of a live run must not queue behind the run's own watcher: with
    // the watcher polling under the run's lock this waited for the run's whole
    // life (measured: 5.6 s for a 5 s run), which is what the watcher's
    // sleep-outside-the-lock shape exists to prevent.
    let asked = Instant::now();
    let live = request(port, "GET", &format!("/api/v0/runs/{id}"), None);
    let waited = asked.elapsed();
    assert_eq!(live.status, 200, "{}", live.body);
    assert_eq!(live.json()["state"], "running", "{}", live.body);
    assert!(
        waited < Duration::from_secs(2),
        "a GET of a live run waited {waited:?}"
    );

    // The live-run input paths: an out-of-range VM is a 400, and a VM that is
    // there takes a line (queued on its console, which is all this call
    // promises - whether the guest reads it is up to the guest).
    let bad_vm = request(
        port,
        "POST",
        &format!("/api/v0/runs/{id}/input"),
        Some(r#"{"data":"cpu","vm":2}"#),
    );
    assert_eq!(bad_vm.status, 400, "{}", bad_vm.body);
    let queued = request(
        port,
        "POST",
        &format!("/api/v0/runs/{id}/input"),
        Some(r#"{"data":"cpu","vm":0}"#),
    );
    assert_eq!(queued.status, 200, "{}", queued.body);
    assert_eq!(queued.json()["vms"][0]["state"], "running");

    let deadline = Instant::now() + RUN_DEADLINE;
    loop {
        let reply = request(port, "GET", &format!("/api/v0/runs/{id}"), None);
        assert_eq!(reply.status, 200, "{}", reply.body);
        let run = reply.json();
        assert_ne!(run["state"], "failed", "the run failed: {}", reply.body);
        if run["state"] == "done" {
            assert!(run["ended_at"].as_u64().expect("ended_at") > 0, "{run}");
            let vms = run["vms"].as_array().expect("an array");
            assert_eq!(vms.len(), 2, "{run}");
            for vm in vms {
                let serial = vm["serial"].as_str().expect("a serial capture");
                assert!(
                    serial.contains("heartbeat"),
                    "VM {} produced no heartbeat: {serial}",
                    vm["id"]
                );
                assert_eq!(vm["state"], "exited", "{vm}");
            }
            eprintln!("smoke: {id} finished as done");
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the run did not finish in {RUN_DEADLINE:?}: {}",
            reply.body
        );
        thread::sleep(Duration::from_millis(500));
    }
}
