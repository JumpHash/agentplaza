//! Operator-spawned agents and every agent's model override, kept in plaza.db (the `spawned` tables, saved by the
//! store's saver). A restart respawns each spawned agent under its old CID, so its workspace lines up.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::agents::Status;
use crate::state::App;

/// One agent the operator spawned. `cid` is its identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct Entry {
    pub cid: u64,
    pub role: String,
    #[serde(default)]
    pub model: Option<String>,
}

/// The registry: the spawned agents, and the model overrides keyed by CID. One document, so one row of `spawned`.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "spawned")]
pub struct Registry {
    #[serde(default)]
    #[table(child = "spawned_entry")]
    pub spawned: Vec<Entry>,
    #[serde(default)]
    #[table(map_value = "spawned_model")]
    pub models: BTreeMap<u64, String>,
}

/// A spawned run still waiting for its first hello: its CID when noded has named it already (a fork), its
/// role and model override.
#[derive(Debug, Clone)]
struct Pending {
    cid: u64,
    role: String,
    model: Option<String>,
}

/// What a hello brings: the agent's model override (its work comes from its session, not from its spawn).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Arrival {
    pub model: Option<String>,
}

/// The registry, plus the spawns still waiting for their hello.
#[derive(Default)]
pub struct Store {
    registry: Mutex<Registry>,
    /// One per spawned run, matched by role in arrival order.
    pending: Mutex<VecDeque<Pending>>,
    /// The role each node booted a guest CID with from its fleet manifest, as noded reports it in its runs
    /// (refreshed by the reconcile pass; not saved: noded reports it again after a restart).
    fleet: Mutex<BTreeMap<u64, String>>,
}

impl Store {
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

    /// Noded booted a spawn (an operator's run, or a fork) as guest `cid`: that guest's first hello is this
    /// spawn, matched by its CID and never by the role a guest claims.
    pub fn expect_cid(&self, cid: u64, role: &str, model: Option<String>) {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(Pending {
                cid,
                role: role.to_owned(),
                model,
            });
    }

    /// A session said hello. A first hello of a pending spawn is recorded; returns the model override to apply
    /// to the agent.
    pub fn hello(&self, cid: u64, _claimed_role: &str) -> Arrival {
        let known = self.guard().spawned.iter().any(|entry| entry.cid == cid);
        if !known {
            let claimed = {
                let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
                // The spawn noded named with this CID; the role in the hello plays no part.
                let index = pending.iter().position(|wanted| wanted.cid == cid);
                index.and_then(|i| pending.remove(i))
            };
            if let Some(Pending {
                role: spawned_role,
                model,
                ..
            }) = claimed
            {
                // The plaza's role for the spawn, never the role the guest claims in its hello.
                let entry = Entry {
                    cid,
                    role: spawned_role,
                    model: model.clone(),
                };
                self.mutate(|registry| {
                    registry.spawned.push(entry);
                    if let Some(model) = &model {
                        registry.models.insert(cid, model.clone());
                    }
                });
            }
        }
        Arrival {
            model: self.model_for(cid),
        }
    }

    /// Sets (or clears) the model override of `cid`, and its spawned entry's model. The saver writes it on its next
    /// pass; the result is always `Ok` (the callers report a failure of the change, which no longer happens here).
    pub fn set_model(&self, cid: u64, model: Option<&str>) -> std::io::Result<()> {
        self.mutate(|registry| {
            match model {
                Some(model) => registry.models.insert(cid, model.to_owned()),
                None => registry.models.remove(&cid),
            };
            for entry in registry.spawned.iter_mut().filter(|entry| entry.cid == cid) {
                entry.model = model.map(str::to_owned);
            }
        });
        Ok(())
    }

    /// Forgets agent `cid` (it was deleted): no respawn, no override. As [`Store::set_model`], always `Ok`.
    pub fn forget(&self, cid: u64) -> std::io::Result<()> {
        self.mutate(|registry| {
            registry.spawned.retain(|entry| entry.cid != cid);
            registry.models.remove(&cid);
        });
        Ok(())
    }

    /// Applies `change` to the registry in memory.
    fn mutate(&self, change: impl FnOnce(&mut Registry)) {
        change(&mut self.guard());
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
    /// The registry's rows, as it is now (the saver writes what changed).
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        RowSet::one(def(), &*self.guard())
    }

    /// Replaces the registry with what plaza.db holds (at start).
    pub fn load(&self, conn: &Connection) -> Result<(), StoreError> {
        let stored = apz_store::table::load_one::<Registry>(conn, def())?;
        *self.guard() = stored.unwrap_or_default();
        Ok(())
    }
}

/// The registry's tables: `spawned`, with its spawned agents and model overrides as child tables.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(Registry)
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;

    /// A registry saved to its tables and loaded into a fresh store is the same registry.
    #[test]
    fn the_registry_comes_back_whole_from_its_tables() {
        let store = Store::default();
        store.expect_cid(42, "worker", Some("model-a".into()));
        store.hello(42, "worker");
        store.set_model(7, Some("model-b")).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &store.rows().unwrap()).unwrap();

        let other = Store::default();
        other.load(&conn).unwrap();
        assert_eq!(*other.guard(), *store.guard());
        assert_eq!(other.model_for(42).as_deref(), Some("model-a"));
        assert_eq!(other.model_for(7).as_deref(), Some("model-b"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fleet_role_comes_from_noded_and_a_spawn_wins() {
        let store = Store::default();
        // A guest's own claim never gives it a role; noded's report does.
        store.hello(200, "orchestrator");
        assert_eq!(store.role_of(200), None);
        assert!(store.note_fleet_role(200, "architect"));
        assert!(!store.note_fleet_role(200, "architect"));
        assert_eq!(store.role_of(200).as_deref(), Some("architect"));
        // An operator spawn's recorded role wins over the fleet's.
        store.expect_cid(201, "builder", None);
        store.hello(201, "builder");
        store.note_fleet_role(201, "qa");
        assert_eq!(store.role_of(201).as_deref(), Some("builder"));
    }

    #[test]
    fn a_spawn_brings_its_model_override_on_the_first_hello_only() {
        let store = Store::default();
        store.expect_cid(301, "builder", Some("openai/gpt-6".into()));
        store.expect_cid(300, "qa", None);
        assert_eq!(
            store.hello(300, "qa"),
            Arrival::default(),
            "a spawn without a model override brings none"
        );
        assert_eq!(
            store.hello(301, "builder").model.as_deref(),
            Some("openai/gpt-6")
        );
        assert_eq!(
            store.hello(301, "builder").model.as_deref(),
            Some("openai/gpt-6"),
            "a reconnect keeps the model it was given"
        );
        assert_eq!(store.role_of(301).as_deref(), Some("builder"));
    }

    #[test]
    fn a_fork_is_matched_by_its_cid_and_a_role_never_takes_it() {
        let store = Store::default();
        store.expect_cid(501, "builder", Some("model-b".into()));
        assert_eq!(
            store.hello(502, "builder"),
            Arrival::default(),
            "another builder's hello does not take a fork that was named by CID"
        );
        assert_eq!(
            store.hello(501, "builder").model.as_deref(),
            Some("model-b"),
            "the fork's own hello takes its spawn"
        );
        // A hello that names another CID, or claims any role, takes no pending spawn.
        store.expect_cid(503, "orchestrator", Some("model-c".into()));
        assert_eq!(
            store.hello(504, "orchestrator"),
            Arrival::default(),
            "a guest claiming the pending role with another CID gets nothing"
        );
        assert_eq!(store.role_of(504), None);
        assert_eq!(
            store.hello(503, "builder").model.as_deref(),
            Some("model-c"),
            "the spawn's own CID takes it whatever role its hello claims"
        );
        assert_eq!(store.role_of(503).as_deref(), Some("orchestrator"));
    }

    /// The registry is kept in its tables: a spawn, an override and a forget come back from the store as they were.
    #[test]
    fn registry_survives_a_round_trip() {
        let store = Store::default();
        store.expect_cid(42, "worker", Some("model-a".into()));
        assert_eq!(store.hello(42, "worker").model.as_deref(), Some("model-a"));
        store.set_model(7, Some("model-b")).unwrap();
        let saved = |store: &Store| {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute_batch(&def().ddl()).unwrap();
            apz_store::table::replace(&conn, def(), &store.rows().unwrap()).unwrap();
            let reloaded = Store::default();
            reloaded.load(&conn).unwrap();
            reloaded
        };

        let reloaded = saved(&store);
        assert_eq!(
            reloaded.entries(),
            vec![Entry {
                cid: 42,
                role: "worker".into(),
                model: Some("model-a".into()),
            }]
        );
        assert_eq!(reloaded.model_for(7).as_deref(), Some("model-b"));

        reloaded.forget(42).unwrap();
        let after = saved(&reloaded);
        assert!(after.entries().is_empty());
        assert_eq!(after.model_for(42), None);
        assert_eq!(after.model_for(7).as_deref(), Some("model-b"));
    }
}
