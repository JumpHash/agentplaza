//! Git platforms (GitHub, GitLab, Forgejo): the owner authorizes the plaza on them, picks what it may use, and
//! connects repositories to projects, whose Weft repos then export to them.
//!
//! - **Authorizations** come three ways ([`Via`]): the plaza's own GitHub App, created from a manifest and installed
//!   on the owner's user or organisations (GitHub's own repository picker: every repository, or the ones chosen); an
//!   OAuth app the owner registers on a GitLab or Forgejo host (authorization code with PKCE); or a token the owner
//!   pastes (a personal token, a GitLab group or project token, a fine-grained GitHub token). That is the user,
//!   organisation and repository level ([`Level`]).
//! - **Grants.** Per authorization the owner picks the namespaces and repositories the plaza may use: every
//!   repository of a namespace, or the ones named. An installation starts with what GitHub was given; an OAuth or a
//!   token authorization starts with nothing.
//! - **Connect.** A granted repository joins a project: the project's Weft repo exports to it (W18: it exists
//!   already), its history is imported once when the Weft repo has none, and exported commits are by the owner as
//!   the platform knows them (W19). The hub's signing key can be registered on that account at the same time.
//! - **Secrets** (client secrets, the app's private key, tokens) are vault entries bound to `hub:git-platforms`; the
//!   record (plaza.db, table `platforms` and its child tables) holds none. Weft gets a token when it pushes or fetches ([`credential`]):
//!   minted for an installation, refreshed for an OAuth sign-in, as pasted for a token.
//! - **The browser's way back.** The platforms send the owner's browser to `/v1/platforms/callback`, a public route
//!   that acts only on a `state` an owner's call created (once, within 15 minutes). Starting a flow gives a `go` URL
//!   the console opens: a form that posts the manifest to GitHub, or a redirect to the platform's own page.
//!
//! Routes (owner-only unless public):
//!
//! | route | does |
//! |---|---|
//! | `GET /v1/platforms` | the kinds, the registered apps, the authorizations, the callback URL |
//! | `POST /v1/platforms/github/app {host?, org?, name?}` | creates the plaza's GitHub App from a manifest → `{state, go}` |
//! | `POST /v1/platforms/github/install {registration}` | installs it on a user or an organisation → `{state, go}` |
//! | `POST /v1/platforms/registrations {kind, host?, client_id, client_secret}` | an OAuth app the owner registered |
//! | `DELETE /v1/platforms/registrations/{id}` | forgets it and its secret |
//! | `POST /v1/platforms/authorize {registration}` | the owner's OAuth sign-in → `{state, go}` |
//! | `POST /v1/platforms/tokens {kind, host?, token, level?, target?}` | an authorization by token |
//! | `GET /v1/platforms/flows/{state}` | a flow's outcome, for the consoles to wait on |
//! | `GET /v1/platforms/flows/{state}/go` (public) | the page the owner's browser opens |
//! | `GET /v1/platforms/callback` (public) | where the platform sends the browser back |
//! | `POST /v1/platforms/{id}/sync` | reads the account (and an app's installations) again |
//! | `DELETE /v1/platforms/{id}` | forgets an authorization and its tokens |
//! | `GET /v1/platforms/{id}/namespaces` | what it reaches, each with its grant |
//! | `GET /v1/platforms/{id}/repos?namespace=` | one namespace's repositories, granted or not, and where they are connected |
//! | `PUT /v1/platforms/{id}/grants {namespaces}` | the owner's choice |
//! | `POST /v1/platforms/{id}/connect {repo, project, sign?, export?}` | a granted repository joins a project |
//! | `POST /v1/platforms/{id}/disconnect {weft_repo, connection}` | the project's repo stops exporting to it |

pub mod forgejo;
pub mod github;
pub mod gitlab;
pub mod http;
pub mod model;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_store::{RowSet, StoreError, TableDef};
use base64::Engine;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use self::github::GitHub;
use self::http::{Http, Ureq};
pub use self::model::{
    Account, Auth, Grants, Kind, Level, Namespace, NamespaceGrant, Platform, Repo, Via,
};
use crate::state::App;

/// The vault binding every platform secret is under.
const BINDING: &str = "hub:git-platforms";
/// How long a started flow waits for the browser to come back.
const FLOW_TTL_MS: u64 = 15 * 60 * 1000;
/// The public route the platforms send the browser back to.
const CALLBACK: &str = "/v1/platforms/callback";
/// A token is renewed this long before it expires.
const EARLY_MS: u64 = 60 * 1000;

/// An app the plaza is registered as on a host: the plaza's GitHub App, or an OAuth app the owner registered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct Registration {
    pub id: String,
    pub kind: Kind,
    pub host: String,
    pub client_id: String,
    /// Set for the plaza's GitHub App.
    #[serde(default)]
    #[table(child)]
    pub app: Option<GitHubApp>,
    pub created_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct GitHubApp {
    pub id: u64,
    pub slug: String,
    pub name: String,
    pub owner: String,
    pub html_url: String,
}

/// One authorization of the plaza on a platform.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct Integration {
    pub id: String,
    pub kind: Kind,
    pub host: String,
    pub via: Via,
    pub level: Level,
    /// The app it came through (an installation's GitHub App, an OAuth app).
    #[serde(default)]
    pub registration: Option<String>,
    #[serde(default)]
    pub installation: Option<u64>,
    /// An installation's choice on GitHub: `all` or `selected`.
    #[serde(default)]
    pub repository_selection: Option<String>,
    /// Where the owner changes it on the platform.
    #[serde(default)]
    pub manage_url: Option<String>,
    /// The organisation, group or repository an organisation- or repository-level authorization is for.
    #[serde(default)]
    pub target: Option<String>,
    /// The account it is: the owner, a group's bot, or (for an installation) the user or organisation it is on.
    #[serde(default)]
    #[table(flatten)]
    pub account: Account,
    #[serde(default)]
    #[table(list)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub expires_ms: Option<u64>,
    #[serde(default)]
    #[table(child)]
    pub grants: Grants,
    pub created_ms: u64,
    #[serde(default)]
    pub checked_ms: u64,
    #[serde(default)]
    pub error: Option<String>,
}

/// The platforms' record: the apps and the authorizations, and the counter that names new ones. Its table is
/// `platforms`; the lists are its children `platforms_registrations` and `platforms_integrations`.
#[derive(Debug, Clone, Default, apz_store::Table)]
#[table(name = "platforms")]
struct Saved {
    #[table(child)]
    registrations: Vec<Registration>,
    #[table(child)]
    integrations: Vec<Integration>,
    next: u64,
}

/// The platforms' table definition: the record's root table (component `platforms`).
pub fn def() -> &'static TableDef {
    apz_store::root_def!(Saved)
}

/// What a started flow waits for.
#[derive(Debug, Clone)]
enum Step {
    /// The owner creates the GitHub App from its manifest on `host` (for `org`, else their own account).
    Manifest {
        host: String,
        org: Option<String>,
        name: String,
    },
    /// The owner installs `registration` (the GitHub App) on a user or an organisation.
    Install { registration: String },
    /// The owner signs in through `registration` (OAuth with PKCE).
    Oauth {
        registration: String,
        verifier: String,
    },
}

struct Flow {
    step: Option<Step>,
    created_ms: u64,
    /// Set when the browser came back: what it did, or why it failed.
    outcome: Option<Result<Value, String>>,
}

/// The platforms' record, the flows under way and the installation tokens minted so far.
pub struct Store {
    saved: Mutex<Saved>,
    flows: Mutex<HashMap<String, Flow>>,
    minted: Mutex<HashMap<String, (String, u64)>>,
    http: Arc<dyn Http>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn now_ms() -> u64 {
    apz_plaza::now_ms()
}

impl Store {
    /// An empty record, with the platforms called over HTTP; [`Store::load`] fills it at start.
    pub fn new() -> Store {
        Store::with_http(Arc::new(Ureq))
    }

    /// An empty record, with the platforms called through `http` (the tests' fake).
    pub fn with_http(http: Arc<dyn Http>) -> Store {
        Store {
            saved: Mutex::new(Saved::default()),
            flows: Mutex::new(HashMap::new()),
            minted: Mutex::new(HashMap::new()),
            http,
        }
    }

    /// Replaces the record with what plaza.db holds (at start). Nothing stored means nothing was authorized yet.
    pub fn load(&self, conn: &apz_store::rusqlite::Connection) -> Result<(), StoreError> {
        let stored: Option<Saved> = apz_store::table::load_one(conn, def())?;
        *lock(&self.saved) = stored.unwrap_or_default();
        Ok(())
    }

    /// The record as rows of its tables, as it is now (the saver writes what changed).
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        RowSet::one(def(), &*lock(&self.saved))
    }

    /// Changes the record in memory; the saver writes it. Where the owner's answer depends on the write (a vault
    /// entry and its record), [`persist`] writes it first.
    fn update<R>(&self, change: impl FnOnce(&mut Saved) -> R) -> Result<R, String> {
        let mut saved = lock(&self.saved);
        Ok(change(&mut saved))
    }

    pub fn registrations(&self) -> Vec<Registration> {
        lock(&self.saved).registrations.clone()
    }

    pub fn integrations(&self) -> Vec<Integration> {
        lock(&self.saved).integrations.clone()
    }

    pub fn registration(&self, id: &str) -> Option<Registration> {
        lock(&self.saved)
            .registrations
            .iter()
            .find(|r| r.id == id)
            .cloned()
    }

    pub fn integration(&self, id: &str) -> Option<Integration> {
        lock(&self.saved)
            .integrations
            .iter()
            .find(|i| i.id == id)
            .cloned()
    }

    /// Adds `integ`, or replaces the one `same` finds (its id, grants and creation kept).
    fn upsert(
        &self,
        mut integ: Integration,
        same: impl Fn(&Integration) -> bool,
    ) -> Result<Integration, String> {
        self.update(|saved| {
            if let Some(old) = saved.integrations.iter_mut().find(|i| same(i)) {
                integ.id = old.id.clone();
                integ.grants = old.grants.clone();
                integ.created_ms = old.created_ms;
                *old = integ.clone();
            } else {
                saved.next += 1;
                integ.id = format!("{}-{}", integ.kind.as_str(), saved.next);
                saved.integrations.push(integ.clone());
            }
            integ
        })
    }

    fn set(&self, integ: &Integration) -> Result<(), String> {
        self.update(|saved| {
            if let Some(old) = saved.integrations.iter_mut().find(|i| i.id == integ.id) {
                *old = integ.clone();
            }
        })
    }

    pub(crate) fn platform(&self, kind: Kind, host: &str) -> Box<dyn Platform> {
        match kind {
            Kind::Github => Box::new(GitHub::new(host, self.http.clone())),
            Kind::Gitlab => Box::new(gitlab::GitLab::new(host, self.http.clone())),
            Kind::Forgejo => Box::new(forgejo::Forgejo::new(host, self.http.clone())),
        }
    }

    pub(crate) fn github(&self, host: &str) -> GitHub {
        GitHub::new(host, self.http.clone())
    }

    /// Starts a flow: its state, random and single-use.
    fn start(&self, step: Step) -> Result<String, String> {
        let state = apz_os::random::hex(16).map_err(|e| e.to_string())?;
        let mut flows = lock(&self.flows);
        let now = now_ms();
        flows.retain(|_, f| now.saturating_sub(f.created_ms) < FLOW_TTL_MS * 4);
        flows.insert(
            state.clone(),
            Flow {
                step: Some(step),
                created_ms: now,
                outcome: None,
            },
        );
        Ok(state)
    }

    /// The step a live flow waits for, without taking it.
    fn peek(&self, state: &str) -> Option<Step> {
        let flows = lock(&self.flows);
        let f = flows.get(state)?;
        (now_ms().saturating_sub(f.created_ms) < FLOW_TTL_MS)
            .then(|| f.step.clone())
            .flatten()
    }

    /// Takes the step of a live flow: the browser came back, once.
    fn claim(&self, state: &str) -> Option<Step> {
        let mut flows = lock(&self.flows);
        let f = flows.get_mut(state)?;
        if now_ms().saturating_sub(f.created_ms) >= FLOW_TTL_MS {
            return None;
        }
        f.step.take()
    }

    fn finish(&self, state: &str, outcome: Result<Value, String>) {
        if let Some(f) = lock(&self.flows).get_mut(state) {
            f.outcome = Some(outcome);
        }
    }
}

/// Writes the record now, for an answer that a vault entry and its record belong to: the owner is told it is done
/// only once both are saved.
fn persist(app: &App) -> Result<(), String> {
    if crate::store::saver::save_now(app, "platforms") {
        Ok(())
    } else {
        Err("the platforms' record could not be saved; try again".into())
    }
}

// ── secrets ──────────────────────────────────────────────────────────────────────────────────────────────────

fn secret_name(id: &str, what: &str) -> String {
    format!(
        "PLATFORM_{}_{what}",
        id.to_ascii_uppercase()
            .replace(|c: char| !c.is_ascii_alphanumeric(), "_")
    )
}

fn put_secret(app: &App, id: &str, what: &str, label: &str, value: &str) -> Result<(), String> {
    app.vault
        .put_plaza(
            &secret_name(id, what),
            BINDING,
            label,
            &SecretString::from(value.to_string()),
            "owner",
            now_ms(),
        )
        .map(|_| ())
        .map_err(|e| format!("the vault: {}", e.message()))
}

fn secret(app: &App, id: &str, what: &str) -> Option<String> {
    app.vault.value_for(
        &secret_name(id, what),
        BINDING,
        &crate::vault::OnBehalf::Hub,
    )
}

fn forget_secret(app: &App, id: &str, what: &str) {
    for e in app
        .vault
        .entries_named(&secret_name(id, what))
        .unwrap_or_default()
    {
        let _ = app.vault.delete(&e.id, "owner", now_ms());
    }
}

// ── tokens ───────────────────────────────────────────────────────────────────────────────────────────────────

/// The token to call the platform with as `integ`: an installation's (minted with the app's key, kept until a minute
/// before it expires), an OAuth sign-in's (renewed a minute before it expires), or the pasted one.
pub fn access(app: &App, integ: &Integration) -> Result<Auth, String> {
    let store = &app.platforms;
    match integ.via {
        Via::App => {
            if let Some((token, until)) = lock(&store.minted).get(&integ.id).cloned()
                && until > now_ms() + EARLY_MS
            {
                return Ok(Auth {
                    token,
                    installation: true,
                });
            }
            let reg = integ
                .registration
                .as_deref()
                .and_then(|r| store.registration(r))
                .ok_or("the GitHub App this installation belongs to is gone")?;
            let jwt = app_jwt(app, &reg)?;
            let id = integ.installation.ok_or("no installation id")?;
            let (token, until) = store.github(&reg.host).installation_token(&jwt, id)?;
            let until = until.unwrap_or(now_ms() + 50 * 60 * 1000);
            lock(&store.minted).insert(integ.id.clone(), (token.clone(), until));
            Ok(Auth {
                token,
                installation: true,
            })
        }
        Via::Oauth => {
            let token =
                secret(app, &integ.id, "TOKEN").ok_or("the sign-in's token is not in the vault")?;
            let due = integ.expires_ms.is_some_and(|t| t <= now_ms() + EARLY_MS);
            if !due {
                return Ok(Auth::user(&token));
            }
            let refresh =
                secret(app, &integ.id, "REFRESH").ok_or("the sign-in expired: sign in again")?;
            let reg = integ
                .registration
                .as_deref()
                .and_then(|r| store.registration(r))
                .ok_or("the app this sign-in came through is gone: sign in again")?;
            let client_secret =
                secret(app, &reg.id, "SECRET").ok_or("the app's secret is not in the vault")?;
            let tokens = store.platform(integ.kind, &integ.host).refresh(
                &reg.client_id,
                &client_secret,
                &refresh,
                &callback_url(app),
            )?;
            keep_tokens(app, &integ.id, &tokens)?;
            let mut next = integ.clone();
            next.expires_ms = tokens.expires_in.map(|s| now_ms() + s * 1000);
            store.set(&next)?;
            Ok(Auth::user(&tokens.access))
        }
        Via::Token => secret(app, &integ.id, "TOKEN")
            .map(|t| Auth::user(&t))
            .ok_or_else(|| "the token is not in the vault".to_string()),
    }
}

fn keep_tokens(app: &App, id: &str, tokens: &model::Tokens) -> Result<(), String> {
    put_secret(
        app,
        id,
        "TOKEN",
        &format!("{id} access token"),
        &tokens.access,
    )?;
    if let Some(r) = &tokens.refresh {
        put_secret(app, id, "REFRESH", &format!("{id} refresh token"), r)?;
    }
    Ok(())
}

fn app_jwt(app: &App, reg: &Registration) -> Result<String, String> {
    let pem =
        secret(app, &reg.id, "KEY").ok_or("the GitHub App's private key is not in the vault")?;
    github::app_jwt(&reg.client_id, &pem, now_ms() / 1000)
}

/// The git credential for a connection Weft pushes to or fetches from: `name` is `platform:<id>`. None for any other
/// name (the caller reads it from the vault as before), and when the authorization cannot give a token.
pub fn credential(app: &App, name: &str) -> Option<Result<String, String>> {
    let id = name.strip_prefix("platform:")?;
    let Some(integ) = app.platforms.integration(id) else {
        return Some(Err(format!("the authorization {id} is gone")));
    };
    Some(access(app, &integ).map(|a| a.token))
}

/// The authorization that speaks for the owner on `integ`'s host: their own sign-in or user-level token there when
/// there is one (the author of exports, the account a signing key goes on), else `integ` itself.
fn identity(app: &App, integ: &Integration) -> Integration {
    app.platforms
        .integrations()
        .into_iter()
        .filter(|i| {
            i.kind == integ.kind
                && i.host == integ.host
                && i.level == Level::User
                && i.via != Via::App
        })
        .max_by_key(|i| (i.via == Via::Oauth, i.checked_ms))
        .unwrap_or_else(|| integ.clone())
}

fn callback_url(app: &App) -> String {
    format!("{}{CALLBACK}", app.config.public_url.trim_end_matches('/'))
}

fn go_url(app: &App, state: &str) -> String {
    format!(
        "{}/v1/platforms/flows/{state}/go",
        app.config.public_url.trim_end_matches('/')
    )
}

/// The PKCE challenge of a verifier (S256).
fn challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn host_of(kind: Kind, raw: Option<&str>) -> Result<String, String> {
    let host = raw
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .unwrap_or(kind.default_host())
        .trim_end_matches('/')
        .to_string();
    let rest = host
        .strip_prefix("https://")
        .or_else(|| host.strip_prefix("http://"))
        .ok_or("a host is an http(s) URL, such as https://gitlab.example.com")?;
    if rest.is_empty() || rest.contains(['?', '#', ' ']) {
        return Err("a host is an http(s) URL, such as https://gitlab.example.com".into());
    }
    Ok(host)
}

// ── what the browser comes back with ─────────────────────────────────────────────────────────────────────────

/// The GitHub App is created: its credentials into the vault, then the owner installs it.
fn manifest_done(app: &App, host: &str, code: &str) -> Result<(String, Option<String>), String> {
    let gh = app.platforms.github(host);
    let creds = gh.convert(code)?;
    let reg_id = app.platforms.update(|saved| {
        saved.next += 1;
        format!("github-app-{}", saved.next)
    })?;
    put_secret(
        app,
        &reg_id,
        "SECRET",
        &format!("{} client secret", creds.name),
        &creds.client_secret,
    )?;
    put_secret(
        app,
        &reg_id,
        "KEY",
        &format!("{} private key", creds.name),
        &creds.pem,
    )?;
    let reg = Registration {
        id: reg_id.clone(),
        kind: Kind::Github,
        host: host.to_string(),
        client_id: creds.client_id.clone(),
        app: Some(GitHubApp {
            id: creds.id,
            slug: creds.slug.clone(),
            name: creds.name.clone(),
            owner: creds.owner.clone(),
            html_url: creds.html_url.clone(),
        }),
        created_ms: now_ms(),
    };
    app.platforms
        .update(|saved| saved.registrations.push(reg.clone()))?;
    persist(app)?;
    publish(app, "registered", &reg_id);
    let state = app.platforms.start(Step::Install {
        registration: reg_id,
    })?;
    Ok((
        format!(
            "The GitHub App {} is created. Next, install it on your account or an organisation and choose its repositories.",
            creds.name
        ),
        Some(go_url(app, &state)),
    ))
}

/// The app is installed (or its installation changed): the owner's sign-in that came with it, then every
/// installation as an authorization.
fn install_done(app: &App, reg_id: &str, code: Option<&str>) -> Result<Vec<String>, String> {
    let reg = app
        .platforms
        .registration(reg_id)
        .ok_or("the GitHub App is gone")?;
    let mut ids = Vec::new();
    if let Some(code) = code {
        let client_secret =
            secret(app, &reg.id, "SECRET").ok_or("the GitHub App's secret is not in the vault")?;
        let gh = app.platforms.github(&reg.host);
        let tokens = gh.exchange(&reg.client_id, &client_secret, code, &callback_url(app), "")?;
        ids.push(signed_in(app, &reg, &tokens)?);
    }
    ids.extend(sync_installations(app, &reg)?);
    Ok(ids)
}

/// Every installation of the plaza's GitHub App, as authorizations: a new one grants what GitHub was given; one that
/// is gone says so.
fn sync_installations(app: &App, reg: &Registration) -> Result<Vec<String>, String> {
    let jwt = app_jwt(app, reg)?;
    let gh = app.platforms.github(&reg.host);
    let found = gh.installations(&jwt)?;
    let mut ids = Vec::new();
    for inst in &found {
        let org = inst.account_type == "Organization";
        let fresh = Integration {
            id: String::new(),
            kind: Kind::Github,
            host: reg.host.clone(),
            via: Via::App,
            level: if org { Level::Org } else { Level::User },
            registration: Some(reg.id.clone()),
            installation: Some(inst.id),
            repository_selection: Some(inst.repository_selection.clone()),
            manage_url: Some(inst.html_url.clone()).filter(|u| !u.is_empty()),
            target: Some(inst.account.clone()),
            account: Account {
                id: inst.account_id,
                login: inst.account.clone(),
                name: inst.account.clone(),
                ..Account::default()
            },
            scopes: Vec::new(),
            expires_ms: None,
            grants: Grants::default(),
            created_ms: now_ms(),
            checked_ms: now_ms(),
            error: None,
        };
        let known =
            app.platforms.integrations().into_iter().any(|i| {
                i.installation == Some(inst.id) && i.registration.as_deref() == Some(&reg.id)
            });
        let mut saved = app.platforms.upsert(fresh, |i| {
            i.installation == Some(inst.id) && i.registration.as_deref() == Some(&reg.id)
        })?;
        if !known {
            // What the owner gave the app on GitHub is what the plaza may use, until they narrow it here.
            saved.grants.namespaces.insert(
                inst.account.clone(),
                NamespaceGrant {
                    all: true,
                    ..NamespaceGrant::default()
                },
            );
            app.platforms.set(&saved)?;
        }
        ids.push(saved.id);
    }
    // An installation GitHub no longer lists was removed there.
    let gone: Vec<Integration> = app
        .platforms
        .integrations()
        .into_iter()
        .filter(|i| i.registration.as_deref() == Some(&reg.id) && i.via == Via::App)
        .filter(|i| !found.iter().any(|f| Some(f.id) == i.installation))
        .collect();
    for mut g in gone {
        g.error = Some("uninstalled on GitHub".into());
        app.platforms.set(&g)?;
    }
    for id in &ids {
        publish(app, "synced", id);
    }
    Ok(ids)
}

/// An OAuth sign-in: the account, its tokens into the vault, one authorization per account and host.
fn signed_in(app: &App, reg: &Registration, tokens: &model::Tokens) -> Result<String, String> {
    let platform = app.platforms.platform(reg.kind, &reg.host);
    let account = platform.account(&Auth::user(&tokens.access))?;
    let integ = Integration {
        id: String::new(),
        kind: reg.kind,
        host: reg.host.clone(),
        via: Via::Oauth,
        level: Level::User,
        registration: Some(reg.id.clone()),
        installation: None,
        repository_selection: None,
        manage_url: None,
        target: None,
        account: account.clone(),
        scopes: tokens.scopes.clone(),
        expires_ms: tokens.expires_in.map(|s| now_ms() + s * 1000),
        grants: Grants::default(),
        created_ms: now_ms(),
        checked_ms: now_ms(),
        error: None,
    };
    let saved = app.platforms.upsert(integ, |i| {
        i.via == Via::Oauth
            && i.kind == reg.kind
            && i.host == reg.host
            && i.account.id == account.id
    })?;
    keep_tokens(app, &saved.id, tokens)?;
    persist(app)?;
    publish(app, "authorized", &saved.id);
    Ok(saved.id)
}

fn oauth_done(app: &App, reg_id: &str, code: &str, verifier: &str) -> Result<String, String> {
    let reg = app
        .platforms
        .registration(reg_id)
        .ok_or("the app is gone")?;
    let client_secret =
        secret(app, &reg.id, "SECRET").ok_or("the app's secret is not in the vault")?;
    let tokens = app.platforms.platform(reg.kind, &reg.host).exchange(
        &reg.client_id,
        &client_secret,
        code,
        &callback_url(app),
        verifier,
    )?;
    signed_in(app, &reg, &tokens)
}

/// `GET /v1/platforms/callback`: the platform sent the owner's browser back.
fn callback(app: &App, req: &Request) -> Response {
    let q = |k: &str| {
        req.query_param(k)
            .map(crate::plaza::percent_decode)
            .filter(|v| !v.is_empty())
    };
    let state = q("state");
    let code = q("code");
    let refused = q("error").map(|e| q("error_description").unwrap_or(e));
    let step = state.as_deref().and_then(|s| app.platforms.claim(s));
    let outcome: Result<(String, Option<String>, Vec<String>), String> = match (refused, step) {
        (Some(why), _) => Err(format!("The platform did not authorize the plaza: {why}")),
        (None, Some(Step::Manifest { host, .. })) => match code.as_deref() {
            Some(code) => {
                manifest_done(app, &host, code).map(|(text, next)| (text, next, Vec::new()))
            }
            None => Err("GitHub sent no code for the app".into()),
        },
        (None, Some(Step::Install { registration })) => {
            install_done(app, &registration, code.as_deref()).map(|ids| {
                (
                    "The GitHub App is installed. Choose what the plaza may use in the console."
                        .to_string(),
                    None,
                    ids,
                )
            })
        }
        (
            None,
            Some(Step::Oauth {
                registration,
                verifier,
            }),
        ) => match code.as_deref() {
            Some(code) => oauth_done(app, &registration, code, &verifier).map(|id| {
                (
                    "Signed in. Choose what the plaza may use in the console.".to_string(),
                    None,
                    vec![id],
                )
            }),
            None => Err("the platform sent no code".into()),
        },
        // An installation changed on GitHub (the owner opened it there): read every app's installations again.
        // A code without our state is never exchanged.
        (None, None) if q("installation_id").is_some() => {
            let mut ids = Vec::new();
            for reg in app
                .platforms
                .registrations()
                .iter()
                .filter(|r| r.app.is_some())
            {
                if let Ok(found) = sync_installations(app, reg) {
                    ids.extend(found);
                }
            }
            Ok(("The installation is updated.".to_string(), None, ids))
        }
        (None, None) => {
            Err("This link has expired or was used already. Start again from the console.".into())
        }
    };
    if let Some(s) = &state {
        app.platforms.finish(
            s,
            outcome
                .as_ref()
                .map(|(text, next, ids)| json!({ "text": text, "next": next, "integrations": ids }))
                .map_err(Clone::clone),
        );
    }
    match outcome {
        Ok((_, Some(next), _)) => {
            Response::new(302, "text/plain", "").with_header("location", &next)
        }
        Ok((text, None, _)) => page(app, "Done", &text, true),
        Err(why) => page(app, "Not authorized", &why, false),
    }
}

/// The page the browser lands on at the end: what happened, and the way back to the console.
fn page(app: &App, title: &str, text: &str, ok: bool) -> Response {
    let console = format!(
        "{}/#/platforms",
        app.config.public_url.trim_end_matches('/')
    );
    let html = format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width\"><title>{t} · AgentPlaza</title>\
         <body style=\"font:15px system-ui;background:#11131a;color:#e8e8ec;display:grid;place-items:center;min-height:90vh\">\
         <main style=\"max-width:32rem\"><h1 style=\"font-size:20px\">{t}</h1><p>{x}</p>\
         <p><a style=\"color:{c}\" href=\"{u}\">Back to the console</a></p></main>",
        t = html_escape(title),
        x = html_escape(text),
        c = if ok { "#f08a3c" } else { "#e0707a" },
        u = html_escape(&console),
    );
    Response::new(if ok { 200 } else { 400 }, "text/html; charset=utf-8", html)
}

fn html_escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// `GET /v1/platforms/flows/{state}/go`: the page the owner's browser opens to start a flow.
fn go(app: &App, state: &str) -> Response {
    let Some(step) = app.platforms.peek(state) else {
        return page(
            app,
            "Expired",
            "This link has expired or was used already. Start again from the console.",
            false,
        );
    };
    match step {
        Step::Manifest { host, org, name } => {
            let gh = app.platforms.github(&host);
            let manifest = GitHub::manifest(
                &name,
                app.config.public_url.trim_end_matches('/'),
                &callback_url(app),
            );
            let action = gh.manifest_url(org.as_deref(), state);
            let html = format!(
                "<!doctype html><meta charset=utf-8><title>Create the GitHub App · AgentPlaza</title>\
                 <body style=\"font:15px system-ui;background:#11131a;color:#e8e8ec\">\
                 <form method=post action=\"{a}\"><input type=hidden name=manifest value=\"{m}\">\
                 <p>GitHub asks you to confirm the app's name, then creates it.</p><button>Continue to GitHub</button></form>\
                 <script>document.forms[0].submit()</script>",
                a = html_escape(&action),
                m = html_escape(&manifest.to_string()),
            );
            Response::new(200, "text/html; charset=utf-8", html)
        }
        Step::Install { registration } => match app.platforms.registration(&registration) {
            Some(Registration {
                host, app: Some(a), ..
            }) => {
                let url = app.platforms.github(&host).install_url(&a.slug, state);
                Response::new(302, "text/plain", "").with_header("location", &url)
            }
            _ => page(app, "Not found", "The GitHub App is gone.", false),
        },
        Step::Oauth {
            registration,
            verifier,
        } => match app.platforms.registration(&registration) {
            Some(reg) => {
                let url = app.platforms.platform(reg.kind, &reg.host).authorize_url(
                    &reg.client_id,
                    &callback_url(app),
                    state,
                    &challenge(&verifier),
                );
                Response::new(302, "text/plain", "").with_header("location", &url)
            }
            None => page(app, "Not found", "The app is gone.", false),
        },
    }
}

/// How a flow went. A flow that handed the browser on to another (the GitHub App created, then installed) reads as
/// that one, so a console waits on the state it started with.
fn flow_status(flows: &HashMap<String, Flow>, state: &str, depth: u8) -> Option<Value> {
    let f = flows.get(state)?;
    Some(match &f.outcome {
        None if now_ms().saturating_sub(f.created_ms) >= FLOW_TTL_MS => {
            json!({ "status": "expired" })
        }
        None => json!({ "status": "pending" }),
        Some(Ok(v)) => match v["next"].as_str().and_then(|u| u.rsplit('/').nth(1)) {
            Some(next) if depth < 3 => flow_status(flows, next, depth + 1)
                .unwrap_or_else(|| json!({ "status": "pending", "detail": v })),
            _ => json!({ "status": "done", "detail": v }),
        },
        Some(Err(e)) => json!({ "status": "failed", "error": e }),
    })
}

// ── what an authorization reaches ────────────────────────────────────────────────────────────────────────────

/// The namespaces an authorization reaches, each with the owner's grant. An organisation- or repository-level one
/// reaches only its own.
fn namespaces(app: &App, integ: &Integration) -> Result<Vec<Namespace>, String> {
    let auth = access(app, integ)?;
    let all = app
        .platforms
        .platform(integ.kind, &integ.host)
        .namespaces(&auth, &integ.account)?;
    let only = match (integ.level, integ.target.as_deref()) {
        (Level::Org, Some(t)) => Some(t.to_string()),
        (Level::Repo, Some(t)) => t.rsplit_once('/').map(|(ns, _)| ns.to_string()),
        _ => None,
    };
    Ok(match only {
        Some(t) => {
            let mut kept: Vec<Namespace> = all.into_iter().filter(|n| n.name == t).collect();
            if kept.is_empty() {
                kept.push(Namespace {
                    name: t.clone(),
                    kind: "org".into(),
                    display: t,
                    id: 0,
                });
            }
            kept
        }
        None => all,
    })
}

/// The Weft connections that push to `repo` (by its clone URL): `[{weft_repo, project, connection}]`.
fn connections_to(weft_repos: &Value, repo: &Repo) -> Vec<Value> {
    let norm = |u: &str| {
        u.trim_end_matches('/')
            .trim_end_matches(".git")
            .to_ascii_lowercase()
    };
    let target = norm(&repo.clone_url);
    let mut out = Vec::new();
    for r in weft_repos["repos"].as_array().into_iter().flatten() {
        for c in r["connections"].as_array().into_iter().flatten() {
            if c["remote"].as_str().map(norm).as_deref() == Some(target.as_str()) {
                out.push(json!({ "weft_repo": r["name"], "project": r["project"], "connection": c["id"], "state": c["state"] }));
            }
        }
    }
    out
}

/// The repositories of one namespace, with whether each is granted and where it is connected.
fn repos(app: &App, integ: &Integration, namespace: &str) -> Result<Vec<Value>, String> {
    let ns = namespaces(app, integ)?
        .into_iter()
        .find(|n| n.name == namespace)
        .ok_or_else(|| format!("{namespace} is not a namespace this authorization reaches"))?;
    let auth = access(app, integ)?;
    let platform = app.platforms.platform(integ.kind, &integ.host);
    let list = match (integ.level, integ.target.as_deref()) {
        (Level::Repo, Some(full)) => vec![platform.repo(&auth, full)?],
        _ => platform.repos(&auth, &ns)?,
    };
    let weft = app.weft.get("/repos").unwrap_or(Value::Null);
    Ok(list
        .into_iter()
        .map(|r| {
            let granted = integ.grants.allows(&r);
            let connected = connections_to(&weft, &r);
            let mut v = json!(r);
            v["granted"] = json!(granted);
            v["connections"] = json!(connected);
            v
        })
        .collect())
}

// ── connect ──────────────────────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ConnectBody {
    /// The repository's full name on the platform.
    repo: String,
    project: String,
    /// Register the hub's signing key on the owner's account, so exports show as verified.
    #[serde(default)]
    sign: bool,
    /// Push what Weft has now, rather than at the next landing.
    #[serde(default)]
    export: bool,
}

/// Connects repository `repo` of authorization `id` to `project`, as the connect route does: for adopting a project
/// that lives on a platform already ([`crate::adopt`]).
pub fn connect_project(
    app: &App,
    id: &str,
    repo: &str,
    project: &str,
    sign: bool,
) -> Result<Value, (u16, String)> {
    let integ = app
        .platforms
        .integration(id)
        .ok_or((404, format!("no authorization {id}")))?;
    connect(
        app,
        &integ,
        &ConnectBody {
            repo: repo.to_string(),
            project: project.to_string(),
            sign,
            export: false,
        },
    )
}

/// A granted repository joins a project: the connection (with the owner as the author), the history imported
/// when the project's repo has none, the signing key registered when asked.
fn connect(app: &App, integ: &Integration, body: &ConnectBody) -> Result<Value, (u16, String)> {
    let bad = |m: String| (400, m);
    let failed = |m: String| (502, m);
    if !app.weft.running() {
        return Err((503, "Weft is not running".into()));
    }
    let auth = access(app, integ).map_err(failed)?;
    let platform = app.platforms.platform(integ.kind, &integ.host);
    let repo = platform.repo(&auth, &body.repo).map_err(failed)?;
    if !integ.grants.allows(&repo) {
        return Err((
            403,
            format!(
                "{} is not among what this authorization may use: grant it first",
                repo.full_name
            ),
        ));
    }
    if !repo.can_push {
        return Err((
            403,
            format!("this authorization cannot push to {}", repo.full_name),
        ));
    }
    let weft_repo = crate::weft_hub::ensure_repo(app, &body.project).map_err(bad)?;
    let detail = app
        .weft
        .get(&format!("/repos/{weft_repo}"))
        .map_err(failed)?;
    let ids: Vec<String> = detail["connections"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["id"].as_str().map(str::to_string))
        .collect();
    let already = detail["connections"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|c| {
            c["remote"].as_str().map(|r| r.trim_end_matches(".git"))
                == Some(repo.clone_url.trim_end_matches(".git"))
        });
    if already {
        return Err((
            409,
            format!("{weft_repo} already exports to {}", repo.full_name),
        ));
    }
    let base = integ.kind.as_str().to_string();
    let id = (1..)
        .map(|n| {
            if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            }
        })
        .find(|c| !ids.contains(c))
        .unwrap_or(base);
    // W19: the owner as this host knows them; the plaza's settings say nothing better.
    let who = identity(app, integ);
    if who.account.email.is_empty() {
        return Err(bad(format!(
            "{} has no email the platform verified: sign in on {} so exported commits have an author",
            who.account.login, integ.host
        )));
    }
    let branch = if repo.default_branch.is_empty() {
        "main".to_string()
    } else {
        repo.default_branch.clone()
    };
    let conn = weft::connect::connection_body(
        &weft_repo,
        &id,
        &repo.clone_url,
        integ.kind.as_str(),
        &branch,
        &who.account.name,
        &who.account.email,
        Some(&format!("platform:{}", integ.id)),
    );
    app.weft.console("connection.add", conn).map_err(failed)?;
    let mut out = json!({ "weft_repo": weft_repo, "connection": id, "remote": repo.clone_url, "branch": branch, "author": format!("{} <{}>", who.account.name, who.account.email) });
    // A repo with no history yet takes the platform's, once.
    if detail["landed"].as_u64().unwrap_or(0) == 0 {
        let imported = app.weft.post(
            &format!("/repos/{weft_repo}/import"),
            &json!({ "source": repo.clone_url, "branch": branch, "token": auth.token, "platform": integ.kind.as_str() }),
        );
        out["imported"] = match imported {
            Ok(r) => r,
            // An empty repository has no branch to import: the first export pushes Weft's history.
            Err(e) if e.contains("has no branch") || e.contains("empty") => {
                json!({ "commits": 0, "empty": true })
            }
            Err(e) => json!({ "error": e }),
        };
    }
    if body.sign {
        out["signing_key"] = match signing_key(app, &who) {
            Ok(()) => json!("registered"),
            Err(e) => json!({ "error": e }),
        };
    }
    if body.export {
        crate::weft_hub::export_all(app, &weft_repo);
    }
    publish(app, "connected", &integ.id);
    Ok(out)
}

/// Registers the hub's signing key on the owner's account.
fn signing_key(app: &App, who: &Integration) -> Result<(), String> {
    let key = app.weft.get("/signing-key")?["key"]
        .as_str()
        .ok_or("Weft holds no signing key")?
        .to_string();
    let auth = access(app, who)?;
    app.platforms.platform(who.kind, &who.host).add_signing_key(
        &auth,
        "AgentPlaza (Weft export)",
        &key,
    )
}

// ── routes ───────────────────────────────────────────────────────────────────────────────────────────────────

fn publish(app: &App, op: &str, id: &str) {
    app.bus.publish("platform", &json!({ "op": op, "id": id }));
}

fn body(req: &Request) -> Result<Value, Response> {
    if req.body.is_empty() {
        return Ok(json!({}));
    }
    req.json::<Value>()
        .map_err(|_| Response::error(400, "the body is not JSON"))
}

fn reply(r: Result<Value, String>, status: u16) -> Response {
    match r {
        Ok(v) => Response::json(200, &v),
        Err(e) => Response::error(status, &e),
    }
}

/// An integration as the consoles see it: no secrets, plus where it sits.
fn view(app: &App, i: &Integration) -> Value {
    let mut v = json!(i);
    v["label"] = json!(i.kind.label());
    v["identity"] = json!(identity(app, i).account);
    v
}

fn list(app: &App) -> Value {
    json!({
        "kinds": Kind::ALL.iter().map(|k| json!({ "kind": k, "label": k.label(), "default_host": k.default_host() })).collect::<Vec<_>>(),
        "registrations": app.platforms.registrations(),
        "integrations": app.platforms.integrations().iter().map(|i| view(app, i)).collect::<Vec<_>>(),
        "callback": callback_url(app),
    })
}

fn integration_of(app: &App, p: &Params<'_>) -> Result<Integration, Response> {
    app.platforms
        .integration(p.get("id"))
        .ok_or_else(|| Response::error(404, "no such authorization"))
}

fn start_route(app: &App, step: Step) -> Response {
    match app.platforms.start(step) {
        Ok(state) => Response::json(200, &json!({ "state": state, "go": go_url(app, &state) })),
        Err(e) => Response::error(500, &e),
    }
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/platforms", |app, _, _| Response::json(200, &list(app)))
        .doc(RouteDoc::new(
            "List the git platforms the plaza is authorized on",
        ))
        .post("/v1/platforms/github/app", |app, req, _| {
            let v = match body(req) {
                Ok(v) => v,
                Err(r) => return r,
            };
            let host = match host_of(Kind::Github, v["host"].as_str()) {
                Ok(h) => h,
                Err(e) => return Response::error(400, &e),
            };
            let suffix = apz_os::random::hex(3).unwrap_or_default();
            let name = v["name"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("AgentPlaza {suffix}"));
            let org = v["org"]
                .as_str()
                .filter(|o| !o.is_empty())
                .map(str::to_string);
            start_route(app, Step::Manifest { host, org, name })
        })
        .doc(RouteDoc::new("Create the plaza's GitHub App from a manifest").errors(&[400]))
        .post("/v1/platforms/github/install", |app, req, _| {
            let v = match body(req) {
                Ok(v) => v,
                Err(r) => return r,
            };
            let reg = v["registration"].as_str().unwrap_or("");
            match app.platforms.registration(reg) {
                Some(r) if r.app.is_some() => {
                    start_route(app, Step::Install { registration: r.id })
                }
                _ => Response::error(404, "no such GitHub App"),
            }
        })
        .doc(
            RouteDoc::new("Install the plaza's GitHub App on a user or an organisation")
                .errors(&[404]),
        )
        .post("/v1/platforms/registrations", |app, req, _| {
            let v = match body(req) {
                Ok(v) => v,
                Err(r) => return r,
            };
            reply(register(app, &v), 400)
        })
        .doc(RouteDoc::new("Record an OAuth app the owner registered on a platform").errors(&[400]))
        .delete("/v1/platforms/registrations/{id}", |app, _, p| {
            let id = p.get("id").to_string();
            // The record is written before the app's secrets are forgotten, so a crash keeps both or neither.
            let gone = app
                .platforms
                .update(|saved| {
                    let n = saved.registrations.len();
                    saved.registrations.retain(|r| r.id != id);
                    n != saved.registrations.len()
                })
                .and_then(|found| persist(app).map(|()| found));
            match gone {
                Ok(true) => {
                    forget_secret(app, &id, "SECRET");
                    forget_secret(app, &id, "KEY");
                    publish(app, "unregistered", &id);
                    Response::json(200, &json!({ "removed": id }))
                }
                Ok(false) => Response::error(404, "no such app"),
                Err(e) => Response::error(500, &e),
            }
        })
        .doc(RouteDoc::new("Forget a registered app").errors(&[404]))
        .post("/v1/platforms/authorize", |app, req, _| {
            let v = match body(req) {
                Ok(v) => v,
                Err(r) => return r,
            };
            let Some(reg) = v["registration"]
                .as_str()
                .and_then(|r| app.platforms.registration(r))
            else {
                return Response::error(404, "no such app");
            };
            let verifier = match apz_os::random::hex(32) {
                Ok(v) => v,
                Err(e) => return Response::error(500, &e.to_string()),
            };
            start_route(
                app,
                Step::Oauth {
                    registration: reg.id,
                    verifier,
                },
            )
        })
        .doc(RouteDoc::new("Start the owner's sign-in on a platform").errors(&[404]))
        .post("/v1/platforms/tokens", |app, req, _| {
            let v = match body(req) {
                Ok(v) => v,
                Err(r) => return r,
            };
            reply(by_token(app, &v), 400)
        })
        .doc(RouteDoc::new("Authorize the plaza with a token").errors(&[400]))
        .get("/v1/platforms/flows/{state}", |app, _, p| {
            let flows = lock(&app.platforms.flows);
            match flow_status(&flows, p.get("state"), 0) {
                Some(v) => Response::json(200, &v),
                None => Response::error(404, "no such flow"),
            }
        })
        .doc(RouteDoc::new("Show how a started authorization went").errors(&[404]))
        .get("/v1/platforms/flows/{state}/go", |app, _, p| {
            go(app, p.get("state"))
        })
        .doc(RouteDoc::new(
            "Open the platform's page for a started authorization",
        ))
        .get(CALLBACK, |app, req, _| callback(app, req))
        .doc(RouteDoc::new("Take the browser back from a platform"))
        .post("/v1/platforms/{id}/sync", |app, _, p| {
            let integ = match integration_of(app, p) {
                Ok(i) => i,
                Err(r) => return r,
            };
            reply(sync(app, integ).map(|i| view(app, &i)), 502)
        })
        .doc(RouteDoc::new("Read an authorization's account again").errors(&[404, 502]))
        .delete("/v1/platforms/{id}", |app, _, p| {
            let id = p.get("id").to_string();
            // The record is written before the tokens are forgotten, so a crash keeps both or neither.
            let removed = app
                .platforms
                .update(|saved| {
                    let n = saved.integrations.len();
                    saved.integrations.retain(|i| i.id != id);
                    n != saved.integrations.len()
                })
                .and_then(|found| persist(app).map(|()| found));
            match removed {
                Ok(true) => {
                    forget_secret(app, &id, "TOKEN");
                    forget_secret(app, &id, "REFRESH");
                    lock(&app.platforms.minted).remove(&id);
                    publish(app, "removed", &id);
                    Response::json(200, &json!({ "removed": id }))
                }
                Ok(false) => Response::error(404, "no such authorization"),
                Err(e) => Response::error(500, &e),
            }
        })
        .doc(RouteDoc::new("Forget an authorization and its tokens").errors(&[404]))
        .get("/v1/platforms/{id}/namespaces", |app, _, p| {
            let integ = match integration_of(app, p) {
                Ok(i) => i,
                Err(r) => return r,
            };
            reply(
                namespaces(app, &integ).map(|list| {
                    json!(
                        list.iter()
                            .map(|n| {
                                let mut v = json!(n);
                                v["grant"] = json!(integ.grants.namespaces.get(&n.name));
                                v
                            })
                            .collect::<Vec<_>>()
                    )
                }),
                502,
            )
        })
        .doc(RouteDoc::new("List the namespaces an authorization reaches").errors(&[404, 502]))
        .get("/v1/platforms/{id}/repos", |app, req, p| {
            let integ = match integration_of(app, p) {
                Ok(i) => i,
                Err(r) => return r,
            };
            let Some(ns) = req
                .query_param("namespace")
                .map(crate::plaza::percent_decode)
            else {
                return Response::error(400, "repos needs ?namespace=");
            };
            reply(repos(app, &integ, &ns).map(|l| json!(l)), 502)
        })
        .doc(
            RouteDoc::new("List a namespace's repositories with their grants")
                .errors(&[400, 404, 502]),
        )
        .route("PUT", "/v1/platforms/{id}/grants", |app, req, p| {
            let mut integ = match integration_of(app, p) {
                Ok(i) => i,
                Err(r) => return r,
            };
            let grants: Grants = match req.json() {
                Ok(g) => g,
                Err(_) => {
                    return Response::error(
                        400,
                        "the body is {namespaces: {<name>: {all, repos}}}",
                    );
                }
            };
            integ.grants = grants;
            match app.platforms.set(&integ).and_then(|()| persist(app)) {
                Ok(()) => {
                    publish(app, "granted", &integ.id);
                    Response::json(200, &view(app, &integ))
                }
                Err(e) => Response::error(500, &e),
            }
        })
        .doc(RouteDoc::new("Choose what an authorization may use").errors(&[400, 404]))
        .post("/v1/platforms/{id}/connect", |app, req, p| {
            let integ = match integration_of(app, p) {
                Ok(i) => i,
                Err(r) => return r,
            };
            let b: ConnectBody = match req.json() {
                Ok(b) => b,
                Err(_) => {
                    return Response::error(400, "the body is {repo, project, sign?, export?}");
                }
            };
            match connect(app, &integ, &b) {
                Ok(v) => Response::json(200, &v),
                Err((status, why)) => Response::error(status, &why),
            }
        })
        .doc(
            RouteDoc::new("Connect a granted repository to a project")
                .errors(&[400, 403, 404, 409, 502, 503]),
        )
        .post("/v1/platforms/{id}/disconnect", |app, req, p| {
            if let Err(r) = integration_of(app, p) {
                return r;
            }
            let v = match body(req) {
                Ok(v) => v,
                Err(r) => return r,
            };
            let (Some(repo), Some(conn)) = (v["weft_repo"].as_str(), v["connection"].as_str())
            else {
                return Response::error(400, "disconnect needs weft_repo and connection");
            };
            reply(
                app.weft
                    .console("connection.remove", json!({ "repo": repo, "id": conn }))
                    .map(|_| {
                        publish(app, "disconnected", p.get("id"));
                        json!({ "weft_repo": repo, "connection": conn, "removed": true })
                    }),
                502,
            )
        })
        .doc(
            RouteDoc::new("Stop a project's repo exporting to a repository")
                .errors(&[400, 404, 502]),
        )
}

/// `POST /v1/platforms/registrations`: an OAuth app the owner registered on a host, with the callback URL given.
fn register(app: &App, v: &Value) -> Result<Value, String> {
    let kind = v["kind"]
        .as_str()
        .and_then(Kind::parse)
        .ok_or("kind is github, gitlab or forgejo")?;
    let host = host_of(kind, v["host"].as_str())?;
    let client_id = v["client_id"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("client_id is required")?;
    let client_secret = v["client_secret"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("client_secret is required")?;
    let id = app.platforms.update(|saved| {
        saved.next += 1;
        format!("{}-app-{}", kind.as_str(), saved.next)
    })?;
    put_secret(
        app,
        &id,
        "SECRET",
        &format!("{} OAuth app on {host}", kind.label()),
        client_secret,
    )?;
    let reg = Registration {
        id: id.clone(),
        kind,
        host,
        client_id: client_id.to_string(),
        app: None,
        created_ms: now_ms(),
    };
    app.platforms
        .update(|saved| saved.registrations.push(reg.clone()))?;
    persist(app)?;
    publish(app, "registered", &id);
    Ok(json!(reg))
}

/// `POST /v1/platforms/tokens`: an authorization by token, checked with the platform. An organisation-level one
/// (a GitLab group token, a token for one organisation) grants its organisation; a repository-level one (a GitLab
/// project token, a fine-grained token for one repository) its repository; a user-level one nothing yet.
fn by_token(app: &App, v: &Value) -> Result<Value, String> {
    let kind = v["kind"]
        .as_str()
        .and_then(Kind::parse)
        .ok_or("kind is github, gitlab or forgejo")?;
    let host = host_of(kind, v["host"].as_str())?;
    let token = v["token"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("token is required")?;
    let level = match v["level"].as_str().unwrap_or("user") {
        "user" => Level::User,
        "org" | "group" => Level::Org,
        "repo" => Level::Repo,
        other => return Err(format!("level {other} is user, org or repo")),
    };
    let target = v["target"]
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    if level != Level::User && target.is_none() {
        return Err("an organisation- or repository-level token names its target".into());
    }
    let platform = app.platforms.platform(kind, &host);
    let auth = Auth::user(token);
    let account = platform.account(&auth)?;
    let mut grants = Grants::default();
    match (level, target.as_deref()) {
        (Level::Org, Some(t)) => {
            grants.namespaces.insert(
                t.to_string(),
                NamespaceGrant {
                    all: true,
                    ..NamespaceGrant::default()
                },
            );
        }
        (Level::Repo, Some(t)) => {
            let repo = platform.repo(&auth, t)?;
            grants
                .namespaces
                .entry(repo.namespace.clone())
                .or_default()
                .repos
                .insert(repo.name.clone());
        }
        _ => {}
    }
    let integ = Integration {
        id: String::new(),
        kind,
        host: host.clone(),
        via: Via::Token,
        level,
        registration: None,
        installation: None,
        repository_selection: None,
        manage_url: None,
        target: target.clone(),
        account: account.clone(),
        scopes: Vec::new(),
        expires_ms: None,
        grants,
        created_ms: now_ms(),
        checked_ms: now_ms(),
        error: None,
    };
    let saved = app.platforms.upsert(integ, |i| {
        i.via == Via::Token
            && i.kind == kind
            && i.host == host
            && i.account.id == account.id
            && i.target == target
    })?;
    put_secret(
        app,
        &saved.id,
        "TOKEN",
        &format!("{} token for {}", kind.label(), account.login),
        token,
    )?;
    persist(app)?;
    publish(app, "authorized", &saved.id);
    Ok(view(app, &saved))
}

/// Reads an authorization's account again (and, for an installation, every installation of its app).
fn sync(app: &App, mut integ: Integration) -> Result<Integration, String> {
    if integ.via == Via::App {
        let reg = integ
            .registration
            .as_deref()
            .and_then(|r| app.platforms.registration(r))
            .ok_or("the GitHub App is gone")?;
        sync_installations(app, &reg)?;
        return app
            .platforms
            .integration(&integ.id)
            .ok_or_else(|| "the installation is gone".to_string());
    }
    let read = access(app, &integ).and_then(|auth| {
        app.platforms
            .platform(integ.kind, &integ.host)
            .account(&auth)
    });
    integ.checked_ms = now_ms();
    match read {
        Ok(account) => {
            integ.account = account;
            integ.error = None;
        }
        Err(e) => integ.error = Some(e),
    }
    app.platforms.set(&integ)?;
    publish(app, "synced", &integ.id);
    match &integ.error {
        Some(e) => Err(e.clone()),
        None => Ok(integ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_names_are_env_names() {
        assert_eq!(
            secret_name("github-app-3", "KEY"),
            "PLATFORM_GITHUB_APP_3_KEY"
        );
        assert_eq!(
            secret_name("gitlab-12", "TOKEN"),
            "PLATFORM_GITLAB_12_TOKEN"
        );
    }

    #[test]
    fn hosts_are_http_origins_and_default_to_the_public_instance() {
        assert_eq!(host_of(Kind::Gitlab, None).unwrap(), "https://gitlab.com");
        assert_eq!(
            host_of(Kind::Forgejo, Some(" https://git.example.com/ ")).unwrap(),
            "https://git.example.com"
        );
        assert!(host_of(Kind::Github, Some("github.com")).is_err());
        assert!(host_of(Kind::Github, Some("https://x?y")).is_err());
    }

    #[test]
    fn the_pkce_challenge_is_s256() {
        // RFC 7636, appendix B.
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn a_connection_is_found_by_its_remote_whatever_its_spelling() {
        let weft = json!({ "repos": [{ "name": "pr1-notes", "project": "pr1", "connections": [{ "id": "github", "remote": "https://github.com/Irfan/Notes.git", "state": "in-step" }] }] });
        let repo = Repo {
            clone_url: "https://github.com/irfan/notes".into(),
            ..Repo::default()
        };
        assert_eq!(
            connections_to(&weft, &repo),
            [
                json!({ "weft_repo": "pr1-notes", "project": "pr1", "connection": "github", "state": "in-step" })
            ]
        );
        assert!(
            connections_to(
                &weft,
                &Repo {
                    clone_url: "https://github.com/irfan/other.git".into(),
                    ..Repo::default()
                }
            )
            .is_empty()
        );
    }

    #[test]
    fn a_grant_names_namespaces_and_repositories() {
        let mut g = Grants::default();
        g.namespaces.insert(
            "irfan".into(),
            NamespaceGrant {
                all: false,
                repos: ["notes".to_string()].into(),
            },
        );
        g.namespaces.insert(
            "JumpHash".into(),
            NamespaceGrant {
                all: true,
                ..NamespaceGrant::default()
            },
        );
        let repo = |ns: &str, n: &str| Repo {
            namespace: ns.into(),
            name: n.into(),
            ..Repo::default()
        };
        assert!(g.allows(&repo("irfan", "notes")));
        assert!(!g.allows(&repo("irfan", "secret")));
        assert!(g.allows(&repo("JumpHash", "anything")));
        assert!(!g.allows(&repo("other", "notes")));
    }

    #[test]
    fn a_flow_that_handed_on_reads_as_the_next_one() {
        let flow = |outcome: Option<Result<Value, String>>| Flow {
            step: None,
            created_ms: now_ms(),
            outcome,
        };
        let mut flows = HashMap::new();
        flows.insert(
            "a".to_string(),
            flow(Some(Ok(
                json!({ "text": "created", "next": "http://hub/v1/platforms/flows/b/go" }),
            ))),
        );
        flows.insert("b".to_string(), flow(None));
        assert_eq!(flow_status(&flows, "a", 0).unwrap()["status"], "pending");
        flows.insert(
            "b".to_string(),
            flow(Some(Ok(json!({ "integrations": ["github-2"] })))),
        );
        assert_eq!(
            flow_status(&flows, "a", 0).unwrap(),
            json!({ "status": "done", "detail": { "integrations": ["github-2"] } })
        );
        flows.insert("b".to_string(), flow(Some(Err("denied".into()))));
        assert_eq!(flow_status(&flows, "a", 0).unwrap()["status"], "failed");
        assert!(flow_status(&flows, "zz", 0).is_none());
    }

    #[test]
    fn a_record_round_trips_through_its_tables() {
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        let mut grants = Grants::default();
        grants.namespaces.insert(
            "JumpHash".into(),
            NamespaceGrant {
                all: false,
                repos: ["notes".to_string()].into(),
            },
        );
        let saved = Saved {
            registrations: vec![Registration {
                id: "github-app-1".into(),
                kind: Kind::Github,
                host: "https://github.com".into(),
                client_id: "Iv1.abc".into(),
                app: Some(GitHubApp {
                    id: 7,
                    slug: "agentplaza".into(),
                    name: "AgentPlaza".into(),
                    owner: "JumpHash".into(),
                    html_url: "https://github.com/apps/agentplaza".into(),
                }),
                created_ms: 1,
            }],
            integrations: vec![Integration {
                id: "github-2".into(),
                kind: Kind::Github,
                host: "https://github.com".into(),
                via: Via::App,
                level: Level::Org,
                registration: Some("github-app-1".into()),
                installation: Some(42),
                repository_selection: Some("selected".into()),
                manage_url: None,
                target: Some("JumpHash".into()),
                account: Account {
                    id: 9,
                    login: "JumpHash".into(),
                    name: "JumpHash".into(),
                    email: String::new(),
                    avatar_url: String::new(),
                },
                scopes: vec!["repo".into()],
                expires_ms: None,
                grants,
                created_ms: 2,
                checked_ms: 3,
                error: None,
            }],
            next: 2,
        };
        apz_store::table::replace(&conn, def(), &RowSet::one(def(), &saved).unwrap()).unwrap();

        let store = Store::with_http(Arc::new(http::Fake::new(|_, _, _| (404, Value::Null))));
        store.load(&conn).unwrap();
        assert_eq!(store.registrations(), saved.registrations);
        assert_eq!(store.integrations(), saved.integrations);
        assert_eq!(lock(&store.saved).next, 2);
        // Nothing stored is an empty record.
        let empty = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        empty.execute_batch(&def().ddl()).unwrap();
        store.load(&empty).unwrap();
        assert!(store.registrations().is_empty() && store.integrations().is_empty());
    }

    #[test]
    fn a_flow_is_taken_once_and_not_after_it_expires() {
        let store = Store::with_http(Arc::new(http::Fake::new(|_, _, _| (404, Value::Null))));
        let state = store
            .start(Step::Install {
                registration: "github-app-1".into(),
            })
            .unwrap();
        assert!(store.peek(&state).is_some());
        assert!(matches!(store.claim(&state), Some(Step::Install { .. })));
        assert!(store.claim(&state).is_none(), "a state works once");
        let old = store
            .start(Step::Install {
                registration: "x".into(),
            })
            .unwrap();
        lock(&store.flows).get_mut(&old).unwrap().created_ms = 0;
        assert!(store.claim(&old).is_none(), "an expired state does nothing");
    }
}
