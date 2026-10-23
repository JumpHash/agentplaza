//! GitHub (github.com and GitHub Enterprise Server): the plaza's own GitHub App and the owner's sign-in.
//!
//! - **The app.** The owner creates it from a manifest (one click on GitHub: no fields to copy): GitHub hands back its
//!   id, client id and secret and private key ([`GitHub::convert`]). It asks for contents and workflows (write) and
//!   metadata (read) on repositories, and the owner's emails (read) and SSH signing keys (write) on their account.
//! - **Installations.** The owner installs the app on their user or on an organisation and picks there, on GitHub,
//!   every repository or the ones named: that is the user and organisation level, and GitHub's own choice of
//!   repositories. The hub signs a short JWT with the app's key ([`app_jwt`]) for an installation's token, which lives
//!   an hour and is minted when needed.
//! - **The owner's sign-in.** Installing also authorizes the app for the owner (`request_oauth_on_install`): that token
//!   says who the owner is (the author of exported commits) and registers the hub's signing key on their account.

use std::sync::Arc;

use base64::Engine;
use serde_json::{Value, json};

use super::http::{Body, Http, escape};
use super::model::{Account, Auth, Kind, Namespace, Platform, Repo, Tokens};

/// The GitHub API's version header.
const API_VERSION: &str = "2022-11-28";
/// The most pages one listing reads.
const PAGES: usize = 50;

pub struct GitHub {
    web: String,
    api: String,
    http: Arc<dyn Http>,
}

/// What GitHub hands back when the owner creates the app from its manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppCredentials {
    pub id: u64,
    pub slug: String,
    pub name: String,
    pub owner: String,
    pub client_id: String,
    pub client_secret: String,
    /// The app's private key (PEM).
    pub pem: String,
    pub html_url: String,
}

/// One installation of the app: on a user or an organisation, with every repository or the ones chosen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Installation {
    pub id: u64,
    pub account: String,
    /// `User` or `Organization`.
    pub account_type: String,
    pub account_id: u64,
    /// `all` or `selected`.
    pub repository_selection: String,
    /// Where the owner changes it on GitHub.
    pub html_url: String,
}

impl GitHub {
    pub fn new(host: &str, http: Arc<dyn Http>) -> Self {
        GitHub {
            web: host.trim_end_matches('/').to_string(),
            api: Kind::Github.api_base(host),
            http,
        }
    }

    fn get(&self, token: &str, path: &str, what: &str) -> Result<Value, String> {
        self.http
            .call("GET", &format!("{}{path}", self.api), &headers(token), None)?
            .ok(what)
    }

    /// Every page of a listing (`per_page=100`); `items` picks the array out of a page.
    fn pages(
        &self,
        token: &str,
        path: &str,
        what: &str,
        items: impl Fn(&Value) -> Vec<Value>,
    ) -> Result<Vec<Value>, String> {
        let sep = if path.contains('?') { '&' } else { '?' };
        let mut out = Vec::new();
        for page in 1..=PAGES {
            let v = self.get(token, &format!("{path}{sep}per_page=100&page={page}"), what)?;
            let got = items(&v);
            let n = got.len();
            out.extend(got);
            if n < 100 {
                break;
            }
        }
        Ok(out)
    }

    // ── the app ──────────────────────────────────────────────────────────────────────────────────────────────

    /// The app's manifest: its name, where GitHub sends the owner back (`callback`, the hub's public callback), and
    /// what it may do. No webhooks: Weft only exports.
    pub fn manifest(name: &str, hub_url: &str, callback: &str) -> Value {
        json!({
            "name": name,
            "url": hub_url,
            "description": "AgentPlaza's version control (Weft) exports landed work to the repositories you choose.",
            "public": false,
            "redirect_url": callback,
            "callback_urls": [callback],
            "setup_url": callback,
            "setup_on_update": true,
            "request_oauth_on_install": true,
            "hook_attributes": { "url": hub_url, "active": false },
            "default_permissions": {
                "contents": "write",
                "workflows": "write",
                "metadata": "read",
                "email_addresses": "read",
                "ssh_signing_keys": "write"
            },
            "default_events": []
        })
    }

    /// Where the manifest is posted (a form with a `manifest` field): the owner's account, or an organisation's.
    pub fn manifest_url(&self, org: Option<&str>, state: &str) -> String {
        match org {
            Some(org) => format!(
                "{}/organizations/{}/settings/apps/new?state={}",
                self.web,
                escape(org),
                escape(state)
            ),
            None => format!("{}/settings/apps/new?state={}", self.web, escape(state)),
        }
    }

    /// The app's credentials for the `code` GitHub sent back after the owner created it.
    pub fn convert(&self, code: &str) -> Result<AppCredentials, String> {
        let v = self
            .http
            .call(
                "POST",
                &format!("{}/app-manifests/{}/conversions", self.api, escape(code)),
                &version(),
                None,
            )?
            .ok("creating the GitHub App")?;
        Ok(AppCredentials {
            id: v["id"].as_u64().unwrap_or(0),
            slug: s(&v["slug"]),
            name: s(&v["name"]),
            owner: s(&v["owner"]["login"]),
            client_id: s(&v["client_id"]),
            client_secret: s(&v["client_secret"]),
            pem: s(&v["pem"]),
            html_url: s(&v["html_url"]),
        })
    }

    /// Where the owner installs the app (and picks its repositories).
    pub fn install_url(&self, slug: &str, state: &str) -> String {
        format!(
            "{}/apps/{}/installations/new?state={}",
            self.web,
            escape(slug),
            escape(state)
        )
    }

    /// Every installation of the app.
    pub fn installations(&self, jwt: &str) -> Result<Vec<Installation>, String> {
        let v = self.get_jwt(
            jwt,
            "/app/installations?per_page=100",
            "listing the installations",
        )?;
        Ok(v.as_array()
            .into_iter()
            .flatten()
            .map(installation_of)
            .collect())
    }

    /// A token for one installation, and when it expires (ms).
    pub fn installation_token(&self, jwt: &str, id: u64) -> Result<(String, Option<u64>), String> {
        let v = self
            .http
            .call(
                "POST",
                &format!("{}/app/installations/{id}/access_tokens", self.api),
                &[
                    ("Authorization", format!("Bearer {jwt}")),
                    ("X-GitHub-Api-Version", API_VERSION.to_string()),
                ],
                None,
            )?
            .ok("a token for the installation")?;
        Ok((
            s(&v["token"]),
            v["expires_at"].as_str().and_then(rfc3339_ms),
        ))
    }

    fn get_jwt(&self, jwt: &str, path: &str, what: &str) -> Result<Value, String> {
        self.http
            .call(
                "GET",
                &format!("{}{path}", self.api),
                &[
                    ("Authorization", format!("Bearer {jwt}")),
                    ("X-GitHub-Api-Version", API_VERSION.to_string()),
                ],
                None,
            )?
            .ok(what)
    }
}

impl Platform for GitHub {
    fn authorize_url(
        &self,
        client_id: &str,
        redirect: &str,
        state: &str,
        challenge: &str,
    ) -> String {
        format!(
            "{}/login/oauth/authorize?client_id={}&redirect_uri={}&state={}&code_challenge={}&code_challenge_method=S256",
            self.web,
            escape(client_id),
            escape(redirect),
            escape(state),
            escape(challenge)
        )
    }

    fn exchange(
        &self,
        client_id: &str,
        client_secret: &str,
        code: &str,
        redirect: &str,
        verifier: &str,
    ) -> Result<Tokens, String> {
        let mut body = json!({ "client_id": client_id, "client_secret": client_secret, "code": code, "redirect_uri": redirect });
        // The sign-in that comes with an installation has no PKCE: no verifier to send.
        if !verifier.is_empty() {
            body["code_verifier"] = json!(verifier);
        }
        self.token(body, "signing in to GitHub")
    }

    fn refresh(
        &self,
        client_id: &str,
        client_secret: &str,
        refresh: &str,
        _redirect: &str,
    ) -> Result<Tokens, String> {
        let body = json!({ "client_id": client_id, "client_secret": client_secret, "grant_type": "refresh_token", "refresh_token": refresh });
        self.token(body, "renewing the GitHub sign-in")
    }

    fn account(&self, auth: &Auth) -> Result<Account, String> {
        let u = self.get(&auth.token, "/user", "reading the GitHub account")?;
        let id = u["id"].as_u64().unwrap_or(0);
        let login = s(&u["login"]);
        // The primary verified email when the app may read them; else the public one; else the no-reply address.
        let emails = self
            .get(&auth.token, "/user/emails", "reading the account's emails")
            .unwrap_or(Value::Null);
        let list = emails.as_array().cloned().unwrap_or_default();
        let verified = |primary: bool| {
            list.iter()
                .find(|e| e["verified"] == true && (!primary || e["primary"] == true))
                .map(|e| s(&e["email"]))
        };
        let email = verified(true)
            .or_else(|| verified(false))
            .or_else(|| {
                u["email"]
                    .as_str()
                    .filter(|e| !e.is_empty())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| format!("{id}+{login}@users.noreply.github.com"));
        Ok(Account {
            id,
            name: u["name"]
                .as_str()
                .filter(|n| !n.is_empty())
                .unwrap_or(&login)
                .to_string(),
            login,
            email,
            avatar_url: s(&u["avatar_url"]),
        })
    }

    fn namespaces(&self, auth: &Auth, account: &Account) -> Result<Vec<Namespace>, String> {
        if auth.installation {
            // An installation reaches its account's repositories: the namespaces are their owners.
            let mut out: Vec<Namespace> = Vec::new();
            for r in self.installation_repos(&auth.token)? {
                let owner = &r["owner"];
                let name = s(&owner["login"]);
                if !out.iter().any(|n| n.name == name) {
                    out.push(Namespace {
                        kind: if owner["type"] == "Organization" {
                            "org".into()
                        } else {
                            "user".into()
                        },
                        display: name.clone(),
                        id: owner["id"].as_u64().unwrap_or(0),
                        name,
                    });
                }
            }
            return Ok(out);
        }
        let mut out = vec![Namespace {
            name: account.login.clone(),
            kind: "user".into(),
            display: account.name.clone(),
            id: account.id,
        }];
        let orgs = self.pages(
            &auth.token,
            "/user/orgs",
            "listing the organisations",
            |v| v.as_array().cloned().unwrap_or_default(),
        )?;
        out.extend(orgs.iter().map(|o| Namespace {
            name: s(&o["login"]),
            kind: "org".into(),
            display: s(&o["login"]),
            id: o["id"].as_u64().unwrap_or(0),
        }));
        Ok(out)
    }

    fn repos(&self, auth: &Auth, namespace: &Namespace) -> Result<Vec<Repo>, String> {
        let raw = if auth.installation {
            self.installation_repos(&auth.token)?
                .into_iter()
                .filter(|r| r["owner"]["login"].as_str() == Some(namespace.name.as_str()))
                .collect()
        } else if namespace.kind == "user" {
            self.pages(
                &auth.token,
                "/user/repos?affiliation=owner",
                "listing the repositories",
                |v| v.as_array().cloned().unwrap_or_default(),
            )?
        } else {
            self.pages(
                &auth.token,
                &format!("/orgs/{}/repos", escape(&namespace.name)),
                "listing the repositories",
                |v| v.as_array().cloned().unwrap_or_default(),
            )?
        };
        Ok(raw.iter().map(|r| repo_of(r, auth.installation)).collect())
    }

    fn repo(&self, auth: &Auth, full_name: &str) -> Result<Repo, String> {
        let (owner, name) = full_name
            .split_once('/')
            .ok_or("a GitHub repository is owner/name")?;
        let v = self.get(
            &auth.token,
            &format!("/repos/{}/{}", escape(owner), escape(name)),
            "reading the repository",
        )?;
        Ok(repo_of(&v, auth.installation))
    }

    fn add_signing_key(&self, auth: &Auth, title: &str, key: &str) -> Result<(), String> {
        let reply = self.http.call(
            "POST",
            &format!("{}/user/ssh_signing_keys", self.api),
            &headers(&auth.token),
            Some(Body::Json(json!({ "title": title, "key": key }))),
        )?;
        let taken = reply.status == 422 && reply.body.to_string().contains("already");
        if taken {
            return Ok(());
        }
        reply.ok("registering the signing key").map(|_| ())
    }
}

/// A pull request as GitHub lists it (`ci` mode: one per lane, PLAN 21.5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pull {
    pub number: u64,
    /// `open` or `closed`.
    pub state: String,
    /// Merged (on GitHub): its merge commit, when GitHub gives one.
    pub merged: bool,
    pub merge_commit: Option<String>,
    pub html_url: String,
}

/// A check run of a commit (a GitHub Actions job or another app's check).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckRun {
    pub name: String,
    /// `queued`, `in_progress` or `completed`.
    pub status: String,
    /// On a completed run: `success`, `failure`, `neutral`, `cancelled`, `skipped`, `timed_out`, `action_required`, …
    pub conclusion: Option<String>,
    pub html_url: String,
}

fn pull_of(v: &Value) -> Pull {
    Pull {
        number: v["number"].as_u64().unwrap_or(0),
        state: s(&v["state"]),
        merged: v["merged_at"].as_str().is_some() || v["merged"] == true,
        merge_commit: v["merge_commit_sha"].as_str().map(str::to_string),
        html_url: s(&v["html_url"]),
    }
}

impl GitHub {
    /// The pull requests of `owner/repo` whose head is `branch` on the same repository, open or not.
    pub fn pulls(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<Vec<Pull>, String> {
        let head = escape(&format!("{owner}:{branch}"));
        let raw = self.pages(
            token,
            &format!(
                "/repos/{}/{}/pulls?state=all&head={head}",
                escape(owner),
                escape(repo)
            ),
            "listing the pull requests",
            |v| v.as_array().cloned().unwrap_or_default(),
        )?;
        Ok(raw.iter().map(pull_of).collect())
    }

    /// Opens a pull request from `head` into `base` (a lane's branch into the repository's main).
    pub fn open_pull(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        title: &str,
        head: &str,
        base: &str,
        body: &str,
    ) -> Result<Pull, String> {
        let reply = self.http.call(
            "POST",
            &format!(
                "{}/repos/{}/{}/pulls",
                self.api,
                escape(owner),
                escape(repo)
            ),
            &headers(token),
            Some(Body::Json(
                json!({ "title": title, "head": head, "base": base, "body": body }),
            )),
        )?;
        reply.ok("opening the pull request").map(|v| pull_of(&v))
    }

    /// The check runs of a commit, every page.
    pub fn check_runs(
        &self,
        token: &str,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<Vec<CheckRun>, String> {
        let raw = self.pages(
            token,
            &format!(
                "/repos/{}/{}/commits/{}/check-runs?filter=latest",
                escape(owner),
                escape(repo),
                escape(sha)
            ),
            "listing the check runs",
            |v| v["check_runs"].as_array().cloned().unwrap_or_default(),
        )?;
        Ok(raw
            .iter()
            .map(|c| CheckRun {
                name: s(&c["name"]),
                status: s(&c["status"]),
                conclusion: c["conclusion"].as_str().map(str::to_string),
                html_url: s(&c["html_url"]),
            })
            .collect())
    }

    fn token(&self, body: Value, what: &str) -> Result<Tokens, String> {
        let v = self
            .http
            .call(
                "POST",
                &format!("{}/login/oauth/access_token", self.web),
                &[],
                Some(Body::Json(body)),
            )?
            .ok(what)?;
        // GitHub answers 200 with an `error` field when the code is bad.
        if let Some(e) = v["error"].as_str() {
            return Err(format!(
                "{what}: {}",
                v["error_description"].as_str().unwrap_or(e)
            ));
        }
        Ok(Tokens {
            access: s(&v["access_token"]),
            refresh: v["refresh_token"]
                .as_str()
                .filter(|r| !r.is_empty())
                .map(str::to_string),
            expires_in: v["expires_in"]
                .as_u64()
                .or_else(|| v["expires_in"].as_str().and_then(|x| x.parse().ok())),
            scopes: v["scope"]
                .as_str()
                .unwrap_or("")
                .split([',', ' '])
                .filter(|x| !x.is_empty())
                .map(str::to_string)
                .collect(),
        })
    }

    fn installation_repos(&self, token: &str) -> Result<Vec<Value>, String> {
        self.pages(
            token,
            "/installation/repositories",
            "listing the installation's repositories",
            |v| v["repositories"].as_array().cloned().unwrap_or_default(),
        )
    }
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

fn version() -> Vec<(&'static str, String)> {
    vec![("X-GitHub-Api-Version", API_VERSION.to_string())]
}

fn headers(token: &str) -> Vec<(&'static str, String)> {
    vec![
        ("Authorization", format!("Bearer {token}")),
        ("X-GitHub-Api-Version", API_VERSION.to_string()),
    ]
}

fn installation_of(v: &Value) -> Installation {
    Installation {
        id: v["id"].as_u64().unwrap_or(0),
        account: s(&v["account"]["login"]),
        account_type: s(&v["account"]["type"]),
        account_id: v["account"]["id"].as_u64().unwrap_or(0),
        repository_selection: s(&v["repository_selection"]),
        html_url: s(&v["html_url"]),
    }
}

/// A repository as GitHub lists it. An installation's listing carries no permissions: the app may push (contents:
/// write) to every repository it was given.
fn repo_of(r: &Value, installation: bool) -> Repo {
    let full_name = s(&r["full_name"]);
    let (namespace, name) = full_name
        .rsplit_once('/')
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .unwrap_or_default();
    let perms = &r["permissions"];
    Repo {
        namespace,
        name,
        private: r["private"].as_bool().unwrap_or(false),
        default_branch: r["default_branch"].as_str().unwrap_or("main").to_string(),
        clone_url: s(&r["clone_url"]),
        web_url: s(&r["html_url"]),
        can_push: if perms.is_object() {
            perms["push"] == true || perms["admin"] == true
        } else {
            installation
        },
        full_name,
    }
}

/// `2026-10-18T12:00:00Z` in ms.
fn rfc3339_ms(raw: &str) -> Option<u64> {
    crate::ide::rfc3339_ms(raw)
}

/// The app's JWT: RS256 over `{iat, exp, iss}`, valid nine minutes (a minute back for clock drift). `iss` is the
/// app's client id (GitHub takes it in place of the numeric id).
pub fn app_jwt(issuer: &str, pem: &str, now_s: u64) -> Result<String, String> {
    use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let header = b64(br#"{"alg":"RS256","typ":"JWT"}"#);
    let claims = b64(
        json!({ "iat": now_s.saturating_sub(60), "exp": now_s + 540, "iss": issuer })
            .to_string()
            .as_bytes(),
    );
    let input = format!("{header}.{claims}");
    let der = pem_der(pem)?;
    let key = if pem.contains("BEGIN RSA PRIVATE KEY") {
        RsaKeyPair::from_der(&der)
    } else {
        RsaKeyPair::from_pkcs8(&der)
    }
    .map_err(|e| format!("the GitHub App's private key: {e}"))?;
    let mut sig = vec![0; key.public().modulus_len()];
    key.sign(
        &RSA_PKCS1_SHA256,
        &ring::rand::SystemRandom::new(),
        input.as_bytes(),
        &mut sig,
    )
    .map_err(|_| "signing the GitHub App's JWT failed".to_string())?;
    Ok(format!("{input}.{}", b64(&sig)))
}

/// The DER inside a PEM block.
fn pem_der(pem: &str) -> Result<Vec<u8>, String> {
    let body: String = pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .map(str::trim)
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(body)
        .map_err(|e| format!("the GitHub App's private key is not PEM: {e}"))
}

#[cfg(test)]
mod tests {
    use super::super::http::Fake;
    use super::*;

    const KEY: &str = include_str!("testdata/github-app-test-key.pem");

    fn github(
        answer: impl Fn(&str, &str, Option<&Body>) -> (u16, Value) + Send + Sync + 'static,
    ) -> (GitHub, Arc<Fake>) {
        let fake = Arc::new(Fake::new(answer));
        (GitHub::new("https://github.com", fake.clone()), fake)
    }

    #[test]
    fn the_app_jwt_is_rs256_signed_by_the_apps_key() {
        use ring::signature::{RSA_PKCS1_2048_8192_SHA256, RsaKeyPair, UnparsedPublicKey};
        let jwt = app_jwt("Iv23li", KEY, 1_800_000_000).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let decode = |p: &str| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(p)
                .unwrap()
        };
        let claims: Value = serde_json::from_slice(&decode(parts[1])).unwrap();
        assert_eq!(
            claims,
            json!({ "iat": 1_799_999_940u64, "exp": 1_800_000_540u64, "iss": "Iv23li" })
        );
        let key = RsaKeyPair::from_der(&pem_der(KEY).unwrap()).unwrap();
        let public = UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, key.public().as_ref());
        public
            .verify(
                format!("{}.{}", parts[0], parts[1]).as_bytes(),
                &decode(parts[2]),
            )
            .expect("the signature verifies with the app's public key");
        assert!(app_jwt("x", "not a key", 0).is_err());
    }

    #[test]
    fn the_manifest_asks_for_what_weft_needs_and_no_webhooks() {
        let m = GitHub::manifest(
            "AgentPlaza hub-1",
            "https://hub.example",
            "https://hub.example/v1/platforms/callback",
        );
        assert_eq!(m["default_permissions"]["contents"], "write");
        assert_eq!(m["default_permissions"]["ssh_signing_keys"], "write");
        assert_eq!(m["hook_attributes"]["active"], false);
        assert_eq!(m["request_oauth_on_install"], true);
        assert_eq!(
            m["redirect_url"],
            "https://hub.example/v1/platforms/callback"
        );
        let (gh, _) = github(|_, _, _| (200, Value::Null));
        assert_eq!(
            gh.manifest_url(None, "s1"),
            "https://github.com/settings/apps/new?state=s1"
        );
        assert_eq!(
            gh.manifest_url(Some("JumpHash"), "s1"),
            "https://github.com/organizations/JumpHash/settings/apps/new?state=s1"
        );
        assert_eq!(
            gh.install_url("agentplaza-hub", "s2"),
            "https://github.com/apps/agentplaza-hub/installations/new?state=s2"
        );
        assert_eq!(
            GitHub::new(
                "https://ghe.example/",
                Arc::new(Fake::new(|_, _, _| (200, Value::Null)))
            )
            .api,
            "https://ghe.example/api/v3"
        );
    }

    #[test]
    fn the_manifest_code_converts_to_the_apps_credentials() {
        let (gh, fake) = github(|m, u, _| match (m, u) {
            ("POST", "https://api.github.com/app-manifests/c0de/conversions") => (
                201,
                json!({
                    "id": 42, "slug": "agentplaza-hub", "name": "AgentPlaza hub", "owner": { "login": "irfan" },
                    "client_id": "Iv23li", "client_secret": "s3cret", "pem": "-----BEGIN RSA PRIVATE KEY-----\n...", "html_url": "https://github.com/apps/agentplaza-hub"
                }),
            ),
            _ => (404, json!({ "message": "Not Found" })),
        });
        let app = gh.convert("c0de").unwrap();
        assert_eq!(
            (
                app.id,
                app.slug.as_str(),
                app.client_id.as_str(),
                app.owner.as_str()
            ),
            (42, "agentplaza-hub", "Iv23li", "irfan")
        );
        assert!(app.pem.starts_with("-----BEGIN"));
        assert_eq!(
            fake.urls(),
            ["POST https://api.github.com/app-manifests/c0de/conversions"]
        );
        assert!(gh.convert("expired").unwrap_err().contains("404 Not Found"));
    }

    #[test]
    fn installations_and_their_tokens_use_the_apps_jwt() {
        let (gh, fake) = github(|m, u, _| match (m, u) {
            ("GET", "https://api.github.com/app/installations?per_page=100") => (
                200,
                json!([{ "id": 7, "account": { "login": "JumpHash", "type": "Organization", "id": 9 }, "repository_selection": "selected", "html_url": "https://github.com/organizations/JumpHash/settings/installations/7" }]),
            ),
            ("POST", "https://api.github.com/app/installations/7/access_tokens") => (
                201,
                json!({ "token": "ghs_1", "expires_at": "2026-10-18T13:00:00Z" }),
            ),
            _ => (404, json!({ "message": "Not Found" })),
        });
        let i = gh.installations("jwt.x.y").unwrap().remove(0);
        assert_eq!(
            (
                i.account.as_str(),
                i.account_type.as_str(),
                i.repository_selection.as_str()
            ),
            ("JumpHash", "Organization", "selected")
        );
        let (token, expires) = gh.installation_token("jwt.x.y", 7).unwrap();
        assert_eq!(token, "ghs_1");
        assert_eq!(expires, crate::ide::rfc3339_ms("2026-10-18T13:00:00Z"));
        assert!(
            fake.seen()
                .iter()
                .all(|(_, _, auth, _)| auth.as_deref() == Some("Bearer jwt.x.y"))
        );
    }

    #[test]
    fn the_owners_sign_in_gives_the_account_with_its_verified_email() {
        let (gh, fake) = github(|m, u, _| match (m, u) {
            ("POST", "https://github.com/login/oauth/access_token") => (
                200,
                json!({ "access_token": "ghu_1", "expires_in": 28800, "refresh_token": "ghr_1", "scope": "", "token_type": "bearer" }),
            ),
            ("GET", "https://api.github.com/user") => (
                200,
                json!({ "id": 5, "login": "irfan", "name": "Irfan S", "email": null, "avatar_url": "https://a/5" }),
            ),
            ("GET", "https://api.github.com/user/emails") => (
                200,
                json!([{ "email": "old@x.test", "verified": false, "primary": false }, { "email": "irfan@x.test", "verified": true, "primary": true }]),
            ),
            _ => (404, json!({ "message": "Not Found" })),
        });
        let t = gh
            .exchange(
                "Iv23li",
                "s3cret",
                "code1",
                "https://hub/v1/platforms/callback",
                "verifier",
            )
            .unwrap();
        assert_eq!(
            (t.access.as_str(), t.refresh.as_deref(), t.expires_in),
            ("ghu_1", Some("ghr_1"), Some(28800))
        );
        let a = gh.account(&Auth::user("ghu_1")).unwrap();
        assert_eq!(
            (a.login.as_str(), a.name.as_str(), a.email.as_str(), a.id),
            ("irfan", "Irfan S", "irfan@x.test", 5)
        );
        let (_, _, _, body) = &fake.seen()[0];
        assert_eq!(
            body.as_ref()
                .map(|b| matches!(b, Body::Json(v) if v["code_verifier"] == "verifier")),
            Some(true)
        );
    }

    #[test]
    fn without_readable_emails_the_noreply_address_is_the_author() {
        let (gh, _) = github(|_, u, _| match u {
            "https://api.github.com/user" => {
                (200, json!({ "id": 5, "login": "irfan", "name": "" }))
            }
            _ => (
                403,
                json!({ "message": "Resource not accessible by integration" }),
            ),
        });
        let a = gh.account(&Auth::user("t")).unwrap();
        assert_eq!(
            (a.name.as_str(), a.email.as_str()),
            ("irfan", "5+irfan@users.noreply.github.com")
        );
    }

    #[test]
    fn a_bad_code_is_an_error_even_on_a_200() {
        let (gh, _) = github(|_, _, _| {
            (
                200,
                json!({ "error": "bad_verification_code", "error_description": "The code passed is incorrect or expired." }),
            )
        });
        assert_eq!(
            gh.exchange("a", "b", "c", "d", "e").unwrap_err(),
            "signing in to GitHub: The code passed is incorrect or expired."
        );
    }

    #[test]
    fn an_installation_lists_its_repositories_by_owner_over_pages() {
        let page = |n: usize, start: usize| -> Value {
            let repos: Vec<Value> = (start..start + n)
                .map(|i| json!({ "full_name": format!("JumpHash/r{i}"), "owner": { "login": "JumpHash", "type": "Organization", "id": 9 }, "private": true, "default_branch": "main", "clone_url": format!("https://github.com/JumpHash/r{i}.git"), "html_url": "h" }))
                .collect();
            json!({ "total_count": 101, "repositories": repos })
        };
        let (gh, fake) = github(move |_, u, _| match u {
            u if u.ends_with("page=1") => (200, page(100, 0)),
            u if u.ends_with("page=2") => (200, page(1, 100)),
            _ => (404, json!({})),
        });
        let auth = Auth {
            token: "ghs_1".into(),
            installation: true,
        };
        let ns = gh.namespaces(&auth, &Account::default()).unwrap();
        assert_eq!(
            ns,
            [Namespace {
                name: "JumpHash".into(),
                kind: "org".into(),
                display: "JumpHash".into(),
                id: 9
            }]
        );
        let repos = gh.repos(&auth, &ns[0]).unwrap();
        assert_eq!(repos.len(), 101);
        assert_eq!(
            (
                repos[100].name.as_str(),
                repos[100].namespace.as_str(),
                repos[100].can_push
            ),
            ("r100", "JumpHash", true)
        );
        assert!(
            fake.urls()
                .iter()
                .all(|u| u.contains("/installation/repositories?per_page=100&page="))
        );
    }

    #[test]
    fn an_account_lists_its_own_and_its_organisations_repositories() {
        let (gh, fake) = github(|_, u, _| match u {
            u if u.starts_with("https://api.github.com/user/orgs") => {
                (200, json!([{ "login": "JumpHash", "id": 9 }]))
            }
            u if u.starts_with("https://api.github.com/user/repos?affiliation=owner") => (
                200,
                json!([{ "full_name": "irfan/notes", "private": false, "default_branch": "trunk", "clone_url": "c", "html_url": "h", "permissions": { "push": true } }]),
            ),
            u if u.starts_with("https://api.github.com/orgs/JumpHash/repos") => (
                200,
                json!([{ "full_name": "JumpHash/site", "permissions": { "push": false, "pull": true } }]),
            ),
            _ => (404, json!({})),
        });
        let auth = Auth::user("ghu_1");
        let me = Account {
            id: 5,
            login: "irfan".into(),
            name: "Irfan".into(),
            ..Account::default()
        };
        let ns = gh.namespaces(&auth, &me).unwrap();
        assert_eq!(
            ns.iter()
                .map(|n| (n.name.as_str(), n.kind.as_str()))
                .collect::<Vec<_>>(),
            [("irfan", "user"), ("JumpHash", "org")]
        );
        let mine = gh.repos(&auth, &ns[0]).unwrap();
        assert_eq!(
            (
                mine[0].full_name.as_str(),
                mine[0].default_branch.as_str(),
                mine[0].can_push
            ),
            ("irfan/notes", "trunk", true)
        );
        let theirs = gh.repos(&auth, &ns[1]).unwrap();
        assert!(!theirs[0].can_push, "a read-only membership cannot push");
        assert!(
            fake.seen()
                .iter()
                .all(|(_, _, a, _)| a.as_deref() == Some("Bearer ghu_1"))
        );
    }

    #[test]
    fn a_lane_branch_has_its_pull_request_and_its_check_runs() {
        let (gh, fake) = github(|m, u, b| match (m, u) {
            ("GET", u)
                if u.starts_with(
                    "https://api.github.com/repos/JumpHash/site/pulls?state=all&head=",
                ) =>
            {
                (
                    200,
                    json!([{ "number": 3, "state": "closed", "merged_at": "2026-10-23T10:00:00Z", "merge_commit_sha": "abc", "html_url": "https://github.example/pull/3" }]),
                )
            }
            ("POST", "https://api.github.com/repos/JumpHash/site/pulls") => {
                assert!(
                    matches!(b, Some(Body::Json(v)) if v["head"] == "agentplaza/w-12" && v["base"] == "main")
                );
                (
                    201,
                    json!({ "number": 4, "state": "open", "merged_at": null, "html_url": "https://github.example/pull/4" }),
                )
            }
            ("GET", u)
                if u.starts_with(
                    "https://api.github.com/repos/JumpHash/site/commits/c0ffee/check-runs",
                ) =>
            {
                (
                    200,
                    json!({ "total_count": 2, "check_runs": [
                    { "name": "lint", "status": "completed", "conclusion": "success", "html_url": "h1" },
                    { "name": "tests", "status": "in_progress", "conclusion": null, "html_url": "h2" }
                ] }),
                )
            }
            _ => (404, json!({})),
        });
        let pulls = gh
            .pulls("ghs_1", "JumpHash", "site", "agentplaza/w-12")
            .unwrap();
        assert_eq!(
            pulls,
            [Pull {
                number: 3,
                state: "closed".into(),
                merged: true,
                merge_commit: Some("abc".into()),
                html_url: "https://github.example/pull/3".into()
            }]
        );
        assert!(fake.urls()[0].contains("head=JumpHash%3Aagentplaza%2Fw-12"));
        let open = gh
            .open_pull(
                "ghs_1",
                "JumpHash",
                "site",
                "Lane w-12",
                "agentplaza/w-12",
                "main",
                "body",
            )
            .unwrap();
        assert_eq!((open.number, open.merged), (4, false));
        let runs = gh
            .check_runs("ghs_1", "JumpHash", "site", "c0ffee")
            .unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(
            (
                runs[0].conclusion.as_deref(),
                runs[1].conclusion.as_deref(),
                runs[1].status.as_str()
            ),
            (Some("success"), None, "in_progress")
        );
        assert!(
            fake.seen()
                .iter()
                .all(|(_, _, a, _)| a.as_deref() == Some("Bearer ghs_1"))
        );
    }

    #[test]
    fn a_signing_key_that_is_there_already_is_fine() {
        let (gh, fake) = github(|_, _, b| match b {
            Some(Body::Json(v)) if v["title"] == "new" => (201, json!({ "id": 1 })),
            _ => (
                422,
                json!({ "message": "Validation Failed", "errors": [{ "message": "key is already in use" }] }),
            ),
        });
        gh.add_signing_key(&Auth::user("t"), "new", "ssh-ed25519 AAAA")
            .unwrap();
        gh.add_signing_key(&Auth::user("t"), "again", "ssh-ed25519 AAAA")
            .unwrap();
        assert_eq!(
            fake.urls()[0],
            "POST https://api.github.com/user/ssh_signing_keys"
        );
    }
}
