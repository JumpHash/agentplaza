// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port (PLAN 22.6, W58). The transport drives a `Box<dyn VirtioDevice>`, but the
// device also has a second driver: the switch delivers frames on its own schedule, so a thread owns the
// receive side. The device lives behind an `Arc<Mutex<..>>` shared with that thread, which holds only a
// `Weak`: when the transport drops the device, the port drops with it, the switch forgets the port, the
// frame channel disconnects and the thread ends. A thread that held a strong reference would keep the
// port alive forever.

//! The virtio-mmio face of the net device, with its receive thread.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::thread;
use std::time::Duration;

use crate::devices::virtio::VirtioError;
use crate::devices::virtio::ack_feature_page;
use crate::devices::virtio::mmio::{QueueConfig, VirtioDevice};
use crate::pal::eventfd::EventFd;
use crate::vstate::memory::GuestMemoryMmap;

use super::device::VirtioNet;
use super::switch::Port;
use super::{
    Mac, NET_QUEUE_NUM, NET_QUEUE_SIZE, NetError, RX_QUEUE, TX_QUEUE, VIRTIO_F_VERSION_1,
    VIRTIO_ID_NET, VIRTIO_NET_F_MAC,
};

/// How often the receive thread checks whether its device is still alive when no frame arrives.
const POLL: Duration = Duration::from_millis(50);

/// The features the device offers: the MAC in its configuration space, and the modern transport.
pub const NET_FEATURES: u64 = VIRTIO_NET_F_MAC | VIRTIO_F_VERSION_1;

/// The virtio-mmio device that hands the net device to the transport.
#[derive(Debug)]
pub struct NetMmioDevice {
    inner: Arc<Mutex<VirtioNet>>,
    /// The frames the switch delivers to the device. Taken by the receive thread on activation.
    frames: Option<Receiver<Vec<u8>>>,
    queues: [QueueConfig; NET_QUEUE_NUM],
    acked: u64,
}

/// The receive thread: each frame the switch delivers goes to the device's receive path, until the
/// device is gone or the switch has dropped the port.
fn pump(frames: Receiver<Vec<u8>>, device: Weak<Mutex<VirtioNet>>) {
    loop {
        match frames.recv_timeout(POLL) {
            Ok(frame) => {
                let Some(device) = device.upgrade() else {
                    return;
                };
                let mut device = device.lock().unwrap_or_else(PoisonError::into_inner);
                // A frame the device cannot take (its ring is unusable) is dropped; the next one may fit.
                device.receive(frame).ok();
            }
            Err(RecvTimeoutError::Timeout) => {
                if device.strong_count() == 0 {
                    return;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

impl NetMmioDevice {
    /// A device with `mac`, sending through `port` and receiving the frames `frames` carries.
    pub fn new(mac: Mac, port: Port, frames: Receiver<Vec<u8>>) -> Self {
        let queues = [(); NET_QUEUE_NUM].map(|()| QueueConfig::new(u32::from(NET_QUEUE_SIZE)));
        Self {
            inner: Arc::new(Mutex::new(VirtioNet::new(mac, port))),
            frames: Some(frames),
            queues,
            acked: 0,
        }
    }

    fn device(&self) -> std::sync::MutexGuard<'_, VirtioNet> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl VirtioDevice for NetMmioDevice {
    fn device_type(&self) -> u32 {
        VIRTIO_ID_NET
    }

    fn features(&self) -> u64 {
        NET_FEATURES
    }

    fn ack_features_by_page(&mut self, page: u32, value: u32) -> Result<(), VirtioError> {
        self.acked = ack_feature_page(NET_FEATURES, self.acked, page, value)?;
        Ok(())
    }

    fn queues(&self) -> &[QueueConfig] {
        &self.queues
    }

    fn queues_mut(&mut self) -> &mut [QueueConfig] {
        &mut self.queues
    }

    fn read_config(&mut self, offset: u64, data: &mut [u8]) {
        // `struct virtio_net_config`: the MAC is its first six bytes, and nothing follows it for the
        // features this device offers.
        let mac = self.device().mac();
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        for (index, byte) in data.iter_mut().enumerate() {
            *byte = mac.get(start.saturating_add(index)).copied().unwrap_or(0);
        }
    }

    fn notify_queue(&mut self, index: u32) -> Result<(), VirtioError> {
        let mut device = self.device();
        match index {
            RX_QUEUE => device.flush_rx()?,
            TX_QUEUE => device.process_tx()?,
            // The device has two queues; a notification for another index is the driver's, and is dropped.
            _ => {}
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.device().reset();
        for queue in &mut self.queues {
            queue.reset();
        }
        self.acked = 0;
    }

    fn acked_features(&self) -> u64 {
        self.acked
    }

    fn activate(&mut self, mem: &GuestMemoryMmap, irq: &EventFd) -> Result<(), VirtioError> {
        let irq = irq
            .try_clone()
            .map_err(|err| NetError::Activate(format!("the interrupt: {err}")))?;
        self.device().activate(mem.clone(), irq, &self.queues)?;
        // The receive thread starts once, at the first activation: a reset and re-activation keeps it.
        if let Some(frames) = self.frames.take() {
            let device = Arc::downgrade(&self.inner);
            thread::Builder::new()
                .name("net-rx".into())
                .spawn(move || pump(frames, device))
                .map_err(|err| NetError::Activate(format!("the receive thread: {err}")))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::virtio::net::NET_HDR_LEN;
    use crate::devices::virtio::net::switch::{Endpoint, Switch};
    use crate::devices::virtio::queue::VIRTQ_DESC_F_WRITE;
    use crate::devices::virtio::test_utils::{VirtQueue, default_mem};
    use crate::pal::eventfd::EFD_NONBLOCK;
    use crate::vstate::memory::{Bytes, GuestAddress};

    const DEV: Mac = [0x02, 0x41, 0x50, 0x5a, 0x00, 0x0a];
    const PEER: Mac = [0x02, 0x41, 0x50, 0x5a, 0x00, 0x0b];

    fn frame(dst: Mac, src: Mac, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&dst);
        out.extend_from_slice(&src);
        out.extend_from_slice(&[0x88, 0xb5]);
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn the_device_offers_the_mac_and_the_modern_transport_and_nothing_else() {
        let switch = Switch::new();
        switch.create_segment("s");
        let (port, frames) = switch.attach("s", Endpoint::Guest { mac: DEV }).unwrap();
        let mut dev = NetMmioDevice::new(DEV, port, frames);
        assert_eq!(dev.device_type(), VIRTIO_ID_NET);
        assert_eq!(dev.features(), VIRTIO_NET_F_MAC | VIRTIO_F_VERSION_1);
        // Acknowledging a feature the device does not offer is refused.
        assert!(dev.ack_features_by_page(0, 1 << 7).is_err());
        dev.ack_features_by_page(0, VIRTIO_NET_F_MAC as u32)
            .unwrap();
        assert_eq!(dev.acked_features(), VIRTIO_NET_F_MAC);
        // The configuration space reads the MAC, and zeroes past it.
        let mut config = [0xffu8; 8];
        dev.read_config(0, &mut config);
        assert_eq!(config, [0x02, 0x41, 0x50, 0x5a, 0x00, 0x0a, 0, 0]);
    }

    #[test]
    fn the_transport_drives_both_queues_and_frames_flow_through_the_receive_thread() {
        let mem = default_mem();
        let switch = Switch::new();
        switch.create_segment("s");
        let (port, frames) = switch.attach("s", Endpoint::Guest { mac: DEV }).unwrap();
        let (peer, _peer_rx) = switch.attach("s", Endpoint::Guest { mac: PEER }).unwrap();
        let mut dev = NetMmioDevice::new(DEV, port, frames);

        // The driver programs both rings, as its register writes would.
        let rx = VirtQueue::new(GuestAddress(0), &mem, 16);
        let tx = VirtQueue::new(GuestAddress(0x2000), &mem, 16);
        for (index, vq) in [&rx, &tx].into_iter().enumerate() {
            let config = &mut dev.queues_mut()[index];
            config.size = 16;
            config.ready = true;
            config.desc = vq.desc_table_addr().0;
            config.avail = vq.avail_ring_addr().0;
            config.used = vq.used_ring_addr().0;
        }
        let irq = EventFd::new(EFD_NONBLOCK).unwrap();
        dev.activate(&mem, &irq).unwrap();

        // Frames the guest receives: the peer sends, the thread delivers, the guest sees it complete.
        rx.set_desc(0, 0x6000, 2048, VIRTQ_DESC_F_WRITE, 0);
        rx.publish(0);
        let f = frame(DEV, PEER, b"over the thread");
        peer.send(&f).unwrap();
        let mut waited = 0;
        while rx.used_idx() == 0 && waited < 200 {
            thread::sleep(Duration::from_millis(10));
            waited += 1;
        }
        assert_eq!(rx.used_idx(), 1, "the receive thread completed the buffer");
        assert_eq!(rx.used_elem(0), (0, (NET_HDR_LEN + f.len()) as u32));
        let mut body = vec![0u8; f.len()];
        mem.read_slice(&mut body, GuestAddress(0x6000 + NET_HDR_LEN as u64))
            .unwrap();
        assert_eq!(body, f);

        // Frames the guest sends: the transmit notification sends them to the peer.
        let g = frame(PEER, DEV, b"from the guest");
        mem.write_slice(&[0u8; NET_HDR_LEN], GuestAddress(0x4000))
            .unwrap();
        mem.write_slice(&g, GuestAddress(0x4100)).unwrap();
        tx.set_desc(
            0,
            0x4000,
            NET_HDR_LEN as u32,
            crate::devices::virtio::queue::VIRTQ_DESC_F_NEXT,
            1,
        );
        tx.set_desc(1, 0x4100, g.len() as u32, 0, 0);
        tx.publish(0);
        dev.notify_queue(TX_QUEUE).unwrap();
        assert_eq!(tx.used_idx(), 1);
    }

    #[test]
    fn dropping_the_device_detaches_its_port_and_ends_the_thread() {
        let mem = default_mem();
        let switch = Switch::new();
        switch.create_segment("s");
        let (port, frames) = switch.attach("s", Endpoint::Guest { mac: DEV }).unwrap();
        let (_peer, _peer_rx) = switch.attach("s", Endpoint::Guest { mac: PEER }).unwrap();
        let mut dev = NetMmioDevice::new(DEV, port, frames);
        let rx = VirtQueue::new(GuestAddress(0), &mem, 16);
        let tx = VirtQueue::new(GuestAddress(0x2000), &mem, 16);
        for (index, vq) in [&rx, &tx].into_iter().enumerate() {
            let config = &mut dev.queues_mut()[index];
            config.size = 16;
            config.ready = true;
            config.desc = vq.desc_table_addr().0;
            config.avail = vq.avail_ring_addr().0;
            config.used = vq.used_ring_addr().0;
        }
        dev.activate(&mem, &EventFd::new(EFD_NONBLOCK).unwrap())
            .unwrap();
        drop(dev);
        // The port left the switch with the device, so its MAC is free to attach again.
        assert!(switch.attach("s", Endpoint::Guest { mac: DEV }).is_ok());
    }
}
