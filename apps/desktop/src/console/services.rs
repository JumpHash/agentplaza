//! The local services this machine runs, started and stopped by the console: in local mode the
//! hub (apid), the agent gateway (vsockd) and the node (noded), plus Postgres / Forgejo (Docker
//! containers) and Garage when installed; as a node, apz-link + vsockd + noded.

use std::env;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use apz_setup::node::{
    self, Layout, PlanProcess, cid_list, find_plan_pids, named_pids, repo_dir, resource, stop_pids,
};

/// The bind address of a hub other machines join, and the public URL it advertises (`https://<this machine>:7700`).
/// `status` and `stop` rebuild the hub's plan with these, so they match the processes `start` made.
const HUB_BIND: &str = "0.0.0.0";

fn hub_public_url() -> String {
    format!("https://{}:7700", hostname())
}

/// The pids this app started, by service name (one per name: a restart replaces it). Stop trusts only
/// these on Windows, where a process's command line cannot be read to tell whose it is.
static STARTED: Mutex<Vec<(String, u32)>> = Mutex::new(Vec::new());

/// One local service and its state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    /// "running" | "stopped" | "missing" (not installed) | "optional" (not set up; not required).
    pub state: String,
    pub detail: String,
}

/// Which set of services a mode runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Hub and one node on this machine, loopback only.
    Local,
    /// A hub other machines join (TLS on, owner token, join codes).
    Hub,
    /// This machine is a node of another hub.
    Node,
}

/// The directory the sockets of apid, vsockd and noded share: `/tmp/plaza-vsock` on Linux and macOS, the
/// state directory's `vsock` on Windows.
fn uds_dir() -> String {
    apz_os::dirs::vsock_sockets().to_string_lossy().into_owned()
}

/// One service of a mode: a Docker container (never stopped by the console) or a process.
struct Svc {
    name: &'static str,
    how: How,
}

enum How {
    /// A Docker container, by its Docker name (the displayed name is the service's).
    Container { docker: &'static str },
    Process {
        /// `None` when the binary (or its config) is not installed.
        bin: Option<PathBuf>,
        args: Vec<String>,
        env: Option<(&'static str, &'static str)>,
    },
}

/// The services of `mode` and their state now.
pub fn status(mode: Mode) -> Vec<Service> {
    plan_of(mode)
        .iter()
        .map(|svc| {
            let (state, detail) = svc_state(svc);
            Service {
                name: svc.name.to_string(),
                state: state.to_string(),
                detail,
            }
        })
        .collect()
}

/// Starts what `mode` needs that is not running; returns a line per service.
pub fn start(mode: Mode) -> Result<Vec<String>, String> {
    if mode == Mode::Hub {
        return start_hub(HUB_BIND, &hub_public_url());
    }
    start_with(mode, None)
}

/// Starts a hub other machines join: apid binds `bind` with TLS and advertises `public_url`. The
/// console's hub is `HUB_BIND` with `hub_public_url()`: `status` and `stop` assume those.
pub fn start_hub(bind: &str, public_url: &str) -> Result<Vec<String>, String> {
    start_with(Mode::Hub, Some((bind, public_url)))
}

/// Stops the services `mode` started (SIGTERM to the processes; containers are left running).
pub fn stop(mode: Mode) -> Result<Vec<String>, String> {
    Ok(plan_of(mode).iter().rev().map(stop_one).collect())
}

/// The services of `mode` as the console starts them: a hub's apid carries the public bind arguments.
fn plan_of(mode: Mode) -> Vec<Svc> {
    let public = hub_public_url();
    plan(
        mode,
        (mode == Mode::Hub).then_some((HUB_BIND, public.as_str())),
    )
}

/// The owner token a local hub wrote for this machine (`<state>/owner-token`), if any.
pub fn local_owner_token() -> Option<String> {
    let text = fs::read_to_string(state_dir().join("owner-token")).ok()?;
    let token = text.trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// Joins a hub as a node with a join token (runs `apz-link join`); returns the node id.
pub fn join(token: &str) -> Result<String, String> {
    let bin =
        bin_path("apz-link").unwrap_or_else(|| PathBuf::from(apz_os::process::exe("apz-link")));
    let out = Command::new(&bin)
        .arg("join")
        .arg(token)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("apz-link: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            "apz-link join failed".into()
        } else {
            err
        });
    }
    parse_join_output(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| "unexpected output from apz-link join".into())
}

/// This machine as a node: whether it has joined a hub, what the hub gave it, and whether its
/// forwarder (apz-link) runs.
#[cfg(test)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeStatus {
    pub joined: bool,
    pub hub: String,
    pub node_id: String,
    /// The guest CIDs as `A-B`; empty when not joined.
    pub cids: String,
    pub running: bool,
    /// apz-link's pid while it runs: the first pid `find_pids` finds, the scan Start and Stop use.
    pub pid: Option<u32>,
}

/// The node's identity (`node/node.json` under the data directory) and whether apz-link runs.
#[cfg(test)]
pub fn node_status() -> NodeStatus {
    let node: Option<serde_json::Value> = fs::read_to_string(data_dir().join("node/node.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    let field = |key: &str| node.as_ref().and_then(|n| n.get(key));
    let text = |key: &str| {
        field(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let cids = match (
        field("cid_base").and_then(|v| v.as_u64()),
        field("cid_count").and_then(|v| v.as_u64()),
    ) {
        (Some(base), Some(count)) if count > 0 => node::cid_range(base, count),
        _ => String::new(),
    };
    let pids = find_plan_pids(&node::link());
    NodeStatus {
        joined: node.is_some(),
        hub: text("hub_url"),
        node_id: text("node_id"),
        cids,
        running: !pids.is_empty(),
        pid: pids.first().copied(),
    }
}

fn start_with(mode: Mode, hub: Option<(&str, &str)>) -> Result<Vec<String>, String> {
    if mode == Mode::Node && node::identity().is_none() {
        return Err("join a hub first".into());
    }
    Ok(plan(mode, hub).iter().map(start_one).collect())
}

/// The services of `mode` in start order. `hub` adds the public bind arguments to apid.
fn plan(mode: Mode, hub: Option<(&str, &str)>) -> Vec<Svc> {
    if mode == Mode::Node {
        let (base, count) = node::identity().unwrap_or((0, 0));
        return node_plan(base, count);
    }
    let roles = path_arg(&resource(
        "roles/initial-roles.json",
        "docs/agents/initial-roles.json",
    ));
    let prompts = path_arg(&resource("prompts", "docs/codex-draft/roles"));
    let console = path_arg(&resource("console", "apps/web/dist"));
    let cids = local_cids();
    let internal = path_arg(&state_dir().join("internal.token"));
    let garage_bin = Path::new("/usr/bin/garage");
    let garage_cfg = data_dir().join("garage/garage.toml");
    let garage_ok = garage_bin.is_file() && garage_cfg.is_file();
    let garage_cfg = path_arg(&garage_cfg);

    let mut apid_args = argv(&[
        "--port",
        "7700",
        "--noded-port",
        "8899",
        "--hub-port",
        "19080",
        "--roles",
        &roles,
        "--prompts",
        &prompts,
        "--console",
        &console,
    ]);
    if let Some((bind, public_url)) = hub {
        apid_args.extend(argv(&["--bind", bind, "--tls", "--public-url", public_url]));
    }

    vec![
        Svc {
            name: "postgres",
            how: How::Container {
                docker: "agentplaza-postgres",
            },
        },
        Svc {
            name: "forgejo",
            how: How::Container {
                docker: "agentplaza-forgejo",
            },
        },
        Svc {
            name: "garage",
            how: How::Process {
                bin: garage_ok.then(|| garage_bin.to_path_buf()),
                args: argv(&["-c", &garage_cfg, "server"]),
                env: None,
            },
        },
        Svc {
            name: "apid",
            how: How::Process {
                bin: bin_path("apid"),
                args: apid_args,
                env: None,
            },
        },
        Svc {
            name: "vsockd",
            how: How::Process {
                bin: bin_path("vsockd"),
                args: argv(&[
                    "--uds-dir",
                    &uds_dir(),
                    "--cids",
                    &cids,
                    "--port",
                    "5000",
                    "--http-port",
                    "19080",
                    // The E2B gateway runs with the hub: the hub apid's default `--e2b-gateway-port`.
                    "--e2b-port",
                    "49982",
                    "--apid",
                    "http://127.0.0.1:7700",
                    "--internal-token-file",
                    &internal,
                ]),
                env: None,
            },
        },
        Svc {
            name: "noded",
            how: How::Process {
                bin: bin_path("noded"),
                args: argv(&[
                    "--port",
                    "8899",
                    "--uds-dir",
                    &uds_dir(),
                    "--agents",
                    &roles,
                    "--internal-token-file",
                    &internal,
                ]),
                // The per-VM process mode and its jailer are Linux-only.
                env: if cfg!(target_os = "linux") {
                    Some(("APZ_VMM_SPAWN", "process"))
                } else {
                    None
                },
            },
        },
    ]
}

/// The node's services, from the plan both apps share (`apz_setup::node::plan`): apz-link forwards to
/// the hub, vsockd and noded serve the node's CIDs. Each binary is found beside this app.
fn node_plan(base: u64, count: u64) -> Vec<Svc> {
    node::plan(base, count, &Layout::current())
        .into_iter()
        .map(|process| Svc {
            name: process.name,
            how: How::Process {
                bin: bin_path(process.name),
                args: process.args,
                env: process.env,
            },
        })
        .collect()
}

/// The CIDs of the local hub's agents: 20 through 130.
fn local_cids() -> String {
    cid_list(20, 111)
}

/// Garage is the Library's file store: optional, so a machine without it is not "missing".
const OPTIONAL_SERVICE: &str = "garage";

fn svc_state(svc: &Svc) -> (&'static str, String) {
    match &svc.how {
        How::Container { docker } => match container_state(docker) {
            "missing" => ("missing", "docker or the container is absent".into()),
            state => (state, format!("container {docker}")),
        },
        How::Process { bin, .. } => {
            if let Some(pid) =
                process_of(svc).and_then(|process| find_plan_pids(&process).first().copied())
            {
                return ("running", format!("pid {pid}"));
            }
            match bin {
                Some(_) => ("stopped", String::new()),
                None if svc.name == OPTIONAL_SERVICE => (
                    "optional",
                    "not set up: the Library keeps files on disk".into(),
                ),
                None => ("missing", "not installed".into()),
            }
        }
    }
}

fn start_one(svc: &Svc) -> String {
    let name = svc.name;
    match &svc.how {
        How::Container { docker } => match container_state(docker) {
            "running" => format!("{name}: already running"),
            "stopped" => match Command::new("docker")
                .args(["start", docker])
                .stdin(Stdio::null())
                .output()
            {
                Ok(out) if out.status.success() => format!("{name}: started"),
                Ok(out) => format!("{name}: failed ({})", first_line(&out.stderr)),
                Err(e) => format!("{name}: failed ({e})"),
            },
            _ => format!("{name}: not installed"),
        },
        How::Process { bin, args, env } => {
            if let Some(pid) =
                process_of(svc).and_then(|process| find_plan_pids(&process).first().copied())
            {
                return format!("{name}: already running (pid {pid})");
            }
            let Some(bin) = bin else {
                if name == OPTIONAL_SERVICE {
                    return format!("{name}: not set up (optional)");
                }
                return format!("{name}: not installed");
            };
            // Another process of this name that is not the plan's holds the ports this one needs.
            if let Some(pid) = named_pids(name).first() {
                return format!(
                    "{name}: not started: another {name} runs (pid {pid}) that this app did not start"
                );
            }
            match spawn(name, bin, args, *env) {
                Ok(pid) => format!("{name}: started (pid {pid})"),
                Err(e) => format!("{name}: failed ({e})"),
            }
        }
    }
}

/// Stops the service's own processes: the plan's, matched by name and arguments (see `node::stop_pids`).
/// A process of the same name that is not the plan's is left alone.
fn stop_one(svc: &Svc) -> String {
    let name = svc.name;
    let Some(process) = process_of(svc) else {
        return format!("{name}: left running (the container persists)");
    };
    let pids = stop_pids(&process, &recorded_pids(name));
    if pids.is_empty() {
        return if find_plan_pids(&process).is_empty() {
            format!("{name}: not running")
        } else {
            format!("{name}: running, but not started by this app; left alone")
        };
    }
    let signalled: Vec<bool> = pids.iter().map(|pid| signal(*pid)).collect();
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    if signalled.iter().all(|ok| *ok) {
        format!("{name}: stopped (pid {list})")
    } else {
        format!("{name}: could not signal pid {list}")
    }
}

/// The plan's process of a service: its name and the exact arguments it was started with. None for a
/// Docker container, which is not a process.
fn process_of(svc: &Svc) -> Option<PlanProcess> {
    match &svc.how {
        How::Process { args, env, .. } => Some(PlanProcess {
            name: svc.name,
            args: args.clone(),
            env: *env,
        }),
        How::Container { .. } => None,
    }
}

/// The pids this app started for a service (see `STARTED`).
fn recorded_pids(name: &str) -> Vec<u32> {
    STARTED
        .lock()
        .map(|started| {
            started
                .iter()
                .filter(|(n, _)| n == name)
                .map(|(_, pid)| *pid)
                .collect()
        })
        .unwrap_or_default()
}

/// Asks process `pid` to stop: `kill` on Unix, `taskkill` on Windows.
fn signal(pid: u32) -> bool {
    let mut cmd = if cfg!(windows) {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/PID", &pid.to_string(), "/F"]);
        cmd
    } else {
        let mut cmd = Command::new("kill");
        cmd.arg(pid.to_string());
        cmd
    };
    cmd.stdin(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Starts a detached process: stdin closed, output appended to its log, its own process group.
fn spawn(
    name: &str,
    bin: &Path,
    args: &[String],
    env: Option<(&str, &str)>,
) -> Result<u32, String> {
    let log_path = node::log_file(name);
    if let Some(dir) = log_path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| e.to_string())?;
    let errors = log.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(errors));
    apz_os::process::detach(&mut cmd);
    if let Some((key, value)) = env {
        cmd.env(key, value);
    }
    let child = cmd.spawn().map_err(|e| e.to_string())?;
    if let Ok(mut started) = STARTED.lock() {
        started.retain(|(n, _)| n != name);
        started.push((name.to_string(), child.id()));
    }
    Ok(child.id())
}

/// "running" | "stopped" | "missing" for a Docker container (missing when docker or it is absent).
fn container_state(name: &str) -> &'static str {
    match Command::new("docker")
        .args(["inspect", "-f", "{{.State.Running}}", name])
        .stdin(Stdio::null())
        .output()
    {
        Ok(out) if out.status.success() => {
            if String::from_utf8_lossy(&out.stdout).trim() == "true" {
                "running"
            } else {
                "stopped"
            }
        }
        _ => "missing",
    }
}

/// The first line of `apz-link join` stdout: "joined <hub> as <node id> (agents …)".
fn parse_join_output(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let mut words = line.trim().strip_prefix("joined ")?.split_whitespace();
        words.by_ref().find(|word| *word == "as")?;
        words.next().map(str::to_string)
    })
}

/// The plaza's state directory (`apz_os::dirs::state`).
fn state_dir() -> PathBuf {
    apz_os::dirs::state()
}

/// The plaza's data directory (`apz_os::dirs::data`).
fn data_dir() -> PathBuf {
    apz_os::dirs::data()
}

/// The service binary `name`: beside this executable (the app ships its services next to itself),
/// else `<repo>/target/debug/<name>` (development), else the first `<name>` on `$PATH`.
fn bin_path(name: &str) -> Option<PathBuf> {
    let file = apz_os::process::exe(name);
    let beside = env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&file)));
    if let Some(path) = beside.filter(|path| path.is_file()) {
        return Some(path);
    }
    let repo = repo_dir();
    if !repo.as_os_str().is_empty() {
        let built = repo.join("target/debug").join(&file);
        if built.is_file() {
            return Some(built);
        }
    }
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|dir| dir.join(&file))
            .find(|candidate| candidate.is_file())
    })
}

/// This machine's name: the kernel's on Linux, `COMPUTERNAME` on Windows, `hostname` elsewhere.
fn hostname() -> String {
    let named = fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .or_else(|| env::var("COMPUTERNAME").ok())
        .or_else(|| {
            Command::new("hostname")
                .stdin(Stdio::null())
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        });
    named
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| part.to_string()).collect()
}

fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use apz_setup::node::{Layout, runs_plan};

    use super::*;

    /// The command line a process of a plan runs: its program, then its arguments.
    fn command(process: &PlanProcess) -> Vec<String> {
        std::iter::once(process.name.to_string())
            .chain(process.args.iter().cloned())
            .collect()
    }

    /// The command line of a process started with these arguments (the owner's main plaza's, say).
    fn owner(name: &str, args: &[&str]) -> Vec<String> {
        std::iter::once(name.to_string())
            .chain(args.iter().map(|a| a.to_string()))
            .collect()
    }

    fn svc_process(services: &[Svc], name: &str) -> PlanProcess {
        let svc = services
            .iter()
            .find(|svc| svc.name == name)
            .expect("a service of the plan");
        process_of(svc).expect("a process")
    }

    #[test]
    fn a_node_and_the_hub_on_one_machine_are_told_apart_by_their_arguments() {
        let hub = plan(Mode::Local, None);
        let layout = Layout {
            roles: "/share/roles.json".into(),
            uds_dir: "/run/uds".into(),
            token: "/state/node/internal-token".into(),
        };
        let node = node::plan(20, 4, &layout);
        let node_proc = |name: &str| {
            node.iter()
                .find(|p| p.name == name)
                .expect("a node process")
                .clone()
        };
        let hub_vsockd = svc_process(&hub, "vsockd");
        let hub_noded = svc_process(&hub, "noded");
        let hub_apid = svc_process(&hub, "apid");

        // What each plan starts: the node's own command lines, and the hub's (its own plan's arguments).
        let node_vsockd_cmd = command(&node_proc("vsockd"));
        let node_noded_cmd = command(&node_proc("noded"));
        let hub_vsockd_cmd = command(&hub_vsockd);
        let hub_noded_cmd = command(&hub_noded);
        let hub_apid_cmd = command(&hub_apid);
        assert!(runs_plan(&node_vsockd_cmd, &node_proc("vsockd")));
        assert!(
            !runs_plan(&node_vsockd_cmd, &hub_vsockd),
            "a node's vsockd is not the hub's"
        );
        assert!(runs_plan(&hub_vsockd_cmd, &hub_vsockd));
        assert!(
            !runs_plan(&hub_vsockd_cmd, &node_proc("vsockd")),
            "a hub's vsockd is not the node's"
        );
        // The node's noded has every argument of the hub's noded and --run-cids besides.
        assert!(runs_plan(&node_noded_cmd, &node_proc("noded")));
        assert!(
            !runs_plan(&node_noded_cmd, &hub_noded),
            "stopping the hub must not stop a node's noded"
        );
        assert!(runs_plan(&hub_noded_cmd, &hub_noded));
        assert!(
            !runs_plan(&hub_noded_cmd, &node_proc("noded")),
            "a hub's noded is not the node's"
        );
        // Each daemon guards its HTTP API with its own token: the hub's noded with the hub's, the node's
        // with the node's, as the node's vsockd is.
        assert!(
            hub_noded
                .args
                .windows(2)
                .any(|pair| pair[0] == "--internal-token-file"
                    && pair[1].ends_with("internal.token"))
        );
        assert!(
            node_proc("noded")
                .args
                .windows(2)
                .any(|pair| pair == ["--internal-token-file", "/state/node/internal-token"])
        );
        assert!(runs_plan(&hub_apid_cmd, &hub_apid));

        // The owner's main plaza, started with its own paths and CIDs, is neither's.
        let owner_apid = owner(
            "apid",
            &[
                "--port",
                "7700",
                "--noded-port",
                "8899",
                "--hub-port",
                "19080",
            ],
        );
        let owner_vsockd = owner(
            "vsockd",
            &[
                "--uds-dir",
                "/home/u/.local/state/agentplaza/vsock",
                "--cids",
                "200,201,202",
            ],
        );
        let owner_noded = owner(
            "noded",
            &[
                "--port",
                "8899",
                "--uds-dir",
                "/home/u/.local/state/agentplaza/vsock",
                "--agents",
                "/srv/roles.json",
            ],
        );
        for cmd in [&owner_apid, &owner_vsockd, &owner_noded] {
            for process in [
                &hub_vsockd,
                &hub_noded,
                &hub_apid,
                &node_proc("vsockd"),
                &node_proc("noded"),
            ] {
                assert!(
                    !runs_plan(cmd, process),
                    "{cmd:?} must not be {}",
                    process.name
                );
            }
        }
    }

    #[test]
    fn the_hub_plan_for_the_console_carries_its_bind_arguments_and_nothing_else_moves() {
        let hub = plan_of(Mode::Hub);
        let apid = svc_process(&hub, "apid");
        let flag = |name: &str| {
            apid.args
                .iter()
                .position(|a| a == name)
                .and_then(|i| apid.args.get(i + 1))
                .cloned()
        };
        assert_eq!(flag("--bind").as_deref(), Some(HUB_BIND));
        assert_eq!(flag("--public-url"), Some(hub_public_url()));
        assert!(apid.args.iter().any(|a| a == "--tls"));
        // Local is the hub without those arguments.
        assert!(!plan_of(Mode::Local).iter().any(
            |svc| matches!(&svc.how, How::Process { args, .. } if args.iter().any(|a| a == "--tls"))
        ));
    }

    #[test]
    fn join_output_yields_the_node_id() {
        let out = "joined https://hub.example:7700 as n3 (agents 200-219)\n";
        assert_eq!(parse_join_output(out).as_deref(), Some("n3"));
    }
}
