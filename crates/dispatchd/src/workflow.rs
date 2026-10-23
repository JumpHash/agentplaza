//! The scripted workflows: what a role family *does* with a task, and how the
//! dispatcher drives it.
//!
//! A workflow is a function from a task and a [`Driver`] to what the run left
//! behind ([`Report`]). The driver is the only thing a workflow talks to, and
//! it has exactly one method: run one tool call on the agent and answer with
//! the tool's own output. So a workflow is a *script* — a sequence of tool
//! calls whose arguments may depend on earlier answers — and the transport (the
//! hub's `POST /tell`, an agent's session, a tool implementation inside a
//! guest) is entirely behind the driver. That is what makes the scripts
//! unit-testable: a fake driver stands in for a whole agent and records the
//! sequence.
//!
//! # What a role grants is what a workflow may drive
//!
//! The codex grants each role its tools (the manifest `--roles` names), and the
//! hub enforces those grants: a call a role does not grant answers `403` and
//! never reaches the agent. The dispatcher therefore does not *knowingly*
//! attempt one — [`plan_for`] is that decision. It takes the role, finds the
//! family's script, and checks every tool that script drives
//! ([`Family::tools`]) against the manifest:
//!
//! * the family's own script, when every tool it drives is granted;
//! * otherwise [`read_only_workflow`], when the role may `search` — a role whose
//!   grants exclude the write a family's script would make still has something
//!   real to do;
//! * otherwise nothing at all: the task is left alone
//!   ([`Plan::Skipped`]), which is what a role whose declared tools the harness
//!   does not implement looks like from here.
//!
//! With no manifest there is no grant to check, so the family's script is
//! driven exactly as it was before the policy existed — and the hub has no
//! manifest either, so nothing can be refused.
//!
//! The **librarian** is why this is not hypothetical: the codex grants it
//! `read_memory`, `search`, `transcript_search`, `read_artifact` and
//! `submit_memory_change`, of which the harness implements one — `search` — so
//! its script is read-only and the note it finishes with *is* the artifact it
//! leaves.
//!
//! # The role → workflow table
//!
//! Keyed like the harness's own `TOOLS` table: one entry per role family, and
//! adding one is one entry plus its function and the tools it drives. A role
//! with no entry gets [`default_workflow`], so a fleet never has a role the
//! dispatcher cannot work with — only one it works with generically.
//!
//! Every write workflow *proves* what it did rather than asserting it: it
//! writes and then reads its artifact back, and a read that does not match what
//! was written is a failure, not a success with a hopeful note. A read-only
//! workflow has nothing to read back and no read grant to read it with, so its
//! proof is the same shape of thing: the answer must be the tool's own — a
//! rendering of an answer that is not that shape is a failure, and the note
//! quotes the tool's own counts and first match rather than claiming anything
//! on its own. Either way the finish note names what the run left and how many
//! tool calls it took.

use serde_json::json;

use crate::roles::Manifest;
use crate::views::Task;

/// The most of a tool's output a note or a log line carries. A tool that
/// refuses something quotes what it was given; that quote is bounded here so
/// nothing a guest says can grow a task's note past the ledger's own bound.
pub const MAX_NOTE_DETAIL: usize = 512;

/// The most a read-only run's search token may occupy, in characters.
///
/// The token comes from the task's own goal, which is free text — so it is cut
/// here, well under the harness's own `search` needle cap, rather than asking
/// the agent for a pattern its tool would refuse. A refused call is not a
/// read-only outcome.
pub const MAX_PATTERN_CHARS: usize = 128;

/// What a workflow produced, or the reason it could not finish.
pub type Run = Result<Report, DriveError>;

/// What a workflow left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    /// Artifact paths the agent's workspace now holds.
    Artifacts(Vec<String>),
    /// The workflow wrote nothing — its role's grants exclude every write — so
    /// this one line, the script's own summary of what it read, *is* the task's
    /// note: the note is the artifact.
    Summary(String),
}

impl Report {
    /// The artifacts the workspace holds; empty for a read-only run.
    pub fn artifacts(&self) -> &[String] {
        match self {
            Self::Artifacts(paths) => paths,
            Self::Summary(_) => &[],
        }
    }

    /// The one line a read-only run carries, when it carries one.
    pub fn summary(&self) -> Option<&str> {
        match self {
            Self::Artifacts(_) => None,
            Self::Summary(line) => Some(line),
        }
    }
}

/// A workflow: the task to do and the driver that talks to the agent.
pub type Workflow = fn(&Task, &mut dyn Driver) -> Run;

/// One agent, as far as a workflow is concerned: it runs tool calls.
///
/// This is the seam the whole dispatcher is built around. Production's
/// implementation ([`crate::dispatch::AgentDriver`]) turns each call into a
/// hub `POST /tell` and waits for the agent's `tool_result`; the tests'
/// implementation is a workspace in a map. A workflow cannot tell them apart,
/// which is why the scripts are proven without a guest.
pub trait Driver {
    /// Runs one tool call and answers with the tool's output (the harness's
    /// own JSON object, as a string).
    fn call(&mut self, name: &str, args: serde_json::Value) -> Result<String, DriveError>;

    /// How many tool calls this driver has made: the count a finished task's
    /// note reports.
    fn calls(&self) -> u64;
}

/// Why a workflow could not finish.
///
/// The split that matters is [`DriveError::is_retryable`]: an agent that
/// answered and refused, or answered something the workflow cannot use, will
/// answer the same way next time — but an agent that could not be reached may
/// well be reachable on the next pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriveError {
    /// The agent ran the tool and the tool refused, or failed: the agent's own
    /// answer, and an answer the next pass would get again.
    Refused {
        /// The tool that refused.
        name: String,
        /// The tool's own output (its refusal reason).
        output: String,
    },
    /// The agent could not be reached: it is not connected, it did not answer
    /// in time, or the call could not be made at all.
    Unreachable {
        /// The tool that could not be run.
        name: String,
        /// Why it could not be run.
        reason: String,
    },
    /// The agent answered, but not with what the workflow needed: the tool
    /// succeeded and its output is not the shape this workflow requires.
    BadOutput {
        /// The tool whose answer was wrong.
        name: String,
        /// What was wrong with it.
        reason: String,
    },
}

impl DriveError {
    /// Whether another pass could succeed where this one did not.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Unreachable { .. })
    }

    /// The one line a failed task is finished with.
    pub fn note(&self) -> String {
        match self {
            Self::Refused { name, output } => {
                format!("tool {name} refused: {}", bounded(output))
            }
            Self::Unreachable { name, reason } => {
                format!("tool {name} unavailable: {}", bounded(reason))
            }
            Self::BadOutput { name, reason } => format!(
                "tool {name} answered something this workflow cannot use: {}",
                bounded(reason)
            ),
        }
    }
}

/// Bounds a piece of text a guest or a daemon supplied, so it can go into a
/// log line or a task's note whatever it contains.
pub fn bounded(text: &str) -> String {
    if text.chars().count() <= MAX_NOTE_DETAIL {
        return text.to_string();
    }
    let mut out: String = text.chars().take(MAX_NOTE_DETAIL).collect();
    out.push('…');
    out
}

/// One role family: the script its tasks run, and every harness tool that
/// script drives — the list [`plan_for`] checks against the role's grants
/// before anything is driven.
///
/// A family is a `const`, so a plan can carry `&'static Family` and be compared
/// by name: two families are the same script exactly when they are the same
/// name, which is what [`Plan`]'s equality means.
#[derive(Debug, Clone, Copy)]
pub struct Family {
    /// The role the family is filed under; the empty string is the generic
    /// default every unmatched role falls to.
    pub role: &'static str,
    /// The family's name, unique among the table's entries.
    pub name: &'static str,
    /// The script.
    pub workflow: Workflow,
    /// Every harness tool the script runs. A script that drove a tool not
    /// listed here would be a script the plan cannot vouch for, so the list is
    /// part of the entry rather than a comment beside it.
    pub tools: &'static [&'static str],
}

/// The tools a read-only run drives.
pub const READ_ONLY_TOOLS: &[&str] = &["search"];

/// The generic default: the round trip into `notes.txt`.
const DEFAULT_FAMILY: Family = Family {
    role: "",
    name: "default",
    workflow: default_workflow,
    tools: &["edit", "read"],
};

/// A builder's: the round trip into `build-notes.txt`.
const BUILDER_FAMILY: Family = Family {
    role: "builder",
    name: "builder",
    workflow: builder_workflow,
    tools: &["edit", "read"],
};

/// A read-only family's: the workspace searched, nothing written. It is the
/// librarian's own script and the fallback for any other role whose grants
/// exclude the write a family's script would make.
const READ_ONLY_FAMILY: Family = Family {
    role: "librarian",
    name: "read-only",
    workflow: read_only_workflow,
    tools: READ_ONLY_TOOLS,
};

/// The role → workflow table, keyed exactly like the harness's `TOOLS`: a role
/// family is one entry, and its function is the script it runs.
pub const WORKFLOWS: &[Family] = &[BUILDER_FAMILY, READ_ONLY_FAMILY];

/// The family a role gets: its own, or the generic default.
pub fn workflow_for(role: &str) -> &'static Family {
    WORKFLOWS
        .iter()
        .find(|family| family.role == role)
        .unwrap_or(&DEFAULT_FAMILY)
}

/// What the dispatcher may do with a task of one role, under the grants it was
/// started with.
#[derive(Clone, Copy)]
pub enum Plan {
    /// Drive this family's script: every tool it runs is granted.
    Drive(&'static Family),
    /// Do not touch the task at all — not claim it, not drive it. The reason
    /// completes the dispatcher's log line.
    Skipped(&'static str),
}

impl PartialEq for Plan {
    /// Two plans are the same when they name the same script (or carry the same
    /// reason). Function pointers are compared by what they *are*, not by the
    /// address the compiler happens to give one, which is not something a
    /// decision about a fleet should depend on.
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Drive(mine), Self::Drive(theirs)) => mine.name == theirs.name,
            (Self::Skipped(mine), Self::Skipped(theirs)) => mine == theirs,
            _ => false,
        }
    }
}

impl Eq for Plan {}

impl std::fmt::Debug for Plan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Drive(family) => write!(formatter, "Drive({})", family.name),
            Self::Skipped(reason) => write!(formatter, "Skipped({reason:?})"),
        }
    }
}

/// The plan for a task of `role`, under `roles` (the manifest `--roles` named,
/// or `None` when the dispatcher was started without one).
///
/// See the module note: the family's script is checked against the grants, the
/// read-only script is the fallback for a role that may search but not write,
/// and a role with nothing drivable at all is skipped — never claimed, never
/// driven, and left open for a dispatcher whose manifest allows it.
pub fn plan_for(role: &str, roles: Option<&Manifest>) -> Plan {
    let family = workflow_for(role);
    let Some(manifest) = roles else {
        // No manifest, no grants to check: the family's script runs, exactly as
        // it did before `--roles` existed.
        return Plan::Drive(family);
    };
    if !manifest.knows(role) {
        return Plan::Skipped("the role is not in the manifest");
    }
    if granted(manifest, role, family.tools) {
        return Plan::Drive(family);
    }
    if granted(manifest, role, READ_ONLY_TOOLS) {
        return Plan::Drive(&READ_ONLY_FAMILY);
    }
    Plan::Skipped("the role grants no tool this dispatcher may drive")
}

/// Whether the manifest grants every tool of `tools` for `role`.
fn granted(manifest: &Manifest, role: &str, tools: &[&str]) -> bool {
    tools.iter().all(|tool| manifest.grants(role, tool))
}

/// The generic default: write the task's own line into `notes.txt` and read it
/// back. Any role without a family of its own still leaves a real artifact, and
/// the round trip proves the agent's workspace really took it.
pub fn default_workflow(task: &Task, driver: &mut dyn Driver) -> Run {
    const ARTIFACT: &str = "notes.txt";
    write_then_read(driver, ARTIFACT, &task_line(task))?;
    Ok(Report::Artifacts(vec![ARTIFACT.to_string()]))
}

/// A builder's: the same round trip, into `build-notes.txt` — the notes a
/// build leaves behind, named for the thing the role builds.
pub fn builder_workflow(task: &Task, driver: &mut dyn Driver) -> Run {
    const ARTIFACT: &str = "build-notes.txt";
    write_then_read(driver, ARTIFACT, &task_line(task))?;
    Ok(Report::Artifacts(vec![ARTIFACT.to_string()]))
}

/// A read-only family's: the workspace **searched** for the token the task is
/// about, with what the search found rendered into the task's note.
///
/// This is the codex librarian's whole reach, and the fallback for any role
/// whose grants exclude the write a family's script would make. Nothing is
/// written, so there is nothing to read back and no read grant to read it with;
/// what the script proves is that the answer is the `search` tool's own — the
/// note quotes its counts and its first match, and a search that matched
/// nothing is reported as matching nothing rather than as an empty artifact.
pub fn read_only_workflow(task: &Task, driver: &mut dyn Driver) -> Run {
    let pattern = search_pattern(task);
    let output = driver.call("search", json!({ "pattern": pattern, "path": "." }))?;
    Ok(Report::Summary(render_search(&pattern, &output)?))
}

/// The token the workspace is searched for: the first word of the task's goal,
/// or the task's id when the goal has none.
///
/// A goal is free text ("summarize the handbook"), and its first word names the
/// thing the task is about — which is what a role that may only read can look
/// for. The token is cut to [`MAX_PATTERN_CHARS`], so a goal that is one
/// enormous word cannot ask the harness for a pattern past its own cap.
pub fn search_pattern(task: &Task) -> String {
    let word = task.goal.split_whitespace().next().unwrap_or("");
    let word = if word.is_empty() {
        task.id.as_str()
    } else {
        word
    };
    word.chars().take(MAX_PATTERN_CHARS).collect()
}

/// The note a task is finished with when its workflow completed: the artifact
/// paths its workspace now holds — or, for a run that wrote nothing, the line
/// the script itself carries — and how many tool calls it took.
pub fn done_note(report: &Report, calls: u64) -> String {
    match report {
        Report::Artifacts(paths) => {
            format!("artifacts: {} ({calls} tool call(s))", paths.join(", "))
        }
        Report::Summary(line) => format!("{line} ({calls} tool call(s))"),
    }
}

/// The line a write workflow writes: the task's id and goal, on one line (a
/// goal is free text and may be many lines; the artifact is a line either way).
fn task_line(task: &Task) -> String {
    format!("task {}: {}", task.id, one_line(&task.goal))
}

/// Collapses any whitespace run to a single space, so a multi-line goal
/// becomes one line.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Writes `content` to `path` on the agent, then reads it back and checks that
/// what came back is what went in — the round trip, proven rather than
/// assumed.
fn write_then_read(driver: &mut dyn Driver, path: &str, content: &str) -> Result<(), DriveError> {
    let wrote = driver.call(
        "edit",
        json!({ "create": [{ "path": path, "content": content }] }),
    )?;
    check_written(&wrote)?;
    let output = driver.call("read", json!({ "files": [{ "path": path }] }))?;
    let read = read_content(&output).ok_or_else(|| DriveError::BadOutput {
        name: "read".to_string(),
        reason: format!("the read of {path} carries no `content`"),
    })?;
    if read != content {
        return Err(DriveError::BadOutput {
            name: "read".to_string(),
            reason: format!(
                "{path} read back {} byte(s), not the {} written",
                read.len(),
                content.len()
            ),
        });
    }
    Ok(())
}

/// Whether an `edit` result says its one file was written: the harness answers
/// per file, with `ok` and, when it refused, the reason.
fn check_written(output: &str) -> Result<(), DriveError> {
    let file = serde_json::from_str::<serde_json::Value>(output)
        .ok()
        .and_then(|value| value.get("files")?.get(0).cloned());
    match file
        .as_ref()
        .and_then(|file| file.get("ok"))
        .and_then(serde_json::Value::as_bool)
    {
        Some(true) => Ok(()),
        Some(false) => Err(DriveError::Refused {
            name: "edit".to_string(),
            output: file
                .as_ref()
                .and_then(|file| file.get("error"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string(),
        }),
        None => Err(DriveError::BadOutput {
            name: "edit".to_string(),
            reason: "the edit result carries no `ok` for its file".to_string(),
        }),
    }
}

/// The `content` of a `read` result's first file, when the answer is that
/// shape. The harness numbers each line (`cat -n`: a right-aligned number, a tab,
/// the line), so the numbers are stripped and the lines joined back.
fn read_content(output: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(output).ok()?;
    let numbered = value.get("files")?.get(0)?.get("content")?.as_str()?;
    Some(
        numbered
            .lines()
            .map(|line| line.split_once('\t').map_or(line, |(_, text)| text))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// A `search` result, rendered as the one line a read-only run finishes with:
/// the token, how many lines matched over how many files, whether the tool says
/// its own answer was cut short, and the first match as the evidence.
///
/// The whole line is bounded ([`bounded`]) so nothing a guest says can grow a
/// task's note past the ledger's own bound.
fn render_search(pattern: &str, output: &str) -> Result<String, DriveError> {
    let bad = |reason: String| DriveError::BadOutput {
        name: "search".to_string(),
        reason,
    };
    let value: serde_json::Value = serde_json::from_str(output)
        .map_err(|err| bad(format!("the search result is not JSON: {err}")))?;
    let matches = value
        .get("matches")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| bad("the search result carries no `matches` array".to_string()))?;
    let mut paths: Vec<&str> = matches
        .iter()
        .filter_map(|entry| entry.get("path").and_then(serde_json::Value::as_str))
        .collect();
    paths.sort_unstable();
    paths.dedup();
    let mut line = format!(
        "read-only: search {pattern:?} matched {} line(s) in {} file(s)",
        matches.len(),
        paths.len()
    );
    if value
        .get("truncated")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        // The tool caps its own answer, so a capped answer says so rather than
        // reading as a whole workspace's worth of matches.
        line.push_str(", the tool says its answer was cut short");
    }
    if let Some(first) = matches.first() {
        let file = first
            .get("path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<unnamed>");
        let number = first
            .get("line")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let text = first
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        line.push_str(&format!("; first: {file}:{number}: {}", bounded(text)));
    }
    Ok(bounded(&line))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testing::FakeAgent;

    fn task(id: &str, goal: &str, role: &str) -> Task {
        serde_json::from_value(json!({
            "id": id,
            "goal": goal,
            "role": role,
            "state": "open",
        }))
        .expect("a task")
    }

    /// Runs a workflow against a fresh fake agent.
    fn run(role: &str, task: &Task) -> (Result<Report, DriveError>, FakeAgent) {
        let mut agent = FakeAgent::default();
        let run = (workflow_for(role).workflow)(task, &mut agent);
        (run, agent)
    }

    /// A manifest built from a role → tools table.
    fn manifest(roles: &[(&str, &[&str])]) -> Manifest {
        let text = json!({
            "roles": roles
                .iter()
                .map(|(role, tools)| (
                    (*role).to_string(),
                    json!({ "role": role, "tools": tools }),
                ))
                .collect::<serde_json::Map<String, serde_json::Value>>(),
        })
        .to_string();
        Manifest::parse(&text).expect("a well-formed test manifest")
    }

    /// The table is keyed by role, so each family runs its own script and every
    /// other role falls through to the default — and no key is repeated, since
    /// a duplicate would silently shadow the entry behind it.
    #[test]
    fn the_workflow_table_is_keyed_by_role() {
        let mut keys: Vec<&str> = WORKFLOWS.iter().map(|family| family.role).collect();
        keys.sort_unstable();
        let count = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), count, "a role family is filed once: {keys:?}");

        let (builder, _) = run("builder", &task("t1", "g", "builder"));
        assert_eq!(
            builder.expect("the builder workflow runs"),
            Report::Artifacts(vec!["build-notes.txt".to_string()])
        );
        let (librarian, _) = run("librarian", &task("t1", "g", "librarian"));
        assert_eq!(
            librarian.expect("the librarian workflow runs"),
            Report::Summary("read-only: search \"g\" matched 0 line(s) in 0 file(s)".to_string())
        );
        for generic in ["architect", "builderx", "Builder", ""] {
            let (run, _) = run(generic, &task("t1", "g", generic));
            assert_eq!(
                run.expect("the default workflow runs"),
                Report::Artifacts(vec!["notes.txt".to_string()]),
                "{generic:?} has no family of its own"
            );
        }
    }

    /// Each family declares the tools its script drives, and those lists are the
    /// scripts' own: the table is what the plan vouches for.
    #[test]
    fn every_family_declares_the_tools_it_drives() {
        for family in WORKFLOWS {
            assert!(!family.tools.is_empty(), "{} drives something", family.role);
            // What the declared list is checked against below: the calls the
            // script actually makes, recorded by the fake agent.
            let mut agent = FakeAgent::default();
            (family.workflow)(&task("t1", "g", family.role), &mut agent).expect("the script runs");
            let driven = agent.sequence();
            for tool in &driven {
                assert!(
                    family.tools.contains(tool),
                    "{} drives {tool:?}, which its entry does not declare: {driven:?}",
                    family.role
                );
            }
            for tool in family.tools {
                assert!(
                    driven.contains(tool),
                    "{} declares {tool:?} but its script never drives it: {driven:?}",
                    family.role
                );
            }
        }
    }

    /// The plan with no manifest: the family's script is driven and nothing is
    /// checked — the dispatcher behaves exactly as it did before `--roles`.
    #[test]
    fn without_a_manifest_every_role_is_driven() {
        for role in ["builder", "librarian", "architect", "intruder", ""] {
            let plan = plan_for(role, None);
            assert_eq!(
                plan,
                Plan::Drive(workflow_for(role)),
                "{role:?} is driven by its own family's script"
            );
        }
    }

    /// The plan with a manifest is the grants: a builder writes, a librarian
    /// only searches, a role that may read but not write falls back to the
    /// read-only script, and a role with nothing drivable is left alone.
    #[test]
    fn the_plan_is_the_roles_grants() {
        let roles = manifest(&[
            (
                "builder",
                &["run", "run_tests", "read", "edit", "find", "search"],
            ),
            ("librarian", &["read_memory", "search", "transcript_search"]),
            ("architect", &["run", "run_tests", "read", "find", "search"]),
            ("auditor", &["read_audit", "read_usage", "search"]),
            (
                "incident",
                &["request_grant", "quarantine_agent", "file_detection"],
            ),
        ]);
        let roles = Some(&roles);

        // Granted: the family's own script.
        assert_eq!(
            plan_for("builder", roles),
            Plan::Drive(&BUILDER_FAMILY),
            "a builder's shell and file tools cover its round trip"
        );
        assert_eq!(
            plan_for("librarian", roles),
            Plan::Drive(&READ_ONLY_FAMILY),
            "a librarian's script is already read-only"
        );
        // The codex's architect declares no `edit` — but it does declare `run`,
        // and a shell can write, so its round trip is granted through the
        // shell. (This is the implication stated in the mapping, taken
        // literally.)
        assert_eq!(
            plan_for("architect", roles),
            Plan::Drive(&DEFAULT_FAMILY),
            "a shell grant covers the default's write"
        );
        // A role that may read and search but has no write *and* no shell — the
        // auditor — falls back to the read-only script.
        assert_eq!(
            plan_for("auditor", roles),
            Plan::Drive(&READ_ONLY_FAMILY),
            "no write grant and no shell: the read-only fallback"
        );
        // A role the harness implements nothing for cannot be driven at all.
        assert_eq!(
            plan_for("incident", roles),
            Plan::Skipped("the role grants no tool this dispatcher may drive")
        );
        // A role the manifest does not carry at all: the hub would refuse every
        // call, so nothing is driven.
        assert_eq!(
            plan_for("intruder", roles),
            Plan::Skipped("the role is not in the manifest")
        );
        assert_eq!(
            plan_for("", roles),
            Plan::Skipped("the role is not in the manifest"),
            "an unnamed agent is not in the manifest either"
        );
    }

    /// The write-then-read workflows: each writes the task's own line into its
    /// artifact, then reads it back. The line is task-derived, and a multi-line
    /// goal is one line in the artifact.
    #[test]
    fn the_write_then_read_workflows_write_the_task_line() {
        // The default workflow's script: write the task's own line, read it back.
        // The builder's script: the same round trip, into its own artifact.
        for (case, role, id, goal, artifact, line) in [
            (
                "the default workflow",
                "architect",
                "t7",
                "draft\nthe handbook",
                "notes.txt",
                "task t7: draft the handbook",
            ),
            (
                "the builder workflow",
                "builder",
                "t3",
                "raise the plaza",
                "build-notes.txt",
                "task t3: raise the plaza",
            ),
        ] {
            let (run, agent) = run(role, &task(id, goal, role));
            assert_eq!(
                run.expect("the workflow runs"),
                Report::Artifacts(vec![artifact.to_string()]),
                "{case} reports its artifact"
            );
            assert_eq!(agent.sequence(), vec!["edit", "read"], "{case}");
            assert_eq!(
                agent.args(0)["create"][0]["path"],
                artifact,
                "{case} writes its artifact"
            );
            assert_eq!(
                agent.args(0)["create"][0]["content"],
                line,
                "{case} writes the task line"
            );
            assert_eq!(
                agent.args(1)["files"][0]["path"],
                artifact,
                "{case} reads it back"
            );
            assert_eq!(agent.calls(), 2, "{case}");
        }
    }

    /// The librarian's script writes nothing at all: it searches for the token
    /// the task is about, and the note it finishes with carries what the search
    /// found. This is the codex's librarian — `search` and nothing else.
    #[test]
    fn the_librarian_workflow_reads_and_never_writes() {
        let mut agent = FakeAgent::default()
            .with_file("handbook.md", "the plaza's handbook\nmemory curation\n");
        let run = read_only_workflow(&task("t5", "memory curation", "librarian"), &mut agent)
            .expect("the workflow runs");

        // One call, and it is a read: nothing was written to the workspace.
        assert_eq!(agent.sequence(), vec!["search"]);
        assert_eq!(agent.arg(0, "pattern"), "memory");
        assert_eq!(agent.arg(0, "path"), ".");
        assert_eq!(
            agent.files().keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["handbook.md"],
            "the workspace holds exactly what it held before"
        );

        // The note is the artifact: the tool's own counts and its first match.
        let Report::Summary(line) = &run else {
            panic!("a read-only run reports a summary, not artifacts: {run:?}");
        };
        assert_eq!(
            line,
            "read-only: search \"memory\" matched 1 line(s) in 1 file(s); \
             first: handbook.md:2: memory curation"
        );
        assert!(run.artifacts().is_empty());
        assert_eq!(
            done_note(&run, agent.calls()),
            format!("{line} (1 tool call(s))")
        );
    }

    /// A search that matched nothing is reported as matching nothing: the note
    /// quotes the tool, it does not invent a finding.
    #[test]
    fn a_read_only_run_that_found_nothing_says_so() {
        let mut agent = FakeAgent::default();
        let run = read_only_workflow(&task("t5", "nothing here", "librarian"), &mut agent)
            .expect("the workflow runs");
        assert_eq!(
            run,
            Report::Summary(
                "read-only: search \"nothing\" matched 0 line(s) in 0 file(s)".to_string()
            )
        );
        // And an answer that is not a search result is a failure, not an empty
        // note.
        let mut agent = FakeAgent::answering("the tool said something else");
        let err = read_only_workflow(&task("t5", "memory", "librarian"), &mut agent)
            .expect_err("not a search result");
        assert!(matches!(err, DriveError::BadOutput { .. }), "{err:?}");
        assert!(!err.is_retryable());
    }

    /// The search token: the first word of the goal, the task's id when the goal
    /// is blank, and never longer than the cap a `search` call can take.
    #[test]
    fn the_search_token_comes_from_the_task_and_is_bounded() {
        assert_eq!(
            search_pattern(&task("t5", "memory curation", "librarian")),
            "memory"
        );
        assert_eq!(
            search_pattern(&task("t5", "  \n memory\tcuration ", "librarian")),
            "memory"
        );
        assert_eq!(search_pattern(&task("t5", "", "librarian")), "t5");
        assert_eq!(search_pattern(&task("t5", "   ", "librarian")), "t5");
        let long = search_pattern(&task(
            "t5",
            &"é".repeat(MAX_PATTERN_CHARS + 40),
            "librarian",
        ));
        assert_eq!(long.chars().count(), MAX_PATTERN_CHARS);
    }

    /// A read-back that does not match what was written is a failure, not a
    /// success with a hopeful note — and it is not retryable: the agent
    /// answered, and it will answer the same way.
    #[test]
    fn a_read_back_that_does_not_match_is_a_bad_output() {
        let mut agent = FakeAgent::corrupting("something else entirely");
        let err = builder_workflow(&task("t3", "raise the plaza", "builder"), &mut agent)
            .expect_err("the round trip did not hold");
        assert!(matches!(err, DriveError::BadOutput { .. }), "{err:?}");
        assert!(!err.is_retryable());
        assert!(err.note().contains("read back"), "{}", err.note());
        assert_eq!(agent.calls(), 2);
    }

    /// A refusal is the agent's own answer and is not retryable; a call that
    /// could not be made is retryable. The note names which, and what.
    #[test]
    fn a_refusal_is_final_and_unreachability_is_retryable() {
        let refusal = DriveError::Refused {
            name: "edit".to_string(),
            output: "denied: path \"../x\" contains \"..\"".to_string(),
        };
        let mut agent = FakeAgent::failing(refusal.clone());
        let err = builder_workflow(&task("t3", "g", "builder"), &mut agent)
            .expect_err("the agent refused");
        assert_eq!(err, refusal);
        assert!(!err.is_retryable());
        assert_eq!(
            err.note(),
            "tool edit refused: denied: path \"../x\" contains \"..\""
        );

        let unreachable = DriveError::Unreachable {
            name: "edit".to_string(),
            reason: "the agent did not answer within 5000 ms".to_string(),
        };
        let mut agent = FakeAgent::failing(unreachable.clone());
        let err = builder_workflow(&task("t3", "g", "builder"), &mut agent)
            .expect_err("the agent did not answer");
        assert!(err.is_retryable());
        assert_eq!(
            err.note(),
            "tool edit unavailable: the agent did not answer within 5000 ms"
        );
    }

    /// The done note names the artifacts and how many tool calls it took — the
    /// one line a finished task's record carries.
    #[test]
    fn the_done_note_names_the_artifacts_and_the_call_count() {
        let mut agent = FakeAgent::default();
        let report = builder_workflow(&task("t3", "raise the plaza", "builder"), &mut agent)
            .expect("the workflow runs");
        assert_eq!(
            done_note(&report, agent.calls()),
            "artifacts: build-notes.txt (2 tool call(s))"
        );
        assert_eq!(
            done_note(&Report::Artifacts(vec![]), 0),
            "artifacts:  (0 tool call(s))"
        );
        assert_eq!(
            done_note(
                &Report::Artifacts(vec!["a.txt".to_string(), "b/index.txt".to_string()]),
                7
            ),
            "artifacts: a.txt, b/index.txt (7 tool call(s))"
        );
        assert_eq!(
            done_note(&Report::Summary("read-only: 3 match(es)".to_string()), 1),
            "read-only: 3 match(es) (1 tool call(s))"
        );
    }

    /// The search rendering, on the shapes a broken agent would send: only the
    /// `search` shape renders, a cut-short answer says so, and a guest's match
    /// text cannot grow the note past its bound.
    #[test]
    fn the_search_rendering_is_the_tools_own_answer() {
        for bad in [
            "not json at all",
            r#"{"pattern":"x"}"#,
            r#"{"matches":"none"}"#,
        ] {
            let err = render_search("p", bad).expect_err("not a search result");
            assert!(
                matches!(err, DriveError::BadOutput { .. }),
                "{bad}: {err:?}"
            );
        }

        let rendered = render_search(
            "p",
            r#"{"matches":[{"path":"a.txt","line":3,"text":"p here","before":[],"after":[]}],
                "truncated":false}"#,
        )
        .expect("a search result renders");
        assert_eq!(
            rendered,
            "read-only: search \"p\" matched 1 line(s) in 1 file(s); first: a.txt:3: p here"
        );

        let cut = render_search(
            "p",
            r#"{"matches":[{"path":"a.txt","line":1,"text":"p"}],"truncated":true}"#,
        )
        .expect("a search result renders");
        assert!(cut.contains("cut short"), "{cut}");
        assert!(cut.contains("1 file(s)"), "{cut}");

        // A guest's enormous match text is bounded, and so is the whole line.
        let huge = render_search(
            "p",
            &format!(
                r#"{{"matches":[{{"path":"a.txt","line":1,"text":"{}"}}]}}"#,
                "x".repeat(100_000)
            ),
        )
        .expect("a search result renders");
        assert!(
            huge.chars().count() <= MAX_NOTE_DETAIL + 1,
            "{} chars",
            huge.chars().count()
        );
    }

    /// Whatever a guest says is bounded before it reaches a note, however many
    /// bytes and however many of them are not one byte wide.
    #[test]
    fn a_guests_words_are_bounded_before_they_reach_a_note() {
        let short = "denied: nope";
        assert_eq!(bounded(short), short);

        let long = "é".repeat(MAX_NOTE_DETAIL + 10);
        let bounded_long = bounded(&long);
        assert_eq!(bounded_long.chars().count(), MAX_NOTE_DETAIL + 1);
        assert!(bounded_long.ends_with('…'));

        // A refusal whose output is enormous still makes a note the ledger
        // will take.
        let err = DriveError::Refused {
            name: "read".to_string(),
            output: "x".repeat(100_000),
        };
        assert!(err.note().len() < 1024, "{} bytes", err.note().len());
        assert!(err.note().ends_with('…'));
    }
}
