//! The Foundry's model registry and model agents (`docs/design/foundry.md`, "The hub").
//!
//! A finished training job leaves `model.onnx` and `model_card.json` in its job directory. `import`
//! (or the jobs agent's `register_from_job`) reads them, checks the inference contract and the
//! bytes with tract (`inference::LoadedModel::load`), and records the model as `trained`. States
//! go `trained -> shadow -> promoted -> retired`: only the owner promotes or retires. A promoted
//! model gets a model agent on the roster: `kind: "model"`, a CID in the reserved range, no VM, wallet,
//! tools or chat. Retiring it removes the entry again.
//!
//! The job's model card has no inference contract yet, so the registry reads an object `serving`
//! from it: `input` and `output` (schemas), `features`, optional `labels` and `allowed_roles`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use apz_http::{Response, RouteDoc, Router};
use apz_proto::chat::Usage;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::text::{self, TextContract, TextModel};
use crate::agents::{Agent, Roster, Status};
use crate::inference::{FeatureSpec, Labels, LoadedModel, ModelCard, Registry as Loaded, Schema};
use crate::state::App;

/// The reserved CID range of model agents: `MODEL_CID_BASE ..= MODEL_CID_BASE + MODEL_CID_COUNT - 1`
/// (10000..=10999). VM guest CIDs are 20..=99 (node n0) and 200..=299 (node n0's VMs), and remote
/// nodes take `REMOTE_CID_BASE` (1000) plus 100 per node, so the range is out of their reach until
/// about 90 nodes have joined.
pub const MODEL_CID_BASE: u64 = 10_000;
pub const MODEL_CID_COUNT: u64 = 1_000;
/// The role and kind of a model agent on the roster.
pub const MODEL_ROLE: &str = "model";
pub const MODEL_KIND: &str = "model";

/// Whether `cid` belongs to the model range. The laws read it as a model agent's kind.
pub fn is_model_cid(cid: u64) -> bool {
    (MODEL_CID_BASE..MODEL_CID_BASE + MODEL_CID_COUNT).contains(&cid)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Trained,
    Shadow,
    Promoted,
    Retired,
    /// Downloaded from Hugging Face without a serving contract: listed, never compiled or promoted.
    Imported,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Trained => "trained",
            State::Shadow => "shadow",
            State::Promoted => "promoted",
            State::Retired => "retired",
            State::Imported => "imported",
        }
    }
}

/// Where a registered model came from: a Hugging Face repo at a commit (`hf_import.rs`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct Source {
    pub hf: String,
    pub revision: String,
    /// `base` or `servable`, as the import asked.
    #[serde(rename = "as")]
    pub as_: String,
    /// The download directory.
    pub path: String,
    /// Why the model is not servable, when it is not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub servable_reason: Option<String>,
    /// The repo's Hugging Face task (`pipeline_tag`), when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

/// How often a shadow model agreed with the path it runs beside, and what its use would have saved.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct ShadowStats {
    pub runs: u64,
    pub agreed: u64,
    pub cost_saved_usd: f64,
    pub updated_ms: u64,
}

impl ShadowStats {
    /// Agreed runs over all runs; `None` before the first run.
    pub fn agreement_rate(&self) -> Option<f64> {
        (self.runs > 0).then(|| self.agreed as f64 / self.runs as f64)
    }
}

/// One registered model: a row of the `model_registry` tables (its card a child), beside its `model.onnx`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct ModelRecord {
    #[table(child)]
    pub card: ModelCard,
    pub state: State,
    pub source_job: Option<String>,
    /// The model agent's CID while promoted (kept after retirement for the record).
    pub agent_cid: Option<u64>,
    /// The data provenance per split, from the job's card (shown verbatim).
    #[serde(default)]
    #[table(content)]
    pub provenance: Value,
    /// Calibration numbers, when the job's card has them (shown verbatim).
    #[serde(default)]
    #[table(content)]
    pub calibration: Value,
    pub size_bytes: u64,
    #[serde(default)]
    pub parameters: u64,
    #[serde(default)]
    #[table(flatten)]
    pub shadow: ShadowStats,
    pub created_ms: u64,
    pub state_changed_ms: u64,
    /// The Hugging Face source, for models imported from the hub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[table(flatten)]
    pub source: Option<Source>,
}

/// The registry's tables: `model_registry`, one row, with every record a child keyed by its model id.
#[derive(Debug, Default, apz_store::Table)]
#[table(name = "model_registry")]
struct Stored {
    #[table(map)]
    records: BTreeMap<String, ModelRecord>,
}

/// The registry's tables (`model_registry`).
pub fn def() -> &'static apz_store::TableDef {
    apz_store::root_def!(Stored)
}

/// The inference contract a job's card carries under `serving`.
#[derive(Debug, Deserialize)]
struct Serving {
    input: Schema,
    output: Schema,
    features: Vec<FeatureSpec>,
    #[serde(default)]
    labels: Option<Vec<String>>,
    allowed_roles: Vec<String>,
}

/// The model store and the loaded models. The records live in `plaza.db` (the `models` component); the ONNX
/// files and the compiled graphs live on disk and in memory.
pub struct Models {
    dir: PathBuf,
    records: Mutex<BTreeMap<String, ModelRecord>>,
    loaded: Loaded,
    /// Counts the changes to `records` (see `records_mut`).
    generation: std::sync::atomic::AtomicU64,
    /// What `derived_ids` built, with the generation it was built at.
    derived: Mutex<Option<(u64, Vec<String>)>>,
    /// The text models, run from their download directories (text.rs), by id.
    text: Mutex<BTreeMap<String, Arc<TextModel>>>,
}

impl Models {
    /// The store under `<state_dir>/foundry/models` (the ONNX files), with no records until [`Models::load`].
    /// Graphs are compiled by [`Models::revive`].
    pub fn open(state_dir: &Path) -> Models {
        let dir = state_dir.join("foundry").join("models");
        Models {
            dir,
            records: Mutex::new(BTreeMap::new()),
            loaded: Loaded::default(),
            generation: std::sync::atomic::AtomicU64::new(0),
            derived: Mutex::new(None),
            text: Mutex::new(BTreeMap::new()),
        }
    }

    fn records(&self) -> MutexGuard<'_, BTreeMap<String, ModelRecord>> {
        self.records.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The records for a change. The generation moves while the lock is held, so a reader that sees a
    /// new generation also sees the change.
    fn records_mut(&self) -> MutexGuard<'_, BTreeMap<String, ModelRecord>> {
        let guard = self.records();
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        guard
    }

    /// The number of changes to the records so far.
    pub fn generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The ids `build` derives from the records, rebuilt only when the generation has moved (the
    /// cascade's shadow hook reads them on every metered call).
    pub fn derived_ids(&self, build: impl FnOnce(&[ModelRecord]) -> Vec<String>) -> Vec<String> {
        let generation = self.generation();
        let mut cache = self.derived.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((seen, ids)) = cache.as_ref()
            && *seen == generation
        {
            return ids.clone();
        }
        let records: Vec<ModelRecord> = self.list();
        let ids = build(&records);
        *cache = Some((generation, ids.clone()));
        ids
    }

    fn model_dir(&self, id: &str) -> PathBuf {
        self.dir.join(id)
    }

    /// Compiles every model that is not retired and puts the promoted ones back on the roster, with
    /// the CIDs they had. Run once at startup.
    pub fn revive(&self, roster: &Roster) {
        let records: Vec<ModelRecord> = self.records().values().cloned().collect();
        let now = apz_plaza::now_ms();
        for record in records {
            if matches!(record.state, State::Retired | State::Imported) {
                continue;
            }
            if let Some(contract) = text_contract(&record) {
                match record
                    .source
                    .as_ref()
                    .map(|s| TextModel::load(Path::new(&s.path), &contract))
                {
                    Some(Ok(model)) => self.text_insert(&record.card.id, model),
                    other => eprintln!(
                        "foundry: text model {} does not load: {:?}",
                        record.card.id,
                        other.and_then(|r| r.err())
                    ),
                }
                continue;
            }
            let compiled = match self.compile(&record) {
                Ok(model) => model,
                Err(e) => {
                    eprintln!("foundry: model {} does not load: {e}", record.card.id);
                    continue;
                }
            };
            self.loaded.insert(compiled);
            remember(&record.card);
            if let (State::Promoted, Some(cid)) = (record.state, record.agent_cid) {
                roster.upsert(model_agent(cid, now));
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<ModelRecord> {
        self.records().get(id).cloned()
    }

    /// Every record, by id.
    pub fn list(&self) -> Vec<ModelRecord> {
        self.records().values().cloned().collect()
    }

    /// The compiled model for `id` when it is loaded (trained, shadow or promoted).
    pub fn loaded(&self, id: &str) -> Option<std::sync::Arc<LoadedModel>> {
        self.loaded.get(id)
    }

    /// Registers the model in a job directory (`model.onnx` and `model_card.json`) as `trained`.
    /// The jobs agent calls this with the finished job's directory and its id.
    pub fn register_from_job(
        &self,
        job_dir: &Path,
        source_job: Option<&str>,
    ) -> Result<ModelRecord, String> {
        self.import_dir(job_dir, source_job)
    }

    /// `POST /v1/foundry/models/import`: the same registration from any directory (owner only).
    pub fn import(&self, dir: &Path) -> Result<ModelRecord, String> {
        self.import_dir(dir, None)
    }

    /// Registers a Hugging Face download (`hf_import.rs`). A directory with a serving contract and an
    /// ONNX graph registers like `import` (state `trained`) and gets the source attached. Anything else
    /// registers as `imported`: listed, never compiled, never promoted. Idempotent per repo and revision.
    pub fn register_hf(
        &self,
        dir: &Path,
        source: Source,
        id: &str,
        size_bytes: u64,
    ) -> Result<ModelRecord, String> {
        let same =
            |s: &Source| s.hf == source.hf && s.revision == source.revision && s.as_ == source.as_;
        if let Some(existing) = self
            .list()
            .into_iter()
            .find(|r| r.source.as_ref().is_some_and(same))
        {
            return Ok(existing);
        }
        let mut source = source;
        let mut problem = servable_problem(dir, &source.as_);
        if problem.is_some()
            && source.as_ == "servable"
            && let Some(task) = source.task.clone()
        {
            match self.register_text(dir, &source, &task, id, size_bytes) {
                Ok(record) => return Ok(record),
                Err(e) => problem = Some(e),
            }
        }
        if problem.is_none() {
            match self.import_dir(dir, None) {
                Ok(record) => return self.attach_source(&record.card.id, source),
                Err(e) => problem = Some(e),
            }
        }
        source.servable_reason = problem;
        let now = apz_plaza::now_ms();
        let mut records = self.records_mut();
        if records.contains_key(id) {
            return Err(format!("a model with id {id} is already registered"));
        }
        let empty = Schema {
            kind: "object".into(),
            properties: BTreeMap::new(),
            required: Vec::new(),
        };
        let card = ModelCard {
            id: id.to_string(),
            name: source.hf.clone(),
            version: "1".into(),
            input: empty.clone(),
            output: empty,
            features: Vec::new(),
            labels: None,
            allowed_roles: Vec::new(),
            onnx_sha256: String::new(),
            created_ms: now,
            metrics: Value::Null,
            serving: None,
        };
        let record = ModelRecord {
            card,
            state: State::Imported,
            source_job: None,
            agent_cid: None,
            provenance: Value::Null,
            calibration: json!({}),
            size_bytes,
            parameters: 0,
            shadow: ShadowStats::default(),
            created_ms: now,
            state_changed_ms: now,
            source: Some(source),
        };
        records.insert(id.to_string(), record.clone());
        Ok(record)
    }

    fn attach_source(&self, id: &str, source: Source) -> Result<ModelRecord, String> {
        let mut records = self.records_mut();
        let record = records.get_mut(id).ok_or_else(|| no_model(id))?;
        record.source = Some(source);
        Ok(record.clone())
    }

    /// Registers a text model (text.rs): derives its contract from the download, loads it, writes the contract
    /// into the download's model card and records the model as trained.
    fn register_text(
        &self,
        dir: &Path,
        source: &Source,
        task: &str,
        id: &str,
        size_bytes: u64,
    ) -> Result<ModelRecord, String> {
        let graph = text::graph_in(dir)?;
        let contract = text::derive(dir, &graph, task)?;
        let model = TextModel::load(dir, &contract)?;
        let onnx = fs::read(dir.join(&graph)).map_err(|e| format!("reading {graph}: {e}"))?;
        let now = apz_plaza::now_ms();
        let mut records = self.records_mut();
        if records.contains_key(id) {
            return Err(format!("a model with id {id} is already registered"));
        }
        text::write_contract(dir, &contract)?;
        let (input, output) = text::schemas(task);
        let card = ModelCard {
            id: id.to_string(),
            name: source.hf.clone(),
            version: "1".into(),
            input,
            output,
            features: Vec::new(),
            labels: contract.labels.clone().map(|names| Labels { names }),
            allowed_roles: vec!["breeder".into()],
            onnx_sha256: sha256_hex(&onnx),
            created_ms: now,
            metrics: Value::Null,
            serving: Some(serde_json::to_value(&contract).map_err(|e| e.to_string())?),
        };
        let record = ModelRecord {
            card,
            state: State::Trained,
            source_job: None,
            agent_cid: None,
            provenance: Value::Null,
            calibration: json!({}),
            size_bytes,
            parameters: 0,
            shadow: ShadowStats::default(),
            created_ms: now,
            state_changed_ms: now,
            source: Some(source.clone()),
        };
        self.text_insert(id, model);
        records.insert(id.to_string(), record.clone());
        Ok(record)
    }

    /// Re-registers an `imported` model from its download: a text model, or a feature contract, that the hub
    /// can serve becomes `trained`. The old record is replaced; a text model keeps its id.
    pub fn rederive(&self, id: &str, task: Option<String>) -> Result<ModelRecord, String> {
        let mut records = self.records_mut();
        let current = records.get(id).cloned().ok_or_else(|| no_model(id))?;
        if current.state != State::Imported {
            return Err(format!(
                "{id} is {}: only imported models are re-derived",
                current.state.name()
            ));
        }
        let mut source = current
            .source
            .clone()
            .ok_or_else(|| format!("{id} has no Hugging Face source"))?;
        if task.is_some() {
            source.task = task;
        }
        records.remove(id);
        drop(records);
        match self.register_hf(&PathBuf::from(&source.path), source, id, current.size_bytes) {
            Ok(record) => {
                if record.card.id != id {
                    let _ = fs::remove_dir_all(self.model_dir(id));
                }
                Ok(record)
            }
            Err(e) => {
                self.records_mut().insert(id.to_string(), current);
                Err(e)
            }
        }
    }

    fn text_insert(&self, id: &str, model: TextModel) {
        self.text
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_string(), Arc::new(model));
    }

    /// The text model for `id`, when it is one.
    pub fn text(&self, id: &str) -> Option<Arc<TextModel>> {
        self.text
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
    }

    fn import_dir(&self, dir: &Path, source_job: Option<&str>) -> Result<ModelRecord, String> {
        let text = fs::read_to_string(dir.join("model_card.json"))
            .map_err(|e| format!("reading model_card.json: {e}"))?;
        let trainer: Value =
            serde_json::from_str(&text).map_err(|e| format!("model_card.json: {e}"))?;
        let serving_value = trainer.get("serving").ok_or(
            "the card has no serving contract (serving: input, output, features, allowed_roles)",
        )?;
        let serving: Serving =
            serde_json::from_value(serving_value.clone()).map_err(|e| format!("serving: {e}"))?;
        let onnx_path = onnx_path(dir);
        let onnx =
            fs::read(&onnx_path).map_err(|e| format!("reading {}: {e}", onnx_path.display()))?;
        check_serving(&serving)?;

        let sha = sha256_hex(&onnx);
        let recipe = trainer
            .get("recipe")
            .and_then(Value::as_str)
            .unwrap_or("model");
        let id = format!("{}-{}", sanitize(recipe), &sha[..8]);
        if !valid_id(&id) {
            return Err(format!("the model id {id:?} is not a valid name"));
        }
        let card = ModelCard {
            id: id.clone(),
            name: recipe.to_string(),
            version: "1".into(),
            input: serving.input,
            output: serving.output,
            features: serving.features,
            labels: serving.labels.map(|names| Labels { names }),
            allowed_roles: serving.allowed_roles,
            onnx_sha256: sha,
            created_ms: apz_plaza::now_ms(),
            metrics: trainer.get("metrics").cloned().unwrap_or(Value::Null),
            serving: Some(serving_value.clone()),
        };
        let record = ModelRecord {
            card: card.clone(),
            state: State::Trained,
            source_job: source_job.map(str::to_string).or_else(|| {
                trainer
                    .get("job_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }),
            agent_cid: None,
            provenance: trainer.get("data").cloned().unwrap_or(Value::Null),
            calibration: trainer
                .get("calibration")
                .cloned()
                .unwrap_or_else(|| json!({})),
            size_bytes: onnx.len() as u64,
            parameters: trainer
                .get("parameters")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            shadow: ShadowStats::default(),
            created_ms: card.created_ms,
            state_changed_ms: card.created_ms,
            source: None,
        };
        let mut records = self.records_mut();
        if records.contains_key(&id) {
            return Err(format!("a model with id {id} is already registered"));
        }
        // Validate before anything is written: the bytes must compile for this card.
        let compiled =
            LoadedModel::load(&onnx, card).map_err(|e| format!("validating the model: {e}"))?;
        let target = self.model_dir(&id);
        fs::create_dir_all(&target).map_err(|e| format!("creating {}: {e}", target.display()))?;
        write_atomic(&target.join("model.onnx"), &onnx)?;
        self.loaded.insert(compiled);
        remember(&record.card);
        records.insert(id, record.clone());
        Ok(record)
    }

    /// The records as rows, as they are now (the saver writes what changed). The ONNX files stay on disk.
    pub fn rows(&self) -> Result<apz_store::RowSet, apz_store::StoreError> {
        let stored = Stored {
            records: (*self.records()).clone(),
        };
        apz_store::RowSet::one(def(), &stored)
    }

    /// Replaces the records with what the store holds (at start). Graphs are not compiled here: startup
    /// calls [`Models::revive`] after the load.
    pub fn load(
        &self,
        conn: &apz_store::rusqlite::Connection,
    ) -> Result<(), apz_store::StoreError> {
        let stored = apz_store::table::load_one::<Stored>(conn, def())?.unwrap_or_default();
        *self.records_mut() = stored.records;
        Ok(())
    }

    fn compile(&self, record: &ModelRecord) -> Result<LoadedModel, String> {
        let onnx = fs::read(self.model_dir(&record.card.id).join("model.onnx"))
            .map_err(|e| format!("reading model.onnx: {e}"))?;
        LoadedModel::load(&onnx, record.card.clone())
    }

    /// `trained -> shadow`: the model runs beside the path it would replace, its decisions logged.
    pub fn shadow(&self, id: &str) -> Result<ModelRecord, String> {
        self.transition(id, &[State::Trained], State::Shadow)
    }

    /// `shadow -> promoted` (owner only, enforced by the route): the model agent appears on the roster.
    pub fn promote(&self, roster: &Roster, id: &str) -> Result<ModelRecord, String> {
        let mut records = self.records_mut();
        let current = records.get(id).ok_or_else(|| no_model(id))?;
        if current.state != State::Shadow {
            return Err(format!(
                "{id} is {}: a model is promoted from shadow",
                current.state.name()
            ));
        }
        let cid = free_cid(roster, &records)?;
        let now = apz_plaza::now_ms();
        let mut record = current.clone();
        roster.upsert(model_agent(cid, now));
        record.state = State::Promoted;
        record.agent_cid = Some(cid);
        record.state_changed_ms = now;
        records.insert(id.to_string(), record.clone());
        Ok(record)
    }

    /// `trained | shadow | promoted -> retired` (owner only): the model agent leaves the roster.
    pub fn retire(&self, roster: &Roster, id: &str) -> Result<ModelRecord, String> {
        let mut records = self.records_mut();
        let current = records.get(id).ok_or_else(|| no_model(id))?;
        if current.state == State::Retired {
            return Err(format!("{id} is already retired"));
        }
        let mut record = current.clone();
        if let Some(cid) = record.agent_cid {
            roster.remove(cid);
        }
        self.loaded.remove(id);
        record.state = State::Retired;
        record.state_changed_ms = apz_plaza::now_ms();
        records.insert(id.to_string(), record.clone());
        Ok(record)
    }

    fn transition(&self, id: &str, from: &[State], to: State) -> Result<ModelRecord, String> {
        let mut records = self.records_mut();
        let current = records.get(id).ok_or_else(|| no_model(id))?;
        if !from.contains(&current.state) {
            return Err(format!(
                "{id} is {}: this move needs it to be {}",
                current.state.name(),
                from[0].name()
            ));
        }
        let mut record = current.clone();
        record.state = to;
        record.state_changed_ms = apz_plaza::now_ms();
        records.insert(id.to_string(), record.clone());
        Ok(record)
    }

    /// Adds one shadow comparison: whether the model agreed with the real decision, and the cost
    /// the real path had that the model would not (the model's own cost is 0).
    pub fn record_shadow(
        &self,
        id: &str,
        agreed: bool,
        cost_saved_usd: f64,
    ) -> Result<ShadowStats, String> {
        let mut records = self.records();
        let record = records.get_mut(id).ok_or_else(|| no_model(id))?;
        record.shadow.runs += 1;
        record.shadow.agreed += u64::from(agreed);
        record.shadow.cost_saved_usd += cost_saved_usd;
        record.shadow.updated_ms = apz_plaza::now_ms();
        Ok(record.shadow.clone())
    }
}

/// The roles each model's card allows, mirrored for the laws, which read them while checking a
/// call (`model.invoke`) and hold no `App`. Filled by import and revive.
static ALLOWED: Mutex<BTreeMap<String, Vec<String>>> = Mutex::new(BTreeMap::new());

/// The roles a model's card allows; empty for an unknown model.
pub fn allowed_roles(id: &str) -> Vec<String> {
    ALLOWED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(id)
        .cloned()
        .unwrap_or_default()
}

fn remember(card: &ModelCard) {
    ALLOWED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(card.id.clone(), card.allowed_roles.clone());
}

fn no_model(id: &str) -> String {
    format!("no model {id}")
}

/// The first CID of the range that neither the roster nor a record holds.
fn free_cid(roster: &Roster, records: &BTreeMap<String, ModelRecord>) -> Result<u64, String> {
    (MODEL_CID_BASE..MODEL_CID_BASE + MODEL_CID_COUNT)
        .find(|cid| {
            roster.get(*cid).is_none() && !records.values().any(|r| r.agent_cid == Some(*cid))
        })
        .ok_or_else(|| {
            format!(
                "no free model CID left in {MODEL_CID_BASE}..{}",
                MODEL_CID_BASE + MODEL_CID_COUNT
            )
        })
}

/// The roster entry of a promoted model: no VM, no wallet, no tools, no chat.
pub fn model_agent(cid: u64, now_ms: u64) -> Agent {
    Agent {
        cid,
        role: MODEL_ROLE.into(),
        kind: MODEL_KIND.into(),
        caps: Vec::new(),
        model_override: None,
        status: Status::Idle,
        connected_at_ms: now_ms,
        last_seen_ms: now_ms,
        run: None,
        vm: None,
        usage: Usage::default(),
    }
}

/// The checks every registration makes beyond the card's own: a classifier or regression output
/// that is an object, feature names that the input schema declares, and the roles allowed.
fn check_serving(serving: &Serving) -> Result<(), String> {
    if serving.input.kind != "object" || serving.output.kind != "object" {
        return Err("serving: the input and output schemas must be objects".into());
    }
    if serving.features.is_empty() {
        return Err("serving: no features".into());
    }
    for feature in &serving.features {
        if !serving.input.properties.contains_key(&feature.name) {
            return Err(format!(
                "serving: feature `{}` is not an input property",
                feature.name
            ));
        }
    }
    if serving.allowed_roles.is_empty() {
        return Err("serving: allowed_roles is empty, so no role could use the model".into());
    }
    Ok(())
}

/// Why a download cannot be served, `None` when it can: a base model is for recipes; a servable one needs
/// an ONNX graph and a `model_card.json` with a serving block.
fn servable_problem(dir: &Path, kind: &str) -> Option<String> {
    if kind == "base" {
        return Some("a base model is for fine-tuning recipes, not for serving".into());
    }
    let contract = fs::read_to_string(dir.join("model_card.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|card| card.get("serving").cloned());
    match contract {
        None => Some("no feature contract: model_card.json has no serving block".into()),
        Some(serving) => match serde_json::from_value::<Serving>(serving) {
            Err(e) => Some(format!("the serving contract is not valid: {e}")),
            Ok(_) if !onnx_path(dir).exists() => {
                Some("no model.onnx (or onnx/model.onnx) in the download".into())
            }
            Ok(_) => None,
        },
    }
}

/// The text contract of a record, when it is a text model.
fn text_contract(record: &ModelRecord) -> Option<TextContract> {
    let serving = record.card.serving.as_ref()?;
    if serving.get("kind").and_then(Value::as_str) != Some("text") {
        return None;
    }
    serde_json::from_value(serving.clone()).ok()
}

/// Where a download's graph is: `model.onnx` at the top, or `onnx/model.onnx` (the Hugging Face layout).
fn onnx_path(dir: &Path) -> PathBuf {
    let top = dir.join("model.onnx");
    if top.exists() {
        top
    } else {
        dir.join("onnx").join("model.onnx")
    }
}

/// Ids name a directory: letters, digits, `-`, `_`, `.`, never a leading dot.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("renaming {}: {e}", path.display()))
}

/// The record as the routes show it: the card, the state, provenance, calibration, size, the shadow
/// agreement rate, and the source job.
pub fn view(record: &ModelRecord) -> Value {
    let mut value = serde_json::to_value(record).unwrap_or(Value::Null);
    if let Value::Object(map) = &mut value {
        map.insert(
            "agreement_rate".into(),
            json!(record.shadow.agreement_rate()),
        );
        map.insert("id".into(), json!(record.card.id));
    }
    value
}

#[derive(Deserialize)]
struct ImportBody {
    path: String,
}

/// The foundry's model routes. The gate in front of the router already admits only the owner (and
/// the machine itself) to `/v1` routes that are not public.
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/foundry/models", |app, req, _p| {
            // `kind=base`: the Hugging Face base models a recipe can fine-tune (docs/design/foundry.md).
            let base_only = match req.query_param("kind") {
                None => false,
                Some("base") => true,
                Some(other) => {
                    return Response::error(400, &format!("kind must be base, got {other:?}"));
                }
            };
            let models: Vec<Value> = app
                .models
                .list()
                .iter()
                .filter(|r| !base_only || r.source.as_ref().is_some_and(|s| s.as_ == "base"))
                .map(view)
                .collect();
            Response::json(200, &json!({ "models": models }))
        })
        .doc(RouteDoc::new("List the foundry's models").errors(&[400]))
        .get("/v1/foundry/models/{id}", |app, _req, p| {
            match app.models.get(p.get("id")) {
                Some(record) => Response::json(200, &view(&record)),
                None => Response::error(404, "no such model"),
            }
        })
        .doc(RouteDoc::new("Show a model").errors(&[404]))
        .post("/v1/foundry/models/import", |app, req, _p| {
            let body: ImportBody = match crate::agents::body(req) {
                Ok(body) => body,
                Err(resp) => return resp,
            };
            match absolute(&body.path).and_then(|dir| app.models.import(dir)) {
                Ok(record) => Response::json(200, &view(&record)),
                Err(e) => Response::error(400, &e),
            }
        })
        .doc(RouteDoc::new("Register a model from a directory").errors(&[400]))
        .post("/v1/foundry/models/{id}/shadow", |app, _req, p| {
            outcome(app.models.shadow(p.get("id")))
        })
        .doc(RouteDoc::new("Move a model to shadow").errors(&[404, 409]))
        .post("/v1/foundry/models/{id}/promote", |app, _req, p| {
            outcome(app.models.promote(&app.agents, p.get("id")))
        })
        .doc(RouteDoc::new("Promote a model to a model agent").errors(&[404, 409]))
        .post("/v1/foundry/models/{id}/invoke", |app, req, p| {
            let input: Value = match crate::agents::body(req) {
                Ok(input) => input,
                Err(resp) => return resp,
            };
            invoked(crate::foundry::breeder::invoke(app, p.get("id"), &input))
        })
        .doc(RouteDoc::new("Run a model on an input").errors(&[400, 404]))
        .post("/v1/foundry/models/{id}/retire", |app, _req, p| {
            outcome(app.models.retire(&app.agents, p.get("id")))
        })
        .doc(RouteDoc::new("Retire a model").errors(&[404, 409]))
}

/// An import's directory: an absolute path only (the upload route's rule, in its words).
fn absolute(path: &str) -> Result<&Path, String> {
    let dir = Path::new(path);
    if dir.is_absolute() {
        Ok(dir)
    } else {
        Err("path is absolute".into())
    }
}

fn outcome(result: Result<ModelRecord, String>) -> Response {
    match result {
        Ok(record) => Response::json(200, &view(&record)),
        Err(e) if e.starts_with("no model") => Response::error(404, &e),
        Err(e) => Response::error(409, &e),
    }
}

/// An invoke's answer: the output, or 404 for an unknown model (as the other model routes answer it) and 400 for
/// what the model refuses (its state, its input).
fn invoked(result: Result<(Value, u128), String>) -> Response {
    match result {
        Ok((output, ms)) => Response::json(200, &json!({ "output": output, "ms": ms })),
        Err(e) if e.starts_with("no model") => Response::error(404, &e),
        Err(e) => Response::error(400, &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_invoke_of_an_unknown_model_is_404_and_a_refused_input_or_state_is_400() {
        let unknown = invoked(Err("no model m-missing".into()));
        assert_eq!(unknown.status, 404);
        let retired = invoked(Err(
            "model m1 is retired: only shadow and promoted models run".into(),
        ));
        assert_eq!(retired.status, 400);
        let bad_input = invoked(Err("input must be a list of numbers".into()));
        assert_eq!(bad_input.status, 400);
        assert_eq!(invoked(Ok((json!(1), 0))).status, 200);
    }

    /// The fixture graph: 3 inputs to 3 classes (`tests/fixtures/linear3.onnx`).
    const LINEAR3: &[u8] = include_bytes!("../../tests/fixtures/linear3.onnx");

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apz-registry-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A job directory as the trainer leaves it, with the serving contract the registry reads.
    /// The registry as a restart leaves it: its rows written to a store, and a fresh registry over `root` loaded from it.
    fn restarted(models: &Models, root: &Path) -> Models {
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &models.rows().unwrap()).unwrap();
        let reopened = Models::open(root);
        reopened.load(&conn).unwrap();
        reopened
    }

    fn job_dir(root: &Path, job: &str, serving: Option<Value>) -> PathBuf {
        let dir = root.join(job);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("model.onnx"), LINEAR3).unwrap();
        let mut card = json!({
            "recipe": "rung_predictor",
            "job_id": job,
            "data": { "train": { "rows": 10 } },
            "metrics": { "val": { "accuracy": 0.9 } },
            "parameters": 12,
        });
        if let Some(serving) = serving {
            card["serving"] = serving;
        }
        fs::write(dir.join("model_card.json"), card.to_string()).unwrap();
        dir
    }

    fn serving() -> Value {
        let numeric = |name: &str| (name.to_string(), json!({ "type": "number" }));
        let input: BTreeMap<String, Value> = [numeric("x0"), numeric("x1"), numeric("x2")]
            .into_iter()
            .collect();
        json!({
            "input": { "type": "object", "properties": input, "required": ["x0", "x1", "x2"] },
            "output": {
                "type": "object",
                "properties": {
                    "label": { "type": "string", "enum": ["cat", "dog", "bird"] },
                    "probabilities": { "type": "object" }
                },
                "required": ["label", "probabilities"]
            },
            "features": [
                { "name": "x0", "kind": "numeric" },
                { "name": "x1", "kind": "numeric" },
                { "name": "x2", "kind": "numeric" }
            ],
            "labels": ["cat", "dog", "bird"],
            "allowed_roles": ["breeder"]
        })
    }

    fn fresh_roster() -> Roster {
        Roster::default()
    }

    #[test]
    fn a_model_moves_trained_shadow_promoted_retired_and_no_other_way() {
        let root = scratch("states");
        let models = Models::open(&root);
        let roster = fresh_roster();
        let dir = job_dir(&root, "job-1", Some(serving()));
        let record = models.register_from_job(&dir, Some("job-1")).unwrap();
        let id = record.card.id.clone();
        assert_eq!(record.state, State::Trained);
        assert_eq!(record.source_job.as_deref(), Some("job-1"));
        assert!(
            models.promote(&roster, &id).is_err(),
            "a trained model is not promoted"
        );
        assert_eq!(models.shadow(&id).unwrap().state, State::Shadow);
        assert!(models.shadow(&id).is_err(), "shadow is entered once");
        let promoted = models.promote(&roster, &id).unwrap();
        assert_eq!(promoted.state, State::Promoted);
        assert!(promoted.agent_cid.is_some());
        assert_eq!(models.retire(&roster, &id).unwrap().state, State::Retired);
        assert!(models.retire(&roster, &id).is_err(), "retired is terminal");
        assert!(models.shadow(&id).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_import_takes_an_absolute_directory_only() {
        assert_eq!(absolute("relative/job-1").unwrap_err(), "path is absolute");
        assert_eq!(absolute("").unwrap_err(), "path is absolute");
        // `/srv/...` is not absolute on Windows (no drive), so the absolute case uses the temp folder.
        let job = std::env::temp_dir().join("jobs").join("job-1");
        let text = job.to_str().expect("a UTF-8 temp folder");
        assert_eq!(absolute(text).unwrap(), job.as_path());
    }

    #[test]
    fn import_validates_the_contract_and_the_bytes() {
        let root = scratch("import");
        let models = Models::open(&root);
        let no_serving = job_dir(&root, "a", None);
        assert!(
            models
                .import(&no_serving)
                .unwrap_err()
                .contains("no serving contract")
        );

        let mut bad_feature = serving();
        bad_feature["features"][0]["name"] = json!("nope");
        let bad = job_dir(&root, "b", Some(bad_feature));
        assert!(
            models
                .import(&bad)
                .unwrap_err()
                .contains("not an input property")
        );

        let missing = job_dir(&root, "c", Some(serving()));
        fs::remove_file(missing.join("model.onnx")).unwrap();
        assert!(models.import(&missing).unwrap_err().contains("model.onnx"));

        let good = job_dir(&root, "d", Some(serving()));
        let record = models.import(&good).unwrap();
        assert_eq!(record.card.onnx_sha256, sha256_hex(LINEAR3));
        assert!(
            models
                .import(&good)
                .unwrap_err()
                .contains("already registered")
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_model_agent_appears_on_promote_and_leaves_on_retire() {
        let root = scratch("agent");
        let models = Models::open(&root);
        let roster = fresh_roster();
        let id = models
            .register_from_job(&job_dir(&root, "job-3", Some(serving())), None)
            .unwrap()
            .card
            .id;
        models.shadow(&id).unwrap();
        let cid = models.promote(&roster, &id).unwrap().agent_cid.unwrap();
        assert!(is_model_cid(cid));
        let agent = roster.get(cid).expect("the model agent is on the roster");
        assert_eq!(agent.kind, "model");
        assert_eq!(agent.role, "model");
        assert!(agent.vm.is_none() && agent.run.is_none());
        models.retire(&roster, &id).unwrap();
        assert!(
            roster.get(cid).is_none(),
            "retiring removes the model agent"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_promoted_model_comes_back_on_the_roster_after_a_restart() {
        let root = scratch("revive");
        let models = Models::open(&root);
        let roster = fresh_roster();
        let id = models
            .register_from_job(&job_dir(&root, "job-4", Some(serving())), None)
            .unwrap()
            .card
            .id;
        models.shadow(&id).unwrap();
        let cid = models.promote(&roster, &id).unwrap().agent_cid.unwrap();
        let reopened = restarted(&models, &root);
        let roster2 = fresh_roster();
        reopened.revive(&roster2);
        assert_eq!(roster2.get(cid).map(|a| a.kind), Some("model".to_string()));
        assert!(reopened.loaded(&id).is_some());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_rows_restore_cards_states_and_shadow_stats() {
        let root = scratch("rows");
        let models = Models::open(&root);
        let roster = fresh_roster();
        let dir = job_dir(&root, "job-s", Some(serving()));
        let id = models
            .register_from_job(&dir, Some("job-s"))
            .unwrap()
            .card
            .id;
        models.shadow(&id).unwrap();
        models.record_shadow(&id, true, 0.5).unwrap();
        models.record_shadow(&id, false, 0.25).unwrap();
        models.promote(&roster, &id).unwrap();

        let other_root = scratch("rows-other");
        let other = restarted(&models, &other_root);
        assert_eq!(other.list(), models.list());
        let stats = other.get(&id).unwrap().shadow;
        assert_eq!((stats.runs, stats.agreed), (2, 1));
        // A store with no rows loads an empty registry.
        let empty = Models::open(&other_root);
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        empty.load(&conn).unwrap();
        assert!(empty.list().is_empty());
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&other_root);
    }

    /// The list survives a restart through the store: its rows are loaded into a fresh registry, and the
    /// promoted record comes back with its shadow stats, parameters and provenance.
    #[test]
    fn the_model_list_survives_a_restart_through_the_store() {
        let root = scratch("list-restart");
        let models = Models::open(&root);
        let roster = fresh_roster();
        let id = models
            .register_from_job(&job_dir(&root, "job-r", Some(serving())), Some("job-r"))
            .unwrap()
            .card
            .id;
        models.shadow(&id).unwrap();
        models.record_shadow(&id, true, 0.5).unwrap();
        models.promote(&roster, &id).unwrap();
        let before = models.list();
        let reopened = restarted(&models, &root);
        assert_eq!(reopened.list(), before);
        assert_eq!(reopened.list().len(), 1);
        let record = reopened.get(&id).expect("the record is in the store");
        assert_eq!(record.state, State::Promoted);
        assert_eq!(record.shadow.runs, 1);
        assert_eq!(record.parameters, 12);
        assert_eq!(record.provenance["train"]["rows"], 10);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_hugging_face_download_without_a_contract_registers_as_imported_and_once() {
        let dir = std::env::temp_dir().join(format!("apid-hf-register-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let download = dir.join("download");
        fs::create_dir_all(download.join("onnx")).unwrap();
        fs::write(download.join("onnx").join("model.onnx"), b"graph").unwrap();
        fs::write(download.join("config.json"), b"{}").unwrap();
        let models = Models::open(&dir.join("state"));
        let source = Source {
            hf: "org/x".into(),
            revision: "a".repeat(40),
            as_: "servable".into(),
            path: download.display().to_string(),
            servable_reason: None,
            task: None,
        };
        let record = models
            .register_hf(&download, source.clone(), "hf-org-x-aaaaaaaa", 5)
            .unwrap();
        assert_eq!(record.state, State::Imported);
        let reason = record
            .source
            .as_ref()
            .and_then(|s| s.servable_reason.clone())
            .unwrap_or_default();
        assert!(reason.contains("model_card.json"), "{reason}");
        // The same repo and revision registers once.
        let again = models
            .register_hf(&download, source, "hf-org-x-aaaaaaaa", 5)
            .unwrap();
        assert_eq!(again.card.id, record.card.id);
        assert_eq!(models.list().len(), 1);
        // An imported model is never compiled or promoted: shadow needs a trained model.
        assert!(models.shadow(&record.card.id).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hugging_face_text_model_registers_as_trained_with_its_contract_and_runs() {
        let dir = std::env::temp_dir().join(format!("apid-hf-text-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let download = dir.join("download");
        fs::create_dir_all(download.join("onnx")).unwrap();
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/text");
        fs::copy(
            format!("{fixture}/classifier.onnx"),
            download.join("onnx").join("model_quantized.onnx"),
        )
        .unwrap();
        for name in ["config.json", "tokenizer.json"] {
            fs::copy(format!("{fixture}/{name}"), download.join(name)).unwrap();
        }
        let models = Models::open(&dir.join("state"));
        let source = Source {
            hf: "org/sst".into(),
            revision: "b".repeat(40),
            as_: "servable".into(),
            path: download.display().to_string(),
            servable_reason: None,
            task: Some("text-classification".into()),
        };
        let record = models
            .register_hf(&download, source.clone(), "hf-org-sst-bbbbbbbb", 1)
            .unwrap();
        assert_eq!(record.state, State::Trained);
        assert_eq!(
            record
                .card
                .serving
                .as_ref()
                .and_then(|s| s["kind"].as_str()),
            Some("text")
        );
        let model = models
            .text(&record.card.id)
            .expect("the text model is loaded");
        let out = model
            .invoke(&json!({ "text": "good great movie" }))
            .unwrap();
        assert_eq!(out["label"], "POSITIVE");
        // The same repo and revision registers once, and the contract is in the download's card.
        let again = models
            .register_hf(&download, source, "hf-org-sst-bbbbbbbb", 1)
            .unwrap();
        assert_eq!(again.card.id, record.card.id);
        let card: Value =
            serde_json::from_str(&fs::read_to_string(download.join("model_card.json")).unwrap())
                .unwrap();
        assert_eq!(card["serving"]["max_len"], 8);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rederived_import_becomes_a_text_model_under_the_same_id() {
        let dir = std::env::temp_dir().join(format!("apid-hf-rederive-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let download = dir.join("download");
        fs::create_dir_all(download.join("onnx")).unwrap();
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/text");
        fs::copy(
            format!("{fixture}/config.json"),
            download.join("config.json"),
        )
        .unwrap();
        let models = Models::open(&dir.join("state"));
        let source = Source {
            hf: "org/old".into(),
            revision: "c".repeat(40),
            as_: "servable".into(),
            path: download.display().to_string(),
            servable_reason: None,
            task: None,
        };
        // An import from before the text runtime: no graph, no tokenizer, no recorded task.
        let old = models
            .register_hf(&download, source, "hf-org-old-cccccccc", 1)
            .unwrap();
        assert_eq!(old.state, State::Imported);
        // The files are there now: the graph and the tokenizer.
        fs::copy(
            format!("{fixture}/classifier.onnx"),
            download.join("onnx").join("model.onnx"),
        )
        .unwrap();
        fs::copy(
            format!("{fixture}/tokenizer.json"),
            download.join("tokenizer.json"),
        )
        .unwrap();
        // Re-derived with the task the lookup found: a trained text model under the same id, and it runs.
        let upgraded = models
            .rederive("hf-org-old-cccccccc", Some("text-classification".into()))
            .unwrap();
        assert_eq!(upgraded.state, State::Trained);
        assert_eq!(upgraded.card.id, "hf-org-old-cccccccc");
        let out = models
            .text(&upgraded.card.id)
            .expect("the text model is loaded")
            .invoke(&json!({ "text": "good great movie" }))
            .unwrap();
        assert_eq!(out["label"], "POSITIVE");
        // Only imported models are re-derived.
        assert!(models.rederive("hf-org-old-cccccccc", None).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn model_cids_are_the_reserved_range_only() {
        assert!(!is_model_cid(9_999));
        assert!(is_model_cid(10_000));
        assert!(is_model_cid(10_999));
        assert!(!is_model_cid(11_000));
        assert!(!is_model_cid(299));
    }

    #[test]
    fn an_id_cannot_leave_the_store_directory() {
        assert!(!valid_id("../etc"));
        assert!(!valid_id(".hidden"));
        assert!(valid_id("rung_predictor-1a2b3c4d"));
    }

    /// The laws refuse what the brief names: the breeder's permit and cap, the card's roles, and the
    /// model agents' no-task and no-message rules.
    fn serving_for(roles: &[&str]) -> Value {
        let mut value = serving();
        value["allowed_roles"] = json!(roles);
        value
    }

    fn law_refusal(cid: u64, role: &str, tool: &str, input: &Value) -> Option<String> {
        crate::laws::Laws::builtin().refusal(cid, role, tool, input)
    }

    #[test]
    fn invoke_model_runs_only_for_a_breeder_and_only_where_the_card_allows() {
        let root = scratch("laws-invoke");
        let models = Models::open(&root);
        // The bytes differ from the other tests' models: the id is the ONNX digest, and the roles
        // mirror is keyed by it, so a shared id would let parallel tests overwrite each other.
        let reviewers = job_dir(&root, "job-reviewers", Some(serving_for(&["reviewer"])));
        fs::write(
            reviewers.join("model.onnx"),
            include_bytes!("../../tests/fixtures/linear3_logits.onnx"),
        )
        .unwrap();
        let only_reviewers = models.register_from_job(&reviewers, None).unwrap().card.id;
        let for_breeders = models
            .register_from_job(
                &job_dir(&root, "job-breeders", Some(serving_for(&["breeder"]))),
                None,
            )
            .unwrap()
            .card
            .id;
        let input = |id: &str| json!({ "model": id, "input": { "x0": 1.0, "x1": 0.0, "x2": 0.0 } });
        let refused =
            law_refusal(7, "builder", "invoke_model", &input(&for_breeders)).expect("no permit");
        assert!(
            refused.contains("S-006h") || refused.contains("no law permits"),
            "{refused}"
        );
        let card_refused =
            law_refusal(7, "breeder", "invoke_model", &input(&only_reviewers)).expect("the card");
        assert!(card_refused.contains("S-006h"), "{card_refused}");
        assert_eq!(
            law_refusal(7, "breeder", "invoke_model", &input(&for_breeders)),
            None
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_breeder_job_over_eight_gpu_hours_is_refused_and_eight_is_not() {
        let root = scratch("laws-gpu");
        let big = json!({ "job": { "budget_gpu_hours": 9.0 } });
        let refused = law_refusal(8, "breeder", "foundry_submit_job", &big).expect("over 8 hours");
        assert!(refused.contains("S-006d"), "{refused}");
        let eight = json!({ "job": { "budget_gpu_hours": 8.0 } });
        assert_eq!(
            law_refusal(8, "breeder", "foundry_submit_job", &eight),
            None
        );
        let missing = json!({ "job": {} });
        assert!(
            law_refusal(8, "breeder", "foundry_submit_job", &missing).is_some(),
            "no budget: refused"
        );
        let other =
            law_refusal(9, "builder", "foundry_submit_job", &eight).expect("only breeders submit");
        assert!(other.contains("no law permits"), "{other}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_model_agent_cannot_be_tasked_or_messaged() {
        let root = scratch("laws-model-agent");
        let assign = law_refusal(
            3,
            "builder",
            "plaza_assign",
            &json!({ "task": "t1", "agent": 10_000 }),
        )
        .expect("a model agent is not tasked");
        assert!(assign.contains("S-006f"), "{assign}");
        let message = law_refusal(
            3,
            "builder",
            "plaza_send_message",
            &json!({ "to_cid": 10_500, "text": "hi" }),
        )
        .expect("a model agent is not messaged");
        assert!(message.contains("S-006g"), "{message}");
        let plain = law_refusal(
            3,
            "builder",
            "plaza_assign",
            &json!({ "task": "t1", "agent": 42 }),
        );
        assert!(
            plain.is_none_or(|r| !r.contains("S-006f")),
            "an ordinary agent is not caught by S-006f"
        );
        let _ = fs::remove_dir_all(&root);
    }
}
