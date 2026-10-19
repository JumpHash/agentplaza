//! GitLab: the REST API v4 and its OAuth 2 endpoints, on GitLab.com or a self-managed instance.

use std::sync::Arc;

use serde_json::{Value, json};

use super::http::{Body, Http, Reply, escape};
use super::model::{Account, Auth, Kind, Namespace, Platform, Repo, Tokens};

/// Items per page (GitLab's most).
const PER_PAGE: usize = 100;
/// The most pages read of one list.
const MAX_PAGES: usize = 50;
/// GitLab's access level of a developer: the least that may push.
const PUSH_LEVEL: u64 = 30;

/// A GitLab instance: `web` is its origin, `api` the REST API under it.
pub struct GitLab {
    web: String,
    api: String,
    http: Arc<dyn Http>,
}

impl GitLab {
    /// `host` is the web origin (`https://gitlab.com`, a self-managed URL); the API base is Kind::api_base.
    pub fn new(host: &str, http: Arc<dyn Http>) -> Self {
        let web = host.trim_end_matches('/').to_string();
        let api = Kind::Gitlab.api_base(&web);
        GitLab { web, api, http }
    }

    /// One API call with the token as a bearer.
    fn call(
        &self,
        auth: &Auth,
        method: &str,
        url: &str,
        body: Option<Body>,
    ) -> Result<Reply, String> {
        let headers = [("Authorization", format!("Bearer {}", auth.token))];
        self.http.call(method, url, &headers, body)
    }

    fn get(&self, auth: &Auth, url: &str, what: &str) -> Result<Value, String> {
        self.call(auth, "GET", url, None)?.ok(what)
    }

    /// Every item of a list: `path` (its query, without the page) read page by page until a short page.
    fn pages(&self, auth: &Auth, path: &str, what: &str) -> Result<Vec<Value>, String> {
        let mut items = Vec::new();
        for page in 1..=MAX_PAGES {
            let url = format!("{}{path}&page={page}", self.api);
            let batch = match self.get(auth, &url, what)? {
                Value::Array(batch) => batch,
                _ => Vec::new(),
            };
            let short = batch.len() < PER_PAGE;
            items.extend(batch);
            if short {
                break;
            }
        }
        Ok(items)
    }

    /// The token endpoint: the form is posted without a bearer, and the tokens are read from its answer.
    fn token(&self, fields: &[(&str, &str)], what: &str) -> Result<Tokens, String> {
        let url = format!("{}/oauth/token", self.web);
        let body = Body::Form(
            fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        );
        let answer = self.http.call("POST", &url, &[], Some(body))?.ok(what)?;
        let access = text(&answer, "access_token");
        if access.is_empty() {
            return Err(format!("{what}: the platform gave no access token"));
        }
        Ok(Tokens {
            access,
            refresh: non_empty(&answer, "refresh_token"),
            expires_in: answer["expires_in"].as_u64(),
            scopes: text(&answer, "scope")
                .split_whitespace()
                .map(str::to_string)
                .collect(),
        })
    }
}

impl Platform for GitLab {
    fn authorize_url(
        &self,
        client_id: &str,
        redirect: &str,
        state: &str,
        challenge: &str,
    ) -> String {
        format!(
            "{}/oauth/authorize?client_id={}&redirect_uri={}&response_type=code&state={}&scope=api%20read_user&code_challenge={}&code_challenge_method=S256",
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
        self.token(
            &[
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("code", code),
                ("grant_type", "authorization_code"),
                ("redirect_uri", redirect),
                ("code_verifier", verifier),
            ],
            "exchanging the code",
        )
    }

    fn refresh(
        &self,
        client_id: &str,
        client_secret: &str,
        refresh: &str,
        redirect: &str,
    ) -> Result<Tokens, String> {
        self.token(
            &[
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("refresh_token", refresh),
                ("grant_type", "refresh_token"),
                ("redirect_uri", redirect),
            ],
            "refreshing the token",
        )
    }

    fn account(&self, auth: &Auth) -> Result<Account, String> {
        let user = self.get(auth, &format!("{}/user", self.api), "reading the account")?;
        // The address on the account when it is confirmed, else the public one; the commit address wins.
        let verified = if non_empty(&user, "confirmed_at").is_some() {
            non_empty(&user, "email")
        } else {
            non_empty(&user, "public_email")
        };
        Ok(Account {
            id: user["id"].as_u64().unwrap_or(0),
            login: text(&user, "username"),
            name: text(&user, "name"),
            email: non_empty(&user, "commit_email")
                .or(verified)
                .unwrap_or_default(),
            avatar_url: text(&user, "avatar_url"),
        })
    }

    fn namespaces(&self, auth: &Auth, account: &Account) -> Result<Vec<Namespace>, String> {
        let mut found = vec![Namespace {
            name: account.login.clone(),
            kind: "user".into(),
            display: account.name.clone(),
            id: account.id,
        }];
        let path = format!("/groups?min_access_level={PUSH_LEVEL}&per_page={PER_PAGE}");
        let groups = self.pages(auth, &path, "listing the groups")?;
        found.extend(groups.iter().map(|g| Namespace {
            name: text(g, "full_path"),
            kind: "group".into(),
            display: text(g, "full_name"),
            id: g["id"].as_u64().unwrap_or(0),
        }));
        Ok(found)
    }

    fn repos(&self, auth: &Auth, namespace: &Namespace) -> Result<Vec<Repo>, String> {
        let what = "listing the repositories";
        if namespace.kind == "user" {
            let path = format!("/users/{}/projects?per_page={PER_PAGE}", namespace.id);
            let projects = self.pages(auth, &path, what)?;
            Ok(projects
                .iter()
                .map(|p| {
                    let owned = p["namespace"]["full_path"]
                        .as_str()
                        .is_some_and(|owner| owner.eq_ignore_ascii_case(&namespace.name));
                    repo_of(p, owned || pushes(p))
                })
                .collect())
        } else {
            // The list is filtered to developers and above, so every project in it may be pushed to.
            let path = format!(
                "/groups/{}/projects?include_subgroups=true&min_access_level={PUSH_LEVEL}&per_page={PER_PAGE}",
                namespace.id
            );
            Ok(self
                .pages(auth, &path, what)?
                .iter()
                .map(|p| repo_of(p, true))
                .collect())
        }
    }

    fn repo(&self, auth: &Auth, full_name: &str) -> Result<Repo, String> {
        let url = format!("{}/projects/{}", self.api, escape(full_name));
        let project = self.get(auth, &url, "reading the repository")?;
        Ok(repo_of(&project, pushes(&project)))
    }

    fn add_signing_key(&self, auth: &Auth, title: &str, key: &str) -> Result<(), String> {
        let url = format!("{}/user/keys", self.api);
        let body = Body::Json(json!({ "title": title, "key": key, "usage_type": "signing" }));
        let reply = self.call(auth, "POST", &url, Some(body))?;
        // A key the account has already is refused with a message naming its fingerprint (a map, not a string).
        if reply.status == 400
            && reply.body["message"]
                .to_string()
                .contains("has already been taken")
        {
            return Ok(());
        }
        reply.ok("adding the signing key").map(|_| ())
    }
}

/// A project as a [`Repo`]; `can_push` is for the caller to say (a filtered list, an owned project, or the
/// project's permissions).
fn repo_of(project: &Value, can_push: bool) -> Repo {
    let full_name = text(project, "path_with_namespace");
    let (namespace, name) = match full_name.rsplit_once('/') {
        Some((namespace, name)) => (namespace.to_string(), name.to_string()),
        None => (String::new(), full_name.clone()),
    };
    Repo {
        full_name,
        namespace,
        name,
        private: text(project, "visibility") != "public",
        default_branch: text(project, "default_branch"),
        clone_url: text(project, "http_url_to_repo"),
        web_url: text(project, "web_url"),
        can_push,
    }
}

/// Whether the caller's own access to a project (its project or its group's) is developer or above.
fn pushes(project: &Value) -> bool {
    let level = |key: &str| {
        project["permissions"][key]["access_level"]
            .as_u64()
            .unwrap_or(0)
    };
    level("project_access") >= PUSH_LEVEL || level("group_access") >= PUSH_LEVEL
}

/// A string field, empty when it is missing.
fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_string()
}

/// A string field, when it is there and not empty.
fn non_empty(v: &Value, key: &str) -> Option<String> {
    v[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platforms::http::Fake;

    const HOST: &str = "https://gitlab.example.com";

    /// A client over a [`Fake`] that answers with `answer`, and the fake to read the calls from.
    fn client(
        answer: impl Fn(&str, &str, Option<&Body>) -> (u16, Value) + Send + Sync + 'static,
    ) -> (GitLab, Arc<Fake>) {
        let fake = Arc::new(Fake::new(answer));
        (GitLab::new(HOST, fake.clone()), fake)
    }

    fn form(pairs: &[(&str, &str)]) -> Body {
        Body::Form(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    fn account() -> Account {
        Account {
            id: 42,
            login: "ada".into(),
            name: "Ada Lovelace".into(),
            email: "ada@example.com".into(),
            avatar_url: String::new(),
        }
    }

    /// A project as a list answers it: `namespace/name`, its visibility, and the caller's access level.
    fn project(namespace: &str, name: &str, visibility: &str, level: u64) -> Value {
        json!({
            "id": 1,
            "path_with_namespace": format!("{namespace}/{name}"),
            "path": name,
            "namespace": { "full_path": namespace },
            "visibility": visibility,
            "default_branch": "main",
            "http_url_to_repo": format!("{HOST}/{namespace}/{name}.git"),
            "web_url": format!("{HOST}/{namespace}/{name}"),
            "permissions": {
                "project_access": { "access_level": level },
                "group_access": null
            }
        })
    }

    #[test]
    fn the_authorize_page_asks_for_the_api_and_read_user_with_pkce() {
        let (gitlab, _) = client(|_, _, _| (200, Value::Null));
        assert_eq!(gitlab.api, "https://gitlab.example.com/api/v4");
        assert_eq!(
            gitlab.authorize_url("app id", "http://localhost:8080/cb?x=1", "s/1", "abc+/="),
            "https://gitlab.example.com/oauth/authorize?client_id=app%20id&redirect_uri=http%3A%2F%2Flocalhost%3A8080%2Fcb%3Fx%3D1&response_type=code&state=s%2F1&scope=api%20read_user&code_challenge=abc%2B%2F%3D&code_challenge_method=S256"
        );
        let trailing = GitLab::new(
            "https://gitlab.example.com/",
            Arc::new(Fake::new(|_, _, _| (200, Value::Null))),
        );
        assert_eq!(trailing.api, "https://gitlab.example.com/api/v4");
    }

    #[test]
    fn the_code_is_exchanged_by_a_form_post_and_the_tokens_are_read() {
        let (gitlab, fake) = client(|_, _, _| {
            (
                200,
                json!({
                    "access_token": "at", "token_type": "bearer", "expires_in": 7200,
                    "refresh_token": "rt", "scope": "api read_user", "created_at": 1700000000
                }),
            )
        });
        let tokens = gitlab
            .exchange("app", "secret", "code", "http://localhost/cb", "verifier")
            .unwrap();
        assert_eq!(
            tokens,
            Tokens {
                access: "at".into(),
                refresh: Some("rt".into()),
                expires_in: Some(7200),
                scopes: vec!["api".into(), "read_user".into()],
            }
        );
        assert_eq!(
            fake.urls(),
            vec!["POST https://gitlab.example.com/oauth/token"]
        );
        let seen = fake.seen();
        assert_eq!(seen[0].2, None, "the token endpoint takes no bearer");
        assert_eq!(
            seen[0].3,
            Some(form(&[
                ("client_id", "app"),
                ("client_secret", "secret"),
                ("code", "code"),
                ("grant_type", "authorization_code"),
                ("redirect_uri", "http://localhost/cb"),
                ("code_verifier", "verifier"),
            ]))
        );
    }

    #[test]
    fn a_refresh_posts_the_refresh_grant_and_reads_the_rotated_tokens() {
        let (gitlab, fake) = client(|_, _, _| {
            (
                200,
                json!({ "access_token": "at2", "refresh_token": "rt2", "expires_in": 7200, "scope": "api read_user" }),
            )
        });
        let tokens = gitlab
            .refresh("app", "secret", "rt", "http://localhost/cb")
            .unwrap();
        assert_eq!(tokens.access, "at2");
        assert_eq!(tokens.refresh.as_deref(), Some("rt2"));
        assert_eq!(
            fake.urls(),
            vec!["POST https://gitlab.example.com/oauth/token"]
        );
        assert_eq!(
            fake.seen()[0].3,
            Some(form(&[
                ("client_id", "app"),
                ("client_secret", "secret"),
                ("refresh_token", "rt"),
                ("grant_type", "refresh_token"),
                ("redirect_uri", "http://localhost/cb"),
            ]))
        );
    }

    #[test]
    fn a_refused_code_gives_the_platforms_words() {
        let (gitlab, _) = client(|_, _, _| {
            (
                400,
                json!({ "error": "invalid_grant", "error_description": "The provided authorization grant is invalid." }),
            )
        });
        assert_eq!(
            gitlab
                .exchange("app", "secret", "code", "http://localhost/cb", "verifier")
                .unwrap_err(),
            "exchanging the code: the platform answered 400 The provided authorization grant is invalid."
        );
    }

    #[test]
    fn the_account_is_read_with_the_email_it_is_known_by() {
        let profile = |confirmed: Value, email: &str, public: &str, commit: &str| {
            json!({
                "id": 42, "username": "ada", "name": "Ada Lovelace", "email": email,
                "public_email": public, "commit_email": commit, "confirmed_at": confirmed,
                "avatar_url": "https://gitlab.example.com/uploads/avatar.png"
            })
        };
        let cases = [
            // Confirmed: the account's email.
            (
                profile(
                    json!("2024-01-02T03:04:05Z"),
                    "ada@example.com",
                    "public@example.com",
                    "",
                ),
                "ada@example.com",
            ),
            // Not confirmed: the public email.
            (
                profile(Value::Null, "ada@example.com", "public@example.com", ""),
                "public@example.com",
            ),
            // A commit email wins over both.
            (
                profile(
                    json!("2024-01-02T03:04:05Z"),
                    "ada@example.com",
                    "",
                    "42-ada@users.noreply.gitlab.com",
                ),
                "42-ada@users.noreply.gitlab.com",
            ),
        ];
        for (user, want) in cases {
            let (gitlab, fake) = client(move |_, _, _| (200, user.clone()));
            let read = gitlab.account(&Auth::user("tok")).unwrap();
            assert_eq!(read.email, want);
            assert_eq!(read.id, 42);
            assert_eq!(read.login, "ada");
            assert_eq!(read.name, "Ada Lovelace");
            assert_eq!(
                read.avatar_url,
                "https://gitlab.example.com/uploads/avatar.png"
            );
            assert_eq!(
                fake.urls(),
                vec!["GET https://gitlab.example.com/api/v4/user"]
            );
            assert_eq!(fake.seen()[0].2.as_deref(), Some("Bearer tok"));
        }
    }

    #[test]
    fn a_refused_account_read_gives_the_platforms_message() {
        let (gitlab, _) = client(|_, _, _| (401, json!({ "message": "401 Unauthorized" })));
        assert_eq!(
            gitlab.account(&Auth::user("tok")).unwrap_err(),
            "reading the account: the platform answered 401 401 Unauthorized"
        );
    }

    #[test]
    fn the_namespaces_are_the_account_then_its_groups_read_over_two_pages() {
        let (gitlab, fake) = client(|_, url, _| {
            if url.ends_with("page=1") {
                (
                    200,
                    Value::Array(
                        (0..100)
                            .map(|i| {
                                json!({
                                    "id": i + 10, "full_path": format!("team-{i}"),
                                    "name": format!("Team {i}"), "full_name": format!("Team {i} Group")
                                })
                            })
                            .collect(),
                    ),
                )
            } else {
                (
                    200,
                    json!([{ "id": 900, "full_path": "jumphash/tools", "name": "tools", "full_name": "Jumphash / Tools" }]),
                )
            }
        });
        let found = gitlab.namespaces(&Auth::user("tok"), &account()).unwrap();
        assert_eq!(found.len(), 102);
        assert_eq!(
            found[0],
            Namespace {
                name: "ada".into(),
                kind: "user".into(),
                display: "Ada Lovelace".into(),
                id: 42
            }
        );
        assert_eq!(
            found[101],
            Namespace {
                name: "jumphash/tools".into(),
                kind: "group".into(),
                display: "Jumphash / Tools".into(),
                id: 900,
            }
        );
        assert_eq!(
            fake.urls(),
            vec![
                "GET https://gitlab.example.com/api/v4/groups?min_access_level=30&per_page=100&page=1",
                "GET https://gitlab.example.com/api/v4/groups?min_access_level=30&per_page=100&page=2",
            ]
        );
        assert_eq!(fake.seen()[0].2.as_deref(), Some("Bearer tok"));
    }

    #[test]
    fn a_users_repos_are_read_over_every_page_until_a_short_one() {
        let (gitlab, fake) = client(|_, url, _| {
            if url.ends_with("page=1") {
                (
                    200,
                    Value::Array(
                        (0..100)
                            .map(|i| project("ada", &format!("p{i}"), "private", 40))
                            .collect(),
                    ),
                )
            } else {
                (
                    200,
                    json!([
                        project("ada", "last", "public", 40),
                        project("ada", "later", "public", 40)
                    ]),
                )
            }
        });
        let user = Namespace {
            name: "ada".into(),
            kind: "user".into(),
            display: "Ada Lovelace".into(),
            id: 42,
        };
        let repos = gitlab.repos(&Auth::user("tok"), &user).unwrap();
        assert_eq!(repos.len(), 102);
        assert_eq!(repos[101].full_name, "ada/later");
        assert_eq!(
            fake.urls(),
            vec![
                "GET https://gitlab.example.com/api/v4/users/42/projects?per_page=100&page=1",
                "GET https://gitlab.example.com/api/v4/users/42/projects?per_page=100&page=2",
            ]
        );
        assert_eq!(fake.seen()[0].2.as_deref(), Some("Bearer tok"));
    }

    #[test]
    fn a_users_repo_reads_its_fields_and_can_push_when_owned_or_permitted() {
        let (gitlab, _) = client(|_, _, _| {
            (
                200,
                json!([
                    project("ada", "notes", "private", 10),
                    project("jumphash", "site", "public", 40),
                    project("jumphash", "docs", "internal", 20),
                ]),
            )
        });
        let user = Namespace {
            name: "ada".into(),
            kind: "user".into(),
            display: "Ada Lovelace".into(),
            id: 42,
        };
        let repos = gitlab.repos(&Auth::user("tok"), &user).unwrap();
        assert_eq!(
            repos[0],
            Repo {
                full_name: "ada/notes".into(),
                namespace: "ada".into(),
                name: "notes".into(),
                private: true,
                default_branch: "main".into(),
                clone_url: "https://gitlab.example.com/ada/notes.git".into(),
                web_url: "https://gitlab.example.com/ada/notes".into(),
                can_push: true,
            }
        );
        assert!(!repos[2].can_push, "a reporter cannot push");
        assert!(repos[2].private, "internal is not public");
        assert!(repos[1].can_push, "a maintainer can push");
        assert!(!repos[1].private);
    }

    #[test]
    fn a_groups_repos_include_subgroups_and_all_may_be_pushed_to() {
        let (gitlab, fake) =
            client(|_, _, _| (200, json!([project("jumphash/tools", "cli", "public", 10)])));
        let group = Namespace {
            name: "jumphash/tools".into(),
            kind: "group".into(),
            display: "Jumphash / Tools".into(),
            id: 900,
        };
        let repos = gitlab.repos(&Auth::user("tok"), &group).unwrap();
        assert_eq!(
            fake.urls(),
            vec![
                "GET https://gitlab.example.com/api/v4/groups/900/projects?include_subgroups=true&min_access_level=30&per_page=100&page=1"
            ]
        );
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].namespace, "jumphash/tools");
        assert_eq!(repos[0].name, "cli");
        assert!(!repos[0].private);
        assert!(
            repos[0].can_push,
            "the list is filtered to developers and above"
        );
    }

    #[test]
    fn a_repo_is_read_by_its_whole_escaped_path() {
        let (gitlab, fake) =
            client(|_, _, _| (200, project("jumphash/tools", "cli", "private", 30)));
        let repo = gitlab
            .repo(&Auth::user("tok"), "jumphash/tools/cli")
            .unwrap();
        assert_eq!(
            fake.urls(),
            vec!["GET https://gitlab.example.com/api/v4/projects/jumphash%2Ftools%2Fcli"]
        );
        assert_eq!(fake.seen()[0].2.as_deref(), Some("Bearer tok"));
        assert_eq!(repo.full_name, "jumphash/tools/cli");
        assert_eq!(repo.namespace, "jumphash/tools");
        assert_eq!(repo.name, "cli");
        assert!(repo.private);
        assert!(repo.can_push);
    }

    #[test]
    fn a_refused_repo_read_gives_the_platforms_message() {
        let (gitlab, _) = client(|_, _, _| (404, json!({ "message": "404 Project Not Found" })));
        assert_eq!(
            gitlab.repo(&Auth::user("tok"), "ada/missing").unwrap_err(),
            "reading the repository: the platform answered 404 404 Project Not Found"
        );
    }

    #[test]
    fn a_signing_key_is_added_as_a_signing_key() {
        let (gitlab, fake) = client(|_, _, _| {
            (
                201,
                json!({ "id": 5, "title": "agentplaza", "usage_type": "signing" }),
            )
        });
        gitlab
            .add_signing_key(&Auth::user("tok"), "agentplaza", "ssh-ed25519 AAAA")
            .unwrap();
        assert_eq!(
            fake.urls(),
            vec!["POST https://gitlab.example.com/api/v4/user/keys"]
        );
        let seen = fake.seen();
        assert_eq!(seen[0].2.as_deref(), Some("Bearer tok"));
        assert_eq!(
            seen[0].3,
            Some(Body::Json(
                json!({ "title": "agentplaza", "key": "ssh-ed25519 AAAA", "usage_type": "signing" })
            ))
        );
    }

    #[test]
    fn a_key_the_account_has_already_is_ok_and_other_refusals_are_not() {
        let (gitlab, _) = client(|_, _, _| {
            (
                400,
                json!({ "message": { "fingerprint": ["has already been taken"] } }),
            )
        });
        assert_eq!(
            gitlab.add_signing_key(&Auth::user("tok"), "agentplaza", "ssh-ed25519 AAAA"),
            Ok(())
        );

        let (gitlab, _) = client(|_, _, _| (400, json!({ "message": { "key": ["is invalid"] } })));
        assert!(
            gitlab
                .add_signing_key(&Auth::user("tok"), "agentplaza", "not a key")
                .is_err()
        );
    }
}
