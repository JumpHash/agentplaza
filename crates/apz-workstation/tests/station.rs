//! The daemon against a loopback fake hub: the station link, real WebSocket frames, the lanes and STOP ALL.
//!
//! The hub is an `apz_http` server that accepts `WS /v1/stations/link` with the station's bearer token and hands
//! each accepted socket to the test. The desktop is [`FakeDesktop`] behind a recorder, so a test can check what
//! the daemon asked the desktop to do. Every file lives in a temporary directory.

use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use apz_desktop::{
    AppInfo, Desktop, FakeDesktop, Frame, Result as DesktopResult, SessionSpec, UiNode,
};
use apz_http::ws::{self, Limits, Message, Origins, Socket};
use apz_http::{Handle, Request, Response, Server};
use apz_station::{
    AccountMode, AppsOp, Button, Display, Event, Facts, Form, HubFrame, ImageFormat, InputAction,
    LINK_PATH, Notice, Op, Os, SUBPROTOCOL, Selector, SessionId, SessionKind, StationFrame,
    StationSettings, UiAction,
};
#[cfg(unix)]
use apz_station::{Bridge, FilesOp, RunOp, TermOp};
use apz_workstation::backend::Backend;
use apz_workstation::daemon::{Daemon, DaemonConfig, Gather};
use apz_workstation::link;
use apz_workstation::log::Log;
use apz_workstation::state::{self, Record};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;

const TOKEN: &str = "station-token-for-tests";
const STATION: &str = "ws-7";

// ---- the fake desktop ----------------------------------------------------------------------------------------

/// Forwards to a [`FakeDesktop`] and records what it was asked to do.
struct Recorder {
    inner: FakeDesktop,
    inputs: Arc<Mutex<Vec<InputAction>>>,
    owner: Arc<AtomicBool>,
}

impl Desktop for Recorder {
    fn open_session(&mut self, spec: &SessionSpec) -> DesktopResult<SessionId> {
        self.inner.open_session(spec)
    }
    fn close_session(&mut self, id: SessionId) -> DesktopResult<()> {
        self.inner.close_session(id)
    }
    fn capture(&mut self, id: SessionId) -> DesktopResult<Frame> {
        self.inner.capture(id)
    }
    fn input(&mut self, id: SessionId, action: &InputAction) -> DesktopResult<()> {
        self.inputs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(action.clone());
        self.inner.input(id, action)
    }
    fn ui_tree(&mut self, id: SessionId, app: Option<&str>, depth: u8) -> DesktopResult<UiNode> {
        self.inner.ui_tree(id, app, depth)
    }
    fn ui_act(
        &mut self,
        id: SessionId,
        selector: &Selector,
        action: &UiAction,
    ) -> DesktopResult<()> {
        self.inner.ui_act(id, selector, action)
    }
    fn apps(&mut self) -> DesktopResult<Vec<AppInfo>> {
        self.inner.apps()
    }
    fn launch(&mut self, id: SessionId, app: &str, args: &[String]) -> DesktopResult<()> {
        self.inner.launch(id, app, args)
    }
    fn owner_active(&self, _id: SessionId) -> bool {
        self.owner.load(Ordering::SeqCst)
    }
}

// ---- the fake hub --------------------------------------------------------------------------------------------

/// A loopback hub that accepts station links and hands each accepted socket to the test.
struct Hub {
    port: u16,
    sockets: Receiver<Socket>,
    _server: Handle,
}

fn start_hub() -> Hub {
    let (tx, sockets) = channel::<Socket>();
    let tx = Mutex::new(tx);
    let server = Server::bind(0)
        .expect("a loopback port")
        .spawn(move |req: &Request| {
            if req.path != LINK_PATH {
                return Response::error(404, "no such path");
            }
            let bearer = format!("Bearer {TOKEN}");
            if req.header("authorization") != Some(bearer.as_str()) {
                return Response::error(401, "not this station");
            }
            let tx = tx.lock().unwrap_or_else(|e| e.into_inner()).clone();
            ws::accept(
                req,
                Origins::None,
                Some(SUBPROTOCOL),
                Limits::default(),
                move |socket| {
                    let _ = tx.send(socket);
                },
            )
        })
        .expect("the hub serves");
    Hub {
        port: server.port(),
        sockets,
        _server: server,
    }
}

/// One station link as the hub sees it: frames in, requests out. Events that arrive while the test waits for
/// an answer are kept for later.
struct Link {
    socket: Socket,
    events: VecDeque<Event>,
}

/// An answer to one request.
#[derive(Debug)]
struct Answer {
    ok: bool,
    result: Option<Value>,
    error: Option<String>,
}

impl Link {
    fn new(socket: Socket) -> Self {
        Link {
            socket,
            events: VecDeque::new(),
        }
    }

    fn send(&mut self, frame: &HubFrame) {
        let text = serde_json::to_string(frame).expect("encodes");
        self.socket
            .send(&Message::Text(text))
            .expect("the daemon's link takes a frame");
    }

    /// The next station frame, or `None` when nothing came within `wait`.
    fn pull(&mut self, wait: Duration) -> Option<StationFrame> {
        let until = Instant::now() + wait;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match self.socket.recv(left.max(Duration::from_millis(1))) {
                Ok(Some(Message::Text(text))) => {
                    return Some(
                        serde_json::from_str(&text).expect("the daemon sends station frames"),
                    );
                }
                Ok(Some(Message::Binary(_))) => panic!("the daemon sent a binary frame"),
                Ok(None) => {
                    if Instant::now() >= until {
                        return None;
                    }
                }
                Err(err) => panic!("the link broke: {err}"),
            }
        }
    }

    /// The next frame, which must come within 10 seconds.
    fn next(&mut self) -> StationFrame {
        self.pull(Duration::from_secs(10))
            .expect("a frame within 10 seconds")
    }

    /// The next answer to any request, with the id it answers.
    fn next_answer(&mut self) -> (u64, Answer) {
        loop {
            match self.next() {
                StationFrame::Response {
                    id,
                    ok,
                    result,
                    error,
                } => return (id, Answer { ok, result, error }),
                StationFrame::Event { event } => self.events.push_back(event),
                other => panic!("unexpected frame {other:?}"),
            }
        }
    }

    /// Sends a request and waits for its answer. Other answers that arrive first are an error.
    fn request(&mut self, id: u64, op: Op) -> Answer {
        self.send(&HubFrame::Request { id, op });
        let (answered, answer) = self.next_answer();
        assert_eq!(answered, id, "an answer came out of order");
        answer
    }

    /// The first event that matches `wanted`, from those already kept or from the next frames.
    fn event(&mut self, wanted: impl Fn(&Event) -> bool) -> Event {
        if let Some(pos) = self.events.iter().position(&wanted) {
            return self.events.remove(pos).expect("the position is in range");
        }
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            match self.pull(Duration::from_millis(200)) {
                Some(StationFrame::Event { event }) => {
                    if wanted(&event) {
                        return event;
                    }
                    self.events.push_back(event);
                }
                Some(StationFrame::Response { .. }) | None => {}
                Some(other) => panic!("unexpected frame {other:?}"),
            }
        }
        panic!("no matching event within 10 seconds");
    }
}

fn fixture_facts(apps: u32) -> Facts {
    Facts {
        os: Os::Linux,
        os_version: "fixture".into(),
        arch: "x86_64".into(),
        hostname: "fixture-box".into(),
        form: Form::Desktop,
        cpus: 4,
        ram_mb: 8192,
        gpus: Vec::new(),
        displays: vec![Display {
            id: 1,
            w: 1280,
            h: 800,
            scale: 1.0,
        }],
        account: AccountMode::AgentUser {
            name: "agentws".into(),
        },
        apps,
        virtual_displays: true,
        ui_tree: false,
        machine_id: None,
    }
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("apz-ws-it-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a scratch dir");
    dir
}

/// A running station: its state, its work folder, its hub, and the link thread.
struct Rig {
    hub: Hub,
    #[cfg_attr(not(unix), allow(dead_code))]
    work: PathBuf,
    inputs: Arc<Mutex<Vec<InputAction>>>,
    owner: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<(), String>>>,
}

impl Rig {
    fn start(tag: &str) -> Rig {
        let root = scratch(tag);
        let state_dir = root.join("state");
        let work = root.join("work");
        let home = root.join("home");
        fs::create_dir_all(&home).expect("a home");
        state::save_settings(
            &state_dir,
            &StationSettings {
                work_folder: work.to_string_lossy().into_owned(),
                ..StationSettings::default()
            },
        )
        .expect("settings save");

        let hub = start_hub();
        let record = Record {
            hub: format!("http://127.0.0.1:{}", hub.port),
            pin: None,
            station: STATION.into(),
            token: TOKEN.into(),
            name: "fixture".into(),
            account: AccountMode::AgentUser {
                name: "agentws".into(),
            },
        };
        state::save_record(&state_dir, &record).expect("record save");

        let inputs = Arc::new(Mutex::new(Vec::new()));
        let owner = Arc::new(AtomicBool::new(false));
        let desktop = Recorder {
            inner: FakeDesktop::new(),
            inputs: inputs.clone(),
            owner: owner.clone(),
        };
        let gather: Gather = Arc::new(fixture_facts);
        let daemon = Daemon::start(
            Box::new(desktop),
            DaemonConfig {
                state_dir: state_dir.clone(),
                env: apz_ops::Env::new(home, "/usr/local/bin:/usr/bin:/bin"),
                gather,
                log: Log::open(&state_dir),
                backend: Backend::Fake,
            },
        )
        .expect("the daemon starts");

        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = stop.clone();
            thread::spawn(move || link::run(&daemon, &record, &stop))
        };
        Rig {
            hub,
            work,
            inputs,
            owner,
            stop,
            thread: Some(thread),
        }
    }

    /// The next link the daemon dials.
    fn connect(&self) -> Link {
        Link::new(
            self.hub
                .sockets
                .recv_timeout(Duration::from_secs(10))
                .expect("the daemon dials the hub"),
        )
    }

    /// Ends the daemon and returns what its link thread said.
    fn finish(mut self) -> Result<(), String> {
        self.stop.store(true, Ordering::SeqCst);
        self.thread
            .take()
            .expect("the thread")
            .join()
            .expect("the link thread did not panic")
    }
}

fn assert_hello(frame: StationFrame) {
    match frame {
        StationFrame::Hello {
            station,
            version: _,
            facts,
        } => {
            assert_eq!(station, STATION);
            assert_eq!(facts, fixture_facts(0), "apps stay 0 with no real backend");
        }
        other => panic!("expected hello, got {other:?}"),
    }
}

fn ok(answer: &Answer) -> Value {
    assert!(answer.ok, "refused: {:?}", answer.error);
    answer.result.clone().unwrap_or(Value::Null)
}

fn refused(answer: &Answer) -> String {
    assert!(!answer.ok, "expected a refusal, got {:?}", answer.result);
    answer.error.clone().expect("a refusal says why")
}

// ---- tests ---------------------------------------------------------------------------------------------------

#[test]
fn hello_comes_first_and_requests_are_answered_by_their_id() {
    let rig = Rig::start("hello");
    let mut link = rig.connect();
    assert_hello(link.next());

    let answer = link.request(41, Op::Facts);
    let facts: Facts = serde_json::from_value(ok(&answer)).expect("a facts object");
    assert_eq!(facts, fixture_facts(0));

    let refusal = link.request(
        42,
        Op::Apps(AppsOp::Focus {
            app: "Notes".into(),
        }),
    );
    // The fake desktop has no focus or quit, so the lane asks it and passes its refusal on.
    assert!(refused(&refusal).contains("cannot focus"));
    let quit = link.request(
        43,
        Op::Apps(AppsOp::Quit {
            app: "Notes".into(),
            force: true,
        }),
    );
    assert!(refused(&quit).contains("cannot quit"));
    assert!(rig.finish().is_ok());
}

#[test]
#[cfg(unix)]
fn an_unknown_bridge_is_refused_and_a_slow_job_does_not_hold_up_the_others() {
    let rig = Rig::start("concurrent");
    let mut link = rig.connect();
    assert_hello(link.next());

    let blender = link.request(
        1,
        Op::Script {
            bridge: Bridge::AppScripting,
            app: Some("Blender".into()),
            code: "import bpy".into(),
            timeout_s: 30,
        },
    );
    assert!(refused(&blender).contains("app_scripting"));

    // A two-second script goes first; a file listing sent after it answers before it.
    link.send(&HubFrame::Request {
        id: 50,
        op: Op::Script {
            bridge: Bridge::Cli,
            app: None,
            code: "sleep 2; echo slow".into(),
            timeout_s: 30,
        },
    });
    link.send(&HubFrame::Request {
        id: 51,
        op: Op::Files(FilesOp::List {
            path: ".".into(),
            cursor: None,
        }),
    });
    let (first, _) = link.next_answer();
    assert_eq!(first, 51, "the listing answers while the script runs");
    let (second, slow) = link.next_answer();
    assert_eq!(second, 50);
    let slow = ok(&slow);
    assert_eq!(slow["exit_code"], 0);
    assert!(slow["output"].as_str().unwrap_or_default().contains("slow"));
    assert_eq!(slow["timed_out"], false);
    assert!(rig.finish().is_ok());
}

#[test]
#[cfg(unix)]
fn files_runs_and_terminals_work_in_the_work_folder_and_settings_move_it() {
    let rig = Rig::start("work");
    let mut link = rig.connect();
    assert_hello(link.next());

    let written = link.request(
        1,
        Op::Files(FilesOp::Write {
            path: "notes/a.txt".into(),
            text: "hello world\n".into(),
            expected_hash: None,
        }),
    );
    assert!(ok(&written)["hash"].is_string());
    assert!(
        rig.work.join("notes/a.txt").is_file(),
        "the file is in the work folder"
    );

    let read = link.request(
        2,
        Op::Files(FilesOp::Read {
            path: "notes/a.txt".into(),
            offset: None,
        }),
    );
    assert_eq!(ok(&read)["text"], "hello world\n");

    let found = link.request(
        3,
        Op::Files(FilesOp::Search {
            q: "hello".into(),
            glob: None,
            max: None,
        }),
    );
    assert!(ok(&found).to_string().contains("notes/a.txt"));

    let escape = link.request(
        4,
        Op::Files(FilesOp::Read {
            path: "../escape.txt".into(),
            offset: None,
        }),
    );
    refused(&escape);

    let run = link.request(
        5,
        Op::Run(RunOp::Start {
            command: "echo hi > out.txt; cat out.txt".into(),
            timeout_s: Some(30),
        }),
    );
    let run_id = ok(&run)["run"].as_str().expect("a run id").to_string();
    let mut output = String::new();
    let mut offset = 0u64;
    for id in 6..200 {
        let poll = link.request(
            id,
            Op::Run(RunOp::Poll {
                run: run_id.clone(),
                offset: Some(offset),
            }),
        );
        let poll = ok(&poll);
        output.push_str(poll["output"].as_str().unwrap_or_default());
        offset = poll["next"].as_u64().unwrap_or(offset);
        if poll["done"] == true {
            assert_eq!(poll["exit_code"], 0);
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(output.contains("hi"), "the run printed: {output:?}");
    assert!(rig.work.join("out.txt").is_file());

    let opened = link.request(
        300,
        Op::Term(TermOp::Open {
            cols: 80,
            rows: 24,
            cwd: None,
        }),
    );
    let term = ok(&opened)["term"]
        .as_str()
        .expect("a terminal")
        .to_string();
    let typed = STANDARD.encode("echo term-ok\n");
    link.request(
        301,
        Op::Term(TermOp::Write {
            term: term.clone(),
            data_b64: typed,
        }),
    );
    let mut seen = String::new();
    for id in 302..400 {
        let read = link.request(
            id,
            Op::Term(TermOp::Read {
                term: term.clone(),
                offset: Some(0),
            }),
        );
        let read = ok(&read);
        let bytes = STANDARD
            .decode(read["data_b64"].as_str().unwrap_or_default())
            .expect("base64");
        seen.push_str(&String::from_utf8_lossy(&bytes));
        if seen.contains("term-ok") {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(seen.contains("term-ok"), "the shell echoed: {seen:?}");
    ok(&link.request(400, Op::Term(TermOp::Close { term })));

    // Settings move the work folder: the file written above is no longer in it.
    let other = rig.work.with_file_name("moved");
    let settings = StationSettings {
        work_folder: other.to_string_lossy().into_owned(),
        ..StationSettings::default()
    };
    ok(&link.request(401, Op::Settings(settings)));
    assert!(other.is_dir(), "the new folder was created");
    let listing = link.request(
        402,
        Op::Files(FilesOp::List {
            path: ".".into(),
            cursor: None,
        }),
    );
    assert_eq!(ok(&listing)["entries"], serde_json::json!([]));
    assert!(rig.finish().is_ok());
}

#[test]
fn screenshots_and_input_map_through_the_scale() {
    let rig = Rig::start("screen");
    let mut link = rig.connect();
    assert_hello(link.next());

    let opened = link.request(
        1,
        Op::SessionOpen {
            agent: 3,
            kind: SessionKind::Pixel,
        },
    );
    let session = ok(&opened)["session"].as_u64().expect("a session");

    let shot = link.request(
        2,
        Op::Screenshot {
            session,
            scale: 0.5,
            grid: false,
            format: ImageFormat::Png,
        },
    );
    let shot = ok(&shot);
    assert_eq!(
        (shot["w"].as_u64(), shot["h"].as_u64()),
        (Some(640), Some(400))
    );
    assert_eq!(shot["format"], "png");
    let png = STANDARD
        .decode(shot["data_b64"].as_str().unwrap_or_default())
        .expect("base64");
    assert_eq!(&png[..4], &[0x89, b'P', b'N', b'G'], "a PNG");

    let gridded = link.request(
        3,
        Op::Screenshot {
            session,
            scale: 1.0,
            grid: true,
            format: ImageFormat::Jpeg,
        },
    );
    let gridded = ok(&gridded);
    assert_eq!(
        (gridded["w"].as_u64(), gridded["h"].as_u64()),
        (Some(1280), Some(800))
    );

    // The last image was the full-size one, so a click at (200, 100) is at (200, 100) of the screen.
    ok(&link.request(
        4,
        Op::Input {
            session,
            action: InputAction::Click {
                x: 200,
                y: 100,
                button: Button::Left,
                count: 1,
            },
        },
    ));
    // After a half-scale screenshot the same click lands at twice the point.
    ok(&link.request(
        5,
        Op::Screenshot {
            session,
            scale: 0.5,
            grid: false,
            format: ImageFormat::Png,
        },
    ));
    ok(&link.request(
        6,
        Op::Input {
            session,
            action: InputAction::Click {
                x: 200,
                y: 100,
                button: Button::Left,
                count: 2,
            },
        },
    ));
    ok(&link.request(
        7,
        Op::Input {
            session,
            action: InputAction::Drag {
                from: (10, 10),
                to: (20, 30),
            },
        },
    ));
    ok(&link.request(
        8,
        Op::Input {
            session,
            action: InputAction::Type { text: "hi".into() },
        },
    ));

    let seen = rig.inputs.lock().expect("the recorder").clone();
    assert_eq!(
        seen,
        vec![
            InputAction::Click {
                x: 200,
                y: 100,
                button: Button::Left,
                count: 1,
            },
            InputAction::Click {
                x: 400,
                y: 200,
                button: Button::Left,
                count: 2,
            },
            InputAction::Drag {
                from: (20, 20),
                to: (40, 60),
            },
            InputAction::Type { text: "hi".into() },
        ]
    );

    let bad = link.request(
        9,
        Op::Screenshot {
            session,
            scale: 0.0,
            grid: false,
            format: ImageFormat::Png,
        },
    );
    refused(&bad);
    let gone = link.request(
        10,
        Op::Input {
            session: 999,
            action: InputAction::Move { x: 1, y: 1 },
        },
    );
    assert!(refused(&gone).contains("no session 999"));
    assert!(rig.finish().is_ok());
}

#[test]
fn a_stream_sends_jpeg_frames_until_it_is_stopped() {
    let rig = Rig::start("stream");
    let mut link = rig.connect();
    assert_hello(link.next());
    let opened = link.request(
        1,
        Op::SessionOpen {
            agent: 3,
            kind: SessionKind::Pixel,
        },
    );
    let session = ok(&opened)["session"].as_u64().expect("a session");

    ok(&link.request(
        2,
        Op::Stream {
            session,
            fps: 10,
            on: true,
            scale: None,
        },
    ));
    let mut seqs = Vec::new();
    while seqs.len() < 3 {
        match link.event(|e| matches!(e, Event::Frame { .. })) {
            Event::Frame {
                session: s,
                seq,
                w,
                h,
                format,
                data_b64,
            } => {
                assert_eq!(s, session);
                assert_eq!((w, h), (1280, 800));
                assert_eq!(format, ImageFormat::Jpeg);
                let bytes = STANDARD.decode(data_b64).expect("base64");
                assert_eq!(&bytes[..3], &[0xFF, 0xD8, 0xFF], "a JPEG");
                seqs.push(seq);
            }
            other => panic!("not a frame: {other:?}"),
        }
    }
    assert!(
        seqs.windows(2).all(|w| w[0] < w[1]),
        "frames count up: {seqs:?}"
    );

    let too_fast = link.request(
        3,
        Op::Stream {
            session,
            fps: 60,
            on: true,
            scale: None,
        },
    );
    refused(&too_fast);
    ok(&link.request(
        4,
        Op::Stream {
            session,
            fps: 10,
            on: false,
            scale: None,
        },
    ));
    assert!(rig.finish().is_ok());
}

#[test]
fn a_stream_is_scaled_by_the_hubs_factor_before_it_is_encoded() {
    let rig = Rig::start("stream-scale");
    let mut link = rig.connect();
    assert_hello(link.next());
    let session = ok(&link.request(
        1,
        Op::SessionOpen {
            agent: 3,
            kind: SessionKind::Pixel,
        },
    ))["session"]
        .as_u64()
        .expect("a session");

    // Half scale: the 1280x800 screen comes as 640x400.
    let refused_factor = link.request(
        2,
        Op::Stream {
            session,
            fps: 10,
            on: true,
            scale: Some(1.5),
        },
    );
    refused(&refused_factor);
    ok(&link.request(
        3,
        Op::Stream {
            session,
            fps: 10,
            on: true,
            scale: Some(0.5),
        },
    ));
    match link.event(|e| matches!(e, Event::Frame { .. })) {
        Event::Frame { session: s, w, h, format, data_b64, .. } => {
            assert_eq!(s, session);
            assert_eq!((w, h), (640, 400), "scaled before it was encoded");
            assert_eq!(format, ImageFormat::Jpeg);
            let bytes = STANDARD.decode(data_b64).expect("base64");
            assert_eq!(&bytes[..3], &[0xFF, 0xD8, 0xFF], "a JPEG");
        }
        other => panic!("not a frame: {other:?}"),
    }
    ok(&link.request(
        4,
        Op::Stream {
            session,
            fps: 10,
            on: false,
            scale: None,
        },
    ));
    assert!(rig.finish().is_ok());
}

#[test]
fn a_shared_session_the_hub_opens_for_the_owners_live_view_names_no_agent_and_is_accepted() {
    let rig = Rig::start("owner-view");
    let mut link = rig.connect();
    assert_hello(link.next());
    let view = ok(&link.request(
        1,
        Op::SessionOpen {
            agent: apz_station::OWNER_VIEW,
            kind: SessionKind::Shared,
        },
    ))["session"]
        .as_u64()
        .expect("the owner's live view opens");
    ok(&link.request(
        2,
        Op::Input {
            session: view,
            action: InputAction::Move { x: 5, y: 5 },
        },
    ));
    assert_eq!(rig.inputs.lock().expect("the recorder").len(), 1, "the view is a live session");
    assert!(rig.finish().is_ok());
}

#[test]
fn freeze_stops_input_ends_pixel_sessions_and_lifts_cleanly() {
    let rig = Rig::start("freeze");
    let mut link = rig.connect();
    assert_hello(link.next());

    let shared = ok(&link.request(
        1,
        Op::SessionOpen {
            agent: 1,
            kind: SessionKind::Shared,
        },
    ))["session"]
        .as_u64()
        .expect("the shared session");
    let pixel = ok(&link.request(
        2,
        Op::SessionOpen {
            agent: 2,
            kind: SessionKind::Pixel,
        },
    ))["session"]
        .as_u64()
        .expect("the pixel session");

    ok(&link.request(3, Op::Freeze { on: true }));
    match link.event(|e| matches!(e, Event::SessionEnded { .. })) {
        Event::SessionEnded { session, reason } => {
            assert_eq!(session, pixel);
            assert_eq!(reason, "stop_all");
        }
        other => panic!("not a session end: {other:?}"),
    }

    let input = link.request(
        4,
        Op::Input {
            session: shared,
            action: InputAction::Move { x: 5, y: 5 },
        },
    );
    assert!(refused(&input).contains("STOP ALL"));
    let facts = link.request(5, Op::Facts);
    assert!(refused(&facts).contains("STOP ALL"));
    assert!(
        rig.inputs.lock().expect("the recorder").is_empty(),
        "no input reached the desktop"
    );

    ok(&link.request(6, Op::Freeze { on: false }));
    ok(&link.request(
        7,
        Op::Input {
            session: shared,
            action: InputAction::Move { x: 5, y: 5 },
        },
    ));
    assert_eq!(rig.inputs.lock().expect("the recorder").len(), 1);
    assert!(rig.finish().is_ok());
}

#[test]
fn the_owner_touching_a_shared_session_is_reported_once_per_takeover() {
    let rig = Rig::start("owner");
    let mut link = rig.connect();
    assert_hello(link.next());
    let shared = ok(&link.request(
        1,
        Op::SessionOpen {
            agent: 1,
            kind: SessionKind::Shared,
        },
    ))["session"]
        .as_u64()
        .expect("a session");

    rig.owner.store(true, Ordering::SeqCst);
    match link.event(|e| matches!(e, Event::OwnerInput { .. })) {
        Event::OwnerInput { session } => assert_eq!(session, shared),
        other => panic!("not an owner input: {other:?}"),
    }
    rig.owner.store(false, Ordering::SeqCst);
    assert!(rig.finish().is_ok());
}

#[test]
fn a_dropped_link_is_dialled_again_and_says_hello_again() {
    let rig = Rig::start("reconnect");
    let mut first = rig.connect();
    assert_hello(first.next());
    // The hub drops the link: the daemon must dial again after its backoff.
    drop(first);

    let mut second = rig.connect();
    assert_hello(second.next());
    assert!(rig.finish().is_ok());
}

#[test]
fn drain_refuses_new_requests_and_revoke_ends_the_daemon() {
    let rig = Rig::start("drain");
    let mut link = rig.connect();
    assert_hello(link.next());

    link.send(&HubFrame::Notify {
        notice: Notice::Drain,
    });
    let refusal = link.request(1, Op::Facts);
    assert!(refused(&refusal).contains("draining"));

    link.send(&HubFrame::Notify {
        notice: Notice::Revoked,
    });
    let outcome = rig.finish();
    assert!(outcome.is_err(), "a revoked station stops with an error");
    assert!(outcome.unwrap_err().contains("revoked"));
}
