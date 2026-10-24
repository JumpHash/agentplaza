//! The health watch (PLAN 23, 11.4): after each stage of a rollout the environment's monitoring is read for the window, and a
//! breach rolls the deploy back on its own (`deploy::roll`), with an incident opened.
//!
//! The source is the `prometheus` connector (`connectors.rs`): a Prometheus-style query API, `GET {url}/api/v1/query`
//! with the query as its `query` parameter. An environment names its two queries in its variables: `HEALTH_ERROR_QUERY`
//! (a ratio from 0 to 1) and `HEALTH_LATENCY_QUERY` (a quantile in seconds), each with `{env}` and `{window}` (seconds)
//! filled in. The defaults read the `environment` label of `http_requests_total` and of
//! `http_request_duration_seconds_bucket`. A query with no sample (no traffic in the window) reads as no value, which
//! breaches nothing. With no monitoring connector enabled, the watch counts the environment's error signals only.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::connectors::encode;
use crate::platforms::http::Http;

/// The error rate query: 5xx responses over all responses, in the environment.
pub const ERROR_QUERY: &str = r#"sum(rate(http_requests_total{environment="{env}",status=~"5.."}[{window}s])) / sum(rate(http_requests_total{environment="{env}"}[{window}s]))"#;
/// The p95 latency query, in seconds.
pub const LATENCY_QUERY: &str = r#"histogram_quantile(0.95, sum by (le) (rate(http_request_duration_seconds_bucket{environment="{env}"}[{window}s])))"#;
/// The most errors a rollout tolerates in a stage's window (and no others: the 11.1 setting).
pub const DEFAULT_MAX_ERROR_RATE: f64 = 0.05;
/// The p95 latency a stage may show, in milliseconds.
pub const DEFAULT_MAX_P95_MS: f64 = 2000.0;

/// What a stage's watch read: the environment's error signals, and the monitoring source's values when it is connected.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Health {
    pub signals: u32,
    /// The error ratio from 0 to 1, when the source gave one.
    pub error_rate: Option<f64>,
    /// The p95 latency in milliseconds, when the source gave one.
    pub p95_ms: Option<f64>,
}

/// One query the watch sends: a Prometheus instant query, answered with its value (the largest of its samples).
pub trait Monitor: Send + Sync {
    fn query(&self, expr: &str) -> Result<Option<f64>, String>;
}

/// The monitoring source's values for an environment over the window: the error ratio and the p95 latency in ms.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reading {
    pub error_rate: Option<f64>,
    pub p95_ms: Option<f64>,
}

/// Reads the two queries of `env` over `window` from `monitor`.
pub fn read(
    monitor: &dyn Monitor,
    env: &str,
    window: Duration,
    error_query: &str,
    latency_query: &str,
) -> Result<Reading, String> {
    let secs = window.as_secs().max(1).to_string();
    let fill = |query: &str| query.replace("{env}", env).replace("{window}", &secs);
    let error_rate = monitor.query(&fill(error_query))?;
    // The latency query answers in seconds; the watch's threshold is in milliseconds.
    let p95_ms = monitor
        .query(&fill(latency_query))?
        .map(|seconds| seconds * 1000.0);
    Ok(Reading { error_rate, p95_ms })
}

/// A Prometheus-style query API, reached through the platforms' HTTP path (a test replaces it with a fake that records
/// each request). The connector's token is sent as a bearer header and never in the URL or an error.
pub struct Prometheus {
    http: Arc<dyn Http>,
    base: String,
    token: Option<String>,
}

impl Prometheus {
    /// A client for the API at `base` (an `http://` or `https://` address), with the connector's `token` when it holds one.
    pub fn new(http: Arc<dyn Http>, base: &str, token: Option<&str>) -> Result<Prometheus, String> {
        let base = base.trim().trim_end_matches('/');
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            return Err("the Prometheus connector's URL is an http or https address".into());
        }
        Ok(Prometheus {
            http,
            base: base.to_string(),
            token: token
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(String::from),
        })
    }

    fn redact(&self, text: &str) -> String {
        match &self.token {
            Some(token) => text.replace(token.as_str(), "[redacted]"),
            None => text.to_string(),
        }
    }
}

impl Monitor for Prometheus {
    fn query(&self, expr: &str) -> Result<Option<f64>, String> {
        let url = format!("{}/api/v1/query?query={}", self.base, encode(expr));
        let headers: Vec<(&str, String)> = self
            .token
            .iter()
            .map(|t| ("Authorization", format!("Bearer {t}")))
            .collect();
        let reply = self
            .http
            .call("GET", &url, &headers, None)
            .map_err(|e| self.redact(&e))?;
        if !(200..300).contains(&reply.status) || reply.body["status"] == "error" {
            let message = reply.body["error"]
                .as_str()
                .unwrap_or("the query was refused");
            return Err(self.redact(&format!(
                "the monitoring source answered {}: {message}",
                reply.status
            )));
        }
        parse_value(&reply.body)
    }
}

/// The value of a Prometheus query answer: a scalar, or the largest sample of a vector. No sample, or a NaN, is no value.
pub fn parse_value(body: &Value) -> Result<Option<f64>, String> {
    let data = &body["data"];
    let samples: Vec<&Value> = match data["resultType"].as_str() {
        Some("scalar") => vec![&data["result"][1]],
        Some("vector") => data["result"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|sample| &sample["value"][1])
            .collect(),
        _ => return Err("the monitoring source's answer names no result type".into()),
    };
    let mut largest: Option<f64> = None;
    for sample in samples {
        let value = match sample {
            Value::String(text) => text
                .parse::<f64>()
                .map_err(|_| format!("the monitoring source sent {text:?} as a value"))?,
            Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
            _ => return Err("the monitoring source's sample has no value".into()),
        };
        if value.is_nan() {
            continue;
        }
        largest = Some(largest.map_or(value, |l| l.max(value)));
    }
    Ok(largest)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;
    use crate::platforms::http::Fake;

    /// A monitor whose error ratio and latency are set by the test, and that keeps each query it was asked.
    pub struct Steady {
        pub error_rate: Mutex<f64>,
        pub p95_seconds: Mutex<f64>,
        pub asked: Mutex<Vec<String>>,
    }

    impl Monitor for Steady {
        fn query(&self, expr: &str) -> Result<Option<f64>, String> {
            self.asked.lock().unwrap().push(expr.to_string());
            if expr.contains("histogram_quantile") {
                Ok(Some(*self.p95_seconds.lock().unwrap()))
            } else {
                Ok(Some(*self.error_rate.lock().unwrap()))
            }
        }
    }

    #[test]
    fn the_queries_name_the_environment_and_the_window() {
        let monitor = Steady {
            error_rate: Mutex::new(0.0),
            p95_seconds: Mutex::new(0.0),
            asked: Mutex::new(Vec::new()),
        };
        read(
            &monitor,
            "production",
            Duration::from_secs(90),
            ERROR_QUERY,
            LATENCY_QUERY,
        )
        .unwrap();
        let asked = monitor.asked.lock().unwrap().clone();
        assert_eq!(asked.len(), 2);
        assert!(asked[0].contains(r#"environment="production""#) && asked[0].contains("[90s]"));
        assert!(
            !asked
                .iter()
                .any(|q| q.contains("{env}") || q.contains("{window}"))
        );
    }

    #[test]
    fn the_latency_is_read_in_milliseconds() {
        let monitor = Steady {
            error_rate: Mutex::new(0.01),
            p95_seconds: Mutex::new(4.5),
            asked: Mutex::new(Vec::new()),
        };
        let reading = read(
            &monitor,
            "staging",
            Duration::from_secs(60),
            ERROR_QUERY,
            LATENCY_QUERY,
        )
        .unwrap();
        assert_eq!(reading.error_rate, Some(0.01));
        assert_eq!(reading.p95_ms, Some(4500.0));
    }

    #[test]
    fn a_vector_reads_its_largest_sample_and_no_sample_is_no_value() {
        let vector = json!({"status": "success", "data": {"resultType": "vector", "result": [
            {"metric": {"pod": "a"}, "value": [1700000000, "0.02"]},
            {"metric": {"pod": "b"}, "value": [1700000000, "0.31"]}
        ]}});
        assert_eq!(parse_value(&vector).unwrap(), Some(0.31));
        let quiet = json!({"status": "success", "data": {"resultType": "vector", "result": []}});
        assert_eq!(parse_value(&quiet).unwrap(), None);
        let nan = json!({"status": "success", "data": {"resultType": "vector", "result": [
            {"metric": {}, "value": [1700000000, "NaN"]}
        ]}});
        assert_eq!(parse_value(&nan).unwrap(), None);
        let scalar = json!({"status": "success", "data": {"resultType": "scalar", "result": [1700000000, "0.5"]}});
        assert_eq!(parse_value(&scalar).unwrap(), Some(0.5));
        assert!(parse_value(&json!({"data": {}})).is_err());
    }

    #[test]
    fn the_client_sends_the_query_with_the_token_only_in_its_header() {
        let fake = Arc::new(Fake::new(|_, url, _| {
            let body = if url.contains("status") {
                json!({"status": "success", "data": {"resultType": "vector", "result": [
                    {"metric": {}, "value": [1, "0.25"]}
                ]}})
            } else {
                json!({"status": "error", "error": "bad query"})
            };
            (200, body)
        }));
        let client =
            Prometheus::new(fake.clone(), "http://prom.test/", Some("prom-token")).unwrap();
        assert_eq!(
            client.query(r#"sum(rate(x{status=~"5.."}[60s]))"#).unwrap(),
            Some(0.25)
        );
        let seen = fake.seen();
        assert_eq!(seen[0].0, "GET");
        assert!(
            seen[0]
                .1
                .starts_with("http://prom.test/api/v1/query?query=sum%28rate")
        );
        assert!(!seen[0].1.contains("prom-token"));
        assert_eq!(seen[0].2.as_deref(), Some("Bearer prom-token"));
        let refused = client.query("bad{").unwrap_err();
        assert!(refused.contains("bad query"), "{refused}");
        assert!(Prometheus::new(fake, "prom.test", None).is_err());
    }
}
