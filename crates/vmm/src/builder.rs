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

use std::sync::Arc;

use crate::spawner::{SpawnError, VmRuntime, VmSpec};

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
        let _ = (spec, runtime);
        Err(SpawnError::NoBackend)
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = (spec, runtime);
        Err(SpawnError::NoBackend)
    }
}

/// Kernel command line handed to the guest: the caller's, plus the defaults
/// this port needs.
///
/// Two options are appended when the caller has not set them:
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
///
/// This is deliberate and documented, not a hidden rewrite: the appended
/// strings are constants, never host state (no environment variable, token or
/// path can reach the guest this way).
fn guest_cmdline(requested: &str) -> String {
    const LOGLEVEL_ARGS: [&str; 4] = ["loglevel=", "ignore_loglevel", "quiet", "debug"];
    const PCI_ARGS: [&str; 1] = ["pci="];

    let requested = requested.trim();
    let args: Vec<&str> = requested.split_whitespace().collect();
    let sets = |prefixes: &[&str]| {
        args.iter()
            .any(|arg| prefixes.iter().any(|prefix| arg.starts_with(prefix)))
    };

    let mut defaults: Vec<&str> = Vec::new();
    if !sets(&PCI_ARGS) {
        defaults.push("pci=off");
    }
    if !sets(&LOGLEVEL_ARGS) {
        defaults.push("loglevel=7");
    }

    if defaults.is_empty() {
        return requested.to_string();
    }
    if requested.is_empty() {
        defaults.join(" ")
    } else {
        format!("{requested} {}", defaults.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::guest_cmdline;

    #[test]
    fn guest_cmdline_appends_console_defaults() {
        assert_eq!(
            guest_cmdline("console=ttyS0 panic=-1"),
            "console=ttyS0 panic=-1 pci=off loglevel=7"
        );
        assert_eq!(guest_cmdline(""), "pci=off loglevel=7");
        assert_eq!(guest_cmdline("  panic=-1  "), "panic=-1 pci=off loglevel=7");
    }

    #[test]
    fn guest_cmdline_respects_the_callers_settings() {
        assert_eq!(guest_cmdline("quiet"), "quiet pci=off");
        assert_eq!(guest_cmdline("debug"), "debug pci=off");
        assert_eq!(
            guest_cmdline("loglevel=3 ignore_loglevel"),
            "loglevel=3 ignore_loglevel pci=off"
        );
        assert_eq!(guest_cmdline("pci=off loglevel=9"), "pci=off loglevel=9");
        assert_eq!(guest_cmdline("pci=nocrs"), "pci=nocrs loglevel=7");
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod x86_64 {
    use std::fs::File;
    use std::io;
    use std::os::fd::AsRawFd;
    use std::sync::{Arc, Mutex, PoisonError};
    use std::thread;

    use linux_loader::loader::Cmdline;
    use vmm_sys_util::eventfd::EventFd;

    use crate::arch::x86_64::layout::CMDLINE_MAX_SIZE;
    use crate::arch::x86_64::{
        ConfigurationError, InitrdConfig, InitrdError, arch_memory_regions,
        configure_system_for_boot, load_kernel,
    };
    use crate::devices::legacy::{I8042Device, SerialDevice, SerialOut};
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
        /// Failed to create an event fd: {0}
        #[error("failed to create an event fd: {0}")]
        EventFd(io::Error),
        /// Failed to watch the VM exit event fd: {0}
        #[error("failed to watch the VM exit event fd: {0}")]
        WaitExitEvent(io::Error),
    }

    /// Builds and boots the VM, leaving its vCPUs running.
    pub(super) fn boot(spec: &VmSpec, runtime: Arc<VmRuntime>) -> Result<(), SpawnError> {
        boot_inner(spec, &runtime).map_err(SpawnError::backend)
    }

    fn boot_inner(spec: &VmSpec, runtime: &Arc<VmRuntime>) -> Result<(), BootError> {
        // Guest memory: one region per architectural region (x86_64 splits the
        // guest RAM around the 32-bit MMIO hole, so there can be two).
        let regions = arch_memory_regions(mib_to_bytes(spec.memory_mib));
        let guest_regions = anonymous(&regions).map_err(BootError::from)?;

        let kvm = Kvm::new().map_err(BootError::from)?;
        let mut vm = KvmVm::new(kvm).map_err(BootError::from)?;

        // The vCPUs, and through them the in-kernel irqchip, must exist before
        // the guest memory is registered.
        let mut vcpus = vm.create_vcpus(spec.vcpus).map_err(BootError::from)?;
        vm.register_dram_memory_regions(guest_regions)
            .map_err(BootError::from)?;

        // The guest's only devices: the serial console and the i8042
        // controller (which emulates just enough to reset the machine). There
        // is no network device by design.
        let serial = Arc::new(Mutex::new(
            SerialDevice::new(SerialOut::shared(Arc::clone(&runtime.serial)))
                .map_err(BootError::Serial)?,
        ));
        let reset_evt = vm.vcpus_exit_evt().try_clone().map_err(BootError::EventFd)?;
        let i8042 = Arc::new(Mutex::new(
            I8042Device::new(reset_evt).map_err(BootError::I8042)?,
        ));
        // `Arc<Mutex<Device>>` unsizes into the bus's `Arc<Mutex<dyn BusDevice>>`
        // at the call site.
        vm.pio_bus
            .insert(serial.clone(), SERIAL_PORT_ADDRESS, SERIAL_PORT_SIZE)?;
        vm.pio_bus
            .insert(i8042.clone(), I8042_DATA_PORT, I8042_DATA_PORT_SIZE)?;
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

        // The bus only holds weak references: the VM must own strong handles
        // to the legacy devices for as long as it runs. They are moved into
        // the exit watcher below, which is the VM's last thread — so the
        // devices live exactly as long as the VM does.
        let legacy_devices = (serial, i8042);

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
        let cmdline =
            Cmdline::try_from(super::guest_cmdline(&spec.cmdline).as_str(), CMDLINE_MAX_SIZE)?;
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

        // Start the vCPUs and then the exit watcher: whichever vCPU stops the
        // VM (guest shutdown/reset, a vCPU fault, or the i8042 reset port)
        // writes `vcpus_exit_evt`, and the watcher — the only thread the
        // spawner joins — stops and joins the vCPU threads.
        let vm = Arc::new(vm);
        vm.start_vcpus(vcpus).map_err(BootError::from)?;

        let stop_evt = vm.vcpus_exit_evt().try_clone().map_err(BootError::EventFd)?;
        runtime.set_stop_control(move || {
            if let Err(err) = stop_evt.write(1) {
                eprintln!("warning: failed to signal VM exit: {err}");
            }
        });

        let exit_evt = vm.vcpus_exit_evt().try_clone().map_err(BootError::EventFd)?;
        let watcher_runtime = Arc::clone(runtime);
        let watcher = thread::Builder::new()
            .name(format!("apz_vm{}_exit", runtime.id))
            .spawn(move || {
                match wait_for_event(&exit_evt) {
                    Ok(()) => {
                        if !watcher_runtime.stop_requested() {
                            watcher_runtime.mark_guest_exited();
                        }
                    }
                    Err(err) => {
                        eprintln!(
                            "warning: VM {} could not watch for guest exit: {err}",
                            watcher_runtime.id
                        );
                    }
                }
                // Stop and join the vCPU threads, then drop the VM (and with
                // it the guest memory and the legacy devices).
                vm.shutdown_vcpus();
                drop(legacy_devices);
            })
            .map_err(BootError::WaitExitEvent)?;
        runtime.add_thread(watcher);

        Ok(())
    }

    /// Blocks until `evt` is signalled.
    ///
    /// The event fds are created with `EFD_NONBLOCK` (a plain `read` would
    /// return `EAGAIN` immediately), so this waits with `poll(2)` and then
    /// drains the counter.
    fn wait_for_event(evt: &EventFd) -> io::Result<()> {
        let fd = evt.as_raw_fd();
        loop {
            let mut pfd = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: `pfd` points to one initialized `pollfd` for a file
            // descriptor this process owns and keeps open for the duration of
            // the call.
            let ret = unsafe { libc::poll(&mut pfd, 1, -1) };
            if ret < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            let _ = evt.read();
            return Ok(());
        }
    }
}
