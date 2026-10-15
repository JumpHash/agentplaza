//! The workstation VMs on the Workstations screen (docs/design/workstations.md, section 10): the VMs the plaza made on its
//! nodes, with Delete, and the New VM form that makes one. The hub checks the form again. The rules and the body live
//! here, so they are tested without a hub, and they mirror `crates/apid/src/stations/vms.rs` and the web console's
//! `workstations-vm-model.ts`.
//!
//!   GET    /v1/stations/vms        → [Vm]
//!   POST   /v1/stations/vms        {node, os, name, cpus, ram_mb, disk_gb, gpu, image?, backend} → 201 Vm
//!   DELETE /v1/stations/vms/{id}   → the removed Vm
//!   GET    /v1/nodes               → the nodes the form offers, with the backends each reports (`machine`)

use std::fmt;

use iced::widget::{column, pick_list, row, text_input};
use iced::{Element, Length};
use serde::Deserialize;
use serde_json::{Value, json};

use super::workstations::{Load, Msg, cells, field, mono_text, ram_text, table, whole_in};
use crate::console::ui;
use crate::theme;

/// The sizes the hub accepts (docs/design/workstations.md section 10): CPUs, memory in MiB, disk in GiB.
pub const VM_CPUS: (u64, u64) = (1, 16);
pub const VM_RAM_MB: (u64, u64) = (2048, 131_072);
pub const VM_WINDOWS_RAM_MB_MIN: u64 = 4096;
pub const VM_DISK_GB: (u64, u64) = (8, 2048);
/// A VM name is a DNS label: 1 to 63 characters of a-z, 0-9 and '-', not starting or ending with '-'.
pub const VM_NAME_MAX: usize = 63;
/// The most macOS VMs one Mac may hold (Apple's licence).
pub const VM_MACOS_PER_NODE: usize = 2;

const OS_CHOICES: [Os; 3] = [Os::Linux, Os::Windows, Os::MacOs];
const GPU_CHOICES: [GpuChoice; 3] = [GpuChoice::None, GpuChoice::Virtual, GpuChoice::Vfio];
const BACKEND_CHOICES: [BackendChoice; 3] = [
    BackendChoice::Auto,
    BackendChoice::Plaza,
    BackendChoice::Qemu,
];

/// A workstation VM as the hub lists it (`GET /v1/stations/vms`). The hub's join code is not read.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Vm {
    pub id: String,
    pub name: String,
    pub node: String,
    pub os: String,
    pub cpus: u64,
    pub ram_mb: u64,
    pub disk_gb: u64,
    pub gpu: String,
    pub backend: String,
    /// The station it enrolled as (`w<seq>`), once it has.
    pub station: Option<String>,
    pub online: bool,
}

/// A machine of the plaza as the hub lists it (`GET /v1/nodes`), with the parts the VM form reads of it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct NodeFacts {
    pub id: String,
    pub name: String,
    pub revoked: bool,
    pub online: bool,
    pub machine: Option<NodeMachine>,
}

/// What a node reports of its machine: its backends (`workstation_backends`), its maker and its Apple details.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct NodeMachine {
    pub workstation_backends: Option<Vec<String>>,
    pub vendor: Option<String>,
    pub apple: Option<Value>,
}

/// A node the form can pick, read the way the hub reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeChoice {
    pub id: String,
    pub name: String,
    /// Whether the node reported its backends at all (older nodes do not).
    pub reported: bool,
    pub backends: Vec<String>,
    pub qemu: bool,
    pub apple: bool,
    pub online: bool,
}

impl From<&NodeFacts> for NodeChoice {
    fn from(node: &NodeFacts) -> NodeChoice {
        let machine = node.machine.clone().unwrap_or_default();
        let backends = machine.workstation_backends.clone().unwrap_or_default();
        NodeChoice {
            id: node.id.clone(),
            name: if node.name.is_empty() {
                node.id.clone()
            } else {
                node.name.clone()
            },
            reported: machine.workstation_backends.is_some(),
            qemu: backends.iter().any(|b| b == "qemu"),
            backends,
            apple: machine.vendor.as_deref() == Some("apple") || machine.apple.is_some(),
            online: node.online,
        }
    }
}

/// A node's option in the picker: its name and id, its backends, and whether QEMU/OVMF is there.
impl fmt::Display for NodeChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let backends = if !self.reported {
            "backends not reported".to_string()
        } else if self.backends.is_empty() {
            "no backends".to_string()
        } else {
            self.backends.join(", ")
        };
        let qemu = if self.qemu {
            "QEMU/OVMF"
        } else {
            "no QEMU/OVMF"
        };
        let offline = if self.online { "" } else { " · offline" };
        write!(
            f,
            "{} ({}) · {backends} · {qemu}{offline}",
            self.name, self.id
        )
    }
}

/// The nodes the form offers: all but the revoked ones.
pub fn node_choices(nodes: &[NodeFacts]) -> Vec<NodeChoice> {
    nodes
        .iter()
        .filter(|n| !n.revoked)
        .map(NodeChoice::from)
        .collect()
}

/// The guest OS, as the hub names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    Windows,
    MacOs,
}

impl Os {
    fn word(self) -> &'static str {
        match self {
            Os::Linux => "linux",
            Os::Windows => "windows",
            Os::MacOs => "macos",
        }
    }

    /// The OS's name as the list and the picker show it.
    pub fn name(self) -> &'static str {
        match self {
            Os::Linux => "Linux",
            Os::Windows => "Windows",
            Os::MacOs => "macOS",
        }
    }

    /// The name of a VM's OS as the hub sent it; an unknown word shows as it is.
    pub fn name_of(word: &str) -> String {
        match word {
            "linux" => Os::Linux.name(),
            "windows" => Os::Windows.name(),
            "macos" => Os::MacOs.name(),
            other => other,
        }
        .to_string()
    }
}

impl fmt::Display for Os {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The GPU the form asks for: none, a virtual one, or a real one passed through (`vfio:<pci-id>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuChoice {
    None,
    Virtual,
    Vfio,
}

impl fmt::Display for GpuChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            GpuChoice::None => "None",
            GpuChoice::Virtual => "Virtual GPU",
            GpuChoice::Vfio => "Passthrough (vfio)",
        })
    }
}

/// The backend the owner asks for; `Auto` lets the hub choose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendChoice {
    Auto,
    Plaza,
    Qemu,
}

impl BackendChoice {
    fn word(self) -> &'static str {
        match self {
            BackendChoice::Auto => "auto",
            BackendChoice::Plaza => "plaza",
            BackendChoice::Qemu => "qemu",
        }
    }
}

impl fmt::Display for BackendChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BackendChoice::Auto => "Auto",
            BackendChoice::Plaza => "plaza",
            BackendChoice::Qemu => "QEMU",
        })
    }
}

/// The backend a VM runs under: plaza, or QEMU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launch {
    Plaza,
    Qemu,
}

impl Launch {
    fn label(self) -> &'static str {
        match self {
            Launch::Plaza => "plaza",
            Launch::Qemu => "QEMU",
        }
    }

    /// The backend a VM reports (`plaza` or `qemu`).
    fn of(word: &str) -> Launch {
        if word == "qemu" {
            Launch::Qemu
        } else {
            Launch::Plaza
        }
    }
}

/// The New VM form as the owner fills it: numbers as text, so a half-typed value is still a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmForm {
    pub node: String,
    pub os: Os,
    pub name: String,
    pub cpus: String,
    pub ram_mb: String,
    pub disk_gb: String,
    pub gpu: GpuChoice,
    pub pci: String,
    pub backend: BackendChoice,
    pub image: String,
}

impl Default for VmForm {
    fn default() -> VmForm {
        VmForm {
            node: String::new(),
            os: Os::Linux,
            name: String::new(),
            cpus: "4".to_string(),
            ram_mb: "8192".to_string(),
            disk_gb: "40".to_string(),
            gpu: GpuChoice::None,
            pci: String::new(),
            backend: BackendChoice::Auto,
            image: String::new(),
        }
    }
}

/// One edit of the New VM form.
#[derive(Debug, Clone)]
pub enum VmFieldEdit {
    Node(String),
    Os(Os),
    Name(String),
    Cpus(String),
    Ram(String),
    Disk(String),
    Gpu(GpuChoice),
    Pci(String),
    Backend(BackendChoice),
    Image(String),
}

/// Applies one edit to the form.
pub fn apply_edit(form: &mut VmForm, edit: VmFieldEdit) {
    match edit {
        VmFieldEdit::Node(id) => form.node = id,
        VmFieldEdit::Os(os) => form.os = os,
        VmFieldEdit::Name(text) => form.name = text,
        VmFieldEdit::Cpus(text) => form.cpus = text,
        VmFieldEdit::Ram(text) => form.ram_mb = text,
        VmFieldEdit::Disk(text) => form.disk_gb = text,
        VmFieldEdit::Gpu(gpu) => form.gpu = gpu,
        VmFieldEdit::Pci(text) => form.pci = text,
        VmFieldEdit::Backend(backend) => form.backend = backend,
        VmFieldEdit::Image(text) => form.image = text,
    }
}

/// Why a VM name cannot be used, or None. The hub's rule, in words.
pub fn vm_name_problem(name: &str) -> Option<String> {
    let t = name.trim();
    if t.is_empty() {
        return Some("Give the VM a name.".to_string());
    }
    let valid = t.len() <= VM_NAME_MAX
        && t.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !t.starts_with('-')
        && !t.ends_with('-');
    (!valid).then(|| {
        format!("The name is up to {VM_NAME_MAX} characters of a-z, 0-9 and '-', not starting or ending with '-'.")
    })
}

/// Why a passthrough GPU's address is malformed, or None: `DDDD:BB:DD.F`. The node checks the device itself.
pub fn pci_problem(pci: &str) -> Option<String> {
    let lower = pci.trim().to_ascii_lowercase();
    let b = lower.as_bytes();
    let well_formed = b.len() == 12
        && b.iter().enumerate().all(|(at, &c)| match at {
            4 | 7 => c == b':',
            10 => c == b'.',
            11 => (b'0'..=b'7').contains(&c),
            _ => c.is_ascii_hexdigit(),
        });
    (!well_formed)
        .then(|| "A passthrough GPU needs its PCI address, like 0000:01:00.0.".to_string())
}

/// Why a Windows image path is not usable, or None: an absolute path on the node, from `/` or a drive.
pub fn image_problem(image: &str) -> Option<String> {
    let t = image.trim();
    if t.is_empty() {
        return Some("A Windows VM needs the path of its disk image on the node.".to_string());
    }
    let b = t.as_bytes();
    let drive = b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\';
    (!(t.starts_with('/') || drive)).then(|| {
        format!("The image is an absolute path on the node, like /srv/images/win.img or D:\\images\\win.img, not {t}.")
    })
}

/// The backend the hub picks for the form (`backend_for`). `Auto` takes QEMU for a Windows VM or a passthrough GPU,
/// which plaza cannot run, and plaza otherwise. An error is the reason the node cannot take it.
pub fn backend_for(form: &VmForm, node_has_qemu: bool) -> Result<Launch, String> {
    if form.backend == BackendChoice::Plaza {
        return Ok(Launch::Plaza);
    }
    let wants_qemu = form.backend == BackendChoice::Qemu
        || form.os == Os::Windows
        || form.gpu == GpuChoice::Vfio;
    if !wants_qemu {
        return Ok(Launch::Plaza);
    }
    if form.os == Os::MacOs {
        return Err("A macOS VM runs on Apple's Virtualization framework, not QEMU.".to_string());
    }
    if !node_has_qemu {
        return Err("This node has no QEMU/OVMF; install qemu and edk2-ovmf.".to_string());
    }
    Ok(Launch::Qemu)
}

/// Why the backend is what it is, in words: "Runs on QEMU: <reason>."
fn backend_reason(form: &VmForm, launch: Launch) -> &'static str {
    match (form.backend, launch) {
        (BackendChoice::Plaza, _) => "you chose plaza",
        (BackendChoice::Qemu, _) => "you chose QEMU",
        (BackendChoice::Auto, Launch::Qemu) if form.os == Os::Windows => {
            "auto picks QEMU for a Windows VM"
        }
        (BackendChoice::Auto, Launch::Qemu) => "auto picks QEMU for a passthrough GPU",
        (BackendChoice::Auto, Launch::Plaza) => "auto picks plaza for this guest",
    }
}

/// The backend hint under the picker: where the VM runs, and a warning when plaza cannot run what was asked.
pub fn backend_hint(form: &VmForm, node_has_qemu: bool) -> String {
    match backend_for(form, node_has_qemu) {
        Err(error) => error,
        Ok(launch) => {
            let note = if form.backend == BackendChoice::Plaza
                && (form.os == Os::Windows || form.gpu == GpuChoice::Vfio)
            {
                " plaza cannot run a Windows VM or a passthrough GPU."
            } else {
                ""
            };
            format!(
                "Runs on {}: {}.{note}",
                launch.label(),
                backend_reason(form, launch)
            )
        }
    }
}

/// The first problem with the form, in the hub's order: the node, the name, the sizes, the GPU, the OS's own rules
/// (the image; macOS on Apple hardware and its count), then the backend. None when the VM can be asked for.
pub fn vm_problem(
    form: &VmForm,
    node: Option<&NodeChoice>,
    macos_on_node: usize,
) -> Option<String> {
    let Some(node) = node else {
        return Some("Choose a node to run the VM on.".to_string());
    };
    if let Some(problem) = vm_name_problem(&form.name) {
        return Some(problem);
    }
    if whole_in(&form.cpus, VM_CPUS).is_none() {
        return Some(format!(
            "CPUs: a whole number from {} to {}.",
            VM_CPUS.0, VM_CPUS.1
        ));
    }
    let min_ram = ram_min(form.os);
    if whole_in(&form.ram_mb, (min_ram, VM_RAM_MB.1)).is_none() {
        let windows = if form.os == Os::Windows {
            " for Windows"
        } else {
            ""
        };
        return Some(format!(
            "Memory: a whole number of MiB from {min_ram}{windows} to {}.",
            VM_RAM_MB.1
        ));
    }
    if whole_in(&form.disk_gb, VM_DISK_GB).is_none() {
        return Some(format!(
            "Disk: a whole number of GiB from {} to {}.",
            VM_DISK_GB.0, VM_DISK_GB.1
        ));
    }
    if form.gpu == GpuChoice::Vfio
        && let Some(problem) = pci_problem(&form.pci)
    {
        return Some(problem);
    }
    if form.os == Os::Windows
        && let Some(problem) = image_problem(&form.image)
    {
        return Some(problem);
    }
    if form.os == Os::MacOs {
        if !node.apple {
            return Some(format!(
                "A macOS VM runs only on Apple hardware, and {} is not an Apple Mac.",
                node.name
            ));
        }
        if macos_on_node >= VM_MACOS_PER_NODE {
            return Some(format!(
                "At most {VM_MACOS_PER_NODE} macOS VMs per Mac, and this Mac already has {macos_on_node}."
            ));
        }
    }
    backend_for(form, node.qemu).err()
}

/// The least memory a VM of this OS may have, in MiB.
fn ram_min(os: Os) -> u64 {
    if os == Os::Windows {
        VM_WINDOWS_RAM_MB_MIN
    } else {
        VM_RAM_MB.0
    }
}

/// The body the form sends, or the first problem with it. The image goes only with a Windows VM.
pub fn read_vm_form(
    form: &VmForm,
    node: Option<&NodeChoice>,
    macos_on_node: usize,
) -> Result<Value, String> {
    if let Some(problem) = vm_problem(form, node, macos_on_node) {
        return Err(problem);
    }
    // vm_problem has checked each size, so the numbers read here are the ones the form holds.
    let number = |text: &str, range: (u64, u64)| whole_in(text, range).unwrap_or_default();
    let gpu = match form.gpu {
        GpuChoice::None => "none".to_string(),
        GpuChoice::Virtual => "virtual".to_string(),
        GpuChoice::Vfio => format!("vfio:{}", form.pci.trim()),
    };
    let mut body = json!({
        "node": form.node,
        "os": form.os.word(),
        "name": form.name.trim(),
        "cpus": number(&form.cpus, VM_CPUS),
        "ram_mb": number(&form.ram_mb, (ram_min(form.os), VM_RAM_MB.1)),
        "disk_gb": number(&form.disk_gb, VM_DISK_GB),
        "gpu": gpu,
        "backend": form.backend.word(),
    });
    if form.os == Os::Windows {
        body["image"] = json!(form.image.trim());
    }
    Ok(body)
}

/// How many macOS VMs a node already holds, from the VMs the screen has.
pub fn macos_on(vms: &Load<Vec<Vm>>, node: &str) -> usize {
    match vms {
        Load::Ready(list) => list
            .iter()
            .filter(|vm| vm.node == node && vm.os == "macos")
            .count(),
        _ => 0,
    }
}

/// A node's name for the words, or its id when it is not in the list.
fn node_label(nodes: &[NodeChoice], id: &str) -> String {
    nodes
        .iter()
        .find(|n| n.id == id)
        .map_or_else(|| id.to_string(), |n| n.name.clone())
}

/// The VM's state, as a chip: online as a workstation; offline once it has enrolled; joining before that.
pub fn vm_state_chip(vm: &Vm) -> (String, ui::Tone) {
    if vm.online {
        ("online".to_string(), ui::Tone::Ok)
    } else if vm.station.is_some() {
        ("offline".to_string(), ui::Tone::Muted)
    } else {
        ("joining".to_string(), ui::Tone::Info)
    }
}

/// A VM's size in words: vCPUs, memory and disk.
pub fn vm_size(vm: &Vm) -> String {
    format!(
        "{} vCPU · {} · {} GB disk",
        vm.cpus,
        ram_text(vm.ram_mb),
        vm.disk_gb
    )
}

/// A VM's GPU in words.
pub fn vm_gpu(gpu: &str) -> String {
    match gpu {
        "" | "none" => "no GPU".to_string(),
        "virtual" => "virtual GPU".to_string(),
        other => match other.strip_prefix("vfio:") {
            Some(pci) => format!("passthrough {pci}"),
            None => other.to_string(),
        },
    }
}

/// A VM's backend in words.
fn vm_backend(word: &str) -> &'static str {
    Launch::of(word).label()
}

/// The VMs in list order: by name, then by id.
fn sorted(list: &[Vm]) -> Vec<&Vm> {
    let mut sorted: Vec<&Vm> = list.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    sorted
}

/// The VMs card's body: the list with Delete, or why there is none.
pub fn vms_view<'a>(vms: &'a Load<Vec<Vm>>, nodes: &Load<Vec<NodeFacts>>) -> Element<'a, Msg> {
    match vms {
        Load::Pending => ui::loading(),
        Load::Failed => ui::empty("alert", "Could not load the workstation VMs.", ""),
        Load::Ready(list) if list.is_empty() => ui::empty(
            "machine",
            "No workstation VMs",
            "Make one with \"New VM\". It joins as a workstation by itself.",
        ),
        Load::Ready(list) => {
            let choices = nodes_of(nodes);
            let rows = sorted(list)
                .into_iter()
                .map(|vm| vm_row(vm, node_label(&choices, &vm.node)))
                .collect();
            table(
                &[
                    (3, "Name"),
                    (2, "Node"),
                    (1, "OS"),
                    (3, "Size"),
                    (2, "GPU"),
                    (1, "Backend"),
                    (2, "State"),
                    (3, ""),
                ],
                rows,
            )
        }
    }
}

/// Whether the VM runs under plaza, the only backend Sleep and Wake reach (QEMU answers 409). A VM with no backend is plaza.
pub fn runs_on_plaza(vm: &Vm) -> bool {
    Launch::of(&vm.backend) == Launch::Plaza
}

/// One VM's row: its name (a link to its station once it has enrolled), node, OS, size, GPU, backend, state, and its
/// actions: Sleep and Wake for a plaza VM, then Delete.
fn vm_row<'a>(vm: &'a Vm, node: String) -> Element<'a, Msg> {
    let (state, tone) = vm_state_chip(vm);
    let name: Element<'a, Msg> = match &vm.station {
        Some(station) => ui::link(&vm.name, Msg::Select(station.clone()), false),
        None => ui::faint(vm.name.clone()).into(),
    };
    let mut actions: Vec<Element<'a, Msg>> = Vec::new();
    if runs_on_plaza(vm) {
        actions.push(ui::btn_sm(
            "Sleep",
            None,
            ui::Kind::Secondary,
            Some(Msg::SleepVm(vm.id.clone())),
        ));
        actions.push(ui::btn_sm(
            "Wake",
            None,
            ui::Kind::Secondary,
            Some(Msg::WakeVm(vm.id.clone())),
        ));
    }
    actions.push(ui::btn_sm(
        "Delete",
        None,
        ui::Kind::Danger,
        Some(Msg::AskRemoveVm(vm.id.clone())),
    ));
    cells(vec![
        (3, name),
        (2, ui::faint(node).into()),
        (1, ui::faint(Os::name_of(&vm.os)).into()),
        (3, ui::faint(vm_size(vm)).into()),
        (2, mono_text(vm_gpu(&vm.gpu))),
        (1, ui::faint(vm_backend(&vm.backend)).into()),
        (2, ui::badge(&state, tone)),
        (3, iced::widget::row(actions).spacing(6).into()),
    ])
}

/// The nodes as the form offers them, from the screen's load (none until it has answered).
pub fn nodes_of(nodes: &Load<Vec<NodeFacts>>) -> Vec<NodeChoice> {
    match nodes {
        Load::Ready(list) => node_choices(list),
        _ => Vec::new(),
    }
}

/// The New VM dialog's body: the form, or (once the hub made the VM) what it made.
pub fn new_vm_view<'a>(
    nodes: &Load<Vec<NodeFacts>>,
    form: &'a VmForm,
    problem: Option<&'a str>,
    made: Option<&'a Vm>,
    saving: bool,
) -> Element<'a, Msg> {
    let choices = nodes_of(nodes);
    if let Some(vm) = made {
        return made_view(vm, node_label(&choices, &vm.node), form);
    }
    let selected = choices.iter().find(|c| c.id == form.node).cloned();
    let node_hint = if choices.is_empty() {
        "There is no node to run the VM on yet.".to_string()
    } else {
        node_facts_line(selected.as_ref())
    };
    let qemu = selected.as_ref().is_some_and(|c| c.qemu);

    let mut parts: Vec<Element<'a, Msg>> = vec![
        field(
            "Name",
            text_field("desk-1", &form.name, |t| Msg::VmField(VmFieldEdit::Name(t))),
            &format!("A DNS label: a-z, 0-9 and '-', up to {VM_NAME_MAX} characters."),
        ),
        field(
            "Node",
            pick(choices.clone(), selected, |c: NodeChoice| {
                Msg::VmField(VmFieldEdit::Node(c.id))
            }),
            &node_hint,
        ),
        field(
            "OS",
            pick(OS_CHOICES.to_vec(), Some(form.os), |os: Os| {
                Msg::VmField(VmFieldEdit::Os(os))
            }),
            "macOS runs only on Apple hardware, two per Mac. Windows needs an image.",
        ),
        field(
            "CPUs",
            number_field(&form.cpus, |t| Msg::VmField(VmFieldEdit::Cpus(t))),
            &format!("{} to {}.", VM_CPUS.0, VM_CPUS.1),
        ),
        field(
            "Memory (MiB)",
            number_field(&form.ram_mb, |t| Msg::VmField(VmFieldEdit::Ram(t))),
            &format!(
                "At least {} ({VM_WINDOWS_RAM_MB_MIN} for Windows), up to {}.",
                VM_RAM_MB.0, VM_RAM_MB.1
            ),
        ),
        field(
            "Disk (GiB)",
            number_field(&form.disk_gb, |t| Msg::VmField(VmFieldEdit::Disk(t))),
            &format!("{} to {}.", VM_DISK_GB.0, VM_DISK_GB.1),
        ),
        field(
            "GPU",
            pick(GPU_CHOICES.to_vec(), Some(form.gpu), |g: GpuChoice| {
                Msg::VmField(VmFieldEdit::Gpu(g))
            }),
            "None, a virtual GPU, or a real one given to the VM.",
        ),
    ];
    if form.gpu == GpuChoice::Vfio {
        parts.push(field(
            "PCI address",
            text_field("0000:01:00.0", &form.pci, |t| {
                Msg::VmField(VmFieldEdit::Pci(t))
            }),
            "The device must already be bound to vfio-pci on the node.",
        ));
    }
    parts.push(field(
        "Backend",
        pick(
            BACKEND_CHOICES.to_vec(),
            Some(form.backend),
            |b: BackendChoice| Msg::VmField(VmFieldEdit::Backend(b)),
        ),
        &backend_hint(form, qemu),
    ));
    if form.os == Os::Windows {
        parts.push(field(
            "Windows image",
            text_field("/srv/images/win.img", &form.image, |t| Msg::VmField(VmFieldEdit::Image(t))),
            "The path of a disk image on the node, copied for the VM and never downloaded. It must carry the virtio-win drivers: viosock is needed, or the VM cannot join.",
        ));
    }
    if let Some(problem) = problem {
        parts.push(ui::error_line(problem));
    }
    parts.push(
        row![
            ui::btn(
                "Make VM",
                None,
                ui::Kind::Primary,
                (!saving).then_some(Msg::CreateVm)
            ),
            ui::btn("Cancel", None, ui::Kind::Secondary, Some(Msg::CloseDialog)),
        ]
        .spacing(8)
        .into(),
    );
    column(parts).spacing(10).into()
}

/// The VM the hub made: what it is, the backend it got and why, and that it joins by itself.
fn made_view<'a>(vm: &Vm, on: String, form: &VmForm) -> Element<'a, Msg> {
    let launch = Launch::of(&vm.backend);
    column![
        ui::faint(format!("Workstation VM {} ({}) is being made on {on}.", vm.name, vm.id)),
        ui::faint(format!("OS: {} · Size: {} · GPU: {}", Os::name_of(&vm.os), vm_size(vm), vm_gpu(&vm.gpu))),
        ui::faint(format!("Runs on {}: {}.", launch.label(), backend_reason(form, launch))),
        ui::faint("It joins as a workstation by itself: when the VM starts, its daemon enrols with the join code it was given, and it appears in Workstations once it does."),
        ui::btn("Close", None, ui::Kind::Secondary, Some(Msg::CloseDialog)),
    ]
    .spacing(10)
    .into()
}

/// A node's facts in one line under the picker: its backends, and whether it runs QEMU and Apple's VMs.
pub fn node_facts_line(node: Option<&NodeChoice>) -> String {
    let Some(node) = node else {
        return "Choose the node that runs the VM.".to_string();
    };
    let offline = if node.online {
        ""
    } else {
        " · offline: the hub asks it anyway, and the VM waits until it is back"
    };
    if !node.reported {
        return format!("Its backends are not reported (an older node).{offline}");
    }
    let backends = if node.backends.is_empty() {
        "none".to_string()
    } else {
        node.backends.join(", ")
    };
    format!(
        "Backends: {backends} · QEMU/OVMF: {} · Apple hardware: {}{offline}",
        if node.qemu { "yes" } else { "no" },
        if node.apple { "yes" } else { "no" },
    )
}

/// A pick list of choices, the selected one shown.
fn pick<'a, T>(options: Vec<T>, selected: Option<T>, on: impl Fn(T) -> Msg + 'a) -> Element<'a, Msg>
where
    T: Clone + PartialEq + fmt::Display + 'a,
{
    pick_list(options, selected, on)
        .style(theme::pick_list)
        .text_size(13.0)
        .width(Length::Fill)
        .into()
}

/// A text field in mono, for names, paths and addresses.
fn text_field<'a>(
    placeholder: &str,
    value: &str,
    on: impl Fn(String) -> Msg + 'a,
) -> Element<'a, Msg> {
    text_input(placeholder, value)
        .font(theme::MONO)
        .style(theme::input)
        .padding(8.0)
        .on_input(on)
        .into()
}

/// A number field: the value as typed, so a half-typed number is still shown as it is.
fn number_field<'a>(value: &str, on: impl Fn(String) -> Msg + 'a) -> Element<'a, Msg> {
    text_input("", value)
        .font(theme::MONO)
        .style(theme::input)
        .padding(8.0)
        .width(Length::Fixed(160.0))
        .on_input(on)
        .into()
}

#[cfg(test)]
#[path = "workstations_vms_tests.rs"]
mod tests;
