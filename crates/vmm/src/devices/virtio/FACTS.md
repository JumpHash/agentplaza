# Platform facts: virtio-mmio + vhost-vsock

What this port's vsock device was built on, and what was measured rather than
assumed. Everything under "measured" was run on the development host; the
probes are in-tree (see below), so the numbers can be re-checked with
`cargo test -p vmm --lib` (the probes span `devices::virtio`, `arch::aarch64::fdt`
and `builder`).

Host: Linux 7.2.8-arch1-2, x86_64 (i9-12900K), 4 KiB pages, KVM. The VMM runs
as an unprivileged user (uid 1000): `/dev/vhost-vsock` is `crw-rw-rw- root kvm`,
and the ioctl sequence below needs no capability beyond that. The guest is the
same kernel, booted from `/boot/vmlinuz-linux` with an initramfs that loads
`vsock.ko`, `vmw_vsock_virtio_transport_common.ko`,
`vmw_vsock_virtio_transport.ko` and `virtio_mmio.ko`. That is the measurement
setup (`scripts/mk-initrd.sh` with its default `APZ_GUEST_MODULES=host`);
`scripts/mk-guest.sh` now builds a guest kernel with the drivers built in
(`APZ_GUEST_MODULES=builtin`).

## Probes in the tree

| Probe | Where | What it establishes |
|---|---|---|
| ioctl encodings | `vhost.rs` tests `ioctl_request_codes_match_the_kernels`, `ioctl_argument_layouts_match_the_kernels` | the request codes and argument layouts are the kernel's (`linux/vhost.h`), byte for byte |
| full device setup | `vhost.rs` test `the_host_can_start_a_vhost_vsock_device` | SET_OWNER → SET_FEATURES → SET_MEM_TABLE → per-queue NUM/ADDR/BASE/KICK/CALL → SET_GUEST_CID → SET_RUNNING all take a real device |
| queue count | same test | the kernel answers `ENOBUFS` for a queue index past the end of its array |
| CID ownership | `vhost.rs` test `a_guest_cid_cannot_be_claimed_twice` | a CID another backend holds cannot be claimed; it is released when the fd closes |
| FDT node (aarch64) | `arch/aarch64/fdt.rs` test `create_fdt_describes_the_vsock_transport` | the node's window and interrupt properties are the layout's |
| guest command line (x86_64) | `builder.rs` test `guest_cmdline_registers_the_x86_64_virtio_transports` | the `virtio_mmio.device=` entry the guest needs is what the layout says |

The probes that need the device skip themselves when `/dev/vhost-vsock` is not
openable, so they are safe on a host without the module.

## Measured

* **`VHOST_GET_FEATURES` = `0x33d000002`** — `VIRTIO_VSOCK_F_SEQPACKET` (1),
  `VIRTIO_F_NOTIFY_ON_EMPTY` (24), `VHOST_F_LOG_ALL` (26),
  `VIRTIO_F_ANY_LAYOUT` (27), `VIRTIO_RING_F_INDIRECT_DESC` (28),
  `VIRTIO_RING_F_EVENT_IDX` (29), `VIRTIO_F_VERSION_1` (32),
  `VIRTIO_F_ACCESS_PLATFORM` (33).
* **Two queues.** This kernel's `vhost_vsock` has `struct vhost_virtqueue
  vqs[2]` — receive (0) and transmit (1); the virtio-vsock event queue is
  device-side only now. `VHOST_SET_VRING_NUM` with index 2 answers `ENOBUFS`,
  which is how a caller learns the count (`drivers/vhost/vsock.c`,
  `vhost_vsock_dev_open`). The guest's `virtio_transport` still sets up three
  (`VSOCK_VQ_MAX` is 3 in `net/vmw_vsock/virtio_transport.c` and it calls
  `virtio_find_vqs(vdev, VSOCK_VQ_MAX, …)`), so the MMIO transport exposes
  three queues and the third is deliberately unwired: its notifications are
  accepted and ignored.
* **`VHOST_SET_VRING_ADDR` takes host userspace addresses**, not guest
  physical ones: the kernel stores them in `vq->desc`/`avail`/`used` as
  `void __user *` and reads the rings with `copy_from_user` on the *owner
  process's* pointers (`drivers/vhost/vhost.c`), while descriptor buffers are
  translated through the memory table. Passing a guest address still passes the
  `access_ok()` check at `SET_RUNNING` (any low user address does), but the
  rings then read as faults and the device silently processes nothing — which
  is why `VhostVsock::start` translates with
  `GuestMemoryMmap::get_host_address`.
* **A memory table is required before `SET_RUNNING`**: without one the kernel
  answers `EFAULT` (`vhost_vsock_start` → `vhost_vq_access_ok` →
  `vq_memory_access_ok`, which is false for a vhost with no `umem`).
* **`VHOST_VSOCK_SET_RUNNING` takes a pointer**: `_IOW(VHOST_VIRTIO, 0x61,
  int)` — the kernel `copy_from_user`s an `int`, so the argument is
  `&1i32`, not `1`. Passing the value gives `EFAULT` (the kernel reads from
  address 0x1).
* **CIDs.** `VHOST_VSOCK_SET_GUEST_CID` refuses `<= 2` (`EINVAL`: 0 is the
  hypervisor, 1 loopback, 2 the host) and `> u32::MAX`, and refuses a CID
  another transport or another VMM's backend already claims (`EADDRINUSE`). The
  CID can be re-set on a device that is not running.
* **The guest takes its own CID from the device configuration space.**
  `virtio_vsock_update_guest_cid` (`net/vmw_vsock/virtio_transport.c`) reads
  `offsetof(struct virtio_vsock_config, guest_cid)` — a little-endian `u64` at
  configuration offset 0 — and stamps it into every packet it sends; the host's
  `vhost_vsock_handle_tx_kick` *silently drops* a packet whose `src_cid` is not
  the CID its device was given. A transport that answers the config space with
  zeroes therefore produces a guest that connects, times out and never appears
  at the host listener: the first end-to-end run of this device failed exactly
  that way, with the transmit ring consumed (`used.idx` advanced, so vhost had
  read the packet) and the receive ring untouched. This is why
  `VirtioDevice::read_config` exists and why the vsock device implements it.
* **Guest enumeration.** There is no device tree on `x86_64`, so the window is
  registered from the kernel command line: `virtio_mmio.device=<size>@<base>
  :<irq>[:<id>]` (`drivers/virtio/virtio_mmio.c`, `vm_cmdline_set`: the size is
  `memparse`d, the rest `sscanf`ed as `@%lli:%u`). The parameter is singular
  (`device`, not `devices`) and it is a *module parameter* on this kernel —
  `virtio_mmio` is a module, so the entry takes effect when the initramfs loads
  the module with it (`finit_module`'s parameter string), not from the kernel's
  own command line. The port writes it into the guest command line anyway,
  because that is where a built-in `virtio_mmio` would read it from, and
  because the guest's init harvests it from `/proc/cmdline` to pass to the
  module. On `aarch64` the device tree node (`compatible = "virtio,mmio"`,
  `reg`, `interrupts`, `dma-coherent`) says the same thing.
* **The transport's register file is virtio 1.0 §4.2.2** with the interrupt
  status in its legacy-free sense: bit 0 (`VIRTIO_MMIO_INT_VRING`) means "a
  used ring has entries". This kernel's guest driver
  (`vm_interrupt`) reads that register, writes it back to `InterruptACK`, and
  services *every* queue when bit 0 is set — and when it reads 0 it drops the
  interrupt as spurious and never walks the used ring. A vhost-backed device
  cannot report which queue its backend signalled, so the transport always
  reports bit 0 (Firecracker does the same for vhost-user).

## End-to-end evidence

`vmctl` boots the guest with a vsock device, the guest's harness opens the loop
and the host's orchestrator stand-in (`vsockd`) answers it. The transcripts were
captured from an earlier build: some of their lines have since changed wording,
and they are kept as the record of that run.

```console
$ target/debug/vsockd --port 5001 &      # in a supervised shell
$ target/debug/vmctl --kernel /boot/vmlinuz-linux \
      --initrd target/guest/initrd.x86_64.gz -n 1 --mem-mib 512 \
      --cmdline 'console=ttyS0 plaza.harness=1 plaza.port=5001' \
      --vsock-cid 3 --timeout-secs 60
[vm 0] plaza guest: virtio_mmio took 1 command-line parameter(s)
[vm 0] agent-harness: guest CID 3 vm 0 -> host CID 2 port 5001
[vm 0] agent-harness: connected after 1 attempt(s)
[vm 0] agent-harness: handshake complete (host echo "plaza-orchestrator")
[vm 0] agent-harness: echo call 1 verified: "plaza-guest-3-hello"
[vm 0] agent-harness: uptime call 2: 244073.16 5326141.84
[vm 0] agent-harness: session complete after 2 tool calls
[vm 0] plaza guest: agent harness session complete, rebooting
vmctl: vm 0: guest exited          # exit status 0
```

```console
vsockd: listening on AF_VSOCK CID 2 port 5001
vsockd: accepted a connection from cid 3
vsockd: guest 3 vm 0 says hello (accepted from cid 3)
[<-] guest 3 vm0 {"type":"welcome","echo":"plaza-orchestrator"}
[guest 3 vm0 ->] {"type":"tool_call","call":1,"name":"echo","args":{"text":"plaza-guest-3-hello"}}
[<-] guest 3 vm0 {"type":"tool_result","call":1,"ok":true,"output":"plaza-guest-3-hello"}
[guest 3 vm0 ->] {"type":"tool_call","call":2,"name":"uptime","args":{}}
[<-] guest 3 vm0 {"type":"tool_result","call":2,"ok":true,"output":"244073.16 5326141.84"}
[guest 3 vm0 ->] {"type":"bye","calls":2}
vsockd: session with guest 3 vm 0 ended cleanly after 2 tool calls
```

`-n 2 --vsock-cid 3` runs two VMs at once with distinct CIDs (3 and 4, the
listener's `accepted a connection from cid N` lines) and both sessions end
cleanly. The harness no longer reads a `plaza.cid` command-line flag (the run's
command line above carried one, and it is dropped from the command here): the
CID the transport stamps into packets comes from the device's configuration
space, which is what `vmctl`'s per-VM `VsockConfig` sets.

## Not verified here

* **`aarch64` at runtime.** The layout, the FDT node and the guest command
  line compile and are unit-tested for `aarch64-unknown-linux-gnu` (the
  workspace's cross target), but no arm64 host is available to boot one: the
  GIC SPI numbering (`VSOCK_GSI = 1` → SPI 1) and the device tree node are
  reasoned from the port's own console node and upstream Firecracker, not
  measured.
* **Apple Silicon.** Hypervisor.framework has no vhost-vsock either, and needs
  none: the userspace device here runs on it too, over the POSIX event layer
  (`pal::posix`: a pipe-backed `EventFd` and a `poll(2)` `Epoll`), with its
  interrupt relayed to an SPI of the backend's userspace GICv3 (see
  `hvf::boot`). Same caveat as aarch64 above, more so: compile-checked for
  `aarch64-apple-darwin`, never run on a Mac.
* **Zero-copy and multi-queue.** Neither is attempted: one virtqueue pair, the
  kernel's own copy in and out.
