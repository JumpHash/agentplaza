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

/// A topology: its name, its services, what it observes, its traffic and its egress allowlist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Topology {
    pub name: String,
    #[serde(rename = "service", default)]
    pub services: Vec<Service>,
    #[serde(default)]
    pub observe: Observe,
    #[serde(default)]
    pub traffic: Option<Traffic>,
    #[serde(default)]
    pub egress: Egress,
}

/// One service: its program (an artifact of the release or a pinned image), its replicas, ports, environment and data.
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

impl Topology {
    /// What TOML cannot say: the names, the sources, the replicas, the ports, the URLs and the references.
    pub fn check(&self) -> Result<(), String> {
        if !is_label(&self.name) {
            return Err(format!("topology name {:?} is not a DNS label", self.name));
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
        Ok(())
    }
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
fn is_label(text: &str) -> bool {
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
}
