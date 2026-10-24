//! Recipe files (PLAN 4.4, 21.1): a recipe is a graph of steps, kept as a TOML file. A built-in recipe ships in the hub
//! (`builtin/*.toml`); a project's own copy lives in its Weft repo at `.plaza/recipes/<id>.toml` and overrides the
//! built-in of the same id; the owner's new versions are saved in the hub (`runs.rs`).
//!
//! ```toml
//! id = "change"
//! version = 1
//! inputs = ["task"]
//! outputs = ["change"]
//!
//! [[steps]]
//! id = "build"
//! role = "builder"
//! model = "any"              # any | differ | a model (provider/name) | a list of models | eval
//! fan_out = 1                # a count, or per_file | per_target | per_finding
//! gate = ["passed"]          # every condition must hold (default: passed)
//!
//! [[edges]]
//! from = "build"
//! to = "review"
//! when = "passed"            # passed | failed | findings>0 | findings == 0 | score >= 3 | ...
//! max = 2                    # a loop edge: taken at most max times, and it rewinds its target
//! ```
//!
//! A step is named by its `id`; the seat it fills (`seat`, default its id) is what a task's sessions are addressed to.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::graph::Cond;
use crate::user_test::target::StandIn;

/// Where a recipe applies: the built-ins, a project's own, or every project's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    #[default]
    BuiltIn,
    Project,
    Every,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// What the recipe is for, and what of it runs (a stub says which phase fills it).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub version: u32,
    #[serde(default)]
    pub scope: Scope,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub outputs: Vec<String>,
    #[serde(default)]
    pub triggers: Vec<String>,
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(default)]
    pub edges: Vec<Edge>,
}

/// Which models a step's session may use: a name (`any`, `differ`, `eval`, or one model) or a list to choose from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Model {
    Name(String),
    List(Vec<String>),
}

impl Default for Model {
    fn default() -> Self {
        Model::Name("any".into())
    }
}

/// How many items a step runs: a fixed count, or one per file, target or finding of the run's input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Fan {
    Count(u32),
    Named(String),
}

impl Default for Fan {
    fn default() -> Self {
        Fan::Count(1)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    /// The seat the step's session fills (default: the step's id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat: Option<String>,
    /// The role of the session that runs the step. A step without one is run by its engine (`engine`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default)]
    pub model: Model,
    /// Who runs a step that is not a session of the work loop: `review_graph`, `base`, `curator`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    #[serde(default)]
    pub fan_out: Fan,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gate: Vec<String>,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub budget_share: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// For a fixed fan-out: how many more items a large or risky change runs (8.2: a third sceptic).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<Risk>,
    /// A `test:user` step's configuration (PLAN 19): its three roles' models, the target and the budgets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<UserTest>,
}

/// The engine of a step that runs user tests of a change (PLAN 19.1).
pub const USER_TEST: &str = "test:user";

/// A `test:user` step's configuration: the models of its three roles, the target its scenarios drive and the budgets.
/// The models are `provider/model` names (the gateway's).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UserTest {
    /// The planner: writes the scenarios from the acceptance, the past confirmed findings and an exploratory budget,
    /// and reads each failure.
    pub planner: String,
    /// The driver's fast picker, and the stronger model it escalates to with a screenshot.
    pub driver: String,
    pub escalate: String,
    /// The checker: looks at checkpoints, and at screens the target gives no tree for.
    pub checker: String,
    /// The target the scenarios drive. Without one the step is not run (it fails, and says why).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stand_in: Option<StandIn>,
    /// A web target (PLAN 19.3): a headless browser on the change's lane preview, at `path` under its address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web: Option<WebTarget>,
    /// A terminal target (PLAN 19.3): a program in a pseudo-terminal in the lane's test fork, typed into by the scenarios.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tui: Option<TuiTarget>,
    /// A desktop target (PLAN 19.3): a Linux or Windows station's screen, leased for the run and driven through its tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop: Option<DesktopTarget>,
    /// A macOS app target (PLAN 19.3): an app on a Mac node, read through the macOS Accessibility API and driven through
    /// the station's workstation tools, as the agent `agent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub macos: Option<MacTarget>,
    /// An iOS simulator target (PLAN 19.3): a device of a Mac node, driven through `simctl` and an accessibility bridge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub simulator: Option<SimTarget>,
    /// An Android target (PLAN 19.3, 8.5): an emulator or a phone on a node, named by its adb serial and leased for the step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub android: Option<AndroidTarget>,
    /// A device-farm run (PLAN 19.3, 8.5): an instrumentation suite on a farm's devices, through the farm's connector. Its
    /// results come back as the run's evidence, and the step has no scenarios.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub farm: Option<FarmTest>,
    /// An iPhone target (PLAN 19.3, 8.5): a physical iPhone on a Mac node, leased for the step like an Android phone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iphone: Option<IphoneTarget>,
    /// The spec's acceptance criteria, as the planner reads them: each becomes acceptance scenarios.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance: Vec<String>,
    /// Below this confidence the fast pick escalates (0 to 1).
    #[serde(default = "threshold")]
    pub threshold: f64,
    /// Most actions one scenario step takes.
    #[serde(default = "actions")]
    pub actions: u32,
    /// Most exploratory scenarios the planner may add.
    #[serde(default)]
    pub exploratory: u32,
}

fn threshold() -> f64 {
    0.6
}

fn actions() -> u32 {
    8
}

impl UserTest {
    /// The configuration must make sense before a run starts on it.
    pub fn check(&self) -> Result<(), String> {
        for (role, model) in [
            ("planner", &self.planner),
            ("driver", &self.driver),
            ("escalate", &self.escalate),
            ("checker", &self.checker),
        ] {
            if !model.contains('/') {
                return Err(format!("the {role} is a provider/model name, not {model}"));
            }
        }
        if !(self.threshold > 0.0 && self.threshold <= 1.0) {
            return Err("the driver's threshold is between 0 and 1".into());
        }
        if self.actions == 0 {
            return Err("a scenario step takes at least one action".into());
        }
        let named = [
            self.stand_in.is_some(),
            self.web.is_some(),
            self.tui.is_some(),
            self.desktop.is_some(),
            self.macos.is_some(),
            self.simulator.is_some(),
            self.android.is_some(),
            self.farm.is_some(),
            self.iphone.is_some(),
        ]
        .iter()
        .filter(|set| **set)
        .count();
        if named > 1 {
            return Err("a test:user step names one target: stand_in, web, tui, desktop, macos, simulator, android or farm".into());
        }
        if let Some(android) = &self.android {
            android.check()?;
        }
        if let Some(farm) = &self.farm {
            farm.check()?;
        }
        if let Some(phone) = &self.iphone {
            phone.check()?;
        }
        if let Some(desk) = &self.desktop {
            crate::user_test::desktop::kind_of(&desk.os)?;
            match (desk.os.as_str(), &desk.station) {
                (crate::user_test::desktop::LINUX, None) => {}
                (crate::user_test::desktop::LINUX, Some(_)) => {
                    return Err("a Linux desktop is the lane's fork, and names no station".into());
                }
                (_, Some(station)) if !station.trim().is_empty() => {}
                _ => return Err("a Windows desktop names its station".into()),
            }
        }
        if let Some(mac) = &self.macos {
            mac.check()?;
        }
        if let Some(sim) = &self.simulator {
            sim.check()?;
        }
        if let Some(web) = &self.web
            && web.path.contains("://")
        {
            return Err("a web target's path is under the lane's preview, not an address".into());
        }
        if let Some(tui) = &self.tui {
            if tui.command.first().is_none_or(|p| p.trim().is_empty()) {
                return Err("a terminal target names a program to run".into());
            }
            if tui.cols == 0 || tui.rows == 0 {
                return Err("a terminal target has a size of at least 1 by 1".into());
            }
        }
        match &self.stand_in {
            Some(stand) => stand.check(),
            None => Ok(()),
        }
    }
}

/// A web target's configuration (PLAN 19.3): the page of the lane's preview the scenarios start on. `path` is under the
/// preview's address (`/` or empty is its root). The browser reaches the preview through a link the hub mints for the run.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebTarget {
    #[serde(default)]
    pub path: String,
}

/// A terminal target's configuration (PLAN 19.3): the program (argv, its first item the program, run in the fork's
/// workspace) and the terminal's size in cells.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuiTarget {
    pub command: Vec<String>,
    #[serde(default = "columns")]
    pub cols: u16,
    #[serde(default = "rows")]
    pub rows: u16,
}

/// A macOS app target's configuration (PLAN 19.3). The app runs on the Mac node `station`, on the owner's screen (the lease
/// is `owner_screen`: the Mac has no virtual display). The target acts as the agent `agent`, so the station's laws and audit
/// apply to its calls as they do to that agent's. The lease is held for each scenario, and only the app's windows are read,
/// captured and pointed at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacTarget {
    pub station: String,
    pub agent: u64,
    /// The app, as the station lists it (its name): the scope of its tree, its pictures and its pointer input.
    pub app: String,
    /// Its bundle id: the app is quit by it at the end of the step, and brought to the front before keys and pointer input.
    pub bundle: String,
    /// Its bundle path on the node: the app is launched from it (`open -n -a`) for each scenario.
    pub path: String,
    /// The lease's task on the station, for its audit and its time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// How many levels of the app's tree each observation reads (1 to 8).
    #[serde(default = "tree_depth")]
    pub depth: u8,
}

fn tree_depth() -> u8 {
    4
}

impl MacTarget {
    fn check(&self) -> Result<(), String> {
        if self.agent == 0 {
            return Err(
                "a macOS target acts as an agent (its cid), never the owner's client (cid 0)"
                    .into(),
            );
        }
        if self.station.trim().is_empty() || self.app.trim().is_empty() {
            return Err("a macOS target names its station and its app".into());
        }
        let bundle_id = !self.bundle.is_empty()
            && self
                .bundle
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
        if !bundle_id {
            return Err("a macOS target's bundle is a bundle id (letters, digits, dots, dashes, underscores)".into());
        }
        if !self.path.starts_with('/') {
            return Err(
                "a macOS target's path is the app's bundle, an absolute path on the Mac node"
                    .into(),
            );
        }
        if !(1..=8).contains(&self.depth) {
            return Err("a macOS target reads between 1 and 8 levels of its tree".into());
        }
        Ok(())
    }
}

/// An iOS simulator target's configuration (PLAN 19.3). The device is a simulator of the Mac node `station`, booted for the
/// step and shut down after it; the app is installed from `app` (an absolute path on the node) when it is given, and
/// launched fresh for each scenario. `video` (an absolute path on the node) records each scenario to that file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimTarget {
    pub station: String,
    pub agent: u64,
    /// The device: a UDID, a name, or `booted`.
    pub device: String,
    /// The app's bundle id.
    pub bundle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<String>,
}

impl SimTarget {
    fn check(&self) -> Result<(), String> {
        if self.agent == 0 {
            return Err(
                "a simulator target acts as an agent (its cid), never the owner's client (cid 0)"
                    .into(),
            );
        }
        if self.station.trim().is_empty()
            || self.device.trim().is_empty()
            || self.bundle.trim().is_empty()
        {
            return Err(
                "a simulator target names its station, its device and its app's bundle id".into(),
            );
        }
        for (what, path) in [("the app", &self.app), ("the video", &self.video)] {
            if path.as_deref().is_some_and(|p| !p.starts_with('/')) {
                return Err(format!("{what} is an absolute path on the Mac node"));
            }
        }
        Ok(())
    }
}

/// An Android target's configuration (PLAN 19.3, 8.5). The device is the adb serial `serial` of the node `station` (an
/// emulator, or a phone plugged into the node). The step leases it for the agent `agent`, as a workstation is leased: the
/// lease is held from the step's start until its end, and the device is refused to another agent meanwhile. The app is
/// installed from `apk` (an absolute path on the node) when it is given, and started fresh (`component`, the full
/// `package/.Activity`) for each scenario. `record` records the device's screen for the whole step; the video lands as an
/// artifact of the run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AndroidTarget {
    pub station: String,
    pub agent: u64,
    pub serial: String,
    pub component: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apk: Option<String>,
    #[serde(default)]
    pub record: bool,
    /// The lease's task on the station, for its audit and its time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

impl AndroidTarget {
    pub fn check(&self) -> Result<(), String> {
        if self.agent == 0 {
            return Err(
                "an Android target acts as an agent (its cid), never the owner's client (cid 0)"
                    .into(),
            );
        }
        if self.station.trim().is_empty() || self.serial.trim().is_empty() {
            return Err("an Android target names its station and its device's serial".into());
        }
        if let Some(apk) = &self.apk
            && !apk.starts_with('/')
        {
            return Err("an Android target's apk is an absolute path on the node".into());
        }
        if !self.component.contains('/') {
            return Err("an Android target's component is package/.Activity".into());
        }
        Ok(())
    }
}

/// An iPhone target's configuration (PLAN 19.3, 8.5). The device is the physical iPhone `udid` (its UDID) attached to the Mac
/// node `station`: leased for the step as an Android phone is (`device` kind), its app installed from `app` (an absolute path
/// on the node) when given, and launched for each scenario (`bundle`). It needs Xcode on the node (`devicectl`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IphoneTarget {
    pub station: String,
    pub agent: u64,
    pub udid: String,
    pub bundle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

impl IphoneTarget {
    pub fn check(&self) -> Result<(), String> {
        if self.agent == 0 {
            return Err(
                "an iPhone target acts as an agent (its cid), never the owner's client (cid 0)"
                    .into(),
            );
        }
        if self.station.trim().is_empty()
            || self.udid.trim().is_empty()
            || self.bundle.trim().is_empty()
        {
            return Err(
                "an iPhone target names its station, its device's UDID and its app's bundle id"
                    .into(),
            );
        }
        if let Some(app) = &self.app
            && !app.starts_with('/')
        {
            return Err("an iPhone target's app is an absolute path on the Mac node".into());
        }
        Ok(())
    }
}

/// A device-farm run's configuration (PLAN 19.3, 8.5). `farm` is the farm's connector: `browserstack`, `firebase-test-lab`
/// or `aws-device-farm` (each with a manifest under `connectors/manifests/farms`). `app` and `tests` are the ids of blobs
/// in the hub's object store (the app's build and the instrumentation package), never paths on the hub. `devices` are the
/// farm's own device names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FarmTest {
    pub farm: String,
    pub app: String,
    pub tests: String,
    pub devices: Vec<String>,
}

impl FarmTest {
    pub fn check(&self) -> Result<(), String> {
        if !matches!(
            self.farm.as_str(),
            "browserstack" | "firebase-test-lab" | "aws-device-farm"
        ) {
            return Err(format!(
                "a device farm is browserstack, firebase-test-lab or aws-device-farm, not {}",
                self.farm
            ));
        }
        if self.app.trim().is_empty() || self.tests.trim().is_empty() {
            return Err("a device-farm run names its app's blob and its tests' blob".into());
        }
        if self.devices.is_empty() || self.devices.iter().any(|d| d.trim().is_empty()) {
            return Err("a device-farm run names at least one device".into());
        }
        Ok(())
    }
}

fn columns() -> u16 {
    100
}

fn rows() -> u16 {
    30
}

/// A desktop target's configuration (PLAN 19.3): the station whose desktop the scenarios drive (its id) and the operating
/// system that desktop runs, `linux` or `windows`. The station must be online and report that OS in its facts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopTarget {
    /// The station that runs a Windows desktop (its id). A Linux desktop is the lane's fork, and names none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station: Option<String>,
    pub os: String,
}

/// What makes a change large or risky: more than `lines` changed lines, or a path one of `paths` matches. Then the
/// step's fan-out grows by `extra` items.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Risk {
    pub lines: u32,
    #[serde(default)]
    pub paths: Vec<String>,
    pub extra: u32,
}

fn one() -> f64 {
    1.0
}

fn is_one(v: &f64) -> bool {
    *v == 1.0
}

impl Step {
    /// The seat the step's session fills.
    pub fn seat_name(&self) -> &str {
        self.seat.as_deref().unwrap_or(&self.id)
    }

    /// The gate's conditions: the step's own, or the default one (its verdict passed).
    pub fn gate_conditions(&self) -> Vec<String> {
        if self.gate.is_empty() {
            vec!["passed".to_string()]
        } else {
            self.gate.clone()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub when: String,
    /// A loop: taken at most this many times.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<u32>,
}

/// The file a project keeps its copy of a recipe in, in its Weft repo at main.
pub fn project_file(id: &str) -> String {
    format!(".plaza/recipes/{id}.toml")
}

impl Recipe {
    pub fn parse(text: &str) -> Result<Recipe, String> {
        let r: Recipe = toml::from_str(text).map_err(|e| format!("recipe: {e}"))?;
        r.check()?;
        Ok(r)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// `id@version`, as runs and seats name the recipe they run.
    pub fn key(&self) -> String {
        format!("{}@{}", self.id, self.version)
    }

    pub fn step(&self, id: &str) -> Option<&Step> {
        self.steps.iter().find(|s| s.id == id)
    }

    /// Whether a step has an edge into it that is not a loop (it starts only after that edge is taken).
    pub fn has_forward_edge_into(&self, step: &str) -> bool {
        self.edges.iter().any(|e| e.to == step && e.max.is_none())
    }

    /// A recipe must make sense before a run starts on it: unique ids, edges between steps that exist, conditions that
    /// parse, loops with a bound, forward edges that make no cycle, and a step to start with.
    pub fn check(&self) -> Result<(), String> {
        if self.id.trim().is_empty() || self.version == 0 {
            return Err("a recipe needs an id and a version".into());
        }
        if self.steps.is_empty() {
            return Err(format!("recipe {} has no steps", self.id));
        }
        let mut seats: BTreeMap<&str, &str> = BTreeMap::new();
        for (i, step) in self.steps.iter().enumerate() {
            if step.id.trim().is_empty() {
                return Err("a step needs an id".into());
            }
            if self.steps[..i].iter().any(|s| s.id == step.id) {
                return Err(format!("step {} appears twice", step.id));
            }
            if let Some(role) = &step.role {
                if role.trim().is_empty() {
                    return Err(format!("step {} has an empty role", step.id));
                }
                let seat = step.seat_name();
                if let Some(other) = seats.insert(seat, &step.id) {
                    return Err(format!(
                        "steps {other} and {} fill the same seat {seat}",
                        step.id
                    ));
                }
            } else if step.engine.is_none() {
                return Err(format!("step {} needs a role or an engine", step.id));
            }
            if step.risk.is_some() && !matches!(step.fan_out, Fan::Count(_)) {
                return Err(format!("step {}: a risk grows a fixed fan-out", step.id));
            }
            if let Fan::Named(named) = &step.fan_out
                && !matches!(named.as_str(), "per_file" | "per_target" | "per_finding")
            {
                return Err(format!(
                    "step {}: fan_out {named} is not a fan-out",
                    step.id
                ));
            }
            if let Model::Name(name) = &step.model
                && name.trim().is_empty()
            {
                return Err(format!("step {}: the model has no name", step.id));
            }
            if let Model::List(list) = &step.model
                && list.is_empty()
            {
                return Err(format!("step {}: a model list is empty", step.id));
            }
            if !(step.budget_share > 0.0 && step.budget_share <= 1.0) {
                return Err(format!("step {}: budget_share is between 0 and 1", step.id));
            }
            for condition in step.gate_conditions() {
                Cond::parse(&condition).map_err(|e| format!("step {} gate: {e}", step.id))?;
            }
            match (step.engine.as_deref() == Some(USER_TEST), &step.user) {
                (true, Some(user)) => user.check().map_err(|e| format!("step {}: {e}", step.id))?,
                (true, None) => {
                    return Err(format!(
                        "step {} is a test:user step with no [user] table",
                        step.id
                    ));
                }
                (false, Some(_)) => {
                    return Err(format!(
                        "step {}: a [user] table is for test:user steps",
                        step.id
                    ));
                }
                (false, None) => {}
            }
        }
        for edge in &self.edges {
            for end in [&edge.from, &edge.to] {
                if self.step(end).is_none() {
                    return Err(format!("an edge names {end}, which is not a step"));
                }
            }
            Cond::parse(&edge.when)
                .map_err(|e| format!("edge {} -> {}: {e}", edge.from, edge.to))?;
            if edge.max == Some(0) {
                return Err(format!(
                    "the loop {} -> {} has no bound",
                    edge.from, edge.to
                ));
            }
        }
        if self.steps.iter().all(|s| self.has_forward_edge_into(&s.id)) {
            return Err("a recipe needs a step that starts it".into());
        }
        // The forward edges (every edge but a loop) make no cycle: every step can be placed after the ones it follows.
        let mut placed: Vec<&str> = Vec::new();
        while placed.len() < self.steps.len() {
            let before = placed.len();
            for s in &self.steps {
                if placed.contains(&s.id.as_str()) {
                    continue;
                }
                let follows = self
                    .edges
                    .iter()
                    .filter(|e| e.to == s.id && e.max.is_none())
                    .all(|e| placed.contains(&e.from.as_str()));
                if follows {
                    placed.push(&s.id);
                }
            }
            if placed.len() == before {
                return Err("the recipe's edges make a cycle without a loop bound".into());
            }
        }
        Ok(())
    }
}

/// The built-in recipes, in the hub.
pub fn builtin() -> Vec<Recipe> {
    [
        include_str!("builtin/change.toml"),
        include_str!("builtin/review.toml"),
        include_str!("builtin/ci.toml"),
        include_str!("builtin/base.toml"),
        include_str!("builtin/distill.toml"),
        include_str!("builtin/intake.toml"),
        include_str!("builtin/plan.toml"),
        include_str!("builtin/release.toml"),
        include_str!("builtin/deploy.toml"),
        include_str!("builtin/incident.toml"),
        include_str!("builtin/upgrade.toml"),
        include_str!("builtin/cve.toml"),
        include_str!("builtin/engine_watch.toml"),
    ]
    .iter()
    .map(|t| Recipe::parse(t).expect("a built-in recipe parses"))
    .collect()
}

/// The built-in recipe named `id`.
pub fn builtin_named(id: &str) -> Option<Recipe> {
    builtin().into_iter().find(|r| r.id == id)
}

/// A project's copy of recipe `id`, from its repo's files at main: `listing` is Weft's file listing of the revision and
/// `fetch` reads one file. `Ok(None)` when the project keeps no copy.
pub fn in_listing(
    id: &str,
    listing: &serde_json::Value,
    fetch: impl FnOnce(&str) -> Result<Vec<u8>, String>,
) -> Result<Option<Recipe>, String> {
    let path = project_file(id);
    let present = listing["files"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|file| file["path"].as_str() == Some(path.as_str()));
    if !present {
        return Ok(None);
    }
    let text = String::from_utf8(fetch(&path)?).map_err(|_| format!("{path} is not UTF-8"))?;
    let recipe = Recipe::parse(&text).map_err(|e| format!("{path}: {e}"))?;
    if recipe.id != id {
        return Err(format!("{path} is recipe {}, not {id}", recipe.id));
    }
    if recipe.scope == Scope::BuiltIn {
        return Err(format!(
            "{path}: a project's recipe is project or every scope"
        ));
    }
    Ok(Some(recipe))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CHANGE: &str = r#"
id = "change"
version = 1
[[steps]]
id = "build"
role = "builder"
[[steps]]
id = "review"
seat = "reviewer"
role = "reviewer"
model = "differ"
[[edges]]
from = "build"
to = "review"
when = "passed"
[[edges]]
from = "review"
to = "build"
when = "failed"
max = 2
"#;

    #[test]
    fn the_built_in_recipes_check_and_round_trip() {
        let recipes = builtin();
        assert_eq!(recipes.len(), 13);
        // The intake and plan recipes are real (planning.rs): their steps are the engine's and the planner's.
        let intake = builtin_named("intake").unwrap();
        assert!(!intake.description.contains("Stub"), "intake is real");
        assert_eq!(
            intake
                .steps
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            ["classify", "task", "spec"]
        );
        let plan = builtin_named("plan").unwrap();
        assert!(!plan.description.contains("Stub"), "plan is real");
        assert!(
            plan.steps
                .iter()
                .all(|s| s.role.as_deref() == Some("planner"))
        );
        let deploy = builtin_named("deploy").unwrap();
        assert!(!deploy.description.contains("Stub"), "deploy is real");
        assert_eq!(deploy.steps[0].fan_out, Fan::Named("per_target".into()));
        // The incident recipe is real (incidents/): its four steps are the engine's.
        let incident = builtin_named("incident").unwrap();
        assert!(!incident.description.contains("Stub"), "incident is real");
        assert_eq!(
            incident
                .steps
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            ["triage", "act", "tell", "after"]
        );
        for stub in ["upgrade", "cve"] {
            let r = builtin_named(stub).unwrap_or_else(|| panic!("{stub} is a built-in"));
            assert!(
                r.description.contains("Stub until"),
                "{stub} says which phase fills it"
            );
            assert!(
                r.steps.iter().all(|s| s.engine.is_some()),
                "{stub} runs by its engine"
            );
        }
        for r in recipes {
            r.check().unwrap();
            assert_eq!(Recipe::parse(&r.to_toml()).unwrap(), r, "{}", r.id);
        }
        let change = builtin_named("change").unwrap();
        assert_eq!(change.steps[1].seat_name(), "reviewer");
        assert_eq!(change.steps[2].seat_name(), "verifier");
        assert_eq!(change.edges[2].max, Some(2));
    }

    #[test]
    fn a_bad_graph_is_refused_and_says_why() {
        let mut r = Recipe::parse(CHANGE).unwrap();
        r.edges.push(Edge {
            from: "review".into(),
            to: "nowhere".into(),
            when: "passed".into(),
            max: None,
        });
        assert!(r.check().unwrap_err().contains("nowhere"));

        // Without the loop bound, the two edges are a cycle, and no step starts the recipe.
        let mut r = Recipe::parse(CHANGE).unwrap();
        r.edges[1].max = None;
        assert!(r.check().unwrap_err().contains("starts it"));

        let mut r = Recipe::parse(CHANGE).unwrap();
        r.edges[1].max = Some(0);
        assert!(r.check().unwrap_err().contains("no bound"));

        let mut r = Recipe::parse(CHANGE).unwrap();
        r.steps[1].model = Model::Name(String::new());
        assert!(r.check().unwrap_err().contains("no name"));
        r.steps[1].model = Model::List(Vec::new());
        assert!(r.check().unwrap_err().contains("model list is empty"));
        r.steps[1].model = Model::List(vec!["a/one".into(), "b/two".into()]);
        assert!(r.check().is_ok());

        let mut r = Recipe::parse(CHANGE).unwrap();
        r.steps[0].gate = vec!["looks good".into()];
        assert!(r.check().unwrap_err().contains("not a condition"));

        let mut r = Recipe::parse(CHANGE).unwrap();
        r.steps[0].seat = Some("reviewer".into());
        assert!(r.check().unwrap_err().contains("same seat"));
    }

    #[test]
    fn a_step_can_take_a_fan_out_a_model_list_and_a_gate() {
        let r = Recipe::parse(
            r#"
id = "scan"
version = 2
[[steps]]
id = "audit"
engine = "scanner"
fan_out = "per_file"
gate = ["tests green", "score >= 3"]
[[steps]]
id = "rank"
engine = "scanner"
model = ["a/one", "b/two"]
fan_out = 4
"#,
        )
        .unwrap();
        assert_eq!(r.steps[0].fan_out, Fan::Named("per_file".into()));
        assert_eq!(r.steps[1].fan_out, Fan::Count(4));
        assert_eq!(r.steps[0].gate_conditions().len(), 2);
        assert_eq!(r.key(), "scan@2");
        let bad = Recipe::parse(
            r#"
id = "scan"
version = 1
[[steps]]
id = "audit"
role = "reviewer"
fan_out = "per_colour"
"#,
        );
        assert!(bad.unwrap_err().contains("not a fan-out"));
    }

    #[test]
    fn a_project_copy_overrides_by_id_and_a_broken_one_is_named() {
        let listing = json!({ "files": [{ "path": ".plaza/recipes/change.toml" }] });
        let text = CHANGE.replace("version = 1", "version = 4\nscope = \"project\"");
        let found = in_listing("change", &listing, |path| {
            assert_eq!(path, ".plaza/recipes/change.toml");
            Ok(text.clone().into_bytes())
        })
        .unwrap()
        .unwrap();
        assert_eq!(found.version, 4);
        assert_eq!(found.scope, Scope::Project);

        let none = in_listing("change", &json!({ "files": [] }), |_| unreachable!()).unwrap();
        assert!(none.is_none());

        let broken = in_listing("change", &listing, |_| Ok(b"id = \"change\"".to_vec()));
        assert!(
            broken
                .unwrap_err()
                .starts_with(".plaza/recipes/change.toml")
        );

        let wrong_id = in_listing(
            "ci",
            &json!({ "files": [{ "path": ".plaza/recipes/ci.toml" }] }),
            |_| Ok(text.clone().into_bytes()),
        );
        assert!(wrong_id.unwrap_err().contains("not ci"));
    }
}
