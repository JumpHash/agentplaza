# Agents on Windows: the Windows Hypervisor Platform backend

Windows x64 nodes run agents in microVMs through the **Windows Hypervisor Platform** (WHP), the
same way Linux nodes use KVM and Apple Silicon uses Hypervisor.framework. The backend lives in
`crates/vmm/src/whp`; this page says what it builds, how to boot a guest by hand on a Windows
machine, and what is not done yet.

## What runs where

| piece | Windows x64 | Windows arm64 |
|---|---|---|
| hub (`apid`), desktop app, CLI, node link (`apz-link`) | yes | yes |
| `noded`, `vsockd`, `vmctl` (the VM node) | yes (shipped in the MSI) | built in CI only when the `APZ_WINDOWS_ARM64` variable is `on`; not shipped: no VM backend yet |
| agent VMs | x86_64 guests through WHP | not yet |

The guest is the one the Linux packages ship (`share\agentplaza\guest\x86_64\vmlinux` and
`initrd.gz`): the WHP backend builds the machine the KVM x86_64 backend builds — the same memory
map, MPTable, hardware-reduced ACPI tables, COM1 console, PM1 power/reset ports and the virtio-mmio
vsock transport announced on the kernel command line — so one guest image boots on both.

## How it is built

| what | KVM (Linux) | WHP (Windows) |
|---|---|---|
| VM | `KVM_CREATE_VM` | `WHvCreatePartition`, `WHvSetupPartition` |
| guest memory | KVM memory slots | `VirtualAlloc` + `WHvMapGpaRange` |
| vCPUs | `KVM_RUN`, one thread each | `WHvRunVirtualProcessor`, one thread each |
| port I/O and MMIO | `KVM_EXIT_IO` / `KVM_EXIT_MMIO` | exits decoded by the WHP instruction emulator (`WinHvEmulation.dll`) |
| local APIC | in-kernel irqchip | WHP's xAPIC emulation |
| I/O APIC | in-kernel irqchip | `whp::ioapic`, in the VMM |
| device interrupts | `EventFd` → KVM irqfd | `EventFd` → interrupt relay thread → I/O APIC → `WHvRequestInterrupt` |
| kernel loader, zero page | `linux-loader` | `whp::kernel`, `whp::zero_page` |
| vsock host side | Unix sockets `{dir}/vm{cid}_{port}` | AF_UNIX sockets (Windows 10 1803+), same paths, through `apz-uds` |
| event loop | `epoll`, `eventfd` | `pal::windows`: `WSAPoll` and loopback-socket events, same API |

`WinHvPlatform.dll` and `WinHvEmulation.dll` are loaded at run time (from System32 only), so
`noded` and `vmctl` start on machines without the feature and report it when a VM is booted.

The guest's TSC rate comes from the host (`WHvCapabilityCodeProcessorClockFrequency`) as
`tsc_early_khz=…`, with `tsc=reliable`: there is no kvmclock under WHP and the machine has no
PIT or HPET for the kernel to calibrate against. CPUID leaf 1 is answered with x2APIC masked,
because only the xAPIC is emulated.

### What is verified where

- The pure half — I/O APIC model, boot state, zero page, kernel loader, command line — has unit
  tests that run on every host (`cargo test -p vmm --lib whp::`).
- The FFI half and the rest of the node compile for `x86_64-pc-windows-msvc` and
  `aarch64-pc-windows-msvc`. `.github/workflows/windows.yml` runs the workspace tests on the org's
  self-hosted Windows x64 runner, on pushes to main (called by `release.yml`) and by hand. The arm64 leg
  runs only when the `APZ_WINDOWS_ARM64` variable is `on`: there the tests are built, not run. The
  x64 run executes the WinSock event shim's and `apz-uds`'s tests for real.
- A guest boot needs a Windows machine with the hypervisor: the procedure below.

## Manual boot

### 1. Enable the platform

Windows 11 (or Windows 10 2004 and later), x64, with virtualization enabled in the firmware.
In an elevated PowerShell:

```powershell
Enable-WindowsOptionalFeature -Online -FeatureName HypervisorPlatform -All
```

Restart. (Equivalently: *Turn Windows features on or off* → **Windows Hypervisor Platform**.)
Inside a VM, nested virtualization has to be enabled for that VM by its host.

### 2. Get the binaries and the guest

Either install the MSI (`AgentPlaza-<version>-windows-x64.msi`; the binaries go to
`C:\Program Files\AgentPlaza\bin`, the guest to `…\share\agentplaza\guest\x86_64`), or build from a
checkout:

```powershell
cargo build --release -p noded -p vsockd -p vmctl
```

and take `vmlinux` and `initrd.gz` from a Linux build (`scripts/mk-guest.sh`, which writes
`target/guest/x86_64/`) or from a Linux package's `share/agentplaza/guest/x86_64`.

Below, `$G` is the guest directory and the binaries are on `PATH`:

```powershell
$G = "C:\Program Files\AgentPlaza\share\agentplaza\guest\x86_64"
```

### 3. Serial boot (no vsock)

```powershell
vmctl --kernel "$G\vmlinux" --initrd "$G\initrd.gz" --cmdline "console=ttyS0 panic=-1" --timeout-secs 60
```

Expected: the kernel banner and boot log on the console (`Linux version …`, `tsc: Detected …`,
`virtio_mmio` absent since no vsock was asked for), ending in the initrd's shell or its exit. A
`reboot`/power-off in the guest ends the VM (`reboot=acpi` is appended for that).

### 4. Heartbeat over vsock

In one terminal, the host end of the guest protocol for guest CID 3:

```powershell
vsockd --cids 3
```

It listens at `%LOCALAPPDATA%\AgentPlaza\state\vsock\vm3_5000` (the guest protocol) and `vm3_5001`
(the egress proxy), and serves its status on `http://127.0.0.1:19080`.

In a second terminal, a keepalive agent VM with CID 3:

```powershell
vmctl --kernel "$G\vmlinux" --initrd "$G\initrd.gz" --vsock-cid 3 --timeout-secs 120 `
  --cmdline "console=ttyS0 plaza.harness=1 plaza.keepalive=1 plaza.vm=0"
```

**Done when** `vsockd` logs `vsockd: agent 3 heartbeat 1` (then every 12th beat; one beat every
5 s) and the agent is listed by

```powershell
curl.exe http://127.0.0.1:19080/v1/status
```

with a recent `last_heartbeat_ms`. That is the full path working: guest virtio-vsock driver →
virtio-mmio transport and the WHP MMIO exits → the userspace vsock device and muxer → an AF_UNIX
connection to `vsockd` → interrupts back through the I/O APIC.

### 5. Through the node

`noded` boots the same VMs for the hub (`POST /v1/runs`); the desktop app starts `noded` and
`vsockd` with the platform's socket directory. On Windows the per-VM process mode
(`APZ_VMM_SPAWN=process`) is Linux-only and falls back to in-process with a warning.

## Troubleshooting

| symptom | cause |
|---|---|
| `WinHvPlatform.dll could not be loaded` | the feature is not enabled (step 1), or no restart since |
| `the Windows Hypervisor Platform reports no hypervisor` | virtualization off in the firmware, or a VM without nested virtualization |
| `WHvSetupPartition failed: HRESULT …` / `WHvCreatePartition failed: HRESULT …` | another hypervisor holds VT-x/AMD-V, or an old Windows build |
| no console output at all | `console=ttyS0` missing from `--cmdline` |
| boot stops around TSC or APIC timer calibration | the host clock rate was not readable: add `tsc_early_khz=<kHz of your CPU>` to `--cmdline` |
| `MMIO … could not be emulated` | report it with the address: a device access the instruction emulator could not decode |
| `vsockd` exits with `another listener is already serving …` | a live listener already holds that socket path (another `vsockd`, or another process): stop it. A leftover socket file from a crashed run is removed by `vsockd` itself |

## Not done yet

- **Memory balloon**: Linux-only. A balloon is ignored with a warning. (The workspace and the
  shared data volumes are attached, in the KVM x86_64 slots.)
- **Confinement**: on Linux every VMM thread runs under a seccomp allowlist, and the per-VM process
  mode adds the jailer (namespaces, `pivot_root`, dropped ids). On Windows the VMM runs as threads
  of `noded` with the user's rights; a Windows equivalent (an AppContainer or a restricted token per
  VM process) is future work.
- **Windows arm64**: WHP on arm64 has its own register set and an in-hypervisor GICv3; the guest
  path for it (the HVF backend's device tree and memory map on WHP) is not written. The platform
  layer (`whp::sys`, `whp::vm`) builds there already.
- **More than one vCPU** is wired (application processors start parked until the guest's
  INIT/SIPI); start a first boot with one vCPU (the default), then try `--vcpus 2`.
