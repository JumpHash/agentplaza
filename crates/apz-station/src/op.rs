//! The operations the hub asks a daemon to perform: the body of a `Request` frame.
//!
//! Argument names for `Files` and `Term` and `Run` are the IDE tools' (`ide_list`, `ide_read`,
//! `ide_write`, `ide_search`, `ide_term_*`, `ide_run_*`) so the same code serves both. Results follow the
//! IDE's output objects where the IDE has one. The examples in this module's tests are the spec for
//! every variant.
//!
//! Result shapes not named by an IDE tool:
//!
//! - `facts`: the [`crate::Facts`] object.
//! - `session_open`: `{"session": 9}`.
//! - `screenshot`: `{"w", "h", "format", "data_b64"}`, the same fields as `Event::Frame` without the session
//!   and seq.
//! - `input`, `ui_act`, `stream`, `freeze`, `session_close`, `settings`, `apps` focus/quit/launch,
//!   `sync` resolve: no result (`null`), and `ok: true`.
//! - `ui_tree`: the UI node tree, as `apz_desktop::UiNode` encodes it: `{"id", "role", "name", "value",
//!   "bounds": [x, y, w, h], "actions", "children"}`.
//! - `apps` list: an array of `{"id", "name", "path", "scripting"}`.
//! - `script`: `{"exit_code", "output", "timed_out"}`; `exit_code` is null when a signal ended it.
//! - `sync` status: `{"files", "pending"}`; manifest: `{"entries": [{"path", "hash", "size"}]}`;
//!   pull: `{"path", "hash", "data_b64"}`; push: `{"path", "hash"}` (or a conflict error).

use serde::{Deserialize, Serialize};

use crate::SessionId;
use crate::input::{InputAction, Selector, UiAction};
use crate::settings::StationSettings;

/// One operation. The tag is `op`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// The machine's facts now (the same object `hello` carried).
    Facts,
    /// The work folder's and the IDE's files: list, read, write, search.
    Files(FilesOp),
    /// Terminals: open, write, read, resize, close, list.
    Term(TermOp),
    /// Runs: a command with a timeout and a capped output, started, polled and stopped.
    Run(RunOp),
    /// Opens a screen session for an agent. A pixel session takes the screen; a shared one is the
    /// owner's own session, used with the takeover rules. A shared session the hub opens for the owner's live
    /// view names no agent: `agent` is [`OWNER_VIEW`] (0).
    SessionOpen {
        agent: u64,
        kind: SessionKind,
    },
    SessionClose {
        session: SessionId,
    },
    /// A screenshot. `scale` is in (0, 1] and the result is scaled by it; `grid` draws the coordinate grid.
    Screenshot {
        session: SessionId,
        scale: f32,
        grid: bool,
        format: ImageFormat,
    },
    /// A pointer or keyboard action.
    Input {
        session: SessionId,
        action: InputAction,
    },
    /// The UI tree of the session, or of one app's window when `app` is given, down to `depth` levels.
    UiTree {
        session: SessionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app: Option<String>,
        depth: u8,
    },
    /// An accessibility action on one control.
    UiAct {
        session: SessionId,
        selector: Selector,
        action: UiAction,
    },
    /// Applications: list, launch, focus, quit.
    Apps(AppsOp),
    /// The general adapter's scripting surface: runs `code` through one bridge, optionally for one app.
    Script {
        bridge: Bridge,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app: Option<String>,
        code: String,
        timeout_s: u32,
    },
    /// Starts or stops the live frame stream of a session at `fps`. `scale` (in (0, 1]) sizes each frame as a
    /// screenshot's is; absent, frames are at full size.
    Stream {
        session: SessionId,
        fps: u8,
        on: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scale: Option<f32>,
    },
    /// STOP ALL when `on` is true: input stops, pixel sessions end, shells and runs stop. `on: false` lifts it.
    Freeze {
        on: bool,
    },
    /// Work-folder sync: status, the manifest, pulls and pushes of content-addressed files, conflict resolution.
    Sync(SyncOp),
    /// The hub-side settings of this station, replacing the daemon's copy.
    Settings(StationSettings),
}

/// Work with files: the IDE's file tools, on the machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum FilesOp {
    /// One page of a directory (`ide_list`).
    List {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<String>,
    },
    /// A chunk of a text file from `offset` (`ide_read`).
    Read {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<u64>,
    },
    /// Writes a file atomically; `expected_hash` makes it refuse when the file changed (`ide_write`).
    Write {
        path: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_hash: Option<String>,
    },
    /// Searches text files for `q` (`ide_search`).
    Search {
        q: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        glob: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<u32>,
    },
}

/// Terminals: shells on a pseudo-terminal (`ide_term_*`). Terminal ids are `term-<n>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum TermOp {
    /// Opens a shell of `cols` by `rows`, in `cwd` (a directory of the workspace; the root by default).
    Open {
        cols: u16,
        rows: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
    /// Writes base64 bytes to the shell. `ok` is false when not all went in; send the rest from `written`.
    Write {
        term: String,
        data_b64: String,
    },
    /// The shell's output from `offset` (`ide_term_read`).
    Read {
        term: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<u64>,
    },
    Resize {
        term: String,
        cols: u16,
        rows: u16,
    },
    Close {
        term: String,
    },
    List,
}

/// Runs: a shell command in the work folder, with a timeout and output capped (`ide_run_*`). Run ids are `run-<n>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum RunOp {
    /// Starts `command`; `timeout_s` defaults to 120 and is capped at 600.
    Start {
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_s: Option<u32>,
    },
    /// The run's output from `offset` (`ide_run_poll`).
    Poll {
        run: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<u64>,
    },
    /// Kills the run's process group (`ide_run_stop`).
    Stop { run: String },
}

/// Applications on the machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum AppsOp {
    List,
    Launch {
        app: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
    },
    Focus {
        app: String,
    },
    /// Asks the app to quit. `force` ends it without a chance to save, where the platform can do that.
    Quit {
        app: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        force: bool,
    },
}

/// Work-folder sync. Files are content-addressed: `hash` is the SHA-256 of the content, in hex.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum SyncOp {
    Status,
    Manifest,
    /// Fetches the content of `path` whose hash is `hash`.
    Pull {
        path: String,
        hash: String,
    },
    /// Writes `data_b64` as `path`, whose content hash is `hash`. `base_hash` is the version the writer
    /// last saw (null for a new file); when the machine's copy has moved on, the push is a conflict.
    Push {
        path: String,
        hash: String,
        data_b64: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base_hash: Option<String>,
    },
    /// Settles a conflict: keep the hub's copy, the station's copy, or both (the other kept as a conflict copy).
    Resolve {
        path: String,
        keep: Keep,
    },
}

/// Which side of a sync conflict is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Keep {
    Hub,
    Station,
    Both,
}

/// What a session is for. `pixel` takes the screen for the agent; `shared` is the owner's own session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    Pixel,
    Shared,
}

/// The `agent` of a shared session that is the owner's live view: no agent. A shared session is the owner's own
/// screen, so the daemon accepts it with this agent and passes it on as given.
pub const OWNER_VIEW: u64 = 0;

/// An image format for screenshots and frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    Png,
    Jpeg,
}

/// A scripting bridge the general adapter can use. `AppScripting` is the app's own embedded scripting
/// (Blender's `--python`, GIMP's Script-Fu, LibreOffice macros, Office through COM).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bridge {
    Cli,
    #[serde(rename = "applescript")]
    AppleScript,
    Jxa,
    #[serde(rename = "powershell")]
    PowerShell,
    Com,
    #[serde(rename = "dbus")]
    DBus,
    AppScripting,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_wire;

    fn settings_example() -> StationSettings {
        StationSettings::default()
    }

    /// Every `Op` variant with the JSON it encodes to.
    fn op_examples() -> Vec<(Op, &'static str)> {
        vec![
            (Op::Facts, r#"{"op":"facts"}"#),
            (
                Op::Files(FilesOp::List {
                    path: "docs".into(),
                    cursor: Some("opaque-cursor".into()),
                }),
                r#"{"op":"files","cmd":"list","path":"docs","cursor":"opaque-cursor"}"#,
            ),
            (
                Op::Files(FilesOp::Read {
                    path: "notes/todo.md".into(),
                    offset: Some(4096),
                }),
                r#"{"op":"files","cmd":"read","path":"notes/todo.md","offset":4096}"#,
            ),
            (
                Op::Files(FilesOp::Write {
                    path: "out/a.txt".into(),
                    text: "hi\n".into(),
                    expected_hash: Some("ab12".into()),
                }),
                r#"{"op":"files","cmd":"write","path":"out/a.txt","text":"hi\n","expected_hash":"ab12"}"#,
            ),
            (
                Op::Files(FilesOp::Search {
                    q: "TODO".into(),
                    glob: Some("*.rs".into()),
                    max: Some(50),
                }),
                r#"{"op":"files","cmd":"search","q":"TODO","glob":"*.rs","max":50}"#,
            ),
            (
                Op::Term(TermOp::Open {
                    cols: 120,
                    rows: 40,
                    cwd: Some("src".into()),
                }),
                r#"{"op":"term","cmd":"open","cols":120,"rows":40,"cwd":"src"}"#,
            ),
            (
                Op::Term(TermOp::Write {
                    term: "term-1".into(),
                    data_b64: "bHMK".into(),
                }),
                r#"{"op":"term","cmd":"write","term":"term-1","data_b64":"bHMK"}"#,
            ),
            (
                Op::Term(TermOp::Read {
                    term: "term-1".into(),
                    offset: Some(0),
                }),
                r#"{"op":"term","cmd":"read","term":"term-1","offset":0}"#,
            ),
            (
                Op::Term(TermOp::Resize {
                    term: "term-1".into(),
                    cols: 80,
                    rows: 24,
                }),
                r#"{"op":"term","cmd":"resize","term":"term-1","cols":80,"rows":24}"#,
            ),
            (
                Op::Term(TermOp::Close {
                    term: "term-1".into(),
                }),
                r#"{"op":"term","cmd":"close","term":"term-1"}"#,
            ),
            (Op::Term(TermOp::List), r#"{"op":"term","cmd":"list"}"#),
            (
                Op::Run(RunOp::Start {
                    command: "cargo test".into(),
                    timeout_s: Some(600),
                }),
                r#"{"op":"run","cmd":"start","command":"cargo test","timeout_s":600}"#,
            ),
            (
                Op::Run(RunOp::Poll {
                    run: "run-2".into(),
                    offset: Some(1024),
                }),
                r#"{"op":"run","cmd":"poll","run":"run-2","offset":1024}"#,
            ),
            (
                Op::Run(RunOp::Stop {
                    run: "run-2".into(),
                }),
                r#"{"op":"run","cmd":"stop","run":"run-2"}"#,
            ),
            (
                Op::SessionOpen {
                    agent: 3,
                    kind: SessionKind::Shared,
                },
                r#"{"op":"session_open","agent":3,"kind":"shared"}"#,
            ),
            (
                Op::SessionClose { session: 9 },
                r#"{"op":"session_close","session":9}"#,
            ),
            (
                Op::Screenshot {
                    session: 9,
                    scale: 0.5,
                    grid: true,
                    format: ImageFormat::Png,
                },
                r#"{"op":"screenshot","session":9,"scale":0.5,"grid":true,"format":"png"}"#,
            ),
            (
                Op::Input {
                    session: 9,
                    action: InputAction::Type { text: "hi".into() },
                },
                r#"{"op":"input","session":9,"action":{"action":"type","text":"hi"}}"#,
            ),
            (
                Op::UiTree {
                    session: 9,
                    app: Some("Notes".into()),
                    depth: 3,
                },
                r#"{"op":"ui_tree","session":9,"app":"Notes","depth":3}"#,
            ),
            (
                Op::UiTree {
                    session: 9,
                    app: None,
                    depth: 2,
                },
                r#"{"op":"ui_tree","session":9,"depth":2}"#,
            ),
            (
                Op::UiAct {
                    session: 9,
                    selector: Selector {
                        role: Some("button".into()),
                        name: Some("Save".into()),
                        path: None,
                        app: Some("Notes".into()),
                    },
                    action: UiAction::Invoke,
                },
                r#"{"op":"ui_act","session":9,"selector":{"role":"button","name":"Save","app":"Notes"},"action":{"act":"invoke"}}"#,
            ),
            (Op::Apps(AppsOp::List), r#"{"op":"apps","cmd":"list"}"#),
            (
                Op::Apps(AppsOp::Launch {
                    app: "Blender".into(),
                    args: vec!["-b".into(), "scene.blend".into()],
                }),
                r#"{"op":"apps","cmd":"launch","app":"Blender","args":["-b","scene.blend"]}"#,
            ),
            (
                Op::Apps(AppsOp::Launch {
                    app: "Notes".into(),
                    args: vec![],
                }),
                r#"{"op":"apps","cmd":"launch","app":"Notes"}"#,
            ),
            (
                Op::Apps(AppsOp::Focus {
                    app: "Notes".into(),
                }),
                r#"{"op":"apps","cmd":"focus","app":"Notes"}"#,
            ),
            (
                Op::Apps(AppsOp::Quit {
                    app: "Notes".into(),
                    force: false,
                }),
                r#"{"op":"apps","cmd":"quit","app":"Notes"}"#,
            ),
            (
                Op::Apps(AppsOp::Quit {
                    app: "Notes".into(),
                    force: true,
                }),
                r#"{"op":"apps","cmd":"quit","app":"Notes","force":true}"#,
            ),
            (
                Op::Script {
                    bridge: Bridge::AppScripting,
                    app: Some("Blender".into()),
                    code: "import bpy\nbpy.ops.render.render(write_still=True)".into(),
                    timeout_s: 900,
                },
                r#"{"op":"script","bridge":"app_scripting","app":"Blender","code":"import bpy\nbpy.ops.render.render(write_still=True)","timeout_s":900}"#,
            ),
            (
                Op::Script {
                    bridge: Bridge::Cli,
                    app: None,
                    code: "ffmpeg -version".into(),
                    timeout_s: 30,
                },
                r#"{"op":"script","bridge":"cli","code":"ffmpeg -version","timeout_s":30}"#,
            ),
            (
                Op::Stream {
                    session: 9,
                    fps: 2,
                    on: true,
                    scale: None,
                },
                r#"{"op":"stream","session":9,"fps":2,"on":true}"#,
            ),
            (
                Op::Stream {
                    session: 9,
                    fps: 5,
                    on: true,
                    scale: Some(0.5),
                },
                r#"{"op":"stream","session":9,"fps":5,"on":true,"scale":0.5}"#,
            ),
            (Op::Freeze { on: true }, r#"{"op":"freeze","on":true}"#),
            (Op::Sync(SyncOp::Status), r#"{"op":"sync","cmd":"status"}"#),
            (
                Op::Sync(SyncOp::Manifest),
                r#"{"op":"sync","cmd":"manifest"}"#,
            ),
            (
                Op::Sync(SyncOp::Pull {
                    path: "out/render.png".into(),
                    hash: "9f86d0".into(),
                }),
                r#"{"op":"sync","cmd":"pull","path":"out/render.png","hash":"9f86d0"}"#,
            ),
            (
                Op::Sync(SyncOp::Push {
                    path: "out/render.png".into(),
                    hash: "aa".into(),
                    data_b64: "Zm9v".into(),
                    base_hash: Some("bb".into()),
                }),
                r#"{"op":"sync","cmd":"push","path":"out/render.png","hash":"aa","data_b64":"Zm9v","base_hash":"bb"}"#,
            ),
            (
                Op::Sync(SyncOp::Resolve {
                    path: "notes.md".into(),
                    keep: Keep::Both,
                }),
                r#"{"op":"sync","cmd":"resolve","path":"notes.md","keep":"both"}"#,
            ),
            (
                Op::Settings(settings_example()),
                r#"{"op":"settings","takeover":"agent_waits","record":true,"retention_days":30,"record_fps":2,"record_takeovers":true,"work_folder":"~/AgentPlaza","snapshot_reset":false,"rate_per_hour":0}"#,
            ),
        ]
    }

    #[test]
    fn every_op_variant_round_trips() {
        for (op, json) in op_examples() {
            assert_wire(&op, json);
        }
    }

    #[test]
    fn a_push_without_a_base_omits_it() {
        assert_wire(
            &Op::Sync(SyncOp::Push {
                path: "new.txt".into(),
                hash: "cc".into(),
                data_b64: "eA==".into(),
                base_hash: None,
            }),
            r#"{"op":"sync","cmd":"push","path":"new.txt","hash":"cc","data_b64":"eA=="}"#,
        );
    }

    #[test]
    fn the_session_kinds_and_formats_are_lowercase_words() {
        assert_wire(&SessionKind::Pixel, r#""pixel""#);
        assert_wire(&SessionKind::Shared, r#""shared""#);
        assert_wire(&ImageFormat::Png, r#""png""#);
        assert_wire(&ImageFormat::Jpeg, r#""jpeg""#);
    }

    #[test]
    fn every_bridge_has_its_wire_name() {
        for (bridge, json) in [
            (Bridge::Cli, "cli"),
            (Bridge::AppleScript, "applescript"),
            (Bridge::Jxa, "jxa"),
            (Bridge::PowerShell, "powershell"),
            (Bridge::Com, "com"),
            (Bridge::DBus, "dbus"),
            (Bridge::AppScripting, "app_scripting"),
        ] {
            assert_wire(&bridge, &format!(r#""{json}""#));
        }
    }

    #[test]
    fn keep_names_the_side_kept() {
        for (keep, json) in [
            (Keep::Hub, "hub"),
            (Keep::Station, "station"),
            (Keep::Both, "both"),
        ] {
            assert_wire(&keep, &format!(r#""{json}""#));
        }
    }
}
