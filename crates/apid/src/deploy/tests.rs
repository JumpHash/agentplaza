//! The deploy recipe's rollouts against a recording Cloudflare, and the Cloudflare client against a fake HTTP answer.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::cloudflare::{Api, Client, Target};
use super::health::{self, Health, Monitor};
use super::{
    Artifacts, Checks, Incidents, Ports, Rolled, Settings, breach, check_gate, eligible, roll,
    starting, telling,
};
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
    fn smoke(&self, _project: &str, _env: &Environment, _release: &Release) -> Result<(), String> {
        let n = self.smoke_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.fail_on == Some(n) {
            return Err("the smoke scenario sign-in failed".into());
        }
        Ok(())
    }

    fn health(
        &self,
        _project: &str,
        _env: &Environment,
        _since_ms: u64,
        _settings: &Settings,
    ) -> Result<Health, String> {
        Ok(Health {
            signals: self.errors,
            ..Health::default()
        })
    }
}

/// A monitoring source the test sets: the error ratio, and the p95 latency in seconds.
struct Gauge {
    error_rate: Mutex<f64>,
    p95_seconds: Mutex<f64>,
}

impl Gauge {
    fn steady() -> Gauge {
        Gauge {
            error_rate: Mutex::new(0.001),
            p95_seconds: Mutex::new(0.12),
        }
    }
}

impl Monitor for Gauge {
    fn query(&self, expr: &str) -> Result<Option<f64>, String> {
        if expr.contains("histogram_quantile") {
            Ok(Some(*self.p95_seconds.lock().unwrap()))
        } else {
            Ok(Some(*self.error_rate.lock().unwrap()))
        }
    }
}

/// The health a monitoring source reads, with no signals of the hub's own (the production checks read both).
struct Watched<'m> {
    monitor: &'m dyn Monitor,
}

impl Checks for Watched<'_> {
    fn smoke(&self, _project: &str, _env: &Environment, _release: &Release) -> Result<(), String> {
        Ok(())
    }

    fn health(
        &self,
        _project: &str,
        env: &Environment,
        _since_ms: u64,
        settings: &Settings,
    ) -> Result<Health, String> {
        let reading = health::read(
            self.monitor,
            &env.name,
            settings.window,
            &settings.error_query,
            &settings.latency_query,
        )?;
        Ok(Health {
            signals: 0,
            error_rate: reading.error_rate,
            p95_ms: reading.p95_ms,
        })
    }
}

/// The incidents a rollout opened, as `project env version: detail`.
#[derive(Default)]
struct Recording {
    opened: Mutex<Vec<String>>,
}

impl Incidents for Recording {
    fn open(&self, project: &str, env: &str, version: &str, detail: &str) {
        self.opened
            .lock()
            .unwrap()
            .push(format!("{project} {env} {version}: {detail}"));
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
        store_steps: Vec::new(),
        compatibility: Vec::new(),
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
    checks: &dyn Checks,
    env: &Environment,
    release: &Release,
) -> (Rolled, RunState) {
    let (rolled, state, _) = rollout_recorded(api, checks, env, release);
    (rolled, state)
}

/// [`rollout`], also returning the incidents it opened.
fn rollout_recorded(
    api: &RecordingApi,
    checks: &dyn Checks,
    env: &Environment,
    release: &Release,
) -> (Rolled, RunState, Vec<String>) {
    let settings = Settings::of(env)
        .unwrap()
        .expect("a Cloudflare environment");
    let incidents = Recording::default();
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
        incidents: &incidents,
        runs: &runs,
    };
    let rolled = roll(&ports, "site", env, &settings, release, &run);
    let state = runs.view(&run).expect("the run").run.state;
    let opened = incidents.opened.lock().unwrap().clone();
    (rolled, state, opened)
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
    let Some(Body::Bytes(content_type, bytes)) = seen[1].3.clone() else {
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
    let incidents = Recording::default();
    let ports = Ports {
        api: &api,
        artifacts: &Bundle,
        checks: &checks,
        incidents: &incidents,
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

fn watched_env(window_secs: &str) -> Environment {
    environment(
        "production",
        "production",
        false,
        &[
            ("CLOUDFLARE_WORKER", "site"),
            ("DEPLOY_WINDOW_SECS", window_secs),
        ],
    )
}

#[test]
fn a_steady_source_does_not_roll_back_and_opens_no_incident() {
    let env = watched_env("1");
    let api = RecordingApi::new(Some("v0"));
    let gauge = Gauge::steady();
    let (rolled, state, opened) = rollout_recorded(
        &api,
        &Watched { monitor: &gauge },
        &env,
        &release("1.2.0", ReleaseState::Published),
    );
    assert_eq!(rolled.status, "passed", "{}", rolled.detail);
    assert_eq!(state, RunState::Passed);
    assert!(opened.is_empty(), "{opened:?}");
    assert!(
        !api.calls().iter().any(|c| c.starts_with("restore")),
        "{:?}",
        api.calls()
    );
}

#[test]
fn an_error_rate_rise_rolls_back_on_its_own_and_opens_an_incident_with_its_numbers() {
    let env = watched_env("1");
    let api = RecordingApi::new(Some("v0"));
    let gauge = Gauge::steady();
    *gauge.error_rate.lock().unwrap() = 0.5;
    let (rolled, state, opened) = rollout_recorded(
        &api,
        &Watched { monitor: &gauge },
        &env,
        &release("1.2.0", ReleaseState::Published),
    );
    assert_eq!(rolled.status, "rolled_back", "{}", rolled.detail);
    assert_eq!(state, RunState::Failed);
    // The first stage breaches: the 50% stage never ran, and the previous version comes back.
    assert_eq!(
        api.calls(),
        [
            "live workers:site",
            "upload workers:site",
            "split workers:site v1:10,v0:90",
            "restore workers:site v0",
        ]
    );
    assert_eq!(opened.len(), 1, "{opened:?}");
    assert!(
        opened[0].contains("error rate 50.0% (at most 5.0%)"),
        "{opened:?}"
    );
    assert!(opened[0].starts_with("site production 1.2.0"), "{opened:?}");
    assert!(
        telling("production", "1.2.0", &rolled).starts_with("Rolled production back"),
        "{}",
        rolled.detail
    );
}

#[test]
fn a_latency_breach_also_rolls_back_and_names_the_p95() {
    let env = watched_env("1");
    let api = RecordingApi::new(Some("v0"));
    let gauge = Gauge::steady();
    *gauge.p95_seconds.lock().unwrap() = 4.5;
    let (rolled, _, opened) = rollout_recorded(
        &api,
        &Watched { monitor: &gauge },
        &env,
        &release("1.2.0", ReleaseState::Published),
    );
    assert_eq!(rolled.status, "rolled_back", "{}", rolled.detail);
    assert_eq!(
        api.calls().last().map(String::as_str),
        Some("restore workers:site v0")
    );
    assert_eq!(opened.len(), 1, "{opened:?}");
    assert!(
        opened[0].contains("p95 latency 4500 ms (at most 2000 ms)"),
        "{opened:?}"
    );
}

#[test]
fn a_failed_smoke_check_rolls_back_without_opening_an_incident() {
    let env = watched_env("1");
    let api = RecordingApi::new(Some("v0"));
    let checks = Scripted {
        fail_on: Some(1),
        ..Scripted::passing()
    };
    let (rolled, _, opened) = rollout_recorded(
        &api,
        &checks,
        &env,
        &release("1.2.0", ReleaseState::Published),
    );
    assert_eq!(rolled.status, "rolled_back", "{}", rolled.detail);
    assert!(opened.is_empty(), "a check is not a breach: {opened:?}");
}

#[test]
fn the_thresholds_are_read_from_the_environment_and_a_breach_names_its_numbers() {
    let env = environment(
        "production",
        "production",
        false,
        &[
            ("CLOUDFLARE_WORKER", "site"),
            ("HEALTH_MAX_ERROR_RATE", "0.2"),
            ("HEALTH_MAX_P95_MS", "900"),
        ],
    );
    let settings = Settings::of(&env).unwrap().unwrap();
    assert_eq!(settings.max_error_rate, 0.2);
    assert_eq!(settings.max_p95_ms, 900.0);
    assert_eq!(settings.error_query, health::ERROR_QUERY);
    let quiet = Health {
        signals: 0,
        error_rate: Some(0.1),
        p95_ms: Some(800.0),
    };
    assert_eq!(breach(&quiet, &settings), None);
    let slow = Health {
        signals: 0,
        error_rate: Some(0.3),
        p95_ms: Some(1200.0),
    };
    assert_eq!(
        breach(&slow, &settings).as_deref(),
        Some(
            "error rate 30.0% (at most 20.0%), p95 latency 1200 ms (at most 900 ms) in the 300s window"
        )
    );
    let signals = Health {
        signals: 2,
        ..Health::default()
    };
    assert!(
        breach(&signals, &settings)
            .unwrap()
            .contains("2 error signals")
    );

    for (key, bad) in [
        ("HEALTH_MAX_ERROR_RATE", "2"),
        ("HEALTH_MAX_ERROR_RATE", "many"),
        ("HEALTH_MAX_P95_MS", "-1"),
    ] {
        let env = environment(
            "production",
            "production",
            false,
            &[("CLOUDFLARE_WORKER", "site"), (key, bad)],
        );
        assert!(Settings::of(&env).is_err(), "{key}={bad} is refused");
    }
}

#[test]
fn the_chat_is_told_when_a_rollout_starts_and_how_it_ends() {
    let env = environment(
        "staging",
        "staging",
        false,
        &[("CLOUDFLARE_WORKER", "site-staging")],
    );
    let settings = Settings::of(&env).unwrap().unwrap();
    assert_eq!(
        starting("staging", "1.2.0", &settings),
        "Deploying 1.2.0 to staging: rolling out at 10% → 50% → 100%, with the smoke set and a health check after each stage."
    );
    let passed = Rolled {
        status: "passed",
        detail: "1.2.0 served".into(),
    };
    assert_eq!(
        telling("staging", "1.2.0", &passed),
        "Deployed 1.2.0 to staging: 1.2.0 served."
    );
    let stuck = Rolled {
        status: "failed",
        detail: "no earlier version to return to".into(),
    };
    assert!(telling("staging", "1.2.0", &stuck).contains("The owner needs to look"));
}
