//! Weft in the hub: the plaza's version control (`crates/weft`).
//!
//! apid starts `weft serve` beside it (the `weft` binary next to apid's own; in-process when there is none) and
//! hands it the hub's signing keys from the vault. It is Weft's only door:
//!
//! - the owner's `/v1/weft/…` (behind the hub's front door), and their console's rulings, which the hub signs for
//!   the owner (`rule-for-owner`, marked as from the console);
//! - agents' `…/gateway/weft/…` from their VMs: the agent's statements are signed with its own key from the vault,
//!   inside its own call, so a key never leaves the hub and the hub witnesses every signature;
//! - node caches' misses, `/internal/v1/nodes/{id}/weft/…`, signed by the node.
//!
//! It also opens a task's lane when an agent claims it, seals it when the task finishes, keeps agents' certificates
//! current, relays Weft's events onto the bus, and holds Weft's writes while STOP ALL is on.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use apz_http::{Params, Request, Response, RouteDoc, Router};
use secrecy::SecretString;
use serde_json::{Value, json};
use weft::client::Client;
use weft::keys::KeyPair;
use weft::record::{Actor, Statement};

use crate::state::App;

/// What apid knows of its Weft.
#[derive(Default)]
pub struct Weft {
    live: Mutex<Option<Live>>,
    why: Mutex<String>,
    /// task → its lane (`w-12`), as Weft opened them.
    lanes: Mutex<HashMap<String, String>>,
    /// cid → (key, the project its certificate names).
    agents: Mutex<HashMap<u64, (Arc<KeyPair>, Option<String>)>>,
    /// change → (the commit its GitHub gate was read on, the gate), so a `ci` gate is recorded once per verdict.
    pub gates: Mutex<HashMap<String, (String, String)>>,
}

struct Live {
    port: u16,
    token: String,
    mode: &'static str,
    child: Option<std::process::Child>,
    /// The child's stdin, held open for as long as apid runs: `weft serve --exit-with-stdin` exits when it closes,
    /// so the child never outlives apid, even one killed without running Drop.
    _stdin: Option<std::process::ChildStdin>,
    _inproc: Option<(Arc<weft::server::Hub>, apz_http::Handle)>,
}

impl Drop for Live {
    /// The `weft serve` child goes with its owner: killed and reaped here, not left running after apid.
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Stops Weft for shutdown: takes the `weft serve` child out of the hub and drops it, which kills and reaps it
/// (`Live::drop`). The process exits with `std::process::exit`, which would not run that drop on its own.
pub fn stop(app: &App) {
    let live = app
        .weft
        .live
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take();
    drop(live);
}

/// The vault binding Weft's keys are read through.
const BINDING: &str = "hub:weft";

impl Weft {
    pub fn status(&self) -> Value {
        let live = self.live.lock().unwrap_or_else(|p| p.into_inner());
        match live.as_ref() {
            Some(l) => json!({ "running": true, "mode": l.mode, "port": l.port }),
            None => {
                json!({ "running": false, "why": self.why.lock().map(|w| w.clone()).unwrap_or_default() })
            }
        }
    }

    pub fn running(&self) -> bool {
        self.live.lock().map(|l| l.is_some()).unwrap_or(false)
    }

    /// A client of `weft serve`, for `caller` (`owner`, `hub`, `agent:107`, `node:n1`).
    pub fn client(&self, caller: &str) -> Result<Client, String> {
        let live = self.live.lock().unwrap_or_else(|p| p.into_inner());
        let l = live.as_ref().ok_or_else(|| {
            format!(
                "Weft is not running: {}",
                self.why.lock().map(|w| w.clone()).unwrap_or_default()
            )
        })?;
        let mut c = Client::new(
            &format!("http://127.0.0.1:{}/v1", l.port),
            None,
            vec![
                (apz_http::auth::HEADER.to_string(), l.token.clone()),
                (weft::wire::CALLER.to_string(), caller.to_string()),
            ],
        )?;
        c.timeout = Duration::from_secs(600);
        Ok(c)
    }

    pub fn get(&self, path: &str) -> Result<Value, String> {
        self.client("hub")?.get(path)
    }

    pub fn post(&self, path: &str, body: &Value) -> Result<Value, String> {
        self.client("hub")?.post(path, body)
    }

    /// A statement the hub signs.
    pub fn hub(&self, kind: &str, body: Value) -> Result<Value, String> {
        self.post("/hub", &json!({ "kind": kind, "body": body }))
    }

    /// A statement for the owner, from a console: the hub signs it with `rule-for-owner`.
    pub fn console(&self, kind: &str, body: Value) -> Result<Value, String> {
        self.client("owner")?
            .post("/console", &json!({ "kind": kind, "body": body }))
    }

    pub fn lane_of(&self, task: &str) -> Option<String> {
        self.lanes.lock().ok()?.get(task).cloned()
    }

    /// The task whose lane is `change`, when one is bound (the inverse of [`Weft::lane_of`]).
    pub fn task_of_lane(&self, change: &str) -> Option<String> {
        self.lanes
            .lock()
            .ok()?
            .iter()
            .find(|(_, lane)| lane.as_str() == change)
            .map(|(task, _)| task.clone())
    }

    /// Binds `task` to `change`, the lane that carries its work when the lane was opened outside `open_lane` (a backport
    /// opened on a release line).
    pub fn bind_lane(&self, task: &str, change: &str) {
        self.lanes
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(task.to_string(), change.to_string());
    }
}

// ── start ────────────────────────────────────────────────────────────────────────────────────────────────────────

fn store_dir(app: &App) -> PathBuf {
    app.config.state_dir.join("weft")
}

/// The owner's key on this machine: `APZ_OWNER_KEY`, else the platform's config folder (`owner.key` beside the CLI's
/// `cli.json`, where `agentplaza weft` finds it).
pub fn owner_key_path(_app: &App) -> PathBuf {
    if let Ok(p) = std::env::var("APZ_OWNER_KEY") {
        return PathBuf::from(p);
    }
    apz_os::dirs::config().join("owner.key")
}

/// Serialises the making of keys: two callers that both find a key missing must not each make one (Weft's boot and the
/// first policy the hub issues can ask for the hub's key at the same moment).
static KEY_MAKING: Mutex<()> = Mutex::new(());

/// A key the vault keeps (made on first use); a file in the state folder while the vault is sealed or absent.
pub(crate) fn key(app: &App, name: &str, label: &str) -> Result<KeyPair, String> {
    let _making = KEY_MAKING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(hex) = app
        .vault
        .value_for(name, BINDING, &crate::vault::OnBehalf::Hub)
    {
        return KeyPair::from_hex(hex.trim());
    }
    let file = app
        .config
        .state_dir
        .join("weft-keys")
        .join(name.to_lowercase());
    if let Ok(k) = KeyPair::load(&file) {
        return Ok(k);
    }
    let k = KeyPair::generate();
    let secret = SecretString::from(k.seed_hex());
    match app
        .vault
        .put_plaza(name, BINDING, label, &secret, "hub", apz_plaza::now_ms())
    {
        Ok(_) => Ok(k),
        Err(_) => {
            // The vault is sealed or not set up: keep it beside the state (0600), as the dev hub does.
            std::fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
            k.save(&file)?;
            Ok(k)
        }
    }
}

/// The hub's own Weft key (`WEFT_HUB_KEY`, the key Weft's boot uses): the hub signs what it issues as itself, such as an
/// agent's policy snapshot. Made on first use, as boot makes it.
pub fn hub_key(app: &App) -> Result<KeyPair, String> {
    key(app, "WEFT_HUB_KEY", "Weft's hub key")
}

/// Starts Weft on a thread: a new store on first run (its genesis signed by the owner's key), then `weft serve`,
/// then the event relay.
pub fn start(app: Arc<App>) {
    std::thread::Builder::new()
        .name("weft".into())
        .spawn(move || {
            if let Err(e) = boot(&app) {
                eprintln!("apid: Weft did not start: {e}");
                *app.weft.why.lock().unwrap_or_else(|p| p.into_inner()) = e;
                return;
            }
            relay_events(&app);
        })
        .ok();
}

fn boot(app: &Arc<App>) -> Result<(), String> {
    let dir = store_dir(app);
    let hub = key(app, "WEFT_HUB_KEY", "Weft's hub key")?;
    let signing = key(
        app,
        "WEFT_SIGNING_KEY",
        "Weft's SSH signing key for exports",
    )?;
    if !dir.join("log").exists() {
        let path = owner_key_path(app);
        let owner = match KeyPair::load(&path) {
            Ok(k) => k,
            Err(_) => {
                let k = KeyPair::generate();
                std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
                k.save(&path)?;
                k
            }
        };
        weft::store::Store::init(
            &dir,
            "AgentPlaza",
            &owner,
            KeyPair::from_hex(&hub.seed_hex())?,
            &["certify-agents", "rule-for-owner"],
        )?;
        eprintln!(
            "apid: Weft started a new store in {} (owner key {})",
            dir.display(),
            path.display()
        );
    }
    let token_file = app.config.state_dir.join("internal.token");
    let live = match weft_binary() {
        Some(bin) => spawn_process(&bin, &dir, &token_file, &hub, &signing, app)?,
        None => {
            let h = weft::server::Hub::open_with(&dir, Some(hub), Some(signing))?;
            h.start_deriving();
            let handle = weft::server::spawn(
                h.clone(),
                0,
                apz_http::auth::InternalToken::file(&token_file),
            )
            .map_err(|e| e.to_string())?;
            Live {
                port: handle.port(),
                token: app.token.clone(),
                mode: "in-process",
                child: None,
                _stdin: None,
                _inproc: Some((h, handle)),
            }
        }
    };
    eprintln!(
        "apid: Weft serves on 127.0.0.1:{} ({})",
        live.port, live.mode
    );
    *app.weft.live.lock().unwrap_or_else(|p| p.into_inner()) = Some(live);
    if app.control.paused() {
        let _ = app.weft.post("/stop", &json!({ "on": true }));
    }
    Ok(())
}

/// The `weft` binary: `WEFT_BIN`, else beside apid's own.
fn weft_binary() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("WEFT_BIN") {
        return Some(PathBuf::from(p)).filter(|p| p.exists());
    }
    let exe = std::env::current_exe().ok()?;
    let name = if cfg!(windows) { "weft.exe" } else { "weft" };
    Some(exe.parent()?.join(name)).filter(|p| p.exists())
}

// The one unsafe call in apid's Weft code: the child's parent-death signal (see the SAFETY note in the body).
#[allow(unsafe_code)]
fn spawn_process(
    bin: &std::path::Path,
    dir: &std::path::Path,
    token: &std::path::Path,
    hub: &KeyPair,
    signing: &KeyPair,
    app: &App,
) -> Result<Live, String> {
    use std::io::Write;
    let port_file = app.config.state_dir.join("weft.port");
    let _ = std::fs::remove_file(&port_file);
    let mut command = std::process::Command::new(bin);
    command
        .args(["serve", "--state"])
        .arg(dir)
        .args([
            "--port",
            "0",
            "--hub-key-stdin",
            "--signing-key",
            "-",
            "--token-file",
        ])
        .arg(token)
        .arg("--port-file")
        .arg(&port_file)
        .arg("--exit-with-stdin")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null());
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: `pre_exec` runs the closure in the forked child before exec, where only async-signal-safe calls
        // are allowed. `prctl` is async-signal-safe and touches no memory of the parent: it only sets the child's
        // parent-death signal. The signal is sent when the thread that spawned the child exits (this is the weft
        // thread, which lives for the whole run), so the child cannot outlive apid even when apid exits without
        // running Drop.
        unsafe {
            command.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("{}: {e}", bin.display()))?;
    // The keys go over stdin, never on the command line or in a file.
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    writeln!(stdin, "{}\n{}", hub.seed_hex(), signing.seed_hex()).map_err(|e| e.to_string())?;
    for _ in 0..200 {
        if let Some(port) = std::fs::read_to_string(&port_file)
            .ok()
            .and_then(|s| s.trim().parse().ok())
        {
            return Ok(Live {
                port,
                token: app.token.clone(),
                mode: "process",
                child: Some(child),
                _stdin: Some(stdin),
                _inproc: None,
            });
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("weft serve exited: {status}"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    Err("weft serve did not start".into())
}

/// Weft's events onto the bus, as `weft` events (and the kinds the consoles follow).
fn relay_events(app: &Arc<App>) {
    loop {
        let (port, token) = match app
            .weft
            .live
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            Some(l) => (l.port, l.token.clone()),
            None => return,
        };
        let stream = apz_http::client::events_with(
            port,
            "/v1/events",
            &[(apz_http::auth::HEADER, token.as_str())],
            Duration::from_secs(60),
        );
        if let Ok(mut events) = stream {
            while let Ok(Some(ev)) = events.next_event() {
                let Ok(v) = serde_json::from_str::<Value>(&ev.data) else {
                    continue;
                };
                let kind = v["kind"].as_str().unwrap_or("");
                let topic = match kind {
                    "lane.open" | "revision.draft" | "revision.seal" | "intent.set" | "attest"
                    | "abandon" | "lane.rebase" | "conflict" => "weft_change",
                    "queue.add" | "queue.remove" => "weft_queue",
                    "land" | "import" => "weft_land",
                    "release.propose" | "release.sign" | "release.export" | "artifact.put"
                    | "release.candidate" | "release.qualify" | "release.verdict"
                    | "release.publish" | "release.promote" | "release.yank" | "release.lane" => {
                        "weft_release"
                    }
                    "export" | "connection.add" | "connection.state" | "connection.remove" => {
                        "weft_export"
                    }
                    _ => "weft_op",
                };
                app.bus.publish(topic, &json!({ "seq": v["seq"], "kind": kind, "summary": v["summary"], "change": v["body"]["change"], "repo": v["body"]["repo"] }));
                // The triggers take the landings, the lanes and the proposed releases (14.6), each once by its sequence.
                if let Some(seq) = v["seq"].as_u64() {
                    crate::triggers::weft_event(app, seq, kind, &v["body"]);
                }
                after_event(app, kind, &v["body"]);
            }
        }
        std::thread::sleep(Duration::from_secs(2));
        restart_if_dead(app);
    }
}

/// A `weft serve` process that exited is started again (the store replays its log; nothing is lost).
fn restart_if_dead(app: &Arc<App>) {
    let dead = {
        let mut live = app.weft.live.lock().unwrap_or_else(|p| p.into_inner());
        match live.as_mut().and_then(|l| l.child.as_mut()) {
            Some(child) => matches!(child.try_wait(), Ok(Some(_))),
            None => false,
        }
    };
    if dead {
        eprintln!("apid: weft serve exited; starting it again");
        *app.weft.live.lock().unwrap_or_else(|p| p.into_inner()) = None;
        if let Err(e) = boot(app) {
            eprintln!("apid: Weft did not restart: {e}");
            *app.weft.why.lock().unwrap_or_else(|p| p.into_inner()) = e;
        }
    }
}

/// What the hub does when Weft says something happened: a landing is exported to the repo's connections.
fn after_event(app: &Arc<App>, kind: &str, body: &Value) {
    // A signed release goes to the environments that run releases (a protected one asks the owner first).
    if kind == "release.sign"
        && let (Some(repo), Some(version)) = (body["repo"].as_str(), body["version"].as_str())
    {
        let (app, repo, version) = (Arc::clone(app), repo.to_string(), version.to_string());
        std::thread::spawn(move || crate::environments::tracked(&app, &repo, "releases", &version));
        return;
    }
    if kind != "land" {
        return;
    }
    let (Some(repo), Some(change)) = (body["repo"].as_str(), body["change"].as_str()) else {
        return;
    };
    let reference = body["ref"].as_str().unwrap_or_default().to_string();
    let app = Arc::clone(app);
    let (repo, change) = (repo.to_string(), change.to_string());
    std::thread::spawn(move || {
        // The task is done when its change lands.
        if let Ok(detail) = app.weft.get(&format!("/changes/{change}"))
            && let Some(task) = detail["task"].as_str()
        {
            if crate::work_loop::owns(&app, task) {
                crate::work_loop::landed(&app, task, &change);
            } else if app
                .plaza
                .tasks
                .find(task)
                .is_some_and(|t| t.state() == "awaiting_review")
            {
                let _ = app
                    .work
                    .owner_verdict(&app, task, true, &format!("{change} landed"));
            }
            crate::review_graph::post_land(&app, &repo, &change, task);
        }
        // A project that releases continuously cuts a release's draft holding each landing on main (22.3): a change
        // with a task or without one (an owner's file drop) alike.
        if reference == "main"
            && let Some(project) = crate::github_modes::project_of(&app, &repo).map(|p| p.id)
            && app.project_settings.get(&project).release_continuous
            && let Err(e) = crate::releases::routes::continuous(&app, &project, &change)
        {
            eprintln!("apid: release of {project} after {change}: {e}");
        }
        // The landing changed main: the projects on the repo read their memory again.
        crate::project_memory::landed(&app, &repo, &change);
        // And the incidents record the landing, for the triage of an alert that follows it (24.1).
        crate::incidents::landed(&app, &repo, &change);
        // And each project's base on the repo is rebuilt from the new main (16.4).
        crate::base_builds::landed(&app, &repo);
        export_all(&app, &repo);
        // And the landing goes to the environments that run main.
        crate::environments::tracked(&app, &repo, "main", &change);
    });
}

/// Exports a repo to each of its connections, with each one's credential from the vault.
pub fn export_all(app: &App, repo: &str) {
    // In `ci` mode GitHub is the truth: landings reach it as pull requests (github_modes), never as pushes to its main.
    if crate::github_modes::mode_of(app, repo) == crate::github_modes::CI {
        return;
    }
    let Ok(detail) = app.weft.get(&format!("/repos/{repo}")) else {
        return;
    };
    for c in detail["connections"].as_array().into_iter().flatten() {
        let id = c["id"].as_str().unwrap_or("");
        let token = credential(app, c["credential"].as_str());
        match app.weft.post(
            &format!("/repos/{repo}/export"),
            &json!({ "connection": id, "token": token }),
        ) {
            Ok(r) if r["state"] == "moved" => {
                app.bus.publish(
                    "weft_export",
                    &json!({ "repo": repo, "connection": id, "state": "moved", "detail": r["detail"] }),
                );
                // A mirror's remote that moved on its own: its commits become an adoption lane (21.6).
                if crate::github_modes::mode_of(app, repo) == "mirror" {
                    adopt_remote(app, repo, id, token.clone());
                }
            }
            Ok(_) => {}
            Err(e) => eprintln!("apid: exporting {repo} to {id}: {e}"),
        }
    }
}

/// An adoption on its way (21.6): the change Weft made of a remote's own commits, its project and title, and whether its
/// review has opened.
#[derive(Clone)]
struct Adoption {
    repo: String,
    change: String,
    project: String,
    title: String,
    opened: bool,
}

/// The adoptions not landed yet.
static ADOPTIONS: std::sync::Mutex<Vec<Adoption>> = std::sync::Mutex::new(Vec::new());

/// The commits a mirror's remote received on its own become a lane (Weft's adopt, 22.4). The lane goes through the review
/// chain like any lane (`adoptions_tick` opens its review and lands it once the review passed), and the project's chat
/// is told.
fn adopt_remote(app: &App, repo: &str, connection: &str, token: Option<String>) {
    let report = match app.weft.post(
        &format!("/repos/{repo}/adopt"),
        &json!({ "connection": connection, "token": token }),
    ) {
        Ok(r) => r,
        Err(e) => return eprintln!("apid: adopting {repo}'s remote commits: {e}"),
    };
    let Some(change) = report["change"].as_str().map(str::to_string) else {
        return;
    };
    let Some(project) = crate::github_modes::project_of(app, repo) else {
        return eprintln!("apid: {repo} has no project to review its adoption {change}");
    };
    let commits = report["commits"].as_array().map(Vec::len).unwrap_or(0);
    ADOPTIONS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push(Adoption {
            repo: repo.to_string(),
            change: change.clone(),
            project: project.id.clone(),
            title: format!("Adopt {commits} commit(s) from the remote"),
            opened: false,
        });
    let now = apz_plaza::now_ms();
    let chat = match app.chats.list(Some(&project.id), false).first() {
        Some(c) => c.id.clone(),
        None => app.chats.open_project(&project.id, "Adoption", now).id,
    };
    let text = format!(
        "The remote received {commits} commit(s) on its own. They are adopted as {change}: reviewed through the review chain, and they land when the review passes."
    );
    app.chats.post(
        &chat,
        crate::chats::Post {
            author: crate::chats::Author::System,
            session: None,
            seat: None,
            kind: crate::chats::MessageKind::Notice,
            text,
            card: None,
            attachments: vec![],
        },
        now,
    );
    adoptions_tick(app);
}

/// Moves the adoptions along (21.6). An adoption's review opens through the chain once the planner runs (its task is
/// bound to the change, as a backport's is), and the change lands once that review passed. An adoption that landed, or
/// whose review did not pass, leaves the list. The scheduler calls it on each pass.
pub fn adoptions_tick(app: &App) {
    let pending: Vec<Adoption> = ADOPTIONS.lock().unwrap_or_else(|p| p.into_inner()).clone();
    for a in pending {
        let finished = if !a.opened {
            match crate::work_loop::planner_cid(app, &a.project) {
                Some(planner) => {
                    let chain = crate::releases::routes::AppChain(app);
                    match crate::releases::lanes::review_adoption(
                        &chain, &a.project, &a.change, planner, &a.title,
                    ) {
                        Ok(_) => {
                            let mut list = ADOPTIONS.lock().unwrap_or_else(|p| p.into_inner());
                            if let Some(x) = list.iter_mut().find(|x| x.change == a.change) {
                                x.opened = true;
                            }
                            false
                        }
                        Err(e) => {
                            eprintln!(
                                "apid: {}: the review of {} did not open: {e}",
                                a.repo, a.change
                            );
                            false
                        }
                    }
                }
                None => false,
            }
        } else {
            match crate::releases::lanes::land_backport(
                &crate::releases::routes::deps(app),
                &a.change,
            ) {
                Ok(_) => true,
                Err(e) if e.contains("not passed yet") || e.contains("waits for the owner") => {
                    false
                }
                Err(e) => {
                    eprintln!("apid: {}: adopted {} did not land: {e}", a.repo, a.change);
                    true
                }
            }
        };
        if finished {
            ADOPTIONS
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .retain(|x| x.change != a.change);
        }
    }
}

/// How often the history keeps in step with the git platforms (21.6): a missed export is retried in order, and a
/// `ci` repo's main is imported again (polling; GitHub's webhooks would bring the same commits sooner).
const CATCH_UP_MS: u64 = 60_000;
static LAST_CATCH_UP: std::sync::Mutex<u64> = std::sync::Mutex::new(0);

/// The catch-up pass: every repo's connections are exported again (what landed since their last export, in order),
/// and a `ci` repo's GitHub main is imported. Throttled to one pass a minute.
pub fn catch_up(app: &App) {
    if !app.weft.running() || app.control.paused() {
        return;
    }
    let now = apz_plaza::now_ms();
    {
        let mut last = LAST_CATCH_UP.lock().unwrap_or_else(|p| p.into_inner());
        if now.saturating_sub(*last) < CATCH_UP_MS {
            return;
        }
        *last = now;
    }
    let Ok(repos) = app.weft.get("/repos") else {
        return;
    };
    for r in repos["repos"].as_array().into_iter().flatten() {
        let Some(name) = r["name"].as_str() else {
            continue;
        };
        if crate::github_modes::mode_of(app, name) == crate::github_modes::CI {
            if let Err(e) = crate::github_modes::poll(app, name) {
                eprintln!("apid: importing {name}'s GitHub main: {e}");
            }
        } else {
            export_all(app, name);
        }
    }
}

/// A connection's credential, by its vault name (the GitHub connector's token when it names none); `platform:<id>`
/// is an authorization of a git platform (platforms.rs), whose token is minted or renewed here.
pub fn credential(app: &App, name: Option<&str>) -> Option<String> {
    let name = name.unwrap_or("GITHUB_TOKEN");
    if let Some(found) = crate::platforms::credential(app, name) {
        return found
            .map_err(|e| eprintln!("apid: no token for {name}: {e}"))
            .ok();
    }
    app.vault
        .value_for(name, "connector:github", &crate::vault::OnBehalf::Hub)
        .or_else(|| {
            app.vault
                .value_for(name, BINDING, &crate::vault::OnBehalf::Hub)
        })
}

// ── agents ───────────────────────────────────────────────────────────────────────────────────────────────────────

/// An agent's key, certified for its current role and project (a new key when the project changed).
pub fn agent_key(app: &App, cid: u64) -> Result<Arc<KeyPair>, String> {
    let project = crate::projects::project_of(app, cid);
    if let Some((k, p)) = app
        .weft
        .agents
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&cid)
        && *p == project
    {
        return Ok(k.clone());
    }
    let agent = app
        .agents
        .get(cid)
        .ok_or_else(|| format!("no agent {cid}"))?;
    let name = format!("WEFT_AGENT_{cid}");
    let mut k = key(app, &name, &format!("Weft key of agent {cid}"))?;
    let keys = app.weft.get("/keys")?;
    let actor = json!({ "kind": "agent", "cid": cid });
    let current = keys["keys"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|x| x["id"].as_str() == Some(&k.id()) && x["revoked_seq"].is_null())
        .cloned();
    match current {
        Some(c) if c["holder"]["project"].as_str() == project.as_deref() => {}
        Some(_) => {
            // The agent moved project: retire its key and certify a new one.
            app.weft.hub(
                "cert.revoke",
                json!({ "key": k.id(), "reason": "the agent moved to another project" }),
            )?;
            k = KeyPair::generate();
            let _ = app.vault.put_plaza(
                &name,
                BINDING,
                &format!("Weft key of agent {cid}"),
                &SecretString::from(k.seed_hex()),
                "hub",
                apz_plaza::now_ms(),
            );
            certify(app, &k, &actor, &agent, project.as_deref())?;
        }
        None => certify(app, &k, &actor, &agent, project.as_deref())?,
    }
    let k = Arc::new(k);
    app.weft
        .agents
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(cid, (k.clone(), project));
    Ok(k)
}

fn certify(
    app: &App,
    k: &KeyPair,
    actor: &Value,
    agent: &crate::agents::Agent,
    project: Option<&str>,
) -> Result<(), String> {
    app.weft.hub("cert.issue", json!({ "key": k.public(), "holder": { "actor": actor, "name": agent.label(), "role": agent.role, "project": project } })).map(|_| ())
}

/// The kinds an agent's own call may have the hub sign with the agent's key.
const AGENT_KINDS: &[&str] = &["revision.draft", "revision.seal", "intent.set", "attest"];

/// Signs `kind` as agent `cid` with its key and appends it.
pub fn sign_as_agent(app: &App, cid: u64, kind: &str, body: Value) -> Result<Value, String> {
    if !AGENT_KINDS.contains(&kind) {
        return Err(format!("an agent does not say {kind}"));
    }
    let k = agent_key(app, cid)?;
    let signed = Statement::new(kind, Actor::Agent { cid }, body).sign(&k);
    app.weft
        .client(&format!("agent:{cid}"))?
        .post("/records", &signed.to_json())
}

// ── projects, lanes, seals ──────────────────────────────────────────────────────────────────────────────────────

/// A project's Weft repo: the project's repo name, created in Weft (by the owner's console) when missing.
pub fn ensure_repo(app: &App, project: &str) -> Result<String, String> {
    let p = app
        .projects
        .get(project)
        .ok_or_else(|| format!("no project {project}"))?;
    let name = p.repo.clone().unwrap_or_else(|| repo_name(&p.id, &p.title));
    if app.weft.get(&format!("/repos/{name}")).is_err() {
        app.weft.console(
            "repo.create",
            json!({ "repo": name, "project": project, "description": p.title }),
        )?;
    }
    if p.repo.is_none() {
        let _ = app.projects.set_repo(&p.id, &name);
    }
    Ok(name)
}

/// `<id>-<the title's first words>`.
pub fn repo_name(id: &str, title: &str) -> String {
    let slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        id.to_string()
    } else {
        format!("{id}-{slug}")
    }
}

/// Opens the lane for a task an agent just claimed: on main's head, or stacked on the lane of the task it runs
/// after. `None` when Weft is not running (the task works without one).
pub fn open_lane(
    app: &App,
    task: &str,
    project: &str,
    title: &str,
    after: &[String],
    worker: u64,
) -> Option<apz_proto::wire::WorkLane> {
    if !app.weft.running() {
        return None;
    }
    let repo = ensure_repo(app, project)
        .map_err(|e| eprintln!("apid: no Weft repo for {project}: {e}"))
        .ok()?;
    let lane = |change: String| {
        Some(apz_proto::wire::WorkLane {
            change,
            repo: repo.clone(),
        })
    };
    if let Some(l) = app.weft.lane_of(task) {
        take_lane(app, &l, worker);
        return lane(l);
    }
    // An existing open lane for this task (a claim after a restart, a rework).
    if let Ok(found) = app.weft.get(&format!(
        "/repos/{repo}/log?q={}",
        weft::server::percent_encode(&format!("lane({task}) & open()"))
    )) && let Some(c) = found["changes"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|c| c["change"].as_str())
    {
        app.weft
            .lanes
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(task.to_string(), c.to_string());
        take_lane(app, c, worker);
        return lane(c.to_string());
    }
    let stacked = after.iter().filter_map(|t| app.weft.lane_of(t)).find(|l| {
        app.weft
            .get(&format!("/changes/{l}"))
            .is_ok_and(|c| c["state"] != "landed" && c["state"] != "abandoned")
    });
    let title: String = title
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(100)
        .collect();
    // A replay's lane is on its own branch, which starts at the case's commit; every other lane is on main.
    let reference = app
        .evals
        .replay_reference(task)
        .unwrap_or_else(|| "main".to_string());
    let r = app.weft.post("/lanes", &json!({ "repo": repo, "ref": reference, "task": task, "title": title, "author": { "kind": "agent", "cid": worker }, "stacked_on": stacked }));
    match r {
        Ok(v) => {
            let change = v["change"].as_str()?.to_string();
            app.weft
                .lanes
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(task.to_string(), change.clone());
            let _ = agent_key(app, worker);
            lane(change)
        }
        Err(e) => {
            eprintln!("apid: no lane for task {task}: {e}");
            None
        }
    }
}

/// Lane `change` is written by `worker` from now on: its key is certified, and a lane another VM wrote (an ended session
/// of the task's builder seat) is handed to it (`lane.author`, signed by the hub; each revision keeps its writer's
/// signature).
fn take_lane(app: &App, change: &str, worker: u64) {
    if let Err(e) = agent_key(app, worker) {
        eprintln!("apid: agent {worker} has no Weft key for {change}: {e}");
    }
    let writes_it = app
        .weft
        .get(&format!("/changes/{change}"))
        .ok()
        .and_then(|d| d["author"].as_str().map(|a| a == format!("agent:{worker}")))
        .unwrap_or(false);
    if !writes_it
        && let Err(e) = app.weft.hub(
            "lane.author",
            json!({ "change": change, "author": { "kind": "agent", "cid": worker }, "reason": "a new session fills the task's builder seat" }),
        )
    {
        eprintln!("apid: {change} not handed to agent {worker}: {e}");
    }
}

/// A reviewer's work on a task's lane: its key is certified so it can read the lane (never write it), and the prompt
/// names the sealed revision to review. Its turns are not drafts, so the origin has no lane.
pub fn review_work(
    app: &App,
    task: &apz_plaza::tasks::Task,
    cid: u64,
    mut prompt: String,
) -> (String, crate::scheduler::Origin) {
    if app.weft.running()
        && let Some(change) = app.weft.lane_of(task.id())
        && let Ok(detail) = app.weft.get(&format!("/changes/{change}"))
    {
        if let Err(e) = agent_key(app, cid) {
            eprintln!("apid: reviewer {cid} has no Weft key for {change}: {e}");
        }
        let repo = detail["repo"].as_str().unwrap_or("");
        let rounds = detail["rounds"].as_array().map_or(0, Vec::len);
        prompt.push_str(&format!(
            "\n\nThe change to review is lane {change} in Weft (repo {repo}), sealed as {change}.r{rounds}. Check it out \
             read-only with `weft checkout {change} --dir ./review` and read it with `weft diff` and `weft show`; you do \
             not write to it."
        ));
    }
    let origin = crate::scheduler::Origin {
        task: Some(task.id().to_string()),
        ..Default::default()
    };
    (prompt, origin)
}

/// The lane an editor session's agent works in (`acp/<session>`, the agent's, never queued): opened the first time
/// on the editor's lane (`branch`, the owner's latest mirror of the editor's project), the same lane after. `None`
/// without Weft, or before the editor has mirrored the project.
pub fn session_lane(
    app: &App,
    repo: &str,
    branch: &str,
    session: &str,
    cid: u64,
) -> Option<apz_proto::wire::WorkLane> {
    if !app.weft.running() {
        return None;
    }
    let key = format!("acp/{session}");
    let lane = |change: String| {
        Some(apz_proto::wire::WorkLane {
            change,
            repo: repo.to_string(),
        })
    };
    if let Some(l) = app.weft.lane_of(&key) {
        return lane(l);
    }
    let enc = weft::server::percent_encode;
    let open = app
        .weft
        .get(&format!(
            "/repos/{}/log?q={}&limit=200",
            enc(repo),
            enc("open()")
        ))
        .ok()?;
    let titled = |title: &str| {
        open["changes"]
            .as_array()?
            .iter()
            .find(|c| c["title"].as_str() == Some(title))
            .and_then(|c| c["change"].as_str())
            .map(str::to_string)
    };
    let change = match titled(&key) {
        Some(c) => c,
        None => {
            let editor = titled(branch)?;
            let r = app.weft.post("/lanes", &json!({ "repo": repo, "title": key, "author": { "kind": "agent", "cid": cid }, "stacked_on": editor }));
            match r {
                Ok(v) => v["change"].as_str()?.to_string(),
                Err(e) => {
                    eprintln!("apid: no lane for editor session {session}: {e}");
                    return None;
                }
            }
        }
    };
    app.weft
        .lanes
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key, change.clone());
    let _ = agent_key(app, cid);
    lane(change)
}

/// A claimed task's work for its worker: the lane opened (or found), and the prompt told where it is.
pub fn task_work(
    app: &App,
    task: &apz_plaza::tasks::Task,
    cid: u64,
    mut prompt: String,
) -> (String, crate::scheduler::Origin) {
    let lane = task
        .project()
        .and_then(|project| open_lane(app, task.id(), project, task.goal(), &task.after(), cid));
    if let Some(l) = &lane {
        prompt.push_str(&format!(
            "\n\nYour work is lane {} in Weft, checked out in ./{}/ in your workspace: write the task's files there; every turn is kept as a draft. \
             Before you finish, name what the change does with weft_describe (one intent per thing it does, with the paths or definitions each covers).",
            l.change, l.repo
        ));
    }
    let origin = crate::scheduler::Origin {
        task: Some(task.id().to_string()),
        lane,
        ..Default::default()
    };
    (prompt, origin)
}

/// Seals the lane of a task its worker finished: the latest draft's files, signed with the worker's key, then
/// `written`. Returns the sealed revision.
pub fn seal(app: &App, task: &str, worker: u64, summary: &str) -> Option<String> {
    let change = app.weft.lane_of(task)?;
    let detail = app.weft.get(&format!("/changes/{change}")).ok()?;
    if detail["state"] == "landed" || detail["state"] == "abandoned" {
        return None;
    }
    let latest_draft = detail["drafts"]
        .as_array()
        .and_then(|d| d.last())
        .and_then(|d| d.as_str())
        .map(str::to_string);
    let deltas = match &latest_draft {
        Some(d) => app.weft.get(&format!("/revisions/{d}")).ok()?["deltas"].clone(),
        None => json!([]),
    };
    let n = detail["rounds"].as_array().map(|r| r.len()).unwrap_or(0) + 1;
    let model = app.agents.get(worker).and_then(|a| a.model_override);
    if let Err(e) = sign_as_agent(
        app,
        worker,
        "revision.seal",
        json!({ "change": change, "n": n, "deltas": deltas, "model": model }),
    ) {
        eprintln!("apid: sealing {change}: {e}");
        return None;
    }
    let rev = format!("{change}.r{n}");
    let digest = app.weft.get(&format!("/revisions/{rev}")).ok()?["digest"].clone();
    let _ = sign_as_agent(
        app,
        worker,
        "attest",
        json!({ "revision": rev, "digest": digest, "predicate_type": "written", "predicate": { "task": task, "summary": summary } }),
    );
    Some(rev)
}

/// Gives up a task's lane (the task failed or was withdrawn): its drafts stay 30 days, then go.
pub fn abandon(app: &App, task: &str, reason: &str) {
    if let Some(change) = app.weft.lane_of(task) {
        let _ = app
            .weft
            .hub("abandon", json!({ "change": change, "reason": reason }));
    }
}

/// Lands what the merge queue holds, one step at a time per queue (4.6). The queue tests its lanes in batches on top of
/// its head (the project's size); a batch that fails is bisected to the lane that breaks it, which leaves with the
/// failure; a project that asks for it holds each landing until the owner approves it (a `land` approval).
pub fn land_queues(app: &App) {
    if !app.weft.running() || app.control.paused() {
        return;
    }
    let Ok(repos) = app.weft.get("/repos") else {
        return;
    };
    for r in repos["repos"].as_array().into_iter().flatten() {
        let name = r["name"].as_str().unwrap_or("");
        for (reference, queue) in r["queues"].as_object().into_iter().flatten() {
            let Some(head) = queue
                .as_array()
                .and_then(|q| q.first())
                .and_then(Value::as_str)
            else {
                continue;
            };
            // In `ci` mode the head lane goes through GitHub (its pull request and checks) before Weft lands it.
            if crate::github_modes::mode_of(app, name) == crate::github_modes::CI {
                crate::github_modes::ci_step(app, name, reference);
                continue;
            }
            let (batch, hold) = queue_policy(app, head);
            // Set when the owner approved the lane a held step named: the next step lands it.
            let mut approved = false;
            for _ in 0..8 {
                let body = json!({ "ref": reference, "recheck": true, "batch": batch, "hold": hold, "approved": approved });
                approved = false;
                match app.weft.post(&format!("/repos/{name}/queue/step"), &body) {
                    Ok(v) if v["outcome"] == "landed" => {
                        app.bus.publish("weft_land", &json!({ "repo": name, "change": v["change"], "revision": v["revision"] }));
                        continue;
                    }
                    Ok(v) if v["outcome"] == "held" => {
                        if landing_answer(app, name, reference, v["change"].as_str().unwrap_or(""))
                        {
                            approved = true;
                            continue;
                        }
                        break;
                    }
                    Ok(v) if v["outcome"] == "failed" => {
                        ejected(app, name, &v);
                        continue;
                    }
                    Ok(v) if v["outcome"] == "conflict" => {
                        conflict_task(app, name, &v);
                        break;
                    }
                    Ok(v) if v["outcome"] == "recheck" || v["outcome"] == "waiting" => {
                        if let (Some(change), Some(digest)) =
                            (v["change"].as_str(), v["digest"].as_str())
                        {
                            crate::review_graph::recheck(app, name, change, digest);
                        }
                        break;
                    }
                    Ok(_) => break,
                    Err(e) => {
                        eprintln!("apid: landing {name}:{reference}: {e}");
                        break;
                    }
                }
            }
        }
    }
}

/// The batch size of the queue whose head is `change`, and whether its project's owner approves its landings. The
/// project is the change's task's.
fn queue_policy(app: &App, change: &str) -> (usize, bool) {
    let (batch, approval) = queue_settings(app, change);
    // An upgrade's change waits for the owner when its batch says so, or its file gate fails (upgrades/, PLAN 24.2).
    let task = app
        .weft
        .get(&format!("/changes/{change}"))
        .ok()
        .and_then(|d| d["task"].as_str().map(str::to_string))
        .unwrap_or_default();
    (
        batch,
        approval || crate::upgrades::queue_hold(app, change, &task),
    )
}

/// The batch size of the queue whose head is `change`, and whether its project's owner approves its landings.
fn queue_settings(app: &App, change: &str) -> (usize, bool) {
    let project = app
        .weft
        .get(&format!("/changes/{change}"))
        .ok()
        .and_then(|d| d["task"].as_str().map(str::to_string))
        .and_then(|task| app.plaza.tasks.find(&task))
        .and_then(|t| t.project().map(str::to_string));
    match project {
        Some(pid) => {
            let settings = app.project_settings.get(&pid);
            (
                settings
                    .queue_batch
                    .unwrap_or(crate::project_settings::DEFAULT_QUEUE_BATCH)
                    as usize,
                settings.land_approval,
            )
        }
        None => (crate::project_settings::DEFAULT_QUEUE_BATCH as usize, false),
    }
}

/// The owner's answer on `change`'s `land` approval for its current round: true when it is approved. A landing with no
/// approval yet asks for one (a `land` approval in the owner's queue); a pending one waits.
fn landing_answer(app: &App, repo: &str, reference: &str, change: &str) -> bool {
    if change.is_empty() {
        return false;
    }
    let Ok(detail) = app.weft.get(&format!("/changes/{change}")) else {
        return false;
    };
    let round = detail["rounds"].as_array().map_or(0, Vec::len) as u64;
    let latest = app.approvals.list().into_iter().rfind(|a| {
        a.kind == "land"
            && a.detail["change"].as_str() == Some(change)
            && a.detail["round"].as_u64() == Some(round)
    });
    match latest {
        Some(a) => a.state == crate::approvals::State::Approved,
        None => {
            let task = detail["task"].as_str().unwrap_or("").to_string();
            let cid = detail["author"]
                .as_str()
                .and_then(|a| a.strip_prefix("agent:"))
                .and_then(|c| c.parse::<u64>().ok())
                .unwrap_or(0);
            let title = app
                .plaza
                .tasks
                .find(&task)
                .map(|t| t.goal().to_string())
                .unwrap_or_default();
            crate::approvals::submit(
                app,
                "land",
                &format!("landing of {change}"),
                json!({
                    "cid": cid,
                    "change": change,
                    "repo": repo,
                    "ref": reference,
                    "round": round,
                    "task": task,
                    "goal": title,
                    "summary": format!("{change} passed review and its checks: it lands on {reference} when you approve it."),
                    "choices": ["approve", "deny"],
                }),
            );
            false
        }
    }
}

/// The owner answered a `land` approval. Approved, the lane lands at the queue's next step. Denied, it leaves the queue
/// and its builder goes again with the owner's word.
pub fn landing_decided(
    app: &App,
    item: &crate::approvals::Approval,
    approve: bool,
) -> Result<(), String> {
    if approve {
        return Ok(());
    }
    let change = item.detail["change"]
        .as_str()
        .ok_or("no change on the card")?;
    let repo = item.detail["repo"].as_str().ok_or("no repo on the card")?;
    let _ = app.weft.hub(
        "queue.remove",
        json!({ "repo": repo, "ref": item.detail["ref"], "change": change, "n": item.detail["round"], "reason": "the owner did not approve the landing" }),
    );
    let note = "the owner did not approve the landing".to_string();
    return_lane(
        app,
        repo,
        change,
        &note,
        format!(
            "Your change {change} was not approved to land: the owner says no. Check it out again (weft checkout {change}) and rework it as the owner needs, then finish: it is reviewed and re-checked like any change."
        ),
    );
    Ok(())
}

/// A lane that left the queue with a failure (4.6: a batch's breaker): its builder goes again with the failure.
fn ejected(app: &App, repo: &str, step: &Value) {
    let Some(change) = step["change"].as_str() else {
        return;
    };
    let why = step["reason"]
        .as_str()
        .unwrap_or("it breaks the queue")
        .to_string();
    let prompt = format!(
        "Your change {change} left the landing queue: {why}. Check it out again (weft checkout {change}), fix what breaks, and finish: the fix is reviewed and re-checked like any change."
    );
    return_lane(app, repo, change, &why, prompt);
}

/// A clash on landing: the lane's agent gets a resolve item (its lane now starts from main, with the clash marked).
pub fn conflict_task(app: &App, repo: &str, step: &Value) {
    let Some(change) = step["change"].as_str() else {
        return;
    };
    let files: Vec<String> = step["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f.as_str().map(str::to_string))
        .collect();
    let prompt = format!(
        "Your change {change} clashed with main on landing ({}). Your lane now starts from main's head and its latest draft marks the clash \
         (<<<<<<< this change … ======= … >>>>>>> main). Check it out again (weft checkout {change}), resolve every marked place keeping both intents where \
         they can live together, and finish: the resolution is reviewed and re-checked like any change.",
        files.join(", ")
    );
    return_lane(
        app,
        repo,
        change,
        &format!("{change} clashed with main on {}", files.join(", ")),
        prompt,
    );
}

/// Sends a lane's builder back with `prompt`: a seat of the task goes again with it, else the task is reworked (with
/// `note`) and its worker gets the work on the lane.
fn return_lane(app: &App, repo: &str, change: &str, note: &str, prompt: String) {
    let Ok(detail) = app.weft.get(&format!("/changes/{change}")) else {
        return;
    };
    let (Some(task), Some(cid)) = (
        detail["task"].as_str(),
        detail["author"]
            .as_str()
            .and_then(|a| a.strip_prefix("agent:"))
            .and_then(|c| c.parse::<u64>().ok()),
    ) else {
        return;
    };
    // A task that runs in seats: its builder seat goes again with the prompt (the session that wrote it has ended).
    if crate::work_loop::owns(app, task) {
        crate::work_loop::clashed(app, task, &prompt);
        return;
    }
    let lane = Some(apz_proto::wire::WorkLane {
        change: change.to_string(),
        repo: repo.to_string(),
    });
    // The task goes back to its worker: its resolution is a new round, reviewed like any other.
    if let Some(t) = app.plaza.tasks.find(task) {
        let _ = t.rework(Some(note.to_string()));
    }
    let origin = crate::scheduler::Origin {
        task: Some(task.to_string()),
        lane,
        ..Default::default()
    };
    app.work.enqueue(
        app,
        cid,
        apz_proto::wire::WorkKind::Task,
        format!("task-{task}"),
        prompt,
        origin,
    );
}

/// STOP ALL holds Weft's writes too.
pub fn hold(app: &App, on: bool) {
    let _ = app.weft.post("/stop", &json!({ "on": on }));
}

// ── the doors ────────────────────────────────────────────────────────────────────────────────────────────────────

fn forward(app: &App, req: &Request, caller: &str, rest: &str) -> Response {
    let client = match app.weft.client(caller) {
        Ok(c) => c,
        Err(e) => return Response::error(503, &e),
    };
    let mut target = client.target.clone();
    for name in ["content-type", "range"] {
        if let Some(v) = req.header(name) {
            target.headers.push((name.to_string(), v.to_string()));
        }
    }
    let path = if req.query.is_empty() {
        format!("/v1/{rest}")
    } else {
        format!("/v1/{rest}?{}", req.query)
    };
    let body = (!req.body.is_empty() || matches!(req.method.as_str(), "POST" | "PUT"))
        .then_some(&req.body[..]);
    match apz_http::client::request(&target, &req.method, &path, body, Duration::from_secs(600)) {
        Ok(r) => {
            let mut resp = Response::new(r.status, &r.content_type, r.body);
            for (k, v) in r.headers.iter().filter(|(k, _)| {
                k.eq_ignore_ascii_case("content-range") || k.eq_ignore_ascii_case("cache-control")
            }) {
                resp = resp.with_header(k, v);
            }
            resp
        }
        Err(e) => Response::error(502, &format!("Weft: {e}")),
    }
}

/// Paths that change history (everything an agent or a console may not do while STOP ALL is on).
fn writes(method: &str, rest: &str) -> bool {
    method != "GET"
        && !rest.starts_with("wire/missing")
        && !rest.starts_with("wire/bundle")
        && !rest.starts_with("wire/info")
        && !rest.starts_with("wire/want")
}

fn owner_door(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let rest = p.get("rest");
    if app.control.paused() && writes(&req.method, rest) && !rest.starts_with("wire/") {
        return Response::error(423, "STOP ALL is on: Weft takes no writes");
    }
    // The owner's CLI on a device without the owner key: the hub signs for the owner, marked as from the console.
    let rest = if rest == "records/unsigned" {
        "console"
    } else {
        rest
    };
    // A push or a fetch the console asks for takes its connection's credential here, as the hub's own exports do.
    if let Some(with_token) = with_credential(app, req, rest) {
        let mut req = req.clone();
        req.body = with_token;
        return forward(app, &req, "owner", rest);
    }
    forward(app, req, "owner", rest)
}

/// The body of an owner's export, release export or adopt with its connection's token added, when it has none.
fn with_credential(app: &App, req: &Request, rest: &str) -> Option<Vec<u8>> {
    let parts: Vec<&str> = rest.split('/').collect();
    let repo = match parts.as_slice() {
        ["repos", repo, "export" | "adopt"] | ["repos", repo, "releases", _, "export"] => *repo,
        _ => return None,
    };
    if req.method != "POST" {
        return None;
    }
    let mut v: Value = req.json().ok()?;
    if v["token"].is_string() {
        return None;
    }
    let conn = v["connection"].as_str()?.to_string();
    let detail = app.weft.get(&format!("/repos/{repo}")).ok()?;
    let name = detail["connections"]
        .as_array()?
        .iter()
        .find(|c| c["id"] == conn.as_str())?["credential"]
        .as_str()
        .map(str::to_string);
    v["token"] = json!(credential(app, name.as_deref())?);
    serde_json::to_vec(&v).ok()
}

fn agent_door(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let rest = p.get("rest").to_string();
    crate::internal::authorized(app, req, p.get("cid"), |cid| {
        if app.control.paused() && writes(&req.method, &rest) && !rest.starts_with("wire/objects") {
            return Err(Response::error(423, "STOP ALL is on: Weft takes no writes"));
        }
        // An agent's statements: signed here with its own key, inside its own call.
        if rest == "records/unsigned" {
            let v: Value = req
                .json()
                .map_err(|e| Response::error(400, &format!("not JSON: {e}")))?;
            let kind = v["kind"].as_str().unwrap_or("");
            let role = app.agents.get(cid).map(|a| a.role).unwrap_or_default();
            let host = jhrl::authz::Entity {
                ty: "Host".into(),
                id: "hub-weft".into(),
            };
            let attrs = std::collections::BTreeMap::from([(
                "group".to_string(),
                jhrl::value::Value::str("hub-weft"),
            )]);
            if let Some(why) =
                app.laws
                    .refuse_op(cid, &role, "egress.connect", host, attrs, &Value::Null)
            {
                return Err(Response::error(403, &why));
            }
            return Ok(match sign_as_agent(app, cid, kind, v["body"].clone()) {
                Ok(r) => Response::json(200, &r),
                Err(e) if e.contains("does not say") => Response::error(403, &e),
                Err(e) => Response::error(409, &e),
            });
        }
        // An agent signs only through the hub; the hub's own routes are not theirs.
        if rest == "records"
            || rest == "hub"
            || rest == "console"
            || rest == "lanes"
            || rest == "stop"
            || rest.contains("/queue/")
            || rest.ends_with("/export")
            || rest.ends_with("/import")
            || rest.ends_with("/adopt")
        {
            return Err(Response::error(403, "that is the hub's to do"));
        }
        Ok(forward(app, req, &format!("agent:{cid}"), &rest))
    })
}

fn node_door(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let node = match app.nodes.authenticate(req, apz_plaza::now_ms()) {
        Ok(n) => n,
        Err(_) => return Response::error(403, "bad node signature"),
    };
    if node.id != p.get("id") {
        return Response::error(403, "that is another node");
    }
    let rest = p.get("rest");
    // A cache reads objects; it writes only what its VMs upload.
    let allowed = rest.starts_with("wire/") || rest == "packs";
    if !allowed {
        return Response::error(403, "a node's cache reads objects only");
    }
    // A cache may speak for an agent on its own node.
    let caller = match req
        .header(weft::wire::CALLER)
        .and_then(|c| c.strip_prefix("agent:"))
        .and_then(|c| c.parse::<u64>().ok())
    {
        Some(cid) if cid >= node.cid_base && cid < node.cid_base + node.cid_count => {
            format!("agent:{cid}")
        }
        _ => format!("node:{}", node.id),
    };
    forward(app, req, &caller, rest)
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/weft", |app, _, _| Response::json(200, &app.weft.status()))
        .doc(RouteDoc::new("Show whether Weft, the plaza's version control, is running"))
        .get("/v1/weft/{rest..}", owner_door)
        .doc(RouteDoc::new("Read Weft: repos, changes, revisions, change trees, blame, queues, releases, artifacts, keys").description("Everything `weft serve` answers under /v1 (see crates/weft/src/server.rs), as the owner.").errors(&[404, 502, 503]))
        .post("/v1/weft/{rest..}", owner_door)
        .doc(RouteDoc::new("Write to Weft as the owner: signed statements, console rulings, releases, exports").errors(&[400, 403, 409, 423, 502, 503]))
        .route("PUT", "/v1/weft/{rest..}", owner_door)
        .doc(RouteDoc::new("Upload an object to Weft").errors(&[400, 503]))
        .get("/internal/v1/agents/{cid}/gateway/weft/{rest..}", agent_door)
        .doc(RouteDoc::new("Relay an agent's Weft reads").errors(&[403, 404, 502, 503]))
        .post("/internal/v1/agents/{cid}/gateway/weft/{rest..}", agent_door)
        .doc(RouteDoc::new("Relay an agent's Weft writes, signed with its key").errors(&[400, 403, 409, 423, 502, 503]))
        .route("PUT", "/internal/v1/agents/{cid}/gateway/weft/{rest..}", agent_door)
        .doc(RouteDoc::new("Relay an agent's object upload to Weft").errors(&[400, 403, 503]))
        .get("/internal/v1/nodes/{id}/weft/{rest..}", node_door)
        .doc(RouteDoc::new("Answer a node cache's Weft object miss").errors(&[403, 404, 502, 503]))
        .post("/internal/v1/nodes/{id}/weft/{rest..}", node_door)
        .doc(RouteDoc::new("Answer a node cache's Weft object batch").errors(&[403, 404, 502, 503]))
        .route("PUT", "/internal/v1/nodes/{id}/weft/{rest..}", node_door)
        .doc(RouteDoc::new("Take an object a node cache uploads for its VMs").errors(&[400, 403, 503]))
}
