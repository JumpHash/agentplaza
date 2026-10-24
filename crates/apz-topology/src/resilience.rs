//! Resilience scenarios (PLAN 22.8): an action on a service while a probe judges the system from outside. The driver
//! probes the scenario's URL on a fixed interval from the start of the run, applies the action at `at_secs`, lifts it
//! after `for_secs`, and keeps probing until the recovery deadline. [`measure`] turns the probes into numbers (the share
//! that failed while the action held, the 95th percentile latency, the time to recover), [`judge`] compares them with
//! what the spec allows, and [`otlp`] records them at the run as gauges, which the hub reads back and judges again.
//!
//! What the action does is the [`Actuator`]'s business: the fault proxies of [`crate::fault`] in front of a dependency,
//! the lab VM's traffic shaping, or a process and a clock in the guest ([`crate::steps`]). An action the guest cannot
//! perform is `not run`, with its reason, and is never counted as passed.

use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};

use crate::compare::Metric;
use crate::load::percentile;
use crate::spec::{Probe, Scenario};

/// Probes that must succeed in a row before the system counts as recovered.
pub const RECOVERY_STREAK: usize = 3;
/// The interval between probes.
pub const PROBE_EVERY: Duration = Duration::from_millis(50);

/// When a scenario's action starts, holds and is lifted, and how often the probe runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schedule {
    pub at: Duration,
    pub hold: Duration,
    pub recover: Duration,
    pub every: Duration,
}

impl Schedule {
    /// The schedule a scenario's file names.
    pub fn of(scenario: &Scenario) -> Schedule {
        Schedule {
            at: Duration::from_secs(scenario.at_secs),
            hold: Duration::from_secs(scenario.for_secs),
            recover: Duration::from_secs(scenario.recover_within_secs),
            every: PROBE_EVERY,
        }
    }
}

/// One probe: when it was sent (ms from the run's start), its latency, and whether it failed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Probed {
    pub at_ms: u64,
    pub latency_ms: f64,
    pub failed: bool,
}

/// What the probes measured: the probes that were sent while the action held and their failures, the 95th percentile
/// latency of those, and how long after the action was lifted the system answered [`RECOVERY_STREAK`] probes in a row.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Measured {
    pub requests: u64,
    pub failed: u64,
    pub error_percent: f64,
    pub p95_ms: f64,
    pub recovered_secs: Option<f64>,
}

/// The numbers of `probes`: the window of the action is `[at_ms, heal_ms)`, and the system must recover by
/// `deadline_ms`. With no probe in the window the error share is 100 (nothing was observed, so nothing passed).
pub fn measure(probes: &[Probed], at_ms: u64, heal_ms: u64, deadline_ms: u64) -> Measured {
    let window: Vec<&Probed> = probes
        .iter()
        .filter(|p| p.at_ms >= at_ms && p.at_ms < heal_ms)
        .collect();
    let requests = window.len() as u64;
    let failed = window.iter().filter(|p| p.failed).count() as u64;
    let error_percent = if requests == 0 {
        100.0
    } else {
        failed as f64 * 100.0 / requests as f64
    };
    let mut latencies: Vec<f64> = window.iter().map(|p| p.latency_ms).collect();
    latencies.sort_by(f64::total_cmp);

    let mut recovered_secs = None;
    let mut streak = 0;
    let mut streak_from = 0;
    for probe in probes
        .iter()
        .filter(|p| p.at_ms >= heal_ms && p.at_ms <= deadline_ms)
    {
        if probe.failed {
            streak = 0;
            continue;
        }
        if streak == 0 {
            streak_from = probe.at_ms;
        }
        streak += 1;
        if streak == RECOVERY_STREAK {
            recovered_secs = Some((streak_from - heal_ms) as f64 / 1000.0);
            break;
        }
    }
    Measured {
        requests,
        failed,
        error_percent,
        p95_ms: percentile(&latencies, 95.0),
        recovered_secs,
    }
}

/// Whether a scenario's measurements are as the spec allows, and the sentence that says so.
#[derive(Debug, Clone, PartialEq)]
pub struct Judgment {
    pub passed: bool,
    pub detail: String,
}

/// Judges `measured` against the scenario's expected degradation and its recovery deadline. The detail carries the
/// numbers, so a finding that cites it says what was seen.
pub fn judge(scenario: &Scenario, measured: &Measured) -> Judgment {
    let expect = &scenario.expect;
    let mut seen = Vec::new();
    let mut problems = Vec::new();
    if measured.requests == 0 {
        problems.push("no probe ran while the action held".to_string());
    } else {
        seen.push(format!(
            "{:.1} % of {} probes failed while it held (the spec allows {:.1} %)",
            measured.error_percent, measured.requests, expect.max_error_percent
        ));
        if measured.error_percent > expect.max_error_percent {
            problems.push(format!(
                "{:.1} % failed, over the {:.1} % the spec allows",
                measured.error_percent, expect.max_error_percent
            ));
        }
    }
    if let Some(limit) = expect.max_p95_ms {
        seen.push(format!("p95 {:.1} ms (limit {limit} ms)", measured.p95_ms));
        if measured.p95_ms > limit as f64 {
            problems.push(format!("p95 {:.1} ms is over {limit} ms", measured.p95_ms));
        }
    }
    let deadline = scenario.recover_within_secs;
    match measured.recovered_secs {
        Some(secs) if secs <= deadline as f64 => {
            seen.push(format!("recovered in {secs:.2} s (deadline {deadline} s)"));
        }
        _ => problems.push(format!("did not recover within {deadline} s")),
    }
    let passed = problems.is_empty();
    let mut detail = format!("{}: {}", scenario.id, seen.join("; "));
    if !passed {
        detail.push_str(&format!(". Not as the spec says: {}", problems.join("; ")));
    }
    Judgment { passed, detail }
}

/// The gauge names a scenario's measurements are recorded under at the run.
pub fn names(id: &str) -> [String; 5] {
    [
        format!("resilience.{id}.requests"),
        format!("resilience.{id}.failed"),
        format!("resilience.{id}.error_percent"),
        format!("resilience.{id}.p95_ms"),
        format!("resilience.{id}.recovered_secs"),
    ]
}

/// The measurements as OTLP/JSON gauges of `apz-fault`, stamped `now_ms`. A recovery that did not come is -1.
pub fn otlp(id: &str, measured: &Measured, now_ms: u64) -> Value {
    let nanos = (now_ms as u128 * 1_000_000).to_string();
    let values = [
        measured.requests as f64,
        measured.failed as f64,
        measured.error_percent,
        measured.p95_ms,
        measured.recovered_secs.unwrap_or(-1.0),
    ];
    let metrics: Vec<Value> = names(id)
        .iter()
        .zip(values)
        .map(|(name, value)| {
            json!({"name": name, "gauge": {"dataPoints": [
                {"timeUnixNano": nanos, "asDouble": value}
            ]}})
        })
        .collect();
    json!({"resourceMetrics": [{
        "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "apz-fault"}}]},
        "scopeMetrics": [{"metrics": metrics}],
    }]})
}

/// The measurements a run recorded for scenario `id`, from its values (the metric names of [`names`]); None when any
/// of the four is missing, which is a scenario that did not run.
pub fn from_metrics(id: &str, values: &[Metric]) -> Option<Measured> {
    let value = |name: &str| values.iter().find(|m| m.name == name).map(|m| m.value);
    let [requests, failed, error, p95, recovered] = names(id);
    let recovered = value(&recovered)?;
    Some(Measured {
        requests: value(&requests)? as u64,
        failed: value(&failed)? as u64,
        error_percent: value(&error)?,
        p95_ms: value(&p95)?,
        recovered_secs: (recovered >= 0.0).then_some(recovered),
    })
}

/// The probe's URL: the service's base URL and the scenario's path.
pub fn probe_url(base: &str, probe: &Probe) -> String {
    format!("{}{}", base.trim_end_matches('/'), probe.path)
}

/// What an action does to the system under test. `available` says before the run whether the action can run in this
/// guest (a tool, a privilege, a volume); `act` starts the action (`on`) or lifts it.
pub trait Actuator {
    fn available(&self, scenario: &Scenario) -> Result<(), String>;
    fn act(&mut self, scenario: &Scenario, on: bool) -> Result<(), String>;
}

/// The outcome of one scenario: what its probes measured, or why it did not run here.
#[derive(Debug, Clone, PartialEq)]
pub enum Ran {
    Measured(Measured),
    NotRun(String),
}

/// Runs a scenario: probes `url` every `schedule.every` from the start until the recovery deadline, applies the action at
/// `schedule.at`, lifts it after `schedule.hold`, and measures what the probes saw.
pub fn run(scenario: &Scenario, url: &str, schedule: Schedule, actuator: &mut dyn Actuator) -> Ran {
    if let Err(why) = actuator.available(scenario) {
        return Ran::NotRun(why);
    }
    let start = Instant::now();
    let end = schedule.at + schedule.hold + schedule.recover;
    let (url, every) = (url.to_string(), schedule.every);
    let prober = thread::spawn(move || probe(&url, start, end, every));

    let heal = schedule.at + schedule.hold;
    sleep_until(start, schedule.at);
    let injected = actuator.act(scenario, true);
    sleep_until(start, heal);
    let lifted = actuator.act(scenario, false);
    let probes = prober.join().unwrap_or_default();
    if let Err(why) = injected.and(lifted) {
        return Ran::NotRun(why);
    }
    let at_ms = schedule.at.as_millis() as u64;
    let heal_ms = heal.as_millis() as u64;
    let deadline_ms = (heal + schedule.recover).as_millis() as u64;
    Ran::Measured(measure(&probes, at_ms, heal_ms, deadline_ms))
}

fn sleep_until(start: Instant, offset: Duration) {
    let due = start + offset;
    let now = Instant::now();
    if due > now {
        thread::sleep(due - now);
    }
}

/// Sends GETs to `url` at each tick from `start` until `end`, and records each one.
fn probe(url: &str, start: Instant, end: Duration, every: Duration) -> Vec<Probed> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(2)))
        .http_status_as_error(false)
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let mut probes = Vec::new();
    let mut tick = Duration::ZERO;
    while tick < end {
        sleep_until(start, tick);
        let sent = Instant::now();
        let failed = match agent.get(url).call() {
            Ok(response) => response.status().as_u16() >= 400,
            Err(_) => true,
        };
        probes.push(Probed {
            at_ms: start.elapsed().as_millis() as u64,
            latency_ms: sent.elapsed().as_secs_f64() * 1000.0,
            failed,
        });
        tick += every;
    }
    probes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{Action, Expect};

    fn scenario(max_error: f64, max_p95: Option<u64>, recover: u64) -> Scenario {
        Scenario {
            id: "cache-outage".into(),
            action: Action::Fault,
            target: "cache".into(),
            fault: Some(crate::fault::Fault::Partition),
            volume: None,
            leave_mib: 0,
            secs: None,
            at_secs: 2,
            for_secs: 5,
            recover_within_secs: recover,
            probe: Probe {
                service: "api".into(),
                path: "/items".into(),
            },
            expect: Expect {
                max_error_percent: max_error,
                max_p95_ms: max_p95,
            },
        }
    }

    fn probe_at(at_ms: u64, failed: bool, latency_ms: f64) -> Probed {
        Probed {
            at_ms,
            latency_ms,
            failed,
        }
    }

    #[test]
    fn measure_counts_the_window_and_finds_the_recovery_streak() {
        // Heal at 1000 ms, deadline at 3000 ms. Two of four probes fail in the window.
        let probes = [
            probe_at(0, false, 5.0),
            probe_at(500, true, 9.0),
            probe_at(800, false, 7.0),
            probe_at(900, true, 11.0),
            // Recovery: a failure resets the streak; three in a row count.
            probe_at(1100, true, 30.0),
            probe_at(1200, false, 4.0),
            probe_at(1300, false, 4.0),
            probe_at(1400, false, 4.0),
            probe_at(1500, false, 4.0),
        ];
        let measured = measure(&probes, 0, 1000, 3000);
        assert_eq!(measured.requests, 4);
        assert_eq!(measured.failed, 2);
        assert_eq!(measured.error_percent, 50.0);
        assert_eq!(measured.recovered_secs, Some(0.2));
    }

    #[test]
    fn a_window_with_no_probe_and_a_system_that_never_recovers_are_measured_as_such() {
        let none = measure(&[], 0, 1000, 3000);
        assert_eq!((none.requests, none.error_percent), (0, 100.0));
        let stuck = [probe_at(1100, true, 0.0), probe_at(1200, false, 0.0)];
        assert_eq!(measure(&stuck, 0, 1000, 3000).recovered_secs, None);
    }

    #[test]
    fn judge_fails_an_error_share_over_the_spec_and_names_the_numbers() {
        let s = scenario(5.0, Some(500), 10);
        let m = Measured {
            requests: 30,
            failed: 10,
            error_percent: 33.333,
            p95_ms: 12.0,
            recovered_secs: Some(0.15),
        };
        let judgment = judge(&s, &m);
        assert!(!judgment.passed);
        assert_eq!(
            judgment.detail,
            "cache-outage: 33.3 % of 30 probes failed while it held (the spec allows 5.0 %); p95 12.0 ms (limit 500 ms); recovered in 0.15 s (deadline 10 s). Not as the spec says: 33.3 % failed, over the 5.0 % the spec allows"
        );
    }

    #[test]
    fn judge_passes_degradation_within_the_spec_and_a_recovery_in_time() {
        let s = scenario(5.0, Some(500), 10);
        let m = Measured {
            requests: 30,
            failed: 1,
            error_percent: 3.3,
            p95_ms: 400.0,
            recovered_secs: Some(0.5),
        };
        assert!(judge(&s, &m).passed);
        let late = Measured {
            recovered_secs: None,
            ..m.clone()
        };
        let judgment = judge(&s, &late);
        assert!(!judgment.passed && judgment.detail.contains("did not recover within 10 s"));
        let slow = Measured { p95_ms: 900.0, ..m };
        assert!(
            judge(&s, &slow)
                .detail
                .contains("p95 900.0 ms is over 500 ms")
        );
    }

    #[test]
    fn the_recorded_numbers_come_back_as_the_measurement() {
        let m = Measured {
            requests: 30,
            failed: 10,
            error_percent: 33.5,
            p95_ms: 12.5,
            recovered_secs: Some(0.25),
        };
        let body = otlp("cache-outage", &m, 1_700_000_000_000);
        let points: Vec<Metric> = body["resourceMetrics"][0]["scopeMetrics"][0]["metrics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|metric| Metric {
                name: metric["name"].as_str().unwrap().to_string(),
                value: metric["gauge"]["dataPoints"][0]["asDouble"]
                    .as_f64()
                    .unwrap(),
            })
            .collect();
        assert_eq!(from_metrics("cache-outage", &points), Some(m));
        assert_eq!(from_metrics("other", &points), None);
        let stuck = Measured {
            requests: 1,
            failed: 1,
            error_percent: 100.0,
            p95_ms: 0.0,
            recovered_secs: None,
        };
        let body = otlp("cache-outage", &stuck, 0);
        assert_eq!(
            body["resourceMetrics"][0]["scopeMetrics"][0]["metrics"][4]["gauge"]["dataPoints"][0]["asDouble"],
            -1.0
        );
    }

    #[test]
    fn a_probe_url_joins_the_service_base_and_the_path() {
        let probe = Probe {
            service: "api".into(),
            path: "/items".into(),
        };
        assert_eq!(
            probe_url("http://10.88.0.3:8080/", &probe),
            "http://10.88.0.3:8080/items"
        );
    }

    /// An actuator that refuses to run, to check the scenario is not run rather than passed.
    struct Absent;

    impl Actuator for Absent {
        fn available(&self, _: &Scenario) -> Result<(), String> {
            Err("no tc in this guest".into())
        }
        fn act(&mut self, _: &Scenario, _: bool) -> Result<(), String> {
            unreachable!("an unavailable action is never applied")
        }
    }

    #[test]
    fn an_action_the_guest_cannot_perform_is_not_run_with_its_reason() {
        let s = scenario(5.0, None, 1);
        let schedule = Schedule {
            at: Duration::ZERO,
            hold: Duration::from_millis(10),
            recover: Duration::from_millis(10),
            every: Duration::from_millis(5),
        };
        assert_eq!(
            run(&s, "http://127.0.0.1:9/items", schedule, &mut Absent),
            Ran::NotRun("no tc in this guest".into())
        );
    }

    #[test]
    fn the_schedule_comes_from_the_scenario_in_seconds() {
        let s = scenario(5.0, None, 10);
        let schedule = Schedule::of(&s);
        assert_eq!(schedule.at, Duration::from_secs(2));
        assert_eq!(schedule.hold, Duration::from_secs(5));
        assert_eq!(schedule.recover, Duration::from_secs(10));
    }
}
