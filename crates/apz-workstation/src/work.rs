//! The worker pool: files, terminals, runs, scripts, settings and facts.
//!
//! The workers share one queue, so a slow job (a script waiting on its command, a facts query) holds one worker
//! and the rest keep serving. Files, terminals and runs work inside the work folder through `apz_ops`, which
//! refuses any path that leaves it.

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

use crate::daemon::{Job, Shared, WorkMsg, work_folder};
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
            Ok(WorkMsg::StopAll) => stop_all(shared),
            Err(_) => return,
        }
    }
}

fn serve(shared: &Shared, job: Job) {
    let kind = kind_of(&job.op);
    let outcome = handle(shared, job.op);
    if let Err(err) = &outcome {
        shared.log.info(format!("{kind} refused: {err}"));
    }
    shared.uplink.respond(job.generation, job.id, outcome);
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
        Op::Sync(_) => Err("work-folder sync is not built in this version".to_string()),
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

/// STOP ALL: every run this daemon started is stopped, and every terminal is closed.
fn stop_all(shared: &Shared) {
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
