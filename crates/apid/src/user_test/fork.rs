//! Fork-placed targets (PLAN 19.3): a web target's browser and a terminal target's program run in the lane's test fork,
//! never in the hub's own process. The hub drives them through the fork's exec channel (`base_vm::Fork`, the base runner's
//! exec path), and reaches a browser's DevTools port as a port stream of the fork: 6.1's tunnel to the node, or vsockd's
//! inbound listener on the hub's own machine, as 6.4's preview proxy reaches an app.

use std::cell::RefCell;
use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

use apz_http::mux::{Event, Stream};
use apz_link::inbound::Duplex;

use crate::base_recipe::{Command, WORKSPACE};
use crate::base_vm::{Answer, Fork, Host};
use crate::fleet::LOCAL;
use crate::nodes;

/// The port the browser serves its DevTools protocol on, in the fork: fixed, so the hub knows where to dial.
pub const DEVTOOLS_PORT: u16 = 9222;
/// The browser a web target runs in the fork: the test base's image has a Chromium under this name.
pub const BROWSER: &str = "chromium";
/// How long a read on a port stream waits for bytes before it times out (a socket with a read timeout does the same).
const POLL: Duration = Duration::from_millis(200);

/// The exec channel of a fork: what a fork-placed target sends its commands through, in the fork's workspace.
pub trait Channel {
    /// Runs `command` in the fork: in the foreground to its answer, or in the background until it has started.
    fn exec(&self, command: &Command, background: bool) -> Result<Answer, String>;
}

impl<H: Host> Channel for RefCell<Fork<'_, H>> {
    fn exec(&self, command: &Command, background: bool) -> Result<Answer, String> {
        let mut fork = self.borrow_mut();
        if background {
            fork.background(command)
        } else {
            fork.exec(command)
        }
    }
}

/// A command of a fork-placed target in the workspace.
pub fn command(label: &str, argv: Vec<String>, timeout_s: u64) -> Command {
    Command {
        label: label.to_string(),
        argv,
        cwd: WORKSPACE,
        timeout_s,
        env: Vec::new(),
        listen: None,
    }
}

/// The port the desktop agent of a Linux desktop target serves on, in the fork: fixed, so the hub knows where to dial.
pub const AGENT_PORT: u16 = 9330;

/// The desktop agent's command in the fork (`apz-gui-agent`, on the image's PATH): on a session bus of its own, with
/// AT-SPI started on that bus, serving [`AGENT_PORT`]. Background: it runs until the fork is retired.
pub fn agent_command() -> Command {
    let launch = format!(
        "/usr/libexec/at-spi-bus-launcher --launch-immediately & exec apz-gui-agent --listen {AGENT_PORT}"
    );
    let mut agent = command(
        "the desktop agent",
        ["dbus-run-session", "--", "sh", "-c", launch.as_str()]
            .map(str::to_string)
            .to_vec(),
        0,
    );
    agent.listen = Some(AGENT_PORT);
    agent
}

/// The browser's command in the fork: headless, on a fresh profile, serving its DevTools protocol on [`DEVTOOLS_PORT`].
/// Background: it runs until the fork is retired, and the guest announces the port once it listens.
pub fn browser_command() -> Command {
    let mut browser = command(
        "the browser",
        [
            BROWSER,
            "--headless=new",
            &format!("--remote-debugging-port={DEVTOOLS_PORT}"),
            "--user-data-dir=/tmp/apz-browser",
            "--no-sandbox",
            "--disable-gpu",
            "--no-first-run",
            "about:blank",
        ]
        .map(str::to_string)
        .to_vec(),
        0,
    );
    browser.listen = Some(DEVTOOLS_PORT);
    browser
}

/// The dialer of a fork's port: a port stream through the node's tunnel, or on the hub's own machine a connection to
/// vsockd's inbound listener on `local_inbound` (a test serves its own, as 6.4's proxy tests do). A fresh connection
/// each time it is called.
pub fn port_dial<'a>(
    nodes: &'a nodes::Store,
    node: &'a str,
    cid: u64,
    port: u16,
    local_inbound: u16,
) -> impl FnMut() -> Result<Box<dyn Duplex>, String> + 'a {
    move || {
        if node == LOCAL {
            let conn = apz_link::inbound::connect(local_inbound, cid, port)
                .map_err(|why| format!("the fork's machine does not answer: {why}"))?;
            conn.set_read_timeout(Some(POLL))
                .map_err(|why| format!("the fork's connection: {why}"))?;
            Ok(Box::new(conn))
        } else {
            let stream = nodes
                .open_port(node, cid, port)
                .map_err(|why| format!("the fork's node is not reachable: {why}"))?;
            Ok(Box::new(Wire::new(stream)))
        }
    }
}

/// A port stream as a byte stream: a read waits at most a poll interval for bytes, as a socket with a read timeout does,
/// and a write sends its bytes in order.
pub struct Wire {
    stream: Stream,
    pending: Vec<u8>,
    at: usize,
}

impl Wire {
    pub fn new(stream: Stream) -> Wire {
        Wire {
            stream,
            pending: Vec::new(),
            at: 0,
        }
    }
}

impl Read for Wire {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let deadline = Instant::now() + POLL;
        loop {
            if self.at < self.pending.len() {
                let n = (self.pending.len() - self.at).min(buf.len());
                buf[..n].copy_from_slice(&self.pending[self.at..self.at + n]);
                self.at += n;
                return Ok(n);
            }
            match self.stream.recv(Duration::from_millis(20)) {
                Some(Event::Data(bytes)) if bytes.is_empty() => {}
                Some(Event::Data(bytes)) => {
                    self.pending = bytes;
                    self.at = 0;
                }
                Some(Event::Fin) | Some(Event::Closed) => return Ok(0),
                Some(Event::Reset(why)) => {
                    return Err(io::Error::new(io::ErrorKind::ConnectionReset, why));
                }
                None if Instant::now() >= deadline => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "the port stream is silent",
                    ));
                }
                None => {}
            }
        }
    }
}

impl Write for Wire {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.send(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Duplex for Wire {
    fn duplicate(&self) -> io::Result<Box<dyn Duplex>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "a port stream has one handle",
        ))
    }

    fn close_write(&self) -> io::Result<()> {
        self.stream.finish()
    }
}
