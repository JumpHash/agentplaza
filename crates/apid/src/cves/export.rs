//! The CVE report's exports (PLAN 24.3): the findings as JSON, or as CSV with one row per finding (its releases and
//! environments joined by `;`).

use serde_json::{Value as Json, json};

use super::Finding;

/// The CSV's columns, in order.
pub const COLUMNS: [&str; 22] = [
    "id",
    "ids",
    "ecosystem",
    "package",
    "title",
    "severity",
    "priority",
    "cvss",
    "published",
    "source",
    "url",
    "state",
    "reachability",
    "main_version",
    "main_environments",
    "fixed",
    "releases",
    "environments",
    "fix_state",
    "fix_task",
    "functions",
    "updated_ms",
];

/// The report as JSON: the findings with their releases.
pub fn json(findings: &[Finding]) -> String {
    json!({ "findings": findings }).to_string()
}

/// The report as CSV: a header row, then one row per finding. Fields with a comma, a quote or a line break are quoted.
pub fn csv(findings: &[Finding]) -> String {
    let mut out = COLUMNS.join(",");
    out.push('\n');
    for f in findings {
        let releases = f
            .releases
            .iter()
            .map(|r| format!("{}@{}", r.release, r.version))
            .collect::<Vec<_>>()
            .join(";");
        let environments = f
            .releases
            .iter()
            .flat_map(|r| r.environments.iter().map(|e| format!("{e}@{}", r.version)))
            .collect::<Vec<_>>()
            .join(";");
        let cells = [
            f.id.clone(),
            f.ids.join(";"),
            f.ecosystem.clone(),
            f.package.clone(),
            f.title.clone(),
            f.severity.clone(),
            f.priority.clone(),
            f.cvss.clone(),
            f.published.clone(),
            f.source.clone(),
            f.url.clone(),
            f.state.clone(),
            f.reachability.clone(),
            f.main_version.clone(),
            f.main_environments.join(";"),
            f.fixed.clone(),
            releases,
            environments,
            f.fix_state.clone(),
            f.fix_task.clone(),
            f.functions.join(";"),
            f.updated_ms.to_string(),
        ];
        let row: Vec<String> = cells.iter().map(|c| cell(c)).collect();
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}

/// One CSV cell: quoted (with its quotes doubled) when it holds a delimiter, a quote or a line break.
fn cell(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

/// A finding as JSON for the detail route.
pub fn finding_json(f: &Finding) -> Json {
    serde_json::to_value(f).unwrap_or(Json::Null)
}
