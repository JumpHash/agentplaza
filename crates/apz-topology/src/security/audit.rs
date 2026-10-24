//! Dependency audits and image scans (PLAN 22.8). Each tool runs where it is on the `PATH`, on the release's tree or
//! the named image, and its output is parsed from the documented JSON of the tool; nothing is installed. A tool whose
//! output does not read is a skip with the reason, not a pass and not an invented hit.

use std::path::Path;

use serde_json::Value as Json;

use super::exec;
use super::{Hit, Outcome, TIMED_OUT, TOOL_LIMIT};

/// The dependency audits a stage may name, in the order they run.
pub const AUDITS: &[&str] = &[
    "cargo-audit",
    "cargo-deny",
    "npm-audit",
    "pip-audit",
    "govulncheck",
    "osv-scanner",
];

/// How an audit runs: the executable, the arguments, the binary that must be on the `PATH` beside it (cargo's
/// subcommands), and the files one of which the tree must hold (none: any tree).
struct Plan {
    program: &'static str,
    args: &'static [&'static str],
    subcommand: Option<&'static str>,
    needs_any: &'static [&'static str],
    parse: fn(&str, &str) -> Result<Vec<Hit>, String>,
}

fn plan(name: &str) -> Option<Plan> {
    Some(match name {
        "cargo-audit" => Plan {
            program: "cargo",
            args: &["audit", "--json"],
            subcommand: Some("cargo-audit"),
            needs_any: &["Cargo.lock"],
            parse: |out, _| parse_cargo_audit(out),
        },
        "cargo-deny" => Plan {
            program: "cargo",
            args: &["deny", "--format", "json", "check"],
            subcommand: Some("cargo-deny"),
            needs_any: &["Cargo.toml"],
            // cargo-deny writes its diagnostics to stderr.
            parse: |_, err| parse_cargo_deny(err),
        },
        "npm-audit" => Plan {
            program: "npm",
            args: &["audit", "--json"],
            subcommand: None,
            needs_any: &["package-lock.json"],
            parse: |out, _| parse_npm_audit(out),
        },
        "pip-audit" => Plan {
            program: "pip-audit",
            args: &["--format", "json"],
            subcommand: None,
            needs_any: &["requirements.txt", "pyproject.toml"],
            parse: |out, _| parse_pip_audit(out),
        },
        "govulncheck" => Plan {
            program: "govulncheck",
            args: &["-json", "./..."],
            subcommand: None,
            needs_any: &["go.mod"],
            parse: |out, _| parse_govulncheck(out),
        },
        "osv-scanner" => Plan {
            program: "osv-scanner",
            args: &["scan", "--format", "json", "--recursive", "."],
            subcommand: None,
            needs_any: &[],
            parse: |out, _| parse_osv(out),
        },
        _ => return None,
    })
}

/// Runs the audit `name` over `tree`. Its stage is `audit:<name>`.
pub fn run_audit(name: &str, tree: &Path, path_var: &str) -> Outcome {
    let stage = format!("audit:{name}");
    let Some(plan) = plan(name) else {
        return Outcome::skipped(&stage, format!("{name} is not a known audit"));
    };
    let mut missing = Vec::new();
    let program = exec::tool(path_var, plan.program);
    if program.is_none() {
        missing.push(plan.program);
    }
    if let Some(sub) = plan.subcommand
        && exec::tool(path_var, sub).is_none()
    {
        missing.push(sub);
    }
    if !missing.is_empty() {
        return Outcome::skipped(&stage, format!("{} not on the PATH", missing.join(" and ")));
    }
    if !plan.needs_any.is_empty() && !plan.needs_any.iter().any(|f| tree.join(f).is_file()) {
        return Outcome::skipped(
            &stage,
            format!("the tree holds none of {}", plan.needs_any.join(", ")),
        );
    }
    let program = program.unwrap_or_default();
    let ran = match exec::run(&program, plan.args, tree, TOOL_LIMIT) {
        Ok(ran) => ran,
        Err(why) => return Outcome::skipped(&stage, format!("could not start: {why}")),
    };
    if ran.timed_out {
        return Outcome::skipped(&stage, TIMED_OUT);
    }
    match (plan.parse)(&ran.stdout, &ran.stderr) {
        Ok(hits) => {
            let evidence = format!("{name} exit {:?}: {} advisories", ran.code, hits.len());
            Outcome::ran(&stage, hits, evidence)
        }
        Err(why) => Outcome::skipped(
            &stage,
            format!("its output was not read ({why}; exit {:?})", ran.code),
        ),
    }
}

/// Scans the image `reference` with trivy, or with grype when trivy is not on the `PATH`. Its stage is `image:<tool>`.
pub fn run_image(reference: &str, path_var: &str) -> Outcome {
    let here = std::env::current_dir().unwrap_or_else(|_| Path::new("/").to_path_buf());
    if let Some(trivy) = exec::tool(path_var, "trivy") {
        let stage = "image:trivy";
        let args = ["image", "--format", "json", "--quiet", reference];
        return image_outcome(
            stage,
            exec::run(&trivy, &args, &here, TOOL_LIMIT),
            parse_trivy,
        );
    }
    if let Some(grype) = exec::tool(path_var, "grype") {
        let stage = "image:grype";
        let args = [reference, "-o", "json"];
        return image_outcome(
            stage,
            exec::run(&grype, &args, &here, TOOL_LIMIT),
            parse_grype,
        );
    }
    Outcome::skipped(
        "image",
        format!("neither trivy nor grype is on the PATH: the image {reference} is not scanned"),
    )
}

fn image_outcome(
    stage: &str,
    ran: Result<exec::Ran, String>,
    parse: fn(&str) -> Result<Vec<Hit>, String>,
) -> Outcome {
    let ran = match ran {
        Ok(ran) => ran,
        Err(why) => return Outcome::skipped(stage, format!("could not start: {why}")),
    };
    if ran.timed_out {
        return Outcome::skipped(stage, TIMED_OUT);
    }
    match parse(&ran.stdout) {
        Ok(hits) => Outcome::ran(stage, hits, format!("exit {:?}", ran.code)),
        Err(why) => Outcome::skipped(stage, format!("its output was not read ({why})")),
    }
}

/// A severity as the stages name it, in one vocabulary: `critical`, `high`, `medium`, `low`, or `unknown`.
pub fn severity(text: &str) -> String {
    match text.to_ascii_lowercase().as_str() {
        "critical" => "critical",
        "high" | "important" => "high",
        "medium" | "moderate" => "medium",
        "low" | "negligible" | "info" => "low",
        _ => "unknown",
    }
    .into()
}

/// The CVSS base score's band, for a numeric score as osv-scanner's groups give it.
fn band(score: &str) -> String {
    match score.parse::<f64>() {
        Ok(s) if s >= 9.0 => "critical",
        Ok(s) if s >= 7.0 => "high",
        Ok(s) if s >= 4.0 => "medium",
        Ok(_) => "low",
        Err(_) => "unknown",
    }
    .into()
}

fn text<'a>(value: &'a Json, key: &str) -> &'a str {
    value.get(key).and_then(Json::as_str).unwrap_or_default()
}

fn array<'a>(value: &'a Json, key: &str) -> &'a [Json] {
    value
        .get(key)
        .and_then(Json::as_array)
        .map_or(&[], Vec::as_slice)
}

/// cargo-audit's `--json` report: its `vulnerabilities.list`, one entry per advisory that matches a locked crate.
pub fn parse_cargo_audit(stdout: &str) -> Result<Vec<Hit>, String> {
    let report: Json = serde_json::from_str(stdout).map_err(|e| e.to_string())?;
    let list = report
        .pointer("/vulnerabilities/list")
        .and_then(Json::as_array)
        .ok_or("no vulnerabilities.list")?;
    Ok(list
        .iter()
        .map(|item| {
            let advisory = &item["advisory"];
            Hit {
                id: text(advisory, "id").into(),
                severity: severity(
                    advisory
                        .get("severity")
                        .and_then(Json::as_str)
                        .unwrap_or(""),
                ),
                subject: text(advisory, "package").into(),
                detail: text(advisory, "title").into(),
            }
        })
        .collect())
}

/// cargo-deny's `--format json` diagnostics: one JSON object per line on stderr. Its errors are the failures; a
/// summary line says it finished.
pub fn parse_cargo_deny(stderr: &str) -> Result<Vec<Hit>, String> {
    let mut objects = 0;
    let mut hits = Vec::new();
    for line in stderr.lines().filter(|l| l.trim_start().starts_with('{')) {
        let value: Json = serde_json::from_str(line).map_err(|e| e.to_string())?;
        objects += 1;
        if text(&value, "type") != "diagnostic" {
            continue;
        }
        let fields = &value["fields"];
        if text(fields, "severity") != "error" {
            continue;
        }
        let notes: Vec<&str> = array(fields, "notes")
            .iter()
            .filter_map(Json::as_str)
            .collect();
        hits.push(Hit {
            id: match text(fields, "code") {
                "" => "deny".into(),
                code => code.into(),
            },
            severity: "high".into(),
            subject: text(fields, "message").into(),
            detail: notes.join("; "),
        });
    }
    if objects == 0 {
        return Err("no JSON diagnostics on stderr".into());
    }
    Ok(hits)
}

/// npm audit's `--json` report (npm 7 and later): `vulnerabilities` keyed by package, with its `via` advisories.
pub fn parse_npm_audit(stdout: &str) -> Result<Vec<Hit>, String> {
    let report: Json = serde_json::from_str(stdout).map_err(|e| e.to_string())?;
    let map = report
        .get("vulnerabilities")
        .and_then(Json::as_object)
        .ok_or("no vulnerabilities map")?;
    Ok(map
        .iter()
        .map(|(name, entry)| {
            let titles: Vec<&str> = array(entry, "via")
                .iter()
                .filter_map(|via| via.get("title").and_then(Json::as_str))
                .collect();
            Hit {
                id: name.clone(),
                severity: severity(text(entry, "severity")),
                subject: format!("{name} {}", text(entry, "range")).trim().into(),
                detail: titles.join("; "),
            }
        })
        .collect())
}

/// pip-audit's `--format json`: each dependency with its `vulns`.
pub fn parse_pip_audit(stdout: &str) -> Result<Vec<Hit>, String> {
    let report: Json = serde_json::from_str(stdout).map_err(|e| e.to_string())?;
    let deps = report
        .get("dependencies")
        .and_then(Json::as_array)
        .ok_or("no dependencies list")?;
    let mut hits = Vec::new();
    for dep in deps {
        for vuln in array(dep, "vulns") {
            hits.push(Hit {
                id: text(vuln, "id").into(),
                severity: "unknown".into(),
                subject: format!("{}=={}", text(dep, "name"), text(dep, "version")),
                detail: text(vuln, "description").into(),
            });
        }
    }
    Ok(hits)
}

/// govulncheck's `-json` output: a stream of objects, one of which per finding is `finding` (its `osv` id, its
/// `fixed_version` and the `trace` of the call that reaches it).
pub fn parse_govulncheck(stdout: &str) -> Result<Vec<Hit>, String> {
    let mut hits = Vec::new();
    let mut objects = 0;
    for value in serde_json::Deserializer::from_str(stdout).into_iter::<Json>() {
        let value = value.map_err(|e| e.to_string())?;
        objects += 1;
        let Some(finding) = value.get("finding") else {
            continue;
        };
        let module = array(finding, "trace")
            .first()
            .map(|frame| text(frame, "module"))
            .unwrap_or_default();
        let fixed = text(finding, "fixed_version");
        hits.push(Hit {
            id: text(finding, "osv").into(),
            severity: "unknown".into(),
            subject: module.into(),
            detail: if fixed.is_empty() {
                "no fixed version yet".into()
            } else {
                format!("fixed in {fixed}")
            },
        });
    }
    if objects == 0 {
        return Err("no JSON objects".into());
    }
    Ok(hits)
}

/// OSV-Scanner's `--format json`: `results`, each with its `packages`, each with its `vulnerabilities`; the `groups`
/// of a package give an advisory's score.
pub fn parse_osv(stdout: &str) -> Result<Vec<Hit>, String> {
    let report: Json = serde_json::from_str(stdout).map_err(|e| e.to_string())?;
    let results = report
        .get("results")
        .and_then(Json::as_array)
        .ok_or("no results")?;
    let mut hits = Vec::new();
    for result in results {
        let path = text(&result["source"], "path");
        for package in array(result, "packages") {
            let name = text(&package["package"], "name");
            let version = text(&package["package"], "version");
            for vuln in array(package, "vulnerabilities") {
                let id = text(vuln, "id");
                let score = array(package, "groups")
                    .iter()
                    .find(|g| array(g, "ids").iter().any(|i| i.as_str() == Some(id)))
                    .map_or("", |g| text(g, "max_severity"));
                hits.push(Hit {
                    id: id.into(),
                    severity: band(score),
                    subject: format!("{name}@{version} in {path}"),
                    detail: text(vuln, "summary").into(),
                });
            }
        }
    }
    Ok(hits)
}

/// trivy's `image --format json`: `Results`, each with its `Vulnerabilities` (null when it has none).
pub fn parse_trivy(stdout: &str) -> Result<Vec<Hit>, String> {
    let report: Json = serde_json::from_str(stdout).map_err(|e| e.to_string())?;
    let results = report
        .get("Results")
        .and_then(Json::as_array)
        .ok_or("no Results")?;
    let mut hits = Vec::new();
    for result in results {
        for vuln in array(result, "Vulnerabilities") {
            hits.push(Hit {
                id: text(vuln, "VulnerabilityID").into(),
                severity: severity(text(vuln, "Severity")),
                subject: format!(
                    "{}@{} in {}",
                    text(vuln, "PkgName"),
                    text(vuln, "InstalledVersion"),
                    text(result, "Target")
                ),
                detail: text(vuln, "Title").into(),
            });
        }
    }
    Ok(hits)
}

/// grype's `-o json`: `matches`, each with its `vulnerability` and the `artifact` it is found in.
pub fn parse_grype(stdout: &str) -> Result<Vec<Hit>, String> {
    let report: Json = serde_json::from_str(stdout).map_err(|e| e.to_string())?;
    let matches = report
        .get("matches")
        .and_then(Json::as_array)
        .ok_or("no matches")?;
    Ok(matches
        .iter()
        .map(|m| {
            let vuln = &m["vulnerability"];
            let artifact = &m["artifact"];
            Hit {
                id: text(vuln, "id").into(),
                severity: severity(text(vuln, "severity")),
                subject: format!("{}@{}", text(artifact, "name"), text(artifact, "version")),
                detail: text(vuln, "dataSource").into(),
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARGO_AUDIT: &str = include_str!("../../tests/fixtures/security/cargo-audit.json");
    const CARGO_DENY: &str = include_str!("../../tests/fixtures/security/cargo-deny.jsonl");
    const NPM_AUDIT: &str = include_str!("../../tests/fixtures/security/npm-audit.json");
    const PIP_AUDIT: &str = include_str!("../../tests/fixtures/security/pip-audit.json");
    const GOVULNCHECK: &str = include_str!("../../tests/fixtures/security/govulncheck.json");
    const OSV: &str = include_str!("../../tests/fixtures/security/osv-scanner.json");
    const TRIVY: &str = include_str!("../../tests/fixtures/security/trivy.json");
    const GRYPE: &str = include_str!("../../tests/fixtures/security/grype.json");

    #[test]
    fn cargo_audit_reads_each_vulnerability_and_not_the_warnings() {
        let hits = parse_cargo_audit(CARGO_AUDIT).unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].id, "RUSTSEC-2024-0001");
        assert_eq!(hits[0].subject, "time");
        assert_eq!(hits[0].severity, "unknown");
        assert!(hits[0].detail.contains("Denial"));
    }

    #[test]
    fn cargo_deny_reads_its_errors_and_ignores_warnings() {
        let hits = parse_cargo_deny(CARGO_DENY).unwrap();
        let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, ["vulnerability", "banned"], "{hits:?}");
        assert!(hits[1].detail.contains("banned"));
        assert!(parse_cargo_deny("not json at all").is_err());
        assert!(parse_cargo_deny("").is_err());
    }

    #[test]
    fn npm_audit_reads_each_vulnerable_package() {
        let hits = parse_npm_audit(NPM_AUDIT).unwrap();
        assert_eq!(hits.len(), 2);
        let lodash = hits.iter().find(|h| h.id == "lodash").unwrap();
        assert_eq!(lodash.severity, "high");
        assert!(lodash.detail.contains("Prototype Pollution"));
        let minimist = hits.iter().find(|h| h.id == "minimist").unwrap();
        assert_eq!(minimist.severity, "critical");
    }

    #[test]
    fn pip_audit_reads_each_vulnerability_of_each_dependency() {
        let hits = parse_pip_audit(PIP_AUDIT).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "PYSEC-2023-74");
        assert_eq!(hits[0].subject, "jinja2==2.11.2");
    }

    #[test]
    fn govulncheck_reads_its_findings_from_the_stream() {
        let hits = parse_govulncheck(GOVULNCHECK).unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].id, "GO-2024-2611");
        assert_eq!(hits[0].subject, "golang.org/x/net");
        assert_eq!(hits[0].detail, "fixed in v0.23.0");
    }

    #[test]
    fn osv_scanner_reads_each_vulnerability_with_its_group_score() {
        let hits = parse_osv(OSV).unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].id, "GHSA-jf85-cpcp-j695");
        assert_eq!(hits[0].severity, "high");
        assert!(hits[0].subject.starts_with("lodash@4.17.20"));
        assert!(parse_osv("{}").is_err());
    }

    #[test]
    fn trivy_and_grype_read_their_image_vulnerabilities() {
        let trivy = parse_trivy(TRIVY).unwrap();
        assert_eq!(trivy.len(), 1);
        assert_eq!(trivy[0].id, "CVE-2024-2222");
        assert_eq!(trivy[0].severity, "high");
        let grype = parse_grype(GRYPE).unwrap();
        assert_eq!(grype.len(), 1);
        assert_eq!(grype[0].id, "CVE-2024-3333");
        assert_eq!(grype[0].severity, "critical");
        assert_eq!(grype[0].subject, "libssl3@3.0.11");
    }

    #[test]
    fn a_missing_audit_tool_is_a_skip_with_its_reason_and_never_a_pass() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = run_audit("cargo-audit", dir.path(), "");
        assert!(!outcome.ran);
        assert!(
            outcome.reason.contains("not on the PATH"),
            "{}",
            outcome.reason
        );
        assert!(outcome.hits.is_empty());
    }

    #[test]
    fn an_audit_of_a_tree_without_its_files_is_a_skip_too() {
        let dir = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        std::fs::write(bin.path().join("npm"), "#!/bin/sh\necho '{}'\n").unwrap();
        let path = bin.path().display().to_string();
        let outcome = run_audit("npm-audit", dir.path(), &path);
        assert!(!outcome.ran);
        assert!(
            outcome.reason.contains("package-lock.json"),
            "{}",
            outcome.reason
        );
    }

    #[test]
    fn an_image_with_no_scanner_on_the_path_is_named_and_skipped() {
        let outcome = run_image("postgres:17", "");
        assert!(!outcome.ran);
        assert!(
            outcome.reason.contains("trivy nor grype"),
            "{}",
            outcome.reason
        );
    }

    #[test]
    fn an_unknown_severity_word_is_unknown_not_guessed() {
        assert_eq!(severity("Moderate"), "medium");
        assert_eq!(severity("CRITICAL"), "critical");
        assert_eq!(severity("whatever"), "unknown");
        assert_eq!(band("7.5"), "high");
        assert_eq!(band(""), "unknown");
    }
}
