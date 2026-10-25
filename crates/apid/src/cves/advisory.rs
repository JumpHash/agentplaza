//! Advisories, normalised (PLAN 24.3): one shape for what OSV, GitHub's advisory database, RustSec and the ecosystems'
//! audits say. Each advisory names its ids and aliases (the first is its own), its ecosystem and package, the versions
//! it affects and the fix, its severity and CVSS, and the vulnerable functions when the source names them. Versions
//! are compared with the ecosystem's own ordering ([`version`]): a range that excludes a version never matches it.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use super::version;
use crate::upgrades::ecosystem::Ecosystem;

/// A version bound: the version, and whether the bound itself is in the range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bound {
    pub version: String,
    pub inclusive: bool,
}

/// A range of versions, each bound optional (an open end).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interval {
    #[serde(default)]
    pub lo: Option<Bound>,
    #[serde(default)]
    pub hi: Option<Bound>,
}

impl Interval {
    /// Whether `v` lies in the range.
    pub fn contains(&self, eco: Ecosystem, v: &str) -> bool {
        let above = match &self.lo {
            None => true,
            Some(b) => match version::compare(eco, v, &b.version) {
                Ordering::Greater => true,
                Ordering::Equal => b.inclusive,
                Ordering::Less => false,
            },
        };
        let below = match &self.hi {
            None => true,
            Some(b) => match version::compare(eco, v, &b.version) {
                Ordering::Less => true,
                Ordering::Equal => b.inclusive,
                Ordering::Greater => false,
            },
        };
        above && below
    }
}

/// Which versions an advisory affects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Affects {
    /// Affected within these ranges (OSV's events, GitHub's `vulnerable_version_range`).
    Within { ranges: Vec<Interval> },
    /// Affected everywhere but in these ranges (RustSec: the patched and unaffected requirements).
    Outside { safe: Vec<Interval> },
    /// Affected at exactly these versions (a list the source gives).
    Exact { versions: Vec<String> },
    /// An audit saw the package vulnerable in `main`'s tree, and names no version: it is matched to `main`'s lock only.
    Observed,
}

/// One advisory, normalised.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Advisory {
    /// The advisory's ids: the first is its own (`GHSA-…`, `RUSTSEC-…`, `GO-…`, a CVE), the rest its aliases.
    pub ids: Vec<String>,
    pub ecosystem: Ecosystem,
    pub package: String,
    pub title: String,
    /// `critical`, `high`, `medium`, `low` or `unknown`.
    pub severity: String,
    /// The CVSS vector or score as the source gave it (empty when none).
    #[serde(default)]
    pub cvss: String,
    #[serde(default)]
    pub published: String,
    /// The sources it came from (`osv`, `github`, `rustsec`, `audit:<tool>`), joined.
    pub source: String,
    #[serde(default)]
    pub url: String,
    pub affects: Affects,
    /// The vulnerable functions the source names (`tokio::net::TcpStream::connect`, a Go symbol), as written.
    #[serde(default)]
    pub functions: Vec<String>,
}

/// The first id: the advisory's own.
impl Advisory {
    pub fn id(&self) -> &str {
        self.ids.first().map_or("", String::as_str)
    }

    /// Whether `v` (a release of the advisory's package) is affected. An `Observed` advisory answers true: its caller
    /// only asks it about `main`'s lock.
    pub fn affected(&self, v: &str) -> bool {
        match &self.affects {
            Affects::Within { ranges } => ranges.iter().any(|r| r.contains(self.ecosystem, v)),
            Affects::Outside { safe } => !safe.iter().any(|r| r.contains(self.ecosystem, v)),
            Affects::Exact { versions } => versions
                .iter()
                .any(|x| version::compare(self.ecosystem, x, v) == Ordering::Equal),
            Affects::Observed => true,
        }
    }

    /// The release that fixes a vulnerable `v`: the smallest fix after it, from the ranges' upper ends (or the safe
    /// ranges' lower ends). None when the source names no fix after `v`.
    pub fn fix_for(&self, v: &str) -> Option<String> {
        let candidates: Vec<&str> = match &self.affects {
            Affects::Within { ranges } => ranges
                .iter()
                .filter_map(|r| r.hi.as_ref().filter(|b| !b.inclusive))
                .map(|b| b.version.as_str())
                .collect(),
            Affects::Outside { safe } => safe
                .iter()
                .filter_map(|r| r.lo.as_ref().filter(|b| b.inclusive))
                .map(|b| b.version.as_str())
                .collect(),
            Affects::Exact { .. } | Affects::Observed => Vec::new(),
        };
        candidates
            .into_iter()
            .filter(|c| version::compare(self.ecosystem, c, v) == Ordering::Greater)
            .min_by(|a, b| version::compare(self.ecosystem, a, b))
            .map(str::to_string)
    }
}

/// The base score of a CVSS v3.x vector (`CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H`), by the specification's formula.
/// None for a vector it does not read.
pub fn cvss3(vector: &str) -> Option<f64> {
    let mut metrics = std::collections::BTreeMap::new();
    for part in vector.split('/') {
        let (k, v) = part.split_once(':')?;
        metrics.insert(k, v);
    }
    if !metrics.get("CVSS")?.starts_with('3') {
        return None;
    }
    let changed = metrics.get("S")? == &"C";
    let av: f64 = match *metrics.get("AV")? {
        "N" => 0.85,
        "A" => 0.62,
        "L" => 0.55,
        "P" => 0.2,
        _ => return None,
    };
    let ac = match *metrics.get("AC")? {
        "L" => 0.77,
        "H" => 0.44,
        _ => return None,
    };
    let pr = match (*metrics.get("PR")?, changed) {
        ("N", _) => 0.85,
        ("L", false) => 0.62,
        ("L", true) => 0.68,
        ("H", false) => 0.27,
        ("H", true) => 0.5,
        _ => return None,
    };
    let ui = match *metrics.get("UI")? {
        "N" => 0.85,
        "R" => 0.62,
        _ => return None,
    };
    let impact_of = |m: &str| match m {
        "H" => Some(0.56),
        "L" => Some(0.22),
        "N" => Some(0.0),
        _ => None,
    };
    let c = impact_of(metrics.get("C")?)?;
    let i = impact_of(metrics.get("I")?)?;
    let a = impact_of(metrics.get("A")?)?;
    let iss: f64 = 1.0 - (1.0 - c) * (1.0 - i) * (1.0 - a);
    let impact: f64 = if changed {
        7.52 * (iss - 0.029) - 3.25 * (iss - 0.02).powf(15.0)
    } else {
        6.42 * iss
    };
    let exploitability: f64 = 8.22 * av * ac * pr * ui;
    if impact <= 0.0 {
        return Some(0.0);
    }
    let total: f64 = if changed {
        (1.08 * (impact + exploitability)).min(10.0)
    } else {
        (impact + exploitability).min(10.0)
    };
    // The specification's round-up: to one decimal, never below the exact value.
    Some(((total * 10.0) - 1e-9).ceil() / 10.0)
}

/// The severity a CVSS v3 score names (the specification's bands).
pub fn band(score: f64) -> &'static str {
    match score {
        s if s >= 9.0 => "critical",
        s if s >= 7.0 => "high",
        s if s >= 4.0 => "medium",
        s if s > 0.0 => "low",
        _ => "unknown",
    }
}

/// A severity's rank: higher is worse.
pub fn rank(severity: &str) -> u8 {
    match severity {
        "critical" => 4,
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0,
    }
}

/// Merges advisories that share an id or an alias for the same package into one, in the order they came. The merged
/// advisory keeps the first concrete range it has (a source's ranges beat an audit's name-only observation), the worst
/// severity, every id, source and function.
pub fn merge(advisories: Vec<Advisory>) -> Vec<Advisory> {
    let mut groups: Vec<Vec<Advisory>> = Vec::new();
    for adv in advisories {
        let joins = groups.iter().position(|group| {
            group.iter().any(|g| {
                g.ecosystem == adv.ecosystem
                    && g.package == adv.package
                    && g.ids.iter().any(|id| adv.ids.contains(id))
            })
        });
        match joins {
            Some(at) => groups[at].push(adv),
            None => groups.push(vec![adv]),
        }
    }
    groups
        .into_iter()
        .map(|mut group| {
            let mut merged = group.remove(0);
            for other in group {
                for id in other.ids {
                    if !merged.ids.contains(&id) {
                        merged.ids.push(id);
                    }
                }
                if rank(&other.severity) > rank(&merged.severity) {
                    merged.severity = other.severity.clone();
                }
                let concrete = |a: &Affects| !matches!(a, Affects::Observed);
                if !concrete(&merged.affects) && concrete(&other.affects) {
                    merged.affects = other.affects.clone();
                }
                if merged.title.is_empty() {
                    merged.title = other.title.clone();
                }
                if merged.cvss.is_empty() {
                    merged.cvss = other.cvss.clone();
                }
                if merged.published.is_empty() {
                    merged.published = other.published.clone();
                }
                if merged.url.is_empty() {
                    merged.url = other.url.clone();
                }
                for source in other.source.split(", ") {
                    if !merged.source.split(", ").any(|s| s == source) {
                        merged.source = format!("{}, {source}", merged.source);
                    }
                }
                for f in other.functions {
                    if !merged.functions.contains(&f) {
                        merged.functions.push(f);
                    }
                }
            }
            merged
        })
        .collect()
}

/// Parses one requirement (`>= 1.2.0, < 1.3`, `^1.2`, `~1.2.3`, `=1.0.0`, `*`) into the range it names. The comparators
/// of a requirement are one range (their intersection).
pub fn requirement(eco: Ecosystem, text: &str) -> Result<Interval, String> {
    let mut range = Interval::default();
    let mut any = false;
    for part in text.split(',') {
        let part = part.trim();
        if part.is_empty() || part == "*" {
            continue;
        }
        any = true;
        let (op, v) = split_op(part);
        let v = v.trim();
        if v.is_empty() {
            return Err(format!("`{part}` names no version"));
        }
        let piece = match op {
            ">=" => Interval {
                lo: Some(bound(v, true)),
                hi: None,
            },
            ">" => Interval {
                lo: Some(bound(v, false)),
                hi: None,
            },
            "<=" => Interval {
                lo: None,
                hi: Some(bound(v, true)),
            },
            "<" => Interval {
                lo: None,
                hi: Some(bound(v, false)),
            },
            "=" | "==" => Interval {
                lo: Some(bound(v, true)),
                hi: Some(bound(v, true)),
            },
            "^" => caret(v),
            "~" => tilde(v),
            other => return Err(format!("`{other}` is not a comparator")),
        };
        range = intersect(eco, range, piece);
    }
    if !any {
        return Ok(Interval::default());
    }
    Ok(range)
}

fn split_op(part: &str) -> (&str, &str) {
    for op in [">=", "<=", "==", ">", "<", "=", "^", "~"] {
        if let Some(rest) = part.strip_prefix(op) {
            return (op, rest);
        }
    }
    ("=", part)
}

fn bound(v: &str, inclusive: bool) -> Bound {
    Bound {
        version: v.trim_start_matches('v').to_string(),
        inclusive,
    }
}

/// `^X.Y.Z`: from the version up to the next change of the first non-zero number.
fn caret(v: &str) -> Interval {
    let parts: Vec<u64> = v.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    let (x, y) = (
        parts.first().copied().unwrap_or(0),
        parts.get(1).copied().unwrap_or(0),
    );
    let upper = if x > 0 {
        format!("{}.0.0", x + 1)
    } else if y > 0 {
        format!("0.{}.0", y + 1)
    } else {
        let z = parts.get(2).copied().unwrap_or(0);
        format!("0.0.{}", z + 1)
    };
    Interval {
        lo: Some(bound(v, true)),
        hi: Some(bound(&upper, false)),
    }
}

/// `~X.Y.Z`: from the version up to the next minor release (or the next major, for `~X`).
fn tilde(v: &str) -> Interval {
    let parts: Vec<u64> = v.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    let upper = if parts.len() >= 2 {
        format!("{}.{}.0", parts[0], parts[1] + 1)
    } else {
        format!("{}.0.0", parts.first().copied().unwrap_or(0) + 1)
    };
    Interval {
        lo: Some(bound(v, true)),
        hi: Some(bound(&upper, false)),
    }
}

/// The intersection of two ranges: the higher lower bound, the lower upper bound.
fn intersect(eco: Ecosystem, a: Interval, b: Interval) -> Interval {
    let lo = match (a.lo, b.lo) {
        (Some(x), Some(y)) => match version::compare(eco, &x.version, &y.version) {
            Ordering::Less => Some(y),
            Ordering::Greater => Some(x),
            Ordering::Equal => Some(Bound {
                version: x.version,
                inclusive: x.inclusive && y.inclusive,
            }),
        },
        (x, y) => x.or(y),
    };
    let hi = match (a.hi, b.hi) {
        (Some(x), Some(y)) => match version::compare(eco, &x.version, &y.version) {
            Ordering::Less => Some(x),
            Ordering::Greater => Some(y),
            Ordering::Equal => Some(Bound {
                version: x.version,
                inclusive: x.inclusive && y.inclusive,
            }),
        },
        (x, y) => x.or(y),
    };
    Interval { lo, hi }
}

/// OSV's `events` of one range, read in version order into the ranges they open and close. `introduced` opens one,
/// `fixed` and `limit` close it before (the fix is not affected), `last_affected` closes it at the version.
pub fn events(eco: Ecosystem, events: &[(String, String)]) -> Vec<Interval> {
    let mut sorted: Vec<&(String, String)> = events.iter().collect();
    sorted.sort_by(|a, b| version::compare(eco, &a.1, &b.1));
    let mut out = Vec::new();
    let mut open: Option<Bound> = None;
    for (kind, v) in sorted {
        match kind.as_str() {
            "introduced" => {
                if open.is_none() {
                    open = Some(bound(v, true));
                }
            }
            "fixed" | "limit" => {
                out.push(Interval {
                    lo: open.take(),
                    hi: Some(bound(v, false)),
                });
            }
            "last_affected" => {
                out.push(Interval {
                    lo: open.take(),
                    hi: Some(bound(v, true)),
                });
            }
            _ => {}
        }
    }
    if open.is_some() {
        out.push(Interval { lo: open, hi: None });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adv(ranges: Vec<Interval>) -> Advisory {
        Advisory {
            ids: vec!["GHSA-x".into()],
            ecosystem: Ecosystem::Cargo,
            package: "h2".into(),
            title: String::new(),
            severity: "high".into(),
            cvss: String::new(),
            published: String::new(),
            source: "osv".into(),
            url: String::new(),
            affects: Affects::Within { ranges },
            functions: Vec::new(),
        }
    }

    #[test]
    fn events_open_and_close_ranges() {
        let ev = vec![
            ("introduced".to_string(), "0".to_string()),
            ("fixed".to_string(), "0.3.26".to_string()),
            ("introduced".to_string(), "0.4.0".to_string()),
            ("last_affected".to_string(), "0.4.2".to_string()),
        ];
        let ranges = events(Ecosystem::Cargo, &ev);
        assert_eq!(ranges.len(), 2);
        let a = adv(ranges);
        assert!(a.affected("0.3.9"));
        assert!(!a.affected("0.3.26"));
        assert!(!a.affected("0.4.3"));
        assert!(a.affected("0.4.2"));
        assert_eq!(a.fix_for("0.3.9").as_deref(), Some("0.3.26"));
    }

    #[test]
    fn a_range_that_excludes_the_version_does_not_match() {
        let a = adv(vec![
            requirement(Ecosystem::Cargo, ">= 0.1.0, < 0.2.0").unwrap(),
        ]);
        assert!(a.affected("0.1.5"));
        assert!(!a.affected("0.2.0"));
        assert!(!a.affected("0.3.0"));
        assert_eq!(a.fix_for("0.1.5").as_deref(), Some("0.2.0"));
        assert_eq!(a.fix_for("0.3.0"), None);
    }

    #[test]
    fn requirements_read_carets_and_tildes() {
        let eco = Ecosystem::Cargo;
        let caret = requirement(eco, "^1.2").unwrap();
        assert!(caret.contains(eco, "1.9.0"));
        assert!(!caret.contains(eco, "2.0.0"));
        let tilde = requirement(eco, "~1.2.3").unwrap();
        assert!(tilde.contains(eco, "1.2.9"));
        assert!(!tilde.contains(eco, "1.3.0"));
        assert!(requirement(eco, "*").unwrap().contains(eco, "9.9.9"));
    }

    #[test]
    fn safe_ranges_mean_affected_everywhere_else() {
        let safe = requirement(Ecosystem::Cargo, ">= 1.2.3").unwrap();
        let a = Advisory {
            affects: Affects::Outside { safe: vec![safe] },
            ..adv(Vec::new())
        };
        assert!(a.affected("1.0.0"));
        assert!(!a.affected("1.2.3"));
        assert_eq!(a.fix_for("1.0.0").as_deref(), Some("1.2.3"));
    }

    #[test]
    fn cvss_scores_match_the_specification() {
        // Log4Shell's vector scores 10.0; an unauthenticated network flaw with low confidentiality impact scores 5.3.
        let critical = cvss3("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:C/C:H/I:H/A:H").unwrap();
        assert_eq!(critical, 10.0);
        assert_eq!(band(critical), "critical");
        let medium = cvss3("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:L/I:N/A:N").unwrap();
        assert_eq!(medium, 5.3);
        assert_eq!(band(medium), "medium");
        assert_eq!(cvss3("CVSS:2.0/AV:N"), None);
    }

    #[test]
    fn merge_joins_advisories_sharing_an_alias() {
        let mut osv = adv(vec![]);
        osv.ids = vec!["GHSA-x".into(), "CVE-2024-1".into()];
        osv.severity = "medium".into();
        let mut rustsec = adv(vec![requirement(Ecosystem::Cargo, "< 1.0").unwrap()]);
        rustsec.ids = vec!["RUSTSEC-2024-0001".into(), "CVE-2024-1".into()];
        rustsec.source = "rustsec".into();
        rustsec.functions = vec!["h2::Frame::read".into()];
        let merged = merge(vec![osv, rustsec]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].ids.len(), 3);
        assert_eq!(merged[0].source, "osv, rustsec");
        assert_eq!(merged[0].severity, "high");
        assert_eq!(merged[0].functions, vec!["h2::Frame::read".to_string()]);
    }
}
