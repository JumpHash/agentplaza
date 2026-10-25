//! Benchmark output parsed into comparable values (PLAN 22.8): criterion (its `estimates.json` files and its console
//! lines), the libtest bencher lines (`cargo bench -- --format bencher` and `#[bench]`), and `go test -bench` lines.
//! Every benchmark is reduced to its mean in nanoseconds, with the spread it reports when it reports one.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The output formats the parser reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Criterion's console output (`name  time:   [low mid high]`).
    CriterionText,
    /// One criterion `estimates.json` (`target/criterion/<group>/<bench>/new/estimates.json`), named by its path.
    CriterionJson,
    /// libtest's bencher lines (`test name ... bench: 1,234 ns/iter (+/- 56)`).
    Libtest,
    /// `go test -bench` lines (`BenchmarkName-8   1000   1234 ns/op`).
    Go,
}

/// One benchmark: its name, its mean in nanoseconds and its spread (± ns) when the output gives one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bench {
    pub name: String,
    pub ns: f64,
    pub spread_ns: Option<f64>,
}

impl Bench {
    /// The metric a benchmark is recorded as: `bench.<name>.ns`, its name's spaces made underscores.
    pub fn metric(&self) -> String {
        format!(
            "bench.{}.ns",
            self.name.split_whitespace().collect::<Vec<_>>().join("_")
        )
    }
}

/// Parses `text` in `format`. A criterion JSON file is named by `name` (its benchmark's path).
pub fn parse(format: Format, text: &str, name: &str) -> Result<Vec<Bench>, String> {
    match format {
        Format::CriterionText => Ok(criterion_text(text)),
        Format::CriterionJson => criterion_json(name, text).map(|b| vec![b]),
        Format::Libtest => Ok(libtest(text)),
        Format::Go => Ok(go(text)),
    }
}

/// The unit's size in nanoseconds (criterion prints `µs` with the micro sign or the Greek mu).
fn unit_ns(unit: &str) -> Option<f64> {
    match unit {
        "ps" => Some(0.001),
        "ns" => Some(1.0),
        "us" | "µs" | "μs" => Some(1_000.0),
        "ms" => Some(1_000_000.0),
        "s" => Some(1_000_000_000.0),
        _ => None,
    }
}

/// Criterion's console lines: `<name> ... time:   [low unit mid unit high unit]`, the mean being the middle value.
pub fn criterion_text(text: &str) -> Vec<Bench> {
    text.lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once("time:")?;
            let inside = rest.trim().strip_prefix('[')?.strip_suffix(']')?;
            let parts: Vec<&str> = inside.split_whitespace().collect();
            if parts.len() != 6 {
                return None;
            }
            let mid: f64 = parts[2].parse().ok()?;
            let ns = mid * unit_ns(parts[3])?;
            let low: f64 = parts[0].parse().ok()?;
            let high: f64 = parts[4].parse().ok()?;
            let spread = (high - low) / 2.0 * unit_ns(parts[1])?;
            let name = name.trim();
            (!name.is_empty()).then(|| Bench {
                name: name.to_string(),
                ns,
                spread_ns: Some(spread),
            })
        })
        .collect()
}

/// One criterion `estimates.json`: the mean's point estimate (ns), with half its confidence interval as the spread.
pub fn criterion_json(name: &str, text: &str) -> Result<Bench, String> {
    let json: Value = serde_json::from_str(text).map_err(|e| format!("{name}: {e}"))?;
    let mean = json.get("mean").ok_or_else(|| format!("{name}: no mean"))?;
    let ns = mean
        .get("point_estimate")
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("{name}: no mean.point_estimate"))?;
    let interval = mean.get("confidence_interval");
    let bound = |key: &str| interval.and_then(|i| i.get(key)).and_then(Value::as_f64);
    let spread_ns = match (bound("lower_bound"), bound("upper_bound")) {
        (Some(lo), Some(hi)) => Some((hi - lo) / 2.0),
        _ => None,
    };
    Ok(Bench {
        name: name.to_string(),
        ns,
        spread_ns,
    })
}

/// libtest's bencher lines, with their thousands separators: `test NAME ... bench:  1,234 ns/iter (+/- 56)`.
pub fn libtest(text: &str) -> Vec<Bench> {
    text.lines()
        .filter_map(|line| {
            let (head, tail) = line.split_once("... bench:")?;
            let name = head.trim().strip_prefix("test ")?.trim();
            let mut words = tail.split_whitespace();
            let ns = words.next()?.replace(',', "").parse::<f64>().ok()?;
            if words.next()? != "ns/iter" {
                return None;
            }
            let spread_ns = line.split("(+/- ").nth(1).and_then(|s| {
                s.trim_end_matches(')')
                    .trim()
                    .replace(',', "")
                    .parse::<f64>()
                    .ok()
            });
            Some(Bench {
                name: name.to_string(),
                ns,
                spread_ns,
            })
        })
        .collect()
}

/// `go test -bench` lines: `BenchmarkName-8    1000    1234 ns/op`, the GOMAXPROCS suffix dropped from the name.
pub fn go(text: &str) -> Vec<Bench> {
    text.lines()
        .filter_map(|line| {
            let words: Vec<&str> = line.split_whitespace().collect();
            let at = words.iter().position(|w| *w == "ns/op")?;
            if at < 2 || !words[0].starts_with("Benchmark") {
                return None;
            }
            let ns: f64 = words[at - 1].parse().ok()?;
            let name = words[0].rsplit_once('-').map_or(words[0], |(name, procs)| {
                if procs.chars().all(|c| c.is_ascii_digit()) {
                    name
                } else {
                    words[0]
                }
            });
            Some(Bench {
                name: name.to_string(),
                ns,
                spread_ns: None,
            })
        })
        .collect()
}

/// The benchmarks as OTLP/JSON gauges, one per benchmark named by [`Bench::metric`], stamped `now_ms`.
pub fn otlp(benches: &[Bench], now_ms: u64) -> Value {
    let nanos = (now_ms as u128 * 1_000_000).to_string();
    let metrics: Vec<Value> = benches
        .iter()
        .map(|bench| {
            json!({"name": bench.metric(), "gauge": {"dataPoints": [
                {"timeUnixNano": nanos, "asDouble": bench.ns}
            ]}})
        })
        .collect();
    json!({"resourceMetrics": [{
        "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "apz-bench"}}]},
        "scopeMetrics": [{"metrics": metrics}],
    }]})
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRITERION: &str = "\
parse_fast              time:   [1.2000 µs 1.3000 µs 1.4000 µs]
                        change: [-1.0% +0.5% +2.0%] (p = 0.42 > 0.05)
parse_slow              time:   [250.00 ns 260.00 ns 270.00 ns]
Benchmarking group with spaces
";

    #[test]
    fn criterion_console_lines_give_the_middle_of_each_interval_in_ns() {
        let benches = criterion_text(CRITERION);
        assert_eq!(benches.len(), 2, "{benches:?}");
        assert_eq!(benches[0].name, "parse_fast");
        assert!((benches[0].ns - 1300.0).abs() < 1e-6);
        assert!((benches[0].spread_ns.unwrap() - 100.0).abs() < 1e-6);
        assert_eq!(benches[1].name, "parse_slow");
        assert!((benches[1].ns - 260.0).abs() < 1e-6);
        assert_eq!(benches[0].metric(), "bench.parse_fast.ns");
    }

    #[test]
    fn a_criterion_estimates_file_gives_its_mean_and_half_its_interval() {
        let text = r#"{"mean":{"confidence_interval":{"confidence_level":0.95,"lower_bound":980.0,"upper_bound":1020.0},
            "point_estimate":1000.5,"standard_error":5.0},"median":{"point_estimate":999.0}}"#;
        let bench = criterion_json("group/parse", text).unwrap();
        assert_eq!(bench.name, "group/parse");
        assert_eq!(bench.ns, 1000.5);
        assert_eq!(bench.spread_ns, Some(20.0));
        assert!(criterion_json("x", "{}").is_err());
        assert!(criterion_json("x", "not json").is_err());
    }

    #[test]
    fn libtest_bencher_lines_parse_with_their_spread() {
        let text = "running 2 tests\n\
            test parse::fast ... bench:       1,234 ns/iter (+/- 56)\n\
            test parse::big ... bench:  2,000,000 ns/iter (+/- 1,000)\n\
            test ignored ... ignored\n";
        let benches = libtest(text);
        assert_eq!(benches.len(), 2);
        assert_eq!(benches[0].name, "parse::fast");
        assert_eq!(benches[0].ns, 1234.0);
        assert_eq!(benches[0].spread_ns, Some(56.0));
        assert_eq!(benches[1].ns, 2_000_000.0);
        assert_eq!(benches[1].spread_ns, Some(1000.0));
    }

    #[test]
    fn go_bench_lines_drop_the_proc_suffix() {
        let text = "goos: linux\nBenchmarkHash-8   \t 1000000\t      1234 ns/op\t 16 B/op\n\
            BenchmarkPlain   \t 500\t 99.5 ns/op\nPASS\n";
        let benches = go(text);
        assert_eq!(benches.len(), 2);
        assert_eq!(benches[0].name, "BenchmarkHash");
        assert_eq!(benches[0].ns, 1234.0);
        assert_eq!(benches[1].name, "BenchmarkPlain");
        assert_eq!(benches[1].ns, 99.5);
    }

    #[test]
    fn parse_dispatches_by_format() {
        assert_eq!(
            parse(Format::Go, "BenchmarkA-2 1 5 ns/op", "").unwrap()[0].ns,
            5.0
        );
        assert!(parse(Format::CriterionJson, "{}", "x").is_err());
    }

    #[test]
    fn benchmarks_are_recorded_as_one_gauge_each() {
        let benches = [Bench {
            name: "parse fast".into(),
            ns: 1300.0,
            spread_ns: Some(100.0),
        }];
        let body = otlp(&benches, 1_700_000_000_000);
        let metrics = &body["resourceMetrics"][0]["scopeMetrics"][0]["metrics"];
        assert_eq!(metrics[0]["name"], "bench.parse_fast.ns");
        assert_eq!(metrics[0]["gauge"]["dataPoints"][0]["asDouble"], 1300.0);
    }
}
