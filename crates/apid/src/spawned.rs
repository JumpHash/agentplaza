//! Operator-spawned agents and every agent's model override, kept at
//! `<state-dir>/spawned.json` (written atomically). A restart respawns each
//! spawned agent under its old CID, so its name, wallet and workspace line up.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::agents::Status;
use crate::state::App;

/// One agent the operator spawned. `cid` is its identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub cid: u64,
    pub role: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub name: String,
}

/// The file: the spawned agents, and the model overrides keyed by CID.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub spawned: Vec<Entry>,
    #[serde(default)]
    pub models: BTreeMap<u64, String>,
}

/// A spawned run still waiting for its first hello: its role, model override and project.
#[derive(Debug, Clone)]
struct Pending {
    role: String,
    model: Option<String>,
    project: Option<String>,
}

/// What a hello brings: the agent's model override, and on a pending spawn's first hello, the project it
/// was spawned for.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Arrival {
    pub model: Option<String>,
    pub project: Option<String>,
}

/// The registry and its file, plus the spawns still waiting for their hello.
pub struct Store {
    path: PathBuf,
    registry: Mutex<Registry>,
    /// One per spawned run, matched by role in arrival order.
    pending: Mutex<VecDeque<Pending>>,
    /// The role each node booted a guest CID with from its fleet manifest, as noded reports it in its runs
    /// (refreshed by the reconcile pass; not saved: noded reports it again after a restart).
    fleet: Mutex<BTreeMap<u64, String>>,
}

impl Store {
    /// Loads the file; a missing or unreadable file starts empty.
    pub fn load(path: PathBuf) -> Self {
        let registry = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            path,
            registry: Mutex::new(registry),
            pending: Mutex::new(VecDeque::new()),
            fleet: Mutex::new(BTreeMap::new()),
        }
    }

    /// Every spawned agent, in the order they were spawned.
    pub fn entries(&self) -> Vec<Entry> {
        self.guard().spawned.clone()
    }

    /// The model override of `cid`, if any.
    pub fn model_for(&self, cid: u64) -> Option<String> {
        self.guard().models.get(&cid).cloned()
    }

    /// The role the plaza spawned agent `cid` with, if the plaza spawned it: an operator spawn's role, else the
    /// role its node booted it with (`note_fleet_role`). Never the role a guest claims.
    pub fn role_of(&self, cid: u64) -> Option<String> {
        let spawned = self
            .entries()
            .into_iter()
            .find(|entry| entry.cid == cid)
            .map(|entry| entry.role);
        spawned.or_else(|| {
            self.fleet
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&cid)
                .cloned()
        })
    }

    /// Noded reports guest `cid` running with `role` (its fleet manifest's role). Returns whether that is new.
    pub fn note_fleet_role(&self, cid: u64, role: &str) -> bool {
        let mut fleet = self.fleet.lock().unwrap_or_else(PoisonError::into_inner);
        fleet.insert(cid, role.to_owned()).as_deref() != Some(role)
    }

    /// The operator asked noded for one more run of `role`; its first hello of that role is it. A
    /// `project` is the one its agent joins at that hello.
    pub fn expect(&self, role: &str, model: Option<String>, project: Option<String>) {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(Pending {
                role: role.to_owned(),
                model,
                project,
            });
    }

    /// A session said hello. A first hello of a pending spawn is recorded; returns the model
    /// override to apply to the agent, and the project it was spawned for (on that first hello only).
    pub fn hello(&self, cid: u64, role: &str) -> Arrival {
        let known = self.guard().spawned.iter().any(|entry| entry.cid == cid);
        let mut project = None;
        if !known {
            let claimed = {
                let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
                let index = pending.iter().position(|wanted| wanted.role == role);
                index.and_then(|i| pending.remove(i))
            };
            if let Some(Pending {
                model,
                project: wanted,
                ..
            }) = claimed
            {
                project = wanted;
                let entry = Entry {
                    cid,
                    role: role.to_owned(),
                    model: model.clone(),
                    name: String::new(),
                };
                let saved = self.mutate(|registry| {
                    registry.spawned.push(entry);
                    if let Some(model) = &model {
                        registry.models.insert(cid, model.clone());
                    }
                });
                if let Err(e) = saved {
                    eprintln!("apid: spawned.json not saved: {e}");
                }
            }
        }
        Arrival {
            model: self.model_for(cid),
            project,
        }
    }

    /// Sets (or clears) the model override of `cid`, and its spawned entry's model.
    pub fn set_model(&self, cid: u64, model: Option<&str>) -> std::io::Result<()> {
        self.mutate(|registry| {
            match model {
                Some(model) => registry.models.insert(cid, model.to_owned()),
                None => registry.models.remove(&cid),
            };
            for entry in registry.spawned.iter_mut().filter(|entry| entry.cid == cid) {
                entry.model = model.map(str::to_owned);
            }
        })
    }

    /// Sets the name of spawned agent `cid`.
    pub fn set_name(&self, cid: u64, name: &str) -> std::io::Result<()> {
        self.mutate(|registry| {
            for entry in registry.spawned.iter_mut().filter(|entry| entry.cid == cid) {
                entry.name = name.to_owned();
            }
        })
    }

    /// Forgets agent `cid` (it was deleted): no respawn, no override.
    pub fn forget(&self, cid: u64) -> std::io::Result<()> {
        self.mutate(|registry| {
            registry.spawned.retain(|entry| entry.cid != cid);
            registry.models.remove(&cid);
        })
    }

    /// Applies `change` to a copy, writes the copy atomically, then keeps it.
    fn mutate(&self, change: impl FnOnce(&mut Registry)) -> std::io::Result<()> {
        let mut current = self.guard();
        let mut next = current.clone();
        change(&mut next);
        let text = serde_json::to_string_pretty(&next).map_err(std::io::Error::other)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.path)?;
        *current = next;
        Ok(())
    }

    fn guard(&self) -> MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Once noded answers, waits 15 s for the sessions to reconnect, then respawns
/// each persisted agent that has no live session, pinned to its old CID. The
/// model override comes back at the agent's hello.
pub fn spawn_respawn(app: Arc<App>) {
    std::thread::spawn(move || {
        if app.spawned.entries().is_empty() {
            return;
        }
        while !matches!(app.noded.get("/v1/machine"), Ok((200, _))) {
            std::thread::sleep(Duration::from_secs(2));
        }
        std::thread::sleep(Duration::from_secs(15));
        for entry in app.spawned.entries() {
            if app
                .agents
                .get(entry.cid)
                .is_some_and(|agent| agent.status != Status::Offline)
            {
                continue;
            }
            let payload = json!({"n": 1, "role": entry.role, "vsock_cid": entry.cid, "vcpus": 1, "memory_mib": 1024});
            let model = entry.model.as_deref().unwrap_or("role default");
            match crate::fleet::noded_for(&app, entry.cid, "POST", "/v1/runs", Some(&payload)) {
                Ok((status, _)) if status < 300 => eprintln!(
                    "apid: respawned {} as cid {} (model {model}, HTTP {status})",
                    entry.role, entry.cid
                ),
                Ok((status, _)) => {
                    eprintln!("apid: respawn of cid {} refused: HTTP {status}", entry.cid)
                }
                Err(e) => eprintln!("apid: respawn of cid {} failed: {e}", entry.cid),
            }
        }
    });
}

impl Store {
    /// The whole state as JSON, for Postgres (`crate::db`); "" when there is nothing to save.
    pub fn snapshot(&self) -> String {
        serde_json::to_string_pretty(&*self.guard()).unwrap_or_default()
    }

    /// Replaces the state with a [`Store::snapshot`]; a refusal changes nothing.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let registry: Registry = serde_json::from_str(json).map_err(|e| e.to_string())?;
        self.mutate(|current| *current = registry)
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;

    #[test]
    fn snapshot_restores_into_a_fresh_store() {
        let dir = std::env::temp_dir().join(format!("apid-spawned-snap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::load(dir.join("spawned.json"));
        store.expect("worker", Some("model-a".into()), None);
        store.hello(42, "worker");
        store.set_name(42, "Ada").unwrap();
        let json = store.snapshot();

        let other = Store::load(dir.join("other.json"));
        other.restore(&json).unwrap();
        assert_eq!(other.snapshot(), json);
        assert_eq!(other.model_for(42).as_deref(), Some("model-a"));
        assert_eq!(other.entries()[0].name, "Ada");
        assert!(other.restore("not json").is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fleet_role_comes_from_noded_and_a_spawn_wins() {
        let dir = std::env::temp_dir().join(format!("apid-spawned-fleet-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::load(dir.join("spawned.json"));
        // A guest's own claim never gives it a role; noded's report does.
        store.hello(200, "orchestrator");
        assert_eq!(store.role_of(200), None);
        assert!(store.note_fleet_role(200, "architect"));
        assert!(!store.note_fleet_role(200, "architect"));
        assert_eq!(store.role_of(200).as_deref(), Some("architect"));
        // An operator spawn's recorded role wins over the fleet's.
        store.expect("builder", None, None);
        store.hello(201, "builder");
        store.note_fleet_role(201, "qa");
        assert_eq!(store.role_of(201).as_deref(), Some("builder"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_spawn_for_a_project_brings_it_on_the_first_hello_only() {
        let dir = std::env::temp_dir().join(format!("apid-spawned-project-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::load(dir.join("spawned.json"));
        store.expect("builder", None, Some("pr2".into()));
        store.expect("qa", None, None);
        assert_eq!(
            store.hello(300, "qa"),
            Arrival::default(),
            "a spawn without a project waits on the bench"
        );
        assert_eq!(store.hello(301, "builder").project.as_deref(), Some("pr2"));
        assert_eq!(
            store.hello(301, "builder").project,
            None,
            "a reconnect joins nothing again"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn registry_survives_a_round_trip() {
        let dir = std::env::temp_dir().join(format!("apid-spawned-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("spawned.json");
        let _ = std::fs::remove_file(&path);

        let store = Store::load(path.clone());
        store.expect("worker", Some("model-a".into()), None);
        assert_eq!(store.hello(42, "worker").model.as_deref(), Some("model-a"));
        store.set_name(42, "Ada").unwrap();
        store.set_model(7, Some("model-b")).unwrap();

        let reloaded = Store::load(path.clone());
        assert_eq!(
            reloaded.entries(),
            vec![Entry {
                cid: 42,
                role: "worker".into(),
                model: Some("model-a".into()),
                name: "Ada".into()
            }]
        );
        assert_eq!(reloaded.model_for(7).as_deref(), Some("model-b"));

        reloaded.forget(42).unwrap();
        let after = Store::load(path);
        assert!(after.entries().is_empty());
        assert_eq!(after.model_for(42), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
