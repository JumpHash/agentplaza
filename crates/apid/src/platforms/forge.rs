//! A git platform's releases and checks (PLAN 22.4): the release a tag gets on GitHub, GitLab or Forgejo (its notes,
//! its pre-release flag, its assets), and the verdicts posted on a commit (GitHub's check runs, the commit statuses of
//! GitLab and Forgejo). Every call goes through the platform's [`Http`], so the tests answer with a stand-in.

use std::sync::Arc;

use serde_json::{Value, json};

use super::http::{Body, Http, Reply};
use super::model::Kind;

/// A repository on a platform: its owner and name (GitLab's full path may name groups before the name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub path: String,
}

impl Repo {
    pub fn new(path: &str) -> Repo {
        Repo {
            path: path.trim_matches('/').trim_end_matches(".git").to_string(),
        }
    }

    /// `owner/name` split at its last slash.
    fn owner_name(&self) -> (&str, &str) {
        self.path.rsplit_once('/').unwrap_or(("", &self.path))
    }

    /// GitLab's project id in a path: the full path, URL-encoded.
    fn encoded(&self) -> String {
        self.path.replace('/', "%2F")
    }
}

/// The release a tag has on a platform: its id there, and the page it is shown on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub id: String,
    pub url: Option<String>,
    /// GitHub's upload base (`…/assets`), where the assets go.
    pub upload: Option<String>,
}

/// What a release says on the platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub tag: String,
    pub name: String,
    pub body: String,
    pub prerelease: bool,
}

/// A verdict posted as a check (`pass` is a success, `fail` a failure).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
}

/// One platform's release and check API at a host.
pub struct Forge {
    kind: Kind,
    api: String,
    http: Arc<dyn Http>,
}

impl Forge {
    pub fn new(kind: Kind, host: &str, http: Arc<dyn Http>) -> Forge {
        Forge {
            kind,
            api: kind.api_base(host),
            http,
        }
    }

    fn call(
        &self,
        token: &str,
        method: &str,
        url: &str,
        body: Option<Body>,
    ) -> Result<Reply, String> {
        self.http.call(
            method,
            url,
            &[("Authorization", format!("Bearer {token}"))],
            body,
        )
    }

    /// The release of `tag`, when the platform has one.
    pub fn release_of_tag(
        &self,
        token: &str,
        repo: &Repo,
        tag: &str,
    ) -> Result<Option<Remote>, String> {
        let url = match self.kind {
            Kind::Gitlab => format!("{}/projects/{}/releases/{tag}", self.api, repo.encoded()),
            _ => {
                let (o, n) = repo.owner_name();
                format!("{}/repos/{o}/{n}/releases/tags/{tag}", self.api)
            }
        };
        let reply = self.call(token, "GET", &url, None)?;
        if reply.status == 404 {
            return Ok(None);
        }
        let v = reply.ok("the release of a tag")?;
        Ok(Some(self.remote_of(&v, tag)))
    }

    fn remote_of(&self, v: &Value, tag: &str) -> Remote {
        match self.kind {
            Kind::Gitlab => Remote {
                id: tag.to_string(),
                url: v["_links"]["self"].as_str().map(str::to_string),
                upload: None,
            },
            _ => Remote {
                id: v["id"].to_string(),
                url: v["html_url"].as_str().map(str::to_string),
                upload: v["upload_url"]
                    .as_str()
                    .map(|u| u.split('{').next().unwrap_or(u).to_string()),
            },
        }
    }

    /// Creates the release of a tag.
    pub fn create_release(&self, token: &str, repo: &Repo, spec: &Spec) -> Result<Remote, String> {
        let name = self.name_of(spec);
        let (url, body) = match self.kind {
            Kind::Gitlab => (
                format!("{}/projects/{}/releases", self.api, repo.encoded()),
                json!({ "tag_name": spec.tag, "name": name, "description": spec.body }),
            ),
            _ => {
                let (o, n) = repo.owner_name();
                (
                    format!("{}/repos/{o}/{n}/releases", self.api),
                    json!({ "tag_name": spec.tag, "name": name, "body": spec.body, "draft": false, "prerelease": spec.prerelease }),
                )
            }
        };
        let v = self
            .call(token, "POST", &url, Some(Body::Json(body)))?
            .ok("the release")?;
        Ok(self.remote_of(&v, &spec.tag))
    }

    /// Rewrites the release's name, notes and pre-release flag.
    pub fn update_release(
        &self,
        token: &str,
        repo: &Repo,
        remote: &Remote,
        spec: &Spec,
    ) -> Result<(), String> {
        let name = self.name_of(spec);
        let (method, url, body) = match self.kind {
            Kind::Gitlab => (
                "PUT",
                format!(
                    "{}/projects/{}/releases/{}",
                    self.api,
                    repo.encoded(),
                    remote.id
                ),
                json!({ "name": name, "description": spec.body }),
            ),
            _ => {
                let (o, n) = repo.owner_name();
                (
                    "PATCH",
                    format!("{}/repos/{o}/{n}/releases/{}", self.api, remote.id),
                    json!({ "name": name, "body": spec.body, "prerelease": spec.prerelease }),
                )
            }
        };
        self.call(token, method, &url, Some(Body::Json(body)))?
            .ok("the release's update")?;
        Ok(())
    }

    /// Attaches a file to the release; returns the address it can be downloaded from. GitHub and Forgejo take the
    /// bytes as the asset; GitLab takes them in its generic package registry and links them to the release.
    pub fn attach(
        &self,
        token: &str,
        repo: &Repo,
        remote: &Remote,
        name: &str,
        bytes: &[u8],
    ) -> Result<String, String> {
        let ty = "application/octet-stream".to_string();
        match self.kind {
            Kind::Gitlab => {
                let url = format!(
                    "{}/projects/{}/packages/generic/agentplaza/{}/{name}",
                    self.api,
                    repo.encoded(),
                    remote.id
                );
                self.http
                    .call(
                        "PUT",
                        &url,
                        &[("Authorization", format!("Bearer {token}"))],
                        Some(Body::Bytes(ty, bytes.to_vec())),
                    )?
                    .ok("the package file")?;
                let link = json!({ "name": name, "url": url, "link_type": "other" });
                self.call(
                    token,
                    "POST",
                    &format!(
                        "{}/projects/{}/releases/{}/assets/links",
                        self.api,
                        repo.encoded(),
                        remote.id
                    ),
                    Some(Body::Json(link)),
                )?
                .ok("the release's link")?;
                Ok(url)
            }
            Kind::Github => {
                let upload = remote
                    .upload
                    .clone()
                    .ok_or("GitHub gave no upload address for the release")?;
                let v = self
                    .http
                    .call(
                        "POST",
                        &format!("{upload}?name={name}"),
                        &[("Authorization", format!("Bearer {token}"))],
                        Some(Body::Bytes(ty, bytes.to_vec())),
                    )?
                    .ok("the asset")?;
                Ok(v["browser_download_url"]
                    .as_str()
                    .unwrap_or(name)
                    .to_string())
            }
            Kind::Forgejo => {
                let (o, n) = repo.owner_name();
                let v = self
                    .http
                    .call(
                        "POST",
                        &format!(
                            "{}/repos/{o}/{n}/releases/{}/assets?name={name}",
                            self.api, remote.id
                        ),
                        &[("Authorization", format!("Bearer {token}"))],
                        Some(Body::Bytes(ty, bytes.to_vec())),
                    )?
                    .ok("the asset")?;
                Ok(v["browser_download_url"]
                    .as_str()
                    .unwrap_or(name)
                    .to_string())
            }
        }
    }

    /// Posts a verdict as a check on `sha`: a check run on GitHub, a commit status on GitLab and Forgejo.
    pub fn check(
        &self,
        token: &str,
        repo: &Repo,
        sha: &str,
        context: &str,
        verdict: Verdict,
        summary: &str,
    ) -> Result<(), String> {
        let passed = verdict == Verdict::Pass;
        let (url, body) = match self.kind {
            Kind::Github => {
                let (o, n) = repo.owner_name();
                (
                    format!("{}/repos/{o}/{n}/check-runs", self.api),
                    json!({ "name": context, "head_sha": sha, "status": "completed", "conclusion": if passed { "success" } else { "failure" }, "output": { "title": context, "summary": summary } }),
                )
            }
            Kind::Gitlab => (
                format!("{}/projects/{}/statuses/{sha}", self.api, repo.encoded()),
                json!({ "state": if passed { "success" } else { "failed" }, "name": context, "description": summary }),
            ),
            Kind::Forgejo => {
                let (o, n) = repo.owner_name();
                (
                    format!("{}/repos/{o}/{n}/statuses/{sha}", self.api),
                    json!({ "state": if passed { "success" } else { "failure" }, "context": context, "description": summary }),
                )
            }
        };
        self.call(token, "POST", &url, Some(Body::Json(body)))?
            .ok("the check")?;
        Ok(())
    }

    /// GitLab and Forgejo have no pre-release flag: a candidate says so in its name.
    fn name_of(&self, spec: &Spec) -> String {
        match (self.kind, spec.prerelease) {
            (Kind::Github, _) | (_, false) => spec.name.clone(),
            _ => format!("{} (pre-release)", spec.name),
        }
    }
}
