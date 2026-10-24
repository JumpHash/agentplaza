//! The AWS calls of a deploy (PLAN 23, 11.3): a Lambda alias shifted between published versions, an ECS service moved to a
//! task definition per release, a static site in S3 published behind CloudFront, and a CloudFormation stack moved through a
//! change set. Each target has the same four operations as the Cloudflare ones (`super::Api`): `live` reads the version
//! that serves now, `upload` makes a release's version from its artifact, `split` serves versions at percentages (Lambda
//! only: the other targets serve one version at a time), and `restore` serves an earlier version again.
//!
//! Every request is signed on the hub with SigV4 (`apz_gateway::signed_headers`, the gateway's signer) using short-lived
//! credentials of an assumed role: STS `AssumeRole` with the connector's base access key, or `AssumeRoleWithWebIdentity`
//! with its web identity token. The base credentials are read on the hub (`connectors::hub_values`) and never leave it; the
//! role's session is cached and renewed a few minutes before it expires.
//!
//! The artifact a target is built from: Lambda takes the release's zip as the function's code; ECS takes a text file naming
//! the image (`repository@sha256:…`); a static site takes a zip of its files, each published under the site's prefix with its
//! content type; a stack takes its template.
//!
//! Request shapes follow AWS's public API references: STS (query), Lambda (REST JSON, `2015-03-31`), ECS (JSON 1.1 with an
//! `X-Amz-Target`), S3 (REST, path-style addresses), CloudFront (REST XML, `2020-05-31`), CloudFormation (query, `2010-05-15`).
//! Where a shape is written from memory of those references rather than checked against them, the comment says "assumed".
//! Nothing here has been run against a real account: the tests drive a local fake that verifies the signatures.

use std::io::{Cursor, Read};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::platforms::http::{Body, Http, Reply};

/// The region a connector without one signs for.
pub const DEFAULT_REGION: &str = "us-east-1";
/// The session's length asked of STS: the least STS allows (assumed: 900 seconds is the minimum `DurationSeconds`).
const SESSION_SECS: u64 = 900;
/// A session is renewed this long before it expires, so no request is signed with one about to lapse.
const RENEW_BEFORE: Duration = Duration::from_secs(300);
/// The environment variable that points every AWS service at one address: a test's fake, or a private endpoint.
pub const BASE_ENV: &str = "APZ_AWS_ENDPOINT";
/// The API versions (assumed: each is the current one for its service).
const STS_API: &str = "2011-06-15";
const LAMBDA_API: &str = "2015-03-31";
const CLOUDFRONT_API: &str = "2020-05-31";
const CFN_API: &str = "2010-05-15";
const ECS_TARGET: &str = "AmazonEC2ContainerServiceV20141113";
/// The session's name, as the role's audit trail shows it.
const SESSION_NAME: &str = "agentplaza-deploy";
/// Where a static site's release files are kept, under the site's prefix, and the pointer naming the release that is live.
const RELEASES_DIR: &str = "apz-releases/";
const POINTER: &str = "current";
const MANIFEST: &str = "manifest.json";
/// The content type of a Lambda code upload and of an ECS or CloudFormation body (assumed: their JSON and query bodies).
const JSON_11: &str = "application/x-amz-json-1.1";
const FORM: &str = "application/x-www-form-urlencoded; charset=utf-8";

/// What a deploy targets on AWS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A function's alias: published versions, shifted by weight (Lambda's weighted aliases).
    Lambda { function: String, alias: String },
    /// An ECS service: a task definition per release in its family, and the service's deployment settings.
    Ecs {
        cluster: String,
        service: String,
        family: String,
        container: String,
        min_healthy: u32,
        max_percent: u32,
    },
    /// A static site: a release's files in the bucket under `prefix`, served by a CloudFront distribution.
    Site {
        bucket: String,
        prefix: String,
        distribution: Option<String>,
    },
    /// A CloudFormation stack: a change set per template.
    Stack { name: String },
}

impl Target {
    /// The target as the logs and the chat name it.
    pub fn label(&self) -> String {
        match self {
            Target::Lambda { function, alias } => format!("lambda:{function}:{alias}"),
            Target::Ecs {
                cluster, service, ..
            } => format!("ecs:{cluster}/{service}"),
            Target::Site { bucket, prefix, .. } => format!("s3:{bucket}/{prefix}"),
            Target::Stack { name } => format!("cloudformation:{name}"),
        }
    }
}

/// The address of each service. AWS's own by default; `APZ_AWS_ENDPOINT` puts them all under one base (a fake answers them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub sts: String,
    pub lambda: String,
    pub ecs: String,
    pub s3: String,
    pub cloudfront: String,
    pub cloudformation: String,
}

impl Endpoints {
    /// AWS's own addresses for `region` (CloudFront is global).
    pub fn aws(region: &str) -> Endpoints {
        Endpoints {
            sts: format!("https://sts.{region}.amazonaws.com"),
            lambda: format!("https://lambda.{region}.amazonaws.com"),
            ecs: format!("https://ecs.{region}.amazonaws.com"),
            s3: format!("https://s3.{region}.amazonaws.com"),
            cloudfront: "https://cloudfront.amazonaws.com".into(),
            cloudformation: format!("https://cloudformation.{region}.amazonaws.com"),
        }
    }

    /// Every service under `base`, each at its own name (`base/sts`, `base/lambda`, …).
    pub fn at(base: &str) -> Endpoints {
        let base = base.trim_end_matches('/');
        Endpoints {
            sts: format!("{base}/sts"),
            lambda: format!("{base}/lambda"),
            ecs: format!("{base}/ecs"),
            s3: format!("{base}/s3"),
            cloudfront: format!("{base}/cloudfront"),
            cloudformation: format!("{base}/cloudformation"),
        }
    }

    /// The environment's override when `APZ_AWS_ENDPOINT` is set, else AWS's addresses for `region`.
    pub fn from_env(region: &str) -> Endpoints {
        match std::env::var(BASE_ENV) {
            Ok(base) if !base.trim().is_empty() => Endpoints::at(base.trim()),
            _ => Endpoints::aws(region),
        }
    }
}

/// The credentials the hub assumes the role with.
pub enum Base {
    /// An access key that may assume the role (`AssumeRole`, signed with it).
    Keys {
        access_key: String,
        secret_key: String,
    },
    /// A web identity token the role trusts (`AssumeRoleWithWebIdentity`, unsigned).
    WebIdentity { token: String },
}

/// How long to wait for a service to settle (an ECS rollout, a stack, a change set, a version's update), and how often
/// to look. The tests use no interval.
#[derive(Debug, Clone, Copy)]
pub struct Wait {
    pub interval: Duration,
    pub attempts: u32,
}

/// The clock every signature and expiry reads. Production uses the system clock; the tests move it.
pub type Clock = Arc<dyn Fn() -> SystemTime + Send + Sync>;

/// One assumed session: its credentials and when they lapse.
#[derive(Clone)]
struct Session {
    access_key: String,
    secret_key: String,
    token: String,
    expires: SystemTime,
}

/// The AWS client of one connector, for one region and role: the calls of each target, signed on the hub.
pub struct Client {
    http: Arc<dyn Http>,
    endpoints: Endpoints,
    region: String,
    role_arn: String,
    base: Base,
    clock: Clock,
    wait: Wait,
    session: Mutex<Option<Session>>,
}

/// One request before it is signed: its method, address, extra headers, and body with its content type.
struct Call {
    method: &'static str,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<(String, Vec<u8>)>,
}

impl Call {
    fn new(method: &'static str, url: String) -> Call {
        Call {
            method,
            url,
            headers: Vec::new(),
            body: None,
        }
    }

    fn header(mut self, name: &str, value: impl Into<String>) -> Call {
        self.headers.push((name.to_string(), value.into()));
        self
    }

    fn json(self, content_type: &str, value: &Value) -> Call {
        let bytes = serde_json::to_vec(value).unwrap_or_default();
        self.bytes(content_type, bytes)
    }

    fn bytes(mut self, content_type: &str, bytes: impl Into<Vec<u8>>) -> Call {
        self.body = Some((content_type.to_string(), bytes.into()));
        self
    }
}

impl Client {
    /// A client that assumes `role_arn` in `region` with `base`. The role must be an IAM role ARN and the base complete.
    pub fn new(
        http: Arc<dyn Http>,
        endpoints: Endpoints,
        region: &str,
        role_arn: &str,
        base: Base,
        clock: Clock,
        wait: Wait,
    ) -> Result<Client, String> {
        let region = region.trim();
        if region.is_empty() {
            return Err("the AWS connector has no region".into());
        }
        let role_arn = role_arn.trim();
        if !(role_arn.starts_with("arn:") && role_arn.contains(":role/")) {
            return Err(
                "the AWS connector's role is an IAM role ARN (arn:aws:iam::ACCOUNT:role/NAME)"
                    .into(),
            );
        }
        let complete = match &base {
            Base::Keys {
                access_key,
                secret_key,
            } => !access_key.trim().is_empty() && !secret_key.trim().is_empty(),
            Base::WebIdentity { token } => !token.trim().is_empty(),
        };
        if !complete {
            return Err(
                "the AWS connector holds neither an access key and secret nor a web identity token"
                    .into(),
            );
        }
        Ok(Client {
            http,
            endpoints,
            region: region.to_string(),
            role_arn: role_arn.to_string(),
            base,
            clock,
            wait,
            session: Mutex::new(None),
        })
    }

    /// The version that serves `target` now: what a rollback returns to. None when there is none yet.
    pub fn live(&self, target: &Target) -> Result<Option<String>, String> {
        match target {
            Target::Lambda { function, alias } => self.lambda_live(function, alias),
            Target::Ecs {
                cluster, service, ..
            } => self.ecs_live(cluster, service),
            Target::Site { bucket, prefix, .. } => self.site_live(bucket, prefix),
            Target::Stack { name } => self.stack_live(name),
        }
    }

    /// Makes the release's version of `target` from its artifact and returns its id.
    pub fn upload(&self, target: &Target, bytes: &[u8]) -> Result<String, String> {
        match target {
            Target::Lambda { function, .. } => self.lambda_publish(function, bytes),
            Target::Ecs {
                family, container, ..
            } => self.ecs_register(family, container, bytes),
            Target::Site { bucket, prefix, .. } => self.site_upload(bucket, prefix, bytes),
            Target::Stack { name } => self.stack_change_set(name, bytes),
        }
    }

    /// Serves `versions` (id and percentage, summing to 100) of `target`. Lambda splits the alias by weight; the other
    /// targets serve a single version, so they take exactly one entry at 100%.
    pub fn split(&self, target: &Target, versions: &[(String, u32)]) -> Result<(), String> {
        match target {
            Target::Lambda { function, alias } => self.lambda_route(function, alias, versions),
            _ => {
                let [(version, 100)] = versions else {
                    return Err(format!("{} serves one version at 100%", target.label()));
                };
                self.serve(target, version)
            }
        }
    }

    /// Serves the earlier version `to` of `target` again, at 100%. For a stack, `to` is the template it ran.
    pub fn restore(&self, target: &Target, to: &str) -> Result<(), String> {
        match target {
            Target::Lambda { function, alias } => {
                self.lambda_route(function, alias, &[(to.to_string(), 100)])
            }
            Target::Stack { name } => {
                let change_set = self.stack_change_set(name, to.as_bytes())?;
                self.serve(target, &change_set)
            }
            _ => self.serve(target, to),
        }
    }

    /// The single-version operation of each target: an ECS service moves to a task definition, a site's pointer moves to
    /// a release, a stack executes a change set.
    fn serve(&self, target: &Target, version: &str) -> Result<(), String> {
        match target {
            Target::Ecs {
                cluster,
                service,
                min_healthy,
                max_percent,
                ..
            } => self.ecs_update(cluster, service, version, *min_healthy, *max_percent),
            Target::Site {
                bucket,
                prefix,
                distribution,
            } => self.site_serve(bucket, prefix, distribution.as_deref(), version),
            Target::Stack { name } => self.stack_execute(name, version),
            Target::Lambda { function, alias } => {
                self.lambda_route(function, alias, &[(version.to_string(), 100)])
            }
        }
    }

    // ---- Credentials ---------------------------------------------------------------------------------------------------

    /// The session in use, renewed when it is within [`RENEW_BEFORE`] of its expiry.
    fn session(&self) -> Result<Session, String> {
        let now = (self.clock)();
        let mut cached = self.session.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(session) = cached.as_ref()
            && now + RENEW_BEFORE < session.expires
        {
            return Ok(session.clone());
        }
        let fresh = self.assume()?;
        *cached = Some(fresh.clone());
        Ok(fresh)
    }

    /// Assumes the role: `AssumeRole` signed with the base key, or `AssumeRoleWithWebIdentity` with the token.
    fn assume(&self) -> Result<Session, String> {
        let duration = SESSION_SECS.to_string();
        let mut pairs: Vec<(&str, &str)> = vec![
            ("Version", STS_API),
            ("RoleArn", self.role_arn.as_str()),
            ("RoleSessionName", SESSION_NAME),
            ("DurationSeconds", duration.as_str()),
        ];
        let (action, signing) = match &self.base {
            Base::Keys {
                access_key,
                secret_key,
            } => (
                "AssumeRole",
                Some((access_key.as_str(), secret_key.as_str())),
            ),
            Base::WebIdentity { token } => {
                pairs.push(("WebIdentityToken", token.as_str()));
                ("AssumeRoleWithWebIdentity", None)
            }
        };
        pairs.insert(0, ("Action", action));
        let call = Call::new("POST", format!("{}/", self.endpoints.sts)).bytes(FORM, query(&pairs));
        let reply = match signing {
            Some((access, secret)) => {
                self.send("sts", &self.region, &call, Some((access, secret, None)))?
            }
            None => self.send("sts", &self.region, &call, None)?,
        };
        let body = ok(self, "sts", reply, "assuming the deploy role")?;
        let text = body.as_str().unwrap_or_default();
        let field = |tag: &str| {
            xml_tag(text, tag)
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("the role's answer has no {tag}"))
        };
        let expires =
            parse_time(&field("Expiration")?).ok_or("the role's answer names no expiry time")?;
        Ok(Session {
            access_key: field("AccessKeyId")?,
            secret_key: field("SecretAccessKey")?,
            token: field("SessionToken")?,
            expires,
        })
    }

    /// Sends `call` signed with the current session.
    fn call(&self, service: &str, region: &str, call: Call) -> Result<Reply, String> {
        let session = self.session()?;
        self.send(
            service,
            region,
            &call,
            Some((
                &session.access_key,
                &session.secret_key,
                Some(&session.token),
            )),
        )
    }

    /// Adds the SigV4 headers (when `creds` are given) and sends the call. Only the three headers the signer names are added.
    fn send(
        &self,
        service: &str,
        region: &str,
        call: &Call,
        creds: Option<(&str, &str, Option<&str>)>,
    ) -> Result<Reply, String> {
        let body: &[u8] = match &call.body {
            Some((_, bytes)) => bytes,
            None => &[],
        };
        let mut headers = call.headers.clone();
        if let Some((access, secret, token)) = creds {
            headers.extend(apz_gateway::signed_headers(
                call.method,
                &call.url,
                &call.headers,
                body,
                access,
                secret,
                token,
                region,
                service,
                (self.clock)(),
            ));
        }
        let refs: Vec<(&str, String)> = headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.clone()))
            .collect();
        let body = call
            .body
            .as_ref()
            .map(|(content_type, bytes)| Body::Bytes(content_type.clone(), bytes.clone()));
        self.http
            .call(call.method, &call.url, &refs, body)
            .map_err(|e| self.redact(&e))
    }

    /// Text with the base secret taken out, for any error that might carry it.
    fn redact(&self, text: &str) -> String {
        let secret = match &self.base {
            Base::Keys { secret_key, .. } => secret_key.as_str(),
            Base::WebIdentity { token } => token.as_str(),
        };
        text.replace(secret, "[redacted]")
    }

    // ---- Lambda --------------------------------------------------------------------------------------------------------

    fn lambda_url(&self, path: &str) -> String {
        format!("{}/{LAMBDA_API}{path}", self.endpoints.lambda)
    }

    /// The version an alias points at (its primary version, under a split).
    fn lambda_live(&self, function: &str, alias: &str) -> Result<Option<String>, String> {
        let url = self.lambda_url(&format!("/functions/{function}/aliases/{alias}"));
        let reply = self.call("lambda", &self.region, Call::new("GET", url))?;
        if reply.status == 404 {
            return Ok(None);
        }
        let body = ok(self, "lambda", reply, "reading the alias")?;
        Ok(body["FunctionVersion"].as_str().map(str::to_string))
    }

    /// Publishes the release's zip as a new version of the function, and waits until its update is done. The code upload
    /// with `Publish: true` answers the version; `LastUpdateStatus` (assumed field) says when it can take an alias.
    fn lambda_publish(&self, function: &str, zip: &[u8]) -> Result<String, String> {
        let code = base64::engine::general_purpose::STANDARD.encode(zip);
        let url = self.lambda_url(&format!("/functions/{function}/code"));
        let body = json!({ "ZipFile": code, "Publish": true });
        let reply = self.call(
            "lambda",
            &self.region,
            Call::new("PUT", url).json("application/json", &body),
        )?;
        let answer = ok(self, "lambda", reply, "publishing the release")?;
        let version = answer["Version"]
            .as_str()
            .ok_or("the publish answered no version")?
            .to_string();
        self.poll("the function's update", || {
            let url = self.lambda_url(&format!(
                "/functions/{function}/configuration?Qualifier={version}"
            ));
            let reply = self.call("lambda", &self.region, Call::new("GET", url))?;
            let config = ok(self, "lambda", reply, "reading the version")?;
            match config["LastUpdateStatus"].as_str() {
                Some("InProgress") => Ok(None),
                Some("Failed") => Err(format!("version {version} did not update")),
                _ => Ok(Some(())),
            }
        })?;
        Ok(version)
    }

    /// Points the alias at the versions: the one with the largest share is the primary, the others take their share of
    /// traffic as additional versions (assumed: `AdditionalVersionWeights` takes fractions from 0 to 1, and an empty map
    /// clears the split). One version at 100% clears it.
    fn lambda_route(
        &self,
        function: &str,
        alias: &str,
        versions: &[(String, u32)],
    ) -> Result<(), String> {
        let total: u32 = versions.iter().map(|(_, pct)| *pct).sum();
        let primary = versions
            .iter()
            .max_by_key(|(_, pct)| *pct)
            .ok_or("an alias needs a version")?;
        if total != 100 {
            return Err("the percentages of an alias sum to 100".into());
        }
        let weights: serde_json::Map<String, Value> = versions
            .iter()
            .filter(|(version, _)| *version != primary.0)
            .map(|(version, pct)| (version.clone(), json!(f64::from(*pct) / 100.0)))
            .collect();
        let body = json!({
            "FunctionVersion": primary.0,
            "RoutingConfig": { "AdditionalVersionWeights": weights },
        });
        let url = self.lambda_url(&format!("/functions/{function}/aliases/{alias}"));
        let reply = self.call(
            "lambda",
            &self.region,
            Call::new("PUT", url).json("application/json", &body),
        )?;
        if reply.status == 404 {
            // The alias does not exist yet (a first deploy): UpdateAlias cannot make it, so it is created (assumed: the
            // CreateAlias request takes the name with the same fields).
            let mut created = body.clone();
            created["Name"] = json!(alias);
            let url = self.lambda_url(&format!("/functions/{function}/aliases"));
            let reply = self.call(
                "lambda",
                &self.region,
                Call::new("POST", url).json("application/json", &created),
            )?;
            return ok(self, "lambda", reply, "creating the alias").map(|_| ());
        }
        ok(self, "lambda", reply, "shifting the alias").map(|_| ())
    }

    // ---- ECS -----------------------------------------------------------------------------------------------------------

    /// One ECS JSON 1.1 call (assumed: the `X-Amz-Target` names the operation, the body is its JSON).
    fn ecs(&self, operation: &str, body: &Value) -> Result<Value, String> {
        let call = Call::new("POST", format!("{}/", self.endpoints.ecs))
            .header("X-Amz-Target", format!("{ECS_TARGET}.{operation}"))
            .json(JSON_11, body);
        let reply = self.call("ecs", &self.region, call)?;
        ok(self, "ecs", reply, operation)
    }

    /// The task definition the service runs now.
    fn ecs_live(&self, cluster: &str, service: &str) -> Result<Option<String>, String> {
        let answer = self.ecs(
            "DescribeServices",
            &json!({ "cluster": cluster, "services": [service] }),
        )?;
        let svc = &answer["services"][0];
        if svc.is_null() || svc["status"] == "INACTIVE" {
            return Ok(None);
        }
        Ok(svc["taskDefinition"].as_str().map(str::to_string))
    }

    /// Registers a task definition in the family, with the release's image in its container (assumed: a task definition
    /// with one essential container and no resource sizes, which an EC2-launched service takes). The artifact is the image.
    fn ecs_register(&self, family: &str, container: &str, bytes: &[u8]) -> Result<String, String> {
        let image = std::str::from_utf8(bytes)
            .map_err(|_| "the ECS artifact is a text file naming one image".to_string())?
            .trim();
        if image.is_empty() || image.contains(char::is_whitespace) {
            return Err("the ECS artifact is a text file naming one image".into());
        }
        let answer = self.ecs(
            "RegisterTaskDefinition",
            &json!({
                "family": family,
                "containerDefinitions": [{ "name": container, "image": image, "essential": true }],
            }),
        )?;
        answer["taskDefinition"]["taskDefinitionArn"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "RegisterTaskDefinition answered no task definition".into())
    }

    /// Moves the service to `task_definition` with its deployment settings, and waits until its rollout completes (assumed:
    /// a steady service has one deployment, with `rolloutState` COMPLETED; `FAILED` is a failed rollout).
    fn ecs_update(
        &self,
        cluster: &str,
        service: &str,
        task_definition: &str,
        min_healthy: u32,
        max_percent: u32,
    ) -> Result<(), String> {
        self.ecs(
            "UpdateService",
            &json!({
                "cluster": cluster,
                "service": service,
                "taskDefinition": task_definition,
                "deploymentConfiguration": {
                    "minimumHealthyPercent": min_healthy,
                    "maximumPercent": max_percent,
                },
            }),
        )?;
        self.poll("the ECS service's rollout", || {
            let answer = self.ecs(
                "DescribeServices",
                &json!({ "cluster": cluster, "services": [service] }),
            )?;
            let deployments = answer["services"][0]["deployments"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if deployments.iter().any(|d| d["rolloutState"] == "FAILED") {
                return Err(format!("the rollout of {service} failed"));
            }
            match deployments.as_slice() {
                [only] if only["rolloutState"] == "COMPLETED" => Ok(Some(())),
                _ => Ok(None),
            }
        })
    }

    // ---- S3 and CloudFront ----------------------------------------------------------------------------------------------

    /// One S3 call on the path-style address of `bucket` and `key`, with the payload hash S3 wants in its header.
    fn s3(
        &self,
        method: &'static str,
        bucket: &str,
        key: &str,
        body: Option<(&str, Vec<u8>)>,
    ) -> Result<Reply, String> {
        let payload: &[u8] = match &body {
            Some((_, bytes)) => bytes,
            None => &[],
        };
        let mut call = Call::new(method, format!("{}/{bucket}/{key}", self.endpoints.s3))
            .header("x-amz-content-sha256", hex(&Sha256::digest(payload)));
        if let Some((content_type, bytes)) = body {
            call = call.bytes(content_type, bytes);
        }
        self.call("s3", &self.region, call)
    }

    fn site_pointer(prefix: &str) -> String {
        format!("{prefix}{RELEASES_DIR}{POINTER}")
    }

    /// The release the site serves now: the pointer's text.
    fn site_live(&self, bucket: &str, prefix: &str) -> Result<Option<String>, String> {
        let reply = self.s3("GET", bucket, &Self::site_pointer(prefix), None)?;
        if reply.status == 404 {
            return Ok(None);
        }
        let text = ok(self, "s3", reply, "reading the live release")?;
        Ok(text
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()))
    }

    /// Uploads the zip's files under the release's own prefix, each with its content type, then its manifest (the list
    /// `site_serve` copies from). The release's id is a hash of the zip.
    fn site_upload(&self, bucket: &str, prefix: &str, zip: &[u8]) -> Result<String, String> {
        let files = zip_files(zip)?;
        let release = format!("rel-{}", &hex(&Sha256::digest(zip))[..16]);
        let root = format!("{prefix}{RELEASES_DIR}{release}/");
        for (path, content) in &files {
            let reply = self.s3(
                "PUT",
                bucket,
                &format!("{root}{path}"),
                Some((content_type(path), content.clone())),
            )?;
            ok(self, "s3", reply, &format!("uploading {path}"))?;
        }
        let manifest: Vec<Value> = files
            .iter()
            .map(|(path, _)| json!({ "path": path, "content_type": content_type(path) }))
            .collect();
        let body = json!({ "files": manifest }).to_string().into_bytes();
        let reply = self.s3(
            "PUT",
            bucket,
            &format!("{root}{MANIFEST}"),
            Some(("application/json", body)),
        )?;
        ok(self, "s3", reply, "uploading the manifest")?;
        Ok(release)
    }

    /// Copies a release's files over the live ones, moves the pointer, and invalidates the distribution's paths.
    fn site_serve(
        &self,
        bucket: &str,
        prefix: &str,
        distribution: Option<&str>,
        release: &str,
    ) -> Result<(), String> {
        let root = format!("{prefix}{RELEASES_DIR}{release}/");
        let reply = self.s3("GET", bucket, &format!("{root}{MANIFEST}"), None)?;
        let manifest = ok(self, "s3", reply, "reading the release's manifest")?;
        let paths: Vec<String> = manifest["files"]
            .as_array()
            .ok_or("the release's manifest lists no files")?
            .iter()
            .filter_map(|f| f["path"].as_str().map(str::to_string))
            .collect();
        for path in &paths {
            let call = Call::new(
                "PUT",
                format!("{}/{bucket}/{prefix}{path}", self.endpoints.s3),
            )
            .header("x-amz-content-sha256", hex(&Sha256::digest(b"")))
            .header("x-amz-copy-source", format!("/{bucket}/{root}{path}"));
            let reply = self.call("s3", &self.region, call)?;
            // S3 answers a failed copy with a 200 whose body is an Error document.
            let copied = ok(self, "s3", reply, &format!("copying {path}"))?;
            if copied.as_str().is_some_and(|text| text.contains("<Error>")) {
                return Err(format!("copying {path} failed"));
            }
        }
        let pointer = format!("{prefix}{RELEASES_DIR}{POINTER}");
        let reply = self.s3(
            "PUT",
            bucket,
            &pointer,
            Some(("text/plain", release.as_bytes().to_vec())),
        )?;
        ok(self, "s3", reply, "moving the live pointer")?;
        if let Some(id) = distribution {
            self.invalidate(id)?;
        }
        Ok(())
    }

    /// Invalidates every path of a distribution (assumed: CloudFront's `InvalidationBatch` document, with a caller
    /// reference that is unique per call).
    fn invalidate(&self, distribution: &str) -> Result<(), String> {
        let now = (self.clock)()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        let xml = format!(
            r#"<InvalidationBatch xmlns="http://cloudfront.amazonaws.com/doc/{CLOUDFRONT_API}/"><Paths><Quantity>1</Quantity><Items><Path>/*</Path></Items></Paths><CallerReference>apz-{distribution}-{now}</CallerReference></InvalidationBatch>"#
        );
        let url = format!(
            "{}/{CLOUDFRONT_API}/distribution/{distribution}/invalidation",
            self.endpoints.cloudfront
        );
        let reply = self.call(
            "cloudfront",
            DEFAULT_REGION,
            Call::new("POST", url).bytes("application/xml", xml),
        )?;
        ok(self, "cloudfront", reply, "invalidating the distribution").map(|_| ())
    }

    // ---- CloudFormation --------------------------------------------------------------------------------------------------

    /// One CloudFormation query call.
    fn cfn(&self, action: &str, fields: &[(&str, &str)]) -> Result<Reply, String> {
        let body = cfn_body(action, fields);
        let url = format!("{}/", self.endpoints.cloudformation);
        self.call(
            "cloudformation",
            &self.region,
            Call::new("POST", url).bytes(FORM, body),
        )
    }

    /// The stack's status, or None when it does not exist.
    fn stack_status(&self, name: &str) -> Result<Option<String>, String> {
        let reply = self.cfn("DescribeStacks", &[("StackName", name)])?;
        if reply.status >= 400 && message_of(&reply).contains("does not exist") {
            return Ok(None);
        }
        let body = ok(self, "cloudformation", reply, "reading the stack")?;
        Ok(xml_tag(body.as_str().unwrap_or_default(), "StackStatus"))
    }

    /// The template the stack runs now (a stack that does not exist has none).
    fn stack_live(&self, name: &str) -> Result<Option<String>, String> {
        if self.stack_status(name)?.is_none() {
            return Ok(None);
        }
        let reply = self.cfn("GetTemplate", &[("StackName", name)])?;
        let body = ok(self, "cloudformation", reply, "reading the template")?;
        Ok(xml_tag(body.as_str().unwrap_or_default(), "TemplateBody"))
    }

    /// Creates a change set for the template (UPDATE for a stack that exists, CREATE otherwise) and waits until it is
    /// created. Its id is the version. A change set that fails to create is an error with its reason.
    fn stack_change_set(&self, name: &str, template: &[u8]) -> Result<String, String> {
        let template = std::str::from_utf8(template)
            .map_err(|_| "the stack's template is text".to_string())?;
        let kind = if self.stack_status(name)?.is_some() {
            "UPDATE"
        } else {
            "CREATE"
        };
        let change_set = format!("apz-{}", &hex(&Sha256::digest(template.as_bytes()))[..12]);
        let reply = self.cfn(
            "CreateChangeSet",
            &[
                ("StackName", name),
                ("ChangeSetName", &change_set),
                ("ChangeSetType", kind),
                ("TemplateBody", template),
            ],
        )?;
        let body = ok(self, "cloudformation", reply, "creating the change set")?;
        let id = xml_tag(body.as_str().unwrap_or_default(), "Id")
            .ok_or("CreateChangeSet answered no change set")?;
        self.poll("the change set", || {
            let reply = self.cfn(
                "DescribeChangeSet",
                &[("StackName", name), ("ChangeSetName", &id)],
            )?;
            let answer = ok(self, "cloudformation", reply, "reading the change set")?;
            let text = answer.as_str().unwrap_or_default();
            match xml_tag(text, "Status").as_deref() {
                Some("CREATE_COMPLETE") => Ok(Some(id.clone())),
                Some("FAILED") => Err(format!(
                    "the change set did not create: {}",
                    xml_tag(text, "StatusReason").unwrap_or_default()
                )),
                _ => Ok(None),
            }
        })
    }

    /// Executes a change set and waits until the stack settles: `EXECUTE_COMPLETE` on the change set is done, and a failed
    /// execution is an error.
    fn stack_execute(&self, name: &str, change_set: &str) -> Result<(), String> {
        let reply = self.cfn(
            "ExecuteChangeSet",
            &[("StackName", name), ("ChangeSetName", change_set)],
        )?;
        ok(self, "cloudformation", reply, "executing the change set")?;
        self.poll("the stack", || {
            let reply = self.cfn(
                "DescribeChangeSet",
                &[("StackName", name), ("ChangeSetName", change_set)],
            )?;
            let answer = ok(self, "cloudformation", reply, "reading the change set")?;
            match xml_tag(answer.as_str().unwrap_or_default(), "ExecutionStatus").as_deref() {
                Some("EXECUTE_COMPLETE") => Ok(Some(())),
                Some("EXECUTE_FAILED" | "OBSOLETE") => {
                    Err(format!("the stack did not update to {change_set}"))
                }
                _ => Ok(None),
            }
        })
    }

    // ---- Waiting -------------------------------------------------------------------------------------------------------

    /// Asks `check` until it is done (`Some`), fails (`Err`), or the attempts run out. Waits between the attempts.
    fn poll<T>(
        &self,
        what: &str,
        mut check: impl FnMut() -> Result<Option<T>, String>,
    ) -> Result<T, String> {
        let attempts = self.wait.attempts.max(1);
        for attempt in 0..attempts {
            if let Some(done) = check()? {
                return Ok(done);
            }
            if attempt + 1 < attempts {
                std::thread::sleep(self.wait.interval);
            }
        }
        Err(format!("{what} did not settle after {attempts} checks"))
    }
}

/// The answer of a 2xx call, or the service's own words for a refusal (never a credential).
fn ok(client: &Client, service: &str, reply: Reply, what: &str) -> Result<Value, String> {
    if (200..300).contains(&reply.status) {
        return Ok(reply.body);
    }
    let message = message_of(&reply);
    Err(client
        .redact(&format!(
            "{what}: {service} answered {} {message}",
            reply.status
        ))
        .trim_end()
        .to_string())
}

/// The message a service gives for a refusal: an XML document's `Message`, or a JSON answer's `message`, `Message` or type.
fn message_of(reply: &Reply) -> String {
    match &reply.body {
        Value::String(xml) => xml_tag(xml, "Message")
            .or_else(|| xml_tag(xml, "Code"))
            .unwrap_or_default(),
        body => body["message"]
            .as_str()
            .or_else(|| body["Message"].as_str())
            .or_else(|| body["__type"].as_str())
            .unwrap_or_default()
            .chars()
            .take(300)
            .collect(),
    }
}

/// A form body (`application/x-www-form-urlencoded`) of `pairs`, in the order given.
fn query(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", pct(k), pct(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// One CloudFormation call's body: its action and API version, then the fields in the order given.
fn cfn_body(action: &str, fields: &[(&str, &str)]) -> String {
    let mut pairs = vec![("Action", action), ("Version", CFN_API)];
    pairs.extend_from_slice(fields);
    query(&pairs)
}

/// RFC 3986 percent-encoding: unreserved characters stay, everything else is `%XX`.
fn pct(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The text inside the first `<tag>…</tag>` of an XML document, with its entities read.
fn xml_tag(doc: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let start = doc.find(&open)? + open.len();
    let end = start + doc[start..].find(&format!("</{tag}>"))?;
    Some(
        doc[start..end]
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&"),
    )
}

/// `YYYY-MM-DDTHH:MM:SS[.fff]Z` (the form STS gives an expiry) as a time.
fn parse_time(text: &str) -> Option<SystemTime> {
    let b = text.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let days = days_from_civil(num(0..4)?, num(5..7)?, num(8..10)?);
    let secs = days * 86_400 + num(11..13)? * 3600 + num(14..16)? * 60 + num(17..19)?;
    u64::try_from(secs)
        .ok()
        .map(|s| UNIX_EPOCH + Duration::from_secs(s))
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The content type a site file is served as, by its extension (assumed: the usual types of a static site).
fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "application/javascript",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// The files of a site's zip, by their paths. A path the signer cannot take unencoded (anything but letters, digits, `-`,
/// `_`, `.`, `~` and `/`) is refused, as is a path that climbs out of the release.
fn zip_files(zip: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(zip))
        .map_err(|e| format!("the site artifact is a zip file: {e}"))?;
    let mut files = Vec::new();
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|e| format!("the site artifact does not read: {e}"))?;
        if file.is_dir() {
            continue;
        }
        let name = file
            .name()
            .map_err(|e| format!("the site artifact has a name that does not read: {e}"))?
            .trim_start_matches("./")
            .to_string();
        let plain = !name.is_empty()
            && !name.starts_with('/')
            && !name.split('/').any(|part| part == "..")
            && name.bytes().all(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/')
            });
        if !plain {
            return Err(format!(
                "the site file {name:?} has a name S3 keys here do not take"
            ));
        }
        let mut content = Vec::new();
        file.read_to_end(&mut content)
            .map_err(|e| format!("{name} does not read: {e}"))?;
        files.push((name, content));
    }
    if files.is_empty() {
        return Err("the site artifact has no files".into());
    }
    Ok(files)
}

/// The production clock: the system's.
pub fn system_clock() -> Clock {
    Arc::new(SystemTime::now)
}

#[cfg(test)]
#[path = "aws_tests.rs"]
mod tests;
