//! The daemon's shared state, and the routing of each hub request to the thread that serves it.
//!
//! Three kinds of thread serve a station:
//! - the link (in [`crate::link`]), which reads the hub's frames and writes the answers;
//! - the desktop lane ([`crate::desk`]), one thread that owns the desktop and serves the screen operations in
//!   arrival order, because the [`Desktop`] contract allows one caller at a time;
//! - the worker pool ([`crate::work`]), which serves files, terminals, runs, settings, facts and scripts, so a
//!   slow operation never holds up the others.
//!
//! STOP ALL is handled on the link thread the moment its request arrives: it sets [`Shared::frozen`], which
//! every later request is refused under, and tells both lanes to stop.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;

use apz_desktop::Desktop;
use apz_ops::{Env, Root, Runs, Terminals};
use apz_station::{Facts, Op};

use crate::backend::Backend;
use crate::log::Log;
use crate::state;
use crate::uplink::Uplink;

/// The worker threads.
pub const WORKERS: usize = 6;

/// The facts the host reports for a given count of applications. The link calls it for each hello.
pub type Gather = Arc<dyn Fn(u32) -> Facts + Send + Sync>;

/// The refusal for every request while STOP ALL is on.
pub const FROZEN: &str = "STOP ALL is on: the station takes no requests until it is lifted";

/// One request, with the connection it came on.
#[derive(Debug)]
pub struct Job {
    pub generation: u64,
    pub id: u64,
    pub op: Op,
}

/// What the desktop lane is sent.
#[derive(Debug)]
pub enum DeskMsg {
    Job(Job),
    /// STOP ALL: end the pixel sessions and every stream.
    StopAll,
}

/// What the worker pool is sent.
#[derive(Debug)]
pub enum WorkMsg {
    Job(Job),
    /// STOP ALL: stop the runs and close the terminals.
    StopAll,
}

/// State every thread shares.
pub struct Shared {
    /// STOP ALL is on.
    pub frozen: AtomicBool,
    root: RwLock<Root>,
    pub env: Env,
    pub state_dir: PathBuf,
    pub runs: Runs,
    /// The runs this daemon started and has not yet reported finished, for STOP ALL.
    pub run_ids: Mutex<BTreeSet<String>>,
    pub terminals: Terminals,
    pub uplink: Uplink,
    pub log: Log,
    /// The applications the desktop reports (0 with no real backend).
    pub apps: AtomicU32,
    pub gather: Gather,
    pub backend: Backend,
    /// The mirror of the work folder the sync keeps (sync.rs).
    pub sync: crate::sync::Mirror,
}

impl Shared {
    /// The work folder: the files, terminals and runs are confined to it.
    pub fn root(&self) -> Root {
        self.root.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Makes `root` the work folder.
    pub fn set_root(&self, root: Root) {
        *self.root.write().unwrap_or_else(|e| e.into_inner()) = root;
    }
}

/// What a daemon needs to start.
pub struct DaemonConfig {
    pub state_dir: PathBuf,
    pub env: Env,
    pub gather: Gather,
    pub log: Log,
    pub backend: Backend,
}

/// A running station's threads and the channels that reach them.
pub struct Daemon {
    pub shared: Arc<Shared>,
    desk: Sender<DeskMsg>,
    work: Sender<WorkMsg>,
    _threads: Vec<JoinHandle<()>>,
}

impl Daemon {
    /// Starts the lanes on `desktop`. The work folder comes from the saved settings and is created if missing.
    pub fn start(desktop: Box<dyn Desktop>, config: DaemonConfig) -> Result<Daemon, String> {
        let settings = state::load_settings(&config.state_dir)?;
        let root = work_folder(&settings.work_folder, config.env.home())?;
        fs::create_dir_all(&root).map_err(|e| format!("cannot create the work folder: {e}"))?;
        let shared = Arc::new(Shared {
            frozen: AtomicBool::new(false),
            root: RwLock::new(Root::new(root)),
            env: config.env,
            state_dir: config.state_dir,
            runs: Runs::new(),
            run_ids: Mutex::new(BTreeSet::new()),
            terminals: Terminals::new(),
            uplink: Uplink::new(),
            log: config.log,
            apps: AtomicU32::new(0),
            gather: config.gather,
            backend: config.backend,
            sync: crate::sync::Mirror::default(),
        });

        let (desk, desk_rx) = channel();
        let (work, work_rx) = channel();
        let mut threads = vec![crate::desk::spawn(desktop, shared.clone(), desk_rx)];
        threads.extend(crate::work::spawn(work_rx, shared.clone(), WORKERS));
        threads.push(crate::sync::spawn(shared.clone()));
        Ok(Daemon {
            shared,
            desk,
            work,
            _threads: threads,
        })
    }

    /// Whether a request may start now. A refusal is the answer the hub gets.
    pub fn admit(&self, op: &Op, draining: bool) -> Result<(), String> {
        if matches!(op, Op::Freeze { .. }) {
            return Ok(());
        }
        if self.shared.frozen.load(Ordering::SeqCst) {
            return Err(FROZEN.to_string());
        }
        if draining {
            return Err("the station is draining: it takes no new requests".to_string());
        }
        Ok(())
    }

    /// Starts `op` on the lane that serves it. The answer is sent on the connection `generation`, when it is still open.
    pub fn submit(&self, generation: u64, id: u64, op: Op) {
        match op {
            Op::Freeze { on } => {
                self.shared.frozen.store(on, Ordering::SeqCst);
                if on {
                    let _ = self.desk.send(DeskMsg::StopAll);
                    let _ = self.work.send(WorkMsg::StopAll);
                }
                self.shared.log.warn(if on {
                    "STOP ALL: input stopped, pixel sessions and runs ended"
                } else {
                    "STOP ALL lifted"
                });
                self.shared.uplink.respond(generation, id, Ok(None));
            }
            op @ (Op::Facts
            | Op::Files(_)
            | Op::Term(_)
            | Op::Run(_)
            | Op::Script { .. }
            | Op::Settings(_)
            | Op::Sync(_)) => {
                let _ = self.work.send(WorkMsg::Job(Job { generation, id, op }));
            }
            op => {
                let _ = self.desk.send(DeskMsg::Job(Job { generation, id, op }));
            }
        }
    }

    /// The facts now, for a hello or a `facts` request.
    pub fn facts_now(&self) -> Facts {
        (self.shared.gather)(self.shared.apps.load(Ordering::SeqCst))
    }
}

/// The work folder a settings value names: `~/...` is under the home, a relative path is under the home too,
/// an absolute path is itself.
pub fn work_folder(raw: &str, home: &Path) -> Result<PathBuf, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("the work folder is empty".to_string());
    }
    let path = if raw == "~" {
        home.to_path_buf()
    } else if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        home.join(rest)
    } else if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        home.join(raw)
    };
    Ok(path)
}

/// The environment the daemon's processes get on this machine: the user's home, and the PATH it was started with.
/// On Windows, the variables a process needs to run at all come along too (see [`WINDOWS_VARS`]).
pub fn host_env() -> Env {
    let home = apz_os::dirs::home().unwrap_or_else(|| PathBuf::from("/"));
    let path = std::env::var("PATH")
        .ok()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".to_string());
    let mut env = Env::new(home, path);
    if cfg!(windows) {
        for name in WINDOWS_VARS {
            if let Some(value) = std::env::var(name).ok().filter(|v| !v.is_empty()) {
                env = env.with_var(name, value);
            }
        }
    }
    env
}

/// The Windows variables a run or a terminal needs from the daemon's own environment: the system directory, the
/// command interpreter, the Windows directory and the executable extensions. Without them most programs fail to
/// start, because a process's environment starts empty.
pub const WINDOWS_VARS: [&str; 4] = ["SYSTEMROOT", "COMSPEC", "WINDIR", "PATHEXT"];
