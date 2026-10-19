//! `agentplaza platforms`: the git platforms the plaza is authorized on (GitHub, GitLab, Forgejo). `list` shows what is
//! authorized and what the plaza may use; `github-app`, `install`, `sign-in` and `token` authorize it (the browser
//! steps print their page and wait for the browser to come back); `grant` and `ungrant` choose the namespaces and
//! repositories; `connect` joins a granted repository to a project.

use std::collections::BTreeSet;
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use clap::Subcommand;
use serde_json::{Map, Value, json};

use crate::ctx::{self, Ctx, confirm};

/// How often a started flow is asked whether the browser has come back.
const POLL: Duration = Duration::from_secs(2);
/// How long a started flow is waited on (the hub expires it after 15 minutes as well).
const WAIT: Duration = Duration::from_secs(15 * 60);
/// The platforms, by the kind names the hub uses.
const KINDS: [&str; 3] = ["github", "gitlab", "forgejo"];

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The state, then the apps and authorizations of each host: what each reaches and who it is.
    List,
    /// Create the plaza's GitHub App (once per host), then install it: the page is printed and the command waits.
    GithubApp {
        /// The GitHub host (default: github.com).
        #[arg(long)]
        host: Option<String>,
        /// Own the App by this organisation (default: your account).
        #[arg(long)]
        org: Option<String>,
        /// The App's name on GitHub (default: AgentPlaza and a short suffix).
        #[arg(long)]
        name: Option<String>,
    },
    /// Install the plaza's GitHub App on your account or an organisation: the page is printed and the command waits.
    Install {
        /// The App's registration, such as `github-app-1`.
        registration: String,
    },
    /// Record an OAuth app you registered on a platform. The client secret is read without echo (or from stdin).
    Register {
        /// The platform.
        #[arg(value_parser = ["github", "gitlab", "forgejo"])]
        kind: String,
        /// The app's client ID.
        #[arg(long)]
        client_id: String,
        /// The platform's web address (default: the public instance).
        #[arg(long)]
        host: Option<String>,
    },
    /// Sign in to the platform through a registered OAuth app: the page is printed and the command waits.
    SignIn {
        /// The registration, such as `gitlab-app-4`.
        registration: String,
    },
    /// Authorize the plaza with a token you made on the platform. The token is read without echo (or from stdin).
    Token {
        /// The platform.
        #[arg(value_parser = ["github", "gitlab", "forgejo"])]
        kind: String,
        /// The platform's web address (default: the public instance).
        #[arg(long)]
        host: Option<String>,
        /// What the token covers: your account, an organisation or group, or one repository.
        #[arg(long, default_value = "user", value_parser = ["user", "org", "repo"])]
        level: String,
        /// The organisation or group (org level), or `namespace/name` (repo level).
        #[arg(long)]
        target: Option<String>,
    },
    /// The namespaces an authorization reaches (users, organisations, groups), with what is granted.
    Namespaces {
        /// The authorization's id, such as `github-2`.
        id: String,
    },
    /// One namespace's repositories: granted (✓), private, whether the plaza can push, and where each is connected.
    Repos {
        /// The authorization's id.
        id: String,
        /// The namespace's name.
        #[arg(long)]
        namespace: String,
    },
    /// Let the plaza use a namespace: all of it (--all) or the named repositories (--repo, repeat for more).
    Grant {
        /// The authorization's id.
        id: String,
        /// The namespace's name.
        namespace: String,
        /// Every repository of the namespace.
        #[arg(long, conflicts_with = "repo")]
        all: bool,
        /// A repository's name without the namespace (repeat for more).
        #[arg(long, value_name = "NAME")]
        repo: Vec<String>,
    },
    /// Take a namespace back: the named repositories (--repo), or the whole namespace when none is named.
    Ungrant {
        /// The authorization's id.
        id: String,
        /// The namespace's name.
        namespace: String,
        /// A repository's name without the namespace (repeat for more).
        #[arg(long, value_name = "NAME")]
        repo: Vec<String>,
    },
    /// Connect a granted repository to a project: the project's landed work exports to it from now on.
    Connect {
        /// The authorization's id.
        id: String,
        /// The repository on the platform, `owner/repo`.
        repo: String,
        /// The project it joins.
        #[arg(long)]
        project: String,
        /// Do not register the plaza's signing key on your account (exported commits then show as unverified).
        #[arg(long)]
        no_sign: bool,
        /// Push what Weft has now, not at the next landing.
        #[arg(long)]
        export: bool,
    },
    /// Stop a project's repository exporting to a platform repository (see `repos` for the two names).
    Disconnect {
        /// The authorization's id.
        id: String,
        /// The project's Weft repository, such as `pr1-notes`.
        #[arg(long)]
        weft_repo: String,
        /// The connection's id, such as `github`.
        #[arg(long)]
        connection: String,
    },
    /// Read the authorization's account again (and an app's installations).
    Sync {
        /// The authorization's id.
        id: String,
    },
    /// Remove an authorization and its tokens. Asks first on a terminal unless --yes.
    Remove {
        /// The authorization's id.
        id: String,
        /// Remove without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Forget a registered OAuth app and its client secret. Asks first on a terminal unless --yes.
    Unregister {
        /// The registration's id, such as `gitlab-app-4`.
        registration: String,
        /// Forget it without asking.
        #[arg(long)]
        yes: bool,
    },
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List => list(ctx),
        Cmd::GithubApp { host, org, name } => {
            let mut body = json!({});
            if let Some(host) = host {
                body["host"] = json!(host);
            }
            if let Some(org) = org {
                body["org"] = json!(org);
            }
            if let Some(name) = name {
                body["name"] = json!(name);
            }
            start_and_wait(ctx, "/v1/platforms/github/app", &body)
        }
        Cmd::Install { registration } => start_and_wait(
            ctx,
            "/v1/platforms/github/install",
            &json!({ "registration": registration }),
        ),
        Cmd::Register {
            kind,
            client_id,
            host,
        } => register(ctx, &kind, &client_id, host),
        Cmd::SignIn { registration } => start_and_wait(
            ctx,
            "/v1/platforms/authorize",
            &json!({ "registration": registration }),
        ),
        Cmd::Token {
            kind,
            host,
            level,
            target,
        } => token(ctx, &kind, host, &level, target),
        Cmd::Namespaces { id } => namespaces(ctx, &id),
        Cmd::Repos { id, namespace } => repos(ctx, &id, &namespace),
        Cmd::Grant {
            id,
            namespace,
            all,
            repo,
        } => grant(ctx, &id, &namespace, all, &repo),
        Cmd::Ungrant {
            id,
            namespace,
            repo,
        } => ungrant(ctx, &id, &namespace, &repo),
        Cmd::Connect {
            id,
            repo,
            project,
            no_sign,
            export,
        } => connect(ctx, &id, &repo, &project, !no_sign, export),
        Cmd::Disconnect {
            id,
            weft_repo,
            connection,
        } => disconnect(ctx, &id, &weft_repo, &connection),
        Cmd::Sync { id } => sync(ctx, &id),
        Cmd::Remove { id, yes } => remove(ctx, &id, yes),
        Cmd::Unregister { registration, yes } => unregister(ctx, &registration, yes),
    }
}

// ---- Listing ----------------------------------------------------------------------------------

/// `platforms list`: the state, then the apps and authorizations of each host.
fn list(ctx: &Ctx) -> Result<(), String> {
    let platforms = ctx.client.get("/v1/platforms")?;
    // The connections come from Weft, for the state line only; `--json` prints the hub's answer as it is.
    let connected = if ctx.json {
        None
    } else {
        ctx.client
            .get("/v1/weft/repos")
            .ok()
            .map(|weft| connections(&weft))
    };
    ctx.out(&platforms, |p| {
        println!("{}", state_line(p, connected));
        for (kind, host) in hosts(p) {
            println!();
            println!("{} ({host})", kind_label(p, &kind));
            let apps = registration_rows(field(p, "registrations"), &host);
            if !apps.is_empty() {
                println!("Apps");
                ctx::table(&["id", "app", "client id"], &apps);
            }
            let auths = authorization_rows(field(p, "integrations"), &host);
            if !auths.is_empty() {
                println!("Authorizations");
                ctx::table(
                    &[
                        "id",
                        "level",
                        "via",
                        "account",
                        "identity",
                        "repositories",
                        "error",
                    ],
                    &auths,
                );
            }
        }
    });
    Ok(())
}

/// The state in one line: what is authorized, how many repositories are connected, and what needs the owner.
fn state_line(p: &Value, connected: Option<usize>) -> String {
    let auths = field(p, "integrations");
    if auths.is_empty() {
        return "Nothing authorized yet: add GitHub, GitLab or Forgejo.".to_string();
    }
    let mut parts = vec![plural(auths.len(), "authorization", "authorizations")];
    if let Some(n) = connected {
        parts.push(format!(
            "{} connected",
            plural(n, "repository", "repositories")
        ));
    }
    let attention = auths
        .iter()
        .filter(|a| !ctx::s(a, "error").is_empty())
        .count();
    if attention > 0 {
        parts.push(format!(
            "{attention} {} attention",
            if attention == 1 { "needs" } else { "need" }
        ));
    }
    format!("{}.", parts.join(". "))
}

/// The number of Weft connections to a git platform, over the repositories of `GET /v1/weft/repos`.
fn connections(weft: &Value) -> usize {
    field(weft, "repos")
        .iter()
        .flat_map(|r| field(r, "connections"))
        .filter(|c| KINDS.contains(&ctx::s(c, "platform")))
        .count()
}

/// The (kind, host) pairs of the apps and authorizations, each host once, in the order they are listed.
fn hosts(p: &Value) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for item in field(p, "registrations")
        .iter()
        .chain(field(p, "integrations"))
    {
        let host = ctx::s(item, "host");
        if !out.iter().any(|(_, seen)| seen == host) {
            out.push((ctx::s(item, "kind").to_string(), host.to_string()));
        }
    }
    out
}

/// A platform's name as the hub gives it (`GitHub`), or its kind when the hub does not list it.
fn kind_label(p: &Value, kind: &str) -> String {
    field(p, "kinds")
        .iter()
        .find(|k| ctx::s(k, "kind") == kind)
        .map_or_else(|| kind.to_string(), |k| ctx::s(k, "label").to_string())
}

/// The apps registered on `host`: id, what the app is, and its client ID (which is not secret).
fn registration_rows(regs: &[Value], host: &str) -> Vec<Vec<String>> {
    regs.iter()
        .filter(|r| ctx::s(r, "host") == host)
        .map(|r| {
            let app = &r["app"];
            let what = if app.is_object() {
                format!("GitHub App {}", ctx::s(app, "slug"))
            } else {
                "OAuth app".to_string()
            };
            vec![
                ctx::s(r, "id").to_string(),
                what,
                ctx::s(r, "client_id").to_string(),
            ]
        })
        .collect()
}

/// The authorizations on `host` as table rows: id, level, how it was given, the account or target it is for, the
/// identity its exports are authored by, what the plaza may use, and the warning.
fn authorization_rows(auths: &[Value], host: &str) -> Vec<Vec<String>> {
    auths
        .iter()
        .filter(|a| ctx::s(a, "host") == host)
        .map(|a| {
            let target = ctx::s(a, "target");
            let account = if target.is_empty() {
                ctx::s(&a["account"], "login")
            } else {
                target
            };
            vec![
                ctx::s(a, "id").to_string(),
                level_label(ctx::s(a, "kind"), ctx::s(a, "level")),
                via_label(ctx::s(a, "via")),
                dash(account),
                dash(ctx::s(&a["identity"], "login")),
                selection(&a["grants"]),
                ctx::s(a, "error").to_string(),
            ]
        })
        .collect()
}

/// A level in the owner's words: "Your account", "Organisation" (a group on GitLab), "One repository".
fn level_label(kind: &str, level: &str) -> String {
    let label = match (level, kind) {
        ("user", _) => "Your account",
        ("org", "gitlab") => "Group",
        ("org", _) => "Organisation",
        ("repo", _) => "One repository",
        _ => level,
    };
    label.to_string()
}

/// How an authorization was given, in a word.
fn via_label(via: &str) -> String {
    let label = match via {
        "app" => "App",
        "oauth" => "Sign-in",
        "token" => "Token",
        other => other,
    };
    label.to_string()
}

/// What the plaza may use under an authorization: `JumpHash (all), irfan (notes, web)`.
fn selection(grants: &Value) -> String {
    let parts: Vec<String> = grants["namespaces"]
        .as_object()
        .into_iter()
        .flat_map(|m| m.iter())
        .filter_map(|(ns, g)| {
            if is_true(&g["all"]) {
                Some(format!("{ns} (all)"))
            } else {
                let repos = strings(&g["repos"]);
                (!repos.is_empty()).then(|| format!("{ns} ({})", repos.join(", ")))
            }
        })
        .collect();
    if parts.is_empty() {
        "none chosen".to_string()
    } else {
        parts.join(", ")
    }
}

/// `platforms namespaces`: what the authorization reaches, each with its grant.
fn namespaces(ctx: &Ctx, id: &str) -> Result<(), String> {
    let list = ctx
        .client
        .get(&format!("/v1/platforms/{}/namespaces", ctx::seg(id)))?;
    ctx.out(&list, |l| {
        ctx::table(
            &["name", "kind", "display", "granted"],
            &namespace_rows(items(l)),
        )
    });
    Ok(())
}

fn namespace_rows(list: &[Value]) -> Vec<Vec<String>> {
    list.iter()
        .map(|n| {
            vec![
                ctx::s(n, "name").to_string(),
                ctx::s(n, "kind").to_string(),
                ctx::s(n, "display").to_string(),
                grant_text(&n["grant"]),
            ]
        })
        .collect()
}

/// A namespace's grant: `✓ all`, `✓ notes, web`, or nothing.
fn grant_text(grant: &Value) -> String {
    if is_true(&grant["all"]) {
        return "✓ all".to_string();
    }
    let repos = strings(&grant["repos"]);
    if repos.is_empty() {
        String::new()
    } else {
        format!("✓ {}", repos.join(", "))
    }
}

/// `platforms repos`: one namespace's repositories, with the grant, visibility, push access and connections.
fn repos(ctx: &Ctx, id: &str, namespace: &str) -> Result<(), String> {
    let list = repo_list(ctx, id, namespace)?;
    ctx.out(&list, |l| {
        ctx::table(
            &["granted", "repository", "visibility", "push", "connections"],
            &repo_rows(items(l)),
        )
    });
    Ok(())
}

/// The repositories of a namespace as the hub lists them now.
fn repo_list(ctx: &Ctx, id: &str, namespace: &str) -> Result<Value, String> {
    ctx.client.get(&format!(
        "/v1/platforms/{}/repos?namespace={}",
        ctx::seg(id),
        ctx::seg(namespace)
    ))
}

fn repo_rows(repos: &[Value]) -> Vec<Vec<String>> {
    repos
        .iter()
        .map(|r| {
            vec![
                word(is_true(&r["granted"]), "✓", ""),
                ctx::s(r, "full_name").to_string(),
                word(is_true(&r["private"]), "private", "public"),
                word(is_true(&r["can_push"]), "yes", "can't push"),
                connections_text(&r["connections"]),
            ]
        })
        .collect()
}

/// A repository's Weft connections as `pr1-notes/github (in-step)`: the disconnect's two names, and the state.
fn connections_text(connections: &Value) -> String {
    items(connections)
        .iter()
        .map(|c| {
            format!(
                "{}/{} ({})",
                ctx::s(c, "weft_repo"),
                ctx::s(c, "connection"),
                ctx::s(c, "state")
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// ---- Authorizing ------------------------------------------------------------------------------

/// Starts a flow on the hub, opens its page in the browser (it is printed too), and waits for the browser to come
/// back. The outcome is printed when the flow is over (with its `state` and `go` under `--json`).
fn start_and_wait(ctx: &Ctx, path: &str, body: &Value) -> Result<(), String> {
    let started = ctx.client.post(path, body)?;
    let state = ctx::s(&started, "state").to_string();
    let go = ctx::s(&started, "go").to_string();
    let opened = open_in_browser(&go);
    say(
        ctx,
        &if opened {
            format!("Opened the platform's page in your browser: {go}")
        } else {
            format!("Open this page in your browser to continue: {go}")
        },
    );
    say(
        ctx,
        "Waiting for the browser to come back (up to 15 minutes).",
    );
    let mut outcome = wait_for(ctx, &state)?;
    outcome["state"] = json!(state);
    outcome["go"] = json!(go);
    ctx.out(&outcome, |o| println!("{}", done_line(o)));
    Ok(())
}

/// Opens `url` in the default browser (`open` on macOS, `xdg-open` on Linux, `start` on Windows); false when there is
/// none to ask, as on a server over SSH.
fn open_in_browser(url: &str) -> bool {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return false;
    }
    let mut cmd = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    cmd.arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Polls the flow every two seconds until it is over, for at most fifteen minutes.
fn wait_for(ctx: &Ctx, state: &str) -> Result<Value, String> {
    let path = format!("/v1/platforms/flows/{}", ctx::seg(state));
    let started = Instant::now();
    loop {
        let status = ctx.client.get(&path)?;
        if let Some(outcome) = flow_over(&status)? {
            return Ok(outcome);
        }
        if started.elapsed() >= WAIT {
            return Err("the browser did not come back within 15 minutes: start it again".into());
        }
        std::thread::sleep(POLL);
    }
}

/// Whether a polled flow is over: `None` while the browser has not come back. Done gives its outcome; failed and
/// expired are errors, in the hub's words.
fn flow_over(status: &Value) -> Result<Option<Value>, String> {
    match ctx::s(status, "status") {
        "pending" => Ok(None),
        "done" => Ok(Some(status.clone())),
        "failed" => Err(format!(
            "the authorization failed: {}",
            ctx::s(status, "error")
        )),
        "expired" => Err("the browser did not come back in time: start it again".into()),
        other => Err(format!(
            "the flow is {other:?}, which this CLI does not know"
        )),
    }
}

/// A finished flow in words: the authorizations it made or updated.
fn done_line(outcome: &Value) -> String {
    let ids = strings(&outcome["detail"]["integrations"]);
    if ids.is_empty() {
        "Done. See `agentplaza platforms list`.".to_string()
    } else {
        format!(
            "Done: authorization {} made or updated. See `agentplaza platforms list`.",
            ids.join(", ")
        )
    }
}

/// `platforms register`: an OAuth app the owner registered. The callback URL is printed first, to paste into the
/// app's settings; the client secret is read without echo and never printed back.
fn register(ctx: &Ctx, kind: &str, client_id: &str, host: Option<String>) -> Result<(), String> {
    let platforms = ctx.client.get("/v1/platforms")?;
    say(
        ctx,
        &format!(
            "Paste this callback URL into the app's settings: {}",
            ctx::s(&platforms, "callback")
        ),
    );
    if kind == "gitlab" {
        say(ctx, "On GitLab, tick the scopes api and read_user.");
    }
    let client_secret = secret("Client secret: ")?;
    let mut body = json!({ "kind": kind, "client_id": client_id, "client_secret": client_secret });
    if let Some(host) = host {
        body["host"] = json!(host);
    }
    let reg = ctx.client.post("/v1/platforms/registrations", &body)?;
    ctx.out(&reg, |r| {
        let id = ctx::s(r, "id");
        println!(
            "Registered {id} for {}. Sign in with `agentplaza platforms sign-in {id}`.",
            ctx::s(r, "host")
        )
    });
    Ok(())
}

/// `platforms token`: an authorization by a token the owner made. The token is read without echo and sent once.
fn token(
    ctx: &Ctx,
    kind: &str,
    host: Option<String>,
    level: &str,
    target: Option<String>,
) -> Result<(), String> {
    let token = secret("Token: ")?;
    let mut body = json!({ "kind": kind, "token": token, "level": level });
    if let Some(host) = host {
        body["host"] = json!(host);
    }
    if let Some(target) = target {
        body["target"] = json!(target);
    }
    let auth = ctx.client.post("/v1/platforms/tokens", &body)?;
    ctx.out(&auth, |a| {
        let id = ctx::s(a, "id");
        println!(
            "Authorized {id} with a token. Choose what the plaza may use with `agentplaza platforms grant {id} NAMESPACE`."
        )
    });
    Ok(())
}

// ---- Grants -----------------------------------------------------------------------------------

/// `platforms grant`: the namespace's repositories (all, or the named ones added to its grant). The whole map is
/// sent back, as the hub replaces it.
fn grant(ctx: &Ctx, id: &str, namespace: &str, all: bool, repos: &[String]) -> Result<(), String> {
    if !all && repos.is_empty() {
        return Err("give --all, or --repo NAME for each repository to grant".into());
    }
    let auth = integration(ctx, id)?;
    let next = with_grant(&auth["grants"], namespace, all, repos);
    let saved = save_grants(ctx, id, &next)?;
    let what = if all {
        format!("all of {namespace}")
    } else {
        format!("{} in {namespace}", repos.join(", "))
    };
    ctx.out(&saved, |_| println!("The plaza may use {what}."));
    Ok(())
}

/// `platforms ungrant`: the named repositories are taken out of the namespace, or the namespace is taken out whole.
fn ungrant(ctx: &Ctx, id: &str, namespace: &str, repos: &[String]) -> Result<(), String> {
    let auth = integration(ctx, id)?;
    let grants = &auth["grants"];
    let next = if repos.is_empty() {
        without_namespace(grants, namespace)
    } else {
        // A namespace granted whole is first narrowed to its repositories, less the ones named.
        let whole = grant_of(&namespaces_of(grants), namespace).0;
        let all_repos: Vec<String> = if whole {
            repo_list(ctx, id, namespace)?
                .as_array()
                .into_iter()
                .flatten()
                .map(|r| ctx::s(r, "name").to_string())
                .collect()
        } else {
            Vec::new()
        };
        without_repos(grants, namespace, repos, &all_repos)
    };
    let saved = save_grants(ctx, id, &next)?;
    let what = if repos.is_empty() {
        namespace.to_string()
    } else {
        format!("{} in {namespace}", repos.join(", "))
    };
    ctx.out(&saved, |_| println!("The plaza may no longer use {what}."));
    Ok(())
}

/// The grants map with `namespace` granted: all of it (`all`), or the named repositories added to what it had.
fn with_grant(grants: &Value, namespace: &str, all: bool, repos: &[String]) -> Value {
    let mut namespaces = namespaces_of(grants);
    let entry = if all {
        json!({ "all": true, "repos": [] })
    } else {
        let (whole, mut named) = grant_of(&namespaces, namespace);
        named.extend(repos.iter().cloned());
        json!({ "all": whole, "repos": named })
    };
    namespaces.insert(namespace.to_string(), entry);
    json!({ "namespaces": namespaces })
}

/// The grants map with the named repositories taken out of `namespace`. A namespace granted whole is first narrowed
/// to `whole_repos` (its repositories as the platform lists them), less the ones named. A namespace left with nothing
/// is dropped from the map.
fn without_repos(
    grants: &Value,
    namespace: &str,
    remove: &[String],
    whole_repos: &[String],
) -> Value {
    let mut namespaces = namespaces_of(grants);
    let (whole, named) = grant_of(&namespaces, namespace);
    let mut left: BTreeSet<String> = if whole {
        whole_repos.iter().cloned().collect()
    } else {
        named
    };
    for repo in remove {
        left.remove(repo);
    }
    if left.is_empty() {
        namespaces.remove(namespace);
    } else {
        namespaces.insert(
            namespace.to_string(),
            json!({ "all": false, "repos": left }),
        );
    }
    json!({ "namespaces": namespaces })
}

/// The grants map without `namespace`.
fn without_namespace(grants: &Value, namespace: &str) -> Value {
    let mut namespaces = namespaces_of(grants);
    namespaces.remove(namespace);
    json!({ "namespaces": namespaces })
}

/// The namespaces of a grants value (`{namespaces: {...}}`), as a map to change.
fn namespaces_of(grants: &Value) -> Map<String, Value> {
    grants["namespaces"]
        .as_object()
        .cloned()
        .unwrap_or_default()
}

/// Whether a namespace is granted whole, and the repositories it is granted by name.
fn grant_of(namespaces: &Map<String, Value>, namespace: &str) -> (bool, BTreeSet<String>) {
    let null = Value::Null;
    let grant = namespaces.get(namespace).unwrap_or(&null);
    (
        is_true(&grant["all"]),
        strings(&grant["repos"]).into_iter().collect(),
    )
}

/// The one authorization from the hub's list, by its id.
fn integration(ctx: &Ctx, id: &str) -> Result<Value, String> {
    let platforms = ctx.client.get("/v1/platforms")?;
    field(&platforms, "integrations")
        .iter()
        .find(|a| ctx::s(a, "id") == id)
        .cloned()
        .ok_or_else(|| format!("no authorization {id}: `agentplaza platforms list` shows them"))
}

fn save_grants(ctx: &Ctx, id: &str, grants: &Value) -> Result<Value, String> {
    ctx.client
        .put(&format!("/v1/platforms/{}/grants", ctx::seg(id)), grants)
}

// ---- Connecting -------------------------------------------------------------------------------

/// `platforms connect`: a granted repository joins a project.
fn connect(
    ctx: &Ctx,
    id: &str,
    repo: &str,
    project: &str,
    sign: bool,
    export: bool,
) -> Result<(), String> {
    let body = json!({ "repo": repo, "project": project, "sign": sign, "export": export });
    let out = ctx
        .client
        .post(&format!("/v1/platforms/{}/connect", ctx::seg(id)), &body)?;
    ctx.out(&out, |o| {
        for line in connected_lines(repo, project, o) {
            println!("{line}");
        }
    });
    Ok(())
}

/// The connect answer in plain words: what now exports where, what the import brought, and the signing key.
fn connected_lines(repo: &str, project: &str, out: &Value) -> Vec<String> {
    let mut lines = vec![format!(
        "Connected {repo} to {project}: landed work exports to {repo} on {}, as {} ({}, connection {}).",
        ctx::s(out, "branch"),
        ctx::s(out, "author"),
        ctx::s(out, "weft_repo"),
        ctx::s(out, "connection"),
    )];
    let imported = &out["imported"];
    if let Some(error) = imported["error"].as_str() {
        lines.push(format!("The history was not imported: {error}."));
    } else if is_true(&imported["empty"]) {
        lines.push(
            "The repository is empty: the first export pushes the project's history.".to_string(),
        );
    } else if let Some(commits) = imported["commits"].as_u64() {
        let commits = usize::try_from(commits).unwrap_or(usize::MAX);
        let mut text = format!("Imported {}", plural(commits, "commit", "commits"));
        let releases = strings(&imported["releases"]);
        if !releases.is_empty() {
            text.push_str(&format!(
                " and {}: {}",
                plural(releases.len(), "release", "releases"),
                releases.join(", ")
            ));
        }
        lines.push(format!("{text}."));
    }
    if out["signing_key"].as_str() == Some("registered") {
        lines.push(
            "The plaza's signing key is registered on your account: exported commits show as verified."
                .to_string(),
        );
    } else if !out["signing_key"].is_null() {
        lines.push(format!(
            "The signing key was not registered: {}.",
            ctx::s(&out["signing_key"], "error")
        ));
    }
    lines
}

/// `platforms disconnect`: a project's repository stops exporting to the platform repository.
fn disconnect(ctx: &Ctx, id: &str, weft_repo: &str, connection: &str) -> Result<(), String> {
    let body = json!({ "weft_repo": weft_repo, "connection": connection });
    let out = ctx
        .client
        .post(&format!("/v1/platforms/{}/disconnect", ctx::seg(id)), &body)?;
    ctx.out(&out, |_| {
        println!("Disconnected {connection} in {weft_repo}: it no longer exports there.")
    });
    Ok(())
}

/// `platforms sync`: the authorization's account is read again.
fn sync(ctx: &Ctx, id: &str) -> Result<(), String> {
    let auth = ctx
        .client
        .post(&format!("/v1/platforms/{}/sync", ctx::seg(id)), &json!({}))?;
    ctx.out(&auth, |a| {
        let error = ctx::s(a, "error");
        if error.is_empty() {
            println!("Synced {id}: no problems.")
        } else {
            println!("Synced {id}: {error}")
        }
    });
    Ok(())
}

/// `platforms remove`: an authorization and its tokens are forgotten.
fn remove(ctx: &Ctx, id: &str, yes: bool) -> Result<(), String> {
    if !confirm(
        yes,
        &format!("Remove the authorization {id} and its tokens."),
    )? {
        return Err("cancelled".into());
    }
    let gone = ctx
        .client
        .delete(&format!("/v1/platforms/{}", ctx::seg(id)))?;
    ctx.out(&gone, |_| println!("Removed {id}."));
    Ok(())
}

/// `platforms unregister`: a registered OAuth app and its client secret are forgotten.
fn unregister(ctx: &Ctx, registration: &str, yes: bool) -> Result<(), String> {
    if !confirm(
        yes,
        &format!("Forget the app {registration} and its client secret."),
    )? {
        return Err("cancelled".into());
    }
    let gone = ctx.client.delete(&format!(
        "/v1/platforms/registrations/{}",
        ctx::seg(registration)
    ))?;
    ctx.out(&gone, |_| println!("Forgot the app {registration}."));
    Ok(())
}

// ---- Helpers ----------------------------------------------------------------------------------

/// A line for the person at the terminal: on stderr, and not at all with `--json` (stdout is then the JSON alone).
fn say(ctx: &Ctx, line: &str) {
    if !ctx.json {
        eprintln!("{line}");
    }
}

/// A secret from the terminal, with the echo off (the prompt is shown there); from stdin, one line and no prompt,
/// when stdin is not a terminal. Never an argument, never printed back.
fn secret(prompt: &str) -> Result<String, String> {
    let line = if std::io::stdin().is_terminal() {
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
        hidden_line()?
    } else {
        let mut buf = String::new();
        std::io::stdin()
            .read_line(&mut buf)
            .map_err(|e| format!("reading stdin: {e}"))?;
        buf
    };
    let value = line.trim().to_string();
    if value.is_empty() {
        return Err("nothing was entered".into());
    }
    Ok(value)
}

/// One line from the terminal with the echo turned off; the settings are put back afterwards.
#[cfg(unix)]
fn hidden_line() -> Result<String, String> {
    let fd = libc::STDIN_FILENO;
    // SAFETY: termios is plain old data, and tcgetattr fills it in before it is read.
    let mut saved: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: fd is the terminal on stdin and `saved` is a valid out-parameter.
    if unsafe { libc::tcgetattr(fd, &mut saved) } != 0 {
        return Err("cannot read the terminal settings".into());
    }
    let mut quiet = saved;
    quiet.c_lflag &= !libc::ECHO;
    // SAFETY: as above; `quiet` is a copy of the settings just read, with echo off.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &quiet) } != 0 {
        return Err("cannot turn off the echo".into());
    }
    let mut line = String::new();
    let read = std::io::stdin().read_line(&mut line);
    // SAFETY: restores the settings read above.
    unsafe {
        libc::tcsetattr(fd, libc::TCSANOW, &saved);
    }
    eprintln!();
    read.map_err(|e| format!("reading the prompt: {e}"))?;
    Ok(line)
}

#[cfg(not(unix))]
fn hidden_line() -> Result<String, String> {
    Err("a hidden prompt needs a Unix terminal here: pipe the value on stdin".into())
}

/// The elements of a JSON array (empty when it is not one).
fn items(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}

/// The elements of the JSON array under `key` (empty when there is none).
fn field<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    items(&v[key])
}

/// The strings of a JSON array (empty when it is not one).
fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// Whether a JSON value is `true`.
fn is_true(v: &Value) -> bool {
    v.as_bool() == Some(true)
}

/// `1 repository`, `2 repositories`.
fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// `yes` or `no` by a flag.
fn word(on: bool, yes: &str, no: &str) -> String {
    let text = if on { yes } else { no };
    text.to_string()
}

/// Text, or a dash where there is none.
fn dash(text: &str) -> String {
    if text.is_empty() {
        "—".to_string()
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// The hub's `GET /v1/platforms` as the tests need it: one GitHub App and an installation of it, one GitLab
    /// sign-in whose session has lapsed.
    fn platforms() -> Value {
        json!({
            "kinds": [
                { "kind": "github", "label": "GitHub", "default_host": "https://github.com" },
                { "kind": "gitlab", "label": "GitLab", "default_host": "https://gitlab.com" },
                { "kind": "forgejo", "label": "Forgejo", "default_host": "https://codeberg.org" }
            ],
            "registrations": [
                { "id": "github-app-1", "kind": "github", "host": "https://github.com", "client_id": "Iv23li",
                  "app": { "id": 42, "slug": "agentplaza-3f9c2a", "name": "AgentPlaza 3f9c2a", "owner": "irfan", "html_url": "https://github.com/apps/agentplaza-3f9c2a" },
                  "created_ms": 1 },
                { "id": "gitlab-app-4", "kind": "gitlab", "host": "https://gitlab.com", "client_id": "abc", "app": null, "created_ms": 2 }
            ],
            "integrations": [
                { "id": "github-2", "kind": "github", "host": "https://github.com", "via": "app", "level": "org",
                  "registration": "github-app-1", "installation": 7, "repository_selection": "selected",
                  "target": "JumpHash", "account": { "id": 9, "login": "JumpHash", "name": "JumpHash", "email": "", "avatar_url": "" },
                  "grants": { "namespaces": { "JumpHash": { "all": true, "repos": [] } } },
                  "error": null, "label": "GitHub",
                  "identity": { "id": 5, "login": "irfan", "name": "Irfan S", "email": "irfan@x.test", "avatar_url": "" } },
                { "id": "gitlab-5", "kind": "gitlab", "host": "https://gitlab.com", "via": "oauth", "level": "user",
                  "registration": "gitlab-app-4", "account": { "login": "irfan" }, "grants": { "namespaces": {} },
                  "error": "the sign-in expired", "identity": { "login": "irfan" } }
            ],
            "callback": "http://127.0.0.1:7700/v1/platforms/callback"
        })
    }

    #[test]
    fn the_state_line_counts_authorizations_connections_and_warnings() {
        let p = platforms();
        assert_eq!(
            state_line(&p, Some(1)),
            "2 authorizations. 1 repository connected. 1 needs attention."
        );
        assert_eq!(state_line(&p, None), "2 authorizations. 1 needs attention.");
        let empty = json!({ "integrations": [] });
        assert_eq!(
            state_line(&empty, Some(0)),
            "Nothing authorized yet: add GitHub, GitLab or Forgejo."
        );
    }

    #[test]
    fn apps_and_authorizations_list_under_their_host() {
        let p = platforms();
        assert_eq!(
            hosts(&p),
            [
                ("github".to_string(), "https://github.com".to_string()),
                ("gitlab".to_string(), "https://gitlab.com".to_string()),
            ]
        );
        assert_eq!(kind_label(&p, "gitlab"), "GitLab");
        assert_eq!(
            registration_rows(field(&p, "registrations"), "https://github.com"),
            [vec![
                "github-app-1",
                "GitHub App agentplaza-3f9c2a",
                "Iv23li"
            ]]
        );
        assert_eq!(
            authorization_rows(field(&p, "integrations"), "https://github.com"),
            [vec![
                "github-2",
                "Organisation",
                "App",
                "JumpHash",
                "irfan",
                "JumpHash (all)",
                "",
            ]]
        );
        assert_eq!(
            authorization_rows(field(&p, "integrations"), "https://gitlab.com"),
            [vec![
                "gitlab-5",
                "Your account",
                "Sign-in",
                "irfan",
                "irfan",
                "none chosen",
                "the sign-in expired",
            ]]
        );
    }

    #[test]
    fn repositories_are_marked_granted_private_and_connected() {
        let repos = json!([
            { "full_name": "irfan/notes", "namespace": "irfan", "name": "notes", "private": false, "can_push": true,
              "granted": true, "connections": [{ "weft_repo": "pr1-notes", "project": "pr1", "connection": "github", "state": "in-step" }] },
            { "full_name": "irfan/web", "namespace": "irfan", "name": "web", "private": true, "can_push": false,
              "granted": false, "connections": [] }
        ]);
        assert_eq!(
            repo_rows(items(&repos)),
            [
                vec![
                    "✓",
                    "irfan/notes",
                    "public",
                    "yes",
                    "pr1-notes/github (in-step)"
                ],
                vec!["", "irfan/web", "private", "can't push", ""],
            ]
        );
        let namespaces = json!([
            { "name": "irfan", "kind": "user", "display": "Irfan S", "id": 5, "grant": { "all": false, "repos": ["notes"] } },
            { "name": "JumpHash", "kind": "org", "display": "JumpHash", "id": 9, "grant": null }
        ]);
        assert_eq!(
            namespace_rows(items(&namespaces)),
            [
                vec!["irfan", "user", "Irfan S", "✓ notes"],
                vec!["JumpHash", "org", "JumpHash", ""],
            ]
        );
    }

    #[test]
    fn a_grant_adds_a_namespace_or_its_repositories() {
        let grants = json!({ "namespaces": { "JumpHash": { "all": true, "repos": [] } } });
        assert_eq!(
            with_grant(&grants, "irfan", false, &["notes".to_string()]),
            json!({ "namespaces": {
                "JumpHash": { "all": true, "repos": [] },
                "irfan": { "all": false, "repos": ["notes"] }
            } })
        );
        let then = with_grant(&grants, "irfan", false, &["notes".to_string()]);
        assert_eq!(
            with_grant(&then, "irfan", false, &["web".to_string()])["namespaces"]["irfan"],
            json!({ "all": false, "repos": ["notes", "web"] })
        );
        assert_eq!(
            with_grant(&then, "irfan", true, &[])["namespaces"]["irfan"],
            json!({ "all": true, "repos": [] })
        );
    }

    #[test]
    fn an_ungrant_takes_repositories_or_the_whole_namespace_out() {
        let grants =
            json!({ "namespaces": { "irfan": { "all": false, "repos": ["notes", "web"] } } });
        assert_eq!(
            without_repos(&grants, "irfan", &["web".to_string()], &[]),
            json!({ "namespaces": { "irfan": { "all": false, "repos": ["notes"] } } })
        );
        assert_eq!(
            without_repos(
                &grants,
                "irfan",
                &["notes".to_string(), "web".to_string()],
                &[]
            ),
            json!({ "namespaces": {} }),
            "a namespace left with nothing is dropped"
        );
        assert_eq!(
            without_namespace(&grants, "irfan"),
            json!({ "namespaces": {} })
        );
        // A namespace granted whole is narrowed to its repositories, less the one named.
        let whole = json!({ "namespaces": { "JumpHash": { "all": true, "repos": [] } } });
        let names = ["notes".to_string(), "web".to_string()];
        assert_eq!(
            without_repos(&whole, "JumpHash", &["web".to_string()], &names),
            json!({ "namespaces": { "JumpHash": { "all": false, "repos": ["notes"] } } })
        );
    }

    #[test]
    fn a_connect_answer_reads_in_plain_words() {
        let out = json!({
            "weft_repo": "pr1-notes", "connection": "github", "remote": "https://github.com/irfan/notes.git",
            "branch": "main", "author": "Irfan S <irfan@x.test>",
            "imported": { "commits": 12, "releases": ["v0.1.0"] }, "signing_key": "registered"
        });
        assert_eq!(
            connected_lines("irfan/notes", "pr1", &out),
            [
                "Connected irfan/notes to pr1: landed work exports to irfan/notes on main, as Irfan S <irfan@x.test> (pr1-notes, connection github).",
                "Imported 12 commits and 1 release: v0.1.0.",
                "The plaza's signing key is registered on your account: exported commits show as verified.",
            ]
        );
        let empty = json!({
            "weft_repo": "pr1-notes", "connection": "github", "branch": "main", "author": "Irfan S <irfan@x.test>",
            "imported": { "commits": 0, "empty": true }
        });
        assert_eq!(
            connected_lines("irfan/notes", "pr1", &empty)[1],
            "The repository is empty: the first export pushes the project's history."
        );
        let failed = json!({
            "weft_repo": "pr1-notes", "connection": "github", "branch": "main", "author": "Irfan S <irfan@x.test>",
            "imported": { "error": "no branch" }, "signing_key": { "error": "Forbidden" }
        });
        assert_eq!(
            connected_lines("irfan/notes", "pr1", &failed)[1..],
            [
                "The history was not imported: no branch.".to_string(),
                "The signing key was not registered: Forbidden.".to_string(),
            ]
        );
    }

    #[test]
    fn a_flow_is_waited_on_until_it_is_done_failed_or_expired() {
        assert_eq!(flow_over(&json!({ "status": "pending" })).unwrap(), None);
        let done = json!({ "status": "done", "detail": { "integrations": ["github-2"] } });
        assert_eq!(flow_over(&done).unwrap(), Some(done.clone()));
        assert_eq!(
            flow_over(&json!({ "status": "failed", "error": "the code was used" })).unwrap_err(),
            "the authorization failed: the code was used"
        );
        assert!(
            flow_over(&json!({ "status": "expired" }))
                .unwrap_err()
                .contains("in time")
        );
        assert_eq!(
            done_line(&done),
            "Done: authorization github-2 made or updated. See `agentplaza platforms list`."
        );
    }

    #[derive(Parser, Debug)]
    struct Wrapped {
        #[command(subcommand)]
        cmd: Cmd,
    }

    fn parse(args: &[&str]) -> Result<Cmd, clap::Error> {
        Wrapped::try_parse_from(std::iter::once("platforms").chain(args.iter().copied()))
            .map(|w| w.cmd)
    }

    #[test]
    fn the_grant_and_connect_flags_parse_as_their_help_says() {
        assert!(parse(&["grant", "github-2", "JumpHash", "--all"]).is_ok());
        assert!(
            parse(&[
                "grant", "github-2", "irfan", "--repo", "notes", "--repo", "web"
            ])
            .is_ok()
        );
        assert!(parse(&["grant", "github-2", "irfan", "--all", "--repo", "notes"]).is_err());
        assert!(
            parse(&["repos", "github-2"]).is_err(),
            "repos needs --namespace"
        );
        assert!(
            parse(&[
                "connect",
                "github-2",
                "irfan/notes",
                "--project",
                "pr1",
                "--no-sign",
                "--export"
            ])
            .is_ok()
        );
        assert!(parse(&["register", "bitbucket", "--client-id", "x"]).is_err());
    }
}
