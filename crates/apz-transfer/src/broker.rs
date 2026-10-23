//! The brokered path (plan 17.5). A node that cannot dial another node dials the hub's broker, says a ticket the hub
//! issued for the sender (`BROKER <ticket>`), and the hub opens a stream over the sender's tunnel and pipes the bytes
//! both ways. The TLS session is the receiver's and the sender's: the hub relays bytes it cannot read, and counts them.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use apz_http::mux::{Event, Stream};

/// The line a brokered connection starts with: the prefix and the ticket.
pub const PREFIX: &str = "BROKER ";
const LINE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_LINE: usize = 128;
const POLL: Duration = Duration::from_millis(500);
const CHUNK: usize = 16 << 10;

/// Opens a tunnel stream to a node by its id (the hub's `nodes::Store::tunnel`).
pub type Open = Arc<dyn Fn(&str) -> io::Result<Stream> + Send + Sync>;

struct Ticket {
    sender: String,
    expires_ms: u64,
    bytes: Arc<AtomicU64>,
}

/// The tickets the hub issued: which sender each reaches, until when, and the bytes brokered on each.
pub struct Tickets {
    map: Mutex<BTreeMap<String, Ticket>>,
    total: Arc<AtomicU64>,
}

impl Tickets {
    /// Tickets whose bytes also count into `total` (the hub's brokered bytes).
    pub fn new(total: Arc<AtomicU64>) -> Tickets {
        Tickets {
            map: Mutex::new(BTreeMap::new()),
            total,
        }
    }

    /// A new ticket for `sender`, valid until `expires_ms`.
    pub fn issue(&self, sender: &str, expires_ms: u64) -> String {
        let ticket = apz_trust::b64(&apz_trust::random::<16>());
        self.map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                ticket.clone(),
                Ticket {
                    sender: sender.to_string(),
                    expires_ms,
                    bytes: Arc::new(AtomicU64::new(0)),
                },
            );
        ticket
    }

    /// The bytes brokered on `ticket` so far.
    pub fn bytes(&self, ticket: &str) -> u64 {
        self.map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(ticket)
            .map_or(0, |t| t.bytes.load(Ordering::Relaxed))
    }

    /// The bytes brokered by every ticket.
    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    /// The sender and the byte counter of a live ticket at `now_ms`.
    fn live(&self, ticket: &str, now_ms: u64) -> Option<(String, Arc<AtomicU64>)> {
        let map = self.map.lock().unwrap_or_else(PoisonError::into_inner);
        let t = map.get(ticket)?;
        (now_ms < t.expires_ms).then(|| (t.sender.clone(), Arc::clone(&t.bytes)))
    }
}

/// Serves brokered connections on `listener` in a thread: each one that says a live ticket is piped to its sender.
pub fn serve(listener: TcpListener, tickets: Arc<Tickets>, open: Open) {
    thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let (tickets, open) = (Arc::clone(&tickets), Arc::clone(&open));
            thread::spawn(move || {
                let _ = handle(conn, &tickets, &open);
            });
        }
    });
}

fn handle(mut conn: TcpStream, tickets: &Tickets, open: &Open) -> io::Result<()> {
    conn.set_read_timeout(Some(LINE_TIMEOUT))?;
    let line = read_line(&mut conn)?;
    let Some(ticket) = line.strip_prefix(PREFIX) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a broker line",
        ));
    };
    let now = crate::now_ms();
    let Some((sender, bytes)) = tickets.live(ticket, now) else {
        conn.write_all(b"ERR\n")?;
        return Ok(());
    };
    let stream = open(&sender)?;
    conn.set_read_timeout(None)?;
    let total = Arc::clone(&tickets.total);
    let count: Arc<dyn Fn(u64) + Send + Sync> = Arc::new(move |n| {
        bytes.fetch_add(n, Ordering::Relaxed);
        total.fetch_add(n, Ordering::Relaxed);
    });
    pipe(stream, conn, count);
    Ok(())
}

/// The first line, read a byte at a time so nothing after it is taken.
fn read_line(conn: &mut TcpStream) -> io::Result<String> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if conn.read(&mut byte)? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        if byte[0] == b'\n' {
            break;
        }
        line.push(byte[0]);
        if line.len() > MAX_LINE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the line is too long",
            ));
        }
    }
    String::from_utf8(line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Carries bytes both ways between a tunnel stream and a TCP connection until either side closes. `count` is told how
/// many bytes move, in either direction.
pub fn pipe(stream: Stream, conn: TcpStream, count: Arc<dyn Fn(u64) + Send + Sync>) {
    let Ok(mut from_conn) = conn.try_clone() else {
        stream.reset("the connection could not be read");
        return;
    };
    let stream = Arc::new(stream);
    let outgoing = Arc::clone(&stream);
    let counted = Arc::clone(&count);
    thread::spawn(move || {
        let mut buf = vec![0u8; CHUNK];
        loop {
            match from_conn.read(&mut buf) {
                Ok(0) | Err(_) => {
                    let _ = outgoing.finish();
                    break;
                }
                Ok(n) => {
                    counted(n as u64);
                    if outgoing.send(&buf[..n]).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let mut to_conn = conn;
    loop {
        match stream.recv(POLL) {
            Some(Event::Data(bytes)) => {
                count(bytes.len() as u64);
                if to_conn.write_all(&bytes).is_err() {
                    stream.reset("the connection closed");
                    break;
                }
            }
            Some(Event::Fin) => {
                let _ = to_conn.shutdown(Shutdown::Write);
            }
            Some(Event::Reset(_) | Event::Closed) => {
                let _ = to_conn.shutdown(Shutdown::Both);
                break;
            }
            None => {}
        }
    }
}
