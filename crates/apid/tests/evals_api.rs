//! The eval routes on a scratch hub (PLAN 25): the cost report, the leaderboard, the reviewers, the archive and the
//! benchmark's replays answer with their shapes; and, over runs and rulings seeded into the hub's `plaza.db` beside it,
//! the routes answer with the figures those records add up to (PLAN 25.2).

mod common;

use apz_store::rusqlite::{Connection, params};
use common::{Hub, Scratch};
use serde_json::{Value, json};

/// The hub's `plaza.db`, opened beside it (WAL: the hub's own connection sees what this one commits).
fn seed_db(hub: &Hub) -> Connection {
    let conn = Connection::open(hub.state_dir().join("plaza.db")).expect("the hub's plaza.db");
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    conn
}

/// One step session's record: `id` is its number (and its position), its task, step, seat, model, cost, wall time,
/// its reviewer's verdict (None: no reviewer), whether it succeeded, when it ended and its later findings.
#[allow(clippy::too_many_arguments)]
fn run(
    conn: &Connection,
    id: i64,
    task: &str,
    step: &str,
    seat: &str,
    model: &str,
    cost: i64,
    wall: i64,
    verdict: Option<i64>,
    ok: i64,
    ended: i64,
    later: i64,
) {
    conn.execute(
        r#"INSERT INTO "eval_run" ("id","pos","session","project","task","recipe","step","seat","round","engine","engine_version","model","provider","harness","node","pool","tokens_in","tokens_out","tokens_cached","cost_micros","model_calls","tool_calls","tool_ms","wall_ms","verdict","outcome","ok","landed","reverted","ended_ms","recipe_version","harness_version","gate","later_findings","incidents","memory_cites","qual_stages","lab_minutes")
           VALUES (?1,?1,'s'||?1,'pr1',?2,'change',?3,?4,1,'harness','0.1.0',?5,'mock','acp',NULL,NULL,10,5,0,?6,1,2,30,?7,?8,'the step ended',?9,NULL,NULL,?10,2,'','none',?11,0,0,0,0)"#,
        params![id, task, step, seat, model, cost, wall, verdict, ok, ended, later],
    )
    .expect("a run row");
}

/// A reviewer's ruling on a finding of task `t3`: who raised it and who ruled, how it was ruled and when.
fn ruling(conn: &Connection, id: i64, reviewer: &str, verifier: &str, verdict: &str, ruled: i64) {
    conn.execute(
        r#"INSERT INTO "eval_finding" ("id","pos","project","task","review","finding","reviewer","reviewer_seat","verifier","verdict","severity","ruled_ms")
           VALUES (?1,?1,'pr1','t3','r1','f'||?1,?2,'reviewer',?3,?4,'major',?5)"#,
        params![id, reviewer, verifier, verdict, ruled],
    )
    .expect("a finding row");
}

/// Seeds four step sessions and two rulings: task t1 and t2 build on mock/agent (both succeed), t3 builds on mock/cheap
/// (it fails, and a later finding is confirmed on it) and is reviewed by mock/echo, which passed it at 1400 ms. Then
/// mock/echo's finding is dropped at 1500, and mock/third's finding is confirmed at 5000, after echo's pass.
fn seed(hub: &Hub) {
    let conn = seed_db(hub);
    run(
        &conn,
        1,
        "t1",
        "build",
        "builder",
        "mock/agent",
        300,
        1000,
        None,
        1,
        1001,
        0,
    );
    run(
        &conn,
        2,
        "t2",
        "build",
        "builder",
        "mock/agent",
        300,
        3000,
        None,
        1,
        3002,
        0,
    );
    run(
        &conn,
        3,
        "t3",
        "build",
        "builder",
        "mock/cheap",
        40,
        800,
        None,
        0,
        803,
        1,
    );
    run(
        &conn,
        4,
        "t3",
        "review",
        "reviewer",
        "mock/echo",
        20,
        200,
        Some(1),
        1,
        1400,
        0,
    );
    ruling(&conn, 1, "mock/echo", "mock/third", "dropped", 1500);
    ruling(&conn, 2, "mock/third", "mock/echo", "confirmed", 5000);
}

/// The float at `key` of a route's row, to compare with a tolerance.
fn num(row: &Value, key: &str) -> Option<f64> {
    row[key].as_f64()
}

/// The row of a report whose `key` is `name`.
fn row<'a>(rows: &'a Value, key: &str, name: &str) -> &'a Value {
    rows.as_array()
        .unwrap()
        .iter()
        .find(|r| r[key] == json!(name))
        .unwrap_or_else(|| panic!("no row {name} in {rows}"))
}

#[test]
fn the_eval_reports_answer_on_a_scratch_hub() {
    let scratch = Scratch::new("evals-api");
    let hub = Hub::start(&scratch, None, None);

    for by in ["project", "task", "step", "model"] {
        let (status, rows) = hub.call("GET", &format!("/v1/evals/costs?by={by}"), None);
        assert_eq!(status, 200, "{rows}");
        assert!(rows.is_array(), "a cost report is a list: {rows}");
    }
    let (status, refused) = hub.call("GET", "/v1/evals/costs?by=seat", None);
    assert_eq!(status, 400, "{refused}");

    for path in [
        "/v1/evals/leaderboard",
        "/v1/evals/leaderboard?step=build",
        "/v1/evals/reviewers",
        "/v1/evals/archive",
        "/v1/evals/bench",
        "/v1/evals/scores",
    ] {
        let (status, body): (u16, Value) = hub.call("GET", path, None);
        assert_eq!(status, 200, "{path}: {body}");
        assert!(body.is_array(), "{path} is a list: {body}");
    }
}

#[test]
fn the_routes_answer_with_the_figures_of_the_seeded_runs_and_rulings() {
    let scratch = Scratch::new("evals-figures");
    let hub = Hub::start(&scratch, None, None);
    seed(&hub);

    // The cost and success per step type: the build step ran three times and two succeeded, at 640 micro-dollars.
    let (status, by_step) = hub.call("GET", "/v1/evals/costs?by=step", None);
    assert_eq!(status, 200, "{by_step}");
    let build = row(&by_step, "key", "build");
    assert_eq!(
        (build["runs"].as_u64(), build["successes"].as_u64()),
        (Some(3), Some(2)),
        "{build}"
    );
    assert_eq!(build["cost_micros"], json!(640));
    assert_eq!(build["cost_per_success_micros"], json!(320.0));
    assert!(
        (num(build, "success_rate").unwrap() - 2.0 / 3.0).abs() < 1e-9,
        "{build}"
    );
    assert_eq!(
        build["time_per_success_ms"],
        json!(2400.0),
        "(1000 + 3000 + 800) over two successes"
    );
    assert_eq!(row(&by_step, "key", "review")["successes"], json!(1));

    // The cost and success per model: mock/agent succeeded twice; mock/cheap never did, so it has no cost per success.
    let (status, by_model) = hub.call("GET", "/v1/evals/costs?by=model", None);
    assert_eq!(status, 200, "{by_model}");
    let agent = row(&by_model, "key", "mock/agent");
    assert_eq!(
        (
            agent["runs"].as_u64(),
            agent["successes"].as_u64(),
            agent["cost_micros"].as_u64()
        ),
        (Some(2), Some(2), Some(600)),
        "{agent}"
    );
    assert_eq!(agent["time_per_success_ms"], json!(2000.0));
    assert_eq!(
        agent["quality"],
        json!(1.0),
        "both of its successes stayed clean"
    );
    let cheap = row(&by_model, "key", "mock/cheap");
    assert_eq!(
        (
            cheap["success_rate"].as_f64(),
            cheap["cost_per_success_micros"].clone()
        ),
        (Some(0.0), Value::Null),
        "{cheap}"
    );

    // Filtered to the builders, the review step's run is left out.
    let (_, builders) = hub.call("GET", "/v1/evals/costs?by=step&seat=builder", None);
    assert_eq!(builders.as_array().unwrap().len(), 1, "{builders}");
    assert_eq!(row(&builders, "key", "build")["runs"], json!(3));

    // The project's total: four runs, 660 micro-dollars.
    let (_, by_project) = hub.call("GET", "/v1/evals/costs?by=project", None);
    assert_eq!(
        (
            by_project[0]["key"].clone(),
            by_project[0]["cost_micros"].clone()
        ),
        (json!("pr1"), json!(660))
    );

    // The leaderboard of the build step: mock/agent first on its success rate, mock/cheap last.
    let (status, board) = hub.call("GET", "/v1/evals/leaderboard?step=build", None);
    assert_eq!(status, 200, "{board}");
    assert_eq!(board.as_array().unwrap().len(), 2, "{board}");
    assert_eq!(board[0]["model"], json!("mock/agent"));
    assert_eq!(board[0]["success_rate"], json!(1.0));
    assert_eq!(board[1]["model"], json!("mock/cheap"));
    assert_eq!(board[1]["success_rate"], json!(0.0));
    assert_eq!(
        board[1]["quality"],
        json!(0.0),
        "its one run failed, and a later finding reached it"
    );

    // The reviewers: mock/echo's finding was dropped, and mock/third's confirmed finding came after echo passed t3,
    // so echo missed it. mock/third's one finding was confirmed, and it passed nothing.
    let (status, reviewers) = hub.call("GET", "/v1/evals/reviewers", None);
    assert_eq!(status, 200, "{reviewers}");
    let echo = row(&reviewers, "model", "mock/echo");
    assert_eq!(
        (
            echo["findings"].clone(),
            echo["dropped"].clone(),
            echo["confirmed"].clone()
        ),
        (json!(1), json!(1), json!(0)),
        "{echo}"
    );
    assert_eq!(echo["precision"], json!(0.0));
    assert_eq!(
        (
            echo["changes_passed"].clone(),
            echo["missed_findings"].clone(),
            echo["misses"].clone()
        ),
        (json!(1), json!(1), json!(1)),
        "{echo}"
    );
    let third = row(&reviewers, "model", "mock/third");
    assert_eq!(
        (
            third["confirmed"].clone(),
            third["precision"].clone(),
            third["misses"].clone()
        ),
        (json!(1), json!(1.0), json!(0)),
        "{third}"
    );
}
