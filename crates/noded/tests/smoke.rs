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
const AGENT_CID: u64 = 300;

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
        // The smoke run is the churn path, not the fleet: no initial agents.
        ..noded::Config::default()
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

/// The task ledger's one live test: a task claimed by a real agent's CID, over
/// the real HTTP API, with a real VM.
///
/// Gated like the churn run above (`APZ_NODED_SMOKE=1`): it needs a hypervisor
/// and the guest images, and it stands up a one-role keepalive fleet so there
/// is an agent that is genuinely connected.
///
/// ```text
/// APZ_NODED_SMOKE=1 cargo test -p noded --test smoke -- --nocapture
/// ```
#[test]
fn a_task_is_claimed_by_a_connected_agent_and_finished() {
    if std::env::var("APZ_NODED_SMOKE").as_deref() != Ok("1") {
        eprintln!("skipping: set APZ_NODED_SMOKE=1 (needs a hypervisor and the guest images)");
        return;
    }
    let root = repo_root();
    let scratch = std::env::temp_dir().join(format!("noded-tasks-smoke-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("a scratch directory");
    let manifest = scratch.join("roles.json");
    std::fs::write(
        &manifest,
        r#"{"source":"noded task-ledger live test","roles":{"builder":{"role":"builder",
            "title":"The builder","permits":["work"],"toolsets":["echo"],"budget":1}}}"#,
    )
    .expect("a one-role manifest");

    // The fleet gets its own vsock directory, so this test never shares a
    // socket path with a live daemon. The daemon's own startup creates it
    // `0700` (the muxer binds `{dir}/vm{cid}` inside it), so this does too.
    let uds_dir = scratch.join("vsock");
    vmm::spawner::create_uds_dir(&uds_dir).expect("the fleet's socket directory");
    let config = noded::Config {
        port: 0,
        kernel: PathBuf::from(noded::DEFAULT_KERNEL),
        initrd: root.join(noded::DEFAULT_INITRD),
        console_dir: root.join(noded::DEFAULT_CONSOLE_DIR),
        uds_dir,
        workspace: None,
        // The one role's guest CID.
        agents_cid_base: AGENT_CID,
        ..noded::Config::default()
    };
    let server = noded::Server::start(config).expect("the task-ledger daemon should bind");
    let port = server.port();

    // Nothing is connected until the fleet is up: a claim now is refused.
    let reply = request(
        port,
        "POST",
        "/api/v0/tasks",
        Some(r#"{"goal":"ship the ledger v0"}"#),
    );
    assert_eq!(reply.status, 201, "{}", reply.body);
    let id = reply.json()["id"].as_str().expect("an id").to_string();
    assert_eq!(reply.json()["state"], "open", "{}", reply.body);
    let refused = request(
        port,
        "POST",
        &format!("/api/v0/tasks/{id}/claim"),
        Some(&format!(r#"{{"agent_cid":{AGENT_CID}}}"#)),
    );
    assert_eq!(refused.status, 409, "{}", refused.body);
    eprintln!(
        "smoke: a claim before the fleet was up was refused: {}",
        refused.body
    );

    // Stand the fleet up the way the daemon does at startup: one keepalive VM
    // per role, through the same manifest reader.
    let fleet = noded::fleet::Fleet::load(&manifest, AGENT_CID, 128, 1).expect("a one-role manifest");
    let run = server
        .node()
        .registry()
        .create_agent_fleet(&fleet)
        .expect("the fleet is a run");
    assert_eq!(
        run.view(noded::registry::Detail::Full).state,
        "running",
        "the one-agent fleet is up"
    );
    eprintln!("smoke: fleet {} is running, one agent on CID {AGENT_CID}", run.id());

    // The agent is connected now, so the claim is accepted, and binds the task
    // to the agent's role.
    let claimed = request(
        port,
        "POST",
        &format!("/api/v0/tasks/{id}/claim"),
        Some(&format!(r#"{{"agent_cid":{AGENT_CID}}}"#)),
    );
    assert_eq!(claimed.status, 200, "{}", claimed.body);
    let task = claimed.json();
    assert_eq!(task["state"], "claimed", "{task}");
    assert_eq!(task["assignee_id"], "builder", "{task}");
    assert_eq!(task["agent_cid"], AGENT_CID, "{task}");
    eprintln!("smoke: claimed by role {} (cid {AGENT_CID})", task["assignee_id"]);

    // A second claim, even from the same live agent, is refused.
    let again = request(
        port,
        "POST",
        &format!("/api/v0/tasks/{id}/claim"),
        Some(&format!(r#"{{"agent_cid":{AGENT_CID}}}"#)),
    );
    assert_eq!(again.status, 409, "{}", again.body);
    assert_eq!(
        again.json()["error"],
        "a task may not go from claimed to claimed"
    );

    // The agent finishes: the task is terminal, and the register shows the
    // fleet still running - the task's life never touched it.
    let finished = request(
        port,
        "POST",
        &format!("/api/v0/tasks/{id}/finish"),
        Some(r#"{"outcome":"done","note":"ledger v0 is up"}"#),
    );
    assert_eq!(finished.status, 200, "{}", finished.body);
    let task = finished.json();
    assert_eq!(task["state"], "done", "{task}");
    assert_eq!(task["outcome"], "accepted", "{task}");
    assert_eq!(task["note"], "ledger v0 is up", "{task}");
    assert_eq!(
        request(port, "GET", &format!("/api/v0/tasks/{id}"), None).json()["state"],
        "done"
    );
    eprintln!("smoke: task {id} finished as done/accepted");

    // Stop the fleet, so the test leaves no VM behind.
    let killed = request(port, "DELETE", &format!("/api/v0/runs/{}", run.id()), None);
    assert_eq!(killed.status, 200, "{}", killed.body);
    assert_eq!(killed.json()["state"], "killed", "{}", killed.body);
    let _ = std::fs::remove_dir_all(&scratch);
}
