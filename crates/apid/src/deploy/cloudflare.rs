//! The Cloudflare calls of a deploy (PLAN 23): Workers' versions and percentage deployments, and Pages' asset uploads,
//! deployments and rollbacks. The calls go through the platforms' one HTTP path (`platforms::http`), so a test replaces
//! it with a fake that records each request.
//!
//! The token is the Cloudflare connector's key, read on the hub (`connectors::hub_values`) and attached to each request
//! here, as the hub's own credential. It is never in a prompt, a fork, an error or a log: an error text is redacted of it.
//! The request shapes follow Cloudflare's public API reference as written for this release; they are not checked
//! against a live account here.

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

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

    /// One call: `path` under the account, with the connector's token. The token goes in the header only.
    fn send(&self, method: &str, path: &str, body: Option<Body>) -> Result<Value, String> {
        let url = format!("{}/accounts/{}{path}", self.base, self.account);
        self.request(method, &url, &self.token, body)
    }

    /// One call to `url`, with `bearer` as its credential: the connector's token, or a Pages project's upload token.
    fn request(
        &self,
        method: &str,
        url: &str,
        bearer: &str,
        body: Option<Body>,
    ) -> Result<Value, String> {
        let headers = [("Authorization", format!("Bearer {bearer}"))];
        let reply = self
            .http
            .call(method, url, &headers, body)
            .map_err(|e| self.redact(&e, bearer))?;
        let refused =
            reply.status < 200 || reply.status >= 300 || reply.body["success"] == json!(false);
        if refused {
            let message = reply.body["errors"][0]["message"]
                .as_str()
                .unwrap_or("the API refused the call");
            return Err(self.redact(
                &format!("cloudflare answered {}: {message}", reply.status),
                bearer,
            ));
        }
        Ok(reply.body)
    }

    /// Text with the token and the bearer taken out, for any error that might carry either.
    fn redact(&self, text: &str, bearer: &str) -> String {
        text.replace(&self.token, "[redacted]")
            .replace(bearer, "[redacted]")
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
        let v = match target {
            Target::Workers(script) => {
                let metadata = json!({ "main_module": WORKER_MODULE }).to_string();
                let (ct, body) = multipart(&[
                    Part::text("metadata", &metadata),
                    Part::file(WORKER_MODULE, "application/javascript+module", bytes),
                ]);
                self.send(
                    "POST",
                    &format!("/workers/scripts/{script}/versions"),
                    Some(Body::Bytes(ct, body)),
                )?
            }
            Target::Pages(project) => self.pages_deploy(project, PAGES_BRANCH, bytes)?,
        };
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

/// The Pages calls a Pages deployment makes, production and preview alike: the content is stored in the project's asset
/// store under its hash before a deployment names that hash.
impl Client {
    /// Deploys `bytes` as a Pages deployment on `branch`, and returns the answer. The project's upload token is asked
    /// for; the hash is checked against the asset store, and the content uploaded when the store lacks it; the hash is
    /// then marked present. Only after that is the deployment made, with its manifest and no file part.
    fn pages_deploy(&self, project: &str, branch: &str, bytes: &[u8]) -> Result<Value, String> {
        let hash = asset_hash(bytes, "html");
        let token = self.send(
            "GET",
            &format!("/pages/projects/{project}/upload-token"),
            None,
        )?;
        let jwt = token["result"]["jwt"]
            .as_str()
            .filter(|t| !t.is_empty())
            .ok_or_else(|| format!("pages:{project}: the upload token answered no token"))?;
        let check = self.assets("check-missing", jwt, json!({ "hashes": [hash.as_str()] }))?;
        let missing = check["result"]
            .as_array()
            .ok_or_else(|| format!("pages:{project}: the asset check answered no list"))?;
        if missing.iter().any(|h| h.as_str() == Some(hash.as_str())) {
            self.assets(
                "upload",
                jwt,
                json!([{
                    "key": hash.as_str(),
                    "value": STANDARD.encode(bytes),
                    "metadata": { "contentType": "text/html" },
                    "base64": true
                }]),
            )?;
        }
        self.assets("upsert-hashes", jwt, json!({ "hashes": [hash.as_str()] }))?;
        let manifest = json!({ PAGES_FILE: hash.as_str() }).to_string();
        let (ct, body) = multipart(&[
            Part::text("manifest", &manifest),
            Part::text("branch", branch),
        ]);
        self.send(
            "POST",
            &format!("/pages/projects/{project}/deployments"),
            Some(Body::Bytes(ct, body)),
        )
    }

    /// One call to the Pages asset store. It takes the project's upload token, not the account's token.
    fn assets(&self, action: &str, jwt: &str, body: Value) -> Result<Value, String> {
        let url = format!("{}/pages/assets/{action}", self.base);
        self.request("POST", &url, jwt, Some(Body::Json(body)))
    }
}

/// The ephemeral environments' calls (`ephemeral`): a Pages deployment on a branch of its own (a preview URL), a
/// Worker script of its own, and their removal.
impl Client {
    /// Uploads `bytes` as a Pages deployment on `branch` (a preview, not the production branch). Its id and its preview
    /// URL, when Pages names one.
    pub fn pages_preview(
        &self,
        project: &str,
        branch: &str,
        bytes: &[u8],
    ) -> Result<(String, Option<String>), String> {
        let v = self.pages_deploy(project, branch, bytes)?;
        let id = v["result"]["id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("pages:{project}: the upload answered no id"))?;
        let url = v["result"]["url"].as_str().map(str::to_string);
        Ok((id, url))
    }

    /// Removes a Pages deployment, forced: a preview with its files.
    pub fn pages_remove(&self, project: &str, deployment: &str) -> Result<(), String> {
        self.send(
            "DELETE",
            &format!("/pages/projects/{project}/deployments/{deployment}?force=true"),
            None,
        )
        .map(|_| ())
    }

    /// Uploads a Worker script of its own under `name` (a preview's script, apart from the production one).
    pub fn script_put(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let metadata = json!({ "main_module": WORKER_MODULE }).to_string();
        let (ct, body) = multipart(&[
            Part::text("metadata", &metadata),
            Part::file(WORKER_MODULE, "application/javascript+module", bytes),
        ]);
        self.send(
            "PUT",
            &format!("/workers/scripts/{name}"),
            Some(Body::Bytes(ct, body)),
        )
        .map(|_| ())
    }

    /// Removes a Worker script; one already gone is not an error.
    pub fn script_delete(&self, name: &str) -> Result<(), String> {
        match self.send("DELETE", &format!("/workers/scripts/{name}"), None) {
            Err(e) if e.contains("answered 404") => Ok(()),
            other => other.map(|_| ()),
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

/// The key a file's content is stored under in Pages' asset store: the first 32 hex characters of blake3 over the
/// content's base64 followed by its extension without the dot (`html` for `/index.html`), as wrangler computes it.
pub(super) fn asset_hash(bytes: &[u8], extension: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(STANDARD.encode(bytes).as_bytes());
    hasher.update(extension.as_bytes());
    hasher.finalize().to_hex()[..32].to_string()
}
