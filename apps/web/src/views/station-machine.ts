// A workstation as the Machines list shows it: its row (a MachineNode) plus the station it came from. Pure, no DOM.
import type { MachineNode, StationForm, StationView } from '../api';

/** A workstation's row in the Machines list: the machine's fields, with the station kept beside them. */
export interface StationNode extends MachineNode { station: StationView }

const FORM_LABEL: Record<StationForm, string> = { laptop: 'Laptop', desktop: 'Desktop', mini: 'Mini', server: 'Server', vm: 'VM' };
const OS_LABEL: Record<StationView['facts']['os'], string> = { macos: 'macOS', windows: 'Windows', linux: 'Linux' };

/** Lease counts: every lease, the ones holding the station (opening or active), and the ones waiting in the queue. */
export function leaseCounts(s: StationView): { leases: number; active: number; queued: number } {
  let active = 0;
  let queued = 0;
  for (const l of s.leases) {
    if (l.state === 'queued') queued++;
    else active++;
  }
  return { leases: s.leases.length, active, queued };
}

/** The row the Machines list shows for a workstation. */
export function stationMachine(s: StationView): StationNode {
  const f = s.facts;
  return {
    id: s.id,
    name: s.name,
    kind: 'workstation',
    vendor: f.os === 'macos' ? 'apple' : null,
    model: null,
    apple: null,
    arch: f.arch,
    backend: '',
    irqchip: '',
    class: '',
    vsock_mode: '',
    started_at_ms: 0,
    cpus: f.cpus,
    memory_mib: f.ram_mb,
    gpus: f.gpus.map((g) => ({ vendor: g.vendor, name: g.name, memory_mib: g.vram_mb > 0 ? g.vram_mb : null })),
    runs: [],
    agents: leaseCounts(s).active,
    reachable: s.online,
    stats: null,
    station: s,
  };
}

/** One line for the workstation: "Workstation · Mac mini · macOS 15.1 · arm64 · 10 cores · 16 GiB · NVIDIA RTX 4090". */
export function stationLine(s: StationView): string {
  const f = s.facts;
  const form = f.form === 'mini' && f.os === 'macos' ? 'Mac mini' : FORM_LABEL[f.form];
  const os = f.os_version ? `${OS_LABEL[f.os]} ${f.os_version}` : OS_LABEL[f.os];
  const gpus = f.gpus.map((g) => (g.name.toLowerCase().includes(g.vendor.toLowerCase()) ? g.name : `${g.vendor} ${g.name}`));
  const parts = [
    'Workstation',
    form,
    os,
    f.arch,
    f.cpus > 0 ? `${f.cpus} ${f.cpus === 1 ? 'core' : 'cores'}` : '',
    f.ram_mb > 0 ? `${Math.round(f.ram_mb / 1024)} GiB` : '',
    ...gpus,
  ];
  return parts.filter((p) => p !== '').join(' · ');
}

/** A badge when the agents work in the owner's own account; null when they have their own. */
export function stationBadge(s: StationView): string | null {
  return s.facts.account.mode === 'owner' ? `agents act as ${s.facts.account.name}` : null;
}

/** The path of the station's latest thumbnail picture. */
export function stationThumbnailPath(id: string): string {
  return `/v1/stations/${encodeURIComponent(id)}/thumbnail`;
}

/** True when a machine row is a workstation (it carries its station). */
export function isStationNode(m: MachineNode): m is StationNode {
  return 'station' in m && m.station != null;
}
