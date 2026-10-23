//! `code_engine` over ACP: one prompt turn of an engine session.
//!
//! The call is validated first, so a bad call installs nothing. The engine's pinned adapter is
//! installed when it is not installed at the catalogue's version (see [`install`]), from the exact
//! tarball whose sha512 the catalogue names.
//! One prompt turn then runs through [`session::prompt_turn`]. The session it ends in is kept in
//! `<workspace>/.plaza/engine-sessions.json`, so a later call can continue it, and the turn's
//! summary is what the model reads back.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use apz_proto::wire::EngineSpec;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256, Sha512};

use super::catalog;
use super::instance::{Instance, Pin};
use super::layout;
use super::link::EngineLink;
use super::session::{self, Options, Outcome};
use crate::bash::{self, Privilege};
use crate::checkpoints;
use crate::engines;
use crate::files::{self, Workspace};

/// The most one summary returns to the model, in bytes.
const SUMMARY_BYTES: usize = 16 * 1024;
/// The most of the turn's message a summary keeps (its end), in bytes.
const MESSAGE_BYTES: usize = 12 * 1024;
/// The most of the other summary sections (files, commands, plan, tool calls), in bytes.
const DETAIL_BYTES: usize = 8 * 1024;
/// The most of the turn's message a session keeps for a later call, in bytes.
const STORED_SUMMARY_BYTES: usize = 2 * 1024;
/// The sessions kept per workspace; the least recently used are dropped first.
const MAX_SESSIONS: usize = 50;
/// The most changed-file summary lines a summary lists.
const FILE_LINES: usize = 40;
/// The most a setup command (Cline's `auth`) may take, in seconds.
const SETUP_TIMEOUT_S: u64 = 60;
const SESSIONS_FILE: &str = "engine-sessions.json";
/// Where a release engine is extracted, one folder per engine and version.
const ENGINES_DIR: &str = "/workspace/.engines";

/// Runs one `code_engine` call as an ACP prompt turn and returns its summary for the model. `spec` is the
/// owner's launch spec for an engine instance (`None`: the catalogue engine the call names).
pub fn run(
    workspace: &Workspace,
    input: &Value,
    link: &dyn EngineLink,
    protocol: u16,
    spec: Option<&EngineSpec>,
    compaction: engines::Compaction,
) -> Result<String, String> {
    let id = input
        .get("engine")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut instance = Instance::resolve(workspace, id, spec)?;
    let request = Request::parse(workspace, input, &instance)?;
    // Before the install, the launch and the setup: the engine's home, its install cache and its job
    // folder must be its own by then (see `layout`).
    layout::prepare_engine_dirs(workspace, &instance.home, instance.version(), &request.cwd)?;
    install(workspace, &mut instance)?;
    let (env, model) = launch(workspace, &instance, request.model.as_deref(), compaction)?;
    let extra = engines::acp_args(&instance.engine, request.model.as_deref())?;
    if instance.engine == "vibe" {
        engines::write_vibe_config(workspace, &instance.home, request.model.as_deref())?;
    }
    // Before the turn and on every call: the setup rewrites the engine's saved settings, so a new
    // model takes effect.
    if let Some(args) = engines::setup_args(&instance.engine, request.model.as_deref())? {
        setup(workspace, &instance, env.clone(), &args)?;
    }
    let options = Options {
        entry: &instance,
        workspace,
        cwd: request.cwd.clone(),
        argv: argv(&instance, env, &extra),
        model,
        protocol,
        mode: request.mode.clone(),
        resume: request.resume.as_ref().map(|session| session.id.clone()),
        context: request
            .resume
            .as_ref()
            .map(|session| session.summary.clone()),
        deadline: Instant::now() + Duration::from_secs(request.timeout_s),
    };
    let turn = session::prompt_turn(&options, &request.prompt, link);
    // The turn's engine is dropped by now on every path (its group is killed then). A process that
    // left the group is killed here, before the workspace is read or written again.
    let stopped = bash::kill_engine_processes();
    let outcome = turn?;
    stopped?;

    let message = keep_end(outcome.turn.message.trim(), STORED_SUMMARY_BYTES);
    let recorded = record_turn(
        &workspace.plaza_dir(),
        &instance.id,
        &outcome.session,
        &message,
        now_secs(),
    );
    let files = outcome.checkpoint.as_ref().map(|since| {
        checkpoints::diff(workspace, &json!({ "since": since }))
            .unwrap_or_else(|err| format!("cannot list the changed files: {err}"))
    });
    let mut text = summary(
        &outcome,
        &instance.id,
        request.resume.is_some(),
        files.as_deref(),
    );
    if let Err(err) = recorded {
        text.push_str(&format!(
            "\n\nwarning: this session cannot be continued: {err}"
        ));
    }
    Ok(text)
}

/// A validated call: the prompt and options, and the session it continues.
struct Request {
    prompt: String,
    timeout_s: u64,
    model: Option<String>,
    mode: Option<String>,
    cwd: PathBuf,
    resume: Option<Session>,
}

impl Request {
    fn parse(workspace: &Workspace, input: &Value, instance: &Instance) -> Result<Self, String> {
        let prompt = match input.get("prompt").and_then(Value::as_str) {
            Some(prompt) if !prompt.trim().is_empty() => prompt.to_string(),
            _ => return Err("invalid input: \"prompt\" must be a non-empty string".to_string()),
        };
        let resume = match engines::optional_text(input, "session")? {
            None => None,
            Some(id) => Some(
                load_sessions(&workspace.plaza_dir())?
                    .into_iter()
                    .find(|session| session.engine == instance.id && session.id == id)
                    .ok_or_else(|| format!("unknown engine session {id:?} for {}", instance.id))?,
            ),
        };
        Ok(Self {
            prompt,
            timeout_s: engines::timeout_arg(input)?,
            // The call's model, else the instance's default route.
            model: engines::optional_text(input, "model")?
                .map(str::to_string)
                .or_else(|| instance.route.clone()),
            mode: engines::optional_text(input, "mode")?.map(str::to_string),
            cwd: engines::job_folder(workspace, input.get("path"))?,
            resume,
        })
    }
}

/// The launch of an instance: its `KEY=VALUE` pairs (the harness's own, then the owner's that do not
/// clash with them) and the model the session is given. A catalogue engine takes its own launch; any other
/// ACP engine runs as a plain agent.
fn launch(
    workspace: &Workspace,
    instance: &Instance,
    model: Option<&str>,
    compaction: engines::Compaction,
) -> Result<(Vec<String>, Option<String>), String> {
    let (mut env, model_id) = if catalog::entry(&instance.engine).is_some() {
        engines::acp_launch_at(
            workspace,
            &instance.engine,
            &instance.home,
            model,
            compaction,
        )?
    } else {
        engines::plain_launch(workspace, &instance.home, model)?
    };
    let extra = instance.extra_env(&env);
    env.extend(extra);
    Ok((env, model_id))
}

/// One engine session the workspace can continue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Session {
    engine: String,
    id: String,
    /// Unix seconds when the session was first recorded.
    created: u64,
    /// Unix seconds of its last turn.
    last_used: u64,
    /// The end of the last turn's message, at most [`STORED_SUMMARY_BYTES`].
    summary: String,
}

/// The ACP protocol version the harness offers an engine of a profile: 2 when the owner turned v2 on
/// (`engine_transport` `acp2`), else 1. The agent loop takes `acp2` as ACP, like the empty transport.
pub fn protocol_offer(engine_transport: &str) -> u16 {
    if engine_transport == "acp2" { 2 } else { 1 }
}

/// The program the ACP session starts: `/usr/bin/env` with the engine's pairs, the installed
/// binary, its arguments and `extra` (the call's model, for an engine that takes it on the command
/// line: see [`engines::acp_args`]). Otherwise the model is not among them: an engine takes it as a
/// session config option.
fn argv(instance: &Instance, env: Vec<String>, extra: &[String]) -> Vec<String> {
    command(instance, env, &instance.launch_args(extra))
}

/// `/usr/bin/env` with the engine's pairs, the installed binary and `args`: the one command line
/// both the session and the setup run.
fn command(instance: &Instance, mut env: Vec<String>, args: &[String]) -> Vec<String> {
    let mut argv = vec![engines::ENV_PROGRAM.to_string()];
    argv.append(&mut env);
    argv.push(instance.program());
    argv.extend(args.iter().cloned());
    argv
}

/// Runs an engine's setup command (see [`engines::setup_args`]) as the agent, in the workspace and
/// with the engine's environment. A failure says which engine and carries the command's tail.
fn setup(
    workspace: &Workspace,
    instance: &Instance,
    env: Vec<String>,
    args: &[String],
) -> Result<(), String> {
    let mut argv = command(instance, env, args);
    let program = argv.remove(0);
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    bash::execute(
        workspace.dir(),
        &program,
        &refs,
        SETUP_TIMEOUT_S,
        Privilege::Engine(layout::engine_home(workspace, &instance.home)),
    )
    .map(|_| ())
    .map_err(|err| format!("cannot set up {}: {}", instance.id, engines::tail(&err)))
}

/// Installs the engine when it is not installed at the catalogue's version, by its install method.
/// An install counts only once its binary is in place and its marker is recorded, so a failed one is
/// tried again, and a version bump in the catalogue installs the new version. Either way the bytes
/// installed are the ones whose checksum the catalogue names: a mismatch deletes the download and
/// installs nothing. The install scripts run as the engine user, which owns the folder being installed
/// into only while the install runs (see [`layout::install_in`]); afterwards root owns it, read-only to
/// the engine, so the engine cannot change the installed files and the launch does not check them again.
fn install(workspace: &Workspace, instance: &mut Instance) -> Result<(), String> {
    if installed(instance) {
        return Ok(());
    }
    let version = instance.version().to_string();
    let binary = match instance.pin.clone() {
        // A command is run where it is: nothing is installed.
        Pin::Command { .. } => return Ok(()),
        Pin::Npm {
            package,
            version,
            integrity,
        } => install_npm(workspace, instance, &package, &version, &integrity)?,
        Pin::Archive {
            version,
            url,
            sha256,
            cmd,
        } => install_archive(workspace, instance, &version, &url, &sha256, &cmd)?,
        Pin::Pip {
            package,
            version,
            file,
            sha256,
        } => install_pip(workspace, instance, &package, &version, &file, &sha256)?,
    };
    instance.binary = binary;
    if !Path::new(&instance.program()).is_file() {
        return Err(format!(
            "{} {version} is installed but {} is missing",
            instance.id,
            instance.program()
        ));
    }
    record_install(
        &instance.bin_root,
        &instance.home,
        &version,
        &instance.binary,
    )
    .map_err(|err| format!("cannot record the install of {}: {err}", instance.id))
}

/// The folder under an install's root where its marker goes.
const INSTALLED_DIR: &str = ".apz-installed";

/// Whether the engine is installed at its version: its command is in place and the marker of that
/// version is recorded. An added instance's marker names its command, which its package names.
fn installed(instance: &mut Instance) -> bool {
    if matches!(instance.pin, Pin::Command { .. }) {
        return true;
    }
    if !instance.catalogue_pin {
        match recorded_binary(&instance.bin_root, &instance.home, instance.version()) {
            Some(binary) => instance.binary = binary,
            None => return false,
        }
    }
    Path::new(&instance.program()).is_file()
        && is_recorded(&instance.bin_root, &instance.home, instance.version())
}

/// Records that `id` at `version` is installed under `prefix`, with the name of its command. Written
/// only after the install and its checks succeeded; the marker is named by version, so a bump names a
/// new one.
fn record_install(prefix: &Path, id: &str, version: &str, binary: &str) -> std::io::Result<()> {
    let dir = prefix.join(INSTALLED_DIR);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(format!("{id}-{version}")), binary)
}

fn is_recorded(prefix: &Path, id: &str, version: &str) -> bool {
    prefix
        .join(INSTALLED_DIR)
        .join(format!("{id}-{version}"))
        .exists()
}

/// The command name a marker recorded, when it names one.
fn recorded_binary(prefix: &Path, id: &str, version: &str) -> Option<String> {
    let text =
        std::fs::read_to_string(prefix.join(INSTALLED_DIR).join(format!("{id}-{version}"))).ok()?;
    let name = text.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Copies a binary to `to` through `<to>.tmp` and a rename: a reader of `to` sees the old file or the
/// new one, never a half-written copy, and a failed copy leaves no file behind.
fn install_binary(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut tmp = to.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let result = std::fs::copy(from, &tmp).and_then(|_| std::fs::rename(&tmp, to));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Installs an npm package's pinned version. `npm pack` fetches the exact version into the
/// workspace's engine cache; the tarball must match the catalogue's integrity (else it is deleted and
/// nothing is installed), and only then is it installed globally.
fn install_npm(
    workspace: &Workspace,
    instance: &Instance,
    package: &str,
    version: &str,
    integrity: &str,
) -> Result<String, String> {
    let home = layout::engine_home(workspace, &instance.home);
    engines::ensure_node(workspace)
        .map_err(|err| format!("cannot install nodejs and npm for {}: {err}", instance.id))?;
    if let Some((name, minimum)) = node_minimum(&instance.engine) {
        check_node(workspace, &home, name, minimum)?;
    }
    let spec = format!("{package}@{version}");
    let cache = layout::install_cache(&home);
    files::create_dirs(&cache)
        .map_err(|err| format!("unavailable: cannot create {}: {err}", cache.display()))?;
    let packed = bash::execute(
        workspace.dir(),
        "npm",
        &[
            "pack",
            spec.as_str(),
            "--pack-destination",
            text_path(&cache)?,
        ],
        engines::INSTALL_TIMEOUT_S,
        Privilege::Engine(home.clone()),
    )
    .map_err(|err| format!("cannot download {spec}: {}", engines::tail(&err)))?;
    let name = packed_name(&packed)
        .ok_or_else(|| format!("cannot download {spec}: npm packed no tarball"))?;
    let tarball = cache.join(name);
    let bytes = std::fs::read(&tarball).map_err(|err| format!("cannot read {spec}: {err}"))?;
    if !integrity_matches(&bytes, integrity) {
        let _ = std::fs::remove_file(&tarball);
        return Err(format!("integrity mismatch for {spec}"));
    }
    let root = instance.bin_root.as_path();
    if !instance.catalogue_pin {
        files::create_dirs(root)
            .map_err(|err| format!("unavailable: cannot create {}: {err}", root.display()))?;
    }
    layout::install_in(root, || {
        bash::execute(
            workspace.dir(),
            "npm",
            &[
                "install",
                "-g",
                "--prefix",
                text_path(root)?,
                text_path(&tarball)?,
            ],
            engines::INSTALL_TIMEOUT_S,
            Privilege::Engine(home.clone()),
        )
        .map_err(|err| format!("cannot install {spec}: {}", engines::tail(&err)))
    })?;
    if instance.catalogue_pin {
        // The catalogue names its command.
        Ok(instance.binary.clone())
    } else {
        npm_binary_name(root, package)
    }
}

/// The command an npm package installed under `prefix` provides, from its `bin` field.
fn npm_binary_name(prefix: &Path, package: &str) -> Result<String, String> {
    let manifest = prefix
        .join("lib")
        .join("node_modules")
        .join(package)
        .join("package.json");
    let text = std::fs::read_to_string(&manifest)
        .map_err(|err| format!("cannot read {}: {err}", manifest.display()))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|err| format!("cannot read {}: {err}", manifest.display()))?;
    let names = match value.get("bin") {
        Some(Value::String(_)) => vec![package.rsplit('/').next().unwrap_or(package).to_string()],
        Some(Value::Object(map)) => map.keys().cloned().collect(),
        _ => Vec::new(),
    };
    pick_binary(names, package)
}

/// The one command an install provides: the only one named, else the one named with `acp`.
fn pick_binary(names: Vec<String>, what: &str) -> Result<String, String> {
    match pick_one(&names) {
        // A command is a file name in the install's `bin`, never a path.
        Some(name) if name.contains('/') || name == "." || name == ".." => {
            Err(format!("invalid input: {what} names its command {name:?}"))
        }
        Some(name) => Ok(name),
        None => Err(format!(
            "cannot tell which command {what} runs; its commands are {names:?}"
        )),
    }
}

/// The one name in `names`, or the only one that names `acp`.
fn pick_one(names: &[String]) -> Option<String> {
    if let [only] = names {
        return Some(only.clone());
    }
    let mut acp = names.iter().filter(|name| name.contains("acp"));
    match (acp.next(), acp.next()) {
        (Some(name), None) => Some(name.clone()),
        _ => None,
    }
}

/// The Node.js version an engine's package needs, with the engine's name for the message. Kimi Code
/// runs on Node 22.19 or later, which the VM's apk `nodejs` may not be.
fn node_minimum(engine: &str) -> Option<(&'static str, (u64, u64))> {
    (engine == "kimi").then_some(("Kimi Code", (22, 19)))
}

/// Checks the VM's `node --version` against `minimum` before the install, so a VM with an older node
/// fails with a message that names both versions.
fn check_node(
    workspace: &Workspace,
    home: &Path,
    name: &str,
    minimum: (u64, u64),
) -> Result<(), String> {
    let printed = bash::execute(
        workspace.dir(),
        "node",
        &["--version"],
        engines::INSTALL_TIMEOUT_S,
        Privilege::Engine(home.to_path_buf()),
    )
    .map_err(|err| format!("cannot run node for {name}: {}", engines::tail(&err)))?;
    let printed = printed.trim();
    let version = printed
        .lines()
        .find(|line| line.starts_with('v'))
        .unwrap_or(printed);
    if node_at_least(version, minimum) {
        Ok(())
    } else {
        Err(format!(
            "{name} needs Node {}.{} or later; the VM has {version}",
            minimum.0, minimum.1
        ))
    }
}

/// True when `version`, as `node --version` prints it (`v22.19.0`), is at least `minimum`.
fn node_at_least(version: &str, minimum: (u64, u64)) -> bool {
    let mut parts = version
        .trim_start_matches('v')
        .split('.')
        .map(|part| part.parse::<u64>().ok());
    match (parts.next().flatten(), parts.next().flatten()) {
        (Some(major), Some(minor)) => (major, minor) >= minimum,
        _ => false,
    }
}

/// Installs a release archive: the asset for this VM's architecture is downloaded into the engine
/// cache, checked against its sha256 (else it is deleted and nothing is installed), extracted into the
/// engine's own folder, and its binary copied into the npm prefix's `bin`, where the launch looks.
fn install_archive(
    workspace: &Workspace,
    instance: &Instance,
    version: &str,
    url: &str,
    sha256: &str,
    cmd: &str,
) -> Result<String, String> {
    let spec = format!("{} {version}", instance.id);
    bash::install_packages(workspace, &json!({ "packages": ["curl"] }))
        .map_err(|err| format!("cannot install curl for {spec}: {err}"))?;
    let home = layout::engine_home(workspace, &instance.home);
    let cache = layout::install_cache(&home);
    files::create_dirs(&cache)
        .map_err(|err| format!("unavailable: cannot create {}: {err}", cache.display()))?;
    let archive = cache.join(archive_name(url));
    bash::execute(
        workspace.dir(),
        "curl",
        &[
            "-fsSL",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "-o",
            text_path(&archive)?,
            url,
        ],
        engines::INSTALL_TIMEOUT_S,
        Privilege::Engine(home.clone()),
    )
    .map_err(|err| format!("cannot download {spec}: {}", engines::tail(&err)))?;
    let bytes = std::fs::read(&archive).map_err(|err| format!("cannot read {spec}: {err}"))?;
    if !sha256_matches(&bytes, sha256) {
        let _ = std::fs::remove_file(&archive);
        return Err(format!("integrity mismatch for {spec}"));
    }
    let dir = engine_dir(&instance.home, version);
    files::create_dirs(&dir)
        .map_err(|err| format!("unavailable: cannot create {}: {err}", dir.display()))?;
    layout::install_in(&dir, || {
        bash::execute(
            workspace.dir(),
            "tar",
            &["-xzf", text_path(&archive)?, "-C", text_path(&dir)?],
            engines::INSTALL_TIMEOUT_S,
            Privilege::Engine(home.clone()),
        )
        .map_err(|err| format!("cannot extract {spec}: {}", engines::tail(&err)))
    })?;
    let found =
        find_binary(&dir, cmd).ok_or_else(|| format!("{spec} has no {cmd} in its archive"))?;
    let target = instance.bin_root.join("bin").join(&instance.binary);
    if let Some(bin_dir) = target.parent() {
        files::create_dirs(bin_dir)
            .map_err(|err| format!("unavailable: cannot create {}: {err}", bin_dir.display()))?;
    }
    install_binary(&found, &target).map_err(|err| format!("cannot install {spec}: {err}"))?;
    Ok(instance.binary.clone())
}

/// Installs a PyPI package into a virtualenv of its own. The VM's Python must be 3.12 or later. `pip
/// download` fetches exactly `file` (no dependencies) into the engine cache; the file must be that name
/// and hash to `sha256`, else it is deleted and nothing is installed. Then pip installs the file,
/// resolving its dependencies, and the venv's binary is copied into the npm prefix's `bin`.
fn install_pip(
    workspace: &Workspace,
    instance: &Instance,
    package: &str,
    version: &str,
    file: &str,
    sha256: &str,
) -> Result<String, String> {
    let spec = format!("{} {version}", instance.id);
    bash::install_packages(workspace, &json!({ "packages": ["python3", "py3-pip"] }))
        .map_err(|err| format!("cannot install python3 and pip for {spec}: {err}"))?;
    let home = layout::engine_home(workspace, &instance.home);
    let python = bash::execute(
        workspace.dir(),
        "python3",
        &["--version"],
        engines::INSTALL_TIMEOUT_S,
        Privilege::Engine(home.clone()),
    )
    .map_err(|err| format!("cannot run python3 for {spec}: {}", engines::tail(&err)))?;
    check_python(&instance.id, &python)?;
    let cache = layout::install_cache(&home);
    files::create_dirs(&cache)
        .map_err(|err| format!("unavailable: cannot create {}: {err}", cache.display()))?;
    let dir = engine_dir(&instance.home, version);
    files::create_dirs(&dir)
        .map_err(|err| format!("unavailable: cannot create {}: {err}", dir.display()))?;
    let venv = dir.join("venv");
    let pip = venv.join("bin").join("pip");
    let pip = text_path(&pip)?;
    // A stale copy of the file would make pip report nothing saved, so it is fetched afresh.
    let downloaded = cache.join(file);
    let _ = std::fs::remove_file(&downloaded);
    let target = format!("{package}=={version}");
    // The venv and the pip work run as the engine user inside its folder; the checks below read
    // what it fetched as root.
    layout::install_in(&dir, || {
        bash::execute(
            workspace.dir(),
            "python3",
            &["-m", "venv", text_path(&venv)?],
            engines::INSTALL_TIMEOUT_S,
            Privilege::Engine(home.clone()),
        )
        .map_err(|err| {
            format!(
                "cannot create the virtualenv for {spec}: {}",
                engines::tail(&err)
            )
        })?;
        let fetched = bash::execute(
            workspace.dir(),
            pip,
            &[
                "download",
                "--no-deps",
                "--dest",
                text_path(&cache)?,
                target.as_str(),
            ],
            engines::INSTALL_TIMEOUT_S,
            Privilege::Engine(home.clone()),
        )
        .map_err(|err| format!("cannot download {spec}: {}", engines::tail(&err)))?;
        let name = pip_saved_name(&fetched)
            .ok_or_else(|| format!("cannot download {spec}: pip saved no file"))?;
        if name != file {
            let _ = std::fs::remove_file(cache.join(name));
            return Err(format!(
                "integrity mismatch for {spec}: pip fetched {name}, not {file}"
            ));
        }
        let bytes =
            std::fs::read(&downloaded).map_err(|err| format!("cannot read {spec}: {err}"))?;
        if !sha256_matches(&bytes, sha256) {
            let _ = std::fs::remove_file(&downloaded);
            return Err(format!("integrity mismatch for {spec}"));
        }
        bash::execute(
            workspace.dir(),
            pip,
            &["install", text_path(&downloaded)?],
            engines::INSTALL_TIMEOUT_S,
            Privilege::Engine(home.clone()),
        )
        .map_err(|err| format!("cannot install {spec}: {}", engines::tail(&err)))?;
        Ok(())
    })?;
    // A catalogue engine names its command; an added one's command comes from the wheel's entry points.
    let binary = if instance.catalogue_pin {
        instance.binary.clone()
    } else {
        venv_binary_name(&venv, package)?
    };
    let found = venv.join("bin").join(&binary);
    if !found.is_file() {
        return Err(format!("{spec} has no {binary} in its virtualenv"));
    }
    let target = instance.bin_root.join("bin").join(&binary);
    if let Some(bin_dir) = target.parent() {
        files::create_dirs(bin_dir)
            .map_err(|err| format!("unavailable: cannot create {}: {err}", bin_dir.display()))?;
    }
    install_binary(&found, &target).map_err(|err| format!("cannot install {spec}: {err}"))?;
    Ok(binary)
}

/// The command a wheel installed into `venv` provides, from the console scripts in its entry points.
fn venv_binary_name(venv: &Path, package: &str) -> Result<String, String> {
    let dist = format!("{}-", package.replace('-', "_"));
    let info = std::fs::read_dir(venv.join("lib"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|python| std::fs::read_dir(python.path().join("site-packages")).ok())
        .flatten()
        .flatten()
        .find(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.starts_with(&dist) && name.ends_with(".dist-info")
        })
        .ok_or_else(|| format!("cannot find the files of {package} in its virtualenv"))?;
    let text = std::fs::read_to_string(info.path().join("entry_points.txt")).unwrap_or_default();
    let mut names = Vec::new();
    let mut scripts = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            scripts = line == "[console_scripts]";
        } else if let (true, Some((name, _))) = (scripts, line.split_once('=')) {
            let name = name.trim();
            if !name.is_empty() {
                names.push(name.to_string());
            }
        }
    }
    pick_binary(names, package)
}

/// The minimum Python a PyPI install runs under: Mistral Vibe's package requires 3.12.
const PIP_PYTHON: (u64, u64) = (3, 12);

/// Checks that `python3 --version`'s output names Python [`PIP_PYTHON`] or later.
fn check_python(id: &str, output: &str) -> Result<(), String> {
    let found = python_version(output).ok_or_else(|| {
        format!(
            "cannot read the VM's Python version from {:?}",
            output.trim()
        )
    })?;
    if found < PIP_PYTHON {
        return Err(format!(
            "{id} needs Python {}.{} or later; the VM has {}.{}",
            PIP_PYTHON.0, PIP_PYTHON.1, found.0, found.1
        ));
    }
    Ok(())
}

/// The `(major, minor)` of a `Python 3.12.3` line, when it names one.
fn python_version(output: &str) -> Option<(u64, u64)> {
    let version = output.trim().strip_prefix("Python ")?;
    let mut parts = version.split('.');
    let major = leading_number(parts.next()?)?;
    let minor = leading_number(parts.next()?)?;
    Some((major, minor))
}

/// The number at the start of `text` (`13` in `13rc1`), when there is one.
fn leading_number(text: &str) -> Option<u64> {
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// The file name `pip download` saved: the last `Saved <path>` line's file name.
fn pip_saved_name(output: &str) -> Option<&str> {
    output
        .lines()
        .rev()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("Saved "))
        .map(|path| path.trim().rsplit('/').next().unwrap_or(path))
}

/// The file name a release archive is downloaded under: the last segment of its URL.
fn archive_name(url: &str) -> &str {
    url.rsplit('/').next().unwrap_or("engine-release.tar.gz")
}

/// The folder an engine's release or virtualenv lives in: `/workspace/.engines/<home>-<version>`.
pub(crate) fn engine_dir(id: &str, version: &str) -> PathBuf {
    Path::new(ENGINES_DIR).join(format!("{id}-{version}"))
}

/// True when `bytes` hash to `sha256` (lowercase hex).
fn sha256_matches(bytes: &[u8], sha256: &str) -> bool {
    let digest: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    digest == sha256
}

/// The binary in an extracted archive: at `binary_path` under `dir` when the archive keeps it there,
/// else the first file of that name found anywhere under `dir`.
fn find_binary(dir: &Path, binary_path: &str) -> Option<PathBuf> {
    let direct = dir.join(binary_path);
    if direct.is_file() {
        return Some(direct);
    }
    find_named(dir, Path::new(binary_path).file_name()?)
}

/// The first file named `name` under `dir`, searching its folders depth first. Symbolic links are not
/// followed, so a link in the archive cannot send the search outside it.
fn find_named(dir: &Path, name: &std::ffi::OsStr) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()? {
        let Ok(entry) = entry else { continue };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            if let Some(found) = find_named(&path, name) {
                return Some(found);
            }
        } else if file_type.is_file() && path.file_name() == Some(name) {
            return Some(path);
        }
    }
    None
}

fn text_path(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("unavailable: {} is not valid UTF-8", path.display()))
}

/// The tarball's name from `npm pack`'s output: its last line that names a `.tgz` in the folder
/// it was given (npm prints the bare name; the shell's exit line follows it).
fn packed_name(output: &str) -> Option<&str> {
    output
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.ends_with(".tgz") && !line.contains('/'))
}

/// True when `bytes` hash to the registry's `integrity` (`sha512-<base64 of the digest>`).
fn integrity_matches(bytes: &[u8], integrity: &str) -> bool {
    let digest = base64::engine::general_purpose::STANDARD.encode(Sha512::digest(bytes));
    integrity.strip_prefix("sha512-") == Some(digest.as_str())
}

/// The sessions file under the workspace's `.plaza` folder. Missing is no sessions; a file that
/// does not parse is an error, so a damaged store is never silently replaced.
fn load_sessions(plaza: &Path) -> Result<Vec<Session>, String> {
    match std::fs::read(plaza.join(SESSIONS_FILE)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|err| format!("unavailable: the engine sessions file is unreadable: {err}")),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(format!(
            "unavailable: cannot read the engine sessions: {err}"
        )),
    }
}

fn save_sessions(plaza: &Path, sessions: &[Session]) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(sessions)
        .map_err(|err| format!("internal: cannot encode the engine sessions: {err}"))?;
    files::create_dirs(plaza)
        .map_err(|err| format!("unavailable: cannot create {}: {err}", plaza.display()))?;
    files::write_atomic(&plaza.join(SESSIONS_FILE), &bytes)
        .map_err(|err| format!("unavailable: cannot save the engine sessions: {err}"))
}

/// Records one turn of session `id`: refreshes its entry or adds one, then keeps the newest
/// [`MAX_SESSIONS`].
fn record(sessions: &mut Vec<Session>, engine: &str, id: &str, summary: &str, now: u64) {
    match sessions
        .iter_mut()
        .find(|session| session.engine == engine && session.id == id)
    {
        Some(session) => {
            session.last_used = now;
            session.summary = summary.to_string();
        }
        None => sessions.push(Session {
            engine: engine.to_string(),
            id: id.to_string(),
            created: now,
            last_used: now,
            summary: summary.to_string(),
        }),
    }
    sessions.sort_by_key(|a| std::cmp::Reverse(a.last_used));
    sessions.truncate(MAX_SESSIONS);
}

fn record_turn(
    plaza: &Path,
    engine: &str,
    id: &str,
    summary: &str,
    now: u64,
) -> Result<(), String> {
    let mut sessions = load_sessions(plaza)?;
    record(&mut sessions, engine, id, summary, now);
    save_sessions(plaza, &sessions)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The summary one turn returns to the model, at most [`SUMMARY_BYTES`]: the session and how the
/// turn stopped, the turn's message (its end when long), the files changed, the commands run, the
/// plan, the tool calls, and how to continue. `files` is the checkpoint's diff, of which only the
/// summary lines before its first `diff --git` are listed.
fn summary(outcome: &Outcome, engine: &str, continued_asked: bool, files: Option<&str>) -> String {
    let mut head = format!(
        "{engine} session {} — {}",
        outcome.session, outcome.stop_reason
    );
    if continued_asked && !outcome.continued {
        head.push_str(" (new session: the previous one could not be continued)");
    }

    let mut details = Vec::new();
    if let Some(list) = files.map(changed_files).filter(|list| !list.is_empty()) {
        details.push(format!("files changed:\n{list}"));
    }
    if !outcome.commands.is_empty() {
        let lines: Vec<String> = outcome
            .commands
            .iter()
            .map(|(command, code)| match code {
                Some(code) => format!("$ {command} → exit {code}"),
                None => format!("$ {command} → exit ?"),
            })
            .collect();
        details.push(format!("commands:\n{}", lines.join("\n")));
    }
    let plan = plan_lines(outcome.turn.plan.as_ref());
    if !plan.is_empty() {
        details.push(format!("plan:\n{}", plan.join("\n")));
    }
    let calls = &outcome.turn.tool_calls;
    if !calls.is_empty() {
        let failed = calls.iter().filter(|call| call.status == "failed").count();
        details.push(format!("tool calls: {} ({failed} failed)", calls.len()));
    }

    let closing = format!(
        "Continue with code_engine {{\"engine\": \"{engine}\", \"session\": \"{}\"}}",
        outcome.session
    );
    let details = keep_start(&details.join("\n\n"), DETAIL_BYTES);
    // Three separators of two bytes each sit between the four parts.
    let room = SUMMARY_BYTES.saturating_sub(head.len() + details.len() + closing.len() + 6);
    let message = keep_end(outcome.turn.message.trim(), room.min(MESSAGE_BYTES));

    let mut parts = vec![head];
    parts.extend(Some(message).filter(|message| !message.is_empty()));
    parts.extend(Some(details).filter(|details| !details.is_empty()));
    parts.push(closing);
    parts.join("\n\n")
}

/// The summary lines of a checkpoint diff: those before its first `diff --git`, at most
/// [`FILE_LINES`].
fn changed_files(diff: &str) -> String {
    diff.lines()
        .take_while(|line| !line.starts_with("diff --git"))
        .take(FILE_LINES)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The plan's entries as checkboxes: `[x]` when completed, `[ ]` otherwise. The plan update is
/// its entries, or an object holding them.
fn plan_lines(plan: Option<&Value>) -> Vec<String> {
    let entries: &[Value] = match plan {
        Some(Value::Array(list)) => list.as_slice(),
        Some(value) => value
            .get("entries")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default(),
        None => &[],
    };
    entries
        .iter()
        .filter_map(|entry| {
            let content = entry.get("content").and_then(Value::as_str)?;
            let done = entry.get("status").and_then(Value::as_str) == Some("completed");
            Some(format!("{} {content}", if done { "[x]" } else { "[ ]" }))
        })
        .collect()
}

/// The end of `text` in at most `max` bytes, on a character boundary, with a note when cut.
fn keep_end(text: &str, max: usize) -> String {
    const CUT: &str = "[earlier text cut]\n";
    if text.len() <= max {
        return text.to_string();
    }
    if max <= CUT.len() {
        return String::new();
    }
    let mut start = text.len() - (max - CUT.len());
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("{CUT}{}", &text[start..])
}

/// The start of `text` in at most `max` bytes, on a character boundary, with a note when cut.
fn keep_start(text: &str, max: usize) -> String {
    const CUT: &str = "\n[later text cut]";
    if text.len() <= max {
        return text.to_string();
    }
    if max <= CUT.len() {
        return String::new();
    }
    let mut end = max - CUT.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{CUT}", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::link::Verdict;
    use crate::acp::stream::{ToolCallSeen, Turn};

    /// A link that allows nothing and never runs a turn: the validation tests need none.
    struct NoLink;

    impl EngineLink for NoLink {
        fn decide(&self, _name: &str, _input: &Value, _deadline: Instant) -> Verdict {
            Verdict::Cancelled
        }

        fn update(&self, _engine: &str, _session: &str, _update: &Value) {}

        fn cancelled(&self) -> bool {
            false
        }
    }

    #[test]
    fn cline_setup_runs_its_binary_with_the_engine_environment_before_the_args() {
        let workspace = scratch("cline-setup");
        let instance = Instance::from_catalogue("cline").expect("cline");
        let (env, _) = engines::acp_launch(&workspace, "cline", None).expect("cline");
        let args = engines::setup_args("cline", Some("openai/gpt-x"))
            .expect("a setup")
            .expect("Cline has one");
        let argv = command(&instance, env.clone(), &args);
        assert_eq!(argv[0], engines::ENV_PROGRAM);
        assert_eq!(argv[1..=env.len()], env[..]);
        assert!(argv[env.len() + 1].ends_with("/bin/cline"));
        assert_eq!(argv[env.len() + 2..], args[..]);
        assert_eq!(argv[env.len() + 2], "auth");
    }

    #[test]
    fn cline_refuses_a_call_without_a_model_before_it_starts() {
        let workspace = scratch("cline-no-model");
        let err = engines::setup_args("cline", None).expect_err("refused");
        assert_eq!(err, "invalid input: Cline needs \"model\"");
        assert!(engines::acp_launch(&workspace, "cline", None).is_ok());
    }

    fn scratch(name: &str) -> Workspace {
        let dir =
            std::env::temp_dir().join(format!("apz-harness-acp-run-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch workspace");
        Workspace::at(dir)
    }

    fn stored(engine: &str, id: &str, summary: &str) -> Session {
        Session {
            engine: engine.to_string(),
            id: id.to_string(),
            created: 1,
            last_used: 1,
            summary: summary.to_string(),
        }
    }

    fn outcome() -> Outcome {
        Outcome {
            session: "s-1".to_string(),
            continued: true,
            stop_reason: "end_turn".to_string(),
            turn: Turn {
                message: "Fixed the parser.".to_string(),
                tool_calls: vec![
                    ToolCallSeen {
                        id: "t1".to_string(),
                        title: "edit".to_string(),
                        kind: "edit".to_string(),
                        status: "completed".to_string(),
                    },
                    ToolCallSeen {
                        id: "t2".to_string(),
                        title: "run".to_string(),
                        kind: "execute".to_string(),
                        status: "failed".to_string(),
                    },
                ],
                plan: Some(json!([
                    {"content": "read", "status": "completed", "priority": "high"},
                    {"content": "test", "status": "pending", "priority": "low"}
                ])),
                usage: None,
                mode: None,
            },
            commands: vec![
                ("cargo test".to_string(), Some(0)),
                ("ls".to_string(), None),
            ],
            checkpoint: Some("ck-1".to_string()),
            modes: Vec::new(),
        }
    }

    #[test]
    fn a_bad_call_is_refused_before_anything_is_installed() {
        let workspace = scratch("validation");
        let ids = catalog::ids();
        let cases = [
            (
                json!({"engine": "bash", "prompt": "hi"}),
                format!("invalid input: \"engine\" must be one of {ids}"),
            ),
            (
                json!({"engine": "claude", "prompt": "  "}),
                "invalid input: \"prompt\" must be a non-empty string".to_string(),
            ),
            (
                json!({"engine": "claude", "prompt": "hi", "session": "nope"}),
                "unknown engine session \"nope\" for claude".to_string(),
            ),
            (
                json!({"engine": "claude", "prompt": "hi", "timeout_s": 5}),
                "invalid input: \"timeout_s\" must be a whole number of seconds from 30 to 3600"
                    .to_string(),
            ),
            (
                json!({"engine": "claude", "prompt": "hi", "mode": 3}),
                "invalid input: \"mode\" must be a non-empty string".to_string(),
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(
                run(
                    &workspace,
                    &input,
                    &NoLink,
                    1,
                    None,
                    engines::Compaction::default()
                )
                .expect_err("a refused call"),
                expected
            );
        }
    }

    #[test]
    fn a_session_is_continued_only_for_the_engine_that_holds_it() {
        let workspace = scratch("lookup");
        let plaza = workspace.plaza_dir();
        save_sessions(&plaza, &[stored("claude", "abc", "earlier turn")]).expect("saved");
        let claude = Instance::from_catalogue("claude").expect("claude");
        let codex = Instance::from_catalogue("codex").expect("codex");
        let found = Request::parse(
            &workspace,
            &json!({"engine": "claude", "prompt": "more", "session": "abc"}),
            &claude,
        )
        .ok()
        .and_then(|request| request.resume)
        .expect("the stored session");
        assert_eq!(found.summary, "earlier turn");
        let err = Request::parse(
            &workspace,
            &json!({"engine": "codex", "prompt": "more", "session": "abc"}),
            &codex,
        )
        .err()
        .expect("another engine's session");
        assert_eq!(err, "unknown engine session \"abc\" for codex");
    }

    #[test]
    fn a_release_is_picked_for_the_vms_architecture_only() {
        // goose ships one archive per architecture: the catalogue's pin takes this VM's.
        let goose = Instance::from_catalogue("goose").expect("goose");
        let Pin::Archive { url, .. } = &goose.pin else {
            panic!("goose is an archive install");
        };
        assert!(url.ends_with(&format!(
            "{}-unknown-linux-musl.tar.gz",
            std::env::consts::ARCH
        )));
    }

    #[test]
    fn a_release_archive_must_hash_to_its_sha256_in_lowercase_hex() {
        // SHA-256 of "abc" (FIPS 180-2 example).
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(sha256_matches(b"abc", abc));
        assert!(!sha256_matches(b"abd", abc));
        assert!(!sha256_matches(b"abc", &abc.to_uppercase()));
    }

    #[test]
    fn a_python_older_than_3_12_is_refused_with_the_version_the_vm_has() {
        assert_eq!(python_version("Python 3.12.3\n"), Some((3, 12)));
        assert_eq!(python_version("Python 3.13.0rc1"), Some((3, 13)));
        assert_eq!(python_version("Python 4.0.0"), Some((4, 0)));
        assert_eq!(python_version("python3: not found"), None);
        assert!(check_python("vibe", "Python 3.12.0\n").is_ok());
        assert!(check_python("vibe", "Python 3.14.1\n").is_ok());
        assert_eq!(
            check_python("vibe", "Python 3.11.9\n").expect_err("too old"),
            "vibe needs Python 3.12 or later; the VM has 3.11"
        );
        assert!(check_python("vibe", "garbage").is_err());
    }

    #[test]
    fn the_file_pip_saved_is_named_by_its_last_saved_line() {
        let output = "Collecting mistral-vibe==2.25.0\n  Downloading mistral_vibe-2.25.0-py3-none-any.whl\n  Saved /workspace/.plaza/engine-cache/mistral_vibe-2.25.0-py3-none-any.whl\nSuccessfully downloaded mistral-vibe\n";
        assert_eq!(
            pip_saved_name(output),
            Some("mistral_vibe-2.25.0-py3-none-any.whl")
        );
        assert_eq!(
            pip_saved_name("  Saved ./mistral_vibe-2.25.0.tar.gz\n"),
            Some("mistral_vibe-2.25.0.tar.gz")
        );
        assert_eq!(pip_saved_name("File was already downloaded x.whl"), None);
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("apz-harness-run-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn an_install_is_recorded_per_version_so_a_version_bump_installs_again() {
        let prefix = scratch_dir("recorded");
        assert!(!is_recorded(&prefix, "claude", "1.0.0"));
        record_install(&prefix, "claude", "1.0.0", "claude-agent-acp").expect("recorded");
        assert!(is_recorded(&prefix, "claude", "1.0.0"));
        assert!(!is_recorded(&prefix, "claude", "1.1.0"));
        assert!(!is_recorded(&prefix, "cline", "1.0.0"));
        let _ = std::fs::remove_dir_all(&prefix);
    }

    #[test]
    fn a_binary_is_replaced_through_a_tmp_file_and_a_failed_copy_leaves_the_old_one() {
        let dir = scratch_dir("binary");
        std::fs::create_dir_all(&dir).expect("a dir");
        let from = dir.join("from");
        let to = dir.join("to");
        std::fs::write(&from, b"new bytes").expect("a source");
        std::fs::write(&to, b"old bytes").expect("an old binary");
        install_binary(&from, &to).expect("copied");
        assert_eq!(std::fs::read(&to).expect("read"), b"new bytes");
        assert!(!dir.join("to.tmp").exists());
        assert!(install_binary(&dir.join("missing"), &to).is_err());
        assert_eq!(
            std::fs::read(&to).expect("read"),
            b"new bytes",
            "the installed binary is not touched by a failed copy"
        );
        assert!(!dir.join("to.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_v2_offer_is_made_only_to_the_acp2_transport() {
        assert_eq!(protocol_offer("acp2"), 2);
        assert_eq!(protocol_offer(""), 1);
    }

    #[test]
    fn the_integrity_check_compares_the_sha512_of_the_bytes() {
        let abc = "sha512-3a81oZNherrMQXNJriBBMRLm+k6JqX6iCp7u5ktV05ohkpkqJ0/BqDa6PCOj/uu9RU1EI2Q86A4qmslPpUyknw==";
        assert!(integrity_matches(b"abc", abc));
        assert!(!integrity_matches(b"abd", abc));
        assert!(!integrity_matches(
            b"abc",
            abc.trim_start_matches("sha512-")
        ));
    }

    #[test]
    fn the_tarball_name_is_npms_last_line_that_names_a_tgz() {
        assert_eq!(
            packed_name("notice\nagentclientprotocol-claude-agent-acp-0.84.0.tgz\nexit 0\n"),
            Some("agentclientprotocol-claude-agent-acp-0.84.0.tgz")
        );
        assert_eq!(packed_name("npm ERR! nothing\nexit 1\n"), None);
        assert_eq!(packed_name("x/evil.tgz\n"), None);
    }

    #[test]
    fn the_adapter_argv_starts_with_its_environment_and_binary_and_takes_the_call_s_model() {
        // Checked: the adapter starts with its environment and binary.
        {
            let instance = Instance::from_catalogue("opencode").expect("opencode");
            assert_eq!(
                argv(&instance, vec!["HOME=/workspace".to_string()], &[]),
                [
                    engines::ENV_PROGRAM,
                    "HOME=/workspace",
                    "/workspace/.npm-global/bin/opencode",
                    "acp"
                ]
            );
        }
        // Checked: auggie takes the call's model after its own arguments.
        {
            let instance = Instance::from_catalogue("auggie").expect("auggie");
            let extra = engines::acp_args("auggie", Some("augment/claude-x")).expect("a model");
            assert_eq!(
                argv(&instance, vec!["HOME=/workspace".to_string()], &extra),
                [
                    engines::ENV_PROGRAM,
                    "HOME=/workspace",
                    "/workspace/.npm-global/bin/auggie",
                    "--acp",
                    "--model",
                    "claude-x"
                ]
            );
        }
    }

    #[test]
    fn kimi_needs_node_22_19_or_later() {
        let (name, minimum) = node_minimum("kimi").expect("kimi names a minimum");
        assert_eq!((name, minimum), ("Kimi Code", (22, 19)));
        assert!(node_at_least("v22.19.0", minimum));
        assert!(node_at_least("v22.20.1", minimum));
        assert!(node_at_least("v24.1.0", minimum));
        assert!(!node_at_least("v22.18.0", minimum));
        assert!(!node_at_least("v20.11.1", minimum));
        assert!(!node_at_least("node: not found", minimum));
        assert!(node_minimum("claude").is_none());
    }

    #[test]
    fn the_launch_names_the_model_the_way_each_engine_takes_it() {
        let workspace = scratch("launch");
        let (env, model) =
            engines::acp_launch(&workspace, "claude", Some("anthropic/claude-x")).expect("claude");
        assert_eq!(
            env[0],
            format!(
                "HOME={}",
                workspace.dir().join(".engines/home/claude").display()
            )
        );
        assert!(env.contains(&"ANTHROPIC_BASE_URL=http://plaza.gateway/anthropic".to_string()));
        assert_eq!(model.as_deref(), Some("claude-x"));
        let (env, model) = engines::acp_launch(&workspace, "opencode", Some("opencode/claude-x"))
            .expect("opencode");
        assert_eq!(model.as_deref(), Some("plaza-opencode-anthropic/claude-x"));
        assert!(
            env.iter()
                .any(|pair| pair.starts_with("OPENCODE_CONFIG_CONTENT="))
        );
        assert_eq!(
            engines::acp_launch(&workspace, "codex", None)
                .expect("codex")
                .1,
            None
        );
    }

    #[test]
    fn the_sessions_store_round_trips_and_keeps_the_newest() {
        let workspace = scratch("store");
        let plaza = workspace.plaza_dir();
        assert!(load_sessions(&plaza).expect("empty").is_empty());
        let mut sessions = Vec::new();
        record(&mut sessions, "claude", "a", "first", 10);
        record(&mut sessions, "codex", "b", "second", 20);
        save_sessions(&plaza, &sessions).expect("saved");
        assert_eq!(load_sessions(&plaza).expect("loaded"), sessions);

        record(&mut sessions, "claude", "a", "again", 30);
        assert_eq!(sessions[0].id, "a");
        assert_eq!(sessions[0].summary, "again");
        assert_eq!(sessions[0].created, 10);
        for n in 0..60 {
            record(&mut sessions, "claude", &format!("s{n}"), "x", 100 + n);
        }
        assert_eq!(sessions.len(), MAX_SESSIONS);
        assert!(sessions.iter().all(|session| session.id != "b"));
    }

    #[test]
    fn a_damaged_sessions_file_is_an_error_not_an_empty_store() {
        let workspace = scratch("damaged");
        let plaza = workspace.plaza_dir();
        std::fs::create_dir_all(&plaza).expect("the folder");
        std::fs::write(plaza.join(SESSIONS_FILE), b"{not json").expect("the file");
        assert!(load_sessions(&plaza).is_err());
    }

    #[test]
    fn a_session_that_could_not_be_continued_says_so() {
        let mut turn = outcome();
        turn.continued = false;
        let text = summary(&turn, "codex", true, None);
        assert!(text.starts_with(
            "codex session s-1 — end_turn (new session: the previous one could not be continued)"
        ));
        assert!(!text.contains("files changed:"));
    }

    #[test]
    fn a_long_turn_is_cut_to_its_end_within_the_summary_cap() {
        let mut long = outcome();
        long.turn.message = format!("{}THE END", "x".repeat(40 * 1024));
        let text = summary(&long, "claude", false, None);
        assert!(text.len() <= SUMMARY_BYTES, "{} bytes", text.len());
        assert!(text.contains("THE END"));
        assert!(text.contains("[earlier text cut]"));
        assert!(text.ends_with("\"session\": \"s-1\"}"));
    }
}

#[cfg(test)]
mod spec_launch_tests {
    use std::collections::BTreeMap;

    use apz_proto::wire::{EngineInstall, EngineSpec};

    use super::*;
    use crate::files::Workspace;

    fn scratch(name: &str) -> Workspace {
        let dir =
            std::env::temp_dir().join(format!("apz-harness-spec-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch workspace");
        Workspace::at(dir)
    }

    #[test]
    fn the_route_is_the_model_only_when_the_call_names_none() {
        let ws = scratch("route");
        let spec = EngineSpec {
            engine: "claude".to_string(),
            route: Some("anthropic/claude-x".to_string()),
            ..EngineSpec::default()
        };
        let instance = Instance::resolve(&ws, "claude", Some(&spec)).expect("claude");
        let bare = json!({"engine": "claude", "prompt": "hi"});
        let request = Request::parse(&ws, &bare, &instance).expect("parsed");
        assert_eq!(request.model.as_deref(), Some("anthropic/claude-x"));
        let named = json!({"engine": "claude", "prompt": "hi", "model": "openrouter/x/y"});
        let request = Request::parse(&ws, &named, &instance).expect("parsed");
        assert_eq!(request.model.as_deref(), Some("openrouter/x/y"));
    }

    #[test]
    fn a_local_command_launches_with_the_gateway_route_and_the_owners_args_and_env() {
        let ws = scratch("local");
        let mut env = BTreeMap::new();
        // The harness's routing wins over the owner's value; the owner's own name is passed on.
        env.insert("ANTHROPIC_BASE_URL".to_string(), "http://evil".to_string());
        env.insert("FOO".to_string(), "1".to_string());
        let spec = EngineSpec {
            engine: "acp-local".to_string(),
            install: Some(EngineInstall::Command {
                command: "/usr/bin/my-agent".to_string(),
            }),
            args: vec!["--stdio".to_string()],
            env,
            ..EngineSpec::default()
        };
        let instance = Instance::resolve(&ws, "local", Some(&spec)).expect("local");
        let (pairs, model) = launch(
            &ws,
            &instance,
            Some("anthropic/claude-x"),
            engines::Compaction::default(),
        )
        .expect("launched");
        assert!(pairs.contains(&"ANTHROPIC_BASE_URL=http://plaza.gateway/anthropic".to_string()));
        assert!(pairs.contains(&"FOO=1".to_string()));
        assert!(!pairs.iter().any(|pair| pair.contains("evil")));
        assert_eq!(model.as_deref(), Some("claude-x"));
        let line = argv(&instance, pairs, &[]);
        assert_eq!(line[0], engines::ENV_PROGRAM);
        assert!(line.iter().any(|arg| arg.starts_with("HOME=")));
        assert_eq!(line[line.len() - 2..], ["/usr/bin/my-agent", "--stdio"]);
    }

    #[test]
    fn an_install_names_its_one_command_or_the_acp_one() {
        assert_eq!(
            pick_binary(vec!["only".to_string()], "p").as_deref(),
            Ok("only")
        );
        let names = vec!["vibe".to_string(), "vibe-acp".to_string()];
        assert_eq!(pick_binary(names, "p").as_deref(), Ok("vibe-acp"));
        let names = vec!["a".to_string(), "b".to_string()];
        assert!(pick_binary(names, "p").is_err());
    }

    #[test]
    fn a_pin_refuses_bytes_that_do_not_match_it() {
        assert!(!integrity_matches(b"tarball", "sha512-AAAA"));
        assert!(!sha256_matches(b"archive", &"0".repeat(64)));
        assert!(sha256_matches(
            b"",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        ));
    }
}
