//! The web and terminal targets on a scenario, through the loop (PLAN 19.3).
//!
//! The targets run in a test fork, as they do in production: a fake fork (`LocalFork`) is a noded and a VM whose exec runs
//! on this machine, in a scratch workspace. Its browser is a real Chrome when one is installed (the test skips, printing
//! why, when none is), and its DevTools port is relayed through vsockd's inbound listener, as 6.4's tests relay an app.
//! The terminal's program is a shell script behind a real pseudo-terminal runner (`pty-run.py`). The driver and the
//! planner are fakes: the gateway's scripts answer for the stand-in's element ids, and a page's and a terminal's ids are
//! their own.

use std::cell::{Cell, RefCell};
use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use apz_blobs::Blobs;
use apz_link::inbound::{Duplex, GuestDial, PortCheck};
use apz_proto::wire::HostMessage;
use serde_json::{Value, json};

use super::fork::{self, Channel, DEVTOOLS_PORT};
use super::models::{Judgement, Look, Models, Pick, PickRequest, PlanRequest, Tier};
use super::run::Ctx;
use super::scenario::{Assertion, Origin, Scenario, Step};
use super::target::{Action, Screenshot, Target};
use super::tui::{ForkTui, TERMINAL};
use super::web::{self, Web};
use super::{Execution, Input, Ruling, execute};
use crate::base_recipe::{OS_TEMPLATE, Step as ForkStep};
use crate::base_vm::{self, Answer, Fork, Host};
use crate::fleet::LOCAL;
use crate::nodes;

/// A driver and planner that answer from code: the plan is one acceptance scenario, a failure is a defect, and the
/// driver's picks come from `pick`, numbered from the start of each target's run.
struct Fake {
    scenario: Scenario,
    pick: fn(usize, &PickRequest) -> Pick,
    picks: Cell<usize>,
}

impl Models for Fake {
    fn plan(&self, _req: &PlanRequest) -> Result<Vec<Scenario>, String> {
        Ok(vec![self.scenario.clone()])
    }

    fn judge(&self, _scenario: &Scenario, _failure: &str) -> Result<Judgement, String> {
        Ok(Judgement {
            defect: true,
            reason: "the receipt is not shown".into(),
        })
    }

    fn pick(
        &self,
        tier: Tier,
        req: &PickRequest,
        _shot: Option<&Screenshot>,
    ) -> Result<Pick, String> {
        if tier == Tier::Strong {
            return Err("the fake driver does not escalate".into());
        }
        let n = self.picks.get();
        self.picks.set(n + 1);
        Ok((self.pick)(n, req))
    }

    fn look(&self, _question: &str, _shot: &Screenshot) -> Result<Look, String> {
        Ok(Look {
            holds: true,
            reason: "the fake checker".into(),
        })
    }
}

/// One scenario on the targets `make` opens (a fresh one for each run), through the loop.
fn run<'a>(
    scenario: Scenario,
    pick: fn(usize, &PickRequest) -> Pick,
    make: impl Fn() -> Result<Box<dyn Target + 'a>, String>,
) -> Execution {
    let fake = Fake {
        scenario,
        pick,
        picks: Cell::new(0),
    };
    let dir = tempfile::tempdir().unwrap();
    let blobs = Blobs::open(dir.path()).unwrap();
    let ctx = Ctx {
        models: &fake,
        blobs: &blobs,
        threshold: 0.6,
        actions: 4,
    };
    let input = Input {
        symbol: "cart",
        acceptance: &[],
        regressions: &[],
        exploratory: 0,
    };
    let mut open = || {
        fake.picks.set(0);
        make()
    };
    execute(&ctx, &input, &mut open).expect("the scenario runs on the target")
}

/// Pressing Pay reaches the receipt.
fn pay_scenario() -> Scenario {
    scenario("Paying reaches the receipt", "press Pay", "Receipt")
}

fn scenario(title: &str, goal: &str, shows: &str) -> Scenario {
    Scenario {
        id: "pay".into(),
        title: title.into(),
        origin: Origin::Acceptance,
        symbol: "cart".into(),
        steps: vec![Step {
            goal: goal.into(),
            expect: vec![Assertion::Present(shows.into())],
            actions: None,
        }],
        assertions: vec![],
        checkpoints: vec![],
        timings: vec![],
    }
}

/// Clicks the button the goal names: the option whose description names a "Pay" button.
fn press_pay(_n: usize, req: &PickRequest) -> Pick {
    match req.options.iter().find(|(_, d)| d.contains("\"Pay\"")) {
        Some((id, why)) => Pick {
            action: Some(Action::Click {
                element: id.clone(),
            }),
            confidence: 0.9,
            why: format!("{why} is the button the goal names"),
        },
        None => Pick {
            action: None,
            confidence: 0.9,
            why: "no Pay button on this screen".into(),
        },
    }
}

/// Types the letter, then presses Enter; a program that answers with the receipt passes, and one that does not fails
/// (the picks go on pressing Enter until the budget is spent).
fn type_then_enter(n: usize, _req: &PickRequest) -> Pick {
    let action = if n == 0 {
        Action::Type {
            element: TERMINAL.into(),
            text: "p".into(),
        }
    } else {
        Action::Press {
            key: "Enter".into(),
        }
    };
    Pick {
        action: Some(action),
        confidence: 0.9,
        why: "type the letter, then press Enter".into(),
    }
}

/// Serves the cart at `/` (its Pay leads to the error page, or to the receipt when `fixed`), the receipt and the error
/// page, on a local port. Returns the address. A thread per connection: a browser opens sockets it sends nothing on
/// (preconnects), and one of them must not hold up the page's request.
fn serve(fixed: bool) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            thread::spawn(move || answer(stream, fixed));
        }
    });
    format!("http://{address}")
}

fn answer(mut stream: TcpStream, fixed: bool) {
    let Ok(clone) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(clone);
    let mut request = String::new();
    if reader.read_line(&mut request).is_err() {
        return;
    }
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) | Err(_) => break,
            Ok(_) if header == "\r\n" || header == "\n" => break,
            Ok(_) => {}
        }
    }
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let (status, html) = match page(fixed, path) {
        Some(html) => ("200 OK", html),
        None => (
            "404 Not Found",
            "<!doctype html><title>Not found</title><h1>Not found</h1>".to_string(),
        ),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

fn page(fixed: bool, path: &str) -> Option<String> {
    let (title, body) = match (path, fixed) {
        ("/", false) => (
            "Cart",
            "<h1>Cart</h1><button onclick=\"location.href='/error'\">Pay</button>",
        ),
        ("/", true) => (
            "Cart",
            "<h1>Cart</h1><button onclick=\"location.href='/receipt'\">Pay</button>",
        ),
        ("/receipt", _) => ("Receipt", "<h1>Receipt</h1><a href=\"/\">Back to shop</a>"),
        ("/error", _) => ("Error", "<h1>Error</h1><button>Retry</button>"),
        _ => return None,
    };
    Some(format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>{title}</title></head><body>{body}</body></html>"
    ))
}

fn sh(script: &str) -> Vec<String> {
    vec!["/bin/sh".into(), "-c".into(), script.into()]
}

/// A program that reads a line at a time: `p` prints the receipt (or, seeded, an error), anything else says so.
fn terminal_program(answer: &str) -> String {
    format!(
        "printf 'Shop\\n> '; while IFS= read -r line; do case \"$line\" in p) echo {answer} ;; *) echo Unknown ;; esac; printf '> '; done"
    )
}

/// The fake fork's VM and its noded: its exec runs on this machine in a scratch workspace. A background browser runs with
/// its DevTools port on a free port here, which the guest's port (`DEVTOOLS_PORT`) is relayed to.
struct LocalFork {
    cid: u64,
    workspace: tempfile::TempDir,
    chrome: Option<PathBuf>,
    children: Mutex<Vec<Child>>,
    devtools: Arc<Mutex<Option<u16>>>,
}

impl LocalFork {
    fn new(cid: u64, chrome: Option<PathBuf>) -> LocalFork {
        LocalFork {
            cid,
            workspace: tempfile::tempdir().unwrap(),
            chrome,
            children: Mutex::new(Vec::new()),
            devtools: Arc::new(Mutex::new(None)),
        }
    }

    /// A foreground command, run to its end here.
    fn run(&self, argv: &[String]) -> Answer {
        let Some((program, args)) = argv.split_first() else {
            return failed("the command is empty");
        };
        match Command::new(program)
            .args(args)
            .current_dir(self.workspace.path())
            .output()
        {
            Ok(out) => Answer {
                ok: out.status.success(),
                exit_code: out.status.code(),
                output: format!(
                    "{}{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                ),
            },
            Err(why) => failed(&format!("{program} did not run: {why}")),
        }
    }

    /// A background command: started here and left running. The browser is the real Chrome, on a free port whose
    /// DevTools file the fake reads (the guest's fixed port is the one the relay serves).
    fn start(&self, argv: &[String], listen: Option<u16>) -> Answer {
        let Some((program, args)) = argv.split_first() else {
            return failed("the command is empty");
        };
        let browser = program == fork::BROWSER;
        let mut command = Command::new(if browser {
            self.chrome
                .clone()
                .unwrap_or_else(|| PathBuf::from(program))
        } else {
            PathBuf::from(program)
        });
        let profile = self.workspace.path().join("browser-profile");
        let args: Vec<String> = if browser {
            args.iter()
                .map(|arg| {
                    if arg == &format!("--remote-debugging-port={DEVTOOLS_PORT}") {
                        "--remote-debugging-port=0".to_string()
                    } else if arg.starts_with("--user-data-dir=") {
                        format!("--user-data-dir={}", profile.display())
                    } else {
                        arg.clone()
                    }
                })
                .collect()
        } else {
            args.to_vec()
        };
        command
            .args(&args)
            .current_dir(self.workspace.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = match command.spawn() {
            Ok(child) => child,
            Err(why) => return failed(&format!("{program} did not start: {why}")),
        };
        self.children.lock().unwrap().push(child);
        if browser {
            let port = wait_for_devtools(&profile);
            match port {
                Some(port) => *self.devtools.lock().unwrap() = Some(port),
                None => return failed("the browser did not open its DevTools port"),
            }
        }
        if let Some(port) = listen {
            base_vm::record_listening(self.cid, port);
        }
        Answer {
            ok: true,
            exit_code: None,
            output: String::new(),
        }
    }
}

fn failed(why: &str) -> Answer {
    Answer {
        ok: false,
        exit_code: None,
        output: why.to_string(),
    }
}

/// The port a browser opened in `profile` serves its DevTools protocol on (its `DevToolsActivePort` file's first line).
fn wait_for_devtools(profile: &Path) -> Option<u16> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let port = std::fs::read_to_string(profile.join("DevToolsActivePort"))
            .ok()
            .and_then(|text| text.lines().next().and_then(|l| l.trim().parse().ok()));
        if port.is_some() || Instant::now() >= deadline {
            return port;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

impl Host for LocalFork {
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
                    200,
                    json!({ "id": "run-test", "vms": [{ "guest_cid": self.cid }] }),
                ))
            }
            ("POST", path) if path.ends_with("/stop") => Ok((200, json!({}))),
            _ => Ok((404, json!({ "error": "the fake fork has no such route" }))),
        }
    }

    fn send(&self, cid: u64, message: &HostMessage) -> Result<(), String> {
        let HostMessage::Exec {
            id,
            argv,
            background,
            listen,
            ..
        } = message
        else {
            return Err("the fake fork takes commands only".into());
        };
        let answer = if *background {
            self.start(argv, *listen)
        } else {
            self.run(argv)
        };
        base_vm::answer(cid, *id, answer);
        Ok(())
    }
}

impl Drop for LocalFork {
    fn drop(&mut self) {
        for child in self.children.get_mut().unwrap().iter_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The inbound listener's guest: the fake fork serves the browser's DevTools port once the browser runs, by dialing it.
struct FakeGuest {
    cid: u64,
    devtools: Arc<Mutex<Option<u16>>>,
}

impl PortCheck for FakeGuest {
    fn served(&self, cid: u64, port: u16) -> bool {
        cid == self.cid && port == DEVTOOLS_PORT && self.devtools.lock().unwrap().is_some()
    }
}

impl GuestDial for FakeGuest {
    fn dial(&self, _cid: u64, _port: u16) -> io::Result<Box<dyn Duplex>> {
        let port = self
            .devtools
            .lock()
            .unwrap()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "no browser"))?;
        Ok(Box::new(TcpStream::connect(("127.0.0.1", port))?))
    }
}

/// Starts the fake fork's inbound listener on this machine and returns its port.
fn relay(fake: &LocalFork) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let guest = Arc::new(FakeGuest {
        cid: fake.cid,
        devtools: Arc::clone(&fake.devtools),
    });
    let check: Arc<dyn PortCheck> = guest.clone();
    let dial: Arc<dyn GuestDial> = guest;
    thread::spawn(move || apz_link::inbound::serve(listener, check, dial));
    port
}

/// The fake fork, started with its VM and no steps of its own.
fn start_fork(fake: &LocalFork) -> RefCell<Fork<'_, LocalFork>> {
    let steps = [ForkStep::Start {
        template: OS_TEMPLATE,
        volumes: vec![],
    }];
    let fork = Fork::start(fake, LOCAL.to_string(), "cart".to_string(), &steps)
        .expect("the fake fork starts");
    RefCell::new(fork)
}

#[test]
fn a_web_target_in_the_lane_fork_fails_on_the_seeded_bug_and_passes_the_fixed_page() {
    let chrome = match web::local::find_chrome() {
        Ok(chrome) => chrome,
        Err(why) => {
            eprintln!("skipped the web target's test: {why}");
            return;
        }
    };
    let fake = LocalFork::new(9101, Some(chrome));
    let inbound = relay(&fake);
    let fork = start_fork(&fake);
    let browser = fork::browser_command();
    let started = Channel::exec(&fork, &browser, true).expect("the browser starts in the fork");
    assert!(started.ok, "{}", started.output);
    let nodes = nodes::Store::holding(Vec::new());
    let cid = fake.cid;

    let buggy_url = format!("{}/", serve(false));
    let bug = run(
        pay_scenario(),
        press_pay,
        || -> Result<Box<dyn Target>, String> {
            let mut dial = fork::port_dial(&nodes, LOCAL, cid, DEVTOOLS_PORT, inbound);
            Ok(Box::new(Web::attach(&mut dial, &buggy_url)?))
        },
    );
    assert_eq!(bug.target, "web");
    assert_eq!(
        bug.found.len(),
        1,
        "the seeded bug is a finding: {}",
        bug.summary()
    );
    assert!(
        matches!(bug.found[0].ruling, Ruling::Confirmed { .. }),
        "the re-run fails again: {:?}",
        bug.found[0].ruling
    );

    let fixed_url = format!("{}/", serve(true));
    let fixed = run(
        pay_scenario(),
        press_pay,
        || -> Result<Box<dyn Target>, String> {
            let mut dial = fork::port_dial(&nodes, LOCAL, cid, DEVTOOLS_PORT, inbound);
            Ok(Box::new(Web::attach(&mut dial, &fixed_url)?))
        },
    );
    assert_eq!(fixed.target, "web");
    assert!(
        fixed.found.is_empty(),
        "the fixed page passes: {}",
        fixed.summary()
    );
    assert!(fixed.outcomes[0].passed, "the acceptance scenario holds");
    assert!(
        fixed
            .evidence
            .iter()
            .any(|r| r.action.starts_with("click dom-") && r.note.contains("Pay")),
        "the press of Pay is evidence"
    );
    assert!(
        fixed
            .evidence
            .iter()
            .all(|r| r.tree.len() == 64 && r.screenshot.len() == 64),
        "each observation keeps its tree and its picture in the object store"
    );
    let fork = fork.into_inner();
    fork.retire().expect("the fake fork retires");
}

#[test]
fn a_terminal_target_in_the_lane_fork_fails_on_the_seeded_bug_and_passes_the_fixed_program() {
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("skipped the terminal target's test: no python3 to run the pty runner");
        return;
    }
    let fake = LocalFork::new(9102, None);
    let fork = start_fork(&fake);
    let channel: &dyn Channel = &fork;
    let fixed_program = sh(&terminal_program("Receipt"));
    let fixed = run(
        scenario("Typing p prints the receipt", "send p", "Receipt"),
        type_then_enter,
        || Ok(Box::new(ForkTui::open(channel, &fixed_program, 80, 24)?)),
    );
    assert_eq!(fixed.target, "tui");
    assert!(
        fixed.found.is_empty(),
        "the fixed program passes: {}",
        fixed.summary()
    );
    assert!(fixed.outcomes[0].passed);

    let buggy_program = sh(&terminal_program("Error"));
    let bug = run(
        scenario("Typing p prints the receipt", "send p", "Receipt"),
        type_then_enter,
        || Ok(Box::new(ForkTui::open(channel, &buggy_program, 80, 24)?)),
    );
    assert_eq!(
        bug.found.len(),
        1,
        "the seeded bug is a finding: {}",
        bug.summary()
    );
    assert!(matches!(bug.found[0].ruling, Ruling::Confirmed { .. }));
    fork.into_inner().retire().expect("the fake fork retires");
}

#[test]
fn a_scenario_on_a_local_chrome_passes_the_fixed_page_and_fails_the_seeded_bug() {
    let chrome = match web::local::find_chrome() {
        Ok(chrome) => chrome,
        Err(why) => {
            eprintln!("skipped the local web test: {why}");
            return;
        }
    };
    let buggy_url = format!("{}/", serve(false));
    let bug = run(
        pay_scenario(),
        press_pay,
        || -> Result<Box<dyn Target>, String> {
            Ok(Box::new(web::local::open(&chrome, &buggy_url)?))
        },
    );
    assert_eq!(bug.found.len(), 1, "the seeded bug is a finding");
    assert!(matches!(bug.found[0].ruling, Ruling::Confirmed { .. }));
    let fixed_url = format!("{}/", serve(true));
    let fixed = run(
        pay_scenario(),
        press_pay,
        || -> Result<Box<dyn Target>, String> {
            Ok(Box::new(web::local::open(&chrome, &fixed_url)?))
        },
    );
    assert!(
        fixed.found.is_empty(),
        "the fixed page passes: {}",
        fixed.summary()
    );
}

#[test]
fn a_step_names_one_target_and_a_terminal_names_a_program() {
    use crate::recipes::model::{TuiTarget, UserTest, WebTarget};
    use crate::user_test::target::StandIn;
    let base = UserTest {
        planner: "mock/script".into(),
        driver: "mock/script".into(),
        escalate: "mock/script".into(),
        checker: "mock/script".into(),
        stand_in: None,
        web: None,
        tui: None,
        acceptance: vec![],
        threshold: 0.6,
        actions: 8,
        exploratory: 0,
    };
    assert!(base.check().is_ok());
    let both = UserTest {
        web: Some(WebTarget::default()),
        stand_in: Some(StandIn {
            start: "a".into(),
            latency_ms: 0,
            screens: vec![],
        }),
        ..base.clone()
    };
    assert!(both.check().unwrap_err().contains("one target"));
    let nothing = UserTest {
        tui: Some(TuiTarget {
            command: vec![],
            cols: 80,
            rows: 24,
        }),
        ..base
    };
    assert!(nothing.check().unwrap_err().contains("names a program"));
}

#[test]
fn a_smoke_set_on_a_local_chrome_passes_the_healthy_page_and_fails_a_broken_deploy() {
    let chrome = match web::local::find_chrome() {
        Ok(chrome) => chrome,
        Err(why) => {
            eprintln!("skipped the smoke set's web test: {why}");
            return;
        }
    };
    let blobs_dir = tempfile::tempdir().unwrap();
    let blobs = Blobs::open(blobs_dir.path()).unwrap();
    // The smoke set reads the screen as it is: the cart page shows "Cart", and a broken deploy's page does not.
    let set = [scenario("The cart loads", "look at the cart", "Cart")];
    let healthy = format!("{}/", serve(true));
    let mut open = || -> Result<Box<dyn Target>, String> {
        Ok(Box::new(web::local::open(&chrome, &healthy)?))
    };
    super::run_smoke(&blobs, &mut open, &set).expect("the healthy page passes its smoke set");

    let broken = format!("{}/error", serve(false));
    let mut open =
        || -> Result<Box<dyn Target>, String> { Ok(Box::new(web::local::open(&chrome, &broken)?)) };
    let failed = super::run_smoke(&blobs, &mut open, &set)
        .expect_err("the broken deploy fails its smoke set");
    assert!(failed.starts_with("The cart loads: "), "{failed}");
}
