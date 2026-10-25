//! The iPhone target (PLAN 19.3, 8.5): a physical iPhone on a Mac node, through Xcode's `devicectl`. Needs Xcode on the node.
//!
//! The step leases the device for its agent as an Android phone is (`station_lease` with kind `device`, the UDID as its
//! serial), installs the app when the step names it, and launches it fresh for each scenario. The scenarios look at the
//! pictures: `devicectl device screenshot` (assumed, see `Devicectl::Screenshot`) gives the checker the screen. The tree is
//! not read, because the accessibility bridge (`simulator::Bridge`) is not wired to a physical device, and so the driver has
//! nothing to press: a tap is refused with that reason. Every line runs through the station's shell as the step's agent
//! (`desk::StationRunner`, the program `xcrun`), so the station's laws and audit apply to it.

use std::path::{Path, PathBuf};
use std::time::Instant;

use apz_desktop_macos::simctl::{Devicectl, Runner};

use super::android::{Held, open_device};
use super::desk::{AppCalls, Calls, StationRunner, release_lease};
use super::target::{Action, Screenshot, Target, Tree};
use crate::recipes::model::IphoneTarget;
use crate::state::App;

/// Where a step's pictures are written on the node.
const NODE_SCRATCH: &str = "/tmp";

/// Why a tap is refused: a physical iPhone's input is the accessibility bridge's, which is not wired.
pub const INPUT_UNWIRED: &str = "a physical iPhone takes no taps or typing yet: its input needs the accessibility bridge (simulator::Bridge), which is not wired to devicectl";

/// A physical iPhone with its app launched, for one scenario.
pub struct Phone<'a> {
    runner: Box<dyn Runner + 'a>,
    udid: String,
    scratch: PathBuf,
    shots: u64,
    started: Instant,
}

impl<'a> Phone<'a> {
    /// Launches `bundle` on the device `udid`, and answers the target. `scratch` is a directory the runner can read.
    pub fn open(
        runner: Box<dyn Runner + 'a>,
        udid: &str,
        scratch: &Path,
        bundle: &str,
    ) -> Result<Self, String> {
        let target = Self {
            runner,
            udid: udid.to_string(),
            scratch: scratch.to_path_buf(),
            shots: 0,
            started: Instant::now(),
        };
        target.send(&Devicectl::Launch {
            device: udid.to_string(),
            bundle: bundle.to_string(),
        })?;
        Ok(target)
    }

    fn send(&self, command: &Devicectl) -> Result<String, String> {
        self.runner.run(&command.args()?)
    }
}

impl Target for Phone<'_> {
    fn kind(&self) -> &str {
        "iphone"
    }

    /// No tree: the checker looks at the pictures instead (the tree of a physical device is not read yet).
    fn tree(&mut self) -> Result<Tree, String> {
        Ok(Tree::default())
    }

    fn screenshot(&mut self) -> Result<Screenshot, String> {
        self.shots += 1;
        let path = self
            .scratch
            .join(format!("apz-iphone-shot-{}.png", self.shots));
        self.send(&Devicectl::Screenshot {
            device: self.udid.clone(),
            path: path.clone(),
        })?;
        let bytes = self.runner.fetch(&path)?;
        Ok(Screenshot {
            mime: "image/png".into(),
            bytes,
        })
    }

    fn act(&mut self, _action: &Action) -> Result<(), String> {
        Err(INPUT_UNWIRED.to_string())
    }

    fn clock_ms(&mut self) -> Option<u64> {
        Some(self.started.elapsed().as_millis() as u64)
    }
}

/// Before a step's scenarios: the device is leased, and the app installed when the step names it. A failure releases the
/// lease again.
pub fn prepare(app: &App, config: &IphoneTarget) -> Result<Held, String> {
    let calls = AppCalls::new(app, config.agent);
    let runner = StationRunner::new(app, config.agent, &config.station);
    prepare_on(&calls, &runner, config)
}

/// [`prepare`] over the station's calls and the node's runner (production) or fakes (a test).
pub fn prepare_on(
    calls: &dyn Calls,
    runner: &dyn Runner,
    config: &IphoneTarget,
) -> Result<Held, String> {
    let lease = open_device(calls, &config.station, &config.udid, config.task.as_deref())?;
    if let Some(app_path) = &config.app {
        let install = Devicectl::Install {
            device: config.udid.clone(),
            app: PathBuf::from(app_path),
        };
        let installed = install.args().and_then(|args| runner.run(&args));
        if let Err(why) = installed {
            let _ = release_lease(calls, &config.station, &lease);
            return Err(why);
        }
    }
    Ok(Held {
        lease,
        recording: None,
    })
}

/// After a step's scenarios: the device's lease is released. The failure is said in the answer, not raised.
pub fn finish(app: &App, config: &IphoneTarget, held: Option<Held>) -> String {
    let calls = AppCalls::new(app, config.agent);
    finish_on(&calls, config, held)
}

/// [`finish`] over the calls given.
pub fn finish_on(calls: &dyn Calls, config: &IphoneTarget, held: Option<Held>) -> String {
    match held {
        Some(held) => match release_lease(calls, &config.station, &held.lease) {
            Ok(()) => String::new(),
            Err(why) => format!("the device lease did not release: {why}"),
        },
        None => String::new(),
    }
}

/// The opener of an iPhone target: each scenario launches the app fresh on the device.
pub fn opener<'a>(app: &'a App, config: &'a IphoneTarget) -> super::Opener<'a> {
    Box::new(move || -> Result<Box<dyn Target + 'a>, String> {
        let runner = StationRunner::new(app, config.agent, &config.station);
        let target = Phone::open(
            Box::new(runner),
            &config.udid,
            Path::new(NODE_SCRATCH),
            &config.bundle,
        )?;
        Ok(Box::new(target))
    })
}
