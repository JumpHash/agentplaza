//! Workstation VMs: the desktop guests a node makes for the hub (docs/design/workstations.md, section 10).
//!
//! A workstation VM is one disk (its own sparse copy of the guest image, grown to the requested size), the
//! desktop guest's kernel and initramfs, and the VMM's vCPUs and memory. The guest runs the workstation daemon. It has
//! no NIC, so it reaches its hub only through this node's relay (`ws_relay`) over its vsock. The relay serves the join
//! config (the hub's link address, its pin and the join code) once, so the join code is never on the kernel command line,
//! and that holds on both backends. The guest's display is the daemon's own virtual display (the guest recipe runs
//! Xvfb), so the node's screen is never touched.
//!
//! Snapshot and revert work on the disk. The VMM takes no live snapshots, so the VM is stopped, its disk is copied
//! to (or back from) a snapshot file, and the VM starts again. The hub sequences those calls around a lease.
//!
//! GPU options: `none`; `virtual` (the guest's software renderer, no device attached); and `vfio:<pci-id>`, which is
//! checked against sysfs and refused unless the device is bound to `vfio-pci` with an IOMMU group. The node never
//! rebinds a device itself.
//!
//! Two launch backends (ws_qemu.rs). `plaza` is apz-vmm: Linux by direct boot, no PCI device, so it refuses a Windows
//! guest and a vfio GPU, validated first, so the refusal names the cause (`VMM_BOOTS_WINDOWS`, `VMM_ATTACHES_VFIO`).
//! `qemu` is QEMU/KVM: UEFI for a Windows guest, the node's desktop kernel for a Linux one, and VFIO. It is chosen for
//! a Windows guest or a vfio GPU, or when asked for, and only on a node that reports QEMU and its firmware. macOS guests
//! are refused everywhere on this node; the hub only places them on Apple hardware, capped at two per Mac.
//!
//! The directory layout is `<dir>/<id>/vm.json` (the record, never the join code). A plaza VM keeps `disk.img` and,
//! after a snapshot, `snapshot.img`. A QEMU VM keeps `disk.qcow2`, an overlay, and `snapshot.qcow2`, plus `vars.fd`
//! (its UEFI variables, Windows only). A record found at start is loaded as a stopped VM, except a QEMU VM that a previous
//! noded left running: its pidfile and QMP socket answer, so it is taken back as running and its relay is restarted.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use apz_station::vsock::{NodeConfig, config_address, link_address};
use serde::{Deserialize, Serialize};
use vmm::spawner::{self, VmHandle, VmSpec, VsockConfig};

use crate::ws_qemu::{self, Available};
use crate::ws_relay::{self, Destination, Relay};

/// The longest workstation name, in characters (a DNS label, so it can be the guest's hostname).
pub const MAX_NAME_LEN: usize = 63;
/// The most vCPUs one workstation VM may have.
pub const MAX_CPUS: u32 = 16;
/// The least memory a Linux workstation VM may have, in MiB.
pub const MIN_RAM_MB: u32 = 2048;
/// The least memory a Windows workstation VM may have, in MiB.
pub const MIN_WINDOWS_RAM_MB: u32 = 4096;
/// The most memory a workstation VM may have, in MiB.
pub const MAX_RAM_MB: u32 = 131_072;
/// The smallest disk a workstation VM may have, in GiB.
pub const MIN_DISK_GB: u32 = 8;
/// The largest disk a workstation VM may have, in GiB.
pub const MAX_DISK_GB: u32 = 2_048;
/// The longest join code accepted, in characters (the hub's own limit for a join token).
pub const MAX_JOIN_CODE: usize = 4_096;
/// Whether apz-vmm can boot a Windows guest. It boots Linux kernels by direct boot only, so this is false.
pub const VMM_BOOTS_WINDOWS: bool = false;
/// Whether apz-vmm can attach a PCI device for `vfio:` passthrough. It cannot yet, so this is false.
pub const VMM_ATTACHES_VFIO: bool = false;

/// The first guest CID a workstation VM may have, on either backend. Agents use 100 and up, and runs 20..=99 by default,
/// so this base sits clear of both. A VM's CID is `CID_BASE` plus its sequence number ([`cid_for`]).
pub const CID_BASE: u32 = 4096;

const GIB: u64 = 1 << 30;
const NO_KERNEL: &str = "no workstation kernel is configured: build the desktop guest with scripts/mk-workstation-guest.sh and start noded with --workstation-kernel and --workstation-initrd";
const NO_IMAGE: &str = "no workstation disk image is configured: build it with scripts/mk-workstation-guest.sh and start noded with --workstation-image";
const NO_QEMU: &str = "this node has no QEMU/OVMF; install qemu and edk2-ovmf";
const NO_UEFI: &str =
    "this node has no UEFI firmware (OVMF) for a Windows guest; install edk2-ovmf";
/// The plaza VM's disk, and its snapshot.
const PLAZA_DISK: &str = "disk.img";
const PLAZA_SNAPSHOT: &str = "snapshot.img";
/// The QEMU VM's lease overlay, its snapshot, and its UEFI variables.
const QEMU_DISK: &str = "disk.qcow2";
const QEMU_SNAPSHOT: &str = "snapshot.qcow2";
const QEMU_VARS: &str = "vars.fd";

/// A refusal: the HTTP status and the message.
pub type Refusal = (u16, String);

/// The launch backend a workstation VM runs under.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// apz-vmm, the plaza's own VMM.
    #[default]
    Plaza,
    /// QEMU/KVM (ws_qemu.rs).
    Qemu,
}

/// What a request asks for: a backend, or `auto`, the node's choice ([`choose`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Auto,
    Plaza,
    Qemu,
}

impl Choice {
    /// Reads `auto` (or nothing), `plaza` or `qemu`.
    pub fn parse(text: Option<&str>) -> Result<Self, String> {
        match text {
            None | Some("auto") => Ok(Self::Auto),
            Some("plaza") => Ok(Self::Plaza),
            Some("qemu") => Ok(Self::Qemu),
            Some(other) => Err(format!(
                "backend must be auto, plaza or qemu, not {other:?}"
            )),
        }
    }
}

/// The guest operating systems a workstation VM may run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Linux,
    Windows,
    Macos,
}

/// A workstation VM's GPU: none, the guest's software renderer, or a PCI device passed through with VFIO.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Gpu {
    #[default]
    None,
    Virtual,
    Vfio(String),
}

impl Gpu {
    /// Reads `none`, `virtual` or `vfio:<pci-id>` (the PCI address is `DDDD:BB:DD.F`, in lower case).
    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "none" => Ok(Self::None),
            "virtual" => Ok(Self::Virtual),
            _ => match text.strip_prefix("vfio:") {
                Some(pci) => parse_pci_id(pci).map(Self::Vfio),
                None => Err(format!(
                    "gpu must be none, virtual or vfio:<pci-id>, not {text:?}"
                )),
            },
        }
    }

    /// The option as the request spells it.
    pub fn text(&self) -> String {
        match self {
            Self::None => "none".into(),
            Self::Virtual => "virtual".into(),
            Self::Vfio(pci) => format!("vfio:{pci}"),
        }
    }
}

impl TryFrom<String> for Gpu {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        Self::parse(&text)
    }
}

impl From<Gpu> for String {
    fn from(gpu: Gpu) -> String {
        gpu.text()
    }
}

/// A PCI address, `DDDD:BB:DD.F`, in lower case: domain, bus and device in hex, and a function from 0 to 7.
pub fn parse_pci_id(text: &str) -> Result<String, String> {
    let lower = text.to_ascii_lowercase();
    let well_formed = lower.len() == 12
        && lower.char_indices().all(|(at, c)| match at {
            4 | 7 => c == ':',
            10 => c == '.',
            11 => ('0'..='7').contains(&c),
            _ => c.is_ascii_hexdigit(),
        });
    if well_formed {
        Ok(lower)
    } else {
        Err(format!(
            "a vfio GPU needs a PCI address like 0000:01:00.0, not {text:?}"
        ))
    }
}

/// The `POST /v1/workstations` body: what the hub asks for, and the join code the guest enrols with.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateWorkstation {
    pub name: String,
    pub os: Os,
    pub cpus: u32,
    pub ram_mb: u32,
    pub disk_gb: u32,
    #[serde(default)]
    pub gpu: Gpu,
    /// For a Windows VM, the owner's disk image on this node (an absolute path, copied per VM, never booted in place).
    /// A Linux VM uses the node's desktop image unless this names another one.
    #[serde(default)]
    pub image: Option<String>,
    /// The hub's URL the daemon joins (`https://...`). The guest reaches it through this node's relay.
    #[serde(default)]
    pub hub: Option<String>,
    /// The hub's certificate pin (base64url). The guest's TLS is verified against it, so a VM with a hub needs one.
    #[serde(default)]
    pub pin: Option<String>,
    pub join_code: String,
    /// `auto` (the default), `plaza` or `qemu`.
    #[serde(default)]
    pub backend: Option<String>,
}

/// A request that passed every check that needs no file system or device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape {
    pub name: String,
    pub os: Os,
    pub cpus: u8,
    pub ram_mb: u32,
    pub disk_gb: u32,
    pub gpu: Gpu,
    pub image: Option<PathBuf>,
    pub hub: Option<String>,
    pub pin: Option<String>,
    pub join_code: String,
    pub backend: Choice,
}

impl CreateWorkstation {
    /// Checks the request: its name, sizes, OS, image and join code. Nothing is made or read here.
    pub fn checked(&self) -> Result<Shape, String> {
        if self.os == Os::Macos {
            return Err("macOS workstation VMs need Apple's Virtualization framework, which this node does not run".into());
        }
        checked_name(&self.name)?;
        let cpus = u8::try_from(in_range("cpus", self.cpus, 1, MAX_CPUS)?)
            .map_err(|_| "cpus is out of range".to_string())?;
        let min_ram = if self.os == Os::Windows {
            MIN_WINDOWS_RAM_MB
        } else {
            MIN_RAM_MB
        };
        let ram_mb = in_range("ram_mb", self.ram_mb, min_ram, MAX_RAM_MB)?;
        let disk_gb = in_range("disk_gb", self.disk_gb, MIN_DISK_GB, MAX_DISK_GB)?;
        let image = match (self.os, &self.image) {
            (Os::Windows, None) => {
                return Err("a Windows workstation needs image: the path of an owner-provided disk image on this node".into());
            }
            (_, Some(path)) => Some(checked_image(path)?),
            (_, None) => None,
        };
        let hub = match &self.hub {
            Some(url) => Some(checked_hub(url)?),
            None => None,
        };
        let pin = match (&hub, &self.pin) {
            (Some(_), None) => {
                return Err("a workstation with a hub needs the hub's certificate pin: pin".into());
            }
            (_, Some(pin)) => Some(checked_pin(pin)?),
            (None, None) => None,
        };
        checked_join_code(&self.join_code)?;
        let backend = Choice::parse(self.backend.as_deref())?;
        Ok(Shape {
            name: self.name.clone(),
            os: self.os,
            cpus,
            ram_mb,
            disk_gb,
            gpu: self.gpu.clone(),
            image,
            hub,
            pin,
            join_code: self.join_code.clone(),
            backend,
        })
    }
}

/// A workstation's name: 1 to [`MAX_NAME_LEN`] characters of `a-z`, `0-9` and `-`, not starting or ending with `-`.
pub fn checked_name(name: &str) -> Result<(), String> {
    let valid = (1..=MAX_NAME_LEN).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "name must be 1 to {MAX_NAME_LEN} characters of a-z, 0-9 and '-', not starting or ending with '-': {name:?}"
        ))
    }
}

fn in_range(field: &str, value: u32, min: u32, max: u32) -> Result<u32, String> {
    if (min..=max).contains(&value) {
        Ok(value)
    } else {
        Err(format!(
            "{field} must be between {min} and {max}, not {value}"
        ))
    }
}

fn checked_image(path: &str) -> Result<PathBuf, String> {
    if path.contains('\0') || !Path::new(path).is_absolute() {
        return Err(format!(
            "image must be an absolute path to a disk image on this node, not {path:?}"
        ));
    }
    Ok(PathBuf::from(path))
}

fn checked_hub(url: &str) -> Result<String, String> {
    let plain = url.starts_with("https://") || url.starts_with("http://");
    if !plain || url.len() > 256 || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!(
            "hub must be an http or https URL with no spaces, not {url:?}"
        ));
    }
    Ok(url.trim_end_matches('/').to_string())
}

/// A certificate pin: base64url text of 1 to 64 characters. The daemon decodes it and checks it is 32 bytes.
fn checked_pin(pin: &str) -> Result<String, String> {
    let valid = (1..=64).contains(&pin.len())
        && pin
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if valid {
        Ok(pin.to_string())
    } else {
        Err("pin is not a base64url certificate pin".into())
    }
}

fn checked_join_code(code: &str) -> Result<(), String> {
    if code.is_empty() {
        return Err("the join code is empty".into());
    }
    if code.chars().count() > MAX_JOIN_CODE {
        return Err(format!(
            "the join code is longer than {MAX_JOIN_CODE} characters"
        ));
    }
    if code.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("the join code contains whitespace or control characters".into());
    }
    Ok(())
}

/// Checks that PCI device `pci` is ready for passthrough under `sysfs` (the sysfs root): the device exists, its
/// driver is `vfio-pci`, and it has an IOMMU group. A device bound to anything else is refused with what it is bound
/// to; the node never unbinds or rebinds a device.
pub fn check_vfio(sysfs: &Path, pci: &str) -> Result<(), String> {
    let device = sysfs.join("bus/pci/devices").join(pci);
    if !device.exists() {
        return Err(format!("no PCI device {pci} on this node"));
    }
    let driver = fs::read_link(device.join("driver")).map_err(|_| {
        format!("PCI device {pci} has no driver: bind it to vfio-pci first (the node does not rebind devices)")
    })?;
    let bound = driver
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if bound != "vfio-pci" {
        return Err(format!(
            "PCI device {pci} is bound to {bound}, not vfio-pci: unbind it and bind it to vfio-pci first (the node does not rebind devices)"
        ));
    }
    if fs::read_link(device.join("iommu_group")).is_err() {
        return Err(format!(
            "PCI device {pci} has no IOMMU group: turn the IOMMU on (intel_iommu=on or amd_iommu=on)"
        ));
    }
    Ok(())
}

/// Why plaza cannot boot a VM of this shape, or `None` when it can. QEMU runs what plaza refuses.
pub fn boot_refusal(os: Os, gpu: &Gpu) -> Option<String> {
    if os == Os::Windows && !VMM_BOOTS_WINDOWS {
        return Some("Windows workstation VMs cannot boot on plaza: apz-vmm boots Linux kernels by direct boot and has no firmware path for Windows; they run under QEMU".into());
    }
    if matches!(gpu, Gpu::Vfio(_)) && !VMM_ATTACHES_VFIO {
        return Some("GPU passthrough (vfio) cannot boot on plaza: apz-vmm cannot attach a PCI device; the device was checked, nothing was bound. It runs under QEMU".into());
    }
    None
}

/// The backend a VM of this shape runs under. `auto` takes QEMU for a Windows guest or a vfio GPU, which plaza cannot
/// run, and plaza otherwise. A request for QEMU, or an `auto` that needs it, is refused when the node has none.
pub fn choose(
    choice: Choice,
    os: Os,
    gpu: &Gpu,
    qemu: Option<&Available>,
) -> Result<Backend, Refusal> {
    let wants_qemu = match choice {
        Choice::Plaza => return Ok(Backend::Plaza),
        Choice::Qemu => true,
        Choice::Auto => os == Os::Windows || matches!(gpu, Gpu::Vfio(_)),
    };
    if !wants_qemu {
        return Ok(Backend::Plaza);
    }
    let available = qemu.ok_or((503, NO_QEMU.to_string()))?;
    if os == Os::Windows && available.firmware.is_none() {
        return Err((
            501,
            "a Windows workstation VM under QEMU needs UEFI firmware (OVMF), which this node's architecture does not have".into(),
        ));
    }
    Ok(Backend::Qemu)
}

/// The guest CID of a QEMU workstation VM: its sequence number above [`CID_BASE`].
pub fn cid_for(id: &str) -> u32 {
    u32::try_from(seq_of(id))
        .unwrap_or(u32::MAX - CID_BASE)
        .saturating_add(CID_BASE)
}

/// The record a workstation VM is kept as on disk. It never holds the join code, which the relay serves once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub name: String,
    pub os: Os,
    pub cpus: u8,
    pub ram_mb: u32,
    pub disk_gb: u32,
    pub gpu: Gpu,
    /// The base image the disk was copied from, when it is not the node's desktop image.
    pub image: Option<PathBuf>,
    /// The hub the daemon joins; a later boot needs it too (it is not a secret).
    #[serde(default)]
    pub hub: Option<String>,
    pub created_ms: u64,
    /// Whether a snapshot of the disk exists.
    #[serde(default)]
    pub snapshot: bool,
    /// The launch backend the VM runs under. A record from before QEMU is a plaza VM.
    #[serde(default)]
    pub backend: Backend,
    /// The guest CID of this VM's vsock device (see [`cid_for`]), on either backend. A record from before the relay has 0
    /// until the node loads it, which assigns its CID.
    #[serde(default)]
    pub cid: u32,
}

/// The node's desktop guest: the kernel and initramfs every Linux workstation VM boots, the disk image each VM
/// copies, where the VMs live, and the sysfs root the GPU checks read.
#[derive(Debug, Clone)]
pub struct Guest {
    pub kernel: Option<PathBuf>,
    pub initrd: Option<PathBuf>,
    pub image: Option<PathBuf>,
    pub dir: PathBuf,
    pub sysfs: PathBuf,
    /// This node's QEMU, when it has one (see `ws_qemu::detect`). `None` leaves only the plaza backend.
    pub qemu: Option<Available>,
    /// Where a plaza VM's vsock sockets are (`--uds-dir`): its relay listens there, as `vm{cid}_{port}`.
    pub vsock_dir: PathBuf,
}

impl Default for Guest {
    fn default() -> Self {
        Self {
            kernel: None,
            initrd: None,
            image: None,
            dir: apz_os::dirs::state().join("workstations"),
            sysfs: PathBuf::from("/sys"),
            qemu: None,
            vsock_dir: apz_os::dirs::vsock_sockets(),
        }
    }
}

/// The kernel command line of a workstation guest, on either backend. It carries only what is not secret: the workstation's
/// name and, for a VM with a hub, the node address its join config is read from. The join code and the hub's address are
/// never on it (`/proc/cmdline` is readable in the guest).
pub fn command_line(record: &Record) -> String {
    let mut cmdline = format!(
        "console=ttyS0 plaza.workstation=1 plaza.workstation_name={}",
        record.name
    );
    if record.hub.is_some() {
        cmdline.push_str(" plaza.workstation_node=");
        cmdline.push_str(&config_address());
    }
    cmdline
}

/// The join config the node serves a new VM once: the link address the VM dials, the hub's pin and the join code. The same
/// config on either backend. `None` for a VM with no hub, which has no relay.
pub fn join_config(shape: &Shape) -> Result<Option<Vec<u8>>, String> {
    let (Some(_), Some(pin)) = (&shape.hub, &shape.pin) else {
        return Ok(None);
    };
    let config = NodeConfig {
        hub: link_address(),
        pin: pin.clone(),
        code: shape.join_code.clone(),
    };
    serde_json::to_vec(&config)
        .map(Some)
        .map_err(|err| format!("the join config: {err}"))
}

/// The VMM spec for one plaza workstation VM: the guest's kernel and initramfs, the record's vCPUs and memory, its disk as
/// the writable block device, and, for a VM with a hub, its vsock device. The guest has no NIC (rule 0010): its daemon
/// reaches the hub only through this node's relay, which is why a hub means a vsock device. The device's muxer sockets
/// are `{vsock_dir}/vm{cid}_{port}`, which the relay listens on. No balloon.
pub fn boot_spec(record: &Record, guest: &Guest, disk: &Path) -> Result<VmSpec, String> {
    let kernel = guest.kernel.clone().ok_or_else(|| NO_KERNEL.to_string())?;
    let initrd = guest.initrd.clone().ok_or_else(|| NO_KERNEL.to_string())?;
    let vsock = vsock_of(record, guest);
    Ok(VmSpec {
        kernel,
        cmdline: command_line(record),
        vcpus: record.cpus,
        memory_mib: usize::try_from(record.ram_mb)
            .map_err(|_| "ram_mb is too large for this host".to_string())?,
        initrd: Some(initrd),
        gic: None,
        vsock,
        workspace: Some(disk.to_path_buf()),
        balloon: None,
    })
}

/// The vsock device a plaza VM gets: its CID, and the muxer's socket base under the node's vsock directory.
fn vsock_of(record: &Record, guest: &Guest) -> Option<VsockConfig> {
    record.hub.as_ref().map(|_| VsockConfig {
        guest_cid: u64::from(record.cid),
        uds_path: guest.vsock_dir.join(format!("vm{}", record.cid)),
    })
}

/// The QEMU argv of one workstation VM. A Windows guest boots its own disk with UEFI, from its variable store. A Linux guest
/// boots the node's desktop kernel directly, as plaza does, from the overlay. Either reaches its hub over its vsock, with
/// the same command line and join config as a plaza VM. The disk and state live in `dir`.
fn qemu_argv(
    record: &Record,
    guest: &Guest,
    available: &Available,
    dir: &Path,
) -> Result<Vec<String>, String> {
    let disk = dir.join(QEMU_DISK);
    let vars = dir.join(QEMU_VARS);
    let cmdline = command_line(record);
    let windows = record.os == Os::Windows;
    let boot = if windows {
        let firmware = available
            .firmware
            .as_ref()
            .ok_or_else(|| "this node has no UEFI firmware for a Windows guest".to_string())?;
        ws_qemu::Boot::Uefi {
            code: &firmware.code,
            vars: &vars,
        }
    } else {
        let kernel = guest
            .kernel
            .as_deref()
            .ok_or_else(|| NO_KERNEL.to_string())?;
        let initrd = guest
            .initrd
            .as_deref()
            .ok_or_else(|| NO_KERNEL.to_string())?;
        ws_qemu::Boot::Direct {
            kernel,
            initrd,
            cmdline: &cmdline,
        }
    };
    let vfio = match &record.gpu {
        Gpu::Vfio(pci) => Some(pci.as_str()),
        Gpu::None | Gpu::Virtual => None,
    };
    ws_qemu::argv(&ws_qemu::Plan {
        name: &record.name,
        arch: available.arch,
        cpus: record.cpus,
        ram_mb: record.ram_mb,
        windows,
        boot,
        disk: &disk,
        dir,
        cid: vsock_cid(record),
        vfio,
    })
}

/// The vsock CID a QEMU VM's device is given: its own, for a VM with a hub (the relay reaches it there), else none.
fn vsock_cid(record: &Record) -> Option<u32> {
    record.hub.as_ref().map(|_| record.cid)
}

/// A workstation VM as the API shows it.
#[derive(Debug, Clone, Serialize)]
pub struct View {
    pub id: String,
    pub name: String,
    pub os: Os,
    pub cpus: u8,
    pub ram_mb: u32,
    pub disk_gb: u32,
    pub gpu: String,
    /// `running` while its VMM process lives, else `stopped`.
    pub state: &'static str,
    pub pid: Option<u32>,
    pub snapshot: bool,
    pub created_ms: u64,
    pub backend: Backend,
}

/// A running workstation VM: a plaza VMM, a QEMU process this node started, or a QEMU process a previous noded started and
/// this one took back ([`ws_qemu::reattach`]).
enum Handle {
    Plaza(VmHandle),
    Qemu(ws_qemu::Process),
    Attached(ws_qemu::Attached),
}

impl Handle {
    fn pid(&self) -> Option<u32> {
        match self {
            Self::Plaza(handle) => handle.process_id(),
            Self::Qemu(process) => Some(process.pid()),
            Self::Attached(attached) => Some(attached.pid()),
        }
    }

    /// Whether the VMM has exited.
    fn exited(&mut self) -> bool {
        match self {
            Self::Plaza(handle) => {
                !matches!(handle.wait_timeout(Duration::from_millis(1)), Ok(None))
            }
            Self::Qemu(process) => process.exited(),
            Self::Attached(attached) => attached.exited(),
        }
    }

    /// Stops the VMM: a plaza VM is killed; a QEMU VM is powered down, then quit, then killed. An attached QEMU VM is
    /// powered down and quit through QMP only, since it is not this process's child.
    fn halt(&mut self) -> Result<(), String> {
        match self {
            Self::Plaza(handle) => handle.kill().map_err(|err| err.to_string()),
            Self::Qemu(process) => process.shut_down(ws_qemu::POWERDOWN_GRACE),
            Self::Attached(attached) => attached.shut_down(ws_qemu::POWERDOWN_GRACE),
        }
    }
}

/// A running VM's hub link: a plaza VM's unix-socket relay, or a QEMU VM's route on the node's AF_VSOCK relay. It is held
/// while the VM runs, and dropping it ends the link. Nothing reads its fields: they are held for their `Drop`.
#[allow(dead_code)]
enum Link {
    Plaza(Relay),
    Qemu(ws_relay::Attachment),
}

struct Slot {
    record: Record,
    handle: Option<Handle>,
    link: Option<Link>,
}

#[derive(Default)]
struct Inner {
    next: u64,
    slots: BTreeMap<String, Slot>,
}

/// The node's workstation VMs.
pub struct Store {
    guest: Guest,
    inner: Mutex<Inner>,
    /// The node's AF_VSOCK relay for its QEMU VMs, shared by all of them.
    host: ws_relay::Host,
}

/// Opens a VM's hub link: a plaza VM's unix-socket relay under `vsock_dir`, or a QEMU VM's route on `host`. A VM with no
/// hub has no link. `config` is the join config the link serves once (`None` when the VM is already enrolled).
fn open_link(
    host: &ws_relay::Host,
    vsock_dir: &Path,
    record: &Record,
    config: Option<Vec<u8>>,
) -> Result<Option<Link>, String> {
    let Some(hub) = &record.hub else {
        return Ok(None);
    };
    let destination = Destination::of_hub(hub)?;
    let link = match record.backend {
        Backend::Plaza => Relay::start(vsock_dir, record.cid, destination, config)
            .map(Link::Plaza)
            .map_err(|err| format!("the workstation's relay did not start: {err}"))?,
        Backend::Qemu => host
            .register(record.cid, destination, config)
            .map(Link::Qemu)
            .map_err(|err| format!("the workstation's relay did not start: {err}"))?,
    };
    Ok(Some(link))
}

impl Store {
    /// Opens the store over `guest.dir`, loading every record found there as a stopped VM, except a QEMU VM that a previous
    /// noded left running, which is taken back as running with its relay restarted. A record with no CID is given one. A
    /// directory that cannot be read starts the store empty, and a record that cannot be read is skipped, both with a line
    /// on stderr.
    pub fn open(guest: Guest) -> Self {
        let mut inner = Inner::default();
        let host = ws_relay::Host::new();
        match fs::read_dir(&guest.dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let dir = entry.path();
                    let path = dir.join("vm.json");
                    let Ok(text) = fs::read(&path) else { continue };
                    match serde_json::from_slice::<Record>(&text) {
                        Ok(mut record) => {
                            inner.next = inner.next.max(seq_of(&record.id) + 1);
                            if record.cid == 0 {
                                record.cid = cid_for(&record.id);
                                if let Err(err) = write_record(&dir, &record) {
                                    eprintln!("noded: workstation {}: {err}", record.id);
                                }
                            }
                            let mut slot = Slot {
                                record,
                                handle: None,
                                link: None,
                            };
                            if slot.record.backend == Backend::Qemu
                                && let Some(attached) = ws_qemu::reattach(&dir)
                            {
                                match open_link(&host, &guest.vsock_dir, &slot.record, None) {
                                    Ok(link) => slot.link = link,
                                    Err(err) => eprintln!(
                                        "noded: workstation {} is running, but its relay did not restart: {err}",
                                        slot.record.id
                                    ),
                                }
                                slot.handle = Some(Handle::Attached(attached));
                            }
                            inner.slots.insert(slot.record.id.clone(), slot);
                        }
                        Err(err) => eprintln!(
                            "noded: workstation record {} is not readable: {err}",
                            path.display()
                        ),
                    }
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => eprintln!(
                "noded: cannot read the workstations directory {}: {err}",
                guest.dir.display()
            ),
        }
        Self {
            guest,
            inner: Mutex::new(inner),
            host,
        }
    }

    /// Every workstation VM, by id.
    pub fn list(&self) -> Vec<View> {
        let mut inner = self.lock();
        inner.slots.values_mut().map(view_of).collect()
    }

    /// One workstation VM, or a 404.
    pub fn view(&self, id: &str) -> Result<View, Refusal> {
        let mut inner = self.lock();
        inner
            .slots
            .get_mut(id)
            .map(view_of)
            .ok_or_else(|| not_found(id))
    }

    /// Makes a workstation VM from `request` and boots it, its join config served once by its relay. Every check that needs no
    /// device or file runs before the directory is made, so a refused request leaves nothing behind.
    pub fn create(&self, request: &CreateWorkstation, now: u64) -> Result<View, Refusal> {
        let shape = request.checked().map_err(bad)?;
        if let Gpu::Vfio(pci) = &shape.gpu {
            check_vfio(&self.guest.sysfs, pci).map_err(bad)?;
        }
        let backend = choose(
            shape.backend,
            shape.os,
            &shape.gpu,
            self.guest.qemu.as_ref(),
        )?;
        if backend == Backend::Plaza
            && let Some(refusal) = boot_refusal(shape.os, &shape.gpu)
        {
            return Err((501, refusal));
        }
        let base = shape
            .image
            .clone()
            .or_else(|| self.guest.image.clone())
            .ok_or((503, NO_IMAGE.to_string()))?;
        let mut inner = self.lock();
        inner.next += 1;
        let id = format!("wv{}", inner.next);
        let record = Record {
            id: id.clone(),
            name: shape.name.clone(),
            os: shape.os,
            cpus: shape.cpus,
            ram_mb: shape.ram_mb,
            disk_gb: shape.disk_gb,
            gpu: shape.gpu.clone(),
            image: shape.image.clone(),
            hub: shape.hub.clone(),
            created_ms: now,
            snapshot: false,
            backend,
            cid: cid_for(&id),
        };
        let dir = self.guest.dir.join(&id);
        // The launch is checked before anything is copied: a missing kernel is refused with no directory made.
        self.check_launch(&record, &dir)?;
        // The relay listens before the VM boots, so the guest's first connection finds it. The join config is served once,
        // on either backend, and the join code is held by the relay until then.
        let config = join_config(&shape).map_err(|err| (500, err))?;
        let link = open_link(&self.host, &self.guest.vsock_dir, &record, config)
            .map_err(|err| (500, err))?;
        let handle = match self.provision(&record, &base, &dir) {
            Ok(handle) => handle,
            Err(refusal) => {
                let _ = fs::remove_dir_all(&dir);
                return Err(refusal);
            }
        };
        inner.slots.insert(
            id.clone(),
            Slot {
                record,
                handle: Some(handle),
                link,
            },
        );
        inner
            .slots
            .get_mut(&id)
            .map(view_of)
            .ok_or_else(|| not_found(&id))
    }

    /// Boots a stopped VM from its disk, with its hub link. No join config: the daemon on its disk is already enrolled.
    pub fn start(&self, id: &str) -> Result<View, Refusal> {
        let mut inner = self.lock();
        let slot = inner.slots.get_mut(id).ok_or_else(|| not_found(id))?;
        refresh(slot);
        if slot.handle.is_some() {
            return Err((409, format!("workstation {id} is already running")));
        }
        let dir = self.dir_of(id);
        // The link is opened before the VM boots, and dropped with a VM that does not.
        let (handle, link) = match slot.record.backend {
            Backend::Plaza => {
                let link = open_link(&self.host, &self.guest.vsock_dir, &slot.record, None)
                    .map_err(|e| (500, e))?;
                let spec = boot_spec(&slot.record, &self.guest, &dir.join(PLAZA_DISK))
                    .map_err(|e| (503, e))?;
                let handle = spawner::spawn(spec)
                    .map(Handle::Plaza)
                    .map_err(|err| (500, format!("the workstation VM did not boot: {err}")))?;
                (handle, link)
            }
            Backend::Qemu => {
                let available = self.qemu()?;
                if let Some(pid) = ws_qemu::running_pid(&dir) {
                    return Err((
                        409,
                        format!(
                            "workstation {id} is still running as pid {pid}, from before this node restarted"
                        ),
                    ));
                }
                let link = open_link(&self.host, &self.guest.vsock_dir, &slot.record, None)
                    .map_err(|e| (500, e))?;
                let argv =
                    qemu_argv(&slot.record, &self.guest, available, &dir).map_err(|e| (503, e))?;
                let mut process = start_qemu(available, &argv, &dir)?;
                if !process.wait_ready(ws_qemu::READY_WAIT) {
                    let _ = process.shut_down(Duration::ZERO);
                    return Err((500, "the workstation VM did not boot".into()));
                }
                (Handle::Qemu(process), link)
            }
        };
        slot.handle = Some(handle);
        slot.link = link;
        Ok(view_of(slot))
    }

    /// Stops a VM. A plaza VM's VMM is killed. A QEMU VM is powered down, given its grace, then quit, then killed.
    pub fn stop(&self, id: &str) -> Result<View, Refusal> {
        let mut inner = self.lock();
        let slot = inner.slots.get_mut(id).ok_or_else(|| not_found(id))?;
        refresh(slot);
        if let Some(mut handle) = slot.handle.take()
            && let Err(err) = handle.halt()
        {
            slot.handle = Some(handle);
            return Err((500, format!("workstation {id} did not stop: {err}")));
        }
        slot.link = None;
        Ok(view_of(slot))
    }

    /// Copies a stopped VM's disk to its snapshot file. The VM must be stopped: the copy is of a quiet disk.
    pub fn snapshot(&self, id: &str) -> Result<View, Refusal> {
        let mut inner = self.lock();
        let slot = inner.slots.get_mut(id).ok_or_else(|| not_found(id))?;
        refresh(slot);
        if slot.handle.is_some() {
            return Err((
                409,
                format!("workstation {id} must be stopped to snapshot its disk"),
            ));
        }
        let dir = self.dir_of(id);
        match slot.record.backend {
            Backend::Plaza => replace_file(
                &dir.join(PLAZA_DISK),
                &dir.join("snapshot.tmp"),
                &dir.join(PLAZA_SNAPSHOT),
            )
            .map_err(|e| (500, e))?,
            Backend::Qemu => {
                let available = self.qemu()?;
                ws_qemu::snapshot(
                    &available.img,
                    &dir.join(QEMU_DISK),
                    &dir.join(QEMU_SNAPSHOT),
                )
                .map_err(|e| (500, e))?;
            }
        }
        slot.record.snapshot = true;
        write_record(&dir, &slot.record).map_err(|e| (500, e))?;
        Ok(view_of(slot))
    }

    /// Copies a stopped VM's snapshot back over its disk: the disk is what it was when the snapshot was taken.
    pub fn revert(&self, id: &str) -> Result<View, Refusal> {
        let mut inner = self.lock();
        let slot = inner.slots.get_mut(id).ok_or_else(|| not_found(id))?;
        refresh(slot);
        if slot.handle.is_some() {
            return Err((
                409,
                format!("workstation {id} must be stopped to revert its disk"),
            ));
        }
        let dir = self.dir_of(id);
        let snapshot = match slot.record.backend {
            Backend::Plaza => dir.join(PLAZA_SNAPSHOT),
            Backend::Qemu => dir.join(QEMU_SNAPSHOT),
        };
        if !snapshot.is_file() {
            return Err((
                409,
                format!("workstation {id} has no snapshot to revert to"),
            ));
        }
        match slot.record.backend {
            Backend::Plaza => replace_file(&snapshot, &dir.join("disk.tmp"), &dir.join(PLAZA_DISK))
                .map_err(|e| (500, e))?,
            Backend::Qemu => {
                let available = self.qemu()?;
                ws_qemu::revert(&available.img, &dir.join(QEMU_DISK), &snapshot)
                    .map_err(|e| (500, e))?;
            }
        }
        Ok(view_of(slot))
    }

    /// Stops a VM and removes its directory: its disk and its snapshot go with it.
    pub fn delete(&self, id: &str) -> Result<View, Refusal> {
        let mut inner = self.lock();
        let mut slot = inner.slots.remove(id).ok_or_else(|| not_found(id))?;
        if let Some(mut handle) = slot.handle.take()
            && let Err(err) = handle.halt()
        {
            slot.handle = Some(handle);
            let message = format!("workstation {id} did not stop, so it was not removed: {err}");
            inner.slots.insert(id.to_string(), slot);
            return Err((500, message));
        }
        slot.link = None;
        let view = view_of(&mut slot);
        let dir = self.dir_of(id);
        if let Err(err) = fs::remove_dir_all(&dir)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            inner.slots.insert(id.to_string(), slot);
            return Err((500, format!("cannot remove {}: {err}", dir.display())));
        }
        Ok(View {
            state: "stopped",
            pid: None,
            ..view
        })
    }

    /// The backends this node can launch a workstation VM under, for its facts.
    pub fn backends(&self) -> Vec<&'static str> {
        let mut backends = vec!["plaza"];
        if self.guest.qemu.is_some() {
            backends.push("qemu");
        }
        backends
    }

    fn qemu(&self) -> Result<&Available, Refusal> {
        self.guest.qemu.as_ref().ok_or((503, NO_QEMU.to_string()))
    }

    /// Checks that a VM of this record can launch: its backend's launch builds. Writes nothing.
    fn check_launch(&self, record: &Record, dir: &Path) -> Result<(), Refusal> {
        match record.backend {
            Backend::Plaza => boot_spec(record, &self.guest, &dir.join(PLAZA_DISK))
                .map(drop)
                .map_err(|e| (503, e)),
            Backend::Qemu => {
                let available = self.qemu()?;
                qemu_argv(record, &self.guest, available, dir)
                    .map(drop)
                    .map_err(|e| (503, e))
            }
        }
    }

    /// Makes a new VM's disk and launches it once. The caller removes the directory when this refuses.
    fn provision(&self, record: &Record, base: &Path, dir: &Path) -> Result<Handle, Refusal> {
        match record.backend {
            Backend::Plaza => {
                let disk = dir.join(PLAZA_DISK);
                if record.os == Os::Windows && !base.is_file() {
                    return Err(bad(format!(
                        "the Windows image {} is not a file on this node",
                        base.display()
                    )));
                }
                self.make_disk(base, dir, &disk, record.disk_gb)
                    .map_err(|e| (500, e))?;
                write_record(dir, record).map_err(|e| (500, e))?;
                let spec = boot_spec(record, &self.guest, &disk).map_err(|e| (503, e))?;
                spawner::spawn(spec)
                    .map(Handle::Plaza)
                    .map_err(|err| (500, format!("the workstation VM did not boot: {err}")))
            }
            Backend::Qemu => {
                let available = self.qemu()?;
                if !base.is_file() {
                    return Err((
                        503,
                        format!(
                            "the disk image {} is not a file on this node",
                            base.display()
                        ),
                    ));
                }
                let format = ws_qemu::base_format(base).map_err(|e| (400, e))?;
                private_dir(dir)
                    .map_err(|e| (500, format!("cannot make {}: {e}", dir.display())))?;
                let overlay_args =
                    ws_qemu::overlay_args(&dir.join(QEMU_DISK), base, format, Some(record.disk_gb));
                ws_qemu::run_qemu_img(&available.img, &overlay_args).map_err(|e| (500, e))?;
                if record.os == Os::Windows {
                    let firmware = available
                        .firmware
                        .as_ref()
                        .ok_or((501, NO_UEFI.to_string()))?;
                    fs::copy(&firmware.vars, dir.join(QEMU_VARS))
                        .map_err(|e| (500, format!("cannot copy the UEFI variables: {e}")))?;
                }
                write_record(dir, record).map_err(|e| (500, e))?;
                let argv = qemu_argv(record, &self.guest, available, dir).map_err(|e| (503, e))?;
                let mut process = start_qemu(available, &argv, dir)?;
                if !process.wait_ready(ws_qemu::READY_WAIT) {
                    let _ = process.shut_down(Duration::ZERO);
                    return Err((500, "the workstation VM did not boot".into()));
                }
                Ok(Handle::Qemu(process))
            }
        }
    }

    fn make_disk(&self, base: &Path, dir: &Path, disk: &Path, disk_gb: u32) -> Result<(), String> {
        private_dir(dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
        let want = u64::from(disk_gb) * GIB;
        let base_len = fs::metadata(base)
            .map_err(|e| format!("cannot read the image {}: {e}", base.display()))?
            .len();
        if base_len > want {
            return Err(format!(
                "disk_gb {disk_gb} is smaller than the image ({} GiB)",
                base_len.div_ceil(GIB)
            ));
        }
        spawner::copy_sparse(base, disk).map_err(|e| format!("cannot copy the image: {e}"))?;
        let file = fs::OpenOptions::new()
            .write(true)
            .open(disk)
            .map_err(|e| format!("cannot open the disk: {e}"))?;
        file.set_len(want)
            .map_err(|e| format!("cannot grow the disk: {e}"))
    }

    fn dir_of(&self, id: &str) -> PathBuf {
        self.guest.dir.join(id)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn bad(message: impl ToString) -> Refusal {
    (400, message.to_string())
}

fn not_found(id: &str) -> Refusal {
    (404, format!("no workstation VM {id}"))
}

/// The sequence number of an id made by [`Store::create`] (`wv<n>`), or 0 for any other id.
fn seq_of(id: &str) -> u64 {
    id.strip_prefix("wv")
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// Starts QEMU for a VM, its stderr appended to the VM's log. A socket a previous run left behind is removed first.
fn start_qemu(
    available: &Available,
    argv: &[String],
    dir: &Path,
) -> Result<ws_qemu::Process, Refusal> {
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(ws_qemu::LOG))
        .map_err(|e| (500, format!("cannot open the VM's log: {e}")))?;
    let socket = dir.join(ws_qemu::QMP_SOCKET);
    let _ = fs::remove_file(&socket);
    ws_qemu::Process::start(&available.program, argv, log, socket)
        .map_err(|err| (500, format!("the workstation VM did not boot: {err}")))
}

/// Notices an exited VMM: its handle is dropped, so the VM reads as stopped.
fn refresh(slot: &mut Slot) {
    if let Some(handle) = slot.handle.as_mut()
        && handle.exited()
    {
        slot.handle = None;
        slot.link = None;
    }
}

fn view_of(slot: &mut Slot) -> View {
    refresh(slot);
    let record = &slot.record;
    View {
        id: record.id.clone(),
        name: record.name.clone(),
        os: record.os,
        cpus: record.cpus,
        ram_mb: record.ram_mb,
        disk_gb: record.disk_gb,
        gpu: record.gpu.text(),
        state: if slot.handle.is_some() {
            "running"
        } else {
            "stopped"
        },
        pid: slot.handle.as_ref().and_then(Handle::pid),
        snapshot: record.snapshot,
        created_ms: record.created_ms,
        backend: record.backend,
    }
}

/// Copies `from` to `tmp` and renames it over `to`, so a copy that fails leaves `to` as it was.
fn replace_file(from: &Path, tmp: &Path, to: &Path) -> Result<(), String> {
    spawner::copy_sparse(from, tmp).map_err(|e| format!("cannot copy {}: {e}", from.display()))?;
    fs::rename(tmp, to).map_err(|e| {
        let _ = fs::remove_file(tmp);
        format!("cannot replace {}: {e}", to.display())
    })
}

fn write_record(dir: &Path, record: &Record) -> Result<(), String> {
    let text = serde_json::to_vec_pretty(record).map_err(|e| e.to_string())?;
    fs::write(dir.join("vm.json"), text).map_err(|e| format!("cannot write the record: {e}"))
}

fn private_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch(name: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "noded-workstation-{}-{}-{name}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// An absolute image path on this node. The check asks the host what "absolute" means, so a Windows node takes a
    /// drive path.
    fn node_image() -> &'static str {
        if cfg!(windows) {
            r"C:\srv\win.img"
        } else {
            "/srv/win.img"
        }
    }

    fn request(os: Os) -> CreateWorkstation {
        CreateWorkstation {
            name: "desk-1".into(),
            os,
            cpus: 4,
            ram_mb: 8192,
            disk_gb: 40,
            gpu: Gpu::None,
            image: if os == Os::Windows {
                Some(node_image().into())
            } else {
                None
            },
            hub: Some("https://hub.example:7443/".into()),
            pin: Some("cGlu".into()),
            join_code: "abc.def-ghi_123".into(),
            backend: None,
        }
    }

    fn record(id: &str) -> Record {
        Record {
            id: id.into(),
            name: "desk-1".into(),
            os: Os::Linux,
            cpus: 2,
            ram_mb: 4096,
            disk_gb: 8,
            gpu: Gpu::None,
            image: None,
            hub: Some("https://hub.example:7443".into()),
            created_ms: 1,
            snapshot: false,
            backend: Backend::Plaza,
            cid: cid_for(id),
        }
    }

    #[test]
    fn a_linux_request_is_accepted_and_its_bounds_are_enforced() {
        let shape = request(Os::Linux).checked().expect("a plain Linux desktop");
        assert_eq!(
            (shape.cpus, shape.ram_mb, shape.disk_gb, shape.gpu),
            (4, 8192, 40, Gpu::None)
        );

        let mut bad_name = request(Os::Linux);
        for name in [
            "",
            "-desk",
            "desk-",
            "Desk",
            "a b",
            &"x".repeat(MAX_NAME_LEN + 1),
        ] {
            bad_name.name = name.into();
            assert!(bad_name.checked().is_err(), "name {name:?} must be refused");
        }
        let mut cpus_zero = request(Os::Linux);
        cpus_zero.cpus = 0;
        assert!(cpus_zero.checked().unwrap_err().contains("cpus"));
        let mut cpus_many = request(Os::Linux);
        cpus_many.cpus = MAX_CPUS + 1;
        assert!(cpus_many.checked().unwrap_err().contains("cpus"));
        let mut ram_small = request(Os::Linux);
        ram_small.ram_mb = MIN_RAM_MB - 1;
        assert!(ram_small.checked().unwrap_err().contains("ram_mb"));
        let mut ram_big = request(Os::Linux);
        ram_big.ram_mb = MAX_RAM_MB + 1;
        assert!(ram_big.checked().unwrap_err().contains("ram_mb"));
        let mut disk_small = request(Os::Linux);
        disk_small.disk_gb = MIN_DISK_GB - 1;
        assert!(disk_small.checked().unwrap_err().contains("disk_gb"));
        let mut disk_big = request(Os::Linux);
        disk_big.disk_gb = MAX_DISK_GB + 1;
        assert!(disk_big.checked().unwrap_err().contains("disk_gb"));
    }

    #[test]
    fn a_windows_vm_needs_an_image_and_more_memory_and_a_macos_vm_is_refused() {
        let mut windows = request(Os::Windows);
        windows.image = None;
        assert!(windows.checked().unwrap_err().contains("image"));

        let mut small = request(Os::Windows);
        small.ram_mb = MIN_WINDOWS_RAM_MB - 1;
        assert!(small.checked().unwrap_err().contains("ram_mb"));

        let mut relative = request(Os::Windows);
        relative.image = Some("win.img".into());
        assert!(relative.checked().unwrap_err().contains("absolute"));

        assert!(request(Os::Windows).checked().is_ok());
        let err = request(Os::Macos).checked().unwrap_err();
        assert!(err.contains("macOS"), "{err}");
    }

    #[test]
    fn a_join_code_with_whitespace_or_no_text_is_refused() {
        let mut r = request(Os::Linux);
        r.join_code = String::new();
        assert!(r.checked().is_err());
        r.join_code = "two words".into();
        assert!(r.checked().unwrap_err().contains("whitespace"));
        r.join_code = "x".repeat(MAX_JOIN_CODE + 1);
        assert!(r.checked().unwrap_err().contains("longer"));
    }

    #[test]
    fn gpu_options_read_as_none_virtual_or_a_pci_address_in_lower_case() {
        assert_eq!(Gpu::parse("none"), Ok(Gpu::None));
        assert_eq!(Gpu::parse("virtual"), Ok(Gpu::Virtual));
        assert_eq!(
            Gpu::parse("vfio:0000:0A:00.0"),
            Ok(Gpu::Vfio("0000:0a:00.0".into()))
        );
        assert_eq!(Gpu::Vfio("0000:0a:00.0".into()).text(), "vfio:0000:0a:00.0");
        for bad in [
            "",
            "cuda",
            "vfio:",
            "vfio:01:00.0",
            "vfio:0000:01:00.8",
            "vfio:0000-01-00.0",
            "vfio:zzzz:01:00.0",
        ] {
            assert!(Gpu::parse(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn the_gpu_option_round_trips_through_its_text_in_json() {
        let parsed: CreateWorkstation = serde_json::from_value(serde_json::json!({
            "name": "desk", "os": "linux", "cpus": 2, "ram_mb": 4096, "disk_gb": 16,
            "gpu": "vfio:0000:01:00.0", "join_code": "j"
        }))
        .expect("a request with a vfio GPU");
        assert_eq!(parsed.gpu, Gpu::Vfio("0000:01:00.0".into()));
        let bad = serde_json::from_value::<CreateWorkstation>(serde_json::json!({
            "name": "desk", "os": "linux", "cpus": 2, "ram_mb": 4096, "disk_gb": 16,
            "gpu": "gpu0", "join_code": "j"
        }));
        assert!(bad.is_err());
    }

    #[cfg(unix)]
    fn pci_device(sysfs: &Path, pci: &str) -> PathBuf {
        let device = sysfs.join("bus/pci/devices").join(pci);
        fs::create_dir_all(&device).expect("a device directory");
        device
    }

    #[cfg(unix)]
    #[test]
    fn a_vfio_device_is_checked_against_sysfs_and_never_rebound() {
        let sysfs = scratch("sysfs");
        let pci = "0000:01:00.0";
        assert!(
            check_vfio(&sysfs, pci)
                .unwrap_err()
                .contains("no PCI device")
        );

        let device = pci_device(&sysfs, pci);
        let err = check_vfio(&sysfs, pci).unwrap_err();
        assert!(
            err.contains("no driver") && err.contains("does not rebind"),
            "{err}"
        );

        let drivers = sysfs.join("bus/pci/drivers");
        fs::create_dir_all(drivers.join("nvidia")).unwrap();
        std::os::unix::fs::symlink("../../../../bus/pci/drivers/nvidia", device.join("driver"))
            .unwrap();
        let err = check_vfio(&sysfs, pci).unwrap_err();
        assert!(
            err.contains("bound to nvidia") && err.contains("not vfio-pci"),
            "{err}"
        );

        fs::remove_file(device.join("driver")).unwrap();
        fs::create_dir_all(drivers.join("vfio-pci")).unwrap();
        std::os::unix::fs::symlink(
            "../../../../bus/pci/drivers/vfio-pci",
            device.join("driver"),
        )
        .unwrap();
        let err = check_vfio(&sysfs, pci).unwrap_err();
        assert!(err.contains("IOMMU"), "{err}");

        fs::create_dir_all(sysfs.join("kernel/iommu_groups/14")).unwrap();
        std::os::unix::fs::symlink(
            "../../../../kernel/iommu_groups/14",
            device.join("iommu_group"),
        )
        .unwrap();
        assert_eq!(check_vfio(&sysfs, pci), Ok(()));
        let _ = fs::remove_dir_all(&sysfs);
    }

    #[test]
    fn a_windows_guest_and_a_vfio_gpu_are_refused_before_anything_boots() {
        assert!(
            boot_refusal(Os::Windows, &Gpu::None)
                .unwrap()
                .contains("Windows")
        );
        assert!(
            boot_refusal(Os::Linux, &Gpu::Vfio("0000:01:00.0".into()))
                .unwrap()
                .contains("vfio")
        );
        assert_eq!(boot_refusal(Os::Linux, &Gpu::None), None);
        assert_eq!(boot_refusal(Os::Linux, &Gpu::Virtual), None);
    }

    #[test]
    fn the_boot_spec_names_its_node_and_its_vsock_and_never_the_join_code_or_the_hub() {
        let guest = Guest {
            kernel: Some(PathBuf::from("/g/vmlinux")),
            initrd: Some(PathBuf::from("/g/initrd.gz")),
            vsock_dir: PathBuf::from("/run/vsock"),
            ..Guest::default()
        };
        let disk = Path::new("/d/disk.img");
        let spec = boot_spec(&record("wv1"), &guest, disk).expect("a spec");
        assert!(
            spec.cmdline
                .contains("plaza.workstation=1 plaza.workstation_name=desk-1"),
            "{}",
            spec.cmdline
        );
        assert!(
            spec.cmdline
                .ends_with("plaza.workstation_node=vsock://2:5101"),
            "{}",
            spec.cmdline
        );
        assert!(
            !spec.cmdline.contains("join")
                && !spec.cmdline.contains("hub.example")
                && !spec.cmdline.contains("CODE"),
            "{}",
            spec.cmdline
        );
        let vsock = spec.vsock.expect("a hub means a vsock device");
        assert_eq!(vsock.guest_cid, u64::from(cid_for("wv1")));
        assert_eq!(vsock.uds_path, PathBuf::from("/run/vsock/vm4097"));
        assert!(spec.balloon.is_none());
        assert_eq!(spec.workspace.as_deref(), Some(disk));
        assert_eq!((spec.vcpus, spec.memory_mib), (2, 4096));

        let mut no_hub = record("wv2");
        no_hub.hub = None;
        let spec = boot_spec(&no_hub, &guest, disk).expect("a spec");
        assert!(spec.vsock.is_none(), "no hub, no relay, no vsock device");
        assert!(!spec.cmdline.contains("plaza.workstation_node"));

        let missing = boot_spec(&record("wv1"), &Guest::default(), disk).unwrap_err();
        assert!(missing.contains("scripts/mk-workstation-guest.sh"));
    }

    #[test]
    fn the_join_config_carries_the_link_address_the_pin_and_the_code() {
        let shape = request(Os::Linux).checked().expect("a shape");
        let bytes = join_config(&shape)
            .expect("a config")
            .expect("a hub has one");
        let config: NodeConfig = serde_json::from_slice(&bytes).expect("the config");
        assert_eq!(config.hub, "vsock://2:5100");
        assert_eq!(config.pin, "cGlu");
        assert_eq!(config.code, "abc.def-ghi_123");
        assert_eq!(apz_station::vsock::LINK_PORT, 5100);
        assert_eq!(apz_station::vsock::CONFIG_PORT, 5101);

        let mut no_hub = request(Os::Linux);
        no_hub.hub = None;
        no_hub.pin = None;
        let shape = no_hub.checked().expect("a shape with no hub");
        assert_eq!(join_config(&shape).expect("no error"), None);
    }

    #[test]
    fn a_hub_needs_a_pin_and_a_pin_must_be_base64url() {
        let mut unpinned = request(Os::Linux);
        unpinned.pin = None;
        assert!(unpinned.checked().unwrap_err().contains("pin"));

        let mut spaced = request(Os::Linux);
        spaced.pin = Some("two words".into());
        assert!(spaced.checked().unwrap_err().contains("base64url"));

        let mut long = request(Os::Linux);
        long.pin = Some("x".repeat(65));
        assert!(long.checked().is_err());
    }

    #[test]
    fn both_backends_hand_the_same_join_config_by_the_same_node_address() {
        let shape = request(Os::Linux).checked().expect("a shape");
        let config = join_config(&shape)
            .expect("a config")
            .expect("a hub has one");
        let config: NodeConfig = serde_json::from_slice(&config).expect("the config");
        assert_eq!(config.code, shape.join_code);

        let guest = Guest {
            kernel: Some(PathBuf::from("/g/vmlinux")),
            initrd: Some(PathBuf::from("/g/initrd.gz")),
            vsock_dir: PathBuf::from("/run/vsock"),
            ..Guest::default()
        };
        let plaza =
            boot_spec(&record("wv1"), &guest, Path::new("/d/disk.img")).expect("a plaza spec");
        let mut qemu_record = record("wv1");
        qemu_record.backend = Backend::Qemu;
        let qemu_available = Available {
            program: PathBuf::from("/usr/bin/qemu-system-x86_64"),
            img: PathBuf::from("/usr/bin/qemu-img"),
            arch: ws_qemu::Arch::X86_64,
            firmware: None,
        };
        let qemu = qemu_argv(
            &qemu_record,
            &guest,
            &qemu_available,
            Path::new("/d/vms/wv1"),
        )
        .expect("a QEMU argv");
        let append = qemu
            .windows(2)
            .find(|w| w[0] == "-append")
            .map(|w| w[1].clone())
            .expect("a Linux guest is booted with its command line");

        for cmdline in [plaza.cmdline.as_str(), append.as_str()] {
            assert!(
                cmdline.ends_with("plaza.workstation_node=vsock://2:5101"),
                "{cmdline}"
            );
            assert!(
                !cmdline.contains(&config.code),
                "the join code is never on the command line"
            );
            assert!(
                !cmdline.contains("hub.example"),
                "the hub's address is never on the command line"
            );
        }
        assert_eq!(plaza.cmdline, append, "one command line on either backend");
        assert!(plaza.vsock.is_some(), "the plaza VM has its vsock device");
        assert!(
            qemu.windows(2)
                .any(|w| w == ["-device", "vhost-vsock-pci,guest-cid=4097"])
        );
    }

    #[test]
    fn a_disk_is_grown_to_its_size_and_never_shrunk() {
        let dir = scratch("disk");
        let base = dir.join("base.img");
        fs::write(&base, vec![7_u8; 1 << 20]).unwrap();
        let store = Store::open(Guest {
            dir: dir.join("vms"),
            ..Guest::default()
        });
        let disk = dir.join("vms/wv1/disk.img");
        store
            .make_disk(&base, &dir.join("vms/wv1"), &disk, 8)
            .expect("an 8 GiB disk");
        assert_eq!(fs::metadata(&disk).unwrap().len(), 8 * GIB);

        let big_base = dir.join("big.img");
        fs::File::create(&big_base)
            .unwrap()
            .set_len(2 * GIB)
            .unwrap();
        let err = store
            .make_disk(
                &big_base,
                &dir.join("vms/wv2"),
                &dir.join("vms/wv2/disk.img"),
                1,
            )
            .expect_err("a 1 GiB disk cannot hold a 2 GiB image");
        assert!(err.contains("smaller"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_snapshot_is_taken_of_a_stopped_disk_and_reverted_over_it() {
        let dir = scratch("snapshot");
        let vms = dir.join("vms");
        let vm = vms.join("wv1");
        fs::create_dir_all(&vm).unwrap();
        write_record(&vm, &record("wv1")).unwrap();
        fs::write(vm.join("disk.img"), b"clean").unwrap();

        let store = Store::open(Guest {
            dir: vms.clone(),
            ..Guest::default()
        });
        assert_eq!(store.view("wv1").unwrap().state, "stopped");
        assert_eq!(store.revert("wv1").unwrap_err().0, 409, "no snapshot yet");

        let taken = store.snapshot("wv1").expect("a stopped VM snapshots");
        assert!(taken.snapshot);
        assert!(
            !vm.join("snapshot.tmp").exists(),
            "the copy is renamed into place"
        );

        fs::write(vm.join("disk.img"), b"work done by an agent").unwrap();
        store.revert("wv1").expect("a revert");
        assert_eq!(fs::read(vm.join("disk.img")).unwrap(), b"clean");

        let reloaded = Store::open(Guest {
            dir: vms,
            ..Guest::default()
        });
        assert!(
            reloaded.view("wv1").unwrap().snapshot,
            "the snapshot flag survives a restart"
        );
        assert_eq!(reloaded.view("wv9").unwrap_err().0, 404);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_record_on_disk_loads_as_a_stopped_vm_and_its_id_continues_the_count() {
        let dir = scratch("load");
        let vms = dir.join("vms");
        for id in ["wv4", "wv2"] {
            let vm = vms.join(id);
            fs::create_dir_all(&vm).unwrap();
            write_record(&vm, &record(id)).unwrap();
        }
        let broken = vms.join("wv7");
        fs::create_dir_all(&broken).unwrap();
        fs::write(broken.join("vm.json"), b"{ not json").unwrap();

        let store = Store::open(Guest {
            dir: vms,
            ..Guest::default()
        });
        let ids: Vec<String> = store.list().into_iter().map(|v| v.id).collect();
        assert_eq!(ids, ["wv2", "wv4"], "the unreadable record is skipped");
        assert_eq!(
            store.inner.lock().unwrap().next,
            5,
            "the next id follows the highest one"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_create_without_a_kernel_is_refused_before_a_directory_is_made() {
        let dir = scratch("nokernel");
        let vms = dir.join("vms");
        let store = Store::open(Guest {
            dir: vms.clone(),
            image: Some(dir.join("base.img")),
            ..Guest::default()
        });
        let err = store.create(&request(Os::Linux), 1).unwrap_err();
        assert_eq!(err.0, 503);
        assert!(err.1.contains("kernel"), "{}", err.1);
        assert!(!vms.exists(), "nothing is written for a refused request");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_create_for_windows_or_vfio_is_refused_with_the_reason() {
        let dir = scratch("refuse");
        let store = Store::open(Guest {
            dir: dir.join("vms"),
            sysfs: dir.join("sysfs"),
            ..Guest::default()
        });
        // A node with no QEMU cannot take a Windows guest, and says so: nothing it has can run one.
        let windows = store.create(&request(Os::Windows), 1).unwrap_err();
        assert_eq!(windows.0, 503);
        assert!(windows.1.contains("no QEMU/OVMF"), "{}", windows.1);
        // Asking plaza for it names plaza's own refusal.
        let mut plaza_windows = request(Os::Windows);
        plaza_windows.backend = Some("plaza".into());
        let refused = store.create(&plaza_windows, 1).unwrap_err();
        assert_eq!(refused.0, 501);
        assert!(refused.1.contains("plaza"), "{}", refused.1);
        let mut vfio = request(Os::Linux);
        vfio.gpu = Gpu::Vfio("0000:01:00.0".into());
        assert_eq!(
            store.create(&vfio, 1).unwrap_err().0,
            400,
            "no such device is refused first"
        );
        assert!(!dir.join("vms").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_backend_is_chosen_per_shape_and_refused_when_the_node_has_no_qemu() {
        let qemu = Available {
            program: PathBuf::from("/usr/bin/qemu-system-x86_64"),
            img: PathBuf::from("/usr/bin/qemu-img"),
            arch: ws_qemu::Arch::X86_64,
            firmware: Some(ws_qemu::Firmware {
                code: PathBuf::from("/fw/code.fd"),
                vars: PathBuf::from("/fw/vars.fd"),
            }),
        };
        let vfio = Gpu::Vfio("0000:01:00.0".into());
        let with = Some(&qemu);
        // auto: QEMU only for what plaza cannot run.
        assert_eq!(
            choose(Choice::Auto, Os::Linux, &Gpu::None, with),
            Ok(Backend::Plaza)
        );
        assert_eq!(
            choose(Choice::Auto, Os::Windows, &Gpu::None, with),
            Ok(Backend::Qemu)
        );
        assert_eq!(
            choose(Choice::Auto, Os::Linux, &vfio, with),
            Ok(Backend::Qemu)
        );
        // plaza: never QEMU, even when asked for a Windows guest (boot_refusal names why).
        assert_eq!(
            choose(Choice::Plaza, Os::Windows, &Gpu::None, with),
            Ok(Backend::Plaza)
        );
        // qemu: a Linux guest may be asked for too.
        assert_eq!(
            choose(Choice::Qemu, Os::Linux, &Gpu::None, with),
            Ok(Backend::Qemu)
        );
        // No QEMU: auto needing it, and an explicit qemu, are refused with the install hint.
        for (choice, os, gpu) in [
            (Choice::Auto, Os::Windows, Gpu::None),
            (Choice::Auto, Os::Linux, vfio.clone()),
            (Choice::Qemu, Os::Linux, Gpu::None),
        ] {
            let (status, message) = choose(choice, os, &gpu, None).unwrap_err();
            assert_eq!(status, 503);
            assert_eq!(message, NO_QEMU);
        }
        assert_eq!(
            choose(Choice::Auto, Os::Linux, &Gpu::None, None),
            Ok(Backend::Plaza)
        );
        // An x86 firmware-less node cannot take a Windows guest under QEMU.
        let arm = Available {
            firmware: None,
            arch: ws_qemu::Arch::Aarch64,
            ..qemu.clone()
        };
        assert_eq!(
            choose(Choice::Auto, Os::Windows, &Gpu::None, Some(&arm))
                .unwrap_err()
                .0,
            501
        );
    }

    #[test]
    fn the_facts_name_qemu_only_when_the_node_has_it() {
        let dir = scratch("facts");
        let plain = Store::open(Guest {
            dir: dir.join("vms"),
            ..Guest::default()
        });
        assert_eq!(plain.backends(), ["plaza"]);
        let with = Store::open(Guest {
            dir: dir.join("vms"),
            qemu: Some(Available {
                program: PathBuf::from("/usr/bin/qemu-system-x86_64"),
                img: PathBuf::from("/usr/bin/qemu-img"),
                arch: ws_qemu::Arch::X86_64,
                firmware: None,
            }),
            ..Guest::default()
        });
        assert_eq!(with.backends(), ["plaza", "qemu"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_qemu_vm_without_a_kernel_is_refused_before_a_directory_is_made() {
        let dir = scratch("qemu-nokernel");
        let vms = dir.join("vms");
        let store = Store::open(Guest {
            dir: vms.clone(),
            image: Some(dir.join("base.img")),
            qemu: Some(Available {
                program: PathBuf::from("/usr/bin/qemu-system-x86_64"),
                img: PathBuf::from("/usr/bin/qemu-img"),
                arch: ws_qemu::Arch::X86_64,
                firmware: None,
            }),
            ..Guest::default()
        });
        let mut linux = request(Os::Linux);
        linux.backend = Some("qemu".into());
        let err = store.create(&linux, 1).unwrap_err();
        assert_eq!(err.0, 503);
        assert!(err.1.contains("kernel"), "{}", err.1);
        assert!(!vms.exists(), "nothing is written for a refused request");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_qemu_argv_boots_a_linux_guest_directly_and_a_windows_one_with_uefi() {
        let dir = scratch("qemu-argv");
        let vm = dir.join("vms/wv1");
        let mut record = record("wv1");
        record.backend = Backend::Qemu;
        record.cid = cid_for("wv1");
        record.gpu = Gpu::Vfio("0000:01:00.0".into());
        let guest = Guest {
            kernel: Some(PathBuf::from("/g/vmlinux")),
            initrd: Some(PathBuf::from("/g/initrd.gz")),
            ..Guest::default()
        };
        let linux_qemu = Available {
            program: PathBuf::from("/usr/bin/qemu-system-x86_64"),
            img: PathBuf::from("/usr/bin/qemu-img"),
            arch: ws_qemu::Arch::X86_64,
            firmware: Some(ws_qemu::Firmware {
                code: PathBuf::from("/fw/code.fd"),
                vars: PathBuf::from("/fw/vars.fd"),
            }),
        };
        let args = qemu_argv(&record, &guest, &linux_qemu, &vm).expect("a Linux VM");
        assert!(args.windows(2).any(|w| w == ["-kernel", "/g/vmlinux"]));
        assert!(
            args.windows(2)
                .any(|w| w == ["-device", "vfio-pci,host=0000:01:00.0"])
        );
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-append"
                    && w[1].ends_with("plaza.workstation_node=vsock://2:5101"))
        );
        assert!(
            args.windows(2)
                .any(|w| w == ["-device", "vhost-vsock-pci,guest-cid=4097"])
        );
        assert!(
            !args.iter().any(|a| a.contains("if=pflash")),
            "a Linux guest boots directly"
        );

        record.os = Os::Windows;
        let windows = qemu_argv(&record, &guest, &linux_qemu, &vm).expect("a Windows VM");
        assert!(
            windows
                .iter()
                .any(|a| a.contains("if=pflash,format=raw,unit=0,readonly=on,file=/fw/code.fd"))
        );
        assert!(
            !windows.iter().any(|a| a == "-fw_cfg"),
            "the join config comes over vsock, never fw_cfg"
        );
        assert!(
            windows
                .windows(2)
                .any(|w| w == ["-device", "vhost-vsock-pci,guest-cid=4097"])
        );
        assert!(!windows.iter().any(|a| a == "-kernel"));
        assert!(windows.windows(2).any(|w| w == ["-nic", "none"]));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_record_from_before_qemu_is_a_plaza_vm_with_no_cid() {
        let text = r#"{"id":"wv2","name":"desk","os":"linux","cpus":2,"ram_mb":4096,"disk_gb":16,
            "gpu":"none","image":null,"created_ms":1}"#;
        let record: Record = serde_json::from_str(text).expect("an older record");
        assert_eq!((record.backend, record.cid), (Backend::Plaza, 0));
    }

    #[test]
    fn a_vm_gets_a_cid_above_the_agent_and_run_ranges_on_either_backend() {
        assert_eq!(cid_for("wv1"), CID_BASE + 1);
        assert!(cid_for("wv1") > 100 && cid_for("wv1") > 1099);
        assert_eq!(cid_for("nonsense"), CID_BASE);
    }

    #[test]
    fn a_record_from_before_the_relay_gets_its_cid_when_the_node_loads_it() {
        let dir = scratch("old-cid");
        let vms = dir.join("vms");
        let vm = vms.join("wv3");
        fs::create_dir_all(&vm).unwrap();
        let mut old = record("wv3");
        old.cid = 0;
        write_record(&vm, &old).unwrap();

        let store = Store::open(Guest {
            dir: vms,
            ..Guest::default()
        });
        assert_eq!(
            store.inner.lock().unwrap().slots["wv3"].record.cid,
            cid_for("wv3")
        );
        let saved: Record = serde_json::from_slice(&fs::read(vm.join("vm.json")).unwrap()).unwrap();
        assert_eq!(
            saved.cid,
            cid_for("wv3"),
            "the CID is kept, so the next load keeps it"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_qemu_vm_whose_pidfile_is_not_a_qemu_loads_stopped_and_is_not_taken_back() {
        let dir = scratch("not-qemu");
        let vms = dir.join("vms");
        let vm = vms.join("wv1");
        fs::create_dir_all(&vm).unwrap();
        let mut qemu = record("wv1");
        qemu.backend = Backend::Qemu;
        write_record(&vm, &qemu).unwrap();
        // This test process is not QEMU, so the pidfile names nothing to take back.
        fs::write(vm.join(ws_qemu::PID_FILE), std::process::id().to_string()).unwrap();

        let store = Store::open(Guest {
            dir: vms,
            ..Guest::default()
        });
        let view = store.view("wv1").expect("the VM is listed");
        assert_eq!(view.state, "stopped");
        assert!(store.inner.lock().unwrap().slots["wv1"].link.is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
