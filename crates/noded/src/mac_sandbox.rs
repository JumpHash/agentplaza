//! The macOS node's host sandbox for Xcode work (PLAN 20.2). There is no VM between a session and the Mac, so the
//! OS provides the walls: a macOS account per project, a Seatbelt profile per session, a packet filter rule that
//! keeps a project account's egress on its local proxy, a keychain per project, APFS clones of a warm DerivedData
//! and simulator set per session, and caps on builds and simulators by the host's cores and memory.
//!
//! Account creation and the packet filter need rights a node may not have, so this module builds their command
//! lines and rule text and runs them only through a [`Runner`] the caller gives: the tests use a fake one. Seatbelt,
//! the keychain and the clones are exercised for real on macOS.
//!
//! A session's directories, under its own root:
//!
//! ```text
//! <root>/workspace     the working directory (the checkout)
//! <root>/DerivedData   the build products: a clone of the project's warm DerivedData
//! <root>/devices       the session's own simulator device set: a clone of the project's warm set
//! <root>/home          HOME and TMPDIR: the tools' caches and temporary files
//! ```
//!
//! The session may write its workspace, its DerivedData, its devices and its home, and read the Xcode bundle it
//! is pinned to. Nothing else of the owner's or another session's is granted. Its network is the local proxy on
//! one port.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use apz_egress_rules::project_party;
use apz_sandbox::{Limits, Network, Spec};
use serde::Serialize;
use vmm::clonefile::{CopyMode, clone_file};
use vsockd::apid::Apid;
use vsockd::egress::{Policy, serve_tcp};

/// The keychain and account tools: fixed system binaries.
const SECURITY: &str = "/usr/bin/security";
const DSCL: &str = "/usr/bin/dscl";
/// The `staff` group (gid 20): a project account's primary group, so no group is created per project.
const STAFF_GID: u32 = 20;
/// The uids project accounts take: above the system's own accounts (below 500) and clear of the owner's (501).
const USER_UIDS: RangeInclusive<u32> = 600..=999;
/// The longest account name macOS takes.
const MAX_NAME_LEN: usize = 31;
/// Memory the host keeps for itself before simulators are counted.
const HOST_RESERVE_MIB: u64 = 4096;
/// Memory one simulator takes, as a round figure for the cap: a booted iOS simulator takes a gigabyte or two.
const SIMULATOR_MIB: u64 = 2048;
/// Cores one build takes, as a round figure: a build saturates the cores it gets.
const BUILD_CORES: u32 = 4;

/// The builds and simulators a Mac node runs at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    pub builds: u32,
    pub simulators: u32,
}

/// What is running now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Load {
    pub builds: u32,
    pub simulators: u32,
}

/// One more unit of work the node is asked to start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Work {
    Build,
    Simulator,
}

impl Caps {
    /// The caps for a host with `cores` CPUs and `memory_mib` of memory: one build per [`BUILD_CORES`] cores (at
    /// least one), and one simulator per [`SIMULATOR_MIB`] above [`HOST_RESERVE_MIB`].
    pub fn for_host(cores: u32, memory_mib: u64) -> Self {
        Caps {
            builds: (cores / BUILD_CORES).max(1),
            simulators: u32::try_from(memory_mib.saturating_sub(HOST_RESERVE_MIB) / SIMULATOR_MIB)
                .unwrap_or(u32::MAX),
        }
    }

    /// Whether one more `want` may start beside `running`. The refusal says which cap is full.
    pub fn admit(&self, running: Load, want: Work) -> Result<(), String> {
        match want {
            Work::Build if running.builds >= self.builds => {
                Err(format!("the node runs its cap of {} builds", self.builds))
            }
            Work::Simulator if running.simulators >= self.simulators => Err(format!(
                "the node runs its cap of {} simulators",
                self.simulators
            )),
            _ => Ok(()),
        }
    }
}

/// A Mac node's caps and what it runs now, as its machine facts carry them (`mac`): placement refuses a build or a
/// simulator a node is at its cap for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MacFacts {
    pub builds: u32,
    pub simulators: u32,
    pub builds_running: u32,
    pub simulators_running: u32,
}

/// The facts a node reports for its Mac caps: the caps for its cores and memory, and `load` as it runs now. None off
/// macOS, and when the memory is unknown (the caps need it).
pub fn facts_for(memory_mib: Option<u64>, cores: u32, load: Load) -> Option<MacFacts> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let caps = Caps::for_host(cores, memory_mib?);
    Some(MacFacts {
        builds: caps.builds,
        simulators: caps.simulators,
        builds_running: load.builds,
        simulators_running: load.simulators,
    })
}

/// A command line: the program and its arguments, run as they are (no shell). `env` is the child's whole
/// environment; empty means it inherits the caller's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl Invocation {
    fn new(program: &str, args: &[&str]) -> Self {
        Invocation {
            program: program.to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            env: Vec::new(),
        }
    }

    /// The process this line starts, with its environment.
    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        if !self.env.is_empty() {
            command
                .env_clear()
                .envs(self.env.iter().map(|(k, v)| (k, v)));
        }
        command
    }
}

/// What a command line answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    pub ok: bool,
    pub stdout: String,
}

/// Runs command lines. The node gives [`System`] where it has the rights; tests give a fake.
pub trait Runner {
    fn run(&mut self, line: &Invocation) -> io::Result<Ran>;
}

/// Runs each line as a process.
pub struct System;

impl Runner for System {
    fn run(&mut self, line: &Invocation) -> io::Result<Ran> {
        let out = line.command().output()?;
        Ok(Ran {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        })
    }
}

/// Starts command lines and leaves them running: a session's process. Returns its pid. The node gives [`System`];
/// tests give a fake.
pub trait Spawner {
    fn spawn(&mut self, line: &Invocation) -> io::Result<u32>;
}

impl Spawner for System {
    fn spawn(&mut self, line: &Invocation) -> io::Result<u32> {
        // The child is not waited for: dropping its handle leaves it running.
        Ok(line.command().spawn()?.id())
    }
}

/// A macOS account for one project. Its sessions run as it, so the OS keeps their files, keychain and simulator
/// devices apart from the owner's and from other projects'.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectUser {
    pub name: String,
    pub uid: u32,
    pub home: PathBuf,
}

impl ProjectUser {
    /// The account for `project` (lowercase letters, digits, `-` and `_`; `-` becomes `_`), at `uid`, with `home`
    /// as its home directory. The name is `apz_` and the project id.
    pub fn new(project: &str, uid: u32, home: &Path) -> Result<Self, String> {
        let slug: String = project
            .chars()
            .map(|c| if c == '-' { '_' } else { c })
            .collect();
        let name = format!("apz_{slug}");
        let valid_slug = !slug.is_empty()
            && slug
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !valid_slug {
            return Err(format!(
                "project id {project:?} does not make an account name"
            ));
        }
        if name.len() > MAX_NAME_LEN {
            return Err(format!(
                "account name {name} is longer than {MAX_NAME_LEN} characters"
            ));
        }
        if !USER_UIDS.contains(&uid) {
            return Err(format!("uid {uid} is outside the project accounts' range"));
        }
        if !mac_absolute(home) {
            return Err(format!("home must be absolute: {}", home.display()));
        }
        Ok(ProjectUser {
            name,
            uid,
            home: home.to_path_buf(),
        })
    }
}

/// The lowest project uid no account in `taken` has, or None when the range is full.
pub fn free_uid(taken: &[u32]) -> Option<u32> {
    USER_UIDS.clone().find(|uid| !taken.contains(uid))
}

fn dscl(args: &[&str]) -> Invocation {
    Invocation::new(DSCL, args)
}

/// The record path of an account in the local directory.
fn record(user: &ProjectUser) -> String {
    format!("/Users/{}", user.name)
}

/// The `dscl` lines that create the account: a hidden user with no login shell and no password login, in the
/// staff group, at its uid and home.
pub fn create_user_commands(user: &ProjectUser) -> Vec<Invocation> {
    let record = record(user);
    let uid = user.uid.to_string();
    let gid = STAFF_GID.to_string();
    let home = user.home.display().to_string();
    let attributes = [
        ("UniqueID", uid.as_str()),
        ("PrimaryGroupID", gid.as_str()),
        ("UserShell", "/usr/bin/false"),
        ("NFSHomeDirectory", home.as_str()),
        ("IsHidden", "1"),
        ("Password", "*"),
    ];
    let mut lines = vec![dscl(&[".", "-create", &record])];
    for (key, value) in attributes {
        lines.push(dscl(&[".", "-create", &record, key, value]));
    }
    lines
}

/// The uid the account's `UniqueID` line reports (`dscl . -read` answers `UniqueID: 601`).
fn uid_in(answer: &str) -> Option<u32> {
    answer
        .lines()
        .find_map(|line| line.strip_prefix("UniqueID:"))
        .and_then(|value| value.trim().parse().ok())
}

/// Makes sure the account exists with the project's uid: an account already there at that uid is kept; one at
/// another uid is refused, and a missing one is created by [`create_user_commands`].
pub fn ensure_user(runner: &mut dyn Runner, user: &ProjectUser) -> Result<(), String> {
    let read = dscl(&[".", "-read", &record(user), "UniqueID"]);
    let found = runner
        .run(&read)
        .map_err(|err| format!("reading account {}: {err}", user.name))?;
    if found.ok {
        return match uid_in(&found.stdout) {
            Some(uid) if uid == user.uid => Ok(()),
            _ => Err(format!("account {} exists with another uid", user.name)),
        };
    }
    for line in create_user_commands(user) {
        let ran = runner
            .run(&line)
            .map_err(|err| format!("{}: {err}", line.program))?;
        if !ran.ok {
            return Err(format!("{} {} failed", line.program, line.args.join(" ")));
        }
    }
    Ok(())
}

/// The packet filter rules for a project account's egress (pure text, for the account's anchor): its TCP on
/// loopback reaches the project's proxy on `proxy_port`, and nothing else leaves it. The proxy runs as another
/// user, so its own traffic is not caught by the block. The text is not loaded here.
pub fn pf_rules(user: &ProjectUser, proxy_port: u16) -> Result<String, String> {
    if proxy_port == 0 {
        return Err("the proxy port must not be 0".to_owned());
    }
    let uid = user.uid;
    Ok(format!(
        "# project account {name} (uid {uid}): its egress reaches the local proxy and nothing else\n\
         pass out quick on lo0 inet proto tcp from any to 127.0.0.1 port {proxy_port} user {uid}\n\
         pass out quick on lo0 inet6 proto tcp from any to ::1 port {proxy_port} user {uid}\n\
         block drop out quick user {uid}\n",
        name = user.name,
    ))
}

/// The Developer directory of the Xcode a project pins. `pin` is the bundle's file name under `apps` (for
/// example `Xcode_16.4.app`); the result is its `Contents/Developer`, which DEVELOPER_DIR names. A pin that is not a
/// bundle name, or a bundle not installed there, is refused.
pub fn developer_dir(apps: &Path, pin: &str) -> Result<PathBuf, String> {
    let bundle_name = Path::new(pin).file_name().is_some_and(|name| name == pin)
        && pin.ends_with(".app")
        && !pin.starts_with('.');
    if !bundle_name {
        return Err(format!("{pin:?} is not an app bundle name"));
    }
    let developer = apps.join(pin).join("Contents").join("Developer");
    if !developer.is_dir() {
        return Err(format!(
            "the pinned Xcode {pin} is not installed in {}",
            apps.display()
        ));
    }
    Ok(developer)
}

/// The directories of one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub workspace: PathBuf,
    pub derived_data: PathBuf,
    pub devices: PathBuf,
    pub home: PathBuf,
}

impl Session {
    /// The session under `root`.
    pub fn new(root: &Path) -> Self {
        Session {
            workspace: root.join("workspace"),
            derived_data: root.join("DerivedData"),
            devices: root.join("devices"),
            home: root.join("home"),
        }
    }
}

/// How many files a clone made, and how many of them share the source's blocks.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Clones {
    pub files: u64,
    pub shared: u64,
}

/// Makes the session's DerivedData and simulator set: a clone of each warm copy where one exists, else an empty
/// directory. The home and the workspace are made empty.
pub fn prepare(session: &Session, warm_derived: &Path, warm_devices: &Path) -> io::Result<Clones> {
    fs::create_dir_all(&session.workspace)?;
    fs::create_dir_all(&session.home)?;
    let derived = clone_or_create(warm_derived, &session.derived_data)?;
    let devices = clone_or_create(warm_devices, &session.devices)?;
    Ok(Clones {
        files: derived.files + devices.files,
        shared: derived.shared + devices.shared,
    })
}

fn clone_or_create(warm: &Path, to: &Path) -> io::Result<Clones> {
    if warm.is_dir() {
        clone_tree(warm, to)
    } else {
        fs::create_dir_all(to)?;
        Ok(Clones::default())
    }
}

/// Copies the tree at `from` to `to`, each file by [`clone_file`]: where the filesystem shares blocks (APFS) a copy
/// takes no space until either side is written. Directories are made and symbolic links copied as links.
pub fn clone_tree(from: &Path, to: &Path) -> io::Result<Clones> {
    let mut clones = Clones::default();
    copy_dir(from, to, &mut clones)?;
    Ok(clones)
}

fn copy_dir(from: &Path, to: &Path, clones: &mut Clones) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_dir(&source, &target, clones)?;
        } else if kind.is_symlink() {
            link(&fs::read_link(&source)?, &target)?;
        } else {
            clones.files += 1;
            if clone_file(&source, &target)? == CopyMode::Reflink {
                clones.shared += 1;
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn link(target: &Path, at: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, at)
}

#[cfg(not(unix))]
fn link(_target: &Path, _at: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "symbolic links are cloned on Unix only",
    ))
}

/// The sandbox for a command of a session (see the module docs). The working directory is the workspace; the
/// writable paths are the workspace, DerivedData, the devices and the home; the Xcode bundle of `developer_dir` is
/// readable; the network is the proxy on `proxy_port`. The environment is the session's own: DEVELOPER_DIR pins the
/// toolchain, HOME and TMPDIR are the session's home, and PATH starts in the pinned Xcode.
pub fn session_spec(
    session: &Session,
    developer_dir: &Path,
    proxy_port: u16,
    program: &str,
    args: &[String],
) -> Spec {
    let bundle = developer_dir
        .parent()
        .and_then(Path::parent)
        .unwrap_or(developer_dir);
    let home = session.home.display().to_string();
    let path = format!(
        "{}/usr/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        developer_dir.display()
    );
    let env = [
        ("PATH", path),
        ("DEVELOPER_DIR", developer_dir.display().to_string()),
        ("HOME", home.clone()),
        ("CFFIXED_USER_HOME", home.clone()),
        ("TMPDIR", home),
    ];
    Spec {
        program: PathBuf::from(program),
        args: args.to_vec(),
        env: env
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
        workdir: session.workspace.clone(),
        read_only: vec![bundle.to_path_buf()],
        read_write: vec![
            session.derived_data.clone(),
            session.devices.clone(),
            session.home.clone(),
        ],
        network: Network::LoopbackPort(proxy_port),
        limits: Limits::default(),
    }
}

/// Creates a keychain file at `path` with `password`, through `security -i`: the commands are read from stdin, so
/// the password is never on a command line. `create-keychain` given a path neither unlocks the keychain nor changes
/// the search list. The password must be letters and digits, and the path may not hold quotes, backslashes or
/// control characters, so the quoted command line reads back as given.
pub fn create_keychain(path: &Path, password: &str) -> Result<(), String> {
    let text = keychain_path(path)?;
    if password.is_empty() || !password.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("a keychain password is letters and digits".to_owned());
    }
    security_script(&format!("create-keychain -p \"{password}\" \"{text}\"\n"))?;
    if path.is_file() {
        Ok(())
    } else {
        Err(format!("security did not create {text}"))
    }
}

/// Deletes the keychain file at `path` (`security delete-keychain`, through `security -i`; see [`create_keychain`]).
pub fn delete_keychain(path: &Path) -> Result<(), String> {
    let text = keychain_path(path)?;
    security_script(&format!("delete-keychain \"{text}\"\n"))?;
    if path.exists() {
        Err(format!("security did not delete {text}"))
    } else {
        Ok(())
    }
}

/// Whether a path names a place on the Mac by its leading `/`, on any host (the tests check it on Windows too).
fn mac_absolute(path: &Path) -> bool {
    path.to_string_lossy().starts_with('/')
}

/// The path of a keychain as the quoted command line takes it: absolute, and free of the characters quoting cannot
/// carry.
fn keychain_path(path: &Path) -> Result<String, String> {
    let text = path.display().to_string();
    if !mac_absolute(path)
        || text
            .chars()
            .any(|c| c == '"' || c == '\\' || c.is_control())
    {
        return Err(format!("keychain path cannot be used: {text}"));
    }
    Ok(text)
}

/// Runs `security -i` with `script` on its stdin. Its exit status is 0 whatever the commands did, so a command that
/// failed is the one that printed: a silent run is the only success.
fn security_script(script: &str) -> Result<(), String> {
    let mut child = Command::new(SECURITY)
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("security: {err}"))?;
    let mut stdin = child.stdin.take().ok_or("security has no stdin")?;
    stdin
        .write_all(script.as_bytes())
        .map_err(|err| format!("security: {err}"))?;
    drop(stdin);
    let out = child
        .wait_with_output()
        .map_err(|err| format!("security: {err}"))?;
    if out.stdout.is_empty() && out.stderr.is_empty() {
        return Ok(());
    }
    let mut said = String::from_utf8_lossy(&out.stdout).into_owned();
    said.push_str(&String::from_utf8_lossy(&out.stderr));
    Err(format!("security refused: {}", said.trim()))
}

/// A project's egress proxy on this node: the vsockd proxy served on loopback. Its port is the one the project's
/// session profiles allow (see [`session_spec`]). Each connection is decided by apid for the project's party
/// (`apz_egress_rules::project_party`), checked for private destinations and audited, exactly as a guest's is.
/// Dropping it stops the listener.
pub struct ProjectProxy {
    port: u16,
    stop: Arc<AtomicBool>,
    serving: Option<JoinHandle<()>>,
}

impl ProjectProxy {
    /// Starts the proxy for `project` on an ephemeral loopback port.
    pub fn start(project: &str, apid: Apid, policy: Policy) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let (cid, flag) = (project_party(project), Arc::clone(&stop));
        let serving = thread::spawn(move || {
            serve_tcp(listener, cid, Arc::new(policy), apid, flag);
        });
        Ok(ProjectProxy {
            port,
            stop,
            serving: Some(serving),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for ProjectProxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(serving) = self.serving.take() {
            let _ = serving.join();
        }
    }
}

/// The sessions a Mac node runs for its projects. A project's sessions share its proxy: the first session starts it,
/// and the last one to end stops it. The sessions run as [`Runs`] says.
pub struct Sessions<S: Spawner> {
    spawner: S,
    apid: Apid,
    /// Each project with sessions here: its proxy and how many sessions it has.
    proxies: BTreeMap<String, (ProjectProxy, usize)>,
    /// Each session that is running: its project and pid.
    running: BTreeMap<u64, (String, u32)>,
    next: u64,
}

impl<S: Spawner> Sessions<S> {
    pub fn new(spawner: S, apid: Apid) -> Self {
        Sessions {
            spawner,
            apid,
            proxies: BTreeMap::new(),
            running: BTreeMap::new(),
            next: 1,
        }
    }

    /// Starts a session of `project`. `build` makes its sandbox from the port of the project's proxy, which starts
    /// with the project's first session. Returns the session's id.
    pub fn start(
        &mut self,
        project: &str,
        runs: &Runs,
        build: impl FnOnce(u16) -> Spec,
    ) -> Result<u64, String> {
        let port = match self.proxies.get_mut(project) {
            Some((proxy, sessions)) => {
                *sessions += 1;
                proxy.port()
            }
            None => {
                let proxy = ProjectProxy::start(project, self.apid.clone(), Policy::new(vec![]))
                    .map_err(|err| format!("the proxy of {project}: {err}"))?;
                let port = proxy.port();
                self.proxies.insert(project.to_owned(), (proxy, 1));
                port
            }
        };
        match start_session(&mut self.spawner, runs, &build(port)) {
            Ok(pid) => {
                let id = self.next;
                self.next += 1;
                self.running.insert(id, (project.to_owned(), pid));
                Ok(id)
            }
            Err(err) => {
                self.release(project);
                Err(err)
            }
        }
    }

    /// Ends a session. When it was the project's last, the project's proxy stops.
    pub fn end(&mut self, session: u64) -> Result<(), String> {
        let (project, _) = self
            .running
            .remove(&session)
            .ok_or_else(|| format!("no session {session} is running"))?;
        self.release(&project);
        Ok(())
    }

    /// The port of `project`'s proxy, while it has sessions.
    pub fn proxy_port(&self, project: &str) -> Option<u16> {
        self.proxies.get(project).map(|(proxy, _)| proxy.port())
    }

    /// Sessions running now.
    pub fn running(&self) -> usize {
        self.running.len()
    }

    /// One session of `project` is over: its proxy stops when none is left.
    fn release(&mut self, project: &str) {
        let Some((_, sessions)) = self.proxies.get_mut(project) else {
            return;
        };
        *sessions -= 1;
        if *sessions == 0 {
            self.proxies.remove(project);
        }
    }
}

/// Where a session runs: as the node's own user (a Mac whose setup gave it no rights to make project accounts), or as
/// its project's account, through the setuid helper the node's setup installs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Runs {
    Current,
    Project { user: ProjectUser, helper: PathBuf },
}

/// The command line that starts `spec`'s sandboxed command under `runs`. As the project account the line is the
/// helper, given the account's uid and the staff gid, then `--`, then the sandboxed command; the environment is the
/// spec's, which the helper keeps when it changes user.
pub fn session_line(runs: &Runs, spec: &Spec) -> Result<Invocation, String> {
    let sandboxed = apz_sandbox::command(spec).map_err(|err| err.to_string())?;
    let line = Invocation {
        program: sandboxed.get_program().to_string_lossy().into_owned(),
        args: sandboxed
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect(),
        env: spec.env.clone(),
    };
    match runs {
        Runs::Current => Ok(line),
        Runs::Project { user, helper } => {
            if !mac_absolute(helper) {
                return Err(format!("the helper must be absolute: {}", helper.display()));
            }
            let mut args = vec![
                "--uid".to_owned(),
                user.uid.to_string(),
                "--gid".to_owned(),
                STAFF_GID.to_string(),
                "--".to_owned(),
                line.program,
            ];
            args.extend(line.args);
            Ok(Invocation {
                program: helper.display().to_string(),
                args,
                env: spec.env.clone(),
            })
        }
    }
}

/// Starts a session's sandboxed command under `runs` and returns its pid.
pub fn start_session(spawner: &mut dyn Spawner, runs: &Runs, spec: &Spec) -> Result<u32, String> {
    let line = session_line(runs, spec)?;
    spawner
        .spawn(&line)
        .map_err(|err| format!("starting {}: {err}", line.program))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    /// The Seatbelt wrapper a session runs under (apz-sandbox's macOS backend).
    #[cfg(target_os = "macos")]
    const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apz-mac-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch directory");
        dir
    }

    /// A session of `root` with a warm DerivedData and an Xcode bundle of its own under `root`.
    #[cfg(target_os = "macos")]
    fn session_fixture(root: &Path) -> (Session, PathBuf) {
        let bundle = root.join("Xcode_test.app");
        let developer = bundle.join("Contents/Developer");
        fs::create_dir_all(developer.join("usr/bin")).unwrap();
        (Session::new(&root.join("s")), developer)
    }

    fn user(uid: u32) -> ProjectUser {
        ProjectUser::new("plaza", uid, Path::new("/Users/apz_plaza")).unwrap()
    }

    /// A runner that knows one account (its `dscl -read` answer) and records every line it is given.
    struct Fake {
        account: Option<String>,
        seen: Vec<Invocation>,
    }

    impl Runner for Fake {
        fn run(&mut self, line: &Invocation) -> io::Result<Ran> {
            self.seen.push(line.clone());
            if line.args.get(1).map(String::as_str) == Some("-read") {
                return Ok(match &self.account {
                    Some(answer) => Ran {
                        ok: true,
                        stdout: answer.clone(),
                    },
                    None => Ran {
                        ok: false,
                        stdout: String::new(),
                    },
                });
            }
            Ok(Ran {
                ok: true,
                stdout: String::new(),
            })
        }
    }

    #[test]
    fn caps_follow_cores_and_memory() {
        assert_eq!(
            Caps::for_host(10, 16384),
            Caps {
                builds: 2,
                simulators: 6
            }
        );
        assert_eq!(
            Caps::for_host(4, 8192),
            Caps {
                builds: 1,
                simulators: 2
            }
        );
        assert_eq!(
            Caps::for_host(2, 2048),
            Caps {
                builds: 1,
                simulators: 0
            }
        );
    }

    #[test]
    fn the_facts_carry_the_caps_on_a_mac_only() {
        let load = Load {
            builds: 1,
            simulators: 0,
        };
        let facts = facts_for(Some(16384), 10, load);
        if cfg!(target_os = "macos") {
            assert_eq!(
                facts,
                Some(MacFacts {
                    builds: 2,
                    simulators: 6,
                    builds_running: 1,
                    simulators_running: 0,
                })
            );
        } else {
            assert_eq!(facts, None);
        }
        assert_eq!(facts_for(None, 10, load), None, "the caps need the memory");
    }

    #[test]
    fn admission_refuses_past_the_cap() {
        let caps = Caps {
            builds: 2,
            simulators: 6,
        };
        let one = Load {
            builds: 1,
            simulators: 0,
        };
        assert!(caps.admit(one, Work::Build).is_ok());
        let full = Load {
            builds: 2,
            simulators: 0,
        };
        let refused = caps.admit(full, Work::Build).unwrap_err();
        assert!(refused.contains("2 builds"), "{refused}");
        let sims = Load {
            builds: 0,
            simulators: 6,
        };
        assert!(caps.admit(sims, Work::Build).is_ok());
        let refused = caps.admit(sims, Work::Simulator).unwrap_err();
        assert!(refused.contains("6 simulators"), "{refused}");
    }

    #[test]
    fn project_accounts_take_clean_names_and_checked_uids() {
        let home = Path::new("/Users/apz_plaza");
        let plain = ProjectUser::new("plaza", 601, home).unwrap();
        assert_eq!(plain.name, "apz_plaza");
        assert_eq!(plain.uid, 601);
        assert_eq!(
            ProjectUser::new("my-app", 601, home).unwrap().name,
            "apz_my_app"
        );
        assert!(ProjectUser::new("Plaza", 601, home).is_err());
        assert!(ProjectUser::new("", 601, home).is_err());
        assert!(ProjectUser::new("a/b", 601, home).is_err());
        assert!(ProjectUser::new(&"p".repeat(30), 601, home).is_err());
        assert!(ProjectUser::new("plaza", 501, home).is_err());
        assert!(ProjectUser::new("plaza", 601, Path::new("relative")).is_err());
    }

    #[test]
    fn free_uid_takes_the_lowest_unused_project_uid() {
        assert_eq!(free_uid(&[]), Some(600));
        assert_eq!(free_uid(&[600, 601, 603]), Some(602));
        let all: Vec<u32> = USER_UIDS.collect();
        assert_eq!(free_uid(&all), None);
    }

    #[test]
    fn account_commands_name_the_record_and_its_attributes() {
        let lines = create_user_commands(&user(601));
        let argv: Vec<Vec<&str>> = lines
            .iter()
            .map(|line| line.args.iter().map(String::as_str).collect())
            .collect();
        assert_eq!(lines[0].program, "/usr/bin/dscl");
        assert_eq!(argv[0], [".", "-create", "/Users/apz_plaza"]);
        assert_eq!(
            argv[1],
            [".", "-create", "/Users/apz_plaza", "UniqueID", "601"]
        );
        assert!(argv.contains(&vec![
            ".",
            "-create",
            "/Users/apz_plaza",
            "PrimaryGroupID",
            "20"
        ]));
        assert!(argv.contains(&vec![
            ".",
            "-create",
            "/Users/apz_plaza",
            "NFSHomeDirectory",
            "/Users/apz_plaza"
        ]));
        assert!(argv.contains(&vec![".", "-create", "/Users/apz_plaza", "Password", "*"]));
    }

    #[test]
    fn ensure_user_creates_a_missing_account() {
        let mut fake = Fake {
            account: None,
            seen: Vec::new(),
        };
        ensure_user(&mut fake, &user(601)).unwrap();
        let creates = fake
            .seen
            .iter()
            .filter(|line| line.args[1] == "-create")
            .count();
        assert_eq!(creates, create_user_commands(&user(601)).len());
    }

    #[test]
    fn ensure_user_keeps_a_matching_account_and_refuses_another_uid() {
        let mut same = Fake {
            account: Some("UniqueID: 601\n".to_owned()),
            seen: Vec::new(),
        };
        ensure_user(&mut same, &user(601)).unwrap();
        assert_eq!(same.seen.len(), 1, "only the read: nothing is created");

        let mut other = Fake {
            account: Some("UniqueID: 602\n".to_owned()),
            seen: Vec::new(),
        };
        let err = ensure_user(&mut other, &user(601)).unwrap_err();
        assert!(err.contains("another uid"), "{err}");
        assert_eq!(other.seen.len(), 1);
    }

    #[test]
    fn pf_rules_pass_the_proxy_for_the_account_and_block_the_rest() {
        let rules = pf_rules(&user(601), 8443).unwrap();
        assert_eq!(
            rules,
            "# project account apz_plaza (uid 601): its egress reaches the local proxy and nothing else\n\
             pass out quick on lo0 inet proto tcp from any to 127.0.0.1 port 8443 user 601\n\
             pass out quick on lo0 inet6 proto tcp from any to ::1 port 8443 user 601\n\
             block drop out quick user 601\n"
        );
        let pass = rules.find("pass out").unwrap();
        let block = rules.find("block drop").unwrap();
        assert!(pass < block, "the pass rules must match before the block");
        assert!(pf_rules(&user(601), 0).is_err());
    }

    #[test]
    fn developer_dir_resolves_the_pinned_bundle_only() {
        let apps = scratch("apps");
        fs::create_dir_all(apps.join("Xcode_16.4.app/Contents/Developer")).unwrap();
        assert_eq!(
            developer_dir(&apps, "Xcode_16.4.app").unwrap(),
            apps.join("Xcode_16.4.app/Contents/Developer")
        );
        assert!(developer_dir(&apps, "Xcode_9.app").is_err());
        assert!(developer_dir(&apps, "../Xcode_16.4.app").is_err());
        assert!(developer_dir(&apps, "Xcode_16.4").is_err());
        assert!(developer_dir(&apps, ".app").is_err());
        let _ = fs::remove_dir_all(&apps);
    }

    #[test]
    fn session_spec_grants_the_session_and_nothing_else() {
        let root = Path::new("/var/apz/sessions/s1");
        let session = Session::new(root);
        let developer = Path::new("/Applications/Xcode_16.4.app/Contents/Developer");
        let spec = session_spec(
            &session,
            developer,
            8443,
            "xcodebuild",
            &["-version".to_owned()],
        );
        assert_eq!(spec.workdir, root.join("workspace"));
        assert_eq!(
            spec.read_only,
            [PathBuf::from("/Applications/Xcode_16.4.app")]
        );
        assert_eq!(
            spec.read_write,
            [
                root.join("DerivedData"),
                root.join("devices"),
                root.join("home")
            ]
        );
        assert_eq!(spec.network, Network::LoopbackPort(8443));
        let env = |key: &str| {
            spec.env
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(env("DEVELOPER_DIR"), developer.to_str());
        assert_eq!(env("HOME"), root.join("home").to_str());
        assert!(
            env("PATH")
                .unwrap()
                .starts_with(&format!("{}/usr/bin", developer.display()))
        );
    }

    #[cfg(unix)]
    #[test]
    fn clone_tree_copies_every_file_and_keeps_the_source() {
        let base = scratch("clone");
        let warm = base.join("warm");
        fs::create_dir_all(warm.join("Build/Products")).unwrap();
        fs::write(warm.join("Build/Products/app.o"), b"object").unwrap();
        fs::write(warm.join("info.plist"), b"plist").unwrap();
        std::os::unix::fs::symlink("info.plist", warm.join("link")).unwrap();

        let copy = base.join("copy");
        let clones = clone_tree(&warm, &copy).unwrap();
        assert_eq!(clones.files, 2);
        assert_eq!(
            fs::read(copy.join("Build/Products/app.o")).unwrap(),
            b"object"
        );
        assert_eq!(
            fs::read_link(copy.join("link")).unwrap(),
            Path::new("info.plist")
        );
        if vmm::clonefile::reflink_supported(&base) {
            assert_eq!(clones.shared, clones.files, "APFS shares the blocks");
        }

        fs::write(copy.join("info.plist"), b"changed").unwrap();
        assert_eq!(fs::read(warm.join("info.plist")).unwrap(), b"plist");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn prepare_clones_the_warm_copies_or_starts_empty() {
        let base = scratch("prepare");
        let warm_derived = base.join("warm-derived");
        fs::create_dir_all(&warm_derived).unwrap();
        fs::write(warm_derived.join("cache.bin"), b"warm").unwrap();
        let session = Session::new(&base.join("s1"));
        let clones = prepare(&session, &warm_derived, &base.join("no-warm-devices")).unwrap();
        assert_eq!(clones.files, 1);
        assert_eq!(
            fs::read(session.derived_data.join("cache.bin")).unwrap(),
            b"warm"
        );
        assert!(session.devices.is_dir() && session.home.is_dir() && session.workspace.is_dir());
        let _ = fs::remove_dir_all(&base);
    }

    /// A fake apid on loopback. It answers the egress check for `host` with allow, unless `refused` names it, and
    /// every other request with `{}`. Returns its port and the bodies of the checks it answered.
    fn fake_hub(refused: &'static str) -> (u16, Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let checks = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&checks);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let seen = Arc::clone(&seen);
                thread::spawn(move || answer_hub(stream, refused, &seen));
            }
        });
        (port, checks)
    }

    fn answer_hub(
        mut stream: std::net::TcpStream,
        refused: &str,
        seen: &std::sync::Mutex<Vec<String>>,
    ) {
        let Some((line, body)) = read_request(&mut stream) else {
            return;
        };
        let reply = if line.starts_with("POST /internal/egress/check") {
            seen.lock().unwrap().push(body.clone());
            let check: serde_json::Value = serde_json::from_str(&body).unwrap();
            let host = check["host"].as_str().unwrap_or_default();
            let allow = host != refused;
            json!({"allow": allow, "reason": "not on the rules", "intercept": false}).to_string()
        } else {
            "{}".to_owned()
        };
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            reply.len()
        );
        let _ = stream.write_all(format!("{head}{reply}").as_bytes());
    }

    /// The request line and the body of one HTTP request.
    fn read_request(stream: &mut std::net::TcpStream) -> Option<(String, String)> {
        use std::io::Read;
        let mut data = Vec::new();
        let mut buf = [0u8; 1024];
        let head_end = loop {
            if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                break at + 4;
            }
            let n = stream.read(&mut buf).ok()?;
            if n == 0 {
                return None;
            }
            data.extend_from_slice(&buf[..n]);
        };
        let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
        let length = head
            .lines()
            .find_map(|line| {
                let lower = line.to_ascii_lowercase();
                lower
                    .strip_prefix("content-length:")
                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
            })
            .unwrap_or(0);
        while data.len() < head_end + length {
            let n = stream.read(&mut buf).ok()?;
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
        }
        let body = String::from_utf8_lossy(&data[head_end..]).into_owned();
        Some((head.lines().next().unwrap_or_default().to_owned(), body))
    }

    /// The stand-in internet: a local server that answers every request with a fixed body.
    fn stand_in_internet() -> u16 {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                thread::spawn(move || {
                    let _ = read_request(&mut stream);
                    let body = "hello from the internet";
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(format!("{head}{body}").as_bytes());
                });
            }
        });
        port
    }

    /// The proxy's answer to one request sent to it directly, read until the proxy closes.
    fn ask_proxy(port: u16, request: &str) -> String {
        use std::io::Read;
        let mut stream = std::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut answer = Vec::new();
        let _ = stream.read_to_end(&mut answer);
        String::from_utf8_lossy(&answer).into_owned()
    }

    fn hub_url_token(port: u16, name: &str) -> Apid {
        let token =
            std::env::temp_dir().join(format!("apz-mac-token-{name}-{}", std::process::id()));
        fs::write(&token, "test-token\n").unwrap();
        Apid::new(&format!("http://127.0.0.1:{port}"), Some(token)).unwrap()
    }

    #[test]
    fn a_keychain_password_and_path_must_be_quotable() {
        let path = std::env::temp_dir().join("apz-mac-never-created.keychain-db");
        assert!(create_keychain(&path, "two words").is_err());
        assert!(create_keychain(&path, "").is_err());
        assert!(create_keychain(Path::new("relative.keychain-db"), "pw1").is_err());
        assert!(keychain_path(Path::new("/tmp/a\"b.keychain-db")).is_err());
        assert!(!path.exists(), "a refused keychain is never created");
    }

    #[test]
    fn the_proxy_allows_a_listed_host_and_refuses_a_private_or_unlisted_one() {
        let internet = stand_in_internet();
        let (hub, checks) = hub_url_token_port("evil.test");
        let apid = hub_url_token(hub, "plain");
        let policy = Policy::new(vec![internet]).allow_loopback();
        let proxy = ProjectProxy::start("plaza", apid, policy).unwrap();

        let allowed = ask_proxy(
            proxy.port(),
            &format!("GET http://localhost:{internet}/ HTTP/1.1\r\nHost: localhost\r\n\r\n"),
        );
        assert!(allowed.starts_with("HTTP/1.1 200 OK"), "{allowed}");
        assert!(allowed.ends_with("hello from the internet"), "{allowed}");

        let private = ask_proxy(
            proxy.port(),
            &format!("GET http://10.0.0.1:{internet}/ HTTP/1.1\r\nHost: 10.0.0.1\r\n\r\n"),
        );
        assert!(private.starts_with("HTTP/1.1 403 Forbidden"), "{private}");

        let unlisted = ask_proxy(
            proxy.port(),
            &format!("GET http://evil.test:{internet}/ HTTP/1.1\r\nHost: evil.test\r\n\r\n"),
        );
        assert!(unlisted.starts_with("HTTP/1.1 403 Forbidden"), "{unlisted}");
        assert!(unlisted.contains("not on the rules"), "{unlisted}");

        let checks = checks.lock().unwrap();
        assert!(!checks.is_empty(), "apid decided the connections");
        assert!(
            checks
                .iter()
                .all(|body| body.contains(&format!("\"cid\":{}", project_party("plaza")))),
            "every check is for the project's party: {checks:?}"
        );
    }

    fn hub_url_token_port(refused: &'static str) -> (u16, Arc<std::sync::Mutex<Vec<String>>>) {
        fake_hub(refused)
    }

    /// A spawner that records each line it starts and hands out pids, or refuses every start.
    #[cfg(target_os = "macos")]
    struct FakeSpawner {
        pid: u32,
        lines: Vec<Invocation>,
        refuse: bool,
    }

    #[cfg(target_os = "macos")]
    impl Spawner for FakeSpawner {
        fn spawn(&mut self, line: &Invocation) -> io::Result<u32> {
            if self.refuse {
                return Err(io::Error::other("refused by the fake"));
            }
            self.lines.push(line.clone());
            self.pid += 1;
            Ok(self.pid)
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_projects_proxy_starts_with_its_first_session_and_stops_with_its_last() {
        let base = scratch("sessions");
        let (_, developer) = session_fixture(&base);
        let root = |name: &str| {
            let session = Session::new(&base.join(name));
            prepare(&session, &base.join("no-warm"), &base.join("no-warm")).unwrap();
            session
        };
        let mut sessions = Sessions::new(
            FakeSpawner {
                pid: 100,
                lines: Vec::new(),
                refuse: false,
            },
            hub_url_token(fake_hub("never").0, "sessions"),
        );
        let fresh = |session: &Session, port: u16| {
            session_spec(
                session,
                &developer,
                port,
                "/bin/sh",
                &["-c".to_owned(), "true".to_owned()],
            )
        };
        let reachable =
            |port: u16| std::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_ok();

        let (one, two) = (root("one"), root("two"));
        let first = sessions
            .start("pr1", &Runs::Current, |port| fresh(&one, port))
            .unwrap();
        let second = sessions
            .start("pr1", &Runs::Current, |port| fresh(&two, port))
            .unwrap();
        let port = sessions.proxy_port("pr1").expect("pr1 has a proxy");
        assert!(reachable(port));
        assert_eq!(sessions.running(), 2);
        for line in &sessions.spawner.lines {
            assert!(
                line.args
                    .iter()
                    .any(|arg| arg.contains(&format!("localhost:{port}"))),
                "each session's sandbox allows the project's proxy: {line:?}"
            );
        }

        let other = sessions
            .start("pr2", &Runs::Current, |port| fresh(&root("three"), port))
            .unwrap();
        let other_port = sessions.proxy_port("pr2").expect("pr2 has a proxy");
        assert_ne!(other_port, port, "each project has its own proxy");

        sessions.end(first).unwrap();
        assert!(
            reachable(port),
            "the project's proxy runs while a session is left"
        );
        sessions.end(second).unwrap();
        assert_eq!(sessions.proxy_port("pr1"), None);
        assert!(!reachable(port), "the last session ending stops the proxy");
        assert!(
            reachable(other_port),
            "another project's proxy is not stopped"
        );

        sessions.end(other).unwrap();
        assert!(sessions.end(other).is_err(), "a session ends once");
        assert_eq!(sessions.running(), 0);
        sessions.spawner.refuse = true;
        assert!(
            sessions
                .start("pr3", &Runs::Current, |port| fresh(&root("four"), port))
                .is_err()
        );
        assert_eq!(
            sessions.proxy_port("pr3"),
            None,
            "a refused start holds no proxy"
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_project_session_is_started_as_its_account_through_the_helper() {
        let base = scratch("launch-helper");
        let (session, developer) = session_fixture(&base);
        prepare(&session, &base.join("no-warm"), &base.join("no-warm")).unwrap();
        let spec = session_spec(
            &session,
            &developer,
            8443,
            "/bin/sh",
            &["-c".to_owned(), "true".to_owned()],
        );
        let runs = Runs::Project {
            user: user(601),
            helper: PathBuf::from("/usr/local/libexec/apz-as-user"),
        };
        let line = session_line(&runs, &spec).unwrap();
        assert_eq!(line.program, "/usr/local/libexec/apz-as-user");
        assert_eq!(
            &line.args[..6],
            ["--uid", "601", "--gid", "20", "--", SANDBOX_EXEC],
            "{:?}",
            line.args
        );
        assert_eq!(
            line.env, spec.env,
            "the helper keeps the session's environment"
        );
        assert!(
            line.args.contains(&"/bin/sh".to_owned()),
            "the sandboxed command follows the helper"
        );
        let relative = Runs::Project {
            user: user(601),
            helper: PathBuf::from("apz-as-user"),
        };
        assert!(session_line(&relative, &spec).is_err());
        let _ = fs::remove_dir_all(&base);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_current_user_runs_a_session_here_for_real() {
        let base = scratch("launch-current");
        let (session, developer) = session_fixture(&base);
        prepare(&session, &base.join("no-warm"), &base.join("no-warm")).unwrap();
        let marker = session.workspace.join("ran.txt");
        let script = format!("echo ran > '{}'", marker.display());
        let spec = session_spec(
            &session,
            &developer,
            8443,
            "/bin/sh",
            &["-c".to_owned(), script],
        );
        let pid = start_session(&mut System, &Runs::Current, &spec).unwrap();
        assert!(pid > 0);
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while !marker.exists() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(fs::read_to_string(&marker).unwrap().trim(), "ran");
        let _ = fs::remove_dir_all(&base);
    }

    #[cfg(target_os = "macos")]
    mod macos {
        use super::*;

        /// The account list the owner's search uses, read without changing it.
        fn search_list() -> String {
            let out = Command::new(SECURITY)
                .args(["list-keychains", "-d", "user"])
                .output()
                .expect("security list-keychains");
            String::from_utf8_lossy(&out.stdout).into_owned()
        }

        #[test]
        fn keychain_is_created_in_a_scratch_directory_and_leaves_the_search_list() {
            // A directory with a space in its name: the quoted path must still reach security whole.
            let dir = scratch("key chain");
            let path = dir.join("project.keychain-db");
            let before = search_list();
            let password = apz_os::random::hex(32).unwrap();
            create_keychain(&path, &password).unwrap();
            assert!(path.is_file());

            // The password is the one the keychain takes: locked, it opens with that password and not another.
            let text = path.display();
            security_script(&format!("lock-keychain \"{text}\"\n")).unwrap();
            assert!(
                security_script(&format!("unlock-keychain -p \"wrong\" \"{text}\"\n")).is_err()
            );
            security_script(&format!("unlock-keychain -p \"{password}\" \"{text}\"\n")).unwrap();

            let info = Command::new(SECURITY)
                .args(["show-keychain-info"])
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                info.status.success(),
                "{}",
                String::from_utf8_lossy(&info.stderr)
            );
            assert_eq!(search_list(), before, "the search list is unchanged");

            delete_keychain(&path).unwrap();
            assert!(!path.exists());
            assert_eq!(search_list(), before);
            let _ = fs::remove_dir_all(&dir);
        }

        fn run_in(spec: &Spec, script: &str) -> std::process::Output {
            let mut spec = spec.clone();
            spec.args = vec!["-c".to_owned(), script.to_owned()];
            apz_sandbox::command(&spec)
                .expect("command builds")
                .output()
                .expect("sandbox-exec starts")
        }

        /// The sandboxed command's output, with the spec's arguments.
        fn run_with(spec: &Spec, args: &[&str]) -> std::process::Output {
            let mut spec = spec.clone();
            spec.args = args.iter().map(|arg| (*arg).to_owned()).collect();
            apz_sandbox::command(&spec)
                .expect("command builds")
                .output()
                .expect("sandbox-exec starts")
        }

        #[test]
        fn a_session_reaches_the_internet_only_through_its_project_proxy() {
            let base = scratch("proxy-session");
            let (session, developer) = session_fixture(&base);
            prepare(&session, &base.join("no-warm"), &base.join("no-warm")).unwrap();
            let internet = stand_in_internet();
            let (hub, _checks) = fake_hub("evil.test");
            let proxy = ProjectProxy::start(
                "plaza",
                hub_url_token(hub, "session"),
                Policy::new(vec![internet]).allow_loopback(),
            )
            .unwrap();
            let via = format!("http://127.0.0.1:{}", proxy.port());
            let spec = session_spec(&session, &developer, proxy.port(), "/usr/bin/curl", &[]);
            let fetch = |target: &str, through: bool| {
                let mut args = vec![
                    "-sS",
                    "-q",
                    "--max-time",
                    "10",
                    "-o",
                    "-",
                    "-w",
                    "\n%{http_code}",
                ];
                if through {
                    args.extend(["-x", via.as_str()]);
                }
                args.push(target);
                run_with(&spec, &args)
            };
            let body =
                |out: &std::process::Output| String::from_utf8_lossy(&out.stdout).into_owned();

            let allowed = fetch(&format!("http://localhost:{internet}/"), true);
            assert!(
                allowed.status.success(),
                "{}",
                String::from_utf8_lossy(&allowed.stderr)
            );
            assert_eq!(body(&allowed), "hello from the internet\n200");

            let private = fetch(&format!("http://10.0.0.1:{internet}/"), true);
            assert!(body(&private).ends_with("403"), "{}", body(&private));

            let unlisted = fetch(&format!("http://evil.test:{internet}/"), true);
            assert!(body(&unlisted).ends_with("403"), "{}", body(&unlisted));

            let direct = fetch(&format!("http://127.0.0.1:{internet}/"), false);
            assert!(
                !direct.status.success(),
                "the session reaches only its proxy"
            );
            let _ = fs::remove_dir_all(&base);
        }

        #[test]
        fn a_session_writes_its_own_directories_and_is_refused_elsewhere() {
            let base = scratch("session");
            let (session, developer) = session_fixture(&base);
            let other = Session::new(&base.join("other"));
            prepare(&session, &base.join("no-warm"), &base.join("no-warm")).unwrap();
            prepare(&other, &base.join("no-warm"), &base.join("no-warm")).unwrap();
            fs::write(other.workspace.join("secret.txt"), b"OTHER-SECRET").unwrap();

            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let proxy = listener.local_addr().unwrap().port();
            let spec = session_spec(&session, &developer, proxy, "/bin/sh", &[]);

            let writes = |dir: &Path| {
                format!(
                    "echo ok > '{}/probe.txt' && cat '{}/probe.txt'",
                    dir.display(),
                    dir.display()
                )
            };
            for own in [
                &session.workspace,
                &session.derived_data,
                &session.devices,
                &session.home,
            ] {
                let out = run_in(&spec, &writes(own));
                assert!(
                    out.status.success(),
                    "{}: {}",
                    own.display(),
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            for foreign in [&other.workspace, &other.derived_data, &other.devices] {
                let out = run_in(&spec, &writes(foreign));
                assert!(
                    !out.status.success(),
                    "a write into {} must fail",
                    foreign.display()
                );
                assert!(!foreign.join("probe.txt").exists());
            }

            let read = run_in(
                &spec,
                &format!("cat '{}'", other.workspace.join("secret.txt").display()),
            );
            assert!(!read.status.success());
            assert!(!String::from_utf8_lossy(&read.stdout).contains("OTHER-SECRET"));

            let env = run_in(&spec, "echo \"$DEVELOPER_DIR|$HOME\"");
            assert_eq!(
                String::from_utf8_lossy(&env.stdout).trim(),
                format!("{}|{}", developer.display(), session.home.display())
            );

            let to_proxy = run_in(&spec, &format!("/usr/bin/nc -z -w 2 127.0.0.1 {proxy}"));
            assert!(to_proxy.status.success(), "the session reaches its proxy");
            let other_port = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let q = other_port.local_addr().unwrap().port();
            let elsewhere = run_in(&spec, &format!("/usr/bin/nc -z -w 2 127.0.0.1 {q}"));
            assert!(!elsewhere.status.success(), "other ports are refused");
            drop((listener, other_port));
            let _ = fs::remove_dir_all(&base);
        }
    }
}
