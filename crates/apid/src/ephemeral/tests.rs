//! Ephemeral instances against fakes: a Kubernetes API server on a loopback port that records every request in order, the
//! Cloudflare and AWS calls answered by [`Fake`], and a local HTTP server standing in for an instance's address. The
//! fakes sit behind the real clients, so each request is the ordinary HTTP request the platform code makes.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};

use super::against::{self, Request as Against};
use super::platform;
use super::platform::slug;
use super::store::{self, Store};
use super::{Instance, Ports, State, Subject, Target, UpRequest, down_with, sweep, up_with};
use crate::deploy::{aws, cloudflare, kubernetes};
use crate::environments::Environment;
use crate::platforms::http::{Body, Fake, Http, Ureq};
use crate::releases::entity::{Release, ReleaseState, ReleaseSync};

const TOKEN: &str = "hub-only-token";
const CF_BASE: &str = "https://cf.test/client/v4";
const AWS_BASE: &str = "https://aws.test";
const ROLE: &str = "arn:aws:iam::123456789012:role/deployer";
const DIGEST: &str = "sha256:abababababababababababababababababababababababababababababababab";

/// The manifests a lane or a release is: a Deployment, a Service and an Ingress naming the instance's host.
pub(crate) const MANIFESTS: &str = "\
apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
spec:
  replicas: 1
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
---
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: web
spec:
  rules:
    - host: shop.test
";

/// The environment's variables for a Kubernetes instance.
pub(crate) fn kube_env(extra: &[(&str, &str)]) -> Environment {
    let mut vars = vec![("EPHEMERAL", "true"), ("KUBERNETES_DEPLOYMENT", "web")];
    vars.extend_from_slice(extra);
    environment("pr", &vars)
}

fn environment(name: &str, vars: &[(&str, &str)]) -> Environment {
    Environment {
        name: name.into(),
        kind: "development".into(),
        url: None,
        track: "manual".into(),
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

fn release(id: &str) -> Release {
    Release {
        id: id.into(),
        project: "shop".into(),
        repo: "shop".into(),
        version: "0.2.0".into(),
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
        sync: vec![ReleaseSync {
            platform: "oci-registry/image:.@any".into(),
            state: "published".into(),
            url: Some(format!("https://registry.test/shop/api:0.2.0@{DIGEST}")),
            at_ms: 1,
        }],
        store_steps: Vec::new(),
        compatibility: Vec::new(),
        created_ms: 0,
        updated_ms: 0,
    }
}

pub(crate) fn lane(id: &str) -> Subject {
    Subject::Lane(id.into())
}

/// The fakes behind the ports: the clock, the environment, the subject's artifact, the platforms' fakes and the secrets.
pub(crate) struct Fakes {
    pub(crate) now: AtomicU64,
    pub(crate) env: Environment,
    pub(crate) release: Option<Release>,
    pub(crate) artifact: Vec<u8>,
    pub(crate) kube: Option<String>,
    pub(crate) cloudflare: Option<Arc<Fake>>,
    pub(crate) aws: Option<Arc<Fake>>,
    pub(crate) secrets: Vec<(String, String)>,
    /// The blobs a build recorded, by digest (a run's artifacts).
    pub(crate) blobs: Vec<(String, Vec<u8>)>,
}

impl Fakes {
    pub(crate) fn new(env: Environment) -> Fakes {
        Fakes {
            now: AtomicU64::new(1_792_843_200_000),
            env,
            release: None,
            artifact: MANIFESTS.as_bytes().to_vec(),
            kube: None,
            cloudflare: None,
            aws: None,
            secrets: Vec::new(),
            blobs: Vec::new(),
        }
    }

    pub(crate) fn advance(&self, minutes: u64) {
        self.now.fetch_add(minutes * 60_000, Ordering::SeqCst);
    }
}

impl Ports for Fakes {
    fn now(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }

    fn environment(&self, _project: &str, env: &str) -> Result<Environment, String> {
        if env == self.env.name {
            Ok(self.env.clone())
        } else {
            Err(format!("no environment {env}"))
        }
    }

    fn release(&self, _project: &str, subject: &Subject) -> Result<Option<Release>, String> {
        match subject {
            Subject::Lane(_) => Ok(None),
            Subject::Release(id) => self
                .release
                .clone()
                .filter(|r| &r.id == id)
                .map(Some)
                .ok_or_else(|| format!("no release {id}")),
        }
    }

    fn artifact(
        &self,
        _project: &str,
        _subject: &Subject,
        _artifact: &str,
    ) -> Result<Vec<u8>, String> {
        Ok(self.artifact.clone())
    }

    fn blob(&self, digest: &str) -> Result<Vec<u8>, String> {
        self.blobs
            .iter()
            .find(|(d, _)| d == digest)
            .map(|(_, bytes)| bytes.clone())
            .ok_or_else(|| format!("no blob {digest}"))
    }

    fn kubernetes(&self) -> Result<kubernetes::Client, String> {
        let base = self.kube.as_deref().ok_or("no cluster in this test")?;
        kubernetes::Client::new(Arc::new(Ureq), base, TOKEN)
    }

    fn tools(&self) -> kubernetes::Tools {
        kubernetes::Tools::on("")
    }

    fn cloudflare(&self) -> Result<cloudflare::Client, String> {
        let http = self
            .cloudflare
            .clone()
            .ok_or("no Cloudflare in this test")?;
        cloudflare::Client::new(http as Arc<dyn Http>, CF_BASE, "acct-1", "cf-token")
    }

    fn aws(&self, region: Option<&str>) -> Result<aws::Client, String> {
        let http = self.aws.clone().ok_or("no AWS in this test")?;
        let clock: aws::Clock =
            Arc::new(|| SystemTime::UNIX_EPOCH + Duration::from_secs(1_792_843_200));
        aws::Client::new(
            http as Arc<dyn Http>,
            aws::Endpoints::at(AWS_BASE),
            region.unwrap_or("eu-west-1"),
            ROLE,
            aws::Base::Keys {
                access_key: "AKIABASE".into(),
                secret_key: "base-secret".into(),
            },
            clock,
            aws::Wait {
                interval: Duration::ZERO,
                attempts: 3,
            },
        )
    }

    fn secrets(&self, _project: &str, _env: &str) -> Vec<(String, String)> {
        self.secrets.clone()
    }
}

// ---- The fake Kubernetes API server ----------------------------------------------------------------------------------

/// One request the fake API server received.
#[derive(Debug, Clone)]
pub(crate) struct Seen {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) auth: Option<String>,
    pub(crate) body: Value,
}

/// A loopback server standing in for the API server: each request is recorded, then answered by `answer`.
pub(crate) struct FakeKube {
    pub(crate) base: String,
    pub(crate) seen: Arc<Mutex<Vec<Seen>>>,
    pub(crate) stop: Arc<AtomicBool>,
}

impl FakeKube {
    pub(crate) fn start(answer: impl Fn(&Seen) -> (u16, Value) + Send + 'static) -> FakeKube {
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

    pub(crate) fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// The requests as `METHOD path`, in order.
    pub(crate) fn trail(&self) -> Vec<String> {
        self.requests()
            .iter()
            .map(|r| format!("{} {}", r.method, r.path))
            .collect()
    }
}

impl Drop for FakeKube {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

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
    let path = parts
        .next()
        .unwrap_or("")
        .split('?')
        .next()
        .unwrap_or("")
        .to_string();
    let (mut length, mut auth) = (0usize, None);
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
            match key.trim().to_ascii_lowercase().as_str() {
                "content-length" => length = value.trim().parse().unwrap_or(0),
                "authorization" => auth = Some(value.trim().to_string()),
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

/// A healthy cluster: it wants the hub's token, every apply succeeds, and a Deployment's rollout is complete.
pub(crate) fn healthy(r: &Seen) -> (u16, Value) {
    if r.auth.as_deref() != Some(&format!("Bearer {TOKEN}")) {
        return (401, json!({ "message": "unauthorized" }));
    }
    if r.method == "POST" && r.path == "/api/v1/namespaces" {
        return (201, r.body.clone());
    }
    if r.method == "GET" && r.path.contains("/deployments/") {
        return (200, complete_deployment(&r.path));
    }
    (200, r.body.clone())
}

fn complete_deployment(path: &str) -> Value {
    let name = path.rsplit('/').next().unwrap_or("web");
    json!({
        "metadata": { "name": name, "generation": 1 },
        "spec": { "replicas": 1, "selector": { "matchLabels": { "app": "web" } } },
        "status": { "observedGeneration": 1, "replicas": 1, "updatedReplicas": 1, "availableReplicas": 1 }
    })
}

/// The trail of a lane's Kubernetes instance in namespace `ns`, from its create to its teardown.
pub(crate) fn kube_up_trail(ns: &str) -> Vec<String> {
    vec![
        "POST /api/v1/namespaces".to_string(),
        format!("PATCH /api/v1/namespaces/{ns}/configmaps/apz-ephemeral-env"),
        format!("PATCH /api/v1/namespaces/{ns}/secrets/apz-ephemeral-secrets"),
        format!("PATCH /apis/apps/v1/namespaces/{ns}/deployments/web"),
        format!("PATCH /api/v1/namespaces/{ns}/services/web"),
        format!("PATCH /apis/networking.k8s.io/v1/namespaces/{ns}/ingresses/web"),
        format!("GET /apis/apps/v1/namespaces/{ns}/deployments/web"),
    ]
}

// ---- The fake Cloudflare and AWS ------------------------------------------------------------------------------------

/// The Cloudflare calls of a Pages and a Worker instance, answered as Cloudflare answers them.
fn cloudflare_fake() -> Arc<Fake> {
    Arc::new(Fake::new(|method, url, _body| {
        if method == "POST" && url.ends_with("/pages/projects/shop-site/deployments") {
            return (
                200,
                json!({"success": true, "result": {"id": "dep-1", "url": "https://dep-1.shop-site.pages.dev"}}),
            );
        }
        (200, json!({"success": true, "result": {}}))
    }))
}

/// The AWS calls of a stack instance: the role's session, and CloudFormation's create, execute, describe and delete. The
/// stack exists once its change set executed, and its `Url` output is the one a stack names.
fn aws_fake() -> Arc<Fake> {
    let exists = Mutex::new(false);
    Arc::new(Fake::new(move |_method, url, body| {
        if url.ends_with("/sts/") {
            let xml = "<AssumeRoleResponse><AssumeRoleResult><Credentials><AccessKeyId>ASIA1</AccessKeyId><SecretAccessKey>sess-secret</SecretAccessKey><SessionToken>tok</SessionToken><Expiration>2030-01-01T00:00:00Z</Expiration></Credentials></AssumeRoleResult></AssumeRoleResponse>";
            return (200, Value::String(xml.to_string()));
        }
        let action = match body {
            Some(Body::Bytes(_, bytes)) => form_action(bytes),
            _ => String::new(),
        };
        let mut present = exists.lock().unwrap();
        match action.as_str() {
            "DescribeStacks" if !*present => (400, Value::String("<ErrorResponse><Error><Message>Stack with id stk does not exist</Message></Error></ErrorResponse>".into())),
            "DescribeStacks" => (200, Value::String("<DescribeStacksResponse><Stacks><member><StackStatus>CREATE_COMPLETE</StackStatus><Outputs><member><OutputKey>Url</OutputKey><OutputValue>https://stack.test</OutputValue></member></Outputs></member></Stacks></DescribeStacksResponse>".into())),
            "CreateChangeSet" => (200, Value::String("<CreateChangeSetResponse><Id>arn:cs-1</Id></CreateChangeSetResponse>".into())),
            "DescribeChangeSet" => (200, Value::String("<DescribeChangeSetResponse><Status>CREATE_COMPLETE</Status><ExecutionStatus>EXECUTE_COMPLETE</ExecutionStatus></DescribeChangeSetResponse>".into())),
            "ExecuteChangeSet" => {
                *present = true;
                (200, Value::String("<ExecuteChangeSetResponse/>".into()))
            }
            "DeleteStack" => {
                *present = false;
                (200, Value::String("<DeleteStackResponse/>".into()))
            }
            other => (500, Value::String(format!("unexpected {other}"))),
        }
    }))
}

/// The `Action` of a CloudFormation form body.
fn form_action(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .split('&')
        .find_map(|pair| pair.strip_prefix("Action="))
        .unwrap_or("")
        .to_string()
}

// ---- A local HTTP server standing in for an instance's address ------------------------------------------------------

/// Answers every request with 200 and "ok" until dropped. Its address is what an instance's URL is set to.
pub(crate) struct Site {
    pub(crate) base: String,
    stop: Arc<AtomicBool>,
}

impl Site {
    pub(crate) fn start() -> Site {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        listener.set_nonblocking(true).expect("a listener");
        let base = format!("http://{}", listener.local_addr().expect("an address"));
        let stop = Arc::new(AtomicBool::new(false));
        let halt = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !halt.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let mut reader = BufReader::new(stream.try_clone().expect("a clone"));
                        loop {
                            let mut line = String::new();
                            if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                                break;
                            }
                        }
                        let _ = stream.write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                        );
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(2)),
                }
            }
        });
        Site { base, stop }
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

// ---- Tests ----------------------------------------------------------------------------------------------------------

#[test]
fn a_lane_comes_up_in_its_own_namespace_and_is_torn_down_last() {
    let cluster = FakeKube::start(healthy);
    let mut fakes = Fakes::new(kube_env(&[]));
    fakes.kube = Some(cluster.base.clone());
    fakes.secrets = vec![("API_KEY".into(), "s3cr3t-value".into())];
    let store = Store::default();

    let up = up_with(
        &store,
        &fakes,
        UpRequest {
            project: "shop".into(),
            environment: "pr".into(),
            subject: lane("ch-1"),
            ttl_minutes: None,
            run: None,
            keep: false,
            built: BTreeMap::new(),
        },
    )
    .expect("the instance comes up");
    let ns = "apz-shop-lane-ch-1";
    assert_eq!(up.state, State::Ready);
    assert_eq!(up.url.as_deref(), Some("http://shop.test"));
    assert_eq!(
        up.target,
        Target::Kubernetes {
            namespace: ns.into(),
            deployment: "web".into()
        }
    );
    assert_eq!(
        cluster.trail(),
        kube_up_trail(ns),
        "the namespace, its applies and the rollout, in order"
    );

    // The bearer token is the hub's: every request carries it, and no instance record or manifest does.
    assert!(
        cluster
            .requests()
            .iter()
            .all(|r| r.auth.as_deref() == Some(&format!("Bearer {TOKEN}")[..]))
    );
    let secret = cluster
        .requests()
        .into_iter()
        .find(|r| r.path.ends_with("/secrets/apz-ephemeral-secrets"))
        .expect("the secret is applied");
    assert_eq!(secret.body["stringData"]["API_KEY"], json!("s3cr3t-value"));
    assert!(!serde_json::to_string(&up).unwrap().contains(TOKEN));

    // The record holds the target and the address, never the secret's value.
    let row = store::from_instance(&up).expect("a row");
    assert!(
        !serde_json::to_string(&row)
            .unwrap()
            .contains("s3cr3t-value")
    );

    down_with(&store, &fakes, &up.id).expect("it tears down");
    let trail = cluster.trail();
    assert_eq!(
        trail.last().map(String::as_str),
        Some(format!("DELETE /api/v1/namespaces/{ns}").as_str())
    );
    assert_eq!(
        store.get(&up.id).expect("the record stays").state,
        State::TornDown
    );
    // A second teardown is not an error and sends nothing.
    down_with(&store, &fakes, &up.id).expect("already torn down");
    assert_eq!(cluster.trail().len(), trail.len());
}

#[test]
fn a_release_is_deployed_with_its_pushed_image_pinned_by_digest() {
    let cluster = FakeKube::start(healthy);
    let mut fakes = Fakes::new(kube_env(&[]));
    fakes.kube = Some(cluster.base.clone());
    fakes.release = Some(release("rel-7"));
    let store = Store::default();

    let up = up_with(
        &store,
        &fakes,
        UpRequest {
            project: "shop".into(),
            environment: "pr".into(),
            subject: Subject::Release("rel-7".into()),
            ttl_minutes: Some(30),
            run: None,
            keep: false,
            built: BTreeMap::new(),
        },
    )
    .expect("the release comes up");
    assert_eq!(up.state, State::Ready);
    let deployment = cluster
        .requests()
        .into_iter()
        .find(|r| r.method == "PATCH" && r.path.ends_with("/deployments/web"))
        .expect("the deployment is applied");
    assert_eq!(
        deployment.body["spec"]["template"]["spec"]["containers"][0]["image"],
        json!(format!("registry.test/shop/api@{DIGEST}"))
    );
    assert_eq!(
        up.expires_ms - up.created_ms,
        30 * 60_000,
        "the request's minutes"
    );
}

#[test]
fn a_rollout_that_fails_removes_its_namespace_and_the_instance_says_why() {
    let cluster = FakeKube::start(|r: &Seen| {
        if r.method == "GET" && r.path.contains("/deployments/") {
            return (
                200,
                json!({
                    "metadata": { "name": "web", "generation": 1 },
                    "spec": { "replicas": 1 },
                    "status": { "observedGeneration": 1, "replicas": 1, "updatedReplicas": 0, "availableReplicas": 0,
                        "conditions": [{ "reason": "ProgressDeadlineExceeded", "message": "image pull back-off" }] }
                }),
            );
        }
        healthy(r)
    });
    let mut fakes = Fakes::new(kube_env(&[]));
    fakes.kube = Some(cluster.base.clone());
    let store = Store::default();

    let err = up_with(
        &store,
        &fakes,
        UpRequest {
            project: "shop".into(),
            environment: "pr".into(),
            subject: lane("ch-2"),
            ttl_minutes: None,
            run: None,
            keep: false,
            built: BTreeMap::new(),
        },
    )
    .expect_err("the rollout fails");
    assert!(err.contains("image pull back-off"), "{err}");
    let ns = "apz-shop-lane-ch-2";
    assert_eq!(
        cluster.trail().last().map(String::as_str),
        Some(format!("DELETE /api/v1/namespaces/{ns}").as_str())
    );
    let listed = store.list("shop");
    assert_eq!(listed.len(), 1);
    assert!(matches!(&listed[0].state, State::Failed(why) if why.contains("image pull back-off")));
}

#[test]
fn a_pages_preview_is_deployed_on_its_own_branch_and_its_deployment_removed() {
    let mut fakes = Fakes::new(environment(
        "pr",
        &[
            ("EPHEMERAL", "true"),
            ("CLOUDFLARE_PAGES", "shop-site"),
            ("DEPLOY_ARTIFACT", "web"),
        ],
    ));
    let fake = cloudflare_fake();
    fakes.cloudflare = Some(Arc::clone(&fake));
    fakes.artifact = b"<html>preview</html>".to_vec();
    let store = Store::default();

    let up = up_with(
        &store,
        &fakes,
        UpRequest {
            project: "shop".into(),
            environment: "pr".into(),
            subject: lane("ch-3"),
            ttl_minutes: None,
            run: None,
            keep: false,
            built: BTreeMap::new(),
        },
    )
    .expect("the preview comes up");
    assert_eq!(up.url.as_deref(), Some("https://dep-1.shop-site.pages.dev"));
    let Target::CloudflarePages {
        branch, deployment, ..
    } = &up.target
    else {
        panic!("a Pages target: {:?}", up.target);
    };
    assert_eq!(
        deployment, "dep-1",
        "the uploaded deployment is recorded for its removal"
    );
    assert!(branch.starts_with("apz-lane-ch-3-"), "{branch}");
    assert!(fake.urls().iter().any(|u| u.starts_with("POST ") && u.ends_with("/pages/projects/shop-site/deployments")));

    down_with(&store, &fakes, &up.id).expect("it tears down");
    assert!(
        fake.urls()
            .last()
            .unwrap()
            .ends_with("/pages/projects/shop-site/deployments/dep-1?force=true"),
        "{:?}",
        fake.urls()
    );
}

#[test]
fn a_worker_script_of_its_own_is_put_under_its_name_and_deleted() {
    let mut fakes = Fakes::new(environment(
        "pr",
        &[
            ("EPHEMERAL", "true"),
            ("CLOUDFLARE_WORKER", "shop-api"),
            ("EPHEMERAL_WORKER", "shop-api-{subject}"),
        ],
    ));
    let fake = cloudflare_fake();
    fakes.cloudflare = Some(Arc::clone(&fake));
    let store = Store::default();

    let up = up_with(
        &store,
        &fakes,
        UpRequest {
            project: "shop".into(),
            environment: "pr".into(),
            subject: lane("ch-4"),
            ttl_minutes: None,
            run: None,
            keep: false,
            built: BTreeMap::new(),
        },
    )
    .expect("the script comes up");
    assert_eq!(
        up.target,
        Target::CloudflareWorker {
            script: "shop-api-lane-ch-4".into()
        }
    );
    assert!(
        fake.urls().iter().any(|u| u
            == "PUT https://cf.test/client/v4/accounts/acct-1/workers/scripts/shop-api-lane-ch-4"),
        "{:?}",
        fake.urls()
    );
    down_with(&store, &fakes, &up.id).expect("it tears down");
    assert!(
        fake.urls()
            .last()
            .unwrap()
            .ends_with("/workers/scripts/shop-api-lane-ch-4")
    );
}

#[test]
fn a_cloudformation_stack_instance_is_applied_reads_its_url_and_is_deleted() {
    let mut fakes = Fakes::new(environment(
        "pr",
        &[("EPHEMERAL", "true"), ("AWS_CLOUDFORMATION_STACK", "shop")],
    ));
    let fake = aws_fake();
    fakes.aws = Some(Arc::clone(&fake));
    fakes.artifact = b"AWSTemplateFormatVersion: '2010-09-09'\n".to_vec();
    let store = Store::default();

    let up = up_with(
        &store,
        &fakes,
        UpRequest {
            project: "shop".into(),
            environment: "pr".into(),
            subject: lane("ch-5"),
            ttl_minutes: None,
            run: None,
            keep: false,
            built: BTreeMap::new(),
        },
    )
    .expect("the stack comes up");
    assert_eq!(up.url.as_deref(), Some("https://stack.test"));
    assert_eq!(
        up.target,
        Target::AwsStack {
            stack: "apz-shop-lane-ch-5".into(),
            region: None
        }
    );
    let actions: Vec<String> = fake
        .seen()
        .iter()
        .filter_map(|(_, _, _, body)| match body {
            Some(Body::Bytes(_, bytes)) => Some(form_action(bytes)),
            _ => None,
        })
        .collect();
    assert!(
        actions
            .windows(2)
            .any(|w| w == ["CreateChangeSet", "DescribeChangeSet"]),
        "{actions:?}"
    );
    assert!(actions.contains(&"ExecuteChangeSet".to_string()));

    down_with(&store, &fakes, &up.id).expect("the stack is deleted");
    let last: Vec<String> = fake
        .seen()
        .iter()
        .rev()
        .take(3)
        .filter_map(|(_, _, _, b)| match b {
            Some(Body::Bytes(_, bytes)) => Some(form_action(bytes)),
            _ => None,
        })
        .collect();
    assert!(last.contains(&"DeleteStack".to_string()), "{last:?}");
}

#[test]
fn the_ticker_tears_down_an_instance_whose_time_is_up_and_leaves_the_others() {
    let cluster = FakeKube::start(healthy);
    let mut fakes = Fakes::new(kube_env(&[]));
    fakes.kube = Some(cluster.base.clone());
    let store = Store::default();
    let short = up_with(
        &store,
        &fakes,
        UpRequest {
            project: "shop".into(),
            environment: "pr".into(),
            subject: lane("short"),
            ttl_minutes: Some(5),
            run: None,
            keep: false,
            built: BTreeMap::new(),
        },
    )
    .expect("the short one comes up");
    let long = up_with(
        &store,
        &fakes,
        UpRequest {
            project: "shop".into(),
            environment: "pr".into(),
            subject: lane("long"),
            ttl_minutes: Some(120),
            run: None,
            keep: false,
            built: BTreeMap::new(),
        },
    )
    .expect("the long one comes up");

    fakes.advance(10);
    let notes = sweep(&store, &fakes, fakes.now());
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(
        notes[0].contains(&short.id) && notes[0].contains("torn down"),
        "{notes:?}"
    );
    assert_eq!(store.get(&short.id).unwrap().state, State::TornDown);
    assert_eq!(store.get(&long.id).unwrap().state, State::Ready);
    assert!(
        cluster
            .trail()
            .contains(&"DELETE /api/v1/namespaces/apz-shop-lane-short".to_string())
    );
    assert!(
        !cluster
            .trail()
            .iter()
            .any(|t| t.contains("apz-shop-lane-long") && t.starts_with("DELETE"))
    );
}

#[test]
fn an_environment_that_is_not_ephemeral_or_a_subject_not_a_name_is_refused_before_anything_is_made()
{
    let fakes = Fakes::new(environment("pr", &[("KUBERNETES_DEPLOYMENT", "web")]));
    let store = Store::default();
    let err = up_with(
        &store,
        &fakes,
        UpRequest {
            project: "shop".into(),
            environment: "pr".into(),
            subject: lane("ch-6"),
            ttl_minutes: None,
            run: None,
            keep: false,
            built: BTreeMap::new(),
        },
    )
    .expect_err("not ephemeral");
    assert!(err.contains("EPHEMERAL"), "{err}");

    let fakes = Fakes::new(kube_env(&[]));
    let err = up_with(
        &store,
        &fakes,
        UpRequest {
            project: "shop".into(),
            environment: "pr".into(),
            subject: lane("ch 6/../x"),
            ttl_minutes: None,
            run: None,
            keep: false,
            built: BTreeMap::new(),
        },
    )
    .expect_err("a subject with a slash");
    assert!(err.contains("subject"), "{err}");
    assert!(store.list("shop").is_empty(), "nothing was recorded");
}

#[test]
fn the_ttl_comes_from_the_request_then_the_environment_then_the_default_and_is_bounded() {
    let env = environment(
        "pr",
        &[("EPHEMERAL", "true"), ("EPHEMERAL_TTL_MINUTES", "15")],
    );
    assert_eq!(super::ttl_minutes(&env, None), Ok(15));
    assert_eq!(super::ttl_minutes(&env, Some(90)), Ok(90));
    let plain = environment("pr", &[("EPHEMERAL", "true")]);
    assert_eq!(
        super::ttl_minutes(&plain, None),
        Ok(super::DEFAULT_TTL_MINUTES)
    );
    assert!(super::ttl_minutes(&plain, Some(0)).is_err());
    assert!(super::ttl_minutes(&plain, Some(super::MAX_TTL_MINUTES + 1)).is_err());
    let bad = environment(
        "pr",
        &[("EPHEMERAL", "true"), ("EPHEMERAL_TTL_MINUTES", "soon")],
    );
    assert!(super::ttl_minutes(&bad, None).is_err());
}

#[test]
fn the_name_patterns_expand_and_a_name_the_platform_would_refuse_says_which_pattern_to_change() {
    let env = environment(
        "pr",
        &[
            ("EPHEMERAL", "true"),
            ("KUBERNETES_DEPLOYMENT", "web"),
            ("EPHEMERAL_NAMESPACE", "{project}-{subject}-{instance}"),
        ],
    );
    let target =
        platform::plan(&env, "Shop", &lane("CH_7.x"), "eph-0123456789abcdef").expect("a plan");
    assert_eq!(
        target,
        Target::Kubernetes {
            namespace: "shop-lane-ch-7-x-01234567".into(),
            deployment: "web".into()
        }
    );

    let long = environment(
        "pr",
        &[
            ("EPHEMERAL", "true"),
            ("KUBERNETES_DEPLOYMENT", "web"),
            (
                "EPHEMERAL_NAMESPACE",
                "{project}-{subject}-{project}-{subject}-{project}",
            ),
        ],
    );
    let err = platform::plan(
        &long,
        "a-very-long-project-name-for-the-test",
        &lane("ch-8"),
        "eph-1",
    )
    .expect_err("too long");
    assert!(err.contains("EPHEMERAL_NAMESPACE"), "{err}");

    assert_eq!(slug(&Subject::Release("rel-9".into())), "release-rel-9");
}

#[test]
fn an_against_stage_runs_a_test_command_and_a_web_check_then_tears_the_instance_down() {
    let site = Site::start();
    let cluster = FakeKube::start(healthy);
    let mut fakes = Fakes::new(kube_env(&[("EPHEMERAL_URL", site.base.as_str())]));
    fakes.kube = Some(cluster.base.clone());
    let store = Store::default();
    let req = Against {
        project: "shop".into(),
        environment: "pr".into(),
        subject: lane("ch-9"),
        paths: vec!["/".into(), "/health".into()],
        keep: false,
        ttl_minutes: None,
    };
    let verdict = against::run(
        &store,
        &fakes,
        &req,
        up_with(
            &store,
            &fakes,
            UpRequest {
                project: "shop".into(),
                environment: "pr".into(),
                subject: lane("ch-9"),
                ttl_minutes: None,
                run: None,
                keep: false,
                built: BTreeMap::new(),
            },
        ),
        &mut |url| {
            // The test command: it sees the instance's address in APZ_TARGET_URL.
            let status = std::process::Command::new("sh")
                .args(["-c", "test \"$APZ_TARGET_URL\" = \"$WANT\""])
                .env("APZ_TARGET_URL", url)
                .env("WANT", url)
                .status()
                .map_err(|e| e.to_string())?;
            if status.success() {
                Ok("the test command passed".to_string())
            } else {
                Err("the test command failed".to_string())
            }
        },
    );
    assert!(verdict.passed, "{:?}", verdict.lines);
    assert!(
        verdict.lines.iter().any(|l| l == "GET /: 200"),
        "{:?}",
        verdict.lines
    );
    assert!(
        verdict.lines.iter().any(|l| l == "GET /health: 200"),
        "{:?}",
        verdict.lines
    );
    assert!(
        verdict
            .lines
            .iter()
            .any(|l| l.starts_with("torn down: eph-")),
        "{:?}",
        verdict.lines
    );
    let torn = verdict.instance.expect("the instance");
    assert_eq!(torn.state, State::TornDown);
    assert!(
        cluster
            .trail()
            .last()
            .unwrap()
            .starts_with("DELETE /api/v1/namespaces/apz-shop-lane-ch-9")
    );
}

#[test]
fn an_against_stage_keeps_its_instance_when_the_run_asks_and_fails_when_a_check_does() {
    let cluster = FakeKube::start(healthy);
    let mut fakes = Fakes::new(kube_env(&[("EPHEMERAL_URL", "http://127.0.0.1:9")]));
    fakes.kube = Some(cluster.base.clone());
    let store = Store::default();
    let req = Against {
        project: "shop".into(),
        environment: "pr".into(),
        subject: lane("ch-10"),
        paths: vec!["/".into()],
        keep: true,
        ttl_minutes: Some(20),
    };
    let up = up_with(
        &store,
        &fakes,
        UpRequest {
            project: "shop".into(),
            environment: "pr".into(),
            subject: lane("ch-10"),
            ttl_minutes: Some(20),
            run: None,
            keep: false,
            built: BTreeMap::new(),
        },
    );
    let verdict = against::run(&store, &fakes, &req, up, &mut |_| Ok("unused".into()));
    assert!(
        !verdict.passed,
        "nothing answers on port 9: {:?}",
        verdict.lines
    );
    assert!(
        verdict.lines.iter().any(|l| l.starts_with("GET /: ")),
        "{:?}",
        verdict.lines
    );
    let kept = verdict.instance.expect("the instance");
    assert_eq!(kept.state, State::Ready, "kept: not torn down");
    assert!(!cluster.trail().iter().any(|t| t.starts_with("DELETE")));
}

#[test]
fn the_store_rows_round_trip_and_a_project_keeps_its_newest_instances() {
    let store = Store::default();
    let instance = Instance {
        id: "eph-0000000000000001".into(),
        project: "shop".into(),
        environment: "pr".into(),
        subject: lane("ch-11"),
        target: Target::AwsStack {
            stack: "s".into(),
            region: Some("eu-west-1".into()),
        },
        url: Some("https://x.test".into()),
        state: State::Failed("why".into()),
        created_ms: 5,
        expires_ms: 9,
        run: None,
        keep: false,
        built: BTreeMap::new(),
    };
    store.insert(&instance).expect("inserted");
    assert_eq!(store.get(&instance.id), Some(instance.clone()));
    assert_eq!(
        store::to_instance(&store::from_instance(&instance).unwrap()).unwrap(),
        instance
    );
    store
        .set_state(&instance.id, &State::TornDown, None)
        .unwrap();
    assert_eq!(store.get(&instance.id).unwrap().state, State::TornDown);
    assert!(
        store.live().is_empty(),
        "a torn-down instance is no longer live"
    );
    assert_eq!(store.list("shop").len(), 1, "but its record is listed");
}

#[test]
fn the_subject_slugs_and_the_stage_variables_are_read_as_they_are_named() {
    assert_eq!(slug(&lane("CH.7")), "lane-ch-7");
    let env = environment(
        "pr",
        &[("EPHEMERAL", "true"), ("KUBERNETES_DEPLOYMENT", "web")],
    );
    assert!(super::is_ephemeral(&env));
    assert!(!super::is_ephemeral(&environment(
        "pr",
        &[("EPHEMERAL", "false")]
    )));
    assert_eq!(against::TARGET_ENV, "APZ_TARGET_URL");
}

#[test]
fn a_deploy_the_hub_stopped_in_the_middle_of_is_failed_on_load_and_an_instance_never_takes_the_own_name()
 {
    let mut rows = vec![
        store::from_instance(&Instance {
            id: "eph-0000000000000002".into(),
            project: "shop".into(),
            environment: "pr".into(),
            subject: lane("ch-12"),
            target: Target::CloudflareWorker { script: "x".into() },
            url: None,
            state: State::Creating,
            created_ms: 1,
            expires_ms: 2,
            run: None,
            keep: false,
            built: BTreeMap::new(),
        })
        .unwrap(),
    ];
    store::recover(&mut rows);
    assert_eq!(rows[0].state, "failed");
    assert!(rows[0].reason.contains("stopped while it was deploying"));

    let own = environment(
        "pr",
        &[
            ("EPHEMERAL", "true"),
            ("KUBERNETES_DEPLOYMENT", "web"),
            ("EPHEMERAL_NAMESPACE", "default"),
        ],
    );
    let err =
        platform::plan(&own, "shop", &lane("ch-13"), "eph-1").expect_err("the default namespace");
    assert!(err.contains("KUBERNETES_NAMESPACE"), "{err}");
    let pages = environment(
        "pr",
        &[
            ("EPHEMERAL", "true"),
            ("CLOUDFLARE_PAGES", "shop-site"),
            ("EPHEMERAL_BRANCH", "main"),
        ],
    );
    assert!(
        platform::plan(&pages, "shop", &lane("ch-13"), "eph-1").is_err(),
        "the production branch is not an instance's"
    );
}

#[test]
fn a_failed_teardown_is_retried_on_a_backoff_and_the_ticker_gives_up_at_the_cap() {
    let cluster = FakeKube::start(healthy);
    let mut fakes = Fakes::new(kube_env(&[]));
    fakes.kube = Some(cluster.base.clone());
    let store = Store::default();
    let stuck = up_with(
        &store,
        &fakes,
        UpRequest {
            project: "shop".into(),
            environment: "pr".into(),
            subject: lane("stuck"),
            ttl_minutes: Some(5),
            run: None,
            keep: false,
            built: BTreeMap::new(),
        },
    )
    .expect("the instance comes up");

    // The platform becomes unreachable: nothing listens where the removal calls go. The time is up, and the ticker runs
    // every minute for twelve hours (a fake clock; each pass advances it one minute).
    fakes.kube = Some("http://127.0.0.1:1".into());
    fakes.advance(10);
    let start = fakes.now();
    let mut tried = Vec::new();
    for _ in 0..720 {
        let notes = sweep(&store, &fakes, fakes.now());
        if notes
            .iter()
            .any(|n| n.contains(&stuck.id) && n.contains("not torn down"))
        {
            tried.push((fakes.now() - start) / 60_000);
        }
        if notes.iter().any(|n| n.contains("gave up")) {
            assert_eq!(tried.len(), 12, "the twelfth try is the last");
        }
        fakes.advance(1);
    }
    // The waits double from one minute to the hour's cap, and the twelfth try ends the retries.
    assert_eq!(tried, vec![0, 1, 3, 7, 15, 31, 63, 123, 183, 243, 303, 363]);
    assert_eq!(store.retry_of(&stuck.id).unwrap().attempts, 12);
    assert_eq!(
        store.get(&stuck.id).unwrap().state,
        State::Ready,
        "the instance is left for the owner, not torn down"
    );

    // The owner's DELETE tries again, and once the platform answers the instance is gone and its retries forgotten.
    fakes.kube = Some(cluster.base.clone());
    down_with(&store, &fakes, &stuck.id).expect("the owner's teardown works");
    assert_eq!(store.get(&stuck.id).unwrap().state, State::TornDown);
    assert!(store.retry_of(&stuck.id).is_none());
}
