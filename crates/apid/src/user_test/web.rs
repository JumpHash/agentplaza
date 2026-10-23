//! The web target (PLAN 19.3): a headless Chromium driven over the DevTools protocol.
//!
//! - The tree is the page's accessibility tree (`Accessibility.getFullAXTree`). An element's id is its DOM node's backend
//!   id, so an action finds the same node again.
//! - Input is `Input.dispatchMouseEvent` at the centre of the element's box (`DOM.getBoxModel`), and
//!   `Input.dispatchKeyEvent` or `Input.insertText` for typing.
//! - The picture is `Page.captureScreenshot`, a PNG.
//!
//! The browser is found on the machine (`CHROME`, or an installed Chrome or Chromium) and never downloaded: with none,
//! [`find_chrome`] says so and the caller skips. It runs headless on a throwaway profile, which is removed, and the
//! browser killed, when the target drops. The profile is made under `APZ_CHROME_PROFILES` when that names a directory,
//! else under the system's temporary directory.

use std::fs;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{Value, json};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use super::target::{Action, Element, Screenshot, Target, Tree};

/// The environment variable that names the browser to drive.
pub const CHROME_ENV: &str = "CHROME";
/// The environment variable that names the directory the throwaway profiles are made in.
pub const PROFILES_ENV: &str = "APZ_CHROME_PROFILES";
/// The browsers looked for, in order, when `CHROME` is not set.
const INSTALLED: [&str; 4] = [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
];
const WIDTH: u32 = 1024;
const HEIGHT: u32 = 768;
/// How long a page may take to load, and a browser's answer to one call.
const LOAD: Duration = Duration::from_secs(15);
const CALL: Duration = Duration::from_secs(30);
/// The read timeout on the socket: the poll interval of a wait.
const POLL: Duration = Duration::from_millis(200);
/// How long an action is given to take effect before the next observation.
const SETTLE: Duration = Duration::from_millis(250);
/// The most elements one tree carries.
const MAX_ELEMENTS: usize = 400;
static SEQ: AtomicU64 = AtomicU64::new(0);

/// The browser to drive: `CHROME` when it names a file, else the first installed one.
pub fn find_chrome() -> Result<PathBuf, String> {
    if let Some(named) = std::env::var_os(CHROME_ENV).filter(|v| !v.is_empty()) {
        let path = PathBuf::from(named);
        return if path.is_file() {
            Ok(path)
        } else {
            Err(format!(
                "{CHROME_ENV} names {}, which is not a file",
                path.display()
            ))
        };
    }
    INSTALLED
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
        .ok_or_else(|| {
            format!(
                "no Chrome or Chromium is installed (looked in {}); set {CHROME_ENV} to one",
                INSTALLED.join(", ")
            )
        })
}

/// The browser process and its profile: both are gone when this drops.
struct Browser {
    child: Child,
    profile: PathBuf,
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.profile);
    }
}

/// A page in a headless browser: the target's session on the browser's DevTools socket.
pub struct Web {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    session: String,
    next: u64,
    started: Instant,
    /// Last, and held only for its drop: the socket above closes before the browser is killed.
    _browser: Browser,
}

impl Web {
    /// Starts the browser at `chrome` on a fresh profile, opens `url` in a page and waits for the page to load.
    pub fn open(chrome: &Path, url: &str) -> Result<Web, String> {
        let profile = profile_dir()?;
        let child = Command::new(chrome)
            .args(launch_args(&profile))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("the browser {} did not start: {e}", chrome.display()))?;
        let browser = Browser { child, profile };
        let endpoint = devtools_endpoint(&browser.profile)?;
        let (mut socket, _) = tungstenite::connect(endpoint.as_str())
            .map_err(|e| format!("the browser's DevTools socket: {e}"))?;
        if let MaybeTlsStream::Plain(tcp) = socket.get_mut() {
            tcp.set_read_timeout(Some(POLL))
                .map_err(|e| format!("the browser's DevTools socket: {e}"))?;
        }
        let mut web = Web {
            socket,
            session: String::new(),
            next: 0,
            started: Instant::now(),
            _browser: browser,
        };
        let created = web.browser_call("Target.createTarget", json!({ "url": "about:blank" }))?;
        let target = text_field(&created, "targetId")?;
        let attached = web.browser_call(
            "Target.attachToTarget",
            json!({ "targetId": target, "flatten": true }),
        )?;
        web.session = text_field(&attached, "sessionId")?;
        web.command("Page.enable", json!({}))?;
        web.command("DOM.enable", json!({}))?;
        web.command("Accessibility.enable", json!({}))?;
        web.command(
            "Emulation.setDeviceMetricsOverride",
            json!({ "width": WIDTH, "height": HEIGHT, "deviceScaleFactor": 1, "mobile": false }),
        )?;
        web.navigate(url)?;
        Ok(web)
    }

    fn navigate(&mut self, url: &str) -> Result<(), String> {
        let nav = self.command("Page.navigate", json!({ "url": url }))?;
        if let Some(why) = nav["errorText"].as_str() {
            return Err(format!("the page {url} did not open: {why}"));
        }
        self.await_event("Page.loadEventFired", LOAD)
    }

    /// Waits for the browser's event `name` (any session's), or fails when `timeout` passes first.
    fn await_event(&mut self, name: &str, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            let message = self
                .next_message(deadline)
                .map_err(|_| format!("{name} did not come within {}s", timeout.as_secs()))?;
            if message.and_then(|m| m["method"].as_str().map(|s| s == name)) == Some(true) {
                return Ok(());
            }
        }
    }

    /// The next JSON message from the browser, or `None` when a poll passes with nothing. An error once `deadline`
    /// has passed or the socket has failed.
    fn next_message(&mut self, deadline: Instant) -> Result<Option<Value>, String> {
        if Instant::now() >= deadline {
            return Err("the browser did not answer in time".into());
        }
        match self.socket.read() {
            Ok(Message::Text(text)) => Ok(serde_json::from_str::<Value>(text.as_str()).ok()),
            Ok(_) => Ok(None),
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
            {
                Ok(None)
            }
            Err(e) => Err(format!("the browser's connection: {e}")),
        }
    }

    /// A command to the browser itself (no session).
    fn browser_call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.exchange(None, method, params)
    }

    /// A command to the page's session.
    fn command(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let session = self.session.clone();
        self.exchange(Some(&session), method, params)
    }

    /// Sends one command and reads until its answer comes back (events in between are skipped).
    fn exchange(
        &mut self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        self.next += 1;
        let id = self.next;
        let mut frame = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session {
            frame["sessionId"] = json!(session);
        }
        self.socket
            .send(Message::Text(frame.to_string().into()))
            .map_err(|e| format!("the browser's connection: {e}"))?;
        let deadline = Instant::now() + CALL;
        loop {
            let Some(message) = self.next_message(deadline)? else {
                continue;
            };
            if message["id"] != json!(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(format!(
                    "{method} failed: {}",
                    error["message"].as_str().unwrap_or("unknown")
                ));
            }
            return Ok(message["result"].clone());
        }
    }

    /// Runs `expression` in the page and returns its value (a promise is awaited).
    fn evaluate(&mut self, expression: &str) -> Result<Value, String> {
        let result = self.command(
            "Runtime.evaluate",
            json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }),
        )?;
        if let Some(details) = result.get("exceptionDetails") {
            return Err(format!(
                "the page's script failed: {}",
                details["text"].as_str().unwrap_or("an error")
            ));
        }
        Ok(result["result"]["value"].clone())
    }

    /// After an action: a short pause for the page to react, then until it reports itself loaded again (a click that
    /// navigates has its new page loaded by then).
    fn settle(&mut self) -> Result<(), String> {
        thread::sleep(SETTLE);
        let deadline = Instant::now() + LOAD;
        while Instant::now() < deadline {
            if self.evaluate("document.readyState")?.as_str() == Some("complete") {
                break;
            }
            thread::sleep(POLL / 4);
        }
        Ok(())
    }

    /// The centre of an element's box, after bringing it into view.
    fn centre(&mut self, element: &str) -> Result<(f64, f64), String> {
        let node = backend_node(element)?;
        let _ = self.command(
            "DOM.scrollIntoViewIfNeeded",
            json!({ "backendNodeId": node }),
        );
        let model = self
            .command("DOM.getBoxModel", json!({ "backendNodeId": node }))
            .map_err(|why| format!("{element} is not on the page to act on: {why}"))?;
        let quad: Vec<f64> = model["model"]["content"]
            .as_array()
            .map(|q| q.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();
        if quad.len() != 8 {
            return Err(format!("{element} has no box on the page"));
        }
        Ok((
            (quad[0] + quad[2] + quad[4] + quad[6]) / 4.0,
            (quad[1] + quad[3] + quad[5] + quad[7]) / 4.0,
        ))
    }

    fn click(&mut self, element: &str) -> Result<(), String> {
        let (x, y) = self.centre(element)?;
        self.command(
            "Input.dispatchMouseEvent",
            json!({ "type": "mouseMoved", "x": x, "y": y }),
        )?;
        for kind in ["mousePressed", "mouseReleased"] {
            self.command(
                "Input.dispatchMouseEvent",
                json!({ "type": kind, "x": x, "y": y, "button": "left", "clickCount": 1 }),
            )?;
        }
        Ok(())
    }

    fn press(&mut self, key: &str) -> Result<(), String> {
        let spec = key_spec(key)?;
        for kind in ["keyDown", "keyUp"] {
            let mut params = json!({
                "type": kind,
                "key": spec.key,
                "code": spec.code,
                "windowsVirtualKeyCode": spec.code_point,
                "nativeVirtualKeyCode": spec.code_point,
            });
            if kind == "keyDown" && !spec.text.is_empty() {
                params["text"] = json!(spec.text);
            }
            self.command("Input.dispatchKeyEvent", params)?;
        }
        Ok(())
    }
}

impl Target for Web {
    fn kind(&self) -> &str {
        "web"
    }

    fn tree(&mut self) -> Result<Tree, String> {
        let ax = self.command("Accessibility.getFullAXTree", json!({}))?;
        let title = self
            .evaluate("document.title")?
            .as_str()
            .unwrap_or_default()
            .to_string();
        let mut elements = Vec::new();
        for node in ax["nodes"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            if node["ignored"].as_bool().unwrap_or(false) {
                continue;
            }
            let Some(role) = node["role"]["value"].as_str().and_then(map_role) else {
                continue;
            };
            let Some(backend) = node["backendDOMNodeId"].as_u64() else {
                continue;
            };
            let label = ax_field(node, "name");
            if role == "text" && label.trim().is_empty() {
                continue;
            }
            elements.push(Element {
                id: format!("dom-{backend}"),
                role: role.into(),
                label,
                value: ax_field(node, "value"),
            });
            if elements.len() == MAX_ELEMENTS {
                break;
            }
        }
        Ok(Tree { title, elements })
    }

    fn screenshot(&mut self) -> Result<Screenshot, String> {
        let shot = self.command("Page.captureScreenshot", json!({ "format": "png" }))?;
        let data = shot["data"].as_str().ok_or("the browser sent no picture")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| format!("the browser's picture: {e}"))?;
        Ok(Screenshot {
            mime: "image/png".into(),
            bytes,
        })
    }

    fn act(&mut self, action: &Action) -> Result<(), String> {
        match action {
            Action::Click { element } => self.click(element)?,
            Action::Type { element, text } => {
                self.click(element)?;
                self.command("Input.insertText", json!({ "text": text }))?;
            }
            Action::Press { key } => self.press(key)?,
        }
        self.settle()
    }

    fn clock_ms(&mut self) -> Option<u64> {
        // The page's own clock (`performance.now()`), read where the page is; the run's start stands in when it cannot be.
        match self.evaluate("performance.now()") {
            Ok(ms) => ms.as_f64().map(|ms| ms as u64),
            Err(_) => Some(self.started.elapsed().as_millis() as u64),
        }
    }
}

/// A key for [`Web::press`]: its DOM name, its code, its virtual key code and the text it types.
struct KeySpec {
    key: String,
    code: String,
    code_point: u32,
    text: String,
}

fn key_spec(key: &str) -> Result<KeySpec, String> {
    let named = |name: &str, code_point: u32, text: &str| KeySpec {
        key: name.into(),
        code: name.into(),
        code_point,
        text: text.into(),
    };
    Ok(match key {
        "Enter" => named("Enter", 13, "\r"),
        "Tab" => named("Tab", 9, "\t"),
        "Escape" | "Esc" => named("Escape", 27, ""),
        "Backspace" => named("Backspace", 8, ""),
        "ArrowUp" => named("ArrowUp", 38, ""),
        "ArrowDown" => named("ArrowDown", 40, ""),
        "ArrowLeft" => named("ArrowLeft", 37, ""),
        "ArrowRight" => named("ArrowRight", 39, ""),
        other => {
            let mut chars = other.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => KeySpec {
                    key: c.to_string(),
                    code: String::new(),
                    code_point: c.to_ascii_uppercase() as u32,
                    text: c.to_string(),
                },
                _ => return Err(format!("the web target does not know the key {other}")),
            }
        }
    })
}

/// The tree's role for an accessibility role (`None` for the roles a driver has no use for).
fn map_role(role: &str) -> Option<&'static str> {
    Some(match role {
        "button" | "menuitem" | "tab" => "button",
        "link" => "link",
        "textbox" | "searchbox" => "textbox",
        "checkbox" | "switch" => "checkbox",
        "heading" => "heading",
        "StaticText" => "text",
        _ => return None,
    })
}

/// The DOM node an element id names.
fn backend_node(element: &str) -> Result<u64, String> {
    element
        .strip_prefix("dom-")
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| format!("no element {element} on the page"))
}

fn ax_field(node: &Value, field: &str) -> String {
    node[field]["value"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn text_field(value: &Value, field: &str) -> Result<String, String> {
    value[field]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("the browser's answer has no {field}"))
}

/// A new, empty profile directory for one browser.
fn profile_dir() -> Result<PathBuf, String> {
    let root = std::env::var_os(PROFILES_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = root.join(format!(
        "apz-web-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).map_err(|e| format!("the browser's profile: {e}"))?;
    Ok(dir)
}

fn launch_args(profile: &Path) -> Vec<String> {
    let mut args = vec![
        "--headless=new".to_string(),
        "--remote-debugging-port=0".to_string(),
        format!("--user-data-dir={}", profile.display()),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--disable-gpu".into(),
        "--disable-extensions".into(),
        "--disable-background-networking".into(),
        "--disable-sync".into(),
        "--remote-allow-origins=*".into(),
        format!("--window-size={WIDTH},{HEIGHT}"),
    ];
    // A root user's Chrome refuses to start without this; a CI container sets the variable.
    if std::env::var_os("APZ_CHROME_NO_SANDBOX").is_some() {
        args.push("--no-sandbox".into());
    }
    args.push("about:blank".into());
    args
}

/// The browser's DevTools socket, which it writes into its profile once it listens: a port and a path.
fn devtools_endpoint(profile: &Path) -> Result<String, String> {
    let file = profile.join("DevToolsActivePort");
    let deadline = Instant::now() + LOAD;
    loop {
        if let Ok(text) = fs::read_to_string(&file) {
            let mut lines = text.lines();
            if let (Some(port), Some(path)) = (lines.next(), lines.next())
                && let Ok(port) = port.trim().parse::<u16>()
            {
                return Ok(format!("ws://127.0.0.1:{port}{}", path.trim()));
            }
        }
        if Instant::now() >= deadline {
            return Err("the browser did not open its DevTools port".into());
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_element_id_names_its_dom_node_and_a_key_names_its_code() {
        assert_eq!(backend_node("dom-42").unwrap(), 42);
        assert!(backend_node("row-3").is_err());
        assert_eq!(key_spec("Enter").unwrap().code_point, 13);
        let letter = key_spec("p").unwrap();
        assert_eq!((letter.text.as_str(), letter.code_point), ("p", 'P' as u32));
        assert!(key_spec("NoSuchKey").is_err());
    }

    #[test]
    fn accessibility_roles_map_to_the_tree_roles_a_driver_acts_on() {
        assert_eq!(map_role("button"), Some("button"));
        assert_eq!(map_role("searchbox"), Some("textbox"));
        assert_eq!(map_role("StaticText"), Some("text"));
        assert_eq!(map_role("generic"), None);
        assert!(super::super::target::ACTIONABLE.contains(&map_role("link").unwrap()));
    }
}
