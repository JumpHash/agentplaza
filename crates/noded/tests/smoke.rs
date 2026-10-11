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

/// The guest CID of the live task-ledger test's one-agent fleet. It is well
/// clear of the live fleet's CIDs (100..=120) so the test never competes for a
/// socket path with a running daemon.

/// The repository root. The daemon's default image paths are relative to the
/// working directory, and a test binary's working directory is the package
/// directory rather than the repository root, so they are resolved here.
///
/// It is canonicalized, so the guest-image paths handed to a run carry no `..`
/// component: the jailer refuses a bind source that names a parent directory
/// (`vmm::jail`), and `crates/noded/../..` is one — a run handed one fails to
/// boot with the jailer's own words rather than with the guest's.
fn repo_root() -> PathBuf {
    std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .expect("the repository root")
}

#[test]
fn a_churn_run_boots_heartbeats_and_finishes() {
    if std::env::var("APZ_NODED_SMOKE").as_deref() != Ok("1") {
        eprintln!("skipping: set APZ_NODED_SMOKE=1 (needs a hypervisor and the guest images)");
        return;
    }
    let root = repo_root();
    // The run's CIDs are allocated from the node's own run range now, and the
    // muxer binds `{uds_dir}/vm{cid}`: a private socket directory is what
    // keeps this test off a live daemon's paths (and off its CIDs), the same
    // reason the task-ledger test below uses one.
    let scratch = std::env::temp_dir().join(format!("noded-churn-smoke-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("a scratch directory");
    let uds_dir = scratch.join("vsock");
    vmm::spawner::create_uds_dir(&uds_dir).expect("the run's socket directory");
    let config = noded::Config {
        port: 0,
        kernel: PathBuf::from(noded::DEFAULT_KERNEL),
        initrd: root.join(noded::DEFAULT_INITRD),
        uds_dir: uds_dir.clone(),
        workspace: None,
        // The smoke run is the churn path, not the fleet: no initial agents.
        ..noded::Config::default()
    };
    let server = noded::Server::start(config).expect("the smoke daemon should bind");
    let port = server.port();

    let reply = request(
        port,
        "POST",
        "/v1/runs",
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

    // The request named no `vsock_cid`, so the node allocated one: the run's
    // VMs carry the run range's span, and each VM's vsock muxer is bound for
    // it (the guest's harness is the client that dials it).
    let cids: Vec<u64> = created["vms"]
        .as_array()
        .expect("an array")
        .iter()
        .map(|vm| {
            vm["guest_cid"]
                .as_u64()
                .unwrap_or_else(|| panic!("VM {} has no guest CID: {vm}", vm["id"]))
        })
        .collect();
    assert_eq!(
        cids,
        vec![
            noded::registry::RUN_CID_BASE,
            noded::registry::RUN_CID_BASE + 1
        ],
        "an allocated run draws the run range in order: {created}"
    );
    for cid in &cids {
        let socket = uds_dir.join(format!("vm{cid}"));
        let mut bound = socket.exists();
        for _ in 0..100 {
            if bound {
                break;
            }
            thread::sleep(Duration::from_millis(50));
            bound = socket.exists();
        }
        assert!(bound, "no muxer bound {} for CID {cid}", socket.display());
    }

    // A second run created while this one is live is given a *different* span:
    // two live runs never share a CID, so two VMs never race one muxer socket.
    let second = request(
        port,
        "POST",
        "/v1/runs",
        Some(r#"{"n":1,"vcpus":1,"memory_mib":128,"beats":null}"#),
    );
    assert_eq!(second.status, 201, "{}", second.body);
    let second = second.json();
    let second_cid = second["vms"][0]["guest_cid"]
        .as_u64()
        .expect("the second run's VM has an allocated CID");
    assert!(
        second_cid >= noded::registry::RUN_CID_BASE
            && second_cid <= noded::registry::RUN_CID_END,
        "the second run's CID is inside the run range: {second}"
    );
    assert!(
        !cids.contains(&second_cid),
        "two live runs were given the same CID: {cids:?} and {second_cid}"
    );
    let second_id = second["id"].as_str().expect("an id").to_string();
    let killed = request(port, "DELETE", &format!("/v1/runs/{second_id}"), None);
    assert_eq!(killed.status, 200, "{}", killed.body);
    assert_eq!(killed.json()["state"], "killed", "{}", killed.body);
    eprintln!("smoke: {second_id} ran one VM on CID {second_cid} and was killed");

    // A GET of a live run must not queue behind the run's own watcher: with
    // the watcher polling under the run's lock this waited for the run's whole
    // life (measured: 5.6 s for a 5 s run), which is what the watcher's
    // sleep-outside-the-lock shape exists to prevent.
    let asked = Instant::now();
    let live = request(port, "GET", &format!("/v1/runs/{id}"), None);
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
        &format!("/v1/runs/{id}/input"),
        Some(r#"{"data":"cpu","vm":2}"#),
    );
    assert_eq!(bad_vm.status, 400, "{}", bad_vm.body);
    let queued = request(
        port,
        "POST",
        &format!("/v1/runs/{id}/input"),
        Some(r#"{"data":"cpu","vm":0}"#),
    );
    assert_eq!(queued.status, 200, "{}", queued.body);
    assert_eq!(queued.json()["vms"][0]["state"], "running");

    let deadline = Instant::now() + RUN_DEADLINE;
    loop {
        let reply = request(port, "GET", &format!("/v1/runs/{id}"), None);
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
                // The guest's own kernel command line, as the kernel printed
                // it: the harness and the operator run's keepalive session,
                // with this VM's index - which is what makes the VM an agent
                // at all. (The CID is not on the command line by design: one
                // line is shared by every VM of a spawn and the device gives
                // each VM its own CID, which is why the run's view carries it.)
                assert!(
                    serial.contains(&format!(
                        "plaza.harness=1 plaza.keepalive=1 plaza.vm={}",
                        vm["id"]
                    )),
                    "VM {} did not boot with the harness: {serial}",
                    vm["id"]
                );
                // And the guest acted on it: `init` forked the harness.
                assert!(
                    serial.contains("plaza guest: agent harness running as pid"),
                    "VM {} never started the harness: {serial}",
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
