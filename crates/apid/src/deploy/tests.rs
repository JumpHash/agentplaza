//! The deploy recipe's rollouts against a recording Cloudflare, and the Cloudflare client against a fake HTTP answer.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::cloudflare::{Api, Client, Target};
use super::{Artifacts, Checks, Ports, Rolled, Settings, check_gate, eligible, roll, telling};
use crate::environments::{Deploy, Environment};
use crate::platforms::http::{Body, Fake};
use crate::recipes::runs::{RunState, Runs};
use crate::releases::entity::{Release, ReleaseState, Store};

/// Records every call and answers from the script: the version that serves now, and which step's checks fail.
struct RecordingApi {
    log: Mutex<Vec<String>>,
    live: Option<String>,
    fail_restore: bool,
}

impl RecordingApi {
    fn new(live: Option<&str>) -> RecordingApi {
        RecordingApi {
            log: Mutex::new(Vec::new()),
            live: live.map(String::from),
            fail_restore: false,
        }
    }

    fn calls(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

impl Api for RecordingApi {
    fn live(&self, target: &Target) -> Result<Option<String>, String> {
        self.log
            .lock()
            .unwrap()
            .push(format!("live {}", target.label()));
        Ok(self.live.clone())
    }

    fn upload(&self, target: &Target, _bytes: &[u8]) -> Result<String, String> {
        self.log
            .lock()
            .unwrap()
            .push(format!("upload {}", target.label()));
        Ok("v1".to_string())
    }

    fn split(&self, target: &Target, versions: &[(String, u32)]) -> Result<(), String> {
        let served: Vec<String> = versions.iter().map(|(v, p)| format!("{v}:{p}")).collect();
        self.log
            .lock()
            .unwrap()
            .push(format!("split {} {}", target.label(), served.join(",")));
        Ok(())
    }

    fn restore(&self, target: &Target, to: &str) -> Result<(), String> {
        self.log
            .lock()
            .unwrap()
            .push(format!("restore {} {to}", target.label()));
        if self.fail_restore {
            return Err("the API refused the restore".into());
        }
        Ok(())
    }
}

struct Bundle;

impl Artifacts for Bundle {
    fn bytes(&self, _release: &Release, _target: &str) -> Result<Vec<u8>, String> {
        Ok(b"export default {};".to_vec())
    }
}

/// The smoke check fails on its `fail_on`-th call (1-based); the signals count `errors`.
struct Scripted {
    smoke_calls: AtomicUsize,
    fail_on: Option<usize>,
    errors: u32,
}

impl Scripted {
    fn passing() -> Scripted {
        Scripted {
            smoke_calls: AtomicUsize::new(0),
            fail_on: None,
            errors: 0,
        }
    }
}

impl Checks for Scripted {
    fn smoke(&self, _project: &str, _env: &Environment) -> Result<(), String> {
        let n = self.smoke_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.fail_on == Some(n) {
            return Err("the smoke scenario sign-in failed".into());
        }
        Ok(())
    }

    fn errors(
        &self,
        _project: &str,
        _env: &Environment,
        _since_ms: u64,
        _window: Duration,
    ) -> Result<u32, String> {
        Ok(self.errors)
    }
}

fn environment(name: &str, kind: &str, protected: bool, vars: &[(&str, &str)]) -> Environment {
    Environment {
        name: name.into(),
        kind: kind.into(),
        url: None,
        track: "releases".into(),
        protected,
        variables: vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        notes: String::new(),
        version: None,
        deployed_ms: 0,
        deploys: Vec::new(),
        created_ms: 0,
        updated_ms: 0,
        secrets: Vec::new(),
        hook: false,
    }
}

fn release(version: &str, state: ReleaseState) -> Release {
    Release {
        id: format!("rel-{version}"),
        project: "site".into(),
        repo: "site".into(),
        version: version.into(),
        channel: "stable".into(),
        base: None,
        lane: "main".into(),
        from: "abc".into(),
        state,
        number: None,
        notes: String::new(),
        run: None,
        changes: Vec::new(),
        artifacts: Vec::new(),
        not_run: Vec::new(),
        verdicts: Vec::new(),
        promotions: Vec::new(),
        sync: Vec::new(),
        created_ms: 0,
        updated_ms: 0,
    }
}

fn deployed(version: &str, status: &str) -> Deploy {
    Deploy {
        id: "d1".into(),
        version: version.into(),
        by: "owner".into(),
        at_ms: 1,
        status: status.into(),
        detail: String::new(),
        approval: None,
    }
}

fn staging(status: &str) -> Environment {
    let mut env = environment(
        "staging",
        "staging",
        false,
        &[("CLOUDFLARE_WORKER", "site-staging")],
    );
    env.deploys.push(deployed("1.2.0", status));
    env
}

/// Runs one rollout of `release` to `env` through `api`, with the checks given; returns its outcome and the run's state.
fn rollout(
    api: &RecordingApi,
    checks: &Scripted,
    env: &Environment,
    release: &Release,
) -> (Rolled, RunState) {
    let settings = Settings::of(env)
        .unwrap()
        .expect("a Cloudflare environment");
    let runs = Runs::default();
    let items =
        std::collections::BTreeMap::from([("rollout".to_string(), settings.targets.len() as u32)]);
    let run = runs
        .start_engine_items(
            "deploy",
            &items,
            "deploy",
            None,
            Some("site"),
            Some("staging@1.2.0"),
            1,
        )
        .expect("the deploy recipe is built in");
    let ports = Ports {
        api,
        artifacts: &Bundle,
        checks,
        runs: &runs,
    };
    let rolled = roll(&ports, "site", env, &settings, release, &run);
    let state = runs.view(&run).expect("the run").run.state;
    (rolled, state)
}

#[test]
fn a_qualified_release_is_served_in_stages_and_the_run_passes() {
    let env = staging("recorded");
    let api = RecordingApi::new(Some("v0"));
    let (rolled, state) = rollout(
        &api,
        &Scripted::passing(),
        &env,
        &release("1.2.0", ReleaseState::Published),
    );
    assert_eq!(rolled.status, "passed", "{}", rolled.detail);
    assert_eq!(state, RunState::Passed);
    assert_eq!(
        api.calls(),
        [
            "live workers:site-staging",
            "upload workers:site-staging",
            "split workers:site-staging v1:10,v0:90",
            "split workers:site-staging v1:50,v0:50",
            "split workers:site-staging v1:100",
        ]
    );
    assert!(telling("staging", "1.2.0", &rolled).starts_with("Deployed 1.2.0 to staging"));
}

#[test]
fn a_failed_smoke_check_restores_the_previous_version_and_the_run_fails() {
    let env = staging("recorded");
    let api = RecordingApi::new(Some("v0"));
    let checks = Scripted {
        fail_on: Some(2),
        ..Scripted::passing()
    };
    let (rolled, state) = rollout(
        &api,
        &checks,
        &env,
        &release("1.2.0", ReleaseState::Published),
    );
    assert_eq!(rolled.status, "rolled_back", "{}", rolled.detail);
    assert_eq!(state, RunState::Failed);
    assert!(
        rolled.detail.contains("sign-in failed"),
        "{}",
        rolled.detail
    );
    assert_eq!(
        api.calls(),
        [
            "live workers:site-staging",
            "upload workers:site-staging",
            "split workers:site-staging v1:10,v0:90",
            "split workers:site-staging v1:50,v0:50",
            "restore workers:site-staging v0",
        ],
        "the 100% stage never ran"
    );
    assert!(telling("staging", "1.2.0", &rolled).starts_with("Rolled staging back"));
}

#[test]
fn error_signals_past_the_window_roll_back_on_their_own() {
    let env = staging("recorded");
    let api = RecordingApi::new(Some("v0"));
    let checks = Scripted {
        errors: 3,
        ..Scripted::passing()
    };
    let (rolled, _) = rollout(
        &api,
        &checks,
        &env,
        &release("1.2.0", ReleaseState::Published),
    );
    assert_eq!(rolled.status, "rolled_back");
    assert!(
        rolled.detail.contains("3 error signals"),
        "{}",
        rolled.detail
    );
    assert_eq!(
        api.calls().last().map(String::as_str),
        Some("restore workers:site-staging v0")
    );
}

#[test]
fn pages_goes_to_full_traffic_at_once_and_a_failed_check_returns_to_the_previous_deployment() {
    let env = environment(
        "docs",
        "staging",
        false,
        &[("CLOUDFLARE_PAGES", "docs-site")],
    );
    let api = RecordingApi::new(Some("d0"));
    let checks = Scripted {
        fail_on: Some(1),
        ..Scripted::passing()
    };
    let (rolled, _) = rollout(
        &api,
        &checks,
        &env,
        &release("1.2.0", ReleaseState::Published),
    );
    assert_eq!(rolled.status, "rolled_back");
    assert_eq!(
        api.calls(),
        [
            "live pages:docs-site",
            "upload pages:docs-site",
            "split pages:docs-site v1:100",
            "restore pages:docs-site d0",
        ]
    );
}

#[test]
fn a_first_deploy_has_nothing_to_return_to_and_says_so() {
    let env = environment(
        "docs",
        "staging",
        false,
        &[("CLOUDFLARE_WORKER", "site-new")],
    );
    let api = RecordingApi::new(None);
    let checks = Scripted {
        fail_on: Some(1),
        ..Scripted::passing()
    };
    let (rolled, state) = rollout(
        &api,
        &checks,
        &env,
        &release("1.2.0", ReleaseState::Published),
    );
    assert_eq!(rolled.status, "failed", "{}", rolled.detail);
    assert_eq!(state, RunState::Failed);
    assert!(
        rolled.detail.contains("no earlier version"),
        "{}",
        rolled.detail
    );
}

#[test]
fn an_unqualified_release_is_refused_and_a_protected_deploy_waits_for_a_passing_staging() {
    for state in [
        ReleaseState::Draft,
        ReleaseState::Candidate,
        ReleaseState::Qualifying,
        ReleaseState::Failed,
        ReleaseState::Yanked,
    ] {
        let releases = Store::default();
        releases.insert(release("1.3.0", state));
        assert!(
            eligible(&releases, "site", "1.3.0").is_err(),
            "{state:?} is not deployed"
        );
    }
    let releases = Store::default();
    releases.insert(release("1.3.0", ReleaseState::Published));
    assert!(eligible(&releases, "site", "1.3.0").is_ok());

    let production = environment(
        "production",
        "production",
        true,
        &[("CLOUDFLARE_WORKER", "site")],
    );
    let refused = check_gate(&releases, &[], "site", &production, "1.3.0").unwrap_err();
    assert!(
        refused.contains("only after a staging environment passed"),
        "{refused}"
    );
    // A staging deploy that did not pass does not open production.
    assert!(
        check_gate(
            &releases,
            &[staging("rolled_back")],
            "site",
            &production,
            "1.3.0"
        )
        .is_err()
    );
    // A staging deploy that passed another version does not either.
    let mut other = staging("passed");
    other.deploys[0].version = "1.2.0".into();
    assert!(check_gate(&releases, &[other], "site", &production, "1.3.0").is_err());
    // A passing staging of this version opens it.
    let mut passed = staging("passed");
    passed.deploys[0].version = "1.3.0".into();
    assert!(
        check_gate(&releases, &[passed], "site", &production, "1.3.0")
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_protected_production_is_never_touched_when_staging_failed() {
    let releases = Store::default();
    releases.insert(release("1.3.0", ReleaseState::Published));
    let production = environment(
        "production",
        "production",
        true,
        &[("CLOUDFLARE_WORKER", "site")],
    );
    // The staging rollout rolled back, so the gate names no passing staging deploy and nothing is sent to production.
    let mut staging_env = environment(
        "staging",
        "staging",
        false,
        &[("CLOUDFLARE_WORKER", "site-staging")],
    );
    staging_env.deploys.push(deployed("1.3.0", "rolled_back"));
    // The gate refuses, so no rollout of production starts and its Cloudflare is never called.
    let gate = check_gate(&releases, &[staging_env], "site", &production, "1.3.0");
    assert!(gate.is_err());
}

#[test]
fn settings_read_the_variables_and_refuse_a_bad_rollout() {
    let plain = environment("hook", "other", false, &[("DEPLOY_HOOK_NOTE", "x")]);
    assert_eq!(Settings::of(&plain).unwrap(), None);

    let good = environment(
        "production",
        "production",
        true,
        &[
            ("CLOUDFLARE_WORKER", "site"),
            ("ROLLOUT_STAGES", "10, 100"),
            ("DEPLOY_WINDOW_SECS", "60"),
        ],
    );
    let settings = Settings::of(&good).unwrap().unwrap();
    assert_eq!(settings.stages, vec![10, 100]);
    assert_eq!(settings.window, Duration::from_secs(60));
    assert_eq!(settings.max_errors, 0);
    assert_eq!(settings.artifact, "web");

    for bad in ["50,10,100", "10,50", "0,100", "10,x,100", "100,100"] {
        let env = environment(
            "production",
            "production",
            true,
            &[("CLOUDFLARE_WORKER", "site"), ("ROLLOUT_STAGES", bad)],
        );
        assert!(Settings::of(&env).is_err(), "{bad} is refused");
    }
    let slow = environment(
        "production",
        "production",
        true,
        &[
            ("CLOUDFLARE_WORKER", "site"),
            ("DEPLOY_WINDOW_SECS", "9999"),
        ],
    );
    assert!(Settings::of(&slow).is_err());
}

fn fake_cloudflare(
    answer: impl Fn(&str, &str, Option<&Body>) -> (u16, Value) + Send + Sync + 'static,
) -> (Arc<Fake>, Client) {
    let fake = Arc::new(Fake::new(answer));
    let client = Client::new(
        fake.clone(),
        "http://cf.test/client/v4",
        "acct1",
        "cf-test-token",
    )
    .expect("a client");
    (fake, client)
}

#[test]
fn the_client_sends_versions_and_percentages_and_the_token_only_in_its_header() {
    let (fake, client) = fake_cloudflare(|method, url, _| match (method, url) {
        ("GET", u) if u.ends_with("/workers/scripts/site/deployments") => (
            200,
            json!({"success": true, "result": {"deployments": [{"versions": [{"version_id": "v0", "percentage": 100}]}]}}),
        ),
        ("POST", u) if u.ends_with("/workers/scripts/site/versions") => {
            (200, json!({"success": true, "result": {"id": "v1"}}))
        }
        ("POST", u) if u.ends_with("/workers/scripts/site/deployments") => {
            (200, json!({"success": true, "result": {}}))
        }
        _ => (
            404,
            json!({"success": false, "errors": [{"message": "no such route"}]}),
        ),
    });
    let target = Target::Workers("site".into());
    assert_eq!(client.live(&target).unwrap().as_deref(), Some("v0"));
    assert_eq!(client.upload(&target, b"export default {};").unwrap(), "v1");
    client
        .split(&target, &[("v1".into(), 10), ("v0".into(), 90)])
        .unwrap();

    let seen = fake.seen();
    assert_eq!(
        fake.urls(),
        [
            "GET http://cf.test/client/v4/accounts/acct1/workers/scripts/site/deployments",
            "POST http://cf.test/client/v4/accounts/acct1/workers/scripts/site/versions",
            "POST http://cf.test/client/v4/accounts/acct1/workers/scripts/site/deployments",
        ]
    );
    for (_, _, auth, _) in &seen {
        assert_eq!(auth.as_deref(), Some("Bearer cf-test-token"));
    }
    let split = seen[2].3.clone();
    assert_eq!(
        split,
        Some(Body::Json(json!({
            "strategy": "percentage",
            "versions": [
                {"version_id": "v1", "percentage": 10},
                {"version_id": "v0", "percentage": 90}
            ]
        })))
    );
    let Some(Body::Raw {
        content_type,
        bytes,
    }) = seen[1].3.clone()
    else {
        panic!("the upload is a multipart body");
    };
    assert!(content_type.starts_with("multipart/form-data; boundary="));
    let text = String::from_utf8_lossy(&bytes).into_owned();
    assert!(
        text.contains("name=\"metadata\"") && text.contains("export default {};"),
        "{text}"
    );
    for (_, _, _, body) in &seen {
        assert!(
            !format!("{body:?}").contains("cf-test-token"),
            "the token stays in its header"
        );
    }
}

#[test]
fn a_pages_deploy_and_a_rollback_use_the_deployment_endpoints() {
    let (fake, client) = fake_cloudflare(|method, url, _| match (method, url) {
        ("GET", u) if u.ends_with("/pages/projects/docs/deployments") => (
            200,
            json!({"success": true, "result": [
                {"id": "d0", "environment": "production"},
                {"id": "dprev", "environment": "preview"}
            ]}),
        ),
        ("POST", u) if u.ends_with("/pages/projects/docs/deployments") => {
            (200, json!({"success": true, "result": {"id": "d1"}}))
        }
        ("POST", u) if u.ends_with("/pages/projects/docs/deployments/d0/rollback") => {
            (200, json!({"success": true, "result": {}}))
        }
        _ => (
            404,
            json!({"success": false, "errors": [{"message": "no such route"}]}),
        ),
    });
    let target = Target::Pages("docs".into());
    assert_eq!(client.live(&target).unwrap().as_deref(), Some("d0"));
    assert_eq!(client.upload(&target, b"<h1>docs</h1>").unwrap(), "d1");
    client.split(&target, &[("d1".into(), 100)]).unwrap();
    client.restore(&target, "d0").unwrap();
    assert!(
        fake.urls()
            .last()
            .unwrap()
            .ends_with("/pages/projects/docs/deployments/d0/rollback")
    );
    assert!(
        client
            .split(&target, &[("d1".into(), 50), ("d0".into(), 50)])
            .is_err()
    );
}

#[test]
fn an_error_from_cloudflare_never_carries_the_token() {
    let (_, client) = fake_cloudflare(|_, _, _| {
        (
            401,
            json!({"success": false, "errors": [{"message": "token cf-test-token is not valid"}]}),
        )
    });
    let err = client.live(&Target::Workers("site".into())).unwrap_err();
    assert!(err.contains("[redacted]"), "{err}");
    assert!(!err.contains("cf-test-token"), "{err}");
}

#[test]
fn an_empty_token_or_account_is_refused_before_any_call() {
    let http: Arc<dyn crate::platforms::http::Http> =
        Arc::new(Fake::new(|_, _, _| (200, Value::Null)));
    assert!(Client::new(http.clone(), "http://cf.test", "acct1", "  ").is_err());
    assert!(Client::new(http, "http://cf.test", "", "cf-test-token").is_err());
}

#[test]
fn a_deploy_to_a_published_release_is_picked_by_its_version_only() {
    let releases = Store::default();
    releases.insert(release("1.2.0", ReleaseState::Published));
    releases.insert(Release {
        id: "rel-x".into(),
        ..release("1.3.0", ReleaseState::Candidate)
    });
    assert_eq!(
        eligible(&releases, "site", "1.2.0").unwrap().version,
        "1.2.0"
    );
    assert!(
        eligible(&releases, "site", "1.3.0")
            .unwrap_err()
            .contains("not a qualified, published")
    );
}

#[test]
fn a_protected_production_rolls_out_in_stages_once_staging_passed_the_same_release() {
    let releases = Store::default();
    releases.insert(release("1.2.0", ReleaseState::Published));
    let production = environment(
        "production",
        "production",
        true,
        &[("CLOUDFLARE_WORKER", "site")],
    );
    let settings = check_gate(
        &releases,
        &[staging("passed")],
        "site",
        &production,
        "1.2.0",
    )
    .expect("the gate opens")
    .expect("a Cloudflare environment");
    let api = RecordingApi::new(Some("v0"));
    let checks = Scripted::passing();
    let runs = Runs::default();
    let release = eligible(&releases, "site", "1.2.0").expect("a published release");
    let ports = Ports {
        api: &api,
        artifacts: &Bundle,
        checks: &checks,
        runs: &runs,
    };
    let rolled = roll(&ports, "site", &production, &settings, &release, "");
    assert_eq!(rolled.status, "passed", "{}", rolled.detail);
    assert_eq!(
        api.calls(),
        [
            "live workers:site",
            "upload workers:site",
            "split workers:site v1:10,v0:90",
            "split workers:site v1:50,v0:50",
            "split workers:site v1:100",
        ]
    );
}
