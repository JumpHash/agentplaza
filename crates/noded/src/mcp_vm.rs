//! MCP servers in microVMs: the host side of the `plaza.mcp=1` guest, whose bridge (`mcp-bridge`, crates/vsockd)
//! carries one MCP server's stdio between its VM and the hub.
//!
//! One run is one VM. Before the VM boots, the run binds three listeners beside the VM's own vsock socket (the
//! muxer dials `<uds_dir>/vm<cid>_<port>` when the guest connects to a port), and two sockets for the hub:
//!
//! - port 5200 is the guest's stdio. The first guest connection gets the launch spec as one JSON line, then the
//!   hub's `mcp-<id>.stdio` connection is copied both ways with it. When the hub closes that connection, the VM stops.
//! - port 5201 is the guest's stderr, copied one way to the hub's `mcp-<id>.stderr` connection.
//! - port 5202 is the guest's only route out. Each guest connection is dialled to `127.0.0.1:<egress_port>`, the
//!   hub's egress proxy, and nothing else is ever dialled. The VM has no network device.
//!
//! The relays are threads of their own (std only). A connection that arrives before the hub's is held until the
//! hub connects, and everything the run holds is closed when the run is deleted.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use apz_http::Response;
use apz_uds::{UnixListener, UnixStream};
use serde::{Deserialize, Serialize};
use serde_json::json;
use vmm::clonefile::clone_file;
use vmm::spawner::{
    self, DataVolume, MAX_GUEST_CID, VmExit, VmHandle, VmSpec, VsockConfig, check_volume_name,
    check_volumes,
};

use crate::registry::{Registry, VmImages};
use crate::snapshots::private_dir;
use crate::volumes::VolumeError;

/// The first guest CID an MCP VM may take: above the run range and the role fleet's CIDs, so an MCP VM and a
/// session VM never share one. [`Registry::cid_in_use`] is still checked for each candidate.
const FIRST_CID: u64 = 1 << 31;
/// The guest's vsock ports (see the module docs).
const STDIO_PORT: u32 = 5200;
const STDERR_PORT: u32 = 5201;
const PROXY_PORT: u32 = 5202;
/// An MCP VM's vCPUs and memory: an MCP server is small.
const VCPUS: u8 = 2;
const MEMORY_MIB: usize = 1024;
/// The longest run id or server name.
const MAX_NAME: usize = 64;
/// How often a wait for a connection looks at the run's stop flag again.
const ACCEPT_POLL: Duration = Duration::from_millis(25);
/// The copy buffer of each relay direction.
const BUF_BYTES: usize = 16 * 1024;

/// A refused request: the HTTP status and the reason.
type Refused = (u16, String);

/// What a run boots its VM with: this host's check, and the boot itself. The node's is [`Kvm`]; tests use a fake.
pub trait Boot: Send + Sync {
    /// Whether this host can boot a VM at all: the first failed check, in words.
    fn host(&self) -> Result<(), String>;
    /// Boots the VM that `spec` describes.
    fn boot(&self, spec: VmSpec) -> Result<Box<dyn Machine>, String>;
}

/// A booted VM, as a run holds it.
pub trait Machine: Send {
    /// How the VM stopped, or `None` while it runs. It does not block.
    fn exit(&mut self) -> Result<Option<VmExit>, String>;
    /// Stops the VM and waits until it has stopped.
    fn stop(&mut self) -> Result<(), String>;
}

impl Machine for VmHandle {
    fn exit(&mut self) -> Result<Option<VmExit>, String> {
        self.wait_timeout(Duration::ZERO)
            .map_err(|err| err.to_string())
    }

    fn stop(&mut self) -> Result<(), String> {
        self.kill().map_err(|err| err.to_string())
    }
}

/// The node's boot: a VM through the spawner, on the KVM backend.
struct Kvm;

impl Boot for Kvm {
    fn host(&self) -> Result<(), String> {
        let backend = crate::machine_backend();
        if backend != "kvm" {
            return Err(format!("the VMM backend is {backend}, not kvm"));
        }
        if !Path::new("/dev/kvm").exists() {
            return Err("no /dev/kvm on this host".to_string());
        }
        Ok(())
    }

    fn boot(&self, spec: VmSpec) -> Result<Box<dyn Machine>, String> {
        let vm = crate::registry::spawn_timed(spec).map_err(|err| err.to_string())?;
        let machine: Box<dyn Machine> = Box::new(vm);
        Ok(machine)
    }
}

/// The MCP VMs of this node: the runs it has booted, and the boot they are booted with.
pub struct McpVms {
    boot: Box<dyn Boot>,
    /// Held for the whole of a create or a storage delete, so two of them never race for an id, a CID or a socket.
    busy: Mutex<()>,
    runs: Mutex<BTreeMap<String, Arc<McpRun>>>,
}

impl McpVms {
    /// The node's MCP VMs, booted on KVM.
    pub fn kvm() -> Self {
        Self::new(Box::new(Kvm))
    }

    /// MCP VMs booted by `boot`.
    pub fn new(boot: Box<dyn Boot>) -> Self {
        Self {
            boot,
            busy: Mutex::new(()),
            runs: Mutex::new(BTreeMap::new()),
        }
    }
}

/// The `POST /v1/mcp-runs` body.
#[derive(Debug, Deserialize)]
struct McpRunRequest {
    /// The run's id: its VM's name and the prefix of its hub sockets.
    id: String,
    /// The MCP server the run serves. A storage server keeps its workspace under its name.
    server: String,
    /// The server's program in the guest, absolute (`/data/<volume>/...`).
    program: PathBuf,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    /// The server's working directory in the guest, absolute.
    cwd: PathBuf,
    /// Whether the workspace is kept across the server's runs.
    #[serde(default)]
    storage: bool,
    /// The hub's egress proxy port on 127.0.0.1: the only place the VM reaches.
    egress_port: u16,
    #[serde(default)]
    mounts: Vec<Mount>,
}

/// A shared data volume the VM mounts read-only at `/data/<name>`.
#[derive(Debug, Deserialize)]
struct Mount {
    name: String,
    /// The absolute host path the volume is made from (see [`crate::volumes::VolumeStore::create`]).
    source: PathBuf,
}

/// The launch spec the guest bridge reads first: the server's program, arguments, environment and working directory.
#[derive(Serialize)]
struct Launch<'a> {
    program: &'a Path,
    args: &'a [String],
    env: &'a BTreeMap<String, String>,
    cwd: &'a Path,
}

/// The launch spec line for `request`: the JSON of [`Launch`] and a newline.
fn launch_line(request: &McpRunRequest) -> Result<Vec<u8>, String> {
    let mut line = serde_json::to_vec(&Launch {
        program: request.program.as_path(),
        args: &request.args,
        env: &request.env,
        cwd: request.cwd.as_path(),
    })
    .map_err(|err| format!("cannot encode the launch spec: {err}"))?;
    line.push(b'\n');
    Ok(line)
}

/// Whether `name` names a run or a server: 1 to 64 characters of `a-z`, `0-9` and `-`.
fn checked_name(name: &str) -> Result<(), String> {
    let valid = !name.is_empty()
        && name.len() <= MAX_NAME
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "a name is 1 to {MAX_NAME} characters of a-z, 0-9 and '-': {name:?}"
        ))
    }
}

/// Why a create request is malformed, if it is.
fn checked(request: &McpRunRequest) -> Result<(), String> {
    checked_name(&request.id)?;
    checked_name(&request.server)?;
    if !request.program.is_absolute() {
        return Err(format!(
            "the program is an absolute path: {}",
            request.program.display()
        ));
    }
    if !request.cwd.is_absolute() {
        return Err(format!(
            "the working directory is an absolute path: {}",
            request.cwd.display()
        ));
    }
    if request.egress_port == 0 {
        return Err("the egress port is 1 to 65535".to_string());
    }
    for mount in &request.mounts {
        check_volume_name(&mount.name)?;
        if !mount.source.is_absolute() {
            return Err(format!(
                "a mount's source is an absolute path: {}",
                mount.source.display()
            ));
        }
    }
    let shape: Vec<DataVolume> = request
        .mounts
        .iter()
        .map(|mount| DataVolume {
            name: mount.name.clone(),
            image: PathBuf::new(),
        })
        .collect();
    check_volumes(&shape)
}

/// Whether this node can boot MCP VMs: the first failed check, in words, or `Ok` when it can.
pub fn availability(registry: &Registry, boot: &dyn Boot) -> Result<(), String> {
    boot.host()?;
    let images: &VmImages = registry.images();
    if !images.kernel.is_file() {
        return Err(format!("no guest kernel at {}", images.kernel.display()));
    }
    if !images.initrd.is_file() {
        return Err(format!("no guest initrd at {}", images.initrd.display()));
    }
    match &images.workspace {
        None => Err("no workspace base image is configured".to_string()),
        Some(base) if !base.is_file() => {
            Err(format!("no workspace base image at {}", base.display()))
        }
        Some(_) => Ok(()),
    }
}

/// `GET /v1/mcp-vm` → 200 `{"available": bool, "reason": "ready"}`, or the first failed check.
pub fn availability_answer(registry: &Registry, vms: &McpVms) -> Response {
    let (available, reason) = match availability(registry, &*vms.boot) {
        Ok(()) => (true, "ready".to_string()),
        Err(why) => (false, why),
    };
    Response::json(200, &json!({"available": available, "reason": reason}))
}

/// The spec of an MCP VM: the node's kernel and initrd, the guest bridge as init's only process (`plaza.mcp=1`), the
/// vsock device the bridge reaches the host on, the workspace and the mounts. It has no balloon and no restore, and
/// no network device: a VM's only route out is its vsock port 5202.
fn vm_spec(
    images: &VmImages,
    uds: &Path,
    cid: u64,
    workspace: PathBuf,
    volumes: Vec<DataVolume>,
) -> VmSpec {
    VmSpec {
        kernel: images.kernel.clone(),
        cmdline: format!("{} plaza.mcp=1", crate::registry::CONSOLE),
        vcpus: VCPUS,
        memory_mib: MEMORY_MIB,
        initrd: Some(images.initrd.clone()),
        gic: None,
        vsock: Some(VsockConfig {
            guest_cid: cid,
            uds_path: uds.join(format!("vm{cid}")),
        }),
        workspace: Some(workspace),
        balloon: None,
        restore: None,
        volumes,
    }
}

/// The address a guest connection on port 5202 is dialled to: the hub's egress proxy on loopback, and nothing else.
fn egress_addr(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// The files a run being set up owns: its sockets, and a per-run workspace copy. They are removed when the set-up is
/// refused, so a refused run leaves nothing behind. [`Files::keep`] hands them to the run.
#[derive(Default)]
struct Files(Vec<PathBuf>);

impl Files {
    /// Binds a nonblocking listener at `path`, after removing a socket that a stopped daemon left there.
    fn listen(&mut self, path: PathBuf) -> Result<UnixListener, Refused> {
        let _ = fs::remove_file(&path);
        self.0.push(path.clone());
        let refuse = |err: io::Error| (503, format!("cannot listen on {}: {err}", path.display()));
        let listener = UnixListener::bind(&path).map_err(refuse)?;
        listener.set_nonblocking(true).map_err(refuse)?;
        Ok(listener)
    }

    /// The paths the run now owns.
    fn keep(mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.0)
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

/// The mounts' volumes. A volume that already exists is used as it is; a missing one is made from its source.
fn mount_volumes(registry: &Registry, mounts: &[Mount]) -> Result<Vec<DataVolume>, Refused> {
    if mounts.is_empty() {
        return Ok(Vec::new());
    }
    let store = registry
        .volume_store()
        .ok_or((409, "this node keeps no volumes".to_string()))?;
    let mut volumes = Vec::with_capacity(mounts.len());
    for mount in mounts {
        match store.create(&mount.name, &mount.source) {
            Ok(_) | Err(VolumeError::Exists(_)) => {}
            Err(VolumeError::Invalid(why)) => return Err((400, why)),
            Err(err) => return Err((503, err.to_string())),
        }
        let image = store
            .image(&mount.name)
            .ok_or_else(|| (503, format!("volume {} has no image", mount.name)))?;
        volumes.push(DataVolume {
            name: mount.name.clone(),
            image,
        });
    }
    Ok(volumes)
}

/// The first guest CID from [`FIRST_CID`] that no live run holds and no MCP VM has.
fn free_cid(registry: &Registry, vms: &McpVms) -> Result<u64, Refused> {
    let runs = lock(&vms.runs);
    let cid = (FIRST_CID..=MAX_GUEST_CID)
        .find(|cid| !registry.cid_in_use(*cid) && !runs.values().any(|run| run.cid == *cid));
    cid.ok_or((503, "no guest CID is free for an MCP VM".to_string()))
}

/// The workspace of a run's VM. A storage server's image lives under `<persist>/mcp/<server>/`: it is copied from the
/// base once and reused by every later run of the server. Otherwise the run gets a copy of the base of its own,
/// which `files` removes when the run ends.
fn workspace_image(
    registry: &Registry,
    request: &McpRunRequest,
    uds: &Path,
    files: &mut Files,
) -> Result<PathBuf, Refused> {
    let base = registry
        .images()
        .workspace
        .as_deref()
        .ok_or((503, "no workspace base image is configured".to_string()))?;
    if !request.storage {
        let copy = uds.join(format!("mcp-{}.workspace.ext4", request.id));
        files.0.push(copy.clone());
        let _ = fs::remove_file(&copy);
        clone_file(base, &copy)
            .map_err(|err| (503, format!("cannot copy the workspace image: {err}")))?;
        return Ok(copy);
    }
    let persist = registry
        .persist_dir()
        .ok_or((409, "this node keeps no persistent directory".to_string()))?;
    let dir = persist.join("mcp").join(&request.server);
    private_dir(&dir).map_err(|err| (503, format!("cannot create {}: {err}", dir.display())))?;
    let image = dir.join("workspace.ext4");
    if !image.exists() {
        let partial = dir.join("workspace.ext4.partial");
        let _ = fs::remove_file(&partial);
        spawner::copy_sparse(base, &partial)
            .and_then(|_| fs::rename(&partial, &image))
            .map_err(|err| (503, format!("cannot prepare the workspace image: {err}")))?;
    }
    Ok(image)
}

/// One run: a VM of one MCP server, and the relays between its guest and the hub.
struct McpRun {
    id: String,
    server: String,
    cid: u64,
    storage: bool,
    egress_port: u16,
    /// The launch spec line the guest bridge reads first.
    launch: Vec<u8>,
    /// The hub's stdio and stderr sockets.
    stdio: PathBuf,
    stderr: PathBuf,
    /// Every socket and per-run workspace copy the run owns; removed when the run is deleted.
    files: Vec<PathBuf>,
    /// Set when the run is deleted: the relays stop accepting, and their waits end.
    stop: Arc<AtomicBool>,
    machine: Mutex<Box<dyn Machine>>,
    /// Clones of the stdio and stderr connections, shut down when the run is deleted so their relays end.
    links: Mutex<Vec<Link>>,
}

/// The hub's listeners for one run: it connects to each of them once.
struct Hub {
    stdio: UnixListener,
    stderr: UnixListener,
}

/// A connection a relay copies through: the guest's Unix stream, or the TCP stream to the egress proxy.
enum Link {
    Unix(UnixStream),
    Tcp(TcpStream),
}

impl Link {
    fn try_clone(&self) -> io::Result<Link> {
        match self {
            Link::Unix(stream) => stream.try_clone().map(Link::Unix),
            Link::Tcp(stream) => stream.try_clone().map(Link::Tcp),
        }
    }

    fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        match self {
            Link::Unix(stream) => stream.shutdown(how),
            Link::Tcp(stream) => stream.shutdown(how),
        }
    }
}

impl Read for Link {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Link::Unix(stream) => stream.read(buf),
            Link::Tcp(stream) => stream.read(buf),
        }
    }
}

impl Write for Link {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Link::Unix(stream) => stream.write(buf),
            Link::Tcp(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Link::Unix(stream) => stream.flush(),
            Link::Tcp(stream) => stream.flush(),
        }
    }
}

/// Copies `from` to `to` until `from` ends. Once a write fails the far end is gone: nothing more is written, but
/// `from` is still read to its end, so whoever writes into it never blocks on a pipe nobody drains.
fn pump<R: Read + ?Sized, W: Write + ?Sized>(from: &mut R, to: &mut W) {
    let mut buf = [0u8; BUF_BYTES];
    let mut writable = true;
    loop {
        match from.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if writable && to.write_all(&buf[..n]).is_err() {
                    writable = false;
                }
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    let _ = to.flush();
}

/// Copies `from` to `to` until `from` ends, then shuts `to`'s write side, so its peer reads end of stream.
fn copy_and_close(mut from: Link, mut to: Link) {
    pump(&mut from, &mut to);
    let _ = to.shutdown(Shutdown::Write);
}

/// Carries one guest connection to the egress proxy and back: one thread per direction.
fn tunnel(guest: Link, host: Link) {
    let (Ok(guest_in), Ok(host_in)) = (guest.try_clone(), host.try_clone()) else {
        return;
    };
    thread::spawn(move || copy_and_close(guest_in, host));
    copy_and_close(host_in, guest);
}

/// The next connection on `listener`, or `None` once `stop` is set. The listener is nonblocking, so the wait ends.
fn accept_next(listener: &UnixListener, stop: &AtomicBool) -> Option<UnixStream> {
    loop {
        if stop.load(Ordering::SeqCst) {
            return None;
        }
        match listener.accept() {
            Ok((conn, _)) => {
                // An accepted stream may inherit the listener's nonblocking mode, and the relays read it blocking.
                let _ = conn.set_nonblocking(false);
                return Some(conn);
            }
            Err(_) => thread::sleep(ACCEPT_POLL),
        }
    }
}

/// Accepts the guest's connections on `listener` until `stop` is set, each one handed to `relay` on a thread of its own.
fn accept_guest(
    listener: UnixListener,
    stop: Arc<AtomicBool>,
    relay: impl Fn(UnixStream) + Send + Sync + 'static,
) {
    let relay = Arc::new(relay);
    thread::spawn(move || {
        while let Some(conn) = accept_next(&listener, &stop) {
            let relay = Arc::clone(&relay);
            thread::spawn(move || (*relay)(conn));
        }
    });
}

/// Starts the three guest relays of a run.
fn serve(run: &Arc<McpRun>, hub: Hub, [stdio, stderr, proxy]: [UnixListener; 3]) {
    let hub = Arc::new(hub);
    let stop = Arc::clone(&run.stop);
    let (stdio_run, stdio_hub) = (Arc::clone(run), Arc::clone(&hub));
    accept_guest(stdio, Arc::clone(&stop), move |conn| {
        stdio_run.relay_stdio(Link::Unix(conn), &stdio_hub);
    });
    let (stderr_run, stderr_hub) = (Arc::clone(run), Arc::clone(&hub));
    accept_guest(stderr, Arc::clone(&stop), move |conn| {
        stderr_run.relay_stderr(Link::Unix(conn), &stderr_hub);
    });
    let proxy_run = Arc::clone(run);
    accept_guest(proxy, stop, move |conn| {
        proxy_run.relay_proxy(Link::Unix(conn));
    });
}

impl McpRun {
    /// A guest's stdio connection on port 5200: the launch spec first, then the hub's stdio connection both ways.
    fn relay_stdio(self: &Arc<Self>, mut guest: Link, hub: &Hub) {
        if guest.write_all(&self.launch).is_err() {
            return;
        }
        let Some(client) = accept_next(&hub.stdio, &self.stop) else {
            return;
        };
        let client = Link::Unix(client);
        self.hold([&guest, &client]);
        let (Ok(mut hub_in), Ok(mut guest_out)) = (client.try_clone(), guest.try_clone()) else {
            return;
        };
        let run = Arc::clone(self);
        thread::spawn(move || {
            pump(&mut hub_in, &mut guest_out);
            // The hub closed its end of the stdio connection, and the run ends with it.
            run.stop_vm();
        });
        copy_and_close(guest, client);
    }

    /// A guest's stderr connection on port 5201, copied one way to the hub's stderr connection.
    fn relay_stderr(&self, guest: Link, hub: &Hub) {
        let Some(client) = accept_next(&hub.stderr, &self.stop) else {
            return;
        };
        let client = Link::Unix(client);
        self.hold([&guest, &client]);
        copy_and_close(guest, client);
    }

    /// A guest's connection on port 5202, dialled to the egress proxy on loopback and nothing else.
    fn relay_proxy(&self, guest: Link) {
        if let Ok(host) = TcpStream::connect(egress_addr(self.egress_port)) {
            tunnel(guest, Link::Tcp(host));
        }
    }

    /// Keeps clones of two connections, so the run can shut them down when it is deleted.
    fn hold(&self, pair: [&Link; 2]) {
        let mut links = lock(&self.links);
        for link in pair {
            if let Ok(clone) = link.try_clone() {
                links.push(clone);
            }
        }
    }

    /// Stops the VM. A stop that fails is reported, and the run ends all the same.
    fn stop_vm(&self) {
        if let Err(why) = lock(&self.machine).stop() {
            eprintln!("noded: mcp run {}: the VM did not stop: {why}", self.id);
        }
    }

    /// Whether the VM runs, and when it has stopped, how: the run's `state` and `error`.
    fn state(&self) -> (&'static str, Option<String>) {
        match lock(&self.machine).exit() {
            Ok(None) => ("running", None),
            Ok(Some(_)) => ("exited", None),
            Err(why) => ("exited", Some(why)),
        }
    }

    /// Ends the run: its VM stops, its relays end, and its files go.
    fn close(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.stop_vm();
        for link in lock(&self.links).drain(..) {
            let _ = link.shutdown(Shutdown::Both);
        }
        for path in &self.files {
            let _ = fs::remove_file(path);
        }
    }
}

/// `POST /v1/mcp-runs` → 201 `{"id", "stdio", "stderr"}`: boots the run's VM and binds its sockets.
pub fn create_run(registry: &Registry, vms: &McpVms, body: &[u8]) -> Response {
    let request: McpRunRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(err) => return Response::error(400, &format!("invalid mcp run request: {err}")),
    };
    if let Err(why) = checked(&request) {
        return Response::error(400, &why);
    }
    let _busy = lock(&vms.busy);
    match start(registry, vms, &request) {
        Ok(run) => Response::json(
            201,
            &json!({"id": run.id, "stdio": run.stdio, "stderr": run.stderr}),
        ),
        Err((status, why)) => Response::error(status, &why),
    }
}

/// Makes a run: its sockets are bound and its VM booted, and the run is registered and its relays started.
fn start(
    registry: &Registry,
    vms: &McpVms,
    request: &McpRunRequest,
) -> Result<Arc<McpRun>, Refused> {
    let launch = launch_line(request).map_err(|why| (500, why))?;
    if lock(&vms.runs).contains_key(&request.id) {
        return Err((409, format!("mcp run {} exists", request.id)));
    }
    if request.storage {
        if registry.persist_dir().is_none() {
            return Err((
                409,
                "this node keeps no persistent directory for a server's storage".to_string(),
            ));
        }
        let busy = lock(&vms.runs)
            .values()
            .any(|run| run.storage && run.server == request.server);
        if busy {
            return Err((
                409,
                format!("a VM of server {} runs with its storage", request.server),
            ));
        }
    }
    availability(registry, &*vms.boot).map_err(|why| (503, why))?;
    let volumes = mount_volumes(registry, &request.mounts)?;
    let cid = free_cid(registry, vms)?;
    let uds = std::path::absolute(registry.uds_dir()).map_err(|err| {
        (
            503,
            format!("cannot make the socket directory absolute: {err}"),
        )
    })?;

    let mut files = Files::default();
    let guest_stdio = files.listen(uds.join(format!("vm{cid}_{STDIO_PORT}")))?;
    let guest_stderr = files.listen(uds.join(format!("vm{cid}_{STDERR_PORT}")))?;
    let guest_proxy = files.listen(uds.join(format!("vm{cid}_{PROXY_PORT}")))?;
    let stdio = uds.join(format!("mcp-{}.stdio", request.id));
    let stderr = uds.join(format!("mcp-{}.stderr", request.id));
    let hub = Hub {
        stdio: files.listen(stdio.clone())?,
        stderr: files.listen(stderr.clone())?,
    };
    let workspace = workspace_image(registry, request, &uds, &mut files)?;
    let spec = vm_spec(registry.images(), &uds, cid, workspace, volumes);
    let machine = vms
        .boot
        .boot(spec)
        .map_err(|why| (503, format!("cannot boot the VM: {why}")))?;

    let run = Arc::new(McpRun {
        id: request.id.clone(),
        server: request.server.clone(),
        cid,
        storage: request.storage,
        egress_port: request.egress_port,
        launch,
        stdio,
        stderr,
        files: files.keep(),
        stop: Arc::new(AtomicBool::new(false)),
        machine: Mutex::new(machine),
        links: Mutex::new(Vec::new()),
    });
    lock(&vms.runs).insert(request.id.clone(), Arc::clone(&run));
    serve(&run, hub, [guest_stdio, guest_stderr, guest_proxy]);
    Ok(run)
}

/// The run with that id, if there is one.
fn find(vms: &McpVms, id: &str) -> Option<Arc<McpRun>> {
    lock(&vms.runs).get(id).cloned()
}

/// `GET /v1/mcp-runs/{id}` → 200 `{"id", "state": "running" | "exited", "error"}`; 404 for an unknown run.
pub fn get_run(vms: &McpVms, id: &str) -> Response {
    let Some(run) = find(vms, id) else {
        return Response::error(404, &format!("no mcp run with id {id}"));
    };
    let (state, error) = run.state();
    Response::json(200, &json!({"id": run.id, "state": state, "error": error}))
}

/// `DELETE /v1/mcp-runs/{id}` → 200 `{"removed": true}`: stops the VM and removes the run's sockets; 404 unknown.
pub fn delete_run(vms: &McpVms, id: &str) -> Response {
    let removed = lock(&vms.runs).remove(id);
    match removed {
        Some(run) => {
            run.close();
            Response::json(200, &json!({"removed": true}))
        }
        None => Response::error(404, &format!("no mcp run with id {id}")),
    }
}

/// `DELETE /v1/mcp-storage/{server}` → 200 `{"removed": true}`: removes the server's kept workspace. 409 while a VM of
/// the server runs with it; 400 for a bad name.
pub fn delete_storage(registry: &Registry, vms: &McpVms, server: &str) -> Response {
    if let Err(why) = checked_name(server) {
        return Response::error(400, &why);
    }
    let _busy = lock(&vms.busy);
    let Some(persist) = registry.persist_dir() else {
        return Response::error(409, "this node keeps no persistent directory");
    };
    let busy = lock(&vms.runs)
        .values()
        .any(|run| run.storage && run.server == server);
    if busy {
        return Response::error(
            409,
            &format!("a VM of server {server} runs with its storage"),
        );
    }
    let dir = persist.join("mcp").join(server);
    match fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => {
            return Response::error(500, &format!("cannot remove {}: {err}", dir.display()));
        }
    }
    Response::json(200, &json!({"removed": true}))
}

/// A mutex's guard, past a poisoned lock: a panicked relay leaves the state it had, which is still the state.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_http::Body;
    use serde_json::Value;

    use crate::events::EventBus;

    /// How long a test waits for a connection or a state before it fails.
    const TIMEOUT: Duration = Duration::from_secs(10);

    /// What the fake VMs did: the specs they were booted from, whether they were stopped, and how they exited on
    /// their own (a guest that reboots).
    #[derive(Default)]
    struct Shared {
        specs: Mutex<Vec<VmSpec>>,
        stopped: AtomicBool,
        exited: Mutex<Option<VmExit>>,
    }

    /// A VM that runs until it is stopped, or until a test makes it exit.
    struct FakeMachine(Arc<Shared>);

    impl Machine for FakeMachine {
        fn exit(&mut self) -> Result<Option<VmExit>, String> {
            if self.0.stopped.load(Ordering::SeqCst) {
                return Ok(Some(VmExit::Killed));
            }
            Ok(*lock(&self.0.exited))
        }

        fn stop(&mut self) -> Result<(), String> {
            self.0.stopped.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    /// A boot that never touches the host: it records the spec, and fails when a test says so.
    struct FakeBoot {
        host: Result<(), String>,
        boot_error: Option<String>,
        shared: Arc<Shared>,
    }

    impl Boot for FakeBoot {
        fn host(&self) -> Result<(), String> {
            self.host.clone()
        }

        fn boot(&self, spec: VmSpec) -> Result<Box<dyn Machine>, String> {
            if let Some(why) = &self.boot_error {
                return Err(why.clone());
            }
            lock(&self.shared.specs).push(spec);
            let machine: Box<dyn Machine> = Box::new(FakeMachine(Arc::clone(&self.shared)));
            Ok(machine)
        }
    }

    /// A test node: a scratch directory with the guest images and a registry over it, with fake VMs.
    struct Rig {
        dir: PathBuf,
        registry: Registry,
        vms: McpVms,
        shared: Arc<Shared>,
    }

    impl Rig {
        fn new(tag: &str) -> Rig {
            Rig::with(tag, Ok(()), None)
        }

        fn with(tag: &str, host: Result<(), String>, boot_error: Option<String>) -> Rig {
            let dir = std::env::temp_dir().join(format!("mcpvm-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("uds")).unwrap();
            fs::write(dir.join("kernel"), b"kernel").unwrap();
            fs::write(dir.join("initrd"), b"initrd").unwrap();
            fs::write(dir.join("base.ext4"), b"BASE").unwrap();
            let images = VmImages {
                kernel: dir.join("kernel"),
                initrd: dir.join("initrd"),
                workspace: Some(dir.join("base.ext4")),
            };
            let registry = Registry::new(images, dir.join("uds"), EventBus::new())
                .with_persist_dir(Some(dir.join("persist")))
                .with_volumes(Some(
                    crate::volumes::VolumeStore::open(dir.join("volumes")).unwrap(),
                ));
            let shared = Arc::new(Shared::default());
            let boot = FakeBoot {
                host,
                boot_error,
                shared: Arc::clone(&shared),
            };
            Rig {
                dir,
                registry,
                vms: McpVms::new(Box::new(boot)),
                shared,
            }
        }

        fn uds(&self) -> PathBuf {
            self.dir.join("uds")
        }

        /// The guest CID of the run's VM, from the spec it was booted from.
        fn cid(&self) -> u64 {
            let specs = lock(&self.shared.specs);
            specs.last().unwrap().vsock.as_ref().unwrap().guest_cid
        }

        fn post(&self, body: &Value) -> Response {
            create_run(&self.registry, &self.vms, &body.to_string().into_bytes())
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    /// A valid create request for run `id` of `server`, with the given egress port.
    fn request(id: &str, server: &str, egress_port: u16) -> Value {
        json!({
            "id": id,
            "server": server,
            "program": "/data/tools/bin/server",
            "args": ["--stdio"],
            "env": {"HTTP_PROXY": "http://apz:token@127.0.0.1:3128"},
            "cwd": "/workspace",
            "storage": false,
            "egress_port": egress_port,
            "mounts": [],
        })
    }

    fn json_of(response: &Response) -> Value {
        match &response.body {
            Body::Bytes(bytes) => serde_json::from_slice(bytes).unwrap(),
            other => panic!("expected a JSON body, got {other:?}"),
        }
    }

    /// Reads one byte at a time up to a newline, so nothing after it is taken from the stream.
    fn read_line(stream: &mut impl Read) -> String {
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            stream.read_exact(&mut byte).unwrap();
            if byte[0] == b'\n' {
                break;
            }
            line.push(byte[0]);
        }
        String::from_utf8(line).unwrap()
    }

    /// Waits until `done` holds, or fails the test.
    fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
        let started = std::time::Instant::now();
        while !done() {
            assert!(started.elapsed() < TIMEOUT, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn guest_connect(rig: &Rig, port: u32) -> UnixStream {
        let guest = UnixStream::connect(rig.uds().join(format!("vm{}_{port}", rig.cid()))).unwrap();
        guest.set_read_timeout(Some(TIMEOUT)).unwrap();
        guest
    }

    fn hub_connect(path: &Value) -> UnixStream {
        let hub = UnixStream::connect(path.as_str().unwrap()).unwrap();
        hub.set_read_timeout(Some(TIMEOUT)).unwrap();
        hub
    }

    #[test]
    fn the_guest_boots_with_the_bridge_the_workspace_and_the_mounts() {
        let rig = Rig::new("vmspec");
        let source = rig.dir.join("tools.img");
        fs::write(&source, b"VOLUME").unwrap();
        let mut body = request("v-1", "srv", 3999);
        body["mounts"] = json!([{"name": "tools", "source": source}]);
        assert_eq!(rig.post(&body).status, 201);

        let specs = lock(&rig.shared.specs);
        assert_eq!(specs.len(), 1);
        let spec = &specs[0];
        assert_eq!(spec.kernel, rig.dir.join("kernel"));
        assert_eq!(spec.initrd, Some(rig.dir.join("initrd")));
        assert!(
            spec.cmdline
                .split_whitespace()
                .any(|word| word == "plaza.mcp=1")
        );
        assert!(!spec.cmdline.contains("plaza.harness"), "{}", spec.cmdline);
        assert_eq!(spec.vcpus, 2);
        assert_eq!(spec.memory_mib, 1024);
        assert_eq!(spec.balloon, None);
        assert_eq!(spec.restore, None);
        let vsock = spec.vsock.as_ref().unwrap();
        assert!(vsock.guest_cid >= FIRST_CID, "cid {}", vsock.guest_cid);
        assert_eq!(
            vsock.uds_path,
            rig.uds().join(format!("vm{}", vsock.guest_cid))
        );
        let workspace = spec.workspace.as_ref().unwrap();
        assert_eq!(fs::read(workspace).unwrap(), b"BASE");
        let store = rig.registry.volume_store().unwrap();
        assert_eq!(
            spec.volumes,
            vec![DataVolume {
                name: "tools".to_string(),
                image: store.image("tools").unwrap(),
            }]
        );
    }

    #[test]
    fn a_volume_that_exists_is_used_as_it_is() {
        let rig = Rig::new("volreuse");
        let first = rig.dir.join("first.img");
        fs::write(&first, b"FIRST").unwrap();
        let store = rig.registry.volume_store().unwrap();
        store.create("tools", &first).unwrap();
        let other = rig.dir.join("other.img");
        fs::write(&other, b"OTHER").unwrap();
        let mut body = request("vol-1", "srv", 3999);
        body["mounts"] = json!([{"name": "tools", "source": other}]);
        assert_eq!(rig.post(&body).status, 201);
        let specs = lock(&rig.shared.specs);
        assert_eq!(specs[0].volumes[0].image, store.image("tools").unwrap());
    }

    #[test]
    fn the_guest_gets_the_spec_first_then_the_hub_both_ways() {
        let rig = Rig::new("relay");
        let created = rig.post(&request("r-1", "srv", 3999));
        assert_eq!(created.status, 201);
        let answer = json_of(&created);
        assert_eq!(answer["id"], "r-1");
        assert!(answer["stdio"].as_str().unwrap().ends_with("mcp-r-1.stdio"));

        // The guest connects first, as its bridge does, and reads the spec line before anything else.
        let mut guest = guest_connect(&rig, STDIO_PORT);
        let spec: Value = serde_json::from_str(&read_line(&mut guest)).unwrap();
        assert_eq!(spec["program"], "/data/tools/bin/server");
        assert_eq!(spec["args"], json!(["--stdio"]));
        assert_eq!(spec["env"]["HTTP_PROXY"], "http://apz:token@127.0.0.1:3128");
        assert_eq!(spec["cwd"], "/workspace");

        // The hub connects once, and the bytes go both ways.
        let mut hub = hub_connect(&answer["stdio"]);
        hub.write_all(b"from the hub\n").unwrap();
        let mut from_hub = vec![0u8; "from the hub\n".len()];
        guest.read_exact(&mut from_hub).unwrap();
        assert_eq!(from_hub, b"from the hub\n");
        guest.write_all(b"from the server\n").unwrap();
        let mut from_server = vec![0u8; "from the server\n".len()];
        hub.read_exact(&mut from_server).unwrap();
        assert_eq!(from_server, b"from the server\n");

        // The hub closing its end stops the VM.
        drop(hub);
        wait_for("the VM to stop", || {
            rig.shared.stopped.load(Ordering::SeqCst)
        });
        assert_eq!(delete_run(&rig.vms, "r-1").status, 200);
    }

    #[test]
    fn the_guests_stderr_reaches_the_hub_on_port_5201() {
        let rig = Rig::new("stderr");
        let created = json_of(&rig.post(&request("e-1", "srv", 3999)));
        let mut guest = guest_connect(&rig, STDERR_PORT);
        guest.write_all(b"boom\n").unwrap();
        drop(guest);
        // The hub connects after the guest has written: the bytes wait for it.
        let mut hub = hub_connect(&created["stderr"]);
        let mut out = String::new();
        hub.read_to_string(&mut out).unwrap();
        assert_eq!(out, "boom\n");
        assert_eq!(delete_run(&rig.vms, "e-1").status, 200);
    }

    #[test]
    fn the_proxy_port_dials_the_egress_port_and_nothing_else() {
        let egress = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = egress.local_addr().unwrap().port();
        let rig = Rig::new("proxy");
        assert_eq!(rig.post(&request("p-1", "srv", port)).status, 201);

        // The egress proxy reads the request to its end, then answers and closes.
        let proxy = thread::spawn(move || {
            let (mut conn, _) = egress.accept().unwrap();
            let mut got = Vec::new();
            conn.read_to_end(&mut got).unwrap();
            conn.write_all(b"pong").unwrap();
            got
        });
        let mut guest = guest_connect(&rig, PROXY_PORT);
        guest.write_all(b"ping").unwrap();
        guest.shutdown(Shutdown::Write).unwrap();
        let mut reply = Vec::new();
        guest.read_to_end(&mut reply).unwrap();
        assert_eq!(reply, b"pong");
        assert_eq!(proxy.join().unwrap(), b"ping");
        assert_eq!(egress_addr(port), SocketAddr::from(([127, 0, 0, 1], port)));
        assert_eq!(delete_run(&rig.vms, "p-1").status, 200);
    }

    #[test]
    fn a_run_is_posted_read_and_deleted_with_its_files() {
        let rig = Rig::new("life");
        let stale = rig.uds().join("mcp-l-1.stdio");
        // A socket path a stopped daemon left behind does not stop the run.
        fs::write(&stale, b"").unwrap();
        let created = json_of(&rig.post(&request("l-1", "srv", 3999)));
        let stdio = PathBuf::from(created["stdio"].as_str().unwrap());
        let copy = rig.uds().join("mcp-l-1.workspace.ext4");
        assert!(stdio.exists() && copy.exists());

        let got = get_run(&rig.vms, "l-1");
        assert_eq!(got.status, 200);
        assert_eq!(
            json_of(&got),
            json!({"id": "l-1", "state": "running", "error": null})
        );

        assert_eq!(delete_run(&rig.vms, "l-1").status, 200);
        assert!(rig.shared.stopped.load(Ordering::SeqCst));
        assert!(!stdio.exists() && !copy.exists());
        assert_eq!(get_run(&rig.vms, "l-1").status, 404);
        assert_eq!(delete_run(&rig.vms, "l-1").status, 404);
    }

    #[test]
    fn a_guest_that_powers_off_reports_exited() {
        let rig = Rig::new("exited");
        assert_eq!(rig.post(&request("x-1", "srv", 3999)).status, 201);
        *lock(&rig.shared.exited) = Some(VmExit::GuestExited { reset: true });
        let got = json_of(&get_run(&rig.vms, "x-1"));
        assert_eq!(got["state"], "exited");
        assert_eq!(got["error"], Value::Null);
        assert_eq!(delete_run(&rig.vms, "x-1").status, 200);
    }

    #[test]
    fn a_duplicate_id_is_a_conflict_and_bad_names_are_refused() {
        let rig = Rig::new("names");
        assert_eq!(rig.post(&request("d-1", "srv", 3999)).status, 201);
        assert_eq!(rig.post(&request("d-1", "srv", 3999)).status, 409);

        let long = "x".repeat(MAX_NAME + 1);
        let mut cases = vec![
            request("Bad_1", "srv", 3999),
            request("", "srv", 3999),
            request(&long, "srv", 3999),
            request("d-2", "UP", 3999),
            request("d-2", "", 3999),
            request("d-2", "srv", 0),
        ];
        let mut relative = request("d-2", "srv", 3999);
        relative["program"] = json!("bin/server");
        cases.push(relative);
        let mut cwd = request("d-2", "srv", 3999);
        cwd["cwd"] = json!("workspace");
        cases.push(cwd);
        let mut name = request("d-2", "srv", 3999);
        name["mounts"] = json!([{"name": "-bad", "source": "/tmp/x"}]);
        cases.push(name);
        let mut source = request("d-2", "srv", 3999);
        source["mounts"] = json!([{"name": "tools", "source": "relative.img"}]);
        cases.push(source);
        let mut twice = request("d-2", "srv", 3999);
        twice["mounts"] = json!([
            {"name": "tools", "source": "/tmp/a"},
            {"name": "tools", "source": "/tmp/b"}
        ]);
        cases.push(twice);
        for case in &cases {
            let refused = rig.post(case);
            assert_eq!(refused.status, 400, "accepted {case}");
        }
        assert_eq!(
            lock(&rig.shared.specs).len(),
            1,
            "a refused run booted a VM"
        );
        assert_eq!(delete_run(&rig.vms, "d-1").status, 200);
    }

    #[test]
    fn a_storage_workspace_is_kept_and_reused_until_its_storage_is_deleted() {
        let rig = Rig::new("stor");
        let mut first = request("s-1", "db", 3999);
        first["storage"] = json!(true);
        assert_eq!(rig.post(&first).status, 201);
        let image = rig.dir.join("persist/mcp/db/workspace.ext4");
        assert_eq!(fs::read(&image).unwrap(), b"BASE");
        fs::write(&image, b"KEPT").unwrap();

        // A second run of the server while the first is live is refused: one image, one VM.
        let mut second = request("s-2", "db", 3999);
        second["storage"] = json!(true);
        assert_eq!(rig.post(&second).status, 409);

        assert_eq!(delete_run(&rig.vms, "s-1").status, 200);
        assert_eq!(fs::read(&image).unwrap(), b"KEPT");
        assert_eq!(rig.post(&second).status, 201);
        assert_eq!(
            fs::read(&image).unwrap(),
            b"KEPT",
            "the image was copied again"
        );
        assert_eq!(delete_run(&rig.vms, "s-2").status, 200);

        // Deleting the storage while a VM runs with it is refused; once none does, the server's directory goes.
        assert_eq!(rig.post(&second).status, 201);
        assert_eq!(delete_storage(&rig.registry, &rig.vms, "db").status, 409);
        assert_eq!(delete_run(&rig.vms, "s-2").status, 200);
        assert_eq!(delete_storage(&rig.registry, &rig.vms, "db").status, 200);
        assert!(!rig.dir.join("persist/mcp/db").exists());
        assert_eq!(delete_storage(&rig.registry, &rig.vms, "Bad").status, 400);
    }

    #[test]
    fn a_vm_that_cannot_boot_is_refused_and_leaves_no_sockets() {
        let rig = Rig::with("boot", Ok(()), Some("boot refused".to_string()));
        let refused = rig.post(&request("b-1", "srv", 3999));
        assert_eq!(refused.status, 503);
        assert!(
            json_of(&refused)["error"]
                .as_str()
                .unwrap()
                .contains("boot refused")
        );
        assert_eq!(fs::read_dir(rig.uds()).unwrap().count(), 0);
        assert_eq!(get_run(&rig.vms, "b-1").status, 404);
    }

    #[test]
    fn a_host_without_kvm_refuses_runs_and_says_why() {
        let rig = Rig::with("nokvm", Err("no /dev/kvm on this host".to_string()), None);
        let answer = json_of(&availability_answer(&rig.registry, &rig.vms));
        assert_eq!(
            answer,
            json!({"available": false, "reason": "no /dev/kvm on this host"})
        );
        let refused = rig.post(&request("k-1", "srv", 3999));
        assert_eq!(refused.status, 503);
        assert!(
            lock(&rig.shared.specs).is_empty(),
            "a VM booted on a host without KVM"
        );
    }

    #[test]
    fn availability_names_the_first_failed_check() {
        let rig = Rig::new("avail");
        assert!(availability(&rig.registry, &*rig.vms.boot).is_ok());
        let answer = json_of(&availability_answer(&rig.registry, &rig.vms));
        assert_eq!(answer, json!({"available": true, "reason": "ready"}));

        fs::remove_file(rig.dir.join("kernel")).unwrap();
        assert!(
            availability(&rig.registry, &*rig.vms.boot)
                .unwrap_err()
                .contains("no guest kernel")
        );
        fs::write(rig.dir.join("kernel"), b"kernel").unwrap();
        fs::remove_file(rig.dir.join("initrd")).unwrap();
        assert!(
            availability(&rig.registry, &*rig.vms.boot)
                .unwrap_err()
                .contains("no guest initrd")
        );
        fs::write(rig.dir.join("initrd"), b"initrd").unwrap();
        fs::remove_file(rig.dir.join("base.ext4")).unwrap();
        assert!(
            availability(&rig.registry, &*rig.vms.boot)
                .unwrap_err()
                .contains("no workspace base")
        );
    }

    #[test]
    fn this_host_reports_its_first_failed_check_for_get_mcp_vm() {
        if crate::machine_backend() == "kvm" && Path::new("/dev/kvm").exists() {
            eprintln!("skipped: this host has KVM, so the host check passes");
            return;
        }
        // The images are the test's own and present, so the host check is the first one that can fail.
        let rig = Rig::new("here");
        let answer = json_of(&availability_answer(&rig.registry, &McpVms::kvm()));
        assert_eq!(answer["available"], false, "{answer}");
        let reason = answer["reason"].as_str().unwrap();
        if crate::machine_backend() == "kvm" {
            assert!(reason.contains("/dev/kvm"), "{reason}");
        } else {
            assert!(reason.contains("not kvm"), "{reason}");
        }
    }

    #[test]
    fn the_node_routes_the_mcp_api() {
        let routes: Vec<(String, String)> = crate::router()
            .routes()
            .map(|route| (route.method.to_string(), route.pattern.to_string()))
            .collect();
        for (method, pattern) in [
            ("GET", "/v1/mcp-vm"),
            ("POST", "/v1/mcp-runs"),
            ("GET", "/v1/mcp-runs/{id}"),
            ("DELETE", "/v1/mcp-runs/{id}"),
            ("DELETE", "/v1/mcp-storage/{server}"),
        ] {
            assert!(
                routes.iter().any(|(m, p)| m == method && p == pattern),
                "{method} {pattern} is not routed"
            );
        }
    }

    #[test]
    fn a_real_vm_answers_the_hub_on_stdio() {
        if crate::machine_backend() != "kvm" || !Path::new("/dev/kvm").exists() {
            eprintln!("skipped: no KVM device on this host, so no real VM boots");
            return;
        }
        let Some(guest) = std::env::var_os("APZ_MCP_TEST_GUEST_DIR").map(PathBuf::from) else {
            eprintln!(
                "skipped: APZ_MCP_TEST_GUEST_DIR names no guest directory (vmlinux, initrd.gz, workspace.ext4)"
            );
            return;
        };
        let (kernel, initrd, workspace) = (
            guest.join("vmlinux"),
            guest.join("initrd.gz"),
            guest.join("workspace.ext4"),
        );
        for file in [&kernel, &initrd, &workspace] {
            if !file.is_file() {
                eprintln!("skipped: no guest file at {}", file.display());
                return;
            }
        }
        let dir = std::env::temp_dir().join(format!("mcpvm-real-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("uds")).unwrap();
        let images = VmImages {
            kernel,
            initrd,
            workspace: Some(workspace),
        };
        let registry = Registry::new(images, dir.join("uds"), EventBus::new())
            .with_persist_dir(Some(dir.join("persist")));
        let vms = McpVms::kvm();
        let mut body = request("real-1", "srv", 3999);
        body["program"] = json!("/bin/sh");
        body["args"] = json!(["-c", "printf '{\"ready\":true}\\n'"]);
        let created = create_run(&registry, &vms, &body.to_string().into_bytes());
        assert_eq!(created.status, 201, "{}", json_of(&created));
        let mut hub = hub_connect(&json_of(&created)["stdio"]);
        let line = read_line(&mut hub);
        let answer: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(answer["ready"], true);
        assert_eq!(delete_run(&vms, "real-1").status, 200);
        let _ = fs::remove_dir_all(&dir);
    }
}
