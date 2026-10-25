//! The ephemeral environments a workflow calls (PLAN 21, 23): an instance a `deploy` step makes short-lived, the checks an
//! `against` step runs against its address, and its teardown. The instances are the hub's ephemeral environments
//! (`crate::ephemeral`), kept in plaza.db: a run finds its instances there, so a teardown after a restart still finds them.
//!
//! [`Hub`] is the one implementation. Production gives it the hub's store and its production ports (the platforms, the
//! connectors and the builds); the tests give it the same store with fake ports, so a run's deploy, check and teardown go
//! through the same code the hub runs.

use crate::ephemeral::against::{self, Verdict};
use crate::ephemeral::{Instance, Ports, Store, UpRequest, down_with, up_with};

/// The calls a workflow makes to ephemeral environments.
pub trait Envs: Sync {
    /// Brings an instance up (the request names the run that holds it and whether the run keeps it).
    fn up(&self, req: &UpRequest) -> Result<Instance, String>;
    /// Every instance a run brought up, oldest first (torn down or not).
    fn held(&self, run: &str) -> Vec<Instance>;
    /// Takes an instance down by its id.
    fn down(&self, instance: &str) -> Result<(), String>;
    /// Runs `tests` against the instance's address (the environment's paths are probed first, and the tests are called
    /// with the address). The instance is left up: its run takes it down.
    fn against(
        &self,
        instance: &Instance,
        tests: &mut dyn FnMut(&str) -> Result<String, String>,
    ) -> Verdict;
}

/// The hub's ephemeral environments over a store and ports. Production: `Hub::of(app)`.
pub struct Hub<'a> {
    pub store: &'a Store,
    pub ports: &'a dyn Ports,
}

impl Envs for Hub<'_> {
    fn up(&self, req: &UpRequest) -> Result<Instance, String> {
        up_with(self.store, self.ports, req.clone())
    }

    fn held(&self, run: &str) -> Vec<Instance> {
        self.store.for_run(run)
    }

    fn down(&self, instance: &str) -> Result<(), String> {
        down_with(self.store, self.ports, instance)
    }

    fn against(
        &self,
        instance: &Instance,
        tests: &mut dyn FnMut(&str) -> Result<String, String>,
    ) -> Verdict {
        let paths = self
            .ports
            .environment(&instance.project, &instance.environment)
            .map(|env| against::paths_of(&env))
            .unwrap_or_else(|_| vec!["/".to_string()]);
        let req = against::Request {
            project: instance.project.clone(),
            environment: instance.environment.clone(),
            subject: instance.subject.clone(),
            paths,
            keep: true,
            ttl_minutes: None,
        };
        against::run(self.store, self.ports, &req, Ok(instance.clone()), tests)
    }
}
