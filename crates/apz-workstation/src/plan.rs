//! A plan: the commands and file writes an enrolment step will make, shown to the owner before anything runs.
//!
//! A [`Plan`] is data. [`Plan::render`] prints it and [`Plan::execute`] runs it, in order, and stops at the first
//! failure. A command that carries a secret (a generated password) keeps the secret in its arguments only: the
//! printed form is `shown`, and a failure names the program and its status, never the arguments.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// One thing a plan does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Runs `program` with `args`. `shown` is the same command line as printed, with secrets replaced.
    Run {
        program: String,
        args: Vec<String>,
        shown: Vec<String>,
    },
    /// Writes `contents` to `path`, creating its parent directories. `mode` is the unix permission bits (ignored on
    /// Windows).
    Write {
        path: PathBuf,
        contents: String,
        mode: u32,
    },
}

/// What an enrolment step will do, and what the owner must know first.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub steps: Vec<Step>,
    /// The steps need administrator rights (root, or an elevated Windows session).
    pub privileged: bool,
    /// Plain-language notes shown with the plan.
    pub notes: Vec<String>,
}

impl Plan {
    /// Adds a command whose printed form is its arguments.
    pub fn run<S: Into<String> + Clone>(&mut self, program: &str, args: &[S]) {
        let args: Vec<String> = args.iter().cloned().map(Into::into).collect();
        self.steps.push(Step::Run {
            program: program.to_string(),
            shown: args.clone(),
            args,
        });
    }

    /// Adds a command with a secret: `args` run, `shown` is printed.
    pub fn run_secret(&mut self, program: &str, args: Vec<String>, shown: Vec<String>) {
        self.steps.push(Step::Run {
            program: program.to_string(),
            args,
            shown,
        });
    }

    /// Adds a file write.
    pub fn write(&mut self, path: impl Into<PathBuf>, contents: impl Into<String>, mode: u32) {
        self.steps.push(Step::Write {
            path: path.into(),
            contents: contents.into(),
            mode,
        });
    }

    /// The plan as lines a person reads: one line per step, `$ ` for commands and `write ` for files.
    pub fn render(&self) -> Vec<String> {
        self.steps
            .iter()
            .map(|step| match step {
                Step::Run { program, shown, .. } => {
                    let mut words = vec![quote(program)];
                    words.extend(shown.iter().map(|a| quote(a)));
                    format!("$ {}", words.join(" "))
                }
                Step::Write { path, contents, .. } => {
                    format!("write {} ({} bytes)", path.display(), contents.len())
                }
            })
            .collect()
    }

    /// Runs the steps in order. The first failure stops the plan.
    pub fn execute(&self) -> Result<(), String> {
        for step in &self.steps {
            match step {
                Step::Run {
                    program,
                    args,
                    shown,
                } => {
                    let status = Command::new(program)
                        .args(args)
                        .status()
                        .map_err(|e| format!("{}: {e}", program))?;
                    if !status.success() {
                        return Err(format!(
                            "{} failed ({status}); the command was: {}",
                            program,
                            shown.join(" ")
                        ));
                    }
                }
                Step::Write {
                    path,
                    contents,
                    mode,
                } => {
                    if let Some(dir) = path.parent() {
                        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                    }
                    fs::write(path, contents).map_err(|e| format!("{}: {e}", path.display()))?;
                    set_mode(path, *mode)?;
                }
            }
        }
        Ok(())
    }
}

/// A plan as a person reads it: a heading, its steps indented, a marker when it needs administrator rights, and
/// its notes.
pub fn block(title: &str, plan: &Plan) -> Vec<String> {
    let mut lines = vec![format!("{title}:")];
    lines.extend(plan.render().into_iter().map(|line| format!("  {line}")));
    if plan.privileged {
        lines.push("  (needs administrator rights)".to_string());
    }
    lines.extend(plan.notes.iter().map(|note| format!("  note: {note}")));
    lines
}

/// Quotes one word for display when it has spaces.
fn quote(word: &str) -> String {
    if word.is_empty() || word.chars().any(|c| c.is_whitespace() || c == '"') {
        format!("\"{}\"", word.replace('"', "\\\""))
    } else {
        word.to_string()
    }
}

#[cfg(unix)]
fn set_mode(path: &std::path::Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(not(unix))]
fn set_mode(_path: &std::path::Path, _mode: u32) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_renders_its_commands_and_writes_in_order() {
        let mut plan = Plan::default();
        plan.run("useradd", &["--create-home", "agentplaza"]);
        plan.write("/tmp/x/unit", "[Unit]\n", 0o644);
        assert_eq!(
            plan.render(),
            vec![
                "$ useradd --create-home agentplaza".to_string(),
                "write /tmp/x/unit (7 bytes)".to_string(),
            ]
        );
    }

    #[test]
    fn a_secret_is_printed_as_its_shown_form_only() {
        let mut plan = Plan::default();
        plan.run_secret(
            "net",
            vec!["user".into(), "s3cret-value".into(), "/add".into()],
            vec!["user".into(), "********".into(), "/add".into()],
        );
        let text = plan.render().join("\n");
        assert!(!text.contains("s3cret-value"), "{text}");
        assert!(text.contains("********"));
    }

    #[test]
    fn words_with_spaces_are_quoted_for_display() {
        let mut plan = Plan::default();
        plan.run("sysadminctl", &["-fullName", "AgentPlaza agents"]);
        assert_eq!(
            plan.render(),
            vec!["$ sysadminctl -fullName \"AgentPlaza agents\"".to_string()]
        );
    }
}
