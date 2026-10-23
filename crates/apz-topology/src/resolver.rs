//! A topology's name service (PLAN 22.6): a service name resolves to its replicas' addresses, in the lab VM and in the
//! process runner alike.
//!
//! [`Table`] maps names to addresses: each service name to all its replicas, each container name to its own. [`serve`]
//! answers A queries for them over UDP in the DNS wire format (RFC 1035), so a process that asks the runner's resolver
//! gets the replicas back; a name the table lacks is NXDOMAIN. [`lookup`] is the client side, which the stand-in
//! services of the tests use to reach each other by name.

use std::collections::BTreeMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

const TYPE_A: u16 = 1;
const CLASS_IN: u16 = 1;
/// Seconds an answer may be kept: short, since replicas come and go.
const TTL: u32 = 5;
const RCODE_NXDOMAIN: u8 = 3;
const RCODE_NOTIMP: u8 = 4;
const FLAG_RESPONSE: u16 = 0x8000;
const FLAG_AUTHORITATIVE: u16 = 0x0400;
const FLAG_RECURSION_DESIRED: u16 = 0x0100;

/// The names a resolver holds, each with the addresses it resolves to (lowercase names).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Table {
    names: BTreeMap<String, Vec<Ipv4Addr>>,
}

impl Table {
    /// Adds `addr` to `name`'s addresses (once).
    pub fn add(&mut self, name: &str, addr: Ipv4Addr) {
        let addrs = self.names.entry(name.to_ascii_lowercase()).or_default();
        if !addrs.contains(&addr) {
            addrs.push(addr);
        }
    }

    /// The addresses of `name`, if the table holds it.
    pub fn resolve(&self, name: &str) -> Option<&[Ipv4Addr]> {
        self.names
            .get(&name.trim_end_matches('.').to_ascii_lowercase())
            .map(Vec::as_slice)
    }
}

/// The reply to one query (pure): an A answer with the addresses, an empty NOERROR for another type of a known name,
/// NXDOMAIN for an unknown name. None for bytes that are no query this server answers.
pub fn answer(table: &Table, query: &[u8]) -> Option<Vec<u8>> {
    if query.len() < 12 {
        return None;
    }
    let flags = u16::from_be_bytes([query[2], query[3]]);
    let is_query = flags & FLAG_RESPONSE == 0 && (flags >> 11) & 0xf == 0;
    if !is_query || u16::from_be_bytes([query[4], query[5]]) != 1 {
        return None;
    }
    let (name, after_name) = read_name(query, 12)?;
    if query.len() < after_name + 4 {
        return None;
    }
    let qtype = u16::from_be_bytes([query[after_name], query[after_name + 1]]);
    let qclass = u16::from_be_bytes([query[after_name + 2], query[after_name + 3]]);
    let question = &query[12..after_name + 4];

    let mut rcode = 0u8;
    let mut addrs: Vec<Ipv4Addr> = Vec::new();
    if qclass != CLASS_IN {
        rcode = RCODE_NOTIMP;
    } else {
        match table.resolve(&name) {
            Some(found) => {
                if qtype == TYPE_A {
                    addrs = found.to_vec();
                }
            }
            None => rcode = RCODE_NXDOMAIN,
        }
    }

    let mut out = Vec::with_capacity(12 + question.len() + addrs.len() * 16);
    out.extend_from_slice(&query[0..2]);
    let out_flags =
        FLAG_RESPONSE | FLAG_AUTHORITATIVE | (flags & FLAG_RECURSION_DESIRED) | u16::from(rcode);
    out.extend_from_slice(&out_flags.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&(addrs.len() as u16).to_be_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(question);
    for addr in addrs {
        // A pointer to the question's name, at offset 12.
        out.extend_from_slice(&[0xc0, 0x0c]);
        out.extend_from_slice(&TYPE_A.to_be_bytes());
        out.extend_from_slice(&CLASS_IN.to_be_bytes());
        out.extend_from_slice(&TTL.to_be_bytes());
        out.extend_from_slice(&4u16.to_be_bytes());
        out.extend_from_slice(&addr.octets());
    }
    Some(out)
}

/// The name at `start` in a DNS message, lowercased and dotted, with the offset after it (pointers followed; the offset
/// is the one after the first pointer when there is one).
pub fn read_name(buf: &[u8], start: usize) -> Option<(String, usize)> {
    let mut labels = Vec::new();
    let mut pos = start;
    let mut end = None;
    let mut jumps = 0;
    loop {
        let len = *buf.get(pos)?;
        if len & 0xc0 == 0xc0 {
            let second = *buf.get(pos + 1)?;
            end.get_or_insert(pos + 2);
            jumps += 1;
            if jumps > 16 {
                return None;
            }
            pos = (usize::from(len & 0x3f) << 8) | usize::from(second);
            continue;
        }
        if len & 0xc0 != 0 {
            return None;
        }
        if len == 0 {
            pos += 1;
            break;
        }
        let label = buf.get(pos + 1..pos + 1 + usize::from(len))?;
        labels.push(String::from_utf8_lossy(label).to_ascii_lowercase());
        pos += 1 + usize::from(len);
    }
    Some((labels.join("."), end.unwrap_or(pos)))
}

/// The query for `name`'s A records, with `id` (pure). None for a name with an empty or over-long label.
pub fn query(id: u16, name: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&FLAG_RECURSION_DESIRED.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&[0; 6]);
    for label in name.trim_end_matches('.').split('.') {
        if label.is_empty() || label.len() > 63 {
            return None;
        }
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out.extend_from_slice(&TYPE_A.to_be_bytes());
    out.extend_from_slice(&CLASS_IN.to_be_bytes());
    Some(out)
}

/// The addresses in a reply to query `id` (pure): an error for NXDOMAIN, another rcode, or a reply that does not parse.
pub fn parse_reply(buf: &[u8], id: u16) -> Result<Vec<Ipv4Addr>, String> {
    let bad = || "a malformed reply".to_string();
    if buf.len() < 12 || buf[0..2] != id.to_be_bytes() {
        return Err(bad());
    }
    let flags = u16::from_be_bytes([buf[2], buf[3]]);
    if flags & FLAG_RESPONSE == 0 {
        return Err(bad());
    }
    match (flags & 0xf) as u8 {
        0 => {}
        RCODE_NXDOMAIN => return Err("no such name".into()),
        code => return Err(format!("rcode {code}")),
    }
    let questions = u16::from_be_bytes([buf[4], buf[5]]);
    let answers = u16::from_be_bytes([buf[6], buf[7]]);
    let mut pos = 12;
    for _ in 0..questions {
        let (_, after) = read_name(buf, pos).ok_or_else(bad)?;
        pos = after + 4;
    }
    let mut addrs = Vec::new();
    for _ in 0..answers {
        let (_, after) = read_name(buf, pos).ok_or_else(bad)?;
        let fixed = buf.get(after..after + 10).ok_or_else(bad)?;
        let kind = u16::from_be_bytes([fixed[0], fixed[1]]);
        let len = usize::from(u16::from_be_bytes([fixed[8], fixed[9]]));
        let data = buf.get(after + 10..after + 10 + len).ok_or_else(bad)?;
        if kind == TYPE_A && len == 4 {
            addrs.push(Ipv4Addr::new(data[0], data[1], data[2], data[3]));
        }
        pos = after + 10 + len;
    }
    Ok(addrs)
}

/// A running resolver: its address, and the thread that answers until it is dropped.
pub struct Server {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Server {
    /// The address the resolver listens on (loopback, an ephemeral port).
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Serves `table` on a loopback UDP port.
pub fn serve(table: Table) -> io::Result<Server> {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    socket.set_read_timeout(Some(Duration::from_millis(50)))?;
    let addr = socket.local_addr()?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let thread = thread::spawn(move || {
        let mut buf = [0u8; 512];
        while !flag.load(Ordering::Relaxed) {
            match socket.recv_from(&mut buf) {
                Ok((n, peer)) => {
                    if let Some(reply) = answer(&table, &buf[..n]) {
                        let _ = socket.send_to(&reply, peer);
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => break,
            }
        }
    });
    Ok(Server {
        addr,
        stop,
        thread: Some(thread),
    })
}

/// The addresses `name` resolves to at the resolver `server`, asked with `timeout` for its reply.
pub fn lookup(server: SocketAddr, name: &str, timeout: Duration) -> Result<Vec<Ipv4Addr>, String> {
    const ID: u16 = 0x2a2a;
    let question = query(ID, name).ok_or_else(|| format!("{name:?} is no DNS name"))?;
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
    socket
        .set_read_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    socket
        .send_to(&question, server)
        .map_err(|e| e.to_string())?;
    let mut buf = [0u8; 512];
    let (n, _) = socket
        .recv_from(&mut buf)
        .map_err(|e| format!("no reply for {name}: {e}"))?;
    parse_reply(&buf[..n], ID)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Table {
        let mut table = Table::default();
        table.add("api", Ipv4Addr::new(10, 88, 0, 2));
        table.add("api", Ipv4Addr::new(10, 88, 0, 3));
        table.add("api-1", Ipv4Addr::new(10, 88, 0, 2));
        table.add("db", Ipv4Addr::new(10, 88, 0, 4));
        table
    }

    #[test]
    fn a_service_name_answers_with_all_its_replicas() {
        let table = table();
        let reply = answer(&table, &query(7, "API").unwrap()).unwrap();
        assert_eq!(
            parse_reply(&reply, 7).unwrap(),
            [Ipv4Addr::new(10, 88, 0, 2), Ipv4Addr::new(10, 88, 0, 3)]
        );
        // Only the matching id is accepted.
        assert!(parse_reply(&reply, 8).is_err());
    }

    #[test]
    fn an_unknown_name_is_nxdomain_and_another_type_is_an_empty_answer() {
        let table = table();
        let reply = answer(&table, &query(1, "nope").unwrap()).unwrap();
        assert_eq!(parse_reply(&reply, 1).unwrap_err(), "no such name");
        // An AAAA query for a known name: NOERROR with no A records.
        let mut aaaa = query(2, "db").unwrap();
        let qtype_at = aaaa.len() - 4;
        aaaa[qtype_at + 1] = 28;
        let reply = answer(&table, &aaaa).unwrap();
        assert_eq!(parse_reply(&reply, 2).unwrap(), Vec::<Ipv4Addr>::new());
    }

    #[test]
    fn names_read_through_pointers_and_bad_messages_are_refused() {
        // A query whose question name is followed by a pointer back to it.
        let mut msg = vec![0u8; 12];
        msg.extend_from_slice(&[2, b'd', b'b', 0]);
        assert_eq!(read_name(&msg, 12), Some(("db".to_string(), 16)));
        msg.extend_from_slice(&[0xc0, 12]);
        assert_eq!(read_name(&msg, 16), Some(("db".to_string(), 18)));
        assert!(answer(&table(), &[0u8; 5]).is_none());
        assert!(query(1, "a..b").is_none());
    }

    #[test]
    fn the_server_answers_lookups_over_udp() {
        let server = serve(table()).unwrap();
        let timeout = Duration::from_secs(2);
        assert_eq!(
            lookup(server.addr(), "db", timeout).unwrap(),
            [Ipv4Addr::new(10, 88, 0, 4)]
        );
        assert_eq!(
            lookup(server.addr(), "missing", timeout).unwrap_err(),
            "no such name"
        );
    }
}
