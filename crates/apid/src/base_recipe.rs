//! The base recipe (PLAN 16.1, 16.4, 21.1): the steps that build a project's base from its declaration (project_stack.rs).
//!
//! A base is a VM built once per project, pool and kind, then snapshotted; forks start from its snapshot. The recipe is a
//! typed list of steps, run in order: each command runs in the VM through noded, and the checkout is fetched from Weft
//! through the node's link. Nothing here starts a VM: the caller gives a [`VmRunner`] (noded's, or a fake in tests).
//!
//! The recipe engine (4.1) takes this list over as the built-in `base` recipe (21.1): the steps and their order move
//! there unchanged. Its one caller, the hub's build of a base on a landing (16.4), comes with 3.3.

// Nothing outside the tests builds a base yet (3.3 does); until then the module is only run by its tests.
#![allow(dead_code)]

use apz_lsp::{Language, ServerSpec};

use crate::project_stack::{Stack, Toolchain};

/// The guest image a base starts from: the OS template every node has (noded's workspace image).
pub const OS_TEMPLATE: &str = "workspace";
/// The checkout's directory in the VM: the agent's `/workspace`.
pub const WORKSPACE: &str = "/workspace";
/// How long one command may run before its step fails (30 minutes).
pub const STEP_TIMEOUT_S: u64 = 1800;

/// The kinds of base (16.1) this recipe builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseKind {
    /// The toolchains, the checkout, the dependencies, the build outputs, and the language servers, warm.
    Build,
    /// A build base with the project's services running and its test setup done.
    Test,
}

impl BaseKind {
    pub fn name(self) -> &'static str {
        match self {
            BaseKind::Build => "build",
            BaseKind::Test => "test",
        }
    }
}

/// One step of the base recipe, run in order in the base's VM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Starts the VM from the guest image `template`.
    Start { template: &'static str },
    /// Runs a command and waits for it: it must exit 0 within its timeout.
    Run(Command),
    /// Starts a process and leaves it running (a language server, a service); the next step does not wait for it.
    Background(Command),
    /// Checks out `commit` of the Weft repository `repo` in [`WORKSPACE`].
    Checkout { repo: String, commit: String },
    /// Stops the VM and snapshots it as the base of `kind`.
    Snapshot { kind: BaseKind },
}

/// A command in the VM: its argument vector (never run through a shell), the directory it runs in, and how long it
/// may run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub label: String,
    pub argv: Vec<String>,
    pub cwd: &'static str,
    pub timeout_s: u64,
}

impl Command {
    fn new(label: &str, argv: &[&str]) -> Command {
        Command {
            label: label.to_string(),
            argv: argv.iter().map(|part| part.to_string()).collect(),
            cwd: WORKSPACE,
            timeout_s: STEP_TIMEOUT_S,
        }
    }
}

impl Step {
    /// What the step is called in the build's log and its failure.
    pub fn label(&self) -> String {
        match self {
            Step::Start { .. } => "start the VM".into(),
            Step::Run(command) | Step::Background(command) => command.label.clone(),
            Step::Checkout { commit, .. } => format!("check out {commit}"),
            Step::Snapshot { kind } => format!("snapshot the {} base", kind.name()),
        }
    }
}

/// Runs the base's steps: noded's commands in the VM, or a fake in tests.
pub trait VmRunner {
    fn run(&mut self, step: &Step) -> Result<(), String>;
}

/// The steps that build a base of `kind` from `stack`, checked out at `commit` of the Weft repository `repo`.
pub fn steps(stack: &Stack, kind: BaseKind, repo: &str, commit: &str) -> Vec<Step> {
    let mut steps = vec![Step::Start {
        template: OS_TEMPLATE,
    }];
    for (toolchain, version) in &stack.toolchains {
        let label = format!("install {} {version}", toolchain.name());
        for argv in toolchain.install(version) {
            steps.push(Step::Run(Command::new(&label, &argv_refs(&argv))));
        }
    }
    steps.push(Step::Checkout {
        repo: repo.to_string(),
        commit: commit.to_string(),
    });
    for argv in commands(stack.build.fetch.as_ref(), stack, Toolchain::fetch) {
        steps.push(Step::Run(Command::new(
            "fetch dependencies",
            &argv_refs(&argv),
        )));
    }
    for argv in commands(stack.build.build.as_ref(), stack, Toolchain::build) {
        steps.push(Step::Run(Command::new("build", &argv_refs(&argv))));
    }
    for spec in servers(&stack.languages) {
        let label = format!("start {}", spec.id);
        steps.push(Step::Background(Command::new(&label, &server_argv(spec))));
    }
    if kind == BaseKind::Test {
        for service in &stack.services {
            let label = format!("start service {}", service.name);
            steps.push(Step::Background(Command::new(
                &label,
                &argv_refs(&service.command),
            )));
        }
        for argv in &stack.test.setup {
            steps.push(Step::Run(Command::new("test setup", &argv_refs(argv))));
        }
    }
    steps.push(Step::Snapshot { kind });
    steps
}

/// Runs `steps` in order on `runner`. The first failure stops the build, and its error names the step.
pub fn execute(runner: &mut dyn VmRunner, steps: &[Step]) -> Result<(), String> {
    for (i, step) in steps.iter().enumerate() {
        runner
            .run(step)
            .map_err(|why| format!("base step {} ({}): {why}", i + 1, step.label()))?;
    }
    Ok(())
}

/// The commands of one phase: the declaration's list when it has one, else each toolchain's defaults in order.
fn commands(
    declared: Option<&Vec<Vec<String>>>,
    stack: &Stack,
    default: fn(Toolchain) -> Vec<Vec<String>>,
) -> Vec<Vec<String>> {
    match declared {
        Some(list) => list.clone(),
        None => stack
            .toolchains
            .keys()
            .flat_map(|toolchain| default(*toolchain))
            .collect(),
    }
}

/// The servers of the declared languages, one per server (TypeScript and JavaScript share one).
fn servers(languages: &[Language]) -> Vec<&'static ServerSpec> {
    let mut specs: Vec<&'static ServerSpec> = Vec::new();
    for language in languages {
        let spec = language.server();
        if !specs.iter().any(|known| known.id == spec.id) {
            specs.push(spec);
        }
    }
    specs
}

fn server_argv(spec: &ServerSpec) -> Vec<&str> {
    std::iter::once(spec.command)
        .chain(spec.args.iter().copied())
        .collect()
}

fn argv_refs(argv: &[String]) -> Vec<&str> {
    argv.iter().map(String::as_str).collect()
}

impl Toolchain {
    /// The commands that install the toolchain at `version` in the VM. Rust's components include rust-analyzer, the
    /// language server the base starts.
    fn install(self, version: &str) -> Vec<Vec<String>> {
        let argv =
            |parts: &[&str]| -> Vec<String> { parts.iter().map(|part| part.to_string()).collect() };
        match self {
            Toolchain::Rust => vec![
                argv(&[
                    "rustup",
                    "toolchain",
                    "install",
                    version,
                    "--profile",
                    "minimal",
                    "--component",
                    "clippy",
                    "--component",
                    "rustfmt",
                    "--component",
                    "rust-analyzer",
                ]),
                argv(&["rustup", "default", version]),
            ],
            Toolchain::Node => vec![
                argv(&["fnm", "install", version]),
                argv(&["fnm", "default", version]),
            ],
            Toolchain::Python => vec![argv(&["uv", "python", "install", version])],
        }
    }

    /// The dependency fetch when the declaration gives none.
    fn fetch(self) -> Vec<Vec<String>> {
        match self {
            Toolchain::Rust => vec![vec!["cargo".into(), "fetch".into()]],
            Toolchain::Node => vec![vec!["npm".into(), "ci".into()]],
            Toolchain::Python => vec![vec!["uv".into(), "sync".into()]],
        }
    }

    /// The build when the declaration gives none: Rust builds the test binaries too, so `target/` is warm for tests.
    fn build(self) -> Vec<Vec<String>> {
        match self {
            Toolchain::Rust => vec![vec!["cargo".into(), "build".into(), "--all-targets".into()]],
            Toolchain::Node => vec![vec![
                "npm".into(),
                "run".into(),
                "build".into(),
                "--if-present".into(),
            ]],
            Toolchain::Python => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_stack::parse;

    /// The sample declaration of project_stack.rs, with Rust and Node and a test setup.
    const SAMPLE: &str = r#"
languages = ["rust", "typescript"]

[toolchains]
rust = "1.88.0"
node = "24"

[[services]]
name = "postgres"
command = ["postgres", "-D", "/var/lib/pg"]

[test]
setup = [["make", "seed"]]
"#;

    fn start() -> Step {
        Step::Start {
            template: OS_TEMPLATE,
        }
    }

    fn run(label: &str, argv: &[&str]) -> Step {
        Step::Run(Command::new(label, argv))
    }

    fn background(label: &str, argv: &[&str]) -> Step {
        Step::Background(Command::new(label, argv))
    }

    /// Records every step it is given; fails at step `fail_at` (0-based) when set.
    #[derive(Default)]
    struct FakeRunner {
        seen: Vec<Step>,
        fail_at: Option<usize>,
    }

    impl VmRunner for FakeRunner {
        fn run(&mut self, step: &Step) -> Result<(), String> {
            self.seen.push(step.clone());
            if self.fail_at == Some(self.seen.len() - 1) {
                return Err("exit 1".into());
            }
            Ok(())
        }
    }

    #[test]
    fn a_sample_declaration_builds_the_build_base_in_order() {
        let stack = parse(SAMPLE).unwrap();
        let steps = steps(&stack, BaseKind::Build, "repo-1", "abc123");
        assert_eq!(
            steps,
            vec![
                start(),
                run(
                    "install rust 1.88.0",
                    &[
                        "rustup",
                        "toolchain",
                        "install",
                        "1.88.0",
                        "--profile",
                        "minimal",
                        "--component",
                        "clippy",
                        "--component",
                        "rustfmt",
                        "--component",
                        "rust-analyzer",
                    ]
                ),
                run("install rust 1.88.0", &["rustup", "default", "1.88.0"]),
                run("install node 24", &["fnm", "install", "24"]),
                run("install node 24", &["fnm", "default", "24"]),
                Step::Checkout {
                    repo: "repo-1".into(),
                    commit: "abc123".into(),
                },
                run("fetch dependencies", &["cargo", "fetch"]),
                run("fetch dependencies", &["npm", "ci"]),
                run("build", &["cargo", "build", "--all-targets"]),
                run("build", &["npm", "run", "build", "--if-present"]),
                background("start rust-analyzer", &["rust-analyzer"]),
                background(
                    "start typescript-language-server",
                    &["typescript-language-server", "--stdio"],
                ),
                Step::Snapshot {
                    kind: BaseKind::Build,
                },
            ]
        );
    }

    #[test]
    fn the_steps_run_in_the_workspace_within_the_step_timeout() {
        let stack = parse(SAMPLE).unwrap();
        for step in steps(&stack, BaseKind::Build, "r", "c") {
            if let Step::Run(command) | Step::Background(command) = step {
                assert_eq!(command.cwd, WORKSPACE);
                assert_eq!(command.timeout_s, STEP_TIMEOUT_S);
            }
        }
    }

    #[test]
    fn the_test_base_is_the_build_base_then_its_services_and_setup_before_its_snapshot() {
        let stack = parse(SAMPLE).unwrap();
        let build = steps(&stack, BaseKind::Build, "repo-1", "abc123");
        let test = steps(&stack, BaseKind::Test, "repo-1", "abc123");
        let shared = build.len() - 1;
        assert_eq!(test[..shared], build[..shared]);
        assert_eq!(
            test[shared..].to_vec(),
            vec![
                background("start service postgres", &["postgres", "-D", "/var/lib/pg"]),
                run("test setup", &["make", "seed"]),
                Step::Snapshot {
                    kind: BaseKind::Test
                },
            ]
        );
    }

    #[test]
    fn the_declared_commands_replace_the_defaults_and_an_empty_list_removes_them() {
        let stack = parse(
            "languages = []\n[toolchains]\nrust = \"1.88.0\"\n[build]\nfetch = [[\"just\", \"fetch\"]]\nbuild = []\n",
        )
        .unwrap();
        let steps = steps(&stack, BaseKind::Build, "r", "c");
        let phases: Vec<Step> = steps
            .iter()
            .filter(|s| matches!(s, Step::Run(c) if c.label != "install rust 1.88.0"))
            .cloned()
            .collect();
        assert_eq!(phases, vec![run("fetch dependencies", &["just", "fetch"])]);
    }

    #[test]
    fn a_python_base_has_no_build_command_and_typescript_and_javascript_share_one_server() {
        let stack = parse(
            "languages = [\"python\", \"javascript\", \"typescript\"]\n[toolchains]\npython = \"3.13\"\n",
        )
        .unwrap();
        let steps = steps(&stack, BaseKind::Build, "r", "c");
        let runs: Vec<String> = steps
            .iter()
            .filter_map(|s| match s {
                Step::Run(command) => Some(command.label.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(runs, vec!["install python 3.13", "fetch dependencies"]);
        let servers: Vec<Step> = steps
            .iter()
            .filter(|s| matches!(s, Step::Background(_)))
            .cloned()
            .collect();
        assert_eq!(
            servers,
            vec![
                background("start pyright", &["pyright-langserver", "--stdio"]),
                background(
                    "start typescript-language-server",
                    &["typescript-language-server", "--stdio"],
                ),
            ]
        );
    }

    #[test]
    fn the_runner_gets_every_step_in_order() {
        let stack = parse(SAMPLE).unwrap();
        let steps = steps(&stack, BaseKind::Build, "repo-1", "abc123");
        let mut runner = FakeRunner::default();
        execute(&mut runner, &steps).unwrap();
        assert_eq!(runner.seen, steps);
    }

    #[test]
    fn a_failing_step_stops_the_build_and_the_error_names_it() {
        let stack = parse(SAMPLE).unwrap();
        let steps = steps(&stack, BaseKind::Build, "repo-1", "abc123");
        let mut runner = FakeRunner {
            fail_at: Some(7),
            ..FakeRunner::default()
        };
        let err = execute(&mut runner, &steps).unwrap_err();
        assert_eq!(runner.seen.len(), 8, "no step after the failed one runs");
        assert_eq!(err, "base step 8 (fetch dependencies): exit 1");
    }
}
