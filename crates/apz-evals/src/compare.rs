//! The comparison with a baseline: a task regresses when it passed in the baseline and does not now, or when its tool
//! calls, model turns, tokens or dollars grew past the tolerance. Latencies are reported, never compared: they are
//! the machine's time, not the harness's work.

use crate::report::{Report, TaskReport};

/// The regressions of `current` against `baseline`, one line each; none when the run is no worse.
pub fn regressions(baseline: &Report, current: &Report, tolerance: f64) -> Vec<String> {
    let mut found = Vec::new();
    for base in &baseline.tasks {
        let Some(now) = current.tasks.iter().find(|t| t.task == base.task) else {
            found.push(format!("{}: not in this run", base.task));
            continue;
        };
        if base.outcome == "pass" && now.outcome != "pass" {
            found.push(format!(
                "{}: the outcome went from pass to {} ({})",
                base.task, now.outcome, now.state
            ));
        }
        let grew = |what: &str, was: f64, is: f64| {
            (is > was * (1.0 + tolerance) + 1e-9)
                .then(|| format!("{}: {what} grew from {was} to {is}", base.task))
        };
        found.extend(grew(
            "tool calls",
            base.tool_calls as f64,
            now.tool_calls as f64,
        ));
        found.extend(grew(
            "model turns",
            base.model_turns as f64,
            now.model_turns as f64,
        ));
        found.extend(grew(
            "tokens",
            (base.tokens.input + base.tokens.output) as f64,
            (now.tokens.input + now.tokens.output) as f64,
        ));
        found.extend(grew("dollars", base.dollars, now.dollars));
    }
    found
}

/// The comparison as a table: each task's calls, turns, tokens, dollars and p95 of its busiest tool, beside the
/// baseline's.
pub fn table(baseline: &Report, current: &Report) -> String {
    let mut out = format!(
        "{:<18} {:<8} {:>13} {:>13} {:>17} {:>10}\n",
        "task", "outcome", "calls (base)", "turns (base)", "tokens (base)", "p95 ms"
    );
    for now in &current.tasks {
        let base = baseline.tasks.iter().find(|t| t.task == now.task);
        out.push_str(&format!(
            "{:<18} {:<8} {:>6} ({:>4}) {:>6} ({:>4}) {:>8} ({:>6}) {:>10}\n",
            now.task,
            now.outcome,
            now.tool_calls,
            base.map_or(0, |b| b.tool_calls),
            now.model_turns,
            base.map_or(0, |b| b.model_turns),
            now.tokens.input + now.tokens.output,
            base.map_or(0, |b| b.tokens.input + b.tokens.output),
            busiest_p95(now),
        ));
    }
    out
}

fn busiest_p95(task: &TaskReport) -> u64 {
    task.tools
        .iter()
        .max_by_key(|tool| tool.calls)
        .map_or(0, |tool| tool.p95_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{Tokens, ToolLatency};

    fn task(name: &str, outcome: &str, calls: u64, turns: u64, tokens: u64) -> TaskReport {
        TaskReport {
            task: name.into(),
            outcome: outcome.into(),
            state: "done".into(),
            tool_calls: calls,
            model_turns: turns,
            tokens: Tokens {
                input: tokens,
                output: 0,
                cached: 0,
            },
            tools: vec![ToolLatency {
                tool: "run".into(),
                calls,
                errors: 0,
                p50_ms: 1,
                p95_ms: 9,
            }],
            ..TaskReport::default()
        }
    }

    fn report(tasks: Vec<TaskReport>) -> Report {
        Report {
            model: "mock".into(),
            tasks,
        }
    }

    #[test]
    fn an_identical_run_has_no_regressions_and_growth_past_the_tolerance_is_one() {
        let base = report(vec![task("a", "pass", 4, 3, 1000)]);
        assert!(regressions(&base, &base, 0.0).is_empty());
        // Ten percent over is within a ten percent tolerance; eleven is not.
        let ok = report(vec![task("a", "pass", 4, 3, 1100)]);
        assert!(regressions(&base, &ok, 0.10).is_empty());
        let worse = report(vec![task("a", "pass", 5, 3, 1200)]);
        let found = regressions(&base, &worse, 0.10);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(
            found[0].contains("tool calls grew from 4 to 5"),
            "{found:?}"
        );
        assert!(found[1].contains("tokens grew"), "{found:?}");
    }

    #[test]
    fn a_task_that_stops_passing_or_goes_missing_regresses() {
        let base = report(vec![task("a", "pass", 1, 1, 1), task("b", "pass", 1, 1, 1)]);
        let now = report(vec![task("a", "fail", 1, 1, 1)]);
        let found = regressions(&base, &now, 0.10);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].contains("from pass to fail"), "{found:?}");
        assert!(found[1].contains("b: not in this run"), "{found:?}");
        // A task that did not pass in the baseline may pass or fail now.
        let base = report(vec![task("a", "fail", 1, 1, 1)]);
        assert!(regressions(&base, &report(vec![task("a", "timeout", 1, 1, 1)]), 0.1).is_empty());
    }

    #[test]
    fn the_table_shows_each_task_beside_its_baseline() {
        let base = report(vec![task("a", "pass", 4, 3, 1000)]);
        let now = report(vec![task("a", "pass", 5, 3, 1000)]);
        let text = table(&base, &now);
        assert!(text.contains("a") && text.contains("5 (   4)"), "{text}");
    }
}
