//! The platforms of an ephemeral instance: its target is planned from the environment's templates ([`plan`]), deployed
//! through the same platform code a deploy uses ([`deploy`]), and removed ([`teardown`]). The production ports
//! ([`Production`]) read the connectors and the artifacts on the hub; a test gives fakes ([`super::Ports`]).

use serde_json::{Value, json};

use super::{EPHEMERAL, Ports, Subject, Target};
use crate::base_vm::Live;
use crate::deploy::{self, kubernetes};
use crate::environments::{self, Environment};
use crate::project_memory;
use crate::project_stack;
use crate::releases::builder;
use crate::releases::entity::Release;
use crate::releases::flow::{Ledger, Outcome};
use crate::state::App;
use crate::targets;

/// The default name patterns: `{project}`, `{subject}`, `{instance}`, `{environment}` and, in an address, `{name}`.
const NAME: &str = "apz-{project}-{subject}";
const BRANCH: &str = "apz-{subject}-{instance}";
/// The ConfigMap and the Secret an instance's variables and vault secrets are kept in.
const CONFIG: &str = "apz-ephemeral-env";
const SECRETS: &str = "apz-ephemeral-secrets";
/// The release artifact a Cloudflare or AWS instance is built from, unless the environment names another.
const ARTIFACT: &str = "web";
/// The stack's output that names an instance's address.
const URL_OUTPUT: &str = "Url";
/// The longest a Kubernetes name (a namespace, a Worker script) may be.
const LABEL: usize = 63;
/// The longest a CloudFormation stack's name may be.
const STACK: usize = 128;
/// The longest subject slug an instance name carries.
const SLUG: usize = 40;

/// The parts a name pattern is expanded from.
pub struct Names {
    project: String,
    environment: String,
    subject: String,
    instance: String,
}

impl Names {
    fn new(env: &Environment, project: &str, subject: &Subject, id: &str) -> Names {
        Names {
            project: clean(project, 40),
            environment: clean(&env.name, 40),
            subject: slug(subject),
            instance: id.trim_start_matches("eph-").chars().take(8).collect(),
        }
    }
}

/// A subject's slug in a name: `lane-<change>` or `release-<id>`, lower-case, dashes for the rest.
pub fn slug(subject: &Subject) -> String {
    let (kind, id) = match subject {
        Subject::Lane(id) => ("lane", id),
        Subject::Release(id) => ("release", id),
    };
    format!("{kind}-{}", clean(id, SLUG))
}

/// Lower-case letters and digits, dashes for anything else, cut to `max` and trimmed of dashes.
fn clean(text: &str, max: usize) -> String {
    let lower: String = text
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(max)
        .collect();
    lower.trim_matches('-').to_string()
}

/// A pattern with its placeholders filled in. `name` is the instance's target name (`{name}`).
pub fn expand(pattern: &str, names: &Names, name: &str) -> String {
    pattern
        .replace("{project}", &names.project)
        .replace("{environment}", &names.environment)
        .replace("{subject}", &names.subject)
        .replace("{instance}", &names.instance)
        .replace("{name}", name)
}

/// A name a platform takes: a DNS label (lower-case letters, digits and dashes, at most 63), or a CloudFormation stack's.
fn label_ok(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= LABEL
        && !text.starts_with('-')
        && !text.ends_with('-')
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn stack_ok(text: &str) -> bool {
    text.len() <= STACK
        && text.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The name a pattern gives, checked; a name the platform would refuse says which variable to change.
fn named(
    env: &Environment,
    key: &str,
    default: &str,
    names: &Names,
    ok: fn(&str) -> bool,
    what: &str,
) -> Result<String, String> {
    let pattern = deploy::variable(env, key).unwrap_or_else(|| default.to_string());
    let name = expand(&pattern, names, "");
    if ok(&name) {
        Ok(name)
    } else {
        Err(format!(
            "{key} gives {name:?}, which is not {what}: change the pattern"
        ))
    }
}

/// Refuses an instance name that is the environment's own deploy target (`own`, named by `key`): an instance never takes
/// the name a deploy writes to. An empty `own` names nothing.
fn not_own(name: &str, own: &str, key: &str) -> Result<(), String> {
    if !own.is_empty() && name == own {
        Err(format!(
            "{name} is the environment's own {key}: an instance takes a name of its own (change its pattern)"
        ))
    } else {
        Ok(())
    }
}

/// The artifact an instance's Cloudflare or AWS target is built from.
fn artifact_name(env: &Environment) -> String {
    deploy::variable(env, "DEPLOY_ARTIFACT").unwrap_or_else(|| ARTIFACT.to_string())
}

/// The target an instance of `env` is planned to have: its platform's name from the templates. The environment's deploy
/// target must be exactly one (a Kubernetes Deployment, a Pages project, a Worker, or a stack).
pub fn plan(
    env: &Environment,
    project: &str,
    subject: &Subject,
    id: &str,
) -> Result<Target, String> {
    if !super::is_ephemeral(env) {
        return Err(format!("{} is not ephemeral ({EPHEMERAL}=true)", env.name));
    }
    let settings = deploy::Settings::of(env)?.ok_or_else(|| {
        format!(
            "{} names no platform target: an ephemeral environment deploys to a Kubernetes Deployment, a Cloudflare project or a CloudFormation stack",
            env.name
        )
    })?;
    let names = Names::new(env, project, subject, id);
    let region = settings.aws_region.clone();
    let [target] = <[deploy::Target; 1]>::try_from(settings.targets).map_err(|_| {
        format!(
            "{} names more than one target: an ephemeral environment has one",
            env.name
        )
    })?;
    match target {
        deploy::Target::Kubernetes(kube) => {
            let namespace = named(
                env,
                "EPHEMERAL_NAMESPACE",
                NAME,
                &names,
                label_ok,
                "a namespace name (lower-case letters, digits and dashes, at most 63)",
            )?;
            let own = deploy::variable(env, "KUBERNETES_NAMESPACE")
                .unwrap_or_else(|| "default".to_string());
            not_own(&namespace, &own, "KUBERNETES_NAMESPACE")?;
            Ok(Target::Kubernetes {
                namespace,
                deployment: kube.deployment,
            })
        }
        deploy::Target::Cloudflare(deploy::cloudflare::Target::Pages(pages)) => {
            let branch = named(
                env,
                "EPHEMERAL_BRANCH",
                BRANCH,
                &names,
                label_ok,
                "a branch name (lower-case letters, digits and dashes, at most 63)",
            )?;
            not_own(&branch, "main", "the production branch")?;
            Ok(Target::CloudflarePages {
                project: pages,
                deployment: String::new(),
                branch,
            })
        }
        deploy::Target::Cloudflare(deploy::cloudflare::Target::Workers(_)) => {
            let script = named(
                env,
                "EPHEMERAL_WORKER",
                NAME,
                &names,
                label_ok,
                "a Worker script name (lower-case letters, digits and dashes, at most 63)",
            )?;
            let own = deploy::variable(env, "CLOUDFLARE_WORKER").unwrap_or_default();
            not_own(&script, &own, "CLOUDFLARE_WORKER")?;
            Ok(Target::CloudflareWorker { script })
        }
        deploy::Target::Aws(deploy::aws::Target::Stack { .. }) => {
            let stack = named(
                env,
                "EPHEMERAL_STACK",
                NAME,
                &names,
                stack_ok,
                "a stack name (letters, digits and dashes, starting with a letter)",
            )?;
            let own = deploy::variable(env, "AWS_CLOUDFORMATION_STACK").unwrap_or_default();
            not_own(&stack, &own, "AWS_CLOUDFORMATION_STACK")?;
            Ok(Target::AwsStack { stack, region })
        }
        _ => Err(format!(
            "{} deploys to a Lambda, ECS, a site or a Cloudflare target that ephemeral instances do not support: use Kubernetes, Pages, a Worker or a stack",
            env.name
        )),
    }
}

/// Deploys an instance's target: the artifact, applied or uploaded, and the address it serves at (`Some` when there is
/// one). The target is filled in as it is made (a Pages deployment's id).
pub fn deploy(
    ports: &dyn Ports,
    env: &Environment,
    project: &str,
    subject: &Subject,
    id: &str,
    target: &mut Target,
) -> Result<Option<String>, String> {
    let names = Names::new(env, project, subject, id);
    let settings = deploy::Settings::of(env)?
        .ok_or_else(|| format!("{} names no platform target", env.name))?;
    match target.clone() {
        Target::Kubernetes { namespace, .. } => {
            let kube = settings
                .kubernetes()
                .cloned()
                .ok_or_else(|| format!("{} names no Kubernetes Deployment", env.name))?;
            let host = kube_deploy(ports, env, project, subject, &namespace, &kube)?;
            address(env, &names, &namespace, host)
        }
        Target::CloudflarePages {
            project: pages,
            branch,
            ..
        } => {
            let client = ports.cloudflare()?;
            let bytes = ports.artifact(project, subject, &artifact_name(env))?;
            let (deployment, url) = client.pages_preview(&pages, &branch, &bytes)?;
            *target = Target::CloudflarePages {
                project: pages,
                deployment,
                branch: branch.clone(),
            };
            address(env, &names, &branch, url)
        }
        Target::CloudflareWorker { script } => {
            let client = ports.cloudflare()?;
            let bytes = ports.artifact(project, subject, &artifact_name(env))?;
            client.script_put(&script, &bytes)?;
            address(env, &names, &script, None)
        }
        Target::AwsStack { stack, region } => {
            let client = ports.aws(region.as_deref())?;
            let bytes = ports.artifact(project, subject, &artifact_name(env))?;
            client.stack_apply(&stack, &bytes)?;
            let url = client.stack_output(&stack, URL_OUTPUT)?;
            address(env, &names, &stack, url)
        }
    }
}

/// The address an instance serves at: the environment's `EPHEMERAL_URL` pattern when it sets one, else the platform's.
fn address(
    env: &Environment,
    names: &Names,
    name: &str,
    platform: Option<String>,
) -> Result<Option<String>, String> {
    Ok(match deploy::variable(env, super::URL_VAR) {
        Some(pattern) => Some(expand(&pattern, names, name)),
        None => platform,
    })
}

/// A Kubernetes instance: its namespace is created, the environment's variables and secrets are applied into it, the
/// release's (or the lane's) manifests are applied with their namespace and image set, and every Deployment is waited on
/// until its rollout completes. Returns the Ingress host the manifests declare, as a URL, when they declare one.
fn kube_deploy(
    ports: &dyn Ports,
    env: &Environment,
    project: &str,
    subject: &Subject,
    namespace: &str,
    kube: &kubernetes::Settings,
) -> Result<Option<String>, String> {
    let client = ports.kubernetes()?;
    client.create_namespace(namespace)?;
    let mut docs: Vec<Value> = Vec::new();
    if !env.variables.is_empty() {
        docs.push(json!({
            "apiVersion": "v1", "kind": "ConfigMap",
            "metadata": { "name": CONFIG, "namespace": namespace },
            "data": env.variables,
        }));
    }
    let secrets = ports.secrets(project, &env.name);
    if !secrets.is_empty() {
        let data: serde_json::Map<String, Value> = secrets
            .into_iter()
            .map(|(name, value)| (name, Value::String(value)))
            .collect();
        docs.push(json!({
            "apiVersion": "v1", "kind": "Secret", "type": "Opaque",
            "metadata": { "name": SECRETS, "namespace": namespace },
            "stringData": data,
        }));
    }
    let bytes = ports.artifact(project, subject, &kube.artifact)?;
    let tools = ports.tools();
    let mut manifests = kubernetes::manifests_of(&bytes, &tools)?;
    let image = match ports.release(project, subject)? {
        Some(release) => Some(kube.image_for(&release)?),
        None => None,
    };
    for doc in manifests.iter_mut() {
        let kind = doc["kind"].as_str().unwrap_or("").to_string();
        if !kubernetes::is_cluster_scoped(&kind) {
            doc["metadata"]["namespace"] = json!(namespace);
        }
        let is_target = kind == "Deployment"
            && doc["metadata"]["name"].as_str() == Some(kube.deployment.as_str());
        if let (true, Some(image)) = (is_target, &image) {
            kubernetes::set_image(doc, kube.container.as_deref(), image)?;
        }
    }
    for doc in docs.iter().chain(manifests.iter()) {
        client.apply(doc)?;
    }
    for doc in &manifests {
        if doc["kind"] == "Deployment"
            && let Some(name) = doc["metadata"]["name"].as_str()
        {
            kubernetes::await_rollout(&client, namespace, name, kube.timeout, kube.poll)?;
        }
    }
    Ok(ingress_host(&manifests))
}

/// The address an Ingress in the manifests declares: its first rule's host, over TLS when it has a TLS section.
fn ingress_host(docs: &[Value]) -> Option<String> {
    let ingress = docs.iter().find(|d| d["kind"] == "Ingress")?;
    let host = ingress["spec"]["rules"]
        .as_array()?
        .iter()
        .find_map(|rule| rule["host"].as_str())?;
    let tls = ingress["spec"]["tls"]
        .as_array()
        .is_some_and(|t| !t.is_empty());
    Some(format!("{}://{host}", if tls { "https" } else { "http" }))
}

/// Removes an instance's target. Each platform's removal is idempotent: what is already gone is not an error.
pub fn teardown(ports: &dyn Ports, target: &Target) -> Result<(), String> {
    match target {
        Target::Kubernetes { namespace, .. } => ports.kubernetes()?.delete_namespace(namespace),
        Target::CloudflarePages { deployment, .. } if deployment.is_empty() => Ok(()),
        Target::CloudflarePages {
            project,
            deployment,
            ..
        } => ports.cloudflare()?.pages_remove(project, deployment),
        Target::CloudflareWorker { script } => ports.cloudflare()?.script_delete(script),
        Target::AwsStack { stack, region } => ports.aws(region.as_deref())?.stack_delete(stack),
    }
}

/// The production ports: the hub's stores, its connectors and the artifacts (the release's, or the lane's build).
pub struct Production<'a> {
    pub app: &'a App,
}

impl Ports for Production<'_> {
    fn now(&self) -> u64 {
        apz_plaza::now_ms()
    }

    fn environment(&self, project: &str, env: &str) -> Result<Environment, String> {
        self.app
            .environments
            .of(project)
            .into_iter()
            .find(|e| e.name == env)
            .ok_or_else(|| format!("no environment {env}"))
    }

    fn release(&self, project: &str, subject: &Subject) -> Result<Option<Release>, String> {
        match subject {
            Subject::Lane(_) => Ok(None),
            Subject::Release(id) => self
                .app
                .releases
                .list(project)
                .into_iter()
                .find(|r| &r.id == id)
                .map(Some)
                .ok_or_else(|| format!("no release {id} in {project}")),
        }
    }

    fn artifact(
        &self,
        project: &str,
        subject: &Subject,
        artifact: &str,
    ) -> Result<Vec<u8>, String> {
        let app = self.app;
        match subject {
            Subject::Release(_) => {
                let release = self
                    .release(project, subject)?
                    .ok_or_else(|| "the subject is not a release".to_string())?;
                let found = release
                    .artifacts
                    .iter()
                    .find(|a| a.target == artifact)
                    .or_else(|| (release.artifacts.len() == 1).then(|| &release.artifacts[0]))
                    .ok_or_else(|| format!("{} has no {artifact} artifact", release.version))?;
                let hash = found
                    .digest
                    .parse::<weft::hash::Hash>()
                    .map_err(|e| format!("the {artifact} artifact's digest is not a hash: {e}"))?;
                app.weft.blob(&hash)
            }
            Subject::Lane(change) => {
                let stack = project_stack::read(app, project)?
                    .ok_or_else(|| format!("{} is not on main", project_stack::FILE))?;
                let repo = app
                    .projects
                    .get(project)
                    .and_then(|p| p.repo)
                    .ok_or_else(|| format!("{project} has no repo yet"))?;
                let rev = project_memory::landed_revision(app, change)?;
                let declared = stack
                    .targets
                    .iter()
                    .find(|t| t.name == artifact && t.platform.is_some())
                    .ok_or_else(|| {
                        format!("{} declares no target {artifact}", project_stack::FILE)
                    })?;
                let platform = declared
                    .platform
                    .map(|p| p.name().replace("arm64", "aarch64"))
                    .ok_or_else(|| format!("{artifact} names no platform"))?;
                let placements = targets::build_plan(app, project)?;
                let target = format!("{artifact}@{platform}");
                match builder::build_target(
                    &Live(app),
                    project,
                    &repo,
                    &rev,
                    &stack,
                    &placements,
                    &target,
                )? {
                    Outcome::Built(built) => Ok(built.bytes),
                    Outcome::NotRun(why) => Err(why),
                }
            }
        }
    }

    fn kubernetes(&self) -> Result<kubernetes::Client, String> {
        deploy::kubernetes_client(self.app)
    }

    fn tools(&self) -> kubernetes::Tools {
        kubernetes::Tools::from_env()
    }

    fn cloudflare(&self) -> Result<deploy::cloudflare::Client, String> {
        deploy::cloudflare_client(self.app)
    }

    fn aws(&self, region: Option<&str>) -> Result<deploy::aws::Client, String> {
        deploy::aws_client(self.app, region)
    }

    fn secrets(&self, project: &str, env: &str) -> Vec<(String, String)> {
        environments::list(self.app, project)
            .into_iter()
            .find(|e| e.name == env)
            .map(|e| {
                e.secrets
                    .into_iter()
                    .filter_map(|name| {
                        environments::secret_value(self.app, project, env, &name)
                            .map(|value| (name, value))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}
