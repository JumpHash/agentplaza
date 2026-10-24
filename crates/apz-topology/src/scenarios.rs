//! The resilience run of a topology (PLAN 22.8): its scenarios run one after another against a guest's processes, each
//! from its own time zero. The runner that started the topology supplies the guest ([`crate::steps::Guest`] over its
//! processes), and the service URLs the probes reach; [`run_with`] runs them, [`report`] says what each did, and
//! [`record`] posts the measurements to the run's collector relay, where the hub judges them.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::collector::Relay;
use crate::resilience::{self, Ran};
use crate::spec::{Action, Scenario};
use crate::steps::{Guest, Host, Processes, Runner};

/// What one scenario did: its id, and what it measured or why it did not run.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub id: String,
    pub ran: Ran,
}

/// Runs `scenarios` in order against `guest`. `urls` gives the base URL of each service the probes reach. A fault on a
/// dependency with no proxy, or an action the guest cannot perform, is not run, with its reason.
pub fn run_with<H: Host, R: Runner, P: Processes>(
    scenarios: &[Scenario],
    urls: &BTreeMap<String, String>,
    guest: &mut Guest<H, R, P>,
) -> Vec<Outcome> {
    scenarios
        .iter()
        .map(|scenario| {
            let ran = match urls.get(&scenario.probe.service) {
                Some(base) => {
                    let url = resilience::probe_url(base, &scenario.probe);
                    resilience::run(scenario, &url, resilience::Schedule::of(scenario), guest)
                }
                None => Ran::NotRun(format!(
                    "no URL for the probed service {}",
                    scenario.probe.service
                )),
            };
            Outcome {
                id: scenario.id.clone(),
                ran,
            }
        })
        .collect()
}

/// What each scenario did, as JSON: its measurement and its judgment, or the reason it did not run.
pub fn report(scenarios: &[Scenario], outcomes: &[Outcome]) -> Value {
    Value::Array(
        outcomes
            .iter()
            .map(|outcome| {
                let scenario = scenarios.iter().find(|s| s.id == outcome.id);
                match (&outcome.ran, scenario) {
                    (Ran::Measured(measured), Some(scenario)) => {
                        let judgment = resilience::judge(scenario, measured);
                        json!({"id": outcome.id, "action": action_name(scenario.action), "measured": measured, "passed": judgment.passed, "detail": judgment.detail})
                    }
                    (Ran::Measured(measured), None) => json!({"id": outcome.id, "measured": measured}),
                    (Ran::NotRun(reason), _) => json!({"id": outcome.id, "not_run": reason}),
                }
            })
            .collect(),
    )
}

fn action_name(action: Action) -> &'static str {
    match action {
        Action::Fault => "fault",
        Action::Kill => "kill",
        Action::Restart => "restart",
        Action::FillDisk => "fill_disk",
        Action::ClockSkew => "clock_skew",
        Action::StopNode => "stop_node",
    }
}

/// Posts each measurement to the run's collector as gauges (the hub judges them against the topology's expectations).
/// Returns how many scenarios were recorded; the ones that did not run are not posted.
pub fn record(relay: &Relay, outcomes: &[Outcome]) -> Result<usize, String> {
    let mut recorded = 0;
    for outcome in outcomes {
        if let Ran::Measured(measured) = &outcome.ran {
            let body =
                resilience::otlp(&outcome.id, measured, crate::collector::now_ms()).to_string();
            relay.post("metrics", body.as_bytes()).map_err(|why| {
                format!(
                    "the measurements of {} were not recorded at the run: {why}",
                    outcome.id
                )
            })?;
            recorded += 1;
        }
    }
    Ok(recorded)
}
