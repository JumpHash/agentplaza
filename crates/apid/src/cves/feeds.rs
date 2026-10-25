//! The advisory feeds (PLAN 24.3): OSV's API, GitHub's advisory database, RustSec's advisory-db and the ecosystems' audits
//! (10.8's parsers). Each feed's documents are read into [`Advisory`]s; the network calls go through the glue's
//! [`Mediator`] (the feed's manifest names its hosts), so a feed reaches only the hosts it declares and nothing else. The
//! tests put a fake server behind [`Outbound`]; production uses the hub's HTTPS client.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value as Json;

use super::advisory::{self, Advisory, Affects, Interval};
use crate::connectors::manifest;
use crate::glue::{BODY_CAP, Mediator, Outbound};
use crate::upgrades::ecosystem::Ecosystem;
use apz_topology::security::{Hit, audit};

/// The feeds' manifests' ids (`connectors/manifests/feeds/`) and GitHub's catalog connector.
pub const OSV: &str = "osv";
pub const RUSTSEC: &str = "rustsec";
pub const GITHUB: &str = "github";

/// The most pages of an OSV query a poll reads for one package.
const MAX_PAGES: usize = 20;

/// The egress hosts a feed's manifest declares.
pub fn egress(connector: &str) -> Vec<String> {
    let mut all = manifest::builtin();
    all.extend(manifest::feeds());
    all.into_iter()
        .find(|m| m.id == connector)
        .map(|m| m.egress)
        .unwrap_or_default()
}

/// OSV's name of an ecosystem, and back.
pub fn osv_name(eco: Ecosystem) -> Option<&'static str> {
    match eco {
        Ecosystem::Cargo => Some("crates.io"),
        Ecosystem::Npm => Some("npm"),
        Ecosystem::Pip => Some("PyPI"),
        Ecosystem::Go => Some("Go"),
        _ => None,
    }
}

fn osv_ecosystem(name: &str) -> Option<Ecosystem> {
    match name {
        "crates.io" => Some(Ecosystem::Cargo),
        "npm" => Some(Ecosystem::Npm),
        "PyPI" => Some(Ecosystem::Pip),
        "Go" => Some(Ecosystem::Go),
        _ => None,
    }
}

/// GitHub's name of an ecosystem (its advisory database's `ecosystem` values), and back.
pub fn github_name(eco: Ecosystem) -> Option<&'static str> {
    match eco {
        Ecosystem::Cargo => Some("rust"),
        Ecosystem::Npm => Some("npm"),
        Ecosystem::Pip => Some("pip"),
        Ecosystem::Go => Some("go"),
        _ => None,
    }
}

fn github_ecosystem(name: &str) -> Option<Ecosystem> {
    match name {
        "rust" => Some(Ecosystem::Cargo),
        "npm" => Some(Ecosystem::Npm),
        "pip" => Some(Ecosystem::Pip),
        "go" => Some(Ecosystem::Go),
        _ => None,
    }
}

fn text<'a>(value: &'a Json, key: &str) -> &'a str {
    value.get(key).and_then(Json::as_str).unwrap_or_default()
}

fn strings(value: &Json, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .map(str::to_string)
        .collect()
}

/// One OSV document (the schema of osv.dev's `Vulnerability`): one advisory per `affected` entry whose ecosystem this
/// reads. An entry with neither ranges nor versions claims nothing, so it gives none.
pub fn osv(doc: &Json) -> Vec<Advisory> {
    let id = text(doc, "id").to_string();
    if id.is_empty() {
        return Vec::new();
    }
    let mut ids = vec![id.clone()];
    for alias in strings(doc, "aliases") {
        if !ids.contains(&alias) {
            ids.push(alias);
        }
    }
    let cvss = doc
        .get("severity")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .find(|s| text(s, "type").starts_with("CVSS"))
        .map(|s| text(s, "score").to_string())
        .unwrap_or_default();
    let severity = match text(&doc["database_specific"], "severity") {
        "" => advisory::cvss3(&cvss)
            .map(advisory::band)
            .unwrap_or("unknown")
            .to_string(),
        named => audit::severity(named),
    };
    let url = doc
        .get("references")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .find(|r| text(r, "type") == "ADVISORY" || text(r, "type") == "WEB")
        .map(|r| text(r, "url").to_string())
        .unwrap_or_else(|| format!("https://osv.dev/vulnerability/{id}"));
    let mut out = Vec::new();
    for affected in doc
        .get("affected")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
    {
        let package = &affected["package"];
        let Some(eco) = osv_ecosystem(text(package, "ecosystem")) else {
            continue;
        };
        let mut ranges: Vec<Interval> = Vec::new();
        for range in affected
            .get("ranges")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            if text(range, "type") == "GIT" {
                continue;
            }
            let events: Vec<(String, String)> = range
                .get("events")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
                .filter_map(|e| {
                    e.as_object().and_then(|o| {
                        o.iter()
                            .find_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                    })
                })
                .collect();
            ranges.extend(advisory::events(eco, &events));
        }
        for v in strings(affected, "versions") {
            ranges.push(Interval {
                lo: Some(advisory::Bound {
                    version: v.clone(),
                    inclusive: true,
                }),
                hi: Some(advisory::Bound {
                    version: v,
                    inclusive: true,
                }),
            });
        }
        if ranges.is_empty() {
            continue;
        }
        let mut functions = Vec::new();
        for import in affected["ecosystem_specific"]["imports"]
            .as_array()
            .into_iter()
            .flatten()
        {
            for symbol in strings(import, "symbols") {
                functions.push(format!("{}.{symbol}", text(import, "path")));
            }
        }
        out.push(Advisory {
            ids: ids.clone(),
            ecosystem: eco,
            package: text(package, "name").to_string(),
            title: text(doc, "summary").to_string(),
            severity: severity.clone(),
            cvss: cvss.clone(),
            published: text(doc, "published").to_string(),
            source: OSV.into(),
            url: url.clone(),
            affects: Affects::Within { ranges },
            functions,
        });
    }
    out
}

/// One of GitHub's advisories (`GET /advisories`): one advisory per vulnerable package it names, with the range of
/// versions the package is vulnerable in. A range that does not read is left out.
pub fn github(item: &Json) -> Vec<Advisory> {
    let ghsa = text(item, "ghsa_id").to_string();
    let mut ids = vec![ghsa.clone()];
    let cve = text(item, "cve_id");
    if !cve.is_empty() && !ids.iter().any(|x| x == cve) {
        ids.push(cve.to_string());
    }
    let cvss = item["cvss"]["vector_string"]
        .as_str()
        .map(str::to_string)
        .or_else(|| item["cvss"]["score"].as_f64().map(|s| s.to_string()))
        .unwrap_or_default();
    let severity = match text(item, "severity") {
        "" => "unknown".to_string(),
        named => audit::severity(named),
    };
    let mut out = Vec::new();
    for vuln in item["vulnerabilities"].as_array().into_iter().flatten() {
        let Some(eco) = github_ecosystem(text(&vuln["package"], "ecosystem")) else {
            continue;
        };
        let Ok(range) = advisory::requirement(eco, text(vuln, "vulnerable_version_range")) else {
            continue;
        };
        out.push(Advisory {
            ids: ids.clone(),
            ecosystem: eco,
            package: text(&vuln["package"], "name").to_string(),
            title: text(item, "summary").to_string(),
            severity: severity.clone(),
            cvss: cvss.clone(),
            published: text(item, "published_at").to_string(),
            source: GITHUB.into(),
            url: text(item, "html_url").to_string(),
            affects: Affects::Within {
                ranges: vec![range],
            },
            functions: Vec::new(),
        });
    }
    out
}

/// One RustSec advisory: a Markdown file whose first fenced `toml` block is the advisory (`[advisory]`, `[versions]` with
/// the `patched` and `unaffected` requirements, `[affected.functions]`). A withdrawn advisory is none. Its affected
/// versions are every version outside the patched and unaffected ones.
pub fn rustsec(markdown: &str, package: &str) -> Result<Option<Advisory>, String> {
    let start = markdown
        .find("```toml")
        .ok_or("the advisory has no toml block")?
        + "```toml".len();
    let end = markdown[start..]
        .find("```")
        .ok_or("the advisory's toml block is not closed")?;
    let doc: toml::Table = markdown[start..start + end]
        .parse()
        .map_err(|e| format!("the advisory does not parse: {e}"))?;
    let advisory_table = doc
        .get("advisory")
        .and_then(|a| a.as_table())
        .ok_or("no [advisory] table")?;
    let field = |k: &str| {
        advisory_table
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
    };
    if advisory_table.contains_key("withdrawn") || field("package") != package {
        return Ok(None);
    }
    let id = field("id");
    if id.is_empty() {
        return Err("the advisory has no id".into());
    }
    let mut ids = vec![id.to_string()];
    for alias in advisory_table
        .get("aliases")
        .and_then(|a| a.as_array())
        .into_iter()
        .flatten()
        .filter_map(|a| a.as_str())
    {
        ids.push(alias.to_string());
    }
    let cvss = field("cvss").to_string();
    let severity = advisory::cvss3(&cvss)
        .map(advisory::band)
        .unwrap_or("unknown")
        .to_string();
    let mut safe = Vec::new();
    let versions = doc.get("versions").and_then(|v| v.as_table());
    for key in ["patched", "unaffected"] {
        for req in versions
            .and_then(|v| v.get(key))
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|r| r.as_str())
        {
            safe.push(advisory::requirement(Ecosystem::Cargo, req)?);
        }
    }
    let functions: Vec<String> = doc
        .get("affected")
        .and_then(|a| a.get("functions"))
        .and_then(|f| f.as_table())
        .map(|f| f.keys().cloned().collect())
        .unwrap_or_default();
    let affects = if safe.is_empty() {
        Affects::Within {
            ranges: vec![Interval::default()],
        }
    } else {
        Affects::Outside { safe }
    };
    Ok(Some(Advisory {
        ids,
        ecosystem: Ecosystem::Cargo,
        package: package.to_string(),
        title: field("title").to_string(),
        severity,
        cvss,
        published: field("date").to_string(),
        source: RUSTSEC.into(),
        url: format!("https://rustsec.org/advisories/{id}.html"),
        affects,
        functions,
    }))
}

/// One audit's hits (10.8's parsers, run on `main`): each hit names an advisory and, where its subject carries a name and
/// a version, the exact version it found; a name alone is an observation in `main`'s lock. osv-scanner's subjects name
/// the lock file (`name@version in path`), which gives the ecosystem. A subject that is not a package (cargo-deny's policy
/// messages) is left out.
pub fn audit(tool: &str, hits: &[Hit]) -> Vec<Advisory> {
    let fixed_eco = match tool {
        "cargo-audit" => Some(Ecosystem::Cargo),
        "npm-audit" => Some(Ecosystem::Npm),
        "pip-audit" => Some(Ecosystem::Pip),
        "govulncheck" => Some(Ecosystem::Go),
        "osv-scanner" => None,
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    for hit in hits {
        // osv-scanner names the lock file the package is in (`name@version in path`): its file says the ecosystem.
        let eco = match (fixed_eco, hit.subject.split_once(" in ")) {
            (Some(eco), _) => eco,
            (None, Some((_, path))) => match path.rsplit('/').next().unwrap_or("") {
                "Cargo.lock" => Ecosystem::Cargo,
                "package-lock.json" => Ecosystem::Npm,
                "go.sum" => Ecosystem::Go,
                f if f == "uv.lock" || f == "poetry.lock" || f.starts_with("requirements") => {
                    Ecosystem::Pip
                }
                _ => continue,
            },
            (None, None) => continue,
        };
        let subject = hit.subject.split(" in ").next().unwrap_or("").trim();
        let (name, version) = match subject.split_once("==").or_else(|| subject.split_once('@')) {
            Some((name, version)) => (name.trim(), Some(version.trim().to_string())),
            None => (subject, None),
        };
        if name.is_empty() || name.contains(char::is_whitespace) || hit.id.is_empty() {
            continue;
        }
        let affects = match version {
            Some(v) if !v.is_empty() && !v.contains(char::is_whitespace) => {
                Affects::Exact { versions: vec![v] }
            }
            _ => Affects::Observed,
        };
        out.push(Advisory {
            ids: vec![hit.id.clone()],
            ecosystem: eco,
            package: name.to_string(),
            title: hit.detail.clone(),
            severity: hit.severity.clone(),
            cvss: String::new(),
            published: String::new(),
            source: format!("audit:{tool}"),
            url: String::new(),
            affects,
            functions: Vec::new(),
        });
    }
    out
}

/// The functions govulncheck found reached (its `finding`s whose trace runs from the vulnerable symbol into this
/// project's code: more than one frame). `Ok` is the set of OSV ids reached; an output with no JSON is an error.
pub fn govulncheck_reached(stdout: &str) -> Result<BTreeSet<String>, String> {
    let mut reached = BTreeSet::new();
    let mut objects = 0;
    for value in serde_json::Deserializer::from_str(stdout).into_iter::<Json>() {
        let value = value.map_err(|e| e.to_string())?;
        objects += 1;
        let Some(finding) = value.get("finding") else {
            continue;
        };
        let frames = finding["trace"].as_array().map_or(0, Vec::len);
        if frames > 1 {
            reached.insert(text(finding, "osv").to_string());
        }
    }
    if objects == 0 {
        return Err("no JSON objects".into());
    }
    Ok(reached)
}

// ---- the network ---------------------------------------------------------------------------------------------------

/// A feed's request through the glue's Mediator. `Ok(None)` for a 404 (nothing there); any other failure is an error.
fn call(
    outbound: &dyn Outbound,
    egress: &[String],
    method: &str,
    url: &str,
    body: &str,
) -> Result<Option<String>, String> {
    let mut headers = BTreeMap::new();
    headers.insert("accept".to_string(), "application/json".to_string());
    headers.insert("user-agent".to_string(), "agentplaza".to_string());
    let mediator = Mediator {
        egress,
        secret: None,
        outbound,
        loopback_http: false,
        body_cap: BODY_CAP,
    };
    let reply = mediator.send(method, url, &headers, body)?;
    match reply.status {
        200..=299 => Ok(Some(reply.body)),
        404 => Ok(None),
        status => Err(format!("{url}: HTTP {status}")),
    }
}

/// The result of one feed's poll: how many advisories it gave, or why it failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Polled {
    pub feed: String,
    pub count: usize,
    pub error: String,
}

/// Polls every feed for the packages: OSV's query per package name (its pages), GitHub's advisories per package, and
/// RustSec's advisory-db for the Cargo packages (each crate's directory, read file by file). A feed that fails for one
/// package keeps the others; its error is returned for the feed's record.
pub fn poll(
    outbound: &dyn Outbound,
    packages: &BTreeSet<(Ecosystem, String)>,
) -> (Vec<Advisory>, Vec<Polled>) {
    let mut out = Vec::new();
    let mut polled = Vec::new();
    let osv_hosts = egress(OSV);
    let github_hosts = egress(GITHUB);
    let rustsec_hosts = egress(RUSTSEC);
    let mut osv_error = String::new();
    let mut github_error = String::new();
    let mut rustsec_error = String::new();
    let (mut osv_count, mut github_count, mut rustsec_count) = (0, 0, 0);
    for (eco, name) in packages {
        let Some(osv_eco) = osv_name(*eco) else {
            continue;
        };
        let mut page_token = String::new();
        for _ in 0..MAX_PAGES {
            let mut query =
                serde_json::json!({ "package": { "name": name, "ecosystem": osv_eco } });
            if !page_token.is_empty() {
                query["page_token"] = Json::String(page_token.clone());
            }
            match call(
                outbound,
                &osv_hosts,
                "POST",
                "https://api.osv.dev/v1/query",
                &query.to_string(),
            ) {
                Ok(Some(body)) => {
                    let page: Json = match serde_json::from_str(&body) {
                        Ok(page) => page,
                        Err(e) => {
                            osv_error = format!("OSV's reply for {name} does not parse: {e}");
                            break;
                        }
                    };
                    for doc in page["vulns"].as_array().into_iter().flatten() {
                        let found = osv(doc);
                        osv_count += found.len();
                        out.extend(found.into_iter().filter(|a| a.package == *name));
                    }
                    page_token = text(&page, "next_page_token").to_string();
                    if page_token.is_empty() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    osv_error = e;
                    break;
                }
            }
        }
        if let Some(gh) = github_name(*eco) {
            let url = format!(
                "https://api.github.com/advisories?ecosystem={gh}&affects={}&per_page=100",
                percent(name)
            );
            match call(outbound, &github_hosts, "GET", &url, "") {
                Ok(Some(body)) => match serde_json::from_str::<Json>(&body) {
                    Ok(items) => {
                        for item in items.as_array().into_iter().flatten() {
                            let found = github(item);
                            github_count += found.len();
                            out.extend(found.into_iter().filter(|a| a.package == *name));
                        }
                    }
                    Err(e) => github_error = format!("GitHub's reply does not parse: {e}"),
                },
                Ok(None) => {}
                Err(e) => github_error = e,
            }
        }
        if *eco == Ecosystem::Cargo {
            let url = format!(
                "https://api.github.com/repos/RustSec/advisory-db/contents/crates/{}",
                percent(name)
            );
            match call(outbound, &rustsec_hosts, "GET", &url, "") {
                Ok(Some(listing)) => {
                    let files: Json = serde_json::from_str(&listing).unwrap_or(Json::Null);
                    for file in files.as_array().into_iter().flatten() {
                        let Some(raw) = file["download_url"].as_str() else {
                            continue;
                        };
                        match call(outbound, &rustsec_hosts, "GET", raw, "") {
                            Ok(Some(md)) => match rustsec(&md, name) {
                                Ok(Some(found)) => {
                                    rustsec_count += 1;
                                    out.push(found);
                                }
                                Ok(None) => {}
                                Err(e) => rustsec_error = format!("{}: {e}", text(file, "name")),
                            },
                            Ok(None) => {}
                            Err(e) => rustsec_error = e,
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => rustsec_error = e,
            }
        }
    }
    polled.push(Polled {
        feed: OSV.into(),
        count: osv_count,
        error: osv_error,
    });
    polled.push(Polled {
        feed: GITHUB.into(),
        count: github_count,
        error: github_error,
    });
    polled.push(Polled {
        feed: RUSTSEC.into(),
        count: rustsec_count,
        error: rustsec_error,
    });
    (out, polled)
}

/// A name in a URL: its path and query characters escaped (`@scope/name` keeps no `/` or `@`).
fn percent(name: &str) -> String {
    weft::server::percent_encode(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osv_documents_read_into_ranges_and_aliases() {
        let doc: Json = serde_json::from_str(include_str!("fixtures/osv_h2.json")).unwrap();
        let found = osv(&doc);
        assert_eq!(found.len(), 1);
        let a = &found[0];
        assert_eq!(
            a.ids,
            vec![
                "RUSTSEC-2024-0003".to_string(),
                "CVE-2024-27316".to_string()
            ]
        );
        assert_eq!(a.severity, "high");
        assert_eq!(a.fix_for("0.3.9").as_deref(), Some("0.3.26"));
        assert!(a.affected("0.3.0"));
        assert!(!a.affected("0.4.3"));
    }

    #[test]
    fn github_advisories_read_their_vulnerable_ranges() {
        let items: Json =
            serde_json::from_str(include_str!("fixtures/github_advisories.json")).unwrap();
        let found: Vec<Advisory> = items.as_array().unwrap().iter().flat_map(github).collect();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].ids[0], "GHSA-qx2v-8332-m7x4");
        assert_eq!(found[0].severity, "high");
        assert_eq!(found[0].fix_for("0.3.9").as_deref(), Some("0.3.26"));
    }

    #[test]
    fn rustsec_markdown_reads_patched_ranges_and_functions() {
        let md = include_str!("fixtures/rustsec_h2.md");
        let found = rustsec(md, "h2").unwrap().unwrap();
        assert_eq!(found.id(), "RUSTSEC-2024-0003");
        assert_eq!(found.functions, vec!["h2::codec::read_frame".to_string()]);
        assert!(found.affected("0.3.9"));
        assert!(!found.affected("0.3.26"));
        assert_eq!(found.fix_for("0.3.9").as_deref(), Some("0.3.26"));
        assert_eq!(rustsec(md, "tokio").unwrap(), None);
    }

    #[test]
    fn audit_hits_name_their_versions_when_they_have_them() {
        let hits = [
            Hit {
                id: "RUSTSEC-2024-0003".into(),
                severity: "high".into(),
                subject: "h2".into(),
                detail: "a flaw".into(),
            },
            Hit {
                id: "GO-2024-2611".into(),
                severity: "unknown".into(),
                subject: "golang.org/x/net@v0.20.0 in go.sum".into(),
                detail: String::new(),
            },
            Hit {
                id: "deny".into(),
                severity: "high".into(),
                subject: "a policy message with spaces".into(),
                detail: String::new(),
            },
        ];
        let cargo = audit("cargo-audit", &[hits[0].clone(), hits[2].clone()]);
        assert_eq!(cargo.len(), 1);
        assert_eq!(cargo[0].affects, Affects::Observed);
        let osv_scan = audit("osv-scanner", &hits[1..2]);
        assert_eq!(osv_scan[0].ecosystem, Ecosystem::Go);
        assert_eq!(
            osv_scan[0].affects,
            Affects::Exact {
                versions: vec!["v0.20.0".into()]
            }
        );
    }

    #[test]
    fn govulncheck_reaches_only_findings_with_a_call_stack() {
        let out = include_str!("fixtures/govulncheck.json");
        let reached = govulncheck_reached(out).unwrap();
        assert!(reached.contains("GO-2024-2611"));
        let symbol_only = r#"{"finding":{"osv":"GO-1","trace":[{"module":"m","function":"F"}]}}"#;
        assert!(govulncheck_reached(symbol_only).unwrap().is_empty());
    }
}
