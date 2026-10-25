//! Load profiles (PLAN 22.8, traffic and performance): the request schedule a named profile drives, the generator that
//! runs one against a URL, the summary the baselines and the findings read, and the k6 and vegeta runners the lab VM
//! uses when its image has them (found on its PATH, never installed).
//!
//! A profile is named `<shape>-<n>rps`: `steady` holds n requests a second, `ramp` climbs from 0 to n over its length,
//! `spike` holds a fifth of n and bursts to n through the middle fifth of its length, and `soak` holds n for a long
//! time. The built-in generator sends each request at its scheduled time from a pool of workers and times it from the
//! moment it is sent.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The length of a profile that names none: a minute.
pub const DEFAULT_MS: u64 = 60_000;
/// The length of a soak that names none: half an hour.
pub const SOAK_MS: u64 = 30 * 60_000;
/// The workers the built-in generator starts when the run names none.
pub const WORKERS: usize = 32;
/// The metrics a summary is recorded as, in the order [`metric_points`] gives them.
pub const METRICS: [&str; 6] = [
    "load.latency.p50_ms",
    "load.latency.p95_ms",
    "load.latency.p99_ms",
    "load.latency.max_ms",
    "load.throughput_rps",
    "load.error_rate",
];

/// The shape of a profile's request rate over its length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Steady,
    Ramp,
    Spike,
    Soak,
}

/// A profile: its name, its shape, the rate it reaches (requests a second) and its length.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub name: String,
    pub shape: Shape,
    pub rps: f64,
    pub duration_ms: u64,
}

/// Parses `<shape>-<n>rps`. The length is `duration_ms` when given and positive, else the shape's default.
pub fn parse(name: &str, duration_ms: Option<u64>) -> Result<Profile, String> {
    let (shape_name, rate) = name
        .rsplit_once('-')
        .ok_or_else(|| format!("{name:?} is not a profile: <shape>-<n>rps"))?;
    let shape = match shape_name {
        "steady" => Shape::Steady,
        "ramp" => Shape::Ramp,
        "spike" => Shape::Spike,
        "soak" => Shape::Soak,
        other => {
            return Err(format!(
                "{other:?} is not a load shape (steady, ramp, spike, soak)"
            ));
        }
    };
    let rps = rate
        .strip_suffix("rps")
        .and_then(|digits| digits.parse::<f64>().ok())
        .filter(|rps| rps.is_finite() && *rps > 0.0)
        .ok_or_else(|| format!("{rate:?} is not a rate: a positive number and rps"))?;
    let duration_ms = match duration_ms {
        Some(ms) if ms > 0 => ms,
        _ if shape == Shape::Soak => SOAK_MS,
        _ => DEFAULT_MS,
    };
    Ok(Profile {
        name: name.to_string(),
        shape,
        rps,
        duration_ms,
    })
}

/// The rate (requests a second) at `t_ms` into the profile.
pub fn rate_at(profile: &Profile, t_ms: f64) -> f64 {
    let length = profile.duration_ms as f64;
    match profile.shape {
        Shape::Steady | Shape::Soak => profile.rps,
        Shape::Ramp => profile.rps * (t_ms / length).clamp(0.0, 1.0),
        Shape::Spike => {
            if (0.4 * length..0.6 * length).contains(&t_ms) {
                profile.rps
            } else {
                profile.rps / 5.0
            }
        }
    }
}

/// The offsets (ms from the start) at which the profile sends each request: the rate integrated over time, one request
/// each time a whole request is owed.
pub fn schedule(profile: &Profile) -> Vec<u64> {
    let mut out = Vec::new();
    let mut owed = 0.0_f64;
    for ms in 0..profile.duration_ms {
        owed += rate_at(profile, ms as f64 + 0.5) / 1000.0;
        while owed >= 1.0 {
            out.push(ms);
            owed -= 1.0;
        }
    }
    out
}

/// One request the generator made: its latency and whether it failed (a transport error, or a status of 400 or more).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub latency_ms: f64,
    pub failed: bool,
}

/// What a run measured: the requests, their failures, the throughput and the latency percentiles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub profile: String,
    pub target: String,
    /// `builtin`, `k6` or `vegeta`: the generator that made the run.
    pub tool: String,
    pub requests: u64,
    pub errors: u64,
    pub elapsed_ms: u64,
    pub throughput_rps: f64,
    /// Failed requests over all requests, 0 to 1.
    pub error_rate: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

/// The nearest-rank percentile `p` (0 to 100) of sorted values; 0 for none.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// The summary of a run's samples over `elapsed`.
pub fn summarize(
    profile: &Profile,
    target: &str,
    tool: &str,
    samples: &[Sample],
    elapsed: Duration,
) -> Summary {
    let mut latencies: Vec<f64> = samples.iter().map(|s| s.latency_ms).collect();
    latencies.sort_by(f64::total_cmp);
    let requests = samples.len() as u64;
    let errors = samples.iter().filter(|s| s.failed).count() as u64;
    let seconds = elapsed.as_secs_f64();
    Summary {
        profile: profile.name.clone(),
        target: target.to_string(),
        tool: tool.to_string(),
        requests,
        errors,
        elapsed_ms: elapsed.as_millis() as u64,
        throughput_rps: if seconds > 0.0 {
            requests as f64 / seconds
        } else {
            0.0
        },
        error_rate: if requests > 0 {
            errors as f64 / requests as f64
        } else {
            0.0
        },
        p50_ms: percentile(&latencies, 50.0),
        p95_ms: percentile(&latencies, 95.0),
        p99_ms: percentile(&latencies, 99.0),
        max_ms: latencies.last().copied().unwrap_or(0.0),
    }
}

/// The built-in generator: the profile's requests are sent at their offsets to `target` (GET) by `workers` threads.
pub fn run_builtin(target: &str, profile: &Profile, workers: usize) -> Summary {
    let offsets = schedule(profile);
    let (tx, rx) = mpsc::channel::<u64>();
    let rx = Arc::new(Mutex::new(rx));
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let start = Instant::now();
    let mut samples = Vec::with_capacity(offsets.len());
    thread::scope(|scope| {
        let workers: Vec<_> = (0..workers.max(1))
            .map(|_| {
                let rx = Arc::clone(&rx);
                let agent = agent.clone();
                scope.spawn(move || {
                    let mut mine = Vec::new();
                    // The lock is held only while waiting for the next offset, so the workers take turns at the queue.
                    while rx.lock().ok().and_then(|queue| queue.recv().ok()).is_some() {
                        let sent = Instant::now();
                        let failed = match agent.get(target).call() {
                            Ok(response) => response.status().as_u16() >= 400,
                            Err(_) => true,
                        };
                        mine.push(Sample {
                            latency_ms: sent.elapsed().as_secs_f64() * 1000.0,
                            failed,
                        });
                    }
                    mine
                })
            })
            .collect();
        for offset in offsets {
            let due = start + Duration::from_millis(offset);
            let now = Instant::now();
            if due > now {
                thread::sleep(due - now);
            }
            if tx.send(offset).is_err() {
                break;
            }
        }
        drop(tx);
        for worker in workers {
            if let Ok(mine) = worker.join() {
                samples.extend(mine);
            }
        }
    });
    summarize(profile, target, "builtin", &samples, start.elapsed())
}

/// Which external generator a run uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tool {
    Builtin,
    K6(PathBuf),
    /// vegeta holds a constant rate only: the steady and soak shapes.
    Vegeta(PathBuf),
}

/// The first file called `name` in the `PATH`-style list `path_var`.
pub fn find_on(path_var: &str, name: &str) -> Option<PathBuf> {
    let exe = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(path_var)
        .filter(|dir| !dir.as_os_str().is_empty())
        .flat_map(|dir| [dir.join(name), dir.join(&exe)])
        .find(|candidate| candidate.is_file())
}

/// The generator a profile runs with: k6 when it is on the path (any shape), else vegeta for a constant rate, else the
/// built-in one. Nothing is installed.
pub fn choose(profile: &Profile, path_var: &str) -> Tool {
    if let Some(k6) = find_on(path_var, "k6") {
        return Tool::K6(k6);
    }
    match (profile.shape, find_on(path_var, "vegeta")) {
        (Shape::Steady | Shape::Soak, Some(vegeta)) => Tool::Vegeta(vegeta),
        _ => Tool::Builtin,
    }
}

/// The k6 script for a profile: a ramping arrival rate whose stages are the profile's rate over its length.
pub fn k6_script(profile: &Profile, target: &str) -> String {
    let ms = profile.duration_ms;
    let rps = profile.rps;
    let (start, stages) = match profile.shape {
        Shape::Steady | Shape::Soak => (rps, vec![(ms, rps)]),
        Shape::Ramp => (0.0, vec![(ms, rps)]),
        Shape::Spike => {
            let base = rps / 5.0;
            let burst = (ms as f64 * 0.2) as u64;
            let calm = (ms as f64 * 0.4) as u64;
            (base, vec![(calm, base), (burst, rps), (calm, base)])
        }
    };
    let stages: Vec<Value> = stages
        .into_iter()
        .map(|(duration, target)| json!({"duration": format!("{duration}ms"), "target": target}))
        .collect();
    let peak = rps.ceil().max(1.0) as u64;
    let options = json!({
        "summaryTrendStats": ["avg", "min", "med", "max", "p(50)", "p(95)", "p(99)"],
        "scenarios": {"load": {
            "executor": "ramping-arrival-rate",
            "startRate": start,
            "timeUnit": "1s",
            "preAllocatedVUs": peak,
            "maxVUs": peak * 4,
            "stages": stages,
        }},
    });
    let target = Value::String(target.to_string());
    format!(
        "import http from 'k6/http';\nexport const options = {options};\nexport default function () {{\n  http.get({target});\n}}\n"
    )
}

/// The vegeta arguments of a constant-rate attack and the `report` that reads its results.
pub fn vegeta_args(profile: &Profile, targets: &Path, out: &Path) -> Vec<String> {
    vec![
        "attack".into(),
        format!("-targets={}", targets.display()),
        format!("-rate={}", profile.rps.ceil().max(1.0) as u64),
        format!("-duration={}ms", profile.duration_ms),
        format!("-output={}", out.display()),
    ]
}

/// Reads k6's `--summary-export` JSON: its request count, failure rate, rate and the duration's percentiles (ms).
pub fn parse_k6(profile: &Profile, target: &str, summary: &Value) -> Result<Summary, String> {
    let metric = |name: &str| -> Option<&Value> {
        let m = summary.get("metrics")?.get(name)?;
        Some(m.get("values").unwrap_or(m))
    };
    let number = |m: Option<&Value>, key: &str| -> Result<f64, String> {
        m.and_then(|m| m.get(key))
            .and_then(Value::as_f64)
            .ok_or_else(|| format!("the k6 summary has no {key}"))
    };
    let duration = metric("http_req_duration");
    let reqs = metric("http_reqs");
    let failed = metric("http_req_failed");
    let requests = number(reqs, "count")?;
    Ok(Summary {
        profile: profile.name.clone(),
        target: target.to_string(),
        tool: "k6".into(),
        requests: requests as u64,
        errors: (number(failed, "rate")? * requests).round() as u64,
        elapsed_ms: profile.duration_ms,
        throughput_rps: number(reqs, "rate")?,
        error_rate: number(failed, "rate")?,
        p50_ms: number(duration, "p(50)")?,
        p95_ms: number(duration, "p(95)")?,
        p99_ms: number(duration, "p(99)")?,
        max_ms: number(duration, "max")?,
    })
}

/// Reads `vegeta report -type=json`: its latencies in nanoseconds, its requests, throughput and success ratio.
pub fn parse_vegeta(profile: &Profile, target: &str, report: &Value) -> Result<Summary, String> {
    let latencies = report
        .get("latencies")
        .ok_or("the vegeta report has no latencies")?;
    let ms = |key: &str| -> Result<f64, String> {
        latencies
            .get(key)
            .and_then(Value::as_f64)
            .map(|ns| ns / 1_000_000.0)
            .ok_or_else(|| format!("the vegeta report has no latencies.{key}"))
    };
    let requests = report
        .get("requests")
        .and_then(Value::as_u64)
        .ok_or("the vegeta report has no requests")?;
    let success = report.get("success").and_then(Value::as_f64).unwrap_or(1.0);
    let error_rate = (1.0 - success).clamp(0.0, 1.0);
    Ok(Summary {
        profile: profile.name.clone(),
        target: target.to_string(),
        tool: "vegeta".into(),
        requests,
        errors: (error_rate * requests as f64).round() as u64,
        elapsed_ms: profile.duration_ms,
        throughput_rps: report
            .get("throughput")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        error_rate,
        p50_ms: ms("50th")?,
        p95_ms: ms("95th")?,
        p99_ms: ms("99th")?,
        max_ms: ms("max")?,
    })
}

/// Runs the profile with `tool`, keeping its files in `scratch`. The built-in generator needs no files.
pub fn run_with(
    tool: &Tool,
    target: &str,
    profile: &Profile,
    scratch: &Path,
) -> Result<Summary, String> {
    match tool {
        Tool::Builtin => Ok(run_builtin(target, profile, WORKERS)),
        Tool::K6(k6) => {
            let script = scratch.join("load.js");
            let summary_file = scratch.join("k6-summary.json");
            std::fs::write(&script, k6_script(profile, target)).map_err(|e| e.to_string())?;
            let status = Command::new(k6)
                .arg("run")
                .arg("--quiet")
                .arg("--summary-export")
                .arg(&summary_file)
                .arg(&script)
                .status()
                .map_err(|e| format!("k6 did not start: {e}"))?;
            if !status.success() {
                return Err(format!("k6 ended with {status}"));
            }
            let text = std::fs::read_to_string(&summary_file).map_err(|e| e.to_string())?;
            let json: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            parse_k6(profile, target, &json)
        }
        Tool::Vegeta(vegeta) => {
            let targets = scratch.join("targets.txt");
            let results = scratch.join("vegeta.bin");
            std::fs::write(&targets, format!("GET {target}\n")).map_err(|e| e.to_string())?;
            let attack = Command::new(vegeta)
                .args(vegeta_args(profile, &targets, &results))
                .status()
                .map_err(|e| format!("vegeta did not start: {e}"))?;
            if !attack.success() {
                return Err(format!("vegeta attack ended with {attack}"));
            }
            let report = Command::new(vegeta)
                .args(["report", "-type=json"])
                .arg(&results)
                .output()
                .map_err(|e| format!("vegeta report did not start: {e}"))?;
            let json: Value = serde_json::from_slice(&report.stdout).map_err(|e| e.to_string())?;
            parse_vegeta(profile, target, &json)
        }
    }
}

/// The summary's metrics, by the names in [`METRICS`].
pub fn metric_points(summary: &Summary) -> [(&'static str, f64); 6] {
    [
        (METRICS[0], summary.p50_ms),
        (METRICS[1], summary.p95_ms),
        (METRICS[2], summary.p99_ms),
        (METRICS[3], summary.max_ms),
        (METRICS[4], summary.throughput_rps),
        (METRICS[5], summary.error_rate),
    ]
}

/// The summary as OTLP/JSON gauges of `apz-load`, stamped `now_ms`: what the lab's collector receives and the hub
/// records as the run's series.
pub fn otlp(summary: &Summary, now_ms: u64) -> Value {
    let nanos = (now_ms as u128 * 1_000_000).to_string();
    let metrics: Vec<Value> = metric_points(summary)
        .iter()
        .map(|(name, value)| {
            json!({"name": name, "gauge": {"dataPoints": [
                {"timeUnixNano": nanos, "asDouble": value}
            ]}})
        })
        .collect();
    json!({"resourceMetrics": [{
        "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "apz-load"}}]},
        "scopeMetrics": [{"metrics": metrics}],
    }]})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str) -> Profile {
        parse(name, Some(10_000)).unwrap()
    }

    #[test]
    fn names_parse_to_shapes_rates_and_default_lengths() {
        let steady = parse("steady-200rps", None).unwrap();
        assert_eq!(steady.shape, Shape::Steady);
        assert_eq!(steady.rps, 200.0);
        assert_eq!(steady.duration_ms, DEFAULT_MS);
        assert_eq!(parse("soak-20rps", None).unwrap().duration_ms, SOAK_MS);
        assert_eq!(parse("ramp-50rps", Some(4_000)).unwrap().duration_ms, 4_000);
        assert_eq!(parse("spike-100rps", None).unwrap().shape, Shape::Spike);
        assert!(parse("burst-100rps", None).is_err());
        assert!(parse("steady-0rps", None).is_err());
        assert!(parse("steady-200", None).is_err());
        assert!(parse("steady", None).is_err());
    }

    #[test]
    fn a_steady_profile_sends_its_rate_evenly() {
        // 200 a second for 10 s is 2000 requests, one every 5 ms.
        let offsets = schedule(&profile("steady-200rps"));
        assert!(
            (offsets.len() as i64 - 2000).abs() <= 1,
            "{}",
            offsets.len()
        );
        assert!(
            offsets
                .windows(2)
                .all(|w| w[1] - w[0] == 5 || w[1] - w[0] == 4 || w[1] - w[0] == 6)
        );
        assert!(offsets.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn a_ramp_sends_fewer_requests_early_and_its_gaps_shrink() {
        // Climbing from 0 to 200 over 10 s is 1000 requests; the first half holds a quarter of them.
        let offsets = schedule(&profile("ramp-200rps"));
        assert!(
            (offsets.len() as i64 - 1000).abs() <= 1,
            "{}",
            offsets.len()
        );
        let first_half = offsets.iter().filter(|&&t| t < 5_000).count();
        assert!((first_half as i64 - 250).abs() <= 2, "{first_half}");
        let early_gap = offsets[10] - offsets[9];
        let late_gap = offsets[offsets.len() - 1] - offsets[offsets.len() - 2];
        assert!(late_gap <= early_gap, "{early_gap} {late_gap}");
    }

    #[test]
    fn a_spike_bursts_through_the_middle_fifth_of_its_length() {
        // A fifth of 200 (40 a second) for 8 s around a burst of 200 a second for 2 s: 320 + 400 = 720.
        let offsets = schedule(&profile("spike-200rps"));
        assert!((offsets.len() as i64 - 720).abs() <= 2, "{}", offsets.len());
        let burst = offsets
            .iter()
            .filter(|&&t| (4_000..6_000).contains(&t))
            .count();
        assert!((burst as i64 - 400).abs() <= 2, "{burst}");
    }

    #[test]
    fn a_soak_holds_its_rate_for_its_whole_length() {
        let soak = Profile {
            name: "soak-50rps".into(),
            shape: Shape::Soak,
            rps: 50.0,
            duration_ms: 20_000,
        };
        let offsets = schedule(&soak);
        assert!(
            (offsets.len() as i64 - 1000).abs() <= 1,
            "{}",
            offsets.len()
        );
        assert!(*offsets.last().unwrap() >= 19_000);
    }

    #[test]
    fn percentiles_are_nearest_rank() {
        let values = [10.0, 20.0, 30.0, 40.0];
        assert_eq!(percentile(&values, 50.0), 20.0);
        assert_eq!(percentile(&values, 95.0), 40.0);
        assert_eq!(percentile(&values, 0.0), 10.0);
        assert_eq!(percentile(&[], 50.0), 0.0);
    }

    #[test]
    fn a_summary_counts_failures_and_takes_the_tail() {
        let samples: Vec<Sample> = (1..=10)
            .map(|i| Sample {
                latency_ms: i as f64 * 10.0,
                failed: i == 10,
            })
            .collect();
        let s = summarize(
            &profile("steady-1rps"),
            "http://x",
            "builtin",
            &samples,
            Duration::from_secs(10),
        );
        assert_eq!(s.requests, 10);
        assert_eq!(s.errors, 1);
        assert_eq!(s.error_rate, 0.1);
        assert_eq!(s.p50_ms, 50.0);
        assert_eq!(s.max_ms, 100.0);
        assert_eq!(s.throughput_rps, 1.0);
    }

    #[test]
    fn the_tools_are_chosen_by_what_is_on_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let vegeta = dir.path().join("vegeta");
        std::fs::write(&vegeta, "").unwrap();
        let path = dir.path().display().to_string();
        assert_eq!(
            choose(&profile("steady-5rps"), &path),
            Tool::Vegeta(vegeta.clone())
        );
        assert_eq!(choose(&profile("ramp-5rps"), &path), Tool::Builtin);
        assert_eq!(
            choose(&profile("steady-5rps"), "/nonexistent"),
            Tool::Builtin
        );
        std::fs::write(dir.path().join("k6"), "").unwrap();
        assert!(matches!(choose(&profile("ramp-5rps"), &path), Tool::K6(_)));
    }

    #[test]
    fn the_k6_script_carries_the_profiles_stages() {
        let spike = k6_script(&profile("spike-100rps"), "http://127.0.0.1:9/x");
        assert!(spike.contains("ramping-arrival-rate"));
        assert!(
            spike.contains("\"duration\":\"4000ms\",\"target\":20.0"),
            "{spike}"
        );
        assert!(spike.contains("\"target\":100.0"), "{spike}");
        assert!(spike.contains("http.get(\"http://127.0.0.1:9/x\")"));
        assert!(k6_script(&profile("ramp-100rps"), "http://h").contains("\"startRate\":0.0"));
    }

    #[test]
    fn vegeta_takes_a_constant_rate_over_the_profile_length() {
        let args = vegeta_args(&profile("steady-40rps"), Path::new("/t"), Path::new("/o"));
        assert!(args.contains(&"-rate=40".to_string()));
        assert!(args.contains(&"-duration=10000ms".to_string()));
    }

    #[test]
    fn k6_and_vegeta_summaries_parse() {
        let k6 = json!({"metrics": {
            "http_req_duration": {"values": {"p(50)": 12.5, "p(95)": 30.0, "p(99)": 41.0, "max": 55.0}},
            "http_reqs": {"values": {"count": 500, "rate": 49.8}},
            "http_req_failed": {"values": {"rate": 0.02}},
        }});
        let s = parse_k6(&profile("steady-50rps"), "http://h", &k6).unwrap();
        assert_eq!(
            (s.requests, s.errors, s.p50_ms, s.max_ms),
            (500, 10, 12.5, 55.0)
        );
        assert_eq!(s.tool, "k6");

        let vegeta = json!({
            "latencies": {"mean": 2_000_000, "50th": 1_500_000, "95th": 4_000_000, "99th": 9_000_000, "max": 12_000_000},
            "requests": 400, "throughput": 39.9, "success": 0.995,
        });
        let v = parse_vegeta(&profile("steady-40rps"), "http://h", &vegeta).unwrap();
        assert_eq!((v.p50_ms, v.p95_ms, v.max_ms), (1.5, 4.0, 12.0));
        assert_eq!(v.errors, 2);
        assert!(
            parse_vegeta(
                &profile("steady-40rps"),
                "http://h",
                &json!({"requests": 1})
            )
            .is_err()
        );
    }

    #[test]
    fn the_summary_is_recorded_as_gauges_by_name() {
        let s = Summary {
            profile: "steady-1rps".into(),
            target: "http://h".into(),
            tool: "builtin".into(),
            requests: 1,
            errors: 0,
            elapsed_ms: 1000,
            throughput_rps: 1.0,
            error_rate: 0.0,
            p50_ms: 3.0,
            p95_ms: 4.0,
            p99_ms: 5.0,
            max_ms: 6.0,
        };
        let body = otlp(&s, 1_700_000_000_000);
        let metrics = &body["resourceMetrics"][0]["scopeMetrics"][0]["metrics"];
        assert_eq!(metrics.as_array().unwrap().len(), 6);
        assert_eq!(metrics[0]["name"], METRICS[0]);
        assert_eq!(metrics[0]["gauge"]["dataPoints"][0]["asDouble"], 3.0);
    }
}
