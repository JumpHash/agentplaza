//! The trainer endpoints: `POST /v1/train` starts a job, `GET /v1/train/{job_id}/events` returns
//! what it printed, `POST /v1/train/{job_id}/stop` stops it, `GET /v1/train` lists the running
//! jobs and `DELETE /v1/train/{job_id}` removes a finished one.
//!
//! A job runs natively on every OS: `uv run --frozen --no-sync --project <trainer> python -m
//! apz_train.run <args>` under apz-sandbox, with the job directory `<state>/train/<job_id>/` as its
//! working directory. That directory holds `pid`, `events.jsonl` (stdout), `stderr.log`,
//! `launch.json` (the runtime and the accelerators the job holds) and `exit` (the exit code, written
//! when the process ends), and the job's own `home`, `bin` and `cache`. The sandbox reads the trainer
//! project and uv's managed-Python directory, writes the job directory and the read-write mounts the
//! job asks for (which must sit under the state directory), and reaches no network. The trainer
//! project is read-only and is not installed: its environment is made beforehand with `uv sync`,
//! and a job only runs it.
//!
//! A job outlives its exit: its directory is kept, with the exit code, until `DELETE` removes it,
//! and finished jobs older than 24 h are reaped on each `GET /v1/train`.
//!
//! Agents never get raw host access: the accelerators are devices the node has, the mounts sit
//! under the state directory, and the ids, env keys and network are checked against strict
//! charsets. Events are stdout's lines, one per event: JSON objects as they are, any other line as
//! `{"ev":"log","text":…}`, each with its 1-based `line`. `after=N` returns the lines after N.
//!
//! The runtime is probed once at start (see `probe_trainer`): `native-nvidia`, `native-amd`,
//! `native-intel`, `native-cpu`, or `native-mps` on macOS.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::File;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use apz_http::Response;
use apz_sandbox::{Limits, Network, Sandboxed, Spec};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Accelerator, Node, Trainer, command_stdout, command_within};

/// The trainer project: the directory with `pyproject.toml` and the `apz_train` package. Defaults
/// to `<state>/trainer`.
const RUNNER_ENV: &str = "APZ_TRAINER_DIR";
/// The uv binary, for a uv that is not on PATH.
const UV_ENV: &str = "APZ_UV";
const UV_HINT: &str =
    "install uv (https://docs.astral.sh/uv/), or set APZ_UV to its path, to run trainers";
const QUICK: Duration = Duration::from_secs(10);
const MAX_DEVICES: usize = 64;
const MAX_ENV: usize = 64;
const MAX_ARGS: usize = 256;
const MAX_MOUNTS: usize = 16;
const MAX_TEXT: usize = 4096;
const STDERR_TAIL: usize = 50;
/// How long a finished job's directory is kept before `GET /v1/train` reaps it.
const REAP_AFTER_SECS: u64 = 24 * 3600;
/// What a job's directory records about the job (`runtime`, `devices`).
const LAUNCH_FILE: &str = "launch.json";

/// An HTTP status and the sentence that goes with it.
type Failure = (u16, String);

fn os_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

/// The uv binary: `$APZ_UV` when it names an absolute file, else the first `uv` on PATH.
fn uv_binary() -> Option<PathBuf> {
    match std::env::var_os(UV_ENV).filter(|value| !value.is_empty()) {
        Some(value) => {
            Some(PathBuf::from(value)).filter(|path| path.is_absolute() && path.is_file())
        }
        None => find_on_path("uv", &std::env::var_os("PATH")?),
    }
}

/// The first absolute entry of `path` holding `name` (with `.exe` on Windows).
fn find_on_path(name: &str, path: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path)
        .filter(|dir| dir.is_absolute())
        .flat_map(|dir| {
            let mut candidates = vec![dir.join(name)];
            if cfg!(windows) {
                candidates.push(dir.join(format!("{name}.exe")));
            }
            candidates
        })
        .find(|candidate| candidate.is_file())
}

/// Whether the uv at `uv` runs.
fn uv_runs(uv: &Path) -> bool {
    uv.to_str()
        .and_then(|program| command_stdout(program, &["--version"]))
        .is_some()
}

/// uv's managed-Python directory, which a job may read: a venv points at its base interpreter
/// there. (uv's cache is not shared: uv writes to it even when it does not sync, so each job gets
/// its own under its directory.)
#[derive(Debug, Default)]
struct UvDirs {
    python: Option<PathBuf>,
}

fn uv_dirs(uv: &Path) -> UvDirs {
    let python = uv
        .to_str()
        .and_then(|program| command_stdout(program, &["python", "dir"]))
        .map(|text| PathBuf::from(text.trim()))
        .filter(|dir| dir.is_absolute() && dir.is_dir());
    UvDirs { python }
}

/// The trainer project: `$APZ_TRAINER_DIR`, else `<state>/trainer`.
fn runner_dir(state: &Path) -> PathBuf {
    std::env::var_os(RUNNER_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| state.join("trainer"))
}

/// Whether the trainer project is there with its environment. The environment is made by
/// `uv sync` (which needs the network), never by a job.
fn check_runner(runner: &Path) -> Result<(), String> {
    if !runner.is_absolute() {
        return Err(format!(
            "{RUNNER_ENV} must be an absolute path to the trainer project"
        ));
    }
    if !runner.join("pyproject.toml").is_file() {
        return Err(format!(
            "no trainer project at {}: point {RUNNER_ENV} at the trainer/ directory",
            runner.display()
        ));
    }
    if !runner.join(".venv").is_dir() {
        return Err(format!(
            "the trainer project has no environment: run uv sync --project {} once",
            runner.display()
        ));
    }
    Ok(())
}

/// The OS sandbox this node has, by its kind; an error when it has none.
fn check_sandbox() -> Result<&'static str, String> {
    let support = apz_sandbox::support();
    if support.kind == "none" {
        Err(format!(
            "trainers run under the OS sandbox, which this node does not have: {}",
            support.detail
        ))
    } else {
        Ok(support.kind)
    }
}

/// The accelerator a node trains on, in NVIDIA, AMD, Intel order.
fn pick_vendor(vendors: &[&str]) -> Option<&'static str> {
    ["nvidia", "amd", "intel"]
        .into_iter()
        .find(|vendor| vendors.contains(vendor))
}

/// The runtime for an accelerator vendor: `apple` is the MPS runner, no vendor is the CPU one.
fn native_runtime(vendor: Option<&str>) -> &'static str {
    match vendor {
        Some("nvidia") => "native-nvidia",
        Some("amd") => "native-amd",
        Some("intel") => "native-intel",
        Some("apple") => "native-mps",
        _ => "native-cpu",
    }
}

/// The device nodes a job needs for the devices it holds, under `dev` (`/dev` on Linux): NVIDIA's
/// control nodes and one node per device, AMD's compute node and DRI directory, Intel's DRI
/// directory. Only the nodes that exist are returned, and none without devices.
fn device_nodes(dev: &Path, vendor: &str, indices: &[u32]) -> Vec<PathBuf> {
    if indices.is_empty() {
        return Vec::new();
    }
    let names: Vec<String> = match vendor {
        "nvidia" => [
            "nvidiactl",
            "nvidia-uvm",
            "nvidia-uvm-tools",
            "nvidia-modeset",
        ]
        .into_iter()
        .map(String::from)
        .chain(indices.iter().map(|index| format!("nvidia{index}")))
        .collect(),
        "amd" => vec!["kfd".to_string(), "dri".to_string()],
        "intel" => vec!["dri".to_string()],
        _ => Vec::new(),
    };
    names
        .into_iter()
        .map(|name| dev.join(name))
        .filter(|path| path.exists())
        .collect()
}

/// Whether the sandbox can give a job the accelerator's device nodes. Linux: the nodes go into a
/// spec and apz-sandbox builds it, without starting anything. Windows: the AppContainer is granted
/// no device, so the accelerator is refused, and the reason says what it would need. macOS: the
/// Seatbelt profile has no device nodes to grant.
fn device_grant(os: &str, vendor: Option<&str>, uv: &Path, state: &Path) -> Result<(), String> {
    match (os, vendor) {
        ("windows", Some(vendor)) => Err(format!(
            "the AppContainer cannot reach the {vendor} GPU: it needs an ACL on the adapter's \
             device interface granted to its package SID, and apz-sandbox grants no device \
             access yet"
        )),
        ("linux", Some(vendor)) => {
            let spec = Spec {
                program: uv.to_path_buf(),
                args: vec!["--version".to_string()],
                env: Vec::new(),
                workdir: state.join("train"),
                read_only: Vec::new(),
                read_write: device_nodes(Path::new("/dev"), vendor, &[0]),
                network: Network::None,
                limits: Limits::default(),
            };
            apz_sandbox::command(&spec).map(|_| ()).map_err(|err| {
                format!("the sandbox cannot give trainers the {vendor} device nodes: {err}")
            })
        }
        _ => Ok(()),
    }
}

/// What the probe read, one check per field. The first failing check is the one the detail names.
struct Inputs {
    os: &'static str,
    vendor: Option<&'static str>,
    uv: Result<(), String>,
    runner: Result<(), String>,
    sandbox: Result<&'static str, String>,
    grant: Result<(), String>,
}

/// The runtime that the checks imply: native, and ready only when every check passes.
fn choose_trainer(input: &Inputs) -> Trainer {
    let runtime = native_runtime(input.vendor);
    let problem = [
        input.uv.as_ref().err(),
        input.runner.as_ref().err(),
        input.sandbox.as_ref().err(),
        input.grant.as_ref().err(),
    ]
    .into_iter()
    .flatten()
    .next();
    if let Some(why) = problem {
        return Trainer {
            runtime: Some(runtime),
            ok: false,
            detail: why.clone(),
        };
    }
    let kind = input.sandbox.as_ref().copied().unwrap_or("sandbox");
    let detail = if input.os == "macos" {
        format!(
            "{runtime} runs trainers under the {kind} sandbox; macOS cannot cap memory, so a \
             job's memory_mib is refused"
        )
    } else {
        format!("{runtime} runs trainers under the {kind} sandbox")
    };
    Trainer {
        runtime: Some(runtime),
        ok: true,
        detail,
    }
}

/// The trainer runtime this node can run jobs in, probed once at start. `vendors` are the
/// accelerators' vendors. Every runtime is native: `uv` runs the trainer under the OS sandbox.
pub fn probe_trainer(vendors: &[&str]) -> Trainer {
    let os = os_name();
    let state = apz_os::dirs::state();
    let vendor = if os == "macos" {
        Some("apple")
    } else {
        pick_vendor(vendors)
    };
    let uv = uv_binary().filter(|uv| uv_runs(uv));
    let sandbox = check_sandbox();
    let grant = match (&uv, &sandbox) {
        (Some(uv), Ok(_)) => device_grant(os, vendor, uv, &state),
        _ => Ok(()),
    };
    choose_trainer(&Inputs {
        os,
        vendor,
        uv: uv.map(|_| ()).ok_or_else(|| UV_HINT.to_string()),
        runner: check_runner(&runner_dir(&state)),
        sandbox,
        grant,
    })
}

/// The `POST /v1/train` body.
#[derive(Debug, Deserialize)]
pub struct StartRequest {
    pub job_id: String,
    /// Accelerator indices of the node's runtime; empty for none. The MPS runner holds device 0
    /// whatever is asked.
    #[serde(default)]
    pub devices: Vec<u32>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Appended to `python -m apz_train.run`.
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub mounts: Vec<Mount>,
    /// `none` (the default). A job gets no network: the node has no egress proxy for jobs.
    #[serde(default)]
    pub network: String,
    #[serde(default)]
    pub limits: RequestLimits,
}

/// The caps a job asks for. `memory_mib` is refused on macOS, which gives the sandbox no per-process
/// memory cap; `processes` counts the user's processes on Unix, not only the job's.
#[derive(Debug, Default, Deserialize)]
pub struct RequestLimits {
    pub memory_mib: Option<u64>,
    pub processes: Option<u32>,
}

/// A bind mount: a path under the state directory, read-only unless `ro` is false. The native
/// runner grants the host path; `container` is checked but not mapped.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Mount {
    pub host: String,
    pub container: String,
    #[serde(default = "read_only")]
    pub ro: bool,
}

fn read_only() -> bool {
    true
}

/// A validated job, ready to run.
#[derive(Debug, PartialEq)]
pub struct Launch {
    pub job_id: String,
    pub runtime: &'static str,
    /// The accelerator indices the job holds.
    pub devices: Vec<u32>,
    pub env: BTreeMap<String, String>,
    pub args: Vec<String>,
    pub mounts: Vec<Mount>,
    pub network: Network,
    pub limits: Limits,
}

/// Checks a start request against the node's runtime, its devices and the state directory.
fn validate(
    request: StartRequest,
    runtime: &'static str,
    available: &[u32],
    state: &Path,
) -> Result<Launch, String> {
    if !valid_job_id(&request.job_id) {
        return Err(
            "job_id is 1-63 letters, digits, '-' or '_', starting with a letter or digit".into(),
        );
    }
    let chosen = check_devices(&request.devices, available)?;
    // The MPS runner holds the Apple GPU whatever the job asks: its one device is index 0.
    let devices = if runtime == "native-mps" {
        vec![0]
    } else {
        chosen
    };
    let network = match request.network.as_str() {
        "" | "none" => Network::None,
        "host" => {
            return Err(
                "network host is not available: a trainer job gets no network, because the node \
                 has no egress proxy for jobs"
                    .into(),
            );
        }
        other => return Err(format!("network is none, not {other:?}")),
    };
    if request.env.len() > MAX_ENV {
        return Err(format!("at most {MAX_ENV} env entries"));
    }
    for (key, value) in &request.env {
        if !valid_env_key(key)
            || value.len() > MAX_TEXT
            || value.chars().any(|c| c == '\0' || c == '\n')
        {
            return Err(format!("env {key:?} is not a KEY with a single-line value"));
        }
    }
    if request.args.len() > MAX_ARGS
        || request
            .args
            .iter()
            .any(|arg| arg.len() > MAX_TEXT || arg.contains('\0'))
    {
        return Err(format!(
            "at most {MAX_ARGS} args of {MAX_TEXT} bytes, without NUL"
        ));
    }
    if request.mounts.len() > MAX_MOUNTS {
        return Err(format!("at most {MAX_MOUNTS} mounts"));
    }
    for mount in &request.mounts {
        check_mount(mount, state)?;
        if !mount.ro && !Path::new(&mount.host).is_dir() {
            return Err(format!(
                "read-write mount {} must be a directory",
                mount.host
            ));
        }
    }
    let memory_bytes = match request.limits.memory_mib {
        None => None,
        Some(_) if cfg!(target_os = "macos") => {
            return Err(
                "limits.memory_mib cannot be enforced on macOS: the kernel gives the sandbox no \
                 per-process memory cap; drop it, or run the job on Linux or Windows"
                    .into(),
            );
        }
        Some(0) => return Err("limits.memory_mib must be at least 1".into()),
        Some(mib) => Some(
            mib.checked_mul(1 << 20)
                .ok_or("limits.memory_mib is too large")?,
        ),
    };
    if request.limits.processes == Some(0) {
        return Err("limits.processes must be at least 1".into());
    }
    Ok(Launch {
        job_id: request.job_id,
        runtime,
        devices,
        env: request.env,
        args: request.args,
        mounts: request.mounts,
        network,
        limits: Limits {
            memory_bytes,
            processes: request.limits.processes,
            cpu_seconds: None,
        },
    })
}

fn valid_job_id(id: &str) -> bool {
    id.len() <= 63
        && id.starts_with(|c: char| c.is_ascii_alphanumeric())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn valid_env_key(key: &str) -> bool {
    key.len() <= 64
        && key.starts_with(|c: char| c == '_' || c.is_ascii_alphabetic())
        && key.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn check_devices(devices: &[u32], available: &[u32]) -> Result<Vec<u32>, String> {
    if devices.len() > MAX_DEVICES {
        return Err(format!("at most {MAX_DEVICES} devices"));
    }
    let mut chosen: Vec<u32> = Vec::with_capacity(devices.len());
    for &device in devices {
        if !available.contains(&device) {
            return Err(format!(
                "device {device} is not an accelerator of this node for this runtime"
            ));
        }
        if chosen.contains(&device) {
            return Err(format!("device {device} is listed twice"));
        }
        chosen.push(device);
    }
    Ok(chosen)
}

/// A path without commas, NUL, newline or `..` parts.
fn plain_path(text: &str) -> bool {
    !text.is_empty()
        && !text.chars().any(|c| matches!(c, ',' | '\0' | '\n'))
        && !text.split(['/', '\\']).any(|part| part == "..")
}

/// A mount's host path must be absolute and exist under the state directory (symlinks resolved);
/// its container path must be absolute.
fn check_mount(mount: &Mount, state: &Path) -> Result<(), String> {
    if !Path::new(&mount.host).is_absolute() || !plain_path(&mount.host) {
        return Err(format!(
            "mount host {:?} must be an absolute path without commas or '..'",
            mount.host
        ));
    }
    if !mount.container.starts_with('/') || !plain_path(&mount.container) {
        return Err(format!(
            "mount container path {:?} must be absolute without commas or '..'",
            mount.container
        ));
    }
    let root = std::fs::canonicalize(state)
        .map_err(|err| format!("state directory {}: {err}", state.display()))?;
    let source =
        std::fs::canonicalize(&mount.host).map_err(|err| format!("mount {}: {err}", mount.host))?;
    if source.starts_with(&root) {
        Ok(())
    } else {
        Err(format!(
            "mount {} is outside the state directory",
            mount.host
        ))
    }
}

/// The env that selects the accelerators a job holds, for its runtime. An empty list hides every
/// device.
fn runtime_env(runtime: &str, devices: &[u32]) -> BTreeMap<String, String> {
    let list = csv(devices);
    let mut env = BTreeMap::new();
    match runtime {
        "native-nvidia" => {
            env.insert("CUDA_VISIBLE_DEVICES".to_string(), list);
            // nvidia-smi numbers the GPUs by PCI bus, and CUDA only does so with this order.
            env.insert("CUDA_DEVICE_ORDER".to_string(), "PCI_BUS_ID".to_string());
        }
        "native-amd" => {
            env.insert("HIP_VISIBLE_DEVICES".to_string(), list.clone());
            env.insert("ROCR_VISIBLE_DEVICES".to_string(), list);
        }
        "native-intel" => {
            env.insert("ZE_AFFINITY_MASK".to_string(), list);
        }
        _ => {}
    }
    env
}

fn csv(devices: &[u32]) -> String {
    devices
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// The sandbox spec of a launch: `uv run` of the trainer project in the job directory. `devices` are
/// the device nodes the job's accelerators need, and `uv_dirs` the uv directories it may read.
fn job_spec(
    launch: &Launch,
    uv: &Path,
    runner: &Path,
    job: &Path,
    uv_dirs: &UvDirs,
    devices: Vec<PathBuf>,
) -> Spec {
    let mut env = launch.env.clone();
    env.extend(runtime_env(launch.runtime, &launch.devices));
    // The job's own home, PATH and uv cache, inside its directory: none of the user's are visible.
    // apz_train is imported from the project (the project is not installed: `package = false`).
    for (key, value) in [
        ("HOME", job.join("home")),
        ("PATH", job.join("bin")),
        ("UV_CACHE_DIR", job.join("cache")),
        ("APZ_JOB_DIR", job.to_path_buf()),
        ("PYTHONPATH", runner.to_path_buf()),
    ] {
        env.insert(key.to_string(), value.display().to_string());
    }
    let mut args: Vec<String> = ["run", "--frozen", "--no-sync", "--project"]
        .map(String::from)
        .into();
    args.push(runner.display().to_string());
    args.extend(["python", "-m", "apz_train.run"].map(String::from));
    args.extend(launch.args.iter().cloned());
    let mut read_only = vec![runner.to_path_buf()];
    read_only.extend(uv_dirs.python.clone());
    let mut read_write = vec![job.to_path_buf()];
    read_write.extend(devices);
    for mount in &launch.mounts {
        let host = PathBuf::from(&mount.host);
        if mount.ro {
            read_only.push(host);
        } else {
            read_write.push(host);
        }
    }
    Spec {
        program: uv.to_path_buf(),
        args,
        env: env.into_iter().collect(),
        workdir: job.to_path_buf(),
        read_only,
        read_write,
        network: launch.network,
        limits: launch.limits,
    }
}

/// What `launch.json` records about a job.
#[derive(Debug, Serialize, Deserialize)]
struct Record {
    runtime: String,
    devices: Vec<u32>,
}

/// A job whose process is alive.
struct Running {
    job_id: String,
    runtime: String,
    devices: Vec<u32>,
}

fn running(state: &Path) -> Vec<Running> {
    let Ok(entries) = std::fs::read_dir(state.join("train")) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let dir = entry.path();
            if !native_pid(&dir).is_some_and(native_alive) {
                return None;
            }
            let record: Record =
                serde_json::from_slice(&std::fs::read(dir.join(LAUNCH_FILE)).ok()?).ok()?;
            Some(Running {
                job_id: entry.file_name().to_string_lossy().into_owned(),
                runtime: record.runtime,
                devices: record.devices,
            })
        })
        .collect()
}

/// The accelerators a running job holds on this node, as (vendor, index). Used to mark `busy` in
/// `GET /v1/machine`.
pub fn busy_devices() -> Vec<(&'static str, u32)> {
    busy_in(&apz_os::dirs::state())
}

fn busy_in(state: &Path) -> Vec<(&'static str, u32)> {
    running(state)
        .iter()
        .flat_map(|job| {
            let vendor = runtime_vendor(&job.runtime);
            job.devices
                .iter()
                .filter_map(move |&index| vendor.map(|vendor| (vendor, index)))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The accelerator vendor a runtime trains on.
fn runtime_vendor(runtime: &str) -> Option<&'static str> {
    match runtime {
        "native-nvidia" => Some("nvidia"),
        "native-amd" => Some("amd"),
        "native-intel" => Some("intel"),
        "native-mps" => Some("apple"),
        _ => None,
    }
}

/// The device indices of the runtime's vendor that this node has.
fn available(accelerators: &[Accelerator], runtime: &str) -> Vec<u32> {
    let Some(vendor) = runtime_vendor(runtime) else {
        return Vec::new();
    };
    accelerators
        .iter()
        .filter(|device| device.vendor == vendor)
        .map(|device| device.index)
        .collect()
}

fn native_dir(state: &Path, job_id: &str) -> PathBuf {
    state.join("train").join(job_id)
}

fn native_pid(dir: &Path) -> Option<u32> {
    std::fs::read_to_string(dir.join("pid"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

#[cfg(not(windows))]
fn native_alive(pid: u32) -> bool {
    let pid = pid.to_string();
    command_stdout("ps", &["-p", pid.as_str(), "-o", "pid="]).is_some_and(|out| !out.is_empty())
}

#[cfg(windows)]
fn native_alive(pid: u32) -> bool {
    let filter = format!("PID eq {pid}");
    command_stdout("tasklist", &["/FI", filter.as_str(), "/NH"])
        .is_some_and(|out| out.split_whitespace().any(|token| token == pid.to_string()))
}

/// The exit code the job's `exit` file holds; None while it runs or when it was killed.
fn native_exit_code(dir: &Path) -> Option<i64> {
    std::fs::read_to_string(dir.join("exit"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// The text the `exit` file holds: the exit code, or `signal` when the process was killed.
fn exit_text(status: ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| "signal".to_string(), |code| code.to_string())
}

/// `POST /v1/train` → 201 `{job_id, pid, runtime, devices}`.
pub fn start(node: &Node, body: &[u8]) -> Response {
    respond(201, start_job(node, body))
}

fn start_job(node: &Node, body: &[u8]) -> Result<Value, Failure> {
    let request: StartRequest = serde_json::from_slice(body)
        .map_err(|err| (400, format!("invalid train request: {err}")))?;
    let trainer = &node.probed.facts.trainer;
    let runtime = trainer.runtime.filter(|_| trainer.ok).ok_or_else(|| {
        (
            409,
            format!("the trainer runtime is not ready: {}", trainer.detail),
        )
    })?;
    let state = apz_os::dirs::state();
    let launch = validate(
        request,
        runtime,
        &available(&node.probed.facts.accelerators, runtime),
        &state,
    )
    .map_err(|reason| (400, reason))?;
    let uv = uv_binary().ok_or_else(|| (409, format!("uv is not found: {UV_HINT}")))?;
    let runner = runner_dir(&state);
    check_runner(&runner).map_err(|why| (409, why))?;
    start_native(&launch, &state, &runner, &uv)
}

/// Starts a job in its own directory. A job that cannot start leaves no directory behind.
fn start_native(launch: &Launch, state: &Path, runner: &Path, uv: &Path) -> Result<Value, Failure> {
    let train = state.join("train");
    std::fs::create_dir_all(&train)
        .map_err(|err| (500, format!("job directory {}: {err}", train.display())))?;
    let dir = native_dir(state, &launch.job_id);
    std::fs::create_dir(&dir).map_err(|err| {
        if err.kind() == ErrorKind::AlreadyExists {
            (
                409,
                format!(
                    "job {} exists: DELETE /v1/train/{} once it is read, or pick another id",
                    launch.job_id, launch.job_id
                ),
            )
        } else {
            (500, format!("job directory {}: {err}", dir.display()))
        }
    })?;
    match spawn_job(launch, &dir, runner, uv) {
        Ok(pid) => Ok(json!({
            "job_id": launch.job_id,
            "pid": pid,
            "runtime": launch.runtime,
            "devices": launch.devices,
        })),
        Err(failure) => {
            let _ = std::fs::remove_dir_all(&dir);
            Err(failure)
        }
    }
}

/// Writes the job's record and starts `uv run` under the sandbox, its output in the directory. A
/// waiter thread writes `exit` when the process ends.
fn spawn_job(launch: &Launch, dir: &Path, runner: &Path, uv: &Path) -> Result<u32, Failure> {
    let io = |err: std::io::Error| (500, format!("job directory {}: {err}", dir.display()));
    for sub in ["home", "bin", "cache"] {
        std::fs::create_dir_all(dir.join(sub)).map_err(io)?;
    }
    let record = Record {
        runtime: launch.runtime.to_string(),
        devices: launch.devices.clone(),
    };
    std::fs::write(
        dir.join(LAUNCH_FILE),
        serde_json::to_vec(&record).unwrap_or_default(),
    )
    .map_err(io)?;
    let events = File::create(dir.join("events.jsonl")).map_err(io)?;
    let stderr = File::create(dir.join("stderr.log")).map_err(io)?;
    #[cfg(target_os = "linux")]
    let devices = device_nodes(
        Path::new("/dev"),
        runtime_vendor(launch.runtime).unwrap_or(""),
        &launch.devices,
    );
    #[cfg(not(target_os = "linux"))]
    let devices = Vec::new();
    let spec = job_spec(launch, uv, runner, dir, &uv_dirs(uv), devices);
    let (mut child, pumps) = start_child(&spec, events, stderr)
        .map_err(|err| (502, format!("trainer sandbox: {err}")))?;
    let pid = child.id();
    if let Err(err) = std::fs::write(dir.join("pid"), pid.to_string()) {
        let _ = child.kill();
        return Err((500, format!("pid file: {err}")));
    }
    let exit_file = dir.join("exit");
    thread::spawn(move || {
        let status = child.wait();
        for handle in pumps {
            let _ = handle.join();
        }
        if let Ok(status) = status {
            let _ = std::fs::write(exit_file, exit_text(status));
        }
    });
    Ok(pid)
}

/// Starts the sandboxed job with its output going to the job's files. Unix hands the files to the
/// child, so the output keeps going there when noded restarts. Windows always pipes the streams,
/// so they are copied into the files here.
#[cfg(unix)]
fn start_child(
    spec: &Spec,
    events: File,
    stderr: File,
) -> Result<(Sandboxed, Vec<JoinHandle<()>>), apz_sandbox::SandboxError> {
    let child = apz_sandbox::spawn(
        spec,
        Stdio::null(),
        Stdio::from(events),
        Stdio::from(stderr),
    )?;
    Ok((child, Vec::new()))
}

#[cfg(not(unix))]
fn start_child(
    spec: &Spec,
    events: File,
    stderr: File,
) -> Result<(Sandboxed, Vec<JoinHandle<()>>), apz_sandbox::SandboxError> {
    let mut child = apz_sandbox::spawn(spec, Stdio::null(), Stdio::piped(), Stdio::piped())?;
    let mut pumps = Vec::new();
    if let Some(out) = child.stdout() {
        pumps.push(pump(out, events));
    }
    if let Some(err) = child.stderr() {
        pumps.push(pump(err, stderr));
    }
    Ok((child, pumps))
}

/// Copies one of the child's streams into a job file until the stream closes.
#[cfg(not(unix))]
fn pump(mut from: Box<dyn std::io::Read + Send>, mut to: File) -> JoinHandle<()> {
    thread::spawn(move || {
        let _ = std::io::copy(&mut from, &mut to);
    })
}

/// How long a finished job's directory is kept before `GET /v1/train` reaps it.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// `GET /v1/train` → the running jobs. Reaps the finished jobs older than 24 h first.
pub fn list() -> Response {
    let state = apz_os::dirs::state();
    reap(&state);
    let jobs: Vec<Value> = running(&state)
        .into_iter()
        .map(|job| json!({"job_id": job.job_id, "runtime": job.runtime, "devices": job.devices}))
        .collect();
    Response::json(200, &jobs)
}

/// Removes the directories of finished jobs more than `REAP_AFTER_SECS` ago. Runs on each listing,
/// so no thread is needed.
fn reap(state: &Path) {
    let cutoff = now_secs().saturating_sub(REAP_AFTER_SECS);
    let Ok(entries) = std::fs::read_dir(state.join("train")) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let dir = entry.path();
        if native_pid(&dir).is_some_and(native_alive) {
            continue;
        }
        let finished = ["exit", "pid"]
            .iter()
            .find_map(|name| std::fs::metadata(dir.join(name)).ok())
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_secs());
        if finished.is_some_and(|at| at < cutoff) {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// `GET /v1/train/{job_id}/events?after=N` → `{job_id, state, exit_code, next, events, stderr}`:
/// the stdout lines after line N as events, `next` the line count to ask from next time, the last
/// lines of stderr for diagnosis. `state` is `running` or `exited` (with `exit_code`); an exited
/// job's events stay readable until `DELETE`.
pub fn events(job_id: &str, after: u64) -> Response {
    respond(200, read_events(&apz_os::dirs::state(), job_id, after))
}

fn read_events(state: &Path, job_id: &str, after: u64) -> Result<Value, Failure> {
    check_job_id(job_id)?;
    let dir = native_dir(state, job_id);
    if !dir.exists() {
        return Err((404, format!("no job {job_id}")));
    }
    let alive = native_pid(&dir).is_some_and(native_alive);
    let exit_code = if alive { None } else { native_exit_code(&dir) };
    let read = |name: &str| {
        std::fs::read(dir.join(name))
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default()
    };
    let (events, next) = event_lines(&read("events.jsonl"), after);
    let stderr = read("stderr.log");
    let lines: Vec<&str> = stderr.lines().collect();
    let tail = &lines[lines.len().saturating_sub(STDERR_TAIL)..];
    let state_name = if alive { "running" } else { "exited" };
    Ok(json!({
        "job_id": job_id,
        "state": state_name,
        "exit_code": exit_code,
        "next": next,
        "events": events,
        "stderr": tail,
    }))
}

/// The events of stdout's lines after `after`, and the line count. A JSON object is an event as it
/// is, with its `line`; any other line is `{"ev":"log","text":…,"line":…}`. Blank lines are skipped.
fn event_lines(stdout: &str, after: u64) -> (Vec<Value>, u64) {
    let mut events = Vec::new();
    let mut next = 0;
    for (index, line) in stdout.lines().enumerate() {
        let line_no = index as u64 + 1;
        next = line_no;
        if line_no <= after || line.trim().is_empty() {
            continue;
        }
        let event = match serde_json::from_str::<Value>(line.trim()) {
            Ok(Value::Object(mut object)) => {
                object.insert("line".to_string(), json!(line_no));
                Value::Object(object)
            }
            _ => json!({"ev": "log", "text": line, "line": line_no}),
        };
        events.push(event);
    }
    (events, next)
}

/// `POST /v1/train/{job_id}/stop`: stops the job's process and what it started. Its directory is
/// kept until `DELETE`, so its events are still read.
pub fn stop(job_id: &str) -> Response {
    respond(200, stop_native(&apz_os::dirs::state(), job_id))
}

fn stop_native(state: &Path, job_id: &str) -> Result<Value, Failure> {
    check_job_id(job_id)?;
    let pid = native_pid(&native_dir(state, job_id))
        .filter(|&pid| native_alive(pid))
        .ok_or_else(|| (404, format!("no running job {job_id}")))?;
    if kill_tree(pid) {
        Ok(json!({"job_id": job_id, "pid": pid, "stopped": true}))
    } else {
        Err((502, format!("could not stop pid {pid}")))
    }
}

/// Terminates a process and every process below it: SIGTERM on Unix, `taskkill /T /F` on Windows.
#[cfg(not(windows))]
fn kill_tree(pid: u32) -> bool {
    let mut targets = vec!["-TERM".to_string(), pid.to_string()];
    targets.extend(descendants(pid).iter().map(u32::to_string));
    let refs: Vec<&str> = targets.iter().map(String::as_str).collect();
    command_within("kill", &refs, QUICK).is_some_and(|out| out.ok)
}

#[cfg(windows)]
fn kill_tree(pid: u32) -> bool {
    let pid = pid.to_string();
    command_within("taskkill", &["/PID", pid.as_str(), "/T", "/F"], QUICK).is_some_and(|out| out.ok)
}

/// The processes below `root` in the process table.
#[cfg(not(windows))]
fn descendants(root: u32) -> Vec<u32> {
    let Some(table) = command_stdout("ps", &["-A", "-o", "pid=", "-o", "ppid="]) else {
        return Vec::new();
    };
    let pairs: Vec<(u32, u32)> = table
        .lines()
        .filter_map(|line| {
            let fields: Vec<u32> = line
                .split_whitespace()
                .filter_map(|field| field.parse().ok())
                .collect();
            match fields[..] {
                [pid, ppid] => Some((pid, ppid)),
                _ => None,
            }
        })
        .collect();
    let mut found = Vec::new();
    let mut parents = vec![root];
    while let Some(parent) = parents.pop() {
        for &(pid, ppid) in &pairs {
            if ppid == parent && pid != root && !found.contains(&pid) {
                found.push(pid);
                parents.push(pid);
            }
        }
    }
    found
}

/// `DELETE /v1/train/{job_id}`: removes a finished job's directory, with its events and exit code.
/// A running job is a 409: stop it first.
pub fn delete(job_id: &str) -> Response {
    respond(200, delete_native(&apz_os::dirs::state(), job_id))
}

fn delete_native(state: &Path, job_id: &str) -> Result<Value, Failure> {
    check_job_id(job_id)?;
    let dir = native_dir(state, job_id);
    if !dir.exists() {
        return Err((404, format!("no job {job_id}")));
    }
    if native_pid(&dir).is_some_and(native_alive) {
        return Err((409, format!("job {job_id} is running: stop it first")));
    }
    std::fs::remove_dir_all(&dir)
        .map_err(|err| (500, format!("job directory {}: {err}", dir.display())))?;
    Ok(json!({"job_id": job_id, "deleted": true}))
}

fn check_job_id(job_id: &str) -> Result<(), Failure> {
    if valid_job_id(job_id) {
        Ok(())
    } else {
        Err((400, "invalid job id".to_string()))
    }
}

/// A handler's result as the response: the body with `status`, or the failure's status and sentence.
fn respond(status: u16, result: Result<Value, Failure>) -> Response {
    match result {
        Ok(body) => Response::json(status, &body),
        Err((code, message)) => Response::error(code, &message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(job: &str) -> StartRequest {
        StartRequest {
            job_id: job.into(),
            devices: Vec::new(),
            env: BTreeMap::new(),
            args: Vec::new(),
            mounts: Vec::new(),
            network: String::new(),
            limits: RequestLimits::default(),
        }
    }

    /// A throwaway directory under the temp dir, unique to this run.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apz-train-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch directory");
        dir
    }

    /// Inputs where every check passes.
    fn ready(os: &'static str, vendor: Option<&'static str>) -> Inputs {
        Inputs {
            os,
            vendor,
            uv: Ok(()),
            runner: Ok(()),
            sandbox: Ok("landlock"),
            grant: Ok(()),
        }
    }

    #[test]
    fn the_runtime_is_native_for_each_accelerator() {
        let cases = [
            ("linux", Some("nvidia"), "native-nvidia"),
            ("linux", Some("amd"), "native-amd"),
            ("linux", Some("intel"), "native-intel"),
            ("macos", Some("apple"), "native-mps"),
            ("linux", None, "native-cpu"),
        ];
        for (os, vendor, runtime) in cases {
            let trainer = choose_trainer(&ready(os, vendor));
            assert_eq!(
                (trainer.runtime, trainer.ok),
                (Some(runtime), true),
                "{}",
                trainer.detail
            );
        }
        let mac = choose_trainer(&ready("macos", Some("apple")));
        assert!(mac.detail.contains("memory"), "{}", mac.detail);
    }

    #[test]
    fn a_failed_check_is_named_in_the_detail() {
        let mut no_uv = ready("linux", Some("nvidia"));
        no_uv.uv = Err(UV_HINT.to_string());
        let trainer = choose_trainer(&no_uv);
        assert_eq!(
            (trainer.runtime, trainer.ok),
            (Some("native-nvidia"), false)
        );
        assert!(trainer.detail.contains("install uv"), "{}", trainer.detail);

        let mut no_runner = ready("linux", None);
        no_runner.runner = Err("no trainer project".into());
        assert!(!choose_trainer(&no_runner).ok);

        let mut no_sandbox = ready("linux", None);
        no_sandbox.sandbox = Err("Landlock is not available".into());
        let trainer = choose_trainer(&no_sandbox);
        assert!(trainer.detail.contains("Landlock"), "{}", trainer.detail);

        let mut no_grant = ready("linux", Some("nvidia"));
        no_grant.grant = Err("the sandbox cannot give trainers the nvidia device nodes".into());
        assert!(!choose_trainer(&no_grant).ok);
    }

    #[test]
    fn an_appcontainer_is_refused_the_gpu_until_it_is_granted_a_device() {
        let why = device_grant(
            "windows",
            Some("nvidia"),
            Path::new("uv"),
            Path::new("state"),
        )
        .expect_err("refused");
        assert!(why.contains("AppContainer"), "{why}");
        assert!(device_grant("windows", None, Path::new("uv"), Path::new("state")).is_ok());
    }

    #[test]
    fn device_nodes_follow_the_devices_a_job_holds() {
        let dev = scratch("dev");
        for name in ["nvidiactl", "nvidia-uvm", "nvidia0", "nvidia1", "kfd"] {
            std::fs::write(dev.join(name), b"").expect("device node");
        }
        std::fs::create_dir(dev.join("dri")).expect("dri");
        assert_eq!(
            device_nodes(&dev, "nvidia", &[1]),
            vec![
                dev.join("nvidiactl"),
                dev.join("nvidia-uvm"),
                dev.join("nvidia1")
            ]
        );
        assert_eq!(
            device_nodes(&dev, "amd", &[0]),
            vec![dev.join("kfd"), dev.join("dri")]
        );
        assert_eq!(device_nodes(&dev, "intel", &[0]), vec![dev.join("dri")]);
        assert!(
            device_nodes(&dev, "nvidia", &[]).is_empty(),
            "no devices, no nodes"
        );
        let _ = std::fs::remove_dir_all(&dev);
    }

    #[test]
    fn a_job_runs_uv_in_its_own_directory_with_its_own_home() {
        let root = scratch("spec");
        let state = root.join("state");
        let runner = root.join("trainer");
        let job = state.join("train").join("job-1");
        let uv = root.join("bin").join("uv");
        let mut req = request("job-1");
        req.env.insert("RECIPE".into(), "tiny".into());
        req.env.insert("HOME".into(), "/etc".into());
        req.args = vec!["--job".into(), "job.json".into()];
        let launch = validate(req, "native-cpu", &[], &state).expect("valid");
        let uv_dirs = UvDirs {
            python: Some(root.join("python")),
        };
        let spec = job_spec(&launch, &uv, &runner, &job, &uv_dirs, Vec::new());

        assert_eq!(spec.program, uv);
        assert_eq!(
            spec.args[..4].to_vec(),
            vec!["run", "--frozen", "--no-sync", "--project"]
        );
        assert_eq!(spec.args[4], runner.display().to_string());
        assert_eq!(
            spec.args[5..].to_vec(),
            vec!["python", "-m", "apz_train.run", "--job", "job.json"]
        );
        let env: BTreeMap<String, String> = spec.env.iter().cloned().collect();
        assert_eq!(env["HOME"], job.join("home").display().to_string());
        assert_eq!(env["PATH"], job.join("bin").display().to_string());
        assert_eq!(env["UV_CACHE_DIR"], job.join("cache").display().to_string());
        assert_eq!(env["PYTHONPATH"], runner.display().to_string());
        assert_eq!(env["APZ_JOB_DIR"], job.display().to_string());
        assert_eq!(env["RECIPE"], "tiny");
        assert_eq!(spec.workdir, job);
        assert_eq!(spec.read_only, vec![runner.clone(), root.join("python")]);
        assert!(spec.read_write.contains(&job));
        assert_eq!(spec.network, Network::None);
        assert_eq!(spec.limits, Limits::default());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_job_keeps_its_uv_cache_in_its_own_directory() {
        let root = scratch("cache");
        let job = root.join("train").join("job-2");
        let launch = validate(request("job-2"), "native-cpu", &[], &root).expect("valid");
        let spec = job_spec(
            &launch,
            &root.join("uv"),
            &root.join("trainer"),
            &job,
            &UvDirs::default(),
            Vec::new(),
        );
        let env: BTreeMap<String, String> = spec.env.iter().cloned().collect();
        assert_eq!(env["UV_CACHE_DIR"], job.join("cache").display().to_string());
        assert_eq!(spec.read_only, vec![root.join("trainer")]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_job_is_validated_before_it_runs() {
        let state = scratch("validate");
        let ok = |job: StartRequest, runtime: &'static str, devices: &[u32]| {
            validate(job, runtime, devices, &state)
        };
        assert!(ok(request("bad id!"), "native-cpu", &[]).is_err());
        assert!(ok(request("-leading"), "native-cpu", &[]).is_err());
        let mut unknown = request("j");
        unknown.devices = vec![3];
        assert!(ok(unknown, "native-nvidia", &[0, 1]).is_err());
        let mut twice = request("j");
        twice.devices = vec![0, 0];
        assert!(ok(twice, "native-nvidia", &[0]).is_err());
        let mut on_cpu = request("j");
        on_cpu.devices = vec![0];
        assert!(
            ok(on_cpu, "native-cpu", &[]).is_err(),
            "the cpu runtime has no devices"
        );
        let mut bad_key = request("j");
        bad_key.env.insert("9X".into(), "v".into());
        assert!(ok(bad_key, "native-cpu", &[]).is_err());
        let mut newline = request("j");
        newline.env.insert("OK_KEY".into(), "a\nb".into());
        assert!(ok(newline, "native-cpu", &[]).is_err());
        let mut bridge = request("j");
        bridge.network = "bridge".into();
        assert!(ok(bridge, "native-cpu", &[]).is_err());
        let mut host = request("h");
        host.network = "host".into();
        assert!(
            ok(host, "native-cpu", &[]).is_err(),
            "no job gets the host network"
        );
        assert_eq!(
            ok(request("h"), "native-cpu", &[]).expect("valid").network,
            Network::None
        );
        let mut missing = request("m");
        missing.mounts.push(Mount {
            host: "/nonexistent/x".into(),
            container: "/data".into(),
            ro: true,
        });
        assert!(ok(missing, "native-cpu", &[]).is_err());
        let file = state.join("file.txt");
        std::fs::write(&file, b"").expect("file");
        let mut writable_file = request("f");
        writable_file.mounts.push(Mount {
            host: file.display().to_string(),
            container: "/data".into(),
            ro: false,
        });
        assert!(
            ok(writable_file, "native-cpu", &[]).is_err(),
            "a read-write mount is a directory"
        );
        let mut mounted = request("mps");
        mounted.mounts.push(Mount {
            host: state.display().to_string(),
            container: "/data".into(),
            ro: true,
        });
        assert!(ok(mounted, "native-mps", &[0]).is_ok());
        let _ = std::fs::remove_dir_all(&state);
    }

    /// The spec a job on macOS gets: uv in its directory, the MPS device held, no device nodes (the
    /// Seatbelt profile has none to grant), and a memory cap refused before anything starts.
    #[cfg(target_os = "macos")]
    #[test]
    fn on_macos_an_mps_job_spec_grants_no_device_nodes() {
        let state = scratch("mac-spec");
        let job = state.join("train").join("mac-spec-1");
        let launch = validate(request("mac-spec-1"), "native-mps", &[0], &state).expect("valid");
        let spec = job_spec(
            &launch,
            &state.join("uv"),
            &state.join("trainer"),
            &job,
            &UvDirs::default(),
            Vec::new(),
        );
        assert_eq!(spec.read_write, vec![job.clone()]);
        assert_eq!(spec.network, Network::None);
        assert!(!spec.env.iter().any(|(key, _)| key.starts_with("CUDA_")));
        let mut capped = request("mac-spec-2");
        capped.limits.memory_mib = Some(64);
        assert!(validate(capped, "native-mps", &[0], &state).is_err());
        let _ = std::fs::remove_dir_all(&state);
    }

    /// The spec a Linux NVIDIA job gets: the device nodes it holds are read-write grants, CUDA sees
    /// only those devices, and the network stays off.
    #[cfg(target_os = "linux")]
    #[test]
    fn on_linux_an_nvidia_job_spec_grants_its_device_nodes() {
        let state = scratch("linux-spec");
        let job = state.join("train").join("linux-spec-1");
        let mut req = request("linux-spec-1");
        req.devices = vec![1];
        let launch = validate(req, "native-nvidia", &[0, 1], &state).expect("valid");
        let nodes = vec![
            PathBuf::from("/dev/nvidiactl"),
            PathBuf::from("/dev/nvidia1"),
        ];
        let spec = job_spec(
            &launch,
            &state.join("uv"),
            &state.join("trainer"),
            &job,
            &UvDirs::default(),
            nodes.clone(),
        );
        assert_eq!(spec.read_write, [vec![job.clone()], nodes].concat());
        let env: BTreeMap<String, String> = spec.env.iter().cloned().collect();
        assert_eq!(env["CUDA_VISIBLE_DEVICES"], "1");
        assert_eq!(spec.network, Network::None);
        let _ = std::fs::remove_dir_all(&state);
    }

    /// The spec a Windows job gets: no network (the AppContainer refuses a loopback port) and no
    /// device nodes, since the AppContainer is granted none.
    #[cfg(windows)]
    #[test]
    fn on_windows_a_job_spec_has_no_network_and_no_device_nodes() {
        let state = scratch("win-spec");
        let job = state.join("train").join("win-spec-1");
        let launch = validate(request("win-spec-1"), "native-cpu", &[], &state).expect("valid");
        let spec = job_spec(
            &launch,
            &state.join("uv.exe"),
            &state.join("trainer"),
            &job,
            &UvDirs::default(),
            Vec::new(),
        );
        assert_eq!(spec.network, Network::None);
        assert_eq!(spec.read_write, vec![job.clone()]);
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn the_mps_runner_holds_device_zero() {
        let state = scratch("mps");
        let launch = validate(request("mac-1"), "native-mps", &[0], &state).expect("valid");
        assert_eq!(
            (launch.runtime, launch.devices.clone()),
            ("native-mps", vec![0])
        );
        let mut asks = request("mac-2");
        asks.devices = vec![0];
        assert_eq!(
            validate(asks, "native-mps", &[0], &state)
                .expect("valid")
                .devices,
            vec![0]
        );
        let mut other = request("mac-3");
        other.devices = vec![1];
        assert!(validate(other, "native-mps", &[0], &state).is_err());
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn a_memory_cap_is_refused_where_the_sandbox_cannot_enforce_it() {
        let state = scratch("memory");
        let mut req = request("mem-1");
        req.limits.memory_mib = Some(512);
        let result = validate(req, "native-cpu", &[], &state);
        if cfg!(target_os = "macos") {
            let why = result.expect_err("refused on macOS");
            assert!(why.contains("macOS"), "{why}");
        } else {
            let launch = result.expect("valid");
            assert_eq!(launch.limits.memory_bytes, Some(512 << 20));
        }
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn the_accelerators_are_selected_per_runtime() {
        let nvidia = runtime_env("native-nvidia", &[0, 2]);
        assert_eq!(nvidia["CUDA_VISIBLE_DEVICES"], "0,2");
        assert_eq!(nvidia["CUDA_DEVICE_ORDER"], "PCI_BUS_ID");
        assert_eq!(
            runtime_env("native-nvidia", &[])["CUDA_VISIBLE_DEVICES"],
            "",
            "no devices hides every GPU"
        );
        assert_eq!(runtime_env("native-amd", &[1])["HIP_VISIBLE_DEVICES"], "1");
        assert_eq!(runtime_env("native-intel", &[0])["ZE_AFFINITY_MASK"], "0");
        assert!(runtime_env("native-mps", &[0]).is_empty());
    }

    #[test]
    fn busy_devices_come_from_the_running_jobs_records() {
        let state = scratch("busy");
        let job = state.join("train").join("busy-1");
        std::fs::create_dir_all(&job).expect("job directory");
        std::fs::write(job.join("pid"), std::process::id().to_string()).expect("pid");
        std::fs::write(
            job.join(LAUNCH_FILE),
            r#"{"runtime":"native-nvidia","devices":[1]}"#,
        )
        .expect("record");
        assert_eq!(busy_in(&state), vec![("nvidia", 1)]);
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn no_code_path_invokes_docker() {
        let source = include_str!("train.rs");
        let code = source.split("#[cfg(test)]").next().unwrap_or_default();
        let mentions: Vec<&str> = code
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .filter(|line| line.to_ascii_lowercase().contains("docker"))
            .collect();
        assert!(mentions.is_empty(), "{mentions:?}");
    }

    #[test]
    fn mounts_must_sit_under_the_state_directory() {
        let root = scratch("mounts");
        let state = root.join("state");
        let outside = root.join("outside");
        std::fs::create_dir_all(state.join("data")).expect("state");
        std::fs::create_dir_all(&outside).expect("outside");
        let inside = Mount {
            host: state.join("data").display().to_string(),
            container: "/data".into(),
            ro: true,
        };
        assert!(check_mount(&inside, &state).is_ok());
        let escape = Mount {
            host: outside.display().to_string(),
            container: "/data".into(),
            ro: true,
        };
        assert!(check_mount(&escape, &state).is_err());
        let dotdot = Mount {
            host: format!("{}/../outside", state.display()),
            container: "/data".into(),
            ro: true,
        };
        assert!(check_mount(&dotdot, &state).is_err());
        let comma = Mount {
            host: format!("{},x", state.join("data").display()),
            container: "/data".into(),
            ro: true,
        };
        assert!(check_mount(&comma, &state).is_err());
        let relative = Mount {
            host: "state/data".into(),
            container: "/data".into(),
            ro: true,
        };
        assert!(check_mount(&relative, &state).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn events_are_json_objects_or_log_lines_after_a_line_number() {
        let stdout = "starting\n{\"ev\":\"step\",\"step\":1,\"loss\":0.5}\n\n{\"ev\":\"done\"}\n";
        let (all, next) = event_lines(stdout, 0);
        assert_eq!(next, 4);
        assert_eq!(all[0], json!({"ev": "log", "text": "starting", "line": 1}));
        assert_eq!(
            all[1],
            json!({"ev": "step", "step": 1, "loss": 0.5, "line": 2})
        );
        assert_eq!(all[2], json!({"ev": "done", "line": 4}));
        let (later, next) = event_lines(stdout, 2);
        assert_eq!(later.len(), 1);
        assert_eq!(next, 4);
        let (none, next) = event_lines("", 0);
        assert!(none.is_empty());
        assert_eq!(next, 0);
    }

    #[test]
    fn job_ids_are_short_and_charset_limited() {
        assert!(valid_job_id("job_1-a"));
        assert!(!valid_job_id(""));
        assert!(!valid_job_id("a/b"));
        assert!(!valid_job_id(&"x".repeat(64)));
    }

    /// A stand-in trainer project: `apz_train.run` prints two events; `--sleep` waits, and
    /// `--probe <path>` tries to write a file there and reports whether it could.
    #[cfg(target_os = "macos")]
    const STAND_IN_RUN: &str = r#"import json
import sys
import time

if "--sleep" in sys.argv:
    print(json.dumps({"ev": "step", "step": 1}), flush=True)
    time.sleep(600)
elif "--probe" in sys.argv:
    path = sys.argv[sys.argv.index("--probe") + 1]
    try:
        with open(path, "w") as handle:
            handle.write("escaped")
        ok = True
    except OSError:
        ok = False
    print(json.dumps({"ev": "probe", "ok": ok}), flush=True)
else:
    print(json.dumps({"ev": "step", "step": 1}), flush=True)
    print(json.dumps({"ev": "done"}), flush=True)
"#;

    /// Writes the stand-in project and makes its environment with `uv sync`, as the installer does.
    #[cfg(target_os = "macos")]
    fn stand_in_trainer(runner: &Path, uv: &Path) -> bool {
        std::fs::create_dir_all(runner.join("apz_train")).expect("stand-in package");
        std::fs::write(
            runner.join("pyproject.toml"),
            "[project]\nname = \"apz-train\"\nversion = \"0.1.0\"\nrequires-python = \">=3.9\"\ndependencies = []\n\n[tool.uv]\npackage = false\n",
        )
        .expect("pyproject");
        std::fs::write(runner.join("apz_train").join("__init__.py"), "").expect("init");
        std::fs::write(runner.join("apz_train").join("run.py"), STAND_IN_RUN).expect("run");
        std::process::Command::new(uv)
            .args(["sync", "--project"])
            .arg(runner)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    /// Polls the job until it has exited and its `exit` file is written (the file follows the
    /// process by a moment), for at most 30 s.
    #[cfg(target_os = "macos")]
    fn wait_for_exit(state: &Path, job_id: &str) -> Value {
        for _ in 0..300 {
            let read = read_events(state, job_id, 0).expect("events");
            if read["state"] == "exited" && native_dir(state, job_id).join("exit").exists() {
                return read;
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!("job {job_id} did not exit");
    }

    /// Stops a job the test started, if the test fails before it stops the job itself.
    #[cfg(target_os = "macos")]
    struct StopOnDrop<'a> {
        state: &'a Path,
        job_id: String,
    }

    #[cfg(target_os = "macos")]
    impl Drop for StopOnDrop<'_> {
        fn drop(&mut self) {
            let _ = stop_native(self.state, &self.job_id);
        }
    }

    /// The native runner on macOS, for real: uv runs a stand-in trainer under Seatbelt. A job prints
    /// its events and exits 0; a write outside its directory is refused; `stop` ends the job and
    /// the process it started. Skipped, with a message, when uv or Seatbelt is missing.
    #[cfg(target_os = "macos")]
    #[test]
    fn native_jobs_run_their_trainer_under_the_sandbox() {
        let Some(uv) = uv_binary().filter(|uv| uv_runs(uv)) else {
            eprintln!("skipped: uv is not installed (brew install uv)");
            return;
        };
        if apz_sandbox::support().kind != "seatbelt" {
            eprintln!("skipped: no Seatbelt sandbox on this Mac");
            return;
        }
        let root = scratch("e2e");
        let runner = root.join("trainer");
        if !stand_in_trainer(&runner, &uv) {
            eprintln!("skipped: uv sync of the stand-in trainer failed");
            let _ = std::fs::remove_dir_all(&root);
            return;
        }
        let state = root.join("state");
        let start = |job: StartRequest| {
            let launch = validate(job, "native-mps", &[0], &state).expect("valid job");
            let id = launch.job_id.clone();
            let started = start_native(&launch, &state, &runner, &uv);
            (id, started)
        };

        let (id, started) = start(request("e2e-two"));
        assert!(started.is_ok(), "{started:?}");
        let read = wait_for_exit(&state, &id);
        assert_eq!(read["exit_code"], 0, "{read}");
        assert_eq!(
            read["events"],
            json!([
                {"ev": "step", "step": 1, "line": 1},
                {"ev": "done", "line": 2}
            ]),
            "{read}"
        );
        assert_eq!(
            delete_native(&state, &id).expect("deleted")["deleted"],
            true
        );

        let outside = root.join("outside.txt");
        let mut probe = request("e2e-probe");
        probe.args = vec!["--probe".into(), outside.display().to_string()];
        let (id, started) = start(probe);
        assert!(started.is_ok(), "{started:?}");
        let read = wait_for_exit(&state, &id);
        assert_eq!(
            read["events"],
            json!([{"ev": "probe", "ok": false, "line": 1}]),
            "the sandbox refuses a write outside the job: {read}"
        );
        assert!(!outside.exists());
        let _ = delete_native(&state, &id);

        let mut sleeper = request("e2e-stop");
        sleeper.args = vec!["--sleep".into()];
        let (id, started) = start(sleeper);
        let _stop_on_failure = StopOnDrop {
            state: &state,
            job_id: id.clone(),
        };
        assert!(started.is_ok(), "{started:?}");
        for _ in 0..300 {
            if !read_events(&state, &id, 0).expect("events")["events"]
                .as_array()
                .is_some_and(Vec::is_empty)
            {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        stop_native(&state, &id).expect("stopped");
        let read = wait_for_exit(&state, &id);
        assert_eq!(read["state"], "exited", "{read}");
        let runner_text = runner.display().to_string();
        let survivors: Vec<String> = command_stdout("ps", &["-A", "-o", "args="])
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains(&runner_text) && line.contains("--sleep"))
            .map(str::to_string)
            .collect();
        assert!(
            survivors.is_empty(),
            "the stopped trainer left processes behind: {survivors:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
