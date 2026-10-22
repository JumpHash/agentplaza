//! End-to-end checks of the Seatbelt backend. Each test runs a confined child and
//! asserts what the kernel let it do. The "outside" directories live under $TMPDIR,
//! which the sandbox does not grant, so they stand in for anything the user owns.

#![cfg(target_os = "macos")]

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use apz_sandbox::{Limits, Network, SandboxError, Spec, command, spawn, support};

/// A fresh directory under $TMPDIR, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("apz-sbx-{tag}-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn spec(workdir: &Path, program: &str, args: &[&str]) -> Spec {
    Spec {
        program: PathBuf::from(program),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        env: vec![(
            "PATH".to_owned(),
            "/usr/bin:/bin:/usr/sbin:/sbin".to_owned(),
        )],
        workdir: workdir.to_path_buf(),
        read_only: vec![],
        read_write: vec![],
        network: Network::None,
        limits: Limits::default(),
    }
}

fn run(spec: &Spec) -> Output {
    command(spec)
        .expect("command() must build for a valid spec")
        .output()
        .expect("the sandbox-exec process must start")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn python3() -> Option<&'static str> {
    let ok = std::process::Command::new("/usr/bin/python3")
        .args(["-c", "print(1)"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    ok.then_some("/usr/bin/python3")
}

#[test]
fn support_reports_seatbelt() {
    let report = support();
    assert_eq!(report.kind, "seatbelt", "{}", report.detail);
}

#[test]
fn writes_work_inside_the_workdir_and_fail_elsewhere() {
    let work = TempDir::new("write-work");
    let outside = TempDir::new("write-outside");
    let inside = work.path().join("inside.txt");
    let elsewhere = outside.path().join("outside.txt");

    let ok = run(&spec(
        work.path(),
        "/bin/sh",
        &["-c", &format!("echo ok > '{}'", inside.display())],
    ));
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    assert_eq!(fs::read_to_string(&inside).unwrap().trim(), "ok");

    let denied = run(&spec(
        work.path(),
        "/bin/sh",
        &["-c", &format!("echo bad > '{}'", elsewhere.display())],
    ));
    assert!(
        !denied.status.success(),
        "write outside the workdir must fail"
    );
    assert!(!elsewhere.exists());

    let temp = format!("/tmp/apz-sbx-{}-tmp.txt", std::process::id());
    let in_tmp = run(&spec(
        work.path(),
        "/bin/sh",
        &[
            "-c",
            &format!("echo t > '{temp}' && cat '{temp}' && rm '{temp}'"),
        ],
    ));
    assert!(in_tmp.status.success(), "/tmp is the granted temp dir");
    assert_eq!(stdout(&in_tmp).trim(), "t");
}

#[test]
fn reads_of_secrets_fail_and_read_only_paths_work() {
    let work = TempDir::new("read-work");
    let secret = TempDir::new("read-secret");
    let secret_file = secret.path().join("s.txt");
    fs::write(&secret_file, "TOP-SECRET").unwrap();
    let public = TempDir::new("read-public");
    fs::write(public.path().join("r.txt"), "PUBLIC").unwrap();

    let blocked = run(&spec(
        work.path(),
        "/bin/sh",
        &["-c", &format!("cat '{}'", secret_file.display())],
    ));
    assert!(
        !blocked.status.success(),
        "reading outside every grant must fail"
    );
    assert!(!stdout(&blocked).contains("TOP-SECRET"));

    let mut granted = spec(
        work.path(),
        "/bin/sh",
        &[
            "-c",
            &format!("cat '{}'", public.path().join("r.txt").display()),
        ],
    );
    granted.read_only = vec![public.path().to_path_buf()];
    let read = run(&granted);
    assert!(
        read.status.success(),
        "{}",
        String::from_utf8_lossy(&read.stderr)
    );
    assert_eq!(stdout(&read).trim(), "PUBLIC");
}

#[test]
fn the_home_directory_is_unreadable() {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let work = TempDir::new("home");
    let listing = run(&spec(work.path(), "/bin/ls", &[&home.to_string_lossy()]));
    assert!(!listing.status.success(), "HOME must not be listable");
}

#[test]
fn loopback_port_reaches_only_the_granted_port() {
    let work = TempDir::new("net");
    let granted = TcpListener::bind("127.0.0.1:0").unwrap();
    let other = TcpListener::bind("127.0.0.1:0").unwrap();
    let p = granted.local_addr().unwrap().port();
    let q = other.local_addr().unwrap().port();

    let mut to_granted = spec(
        work.path(),
        "/usr/bin/nc",
        &["-z", "-w", "2", "127.0.0.1", &p.to_string()],
    );
    to_granted.network = Network::LoopbackPort(p);
    let ok = run(&to_granted);
    assert!(
        ok.status.success(),
        "connect to the granted port: {}",
        String::from_utf8_lossy(&ok.stderr)
    );

    let mut to_other = spec(
        work.path(),
        "/usr/bin/nc",
        &["-z", "-w", "2", "127.0.0.1", &q.to_string()],
    );
    to_other.network = Network::LoopbackPort(p);
    assert!(
        !run(&to_other).status.success(),
        "other loopback ports must be refused"
    );

    if let Some(python) = python3() {
        // connect_ex returns 0 on success, 1 (EPERM) when the sandbox refuses the call.
        let probe = |port: u16| {
            format!(
                "import socket; s=socket.socket(); s.settimeout(2); print(s.connect_ex(('127.0.0.1', {port})))"
            )
        };
        let mut allowed = spec(work.path(), python, &["-c", &probe(p)]);
        allowed.network = Network::LoopbackPort(p);
        assert_eq!(stdout(&run(&allowed)).trim(), "0");
        let mut refused = spec(work.path(), python, &["-c", &probe(q)]);
        refused.network = Network::LoopbackPort(p);
        assert_eq!(stdout(&run(&refused)).trim(), "1");
    }

    let mut none_to_granted = spec(
        work.path(),
        "/usr/bin/nc",
        &["-z", "-w", "2", "127.0.0.1", &p.to_string()],
    );
    none_to_granted.network = Network::None;
    assert!(
        !run(&none_to_granted).status.success(),
        "Network::None reaches nothing"
    );
    drop((granted, other));
}

/// The rule is TCP to the granted port only: a connect to a listener on a free port succeeds, a connect to another
/// port is refused, and a UDP datagram to the granted port is refused too, since the rule names `tcp`, not `ip`.
#[test]
fn the_loopback_rule_is_tcp_to_the_granted_port_only() {
    let Some(python) = python3() else {
        return;
    };
    let work = TempDir::new("tcp-only");
    let granted = TcpListener::bind("127.0.0.1:0").unwrap();
    let other = TcpListener::bind("127.0.0.1:0").unwrap();
    let (g, o) = (
        granted.local_addr().unwrap().port(),
        other.local_addr().unwrap().port(),
    );
    // Prints three results: 0 when the connect or the send succeeds, else the errno (1 is EPERM, the sandbox's refusal).
    let script = r#"import socket
def tcp(p):
    with socket.socket() as s:
        s.settimeout(2)
        return s.connect_ex(('127.0.0.1', p))
def udp(p):
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
        try:
            s.sendto(b'x', ('127.0.0.1', p))
            return 0
        except OSError as e:
            return e.errno
print(tcp(GRANTED), tcp(OTHER), udp(GRANTED))
"#
    .replace("GRANTED", &g.to_string())
    .replace("OTHER", &o.to_string());
    let mut probe = spec(work.path(), python, &["-c", &script]);
    probe.network = Network::LoopbackPort(g);
    let out = run(&probe);
    assert_eq!(
        stdout(&out).trim(),
        "0 1 1",
        "tcp to the granted port, tcp to another port, udp to the granted port: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    drop((granted, other));
}

#[test]
fn child_gets_exactly_the_spec_environment() {
    let work = TempDir::new("env");
    let mut env_spec = spec(work.path(), "/usr/bin/env", &[]);
    env_spec.env = vec![
        ("LANG".to_owned(), "C".to_owned()),
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
    ];
    let out = run(&env_spec);
    assert!(out.status.success());
    let mut lines: Vec<String> = stdout(&out).lines().map(str::to_owned).collect();
    lines.sort();
    assert_eq!(lines, ["LANG=C", "PATH=/usr/bin:/bin"]);
}

#[test]
fn memory_limit_is_refused_because_macos_cannot_enforce_it() {
    let work = TempDir::new("mem");
    let mut capped = spec(work.path(), "/bin/sh", &["-c", "true"]);
    capped.limits.memory_bytes = Some(512 << 20);
    match command(&capped) {
        Err(SandboxError::Unsupported(why)) => assert!(why.contains("memory_bytes"), "{why}"),
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

#[test]
fn cpu_limit_kills_a_busy_loop() {
    let work = TempDir::new("cpu");
    let mut busy = spec(work.path(), "/bin/sh", &["-c", "while :; do :; done"]);
    busy.limits.cpu_seconds = Some(1);
    let mut child = spawn(&busy, Stdio::null(), Stdio::null(), Stdio::null()).unwrap();

    // Watchdog: if the limit did not stop the loop, kill it so the test cannot hang.
    let (done, waiting) = mpsc::channel::<()>();
    let pid = child.id();
    let watchdog = std::thread::spawn(move || {
        if let Err(RecvTimeoutError::Timeout) = waiting.recv_timeout(Duration::from_secs(60)) {
            // SAFETY: plain kill(2) on a child that has not been reaped yet.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
        }
    });
    let status = child.wait().unwrap();
    done.send(()).unwrap();
    watchdog.join().unwrap();
    assert!(
        !status.success(),
        "a busy loop with a 1s CPU cap must not exit cleanly"
    );
}

#[test]
fn spawn_pipes_stdin_stdout_and_stderr_and_reports_status() {
    let work = TempDir::new("spawn");
    let cat = spec(work.path(), "/bin/cat", &[]);
    let mut child = spawn(&cat, Stdio::piped(), Stdio::piped(), Stdio::piped()).unwrap();
    child
        .stdin()
        .expect("stdin was piped")
        .write_all(b"through stdin\n")
        .unwrap();
    drop(child.stdin());
    let mut out = String::new();
    child
        .stdout()
        .expect("stdout was piped")
        .read_to_string(&mut out)
        .unwrap();
    assert_eq!(out.trim(), "through stdin");
    assert!(child.stdout().is_none(), "a stream is handed out once");
    assert!(child.wait().unwrap().success());

    let failing = spec(work.path(), "/bin/sh", &["-c", "echo to-err 1>&2; exit 3"]);
    let mut child = spawn(&failing, Stdio::null(), Stdio::piped(), Stdio::piped()).unwrap();
    let mut err = String::new();
    child
        .stderr()
        .expect("stderr was piped")
        .read_to_string(&mut err)
        .unwrap();
    assert_eq!(err.trim(), "to-err");
    assert_eq!(child.wait().unwrap().code(), Some(3));
}

#[test]
fn kill_stops_a_running_child() {
    let work = TempDir::new("kill");
    let sleeper = spec(work.path(), "/bin/sleep", &["60"]);
    let mut child = spawn(&sleeper, Stdio::null(), Stdio::null(), Stdio::null()).unwrap();
    child.kill().unwrap();
    assert!(!child.wait().unwrap().success());
}

#[test]
fn workdir_names_with_quotes_and_parentheses_are_granted() {
    let base = TempDir::new("odd");
    let work = base.path().join("a\"b (c); d\\e");
    fs::create_dir_all(&work).unwrap();
    let out = run(&spec(
        &work,
        "/bin/sh",
        &["-c", "echo ok > ./made.txt && cat ./made.txt"],
    ));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stdout(&out).trim(), "ok");
}

#[test]
fn bare_program_names_resolve_on_the_spec_path() {
    let work = TempDir::new("bare");
    let mut bare = spec(work.path(), "env", &[]);
    bare.env = vec![("PATH".to_owned(), "/usr/bin".to_owned())];
    let out = run(&bare);
    assert!(out.status.success());
}

#[test]
fn relative_grant_paths_are_refused() {
    let work = TempDir::new("relative");
    let mut relative = spec(work.path(), "/bin/sh", &["-c", "true"]);
    relative.read_only = vec![PathBuf::from("relative/dir")];
    assert!(matches!(command(&relative), Err(SandboxError::Invalid(_))));
}

#[test]
fn a_confined_child_cannot_listen_even_on_the_granted_port() {
    let Some(python) = python3() else {
        return;
    };
    let work = TempDir::new("listen");
    // Take a free port, then release it so the only possible failure is the sandbox.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let bind = format!(
        "import socket; s=socket.socket(); \
         s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)\n\
         try:\n s.bind(('127.0.0.1', {port})); print('bound')\n\
         except OSError as e: print(e.errno)"
    );
    let mut listen = spec(work.path(), python, &["-c", &bind]);
    listen.network = Network::LoopbackPort(port);
    // errno 1 is EPERM: the sandbox refused the bind, not the address.
    assert_eq!(stdout(&run(&listen)).trim(), "1");
}

#[test]
fn descriptors_the_hub_holds_are_closed_before_exec() {
    let work = TempDir::new("fds");
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: `fds` has room for the two descriptors pipe(2) returns.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    let (read_end, write_end) = (fds[0], fds[1]);
    // The write end is inheritable, so the child would get it if it were not closed.
    // The child writes "leaked" through the number. The test checks the pipe itself,
    // not the shell's exit status: a closed number can be reused by a file the shell
    // opens for itself (such as /dev/tty), and writing to that is harmless.
    let leak = spec(
        work.path(),
        "/bin/sh",
        &["-c", &format!("echo leaked >&{write_end}")],
    );
    let out = run(&leak);
    // SAFETY: `write_end` is ours, closed once. `read_end` is ours and reads only the
    // pipe's buffer, which is non-blocking so the read cannot hang.
    let received = unsafe {
        libc::close(write_end);
        let flags = libc::fcntl(read_end, libc::F_GETFL);
        libc::fcntl(read_end, libc::F_SETFL, flags | libc::O_NONBLOCK);
        let mut buf = [0u8; 64];
        let n = libc::read(read_end, buf.as_mut_ptr().cast(), buf.len());
        libc::close(read_end);
        if n > 0 {
            buf[..n as usize].to_vec()
        } else {
            Vec::new()
        }
    };
    assert!(
        !String::from_utf8_lossy(&received).contains("leaked"),
        "the hub's descriptor {write_end} reached the sandbox (exit {:?})",
        out.status.code()
    );
}

#[test]
fn links_planted_in_the_workdir_do_not_reach_outside_files() {
    let work = TempDir::new("links");
    let secret = TempDir::new("links-secret");
    let file = secret.path().join("s.txt");
    fs::write(&file, "LINKED-SECRET").unwrap();
    // A symlink resolves to the outside file; a hard link needs the outside file
    // readable to be created at all. Neither may expose its contents.
    let script = format!(
        "ln -s '{f}' ./sym.txt; cat ./sym.txt; ln '{f}' ./hard.txt; cat ./hard.txt",
        f = file.display()
    );
    let out = run(&spec(work.path(), "/bin/sh", &["-c", &script]));
    assert!(
        !stdout(&out).contains("LINKED-SECRET"),
        "linked outside file was readable"
    );
}

/// Killing a confined child takes down what it started too: a grandchild left running would live on, confined, with
/// no one to stop it (a `uv tool run` server's Python).
#[test]
fn kill_takes_down_the_childs_own_children() {
    let dir = TempDir::new("killpg");
    let spec = spec(dir.path(), "/bin/sh", &["-c", "sleep 300 & echo $!; wait"]);
    let mut child = spawn(&spec, Stdio::null(), Stdio::piped(), Stdio::null()).expect("spawn");
    let mut out = child.stdout().expect("its stdout");
    let mut line = String::new();
    let mut byte = [0u8; 1];
    while out.read(&mut byte).expect("read") == 1 && byte[0] != b'\n' {
        line.push(byte[0] as char);
    }
    let grandchild: i32 = line.trim().parse().expect("the grandchild's pid");
    let alive = |pid: i32| {
        std::process::Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    assert!(alive(grandchild), "the grandchild runs");
    child.kill().expect("kill");
    let _ = child.wait();
    let mut gone = false;
    for _ in 0..50 {
        if !alive(grandchild) {
            gone = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(gone, "the grandchild went with its parent");
}
