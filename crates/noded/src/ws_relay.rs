//! The node's relay for a workstation VM's hub link (docs/design/workstations.md, section 10; rule 0010).
//!
//! A workstation VM has no NIC, so its daemon reaches its hub through its vsock device. The guest's connect to the host
//! on [`LINK_PORT`] arrives here, and each connection is forwarded, byte for byte, to the hub's TCP address: the one
//! [`Destination`] its VM was given. The relay never terminates TLS, never reads what the bytes say, and never takes an
//! address from the guest. The daemon's TLS runs end to end with the hub, pinned.
//!
//! Each backend's connections reach this relay by its own transport, and both serve the same join config:
//! - a plaza VM's vsock is the VMM's muxer, which names each guest connection `{dir}/vm{cid}_{port}`. [`Relay`] listens on
//!   those unix sockets, for that one VM, and lives as long as the VM.
//! - a QEMU VM's vsock is the kernel's. Its connections arrive on the host's `AF_VSOCK` listeners, which every QEMU
//!   workstation VM of the node shares: [`Host`] runs them while any QEMU VM has a route, and serves a connection only when
//!   its peer's CID belongs to a running QEMU VM of this node ([`admitted`]).
//!
//! [`CONFIG_PORT`] serves the VM's join config once: the first connection to it gets the config and every later one gets
//! nothing. The join code is held here until then, never written to disk and never on the kernel command line.
//!
//! At most [`MAX_CONNECTIONS`] connections are open per VM; a connection past that is closed at once.
//!
//! Stop is a flag polled between reads, not a `shutdown` that wakes a blocked read: on Windows a `shutdown` does not wake
//! a `recv` in another thread, so every read here has a short timeout.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use apz_station::vsock::{CONFIG_PORT, HOST_CID, LINK_PORT};
use apz_uds::{UnixListener, UnixStream};
#[cfg(target_os = "linux")]
use vmm::host_vsock::{Listener as VsockListener, Stream as VsockStream};

/// Connections open at once for one VM (the link, and the one-shot config).
pub const MAX_CONNECTIONS: usize = 4;
/// How long one read waits before it checks whether the relay is stopping.
const POLL: Duration = Duration::from_millis(100);
/// How long an accept loop sleeps when nothing is waiting.
const ACCEPT_POLL: Duration = Duration::from_millis(50);
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
const COPY_BUF: usize = 16 * 1024;
/// DIAGNOSTIC (branch diag/windows-relay only): when set, each relayed connection says how its directions ended.
static TRACE: AtomicBool = AtomicBool::new(false);

/// The one place a relay sends bytes: the hub's TCP address, read from its `http(s)://host[:port]` URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    host: String,
    port: u16,
}

impl Destination {
    /// The hub's TCP address. Only `http://` and `https://` with a host and an optional port are a destination: a vsock
    /// address names no TCP host, and a URL with a path is not an address the relay will dial.
    pub fn of_hub(hub: &str) -> Result<Self, String> {
        let (_, host, port) = apz_http::client::split_base(hub).map_err(|_| {
            format!("the hub {hub:?} is not an http or https address the relay can dial")
        })?;
        if host.is_empty() {
            return Err(format!("the hub {hub:?} names no host"));
        }
        Ok(Self { host, port })
    }

    /// The host the relay dials, as the hub URL names it.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The port the relay dials.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Connects to the hub. A hub on this machine is told first that the connection is a VM's
    /// ([`apz_http::RELAY_PREFACE`]), so it gives the VM none of the trust a caller on its own
    /// machine has: without it every byte the guest sends would arrive from loopback.
    fn dial(&self) -> io::Result<TcpStream> {
        let mut last = io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} has no address", self.host),
        );
        for addr in (self.host.as_str(), self.port).to_socket_addrs()? {
            match TcpStream::connect_timeout(&addr, DIAL_TIMEOUT) {
                Ok(mut stream) => {
                    if addr.ip().is_loopback() {
                        stream.write_all(apz_http::RELAY_PREFACE)?;
                    }
                    return Ok(stream);
                }
                Err(err) => last = err,
            }
        }
        Err(last)
    }
}

/// Whether a vsock peer may use a relay: its CID must be one of `running`, the CIDs of this node's running QEMU workstation
/// VMs. The host (CID [`HOST_CID`]) and the reserved CIDs below it are never a VM, and neither is the any-CID (`u32::MAX`,
/// `VMADDR_CID_ANY`), so those are refused whatever runs.
pub fn admitted(peer: u32, running: &[u32]) -> bool {
    peer > HOST_CID && peer != u32::MAX && running.contains(&peer)
}

/// The Unix socket a plaza VM's guest connection to `port` arrives on (`{dir}/vm{cid}_{port}`).
pub fn socket_path(dir: &Path, cid: u32, port: u32) -> PathBuf {
    dir.join(format!("vm{cid}_{port}"))
}

/// The guest's end of one relayed connection: a unix socket (a plaza VM) or an AF_VSOCK stream (a QEMU VM).
pub enum Guest {
    Unix(UnixStream),
    #[cfg(target_os = "linux")]
    Vsock(VsockStream),
}

impl Guest {
    fn dup(&self) -> io::Result<Self> {
        match self {
            Self::Unix(stream) => stream.try_clone().map(Self::Unix),
            #[cfg(target_os = "linux")]
            Self::Vsock(stream) => stream.try_clone().map(Self::Vsock),
        }
    }

    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Self::Unix(stream) => stream.set_read_timeout(timeout),
            #[cfg(target_os = "linux")]
            Self::Vsock(stream) => stream.set_read_timeout(timeout),
        }
    }

    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Self::Unix(stream) => stream.set_write_timeout(timeout),
            #[cfg(target_os = "linux")]
            Self::Vsock(stream) => stream.set_write_timeout(timeout),
        }
    }

    fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        match self {
            Self::Unix(stream) => stream.shutdown(how),
            #[cfg(target_os = "linux")]
            Self::Vsock(stream) => stream.shutdown(how),
        }
    }

    /// Puts an accepted unix stream in blocking mode: the reads and writes here use timeouts, not polling.
    fn make_blocking(&self) -> io::Result<()> {
        match self {
            Self::Unix(stream) => stream.set_nonblocking(false),
            #[cfg(target_os = "linux")]
            Self::Vsock(_) => Ok(()),
        }
    }
}

impl Read for Guest {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Unix(stream) => stream.read(buf),
            #[cfg(target_os = "linux")]
            Self::Vsock(stream) => stream.read(buf),
        }
    }
}

impl Write for Guest {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Unix(stream) => stream.write(buf),
            #[cfg(target_os = "linux")]
            Self::Vsock(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Unix(stream) => stream.flush(),
            #[cfg(target_os = "linux")]
            Self::Vsock(stream) => stream.flush(),
        }
    }
}

/// A listener the relay accepts guest connections on.
enum Listening {
    Unix(UnixListener),
    #[cfg(target_os = "linux")]
    Vsock(VsockListener),
}

impl Listening {
    /// The next waiting connection, with the CID of its guest when the transport names one. `None` when none is waiting.
    fn accept_one(&self) -> io::Result<Option<(Guest, Option<u32>)>> {
        match self {
            Self::Unix(listener) => match listener.accept() {
                Ok((stream, _)) => Ok(Some((Guest::Unix(stream), None))),
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => Ok(None),
                Err(err) => Err(err),
            },
            #[cfg(target_os = "linux")]
            Self::Vsock(listener) => Ok(listener
                .accept()?
                .map(|(stream, peer)| (Guest::Vsock(stream), Some(peer)))),
        }
    }
}

/// What a relayed connection needs: the hub it dials, the join config it may take once, its open connections, and whether
/// its VM is gone (its connections then end).
pub struct Route {
    destination: Destination,
    config: Mutex<Option<Vec<u8>>>,
    active: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl Route {
    fn new(destination: Destination, config: Option<Vec<u8>>) -> Arc<Self> {
        Arc::new(Self {
            destination,
            config: Mutex::new(config),
            active: Arc::new(AtomicUsize::new(0)),
            stop: Arc::new(AtomicBool::new(false)),
        })
    }
}

/// The route a connection belongs to, from its peer's CID when the transport names one. `None` refuses the connection.
type RouteFor = Box<dyn Fn(Option<u32>) -> Option<Arc<Route>> + Send>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Link,
    Config,
}

/// Holds one of a route's connection slots until it is dropped.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Accepts the connections of one listener until `stop` is set, and serves each on its own thread.
fn accept_loop(listener: Listening, kind: Kind, route_for: RouteFor, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::SeqCst) {
        let Ok(Some((guest, peer))) = listener.accept_one() else {
            thread::sleep(ACCEPT_POLL);
            continue;
        };
        let Some(route) = route_for(peer) else {
            eprintln!("noded: a workstation VM's relay refused a connection from CID {peer:?}");
            continue;
        };
        if route.active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            route.active.fetch_sub(1, Ordering::SeqCst);
            eprintln!(
                "noded: a workstation VM's relay refused a connection past {MAX_CONNECTIONS} open"
            );
            continue;
        }
        let slot = Slot(Arc::clone(&route.active));
        thread::spawn(move || {
            let _slot = slot;
            serve(guest, kind, &route);
        });
    }
}

fn serve(mut guest: Guest, kind: Kind, route: &Route) {
    if guest.make_blocking().is_err() {
        return;
    }
    match kind {
        Kind::Config => {
            // The config is taken, not copied: the second connection finds nothing.
            let taken = route
                .config
                .lock()
                .ok()
                .and_then(|mut config| config.take());
            if let Some(bytes) = taken {
                let _ = guest.set_write_timeout(Some(WRITE_TIMEOUT));
                if guest.write_all(&bytes).is_ok() {
                    let _ = guest.shutdown(Shutdown::Both);
                }
            }
        }
        Kind::Link => match route.destination.dial() {
            Ok(tcp) => forward(guest, tcp, &route.stop),
            Err(err) => eprintln!("noded: a workstation VM's relay cannot reach its hub: {err}"),
        },
    }
}

/// Copies bytes both ways until both directions end or the relay stops. A clean end in one direction (EOF) is passed on as
/// a half-close of the other side, so a reply still in flight is not cut off; a broken read or write ends both directions.
/// Each read has a short timeout, so a direction sees the other's end and the stop flag within [`POLL`].
fn forward(guest: Guest, tcp: TcpStream, stop: &Arc<AtomicBool>) {
    let (Ok(mut guest_in), Ok(mut tcp_out)) = (guest.dup(), tcp.try_clone()) else {
        return;
    };
    let mut tcp_in = tcp;
    let mut guest_out = guest;
    let _ = guest_in.set_read_timeout(Some(POLL));
    let _ = tcp_in.set_read_timeout(Some(POLL));
    let _ = guest_out.set_write_timeout(Some(WRITE_TIMEOUT));
    let _ = tcp_out.set_write_timeout(Some(WRITE_TIMEOUT));

    let done = Arc::new(AtomicBool::new(false));
    let up = {
        let done = Arc::clone(&done);
        let stop = Arc::clone(stop);
        thread::spawn(move || {
            let ended = pump(&mut guest_in, &mut tcp_out, &done, &stop, "up");
            if ended == Ended::Eof {
                let half = tcp_out.shutdown(Shutdown::Write);
                if TRACE.load(Ordering::SeqCst) {
                    eprintln!("TRACE up: Eof; tcp half-close {half:?}");
                }
            } else {
                if TRACE.load(Ordering::SeqCst) {
                    eprintln!("TRACE up: {ended:?}");
                }
                done.store(true, Ordering::SeqCst);
            }
        })
    };
    let ended = pump(&mut tcp_in, &mut guest_out, &done, stop, "down");
    if TRACE.load(Ordering::SeqCst) {
        eprintln!("TRACE down: {ended:?}");
    }
    if ended == Ended::Eof {
        let _ = guest_out.shutdown(Shutdown::Write);
    }
    done.store(true, Ordering::SeqCst);
    let _ = up.join();
    let _ = tcp_in.shutdown(Shutdown::Both);
    let _ = guest_out.shutdown(Shutdown::Both);
}

/// How one direction of a relayed connection ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    /// The source reached its end: the other side is half-closed, and the reply may still come back.
    Eof,
    /// A read or write failed, or the other direction ended, or the relay stopped.
    Broken,
}

fn pump(
    from: &mut impl Read,
    to: &mut impl Write,
    done: &AtomicBool,
    stop: &AtomicBool,
    dir: &str,
) -> Ended {
    let trace = TRACE.load(Ordering::SeqCst);
    let mut buf = vec![0u8; COPY_BUF];
    let mut timeouts = 0u32;
    while !done.load(Ordering::SeqCst) && !stop.load(Ordering::SeqCst) {
        match from.read(&mut buf) {
            Ok(0) => {
                if trace {
                    eprintln!("TRACE {dir}: read 0 after {timeouts} timeouts");
                }
                return Ended::Eof;
            }
            Ok(n) => {
                if trace {
                    eprintln!("TRACE {dir}: read {n} bytes");
                }
                if let Err(err) = to.write_all(&buf[..n]) {
                    if trace {
                        eprintln!(
                            "TRACE {dir}: write failed: {err} ({:?})",
                            err.raw_os_error()
                        );
                    }
                    return Ended::Broken;
                }
            }
            Err(err)
                if trace
                    && !matches!(
                        err.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) =>
            {
                eprintln!("TRACE {dir}: read failed: {err} ({:?})", err.raw_os_error());
                return Ended::Broken;
            }
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                timeouts += 1
            }
            Err(_) => return Ended::Broken,
        }
    }
    if trace {
        eprintln!("TRACE {dir}: stopped (done or stop) after {timeouts} timeouts");
    }
    Ended::Broken
}

fn listen_unix(path: &Path) -> io::Result<Listening> {
    // A socket left by a relay that did not stop is replaced; the path names one VM, so no live relay holds it.
    let _ = fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    listener.set_nonblocking(true)?;
    Ok(Listening::Unix(listener))
}

/// A running relay for one plaza workstation VM: its unix sockets, and the route they serve. Dropping it stops it, ends its
/// connections and removes its sockets.
pub struct Relay {
    route: Arc<Route>,
    paths: Vec<PathBuf>,
}

impl Relay {
    /// Starts the relay for guest `cid` under `dir`: it listens on the link and config ports, forwards the link to
    /// `destination`, and serves `config` (the join config's bytes) once on the config port.
    pub fn start(
        dir: &Path,
        cid: u32,
        destination: Destination,
        config: Option<Vec<u8>>,
    ) -> io::Result<Relay> {
        let link_path = socket_path(dir, cid, LINK_PORT);
        let config_path = socket_path(dir, cid, CONFIG_PORT);
        let link = listen_unix(&link_path)?;
        let config_listener = match listen_unix(&config_path) {
            Ok(listener) => listener,
            Err(err) => {
                let _ = fs::remove_file(&link_path);
                return Err(err);
            }
        };
        let route = Route::new(destination, config);
        for (listener, kind) in [(link, Kind::Link), (config_listener, Kind::Config)] {
            let fixed = Arc::clone(&route);
            let route_for: RouteFor = Box::new(move |_| Some(Arc::clone(&fixed)));
            let stop = Arc::clone(&route.stop);
            thread::spawn(move || accept_loop(listener, kind, route_for, stop));
        }
        Ok(Relay {
            route,
            paths: vec![link_path, config_path],
        })
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.route.stop.store(true, Ordering::SeqCst);
        for path in &self.paths {
            let _ = fs::remove_file(path);
        }
    }
}

/// The route a vsock connection belongs to: the one held for its peer's CID, when that CID is admitted. A connection from
/// any other CID has no route.
pub fn route_of_peer(routes: &BTreeMap<u32, Arc<Route>>, peer: Option<u32>) -> Option<Arc<Route>> {
    let peer = peer?;
    let running: Vec<u32> = routes.keys().copied().collect();
    if admitted(peer, &running) {
        routes.get(&peer).cloned()
    } else {
        None
    }
}

#[cfg(target_os = "linux")]
mod host {
    use std::sync::{MutexGuard, PoisonError};
    use std::thread::JoinHandle;

    use super::*;

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The node's AF_VSOCK relay for its QEMU workstation VMs. The host's listeners on [`LINK_PORT`] and [`CONFIG_PORT`] are
    /// shared by every QEMU VM of the node: a VM holds a route ([`Host::register`]) for as long as it runs, the listeners run
    /// while any route is held, and a connection is served only for a running VM's CID ([`route_of_peer`]).
    #[derive(Clone, Default)]
    pub struct Host {
        routes: Arc<Mutex<BTreeMap<u32, Arc<Route>>>>,
        listeners: Arc<Mutex<Option<Listeners>>>,
    }

    struct Listeners {
        stop: Arc<AtomicBool>,
        threads: Vec<JoinHandle<()>>,
    }

    impl Host {
        /// A host with no listeners yet: they start with the first route.
        pub fn new() -> Self {
            Self::default()
        }

        /// Routes `cid`'s connections to `destination`, serving `config` once, for as long as the returned attachment lives.
        /// The first route starts the listeners. A CID that already has a route is refused.
        pub fn register(
            &self,
            cid: u32,
            destination: Destination,
            config: Option<Vec<u8>>,
        ) -> io::Result<Attachment> {
            let mut listeners = lock(&self.listeners);
            if listeners.is_none() {
                *listeners = Some(self.listen()?);
            }
            let route = Route::new(destination, config);
            {
                let mut routes = lock(&self.routes);
                if routes.contains_key(&cid) {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!("CID {cid} already has a relay"),
                    ));
                }
                routes.insert(cid, Arc::clone(&route));
            }
            Ok(Attachment {
                host: self.clone(),
                cid,
                route,
            })
        }

        fn listen(&self) -> io::Result<Listeners> {
            let link = VsockListener::bind(LINK_PORT)?;
            let config = VsockListener::bind(CONFIG_PORT)?;
            let stop = Arc::new(AtomicBool::new(false));
            let threads = [(link, Kind::Link), (config, Kind::Config)]
                .into_iter()
                .map(|(listener, kind)| {
                    let routes = Arc::clone(&self.routes);
                    let route_for: RouteFor =
                        Box::new(move |peer| route_of_peer(&lock(&routes), peer));
                    let stop = Arc::clone(&stop);
                    thread::spawn(move || {
                        accept_loop(Listening::Vsock(listener), kind, route_for, stop);
                    })
                })
                .collect();
            Ok(Listeners { stop, threads })
        }
    }

    /// A VM's route on the node's AF_VSOCK relay. Dropping it ends the VM's connections and removes its route; the last route
    /// stops the listeners, and they are closed before this returns, so a new route can bind them again.
    pub struct Attachment {
        host: Host,
        cid: u32,
        route: Arc<Route>,
    }

    impl Drop for Attachment {
        fn drop(&mut self) {
            self.route.stop.store(true, Ordering::SeqCst);
            let mut listeners = lock(&self.host.listeners);
            let emptied = {
                let mut routes = lock(&self.host.routes);
                if routes
                    .get(&self.cid)
                    .is_some_and(|held| Arc::ptr_eq(held, &self.route))
                {
                    routes.remove(&self.cid);
                }
                routes.is_empty()
            };
            if emptied && let Some(running) = listeners.take() {
                running.stop.store(true, Ordering::SeqCst);
                for thread in running.threads {
                    let _ = thread.join();
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub use host::{Attachment, Host};

/// Off Linux there is no AF_VSOCK, so no QEMU workstation VM can have a route: a register is refused with that reason.
#[cfg(not(target_os = "linux"))]
#[derive(Clone, Default)]
pub struct Host;

#[cfg(not(target_os = "linux"))]
pub struct Attachment;

#[cfg(not(target_os = "linux"))]
impl Host {
    pub fn new() -> Self {
        Self
    }

    pub fn register(
        &self,
        _cid: u32,
        _destination: Destination,
        _config: Option<Vec<u8>>,
    ) -> io::Result<Attachment> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "QEMU workstation VMs need AF_VSOCK, which this platform does not have",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::Instant;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("noded-relay-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// The guest's end of a connection to the relay's link port (a Unix socket stands in for the muxer's).
    fn guest_link(dir: &Path, cid: u32) -> UnixStream {
        UnixStream::connect(socket_path(dir, cid, LINK_PORT)).expect("the link port is listening")
    }

    fn hub_destination(listener: &TcpListener) -> Destination {
        let port = listener.local_addr().expect("addr").port();
        Destination::of_hub(&format!("http://127.0.0.1:{port}")).expect("a destination")
    }

    fn route_to(hub: &str) -> Arc<Route> {
        Route::new(Destination::of_hub(hub).expect("a destination"), None)
    }

    #[test]
    fn the_destination_is_the_hubs_tcp_address_and_nothing_else() {
        let hub = Destination::of_hub("https://hub.example:7443").expect("a destination");
        assert_eq!((hub.host(), hub.port()), ("hub.example", 7443));
        let default_port = Destination::of_hub("https://hub.example").expect("a destination");
        assert_eq!(default_port.port(), 443);
        let v6 = Destination::of_hub("http://[::1]:7700").expect("a destination");
        assert_eq!((v6.host(), v6.port()), ("::1", 7700));
        for refused in [
            "vsock://2:5100",
            "https://hub.example:7443/v1/stations/link",
            "ftp://hub.example",
            "hub.example:7443",
            "https://",
            "",
        ] {
            assert!(
                Destination::of_hub(refused).is_err(),
                "{refused:?} must not be a destination"
            );
        }
    }

    #[test]
    fn only_a_running_qemu_vm_cid_is_admitted_and_the_host_never_is() {
        let running = [4097, 4098];
        assert!(admitted(4097, &running));
        assert!(admitted(4098, &running));
        assert!(
            !admitted(4099, &running),
            "a CID that is not running is refused"
        );
        assert!(!admitted(HOST_CID, &[HOST_CID]), "the host is never a VM");
        assert!(!admitted(0, &[0]), "the reserved CIDs are never a VM");
        assert!(!admitted(1, &[1]), "the reserved CIDs are never a VM");
        assert!(!admitted(u32::MAX, &[u32::MAX]), "any-CID is not a peer");
        assert!(
            !admitted(4097, &[]),
            "with no VM running, nothing is admitted"
        );
    }

    #[test]
    fn a_vsock_peer_is_routed_by_its_own_cid_and_refused_otherwise() {
        let mut routes = BTreeMap::new();
        let vm_a = route_to("https://hub.example:7443");
        let vm_b = route_to("https://hub.example:7443");
        routes.insert(4097, Arc::clone(&vm_a));
        routes.insert(4098, Arc::clone(&vm_b));

        let own = route_of_peer(&routes, Some(4097)).expect("VM 4097's own route");
        assert!(
            Arc::ptr_eq(&own, &vm_a),
            "a peer gets its own route, never another VM's"
        );
        assert!(Arc::ptr_eq(
            &route_of_peer(&routes, Some(4098)).expect("VM 4098"),
            &vm_b
        ));
        assert!(
            route_of_peer(&routes, Some(HOST_CID)).is_none(),
            "the host is refused"
        );
        assert!(
            route_of_peer(&routes, Some(4099)).is_none(),
            "a stopped VM's CID is refused"
        );
        assert!(
            route_of_peer(&routes, None).is_none(),
            "a connection with no peer CID is refused"
        );
    }

    #[test]
    fn bytes_go_both_ways_between_the_guest_and_the_hub_and_to_no_other_address() {
        let dir = scratch("forward");
        let hub = TcpListener::bind("127.0.0.1:0").expect("a hub");
        // A decoy the relay must never dial: it is not the destination, so nothing may connect to it.
        let decoy = TcpListener::bind("127.0.0.1:0").expect("a decoy");
        decoy.set_nonblocking(true).expect("nonblocking");
        let destination = hub_destination(&hub);
        let relay = Relay::start(&dir, 4097, destination, None).expect("the relay");

        let hub_thread = thread::spawn(move || {
            let (mut conn, _) = hub.accept().expect("the relay dials the hub");
            conn.set_read_timeout(Some(Duration::from_secs(5)))
                .expect("timeout");
            // A hub on this machine is told first that the connection is a VM's.
            let mut preface = vec![0u8; apz_http::RELAY_PREFACE.len()];
            conn.read_exact(&mut preface).expect("the relay's preface");
            assert_eq!(preface, apz_http::RELAY_PREFACE);
            let mut got = [0u8; 5];
            conn.read_exact(&mut got).expect("the guest's bytes");
            conn.write_all(b"pong").expect("the reply");
            let mut rest = Vec::new();
            conn.read_to_end(&mut rest).expect("the guest's end");
            (got, rest)
        });

        let mut guest = guest_link(&dir, 4097);
        guest
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        guest.write_all(b"hello").expect("send");
        let mut reply = [0u8; 4];
        guest.read_exact(&mut reply).expect("the hub's reply");
        assert_eq!(&reply, b"pong");
        guest.shutdown(Shutdown::Write).expect("half close");
        let (got, rest) = hub_thread.join().expect("the hub thread");
        assert_eq!(&got, b"hello");
        assert!(rest.is_empty());

        assert!(
            decoy.accept().is_err(),
            "nothing may dial an address the relay was not given"
        );
        drop(relay);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_join_config_is_served_once_and_the_second_connection_gets_nothing() {
        let dir = scratch("config");
        let hub = TcpListener::bind("127.0.0.1:0").expect("a hub");
        let config = br#"{"hub":"vsock://2:5100","pin":"cGlu","code":"apzj1.x"}"#.to_vec();
        let relay = Relay::start(&dir, 4098, hub_destination(&hub), Some(config.clone()))
            .expect("the relay");

        let mut first =
            UnixStream::connect(socket_path(&dir, 4098, CONFIG_PORT)).expect("the config port");
        first
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        let mut got = Vec::new();
        first.read_to_end(&mut got).expect("the config");
        assert_eq!(got, config);

        let mut second =
            UnixStream::connect(socket_path(&dir, 4098, CONFIG_PORT)).expect("the config port");
        second
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        let mut again = Vec::new();
        second.read_to_end(&mut again).expect("an empty end");
        assert!(again.is_empty(), "the config is not served twice");
        drop(relay);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fifth_connection_is_closed_while_four_are_open() {
        let dir = scratch("limit");
        let hub = TcpListener::bind("127.0.0.1:0").expect("a hub");
        let destination = hub_destination(&hub);
        let relay = Relay::start(&dir, 4099, destination, None).expect("the relay");

        // Four connections the hub holds open: each is a forwarding thread, so each counts.
        let hub_holds = thread::spawn(move || {
            let mut held = Vec::new();
            for _ in 0..MAX_CONNECTIONS {
                held.push(hub.accept().expect("a dial").0);
            }
            thread::sleep(Duration::from_secs(3));
            drop(held);
        });
        let mut open: Vec<UnixStream> = (0..MAX_CONNECTIONS)
            .map(|_| guest_link(&dir, 4099))
            .collect();
        // Give the relay a moment to dial all four.
        thread::sleep(Duration::from_millis(300));

        let mut fifth = guest_link(&dir, 4099);
        fifth
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        let started = Instant::now();
        let mut byte = [0u8; 1];
        let n = fifth.read(&mut byte).expect("the relay closes the fifth");
        assert_eq!(n, 0, "the fifth connection is closed at once");
        assert!(started.elapsed() < Duration::from_secs(4));

        for conn in &mut open {
            let _ = conn.shutdown(Shutdown::Both);
        }
        drop(open);
        hub_holds.join().expect("the hub thread");
        drop(relay);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dropping_the_relay_removes_its_sockets() {
        let dir = scratch("drop");
        let hub = TcpListener::bind("127.0.0.1:0").expect("a hub");
        let relay = Relay::start(&dir, 4100, hub_destination(&hub), None).expect("the relay");
        assert!(socket_path(&dir, 4100, LINK_PORT).exists());
        assert!(socket_path(&dir, 4100, CONFIG_PORT).exists());
        drop(relay);
        assert!(!socket_path(&dir, 4100, LINK_PORT).exists());
        assert!(!socket_path(&dir, 4100, CONFIG_PORT).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// DIAGNOSTIC (branch diag/windows-relay only): the half-close test, 100 times, traced. `reply` makes the hub answer
    /// "pong" (a write on the guest's socket while the relay waits to read it) before the guest half-closes.
    fn relay_stress(tag: &str, reply: bool) {
        TRACE.store(true, Ordering::SeqCst);
        let mut bad = Vec::new();
        for round in 0..100u32 {
            let dir = scratch(&format!("{tag}{round}"));
            let hub = TcpListener::bind("127.0.0.1:0").expect("a hub");
            let destination = hub_destination(&hub);
            let relay = Relay::start(&dir, 4097, destination, None).expect("the relay");
            let hub_thread = thread::spawn(move || -> Result<(), String> {
                let (mut conn, _) = hub.accept().map_err(|e| format!("accept: {e}"))?;
                conn.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut preface = vec![0u8; apz_http::RELAY_PREFACE.len()];
                conn.read_exact(&mut preface)
                    .map_err(|e| format!("preface: {e}"))?;
                let mut got = [0u8; 5];
                conn.read_exact(&mut got)
                    .map_err(|e| format!("hello: {e}"))?;
                if reply {
                    conn.write_all(b"pong").map_err(|e| format!("pong: {e}"))?;
                }
                let mut rest = Vec::new();
                conn.read_to_end(&mut rest)
                    .map_err(|e| format!("the guest's end: {e}"))?;
                Ok(())
            });
            let mut guest = guest_link(&dir, 4097);
            guest
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            guest.write_all(b"hello").unwrap();
            if reply {
                let mut r = [0u8; 4];
                guest.read_exact(&mut r).expect("the pong");
            }
            eprintln!("TRACE {tag} round {round}: guest half-closes");
            guest.shutdown(Shutdown::Write).expect("half close");
            if let Err(why) = hub_thread.join().expect("the hub thread") {
                eprintln!("TRACE {tag} round {round}: BAD: {why}");
                bad.push(round);
            }
            drop(relay);
            let _ = fs::remove_dir_all(&dir);
        }
        eprintln!("DIAG {tag}: {} of 100 rounds bad: {bad:?}", bad.len());
        assert!(bad.is_empty(), "DIAG {tag}: bad rounds {bad:?}");
    }

    #[test]
    fn diag_relay_half_close_after_reply() {
        relay_stress("reply", true);
    }

    #[test]
    fn diag_relay_half_close_no_reply() {
        relay_stress("noreply", false);
    }
}
