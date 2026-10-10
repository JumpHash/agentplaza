//! The scripted workflows: what a role family *does* with a task, and how the
//! dispatcher drives it.
//!
//! A workflow is a function from a task and a [`Driver`] to the artifact paths
//! the agent's workspace holds when it is done. The driver is the only thing a
//! workflow talks to, and it has exactly one method: run one tool call on the
//! agent and answer with the tool's own output. So a workflow is a *script* —
//! a sequence of tool calls whose arguments may depend on earlier answers —
//! and the transport (the hub's `POST /tell`, an agent's session, a tool
//! implementation inside a guest) is entirely behind the driver. That is what
//! makes the scripts unit-testable: a fake driver stands in for a whole agent
//! and records the sequence.
//!
//! The role → workflow table is keyed like the harness's own `TOOLS` table:
//! one entry per role family, adding one is one entry plus its function. A
//! role with no entry gets [`default_workflow`], so a fleet never has a role
//! the dispatcher cannot work with — only one it works with generically.
//!
//! Every workflow *proves* what it did rather than asserting it: each writes
//! and then reads its artifact back, and a read that does not match what was
//! written is a failure, not a success with a hopeful note. The finish note
//! names the artifacts and the number of tool calls it took.

use serde_json::json;

use crate::views::Task;

/// The most of a tool's output a note or a log line carries. A tool that
/// refuses something quotes what it was given; that quote is bounded here so
/// nothing a guest says can grow a task's note past the ledger's own bound.
pub const MAX_NOTE_DETAIL: usize = 512;

/// The artifacts a workflow left behind, or the reason it could not finish.
pub type Run = Result<Vec<String>, DriveError>;

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

/// The role → workflow table, keyed exactly like the harness's `TOOLS`: a role
/// family is one entry, and its function is the script it runs.
pub const WORKFLOWS: &[(&str, Workflow)] = &[
    ("builder", builder_workflow),
    ("librarian", librarian_workflow),
];

/// The workflow a role gets: its family's own, or the generic default.
pub fn workflow_for(role: &str) -> Workflow {
    WORKFLOWS
        .iter()
        .find(|(name, _)| *name == role)
        .map_or(default_workflow, |(_, workflow)| *workflow)
}

/// The generic default: write the task's own line into `notes.txt` and read it
/// back. Any role without a family of its own still leaves a real artifact, and
/// the round trip proves the agent's workspace really took it.
pub fn default_workflow(task: &Task, driver: &mut dyn Driver) -> Run {
    const ARTIFACT: &str = "notes.txt";
    write_then_read(driver, ARTIFACT, &task_line(task))?;
    Ok(vec![ARTIFACT.to_string()])
}

/// A builder's: the same round trip, into `build-notes.txt` — the notes a
/// build leaves behind, named for the thing the role builds.
pub fn builder_workflow(task: &Task, driver: &mut dyn Driver) -> Run {
    const ARTIFACT: &str = "build-notes.txt";
    write_then_read(driver, ARTIFACT, &task_line(task))?;
    Ok(vec![ARTIFACT.to_string()])
}

/// A librarian's: **list** the workspace first, then write the index from what
/// the listing actually holds — so the index is of the workspace as it is, not
/// as the dispatcher assumed — and read it back.
pub fn librarian_workflow(task: &Task, driver: &mut dyn Driver) -> Run {
    const ARTIFACT: &str = "index.txt";
    let listing = driver.call("list", json!({ "path": "." }))?;
    // A listing the workflow cannot read is the agent's answer, not an
    // artifact: the index would be a lie rather than a smaller index.
    let rendered = render_listing(&listing)?;
    let content = format!("{}\n{}", task_line(task), rendered);
    write_then_read(driver, ARTIFACT, &content)?;
    Ok(vec![ARTIFACT.to_string()])
}

/// The note a task is finished with when its workflow completed: the artifact
/// paths its workspace now holds and how many tool calls it took.
pub fn done_note(artifacts: &[String], calls: u64) -> String {
    format!("artifacts: {} ({calls} tool call(s))", artifacts.join(", "))
}

/// The line a workflow writes: the task's id and goal, on one line (a goal is
/// free text and may be many lines; the artifact is a line either way).
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
    driver.call("write_file", json!({ "path": path, "content": content }))?;
    let output = driver.call("read_file", json!({ "path": path }))?;
    let read = read_content(&output).ok_or_else(|| DriveError::BadOutput {
        name: "read_file".to_string(),
        reason: format!("the read of {path} carries no `content`"),
    })?;
    if read != content {
        return Err(DriveError::BadOutput {
            name: "read_file".to_string(),
            reason: format!(
                "{path} read back {} byte(s), not the {} written",
                read.len(),
                content.len()
            ),
        });
    }
    Ok(())
}

/// The `content` of a `read_file` result, when the answer is that shape.
fn read_content(output: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(output).ok()?;
    value
        .get("content")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// The `entries` of a `list` result, rendered as the index's body: one line
/// per entry, in the order the agent reported them (the harness sorts by
/// name), with the size for files.
fn render_listing(output: &str) -> Result<String, DriveError> {
    let bad = |reason: String| DriveError::BadOutput {
        name: "list".to_string(),
        reason,
    };
    let value: serde_json::Value = serde_json::from_str(output)
        .map_err(|err| bad(format!("the listing is not JSON: {err}")))?;
    let entries = value
        .get("entries")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| bad("the listing carries no `entries` array".to_string()))?;
    let mut lines = Vec::with_capacity(entries.len() + 1);
    lines.push(format!(
        "workspace: {} entr{}",
        entries.len(),
        if entries.len() == 1 { "y" } else { "ies" }
    ));
    for entry in entries {
        let name = entry
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<unnamed>");
        let kind = entry
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("?");
        match entry.get("size").and_then(serde_json::Value::as_u64) {
            Some(size) => lines.push(format!("{name}\t{kind}\t{size}")),
            None => lines.push(format!("{name}\t{kind}")),
        }
    }
    Ok(lines.join("\n"))
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
    fn run(role: &str, task: &Task) -> (Result<Vec<String>, DriveError>, FakeAgent) {
        let mut agent = FakeAgent::default();
        let run = workflow_for(role)(task, &mut agent);
        (run, agent)
    }

    /// The table is keyed by role, so each family runs its own script and every
    /// other role falls through to the default — and no key is repeated, since
    /// a duplicate would silently shadow the entry behind it.
    #[test]
    fn the_workflow_table_is_keyed_by_role() {
        let mut keys: Vec<&str> = WORKFLOWS.iter().map(|(name, _)| *name).collect();
        keys.sort_unstable();
        let count = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), count, "a role family is filed once: {keys:?}");

        let (builder, _) = run("builder", &task("t1", "g", "builder"));
        assert_eq!(
            builder.expect("the builder workflow runs"),
            vec!["build-notes.txt".to_string()]
        );
        let (librarian, _) = run("librarian", &task("t1", "g", "librarian"));
        assert_eq!(
            librarian.expect("the librarian workflow runs"),
            vec!["index.txt".to_string()]
        );
        for generic in ["architect", "builderx", "Builder", ""] {
            let (run, _) = run(generic, &task("t1", "g", generic));
            assert_eq!(
                run.expect("the default workflow runs"),
                vec!["notes.txt".to_string()],
                "{generic:?} has no family of its own"
            );
        }
    }

    /// The default workflow's script: write the task's own line, read it back.
    /// The line is task-derived, and a multi-line goal is one line in the
    /// artifact.
    #[test]
    fn the_default_workflow_writes_the_task_line_then_reads_it_back() {
        let (run, agent) = run("architect", &task("t7", "draft\nthe handbook", "architect"));
        assert_eq!(
            run.expect("the workflow runs"),
            vec!["notes.txt".to_string()]
        );
        assert_eq!(agent.sequence(), vec!["write_file", "read_file"]);
        assert_eq!(agent.arg(0, "path"), "notes.txt");
        assert_eq!(agent.arg(0, "content"), "task t7: draft the handbook");
        assert_eq!(agent.arg(1, "path"), "notes.txt");
        assert_eq!(agent.calls(), 2);
    }

    /// The builder's script: the same round trip, into its own artifact.
    #[test]
    fn the_builder_workflow_writes_build_notes() {
        let (run, agent) = run("builder", &task("t3", "raise the plaza", "builder"));
        assert_eq!(
            run.expect("the workflow runs"),
            vec!["build-notes.txt".to_string()]
        );
        assert_eq!(agent.sequence(), vec!["write_file", "read_file"]);
        assert_eq!(agent.arg(0, "path"), "build-notes.txt");
        assert_eq!(agent.arg(0, "content"), "task t3: raise the plaza");
        assert_eq!(agent.calls(), 2);
    }

    /// The librarian's script: list first, then write an index of what the
    /// workspace actually holds (the task's line, then the listing), then read
    /// it back.
    #[test]
    fn the_librarian_workflow_lists_the_workspace_into_an_index() {
        let (run, agent) = run("librarian", &task("t5", "index the plaza", "librarian"));
        assert_eq!(
            run.expect("the workflow runs"),
            vec!["index.txt".to_string()]
        );
        assert_eq!(agent.sequence(), vec!["list", "write_file", "read_file"]);
        assert_eq!(agent.arg(0, "path"), ".");
        assert_eq!(agent.arg(1, "path"), "index.txt");
        let index = agent.arg(1, "content");
        assert!(
            index.starts_with("task t5: index the plaza\nworkspace: 0 entries"),
            "{index:?}"
        );
        assert_eq!(agent.arg(2, "path"), "index.txt");
        assert_eq!(agent.calls(), 3);
    }

    /// The index is of the workspace as it is: an entry already there shows up
    /// in the listing the index is written from.
    #[test]
    fn the_librarians_index_is_of_the_workspace_it_found() {
        let mut agent = FakeAgent::default().with_file("handbook.md", "the plaza's handbook");
        let run = librarian_workflow(&task("t5", "index", "librarian"), &mut agent)
            .expect("the workflow runs");
        assert_eq!(run, vec!["index.txt".to_string()]);
        let index = agent
            .file("index.txt")
            .expect("the index was written to the agent's workspace");
        assert!(index.contains("workspace: 1 entry"), "{index:?}");
        assert!(index.contains("handbook.md\tfile\t20"), "{index:?}");
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

    /// A listing the workflow cannot read is the agent's answer, not an
    /// artifact: the index is not written from a guess.
    #[test]
    fn a_listing_that_is_not_a_listing_is_a_bad_output() {
        // The renderer's rule, on the shapes a broken agent would send.
        let err = render_listing("not json at all").expect_err("not a listing");
        assert!(matches!(err, DriveError::BadOutput { .. }), "{err:?}");
        let err = render_listing(r#"{"path":"."}"#).expect_err("no entries array");
        assert!(matches!(err, DriveError::BadOutput { .. }), "{err:?}");
        assert!(!err.is_retryable());

        // And a real listing renders deterministically.
        let listing = render_listing(
            r#"{"path":".","entries":[{"name":"a.txt","kind":"file","size":3},
                {"name":"sub","kind":"dir","size":null}],"truncated":false,"skipped":0}"#,
        )
        .expect("a listing renders");
        assert_eq!(listing, "workspace: 2 entries\na.txt\tfile\t3\nsub\tdir");

        // An agent that answers `list` with something else writes no index.
        let mut agent = FakeAgent::listing("not a listing");
        let err = librarian_workflow(&task("t5", "index", "librarian"), &mut agent)
            .expect_err("the listing is not a listing");
        assert!(matches!(err, DriveError::BadOutput { .. }), "{err:?}");
        assert_eq!(agent.sequence(), vec!["list"]);
        assert!(
            agent.files().is_empty(),
            "no index was written from a guess"
        );
    }

    /// A refusal is the agent's own answer and is not retryable; a call that
    /// could not be made is retryable. The note names which, and what.
    #[test]
    fn a_refusal_is_final_and_unreachability_is_retryable() {
        let refusal = DriveError::Refused {
            name: "write_file".to_string(),
            output: "denied: path \"../x\" contains \"..\"".to_string(),
        };
        let mut agent = FakeAgent::failing(refusal.clone());
        let err = builder_workflow(&task("t3", "g", "builder"), &mut agent)
            .expect_err("the agent refused");
        assert_eq!(err, refusal);
        assert!(!err.is_retryable());
        assert_eq!(
            err.note(),
            "tool write_file refused: denied: path \"../x\" contains \"..\""
        );

        let unreachable = DriveError::Unreachable {
            name: "write_file".to_string(),
            reason: "the agent did not answer within 5000 ms".to_string(),
        };
        let mut agent = FakeAgent::failing(unreachable.clone());
        let err = builder_workflow(&task("t3", "g", "builder"), &mut agent)
            .expect_err("the agent did not answer");
        assert!(err.is_retryable());
        assert_eq!(
            err.note(),
            "tool write_file unavailable: the agent did not answer within 5000 ms"
        );
    }

    /// The done note names the artifacts and how many tool calls it took — the
    /// one line a finished task's record carries.
    #[test]
    fn the_done_note_names_the_artifacts_and_the_call_count() {
        let mut agent = FakeAgent::default();
        let artifacts = builder_workflow(&task("t3", "raise the plaza", "builder"), &mut agent)
            .expect("the workflow runs");
        assert_eq!(
            done_note(&artifacts, agent.calls()),
            "artifacts: build-notes.txt (2 tool call(s))"
        );
        assert_eq!(done_note(&[], 0), "artifacts:  (0 tool call(s))");
        assert_eq!(
            done_note(&["a.txt".to_string(), "b/index.txt".to_string()], 7),
            "artifacts: a.txt, b/index.txt (7 tool call(s))"
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
            name: "read_file".to_string(),
            output: "x".repeat(100_000),
        };
        assert!(err.note().len() < 1024, "{} bytes", err.note().len());
        assert!(err.note().ends_with('…'));
    }
}
