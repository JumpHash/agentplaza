//! The skill tools agents are offered (docs/design/skill-library.md, "What a skill is" and "Agents and the hub MCP
//! server"): the skills an agent may use (the plaza's and its role's) and the text of one of them. They are host tools
//! granted to every agent: they read the library and run nothing. A skill's scripts run only in the agent's own VM.

use std::collections::BTreeMap;

use base64::Engine as _;
use serde_json::{Value, json};

use super::{Skill, Skills, VersionMeta};
use crate::agents::Agent;
use crate::state::App;

/// The tools this module serves.
pub const TOOLS: &[&str] = &["plaza_skills", "plaza_skill_read"];

/// The most skills the system prompt lists.
pub const MAX_LISTED: usize = 50;

/// The most text a skill tool returns, in bytes.
pub const MAX_RESPONSE: usize = 256 * 1024;

/// The live skills `role` may use: the plaza's and the role's, one per name, sorted by name. Each carries the name
/// and description of its PUBLISHED version (a newer draft may rename or redescribe the skill; agents only ever see
/// what is live), read from that version's SKILL.md.
fn available(
    skills: &Skills,
    role: &str,
    project: Option<&str>,
) -> Result<Vec<(Skill, VersionMeta)>, String> {
    let mut by_name: BTreeMap<String, (Skill, VersionMeta)> = BTreeMap::new();
    // The plaza's, the role's, and the project's when the agent works on one.
    let mut bindings = vec!["plaza".to_string(), format!("role:{role}")];
    if let Some(project) = project {
        bindings.push(format!("project:{project}"));
    }
    for binding in bindings {
        for (mut skill, meta) in skills.store().bound(&binding)? {
            if meta.version != skill.latest_version
                && let Some(live) = skills.store().version(&skill.id, meta.version)?
                && let Ok(parsed) = apz_skill::parse(&live.skill_md)
            {
                skill.name = parsed.frontmatter.name;
                skill.description = parsed.frontmatter.description;
            }
            by_name.entry(skill.name.clone()).or_insert((skill, meta));
        }
    }
    Ok(by_name.into_values().collect())
}

/// The `## Skills` section of an agent's system prompt, or `None` when it has no skills. A library that cannot be
/// read leaves the section out; the skill tools report the error when asked.
pub fn prompt_section(app: &App, role: &str, project: Option<&str>) -> Option<String> {
    section(&app.skills, role, project)
}

fn section(skills: &Skills, role: &str, project: Option<&str>) -> Option<String> {
    let found = available(skills, role, project).ok()?;
    if found.is_empty() {
        return None;
    }
    let lines: Vec<String> = found
        .iter()
        .take(MAX_LISTED)
        .map(|(skill, _)| format!("{}: {}", skill.name, skill.description))
        .collect();
    Some(format!(
        "## Skills\n\nWhen a task matches a skill, call plaza_skill_read with its name before you start, and follow it. \
         Run a skill's scripts only in your own VM.\n\n{}",
        lines.join("\n")
    ))
}

/// The description and input schema of skill tool `name`.
pub fn spec(name: &str) -> Option<(&'static str, Value)> {
    Some(match name {
        "plaza_skills" => (
            "List the skills you may use: the plaza's and your role's, one per line as `name — description`. Call it when a task may match a skill, before you start the work.",
            closed(json!({}), &[]),
        ),
        "plaza_skill_read" => (
            "Read a skill you may use. With no path, returns its SKILL.md; with a path, that file of the skill (text whole, binary as base64). Call it with the skill's name before you follow it. A skill's scripts run only in your own VM.",
            closed(
                json!({"name": {"type": "string", "minLength": 1}, "path": {"type": "string", "minLength": 1}}),
                &["name"],
            ),
        ),
        _ => return None,
    })
}

/// Runs skill tool `name` for `agent`; the text the model reads.
pub fn run(app: &App, agent: &Agent, name: &str, input: &Value) -> Result<String, String> {
    match name {
        "plaza_skills" => list(
            &app.skills,
            &agent.role,
            crate::projects::project_of(app, agent.cid).as_deref(),
        ),
        "plaza_skill_read" => read(
            &app.skills,
            &agent.role,
            crate::projects::project_of(app, agent.cid).as_deref(),
            input,
        ),
        other => Err(format!("{other} is not a skill tool")),
    }
}

fn list(skills: &Skills, role: &str, project: Option<&str>) -> Result<String, String> {
    let found = available(skills, role, project)?;
    if found.is_empty() {
        return Ok("No skills are available to you.".to_string());
    }
    let lines: Vec<String> = found
        .iter()
        .map(|(skill, _)| format!("{} — {}", skill.name, skill.description))
        .collect();
    Ok(cap(lines.join("\n")))
}

fn read(
    skills: &Skills,
    role: &str,
    project: Option<&str>,
    input: &Value,
) -> Result<String, String> {
    let name = input
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .ok_or("missing or empty string `name`")?;
    let (skill, meta) = available(skills, role, project)?
        .into_iter()
        .find(|(skill, _)| skill.name == name)
        .ok_or_else(|| format!("no skill named {name} is available to you"))?;
    let version = skills
        .store()
        .version(&skill.id, meta.version)?
        .ok_or_else(|| format!("{name} has no version {}", meta.version))?;
    let path = match input.get("path") {
        None | Some(Value::Null) => return Ok(cap(version.skill_md.clone())),
        Some(Value::String(path)) => path,
        Some(_) => return Err("path must be a string".to_string()),
    };
    if path == "SKILL.md" {
        return Ok(cap(version.skill_md.clone()));
    }
    let file = version
        .files
        .iter()
        .find(|file| &file.path == path)
        .ok_or_else(|| format!("{name} has no file {path}"))?;
    let bytes = skills.blobs().get(&file.hash)?;
    Ok(match String::from_utf8(bytes) {
        Ok(text) => cap(text),
        Err(err) => {
            let bytes = err.into_bytes();
            let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
            cap(format!(
                "{path}: binary, {} bytes, as base64:\n{encoded}",
                bytes.len()
            ))
        }
    })
}

/// `text`, cut to the response cap on a character boundary, with a note when it was cut.
fn cap(mut text: String) -> String {
    if text.len() <= MAX_RESPONSE {
        return text;
    }
    let mut end = MAX_RESPONSE;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push_str("\n[truncated: the response is capped at 256 KB]");
    text
}

/// A closed JSON Schema object with the given properties and required keys.
fn closed(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::skills::store_mem::MemorySkills;
    use crate::skills::tests::MemBlobs;
    use crate::skills::{FileBody, FileIn, NewSkill, Source};

    const T0: u64 = 1_800_000_000_000;

    fn library() -> Skills {
        Skills::new(
            Arc::new(MemorySkills::in_memory()),
            Arc::new(MemBlobs::default()),
        )
    }

    /// Creates skill `name` (id = name) bound to `bindings`, with one file, and publishes it when `live`.
    fn add(skills: &Skills, name: &str, bindings: &[&str], live: bool) {
        let skill_md = format!(
            "---\nname: {name}\ndescription: Does {name} work. Use when the task involves {name}.\n---\n# {name}\n\nRead [the reference](reference.md) first.\n"
        );
        let files = vec![FileIn {
            path: "reference.md".into(),
            body: FileBody::Bytes(b"# Reference\n".to_vec()),
            executable: false,
        }];
        let first = skills
            .prepare(skill_md, files, String::new(), None)
            .expect("prepares");
        let new = NewSkill {
            id: name.into(),
            title: name.into(),
            tags: vec![],
            bindings: bindings.iter().map(|b| b.to_string()).collect(),
            source: Source::Built,
        };
        skills
            .store()
            .create(&new, &first, "owner", T0)
            .expect("created");
        if live {
            skills.publish(name, 1, "owner", T0).expect("published");
        }
    }

    #[test]
    fn an_agent_sees_the_plaza_skills_and_its_roles_only() {
        let skills = library();
        add(&skills, "pdf-forms", &["role:builder"], true);
        add(&skills, "house-style", &["plaza"], true);
        add(&skills, "review-notes", &["role:reviewer"], true);
        add(&skills, "draft-only", &["plaza"], false);
        let names = |role: &str| -> Vec<String> {
            available(&skills, role, None)
                .expect("lists")
                .into_iter()
                .map(|(skill, _)| skill.name)
                .collect()
        };
        assert_eq!(names("builder"), ["house-style", "pdf-forms"]);
        assert_eq!(names("reviewer"), ["house-style", "review-notes"]);
        // A project's skills are its agents' only.
        add(&skills, "release-checklist", &["project:pr4"], true);
        let on = |project: Option<&str>| -> Vec<String> {
            available(&skills, "builder", project)
                .expect("lists")
                .into_iter()
                .map(|(skill, _)| skill.name)
                .collect()
        };
        assert_eq!(
            on(Some("pr4")),
            ["house-style", "pdf-forms", "release-checklist"]
        );
        assert_eq!(on(Some("pr5")), ["house-style", "pdf-forms"]);
        assert_eq!(on(None), ["house-style", "pdf-forms"]);
        assert!(
            list(&skills, "builder", None)
                .expect("lists")
                .contains("pdf-forms — ")
        );
        let fetch = |role: &str, input: Value| read(&skills, role, None, &input);
        assert!(
            fetch("builder", json!({"name": "pdf-forms"}))
                .expect("reads")
                .contains("# pdf-forms")
        );
        assert!(
            fetch(
                "builder",
                json!({"name": "pdf-forms", "path": "reference.md"})
            )
            .expect("reads")
            .contains("# Reference")
        );
        assert!(
            fetch("reviewer", json!({"name": "pdf-forms"})).is_err(),
            "another role's skill is not offered"
        );
        assert!(
            fetch("builder", json!({"name": "draft-only"})).is_err(),
            "an unpublished skill is not offered"
        );
        assert!(
            fetch(
                "builder",
                json!({"name": "pdf-forms", "path": "missing.md"})
            )
            .is_err()
        );
        let text = section(&skills, "reviewer", None).expect("a section");
        assert!(
            text.starts_with("## Skills") && text.contains("review-notes: Does review-notes work.")
        );
        assert!(!text.contains("pdf-forms"));
    }

    #[test]
    fn a_response_is_capped_on_a_character_boundary() {
        let out = cap("é".repeat(MAX_RESPONSE));
        assert!(out.len() <= MAX_RESPONSE + 60, "{}", out.len());
        assert!(out.contains("truncated"));
    }
}
