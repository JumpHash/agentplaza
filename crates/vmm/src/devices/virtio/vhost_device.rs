// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port from the virtio specification (virtio 1.0 §5.10,
// virtio-vsock): Firecracker's vsock device implements the protocol in
// userspace, which this port does not — the guest's packets are carried by the
// host kernel's vhost-vsock backend ([`super::vhost`]) instead, so what is left
// here is the device's identity (type id, queues, features) and the device
// model the MMIO transport drives.
//
// This was `vsock.rs` before the userspace vsock core landed; the module was
// renamed so `vsock` could become the userspace device. The vhost device keeps
// booting (the boot-path cutover to the userspace device is the next slice),
// so the two device flavours coexist until then.

//! The virtio-vsock device: the guest's `AF_VSOCK` endpoint.
//!
//! The device is thin on purpose. It owns the three queues of a virtio-vsock
//! device and the feature set the host's backend offers, and on activation it
//! hands the rings the driver programmed to that backend. From then on the
//! guest's packets never enter this process: the kernel's `vhost_vsock` driver
//! reads the transmit ring, delivers the packets to the host's vsock stack
//! (where the orchestrator listens on CID 2) and puts replies on the receive
//! ring, signalling the guest through this device's interrupt. The device's
//! only job per notification is telling the backend that a queue has buffers.

use vmm_sys_util::eventfd::EventFd;

use crate::devices::virtio::VirtioError;
use crate::devices::virtio::mmio::{QueueConfig, VirtioDevice};
use crate::devices::virtio::vhost::VhostVsock;
use crate::vstate::memory::GuestMemoryMmap;

/// `VIRTIO_ID_VSOCK`: the device type id of a virtio-vsock device.
const VIRTIO_ID_VSOCK: u32 = 19;

/// `VIRTIO_F_VERSION_1` (feature bit 32): the feature a modern device must
/// offer, and the one the vhost interface's ring handling needs.
pub(crate) const VIRTIO_F_VERSION_1: u64 = 1 << 32;

/// `VIRTIO_VSOCK_F_SEQPACKET` (feature bit 1): the guest may use
/// `SOCK_SEQPACKET` sockets.
const VIRTIO_VSOCK_F_SEQPACKET: u64 = 1 << 1;

/// `VIRTIO_RING_F_INDIRECT_DESC` (feature bit 28): descriptor chains may be
/// indirect.
const VIRTIO_RING_F_INDIRECT_DESC: u64 = 1 << 28;

/// `VIRTIO_RING_F_EVENT_IDX` (feature bit 29): used/available event indices may
/// be used to suppress interrupts. The kernel's vhost driver implements it.
const VIRTIO_RING_F_EVENT_IDX: u64 = 1 << 29;

/// The smallest CID a guest may be given, as the spawner's public contract
/// states it (the kernel's vsock stack enforces the same reservation).
pub(crate) use crate::spawner::MIN_GUEST_CID;

/// The features this port forwards between the driver and the backend: the bits
/// both ends understand and this VMM can honour.
///
/// The backend's own feature set is masked with this before it is offered to
/// the driver, which is what keeps `VIRTIO_F_ACCESS_PLATFORM` (it would make
/// the backend expect an IOTLB, i.e. a vIOMMU) and `VHOST_F_LOG_ALL` (dirty
/// logging, for snapshots) out of the negotiation: neither exists in this port,
/// and a feature acknowledged but not honoured is worse than one never offered.
pub(crate) const VSOCK_FEATURES: u64 = VIRTIO_F_VERSION_1
    | VIRTIO_VSOCK_F_SEQPACKET
    | VIRTIO_RING_F_INDIRECT_DESC
    | VIRTIO_RING_F_EVENT_IDX;

/// How many queues a virtio-vsock device has: 0 receives, 1 transmits and 2
/// carries transport events. All three are set up by the guest's driver, but
/// the kernel's vhost-vsock has no queue for the third one (see
/// [`crate::devices::virtio::vhost`]) — this device still exposes it, because a
/// driver that cannot find a queue it asks for fails to probe.
const VSOCK_QUEUE_COUNT: usize = 3;

/// The ring size offered to the driver (`QueueNumMax`). 256 descriptors is what
/// the reference implementations use for this device, and it is well within
/// what the kernel's vhost interface accepts (it wants a power of two, at most
/// 32768).
const VSOCK_QUEUE_MAX_SIZE: u32 = 256;

/// The virtio-vsock device of one VM.
///
/// Creating it opens the host backend and claims the guest's CID, so a VM whose
/// CID is reserved or already taken fails here — before its guest boots, with
/// an error the caller can act on, rather than at the first packet.
#[derive(Debug)]
pub struct VsockDevice {
    /// The device's queues, in driver order.
    queues: [QueueConfig; VSOCK_QUEUE_COUNT],
    /// The host backend, opened with the device and closed by [`Self::reset`]
    /// (or by dropping the device, which is what stops a VM's backend when the
    /// VM goes away).
    backend: Option<VhostVsock>,
    /// The guest CID this device's guest was assigned.
    guest_cid: u64,
    /// The features the backend offers and this port forwards.
    features: u64,
    /// The features the driver acknowledged.
    acked_features: u64,
}

impl VsockDevice {
    /// Creates the device for a guest with CID `guest_cid`.
    pub fn new(guest_cid: u64) -> Result<Self, VirtioError> {
        let backend = VhostVsock::new(guest_cid)?;
        let features = backend.features();
        Ok(Self {
            queues: [QueueConfig::new(VSOCK_QUEUE_MAX_SIZE); VSOCK_QUEUE_COUNT],
            backend: Some(backend),
            guest_cid,
            features,
            acked_features: 0,
        })
    }
}

impl VirtioDevice for VsockDevice {
    fn device_type(&self) -> u32 {
        VIRTIO_ID_VSOCK
    }

    fn features(&self) -> u64 {
        self.features
    }

    fn ack_features_by_page(&mut self, page: u32, value: u32) -> Result<(), VirtioError> {
        self.acked_features = ack_feature_page(self.features, self.acked_features, page, value)?;
        Ok(())
    }

    fn queues(&self) -> &[QueueConfig] {
        &self.queues
    }

    fn queues_mut(&mut self) -> &mut [QueueConfig] {
        &mut self.queues
    }

    fn read_config(&mut self, offset: u64, data: &mut [u8]) {
        read_guest_cid_config(self.guest_cid, offset, data);
    }

    fn activate(&mut self, mem: &GuestMemoryMmap, irq: &EventFd) -> Result<(), VirtioError> {
        // A driver that did not acknowledge `VIRTIO_F_VERSION_1` is a legacy
        // driver, which this device (and the vhost rings) cannot be driven by.
        if self.acked_features & VIRTIO_F_VERSION_1 == 0 {
            return Err(VirtioError::RequiredFeatureNotAcked {
                feature: "VIRTIO_F_VERSION_1",
            });
        }

        let mut backend = match self.backend.take() {
            Some(backend) => backend,
            // The driver reset the device (which closed the backend) and is
            // initializing it again: bring up a fresh backend for the same CID.
            None => {
                let backend = VhostVsock::new(self.guest_cid)?;
                self.features = backend.features();
                backend
            }
        };
        backend.start(mem, irq, self.acked_features, &self.queues)?;
        self.backend = Some(backend);
        Ok(())
    }

    fn notify_queue(&mut self, index: u32) -> Result<(), VirtioError> {
        // The index comes from the guest's `QueueNotify` write, so it is
        // validated against this device's own queues before it goes anywhere
        // near the backend — which knows about the wire it was given, not about
        // the queues the driver was offered.
        if !usize::try_from(index).is_ok_and(|index| index < self.queues.len()) {
            return Err(VirtioError::NoSuchQueue(index));
        }
        match &self.backend {
            Some(backend) => backend.kick(index).map_err(VirtioError::from),
            // A queue notification only means anything to an activated device:
            // before activation (and after a reset) there is no backend to hand
            // buffers to, and the driver's own state machine does not notify
            // then.
            None => Ok(()),
        }
    }

    fn reset(&mut self) {
        // Dropping the backend closes `/dev/vhost-vsock`, which is what stops
        // the kernel's device and releases its memory table, its rings and the
        // guest CID.
        self.backend = None;
        self.acked_features = 0;
        for queue in &mut self.queues {
            queue.reset();
        }
    }
}

/// Fills `data` with this device's configuration space at `offset`: the
/// `struct virtio_vsock_config` of virtio 1.0 §5.10.4, whose only field is the
/// guest CID.
///
/// The guest's `virtio_transport` reads its own CID from here at probe time
/// (`virtio_vsock_update_guest_cid`) and stamps it into every packet it sends;
/// the host's vhost-vsock drops a packet whose source CID is not the CID its
/// device was given, so a device that reported anything else (zero, for
/// instance) would leave its guest talking to nobody, one silent drop at a
/// time.
///
/// Bytes outside the field read as zero (the driver may read the space in
/// 8-, 4-, 2- or 1-byte accesses, and `offset` is whatever it asked for).
fn read_guest_cid_config(guest_cid: u64, offset: u64, data: &mut [u8]) {
    let cid = guest_cid.to_le_bytes();
    for (index, byte) in data.iter_mut().enumerate() {
        *byte = usize::try_from(offset)
            .ok()
            .and_then(|offset| offset.checked_add(index))
            .and_then(|index| cid.get(index))
            .copied()
            .unwrap_or(0);
    }
}

/// Records the features the driver acknowledged for one page.
///
/// `offered` is the device's full feature set, `acked` what the driver has
/// acknowledged so far, and `page`/`value` what it just wrote to the feature
/// registers. Bits that were not offered are an error: a driver and a device
/// that disagree about the features in use are undefined, so the device is not
/// allowed to accept them and carry on.
///
/// The page is whatever the driver selected, so it is not trusted here: a
/// 64-bit feature set has two pages, and a driver asking about any other one
/// reads no features at all, which makes acknowledging zero there a no-op and
/// acknowledging anything there an error (this is also what keeps the shifts
/// below in range for a page a guest chooses arbitrarily).
pub(crate) fn ack_feature_page(
    offered: u64,
    acked: u64,
    page: u32,
    value: u32,
) -> Result<u64, VirtioError> {
    if page > 1 {
        return if value == 0 {
            Ok(acked)
        } else {
            Err(VirtioError::UnsupportedFeatures {
                flags: u64::from(value),
            })
        };
    }
    let offered_page = if page == 0 {
        offered as u32
    } else {
        (offered >> 32) as u32
    };
    if value & !offered_page != 0 {
        return Err(VirtioError::UnsupportedFeatures {
            flags: u64::from(value) << (32 * page),
        });
    }
    let mask = 0xffff_ffff_u64 << (32 * page);
    Ok((acked & !mask) | (u64::from(value) << (32 * page)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The feature set this device offers must be the one the vhost interface
    /// can honour: a modern ring, no vIOMMU and no dirty logging.
    #[test]
    fn the_offered_features_are_the_modern_vsock_set() {
        assert_ne!(VSOCK_FEATURES & VIRTIO_F_VERSION_1, 0);
        assert_ne!(VSOCK_FEATURES & VIRTIO_VSOCK_F_SEQPACKET, 0);
        assert_ne!(VSOCK_FEATURES & VIRTIO_RING_F_INDIRECT_DESC, 0);
        assert_ne!(VSOCK_FEATURES & VIRTIO_RING_F_EVENT_IDX, 0);
        assert_eq!(VSOCK_FEATURES & (1 << 33), 0, "VIRTIO_F_ACCESS_PLATFORM");
        assert_eq!(VSOCK_FEATURES & (1 << 26), 0, "VHOST_F_LOG_ALL");
    }

    /// The guest takes its own CID from the configuration space and stamps it
    /// into the packets it sends; the host drops anything else as
    /// misaddressed, so this read has to report the CID the VM was given, in
    /// the layout the driver reads it with.
    #[test]
    fn the_configuration_space_reports_the_guest_cid() {
        let guest_cid = 0x0000_0001_0000_0003u64;
        // The 8-byte read the driver does at probe time.
        let mut data = [0u8; 8];
        read_guest_cid_config(guest_cid, 0, &mut data);
        assert_eq!(u64::from_le_bytes(data), guest_cid);
        // The same value through the 4-byte accesses an MMIO guest's `readl`
        // is limited to.
        let mut low = [0u8; 4];
        read_guest_cid_config(guest_cid, 0, &mut low);
        assert_eq!(u32::from_le_bytes(low), 3);
        let mut high = [0u8; 4];
        read_guest_cid_config(guest_cid, 4, &mut high);
        assert_eq!(u32::from_le_bytes(high), 1);
        // Beyond the field (and for an offset no configuration space has) the
        // read is zero, and the caller's buffer keeps its other bytes.
        let mut beyond = [0xffu8; 4];
        read_guest_cid_config(guest_cid, 8, &mut beyond);
        assert_eq!(beyond, [0, 0, 0, 0]);
        let mut far = [0xffu8; 2];
        read_guest_cid_config(guest_cid, u64::MAX, &mut far);
        assert_eq!(far, [0, 0]);
    }

    /// A virtio-vsock driver asks for three queues — receive, transmit and
    /// transport events — and fails to probe when one of them has no
    /// `QueueNumMax`, so the device must expose all three, even though the
    /// kernel's vhost-vsock backend has no queue behind the third.
    #[test]
    fn the_device_exposes_the_three_queues_the_driver_asks_for() {
        assert_eq!(VSOCK_QUEUE_COUNT, 3);
        assert!(VSOCK_QUEUE_MAX_SIZE.is_power_of_two());
    }

    /// Acknowledging features keeps both pages independent: the second page
    /// write must not clear what the first one acknowledged.
    #[test]
    fn feature_pages_are_acknowledged_independently() {
        let offered = VIRTIO_F_VERSION_1 | VIRTIO_VSOCK_F_SEQPACKET;
        let acked =
            ack_feature_page(offered, 0, 0, VIRTIO_VSOCK_F_SEQPACKET as u32).expect("page 0");
        assert_eq!(acked, VIRTIO_VSOCK_F_SEQPACKET);
        let acked = ack_feature_page(offered, acked, 1, 1).expect("page 1");
        assert_eq!(acked, VIRTIO_F_VERSION_1 | VIRTIO_VSOCK_F_SEQPACKET);
        // Re-writing a page replaces only that page's bits.
        let acked = ack_feature_page(offered, acked, 0, 0).expect("page 0 cleared");
        assert_eq!(acked, VIRTIO_F_VERSION_1);
    }

    /// A driver asking for a feature the device did not offer must be refused,
    /// page by page.
    #[test]
    fn unoffered_features_are_refused() {
        let offered = VIRTIO_F_VERSION_1;
        assert!(matches!(
            ack_feature_page(offered, 0, 0, 1),
            Err(VirtioError::UnsupportedFeatures { flags: 1 })
        ));
        // The offered page-1 bit is accepted and lands in the high half...
        assert_eq!(
            ack_feature_page(offered, 0, 1, 1).expect("page 1"),
            VIRTIO_F_VERSION_1
        );
        // ...while a bit the device did not offer on that page is refused,
        // named by its position in the whole feature set.
        assert!(matches!(
            ack_feature_page(offered, 0, 1, 2),
            Err(VirtioError::UnsupportedFeatures { flags }) if flags == 1 << 33
        ));
    }

    /// A feature page the driver selects arbitrarily must not panic the VMM:
    /// the register it comes from is guest-controlled.
    #[test]
    fn an_out_of_range_feature_page_is_handled_total() {
        let offered = VIRTIO_F_VERSION_1;
        assert_eq!(ack_feature_page(offered, 0, 2, 0).expect("zero"), 0);
        assert_eq!(
            ack_feature_page(offered, 7, u32::MAX - 1, 0).expect("zero"),
            7
        );
        assert!(ack_feature_page(offered, 0, 2, 1).is_err());
        assert!(ack_feature_page(offered, 0, u32::MAX, 1).is_err());
    }
}
