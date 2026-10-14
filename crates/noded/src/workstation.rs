//! Workstation VMs: the desktop guests a node makes for the hub (docs/design/workstations.md, section 10).
//!
//! A workstation VM is one disk (its own sparse copy of the guest image, grown to the requested size), the
//! desktop guest's kernel and initramfs, and the VMM's vCPUs and memory. The guest runs the workstation daemon,
//! started with the join code the hub issued for it: the code rides on the command line as
//! `plaza.workstation_join=`, so the daemon can enrol. The guest's display is the daemon's own virtual display (the
//! guest recipe runs Xvfb), so the node's screen is never touched.
//!
//! Snapshot and revert work on the disk. The VMM takes no live snapshots, so the VM is stopped, its disk is copied
//! to (or back from) a snapshot file, and the VM starts again. The hub sequences those calls around a lease.
//!
//! GPU options: `none`; `virtual` (the guest's software renderer, no device attached); and `vfio:<pci-id>`, which is
//! checked against sysfs and refused unless the device is bound to `vfio-pci` with an IOMMU group. The node never
//! rebinds a device itself.
//!
//! Refused rather than faked, until the VMM can do them: apz-vmm attaches no PCI device (`VMM_ATTACHES_VFIO`), and
//! it boots Linux kernels by direct boot only, with no firmware path for Windows (`VMM_BOOTS_WINDOWS`). Both are
//! validated first, so the refusal names the cause. macOS guests are refused everywhere on this node; the hub only
//! places them on Apple hardware, capped at two per Mac.
//!
//! The directory layout is `<dir>/<id>/vm.json` (the record, never the join code), `disk.img`, and, after a
//! snapshot, `snapshot.img`. A record found at start is loaded as a stopped VM.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use vmm::spawner::{self, VmHandle, VmSpec};

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

const GIB: u64 = 1 << 30;
const NO_KERNEL: &str = "no workstation kernel is configured: build the desktop guest with scripts/mk-workstation-guest.sh and start noded with --workstation-kernel and --workstation-initrd";
const NO_IMAGE: &str = "no workstation disk image is configured: build it with scripts/mk-workstation-guest.sh and start noded with --workstation-image";

/// A refusal: the HTTP status and the message.
pub type Refusal = (u16, String);

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
    /// The hub's URL the daemon joins (`https://...`); the guest's join command takes it beside the code.
    #[serde(default)]
    pub hub: Option<String>,
    pub join_code: String,
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
    pub join_code: String,
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
        checked_join_code(&self.join_code)?;
        Ok(Shape {
            name: self.name.clone(),
            os: self.os,
            cpus,
            ram_mb,
            disk_gb,
            gpu: self.gpu.clone(),
            image,
            hub,
            join_code: self.join_code.clone(),
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

/// Why this node cannot boot a VM of this shape yet, or `None` when it can.
pub fn boot_refusal(os: Os, gpu: &Gpu) -> Option<String> {
    if os == Os::Windows && !VMM_BOOTS_WINDOWS {
        return Some("Windows workstation VMs cannot boot yet: apz-vmm boots Linux kernels by direct boot and has no firmware path for Windows".into());
    }
    if matches!(gpu, Gpu::Vfio(_)) && !VMM_ATTACHES_VFIO {
        return Some("GPU passthrough (vfio) cannot boot yet: apz-vmm cannot attach a PCI device; the device was checked, nothing was bound".into());
    }
    None
}

/// The record a workstation VM is kept as on disk. It never holds the join code, which is used once, at the first boot.
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
}

impl Default for Guest {
    fn default() -> Self {
        Self {
            kernel: None,
            initrd: None,
            image: None,
            dir: apz_os::dirs::state().join("workstations"),
            sysfs: PathBuf::from("/sys"),
        }
    }
}

/// The VMM spec for one workstation VM: the guest's kernel and initramfs, the record's vCPUs and memory, its disk as
/// the writable block device. The command line names the workstation, its hub, and, on the first boot only, the join
/// code. No balloon. No vsock device: the guest has no NIC (rule 0010), and the way its daemon reaches the hub is not
/// built yet (docs/design/workstations.md, section 10), so the device is added with that path, not before.
pub fn boot_spec(
    record: &Record,
    guest: &Guest,
    join_code: Option<&str>,
    disk: &Path,
) -> Result<VmSpec, String> {
    let kernel = guest.kernel.clone().ok_or_else(|| NO_KERNEL.to_string())?;
    let initrd = guest.initrd.clone().ok_or_else(|| NO_KERNEL.to_string())?;
    let mut cmdline = format!(
        "console=ttyS0 plaza.workstation=1 plaza.workstation_name={}",
        record.name
    );
    if let Some(hub) = &record.hub {
        cmdline.push_str(" plaza.workstation_hub=");
        cmdline.push_str(hub);
    }
    if let Some(code) = join_code {
        cmdline.push_str(" plaza.workstation_join=");
        cmdline.push_str(code);
    }
    Ok(VmSpec {
        kernel,
        cmdline,
        vcpus: record.cpus,
        memory_mib: usize::try_from(record.ram_mb)
            .map_err(|_| "ram_mb is too large for this host".to_string())?,
        initrd: Some(initrd),
        gic: None,
        vsock: None,
        workspace: Some(disk.to_path_buf()),
        balloon: None,
    })
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
}

struct Slot {
    record: Record,
    handle: Option<VmHandle>,
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
}

impl Store {
    /// Opens the store over `guest.dir`, loading every record found there as a stopped VM. A directory that cannot
    /// be read starts the store empty, and a record that cannot be read is skipped, both with a line on stderr.
    pub fn open(guest: Guest) -> Self {
        let mut inner = Inner::default();
        match fs::read_dir(&guest.dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let path = entry.path().join("vm.json");
                    let Ok(text) = fs::read(&path) else { continue };
                    match serde_json::from_slice::<Record>(&text) {
                        Ok(record) => {
                            inner.next = inner.next.max(seq_of(&record.id) + 1);
                            inner.slots.insert(
                                record.id.clone(),
                                Slot {
                                    record,
                                    handle: None,
                                },
                            );
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

    /// Makes a workstation VM from `request` and boots it, with its join code, once. Every check that needs no
    /// device or file runs before the directory is made, so a refused request leaves nothing behind.
    pub fn create(&self, request: &CreateWorkstation, now: u64) -> Result<View, Refusal> {
        let shape = request.checked().map_err(bad)?;
        if let Gpu::Vfio(pci) = &shape.gpu {
            check_vfio(&self.guest.sysfs, pci).map_err(bad)?;
        }
        if let Some(refusal) = boot_refusal(shape.os, &shape.gpu) {
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
        };
        let dir = self.guest.dir.join(&id);
        let disk = dir.join("disk.img");
        // The spec is built before anything is copied: a missing kernel is refused with no directory made.
        boot_spec(&record, &self.guest, Some(&shape.join_code), &disk).map_err(|e| (503, e))?;
        if shape.os == Os::Windows && !base.is_file() {
            return Err(bad(format!(
                "the Windows image {} is not a file on this node",
                base.display()
            )));
        }
        if let Err(err) = self.make_disk(&base, &dir, &disk, shape.disk_gb) {
            let _ = fs::remove_dir_all(&dir);
            return Err((500, err));
        }
        if let Err(err) = write_record(&dir, &record) {
            let _ = fs::remove_dir_all(&dir);
            return Err((500, err));
        }
        let spec = match boot_spec(&record, &self.guest, Some(&shape.join_code), &disk) {
            Ok(spec) => spec,
            Err(err) => {
                let _ = fs::remove_dir_all(&dir);
                return Err((503, err));
            }
        };
        let handle = match spawner::spawn(spec) {
            Ok(handle) => handle,
            Err(err) => {
                let _ = fs::remove_dir_all(&dir);
                return Err((500, format!("the workstation VM did not boot: {err}")));
            }
        };
        inner.slots.insert(
            id.clone(),
            Slot {
                record,
                handle: Some(handle),
            },
        );
        inner
            .slots
            .get_mut(&id)
            .map(view_of)
            .ok_or_else(|| not_found(&id))
    }

    /// Boots a stopped VM from its disk. No join code: the daemon on its disk is already enrolled.
    pub fn start(&self, id: &str) -> Result<View, Refusal> {
        let mut inner = self.lock();
        let slot = inner.slots.get_mut(id).ok_or_else(|| not_found(id))?;
        refresh(slot);
        if slot.handle.is_some() {
            return Err((409, format!("workstation {id} is already running")));
        }
        let disk = self.disk_of(&slot.record.id);
        let spec = boot_spec(&slot.record, &self.guest, None, &disk).map_err(|e| (503, e))?;
        let handle = spawner::spawn(spec)
            .map_err(|err| (500, format!("the workstation VM did not boot: {err}")))?;
        slot.handle = Some(handle);
        Ok(view_of(slot))
    }

    /// Stops a VM: its VMM process is killed (the VMM has no guest shutdown the node can wait on).
    pub fn stop(&self, id: &str) -> Result<View, Refusal> {
        let mut inner = self.lock();
        let slot = inner.slots.get_mut(id).ok_or_else(|| not_found(id))?;
        refresh(slot);
        if let Some(mut handle) = slot.handle.take()
            && let Err(err) = handle.kill()
        {
            slot.handle = Some(handle);
            return Err((500, format!("workstation {id} did not stop: {err}")));
        }
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
        replace_file(
            &self.disk_of(id),
            &dir.join("snapshot.tmp"),
            &dir.join("snapshot.img"),
        )
        .map_err(|e| (500, e))?;
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
        let snapshot = dir.join("snapshot.img");
        if !snapshot.is_file() {
            return Err((
                409,
                format!("workstation {id} has no snapshot to revert to"),
            ));
        }
        replace_file(&snapshot, &dir.join("disk.tmp"), &self.disk_of(id)).map_err(|e| (500, e))?;
        Ok(view_of(slot))
    }

    /// Stops a VM and removes its directory: its disk and its snapshot go with it.
    pub fn delete(&self, id: &str) -> Result<View, Refusal> {
        let mut inner = self.lock();
        let mut slot = inner.slots.remove(id).ok_or_else(|| not_found(id))?;
        if let Some(mut handle) = slot.handle.take()
            && let Err(err) = handle.kill()
        {
            let message = format!("workstation {id} did not stop, so it was not removed: {err}");
            inner.slots.insert(id.to_string(), slot);
            return Err((500, message));
        }
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

    fn disk_of(&self, id: &str) -> PathBuf {
        self.dir_of(id).join("disk.img")
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

/// Notices an exited VMM: its handle is dropped, so the VM reads as stopped.
fn refresh(slot: &mut Slot) {
    if let Some(handle) = slot.handle.as_mut()
        && !matches!(handle.wait_timeout(Duration::from_millis(1)), Ok(None))
    {
        slot.handle = None;
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
        pid: slot.handle.as_ref().and_then(VmHandle::process_id),
        snapshot: record.snapshot,
        created_ms: record.created_ms,
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

    fn request(os: Os) -> CreateWorkstation {
        CreateWorkstation {
            name: "desk-1".into(),
            os,
            cpus: 4,
            ram_mb: 8192,
            disk_gb: 40,
            gpu: Gpu::None,
            image: if os == Os::Windows {
                Some("/srv/win.img".into())
            } else {
                None
            },
            hub: Some("https://hub.example:7443/".into()),
            join_code: "abc.def-ghi_123".into(),
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
    fn the_boot_spec_carries_the_join_code_once_and_no_vsock_device() {
        let guest = Guest {
            kernel: Some(PathBuf::from("/g/vmlinux")),
            initrd: Some(PathBuf::from("/g/initrd.gz")),
            ..Guest::default()
        };
        let disk = Path::new("/d/disk.img");
        let first = boot_spec(&record("wv1"), &guest, Some("CODE"), disk).expect("a spec");
        assert!(
            first
                .cmdline
                .contains("plaza.workstation=1 plaza.workstation_name=desk-1"),
            "{}",
            first.cmdline
        );
        assert!(
            first
                .cmdline
                .contains("plaza.workstation_hub=https://hub.example:7443")
        );
        assert!(
            first.cmdline.ends_with("plaza.workstation_join=CODE"),
            "{}",
            first.cmdline
        );
        assert!(first.vsock.is_none() && first.balloon.is_none());
        assert_eq!(first.workspace.as_deref(), Some(disk));
        assert_eq!((first.vcpus, first.memory_mib), (2, 4096));

        let later = boot_spec(&record("wv1"), &guest, None, disk).expect("a spec");
        assert!(
            !later.cmdline.contains("join"),
            "a later boot carries no join code"
        );
        assert!(
            later.cmdline.contains("plaza.workstation_hub="),
            "a later boot still names its hub"
        );

        let missing = boot_spec(&record("wv1"), &Guest::default(), None, disk).unwrap_err();
        assert!(missing.contains("scripts/mk-workstation-guest.sh"));
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
        let windows = store.create(&request(Os::Windows), 1).unwrap_err();
        assert_eq!(windows.0, 501);
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
}
