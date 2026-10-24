//! The workstation tools agents are offered (docs/design/workstations.md, section 6). A call becomes one operation sent
//! through `act::agent_op`, or one lease opened or released through `act`, so every call passes the same checks and
//! the same audit. The hub MCP surface (`hub_mcp/tools`) offers the same names to owner-token clients, as the owner
//! (cid 0). Results are JSON; a screenshot is base64 image data with its width and height.

use std::thread;
use std::time::Duration;

use apz_station::{
    AppsOp, Bridge, FilesOp, ImageFormat, InputAction, Op, Selector, TermOp, UiAction,
};
use base64::Engine as _;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::act;
use super::leases::Kind;
use crate::agents::Agent;
use crate::state::App;

/// The longest a `station_wait` waits.
const MAX_WAIT_MS: u64 = 30_000;
/// The longest a run may be given, as the daemon caps it.
const MAX_RUN_S: u64 = 600;
/// The longest a script may run.
const MAX_SCRIPT_S: u64 = 600;

/// Every workstation tool's name.
pub const TOOLS: &[&str] = &[
    "station_list",
    "station_lease",
    "station_release",
    "station_screenshot",
    "station_click",
    "station_type",
    "station_key",
    "station_scroll",
    "station_drag",
    "station_wait",
    "station_ui_tree",
    "station_ui_act",
    "station_shell",
    "station_run",
    "station_files",
    "station_app_list",
    "station_app_launch",
    "app_script",
];

/// The description and input schema of a workstation tool, or `None` for another name.
pub fn spec(name: &str) -> Option<(&'static str, Value)> {
    let obj = |properties: Value, required: &[&str]| json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false});
    let station =
        json!({"type": "string", "description": "The workstation's id, from station_list."});
    let lease = json!({"type": "string", "description": "Your lease on the workstation, from station_lease. Screen tools need a pixel lease."});
    let scale = json!({"type": "number", "exclusiveMinimum": 0, "maximum": 1, "description": "The scale of the screenshot the coordinates were read from, as station_screenshot takes it (default 0.5)."});
    let coordinate = json!({"type": "number", "description": "A coordinate in the screenshot's pixels, at `scale`."});
    let pair = json!({"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2, "description": "[x, y] in the screenshot's pixels, at `scale`."});
    let text = json!({"type": "string"});
    let number = json!({"type": "integer"});
    Some(match name {
        "station_list" => (
            "Lists the plaza's workstations: id, name, status (online, offline, draining, revoked), what each machine is (OS, form, displays) and its leases. Start here.",
            obj(json!({}), &[]),
        ),
        "station_lease" => (
            "Opens a lease on a workstation for your task. kind 'shared' is for files, shells, runs and scripts, and is active at once. kind 'pixel' takes the screen for you: it waits up to 25 seconds in the machine's queue for a free session, then answers with its place. kind 'owner_screen' takes the owner's own screen on a machine with no virtual display (a Mac): the same queue and the same screen tools, on the owner's screen. kind 'device' (with device, the serial) takes an emulator or phone on the machine: active at once, held by one agent at a time, for device tools and app_script. Release it with station_release when you are done. The screen tools (station_screenshot, station_click, station_type, station_key, station_scroll, station_drag, station_ui_tree, station_ui_act) need your pixel lease.",
            obj(
                json!({"station": station, "kind": {"type": "string", "enum": ["shared", "pixel", "owner_screen", "device"]}, "task": text, "device": text}),
                &["station", "kind"],
            ),
        ),
        "station_release" => (
            "Releases one of your leases on a workstation. A pixel lease's session closes, and the next queued lease gets the screen.",
            obj(
                json!({"station": station, "lease": lease}),
                &["station", "lease"],
            ),
        ),
        "station_screenshot" => (
            "Takes a screenshot of the screen your pixel lease holds, as base64 image data with its width and height. Reach for it last: prefer app_script, station_files and station_run, then a shell, then the UI tree (station_ui_tree, station_ui_act). scale (above 0, at most 1, default 0.5) sets the image size; grid draws a coordinate grid to read positions from. Give the same scale to the input tools.",
            obj(
                json!({"station": station, "lease": lease, "scale": scale, "grid": {"type": "boolean"}, "format": {"type": "string", "enum": ["png", "jpeg"]}}),
                &["station", "lease"],
            ),
        ),
        "station_click" => (
            "Clicks on the screen your pixel lease holds, at x and y read from a station_screenshot taken at `scale`. button: left, right or middle. count: 2 for a double click. Use station_ui_act on a named control first when there is one.",
            obj(
                json!({"station": station, "lease": lease, "x": coordinate, "y": coordinate, "scale": scale, "button": {"type": "string", "enum": ["left", "right", "middle"]}, "count": {"type": "integer", "minimum": 1, "maximum": 3}}),
                &["station", "lease", "x", "y"],
            ),
        ),
        "station_type" => (
            "Types text as keystrokes into the focused control of the screen your pixel lease holds. For a named field, station_ui_act with set_value is more reliable.",
            obj(
                json!({"station": station, "lease": lease, "text": text}),
                &["station", "lease", "text"],
            ),
        ),
        "station_key" => (
            "Presses a key chord on the screen your pixel lease holds, with the cross-platform key names: 'ctrl+s', 'Return', 'alt+Tab'.",
            obj(
                json!({"station": station, "lease": lease, "combo": text}),
                &["station", "lease", "combo"],
            ),
        ),
        "station_scroll" => (
            "Scrolls at x and y (read from a screenshot at `scale`, as station_click) by dx and dy wheel steps, on the screen your pixel lease holds.",
            obj(
                json!({"station": station, "lease": lease, "x": coordinate, "y": coordinate, "scale": scale, "dx": number, "dy": number}),
                &["station", "lease", "x", "y"],
            ),
        ),
        "station_drag" => (
            "Drags with the left button from one point to another on the screen your pixel lease holds, both read from a screenshot at `scale`.",
            obj(
                json!({"station": station, "lease": lease, "from": pair, "to": pair, "scale": scale}),
                &["station", "lease", "from", "to"],
            ),
        ),
        "station_wait" => (
            "Waits `ms` milliseconds (default 1000, at most 30000) for an app to start or a page to load. Sends nothing to the workstation.",
            obj(
                json!({"ms": {"type": "integer", "minimum": 0, "maximum": MAX_WAIT_MS}}),
                &[],
            ),
        ),
        "station_ui_tree" => (
            "Reads the accessibility tree of the screen your pixel lease holds, or of one app's window (app), down to `depth` levels (default 4, at most 8). Each node has a role, name, value, bounds and the actions it takes. Find controls here by role and name before you click at pixels.",
            obj(
                json!({"station": station, "lease": lease, "app": text, "depth": {"type": "integer", "minimum": 1, "maximum": 8}}),
                &["station", "lease"],
            ),
        ),
        "station_ui_act" => (
            "Acts on one control of the UI tree: finds it by selector (role, name, path, app; each field given must match) and does act: invoke, set_value (with value), focus, select, toggle, expand or collapse.",
            obj(
                json!({"station": station, "lease": lease, "selector": {"type": "object"}, "act": {"type": "string", "enum": ["invoke", "set_value", "focus", "select", "toggle", "expand", "collapse"]}, "value": text}),
                &["station", "lease", "act"],
            ),
        ),
        "station_shell" => (
            "Works in a terminal on the workstation; no pixel lease needed. action: open (cols, rows, cwd; answers with a term id), write (term, text: typed into the shell, end it with a newline to run it), read (term, offset: the output from offset), resize (term, cols, rows), close (term), list.",
            obj(
                json!({"station": station, "action": {"type": "string", "enum": ["open", "write", "read", "resize", "close", "list"]}, "term": text, "text": text, "cols": number, "rows": number, "cwd": text, "offset": number}),
                &["station", "action"],
            ),
        ),
        "station_run" => (
            "Runs one shell command in the workstation's work folder, without a terminal; no pixel lease needed. action: start (command, timeout_s up to 600; answers with a run id), poll (run, offset: the output so far), stop (run).",
            obj(
                json!({"station": station, "action": {"type": "string", "enum": ["start", "poll", "stop"]}, "command": text, "timeout_s": number, "run": text, "offset": number}),
                &["station", "action"],
            ),
        ),
        "station_files" => (
            "Reads and writes files in the workstation's work folder; no pixel lease needed. cmd: list (path, cursor), read (path, offset), write (path, text, expected_hash: refused when the file changed since you read it), search (q, glob, max). deliver: true with path copies that file into the Library as a deliverable credited to you and your task on this workstation, and answers with its id.",
            obj(
                json!({"station": station, "cmd": {"type": "string", "enum": ["list", "read", "write", "search"]}, "deliver": {"type": "boolean"}, "path": text, "cursor": text, "offset": number, "text": text, "expected_hash": text, "q": text, "glob": text, "max": number}),
                &["station"],
            ),
        ),
        "station_app_list" => (
            "Lists the applications on the workstation, with their ids, paths and scripting entry points. Use it before app_script, to learn what an app can be scripted with.",
            obj(json!({"station": station}), &["station"]),
        ),
        "station_app_launch" => (
            "Launches an application on the workstation (app, args).",
            obj(
                json!({"station": station, "app": text, "args": {"type": "array", "items": {"type": "string"}}}),
                &["station", "app"],
            ),
        ),
        "app_script" => (
            "Runs a script through one scripting bridge of the workstation, and answers with its exit code and output; no pixel lease needed. Prefer it to pixels. bridge: cli (a command line), app_scripting (the app's own embedded scripting, such as Blender's Python, with app), applescript or jxa (macOS), powershell or com (Windows), dbus (Linux). timeout_ms defaults to 60000, at most 600000.",
            obj(
                json!({"station": station, "bridge": {"type": "string", "enum": ["cli", "app_scripting", "applescript", "jxa", "powershell", "com", "dbus"]}, "script": text, "app": text, "timeout_ms": {"type": "integer", "minimum": 1000, "maximum": MAX_SCRIPT_S * 1000}}),
                &["station", "bridge", "script"],
            ),
        ),
        _ => return None,
    })
}

/// A parsed call: the operation it sends, or the lease, release, wait or listing it asks for.
#[derive(Debug, PartialEq)]
pub(crate) enum Call {
    /// An operation on a station, under `lease` when the call names one.
    Op {
        station: String,
        lease: Option<String>,
        op: Op,
    },
    Lease {
        station: String,
        kind: Kind,
        task: Option<String>,
        /// The device's serial, for a device lease.
        device: Option<String>,
    },
    Release {
        station: String,
        lease: String,
    },
    /// A mirrored file of the station copied into the Library (`station_files` with `deliver`).
    Deliver {
        station: String,
        path: String,
    },
    Wait(Duration),
    List,
}

/// Reads a call's input into what it asks for. Nothing is sent here: the screen operations carry session 0 until
/// `call` takes their session from the lease.
pub(crate) fn parse(tool: &str, input: &Value) -> Result<Call, String> {
    match tool {
        "station_list" => return Ok(Call::List),
        "station_wait" => {
            let ms = optional(input, "ms", Value::as_u64, "a whole number")?
                .unwrap_or(1000)
                .min(MAX_WAIT_MS);
            return Ok(Call::Wait(Duration::from_millis(ms)));
        }
        _ => {}
    }
    let station = str_arg(input, "station")?;
    let lease = optional(input, "lease", str_of, "a string")?;
    if tool == "station_files"
        && optional(input, "deliver", Value::as_bool, "true or false")?.unwrap_or(false)
    {
        return Ok(Call::Deliver {
            station,
            path: str_arg(input, "path")?,
        });
    }
    let op = match tool {
        "station_lease" => {
            let kind = match str_arg(input, "kind")?.as_str() {
                "shared" => Kind::Shared,
                "pixel" => Kind::Pixel,
                "owner_screen" => Kind::OwnerScreen,
                "device" => Kind::Device,
                other => {
                    return Err(format!(
                        "`kind` must be shared, pixel, owner_screen or device, not {other}"
                    ));
                }
            };
            let device = optional(input, "device", str_of, "a string")?;
            if (kind == Kind::Device) != device.is_some() {
                return Err("a device lease names its device (the adb serial), and only a device lease does".into());
            }
            return Ok(Call::Lease {
                station,
                kind,
                task: optional(input, "task", str_of, "a string")?,
                device,
            });
        }
        "station_release" => {
            return Ok(Call::Release {
                station,
                lease: str_arg(input, "lease")?,
            });
        }
        "station_screenshot" => {
            let format = match optional(input, "format", str_of, "a string")?.as_deref() {
                None | Some("png") => ImageFormat::Png,
                Some("jpeg") => ImageFormat::Jpeg,
                Some(other) => return Err(format!("`format` must be png or jpeg, not {other}")),
            };
            let grid = optional(input, "grid", Value::as_bool, "true or false")?.unwrap_or(false);
            Op::Screenshot {
                session: 0,
                scale: scale(input)? as f32,
                grid,
                format,
            }
        }
        "station_click" => {
            let s = scale(input)?;
            let count = optional(input, "count", Value::as_u64, "a whole number")?.unwrap_or(1);
            if !(1..=3).contains(&count) {
                return Err("`count` must be 1, 2 or 3".into());
            }
            let button =
                optional(input, "button", str_of, "a string")?.unwrap_or_else(|| "left".into());
            input_op(
                json!({"action": "click", "x": point(input, "x", s)?, "y": point(input, "y", s)?, "button": button, "count": count}),
            )?
        }
        "station_type" => input_op(json!({"action": "type", "text": str_arg(input, "text")?}))?,
        "station_key" => input_op(json!({"action": "key", "combo": str_arg(input, "combo")?}))?,
        "station_scroll" => {
            let s = scale(input)?;
            let (dx, dy) = (
                optional(input, "dx", Value::as_i64, "a whole number")?.unwrap_or(0),
                optional(input, "dy", Value::as_i64, "a whole number")?.unwrap_or(0),
            );
            input_op(
                json!({"action": "scroll", "x": point(input, "x", s)?, "y": point(input, "y", s)?, "dx": dx, "dy": dy}),
            )?
        }
        "station_drag" => {
            let s = scale(input)?;
            input_op(
                json!({"action": "drag", "from": pair(input, "from", s)?, "to": pair(input, "to", s)?}),
            )?
        }
        "station_ui_tree" => Op::UiTree {
            session: 0,
            app: optional(input, "app", str_of, "a string")?,
            depth: optional(input, "depth", Value::as_u64, "a whole number")?
                .unwrap_or(4)
                .clamp(1, 8) as u8,
        },
        "station_ui_act" => {
            let selector: Selector =
                decode(input.get("selector").cloned().unwrap_or_else(|| json!({})))?;
            let mut body = json!({"act": str_arg(input, "act")?});
            if let Some(value) = optional(input, "value", str_of, "a string")? {
                body["value"] = json!(value);
            }
            Op::UiAct {
                session: 0,
                selector,
                action: decode::<UiAction>(body)?,
            }
        }
        "station_shell" => {
            let body = match str_arg(input, "action")?.as_str() {
                "open" => json!({
                    "cmd": "open",
                    "cols": optional(input, "cols", Value::as_u64, "a whole number")?.unwrap_or(120),
                    "rows": optional(input, "rows", Value::as_u64, "a whole number")?.unwrap_or(32),
                    "cwd": optional(input, "cwd", str_of, "a string")?,
                }),
                "write" => json!({
                    "cmd": "write",
                    "term": str_arg(input, "term")?,
                    "data_b64": base64::engine::general_purpose::STANDARD.encode(str_arg(input, "text")?),
                }),
                "read" => {
                    json!({"cmd": "read", "term": str_arg(input, "term")?, "offset": optional(input, "offset", Value::as_u64, "a whole number")?})
                }
                "resize" => json!({
                    "cmd": "resize",
                    "term": str_arg(input, "term")?,
                    "cols": optional(input, "cols", Value::as_u64, "a whole number")?.unwrap_or(120),
                    "rows": optional(input, "rows", Value::as_u64, "a whole number")?.unwrap_or(32),
                }),
                "close" => json!({"cmd": "close", "term": str_arg(input, "term")?}),
                "list" => json!({"cmd": "list"}),
                other => {
                    return Err(format!(
                        "`action` must be open, write, read, resize, close or list, not {other}"
                    ));
                }
            };
            Op::Term(decode::<TermOp>(body)?)
        }
        "station_run" => {
            let body = match str_arg(input, "action")?.as_str() {
                "start" => json!({
                    "cmd": "start",
                    "command": str_arg(input, "command")?,
                    "timeout_s": optional(input, "timeout_s", Value::as_u64, "a whole number")?.map(|s| s.min(MAX_RUN_S)),
                }),
                "poll" => {
                    json!({"cmd": "poll", "run": str_arg(input, "run")?, "offset": optional(input, "offset", Value::as_u64, "a whole number")?})
                }
                "stop" => json!({"cmd": "stop", "run": str_arg(input, "run")?}),
                other => return Err(format!("`action` must be start, poll or stop, not {other}")),
            };
            Op::Run(decode(body)?)
        }
        "station_files" => Op::Files(decode::<FilesOp>(input.clone())?),
        "station_app_list" => Op::Apps(AppsOp::List),
        "station_app_launch" => {
            let args = input.get("args").cloned().unwrap_or_else(|| json!([]));
            Op::Apps(decode::<AppsOp>(
                json!({"cmd": "launch", "app": str_arg(input, "app")?, "args": args}),
            )?)
        }
        "app_script" => {
            let bridge: Bridge = decode(json!(str_arg(input, "bridge")?))?;
            let ms = optional(input, "timeout_ms", Value::as_u64, "a whole number")?
                .unwrap_or(60_000)
                .clamp(1000, MAX_SCRIPT_S * 1000);
            Op::Script {
                bridge,
                app: optional(input, "app", str_of, "a string")?,
                code: str_arg(input, "script")?,
                timeout_s: ms.div_ceil(1000) as u32,
            }
        }
        other => return Err(format!("{other} is not a workstation tool")),
    };
    Ok(Call::Op { station, lease, op })
}

/// Runs workstation tool `tool` for the agent `cid` (0: the owner's MCP client), answering with JSON.
pub fn call(app: &App, cid: u64, tool: &str, input: &Value) -> Result<Value, String> {
    match parse(tool, input)? {
        Call::List => Ok(Value::Array(list(app))),
        Call::Wait(wait) => {
            thread::sleep(wait);
            Ok(json!({"waited_ms": wait.as_millis() as u64}))
        }
        Call::Lease {
            station,
            kind,
            task,
            device,
        } => act::lease_open(app, cid, &station, kind, task, device)
            .map(|view| json!(view))
            .map_err(message),
        Call::Release { station, lease } => act::lease_release(app, cid, &station, &lease)
            .map(|released| json!(released))
            .map_err(message),
        Call::Deliver { station, path } => {
            act::agent_deliver(app, cid, &station, &path).map_err(message)
        }
        Call::Op {
            station,
            lease,
            mut op,
        } => {
            fill_session(app, lease.as_deref(), &mut op);
            act::agent_op(app, cid, &station, lease.as_deref(), op).map_err(message)
        }
    }
}

/// The host tool entry point: `tool` for `agent`, its answer as JSON text.
pub fn run(app: &App, agent: &Agent, tool: &str, input: &Value) -> Result<String, String> {
    call(app, agent.cid, tool, input).map(|out| out.to_string())
}

/// The workstations as the agents and the owner see them: their id, name, status, facts and leases. The join code
/// stays out.
pub fn list(app: &App) -> Vec<Value> {
    app.stations
        .views(apz_plaza::now_ms())
        .iter()
        .filter_map(|view| {
            let mut row = serde_json::to_value(view).ok()?;
            row.as_object_mut()?.remove("code");
            Some(row)
        })
        .collect()
}

/// Sets a screen operation's session to the one its lease holds, so the choke point checks that the two match.
fn fill_session(app: &App, lease: Option<&str>, op: &mut Op) {
    let session = lease
        .and_then(|id| app.stations.leases.get(id))
        .and_then(|held| held.session)
        .unwrap_or(0);
    if let Op::Screenshot { session: at, .. }
    | Op::Input { session: at, .. }
    | Op::UiTree { session: at, .. }
    | Op::UiAct { session: at, .. } = op
    {
        *at = session;
    }
}

fn message((_, text): super::ops::Failure) -> String {
    text
}

fn input_op(body: Value) -> Result<Op, String> {
    Ok(Op::Input {
        session: 0,
        action: decode::<InputAction>(body)?,
    })
}

fn decode<T: DeserializeOwned>(body: Value) -> Result<T, String> {
    serde_json::from_value(body).map_err(|e| format!("invalid input: {e}"))
}

fn str_of(value: &Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

fn str_arg(input: &Value, key: &str) -> Result<String, String> {
    input
        .get(key)
        .and_then(str_of)
        .ok_or_else(|| format!("missing string `{key}`"))
}

/// An optional argument: absent or null is `None`; anything else must read as `what`.
fn optional<T>(
    input: &Value,
    key: &str,
    read: impl Fn(&Value) -> Option<T>,
    what: &str,
) -> Result<Option<T>, String> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => read(value)
            .map(Some)
            .ok_or_else(|| format!("`{key}` must be {what}")),
    }
}

fn number(input: &Value, key: &str) -> Result<f64, String> {
    input
        .get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("missing number `{key}`"))
}

/// The screenshot scale the coordinates were read at: above 0 and at most 1, 0.5 by default.
fn scale(input: &Value) -> Result<f64, String> {
    let scale = optional(input, "scale", Value::as_f64, "a number")?.unwrap_or(0.5);
    if scale > 0.0 && scale <= 1.0 {
        Ok(scale)
    } else {
        Err("`scale` must be above 0 and at most 1".into())
    }
}

/// A screen coordinate: the screenshot's pixel over its scale is the screen's point.
fn point(input: &Value, key: &str, scale: f64) -> Result<i32, String> {
    Ok((number(input, key)? / scale).round() as i32)
}

/// A screen point given as `[x, y]`, mapped as `point` maps each coordinate.
fn pair(input: &Value, key: &str, scale: f64) -> Result<Value, String> {
    let items = input
        .get(key)
        .and_then(Value::as_array)
        .filter(|items| items.len() == 2)
        .ok_or_else(|| format!("missing `{key}`: [x, y]"))?;
    let (x, y) = (items[0].as_f64(), items[1].as_f64());
    match (x, y) {
        (Some(x), Some(y)) => Ok(json!([
            (x / scale).round() as i32,
            (y / scale).round() as i32
        ])),
        _ => Err(format!("`{key}` must be two numbers")),
    }
}

#[cfg(test)]
mod tests {
    use apz_station::{Bridge, Button, InputAction, TermOp};

    use super::*;

    #[test]
    fn every_tool_has_a_spec_and_no_other_name_does() {
        for name in TOOLS {
            assert!(spec(name).is_some(), "{name} has no spec");
        }
        assert!(spec("station_nothing").is_none());
    }

    #[test]
    fn a_click_point_is_mapped_from_the_screenshot_scale() {
        let call = parse(
            "station_click",
            &json!({"station": "w1", "lease": "l1", "x": 100, "y": 50, "scale": 0.5}),
        )
        .unwrap();
        assert_eq!(
            call,
            Call::Op {
                station: "w1".into(),
                lease: Some("l1".into()),
                op: Op::Input {
                    session: 0,
                    action: InputAction::Click {
                        x: 200,
                        y: 100,
                        button: Button::Left,
                        count: 1
                    }
                },
            }
        );
    }

    #[test]
    fn a_wait_is_capped_and_is_not_an_operation() {
        assert_eq!(
            parse("station_wait", &json!({"ms": 60000})).unwrap(),
            Call::Wait(Duration::from_secs(30))
        );
        assert_eq!(
            parse("station_wait", &json!({})).unwrap(),
            Call::Wait(Duration::from_secs(1))
        );
    }

    #[test]
    fn shell_text_is_sent_as_base64() {
        let call = parse(
            "station_shell",
            &json!({"station": "w1", "action": "write", "term": "term-1", "text": "ls\n"}),
        )
        .unwrap();
        assert_eq!(
            call,
            Call::Op {
                station: "w1".into(),
                lease: None,
                op: Op::Term(TermOp::Write {
                    term: "term-1".into(),
                    data_b64: "bHMK".into()
                })
            }
        );
    }

    #[test]
    fn a_script_runs_for_its_timeout_capped_at_ten_minutes() {
        let call = parse(
            "app_script",
            &json!({"station": "w1", "bridge": "applescript", "script": "tell app", "timeout_ms": 9_999_999}),
        )
        .unwrap();
        assert_eq!(
            call,
            Call::Op {
                station: "w1".into(),
                lease: None,
                op: Op::Script {
                    bridge: Bridge::AppleScript,
                    app: None,
                    code: "tell app".into(),
                    timeout_s: 600
                },
            }
        );
        assert!(
            parse(
                "app_script",
                &json!({"station": "w1", "bridge": "bash", "script": "ls"})
            )
            .is_err()
        );
    }

    #[test]
    fn a_lease_names_its_kind_and_a_release_its_lease() {
        assert_eq!(
            parse(
                "station_lease",
                &json!({"station": "w1", "kind": "pixel", "task": "t1"})
            )
            .unwrap(),
            Call::Lease {
                station: "w1".into(),
                kind: Kind::Pixel,
                task: Some("t1".into()),
                device: None,
            }
        );
        assert!(parse("station_lease", &json!({"station": "w1", "kind": "both"})).is_err());
        assert_eq!(
            parse(
                "station_lease",
                &json!({"station": "w1", "kind": "device", "device": "R58N1234"})
            )
            .unwrap(),
            Call::Lease {
                station: "w1".into(),
                kind: Kind::Device,
                task: None,
                device: Some("R58N1234".into()),
            }
        );
        assert!(
            parse("station_lease", &json!({"station": "w1", "kind": "device"})).is_err(),
            "a device lease names its device"
        );
        assert!(
            parse(
                "station_lease",
                &json!({"station": "w1", "kind": "pixel", "device": "x"})
            )
            .is_err(),
            "only a device lease names one"
        );
        assert_eq!(
            parse("station_release", &json!({"station": "w1", "lease": "l4"})).unwrap(),
            Call::Release {
                station: "w1".into(),
                lease: "l4".into()
            }
        );
    }

    #[test]
    fn a_file_delivery_is_a_call_of_its_own_and_a_file_operation_is_not() {
        assert_eq!(
            parse(
                "station_files",
                &json!({"station": "w1", "deliver": true, "path": "out/render.png"})
            )
            .unwrap(),
            Call::Deliver {
                station: "w1".into(),
                path: "out/render.png".into()
            }
        );
        assert!(
            parse("station_files", &json!({"station": "w1", "deliver": true})).is_err(),
            "a delivery names its path"
        );
        assert!(
            matches!(
                parse(
                    "station_files",
                    &json!({"station": "w1", "deliver": false, "cmd": "list", "path": "."})
                )
                .unwrap(),
                Call::Op {
                    op: Op::Files(_),
                    ..
                }
            ),
            "deliver false is an ordinary file operation"
        );
        assert!(
            parse("station_files", &json!({"station": "w1", "path": "."})).is_err(),
            "a file operation needs its cmd"
        );
        assert!(
            parse(
                "station_files",
                &json!({"station": "w1", "deliver": "yes", "path": "a"})
            )
            .is_err()
        );
    }

    #[test]
    fn a_screen_tool_needs_a_station_and_a_valid_scale() {
        assert!(
            parse("station_click", &json!({"lease": "l1", "x": 1, "y": 1}))
                .unwrap_err()
                .contains("station")
        );
        assert!(
            parse(
                "station_click",
                &json!({"station": "w1", "x": 1, "y": 1, "scale": 2})
            )
            .is_err()
        );
        assert!(
            parse(
                "station_click",
                &json!({"station": "w1", "x": 1, "y": 1, "button": "wheel"})
            )
            .is_err()
        );
    }
}
