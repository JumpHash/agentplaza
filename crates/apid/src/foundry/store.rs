//! The Foundry's state: recipes, dataset metadata, jobs and their series. Kept in `plaza.db` as the `foundry`
//! component (its tables: `foundry`, and its recipes, datasets and jobs as children); the event logs and the recipe
//! and dataset files live under the Foundry's directory (`<state>/foundry`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::foundry::datasets::Dataset;
use crate::foundry::diloco::Hub;

/// The recipe the in-repo trainer ships; seeded as a local dev recipe.
pub const SEED_RECIPE: &str = "rung-predictor@1";
/// The seed recipe's source, relative to this crate.
const SEED_SOURCE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../trainer/recipes/rung_predictor"
);
/// Series points kept per job (the newest).
pub const SERIES_CAP: usize = 5000;

/// A recipe at a vetted commit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, apz_store::Table)]
pub struct Recipe {
    pub id: String,
    pub name: String,
    pub repo: String,
    pub commit: String,
    /// The recipe's schema as its file gives it (shown and forwarded, never read by the hub).
    #[table(content)]
    pub schema: Value,
    pub vetted_by: Option<String>,
    pub vetted_ms: Option<u64>,
    /// A recipe from this repo's trainer tree, pinned by a hash of its files, not a commit.
    pub local_dev: bool,
}

/// One noded job: a machine's share of a plan island.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, apz_store::Table)]
pub struct Run {
    /// The noded job id (`<job>-<island>-<n>`).
    pub node_job: String,
    pub node: String,
    pub island: String,
    #[table(list)]
    pub devices: Vec<u32>,
    pub started_ms: u64,
    pub ended_ms: Option<u64>,
    /// `running`, `exited`, or `stopped` (cancelled by the hub).
    pub state: String,
    pub exit_code: Option<i64>,
    /// The next noded event line to ask for.
    pub next: u64,
    pub gpu_hours: f64,
}

/// One point of a job's series: a trainer event the hub kept (`kind` is the event's name, as the consoles read it).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, apz_store::Table)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Point {
    Step {
        island: String,
        step: Option<u64>,
        loss: Option<f64>,
        lr: Option<f64>,
        samples: Option<u64>,
        t: u64,
    },
    Eval {
        island: String,
        step: Option<u64>,
        split: Option<String>,
        /// The split's metrics as the trainer wrote them (shown verbatim).
        #[table(content)]
        metrics: Value,
        t: u64,
    },
    Round {
        island: String,
        round: Option<u64>,
        accepted: Option<bool>,
        hub_round: Option<u64>,
        step: Option<u64>,
        t: u64,
    },
    Trial {
        island: String,
        trial: Option<u64>,
        objective: Option<f64>,
        t: u64,
    },
}

/// A training job: its document, its plan, its runs and the series for the charts.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, apz_store::Table)]
pub struct Job {
    pub id: String,
    /// `queued`, `running`, `done`, `failed` or `cancelled`.
    pub status: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub recipe: String,
    #[table(list)]
    pub datasets: Vec<String>,
    pub mode: String,
    pub budget_gpu_hours: f64,
    pub gpu_hours: f64,
    /// The breeder (agent cid) that submitted the job, from its document; absent for the owner's own jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<u64>,
    /// The job document as submitted (kept as written).
    #[table(content)]
    pub doc: Value,
    /// The plan with the host paths of the job directory the trainer reads (shown verbatim).
    #[table(content)]
    pub plan: Value,
    #[table(child)]
    pub runs: Vec<Run>,
    /// Loss, eval and round points, newest last, capped at `SERIES_CAP`.
    #[table(child)]
    pub series: Vec<Point>,
    /// The last event sequence number written to the job's `events.jsonl`.
    pub seq: u64,
    #[table(list)]
    pub outputs: Vec<String>,
    /// True when a run is on a remote machine, whose outputs stay there.
    pub outputs_remote: bool,
    /// The job's metrics from the trainer's `done` event, per split (shown verbatim).
    #[table(content)]
    pub metrics: Value,
    pub error: Option<String>,
    /// The islands whose models were not registered: the other islands of a multi-island job,
    /// best eval first. Empty for a single island.
    #[serde(default)]
    #[table(list)]
    pub candidates: Vec<String>,
}

/// Everything persisted: the `foundry` row, with its recipes, datasets and jobs as children keyed by id.
#[derive(Debug, Default, Serialize, Deserialize, apz_store::Table)]
#[table(name = "foundry")]
pub struct State {
    #[table(map)]
    pub recipes: BTreeMap<String, Recipe>,
    #[table(map)]
    pub datasets: BTreeMap<String, Dataset>,
    #[table(map)]
    pub jobs: BTreeMap<String, Job>,
}

/// The Foundry's store: the state behind a lock, and the directory it keeps files in.
pub struct Foundry {
    pub dir: PathBuf,
    state: RwLock<State>,
    /// The DiLoCo hub of each loose job. Live state: not persisted, lost on restart.
    pub hubs: RwLock<BTreeMap<String, Arc<Hub>>>,
}

impl Foundry {
    /// An empty Foundry under `dir`; [`Foundry::load`] fills it from the store at start.
    pub fn new(dir: PathBuf) -> Foundry {
        Foundry {
            dir,
            state: RwLock::new(State::default()),
            hubs: RwLock::new(BTreeMap::new()),
        }
    }

    /// Fills the state from the store (at start), then seeds the dev recipe when the store lacks it.
    pub fn load(&self, conn: &Connection) -> Result<(), StoreError> {
        let stored = apz_store::table::load_one::<State>(conn, def())?.unwrap_or_default();
        *self.state.write().unwrap_or_else(|e| e.into_inner()) = stored;
        self.seed(apz_plaza::now_ms());
        Ok(())
    }

    /// The state as its rows, as it is now (the saver writes what changed).
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());
        RowSet::one(def(), &*state)
    }

    /// Copies the seed recipe into the Foundry's directory (noded mounts must sit under the state
    /// directory, so the trainer reads this copy) and registers it, vetted as a local dev pin.
    fn seed(&self, now_ms: u64) {
        let source = Path::new(SEED_SOURCE);
        let Ok(meta) = std::fs::read(source.join("recipe.json")) else {
            return;
        };
        let Ok(code) = std::fs::read(source.join("recipe.py")) else {
            return;
        };
        let dest = self.recipes_dir().join("rung_predictor");
        if std::fs::create_dir_all(&dest).is_err()
            || std::fs::write(dest.join("recipe.json"), &meta).is_err()
            || std::fs::write(dest.join("recipe.py"), &code).is_err()
        {
            return;
        }
        let mut hasher = Sha256::new();
        hasher.update(&meta);
        hasher.update(&code);
        let pin = format!("dev-{}", &hex(&hasher.finalize())[..16]);
        let spec: Value = serde_json::from_slice(&meta).unwrap_or(Value::Null);
        let recipe = Recipe {
            id: SEED_RECIPE.into(),
            name: spec["title"].as_str().unwrap_or(SEED_RECIPE).into(),
            repo: "local:trainer/recipes".into(),
            commit: pin,
            schema: json!({"input": spec["input"], "output": spec["output"]}),
            vetted_by: Some("local-dev".into()),
            vetted_ms: Some(now_ms),
            local_dev: true,
        };
        let mut state = self.write();
        state.recipes.entry(SEED_RECIPE.into()).or_insert(recipe);
    }

    pub fn recipes_dir(&self) -> PathBuf {
        self.dir.join("recipes")
    }

    pub fn jobs_dir(&self) -> PathBuf {
        self.dir.join("jobs")
    }

    /// Reads the state.
    pub fn read<R>(&self, f: impl FnOnce(&State) -> R) -> R {
        f(&self.state.read().unwrap_or_else(|e| e.into_inner()))
    }

    /// Changes the state.
    pub fn write(&self) -> std::sync::RwLockWriteGuard<'_, State> {
        self.state.write().unwrap_or_else(|e| e.into_inner())
    }
}

/// The Foundry's tables (`foundry`, and its children).
pub fn def() -> &'static TableDef {
    apz_store::root_def!(State)
}

/// A Foundry loaded from an empty store in `dir`, as a test starts one (the dev recipe seeded).
#[cfg(test)]
pub(crate) fn open_for_test(dir: PathBuf) -> Foundry {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(&def().ddl()).unwrap();
    let foundry = Foundry::new(dir);
    foundry.load(&conn).unwrap();
    foundry
}

/// Lower-case hex of bytes.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A connection with the Foundry's tables made, as the hub's plaza.db has them.
    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        conn
    }

    /// A Foundry over the same folder, loaded from the rows of `source` as a restart loads them.
    fn restarted(source: &Foundry) -> Foundry {
        let conn = migrated();
        apz_store::table::replace(&conn, def(), &source.rows().unwrap()).unwrap();
        let loaded = Foundry::new(source.dir.clone());
        loaded.load(&conn).unwrap();
        loaded
    }

    /// The state is kept in `plaza.db`: populated, written as rows, loaded into a fresh Foundry and the same.
    #[test]
    fn the_state_comes_back_whole_from_its_tables() {
        let dir = std::env::temp_dir().join(format!("foundry-persist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let foundry = Foundry::new(dir.clone());
        foundry.load(&migrated()).unwrap();
        foundry.write().recipes.insert(
            "rung-test@1".into(),
            Recipe {
                id: "rung-test@1".into(),
                name: "test".into(),
                repo: "forge:owner/recipes".into(),
                commit: "c0ffee".into(),
                schema: json!({"input": {"type": "object"}}),
                vetted_by: Some("owner".into()),
                vetted_ms: Some(2),
                local_dev: false,
            },
        );
        foundry.write().datasets.insert(
            "ds-test".into(),
            Dataset {
                id: "ds-test".into(),
                name: "test rows".into(),
                kind: "cascade".into(),
                created_ms: 3,
                from_ms: 0,
                to_ms: 9,
                rows: 4,
                frozen_rows: 1,
                label_balance: BTreeMap::from([("cat".to_string(), 2)]),
                taint: vec!["dup".into()],
                provenance: json!({"sources": ["cascade"]}),
                file: "datasets/ds-test.jsonl".into(),
                frozen_file: "datasets/ds-test.frozen.jsonl".into(),
                sha256: "ab".into(),
            },
        );
        foundry.write().jobs.insert(
            "job-1".into(),
            Job {
                id: "job-1".into(),
                status: "running".into(),
                recipe: SEED_RECIPE.into(),
                mode: "loose".into(),
                datasets: vec!["ds-test".into()],
                agent: Some(7),
                doc: json!({"agent": 7, "steps": 10}),
                plan: json!({"islands": ["i1"]}),
                runs: vec![Run {
                    node_job: "job-1-i1-1".into(),
                    node: "n0".into(),
                    island: "i1".into(),
                    devices: vec![0, 1],
                    started_ms: 4,
                    ended_ms: None,
                    state: "running".into(),
                    exit_code: None,
                    next: 3,
                    gpu_hours: 0.5,
                }],
                series: vec![
                    Point::Step {
                        island: "i1".into(),
                        step: Some(1),
                        loss: Some(0.75),
                        lr: None,
                        samples: Some(32),
                        t: 5,
                    },
                    Point::Eval {
                        island: "i1".into(),
                        step: Some(2),
                        split: Some("val".into()),
                        metrics: json!({"accuracy": 0.5}),
                        t: 6,
                    },
                    Point::Round {
                        island: "i1".into(),
                        round: Some(1),
                        accepted: Some(true),
                        hub_round: None,
                        step: Some(2),
                        t: 7,
                    },
                    Point::Trial {
                        island: "i1".into(),
                        trial: Some(3),
                        objective: Some(0.25),
                        t: 8,
                    },
                ],
                seq: 9,
                outputs: vec!["out/model.onnx".into()],
                outputs_remote: true,
                metrics: json!({"val": {"accuracy": 0.9}}),
                error: None,
                candidates: vec!["i2".into()],
                ..Job::default()
            },
        );
        let before = foundry.read(|s| serde_json::to_value(s).unwrap());
        let back = restarted(&foundry);
        assert_eq!(back.read(|s| serde_json::to_value(s).unwrap()), before);
        assert!(back.read(|s| s.jobs.contains_key("job-1") && s.datasets.contains_key("ds-test")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn seed_recipe_is_vetted_as_a_local_dev_pin() {
        let dir = std::env::temp_dir().join(format!("foundry-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let foundry = Foundry::new(dir.clone());
        foundry.load(&migrated()).unwrap();
        let recipe = foundry
            .read(|s| s.recipes.get(SEED_RECIPE).cloned())
            .expect("seeded");
        assert!(recipe.local_dev);
        assert!(recipe.commit.starts_with("dev-"));
        assert!(recipe.vetted_ms.is_some());
        assert!(dir.join("recipes/rung_predictor/recipe.py").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
