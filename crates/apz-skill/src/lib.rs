//! The skill library's core (docs/design/skill-library.md): parsing SKILL.md (frontmatter and body), best-practice
//! checks over a skill's SKILL.md and files with a score, the starter template, zip import and export, and GitHub
//! URLs. Pure and deterministic; network and storage live in apid.

pub mod archive;
mod frontmatter;
mod rules;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use apz_prompt::Severity;
pub use archive::{find_skills, parse_github_url, read_zip, write_zip, Entry, FoundSkill, GithubRef, Limits};

/// Bumped whenever a rule is added, removed or changes what it reports.
pub const RULES_VERSION: u32 = 1;

/// The frontmatter of a SKILL.md.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frontmatter {
    pub name: String,
    pub description: String,
    pub license: Option<String>,
    /// `allowed-tools`: a list, or a comma or space separated string.
    pub allowed_tools: Vec<String>,
    /// `metadata:` one level of string values.
    pub metadata: BTreeMap<String, String>,
    /// Any other top-level key, as written.
    pub extra: BTreeMap<String, String>,
}

/// A parsed SKILL.md.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub frontmatter: Frontmatter,
    pub body: String,
    /// 1-based line in SKILL.md where the body starts (after the closing `---`).
    pub body_line: u32,
}

/// Parses SKILL.md: a `---` frontmatter block (a YAML subset: `key: value`, quoted values, `key:` followed by `- item`
/// lists or one level of `  sub: value` maps) and the body. Err with a one-line reason and the line when it does
/// not parse.
pub fn parse(skill_md: &str) -> Result<Parsed, (u32, String)> {
    frontmatter::parse(skill_md)
}

/// One file of a skill, as the checks see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    /// Relative to the skill's folder, `/`-separated, never `SKILL.md` itself.
    pub path: String,
    pub size: u64,
    pub executable: bool,
    /// The content when it is UTF-8 text of at most 1 MB (used to find references and secrets); None otherwise.
    pub text: Option<String>,
}

/// One thing a check found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub rule: String,
    pub severity: Severity,
    /// `SKILL.md` or the file's path.
    pub path: String,
    /// 1-based line in that file; 0 for the whole file or skill.
    pub line: u32,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    pub files: u32,
    pub total_bytes: u64,
    pub body_lines: u32,
    pub est_tokens: u32,
    pub scripts: u32,
}

/// The result of `check`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub rules_version: u32,
    pub findings: Vec<Finding>,
    pub score: u8,
    pub errors: u32,
    pub warnings: u32,
    pub infos: u32,
    pub stats: Stats,
    /// The parsed name and description, when the frontmatter parsed.
    pub name: Option<String>,
    pub description: Option<String>,
}

impl Report {
    pub fn publishable(&self) -> bool {
        self.errors == 0
    }
}

/// One rule in the catalogue.
#[derive(Debug, Clone, Serialize)]
pub struct Rule {
    pub id: &'static str,
    pub severity: Severity,
    pub title: &'static str,
    pub why: &'static str,
}

/// The skill rules, then the prompt rules that run over the body (`apz_prompt::rules()` for kind fragment).
pub fn rules() -> Vec<Rule> {
    rules::RULES.to_vec()
}

/// Runs every skill rule and `apz_prompt::check(Fragment, body)` (its findings get `path: "SKILL.md"` and line numbers
/// shifted to SKILL.md lines). Sorted by path, line, severity, rule.
pub fn check(skill_md: &str, files: &[FileInfo]) -> Report {
    rules::check(skill_md, files)
}

/// The starter SKILL.md (scores 100).
pub fn template() -> &'static str {
    include_str!("templates/SKILL.md")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_template_parses_and_scores_100() {
        let parsed = parse(template()).expect("the template parses");
        assert_eq!(parsed.frontmatter.name, "new-skill");
        let report = check(template(), &[]);
        assert_eq!(report.score, 100, "{:?}", report.findings);
    }
}
