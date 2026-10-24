//! The AWS client against a local fake of STS, Lambda, ECS, S3 and CloudFront. The fake is a real HTTP server on a loopback
//! port, reached through the hub's own HTTP path (`Ureq`), and it verifies every request's SigV4 signature: it recomputes the
//! signature with the credentials it issued (or, for STS, the base key) and refuses a request whose signature differs. Each
//! call it serves is written to a log, in order, which the tests read back. CloudFormation is covered by its request builder
//! only (the fake does not serve it).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::*;
use crate::deploy::{Artifacts, Checks, Incidents, Platforms, Ports, Rolled, Settings};
use crate::environments::Environment;
use crate::platforms::http::{Http, Ureq};
use crate::recipes::runs::Runs;
use crate::releases::entity::{Release, ReleaseState};

const REGION: &str = "eu-west-1";
const ROLE: &str = "arn:aws:iam::123456789012:role/deployer";
const BASE_AK: &str = "AKIABASEONLY0000000";
const BASE_SK: &str = "base-secret-only-in-the-vault";
const WEB_TOKEN: &str = "web-identity-token-from-the-issuer";
/// 2026-10-24T12:00:00Z.
const T0: u64 = 1_792_843_200;
const CLUSTER: &str = "prod";
const SERVICE: &str = "web";
const FAMILY: &str = "web";
const TASK_ARN: &str = "arn:aws:ecs:eu-west-1:123456789012:task-definition/web";
/// How many keys one S3 listing page answers (the fake is a valid S3 that pages early).
const LIST_PAGE: usize = 2;

/// One session the fake issued.
#[derive(Clone)]
struct Issued {
    access: String,
    secret: String,
    token: Option<String>,
}

#[derive(Default)]
struct State {
    issued: Vec<Issued>,
    /// The zips published as Lambda versions: version `n` is `versions[n - 1]`.
    versions: Vec<Vec<u8>>,
    /// Configuration reads that answer `InProgress` after a publish, before `Successful`.
    updating: u32,
    /// `function/alias` → (primary version, additional version weights).
    aliases: BTreeMap<String, (String, Value)>,
    /// ECS task definitions by revision: index + 1 is the revision; each names its image.
    task_images: Vec<String>,
    /// The task definition the service runs, and the polls that still answer an open rollout.
    service_task: usize,
    rollout_polls: u32,
    /// S3 objects: key → (content type, bytes).
    objects: BTreeMap<String, (String, Vec<u8>)>,
    /// Whether a rollout is open (its describes are logged), and the polls that still answer it in progress.
    rolling: bool,
    /// Every call served, in order.
    log: Vec<String>,
}

/// The fake AWS: a local server whose state the tests read and seed.
struct Fake {
    state: Mutex<State>,
    now: Arc<Mutex<SystemTime>>,
}

struct Req {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Req {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

struct Resp {
    status: u16,
    body: Vec<u8>,
}

impl Resp {
    fn json(status: u16, value: Value) -> Resp {
        Resp {
            status,
            body: value.to_string().into_bytes(),
        }
    }

    fn text(status: u16, text: &str) -> Resp {
        Resp {
            status,
            body: text.as_bytes().to_vec(),
        }
    }
}

fn refused() -> Resp {
    Resp::text(
        403,
        "<ErrorResponse><Error><Code>SignatureDoesNotMatch</Code><Message>The request signature we calculated does not match the signature you provided.</Message></Error></ErrorResponse>",
    )
}

impl Fake {
    fn now(&self) -> SystemTime {
        *self.now.lock().unwrap()
    }

    fn log(&self, line: impl Into<String>) {
        self.state.lock().unwrap().log.push(line.into());
    }

    /// The log so far, oldest first.
    fn calls(&self) -> Vec<String> {
        self.state.lock().unwrap().log.clone()
    }

    fn issued(&self) -> Vec<Issued> {
        self.state.lock().unwrap().issued.clone()
    }

    /// The live keys: every object outside the release store, in key order.
    fn live_keys(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .objects
            .keys()
            .filter(|k| !k.contains("apz-releases/"))
            .cloned()
            .collect()
    }

    /// The bytes of an object, if it is there.
    fn object(&self, key: &str) -> Option<Vec<u8>> {
        self.state
            .lock()
            .unwrap()
            .objects
            .get(key)
            .map(|(_, b)| b.clone())
    }

    /// Whether `req` carries a SigV4 signature that one of the candidate credentials produces for it.
    fn verified(&self, req: &Req, service: &str, region: &str, candidates: &[Issued]) -> bool {
        let Some(auth) = req.header("authorization") else {
            return false;
        };
        let Some(rest) = auth.strip_prefix("AWS4-HMAC-SHA256 ") else {
            return false;
        };
        let mut credential = "";
        let mut signed = "";
        for part in rest.split(", ") {
            if let Some(v) = part.strip_prefix("Credential=") {
                credential = v;
            } else if let Some(v) = part.strip_prefix("SignedHeaders=") {
                signed = v;
            }
        }
        let access = credential.split('/').next().unwrap_or("");
        let Some(issued) = candidates.iter().find(|c| c.access == access) else {
            return false;
        };
        let Some(host) = req.header("host") else {
            return false;
        };
        let method: &'static str = match req.method.as_str() {
            "GET" => "GET",
            "PUT" => "PUT",
            "POST" => "POST",
            _ => return false,
        };
        let url = format!("http://{host}{}", req.target);
        let headers: Vec<(String, String)> = signed
            .split(';')
            .filter(|n| !matches!(*n, "host" | "x-amz-date" | "x-amz-security-token"))
            .map(|n| (n.to_string(), req.header(n).unwrap_or_default().to_string()))
            .collect();
        let sign = if service == "s3" {
            apz_gateway::signed_headers_s3
        } else {
            apz_gateway::signed_headers
        };
        let expected = sign(
            method,
            &url,
            &headers,
            &req.body,
            &issued.access,
            &issued.secret,
            issued.token.as_deref(),
            region,
            service,
            self.now(),
        );
        expected
            .iter()
            .any(|(name, value)| name == "authorization" && value == auth)
    }

    fn handle(&self, req: Req) -> Resp {
        let (service, rest) = {
            let trimmed = req.target.trim_start_matches('/');
            match trimmed.split_once('/') {
                Some((service, rest)) => (service.to_string(), format!("/{rest}")),
                None => (trimmed.to_string(), String::new()),
            }
        };
        match service.as_str() {
            "sts" => self.sts(&req),
            "lambda" | "ecs" | "s3" | "cloudfront" => {
                let region = if service == "cloudfront" {
                    DEFAULT_REGION
                } else {
                    REGION
                };
                let candidates = self.issued();
                if !self.verified(&req, &service, region, &candidates) {
                    return refused();
                }
                match service.as_str() {
                    "lambda" => self.lambda(&req, &rest),
                    "ecs" => self.ecs(&req),
                    "s3" => self.s3(&req, &rest),
                    _ => self.cloudfront(&req, &rest),
                }
            }
            _ => Resp::text(404, "no such service"),
        }
    }

    /// `AssumeRole` (signed with the base key) or `AssumeRoleWithWebIdentity` (unsigned, with the trusted token).
    fn sts(&self, req: &Req) -> Resp {
        let form = String::from_utf8_lossy(&req.body).to_string();
        let action = form_field(&form, "Action").unwrap_or_default();
        match action.as_str() {
            "AssumeRole" => {
                let base = Issued {
                    access: BASE_AK.into(),
                    secret: BASE_SK.into(),
                    token: None,
                };
                if !self.verified(req, "sts", REGION, &[base]) {
                    return refused();
                }
            }
            "AssumeRoleWithWebIdentity" => {
                if form_field(&form, "WebIdentityToken").as_deref() != Some(WEB_TOKEN) {
                    return Resp::text(
                        403,
                        "<ErrorResponse><Error><Code>AccessDenied</Code></Error></ErrorResponse>",
                    );
                }
            }
            _ => return Resp::text(400, "unknown action"),
        }
        if form_field(&form, "RoleArn").as_deref() != Some(ROLE) {
            return Resp::text(
                403,
                "<ErrorResponse><Error><Code>AccessDenied</Code></Error></ErrorResponse>",
            );
        }
        let expires = self.now() + Duration::from_secs(900);
        let issued = {
            let mut state = self.state.lock().unwrap();
            let n = state.issued.len() + 1;
            let issued = Issued {
                access: format!("ASIATEST{n:08}"),
                secret: format!("session-secret-{n}"),
                token: Some(format!("session-token-{n}")),
            };
            state.issued.push(issued.clone());
            let unsigned = if action == "AssumeRole" {
                ""
            } else {
                " (unsigned)"
            };
            state.log.push(format!("sts {action}{unsigned}"));
            issued
        };
        let xml = format!(
            "<{action}Response xmlns=\"https://sts.amazonaws.com/doc/2011-06-15/\">\n  <{action}Result>\n    <Credentials>\n      <AccessKeyId>{}</AccessKeyId>\n      <SecretAccessKey>{}</SecretAccessKey>\n      <SessionToken>{}</SessionToken>\n      <Expiration>{}</Expiration>\n    </Credentials>\n  </{action}Result>\n</{action}Response>\n",
            issued.access,
            issued.secret,
            issued.token.unwrap_or_default(),
            iso(expires)
        );
        Resp::text(200, &xml)
    }

    fn lambda(&self, req: &Req, rest: &str) -> Resp {
        let (route, query) = rest.split_once('?').unwrap_or((rest, ""));
        let parts: Vec<&str> = route.trim_matches('/').split('/').collect();
        match (req.method.as_str(), parts.as_slice()) {
            ("GET", ["2015-03-31", "functions", function, "aliases", alias]) => {
                let key = format!("{function}/{alias}");
                let mut state = self.state.lock().unwrap();
                state.log.push(format!("lambda get-alias {key}"));
                match state.aliases.get(&key) {
                    Some((version, routing)) => Resp::json(
                        200,
                        json!({ "Name": alias, "FunctionVersion": version, "RoutingConfig": routing }),
                    ),
                    None => Resp::json(
                        404,
                        json!({ "Type": "User", "message": "Cannot find alias" }),
                    ),
                }
            }
            ("PUT", ["2015-03-31", "functions", function, "code"]) => {
                let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                let zip = body["ZipFile"]
                    .as_str()
                    .and_then(|c| base64::engine::general_purpose::STANDARD.decode(c).ok());
                let Some(zip) = zip else {
                    return Resp::json(400, json!({ "message": "ZipFile is not base64" }));
                };
                let mut state = self.state.lock().unwrap();
                state.versions.push(zip);
                let version = state.versions.len();
                state.updating = 1;
                state
                    .log
                    .push(format!("lambda publish {function} {version}"));
                Resp::json(
                    201,
                    json!({ "FunctionName": function, "Version": version.to_string(), "LastUpdateStatus": "InProgress" }),
                )
            }
            ("GET", ["2015-03-31", "functions", function, "configuration"]) => {
                let version = query.strip_prefix("Qualifier=").unwrap_or("");
                let mut state = self.state.lock().unwrap();
                let status = if state.updating > 0 {
                    state.updating -= 1;
                    "InProgress"
                } else {
                    "Successful"
                };
                state.log.push(format!(
                    "lambda get-configuration {function} {version} {status}"
                ));
                Resp::json(
                    200,
                    json!({ "FunctionName": function, "LastUpdateStatus": status }),
                )
            }
            ("POST", ["2015-03-31", "functions", function, "aliases"]) => {
                let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                let alias = body["Name"].as_str().unwrap_or("").to_string();
                let primary = body["FunctionVersion"].as_str().unwrap_or("").to_string();
                let routing = body["RoutingConfig"].clone();
                let mut state = self.state.lock().unwrap();
                let key = format!("{function}/{alias}");
                if state.aliases.contains_key(&key) {
                    return Resp::json(
                        409,
                        json!({ "Type": "User", "message": "Alias already exists" }),
                    );
                }
                state
                    .log
                    .push(format!("lambda create-alias {key} -> {primary}"));
                state
                    .aliases
                    .insert(key, (primary.clone(), routing.clone()));
                Resp::json(
                    201,
                    json!({ "Name": alias, "FunctionVersion": primary, "RoutingConfig": routing }),
                )
            }
            ("PUT", ["2015-03-31", "functions", function, "aliases", alias]) => {
                let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                let primary = body["FunctionVersion"].as_str().unwrap_or("").to_string();
                let routing = body["RoutingConfig"].clone();
                let mut state = self.state.lock().unwrap();
                if !state.aliases.contains_key(&format!("{function}/{alias}")) {
                    return Resp::json(
                        404,
                        json!({ "Type": "User", "message": "Cannot find alias" }),
                    );
                }
                let exists = primary
                    .parse::<usize>()
                    .is_ok_and(|n| (1..=state.versions.len()).contains(&n));
                if !exists {
                    return Resp::json(
                        404,
                        json!({ "Type": "User", "message": "Version not found" }),
                    );
                }
                let weights = routing["AdditionalVersionWeights"]
                    .as_object()
                    .map(|m| {
                        m.iter()
                            .map(|(k, v)| format!("{k}={}", v.as_f64().unwrap_or(-1.0)))
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .filter(|w| !w.is_empty())
                    .unwrap_or_else(|| "-".into());
                state.log.push(format!(
                    "lambda update-alias {function}/{alias} -> {primary} weights {weights}"
                ));
                state.aliases.insert(
                    format!("{function}/{alias}"),
                    (primary.clone(), routing.clone()),
                );
                Resp::json(
                    200,
                    json!({ "Name": alias, "FunctionVersion": primary, "RoutingConfig": routing }),
                )
            }
            _ => Resp::json(404, json!({ "Type": "User", "message": "no such route" })),
        }
    }

    fn ecs(&self, req: &Req) -> Resp {
        let operation = req
            .header("x-amz-target")
            .and_then(|t| t.rsplit('.').next())
            .unwrap_or("")
            .to_string();
        let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let mut state = self.state.lock().unwrap();
        match operation.as_str() {
            "RegisterTaskDefinition" => {
                let image = body["containerDefinitions"][0]["image"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                state.task_images.push(image.clone());
                let revision = state.task_images.len();
                state
                    .log
                    .push(format!("ecs register {image} -> revision {revision}"));
                Resp::json(
                    200,
                    json!({ "taskDefinition": { "taskDefinitionArn": format!("{TASK_ARN}:{revision}") } }),
                )
            }
            "UpdateService" => {
                let task = body["taskDefinition"].as_str().unwrap_or("");
                let revision = task
                    .rsplit(':')
                    .next()
                    .and_then(|r| r.parse::<usize>().ok());
                let min = body["deploymentConfiguration"]["minimumHealthyPercent"].as_u64();
                let max = body["deploymentConfiguration"]["maximumPercent"].as_u64();
                state.service_task = revision.unwrap_or(0);
                state.rollout_polls = 1;
                state.rolling = true;
                let line = format!(
                    "ecs update-service {SERVICE} -> revision {} min {min:?} max {max:?}",
                    state.service_task
                );
                state.log.push(line);
                Resp::json(200, json!({ "service": { "serviceName": SERVICE } }))
            }
            "DescribeServices" => {
                let current = state.service_task;
                let in_progress = state.rollout_polls > 0;
                state.rollout_polls = state.rollout_polls.saturating_sub(1);
                if state.rolling {
                    let status = if in_progress {
                        "IN_PROGRESS"
                    } else {
                        "COMPLETED"
                    };
                    state.log.push(format!("ecs describe-services {status}"));
                    state.rolling = in_progress;
                }
                let deployments = if in_progress {
                    json!([
                        { "status": "PRIMARY", "taskDefinition": format!("{TASK_ARN}:{current}"), "rolloutState": "IN_PROGRESS" },
                        { "status": "ACTIVE", "taskDefinition": format!("{TASK_ARN}:{}", current.saturating_sub(1).max(1)), "rolloutState": null }
                    ])
                } else {
                    json!([{ "status": "PRIMARY", "taskDefinition": format!("{TASK_ARN}:{current}"), "rolloutState": "COMPLETED" }])
                };
                Resp::json(
                    200,
                    json!({
                        "services": [{
                            "serviceName": SERVICE,
                            "status": "ACTIVE",
                            "taskDefinition": format!("{TASK_ARN}:{current}"),
                            "deployments": deployments,
                        }],
                        "failures": [],
                    }),
                )
            }
            _ => Resp::json(400, json!({ "__type": "UnknownOperationException" })),
        }
    }

    /// S3 with path-style keys: the URL carries each key's segments percent-encoded, and the fake decodes them. It serves
    /// PUT (with the payload hash S3 checks), COPY (an `x-amz-copy-source` PUT), GET, a LIST that answers two keys a page
    /// (so a listing must follow its continuation tokens), and a batch DELETE (with the body's Content-MD5 checked). Each
    /// call is logged with the key it names, decoded.
    fn s3(&self, req: &Req, rest: &str) -> Resp {
        let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
        let key = percent_decode(
            path.trim_start_matches('/')
                .split_once('/')
                .map(|(_, key)| key)
                .unwrap_or(""),
        );
        let params: Vec<(String, String)> = query
            .split('&')
            .filter(|p| !p.is_empty())
            .map(|p| {
                let (k, v) = p.split_once('=').unwrap_or((p, ""));
                (percent_decode(k), percent_decode(v))
            })
            .collect();
        let param = |name: &str| {
            params
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };
        let mut state = self.state.lock().unwrap();
        match req.method.as_str() {
            "GET" if key.is_empty() && param("list-type").is_some() => {
                let prefix = param("prefix").unwrap_or_default();
                let start: usize = param("continuation-token")
                    .and_then(|t| t.parse().ok())
                    .unwrap_or(0);
                let keys: Vec<String> = state
                    .objects
                    .keys()
                    .filter(|k| k.starts_with(&prefix))
                    .cloned()
                    .collect();
                let end = (start + LIST_PAGE).min(keys.len());
                let mut xml = String::from(
                    "<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Name>site-bucket</Name>",
                );
                for key in &keys[start.min(keys.len())..end] {
                    xml.push_str(&format!("<Contents><Key>{}</Key></Contents>", escape(key)));
                }
                let truncated = end < keys.len();
                xml.push_str(&format!("<IsTruncated>{truncated}</IsTruncated>"));
                if truncated {
                    xml.push_str(&format!(
                        "<NextContinuationToken>{end}</NextContinuationToken>"
                    ));
                }
                xml.push_str("</ListBucketResult>");
                state.log.push(format!("s3 list {prefix}"));
                Resp::text(200, &xml)
            }
            "POST" if key.is_empty() && param("delete").is_some() => {
                let digest =
                    base64::engine::general_purpose::STANDARD.encode(md5::Md5::digest(&req.body));
                if req.header("content-md5") != Some(digest.as_str()) {
                    return Resp::text(400, "<Error><Code>BadDigest</Code></Error>");
                }
                let body = String::from_utf8_lossy(&req.body).to_string();
                let keys: Vec<String> = body
                    .split("<Key>")
                    .skip(1)
                    .filter_map(|part| part.split("</Key>").next())
                    .map(unescape)
                    .collect();
                for key in &keys {
                    state.objects.remove(key);
                }
                state.log.push(format!("s3 delete {}", keys.join(",")));
                Resp::text(200, "<DeleteResult></DeleteResult>")
            }
            "PUT" if req.header("x-amz-copy-source").is_some() => {
                let source = req
                    .header("x-amz-copy-source")
                    .unwrap_or_default()
                    .trim_start_matches('/');
                let source_key =
                    percent_decode(source.split_once('/').map(|(_, k)| k).unwrap_or_default());
                let Some(object) = state.objects.get(&source_key).cloned() else {
                    return Resp::text(404, "<Error><Code>NoSuchKey</Code></Error>");
                };
                state.log.push(format!("s3 copy {source_key} -> {key}"));
                state.objects.insert(key, object);
                Resp::text(
                    200,
                    "<CopyObjectResult><ETag>\"copied\"</ETag></CopyObjectResult>",
                )
            }
            "PUT" => {
                let digest = hex(&Sha256::digest(&req.body));
                if req.header("x-amz-content-sha256") != Some(digest.as_str()) {
                    return Resp::text(
                        400,
                        "<Error><Code>XAmzContentSHA256Mismatch</Code></Error>",
                    );
                }
                let content_type = req
                    .header("content-type")
                    .unwrap_or("application/octet-stream")
                    .to_string();
                state.log.push(format!("s3 put {key} {content_type}"));
                state.objects.insert(key, (content_type, req.body.clone()));
                Resp::text(200, "")
            }
            "GET" => match state.objects.get(&key).cloned() {
                Some((_, bytes)) => {
                    state.log.push(format!("s3 get {key}"));
                    Resp {
                        status: 200,
                        body: bytes,
                    }
                }
                None => {
                    state.log.push(format!("s3 get {key} missing"));
                    Resp::text(404, "<Error><Code>NoSuchKey</Code></Error>")
                }
            },
            _ => Resp::text(405, "no such method"),
        }
    }

    fn cloudfront(&self, req: &Req, rest: &str) -> Resp {
        let parts: Vec<&str> = rest.trim_matches('/').split('/').collect();
        match (req.method.as_str(), parts.as_slice()) {
            ("POST", ["2020-05-31", "distribution", id, "invalidation"]) => {
                let xml = String::from_utf8_lossy(&req.body).to_string();
                let paths: Vec<&str> = xml
                    .split("<Path>")
                    .skip(1)
                    .filter_map(|p| p.split("</Path>").next())
                    .collect();
                self.log(format!("cloudfront invalidate {id} {}", paths.join(",")));
                Resp::text(
                    201,
                    "<Invalidation><Id>I1</Id><Status>InProgress</Status></Invalidation>",
                )
            }
            _ => Resp::text(404, "no such route"),
        }
    }
}

/// The fake's answer to a form field, percent-decoded.
fn form_field(form: &str, key: &str) -> Option<String> {
    form.split('&')
        .find_map(|pair| {
            pair.split_once('=')
                .filter(|(k, _)| *k == key)
                .map(|(_, v)| v)
        })
        .map(percent_decode)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = text.get(i + 1..i + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The time as STS writes an expiry: `YYYY-MM-DDTHH:MM:SSZ`.
fn iso(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Serves `fake` on a loopback port, one thread per connection; returns the base address.
fn serve(fake: Arc<Fake>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let addr = listener.local_addr().expect("the port's address");
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let fake = Arc::clone(&fake);
            std::thread::spawn(move || answer(stream, &fake));
        }
    });
    format!("http://{addr}")
}

fn answer(mut stream: TcpStream, fake: &Fake) {
    let Some(req) = read_request(&stream) else {
        return;
    };
    let resp = fake.handle(req);
    let head = format!(
        "HTTP/1.1 {} Fake\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        resp.status,
        resp.body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&resp.body);
    let _ = stream.flush();
}

fn read_request(stream: &TcpStream) -> Option<Req> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let mut headers = Vec::new();
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).ok()?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    let length = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).ok()?;
    Some(Req {
        method,
        target,
        headers,
        body,
    })
}

/// A fake with one published version, `site/live` on it, and an ECS service on revision 1.
struct Harness {
    fake: Arc<Fake>,
    base: String,
    now: Arc<Mutex<SystemTime>>,
    clock: Clock,
}

fn harness() -> Harness {
    let now = Arc::new(Mutex::new(UNIX_EPOCH + Duration::from_secs(T0)));
    let fake = Arc::new(Fake {
        state: Mutex::new(State {
            versions: vec![zipped(&[("index.html", "<h1>one</h1>")])],
            aliases: BTreeMap::from([("site/live".to_string(), ("1".to_string(), json!({})))]),
            task_images: vec!["registry.example/web@sha256:one".to_string()],
            service_task: 1,
            ..State::default()
        }),
        now: Arc::clone(&now),
    });
    let base = serve(Arc::clone(&fake));
    let clock_now = Arc::clone(&now);
    let clock: Clock = Arc::new(move || *clock_now.lock().unwrap());
    Harness {
        fake,
        base,
        now,
        clock,
    }
}

fn advance(h: &Harness, secs: u64) {
    *h.now.lock().unwrap() += Duration::from_secs(secs);
}

fn client(h: &Harness) -> Client {
    Client::new(
        Arc::new(Ureq),
        Endpoints::at(&h.base),
        REGION,
        ROLE,
        Base::Keys {
            access_key: BASE_AK.into(),
            secret_key: BASE_SK.into(),
        },
        Arc::clone(&h.clock),
        Wait {
            interval: Duration::ZERO,
            attempts: 3,
        },
    )
    .expect("a client")
}

/// A zip of the given files, as a release's artifact is.
fn zipped(files: &[(&str, &str)]) -> Vec<u8> {
    use std::io::Write as _;
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, text) in files {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(text.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn lambda(function: &str, alias: &str) -> Target {
    Target::Lambda {
        function: function.into(),
        alias: alias.into(),
    }
}

fn versions(pairs: &[(&str, u32)]) -> Vec<(String, u32)> {
    pairs.iter().map(|(v, p)| (v.to_string(), *p)).collect()
}

#[test]
fn a_lambda_alias_shifts_and_shifts_back_under_a_signed_role_session() {
    let h = harness();
    let client = client(&h);
    let target = lambda("site", "live");

    assert_eq!(client.live(&target).unwrap().as_deref(), Some("1"));
    let zip_v2 = zipped(&[("index.html", "<h1>two</h1>")]);
    assert_eq!(client.upload(&target, &zip_v2).unwrap(), "2");
    client
        .split(&target, &versions(&[("2", 10), ("1", 90)]))
        .unwrap();
    client
        .split(&target, &versions(&[("2", 50), ("1", 50)]))
        .unwrap();
    client.split(&target, &versions(&[("2", 100)])).unwrap();
    client.restore(&target, "1").unwrap();

    assert_eq!(
        h.fake.calls(),
        [
            "sts AssumeRole",
            "lambda get-alias site/live",
            "lambda publish site 2",
            "lambda get-configuration site 2 InProgress",
            "lambda get-configuration site 2 Successful",
            "lambda update-alias site/live -> 1 weights 2=0.1",
            "lambda update-alias site/live -> 1 weights 2=0.5",
            "lambda update-alias site/live -> 2 weights -",
            "lambda update-alias site/live -> 1 weights -",
        ]
    );
    // The published zip is the release's, byte for byte.
    assert_eq!(h.fake.state.lock().unwrap().versions[1], zip_v2);
    let state = h.fake.state.lock().unwrap();
    assert_eq!(state.aliases["site/live"].0, "1");
    assert_eq!(
        state.aliases["site/live"].1["AdditionalVersionWeights"],
        json!({})
    );
    drop(state);

    // The fake refuses what the hub would not sign for it: an unsigned call, and one signed with a wrong secret.
    let url = format!("{}/lambda/2015-03-31/functions/site/aliases/live", h.base);
    assert_eq!(Ureq.call("GET", &url, &[], None).unwrap().status, 403);
    let issued = h.fake.issued()[0].clone();
    let wrong = apz_gateway::signed_headers(
        "GET",
        &url,
        &[],
        &[],
        &issued.access,
        "not-the-secret",
        issued.token.as_deref(),
        REGION,
        "lambda",
        h.fake.now(),
    );
    let headers: Vec<(&str, String)> = wrong.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    assert_eq!(Ureq.call("GET", &url, &headers, None).unwrap().status, 403);
}

#[test]
fn a_first_deploy_creates_its_alias_and_a_later_one_shifts_it() {
    let h = harness();
    let client = client(&h);
    let target = lambda("site", "beta");

    assert_eq!(client.live(&target).unwrap(), None, "no beta alias yet");
    let made = client
        .upload(&target, &zipped(&[("index.html", "<h1>one</h1>")]))
        .unwrap();
    assert_eq!(made, "2");
    client
        .split(&target, &versions(&[(made.as_str(), 100)]))
        .unwrap();
    assert_eq!(client.live(&target).unwrap().as_deref(), Some("2"));

    let calls = h.fake.calls();
    assert_eq!(
        calls[calls.len() - 3..],
        [
            "lambda get-configuration site 2 Successful".to_string(),
            "lambda create-alias site/beta -> 2".to_string(),
            "lambda get-alias site/beta".to_string(),
        ][..],
    );
}

#[test]
fn the_session_is_cached_and_renewed_shortly_before_it_lapses() {
    let h = harness();
    let client = client(&h);
    let target = lambda("site", "live");

    client.live(&target).unwrap();
    advance(&h, 500);
    client.live(&target).unwrap();
    let assumed = |h: &Harness| {
        h.fake
            .calls()
            .iter()
            .filter(|c| c.starts_with("sts AssumeRole"))
            .count()
    };
    assert_eq!(
        assumed(&h),
        1,
        "a session 400 seconds from its expiry is reused"
    );

    advance(&h, 100);
    client.live(&target).unwrap();
    assert_eq!(
        assumed(&h),
        2,
        "a session within five minutes of its expiry is renewed"
    );
    let issued = h.fake.state.lock().unwrap().issued.clone();
    assert_ne!(issued[0].access, issued[1].access);
}

#[test]
fn a_web_identity_token_assumes_the_role_without_a_base_key() {
    let h = harness();
    let web = Client::new(
        Arc::new(Ureq),
        Endpoints::at(&h.base),
        REGION,
        ROLE,
        Base::WebIdentity {
            token: WEB_TOKEN.into(),
        },
        Arc::clone(&h.clock),
        Wait {
            interval: Duration::ZERO,
            attempts: 3,
        },
    )
    .unwrap();
    assert_eq!(
        web.live(&lambda("site", "live")).unwrap().as_deref(),
        Some("1")
    );
    assert_eq!(
        h.fake.calls()[0],
        "sts AssumeRoleWithWebIdentity (unsigned)"
    );

    let wrong_token = Client::new(
        Arc::new(Ureq),
        Endpoints::at(&h.base),
        REGION,
        ROLE,
        Base::WebIdentity {
            token: "a-token-the-role-does-not-trust".into(),
        },
        Arc::clone(&h.clock),
        Wait {
            interval: Duration::ZERO,
            attempts: 3,
        },
    )
    .unwrap();
    let err = wrong_token.live(&lambda("site", "live")).unwrap_err();
    assert!(err.contains("assuming the deploy role"), "{err}");
}

#[test]
fn a_base_key_the_fake_does_not_know_is_refused_and_never_echoed() {
    let h = harness();
    let wrong = Client::new(
        Arc::new(Ureq),
        Endpoints::at(&h.base),
        REGION,
        ROLE,
        Base::Keys {
            access_key: BASE_AK.into(),
            secret_key: "a-wrong-secret-value".into(),
        },
        Arc::clone(&h.clock),
        Wait {
            interval: Duration::ZERO,
            attempts: 3,
        },
    )
    .unwrap();
    let err = wrong.live(&lambda("site", "live")).unwrap_err();
    assert!(err.contains("403"), "{err}");
    assert!(!err.contains("a-wrong-secret-value"));
    assert!(h.fake.calls().is_empty());
}

#[test]
fn a_bad_role_or_an_empty_base_is_refused_before_any_call() {
    let h = harness();
    let endpoints = Endpoints::at(&h.base);
    let keys = || Base::Keys {
        access_key: BASE_AK.into(),
        secret_key: BASE_SK.into(),
    };
    let wait = Wait {
        interval: Duration::ZERO,
        attempts: 1,
    };
    assert!(
        Client::new(
            Arc::new(Ureq),
            endpoints.clone(),
            REGION,
            "deployer",
            keys(),
            Arc::clone(&h.clock),
            wait
        )
        .is_err()
    );
    assert!(
        Client::new(
            Arc::new(Ureq),
            endpoints.clone(),
            REGION,
            ROLE,
            Base::WebIdentity { token: " ".into() },
            Arc::clone(&h.clock),
            wait
        )
        .is_err()
    );
    assert!(
        Client::new(
            Arc::new(Ureq),
            endpoints,
            "",
            ROLE,
            keys(),
            Arc::clone(&h.clock),
            wait
        )
        .is_err()
    );
}

#[test]
fn an_ecs_service_registers_its_release_updates_waits_and_rolls_back_in_order() {
    let h = harness();
    let client = client(&h);
    let target = Target::Ecs {
        cluster: CLUSTER.into(),
        service: SERVICE.into(),
        family: FAMILY.into(),
        container: "web".into(),
        min_healthy: 100,
        max_percent: 200,
    };
    let rev1 = format!("{TASK_ARN}:1");
    let rev2 = format!("{TASK_ARN}:2");

    assert_eq!(
        client.live(&target).unwrap().as_deref(),
        Some(rev1.as_str())
    );
    let made = client
        .upload(&target, b"registry.example/web@sha256:two\n")
        .unwrap();
    assert_eq!(made, rev2);
    client
        .split(&target, &versions(&[(rev2.as_str(), 100)]))
        .unwrap();
    client.restore(&target, &rev1).unwrap();

    assert_eq!(
        h.fake.calls(),
        [
            "sts AssumeRole",
            "ecs register registry.example/web@sha256:two -> revision 2",
            "ecs update-service web -> revision 2 min Some(100) max Some(200)",
            "ecs describe-services IN_PROGRESS",
            "ecs describe-services COMPLETED",
            "ecs update-service web -> revision 1 min Some(100) max Some(200)",
            "ecs describe-services IN_PROGRESS",
            "ecs describe-services COMPLETED",
        ]
    );
    assert_eq!(
        client.live(&target).unwrap().as_deref(),
        Some(rev1.as_str())
    );
}

#[test]
fn a_static_site_uploads_its_files_with_content_types_then_invalidates_and_rolls_back() {
    let h = harness();
    let client = client(&h);
    let target = Target::Site {
        bucket: "site-bucket".into(),
        prefix: String::new(),
        distribution: Some("EDIST1".into()),
    };
    let one = zipped(&[
        ("index.html", "<h1>one</h1>"),
        ("assets/app.css", "body{}"),
        ("assets/app.js", "console.log(1)"),
    ]);
    let two = zipped(&[("index.html", "<h1>two</h1>")]);

    assert_eq!(client.live(&target).unwrap(), None, "no pointer yet");
    let rel1 = client.upload(&target, &one).unwrap();
    client
        .split(&target, &versions(&[(rel1.as_str(), 100)]))
        .unwrap();
    let rel2 = client.upload(&target, &two).unwrap();
    client
        .split(&target, &versions(&[(rel2.as_str(), 100)]))
        .unwrap();
    assert_eq!(
        client.live(&target).unwrap().as_deref(),
        Some(rel2.as_str())
    );
    // The files release two does not have are gone from the live site; the release store keeps both releases.
    assert_eq!(h.fake.live_keys(), ["index.html"]);
    client.restore(&target, &rel1).unwrap();
    assert_eq!(
        client.live(&target).unwrap().as_deref(),
        Some(rel1.as_str())
    );
    assert_eq!(
        h.fake.live_keys(),
        ["assets/app.css", "assets/app.js", "index.html"]
    );
    assert_eq!(h.fake.object("index.html").unwrap(), b"<h1>one</h1>");

    let r1 = format!("apz-releases/{rel1}/");
    let r2 = format!("apz-releases/{rel2}/");
    assert_eq!(
        calls_without_listings(&h),
        [
            "sts AssumeRole".to_string(),
            "s3 get apz-releases/current missing".to_string(),
            format!("s3 put {r1}index.html text/html; charset=utf-8"),
            format!("s3 put {r1}assets/app.css text/css; charset=utf-8"),
            format!("s3 put {r1}assets/app.js application/javascript"),
            format!("s3 put {r1}manifest.json application/json"),
            format!("s3 get {r1}manifest.json"),
            format!("s3 copy {r1}index.html -> index.html"),
            format!("s3 copy {r1}assets/app.css -> assets/app.css"),
            format!("s3 copy {r1}assets/app.js -> assets/app.js"),
            "s3 put apz-releases/current text/plain".to_string(),
            "cloudfront invalidate EDIST1 /*".to_string(),
            format!("s3 put {r2}index.html text/html; charset=utf-8"),
            format!("s3 put {r2}manifest.json application/json"),
            format!("s3 get {r2}manifest.json"),
            format!("s3 copy {r2}index.html -> index.html"),
            "s3 delete assets/app.css,assets/app.js".to_string(),
            "s3 put apz-releases/current text/plain".to_string(),
            "cloudfront invalidate EDIST1 /*".to_string(),
            "s3 get apz-releases/current".to_string(),
            format!("s3 get {r1}manifest.json"),
            format!("s3 copy {r1}index.html -> index.html"),
            format!("s3 copy {r1}assets/app.css -> assets/app.css"),
            format!("s3 copy {r1}assets/app.js -> assets/app.js"),
            "s3 put apz-releases/current text/plain".to_string(),
            "cloudfront invalidate EDIST1 /*".to_string(),
            "s3 get apz-releases/current".to_string(),
        ]
    );
}

#[test]
fn site_keys_with_spaces_unicode_and_reserved_characters_are_signed_served_and_removed() {
    let h = harness();
    let client = client(&h);
    let target = Target::Site {
        bucket: "site-bucket".into(),
        prefix: "www/".into(),
        distribution: None,
    };
    let names = [
        "docs/hello world.html",
        "é/ünï.css",
        "q?x=1&y#frag.txt",
        "100%.txt",
        "a+b(c)!.js",
    ];
    let first: Vec<(&str, &str)> = names.iter().map(|n| (*n, *n)).collect();
    let rel1 = client.upload(&target, &zipped(&first)).unwrap();
    client
        .split(&target, &versions(&[(rel1.as_str(), 100)]))
        .unwrap();
    assert_eq!(
        h.fake.live_keys(),
        [
            "www/100%.txt",
            "www/a+b(c)!.js",
            "www/docs/hello world.html",
            "www/q?x=1&y#frag.txt",
            "www/é/ünï.css",
        ]
    );
    assert_eq!(
        h.fake.object("www/é/ünï.css").unwrap(),
        "é/ünï.css".as_bytes()
    );

    // A second release that keeps one of them removes the rest, reserved characters and all.
    let rel2 = client
        .upload(&target, &zipped(&[("é/ünï.css", "v2")]))
        .unwrap();
    client
        .split(&target, &versions(&[(rel2.as_str(), 100)]))
        .unwrap();
    assert_eq!(h.fake.live_keys(), ["www/é/ünï.css"]);
    assert_eq!(h.fake.object("www/é/ünï.css").unwrap(), b"v2");
    let deletes: Vec<String> = h
        .fake
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("s3 delete"))
        .collect();
    assert_eq!(deletes.len(), 1, "one batch removes what release two lacks");
    assert!(
        deletes[0].contains("q?x=1&y#frag.txt") && deletes[0].contains("100%.txt"),
        "{}",
        deletes[0]
    );
    // The listings answered two keys a page, so the live prefix was read across pages.
    assert!(
        h.fake
            .calls()
            .iter()
            .filter(|c| c.starts_with("s3 list"))
            .count()
            >= 4
    );

    // The signature on such a key: the fake recomputes it with the session's secret, and refuses one made with another.
    let issued = h.fake.issued()[0].clone();
    let url = format!("{}/s3/site-bucket/{}", h.base, key_path("www/é/ünï.css"));
    let wrong = apz_gateway::signed_headers_s3(
        "GET",
        &url,
        &[],
        &[],
        &issued.access,
        "not-the-secret",
        issued.token.as_deref(),
        REGION,
        "s3",
        h.fake.now(),
    );
    let headers: Vec<(&str, String)> = wrong.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    assert_eq!(Ureq.call("GET", &url, &headers, None).unwrap().status, 403);
    let right = apz_gateway::signed_headers_s3(
        "GET",
        &url,
        &[],
        &[],
        &issued.access,
        &issued.secret,
        issued.token.as_deref(),
        REGION,
        "s3",
        h.fake.now(),
    );
    let headers: Vec<(&str, String)> = right.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    assert_eq!(Ureq.call("GET", &url, &headers, None).unwrap().status, 200);
}

/// The fake's log without the listings (their number follows the page size, not the deploy).
fn calls_without_listings(h: &Harness) -> Vec<String> {
    h.fake
        .calls()
        .into_iter()
        .filter(|c| !c.starts_with("s3 list"))
        .collect()
}

#[test]
fn a_lambda_deploy_through_the_recipe_rolls_back_on_a_breach_at_its_second_stage() {
    let h = harness();
    let platforms = Platforms {
        cloudflare: None,
        aws: Some(client(&h)),
        kubernetes: None,
        tools: crate::deploy::kubernetes::Tools::on(""),
    };
    let env = env_with(&[
        ("AWS_LAMBDA_FUNCTION", "site"),
        ("AWS_LAMBDA_ALIAS", "live"),
        ("DEPLOY_ARTIFACT", "web"),
        ("ROLLOUT_STAGES", "10,50,100"),
        ("DEPLOY_WINDOW_SECS", "60"),
    ]);
    let settings = Settings::of(&env).unwrap().expect("an AWS environment");
    assert_eq!(
        settings.targets,
        vec![crate::deploy::Target::Aws(lambda("site", "live"))]
    );
    let release = published("2.0.0");
    let checks = Watch {
        calls: Mutex::new(0),
        breach_at: 2,
    };
    let incidents = Opened::default();
    let runs = Runs::default();
    let items = BTreeMap::from([("rollout".to_string(), settings.targets.len() as u32)]);
    let run = runs
        .start_engine_items(
            "deploy",
            &items,
            "deploy",
            None,
            Some("site"),
            Some("production@2.0.0"),
            1,
        )
        .expect("the deploy recipe is built in");
    let bundle = Bundle(zipped(&[("index.html", "<h1>two</h1>")]));
    let ports = Ports {
        api: &platforms,
        artifacts: &bundle,
        checks: &checks,
        incidents: &incidents,
        runs: &runs,
    };
    let rolled: Rolled = crate::deploy::roll(&ports, "site", &env, &settings, &release, &run);

    assert_eq!(rolled.status, "rolled_back", "{}", rolled.detail);
    assert!(rolled.detail.contains("error rate"), "{}", rolled.detail);
    assert_eq!(
        incidents.0.lock().unwrap().len(),
        1,
        "the breach opens an incident"
    );
    let calls = h.fake.calls();
    let shifts: Vec<String> = calls
        .iter()
        .filter(|c| c.starts_with("lambda update-alias"))
        .cloned()
        .collect();
    assert_eq!(
        shifts,
        [
            "lambda update-alias site/live -> 1 weights 2=0.1",
            "lambda update-alias site/live -> 1 weights 2=0.5",
            "lambda update-alias site/live -> 1 weights -",
        ]
    );
}

#[test]
fn an_aws_environment_names_its_targets_and_refuses_an_incomplete_one() {
    let lambda_env = env_with(&[("AWS_LAMBDA_FUNCTION", "site")]);
    let settings = Settings::of(&lambda_env).unwrap().unwrap();
    assert_eq!(
        settings.targets,
        vec![crate::deploy::Target::Aws(lambda("site", "live"))],
        "the alias defaults to live"
    );

    let full = env_with(&[
        ("AWS_ECS_CLUSTER", "prod"),
        ("AWS_ECS_SERVICE", "web"),
        ("AWS_ECS_CONTAINER", "app"),
        ("ECS_MAX_PERCENT", "150"),
        ("AWS_S3_BUCKET", "site-bucket"),
        ("AWS_S3_PREFIX", "/www/"),
        ("AWS_CLOUDFRONT_DISTRIBUTION", "EDIST1"),
        ("AWS_REGION", "eu-central-1"),
    ]);
    let settings = Settings::of(&full).unwrap().unwrap();
    assert_eq!(settings.aws_region.as_deref(), Some("eu-central-1"));
    assert_eq!(
        settings.targets,
        vec![
            crate::deploy::Target::Aws(Target::Ecs {
                cluster: "prod".into(),
                service: "web".into(),
                family: "web".into(),
                container: "app".into(),
                min_healthy: 100,
                max_percent: 150,
            }),
            crate::deploy::Target::Aws(Target::Site {
                bucket: "site-bucket".into(),
                prefix: "www/".into(),
                distribution: Some("EDIST1".into()),
            }),
        ]
    );

    let partial = env_with(&[("AWS_ECS_CLUSTER", "prod"), ("AWS_ECS_SERVICE", "web")]);
    assert!(
        Settings::of(&partial).is_err(),
        "a service needs its container"
    );
    let stack = env_with(&[("AWS_CLOUDFORMATION_STACK", "edge")]);
    assert_eq!(
        Settings::of(&stack).unwrap().unwrap().targets,
        vec![crate::deploy::Target::Aws(Target::Stack {
            name: "edge".into()
        })]
    );
}

#[test]
fn a_stack_change_set_is_built_as_the_query_api_asks() {
    // CreateChangeSet: the action and version first, then the fields in the order given, each percent-encoded.
    let body = cfn_body(
        "CreateChangeSet",
        &[
            ("StackName", "edge"),
            ("ChangeSetName", "apz-0123456789ab"),
            ("ChangeSetType", "UPDATE"),
            ("TemplateBody", "{\"A\": 1}"),
        ],
    );
    assert_eq!(
        body,
        "Action=CreateChangeSet&Version=2010-05-15&StackName=edge&ChangeSetName=apz-0123456789ab&ChangeSetType=UPDATE&TemplateBody=%7B%22A%22%3A%201%7D"
    );
    assert_eq!(
        cfn_body(
            "ExecuteChangeSet",
            &[
                ("StackName", "edge"),
                (
                    "ChangeSetName",
                    "arn:aws:cloudformation:eu-west-1:1:changeSet/x/y"
                )
            ]
        ),
        "Action=ExecuteChangeSet&Version=2010-05-15&StackName=edge&ChangeSetName=arn%3Aaws%3Acloudformation%3Aeu-west-1%3A1%3AchangeSet%2Fx%2Fy"
    );
    assert_eq!(
        cfn_body("DescribeStacks", &[("StackName", "edge")]),
        "Action=DescribeStacks&Version=2010-05-15&StackName=edge"
    );
    assert_eq!(
        cfn_body("GetTemplate", &[("StackName", "edge")]),
        "Action=GetTemplate&Version=2010-05-15&StackName=edge"
    );
    // The change set's id is the answer's Id, and a stack's template is read from its answer's TemplateBody with the
    // entities read.
    let answer = "<CreateChangeSetResponse>\n  <CreateChangeSetResult>\n    <Id>arn:aws:cloudformation:eu-west-1:1:changeSet/apz-1/2</Id>\n  </CreateChangeSetResult>\n</CreateChangeSetResponse>";
    assert_eq!(
        xml_tag(answer, "Id").as_deref(),
        Some("arn:aws:cloudformation:eu-west-1:1:changeSet/apz-1/2")
    );
    let template = "<GetTemplateResponse><GetTemplateResult><TemplateBody>{&quot;Resources&quot;: {}}</TemplateBody></GetTemplateResult></GetTemplateResponse>";
    assert_eq!(
        xml_tag(template, "TemplateBody").as_deref(),
        Some("{\"Resources\": {}}")
    );
}

#[test]
fn the_time_an_expiry_names_and_the_content_types_and_site_names_are_read_as_they_are_written() {
    let when = UNIX_EPOCH + Duration::from_secs(T0 + 900);
    assert_eq!(iso(when), "2026-10-24T12:15:00Z");
    assert_eq!(parse_time("2026-10-24T12:15:00Z"), Some(when));
    assert_eq!(parse_time("2026-10-24T12:15:00.000Z"), Some(when));
    assert_eq!(parse_time("yesterday"), None);

    assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
    assert_eq!(content_type("assets/app.js"), "application/javascript");
    assert_eq!(content_type("LICENSE"), "application/octet-stream");

    let refused = zip_files(&zipped(&[("../escape.html", "x")])).unwrap_err();
    assert!(refused.contains("escape"), "{refused}");
    assert!(
        zip_files(&zipped(&[])).is_err(),
        "a site with no files is refused"
    );
    assert_eq!(
        zip_files(&zipped(&[("ok/file-1_2.txt", "x")])).unwrap()[0].0,
        "ok/file-1_2.txt"
    );
}

#[test]
fn an_ecs_artifact_names_one_image_and_a_refusal_never_carries_a_secret() {
    let h = harness();
    let client = client(&h);
    let target = Target::Ecs {
        cluster: CLUSTER.into(),
        service: SERVICE.into(),
        family: FAMILY.into(),
        container: "web".into(),
        min_healthy: 100,
        max_percent: 200,
    };
    let err = client.upload(&target, b"two images here").unwrap_err();
    assert!(err.contains("one image"), "{err}");
    assert!(!err.contains(BASE_SK));
}

/// The environment the rollout reads its settings from.
fn env_with(vars: &[(&str, &str)]) -> Environment {
    Environment {
        name: "production".into(),
        kind: "production".into(),
        url: None,
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

fn published(version: &str) -> Release {
    Release {
        id: format!("rel-{version}"),
        project: "site".into(),
        repo: "site".into(),
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

/// The release's bytes, whatever target asks.
struct Bundle(Vec<u8>);

impl Artifacts for Bundle {
    fn bytes(&self, _release: &Release, _target: &str) -> Result<Vec<u8>, String> {
        Ok(self.0.clone())
    }
}

/// Health that is clean until its `breach_at`-th stage, then breaches the error rate.
struct Watch {
    calls: Mutex<u32>,
    breach_at: u32,
}

impl Checks for Watch {
    fn smoke(&self, _project: &str, _env: &Environment, _release: &Release) -> Result<(), String> {
        Ok(())
    }

    fn health(
        &self,
        _project: &str,
        _env: &Environment,
        _since_ms: u64,
        _settings: &Settings,
    ) -> Result<crate::deploy::health::Health, String> {
        let mut calls = self.calls.lock().unwrap();
        *calls += 1;
        let rate = if *calls >= self.breach_at { 0.2 } else { 0.0 };
        Ok(crate::deploy::health::Health {
            signals: 0,
            error_rate: Some(rate),
            p95_ms: Some(100.0),
        })
    }
}

#[derive(Default)]
struct Opened(Mutex<Vec<String>>);

impl Incidents for Opened {
    fn open(&self, _project: &str, _env: &str, _version: &str, detail: &str) {
        self.0.lock().unwrap().push(detail.to_string());
    }
}
