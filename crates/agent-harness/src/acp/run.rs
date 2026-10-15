//! `code_engine` over ACP: one prompt turn of an engine session.
//!
//! The call is validated first, so a bad call installs nothing. The engine's pinned adapter is
//! installed when its binary is missing, from the exact tarball whose sha512 the catalogue names.
//! One prompt turn then runs through [`session::prompt_turn`]. The session it ends in is kept in
//! `<workspace>/.plaza/engine-sessions.json`, so a later call can continue it, and the turn's
//! summary is what the model reads back.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256, Sha512};

use super::catalog::{self, Asset, Entry, Install};
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
const CACHE_DIR: &str = "engine-cache";
/// Where a release engine is extracted, one folder per engine and version.
const ENGINES_DIR: &str = "/workspace/.engines";

/// Runs one `code_engine` call as an ACP prompt turn and returns its summary for the model.
pub fn run(workspace: &Workspace, input: &Value, link: &dyn EngineLink) -> Result<String, String> {
    let request = Request::parse(workspace, input)?;
    let entry = request.entry;
    install(workspace, entry)?;
    let (env, model) = engines::acp_launch(workspace, entry.id, request.model.as_deref())?;
    // Before the turn and on every call: the setup rewrites the engine's saved settings, so a new
    // model takes effect.
    if let Some(args) = engines::setup_args(entry.id, request.model.as_deref())? {
        setup(workspace, entry, env.clone(), &args)?;
    }
    let options = Options {
        entry,
        workspace,
        cwd: request.cwd.clone(),
        argv: argv(entry, env),
        model,
        mode: request.mode.clone(),
        resume: request.resume.as_ref().map(|session| session.id.clone()),
        context: request
            .resume
            .as_ref()
            .map(|session| session.summary.clone()),
        deadline: Instant::now() + Duration::from_secs(request.timeout_s),
    };
    let outcome = session::prompt_turn(&options, &request.prompt, link)?;

    let message = keep_end(outcome.turn.message.trim(), STORED_SUMMARY_BYTES);
    let recorded = record_turn(
        &workspace.plaza_dir(),
        entry.id,
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
        entry.id,
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

/// A validated call: the engine, its prompt and options, and the session it continues.
struct Request {
    entry: &'static Entry,
    prompt: String,
    timeout_s: u64,
    model: Option<String>,
    mode: Option<String>,
    cwd: PathBuf,
    resume: Option<Session>,
}

impl Request {
    fn parse(workspace: &Workspace, input: &Value) -> Result<Self, String> {
        let id = input
            .get("engine")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let entry = catalog::entry(id).ok_or_else(|| {
            format!(
                "invalid input: \"engine\" must be one of {}",
                catalog::ids()
            )
        })?;
        let prompt = match input.get("prompt").and_then(Value::as_str) {
            Some(prompt) if !prompt.trim().is_empty() => prompt.to_string(),
            _ => return Err("invalid input: \"prompt\" must be a non-empty string".to_string()),
        };
        let resume = match engines::optional_text(input, "session")? {
            None => None,
            Some(id) => Some(
                load_sessions(&workspace.plaza_dir())?
                    .into_iter()
                    .find(|session| session.engine == entry.id && session.id == id)
                    .ok_or_else(|| format!("unknown engine session {id:?} for {}", entry.id))?,
            ),
        };
        Ok(Self {
            entry,
            prompt,
            timeout_s: engines::timeout_arg(input)?,
            model: engines::optional_text(input, "model")?.map(str::to_string),
            mode: engines::optional_text(input, "mode")?.map(str::to_string),
            cwd: engines::job_folder(workspace, input.get("path"))?,
            resume,
        })
    }
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

/// The program the ACP session starts: `/usr/bin/env` with the engine's pairs, the installed
/// binary and its arguments. The model is not among them: every engine takes it as a session config
/// option.
fn argv(entry: &Entry, env: Vec<String>) -> Vec<String> {
    let args: Vec<String> = entry.args.iter().map(|arg| arg.to_string()).collect();
    command(entry, env, &args)
}

/// `/usr/bin/env` with the engine's pairs, the installed binary and `args`: the one command line
/// both the session and the setup run.
fn command(entry: &Entry, mut env: Vec<String>, args: &[String]) -> Vec<String> {
    let mut argv = vec![engines::ENV_PROGRAM.to_string()];
    argv.append(&mut env);
    argv.push(npm_binary(entry).display().to_string());
    argv.extend(args.iter().cloned());
    argv
}

/// Runs an engine's setup command (see [`engines::setup_args`]) as the agent, in the workspace and
/// with the engine's environment. A failure says which engine and carries the command's tail.
fn setup(
    workspace: &Workspace,
    entry: &Entry,
    env: Vec<String>,
    args: &[String],
) -> Result<(), String> {
    let mut argv = command(entry, env, args);
    let program = argv.remove(0);
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    bash::execute(
        workspace.dir(),
        &program,
        &refs,
        SETUP_TIMEOUT_S,
        Privilege::Agent,
    )
    .map(|_| ())
    .map_err(|err| format!("cannot set up {}: {}", entry.id, engines::tail(&err)))
}

/// The engine's binary in the npm prefix that installs go to.
fn npm_binary(entry: &Entry) -> PathBuf {
    Path::new(engines::NPM_PREFIX)
        .join("bin")
        .join(entry.binary)
}

/// Installs the engine when its binary is missing, by its install method. Either way the bytes that
/// run are the ones whose checksum the catalogue names: a mismatch deletes the download and installs
/// nothing.
fn install(workspace: &Workspace, entry: &Entry) -> Result<(), String> {
    let binary = npm_binary(entry);
    if binary.exists() {
        return Ok(());
    }
    match entry.install {
        Install::Npm {
            package,
            version,
            integrity,
        } => install_npm(workspace, entry, package, version, integrity),
        Install::Release {
            version,
            assets,
            binary_path,
        } => install_release(workspace, entry, version, assets, binary_path),
    }
}

/// Installs an npm package's pinned version. `npm pack` fetches the exact version into the
/// workspace's engine cache; the tarball must match the catalogue's integrity (else it is deleted and
/// nothing is installed), and only then is it installed globally.
fn install_npm(
    workspace: &Workspace,
    entry: &Entry,
    package: &str,
    version: &str,
    integrity: &str,
) -> Result<(), String> {
    let binary = npm_binary(entry);
    engines::ensure_node(workspace)
        .map_err(|err| format!("cannot install nodejs and npm for {}: {err}", entry.id))?;
    let spec = format!("{package}@{version}");
    let cache = workspace.plaza_dir().join(CACHE_DIR);
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
        Privilege::Agent,
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
    bash::execute(
        workspace.dir(),
        "npm",
        &[
            "install",
            "-g",
            "--prefix",
            engines::NPM_PREFIX,
            text_path(&tarball)?,
        ],
        engines::INSTALL_TIMEOUT_S,
        Privilege::Agent,
    )
    .map_err(|err| format!("cannot install {spec}: {}", engines::tail(&err)))?;
    if binary.exists() {
        Ok(())
    } else {
        Err(format!(
            "npm installed {spec} but {} is missing",
            binary.display()
        ))
    }
}

/// Installs a release archive: the asset for this VM's architecture is downloaded into the engine
/// cache, checked against its sha256 (else it is deleted and nothing is installed), extracted into the
/// engine's own folder, and its binary copied into the npm prefix's `bin`, where the launch looks.
fn install_release(
    workspace: &Workspace,
    entry: &Entry,
    version: &str,
    assets: &[Asset],
    binary_path: &str,
) -> Result<(), String> {
    let spec = format!("{} {version}", entry.id);
    let arch = std::env::consts::ARCH;
    let asset = asset_for(assets, arch)
        .ok_or_else(|| format!("cannot install {spec}: no build for {arch}"))?;
    bash::install_packages(workspace, &json!({ "packages": ["curl"] }))
        .map_err(|err| format!("cannot install curl for {spec}: {err}"))?;
    let cache = workspace.plaza_dir().join(CACHE_DIR);
    files::create_dirs(&cache)
        .map_err(|err| format!("unavailable: cannot create {}: {err}", cache.display()))?;
    let archive = cache.join(archive_name(asset.url));
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
            asset.url,
        ],
        engines::INSTALL_TIMEOUT_S,
        Privilege::Agent,
    )
    .map_err(|err| format!("cannot download {spec}: {}", engines::tail(&err)))?;
    let bytes = std::fs::read(&archive).map_err(|err| format!("cannot read {spec}: {err}"))?;
    if !sha256_matches(&bytes, asset.sha256) {
        let _ = std::fs::remove_file(&archive);
        return Err(format!("integrity mismatch for {} {version}", entry.id));
    }
    let dir = release_dir(entry.id, version);
    files::create_dirs(&dir)
        .map_err(|err| format!("unavailable: cannot create {}: {err}", dir.display()))?;
    bash::execute(
        workspace.dir(),
        "tar",
        &["-xzf", text_path(&archive)?, "-C", text_path(&dir)?],
        engines::INSTALL_TIMEOUT_S,
        Privilege::Agent,
    )
    .map_err(|err| format!("cannot extract {spec}: {}", engines::tail(&err)))?;
    let found = find_binary(&dir, binary_path)
        .ok_or_else(|| format!("{spec} has no {binary_path} in its archive"))?;
    let binary = npm_binary(entry);
    if let Some(bin_dir) = binary.parent() {
        files::create_dirs(bin_dir)
            .map_err(|err| format!("unavailable: cannot create {}: {err}", bin_dir.display()))?;
    }
    std::fs::copy(&found, &binary).map_err(|err| format!("cannot install {spec}: {err}"))?;
    Ok(())
}

/// The release archive for `arch` (`std::env::consts::ARCH`), if the engine ships one for it.
fn asset_for<'a>(assets: &'a [Asset], arch: &str) -> Option<&'a Asset> {
    assets.iter().find(|asset| asset.arch == arch)
}

/// The file name a release archive is downloaded under: the last segment of its URL.
fn archive_name(url: &str) -> &str {
    url.rsplit('/').next().unwrap_or("engine-release.tar.gz")
}

/// The folder a release is extracted into: `/workspace/.engines/<id>-<version>`.
fn release_dir(id: &str, version: &str) -> PathBuf {
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
    sessions.sort_by(|a, b| b.last_used.cmp(&a.last_used));
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
        let entry = catalog::entry("cline").expect("cline");
        let (env, _) = engines::acp_launch(&workspace, "cline", None).expect("cline");
        let args = engines::setup_args("cline", Some("openai/gpt-x"))
            .expect("a setup")
            .expect("Cline has one");
        let argv = command(entry, env.clone(), &args);
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
                run(&workspace, &input, &NoLink).expect_err("a refused call"),
                expected
            );
        }
    }

    #[test]
    fn a_session_is_continued_only_for_the_engine_that_holds_it() {
        let workspace = scratch("lookup");
        let plaza = workspace.plaza_dir();
        save_sessions(&plaza, &[stored("claude", "abc", "earlier turn")]).expect("saved");
        let found = Request::parse(
            &workspace,
            &json!({"engine": "claude", "prompt": "more", "session": "abc"}),
        )
        .ok()
        .and_then(|request| request.resume)
        .expect("the stored session");
        assert_eq!(found.summary, "earlier turn");
        let err = Request::parse(
            &workspace,
            &json!({"engine": "codex", "prompt": "more", "session": "abc"}),
        )
        .err()
        .expect("another engine's session");
        assert_eq!(err, "unknown engine session \"abc\" for codex");
    }

    #[test]
    fn a_release_is_picked_for_the_vms_architecture_only() {
        let goose = catalog::entry("goose").expect("goose");
        let Install::Release { assets, .. } = goose.install else {
            panic!("goose is a release install");
        };
        assert!(
            asset_for(assets, "x86_64")
                .expect("x86_64")
                .url
                .ends_with("x86_64-unknown-linux-musl.tar.gz")
        );
        assert!(
            asset_for(assets, "aarch64")
                .expect("aarch64")
                .url
                .ends_with("aarch64-unknown-linux-musl.tar.gz")
        );
        assert!(asset_for(assets, "riscv64").is_none());
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
    fn a_release_extracts_into_its_own_folder_and_is_named_by_its_url_tail() {
        assert_eq!(
            release_dir("goose", "1.52.0"),
            Path::new("/workspace/.engines/goose-1.52.0")
        );
        assert_eq!(
            archive_name("https://example.com/a/goose-x86_64.tar.gz"),
            "goose-x86_64.tar.gz"
        );
    }

    #[test]
    fn the_binary_is_found_where_the_archive_keeps_it_or_in_a_folder() {
        let workspace = scratch("release-binary");
        let dir = workspace.dir().join("extracted");
        std::fs::create_dir_all(dir.join("goose-1.52.0/bin")).expect("a folder");
        std::fs::write(dir.join("goose-1.52.0/bin/goose"), b"#!/bin/sh\n").expect("a file");
        std::fs::write(dir.join("README"), b"").expect("a file");
        let nested = dir.join("goose-1.52.0/bin/goose");
        assert_eq!(find_binary(&dir, "goose"), Some(nested.clone()));
        assert_eq!(find_binary(&dir, "bin/goose"), Some(nested));
        assert_eq!(find_binary(&dir, "missing"), None);
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
    fn the_adapter_starts_with_its_environment_and_binary() {
        let entry = catalog::entry("opencode").expect("opencode");
        assert_eq!(
            argv(entry, vec!["HOME=/workspace".to_string()]),
            [
                engines::ENV_PROGRAM,
                "HOME=/workspace",
                "/workspace/.npm-global/bin/opencode",
                "acp"
            ]
        );
    }

    #[test]
    fn the_launch_names_the_model_the_way_each_engine_takes_it() {
        let workspace = scratch("launch");
        let (env, model) =
            engines::acp_launch(&workspace, "claude", Some("anthropic/claude-x")).expect("claude");
        assert_eq!(env[0], format!("HOME={}", workspace.dir().display()));
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
    fn the_summary_says_what_the_turn_did_and_how_to_continue() {
        let files = "M src/lib.rs\n\ndiff --git a/src/lib.rs b/src/lib.rs\n+fn x() {}";
        let text = summary(&outcome(), "claude", true, Some(files));
        let expected = [
            "claude session s-1 — end_turn",
            "Fixed the parser.",
            "files changed:\nM src/lib.rs",
            "commands:\n$ cargo test → exit 0\n$ ls → exit ?",
            "plan:\n[x] read\n[ ] test",
            "tool calls: 2 (1 failed)",
            "Continue with code_engine {\"engine\": \"claude\", \"session\": \"s-1\"}",
        ];
        let mut at = 0;
        for part in expected {
            let found = text[at..].find(part).expect(part);
            at += found + part.len();
        }
        assert!(!text.contains("diff --git"));
        assert!(!text.contains("new session"));
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

    #[test]
    fn a_long_file_list_keeps_its_first_lines_only() {
        let listed: String = (0..60).map(|n| format!("M file{n}\n")).collect();
        let lines = changed_files(&listed);
        assert_eq!(lines.lines().count(), FILE_LINES);
        assert!(lines.starts_with("M file0\n"));
    }
}
