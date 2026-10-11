//! The threaded loopback server.

use std::io::{self, BufReader};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::{Limits, Request, Response, host_is_loopback};

/// The most connections served at once; the next one is answered 503.
const MAX_CONNECTIONS: usize = 256;

/// Every connection's read and write timeout; it also bounds how long an SSE
/// client may stall a write.
const TIMEOUT: Duration = Duration::from_secs(10);

/// A bound, not yet serving, loopback listener.
pub struct Server {
    listener: TcpListener,
    limits: Limits,
}

impl Server {
    /// Binds `127.0.0.1:port` (`0` picks a free port).
    pub fn bind(port: u16) -> io::Result<Self> {
        Ok(Self { listener: TcpListener::bind(("127.0.0.1", port))?, limits: Limits::default() })
    }

    pub fn limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Serves on a background thread, one thread per connection, until the
    /// returned [`Handle`] is stopped or dropped.
    pub fn spawn<H>(self, handler: H) -> io::Result<Handle>
    where
        H: Fn(&Request) -> Response + Send + Sync + 'static,
    {
        let addr = self.listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = thread::spawn(move || self.accept_loop(Arc::new(handler), &flag));
        Ok(Handle { addr, stop, thread: Some(thread) })
    }

    fn accept_loop<H>(self, handler: Arc<H>, stop: &AtomicBool)
    where
        H: Fn(&Request) -> Response + Send + Sync + 'static,
    {
        let active = Arc::new(AtomicUsize::new(0));
        for stream in self.listener.incoming() {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            let stream = match stream {
                Ok(stream) => stream,
                Err(err) => {
                    // EMFILE and friends: back off rather than spin.
                    eprintln!("apz-http: accept failed: {err}");
                    thread::sleep(Duration::from_millis(50));
                    continue;
                }
            };
            if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                active.fetch_sub(1, Ordering::SeqCst);
                let mut stream = stream;
                let _ = stream.set_write_timeout(Some(TIMEOUT));
                let _ = Response::error(503, "too many connections").write_to(&mut stream);
                continue;
            }
            let (handler, active) = (Arc::clone(&handler), Arc::clone(&active));
            let limits = self.limits;
            thread::spawn(move || {
                serve_connection(stream, &*handler, limits);
                active.fetch_sub(1, Ordering::SeqCst);
            });
        }
    }
}

/// One request, one response, close.
fn serve_connection<H>(stream: TcpStream, handler: &H, limits: Limits)
where
    H: Fn(&Request) -> Response,
{
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(TIMEOUT));
    let _ = stream.set_write_timeout(Some(TIMEOUT));
    let mut reader = BufReader::new(stream);
    let response = match Request::read(&mut reader, limits) {
        Err(message) => Response::error(400, message),
        Ok(request) => match request.header("host") {
            Some(host) if !host_is_loopback(host) => {
                Response::error(403, "the Host header must name loopback")
            }
            _ => handler(&request),
        },
    };
    // A client that hung up has nobody left to tell.
    let _ = response.write_to(reader.get_mut());
}

/// A serving [`Server`]. Dropping it stops the accept loop.
pub struct Handle {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Handle {
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Blocks until the server is stopped.
    pub fn join(mut self) -> io::Result<()> {
        match self.thread.take() {
            Some(thread) => thread.join().map_err(|_| io::Error::other("the accept thread panicked")),
            None => Ok(()),
        }
    }

    /// Stops accepting; connections in flight finish on their own threads.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            // Wake the loop out of `accept(2)`.
            let _ = TcpStream::connect(self.addr);
            let _ = thread.join();
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop();
    }
}
