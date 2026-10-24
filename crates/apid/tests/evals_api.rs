//! The eval routes on a scratch hub (PLAN 25): the cost report, the leaderboard, the reviewers, the archive and the
//! benchmark's replays answer with their shapes, and a grouping the report does not know is refused. The seeded
//! figures are checked on the store itself (`evals.rs`'s tests): a scratch hub takes no sessions of its own here.

mod common;

use common::{Hub, Scratch};
use serde_json::Value;

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
