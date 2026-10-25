//! The base recipe (PLAN 16.1, 16.4, 21.1): the steps that build a project's base from its declaration (project_stack.rs).
//!
//! A base is a VM built once per project, pool and kind, then snapshotted; forks start from its snapshot. The recipe is a
//! typed list of steps, run in order: each command runs in the VM through noded, and the checkout is fetched from Weft
//! through the node's link. Nothing here starts a VM: the caller gives a [`VmRunner`] (noded's, or a fake in tests).
//!
//! The VM mounts the node's shared volumes: the git objects, which its checkout borrows (16.6), and, for Rust, the
//! build cache that sccache keeps (16.5); the commands run with sccache as their compiler wrapper.
//!
//! The recipe engine (4.1) takes this list over as the built-in `base` recipe (21.1): the steps and their order move
//! there unchanged. Its caller is base_builds.rs, the hub's build of a base on a landing (16.4).

use apz_lsp::bridge;
use apz_lsp::{Language, ServerSpec};

use crate::project_stack::{Stack, Toolchain};

/// The guest image a base starts from: the OS template every node has (noded's workspace image).
pub const OS_TEMPLATE: &str = "workspace";
/// The harness binary in the guest (`hooks.rs` names it the same way). Its `lsp-serve` and `lsp-warm` commands run a
/// base's language servers behind their sockets, and warm them (PLAN 3.8).
pub const HARNESS: &str = "/usr/bin/agent-harness";
/// How long a base's warm-up waits for each server to index, in seconds: a server that has not indexed by then is
/// snapshotted as it is, and the fork's first query waits for it as it would on any cold server.
pub const WARM_WAIT_S: &str = "300";
/// The checkout's directory in the VM: the agent's `/workspace`.
pub const WORKSPACE: &str = "/workspace";
/// How long one command may run before its step fails (30 minutes).
pub const STEP_TIMEOUT_S: u64 = 1800;
/// The node's shared git objects, read-only (16.6): a volume whose root holds `objects/`, mounted at `/data/<name>`.
pub const GIT_OBJECTS_VOLUME: &str = "git-objects";
/// The node's per-node build cache (16.5), mounted at `/data/<name>` like every data volume.
pub const SCCACHE_VOLUME: &str = "sccache";
/// The object directory a checkout borrows from: the git objects volume, as the VM sees it.
pub const GIT_OBJECTS_DIR: &str = "/data/git-objects/objects";
/// The directory sccache keeps its cache in: the per-node cache volume, as the VM sees it.
pub const SCCACHE_DIR: &str = "/data/sccache";
/// The file that points the checkout's git at [`GIT_OBJECTS_DIR`]: the checkout's own objects stay in its disk.
pub const ALTERNATES_FILE: &str = "/workspace/.git/objects/info/alternates";

/// The kinds of base (16.1) this recipe builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseKind {
    /// The toolchains, the checkout, the dependencies, the build outputs, and the language servers, warm.
    Build,
    /// A build base with the project's services running and its test setup done.
    Test,
    /// A lane's preview (PLAN 18.1): the checkout and its build, then the app the project's preview target serves,
    /// started and listening before the base answers. It is never snapshotted: the preview's VM keeps running.
    Preview,
}

impl BaseKind {
    pub fn name(self) -> &'static str {
        match self {
            BaseKind::Build => "build",
            BaseKind::Test => "test",
            BaseKind::Preview => "preview",
        }
    }
}

/// One step of the base recipe, run in order in the base's VM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Starts the VM from the guest image `template`, with the node's shared `volumes` mounted read-only.
    Start {
        template: &'static str,
        volumes: Vec<&'static str>,
    },
    /// Runs a command and waits for it: it must exit 0 within its timeout.
    Run(Command),
    /// Starts a process and leaves it running (a language server, a service); the next step does not wait for it.
    Background(Command),
    /// Checks out `commit` of the Weft repository `repo` in [`WORKSPACE`]. The runner first initializes the repository
    /// and writes [`alternates_file`] for `alternates` (the objects directory it borrows from), then fetches, so the
    /// fetch finds the node's objects already there.
    Checkout {
        repo: String,
        commit: String,
        alternates: &'static str,
    },
    /// Stops the VM and snapshots it as the base of `kind`.
    Snapshot { kind: BaseKind },
}

/// A command in the VM: its argument vector (never run through a shell), the directory it runs in, how long it may
/// run, and the environment it adds to the VM's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub label: String,
    pub argv: Vec<String>,
    pub cwd: &'static str,
    pub timeout_s: u64,
    pub env: Vec<(&'static str, &'static str)>,
    /// The port a background command listens on, announced by the guest once it does (a preview's app).
    pub listen: Option<u16>,
}

impl Command {
    fn new(label: &str, argv: &[&str]) -> Command {
        Command {
            label: label.to_string(),
            argv: argv.iter().map(|part| part.to_string()).collect(),
            cwd: WORKSPACE,
            timeout_s: STEP_TIMEOUT_S,
            env: Vec::new(),
            listen: None,
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

/// The file a checkout's git reads its borrowed objects from, and its text: the objects directory, on one line.
pub fn alternates_file(objects: &str) -> (&'static str, String) {
    (ALTERNATES_FILE, format!("{objects}\n"))
}

/// The node volumes a base of `stack` mounts. Every base has the git objects; the per-node build cache is for Rust,
/// the only language whose compiler sccache wraps here.
pub fn volumes(stack: &Stack) -> Vec<&'static str> {
    let mut volumes = vec![GIT_OBJECTS_VOLUME];
    if stack.toolchains.contains_key(&Toolchain::Rust) {
        volumes.push(SCCACHE_VOLUME);
    }
    volumes
}

/// The environment every command of a base adds: sccache wraps rustc and keeps its cache on the node's volume. Empty
/// without Rust.
pub fn cache_env(stack: &Stack) -> Vec<(&'static str, &'static str)> {
    if stack.toolchains.contains_key(&Toolchain::Rust) {
        vec![("RUSTC_WRAPPER", "sccache"), ("SCCACHE_DIR", SCCACHE_DIR)]
    } else {
        Vec::new()
    }
}

/// Runs the base's steps: noded's commands in the VM, or a fake in tests.
pub trait VmRunner {
    fn run(&mut self, step: &Step) -> Result<(), String>;
}

/// The steps that build a base of `kind` from `stack`, checked out at `commit` of the Weft repository `repo`.
pub fn steps(stack: &Stack, kind: BaseKind, repo: &str, commit: &str) -> Vec<Step> {
    steps_of(stack, kind, repo, commit, true)
}

/// The steps of a test fork (PLAN 21.3): the test base's steps without its build and without its snapshot, so the
/// fork's checks (the CI recipe's lint, build, tests and audit) run their own commands in the VM, one by one.
pub fn fork_steps(stack: &Stack, repo: &str, commit: &str) -> Vec<Step> {
    let mut steps = steps_of(stack, BaseKind::Test, repo, commit, false);
    steps.retain(|step| !matches!(step, Step::Snapshot { .. }));
    steps
}

/// The steps of a lane's preview (PLAN 18.1): the preview base's steps at `commit` of `repo` (no snapshot), then the
/// app's `serve` command started in the background and a wait until it listens on `port`. A failed wait fails the start.
pub fn preview_steps(stack: &Stack, repo: &str, commit: &str, serve: &str, port: u16) -> Vec<Step> {
    let mut steps = steps_of(stack, BaseKind::Preview, repo, commit, true);
    steps.retain(|step| !matches!(step, Step::Snapshot { .. }));
    let mut app = Command::new("serve", &[]);
    app.argv = vec!["sh".into(), "-c".into(), serve.to_string()];
    app.env = cache_env(stack);
    app.listen = Some(port);
    steps.push(Step::Background(app));
    let mut wait = Command::new("wait for the app", &[]);
    wait.argv = vec![
        "bash".into(),
        "-c".into(),
        format!(
            "for i in $(seq 1 300); do (exec 3<>/dev/tcp/127.0.0.1/{port}) 2>/dev/null && exit 0; sleep 0.2; done; exit 1"
        ),
    ];
    wait.timeout_s = PREVIEW_WAIT_S;
    steps.push(Step::Run(wait));
    steps
}

/// How long a preview's app may take to listen (2 minutes).
pub const PREVIEW_WAIT_S: u64 = 120;

/// The build's commands: the declaration's, or each toolchain's default (what a base builds, and a CI build checks).
pub fn build_commands(stack: &Stack) -> Vec<Vec<String>> {
    commands(stack.build.build.as_ref(), stack, Toolchain::build)
}

fn steps_of(
    stack: &Stack,
    kind: BaseKind,
    repo: &str,
    commit: &str,
    with_build: bool,
) -> Vec<Step> {
    let env = cache_env(stack);
    // Every command of the base runs with the environment of the base's cache (16.5).
    let command = |label: &str, argv: &[&str]| {
        let mut command = Command::new(label, argv);
        command.env = env.clone();
        command
    };
    let mut steps = vec![Step::Start {
        template: OS_TEMPLATE,
        volumes: volumes(stack),
    }];
    for (toolchain, version) in &stack.toolchains {
        let label = format!("install {} {version}", toolchain.name());
        for argv in toolchain.install(version) {
            steps.push(Step::Run(command(&label, &argv_refs(&argv))));
        }
    }
    if stack.toolchains.contains_key(&Toolchain::Rust) {
        // Rust's build daemon (PLAN 3.8): sccache's server, started on a fresh VM before anything compiles, so it is
        // up when the base is snapshotted and every fork's compiles reach it.
        steps.push(Step::Run(command(
            "start sccache",
            &["sccache", "--start-server"],
        )));
    }
    steps.push(Step::Checkout {
        repo: repo.to_string(),
        commit: commit.to_string(),
        alternates: GIT_OBJECTS_DIR,
    });
    for argv in commands(stack.build.fetch.as_ref(), stack, Toolchain::fetch) {
        steps.push(Step::Run(command("fetch dependencies", &argv_refs(&argv))));
    }
    if with_build {
        for argv in build_commands(stack) {
            steps.push(Step::Run(command("build", &argv_refs(&argv))));
        }
    }
    // Each server runs behind its socket for the VM's life (PLAN 3.8), and is warmed before the snapshot: the warm-up
    // waits until the server has indexed, so a fork restored from the base finds it running and indexed.
    // A preview serves an app, not an editor: no language servers start in its VM.
    let servers = if kind == BaseKind::Preview {
        Vec::new()
    } else {
        servers(&stack.languages)
    };
    for spec in &servers {
        let label = format!("start {}", spec.id);
        steps.push(Step::Background(command(
            &label,
            &argv_refs(&harness_argv("lsp-serve", spec, &[])),
        )));
    }
    for spec in &servers {
        let label = format!("warm {}", spec.id);
        steps.push(Step::Run(command(
            &label,
            &argv_refs(&harness_argv("lsp-warm", spec, &["--wait", WARM_WAIT_S])),
        )));
    }
    if kind == BaseKind::Test {
        for service in &stack.services {
            let label = format!("start service {}", service.name);
            steps.push(Step::Background(command(
                &label,
                &argv_refs(&service.command),
            )));
        }
        for argv in &stack.test.setup {
            steps.push(Step::Run(command("test setup", &argv_refs(argv))));
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

/// The harness's command for a server (PLAN 3.8): `lsp-serve` runs the server behind its socket, `lsp-warm` warms it.
/// Both name the server by its id, and the socket it listens on in the VM's warm directory.
fn harness_argv(verb: &str, spec: &ServerSpec, rest: &[&str]) -> Vec<String> {
    let socket = bridge::guest_socket(spec.id);
    let mut argv = vec![
        HARNESS,
        verb,
        "--id",
        spec.id,
        "--socket",
        socket.as_str(),
        "--root",
        WORKSPACE,
    ];
    argv.extend_from_slice(rest);
    argv.into_iter().map(str::to_string).collect()
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

    /// The sample declares Rust, so its base mounts the git objects and the build cache.
    fn start() -> Step {
        Step::Start {
            template: OS_TEMPLATE,
            volumes: vec![GIT_OBJECTS_VOLUME, SCCACHE_VOLUME],
        }
    }

    /// A command of a Rust base: it runs with the cache's environment.
    fn cached(label: &str, argv: &[&str]) -> Command {
        let mut command = Command::new(label, argv);
        command.env = vec![
            ("RUSTC_WRAPPER", "sccache"),
            ("SCCACHE_DIR", "/data/sccache"),
        ];
        command
    }

    fn run(label: &str, argv: &[&str]) -> Step {
        Step::Run(cached(label, argv))
    }

    fn background(label: &str, argv: &[&str]) -> Step {
        Step::Background(cached(label, argv))
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
                run("start sccache", &["sccache", "--start-server"]),
                Step::Checkout {
                    repo: "repo-1".into(),
                    commit: "abc123".into(),
                    alternates: GIT_OBJECTS_DIR,
                },
                run("fetch dependencies", &["cargo", "fetch"]),
                run("fetch dependencies", &["npm", "ci"]),
                run("build", &["cargo", "build", "--all-targets"]),
                run("build", &["npm", "run", "build", "--if-present"]),
                background_owned("start rust-analyzer", &serve("rust-analyzer")),
                background_owned(
                    "start typescript-language-server",
                    &serve("typescript-language-server"),
                ),
                run_owned("warm rust-analyzer", &warm("rust-analyzer")),
                run_owned(
                    "warm typescript-language-server",
                    &warm("typescript-language-server"),
                ),
                Step::Snapshot {
                    kind: BaseKind::Build,
                },
            ]
        );
    }

    /// The harness command that runs server `id` behind its socket in the VM.
    fn serve(id: &str) -> Vec<String> {
        lsp("lsp-serve", id, &[])
    }

    /// The harness command that warms server `id`, waiting the recipe's time for it to index.
    fn warm(id: &str) -> Vec<String> {
        lsp("lsp-warm", id, &["--wait", "300"])
    }

    fn lsp(verb: &str, id: &str, rest: &[&str]) -> Vec<String> {
        let socket = format!("/workspace/.plaza/lsp/{id}.sock");
        let mut argv = vec![
            "/usr/bin/agent-harness",
            verb,
            "--id",
            id,
            "--socket",
            socket.as_str(),
            "--root",
            "/workspace",
        ];
        argv.extend_from_slice(rest);
        argv.into_iter().map(str::to_string).collect()
    }

    fn refs(argv: &[String]) -> Vec<&str> {
        argv.iter().map(String::as_str).collect()
    }

    fn run_owned(label: &str, argv: &[String]) -> Step {
        run(label, &refs(argv))
    }

    fn background_owned(label: &str, argv: &[String]) -> Step {
        background(label, &refs(argv))
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
            .filter(|s| matches!(s, Step::Run(c) if c.label == "fetch dependencies" || c.label == "build"))
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
        assert_eq!(
            runs,
            vec![
                "install python 3.13",
                "fetch dependencies",
                "warm pyright",
                "warm typescript-language-server",
            ]
        );
        let servers: Vec<Step> = steps
            .iter()
            .filter(|s| matches!(s, Step::Background(_)))
            .cloned()
            .collect();
        assert_eq!(
            servers,
            vec![
                Step::Background(Command::new("start pyright", &refs(&serve("pyright")))),
                Step::Background(Command::new(
                    "start typescript-language-server",
                    &refs(&serve("typescript-language-server")),
                )),
            ]
        );
        // No Rust, so no build daemon to start.
        assert!(!steps.iter().any(|s| s.label().contains("sccache")));
    }

    #[test]
    fn a_rust_base_mounts_the_git_objects_and_the_build_cache_and_other_bases_only_the_objects() {
        let rust = parse(SAMPLE).unwrap();
        assert_eq!(volumes(&rust), vec!["git-objects", "sccache"]);
        let python = parse("languages = []\n[toolchains]\npython = \"3.13\"\n").unwrap();
        assert_eq!(volumes(&python), vec!["git-objects"]);
        let started = steps(&python, BaseKind::Build, "r", "c");
        assert_eq!(
            started[0],
            Step::Start {
                template: OS_TEMPLATE,
                volumes: vec!["git-objects"],
            }
        );
    }

    #[test]
    fn every_command_of_a_rust_base_runs_with_sccache_on_the_node_cache_and_no_other_base_does() {
        let rust = parse(SAMPLE).unwrap();
        let test_base = steps(&rust, BaseKind::Test, "r", "c");
        let commands: Vec<&Command> = test_base
            .iter()
            .filter_map(|step| match step {
                Step::Run(command) | Step::Background(command) => Some(command),
                _ => None,
            })
            .collect();
        assert!(commands.len() > 5);
        for command in commands {
            assert_eq!(
                command.env,
                vec![
                    ("RUSTC_WRAPPER", "sccache"),
                    ("SCCACHE_DIR", "/data/sccache")
                ],
                "{}",
                command.label
            );
        }
        let python = parse("languages = []\n[toolchains]\npython = \"3.13\"\n").unwrap();
        for step in steps(&python, BaseKind::Build, "r", "c") {
            if let Step::Run(command) | Step::Background(command) = step {
                assert!(command.env.is_empty(), "{}", command.label);
            }
        }
    }

    #[test]
    fn a_checkout_borrows_the_node_objects_through_the_alternates_file_it_is_given() {
        let stack = parse(SAMPLE).unwrap();
        let steps = steps(&stack, BaseKind::Build, "repo-1", "abc123");
        let checkout = steps
            .iter()
            .position(|step| matches!(step, Step::Checkout { .. }))
            .unwrap();
        assert_eq!(
            steps[checkout],
            Step::Checkout {
                repo: "repo-1".into(),
                commit: "abc123".into(),
                alternates: GIT_OBJECTS_DIR,
            }
        );
        assert_eq!(
            checkout, 6,
            "the checkout follows the toolchains (Start, rust's two, node's two) and sccache's start"
        );
        // The file a fork's checkout gets: git reads the node's objects from the volume, the fork keeps its own.
        assert_eq!(
            alternates_file(GIT_OBJECTS_DIR),
            (
                "/workspace/.git/objects/info/alternates",
                "/data/git-objects/objects\n".to_string()
            )
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
