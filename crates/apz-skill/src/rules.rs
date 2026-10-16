//! The skill rules (docs/design/skill-library.md, "Checks").

use std::collections::BTreeSet;

use apz_prompt::Kind;

use crate::{FileInfo, Finding, Frontmatter, Report, Rule, Severity, Stats};

pub const RULES: &[Rule] = &[
    Rule {
        id: "frontmatter",
        severity: Severity::Error,
        title: "Frontmatter missing or broken",
        why: "SKILL.md must open with a --- block holding the name and the description, or no agent can find the skill.",
    },
    Rule {
        id: "name-format",
        severity: Severity::Error,
        title: "Name not usable",
        why: "The name is how agents call the skill: at most 64 lower-case letters, digits and hyphens, without the reserved words \"anthropic\" and \"claude\".",
    },
    Rule {
        id: "description-missing",
        severity: Severity::Error,
        title: "No description",
        why: "Agents choose a skill from its description alone; without one it is never used.",
    },
    Rule {
        id: "description-length",
        severity: Severity::Error,
        title: "Description too long",
        why: "The description is read for every skill on every task; keep it under 1024 characters.",
    },
    Rule {
        id: "description-when",
        severity: Severity::Warning,
        title: "Description does not say when to use it",
        why: "A description that names the situation (\"Use when …\") is what makes an agent pick the skill at the right time.",
    },
    Rule {
        id: "body-long",
        severity: Severity::Warning,
        title: "Long SKILL.md",
        why: "Over 500 lines, move detail into referenced files the agent opens only when it needs them.",
    },
    Rule {
        id: "missing-file",
        severity: Severity::Error,
        title: "Linked file missing",
        why: "The body points at a file the skill does not contain; the agent would follow a dead link.",
    },
    Rule {
        id: "unused-file",
        severity: Severity::Info,
        title: "File nothing mentions",
        why: "A file no instruction mentions is never opened; mention it where it helps, or remove it.",
    },
    Rule {
        id: "script",
        severity: Severity::Warning,
        title: "Contains code",
        why: "Agents may run the skill's scripts in their own VMs. Read them before publishing, as you would any code you run.",
    },
    Rule {
        id: "large-file",
        severity: Severity::Warning,
        title: "Large file",
        why: "Files over 1 MB cost time and tokens every time an agent reads them.",
    },
    Rule {
        id: "binary-file",
        severity: Severity::Info,
        title: "Binary file",
        why: "Not text: shown in the file list, not previewed or checked.",
    },
    Rule {
        id: "size-limit",
        severity: Severity::Error,
        title: "Too big",
        why: "At most 200 files, 5 MB per file and 20 MB in all.",
    },
];

const SKILL: &str = "SKILL.md";
const MAX_NAME: usize = 64;
const MAX_DESCRIPTION: usize = 1024;
const MAX_BODY_LINES: usize = 500;
/// Sizes are decimal: 1 MB is 1,000,000 bytes.
// Binary megabytes, the units apid enforces and migration 0007 checks (5242880 and 20971520 bytes).
const LARGE_FILE: u64 = 1024 * 1024;
const HUGE_FILE: u64 = 5 * 1024 * 1024;
const MAX_TOTAL: u64 = 20 * 1024 * 1024;
/// Counted with SKILL.md.
const MAX_FILES: usize = 200;
const SCRIPT_EXTENSIONS: &[&str] = &[
    "py", "sh", "bash", "js", "mjs", "ts", "rb", "pl", "php", "ps1", "bat", "go", "rs",
];

pub fn check(skill_md: &str, files: &[FileInfo]) -> Report {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let parsed = crate::parse(skill_md);
    let (mut name, mut description, mut body_lines): (Option<String>, Option<String>, u32) =
        (None, None, 0);
    match &parsed {
        Err((line, reason)) => out.push(finding("frontmatter", SKILL, *line, reason.clone())),
        Ok(p) => {
            let fm = &p.frontmatter;
            name = Some(fm.name.clone());
            description = Some(fm.description.clone());
            body_lines = p.body.lines().count() as u32;
            frontmatter_rules(fm, &mut out);
            if body_lines as usize > MAX_BODY_LINES {
                out.push(finding(
                    "body-long",
                    SKILL,
                    0,
                    format!("{body_lines} lines; over 500, move detail into referenced files"),
                ));
            }
            let offset = p.body_line - 1;
            check_refs(&p.body, "", SKILL, offset, files, &mut seen, &mut out);
            for f in apz_prompt::check(Kind::Fragment, &p.body).findings {
                let line = if f.line == 0 { 0 } else { f.line + offset };
                out.push(Finding {
                    rule: f.rule,
                    severity: f.severity,
                    path: SKILL.to_string(),
                    line,
                    message: f.message,
                });
            }
        }
    }

    for f in files {
        if let Some(text) = &f.text {
            check_refs(
                text,
                dir_of(&f.path),
                &f.path,
                0,
                files,
                &mut seen,
                &mut out,
            );
        }
    }
    unused_files(files, skill_md, &mut out);

    let mut scripts = 0u32;
    for f in files {
        let reasons = script_reasons(f);
        if !reasons.is_empty() {
            scripts += 1;
            out.push(finding(
                "script",
                &f.path,
                0,
                format!("code the agent may run: {}", reasons.join(", ")),
            ));
        }
        if f.size > LARGE_FILE {
            out.push(finding(
                "large-file",
                &f.path,
                0,
                format!("{} bytes; over 1 MB", f.size),
            ));
        } else if f.text.is_none() && f.size > 0 {
            out.push(finding(
                "binary-file",
                &f.path,
                0,
                "not UTF-8 text, so it is listed but not checked",
            ));
        }
        if f.size > HUGE_FILE {
            out.push(finding(
                "size-limit",
                &f.path,
                0,
                format!("{} bytes; at most 5 MB a file", f.size),
            ));
        }
    }
    // SKILL.md is not counted: the limit is on the skill's other files, as apid and skill_versions.file_count count them.
    let total_files = files.len();
    let total_bytes = skill_md.len() as u64 + files.iter().map(|f| f.size).sum::<u64>();
    if total_files > MAX_FILES {
        out.push(finding(
            "size-limit",
            SKILL,
            0,
            format!("{total_files} files; at most 200"),
        ));
    }
    if total_bytes > MAX_TOTAL {
        out.push(finding(
            "size-limit",
            SKILL,
            0,
            format!("{total_bytes} bytes in all; at most 20 MB"),
        ));
    }

    out.sort_by(|a, b| {
        (&a.path, a.line, a.severity, &a.rule).cmp(&(&b.path, b.line, b.severity, &b.rule))
    });
    let count = |s: Severity| out.iter().filter(|f| f.severity == s).count() as u32;
    let (errors, warnings, infos) = (
        count(Severity::Error),
        count(Severity::Warning),
        count(Severity::Info),
    );
    Report {
        rules_version: crate::RULES_VERSION,
        score: apz_prompt::score(errors, warnings, infos),
        findings: out,
        errors,
        warnings,
        infos,
        stats: Stats {
            files: total_files as u32,
            total_bytes,
            body_lines,
            est_tokens: (skill_md.chars().count() as u32).div_ceil(4),
            scripts,
        },
        name,
        description,
    }
}

/// A finding for rule `id`, at that rule's catalogue severity.
fn finding(id: &str, path: &str, line: u32, message: impl Into<String>) -> Finding {
    let severity = RULES
        .iter()
        .find(|r| r.id == id)
        .map_or(Severity::Error, |r| r.severity);
    Finding {
        rule: id.to_string(),
        severity,
        path: path.to_string(),
        line,
        message: message.into(),
    }
}

fn frontmatter_rules(fm: &Frontmatter, out: &mut Vec<Finding>) {
    if let Some(problem) = name_problem(&fm.name) {
        out.push(finding("name-format", SKILL, 0, problem));
    }
    let desc = fm.description.trim();
    if desc.is_empty() {
        out.push(finding(
            "description-missing",
            SKILL,
            0,
            "the description is missing or empty",
        ));
    } else {
        let len = desc.chars().count();
        if len > MAX_DESCRIPTION {
            out.push(finding(
                "description-length",
                SKILL,
                0,
                format!("{len} characters; at most 1024"),
            ));
        }
        let lower = desc.to_lowercase();
        if !(lower.contains("use when") || lower.contains("when ") || lower.contains("use this")) {
            out.push(finding(
                "description-when",
                SKILL,
                0,
                "the description does not say when to use the skill (\"Use when …\")",
            ));
        }
    }
}

fn name_problem(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("the name is missing".into());
    }
    let problem = if name.chars().count() > MAX_NAME {
        "it is longer than 64 characters"
    } else if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        "use lower-case letters, digits and hyphens only"
    } else if name.starts_with('-') || name.ends_with('-') {
        "it must not start or end with a hyphen"
    } else if name.contains("--") {
        "it must not contain \"--\""
    } else if name.contains("anthropic") || name.contains("claude") {
        "it must not contain \"anthropic\" or \"claude\""
    } else {
        return None;
    };
    Some(format!("name `{name}`: {problem}"))
}

/// The lower-case extension of a file name, when the name is not only an extension (`.bashrc` has none).
fn extension(path: &str) -> Option<String> {
    let name = path.rsplit('/').next()?;
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty()).then(|| ext.to_ascii_lowercase())
}

/// Why a file counts as code: an executable bit, a shebang, or a script extension.
fn script_reasons(f: &FileInfo) -> Vec<String> {
    let mut why = Vec::new();
    if f.executable {
        why.push("an executable bit".to_string());
    }
    if matches!(&f.text, Some(t) if t.starts_with("#!")) {
        why.push("a shebang line".to_string());
    }
    if let Some(ext) = extension(&f.path)
        && SCRIPT_EXTENSIONS.contains(&ext.as_str())
    {
        why.push(format!("a .{ext} file"));
    }
    why
}

/// The folder of a path, `""` at the root.
fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// A backticked token that looks like a file path inside the skill: `scripts/fill.py`, no spaces, a short extension.
fn is_file_path(code: &str) -> bool {
    !code.contains(char::is_whitespace)
        && code.contains('/')
        && !code.starts_with('/')
        && !code.ends_with('/')
        && !code.contains("://")
        && matches!(extension(code), Some(e) if (1..=5).contains(&e.len()) && e.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// The references in a text, as (1-based line, target, markdown). Markdown targets are relative to the file's
/// folder; backticked paths are relative to the skill root.
fn references(text: &str) -> Vec<(u32, String, bool)> {
    let mut out = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let no = idx as u32 + 1;
        let mut rest = line;
        while let Some(at) = rest.find("](") {
            let after = &rest[at + 2..];
            let Some(end) = after.find(')') else { break };
            let target = after[..end].split_whitespace().next().unwrap_or("");
            let target = target.split(['#', '?']).next().unwrap_or("");
            let relative = !target.is_empty()
                && !target.starts_with('/')
                && !target.contains("://")
                && !target.starts_with("mailto:")
                && !target.starts_with("data:")
                && !target.ends_with('/');
            if relative {
                out.push((no, target.to_string(), true));
            }
            rest = &after[end + 1..];
        }
        for (i, code) in line.split('`').enumerate() {
            if i % 2 == 1 && is_file_path(code) {
                out.push((no, code.to_string(), false));
            }
        }
    }
    out
}

/// `base/target` with `.` and `..` resolved; None when it climbs out of the skill.
fn resolve(base: &str, target: &str) -> Option<String> {
    let joined = if base.is_empty() {
        target.to_string()
    } else {
        format!("{base}/{target}")
    };
    let mut parts: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// missing-file for the references in one text. `offset` turns the text's line into a SKILL.md line for the body.
fn check_refs(
    text: &str,
    base: &str,
    path: &str,
    offset: u32,
    files: &[FileInfo],
    seen: &mut BTreeSet<(String, String)>,
    out: &mut Vec<Finding>,
) {
    for (line, target, markdown) in references(text) {
        let from = if markdown { base } else { "" };
        let Some(resolved) = resolve(from, &target) else {
            continue;
        };
        let folder = format!("{resolved}/");
        if resolved == SKILL
            || files
                .iter()
                .any(|f| f.path == resolved || f.path.starts_with(&folder))
        {
            continue;
        }
        if seen.insert((path.to_string(), target.clone())) {
            out.push(finding(
                "missing-file",
                path,
                offset + line,
                format!("`{target}` is linked but the skill has no such file"),
            ));
        }
    }
}

/// unused-file: a file whose path or name nothing in SKILL.md or another text file mentions.
fn unused_files(files: &[FileInfo], skill_md: &str, out: &mut Vec<Finding>) {
    for f in files {
        let name = f.path.rsplit('/').next().unwrap_or("");
        let mentioned = std::iter::once((SKILL, skill_md))
            .chain(
                files
                    .iter()
                    .filter_map(|g| g.text.as_deref().map(|t| (g.path.as_str(), t))),
            )
            .any(|(p, t)| {
                p != f.path.as_str() && (t.contains(f.path.as_str()) || t.contains(name))
            });
        if !mentioned {
            out.push(finding(
                "unused-file",
                &f.path,
                0,
                format!(
                    "nothing in SKILL.md or the other text files mentions `{}`",
                    f.path
                ),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template;

    const BODY: &str = "# Title\n\nDo the task.\n";

    fn file(path: &str, size: u64, text: Option<&str>) -> FileInfo {
        FileInfo {
            path: path.to_string(),
            size,
            executable: false,
            text: text.map(str::to_string),
        }
    }

    fn skill(name: &str, description: &str, body: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\n{body}")
    }

    fn fired(report: &Report, id: &str) -> Vec<Finding> {
        report
            .findings
            .iter()
            .filter(|f| f.rule == id)
            .cloned()
            .collect()
    }

    #[test]
    fn frontmatter_fires_on_broken_yaml_only() {
        let f = fired(&check("name: x\n", &[]), "frontmatter");
        assert_eq!((f.len(), f[0].line, f[0].severity), (1, 1, Severity::Error));
        assert_eq!(fired(&check("---\nname: x\n", &[]), "frontmatter").len(), 1);
        assert!(fired(&check(template(), &[]), "frontmatter").is_empty());
    }

    #[test]
    fn frontmatter_rules_check_names_and_descriptions() {
        // name-format fires once per unusable name; a valid name does not fire.
        {
            let long = "a".repeat(65);
            for name in [
                "PDF_forms",
                "-pdf",
                "pdf-",
                "pdf--forms",
                "my-claude-tool",
                "anthropic-x",
                "",
                long.as_str(),
            ] {
                assert_eq!(
                    fired(
                        &check(&skill(name, "Use when x.", BODY), &[]),
                        "name-format"
                    )
                    .len(),
                    1,
                    "{name}"
                );
            }
            assert!(
                fired(
                    &check(&skill("pdf-forms-2", "Use when x.", BODY), &[]),
                    "name-format"
                )
                .is_empty()
            );
        }
        // description-missing fires only when the description is empty.
        {
            assert_eq!(
                fired(&check(&skill("pdf", "", BODY), &[]), "description-missing").len(),
                1
            );
            assert!(
                fired(
                    &check(&skill("pdf", "Use when x.", BODY), &[]),
                    "description-missing"
                )
                .is_empty()
            );
        }
        // description-length fires only above 1024 characters.
        {
            let at_limit = format!("Use when {}", "a".repeat(1015));
            let over = format!("Use when {}", "a".repeat(1016));
            assert_eq!(at_limit.len(), 1024);
            assert!(
                fired(
                    &check(&skill("pdf", &at_limit, BODY), &[]),
                    "description-length"
                )
                .is_empty()
            );
            assert_eq!(
                fired(
                    &check(&skill("pdf", &over, BODY), &[]),
                    "description-length"
                )
                .len(),
                1
            );
        }
        // description-when warns without a trigger; any trigger phrase satisfies it.
        {
            let f = fired(
                &check(&skill("pdf", "Fills PDF forms.", BODY), &[]),
                "description-when",
            );
            assert_eq!((f.len(), f[0].severity), (1, Severity::Warning));
            for ok in [
                "Fills PDF forms. Use when a PDF has fields.",
                "Fills forms. Use this for PDFs.",
                "Fills PDF forms, when a PDF has fields.",
            ] {
                assert!(
                    fired(&check(&skill("pdf", ok, BODY), &[]), "description-when").is_empty(),
                    "{ok}"
                );
            }
        }
    }

    #[test]
    fn size_rules_warn_on_long_bodies_big_files_and_big_skills() {
        // body-long warns over 500 lines; exactly 500 lines do not warn.
        {
            let long = "Step.\n".repeat(501);
            let f = fired(
                &check(&skill("pdf", "Use when x.", &long), &[]),
                "body-long",
            );
            assert_eq!((f.len(), f[0].severity), (1, Severity::Warning));
            let ok = "Step.\n".repeat(500);
            assert!(fired(&check(&skill("pdf", "Use when x.", &ok), &[]), "body-long").is_empty());
        }
        // large-file warns on a big text file; binary-file is info for a binary file.
        {
            let big = [file("data.csv", 1_500_000, None)];
            let r = check(&skill("pdf", "Use when x.", BODY), &big);
            assert_eq!(fired(&r, "large-file").len(), 1);
            assert!(fired(&r, "binary-file").is_empty());
            let bin = [file("logo.png", 10, None), file("a.txt", 10, Some("x"))];
            let r = check(&skill("pdf", "Use when x.", BODY), &bin);
            assert_eq!(fired(&r, "binary-file").len(), 1);
            assert!(fired(&r, "large-file").is_empty());
        }
        // size-limit fires on one file over 5 MB, on more than 200 files, and on 25 MB in all.
        {
            let huge = [file("model.bin", 5 * 1024 * 1024 + 1, None)];
            let f = fired(
                &check(&skill("pdf", "Use when x.", BODY), &huge),
                "size-limit",
            );
            assert_eq!((f.len(), f[0].path.as_str()), (1, "model.bin"));
            let many: Vec<FileInfo> = (0..201)
                .map(|i| file(&format!("f{i}.txt"), 1, Some("x")))
                .collect();
            let f = fired(
                &check(&skill("pdf", "Use when x.", BODY), &many),
                "size-limit",
            );
            assert_eq!((f.len(), f[0].path.as_str()), (1, "SKILL.md"));
            let chunky: Vec<FileInfo> = (0..5)
                .map(|i| file(&format!("c{i}.bin"), 5 * 1024 * 1024, None))
                .collect();
            let f = fired(
                &check(&skill("pdf", "Use when x.", BODY), &chunky),
                "size-limit",
            );
            assert_eq!(
                (f.len(), f[0].path.as_str()),
                (1, "SKILL.md"),
                "25 MB in all"
            );
            let small = [file("a.txt", 1, Some("x"))];
            assert!(
                fired(
                    &check(&skill("pdf", "Use when x.", BODY), &small),
                    "size-limit"
                )
                .is_empty()
            );
        }
    }

    #[test]
    fn missing_file_flags_dead_links_and_unused_files() {
        // A dead link in the body is an error on SKILL.md, counted from the body's first line.
        {
            let body = "See [the form](forms/fill.md) and `scripts/run.py`.\n";
            let f = fired(
                &check(&skill("pdf", "Use when x.", body), &[]),
                "missing-file",
            );
            assert_eq!(f.len(), 2);
            assert!(
                f.iter()
                    .all(|x| x.path == "SKILL.md" && x.severity == Severity::Error)
            );
            assert_eq!(
                f[0].line, 5,
                "the body starts after the four frontmatter lines"
            );
            let files = [
                file("forms/fill.md", 3, Some("x")),
                file("scripts/run.py", 3, Some("x")),
            ];
            assert!(
                fired(
                    &check(&skill("pdf", "Use when x.", body), &files),
                    "missing-file"
                )
                .is_empty()
            );
        }
        // URLs, in-page anchors and mail links are never checked as files.
        {
            let body = "[a](https://example.com) [b](#top) [c](mailto:me@example.com)\n";
            assert!(
                fired(
                    &check(&skill("pdf", "Use when x.", body), &[]),
                    "missing-file"
                )
                .is_empty()
            );
        }
        // Text files are checked relative to their own folder.
        {
            let text = [file("reference.md", 20, Some("see [x](gone.md)\n"))];
            let f = fired(
                &check(&skill("pdf", "Use when x.", BODY), &text),
                "missing-file",
            );
            assert_eq!(
                (f.len(), f[0].path.as_str(), f[0].line),
                (1, "reference.md", 1)
            );
            let nested = [
                file("docs/a.md", 9, Some("[b](b.md)")),
                file("docs/b.md", 1, Some("")),
            ];
            assert!(
                fired(
                    &check(&skill("pdf", "Use when x.", BODY), &nested),
                    "missing-file"
                )
                .is_empty()
            );
        }
        // unused-file is info when nothing mentions the file.
        {
            let files = [file("assets/logo.png", 10, None)];
            let f = fired(
                &check(&skill("pdf", "Use when x.", BODY), &files),
                "unused-file",
            );
            assert_eq!((f.len(), f[0].severity), (1, Severity::Info));
            let body = "Use `assets/logo.png` for the header.\n";
            assert!(
                fired(
                    &check(&skill("pdf", "Use when x.", body), &files),
                    "unused-file"
                )
                .is_empty()
            );
        }
    }

    #[test]
    fn script_flags_shebangs_extensions_and_executables() {
        let mut exec = file("tool", 4, Some("hi"));
        exec.executable = true;
        let files = [
            file("scripts/fill", 10, Some("#!/bin/sh\necho\n")),
            file("tool.py", 5, Some("print(1)")),
            exec,
            file("notes.txt", 2, Some("hi")),
        ];
        let r = check(&skill("pdf", "Use when x.", BODY), &files);
        let paths: Vec<String> = fired(&r, "script").into_iter().map(|f| f.path).collect();
        assert_eq!(paths, ["scripts/fill", "tool", "tool.py"]);
        assert_eq!(r.stats.scripts, 3);
    }

    #[test]
    fn findings_are_sorted_and_stats_count_the_skill() {
        let files = [file("z.bin", 1, None), file("a.bin", 1, None)];
        let r = check(template(), &files);
        assert!(r.findings.windows(2).all(|w| {
            (&w[0].path, w[0].line, w[0].severity, &w[0].rule)
                <= (&w[1].path, w[1].line, w[1].severity, &w[1].rule)
        }));
        assert_eq!(r.stats.files, 2, "the files besides SKILL.md");
        assert_eq!(r.stats.total_bytes, template().len() as u64 + 2);
        assert_eq!(r.name.as_deref(), Some("new-skill"));
        assert_eq!(r.score, apz_prompt::score(r.errors, r.warnings, r.infos));
    }
}
