//! A topology file (PLAN 22.6): `.plaza/topologies/<name>.toml`, the system under test with its dependencies.
//!
//! ```toml
//! name = "web-stack"
//!
//! [[service]]
//! name = "api"
//! artifact = "api-linux-x86_64"            # built by the release under test
//! replicas = 2
//! ports = [8080]
//! env = { DATABASE_URL = "postgres://app@db/app", CACHE_URL = "redis://cache:6379" }
//!
//! [[service]]
//! name = "db"
//! image = "postgres:17"                    # a pinned dependency
//! data = "seed:app-small"
//!
//! [observe]
//! logs = "all"
//! metrics = "otlp"
//! traces = "otlp"
//!
//! [traffic]
//! profile = "steady-200rps"
//! ```
//!
//! Every field is typed and an unknown key is refused. [`parse`] also checks what the types cannot: names are DNS labels
//! and unique, a service has exactly one of `artifact` (from the release) and `image` (a pinned dependency), `serve` goes
//! with an artifact only, and a URL in an environment value whose host is a bare name names a service of the topology,
//! since services resolve by name inside the lab VM.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::cache::CacheStage;
use crate::data::DataStage;
use crate::fault::Fault;
use crate::messaging::MessagingStage;

/// The directory of a project's topology files in its repo at main.
pub const DIR: &str = ".plaza/topologies";
/// The most replicas one service may declare (a lab VM is sized for its topology).
pub const MAX_REPLICAS: u32 = 16;
/// The largest declared volume size, in MiB (a lab volume is small: a fill writes it).
pub const MAX_VOLUME_MIB: u64 = 1024;

/// A topology: its name, what it runs, its services (or its k3s cluster), what it observes, its traffic and its egress
/// allowlist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Topology {
    pub name: String,
    /// `services` (the default: containers on the VM's bridge) or `k3s` (a Kubernetes cluster in the VM, with `[k3s]`).
    #[serde(default)]
    pub kind: Kind,
    #[serde(rename = "service", default)]
    pub services: Vec<Service>,
    /// The VMs of a multi-VM topology (PLAN 22.6, W58): each runs some of the services. Two or more machines make the
    /// topology multi-VM: they share a private segment named by the topology, and a `stop_node` scenario can take one
    /// machine's node out of service. No machines: the topology runs in one lab VM, as before.
    #[serde(rename = "machine", default)]
    pub machines: Vec<Machine>,
    #[serde(default)]
    pub k3s: Option<K3s>,
    #[serde(default)]
    pub observe: Observe,
    #[serde(default)]
    pub traffic: Option<Traffic>,
    /// The test steps the run's observations must pass (PLAN 22.7), each over its logs, traces and series.
    #[serde(rename = "assert", default)]
    pub assertions: Vec<Assertion>,
    /// The resilience scenarios (PLAN 22.8): a fault or a kill on a service, and the degradation and recovery they judge.
    #[serde(rename = "resilience", default)]
    pub resilience: Vec<Scenario>,
    #[serde(default)]
    pub egress: Egress,
    /// The data stage (PLAN 22.8): migrations, backup and restore, writers and invariants (`[data]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<DataStage>,
    /// The cache stage (PLAN 22.8): hit rate, invalidation and stampede (`[cache]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheStage>,
    /// The messaging stage (PLAN 22.8): delivery, ordering, retries, dead letters and duplicates (`[messaging]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messaging: Option<MessagingStage>,
    /// The security stages (PLAN 22.8), declared in the `[security]` table.
    #[serde(default)]
    pub security: Security,
}

/// What a topology runs: its services as containers on the lab VM's bridge, or a Kubernetes cluster (k3s) in the VM.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Services,
    K3s,
}

/// A k3s topology's cluster: a pinned k3s release, the manifests applied when it starts (repo paths), and the images the
/// lab base preloads into the cluster's containerd.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct K3s {
    /// A k3s release, as k3s names it: `v1.31.4+k3s1`.
    pub version: String,
    #[serde(default)]
    pub manifests: Vec<String>,
    #[serde(default)]
    pub images: Vec<String>,
}

/// One service: its program (an artifact of the release or a pinned image), its replicas, ports, environment, data,
/// the services it waits for, its volumes and its health check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Service {
    pub name: String,
    #[serde(default)]
    pub artifact: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default = "one")]
    pub replicas: u32,
    #[serde(default)]
    pub ports: Vec<u16>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub data: Option<Data>,
    #[serde(default)]
    pub serve: Option<Serve>,
    /// Services that start first, and whose ports must accept connections before this one starts.
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub volumes: Vec<Volume>,
    #[serde(default)]
    pub health: Option<Health>,
}

/// A named volume mounted at `target`. Every service that names the volume shares its directory. The lab VM has no host
/// paths, so a bind mount is not a topology's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Volume {
    pub name: String,
    pub target: String,
    #[serde(default)]
    pub read_only: bool,
    /// The volume's declared size in MiB (1 to 1024): what a `fill_disk` scenario fills, and the bound of that fill.
    #[serde(default)]
    pub size_mib: Option<u64>,
}

/// A service's health check: the command that runs inside its container, and the timing Compose gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Health {
    pub test: Vec<String>,
    pub interval_secs: Option<u32>,
    pub timeout_secs: Option<u32>,
    pub retries: Option<u32>,
    pub start_period_secs: Option<u32>,
}

fn one() -> u32 {
    1
}

/// A service's data: a seed set the base loads, or a generated set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Data {
    Seed(String),
    Generated(String),
}

impl TryFrom<String> for Data {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        match text.split_once(':') {
            Some(("seed", name)) if !name.is_empty() => Ok(Data::Seed(name.to_string())),
            Some(("generated", name)) if !name.is_empty() => Ok(Data::Generated(name.to_string())),
            _ => Err(format!(
                "data {text:?} is not seed:<name> or generated:<name>"
            )),
        }
    }
}

impl From<Data> for String {
    fn from(data: Data) -> String {
        match data {
            Data::Seed(name) => format!("seed:{name}"),
            Data::Generated(name) => format!("generated:{name}"),
        }
    }
}

/// How a service is served: `static` serves an artifact's files (a web build).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Serve {
    Static,
}

/// What a lab run observes: the logs of `all` services or of the named ones, and the OTLP collection of metrics and
/// traces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Observe {
    pub logs: Logs,
    pub metrics: Collect,
    pub traces: Collect,
}

impl Default for Observe {
    fn default() -> Self {
        Observe {
            logs: Logs::All,
            metrics: Collect::Otlp,
            traces: Collect::Otlp,
        }
    }
}

/// Which services' logs are kept: `"all"`, or a list of service names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Logs {
    All,
    Services(Vec<String>),
}

impl Serialize for Logs {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Logs::All => serializer.serialize_str("all"),
            Logs::Services(names) => names.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Logs {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Word(String),
            List(Vec<String>),
        }
        match Repr::deserialize(de)? {
            Repr::Word(word) if word == "all" => Ok(Logs::All),
            Repr::Word(word) => Err(serde::de::Error::custom(format!(
                "logs is \"all\" or a list of services, not {word:?}"
            ))),
            Repr::List(names) => Ok(Logs::Services(names)),
        }
    }
}

/// Whether a signal is collected over OTLP (the lab VM's collector) or not.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Collect {
    #[default]
    Otlp,
    Off,
}

/// The traffic a run drives: a load profile by name (10.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Traffic {
    pub profile: String,
}

/// A test step over what a run observed (PLAN 22.7). Each names its `kind`; `id` names it in findings (default: the kind).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Assertion {
    /// No ERROR line outside the allowlist (each entry a substring of the line).
    NoErrors {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        allow: Vec<String>,
    },
    /// Every request is traced: the requests counted by `http.server.requests` are at most the server spans, and every
    /// span has a trace id.
    EveryRequestTraced {
        #[serde(default)]
        id: Option<String>,
    },
    /// The 95th percentile of a route's server spans, in milliseconds, is under `ms` (`*` is every route).
    P95Under {
        #[serde(default)]
        id: Option<String>,
        #[serde(default = "every_route")]
        route: String,
        ms: u64,
    },
    /// The hit rate of `hits` over `hits` plus `misses` is above `floor_percent`, counting from `after_secs` into the run
    /// (the warm-up).
    HitRateAbove {
        #[serde(default)]
        id: Option<String>,
        #[serde(default = "cache_hits")]
        hits: String,
        #[serde(default = "cache_misses")]
        misses: String,
        floor_percent: u32,
        #[serde(default)]
        after_secs: u64,
    },
    /// No personal data pattern (an email, a phone number, a card number that passes Luhn, a national id) in any log.
    NoPersonalData {
        #[serde(default)]
        id: Option<String>,
    },
    /// The log lines per request are at most `max_lines_per_request`.
    LogVolumePerRequest {
        #[serde(default)]
        id: Option<String>,
        max_lines_per_request: u64,
    },
    /// Each expected audit event (a log line whose `event.name` attribute is it) is present.
    AuditEvents {
        #[serde(default)]
        id: Option<String>,
        expected: Vec<String>,
    },
}

fn every_route() -> String {
    "*".into()
}

fn cache_hits() -> String {
    "cache.hits".into()
}

fn cache_misses() -> String {
    "cache.misses".into()
}

impl Assertion {
    /// The name the assertion's findings carry: its `id`, or its kind.
    pub fn id(&self) -> String {
        match self {
            Assertion::NoErrors { id, .. }
            | Assertion::EveryRequestTraced { id }
            | Assertion::P95Under { id, .. }
            | Assertion::HitRateAbove { id, .. }
            | Assertion::NoPersonalData { id }
            | Assertion::LogVolumePerRequest { id, .. }
            | Assertion::AuditEvents { id, .. } => id.clone().unwrap_or_else(|| self.kind().into()),
        }
    }

    /// The kind's name as the file writes it.
    pub fn kind(&self) -> &'static str {
        match self {
            Assertion::NoErrors { .. } => "no_errors",
            Assertion::EveryRequestTraced { .. } => "every_request_traced",
            Assertion::P95Under { .. } => "p95_under",
            Assertion::HitRateAbove { .. } => "hit_rate_above",
            Assertion::NoPersonalData { .. } => "no_personal_data",
            Assertion::LogVolumePerRequest { .. } => "log_volume_per_request",
            Assertion::AuditEvents { .. } => "audit_events",
        }
    }

    /// What the step asserts, in a sentence (the finding's scenario says it).
    pub fn describe(&self) -> String {
        match self {
            Assertion::NoErrors { allow, .. } => format!(
                "no ERROR line outside the allowlist ({} entries)",
                allow.len()
            ),
            Assertion::EveryRequestTraced { .. } => {
                "every request is traced, and every span has a trace id".into()
            }
            Assertion::P95Under { route, ms, .. } => {
                format!("the p95 latency of {route} is under {ms} ms")
            }
            Assertion::HitRateAbove {
                floor_percent,
                after_secs,
                ..
            } => format!(
                "the cache hit rate is above {floor_percent} % after {after_secs} s of warm-up"
            ),
            Assertion::NoPersonalData { .. } => {
                "no personal data pattern appears in any log".into()
            }
            Assertion::LogVolumePerRequest {
                max_lines_per_request,
                ..
            } => format!("at most {max_lines_per_request} log lines per request"),
            Assertion::AuditEvents { expected, .. } => {
                format!("the audit events {} are present", expected.join(", "))
            }
        }
    }

    /// What the assertion itself must make sense of (the topology's check refuses the rest).
    fn check(&self) -> Result<(), String> {
        match self {
            Assertion::NoErrors { allow, .. } => {
                if allow.iter().any(String::is_empty) {
                    return Err(
                        "no_errors allows an empty substring, which allows every line".into(),
                    );
                }
            }
            Assertion::EveryRequestTraced { .. } | Assertion::NoPersonalData { .. } => {}
            Assertion::P95Under { route, ms, .. } => {
                if route.trim().is_empty() {
                    return Err("p95_under names no route (\"*\" is every route)".into());
                }
                if *ms == 0 {
                    return Err(format!("p95_under for {route} has a threshold of 0 ms"));
                }
            }
            Assertion::HitRateAbove {
                hits,
                misses,
                floor_percent,
                ..
            } => {
                if hits.is_empty() || misses.is_empty() || hits == misses {
                    return Err("hit_rate_above names two different series, hits and misses".into());
                }
                if *floor_percent > 100 {
                    return Err(format!(
                        "hit_rate_above floor {floor_percent} is over 100 %"
                    ));
                }
            }
            Assertion::LogVolumePerRequest {
                max_lines_per_request,
                ..
            } => {
                if *max_lines_per_request == 0 {
                    return Err("log_volume_per_request allows 0 lines per request".into());
                }
            }
            Assertion::AuditEvents { expected, .. } => {
                if expected.is_empty() || expected.iter().any(String::is_empty) {
                    return Err("audit_events names no event (or an empty one)".into());
                }
            }
        }
        Ok(())
    }
}

/// A resilience scenario (PLAN 22.8): an action on a service at a time, held for a while, and what the system must do
/// while it holds and after it is lifted. The `probe` is the traffic that is judged: a GET on a service's path, sent
/// through the run, whose failures and latencies are measured against `expect`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub id: String,
    pub action: Action,
    /// The service the action is on: a dependency behind a fault proxy, or the service killed, restarted or skewed.
    pub target: String,
    /// The fault a `fault` action injects (a proxy's fault, or `netem` on the lab VM's bridge).
    #[serde(default)]
    pub fault: Option<Fault>,
    /// The target's volume a `fill_disk` action fills.
    #[serde(default)]
    pub volume: Option<String>,
    /// The space a `fill_disk` action leaves free, in MiB.
    #[serde(default)]
    pub leave_mib: u64,
    /// The seconds a `clock_skew` action moves the target's clock by (negative: back).
    #[serde(default)]
    pub secs: Option<i64>,
    /// When the action starts, in seconds from the run's start.
    pub at_secs: u64,
    /// How long the action holds, in seconds.
    pub for_secs: u64,
    /// The seconds after the action is lifted within which the probe must succeed again.
    pub recover_within_secs: u64,
    pub probe: Probe,
    /// The degradation the spec allows while the action holds.
    pub expect: Expect,
}

/// One VM of a multi-VM topology: its name, and the services it runs (each service runs on exactly one machine).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Machine {
    pub name: String,
    pub services: Vec<String>,
}

/// What a scenario does to its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// A fault in front of the target (a fault proxy's, or the lab VM's bridge's with `netem`).
    Fault,
    /// The target's process is killed; lifting the action starts it again.
    Kill,
    /// The target's process is killed and started again at once.
    Restart,
    /// The target's volume is filled, leaving `leave_mib` free.
    FillDisk,
    /// The target's clock is moved by `secs`, and moved back when lifted.
    ClockSkew,
    /// The node that runs the target machine (a `[[machine]]` of a multi-VM topology) is stopped through the hub's
    /// node stop, and resumed when lifted (see [`crate::node_loss`]).
    StopNode,
}

/// The traffic a scenario judges: a GET of `path` on `service`, through the run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Probe {
    pub service: String,
    #[serde(default = "root_path")]
    pub path: String,
}

fn root_path() -> String {
    "/".into()
}

/// The degradation the spec allows while the action holds: the share of probes that may fail, and the 95th percentile
/// latency of the probes that may not be exceeded (none: not judged).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    pub max_error_percent: f64,
    #[serde(default)]
    pub max_p95_ms: Option<u64>,
}

impl Topology {
    /// The machines of a multi-VM topology: each named and unique, each running some services, and each service
    /// on exactly one of them. A topology with no machines has none of this to check.
    fn check_machines(&self) -> Result<(), String> {
        if self.machines.is_empty() {
            return Ok(());
        }
        let services: BTreeSet<&str> = self.services.iter().map(|s| s.name.as_str()).collect();
        let mut names = BTreeSet::new();
        let mut placed = BTreeSet::new();
        for machine in &self.machines {
            if !is_label(&machine.name) {
                return Err(format!("machine {:?} is not a DNS label", machine.name));
            }
            if !names.insert(machine.name.as_str()) {
                return Err(format!("machine {} is declared twice", machine.name));
            }
            if machine.services.is_empty() {
                return Err(format!("machine {} runs no services", machine.name));
            }
            for service in &machine.services {
                if !services.contains(service.as_str()) {
                    return Err(format!(
                        "machine {} runs {service}, which is no service",
                        machine.name
                    ));
                }
                if !placed.insert(service.as_str()) {
                    return Err(format!("service {service} is on two machines"));
                }
            }
        }
        if let Some(service) = services.iter().find(|s| !placed.contains(*s)) {
            return Err(format!("service {service} is on no machine"));
        }
        Ok(())
    }
}

impl Scenario {
    /// What the scenario's fields must say for its action, and that its names are in the topology.
    fn check(&self, topology: &Topology) -> Result<(), String> {
        let who = format!("resilience scenario {}", self.id);
        if !is_label(&self.id) {
            return Err(format!("{who}: the id is not a DNS label"));
        }
        let names: BTreeSet<&str> = topology.services.iter().map(|s| s.name.as_str()).collect();
        if self.action == Action::StopNode {
            // A node stop takes out a machine's node, so the target is a machine of a multi-VM topology.
            if topology.machines.len() < 2 {
                return Err(format!(
                    "{who} stops a node, which needs a multi-VM topology (two or more [[machine]] tables)"
                ));
            }
            if !topology.machines.iter().any(|m| m.name == self.target) {
                return Err(format!("{who} stops {}, which is no machine", self.target));
            }
        } else if !names.contains(self.target.as_str()) {
            return Err(format!(
                "{who} targets {}, which is no service",
                self.target
            ));
        }
        if !names.contains(self.probe.service.as_str()) {
            return Err(format!(
                "{who} probes {}, which is no service",
                self.probe.service
            ));
        }
        if !self.probe.path.starts_with('/') || self.probe.path.chars().any(char::is_whitespace) {
            return Err(format!(
                "{who}: the probe path {:?} is not a path",
                self.probe.path
            ));
        }
        if self.for_secs == 0 || self.recover_within_secs == 0 {
            return Err(format!("{who} holds for 0 s or allows no recovery time"));
        }
        if !self.expect.max_error_percent.is_finite()
            || !(0.0..=100.0).contains(&self.expect.max_error_percent)
        {
            return Err(format!("{who}: max_error_percent must be 0 to 100"));
        }
        if self.expect.max_p95_ms == Some(0) {
            return Err(format!("{who}: max_p95_ms of 0 ms can never pass"));
        }
        let only = |field: &str, set: bool, action: &str| -> Result<(), String> {
            if set {
                Err(format!("{who}: {field} goes with a {action} action only"))
            } else {
                Ok(())
            }
        };
        match self.action {
            Action::Fault => {
                let fault = self
                    .fault
                    .as_ref()
                    .ok_or_else(|| format!("{who} is a fault and names no fault"))?;
                fault.check().map_err(|e| format!("{who}: {e}"))?;
                if *fault == Fault::Healthy {
                    return Err(format!("{who} injects no fault"));
                }
                only("volume", self.volume.is_some(), "fill_disk")?;
                only("secs", self.secs.is_some(), "clock_skew")?;
            }
            Action::Kill | Action::Restart => {
                only("fault", self.fault.is_some(), "fault")?;
                only("volume", self.volume.is_some(), "fill_disk")?;
                only("secs", self.secs.is_some(), "clock_skew")?;
            }
            Action::StopNode => {
                only("fault", self.fault.is_some(), "fault")?;
                only("volume", self.volume.is_some(), "fill_disk")?;
                only("secs", self.secs.is_some(), "clock_skew")?;
            }
            Action::FillDisk => {
                let volume = self
                    .volume
                    .as_deref()
                    .ok_or_else(|| format!("{who} fills a disk and names no volume"))?;
                let declared = topology.volume_of(&self.target, volume).ok_or_else(|| {
                    format!("{who} fills {volume}, which {} does not mount", self.target)
                })?;
                if declared.size_mib.is_none() {
                    return Err(format!(
                        "{who} fills {volume}, which has no size_mib: a fill is bounded by the volume's declared size"
                    ));
                }
                only("fault", self.fault.is_some(), "fault")?;
                only("secs", self.secs.is_some(), "clock_skew")?;
            }
            Action::ClockSkew => {
                let secs = self
                    .secs
                    .ok_or_else(|| format!("{who} skews a clock and names no secs"))?;
                if secs == 0 || secs.unsigned_abs() > 365 * 86_400 {
                    return Err(format!("{who}: a clock skew is between 1 s and a year"));
                }
                only("fault", self.fault.is_some(), "fault")?;
                only("volume", self.volume.is_some(), "fill_disk")?;
            }
        }
        Ok(())
    }
}

impl Topology {
    /// The named volume of `service`, as the service mounts it.
    pub fn volume_of(&self, service: &str, volume: &str) -> Option<&Volume> {
        self.services
            .iter()
            .filter(|s| s.name == service)
            .find_map(|s| s.volumes.iter().find(|v| v.name == volume))
    }
}

/// The hosts the topology may reach through the egress proxy, beyond the images and the artifacts it already holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Egress {
    pub allow: Vec<String>,
}

/// The security stages a topology declares (PLAN 22.8): the dependency audits and the image scan, the secrets scan of
/// the release's tree, the DAST scan of the routes, and the fuzzers. Absent, the defaults: every known audit (each one
/// its tool is installed for), the secrets scan, and no image, DAST or fuzz stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Security {
    /// The audits to run (`cargo-audit`, `cargo-deny`, `npm-audit`, `pip-audit`, `govulncheck`, `osv-scanner`); none
    /// names every known one.
    pub audit: Option<Vec<String>>,
    /// The image a trivy or grype scan reads, by reference (`postgres:17`); none skips the stage.
    pub image: Option<String>,
    /// Whether the secrets scan reads the release's tree.
    pub secrets: bool,
    /// The routes the DAST scan probes: a GET of `path` on `service`, with each parameter's sample value.
    #[serde(rename = "dast")]
    pub dast: Vec<Route>,
    /// The fuzzers the project gates on.
    #[serde(rename = "fuzz")]
    pub fuzz: Vec<Fuzz>,
}

impl Default for Security {
    fn default() -> Self {
        Security {
            audit: None,
            image: None,
            secrets: true,
            dast: Vec::new(),
            fuzz: Vec::new(),
        }
    }
}

/// A route of the DAST scan: a GET of `path` on `service`, its query parameters and a sample value for each (the value the
/// route answers normally).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub service: String,
    pub path: String,
    pub params: BTreeMap<String, String>,
}

/// A fuzzer the project gates on. `cargo-fuzz` runs `target` in the crate at `dir` for `seconds`; `schemathesis` runs the
/// property tests of `schema` against `service` for `seconds`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fuzz {
    pub tool: String,
    #[serde(default)]
    pub dir: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default)]
    pub service: Option<String>,
    #[serde(default = "minute")]
    pub seconds: u64,
}

fn minute() -> u64 {
    60
}

/// The largest fuzzing budget a declared fuzzer may have, in seconds.
pub const MAX_FUZZ_SECS: u64 = 3600;

impl Security {
    /// What the stages name, and that they name what the topology has: the audits are known, the routes' services are
    /// the topology's, a route's parameters are named, a fuzzer's fields fit its tool.
    fn check(&self, topology: &Topology) -> Result<(), String> {
        let known = crate::security::audit::AUDITS;
        for name in self.audit.iter().flatten() {
            if !known.contains(&name.as_str()) {
                return Err(format!("security audit {name:?} is not a known audit"));
            }
        }
        if let Some(image) = &self.image
            && !is_name(image)
        {
            return Err(format!(
                "security image {image:?} is not an image reference"
            ));
        }
        let services: BTreeSet<&str> = topology.services.iter().map(|s| s.name.as_str()).collect();
        for route in &self.dast {
            if !services.contains(route.service.as_str()) {
                return Err(format!(
                    "security route names service {} the topology does not have",
                    route.service
                ));
            }
            if !route.path.starts_with('/')
                || route.path.contains(char::is_whitespace)
                || route.path.contains(['?', '#'])
            {
                return Err(format!(
                    "security route path {:?} is not an absolute path without a query",
                    route.path
                ));
            }
            if route.params.is_empty() {
                return Err(format!(
                    "security route {} names no parameter to probe",
                    route.path
                ));
            }
            for (name, sample) in &route.params {
                if name.is_empty()
                    || name.contains(['&', '=', '#', '%', '+'])
                    || name.contains(char::is_whitespace)
                {
                    return Err(format!(
                        "security route parameter {name:?} is not a parameter name"
                    ));
                }
                if sample.is_empty() {
                    return Err(format!(
                        "security route parameter {name} needs a sample value"
                    ));
                }
            }
        }
        for fuzz in &self.fuzz {
            if fuzz.seconds == 0 || fuzz.seconds > MAX_FUZZ_SECS {
                return Err(format!(
                    "fuzz budget of {} s is not between 1 and {MAX_FUZZ_SECS}",
                    fuzz.seconds
                ));
            }
            match fuzz.tool.as_str() {
                "cargo-fuzz" => {
                    let target = fuzz.target.as_deref().unwrap_or_default();
                    if target.is_empty()
                        || !target
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                    {
                        return Err(format!("cargo-fuzz target {target:?} is not a name"));
                    }
                }
                "schemathesis" => {
                    if fuzz.schema.as_deref().unwrap_or_default().is_empty() {
                        return Err("schemathesis names no schema".into());
                    }
                    let service = fuzz.service.as_deref().unwrap_or_default();
                    if !services.contains(service) {
                        return Err(format!(
                            "schemathesis names service {service:?} the topology does not have"
                        ));
                    }
                }
                other => {
                    return Err(format!(
                        "fuzz tool {other:?} is not cargo-fuzz or schemathesis"
                    ));
                }
            }
        }
        Ok(())
    }
}

/// The topology in `text`, checked.
pub fn parse(text: &str) -> Result<Topology, String> {
    let topology: Topology = toml::from_str(text).map_err(|e| format!("topology: {e}"))?;
    topology.check()?;
    Ok(topology)
}

/// The topology as its file's text: the TOML that [`parse`] reads back to the same topology.
pub fn to_toml(topology: &Topology) -> Result<String, String> {
    toml::to_string(topology).map_err(|e| format!("topology {}: {e}", topology.name))
}

impl Topology {
    /// What TOML cannot say: the names, the sources, the replicas, the ports, the URLs and the references.
    pub fn check(&self) -> Result<(), String> {
        if !is_label(&self.name) {
            return Err(format!("topology name {:?} is not a DNS label", self.name));
        }
        self.check_machines()?;
        self.check_machines()?;
        if let Some(stage) = &self.data {
            stage
                .check()
                .map_err(|why| format!("topology {}: {why}", self.name))?;
        }
        if let Some(stage) = &self.cache {
            stage
                .check()
                .map_err(|why| format!("topology {}: {why}", self.name))?;
            if !self.services.iter().any(|s| s.name == stage.service) {
                return Err(format!(
                    "topology {}: the cache stage names service {}, which the topology does not declare",
                    self.name, stage.service
                ));
            }
        }
        if let Some(stage) = &self.messaging {
            stage
                .check()
                .map_err(|why| format!("topology {}: {why}", self.name))?;
        }
        match (self.kind, &self.k3s) {
            (Kind::Services, Some(_)) => {
                return Err(format!(
                    "topology {} sets [k3s] but is not kind = \"k3s\"",
                    self.name
                ));
            }
            (Kind::K3s, None) => {
                return Err(format!(
                    "topology {} is kind = \"k3s\" and has no [k3s]",
                    self.name
                ));
            }
            (Kind::K3s, Some(k3s)) => {
                if !self.services.is_empty() {
                    return Err(format!(
                        "topology {} is a k3s cluster: its manifests are its services, so it declares no [[service]]",
                        self.name
                    ));
                }
                return k3s.check();
            }
            (Kind::Services, None) => {}
        }
        if self.services.is_empty() {
            return Err(format!("topology {} declares no service", self.name));
        }
        let mut names: BTreeSet<&str> = BTreeSet::new();
        for service in &self.services {
            service.check()?;
            if !names.insert(service.name.as_str()) {
                return Err(format!("service {} is declared twice", service.name));
            }
        }
        check_order(&self.services)?;
        for service in &self.services {
            for (key, value) in &service.env {
                if key.is_empty() || key.contains('=') || key.chars().any(char::is_whitespace) {
                    return Err(format!(
                        "service {} has an environment name {key:?} that is not a name",
                        service.name
                    ));
                }
                if let Some(host) = url_host(value)
                    && !host.contains('.')
                    && host != "localhost"
                    && !names.contains(host)
                {
                    return Err(format!(
                        "service {} sets {key} to a URL on host {host}, which is no service of {}",
                        service.name, self.name
                    ));
                }
            }
        }
        if let Logs::Services(list) = &self.observe.logs {
            for name in list {
                if !names.contains(name.as_str()) {
                    return Err(format!("observe.logs names {name}, which is no service"));
                }
            }
        }
        if let Some(traffic) = &self.traffic
            && !is_label(&traffic.profile)
        {
            return Err(format!(
                "traffic profile {:?} is not a name",
                traffic.profile
            ));
        }
        for host in &self.egress.allow {
            if !is_host(host) {
                return Err(format!("egress allow entry {host:?} is not a host name"));
            }
        }
        let mut ids: BTreeSet<String> = BTreeSet::new();
        for assertion in &self.assertions {
            assertion.check()?;
            if !ids.insert(assertion.id()) {
                return Err(format!(
                    "topology {} has two assertions named {}",
                    self.name,
                    assertion.id()
                ));
            }
        }
        let mut scenarios: BTreeSet<&str> = BTreeSet::new();
        for scenario in &self.resilience {
            scenario.check(self)?;
            if !scenarios.insert(scenario.id.as_str()) {
                return Err(format!(
                    "topology {} has two resilience scenarios named {}",
                    self.name, scenario.id
                ));
            }
        }
        self.security.check(self)?;
        Ok(())
    }
}

impl Service {
    fn check(&self) -> Result<(), String> {
        if !is_label(&self.name) {
            return Err(format!(
                "service {:?} is not a DNS label (lowercase letters, digits and dashes)",
                self.name
            ));
        }
        match (&self.artifact, &self.image) {
            (Some(_), Some(_)) => Err(format!(
                "service {} names both an artifact and an image",
                self.name
            )),
            (None, None) => Err(format!(
                "service {} names neither an artifact nor an image",
                self.name
            )),
            (Some(artifact), None) if !is_name(artifact) => {
                Err(format!("service {} has an empty artifact name", self.name))
            }
            (None, Some(image)) if !is_name(image) => {
                Err(format!("service {} has an empty image", self.name))
            }
            _ => Ok(()),
        }?;
        if self.replicas == 0 || self.replicas > MAX_REPLICAS {
            return Err(format!(
                "service {} has {} replicas: 1 to {MAX_REPLICAS} allowed",
                self.name, self.replicas
            ));
        }
        let mut ports = BTreeSet::new();
        for port in &self.ports {
            if *port == 0 || !ports.insert(*port) {
                return Err(format!(
                    "service {} lists port {port} twice or as 0",
                    self.name
                ));
            }
        }
        if self.serve.is_some() && self.artifact.is_none() {
            return Err(format!(
                "service {} sets serve, which needs an artifact",
                self.name
            ));
        }
        if self.depends_on.contains(&self.name) {
            return Err(format!("service {} depends on itself", self.name));
        }
        let mut waits = BTreeSet::new();
        for dependency in &self.depends_on {
            if !waits.insert(dependency.as_str()) {
                return Err(format!(
                    "service {} lists {dependency} in depends_on twice",
                    self.name
                ));
            }
        }
        let mut targets = BTreeSet::new();
        for volume in &self.volumes {
            if !is_label(&volume.name) {
                return Err(format!(
                    "service {} mounts volume {:?}, which is not a DNS label",
                    self.name, volume.name
                ));
            }
            if !volume.target.starts_with('/')
                || volume.target.split('/').any(|part| part == "..")
                || volume.target.contains('\n')
            {
                return Err(format!(
                    "service {} mounts {} at {:?}: a target is an absolute path",
                    self.name, volume.name, volume.target
                ));
            }
            if volume
                .size_mib
                .is_some_and(|mib| mib == 0 || mib > MAX_VOLUME_MIB)
            {
                return Err(format!(
                    "service {} declares volume {} at {} MiB: 1 to {MAX_VOLUME_MIB} MiB allowed",
                    self.name,
                    volume.name,
                    volume.size_mib.unwrap_or(0)
                ));
            }
            if !targets.insert(volume.target.as_str()) {
                return Err(format!(
                    "service {} mounts two volumes at {}",
                    self.name, volume.target
                ));
            }
        }
        if let Some(health) = &self.health {
            if health.test.first().is_none_or(|program| program.is_empty()) {
                return Err(format!(
                    "service {} has a health check with no command",
                    self.name
                ));
            }
            for seconds in [
                health.interval_secs,
                health.timeout_secs,
                health.start_period_secs,
            ]
            .into_iter()
            .flatten()
            {
                if seconds == 0 {
                    return Err(format!(
                        "service {} has a health check timing of 0",
                        self.name
                    ));
                }
            }
        }
        Ok(())
    }
}

/// The services a `depends_on` names exist, and no service waits for itself through others (a cycle).
fn check_order(services: &[Service]) -> Result<(), String> {
    let names: BTreeSet<&str> = services.iter().map(|s| s.name.as_str()).collect();
    for service in services {
        for dependency in &service.depends_on {
            if !names.contains(dependency.as_str()) {
                return Err(format!(
                    "service {} depends on {dependency}, which is no service of the topology",
                    service.name
                ));
            }
        }
    }
    // Depth-first from each service: a service still on the path when reached again is a cycle.
    let edges: BTreeMap<&str, Vec<&str>> = services
        .iter()
        .map(|s| {
            (
                s.name.as_str(),
                s.depends_on.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    let mut done: BTreeSet<&str> = BTreeSet::new();
    for service in services {
        let mut path: Vec<&str> = Vec::new();
        visit(service.name.as_str(), &edges, &mut path, &mut done)?;
    }
    Ok(())
}

fn visit<'a>(
    name: &'a str,
    edges: &BTreeMap<&'a str, Vec<&'a str>>,
    path: &mut Vec<&'a str>,
    done: &mut BTreeSet<&'a str>,
) -> Result<(), String> {
    if done.contains(name) {
        return Ok(());
    }
    if let Some(at) = path.iter().position(|step| *step == name) {
        let cycle: Vec<&str> = path[at..]
            .iter()
            .copied()
            .chain(std::iter::once(name))
            .collect();
        return Err(format!("depends_on has a cycle: {}", cycle.join(" -> ")));
    }
    path.push(name);
    for next in edges.get(name).into_iter().flatten() {
        visit(next, edges, path, done)?;
    }
    path.pop();
    done.insert(name);
    Ok(())
}

impl K3s {
    /// The version is a k3s release name, and the manifests and images are repo paths and image names.
    pub fn check(&self) -> Result<(), String> {
        if !is_k3s_version(&self.version) {
            return Err(format!(
                "k3s version {:?} is not a k3s release (v1.31.4+k3s1)",
                self.version
            ));
        }
        for path in &self.manifests {
            let yaml = path.ends_with(".yaml") || path.ends_with(".yml");
            if !yaml || !is_repo_path(path) {
                return Err(format!(
                    "k3s manifest {path:?} is not a repo path to a .yaml file"
                ));
            }
        }
        for image in &self.images {
            if !is_name(image) {
                return Err(format!("k3s image {image:?} is not an image name"));
            }
        }
        Ok(())
    }
}

/// A k3s release: `v<major>.<minor>.<patch>+k3s<n>`.
fn is_k3s_version(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('v') else {
        return false;
    };
    let Some((semver, k3s)) = rest.split_once("+k3s") else {
        return false;
    };
    let parts: Vec<&str> = semver.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && !k3s.is_empty()
        && k3s.bytes().all(|b| b.is_ascii_digit())
}

/// A path inside the repository: relative, `/`-separated, no `..`, no leading or trailing separator.
pub(crate) fn is_repo_path(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with('/')
        && !text.ends_with('/')
        && !text.contains('\\')
        && text.split('/').all(|part| !part.is_empty() && part != "..")
        && !text.chars().any(char::is_control)
}

/// The host of a URL in an environment value (`postgres://app@db/app` names `db`); None when the value is no URL, or
/// names an IPv6 literal.
pub fn url_host(value: &str) -> Option<&str> {
    let (_, rest) = value.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host_port)| host_port);
    if host_port.starts_with('[') {
        return None;
    }
    let host = host_port.split(':').next()?;
    (!host.is_empty()).then_some(host)
}

/// A DNS label: lowercase letters, digits and dashes, 1 to 63 characters, not starting or ending with a dash.
pub(crate) fn is_label(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 63
        && !text.starts_with('-')
        && !text.ends_with('-')
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A host name for an egress allowlist: lowercase letters, digits, dots and dashes.
fn is_host(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with('.')
        && !text.ends_with('.')
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
}

/// A name of an artifact or an image: non-empty, no whitespace.
fn is_name(text: &str) -> bool {
    !text.trim().is_empty() && !text.chars().any(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEB_STACK: &str = include_str!("../tests/fixtures/web-stack.toml");

    #[test]
    fn the_web_stack_example_parses_into_its_typed_sections() {
        let topology = parse(WEB_STACK).unwrap();
        assert_eq!(topology.name, "web-stack");
        let names: Vec<&str> = topology.services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["api", "db", "cache", "web"]);
        let api = &topology.services[0];
        assert_eq!(api.artifact.as_deref(), Some("api-linux-x86_64"));
        assert_eq!(api.replicas, 2);
        assert_eq!(api.ports, [8080]);
        assert_eq!(api.env["CACHE_URL"], "redis://cache:6379");
        assert_eq!(
            topology.services[1].data,
            Some(Data::Seed("app-small".into()))
        );
        assert_eq!(topology.services[3].serve, Some(Serve::Static));
        assert_eq!(topology.observe.logs, Logs::All);
        assert_eq!(topology.traffic.unwrap().profile, "steady-200rps");
    }

    #[test]
    fn an_unknown_key_and_a_bad_data_form_are_refused() {
        let unknown = "name = \"t\"\nflavour = 1\n[[service]]\nname = \"a\"\nimage = \"x:1\"\n";
        assert!(parse(unknown).is_err());
        let data =
            "name = \"t\"\n[[service]]\nname = \"a\"\nimage = \"x:1\"\ndata = \"app-small\"\n";
        assert!(parse(data).unwrap_err().contains("seed:<name>"));
    }

    #[test]
    fn a_service_has_exactly_one_source_and_serve_needs_an_artifact() {
        let both = "name = \"t\"\n[[service]]\nname = \"a\"\nartifact = \"x\"\nimage = \"y:1\"\n";
        assert!(parse(both).unwrap_err().contains("both"));
        let neither = "name = \"t\"\n[[service]]\nname = \"a\"\n";
        assert!(parse(neither).unwrap_err().contains("neither"));
        let served =
            "name = \"t\"\n[[service]]\nname = \"a\"\nimage = \"y:1\"\nserve = \"static\"\n";
        assert!(parse(served).unwrap_err().contains("serve"));
    }

    #[test]
    fn names_replicas_and_ports_are_checked() {
        let repeated = "name = \"t\"\n[[service]]\nname = \"a\"\nimage = \"y\"\n[[service]]\nname = \"a\"\nimage = \"y\"\n";
        assert!(parse(repeated).unwrap_err().contains("twice"));
        let bad_label = "name = \"t\"\n[[service]]\nname = \"Api_1\"\nimage = \"y\"\n";
        assert!(parse(bad_label).unwrap_err().contains("DNS label"));
        let no_replicas = "name = \"t\"\n[[service]]\nname = \"a\"\nimage = \"y\"\nreplicas = 0\n";
        assert!(parse(no_replicas).unwrap_err().contains("replicas"));
        let port_zero = "name = \"t\"\n[[service]]\nname = \"a\"\nimage = \"y\"\nports = [0]\n";
        assert!(parse(port_zero).unwrap_err().contains("port"));
    }

    #[test]
    fn a_url_naming_a_bare_host_must_name_a_service() {
        let unknown = "name = \"t\"\n[[service]]\nname = \"api\"\nimage = \"y\"\nenv = { DATABASE_URL = \"postgres://app@dbx/app\" }\n";
        let why = parse(unknown).unwrap_err();
        assert!(why.contains("dbx"), "{why}");
        let external = "name = \"t\"\n[[service]]\nname = \"api\"\nimage = \"y\"\nenv = { SINK = \"https://collector.example.com/v1\", LOCAL = \"http://127.0.0.1:9\" }\n";
        assert!(parse(external).is_ok());
    }

    #[test]
    fn logs_name_known_services_and_the_egress_allowlist_takes_hosts() {
        let logs = "name = \"t\"\n[observe]\nlogs = [\"ghost\"]\n[[service]]\nname = \"api\"\nimage = \"y\"\n";
        assert!(parse(logs).unwrap_err().contains("ghost"));
        let allow = "name = \"t\"\n[egress]\nallow = [\"Bad Host\"]\n[[service]]\nname = \"api\"\nimage = \"y\"\n";
        assert!(parse(allow).is_err());
        let ok = "name = \"t\"\n[egress]\nallow = [\"registry.example.com\"]\n[observe]\nlogs = [\"api\"]\n[[service]]\nname = \"api\"\nimage = \"y\"\n";
        assert_eq!(parse(ok).unwrap().egress.allow, ["registry.example.com"]);
    }

    #[test]
    fn url_hosts_are_read_past_userinfo_ports_and_paths() {
        assert_eq!(url_host("postgres://app:pw@db:5432/app"), Some("db"));
        assert_eq!(url_host("redis://cache:6379"), Some("cache"));
        assert_eq!(url_host("http://[::1]:80/"), None);
        assert_eq!(url_host("plain text"), None);
    }

    const K3S: &str = "name = \"cluster\"\nkind = \"k3s\"\n[k3s]\nversion = \"v1.31.4+k3s1\"\nmanifests = [\".plaza/topologies/cluster/app.yaml\"]\nimages = [\"postgres:17\"]\n";

    #[test]
    fn a_k3s_topology_names_a_release_manifests_and_images() {
        let topology = parse(K3S).unwrap();
        assert_eq!(topology.kind, Kind::K3s);
        assert!(topology.services.is_empty());
        let k3s = topology.k3s.unwrap();
        assert_eq!(k3s.version, "v1.31.4+k3s1");
        assert_eq!(k3s.images, ["postgres:17"]);
    }

    #[test]
    fn a_k3s_topology_is_refused_with_services_a_bad_release_or_a_bad_manifest() {
        let services = format!("{K3S}[[service]]\nname = \"api\"\nimage = \"y\"\n");
        assert!(
            parse(&services)
                .unwrap_err()
                .contains("manifests are its services")
        );
        let release = K3S.replace("v1.31.4+k3s1", "1.31");
        assert!(parse(&release).unwrap_err().contains("not a k3s release"));
        let manifest = K3S.replace("app.yaml", "../app.yaml");
        assert!(parse(&manifest).unwrap_err().contains("manifest"));
        let no_section = "name = \"c\"\nkind = \"k3s\"\n";
        assert!(parse(no_section).unwrap_err().contains("no [k3s]"));
        let stray = "name = \"c\"\n[k3s]\nversion = \"v1.31.4+k3s1\"\n[[service]]\nname = \"a\"\nimage = \"y\"\n";
        assert!(parse(stray).unwrap_err().contains("not kind"));
    }

    #[test]
    fn depends_on_names_services_and_has_no_cycle() {
        let ok = "name = \"t\"\n[[service]]\nname = \"web\"\nimage = \"y\"\ndepends_on = [\"api\"]\n[[service]]\nname = \"api\"\nimage = \"y\"\ndepends_on = [\"db\"]\n[[service]]\nname = \"db\"\nimage = \"y\"\n";
        let topology = parse(ok).unwrap();
        assert_eq!(topology.services[0].depends_on, ["api"]);
        let ghost =
            "name = \"t\"\n[[service]]\nname = \"web\"\nimage = \"y\"\ndepends_on = [\"cache\"]\n";
        assert!(parse(ghost).unwrap_err().contains("cache"));
        let cycle = "name = \"t\"\n[[service]]\nname = \"a\"\nimage = \"y\"\ndepends_on = [\"b\"]\n[[service]]\nname = \"b\"\nimage = \"y\"\ndepends_on = [\"a\"]\n";
        let why = parse(cycle).unwrap_err();
        assert!(
            why.contains("cycle") && why.contains("a -> b -> a"),
            "{why}"
        );
        let itself =
            "name = \"t\"\n[[service]]\nname = \"a\"\nimage = \"y\"\ndepends_on = [\"a\"]\n";
        assert!(parse(itself).unwrap_err().contains("itself"));
    }

    #[test]
    fn volumes_and_health_are_typed_and_checked() {
        let text = "name = \"t\"\n[[service]]\nname = \"db\"\nimage = \"postgres:17\"\nvolumes = [{ name = \"db-data\", target = \"/var/lib/postgresql/data\" }]\n[service.health]\ntest = [\"sh\", \"-c\", \"pg_isready\"]\ninterval_secs = 5\nretries = 5\n";
        let topology = parse(text).unwrap();
        let db = &topology.services[0];
        assert_eq!(db.volumes[0].name, "db-data");
        assert!(!db.volumes[0].read_only);
        assert_eq!(db.health.as_ref().unwrap().interval_secs, Some(5));
        let relative = "name = \"t\"\n[[service]]\nname = \"db\"\nimage = \"y\"\nvolumes = [{ name = \"d\", target = \"data\" }]\n";
        assert!(parse(relative).unwrap_err().contains("absolute path"));
        let twice = "name = \"t\"\n[[service]]\nname = \"db\"\nimage = \"y\"\nvolumes = [{ name = \"d\", target = \"/d\" }, { name = \"e\", target = \"/d\" }]\n";
        assert!(parse(twice).unwrap_err().contains("two volumes"));
    }

    #[test]
    fn a_topology_survives_its_own_toml() {
        for text in [WEB_STACK, K3S] {
            let topology = parse(text).unwrap();
            let again = to_toml(&topology).unwrap();
            assert_eq!(parse(&again).unwrap(), topology, "{again}");
        }
    }

    const RESILIENCE: &str = "name = \"stack\"\n[[service]]\nname = \"api\"\nimage = \"api:1\"\nenv = { CACHE_URL = \"redis://cache:6379\" }\n[[service]]\nname = \"cache\"\nimage = \"redis:7\"\nvolumes = [{ name = \"cache-data\", target = \"/data\", size_mib = 4 }]\n";

    fn with(scenario: &str) -> String {
        format!("{RESILIENCE}[[resilience]]\n{scenario}")
    }

    #[test]
    fn a_fault_scenario_parses_with_its_target_probe_and_expected_degradation() {
        let text = with(
            "id = \"cache-outage\"\naction = \"fault\"\ntarget = \"cache\"\nfault = { kind = \"reset\", percent = 30 }\nat_secs = 2\nfor_secs = 5\nrecover_within_secs = 10\nprobe = { service = \"api\", path = \"/items\" }\nexpect = { max_error_percent = 5.0, max_p95_ms = 500 }\n",
        );
        let topology = parse(&text).unwrap();
        let scenario = &topology.resilience[0];
        assert_eq!(scenario.id, "cache-outage");
        assert_eq!(scenario.action, Action::Fault);
        assert_eq!(scenario.fault, Some(Fault::Reset { percent: 30 }));
        assert_eq!((scenario.at_secs, scenario.for_secs), (2, 5));
        assert_eq!(scenario.probe.path, "/items");
        assert_eq!(scenario.expect.max_p95_ms, Some(500));
        assert_eq!(scenario.expect.max_error_percent, 5.0);
    }

    #[test]
    fn every_fault_kind_parses() {
        for fault in [
            "{ kind = \"latency\", ms = 200 }",
            "{ kind = \"loss\", percent = 20 }",
            "{ kind = \"partition\" }",
            "{ kind = \"netem\", delay_ms = 50, loss_percent = 2 }",
        ] {
            let text = with(&format!(
                "id = \"f\"\naction = \"fault\"\ntarget = \"cache\"\nfault = {fault}\nat_secs = 0\nfor_secs = 1\nrecover_within_secs = 1\nprobe = {{ service = \"api\" }}\nexpect = {{ max_error_percent = 100 }}\n"
            ));
            let topology = parse(&text).unwrap_or_else(|e| panic!("{fault}: {e}"));
            assert!(topology.resilience[0].fault.is_some());
            assert_eq!(
                topology.resilience[0].probe.path, "/",
                "the default probe path"
            );
        }
    }

    #[test]
    fn a_scenario_is_refused_when_its_names_or_numbers_do_not_fit() {
        let base = |action: &str, extra: &str| {
            with(&format!(
                "id = \"s\"\naction = \"{action}\"\ntarget = \"cache\"\nat_secs = 0\nfor_secs = 1\nrecover_within_secs = 1\nprobe = {{ service = \"api\" }}\nexpect = {{ max_error_percent = 5 }}\n{extra}"
            ))
        };
        // A target or a probe that is no service.
        let ghost = with(
            "id = \"s\"\naction = \"kill\"\ntarget = \"queue\"\nat_secs = 0\nfor_secs = 1\nrecover_within_secs = 1\nprobe = { service = \"api\" }\nexpect = { max_error_percent = 5 }\n",
        );
        assert!(parse(&ghost).unwrap_err().contains("queue"));
        let probe = base("kill", "").replace("service = \"api\"", "service = \"web\"");
        assert!(parse(&probe).unwrap_err().contains("web"));
        // A fault action names a fault, and not healthy.
        assert!(
            parse(&base("fault", ""))
                .unwrap_err()
                .contains("names no fault")
        );
        let healthy = base("fault", "").replace(
            "[[resilience]]\n",
            "[[resilience]]\nfault = { kind = \"healthy\" }\n",
        );
        assert!(parse(&healthy).unwrap_err().contains("injects no fault"));
        // A percentage over 100 and an expectation over 100 %.
        let over = base("fault", "").replace(
            "[[resilience]]\n",
            "[[resilience]]\nfault = { kind = \"loss\", percent = 101 }\n",
        );
        assert!(parse(&over).unwrap_err().contains("100"));
        let expect = base("kill", "").replace("max_error_percent = 5", "max_error_percent = 120");
        assert!(parse(&expect).unwrap_err().contains("max_error_percent"));
        // A field of another action is refused, as are a zero hold and a zero recovery.
        let stray = base("kill", "").replace(
            "[[resilience]]\n",
            "[[resilience]]\nvolume = \"cache-data\"\n",
        );
        assert!(parse(&stray).unwrap_err().contains("fill_disk"));
        let zero = base("kill", "").replace("for_secs = 1", "for_secs = 0");
        assert!(parse(&zero).unwrap_err().contains("0 s"));
        // Two scenarios of one name.
        let twice = format!(
            "{}{}",
            base("kill", ""),
            "\n[[resilience]]\nid = \"s\"\naction = \"kill\"\ntarget = \"api\"\nat_secs = 0\nfor_secs = 1\nrecover_within_secs = 1\nprobe = { service = \"api\" }\nexpect = { max_error_percent = 5 }\n"
        );
        assert!(parse(&twice).unwrap_err().contains("two resilience"));
        // An unknown key is refused.
        let unknown = base("kill", "").replace("for_secs = 1", "for_secs = 1\nseverity = 3");
        assert!(parse(&unknown).is_err());
    }

    #[test]
    fn a_disk_fill_names_a_volume_the_target_mounts_and_a_skew_names_its_seconds() {
        let fill = with(
            "id = \"full-disk\"\naction = \"fill_disk\"\ntarget = \"cache\"\nvolume = \"cache-data\"\nleave_mib = 16\nat_secs = 1\nfor_secs = 4\nrecover_within_secs = 5\nprobe = { service = \"api\" }\nexpect = { max_error_percent = 100 }\n",
        );
        let topology = parse(&fill).unwrap();
        assert_eq!(topology.resilience[0].volume.as_deref(), Some("cache-data"));
        assert_eq!(topology.resilience[0].leave_mib, 16);
        let other = fill.replace("cache-data\"\nleave", "api-data\"\nleave");
        assert!(parse(&other).unwrap_err().contains("does not mount"));
        let unsized_text = RESILIENCE.replace(", size_mib = 4", "");
        let no_size = format!(
            "{unsized_text}[[resilience]]\nid = \"full-disk\"\naction = \"fill_disk\"\ntarget = \"cache\"\nvolume = \"cache-data\"\nat_secs = 1\nfor_secs = 4\nrecover_within_secs = 5\nprobe = {{ service = \"api\" }}\nexpect = {{ max_error_percent = 100 }}\n"
        );
        assert!(parse(&no_size).unwrap_err().contains("no size_mib"));
        let too_big = RESILIENCE.replace("size_mib = 4", "size_mib = 4096");
        assert!(parse(&too_big).unwrap_err().contains("MiB allowed"));

        let skew = with(
            "id = \"skew\"\naction = \"clock_skew\"\ntarget = \"api\"\nsecs = 3600\nat_secs = 1\nfor_secs = 4\nrecover_within_secs = 5\nprobe = { service = \"api\" }\nexpect = { max_error_percent = 0 }\n",
        );
        assert_eq!(parse(&skew).unwrap().resilience[0].secs, Some(3600));
        let none = skew.replace("secs = 3600", "secs = 0");
        assert!(parse(&none).unwrap_err().contains("clock skew"));
    }

    #[test]
    fn a_topology_with_scenarios_survives_its_own_toml() {
        let text = with(
            "id = \"cache-outage\"\naction = \"fault\"\ntarget = \"cache\"\nfault = { kind = \"partition\" }\nat_secs = 2\nfor_secs = 5\nrecover_within_secs = 10\nprobe = { service = \"api\", path = \"/items\" }\nexpect = { max_error_percent = 5.0 }\n",
        );
        let topology = parse(&text).unwrap();
        let again = to_toml(&topology).unwrap();
        assert_eq!(parse(&again).unwrap(), topology, "{again}");
    }
}
