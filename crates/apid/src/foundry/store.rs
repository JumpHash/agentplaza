//! The Foundry's state: recipes, dataset metadata, jobs and their series. Persisted as one
//! `foundry` component (db.rs); the event logs and the recipe and dataset files live under the
//! Foundry's directory (`<state>/foundry`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::foundry::datasets::Dataset;
use crate::foundry::diloco::Hub;

/// The recipe the in-repo trainer ships; seeded as a local dev recipe.
pub const SEED_RECIPE: &str = "rung-predictor@1";
/// The seed recipe's source, relative to this crate.
const SEED_SOURCE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../trainer/recipes/rung_predictor");
/// Series points kept per job (the newest).
pub const SERIES_CAP: usize = 5000;

/// A recipe at a vetted commit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Recipe {
    pub id: String,
    pub name: String,
    pub repo: String,
    pub commit: String,
    pub schema: Value,
    pub vetted_by: Option<String>,
    pub vetted_ms: Option<u64>,
    /// A recipe from this repo's trainer tree, pinned by a hash of its files, not a Forgejo commit.
    pub local_dev: bool,
}

/// One noded job: a machine's share of a plan island.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Run {
    /// The noded job id (`<job>-<island>-<n>`).
    pub node_job: String,
    pub node: String,
    pub island: String,
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

/// A training job: its document, its plan, its runs and the series for the charts.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Job {
    pub id: String,
    /// `queued`, `running`, `done`, `failed` or `cancelled`.
    pub status: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub recipe: String,
    pub datasets: Vec<String>,
    pub mode: String,
    pub budget_gpu_hours: f64,
    pub gpu_hours: f64,
    /// The job document as submitted.
    pub doc: Value,
    /// The plan with the container paths the trainer reads.
    pub plan: Value,
    pub runs: Vec<Run>,
    /// Loss, eval and round points, newest last, capped at `SERIES_CAP`.
    pub series: Vec<Value>,
    /// The last event sequence number written to the job's `events.jsonl`.
    pub seq: u64,
    pub outputs: Vec<String>,
    /// True when a run is on a remote machine, whose outputs stay there.
    pub outputs_remote: bool,
    pub metrics: Value,
    pub error: Option<String>,
}

/// Everything persisted.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    pub recipes: BTreeMap<String, Recipe>,
    pub datasets: BTreeMap<String, Dataset>,
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
    /// Opens the store under `dir`, seeding the local dev recipe when it is missing.
    pub fn open(dir: PathBuf, now_ms: u64) -> Foundry {
        let foundry = Foundry { dir, state: RwLock::new(State::default()), hubs: RwLock::new(BTreeMap::new()) };
        foundry.seed(now_ms);
        foundry
    }

    /// Copies the seed recipe into the Foundry's directory (noded mounts must sit under the state
    /// directory, so the trainer reads this copy) and registers it, vetted as a local dev pin.
    fn seed(&self, now_ms: u64) {
        let source = Path::new(SEED_SOURCE);
        let Ok(meta) = std::fs::read(source.join("recipe.json")) else { return };
        let Ok(code) = std::fs::read(source.join("recipe.py")) else { return };
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

    /// The JSON the db writer stores (series capped by construction).
    pub fn snapshot(&self) -> String {
        self.read(|s| serde_json::to_string(s)).unwrap_or_default()
    }

    /// Restores a snapshot, then seeds the dev recipe again when the snapshot lacks it.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let restored: State = serde_json::from_str(json).map_err(|e| e.to_string())?;
        *self.write() = restored;
        self.seed(apz_plaza::now_ms());
        Ok(())
    }
}

/// Lower-case hex of bytes.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_recipe_is_vetted_as_a_local_dev_pin() {
        let dir = std::env::temp_dir().join(format!("foundry-store-{}", std::process::id()));
        let foundry = Foundry::open(dir.clone(), 7);
        let recipe = foundry.read(|s| s.recipes.get(SEED_RECIPE).cloned()).expect("seeded");
        assert!(recipe.local_dev);
        assert!(recipe.commit.starts_with("dev-"));
        assert_eq!(recipe.vetted_ms, Some(7));
        assert!(dir.join("recipes/rung_predictor/recipe.py").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn snapshot_round_trips() {
        let dir = std::env::temp_dir().join(format!("foundry-snap-{}", std::process::id()));
        let foundry = Foundry::open(dir.clone(), 1);
        let before = foundry.snapshot();
        let other = Foundry::open(dir.clone(), 2);
        other.restore(&before).expect("restores");
        assert_eq!(other.snapshot(), before);
        assert!(other.restore("not json").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
