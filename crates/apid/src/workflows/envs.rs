//! The ephemeral environments a workflow calls (PLAN 23): an instance a `deploy` step makes short-lived, the checks an
//! `against` step runs against its URL, and its teardown.
//!
//! The calls go through [`Envs`], so the walker's tests give a fake. The production implementation, [`HubEnvs`], is the
//! hub's ephemeral environments; until those land on the hub, its calls refuse with a message that says so. The types
//! here mirror the ephemeral module's (`crate::ephemeral`) field for field, so that the calls can be made by the hub's
//! functions without a conversion.

use serde::{Deserialize, Serialize};

/// What an ephemeral instance is for: a change's lane, or a release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Subject {
    Lane(String),
    Release(String),
}

/// A request to bring up an ephemeral instance of a project's environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpRequest {
    pub project: String,
    pub environment: String,
    pub subject: Subject,
    pub ttl_minutes: Option<u32>,
}

/// An ephemeral instance: what it is, where it serves, and when it expires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    pub id: String,
    pub project: String,
    pub environment: String,
    pub subject: Subject,
    pub target: String,
    pub url: Option<String>,
    pub state: String,
    pub created_ms: u64,
    pub expires_ms: u64,
}

/// The calls a workflow makes to ephemeral environments.
pub trait Envs: Sync {
    /// Brings an instance up.
    fn up(&self, req: &UpRequest) -> Result<Instance, String>;
    /// Takes an instance down.
    fn down(&self, instance: &Instance) -> Result<(), String>;
    /// Runs `command` against the instance's URL (its checks and tests); the output on success, the reason on failure.
    fn against(&self, instance: &Instance, command: &str) -> Result<String, String>;
}

/// The hub's ephemeral environments. Not wired yet: each call refuses, and says so.
pub struct HubEnvs;

/// Why the hub's ephemeral environments are not there yet.
pub const NOT_WIRED: &str = "not yet wired: ephemeral environments land on their own branch";

impl Envs for HubEnvs {
    fn up(&self, req: &UpRequest) -> Result<Instance, String> {
        let _ = req;
        Err(NOT_WIRED.to_string())
    }

    fn down(&self, instance: &Instance) -> Result<(), String> {
        let _ = instance;
        Err(NOT_WIRED.to_string())
    }

    fn against(&self, instance: &Instance, command: &str) -> Result<String, String> {
        let _ = (instance, command);
        Err(NOT_WIRED.to_string())
    }
}
