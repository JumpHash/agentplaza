//! The Cloudflare calls of a deploy (PLAN 23): Workers' versions and percentage deployments, and Pages' deployments and
//! rollbacks. The calls go through the platforms' one HTTP path (`platforms::http`), so a test replaces it with a fake
//! that records each request.
//!
//! The token is the Cloudflare connector's key, read on the hub (`connectors::hub_values`) and attached to each request
//! here, as the hub's own credential. It is never in a prompt, a fork, an error or a log: an error text is redacted of it.
//! The request shapes follow Cloudflare's public API reference as written for this release; they are not checked
//! against a live account here.

use std::sync::Arc;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::platforms::http::{Body, Http};

/// The API's address, unless the hub is started with another (a test's fake, `APZ_CLOUDFLARE_API`).
pub const DEFAULT_BASE: &str = "https://api.cloudflare.com/client/v4";
pub const BASE_ENV: &str = "APZ_CLOUDFLARE_API";
/// The module name a Workers version is uploaded as.
const WORKER_MODULE: &str = "worker.js";
/// The file a Pages deployment serves its artifact as.
const PAGES_FILE: &str = "/index.html";
/// The branch a Pages deployment is made from (the production branch by default).
const PAGES_BRANCH: &str = "main";

/// What a deploy targets on Cloudflare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A Worker script: versions, and percentage deployments between them.
    Workers(String),
    /// A Pages project: deployments, and rollback to an earlier one.
    Pages(String),
}

impl Target {
    /// The target as the logs and the chat name it: `workers:site`.
    pub fn label(&self) -> String {
        match self {
            Target::Workers(name) => format!("workers:{name}"),
            Target::Pages(name) => format!("pages:{name}"),
        }
    }
}

/// The production client: one account, one token, the platforms' HTTP path.
pub struct Client {
    http: Arc<dyn Http>,
    base: String,
    account: String,
    token: String,
}

impl Client {
    /// A client for `account` on `base`, with the connector's `token`. An empty token or account is refused.
    pub fn new(
        http: Arc<dyn Http>,
        base: &str,
        account: &str,
        token: &str,
    ) -> Result<Client, String> {
        if account.trim().is_empty() {
            return Err("the Cloudflare connector has no account ID".into());
        }
        if token.trim().is_empty() {
            return Err("the Cloudflare connector holds no API token".into());
        }
        Ok(Client {
            http,
            base: base.trim_end_matches('/').to_string(),
            account: account.trim().to_string(),
            token: token.trim().to_string(),
        })
    }

    /// The address to call: `APZ_CLOUDFLARE_API` when set, else the API.
    pub fn base_from_env() -> String {
        std::env::var(BASE_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_BASE.to_string())
    }

    /// One call: `path` under the account. The token goes in the header only.
    fn send(&self, method: &str, path: &str, body: Option<Body>) -> Result<Value, String> {
        let url = format!("{}/accounts/{}{path}", self.base, self.account);
        let headers = [("Authorization", format!("Bearer {}", self.token))];
        let reply = self
            .http
            .call(method, &url, &headers, body)
            .map_err(|e| self.redact(&e))?;
        let refused =
            reply.status < 200 || reply.status >= 300 || reply.body["success"] == json!(false);
        if refused {
            let message = reply.body["errors"][0]["message"]
                .as_str()
                .unwrap_or("the API refused the call");
            return Err(self.redact(&format!("cloudflare answered {}: {message}", reply.status)));
        }
        Ok(reply.body)
    }

    /// Text with the token taken out, for any error that might carry it.
    fn redact(&self, text: &str) -> String {
        text.replace(&self.token, "[redacted]")
    }
}

impl Client {
    pub fn live(&self, target: &Target) -> Result<Option<String>, String> {
        match target {
            Target::Workers(script) => {
                let v = self.send(
                    "GET",
                    &format!("/workers/scripts/{script}/deployments"),
                    None,
                )?;
                let deployments = v["result"]["deployments"]
                    .as_array()
                    .or_else(|| v["result"].as_array());
                let Some(first) = deployments.and_then(|d| d.first()) else {
                    return Ok(None);
                };
                // The version that serves the most of the traffic is the one a rollback returns to.
                Ok(first["versions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .max_by_key(|x| x["percentage"].as_u64().unwrap_or(0))
                    .and_then(|x| x["version_id"].as_str())
                    .map(str::to_string))
            }
            Target::Pages(project) => {
                let v = self.send(
                    "GET",
                    &format!("/pages/projects/{project}/deployments"),
                    None,
                )?;
                Ok(v["result"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|d| d["environment"] == json!("production"))
                    .and_then(|d| d["id"].as_str())
                    .map(str::to_string))
            }
        }
    }

    pub fn upload(&self, target: &Target, bytes: &[u8]) -> Result<String, String> {
        let (path, content_type, body) = match target {
            Target::Workers(script) => {
                let metadata = json!({ "main_module": WORKER_MODULE }).to_string();
                let (ct, body) = multipart(&[
                    Part::text("metadata", &metadata),
                    Part::file(WORKER_MODULE, "application/javascript+module", bytes),
                ]);
                (format!("/workers/scripts/{script}/versions"), ct, body)
            }
            Target::Pages(project) => {
                let manifest = json!({ PAGES_FILE: sha256_hex(bytes) }).to_string();
                let (ct, body) = multipart(&[
                    Part::text("manifest", &manifest),
                    Part::text("branch", PAGES_BRANCH),
                    Part::file(PAGES_FILE, "text/html", bytes),
                ]);
                (format!("/pages/projects/{project}/deployments"), ct, body)
            }
        };
        let v = self.send("POST", &path, Some(Body::Bytes(content_type, body)))?;
        v["result"]["id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("{}: the upload answered no id", target.label()))
    }

    pub fn split(&self, target: &Target, versions: &[(String, u32)]) -> Result<(), String> {
        match target {
            Target::Workers(script) => {
                let total: u32 = versions.iter().map(|(_, p)| *p).sum();
                if total != 100 || versions.is_empty() {
                    return Err("the percentages of a deployment sum to 100".into());
                }
                let list: Vec<Value> = versions
                    .iter()
                    .map(|(id, pct)| json!({ "version_id": id, "percentage": pct }))
                    .collect();
                self.send(
                    "POST",
                    &format!("/workers/scripts/{script}/deployments"),
                    Some(Body::Json(
                        json!({ "strategy": "percentage", "versions": list }),
                    )),
                )?;
                Ok(())
            }
            // Pages serves one deployment at a time: the upload is the stage.
            Target::Pages(_) => match versions {
                [(_, 100)] => Ok(()),
                _ => Err("Pages serves one deployment at 100%".into()),
            },
        }
    }

    pub fn restore(&self, target: &Target, to: &str) -> Result<(), String> {
        match target {
            Target::Workers(_) => self.split(target, &[(to.to_string(), 100)]),
            Target::Pages(project) => {
                self.send(
                    "POST",
                    &format!("/pages/projects/{project}/deployments/{to}/rollback"),
                    None,
                )?;
                Ok(())
            }
        }
    }
}

/// One part of a multipart body.
pub struct Part<'a> {
    name: &'a str,
    filename: Option<&'a str>,
    content_type: &'a str,
    bytes: &'a [u8],
}

impl<'a> Part<'a> {
    fn text(name: &'a str, value: &'a str) -> Part<'a> {
        Part {
            name,
            filename: None,
            content_type: "application/json",
            bytes: value.as_bytes(),
        }
    }

    fn file(name: &'a str, content_type: &'a str, bytes: &'a [u8]) -> Part<'a> {
        Part {
            name,
            filename: Some(name),
            content_type,
            bytes,
        }
    }
}

/// A `multipart/form-data` body: its content type (with the boundary) and its bytes.
pub fn multipart(parts: &[Part]) -> (String, Vec<u8>) {
    const BOUNDARY: &str = "apzdeploy7f3a9c51e2";
    let mut body: Vec<u8> = Vec::new();
    for part in parts {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        let file = part
            .filename
            .map(|f| format!("; filename=\"{f}\""))
            .unwrap_or_default();
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"{}\"{file}\r\nContent-Type: {}\r\n\r\n",
                part.name, part.content_type
            )
            .as_bytes(),
        );
        body.extend_from_slice(part.bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={BOUNDARY}"), body)
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
