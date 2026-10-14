//! Live tests against a real Xvfb: a private display, a fake X client that records what it receives, capture,
//! input, launch, and close. They skip with a message when `Xvfb` is not on PATH (run with `--nocapture` to see
//! it). They never touch the owner's display: every session is a pixel session on its own display number, and
//! the test's own connections name that display explicitly. No environment variable is changed.

#![cfg(target_os = "linux")]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use apz_desktop::{Desktop, Frame, SessionSpec};
use apz_desktop_x11::{X11Config, X11Desktop};
use apz_station::{Button, InputAction, SessionKind};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    ConnectionExt as _, CreateWindowAux, EventMask, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::{COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT};

const GREEN: [u8; 4] = [0, 255, 0, 255];
const WAIT: Duration = Duration::from_secs(10);

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// A fake X client: a green window at (100, 100) that records the presses it receives.
struct Probe {
    conn: Arc<RustConnection>,
    root: Window,
    events: Arc<Mutex<Vec<Event>>>,
}

fn open_probe(display: &str) -> Probe {
    let (conn, screen) = RustConnection::connect(Some(display)).expect("the probe connects");
    let conn = Arc::new(conn);
    let root = conn.setup().roots[screen].root;
    let window = conn.generate_id().expect("a window id");
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        window,
        root,
        100,
        100,
        200,
        200,
        0,
        WindowClass::INPUT_OUTPUT,
        COPY_FROM_PARENT,
        &CreateWindowAux::new()
            .background_pixel(0x0000_ff00)
            .event_mask(EventMask::BUTTON_PRESS | EventMask::KEY_PRESS | EventMask::EXPOSURE),
    )
    .expect("the window is requested")
    .check()
    .expect("the window is created");
    conn.map_window(window)
        .expect("the map is requested")
        .check()
        .expect("the window is mapped");
    let events = Arc::new(Mutex::new(Vec::new()));
    let reader = Arc::clone(&conn);
    let sink = Arc::clone(&events);
    thread::spawn(move || {
        while let Ok(event) = reader.wait_for_event() {
            sink.lock().expect("the event list").push(event);
        }
    });
    Probe { conn, root, events }
}

impl Probe {
    /// The keysym of the first column of `keycode`, from the server's own map.
    fn keysym_of(&self, keycode: u8) -> u32 {
        let setup = self.conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let reply = self
            .conn
            .get_keyboard_mapping(min, max - min + 1)
            .expect("the map is requested")
            .reply()
            .expect("the map is read");
        let per = usize::from(reply.keysyms_per_keycode);
        reply.keysyms[usize::from(keycode - min) * per]
    }

    /// Whether any key on the server's map makes `keysym`.
    fn map_has(&self, keysym: u32) -> bool {
        let setup = self.conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let reply = self
            .conn
            .get_keyboard_mapping(min, max - min + 1)
            .expect("the map is requested")
            .reply()
            .expect("the map is read");
        reply.keysyms.contains(&keysym)
    }

    fn root_children(&self) -> usize {
        self.conn
            .query_tree(self.root)
            .expect("the tree is requested")
            .reply()
            .expect("the tree is read")
            .children
            .len()
    }

    fn recorded(&self) -> Vec<Event> {
        self.events.lock().expect("the event list").clone()
    }
}

fn pixel(frame: &Frame, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * frame.w + x) * 4) as usize;
    [
        frame.rgba[at],
        frame.rgba[at + 1],
        frame.rgba[at + 2],
        frame.rgba[at + 3],
    ]
}

/// Captures until `ready` accepts a frame, or fails after the wait.
fn capture_until(desktop: &mut X11Desktop, id: u64, ready: impl Fn(&Frame) -> bool) -> Frame {
    let deadline = Instant::now() + WAIT;
    loop {
        let frame = desktop.capture(id).expect("a frame");
        if ready(&frame) {
            return frame;
        }
        assert!(
            Instant::now() < deadline,
            "the frame never showed the window"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(50));
    }
}

fn scratch_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("apz-x11-live-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

#[test]
fn a_private_display_captures_takes_input_launches_and_closes() {
    if !on_path("Xvfb") {
        eprintln!("Xvfb not installed; skipped");
        return;
    }
    let dir = scratch_dir();
    let apps = dir.join("applications");
    fs::create_dir_all(&apps).expect("an applications directory");
    let has_xterm = on_path("xterm");
    if has_xterm {
        fs::write(
            apps.join("test-terminal.desktop"),
            "[Desktop Entry]\nType=Application\nName=Test Terminal\nExec=xterm\n",
        )
        .expect("a desktop entry");
    }

    let config = X11Config {
        application_dirs: Some(vec![apps]),
        ..X11Config::default()
    };
    let mut desktop = X11Desktop::new(config);
    let id = desktop
        .open_session(&SessionSpec {
            agent: 1,
            kind: SessionKind::Pixel,
            width: 640,
            height: 480,
        })
        .expect("a pixel session opens");
    let display = desktop.display_of(id).expect("the display").to_string();
    let number: u32 = display
        .strip_prefix(':')
        .and_then(|n| n.parse().ok())
        .expect("a display number");
    assert!(number >= 90, "a private display, not the system's");
    if let Ok(owner) = std::env::var("DISPLAY") {
        assert_ne!(display, owner, "the owner's display is never used");
    }
    let server_pid = desktop.server_pid(id).expect("the server's pid");
    let socket = PathBuf::from("/tmp/.X11-unix").join(format!("X{number}"));

    let probe = open_probe(&display);
    let frame = capture_until(&mut desktop, id, |frame| pixel(frame, 200, 200) == GREEN);
    assert_eq!(
        (frame.w, frame.h),
        (640, 480),
        "the frame is the session's size"
    );

    // The window is at (100, 100): a click at (150, 150) is at (50, 50) inside it.
    desktop
        .input(
            id,
            &InputAction::Click {
                x: 150,
                y: 150,
                button: Button::Left,
                count: 1,
            },
        )
        .expect("the click is injected");
    desktop
        .input(id, &InputAction::Type { text: "a".into() })
        .expect("the text is typed");
    wait_for("a button press and a key press", || {
        let events = probe.recorded();
        events.iter().any(|e| matches!(e, Event::ButtonPress(_)))
            && events.iter().any(|e| matches!(e, Event::KeyPress(_)))
    });
    let events = probe.recorded();
    let press = events
        .iter()
        .find_map(|e| match e {
            Event::ButtonPress(press) => Some(press),
            _ => None,
        })
        .expect("a button press");
    assert_eq!(press.detail, 1, "the left button");
    assert_eq!(
        (press.event_x, press.event_y),
        (50, 50),
        "the click's place in the window"
    );
    let key = events
        .iter()
        .find_map(|e| match e {
            Event::KeyPress(key) => Some(key),
            _ => None,
        })
        .expect("a key press");
    assert_eq!(
        probe.keysym_of(key.detail),
        u32::from(b'a'),
        "the typed letter"
    );

    // A character the map lacks is typed through a scratch remap, and the map is restored afterwards.
    desktop
        .input(id, &InputAction::Type { text: "é".into() })
        .expect("the accented letter is typed");
    assert!(
        !probe.map_has(0xe9),
        "the scratch keycode is cleared after the remap"
    );

    // An app from the test's own entry runs on the private display.
    if has_xterm {
        let before = probe.root_children();
        desktop
            .launch(id, "Test Terminal", &[])
            .expect("the terminal starts");
        wait_for("the terminal's window", || probe.root_children() > before);
    }

    // Closing kills the server and everything on it.
    drop(probe);
    desktop.close_session(id).expect("the session closes");
    assert!(
        !Path::new(&format!("/proc/{server_pid}")).exists(),
        "the server is gone"
    );
    assert!(!socket.exists(), "the server's socket is gone");
    let _ = fs::remove_dir_all(&dir);
}
