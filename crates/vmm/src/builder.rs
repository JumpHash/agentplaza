// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/builder.rs and
// src/device_manager/legacy.rs (trimmed: no `Vmm` newtype, event manager,
// seccomp, snapshot/restore, ACPI or device manager — this port boots a
// single microVM from a [`VmSpec`] through `boot`, which wires guest memory,
// the kernel image, the command line and the legacy devices and leaves the
// vCPUs running; the exit watcher that upstream runs on the main event loop
// is a plain thread here).
//
// One deliberate behavioural difference from upstream: there is no ACPI table
// generation and no PCI, so the guest discovers its devices through the
// MPTABLE written by `configure_system_for_boot` and the legacy PIC/PIT the
// in-kernel irqchip provides. The guest kernel must be built with
// `CONFIG_X86_MPPARSE`/legacy PIC support (all mainstream x86_64 kernels are).

//! Single-VM build and boot orchestration.

use std::io;
use std::os::fd::AsRawFd;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use vmm_sys_util::eventfd::EventFd;

use crate::devices::legacy::SerialDevice;
use crate::devices::virtio::vsock::unix::VsockUnixBackend;
use crate::devices::virtio::vsock::Vsock;
use crate::spawner::{SpawnError, VmRuntime, VmSpec};
use crate::vstate::vm::KvmVm;

/// Boots the VM described by `spec` and returns once its vCPUs are running.
///
/// The backend fills in `runtime` (see [`crate::spawner`]); the spawned vCPU
/// threads are joined by the VM's exit watcher, which is the thread this
/// function registers with [`VmRuntime::add_thread`].
pub fn boot(spec: &VmSpec, runtime: Arc<VmRuntime>) -> Result<(), SpawnError> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        x86_64::boot(spec, runtime)
    }

    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        aarch64::boot(spec, runtime)
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = (spec, runtime);
        Err(SpawnError::NoBackend)
    }
}

/// The confinement step of a VM's build: enters the jailer's per-VM root
/// ([`crate::jail::jailer`]) when it is armed and this process can.
///
/// It runs **once per VM, on the calling thread, before any VMM thread exists**
/// — the precondition the jailer states — and before the devices that open
/// paths are built. Every path opened after it (the kernel image, the initrd,
/// the block image, the muxer's socket) resolves inside the new root, which
/// mirrors each source at its own path; see the jailer's module documentation
/// for that audit. The descriptors the setup already holds (`/dev/kvm`, the
/// image files) are unaffected.
///
/// The one tolerated outcome is "this process cannot be jailed by
/// construction" (a multi-threaded caller such as `noded`), reported once by
/// [`crate::jail::jailer::enter_thread`]; there the VM keeps its seccomp jail.
/// A *real* failure — the jail was supposed to land and a step did not — is
/// returned, and the boot must not continue unjailed.
fn jail_the_vm(spec: &VmSpec, id: u32) -> Result<(), crate::jail::jailer::JailerError> {
    use crate::jail::jailer::{self, Access, Jailer};

    // The VM's socket directory is the base when there is one, so a run's jails
    // live beside its sockets (a sibling, never a child: the muxer's UDS
    // directory is itself mirrored into the jail).
    let uds_dir = spec
        .vsock
        .as_ref()
        .and_then(|vsock| vsock.uds_path.parent());
    let base = jailer::default_base(uds_dir);

    let mut plan = Jailer::new(jailer::root_for(&base, id));
    plan.bind(&spec.kernel, Access::ReadOnly);
    if let Some(initrd) = spec.initrd.as_ref() {
        plan.bind(initrd, Access::ReadOnly);
    }
    if let Some(workspace) = spec.workspace.as_ref() {
        // The one writable source in the whole jail (invariant 3).
        plan.bind(workspace, Access::ReadWrite);
    }
    for dev in ["/dev/kvm", "/dev/null", "/dev/urandom"] {
        plan.bind(dev, Access::ReadOnly);
    }
    if let Some(uds_dir) = uds_dir {
        plan.bind_dir(uds_dir, Access::ReadWrite);
    }
    plan.tmpfs("/run");

    jailer::enter_thread(plan, id).map(|_| ())
}

/// Kernel command line handed to the guest: the caller's, plus the defaults
/// this port needs.
///
/// These options are appended when the caller has not set them:
///
/// * `pci=off` — this port has no PCI (no config space, no virtio-pci), so a
///   guest that probes for it only wastes boot time and floods the user-space
///   exit path with unhandled port accesses. Upstream Firecracker appends the
///   same option whenever PCI is disabled.
/// * `loglevel=7` — the guest's only console is the serial capture, and
///   distribution kernels commonly ship
///   `CONFIG_CONSOLE_LOGLEVEL_DEFAULT=4`, which hides everything below a
///   warning, boot banner included (verified on the host kernel,
///   7.2.8-arch1-2: the banner is missing under QEMU with the same command
///   line). Callers that set the level themselves (`loglevel=`, `quiet`,
///   `debug`, `ignore_loglevel`) are left alone.
/// * on `x86_64` only, `reboot=acpi` — this port's ACPI tables declare the
///   FADT RESET_REGISTER (a write of `RESET_VALUE` to PIO `0x606`, handled by
///   [`crate::devices::legacy::pm1_cnt::Pm1CntDevice`]) and set the
///   HW-reduced flag so S5 poweroff works. The HW-reduced flag makes the
///   kernel's `efi_reboot_required()` return true, which silently rewrites the
///   default `reboot_type` to `BOOT_EFI`; with no EFI runtime services the
///   reboot then falls straight through to `machine_real_restart` (the BIOS
///   real-mode jump), which double-faults on CET/IBT kernels. Forcing
///   `reboot=acpi` disables that DMI/EFI quirk (`reboot_default = 0`) and keeps
///   the reboot on the ACPI reset register, so a guest reboot stops the VM
///   cleanly with `reset=true`. Callers that set a `reboot=` mode themselves
///   are left alone.
/// * on `x86_64` only, one `virtio_mmio.device` entry per virtio-mmio device
///   the VM has (the vsock device and/or the block device's writable volume):
///   an `x86_64` guest has no device tree, so the kernel learns about each
///   transport from its command line. An entry is
///   `<size>@<base>:<irq>`, the format of `drivers/virtio/virtio_mmio.c`,
///   whose parameters the kernel consumes as it loads the `virtio_mmio`
///   platform driver (built in, or — as on the host kernel 7.2.8-arch1-2 —
///   loaded from the initramfs with the same entry as a module parameter; the
///   parameter is repeatable, the kernel appends every occurrence to its
///   command-line device array). A caller that registered its own transport by
///   setting the parameter is left alone entirely. On `aarch64` the device
///   tree nodes [`crate::arch::aarch64::fdt`] writes say
///   the same thing, so nothing is added here.
///
/// This is deliberate and documented, not a hidden rewrite: the appended
/// strings are constants derived from the port's own layout, never host state
/// (no environment variable, token or path can reach the guest this way).
fn guest_cmdline(requested: &str, has_vsock: bool, has_workspace: bool, has_balloon: bool) -> String {
    const LOGLEVEL_ARGS: [&str; 4] = ["loglevel=", "ignore_loglevel", "quiet", "debug"];
    const PCI_ARGS: [&str; 1] = ["pci="];
    #[cfg(target_arch = "x86_64")]
    const REBOOT_ARGS: [&str; 1] = ["reboot="];
    #[cfg(target_arch = "x86_64")]
    const VIRTIO_MMIO_ARGS: [&str; 1] = ["virtio_mmio.device"];

    let requested = requested.trim();
    let args: Vec<&str> = requested.split_whitespace().collect();
    let sets = |prefixes: &[&str]| {
        args.iter()
            .any(|arg| prefixes.iter().any(|prefix| arg.starts_with(prefix)))
    };

    let mut defaults: Vec<String> = Vec::new();
    if !sets(&PCI_ARGS) {
        defaults.push("pci=off".to_string());
    }
    if !sets(&LOGLEVEL_ARGS) {
        defaults.push("loglevel=7".to_string());
    }

    // `x86_64` only: keep guest reboots on the ACPI reset register instead of
    // the kernel's HW-reduced-triggered EFI fallthrough (which ends in the
    // BIOS real-mode jump and a CET double-fault). See the doc comment above.
    #[cfg(target_arch = "x86_64")]
    if !sets(&REBOOT_ARGS) {
        defaults.push("reboot=acpi".to_string());
    }

    // The `x86_64` transport windows, one entry per virtio-mmio device. The
    // numbers are this port's own layout constants, and no two devices share an
    // address or an interrupt.
    #[cfg(target_arch = "x86_64")]
    if !sets(&VIRTIO_MMIO_ARGS) {
        use crate::arch::x86_64::layout::{
            BALLOON_GSI, BALLOON_MMIO_START, MMIO_LEN, VSOCK_GSI, VSOCK_MMIO_START, WORKSPACE_GSI,
            WORKSPACE_MMIO_START,
        };
        if has_vsock {
            defaults.push(format!(
                "virtio_mmio.device={MMIO_LEN:#x}@{VSOCK_MMIO_START:#x}:{VSOCK_GSI}"
            ));
        }
        if has_workspace {
            defaults.push(format!(
                "virtio_mmio.device={MMIO_LEN:#x}@{WORKSPACE_MMIO_START:#x}:{WORKSPACE_GSI}"
            ));
        }
        if has_balloon {
            defaults.push(format!(
                "virtio_mmio.device={MMIO_LEN:#x}@{BALLOON_MMIO_START:#x}:{BALLOON_GSI}"
            ));
        }
    }
    // On `aarch64` the device tree describes the transports.
    #[cfg(not(target_arch = "x86_64"))]
    let _ = (has_vsock, has_workspace, has_balloon);

    if defaults.is_empty() {
        return requested.to_string();
    }
    if requested.is_empty() {
        defaults.join(" ")
    } else {
        format!("{requested} {}", defaults.join(" "))
    }
}

/// The memory control of a VM whose balloon lives in this process.
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
struct InProcessBalloon {
    shared: crate::devices::virtio::balloon::SharedBalloon,
    max_mib: u32,
}

impl crate::spawner::MemoryControl for InProcessBalloon {
    fn set_usable_mib(&self, mib: u32) -> Result<(), SpawnError> {
        self.shared.set_usable_mib(self.max_mib, mib);
        Ok(())
    }

    fn stats(&self) -> crate::spawner::MemoryStats {
        let stats = self.shared.stats();
        let target_mib = self.shared.target_mib();
        crate::spawner::MemoryStats {
            max_mib: self.max_mib,
            usable_mib: self.max_mib.saturating_sub(target_mib),
            held_mib: stats.actual_mib,
            free_mib: stats.free_mib,
            available_mib: stats.available_mib,
        }
    }
}

/// Installs the VM's stop control and spawns its exit watcher.
///
/// Both architectures end here: whichever vCPU stops the VM (guest shutdown or
/// reset, a vCPU fault, or an arch-specific reset source) writes the VM's exit
/// event, and the watcher — the thread the spawner joins — stops and joins the
/// vCPU threads, then drops `devices`. It also reports whether the guest's exit
/// was a reset rather than a shutdown, which the vCPU threads publish
/// separately because the exit event itself is a shared counter.
///
/// The bus only holds weak references, so the caller passes the strong handles
/// of the devices it registered: the watcher is the VM's last thread, which
/// makes those devices live exactly as long as the VM does.
fn start_exit_watcher<D: Send + 'static>(
    vm: Arc<KvmVm>,
    runtime: &Arc<VmRuntime>,
    devices: D,
) -> io::Result<()> {
    let stop_evt = vm.vcpus_exit_evt().try_clone()?;
    // Publish the exit state before the notify, exactly like the guest exit
    // paths do, so the vsock event thread (a shared *observer* of the same
    // event fd, never a reader) sees the exit even if the watcher consumes the
    // count first.
    let stop_signalled = Arc::clone(vm.exit_signalled());
    runtime.set_stop_control(move || {
        stop_signalled.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Err(err) = stop_evt.write(1) {
            eprintln!("warning: failed to signal VM exit: {err}");
        }
    });

    let exit_evt = vm.vcpus_exit_evt().try_clone()?;
    let watcher_runtime = Arc::clone(runtime);
    let watcher = thread::Builder::new()
        .name(format!("apz_vm{}_exit", runtime.id))
        .spawn(move || {
            // The jail goes on before this thread waits: the boot's setup is
            // all on the caller's thread by now, and this thread only polls,
            // joins the vCPUs and drops the devices. A real install error is
            // fail-closed; a platform with no seccomp at all is reported and
            // tolerated (see `crate::jail`).
            if let Err(err) = crate::jail::Jail::arm_thread() {
                eprintln!("vm: cannot arm the seccomp jail: {err}");
                return;
            }
            match wait_for_event(&exit_evt) {
                Ok(()) => {
                    if !watcher_runtime.stop_requested() {
                        // The guest ended the VM itself. A vCPU that left for
                        // a reset published that before signalling the event,
                        // so a reboot is not reported as a plain shutdown.
                        if vm.guest_reset() {
                            watcher_runtime.mark_guest_reset();
                        } else {
                            watcher_runtime.mark_guest_exited();
                        }
                    }
                }
                Err(err) => {
                    eprintln!(
                        "warning: VM {} could not watch for guest exit: {err}",
                        watcher_runtime.id
                    );
                }
            }
            // Stop and join the vCPU threads, then drop the VM (and with it the
            // guest memory) and the devices.
            vm.shutdown_vcpus();
            drop(devices);
        })?;
    runtime.add_thread(watcher);

    Ok(())
}

/// Installs the VM's console input hook: host bytes → the guest's console.
///
/// The device is held **weakly**. The console lives exactly as long as the VM
/// — it is owned by the exit watcher, the VM's last thread — and a strong
/// reference here would keep it (and its input queue) alive after the VM
/// stopped, silently accepting input that no guest will ever read. With a weak
/// reference, input sent to a stopped VM fails with "the VM has stopped",
/// which is what a caller that keeps a console open needs to know.
/// Spawns the VM's vsock event thread: the one thread that processes the
/// userspace device's virtqueues. It is registered with the runtime so the
/// spawner joins it on stop, and it polls the device's queue/activate
/// eventfds, the muxer's nested epoll fd and the VM's exit event.
///
/// `exit_evt` is the VM's shared exit event; the thread observes its
/// readability and exits *without* consuming it, because the exit watcher is
/// the VM's only reader. `exit_signalled` is the shared flag every exit path
/// sets before writing that event (the guest's vCPUs, the ACPI/i8042 reset
/// ports, and the stop control), and is what the thread actually trusts: an
/// event fd count the watcher consumed first is invisible to a level-triggered
/// `epoll_wait`, so the flag - not the wakeup - is the source of truth.
fn start_vsock_event_thread(
    exit_evt: &EventFd,
    exit_signalled: &Arc<AtomicBool>,
    device: Arc<Mutex<Vsock<VsockUnixBackend>>>,
    runtime: &Arc<VmRuntime>,
) -> io::Result<()> {
    let exit_evt = exit_evt.try_clone()?;
    let exit_signalled = Arc::clone(exit_signalled);
    let handle = crate::devices::virtio::vsock::event_thread::start_event_thread(
        device,
        exit_evt,
        exit_signalled,
        runtime.id,
    )?;
    runtime.add_thread(handle);
    Ok(())
}

fn set_serial_input_hook(runtime: &Arc<VmRuntime>, serial: &Arc<Mutex<SerialDevice>>) {
    let serial = Arc::downgrade(serial);
    runtime.set_input_hook(move |data| match serial.upgrade() {
        Some(serial) => {
            let mut serial = serial.lock().unwrap_or_else(PoisonError::into_inner);
            serial.queue_input(data)
        }
        None => Err("the VM has stopped".to_string()),
    });
}

/// Blocks until `evt` is signalled.
///
/// The event fds are created with `EFD_NONBLOCK` (a plain `read` would return
/// `EAGAIN` immediately), so this waits in [`crate::sys::poll_readable`] and
/// then drains the counter.
fn wait_for_event(evt: &EventFd) -> io::Result<()> {
    let fd = evt.as_raw_fd();
    loop {
        match crate::sys::poll_readable(fd, -1) {
            Ok(_) => {
                let _ = evt.read();
                return Ok(());
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::guest_cmdline;

    /// The `x86_64`-only `reboot=acpi` suffix this port appends to the guest
    /// command line (ACPI tables are x86_64-only; the knob is meaningless on
    /// `aarch64`, whose reboots go through PSCI).
    #[cfg(target_arch = "x86_64")]
    const REBOOT_ACPI: &str = " reboot=acpi";
    #[cfg(not(target_arch = "x86_64"))]
    const REBOOT_ACPI: &str = "";

    #[test]
    fn guest_cmdline_appends_console_defaults() {
        assert_eq!(
            guest_cmdline("console=ttyS0 panic=-1", false, false, false),
            format!("console=ttyS0 panic=-1 pci=off loglevel=7{REBOOT_ACPI}")
        );
        assert_eq!(
            guest_cmdline("", false, false, false),
            format!("pci=off loglevel=7{REBOOT_ACPI}")
        );
        assert_eq!(
            guest_cmdline("  panic=-1  ", false, false, false),
            format!("panic=-1 pci=off loglevel=7{REBOOT_ACPI}")
        );
    }

    #[test]
    fn guest_cmdline_respects_the_callers_settings() {
        assert_eq!(
            guest_cmdline("quiet", false, false, false),
            format!("quiet pci=off{REBOOT_ACPI}")
        );
        assert_eq!(
            guest_cmdline("debug", false, false, false),
            format!("debug pci=off{REBOOT_ACPI}")
        );
        assert_eq!(
            guest_cmdline("loglevel=3 ignore_loglevel", false, false, false),
            format!("loglevel=3 ignore_loglevel pci=off{REBOOT_ACPI}")
        );
        assert_eq!(
            guest_cmdline("pci=off loglevel=9", false, false, false),
            format!("pci=off loglevel=9{REBOOT_ACPI}")
        );
        assert_eq!(
            guest_cmdline("pci=nocrs", false, false, false),
            format!("pci=nocrs loglevel=7{REBOOT_ACPI}")
        );
        // A caller-chosen reboot mode is left alone.
        assert_eq!(
            guest_cmdline("reboot=k", false, false, false),
            "reboot=k pci=off loglevel=7"
        );
    }

    /// With a virtio-mmio device, the command line tells an `x86_64` guest
    /// where its transport is — an architecture without a device tree has no
    /// other way to say it. One entry per device, each with the port's own
    /// window and interrupt, and a caller that registered a transport itself is
    /// left alone entirely.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn guest_cmdline_registers_the_x86_64_virtio_transports() {
        use crate::arch::x86_64::layout::{
            BALLOON_GSI, BALLOON_MMIO_START, MMIO_LEN, VSOCK_GSI, VSOCK_MMIO_START, WORKSPACE_GSI,
            WORKSPACE_MMIO_START,
        };

        let vsock = format!("{MMIO_LEN:#x}@{VSOCK_MMIO_START:#x}:{VSOCK_GSI}");
        let workspace = format!("{MMIO_LEN:#x}@{WORKSPACE_MMIO_START:#x}:{WORKSPACE_GSI}");
        assert_ne!(
            vsock, workspace,
            "the two transports must not share a window or an interrupt"
        );

        // A vsock device, a workspace volume, or both.
        assert_eq!(
            guest_cmdline("console=ttyS0", true, false, false),
            format!("console=ttyS0 pci=off loglevel=7 reboot=acpi virtio_mmio.device={vsock}")
        );
        assert_eq!(
            guest_cmdline("console=ttyS0", false, true, false),
            format!("console=ttyS0 pci=off loglevel=7 reboot=acpi virtio_mmio.device={workspace}")
        );
        assert_eq!(
            guest_cmdline("console=ttyS0", true, true, false),
            format!(
                "console=ttyS0 pci=off loglevel=7 reboot=acpi virtio_mmio.device={vsock} \
                 virtio_mmio.device={workspace}"
            )
        );
        // A balloon gets its own window and interrupt, after the other two.
        let balloon = format!("{MMIO_LEN:#x}@{BALLOON_MMIO_START:#x}:{BALLOON_GSI}");
        assert!(balloon != vsock && balloon != workspace);
        assert_eq!(
            guest_cmdline("console=ttyS0", true, true, true),
            format!(
                "console=ttyS0 pci=off loglevel=7 reboot=acpi virtio_mmio.device={vsock} \
                 virtio_mmio.device={workspace} virtio_mmio.device={balloon}"
            )
        );
        // A VM with neither device registers no transport...
        assert_eq!(
            guest_cmdline("console=ttyS0", false, false, false),
            "console=ttyS0 pci=off loglevel=7 reboot=acpi"
        );
        // ...and a caller that registered transports itself keeps its own.
        assert_eq!(
            guest_cmdline("virtio_mmio.device=0x200@0x1000:6", true, true, false),
            "virtio_mmio.device=0x200@0x1000:6 pci=off loglevel=7 reboot=acpi"
        );
    }

    /// On `aarch64` the device tree describes the transports, so the command
    /// line is the same with and without them.
    #[cfg(target_arch = "aarch64")]
    #[test]
    fn guest_cmdline_adds_nothing_for_the_virtio_transports() {
        assert_eq!(
            guest_cmdline("console=ttyS0", true, true, false),
            guest_cmdline("console=ttyS0", false, false, false)
        );
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod x86_64 {
    use std::fs::File;
    use std::io;
    use std::sync::{Arc, Mutex, PoisonError};

    use linux_loader::loader::Cmdline;
    use vmm_sys_util::eventfd::EventFd;

    use crate::arch::x86_64::layout::{
        BALLOON_GSI, BALLOON_MMIO_START, CMDLINE_MAX_SIZE, MMIO_LEN, VSOCK_GSI, VSOCK_MMIO_START, WORKSPACE_GSI,
        WORKSPACE_MMIO_START,
    };
    use crate::arch::x86_64::{
        ConfigurationError, InitrdConfig, InitrdError, arch_memory_regions,
        configure_system_for_boot, load_kernel,
    };
    use crate::devices::legacy::{
        I8042Device, PM1_CNT_SIZE, Pm1CntDevice, SLEEP_CONTROL_PORT, SerialDevice, SerialOut,
    };
    use crate::devices::virtio::VirtioError;
    use crate::devices::virtio::block::device::VirtioBlock;
    use crate::devices::virtio::balloon::{SharedBalloon, VirtioBalloon};
    use crate::devices::virtio::block::mmio_device::BlockMmioDevice;
    use crate::devices::virtio::mmio::VirtioMmioTransport;
    use crate::devices::virtio::vsock::VsockError;
    use crate::devices::virtio::vsock::mmio_device::VsockMmioDevice;
    use crate::devices::virtio::vsock::unix::VsockUnixBackend;
    use crate::spawner::{SpawnError, VmRuntime, VmSpec};
    use crate::vstate::kvm::{Kvm, KvmError};
    use crate::vstate::memory::{MemoryError, anonymous, mib_to_bytes};
    use crate::vstate::vm::{KvmVm, StartVcpusError, VmError};

    /// Serial port 1 (`COM1`): data register base and register window.
    const SERIAL_PORT_ADDRESS: u64 = 0x3f8;
    /// Size of the legacy serial port window.
    const SERIAL_PORT_SIZE: u64 = 0x8;
    /// IRQ line of `COM1`, routed by the in-kernel irqchip.
    const COM1_GSI: u32 = 4;
    /// i8042 keyboard data register.
    const I8042_DATA_PORT: u64 = 0x060;
    /// Size of the i8042 data register window (0x60..0x64 inclusive).
    const I8042_DATA_PORT_SIZE: u64 = 0x5;
    /// IRQ line of the keyboard controller.
    const KBD_GSI: u32 = 1;

    /// Errors from [`boot`].
    #[derive(Debug, thiserror::Error)]
    pub enum BootError {
        /// Cannot open the kernel image {path}: {source}
        #[error("cannot open the kernel image {path}: {source}")]
        Kernel {
            /// Path of the kernel image that could not be opened.
            path: std::path::PathBuf,
            /// The error the open failed with.
            source: io::Error,
        },
        /// Cannot open the initrd image {path}: {source}
        #[error("cannot open the initrd image {path}: {source}")]
        Initrd {
            /// Path of the initrd image that could not be opened.
            path: std::path::PathBuf,
            /// The error the open failed with.
            source: io::Error,
        },
        /// Failed to allocate guest memory: {0}
        #[error("failed to allocate guest memory: {0}")]
        GuestMemory(#[from] MemoryError),
        /// Failed to initialize KVM: {0}
        #[error("failed to initialize KVM: {0}")]
        Kvm(#[from] KvmError),
        /// Failed to configure the VM: {0}
        #[error("failed to configure the VM: {0}")]
        Vm(#[from] VmError),
        /// Failed to start the vCPUs: {0}
        #[error("failed to start the vCPUs: {0}")]
        StartVcpus(#[from] StartVcpusError),
        /// Failed to configure the VM for boot: {0}
        #[error("failed to configure the VM for boot: {0}")]
        Configure(#[from] ConfigurationError),
        /// Failed to load the initrd into guest memory: {0}
        #[error("failed to load the initrd into guest memory: {0}")]
        InitrdLoad(#[from] InitrdError),
        /// Failed to build the kernel command line: {0}
        #[error("failed to build the kernel command line: {0}")]
        CommandLine(#[from] linux_loader::cmdline::Error),
        /// Failed to create the serial device: {0}
        #[error("failed to create the serial device: {0}")]
        Serial(io::Error),
        /// Failed to create the i8042 device: {0}
        #[error("failed to create the i8042 device: {0}")]
        I8042(io::Error),
        /// Failed to register a legacy device on the I/O bus: {0}
        #[error("failed to register a legacy device on the I/O bus: {0}")]
        Bus(#[from] crate::vstate::bus::BusError),
        /// Failed to register a device IRQ: {0}
        #[error("failed to register a device IRQ: {0}")]
        Irq(vmm_sys_util::errno::Error),
        /// Failed to create a virtio device: {0}
        #[error("failed to create a virtio device: {0}")]
        Virtio(#[from] VirtioError),
        /// Failed to create an event fd: {0}
        #[error("failed to create an event fd: {0}")]
        EventFd(io::Error),
        /// Failed to watch the VM exit event fd: {0}
        #[error("failed to watch the VM exit event fd: {0}")]
        WaitExitEvent(io::Error),
        /// Failed to enter the jailer's per-VM root: {0}
        #[error("failed to enter the jail: {0}")]
        Jail(#[from] crate::jail::jailer::JailerError),
    }

    /// Builds and boots the VM, leaving its vCPUs running.
    pub(super) fn boot(spec: &VmSpec, runtime: Arc<VmRuntime>) -> Result<(), SpawnError> {
        boot_inner(spec, &runtime).map_err(SpawnError::backend)
    }

    /// Guest memory: one region per architectural region (x86_64 splits the
    /// guest RAM around the 32-bit MMIO hole, so there can be two).
    fn boot_inner(spec: &VmSpec, runtime: &Arc<VmRuntime>) -> Result<(), BootError> {
        let regions = arch_memory_regions(mib_to_bytes(spec.memory_mib));
        let guest_regions = anonymous(&regions).map_err(BootError::from)?;

        let kvm = Kvm::new().map_err(BootError::from)?;
        let mut vm = KvmVm::new(kvm).map_err(BootError::from)?;

        // The vCPUs, and through them the in-kernel irqchip, must exist before
        // the guest memory is registered.
        let mut vcpus = vm.create_vcpus(spec.vcpus).map_err(BootError::from)?;
        vm.register_dram_memory_regions(guest_regions)
            .map_err(BootError::from)?;

        // The jailer, before the first device opens a path and before any VMM
        // thread exists (see `jail_the_vm` and `crate::jail::jailer`).
        super::jail_the_vm(spec, runtime.id).map_err(BootError::from)?;

        // The guest's only devices: the serial console and the i8042
        // controller (which emulates just enough to reset the machine), plus
        // the vsock device when the caller asked for one. There is no network
        // device by design.
        let serial = Arc::new(Mutex::new(
            SerialDevice::new(SerialOut::shared(Arc::clone(&runtime.serial)))
                .map_err(BootError::Serial)?,
        ));
        let reset_evt = vm
            .vcpus_exit_evt()
            .try_clone()
            .map_err(BootError::EventFd)?;
        let i8042 = Arc::new(Mutex::new(
            I8042Device::new(reset_evt, Arc::clone(&vm.common.exit_signalled))
                .map_err(BootError::I8042)?,
        ));
        // The PM1-control device: turns a guest ACPI S5 write (SLP_EN on the
        // sleep control register, port 0x604) into the VM's exit event as a
        // clean shutdown (reset=false), and a guest ACPI reboot (a write of
        // RESET_VALUE to the reset register, port 0x606) into the exit event
        // as a reset (reset=true) — the ACPI reboot path, which avoids the
        // BIOS real-mode jump that double-faults on this port. It shares the
        // exit event with the i8042 reset port, and publishes reset-vs-
        // shutdown through the VM's `guest_reset` flag, which the exit watcher
        // reads.
        let shutdown_evt = vm
            .vcpus_exit_evt()
            .try_clone()
            .map_err(BootError::EventFd)?;
        let pm1 = Arc::new(Mutex::new(Pm1CntDevice::new(
            shutdown_evt,
            Arc::clone(&vm.common.guest_reset),
            Arc::clone(&vm.common.exit_signalled),
            Arc::clone(&vm.common.vcpu_finish_requested),
        )));
        // `Arc<Mutex<Device>>` unsizes into the bus's `Arc<Mutex<dyn BusDevice>>`
        // at the call site.
        vm.pio_bus
            .insert(serial.clone(), SERIAL_PORT_ADDRESS, SERIAL_PORT_SIZE)?;
        vm.pio_bus
            .insert(i8042.clone(), I8042_DATA_PORT, I8042_DATA_PORT_SIZE)?;
        vm.pio_bus
            .insert(pm1.clone(), SLEEP_CONTROL_PORT, PM1_CNT_SIZE)?;
        {
            let serial = serial.lock().unwrap_or_else(PoisonError::into_inner);
            vm.register_irq(serial.serial.interrupt_evt(), COM1_GSI)
                .map_err(BootError::Irq)?;
        }
        {
            let i8042 = i8042.lock().unwrap_or_else(PoisonError::into_inner);
            vm.register_irq(&i8042.kbd_interrupt_evt, KBD_GSI)
                .map_err(BootError::Irq)?;
        }

        // The vsock device, when the caller configured one: a virtio-mmio
        // transport in the layout's MMIO window, its interrupt registered as a
        // KVM irqfd on the layout's GSI, and the *userspace* vsock device
        // behind it (the VMM's own muxer, not the kernel's vhost-vsock). The
        // muxer binds `uds_path`, so a taken path fails the boot here — before
        // the guest is started, instead of leaving it unreachable. (A CID
        // outside `MIN_GUEST_CID..=MAX_GUEST_CID` is refused even earlier, by
        // the spawner, before any device is built.) Its ring processing runs
        // on a dedicated event thread, never on the vCPU thread.
        let vsock = match &spec.vsock {
            Some(config) => {
                let irq = EventFd::new(libc::EFD_NONBLOCK).map_err(BootError::EventFd)?;
                vm.register_irq(&irq, VSOCK_GSI).map_err(BootError::Irq)?;
                let backend = VsockUnixBackend::new(
                    config.guest_cid,
                    config.uds_path.to_string_lossy().into_owned(),
                )
                .map_err(VsockError::from)
                .map_err(VirtioError::from)
                .map_err(BootError::Virtio)?;
                let device = Arc::new(Mutex::new(
                    super::Vsock::new(config.guest_cid, backend)
                        .map_err(VirtioError::from)
                        .map_err(BootError::Virtio)?,
                ));
                super::start_vsock_event_thread(
                    vm.vcpus_exit_evt(),
                    vm.exit_signalled(),
                    Arc::clone(&device),
                    runtime,
                )
                .map_err(BootError::EventFd)?;
                let transport = Arc::new(Mutex::new(VirtioMmioTransport::new(
                    Box::new(VsockMmioDevice::new(device)),
                    vm.guest_memory().clone(),
                    irq,
                )));
                vm.common
                    .mmio_bus
                    .insert(transport.clone(), VSOCK_MMIO_START, MMIO_LEN)?;
                Some(transport)
            }
            None => None,
        };

        // The block device, when the caller configured a workspace volume: a
        // second virtio-mmio transport in the layout's next MMIO slot, its own
        // interrupt registered as a KVM irqfd on the layout's next legacy GSI,
        // and the *writable* volume behind it — the only writable queue in this
        // port (invariant 3). The device opens the image read-write here, so a
        // path that does not exist (or is not writable) fails the boot with the
        // path in the error, before the guest is started.
        let workspace = match &spec.workspace {
            Some(image) => {
                let irq = EventFd::new(libc::EFD_NONBLOCK).map_err(BootError::EventFd)?;
                vm.register_irq(&irq, WORKSPACE_GSI).map_err(BootError::Irq)?;
                let device = VirtioBlock::new(image)
                    .map_err(VirtioError::from)
                    .map_err(BootError::Virtio)?;
                let transport = Arc::new(Mutex::new(VirtioMmioTransport::new(
                    Box::new(BlockMmioDevice::new(device)),
                    vm.guest_memory().clone(),
                    irq,
                )));
                vm.common
                    .mmio_bus
                    .insert(transport.clone(), WORKSPACE_MMIO_START, MMIO_LEN)?;
                Some(transport)
            }
            None => None,
        };

        // The balloon, when the caller asked for one: a third transport in the
        // next MMIO slot on the next legacy GSI. The VM is sized at
        // `memory_mib` and the balloon starts out holding everything above the
        // usable size, so the guest boots small and the host grows it later
        // through the runtime's memory control.
        let balloon = match spec.balloon {
            Some(usable) => {
                let irq = EventFd::new(libc::EFD_NONBLOCK).map_err(BootError::EventFd)?;
                vm.register_irq(&irq, BALLOON_GSI).map_err(BootError::Irq)?;
                let shared = SharedBalloon::new(VirtioBalloon::new());
                let transport = VirtioMmioTransport::new(
                    Box::new(shared.clone()),
                    vm.guest_memory().clone(),
                    irq,
                );
                shared.set_interrupt_status(transport.interrupt_status_handle());
                let transport = Arc::new(Mutex::new(transport));
                vm.common
                    .mmio_bus
                    .insert(transport.clone(), BALLOON_MMIO_START, MMIO_LEN)?;
                let max_mib = u32::try_from(spec.memory_mib).unwrap_or(u32::MAX);
                shared.set_usable_mib(max_mib, usable);
                runtime.set_memory_control(Arc::new(super::InProcessBalloon {
                    shared,
                    max_mib,
                }));
                Some(transport)
            }
            None => None,
        };

        // Host console input (a caller's bytes for the guest to read from
        // ttyS0): installed before the vCPUs start, so it is in place by the
        // time the spawner hands the VM out. It holds the console weakly, so
        // input sent after the VM stopped fails instead of being queued for a
        // guest that is gone.
        super::set_serial_input_hook(runtime, &serial);

        // The bus only holds weak references: the VM must own strong handles
        // to its devices for as long as it runs. They are moved into the exit
        // watcher below, which is the VM's last thread — so the devices live
        // exactly as long as the VM does.
        let legacy_devices = (serial, i8042, pm1, vsock, workspace, balloon);

        // Load the kernel (ELF first, bzImage fallback), the optional initrd,
        // and write the boot parameters into guest memory.
        let kernel = File::open(&spec.kernel).map_err(|source| BootError::Kernel {
            path: spec.kernel.clone(),
            source,
        })?;
        let entry_point = load_kernel(&kernel, vm.guest_memory()).map_err(BootError::from)?;
        let initrd = match &spec.initrd {
            Some(path) => {
                let file = File::open(path).map_err(|source| BootError::Initrd {
                    path: path.clone(),
                    source,
                })?;
                Some(InitrdConfig::from_file(vm.guest_memory(), file)?)
            }
            None => None,
        };
        let cmdline = Cmdline::try_from(
            super::guest_cmdline(
                &spec.cmdline,
                spec.vsock.is_some(),
                spec.workspace.is_some(),
                spec.balloon.is_some(),
            )
                .as_str(),
            CMDLINE_MAX_SIZE,
        )?;
        configure_system_for_boot(
            vm.kvm(),
            &vm,
            &mut vcpus,
            spec.vcpus,
            false,
            entry_point,
            &initrd,
            cmdline,
        )
        .map_err(BootError::from)?;

        // The exit watcher goes in before the vCPUs start: it is the VM's last
        // thread (the one the spawner joins) and it owns the stop control, so
        // whichever vCPU stops the VM - guest shutdown or reset, a vCPU
        // fault, or the i8042 reset port - writes `vcpus_exit_evt` and the
        // watcher stops and joins the vCPU threads, then drops the devices.
        // It only polls the event fd until then, so it does not need the vCPUs
        // to exist yet.
        let vm = Arc::new(vm);
        super::start_exit_watcher(Arc::clone(&vm), runtime, legacy_devices)
            .map_err(BootError::WaitExitEvent)?;

        if let Err(err) = vm.start_vcpus(vcpus) {
            // A vCPU that started and then failed to be joined, or one left
            // running, would fault through guest memory this function's caller
            // is about to free. `start_vcpus` already stopped and joined the
            // partial set; this makes sure of it, and lets the watcher finish
            // too (it is waiting for the VM's exit event, which nothing wrote
            // because these vCPUs never had a chance to leave the guest).
            vm.shutdown_vcpus();
            vm.exit_signalled()
                .store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = vm.vcpus_exit_evt().write(1);
            return Err(BootError::from(err));
        }

        Ok(())
    }
}

/// The aarch64/KVM boot path.
///
/// The order differs from x86_64 in one important way: the in-kernel vGIC must
/// be created *after* the vCPUs exist (`KVM_CREATE_VCPU` fails once the irqchip
/// is initialized on arm64), which `KvmVm::arch_post_create_vcpus` takes care
/// of inside `create_vcpus`.
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
mod aarch64 {
    use std::fs::File;
    use std::io;
    use std::sync::{Arc, Mutex, PoisonError};

    use linux_loader::loader::Cmdline;
    use vmm_sys_util::eventfd::EventFd;

    use crate::arch::aarch64::gic::GICVersion as KvmGicVersion;
    use crate::arch::aarch64::layout::{
        CMDLINE_MAX_SIZE, GSI_LEGACY_START, MMIO_LEN, SERIAL_MEM_START, VSOCK_GSI, VSOCK_MMIO_START,
        WORKSPACE_GSI, WORKSPACE_MMIO_START,
    };
    use crate::arch::aarch64::{
        ConfigurationError, InitrdConfig, InitrdError, SerialFdtInfo, VirtioMmioFdtInfo,
        arch_memory_regions, configure_system_for_boot, load_kernel,
    };
    use crate::devices::legacy::{SerialDevice, SerialOut};
    use crate::devices::virtio::VirtioError;
    use crate::devices::virtio::block::device::VirtioBlock;
    use crate::devices::virtio::block::mmio_device::BlockMmioDevice;
    use crate::devices::virtio::mmio::VirtioMmioTransport;
    use crate::devices::virtio::vsock::VsockError;
    use crate::devices::virtio::vsock::mmio_device::VsockMmioDevice;
    use crate::devices::virtio::vsock::unix::VsockUnixBackend;
    use crate::spawner::{GicVersion, SpawnError, VmRuntime, VmSpec};
    use crate::vstate::kvm::{Kvm, KvmError};
    use crate::vstate::memory::{MemoryError, anonymous, mib_to_bytes};
    use crate::vstate::vm::{KvmVm, StartVcpusError, VmError};

    /// The guest's only legacy device is the serial console, so it takes the
    /// first legacy GSI. The FDT and the KVM irqfd use the same number: the
    /// in-kernel vGIC shifts it by 32, so GSI 0 is SPI 32 in the guest's
    /// interrupt numbering.
    const SERIAL_GSI: u32 = GSI_LEGACY_START;

    /// Errors from [`boot`].
    #[derive(Debug, thiserror::Error)]
    pub enum BootError {
        /// Cannot open the kernel image {path}: {source}
        #[error("cannot open the kernel image {path}: {source}")]
        Kernel {
            /// Path of the kernel image that could not be opened.
            path: std::path::PathBuf,
            /// The error the open failed with.
            source: io::Error,
        },
        /// Cannot open the initrd image {path}: {source}
        #[error("cannot open the initrd image {path}: {source}")]
        Initrd {
            /// Path of the initrd image that could not be opened.
            path: std::path::PathBuf,
            /// The error the open failed with.
            source: io::Error,
        },
        /// Failed to allocate guest memory: {0}
        #[error("failed to allocate guest memory: {0}")]
        GuestMemory(#[from] MemoryError),
        /// Failed to initialize KVM: {0}
        #[error("failed to initialize KVM: {0}")]
        Kvm(#[from] KvmError),
        /// Failed to configure the VM: {0}
        #[error("failed to configure the VM: {0}")]
        Vm(#[from] VmError),
        /// Failed to start the vCPUs: {0}
        #[error("failed to start the vCPUs: {0}")]
        StartVcpus(#[from] StartVcpusError),
        /// Failed to configure the VM for boot: {0}
        #[error("failed to configure the VM for boot: {0}")]
        Configure(#[from] ConfigurationError),
        /// Failed to load the initrd into guest memory: {0}
        #[error("failed to load the initrd into guest memory: {0}")]
        InitrdLoad(#[from] InitrdError),
        /// Failed to build the kernel command line: {0}
        #[error("failed to build the kernel command line: {0}")]
        CommandLine(#[from] linux_loader::cmdline::Error),
        /// Failed to create the serial device: {0}
        #[error("failed to create the serial device: {0}")]
        Serial(io::Error),
        /// Failed to register the serial console on the MMIO bus: {0}
        #[error("failed to register the serial console on the MMIO bus: {0}")]
        Bus(#[from] crate::vstate::bus::BusError),
        /// Failed to register the serial console IRQ: {0}
        #[error("failed to register the serial console IRQ: {0}")]
        Irq(vmm_sys_util::errno::Error),
        /// Failed to create a virtio device: {0}
        #[error("failed to create a virtio device: {0}")]
        Virtio(#[from] VirtioError),
        /// Failed to create an event fd: {0}
        #[error("failed to create an event fd: {0}")]
        EventFd(io::Error),
        /// Failed to watch the VM exit event fd: {0}
        #[error("failed to watch the VM exit event fd: {0}")]
        WaitExitEvent(io::Error),
        /// Failed to enter the jailer's per-VM root: {0}
        #[error("failed to enter the jail: {0}")]
        Jail(#[from] crate::jail::jailer::JailerError),
    }

    /// Builds and boots the VM, leaving its vCPUs running.
    pub(super) fn boot(spec: &VmSpec, runtime: Arc<VmRuntime>) -> Result<(), SpawnError> {
        boot_inner(spec, &runtime).map_err(SpawnError::backend)
    }

    fn boot_inner(spec: &VmSpec, runtime: &Arc<VmRuntime>) -> Result<(), BootError> {
        // Guest memory: aarch64 has one DRAM region below the 64-bit MMIO hole,
        // unless the requested size exceeds the architectural maximum.
        let regions = arch_memory_regions(mib_to_bytes(spec.memory_mib));
        let guest_regions = anonymous(&regions)?;

        let kvm = Kvm::new()?;
        let mut vm = KvmVm::new(kvm)?;
        // The caller's GIC selection (`VmSpec::gic`); `None` keeps upstream's
        // default, GICv3 with a fallback to GICv2 (the Raspberry Pi 4/5 path).
        vm.set_gic_version(gic_version(spec.gic));

        // The vCPUs, and through them the in-kernel vGIC, must exist before the
        // guest memory is registered.
        let mut vcpus = vm.create_vcpus(spec.vcpus)?;
        vm.register_dram_memory_regions(guest_regions)?;

        // The jailer, before the first device opens a path and before any VMM
        // thread exists (see `jail_the_vm` and `crate::jail::jailer`).
        super::jail_the_vm(spec, runtime.id).map_err(BootError::from)?;

        // The guest's only device: the serial console, on the MMIO bus (there
        // is no network device by design).
        let serial = Arc::new(Mutex::new(
            SerialDevice::new(SerialOut::shared(Arc::clone(&runtime.serial)))
                .map_err(BootError::Serial)?,
        ));
        vm.common
            .mmio_bus
            .insert(serial.clone(), SERIAL_MEM_START, MMIO_LEN)?;
        {
            let serial = serial.lock().unwrap_or_else(PoisonError::into_inner);
            vm.register_irq(serial.serial.interrupt_evt(), SERIAL_GSI)
                .map_err(BootError::Irq)?;
        }

        // The guest's other device, when the caller configured one: the vsock
        // device, whose transport sits on the MMIO bus at the layout's window
        // with its interrupt on the layout's GSI (registered as a KVM irqfd,
        // like the console's). The *userspace* device (the VMM's own muxer,
        // not the kernel's vhost-vsock) binds `uds_path`, so a taken path
        // fails the boot here, before the guest is started. (A CID outside
        // `MIN_GUEST_CID..=MAX_GUEST_CID` is refused even earlier, by the
        // spawner.) Its ring processing runs on a dedicated event thread,
        // never on the vCPU thread.
        let vsock = match &spec.vsock {
            Some(config) => {
                let irq = EventFd::new(libc::EFD_NONBLOCK).map_err(BootError::EventFd)?;
                vm.register_irq(&irq, VSOCK_GSI).map_err(BootError::Irq)?;
                let backend = VsockUnixBackend::new(
                    config.guest_cid,
                    config.uds_path.to_string_lossy().into_owned(),
                )
                .map_err(VsockError::from)
                .map_err(VirtioError::from)
                .map_err(BootError::Virtio)?;
                let device = Arc::new(Mutex::new(
                    super::Vsock::new(config.guest_cid, backend)
                        .map_err(VirtioError::from)
                        .map_err(BootError::Virtio)?,
                ));
                super::start_vsock_event_thread(
                    vm.vcpus_exit_evt(),
                    vm.exit_signalled(),
                    Arc::clone(&device),
                    runtime,
                )
                .map_err(BootError::EventFd)?;
                let transport = Arc::new(Mutex::new(VirtioMmioTransport::new(
                    Box::new(VsockMmioDevice::new(device)),
                    vm.guest_memory().clone(),
                    irq,
                )));
                vm.common
                    .mmio_bus
                    .insert(transport.clone(), VSOCK_MMIO_START, MMIO_LEN)?;
                Some(transport)
            }
            None => None,
        };

        // The block device, when the caller configured a workspace volume: a
        // second virtio-mmio transport in the layout's next MMIO slot, its own
        // interrupt on the layout's next legacy GSI (registered as a KVM
        // irqfd, like the console's), and the *writable* volume behind it — the
        // only writable queue in this port (invariant 3). The device opens the
        // image read-write here, so a path that does not exist (or is not
        // writable) fails the boot with the path in the error, before the guest
        // is started.
        let workspace = match &spec.workspace {
            Some(image) => {
                let irq = EventFd::new(libc::EFD_NONBLOCK).map_err(BootError::EventFd)?;
                vm.register_irq(&irq, WORKSPACE_GSI).map_err(BootError::Irq)?;
                let device = VirtioBlock::new(image)
                    .map_err(VirtioError::from)
                    .map_err(BootError::Virtio)?;
                let transport = Arc::new(Mutex::new(VirtioMmioTransport::new(
                    Box::new(BlockMmioDevice::new(device)),
                    vm.guest_memory().clone(),
                    irq,
                )));
                vm.common
                    .mmio_bus
                    .insert(transport.clone(), WORKSPACE_MMIO_START, MMIO_LEN)?;
                Some(transport)
            }
            None => None,
        };

        // The device tree nodes that tell the guest where the virtio-mmio
        // transports are — the `aarch64` counterpart of the command-line
        // entries the `x86_64` backend appends. They are written with the rest
        // of the device tree below, after the devices are handed to the
        // watcher. Each node carries its own window and interrupt.
        let mut virtio_mmio = Vec::new();
        if vsock.is_some() {
            virtio_mmio.push(VirtioMmioFdtInfo {
                addr: VSOCK_MMIO_START,
                len: MMIO_LEN,
                gsi: VSOCK_GSI,
            });
        }
        if workspace.is_some() {
            virtio_mmio.push(VirtioMmioFdtInfo {
                addr: WORKSPACE_MMIO_START,
                len: MMIO_LEN,
                gsi: WORKSPACE_GSI,
            });
        }

        // Host console input (a caller's bytes for the guest to read from its
        // serial line): installed before the vCPUs start, so it is in place by
        // the time the spawner hands the VM out. It holds the console weakly,
        // so input sent after the VM stopped fails instead of being queued for
        // a guest that is gone.
        super::set_serial_input_hook(runtime, &serial);

        // The bus only holds weak references: the watcher below keeps the
        // strong handles, so the devices live exactly as long as the VM.
        // The balloon is `x86_64` only for now: this backend boots with all
        // of `memory_mib` and says so.
        if spec.balloon.is_some() {
            eprintln!("warning: no balloon on this backend yet; the VM gets all of its memory");
        }
        let legacy_devices = (serial, vsock, workspace);

        // Load the kernel (the PE loader, which covers arm64 `Image` and
        // EFI-stub images alike), the optional initrd, and write the boot
        // parameters — here the device tree — into guest memory.
        let kernel = File::open(&spec.kernel).map_err(|source| BootError::Kernel {
            path: spec.kernel.clone(),
            source,
        })?;
        let entry_point = load_kernel(&kernel, vm.guest_memory())?;
        let initrd = match &spec.initrd {
            Some(path) => {
                let file = File::open(path).map_err(|source| BootError::Initrd {
                    path: path.clone(),
                    source,
                })?;
                Some(InitrdConfig::from_file(vm.guest_memory(), file)?)
            }
            None => None,
        };
        let cmdline = Cmdline::try_from(
            super::guest_cmdline(
                &spec.cmdline,
                spec.vsock.is_some(),
                spec.workspace.is_some(),
                spec.balloon.is_some(),
            )
                .as_str(),
            CMDLINE_MAX_SIZE,
        )?;
        let serial_info = SerialFdtInfo {
            addr: SERIAL_MEM_START,
            len: MMIO_LEN,
            gsi: SERIAL_GSI,
        };
        configure_system_for_boot(
            &vm,
            &mut vcpus,
            entry_point,
            &initrd,
            cmdline,
            &serial_info,
            &virtio_mmio,
        )?;

        // The exit watcher goes in before the vCPUs start, exactly as on
        // x86_64: it owns the stop control and is the thread the spawner
        // joins, and it only polls the VM's exit event until the guest (or a
        // fault) stops the VM.
        let vm = Arc::new(vm);
        super::start_exit_watcher(Arc::clone(&vm), runtime, legacy_devices)
            .map_err(BootError::WaitExitEvent)?;

        if let Err(err) = vm.start_vcpus(vcpus) {
            // Stop and join whatever started before the failure: the caller is
            // about to free the VM and its guest memory, and a vCPU left
            // running would fault through memory that is no longer its own.
            vm.shutdown_vcpus();
            vm.exit_signalled()
                .store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = vm.vcpus_exit_evt().write(1);
            return Err(BootError::from(err));
        }

        Ok(())
    }

    /// Maps the spawner's arch-neutral GIC selection onto the KVM GIC versions.
    fn gic_version(requested: Option<GicVersion>) -> Option<KvmGicVersion> {
        match requested {
            // The backend default: GICv3, falling back to GICv2.
            None => None,
            Some(GicVersion::V2) => Some(KvmGicVersion::GICV2),
            Some(GicVersion::V3) => Some(KvmGicVersion::GICV3),
        }
    }
}
