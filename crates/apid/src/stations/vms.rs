//! Workstation VMs the plaza makes on a node (docs/design/workstations.md, section 10).
//!
//! Owner routes (the owner gate applies, as to every `/v1/stations` route):
//!   POST   /v1/stations/vms       {node, os: linux|windows|macos, name, cpus, ram_mb, disk_gb, gpu?, image?, backend?} → 201 VmView
//!   GET    /v1/stations/vms       → [VmView]
//!   DELETE /v1/stations/vms/{id}  → the removed VmView (its VM is removed from the node first)
//!
//! A VM is made on a node by noded (`POST /v1/workstations`) with a join code the hub issues for it, so its daemon
//! enrols as a workstation. The station that enrols with that code is this VM: its facts say `form: vm`, and its
//! leases run the snapshot and the revert (`snapshot_reset`) through the node.
//!
//! Checked here, before any node is asked: macOS VMs only on a node that reports Apple hardware, at most
//! [`MAX_MACOS_PER_NODE`] per node; Windows VMs need an `image` (an owner-provided image on the node, copied per VM,
//! nothing downloaded); the sizes and GPU options, as noded checks them too; and the backend. `backend` is `auto`
//! (the default), `plaza` or `qemu`. `auto` takes QEMU for a Windows guest or a vfio GPU, which plaza cannot run, and
//! refuses when the node does not report QEMU (its facts list `qemu` in `workstation_backends`).
//!
//! The lease hooks (`hooks.rs`) run the snapshot before a lease on a workstation VM with `snapshot_reset` is opened,
//! and the revert after the last active lease ends. Each is stop, then the disk step, then start, through the node,
//! and the station is waited on to come back before the lease opens (or the queue is granted again). While a reset
//! runs the station is marked resetting: its leases wait and no new lease opens on it.

use std::time::Duration;

use apz_http::{Request, Response, Router};
use apz_station::{Facts, Form};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::registry::Station;
use super::{ops, ops::Failure};
use crate::fleet;
use crate::state::App;

/// The most macOS VMs one Mac may hold (Apple's licence for macOS guests).
pub const MAX_MACOS_PER_NODE: usize = 2;
/// The sizes noded checks (workstation.rs there), checked here first so a refusal comes before a node is asked.
pub const MAX_CPUS: u32 = 16;
pub const MIN_RAM_MB: u32 = 2048;
pub const MIN_WINDOWS_RAM_MB: u32 = 4096;
pub const MAX_RAM_MB: u32 = 131_072;
pub const MIN_DISK_GB: u32 = 8;
pub const MAX_DISK_GB: u32 = 2_048;
/// The longest VM name, in characters (a DNS label).
pub const MAX_NAME_LEN: usize = 63;
/// How long a join code for a VM lives, in minutes.
const CODE_TTL_MINUTES: u64 = 15;
/// How long a station has to drop its link (the VM is stopped) and then to dial in again (the VM is started).
const LINK_DOWN_WAIT: Duration = Duration::from_secs(60);
const LINK_BACK_WAIT: Duration = Duration::from_secs(240);
const LINK_POLL: Duration = Duration::from_millis(500);

/// A workstation VM the hub knows: where it runs on its node, and the station it enrolled as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmRecord {
    /// `v<seq>`.
    pub id: String,
    /// The node the VM runs on (`n<seq>`, or `n0` for the hub's own machine).
    pub node: String,
    /// The VM's id on that node (`wv<n>`).
    pub vm: String,
    pub name: String,
    pub os: String,
    pub cpus: u32,
    pub ram_mb: u32,
    pub disk_gb: u32,
    pub gpu: String,
    pub image: Option<String>,
    /// The launch backend the node runs the VM under: `plaza` or `qemu`. A record from before QEMU is plaza.
    #[serde(default = "plaza_backend")]
    pub backend: String,
    /// The join code it was issued (`j<seq>`).
    pub code: String,
    /// The station that enrolled from its code (`w<seq>`), once one has.
    #[serde(default)]
    pub station: Option<String>,
    pub created_ms: u64,
}

fn plaza_backend() -> String {
    "plaza".into()
}

/// The `POST /v1/stations/vms` body.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateVm {
    pub node: String,
    pub os: String,
    pub name: String,
    pub cpus: u32,
    pub ram_mb: u32,
    pub disk_gb: u32,
    #[serde(default)]
    pub gpu: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    /// `auto` (the default), `plaza` or `qemu`.
    #[serde(default)]
    pub backend: Option<String>,
}

/// The launch backend a create request gets: `plaza` or `qemu`. `auto` takes QEMU for a Windows guest or a vfio GPU,
/// and plaza otherwise. QEMU is refused, with the install hint, on a node that does not report it.
pub fn backend_for(request: &CreateVm, node_has_qemu: bool) -> Result<&'static str, String> {
    let vfio = request
        .gpu
        .as_deref()
        .is_some_and(|gpu| gpu.starts_with("vfio:"));
    let needs_qemu = request.os == "windows" || vfio;
    let want_qemu = match request.backend.as_deref().unwrap_or("auto") {
        "auto" => needs_qemu,
        "plaza" => return Ok("plaza"),
        "qemu" => true,
        other => return Err(format!("backend must be auto, plaza or qemu, not {other:?}")),
    };
    if !want_qemu {
        return Ok("plaza");
    }
    if request.os == "macos" {
        return Err("a macOS VM runs on Apple's Virtualization framework, not QEMU".into());
    }
    if !node_has_qemu {
        return Err("this node has no QEMU/OVMF; install qemu and edk2-ovmf".into());
    }
    Ok("qemu")
}

/// A VM as the console lists it: its record, and whether its station is enrolled and online.
#[derive(Debug, Clone, Serialize)]
pub struct VmView {
    #[serde(flatten)]
    pub vm: VmRecord,
    pub online: bool,
}

/// Checks a create request: the node's Apple hardware and macOS count, the OS and its image, the sizes and the GPU
/// option. Pure, so the rules are tested without a node.
pub fn check(request: &CreateVm, node_is_apple: bool, macos_on_node: usize) -> Result<(), String> {
    checked_name(&request.name)?;
    in_range("cpus", request.cpus, 1, MAX_CPUS)?;
    let min_ram = if request.os == "windows" {
        MIN_WINDOWS_RAM_MB
    } else {
        MIN_RAM_MB
    };
    in_range("ram_mb", request.ram_mb, min_ram, MAX_RAM_MB)?;
    in_range("disk_gb", request.disk_gb, MIN_DISK_GB, MAX_DISK_GB)?;
    checked_gpu(request.gpu.as_deref().unwrap_or("none"))?;
    match request.os.as_str() {
        "linux" => Ok(()),
        "windows" => match request.image.as_deref() {
            Some(path) if path.starts_with('/') || is_windows_absolute(path) => Ok(()),
            Some(path) => Err(format!(
                "image must be an absolute path to a disk image on the node, not {path:?}"
            )),
            None => Err(
                "a Windows VM needs image: the path of an owner-provided disk image on its node"
                    .into(),
            ),
        },
        "macos" => {
            if !node_is_apple {
                return Err(format!(
                    "a macOS VM runs only on Apple hardware, and node {} is not an Apple Mac",
                    request.node
                ));
            }
            if macos_on_node >= MAX_MACOS_PER_NODE {
                return Err(format!(
                    "at most {MAX_MACOS_PER_NODE} macOS VMs per Mac, and this one already has {macos_on_node}"
                ));
            }
            Ok(())
        }
        other => Err(format!("os must be linux, windows or macos, not {other:?}")),
    }
}

fn is_windows_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() > 2 && bytes[1] == b':' && bytes[2] == b'\\' && bytes[0].is_ascii_alphabetic()
}

fn checked_name(name: &str) -> Result<(), String> {
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
            "name must be 1 to {MAX_NAME_LEN} characters of a-z, 0-9 and '-': {name:?}"
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

/// `none`, `virtual`, or `vfio:<pci-id>` (the address shape only; the node checks the device itself).
fn checked_gpu(text: &str) -> Result<(), String> {
    if matches!(text, "none" | "virtual") {
        return Ok(());
    }
    let Some(pci) = text.strip_prefix("vfio:") else {
        return Err(format!(
            "gpu must be none, virtual or vfio:<pci-id>, not {text:?}"
        ));
    };
    let lower = pci.to_ascii_lowercase();
    let well_formed = lower.len() == 12
        && lower.char_indices().all(|(at, c)| match at {
            4 | 7 => c == ':',
            10 => c == '.',
            11 => ('0'..='7').contains(&c),
            _ => c.is_ascii_hexdigit(),
        });
    if well_formed {
        Ok(())
    } else {
        Err(format!(
            "a vfio GPU needs a PCI address like 0000:01:00.0, not {pci:?}"
        ))
    }
}

/// The steps a lease hook sends a node, by the VM they act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Stop,
    Snapshot,
    Revert,
    Start,
}

impl Step {
    fn path(self, vm: &str) -> String {
        let action = match self {
            Self::Stop => "stop",
            Self::Snapshot => "snapshot",
            Self::Revert => "revert",
            Self::Start => "start",
        };
        format!("/v1/workstations/{vm}/{action}")
    }
}

/// How the hub calls a node's noded: the seam the lease hooks are tested through.
pub trait NodeCalls {
    fn call(&self, node: &str, method: &str, path: &str) -> Result<(u16, Value), String>;
}

/// The real calls: through the node link (`fleet::noded`).
pub struct Through<'a>(pub &'a App);

impl NodeCalls for Through<'_> {
    fn call(&self, node: &str, method: &str, path: &str) -> Result<(u16, Value), String> {
        fleet::noded(self.0, node, method, path, None)
    }
}

fn step(calls: &dyn NodeCalls, node: &str, vm: &str, what: Step) -> Result<(), String> {
    match calls.call(node, "POST", &what.path(vm)) {
        Ok((status, _)) if (200..300).contains(&status) => Ok(()),
        Ok((status, body)) => {
            let reason = body
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| body.to_string());
            Err(format!("{what:?} on {node} answered {status}: {reason}"))
        }
        Err(reason) => Err(format!("{what:?} on {node}: {reason}")),
    }
}

/// Runs `work` between a stop and a start, so a VM stopped for it is started again whatever the work did. A stop that
/// fails runs nothing else: the VM is as it was. Returns the work's failure, else the start's.
pub fn around(calls: &dyn NodeCalls, node: &str, vm: &str, work: Step) -> Result<(), String> {
    step(calls, node, vm, Step::Stop)?;
    let worked = step(calls, node, vm, work);
    let started = step(calls, node, vm, Step::Start);
    worked.and(started)
}

/// The snapshot or the revert a lease hook runs for `station`'s VM, when the station is a VM whose setting asks for
/// resets. The reset is the disk step of [`around`].
fn reset_vm(app: &App, station: &str) -> Option<VmRecord> {
    let vm = app.stations.vm_of_station(station)?;
    let settings = app.stations.station(station)?.settings;
    settings.snapshot_reset.then_some(vm)
}

/// Whether a lease is open on `station` that holds the VM (anything but a queued lease).
fn busy(app: &App, station: &str) -> bool {
    app.stations
        .leases
        .views(station)
        .iter()
        .any(|view| view.lease.state != super::leases::State::Queued)
}

/// Waits for the station's link to go down and come back (`live` is set while its daemon's link is up).
fn wait_for_return(app: &App, station: &str) -> bool {
    let down = wait_until(LINK_DOWN_WAIT, || app.stations.live(station).is_none());
    down && wait_until(LINK_BACK_WAIT, || app.stations.live(station).is_some())
}

fn wait_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + limit;
    loop {
        if done() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(LINK_POLL);
    }
}

/// Before a lease opens on `station`: when it is a VM with `snapshot_reset` and no lease holds it, its disk is
/// snapshotted (stop, snapshot, start) and the station is waited on to come back. Refused while a reset runs.
pub fn before_lease(app: &App, station: &str) -> Result<(), Failure> {
    let Some(vm) = reset_vm(app, station) else {
        return Ok(());
    };
    if busy(app, station) {
        return Ok(());
    }
    if !app.stations.begin_reset(station) {
        return Err((
            409,
            "this workstation is resetting after its last lease; try again shortly".into(),
        ));
    }
    let made = around(&Through(app), &vm.node, &vm.vm, Step::Snapshot);
    let back = wait_for_return(app, station);
    app.stations.end_reset(station);
    made.map_err(|reason| {
        (
            502,
            format!("the snapshot before this lease failed: {reason}"),
        )
    })?;
    if !back {
        return Err((
            502,
            "the workstation did not come back after its snapshot".into(),
        ));
    }
    Ok(())
}

/// After a lease ends on `station`: when it was the last active lease and the station is a VM with `snapshot_reset`,
/// its disk is reverted (stop, revert, start) on a thread, and the queue is granted once the station is back.
pub fn after_lease(app: &App, station: &str) {
    let Some(vm) = reset_vm(app, station) else {
        return;
    };
    if busy(app, station) || app.stations.station(station).is_some_and(|s| s.revoked) {
        return;
    }
    if !app.stations.begin_reset(station) {
        return;
    }
    let weak = app.handle();
    let station = station.to_string();
    std::thread::spawn(move || {
        let Some(app) = weak.upgrade() else { return };
        let reverted = around(&Through(&app), &vm.node, &vm.vm, Step::Revert);
        let back = wait_for_return(&app, &station);
        app.stations.end_reset(&station);
        let (kind, data) = match (reverted, back) {
            (Ok(()), true) => ("vm_reset", json!({"vm": vm.id})),
            (Err(reason), _) => ("vm_reset_failed", json!({"vm": vm.id, "reason": reason})),
            (Ok(()), false) => (
                "vm_reset_failed",
                json!({"vm": vm.id, "reason": "the workstation did not come back"}),
            ),
        };
        if let Err(reason) = ops::audit(&app, Some(&station), None, kind, "station", data) {
            eprintln!("apid: workstation {station}: its reset is not in the audit: {reason}");
        }
        ops::schedule_grant(&app, &station);
    });
}

/// The facts a station reports, with its form set to `vm` when its hub record says it is a workstation VM's daemon.
pub fn stamp_form(app: &App, station: &str, mut facts: Facts) -> Facts {
    if app.stations.vm_of_station(station).is_some() {
        facts.form = Form::Vm;
    }
    facts
}

/// Links a station that enrolled with a VM's join code to that VM, and stamps its form. Returns the station.
pub fn adopt(app: &App, station: Station) -> Station {
    if app.stations.vm_of_code(&station.code).is_none() {
        return station;
    }
    app.stations.link_vm(&station.code, &station.id);
    let mut facts = station.facts.clone();
    facts.form = Form::Vm;
    app.stations.set_facts(&station.id, facts);
    app.stations.station(&station.id).unwrap_or(station)
}

/// Makes a workstation VM on a node and issues its join code: noded boots it with the code, and the VM's daemon
/// enrols as a workstation with it.
pub fn create(app: &App, request: &CreateVm, actor: &str) -> Result<VmView, Failure> {
    let node = app
        .nodes
        .get(&request.node)
        .ok_or((404, format!("no node {}", request.node)))?;
    if node.revoked {
        return Err((409, format!("node {} is revoked", node.id)));
    }
    let apple = node.machine.get("vendor").and_then(Value::as_str) == Some("apple")
        || node
            .machine
            .get("apple")
            .is_some_and(|apple| !apple.is_null());
    let macos = app
        .stations
        .vms()
        .iter()
        .filter(|vm| vm.node == node.id && vm.os == "macos")
        .count();
    check(request, apple, macos).map_err(|reason| (400, reason))?;
    let node_has_qemu = node
        .machine
        .get("workstation_backends")
        .and_then(Value::as_array)
        .is_some_and(|backends| backends.iter().any(|b| b.as_str() == Some("qemu")));
    let backend = backend_for(request, node_has_qemu).map_err(|reason| (400, reason))?;

    // The hub's address and its certificate pin: the node relays the VM's link to the hub and hands the VM the pin,
    // so the VM's TLS to the hub is verified although the VM reaches it only through the node's vsock relay.
    let (hub, cert) = ops::hub_pin(app)?;
    let (code, join_code, _) = ops::issue_code(
        app,
        &format!("vm {}", request.name),
        CODE_TTL_MINUTES,
        actor,
    )?;
    let body = json!({
        "name": request.name,
        "os": request.os,
        "cpus": request.cpus,
        "ram_mb": request.ram_mb,
        "disk_gb": request.disk_gb,
        "gpu": request.gpu.clone().unwrap_or_else(|| "none".into()),
        "image": request.image,
        "hub": hub,
        "pin": apz_trust::b64(&cert),
        "join_code": join_code,
        "backend": backend,
    });
    let reply = fleet::noded(app, &node.id, "POST", "/v1/workstations", Some(&body));
    let node_vm = match reply {
        Ok((201, answer)) => answer.get("id").and_then(Value::as_str).map(str::to_string),
        other => {
            let _ = ops::revoke_code(app, &code, actor);
            return Err(refusal_of(other));
        }
    };
    let Some(node_vm) = node_vm else {
        let _ = ops::revoke_code(app, &code, actor);
        return Err((502, format!("node {} answered without a VM id", node.id)));
    };
    let record = VmRecord {
        id: app.stations.new_vm_id().map_err(|reason| (500, reason))?,
        node: node.id.clone(),
        vm: node_vm,
        name: request.name.clone(),
        os: request.os.clone(),
        cpus: request.cpus,
        ram_mb: request.ram_mb,
        disk_gb: request.disk_gb,
        gpu: request.gpu.clone().unwrap_or_else(|| "none".into()),
        image: request.image.clone(),
        backend: backend.into(),
        code: code.clone(),
        station: None,
        created_ms: apz_plaza::now_ms(),
    };
    app.stations
        .add_vm(record.clone())
        .map_err(|reason| (500, reason))?;
    let row = json!({"vm": record.id, "node": record.node, "node_vm": record.vm, "os": record.os, "code": code});
    if let Err(reason) = ops::audit(app, None, None, "vm_create", actor, row) {
        eprintln!(
            "apid: workstation VM {}: its creation is not in the audit: {reason}",
            record.id
        );
    }
    Ok(view(app, record))
}

/// Removes a workstation VM: its VM goes from the node, its station (if it enrolled) is revoked, and its record
/// goes. A VM whose station is resetting waits: its disk is in use by the reset.
pub fn remove(app: &App, id: &str, actor: &str) -> Result<VmView, Failure> {
    let record = app
        .stations
        .vm(id)
        .ok_or((404, format!("no workstation VM {id}")))?;
    if let Some(station) = &record.station
        && app.stations.is_resetting(station)
    {
        return Err((
            409,
            "this workstation is resetting after a lease; remove it once the reset is done".into(),
        ));
    }
    match fleet::noded(
        app,
        &record.node,
        "DELETE",
        &format!("/v1/workstations/{}", record.vm),
        None,
    ) {
        Ok((200, _)) | Ok((404, _)) => {}
        other => return Err(refusal_of(other)),
    }
    match record.station.clone() {
        Some(station) => {
            if app.stations.station(&station).is_some_and(|s| !s.revoked) {
                let _ = ops::revoke(app, &station, actor);
            }
        }
        // A code nobody used is revoked too, so the removed VM's join code cannot enrol anything.
        None => {
            let _ = ops::revoke_code(app, &record.code, actor);
        }
    }
    app.stations.remove_vm(id);
    let row = json!({"vm": record.id, "node": record.node, "node_vm": record.vm, "station": record.station});
    if let Err(reason) = ops::audit(
        app,
        record.station.as_deref(),
        None,
        "vm_remove",
        actor,
        row,
    ) {
        eprintln!("apid: workstation VM {id}: its removal is not in the audit: {reason}");
    }
    Ok(view(app, record))
}

fn refusal_of(answer: Result<(u16, Value), String>) -> Failure {
    match answer {
        Ok((status, body)) => {
            let message = body
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| body.to_string());
            let status = if (400..500).contains(&status) {
                status
            } else {
                502
            };
            (status, message)
        }
        Err(reason) => (502, reason),
    }
}

fn view(app: &App, vm: VmRecord) -> VmView {
    let online = vm
        .station
        .as_deref()
        .is_some_and(|station| app.stations.is_online(station, apz_plaza::now_ms()));
    VmView { vm, online }
}

/// Every workstation VM with its station's state.
pub fn views(app: &App) -> Vec<VmView> {
    app.stations
        .vms()
        .into_iter()
        .map(|vm| view(app, vm))
        .collect()
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .post("/v1/stations/vms", |app, req, _| create_route(app, req))
        .get("/v1/stations/vms", |app, _, _| {
            Response::json(200, &views(app))
        })
        .delete("/v1/stations/vms/{id}", |app, _, p| {
            remove_route(app, p.get("id"))
        })
}

fn create_route(app: &App, req: &Request) -> Response {
    let request: CreateVm = match req.json() {
        Ok(request) => request,
        Err(err) => return Response::error(400, &format!("bad body: {err}")),
    };
    match create(app, &request, super::api::OWNER) {
        Ok(view) => Response::json(201, &view),
        Err((status, message)) => Response::error(status, &message),
    }
}

fn remove_route(app: &App, id: &str) -> Response {
    match remove(app, id, super::api::OWNER) {
        Ok(view) => Response::json(200, &view),
        Err((status, message)) => Response::error(status, &message),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn request(os: &str) -> CreateVm {
        CreateVm {
            node: "n1".into(),
            os: os.into(),
            name: "desk-1".into(),
            cpus: 4,
            ram_mb: 8192,
            disk_gb: 40,
            gpu: None,
            image: if os == "windows" {
                Some("/srv/win.img".into())
            } else {
                None
            },
            backend: None,
        }
    }

    #[test]
    fn a_linux_vm_is_accepted_and_the_sizes_are_bounded() {
        assert_eq!(check(&request("linux"), false, 0), Ok(()));
        let mut big = request("linux");
        big.cpus = MAX_CPUS + 1;
        assert!(check(&big, false, 0).unwrap_err().contains("cpus"));
        let mut small = request("linux");
        small.disk_gb = MIN_DISK_GB - 1;
        assert!(check(&small, false, 0).unwrap_err().contains("disk_gb"));
        let mut name = request("linux");
        name.name = "Desk".into();
        assert!(check(&name, false, 0).is_err());
    }

    #[test]
    fn a_windows_vm_needs_an_owner_image_on_the_node_and_more_memory() {
        assert_eq!(check(&request("windows"), false, 0), Ok(()));
        let mut none = request("windows");
        none.image = None;
        assert!(check(&none, false, 0).unwrap_err().contains("image"));
        let mut relative = request("windows");
        relative.image = Some("win.img".into());
        assert!(check(&relative, false, 0).unwrap_err().contains("absolute"));
        let mut windows_path = request("windows");
        windows_path.image = Some(r"D:\images\win.img".into());
        assert_eq!(check(&windows_path, false, 0), Ok(()));
        let mut low = request("windows");
        low.ram_mb = MIN_WINDOWS_RAM_MB - 1;
        assert!(check(&low, false, 0).unwrap_err().contains("ram_mb"));
    }

    #[test]
    fn a_macos_vm_runs_only_on_apple_hardware_and_at_most_two_per_mac() {
        let mac = request("macos");
        assert!(check(&mac, false, 0).unwrap_err().contains("Apple"));
        assert_eq!(check(&mac, true, 0), Ok(()));
        assert_eq!(check(&mac, true, 1), Ok(()));
        let err = check(&mac, true, MAX_MACOS_PER_NODE).unwrap_err();
        assert!(err.contains("at most 2"), "{err}");
    }

    #[test]
    fn the_gpu_option_is_checked_by_its_shape() {
        for ok in ["none", "virtual", "vfio:0000:01:00.0", "vfio:0000:0A:00.1"] {
            let mut r = request("linux");
            r.gpu = Some(ok.into());
            assert_eq!(check(&r, false, 0), Ok(()), "{ok}");
        }
        for bad in ["cuda", "vfio:01:00.0", "vfio:0000:01:00.9"] {
            let mut r = request("linux");
            r.gpu = Some(bad.into());
            assert!(check(&r, false, 0).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_unknown_os_is_refused() {
        assert!(
            check(&request("freebsd"), true, 0)
                .unwrap_err()
                .contains("os must be")
        );
    }

    /// A fake node: records every call in order, and answers each step with the status it is given.
    struct Fake {
        calls: RefCell<Vec<String>>,
        answer: fn(&str) -> (u16, Value),
    }

    impl NodeCalls for Fake {
        fn call(&self, node: &str, method: &str, path: &str) -> Result<(u16, Value), String> {
            self.calls
                .borrow_mut()
                .push(format!("{node} {method} {path}"));
            Ok((self.answer)(path))
        }
    }

    fn all_ok(_: &str) -> (u16, Value) {
        (200, json!({}))
    }

    #[test]
    fn a_reset_is_stop_then_the_disk_step_then_start_through_the_node() {
        let fake = Fake {
            calls: RefCell::new(Vec::new()),
            answer: all_ok,
        };
        around(&fake, "n1", "wv3", Step::Snapshot).expect("every step answers");
        assert_eq!(
            *fake.calls.borrow(),
            [
                "n1 POST /v1/workstations/wv3/stop",
                "n1 POST /v1/workstations/wv3/snapshot",
                "n1 POST /v1/workstations/wv3/start",
            ]
        );
    }

    #[test]
    fn a_failed_disk_step_still_starts_the_vm_and_reports_the_step() {
        fn snapshot_fails(path: &str) -> (u16, Value) {
            if path.ends_with("/snapshot") {
                (409, json!({"error": "must be stopped"}))
            } else {
                (200, json!({}))
            }
        }
        let fake = Fake {
            calls: RefCell::new(Vec::new()),
            answer: snapshot_fails,
        };
        let err = around(&fake, "n1", "wv3", Step::Snapshot).expect_err("the snapshot failed");
        assert!(
            err.contains("Snapshot") && err.contains("must be stopped"),
            "{err}"
        );
        assert_eq!(
            fake.calls.borrow().last().unwrap(),
            "n1 POST /v1/workstations/wv3/start",
            "the VM is started again"
        );
        assert_eq!(fake.calls.borrow().len(), 3);
    }

    #[test]
    fn a_failed_stop_runs_nothing_else() {
        fn stop_fails(path: &str) -> (u16, Value) {
            if path.ends_with("/stop") {
                (500, json!({"error": "did not stop"}))
            } else {
                (200, json!({}))
            }
        }
        let fake = Fake {
            calls: RefCell::new(Vec::new()),
            answer: stop_fails,
        };
        assert!(around(&fake, "n1", "wv3", Step::Revert).is_err());
        assert_eq!(
            fake.calls.borrow().len(),
            1,
            "a VM that did not stop is not touched again"
        );
    }

    #[test]
    fn a_revert_sequences_the_same_way_with_the_revert_step() {
        let fake = Fake {
            calls: RefCell::new(Vec::new()),
            answer: all_ok,
        };
        around(&fake, "n2", "wv7", Step::Revert).unwrap();
        let calls = fake.calls.borrow();
        assert_eq!(calls[1], "n2 POST /v1/workstations/wv7/revert");
        assert_eq!(calls.len(), 3);
    }

    #[test]
    fn a_node_refusal_names_the_node_and_the_step() {
        fn refused(_: &str) -> (u16, Value) {
            (404, json!({"error": "no workstation VM wv9"}))
        }
        let fake = Fake {
            calls: RefCell::new(Vec::new()),
            answer: refused,
        };
        let err = around(&fake, "n5", "wv9", Step::Snapshot).unwrap_err();
        assert!(
            err.contains("n5") && err.contains("404") && err.contains("wv9"),
            "{err}"
        );
    }

    #[test]
    fn a_refusal_from_a_node_keeps_its_status_when_it_is_a_client_error() {
        let (status, message) = refusal_of(Ok((400, json!({"error": "gpu must be none"}))));
        assert_eq!((status, message.as_str()), (400, "gpu must be none"));
        assert_eq!(refusal_of(Ok((500, json!({"error": "boom"})))).0, 502);
        assert_eq!(
            refusal_of(Err("node n1 is offline".into())),
            (502, "node n1 is offline".into())
        );
    }

    #[test]
    fn the_record_is_kept_without_its_station_until_one_enrols() {
        let record = VmRecord {
            id: "v1".into(),
            node: "n0".into(),
            vm: "wv1".into(),
            name: "desk".into(),
            os: "linux".into(),
            cpus: 2,
            ram_mb: 4096,
            disk_gb: 16,
            gpu: "none".into(),
            image: None,
            backend: "plaza".into(),
            code: "j1".into(),
            station: None,
            created_ms: 1,
        };
        let text = serde_json::to_string(&record).unwrap();
        let back: VmRecord = serde_json::from_str(&text).unwrap();
        assert_eq!(back, record);
    }

    #[test]
    fn auto_takes_qemu_for_windows_or_vfio_and_plaza_otherwise() {
        let mut vfio = request("linux");
        vfio.gpu = Some("vfio:0000:01:00.0".into());
        assert_eq!(backend_for(&request("linux"), false), Ok("plaza"));
        assert_eq!(backend_for(&request("linux"), true), Ok("plaza"));
        assert_eq!(backend_for(&vfio, true), Ok("qemu"));
        assert_eq!(backend_for(&request("windows"), true), Ok("qemu"));
        // A Windows guest on a node with no QEMU is refused with the install hint, not sent to plaza to fail there.
        let err = backend_for(&request("windows"), false).unwrap_err();
        assert_eq!(err, "this node has no QEMU/OVMF; install qemu and edk2-ovmf");
        assert!(backend_for(&vfio, false).unwrap_err().contains("no QEMU"));
    }

    #[test]
    fn an_explicit_backend_is_honored_and_checked() {
        let mut plaza = request("windows");
        plaza.backend = Some("plaza".into());
        assert_eq!(backend_for(&plaza, true), Ok("plaza"), "noded refuses it with the cause");

        let mut qemu = request("linux");
        qemu.backend = Some("qemu".into());
        assert_eq!(backend_for(&qemu, true), Ok("qemu"));
        assert!(backend_for(&qemu, false).unwrap_err().contains("no QEMU"));

        let mut bogus = request("linux");
        bogus.backend = Some("hyperv".into());
        assert!(backend_for(&bogus, true).unwrap_err().contains("backend must be"));

        let mut mac = request("macos");
        mac.backend = Some("qemu".into());
        assert!(backend_for(&mac, true).unwrap_err().contains("Apple"));
    }
}
