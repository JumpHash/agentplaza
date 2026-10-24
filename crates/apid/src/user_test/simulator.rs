//! The iOS simulator target (PLAN 19.3, W47): an app in a simulator device of a Mac node, driven through `simctl` (launch,
//! screenshots, video) and an accessibility bridge (the tree and the taps).
//!
//! - The command lines are `apz-desktop-macos`'s [`Simctl`]; a [`Runner`] runs them on the node (`desk::StationRunner`
//!   sends them through the station's shell, as the target's agent).
//! - The screenshot is `simctl io screenshot` to a file on the node, read back through the runner.
//! - The tree and the taps come from the accessibility bridge, behind [`Bridge`]. The bridge is the one piece this
//!   repository does not yet speak: [`Unwired`] answers that reason, and a client for the bridge's documented protocol
//!   (Callstack's open-source `agent-device`, PLAN 19.3) implements [`Bridge`] to replace it. A simulator step whose app
//!   has no tree does not get past its first look until then.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use apz_desktop_macos::simctl::{Runner, Simctl, exec};

use super::target::{Action, Screenshot, Target, Tree};

/// The accessibility bridge of the simulator: what the target asks of it.
pub trait Bridge {
    /// The tree of the app on the device's screen now.
    fn tree(&mut self) -> Result<Tree, String>;
    /// One action on the screen: a tap or a typed value (through the bridge's own channel).
    fn act(&mut self, action: &Action) -> Result<(), String>;
}

/// Why the bridge is not wired. The extension point is [`Bridge`]: a client implements it and replaces [`Unwired`] where the
/// simulator target is opened (`desk::simulator_opener`).
pub const UNWIRED: &str = "the simulator's accessibility bridge is not wired: an app's tree and its taps need a client for the bridge's documented protocol (PLAN 19.3 names Callstack's agent-device); none is in this repository yet";

/// The bridge when none is wired: the tree and the actions answer why.
pub struct Unwired;

impl Bridge for Unwired {
    fn tree(&mut self) -> Result<Tree, String> {
        Err(UNWIRED.to_string())
    }

    fn act(&mut self, _action: &Action) -> Result<(), String> {
        Err(UNWIRED.to_string())
    }
}

/// Screenshot files on the node are numbered across the process, so two targets never write the same one.
static SHOTS: AtomicU64 = AtomicU64::new(0);

/// A simulator device with the app launched in it, for one scenario.
pub struct Simulator<'a> {
    runner: Box<dyn Runner + 'a>,
    bridge: Box<dyn Bridge + 'a>,
    device: String,
    /// The recording in progress: its process on the node, and the file it writes.
    recording: Option<(u32, String)>,
    started: Instant,
}

impl<'a> Simulator<'a> {
    /// Launches `bundle` fresh in `device` (a copy already running is ended), and answers the target.
    pub fn launch(
        runner: Box<dyn Runner + 'a>,
        bridge: Box<dyn Bridge + 'a>,
        device: &str,
        bundle: &str,
    ) -> Result<Self, String> {
        let target = Self {
            runner,
            bridge,
            device: device.to_string(),
            recording: None,
            started: Instant::now(),
        };
        exec(
            target.runner.as_ref(),
            &Simctl::Launch {
                device: device.to_string(),
                bundle: bundle.to_string(),
                args: Vec::new(),
            },
        )?;
        Ok(target)
    }

    /// Starts recording the device's screen to `path` (an absolute path on the node) until [`Self::stop_recording`].
    pub fn record(&mut self, path: &str) -> Result<(), String> {
        if self.recording.is_some() {
            return Err("a recording of this simulator is already running".into());
        }
        let args = Simctl::Record {
            device: self.device.clone(),
            path: PathBuf::from(path),
        }
        .args()?;
        let pid = self.runner.start(&args)?;
        self.recording = Some((pid, path.to_string()));
        Ok(())
    }

    /// Ends the recording (the tool finishes its file when it is interrupted) and answers the file's path.
    pub fn stop_recording(&mut self) -> Result<Option<String>, String> {
        let Some((pid, path)) = self.recording.take() else {
            return Ok(None);
        };
        self.runner.stop(pid)?;
        Ok(Some(path))
    }
}

impl Drop for Simulator<'_> {
    fn drop(&mut self) {
        if let Err(why) = self.stop_recording() {
            eprintln!("apid: a simulator recording of {}: {why}", self.device);
        }
    }
}

impl Target for Simulator<'_> {
    fn kind(&self) -> &str {
        "simulator"
    }

    fn tree(&mut self) -> Result<Tree, String> {
        self.bridge.tree()
    }

    fn screenshot(&mut self) -> Result<Screenshot, String> {
        let n = SHOTS.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from(format!("/tmp/apz-simulator-shot-{n}.png"));
        exec(
            self.runner.as_ref(),
            &Simctl::Screenshot {
                device: self.device.clone(),
                path: path.clone(),
            },
        )?;
        let bytes = self.runner.fetch(&path)?;
        Ok(Screenshot {
            mime: "image/png".into(),
            bytes,
        })
    }

    fn act(&mut self, action: &Action) -> Result<(), String> {
        self.bridge.act(action)
    }

    fn clock_ms(&mut self) -> Option<u64> {
        Some(self.started.elapsed().as_millis() as u64)
    }
}
