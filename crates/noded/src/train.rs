//! The trainer endpoints: `POST /v1/train` starts a job, `GET /v1/train/{job_id}/events` returns
//! what it printed, `POST /v1/train/{job_id}/stop` stops it and `GET /v1/train` lists the running
//! jobs. A job on Linux and Windows is a docker container labelled `agentplaza.job` (so the state
//! lives in docker, not in noded); on macOS it is the native runner, a `uv run` process with a job
//! directory `<state>/train/<job_id>/` (`pid`, `events.jsonl`, `stderr.log`, `exit`).
//!
//! A job outlives its exit: its container (or job directory) is kept, with the exit code, until
//! `DELETE /v1/train/{job_id}` removes it; the stop removes a container after stopping it. Finished
//! jobs older than 24 h are reaped on each `GET /v1/train`.
//!
//! Agents never get raw host access: the image is an `apz-trainer:` tag that fits the node's
//! runtime, the devices are accelerators the node has, the mounts sit under the state directory,
//! and the ids, env keys and network are checked against strict charsets. Events are the
//! container's stdout, one line per event: JSON objects as they are, any other line as
//! `{"ev":"log","text":…}`, each with its 1-based `line`. `after=N` returns the lines after N.
//!
//! The runtime is probed once at start (see `probe_trainer`); the docker engine is the native one
//! at `/var/run/docker.sock` on Linux unless `DOCKER_HOST` is set, because Docker Desktop's VM has
//! no GPU.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use apz_http::Response;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{Accelerator, Node, Output, Trainer, command_stdout, command_within};

const IMAGE_PREFIX: &str = "apz-trainer:";
const NATIVE_DOCKER_SOCKET: &str = "/var/run/docker.sock";
/// The directory with the `apz_train` package for the native runner (`uv run --project`).
const NATIVE_RUNNER_ENV: &str = "APZ_TRAINER_DIR";
const QUICK: Duration = Duration::from_secs(10);
/// `docker run` (with `--pull never`), `docker logs` and `docker stop`.
const RUN: Duration = Duration::from_secs(120);
const MAX_DEVICES: usize = 64;
const MAX_ENV: usize = 64;
const MAX_ARGS: usize = 256;
const MAX_MOUNTS: usize = 16;
const MAX_TEXT: usize = 4096;
const STDERR_TAIL: usize = 50;

/// The docker engine trainers run in, as `docker -H` takes it: DOCKER_HOST when set, else the
/// native engine's socket on Linux, else None (the default context).
pub fn docker_host() -> Option<String> {
    if let Some(host) = std::env::var("DOCKER_HOST")
        .ok()
        .map(|host| host.trim().to_string())
        .filter(|host| !host.is_empty())
    {
        return Some(host);
    }
    (cfg!(target_os = "linux") && Path::new(NATIVE_DOCKER_SOCKET).exists())
        .then(|| format!("unix://{NATIVE_DOCKER_SOCKET}"))
}

fn os_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

/// Runs `docker` (on `host` when set) with `args`; None when it cannot start or does not finish.
fn docker<S: AsRef<str>>(host: Option<&str>, args: &[S], timeout: Duration) -> Option<Output> {
    let mut all: Vec<&str> = Vec::with_capacity(args.len() + 2);
    if let Some(host) = host {
        all.push("-H");
        all.push(host);
    }
    all.extend(args.iter().map(|arg| arg.as_ref()));
    command_within("docker", &all, timeout)
}

/// The runtimes the engine registered (the keys of `docker info`'s `Runtimes`); None when the
/// engine does not answer.
fn docker_runtimes(host: Option<&str>) -> Option<Vec<String>> {
    let out =
        docker(host, &["info", "--format", "{{json .Runtimes}}"], QUICK).filter(|out| out.ok)?;
    Some(match serde_json::from_str::<Value>(out.stdout.trim()) {
        Ok(Value::Object(runtimes)) => runtimes.keys().cloned().collect(),
        _ => Vec::new(),
    })
}

/// Whether the NVIDIA CDI spec lists devices (`nvidia.com/gpu=0`).
fn nvidia_cdi_ok() -> bool {
    command_within("nvidia-ctk", &["cdi", "list"], QUICK).is_some_and(|out| {
        out.ok && (out.stdout.contains("nvidia.com/gpu=") || out.stderr.contains("nvidia.com/gpu="))
    })
}

/// What the trainer check reads from the host; `choose_trainer` judges it.
struct Inputs<'a> {
    os: &'static str,
    vendors: &'a [&'a str],
    runtimes: Option<&'a [String]>,
    cdi: bool,
    kfd: bool,
    dri: bool,
    uv: bool,
}

/// The trainer runtime this node can run jobs in, probed once at start. `vendors` are the
/// accelerators' vendors; `docker_host` is the engine to ask.
pub fn probe_trainer(vendors: &[&str], docker_host: Option<&str>) -> Trainer {
    let os = os_name();
    let macos = os == "macos";
    let runtimes = if macos {
        None
    } else {
        docker_runtimes(docker_host)
    };
    choose_trainer(&Inputs {
        os,
        vendors,
        runtimes: runtimes.as_deref(),
        cdi: os == "linux" && vendors.contains(&"nvidia") && nvidia_cdi_ok(),
        kfd: Path::new("/dev/kfd").exists(),
        dri: Path::new("/dev/dri").exists(),
        uv: macos && command_stdout("uv", &["--version"]).is_some(),
    })
}

/// The runtime for the node's accelerator (NVIDIA first, then AMD, Intel), `docker-cpu` when there
/// is none and Docker answers, and the native MPS runner on macOS.
fn choose_trainer(input: &Inputs) -> Trainer {
    let ready = |runtime: &'static str, detail: &str| Trainer {
        runtime: Some(runtime),
        ok: true,
        detail: detail.to_string(),
    };
    let not_ready = |runtime: Option<&'static str>, detail: &str| Trainer {
        runtime,
        ok: false,
        detail: detail.to_string(),
    };
    if input.os == "macos" {
        return if input.uv {
            ready("native-mps", "uv runs the native MPS runner")
        } else {
            not_ready(
                Some("native-mps"),
                "install uv (https://docs.astral.sh/uv/) to run the native MPS runner",
            )
        };
    }
    let Some(runtimes) = input.runtimes else {
        return not_ready(
            None,
            "Docker is not reachable: install Docker, or set DOCKER_HOST to the engine with the GPU",
        );
    };
    let has = |name: &str| runtimes.iter().any(|runtime| runtime == name);
    match ["nvidia", "amd", "intel"]
        .into_iter()
        .find(|vendor| input.vendors.contains(vendor))
    {
        Some("nvidia") if !has("nvidia") => not_ready(
            Some("docker-nvidia"),
            "Docker has no nvidia runtime: install the NVIDIA Container Toolkit and register it with nvidia-ctk runtime configure",
        ),
        Some("nvidia") if input.os == "linux" && !input.cdi => not_ready(
            Some("docker-nvidia"),
            "the nvidia runtime is registered but the CDI spec lists no devices: run nvidia-ctk cdi generate",
        ),
        Some("nvidia") if input.os == "linux" => ready(
            "docker-nvidia",
            "nvidia runtime registered; CDI devices listed",
        ),
        Some("nvidia") => ready("docker-nvidia", "nvidia runtime registered"),
        Some("amd") if !input.kfd => not_ready(
            Some("docker-rocm"),
            "/dev/kfd is missing: the ROCm kernel driver is not loaded",
        ),
        Some("amd") => ready("docker-rocm", "/dev/kfd present"),
        Some("intel") if !input.dri => not_ready(
            Some("docker-xpu"),
            "/dev/dri is missing: the Intel GPU driver is not loaded",
        ),
        Some("intel") => ready("docker-xpu", "/dev/dri present"),
        _ => ready(
            "docker-cpu",
            "Docker is reachable; the node has no accelerator to train on",
        ),
    }
}

/// The trainer runtime's accelerator vendor.
fn runtime_vendor(runtime: &str) -> Option<&'static str> {
    match runtime {
        "docker-nvidia" => Some("nvidia"),
        "docker-rocm" => Some("amd"),
        "docker-xpu" => Some("intel"),
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

/// A job running on this node: its container (None for the native runner), runtime and devices.
struct Running {
    job_id: String,
    container: Option<String>,
    runtime: String,
    devices: Vec<u32>,
}

fn running(docker_host: Option<&str>, state: &Path) -> Vec<Running> {
    let mut jobs = docker_running(docker_host);
    jobs.extend(native_running(state));
    jobs
}

fn docker_running(host: Option<&str>) -> Vec<Running> {
    let format = "{{.Label \"agentplaza.job\"}}\t{{.Names}}\t{{.Label \"agentplaza.runtime\"}}\t{{.Label \"agentplaza.devices\"}}";
    let Some(out) = docker(
        host,
        &[
            "ps",
            "--filter",
            "label=agentplaza.job",
            "--filter",
            "status=running",
            "--format",
            format,
        ],
        QUICK,
    )
    .filter(|out| out.ok) else {
        return Vec::new();
    };
    out.stdout
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            match fields.as_slice() {
                [job, container, runtime, devices] => Some(Running {
                    job_id: job.to_string(),
                    container: Some(container.to_string()),
                    runtime: runtime.to_string(),
                    devices: parse_devices(devices),
                }),
                _ => None,
            }
        })
        .collect()
}

/// The container of a job (running or not yet removed) and its state.
fn docker_job(host: Option<&str>, job_id: &str) -> Option<(String, String)> {
    let filter = format!("label=agentplaza.job={job_id}");
    let out = docker(
        host,
        &[
            "ps",
            "-a",
            "--filter",
            filter.as_str(),
            "--format",
            "{{.Names}}\t{{.State}}",
        ],
        QUICK,
    )
    .filter(|out| out.ok)?;
    let (name, state) = out.stdout.lines().next()?.split_once('\t')?;
    Some((name.to_string(), state.to_string()))
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

fn native_alive(pid: u32) -> bool {
    let pid = pid.to_string();
    command_stdout("ps", &["-p", pid.as_str(), "-o", "pid="]).is_some_and(|out| !out.is_empty())
}

fn native_running(state: &Path) -> Vec<Running> {
    let Ok(entries) = std::fs::read_dir(state.join("train")) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let pid = native_pid(&entry.path())?;
            native_alive(pid).then(|| Running {
                job_id: entry.file_name().to_string_lossy().into_owned(),
                container: None,
                runtime: "native-mps".to_string(),
                devices: vec![0],
            })
        })
        .collect()
}

fn parse_devices(csv: &str) -> Vec<u32> {
    csv.split(',')
        .filter_map(|device| device.trim().parse().ok())
        .collect()
}

fn csv(devices: &[u32]) -> String {
    devices
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// The accelerators a running job holds on this node, as (vendor, index): read from the containers'
/// labels and the native runner's pid. Used to mark `busy` in `GET /v1/machine`.
pub fn busy_devices(docker_host: Option<&str>) -> Vec<(&'static str, u32)> {
    running(docker_host, &apz_os::dirs::state())
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

/// The `POST /v1/train` body.
#[derive(Debug, Deserialize)]
pub struct StartRequest {
    pub job_id: String,
    /// `apz-trainer:cuda`, `:rocm`, `:xpu`, `:cpu` (any docker runtime), or `:mps` (native).
    pub image: String,
    /// Accelerator indices of the runtime's vendor; empty for none.
    #[serde(default)]
    pub devices: Vec<u32>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Appended to the image's entrypoint (or to the native runner's `python -m apz_train.run`).
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub mounts: Vec<Mount>,
    /// `host` or `none` (the default).
    #[serde(default)]
    pub network: String,
}

/// A bind mount: a path under the state directory, read-only unless `ro` is false.
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
    pub image: String,
    pub devices: Vec<u32>,
    pub env: BTreeMap<String, String>,
    pub args: Vec<String>,
    pub mounts: Vec<Mount>,
    pub network: &'static str,
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
    check_image(&request.image, runtime)?;
    let devices = check_devices(&request.devices, available)?;
    if request.image.ends_with(":cpu") && !devices.is_empty() {
        return Err("the cpu image takes no devices".into());
    }
    let network = match request.network.as_str() {
        "" | "none" => "none",
        "host" => "host",
        other => return Err(format!("network is host or none, not {other:?}")),
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
    }
    if runtime == "native-mps" && !request.mounts.is_empty() {
        return Err(
            "the native runner reads its inputs from its job directory: mounts are for containers"
                .into(),
        );
    }
    Ok(Launch {
        job_id: request.job_id,
        runtime,
        image: request.image,
        devices,
        env: request.env,
        args: request.args,
        mounts: request.mounts,
        network,
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

/// The image's tag must be one the runtime runs: `cuda` on `docker-nvidia`, `rocm`, `xpu`, `cpu` on
/// any docker runtime, `mps` on the native runner.
fn check_image(image: &str, runtime: &str) -> Result<(), String> {
    let tag = image
        .strip_prefix(IMAGE_PREFIX)
        .ok_or_else(|| format!("image must be {IMAGE_PREFIX}<tag>"))?;
    let fits = match tag {
        "cuda" => runtime == "docker-nvidia",
        "rocm" => runtime == "docker-rocm",
        "xpu" => runtime == "docker-xpu",
        "cpu" => runtime.starts_with("docker-"),
        "mps" => runtime == "native-mps",
        _ => return Err(format!("unknown trainer image tag {tag:?}")),
    };
    if fits {
        Ok(())
    } else {
        Err(format!(
            "image {image} does not run on the {runtime} runtime"
        ))
    }
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

/// A path without commas (docker's `--mount` separator), NUL, newline or `..` parts.
fn plain_path(text: &str) -> bool {
    !text.is_empty()
        && !text.chars().any(|c| matches!(c, ',' | '\0' | '\n'))
        && !text
            .split(|c| c == '/' || c == '\\')
            .any(|part| part == "..")
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

/// The env a runtime needs to see the chosen devices (the job's own env overrides it).
fn runtime_env(runtime: &str, devices: &[u32]) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    if devices.is_empty() {
        return env;
    }
    match runtime {
        // The NVIDIA runtime mounts the driver's utilities (nvidia-smi) only with `utility`.
        "docker-nvidia" => {
            env.insert(
                "NVIDIA_DRIVER_CAPABILITIES".to_string(),
                "compute,utility".to_string(),
            );
        }
        "docker-rocm" => {
            env.insert("HIP_VISIBLE_DEVICES".to_string(), csv(devices));
            env.insert("ROCR_VISIBLE_DEVICES".to_string(), csv(devices));
        }
        "docker-xpu" => {
            env.insert("ZE_AFFINITY_MASK".to_string(), csv(devices));
        }
        _ => {}
    }
    env
}

/// The `--user` of a container on Unix: this process's uid and gid, so the trainer writes its files
/// into the job directory as the node's user and not as root. The crate forbids unsafe code, so the
/// ids come from `id`; None when it cannot say, and then the image's own user stays.
#[cfg(unix)]
fn run_user() -> Option<String> {
    let id = |flag: &str| {
        let out = std::process::Command::new("id").arg(flag).output().ok()?;
        let text = String::from_utf8(out.stdout).ok()?;
        (out.status.success() && !text.trim().is_empty()).then(|| text.trim().to_string())
    };
    Some(format!("{}:{}", id("-u")?, id("-g")?))
}

/// The `docker run` arguments of a launch: detached, labelled, with the runtime's device flags, the
/// mounts, the network (none unless asked) and the image's pull disabled, so an unknown image fails
/// instead of being fetched. The container stays after it exits, until `DELETE`.
fn run_args(launch: &Launch, name: &str) -> Vec<String> {
    let devices = csv(&launch.devices);
    let mut args: Vec<String> = ["run", "-d", "--pull", "never", "--name", name]
        .map(String::from)
        .into();
    args.extend([
        "--label".to_string(),
        format!("agentplaza.job={}", launch.job_id),
        "--label".to_string(),
        format!("agentplaza.runtime={}", launch.runtime),
        "--label".to_string(),
        format!("agentplaza.devices={devices}"),
        "--network".to_string(),
        launch.network.to_string(),
    ]);
    match launch.runtime {
        "docker-nvidia" if !launch.devices.is_empty() => {
            args.push("--gpus".to_string());
            args.push(format!("device={devices}"));
        }
        "docker-rocm" => args.extend(
            [
                "--device",
                "/dev/kfd",
                "--device",
                "/dev/dri",
                "--group-add",
                "video",
            ]
            .map(String::from),
        ),
        "docker-xpu" => args.extend(["--device", "/dev/dri"].map(String::from)),
        _ => {}
    }
    #[cfg(unix)]
    args.extend(
        run_user()
            .into_iter()
            .flat_map(|user| ["--user".to_string(), user]),
    );
    // The trainer runs as the node's user, which has no home: /tmp is writable and holds its caches.
    let mut env = runtime_env(launch.runtime, &launch.devices);
    env.insert("HOME".to_string(), "/tmp".to_string());
    env.extend(launch.env.clone());
    for (key, value) in env {
        args.push("-e".to_string());
        args.push(format!("{key}={value}"));
    }
    for mount in &launch.mounts {
        let read_only = if mount.ro { ",readonly" } else { "" };
        args.push("--mount".to_string());
        args.push(format!(
            "type=bind,source={},target={}{read_only}",
            mount.host, mount.container
        ));
    }
    args.push(launch.image.clone());
    args.extend(launch.args.iter().cloned());
    args
}

fn host(node: &Node) -> Option<&str> {
    node.probed.facts.docker_host.as_deref()
}

/// `POST /v1/train` → 201 `{container, job_id, runtime, devices}` (the native runner: `{pid}`).
pub fn start(node: &Node, body: &[u8]) -> Response {
    let request: StartRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(err) => return Response::error(400, &format!("invalid train request: {err}")),
    };
    let trainer = &node.probed.facts.trainer;
    let Some(runtime) = trainer.runtime.filter(|_| trainer.ok) else {
        return Response::error(
            409,
            &format!("the trainer runtime is not ready: {}", trainer.detail),
        );
    };
    let state = apz_os::dirs::state();
    let launch = match validate(
        request,
        runtime,
        &available(&node.probed.facts.accelerators, runtime),
        &state,
    ) {
        Ok(launch) => launch,
        Err(reason) => return Response::error(400, &reason),
    };
    let docker_host = host(node);
    let taken = docker_job(docker_host, &launch.job_id).is_some()
        || native_dir(&state, &launch.job_id).exists();
    if taken {
        return Response::error(
            409,
            &format!(
                "job {} exists: DELETE /v1/train/{} once it is read, or pick another id",
                launch.job_id, launch.job_id
            ),
        );
    }
    if runtime == "native-mps" {
        native_start(&launch, &state)
    } else {
        docker_start(docker_host, &launch)
    }
}

fn docker_start(host: Option<&str>, launch: &Launch) -> Response {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    let name = format!("apz-train-{}-{millis}", launch.job_id);
    let args = run_args(launch, &name);
    match docker(host, args.as_slice(), RUN) {
        Some(out) if out.ok => Response::json(
            201,
            &json!({"container": name, "job_id": launch.job_id, "runtime": launch.runtime, "devices": launch.devices}),
        ),
        Some(out) => Response::error(502, &format!("docker run failed: {}", out.stderr.trim())),
        None => Response::error(502, "docker run did not finish"),
    }
}

/// The native runner: `uv run --project <runner> python -m apz_train.run <args>` in the job
/// directory, stdout to `events.jsonl`, stderr to `stderr.log`, its pid in `pid`.
fn native_start(launch: &Launch, state: &Path) -> Response {
    let dir = native_dir(state, &launch.job_id);
    let files = std::fs::create_dir_all(&dir).and_then(|()| {
        Ok((
            File::create(dir.join("events.jsonl"))?,
            File::create(dir.join("stderr.log"))?,
        ))
    });
    let (events, stderr) = match files {
        Ok(files) => files,
        Err(err) => {
            return Response::error(500, &format!("job directory {}: {err}", dir.display()));
        }
    };
    let runner = std::env::var(NATIVE_RUNNER_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| state.join("trainer"));
    let mut command = std::process::Command::new("uv");
    command
        .arg("run")
        .arg("--project")
        .arg(&runner)
        .args(["python", "-m", "apz_train.run"])
        .args(&launch.args)
        .current_dir(&dir)
        .env("APZ_JOB_DIR", &dir)
        .envs(&launch.env)
        .stdin(Stdio::null())
        .stdout(events)
        .stderr(stderr);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => return Response::error(502, &format!("uv run: {err}")),
    };
    let pid = child.id();
    if let Err(err) = std::fs::write(dir.join("pid"), pid.to_string()) {
        let _ = child.kill();
        return Response::error(500, &format!("pid file: {err}"));
    }
    // Reaped on a thread, so the daemon does not keep a zombie for every finished job; the exit code
    // goes to `exit` (`signal` when it was killed), which outlives the process until DELETE.
    let exit_file = dir.join("exit");
    thread::spawn(move || {
        if let Ok(status) = child.wait() {
            let code = status
                .code()
                .map_or_else(|| "signal".to_string(), |code| code.to_string());
            let _ = std::fs::write(exit_file, code);
        }
    });
    Response::json(
        201,
        &json!({"job_id": launch.job_id, "container": null, "pid": pid, "runtime": launch.runtime}),
    )
}

/// How long a finished job's container or directory is kept before `GET /v1/train` reaps it.
const REAP_AFTER_SECS: u64 = 24 * 3600;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// `GET /v1/train` → the running jobs. Reaps the finished jobs older than 24 h first.
pub fn list(node: &Node) -> Response {
    let state = apz_os::dirs::state();
    reap(host(node), &state);
    let jobs: Vec<Value> = running(host(node), &state)
        .into_iter()
        .map(|job| json!({"job_id": job.job_id, "container": job.container, "runtime": job.runtime, "devices": job.devices}))
        .collect();
    Response::json(200, &jobs)
}

/// Removes the apz-trainer containers that exited, and the native job directories of finished jobs,
/// more than `REAP_AFTER_SECS` ago. Runs on each listing, so no thread is needed.
fn reap(docker_host: Option<&str>, state: &Path) {
    let cutoff = now_secs().saturating_sub(REAP_AFTER_SECS);
    reap_docker(docker_host, cutoff);
    reap_native(state, cutoff);
}

fn reap_docker(host: Option<&str>, cutoff: u64) {
    let listing = [
        "ps",
        "-a",
        "--filter",
        "label=agentplaza.job",
        "--filter",
        "status=exited",
        "--format",
        "{{.Names}}\t{{.Image}}",
    ];
    let Some(out) = docker(host, &listing, QUICK).filter(|out| out.ok) else {
        return;
    };
    for line in out.stdout.lines() {
        let Some((name, image)) = line.split_once('\t') else {
            continue;
        };
        if !image.starts_with(IMAGE_PREFIX) {
            continue;
        }
        let finished = docker(
            host,
            &["inspect", "--format", "{{.State.FinishedAt}}", name],
            QUICK,
        )
        .filter(|out| out.ok)
        .and_then(|out| utc_epoch(out.stdout.trim()));
        if finished.is_some_and(|at| at < cutoff) {
            let _ = docker(host, &["rm", name], QUICK);
        }
    }
}

fn reap_native(state: &Path, cutoff: u64) {
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

/// The exit code of a container that is not running; None when docker does not say.
fn docker_exit_code(host: Option<&str>, container: &str) -> Option<i64> {
    let out = docker(
        host,
        &["inspect", "--format", "{{.State.ExitCode}}", container],
        QUICK,
    )
    .filter(|out| out.ok)?;
    out.stdout.trim().parse().ok()
}

/// The exit code the native runner's reaper wrote; None while it runs or when it was killed.
fn native_exit_code(dir: &Path) -> Option<i64> {
    std::fs::read_to_string(dir.join("exit"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// `GET /v1/train/{job_id}/events?after=N` → `{job_id, container, state, exit_code, next, events,
/// stderr}`: the stdout lines after line N as events, `next` the line count to ask from next time,
/// the last lines of stderr for diagnosis. `state` is `running` or `exited` (with `exit_code`); an
/// exited job's events stay readable until `DELETE`.
pub fn events(node: &Node, job_id: &str, after: u64) -> Response {
    if !valid_job_id(job_id) {
        return Response::error(400, "invalid job id");
    }
    let docker_host = host(node);
    let (container, state, exit_code, stdout, stderr) = match docker_job(docker_host, job_id) {
        Some((container, state)) => {
            let exit_code = if state == "running" {
                None
            } else {
                docker_exit_code(docker_host, &container)
            };
            match docker(docker_host, &["logs", container.as_str()], RUN) {
                Some(logs) if logs.ok => {
                    (Some(container), state, exit_code, logs.stdout, logs.stderr)
                }
                _ => return Response::error(502, "docker logs failed"),
            }
        }
        None => {
            let dir = native_dir(&apz_os::dirs::state(), job_id);
            if !dir.exists() {
                return Response::error(404, &format!("no job {job_id}"));
            }
            let alive = native_pid(&dir).is_some_and(native_alive);
            let exit_code = if alive { None } else { native_exit_code(&dir) };
            let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap_or_default();
            let state = if alive { "running" } else { "exited" };
            (
                None,
                state.to_string(),
                exit_code,
                read("events.jsonl"),
                read("stderr.log"),
            )
        }
    };
    let (events, next) = event_lines(&stdout, after);
    let lines: Vec<&str> = stderr.lines().collect();
    let tail = &lines[lines.len().saturating_sub(STDERR_TAIL)..];
    Response::json(
        200,
        &json!({"job_id": job_id, "container": container, "state": state, "exit_code": exit_code, "next": next, "events": events, "stderr": tail}),
    )
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

/// `POST /v1/train/{job_id}/stop`: stops a container (up to 20 s) and removes it, so its events are
/// read before the stop; a native runner gets a terminate and keeps its job directory until DELETE.
pub fn stop(node: &Node, job_id: &str) -> Response {
    if !valid_job_id(job_id) {
        return Response::error(400, "invalid job id");
    }
    let docker_host = host(node);
    if let Some((container, _)) = docker_job(docker_host, job_id) {
        return match docker(
            docker_host,
            &["stop", "--time", "20", container.as_str()],
            RUN,
        ) {
            Some(out) if out.ok => {
                let removed = docker(docker_host, &["rm", container.as_str()], QUICK)
                    .is_some_and(|out| out.ok);
                Response::json(
                    200,
                    &json!({"job_id": job_id, "container": container, "stopped": true, "removed": removed}),
                )
            }
            Some(out) => {
                Response::error(502, &format!("docker stop failed: {}", out.stderr.trim()))
            }
            None => Response::error(502, "docker stop did not finish"),
        };
    }
    let dir = native_dir(&apz_os::dirs::state(), job_id);
    let Some(pid) = native_pid(&dir).filter(|&pid| native_alive(pid)) else {
        return Response::error(404, &format!("no running job {job_id}"));
    };
    let pid_text = pid.to_string();
    match command_within("kill", &[pid_text.as_str()], QUICK) {
        Some(out) if out.ok => Response::json(
            200,
            &json!({"job_id": job_id, "container": null, "pid": pid, "stopped": true}),
        ),
        _ => Response::error(502, &format!("could not stop pid {pid}")),
    }
}

/// `DELETE /v1/train/{job_id}`: removes a finished job's container or job directory, with its events
/// and exit code. A running job is a 409: stop it first.
pub fn delete(node: &Node, job_id: &str) -> Response {
    if !valid_job_id(job_id) {
        return Response::error(400, "invalid job id");
    }
    let docker_host = host(node);
    if let Some((container, state)) = docker_job(docker_host, job_id) {
        if state == "running" {
            return Response::error(409, &format!("job {job_id} is running: stop it first"));
        }
        return match docker(docker_host, &["rm", container.as_str()], QUICK) {
            Some(out) if out.ok => Response::json(
                200,
                &json!({"job_id": job_id, "container": container, "deleted": true}),
            ),
            Some(out) => Response::error(502, &format!("docker rm failed: {}", out.stderr.trim())),
            None => Response::error(502, "docker rm did not finish"),
        };
    }
    let dir = native_dir(&apz_os::dirs::state(), job_id);
    if !dir.exists() {
        return Response::error(404, &format!("no job {job_id}"));
    }
    if native_pid(&dir).is_some_and(native_alive) {
        return Response::error(409, &format!("job {job_id} is running: stop it first"));
    }
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Response::json(
            200,
            &json!({"job_id": job_id, "container": null, "deleted": true}),
        ),
        Err(err) => Response::error(500, &format!("job directory {}: {err}", dir.display())),
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The Unix time of docker's UTC timestamp (`2026-10-13T01:02:03.123Z`); None when it is not one.
fn utc_epoch(stamp: &str) -> Option<u64> {
    let field = |range: std::ops::Range<usize>| stamp.get(range)?.parse::<i64>().ok();
    let days = days_from_civil(field(0..4)?, field(5..7)?, field(8..10)?);
    let seconds = days * 86_400 + field(11..13)? * 3600 + field(14..16)? * 60 + field(17..19)?;
    u64::try_from(seconds).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    const NVIDIA_HOST: &[&str] = &["nvidia"];

    fn trainer_inputs<'a>(
        os: &'static str,
        vendors: &'a [&'a str],
        runtimes: Option<&'a [String]>,
    ) -> Inputs<'a> {
        Inputs {
            os,
            vendors,
            runtimes,
            cdi: true,
            kfd: false,
            dri: false,
            uv: false,
        }
    }

    fn runtimes(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn request(job: &str, image: &str) -> StartRequest {
        StartRequest {
            job_id: job.into(),
            image: image.into(),
            devices: Vec::new(),
            env: BTreeMap::new(),
            args: Vec::new(),
            mounts: Vec::new(),
            network: String::new(),
        }
    }

    #[test]
    fn trainer_choice_follows_the_accelerators_and_the_runtimes() {
        // nvidia_is_docker_nvidia_and_ready_only_with_its_runtime_and_cdi: the nvidia runtime and CDI are both needed for a ready nvidia trainer.
        {
            let nvidia = runtimes(&["runc", "nvidia"]);
            let trainer = choose_trainer(&trainer_inputs("linux", NVIDIA_HOST, Some(&nvidia)));
            assert_eq!((trainer.runtime, trainer.ok), (Some("docker-nvidia"), true));

            let plain = runtimes(&["runc"]);
            let missing = choose_trainer(&trainer_inputs("linux", NVIDIA_HOST, Some(&plain)));
            assert_eq!(
                (missing.runtime, missing.ok),
                (Some("docker-nvidia"), false)
            );
            assert!(missing.detail.contains("nvidia runtime"));
            let no_cdi = Inputs {
                cdi: false,
                ..trainer_inputs("linux", NVIDIA_HOST, Some(&nvidia))
            };
            assert!(!choose_trainer(&no_cdi).ok);
        }
        // amd_needs_kfd_and_intel_needs_dri: amd needs kfd and intel needs dri before they are ready.
        {
            let docker = runtimes(&["runc"]);
            let amd = Inputs {
                kfd: true,
                ..trainer_inputs("linux", &["amd"], Some(&docker))
            };
            assert_eq!(choose_trainer(&amd).runtime, Some("docker-rocm"));
            assert!(choose_trainer(&amd).ok);
            assert!(!choose_trainer(&trainer_inputs("linux", &["amd"], Some(&docker))).ok);
            let intel = Inputs {
                dri: true,
                ..trainer_inputs("linux", &["intel"], Some(&docker))
            };
            assert_eq!(
                (choose_trainer(&intel).runtime, choose_trainer(&intel).ok),
                (Some("docker-xpu"), true)
            );
        }
        // docker_without_accelerators_is_docker_cpu_and_no_docker_has_no_runtime: docker alone is the cpu trainer, and no docker means no runtime.
        {
            let docker = runtimes(&["runc"]);
            let cpu = choose_trainer(&trainer_inputs("linux", &[], Some(&docker)));
            assert_eq!((cpu.runtime, cpu.ok), (Some("docker-cpu"), true));
            let none = choose_trainer(&trainer_inputs("linux", &[], None));
            assert_eq!((none.runtime, none.ok), (None, false));
        }
        // macos_runs_natively_when_uv_is_installed: macos runs natively, ready only when uv is installed.
        {
            let with_uv = Inputs {
                uv: true,
                ..trainer_inputs("macos", &["apple"], None)
            };
            assert_eq!(
                (
                    choose_trainer(&with_uv).runtime,
                    choose_trainer(&with_uv).ok
                ),
                (Some("native-mps"), true)
            );
            let without = choose_trainer(&trainer_inputs("macos", &["apple"], None));
            assert_eq!((without.runtime, without.ok), (Some("native-mps"), false));
            assert!(without.detail.contains("uv"));
        }
    }

    #[test]
    fn the_run_flags_follow_the_nvidia_runtime() {
        let root = std::env::temp_dir().join(format!("apz-train-flags-{}", std::process::id()));
        let state = root.join("state");
        std::fs::create_dir_all(state.join("data")).expect("state");
        let mut job = request("job-1", "apz-trainer:cuda");
        job.devices = vec![0, 1];
        job.env.insert("RECIPE".into(), "tiny".into());
        job.mounts.push(Mount {
            host: state.join("data").display().to_string(),
            container: "/data".into(),
            ro: true,
        });
        job.args = vec!["--job".into(), "job.json".into()];
        let launch = validate(job, "docker-nvidia", &[0, 1], &state).expect("valid");
        let args = run_args(&launch, "apz-train-job-1-1");
        assert!(
            args.windows(2).any(|pair| pair == ["--gpus", "device=0,1"]),
            "{args:?}"
        );
        assert!(args.windows(2).any(|pair| pair == ["--network", "none"]));
        assert!(args.windows(2).any(|pair| pair == ["--pull", "never"]));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--label", "agentplaza.job=job-1"])
        );
        assert!(args.windows(2).any(|pair| pair == ["-e", "RECIPE=tiny"]));
        let mount = format!(
            "type=bind,source={},target=/data,readonly",
            state.join("data").display()
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "--mount" && pair[1] == mount),
            "{args:?}"
        );
        assert!(!args.contains(&"/dev/kfd".to_string()));
        let tail: Vec<&str> = args
            .iter()
            .rev()
            .take(3)
            .rev()
            .map(String::as_str)
            .collect();
        assert_eq!(
            tail,
            ["apz-trainer:cuda", "--job", "job.json"],
            "the image, then the args"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn the_container_runs_as_the_node_user_with_a_writable_home() {
        let root = std::env::temp_dir().join(format!("apz-train-user-{}", std::process::id()));
        let state = root.join("state");
        std::fs::create_dir_all(&state).expect("state");
        let mut job = request("job-3", "apz-trainer:cuda");
        job.devices = vec![0];
        let launch = validate(job, "docker-nvidia", &[0], &state).expect("valid");
        let args = run_args(&launch, "apz-train-job-3-1");
        let ids = |flag: &str| {
            String::from_utf8(
                std::process::Command::new("id")
                    .arg(flag)
                    .output()
                    .expect("id")
                    .stdout,
            )
            .expect("utf8")
            .trim()
            .to_string()
        };
        let expected = format!("{}:{}", ids("-u"), ids("-g"));
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "--user" && pair[1] == expected),
            "{args:?}"
        );
        assert!(
            args.windows(2).any(|pair| pair == ["-e", "HOME=/tmp"]),
            "{args:?}"
        );
        let user = args.iter().position(|arg| arg == "--user").expect("--user");
        let image = args
            .iter()
            .position(|arg| arg == "apz-trainer:cuda")
            .expect("image");
        assert!(user < image, "--user comes before the image");
        assert_eq!(
            args.last().map(String::as_str),
            Some("apz-trainer:cuda"),
            "the image is last"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rocm_and_xpu_get_their_device_nodes_and_visibility_env() {
        let mut rocm = request("r", "apz-trainer:rocm");
        rocm.devices = vec![1];
        let launch = validate(rocm, "docker-rocm", &[0, 1], &std::env::temp_dir()).expect("valid");
        let args = run_args(&launch, "n");
        assert!(args.windows(2).any(|pair| pair == ["--device", "/dev/kfd"]));
        assert!(args.windows(2).any(|pair| pair == ["--group-add", "video"]));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-e", "HIP_VISIBLE_DEVICES=1"])
        );
        let mut xpu = request("x", "apz-trainer:xpu");
        xpu.devices = vec![0];
        let args = run_args(
            &validate(xpu, "docker-xpu", &[0], &std::env::temp_dir()).expect("valid"),
            "n",
        );
        assert!(args.windows(2).any(|pair| pair == ["--device", "/dev/dri"]));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-e", "ZE_AFFINITY_MASK=0"])
        );
    }

    #[test]
    fn a_job_is_validated_before_it_runs() {
        let state = std::env::temp_dir();
        let ok = |job: StartRequest, runtime: &'static str, devices: &[u32]| {
            validate(job, runtime, devices, &state)
        };
        assert!(ok(request("bad id!", "apz-trainer:cpu"), "docker-cpu", &[]).is_err());
        assert!(ok(request("-leading", "apz-trainer:cpu"), "docker-cpu", &[]).is_err());
        assert!(
            ok(request("j", "alpine:latest"), "docker-cpu", &[]).is_err(),
            "image prefix"
        );
        assert!(
            ok(request("j", "apz-trainer:cuda"), "docker-cpu", &[]).is_err(),
            "tag fits the runtime"
        );
        assert!(
            ok(request("j", "apz-trainer:latest"), "docker-cpu", &[]).is_err(),
            "unknown tag"
        );
        let mut unknown = request("j", "apz-trainer:cuda");
        unknown.devices = vec![3];
        assert!(ok(unknown, "docker-nvidia", &[0, 1]).is_err());
        let mut twice = request("j", "apz-trainer:cuda");
        twice.devices = vec![0, 0];
        assert!(ok(twice, "docker-nvidia", &[0]).is_err());
        let mut bad_key = request("j", "apz-trainer:cpu");
        bad_key.env.insert("9X".into(), "v".into());
        assert!(ok(bad_key, "docker-cpu", &[]).is_err());
        let mut newline = request("j", "apz-trainer:cpu");
        newline.env.insert("OK_KEY".into(), "a\nb".into());
        assert!(ok(newline, "docker-cpu", &[]).is_err());
        let mut network = request("j", "apz-trainer:cpu");
        network.network = "bridge".into();
        assert!(ok(network, "docker-cpu", &[]).is_err());
        let mut mounted = request("j", "apz-trainer:mps");
        mounted.mounts.push(Mount {
            host: state.display().to_string(),
            container: "/data".into(),
            ro: true,
        });
        assert!(
            validate(mounted, "native-mps", &[0], &state).is_err(),
            "native runs take no mounts"
        );
        let mut host_net = request("h", "apz-trainer:cpu");
        host_net.network = "host".into();
        assert_eq!(
            ok(host_net, "docker-cpu", &[]).expect("valid").network,
            "host"
        );
        assert_eq!(
            ok(request("h", "apz-trainer:cpu"), "docker-cpu", &[])
                .expect("valid")
                .network,
            "none"
        );
    }

    #[test]
    fn mounts_must_sit_under_the_state_directory() {
        let root = std::env::temp_dir().join(format!("apz-train-test-{}", std::process::id()));
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
    fn docker_timestamps_are_utc_epochs() {
        assert_eq!(utc_epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(utc_epoch("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(
            utc_epoch("2026-10-13T01:02:03.123456789Z"),
            Some(1_791_853_323)
        );
        assert_eq!(
            utc_epoch("0001-01-01T00:00:00Z"),
            None,
            "a container never started"
        );
        assert_eq!(utc_epoch("not a time"), None);
    }

    #[test]
    fn job_ids_are_short_and_charset_limited() {
        assert!(valid_job_id("job_1-a"));
        assert!(!valid_job_id(""));
        assert!(!valid_job_id("a/b"));
        assert!(!valid_job_id(&"x".repeat(64)));
    }

    #[test]
    fn a_native_job_with_no_device_still_runs_on_the_mps_runner() {
        let mut job = request("mac-1", "apz-trainer:mps");
        job.args = vec!["--job".into(), "job.json".into()];
        let launch = validate(job, "native-mps", &[0], &std::env::temp_dir()).expect("valid");
        assert_eq!(
            (launch.runtime, launch.devices, launch.network),
            ("native-mps", Vec::<u32>::new(), "none")
        );
    }

    /// The live check on the native engine: the same `docker run` arguments noded builds (without `-d`,
    /// so the output comes back) with the NVIDIA runtime's device flags on the CUDA base image. Needs
    /// the image `nvidia/cuda:12.8.1-base-ubuntu24.04` locally: `cargo test -p noded -- --ignored --nocapture docker_gpu_smoke`.
    #[test]
    #[ignore]
    fn docker_gpu_smoke() {
        let host = docker_host();
        let mut job = request("smoke", "apz-trainer:cuda");
        job.devices = vec![0];
        job.args = vec!["nvidia-smi".into(), "-L".into()];
        let launch = Launch {
            image: "nvidia/cuda:12.8.1-base-ubuntu24.04".into(),
            ..validate(job, "docker-nvidia", &[0], &std::env::temp_dir()).expect("valid")
        };
        let name = format!("apz-train-smoke-{}", std::process::id());
        let mut args = run_args(&launch, &name);
        args.retain(|arg| arg != "-d");
        let out = docker(host.as_deref(), args.as_slice(), RUN).expect("docker runs");
        // Without --rm the container stays after it exits: remove it.
        let _ = docker(host.as_deref(), &["rm", "-f", name.as_str()], QUICK);
        println!("{}\n{}", out.stdout, out.stderr);
        assert!(out.ok && out.stdout.contains("GPU 0"));
    }

    /// One request to a daemon started in the test; returns the status and the JSON body.
    fn http(port: u16, method: &str, path: &str, body: &str) -> (u16, Value) {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).expect("write");
        let mut reply = String::new();
        stream.read_to_string(&mut reply).expect("read");
        let status = reply
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        (
            status,
            serde_json::from_str(reply.split("\r\n\r\n").nth(1).unwrap_or(""))
                .unwrap_or(Value::Null),
        )
    }

    /// The lifecycle of a finished job on the native engine: its events and exit code stay readable
    /// after it exits, and `DELETE` removes them. Labels are set by hand (a job image is not needed);
    /// `cargo test -p noded -- --ignored --nocapture a_finished_job_is_read_then_deleted` (needs alpine).
    #[test]
    #[ignore]
    fn a_finished_job_is_read_then_deleted() {
        let server = crate::Server::start(crate::Config {
            port: 0,
            ..crate::Config::default()
        })
        .expect("serve");
        let port = server.port();
        let host = docker_host();
        let name = "apz-train-life-1-test";
        let _ = docker(host.as_deref(), &["rm", "-f", name], QUICK);
        let script = r#"echo '{"ev":"step","step":1}'; echo hello; echo oops >&2; exit 3"#;
        let started = docker(
            host.as_deref(),
            &[
                "run",
                "-d",
                "--pull",
                "never",
                "--name",
                name,
                "--label",
                "agentplaza.job=life-1",
                "--label",
                "agentplaza.runtime=docker-cpu",
                "--label",
                "agentplaza.devices=",
                "alpine:latest",
                "sh",
                "-c",
                script,
            ],
            RUN,
        )
        .expect("docker run");
        assert!(started.ok, "{}", started.stderr);
        let mut read = Value::Null;
        for _ in 0..100 {
            let (status, body) = http(port, "GET", "/v1/train/life-1/events?after=0", "");
            assert_eq!(status, 200, "{body}");
            if body["state"] == "exited" {
                read = body;
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        println!("{read}");
        assert_eq!(read["exit_code"], 3);
        assert_eq!(
            read["events"][0],
            json!({"ev": "step", "step": 1, "line": 1})
        );
        assert_eq!(
            read["events"][1],
            json!({"ev": "log", "text": "hello", "line": 2})
        );
        assert_eq!(read["next"], 2);
        assert_eq!(read["stderr"][0], "oops");
        let (status, _) = http(port, "DELETE", "/v1/train/life-1", "");
        assert_eq!(status, 200);
        let (status, _) = http(port, "GET", "/v1/train/life-1/events?after=0", "");
        assert_eq!(status, 404);
        let gone = docker(
            host.as_deref(),
            &[
                "ps",
                "-a",
                "--filter",
                "name=apz-train-life-1-test",
                "--format",
                "{{.Names}}",
            ],
            QUICK,
        );
        assert_eq!(
            gone.map(|out| out.stdout.trim().to_string())
                .unwrap_or_default(),
            ""
        );
    }

    /// The live check of the routes: starts a daemon on an ephemeral port and asks it over HTTP. Only
    /// read paths and a start that fails validation, so nothing runs:
    /// `cargo test -p noded -- --ignored --nocapture routes_answer`.
    #[test]
    #[ignore]
    fn routes_answer_the_foundry_endpoints() {
        use std::io::{Read, Write};
        let server = crate::Server::start(crate::Config {
            port: 0,
            ..crate::Config::default()
        })
        .expect("serve");
        let port = server.port();
        let call = |method: &str, path: &str, body: &str| -> (String, String) {
            let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
            let request = format!(
                "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(request.as_bytes()).expect("write");
            let mut reply = String::new();
            stream.read_to_string(&mut reply).expect("read");
            let status = reply.lines().next().unwrap_or("").to_string();
            (
                status,
                reply.split("\r\n\r\n").nth(1).unwrap_or("").to_string(),
            )
        };
        let calls = [
            ("GET", "/v1/train", ""),
            ("GET", "/v1/train/nope/events?after=0", ""),
            ("POST", "/v1/train/nope/stop", ""),
            (
                "POST",
                "/v1/train",
                "{\"job_id\":\"bad id\",\"image\":\"apz-trainer:cpu\"}",
            ),
            ("DELETE", "/v1/train/nope", ""),
            ("GET", "/v1/machine", ""),
        ];
        for (method, path, body) in calls {
            let (status, text) = call(method, path, body);
            let shown: String = text.chars().take(900).collect();
            println!("{method} {path} -> {status}\n{shown}\n");
        }
    }
}
