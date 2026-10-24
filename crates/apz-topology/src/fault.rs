//! The fault proxy (PLAN 22.8): a TCP proxy in front of one dependency of a topology. Its services reach the dependency
//! through it, and a resilience scenario sets its fault: latency on every request, a loss or a reset on a share of the
//! connections, or a partition that refuses traffic until it heals.
//!
//! The share of connections is exact and needs no random numbers: of every 100 connections, `percent` are hit, spread
//! evenly (see [`hit`]), so a run with 30 % resets resets connections 3, 6, 9, 13, ... and a test can count them.
//!
//! What a client sees: a loss reads the request and closes without an answer (the client reads end of stream); a reset
//! closes the connection on arrival (the client's request meets a reset); a partition closes new connections on arrival
//! and severs the open ones. Latency holds each request for the set time before it reaches the dependency.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The most latency a scenario may add to a request, in milliseconds (ten minutes).
pub const MAX_LATENCY_MS: u64 = 600_000;

/// A fault the proxy applies to its dependency's traffic. `Healthy` is what a heal sets; a scenario never declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Fault {
    /// No fault: traffic passes.
    Healthy,
    /// Each request is held for `ms` before it reaches the dependency.
    Latency { ms: u64 },
    /// `percent` of the connections lose their request: it is read and the connection closed without an answer.
    Loss { percent: u32 },
    /// `percent` of the connections are reset when they arrive.
    Reset { percent: u32 },
    /// The dependency is unreachable: new connections are closed on arrival and open ones are severed.
    Partition,
    /// The traffic of the lab VM's bridge is shaped (tc netem), not the proxy's: `delay_ms` and `loss_percent` on every
    /// packet. Applied by [`crate::netem`] in the guest, where the node can.
    Netem { delay_ms: u64, loss_percent: u32 },
}

impl Fault {
    /// The fault's numbers are in range.
    pub fn check(&self) -> Result<(), String> {
        match self {
            Fault::Healthy => Ok(()),
            Fault::Latency { ms } if *ms > MAX_LATENCY_MS => {
                Err(format!("latency of {ms} ms is over {MAX_LATENCY_MS} ms"))
            }
            Fault::Loss { percent } | Fault::Reset { percent } if *percent > 100 => {
                Err(format!("a share of {percent} % is over 100 %"))
            }
            Fault::Netem {
                delay_ms,
                loss_percent,
            } if *delay_ms > MAX_LATENCY_MS || *loss_percent > 100 => Err(format!(
                "netem with {delay_ms} ms and {loss_percent} % is out of range"
            )),
            _ => Ok(()),
        }
    }
}

/// Whether connection `k` (counted from 0) of a fault that hits `percent` of connections is hit: exactly `percent` of
/// every 100 consecutive connections, spread evenly.
pub fn hit(k: u64, percent: u32) -> bool {
    let p = u64::from(percent.min(100));
    ((k + 1) * p) / 100 > (k * p) / 100
}

/// What the proxy has done since it started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    pub connections: u64,
    pub forwarded: u64,
    pub lost: u64,
    pub reset: u64,
    pub refused: u64,
}

#[derive(Default)]
struct Counters {
    connections: AtomicU64,
    forwarded: AtomicU64,
    lost: AtomicU64,
    reset: AtomicU64,
    refused: AtomicU64,
}

/// The open connections: each client socket and its upstream socket, by an id, so a partition can sever them.
type Open = BTreeMap<u64, (TcpStream, TcpStream)>;

struct Shared {
    fault: Mutex<Fault>,
    seq: AtomicU64,
    open: Mutex<Open>,
    counters: Counters,
}

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A running proxy: it listens on `addr` and forwards to `upstream` under its fault. It stops when dropped.
pub struct Proxy {
    addr: SocketAddr,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
}

impl Proxy {
    /// Starts a proxy listening on `listen` (use port 0 for a free port) in front of `upstream`, with `fault` set.
    pub fn start(listen: SocketAddr, upstream: SocketAddr, fault: Fault) -> io::Result<Proxy> {
        let listener = TcpListener::bind(listen)?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            fault: Mutex::new(fault),
            seq: AtomicU64::new(0),
            open: Mutex::new(BTreeMap::new()),
            counters: Counters::default(),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let (serving, halt) = (Arc::clone(&shared), Arc::clone(&stop));
        thread::spawn(move || {
            for stream in listener.incoming() {
                if halt.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(client) = stream else { continue };
                let shared = Arc::clone(&serving);
                thread::spawn(move || handle(client, upstream, &shared));
            }
        });
        Ok(Proxy { addr, shared, stop })
    }

    /// The address the proxy listens on (the dependency's address in the services' environment).
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Sets the fault for the connections from now on. A partition also severs the connections that are open.
    pub fn set_fault(&self, fault: Fault) {
        *locked(&self.shared.fault) = fault;
        if fault == Fault::Partition {
            let mut open = locked(&self.shared.open);
            for (client, upstream) in open.values() {
                let _ = client.shutdown(Shutdown::Both);
                let _ = upstream.shutdown(Shutdown::Both);
            }
            open.clear();
        }
    }

    /// The fault now in force.
    pub fn fault(&self) -> Fault {
        *locked(&self.shared.fault)
    }

    /// What the proxy has done so far.
    pub fn stats(&self) -> Stats {
        let c = &self.shared.counters;
        Stats {
            connections: c.connections.load(Ordering::SeqCst),
            forwarded: c.forwarded.load(Ordering::SeqCst),
            lost: c.lost.load(Ordering::SeqCst),
            reset: c.reset.load(Ordering::SeqCst),
            refused: c.refused.load(Ordering::SeqCst),
        }
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop so it sees the flag.
        let _ = TcpStream::connect(self.addr);
        for (client, upstream) in locked(&self.shared.open).values() {
            let _ = client.shutdown(Shutdown::Both);
            let _ = upstream.shutdown(Shutdown::Both);
        }
    }
}

/// Serves one client connection under the fault in force when it arrives.
fn handle(client: TcpStream, upstream: SocketAddr, shared: &Shared) {
    let k = shared.seq.fetch_add(1, Ordering::SeqCst);
    let counters = &shared.counters;
    counters.connections.fetch_add(1, Ordering::SeqCst);
    let fault = *locked(&shared.fault);
    match fault {
        Fault::Partition => {
            counters.refused.fetch_add(1, Ordering::SeqCst);
            let _ = client.shutdown(Shutdown::Both);
            return;
        }
        Fault::Reset { percent } if hit(k, percent) => {
            counters.reset.fetch_add(1, Ordering::SeqCst);
            drop(client);
            return;
        }
        Fault::Loss { percent } if hit(k, percent) => {
            counters.lost.fetch_add(1, Ordering::SeqCst);
            let mut client = client;
            let _ = client.set_read_timeout(Some(Duration::from_secs(5)));
            let mut request = [0u8; 4096];
            let _ = client.read(&mut request);
            let _ = client.shutdown(Shutdown::Both);
            return;
        }
        _ => {}
    }
    let delay = match fault {
        Fault::Latency { ms } => Duration::from_millis(ms),
        _ => Duration::ZERO,
    };
    let Ok(server) = TcpStream::connect_timeout(&upstream, Duration::from_secs(5)) else {
        return;
    };
    let (Ok(client_open), Ok(server_open), Ok(client_read), Ok(server_read)) = (
        client.try_clone(),
        server.try_clone(),
        client.try_clone(),
        server.try_clone(),
    ) else {
        return;
    };
    locked(&shared.open).insert(k, (client_open, server_open));
    counters.forwarded.fetch_add(1, Ordering::SeqCst);
    let forward = thread::spawn(move || pump(client_read, server, delay));
    pump(server_read, client, Duration::ZERO);
    let _ = forward.join();
    locked(&shared.open).remove(&k);
}

/// Copies `from` to `to` until `from` ends, holding each chunk for `delay` first.
fn pump(mut from: TcpStream, mut to: TcpStream, delay: Duration) {
    let mut buf = [0u8; 8192];
    loop {
        match from.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if !delay.is_zero() {
                    thread::sleep(delay);
                }
                if to.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        }
    }
    let _ = to.shutdown(Shutdown::Write);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// An echo server on the loopback: answers each connection with what it reads, until the client closes.
    fn echo() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                thread::spawn(move || {
                    let mut buf = [0u8; 1024];
                    while let Ok(n) = stream.read(&mut buf) {
                        if n == 0 || stream.write_all(&buf[..n]).is_err() {
                            break;
                        }
                    }
                });
            }
        });
        addr
    }

    fn loopback() -> SocketAddr {
        "127.0.0.1:0".parse().unwrap()
    }

    /// One request through `addr`: the echoed bytes, or None when the proxy did not answer (lost, reset or refused).
    fn exchange(addr: SocketAddr) -> Option<Vec<u8>> {
        let mut stream = TcpStream::connect(addr).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        stream.write_all(b"ping").ok()?;
        let mut got = [0u8; 4];
        stream.read_exact(&mut got).ok()?;
        Some(got.to_vec())
    }

    #[test]
    fn a_share_of_connections_is_exact_and_spread_evenly() {
        let hits: Vec<u64> = (0..10).filter(|k| hit(*k, 30)).collect();
        assert_eq!(hits, [3, 6, 9]);
        assert_eq!((0..100).filter(|k| hit(*k, 25)).count(), 25);
        assert!((0..100).all(|k| hit(k, 100)));
        assert!((0..100).all(|k| !hit(k, 0)));
        // No two lost in a row at 30 %: a retry after one loss always finds the next connection up.
        assert!((0..1000).all(|k| !(hit(k, 30) && hit(k + 1, 30))));
    }

    #[test]
    fn the_fault_numbers_are_checked() {
        assert!(Fault::Loss { percent: 101 }.check().is_err());
        assert!(
            Fault::Latency {
                ms: MAX_LATENCY_MS + 1
            }
            .check()
            .is_err()
        );
        assert!(
            Fault::Netem {
                delay_ms: 5,
                loss_percent: 5
            }
            .check()
            .is_ok()
        );
        assert!(Fault::Partition.check().is_ok());
    }

    #[test]
    fn healthy_traffic_passes_and_latency_holds_each_request() {
        let upstream = echo();
        let proxy = Proxy::start(loopback(), upstream, Fault::Healthy).unwrap();
        assert_eq!(exchange(proxy.addr()).as_deref(), Some(&b"ping"[..]));

        proxy.set_fault(Fault::Latency { ms: 150 });
        let started = Instant::now();
        assert_eq!(exchange(proxy.addr()).as_deref(), Some(&b"ping"[..]));
        let took = started.elapsed();
        assert!(took >= Duration::from_millis(150), "{took:?}");
        assert_eq!(proxy.stats().forwarded, 2);
    }

    #[test]
    fn loss_drops_exactly_its_share_of_requests() {
        let upstream = echo();
        let proxy = Proxy::start(loopback(), upstream, Fault::Loss { percent: 50 }).unwrap();
        let answered = (0..10).filter(|_| exchange(proxy.addr()).is_some()).count();
        let stats = proxy.stats();
        assert_eq!(stats.lost, 5, "{stats:?}");
        assert_eq!(answered, 5, "the clients saw the lost requests end");
        assert_eq!(stats.forwarded, 5);
    }

    #[test]
    fn resets_fail_their_share_of_connections() {
        let upstream = echo();
        let proxy = Proxy::start(loopback(), upstream, Fault::Reset { percent: 30 }).unwrap();
        let answered = (0..10).filter(|_| exchange(proxy.addr()).is_some()).count();
        let stats = proxy.stats();
        assert_eq!(stats.reset, 3, "{stats:?}");
        assert_eq!(answered, 7);
    }

    #[test]
    fn a_partition_blocks_then_heals() {
        let upstream = echo();
        let proxy = Proxy::start(loopback(), upstream, Fault::Healthy).unwrap();
        // An open connection before the partition.
        let mut open = TcpStream::connect(proxy.addr()).unwrap();
        open.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        open.write_all(b"ping").unwrap();
        let mut got = [0u8; 4];
        open.read_exact(&mut got).unwrap();

        proxy.set_fault(Fault::Partition);
        // The open connection is severed: it reads end of stream.
        assert_eq!(open.read(&mut got).unwrap_or(0), 0);
        assert_eq!(exchange(proxy.addr()), None, "blocked while partitioned");
        assert!(proxy.stats().refused >= 1);

        proxy.set_fault(Fault::Healthy);
        assert_eq!(
            exchange(proxy.addr()).as_deref(),
            Some(&b"ping"[..]),
            "healed"
        );
    }
}
