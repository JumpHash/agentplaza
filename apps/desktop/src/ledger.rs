//! The ledger's create form: four free-text fields, and the validation that
//! mirrors `noded`'s (`crates/noded/src/tasks.rs`, `CreateTask::checked`).
//!
//! The daemon is the authority; this only spares the operator a round trip for
//! a request it would refuse, with the same bounds and the same wording, so a
//! refusal reads the same whether the ledger or the daemon said it. The claim
//! and the finish are held to the same rule where they have one: the number of
//! the agent's guest CID, and the finish note's bound — both of which the
//! ledger's own pane checks before anything is sent.

use crate::api::CreateTask;

/// The most one task text field (the goal, the acceptance criteria, a finish
/// note) may be, in bytes (`noded::tasks::MAX_TEXT`).
pub const MAX_TEXT: usize = 4096;

/// The most a role name may be, in bytes (`noded::tasks::MAX_ROLE`).
pub const MAX_ROLE: usize = 64;

/// The most a task's budget may be: the schema's `budget_credits` is a
/// bigint, which is what `noded::tasks::CreateTask::checked` bounds it by.
pub const MAX_BUDGET: u64 = i64::MAX as u64;

/// One field of the ledger's create form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskField {
    /// What the task is for.
    Goal,
    /// How the work will be judged complete.
    Acceptance,
    /// The task's budget, in credits.
    Budget,
    /// The role the task is for.
    Role,
}

impl TaskField {
    /// The name the daemon's contract gives the field: what a refusal calls
    /// it, so the ledger's own refusal reads in the daemon's words.
    pub fn name(self) -> &'static str {
        match self {
            TaskField::Goal => "goal",
            TaskField::Acceptance => "acceptance",
            TaskField::Budget => "budget",
            TaskField::Role => "role",
        }
    }

    /// What the field holds, as the form labels it.
    pub fn label(self) -> &'static str {
        match self {
            TaskField::Goal => "what the task is for",
            TaskField::Acceptance => "how it will be judged complete (optional)",
            TaskField::Budget => "budget in credits (optional)",
            TaskField::Role => "the role it is for (optional)",
        }
    }

    /// This field's complaint from the last submission, if it had one.
    pub fn error<'a>(self, errors: &'a [(TaskField, String)]) -> Option<&'a str> {
        errors
            .iter()
            .find(|(field, _)| *field == self)
            .map(|(_, message)| message.as_str())
    }
}

/// The create form's raw input: what the operator typed, before validation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskForm {
    /// What the task is for.
    pub goal: String,
    /// How the work will be judged complete.
    pub acceptance: String,
    /// The budget, in credits.
    pub budget: String,
    /// The role the task is for.
    pub role: String,
}

impl TaskForm {
    /// Replaces one field with what is now in its box.
    pub fn set(&mut self, field: TaskField, value: String) {
        match field {
            TaskField::Goal => self.goal = value,
            TaskField::Acceptance => self.acceptance = value,
            TaskField::Budget => self.budget = value,
            TaskField::Role => self.role = value,
        }
    }

    /// Checks the form and answers the request the daemon accepts, or every
    /// field that is outside its contract.
    ///
    /// An optional box the operator left blank is absent rather than empty:
    /// the daemon refuses a blank `acceptance` or `role`, and "I did not say"
    /// is a different fact from "I said nothing".
    pub fn validate(&self) -> Result<CreateTask, Vec<(TaskField, String)>> {
        let mut errors: Vec<(TaskField, String)> = Vec::new();

        let goal = match required_text("goal", &self.goal, MAX_TEXT) {
            Ok(goal) => Some(goal),
            Err(why) => {
                errors.push((TaskField::Goal, why));
                None
            }
        };
        let acceptance = match optional_text("acceptance", &self.acceptance, MAX_TEXT) {
            Ok(acceptance) => Some(acceptance),
            Err(why) => {
                errors.push((TaskField::Acceptance, why));
                None
            }
        };
        let budget = match self.budget.trim() {
            "" => Some(None),
            typed => match typed.parse::<u64>() {
                Ok(budget) if budget <= MAX_BUDGET => Some(Some(budget)),
                Ok(_) => {
                    errors.push((
                        TaskField::Budget,
                        format!("budget must be at most {MAX_BUDGET}"),
                    ));
                    None
                }
                Err(_) => {
                    errors.push((
                        TaskField::Budget,
                        "budget must be a whole number of credits".to_string(),
                    ));
                    None
                }
            },
        };
        let role = match optional_text("role", &self.role, MAX_ROLE) {
            Ok(role) => Some(role),
            Err(why) => {
                errors.push((TaskField::Role, why));
                None
            }
        };

        match (goal, acceptance, budget, role) {
            (Some(goal), Some(acceptance), Some(budget), Some(role)) => Ok(CreateTask {
                goal,
                acceptance,
                budget,
                role,
            }),
            _ => Err(errors),
        }
    }
}

/// A required text field: present, not blank, and within its bound — the same
/// rule, and the same words, as `noded::tasks::required_text`.
fn required_text(name: &str, value: &str, max: usize) -> Result<String, String> {
    if value.trim().is_empty() {
        return Err(format!("{name} must not be empty"));
    }
    if value.len() > max {
        return Err(format!("{name} must be at most {max} bytes"));
    }
    Ok(value.to_string())
}

/// An optional text field: a blank box is "not given", anything else is held
/// to the required field's rule.
fn optional_text(name: &str, value: &str, max: usize) -> Result<Option<String>, String> {
    if value.trim().is_empty() {
        return Ok(None);
    }
    required_text(name, value, max).map(Some)
}

/// The finish note the daemon would accept, or why it would not: a blank box
/// is no note, and the bound is `noded::tasks::FinishRequest::checked`'s.
pub fn finish_note(note: &str) -> Result<Option<String>, String> {
    optional_text("note", note, MAX_TEXT)
}

/// The guest CID a claim would be made as, or why the box holds no CID.
///
/// The daemon reads a whole `u64`; a box that is blank, negative, or not a
/// number at all names the problem here rather than sending a request the
/// daemon's own parser would refuse.
pub fn agent_cid(typed: &str) -> Result<u64, String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return Err("claim: the agent CID box is empty".to_string());
    }
    typed
        .parse::<u64>()
        .map_err(|_| "claim: the agent CID must be a whole number".to_string())
}

/// The artifacts a task's finish note names, read back out of it; an empty list
/// when the note names none.
///
/// `dispatchd` finishes a task `done` with the note
/// `artifacts: <path>[, <path>…] (<n> tool call(s))` (`done_note`), and that one
/// line is the only place the workspace's artifact paths are recorded. This
/// reads the paths out of it as literal text — never a path *policy*: where a
/// path points and what it reaches is the agent's own harness's rule, and a
/// read of a path named here may still be refused.
///
/// A note of any other shape — a read-only run's summary line, an operator's
/// own words, a note still being written — names no artifacts and answers an
/// empty list rather than guessing a path out of prose. The call count the note
/// trails is not part of the list; a note that has not (yet) been given one is
/// still read, so the paths come out either way.
pub fn artifact_paths(note: &str) -> Vec<String> {
    let Some(list) = note.trim().strip_prefix("artifacts:") else {
        // Not the dispatcher's artifacts line: it names no artifact, and a
        // summary or a human's sentence is not a path.
        return Vec::new();
    };
    // The dispatcher writes the call count after the paths:
    // `artifacts: a.txt, b.txt (2 tool call(s))`. When that tail is there the
    // list ends where it begins; a note somebody wrote without it is read
    // whole, because the paths are the prefix either way.
    let list = match list.rfind(" (") {
        Some(at) if list.ends_with("tool call(s))") => &list[..at],
        _ => list,
    };
    list.split(',')
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A form with only a goal, as an operator files a bare task.
    fn form(goal: &str) -> TaskForm {
        TaskForm {
            goal: goal.to_string(),
            ..TaskForm::default()
        }
    }

    #[test]
    fn a_bare_goal_is_the_whole_request() {
        let create = form("ship the ledger")
            .validate()
            .expect("a goal is enough for the daemon");
        assert_eq!(create.goal, "ship the ledger");
        assert_eq!(create.acceptance, None, "a blank box is not an empty column");
        assert_eq!(create.budget, None);
        assert_eq!(create.role, None);
    }

    #[test]
    fn the_optional_boxes_are_taken_as_typed() {
        let create = TaskForm {
            goal: "  port the ledger  ".to_string(),
            acceptance: "the pane draws it".to_string(),
            budget: "500".to_string(),
            role: "librarian".to_string(),
        }
        .validate()
        .expect("every field is inside the contract");
        assert_eq!(
            create.goal, "  port the ledger  ",
            "the daemon keeps the goal as the caller wrote it"
        );
        assert_eq!(create.acceptance.as_deref(), Some("the pane draws it"));
        assert_eq!(create.budget, Some(500));
        assert_eq!(create.role.as_deref(), Some("librarian"));
    }

    #[test]
    fn an_empty_goal_is_refused_in_the_daemons_words() {
        for goal in ["", "   ", "\t\n"] {
            let errors = form(goal).validate().expect_err("a blank goal is refused");
            assert_eq!(errors.len(), 1);
            assert_eq!(errors[0].0, TaskField::Goal);
            assert_eq!(errors[0].1, "goal must not be empty");
            assert_eq!(
                TaskField::Goal.error(&errors),
                Some("goal must not be empty")
            );
        }
    }

    #[test]
    fn a_box_over_its_bound_is_refused_in_the_daemons_words() {
        let errors = form(&"x".repeat(MAX_TEXT + 1))
            .validate()
            .expect_err("a goal past the bound is refused");
        assert_eq!(errors[0].1, "goal must be at most 4096 bytes");

        let mut over = form("g");
        over.role = "r".repeat(MAX_ROLE + 1);
        let errors = over.validate().expect_err("a role past the bound is refused");
        assert_eq!(errors[0].0, TaskField::Role);
        assert_eq!(errors[0].1, "role must be at most 64 bytes");
        assert_eq!(over.clone().validate().unwrap_err().len(), 1);
        // Exactly at the bound is inside it.
        let mut at = form("g");
        at.role = "r".repeat(MAX_ROLE);
        assert_eq!(at.validate().expect("at the bound").role.unwrap().len(), MAX_ROLE);
    }

    #[test]
    fn a_budget_that_is_not_a_whole_number_is_refused_here() {
        for typed in ["12.5", "-1", "1e3", "9 9", "many"] {
            let errors = TaskForm {
                budget: typed.to_string(),
                ..form("g")
            }
            .validate()
            .expect_err("a budget the daemon cannot parse is refused");
            assert_eq!(errors[0].0, TaskField::Budget);
            assert_eq!(errors[0].1, "budget must be a whole number of credits");
        }
        let errors = TaskForm {
            budget: (MAX_BUDGET + 1).to_string(),
            ..form("g")
        }
        .validate()
        .expect_err("a budget past the schema's bigint is refused");
        assert_eq!(errors[0].1, format!("budget must be at most {MAX_BUDGET}"));
        assert_eq!(
            TaskForm {
                budget: MAX_BUDGET.to_string(),
                ..form("g")
            }
            .validate()
            .expect("the schema's own ceiling")
            .budget,
            Some(MAX_BUDGET)
        );
    }

    #[test]
    fn every_broken_field_is_named_at_once() {
        let errors = TaskForm {
            goal: String::new(),
            budget: "lots".to_string(),
            role: "r".repeat(MAX_ROLE + 1),
            ..TaskForm::default()
        }
        .validate()
        .expect_err("three fields are outside the contract");
        assert_eq!(errors.len(), 3);
        let named: Vec<TaskField> = errors.iter().map(|(field, _)| *field).collect();
        assert_eq!(
            named,
            vec![TaskField::Goal, TaskField::Budget, TaskField::Role],
            "the refusal names every field, in the form's own order"
        );
        assert!(TaskField::Acceptance.error(&errors).is_none());
    }

    #[test]
    fn a_field_is_set_by_name() {
        let mut form = TaskForm::default();
        form.set(TaskField::Goal, "g".to_string());
        form.set(TaskField::Acceptance, "a".to_string());
        form.set(TaskField::Budget, "7".to_string());
        form.set(TaskField::Role, "builder".to_string());
        assert_eq!(
            form,
            TaskForm {
                goal: "g".to_string(),
                acceptance: "a".to_string(),
                budget: "7".to_string(),
                role: "builder".to_string(),
            }
        );
        assert_eq!(TaskField::Goal.name(), "goal");
        assert_eq!(TaskField::Acceptance.name(), "acceptance");
        assert_eq!(TaskField::Budget.name(), "budget");
        assert_eq!(TaskField::Role.name(), "role");
    }

    #[test]
    fn a_finish_note_is_absent_when_blank_and_bounded_like_the_daemons() {
        assert_eq!(finish_note(""), Ok(None));
        assert_eq!(finish_note("   "), Ok(None));
        assert_eq!(finish_note("done early"), Ok(Some("done early".to_string())));
        assert_eq!(
            finish_note(&"x".repeat(MAX_TEXT + 1)),
            Err("note must be at most 4096 bytes".to_string())
        );
    }

    #[test]
    fn an_agent_cid_must_be_a_whole_number() {
        assert_eq!(agent_cid(" 112 "), Ok(112));
        assert_eq!(agent_cid("0"), Ok(0));
        for typed in ["", "   ", "-1", "9.5", "cid 112", "1e3"] {
            assert!(agent_cid(typed).is_err(), "{typed:?}");
        }
        assert_eq!(
            agent_cid("").unwrap_err(),
            "claim: the agent CID box is empty"
        );
        assert_eq!(
            agent_cid("nine").unwrap_err(),
            "claim: the agent CID must be a whole number"
        );
    }

    /// The finish note is the only place a task's artifact paths are recorded:
    /// the dispatcher's `done_note` line, read for the paths it names.
    #[test]
    fn a_finish_notes_artifacts_are_read_back_out_of_it() {
        // The dispatcher's own note, byte for byte (`done_note`).
        assert_eq!(
            artifact_paths("artifacts: build-notes.txt (2 tool call(s))"),
            vec!["build-notes.txt".to_string()]
        );
        // Several artifacts, joined with ", ".
        assert_eq!(
            artifact_paths("artifacts: index.txt, notes.txt (3 tool call(s))"),
            vec!["index.txt".to_string(), "notes.txt".to_string()]
        );
        // A run that wrote nothing names an empty list, and gets an empty one.
        assert!(artifact_paths("artifacts:  (0 tool call(s))").is_empty());
        // A note without the call count still names its paths — the paths are
        // the prefix either way, and the count is not the list.
        assert_eq!(
            artifact_paths("artifacts: notes.txt"),
            vec!["notes.txt".to_string()]
        );
        // The note's own trim is not a path, and neither is the space after a
        // comma.
        assert_eq!(
            artifact_paths("  artifacts: a.txt,b.txt  "),
            vec!["a.txt".to_string(), "b.txt".to_string()]
        );
    }

    /// A note that is not the dispatcher's artifacts line names no artifact:
    /// the pane has nothing to offer rather than a path guessed out of prose.
    #[test]
    fn a_note_that_is_not_the_artifacts_line_names_nothing() {
        for note in [
            "",
            "   ",
            "every negative covered",
            // A read-only run's note is a summary, and its artifact *is* the
            // line itself: there is no file to read.
            "read-only: search \"plaza\" -> 2 match(es) in index.txt (1 tool call(s))",
            // A capital A is a different line, not the dispatcher's.
            "Artifacts: notes.txt (1 tool call(s))",
            // The call count with no paths before it is nothing.
            "artifacts: (0 tool call(s))",
        ] {
            assert!(artifact_paths(note).is_empty(), "{note:?} names no artifact");
        }
    }

    /// The helper reads text and only text: a path it reports is a candidate
    /// the agent's own harness still judges, and no path policy is invented
    /// here.
    #[test]
    fn the_artifacts_line_is_not_a_path_policy() {
        assert_eq!(
            artifact_paths("artifacts: ../escape (1 tool call(s))"),
            vec!["../escape".to_string()],
            "the helper does not sanitize a path; it reports what the note says"
        );
    }
}
