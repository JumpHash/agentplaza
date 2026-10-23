//! The web target (PLAN 19.3): a headless Chromium driven over the DevTools protocol.
//!
//! - The tree is the page's accessibility tree (`Accessibility.getFullAXTree`). An element's id is its DOM node's backend
//!   id, so an action finds the same node again.
//! - Input is `Input.dispatchMouseEvent` at the centre of the element's box (`DOM.getBoxModel`), and
//!   `Input.dispatchKeyEvent` or `Input.insertText` for typing.
//! - The picture is `Page.captureScreenshot`, a PNG.
//!
//! In production the browser runs in the lane's test fork (`fork.rs`): [`Web::attach`] dials its DevTools port as a port
//! stream of the fork, opens a page in a new browser context of its own and loads the page there. The hub never starts a
//! browser. A test starts a real one on this machine (`open_local`, `cfg(test)`), with the same attach.

use std::io::{ErrorKind, Read, Write};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{Value, json};
use tungstenite::{Message, WebSocket};

use apz_link::inbound::Duplex;

use super::target::{Action, Element, Screenshot, Target, Tree};

/// How long a page may take to load, and a browser's answer to one call.
const LOAD: Duration = Duration::from_secs(15);
const CALL: Duration = Duration::from_secs(30);
/// The read timeout on the socket: the poll interval of a wait.
const POLL: Duration = Duration::from_millis(200);
/// How long an action is given to take effect before the next observation.
const SETTLE: Duration = Duration::from_millis(250);
/// The most elements one tree carries.
const MAX_ELEMENTS: usize = 400;
#[cfg(test)]
const WIDTH: u32 = 1024;
#[cfg(test)]
const HEIGHT: u32 = 768;

/// A page in a headless browser: the target's session on the browser's DevTools socket.
pub struct Web {
    socket: WebSocket<Box<dyn Duplex>>,
    session: String,
    next: u64,
    started: Instant,
    /// The browser this target started on this machine (a test's), killed with the target.
    #[cfg(test)]
    _browser: Option<local::Browser>,
}

impl Web {
    /// Connects to the browser's DevTools port through `dial` (a fresh connection for each request), opens a page in a
    /// new browser context of its own and loads `url` in it. Retries the first connection for the browser's start.
    pub fn attach(
        dial: &mut dyn FnMut() -> Result<Box<dyn Duplex>, String>,
        url: &str,
    ) -> Result<Web, String> {
        let ws_url = devtools_socket(dial)?;
        let stream = dial()?;
        let (socket, _) = tungstenite::client(ws_url.as_str(), stream)
            .map_err(|e| format!("the browser's DevTools socket: {e}"))?;
        let mut web = Web {
            socket,
            session: String::new(),
            next: 0,
            started: Instant::now(),
            #[cfg(test)]
            _browser: None,
        };
        let context = web.browser_call(
            "Target.createBrowserContext",
            json!({ "disposeOnDetach": true }),
        )?;
        let context = text_field(&context, "browserContextId")?;
        let created = web.browser_call(
            "Target.createTarget",
            json!({ "url": "about:blank", "browserContextId": context }),
        )?;
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
            json!({ "width": 1024, "height": 768, "deviceScaleFactor": 1, "mobile": false }),
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
        std::thread::sleep(SETTLE);
        let deadline = Instant::now() + LOAD;
        while Instant::now() < deadline {
            if self.evaluate("document.readyState")?.as_str() == Some("complete") {
                break;
            }
            std::thread::sleep(POLL / 4);
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
        // The page's own clock (`performance.now()`); the time since the target opened stands in when it cannot be read.
        match self.evaluate("performance.now()") {
            Ok(ms) => ms.as_f64().map(|ms| ms as u64),
            Err(_) => Some(self.started.elapsed().as_millis() as u64),
        }
    }
}

/// The browser's WebSocket address for this session, from its version answer: a fresh connection, one HTTP request, the
/// answer's body. Retried while the browser starts (its port is not open until it is listening).
fn devtools_socket(
    dial: &mut dyn FnMut() -> Result<Box<dyn Duplex>, String>,
) -> Result<String, String> {
    let deadline = Instant::now() + LOAD;
    let mut conn = loop {
        match dial() {
            Ok(conn) => break conn,
            Err(why) if Instant::now() >= deadline => {
                return Err(format!(
                    "the browser's DevTools port does not answer: {why}"
                ));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    };
    conn.write_all(b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .map_err(|e| format!("the browser's version request: {e}"))?;
    let answer = read_answer(&mut conn, deadline)?;
    let text = String::from_utf8_lossy(&answer);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .ok_or("the browser's version answer has no body")?;
    let version: Value =
        serde_json::from_str(body).map_err(|e| format!("the browser's version answer: {e}"))?;
    version["webSocketDebuggerUrl"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "the browser's version answer names no socket".to_string())
}

/// The whole of an HTTP answer: read until the body its `Content-Length` names is in, or the connection ends (a browser
/// closes it: `Connection: close`), or `deadline` passes.
fn read_answer(conn: &mut Box<dyn Duplex>, deadline: Instant) -> Result<Vec<u8>, String> {
    let mut answer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match conn.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => answer.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
        if let Some((head, body)) = String::from_utf8_lossy(&answer)
            .split_once("\r\n\r\n")
            .map(|(h, b)| (h.to_string(), b.len()))
            && let Some(length) = head
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>())
                })
                .and_then(Result::ok)
            && body >= length
        {
            break;
        }
        if Instant::now() >= deadline {
            return Err("the browser's version answer did not finish".into());
        }
    }
    Ok(answer)
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

/// A browser started on this machine, for the tests: the real Chrome that stands in for a fork's browser.
#[cfg(test)]
pub mod local {
    use std::fs;
    use std::net::TcpStream;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    use apz_link::inbound::Duplex;

    use super::{HEIGHT, POLL, WIDTH, Web};

    /// The browser to drive: `CHROME` when it names a file, else the first installed one.
    pub fn find_chrome() -> Result<PathBuf, String> {
        const INSTALLED: [&str; 4] = [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/usr/bin/google-chrome",
            "/usr/bin/chromium",
        ];
        if let Some(named) = std::env::var_os("CHROME").filter(|v| !v.is_empty()) {
            let path = PathBuf::from(named);
            return if path.is_file() {
                Ok(path)
            } else {
                Err(format!(
                    "CHROME names {}, which is not a file",
                    path.display()
                ))
            };
        }
        INSTALLED
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
            .ok_or_else(|| "no Chrome or Chromium is installed; set CHROME to one".to_string())
    }

    static SEQ: AtomicU64 = AtomicU64::new(0);

    /// The browser process and its profile: both are gone when this drops.
    pub struct Browser {
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

    /// Starts `chrome` headless on a throwaway profile, and loads `url` in a page of it through the same attach a
    /// fork's browser takes. The profile is made under `APZ_CHROME_PROFILES` when that names a directory.
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
        let port = devtools_port(&browser.profile)?;
        let mut dial = move || -> Result<Box<dyn Duplex>, String> {
            let conn = TcpStream::connect(("127.0.0.1", port))
                .map_err(|e| format!("the browser's port {port}: {e}"))?;
            conn.set_read_timeout(Some(POLL))
                .map_err(|e| format!("the browser's port {port}: {e}"))?;
            Ok(Box::new(conn))
        };
        let mut web = Web::attach(&mut dial, url)?;
        web._browser = Some(browser);
        Ok(web)
    }

    fn profile_dir() -> Result<PathBuf, String> {
        let root = std::env::var_os("APZ_CHROME_PROFILES")
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
        if std::env::var_os("APZ_CHROME_NO_SANDBOX").is_some() {
            args.push("--no-sandbox".into());
        }
        args.push("about:blank".into());
        args
    }

    /// The port the browser serves its DevTools protocol on: the first line of its `DevToolsActivePort` file.
    fn devtools_port(profile: &Path) -> Result<u16, String> {
        let file = profile.join("DevToolsActivePort");
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(port) = fs::read_to_string(&file)
                .ok()
                .and_then(|text| text.lines().next().and_then(|l| l.trim().parse().ok()))
            {
                return Ok(port);
            }
            if Instant::now() >= deadline {
                return Err("the browser did not open its DevTools port".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
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
