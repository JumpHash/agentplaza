//! The macOS and simulator targets on a scenario, through the loop (PLAN 19.3), against fakes: a scripted station (the
//! workstation tools, answered from a small app), a scripted simctl runner, and a stand-in bridge. The planner and the
//! driver are fakes that answer from code. The real Xcode simulator is not used here (it needs Xcode, which this
//! machine does not have); the app-on-a-real-Mac test is in `apz-desktop-macos`.

use std::cell::RefCell;
use std::path::Path;

use apz_blobs::Blobs;
use base64::Engine as _;
use serde_json::{Value, json};

use super::desk::{Calls, StationRunner, open_lease, release_lease};
use super::macos::MacApp;
use super::models::{Judgement, Look, Models, Pick, PickRequest, PlanRequest, Tier};
use super::run::{Ctx, Failure};
use super::scenario::{Assertion, Origin, Scenario, Step};
use super::simulator::{Bridge, Simulator, UNWIRED, Unwired};
use super::target::{Action, Screenshot, Stand, StandIn, Target, Tree};
use super::{Execution, Input, Ruling, execute};
use crate::recipes::model::{MacTarget, SimTarget, UserTest};
use apz_desktop_macos::simctl::{Runner, Simctl, exec};

/// A driver and planner that answer from code: one acceptance scenario that presses Pay and expects the receipt, a failure is
/// a defect, and the driver presses the control named Pay when the screen has one.
struct Fake;

impl Models for Fake {
    fn plan(&self, _req: &PlanRequest) -> Result<Vec<Scenario>, String> {
        Ok(vec![scenario()])
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
        Ok(
            match req.options.iter().find(|(_, d)| d.contains("\"Pay\"")) {
                Some((id, _)) => Pick {
                    action: Some(Action::Click {
                        element: id.clone(),
                    }),
                    confidence: 0.9,
                    why: "Pay is the button".into(),
                },
                None => Pick {
                    action: None,
                    confidence: 0.9,
                    why: "no control on this screen serves the goal".into(),
                },
            },
        )
    }

    fn look(&self, _question: &str, _shot: &Screenshot) -> Result<Look, String> {
        Ok(Look {
            holds: true,
            reason: "the fake checker".into(),
        })
    }
}

/// Pressing Pay reaches the receipt.
fn scenario() -> Scenario {
    Scenario {
        id: "pay".into(),
        title: "Paying reaches the receipt".into(),
        origin: Origin::Acceptance,
        symbol: "shop".into(),
        steps: vec![Step {
            goal: "press Pay".into(),
            expect: vec![Assertion::Present("Receipt".into())],
            actions: None,
        }],
        assertions: vec![],
        checkpoints: vec![],
        timings: vec![],
    }
}

/// One scenario on the targets `open` makes (a fresh one for each run), through the loop.
fn run<'a>(open: &mut dyn FnMut() -> Result<Box<dyn Target + 'a>, String>) -> Execution {
    let dir = tempfile::tempdir().unwrap();
    let blobs = Blobs::open(dir.path()).unwrap();
    let models = Fake;
    let ctx = Ctx {
        models: &models,
        blobs: &blobs,
        threshold: 0.6,
        actions: 4,
    };
    let input = Input {
        symbol: "shop",
        acceptance: &[],
        regressions: &[],
        exploratory: 0,
    };
    execute(&ctx, &input, open).expect("the scenario runs on the target")
}

/// The calls a test answers from a closure, and what they were asked.
struct Answers<F> {
    log: RefCell<Vec<(String, Value)>>,
    answer: F,
}

impl<F: Fn(&str, &Value) -> Result<Value, String>> Calls for Answers<F> {
    fn call(&self, tool: &str, input: &Value) -> Result<Value, String> {
        self.log
            .borrow_mut()
            .push((tool.to_string(), input.clone()));
        (self.answer)(tool, input)
    }
}

fn answers<F: Fn(&str, &Value) -> Result<Value, String>>(answer: F) -> Answers<F> {
    Answers {
        log: RefCell::new(Vec::new()),
        answer,
    }
}

fn tools(log: &RefCell<Vec<(String, Value)>>) -> Vec<String> {
    log.borrow().iter().map(|(tool, _)| tool.clone()).collect()
}

/// A control or a window of the station's tree.
fn node(role: &str, name: Option<&str>, actions: &[&str], children: Vec<Value>) -> Value {
    json!({
        "id": 0,
        "role": role,
        "name": name,
        "value": null,
        "bounds": [0, 0, 0, 0],
        "actions": actions,
        "children": children,
    })
}

/// A small shop app on the station: the cart's Pay goes to the receipt, or, when `buggy`, to an error screen.
struct Shop {
    buggy: bool,
    screen: RefCell<&'static str>,
}

impl Shop {
    fn new(buggy: bool) -> Self {
        Self {
            buggy,
            screen: RefCell::new("cart"),
        }
    }

    /// The app's tree on the screen now: the application, with its one window.
    fn tree(&self) -> Value {
        let screen = *self.screen.borrow();
        let window = match screen {
            "receipt" => node(
                "window",
                Some("Receipt"),
                &[],
                vec![
                    node("static_text", Some("Receipt"), &[], vec![]),
                    node("button", Some("Back to shop"), &["invoke"], vec![]),
                ],
            ),
            "error" => node(
                "window",
                Some("Error"),
                &[],
                vec![
                    node("static_text", Some("Something went wrong"), &[], vec![]),
                    node("button", Some("Retry"), &["invoke"], vec![]),
                ],
            ),
            _ => node(
                "window",
                Some("Cart"),
                &[],
                vec![
                    node("button", Some("Pay"), &["invoke"], vec![]),
                    node("text_field", Some("Note"), &["set_value"], vec![]),
                ],
            ),
        };
        node("application", Some("Shop"), &[], vec![window])
    }

    /// The control a selector path names on the current tree, as the station checks it: the path's role and name must match.
    fn ui_act(&self, input: &Value) -> Result<Value, String> {
        let selector = &input["selector"];
        let path: Vec<u64> = selector["path"]
            .as_array()
            .ok_or("the selector has no path")?
            .iter()
            .filter_map(Value::as_u64)
            .collect();
        let tree = self.tree();
        let mut at = &tree;
        for index in &path {
            at = at["children"]
                .get(*index as usize)
                .ok_or("no control at that path")?;
        }
        if selector["role"].as_str() != at["role"].as_str()
            || selector["name"].as_str() != at["name"].as_str()
        {
            return Err("the selector no longer names the control at its path".into());
        }
        let act = input["act"].as_str().unwrap_or_default();
        if act == "invoke" && at["name"].as_str() == Some("Pay") && *self.screen.borrow() == "cart"
        {
            let to = if self.buggy { "error" } else { "receipt" };
            *self.screen.borrow_mut() = to;
        }
        Ok(Value::Null)
    }
}

impl Calls for Shop {
    fn call(&self, tool: &str, input: &Value) -> Result<Value, String> {
        match tool {
            "station_lease" => Ok(json!({"id": "l1", "state": "active", "session": 1})),
            "station_release" | "station_app_launch" | "station_key" => Ok(Value::Null),
            "app_script" => Ok(json!({"exit_code": 0, "output": "", "timed_out": false})),
            "station_ui_tree" => Ok(self.tree()),
            "station_screenshot" => Ok(json!({
                "w": 1,
                "h": 1,
                "format": "png",
                "data_b64": base64::engine::general_purpose::STANDARD.encode(b"\x89PNG the shop"),
            })),
            "station_ui_act" => self.ui_act(input),
            other => Err(format!("the shop has no tool {other}")),
        }
    }
}

/// The macOS target of a fresh shop, under the lease `l1`.
fn mac_target(buggy: bool) -> Box<dyn Target> {
    Box::new(MacApp::new(
        Box::new(Shop::new(buggy)),
        "w1",
        "l1".into(),
        "Shop",
        4,
    ))
}

#[test]
fn a_macos_scenario_fails_on_the_seeded_bug_and_passes_the_fixed_build() {
    let bug = run(&mut || Ok(mac_target(true)));
    assert_eq!(bug.target, "macos");
    assert_eq!(
        bug.found.len(),
        1,
        "the seeded bug is a finding: {}",
        bug.summary()
    );
    assert!(
        matches!(bug.found[0].failure, Failure::Feature { .. }),
        "{:?}",
        bug.found[0].failure
    );
    assert!(matches!(bug.found[0].ruling, Ruling::Confirmed { .. }));
    assert_eq!(bug.confirmed(), 1);
    // The evidence carries the tree and the picture of each look, and the press that changed the screen.
    assert!(bug.evidence.iter().any(|row| row.action == "click 0.0"));
    assert!(bug.evidence.iter().all(|row| !row.tree.is_empty()));

    let fixed = run(&mut || Ok(mac_target(false)));
    assert!(
        fixed.found.is_empty(),
        "the fixed build passes: {}",
        fixed.summary()
    );
    assert_eq!(fixed.confirmed(), 0);
}

#[test]
fn a_macos_click_is_an_invoke_as_the_agent_under_its_lease_and_a_typed_value_is_a_set() {
    let shop = answers(|tool: &str, input: &Value| match tool {
        "station_ui_tree" => Ok(json!(node(
            "application",
            Some("Shop"),
            &[],
            vec![node(
                "window",
                Some("Cart"),
                &[],
                vec![
                    node("button", Some("Pay"), &["invoke"], vec![]),
                    node("text_field", Some("Note"), &["set_value"], vec![]),
                ],
            )],
        ))),
        "station_ui_act" => {
            assert_eq!(input["lease"], "l1");
            assert_eq!(input["station"], "w1");
            Ok(Value::Null)
        }
        "station_release" => Ok(Value::Null),
        other => Err(format!("unexpected {other}")),
    });
    let calls: &Answers<_> = &shop;
    let mut app = MacApp::new(Box::new(calls), "w1", "l1".into(), "Shop", 4);
    let tree = app.tree().expect("the tree");
    assert_eq!(tree.title, "Cart");
    assert!(tree.shows("Pay"));
    app.act(&Action::Click {
        element: "0.0".into(),
    })
    .expect("the click invokes Pay");
    app.act(&Action::Type {
        element: "0.1".into(),
        text: "no ice".into(),
    })
    .expect("the text is set");
    let log = shop.log.borrow();
    let acts: Vec<&Value> = log
        .iter()
        .filter(|(tool, _)| tool == "station_ui_act")
        .map(|(_, input)| input)
        .collect();
    assert_eq!(acts[0]["act"], "invoke");
    assert_eq!(acts[0]["selector"]["path"], json!([0, 0]));
    assert_eq!(acts[0]["selector"]["role"], "button");
    assert_eq!(acts[1]["act"], "set_value");
    assert_eq!(acts[1]["value"], "no ice");
    assert_eq!(acts[1]["selector"]["name"], "Note");
    drop(log);
    // A control that takes no set_value is refused before the station is asked.
    let why = app
        .act(&Action::Type {
            element: "0.0".into(),
            text: "x".into(),
        })
        .unwrap_err();
    assert!(why.contains("takes no set_value"), "{why}");
    // Dropping the target releases its lease.
    drop(app);
    assert_eq!(
        tools(&shop.log).last().map(String::as_str),
        Some("station_release")
    );
}

#[test]
fn a_queued_pixel_lease_is_refused_and_released_so_no_scenario_runs_without_its_session() {
    let calls = answers(|tool: &str, _input: &Value| match tool {
        "station_lease" => Ok(json!({"id": "l9", "state": "queued", "position": 1})),
        "station_release" => Ok(Value::Null),
        other => Err(format!("unexpected {other}")),
    });
    let why = open_lease(&&calls, "w1", Some("review")).unwrap_err();
    assert!(why.contains("queued"), "{why}");
    let released = calls
        .log
        .borrow()
        .iter()
        .find(|(tool, _)| tool == "station_release")
        .map(|(_, input)| input.clone())
        .expect("the queued lease is released");
    assert_eq!(released["lease"], "l9");
    // A lease the station grants is answered as its id; releasing it is one call.
    let granted = answers(|tool: &str, _input: &Value| match tool {
        "station_lease" => Ok(json!({"id": "l2", "state": "active", "session": 3})),
        "station_release" => Ok(Value::Null),
        other => Err(format!("unexpected {other}")),
    });
    assert_eq!(open_lease(&&granted, "w1", None).unwrap(), "l2");
    release_lease(&&granted, "w1", "l2").unwrap();
}

#[test]
fn a_simctl_line_runs_on_the_node_as_one_app_script_and_a_background_start_answers_its_pid() {
    let node_shell = answers(|tool: &str, input: &Value| {
        assert_eq!(tool, "app_script");
        let script = input["script"].as_str().unwrap_or_default();
        if script.ends_with("& echo $!") {
            return Ok(json!({"exit_code": 0, "output": "4242\n", "timed_out": false}));
        }
        if script.starts_with("kill -INT") {
            return Ok(json!({"exit_code": 0, "output": "", "timed_out": false}));
        }
        if script.starts_with("'xcrun' 'simctl' 'shutdown'") {
            return Ok(json!({"exit_code": 1, "output": "Unable to shutdown", "timed_out": false}));
        }
        Ok(json!({"exit_code": 0, "output": "booted\n", "timed_out": false}))
    });
    let runner = StationRunner::over(Box::new(&node_shell), "w1");

    let out = runner
        .run(&[
            "simctl".into(),
            "bootstatus".into(),
            "booted".into(),
            "-b".into(),
        ])
        .expect("boot runs");
    assert_eq!(out, "booted\n");
    {
        let log = node_shell.log.borrow();
        let (_, input) = &log[0];
        assert_eq!(input["station"], "w1");
        assert_eq!(input["bridge"], "cli");
        assert_eq!(
            input["script"],
            "'xcrun' 'simctl' 'bootstatus' 'booted' '-b'"
        );
    }

    let pid = runner
        .start(&[
            "simctl".into(),
            "io".into(),
            "booted".into(),
            "recordVideo".into(),
            "--codec=h264".into(),
            "--force".into(),
            "/tmp/run.mov".into(),
        ])
        .expect("the recording starts");
    assert_eq!(pid, 4242);
    runner.stop(pid).expect("the recording is interrupted");

    let why = runner
        .run(&["simctl".into(), "shutdown".into(), "booted".into()])
        .unwrap_err();
    assert!(why.contains("Unable to shutdown"), "{why}");
}

#[test]
fn a_simulator_scenario_runs_through_the_bridge_and_fails_the_seeded_bug() {
    /// A bridge over a stand-in app: the simulator's tree and taps come from the script of the app's screens.
    struct Scripted(Stand);

    impl Bridge for Scripted {
        fn tree(&mut self) -> Result<Tree, String> {
            Target::tree(&mut self.0)
        }

        fn act(&mut self, action: &Action) -> Result<(), String> {
            Target::act(&mut self.0, action)
        }
    }

    fn app(buggy: bool) -> Box<dyn Target> {
        let to = if buggy { "error" } else { "receipt" };
        let script = StandIn {
            start: "cart".into(),
            latency_ms: 0,
            screens: vec![
                crate::user_test::target::Screen {
                    id: "cart".into(),
                    title: "Cart".into(),
                    elements: vec![crate::user_test::target::Element {
                        id: "pay".into(),
                        role: "button".into(),
                        label: "Pay".into(),
                        value: String::new(),
                    }],
                    on: vec![crate::user_test::target::Link {
                        element: "pay".into(),
                        to: to.into(),
                    }],
                },
                crate::user_test::target::Screen {
                    id: "receipt".into(),
                    title: "Receipt".into(),
                    elements: vec![crate::user_test::target::Element {
                        id: "home".into(),
                        role: "link".into(),
                        label: "Back to shop".into(),
                        value: String::new(),
                    }],
                    on: vec![],
                },
                crate::user_test::target::Screen {
                    id: "error".into(),
                    title: "Error".into(),
                    elements: vec![crate::user_test::target::Element {
                        id: "retry".into(),
                        role: "button".into(),
                        label: "Retry".into(),
                        value: String::new(),
                    }],
                    on: vec![],
                },
            ],
        };
        // The simulator's runner is the fake recorder below; the app's screens are the bridge's.
        Box::new(
            Simulator::launch(
                Box::new(Recorder::default()),
                Box::new(Scripted(script.open())),
                "booted",
                "dev.example.shop",
            )
            .expect("the app launches"),
        )
    }

    let bug = run(&mut || Ok(app(true)));
    assert_eq!(bug.target, "simulator");
    assert_eq!(bug.found.len(), 1, "{}", bug.summary());
    assert!(matches!(bug.found[0].ruling, Ruling::Confirmed { .. }));
    let fixed = run(&mut || Ok(app(false)));
    assert!(fixed.found.is_empty(), "{}", fixed.summary());
}

/// A simctl runner that answers from nothing: it records each line, answers a screenshot with a PNG, and starts a recording
/// as process 7.
#[derive(Default)]
struct Recorder {
    lines: RefCell<Vec<Vec<String>>>,
    stopped: RefCell<Vec<u32>>,
}

impl Runner for Recorder {
    fn run(&self, args: &[String]) -> Result<String, String> {
        self.lines.borrow_mut().push(args.to_vec());
        Ok(String::new())
    }

    fn start(&self, args: &[String]) -> Result<u32, String> {
        self.lines.borrow_mut().push(args.to_vec());
        Ok(7)
    }

    fn stop(&self, pid: u32) -> Result<(), String> {
        self.stopped.borrow_mut().push(pid);
        Ok(())
    }

    fn fetch(&self, _path: &Path) -> Result<Vec<u8>, String> {
        Ok(b"\x89PNG the simulator".to_vec())
    }
}

#[test]
fn a_simulator_without_its_bridge_says_so_and_a_recording_stops_with_the_target() {
    let unwired = Simulator::launch(
        Box::new(Recorder::default()),
        Box::new(Unwired),
        "booted",
        "dev.example.shop",
    )
    .expect("the app launches");
    let mut unwired = unwired;
    assert_eq!(unwired.tree().unwrap_err(), UNWIRED);
    assert_eq!(
        unwired
            .act(&Action::Click {
                element: "pay".into()
            })
            .unwrap_err(),
        UNWIRED
    );
    // The launch is a fresh start, and a screenshot is a simctl io line and its file, read back.
    let shot = unwired.screenshot().expect("the screenshot is read");
    assert_eq!(shot.mime, "image/png");
    assert!(shot.bytes.starts_with(b"\x89PNG"));

    let recorder = Recorder::default();
    let mut recorded = Simulator::launch(
        Box::new(recorder),
        Box::new(Unwired),
        "booted",
        "dev.example.shop",
    )
    .unwrap();
    recorded
        .record("/tmp/run.mov")
        .expect("the recording starts");
    assert!(
        recorded.record("/tmp/again.mov").is_err(),
        "one recording at a time"
    );
    assert_eq!(
        recorded.stop_recording().unwrap(),
        Some("/tmp/run.mov".into())
    );
    assert_eq!(recorded.stop_recording().unwrap(), None);
    recorded.record("/tmp/run2.mov").unwrap();
    // Dropping the target stops a recording it still holds.
    drop(recorded);
}

#[test]
fn the_simctl_lines_a_simulator_target_sends_are_the_launch_and_the_recording() {
    let runner = Recorder::default();
    exec(
        &runner,
        &Simctl::Launch {
            device: "booted".into(),
            bundle: "dev.example.shop".into(),
            args: vec![],
        },
    )
    .unwrap();
    assert_eq!(
        runner.lines.borrow()[0],
        [
            "simctl",
            "launch",
            "--terminate-running-process",
            "booted",
            "dev.example.shop"
        ]
    );
}

fn mac(agent: u64) -> MacTarget {
    MacTarget {
        station: "w1".into(),
        agent,
        app: "Shop".into(),
        task: None,
        depth: 4,
    }
}

fn sim(agent: u64) -> SimTarget {
    SimTarget {
        station: "w1".into(),
        agent,
        device: "booted".into(),
        bundle: "dev.example.shop".into(),
        app: None,
        video: None,
    }
}

#[test]
fn a_desktop_target_acts_as_an_agent_and_names_one_target() {
    let base = UserTest {
        planner: "mock/script".into(),
        driver: "mock/script".into(),
        escalate: "mock/script".into(),
        checker: "mock/script".into(),
        stand_in: None,
        web: None,
        tui: None,
        macos: None,
        simulator: None,
        acceptance: vec![],
        threshold: 0.6,
        actions: 8,
        exploratory: 0,
    };
    let owner = UserTest {
        macos: Some(mac(0)),
        ..base.clone()
    };
    assert!(
        owner
            .check()
            .unwrap_err()
            .contains("never the owner's client")
    );
    let both = UserTest {
        macos: Some(mac(5)),
        simulator: Some(sim(5)),
        ..base.clone()
    };
    assert!(both.check().unwrap_err().contains("one target"));
    let relative = UserTest {
        simulator: Some(SimTarget {
            app: Some("build/Shop.app".into()),
            ..sim(5)
        }),
        ..base.clone()
    };
    assert!(relative.check().unwrap_err().contains("absolute path"));
    let good = UserTest {
        macos: Some(mac(5)),
        ..base.clone()
    };
    assert!(good.check().is_ok());
    let deep = UserTest {
        macos: Some(MacTarget { depth: 9, ..mac(5) }),
        ..base
    };
    assert!(deep.check().unwrap_err().contains("levels"));
}
