/* The 3D district's pure derivations — every number and string the scene
   raises from the register's data, computed without three.js so the harness
   can test them directly: the run platforms' nameplates, the host marker's
   label, the camera framings (overview and focus-run), and a session arc's
   lift. view3d.ts consumes these; nothing here touches the DOM or WebGL. */

import { fmtNum } from '../format';
import type { Run } from '../models/run';
import { cidRange } from '../models/run';

/** The vsock host endpoint every guest session addresses (VMADDR_CID_HOST). */
export const HOST_CID = 2;

/** A run platform's nameplate: run id + state over the CID range and shape. */
export function runPlateLines(run: Run): { title: string; sub: string } {
  const cids = cidRange(run);
  const n = run.vms.length;
  return {
    title: `${run.id} · ${run.state}`,
    sub:
      `${cids != null ? `cid ${cids}` : 'no vsock'} · ${n} ${n === 1 ? 'VM' : 'VMs'} × ` +
      `${run.vcpus} vCPU · ${fmtNum(run.memory_mib)} MiB`,
  };
}

export interface Framing {
  pos: [number, number, number];
  target: [number, number, number];
}

/** The overview framing: the whole plot plus every platform row in one
    frame. The machine span mirrors the boot framing (1 unit per CPU across,
    1 per 4 GiB deep, clamped); many runs widen past it so no platform is
    cropped. */
export function overviewFraming(cpus: number, memoryMib: number, runs: number): Framing {
  const machine = Math.max(16, Math.min(48, Math.max(cpus, memoryMib / 4096)));
  const span = Math.max(machine, runs * 4.5 + 8);
  return { pos: [span * 0.9, span * 0.85, span * 0.9], target: [0, 0, 0] };
}

/** Framing one run's platform: close enough that its blocks, plates and
    arcs read, from the front-right so the nameplate's end stays in view. */
export function focusFraming(platformLen: number, platformDep: number, z: number): Framing {
  const d = Math.max(9, Math.max(platformLen, platformDep) * 1.35 + 4);
  return { pos: [d * 0.62, d * 0.6, z + d * 0.62], target: [0, 0.8, z] };
}

/** A session arc's control-point lift above the higher endpoint: longer
    spans bow higher, bounded so a far host never leaves the frame. */
export function arcLift(distance: number): number {
  return Math.min(14, Math.max(2.5, distance * 0.32));
}

/** How long a just-completed session's arc stays on the plot. The frozen
    demo session runs hello→bye in under a second — inside one serial
    delivery frame — so the protocol's own window is never observed
    mid-flight; the arc dwells for one heartbeat beat after the bye lands,
    the same recency idiom the activity EMA gives the glow and pulse. */
export const ARC_DWELL_MS = 4000;

/**
 * Whether a session arc is on the plot, from the transcript's own facts:
 * - killed VM: never — the strike ends the wire with the VM;
 * - in flight (hello heard, no bye): while the VM runs;
 * - complete: while the dwell clock the view stamped at the bye's arrival
 *   runs — `doneAt` is null for a session read from history (a run already
 *   closed when the view first saw it), and history draws no arc.
 */
export function arcAlive(
  phase: 'active' | 'complete',
  vmState: 'running' | 'exited' | 'killed',
  doneAt: number | null,
  now: number,
): boolean {
  if (vmState === 'killed') return false;
  if (phase === 'active') return vmState === 'running';
  return doneAt != null && now - doneAt < ARC_DWELL_MS;
}
