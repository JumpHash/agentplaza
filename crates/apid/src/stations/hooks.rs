//! The hook every workstation action passes after it runs (`act::agent_op`). Recordings (per lease: the screenshots
//! and the actions, with timestamps) and the treasury (lease and GPU time charged to the task) hook in here. Until
//! they do, it does nothing.

use apz_station::Op;
use serde_json::Value;

use super::ops::Failure;
use crate::state::App;

/// Called after every operation an agent or the owner sends to a station, with its station, its lease (if any), the
/// operation and its outcome, whether it ran or was refused.
pub fn after_agent_op(app: &App, station: &str, lease: Option<&str>, op: &Op, result: &Result<Value, Failure>) {
    let _ = (app, station, lease, op, result);
}
