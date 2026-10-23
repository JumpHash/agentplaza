//! Base builds on landings (PLAN 16.4): a landing on a project's main starts the base recipe (base_recipe.rs) for each
//! pool the project runs in. Builds coalesce: one build per project and pool at a time. A landing during a build marks
//! it stale, and when it ends the next build starts at once from the newest main, so a burst of merges costs one or
//! two builds, not one each. Sessions fork the newest finished base ([`Builds::newest_finished`]).
//!
//! The state is in memory: a restart forgets the finished bases, and the next landing builds again.
//!
//! The recipe's commands run in the base's VM, through the base runner (base_vm.rs): noded starts the VM, its harness
//! answers the exec items, and the snapshot holds the base on the node that built it.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::base_recipe::{self, BaseKind};
use crate::base_vm::{Live, Runner};
use crate::placement;
use crate::project_stack::{self, FILE, Stack};
use crate::state::App;

/// The pool key of a project that declares no pools: its one build serves any pool.
const ANY_POOL: &str = "any";

/// One build's key: a project and a pool (`aarch64-kvm`, or `any`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub project: String,
    pub pool: String,
}

/// What the scheduler knows about one key.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Slot {
    /// A build is running.
    running: bool,
    /// A landing came while the build ran: the next build starts when it ends.
    stale: bool,
    /// The commit of the newest base that finished.
    finished: Option<String>,
}

/// Every key's builds: whether one runs, whether a landing waits, and the newest finished base.
#[derive(Debug, Default)]
pub struct Builds {
    slots: BTreeMap<Key, Slot>,
}

impl Builds {
    /// A landing: true when the caller starts a build now. False when a build is running, which the landing marks stale.
    pub fn landed(&mut self, key: &Key) -> bool {
        let slot = self.slots.entry(key.clone()).or_default();
        if slot.running {
            slot.stale = true;
            false
        } else {
            slot.running = true;
            true
        }
    }

    /// A build ended with `built`, the commit it built (a failed build keeps the newest finished base). True when a
    /// landing came meanwhile: the caller builds again, from the newest main.
    pub fn finished(&mut self, key: &Key, built: Result<String, String>) -> bool {
        let slot = self.slots.entry(key.clone()).or_default();
        if let Ok(commit) = built {
            slot.finished = Some(commit);
        }
        if slot.stale {
            slot.stale = false;
            true
        } else {
            slot.running = false;
            false
        }
    }

    /// The commit of the newest base that finished for `key`.
    // Sessions fork it once they are placed on a base (3.5).
    #[allow(dead_code)]
    pub fn newest_finished(&self, key: &Key) -> Option<&str> {
        self.slots
            .get(key)
            .and_then(|slot| slot.finished.as_deref())
    }
}

/// The builds of the running hub.
static BUILDS: Mutex<Builds> = Mutex::new(Builds {
    slots: BTreeMap::new(),
});

fn lock() -> MutexGuard<'static, Builds> {
    BUILDS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Runs `build` for `key` until no landing is waiting. `build` runs without the lock held, and its result goes to
/// [`Builds::finished`].
pub fn run_until_quiet(
    state: &Mutex<Builds>,
    key: &Key,
    mut build: impl FnMut() -> Result<String, String>,
) {
    loop {
        let built = build();
        let again = state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .finished(key, built);
        if !again {
            return;
        }
    }
}

/// A landing on `repo`'s main: starts a build for each pool of each project on the repo that declares a stack.
pub fn landed(app: &Arc<App>, repo: &str) {
    for project in app
        .projects
        .list()
        .into_iter()
        .filter(|p| p.repo.as_deref() == Some(repo))
    {
        let stack = match project_stack::read(app, &project.id) {
            Ok(Some(stack)) => stack,
            Ok(None) => continue,
            Err(why) => {
                eprintln!("apid: base of {}: {why}", project.id);
                continue;
            }
        };
        for pool in pool_keys(&stack) {
            let key = Key {
                project: project.id.clone(),
                pool,
            };
            if !lock().landed(&key) {
                continue;
            }
            let (app, repo) = (Arc::clone(app), repo.to_string());
            std::thread::spawn(move || {
                run_until_quiet(&BUILDS, &key, || {
                    let built = build_once(&app, &key, &repo);
                    if let Err(why) = &built {
                        eprintln!("apid: base of {} ({}): {why}", key.project, key.pool);
                    }
                    built
                })
            });
        }
    }
}

/// One build: the newest main's commit, the declaration on main, and the recipe's steps run in the base's VM. Returns
/// the commit built.
fn build_once(app: &App, key: &Key, repo: &str) -> Result<String, String> {
    let detail = app.weft.get(&format!("/repos/{repo}"))?;
    let commit = detail["refs"]["main"]
        .as_str()
        .ok_or_else(|| format!("{repo} has no main"))?
        .to_string();
    let stack =
        project_stack::read(app, &key.project)?.ok_or_else(|| format!("{FILE} is not on main"))?;
    let steps = base_recipe::steps(&stack, BaseKind::Build, repo, &commit);
    let node = placement::base_node(app, &key.project, &key.pool, &base_recipe::volumes(&stack))
        .ok_or_else(|| format!("no node in the pool {} has room for a base", key.pool))?;
    let host = Live(app);
    let mut runner = Runner::new(&host, node, key.project.clone());
    base_recipe::execute(&mut runner, &steps)?;
    Ok(commit)
}

/// The pool keys a stack's base is built for: one per declared pool (`aarch64-kvm`), or [`ANY_POOL`] when it declares
/// none.
pub fn pool_keys(stack: &Stack) -> Vec<String> {
    let mut keys: Vec<String> = stack
        .pools
        .iter()
        .map(|pool| format!("{}-{}", pool.arch.name(), pool.hypervisor.name()))
        .collect();
    if keys.is_empty() {
        return vec![ANY_POOL.to_string()];
    }
    keys.sort();
    keys.dedup();
    keys
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::project_stack::parse;

    fn key(project: &str, pool: &str) -> Key {
        Key {
            project: project.into(),
            pool: pool.into(),
        }
    }

    #[test]
    fn a_burst_of_five_landings_costs_two_builds_and_the_second_starts_from_the_newest_main() {
        let state = Mutex::new(Builds::default());
        let k = key("p", "aarch64-kvm");
        let main = Cell::new(1);
        assert!(
            state.lock().unwrap().landed(&k),
            "the first landing starts a build"
        );
        let mut built = Vec::new();
        run_until_quiet(&state, &k, || {
            let commit = format!("c{}", main.get());
            if built.is_empty() {
                // Four more landings arrive while the first build runs.
                for _ in 0..4 {
                    main.set(main.get() + 1);
                    assert!(!state.lock().unwrap().landed(&k));
                }
            }
            built.push(commit.clone());
            Ok(commit)
        });
        assert_eq!(
            built,
            ["c1", "c5"],
            "the second build reads the newest main"
        );
        assert_eq!(state.lock().unwrap().newest_finished(&k), Some("c5"));
        assert!(
            state.lock().unwrap().landed(&k),
            "the queue is empty once the second build ends"
        );
    }

    #[test]
    fn a_failed_build_keeps_the_newest_finished_base() {
        let state = Mutex::new(Builds::default());
        let k = key("p", "any");
        assert!(state.lock().unwrap().landed(&k));
        run_until_quiet(&state, &k, || Ok("c1".into()));
        assert!(state.lock().unwrap().landed(&k));
        run_until_quiet(&state, &k, || Err("exit 1".into()));
        assert_eq!(state.lock().unwrap().newest_finished(&k), Some("c1"));
        assert!(
            state.lock().unwrap().landed(&k),
            "a failed build is not still running"
        );
    }

    #[test]
    fn one_build_runs_per_project_and_pool() {
        let mut builds = Builds::default();
        assert!(builds.landed(&key("p", "aarch64-kvm")));
        assert!(!builds.landed(&key("p", "aarch64-kvm")));
        assert!(
            builds.landed(&key("p", "x86_64-kvm")),
            "another pool builds at once"
        );
        assert!(
            builds.landed(&key("q", "aarch64-kvm")),
            "another project builds at once"
        );
    }

    #[test]
    fn pool_keys_are_one_per_declared_pool_and_any_when_none_is_declared() {
        let stack = parse(
            "[[pools]]\narch = \"aarch64\"\nhypervisor = \"kvm\"\n\n[[pools]]\narch = \"x86_64\"\nhypervisor = \"kvm\"\n\n[[pools]]\narch = \"aarch64\"\nhypervisor = \"kvm\"\n",
        )
        .unwrap();
        assert_eq!(pool_keys(&stack), ["aarch64-kvm", "x86_64-kvm"]);
        assert_eq!(pool_keys(&parse("").unwrap()), ["any"]);
    }
}
