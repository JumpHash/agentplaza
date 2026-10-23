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

/// The directory of a project's topology files in its repo at main.
pub const DIR: &str = ".plaza/topologies";
/// The most replicas one service may declare (a lab VM is sized for its topology).
pub const MAX_REPLICAS: u32 = 16;

/// A topology: its name, what it runs, its services (or its k3s cluster), what it observes, its traffic and its egress
/// allowlist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Topology {
    pub name: String,
    /// `services` (the default: containers on the VM's bridge) or `k3s` (a Kubernetes cluster in the VM, with `[k3s]`).
    #[serde(default)]
    pub kind: Kind,
    #[serde(rename = "service", default)]
    pub services: Vec<Service>,
    #[serde(default)]
    pub k3s: Option<K3s>,
    #[serde(default)]
    pub observe: Observe,
    #[serde(default)]
    pub traffic: Option<Traffic>,
    #[serde(default)]
    pub egress: Egress,
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

/// The hosts the topology may reach through the egress proxy, beyond the images and the artifacts it already holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Egress {
    pub allow: Vec<String>,
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
fn is_repo_path(text: &str) -> bool {
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
}
