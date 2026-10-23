//! The process runner (PLAN 22.6): a plan's containers started as processes, their names resolving through the runner's
//! resolver ([`crate::resolver`]).
//!
//! In the lab VM the runner starts each container's artifact on its bridge address, after `apz-topo-run --net` has built
//! the network. Where a test has no container runtime, [`Addressing::Loopback`] gives each container its own loopback
//! address instead (`127.0.1.<n>`, all of 127/8 is local on Linux), so the same plan runs as processes on one machine. A
//! container runs with its environment only (cleared of the runner's own), plus the runner's variables:
//!
//! - `APZ_TOPO_SERVICE`, `APZ_TOPO_CONTAINER`: its service and its container name.
//! - `APZ_TOPO_BIND`: the address it listens on; `APZ_TOPO_PORTS`: its ports, comma separated.
//! - `APZ_TOPO_RESOLVER`: the runner's resolver, where a name is asked for.
//!
//! Services start in dependency order: a service with `depends_on` starts once its dependencies' ports accept connections
//! (their readiness here; a dependency without ports starts without a wait). A named volume is a directory under the run's
//! scratch root, removed when the run drops, given to each container that mounts it as `APZ_TOPO_VOLUME_<NAME>`, and
//! `APZ_TOPO_MOUNTS` lists `name=target[:ro]`: a process has no mount namespace, so the target is the program's to use.
//!
//! An image needs the guest's container runtime, which the runner does not start: such a container is refused, and the
//! check that runs one on a KVM host is named in the report.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::plan::{Container, GuestPlan, Program};
use crate::resolver::{self, Table};

/// How long a container waits for its dependencies' ports before it starts anyway, and the run fails.
const DEPENDENCY_WAIT: Duration = Duration::from_secs(30);
/// Tells the scratch roots of runs in one process apart.
static RUNS: AtomicU64 = AtomicU64::new(0);

/// Where the containers listen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Addressing {
    /// The addresses the plan gives (the bridge's, in the lab VM).
    Plan,
    /// One loopback address per container, `127.0.1.<n>` for the container at index `n - 1`.
    Loopback,
}

/// A stand-in for a service's program: the command that runs in its place, with its own environment (a test's).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Standin {
    /// The program and its arguments.
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// A started plan: its containers' processes and its resolver. Dropping it stops them all and removes its volumes.
pub struct Running {
    server: resolver::Server,
    children: Vec<(String, Child)>,
    /// Each container's name, service, address and ports.
    placed: Vec<(String, String, Ipv4Addr, Vec<u16>)>,
    /// The containers' names in the order they were started.
    started: Vec<String>,
    /// The run's scratch root, holding its named volumes (None when the plan has none).
    scratch: Option<PathBuf>,
    /// Each named volume's directory.
    volumes: BTreeMap<String, PathBuf>,
}

/// Starts every container of `plan`. A service with a stand-in runs the stand-in; otherwise its artifact runs. An image
/// container without a stand-in is refused before any process starts, so a refused plan starts nothing.
pub fn start(
    plan: &GuestPlan,
    addressing: Addressing,
    standins: &BTreeMap<String, Standin>,
) -> Result<Running, String> {
    let mut table = Table::default();
    let mut launches = Vec::new();
    for (index, container) in plan.containers.iter().enumerate() {
        let addr = match addressing {
            Addressing::Plan => container.address,
            Addressing::Loopback => {
                let last = u8::try_from(index + 1)
                    .map_err(|_| "too many containers for loopback".to_string())?;
                Ipv4Addr::new(127, 0, 1, last)
            }
        };
        let standin = standins.get(&container.service);
        let argv = match (standin, &container.program) {
            (Some(standin), _) => standin.argv.clone(),
            (None, Program::Artifact { path }) => vec![path.clone()],
            (None, Program::Image { image }) => {
                return Err(format!(
                    "{}: image {image} needs the guest's container runtime, which the runner does not start",
                    container.name
                ));
            }
        };
        table.add(&container.service, addr);
        table.add(&container.name, addr);
        launches.push((container, addr, argv, standin));
    }
    let server = resolver::serve(table).map_err(|e| format!("resolver: {e}"))?;
    let resolver_addr = server.addr();
    let (volumes, scratch) = volume_dirs(plan)?;
    let mut running = Running {
        server,
        children: Vec::new(),
        placed: launches
            .iter()
            .map(|(c, addr, _, _)| (c.name.clone(), c.service.clone(), *addr, c.ports.clone()))
            .collect(),
        started: Vec::new(),
        scratch,
        volumes: volumes.clone(),
    };
    for index in start_order(plan)? {
        let (container, addr, argv, standin) = &launches[index];
        wait_for_dependencies(&running, container)?;
        let extra = volume_env(container, &volumes);
        let child = spawn(container, *addr, argv, *standin, resolver_addr, &extra)?;
        running.children.push((container.name.clone(), child));
        running.started.push(container.name.clone());
    }
    Ok(running)
}

/// The containers' indices in start order: each service after the services it depends on, the rest in plan order.
fn start_order(plan: &GuestPlan) -> Result<Vec<usize>, String> {
    let mut services: Vec<&str> = Vec::new();
    for container in &plan.containers {
        if !services.contains(&container.service.as_str()) {
            services.push(container.service.as_str());
        }
    }
    let waits: BTreeMap<&str, &[String]> = plan
        .containers
        .iter()
        .map(|c| (c.service.as_str(), c.depends_on.as_slice()))
        .collect();
    let mut done: BTreeSet<&str> = BTreeSet::new();
    let mut ordered: Vec<&str> = Vec::new();
    for service in services {
        visit(service, &waits, &mut Vec::new(), &mut done, &mut ordered)?;
    }
    Ok(ordered
        .iter()
        .flat_map(|service| {
            plan.containers
                .iter()
                .enumerate()
                .filter(move |(_, c)| c.service == *service)
                .map(|(index, _)| index)
        })
        .collect())
}

fn visit<'a>(
    service: &'a str,
    waits: &BTreeMap<&'a str, &'a [String]>,
    path: &mut Vec<&'a str>,
    done: &mut BTreeSet<&'a str>,
    ordered: &mut Vec<&'a str>,
) -> Result<(), String> {
    if done.contains(service) {
        return Ok(());
    }
    if path.contains(&service) {
        return Err(format!("depends_on has a cycle through {service}"));
    }
    path.push(service);
    for dependency in waits.get(service).copied().unwrap_or(&[]) {
        visit(dependency, waits, path, done, ordered)?;
    }
    path.pop();
    done.insert(service);
    ordered.push(service);
    Ok(())
}

/// Waits until the ports of every replica of a container's dependencies accept connections.
fn wait_for_dependencies(running: &Running, container: &Container) -> Result<(), String> {
    let deadline = Instant::now() + DEPENDENCY_WAIT;
    for dependency in &container.depends_on {
        for (name, service, addr, ports) in &running.placed {
            if service == dependency {
                wait_ports(name, *addr, ports, deadline).map_err(|why| {
                    format!("{} starts after {dependency}, but {why}", container.name)
                })?;
            }
        }
    }
    Ok(())
}

/// Makes the directory of each named volume the plan mounts, under one scratch root (None when there is none).
fn volume_dirs(plan: &GuestPlan) -> Result<(BTreeMap<String, PathBuf>, Option<PathBuf>), String> {
    let names: BTreeSet<&str> = plan
        .containers
        .iter()
        .flat_map(|c| c.volumes.iter().map(|v| v.name.as_str()))
        .collect();
    if names.is_empty() {
        return Ok((BTreeMap::new(), None));
    }
    let root = std::env::temp_dir().join(format!(
        "apz-topo-{}-{}",
        std::process::id(),
        RUNS.fetch_add(1, Ordering::Relaxed)
    ));
    let mut dirs = BTreeMap::new();
    for name in names {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).map_err(|e| format!("volume {name}: {e}"))?;
        dirs.insert(name.to_string(), dir);
    }
    Ok((dirs, Some(root)))
}

/// The volume variables a container gets: the directory of each volume it mounts, and the mounts as `name=target`.
fn volume_env(container: &Container, dirs: &BTreeMap<String, PathBuf>) -> Vec<(String, String)> {
    let mut env = Vec::new();
    let mut mounts = Vec::new();
    for volume in &container.volumes {
        if let Some(dir) = dirs.get(&volume.name) {
            let key = format!(
                "APZ_TOPO_VOLUME_{}",
                volume.name.to_uppercase().replace('-', "_")
            );
            env.push((key, dir.display().to_string()));
        }
        let ro = if volume.read_only { ":ro" } else { "" };
        mounts.push(format!("{}={}{ro}", volume.name, volume.target));
    }
    if !mounts.is_empty() {
        env.push(("APZ_TOPO_MOUNTS".to_string(), mounts.join(",")));
    }
    env
}

fn spawn(
    container: &Container,
    addr: Ipv4Addr,
    argv: &[String],
    standin: Option<&Standin>,
    resolver_addr: SocketAddr,
    extra: &[(String, String)],
) -> Result<Child, String> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| format!("{} has an empty command", container.name))?;
    let ports: Vec<String> = container.ports.iter().map(u16::to_string).collect();
    let mut command = Command::new(program);
    command
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .envs(&container.env)
        .env("APZ_TOPO_SERVICE", &container.service)
        .env("APZ_TOPO_CONTAINER", &container.name)
        .env("APZ_TOPO_BIND", addr.to_string())
        .env("APZ_TOPO_PORTS", ports.join(","))
        .env("APZ_TOPO_RESOLVER", resolver_addr.to_string())
        .envs(extra.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    if let Some(standin) = standin {
        command.envs(&standin.env);
    }
    command
        .spawn()
        .map_err(|e| format!("{}: cannot start {program}: {e}", container.name))
}

impl Running {
    /// The address of the runner's resolver.
    pub fn resolver(&self) -> SocketAddr {
        self.server.addr()
    }

    /// The addresses of a service's replicas, in plan order.
    pub fn addresses(&self, service: &str) -> Vec<Ipv4Addr> {
        self.placed
            .iter()
            .filter(|(_, name, _, _)| name == service)
            .map(|(_, _, addr, _)| *addr)
            .collect()
    }

    /// Waits until every container's ports accept connections, or `timeout` passes.
    pub fn wait_listening(&self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        for (name, _, addr, ports) in &self.placed {
            wait_ports(name, *addr, ports, deadline)?;
        }
        Ok(())
    }

    /// The directory of a named volume of the plan, while the run lasts.
    pub fn volume(&self, name: &str) -> Option<&std::path::Path> {
        self.volumes.get(name).map(PathBuf::as_path)
    }

    /// The containers' names in the order they started (a dependency before its dependents).
    pub fn started_order(&self) -> &[String] {
        &self.started
    }

    /// The containers whose processes have ended, with their exit status (a process still running is not listed).
    pub fn exited(&mut self) -> Vec<(String, ExitStatus)> {
        let mut ended = Vec::new();
        for (name, child) in &mut self.children {
            if let Ok(Some(status)) = child.try_wait() {
                ended.push((name.clone(), status));
            }
        }
        ended
    }
}

/// Waits until each of `ports` at `addr` accepts a connection, or `deadline` passes.
fn wait_ports(name: &str, addr: Ipv4Addr, ports: &[u16], deadline: Instant) -> Result<(), String> {
    for port in ports {
        loop {
            if TcpStream::connect_timeout(
                &SocketAddr::new(addr.into(), *port),
                Duration::from_millis(100),
            )
            .is_ok()
            {
                break;
            }
            if Instant::now() >= deadline {
                return Err(format!("{name} is not listening on {addr}:{port}"));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    Ok(())
}

impl Drop for Running {
    fn drop(&mut self) {
        for (_, child) in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(scratch) = self.scratch.take() {
            let _ = std::fs::remove_dir_all(scratch);
        }
    }
}
