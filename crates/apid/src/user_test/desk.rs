//! The plumbing of the desktop targets (PLAN 19.3, W47): a macOS app and an iOS simulator run on a Mac node, and the run
//! reaches the node only as its own agent. Every call is a workstation tool (`stations/tools.rs`), so the station's choke
//! point checks the agent's lease and laws and audits the call, as it does for the agent's own work. Nothing here goes to
//! the owner's client (cid 0), and a target names the agent it acts as in its config.

use std::path::{Path, PathBuf};

use apz_desktop_macos::simctl::{self, Runner, Simctl, XCRUN, shell_line};
use base64::Engine as _;
use serde_json::{Value, json};

use super::Opener;
use super::macos::{self, MacApp};
use super::simulator::{self, Simulator, Unwired};
use super::target::Target;
use crate::artifacts::{Budget, Refusal};
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

/// Opens an owner-screen lease on `station` for the calls' agent, and answers its id. The lease is the owner's own screen
/// (the Mac daemon serves no virtual display). A lease still queued when the station answers is released again: the step
/// does not run without its session.
pub fn open_lease(calls: &dyn Calls, station: &str, task: Option<&str>) -> Result<String, String> {
    let mut input = json!({"station": station, "kind": "owner_screen"});
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
            "the owner's screen on {station} was not granted (the lease is {state}, and the screen is not free)"
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
    /// The program every line starts with on the node: `xcrun` for simctl, `adb` for an Android device.
    program: String,
}

impl<'a> StationRunner<'a> {
    pub fn new(app: &'a App, cid: u64, station: &str) -> Self {
        Self::over(Box::new(AppCalls::new(app, cid)), station)
    }

    /// The runner over `calls` (the workstation tools of the production path; a script in a test).
    pub fn over(calls: Box<dyn Calls + 'a>, station: &str) -> Self {
        Self::over_program(calls, station, XCRUN)
    }

    /// The runner over `calls` for the program `program` on the node (`xcrun`, or `adb`).
    pub fn over_program(calls: Box<dyn Calls + 'a>, station: &str, program: &str) -> Self {
        Self {
            calls,
            station: station.to_string(),
            program: program.to_string(),
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

/// The line as the node runs it: the program and its arguments.
fn tool_line(program: &str, args: &[String]) -> Vec<String> {
    std::iter::once(program.to_string())
        .chain(args.iter().cloned())
        .collect()
}

impl Runner for StationRunner<'_> {
    fn run(&self, args: &[String]) -> Result<String, String> {
        let (exit, output) = self.sh(&shell_line(&tool_line(&self.program, args)), RUN_MS)?;
        if exit == 0 {
            Ok(output)
        } else {
            Err(format!(
                "{} {} failed on {}: {}",
                self.program,
                args.join(" "),
                self.station,
                output.trim()
            ))
        }
    }

    fn start(&self, args: &[String]) -> Result<u32, String> {
        let script = format!(
            "{} </dev/null >/dev/null 2>&1 & echo $!",
            shell_line(&tool_line(&self.program, args))
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
            .map_err(|e| format!("the file from {}: {e}", self.station))
    }
}

/// The opener of a macOS target: each scenario leases the owner's screen, launches the app fresh from its bundle path and
/// reads it. The lease ends with the target.
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
            &mac.bundle,
            &mac.path,
            mac.depth,
        );
        target.relaunch()?;
        Ok(Box::new(target))
    })
}

/// The opener of a simulator target: each scenario launches the app fresh in the device.
fn simulator_opener<'a>(app: &'a App, sim: &'a SimTarget) -> Opener<'a> {
    Box::new(move || -> Result<Box<dyn Target + 'a>, String> {
        let runner = StationRunner::new(app, sim.agent, &sim.station);
        let target = Simulator::launch(
            Box::new(runner),
            Box::new(Unwired),
            &sim.device,
            &sim.bundle,
        )?;
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

/// What a step keeps from its setup until its end: the simulator's recording, as the process on the node that writes it
/// and the file it writes.
#[derive(Debug, Default)]
pub struct Setup {
    recording: Option<(u32, String)>,
    /// An Android device's lease and recording (`android.rs`), held from the step's start to its end.
    android: Option<super::android::Held>,
}

/// Before a step's scenarios: a simulator is booted, the app installed when the step gives its path, and the step's video
/// started when it names one. A failure shuts the simulator down again. A Mac app needs nothing up front.
pub fn prepare(app: &App, config: &UserTest) -> Result<Setup, String> {
    if let Some(android) = &config.android {
        return super::android::prepare(app, android).map(|held| Setup {
            recording: None,
            android: Some(held),
        });
    }
    let Some(sim) = &config.simulator else {
        return Ok(Setup::default());
    };
    let runner = StationRunner::new(app, sim.agent, &sim.station);
    match set_up(&runner, sim) {
        Ok(recording) => Ok(Setup {
            recording,
            android: None,
        }),
        Err(why) => {
            let _ = simctl::exec(
                &runner,
                &Simctl::Shutdown {
                    device: sim.device.clone(),
                },
            );
            Err(why)
        }
    }
}

fn set_up(runner: &dyn Runner, sim: &SimTarget) -> Result<Option<(u32, String)>, String> {
    simctl::exec(
        runner,
        &Simctl::Boot {
            device: sim.device.clone(),
        },
    )?;
    if let Some(path) = &sim.app {
        simctl::exec(
            runner,
            &Simctl::Install {
                device: sim.device.clone(),
                app: PathBuf::from(path),
            },
        )?;
    }
    match &sim.video {
        Some(video) => Ok(Some((
            simulator::start_recording(runner, &sim.device, video)?,
            video.clone(),
        ))),
        None => Ok(None),
    }
}

/// After a step's scenarios: a Mac app is quit, a simulator's video is stopped and landed as an artifact of the run, and the
/// simulator is shut down. Each step's failure is said in the answer, not raised: the step's result stands.
pub fn finish(app: &App, config: &UserTest, setup: Setup) -> String {
    if let Some(android) = &config.android {
        return super::android::finish(app, android, setup.android);
    }
    let mut notes = Vec::new();
    if let Some(mac) = &config.macos
        && let Err(why) = macos::quit(&AppCalls::new(app, mac.agent), &mac.station, &mac.bundle)
    {
        notes.push(why);
    }
    if let Some(sim) = &config.simulator {
        let runner = StationRunner::new(app, sim.agent, &sim.station);
        if let Some((pid, path)) = setup.recording {
            let landed = simulator::land_recording(&runner, pid, &path, |bytes| {
                app.artifacts
                    .put_blob_within(
                        Some(sim.agent),
                        "video/quicktime",
                        bytes,
                        &Budget::unlimited(),
                    )
                    .map(|blob| blob.id)
                    .map_err(|refusal| match refusal {
                        Refusal::Budget(why) => why,
                        Refusal::Problems(problems) => problems.join("; "),
                    })
            });
            match landed {
                Ok(id) => notes.push(format!("the recording of {} is artifact {id}", sim.device)),
                Err(why) => notes.push(format!(
                    "the recording of {} did not land: {why}",
                    sim.device
                )),
            }
        }
        if let Err(why) = simctl::exec(
            &runner,
            &Simctl::Shutdown {
                device: sim.device.clone(),
            },
        ) {
            notes.push(format!("{} did not shut down: {why}", sim.device));
        }
    }
    notes.join("; ")
}
