//! The council's HTTP API, exercised over a real socket on an ephemeral port:
//! the motions board, the propose and vote rules a body can reach without a
//! live agent, the owner's decision surface, and the seat matrix the doctrine
//! pins against the fleet's own roles manifest.
//!
//! The daemon here boots no VMs, so no agent is connected: what the refusals at
//! this surface prove is the ones that come *first* — a body outside the
//! contract (400), a proposer the register does not show (409), and a decide
//! body outside its own contract (400) or naming no motion (404), which the
//! **owner** reaches with no agent at all (the owner is the operator, not an
//! agent). What needs a connected agent — the accepted propose, the seat rule
//! (403), the count, the re-vote that replaces, the `motion` frames — is covered
//! by the council's own unit tests (`crates/noded/src/council.rs`, where a
//! `ConnectedAgent` is built directly, exactly as the claim and the bid rules
//! are) and by the gated live test in this file, which stands up a real
//! five-role seat fleet over this same HTTP surface and drives the owner's
//! decision through it.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::request;

/// A fresh directory to serve as the console, unique to the calling test.
fn console_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("noded-council-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a console directory");
    dir
}

/// The motions board the daemon serves.
fn board(port: u16) -> serde_json::Value {
    let reply = request(port, "GET", "/v1/motions", None);
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert!(
        reply.content_type.starts_with("application/json"),
        "{}",
        reply.content_type
    );
    reply.json()
}

/// The repository root, resolved the way `smoke.rs` resolves it: a test
/// binary's working directory is the package directory, and the fleet's roles
/// manifest is read relative to the repository root.
///
/// It is canonicalized, so the guest-image paths handed to a run carry no `..`
/// component: the jailer refuses a bind source that names a parent directory
/// (`crate::vmm::jail`), and `crates/noded/../..` is one.
fn repo_root() -> PathBuf {
    fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .expect("the repository root")
}

#[test]
fn the_motions_board_starts_empty() {
    let server = common::start(console_dir("empty"));
    assert_eq!(
        board(server.port()),
        serde_json::json!({
            "motions": [],
            "total": 0,
            "limit": noded::council::MOTIONS_LIMIT,
            "truncated": false,
        })
    );
}

/// The seat table, asserted against the fleet's own manifest: `docs/agents/
/// council.md` seats exactly the Finance, Legal (Agents), Incident and Mayor
/// Office heads, and every one of the manifest's 21 roles is on one side of
/// that line.
#[test]
fn the_seat_table_is_derived_from_the_fleet_manifest() {
    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(repo_root().join("docs/agents/initial-roles.json"))
            .expect("the roles manifest"),
    )
    .expect("the manifest is JSON");
    let roles = manifest["roles"]
        .as_object()
        .expect("the manifest's roles")
        .keys()
        .cloned()
        .collect::<Vec<String>>();
    assert_eq!(roles.len(), 21, "the fleet's roles: {roles:?}");

    // The doctrine's four seats, and the exact set of manifest roles that hold
    // one.
    let seats = [
        noded::council::FINANCE,
        noded::council::LEGAL_AGENTS_COUNSEL,
        noded::council::INCIDENT,
        noded::council::MAYOR_OFFICE,
    ];
    assert_eq!(noded::council::SEATS, seats);
    let mut seated: Vec<&str> = roles
        .iter()
        .map(String::as_str)
        .filter(|role| noded::council::is_seated(role))
        .collect();
    seated.sort_unstable();
    let mut expected = seats.to_vec();
    expected.sort_unstable();
    assert_eq!(seated, expected, "exactly the four heads sit");

    // Every other manifest role holds no seat — including the courts, which
    // S-008a excludes by name.
    for role in &roles {
        let expected = seats.contains(&role.as_str());
        assert_eq!(
            noded::council::is_seated(role),
            expected,
            "{role} seated? {expected}"
        );
    }
    for excluded in noded::council::EXCLUDED_ROLES {
        assert!(roles.contains(&excluded.to_string()), "{excluded} is a role");
        assert!(
            !noded::council::is_seated(excluded),
            "{excluded} never sits (S-008a)"
        );
    }
    // And the roles a reader would most expect to be seated, but are not.
    for role in ["mayor", "legal-agents-defender", "legal-agents-ombudsman", "builder"] {
        assert!(roles.contains(&role.to_string()), "{role} is a role");
        assert!(!noded::council::is_seated(role), "{role} holds no seat");
    }
}

#[test]
fn a_propose_request_outside_the_contract_is_400() {
    let server = common::start(console_dir("400"));
    let port = server.port();

    // The body: the kind is the schema's enum, the text is non-empty and within
    // its cap, and a budget_move names its amount.
    let long = "x".repeat(noded::council::MAX_MOTION_TEXT + 1);
    let cases = vec![
        r#"{}"#.to_string(),
        r#"{"proposer_cid":100,"kind":"agenda","text":"a motion"}"#.to_string(),
        r#"{"proposer_cid":100,"kind":"priority","text":""}"#.to_string(),
        r#"{"proposer_cid":100,"kind":"priority","text":"  "}"#.to_string(),
        format!(r#"{{"proposer_cid":100,"kind":"priority","text":"{long}"}}"#),
        r#"{"proposer_cid":100,"kind":"budget_move","text":"move some budget"}"#.to_string(),
        "not json at all".to_string(),
        "[]".to_string(),
        String::new(),
    ];
    for body in cases {
        let reply = request(port, "POST", "/v1/motions", Some(&body));
        assert_eq!(reply.status, 400, "{body}: {}", reply.body);
        assert!(reply.json()["error"].is_string(), "{body}: {}", reply.body);
    }
    // A budget_move without its amount names the rule it broke.
    let refused = request(
        port,
        "POST",
        "/v1/motions",
        Some(r#"{"proposer_cid":100,"kind":"budget_move","text":"move some budget"}"#),
    );
    assert!(
        refused.json()["error"]
            .as_str()
            .expect("an error")
            .contains("must name an amount"),
        "{}",
        refused.body
    );

    // Nothing was recorded by any of those refusals.
    assert_eq!(board(port)["total"], 0);
}

/// `needs_owner` is derived, never trusted: a caller flag that disagrees with
/// the derivation is refused with the rule named, in either direction — and the
/// check happens against the treasury's own minted total, read at propose time.
#[test]
fn a_caller_flag_that_disagrees_with_the_derived_gate_is_400() {
    let server = common::start(console_dir("gate"));
    let port = server.port();

    // Understating an always-gated kind: the rule is named.
    let refused = request(
        port,
        "POST",
        "/v1/motions",
        Some(r#"{"proposer_cid":100,"kind":"law_proposal","text":"a law","needs_owner":false}"#),
    );
    assert_eq!(refused.status, 400, "{}", refused.body);
    let message = refused.json()["error"].as_str().expect("an error").to_string();
    assert!(message.contains("always owner-gated"), "{message}");
    assert!(message.contains("0067"), "{message}");

    // Overstating a kind the council decides: refused, stated.
    let refused = request(
        port,
        "POST",
        "/v1/motions",
        Some(r#"{"proposer_cid":100,"kind":"priority","text":"an agenda","needs_owner":true}"#),
    );
    assert_eq!(refused.status, 400, "{}", refused.body);
    let message = refused.json()["error"].as_str().expect("an error").to_string();
    assert!(message.contains("majority"), "{message}");

    // The budget rule reads the minted total: with an empty ledger any positive
    // move is over the fraction of nothing, so a false flag understates it.
    let refused = request(
        port,
        "POST",
        "/v1/motions",
        Some(
            r#"{"proposer_cid":100,"kind":"budget_move","text":"move 1","amount":1,"needs_owner":false}"#,
        ),
    );
    assert_eq!(refused.status, 400, "{}", refused.body);
    let message = refused.json()["error"].as_str().expect("an error").to_string();
    assert!(message.contains("10%"), "{message}");
    assert!(message.contains("minted budget total 0"), "{message}");

    // A flag that *agrees* with the derivation is inside the contract, and the
    // refusal further along (the register, since no agent is connected) proves
    // the body was accepted.
    let agreed = request(
        port,
        "POST",
        "/v1/motions",
        Some(r#"{"proposer_cid":100,"kind":"law_proposal","text":"a law","needs_owner":true}"#),
    );
    assert_eq!(agreed.status, 409, "{}", agreed.body);
    assert_eq!(agreed.json()["error"], "no live agent has guest CID 100");

    assert_eq!(board(port)["total"], 0, "no refusal recorded a motion");
}

/// A `budget_move`'s gate follows the treasury's minted total: mint 1000 into a
/// task, and the same motion is gated once its amount is over 10% of it.
#[test]
fn a_budget_move_is_gated_over_tenth_of_the_minted_total() {
    let server = common::start(console_dir("budget"));
    let port = server.port();

    // Mint a 1000-credit task budget into escrow: the treasury's own record.
    let minted = request(
        port,
        "POST",
        "/v1/tasks",
        Some(r#"{"goal":"fund the pool","budget":1000}"#),
    );
    assert_eq!(minted.status, 201, "{}", minted.body);

    // Exactly 10% is not over it: a false flag agrees, and the register is the
    // only thing left to refuse.
    let agreed = request(
        port,
        "POST",
        "/v1/motions",
        Some(
            r#"{"proposer_cid":100,"kind":"budget_move","text":"move 100","amount":100,"needs_owner":false}"#,
        ),
    );
    assert_eq!(agreed.status, 409, "{}", agreed.body);
    assert_eq!(agreed.json()["error"], "no live agent has guest CID 100");

    // One credit over 10% is over it: a false flag understates, naming the
    // amount and the total.
    let refused = request(
        port,
        "POST",
        "/v1/motions",
        Some(
            r#"{"proposer_cid":100,"kind":"budget_move","text":"move 101","amount":101,"needs_owner":false}"#,
        ),
    );
    assert_eq!(refused.status, 400, "{}", refused.body);
    let message = refused.json()["error"].as_str().expect("an error").to_string();
    assert!(message.contains("101"), "{message}");
    assert!(message.contains("minted budget total 1000"), "{message}");
}

#[test]
fn motions_need_a_connected_seated_agent() {
    let server = common::start(console_dir("connected"));
    let port = server.port();

    // Propose: the register is asked before the council's own seat rule, so an
    // agent that is not connected is a 409 in the register's own words.
    let refused = request(
        port,
        "POST",
        "/v1/motions",
        Some(r#"{"proposer_cid":100,"kind":"priority","text":"an agenda"}"#),
    );
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(refused.json()["error"], "no live agent has guest CID 100");

    // Vote: the same check, the same words.
    let refused = request(
        port,
        "POST",
        "/v1/motions/m1/vote",
        Some(r#"{"agent_cid":100,"vote":"yes","reason":"sound"}"#),
    );
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(refused.json()["error"], "no live agent has guest CID 100");

    // A vote body the schema does not carry is a 400 before the register is
    // asked (the vote word and the reason are the request's own).
    for body in [
        r#"{"agent_cid":100,"vote":"maybe","reason":"sound"}"#,
        r#"{"agent_cid":100,"vote":"yes","reason":""}"#,
        r#"{"agent_cid":100,"vote":"yes"}"#,
    ] {
        let reply = request(port, "POST", "/v1/motions/m1/vote", Some(body));
        assert_eq!(reply.status, 400, "{body}: {}", reply.body);
    }
}

#[test]
fn unknown_motions_are_404_and_wrong_methods_are_405() {
    let server = common::start(console_dir("404-405"));
    let port = server.port();

    let reply = request(port, "GET", "/v1/motions/m7", None);
    assert_eq!(reply.status, 404, "{}", reply.body);
    assert_eq!(reply.json()["error"], "no motion with id m7");
    for target in ["/v1/motions/m7/vote", "/v1/motions/", "/v1/motions/x/y"] {
        let reply = request(port, "GET", target, None);
        assert!(
            reply.status == 404 || reply.status == 405,
            "GET {target}: {}",
            reply.body
        );
    }
    for method in ["PUT", "DELETE", "PATCH"] {
        let reply = request(port, method, "/v1/motions", None);
        assert_eq!(reply.status, 405, "{method}: {}", reply.body);
        let reply = request(port, method, "/v1/motions/m1", None);
        assert_eq!(reply.status, 405, "{method}: {}", reply.body);
        let reply = request(port, method, "/v1/motions/m1/vote", None);
        assert_eq!(reply.status, 405, "{method}: {}", reply.body);
        let reply = request(port, method, "/v1/motions/m1/decide", None);
        assert_eq!(reply.status, 405, "{method}: {}", reply.body);
    }
}

/// The owner's decision surface: the body's own contract (400), the store's
/// refusals that need no motion (404), and the method (405) — everything the
/// decide endpoint can answer without a live gated motion. **The owner is the
/// operator**: the body names no agent CID, so no register check stands between
/// a well-formed body and the council's own rules — a body outside the contract
/// is a 400, and a motion the daemon does not hold is a 404, both reached with
/// no agent connected.
#[test]
fn a_decide_request_is_the_owners_and_is_checked_at_the_edge() {
    let server = common::start(console_dir("decide"));
    let port = server.port();

    // `approve` is required and typed: absent or non-boolean never deserializes.
    for body in [
        r#"{}"#,
        r#"{"note":"a note"}"#,
        r#"{"approve":"yes"}"#,
        r#"{"approve":1}"#,
        "not json at all",
        "",
    ] {
        let reply = request(port, "POST", "/v1/motions/m1/decide", Some(body));
        assert_eq!(reply.status, 400, "{body}: {}", reply.body);
        assert!(reply.json()["error"].is_string(), "{body}: {}", reply.body);
    }

    // The note is optional but capped, and blank is refused in the shared rule's
    // words.
    let blank = request(
        port,
        "POST",
        "/v1/motions/m1/decide",
        Some(r#"{"approve":true,"note":"   "}"#),
    );
    assert_eq!(blank.status, 400, "{}", blank.body);
    assert_eq!(blank.json()["error"], "note must not be empty");
    let long = "x".repeat(1001);
    let over = request(
        port,
        "POST",
        "/v1/motions/m1/decide",
        Some(&format!(r#"{{"approve":true,"note":"{long}"}}"#)),
    );
    assert_eq!(over.status, 400, "{}", over.body);
    assert_eq!(over.json()["error"], "note must be at most 1000 bytes");

    // A well-formed body reaches the council: no motion with that id is a 404,
    // not a register refusal (there are no agents at this surface at all).
    let reply = request(
        port,
        "POST",
        "/v1/motions/m7/decide",
        Some(r#"{"approve":true}"#),
    );
    assert_eq!(reply.status, 404, "{}", reply.body);
    assert_eq!(reply.json()["error"], "no motion with id m7");

    // Nothing was recorded by any of those refusals.
    assert_eq!(board(port)["total"], 0);
}

/// The council live: a real five-role fleet — the four seats and one agent that
/// holds no seat — proposed to, voted on and counted over the real HTTP API,
/// with the `motion` frames its events stream carries while it happens, and the
/// **owner's decision** driven through the same surface (a tie ratified, a gated
/// law vetoed, the council's own carried majority refused, a second decision
/// refused).
///
/// Gated on `APZ_NODED_SMOKE=1`, like `smoke.rs`: it needs a hypervisor and the
/// guest images.
///
/// ```text
/// APZ_NODED_SMOKE=1 cargo test -p noded --test council -- --nocapture
/// ```
#[test]
fn the_council_decides_a_motion_with_a_seated_fleet() {
    if std::env::var("APZ_NODED_SMOKE").as_deref() != Ok("1") {
        eprintln!("skipping: set APZ_NODED_SMOKE=1 (needs a hypervisor and the guest images)");
        return;
    }
    // The five roles, its own CIDs (well clear of the smoke tests' 300/400s and
    // the live fleet's 100..=120) and its own vsock directory, so nothing here
    // shares a socket path with a running daemon.
    const CID: u64 = 500;
    let root = repo_root();
    let scratch = std::env::temp_dir().join(format!("noded-council-smoke-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).expect("a scratch directory");
    let manifest = scratch.join("roles.json");
    fs::write(
        &manifest,
        r#"{"source":"noded council live test","roles":{
            "builder":{"role":"builder","title":"Builder","permits":["work"],"toolsets":["echo"],"budget":1},
            "finance":{"role":"finance","title":"Finance","permits":["pay"],"toolsets":["echo"],"budget":1},
            "incident":{"role":"incident","title":"Incident","permits":["respond"],"toolsets":["echo"],"budget":1},
            "legal-agents-counsel":{"role":"legal-agents-counsel","title":"Counsel","permits":["advise"],"toolsets":["echo"],"budget":1},
            "mayor-office":{"role":"mayor-office","title":"Mayor Office","permits":["coordinate"],"toolsets":["echo"],"budget":1}}}"#,
    )
    .expect("a five-role manifest");

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
    let server = noded::Server::start(config).expect("the council daemon should bind");
    let port = server.port();

    // Nothing is connected until the fleet is up: a propose now is refused by
    // the register, not by the seat table.
    let refused = request(
        port,
        "POST",
        "/v1/motions",
        Some(&format!(r#"{{"proposer_cid":{},"kind":"priority","text":"an agenda"}}"#, CID + 1)),
    );
    assert_eq!(refused.status, 409, "{}", refused.body);
    eprintln!("smoke: a propose before the fleet was up was refused: {}", refused.body);

    // Watch the events stream across the whole session, so the `motion` frames
    // are captured as they happen rather than inferred.
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("the stream connects");
    stream
        .set_read_timeout(Some(std::time::Duration::from_millis(500)))
        .expect("a read timeout");
    use std::io::{BufRead as _, Write as _};
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

    // Stand the fleet up the way the daemon does at startup.
    let fleet = noded::fleet::Fleet::load(&manifest, CID, 128, 1).expect("a five-role manifest");
    let run = server
        .node()
        .registry()
        .create_agent_fleet(&fleet)
        .expect("the fleet is a run");
    assert_eq!(
        run.view(noded::registry::Detail::Full).state,
        "running",
        "the five-agent fleet is up"
    );
    let connected: Vec<u64> = server
        .node()
        .registry()
        .connected_agents()
        .iter()
        .map(|agent| agent.cid)
        .collect();
    eprintln!("smoke: fleet {} is running, agents on CIDs {connected:?}", run.id());
    assert_eq!(connected, vec![CID, CID + 1, CID + 2, CID + 3, CID + 4]);

    // The manifest boots alphabetically: builder, finance, incident,
    // legal-agents-counsel, mayor-office.
    let builder = CID;
    let finance = CID + 1;
    let incident = CID + 2;
    let counsel = CID + 3;
    let mayor_office = CID + 4;

    // Mint a 1000-credit budget, so the treasury's minted total is real (the
    // budget_move gate reads it).
    let minted = request(
        port,
        "POST",
        "/v1/tasks",
        Some(r#"{"goal":"fund the pool","budget":1000}"#),
    );
    assert_eq!(minted.status, 201, "{}", minted.body);

    // A builder holds no seat, so proposing is a 403 naming the role.
    let refused = request(
        port,
        "POST",
        "/v1/motions",
        Some(&format!(
            r#"{{"proposer_cid":{builder},"kind":"priority","text":"a builder's wish"}}"#
        )),
    );
    assert_eq!(refused.status, 403, "{}", refused.body);
    assert_eq!(
        refused.json()["error"],
        format!("the agent on guest CID {builder} is role builder, which holds no council seat")
    );
    eprintln!("smoke: the unseated builder was refused: {}", refused.json()["error"]);

    // The Finance head proposes a priority motion (not gated).
    let proposed = request(
        port,
        "POST",
        "/v1/motions",
        Some(&format!(
            r#"{{"proposer_cid":{finance},"kind":"priority","text":"make the fleet's next priority the observability pass"}}"#
        )),
    );
    assert_eq!(proposed.status, 201, "{}", proposed.body);
    let motion = proposed.json();
    assert_eq!(motion["id"], "m1", "{motion}");
    assert_eq!(motion["proposer_role"], "finance");
    assert_eq!(motion["state"], "open");
    assert_eq!(motion["needs_owner"], false);
    assert_eq!(motion["tally"], serde_json::json!({"yes":0,"no":0,"abstain":0,"voted":0,"seats":4}));
    eprintln!(
        "smoke: m1 proposed by finance, state {} needs_owner {}",
        motion["state"], motion["needs_owner"]
    );

    // The unseated builder cannot vote either.
    let refused = request(
        port,
        "POST",
        "/v1/motions/m1/vote",
        Some(&format!(r#"{{"agent_cid":{builder},"vote":"yes","reason":"I like it"}}"#)),
    );
    assert_eq!(refused.status, 403, "{}", refused.body);
    eprintln!("smoke: the unseated builder's vote was refused: {}", refused.json()["error"]);

    // Two in favour, two against: 2-2 escalates to the owner. The Finance seat
    // also re-votes first, proving a seat holds one vote: the replacement is
    // visible in the tally before the other seats arrive.
    let vote = |id: &str, cid: u64, word: &str, reason: &str| {
        let reply = request(
            port,
            "POST",
            &format!("/v1/motions/{id}/vote"),
            Some(&format!(
                r#"{{"agent_cid":{cid},"vote":"{word}","reason":"{reason}"}}"#
            )),
        );
        assert_eq!(reply.status, 200, "{word} by {cid} on {id}: {}", reply.body);
        reply.json()
    };
    vote("m1", finance, "no", "I want the ledger first");
    let replaced = vote("m1", finance, "yes", "on reflection the dashboard is urgent");
    assert_eq!(replaced["tally"], serde_json::json!({"yes":1,"no":0,"abstain":0,"voted":1,"seats":4}), "{replaced}");
    assert_eq!(replaced["votes"].as_array().expect("votes").len(), 1, "one vote per seat");
    assert_eq!(replaced["votes"][0]["reason"], "on reflection the dashboard is urgent");
    eprintln!(
        "smoke: finance re-voted; the finance seat holds one vote ({}), newest reason stands",
        replaced["votes"][0]["reason"]
    );
    vote("m1", counsel, "yes", "the counsel sees no objection");
    vote("m1", incident, "no", "incident load is the priority");
    let tied = vote("m1", mayor_office, "no", "the towns disagree");
    assert_eq!(tied["state"], "tie_to_owner", "{tied}");
    eprintln!("smoke: m1 ended {} (2-2)", tied["state"]);
    assert_eq!(tied["tally"], serde_json::json!({"yes":2,"no":2,"abstain":0,"voted":4,"seats":4}));

    // A decided motion takes no further vote, and the state is named.
    let refused = request(
        port,
        "POST",
        "/v1/motions/m1/vote",
        Some(&format!(r#"{{"agent_cid":{finance},"vote":"yes","reason":"again"}}"#)),
    );
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(
        refused.json()["error"],
        "motion m1 is tie_to_owner, not open: a decided motion takes no further vote"
    );
    eprintln!("smoke: a vote on the decided m1 was refused: {}", refused.json()["error"]);

    // A law proposal is always owner-gated: unanimous, and still waiting on the
    // owner.
    let law = request(
        port,
        "POST",
        "/v1/motions",
        Some(&format!(
            r#"{{"proposer_cid":{counsel},"kind":"law_proposal","text":"codify the retention window"}}"#
        )),
    );
    assert_eq!(law.status, 201, "{}", law.body);
    assert_eq!(law.json()["needs_owner"], true, "{}", law.body);
    assert_eq!(law.json()["id"], "m2");
    for (cid, reason) in [
        (finance, "affordable"),
        (counsel, "lawful"),
        (incident, "no operational risk"),
    ] {
        vote("m2", cid, "yes", reason);
    }
    let carried = request(
        port,
        "POST",
        "/v1/motions/m2/vote",
        Some(&format!(r#"{{"agent_cid":{mayor_office},"vote":"yes","reason":"the towns agree"}}"#)),
    );
    assert_eq!(carried.status, 200, "{}", carried.body);
    assert_eq!(carried.json()["state"], "awaiting_owner", "{}", carried.body);
    eprintln!(
        "smoke: m2 (law_proposal) carried 4-0 and is {} — the owner ratifies",
        carried.json()["state"]
    );

    // A budget_move over 10% of the minted total is gated: a false flag is
    // refused with the rule named, and the derived gate is true.
    let refused = request(
        port,
        "POST",
        "/v1/motions",
        Some(&format!(
            r#"{{"proposer_cid":{finance},"kind":"budget_move","text":"move 101 of the pool","amount":101,"needs_owner":false}}"#
        )),
    );
    assert_eq!(refused.status, 400, "{}", refused.body);
    eprintln!("smoke: the understated budget_move was refused: {}", refused.json()["error"]);
    let gated = request(
        port,
        "POST",
        "/v1/motions",
        Some(&format!(
            r#"{{"proposer_cid":{finance},"kind":"budget_move","text":"move 101 of the pool","amount":101}}"#
        )),
    );
    assert_eq!(gated.status, 201, "{}", gated.body);
    assert_eq!(gated.json()["needs_owner"], true, "{}", gated.body);
    eprintln!(
        "smoke: m3 (budget_move 101 of a minted 1000) is owner-gated: needs_owner {}",
        gated.json()["needs_owner"]
    );

    // A priority the council's majority decides and carries 4-0: the owner is
    // not the decider of it — a majority is not theirs to re-decide.
    let plain = request(
        port,
        "POST",
        "/v1/motions",
        Some(&format!(
            r#"{{"proposer_cid":{incident},"kind":"priority","text":"an ordinary priority"}}"#
        )),
    );
    assert_eq!(plain.status, 201, "{}", plain.body);
    assert_eq!(plain.json()["id"], "m4");
    for (cid, reason) in [
        (finance, "affordable"),
        (counsel, "lawful"),
        (incident, "operationally fine"),
        (mayor_office, "the towns agree"),
    ] {
        vote("m4", cid, "yes", reason);
    }
    let carried = request(port, "GET", "/v1/motions/m4", None);
    assert_eq!(carried.status, 200, "{}", carried.body);
    assert_eq!(carried.json()["state"], "carried", "{}", carried.body);

    // **The owner's decision, over this same HTTP surface.** The owner is the
    // operator: the body carries no agent CID, and no agent holds their hand.
    let decide = |id: &str, approve: bool, note: Option<&str>| {
        let body = match note {
            Some(note) => format!(r#"{{"approve":{approve},"note":"{note}"}}"#),
            None => format!(r#"{{"approve":{approve}}}"#),
        };
        let reply = request(port, "POST", &format!("/v1/motions/{id}/decide"), Some(&body));
        (reply.status, reply.json())
    };

    // The 2-2 tie (m1) is ratified, with a note and a stamp.
    let (status, view) = decide("m1", true, Some("the owner carries it"));
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["state"], "ratified", "{view}");
    assert_eq!(view["owner"]["approve"], true, "{view}");
    assert_eq!(view["owner"]["note"], "the owner carries it", "{view}");
    assert!(view["owner"]["at"].is_u64(), "the stamp: {view}");
    eprintln!("smoke: the owner ratified the tied m1: state {}", view["state"]);

    // The gated law (m2, awaiting_owner) is vetoed.
    let (status, view) = decide("m2", false, None);
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["state"], "vetoed", "{view}");
    assert_eq!(view["owner"]["approve"], false, "{view}");
    assert!(view["owner"]["note"].is_null(), "an absent note: {view}");
    eprintln!("smoke: the owner vetoed the gated m2: state {}", view["state"]);

    // The council's own carried majority (m4) is refused: the council's own
    // decision stands.
    let (status, view) = decide("m4", true, None);
    assert_eq!(status, 409, "{view}");
    assert_eq!(
        view["error"],
        "motion m4 is carried: the owner decides only a tie_to_owner or awaiting_owner motion \
         — the council's own decision stands",
        "{view}"
    );
    eprintln!("smoke: the council's carried m4 was refused: {}", view["error"]);

    // And a second decision is refused: the owner decides a motion once.
    let (status, view) = decide("m1", false, None);
    assert_eq!(status, 409, "{view}");
    assert_eq!(
        view["error"],
        "motion m1 is already decided (ratified): the owner decides a motion once",
        "{view}"
    );
    eprintln!("smoke: a second decision on m1 was refused: {}", view["error"]);

    // The motions board is newest first and carries the whole session.
    let listed = board(port);
    assert_eq!(listed["total"], 4, "{listed}");
    assert_eq!(listed["truncated"], false);
    let ids: Vec<&str> = listed["motions"]
        .as_array()
        .expect("motions")
        .iter()
        .map(|motion| motion["id"].as_str().expect("an id"))
        .collect();
    assert_eq!(ids, vec!["m4", "m3", "m2", "m1"], "newest first");

    // And the stream carried the `motion` frames: one on propose, one per vote,
    // carrying the id, the kind and the state — and no text.
    let mut frames = String::new();
    loop {
        let mut line = String::new();
        match stream.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => frames.push_str(&line),
            Err(_) => break,
        }
    }
    let motion_frames: Vec<&str> = frames
        .lines()
        .filter(|line| line.starts_with("event: motion"))
        .collect();
    assert!(
        motion_frames.len() >= 8,
        "the stream carried {} motion frames:\n{frames}",
        motion_frames.len()
    );
    assert!(frames.contains("\"id\":\"m1\",\"kind\":\"priority\",\"state\":\"open\""), "{frames}");
    assert!(frames.contains("\"state\":\"tie_to_owner\""), "{frames}");
    assert!(frames.contains("\"id\":\"m2\",\"kind\":\"law_proposal\",\"state\":\"awaiting_owner\""), "{frames}");
    assert!(frames.contains("\"id\":\"m3\",\"kind\":\"budget_move\""), "{frames}");
    // The owner's decision fires the same frame, carrying the new state word:
    // no new frame kind, and still no text.
    assert!(frames.contains("\"id\":\"m1\",\"kind\":\"priority\",\"state\":\"ratified\""), "{frames}");
    assert!(frames.contains("\"id\":\"m2\",\"kind\":\"law_proposal\",\"state\":\"vetoed\""), "{frames}");
    assert!(
        !frames.contains("observability pass") && !frames.contains("the owner carries it"),
        "a motion frame carries no text:\n{frames}"
    );
    eprintln!(
        "smoke: the stream carried {} motion frames, e.g.:\n{}",
        motion_frames.len(),
        motion_frames
            .iter()
            .take(3)
            .copied()
            .collect::<Vec<&str>>()
            .join("\n")
    );

    // Stop the fleet, so the test leaves no VM behind.
    let killed = request(port, "DELETE", &format!("/v1/runs/{}", run.id()), None);
    assert_eq!(killed.status, 200, "{}", killed.body);
    assert_eq!(killed.json()["state"], "killed", "{}", killed.body);
    let _ = fs::remove_dir_all(&scratch);
}
