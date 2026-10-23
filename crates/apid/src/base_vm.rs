//! The base runner (PLAN 16.4): the production [`VmRunner`] of a base's steps. noded starts the base's VM as a session of
//! kind `base`, with no model. Its harness says hello, and each step goes to it as an exec item (`HostMessage::Exec`), whose
//! answer (`GuestMessage::ExecResult`) reaches the hub on the guest's message route. The snapshot is noded's checkpoint of
//! the VM, and the base it makes is held on that node: placement prefers the node that holds a project's base (3.5).
//!
//! The VM is reached through a [`Host`]: noded's calls and the agent gateway's send. [`Live`] is the hub's; the tests
//! give a fake one. The sessions that said hello, and the answers the steps wait for, are in memory, as base_builds'
//! state is: a restart forgets them, and the next landing builds again.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use apz_proto::wire::{HostMessage, KIND_BASE};
use serde_json::{Value, json};

use crate::base_recipe::{
    self, BaseKind, Command, OS_TEMPLATE, STEP_TIMEOUT_S, Step, VmRunner, WORKSPACE,
};
use crate::fleet;
use crate::state::App;

/// The VM a base is built in: two vCPUs and 4 GiB, a build's shape.
const VCPUS: u32 = 2;
const MEMORY_MIB: u32 = 4096;
/// How long a base's VM may take to say hello: a cold boot of the guest, with its volumes.
const BOOT_WAIT: Duration = Duration::from_secs(300);
/// How long the hub waits for a foreground command's answer past its own timeout (the harness kills the command at it).
const ANSWER_GRACE: Duration = Duration::from_secs(60);
/// How long a background command may take to answer that it has started.
const START_WAIT: Duration = Duration::from_secs(60);
/// How often the hub checks whether a VM has said hello.
const HELLO_POLL: Duration = Duration::from_millis(100);

/// A command's answer, as the VM sent it (`GuestMessage::ExecResult`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub output: String,
}

/// The sessions that said hello, the answers the steps wait for, and the bases each node holds.
#[derive(Default)]
struct State {
    /// The VMs whose base session said hello.
    up: BTreeSet<u64>,
    /// The step waiting for each (VM, exec id)'s answer.
    waiting: BTreeMap<(u64, u64), Sender<Answer>>,
    /// The base keys each node holds, from the builds that snapshotted there.
    held: BTreeMap<String, BTreeSet<String>>,
    /// The (VM, port) pairs whose guest announced that the port listens, and noded registered it (6.4).
    listening: BTreeSet<(u64, u16)>,
}

static STATE: Mutex<State> = Mutex::new(State {
    up: BTreeSet::new(),
    waiting: BTreeMap::new(),
    held: BTreeMap::new(),
    listening: BTreeSet::new(),
});

fn lock() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A base session said hello (apid's hello route, for `kind` base).
pub fn arrive(cid: u64) {
    let mut state = lock();
    state.up.insert(cid);
    // A hello is a new VM: ports an earlier VM with this CID announced are not its.
    state.listening.retain(|(seen, _)| *seen != cid);
}

/// The guest at `cid` announced that `port` listens and noded registered it (6.4): the hub need not register it.
pub fn record_listening(cid: u64, port: u16) {
    lock().listening.insert((cid, port));
}

/// Whether the guest at `cid` announced that `port` listens, since its hello.
pub fn announced(cid: u64, port: u16) -> bool {
    lock().listening.contains(&(cid, port))
}

/// The VM at `cid` answered exec item `id`. Nothing waits for an answer nobody asked for, so it is dropped.
pub fn answer(cid: u64, id: u64, answer: Answer) {
    let waiter = lock().waiting.remove(&(cid, id));
    if let Some(waiter) = waiter {
        // The step gave up on its answer, and dropped its receiver.
        let _ = waiter.send(answer);
    }
}

/// The base keys the builds held on `node` (placement reads them with the node's own, 3.5).
pub fn held_on(node: &str) -> Vec<String> {
    lock()
        .held
        .get(node)
        .map(|keys| keys.iter().cloned().collect())
        .unwrap_or_default()
}

fn hold(node: &str, key: &str) {
    lock()
        .held
        .entry(node.to_string())
        .or_default()
        .insert(key.to_string());
}

/// The base a build of `kind` for `project` is recorded under: `<project>/build` or `<project>/test`.
pub fn key(project: &str, kind: BaseKind) -> String {
    format!("{project}/{}", kind.name())
}

/// What the runner needs from the world outside the build: noded on a node, and the VM's gateway.
pub trait Host {
    /// A noded call on `node`: the status and body it answered.
    fn noded(
        &self,
        node: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Value), String>;
    /// Pushes `message` to the VM at `cid`, through its gateway: Ok once it is on the wire.
    fn send(&self, cid: u64, message: &HostMessage) -> Result<(), String>;
}

/// The hub's own calls: noded through the fleet, the VM through its node's gateway.
pub struct Live<'a>(pub &'a App);

impl Host for Live<'_> {
    fn noded(
        &self,
        node: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Value), String> {
        fleet::noded(self.0, node, method, path, body)
    }

    fn send(&self, cid: u64, message: &HostMessage) -> Result<(), String> {
        match fleet::send(self.0, cid, &json!({ "message": message }))? {
            (200, _) => Ok(()),
            (status, reply) => Err(format!(
                "the VM did not take the command (HTTP {status}): {reply}"
            )),
        }
    }
}

/// Runs a base's steps in its VM on `node`, for `project`.
pub struct Runner<'a, H: Host> {
    host: &'a H,
    node: String,
    project: String,
    /// How long the VM may take to say hello.
    boot_wait: Duration,
    /// The run and the VM the base's `Start` made.
    run: Option<String>,
    cid: Option<u64>,
    /// The next exec item's id.
    next_id: u64,
    /// The port the background command being started listens on (a preview's app), so its exec item says so.
    listen: Option<u16>,
}

impl<'a, H: Host> Runner<'a, H> {
    pub fn new(host: &'a H, node: String, project: String) -> Self {
        Self {
            host,
            node,
            project,
            boot_wait: BOOT_WAIT,
            run: None,
            cid: None,
            next_id: 1,
            listen: None,
        }
    }

    /// Starts the base's VM on the node: noded's run, of the workspace image, with the shared `volumes` mounted.
    fn start(&mut self, volumes: &[&str]) -> Result<(), String> {
        let body = json!({
            "n": 1,
            "vcpus": VCPUS,
            "memory_mib": MEMORY_MIB,
            "keepalive": true,
            "kind": KIND_BASE,
            "volumes": volumes,
        });
        let (status, run) = self
            .host
            .noded(&self.node, "POST", "/v1/runs", Some(&body))?;
        if status >= 300 {
            return Err(format!("noded: status {status}: {run}"));
        }
        let id = run["id"].as_str().ok_or("noded named no run")?.to_string();
        let cid = run["vms"][0]["guest_cid"]
            .as_u64()
            .ok_or("noded named no CID for the base's VM")?;
        self.run = Some(id);
        self.cid = Some(cid);
        self.await_hello(cid)
    }

    /// Waits until the VM's base session says hello, for at most `boot_wait`.
    fn await_hello(&self, cid: u64) -> Result<(), String> {
        let deadline = Instant::now() + self.boot_wait;
        loop {
            if lock().up.contains(&cid) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "the base's VM (cid {cid}) did not say hello in time"
                ));
            }
            thread::sleep(HELLO_POLL);
        }
    }

    /// Sends one exec item to the VM and waits for its answer: a foreground command for as long as its timeout (and the
    /// grace), a background one until it has started.
    fn exec(
        &mut self,
        argv: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: &str,
        timeout_s: u64,
        background: bool,
    ) -> Result<Answer, String> {
        let cid = self.cid.ok_or("the base's VM is not started")?;
        let id = self.next_id;
        self.next_id += 1;
        let message = HostMessage::Exec {
            listen: self.listen,
            id,
            argv,
            env,
            cwd: cwd.to_string(),
            background,
            timeout_s,
        };
        let wait = if background {
            START_WAIT
        } else {
            Duration::from_secs(timeout_s) + ANSWER_GRACE
        };
        let (tx, rx): (Sender<Answer>, Receiver<Answer>) = channel();
        lock().waiting.insert((cid, id), tx);
        let sent = self.host.send(cid, &message);
        let answered = match sent {
            Ok(()) => rx
                .recv_timeout(wait)
                .map_err(|_| format!("the VM did not answer command {id} in time")),
            Err(why) => Err(why),
        };
        lock().waiting.remove(&(cid, id));
        answered
    }

    /// Runs a command that must succeed: its failure is the step's, with the command's output.
    fn must(
        &mut self,
        argv: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: &str,
        timeout_s: u64,
        background: bool,
    ) -> Result<(), String> {
        let answer = self.exec(argv, env, cwd, timeout_s, background)?;
        if answer.ok {
            return Ok(());
        }
        let status = match answer.exit_code {
            Some(code) => format!("exit {code}"),
            None => "no exit status".to_string(),
        };
        Err(format!("{status}: {}", answer.output.trim_end()))
    }

    /// Checks out `commit` of `repo` in the workspace: git starts the repository with the node's objects as its
    /// alternates (the file [`base_recipe::alternates_file`] names), and the Weft CLI lays the commit's files out.
    fn checkout(&mut self, commit: &str, alternates: &str) -> Result<(), String> {
        let (path, text) = base_recipe::alternates_file(alternates);
        let none = BTreeMap::new();
        self.must(
            argv(&["git", "init", "--quiet", WORKSPACE]),
            none.clone(),
            WORKSPACE,
            STEP_TIMEOUT_S,
            false,
        )?;
        self.must(
            argv(&["sh", "-c", "printf '%s' \"$0\" > \"$1\"", &text, path]),
            none.clone(),
            WORKSPACE,
            STEP_TIMEOUT_S,
            false,
        )?;
        self.must(
            argv(&[
                "weft",
                "checkout",
                "--candidate",
                commit,
                "--dir",
                WORKSPACE,
            ]),
            none,
            WORKSPACE,
            STEP_TIMEOUT_S,
            false,
        )
    }

    /// Stops the VM, snapshots it as the base of `kind` (a checkpoint, then the stop), and holds the base on the node.
    fn snapshot(&mut self, kind: BaseKind) -> Result<(), String> {
        let (run, cid) = match (self.run.clone(), self.cid) {
            (Some(run), Some(cid)) => (run, cid),
            _ => return Err("the base's VM is not started".into()),
        };
        let label = format!("base-{}", kind.name());
        let checkpoint = format!("/v1/runs/{run}/vms/0/checkpoints");
        let (status, reply) = self.host.noded(
            &self.node,
            "POST",
            &checkpoint,
            Some(&json!({ "label": label })),
        )?;
        if status >= 300 {
            return Err(format!("noded: status {status} for the snapshot: {reply}"));
        }
        let (status, reply) = self.host.noded(
            &self.node,
            "POST",
            &format!("/v1/runs/{run}/vms/0/stop"),
            None,
        )?;
        if status >= 300 {
            return Err(format!("noded: status {status} stopping VM {cid}: {reply}"));
        }
        hold(&self.node, &key(&self.project, kind));
        Ok(())
    }
}

/// A test fork (PLAN 21.3): a VM started with a test base's steps but not its snapshot, so the checks of a change (the
/// CI recipe's lint, build, tests and audit) run their own commands in it, one at a time. The fork is retired, its VM
/// stopped, when its checks are done.
pub struct Fork<'a, H: Host> {
    runner: Runner<'a, H>,
}

impl<'a, H: Host> Fork<'a, H> {
    /// Starts the fork's VM and runs `steps` in it, in order (`base_recipe::fork_steps`). The first that fails fails the
    /// start, which names the step.
    pub fn start(
        host: &'a H,
        node: String,
        project: String,
        steps: &[Step],
    ) -> Result<Self, String> {
        let mut runner = Runner::new(host, node, project);
        base_recipe::execute(&mut runner, steps)?;
        Ok(Self { runner })
    }

    /// Runs one command in the fork. Its answer is what the command did: a command that fails is an answer, not an error.
    pub fn exec(&mut self, command: &Command) -> Result<Answer, String> {
        let env = command
            .env
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        self.runner.exec(
            command.argv.clone(),
            env,
            command.cwd,
            command.timeout_s,
            false,
        )
    }

    /// Where the fork's VM runs: its noded run and its CID, once the start made the VM (a preview keeps both).
    pub fn location(&self) -> Option<(String, u64)> {
        Some((self.runner.run.clone()?, self.runner.cid?))
    }

    /// Stops the fork's VM.
    pub fn retire(self) -> Result<(), String> {
        let Some(run) = self.runner.run.clone() else {
            return Ok(());
        };
        let path = format!("/v1/runs/{run}/vms/0/stop");
        let (status, reply) = self
            .runner
            .host
            .noded(&self.runner.node, "POST", &path, None)?;
        if status >= 300 {
            return Err(format!("noded: status {status} stopping the fork: {reply}"));
        }
        Ok(())
    }
}

impl<H: Host> VmRunner for Runner<'_, H> {
    fn run(&mut self, step: &Step) -> Result<(), String> {
        match step {
            Step::Start { template, volumes } => {
                if *template != OS_TEMPLATE {
                    return Err(format!(
                        "noded starts the {OS_TEMPLATE} image, not {template}"
                    ));
                }
                self.start(volumes)
            }
            Step::Run(command) | Step::Background(command) => {
                let background = matches!(step, Step::Background(_));
                let env = command
                    .env
                    .iter()
                    .map(|(name, value)| (name.to_string(), value.to_string()))
                    .collect();
                let argv = command.argv.clone();
                // A background command with a port says so in its exec item: the guest announces it once it listens.
                self.listen = command.listen;
                let outcome = self.must(argv, env, command.cwd, command.timeout_s, background);
                self.listen = None;
                outcome
            }
            Step::Checkout {
                commit, alternates, ..
            } => self.checkout(commit, alternates),
            Step::Snapshot { kind } => self.snapshot(*kind),
        }
    }
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| part.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_stack::parse;

    /// A fake host: noded starts run `r1` with the VM at `vm` and records what it was asked. The VM answers each exec
    /// item by `answers` (argv[0] to an answer; a missing one succeeds), and says hello when it is started if `hello`.
    #[derive(Default)]
    struct FakeHost {
        vm: u64,
        calls: Mutex<Vec<String>>,
        sent: Mutex<Vec<HostMessage>>,
        answers: BTreeMap<&'static str, Answer>,
        hello: bool,
    }

    impl FakeHost {
        fn sent(&self) -> Vec<HostMessage> {
            self.sent.lock().unwrap().clone()
        }
    }

    impl Host for FakeHost {
        fn noded(
            &self,
            node: &str,
            method: &str,
            path: &str,
            body: Option<&Value>,
        ) -> Result<(u16, Value), String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{node} {method} {path}"));
            match (method, path) {
                ("POST", "/v1/runs") => {
                    assert_eq!(body.unwrap()["kind"], "base");
                    assert_eq!(body.unwrap()["keepalive"], true);
                    assert!(body.unwrap()["role"].is_null(), "a base has no role");
                    if self.hello {
                        arrive(self.vm);
                    }
                    Ok((201, json!({"id": "r1", "vms": [{"guest_cid": self.vm}]})))
                }
                ("POST", "/v1/runs/r1/vms/0/checkpoints") => {
                    assert_eq!(body.unwrap()["label"], "base-build");
                    Ok((201, json!({"name": "cp-1-base-build"})))
                }
                ("POST", "/v1/runs/r1/vms/0/stop") => Ok((200, json!({}))),
                other => panic!("unexpected noded call {other:?}"),
            }
        }

        fn send(&self, cid: u64, message: &HostMessage) -> Result<(), String> {
            assert_eq!(cid, self.vm);
            self.sent.lock().unwrap().push(message.clone());
            // The VM answers in another thread, as the harness does, once the hub waits for it.
            if let HostMessage::Exec { id, argv, .. } = message {
                let answer = self
                    .answers
                    .get(argv[0].as_str())
                    .cloned()
                    .unwrap_or(Answer {
                        ok: true,
                        exit_code: Some(0),
                        output: String::new(),
                    });
                let id = *id;
                let vm = self.vm;
                thread::spawn(move || answer_later(vm, id, answer));
            }
            Ok(())
        }
    }

    fn answer_later(cid: u64, id: u64, answer: Answer) {
        // The step registers its waiter before it sends; the answer is given once it has.
        for _ in 0..1000 {
            if lock().waiting.contains_key(&(cid, id)) {
                return super::answer(cid, id, answer);
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Rust, no declared commands: the recipe's steps for a build base, checked out at `c1`.
    const RUST: &str = "languages = []\n[toolchains]\nrust = \"1.88.0\"\n";

    /// A fake VM that says hello. Each test has its own VM: the waiting answers are keyed by (VM, exec id), and tests
    /// run in parallel.
    fn host_that_hello(vm: u64) -> FakeHost {
        FakeHost {
            vm,
            hello: true,
            ..FakeHost::default()
        }
    }

    #[test]
    fn a_build_runs_every_step_in_the_vm_and_holds_the_base_on_its_node() {
        let host = host_that_hello(500);
        let stack = parse(RUST).unwrap();
        let steps = base_recipe::steps(&stack, BaseKind::Build, "repo-1", "c1");
        let node = "n-build-1".to_string();
        let mut runner = Runner::new(&host, node.clone(), "pr-held".into());
        base_recipe::execute(&mut runner, &steps).unwrap();

        let sent = host.sent();
        let argvs: Vec<Vec<String>> = sent
            .iter()
            .filter_map(|message| match message {
                HostMessage::Exec { argv, .. } => Some(argv.clone()),
                _ => None,
            })
            .collect();
        // install rust (two), sccache's start, git init, alternates, weft checkout, cargo fetch, cargo build.
        assert_eq!(argvs.len(), 8, "{argvs:?}");
        assert_eq!(argvs[0][..3], ["rustup", "toolchain", "install"]);
        assert_eq!(argvs[2], ["sccache", "--start-server"]);
        assert_eq!(argvs[3], ["git", "init", "--quiet", WORKSPACE]);
        // The alternates file: the node's objects, written by a shell that takes the path and text as arguments.
        assert_eq!(argvs[4][3], "/data/git-objects/objects\n");
        assert_eq!(argvs[4][4], base_recipe::ALTERNATES_FILE);
        assert_eq!(
            argvs[5],
            ["weft", "checkout", "--candidate", "c1", "--dir", WORKSPACE]
        );
        assert_eq!(argvs[6], ["cargo", "fetch"]);
        assert_eq!(argvs[7], ["cargo", "build", "--all-targets"]);
        let Some(HostMessage::Exec { env, cwd, .. }) = sent.last() else {
            panic!("the last item is not an exec: {sent:?}")
        };
        assert_eq!(cwd, WORKSPACE);
        assert_eq!(
            env.get("RUSTC_WRAPPER").map(String::as_str),
            Some("sccache")
        );
        assert_eq!(held_on(&node), vec!["pr-held/build".to_string()]);
        assert!(
            host.calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| call.ends_with("/stop"))
        );
    }

    #[test]
    fn a_test_fork_runs_its_checks_in_a_vm_without_the_build_or_the_snapshot_and_is_retired() {
        let host = host_that_hello(700);
        let stack = parse(RUST).unwrap();
        let steps = base_recipe::fork_steps(&stack, "repo-1", "c1");
        let mut fork = Fork::start(&host, "n-fork-1".into(), "pr-fork".into(), &steps).unwrap();
        let check = Command {
            label: "lint".into(),
            argv: argv(&["cargo", "clippy", "--workspace"]),
            cwd: WORKSPACE,
            timeout_s: 60,
            env: Vec::new(),
            listen: None,
        };
        let answer = fork.exec(&check).unwrap();
        assert!(answer.ok);
        fork.retire().unwrap();

        let argvs: Vec<Vec<String>> = host
            .sent()
            .iter()
            .filter_map(|message| match message {
                HostMessage::Exec { argv, .. } => Some(argv.clone()),
                _ => None,
            })
            .collect();
        assert!(argvs.contains(&argv(&["cargo", "fetch"])), "{argvs:?}");
        assert!(
            !argvs.contains(&argv(&["cargo", "build", "--all-targets"])),
            "the build is the check's"
        );
        assert_eq!(
            argvs.last(),
            Some(&argv(&["cargo", "clippy", "--workspace"]))
        );
        let calls = host.calls.lock().unwrap().clone();
        assert!(
            calls.iter().all(|call| !call.contains("checkpoints")),
            "{calls:?}"
        );
        assert!(
            calls.iter().any(|call| call.ends_with("/vms/0/stop")),
            "{calls:?}"
        );
    }

    #[test]
    fn a_failing_command_fails_its_step_with_the_exit_and_the_output() {
        let mut answers = BTreeMap::new();
        answers.insert(
            "cargo",
            Answer {
                ok: false,
                exit_code: Some(101),
                output: "error[E0432]: unresolved import\n".into(),
            },
        );
        let host = FakeHost {
            vm: 501,
            hello: true,
            answers,
            ..FakeHost::default()
        };
        let stack = parse(RUST).unwrap();
        let steps = base_recipe::steps(&stack, BaseKind::Build, "repo-2", "c2");
        let mut runner = Runner::new(&host, "n-fail".into(), "pr-fail".into());
        let err = base_recipe::execute(&mut runner, &steps).unwrap_err();
        assert!(
            err.contains("base step") && err.contains("fetch dependencies"),
            "{err}"
        );
        assert!(err.contains("exit 101: error[E0432]"), "{err}");
        // No step after the failed one ran, and the base is not held.
        assert!(host.sent().len() < steps.len());
        assert!(held_on("n-fail").is_empty());
    }

    #[test]
    fn a_vm_that_never_says_hello_fails_the_start() {
        let host = FakeHost {
            vm: 900,
            ..FakeHost::default()
        };
        let mut runner = Runner::new(&host, "n-silent".into(), "pr-silent".into());
        runner.boot_wait = Duration::from_millis(50);
        let err = runner
            .run(&Step::Start {
                template: OS_TEMPLATE,
                volumes: vec!["git-objects"],
            })
            .unwrap_err();
        assert!(err.contains("did not say hello"), "{err}");
    }

    #[test]
    fn a_background_command_is_answered_when_it_starts() {
        let host = host_that_hello(502);
        let mut runner = Runner::new(&host, "n-daemon".into(), "pr-daemon".into());
        runner
            .run(&Step::Start {
                template: OS_TEMPLATE,
                volumes: vec![],
            })
            .unwrap();
        let daemon = base_recipe::Command {
            label: "start rust-analyzer".into(),
            argv: vec!["rust-analyzer".into()],
            cwd: WORKSPACE,
            timeout_s: STEP_TIMEOUT_S,
            env: Vec::new(),
            listen: None,
        };
        runner.run(&Step::Background(daemon)).unwrap();
        let sent = host.sent();
        assert!(matches!(
            sent.last(),
            Some(HostMessage::Exec {
                background: true,
                ..
            })
        ));
    }

    #[test]
    fn a_wrong_template_is_refused_before_noded_is_asked() {
        let host = host_that_hello(503);
        let mut runner = Runner::new(&host, "n-x".into(), "pr-x".into());
        let err = runner
            .run(&Step::Start {
                template: "other",
                volumes: vec![],
            })
            .unwrap_err();
        assert!(err.contains("starts the workspace image"), "{err}");
        assert!(host.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn an_answer_nobody_waits_for_is_dropped() {
        answer(
            777,
            1,
            Answer {
                ok: true,
                exit_code: Some(0),
                output: String::new(),
            },
        );
        // The waiter map is shared by the tests that run in parallel: only this VM's entry is asserted.
        assert!(!lock().waiting.contains_key(&(777, 1)));
    }

    #[test]
    fn a_guests_port_announcement_counts_until_its_vm_says_hello_again() {
        record_listening(778, 8080);
        assert!(announced(778, 8080), "the announcement is recorded");
        assert!(!announced(778, 9999), "only the announced port counts");
        arrive(778);
        assert!(
            !announced(778, 8080),
            "a new VM with the same CID has announced nothing"
        );
    }
}
