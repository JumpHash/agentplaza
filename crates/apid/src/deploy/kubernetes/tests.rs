//! The Kubernetes platform's rollouts against a fake API server: a local HTTP server on 127.0.0.1 that records every request
//! in order and answers from a script. The rollouts reach it through the platforms' real HTTP path, so the calls are
//! ordinary HTTP requests with their headers and bodies.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::{
    APPLY, Client, MERGE, Ports, Progress, STRATEGIC, Settings, manifests, progress, roll, share,
};
use crate::deploy::health::{self, Health};
use crate::deploy::tests::Gauge;
use crate::deploy::{Artifacts, Checks, Incidents, Rolled};
use crate::environments::Environment;
use crate::platforms::http::{Http, Ureq};
use crate::recipes::runs::Runs;
use crate::releases::entity::{Release, ReleaseState};

/// The bearer token the connector holds in these tests.
const TOKEN: &str = "test-token";

/// The release's manifests: the stable Deployment (three replicas, its image set by the deploy) and its Service.
const MANIFESTS: &str = "\
apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
spec:
  replicas: 3
  selector:
    matchLabels:
      app: web
  template:
    metadata:
      labels:
        app: web
    spec:
      containers:
        - name: api
          image: registry.test/shop/api:0.0.1
---
apiVersion: v1
kind: Service
metadata:
  name: web
spec:
  selector:
    app: web
  ports:
    - port: 80
";

/// One request the fake API server received.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    auth: Option<String>,
    media: String,
    body: Value,
}

/// A local HTTP server standing in for the API server: `answer` gives each request's status and body.
struct FakeKube {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
}

impl FakeKube {
    fn start(answer: impl Fn(&Seen) -> (u16, Value) + Send + 'static) -> FakeKube {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        listener.set_nonblocking(true).expect("a listener");
        let base = format!("http://{}", listener.local_addr().expect("an address"));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (log, halt) = (Arc::clone(&seen), Arc::clone(&stop));
        std::thread::spawn(move || {
            while !halt.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => serve(stream, &answer, &log),
                    Err(_) => std::thread::sleep(Duration::from_millis(2)),
                }
            }
        });
        FakeKube { base, seen, stop }
    }

    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

impl Drop for FakeKube {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Reads one HTTP/1.1 request from `stream`, records it, and writes the answer the script gives.
fn serve(stream: TcpStream, answer: &dyn Fn(&Seen) -> (u16, Value), log: &Mutex<Vec<Seen>>) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let Ok(clone) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(clone);
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let (mut length, mut auth, mut media) = (0usize, None, String::new());
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 {
            break;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((key, value)) = header.split_once(':') {
            let value = value.trim().to_string();
            match key.trim().to_ascii_lowercase().as_str() {
                "content-length" => length = value.parse().unwrap_or(0),
                "authorization" => auth = Some(value),
                "content-type" => media = value,
                _ => {}
            }
        }
    }
    let mut raw = vec![0u8; length];
    let _ = reader.read_exact(&mut raw);
    let seen = Seen {
        method,
        path,
        auth,
        media,
        body: serde_json::from_slice(&raw).unwrap_or(Value::Null),
    };
    let (status, reply) = answer(&seen);
    log.lock().unwrap().push(seen);
    let text = reply.to_string();
    let head = format!(
        "HTTP/1.1 {status} status\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        text.len()
    );
    let mut stream = reader.into_inner();
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(text.as_bytes());
}

/// A Deployment as the API server reports it: its revision, its replicas, and whether its rollout is complete.
fn deployment(name: &str, replicas: u64, revision: u64, complete: bool) -> Value {
    let updated = if complete { replicas } else { 1 };
    json!({
        "apiVersion": "apps/v1",
        "kind": "Deployment",
        "metadata": {
            "name": name,
            "namespace": "shop",
            "generation": 2,
            "annotations": { "deployment.kubernetes.io/revision": revision.to_string() }
        },
        "spec": {
            "replicas": replicas,
            "selector": { "matchLabels": { "app": "web" } },
            "template": { "metadata": { "labels": { "app": "web" } }, "spec": { "containers": [{ "name": "api", "image": "registry.test/shop/api:1.1.0" }] } }
        },
        "status": { "observedGeneration": 2, "replicas": replicas, "updatedReplicas": updated, "availableReplicas": updated }
    })
}

/// A ReplicaSet of revision `revision` whose pods run `image`.
fn replica_set(revision: u64, image: &str) -> Value {
    json!({
        "metadata": { "name": format!("web-{revision}"), "annotations": { "deployment.kubernetes.io/revision": revision.to_string() } },
        "spec": { "template": {
            "metadata": { "labels": { "app": "web", "pod-template-hash": format!("h{revision}") } },
            "spec": { "containers": [{ "name": "api", "image": image }] }
        } }
    })
}

/// The API server of a healthy cluster: it wants the bearer token, the stable Deployment is at revision 3 (`1.1.0`) with
/// three replicas, and revision 2 (`1.0.0`) is in its history. Requests without the token are refused.
fn cluster(complete: bool) -> impl Fn(&Seen) -> (u16, Value) + Send + 'static {
    move |r: &Seen| {
        if r.auth.as_deref() != Some("Bearer test-token") {
            return (401, json!({ "message": "unauthorized" }));
        }
        let path = r.path.split('?').next().unwrap_or("");
        if path.ends_with("/replicasets") {
            return (
                200,
                json!({ "items": [
                    replica_set(2, "registry.test/shop/api:1.0.0"),
                    replica_set(3, "registry.test/shop/api:1.1.0")
                ] }),
            );
        }
        if r.method == "GET" && path.ends_with("/deployments/web") {
            return (200, deployment("web", 3, 3, complete));
        }
        if r.method == "GET" && path.ends_with("/deployments/web-canary") {
            return (200, deployment("web-canary", 3, 1, complete));
        }
        (200, r.body.clone())
    }
}

/// The requests as short lines: the method, the object's path under the namespace, the patch's media, and the replicas.
fn trail(seen: &[Seen]) -> Vec<String> {
    seen.iter()
        .map(|r| {
            let path = r
                .path
                .split('?')
                .next()
                .unwrap_or("")
                .trim_start_matches("/apis/apps/v1/namespaces/shop/")
                .trim_start_matches("/api/v1/namespaces/shop/");
            let media = match r.media.as_str() {
                APPLY => " apply",
                MERGE => " merge",
                STRATEGIC => " strategic",
                _ => "",
            };
            let replicas = r.body["spec"]["replicas"]
                .as_u64()
                .map(|n| format!(" replicas={n}"))
                .unwrap_or_default();
            format!("{} {path}{media}{replicas}", r.method)
        })
        .collect()
}

/// The manifests the release's artifact is.
struct Manifest(Vec<u8>);

impl Artifacts for Manifest {
    fn bytes(&self, _release: &Release, _target: &str) -> Result<Vec<u8>, String> {
        Ok(self.0.clone())
    }
}

/// The checks a test sets: 11.4's fake monitoring source, a smoke set that may fail, and a breach at the nth health read.
struct Stages {
    gauge: Gauge,
    smoke_fails: bool,
    breach_at: Option<usize>,
    reads: Mutex<usize>,
}

impl Stages {
    fn steady() -> Stages {
        Stages {
            gauge: Gauge::steady(),
            smoke_fails: false,
            breach_at: None,
            reads: Mutex::new(0),
        }
    }
}

impl Checks for Stages {
    fn smoke(&self, _project: &str, _env: &Environment, _release: &Release) -> Result<(), String> {
        if self.smoke_fails {
            Err("the sign-in scenario failed".into())
        } else {
            Ok(())
        }
    }

    fn health(
        &self,
        _project: &str,
        env: &Environment,
        _since_ms: u64,
        settings: &crate::deploy::Settings,
    ) -> Result<Health, String> {
        let read = {
            let mut reads = self.reads.lock().unwrap();
            *reads += 1;
            *reads
        };
        *self.gauge.error_rate.lock().unwrap() = if self.breach_at == Some(read) {
            0.5
        } else {
            0.001
        };
        let reading = health::read(
            &self.gauge,
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

#[derive(Default)]
struct Opened(Mutex<Vec<String>>);

impl Incidents for Opened {
    fn open(&self, project: &str, env: &str, version: &str, detail: &str) {
        self.0
            .lock()
            .unwrap()
            .push(format!("{project} {env} {version}: {detail}"));
    }
}

fn environment(vars: &[(&str, &str)]) -> Environment {
    Environment {
        name: "production".into(),
        kind: "production".into(),
        url: Some("https://shop.test".into()),
        track: "releases".into(),
        protected: false,
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

fn release(version: &str) -> Release {
    Release {
        id: format!("rel-{version}"),
        project: "shop".into(),
        repo: "shop".into(),
        version: version.into(),
        channel: "stable".into(),
        base: None,
        lane: "main".into(),
        from: "abc".into(),
        state: ReleaseState::Published,
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
        created_ms: 0,
        updated_ms: 0,
    }
}

/// The settings of a rollout: the stable Deployment `web` in `shop`, with `canary` as its canary when one is named, and
/// the polling and the timeout a test can run in milliseconds.
fn settings(canary: Option<&str>, timeout: Duration) -> Settings {
    let mut vars = vec![
        ("KUBERNETES_DEPLOYMENT", "web"),
        ("KUBERNETES_IMAGE", "registry.test/shop/api"),
        ("KUBERNETES_NAMESPACE", "shop"),
        ("KUBERNETES_CONTAINER", "api"),
    ];
    if let Some(canary) = canary {
        vars.push(("KUBERNETES_CANARY", canary));
    }
    let env = environment(&vars);
    let mut settings = Settings::of(&env)
        .unwrap()
        .expect("a Kubernetes environment");
    settings.poll = Duration::from_millis(10);
    settings.timeout = timeout;
    settings
}

/// One rollout of release `1.2.0` to the fake cluster, with the checks given: its outcome and the incidents it opened.
fn rollout(kube: &FakeKube, checks: &dyn Checks, settings: &Settings) -> (Rolled, Vec<String>) {
    let client = Client::new(Arc::new(Ureq), &kube.base, TOKEN).expect("a client");
    let artifacts = Manifest(MANIFESTS.as_bytes().to_vec());
    let incidents = Opened::default();
    let runs = Runs::default();
    let items = BTreeMap::from([("rollout".to_string(), 1)]);
    let run = runs
        .start_engine_items(
            "deploy",
            &items,
            "deploy",
            None,
            Some("shop"),
            Some("production@1.2.0"),
            1,
        )
        .expect("the deploy recipe is built in");
    let ports = Ports {
        client: &client,
        artifacts: &artifacts,
        checks,
        incidents: &incidents,
        runs: &runs,
    };
    let env = environment(&[]);
    let out = roll(&ports, "shop", &env, settings, &release("1.2.0"), &run);
    let opened = incidents.0.lock().unwrap().clone();
    (out, opened)
}

#[test]
fn a_service_rolls_out_and_a_failed_check_rolls_it_back_to_its_previous_revision() {
    let kube = FakeKube::start(cluster(true));
    let mut checks = Stages::steady();
    checks.smoke_fails = true;
    let settings = settings(None, Duration::from_secs(5));
    let (rolled, opened) = rollout(&kube, &checks, &settings);
    assert_eq!(rolled.status, "rolled_back", "{rolled:?}");
    assert!(
        rolled.detail.contains("sign-in scenario failed"),
        "{rolled:?}"
    );
    assert!(opened.is_empty(), "a failed check opens no incident");
    assert_eq!(
        trail(&kube.requests()),
        [
            "GET deployments/web",
            "PATCH services/web apply",
            "PATCH deployments/web apply replicas=3",
            "GET deployments/web",
            "GET deployments/web",
            "GET replicasets",
            "PATCH deployments/web strategic",
        ],
        "the apply, the status read, then the undo to revision 3"
    );
    let requests = kube.requests();
    let applied = &requests[2].body;
    assert_eq!(
        applied["spec"]["template"]["spec"]["containers"][0]["image"],
        "registry.test/shop/api:1.2.0",
        "the Deployment's image is the release's"
    );
    let undo = requests.last().expect("the undo");
    assert_eq!(
        undo.body["spec"]["template"]["spec"]["containers"][0]["image"],
        "registry.test/shop/api:1.1.0",
        "the undo puts back revision 3's template"
    );
    assert!(
        undo.body["spec"]["template"]["metadata"]["labels"]
            .get("pod-template-hash")
            .is_none(),
        "the ReplicaSet's hash label is taken out of the template"
    );
}

#[test]
fn a_passing_rollout_applies_once_and_reads_the_status_until_it_completes() {
    let kube = FakeKube::start(cluster(true));
    let settings = settings(None, Duration::from_secs(5));
    let (rolled, opened) = rollout(&kube, &Stages::steady(), &settings);
    assert_eq!(rolled.status, "passed", "{rolled:?}");
    assert!(opened.is_empty());
    assert_eq!(
        trail(&kube.requests()),
        [
            "GET deployments/web",
            "PATCH services/web apply",
            "PATCH deployments/web apply replicas=3",
            "GET deployments/web",
        ]
    );
}

#[test]
fn the_bearer_token_is_only_on_the_hubs_side_and_a_request_without_it_is_refused() {
    let kube = FakeKube::start(cluster(true));
    let settings = settings(None, Duration::from_secs(5));
    let (rolled, _) = rollout(&kube, &Stages::steady(), &settings);
    assert_eq!(rolled.status, "passed");
    // A fork-side request (no hub, no connector): it carries no token and the API server refuses it.
    let fork = Ureq
        .call(
            "GET",
            &format!("{}/apis/apps/v1/namespaces/shop/deployments/web", kube.base),
            &[],
            None,
        )
        .expect("the request reaches the server");
    assert_eq!(fork.status, 401);
    let requests = kube.requests();
    assert_eq!(
        requests.last().and_then(|r| r.auth.clone()),
        None,
        "the fork's request had no token"
    );
    let hub = requests.iter().filter(|r| r.auth.is_some()).count();
    assert_eq!(
        hub,
        requests.len() - 1,
        "every hub request carried the connector's token"
    );
    assert!(
        requests
            .iter()
            .filter(|r| r.auth.is_some())
            .all(|r| r.auth.as_deref() == Some("Bearer test-token")),
        "the token is the connector's, sent as a bearer header"
    );
    assert!(!rolled.detail.contains(TOKEN), "{rolled:?}");
}

#[test]
fn a_rollout_that_never_completes_fails_its_stage_at_the_timeout_and_is_undone() {
    let kube = FakeKube::start(cluster(false));
    let settings = settings(None, Duration::from_millis(300));
    let (rolled, opened) = rollout(&kube, &Stages::steady(), &settings);
    assert_eq!(rolled.status, "rolled_back", "{rolled:?}");
    assert!(
        rolled
            .detail
            .contains("did not finish its rollout within 300ms"),
        "{rolled:?}"
    );
    assert!(
        rolled.detail.contains("1 of 3 replicas updated"),
        "the last status read says why: {rolled:?}"
    );
    assert!(
        opened.is_empty(),
        "a timeout is a failed check, not a breach"
    );
    let lines = trail(&kube.requests());
    assert!(
        lines.len() > 6,
        "the status was read until the timeout: {lines:?}"
    );
    assert_eq!(lines[1], "PATCH services/web apply");
    assert_eq!(lines[2], "PATCH deployments/web apply replicas=3");
    assert_eq!(
        lines[lines.len() - 3..].to_vec(),
        vec![
            "GET deployments/web",
            "GET replicasets",
            "PATCH deployments/web strategic"
        ],
        "the undo reads the Deployment, lists its ReplicaSets, and patches the template back"
    );
}

#[test]
fn a_canary_promotes_on_a_steady_health_source() {
    let kube = FakeKube::start(cluster(true));
    let settings = settings(Some("web-canary"), Duration::from_secs(5));
    let (rolled, opened) = rollout(&kube, &Stages::steady(), &settings);
    assert_eq!(rolled.status, "passed", "{rolled:?}");
    assert!(opened.is_empty());
    assert_eq!(
        trail(&kube.requests()),
        [
            "GET deployments/web",
            "PATCH services/web apply",
            "PATCH deployments/web-canary apply replicas=1",
            "PATCH deployments/web merge replicas=2",
            "GET deployments/web-canary",
            "GET deployments/web",
            "PATCH deployments/web-canary merge replicas=2",
            "PATCH deployments/web merge replicas=1",
            "GET deployments/web-canary",
            "GET deployments/web",
            "PATCH deployments/web apply replicas=3",
            "PATCH deployments/web-canary merge replicas=0",
            "GET deployments/web",
        ],
        "the canary takes 10% then 50% of the replicas, then the release is promoted and the canary's replicas go"
    );
}

#[test]
fn a_canary_aborts_on_a_breach_and_the_stable_deployment_is_never_promoted() {
    let kube = FakeKube::start(cluster(true));
    let mut checks = Stages::steady();
    checks.breach_at = Some(2); // the health read after the 50% stage
    let settings = settings(Some("web-canary"), Duration::from_secs(5));
    let (rolled, opened) = rollout(&kube, &checks, &settings);
    assert_eq!(rolled.status, "rolled_back", "{rolled:?}");
    assert!(rolled.detail.contains("canary at 50%"), "{rolled:?}");
    assert!(rolled.detail.contains("error rate"), "{rolled:?}");
    assert_eq!(opened.len(), 1, "a breach opens an incident");
    assert!(
        opened[0].contains("1.2.0") && opened[0].contains("production"),
        "{opened:?}"
    );
    let lines = trail(&kube.requests());
    assert!(
        !lines
            .iter()
            .any(|l| l == "PATCH deployments/web apply replicas=3"),
        "the release never reached the stable Deployment: {lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("strategic")),
        "the stable Deployment was not undone, it was never changed: {lines:?}"
    );
    assert_eq!(
        lines[lines.len() - 2..].to_vec(),
        vec![
            "PATCH deployments/web-canary merge replicas=0",
            "PATCH deployments/web merge replicas=3"
        ],
    );
}

#[test]
fn a_canary_takes_its_share_of_the_replicas_and_never_all_of_them() {
    let settings = settings(Some("web-canary"), Duration::from_secs(5));
    assert_eq!(share(10, 3), 1);
    assert_eq!(share(50, 3), 2);
    assert_eq!(share(90, 2), 1, "a canary never takes every replica");
    assert_eq!(share(100, 3), 2, "100% is the promotion, never a share");
    assert_eq!(settings.canary.as_deref(), Some("web-canary"));
}

#[test]
fn the_status_reads_say_when_a_rollout_is_done() {
    let waiting = deployment("web", 3, 4, false);
    assert_eq!(
        progress(&waiting),
        Progress::Waiting("1 of 3 replicas updated".into())
    );
    let done = deployment("web", 3, 4, true);
    assert_eq!(progress(&done), Progress::Done);
    let mut stale = deployment("web", 3, 4, true);
    stale["metadata"]["generation"] = json!(5);
    assert!(matches!(progress(&stale), Progress::Waiting(_)));
    let mut stuck = deployment("web", 3, 4, false);
    stuck["status"]["conditions"] = json!([{ "type": "Progressing", "reason": "ProgressDeadlineExceeded", "message": "too slow" }]);
    assert_eq!(progress(&stuck), Progress::Failed("too slow".into()));
}

#[test]
fn a_kustomize_overlay_sets_the_namespace_and_image_and_refuses_what_it_cannot_build() {
    let settings = settings(None, Duration::from_secs(5));
    let overlaid = format!(
        "{MANIFESTS}---\napiVersion: kustomize.config.k8s.io/v1beta1\nkind: Kustomization\nnamespace: shop\nimages:\n  - name: registry.test/shop/api\n    newTag: \"9.9\"\n"
    );
    let (target, others) = manifests(
        overlaid.as_bytes(),
        &settings,
        "registry.test/shop/api:1.2.0",
    )
    .expect("a plain overlay resolves");
    assert_eq!(target["metadata"]["namespace"], "shop");
    assert_eq!(
        others[0]["metadata"]["namespace"], "shop",
        "the Service takes the overlay's namespace"
    );
    let refused = format!(
        "{MANIFESTS}---\napiVersion: kustomize.config.k8s.io/v1beta1\nkind: Kustomization\ncommonLabels:\n  team: web\n"
    );
    let why = manifests(
        refused.as_bytes(),
        &settings,
        "registry.test/shop/api:1.2.0",
    )
    .unwrap_err();
    assert!(why.contains("commonLabels"), "{why}");
    let files = format!(
        "{MANIFESTS}---\napiVersion: kustomize.config.k8s.io/v1beta1\nkind: Kustomization\nresources:\n  - base/deploy.yaml\n"
    );
    assert!(manifests(files.as_bytes(), &settings, "registry.test/shop/api:1.2.0").is_err());
}

#[test]
fn the_settings_of_an_environment_are_read_and_refused_when_they_cannot_name_an_image() {
    let plain = environment(&[("CLOUDFLARE_WORKER", "site")]);
    assert_eq!(
        Settings::of(&plain).unwrap(),
        None,
        "a Cloudflare environment is not a Kubernetes one"
    );
    let no_image = environment(&[("KUBERNETES_DEPLOYMENT", "web")]);
    assert!(Settings::of(&no_image).is_err());
    let same = environment(&[
        ("KUBERNETES_DEPLOYMENT", "web"),
        ("KUBERNETES_IMAGE", "registry.test/shop/api"),
        ("KUBERNETES_CANARY", "web"),
    ]);
    assert!(
        Settings::of(&same).is_err(),
        "a canary is a second Deployment"
    );
    let bad_timeout = environment(&[
        ("KUBERNETES_DEPLOYMENT", "web"),
        ("KUBERNETES_IMAGE", "registry.test/shop/api"),
        ("KUBERNETES_TIMEOUT_SECS", "0"),
    ]);
    assert!(Settings::of(&bad_timeout).is_err());
    let good = environment(&[
        ("KUBERNETES_DEPLOYMENT", "web"),
        ("KUBERNETES_IMAGE", "registry.test/shop/api"),
    ]);
    let settings = Settings::of(&good).unwrap().unwrap();
    assert_eq!(settings.namespace, "default");
    assert_eq!(settings.artifact, "manifests");
    assert_eq!(settings.timeout, Duration::from_secs(300));
    assert_eq!(
        settings.image_for("1.2.0").unwrap(),
        "registry.test/shop/api:1.2.0"
    );
    assert!(settings.image_for("bad tag").is_err());
}

#[test]
fn a_client_refuses_an_address_or_a_token_it_cannot_use() {
    assert!(Client::new(Arc::new(Ureq), "k8s.test:6443", TOKEN).is_err());
    assert!(Client::new(Arc::new(Ureq), "https://k8s.test:6443", "  ").is_err());
    let client = Client::new(Arc::new(Ureq), "https://k8s.test:6443/", TOKEN).unwrap();
    assert_eq!(client.base, "https://k8s.test:6443");
}
