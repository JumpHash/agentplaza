//! Data out: the OpenMetrics text (latest aggregates, catalog labels only), CSV of a composed view, and the opt-in
//! OTLP/HTTP export of kept traces (JSON encoding, only after the owner confirmed the collector).

use std::fmt::Write as _;
use std::time::Duration;

use apz_metrics::trace::{Span, Trace};
use apz_metrics::{Agg, EntityKind, SeriesKey, Tier};
use apz_metrics_view::{Body, Chart, Panel, Series, View, XAxis};
use serde_json::{Value, json};

use super::Metrics;

/// The OpenMetrics text of every series' latest 1-minute window.
pub fn openmetrics(m: &Metrics) -> String {
    let series: Vec<(SeriesKey, Agg)> = super::lock(&m.rollup)
        .open_at(Tier::Min1)
        .map(|(key, _start, agg)| (key.clone(), agg.clone()))
        .collect();
    apz_metrics::openmetrics::render(&series)
}

/// The chart series of a view as CSV: `panel,series,time,value` (time as RFC 3339 UTC), quoting as needed.
pub fn csv(view: &View) -> String {
    let mut out = String::from("panel,series,time,value\n");
    for panel in &view.panels {
        let Body::Chart(chart) = &panel.body else {
            continue;
        };
        for series in chart.series.iter().chain(&chart.ghosts) {
            csv_rows(&mut out, panel, chart, series);
        }
    }
    out
}

/// One CSV row per non-null value of a series, at the time its index has on the chart's x axis.
fn csv_rows(out: &mut String, panel: &Panel, chart: &Chart, series: &Series) {
    for (i, value) in series.values.iter().enumerate() {
        let Some(value) = value else {
            continue;
        };
        let time = match &chart.x {
            XAxis::Time { start_ms, step_ms } => {
                rfc3339(start_ms.saturating_add((i as u64).saturating_mul(*step_ms)))
            }
            XAxis::Labels { labels } => labels.get(i).cloned().unwrap_or_default(),
        };
        let _ = writeln!(
            out,
            "{},{},{},{}",
            csv_field(&panel.id),
            csv_field(&series.name),
            csv_field(&time),
            value
        );
    }
}

/// A CSV field, quoted (with its quotes doubled) when it holds a comma, a quote or a line break.
fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

/// An epoch-millisecond time as RFC 3339 in UTC, to the second (`2026-10-16T14:20:00Z`).
fn rfc3339(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    let in_day = secs.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        in_day / 3600,
        in_day % 3600 / 60,
        in_day % 60
    )
}

/// The civil (proleptic Gregorian) year, month and day of a day count since 1970-01-01 (Howard Hinnant's
/// algorithm, valid for any date the timestamps can hold).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097; // 0 ..= 146096
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100); // 0 ..= 365
    let march_based_month = (5 * day_of_year + 2) / 153; // 0 ..= 11, from March
    let day = day_of_year - (153 * march_based_month + 2) / 5 + 1;
    let month = if march_based_month < 10 {
        march_based_month + 3
    } else {
        march_based_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Sends a kept trace to the owner's OTLP collector when the export is on and confirmed. Runs on its own thread;
/// failures are counted in `ext.errors` for the collector and never retried.
pub fn otlp(m: &Metrics, trace: &Trace) {
    let outside = m.settings.get().outside;
    if !(outside.otlp && outside.otlp_confirmed && !outside.otlp_url.is_empty()) {
        return;
    }
    let url = otlp_traces_url(&outside.otlp_url);
    let body = otlp_json(trace);
    let failures = m
        .registry
        .counter(SeriesKey::new("ext.errors", EntityKind::Service, "otlp"));
    std::thread::spawn(move || {
        if post_json(&url, &body).is_err() {
            failures.inc();
        }
    });
}

/// The collector's traces endpoint: the owner's URL, with `/v1/traces` added when it names no path.
fn otlp_traces_url(url: &str) -> String {
    let base = url.trim_end_matches('/');
    let host_and_path = base.split_once("://").map_or(base, |(_, rest)| rest);
    if host_and_path.contains('/') {
        base.to_string()
    } else {
        format!("{base}/v1/traces")
    }
}

/// One POST of a JSON body. A transport failure and an error status are both `Err`.
fn post_json(url: &str, body: &Value) -> Result<(), ureq::Error> {
    let agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_secs(10)))
            .build(),
    );
    agent.post(url).send_json(body).map(|_| ())
}

/// The OTLP/HTTP JSON body of one trace: every span of it, in one resource and scope.
fn otlp_json(trace: &Trace) -> Value {
    let trace_id = trace_hex_id(&trace.id);
    let spans: Vec<Value> = trace
        .spans
        .iter()
        .map(|span| otlp_span(&trace_id, span))
        .collect();
    json!({
        "resourceSpans": [{
            "resource": {
                "attributes": [{"key": "service.name", "value": {"stringValue": "agentplaza"}}]
            },
            "scopeSpans": [{
                "scope": {"name": "apz-metrics"},
                "spans": spans,
            }],
        }],
    })
}

/// One span as OTLP JSON: ids in hex, times in nanoseconds as strings, a parent when it has one.
fn otlp_span(trace_id: &str, span: &Span) -> Value {
    let start_ns = span.start_ms.saturating_mul(1_000_000);
    let duration_ns = (span.dur_ms.max(0.0) * 1_000_000.0).round() as u64;
    let attributes: Vec<Value> = span
        .attrs
        .iter()
        .map(|(key, value)| json!({"key": key, "value": {"stringValue": value}}))
        .collect();
    let mut out = json!({
        "traceId": trace_id,
        "spanId": format!("{:016x}", span.id),
        "name": span.name,
        "startTimeUnixNano": start_ns.to_string(),
        "endTimeUnixNano": start_ns.saturating_add(duration_ns).to_string(),
        "attributes": attributes,
    });
    if let Some(parent) = span.parent {
        out["parentSpanId"] = json!(format!("{parent:016x}"));
    }
    if !span.ok {
        out["status"] = json!({"code": 2});
    }
    out
}

/// 32 hex characters for a trace id: two FNV-1a 64-bit hashes of its text, one for each half.
fn trace_hex_id(id: &str) -> String {
    const BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    let high = fnv1a(id.as_bytes(), BASIS);
    let low = fnv1a(id.as_bytes(), BASIS ^ 0x9e37_79b9_7f4a_7c15);
    format!("{high:016x}{low:016x}")
}

fn fnv1a(bytes: &[u8], basis: u64) -> u64 {
    bytes.iter().fold(basis, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_metrics::Unit;
    use apz_metrics::trace::SpanKind;
    use apz_metrics_view::Tone;

    #[test]
    fn a_chart_csv_quotes_a_series_name_and_writes_utc_times() {
        let chart = Chart {
            unit: Unit::Count,
            x: XAxis::Time {
                start_ms: 1_792_160_400_000,
                step_ms: 60_000,
            },
            series: vec![Series {
                name: "api, \"fast\"".into(),
                tone: Tone::C1,
                dash: false,
                values: vec![Some(1.0), Some(2.5)],
                gap_reason: None,
                scope: None,
            }],
            stacked: false,
            area: false,
            y_max: None,
            band: None,
            ghosts: Vec::new(),
            markers: Vec::new(),
            limit: None,
            height: 200,
            annotate: false,
            point_tab: None,
        };
        let view = View {
            panels: vec![Panel {
                id: "ttft".into(),
                title: "Time to first token".into(),
                sub: String::new(),
                right: String::new(),
                span: 12,
                metric: None,
                live: false,
                pinned: None,
                actions: Vec::new(),
                body: Body::Chart(chart),
            }],
            ..View::default()
        };
        assert_eq!(
            csv(&view),
            "panel,series,time,value\n\
             ttft,\"api, \"\"fast\"\"\",2026-10-16T14:20:00Z,1\n\
             ttft,\"api, \"\"fast\"\"\",2026-10-16T14:21:00Z,2.5\n"
        );
    }

    #[test]
    fn an_otlp_payload_has_one_span_per_trace_span_with_parent_ids() {
        let trace = Trace {
            id: "turn-4417-1".into(),
            cid: 4417,
            role: "builder".into(),
            start_ms: 1_792_160_400_000,
            dur_ms: 1520.0,
            ok: false,
            spans: vec![
                Span {
                    id: 1,
                    parent: None,
                    kind: SpanKind::Model,
                    name: "model call · anthropic/opus".into(),
                    start_ms: 1_792_160_400_000,
                    dur_ms: 1500.0,
                    first_token_ms: Some(300.0),
                    ok: true,
                    attrs: vec![("model".into(), "opus".into())],
                },
                Span {
                    id: 2,
                    parent: Some(1),
                    kind: SpanKind::Tool,
                    name: "tool · bash".into(),
                    start_ms: 1_792_160_401_500,
                    dur_ms: 20.0,
                    first_token_ms: None,
                    ok: false,
                    attrs: Vec::new(),
                },
            ],
            kept: None,
        };
        let body = otlp_json(&trace);
        let spans = &body["resourceSpans"][0]["scopeSpans"][0]["spans"];
        assert_eq!(spans.as_array().map(Vec::len), Some(2));
        assert_eq!(spans[0].get("parentSpanId"), None);
        assert_eq!(spans[1]["parentSpanId"], spans[0]["spanId"]);
        assert_eq!(spans[1]["status"]["code"], 2);
        assert_eq!(spans[0]["traceId"].as_str().map(str::len), Some(32));
        assert_eq!(spans[0]["spanId"].as_str().map(str::len), Some(16));
        assert_eq!(spans[0]["startTimeUnixNano"], "1792160400000000000");
        assert_eq!(spans[1]["endTimeUnixNano"], "1792160401520000000");
    }

    #[test]
    fn rfc3339_writes_known_timestamps_in_utc() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_000_000_000_000), "2001-09-09T01:46:40Z");
        assert_eq!(rfc3339(1_792_160_400_000), "2026-10-16T14:20:00Z");
    }
}
