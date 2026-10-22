//! The saver: keeps every component's tables equal to its in-memory state, writing only what changed.
//!
//! At start [`load`] fills each component from its tables and records what was loaded as saved. Then a thread runs a
//! [`pass`] every [`EVERY`] (and once more at shutdown): for each component, its rows now, diffed against what was last
//! saved, become one write (upserts and deletes) through `plaza.db`'s writer; once it commits, it is what was saved.
//! A failed write is retried by the next pass (the diff is computed again from the last commit that succeeded). A
//! component whose load failed is never written while the hub runs: its working copy is not what its tables hold, so a
//! save would put defaults over the stored rows. Saves of all components go one at a time (a pass and a `save_now`
//! never diff the same last save at once).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use apz_store::{Saved, StoreError};

use crate::state::App;
use crate::store::components::{self, Component};

/// How often the saver writes what changed.
pub const EVERY: Duration = Duration::from_secs(1);

/// What each component last saved, by name.
#[derive(Default)]
pub struct Ledger {
    saved: Mutex<HashMap<&'static str, Saved>>,
    /// The components whose load failed: held out of every write until the hub starts again.
    held_out: Mutex<HashSet<&'static str>>,
    /// Held across a component's diff, write and commit.
    saving: Mutex<()>,
}

/// Loads every component from `plaza.db` (at start, before anything serves). A component that fails to load is
/// reported and starts empty; the others still load.
pub fn load(app: &App) -> Vec<String> {
    let mut problems = Vec::new();
    let db = app.store.db(crate::store::PLAZA);
    for c in components::all() {
        let loaded = db.read(|conn| (c.load)(app, conn));
        if let Err(e) = loaded {
            problems.push(format!(
                "{}: {e} (it is not written until the hub starts again)",
                c.name
            ));
            app.saved
                .held_out
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(c.name);
            continue;
        }
        match (c.rows)(app) {
            Ok(rows) => {
                let saved = Saved::of((c.def)(), &rows);
                app.saved
                    .saved
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(c.name, saved);
            }
            Err(e) => problems.push(format!("{}: {e}", c.name)),
        }
    }
    problems
}

/// Writes what changed in every component since the last pass. Returns how many components were written.
pub fn pass(app: &App) -> usize {
    let mut written = 0;
    for c in components::all() {
        match save_one(app, &c) {
            Ok(true) => written += 1,
            Ok(false) => {}
            Err(e) => eprintln!(
                "store: saving {} failed (retried on the next pass): {e}",
                c.name
            ),
        }
    }
    written
}

fn save_one(app: &App, c: &Component) -> Result<bool, StoreError> {
    if app
        .saved
        .held_out
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(c.name)
    {
        return Ok(false);
    }
    let _one_at_a_time = app
        .saved
        .saving
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let def = (c.def)();
    let rows = (c.rows)(app)?;
    let changes = {
        let saved = app
            .saved
            .saved
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        saved
            .get(c.name)
            .cloned()
            .unwrap_or_default()
            .diff(def, &rows)
    };
    if changes.is_empty() {
        return Ok(false);
    }
    let written = app.store.db(crate::store::PLAZA).write(move |conn| {
        apz_store::table::apply(conn, def, &changes)?;
        Ok(changes)
    })?;
    app.saved
        .saved
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(c.name)
        .or_default()
        .commit(written.value);
    Ok(true)
}

/// Writes component `name` now (an owner's message, a card answer, a rule): its write commits before this returns.
/// Whether it is saved (false when the write failed; the next pass tries again).
pub fn save_now(app: &App, name: &str) -> bool {
    let Some(c) = components::all().into_iter().find(|c| c.name == name) else {
        return false;
    };
    match save_one(app, &c) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("store: saving {name} failed (retried on the next pass): {e}");
            false
        }
    }
}

/// Starts the saver thread.
pub fn spawn(app: Arc<App>) {
    std::thread::Builder::new()
        .name("store-saver".into())
        .spawn(move || {
            loop {
                std::thread::sleep(EVERY);
                pass(&app);
            }
        })
        .expect("spawn the store saver");
}
