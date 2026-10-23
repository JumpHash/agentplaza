//! What the platforms have in common: the kinds, an account, a namespace (a user, an organisation or a group), a
//! repository, the tokens an OAuth exchange gives, and [`Platform`], the calls every client answers.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// A git platform.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    apz_store::Column,
)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Github,
    Gitlab,
    /// Forgejo, and Gitea, which answers the same API (Codeberg runs Forgejo).
    Forgejo,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Github, Kind::Gitlab, Kind::Forgejo];

    pub fn parse(raw: &str) -> Option<Kind> {
        match raw.to_ascii_lowercase().as_str() {
            "github" => Some(Kind::Github),
            "gitlab" => Some(Kind::Gitlab),
            "forgejo" | "gitea" | "codeberg" => Some(Kind::Forgejo),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Github => "github",
            Kind::Gitlab => "gitlab",
            Kind::Forgejo => "forgejo",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Github => "GitHub",
            Kind::Gitlab => "GitLab",
            Kind::Forgejo => "Forgejo",
        }
    }

    /// The public instance a host defaults to.
    pub fn default_host(self) -> &'static str {
        match self {
            Kind::Github => "https://github.com",
            Kind::Gitlab => "https://gitlab.com",
            Kind::Forgejo => "https://codeberg.org",
        }
    }

    /// The REST API's base for `host` (a web origin such as `https://gitlab.example.com`).
    pub fn api_base(self, host: &str) -> String {
        let host = host.trim_end_matches('/');
        match self {
            Kind::Github if host == "https://github.com" => "https://api.github.com".to_string(),
            Kind::Github => format!("{host}/api/v3"),
            Kind::Gitlab => format!("{host}/api/v4"),
            Kind::Forgejo => format!("{host}/api/v1"),
        }
    }
}

/// How an authorization was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "lowercase")]
pub enum Via {
    /// A GitHub App installed on a user or an organisation (the repositories chosen on GitHub).
    App,
    /// An OAuth authorization of the owner's account (GitLab, Forgejo, and the owner's own GitHub sign-in).
    Oauth,
    /// A token the owner pasted: a personal, group, project or fine-grained access token.
    Token,
}

/// What an authorization covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// An account and whatever it can reach.
    User,
    /// One organisation or group.
    Org,
    /// One repository.
    Repo,
}

/// The account an authorization is: the owner as the platform knows them (W19: the author of exported commits).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[serde(default)]
pub struct Account {
    pub id: u64,
    pub login: String,
    pub name: String,
    /// A verified email (the primary one when there is one), or the platform's no-reply address.
    pub email: String,
    pub avatar_url: String,
}

/// Where repositories live: a user's own, an organisation's or a group's (a GitLab subgroup is its full path).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Namespace {
    /// The login or full path (`irfan`, `jumphash`, `jumphash/tools`).
    pub name: String,
    /// `user`, `org` or `group`.
    pub kind: String,
    pub display: String,
    pub id: u64,
}

/// A repository on a platform.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Repo {
    /// `namespace/name` (a GitLab path may have more segments).
    pub full_name: String,
    pub namespace: String,
    pub name: String,
    pub private: bool,
    pub default_branch: String,
    /// The HTTPS clone URL.
    pub clone_url: String,
    pub web_url: String,
    /// Whether this authorization may push to it.
    pub can_push: bool,
}

/// What an OAuth exchange or refresh gives.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tokens {
    pub access: String,
    pub refresh: Option<String>,
    /// Seconds the access token lives, when it expires.
    pub expires_in: Option<u64>,
    pub scopes: Vec<String>,
}

/// The token a call is made with, and whether it is a GitHub App installation's (whose repositories are
/// `/installation/repositories`, not the account's).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Auth {
    pub token: String,
    pub installation: bool,
}

impl Auth {
    pub fn user(token: &str) -> Auth {
        Auth {
            token: token.to_string(),
            installation: false,
        }
    }
}

/// The owner's choice of what the plaza may use under one authorization: per namespace, every repository or the
/// ones named. A namespace that is not here is not authorized.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[serde(default)]
pub struct Grants {
    #[table(map)]
    pub namespaces: BTreeMap<String, NamespaceGrant>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[serde(default)]
pub struct NamespaceGrant {
    pub all: bool,
    /// Repository names (without the namespace).
    #[table(set)]
    pub repos: BTreeSet<String>,
}

impl Grants {
    /// Whether `repo` (its full name) is authorized.
    pub fn allows(&self, repo: &Repo) -> bool {
        self.namespaces
            .get(&repo.namespace)
            .is_some_and(|g| g.all || g.repos.contains(&repo.name))
    }
}

/// The calls every platform client answers. `api` is [`Kind::api_base`] of the host and `web` the host itself; the
/// HTTP goes through the client's own [`super::http::Http`].
pub trait Platform {
    /// The page the owner authorizes the plaza on (OAuth with PKCE, S256).
    fn authorize_url(
        &self,
        client_id: &str,
        redirect: &str,
        state: &str,
        challenge: &str,
    ) -> String;

    /// The tokens for an authorization's `code`.
    fn exchange(
        &self,
        client_id: &str,
        client_secret: &str,
        code: &str,
        redirect: &str,
        verifier: &str,
    ) -> Result<Tokens, String>;

    /// New tokens from a refresh token.
    fn refresh(
        &self,
        client_id: &str,
        client_secret: &str,
        refresh: &str,
        redirect: &str,
    ) -> Result<Tokens, String>;

    /// The account the token is, with its verified email.
    fn account(&self, auth: &Auth) -> Result<Account, String>;

    /// The namespaces the token reaches where it may push: the account's own first, then its organisations or
    /// groups.
    fn namespaces(&self, auth: &Auth, account: &Account) -> Result<Vec<Namespace>, String>;

    /// The repositories of one namespace the token reaches (every page).
    fn repos(&self, auth: &Auth, namespace: &Namespace) -> Result<Vec<Repo>, String>;

    /// One repository by its full name.
    fn repo(&self, auth: &Auth, full_name: &str) -> Result<Repo, String>;

    /// Registers `key` (an `ssh-ed25519 ...` line) on the account as a commit signing key, so the platform shows
    /// signed exports as verified. Ok when it is there already.
    fn add_signing_key(&self, auth: &Auth, title: &str, key: &str) -> Result<(), String>;
}
