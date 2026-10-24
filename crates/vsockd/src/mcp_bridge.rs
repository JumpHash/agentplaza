//! The guest end of an MCP server's stdio: the `mcp-bridge` binary, which init runs as the only process of a
//! `plaza.mcp=1` VM. It carries one MCP server's traffic between the host (noded) and the server process in the guest.
//!
//! The host is CID 2, and the bridge uses three of its ports:
//!
//! - [`STDIO_PORT`] (5200): the bridge first reads one newline-terminated JSON line, the launch spec ([`Spec`]), and
//!   runs the server with exactly that program, arguments, environment and working directory. The rest of the
//!   connection is the server's stdin (host to guest) and stdout (guest to host), raw.
//! - [`STDERR_PORT`] (5201): the server's stderr, raw, guest to host.
//! - [`PROXY_PORT`] (5202): the VM's only route out. The bridge listens on [`PROXY_ADDR`] and opens one connection to
//!   this port for each TCP connection it accepts, copying bytes both ways. The host dials the hub's egress proxy for
//!   each one. Nothing else is ever dialled.
//!
//! The bridge exits when the server exits, with the server's exit code (1 when a signal killed it, or when it could
//! not be started). The transport is a [`Transport`]: vsock on Linux, and a Unix socket per port under a directory
//! where vsock is not available (the tests, and `APZ_MCP_BRIDGE_UNIX_DIR` on a development host).

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::sync::Arc;
use std::thread;

use serde::Deserialize;

/// The server's stdio: the launch spec, then stdin and stdout.
const STDIO_PORT: u32 = 5200;
/// The server's stderr.
const STDERR_PORT: u32 = 5201;
/// The egress proxy: one connection per accepted proxy connection.
const PROXY_PORT: u32 = 5202;
/// Where the server reaches the egress proxy (`HTTP_PROXY` in the server's environment names it).
const PROXY_ADDR: &str = "127.0.0.1:3128";
/// Development hosts only: a directory of Unix sockets `port<N>`, used instead of vsock when set.
const UNIX_DIR_ENV: &str = "APZ_MCP_BRIDGE_UNIX_DIR";
/// The copy buffer of each pump.
const BUF_BYTES: usize = 16 * 1024;
/// The longest launch spec line accepted. A spec names a program, its arguments and an environment; it is never a
/// payload.
const MAX_SPEC_BYTES: usize = 1 << 20;

/// The launch spec the host writes first on the stdio port.
#[derive(Deserialize)]
struct Spec {
    program: String,
    #[serde(default)]
    args: Vec<String>,
    /// The server's whole environment: the bridge clears its own first.
    #[serde(default)]
    env: BTreeMap<String, String>,
    cwd: String,
}

/// Opens a connection to one of the host's ports.
trait Transport: Send + Sync {
    fn connect(&self, port: u32) -> io::Result<Box<dyn Link>>;
}

/// One connected stream: the bridge's link to a host port, or one end of a proxy tunnel.
trait Link: Read + Write + Send {
    /// A second handle to the same connection, for the thread that copies the other way.
    fn duplicate(&self) -> io::Result<Box<dyn Link>>;
    /// Ends the write side, so the peer reads end of stream. The read side stays open.
    fn shutdown_write(&self) -> io::Result<()>;
}

impl Link for TcpStream {
    fn duplicate(&self) -> io::Result<Box<dyn Link>> {
        Ok(Box::new(self.try_clone()?))
    }

    fn shutdown_write(&self) -> io::Result<()> {
        self.shutdown(Shutdown::Write)
    }
}

impl Link for apz_uds::UnixStream {
    fn duplicate(&self) -> io::Result<Box<dyn Link>> {
        Ok(Box::new(self.try_clone()?))
    }

    fn shutdown_write(&self) -> io::Result<()> {
        self.shutdown(Shutdown::Write)
    }
}

/// The guest's transport: AF_VSOCK connections to the host.
struct Vsock;

impl Transport for Vsock {
    #[cfg(target_os = "linux")]
    fn connect(&self, port: u32) -> io::Result<Box<dyn Link>> {
        /// The host's vsock context ID (`VMADDR_CID_HOST`).
        const HOST_CID: u32 = 2;
        let sock = crate::sys::vsock_connect(HOST_CID, port)?;
        // A connected AF_VSOCK stream is a socket descriptor like any other: read, write, dup and shutdown(2) all
        // apply, and those are the only calls a `Link` makes. std's TCP stream wraps the descriptor, so it carries
        // the vsock link. Its address-family methods (peer_addr, set_nodelay) are never called.
        Ok(Box::new(TcpStream::from(sock)))
    }

    #[cfg(not(target_os = "linux"))]
    fn connect(&self, port: u32) -> io::Result<Box<dyn Link>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("vsock port {port}: the mcp bridge reaches the host over vsock only on Linux"),
        ))
    }
}

/// Sockets under a directory: `<dir>/port<N>` for port `N`.
struct UnixDir(PathBuf);

impl Transport for UnixDir {
    fn connect(&self, port: u32) -> io::Result<Box<dyn Link>> {
        Ok(Box::new(apz_uds::UnixStream::connect(
            self.0.join(format!("port{port}")),
        )?))
    }
}

/// The bridge's entry point: serves the transport the environment selects, on the loopback proxy address.
pub fn main() -> ExitCode {
    let transport: Arc<dyn Transport> = match std::env::var_os(UNIX_DIR_ENV) {
        Some(dir) => Arc::new(UnixDir(PathBuf::from(dir))),
        None => Arc::new(Vsock),
    };
    let proxy = match TcpListener::bind(PROXY_ADDR) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("mcp-bridge: cannot listen on {PROXY_ADDR}: {err}");
            return ExitCode::FAILURE;
        }
    };
    match run(transport, proxy) {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            eprintln!("mcp-bridge: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Runs the bridge once: reads the launch spec, starts the server, pumps its stdio and stderr, and serves the proxy
/// until the server exits. Returns the server's exit code (1 when a signal killed it, or it could not be started).
/// Errors are the bridge's own: a host port could not be reached, or the spec was unreadable.
fn run(transport: Arc<dyn Transport>, proxy: TcpListener) -> io::Result<u8> {
    let mut stdio = transport.connect(STDIO_PORT)?;
    let spec = read_spec(&mut *stdio)?;
    let mut stderr_link = transport.connect(STDERR_PORT)?;
    let mut child = match spawn(&spec) {
        Ok(child) => child,
        Err(err) => {
            // The server never ran, so the host learns why on the stderr port. Both links close on return, so
            // the host sees end of stream on each.
            let _ = writeln!(
                stderr_link,
                "mcp-bridge: cannot start {}: {err}",
                spec.program
            );
            return Ok(1);
        }
    };
    let stdin = child.stdin.take().expect("the server's stdin is piped");
    let mut stdout = child.stdout.take().expect("the server's stdout is piped");
    let mut stderr_pipe = child.stderr.take().expect("the server's stderr is piped");

    // Host to server. The server's stdin is closed when the host ends the stdio link, which is end of input for
    // the server. After the server exits, this thread stays blocked on the link until the host sends or closes.
    let mut input = stdio.duplicate()?;
    thread::spawn(move || {
        let mut stdin = stdin;
        pump(&mut *input, &mut stdin);
    });
    // Server to host. When the server's stdout ends, the host reads end of stream.
    let output = thread::spawn(move || {
        pump(&mut stdout, &mut *stdio);
        let _ = stdio.shutdown_write();
    });
    // Server stderr to the host's stderr port.
    let errors = thread::spawn(move || {
        pump(&mut stderr_pipe, &mut *stderr_link);
        let _ = stderr_link.shutdown_write();
    });

    serve_proxy(transport, proxy);
    let status = child.wait()?;
    // The server is gone, so its stdout and stderr end once they are drained.
    let _ = output.join();
    let _ = errors.join();
    Ok(exit_code(status))
}

/// Reads the launch spec: one line, read a byte at a time, so nothing after its newline is taken from the stream.
fn read_spec<R: Read + ?Sized>(stream: &mut R) -> io::Result<Spec> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the host closed the stdio port before the launch spec",
                ));
            }
            Ok(_) if byte[0] == b'\n' => break,
            Ok(_) if line.len() == MAX_SPEC_BYTES => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the launch spec is longer than 1 MiB",
                ));
            }
            Ok(_) => line.push(byte[0]),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    serde_json::from_slice(&line).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("bad launch spec: {err}"),
        )
    })
}

/// Starts the server with exactly the spec's environment and working directory, its stdio piped.
fn spawn(spec: &Spec) -> io::Result<Child> {
    Command::new(&spec.program)
        .args(&spec.args)
        .env_clear()
        .envs(&spec.env)
        .current_dir(&spec.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
}

/// The bridge's exit code for the server's exit status: the server's own code, or 1 when a signal ended it.
fn exit_code(status: ExitStatus) -> u8 {
    status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1)
}

/// Accepts the proxy connections on a thread of their own. The thread ends with the process.
fn serve_proxy(transport: Arc<dyn Transport>, listener: TcpListener) {
    thread::spawn(move || {
        for conn in listener.incoming() {
            match conn {
                Ok(tcp) => {
                    let transport = Arc::clone(&transport);
                    thread::spawn(move || tunnel(&*transport, Box::new(tcp)));
                }
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::ConnectionAborted | io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => break,
            }
        }
    });
}

/// Carries one accepted proxy connection to a new connection to the host's proxy port, both ways. Each direction
/// ends with a half-close: when it reads end of stream, the other side's write end is shut down.
fn tunnel(transport: &dyn Transport, client: Box<dyn Link>) {
    // If the host cannot be reached, `client` is dropped, which closes the connection.
    let Ok(host) = transport.connect(PROXY_PORT) else {
        return;
    };
    let (Ok(client_out), Ok(host_out)) = (client.duplicate(), host.duplicate()) else {
        return;
    };
    thread::spawn(move || relay(client, host_out));
    relay(host, client_out);
}

/// Copies `from` to `to` until `from` ends, then shuts down `to`'s write side.
fn relay(mut from: Box<dyn Link>, mut to: Box<dyn Link>) {
    pump(&mut *from, &mut *to);
    let _ = to.shutdown_write();
}

/// Copies `src` to `dst` until `src` ends. Once a write fails (the far end is gone), nothing more is written, but
/// `src` is still read to its end: the process writing into it never blocks on a pipe nobody drains.
fn pump<R: Read + ?Sized, W: Write + ?Sized>(src: &mut R, dst: &mut W) {
    let mut buf = [0u8; BUF_BYTES];
    let mut writable = true;
    loop {
        match src.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if writable && dst.write_all(&buf[..n]).is_err() {
                    writable = false;
                }
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    let _ = dst.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::net::SocketAddr;
    use std::thread::JoinHandle;

    use apz_uds::UnixListener;

    /// The host's side of the three ports: one Unix socket per port, under a directory of this test's own.
    struct Host {
        dir: PathBuf,
        stdio: UnixListener,
        stderr: UnixListener,
        // Only the Unix-only tunnel test reads the proxy socket.
        #[cfg_attr(not(unix), allow(dead_code))]
        proxy: UnixListener,
    }

    impl Host {
        fn new(name: &str) -> Host {
            let dir = std::env::temp_dir().join(format!("vmcp-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let bind = |port: u32| UnixListener::bind(dir.join(format!("port{port}"))).unwrap();
            Host {
                stdio: bind(STDIO_PORT),
                stderr: bind(STDERR_PORT),
                proxy: bind(PROXY_PORT),
                dir,
            }
        }

        /// Runs the bridge in a thread, with its proxy listener on an ephemeral loopback port. Returns the thread
        /// and the proxy address the server would use.
        fn start(&self) -> (JoinHandle<io::Result<u8>>, SocketAddr) {
            let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = proxy.local_addr().unwrap();
            let transport: Arc<dyn Transport> = Arc::new(UnixDir(self.dir.clone()));
            (thread::spawn(move || run(transport, proxy)), addr)
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Accepts the bridge's stdio connection and writes `spec` as its launch line.
    fn launch(host: &Host, spec: &str) -> apz_uds::UnixStream {
        let (mut stdio, _) = host.stdio.accept().unwrap();
        stdio.write_all(format!("{spec}\n").as_bytes()).unwrap();
        stdio
    }

    #[test]
    fn mcp_spec_is_one_line_with_defaults() {
        let mut input =
            Cursor::new(b"{\"program\":\"/bin/true\",\"cwd\":\"/\"}\nleft over".to_vec());
        let spec = read_spec(&mut input).unwrap();
        assert_eq!(spec.program, "/bin/true");
        assert!(spec.args.is_empty() && spec.env.is_empty());
        assert_eq!(spec.cwd, "/");
        // The bytes after the newline stay in the stream: they are the server's stdin.
        let mut rest = String::new();
        input.read_to_string(&mut rest).unwrap();
        assert_eq!(rest, "left over");

        // The host closing before the newline, and a line that is not a spec, are both errors.
        assert!(read_spec(&mut Cursor::new(b"{\"program\":".to_vec())).is_err());
        assert!(read_spec(&mut Cursor::new(b"not json\n".to_vec())).is_err());
    }

    // These run the guest's /bin/sh, /bin/cat or env, so they are Unix-only. The bridge only runs in the Linux guest.
    #[test]
    #[cfg(unix)]
    fn mcp_server_env_is_exactly_the_spec() {
        let host = Host::new("env");
        let (bridge, _) = host.start();
        let mut stdio = launch(
            &host,
            r#"{"program":"/usr/bin/env","env":{"HOME":"/workspace","GREETING":"hi"},"cwd":"/"}"#,
        );
        // `env` prints its environment and exits, so the host reads it to end of stream.
        let mut out = String::new();
        stdio.read_to_string(&mut out).unwrap();
        let mut lines: Vec<&str> = out.lines().collect();
        lines.sort();
        assert_eq!(lines, ["GREETING=hi", "HOME=/workspace"]);
        assert_eq!(bridge.join().unwrap().unwrap(), 0);
    }

    #[test]
    #[cfg(unix)]
    fn mcp_stdin_round_trips_and_its_end_ends_the_server() {
        let host = Host::new("stdin");
        let (bridge, _) = host.start();
        let mut stdio = launch(&host, r#"{"program":"/bin/cat","cwd":"/"}"#);
        stdio.write_all(b"round trip\n").unwrap();
        // The host's end of input: `cat` sees end of file, exits, and the host reads its output to the end.
        stdio.shutdown(Shutdown::Write).unwrap();
        let mut out = String::new();
        stdio.read_to_string(&mut out).unwrap();
        assert_eq!(out, "round trip\n");
        assert_eq!(bridge.join().unwrap().unwrap(), 0);
    }

    #[test]
    #[cfg(unix)]
    fn mcp_stderr_lines_reach_port_5201() {
        let host = Host::new("stderr");
        let (bridge, _) = host.start();
        let spec = r#"{"program":"/bin/sh","args":["-c","echo first-error >&2; echo second-error >&2; exec cat"],"cwd":"/"}"#;
        let mut stdio = launch(&host, spec);
        let (mut stderr, _) = host.stderr.accept().unwrap();
        stdio.shutdown(Shutdown::Write).unwrap();
        let mut out = String::new();
        stdio.read_to_string(&mut out).unwrap();
        assert_eq!(out, "");
        let mut errors = String::new();
        stderr.read_to_string(&mut errors).unwrap();
        assert_eq!(errors, "first-error\nsecond-error\n");
        assert_eq!(bridge.join().unwrap().unwrap(), 0);
    }

    #[test]
    #[cfg(unix)]
    fn mcp_proxy_connections_tunnel_to_port_5202_both_ways() {
        let host = Host::new("tunnel");
        let (bridge, addr) = host.start();
        let mut stdio = launch(&host, r#"{"program":"/bin/cat","cwd":"/"}"#);
        let payload: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let reply = b"reply from the egress proxy".to_vec();

        // The host's proxy: reads the request to end of stream, then answers and closes.
        let proxy = host.proxy.try_clone().unwrap();
        let host_reply = reply.clone();
        let host_side = thread::spawn(move || {
            let (mut conn, _) = proxy.accept().unwrap();
            let mut received = Vec::new();
            conn.read_to_end(&mut received).unwrap();
            conn.write_all(&host_reply).unwrap();
            received
        });

        // The server's side: a TCP client on the bridge's proxy address.
        let mut client = TcpStream::connect(addr).unwrap();
        client.write_all(&payload).unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        let mut got_reply = Vec::new();
        client.read_to_end(&mut got_reply).unwrap();
        let received = host_side.join().unwrap();
        assert!(received == payload, "the request reached the host altered");
        assert_eq!(got_reply, reply);

        // The server has nothing on stdin; its end of input ends it.
        stdio.shutdown(Shutdown::Write).unwrap();
        let mut out = String::new();
        stdio.read_to_string(&mut out).unwrap();
        assert_eq!(out, "");
        assert_eq!(bridge.join().unwrap().unwrap(), 0);
    }

    #[test]
    #[cfg(unix)]
    fn mcp_exit_code_is_the_servers() {
        let host = Host::new("exit");
        let (bridge, _) = host.start();
        let mut stdio = launch(
            &host,
            r#"{"program":"/bin/sh","args":["-c","exit 7"],"cwd":"/"}"#,
        );
        let mut out = String::new();
        stdio.read_to_string(&mut out).unwrap();
        assert_eq!(out, "");
        assert_eq!(bridge.join().unwrap().unwrap(), 7);
    }

    #[test]
    #[cfg(unix)]
    fn mcp_server_killed_by_a_signal_exits_1() {
        let host = Host::new("signal");
        let (bridge, _) = host.start();
        let mut stdio = launch(
            &host,
            r#"{"program":"/bin/sh","args":["-c","kill -KILL $$"],"cwd":"/"}"#,
        );
        let mut out = String::new();
        stdio.read_to_string(&mut out).unwrap();
        assert_eq!(bridge.join().unwrap().unwrap(), 1);
    }

    #[test]
    fn mcp_server_that_cannot_start_reports_on_5201_and_exits_1() {
        let host = Host::new("spawn");
        let (bridge, _) = host.start();
        let mut stdio = launch(&host, r#"{"program":"/nonexistent/mcp-server","cwd":"/"}"#);
        let (mut stderr, _) = host.stderr.accept().unwrap();
        let mut message = String::new();
        stderr.read_to_string(&mut message).unwrap();
        assert!(
            message.starts_with("mcp-bridge: cannot start /nonexistent/mcp-server: "),
            "unexpected message: {message}"
        );
        let mut out = String::new();
        stdio.read_to_string(&mut out).unwrap();
        assert_eq!(out, "");
        assert_eq!(bridge.join().unwrap().unwrap(), 1);
    }

    #[test]
    fn mcp_pump_reads_on_after_the_far_end_is_gone() {
        struct Gone;
        impl Write for Gone {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut src = io::repeat(7).take(100_000);
        pump(&mut src, &mut Gone);
        assert_eq!(src.limit(), 0);
    }
}
