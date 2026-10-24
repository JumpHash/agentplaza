//! The Android target (PLAN 19.3, 8.5): an emulator or a phone, named by its adb serial, on a node. The step leases the
//! device for its agent as a workstation is leased (`station_lease` with kind `device`), installs the app when the step
//! names its APK, and records the device's screen when asked. Each scenario starts the app fresh and reads the screen with
//! `uiautomator dump`, presses with `input tap` and `input keyevent`, types with `input text` and pictures with `screencap`.
//!
//! PLAN 28.1 point 8 (the emulator's host): the first path taken is "on the node directly, under a sandbox". The emulator or
//! the phone is attached to the node itself, and adb runs there as the step's agent, through the station's shell
//! (`app_script`, as the simulator's `xcrun` runs, see `desk.rs`). Each line is one call as that agent, so the station's
//! laws and audit apply to it. The node's own confinement (the Seatbelt profile of 7.2) is not applied to adb here. The other
//! path (an emulator in a KVM fork with nested virtualisation) is not built.
//!
//! The adb lines and the runner are `apz-android`'s. The node runs them through [`NodeAdb`]; a test runs them on a fake adb.
//! The accessibility bridge is adb's own: the tree is the `uiautomator` dump, read into nodes (`apz_android::uiautomator`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use apz_android::adb::{self, Adb, RECORD_MAX_S, Runner};
use apz_android::uiautomator::{self, Node};
use apz_desktop_macos::simctl;
use serde_json::json;

use super::desk::{AppCalls, Calls, StationRunner, release_lease};
use super::target::{Action, Element, Screenshot, Target, Tree};
use crate::artifacts::{Budget, Refusal};
use crate::recipes::model::AndroidTarget;
use crate::state::App;

/// Where a step's scratch files live on the node (screenshots pulled from the device, the step's video).
const NODE_SCRATCH: &str = "/tmp";
/// Where the device writes the step's video before it is pulled to the node.
const DEVICE_VIDEO: &str = "/sdcard/apz-step.mp4";

/// adb on a node: the station's shell as the step's agent, with `adb` as the program. Its lines are the station runner's: a
/// command to its end, a background process, a file.
pub struct NodeAdb<'a>(StationRunner<'a>);

impl<'a> NodeAdb<'a> {
    pub fn new(app: &'a App, cid: u64, station: &str) -> Self {
        Self(StationRunner::over_program(
            Box::new(AppCalls::new(app, cid)),
            station,
            adb::ADB,
        ))
    }

    /// The node adb over `calls` (a script in a test).
    #[cfg(test)]
    pub fn over(calls: Box<dyn Calls + 'a>, station: &str) -> Self {
        Self(StationRunner::over_program(calls, station, adb::ADB))
    }
}

impl Runner for NodeAdb<'_> {
    fn run(&self, args: &[String]) -> Result<String, String> {
        simctl::Runner::run(&self.0, args)
    }

    fn start(&self, args: &[String]) -> Result<u32, String> {
        simctl::Runner::start(&self.0, args)
    }

    fn stop(&self, pid: u32) -> Result<(), String> {
        simctl::Runner::stop(&self.0, pid)
    }

    fn fetch(&self, path: &Path) -> Result<Vec<u8>, String> {
        simctl::Runner::fetch(&self.0, path)
    }
}

/// An Android device for one scenario: the app started fresh on it, and the screen's elements with the points a tap lands on.
pub struct Device<'a> {
    runner: Box<dyn Runner + 'a>,
    serial: String,
    scratch: PathBuf,
    shots: u64,
    /// The element ids of the last tree read, and each one's point.
    points: BTreeMap<String, (i32, i32)>,
    started: Instant,
}

impl<'a> Device<'a> {
    /// Ends the app's processes and starts `component` (`package/.Activity`) on the device `serial`. `scratch` is a directory
    /// the runner can read (on the node, or the test's own).
    pub fn open(
        runner: Box<dyn Runner + 'a>,
        serial: &str,
        scratch: &Path,
        component: &str,
    ) -> Result<Self, String> {
        let package = component.split('/').next().unwrap_or_default().to_string();
        let target = Self {
            runner,
            serial: serial.to_string(),
            scratch: scratch.to_path_buf(),
            shots: 0,
            points: BTreeMap::new(),
            started: Instant::now(),
        };
        target.send(&Adb::ForceStop { package })?;
        target.send(&Adb::Start {
            component: component.to_string(),
        })?;
        Ok(target)
    }

    fn send(&self, command: &Adb) -> Result<String, String> {
        adb::exec(self.runner.as_ref(), &self.serial, command)
    }
}

/// A node's role in the driver's terms (`button`, `link`, `textbox`, `checkbox`, `text`): what a person can act on, and what
/// they read.
fn role_of(node: &Node) -> &'static str {
    let class = node.class.rsplit('.').next().unwrap_or_default();
    if class.ends_with("EditText") {
        "textbox"
    } else if class.ends_with("CheckBox")
        || class.ends_with("Switch")
        || class.ends_with("ToggleButton")
    {
        "checkbox"
    } else if class.ends_with("Button") || node.clickable {
        "button"
    } else {
        "text"
    }
}

/// The tree's elements, and each one's point. A node is kept when a person can act on it or read it. An element's id is its
/// resource id's name when that is unique on the screen (`com.shop:id/pay` is `pay`), and its path in the dump otherwise.
pub fn elements_of(nodes: &[Node]) -> (Vec<Element>, BTreeMap<String, (i32, i32)>) {
    let kept: Vec<&Node> = nodes
        .iter()
        .filter(|n| n.clickable || n.checkable || !n.text.is_empty() || !n.content_desc.is_empty())
        .collect();
    let short = |n: &Node| {
        n.resource_id
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string()
    };
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for node in &kept {
        *counts.entry(short(node)).or_insert(0) += 1;
    }
    let mut elements = Vec::new();
    let mut points = BTreeMap::new();
    for node in kept {
        let name = short(node);
        let id = if !name.is_empty() && counts.get(&name) == Some(&1) {
            name
        } else {
            format!("n{}", node.path)
        };
        let role = role_of(node);
        let label = if node.content_desc.is_empty() {
            node.text.clone()
        } else {
            node.content_desc.clone()
        };
        let value = if role == "textbox" {
            node.text.clone()
        } else {
            String::new()
        };
        points.insert(id.clone(), node.bounds.center());
        elements.push(Element {
            id,
            role: role.into(),
            label,
            value,
        });
    }
    (elements, points)
}

/// The keys a driver may press, by the names the scenarios use.
fn keycode(key: &str) -> Result<&'static str, String> {
    Ok(match key {
        "back" => "KEYCODE_BACK",
        "home" => "KEYCODE_HOME",
        "enter" => "KEYCODE_ENTER",
        "tab" => "KEYCODE_TAB",
        "delete" => "KEYCODE_DEL",
        other => return Err(format!("the Android target does not press {other}")),
    })
}

impl Target for Device<'_> {
    fn kind(&self) -> &str {
        "android"
    }

    fn tree(&mut self) -> Result<Tree, String> {
        let dump = self.send(&Adb::UiDump)?;
        let nodes = uiautomator::parse_dump(&dump)?;
        // The screen's title is its first words of text, in document order.
        let title = nodes
            .iter()
            .find(|n| !n.text.is_empty())
            .map(|n| n.text.clone())
            .unwrap_or_default();
        let (elements, points) = elements_of(&nodes);
        self.points = points;
        Ok(Tree { title, elements })
    }

    fn screenshot(&mut self) -> Result<Screenshot, String> {
        self.shots += 1;
        let device = format!("/sdcard/apz-shot-{}.png", self.shots);
        let node = self
            .scratch
            .join(format!("apz-android-shot-{}.png", self.shots));
        self.send(&Adb::Screencap {
            path: device.clone(),
        })?;
        self.send(&Adb::Pull {
            device_path: device.clone(),
            local: node.clone(),
        })?;
        self.send(&Adb::Remove { path: device })?;
        let bytes = self.runner.fetch(&node)?;
        Ok(Screenshot {
            mime: "image/png".into(),
            bytes,
        })
    }

    fn act(&mut self, action: &Action) -> Result<(), String> {
        let point = |element: &str| {
            self.points
                .get(element)
                .copied()
                .ok_or_else(|| format!("no element {element} on the screen"))
        };
        match action {
            Action::Click { element } => {
                let (x, y) = point(element)?;
                self.send(&Adb::Tap { x, y })?;
            }
            Action::Type { element, text } => {
                let (x, y) = point(element)?;
                self.send(&Adb::Tap { x, y })?;
                self.send(&Adb::Text { text: text.clone() })?;
            }
            Action::Press { key } => {
                let code = keycode(key)?;
                self.send(&Adb::Key { code: code.into() })?;
            }
        }
        Ok(())
    }

    fn clock_ms(&mut self) -> Option<u64> {
        Some(self.started.elapsed().as_millis() as u64)
    }
}

/// A lease of a device on a node, and the step's recording of it: what a step keeps from its setup to its end.
#[derive(Debug, Default)]
pub struct Held {
    lease: String,
    recording: Option<(u32, PathBuf)>,
}

/// Leases the device `serial` on `station` for `task`. A device lease is active at once; one that another agent holds is
/// refused, and the lease is released again if it does not come back active.
fn open_device(
    calls: &dyn Calls,
    station: &str,
    serial: &str,
    task: Option<&str>,
) -> Result<String, String> {
    let mut input = json!({"station": station, "kind": "device", "device": serial});
    if let Some(task) = task {
        input["task"] = json!(task);
    }
    let view = calls.call("station_lease", &input)?;
    let id = view["id"]
        .as_str()
        .ok_or("the station answered a lease with no id")?
        .to_string();
    let state = view["state"].as_str().unwrap_or("unknown");
    if state != "active" {
        let _ = release_lease(calls, station, &id);
        return Err(format!(
            "the device {serial} on {station} was not granted (the lease is {state}: another agent holds it)"
        ));
    }
    Ok(id)
}

/// Before a step's scenarios: the device is leased, the app installed when the step names its APK, and the screen recorded
/// when the step asks for it. A failure releases the lease again.
pub fn prepare(app: &App, config: &AndroidTarget) -> Result<Held, String> {
    let calls = AppCalls::new(app, config.agent);
    let runner = NodeAdb::new(app, config.agent, &config.station);
    prepare_on(&calls, &runner, config, Path::new(NODE_SCRATCH))
}

/// [`prepare`] over the station's calls and the node's adb (production) or fakes (a test). `scratch` is the node's directory
/// the step's video is pulled to.
pub fn prepare_on(
    calls: &dyn Calls,
    runner: &dyn Runner,
    config: &AndroidTarget,
    scratch: &Path,
) -> Result<Held, String> {
    let lease = open_device(
        calls,
        &config.station,
        &config.serial,
        config.task.as_deref(),
    )?;
    match set_up(runner, config, scratch) {
        Ok(recording) => Ok(Held { lease, recording }),
        Err(why) => {
            let _ = release_lease(calls, &config.station, &lease);
            Err(why)
        }
    }
}

fn set_up(
    runner: &dyn Runner,
    config: &AndroidTarget,
    scratch: &Path,
) -> Result<Option<(u32, PathBuf)>, String> {
    if let Some(apk) = &config.apk {
        adb::exec(
            runner,
            &config.serial,
            &Adb::Install {
                apk: PathBuf::from(apk),
            },
        )?;
    }
    if !config.record {
        return Ok(None);
    }
    let node = scratch.join("apz-android-step.mp4");
    let args = Adb::Record {
        path: DEVICE_VIDEO.into(),
        seconds: RECORD_MAX_S,
    }
    .args(&config.serial)?;
    let pid = runner.start(&args)?;
    Ok(Some((pid, node)))
}

/// Ends the step's recording and lands it as an artifact of the run, then releases the lease. Each failure is said in the
/// answer, not raised: the step's result stands.
pub fn finish(app: &App, config: &AndroidTarget, held: Option<Held>) -> String {
    let calls = AppCalls::new(app, config.agent);
    let runner = NodeAdb::new(app, config.agent, &config.station);
    finish_on(&calls, &runner, config, held, |bytes| {
        app.artifacts
            .put_blob_within(Some(config.agent), "video/mp4", bytes, &Budget::unlimited())
            .map(|blob| blob.id)
            .map_err(|refusal| match refusal {
                Refusal::Budget(why) => why,
                Refusal::Problems(problems) => problems.join("; "),
            })
    })
}

/// [`finish`] over the calls and the runner given. `store` takes the video's bytes as an artifact and answers its id.
pub fn finish_on(
    calls: &dyn Calls,
    runner: &dyn Runner,
    config: &AndroidTarget,
    held: Option<Held>,
    store: impl FnOnce(&[u8]) -> Result<String, String>,
) -> String {
    let Some(held) = held else {
        return String::new();
    };
    let mut notes = Vec::new();
    if let Some((pid, node)) = held.recording {
        match land_recording(runner, &config.serial, pid, &node, store) {
            Ok(id) => notes.push(format!(
                "the recording of {} is artifact {id}",
                config.serial
            )),
            Err(why) => notes.push(format!(
                "the recording of {} did not land: {why}",
                config.serial
            )),
        }
    }
    if let Err(why) = release_lease(calls, &config.station, &held.lease) {
        notes.push(format!("the device lease did not release: {why}"));
    }
    notes.join("; ")
}

/// Ends the recording `pid` (the device's screenrecord is interrupted, and the local adb process that carries it ends), pulls
/// its file to the node, and hands its bytes to `store`.
fn land_recording<T>(
    runner: &dyn Runner,
    serial: &str,
    pid: u32,
    node: &Path,
    store: impl FnOnce(&[u8]) -> Result<T, String>,
) -> Result<T, String> {
    adb::exec(runner, serial, &Adb::StopRecord)?;
    runner.stop(pid)?;
    adb::exec(
        runner,
        serial,
        &Adb::Pull {
            device_path: DEVICE_VIDEO.into(),
            local: node.to_path_buf(),
        },
    )?;
    adb::exec(
        runner,
        serial,
        &Adb::Remove {
            path: DEVICE_VIDEO.into(),
        },
    )?;
    let bytes = runner.fetch(node)?;
    store(&bytes)
}

/// The opener of an Android target: each scenario starts the app fresh on the device.
pub fn opener<'a>(app: &'a App, config: &'a AndroidTarget) -> super::Opener<'a> {
    Box::new(move || -> Result<Box<dyn Target + 'a>, String> {
        let runner = NodeAdb::new(app, config.agent, &config.station);
        let target = Device::open(
            Box::new(runner),
            &config.serial,
            Path::new(NODE_SCRATCH),
            &config.component,
        )?;
        Ok(Box::new(target))
    })
}
