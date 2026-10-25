//! The production release builder (PLAN 20.5, 22.2): each target of a release goes to the pool 7.5 places it on, is built
//! in a fork of the project's base on that node, and the file the build leaves comes back as the artifact's bytes.
//!
//! A target no pool in service can build is not run, with the reason its placement gives (`Outcome::NotRun`): the
//! release is cut without it. The fork starts from the workspace image and runs the base's steps (`base_recipe::
//! fork_steps`): noded restores no snapshot here, so the fork repeats the base's toolchains and checkout instead of
//! resuming the base's VM. The file comes back base64-encoded in chunks, each one an answer under the cap, and is checked
//! against its size and sha256.
//!
//! A macOS or iOS target is not run, even on a Mac node: that node's fork is the Linux workspace guest, and the release
//! builder drives no native Mac runner yet (`Outcome::NotRun`).

use apz_proto::wire::MAX_TOOL_OUTPUT;
use base64::Engine;
use sha2::{Digest, Sha256};

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

/// A stand-in for the build farm, for the booted-hub tests: when this variable names a file, each target of a release is built
/// as that file's bytes on no pool. A hub that builds for real never has it set.
pub const STAND_IN: &str = "APZ_RELEASE_BUILD_STAND_IN";

impl Builder for Pools<'_> {
    fn build(&self, release: &Release, target: &str) -> Result<Outcome, String> {
        if let Some(path) = std::env::var_os(STAND_IN) {
            let bytes = std::fs::read(&path).map_err(|e| format!("{STAND_IN}: {e}"))?;
            return Ok(Outcome::Built(Built {
                bytes,
                pool: "stand-in".into(),
                base: "stand-in".into(),
                builder: "stand-in".into(),
                command: "stand-in".into(),
            }));
        }
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
/// named `<kind>:<name>@<platform>`: the declared target (`[[targets]]`, 20.5) of that name and platform is the one
/// built. Weft names a release's targets after the build files' binaries (`cargo:app@linux-x86_64`), which need not
/// be a declared name: such a target takes the platform's only declaration, and a platform with several, none of that
/// name, is an error rather than a guess. A declared target no placement covers is not run, with the reason.
pub fn build_target<H: Host>(
    host: &H,
    project: &str,
    repo: &str,
    commit: &str,
    stack: &Stack,
    placements: &[Placement],
    target: &str,
) -> Result<Outcome, String> {
    let (named, platform_name) = target.rsplit_once('@').unwrap_or(("", target));
    // The name comes after the kind (`cargo:server`), when the target names one.
    let name = named.rsplit_once(':').map_or(named, |(_, n)| n);
    // Weft names the platforms its build files declare (`macos-aarch64`); the stack names them as the plan does
    // (`macos-arm64`).
    let stack_name = platform_name.replace("aarch64", "arm64");
    let on_platform: Vec<_> = stack
        .targets
        .iter()
        .filter(|t| t.platform.is_some_and(|p| p.name() == stack_name))
        .collect();
    let declared = match on_platform.iter().find(|t| t.name == name) {
        Some(named) => *named,
        None => match on_platform.as_slice() {
            [only] => *only,
            [] => {
                return Err(format!(
                    "{FILE} declares no target {name} for {platform_name}"
                ));
            }
            several => {
                let names: Vec<&str> = several.iter().map(|t| t.name.as_str()).collect();
                return Err(format!(
                    "{FILE} declares no target {name} for {platform_name}, and {} others ({}): name one",
                    names.len(),
                    names.join(", ")
                ));
            }
        },
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
            // A Mac node's fork is still the Linux workspace guest: an Apple target is not built in it, since Xcode and
            // signing need macOS.
            if matches!(platform, targets::Platform::Macos | targets::Platform::Ios) {
                return Ok(Outcome::NotRun(format!(
                    "{platform_name} is native Apple work: it needs a Mac runner the release builder does not drive yet"
                )));
            }
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

/// The bytes one read of the artifact carries. Base64'd, a chunk is 4/3 of this, which fits an answer under the cap
/// (`MAX_TOOL_OUTPUT`) with room to spare.
const CHUNK: u64 = 32 << 10;
const _: () = assert!(CHUNK * 4 / 3 + 1024 < MAX_TOOL_OUTPUT as u64);
/// The marker `cap_output` leaves in an answer it cut (`[... N bytes cut ...]`).
const CUT: &str = "bytes cut ...]";

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
        listen: None,
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
    let path = format!("{WORKSPACE}/{output}");
    let size = ask(
        fork,
        job,
        format!("size {output}"),
        vec!["stat".into(), "-c".into(), "%s".into(), path.clone()],
        output,
    )?;
    let size: u64 = size
        .trim()
        .parse()
        .map_err(|e| format!("{output} has no size: {e}"))?;
    let sum = ask(
        fork,
        job,
        format!("checksum {output}"),
        vec!["sha256sum".into(), path.clone()],
        output,
    )?;
    let sum = sum
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    // Each read is one chunk: `dd` seeks to it, and the pipe base64s it (dd's byte counts go to the dropped stderr).
    let chunk = format!("dd if=\"$1\" bs={CHUNK} skip=\"$2\" count=1 2>/dev/null | base64 -w0");
    let mut bytes = Vec::new();
    for index in 0..size.div_ceil(CHUNK) {
        let argv = vec![
            "sh".into(),
            "-c".into(),
            chunk.clone(),
            "sh".into(),
            path.clone(),
            index.to_string(),
        ];
        let text = ask(fork, job, format!("read {output} at {index}"), argv, output)?;
        let part = base64::engine::general_purpose::STANDARD
            .decode(text.trim())
            .map_err(|e| format!("{output} chunk {index} did not come back as base64: {e}"))?;
        let want = (size - index * CHUNK).min(CHUNK);
        if part.len() as u64 != want {
            return Err(format!(
                "{output} chunk {index} came back {} bytes, not {want}",
                part.len()
            ));
        }
        bytes.extend(part);
    }
    if bytes.len() as u64 != size {
        return Err(format!(
            "{output} came back {} bytes, not {size}",
            bytes.len()
        ));
    }
    let digest = format!("{:x}", Sha256::digest(&bytes));
    if digest != sum {
        return Err(format!(
            "{output} is not the file the build checked: {digest}, not {sum}"
        ));
    }
    Ok(bytes)
}

/// Runs `argv` in the fork and returns the text of its answer. A command that fails, or an answer the harness cut at its
/// cap (so the file read is not whole), is an error.
fn ask<H: Host>(
    fork: &mut Fork<'_, H>,
    job: &Job,
    label: String,
    argv: Vec<String>,
    output: &str,
) -> Result<String, String> {
    let command = Command {
        label,
        argv,
        cwd: WORKSPACE,
        timeout_s: STEP_TIMEOUT_S,
        env: Vec::new(),
        listen: None,
    };
    let answer = fork.exec(&command)?;
    if !answer.ok {
        return Err(format!(
            "the build of {} left no {output}: {}",
            job.target,
            answer.output.trim_end()
        ));
    }
    if answer.output.contains(CUT) {
        return Err(format!("the read of {output} was cut at the answer's cap"));
    }
    Ok(answer.output)
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

    use apz_proto::wire::{HostMessage, MAX_TOOL_OUTPUT, cap_output};
    use serde_json::{Value, json};

    use super::*;
    use crate::base_vm::{self, Answer};
    use crate::nodes::Pool;
    use crate::placement::Candidate;
    use crate::project_stack::parse;

    /// A fake noded and VM, as the base runner's tests use one: the run starts with the VM saying hello, and each exec
    /// answers at once (`stat`, `sha256sum` and the `sh` chunk reads answer the file's size, checksum and bytes; `cargo`
    /// answers `build_ok`). An answer past the cap is cut, as the harness cuts it.
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
                    "stat" => Answer {
                        ok: true,
                        exit_code: Some(0),
                        output: format!("{}\n", self.file.len()),
                    },
                    "sha256sum" => Answer {
                        ok: true,
                        exit_code: Some(0),
                        output: format!("{:x}  {}\n", Sha256::digest(&self.file), argv[1]),
                    },
                    // The chunk `dd` seeks to: its index is the last argument (`build_in`'s chunk reads). The fork's own
                    // setup runs other `sh -c` lines, which fall through to the default answer.
                    "sh" if argv
                        .get(2)
                        .is_some_and(|script| script.starts_with("dd if=")) =>
                    {
                        let start = argv[5].parse::<usize>().unwrap() * CHUNK as usize;
                        let end = (start + CHUNK as usize).min(self.file.len());
                        Answer {
                            ok: true,
                            exit_code: Some(0),
                            output: base64::engine::general_purpose::STANDARD
                                .encode(&self.file[start..end]),
                        }
                    }
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
                // The harness cuts an answer past the cap, and so does the fake: a whole file is never one answer.
                let answer = Answer {
                    output: cap_output(&answer.output, MAX_TOOL_OUTPUT),
                    ..answer
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
            "cargo:linux@linux-x86_64",
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
        assert!(
            argvs
                .iter()
                .any(|argv| argv == &["cargo", "build", "--release"]),
            "{argvs:?}"
        );
        assert!(
            argvs
                .iter()
                .any(|argv| argv == &["sha256sum", "/workspace/target/release/app"]),
            "{argvs:?}"
        );
        assert_eq!(argvs.last().unwrap()[4], "/workspace/target/release/app");
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
            "cargo:mac@macos-arm64",
        )
        .unwrap();
        match outcome {
            Outcome::NotRun(reason) => assert!(reason.contains("a Mac pool"), "{reason}"),
            Outcome::Built(_) => panic!("no pool builds a Mac target here"),
        }
        assert!(host.argvs().is_empty(), "nothing ran");
    }

    #[test]
    fn a_macos_or_ios_target_is_not_run_in_the_linux_fork_even_on_a_mac_node() {
        // The Mac node is placed for both targets, but its fork is the Linux workspace guest: no command reaches it.
        let stack = parse(&format!(
            "{STACK}[[targets]]\nname = \"app\"\nplatform = \"ios-arm64\"\ncommand = [\"xcodebuild\"]\noutput = \"build/app\"\n"
        ))
        .unwrap();
        let mac = Candidate {
            node: "n-mac".into(),
            free_mib: 8000,
            running_vms: 0,
            warm: false,
            pool: Some(Pool {
                arch: "aarch64".into(),
                hypervisor: "hvf".into(),
                cpu_baseline: "arm64 cpufamily=0x67f674b3".into(),
            }),
            base: true,
            disk_free_gib: None,
            idle_cpus: None,
        };
        let placements =
            targets::place(&[targets::Platform::Macos, targets::Platform::Ios], &[mac]);
        assert!(
            placements
                .iter()
                .all(|p| matches!(p, Placement::Built { .. })),
            "the Mac node is placed for both: {placements:?}"
        );
        let host = FakeHost::new(9308, b"unused");
        for target in ["cargo:mac@macos-arm64", "xcodebuild:app@ios-arm64"] {
            let outcome =
                build_target(&host, "app", "repo-app", "c1", &stack, &placements, target).unwrap();
            match outcome {
                Outcome::NotRun(reason) => assert!(
                    reason.contains("native Apple work") && reason.contains("Mac runner"),
                    "{target}: {reason}"
                ),
                Outcome::Built(_) => panic!("{target} is not built in a Linux fork"),
            }
        }
        assert!(
            host.argvs().is_empty(),
            "no command reached the Linux fork: {:?}",
            host.argvs()
        );
    }

    #[test]
    fn a_name_the_declaration_gives_for_no_such_platform_is_an_error_and_a_failed_build_is_an_error()
     {
        let stack = parse(STACK).unwrap();
        let placements = targets::place(&[targets::Platform::Windows], &[]);
        let host = FakeHost::new(9303, b"unused");
        // `linux` is declared, for Linux: asked for Windows, it names no declared target.
        let err = match build_target(
            &host,
            "app",
            "r",
            "c1",
            &stack,
            &placements,
            "cargo:linux@windows-x86_64",
        ) {
            Err(e) => e,
            Ok(_) => panic!("a name the declaration gives for another platform is an error"),
        };
        assert!(err.contains("no target linux for windows-x86_64"), "{err}");
        assert!(host.argvs().is_empty(), "nothing ran");

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
            "cargo:linux@linux-x86_64",
        ) {
            Err(e) => e,
            Ok(_) => panic!("a failed build is an error"),
        };
        assert!(err.contains("failed") && err.contains("E0432"), "{err}");
    }

    /// Two named targets on one platform, each with its own command and output.
    const TWO_ON_LINUX: &str = "[[targets]]\nname = \"server\"\nplatform = \"linux-x86_64\"\ncommand = [\"cargo\", \"build\", \"--release\", \"--bin\", \"server\"]\noutput = \"target/release/server\"\n\n[[targets]]\nname = \"cli\"\nplatform = \"linux-x86_64\"\ncommand = [\"cargo\", \"build\", \"--release\", \"--bin\", \"cli\"]\noutput = \"target/release/cli\"\n";

    #[test]
    fn two_named_targets_on_one_platform_build_their_own_declarations() {
        let stack = parse(TWO_ON_LINUX).unwrap();
        let placements = targets::place(&[targets::Platform::LinuxX86_64], &[linux_node("n-lin")]);
        for (cid, target, bytes, command, path) in [
            (
                9305,
                "cargo:server@linux-x86_64",
                &b"server"[..],
                "cargo build --release --bin server",
                "/workspace/target/release/server",
            ),
            (
                9306,
                "cargo:cli@linux-x86_64",
                &b"cli"[..],
                "cargo build --release --bin cli",
                "/workspace/target/release/cli",
            ),
        ] {
            let host = FakeHost::new(cid, bytes);
            let outcome =
                build_target(&host, "app", "repo-app", "c1", &stack, &placements, target).unwrap();
            let Outcome::Built(built) = outcome else {
                panic!("{target} is placed and built");
            };
            assert_eq!(built.bytes, bytes);
            assert_eq!(built.command, command);
            assert!(
                host.argvs()
                    .iter()
                    .any(|argv| argv.last().is_some_and(|last| last == path)),
                "{path}: {:?}",
                host.argvs()
            );
        }
    }

    #[test]
    fn a_binary_name_takes_the_platforms_only_declaration_and_is_an_error_among_several() {
        let placements = targets::place(&[targets::Platform::LinuxX86_64], &[linux_node("n-lin")]);
        // Weft names a release's targets after the binaries (`app`): one declaration for the platform is the one built.
        let host = FakeHost::new(9308, b"app");
        let outcome = build_target(
            &host,
            "app",
            "repo-app",
            "c1",
            &parse(STACK).unwrap(),
            &placements,
            "cargo:app@linux-x86_64",
        )
        .unwrap();
        assert!(
            matches!(outcome, Outcome::Built(_)),
            "the only Linux declaration is built"
        );
        // Among several, a name none of them has is not guessed.
        let host = FakeHost::new(9309, b"unused");
        let err = match build_target(
            &host,
            "app",
            "repo-app",
            "c1",
            &parse(TWO_ON_LINUX).unwrap(),
            &placements,
            "cargo:app@linux-x86_64",
        ) {
            Err(e) => e,
            Ok(_) => panic!("an undeclared name among several declarations is an error"),
        };
        assert!(err.contains("no target app for linux-x86_64"), "{err}");
        assert!(err.contains("server") && err.contains("cli"), "{err}");
        assert!(host.argvs().is_empty(), "nothing ran");
    }

    #[test]
    fn an_artifact_past_the_answer_cap_arrives_byte_identical() {
        // 150,000 bytes: more than one answer carries, so the file comes back in 32 KiB chunks (five of them).
        let file: Vec<u8> = (0..150_000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
            .collect();
        let host = FakeHost::new(9307, &file);
        let stack = parse(STACK).unwrap();
        let placements = targets::place(&[targets::Platform::LinuxX86_64], &[linux_node("n-lin")]);
        let outcome = build_target(
            &host,
            "app",
            "repo-app",
            "c1",
            &stack,
            &placements,
            "cargo:linux@linux-x86_64",
        )
        .unwrap();
        let Outcome::Built(built) = outcome else {
            panic!("the target is placed and built");
        };
        assert_eq!(built.bytes, file);
        let chunks = host
            .argvs()
            .iter()
            .filter(|argv| argv[0] == "sh" && argv[2].starts_with("dd if="))
            .count();
        assert_eq!(chunks, 5, "150,000 bytes in 32 KiB chunks");
    }
}
