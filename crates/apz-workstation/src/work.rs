//! The worker pool: files, terminals, runs, scripts, settings and facts.
//!
//! The workers share one queue, so a slow job (a script waiting on its command, a facts query) holds one worker
//! and the rest keep serving. A job a worker takes while STOP ALL is on is refused and never runs. Files,
//! terminals and runs work inside the work folder through `apz_ops`, which refuses any path that leaves it.

use std::fs;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use apz_ops::files::{self, NoHooks};
use apz_ops::{OpError, Root};
use apz_station::{FilesOp, Op, RunOp, StationSettings, TermOp};
use serde::Serialize;
use serde_json::Value;

use crate::daemon::{FROZEN, Job, Shared, WorkMsg, may_run, work_folder};
use crate::{script, state};

/// Starts `count` workers that read from `rx`.
pub fn spawn(rx: Receiver<WorkMsg>, shared: Arc<Shared>, count: usize) -> Vec<JoinHandle<()>> {
    let rx = Arc::new(Mutex::new(rx));
    (0..count)
        .map(|_| {
            let rx = rx.clone();
            let shared = shared.clone();
            thread::spawn(move || worker(&rx, &shared))
        })
        .collect()
}

fn worker(rx: &Mutex<Receiver<WorkMsg>>, shared: &Shared) {
    loop {
        // The lock is held only while waiting for the next job, so the other workers queue behind it.
        let msg = match rx.lock() {
            Ok(guard) => guard.recv(),
            Err(_) => return,
        };
        match msg {
            Ok(WorkMsg::Job(job)) => serve(shared, job),
            Err(_) => return,
        }
    }
}

fn serve(shared: &Shared, job: Job) {
    let kind = kind_of(&job.op);
    let outcome = answer(shared, job.epoch, job.op);
    if let Err(err) = &outcome {
        shared.log.info(format!("{kind} refused: {err}"));
    }
    shared.uplink.respond(job.generation, job.id, outcome);
}

/// The answer to a job a worker has taken. While STOP ALL is on, or when it came on after the job was admitted
/// (`epoch`), the job is refused the way admission refuses a request, so it does not run.
fn answer(shared: &Shared, epoch: u64, op: Op) -> Result<Option<Value>, String> {
    if !may_run(shared, epoch) {
        return Err(FROZEN.to_string());
    }
    handle(shared, op)
}

fn handle(shared: &Shared, op: Op) -> Result<Option<Value>, String> {
    match op {
        Op::Files(op) => files(shared, op),
        Op::Term(op) => term(shared, op),
        Op::Run(op) => run(shared, op),
        Op::Script {
            bridge,
            app,
            code,
            timeout_s,
        } => script::run(shared, bridge, app.as_deref(), &code, timeout_s).map(Some),
        Op::Settings(settings) => apply_settings(shared, settings).map(|()| None),
        Op::Facts => to_value(&(shared.gather)(shared.apps.load(Ordering::SeqCst))).map(Some),
        Op::Sync(op) => crate::sync::serve(shared, op),
        _ => Err("not a worker operation".to_string()),
    }
}

fn files(shared: &Shared, op: FilesOp) -> Result<Option<Value>, String> {
    let root = shared.root();
    let reply = match op {
        FilesOp::List { path, cursor } => {
            to_value(&check(files::list(&root, &path, cursor.as_deref()))?)
        }
        FilesOp::Read { path, offset } => to_value(&check(files::read(&root, &path, offset))?),
        FilesOp::Write {
            path,
            text,
            expected_hash,
        } => to_value(&check(files::write(
            &root,
            &path,
            &text,
            expected_hash.as_deref(),
            &mut NoHooks,
        ))?),
        FilesOp::Search { q, glob, max } => to_value(&check(files::search(
            &root,
            &q,
            glob.as_deref(),
            max.map(|m| m as usize),
        ))?),
    }?;
    Ok(Some(reply))
}

fn term(shared: &Shared, op: TermOp) -> Result<Option<Value>, String> {
    let root = shared.root();
    let env = &shared.env;
    let terms = &shared.terminals;
    let reply = match op {
        TermOp::Open { cols, rows, cwd } => to_value(&check(terms.open(
            &root,
            env,
            u64::from(cols),
            u64::from(rows),
            cwd.as_deref().unwrap_or(""),
        ))?),
        TermOp::Write { term, data_b64 } => to_value(&check(terms.write(&term, &data_b64))?),
        TermOp::Read { term, offset } => to_value(&check(terms.read(&term, offset))?),
        TermOp::Resize { term, cols, rows } => to_value(&check(terms.resize(
            &term,
            u64::from(cols),
            u64::from(rows),
        ))?),
        TermOp::Close { term } => to_value(&check(terms.close(&term))?),
        TermOp::List => to_value(&check(terms.list())?),
    }?;
    Ok(Some(reply))
}

fn run(shared: &Shared, op: RunOp) -> Result<Option<Value>, String> {
    let root = shared.root();
    let runs = &shared.runs;
    let reply = match op {
        RunOp::Start { command, timeout_s } => {
            let started =
                check(runs.start(&root, &shared.env, &command, timeout_s.map(u64::from)))?;
            remember_run(shared, |ids| {
                ids.insert(started.run.clone());
            });
            to_value(&started)
        }
        RunOp::Poll { run, offset } => to_value(&check(runs.poll(&run, offset))?),
        RunOp::Stop { run } => {
            let stopped = check(runs.stop(&run))?;
            remember_run(shared, |ids| {
                ids.remove(&run);
            });
            to_value(&stopped)
        }
    }?;
    Ok(Some(reply))
}

/// Changes the set of runs STOP ALL will stop.
fn remember_run(shared: &Shared, change: impl FnOnce(&mut std::collections::BTreeSet<String>)) {
    let mut ids = shared.run_ids.lock().unwrap_or_else(|e| e.into_inner());
    change(&mut ids);
}

/// Saves the settings, and moves the work folder when it changed.
fn apply_settings(shared: &Shared, settings: StationSettings) -> Result<(), String> {
    let folder = work_folder(&settings.work_folder, shared.env.home())?;
    fs::create_dir_all(&folder).map_err(|e| format!("cannot create the work folder: {e}"))?;
    state::save_settings(&shared.state_dir, &settings)?;
    shared.set_root(Root::new(folder));
    Ok(())
}

/// STOP ALL: every run this daemon started is stopped, and every terminal is closed. The link runs it on a
/// thread of its own, with a cap on how long the answer waits for it (see `daemon::stop_within`).
pub fn stop_all(shared: &Shared) {
    let runs: Vec<String> = shared
        .run_ids
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .cloned()
        .collect();
    for run in &runs {
        let _ = shared.runs.stop(run);
    }
    remember_run(shared, |ids| ids.clear());
    if let Ok(list) = shared.terminals.list() {
        for term in list.terminals {
            let _ = shared.terminals.close(&term.term);
        }
    }
    shared.log.warn("STOP ALL: runs stopped, terminals closed");
}

/// An `apz_ops` result as the text the hub gets.
fn check<T>(result: Result<T, OpError>) -> Result<T, String> {
    result.map_err(|err| err.to_string())
}

fn to_value<T: Serialize>(item: &T) -> Result<Value, String> {
    serde_json::to_value(item).map_err(|e| e.to_string())
}

/// The operation's name, for the log.
fn kind_of(op: &Op) -> &'static str {
    match op {
        Op::Files(_) => "files",
        Op::Term(_) => "term",
        Op::Run(_) => "run",
        Op::Script { .. } => "script",
        Op::Settings(_) => "settings",
        Op::Facts => "facts",
        Op::Sync(_) => "sync",
        _ => "operation",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::sync::RwLock;
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};

    use apz_ops::{Env, Runs, Terminals};
    use apz_station::Facts;

    use super::*;
    use crate::backend::Backend;
    use crate::log::Log;
    use crate::uplink::Uplink;

    /// A station whose work folder is a fresh scratch folder, with the fake backend and no facts to report.
    fn station(name: &str) -> (PathBuf, Shared) {
        let dir = std::env::temp_dir().join(format!(
            "apz-workstation-work-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch folder");
        let shared = Shared {
            frozen: AtomicBool::new(false),
            freezes: AtomicU64::new(0),
            root: RwLock::new(Root::new(&dir)),
            env: Env::new(dir.clone(), "/usr/bin:/bin".to_string()),
            state_dir: dir.clone(),
            runs: Runs::new(),
            run_ids: Mutex::new(BTreeSet::new()),
            terminals: Terminals::new(),
            uplink: Uplink::new(),
            log: Log::open(&dir),
            apps: AtomicU32::new(0),
            gather: Arc::new(|_: u32| -> Facts { unreachable!("the test asks for no facts") }),
            backend: Backend::Fake,
            sync: crate::sync::Mirror::default(),
        };
        (dir, shared)
    }

    #[test]
    fn a_job_taken_during_stop_all_is_refused_and_does_not_run() {
        let (dir, shared) = station("frozen");
        let write = |path: &str| {
            Op::Files(FilesOp::Write {
                path: path.to_string(),
                text: "x\n".to_string(),
                expected_hash: None,
            })
        };
        // Before STOP ALL, a write runs.
        assert!(answer(&shared, 0, write("live.txt")).is_ok());
        assert!(dir.join("live.txt").exists());
        // A job queued before STOP ALL and taken after it is refused, and its write never happens.
        shared.frozen.store(true, Ordering::SeqCst);
        shared.freezes.store(1, Ordering::SeqCst);
        assert_eq!(
            answer(&shared, 0, write("queued.txt")),
            Err(FROZEN.to_string())
        );
        serve(
            &shared,
            Job {
                generation: 1,
                id: 2,
                op: write("served.txt"),
                epoch: 0,
            },
        );
        assert!(!dir.join("queued.txt").exists() && !dir.join("served.txt").exists());
        // Lifted before the worker takes it, a job admitted before the STOP ALL still does not run; one admitted
        // after the lift does.
        shared.frozen.store(false, Ordering::SeqCst);
        assert_eq!(
            answer(&shared, 0, write("stale.txt")),
            Err(FROZEN.to_string())
        );
        assert!(!dir.join("stale.txt").exists());
        assert!(answer(&shared, 1, write("fresh.txt")).is_ok());
        assert!(dir.join("fresh.txt").exists());
    }
}
