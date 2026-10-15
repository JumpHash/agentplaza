//! An E2B sandbox's metrics and logs, from its daemon: `GET /metrics` (envd's own) mapped to E2B's
//! `SandboxMetric`, and the daemon's log (`GET /logs`, a plaza extension: each process's start and
//! end) mapped to E2B's `SandboxLogEntry`, filtered and paged the way `GET /v2/sandboxes/{id}/logs`
//! asks.

use serde_json::{Value, json};

/// envd's `Metrics` as E2B's `SandboxMetric`. `timestamp` is RFC 3339 (`rfc3339` formats epoch ms).
pub fn metric(envd: &Value, rfc3339: impl Fn(u64) -> String) -> Value {
    let int = |key: &str| envd.get(key).and_then(Value::as_u64).unwrap_or(0);
    let ts = int("ts");
    json!({
        "timestamp": rfc3339(ts * 1000),
        "timestampUnix": ts,
        "cpuCount": int("cpu_count"),
        "cpuUsedPct": envd.get("cpu_used_pct").and_then(Value::as_f64).unwrap_or(0.0),
        "memUsed": int("mem_used"),
        "memTotal": int("mem_total"),
        "memCache": int("mem_cache"),
        "diskUsed": int("disk_used"),
        "diskTotal": int("disk_total"),
    })
}

/// A log level's rank: `debug` < `info` < `warn` < `error`.
fn rank(level: &str) -> u8 {
    match level {
        "debug" => 0,
        "info" => 1,
        "warn" => 2,
        "error" => 3,
        _ => 1,
    }
}

/// What a logs request asks for.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LogQuery {
    /// Entries at or after (forward) / at or before (backward) this time, epoch ms.
    pub cursor: Option<u64>,
    /// The most entries (at most 1000).
    pub limit: usize,
    /// Newest first.
    pub backward: bool,
    /// The lowest level returned.
    pub level: Option<String>,
    /// A substring the message must contain.
    pub search: Option<String>,
}

/// The daemon's entries (oldest first, each `{timestamp_ms, level, message, fields}`) that `query`
/// selects, as E2B's `SandboxLogEntry` (with `rfc3339` for the time), in the order it asks.
pub fn entries(daemon: &Value, query: &LogQuery, rfc3339: impl Fn(u64) -> String) -> Vec<Value> {
    let floor = query.level.as_deref().map_or(0, rank);
    let mut selected: Vec<&Value> = daemon
        .as_array()
        .map(|all| all.iter().collect())
        .unwrap_or_default();
    selected.retain(|entry| {
        let at = entry["timestamp_ms"].as_u64().unwrap_or(0);
        let level = entry["level"].as_str().unwrap_or("info");
        let message = entry["message"].as_str().unwrap_or("");
        let in_window = match (query.cursor, query.backward) {
            (Some(cursor), false) => at >= cursor,
            (Some(cursor), true) => at <= cursor,
            (None, _) => true,
        };
        in_window
            && rank(level) >= floor
            && query
                .search
                .as_deref()
                .is_none_or(|needle| message.contains(needle))
    });
    if query.backward {
        selected.reverse();
    }
    selected
        .into_iter()
        .take(query.limit.min(1000))
        .map(|entry| {
            json!({
                "timestamp": rfc3339(entry["timestamp_ms"].as_u64().unwrap_or(0)),
                "level": entry["level"].as_str().unwrap_or("info"),
                "message": entry["message"].as_str().unwrap_or(""),
                "fields": entry.get("fields").cloned().unwrap_or_else(|| json!({})),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn time(ms: u64) -> String {
        format!("t{ms}")
    }

    #[test]
    fn envd_metrics_map_to_e2b() {
        let envd = json!({"ts": 10, "cpu_count": 2, "cpu_used_pct": 12.5, "mem_total": 512, "mem_used": 100, "mem_cache": 7, "disk_used": 3, "disk_total": 9});
        let metric = metric(&envd, time);
        assert_eq!(metric["timestamp"], "t10000");
        assert_eq!(metric["timestampUnix"], 10);
        assert_eq!(metric["cpuUsedPct"], 12.5);
        assert_eq!(metric["memTotal"], 512);
        assert_eq!(metric["diskTotal"], 9);
    }

    #[test]
    fn logs_are_filtered_paged_and_ordered() {
        let daemon = json!([
            {"timestamp_ms": 1, "level": "info", "message": "process 1 started: ls", "fields": {"pid": "1"}},
            {"timestamp_ms": 2, "level": "warn", "message": "process 1 ended with 2", "fields": {}},
            {"timestamp_ms": 3, "level": "info", "message": "process 2 started: cat", "fields": {}},
        ]);
        let all = LogQuery {
            limit: 1000,
            ..Default::default()
        };
        assert_eq!(entries(&daemon, &all, time).len(), 3);
        let warn = LogQuery {
            level: Some("warn".into()),
            ..all.clone()
        };
        assert_eq!(
            entries(&daemon, &warn, time)[0]["message"],
            "process 1 ended with 2"
        );
        let back = LogQuery {
            backward: true,
            limit: 2,
            ..all.clone()
        };
        let newest = entries(&daemon, &back, time);
        assert_eq!(newest.len(), 2);
        assert_eq!(newest[0]["timestamp"], "t3");
        let from = LogQuery {
            cursor: Some(2),
            search: Some("started".into()),
            ..all
        };
        let found = entries(&daemon, &from, time);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0]["fields"], json!({}));
    }
}
