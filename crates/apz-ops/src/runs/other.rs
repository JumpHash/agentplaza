//! Runs on a platform without a backend yet. Every call is refused as unavailable, and nothing starts.

use super::{RunPoll, RunStarted, RunStopped};
use crate::env::Env;
use crate::error::OpError;
use crate::root::Root;

/// The run table. Here it refuses every call.
#[derive(Debug, Default)]
pub struct Runs;

impl Runs {
    /// A run table with no runs.
    pub const fn new() -> Self {
        Self
    }

    /// Refused: runs are unsupported on this platform.
    pub fn start(
        &self,
        _root: &Root,
        _env: &Env,
        _command: &str,
        _timeout_s: Option<u64>,
    ) -> Result<RunStarted, OpError> {
        Err(OpError::unsupported("runs"))
    }

    /// Refused: runs are unsupported on this platform.
    pub fn poll(&self, _run: &str, _offset: Option<u64>) -> Result<RunPoll, OpError> {
        Err(OpError::unsupported("runs"))
    }

    /// Refused: runs are unsupported on this platform.
    pub fn stop(&self, _run: &str) -> Result<RunStopped, OpError> {
        Err(OpError::unsupported("runs"))
    }
}
