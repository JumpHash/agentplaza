//! The deploy recipe on a scratch hub: a Cloudflare environment refuses a deploy of a release that is not qualified and
//! published, and a protected production refuses a deploy that no passing staging stage has earned. The Cloudflare API is
//! a fake HTTP server the hub is pointed at (`APZ_CLOUDFLARE_API`); it records every call, and the token the connector
//! holds never shows in the hub's log or in an answer.
//!
//! What the booted hub does not reach here: a published release needs a landed change, a candidate built by a pool and
//! the hub's signatures (the release flow's own tests make one in-process). The qualified path (staged percentages,
//! smoke and signal checks, the rollback, the protected staging gate) is tested in `crates/apid/src/deploy/tests.rs`.

mod common;

use serde_json::json;

use common::{FakeDaemon, Hub, Scratch, text};

const TOKEN: &str = "cf-test-token-7f3a";

#[test]
fn a_cloudflare_environment_refuses_a_release_that_is_not_published_and_never_calls_cloudflare() {
    let scratch = Scratch::new("deploy-refused");
    let cloudflare = FakeDaemon::serve(|method, path, _| {
        // Every call is recorded by the fake; nothing here should be reached.
        (
            404,
            json!({ "success": false, "errors": [{ "message": format!("{method} {path} is not expected") }] })
                .to_string(),
        )
    });
    let base = format!("http://127.0.0.1:{}/client/v4", cloudflare.port);
    let hub = Hub::start_with_env(
        &scratch,
        None,
        None,
        None,
        &[
            ("APZ_CLOUDFLARE_API", base.as_str()),
            ("CLOUDFLARE_API_TOKEN", TOKEN),
            ("CLOUDFLARE_ACCOUNT_ID", "acct-test"),
        ],
    );

    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Site", "goal": "Serve the site" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);

    let (status, staging) = hub.call(
        "PUT",
        &format!("/v1/projects/{pid}/environments/staging"),
        Some(json!({
            "kind": "staging",
            "track": "releases",
            "protected": false,
            "variables": { "CLOUDFLARE_WORKER": "site-staging", "DEPLOY_WINDOW_SECS": "0" }
        })),
    );
    assert_eq!(status, 200, "{staging}");
    let (status, production) = hub.call(
        "PUT",
        &format!("/v1/projects/{pid}/environments/production"),
        Some(json!({
            "kind": "production",
            "track": "releases",
            "protected": true,
            "variables": { "CLOUDFLARE_WORKER": "site" }
        })),
    );
    assert_eq!(status, 200, "{production}");

    // Staging: a version that is not a qualified, published release is refused, and nothing is recorded.
    let (status, refused) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/environments/staging/deploy"),
        Some(json!({ "version": "1.2.0" })),
    );
    assert_eq!(status, 409, "{refused}");
    assert!(
        refused
            .to_string()
            .contains("not a qualified, published release"),
        "{refused}"
    );

    // Production: the same refusal, and it is not a staging gate either.
    let (status, refused) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/environments/production/deploy"),
        Some(json!({ "version": "1.2.0" })),
    );
    assert_eq!(status, 409, "{refused}");
    assert!(
        refused
            .to_string()
            .contains("not a qualified, published release"),
        "{refused}"
    );

    // The environments hold no deploy, and Cloudflare was never called.
    let (_, envs) = hub.call("GET", &format!("/v1/projects/{pid}/environments"), None);
    for env in envs.as_array().cloned().unwrap_or_default() {
        assert_eq!(env["deploys"], json!([]), "{env}");
        assert_eq!(env["version"], json!(null), "{env}");
    }
    assert!(cloudflare.seen().is_empty(), "{:?}", cloudflare.seen());

    // The token is the connector's; it never shows in the hub's log or in the environments.
    assert!(!hub.log().contains(TOKEN), "the token is in the hub's log");
    assert!(
        !envs.to_string().contains(TOKEN),
        "the token is in an answer"
    );
}
