//! What a lab run observes (PLAN 22.7): the logs, spans and metric points of a topology's services, read from OTLP/HTTP
//! JSON, and the log lines the lab VM's collector ships. Pure: no I/O.
//!
//! The encoding is OTLP's JSON (`http/json`): the workspace has no opentelemetry-proto or prost types for OTLP, so the
//! receiver and the collector speak the JSON encoding, which every OpenTelemetry SDK can send. Histograms are not read
//! (their buckets are not kept); the counters, gauges and spans are.

use serde_json::{Value, json};

/// An attribute list: each key with its value as text.
pub type Attrs = Vec<(String, String)>;

/// One log record: its time (ms since the epoch), the service it came from, its severity, its body and attributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub t_ms: u64,
    pub service: String,
    pub severity: String,
    pub body: String,
    pub trace_id: String,
    pub span_id: String,
    pub attrs: Attrs,
}

/// One span: its ids, its name, its kind (`server`, `client`, ... or `unspecified`), its times and its status (`ok`,
/// `error` or `unset`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanRec {
    pub trace_id: String,
    pub span_id: String,
    pub parent_id: String,
    pub name: String,
    pub service: String,
    pub kind: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub status: String,
    pub attrs: Attrs,
}

impl SpanRec {
    /// The span's duration in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}

/// How a metric point's value reads: a gauge's reading, a counter's increase since the last point (delta), or a
/// counter's running total (cumulative, which the hub turns into increases).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointKind {
    Gauge,
    Delta,
    Cumulative,
}

/// One data point of a metric.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricPoint {
    pub name: String,
    pub service: String,
    pub t_ms: u64,
    pub value: f64,
    pub kind: PointKind,
    pub attrs: Attrs,
}

/// The severity of a plain log line: the first level word among its first tokens (`ERROR`, `level=warn`, `[info]`), or
/// `INFO` when it names none.
pub fn severity_of(line: &str) -> &'static str {
    for token in line.split_whitespace().take(8) {
        let word = token
            .rsplit_once('=')
            .map_or(token, |(_, value)| value)
            .trim_matches(|c: char| !c.is_ascii_alphabetic())
            .to_ascii_uppercase();
        match word.as_str() {
            "FATAL" | "CRITICAL" | "PANIC" => return "FATAL",
            "ERROR" | "ERR" => return "ERROR",
            "WARN" | "WARNING" => return "WARN",
            "INFO" => return "INFO",
            "DEBUG" | "TRACE" => return "DEBUG",
            _ => {}
        }
    }
    "INFO"
}

/// Whether a severity is an error (ERROR, FATAL and their kin).
pub fn is_error(severity: &str) -> bool {
    matches!(severity, "ERROR" | "FATAL")
}

/// The severity an OTLP record names: its `severityText` (normalised), else its `severityNumber` (17 to 20 is an error),
/// else the body's own level word.
fn severity_of_record(rec: &Value, body: &str) -> String {
    if let Some(text) = rec.get("severityText").and_then(Value::as_str)
        && !text.trim().is_empty()
    {
        return match text.trim().to_ascii_uppercase().as_str() {
            "WARNING" => "WARN".into(),
            "ERR" => "ERROR".into(),
            other => other.to_string(),
        };
    }
    match rec.get("severityNumber").and_then(Value::as_u64) {
        Some(17..=20) => "ERROR".into(),
        Some(21..=24) => "FATAL".into(),
        Some(13..=16) => "WARN".into(),
        Some(9..=12) => "INFO".into(),
        Some(1..=8) => "DEBUG".into(),
        _ => severity_of(body).into(),
    }
}

/// The OTLP `KeyValue` list as attributes (a missing list is none).
fn attributes(list: Option<&Value>) -> Attrs {
    list.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|kv| {
            let key = kv.get("key")?.as_str()?.to_string();
            Some((key, any_value(kv.get("value").unwrap_or(&Value::Null))))
        })
        .collect()
}

/// An OTLP `AnyValue` as text: its string, number, boolean, or its JSON for a list or a map.
fn any_value(value: &Value) -> String {
    if let Some(text) = value.get("stringValue").and_then(Value::as_str) {
        return text.to_string();
    }
    for key in ["intValue", "doubleValue", "boolValue"] {
        if let Some(inner) = value.get(key) {
            return match inner {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
        }
    }
    if value.is_null() {
        return String::new();
    }
    value.to_string()
}

/// A nanosecond timestamp as milliseconds (OTLP writes it as a string or a number; missing is 0).
fn nanos_ms(value: Option<&Value>) -> u64 {
    let nanos = match value {
        Some(Value::String(text)) => text.parse::<u64>().ok(),
        Some(other) => other.as_u64(),
        None => None,
    };
    nanos.map_or(0, |n| n / 1_000_000)
}

/// The service a resource names (`service.name`), or `unknown`.
fn service_of(resource: Option<&Value>) -> String {
    attributes(resource.and_then(|r| r.get("attributes")))
        .into_iter()
        .find(|(key, _)| key == "service.name")
        .map_or_else(|| "unknown".to_string(), |(_, value)| value)
}

fn text_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The elements of a list field of an object (a missing field is an empty list).
fn elements<'a>(value: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// The log records of an OTLP `ExportLogsServiceRequest`, as JSON.
pub fn parse_logs(body: &Value) -> Result<Vec<LogLine>, String> {
    if !body.is_object() {
        return Err("OTLP logs are a JSON object".into());
    }
    let mut out = Vec::new();
    for resource in elements(body, "resourceLogs") {
        let service = service_of(resource.get("resource"));
        for scope in elements(resource, "scopeLogs") {
            for rec in elements(scope, "logRecords") {
                let text = rec.get("body").map(any_value).unwrap_or_default();
                out.push(LogLine {
                    t_ms: nanos_ms(rec.get("timeUnixNano").or(rec.get("observedTimeUnixNano"))),
                    service: service.clone(),
                    severity: severity_of_record(rec, &text),
                    body: text,
                    trace_id: text_field(rec, "traceId"),
                    span_id: text_field(rec, "spanId"),
                    attrs: attributes(rec.get("attributes")),
                });
            }
        }
    }
    Ok(out)
}

/// The spans of an OTLP `ExportTraceServiceRequest`, as JSON.
pub fn parse_traces(body: &Value) -> Result<Vec<SpanRec>, String> {
    if !body.is_object() {
        return Err("OTLP traces are a JSON object".into());
    }
    let mut out = Vec::new();
    for resource in elements(body, "resourceSpans") {
        let service = service_of(resource.get("resource"));
        for scope in elements(resource, "scopeSpans") {
            for span in elements(scope, "spans") {
                out.push(SpanRec {
                    trace_id: text_field(span, "traceId"),
                    span_id: text_field(span, "spanId"),
                    parent_id: text_field(span, "parentSpanId"),
                    name: text_field(span, "name"),
                    service: service.clone(),
                    kind: span_kind(span.get("kind")),
                    start_ms: nanos_ms(span.get("startTimeUnixNano")),
                    end_ms: nanos_ms(span.get("endTimeUnixNano")),
                    status: span_status(span.get("status")),
                    attrs: attributes(span.get("attributes")),
                });
            }
        }
    }
    Ok(out)
}

/// A span kind from OTLP's number (`2`) or its name (`SPAN_KIND_SERVER`).
fn span_kind(value: Option<&Value>) -> String {
    let name = match value {
        Some(Value::Number(n)) => match n.as_u64() {
            Some(1) => "internal",
            Some(2) => "server",
            Some(3) => "client",
            Some(4) => "producer",
            Some(5) => "consumer",
            _ => "unspecified",
        },
        Some(Value::String(text)) => {
            let upper = text.to_ascii_uppercase();
            ["INTERNAL", "SERVER", "CLIENT", "PRODUCER", "CONSUMER"]
                .into_iter()
                .find(|kind| upper.ends_with(kind))
                .map_or("unspecified", |kind| match kind {
                    "INTERNAL" => "internal",
                    "SERVER" => "server",
                    "CLIENT" => "client",
                    "PRODUCER" => "producer",
                    _ => "consumer",
                })
        }
        _ => "unspecified",
    };
    name.to_string()
}

/// A span status from OTLP's code (`2`) or its name (`STATUS_CODE_ERROR`).
fn span_status(value: Option<&Value>) -> String {
    let code = value.and_then(|status| status.get("code"));
    let name = match code {
        Some(Value::Number(n)) => match n.as_u64() {
            Some(1) => "ok",
            Some(2) => "error",
            _ => "unset",
        },
        Some(Value::String(text)) => {
            let upper = text.to_ascii_uppercase();
            if upper.ends_with("ERROR") {
                "error"
            } else if upper.ends_with("OK") {
                "ok"
            } else {
                "unset"
            }
        }
        _ => "unset",
    };
    name.to_string()
}

/// The data points of an OTLP `ExportMetricsServiceRequest`, as JSON. A sum that is monotonic is a counter: cumulative
/// (temporality 2) totals are kept as totals, deltas as increases; every other point is a gauge's reading.
pub fn parse_metrics(body: &Value) -> Result<Vec<MetricPoint>, String> {
    if !body.is_object() {
        return Err("OTLP metrics are a JSON object".into());
    }
    let mut out = Vec::new();
    for resource in elements(body, "resourceMetrics") {
        let service = service_of(resource.get("resource"));
        for scope in elements(resource, "scopeMetrics") {
            for metric in elements(scope, "metrics") {
                let name = text_field(metric, "name");
                if name.is_empty() {
                    continue;
                }
                let (points, kind) = if let Some(gauge) = metric.get("gauge") {
                    (
                        elements(gauge, "dataPoints").collect::<Vec<_>>(),
                        PointKind::Gauge,
                    )
                } else if let Some(sum) = metric.get("sum") {
                    let monotonic = sum
                        .get("isMonotonic")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let cumulative = matches!(
                        sum.get("aggregationTemporality"),
                        Some(Value::Number(n)) if n.as_u64() == Some(2)
                    ) || matches!(
                        sum.get("aggregationTemporality"),
                        Some(Value::String(s)) if s.ends_with("CUMULATIVE")
                    );
                    let kind = match (monotonic, cumulative) {
                        (false, _) => PointKind::Gauge,
                        (true, true) => PointKind::Cumulative,
                        (true, false) => PointKind::Delta,
                    };
                    (elements(sum, "dataPoints").collect::<Vec<_>>(), kind)
                } else {
                    continue;
                };
                for point in points {
                    let value = point
                        .get("asDouble")
                        .or_else(|| point.get("asInt"))
                        .and_then(|v| match v {
                            Value::Number(n) => n.as_f64(),
                            Value::String(s) => s.parse::<f64>().ok(),
                            _ => None,
                        });
                    let Some(value) = value.filter(|v| v.is_finite()) else {
                        continue;
                    };
                    out.push(MetricPoint {
                        name: name.clone(),
                        service: service.clone(),
                        t_ms: nanos_ms(point.get("timeUnixNano")),
                        value,
                        kind,
                        attrs: attributes(point.get("attributes")),
                    });
                }
            }
        }
    }
    Ok(out)
}

/// The OTLP logs request the collector ships for a service's new lines: each line with its time (`lines` are
/// (ms since the epoch, text)), its severity read from the text, and its `event.name` when the line is a JSON object
/// that names one (an audit event).
pub fn logs_otlp(service: &str, lines: &[(u64, String)]) -> Value {
    let records: Vec<Value> = lines
        .iter()
        .map(|(t_ms, text)| {
            let mut attributes = Vec::new();
            if let Ok(Value::Object(object)) = serde_json::from_str::<Value>(text)
                && let Some(event) = object.get("event.name").and_then(Value::as_str)
            {
                attributes.push(json!({"key": "event.name", "value": {"stringValue": event}}));
            }
            json!({
                "timeUnixNano": (t_ms.saturating_mul(1_000_000)).to_string(),
                "severityText": severity_of(text),
                "body": {"stringValue": text},
                "attributes": attributes,
            })
        })
        .collect();
    json!({"resourceLogs": [{
        "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": service}}]},
        "scopeLogs": [{"scope": {"name": "apz-topo-collect"}, "logRecords": records}],
    }]})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_is_the_level_word_of_the_first_tokens() {
        assert_eq!(severity_of("ERROR boom"), "ERROR");
        assert_eq!(
            severity_of("2026-10-24T10:00Z level=error db down"),
            "ERROR"
        );
        assert_eq!(severity_of("[warn] slow"), "WARN");
        assert_eq!(severity_of("Warning: slow"), "WARN");
        assert_eq!(severity_of("served 200 in 3 ms"), "INFO");
        assert!(is_error("FATAL") && !is_error("WARN"));
    }

    #[test]
    fn otlp_logs_round_trip_through_the_collector_encoding() {
        let body = logs_otlp(
            "api-1",
            &[
                (1_700_000_000_123, "ERROR seeded failure".into()),
                (
                    1_700_000_000_500,
                    r#"{"event.name":"login.ok","user":"x"}"#.into(),
                ),
            ],
        );
        let lines = parse_logs(&body).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].service, "api-1");
        assert_eq!(lines[0].severity, "ERROR");
        assert_eq!(lines[0].t_ms, 1_700_000_000_123);
        assert_eq!(
            lines[1].attrs,
            [("event.name".to_string(), "login.ok".to_string())]
        );
    }

    #[test]
    fn otlp_spans_read_their_kind_status_and_times() {
        let body = json!({"resourceSpans": [{
            "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "api-1"}}]},
            "scopeSpans": [{"spans": [{
                "traceId": "0af7651916cd43dd8448eb211c80319c",
                "spanId": "b7ad6b7169203331",
                "name": "GET /items",
                "kind": 2,
                "startTimeUnixNano": "1000000000",
                "endTimeUnixNano": "1250000000",
                "status": {"code": 2},
                "attributes": [{"key": "http.route", "value": {"stringValue": "/items"}}],
            }]}],
        }]});
        let spans = parse_traces(&body).unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].kind, "server");
        assert_eq!(spans[0].status, "error");
        assert_eq!(spans[0].duration_ms(), 250);
        assert_eq!(spans[0].service, "api-1");
        assert_eq!(spans[0].attrs[0].1, "/items");
    }

    #[test]
    fn otlp_metrics_keep_gauges_deltas_and_cumulative_totals_apart() {
        let body = json!({"resourceMetrics": [{
            "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "api-1"}}]},
            "scopeMetrics": [{"metrics": [
                {"name": "cache.hits", "sum": {"isMonotonic": true, "aggregationTemporality": 2,
                    "dataPoints": [{"timeUnixNano": "2000000000", "asInt": "7"}]}},
                {"name": "queue.depth", "gauge": {"dataPoints": [{"timeUnixNano": "2000000000", "asDouble": 3.5}]}},
                {"name": "http.server.requests", "sum": {"isMonotonic": true, "aggregationTemporality": 1,
                    "dataPoints": [{"timeUnixNano": "2000000000", "asInt": 2}]}},
            ]}],
        }]});
        let points = parse_metrics(&body).unwrap();
        assert_eq!(points.len(), 3);
        assert_eq!(points[0].kind, PointKind::Cumulative);
        assert_eq!(points[0].value, 7.0);
        assert_eq!(points[1].kind, PointKind::Gauge);
        assert_eq!(points[2].kind, PointKind::Delta);
        assert_eq!(points[2].t_ms, 2000);
    }

    #[test]
    fn an_empty_or_foreign_body_is_an_empty_or_refused_export() {
        assert!(parse_logs(&json!({})).unwrap().is_empty());
        assert!(parse_traces(&json!([1])).is_err());
    }
}
