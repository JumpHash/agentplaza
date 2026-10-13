//! The prompt library's core (docs/design/prompt-library.md): best-practice checks and a score for system prompts,
//! AGENTS.md files and fragments, a line diff between versions, `{{variable}}` substitution and slugs. Pure and
//! deterministic: apid checks every stored version with it, and the desktop runs it natively while the owner types.

mod diff;
mod doc;
mod rules_common;
mod rules_kind;
mod vars;

use serde::{Deserialize, Serialize};

pub use diff::{diff, DiffLine, Op};
pub use doc::{Block, Doc};
pub use vars::{render, slug, unknown_vars, VARIABLES};

/// Bumped whenever a rule is added, removed or changes what it reports. Stored with every scored version.
pub const RULES_VERSION: u32 = 1;

/// What a prompt is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A role's system prompt.
    System,
    /// An AGENTS.md: house rules for agents, after the agents.md convention.
    AgentsMd,
    /// A reusable piece of text.
    Fragment,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::System, Kind::AgentsMd, Kind::Fragment];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::System => "system",
            Kind::AgentsMd => "agents_md",
            Kind::Fragment => "fragment",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }

    pub fn parse(s: &str) -> Option<Severity> {
        [Severity::Error, Severity::Warning, Severity::Info].into_iter().find(|v| v.as_str() == s)
    }
}

/// One thing a check found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// The rule's id, one of `rules()`.
    pub rule: String,
    pub severity: Severity,
    /// 1-based line in the body; 0 for the whole document.
    pub line: u32,
    /// One sentence, at most 300 characters.
    pub message: String,
}

impl Finding {
    pub fn new(rule: &str, severity: Severity, line: u32, message: impl Into<String>) -> Self {
        let mut message: String = message.into();
        if message.chars().count() > 300 {
            message = message.chars().take(299).collect::<String>() + "…";
        }
        Finding { rule: rule.to_string(), severity, line, message }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    pub chars: u32,
    pub words: u32,
    pub lines: u32,
    pub headings: u32,
    /// Characters / 4, rounded up: a rough token count for every model.
    pub est_tokens: u32,
}

/// The result of `check`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub kind: Kind,
    pub rules_version: u32,
    /// Sorted by line, then severity, then rule.
    pub findings: Vec<Finding>,
    pub score: u8,
    pub errors: u32,
    pub warnings: u32,
    pub infos: u32,
    pub stats: Stats,
}

impl Report {
    /// Whether the version may be published (no error-level finding).
    pub fn publishable(&self) -> bool {
        self.errors == 0
    }
}

/// One rule in the catalogue.
#[derive(Debug, Clone, Serialize)]
pub struct Rule {
    pub id: &'static str,
    /// The usual severity (`too-long` and `agents-md-sections` also report at another level, as their `why` says).
    pub severity: Severity,
    /// The kinds it applies to; empty means every kind.
    pub kinds: &'static [Kind],
    pub title: &'static str,
    /// Why it matters, in one or two sentences: the best practice behind it.
    pub why: &'static str,
}

impl Rule {
    pub fn applies_to(&self, kind: Kind) -> bool {
        self.kinds.is_empty() || self.kinds.contains(&kind)
    }
}

/// The rule catalogue, in the order of the design doc.
pub fn rules() -> Vec<Rule> {
    let mut all = rules_common::RULES.to_vec();
    all.extend_from_slice(rules_kind::RULES);
    all
}

/// `100 - 25 × errors - 8 × warnings - 2 × infos`, floored at 0.
pub fn score(errors: u32, warnings: u32, infos: u32) -> u8 {
    let lost = 25 * errors as i64 + 8 * warnings as i64 + 2 * infos as i64;
    (100 - lost).clamp(0, 100) as u8
}

/// Runs every rule that applies to `kind` over `body`.
pub fn check(kind: Kind, body: &str) -> Report {
    let doc = Doc::parse(body);
    let mut findings = rules_common::check(kind, &doc);
    findings.extend(rules_kind::check(kind, &doc));
    findings.sort_by(|a, b| (a.line, a.severity, &a.rule).cmp(&(b.line, b.severity, &b.rule)));
    let count = |s: Severity| findings.iter().filter(|f| f.severity == s).count() as u32;
    let (errors, warnings, infos) = (count(Severity::Error), count(Severity::Warning), count(Severity::Info));
    let chars = body.chars().count() as u32;
    let stats = Stats {
        chars,
        words: body.split_whitespace().count() as u32,
        lines: doc.lines.len() as u32,
        headings: doc.headings().count() as u32,
        est_tokens: chars.div_ceil(4),
    };
    Report { kind, rules_version: RULES_VERSION, findings, score: score(errors, warnings, infos), errors, warnings, infos, stats }
}

/// The starter body for a new prompt of `kind`. Each one passes every check (score 100).
pub fn template(kind: Kind) -> &'static str {
    match kind {
        Kind::System => include_str!("templates/system.md"),
        Kind::AgentsMd => include_str!("templates/agents_md.md"),
        Kind::Fragment => include_str!("templates/fragment.md"),
    }
}

/// The best of several scored versions: the highest score, ties going to the newest (largest version number).
pub fn best_version(scores: impl IntoIterator<Item = (u32, u8)>) -> Option<u32> {
    scores.into_iter().max_by_key(|&(version, score)| (score, version)).map(|(version, _)| version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_floors_at_zero() {
        assert_eq!(score(0, 0, 0), 100);
        assert_eq!(score(1, 1, 1), 65);
        assert_eq!(score(5, 0, 0), 0);
    }

    #[test]
    fn best_version_prefers_score_then_newest() {
        assert_eq!(best_version([(1, 80), (2, 90), (3, 90)]), Some(3));
        assert_eq!(best_version([(1, 95), (2, 90)]), Some(1));
        assert_eq!(best_version([]), None);
    }

    #[test]
    fn kinds_round_trip_and_serialise_as_their_names() {
        for kind in Kind::ALL {
            assert_eq!(Kind::parse(kind.as_str()), Some(kind));
            assert_eq!(serde_json::to_string(&kind).unwrap(), format!("\"{}\"", kind.as_str()));
        }
    }

    #[test]
    fn templates_pass_every_check() {
        for kind in Kind::ALL {
            let report = check(kind, template(kind));
            assert_eq!(report.score, 100, "{}: {:?}", kind.as_str(), report.findings);
        }
    }

    #[test]
    fn every_rule_id_is_unique_and_kebab_case() {
        let rules = rules();
        let mut ids: Vec<_> = rules.iter().map(|r| r.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), rules.len());
        assert!(ids.iter().all(|id| id.chars().all(|c| c.is_ascii_lowercase() || c == '-')));
    }

    #[test]
    fn every_finding_names_a_catalogue_rule() {
        let ids: Vec<_> = rules().iter().map(|r| r.id).collect();
        let samples = [
            "",
            "no heading\n\n```\nunclosed",
            "# T\n\n### skip\n\nTry to do things etc. {{nope}} sk-abcdefghijklmnopqrstuvwxyz012345\n",
        ];
        for kind in Kind::ALL {
            for body in samples {
                for f in check(kind, body).findings {
                    assert!(ids.contains(&f.rule.as_str()), "unknown rule {}", f.rule);
                }
            }
        }
    }
}
