//! Inbound streams (PLAN 6.1): the hub's port streams, handed by the node's relay to vsockd and on to a fork's port.
//!
//! The relay connects to vsockd's inbound listener on loopback and writes one line, `INBOUND <cid> <port>\n`, then
//! waits for the answer. vsockd asks noded whether a live run serves that port on that guest ([`PortCheck`]); when none
//! does it answers `ERR <reason>\n` and closes. When one does, it dials the guest's port ([`GuestDial`]: vsock on the
//! node), answers `OK\n`, and relays bytes both ways until either side closes. A line that is not this shape is refused
//! the same way: nothing else is accepted.
//!
//! The handshake reads one byte at a time, so no byte of the stream is read before the caller wants it.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// How long either side may take to send or answer the line.
const LINE_TIMEOUT: Duration = Duration::from_secs(5);
/// The longest line accepted, in bytes.
const MAX_LINE: usize = 64;
/// How much one relay copy reads at a time.
const CHUNK: usize = 16 << 10;

/// A duplex connection: a TCP stream to a relay, or a vsock stream to a guest.
pub trait Duplex: Read + Write + Send {
    /// Another handle to the same connection.
    fn duplicate(&self) -> io::Result<Box<dyn Duplex>>;
    /// Ends this side's sending: the peer reads end of stream, and its own sending goes on.
    fn close_write(&self) -> io::Result<()>;
}

impl Duplex for TcpStream {
    fn duplicate(&self) -> io::Result<Box<dyn Duplex>> {
        Ok(Box::new(self.try_clone()?))
    }

    fn close_write(&self) -> io::Result<()> {
        self.shutdown(Shutdown::Write)
    }
}

/// Whether a live run serves a port on a guest: vsockd's check, answered by noded.
pub trait PortCheck: Send + Sync {
    fn served(&self, cid: u64, port: u16) -> bool;
}

/// Opens a connection to a guest's port: vsock on the node, a loopback address in tests.
pub trait GuestDial: Send + Sync {
    fn dial(&self, cid: u64, port: u16) -> io::Result<Box<dyn Duplex>>;
}

/// Serves inbound lines on `listener` until it fails: each connection on its own thread.
pub fn serve(listener: TcpListener, check: Arc<dyn PortCheck>, dial: Arc<dyn GuestDial>) {
    for conn in listener.incoming() {
        let Ok(conn) = conn else {
            return;
        };
        let (check, dial) = (Arc::clone(&check), Arc::clone(&dial));
        thread::spawn(move || {
            let _ = handle(conn, check.as_ref(), dial.as_ref());
        });
    }
}

/// One connection: its line, the check, the dial, then the relay until either side closes. A refused line gets its
/// `ERR` and the connection closes.
pub fn handle(mut conn: TcpStream, check: &dyn PortCheck, dial: &dyn GuestDial) -> io::Result<()> {
    conn.set_read_timeout(Some(LINE_TIMEOUT))?;
    conn.set_write_timeout(Some(LINE_TIMEOUT))?;
    let (cid, port) = match read_line(&mut conn).and_then(|line| parse(&line)) {
        Ok(target) => target,
        Err(err) => {
            let _ = conn.write_all(format!("ERR {err}\n").as_bytes());
            return Err(err);
        }
    };
    if !check.served(cid, port) {
        let _ = conn.write_all(b"ERR no live run serves that port on that guest\n");
        return Ok(());
    }
    let guest = match dial.dial(cid, port) {
        Ok(guest) => guest,
        Err(err) => {
            let _ =
                conn.write_all(format!("ERR the guest's port does not answer: {err}\n").as_bytes());
            return Err(err);
        }
    };
    conn.write_all(b"OK\n")?;
    conn.set_read_timeout(None)?;
    conn.set_write_timeout(None)?;
    relay(Box::new(conn), guest);
    Ok(())
}

/// The relay's side of the handshake: connects to vsockd's inbound listener on loopback and asks for `cid`'s `port`.
/// Returns the connection once vsockd answers `OK`; the bytes after it are the guest's.
pub fn connect(inbound: u16, cid: u64, port: u16) -> io::Result<TcpStream> {
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], inbound));
    let mut conn = TcpStream::connect_timeout(&address, LINE_TIMEOUT)?;
    conn.set_read_timeout(Some(LINE_TIMEOUT))?;
    conn.set_write_timeout(Some(LINE_TIMEOUT))?;
    conn.write_all(format!("INBOUND {cid} {port}\n").as_bytes())?;
    let answer = read_line(&mut conn)?;
    if answer != "OK" {
        return Err(io::Error::new(io::ErrorKind::ConnectionRefused, answer));
    }
    conn.set_read_timeout(None)?;
    conn.set_write_timeout(None)?;
    Ok(conn)
}

/// Reads one line, byte by byte, without reading past its newline. Refused past [`MAX_LINE`].
fn read_line(conn: &mut impl Read) -> io::Result<String> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if conn.read(&mut byte)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the line ended early",
            ));
        }
        if byte[0] == b'\n' {
            break;
        }
        if line.len() == MAX_LINE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the line is too long",
            ));
        }
        line.push(byte[0]);
    }
    let text = String::from_utf8(line)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "the line is not text"))?;
    Ok(text.trim_end_matches('\r').to_string())
}

/// `INBOUND <cid> <port>`: the target of an inbound line. Anything else is refused.
fn parse(line: &str) -> io::Result<(u64, u16)> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "the line is not INBOUND <cid> <port>",
        )
    };
    let mut parts = line.split(' ');
    if parts.next() != Some("INBOUND") {
        return Err(invalid());
    }
    let cid = parts
        .next()
        .and_then(|cid| cid.parse::<u64>().ok())
        .ok_or_else(invalid)?;
    let port = parts
        .next()
        .and_then(|port| port.parse::<u16>().ok())
        .filter(|port| *port != 0)
        .ok_or_else(invalid)?;
    if parts.next().is_some() {
        return Err(invalid());
    }
    Ok((cid, port))
}

/// Copies both ways until each side has ended: the client's bytes go to the guest, the guest's to the client, and
/// each side's end of stream is passed on.
fn relay(client: Box<dyn Duplex>, mut guest: Box<dyn Duplex>) {
    let (Ok(mut from_client), Ok(mut to_guest)) = (client.duplicate(), guest.duplicate()) else {
        return;
    };
    let forward = thread::spawn(move || {
        pump_into(from_client.as_mut(), to_guest.as_mut());
        let _ = to_guest.close_write();
    });
    let mut to_client = client;
    pump_into(guest.as_mut(), to_client.as_mut());
    let _ = to_client.close_write();
    let _ = forward.join();
}

fn pump_into(from: &mut dyn Read, to: &mut dyn Write) {
    let mut buf = vec![0u8; CHUNK];
    loop {
        match from.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if to.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        }
    }
}

/// A check and a dial from tables, for tests and for a node with its guests on loopback.
#[derive(Default)]
pub struct Table {
    pub served: std::sync::Mutex<std::collections::BTreeSet<(u64, u16)>>,
    pub guests: BTreeMap<(u64, u16), std::net::SocketAddr>,
}

impl PortCheck for Table {
    fn served(&self, cid: u64, port: u16) -> bool {
        self.served
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&(cid, port))
    }
}

impl GuestDial for Table {
    fn dial(&self, cid: u64, port: u16) -> io::Result<Box<dyn Duplex>> {
        let address = self
            .guests
            .get(&(cid, port))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no guest port"))?;
        Ok(Box::new(TcpStream::connect(address)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    /// A guest port on loopback that echoes what it reads, then closes its side once the client does.
    fn guest_port() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
        let address = listener.local_addr().expect("address");
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { return };
                thread::spawn(move || {
                    let mut buf = [0u8; 1024];
                    loop {
                        match conn.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if conn.write_all(&buf[..n]).is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    let _ = conn.shutdown(Shutdown::Write);
                });
            }
        });
        address
    }

    /// vsockd's listener on loopback, serving `table`.
    fn inbound(table: Table) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
        let port = listener.local_addr().expect("address").port();
        let table = Arc::new(table);
        thread::spawn(move || serve(listener, table.clone(), table));
        port
    }

    #[test]
    fn a_served_port_carries_bytes_both_ways_once_vsockd_answers_ok() {
        let guest = guest_port();
        let mut table = Table::default();
        table.served.lock().unwrap().insert((1000, 7681));
        table.guests.insert((1000, 7681), guest);
        let port = inbound(table);
        let mut conn = connect(port, 1000, 7681).expect("the port is served");
        conn.write_all(b"ls -la\n").expect("sends");
        conn.shutdown(Shutdown::Write).expect("ends its side");
        let mut echoed = Vec::new();
        conn.read_to_end(&mut echoed)
            .expect("reads the echo to its end");
        assert_eq!(echoed, b"ls -la\n");
    }

    #[test]
    fn an_unserved_port_is_refused_and_so_is_a_malformed_line() {
        let guest = guest_port();
        let mut table = Table::default();
        table.served.lock().unwrap().insert((1000, 7681));
        table.guests.insert((1000, 7681), guest);
        table.guests.insert((1000, 9999), guest);
        let port = inbound(table);
        let refused = connect(port, 1000, 9999).expect_err("registered on no run: refused");
        assert!(refused.to_string().contains("no live run"), "{refused}");
        let refused = connect(port, 1001, 7681).expect_err("another guest's port: refused");
        assert!(refused.to_string().contains("no live run"), "{refused}");
        // A line the protocol does not know is refused without any dial.
        let mut raw = TcpStream::connect(("127.0.0.1", port)).expect("connects");
        raw.write_all(b"HELLO 1000 7681\n").expect("sends");
        let mut answer = String::new();
        raw.read_to_string(&mut answer).expect("reads");
        assert!(answer.starts_with("ERR "), "{answer}");
    }

    #[test]
    fn the_line_parses_only_as_inbound_cid_port() {
        assert_eq!(parse("INBOUND 1000 7681").unwrap(), (1000, 7681));
        for bad in [
            "",
            "INBOUND",
            "INBOUND x 1",
            "INBOUND 1000 0",
            "INBOUND 1000 70000",
            "INBOUND 1 2 3",
            "inbound 1 2",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} is refused");
        }
    }

    #[test]
    fn an_overlong_line_is_refused() {
        let long = format!("INBOUND 1000 {}\n", "7".repeat(MAX_LINE));
        let mut cursor = io::Cursor::new(long.into_bytes());
        assert!(read_line(&mut cursor).is_err());
    }

    #[test]
    fn a_guest_that_does_not_answer_is_reported_to_the_relay() {
        let table = Table::default();
        table.served.lock().unwrap().insert((1000, 7681));
        // No guest port is registered for it, so the dial fails.
        let port = inbound(table);
        let err = connect(port, 1000, 7681).expect_err("the guest does not answer");
        assert!(err.to_string().contains("does not answer"), "{err}");
    }
}
