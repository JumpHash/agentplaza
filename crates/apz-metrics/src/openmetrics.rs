//! The OpenMetrics text exposition for an outside scraper (the opt-in `/metrics` endpoint). Aggregates only; the
//! labels are the entity (`kind`, `entity`) and the metric's catalog labels — any other label on a key is dropped,
//! so no request text can leak through a label value. Label values are escaped (`\\`, `"`, newline).
//!
//! Names: the metric id with `.` replaced by `_`, prefixed `apz_`. Counters get `_total`; histograms are written
//! as summaries with `quantile="0.5"|"0.95"|"0.99"` plus `_sum` and `_count`; gauges as gauges. Ends with `# EOF`.

use std::fmt::Write as _;

use crate::SeriesKey;
use crate::agg::Agg;
use crate::catalog::{MetricDef, MetricKind, find};

/// The quantiles written for each histogram series.
const QUANTILES: [f64; 3] = [0.5, 0.95, 0.99];

/// Renders the latest aggregate of each series (counters as their running totals).
pub fn render(series: &[(SeriesKey, Agg)]) -> String {
    let mut rows: Vec<&(SeriesKey, Agg)> = series.iter().collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = String::new();
    let mut current: Option<&str> = None;
    for (key, agg) in rows {
        // Metrics the catalog does not know are skipped rather than exposed under a guessed type.
        let Some(def) = find(&key.metric) else {
            continue;
        };
        if current != Some(key.metric.as_str()) {
            current = Some(key.metric.as_str());
            write_header(&mut out, def);
        }
        write_series(&mut out, def, key, agg);
    }
    out.push_str("# EOF\n");
    out
}

/// The `# TYPE` and `# HELP` lines of one metric family.
fn write_header(out: &mut String, def: &MetricDef) {
    let name = family_name(def);
    let kind = match def.kind {
        MetricKind::Counter => "counter",
        MetricKind::Gauge => "gauge",
        MetricKind::Histogram => "summary",
    };
    let title = def.title.replace('\\', "\\\\").replace('\n', "\\n");
    let _ = writeln!(out, "# TYPE {name} {kind}");
    let _ = writeln!(out, "# HELP {name} {title}");
}

/// The sample lines of one series.
fn write_series(out: &mut String, def: &MetricDef, key: &SeriesKey, agg: &Agg) {
    let name = family_name(def);
    let labels = label_set(def, key);
    match def.kind {
        MetricKind::Counter => {
            let _ = writeln!(out, "{name}_total{{{labels}}} {}", number(agg.sum));
        }
        MetricKind::Gauge => {
            let _ = writeln!(out, "{name}{{{labels}}} {}", number(agg.last));
        }
        MetricKind::Histogram => {
            if let Some(hist) = &agg.hist {
                for q in QUANTILES {
                    if let Some(value) = hist.quantile(q) {
                        let _ =
                            writeln!(out, "{name}{{{labels},quantile=\"{q}\"}} {}", number(value));
                    }
                }
            }
            let _ = writeln!(out, "{name}_sum{{{labels}}} {}", number(agg.sum));
            let _ = writeln!(out, "{name}_count{{{labels}}} {}", agg.count);
        }
    }
}

/// The exposed family name: `apz_` and the metric id with dots replaced by underscores.
fn family_name(def: &MetricDef) -> String {
    format!("apz_{}", def.id.replace('.', "_"))
}

/// The label set of a series: the entity first, then only the labels the catalog lists for the metric.
fn label_set(def: &MetricDef, key: &SeriesKey) -> String {
    let mut parts = vec![
        format!("kind=\"{}\"", key.kind.prefix()),
        format!("entity=\"{}\"", escape(&key.entity)),
    ];
    for (name, value) in &key.labels {
        if def.labels.iter().any(|allowed| *allowed == name.as_str()) {
            parts.push(format!("{name}=\"{}\"", escape(value)));
        }
    }
    parts.join(",")
}

/// Escapes a label value: backslash, double quote and newline.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
}

/// A sample value as OpenMetrics spells it, including the non-finite values.
fn number(value: f64) -> String {
    if value.is_nan() {
        "NaN".to_string()
    } else if value == f64::INFINITY {
        "+Inf".to_string()
    } else if value == f64::NEG_INFINITY {
        "-Inf".to_string()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EntityKind;

    #[test]
    fn openmetrics_drops_labels_outside_the_catalog_and_escapes_quotes() {
        // llm.calls lists the labels model and provider; prompt is not in the catalog.
        let key = SeriesKey {
            metric: "llm.calls".to_string(),
            kind: EntityKind::Agent,
            entity: "agent-1".to_string(),
            labels: vec![
                ("model".to_string(), "claude \"opus\"".to_string()),
                ("prompt".to_string(), "secret text".to_string()),
            ],
        };
        let agg = Agg {
            sum: 3.0,
            ..Agg::default()
        };
        let text = render(&[(key, agg)]);
        assert!(text.contains("# TYPE apz_llm_calls counter\n"));
        assert!(text.contains("apz_llm_calls_total{kind=\"agent\",entity=\"agent-1\",model=\"claude \\\"opus\\\"\"} 3\n"));
        assert!(!text.contains("prompt"));
        assert!(!text.contains("secret text"));
        assert!(text.ends_with("# EOF\n"));
    }
}
