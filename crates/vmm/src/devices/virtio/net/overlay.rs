// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port (PLAN 22.6, W58, 17.4). A segment that spans nodes is one switch per node,
// each with a trunk for the segment; the trunk carries the segment's frames over a byte stream. In
// production that stream is the node tunnel (the mesh of PLAN 17.4); the tests use a loopback TCP
// connection as the stand-in. The frames are the same bytes the guests exchange, so the overlay adds
// nothing a guest could see.

//! The overlay: one segment's frames between two switches over a byte stream.
//!
//! A trunk frames each Ethernet frame as `u32` big-endian body length, then the body: a version byte
//! ([`VERSION`]), the segment's name (a length byte and the name), and the frame. The receiving side
//! refuses a body of another version or another segment, and a body larger than a frame can be, so a
//! corrupt or hostile stream ends the trunk rather than feeding the switch.
//!
//! A trunk is a port of the switch ([`super::switch::Endpoint::Trunk`]). Its frames go in as the
//! segment's remote side and are never sent to another trunk, so a switch joined to several others
//! (a star) forwards no frame around a loop.

use std::io::{self, Read, Write};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use super::switch::{Endpoint, Switch};
use super::{MAX_FRAME_LEN, NetError};

/// The trunk's wire version.
pub const VERSION: u8 = 1;

/// The largest body a trunk accepts: the version, the name's length and name, and a frame.
const MAX_BODY_LEN: usize = 2 + u8::MAX as usize + MAX_FRAME_LEN;

/// The body of one frame: version, segment name, frame.
fn encode(segment: &str, frame: &[u8]) -> Result<Vec<u8>, NetError> {
    let name = segment.as_bytes();
    let name_len = u8::try_from(name.len())
        .map_err(|_| NetError::Overlay(format!("the segment name {segment:?} is too long")))?;
    let body_len = 2 + name.len() + frame.len();
    if body_len > MAX_BODY_LEN {
        return Err(NetError::FrameTooLarge(frame.len()));
    }
    let mut body = Vec::with_capacity(4 + body_len);
    body.extend_from_slice(&(body_len as u32).to_be_bytes());
    body.push(VERSION);
    body.push(name_len);
    body.extend_from_slice(name);
    body.extend_from_slice(frame);
    Ok(body)
}

/// The frame a body carries, once its version and segment are checked.
fn decode<'a>(segment: &str, body: &'a [u8]) -> Result<&'a [u8], NetError> {
    let [version, name_len, rest @ ..] = body else {
        return Err(NetError::Overlay(
            "a trunk body is shorter than its header".into(),
        ));
    };
    if *version != VERSION {
        return Err(NetError::Overlay(format!(
            "a trunk body is of version {version}, not {VERSION}"
        )));
    }
    let name_len = usize::from(*name_len);
    if rest.len() < name_len || &rest[..name_len] != segment.as_bytes() {
        return Err(NetError::Overlay(format!(
            "a trunk body is not for the segment {segment:?}"
        )));
    }
    Ok(&rest[name_len..])
}

/// The threads of a trunk: the one that reads the stream into the switch, and the one that writes
/// the switch's frames for the segment to the stream.
#[derive(Debug)]
pub struct Trunk {
    reader: JoinHandle<Result<(), NetError>>,
    writer: JoinHandle<Result<(), NetError>>,
}

impl Trunk {
    /// Waits for the trunk to end: when the stream closes, the reader ends and the writer follows.
    /// Returns the reader's error, if the stream sent something the trunk refuses.
    pub fn join(self) -> Result<(), NetError> {
        let read = self
            .reader
            .join()
            .unwrap_or_else(|_| Err(NetError::Overlay("the trunk's reader panicked".into())));
        let wrote = self
            .writer
            .join()
            .unwrap_or_else(|_| Err(NetError::Overlay("the trunk's writer panicked".into())));
        read.and(wrote)
    }
}

/// Joins `switch`'s `segment` to the stream whose read half is `reader` and write half is `writer`.
/// The segment must exist on the switch. The trunk runs until its stream ends.
pub fn connect<R, W>(
    switch: &Arc<Switch>,
    segment: &str,
    mut reader: R,
    mut writer: W,
) -> Result<Trunk, NetError>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    let (port, frames) = switch.attach(segment, Endpoint::Trunk)?;
    let writer_segment = segment.to_owned();
    let writer = thread::Builder::new()
        .name("net-trunk-write".into())
        .spawn(move || -> Result<(), NetError> {
            // The channel ends when the reader drops the port, which it does when the stream ends.
            for frame in frames {
                let body = encode(&writer_segment, &frame)?;
                writer
                    .write_all(&body)
                    .and_then(|()| writer.flush())
                    .map_err(|err| NetError::Overlay(format!("the stream broke: {err}")))?;
            }
            Ok(())
        })
        .map_err(|err| NetError::Overlay(format!("the writer thread: {err}")))?;

    let reader_segment = segment.to_owned();
    let reader = thread::Builder::new()
        .name("net-trunk-read".into())
        .spawn(move || -> Result<(), NetError> {
            let port = port;
            loop {
                let mut len = [0u8; 4];
                match reader.read_exact(&mut len) {
                    Ok(()) => {}
                    Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
                    Err(err) => return Err(NetError::Overlay(format!("the stream broke: {err}"))),
                }
                let body_len = u32::from_be_bytes(len) as usize;
                if body_len > MAX_BODY_LEN {
                    return Err(NetError::Overlay(format!(
                        "a trunk body of {body_len} bytes is larger than a frame"
                    )));
                }
                let mut body = vec![0u8; body_len];
                reader
                    .read_exact(&mut body)
                    .map_err(|err| NetError::Overlay(format!("the stream broke: {err}")))?;
                let frame = decode(&reader_segment, &body)?;
                port.send(frame)?;
            }
        })
        .map_err(|err| NetError::Overlay(format!("the reader thread: {err}")))?;

    Ok(Trunk { reader, writer })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::virtio::net::switch::Endpoint;
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc::Receiver;
    use std::time::Duration;

    const A: [u8; 6] = [0x02, 0, 0, 0, 0, 0x0a];
    const B: [u8; 6] = [0x02, 0, 0, 0, 0, 0x0b];
    const C: [u8; 6] = [0x02, 0, 0, 0, 0, 0x0c];

    fn frame(dst: [u8; 6], src: [u8; 6], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&dst);
        out.extend_from_slice(&src);
        out.extend_from_slice(&[0x88, 0xb5]);
        out.extend_from_slice(payload);
        out
    }

    fn got(rx: &Receiver<Vec<u8>>) -> Option<Vec<u8>> {
        rx.recv_timeout(Duration::from_secs(5)).ok()
    }

    /// A connected loopback pair: (the client's stream, the server's stream).
    fn pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }

    fn join(stream: &TcpStream, segment: &str, switch: &Arc<Switch>) -> Trunk {
        connect(
            switch,
            segment,
            stream.try_clone().unwrap(),
            stream.try_clone().unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn a_body_round_trips_and_a_wrong_segment_or_version_is_refused() {
        let body = encode("seg", b"frame bytes").unwrap();
        assert_eq!(
            u32::from_be_bytes(body[..4].try_into().unwrap()) as usize,
            body.len() - 4
        );
        assert_eq!(decode("seg", &body[4..]).unwrap(), b"frame bytes");
        assert!(
            decode("other", &body[4..]).is_err(),
            "another segment is refused"
        );
        let mut wrong_version = body.clone();
        wrong_version[4] = 9;
        assert!(decode("seg", &wrong_version[4..]).is_err());
        assert!(
            decode("seg", &[VERSION]).is_err(),
            "a truncated header is refused"
        );
        assert!(
            encode(&"x".repeat(300), b"f").is_err(),
            "a name longer than a byte is refused"
        );
    }

    #[test]
    fn frames_cross_the_overlay_between_two_switches_in_both_directions() {
        let (client, server) = pair();
        let left = Switch::new();
        left.create_segment("seg");
        let right = Switch::new();
        right.create_segment("seg");
        let (pa, ra) = left.attach("seg", Endpoint::Guest { mac: A }).unwrap();
        let (pb, rb) = right.attach("seg", Endpoint::Guest { mac: B }).unwrap();
        let _left_trunk = join(&client, "seg", &left);
        let _right_trunk = join(&server, "seg", &right);

        // A's frame to B: B's MAC is not on the left, so it floods to the trunk and crosses.
        let there = frame(B, A, b"to b");
        assert!(pa.send(&there).is_ok());
        assert_eq!(got(&rb), Some(there));

        // B answers A: the right switch learned A behind its trunk, so the reply crosses too.
        let back = frame(A, B, b"to a");
        assert!(pb.send(&back).is_ok());
        assert_eq!(got(&ra), Some(back));

        client.shutdown(std::net::Shutdown::Both).ok();
        server.shutdown(std::net::Shutdown::Both).ok();
    }

    #[test]
    fn a_segment_on_the_far_side_that_the_trunk_does_not_carry_hears_nothing() {
        let (client, server) = pair();
        let left = Switch::new();
        left.create_segment("seg");
        let right = Switch::new();
        right.create_segment("seg");
        right.create_segment("other");
        let (pa, _ra) = left.attach("seg", Endpoint::Guest { mac: A }).unwrap();
        let (_px, rx_other) = right.attach("other", Endpoint::Guest { mac: C }).unwrap();
        let (_pb, rb) = right.attach("seg", Endpoint::Guest { mac: B }).unwrap();
        let _left_trunk = join(&client, "seg", &left);
        let _right_trunk = join(&server, "seg", &right);

        let broadcast = frame([0xff; 6], A, b"hello all");
        assert!(pa.send(&broadcast).is_ok());
        assert_eq!(got(&rb), Some(broadcast));
        assert_eq!(
            got(&rx_other),
            None,
            "the other segment never hears the trunk"
        );

        client.shutdown(std::net::Shutdown::Both).ok();
        server.shutdown(std::net::Shutdown::Both).ok();
    }

    #[test]
    fn a_trunk_never_transits_frames_between_two_trunks() {
        // A star: the middle switch has a trunk to each of two other switches. A frame from the
        // first must not come out of the second, or two nodes would loop through the hub.
        let (hub_to_left, left_end) = pair();
        let (hub_to_right, right_end) = pair();
        let hub = Switch::new();
        hub.create_segment("seg");
        let left = Switch::new();
        left.create_segment("seg");
        let right = Switch::new();
        right.create_segment("seg");
        let (pa, _ra) = left.attach("seg", Endpoint::Guest { mac: A }).unwrap();
        let (_pc, rc) = right.attach("seg", Endpoint::Guest { mac: C }).unwrap();
        let _t1 = join(&left_end, "seg", &left);
        let _t2 = join(&right_end, "seg", &right);
        let _h1 = join(&hub_to_left, "seg", &hub);
        let _h2 = join(&hub_to_right, "seg", &hub);

        // C's MAC is not known on the left, so the frame floods; the hub takes it from the left trunk
        // and gives it to no other trunk.
        assert!(pa.send(&frame(C, A, b"through the hub")).is_ok());
        assert_eq!(
            got(&rc),
            None,
            "the hub does not carry a frame from one trunk to another"
        );

        for stream in [&hub_to_left, &hub_to_right, &left_end, &right_end] {
            stream.shutdown(std::net::Shutdown::Both).ok();
        }
    }
}
