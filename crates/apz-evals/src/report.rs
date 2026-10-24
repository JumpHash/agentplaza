//! The suite's report: what each task did, as JSON (the baseline is one) and as a table.

use serde::{Deserialize, Serialize};

/// The whole run: the model the seats ran and each task's report, in the tasks' order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub model: String,
    pub tasks: Vec<TaskReport>,
}

/// One task: its outcome and what its runs cost, summed over the runs of its project.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TaskReport {
    pub task: String,
    /// `pass` (the task is done and its check exits 0), `fail` (done and the check fails, or stopped short) or
    /// `timeout` (not done in time).
    pub outcome: String,
    /// The task's state when the run stopped.
    pub state: String,
    /// The check's exit status, when it ran.
    pub check_exit: Option<i32>,
    /// The step runs (sessions) the task made.
    pub runs: u64,
    pub tool_calls: u64,
    /// The model's turns (its calls).
    pub model_turns: u64,
    pub tokens: Tokens,
    pub dollars: f64,
    /// The runs' wall time and their tools' time, in milliseconds.
    pub wall_ms: u64,
    pub tool_ms: u64,
    /// Each tool's calls, errors and latency (p50, p95) in milliseconds.
    pub tools: Vec<ToolLatency>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cached: u64,
}

/// A tool's latency as the hub measured it (see `GET /v1/evals/tools`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolLatency {
    pub tool: String,
    pub calls: u64,
    pub errors: u64,
    pub p50_ms: u64,
    pub p95_ms: u64,
}

/// The report as a table: one row per task.
pub fn table(report: &Report) -> String {
    let mut out = format!(
        "{:<18} {:<8} {:>6} {:>6} {:>10} {:>10} {:>10} {:>9}\n",
        "task", "outcome", "calls", "turns", "tokens in", "tokens out", "dollars", "wall ms"
    );
    for task in &report.tasks {
        out.push_str(&format!(
            "{:<18} {:<8} {:>6} {:>6} {:>10} {:>10} {:>10.4} {:>9}\n",
            task.task,
            task.outcome,
            task.tool_calls,
            task.model_turns,
            task.tokens.input,
            task.tokens.output,
            task.dollars,
            task.wall_ms,
        ));
    }
    out
}
