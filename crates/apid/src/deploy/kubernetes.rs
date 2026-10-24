//! The Kubernetes platform of a deploy (PLAN 23, 11.2): the release's image is taken from the release's pushed OCI image
//! (pinned by digest), the release's manifests are applied by server-side apply with the Deployment's image set to it, and
//! the rollout is watched until it completes or times out. A project may deploy a Helm chart instead ([`helm`]).
//!
//! A project that declares a canary (`KUBERNETES_CANARY`) runs a second Deployment beside the stable one. Each stage below
//! 100% gives the canary its share of the stable Deployment's replicas (at least one, never all), and the checks of the
//! health watch run after it. At 100% the release is promoted into the stable Deployment and the canary's replicas go. A
//! failed check or a breach puts everything back: the canary's replicas go to zero, the stable replicas return, and a
//! promoted stable Deployment is put back on its previous revision (`rollout undo`: the pod template of the revision the
//! deploy replaced, from its ReplicaSet). Without a canary the one stage is the whole rollout.
//!
//! The calls go to the cluster's API server, through the platforms' HTTP path. The bearer token is the `kubernetes`
//! connector's, read on the hub and attached to each request here, never in a fork, an error or a log. A private cluster
//! CA is the connector's `ca` (`platforms::http::PrivateCa`).
//!
//! The image: a release's registry row for the OCI registry (9.5) holds the pushed reference and its manifest digest, and
//! the deploy runs `<reference>@<digest>`. `KUBERNETES_IMAGE` only replaces the repository. A release with no pushed image
//! fails its stage with that reason.
//!
//! Environment variables (not secret): `KUBERNETES_DEPLOYMENT` (names the platform: the stable Deployment),
//! `KUBERNETES_IMAGE` (an image repository override), `KUBERNETES_NAMESPACE` (`default`), `KUBERNETES_CONTAINER` (the
//! Deployment's first container), `KUBERNETES_CANARY`, `KUBERNETES_ARTIFACT` (`manifests`: the release artifact, a YAML
//! stream), `KUBERNETES_TIMEOUT_SECS` (300), `KUBERNETES_HELM_CHART` (a Helm chart to deploy instead of the manifests) and
//! `KUBERNETES_HELM_RELEASE` (the release's name, the Deployment's by default), and the health watch's variables
//! ([`super::Settings::watch_of`]: `ROLLOUT_STAGES` is the canary's stages).
//!
//! The manifests are the stream's own documents. A `kind: Kustomization` document in it is an overlay, built by `kubectl
//! kustomize` (or `kustomize build`) when one is on the hub's PATH (detected, never installed). Without either, the overlay's
//! namespace and images are resolved here, and its fields that only kustomize builds are refused.

pub mod helm;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{Value, json};

use super::{Artifacts, Checks, Incidents, Rolled, Stop};
use crate::connectors::encode;
use crate::environments::Environment;
use crate::platforms::http::{Body, Http, Reply};
use crate::recipes::graph::Report;
use crate::recipes::runs::Runs;
use crate::releases::entity::Release;

/// The annotation a ReplicaSet and a Deployment carry with their revision number.
const REVISION: &str = "deployment.kubernetes.io/revision";
/// The field manager of the deploy's server-side applies: a conflict with another manager is taken over.
const FIELD_MANAGER: &str = "apz-deploy";
/// The label a canary's pods and selector carry, which the stable Deployment's do not.
const TRACK: &str = "apz.deploy/track";
const CANARY: &str = "canary";
const JSON: &str = "application/json";
const APPLY: &str = "application/apply-patch+yaml";
const MERGE: &str = "application/merge-patch+json";
const STRATEGIC: &str = "application/strategic-merge-patch+json";
const DEFAULT_NAMESPACE: &str = "default";
const DEFAULT_ARTIFACT: &str = "manifests";
const DEFAULT_TIMEOUT_SECS: u64 = 300;
const MAX_TIMEOUT_SECS: u64 = 3600;
/// How often a rollout's status is read.
const POLL: Duration = Duration::from_secs(5);
/// The OCI registry's connector: its sync rows (9.5) name a release's pushed image.
const OCI_PLATFORM: &str = "oci-registry/";
/// Kinds without a namespace.
const CLUSTER_SCOPED: [&str; 8] = [
    "Namespace",
    "ClusterRole",
    "ClusterRoleBinding",
    "CustomResourceDefinition",
    "PersistentVolume",
    "StorageClass",
    "IngressClass",
    "PriorityClass",
];
/// Overlay fields that kustomize builds and the fallback resolver does not.
const UNBUILT_OVERLAY_FIELDS: [&str; 9] = [
    "commonLabels",
    "patches",
    "patchesStrategicMerge",
    "configMapGenerator",
    "secretGenerator",
    "components",
    "replacements",
    "namePrefix",
    "nameSuffix",
];

/// A Kubernetes environment's rollout settings.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// The stable Deployment: its image is the release's, and its previous revision is what a rollback returns to.
    pub deployment: String,
    pub namespace: String,
    /// The container whose image is set; the Deployment's first when none is named.
    pub container: Option<String>,
    /// A second Deployment that carries each stage's share of the replicas before the promotion.
    pub canary: Option<String>,
    /// An image repository that replaces the release's (`KUBERNETES_IMAGE`); the release's digest is kept.
    pub image: Option<String>,
    /// The release artifact the manifests are read from.
    pub artifact: String,
    /// A Helm chart to deploy instead of the manifests, and the release's name.
    pub chart: Option<String>,
    pub helm_release: String,
    /// How long one Deployment (or the helm release) may take to complete its rollout.
    pub timeout: Duration,
    pub poll: Duration,
    /// The stages and the health watch (shared with the Cloudflare platform).
    pub watch: super::Settings,
}

impl Settings {
    /// The settings of an environment, or None when it names no Kubernetes Deployment (it keeps another platform or its hook).
    pub fn of(env: &Environment) -> Result<Option<Settings>, String> {
        let Some(deployment) = super::variable(env, "KUBERNETES_DEPLOYMENT") else {
            return Ok(None);
        };
        if !dns_name(&deployment) {
            return Err(
                "KUBERNETES_DEPLOYMENT is a Deployment's name: lower-case letters, digits and dashes".into(),
            );
        }
        let image = super::variable(env, "KUBERNETES_IMAGE");
        if let Some(repository) = &image
            && (repository.chars().any(char::is_whitespace) || repository.contains('@'))
        {
            return Err(
                "KUBERNETES_IMAGE is an image repository, without a tag or a digest".into(),
            );
        }
        let namespace = super::variable(env, "KUBERNETES_NAMESPACE")
            .unwrap_or_else(|| DEFAULT_NAMESPACE.into());
        if !dns_name(&namespace) {
            return Err("KUBERNETES_NAMESPACE is a namespace's name".into());
        }
        let canary = super::variable(env, "KUBERNETES_CANARY");
        if let Some(canary) = &canary
            && (!dns_name(canary) || *canary == deployment)
        {
            return Err("KUBERNETES_CANARY names a second Deployment, not the stable one".into());
        }
        let chart = super::variable(env, "KUBERNETES_HELM_CHART");
        if chart.is_some() && canary.is_some() {
            return Err(
                "KUBERNETES_CANARY is not run with a Helm chart: a Helm release has one stage"
                    .into(),
            );
        }
        let timeout = match super::variable(env, "KUBERNETES_TIMEOUT_SECS") {
            Some(text) => text
                .parse::<u64>()
                .ok()
                .filter(|s| (1..=MAX_TIMEOUT_SECS).contains(s))
                .map(Duration::from_secs)
                .ok_or_else(|| {
                    format!("KUBERNETES_TIMEOUT_SECS is a number of seconds from 1 to {MAX_TIMEOUT_SECS}")
                })?,
            None => Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        };
        Ok(Some(Settings {
            helm_release: super::variable(env, "KUBERNETES_HELM_RELEASE")
                .unwrap_or_else(|| deployment.clone()),
            deployment,
            namespace,
            container: super::variable(env, "KUBERNETES_CONTAINER"),
            canary,
            image,
            artifact: super::variable(env, "KUBERNETES_ARTIFACT")
                .unwrap_or_else(|| DEFAULT_ARTIFACT.to_string()),
            chart,
            timeout,
            poll: POLL,
            watch: super::Settings::watch_of(env)?,
        }))
    }

    /// The release's image: its pushed OCI reference, pinned by the digest the registry row holds. `KUBERNETES_IMAGE`, when
    /// set, replaces the repository and keeps the digest.
    pub fn image_for(&self, release: &Release) -> Result<String, String> {
        let reference = release
            .sync
            .iter()
            .find(|s| s.state == "published" && s.platform.starts_with(OCI_PLATFORM))
            .and_then(|s| s.url.as_deref())
            .ok_or_else(|| {
                format!(
                    "{} has no pushed image: no OCI registry row of it is published, so there is no image to deploy",
                    release.version
                )
            })?;
        let reference = reference.trim_start_matches("https://");
        let (named, digest) = reference
            .rsplit_once('@')
            .filter(|(_, d)| d.starts_with("sha256:") && d.len() == "sha256:".len() + 64)
            .ok_or_else(|| {
                format!(
                    "{}'s pushed image {reference} is not pinned by a sha256 digest",
                    release.version
                )
            })?;
        let repository = match &self.image {
            Some(repository) => repository.clone(),
            None => match named.rsplit_once(':') {
                Some((repository, tag)) if !tag.contains('/') => repository.to_string(),
                _ => named.to_string(),
            },
        };
        Ok(format!("{repository}@{digest}"))
    }
}

/// A DNS-1123 label: what a Deployment, a namespace or a canary is named.
fn dns_name(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 63
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !text.starts_with('-')
        && !text.ends_with('-')
}

/// A name or an API group version a request path carries: nothing that could leave the path.
fn safe(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._/".contains(c))
}

/// A name for a private temporary file or directory: unique to this process and call.
pub fn private_name(label: &str) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!(
        "apz-{label}-{}-{nanos}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// The API server's client: its address, its CA and the connector's bearer token, through the platforms' HTTP path.
pub struct Client {
    http: Arc<dyn Http>,
    base: String,
    ca: Option<String>,
    token: String,
}

impl Client {
    /// A client for the API server at `base` (`http://` or `https://`) with the connector's `token`.
    pub fn new(http: Arc<dyn Http>, base: &str, token: &str) -> Result<Client, String> {
        let base = base.trim().trim_end_matches('/');
        if !(base.starts_with("https://") || base.starts_with("http://")) {
            return Err(
                "the Kubernetes connector's address is the API server's https address".into(),
            );
        }
        let token = token.trim();
        if token.is_empty() {
            return Err("the Kubernetes connector holds no bearer token".into());
        }
        Ok(Client {
            http,
            base: base.to_string(),
            ca: None,
            token: token.to_string(),
        })
    }

    /// The cluster's CA certificate (PEM), which helm's kubeconfig trusts; without one, the system roots.
    pub fn with_ca(mut self, ca: Option<String>) -> Client {
        self.ca = ca;
        self
    }

    /// The kubeconfig helm reads for one call: the server, its CA and the token. It is written to a private file for
    /// that call only ([`helm`]).
    fn kubeconfig(&self) -> Value {
        let mut cluster = json!({ "server": self.base });
        if let Some(ca) = &self.ca {
            cluster["certificate-authority-data"] = json!(crate::connectors::base64(ca.as_bytes()));
        }
        json!({
            "apiVersion": "v1",
            "kind": "Config",
            "clusters": [{ "name": "hub", "cluster": cluster }],
            "users": [{ "name": "hub", "user": { "token": self.token } }],
            "contexts": [{ "name": "hub", "context": { "cluster": "hub", "user": "hub" } }],
            "current-context": "hub",
        })
    }

    /// One call: `path` on the API server, with a patch body of its media type when there is one. The token goes in
    /// the header only.
    fn call(
        &self,
        method: &str,
        path: &str,
        patch: Option<(&str, &Value)>,
    ) -> Result<Reply, String> {
        let url = format!("{}{path}", self.base);
        let headers = [("Authorization", format!("Bearer {}", self.token))];
        let body = patch.map(|(media, doc)| {
            Body::Bytes(
                media.to_string(),
                serde_json::to_vec(doc).unwrap_or_default(),
            )
        });
        self.http
            .call(method, &url, &headers, body)
            .map_err(|e| self.redact(&e))
    }

    /// A call whose 2xx answer is its body; anything else is the API server's own message.
    fn json(
        &self,
        method: &str,
        path: &str,
        patch: Option<(&str, &Value)>,
    ) -> Result<Value, String> {
        let reply = self.call(method, path, patch)?;
        let what = format!("{method} {}", path.split('?').next().unwrap_or(path));
        reply.ok(&what).map_err(|e| self.redact(&e))
    }

    /// Text with the token taken out, for any error that might carry it.
    fn redact(&self, text: &str) -> String {
        text.replace(&self.token, "[redacted]")
    }

    /// The stable Deployment as the API server holds it, or None when it does not exist.
    pub fn deployment(&self, namespace: &str, name: &str) -> Result<Option<Value>, String> {
        let reply = self.call("GET", &deployment_path(namespace, name), None)?;
        if reply.status == 404 {
            return Ok(None);
        }
        reply
            .ok(&format!("GET deployments/{name}"))
            .map(Some)
            .map_err(|e| self.redact(&e))
    }

    /// Server-side apply of one manifest (its kind, name and namespace name its path), taking over its fields.
    pub fn apply(&self, doc: &Value) -> Result<(), String> {
        let path = format!(
            "{}?fieldManager={FIELD_MANAGER}&force=true",
            resource_path(doc)?
        );
        self.json("PATCH", &path, Some((APPLY, doc))).map(|_| ())
    }

    /// Sets a Deployment's replicas.
    pub fn scale(&self, namespace: &str, name: &str, replicas: u32) -> Result<(), String> {
        let patch = json!({ "spec": { "replicas": replicas } });
        self.json(
            "PATCH",
            &deployment_path(namespace, name),
            Some((MERGE, &patch)),
        )
        .map(|_| ())
    }

    /// Creates a namespace; one that is already there is not an error.
    pub fn create_namespace(&self, name: &str) -> Result<(), String> {
        let doc = json!({ "apiVersion": "v1", "kind": "Namespace", "metadata": { "name": name } });
        let reply = self.call("POST", "/api/v1/namespaces", Some((JSON, &doc)))?;
        if reply.status == 409 {
            return Ok(());
        }
        reply
            .ok(&format!("POST namespaces/{name}"))
            .map(|_| ())
            .map_err(|e| self.redact(&e))
    }

    /// Deletes a namespace and everything in it; one already gone is not an error.
    pub fn delete_namespace(&self, name: &str) -> Result<(), String> {
        let reply = self.call("DELETE", &format!("/api/v1/namespaces/{name}"), None)?;
        if reply.status == 404 {
            return Ok(());
        }
        reply
            .ok(&format!("DELETE namespaces/{name}"))
            .map(|_| ())
            .map_err(|e| self.redact(&e))
    }

    /// `rollout undo --to-revision`: the Deployment's pod template becomes the one its ReplicaSet of `revision` holds.
    pub fn undo(&self, namespace: &str, name: &str, revision: u64) -> Result<(), String> {
        let deploy = self.deployment(namespace, name)?.ok_or_else(|| {
            format!("{name} is not in {namespace}, so it has no revision to undo to")
        })?;
        let selector = encode(&label_selector(&deploy)?);
        let sets = self.json(
            "GET",
            &format!("/apis/apps/v1/namespaces/{namespace}/replicasets?labelSelector={selector}"),
            None,
        )?;
        let set = sets["items"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|rs| revision_of(rs) == Some(revision))
            .ok_or_else(|| format!("{name} has no ReplicaSet of revision {revision}"))?;
        let mut template = set["spec"]["template"].clone();
        if let Some(labels) = template["metadata"]["labels"].as_object_mut() {
            labels.remove("pod-template-hash");
        }
        let patch = json!({ "spec": { "template": template } });
        self.json(
            "PATCH",
            &deployment_path(namespace, name),
            Some((STRATEGIC, &patch)),
        )
        .map(|_| ())
    }
}

/// The tools the hub may run, found on the PATH it is given. They are detected, never installed.
pub struct Tools {
    path: String,
}

impl Tools {
    /// The hub's own PATH.
    pub fn from_env() -> Tools {
        Tools {
            path: std::env::var("PATH").unwrap_or_default(),
        }
    }

    /// The tools on `path` (a PATH-style list of directories).
    #[cfg(test)]
    pub fn on(path: &str) -> Tools {
        Tools {
            path: path.to_string(),
        }
    }

    /// The first executable file called `name` on the path.
    pub fn find(&self, name: &str) -> Option<PathBuf> {
        self.path
            .split(':')
            .filter(|dir| !dir.is_empty())
            .map(|dir| Path::new(dir).join(name))
            .find(|candidate| is_executable(candidate))
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// A private scratch directory, removed when it goes out of scope.
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Result<Scratch, String> {
        let dir = std::env::temp_dir().join(private_name(label));
        std::fs::create_dir(&dir)
            .map_err(|e| format!("the hub could not make {label}'s directory: {e}"))?;
        Ok(Scratch(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn deployment_path(namespace: &str, name: &str) -> String {
    format!("/apis/apps/v1/namespaces/{namespace}/deployments/{name}")
}

/// The Deployment's selector as a label selector (`k=v,k2=v2`).
fn label_selector(deploy: &Value) -> Result<String, String> {
    let labels = deploy["spec"]["selector"]["matchLabels"]
        .as_object()
        .ok_or("the Deployment has no selector labels")?;
    Ok(labels
        .iter()
        .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or("")))
        .collect::<Vec<_>>()
        .join(","))
}

/// The path of a manifest's resource: its group and version, its namespace (when it has one), its plural kind and name.
fn resource_path(doc: &Value) -> Result<String, String> {
    let api = doc["apiVersion"]
        .as_str()
        .filter(|a| safe(a))
        .ok_or("a manifest has no apiVersion it can be applied under")?;
    let kind = kind_of(doc);
    let name = doc["metadata"]["name"]
        .as_str()
        .filter(|n| safe(n))
        .ok_or_else(|| format!("a {kind} has no name"))?;
    if kind.is_empty() {
        return Err("a manifest has no kind".into());
    }
    let base = if api.contains('/') {
        format!("/apis/{api}")
    } else {
        format!("/api/{api}")
    };
    let scope = match doc["metadata"]["namespace"].as_str() {
        Some(ns) if safe(ns) && !CLUSTER_SCOPED.contains(&kind) => format!("/namespaces/{ns}"),
        _ => String::new(),
    };
    Ok(format!("{base}{scope}/{}/{name}", plural(kind)))
}

/// A kind's plural resource name: `Deployment` to `deployments`, `Ingress` to `ingresses`, `NetworkPolicy` to `networkpolicies`.
fn plural(kind: &str) -> String {
    let lower = kind.to_lowercase();
    if lower == "endpoints" {
        return lower;
    }
    if lower.ends_with('s') {
        return format!("{lower}es");
    }
    match lower.strip_suffix('y') {
        Some(stem) if !stem.ends_with(['a', 'e', 'i', 'o', 'u']) => format!("{stem}ies"),
        _ => format!("{lower}s"),
    }
}

fn kind_of(doc: &Value) -> &str {
    doc["kind"].as_str().unwrap_or("")
}

fn revision_of(doc: &Value) -> Option<u64> {
    doc["metadata"]["annotations"][REVISION]
        .as_str()?
        .parse()
        .ok()
}

fn replicas_of(doc: &Value) -> u32 {
    u32::try_from(doc["spec"]["replicas"].as_u64().unwrap_or(1)).unwrap_or(u32::MAX)
}

/// Where a Deployment's rollout stands.
#[derive(Debug, PartialEq)]
enum Progress {
    Done,
    Waiting(String),
    Failed(String),
}

/// The status a Deployment's controller reports, read as `kubectl rollout status` reads it.
fn progress(deploy: &Value) -> Progress {
    let status = &deploy["status"];
    let deadline = status["conditions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|c| c["reason"] == "ProgressDeadlineExceeded");
    if let Some(condition) = deadline {
        let why = condition["message"]
            .as_str()
            .unwrap_or("the progress deadline passed");
        return Progress::Failed(why.to_string());
    }
    let generation = deploy["metadata"]["generation"].as_u64().unwrap_or(0);
    if status["observedGeneration"].as_u64().unwrap_or(0) < generation {
        return Progress::Waiting("the controller has not seen the new spec yet".into());
    }
    let want = deploy["spec"]["replicas"].as_u64().unwrap_or(1);
    let updated = status["updatedReplicas"].as_u64().unwrap_or(0);
    let available = status["availableReplicas"].as_u64().unwrap_or(0);
    let total = status["replicas"].as_u64().unwrap_or(0);
    if updated < want {
        Progress::Waiting(format!("{updated} of {want} replicas updated"))
    } else if available < want {
        Progress::Waiting(format!("{available} of {want} replicas available"))
    } else if total > updated {
        Progress::Waiting(format!("{} old replicas still running", total - updated))
    } else {
        Progress::Done
    }
}

/// Reads a Deployment's status until its rollout completes; a failed rollout, or one that takes longer than the timeout,
/// is an error naming why. `kubectl rollout status` reads it the same way.
pub fn await_rollout(
    client: &Client,
    namespace: &str,
    name: &str,
    timeout: Duration,
    poll: Duration,
) -> Result<(), String> {
    let started = Instant::now();
    loop {
        let deploy = client
            .deployment(namespace, name)?
            .ok_or_else(|| format!("{name} is not in {namespace}"))?;
        match progress(&deploy) {
            Progress::Done => return Ok(()),
            Progress::Failed(why) => return Err(format!("{name}: {why}")),
            Progress::Waiting(detail) => {
                if started.elapsed() >= timeout {
                    return Err(format!(
                        "{name} did not finish its rollout within {timeout:?}: {detail}"
                    ));
                }
                std::thread::sleep(poll);
            }
        }
    }
}

fn wait(client: &Client, namespace: &str, name: &str, settings: &Settings) -> Result<(), String> {
    await_rollout(client, namespace, name, settings.timeout, settings.poll)
}

/// The canary's replicas for a stage at `pct`: its share of the stable Deployment's `total`, rounded up, at least one and
/// never all of them.
fn share(pct: u32, total: u32) -> u32 {
    let n = (u64::from(pct) * u64::from(total)).div_ceil(100);
    u32::try_from(n).unwrap_or(u32::MAX).clamp(1, total - 1)
}

/// The manifests as a stream of YAML documents.
fn parse(text: &str) -> Result<Vec<Value>, String> {
    let mut docs = Vec::new();
    for document in serde_yaml_ng::Deserializer::from_str(text) {
        let doc = Value::deserialize(document).map_err(|e| format!("the manifests: {e}"))?;
        if doc.is_null() {
            continue;
        }
        if !doc.is_object() {
            return Err("the manifests: a document is not a mapping".into());
        }
        docs.push(doc);
    }
    Ok(docs)
}

/// A release's manifests as the documents a deploy applies: the stream parsed and its Kustomize overlay built (see
/// [`build`]). The ephemeral environments (`ephemeral`) apply them to their own namespace.
pub fn manifests_of(bytes: &[u8], tools: &Tools) -> Result<Vec<Value>, String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "the release's manifests are not UTF-8 text".to_string())?;
    build(parse(text)?, tools)
}

/// Whether a kind has no namespace.
pub fn is_cluster_scoped(kind: &str) -> bool {
    CLUSTER_SCOPED.contains(&kind)
}

/// The manifests with their Kustomize overlay built: by `kubectl kustomize` or `kustomize build` when the hub has one, else
/// by [`resolve`]. A stream without an overlay is its own documents.
fn build(docs: Vec<Value>, tools: &Tools) -> Result<Vec<Value>, String> {
    if !docs.iter().any(|d| kind_of(d) == "Kustomization") {
        return Ok(docs);
    }
    if let Some(kubectl) = tools.find("kubectl") {
        return render(&kubectl, &["kustomize"], docs);
    }
    if let Some(kustomize) = tools.find("kustomize") {
        return render(&kustomize, &["build"], docs);
    }
    resolve(docs)
}

/// Builds the overlay with a tool: the stream's other documents are written as the overlay's resources in a private
/// directory, the tool renders it, and its output is the manifests. A failed build fails the stage with the tool's words.
fn render(bin: &Path, args: &[&str], docs: Vec<Value>) -> Result<Vec<Value>, String> {
    let (overlays, others): (Vec<Value>, Vec<Value>) = docs
        .into_iter()
        .partition(|d| kind_of(d) == "Kustomization");
    let [mut overlay] = <[Value; 1]>::try_from(overlays)
        .map_err(|_| "the manifests hold more than one Kustomization".to_string())?;
    let mut stream = String::new();
    for doc in &others {
        stream.push_str("---\n");
        stream.push_str(&serde_yaml_ng::to_string(doc).map_err(|e| e.to_string())?);
    }
    overlay["resources"] = json!(["resources.yaml"]);
    let scratch = Scratch::new("kustomize")?;
    std::fs::write(scratch.0.join("resources.yaml"), stream)
        .and_then(|_| {
            std::fs::write(
                scratch.0.join("kustomization.yaml"),
                serde_yaml_ng::to_string(&overlay).unwrap_or_default(),
            )
        })
        .map_err(|e| format!("the hub could not write the overlay: {e}"))?;
    let output = Command::new(bin)
        .args(args)
        .arg(&scratch.0)
        .output()
        .map_err(|e| format!("kustomize did not start: {e}"))?;
    if !output.status.success() {
        let why = String::from_utf8_lossy(&output.stderr);
        return Err(format!("the overlay did not build: {}", why.trim()));
    }
    parse(&String::from_utf8_lossy(&output.stdout))
}

/// The manifests with the Kustomize overlay in them applied, when no tool builds it: its namespace, and its image names
/// and tags.
fn resolve(docs: Vec<Value>) -> Result<Vec<Value>, String> {
    let (overlays, mut docs): (Vec<Value>, Vec<Value>) = docs
        .into_iter()
        .partition(|d| kind_of(d) == "Kustomization");
    if overlays.len() > 1 {
        return Err("the manifests hold more than one Kustomization".into());
    }
    let Some(overlay) = overlays.first() else {
        return Ok(docs);
    };
    for field in UNBUILT_OVERLAY_FIELDS {
        if !overlay[field].is_null() {
            return Err(format!(
                "the overlay's {field} is built by kustomize, and no kustomize or kubectl is on the hub's PATH"
            ));
        }
    }
    if overlay["resources"]
        .as_array()
        .is_some_and(|r| !r.is_empty())
    {
        return Err("the overlay's resources are files: the release's manifests are the stream's own documents".into());
    }
    let namespace = overlay["namespace"].as_str().map(str::to_string);
    let images = overlay["images"].as_array().cloned().unwrap_or_default();
    for doc in &mut docs {
        if let Some(ns) = &namespace
            && !CLUSTER_SCOPED.contains(&kind_of(doc))
            && doc["metadata"]["namespace"].is_null()
        {
            doc["metadata"]["namespace"] = json!(ns);
        }
        for list in ["containers", "initContainers"] {
            let Some(containers) = doc.pointer_mut(&format!("/spec/template/spec/{list}")) else {
                continue;
            };
            for container in containers.as_array_mut().into_iter().flatten() {
                if let Some(image) = container["image"].as_str().map(str::to_string) {
                    container["image"] = json!(retag(&image, &images));
                }
            }
        }
    }
    Ok(docs)
}

/// An image with the overlay's new name and tag for its repository, when the overlay names one.
fn retag(image: &str, rules: &[Value]) -> String {
    let (base, tag) = match image.rsplit_once(':') {
        Some((base, tag)) if !tag.contains('/') => (base, Some(tag)),
        _ => (image, None),
    };
    let Some(rule) = rules.iter().find(|r| r["name"].as_str() == Some(base)) else {
        return image.to_string();
    };
    let name = rule["newName"].as_str().unwrap_or(base);
    match rule["newTag"].as_str().or(tag) {
        Some(tag) => format!("{name}:{tag}"),
        None => name.to_string(),
    }
}

/// The release's manifests, with the stable Deployment's image set: the target Deployment, and every other document.
fn manifests(
    bytes: &[u8],
    settings: &Settings,
    image: &str,
    tools: &Tools,
) -> Result<(Value, Vec<Value>), String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "the release's manifests are not UTF-8 text".to_string())?;
    let mut target: Option<Value> = None;
    let mut others = Vec::new();
    for mut doc in build(parse(text)?, tools)? {
        if !CLUSTER_SCOPED.contains(&kind_of(&doc)) && doc["metadata"]["namespace"].is_null() {
            doc["metadata"]["namespace"] = json!(settings.namespace);
        }
        let is_target = kind_of(&doc) == "Deployment"
            && doc["metadata"]["name"].as_str() == Some(settings.deployment.as_str());
        if is_target {
            if target.is_some() {
                return Err(format!(
                    "the manifests hold two Deployments named {}",
                    settings.deployment
                ));
            }
            target = Some(doc);
        } else {
            others.push(doc);
        }
    }
    let mut target = target.ok_or_else(|| {
        format!(
            "the manifests have no Deployment named {}",
            settings.deployment
        )
    })?;
    set_image(&mut target, settings.container.as_deref(), image)?;
    Ok((target, others))
}

/// Sets the image of a Deployment's container (the first, when none is named).
pub(crate) fn set_image(
    doc: &mut Value,
    container: Option<&str>,
    image: &str,
) -> Result<(), String> {
    let containers = doc
        .pointer_mut("/spec/template/spec/containers")
        .and_then(Value::as_array_mut)
        .ok_or("the Deployment has no containers")?;
    let chosen = match container {
        Some(name) => containers
            .iter_mut()
            .find(|c| c["name"].as_str() == Some(name)),
        None => containers.first_mut(),
    }
    .ok_or_else(|| {
        format!(
            "the Deployment has no container {}",
            container.unwrap_or("(the first)")
        )
    })?;
    chosen["image"] = json!(image);
    Ok(())
}

/// The canary's copy of the stable Deployment: another name, and its own track label on its selector and pods.
fn canary_of(target: &Value, name: &str) -> Value {
    let mut doc = target.clone();
    doc["metadata"]["name"] = json!(name);
    doc["spec"]["selector"]["matchLabels"][TRACK] = json!(CANARY);
    doc["spec"]["template"]["metadata"]["labels"][TRACK] = json!(CANARY);
    doc
}

fn with_replicas(doc: &Value, replicas: u32) -> Value {
    let mut doc = doc.clone();
    doc["spec"]["replicas"] = json!(replicas);
    doc
}

/// The ports a rollout uses: the cluster, the tools, the release's bytes, the checks, the incidents, and the recipe run.
pub struct Ports<'a> {
    pub client: &'a Client,
    pub tools: &'a Tools,
    pub artifacts: &'a dyn Artifacts,
    pub checks: &'a dyn Checks,
    pub incidents: &'a dyn Incidents,
    pub runs: &'a Runs,
}

/// What a rollout has changed on the cluster (or in helm), so a failed stage can put it back.
#[derive(Default)]
struct Track {
    /// The stable Deployment's revision (or the helm release's) before the deploy: what a rollback returns to.
    prior: Option<u64>,
    /// The stable Deployment's replicas before the deploy.
    total: u32,
    canary: bool,
    scaled: bool,
    applied: bool,
    /// The helm release this rollout installs, when the environment deploys a chart.
    helm: Option<helm::Helm>,
}

/// Rolls `release` out to the Kubernetes Deployment of `settings` (or its Helm chart), in stages when it has a canary. The
/// recipe run's step passes when the rollout and every check pass, and fails otherwise after the cluster is put back.
pub fn roll(
    ports: &Ports,
    project: &str,
    env: &Environment,
    settings: &Settings,
    release: &Release,
    run: &str,
) -> Rolled {
    let mut track = Track::default();
    match rollout(ports, project, env, settings, release, &mut track) {
        Ok(()) => {
            ports
                .runs
                .settle(run, super::STEP, &Report::verdict(true), super::now());
            Rolled {
                status: "passed",
                detail: format!(
                    "{} rolled out to {}/{} at {}; smoke and health passed at each stage",
                    release.version,
                    settings.namespace,
                    settings.deployment,
                    super::stages_text(&settings.watch.stages)
                ),
            }
        }
        Err(stop) => {
            ports
                .runs
                .settle(run, super::STEP, &Report::verdict(false), super::now());
            let changed = track.applied || track.scaled || track.canary;
            let notes = restore(ports.client, settings, &track);
            let why = stop.why().to_string();
            if matches!(stop, Stop::Breach(_)) {
                ports
                    .incidents
                    .open(project, &env.name, &release.version, &why);
            }
            let (status, detail) = match (notes.is_empty(), changed) {
                (true, true) => (
                    "rolled_back",
                    format!("{why}; restored the previous version"),
                ),
                (true, false) => ("rolled_back", format!("{why}; nothing had changed")),
                (false, _) => (
                    "failed",
                    format!("{why}; not restored: {}", notes.join("; ")),
                ),
            };
            Rolled { status, detail }
        }
    }
}

fn rollout(
    ports: &Ports,
    project: &str,
    env: &Environment,
    settings: &Settings,
    release: &Release,
    track: &mut Track,
) -> Result<(), Stop> {
    let client = ports.client;
    let ns = settings.namespace.as_str();
    let name = settings.deployment.as_str();
    let image = settings.image_for(release).map_err(Stop::Check)?;
    if let Some(chart) = settings.chart.as_deref() {
        return helm_rollout(ports, project, env, settings, release, chart, &image, track);
    }
    let bytes = ports
        .artifacts
        .bytes(release, &settings.artifact)
        .map_err(Stop::Check)?;
    let (target, others) = manifests(&bytes, settings, &image, ports.tools).map_err(Stop::Check)?;
    let stable = client.deployment(ns, name).map_err(Stop::Check)?;
    track.prior = stable.as_ref().and_then(revision_of);
    track.total = stable.as_ref().map_or(1, replicas_of);
    for doc in &others {
        client.apply(doc).map_err(Stop::Check)?;
    }
    let Some(canary) = settings.canary.as_deref() else {
        client.apply(&target).map_err(Stop::Check)?;
        track.applied = true;
        wait(client, ns, name, settings).map_err(Stop::Check)?;
        return super::checks(ports.checks, project, env, &settings.watch, release)
            .map_err(|s| s.at(&format!("{name} at 100%")));
    };
    if stable.is_none() {
        return Err(Stop::Check(format!(
            "a canary needs its stable Deployment {name} in {ns}"
        )));
    }
    let total = track.total;
    if total < 2 {
        return Err(Stop::Check(format!(
            "a canary needs at least two replicas on {name}, which has {total}"
        )));
    }
    let canary_doc = canary_of(&target, canary);
    for pct in settings.watch.stages.iter().copied().filter(|p| *p < 100) {
        let share = share(pct, total);
        if track.canary {
            client.scale(ns, canary, share).map_err(Stop::Check)?;
        } else {
            client
                .apply(&with_replicas(&canary_doc, share))
                .map_err(Stop::Check)?;
            track.canary = true;
        }
        client.scale(ns, name, total - share).map_err(Stop::Check)?;
        track.scaled = true;
        wait(client, ns, canary, settings).map_err(Stop::Check)?;
        wait(client, ns, name, settings).map_err(Stop::Check)?;
        super::checks(ports.checks, project, env, &settings.watch, release)
            .map_err(|s| s.at(&format!("canary at {pct}%")))?;
    }
    // The promotion: the release goes into the stable Deployment, and the canary's replicas go.
    client
        .apply(&with_replicas(&target, total))
        .map_err(Stop::Check)?;
    track.applied = true;
    if track.canary {
        client.scale(ns, canary, 0).map_err(Stop::Check)?;
    }
    wait(client, ns, name, settings).map_err(Stop::Check)?;
    super::checks(ports.checks, project, env, &settings.watch, release)
        .map_err(|s| s.at(&format!("{name} at 100%")))
}

/// A Helm chart's rollout: `helm upgrade --install` with the release's image, the status read with `helm status`, and the
/// checks after it. The previous revision is recorded first, so a failed stage can roll back to it.
fn helm_rollout(
    ports: &Ports,
    project: &str,
    env: &Environment,
    settings: &Settings,
    release: &Release,
    chart: &str,
    image: &str,
    track: &mut Track,
) -> Result<(), Stop> {
    let client = ports.client;
    let ns = settings.namespace.as_str();
    let installed =
        helm::locate(ports.tools, chart, &settings.helm_release).map_err(Stop::Check)?;
    let before = helm::status(&installed, client, ns).map_err(Stop::Check)?;
    track.prior = before.map(|s| s.revision);
    track.helm = Some(installed.clone());
    track.applied = true;
    helm::upgrade(&installed, client, ns, image, settings.timeout).map_err(Stop::Check)?;
    let after = helm::status(&installed, client, ns)
        .map_err(Stop::Check)?
        .ok_or_else(|| Stop::Check(format!("helm reports no release {}", installed.release)))?;
    if after.status != "deployed" {
        return Err(Stop::Check(format!(
            "helm reports {} as {}",
            installed.release, after.status
        )));
    }
    super::checks(ports.checks, project, env, &settings.watch, release)
        .map_err(|s| s.at(&format!("{} at 100%", installed.release)))
}

/// Puts back what a rollout changed, newest first. Returns what could not be put back (none when all of it was).
fn restore(client: &Client, settings: &Settings, track: &Track) -> Vec<String> {
    let ns = settings.namespace.as_str();
    let name = settings.deployment.as_str();
    let mut notes = Vec::new();
    if track.canary
        && let Some(canary) = settings.canary.as_deref()
        && let Err(e) = client.scale(ns, canary, 0)
    {
        notes.push(format!("{canary}: {e}"));
    }
    if track.scaled
        && let Err(e) = client.scale(ns, name, track.total)
    {
        notes.push(format!("{name} replicas: {e}"));
    }
    if track.applied {
        let undone = match (&track.helm, track.prior) {
            (Some(installed), Some(revision)) => {
                helm::rollback(installed, client, ns, revision, settings.timeout)
            }
            (Some(_), None) => Err("the release has no earlier revision to return to".to_string()),
            (None, Some(revision)) => client.undo(ns, name, revision),
            (None, None) => Err("it has no earlier revision to return to".to_string()),
        };
        if let Err(e) = undone {
            notes.push(format!("{name}: {e}"));
        }
    }
    notes
}

#[cfg(test)]
mod tests;
