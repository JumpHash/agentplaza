// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//

/// `VsockMuxer` is the device-facing component of the Unix domain sockets vsock backend. I.e.
/// by implementing the `VsockBackend` trait, it abstracts away the gory details of translating
/// between AF_VSOCK and AF_UNIX, and presents a clean interface to the rest of the vsock
/// device model.
///
/// The vsock muxer has two main roles:
/// 1. Vsock connection multiplexer: It's the muxer's job to create, manage, and terminate
///    `VsockConnection` objects. The muxer also routes packets to their owning connections. It
///    does so via a connection `HashMap`, keyed by what is basically a (host_port, guest_port)
///    tuple. Vsock packet traffic needs to be inspected, in order to detect connection request
///    packets (leading to the creation of a new connection), and connection reset packets
///    (leading to the termination of an existing connection). All other packets, though, must
///    belong to an existing connection and, as such, the muxer simply forwards them.
/// 2. Event dispatcher There are three event categories that the vsock backend is interested
///    it:
///    1. A new host-initiated connection is ready to be accepted from the listening host Unix
///       socket;
///    2. Data is available for reading from a newly-accepted host-initiated connection (i.e.
///       the host is ready to issue a vsock connection request, informing us of the
///       destination port to which it wants to connect);
///    3. Some event was triggered for a connected Unix socket, that belongs to a
///       `VsockConnection`.
///
///  The muxer gets notified about all of these events, because, as a `VsockEpollListener`
///  implementor, it gets to register a nested epoll FD into the main VMM epolling loop. All
///  other pollable FDs are then registered under this nested epoll FD.
///  To route all these events to their handlers, the muxer uses another `HashMap` object,
///  mapping `RawFd`s to `EpollListener`s.
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::io::Read;
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};

use vmm_sys_util::epoll::{ControlOperation, Epoll, EpollEvent, EventSet};

use super::super::csm::ConnState;
use super::super::defs::uapi;
use super::super::{VsockBackend, VsockChannel, VsockEpollListener, VsockError};
use super::muxer_killq::MuxerKillQ;
use super::muxer_rxq::MuxerRxQ;
use super::{MuxerConnection, VsockUnixBackendError, defs};
use crate::devices::virtio::vsock::metrics::METRICS;
use crate::devices::virtio::vsock::packet::{VsockPacketRx, VsockPacketTx};
use crate::devices::virtio::vsock::metrics::IncMetric;

/// A unique identifier of a `MuxerConnection` object. Connections are stored in a hash map,
/// keyed by a `ConnMapKey` object.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ConnMapKey {
    local_port: u32,
    peer_port: u32,
}

/// A muxer RX queue item.
#[derive(Clone, Copy, Debug)]
pub enum MuxerRx {
    /// The packet must be fetched from the connection identified by `ConnMapKey`.
    ConnRx(ConnMapKey),
    /// The muxer must produce an RST packet.
    RstPkt { local_port: u32, peer_port: u32 },
}

/// An epoll listener, registered under the muxer's nested epoll FD.
#[derive(Debug)]
enum EpollListener {
    /// The listener is a `MuxerConnection`, identified by `key`, and interested in the events
    /// in `evset`. Since `MuxerConnection` implements `VsockEpollListener`, notifications will
    /// be forwarded to the listener via `VsockEpollListener::notify()`.
    Connection { key: ConnMapKey, evset: EventSet },
    /// A listener interested in new host-initiated connections.
    HostSock,
    /// A listener interested in reading host `connect <port>` commands from a freshly
    /// connected host socket.
    LocalStream(UnixStream),
}

/// The vsock connection multiplexer.
#[derive(Debug)]
pub struct VsockMuxer {
    /// Guest CID.
    cid: u64,
    /// A hash map used to store the active connections.
    conn_map: HashMap<ConnMapKey, MuxerConnection>,
    /// A hash map used to store epoll event listeners / handlers.
    listener_map: HashMap<RawFd, EpollListener>,
    /// The RX queue. Items in this queue are consumed by `VsockMuxer::recv_pkt()`, and
    /// produced
    /// - by `VsockMuxer::send_pkt()` (e.g. RST in response to a connection request packet); and
    /// - in response to EPOLLIN events (e.g. data available to be read from an AF_UNIX socket).
    rxq: MuxerRxQ,
    /// A queue used for terminating connections that are taking too long to shut down.
    killq: MuxerKillQ,
    /// The Unix socket, through which host-initiated connections are accepted.
    host_sock: UnixListener,
    /// The file system path of the host-side Unix socket. This is used to figure out the path
    /// to Unix sockets listening on specific ports. I.e. `"<this path>_<port number>"`.
    pub(crate) host_sock_path: String,
    /// The nested epoll event set, used to register epoll listeners.
    epoll: Epoll,
    /// A hash set used to keep track of used host-side (local) ports, in order to assign local
    /// ports to host-initiated connections.
    local_port_set: HashSet<u32>,
    /// The last used host-side port.
    ///
    /// Local ports are allocated in a round-robin fashion within the range [1 << 30, 1 << 31).
    /// There should be no inherent technical requirement for this specific range. But the range
    /// provides 1 billion available ports, making port collisions unlikely. In addition, the
    /// most significant bits are fixed to 01, which may facilitate debugging and identification.
    /// This appears to have been a design decision dating back to the initial introduction of the
    /// vsock implementation.
    pub(crate) local_port_last: u32,
}

impl VsockChannel for VsockMuxer {
    /// Deliver a vsock packet to the guest vsock driver.
    ///
    /// Retuns:
    /// - `Ok(())`: `pkt` has been successfully filled in; or
    /// - `Err(VsockError::NoData)`: there was no available data with which to fill in the packet.
    fn recv_pkt(&mut self, pkt: &mut VsockPacketRx) -> Result<(), VsockError> {
        // We'll look for instructions on how to build the RX packet in the RX queue. If the
        // queue is empty, that doesn't necessarily mean we don't have any pending RX, since
        // the queue might be out-of-sync. If that's the case, we'll attempt to sync it first,
        // and then try to pop something out again.
        if self.rxq.is_empty() && !self.rxq.is_synced() {
            self.rxq = MuxerRxQ::from_conn_map(&self.conn_map);
        }

        while let Some(rx) = self.rxq.peek() {
            let res = match rx {
                // We need to build an RST packet, going from `local_port` to `peer_port`.
                MuxerRx::RstPkt {
                    local_port,
                    peer_port,
                } => {
                    pkt.hdr
                        .set_op(uapi::VSOCK_OP_RST)
                        .set_src_cid(uapi::VSOCK_HOST_CID)
                        .set_dst_cid(self.cid)
                        .set_src_port(local_port)
                        .set_dst_port(peer_port)
                        .set_len(0)
                        .set_type(uapi::VSOCK_TYPE_STREAM)
                        .set_flags(0)
                        .set_buf_alloc(0)
                        .set_fwd_cnt(0);
                    self.rxq.pop().unwrap();
                    return Ok(());
                }

                // We'll defer building the packet to this connection, since it has something
                // to say.
                MuxerRx::ConnRx(key) => {
                    let mut conn_res = Err(VsockError::NoData);
                    let mut do_pop = true;
                    self.apply_conn_mutation(key, |conn| {
                        conn_res = conn.recv_pkt(pkt);
                        do_pop = !conn.has_pending_rx();
                    });
                    if do_pop {
                        self.rxq.pop().unwrap();
                    }
                    conn_res
                }
            };

            if res.is_ok() {
                // Inspect traffic, looking for RST packets, since that means we have to
                // terminate and remove this connection from the active connection pool.
                //
                if pkt.hdr.op() == uapi::VSOCK_OP_RST {
                    self.remove_connection(ConnMapKey {
                        local_port: pkt.hdr.src_port(),
                        peer_port: pkt.hdr.dst_port(),
                    });
                }


                return Ok(());
            }
        }

        Err(VsockError::NoData)
    }

    /// Deliver a guest-generated packet to its destination in the vsock backend.
    ///
    /// This absorbs unexpected packets, handles RSTs (by dropping connections), and forwards
    /// all the rest to their owning `MuxerConnection`.
    ///
    /// The packet is always consumed, and its virtio TX buffers can be returned to the guest
    /// vsock driver.
    fn send_pkt(&mut self, pkt: &VsockPacketTx) {
        let conn_key = ConnMapKey {
            local_port: pkt.hdr.dst_port(),
            peer_port: pkt.hdr.src_port(),
        };



        // If this packet has an unsupported type (!=stream), we must send back an RST.
        //
        if pkt.hdr.type_() != uapi::VSOCK_TYPE_STREAM {
            self.enq_rst(pkt.hdr.dst_port(), pkt.hdr.src_port());
            return;
        }

        // We don't know how to handle packets addressed to other CIDs. We only handle the host
        // part of the guest - host communication here.
        if pkt.hdr.dst_cid() != uapi::VSOCK_HOST_CID {

            return;
        }

        if !self.conn_map.contains_key(&conn_key) {
            // This packet can't be routed to any active connection (based on its src and dst
            // ports).  The only orphan / unroutable packets we know how to handle are
            // connection requests.
            if pkt.hdr.op() == uapi::VSOCK_OP_REQUEST {
                // Oh, this is a connection request!
                self.handle_peer_request_pkt(pkt);
            } else {
                // Send back an RST, to let the drive know we weren't expecting this packet.
                self.enq_rst(pkt.hdr.dst_port(), pkt.hdr.src_port());
            }
            return;
        }

        // Right, we know where to send this packet, then (to `conn_key`).
        // However, if this is an RST, we have to forcefully terminate the connection, so
        // there's no point in forwarding it the packet.
        if pkt.hdr.op() == uapi::VSOCK_OP_RST {
            self.remove_connection(conn_key);
            return;
        }

        // Alright, everything looks in order - forward this packet to its owning connection.
        self.apply_conn_mutation(conn_key, |conn| {
            conn.send_pkt(pkt);
        });
    }

    /// Check if the muxer has any pending RX data, with which to fill a guest-provided RX
    /// buffer.
    fn has_pending_rx(&self) -> bool {
        !self.rxq.is_empty() || !self.rxq.is_synced()
    }
}

impl AsRawFd for VsockMuxer {
    /// Get the FD to be registered for polling upstream (in the main VMM epoll loop, in this
    /// case).
    ///
    /// This will be the muxer's nested epoll FD.
    fn as_raw_fd(&self) -> RawFd {
        self.epoll.as_raw_fd()
    }
}

impl VsockEpollListener for VsockMuxer {
    /// Get the epoll events to be polled upstream.
    ///
    /// Since the polled FD is a nested epoll FD, we're only interested in EPOLLIN events (i.e.
    /// some event occurred on one of the FDs registered under our epoll FD).
    fn get_polled_evset(&self) -> EventSet {
        EventSet::IN
    }

    /// Notify the muxer about a pending event having occurred under its nested epoll FD.
    fn notify(&mut self, _: EventSet) {
        let mut epoll_events = vec![EpollEvent::new(EventSet::empty(), 0); 32];
        match self.epoll.wait(0, epoll_events.as_mut_slice()) {
            Ok(ev_cnt) => {
                for ev in &epoll_events[0..ev_cnt] {
                    self.handle_event(
                        ev.fd(),
                        // It's ok to unwrap here, since the `epoll_events[i].events` is filled
                        // in by `epoll::wait()`, and therefore contains only valid epoll
                        // flags.
                        EventSet::from_bits(ev.events).unwrap(),
                    );
                }
            }
            Err(err) => {
                eprintln!("vsock: failed to consume muxer epoll event: {}", err);
                METRICS.muxer_event_fails.inc();
            }
        }
    }
}

impl VsockBackend for VsockMuxer {
    fn activate(&mut self) -> Result<(), VsockError> {
        // Listen on the host-initiated socket, for incoming connections.
        self.add_listener(self.host_sock.as_raw_fd(), EpollListener::HostSock)
            .map_err(VsockError::VsockUdsBackend)
    }

    fn reset(&mut self) {
        // Remove all connections and their epoll listeners.
        let keys: Vec<ConnMapKey> = self.conn_map.keys().copied().collect();
        for key in keys {
            self.remove_connection(key);
        }

        let fds: Vec<RawFd> = self.listener_map.keys().copied().collect();
        for fd in fds {
            self.remove_listener(fd);
        }

        self.rxq = MuxerRxQ::new();
        self.killq = MuxerKillQ::new();
        self.local_port_set.clear();
        self.local_port_last = (1u32 << 30) - 1;
    }
}

impl VsockMuxer {
    /// Muxer constructor.
    pub fn new(cid: u64, host_sock_path: String) -> Result<Self, VsockUnixBackendError> {
        // Open/bind on the host Unix socket, so we can accept host-initiated
        // connections.
        let host_sock = UnixListener::bind(&host_sock_path)
            .and_then(|sock| sock.set_nonblocking(true).map(|_| sock))
            .map_err(VsockUnixBackendError::UnixBind)?;

        let muxer = Self {
            cid,
            host_sock,
            host_sock_path,
            epoll: Epoll::new().map_err(VsockUnixBackendError::EpollFdCreate)?,
            rxq: MuxerRxQ::new(),
            conn_map: HashMap::with_capacity(defs::MAX_CONNECTIONS),
            listener_map: HashMap::with_capacity(defs::MAX_CONNECTIONS + 1),
            killq: MuxerKillQ::new(),
            local_port_last: (1u32 << 30) - 1,
            local_port_set: HashSet::with_capacity(defs::MAX_CONNECTIONS),
        };

        Ok(muxer)
    }

    /// Return the file system path of the host-side Unix socket.
    pub fn host_sock_path(&self) -> &str {
        &self.host_sock_path
    }

    /// Handle/dispatch an epoll event to its listener.
    fn handle_event(&mut self, fd: RawFd, event_set: EventSet) {


        match self.listener_map.get_mut(&fd) {
            // This event needs to be forwarded to a `MuxerConnection` that is listening for
            // it.
            Some(EpollListener::Connection { key, evset: _ }) => {
                let key_copy = *key;
                // The handling of this event will most probably mutate the state of the
                // receiving connection. We'll need to check for new pending RX, event set
                // mutation, and all that, so we're wrapping the event delivery inside those
                // checks.
                self.apply_conn_mutation(key_copy, |conn| {
                    conn.notify(event_set);
                });
            }

            // A new host-initiated connection is ready to be accepted.
            Some(EpollListener::HostSock) => {
                if self.conn_map.len() == defs::MAX_CONNECTIONS {
                    // If we're already maxed-out on connections, we'll just accept and
                    // immediately discard this potentially new one.
                    eprintln!("vsock: connection limit reached; refusing new host connection");
                    self.host_sock.accept().map(|_| 0).unwrap_or(0);
                    return;
                }
                self.host_sock
                    .accept()
                    .map_err(VsockUnixBackendError::UnixAccept)
                    .and_then(|(stream, _)| {
                        stream
                            .set_nonblocking(true)
                            .map(|_| stream)
                            .map_err(VsockUnixBackendError::UnixAccept)
                    })
                    .and_then(|stream| {
                        // Before forwarding this connection to a listening AF_VSOCK socket on
                        // the guest side, we need to know the destination port. We'll read
                        // that port from a "connect" command received on this socket, so the
                        // next step is to ask to be notified the moment we can read from it.
                        self.add_listener(stream.as_raw_fd(), EpollListener::LocalStream(stream))
                    })
                    .unwrap_or_else(|err| {
                        eprintln!("vsock: unable to accept local connection: {:?}", err);
                    });
            }

            // Data is ready to be read from a host-initiated connection. That would be the
            // "connect" command that we're expecting.
            Some(EpollListener::LocalStream(_)) => {
                if let Some(EpollListener::LocalStream(mut stream)) = self.remove_listener(fd) {
                    Self::read_local_stream_port(&mut stream)
                        .map(|peer_port| (self.allocate_local_port(), peer_port))
                        .and_then(|(local_port, peer_port)| {
                            self.add_connection(
                                ConnMapKey {
                                    local_port,
                                    peer_port,
                                },
                                MuxerConnection::new_local_init(
                                    stream,
                                    uapi::VSOCK_HOST_CID,
                                    self.cid,
                                    local_port,
                                    peer_port,
                                ),
                            )
                        })
                        .unwrap_or_else(|err| {
                            eprintln!("vsock: error adding local-init connection: {err:?}");
                        })
                }
            }

            _ => {

                METRICS.muxer_event_fails.inc();
            }
        }
    }

    /// Parse a host "connect" command, and extract the destination vsock port.
    fn read_local_stream_port(stream: &mut UnixStream) -> Result<u32, VsockUnixBackendError> {
        let mut buf = [0u8; 32];

        // This is the minimum number of bytes that we should be able to read, when parsing a
        // valid connection request. I.e. `b"connect 0\n".len()`.
        const MIN_READ_LEN: usize = 10;

        // Bring in the minimum number of bytes that we should be able to read.
        stream
            .read_exact(&mut buf[..MIN_READ_LEN])
            .map_err(VsockUnixBackendError::UnixRead)?;

        // Now, finish reading the destination port number, by bringing in one byte at a time,
        // until we reach an EOL terminator (or our buffer space runs out).  Yeah, not
        // particularly proud of this approach, but it will have to do for now.
        let mut blen = MIN_READ_LEN;
        while buf[blen - 1] != b'\n' && blen < buf.len() {
            stream
                .read_exact(&mut buf[blen..=blen])
                .map_err(VsockUnixBackendError::UnixRead)?;
            blen += 1;
        }

        let mut word_iter = std::str::from_utf8(&buf[..blen])
            .map_err(|_| VsockUnixBackendError::InvalidPortRequest)?
            .split_whitespace();

        word_iter
            .next()
            .ok_or(VsockUnixBackendError::InvalidPortRequest)
            .and_then(|word| {
                if word.to_lowercase() == "connect" {
                    Ok(())
                } else {
                    Err(VsockUnixBackendError::InvalidPortRequest)
                }
            })
            .and_then(|_| {
                word_iter
                    .next()
                    .ok_or(VsockUnixBackendError::InvalidPortRequest)
            })
            .and_then(|word| {
                word.parse::<u32>()
                    .map_err(|_| VsockUnixBackendError::InvalidPortRequest)
            })
            .map_err(|_| VsockUnixBackendError::InvalidPortRequest)
    }

    /// Add a new connection to the active connection pool.
    fn add_connection(
        &mut self,
        key: ConnMapKey,
        conn: MuxerConnection,
    ) -> Result<(), VsockUnixBackendError> {
        // We might need to make room for this new connection, so let's sweep the kill queue
        // first.  It's fine to do this here because:
        // - unless the kill queue is out of sync, this is a pretty inexpensive operation; and
        // - we are under no pressure to respect any accurate timing for connection termination.
        self.sweep_killq();

        if self.conn_map.len() >= defs::MAX_CONNECTIONS {

            return Err(VsockUnixBackendError::TooManyConnections);
        }

        self.add_listener(
            conn.as_raw_fd(),
            EpollListener::Connection {
                key,
                evset: conn.get_polled_evset(),
            },
        )
        .map(|_| {
            if conn.has_pending_rx() {
                // We can safely ignore any error in adding a connection RX indication. Worst
                // case scenario, the RX queue will get desynchronized, but we'll handle that
                // the next time we need to yield an RX packet.
                self.rxq.push(MuxerRx::ConnRx(key));
            }
            self.conn_map.insert(key, conn);
            METRICS.conns_added.inc();
        })
    }

    /// Remove a connection from the active connection poll.
    fn remove_connection(&mut self, key: ConnMapKey) {
        if let Some(conn) = self.conn_map.remove(&key) {
            self.remove_listener(conn.as_raw_fd());
            METRICS.conns_removed.inc();
        }
        self.free_local_port(key.local_port);
    }

    /// Schedule a connection for immediate termination.
    /// I.e. as soon as we can also let our peer know we're dropping the connection, by sending
    /// it an RST packet.
    fn kill_connection(&mut self, key: ConnMapKey) {
        let mut had_rx = false;
        METRICS.conns_killed.inc();

        self.conn_map.entry(key).and_modify(|conn| {
            had_rx = conn.has_pending_rx();
            conn.kill();
        });
        // This connection will now have an RST packet to yield, so we need to add it to the RX
        // queue.  However, there's no point in doing that if it was already in the queue.
        if !had_rx {
            // We can safely ignore any error in adding a connection RX indication. Worst case
            // scenario, the RX queue will get desynchronized, but we'll handle that the next
            // time we need to yield an RX packet.
            self.rxq.push(MuxerRx::ConnRx(key));
        }
    }

    /// Register a new epoll listener under the muxer's nested epoll FD.
    fn add_listener(
        &mut self,
        fd: RawFd,
        listener: EpollListener,
    ) -> Result<(), VsockUnixBackendError> {
        let evset = match listener {
            EpollListener::Connection { evset, .. } => evset,
            EpollListener::LocalStream(_) => EventSet::IN,
            EpollListener::HostSock => EventSet::IN,
        };

        self.epoll
            .ctl(
                ControlOperation::Add,
                fd,
                EpollEvent::new(evset, u64::try_from(fd).unwrap()),
            )
            .map(|_| {
                self.listener_map.insert(fd, listener);
            })
            .map_err(VsockUnixBackendError::EpollAdd)?;

        Ok(())
    }

    /// Remove (and return) a previously registered epoll listener.
    fn remove_listener(&mut self, fd: RawFd) -> Option<EpollListener> {
        let maybe_listener = self.listener_map.remove(&fd);

        if maybe_listener.is_some() {
            self.epoll
                .ctl(ControlOperation::Delete, fd, EpollEvent::default())
                .unwrap_or_else(|err| {
                    eprintln!(
                        "vosck muxer: error removing epoll listener for fd {:?}: {:?}",
                        fd, err
                    );
                });
        }

        maybe_listener
    }

    /// Allocate a host-side port to be assigned to a new host-initiated connection.
    fn allocate_local_port(&mut self) -> u32 {
        // TODO: this doesn't seem very space-efficient.
        // Mybe rewrite this to limit port range and use a bitmap?
        //

        loop {
            self.local_port_last = (self.local_port_last + 1) & !(1 << 31) | (1 << 30);
            if self.local_port_set.insert(self.local_port_last) {
                break;
            }
        }
        self.local_port_last
    }

    /// Mark a previously used host-side port as free.
    fn free_local_port(&mut self, port: u32) {
        self.local_port_set.remove(&port);
    }

    /// Connect to the Unix socket at `path` without blocking; a full accept backlog
    /// fails with `EAGAIN`.
    ///
    /// The `socket(2)`+`connect(2)` pair lives behind the crate's audited
    /// syscall boundary ([`crate::sys::connect_unix_nonblocking`]), because it
    /// must create the descriptor non-blocking *before* connecting:
    /// `std::os::unix::net::UnixStream::connect` connects blocking first and
    /// only then sets `O_NONBLOCK`, which turns a full listener backlog into a
    /// parked thread. That thread is the VM's vsock event thread — the muxer
    /// admits up to `defs::MAX_CONNECTIONS` (1023) guest connections while a
    /// host listener's default backlog is 128 — so the guest would lose all
    /// vsock RX/TX until the listener accepted.
    fn connect_nonblocking(path: &str) -> std::io::Result<UnixStream> {
        crate::sys::connect_unix_nonblocking(path)
    }

    /// Handle a new connection request comming from our peer (the guest vsock driver).
    ///
    /// This will attempt to connect to a host-side Unix socket, expected to be listening at
    /// the file system path corresponing to the destination port. If successful, a new
    /// connection object will be created and added to the connection pool. On failure, a new
    /// RST packet will be scheduled for delivery to the guest.
    fn handle_peer_request_pkt(&mut self, pkt: &VsockPacketTx) {
        let port_path = format!("{}_{}", self.host_sock_path, pkt.hdr.dst_port());

        Self::connect_nonblocking(&port_path)
            .map_err(VsockUnixBackendError::UnixConnect)
            .and_then(|stream| {
                self.add_connection(
                    ConnMapKey {
                        local_port: pkt.hdr.dst_port(),
                        peer_port: pkt.hdr.src_port(),
                    },
                    MuxerConnection::new_peer_init(
                        stream,
                        uapi::VSOCK_HOST_CID,
                        self.cid,
                        pkt.hdr.dst_port(),
                        pkt.hdr.src_port(),
                        pkt.hdr.buf_alloc(),
                    ),
                )
            })
            .unwrap_or_else(|_| self.enq_rst(pkt.hdr.dst_port(), pkt.hdr.src_port()));
    }

    /// Perform an action that might mutate a connection's state.
    ///
    /// This is used as shorthand for repetitive tasks that need to be performed after a
    /// connection object mutates. E.g.
    /// - update the connection's epoll listener;
    /// - schedule the connection to be queried for RX data;
    /// - kill the connection if an unrecoverable error occurs.
    fn apply_conn_mutation<F>(&mut self, key: ConnMapKey, mut_fn: F)
    where
        F: FnOnce(&mut MuxerConnection),
    {
        if let Some(conn) = self.conn_map.get_mut(&key) {
            let had_rx = conn.has_pending_rx();
            let was_expiring = conn.will_expire();
            let prev_state = conn.state();

            mut_fn(conn);

            // If this is a host-initiated connection that has just become established, we'll have
            // to send an ack message to the host end.
            if prev_state == ConnState::LocalInit && conn.state() == ConnState::Established {
                let msg = format!("OK {}\n", key.local_port);
                match conn.send_bytes_raw(msg.as_bytes()) {
                    Ok(written) if written == msg.len() => (),
                    Ok(_) => {
                        // If we can't write a dozen bytes to a pristine connection something
                        // must be really wrong. Killing it.
                        conn.kill();
                        eprintln!("vsock: unable to fully write connection ack msg.");
                    }
                    Err(err) => {
                        conn.kill();
                        eprintln!("vsock: unable to ack host connection: {:?}", err);
                    }
                };
            }

            // If the connection wasn't previously scheduled for RX, add it to our RX queue.
            if !had_rx && conn.has_pending_rx() {
                self.rxq.push(MuxerRx::ConnRx(key));
            }

            // If the connection wasn't previously scheduled for termination, add it to the
            // kill queue.
            if !was_expiring && conn.will_expire() {
                // It's safe to unwrap here, since `conn.will_expire()` already guaranteed that
                // an `conn.expiry` is available.
                self.killq.push(key, conn.expiry().unwrap());
            }

            let fd = conn.as_raw_fd();
            let new_evset = conn.get_polled_evset();
            if new_evset.is_empty() {
                // If the connection no longer needs epoll notifications, remove its listener
                // from our list.
                self.remove_listener(fd);
                return;
            }
            if let Some(EpollListener::Connection { evset, .. }) = self.listener_map.get_mut(&fd) {
                if *evset != new_evset {
                    // If the set of events that the connection is interested in has changed,
                    // we need to update its epoll listener.


                    *evset = new_evset;
                    self.epoll
                        .ctl(
                            ControlOperation::Modify,
                            fd,
                            EpollEvent::new(new_evset, u64::try_from(fd).unwrap()),
                        )
                        .unwrap_or_else(|err| {
                            // This really shouldn't happen, like, ever. However, "famous last
                            // words" and all that, so let's just kill it with fire, and walk away.
                            self.kill_connection(key);
                            eprintln!(
                                "vsock: error updating epoll listener for (lp={}, pp={}): {:?}",
                                key.local_port, key.peer_port, err
                            );
                            METRICS.muxer_event_fails.inc();
                        });
                }
            } else {
                // The connection had previously asked to be removed from the listener map (by
                // returning an empty event set via `get_polled_fd()`), but now wants back in.
                self.add_listener(
                    fd,
                    EpollListener::Connection {
                        key,
                        evset: new_evset,
                    },
                )
                .unwrap_or_else(|err| {
                    self.kill_connection(key);
                    eprintln!(
                        "vsock: error updating epoll listener for (lp={}, pp={}): {:?}",
                        key.local_port, key.peer_port, err
                    );
                    METRICS.muxer_event_fails.inc();
                });
            }
        }
    }

    /// Check if any connections have timed out, and if so, schedule them for immediate
    /// termination.
    fn sweep_killq(&mut self) {
        while let Some(key) = self.killq.pop() {
            // Connections don't get removed from the kill queue when their kill timer is
            // disarmed, since that would be a costly operation. This means we must check if
            // the connection has indeed expired, prior to killing it.
            let mut kill = false;
            self.conn_map
                .entry(key)
                .and_modify(|conn| kill = conn.has_expired());
            if kill {
                self.kill_connection(key);
            }
        }

        if self.killq.is_empty() && !self.killq.is_synced() {
            self.killq = MuxerKillQ::from_conn_map(&self.conn_map);
            METRICS.killq_resync.inc();
            // If we've just re-created the kill queue, we can sweep it again; maybe there's
            // more to kill.
            self.sweep_killq();
        }
    }

    /// Enqueue an RST packet into `self.rxq`.
    ///
    /// Enqueue errors aren't propagated up the call chain, since there is nothing we can do to
    /// handle them. We do, however, log a warning, since not being able to enqueue an RST
    /// packet means we have to drop it, which is not normal operation.
    fn enq_rst(&mut self, local_port: u32, peer_port: u32) {
        let pushed = self.rxq.push(MuxerRx::RstPkt {
            local_port,
            peer_port,
        });
        if !pushed {
            eprintln!(
                "vsock: muxer.rxq full; dropping RST packet for lp={}, pp={}",
                local_port, peer_port
            );
        }
    }
}

impl Drop for VsockMuxer {
    fn drop(&mut self) {
        // The Unix socket the muxer bound lives in the filesystem, not in the
        // process: close does not unlink it. Removing it here means a VM whose
        // muxer is dropped (a clean stop) leaves no stale path behind for the
        // next VM that reuses the CID. A VM killed by SIGKILL still leaves one
        // behind, which is why a re-boot over a stale path fails with a clear
        // "address already in use" rather than silently stealing a live CID.
        let _ = std::fs::remove_file(&self.host_sock_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The F2 security property: a guest packet addressed to any CID other
    /// than the host (2) is dropped, not routed — cross-guest traffic is
    /// structurally impossible because no such packet ever leaves the muxer.
    #[test]
    fn a_packet_addressed_to_another_cid_is_dropped_not_routed() {
        let path = format!("/tmp/plaza-test-muxer-{}", std::process::id());
        let mut muxer = VsockMuxer::new(3, path.clone()).expect("muxer binds its host socket");

        let mut pkt = VsockPacketTx::default();
        pkt.hdr
            .set_src_cid(3)
            .set_dst_cid(4)
            .set_src_port(1000)
            .set_dst_port(1000)
            .set_len(0)
            .set_type(uapi::VSOCK_TYPE_STREAM)
            .set_op(uapi::VSOCK_OP_RW)
            .set_flags(0)
            .set_buf_alloc(0)
            .set_fwd_cnt(0);

        muxer.send_pkt(&pkt);

        // Nothing was enqueued for the guest: no RST, no connection, no data.
        let mut rx = VsockPacketRx::new().expect("an RX packet buffer");
        assert!(
            matches!(muxer.recv_pkt(&mut rx), Err(VsockError::NoData)),
            "a cross-CID packet must not yield any RX for the guest"
        );

        drop(muxer);
        let _ = std::fs::remove_file(&path);
    }

    /// A guest `REQUEST` packet, from `peer_port` to `local_port`, addressed to
    /// the host CID — the packet shape that makes the muxer connect to
    /// `{uds_path}_{local_port}`.
    fn peer_request(local_port: u32, peer_port: u32) -> VsockPacketTx {
        let mut pkt = VsockPacketTx::default();
        pkt.hdr
            .set_src_cid(3)
            .set_dst_cid(uapi::VSOCK_HOST_CID)
            .set_src_port(peer_port)
            .set_dst_port(local_port)
            .set_len(0)
            .set_type(uapi::VSOCK_TYPE_STREAM)
            .set_op(uapi::VSOCK_OP_REQUEST)
            .set_flags(0)
            .set_buf_alloc(0)
            .set_fwd_cnt(0);
        pkt
    }

    /// P1 regression: a guest-initiated connect that finds the host listener's
    /// accept backlog full must fail — the guest gets an RST — instead of
    /// blocking the muxer, which is the VM's vsock event thread.
    ///
    /// Adapted from Firecracker's `test_peer_connection_backlog_full`
    /// (`src/vmm/src/devices/virtio/vsock/unix/muxer.rs`): the listener's queue
    /// is filled with un-accepted connects, a guest `REQUEST` must then be
    /// answered with an RST, and — once the queue has drained — a second
    /// `REQUEST` must be accepted normally, proving the RST was the full queue
    /// and not a broken helper. The muxer allows 1023 connections while
    /// `UnixListener`'s default backlog is 128, so the pre-fix code (a
    /// blocking `UnixStream::connect`) parked this thread inside `connect(2)`
    /// for as long as the listener did not accept: no vsock RX/TX for the VM,
    /// and `VmHandle::kill` timing out while the VM kept running. This test
    /// hangs (it blocks inside the fill loop) without the non-blocking
    /// pre-connect.
    #[test]
    fn a_full_listener_backlog_fails_the_peer_connect_instead_of_blocking() {
        const LOCAL_PORT: u32 = 1026;
        const PEER_PORT: u32 = 1025;

        let path = format!("/tmp/plaza-test-muxer-backlog-{}", std::process::id());
        let listener_path = format!("{path}_{LOCAL_PORT}");
        // A listener at `{uds_path}_{port}` is the host service the guest's
        // `connect(CID 2, LOCAL_PORT)` is translated into; it never accepts.
        let listener = UnixListener::bind(&listener_path).expect("test listener binds");
        let mut muxer = VsockMuxer::new(3, path.clone()).expect("muxer binds its host socket");

        // Fill the accept queue (std's default backlog of 128) with connects
        // nobody accepts. The next connect must report `WouldBlock`: it is the
        // muxer's own helper, i.e. exactly what a guest `REQUEST` calls.
        let mut pending = Vec::new();
        loop {
            match VsockMuxer::connect_nonblocking(&listener_path) {
                Ok(stream) => pending.push(stream),
                Err(err) => {
                    assert_eq!(
                        err.kind(),
                        std::io::ErrorKind::WouldBlock,
                        "filling the accept queue must end in EAGAIN, not {err:?}"
                    );
                    break;
                }
            }
        }

        // The guest connects into the full queue: no connection is created and
        // the muxer answers with an RST.
        muxer.send_pkt(&peer_request(LOCAL_PORT, PEER_PORT));
        assert!(muxer.conn_map.is_empty());
        let mut rx = VsockPacketRx::new().expect("an RX packet buffer");
        muxer.recv_pkt(&mut rx).expect("the RST is pending for the guest");
        assert_eq!(rx.hdr.op(), uapi::VSOCK_OP_RST);
        assert_eq!(rx.hdr.src_port(), LOCAL_PORT);
        assert_eq!(rx.hdr.dst_port(), PEER_PORT);

        // Drain the queue, keeping the queued connects alive until they are
        // accepted: the same `REQUEST` now connects, and the muxer answers with
        // a RESPONSE.
        for _ in &pending {
            let _ = listener.accept();
        }
        muxer.send_pkt(&peer_request(LOCAL_PORT, PEER_PORT));
        assert_eq!(muxer.conn_map.len(), 1);
        let mut rx = VsockPacketRx::new().expect("an RX packet buffer");
        muxer.recv_pkt(&mut rx).expect("the RESPONSE is pending for the guest");
        assert_eq!(rx.hdr.op(), uapi::VSOCK_OP_RESPONSE);
        assert_eq!(rx.hdr.src_port(), LOCAL_PORT);
        assert_eq!(rx.hdr.dst_port(), PEER_PORT);

        drop(muxer);
        drop(pending);
        let _ = std::fs::remove_file(&listener_path);
    }
}
