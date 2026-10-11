//! The treasury's HTTP API, exercised over a real socket on an ephemeral port:
//! the accounts view, the journal, the SSE frames, and the task lifecycle's
//! mint/refund wiring.
//!
//! The daemon here boots no VMs, so no agent can claim a task: the `done` and
//! `failed` paths (which need a claim) are covered by the ledger's own unit
//! tests, and this file exercises the paths this surface can reach — a task
//! created with a budget (minted into escrow) and withdrawn (refunded) — plus
//! the views themselves. v0 meters no real spend, so a withdrawn task refunds
//! its whole escrow; the view says so in its own words.

mod common;

use std::fs;
use std::io::BufRead as _;
use std::io::Write as _;
use std::path::PathBuf;

use common::request;

/// A fresh directory to serve as the console, unique to the calling test.
fn console_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("noded-treasury-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a console directory");
    dir
}

/// `GET /api/v0/treasury`'s body.
fn treasury_view(port: u16) -> serde_json::Value {
    let reply = request(port, "GET", "/api/v0/treasury", None);
    assert_eq!(reply.status, 200, "{}", reply.body);
    reply.json()
}

/// `GET /api/v0/treasury/journal`'s body.
fn journal_view(port: u16) -> serde_json::Value {
    let reply = request(port, "GET", "/api/v0/treasury/journal", None);
    assert_eq!(reply.status, 200, "{}", reply.body);
    reply.json()
}

/// An account's balance, by kind and owner.
fn balance(view: &serde_json::Value, kind: &str, owner: &str) -> i64 {
    view["accounts"]
        .as_array()
        .expect("an accounts array")
        .iter()
        .find(|account| account["kind"] == kind && account["owner_ref"] == owner)
        .unwrap_or_else(|| panic!("{kind} {owner} in {view}"))["balance"]
        .as_i64()
        .expect("a balance")
}

#[test]
fn the_treasury_starts_with_only_the_mint_and_no_note_spend() {
    let server = common::start(console_dir("empty"));
    let view = treasury_view(server.port());
    // Only the mint exists until something needs an account.
    assert_eq!(view["accounts"].as_array().expect("an array").len(), 1);
    assert_eq!(view["accounts"][0]["kind"], "mint");
    assert_eq!(view["accounts"][0]["owner_ref"], "mint");
    assert_eq!(view["accounts"][0]["balance"], 0);
    assert_eq!(view["totals"]["accounts"], 1);
    assert_eq!(view["totals"]["entries"], 0);
    assert_eq!(view["totals"]["balance"], 0);
    // v0's honest words about real spend.
    assert_eq!(view["spent"], 0);
    assert_eq!(
        view["note"],
        "v0 meters no real spend; a done task refunds its whole escrow"
    );

    let journal = journal_view(server.port());
    assert_eq!(journal["entries"].as_array().expect("an array").len(), 0);
    assert_eq!(journal["total"], 0);
    assert!(!journal["truncated"].as_bool().expect("a bool"));
}

#[test]
fn a_budgeted_task_mints_its_escrow_and_withdrawing_refunds_it() {
    let server = common::start(console_dir("cycle"));
    let port = server.port();

    // Creating a task with a budget mints it: a `mint` entry, the mint debited
    // and the task's escrow credited.
    let created = request(
        port,
        "POST",
        "/api/v0/tasks",
        Some(r#"{"goal":"paid work","budget":500}"#),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    assert_eq!(created.json()["id"], "t1");

    let view = treasury_view(port);
    assert_eq!(balance(&view, "task_escrow", "t1"), 500);
    assert_eq!(balance(&view, "mint", "mint"), -500);
    assert_eq!(view["totals"]["entries"], 1);
    assert_eq!(view["totals"]["credits"], 500);
    assert_eq!(view["totals"]["debits"], 500);

    // The journal shows the double entry, and it balances to zero.
    let journal = journal_view(port);
    assert_eq!(journal["total"], 1);
    assert!(!journal["truncated"].as_bool().expect("a bool"));
    let entry = &journal["entries"][0];
    assert_eq!(entry["id"], 1);
    assert_eq!(entry["kind"], "mint");
    assert_eq!(entry["task_id"], "t1");
    assert_eq!(entry["root_task"], "t1");
    assert_eq!(entry["posted_by"], "treasury-service");
    assert_eq!(entry["amount"], 500);
    let postings = entry["postings"].as_array().expect("an array");
    assert_eq!(postings.len(), 2);
    let sum: i64 = postings
        .iter()
        .map(|posting| posting["amount"].as_i64().expect("an amount"))
        .sum();
    assert_eq!(sum, 0, "the entry balances: {entry}");
    // The two sides name the same amount, opposite signs.
    let amounts: Vec<i64> = postings
        .iter()
        .map(|posting| posting["amount"].as_i64().expect("an amount"))
        .collect();
    assert!(amounts.contains(&-500) && amounts.contains(&500), "{entry}");

    // Withdrawing it refunds the whole escrow (v0 meters no real spend).
    let withdrawn = request(port, "DELETE", "/api/v0/tasks/t1", None);
    assert_eq!(withdrawn.status, 200, "{}", withdrawn.body);
    assert_eq!(withdrawn.json()["state"], "withdrawn");

    let view = treasury_view(port);
    assert_eq!(balance(&view, "task_escrow", "t1"), 0);
    assert_eq!(balance(&view, "mint", "mint"), 0);
    assert_eq!(view["totals"]["entries"], 2);
    assert_eq!(view["totals"]["balance"], 0);
    assert_eq!(view["spent"], 0);

    // The journal's second entry is the refund, oldest first.
    let journal = journal_view(port);
    assert_eq!(journal["total"], 2);
    let entries = journal["entries"].as_array().expect("an array");
    assert_eq!(entries[0]["kind"], "mint");
    assert_eq!(entries[1]["kind"], "refund");
    assert_eq!(entries[1]["task_id"], "t1");
    assert_eq!(entries[1]["amount"], 500);
    let sum: i64 = entries[1]["postings"]
        .as_array()
        .expect("an array")
        .iter()
        .map(|posting| posting["amount"].as_i64().expect("an amount"))
        .sum();
    assert_eq!(sum, 0, "the refund balances: {}", entries[1]);
}

#[test]
fn an_unbudgeted_task_posts_nothing() {
    let server = common::start(console_dir("unbudgeted"));
    let port = server.port();
    let created = request(port, "POST", "/api/v0/tasks", Some(r#"{"goal":"free work"}"#));
    assert_eq!(created.status, 201, "{}", created.body);
    assert_eq!(created.json()["budget"], 0);

    // No budget, no escrow: the treasury is still just the mint.
    let view = treasury_view(port);
    assert_eq!(view["accounts"].as_array().expect("an array").len(), 1);
    assert_eq!(view["totals"]["entries"], 0);
    assert_eq!(journal_view(port)["total"], 0);

    // Ending it is a no-op to the treasury.
    let withdrawn = request(port, "DELETE", "/api/v0/tasks/t1", None);
    assert_eq!(withdrawn.status, 200, "{}", withdrawn.body);
    assert_eq!(treasury_view(port)["totals"]["entries"], 0);
}

#[test]
fn the_treasury_endpoints_answer_only_their_own_methods_and_paths() {
    let server = common::start(console_dir("methods"));
    let port = server.port();

    // GET is the only method the two endpoints accept.
    for target in ["/api/v0/treasury", "/api/v0/treasury/journal"] {
        for method in ["POST", "PUT", "DELETE"] {
            let reply = request(port, method, target, Some("{}"));
            assert_eq!(reply.status, 405, "{method} {target}: {}", reply.body);
            assert!(reply.json()["error"].is_string(), "{}", reply.body);
        }
        assert_eq!(request(port, "GET", target, None).status, 200);
    }

    // A path under the treasury that is not the journal is not an endpoint.
    let reply = request(port, "GET", "/api/v0/treasury/nope", None);
    assert_eq!(reply.status, 404, "{}", reply.body);
    assert_eq!(reply.json()["error"], "no such endpoint");
    // And the journal accepts no trailing segment.
    assert_eq!(
        request(port, "GET", "/api/v0/treasury/journal/extra", None).status,
        404
    );
}

#[test]
fn an_events_stream_carries_the_mint_and_the_refund() {
    let server = common::start(console_dir("events"));
    let port = server.port();

    // Subscribe before the task exists.
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port))
        .expect("the events stream connects");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .expect("the stream gets a read timeout");
    stream
        .write_all(b"GET /api/v0/events HTTP/1.1\r\nHost: localhost\r\n\r\n")
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

    let created = request(
        port,
        "POST",
        "/api/v0/tasks",
        Some(r#"{"goal":"budgeted","budget":500}"#),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let withdrawn = request(port, "DELETE", "/api/v0/tasks/t1", None);
    assert_eq!(withdrawn.status, 200, "{}", withdrawn.body);

    let mut sock = stream.into_inner();
    let mut raw = Vec::new();
    for _ in 0..64 {
        let mut chunk = [0u8; 4096];
        match std::io::Read::read(&mut sock, &mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(err) => panic!("the stream failed: {err}"),
        }
        if String::from_utf8_lossy(&raw).contains("\"state\":\"withdrawn\"") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    // The mint's frame precedes the task's `open` (the escrow is funded before
    // the task is announced), and the refund precedes the withdrawal.
    assert_eq!(
        text,
        "event: treasury\ndata: {\"entry\":1,\"kind\":\"mint\",\"amount\":500,\"task\":\"t1\"}\n\n\
         event: task\ndata: {\"id\":\"t1\",\"state\":\"open\",\"assignee\":null}\n\n\
         event: treasury\ndata: {\"entry\":2,\"kind\":\"refund\",\"amount\":500,\"task\":\"t1\"}\n\n\
         event: task\ndata: {\"id\":\"t1\",\"state\":\"withdrawn\",\"assignee\":null}\n\n",
        "the stream carries the mint, the open, the refund and the withdrawal"
    );
}
