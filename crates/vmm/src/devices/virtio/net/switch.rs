// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port (PLAN 22.6, W58). Forwarding is decided here, on the host, for every frame a
// lab VM sends: the guest cannot address another segment, cannot send from another MAC, and
// reaches the outside only through the gateway port, which carries the egress proxy's TCP
// connections to its own address and port and nothing else.

//! The userspace switch behind a lab topology's segments.
//!
//! A [`Switch`] holds named segments. Every port belongs to one segment and only ever sees that
//! segment's frames:
//!
//! * a guest port ([`Endpoint::Guest`]) sends frames whose source is its own MAC (anything else is
//!   refused as spoofing); a frame to a MAC of the segment reaches that port; a broadcast or an
//!   unknown unicast floods the segment's guests and trunks;
//! * the gateway port ([`Endpoint::Gateway`]) is the segment's one way out. A frame from a guest to
//!   the gateway's MAC is delivered to it only when it is IPv4 TCP to the gateway's address on
//!   [`PROXY_PORT`] (the egress proxy); any other frame to the gateway is refused. The gateway is
//!   never flooded, and it sends only unicast to a MAC of its own segment;
//! * a trunk port ([`Endpoint::Trunk`]) carries the segment's frames to another switch (see
//!   [`super::overlay`]). Its source MACs are learned, and a frame it brings is delivered to the
//!   segment's local ports only: a trunk never forwards to another trunk, so two switches joined by
//!   a trunk cannot loop.
//!
//! Frames are copied into the receiving port's channel while the switch's lock is held; nothing
//! calls back into a device under that lock, so a device's transmit path and a switch's delivery to
//! it cannot deadlock.

use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::{ETH_HDR_LEN, MAX_FRAME_LEN, Mac, NetError, mac_text};

/// The port the egress proxy listens on (`crates/vsockd`'s proxy, `127.0.0.1:3128` in a guest).
pub const PROXY_PORT: u16 = 3128;

const ETHERTYPE_IPV4: [u8; 2] = [0x08, 0x00];
const IP_PROTO_TCP: u8 = 6;

/// A port's identity on its switch.
pub type PortId = u64;

/// What a port is attached as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    /// A guest's interface with its MAC.
    Guest {
        /// The interface's MAC: the only source the port may send from.
        mac: Mac,
    },
    /// Another switch, joined by an overlay stream. Its MACs are learned from the frames it brings.
    Trunk,
    /// The segment's egress gateway, with the MAC and address the guests send their proxy traffic to.
    Gateway {
        /// The gateway's MAC.
        mac: Mac,
        /// The gateway's IPv4 address: the destination of the proxy connections it accepts.
        ip: Ipv4Addr,
    },
}

/// The kind of a port, as the switch sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Guest,
    Trunk,
    Gateway,
}

/// Why a frame was not forwarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The source is multicast, or the port was detached.
    Malformed,
    /// A guest sent from a source that is not its own MAC.
    Spoofed,
    /// No port of the segment can take the frame (a gateway's frame to an unknown MAC, or a
    /// frame to the port that sent it).
    NoRoute,
    /// The gateway does not carry the frame: not the egress proxy, or a broadcast from the gateway.
    Refused,
    /// A trunk's frame addressed to another trunk's MAC: a trunk never transits.
    Split,
}

/// What became of a frame the port sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Delivered to this many ports (zero for a flood with nowhere to go).
    Delivered(usize),
    /// Not forwarded, for this reason.
    Dropped(Reason),
}

#[derive(Debug, Clone, Copy)]
struct Gateway {
    id: PortId,
    mac: Mac,
    ip: Ipv4Addr,
}

#[derive(Debug)]
struct Entry {
    segment: String,
    kind: Kind,
    mac: Option<Mac>,
    tx: Sender<Vec<u8>>,
}

#[derive(Debug, Default)]
struct Segment {
    ports: BTreeSet<PortId>,
    /// Each MAC the segment knows and the port it lives behind: a local guest or gateway (static,
    /// from attachment) or a remote guest behind a trunk (learned from the trunk's frames).
    macs: BTreeMap<Mac, PortId>,
    gateway: Option<Gateway>,
}

#[derive(Debug, Default)]
struct State {
    segments: BTreeMap<String, Segment>,
    ports: BTreeMap<PortId, Entry>,
    next: PortId,
}

/// A switch: the segments of one node's lab topologies.
pub struct Switch {
    state: Mutex<State>,
}

impl std::fmt::Debug for Switch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.lock();
        f.debug_struct("Switch")
            .field("segments", &state.segments.keys().collect::<Vec<_>>())
            .field("ports", &state.ports.len())
            .finish()
    }
}

/// Whether a frame's destination (or source) MAC is a group address: the low bit of its first octet.
fn is_multicast(mac: &Mac) -> bool {
    mac[0] & 1 != 0
}

/// Whether `frame` is IPv4 TCP to `ip` on [`PROXY_PORT`]: the only traffic the gateway carries.
pub fn is_proxy_connection(frame: &[u8], ip: Ipv4Addr) -> bool {
    if frame.len() < ETH_HDR_LEN + 20 || frame[12..14] != ETHERTYPE_IPV4 {
        return false;
    }
    let packet = &frame[ETH_HDR_LEN..];
    if packet[0] >> 4 != 4 {
        return false;
    }
    let header_len = usize::from(packet[0] & 0x0f) * 4;
    if header_len < 20 || packet.len() < header_len + 4 {
        return false;
    }
    if packet[9] != IP_PROTO_TCP || packet[16..20] != ip.octets() {
        return false;
    }
    let tcp = &packet[header_len..];
    u16::from_be_bytes([tcp[2], tcp[3]]) == PROXY_PORT
}

impl Switch {
    /// A switch with no segments.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State::default()),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Creates a segment. Creating one that exists is a no-op: a topology's segment is named by its
    /// topology, so each of its VMs' attachments may ask for it.
    pub fn create_segment(&self, name: &str) {
        self.lock().segments.entry(name.to_owned()).or_default();
    }

    /// Attaches a port to `segment`. The returned [`Port`] sends frames into the switch, and detaches
    /// when dropped; the receiver gets the frames the switch delivers to it.
    pub fn attach(
        self: &Arc<Self>,
        segment: &str,
        endpoint: Endpoint,
    ) -> Result<(Port, Receiver<Vec<u8>>), NetError> {
        let mut state = self.lock();
        let State {
            segments,
            ports,
            next,
        } = &mut *state;
        let seg = segments
            .get_mut(segment)
            .ok_or_else(|| NetError::UnknownSegment(segment.to_owned()))?;
        let id = *next;
        *next += 1;
        let (tx, rx) = mpsc::channel();
        let (kind, mac) = match endpoint {
            Endpoint::Guest { mac } => (Kind::Guest, Some(mac)),
            Endpoint::Trunk => (Kind::Trunk, None),
            Endpoint::Gateway { mac, .. } => {
                if seg.gateway.is_some() {
                    return Err(NetError::GatewayTaken(segment.to_owned()));
                }
                (Kind::Gateway, Some(mac))
            }
        };
        if let Some(mac) = mac
            && let Some(&owner) = seg.macs.get(&mac)
        {
            // A MAC learned behind a trunk gives way to a local port; a local MAC cannot be taken twice.
            let learned = ports
                .get(&owner)
                .is_some_and(|entry| entry.kind == Kind::Trunk);
            if !learned {
                return Err(NetError::MacInUse {
                    mac: mac_text(&mac),
                    segment: segment.to_owned(),
                });
            }
        }
        if let Endpoint::Gateway { mac, ip } = endpoint {
            seg.gateway = Some(Gateway { id, mac, ip });
        }
        if let Some(mac) = mac {
            seg.macs.insert(mac, id);
        }
        seg.ports.insert(id);
        ports.insert(
            id,
            Entry {
                segment: segment.to_owned(),
                kind,
                mac,
                tx,
            },
        );
        drop(state);
        Ok((
            Port {
                switch: Arc::clone(self),
                id,
                kind,
                mac,
            },
            rx,
        ))
    }

    /// Removes a port: its MACs leave the segment's table and the frames it would have taken are
    /// no longer delivered (a learned MAC behind it is forgotten and floods until learned again).
    fn detach(&self, id: PortId) {
        let mut state = self.lock();
        let Some(entry) = state.ports.remove(&id) else {
            return;
        };
        if let Some(seg) = state.segments.get_mut(&entry.segment) {
            seg.ports.remove(&id);
            seg.macs.retain(|_, owner| *owner != id);
            if seg.gateway.is_some_and(|gw| gw.id == id) {
                seg.gateway = None;
            }
        }
    }

    /// Decides where a frame from port `origin` goes and delivers it.
    fn forward(&self, origin: PortId, frame: &[u8]) -> Result<Verdict, NetError> {
        if frame.len() > MAX_FRAME_LEN {
            return Err(NetError::FrameTooLarge(frame.len()));
        }
        if frame.len() < ETH_HDR_LEN {
            return Err(NetError::FrameTooShort(frame.len()));
        }
        let dst: Mac = [frame[0], frame[1], frame[2], frame[3], frame[4], frame[5]];
        let src: Mac = [frame[6], frame[7], frame[8], frame[9], frame[10], frame[11]];
        let mut state = self.lock();
        let Some(origin_entry) = state.ports.get(&origin) else {
            return Ok(Verdict::Dropped(Reason::Malformed));
        };
        let segment_name = origin_entry.segment.clone();
        let kind = origin_entry.kind;
        if is_multicast(&src) {
            return Ok(Verdict::Dropped(Reason::Malformed));
        }
        let State {
            segments, ports, ..
        } = &mut *state;
        let Some(seg) = segments.get_mut(&segment_name) else {
            return Ok(Verdict::Dropped(Reason::Malformed));
        };
        match kind {
            Kind::Guest | Kind::Gateway => {
                let own = ports.get(&origin).and_then(|entry| entry.mac);
                if own != Some(src) {
                    return Ok(Verdict::Dropped(Reason::Spoofed));
                }
            }
            Kind::Trunk => {
                // A remote guest's MAC: learned, unless it is one of this segment's local ports.
                let local = seg
                    .macs
                    .get(&src)
                    .is_some_and(|owner| ports.get(owner).is_some_and(|e| e.kind != Kind::Trunk));
                if !local {
                    seg.macs.insert(src, origin);
                }
            }
        }

        let kind_of = |id: &PortId| ports.get(id).map(|entry| entry.kind);
        let targets: Vec<PortId> = if is_multicast(&dst) {
            if kind == Kind::Gateway {
                return Ok(Verdict::Dropped(Reason::Refused));
            }
            seg.ports
                .iter()
                .copied()
                .filter(|id| *id != origin)
                .filter(|id| match kind_of(id) {
                    Some(Kind::Gateway) => false,
                    Some(Kind::Trunk) => kind != Kind::Trunk,
                    _ => true,
                })
                .collect()
        } else if let Some(gw) = seg.gateway.filter(|gw| gw.mac == dst) {
            if gw.id == origin {
                return Ok(Verdict::Dropped(Reason::NoRoute));
            }
            if kind == Kind::Gateway || !is_proxy_connection(frame, gw.ip) {
                return Ok(Verdict::Dropped(Reason::Refused));
            }
            vec![gw.id]
        } else if let Some(&target) = seg.macs.get(&dst) {
            if target == origin {
                return Ok(Verdict::Dropped(Reason::NoRoute));
            }
            if kind == Kind::Trunk && kind_of(&target) == Some(Kind::Trunk) {
                return Ok(Verdict::Dropped(Reason::Split));
            }
            vec![target]
        } else {
            // An unknown unicast: the segment's guests (and, for a local sender, its trunks) get it.
            // A gateway never floods: it answers the MACs it knows, and only those.
            if kind == Kind::Gateway {
                return Ok(Verdict::Dropped(Reason::NoRoute));
            }
            seg.ports
                .iter()
                .copied()
                .filter(|id| *id != origin)
                .filter(|id| match kind_of(id) {
                    Some(Kind::Gateway) => false,
                    Some(Kind::Trunk) => kind != Kind::Trunk,
                    _ => true,
                })
                .collect()
        };

        let mut delivered = 0;
        for target in targets {
            if let Some(entry) = ports.get(&target)
                && entry.tx.send(frame.to_vec()).is_ok()
            {
                delivered += 1;
            }
        }
        Ok(Verdict::Delivered(delivered))
    }
}

/// A port on a switch: the handle a device or an overlay sends its frames through. Dropping it
/// detaches the port.
pub struct Port {
    switch: Arc<Switch>,
    id: PortId,
    kind: Kind,
    mac: Option<Mac>,
}

impl std::fmt::Debug for Port {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Port")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("mac", &self.mac.as_ref().map(mac_text))
            .finish()
    }
}

impl Port {
    /// The MAC a guest or gateway port sends from (`None` for a trunk).
    pub fn mac(&self) -> Option<Mac> {
        self.mac
    }

    /// Whether this port is the segment's gateway.
    pub fn is_gateway(&self) -> bool {
        self.kind == Kind::Gateway
    }

    /// Whether this port is a trunk to another switch.
    pub fn is_trunk(&self) -> bool {
        self.kind == Kind::Trunk
    }

    /// Sends one Ethernet frame into the switch. Frames of the wrong size are refused with an error,
    /// frames the segment's rules do not carry with a [`Verdict::Dropped`].
    pub fn send(&self, frame: &[u8]) -> Result<Verdict, NetError> {
        self.switch.forward(self.id, frame)
    }
}

impl Drop for Port {
    fn drop(&mut self) {
        self.switch.detach(self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const A: Mac = [0x02, 0, 0, 0, 0, 0x0a];
    const B: Mac = [0x02, 0, 0, 0, 0, 0x0b];
    const C: Mac = [0x02, 0, 0, 0, 0, 0x0c];
    const GW: Mac = [0x02, 0, 0, 0, 0, 0xfe];
    const GW_IP: Ipv4Addr = Ipv4Addr::new(10, 7, 0, 1);
    const BCAST: Mac = [0xff; 6];

    /// An Ethernet frame from `src` to `dst` carrying `payload` (EtherType 0x88b5, not IP).
    fn frame(dst: Mac, src: Mac, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&dst);
        out.extend_from_slice(&src);
        out.extend_from_slice(&[0x88, 0xb5]);
        out.extend_from_slice(payload);
        out
    }

    /// An IPv4 TCP frame from `src` to `dst_mac`, to `ip`:`port`.
    fn tcp_frame(dst_mac: Mac, src: Mac, ip: Ipv4Addr, port: u16) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&dst_mac);
        out.extend_from_slice(&src);
        out.extend_from_slice(&ETHERTYPE_IPV4);
        let mut ipv4 = [0u8; 20];
        ipv4[0] = 0x45;
        ipv4[9] = IP_PROTO_TCP;
        ipv4[12..16].copy_from_slice(&[10, 7, 0, 10]);
        ipv4[16..20].copy_from_slice(&ip.octets());
        out.extend_from_slice(&ipv4);
        let mut tcp = [0u8; 20];
        tcp[2..4].copy_from_slice(&port.to_be_bytes());
        out.extend_from_slice(&tcp);
        out
    }

    fn got(rx: &Receiver<Vec<u8>>) -> Option<Vec<u8>> {
        rx.recv_timeout(Duration::from_millis(50)).ok()
    }

    fn lan() -> (
        Arc<Switch>,
        Port,
        Receiver<Vec<u8>>,
        Port,
        Receiver<Vec<u8>>,
        Port,
        Receiver<Vec<u8>>,
    ) {
        let switch = Switch::new();
        switch.create_segment("s");
        let (pa, ra) = switch.attach("s", Endpoint::Guest { mac: A }).unwrap();
        let (pb, rb) = switch.attach("s", Endpoint::Guest { mac: B }).unwrap();
        let (pc, rc) = switch.attach("s", Endpoint::Guest { mac: C }).unwrap();
        (switch, pa, ra, pb, rb, pc, rc)
    }

    #[test]
    fn a_unicast_frame_reaches_only_its_destination() {
        let (_switch, pa, _ra, _pb, rb, _pc, rc) = lan();
        let f = frame(B, A, b"hello");
        assert_eq!(pa.send(&f).unwrap(), Verdict::Delivered(1));
        assert_eq!(got(&rb), Some(f));
        assert_eq!(got(&rc), None, "the third port hears nothing");
    }

    #[test]
    fn a_broadcast_floods_the_segment_except_the_sender() {
        let (_switch, pa, ra, _pb, rb, _pc, rc) = lan();
        let f = frame(BCAST, A, b"arp");
        assert_eq!(pa.send(&f).unwrap(), Verdict::Delivered(2));
        assert_eq!(got(&rb), Some(f.clone()));
        assert_eq!(got(&rc), Some(f));
        assert_eq!(got(&ra), None, "a port never hears its own frame");
    }

    #[test]
    fn an_unknown_unicast_floods_the_segment_and_a_detached_mac_is_forgotten() {
        let (_switch, pa, _ra, pb, rb, _pc, rc) = lan();
        let unknown = [0x02, 0, 0, 0, 0, 0x77];
        let f = frame(unknown, A, b"who");
        assert_eq!(pa.send(&f).unwrap(), Verdict::Delivered(2));
        assert_eq!(got(&rb), Some(f.clone()));
        assert_eq!(got(&rc), Some(f));
        drop(pb);
        drop(rb);
        // B is gone: a frame to its MAC no longer finds it, so it floods to C alone.
        let g = frame(B, A, b"again");
        assert_eq!(pa.send(&g).unwrap(), Verdict::Delivered(1));
        assert_eq!(got(&rc), Some(g));
    }

    #[test]
    fn a_guest_cannot_send_from_another_mac() {
        let (_switch, pa, _ra, _pb, rb, _pc, rc) = lan();
        let spoofed = frame(B, B, b"i am b");
        assert_eq!(
            pa.send(&spoofed).unwrap(),
            Verdict::Dropped(Reason::Spoofed)
        );
        assert_eq!(got(&rb), None);
        assert_eq!(got(&rc), None);
    }

    #[test]
    fn two_segments_cannot_reach_each_other() {
        let switch = Switch::new();
        switch.create_segment("one");
        switch.create_segment("two");
        let (p1, _r1) = switch.attach("one", Endpoint::Guest { mac: A }).unwrap();
        let (_p2, r2) = switch.attach("two", Endpoint::Guest { mac: B }).unwrap();
        // The same MAC in another segment is not the same station.
        let (_p3, r3) = switch.attach("one", Endpoint::Guest { mac: C }).unwrap();

        // B's MAC is known only in "two": a frame to it from "one" floods "one" and never reaches B.
        assert_eq!(p1.send(&frame(B, A, b"x")).unwrap(), Verdict::Delivered(1));
        assert_eq!(got(&r2), None, "segment two never hears segment one");
        assert_eq!(
            got(&r3),
            Some(frame(B, A, b"x")),
            "the flood stays in segment one"
        );
        assert_eq!(
            p1.send(&frame(BCAST, A, b"y")).unwrap(),
            Verdict::Delivered(1)
        );
        assert_eq!(got(&r2), None, "a broadcast is not carried across segments");
    }

    #[test]
    fn an_unknown_segment_and_a_second_mac_are_refused() {
        let switch = Switch::new();
        assert!(matches!(
            switch.attach("nope", Endpoint::Guest { mac: A }),
            Err(NetError::UnknownSegment(_))
        ));
        switch.create_segment("s");
        let (_p, _r) = switch.attach("s", Endpoint::Guest { mac: A }).unwrap();
        assert!(matches!(
            switch.attach("s", Endpoint::Guest { mac: A }),
            Err(NetError::MacInUse { .. })
        ));
    }

    #[test]
    fn the_gateway_carries_the_proxy_connection_and_nothing_else() {
        let switch = Switch::new();
        switch.create_segment("s");
        let (pa, _ra) = switch.attach("s", Endpoint::Guest { mac: A }).unwrap();
        let (pb, rb) = switch.attach("s", Endpoint::Guest { mac: B }).unwrap();
        let (gw, rg) = switch
            .attach("s", Endpoint::Gateway { mac: GW, ip: GW_IP })
            .unwrap();
        assert!(gw.is_gateway());

        // The proxy connection: delivered to the gateway.
        let proxy = tcp_frame(GW, A, GW_IP, PROXY_PORT);
        assert_eq!(pa.send(&proxy).unwrap(), Verdict::Delivered(1));
        assert_eq!(got(&rg), Some(proxy));

        // Any other port, or another address, is refused.
        assert_eq!(
            pa.send(&tcp_frame(GW, A, GW_IP, 443)).unwrap(),
            Verdict::Dropped(Reason::Refused)
        );
        assert_eq!(
            pa.send(&tcp_frame(
                GW,
                A,
                Ipv4Addr::new(93, 184, 216, 34),
                PROXY_PORT
            ))
            .unwrap(),
            Verdict::Dropped(Reason::Refused)
        );
        assert_eq!(got(&rg), None);

        // A broadcast from a guest never reaches the gateway, and the gateway floods nothing.
        assert_eq!(
            pa.send(&frame(BCAST, A, b"arp")).unwrap(),
            Verdict::Delivered(1)
        );
        assert_eq!(got(&rg), None);
        assert_eq!(got(&rb), Some(frame(BCAST, A, b"arp")));

        // The gateway answers a guest it knows, and only that.
        assert_eq!(
            gw.send(&frame(B, GW, b"reply")).unwrap(),
            Verdict::Delivered(1)
        );
        assert_eq!(got(&rb), Some(frame(B, GW, b"reply")));
        assert_eq!(
            gw.send(&frame([0x02, 0, 0, 0, 0, 0x55], GW, b"stray"))
                .unwrap(),
            Verdict::Dropped(Reason::NoRoute)
        );
        assert_eq!(
            gw.send(&frame(BCAST, GW, b"flood")).unwrap(),
            Verdict::Dropped(Reason::Refused)
        );
        drop(pb);
    }

    #[test]
    fn a_second_gateway_is_refused_and_a_detached_one_frees_the_slot() {
        let switch = Switch::new();
        switch.create_segment("s");
        let (gw, _rg) = switch
            .attach("s", Endpoint::Gateway { mac: GW, ip: GW_IP })
            .unwrap();
        assert!(matches!(
            switch.attach(
                "s",
                Endpoint::Gateway {
                    mac: [2, 0, 0, 0, 0, 0xfd],
                    ip: GW_IP
                }
            ),
            Err(NetError::GatewayTaken(_))
        ));
        drop(gw);
        assert!(
            switch
                .attach("s", Endpoint::Gateway { mac: GW, ip: GW_IP })
                .is_ok()
        );
    }

    #[test]
    fn a_frame_that_is_too_short_or_too_large_is_an_error() {
        let (_switch, pa, _ra, _pb, _rb, _pc, _rc) = lan();
        assert!(matches!(
            pa.send(&[0u8; 13]),
            Err(NetError::FrameTooShort(13))
        ));
        assert!(matches!(
            pa.send(&vec![0u8; MAX_FRAME_LEN + 1]),
            Err(NetError::FrameTooLarge(_))
        ));
    }

    #[test]
    fn a_proxy_connection_is_recognized_by_its_address_and_port_only() {
        assert!(is_proxy_connection(
            &tcp_frame(GW, A, GW_IP, PROXY_PORT),
            GW_IP
        ));
        assert!(!is_proxy_connection(&tcp_frame(GW, A, GW_IP, 80), GW_IP));
        assert!(!is_proxy_connection(
            &tcp_frame(GW, A, GW_IP, PROXY_PORT),
            Ipv4Addr::LOCALHOST
        ));
        let udp = {
            let mut f = tcp_frame(GW, A, GW_IP, PROXY_PORT);
            f[14 + 9] = 17;
            f
        };
        assert!(
            !is_proxy_connection(&udp, GW_IP),
            "UDP is not a proxy connection"
        );
        assert!(!is_proxy_connection(&frame(GW, A, b"arp"), GW_IP));
    }
}
