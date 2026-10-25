// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port (PLAN 22.6, W58, round 2). A lab topology's segments live on this node's switch.
// Each segment has a gateway, the one way out for its VMs, and a proxied connection through it runs the
// egress proxy's own code: vsockd's CONNECT handling, its policy and its audit, on a loopback socket
// pair the gateway's flow is bridged to. A VM's frames reach the outside only as that proxy traffic.

//! The lab's network on this node: one switch, a gateway per topology segment, and the egress proxy behind
//! each gateway.

use std::collections::BTreeMap;
use std::io;
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use vmm::devices::virtio::net::Mac;
use vmm::devices::virtio::net::gateway::{self, Flow, Gateway};
use vmm::devices::virtio::net::overlay::{self, Trunk};
use vmm::devices::virtio::net::switch::Switch;
use vsockd::apid::Apid;
use vsockd::egress::{Policy, serve_bridged};

/// The node's lab network: its switch, the egress policy and apid's decisions, and the open segments.
pub struct LabNet {
    switch: Arc<Switch>,
    policy: Policy,
    apid: Apid,
    gateways: Mutex<BTreeMap<String, Gateway>>,
}

impl LabNet {
    /// A lab network with no segments, whose gateways apply `policy` and ask `apid` for each destination.
    pub fn new(policy: Policy, apid: Apid) -> Self {
        Self {
            switch: Switch::new(),
            policy,
            apid,
            gateways: Mutex::new(BTreeMap::new()),
        }
    }

    /// The node's switch: a lab VM's network device attaches to it on its segment.
    pub fn switch(&self) -> &Arc<Switch> {
        &self.switch
    }

    /// Opens `segment` with its gateway at `mac`/`ip`. Proxied connections through it are charged to the lab's
    /// `cid` in vsockd's audit. A segment that is open already is an error.
    pub fn open_segment(
        &self,
        segment: &str,
        cid: u32,
        mac: Mac,
        ip: Ipv4Addr,
    ) -> Result<(), String> {
        let mut gateways = self.gateways.lock().unwrap_or_else(PoisonError::into_inner);
        if gateways.contains_key(segment) {
            return Err(format!("the segment {segment:?} is open already"));
        }
        self.switch.create_segment(segment);
        let (policy, apid) = (self.policy.clone(), self.apid.clone());
        let gateway = gateway::serve(&self.switch, segment, mac, ip, move |flow| {
            proxy_flow(flow, cid, policy.clone(), apid.clone());
        })
        .map_err(|err| err.to_string())?;
        gateways.insert(segment.to_owned(), gateway);
        Ok(())
    }

    /// Opens the segment of a topology's plan: its gateway at the plan's gateway address and MAC, the VMs' interfaces
    /// attach with the plan's MACs.
    pub fn open_plan(&self, plan: &apz_topology::segment::Plan, cid: u32) -> Result<(), String> {
        self.open_segment(&plan.segment, cid, plan.gateway.mac, plan.gateway.ip)
    }

    /// Closes `segment`: its gateway stops, so its VMs have no way out. `false` when it was not open.
    pub fn close_segment(&self, segment: &str) -> bool {
        let mut gateways = self.gateways.lock().unwrap_or_else(PoisonError::into_inner);
        gateways.remove(segment).is_some()
    }
}

/// Joins the node's switch segment `segment` to the other nodes' switches through this node's tunnel `mux`. The
/// trunk is the stream the hub splices to the other node's trunk named `key` (the segment's name scoped by its
/// topology). The overlay carries the segment's frames until the tunnel ends.
pub fn join_trunk(
    switch: &Arc<Switch>,
    segment: &str,
    mux: &apz_http::mux::Mux,
    key: &str,
) -> Result<Trunk, String> {
    let stream = apz_link::tunnel::open_trunk(mux, key).map_err(|err| err.to_string())?;
    let (reader, writer) = apz_http::trunk::halves(stream);
    overlay::connect(switch, segment, reader, writer).map_err(|err| err.to_string())
}

/// Serves one proxied connection: the guest's bytes go to the egress proxy's connection handling and its
/// answers come back, over a loopback socket pair. The proxy's code is vsockd's, the same one vsock egress runs.
fn proxy_flow(flow: Flow, cid: u32, policy: Policy, apid: Apid) {
    let Ok(listener) = TcpListener::bind("127.0.0.1:0") else {
        return;
    };
    let Ok(addr) = listener.local_addr() else {
        return;
    };
    let Ok(client) = TcpStream::connect(addr) else {
        return;
    };
    let Ok((server, _)) = listener.accept() else {
        return;
    };
    thread::spawn(move || serve_bridged(server, cid, &policy, &apid));

    let (mut from_guest, mut to_guest) = flow.split();
    let Ok(mut to_proxy) = client.try_clone() else {
        return;
    };
    let mut from_proxy = client;
    // The guest's bytes into the proxy; the proxy's answers out to the guest. Each side's end closes its half.
    thread::spawn(move || {
        let _ = io::copy(&mut from_guest, &mut to_proxy);
        let _ = to_proxy.shutdown(std::net::Shutdown::Write);
    });
    thread::spawn(move || {
        let _ = io::copy(&mut from_proxy, &mut to_guest);
        // Dropping the writing half closes the guest's side of the connection.
        drop(to_guest);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{Read, Write};
    use std::sync::atomic::Ordering;
    use std::sync::mpsc::Receiver;
    use std::time::Duration;
    use vmm::devices::virtio::net::gateway::{ACK, PSH, RST, SYN, frame_for, parse};
    use vmm::devices::virtio::net::switch::{Endpoint, Port};

    const CID: u32 = 7;
    const GUEST: Mac = [0x02, 0x41, 0x50, 0x07, 0x00, 0x02];
    const GW: Mac = [0x02, 0x41, 0x50, 0x07, 0x00, 0x01];
    const GUEST_IP: Ipv4Addr = Ipv4Addr::new(10, 7, 0, 2);
    const GW_IP: Ipv4Addr = Ipv4Addr::new(10, 7, 0, 1);
    const GUEST_PORT: u16 = 40000;
    const WAIT: Duration = Duration::from_secs(10);

    /// A loopback "internet": answers each connection with "echo:" and what it read (4 bytes).
    fn internet() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { continue };
                thread::spawn(move || {
                    let mut got = [0u8; 4];
                    if conn.read_exact(&mut got).is_ok() {
                        let mut reply = b"echo:".to_vec();
                        reply.extend_from_slice(&got);
                        let _ = conn.write_all(&reply);
                    }
                });
            }
        });
        port
    }

    /// A guest on the node's lab segment, with the gateway at GW and apid's stand-in deciding egress.
    struct Guest {
        /// Keeps the segment's gateway open: dropping the lab network stops it.
        _lab: LabNet,
        port: Port,
        rx: Receiver<Vec<u8>>,
        checks: Arc<std::sync::atomic::AtomicUsize>,
        token: std::path::PathBuf,
    }

    fn guest(extra_ports: Vec<u16>) -> Guest {
        let (apid, checks, token) = vsockd::egress::fake_apid(
            "lab-net",
            |_| json!({"allow": true, "reason": "lab segment", "intercept": false}),
        );
        let lab = LabNet::new(Policy::new(extra_ports).allow_loopback(), apid);
        lab.open_segment("web", CID, GW, GW_IP).unwrap();
        let (port, rx) = lab
            .switch()
            .attach("web", Endpoint::Guest { mac: GUEST })
            .unwrap();
        Guest {
            _lab: lab,
            port,
            rx,
            checks,
            token,
        }
    }

    /// The next TCP segment the gateway sent the guest: its flags, sequence, acknowledgement and payload.
    fn next(rx: &Receiver<Vec<u8>>) -> (u8, u32, u32, Vec<u8>) {
        let frame = rx.recv_timeout(WAIT).expect("the gateway answers");
        let seg = parse(&frame).expect("a TCP segment");
        (seg.flags, seg.seq, seg.ack, seg.payload.to_vec())
    }

    /// Reads from the gateway, acknowledging each segment, until the bytes it has sent contain `marker`. `from` is
    /// the sequence number of its first byte of this read.
    fn gateway_until(g: &Guest, guest_seq: u32, from: u32, marker: &[u8]) -> Vec<u8> {
        let mut got = Vec::new();
        while !got.windows(marker.len()).any(|w| w == marker) {
            let (flags, seq, _ack, payload) = next(&g.rx);
            assert_eq!(
                flags & RST,
                0,
                "the gateway does not reset a live connection"
            );
            assert_eq!(seq, from.wrapping_add(got.len() as u32), "in order");
            got.extend_from_slice(&payload);
            let ack = seq.wrapping_add(payload.len() as u32);
            g.port
                .send(&frame_for(
                    (GUEST, GW),
                    (GUEST_IP, GW_IP),
                    (GUEST_PORT, 3128),
                    (guest_seq, ack, ACK, 65535),
                    &[],
                    1,
                ))
                .unwrap();
        }
        got
    }

    #[test]
    fn a_proxied_connection_reaches_the_outside_through_the_egress_proxy() {
        let target = internet();
        let g = guest(vec![target]);

        // The guest opens TCP to the gateway's proxy port: the handshake.
        g.port
            .send(&frame_for(
                (GUEST, GW),
                (GUEST_IP, GW_IP),
                (GUEST_PORT, 3128),
                (1000, 0, SYN, 65535),
                &[],
                1,
            ))
            .unwrap();
        let (flags, gw_isn, ack, _) = next(&g.rx);
        assert_eq!(flags & (SYN | ACK), SYN | ACK);
        assert_eq!(ack, 1001);
        g.port
            .send(&frame_for(
                (GUEST, GW),
                (GUEST_IP, GW_IP),
                (GUEST_PORT, 3128),
                (1001, gw_isn + 1, ACK, 65535),
                &[],
                2,
            ))
            .unwrap();

        // An HTTP CONNECT to the internet's address, as a proxied client sends it.
        let request =
            format!("CONNECT 127.0.0.1:{target} HTTP/1.1\r\nHost: 127.0.0.1:{target}\r\n\r\n");
        g.port
            .send(&frame_for(
                (GUEST, GW),
                (GUEST_IP, GW_IP),
                (GUEST_PORT, 3128),
                (1001, gw_isn + 1, ACK | PSH, 65535),
                request.as_bytes(),
                3,
            ))
            .unwrap();
        let head = gateway_until(&g, 1001 + request.len() as u32, gw_isn + 1, b"\r\n\r\n");
        assert!(
            String::from_utf8_lossy(&head).starts_with("HTTP/1.1 200"),
            "the proxy accepted the CONNECT: {:?}",
            String::from_utf8_lossy(&head)
        );

        // Through the tunnel the bytes reach the internet and its answer comes back.
        let after_request = 1001 + request.len() as u32;
        g.port
            .send(&frame_for(
                (GUEST, GW),
                (GUEST_IP, GW_IP),
                (GUEST_PORT, 3128),
                (after_request, gw_isn + 1 + 17, ACK | PSH, 65535),
                b"ping",
                4,
            ))
            .unwrap();
        let answer = gateway_until(
            &g,
            after_request + 4,
            gw_isn + 1 + head.len() as u32,
            b"echo:ping",
        );
        assert!(
            answer.ends_with(b"echo:ping"),
            "the internet's answer came back: {answer:?}"
        );
        assert!(
            g.checks.load(Ordering::SeqCst) >= 1,
            "the destination was decided by apid, as vsock egress does"
        );
        std::fs::remove_file(&g.token).ok();
    }

    #[test]
    fn a_direct_connection_out_of_the_segment_never_leaves_it() {
        let g = guest(Vec::new());
        // A listener that would see any dial: the guest's direct connection, through the gateway's MAC, to it.
        let quiet = TcpListener::bind("127.0.0.1:0").unwrap();
        quiet.set_nonblocking(true).unwrap();
        let direct = quiet.local_addr().unwrap().port();
        g.port
            .send(&frame_for(
                (GUEST, GW),
                (GUEST_IP, Ipv4Addr::LOCALHOST),
                (GUEST_PORT, direct),
                (1000, 0, SYN, 65535),
                &[],
                9,
            ))
            .unwrap();
        assert!(
            g.rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "the switch refuses it: no answer"
        );
        thread::sleep(Duration::from_millis(100));
        assert!(
            matches!(quiet.accept(), Err(ref e) if e.kind() == io::ErrorKind::WouldBlock),
            "nothing dialled the destination"
        );
        assert_eq!(
            g.checks.load(Ordering::SeqCst),
            0,
            "apid was never asked about it"
        );
        std::fs::remove_file(&g.token).ok();
    }

    #[test]
    fn a_segment_opened_twice_is_refused_and_a_closed_one_has_no_gateway() {
        let (apid, _checks, token) =
            vsockd::egress::fake_apid("lab-net-twice", |_| json!({"allow": false}));
        let lab = LabNet::new(Policy::new(Vec::new()), apid);
        lab.open_segment("db", CID, GW, GW_IP).unwrap();
        assert!(lab.open_segment("db", CID, GW, GW_IP).is_err());
        assert!(lab.close_segment("db"));
        assert!(!lab.close_segment("db"));
        // With the gateway gone the segment accepts a new one.
        lab.open_segment("db", CID, GW, GW_IP).unwrap();
        std::fs::remove_file(&token).ok();
    }
}

#[cfg(test)]
mod trunk_tests {
    use super::*;
    use apz_http::mux::{Carrier, Mux, Side};
    use apz_http::trunk::{TrunkHub, trunk_segment};
    use std::net::TcpListener;
    use std::sync::mpsc::{self, Receiver};
    use std::time::Duration;
    use vmm::devices::virtio::net::switch::{Endpoint, Port};

    const A: Mac = [0x02, 0x41, 0x50, 0x0a, 0x00, 0x01];
    const B: Mac = [0x02, 0x41, 0x50, 0x0b, 0x00, 0x01];
    const WAIT: Duration = Duration::from_secs(5);

    /// One mux carrier over a loopback TCP connection: each message is a length-prefixed frame, read by a thread.
    struct Wire {
        out: std::net::TcpStream,
        incoming: Receiver<io::Result<Vec<u8>>>,
    }

    impl Wire {
        fn new(stream: std::net::TcpStream) -> Self {
            let (tx, incoming) = mpsc::channel();
            let mut read = stream.try_clone().expect("a clone of the socket");
            thread::spawn(move || {
                loop {
                    let mut len = [0u8; 4];
                    let body = io::Read::read_exact(&mut read, &mut len).and_then(|()| {
                        let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
                        io::Read::read_exact(&mut read, &mut body).map(|()| body)
                    });
                    let failed = body.is_err();
                    if tx.send(body).is_err() || failed {
                        return;
                    }
                }
            });
            Self {
                out: stream,
                incoming,
            }
        }
    }

    impl Carrier for Wire {
        fn send(&mut self, frame: &[u8]) -> io::Result<()> {
            let mut message = (frame.len() as u32).to_be_bytes().to_vec();
            message.extend_from_slice(frame);
            io::Write::write_all(&mut self.out, &message)
        }

        fn recv(&mut self, wait: Duration) -> io::Result<Option<Vec<u8>>> {
            match self.incoming.recv_timeout(wait) {
                Ok(frame) => frame.map(Some),
                Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
                Err(mpsc::RecvTimeoutError::Disconnected) => Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the tunnel is closed",
                )),
            }
        }
    }

    /// A node's mux (the dialing end) and the hub's mux for that node (the accepting end), over loopback.
    fn tunnel() -> (Arc<Mux>, Arc<Mux>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let node = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (hub, _) = listener.accept().unwrap();
        (
            Mux::new(Wire::new(node), Side::Dialer),
            Mux::new(Wire::new(hub), Side::Acceptor),
        )
    }

    /// The hub's side of one node's tunnel: each trunk the node opens is offered to the hub's pairing table.
    fn hub_serves(mux: Arc<Mux>, hub: Arc<TrunkHub>) {
        thread::spawn(move || {
            loop {
                match mux.accept(Duration::from_millis(100)) {
                    Ok(Some(stream)) => match trunk_segment(&stream) {
                        Some(segment) => hub.offer(&segment, stream),
                        None => stream.reset("not a trunk"),
                    },
                    Ok(None) if mux.alive() => {}
                    _ => return,
                }
            }
        });
    }

    fn frame(dst: Mac, src: Mac, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&dst);
        out.extend_from_slice(&src);
        out.extend_from_slice(&[0x88, 0xb5]);
        out.extend_from_slice(payload);
        out
    }

    fn lab_guest(switch: &Arc<Switch>, mac: Mac) -> (Port, Receiver<Vec<u8>>) {
        switch.attach("web", Endpoint::Guest { mac }).unwrap()
    }

    #[test]
    fn two_switches_joined_through_the_node_tunnel_carry_the_segments_frames_both_ways() {
        let hub = Arc::new(TrunkHub::new());
        let (node_a, hub_a) = tunnel();
        let (node_b, hub_b) = tunnel();
        hub_serves(hub_a, Arc::clone(&hub));
        hub_serves(hub_b, Arc::clone(&hub));

        let switch_a = Switch::new();
        switch_a.create_segment("web");
        let switch_b = Switch::new();
        switch_b.create_segment("web");
        let (pa, ra) = lab_guest(&switch_a, A);
        let (pb, rb) = lab_guest(&switch_b, B);

        // Each node's trunk for the segment, opened on its tunnel: the hub pairs them.
        let _trunk_a = join_trunk(&switch_a, "web", &node_a, "t1/web").unwrap();
        let _trunk_b = join_trunk(&switch_b, "web", &node_b, "t1/web").unwrap();

        // A's frame reaches B across the tunnel, and B's answer comes back.
        let there = frame(B, A, b"from node a");
        assert!(pa.send(&there).is_ok());
        assert_eq!(rb.recv_timeout(WAIT).unwrap(), there);
        let back = frame(A, B, b"from node b");
        assert!(pb.send(&back).is_ok());
        assert_eq!(ra.recv_timeout(WAIT).unwrap(), back);
        assert_eq!(hub.waiting(), 0, "both trunks are paired");
    }

    #[test]
    fn a_trunk_for_another_topology_is_never_paired_with_this_one() {
        let hub = Arc::new(TrunkHub::new());
        let (node_a, hub_a) = tunnel();
        let (node_b, hub_b) = tunnel();
        hub_serves(hub_a, Arc::clone(&hub));
        hub_serves(hub_b, Arc::clone(&hub));

        let switch_a = Switch::new();
        switch_a.create_segment("web");
        let switch_b = Switch::new();
        switch_b.create_segment("web");
        let (pa, _ra) = lab_guest(&switch_a, A);
        let (_pb, rb) = lab_guest(&switch_b, B);
        let _trunk_a = join_trunk(&switch_a, "web", &node_a, "t1/web").unwrap();
        let _trunk_b = join_trunk(&switch_b, "web", &node_b, "t2/web").unwrap();

        // Not paired: the frame stays on its own node.
        assert!(pa.send(&frame(B, A, b"nowhere")).is_ok());
        assert!(rb.recv_timeout(Duration::from_millis(300)).is_err());
        assert_eq!(hub.waiting(), 2, "each trunk waits for its own pair");
    }
}
