//! The plumbing of the desktop targets (PLAN 19.3, W47): a macOS app and an iOS simulator run on a Mac node, and the run
//! reaches the node only as its own agent. Every call is a workstation tool (`stations/tools.rs`), so the station's choke
//! point checks the agent's pixel lease and laws and audits the call, as it does for the agent's own work. Nothing here
//! goes to the owner's client (cid 0), and a target names the agent it acts as in its config.

use std::path::{Path, PathBuf};

use apz_desktop_macos::simctl::{self, Runner, Simctl, XCRUN, shell_line};
use base64::Engine as _;
use serde_json::{Value, json};

use super::Opener;
use super::macos::MacApp;
use super::simulator::{Simulator, Unwired};
use super::target::Target;
use crate::recipes::model::{MacTarget, SimTarget, UserTest};
use crate::state::App;

/// The longest a simctl line may run on the node, and a background start's answer.
const RUN_MS: u64 = 300_000;
const START_MS: u64 = 30_000;

/// The calls a desktop target makes on its station. Production sends them to the workstation tools as the target's agent;
/// a test answers from a script.
pub trait Calls {
    fn call(&self, tool: &str, input: &Value) -> Result<Value, String>;
}

/// The workstation tools, as the agent `cid`: the station's choke point applies that agent's lease, its laws and its audit.
pub struct AppCalls<'a> {
    app: &'a App,
    cid: u64,
}

impl<'a> AppCalls<'a> {
    pub fn new(app: &'a App, cid: u64) -> Self {
        Self { app, cid }
    }
}

impl Calls for AppCalls<'_> {
    fn call(&self, tool: &str, input: &Value) -> Result<Value, String> {
        crate::stations::tools::call(self.app, self.cid, tool, input)
    }
}

impl<T: Calls + ?Sized> Calls for &T {
    fn call(&self, tool: &str, input: &Value) -> Result<Value, String> {
        (**self).call(tool, input)
    }
}

/// Opens a pixel lease on `station` for the calls' agent, and answers its id. A lease still queued when the station answers
/// is released again: the scenario does not run without its session.
pub fn open_lease(calls: &dyn Calls, station: &str, task: Option<&str>) -> Result<String, String> {
    let mut input = json!({"station": station, "kind": "pixel"});
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
            "no pixel session on {station} was granted (the lease is {state}, and the station's session is not free)"
        ));
    }
    Ok(id)
}

/// Releases a lease the calls' agent holds on `station`.
pub fn release_lease(calls: &dyn Calls, station: &str, lease: &str) -> Result<(), String> {
    calls
        .call(
            "station_release",
            &json!({"station": station, "lease": lease}),
        )
        .map(|_| ())
}

/// simctl on a Mac node, through the station's shell: `app_script` with the cli bridge, as the run's agent. Each line runs to
/// its end, or a background process starts with `&` and is interrupted by its pid.
pub struct StationRunner<'a> {
    calls: Box<dyn Calls + 'a>,
    station: String,
}

impl<'a> StationRunner<'a> {
    pub fn new(app: &'a App, cid: u64, station: &str) -> Self {
        Self::over(Box::new(AppCalls::new(app, cid)), station)
    }

    /// The runner over `calls` (the workstation tools of the production path; a script in a test).
    pub fn over(calls: Box<dyn Calls + 'a>, station: &str) -> Self {
        Self {
            calls,
            station: station.to_string(),
        }
    }

    /// Runs `script` on the node: its exit code and its output.
    fn sh(&self, script: &str, timeout_ms: u64) -> Result<(i64, String), String> {
        let answer = self.calls.call(
            "app_script",
            &json!({
                "station": self.station,
                "bridge": "cli",
                "script": script,
                "timeout_ms": timeout_ms,
            }),
        )?;
        if answer["timed_out"].as_bool() == Some(true) {
            return Err(format!("a command on {} ran past its time", self.station));
        }
        let exit = answer["exit_code"].as_i64().unwrap_or(-1);
        let output = answer["output"].as_str().unwrap_or_default().to_string();
        Ok((exit, output))
    }
}

/// The xcrun line as the node runs it: `xcrun` and its arguments.
fn tool_line(args: &[String]) -> Vec<String> {
    std::iter::once(XCRUN.to_string())
        .chain(args.iter().cloned())
        .collect()
}

impl Runner for StationRunner<'_> {
    fn run(&self, args: &[String]) -> Result<String, String> {
        let (exit, output) = self.sh(&shell_line(&tool_line(args)), RUN_MS)?;
        if exit == 0 {
            Ok(output)
        } else {
            Err(format!(
                "{XCRUN} {} failed on {}: {}",
                args.join(" "),
                self.station,
                output.trim()
            ))
        }
    }

    fn start(&self, args: &[String]) -> Result<u32, String> {
        let script = format!(
            "{} </dev/null >/dev/null 2>&1 & echo $!",
            shell_line(&tool_line(args))
        );
        let (exit, output) = self.sh(&script, START_MS)?;
        if exit != 0 {
            return Err(format!(
                "could not start a background process on {}: {}",
                self.station,
                output.trim()
            ));
        }
        output
            .trim()
            .parse::<u32>()
            .map_err(|_| format!("the node answered no process id: {}", output.trim()))
    }

    fn stop(&self, pid: u32) -> Result<(), String> {
        // SIGINT finishes a recording's file; the wait is bounded, so a process that ignores it is reported, not waited for.
        let script = format!(
            "kill -INT {pid}; i=0; while kill -0 {pid} 2>/dev/null && [ $i -lt 150 ]; do sleep 0.2; i=$((i+1)); done; kill -0 {pid} 2>/dev/null && exit 1; exit 0"
        );
        let (exit, output) = self.sh(&script, RUN_MS)?;
        if exit == 0 {
            Ok(())
        } else {
            Err(format!(
                "process {pid} on {} did not stop: {}",
                self.station,
                output.trim()
            ))
        }
    }

    fn fetch(&self, path: &Path) -> Result<Vec<u8>, String> {
        let file = shell_line(&[path.display().to_string()]);
        let (exit, output) = self.sh(&format!("base64 < {file}; rm -f {file}"), RUN_MS)?;
        if exit != 0 {
            return Err(format!(
                "could not read {} on {}",
                path.display(),
                self.station
            ));
        }
        let text: String = output.split_whitespace().collect();
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .map_err(|e| format!("the picture from {}: {e}", self.station))
    }
}

/// The opener of a macOS target: each scenario leases a pixel session, launches the app fresh and reads it. The lease ends
/// with the target.
fn mac_opener<'a>(app: &'a App, mac: &'a MacTarget) -> Opener<'a> {
    Box::new(move || -> Result<Box<dyn Target + 'a>, String> {
        let lease = open_lease(
            &AppCalls::new(app, mac.agent),
            &mac.station,
            mac.task.as_deref(),
        )?;
        let target = MacApp::new(
            Box::new(AppCalls::new(app, mac.agent)),
            &mac.station,
            lease,
            &mac.app,
            mac.depth,
        );
        target.relaunch()?;
        Ok(Box::new(target))
    })
}

/// The opener of a simulator target: each scenario launches the app fresh in the device, and records it when the target
/// names a video.
fn simulator_opener<'a>(app: &'a App, sim: &'a SimTarget) -> Opener<'a> {
    Box::new(move || -> Result<Box<dyn Target + 'a>, String> {
        let runner = StationRunner::new(app, sim.agent, &sim.station);
        let mut target = Simulator::launch(
            Box::new(runner),
            Box::new(Unwired),
            &sim.device,
            &sim.bundle,
        )?;
        if let Some(video) = &sim.video {
            target.record(video)?;
        }
        Ok(Box::new(target))
    })
}

/// The opener of the step's desktop target, or an error when the step names none.
pub fn opener<'a>(app: &'a App, config: &'a UserTest) -> Result<Opener<'a>, String> {
    match (&config.macos, &config.simulator) {
        (Some(mac), _) => Ok(mac_opener(app, mac)),
        (None, Some(sim)) => Ok(simulator_opener(app, sim)),
        (None, None) => Err("the step names no desktop target".into()),
    }
}

/// Before a step's scenarios: a simulator is booted, and the app installed when the step gives its path. A Mac app needs
/// nothing up front; its lease is opened for each scenario.
pub fn prepare(app: &App, config: &UserTest) -> Result<(), String> {
    let Some(sim) = &config.simulator else {
        return Ok(());
    };
    let runner = StationRunner::new(app, sim.agent, &sim.station);
    simctl::exec(
        &runner,
        &Simctl::Boot {
            device: sim.device.clone(),
        },
    )?;
    if let Some(path) = &sim.app {
        simctl::exec(
            &runner,
            &Simctl::Install {
                device: sim.device.clone(),
                app: PathBuf::from(path),
            },
        )?;
    }
    Ok(())
}

/// After a step's scenarios: a simulator is shut down. A failure is reported, not raised: the step's result stands.
pub fn finish(app: &App, config: &UserTest) {
    let Some(sim) = &config.simulator else {
        return;
    };
    let runner = StationRunner::new(app, sim.agent, &sim.station);
    if let Err(why) = simctl::exec(
        &runner,
        &Simctl::Shutdown {
            device: sim.device.clone(),
        },
    ) {
        eprintln!("apid: user test simulator {}: {why}", sim.device);
    }
}
