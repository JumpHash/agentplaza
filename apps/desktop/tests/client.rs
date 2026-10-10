//! The dashboard's HTTP client against a real socket.
//!
//! `crates/noded/tests/common/mod.rs` stubs the daemon's *server* side with
//! nothing but `std::net`; `tests/common/mod.rs` is the mirror image, and
//! these tests use it to assert the exact request the client sends and to
//! exercise its parsing over a real connection.
//!
//! One test talks to a live daemon when `APZ_DESKTOP_LIVE` names one (for
//! example `http://127.0.0.1:8899`), and does nothing otherwise.

mod common;

use std::time::Duration;

use common::{DETAIL, LIST, MACHINE, always, raw, replies, streams};

use agentplaza_desktop::api::{
    ApiError, Client, CreateRun, CreateTask, Endpoint, FinishOutcome, RunState, TaskOutcome,
    TaskState, VmState,
};
use agentplaza_desktop::events;
use agentplaza_desktop::exit::{ExitKind, exit_kind};

/// A client for a stub on `port`, with a short timeout so a stall fails fast.
fn client(port: u16) -> Client {
    Client::new(Endpoint::loopback_at("127.0.0.1", port).expect("loopback"))
        .with_timeout(Duration::from_secs(5))
}

/// The exact bytes the daemon parses out of a create request.
const CREATE_BODY: &str = r#"{"n":2,"vcpus":4,"memory_mib":256,"beats":3,"vsock_cid":50}"#;

#[test]
fn listing_runs_sends_the_documented_request_and_reads_the_answer() {
    let server = replies(vec![(200, LIST.to_string())]);
    let page = client(server.port).runs().expect("the list should parse");

    assert_eq!(page.runs.len(), 2);
    assert_eq!(page.runs[0].id, "r1");
    assert_eq!(page.runs[0].state, RunState::Running);
    assert_eq!(page.runs[0].live_vms(), 2);
    assert_eq!(page.runs[0].base_cid(), Some(50));
    assert_eq!(page.runs[1].state, RunState::Killed);
    assert_eq!(page.runs[1].vms[0].state, VmState::Killed);

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].target, "/api/v0/runs");
    assert_eq!(requests[0].body, "");
}

#[test]
fn the_host_header_names_the_endpoint() {
    let server = always(200, LIST);
    let port = server.port;
    client(port).runs().expect("the list should parse");
    let requests = server.requests();
    let host = format!("127.0.0.1:{port}");
    assert_eq!(requests[0].header("Host"), Some(host.as_str()));
}

#[test]
fn reading_one_run_follows_the_runs_path() {
    let server = replies(vec![(200, DETAIL.to_string())]);
    let run = client(server.port).run("r1").expect("the run should parse");

    assert_eq!(run.id, "r1");
    assert_eq!(run.vms[0].guest_cid, Some(50));
    assert_eq!(run.serial().lines().count(), 3);

    let requests = server.requests();
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].target, "/api/v0/runs/r1");
}

#[test]
fn a_role_field_parses_off_a_real_socket_and_a_missing_one_stays_none() {
    // The daemon's role fleet: every VM row carries a role.
    let server = replies(vec![(200, common::ROLE_DETAIL.to_string())]);
    let run = client(server.port).run("r1").expect("the run should parse");
    assert_eq!(run.vms[0].role.as_deref(), Some("builder"));
    assert_eq!(run.vms[1].role.as_deref(), Some("reviewer"));
    assert_eq!(run.vms[2].role.as_deref(), Some("legal-gov-prosecutor"));
    assert_eq!(run.role(), None, "three roles are a count, not one name");

    // An older daemon's body: no `role` key anywhere, and not a parse error.
    let server = replies(vec![(200, DETAIL.to_string())]);
    let run = client(server.port).run("r1").expect("the run should parse");
    assert_eq!(run.vms[0].role, None);
    assert!(run.roles().is_empty());
    assert_eq!(run.role(), None);
    // Explicitly null is the same as absent.
    let server = replies(vec![(
        200,
        r#"{"id":"r1","state":"running","vcpus":1,"memory_mib":128,"created_at":1,
            "ended_at":null,"vms":[{"id":0,"role":null,"state":"running"}]}"#
            .to_string(),
    )]);
    let run = client(server.port).run("r1").expect("the run should parse");
    assert_eq!(run.vms[0].role, None);
}

#[test]
fn killing_a_run_deletes_the_runs_path() {
    let server = replies(vec![(200, DETAIL.to_string())]);
    let run = client(server.port).kill("r1").expect("the answer should parse");
    assert_eq!(run.id, "r1");

    let requests = server.requests();
    assert_eq!(requests[0].method, "DELETE");
    assert_eq!(requests[0].target, "/api/v0/runs/r1");
}

#[test]
fn reading_the_machine_follows_the_machines_path() {
    let server = always(200, MACHINE);
    let machine = client(server.port)
        .machine()
        .expect("the machine should parse");

    assert_eq!(machine.name, "plaza-node");
    assert_eq!(machine.arch, "x86_64");
    assert_eq!(machine.vsock_mode, "uds-muxer");
    assert_eq!(machine.cpus, 24);
    assert_eq!(machine.memory_mib, 128_320);
    assert_eq!(machine.started_at_ms, 1_760_000_000_000);

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].target, "/api/v0/machine");
    assert_eq!(requests[0].body, "");
}

#[test]
fn a_daemon_without_the_machine_endpoint_is_a_recognizable_404() {
    let server = always(404, r#"{"error":"no such endpoint"}"#);
    let error = client(server.port)
        .machine()
        .expect_err("an older daemon has no machine endpoint");
    assert!(error.is_not_found());
    assert_eq!(error.to_string(), "404: no such endpoint");
}

#[test]
fn reading_the_hubs_status_follows_the_status_path_and_reads_the_frozen_shape() {
    let server = common::hub(common::HUB_STATUS);
    let status = client(server.port)
        .hub_status()
        .expect("the status should parse");

    // Field for field, off a real socket: the frozen contract's own body.
    assert_eq!(status.node.cids, vec![10, 11, 12, 13, 14, 15]);
    assert_eq!(status.node.port, 5_000);
    assert_eq!(status.node.uptime_s, 673);
    assert_eq!(status.node.max_sessions, 64);
    assert_eq!(status.node.keepalive_idle_s, 300);
    assert_eq!(status.sessions_active, 1);
    let agent = &status.agents[0];
    assert_eq!(agent.cid, 10);
    assert_eq!(agent.role, "builder");
    assert_eq!(agent.connected_at_ms, 1_760_000_000_000);
    assert_eq!(agent.last_heartbeat_ms, 1_760_000_009_000);
    assert_eq!(agent.calls, 7);
    assert_eq!(status.agent(10).map(|agent| agent.role.as_str()), Some("builder"));
    assert_eq!(status.agent(11), None);

    let requests = server.requests();
    assert_eq!(requests.len(), 1, "one request, on the status path");
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].target, "/status");
    assert_eq!(requests[0].body, "");
}

#[test]
fn a_hub_with_nobody_connected_reads_as_an_answer_not_a_failure() {
    let server = common::hub(common::HUB_STATUS_EMPTY);
    let status = client(server.port)
        .hub_status()
        .expect("an empty registry is still an answer");
    assert!(status.agents.is_empty());
    assert_eq!(status.sessions_active, 0);
    assert_eq!(status.node.cids.len(), 6);
}

#[test]
fn a_hub_body_the_client_does_not_know_parses_and_a_missing_field_is_named() {
    // A newer hub may add fields at any level: the known shape still parses.
    let server = common::hub(
        r#"{"node":{"cids":[10],"port":5000,"uptime_s":7,"max_sessions":64,
            "keepalive_idle_s":300,"revision":"v2"},
            "agents":[{"cid":10,"role":"builder","connected_at_ms":1,
            "last_heartbeat_ms":2,"calls":3,"latency_ms":4}],
            "sessions_active":1,"future":true}"#,
    );
    let status = client(server.port).hub_status().expect("known fields parse");
    assert_eq!(status.agents[0].calls, 3);
    assert_eq!(status.node.port, 5_000);

    // A field the contract freezes and the body does not carry is a malformed
    // answer whose message names it.
    let server = common::hub(r#"{"node":{"cids":[10]},"agents":[],"sessions_active":0}"#);
    let error = client(server.port)
        .hub_status()
        .expect_err("a body missing a field is not the frozen shape");
    assert!(matches!(error, ApiError::Malformed(_)), "{error:?}");
    let message = error.to_string();
    assert!(message.contains("missing field"), "{message}");
    assert!(message.contains("port"), "the error names the field: {message}");
}

#[test]
fn a_daemon_without_a_status_surface_is_a_recognizable_404() {
    // `noded` has no `/status`: pointed at it, the client says what the
    // daemon said rather than inventing a hub.
    let server = always(404, r#"{"error":"no such endpoint"}"#);
    let error = client(server.port)
        .hub_status()
        .expect_err("the daemon has no status surface");
    assert!(error.is_not_found());
    assert_eq!(error.to_string(), "404: no such endpoint");
}

#[test]
fn a_hub_url_off_the_loopback_is_refused_before_any_socket_is_opened() {
    for url in [
        "example.com:19080",
        "10.0.0.1:19080",
        "192.168.1.10:19080",
        "127.0.0.1.evil.tld:19080",
        "api.127.0.0.1:19080",
        "localhost.evil.tld:19080",
        "127.0.0.1.:19080",
        "LOCALHOST:19080",
        "Localhost:19080",
        "http://[::1]:19080",
        "http://0.0.0.0:19080",
        "https://127.0.0.1:19080",
        "127.0.0.1:0",
        "http://127.0.0.1:19080/status",
        "http://127.0.0.1:19080?q=1",
    ] {
        let error = Endpoint::hub_url(url).expect_err("must be refused");
        assert!(
            matches!(error, ApiError::Config(_)),
            "{url} should be a configuration error, got {error:?}"
        );
    }
    // The exact loopback names, and only those, are the hub's.
    assert_eq!(
        Endpoint::hub_url("localhost:19080").expect("localhost is loopback").host(),
        "127.0.0.1"
    );
    assert_eq!(Endpoint::hub_url("127.0.0.1:19080").expect("loopback").port(), 19_080);
    // And the port is required: there is no default status port to fall to.
    let error = Endpoint::hub_url("127.0.0.1").expect_err("no port to read");
    assert!(error.to_string().contains("--status-port"), "{error}");
}

#[test]
fn an_unreachable_hub_is_a_connect_error_rather_than_a_panic() {
    // Nothing listens on port 1 of the loopback: the hub is a second process,
    // and its absence is its own error, not the daemon's.
    let error = Client::new(Endpoint::hub_url("127.0.0.1:1").expect("loopback"))
        .hub_status()
        .expect_err("nothing is listening");
    assert!(error.is_unreachable(), "{error:?}");
    assert!(error.to_string().starts_with("cannot reach "), "{error}");
}

/// The exact bytes the daemon parses out of a console write.
const INPUT_BODY: &str = r#"{"data":"uname\n","vm":1}"#;

#[test]
fn writing_to_a_console_posts_exactly_the_daemons_input_body() {
    let server = replies(vec![(200, DETAIL.to_string())]);
    let run = client(server.port)
        .input("r1", 1, "uname\n")
        .expect("the answer should parse");
    assert_eq!(run.id, "r1");
    assert_eq!(run.serial().lines().count(), 3);

    let requests = server.requests();
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].target, "/api/v0/runs/r1/input");
    assert_eq!(requests[0].header("Content-Type"), Some("application/json"));
    assert_eq!(
        requests[0].header("Content-Length"),
        Some(INPUT_BODY.len().to_string().as_str())
    );
    assert_eq!(
        requests[0].body, INPUT_BODY,
        "the body is the daemon's own field set, byte for byte"
    );
}

#[test]
fn a_refused_console_write_carries_the_daemons_own_words() {
    let server = replies(vec![(
        409,
        r#"{"error":"the run is killed: console input needs a running run"}"#.to_string(),
    )]);
    let error = client(server.port)
        .input("r1", 0, "uname\n")
        .expect_err("a killed run refuses input");
    assert_eq!(error.status(), Some(409));
    assert_eq!(
        error.to_string(),
        "409: the run is killed: console input needs a running run"
    );
}

#[test]
fn spawning_posts_exactly_the_daemons_create_body() {
    let server = replies(vec![(201, DETAIL.to_string())]);
    let create = CreateRun {
        n: 2,
        vcpus: 4,
        memory_mib: 256,
        beats: Some(3),
        vsock_cid: Some(50),
    };
    let run = client(server.port).spawn(&create).expect("the answer should parse");
    assert_eq!(run.id, "r1");

    let requests = server.requests();
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].target, "/api/v0/runs");
    assert_eq!(requests[0].header("Content-Type"), Some("application/json"));
    assert_eq!(
        requests[0].header("Content-Length"),
        Some(CREATE_BODY.len().to_string().as_str())
    );
    assert_eq!(
        requests[0].body, CREATE_BODY,
        "the body is the daemon's own field set, byte for byte"
    );
}

#[test]
fn a_refusal_carries_the_daemons_own_words() {
    let server = replies(vec![(400, r#"{"error":"vcpus must be between 1 and 64"}"#.to_string())]);
    let error = client(server.port)
        .spawn(&CreateRun {
            n: 1,
            vcpus: 65,
            memory_mib: 128,
            beats: None,
            vsock_cid: None,
        })
        .expect_err("the daemon refuses");

    assert_eq!(error.status(), Some(400));
    assert_eq!(error.to_string(), "400: vcpus must be between 1 and 64");
}

#[test]
fn a_missing_run_is_a_recognizable_404() {
    let server = replies(vec![(404, r#"{"error":"no run with id r9"}"#.to_string())]);
    let error = client(server.port).run("r9").expect_err("there is no such run");
    assert!(error.is_not_found());
    assert_eq!(error.to_string(), "404: no run with id r9");
}

#[test]
fn an_unreachable_daemon_is_a_connect_error_rather_than_a_panic() {
    // Nothing listens on port 1 of the loopback: the connect is refused
    // immediately, which is what a stopped daemon looks like.
    let error = Client::new(Endpoint::loopback_at("127.0.0.1", 1).expect("loopback"))
        .runs()
        .expect_err("nothing is listening");
    assert!(error.is_unreachable(), "{error:?}");
    assert!(error.to_string().starts_with("cannot reach "), "{error}");
}

#[test]
fn a_body_shorter_than_content_length_is_malformed() {
    let server = raw(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{\"runs\":",
    );
    let error = client(server.port).runs().expect_err("the body is short");
    assert!(
        matches!(error, ApiError::Malformed(_)),
        "a truncated answer is malformed, not a panic: {error:?}"
    );
}

#[test]
fn a_response_without_a_length_is_malformed() {
    let server = raw("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n");
    let error = client(server.port).runs().expect_err("there is no body to read");
    assert!(matches!(error, ApiError::Malformed(_)), "{error:?}");
    assert!(error.to_string().contains("Content-Length"), "{error}");
}

#[test]
fn a_non_loopback_endpoint_is_refused_before_any_socket_is_opened() {
    for host in ["example.com", "10.0.0.1", "127.0.0.1.evil.tld", "::1"] {
        let error = Endpoint::loopback_at(host, 8899).expect_err("must be refused");
        assert!(matches!(error, ApiError::Config(_)), "{host}: {error:?}");
    }
    assert_eq!(
        Endpoint::loopback_at("localhost", 8899)
            .expect("localhost is loopback")
            .host(),
        "127.0.0.1"
    );
}

#[test]
fn the_stream_client_reads_the_daemons_frames_off_a_real_socket() {
    let server = streams(vec![
        "event: run\ndata: {\"id\":\"r1\",\"state\":\"starting\"}\n\n\
         : heartbeat\n\n\
         event: vm\ndata: {\"run\":\"r1\",\"vm\":0,\"state\":\"running\"}\n\n\
         event: serial\ndata: {\"run\":\"r1\",\"vm\":0,\"offset\":13,\"text\":\"$ uname\\n\"}\n\n\
         event: wat\ndata: {\"new\":true}\n\n",
    ]);
    let endpoint = Endpoint::loopback_at("127.0.0.1", server.port).expect("loopback");

    let mut seen = Vec::new();
    let delivered = events::follow(&endpoint, &mut |event| {
        seen.push(event);
        seen.len() < 3
    });
    assert_eq!(delivered, 3, "three events, and the heartbeat is not one");

    assert_eq!(
        seen,
        vec![
            events::Event::Run {
                id: "r1".to_string(),
                state: RunState::Starting,
            },
            events::Event::Vm {
                run: "r1".to_string(),
                vm: 0,
                state: VmState::Running,
            },
            events::Event::Serial {
                run: "r1".to_string(),
                vm: 0,
                offset: 13,
                text: "$ uname\n".to_string(),
            },
        ]
    );

    // The request the console sent is the one the daemon's endpoint wants.
    let requests = server.requests();
    let streamed = requests
        .iter()
        .find(|request| request.target == "/api/v0/events")
        .expect("the console asked for the stream");
    assert_eq!(streamed.method, "GET");
    assert_eq!(streamed.header("Host"), Some("localhost"));
}

#[test]
fn the_stream_client_reconnects_when_the_daemon_closes_the_stream() {
    // One run event per connection: the caller stops after the second, so the
    // second has to have come off a reconnected stream.
    let server = streams(vec![
        "event: run\ndata: {\"id\":\"r1\",\"state\":\"running\"}\n\n",
        "event: run\ndata: {\"id\":\"r1\",\"state\":\"done\"}\n\n",
    ]);
    let endpoint = Endpoint::loopback_at("127.0.0.1", server.port).expect("loopback");
    let started = std::time::Instant::now();

    let mut seen = Vec::new();
    let delivered = events::follow(&endpoint, &mut |event| {
        seen.push(event);
        seen.len() < 2
    });
    assert_eq!(delivered, 2);
    assert_eq!(
        seen,
        vec![
            events::Event::Run {
                id: "r1".to_string(),
                state: RunState::Running,
            },
            events::Event::Run {
                id: "r1".to_string(),
                state: RunState::Done,
            },
        ],
        "the second event came off a second connection"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(events::BACKOFF_MIN_MS),
        "the reconnect waits before it asks again"
    );
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.target == "/api/v0/events")
            .count(),
        2,
        "two connections"
    );
}

/// The exact bytes the daemon parses out of a task create.
const CREATE_TASK_BODY: &str =
    r#"{"goal":"write the plaza handbooks","acceptance":null,"budget":null,"role":null}"#;

#[test]
fn listing_tasks_sends_the_documented_request_and_reads_the_ledger() {
    let server = common::ledger(common::TASKS, common::TASK);
    let page = client(server.port).tasks().expect("the ledger should parse");

    assert_eq!(page.tasks.len(), 3, "oldest first, as the daemon lists them");
    let claimed = &page.tasks[0];
    assert_eq!(claimed.id, "t1");
    assert_eq!(claimed.goal, "write the plaza handbooks");
    assert_eq!(claimed.acceptance.as_deref(), Some("all 21 present"));
    assert_eq!(claimed.budget, 2_000);
    assert_eq!(claimed.role.as_deref(), Some("librarian"));
    assert_eq!(claimed.state, TaskState::Claimed);
    assert!(claimed.state.is_finishable());
    assert_eq!(claimed.outcome.get(), None, "a living task has no outcome");
    assert_eq!(claimed.assignee_id.as_deref(), Some("librarian"));
    assert_eq!(claimed.verifier_id.get(), None, "v0 records no verifier");
    assert_eq!(claimed.agent_cid.copied(), Some(112));
    assert_eq!(claimed.note.get(), None);

    let open = &page.tasks[1];
    assert_eq!(open.state, TaskState::Open);
    assert!(open.state.is_claimable());
    assert_eq!(open.role.get(), None);

    let ended = &page.tasks[2];
    assert_eq!(ended.state, TaskState::Done);
    assert_eq!(ended.outcome.get(), Some(&TaskOutcome::Accepted));
    assert_eq!(ended.note.as_deref(), Some("every negative covered"));

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].target, "/api/v0/tasks");
    assert_eq!(requests[0].body, "");
}

#[test]
fn reading_one_task_follows_the_tasks_path() {
    let server = common::ledger(common::TASKS, common::TASK);
    let task = client(server.port).task("t1").expect("the task should parse");
    assert_eq!(task.id, "t1");
    assert_eq!(task.state, TaskState::Claimed);
    assert_eq!(task.agent_cid.copied(), Some(112));

    let requests = server.requests();
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].target, "/api/v0/tasks/t1");
}

#[test]
fn creating_a_task_posts_exactly_the_daemons_create_body() {
    let server = common::ledger(common::TASKS, common::TASK);
    let task = client(server.port)
        .create_task(&CreateTask {
            goal: "write the plaza handbooks".to_string(),
            acceptance: None,
            budget: None,
            role: None,
        })
        .expect("the answer should parse");
    assert_eq!(task.id, "t1");

    let requests = server.requests();
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].target, "/api/v0/tasks");
    assert_eq!(requests[0].body, CREATE_TASK_BODY);
    assert_eq!(
        requests[0].header("Content-Type"),
        Some("application/json"),
        "the daemon parses the body as JSON"
    );
}

#[test]
fn claiming_a_task_posts_the_agent_cid_and_a_409_carries_the_daemons_words() {
    // The daemon answers a claim with the task it bound.
    let server = common::ledger(common::TASKS, common::TASK);
    let task = client(server.port)
        .claim_task("t1", 112)
        .expect("the answer should parse");
    assert_eq!(task.state, TaskState::Claimed);
    let requests = server.requests();
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].target, "/api/v0/tasks/t1/claim");
    assert_eq!(requests[0].body, r#"{"agent_cid":112}"#);

    // A CID no live agent holds is refused 409, and the client carries the
    // daemon's own message through unchanged.
    let server = common::always(
        409,
        r#"{"error":"no live agent has guest CID 4"}"#,
    );
    let error = client(server.port)
        .claim_task("t1", 4)
        .expect_err("a CID no live agent holds is refused");
    assert_eq!(error.status(), Some(409));
    assert_eq!(error.to_string(), "409: no live agent has guest CID 4");
}

#[test]
fn finishing_posts_the_outcome_and_note_and_withdrawing_deletes_the_path() {
    let server = common::ledger(common::TASKS, common::TASK);
    let task = client(server.port)
        .finish_task(
            "t1",
            FinishOutcome::Done,
            Some("21 handbooks committed"),
        )
        .expect("the answer should parse");
    assert_eq!(task.id, "t1");

    // The withdrawal is a DELETE on the task's own path.
    let server2 = common::ledger(common::TASKS, common::TASK);
    client(server2.port)
        .withdraw_task("t1")
        .expect("the answer should parse");

    let requests = server.requests();
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].target, "/api/v0/tasks/t1/finish");
    assert_eq!(
        requests[0].body,
        r#"{"outcome":"done","note":"21 handbooks committed"}"#
    );

    let requests = server2.requests();
    assert_eq!(requests[0].method, "DELETE");
    assert_eq!(requests[0].target, "/api/v0/tasks/t1");
    assert_eq!(requests[0].body, "");
}

#[test]
fn a_missing_task_and_a_task_id_that_is_not_plain_are_both_refused() {
    let server = common::always(404, r#"{"error":"no task with id t9"}"#);
    let error = client(server.port)
        .task("t9")
        .expect_err("a missing task is a 404");
    assert!(error.is_not_found());
    assert_eq!(error.to_string(), "404: no task with id t9");

    // A path that could change the request never reaches a socket.
    let error = Client::default()
        .task("t1/../t2")
        .expect_err("an id with a path in it is refused");
    assert!(matches!(error, ApiError::Config(_)), "{error:?}");
}

/// Reads the live hub's status surface when `APZ_DESKTOP_LIVE_HUB` names one
/// (`host:port`), and does nothing otherwise: the suite stays runnable on a
/// host with no hub.
#[test]
fn a_live_hub_reads_when_one_is_named() {
    let Ok(url) = std::env::var("APZ_DESKTOP_LIVE_HUB") else {
        eprintln!("APZ_DESKTOP_LIVE_HUB is not set: skipping the live hub read");
        return;
    };
    let endpoint = Endpoint::hub_url(&url).expect("a loopback host:port");
    let client = Client::new(endpoint.clone());
    let status = client
        .hub_status()
        .expect("the live hub should answer its status surface");
    println!(
        "live {}: {} cid(s) served on vsock port {} · up {}s · {}/{} session(s) · {} agent(s) connected",
        endpoint.url(),
        status.node.cids.len(),
        status.node.port,
        status.node.uptime_s,
        status.sessions_active,
        status.node.max_sessions,
        status.agents.len()
    );
    let now = agentplaza_desktop::epoch_ms();
    for agent in &status.agents {
        println!(
            "  cid {} · role {} · connected {} · beat {}s ago · {} call(s)",
            agent.cid,
            agent.role,
            agent.connected_at_ms,
            agent.heartbeat_age_ms(now) / 1_000,
            agent.calls
        );
    }
}

/// Reads the live daemon when `APZ_DESKTOP_LIVE` names one, and does nothing
/// otherwise: the suite stays runnable on a host with no daemon.
#[test]
fn a_live_daemon_reads_when_one_is_named() {
    let Ok(url) = std::env::var("APZ_DESKTOP_LIVE") else {
        eprintln!("APZ_DESKTOP_LIVE is not set: skipping the live read");
        return;
    };
    let endpoint = Endpoint::from_url(&url).expect("a loopback URL");
    let client = Client::new(endpoint.clone());
    let machine = client.machine().expect("the live daemon should report its machine");
    println!(
        "live {}: machine {} · {}/{} · {} · {} cpus · {} MiB · vsock {}",
        endpoint.url(),
        machine.name,
        machine.arch,
        machine.backend,
        machine.class,
        machine.cpus,
        machine.memory_mib,
        machine.vsock_mode
    );
    let page = client.runs().expect("the live daemon should answer");
    println!("live {}: {} run(s)", endpoint.url(), page.runs.len());
    for run in &page.runs {
        println!(
            "  {} {} · {}/{} vm · {} vcpu · {} MiB",
            run.id,
            run.state.label(),
            run.live_vms(),
            run.vms.len(),
            run.vcpus,
            run.memory_mib
        );
        let detail = client.run(&run.id).expect("the run should read");
        println!(
            "    detail: serial {} bytes, {} vm",
            detail.serial().len(),
            detail.vms.len()
        );
        for vm in &detail.vms {
            let kind = vm.serial.as_deref().map(exit_kind);
            println!(
                "    vm {} {} · role {:?} · cid {:?} · serial {:?} bytes · exit {}",
                vm.id,
                vm.state.label(),
                vm.role,
                vm.guest_cid,
                vm.serial.as_deref().map(str::len),
                kind.unwrap_or(ExitKind::Unknown).label()
            );
        }
    }

    // The ledger, read the same way and mutating nothing: every task the live
    // daemon holds, with its own state and the agent holding it.
    let page = client.tasks().expect("the live daemon should answer its ledger");
    println!("live {}: {} task(s)", endpoint.url(), page.tasks.len());
    for task in &page.tasks {
        println!(
            "  {} {} · goal {:?} · role {:?} · budget {} · assignee {:?} · agent cid {:?} · note {:?}",
            task.id,
            task.state.label(),
            task.goal,
            task.role.as_deref(),
            task.budget,
            task.assignee_id.as_deref(),
            task.agent_cid.copied(),
            task.note.as_deref()
        );
        // Each row is also readable on its own, which is the ledger pane's
        // refresh path.
        let one = client.task(&task.id).expect("the task should read");
        assert_eq!(one, *task, "the single-task read is the list's own row");
    }
}
