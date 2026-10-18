//! Forgejo (Gitea, and Codeberg, which runs Forgejo): the REST API under `/api/v1` and the instance's OAuth 2 endpoints.

use std::sync::Arc;

use serde_json::{Value, json};

use super::http::{Body, Http, escape};
use super::model::{Account, Auth, Kind, Namespace, Platform, Repo, Tokens};

/// Items per page of a listing.
const LIMIT: usize = 50;
/// The most pages one listing reads.
const MAX_PAGES: usize = 50;

/// A Forgejo instance.
pub struct Forgejo {
    web: String,
    api: String,
    http: Arc<dyn Http>,
}

impl Forgejo {
    /// `host` is the web origin (`https://codeberg.org`, a self-managed URL); the API base is Kind::api_base.
    pub fn new(host: &str, http: Arc<dyn Http>) -> Self {
        Forgejo {
            web: host.trim_end_matches('/').to_string(),
            api: Kind::Forgejo.api_base(host),
            http,
        }
    }

    /// A GET with the token, the 2xx body read.
    fn get(&self, auth: &Auth, url: &str, what: &str) -> Result<Value, String> {
        self.http
            .call("GET", url, &[auth_header(auth)], None)?
            .ok(what)
    }

    /// Every item of a listing: `LIMIT` a page, read until a short page (at most `MAX_PAGES`).
    fn list(&self, auth: &Auth, path: &str, what: &str) -> Result<Vec<Value>, String> {
        let mut items = Vec::new();
        for page in 1..=MAX_PAGES {
            let url = format!("{}{path}?limit={LIMIT}&page={page}", self.api);
            let batch = self
                .get(auth, &url, what)?
                .as_array()
                .cloned()
                .unwrap_or_default();
            let short = batch.len() < LIMIT;
            items.extend(batch);
            if short {
                break;
            }
        }
        Ok(items)
    }

    /// A form POSTed to the token endpoint, the answer read as tokens (Forgejo names no scopes).
    fn token(&self, fields: &[(&str, &str)], what: &str) -> Result<Tokens, String> {
        let form: Vec<(String, String)> = fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let url = format!("{}/login/oauth/access_token", self.web);
        let body = self
            .http
            .call("POST", &url, &[], Some(Body::Form(form)))?
            .ok(what)?;
        let access = field(&body, &["access_token"]);
        if access.is_empty() {
            return Err(format!("{what}: the platform gave no access token"));
        }
        Ok(Tokens {
            access,
            refresh: Some(field(&body, &["refresh_token"])).filter(|r| !r.is_empty()),
            expires_in: body["expires_in"].as_u64(),
            scopes: Vec::new(),
        })
    }
}

/// The `Authorization` header: the token as a bearer (OAuth and personal tokens alike).
fn auth_header(auth: &Auth) -> (&'static str, String) {
    ("Authorization", format!("Bearer {}", auth.token))
}

/// The first of `keys` that is a non-empty string in `v`, or "".
fn field(v: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|k| v[*k].as_str().filter(|s| !s.is_empty()))
        .unwrap_or_default()
        .to_string()
}

/// Whether `key` is `true` in `v`.
fn flag(v: &Value, key: &str) -> bool {
    v[key].as_bool().unwrap_or(false)
}

/// The primary verified address, else any verified one.
fn verified_email(emails: &Value) -> Option<String> {
    let list = emails.as_array()?;
    let picked = list
        .iter()
        .find(|e| flag(e, "verified") && flag(e, "primary"))
        .or_else(|| list.iter().find(|e| flag(e, "verified")))?;
    let email = field(picked, &["email"]);
    (!email.is_empty()).then_some(email)
}

/// A repository from Forgejo's JSON; the namespace and the name are the parts of its full name.
fn repo_of(r: &Value) -> Repo {
    let mut full_name = field(r, &["full_name"]);
    if full_name.is_empty() {
        full_name = format!("{}/{}", field(&r["owner"], &["login"]), field(r, &["name"]));
    }
    let (namespace, name) = match full_name.rsplit_once('/') {
        Some((namespace, name)) => (namespace.to_string(), name.to_string()),
        None => (String::new(), full_name.clone()),
    };
    Repo {
        full_name,
        namespace,
        name,
        private: flag(r, "private"),
        default_branch: field(r, &["default_branch"]),
        clone_url: field(r, &["clone_url"]),
        web_url: field(r, &["html_url"]),
        can_push: flag(&r["permissions"], "push") || flag(&r["permissions"], "admin"),
    }
}

impl Platform for Forgejo {
    fn authorize_url(
        &self,
        client_id: &str,
        redirect: &str,
        state: &str,
        challenge: &str,
    ) -> String {
        format!(
            "{}/login/oauth/authorize?client_id={}&redirect_uri={}&response_type=code&state={}&code_challenge={}&code_challenge_method=S256",
            self.web,
            escape(client_id),
            escape(redirect),
            escape(state),
            escape(challenge),
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
        self.token(
            &[
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("code", code),
                ("grant_type", "authorization_code"),
                ("redirect_uri", redirect),
                ("code_verifier", verifier),
            ],
            "exchanging the authorization code",
        )
    }

    fn refresh(
        &self,
        client_id: &str,
        client_secret: &str,
        refresh: &str,
        _redirect: &str,
    ) -> Result<Tokens, String> {
        self.token(
            &[
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("refresh_token", refresh),
                ("grant_type", "refresh_token"),
            ],
            "refreshing the token",
        )
    }

    fn account(&self, auth: &Auth) -> Result<Account, String> {
        let user = self.get(auth, &format!("{}/user", self.api), "reading the account")?;
        // A scoped token may not read the emails: the account's own address then.
        let emails = self
            .get(
                auth,
                &format!("{}/user/emails", self.api),
                "reading the account's emails",
            )
            .unwrap_or(Value::Null);
        Ok(Account {
            id: user["id"].as_u64().unwrap_or(0),
            name: field(&user, &["full_name", "login"]),
            login: field(&user, &["login"]),
            email: verified_email(&emails).unwrap_or_else(|| field(&user, &["email"])),
            avatar_url: field(&user, &["avatar_url"]),
        })
    }

    fn namespaces(&self, auth: &Auth, account: &Account) -> Result<Vec<Namespace>, String> {
        let mut out = vec![Namespace {
            name: account.login.clone(),
            kind: "user".to_string(),
            display: if account.name.is_empty() {
                account.login.clone()
            } else {
                account.name.clone()
            },
            id: account.id,
        }];
        for org in self.list(auth, "/user/orgs", "reading the organisations")? {
            let name = field(&org, &["username", "name"]);
            if name.is_empty() {
                continue;
            }
            out.push(Namespace {
                display: field(&org, &["full_name", "username", "name"]),
                name,
                kind: "org".to_string(),
                id: org["id"].as_u64().unwrap_or(0),
            });
        }
        Ok(out)
    }

    fn repos(&self, auth: &Auth, namespace: &Namespace) -> Result<Vec<Repo>, String> {
        if namespace.kind == "user" {
            let mine = self.list(auth, "/user/repos", "reading the repositories")?;
            return Ok(mine
                .iter()
                .filter(|r| field(&r["owner"], &["login"]) == namespace.name)
                .map(repo_of)
                .collect());
        }
        let path = format!("/orgs/{}/repos", escape(&namespace.name));
        Ok(self
            .list(auth, &path, "reading the repositories")?
            .iter()
            .map(repo_of)
            .collect())
    }

    fn repo(&self, auth: &Auth, full_name: &str) -> Result<Repo, String> {
        let Some((owner, name)) = full_name.split_once('/') else {
            return Err(format!(
                "reading the repository: {full_name} is not owner/name"
            ));
        };
        let url = format!("{}/repos/{}/{}", self.api, escape(owner), escape(name));
        Ok(repo_of(&self.get(auth, &url, "reading the repository")?))
    }

    fn add_signing_key(&self, auth: &Auth, title: &str, key: &str) -> Result<(), String> {
        let body = json!({ "title": title, "key": key, "read_only": true });
        let reply = self.http.call(
            "POST",
            &format!("{}/user/keys", self.api),
            &[auth_header(auth)],
            Some(Body::Json(body)),
        )?;
        let message = reply.body["message"]
            .as_str()
            .or_else(|| reply.body.as_str())
            .unwrap_or_default();
        if reply.status == 422
            && (message.contains("has been used") || message.contains("already exists"))
        {
            return Ok(());
        }
        reply.ok("adding the signing key")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platforms::http::Fake;

    /// A Forgejo on codeberg.org answered by `answer`, and the fake it talks through.
    fn forgejo(
        answer: impl Fn(&str, &str, Option<&Body>) -> (u16, Value) + Send + Sync + 'static,
    ) -> (Forgejo, Arc<Fake>) {
        let fake = Arc::new(Fake::new(answer));
        (Forgejo::new("https://codeberg.org", fake.clone()), fake)
    }

    fn auth() -> Auth {
        Auth::user("tok")
    }

    /// The form a token call sends.
    fn form(pairs: &[(&str, &str)]) -> Option<Body> {
        Some(Body::Form(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        ))
    }

    /// A repository as Forgejo answers it, with the fields the client reads.
    fn repo_json(owner: &str, name: &str, permissions: Value) -> Value {
        json!({
            "id": 1,
            "full_name": format!("{owner}/{name}"),
            "name": name,
            "owner": { "id": 51, "login": owner },
            "private": false,
            "default_branch": "main",
            "clone_url": format!("https://codeberg.org/{owner}/{name}.git"),
            "html_url": format!("https://codeberg.org/{owner}/{name}"),
            "permissions": permissions,
        })
    }

    #[test]
    fn the_authorize_page_carries_the_pkce_challenge_and_no_scope() {
        let (forge, _) = forgejo(|_, _, _| (200, Value::Null));
        assert_eq!(
            forge.authorize_url("cid", "https://plaza.example/cb?x", "st 8", "chal"),
            "https://codeberg.org/login/oauth/authorize?client_id=cid&redirect_uri=https%3A%2F%2Fplaza.example%2Fcb%3Fx&response_type=code&state=st%208&code_challenge=chal&code_challenge_method=S256"
        );
        let own = Forgejo::new(
            "https://git.example.com/",
            Arc::new(Fake::new(|_, _, _| (200, Value::Null))),
        );
        assert!(
            own.authorize_url("cid", "https://plaza.example/cb", "s", "c")
                .starts_with("https://git.example.com/login/oauth/authorize?")
        );
    }

    #[test]
    fn exchange_posts_the_form_and_reads_the_tokens() {
        let (forge, fake) = forgejo(|method, url, _| match (method, url) {
            ("POST", "https://codeberg.org/login/oauth/access_token") => (
                200,
                json!({ "access_token": "gto_1", "token_type": "bearer", "expires_in": 3600, "refresh_token": "ref_1" }),
            ),
            _ => (404, json!({ "message": "not found" })),
        });
        let tokens = forge
            .exchange("cid", "sec", "code1", "https://plaza.example/cb", "ver")
            .unwrap();
        assert_eq!(
            tokens,
            Tokens {
                access: "gto_1".into(),
                refresh: Some("ref_1".into()),
                expires_in: Some(3600),
                scopes: vec![],
            }
        );
        assert_eq!(
            fake.urls(),
            vec!["POST https://codeberg.org/login/oauth/access_token"]
        );
        assert_eq!(fake.seen()[0].2, None, "the token endpoint takes no bearer");
        assert_eq!(
            fake.seen()[0].3,
            form(&[
                ("client_id", "cid"),
                ("client_secret", "sec"),
                ("code", "code1"),
                ("grant_type", "authorization_code"),
                ("redirect_uri", "https://plaza.example/cb"),
                ("code_verifier", "ver"),
            ])
        );
    }

    #[test]
    fn refresh_posts_the_refresh_form() {
        let (forge, fake) = forgejo(|_, _, _| {
            (
                200,
                json!({ "access_token": "gto_2", "token_type": "bearer", "expires_in": 3600, "refresh_token": "ref_2" }),
            )
        });
        let tokens = forge
            .refresh("cid", "sec", "ref_1", "https://plaza.example/cb")
            .unwrap();
        assert_eq!(tokens.access, "gto_2");
        assert_eq!(tokens.refresh.as_deref(), Some("ref_2"));
        assert_eq!(
            fake.urls(),
            vec!["POST https://codeberg.org/login/oauth/access_token"]
        );
        assert_eq!(
            fake.seen()[0].3,
            form(&[
                ("client_id", "cid"),
                ("client_secret", "sec"),
                ("refresh_token", "ref_1"),
                ("grant_type", "refresh_token"),
            ])
        );
    }

    #[test]
    fn account_reads_the_user_and_the_primary_verified_email() {
        let (forge, fake) = forgejo(|_, url, _| match url {
            "https://codeberg.org/api/v1/user" => (
                200,
                json!({
                    "id": 7,
                    "login": "irfan",
                    "full_name": "Irfan S",
                    "email": "public@example.com",
                    "avatar_url": "https://codeberg.org/avatars/7",
                }),
            ),
            "https://codeberg.org/api/v1/user/emails" => (
                200,
                json!([
                    { "email": "old@example.com", "verified": true, "primary": false },
                    { "email": "pending@example.com", "verified": false, "primary": true },
                    { "email": "irfan@example.com", "verified": true, "primary": true },
                ]),
            ),
            _ => (404, json!({ "message": "not found" })),
        });
        let account = forge.account(&auth()).unwrap();
        assert_eq!(
            account,
            Account {
                id: 7,
                login: "irfan".into(),
                name: "Irfan S".into(),
                email: "irfan@example.com".into(),
                avatar_url: "https://codeberg.org/avatars/7".into(),
            }
        );
        assert_eq!(
            fake.urls(),
            vec![
                "GET https://codeberg.org/api/v1/user",
                "GET https://codeberg.org/api/v1/user/emails"
            ]
        );
        assert_eq!(fake.seen()[0].2.as_deref(), Some("Bearer tok"));
        assert_eq!(fake.seen()[1].2.as_deref(), Some("Bearer tok"));
    }

    #[test]
    fn account_takes_any_verified_email_then_the_public_one_and_the_login_for_a_name() {
        let account_with = |emails: Value| {
            let (forge, _) = forgejo(move |_, url, _| match url {
                "https://codeberg.org/api/v1/user" => (
                    200,
                    json!({ "id": 9, "login": "ada", "full_name": "", "email": "ada@public.example" }),
                ),
                "https://codeberg.org/api/v1/user/emails" => (200, emails.clone()),
                _ => (404, json!({ "message": "not found" })),
            });
            forge.account(&auth()).unwrap()
        };
        let any = account_with(
            json!([{ "email": "ada@work.example", "verified": true, "primary": false }]),
        );
        assert_eq!(
            (any.name.as_str(), any.email.as_str()),
            ("ada", "ada@work.example")
        );
        let none = account_with(
            json!([{ "email": "ada@work.example", "verified": false, "primary": true }]),
        );
        assert_eq!(none.email, "ada@public.example");
    }

    #[test]
    fn namespaces_are_the_user_then_the_orgs_across_two_pages() {
        let (forge, fake) = forgejo(|_, url, _| {
            if url == "https://codeberg.org/api/v1/user/orgs?limit=50&page=1" {
                let first: Vec<Value> = (1..=50)
                    .map(|i| json!({ "id": i, "username": format!("org{i}"), "full_name": "" }))
                    .collect();
                (200, Value::Array(first))
            } else if url == "https://codeberg.org/api/v1/user/orgs?limit=50&page=2" {
                (
                    200,
                    json!([
                        { "id": 51, "username": "jumphash", "full_name": "Jump Hash" },
                        { "id": 52, "name": "tools", "full_name": "" },
                    ]),
                )
            } else {
                (404, json!({ "message": "not found" }))
            }
        });
        let account = Account {
            id: 7,
            login: "irfan".into(),
            name: "Irfan S".into(),
            ..Default::default()
        };
        let spaces = forge.namespaces(&auth(), &account).unwrap();
        assert_eq!(spaces.len(), 53);
        assert_eq!(
            spaces[0],
            Namespace {
                name: "irfan".into(),
                kind: "user".into(),
                display: "Irfan S".into(),
                id: 7,
            }
        );
        assert_eq!(
            spaces[51],
            Namespace {
                name: "jumphash".into(),
                kind: "org".into(),
                display: "Jump Hash".into(),
                id: 51,
            }
        );
        assert_eq!(spaces[52].name, "tools");
        assert_eq!(spaces[52].display, "tools");
        assert_eq!(
            fake.urls(),
            vec![
                "GET https://codeberg.org/api/v1/user/orgs?limit=50&page=1",
                "GET https://codeberg.org/api/v1/user/orgs?limit=50&page=2",
            ]
        );
    }

    #[test]
    fn an_orgs_repos_read_every_page_and_say_what_may_be_pushed() {
        let (forge, fake) = forgejo(|_, url, _| {
            if url.ends_with("page=1") {
                let first: Vec<Value> = (0..50)
                    .map(|i| {
                        repo_json(
                            "jumphash",
                            &format!("r{i}"),
                            json!({ "admin": false, "push": true, "pull": true }),
                        )
                    })
                    .collect();
                (200, Value::Array(first))
            } else if url.ends_with("page=2") {
                (
                    200,
                    json!([repo_json(
                        "jumphash",
                        "docs",
                        json!({ "admin": false, "push": false, "pull": true })
                    )]),
                )
            } else {
                (404, json!({ "message": "not found" }))
            }
        });
        let org = Namespace {
            name: "jumphash".into(),
            kind: "org".into(),
            display: "Jump Hash".into(),
            id: 51,
        };
        let repos = forge.repos(&auth(), &org).unwrap();
        assert_eq!(repos.len(), 51);
        assert_eq!(repos[0].full_name, "jumphash/r0");
        assert_eq!(repos[0].namespace, "jumphash");
        assert_eq!(repos[0].name, "r0");
        assert!(repos[0].can_push);
        assert_eq!(repos[50].full_name, "jumphash/docs");
        assert!(!repos[50].can_push);
        assert_eq!(
            fake.urls(),
            vec![
                "GET https://codeberg.org/api/v1/orgs/jumphash/repos?limit=50&page=1",
                "GET https://codeberg.org/api/v1/orgs/jumphash/repos?limit=50&page=2",
            ]
        );
    }

    #[test]
    fn a_users_repos_keep_only_the_ones_they_own() {
        let (forge, fake) = forgejo(|_, url, _| match url {
            "https://codeberg.org/api/v1/user/repos?limit=50&page=1" => (
                200,
                json!([
                    repo_json(
                        "irfan",
                        "agentplaza",
                        json!({ "admin": true, "push": false, "pull": true })
                    ),
                    repo_json(
                        "jumphash",
                        "shared",
                        json!({ "admin": false, "push": true, "pull": true })
                    ),
                ]),
            ),
            _ => (404, json!({ "message": "not found" })),
        });
        let user = Namespace {
            name: "irfan".into(),
            kind: "user".into(),
            display: "Irfan S".into(),
            id: 7,
        };
        let repos = forge.repos(&auth(), &user).unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].full_name, "irfan/agentplaza");
        assert!(repos[0].can_push, "an admin may push");
        assert_eq!(
            fake.urls(),
            vec!["GET https://codeberg.org/api/v1/user/repos?limit=50&page=1"]
        );
    }

    #[test]
    fn one_repo_is_read_by_its_path_with_its_fields() {
        let (forge, fake) = forgejo(|_, url, _| match url {
            "https://codeberg.org/api/v1/repos/jumphash/weft" => (
                200,
                json!({
                    "id": 3,
                    "full_name": "jumphash/weft",
                    "name": "weft",
                    "owner": { "login": "jumphash" },
                    "private": true,
                    "default_branch": "trunk",
                    "clone_url": "https://codeberg.org/jumphash/weft.git",
                    "html_url": "https://codeberg.org/jumphash/weft",
                    "permissions": { "admin": true, "push": false, "pull": true },
                }),
            ),
            _ => (404, json!({ "message": "not found" })),
        });
        let repo = forge.repo(&auth(), "jumphash/weft").unwrap();
        assert_eq!(
            repo,
            Repo {
                full_name: "jumphash/weft".into(),
                namespace: "jumphash".into(),
                name: "weft".into(),
                private: true,
                default_branch: "trunk".into(),
                clone_url: "https://codeberg.org/jumphash/weft.git".into(),
                web_url: "https://codeberg.org/jumphash/weft".into(),
                can_push: true,
            }
        );
        assert_eq!(
            fake.urls(),
            vec!["GET https://codeberg.org/api/v1/repos/jumphash/weft"]
        );
        assert_eq!(fake.seen()[0].2.as_deref(), Some("Bearer tok"));
        assert!(
            forge.repo(&auth(), "jumphash").is_err(),
            "a name needs an owner"
        );
    }

    #[test]
    fn a_refused_call_answers_with_the_platforms_message() {
        let (forge, _) = forgejo(|_, _, _| (401, json!({ "message": "token is required" })));
        assert_eq!(
            forge.account(&auth()).unwrap_err(),
            "reading the account: the platform answered 401 token is required"
        );
        let (forge, _) = forgejo(|_, _, _| {
            (
                400,
                json!({ "error": "invalid_grant", "error_description": "authorization code expired" }),
            )
        });
        assert_eq!(
            forge
                .exchange("cid", "sec", "code", "https://plaza.example/cb", "ver")
                .unwrap_err(),
            "exchanging the authorization code: the platform answered 400 authorization code expired"
        );
    }

    #[test]
    fn the_signing_key_is_posted_read_only_and_a_known_key_is_ok() {
        let (forge, fake) = forgejo(|_, _, _| (201, json!({ "id": 3, "title": "agentplaza" })));
        forge
            .add_signing_key(&auth(), "agentplaza", "ssh-ed25519 AAAA")
            .unwrap();
        assert_eq!(
            fake.urls(),
            vec!["POST https://codeberg.org/api/v1/user/keys"]
        );
        assert_eq!(fake.seen()[0].2.as_deref(), Some("Bearer tok"));
        assert_eq!(
            fake.seen()[0].3,
            Some(Body::Json(
                json!({ "title": "agentplaza", "key": "ssh-ed25519 AAAA", "read_only": true })
            ))
        );

        for known in ["Key content has been used", "Key already exists"] {
            let (forge, _) = forgejo(move |_, _, _| (422, json!({ "message": known })));
            assert_eq!(
                forge.add_signing_key(&auth(), "agentplaza", "ssh-ed25519 AAAA"),
                Ok(())
            );
        }
        let (forge, _) = forgejo(|_, _, _| (422, json!({ "message": "Key is invalid" })));
        assert_eq!(
            forge
                .add_signing_key(&auth(), "agentplaza", "ssh-ed25519 AAAA")
                .unwrap_err(),
            "adding the signing key: the platform answered 422 Key is invalid"
        );
    }
}
