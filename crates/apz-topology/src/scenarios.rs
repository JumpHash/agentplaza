//! The lab VM's resilience run (PLAN 22.8): the topology's scenarios run one after another against the services of the
//! guest, each from its own time zero. [`Config`] is what the guest's bin reads (the fault proxies in front of the
//! dependencies, the services' URLs and processes, the volumes, the collector); [`run_all`] starts the proxies, runs each
//! scenario and returns what each measured or why it did not run. [`record`] posts the measurements to the run.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::collector::{Relay, now_ms};
use crate::fault::{Fault, Proxy};
use crate::plan::GuestPlan;
use crate::resilience::{self, Ran};
use crate::spec::{Action, Topology};
use crate::steps::{Commands, Guest, Service, System};

fn default_device() -> String {
    crate::plan::BRIDGE.into()
}

/// One fault proxy: where it listens, and the dependency it forwards to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyConfig {
    pub listen: String,
    pub upstream: String,
}

/// A service's process in the guest: the pid file the guest writes, and the command that starts it again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceConfig {
    pub pid_file: PathBuf,
    #[serde(default)]
    pub start: Vec<String>,
}

/// A volume of a service and the directory it is mounted at in the guest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VolumeConfig {
    pub service: String,
    pub volume: String,
    pub dir: PathBuf,
}

/// Where the run's measurements are posted: the run's collector relay and its token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Collector {
    pub upstream: String,
    pub token: String,
}

/// The guest's configuration for a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The fault proxies by dependency (the service whose traffic they carry).
    #[serde(default)]
    pub proxies: BTreeMap<String, ProxyConfig>,
    /// The base URL of each service the probes reach.
    #[serde(default)]
    pub urls: BTreeMap<String, String>,
    #[serde(default)]
    pub services: BTreeMap<String, ServiceConfig>,
    #[serde(default)]
    pub volumes: Vec<VolumeConfig>,
    /// The bridge netem shapes.
    #[serde(default = "default_device")]
    pub device: String,
    #[serde(default)]
    pub collector: Option<Collector>,
}

/// What one scenario did: its id, and what it measured or why it did not run.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub id: String,
    pub ran: Ran,
}

/// The guest config of a planned topology (PLAN 22.8): a fault proxy on the bridge's gateway in front of each dependency
/// that a fault scenario names, listening on the dependency's first port and forwarding to its first replica, and the URL
/// of each service's first replica, which the probes reach. The services' processes and volumes exist only once the run
/// starts, so the config names none: a kill or a fill is reported as not run until they do.
pub fn config_for(
    topology: &Topology,
    plan: &GuestPlan,
    collector: Option<Collector>,
) -> Result<Config, String> {
    let first = |service: &str| {
        plan.containers
            .iter()
            .find(|c| c.service == service && c.replica == 1)
    };
    let mut proxies = BTreeMap::new();
    for scenario in &topology.resilience {
        let netem = matches!(scenario.fault, Some(Fault::Netem { .. }));
        if scenario.action != Action::Fault || netem || proxies.contains_key(&scenario.target) {
            continue;
        }
        let target = scenario.target.as_str();
        let container =
            first(target).ok_or_else(|| format!("{target} has no container in the plan"))?;
        let port = *container.ports.first().ok_or_else(|| {
            format!(
                "{target} is faulted by {} and has no port to proxy",
                scenario.id
            )
        })?;
        proxies.insert(
            target.to_string(),
            ProxyConfig {
                listen: SocketAddr::new(IpAddr::V4(plan.gateway), port).to_string(),
                upstream: SocketAddr::new(IpAddr::V4(container.address), port).to_string(),
            },
        );
    }
    let mut urls = BTreeMap::new();
    for service in &topology.services {
        if let Some(container) = first(&service.name)
            && let Some(port) = container.ports.first()
        {
            urls.insert(
                service.name.clone(),
                format!("http://{}:{port}", container.address),
            );
        }
    }
    Ok(Config {
        proxies,
        urls,
        services: BTreeMap::new(),
        volumes: Vec::new(),
        device: plan.bridge.clone(),
        collector,
    })
}

/// Starts the proxies the topology's fault scenarios need, then runs its scenarios in order. A fault on a dependency with
/// no proxy in the config is refused before anything starts.
pub fn run_all(topology: &Topology, config: &Config) -> Result<Vec<Outcome>, String> {
    for scenario in &topology.resilience {
        if scenario.action == Action::Fault
            && !matches!(scenario.fault, Some(Fault::Netem { .. }))
            && !config.proxies.contains_key(&scenario.target)
        {
            return Err(format!(
                "scenario {} faults {}, which has no proxy in the config",
                scenario.id, scenario.target
            ));
        }
    }
    let mut proxies = BTreeMap::new();
    for (name, proxy) in &config.proxies {
        let listen: SocketAddr = proxy
            .listen
            .parse()
            .map_err(|_| format!("{name}: {}", proxy.listen))?;
        let upstream: SocketAddr = proxy
            .upstream
            .parse()
            .map_err(|_| format!("{name}: {}", proxy.upstream))?;
        let started =
            Proxy::start(listen, upstream, Fault::Healthy).map_err(|e| format!("{name}: {e}"))?;
        proxies.insert(name.clone(), started);
    }
    let mut guest = Guest {
        host: System {
            path: std::env::var("PATH").unwrap_or_default(),
        },
        runner: Commands,
        proxies,
        services: config
            .services
            .iter()
            .map(|(name, s)| {
                (
                    name.clone(),
                    Service {
                        pid_file: s.pid_file.clone(),
                        start: s.start.clone(),
                    },
                )
            })
            .collect(),
        volumes: config
            .volumes
            .iter()
            .map(|v| ((v.service.clone(), v.volume.clone()), v.dir.clone()))
            .collect(),
        device: config.device.clone(),
    };
    let mut results = Vec::new();
    for scenario in &topology.resilience {
        let ran = match config.urls.get(&scenario.probe.service) {
            Some(base) => {
                let url = resilience::probe_url(base, &scenario.probe);
                resilience::run(
                    scenario,
                    &url,
                    resilience::Schedule::of(scenario),
                    &mut guest,
                )
            }
            None => Ran::NotRun(format!(
                "no URL for the probed service {}",
                scenario.probe.service
            )),
        };
        results.push(Outcome {
            id: scenario.id.clone(),
            ran,
        });
    }
    Ok(results)
}

/// Posts each measurement to the run's collector as gauges (the hub judges them against the topology's expectations).
/// Returns the scenarios that were recorded.
pub fn record(config: &Config, results: &[Outcome]) -> Result<usize, String> {
    let Some(collector) = &config.collector else {
        return Ok(0);
    };
    let relay = Relay::new(&collector.upstream, &collector.token);
    let mut recorded = 0;
    for result in results {
        if let Ran::Measured(measured) = &result.ran {
            let body = resilience::otlp(&result.id, measured, now_ms()).to_string();
            relay.post("metrics", body.as_bytes()).map_err(|why| {
                format!(
                    "the measurements of {} were not recorded at the run: {why}",
                    result.id
                )
            })?;
            recorded += 1;
        }
    }
    Ok(recorded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::plan;
    use crate::spec;

    const STACK: &str = "name = \"stack\"\n[[service]]\nname = \"api\"\nartifact = \"api-linux-x86_64\"\nports = [8080]\nenv = { CACHE_URL = \"redis://cache:6379\" }\n[[service]]\nname = \"cache\"\nimage = \"redis:7\"\nports = [6379]\n[[resilience]]\nid = \"cache-outage\"\naction = \"fault\"\ntarget = \"cache\"\nfault = { kind = \"partition\" }\nat_secs = 2\nfor_secs = 5\nrecover_within_secs = 10\nprobe = { service = \"api\", path = \"/items\" }\nexpect = { max_error_percent = 5.0 }\n";

    #[test]
    fn a_planned_topology_gets_a_proxy_on_the_gateway_in_front_of_its_faulted_dependency() {
        let topology = spec::parse(STACK).unwrap();
        let artifacts = BTreeMap::from([("api-linux-x86_64".to_string(), "/opt/api".to_string())]);
        let planned = plan(&topology, &artifacts).unwrap();
        let config = config_for(&topology, &planned, None).unwrap();
        let cache = planned
            .containers
            .iter()
            .find(|c| c.service == "cache")
            .unwrap();
        let api = planned
            .containers
            .iter()
            .find(|c| c.service == "api")
            .unwrap();
        assert_eq!(
            config.proxies["cache"].listen,
            format!("{}:6379", planned.gateway)
        );
        assert_eq!(
            config.proxies["cache"].upstream,
            format!("{}:6379", cache.address)
        );
        assert_eq!(config.urls["api"], format!("http://{}:8080", api.address));
        assert_eq!(config.device, planned.bridge);
        assert_eq!(
            config.urls["cache"],
            format!("http://{}:6379", cache.address)
        );
    }

    #[test]
    fn a_faulted_dependency_with_no_port_is_refused_by_the_plan_config() {
        let text = STACK.replace("ports = [6379]", "");
        let topology = spec::parse(&text).unwrap();
        let artifacts = BTreeMap::from([("api-linux-x86_64".to_string(), "/opt/api".to_string())]);
        let planned = plan(&topology, &artifacts).unwrap();
        assert!(
            config_for(&topology, &planned, None)
                .unwrap_err()
                .contains("no port")
        );
    }
}
