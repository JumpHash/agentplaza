//! The production release builder (PLAN 20.5, 22.2): each target of a release goes to the pool 7.5 places it on, is built
//! in a fork of the project's base on that node, and the file the build leaves comes back as the artifact's bytes.
//!
//! A target no pool in service can build is not run, with the reason its placement gives (`Outcome::NotRun`): the
//! release is cut without it. The fork starts from the workspace image and runs the base's steps (`base_recipe::
//! fork_steps`): noded restores no snapshot here, so the fork repeats the base's toolchains and checkout instead of
//! resuming the base's VM. The file comes back base64-encoded in the command's answer.

use base64::Engine;

use super::flow::{Builder, Built, Outcome};
use crate::base_recipe::{self, Command, STEP_TIMEOUT_S, WORKSPACE};
use crate::base_vm::{Fork, Host, Live};
use crate::project_stack::{self, FILE, Stack};
use crate::state::App;
use crate::targets::{self, Placement};

use super::entity::Release;

/// The production builder: the fleet's pools, placed through the app (`targets::build_plan`).
pub struct Pools<'a> {
    pub app: &'a App,
}

impl Builder for Pools<'_> {
    fn build(&self, release: &Release, target: &str) -> Result<Outcome, String> {
        let Some(stack) = project_stack::read(self.app, &release.project)? else {
            return Ok(Outcome::NotRun(format!(
                "{} declares no build target: {FILE} is not on main",
                release.project
            )));
        };
        let placements = targets::build_plan(self.app, &release.project)?;
        build_target(
            &Live(self.app),
            &release.project,
            &release.repo,
            &release.from,
            &stack,
            &placements,
            target,
        )
    }
}

/// Builds one release target from its placements (pure over the host: the tests give a fake noded). A release target is
/// named `<kind>:<name>@<platform>`; its platform picks the declared target (`platform`, 20.5) and that target's
/// placement. No declared target, or no placement for it, is not run with the reason.
pub fn build_target<H: Host>(
    host: &H,
    project: &str,
    repo: &str,
    commit: &str,
    stack: &Stack,
    placements: &[Placement],
    target: &str,
) -> Result<Outcome, String> {
    let platform_name = target.rsplit_once('@').map_or(target, |(_, p)| p);
    // Weft names the platforms its build files declare (`macos-aarch64`); the stack names them as the plan does
    // (`macos-arm64`).
    let stack_name = platform_name.replace("aarch64", "arm64");
    let Some(declared) = stack
        .targets
        .iter()
        .find(|t| t.platform.is_some_and(|p| p.name() == stack_name))
    else {
        return Ok(Outcome::NotRun(format!(
            "{FILE} declares no target for {platform_name}"
        )));
    };
    let Some(platform) = declared.platform else {
        return Ok(Outcome::NotRun(format!(
            "{FILE} declares no target for {platform_name}"
        )));
    };
    match placements.iter().find(|p| p.platform() == platform) {
        None => Ok(Outcome::NotRun(format!(
            "no placement was made for {platform_name}"
        ))),
        Some(Placement::NotRun { reason, .. }) => Ok(Outcome::NotRun(reason.clone())),
        Some(Placement::Built { node, pool, .. }) => {
            let pool = pool.as_ref().map_or_else(
                || "any".to_string(),
                |p| format!("{}-{}", p.arch, p.hypervisor),
            );
            let (Some(command), Some(output)) = (&declared.command, &declared.output) else {
                return Err(format!("{FILE} declares no command for {platform_name}"));
            };
            let job = Job {
                project,
                repo,
                commit,
                target,
                stack,
            };
            let built = build_on(host, &job, node, pool, command, output)?;
            Ok(Outcome::Built(built))
        }
    }
}

/// What a placed target is built from: its project, its Weft repository and commit, the release target's name, and the
/// stack the base is built from.
struct Job<'a> {
    project: &'a str,
    repo: &'a str,
    commit: &'a str,
    target: &'a str,
    stack: &'a Stack,
}

/// Builds on `node` in a fork: the command runs in the workspace, and the file it leaves comes back as the bytes.
fn build_on<H: Host>(
    host: &H,
    job: &Job,
    node: &str,
    pool: String,
    command: &[String],
    output: &str,
) -> Result<Built, String> {
    let steps = base_recipe::fork_steps(job.stack, job.repo, job.commit);
    let mut fork = Fork::start(host, node.to_string(), job.project.to_string(), &steps)?;
    let bytes = build_in(&mut fork, job, command, output);
    let retired = fork.retire();
    let bytes = bytes?;
    retired?;
    Ok(Built {
        bytes,
        pool,
        base: format!("{}/fork", job.project),
        builder: format!("node {node}"),
        command: command.join(" "),
    })
}

/// Runs the build command in the fork and reads back the file it leaves.
fn build_in<H: Host>(
    fork: &mut Fork<'_, H>,
    job: &Job,
    command: &[String],
    output: &str,
) -> Result<Vec<u8>, String> {
    let build = Command {
        label: format!("build {}", job.target),
        argv: command.to_vec(),
        cwd: WORKSPACE,
        timeout_s: STEP_TIMEOUT_S,
        env: base_recipe::cache_env(job.stack),
    };
    let answer = fork.exec(&build)?;
    if !answer.ok {
        return Err(format!(
            "the build of {} failed ({}): {}",
            job.target,
            exit_of(answer.exit_code),
            answer.output.trim_end()
        ));
    }
    let read = Command {
        label: format!("read {output}"),
        argv: vec![
            "base64".to_string(),
            "-w0".to_string(),
            format!("{WORKSPACE}/{output}"),
        ],
        cwd: WORKSPACE,
        timeout_s: STEP_TIMEOUT_S,
        env: Vec::new(),
    };
    let answer = fork.exec(&read)?;
    if !answer.ok {
        return Err(format!(
            "the build of {} left no {output}: {}",
            job.target,
            answer.output.trim_end()
        ));
    }
    base64::engine::general_purpose::STANDARD
        .decode(answer.output.trim())
        .map_err(|e| format!("{output} did not come back as base64: {e}"))
}

fn exit_of(code: Option<i32>) -> String {
    match code {
        Some(code) => format!("exit {code}"),
        None => "no exit status".to_string(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use apz_proto::wire::HostMessage;
    use serde_json::{Value, json};

    use super::*;
    use crate::base_vm::{self, Answer};
    use crate::nodes::Pool;
    use crate::placement::Candidate;
    use crate::project_stack::parse;

    /// A fake noded and VM, as the base runner's tests use one: the run starts with the VM saying hello, and each exec
    /// answers at once (`base64` answers with the file's bytes; `cargo` answers `build_ok`).
    pub(crate) struct FakeHost {
        pub(crate) cid: u64,
        pub(crate) file: Vec<u8>,
        pub(crate) build_ok: bool,
        sent: Mutex<Vec<HostMessage>>,
    }

    impl FakeHost {
        pub(crate) fn new(cid: u64, file: &[u8]) -> FakeHost {
            FakeHost {
                cid,
                file: file.to_vec(),
                build_ok: true,
                sent: Mutex::new(Vec::new()),
            }
        }

        pub(crate) fn argvs(&self) -> Vec<Vec<String>> {
            self.sent
                .lock()
                .unwrap()
                .iter()
                .filter_map(|m| match m {
                    HostMessage::Exec { argv, .. } => Some(argv.clone()),
                    _ => None,
                })
                .collect()
        }
    }

    impl Host for FakeHost {
        fn noded(
            &self,
            _node: &str,
            method: &str,
            path: &str,
            _body: Option<&Value>,
        ) -> Result<(u16, Value), String> {
            match (method, path) {
                ("POST", "/v1/runs") => {
                    base_vm::arrive(self.cid);
                    Ok((
                        201,
                        json!({"id": "r-build", "vms": [{"guest_cid": self.cid}]}),
                    ))
                }
                ("POST", p) if p.ends_with("/vms/0/stop") => Ok((200, json!({}))),
                other => Err(format!("unexpected noded call {other:?}")),
            }
        }

        fn send(&self, cid: u64, message: &HostMessage) -> Result<(), String> {
            if cid != self.cid {
                return Err(format!("no VM at {cid}"));
            }
            self.sent.lock().unwrap().push(message.clone());
            if let HostMessage::Exec { id, argv, .. } = message {
                let answer = match argv[0].as_str() {
                    "base64" => Answer {
                        ok: true,
                        exit_code: Some(0),
                        output: base64::engine::general_purpose::STANDARD.encode(&self.file),
                    },
                    "cargo" if !self.build_ok => Answer {
                        ok: false,
                        exit_code: Some(101),
                        output: "error[E0432]: unresolved import\n".into(),
                    },
                    _ => Answer {
                        ok: true,
                        exit_code: Some(0),
                        output: String::new(),
                    },
                };
                base_vm::answer(cid, *id, answer);
            }
            Ok(())
        }
    }

    pub(crate) const STACK: &str = "[[targets]]\nname = \"linux\"\nplatform = \"linux-x86_64\"\ncommand = [\"cargo\", \"build\", \"--release\"]\noutput = \"target/release/app\"\n\n[[targets]]\nname = \"mac\"\nplatform = \"macos-arm64\"\ncommand = [\"cargo\", \"build\", \"--release\"]\noutput = \"target/release/app\"\n";

    pub(crate) fn linux_node(cid_node: &str) -> Candidate {
        Candidate {
            node: cid_node.into(),
            free_mib: 8000,
            running_vms: 0,
            warm: false,
            pool: Some(Pool {
                arch: "x86_64".into(),
                hypervisor: "kvm".into(),
                cpu_baseline: "x86_64 kvm baseline".into(),
            }),
            base: true,
            disk_free_gib: None,
            idle_cpus: None,
        }
    }

    #[test]
    fn a_placed_target_is_built_in_a_fork_and_the_file_it_leaves_comes_back_as_its_bytes() {
        let host = FakeHost::new(9301, b"\x7fELF fake app");
        let stack = parse(STACK).unwrap();
        let placements = targets::place(&[targets::Platform::LinuxX86_64], &[linux_node("n-lin")]);
        let outcome = build_target(
            &host,
            "app",
            "repo-app",
            "c1",
            &stack,
            &placements,
            "cargo:app@linux-x86_64",
        )
        .unwrap();
        let Outcome::Built(built) = outcome else {
            panic!("the target is placed and built");
        };
        assert_eq!(built.bytes, b"\x7fELF fake app");
        assert_eq!(built.pool, "x86_64-kvm");
        assert_eq!(built.builder, "node n-lin");
        assert_eq!(built.command, "cargo build --release");
        let argvs = host.argvs();
        assert_eq!(
            argvs[argvs.len() - 2],
            ["cargo", "build", "--release"],
            "{argvs:?}"
        );
        assert_eq!(
            argvs.last().unwrap(),
            &["base64", "-w0", "/workspace/target/release/app"]
        );
    }

    #[test]
    fn a_target_no_pool_can_build_is_not_run_with_the_reason_its_placement_gives() {
        let host = FakeHost::new(9302, b"unused");
        let stack = parse(STACK).unwrap();
        // Only a Linux pool is in service: the Mac target has none.
        let placements = targets::place(&[targets::Platform::Macos], &[linux_node("n-lin")]);
        let outcome = build_target(
            &host,
            "app",
            "repo-app",
            "c1",
            &stack,
            &placements,
            "cargo:app@macos-arm64",
        )
        .unwrap();
        match outcome {
            Outcome::NotRun(reason) => assert!(reason.contains("a Mac pool"), "{reason}"),
            Outcome::Built(_) => panic!("no pool builds a Mac target here"),
        }
        assert!(host.argvs().is_empty(), "nothing ran");
    }

    #[test]
    fn a_target_the_declaration_does_not_name_is_not_run_and_a_failed_build_is_an_error() {
        let stack = parse(STACK).unwrap();
        let placements = targets::place(&[targets::Platform::Windows], &[]);
        let host = FakeHost::new(9303, b"unused");
        match build_target(
            &host,
            "app",
            "r",
            "c1",
            &stack,
            &placements,
            "cargo:app@windows-x86_64",
        )
        .unwrap()
        {
            Outcome::NotRun(reason) => assert!(reason.contains("no target for windows-x86_64")),
            Outcome::Built(_) => panic!("the declaration names no Windows target"),
        }

        let mut failing = FakeHost::new(9304, b"unused");
        failing.build_ok = false;
        let placements = targets::place(&[targets::Platform::LinuxX86_64], &[linux_node("n-lin")]);
        let err = match build_target(
            &failing,
            "app",
            "r",
            "c1",
            &stack,
            &placements,
            "cargo:app@linux-x86_64",
        ) {
            Err(e) => e,
            Ok(_) => panic!("a failed build is an error"),
        };
        assert!(err.contains("failed") && err.contains("E0432"), "{err}");
    }
}
