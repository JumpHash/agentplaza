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

use std::io::{BufRead as _, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use common::request;
use noded::hub::Hub;

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
        console_dir: root.join(noded::DEFAULT_CONSOLE_DIR),
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
        "/v1/tasks",
        Some(r#"{"goal":"ship the ledger v0"}"#),
    );
    assert_eq!(reply.status, 201, "{}", reply.body);
    let id = reply.json()["id"].as_str().expect("an id").to_string();
    assert_eq!(reply.json()["state"], "open", "{}", reply.body);
    let refused = request(
        port,
        "POST",
        &format!("/v1/tasks/{id}/claim"),
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
        &format!("/v1/tasks/{id}/claim"),
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
        &format!("/v1/tasks/{id}/claim"),
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
        &format!("/v1/tasks/{id}/finish"),
        Some(r#"{"outcome":"done","note":"ledger v0 is up"}"#),
    );
    assert_eq!(finished.status, 200, "{}", finished.body);
    let task = finished.json();
    assert_eq!(task["state"], "done", "{task}");
    assert_eq!(task["outcome"], "accepted", "{task}");
    assert_eq!(task["note"], "ledger v0 is up", "{task}");
    assert_eq!(
        request(port, "GET", &format!("/v1/tasks/{id}"), None).json()["state"],
        "done"
    );
    eprintln!("smoke: task {id} finished as done/accepted");

    // Stop the fleet, so the test leaves no VM behind.
    let killed = request(port, "DELETE", &format!("/v1/runs/{}", run.id()), None);
    assert_eq!(killed.status, 200, "{}", killed.body);
    assert_eq!(killed.json()["state"], "killed", "{}", killed.body);
    let _ = std::fs::remove_dir_all(&scratch);
}

/// The agora's one live test: a roled, budgeted task whose call a real
/// connected agent bids on and is awarded, over the real HTTP API, with a real
/// VM — and the `agora` frames its events stream carries while it happens.
///
/// Gated like the two above (`APZ_NODED_SMOKE=1`): it needs a hypervisor and
/// the guest images, and it stands up a one-role keepalive fleet so there is
/// an agent that is genuinely connected. What it proves that the agora's unit
/// tests cannot is the HTTP wiring of the accepted path — the bid and the
/// award through the register check, and the award binding the task exactly as
/// a claim does.
///
/// ```text
/// APZ_NODED_SMOKE=1 cargo test -p noded --test smoke -- --nocapture
/// ```
#[test]
fn an_agora_call_is_bid_on_and_awarded_by_a_connected_agent() {
    if std::env::var("APZ_NODED_SMOKE").as_deref() != Ok("1") {
        eprintln!("skipping: set APZ_NODED_SMOKE=1 (needs a hypervisor and the guest images)");
        return;
    }
    let root = repo_root();
    let scratch = std::env::temp_dir().join(format!("noded-agora-smoke-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("a scratch directory");
    let manifest = scratch.join("roles.json");
    std::fs::write(
        &manifest,
        r#"{"source":"noded agora live test","roles":{
            "builder":{"role":"builder","title":"The builder","permits":["work"],"toolsets":["echo"],"budget":1},
            "reviewer":{"role":"reviewer","title":"The reviewer","permits":["review"],"toolsets":["echo"],"budget":1}}}"#,
    )
    .expect("a two-role manifest");

    // Its own CIDs (well clear of the smoke tests' and the live fleet's) and
    // its own vsock directory, so nothing here shares a socket path. Two
    // roles, because the bid's role rule needs an agent that is connected and
    // is *not* the call's role.
    const CID: u64 = 400;
    const REVIEWER_CID: u64 = 401;
    let uds_dir = scratch.join("vsock");
    vmm::spawner::create_uds_dir(&uds_dir).expect("the fleet's socket directory");
    let config = noded::Config {
        port: 0,
        kernel: PathBuf::from(noded::DEFAULT_KERNEL),
        initrd: root.join(noded::DEFAULT_INITRD),
        console_dir: root.join(noded::DEFAULT_CONSOLE_DIR),
        uds_dir,
        workspace: None,
        agents_cid_base: CID,
        ..noded::Config::default()
    };
    let server = noded::Server::start(config).expect("the agora daemon should bind");
    let port = server.port();

    let id = request(
        port,
        "POST",
        "/v1/tasks",
        Some(r#"{"goal":"ship the agora v0","acceptance":"the board shows the award","budget":500,"role":"builder"}"#),
    );
    assert_eq!(id.status, 201, "{}", id.body);
    let id = id.json()["id"].as_str().expect("an id").to_string();
    eprintln!("smoke: created roled task {id} for role builder with a 500-credit budget");

    // The board already shows it as a call, before any agent is up.
    let board = request(port, "GET", "/v1/agora", None);
    assert_eq!(board.status, 200, "{}", board.body);
    assert_eq!(board.json()["calls"][0]["task"], id.as_str(), "{}", board.body);
    assert_eq!(board.json()["calls"][0]["state"], "open", "{}", board.body);

    // Nothing is connected until the fleet is up: a bid now is refused.
    let refused = request(
        port,
        "POST",
        &format!("/v1/tasks/{id}/bid"),
        Some(&format!(r#"{{"agent_cid":{CID},"price":400}}"#)),
    );
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(
        refused.json()["error"],
        format!("no live agent has guest CID {CID}")
    );
    eprintln!("smoke: a bid before the fleet was up was refused: {}", refused.body);

    // Stand the fleet up the way the daemon does at startup.
    let fleet = noded::fleet::Fleet::load(&manifest, CID, 128, 1).expect("a two-role manifest");
    let run = server
        .node()
        .registry()
        .create_agent_fleet(&fleet)
        .expect("the fleet is a run");
    assert_eq!(
        run.view(noded::registry::Detail::Full).state,
        "running",
        "the two-agent fleet is up"
    );

    // A connected agent of *another* role is refused by the call's own rule —
    // the invited check, as v0 makes it.
    let wrong_role = request(
        port,
        "POST",
        &format!("/v1/tasks/{id}/bid"),
        Some(&format!(r#"{{"agent_cid":{REVIEWER_CID},"price":100}}"#)),
    );
    assert_eq!(wrong_role.status, 409, "{}", wrong_role.body);
    assert_eq!(
        wrong_role.json()["error"],
        format!("the agent on guest CID {REVIEWER_CID} is role reviewer, not builder")
    );
    eprintln!(
        "smoke: a bid from the connected {REVIEWER_CID} (reviewer) was refused: {}",
        wrong_role.json()["error"]
    );

    // Watch the events stream across the whole call, so the `agora` frames are
    // captured as they happen rather than inferred.
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("the stream connects");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("a read timeout");
    stream
        .write_all(b"GET /v1/events HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .expect("the events request is sent");
    let mut stream = std::io::BufReader::new(stream);
    let mut head = String::new();
    stream.read_line(&mut head).expect("the stream head");
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    for _ in 0..3 {
        let mut header = String::new();
        stream.read_line(&mut header).expect("a header line");
    }
    let mut blank = String::new();
    stream.read_line(&mut blank).expect("the head ends");
    assert_eq!(blank, "\r\n");
    let mut socket = stream.into_inner();

    // The connected agent bids — twice, so the re-bid that replaces is proven
    // over the wire too — and is awarded at the price it last offered.
    let bid = request(
        port,
        "POST",
        &format!("/v1/tasks/{id}/bid"),
        Some(&format!(r#"{{"agent_cid":{CID},"price":400}}"#)),
    );
    assert_eq!(bid.status, 200, "{}", bid.body);
    assert_eq!(bid.json()["price"], 400, "{}", bid.body);
    assert_eq!(bid.json()["bidder"], "builder", "{}", bid.body);
    assert_eq!(bid.json()["agent_cid"], CID, "{}", bid.body);
    let rebid = request(
        port,
        "POST",
        &format!("/v1/tasks/{id}/bid"),
        Some(&format!(r#"{{"agent_cid":{CID},"price":300}}"#)),
    );
    assert_eq!(rebid.status, 200, "{}", rebid.body);
    assert_eq!(rebid.json()["price"], 300, "{}", rebid.body);
    eprintln!("smoke: agent {CID} bid 400 then re-bid 300 on call {id}");

    // A price over the call's budget is refused over the wire too.
    let over = request(
        port,
        "POST",
        &format!("/v1/tasks/{id}/bid"),
        Some(&format!(r#"{{"agent_cid":{CID},"price":501}}"#)),
    );
    assert_eq!(over.status, 409, "{}", over.body);
    assert_eq!(
        over.json()["error"],
        "bid price 501 exceeds max_price 500"
    );

    // The award binds the task through the ledger's own claim.
    let awarded = request(
        port,
        "POST",
        &format!("/v1/tasks/{id}/award"),
        Some(&format!(r#"{{"agent_cid":{CID}}}"#)),
    );
    assert_eq!(awarded.status, 200, "{}", awarded.body);
    let task = awarded.json();
    assert_eq!(task["state"], "claimed", "{task}");
    assert_eq!(task["assignee_id"], "builder", "{task}");
    assert_eq!(task["agent_cid"], CID, "{task}");
    eprintln!(
        "smoke: awarded call {id} to cid {CID}: task is {} and assigned to {}",
        task["state"], task["assignee_id"]
    );

    // A second award is refused in the award's own words.
    let again = request(
        port,
        "POST",
        &format!("/v1/tasks/{id}/award"),
        Some(&format!(r#"{{"agent_cid":{CID}}}"#)),
    );
    assert_eq!(again.status, 409, "{}", again.body);
    assert_eq!(
        again.json()["error"],
        "call t1 is already awarded to builder"
    );

    // The board now shows the call awarded, holding one bid (the re-bid
    // replaced the first) at the price the award was made at.
    let board = request(port, "GET", "/v1/agora", None).json();
    let call = &board["calls"][0];
    assert_eq!(call["state"], "awarded", "{board}");
    assert_eq!(call["max_price"], 500, "{board}");
    assert_eq!(call["bids"].as_array().expect("bids").len(), 1, "{board}");
    assert_eq!(call["bids"][0]["price"], 300, "{board}");
    assert_eq!(call["award"]["price"], 300, "{board}");
    assert_eq!(call["award"]["contractor"], "builder", "{board}");
    assert_eq!(call["award"]["escrow_entry"], 1, "{board}");
    eprintln!("smoke: board shows the awarded call: {}", board["calls"][0]);

    // The agent finishes the task it won, and the call keeps its award.
    let finished = request(
        port,
        "POST",
        &format!("/v1/tasks/{id}/finish"),
        Some(r#"{"outcome":"done","note":"agora v0 is up"}"#),
    );
    assert_eq!(finished.status, 200, "{}", finished.body);
    assert_eq!(finished.json()["state"], "done", "{}", finished.body);

    // The stream carried both bids and the award, as `agora` frames.
    let mut raw = Vec::new();
    let deadline = std::time::Instant::now();
    while std::time::Instant::now().duration_since(deadline) < Duration::from_secs(10) {
        let mut chunk = [0u8; 4096];
        match std::io::Read::read(&mut socket, &mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
        let text = String::from_utf8_lossy(&raw);
        if text.contains("\"kind\":\"award\"") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    for expected in [
        format!(
            "event: agora\ndata: {{\"task\":\"{id}\",\"kind\":\"bid\",\"agent_cid\":{CID},\"price\":400}}"
        ),
        format!(
            "event: agora\ndata: {{\"task\":\"{id}\",\"kind\":\"bid\",\"agent_cid\":{CID},\"price\":300}}"
        ),
        format!(
            "event: agora\ndata: {{\"task\":\"{id}\",\"kind\":\"award\",\"agent_cid\":{CID},\"price\":300}}"
        ),
    ] {
        assert!(text.contains(&expected), "the stream carried {expected:?}: {text}");
        eprintln!("smoke: stream carried {expected}");
    }

    let killed = request(port, "DELETE", &format!("/v1/runs/{}", run.id()), None);
    assert_eq!(killed.status, 200, "{}", killed.body);
    let _ = std::fs::remove_dir_all(&scratch);
}

/// A stub admin surface that answers each of `answers` in turn, handing back
/// the requests it read — the hub's own `/tell` shape, without the hub.
fn stub_admin(answers: Vec<String>) -> (u16, std::thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("the stub admin binds");
    let port = listener.local_addr().expect("a bound address").port();
    let handle = thread::spawn(move || {
        let mut seen = Vec::new();
        for raw in answers {
            let (mut stream, _peer) = listener.accept().expect("the daemon connects");
            stream
                .set_read_timeout(Some(Duration::from_secs(30)))
                .expect("a read timeout");
            let mut reader =
                std::io::BufReader::new(stream.try_clone().expect("the stub clones its socket"));
            let mut head = String::new();
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).expect("a header line") == 0 {
                    break;
                }
                if let Some((name, value)) = line.trim_end().split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    content_length = value.trim().parse().unwrap_or(0);
                }
                let done = line == "\r\n" || line == "\n";
                head.push_str(&line);
                if done {
                    break;
                }
            }
            let mut body = vec![0u8; content_length];
            if content_length > 0 {
                reader.read_exact(&mut body).expect("the request body");
            }
            stream.write_all(raw.as_bytes()).expect("the stub answers");
            stream.flush().expect("the answer is flushed");
            seen.push(format!("{head}{}", String::from_utf8_lossy(&body)));
        }
        seen
    });
    (port, handle)
}

/// A complete stub answer with its own length.
fn stub_answer(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// The social layer's one live test: the board, a read cursor, a conversation
/// and an award's note and contacts, over the real HTTP API, with two real
/// agents of two roles.
///
/// Gated like the three above (`APZ_NODED_SMOKE=1`): it needs a hypervisor and
/// the guest images. What it proves that the store's unit tests cannot is the
/// whole accepted path over HTTP — the register check on both CIDs, the unread
/// counts the register's agent list feeds, the delivery the hub
/// is asked for, and the award's note landing on the board.
///
/// ```text
/// APZ_NODED_SMOKE=1 cargo test -p noded --test smoke -- --nocapture
/// ```
#[test]
fn the_social_layer_is_live_over_two_connected_agents() {
    if std::env::var("APZ_NODED_SMOKE").as_deref() != Ok("1") {
        eprintln!("skipping: set APZ_NODED_SMOKE=1 (needs a hypervisor and the guest images)");
        return;
    }
    let root = repo_root();
    let scratch = std::env::temp_dir().join(format!("noded-social-smoke-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("a scratch directory");
    let manifest = scratch.join("roles.json");
    std::fs::write(
        &manifest,
        r#"{"source":"noded social live test","roles":{
            "builder":{"role":"builder","title":"The builder","permits":["work"],"toolsets":["echo"],"budget":1},
            "librarian":{"role":"librarian","title":"The librarian","permits":["read"],"toolsets":["echo"],"budget":1}}}"#,
    )
    .expect("a two-role manifest");

    // Two roles, its own CIDs (well clear of the other smoke tests' and the
    // live fleet's) and its own vsock directory, so nothing here shares a
    // socket path. The peer's session is a *stub* admin surface, so the
    // delivery's own half is asserted without touching the live `vsockd`: the
    // first answer is the peer taking the call, the second its own refusal.
    const BUILDER: u64 = 500;
    const LIBRARIAN: u64 = 501;
    let (admin_port, admin) = stub_admin(vec![
        stub_answer("200 OK", r#"{"ok":true,"output":"heard"}"#),
        stub_answer("200 OK", r#"{"ok":false,"output":"denied: no such tool"}"#),
    ]);
    let uds_dir = scratch.join("vsock");
    vmm::spawner::create_uds_dir(&uds_dir).expect("the fleet's socket directory");
    let config = noded::Config {
        port: 0,
        kernel: PathBuf::from(noded::DEFAULT_KERNEL),
        initrd: root.join(noded::DEFAULT_INITRD),
        console_dir: root.join(noded::DEFAULT_CONSOLE_DIR),
        uds_dir,
        workspace: None,
        // The hub is the stub above.
        hub: Some(Hub::new(admin_port)),
        agents_cid_base: BUILDER,
        ..noded::Config::default()
    };
    let server = noded::Server::start(config).expect("the social daemon should bind");
    let port = server.port();

    // Nothing is connected until the fleet is up: every social action that
    // needs an agent is refused by the register, in the register's words.
    let refused = request(port, "POST", "/v1/board", Some(&format!(
        r#"{{"agent_cid":{BUILDER},"subject":"hello","body":"anybody home?"}}"#
    )));
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(
        refused.json()["error"],
        format!("no live agent has guest CID {BUILDER}")
    );
    assert_eq!(
        request(port, "GET", &format!("/v1/agents/{BUILDER}/contacts"), None).status,
        404
    );
    eprintln!("smoke: a post before the fleet was up was refused: {}", refused.body);

    // Stand the fleet up the way the daemon does at startup.
    let fleet = noded::fleet::Fleet::load(&manifest, BUILDER, 128, 1).expect("a two-role manifest");
    let run = server
        .node()
        .registry()
        .create_agent_fleet(&fleet)
        .expect("the fleet is a run");
    assert_eq!(
        run.view(noded::registry::Detail::Full).state,
        "running",
        "the two-agent fleet is up"
    );
    let connected: Vec<u64> = server
        .node()
        .registry()
        .connected_agents()
        .iter()
        .map(|agent| agent.cid)
        .collect();
    assert_eq!(connected, vec![BUILDER, LIBRARIAN]);
    eprintln!("smoke: the fleet is up, agents on CIDs {connected:?}");

    // Watch the events stream across the whole exchange, so the `board` and
    // `conversation` frames are captured as they happen rather than inferred.
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("the stream connects");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("a read timeout");
    stream
        .write_all(b"GET /v1/events HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .expect("the events request is sent");
    let mut stream = std::io::BufReader::new(stream);
    let mut head = String::new();
    stream.read_line(&mut head).expect("the stream head");
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    for _ in 0..3 {
        let mut header = String::new();
        stream.read_line(&mut header).expect("a header line");
    }
    let mut blank = String::new();
    stream.read_line(&mut blank).expect("the head ends");
    assert_eq!(blank, "\r\n");
    let mut socket = stream.into_inner();

    // --- the board: a post from the builder, the librarian's cursor ---------
    let posted = request(
        port,
        "POST",
        "/v1/board",
        Some(&format!(
            r#"{{"agent_cid":{BUILDER},"subject":"hiring for the social layer","body":"the board is up — who wants it?","posted_by":"dispatchd"}}"#
        )),
    );
    assert_eq!(posted.status, 201, "{}", posted.body);
    let post = posted.json();
    assert_eq!(post["id"], "p1");
    assert_eq!(post["agent_cid"], BUILDER);
    assert_eq!(post["role"], "builder");
    assert_eq!(post["task"], serde_json::Value::Null);
    assert_eq!(post["posted_by"], "dispatchd");
    eprintln!("smoke: the builder posted {post}");

    let board = request(port, "GET", "/v1/board", None).json();
    assert_eq!(board["total"], 1, "{board}");
    assert_eq!(board["limit"], 256, "{board}");
    assert_eq!(board["truncated"], false, "{board}");
    assert_eq!(board["posts"][0]["id"], "p1", "{board}");
    assert_eq!(
        board["unread"],
        serde_json::json!([
            {"agent_cid": BUILDER, "count": 1},
            {"agent_cid": LIBRARIAN, "count": 1},
        ]),
        "every connected agent is one post behind: {board}"
    );

    let read = request(
        port,
        "POST",
        "/v1/board/read",
        Some(&format!(r#"{{"agent_cid":{LIBRARIAN},"up_to":"p1"}}"#)),
    );
    assert_eq!(read.status, 200, "{}", read.body);
    assert_eq!(read.json()["unread"], 0, "{}", read.body);
    let board = request(port, "GET", "/v1/board", None).json();
    assert_eq!(
        board["unread"],
        serde_json::json!([
            {"agent_cid": BUILDER, "count": 1},
            {"agent_cid": LIBRARIAN, "count": 0},
        ]),
        "the librarian's cursor dropped its count: {board}"
    );
    eprintln!("smoke: the librarian read up to p1; unread is now {}", board["unread"]);

    // Idempotent, and the future is refused.
    let replayed = request(
        port,
        "POST",
        "/v1/board/read",
        Some(&format!(r#"{{"agent_cid":{LIBRARIAN},"up_to":"p1"}}"#)),
    );
    assert_eq!(replayed.status, 200, "{}", replayed.body);
    assert_eq!(replayed.json(), read.json(), "a replay is the same answer");
    let future = request(
        port,
        "POST",
        "/v1/board/read",
        Some(&format!(r#"{{"agent_cid":{LIBRARIAN},"up_to":"p2"}}"#)),
    );
    assert_eq!(future.status, 409, "{}", future.body);
    assert_eq!(future.json()["error"], "up_to p2 is past the newest post p1");

    // --- the conversation: delivered, and a refusal recorded honestly -------
    let said = request(
        port,
        "POST",
        &format!("/v1/agents/{BUILDER}/say"),
        Some(&format!(
            r#"{{"to_cid":{LIBRARIAN},"text":"do you have the index?"}}"#
        )),
    );
    assert_eq!(said.status, 201, "{}", said.body);
    let exchange = said.json();
    assert_eq!(exchange["id"], "c1");
    assert_eq!(exchange["from_cid"], BUILDER);
    assert_eq!(exchange["to_cid"], LIBRARIAN);
    assert_eq!(exchange["from_role"], "builder");
    assert_eq!(exchange["delivered"], true, "{exchange}");
    assert_eq!(exchange["delivery"], serde_json::Value::Null, "{exchange}");
    eprintln!("smoke: the builder said {exchange}");
    let refused_delivery = request(
        port,
        "POST",
        &format!("/v1/agents/{BUILDER}/say"),
        Some(&format!(r#"{{"to_cid":{LIBRARIAN},"text":"again?"}}"#)),
    );
    assert_eq!(refused_delivery.status, 201, "{}", refused_delivery.body);
    let exchange = refused_delivery.json();
    assert_eq!(exchange["delivered"], false, "{exchange}");
    assert_eq!(exchange["delivery"], "denied: no such tool", "{exchange}");
    eprintln!("smoke: the peer refused the second call, recorded as {exchange}");

    // What the daemon asked the hub for: the contract's tool,
    // on the peer's CID, with the speaker and the words.
    let asked = admin.join().expect("the stub read every request");
    assert_eq!(asked.len(), 2);
    for request in &asked {
        assert!(request.starts_with(&format!("POST /v1/agents/{LIBRARIAN}/tell HTTP/1.1")), "{request}");
        assert!(request.contains(r#""name":"conversation""#), "{request}");
        assert!(request.contains(r#""from_role":"builder""#), "{request}");
    }
    assert!(asked[0].contains(r#""text":"do you have the index?""#), "{}", asked[0]);

    let record = request(port, "GET", "/v1/conversations", None).json();
    assert_eq!(record["total"], 2, "{record}");
    assert_eq!(record["active_secs"], 120, "{record}");
    assert_eq!(record["pairs"].as_array().expect("pairs").len(), 1, "{record}");
    assert_eq!(record["pairs"][0]["a"], BUILDER, "{record}");
    assert_eq!(record["pairs"][0]["b"], LIBRARIAN, "{record}");
    assert_eq!(record["pairs"][0]["active"], true, "they just spoke: {record}");
    assert_eq!(record["pairs"][0]["count"], 2, "{record}");
    let thread = request(
        port,
        "GET",
        &format!("/v1/conversations/{BUILDER}/{LIBRARIAN}"),
        None,
    )
    .json();
    assert_eq!(thread["total"], 2, "{thread}");
    assert_eq!(thread["active"], true, "{thread}");
    assert_eq!(thread["messages"][0]["id"], "c1", "oldest first: {thread}");
    assert_eq!(thread["messages"][1]["id"], "c2", "{thread}");
    eprintln!("smoke: the pair's thread is {}", thread["messages"]);

    // --- the award: the hiring note and the contacts both ways ---------------
    let created = request(
        port,
        "POST",
        "/v1/tasks",
        Some(r#"{"goal":"build the social layer","acceptance":"the board shows it","budget":500,"role":"builder"}"#),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let task = created.json()["id"].as_str().expect("an id").to_string();
    let bid = request(
        port,
        "POST",
        &format!("/v1/tasks/{task}/bid"),
        Some(&format!(r#"{{"agent_cid":{BUILDER},"price":300}}"#)),
    );
    assert_eq!(bid.status, 200, "{}", bid.body);
    // The librarian awards the call: it is the employer, and the awarder's own
    // CID is what lets the note be authored and the links be made.
    let awarded = request(
        port,
        "POST",
        &format!("/v1/tasks/{task}/award"),
        Some(&format!(r#"{{"agent_cid":{BUILDER},"awarder_cid":{LIBRARIAN}}}"#)),
    );
    assert_eq!(awarded.status, 200, "{}", awarded.body);
    assert_eq!(awarded.json()["state"], "claimed", "{}", awarded.body);
    eprintln!("smoke: awarded {task} to {BUILDER} as {LIBRARIAN}");

    let board = request(port, "GET", "/v1/board", None).json();
    assert_eq!(board["total"], 2, "{board}");
    let note = &board["posts"][0];
    assert_eq!(note["id"], "p2", "the note is the newest post: {board}");
    assert_eq!(note["subject"], format!("hiring for {task}"), "{note}");
    assert_eq!(note["task"], task, "{note}");
    assert_eq!(note["agent_cid"], LIBRARIAN, "the awarder speaks: {note}");
    assert_eq!(note["role"], "librarian", "{note}");
    assert_eq!(note["posted_by"], "librarian", "{note}");
    assert_eq!(
        note["body"],
        "builder — build the social layer (acceptance: the board shows it)",
        "{note}"
    );
    eprintln!("smoke: the award posted the hiring note {note}");

    // The links are both ways, with each side's own kind.
    let awarder = request(
        port,
        "GET",
        &format!("/v1/agents/{LIBRARIAN}/contacts"),
        None,
    )
    .json();
    assert_eq!(awarder["total"], 1, "{awarder}");
    assert_eq!(awarder["limit"], 256, "{awarder}");
    assert_eq!(awarder["truncated"], false, "{awarder}");
    assert_eq!(awarder["contacts"][0]["peer_cid"], BUILDER, "{awarder}");
    assert_eq!(awarder["contacts"][0]["role"], "builder", "{awarder}");
    assert_eq!(awarder["contacts"][0]["kind"], "hired", "{awarder}");
    let contractor = request(port, "GET", &format!("/v1/agents/{BUILDER}/contacts"), None).json();
    assert_eq!(contractor["contacts"][0]["peer_cid"], LIBRARIAN, "{contractor}");
    assert_eq!(contractor["contacts"][0]["role"], "librarian", "{contractor}");
    assert_eq!(contractor["contacts"][0]["kind"], "was-hired", "{contractor}");
    eprintln!("smoke: the contacts are {} and {}", awarder["contacts"][0], contractor["contacts"][0]);

    // A handshake that arrives after the hire does not downgrade it: the add is
    // idempotent and the kind a link was first recorded with stands.
    let handshake = request(
        port,
        "POST",
        &format!("/v1/agents/{BUILDER}/contacts"),
        Some(&format!(r#"{{"peer_cid":{LIBRARIAN}}}"#)),
    );
    assert_eq!(handshake.status, 200, "{}", handshake.body);
    assert_eq!(handshake.json()["kind"], "was-hired", "{}", handshake.body);
    assert_eq!(
        request(port, "GET", &format!("/v1/agents/{BUILDER}/contacts"), None).json()["total"],
        1,
        "no second link"
    );

    // --- the frames the stream carried --------------------------------------
    let mut raw = Vec::new();
    let deadline = Instant::now();
    while Instant::now().duration_since(deadline) < Duration::from_secs(10) {
        let mut chunk = [0u8; 4096];
        match socket.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
        if String::from_utf8_lossy(&raw).contains("event: conversation") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    for expected in [
        format!(
            "event: board\ndata: {{\"kind\":\"post\",\"post\":{{\"id\":\"p1\",\"agent_cid\":{BUILDER},\"role\":\"builder\",\"subject\":\"hiring for the social layer\",\"body\":\"the board is up — who wants it?\",\"task\":null,\"posted_by\":\"dispatchd\",\"created_at\":"
        ),
        format!(
            "event: board\ndata: {{\"kind\":\"read\",\"post\":null,\"agent_cid\":{LIBRARIAN},\"role\":\"librarian\",\"read_up_to\":\"p1\"}}"
        ),
        format!(
            "event: conversation\ndata: {{\"id\":\"c1\",\"from_cid\":{BUILDER},\"to_cid\":{LIBRARIAN},\"delivered\":true}}"
        ),
    ] {
        assert!(text.contains(&expected), "the stream carried {expected:?}: {text}");
        eprintln!("smoke: stream carried {expected}");
    }

    let killed = request(port, "DELETE", &format!("/v1/runs/{}", run.id()), None);
    assert_eq!(killed.status, 200, "{}", killed.body);
    let _ = std::fs::remove_dir_all(&scratch);
}
