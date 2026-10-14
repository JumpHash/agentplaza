//! Terminals on a platform without a PTY backend yet. Every call is refused as unavailable; a Windows
//! backend (ConPTY) is a later change.

use super::{TermClosed, TermList, TermOpened, TermRead, TermResized, TermWritten};
use crate::env::Env;
use crate::error::OpError;
use crate::root::Root;

/// The terminal table. Here it refuses every call.
#[derive(Debug, Default)]
pub struct Terminals;

impl Terminals {
    /// A terminal table with no terminals.
    pub fn new() -> Self {
        Self
    }

    /// Refused: terminals are unsupported on this platform.
    pub fn open(
        &self,
        _root: &Root,
        _env: &Env,
        _cols: u64,
        _rows: u64,
        _cwd: &str,
    ) -> Result<TermOpened, OpError> {
        Err(OpError::unsupported("terminals"))
    }

    /// Refused: terminals are unsupported on this platform.
    pub fn write(&self, _term: &str, _data_b64: &str) -> Result<TermWritten, OpError> {
        Err(OpError::unsupported("terminals"))
    }

    /// Refused: terminals are unsupported on this platform.
    pub fn read(&self, _term: &str, _offset: Option<u64>) -> Result<TermRead, OpError> {
        Err(OpError::unsupported("terminals"))
    }

    /// Refused: terminals are unsupported on this platform.
    pub fn resize(&self, _term: &str, _cols: u64, _rows: u64) -> Result<TermResized, OpError> {
        Err(OpError::unsupported("terminals"))
    }

    /// Refused: terminals are unsupported on this platform.
    pub fn close(&self, _term: &str) -> Result<TermClosed, OpError> {
        Err(OpError::unsupported("terminals"))
    }

    /// Refused: terminals are unsupported on this platform.
    pub fn list(&self) -> Result<TermList, OpError> {
        Err(OpError::unsupported("terminals"))
    }
}
