// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port (PLAN 22.6, W58, round 2). The gateway is the segment's one way out, and the
// switch lets it carry only TCP to the egress proxy's address and port. Frames arrive as bytes, so the
// gateway terminates TCP itself: a minimal endpoint (the handshake, in-order data acknowledged as it
// arrives, the guest's window honoured for what the gateway sends, and FIN in both directions) that
// hands each accepted connection to the owner as an ordinary byte stream. The owner runs the egress
// proxy's own connection handling on that stream, the code vsock egress uses.
//
// The endpoint does not retransmit. The segment is a lossless in-process link, so a segment is
// either acknowledged on arrival or the guest is not talking to a working gateway; a guest that
// stops acknowledging stalls its own connection, and nothing else.

//! The egress gateway of a lab segment: TCP over frames, one [`Flow`] per proxied connection.

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Read, Write};
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use super::switch::{Endpoint, PROXY_PORT, Port, Switch};
use super::{ETH_HDR_LEN, Mac, NetError};

/// The largest payload one segment carries (an Ethernet MTU less the headers).
const MSS: usize = 1460;
/// The window the gateway advertises: what it can take from a guest, which it reads as fast as the proxy does.
const WINDOW: u16 = 65535;
/// How long the endpoint waits for a frame before it looks at the owner's commands.
const POLL: Duration = Duration::from_millis(1);

/// TCP flags, as a segment carries them.
pub const FIN: u8 = 0x01;
pub const SYN: u8 = 0x02;
pub const RST: u8 = 0x04;
pub const PSH: u8 = 0x08;
pub const ACK: u8 = 0x10;

const ETHERTYPE_IPV4: [u8; 2] = [0x08, 0x00];
const IP_PROTO_TCP: u8 = 6;

type Key = (Ipv4Addr, u16);

/// What the owner asks of the endpoint, from its [`Flow`]s and the [`Gateway`].
enum Cmd {
    Data(Key, Vec<u8>),
    Close(Key),
    Stop,
}

/// One proxied connection, as the owner reads and writes it. Reading gives what the guest sent, and
/// the end of the stream when the guest closes; writing sends to the guest. Dropping it closes the
/// connection after what was written has gone out.
pub struct Flow {
    key: Key,
    rx: Receiver<Vec<u8>>,
    cmd: Sender<Cmd>,
    pending: Vec<u8>,
    /// Set once the flow is split: the halves close the connection, not this value.
    split: bool,
}

/// The reading half of a split [`Flow`]: what the guest sent, and the end of it when the guest closes.
pub struct FlowRead {
    rx: Receiver<Vec<u8>>,
    pending: Vec<u8>,
}

/// The writing half of a split [`Flow`]. Dropping it closes the connection's owner side, after what was
/// written has gone out.
pub struct FlowWrite {
    key: Key,
    cmd: Sender<Cmd>,
}

impl Flow {
    /// Splits the flow into its reading and writing halves, so two threads can each own one.
    pub fn split(mut self) -> (FlowRead, FlowWrite) {
        self.split = true;
        // The flow's receiver moves to the reading half; a placeholder stands in its place.
        let (_, placeholder) = mpsc::channel();
        let rx = std::mem::replace(&mut self.rx, placeholder);
        (
            FlowRead {
                rx,
                pending: std::mem::take(&mut self.pending),
            },
            FlowWrite {
                key: self.key,
                cmd: self.cmd.clone(),
            },
        )
    }
}

impl Read for FlowRead {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        read_from(&self.rx, &mut self.pending, buf)
    }
}

impl Write for FlowWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        write_to(&self.cmd, self.key, buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for FlowWrite {
    fn drop(&mut self) {
        self.cmd.send(Cmd::Close(self.key)).ok();
    }
}

fn read_from(rx: &Receiver<Vec<u8>>, pending: &mut Vec<u8>, buf: &mut [u8]) -> io::Result<usize> {
    if pending.is_empty() {
        match rx.recv() {
            Ok(bytes) => *pending = bytes,
            // The guest closed its side (or the endpoint stopped): the end of the stream.
            Err(_) => return Ok(0),
        }
    }
    let n = buf.len().min(pending.len());
    buf[..n].copy_from_slice(&pending[..n]);
    pending.drain(..n);
    Ok(n)
}

fn write_to(cmd: &Sender<Cmd>, key: Key, buf: &[u8]) -> io::Result<usize> {
    if buf.is_empty() {
        return Ok(0);
    }
    cmd.send(Cmd::Data(key, buf.to_vec()))
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the gateway stopped"))?;
    Ok(buf.len())
}

impl Read for Flow {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        read_from(&self.rx, &mut self.pending, buf)
    }
}

impl Write for Flow {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        write_to(&self.cmd, self.key, buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Flow {
    fn drop(&mut self) {
        if !self.split {
            self.cmd.send(Cmd::Close(self.key)).ok();
        }
    }
}

/// The gateway's handle: dropping it stops the endpoint and waits until it has detached from the switch, so
/// the segment can take a gateway again at once.
pub struct Gateway {
    cmd: Sender<Cmd>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Drop for Gateway {
    fn drop(&mut self) {
        self.cmd.send(Cmd::Stop).ok();
        if let Some(thread) = self.thread.take() {
            thread.join().ok();
        }
    }
}

/// One connection's state in the endpoint.
struct Conn {
    guest_mac: Mac,
    /// Established once the guest's ACK of our SYN-ACK arrived.
    open: bool,
    rcv_nxt: u32,
    snd_nxt: u32,
    snd_una: u32,
    /// The guest's advertised window: how much we may have in flight.
    window: u32,
    /// Bytes from the owner not yet sent.
    out: VecDeque<u8>,
    close_queued: bool,
    fin_sent: bool,
    remote_fin: bool,
    /// Delivers the guest's bytes to the flow; `None` once the guest has closed its side.
    to_flow: Option<Sender<Vec<u8>>>,
    /// The flow's receiving end, handed to the owner when the connection opens.
    from_guest: Option<Receiver<Vec<u8>>>,
}

/// What a parsed TCP segment carries.
pub struct Segment<'a> {
    pub src_mac: Mac,
    pub src_ip: Ipv4Addr,
    pub src_port: u16,
    pub dst_ip: Ipv4Addr,
    pub dst_port: u16,
    pub seq: u32,
    pub ack: u32,
    pub flags: u8,
    pub window: u16,
    pub payload: &'a [u8],
}

/// A TCP segment from a frame (IPv4 and TCP only, no options read), or `None` for anything else.
pub fn parse(frame: &[u8]) -> Option<Segment<'_>> {
    if frame.len() < ETH_HDR_LEN + 40 || frame[12..14] != ETHERTYPE_IPV4 {
        return None;
    }
    let ip = &frame[ETH_HDR_LEN..];
    if ip[0] >> 4 != 4 {
        return None;
    }
    let ihl = usize::from(ip[0] & 0x0f) * 4;
    let total = usize::from(u16::from_be_bytes([ip[2], ip[3]]));
    if ihl < 20 || total < ihl + 20 || total > ip.len() || ip[9] != IP_PROTO_TCP {
        return None;
    }
    let tcp = &ip[ihl..total];
    let data_offset = usize::from(tcp[12] >> 4) * 4;
    if data_offset < 20 || data_offset > tcp.len() {
        return None;
    }
    Some(Segment {
        src_mac: [frame[6], frame[7], frame[8], frame[9], frame[10], frame[11]],
        src_ip: Ipv4Addr::new(ip[12], ip[13], ip[14], ip[15]),
        dst_ip: Ipv4Addr::new(ip[16], ip[17], ip[18], ip[19]),
        src_port: u16::from_be_bytes([tcp[0], tcp[1]]),
        dst_port: u16::from_be_bytes([tcp[2], tcp[3]]),
        seq: u32::from_be_bytes([tcp[4], tcp[5], tcp[6], tcp[7]]),
        ack: u32::from_be_bytes([tcp[8], tcp[9], tcp[10], tcp[11]]),
        flags: tcp[13],
        window: u16::from_be_bytes([tcp[14], tcp[15]]),
        payload: &tcp[data_offset..],
    })
}

/// The ones' complement sum of `bytes` (the IPv4 and TCP checksum).
fn checksum(parts: &[&[u8]]) -> u16 {
    let mut sum: u32 = 0;
    for part in parts {
        for chunk in part.chunks(2) {
            let word = if chunk.len() == 2 {
                u16::from_be_bytes([chunk[0], chunk[1]])
            } else {
                u16::from_be_bytes([chunk[0], 0])
            };
            sum += u32::from(word);
        }
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// One Ethernet frame carrying a TCP segment from `src` to `dst`, with the given sequence numbers,
/// flags, window and payload.
pub fn frame_for(
    (src_mac, dst_mac): (Mac, Mac),
    (src_ip, dst_ip): (Ipv4Addr, Ipv4Addr),
    (src_port, dst_port): (u16, u16),
    (seq, ack, flags, window): (u32, u32, u8, u16),
    payload: &[u8],
    ip_id: u16,
) -> Vec<u8> {
    let total = (20 + 20 + payload.len()) as u16;
    let mut ip = [0u8; 20];
    ip[0] = 0x45;
    ip[2..4].copy_from_slice(&total.to_be_bytes());
    ip[4..6].copy_from_slice(&ip_id.to_be_bytes());
    ip[6..8].copy_from_slice(&0x4000u16.to_be_bytes());
    ip[8] = 64;
    ip[9] = IP_PROTO_TCP;
    ip[12..16].copy_from_slice(&src_ip.octets());
    ip[16..20].copy_from_slice(&dst_ip.octets());
    let ip_sum = checksum(&[&ip]);
    ip[10..12].copy_from_slice(&ip_sum.to_be_bytes());

    let mut tcp = [0u8; 20];
    tcp[0..2].copy_from_slice(&src_port.to_be_bytes());
    tcp[2..4].copy_from_slice(&dst_port.to_be_bytes());
    tcp[4..8].copy_from_slice(&seq.to_be_bytes());
    tcp[8..12].copy_from_slice(&ack.to_be_bytes());
    tcp[12] = 5 << 4;
    tcp[13] = flags;
    tcp[14..16].copy_from_slice(&window.to_be_bytes());
    let tcp_len = (20 + payload.len()) as u16;
    let mut pseudo = [0u8; 12];
    pseudo[0..4].copy_from_slice(&src_ip.octets());
    pseudo[4..8].copy_from_slice(&dst_ip.octets());
    pseudo[9] = IP_PROTO_TCP;
    pseudo[10..12].copy_from_slice(&tcp_len.to_be_bytes());
    let tcp_sum = checksum(&[&pseudo, &tcp, payload]);
    tcp[16..18].copy_from_slice(&tcp_sum.to_be_bytes());

    let mut out = Vec::with_capacity(ETH_HDR_LEN + usize::from(total));
    out.extend_from_slice(&dst_mac);
    out.extend_from_slice(&src_mac);
    out.extend_from_slice(&ETHERTYPE_IPV4);
    out.extend_from_slice(&ip);
    out.extend_from_slice(&tcp);
    out.extend_from_slice(payload);
    out
}

/// The endpoint's state: its port, its address, its connections and the owner's callback.
struct Stack {
    port: Port,
    mac: Mac,
    ip: Ipv4Addr,
    conns: BTreeMap<Key, Conn>,
    cmd_tx: Sender<Cmd>,
    isn: u32,
    ip_id: u16,
    on_connect: Box<dyn FnMut(Flow) + Send>,
}

impl Stack {
    fn send(&mut self, to: Key, guest_mac: Mac, seq: u32, ack: u32, flags: u8, payload: &[u8]) {
        self.ip_id = self.ip_id.wrapping_add(1);
        let frame = frame_for(
            (self.mac, guest_mac),
            (self.ip, to.0),
            (PROXY_PORT, to.1),
            (seq, ack, flags, WINDOW),
            payload,
            self.ip_id,
        );
        self.port.send(&frame).ok();
    }

    /// A segment that is not for any connection of ours: reset the guest.
    fn reset(&mut self, seg: &Segment<'_>) {
        let key = (seg.src_ip, seg.src_port);
        self.send(key, seg.src_mac, seg.ack, 0, RST, &[]);
    }

    fn frame(&mut self, frame: &[u8]) {
        let Some(seg) = parse(frame) else { return };
        if seg.dst_port != PROXY_PORT || seg.dst_ip != self.ip {
            return;
        }
        let key = (seg.src_ip, seg.src_port);
        if seg.flags & RST != 0 {
            self.conns.remove(&key);
            return;
        }
        if seg.flags & SYN != 0 && seg.flags & ACK == 0 {
            // A new connection (or the guest's SYN again, which gets the SYN-ACK again).
            if let Some(conn) = self.conns.get(&key) {
                let (isn, rcv) = (conn.snd_una, conn.rcv_nxt);
                self.send(key, seg.src_mac, isn, rcv, SYN | ACK, &[]);
                return;
            }
            let isn = self.isn;
            self.isn = self.isn.wrapping_add(0x10000);
            let (to_flow, from_guest) = mpsc::channel();
            self.conns.insert(
                key,
                Conn {
                    guest_mac: seg.src_mac,
                    open: false,
                    rcv_nxt: seg.seq.wrapping_add(1),
                    snd_nxt: isn.wrapping_add(1),
                    snd_una: isn,
                    window: u32::from(seg.window),
                    out: VecDeque::new(),
                    close_queued: false,
                    fin_sent: false,
                    remote_fin: false,
                    to_flow: Some(to_flow),
                    from_guest: Some(from_guest),
                },
            );
            let rcv = seg.seq.wrapping_add(1);
            self.send(key, seg.src_mac, isn, rcv, SYN | ACK, &[]);
            return;
        }
        if !self.conns.contains_key(&key) {
            self.reset(&seg);
            return;
        }
        let mut open_now = None;
        let mut reply = None;
        {
            let conn = self.conns.get_mut(&key).expect("checked above");
            if seg.flags & ACK != 0 {
                if !conn.open && seg.ack == conn.snd_una.wrapping_add(1) {
                    conn.open = true;
                    conn.snd_una = seg.ack;
                    conn.window = u32::from(seg.window);
                    open_now = conn.from_guest.take();
                } else if conn.open
                    && seq_after(seg.ack, conn.snd_una)
                    && !seq_after(seg.ack, conn.snd_nxt)
                {
                    conn.snd_una = seg.ack;
                }
                conn.window = u32::from(seg.window);
            }
            if conn.open && !seg.payload.is_empty() {
                if seg.seq == conn.rcv_nxt {
                    conn.rcv_nxt = conn.rcv_nxt.wrapping_add(seg.payload.len() as u32);
                    if let Some(to_flow) = &conn.to_flow {
                        to_flow.send(seg.payload.to_vec()).ok();
                    }
                }
                // Acknowledge what we have, in order or not: a repeated ACK tells the guest where we are.
                reply = Some(conn.rcv_nxt);
            }
            if conn.open && seg.flags & FIN != 0 && !conn.remote_fin {
                let fin_at = seg.seq.wrapping_add(seg.payload.len() as u32);
                if fin_at == conn.rcv_nxt {
                    conn.rcv_nxt = conn.rcv_nxt.wrapping_add(1);
                    conn.remote_fin = true;
                    conn.to_flow = None;
                    reply = Some(conn.rcv_nxt);
                }
            }
        }
        let guest_mac = self
            .conns
            .get(&key)
            .map(|conn| conn.guest_mac)
            .unwrap_or([0; 6]);
        if let Some(ack) = reply {
            let seq = self.conns.get(&key).map(|conn| conn.snd_nxt).unwrap_or(0);
            self.send(key, guest_mac, seq, ack, ACK, &[]);
        }
        if let Some(from_guest) = open_now {
            let flow = Flow {
                key,
                rx: from_guest,
                cmd: self.cmd_tx.clone(),
                pending: Vec::new(),
                split: false,
            };
            (self.on_connect)(flow);
        }
        self.pump(key);
        self.finish(key);
    }

    /// The owner's command. `false` when the endpoint is to stop.
    fn command(&mut self, cmd: Cmd) -> bool {
        let key = match cmd {
            Cmd::Stop => return false,
            Cmd::Data(key, bytes) => {
                if let Some(conn) = self.conns.get_mut(&key) {
                    conn.out.extend(bytes);
                }
                key
            }
            Cmd::Close(key) => {
                if let Some(conn) = self.conns.get_mut(&key) {
                    conn.close_queued = true;
                }
                key
            }
        };
        self.pump(key);
        self.finish(key);
        true
    }

    /// Sends what the guest's window allows, then the FIN if the owner closed its side and all is out.
    fn pump(&mut self, key: Key) {
        loop {
            let Some(conn) = self.conns.get(&key) else {
                return;
            };
            if !conn.open {
                return;
            }
            let in_flight = conn.snd_nxt.wrapping_sub(conn.snd_una);
            let room = conn.window.saturating_sub(in_flight) as usize;
            if !conn.out.is_empty() && room > 0 {
                let n = MSS.min(room).min(conn.out.len());
                let payload: Vec<u8> = self
                    .conns
                    .get_mut(&key)
                    .expect("present")
                    .out
                    .drain(..n)
                    .collect();
                let (seq, ack, mac) = {
                    let conn = self.conns.get_mut(&key).expect("present");
                    let seq = conn.snd_nxt;
                    conn.snd_nxt = conn.snd_nxt.wrapping_add(n as u32);
                    (seq, conn.rcv_nxt, conn.guest_mac)
                };
                self.send(key, mac, seq, ack, ACK | PSH, &payload);
                continue;
            }
            if conn.out.is_empty() && conn.close_queued && !conn.fin_sent {
                let (seq, ack, mac) = {
                    let conn = self.conns.get_mut(&key).expect("present");
                    let seq = conn.snd_nxt;
                    conn.snd_nxt = conn.snd_nxt.wrapping_add(1);
                    conn.fin_sent = true;
                    (seq, conn.rcv_nxt, conn.guest_mac)
                };
                self.send(key, mac, seq, ack, FIN | ACK, &[]);
            }
            return;
        }
    }

    /// Forgets a connection once both sides have closed and our FIN is acknowledged.
    fn finish(&mut self, key: Key) {
        let done = self
            .conns
            .get(&key)
            .is_some_and(|conn| conn.fin_sent && conn.remote_fin && conn.snd_una == conn.snd_nxt);
        if done {
            self.conns.remove(&key);
        }
    }
}

/// Whether `a` is after `b` in sequence space.
fn seq_after(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

/// Serves the gateway: owns the port, handles the frames and the owner's commands, until stopped.
fn run(
    port: Port,
    frames: Receiver<Vec<u8>>,
    cmds: Receiver<Cmd>,
    cmd_tx: Sender<Cmd>,
    mac: Mac,
    ip: Ipv4Addr,
    on_connect: Box<dyn FnMut(Flow) + Send>,
) {
    let mut endpoint = Stack {
        port,
        mac,
        ip,
        conns: BTreeMap::new(),
        cmd_tx,
        isn: 0x0001_0000,
        ip_id: 0,
        on_connect,
    };
    loop {
        match frames.recv_timeout(POLL) {
            Ok(frame) => endpoint.frame(&frame),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        while let Ok(cmd) = cmds.try_recv() {
            if !endpoint.command(cmd) {
                return;
            }
        }
    }
}

/// Attaches the segment's gateway at `mac` with address `ip`, and serves it on its own thread. Each
/// connection the guest opens to the proxy's port is handed to `on_connect` as a [`Flow`] once it is
/// established. The returned [`Gateway`] stops the endpoint when dropped.
pub fn serve(
    switch: &Arc<Switch>,
    segment: &str,
    mac: Mac,
    ip: Ipv4Addr,
    on_connect: impl FnMut(Flow) + Send + 'static,
) -> Result<Gateway, NetError> {
    let (port, frames) = switch.attach(segment, Endpoint::Gateway { mac, ip })?;
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let owner = cmd_tx.clone();
    let thread = thread::Builder::new()
        .name("net-gateway".into())
        .spawn(move || run(port, frames, cmd_rx, owner, mac, ip, Box::new(on_connect)))
        .map_err(|err| NetError::Overlay(format!("the gateway thread: {err}")))?;
    Ok(Gateway {
        cmd: cmd_tx,
        thread: Some(thread),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::virtio::net::switch::Endpoint;
    use std::net::Ipv4Addr;

    const GUEST: Mac = [0x02, 0x41, 0x50, 0x07, 0x00, 0x02];
    const GW: Mac = [0x02, 0x41, 0x50, 0x07, 0x00, 0x01];
    const GUEST_IP: Ipv4Addr = Ipv4Addr::new(10, 7, 0, 2);
    const GW_IP: Ipv4Addr = Ipv4Addr::new(10, 7, 0, 1);
    const GUEST_PORT: u16 = 40000;
    const WAIT: Duration = Duration::from_secs(5);

    /// A segment from the guest to the proxy's port.
    fn from_guest(seq: u32, ack: u32, flags: u8, window: u16, payload: &[u8]) -> Vec<u8> {
        frame_for(
            (GUEST, GW),
            (GUEST_IP, GW_IP),
            (GUEST_PORT, PROXY_PORT),
            (seq, ack, flags, window),
            payload,
            7,
        )
    }

    /// What the gateway sent the guest: flags, sequence and acknowledgement numbers, and payload.
    #[derive(Debug)]
    struct Got {
        flags: u8,
        seq: u32,
        ack: u32,
        payload: Vec<u8>,
    }

    fn next(rx: &Receiver<Vec<u8>>) -> Got {
        let frame = rx.recv_timeout(WAIT).expect("the gateway answers");
        let seg = parse(&frame).expect("a TCP segment");
        assert_eq!((seg.src_ip, seg.src_port), (GW_IP, PROXY_PORT));
        assert_eq!(seg.dst_port, GUEST_PORT);
        Got {
            flags: seg.flags,
            seq: seg.seq,
            ack: seg.ack,
            payload: seg.payload.to_vec(),
        }
    }

    /// A guest and a gateway on one segment, the gateway handing each connection to a channel.
    struct Lab {
        guest: crate::devices::virtio::net::switch::Port,
        guest_rx: Receiver<Vec<u8>>,
        flows: Receiver<Flow>,
        _gateway: Gateway,
    }

    fn lab() -> Lab {
        let switch = Switch::new();
        switch.create_segment("lab");
        let (guest, guest_rx) = switch
            .attach("lab", Endpoint::Guest { mac: GUEST })
            .unwrap();
        let (flows_tx, flows) = mpsc::channel();
        let gateway = serve(&switch, "lab", GW, GW_IP, move |flow| {
            flows_tx.send(flow).ok();
        })
        .unwrap();
        Lab {
            guest,
            guest_rx,
            flows,
            _gateway: gateway,
        }
    }

    /// The handshake from the guest's side, with `window` in its acknowledgement. Returns the gateway's
    /// initial sequence number and the guest's next sequence number.
    fn handshake(lab: &Lab, window: u16) -> (u32, u32) {
        lab.guest
            .send(&from_guest(1000, 0, SYN, 65535, &[]))
            .unwrap();
        let synack = next(&lab.guest_rx);
        assert_eq!(synack.flags, SYN | ACK, "the gateway answers the SYN");
        assert_eq!(synack.ack, 1001);
        lab.guest
            .send(&from_guest(1001, synack.seq + 1, ACK, window, &[]))
            .unwrap();
        (synack.seq, 1001)
    }

    #[test]
    fn a_proxied_connection_carries_bytes_both_ways_and_closes_cleanly() {
        let lab = lab();
        let (gw_isn, _) = handshake(&lab, 65535);
        let mut flow = lab
            .flows
            .recv_timeout(WAIT)
            .expect("the connection opens to the owner");

        // The guest's request arrives in order and the owner reads it.
        lab.guest
            .send(&from_guest(
                1001,
                gw_isn + 1,
                ACK | PSH,
                65535,
                b"CONNECT example.test:443\n",
            ))
            .unwrap();
        let mut buf = [0u8; 64];
        let n = flow.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"CONNECT example.test:443\n");
        let ack_of_request = next(&lab.guest_rx);
        assert_eq!(ack_of_request.ack, 1001 + 25, "the request is acknowledged");

        // The owner's answer reaches the guest, with the next sequence number.
        flow.write_all(b"OK\n").unwrap();
        let answer = next(&lab.guest_rx);
        assert_eq!(answer.payload, b"OK\n");
        assert_eq!(answer.seq, gw_isn + 1);
        lab.guest
            .send(&from_guest(1001 + 25, gw_isn + 1 + 3, ACK, 65535, &[]))
            .unwrap();

        // The guest closes its side: the owner reads the end of the stream, and the gateway acknowledges.
        lab.guest
            .send(&from_guest(
                1001 + 25,
                gw_isn + 1 + 3,
                FIN | ACK,
                65535,
                &[],
            ))
            .unwrap();
        let fin_ack = next(&lab.guest_rx);
        assert_eq!(fin_ack.flags & ACK, ACK);
        assert_eq!(fin_ack.ack, 1001 + 25 + 1);
        assert_eq!(
            flow.read(&mut buf).unwrap(),
            0,
            "the guest's end is the end of the stream"
        );

        // The owner closes too: the gateway sends its FIN, and the guest's ACK finishes the connection.
        drop(flow);
        let fin = next(&lab.guest_rx);
        assert_eq!(
            fin.flags & FIN,
            FIN,
            "the gateway closes after the owner does"
        );
        lab.guest
            .send(&from_guest(1001 + 25 + 1, fin.seq + 1, ACK, 65535, &[]))
            .unwrap();
    }

    #[test]
    fn the_guests_window_bounds_what_the_gateway_sends_at_once() {
        let lab = lab();
        let (gw_isn, _) = handshake(&lab, 4);
        let mut flow = lab.flows.recv_timeout(WAIT).unwrap();
        flow.write_all(b"0123456789").unwrap();

        // Four bytes fit the window; the rest waits for the guest to acknowledge them.
        let first = next(&lab.guest_rx);
        assert_eq!(first.payload, b"0123");
        assert!(
            lab.guest_rx
                .recv_timeout(Duration::from_millis(100))
                .is_err()
        );

        lab.guest
            .send(&from_guest(1001, gw_isn + 1 + 4, ACK, 4, &[]))
            .unwrap();
        let second = next(&lab.guest_rx);
        assert_eq!(second.payload, b"4567");
        lab.guest
            .send(&from_guest(1001, gw_isn + 1 + 8, ACK, 4, &[]))
            .unwrap();
        assert_eq!(next(&lab.guest_rx).payload, b"89");
    }

    #[test]
    fn a_segment_for_no_connection_is_reset_and_nothing_opens() {
        let lab = lab();
        lab.guest
            .send(&from_guest(5000, 77, ACK, 65535, b"stray"))
            .unwrap();
        let reset = next(&lab.guest_rx);
        assert_eq!(reset.flags & RST, RST, "the gateway resets a stray segment");
        assert!(lab.flows.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[test]
    fn a_direct_connection_is_refused_by_the_switch_and_never_reaches_the_gateway() {
        // A guest that sends to a remote address through the gateway's MAC: the switch refuses it, so the
        // gateway sees nothing and answers nothing.
        let lab = lab();
        let remote = Ipv4Addr::new(93, 184, 216, 34);
        let direct = frame_for(
            (GUEST, GW),
            (GUEST_IP, remote),
            (GUEST_PORT, 443),
            (1000, 0, SYN, 65535),
            &[],
            9,
        );
        lab.guest.send(&direct).unwrap();
        assert!(
            lab.guest_rx
                .recv_timeout(Duration::from_millis(100))
                .is_err()
        );
        assert!(lab.flows.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[test]
    fn the_checksums_of_what_the_gateway_builds_are_right() {
        let frame = frame_for(
            (GW, GUEST),
            (GW_IP, GUEST_IP),
            (PROXY_PORT, GUEST_PORT),
            (1, 2, ACK, 9),
            b"abc",
            3,
        );
        let ip = &frame[ETH_HDR_LEN..ETH_HDR_LEN + 20];
        // A correct IPv4 header sums to zero, its checksum included.
        assert_eq!(checksum(&[ip]), 0);
        let tcp_len = (20 + 3) as u16;
        let mut pseudo = [0u8; 12];
        pseudo[0..4].copy_from_slice(&GW_IP.octets());
        pseudo[4..8].copy_from_slice(&GUEST_IP.octets());
        pseudo[9] = IP_PROTO_TCP;
        pseudo[10..12].copy_from_slice(&tcp_len.to_be_bytes());
        let tcp = &frame[ETH_HDR_LEN + 20..];
        assert_eq!(
            checksum(&[&pseudo, tcp]),
            0,
            "the TCP checksum covers the pseudo header and payload"
        );
    }
}
