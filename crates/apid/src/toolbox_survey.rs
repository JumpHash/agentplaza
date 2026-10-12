//! `plaza_toolbox`'s answer: the toolbox as it stands for one agent, from live state.

use crate::agents::Agent;
use crate::state::App;

/// A plain-text survey for `agent`, filtered by the words of `query` when it is not empty.
pub fn survey(_app: &App, _agent: &Agent, _query: &str) -> String {
    // contract: F-T2
    "the toolbox survey is not available yet".to_string()
}
