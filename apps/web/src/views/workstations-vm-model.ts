// The VM form's view-model for the Workstations page: the hub's rules for a workstation VM (sizes, the Windows image,
// macOS only on Apple hardware and at most two per Mac, the GPU's shape, the backend), the body it sends, and the labels
// the list shows. Mirrors `crates/apid/src/stations/vms.rs`, which checks the same things and says the same reasons.
// No DOM here, so the tests cover it directly.

import type { NodeView, StationVm, StationVmBody, VmBackend, VmOs } from '../api';
import type { Tone } from '../ui/components';
import { ramText } from './workstations-model';

/** The sizes the hub accepts (docs/design/workstations.md section 10). */
export const VM_CPUS = { min: 1, max: 16 } as const;
export const VM_RAM_MB = { min: 2048, max: 131_072 } as const;
export const VM_WINDOWS_RAM_MB_MIN = 4096;
export const VM_DISK_GB = { min: 8, max: 2048 } as const;
/** A VM name is a DNS label: 1 to 63 characters of a-z, 0-9 and '-', not starting or ending with '-'. */
export const VM_NAME_MAX = 63;
/** The most macOS VMs one Mac may hold (Apple's licence). */
export const VM_MACOS_PER_NODE = 2;

export type VmGpuChoice = 'none' | 'virtual' | 'vfio';
export type VmBackendChoice = 'auto' | VmBackend;

/** The form as the owner fills it: numbers as text, so a half-typed value is still a value. */
export interface VmForm {
  node: string;
  os: VmOs;
  name: string;
  cpus: string;
  ram_mb: string;
  disk_gb: string;
  gpu: VmGpuChoice;
  pci: string;
  backend: VmBackendChoice;
  image: string;
}

export const VM_FORM_DEFAULTS: VmForm = {
  node: '',
  os: 'linux',
  name: '',
  cpus: '4',
  ram_mb: '8192',
  disk_gb: '40',
  gpu: 'none',
  pci: '',
  backend: 'auto',
  image: '',
};

/** What the form needs of a node: its backends, and whether it is Apple hardware (the hub's test). */
export interface NodeVmFacts {
  id: string;
  name: string;
  /** Whether the node reported its backends at all (older nodes do not). */
  reported: boolean;
  backends: string[];
  qemu: boolean;
  apple: boolean;
  online: boolean;
}

/** A node's facts for the form, read the way the hub reads them (`workstation_backends`, `vendor`, `apple`). */
export function nodeVmFacts(node: NodeView): NodeVmFacts {
  const machine = node.machine;
  const backends = machine?.workstation_backends ?? [];
  return {
    id: node.id,
    name: node.name || node.id,
    reported: Array.isArray(machine?.workstation_backends),
    backends,
    qemu: backends.includes('qemu'),
    apple: machine?.vendor === 'apple' || machine?.apple != null,
    online: node.online,
  };
}

/** The nodes the form offers: all but the revoked ones. */
export function nodeChoices(nodes: NodeView[]): NodeVmFacts[] {
  return nodes.filter((n) => !n.revoked).map(nodeVmFacts);
}

/** A node's option in the picker: its name and id, its backends, and whether QEMU/OVMF is there. */
export function nodeOptionLabel(facts: NodeVmFacts): string {
  const backends = !facts.reported ? 'backends not reported' : facts.backends.length ? facts.backends.join(', ') : 'no backends';
  return `${facts.name} (${facts.id}) · ${backends} · ${facts.qemu ? 'QEMU/OVMF' : 'no QEMU/OVMF'}${facts.online ? '' : ' · offline'}`;
}

/** One line under the node picker: its backends, and whether it runs QEMU and Apple's VMs. */
export function nodeFactsLine(facts: NodeVmFacts | undefined): string {
  if (!facts) return 'Choose the node that runs the VM.';
  const state = facts.online ? '' : ' · offline: the hub asks it anyway, and the VM waits until it is back';
  if (!facts.reported) return `Its backends are not reported (an older node).${state}`;
  const backends = facts.backends.length ? facts.backends.join(', ') : 'none';
  return `Backends: ${backends} · QEMU/OVMF: ${facts.qemu ? 'yes' : 'no'} · Apple hardware: ${facts.apple ? 'yes' : 'no'}${state}`;
}

/** The hub's name rule, in words. Null when the name is fine. */
export function vmNameProblem(name: string): string | null {
  const t = name.trim();
  if (!t) return 'Give the VM a name.';
  return /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(t)
    ? null
    : `The name is up to ${VM_NAME_MAX} characters of a-z, 0-9 and '-', not starting or ending with '-'.`;
}

/** A whole number in `[lo, hi]` from `text`, or null. */
export function wholeIn(text: string, lo: number, hi: number): number | null {
  const t = text.trim();
  if (!/^\d+$/.test(t)) return null;
  const n = Number(t);
  return n >= lo && n <= hi ? n : null;
}

/** A PCI address, `DDDD:BB:DD.F` (the hub checks the shape; the node checks the device). Null when fine. */
export function pciProblem(pci: string): string | null {
  return /^[0-9a-f]{4}:[0-9a-f]{2}:[0-9a-f]{2}\.[0-7]$/.test(pci.trim().toLowerCase())
    ? null
    : 'A passthrough GPU needs its PCI address, like 0000:01:00.0.';
}

/** A Windows image is a path on the node: absolute from `/`, or a drive path like `D:\images\win.img`. Null when fine. */
export function imageProblem(image: string): string | null {
  const t = image.trim();
  if (!t) return 'A Windows VM needs the path of its disk image on the node.';
  return t.startsWith('/') || /^[A-Za-z]:\\/.test(t)
    ? null
    : `The image is an absolute path on the node, like /srv/images/win.img or D:\\images\\win.img, not ${t}.`;
}

/**
 * The backend the hub picks (`backend_for`). `auto` takes QEMU for Windows or a passthrough GPU, which plaza cannot
 * run, and plaza otherwise. Returns the backend, or the reason the node cannot take it.
 */
export function backendFor(
  form: Pick<VmForm, 'os' | 'gpu' | 'backend'>,
  nodeHasQemu: boolean,
): { ok: true; backend: VmBackend } | { ok: false; error: string } {
  if (form.backend === 'plaza') return { ok: true, backend: 'plaza' };
  const wantQemu = form.backend === 'qemu' || form.os === 'windows' || form.gpu === 'vfio';
  if (!wantQemu) return { ok: true, backend: 'plaza' };
  if (form.os === 'macos') return { ok: false, error: "A macOS VM runs on Apple's Virtualization framework, not QEMU." };
  if (!nodeHasQemu) return { ok: false, error: 'This node has no QEMU/OVMF; install qemu and edk2-ovmf.' };
  return { ok: true, backend: 'qemu' };
}

/** Why the backend is what it is, in words: "Runs on QEMU: <reason>." */
export function backendReason(form: Pick<VmForm, 'os' | 'gpu' | 'backend'>, backend: VmBackend): string {
  if (form.backend === 'plaza') return 'you chose plaza';
  if (form.backend === 'qemu') return 'you chose QEMU';
  if (backend === 'qemu') {
    return form.os === 'windows' ? 'auto picks QEMU for a Windows VM' : 'auto picks QEMU for a passthrough GPU';
  }
  return 'auto picks plaza for this guest';
}

/** The backend hint under the picker: where the VM runs, and a warning when plaza cannot run what was asked. */
export function backendHint(form: Pick<VmForm, 'os' | 'gpu' | 'backend'>, nodeHasQemu: boolean): string {
  const picked = backendFor(form, nodeHasQemu);
  if (!picked.ok) return picked.error;
  const note = form.backend === 'plaza' && (form.os === 'windows' || form.gpu === 'vfio')
    ? ' plaza cannot run a Windows VM or a passthrough GPU.'
    : '';
  return `Runs on ${vmBackendText(picked.backend)}: ${backendReason(form, picked.backend)}.${note}`;
}

/**
 * The first problem with the form, in the hub's order: the node, the name, the sizes, the GPU, the OS's own rules
 * (the image; macOS on Apple hardware and its count), then the backend. Null when the VM can be asked for.
 */
export function vmProblem(form: VmForm, node: NodeVmFacts | undefined, macosOnNode: number): string | null {
  if (!node) return 'Choose a node to run the VM on.';
  const name = vmNameProblem(form.name);
  if (name) return name;
  if (wholeIn(form.cpus, VM_CPUS.min, VM_CPUS.max) === null) {
    return `CPUs: a whole number from ${VM_CPUS.min} to ${VM_CPUS.max}.`;
  }
  const minRam = form.os === 'windows' ? VM_WINDOWS_RAM_MB_MIN : VM_RAM_MB.min;
  if (wholeIn(form.ram_mb, minRam, VM_RAM_MB.max) === null) {
    return `Memory: a whole number of MiB from ${minRam}${form.os === 'windows' ? ' for Windows' : ''} to ${VM_RAM_MB.max}.`;
  }
  if (wholeIn(form.disk_gb, VM_DISK_GB.min, VM_DISK_GB.max) === null) {
    return `Disk: a whole number of GiB from ${VM_DISK_GB.min} to ${VM_DISK_GB.max}.`;
  }
  if (form.gpu === 'vfio') {
    const pci = pciProblem(form.pci);
    if (pci) return pci;
  }
  if (form.os === 'windows') {
    const image = imageProblem(form.image);
    if (image) return image;
  }
  if (form.os === 'macos') {
    if (!node.apple) return `A macOS VM runs only on Apple hardware, and ${node.name} is not an Apple Mac.`;
    if (macosOnNode >= VM_MACOS_PER_NODE) {
      return `At most ${VM_MACOS_PER_NODE} macOS VMs per Mac, and this Mac already has ${macosOnNode}.`;
    }
  }
  const backend = backendFor(form, node.qemu);
  return backend.ok ? null : backend.error;
}

/** The body the form sends. The image goes only with a Windows VM; the GPU is `none`, `virtual` or `vfio:<pci-id>`. */
export function vmBody(form: VmForm): StationVmBody {
  const body: StationVmBody = {
    node: form.node,
    os: form.os,
    name: form.name.trim(),
    cpus: Number(form.cpus.trim()),
    ram_mb: Number(form.ram_mb.trim()),
    disk_gb: Number(form.disk_gb.trim()),
    gpu: form.gpu === 'vfio' ? `vfio:${form.pci.trim()}` : form.gpu,
    backend: form.backend,
  };
  if (form.os === 'windows') body.image = form.image.trim();
  return body;
}

/** The form's problem, or the body to send. */
export function readVmForm(
  form: VmForm,
  node: NodeVmFacts | undefined,
  macosOnNode: number,
): { ok: true; body: StationVmBody } | { ok: false; error: string } {
  const error = vmProblem(form, node, macosOnNode);
  return error ? { ok: false, error } : { ok: true, body: vmBody(form) };
}

/** Which fields the form shows for the choices made: the Windows image, the passthrough address. */
export const showsImage = (form: Pick<VmForm, 'os'>): boolean => form.os === 'windows';
export const showsPci = (form: Pick<VmForm, 'gpu'>): boolean => form.gpu === 'vfio';

export function vmOsText(os: VmOs): string {
  return os === 'macos' ? 'macOS' : os === 'windows' ? 'Windows' : 'Linux';
}

export function vmGpuText(gpu: string): string {
  if (!gpu || gpu === 'none') return 'no GPU';
  if (gpu === 'virtual') return 'virtual GPU';
  return gpu.startsWith('vfio:') ? `passthrough ${gpu.slice('vfio:'.length)}` : gpu;
}

export function vmBackendText(backend: VmBackend): string {
  return backend === 'qemu' ? 'QEMU' : 'plaza';
}

/** A VM's size in words: vCPUs, memory and disk. */
export function vmSizeText(vm: Pick<StationVm, 'cpus' | 'ram_mb' | 'disk_gb'>): string {
  return `${vm.cpus} vCPU · ${ramText(vm.ram_mb)} · ${vm.disk_gb} GB disk`;
}

/** The VM's state chip: online as a workstation; offline once it has enrolled; else not joined yet. */
export function vmStateChip(vm: Pick<StationVm, 'online' | 'station'>): { label: string; tone: Tone } {
  if (vm.online) return { label: 'online', tone: 'ok' };
  if (vm.station) return { label: 'offline', tone: 'muted' };
  return { label: 'joining', tone: 'info' };
}

/** The VMs in list order: by name, then by id. The input is not changed. */
export function sortVms(vms: StationVm[]): StationVm[] {
  return [...vms].sort((a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id));
}

/** How many macOS VMs a node already holds, from the VMs the page has. */
export function macosOn(vms: StationVm[], node: string): number {
  return vms.filter((vm) => vm.node === node && vm.os === 'macos').length;
}
