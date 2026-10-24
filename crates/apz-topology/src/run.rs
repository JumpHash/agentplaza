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
//! - `OTEL_SERVICE_NAME` (the container's name), and with OTLP on, `OTEL_EXPORTER_OTLP_ENDPOINT` (the collector) and
//!   `OTEL_EXPORTER_OTLP_PROTOCOL=http/json`.
//!
//! A container's standard output and error go to `<container>.log` in the run's log directory ([`Observed::logs_dir`],
//! the scratch directory's `logs` by default), which the lab VM's collector tails ([`crate::collector`]).
//!
//! Services start in dependency order: a service with `depends_on` starts once its dependencies' ports accept connections
//! (their readiness here; a dependency without ports starts without a wait). A named volume is a directory under the run's
//! scratch root, removed when the run drops, given to each container that mounts it as `APZ_TOPO_VOLUME_<NAME>`, and
//! `APZ_TOPO_MOUNTS` lists `name=target[:ro]`: a process has no mount namespace, so the target is the program's to use.
//!
//! An image needs the guest's container runtime, which the runner does not start: such a container is refused, and the
//! check that runs one on a KVM host is named in the report.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use std::net::{IpAddr, TcpListener};

use crate::fault::{Fault, Remote};
use crate::plan::{Container, GuestPlan, Program};
use crate::resolver::{self, Table};
use crate::spec::Action;
use crate::steps::Processes;

/// How long a container waits for its dependencies' ports before it starts anyway, and the run fails.
const DEPENDENCY_WAIT: Duration = Duration::from_secs(30);
/// The listen shift of a routed dependency under [`Addressing::Local`]: its real port is its declared port plus this, and
/// its fault proxy takes the declared one (every container shares 127.0.0.1 there).
pub const LOCAL_SHIFT: u16 = 10_000;
/// Tells the scratch roots of runs in one process apart.
static RUNS: AtomicU64 = AtomicU64::new(0);

/// Where the containers listen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Addressing {
    /// The addresses the plan gives (the bridge's, in the lab VM).
    Plan,
    /// One loopback address per container, `127.0.1.<n>` for the container at index `n - 1` (Linux).
    Loopback,
    /// Every container on 127.0.0.1, on its declared ports: a dependency a fault names listens on its port plus
    /// [`LOCAL_SHIFT`], and its proxy takes the declared port. Runs on any host with a loopback interface.
    Local,
}

/// A stand-in for a service's program: the command that runs in its place, with its own environment (a test's).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Standin {
    /// The program and its arguments.
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// What a run observes beyond its exit status (PLAN 22.7): where the containers' output goes, and where their OTLP goes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Observed {
    /// The directory the containers' output goes to, one `<container>.log` each, for the collector to tail. The run's
    /// scratch directory when None (it goes with the run).
    pub logs_dir: Option<PathBuf>,
    /// The OTLP endpoint the containers export to, as JSON (`OTEL_EXPORTER_OTLP_ENDPOINT`). The plan's when None.
    pub otlp: Option<String>,
    /// The `apz-fault` binary that runs a fault proxy per routed dependency's port (`apz-fault proxy ...`). Required when
    /// the plan has a fault scenario on a dependency.
    pub fault_binary: Option<PathBuf>,
}

/// What a container was started with, kept so a restart starts it the same way.
#[derive(Debug, Clone)]
struct Launch {
    container: Container,
    addr: Ipv4Addr,
    argv: Vec<String>,
    standin: Option<Standin>,
    extra: Vec<(String, String)>,
}

/// A fault proxy the run started for one port of a routed dependency, with its control.
struct RunningProxy {
    service: String,
    control: Remote,
    child: Child,
}

/// A started plan: its containers' processes and its resolver. Dropping it stops them all and removes its scratch root
/// (its volumes and, unless the caller gave a directory, its logs).
pub struct Running {
    server: resolver::Server,
    children: Vec<(String, Child)>,
    /// Each container's name, service, address and ports.
    placed: Vec<(String, String, Ipv4Addr, Vec<u16>)>,
    /// The containers' names in the order they were started.
    started: Vec<String>,
    /// The run's scratch root, holding its named volumes and, unless given, its logs.
    scratch: PathBuf,
    /// Each named volume's directory.
    volumes: BTreeMap<String, PathBuf>,
    /// The directory of the containers' output.
    logs_dir: PathBuf,
    /// What each container was started with, by plan order (a restart starts it the same way).
    launches: Vec<Launch>,
    /// The fault proxies of the routed dependencies' ports.
    proxies: Vec<RunningProxy>,
    /// Each container's pid file, by container name (`<scratch>/pids/<container>.pid`).
    pids: BTreeMap<String, PathBuf>,
    /// The runner's resolver's address and the OTLP endpoint, as the containers were started with them.
    resolver: SocketAddr,
    otlp: Option<String>,
}

/// Starts every container of `plan`. A service with a stand-in runs the stand-in; otherwise its artifact runs. An image
/// container without a stand-in is refused before any process starts, so a refused plan starts nothing.
pub fn start(
    plan: &GuestPlan,
    addressing: Addressing,
    standins: &BTreeMap<String, Standin>,
) -> Result<Running, String> {
    start_observed(plan, addressing, standins, &Observed::default())
}

/// The dependencies a plan's fault scenarios name (not netem: that shapes the bridge, not a proxy's traffic).
fn routed(plan: &GuestPlan) -> BTreeSet<String> {
    plan.resilience
        .iter()
        .filter(|s| s.action == Action::Fault)
        .filter(|s| !matches!(s.fault, Some(Fault::Netem { .. })))
        .map(|s| s.target.clone())
        .collect()
}

/// A loopback port nobody listens on now.
fn free_port() -> Result<u16, String> {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| format!("a loopback port: {e}"))
}

/// [`start`], with what the run observes: the containers' output in `observed`'s directory, and their OTLP endpoint. A
/// dependency that a fault scenario names is routed through an apz-fault proxy per port: the resolver answers its name
/// with the proxy, and the proxy forwards to the dependency's replica.
pub fn start_observed(
    plan: &GuestPlan,
    addressing: Addressing,
    standins: &BTreeMap<String, Standin>,
    observed: &Observed,
) -> Result<Running, String> {
    let routed = routed(plan);
    if !routed.is_empty() && observed.fault_binary.is_none() {
        return Err("the plan's faults route through apz-fault proxies, and the run names no apz-fault binary".into());
    }
    let proxy_ip = match addressing {
        Addressing::Plan => plan.gateway,
        Addressing::Loopback | Addressing::Local => Ipv4Addr::LOCALHOST,
    };
    let shift = if addressing == Addressing::Local {
        LOCAL_SHIFT
    } else {
        0
    };
    let scratch = scratch_root()?;
    let volumes = volume_dirs(plan, &scratch)?;
    let logs_dir = observed
        .logs_dir
        .clone()
        .unwrap_or_else(|| scratch.join("logs"));
    std::fs::create_dir_all(&logs_dir).map_err(|e| format!("logs {}: {e}", logs_dir.display()))?;
    std::fs::create_dir_all(scratch.join("pids")).map_err(|e| format!("pids: {e}"))?;
    let otlp = observed.otlp.clone().or_else(|| plan.otlp.clone());

    let mut table = Table::default();
    let mut launches = Vec::new();
    let mut routed_done: BTreeSet<&str> = BTreeSet::new();
    for (index, container) in plan.containers.iter().enumerate() {
        let addr = match addressing {
            Addressing::Plan => container.address,
            Addressing::Loopback => {
                let last = u8::try_from(index + 1)
                    .map_err(|_| "too many containers for loopback".to_string())?;
                Ipv4Addr::new(127, 0, 1, last)
            }
            Addressing::Local => Ipv4Addr::LOCALHOST,
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
        let mut launched = container.clone();
        if routed.contains(&container.service) {
            // The dependency's name answers with its proxy; its real ports are shifted under the local addressing.
            if routed_done.insert(container.service.as_str()) {
                table.add(&container.service, proxy_ip);
            }
            launched.ports = container
                .ports
                .iter()
                .map(|port| port.checked_add(shift))
                .collect::<Option<Vec<u16>>>()
                .ok_or_else(|| {
                    format!(
                        "{}: a routed port is too high for the local shift",
                        container.name
                    )
                })?;
        } else {
            table.add(&container.service, addr);
        }
        table.add(&container.name, addr);
        let extra = volume_env(container, &volumes);
        launches.push(Launch {
            container: launched,
            addr,
            argv,
            standin: standin.cloned(),
            extra,
        });
    }
    let server = resolver::serve(table).map_err(|e| format!("resolver: {e}"))?;
    let resolver_addr = server.addr();
    let placed = launches
        .iter()
        .map(|l| {
            (
                l.container.name.clone(),
                l.container.service.clone(),
                l.addr,
                l.container.ports.clone(),
            )
        })
        .collect();
    let mut running = Running {
        server,
        children: Vec::new(),
        placed,
        started: Vec::new(),
        scratch,
        volumes,
        logs_dir,
        launches,
        proxies: Vec::new(),
        pids: BTreeMap::new(),
        resolver: resolver_addr,
        otlp,
    };
    // The fault proxies first: a routed dependency's proxy listens before any service starts.
    for service in &routed {
        let first = plan
            .containers
            .iter()
            .position(|c| &c.service == service && c.replica == 1)
            .ok_or_else(|| format!("{service} has no replica to proxy"))?;
        let container = &plan.containers[first];
        if container.ports.is_empty() {
            return Err(format!("{service} is faulted and has no port to proxy"));
        }
        let upstream_ip = running.launches[first].addr;
        let binary = observed.fault_binary.as_deref().unwrap_or(Path::new(""));
        for port in &container.ports {
            let control = free_port()?;
            let listen = SocketAddr::new(IpAddr::V4(proxy_ip), *port);
            let upstream = SocketAddr::new(
                IpAddr::V4(upstream_ip),
                port.checked_add(shift).unwrap_or(*port),
            );
            let child = spawn_proxy(binary, listen, upstream, control)?;
            running.proxies.push(RunningProxy {
                service: service.clone(),
                control: Remote::new(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), control)),
                child,
            });
        }
    }
    let deadline = Instant::now() + DEPENDENCY_WAIT;
    for proxy in &running.proxies {
        let control = proxy.control.control();
        wait_ports(
            &proxy.service,
            Ipv4Addr::LOCALHOST,
            &[control.port()],
            deadline,
        )
        .map_err(|why| format!("the fault proxy in front of {}: {why}", proxy.service))?;
    }
    for index in start_order(plan)? {
        let name = running.launches[index].container.name.clone();
        let container = running.launches[index].container.clone();
        wait_for_dependencies(&running, &container)?;
        let launch = running.launches[index].clone();
        let output = Output {
            logs: &running.logs_dir,
            otlp: running.otlp.as_deref(),
        };
        let child = spawn(
            &launch.container,
            launch.addr,
            &launch.argv,
            launch.standin.as_ref(),
            resolver_addr,
            &launch.extra,
            &output,
        )?;
        let pid_file = running.pid_path(&name)?;
        std::fs::write(&pid_file, child.id().to_string())
            .map_err(|e| format!("{}: {e}", pid_file.display()))?;
        running.pids.insert(name.clone(), pid_file);
        running.children.push((name.clone(), child));
        running.started.push(name);
    }
    Ok(running)
}

/// Starts `apz-fault proxy <listen> <upstream> <control>`: a fault proxy for one port, its control on loopback.
fn spawn_proxy(
    binary: &Path,
    listen: SocketAddr,
    upstream: SocketAddr,
    control: u16,
) -> Result<Child, String> {
    Command::new(binary)
        .arg("proxy")
        .arg(listen.to_string())
        .arg(upstream.to_string())
        .arg(format!("127.0.0.1:{control}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{}: cannot start the fault proxy: {e}", binary.display()))
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

/// A new scratch root for a run: unique in this process, made now.
fn scratch_root() -> Result<PathBuf, String> {
    let root = std::env::temp_dir().join(format!(
        "apz-topo-{}-{}",
        std::process::id(),
        RUNS.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).map_err(|e| format!("scratch {}: {e}", root.display()))?;
    Ok(root)
}

/// Makes the directory of each named volume the plan mounts, under the scratch root.
fn volume_dirs(plan: &GuestPlan, root: &Path) -> Result<BTreeMap<String, PathBuf>, String> {
    let names: BTreeSet<&str> = plan
        .containers
        .iter()
        .flat_map(|c| c.volumes.iter().map(|v| v.name.as_str()))
        .collect();
    let mut dirs = BTreeMap::new();
    for name in names {
        let dir = root.join("volumes").join(name);
        std::fs::create_dir_all(&dir).map_err(|e| format!("volume {name}: {e}"))?;
        dirs.insert(name.to_string(), dir);
    }
    Ok(dirs)
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

/// Where a container's output and OTLP go.
struct Output<'a> {
    logs: &'a Path,
    otlp: Option<&'a str>,
}

fn spawn(
    container: &Container,
    addr: Ipv4Addr,
    argv: &[String],
    standin: Option<&Standin>,
    resolver_addr: SocketAddr,
    extra: &[(String, String)],
    output: &Output<'_>,
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
        .env("OTEL_SERVICE_NAME", &container.name)
        .envs(extra.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::null());
    if let Some(endpoint) = output.otlp {
        // The services export JSON to the collector, which reads JSON (observe.rs).
        command
            .env("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint)
            .env("OTEL_EXPORTER_OTLP_PROTOCOL", "http/json");
    }
    if let Some(standin) = standin {
        command.envs(&standin.env);
    }
    // The container's output goes to its log file, which the collector tails (`<container>.log`).
    let log = output.logs.join(format!("{}.log", container.name));
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .map_err(|e| format!("{}: its log {}: {e}", container.name, log.display()))?;
    let errors = file
        .try_clone()
        .map_err(|e| format!("{}: its log: {e}", container.name))?;
    command
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(errors));
    command
        .spawn()
        .map_err(|e| format!("{}: cannot start {program}: {e}", container.name))
}

impl Running {
    /// The address of the runner's resolver.
    pub fn resolver(&self) -> SocketAddr {
        self.server.addr()
    }

    /// The base URL of each service the probes reach: its first replica's first port.
    pub fn urls(&self) -> BTreeMap<String, String> {
        let mut urls = BTreeMap::new();
        for launch in &self.launches {
            if launch.container.replica != 1 {
                continue;
            }
            if let Some(port) = launch.container.ports.first() {
                urls.entry(launch.container.service.clone())
                    .or_insert_with(|| format!("http://{}:{port}", launch.addr));
            }
        }
        urls
    }

    /// Where a container's pid file is kept while the run lasts.
    fn pid_path(&self, container: &str) -> Result<PathBuf, String> {
        Ok(self.scratch.join("pids").join(format!("{container}.pid")))
    }

    /// Starts a container again, the way it was started: its old process is ended, its new one's pid goes to its pid
    /// file, and its ports accept connections again before this returns.
    fn restart_container(&mut self, container: &str) -> Result<(), String> {
        let index = self
            .launches
            .iter()
            .position(|l| l.container.name == container)
            .ok_or_else(|| format!("the run has no process {container}"))?;
        if let Some((_, old)) = self.children.iter_mut().find(|(name, _)| name == container) {
            let _ = old.kill();
            let _ = old.wait();
        }
        let launch = self.launches[index].clone();
        let output = Output {
            logs: &self.logs_dir,
            otlp: self.otlp.as_deref(),
        };
        let child = spawn(
            &launch.container,
            launch.addr,
            &launch.argv,
            launch.standin.as_ref(),
            self.resolver,
            &launch.extra,
            &output,
        )?;
        let pid_file = self.pid_path(container)?;
        std::fs::write(&pid_file, child.id().to_string())
            .map_err(|e| format!("{}: {e}", pid_file.display()))?;
        match self.children.iter_mut().find(|(name, _)| name == container) {
            Some((_, slot)) => *slot = child,
            None => self.children.push((container.to_string(), child)),
        }
        wait_ports(
            container,
            launch.addr,
            &launch.container.ports,
            Instant::now() + DEPENDENCY_WAIT,
        )
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

    /// The directory the containers' output goes to: `<container>.log` for each container, while the run lasts.
    pub fn log_dir(&self) -> &Path {
        &self.logs_dir
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

impl Processes for Running {
    fn has_proxy(&self, service: &str) -> bool {
        self.proxies.iter().any(|p| p.service == service)
    }

    fn steer(&self, service: &str, fault: Fault) -> Result<(), String> {
        let mut steered = false;
        for proxy in self.proxies.iter().filter(|p| p.service == service) {
            steered = true;
            crate::fault::Steer::steer(&proxy.control, fault)?;
        }
        if steered {
            Ok(())
        } else {
            Err(format!("no fault proxy in front of {service}"))
        }
    }

    fn pid_file(&self, service: &str) -> Option<PathBuf> {
        self.pids.get(&format!("{service}-1")).cloned()
    }

    fn restart(&mut self, service: &str) -> Result<(), String> {
        self.restart_container(&format!("{service}-1"))
    }

    fn volume(&self, service: &str, volume: &str) -> Option<(PathBuf, u64)> {
        let launch = self
            .launches
            .iter()
            .find(|l| l.container.service == service && l.container.replica == 1)?;
        let declared = launch
            .container
            .volumes
            .iter()
            .find(|v| v.name == volume)?
            .size_mib?;
        let dir = self.volumes.get(volume)?.clone();
        Some((dir, declared))
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        for proxy in &mut self.proxies {
            let _ = proxy.child.kill();
            let _ = proxy.child.wait();
        }
        for (_, child) in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}
