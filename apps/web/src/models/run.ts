/* Runs and VMs: the frozen /api/v0 shapes (crates/noded RunView/VmView). */

export type RunState = 'starting' | 'running' | 'done' | 'failed' | 'killed';
export type VmState = 'running' | 'exited' | 'killed';

export interface Vm {
  /** The VM's index among the run's VMs. */
  id: number;
  state: VmState;
  /** vsock guest CID (vsock_cid + index); absent on runs without vsock. */
  guest_cid?: number;
  /** The role this guest runs as, when it is one of the fleet's agents
      (the register's VmView carries it only for role VMs; serde skips the
      field otherwise, so it is absent on every ordinary run). */
  role?: string;
  /** Serial capture; present only in a single-run GET. */
  serial?: string;
}

export interface Run {
  /** `r<seq>`. */
  id: string;
  state: RunState;
  vcpus: number;
  memory_mib: number;
  /** Heartbeats before the guest powers itself off, or null (steady). */
  beats: number | null;
  created_at: number;
  ended_at: number | null;
  vms: Vm[];
}

export interface RunsView {
  runs: Run[];
}

export interface CreateRun {
  n: number;
  vcpus: number;
  memory_mib: number;
  beats?: number;
  vsock_cid?: number;
}

/** The numeric folio of a run id (`r12` -> 12); 0 when it does not parse. */
export function seqOf(id: string | null | undefined): number {
  const m = /^r(\d+)$/.exec(id ?? '');
  return m && m[1] ? Number(m[1]) : 0;
}

/** A run whose VMs may still be alive. */
export function isOpen(run: Run | null | undefined): boolean {
  return !!run && (run.state === 'starting' || run.state === 'running');
}

/** A run is on the wire when any of its VMs reports a vsock guest CID. */
export function hasVsock(run: Run | null | undefined): boolean {
  return !!run && run.vms.some((v) => v.guest_cid != null);
}

/** The role the register names for a VM, or null. Defensive on the way in:
    the field is optional (it lands only for the fleet's agents) and its value
    comes off the wire, so only a non-empty string counts — an absent field, a
    blank, or a hostile non-string all read as "no role", never as a role. */
export function roleOf(vm: Vm | null | undefined): string | null {
  const role = vm?.role;
  return typeof role === 'string' && role !== '' ? role : null;
}

export function modeText(run: Run): string {
  return run.beats == null ? 'steady · runs until killed' : `churn · ${run.beats} beats then poweroff`;
}

/** The run's guest CID range as marginalia (`10–11`); null without vsock. */
export function cidRange(run: Run): string | null {
  let lo: number | null = null;
  let hi: number | null = null;
  for (const v of run.vms) {
    if (v.guest_cid == null) continue;
    lo = lo == null ? v.guest_cid : Math.min(lo, v.guest_cid);
    hi = hi == null ? v.guest_cid : Math.max(hi, v.guest_cid);
  }
  if (lo == null || hi == null) return null;
  return lo === hi ? String(lo) : `${lo}–${hi}`;
}

/** How a VM left the running state, derived from its transcript's final lines. */
export type ExitKind = 'slept' | 'reset' | 'parked';

const EXIT_SCAN = 12;
/**
 * Slept = the ACPI S5 poweroff path (the kernel prints "reboot: Power down");
 * reset = a guest-initiated reboot (RB_AUTOBOOT — "rebooting" / "Restarting");
 * parked = the guest halted in place ("parking forever") and waited for the kill.
 */
export function exitKind(lines: string[]): ExitKind | null {
  for (let i = lines.length - 1; i >= 0 && i >= lines.length - EXIT_SCAN; i--) {
    const l = lines[i];
    if (l === undefined) continue;
    if (/power down/i.test(l)) return 'slept';
    if (/rebooting|restarting system/i.test(l)) return 'reset';
    if (/parking forever/i.test(l)) return 'parked';
  }
  return null;
}

export function exitKindText(kind: ExitKind | null): string {
  switch (kind) {
    case 'slept': return 'Slept · S5';
    case 'reset': return 'Reset';
    case 'parked': return 'Parked';
    default: return 'Exited';
  }
}
