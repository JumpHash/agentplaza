//! The guest plan of a topology (PLAN 22.6): what one lab VM runs for it, and the network it runs on (pure).
//!
//! One lab VM per topology. Every replica of every service is a container (or, where the VM has no container runtime,
//! a process) on the VM's own bridge, `apz0`, with addresses from `10.88.0.0/24`. Its service name resolves to its
//! replicas inside the VM ([`crate::resolver`]). The VM has no network device: its only way out is the egress proxy,
//! vsockd's, at the guest's `127.0.0.1:3128` over vsock, and the containers reach it through a relay on the bridge's
//! gateway (`10.88.0.1:3128`). The containers have no default route, so the proxy is their only way out, and every
//! container's proxy variables name it.
//!
//! [`plan`] names the containers, their addresses, their programs and their environment; [`bridge_commands`] are the
//! `ip` commands that build the network inside the guest.

use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

use crate::spec::{Data, Observe, Serve, Topology, Traffic};

/// The bridge inside the guest.
pub const BRIDGE: &str = "apz0";
/// The bridge's gateway: the containers' address space starts after it.
pub const GATEWAY: Ipv4Addr = Ipv4Addr::new(10, 88, 0, 1);
/// The prefix length of the bridge's network.
pub const PREFIX: u8 = 24;
/// The relay on the gateway that carries a container's proxy traffic to the guest's proxy.
pub const PROXY_URL: &str = "http://10.88.0.1:3128";
/// The most containers a topology's VM holds (`.2` to `.254` on the bridge).
pub const MAX_CONTAINERS: usize = 253;

/// A topology's guest plan: the containers of its lab VM, its network and its egress, and what its lab base holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestPlan {
    pub topology: String,
    pub bridge: String,
    pub gateway: Ipv4Addr,
    pub prefix: u8,
    /// The proxy the containers' variables name (the relay on the gateway).
    pub proxy: String,
    /// The hosts the egress proxy lets through, from the topology's `[egress]`.
    pub allow: Vec<String>,
    pub containers: Vec<Container>,
    pub observe: Observe,
    pub traffic: Option<Traffic>,
    /// The pinned images the lab base holds warm (sorted, each once).
    pub pinned_images: Vec<String>,
    /// The seed sets the lab base loads (sorted, each once).
    pub seeds: Vec<String>,
}

/// One replica of a service: a container with its address on the bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Container {
    /// `<service>-<replica>`, replicas numbered from 1.
    pub name: String,
    pub service: String,
    pub replica: u32,
    pub program: Program,
    pub serve: Option<Serve>,
    pub address: Ipv4Addr,
    pub ports: Vec<u16>,
    /// The service's environment, with the proxy variables set by the plan (they are not the service's to change).
    pub env: BTreeMap<String, String>,
    pub data: Option<Data>,
}

/// What a container runs: an image the runtime pulls from the lab base, or an artifact of the release, by its path in
/// the guest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Program {
    Image { image: String },
    Artifact { path: String },
}

/// The plan of `topology`, its artifacts (name to the path the release puts them at in the guest) given. Every
/// artifact a service names must be among them; an invalid topology is refused by its own check.
pub fn plan(
    topology: &Topology,
    artifacts: &BTreeMap<String, String>,
) -> Result<GuestPlan, String> {
    topology.check()?;
    let services: BTreeSet<&str> = topology.services.iter().map(|s| s.name.as_str()).collect();
    let no_proxy = std::iter::once("localhost".to_string())
        .chain(std::iter::once("127.0.0.1".to_string()))
        .chain(services.iter().map(|name| name.to_string()))
        .collect::<Vec<String>>()
        .join(",");

    let mut containers = Vec::new();
    for service in &topology.services {
        let program = match (&service.artifact, &service.image) {
            (Some(artifact), None) => Program::Artifact {
                path: artifacts.get(artifact).cloned().ok_or_else(|| {
                    format!(
                        "service {}: artifact {artifact} is not in the release under test",
                        service.name
                    )
                })?,
            },
            (None, Some(image)) => Program::Image {
                image: image.clone(),
            },
            _ => return Err(format!("service {} has no single source", service.name)),
        };
        for replica in 1..=service.replicas {
            let index = containers.len();
            if index >= MAX_CONTAINERS {
                return Err(format!(
                    "topology {} has more than {MAX_CONTAINERS} containers: the VM's bridge has no room",
                    topology.name
                ));
            }
            let mut env = service.env.clone();
            env.insert("HTTP_PROXY".into(), PROXY_URL.into());
            env.insert("HTTPS_PROXY".into(), PROXY_URL.into());
            env.insert("NO_PROXY".into(), no_proxy.clone());
            containers.push(Container {
                name: format!("{}-{replica}", service.name),
                service: service.name.clone(),
                replica,
                program: program.clone(),
                serve: service.serve,
                address: address(index),
                ports: service.ports.clone(),
                env,
                data: service.data.clone(),
            });
        }
    }

    let pinned_images: BTreeSet<String> = topology
        .services
        .iter()
        .filter_map(|service| service.image.clone())
        .collect();
    let seeds: BTreeSet<String> = topology
        .services
        .iter()
        .filter_map(|service| match &service.data {
            Some(Data::Seed(name)) => Some(name.clone()),
            _ => None,
        })
        .collect();

    Ok(GuestPlan {
        topology: topology.name.clone(),
        bridge: BRIDGE.into(),
        gateway: GATEWAY,
        prefix: PREFIX,
        proxy: PROXY_URL.into(),
        allow: topology.egress.allow.clone(),
        containers,
        observe: topology.observe.clone(),
        traffic: topology.traffic.clone(),
        pinned_images: pinned_images.into_iter().collect(),
        seeds: seeds.into_iter().collect(),
    })
}

/// The address of the container at `index` on the bridge (the first is `.2`).
fn address(index: usize) -> Ipv4Addr {
    // plan() bounds the index by MAX_CONTAINERS, so the address stays on the bridge's network.
    Ipv4Addr::from(u32::from(GATEWAY) + 1 + index as u32)
}

/// The `ip` commands that build a plan's network inside the guest, in order: the bridge, then for each container a
/// network namespace joined to it by a veth pair, with its address and loopback up, and no default route (pure).
pub fn bridge_commands(plan: &GuestPlan) -> Vec<Vec<String>> {
    let mut commands = vec![
        cmd(&["ip", "link", "add", &plan.bridge, "type", "bridge"]),
        cmd(&[
            "ip",
            "addr",
            "add",
            &format!("{}/{}", plan.gateway, plan.prefix),
            "dev",
            &plan.bridge,
        ]),
        cmd(&["ip", "link", "set", &plan.bridge, "up"]),
    ];
    for (index, container) in plan.containers.iter().enumerate() {
        let host = format!("vh{index}");
        let guest = format!("vg{index}");
        let ns = container.name.as_str();
        let address = format!("{}/{}", container.address, plan.prefix);
        commands.push(cmd(&["ip", "netns", "add", ns]));
        commands.push(cmd(&[
            "ip", "link", "add", &host, "type", "veth", "peer", "name", &guest,
        ]));
        commands.push(cmd(&["ip", "link", "set", &guest, "netns", ns]));
        commands.push(cmd(&["ip", "link", "set", &host, "master", &plan.bridge]));
        commands.push(cmd(&["ip", "link", "set", &host, "up"]));
        commands.push(cmd(&[
            "ip", "netns", "exec", ns, "ip", "link", "set", &guest, "name", "eth0",
        ]));
        commands.push(cmd(&[
            "ip", "netns", "exec", ns, "ip", "addr", "add", &address, "dev", "eth0",
        ]));
        commands.push(cmd(&[
            "ip", "netns", "exec", ns, "ip", "link", "set", "eth0", "up",
        ]));
        commands.push(cmd(&[
            "ip", "netns", "exec", ns, "ip", "link", "set", "lo", "up",
        ]));
    }
    commands
}

/// A command's argument vector from its parts.
fn cmd(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| part.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec;

    const WEB_STACK: &str = include_str!("../tests/fixtures/web-stack.toml");

    fn artifacts() -> BTreeMap<String, String> {
        [
            ("api-linux-x86_64", "/opt/apz/api"),
            ("web-dist", "/opt/apz/web"),
        ]
        .into_iter()
        .map(|(name, path)| (name.to_string(), path.to_string()))
        .collect()
    }

    fn web_stack() -> GuestPlan {
        plan(&spec::parse(WEB_STACK).unwrap(), &artifacts()).unwrap()
    }

    #[test]
    fn the_web_stack_plans_five_containers_on_the_bridge_by_service_name() {
        let plan = web_stack();
        let names: Vec<&str> = plan.containers.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["api-1", "api-2", "db-1", "cache-1", "web-1"]);
        let addresses: Vec<String> = plan
            .containers
            .iter()
            .map(|c| c.address.to_string())
            .collect();
        assert_eq!(
            addresses,
            [
                "10.88.0.2",
                "10.88.0.3",
                "10.88.0.4",
                "10.88.0.5",
                "10.88.0.6"
            ]
        );
        assert_eq!(plan.containers[0].service, "api");
        assert_eq!(plan.containers[1].replica, 2);
        assert_eq!(
            plan.containers[0].program,
            Program::Artifact {
                path: "/opt/apz/api".into()
            }
        );
        assert_eq!(
            plan.containers[2].program,
            Program::Image {
                image: "postgres:17".into()
            }
        );
        assert_eq!(plan.containers[4].serve, Some(Serve::Static));
    }

    #[test]
    fn a_containers_environment_keeps_the_service_variables_and_names_the_proxy() {
        let plan = web_stack();
        let api = &plan.containers[0];
        assert_eq!(api.env["DATABASE_URL"], "postgres://app@db/app");
        assert_eq!(api.env["HTTP_PROXY"], PROXY_URL);
        assert_eq!(api.env["HTTPS_PROXY"], PROXY_URL);
        assert_eq!(api.env["NO_PROXY"], "localhost,127.0.0.1,api,cache,db,web");
    }

    #[test]
    fn the_lab_base_holds_the_pinned_images_and_the_seeds_once() {
        let plan = web_stack();
        assert_eq!(plan.pinned_images, ["postgres:17", "redis:7"]);
        assert_eq!(plan.seeds, ["app-small"]);
        assert_eq!(plan.proxy, PROXY_URL);
    }

    #[test]
    fn a_missing_artifact_is_named_and_a_plan_past_the_bridge_is_refused() {
        let mut none = BTreeMap::new();
        none.insert("web-dist".to_string(), "/opt/apz/web".to_string());
        let why = plan(&spec::parse(WEB_STACK).unwrap(), &none).unwrap_err();
        assert!(why.contains("api-linux-x86_64"), "{why}");

        let mut big = String::from("name = \"big\"\n");
        for i in 0..17 {
            big.push_str(&format!(
                "[[service]]\nname = \"s{i}\"\nimage = \"y\"\nreplicas = 16\n"
            ));
        }
        assert!(
            plan(&spec::parse(&big).unwrap(), &artifacts())
                .unwrap_err()
                .contains("bridge")
        );
        assert!(
            plan(&spec::parse(&big).unwrap(), &artifacts())
                .unwrap_err()
                .contains("bridge")
        );
    }

    #[test]
    fn the_bridge_commands_build_the_network_and_give_no_default_route() {
        let plan = web_stack();
        let commands = bridge_commands(&plan);
        assert_eq!(commands[0].join(" "), "ip link add apz0 type bridge");
        assert_eq!(commands[1].join(" "), "ip addr add 10.88.0.1/24 dev apz0");
        // Three commands for the bridge, nine per container.
        assert_eq!(commands.len(), 3 + 9 * plan.containers.len());
        assert!(
            commands
                .iter()
                .any(|c| c.join(" ") == "ip netns exec api-2 ip addr add 10.88.0.3/24 dev eth0")
        );
        assert!(!commands.iter().any(|c| c.contains(&"route".to_string())));
    }
}
