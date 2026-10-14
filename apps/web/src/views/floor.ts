// The Floor: the Machines view and each machine's plaza, drawn by the Rust engine (engine-mount.ts: the same engine the
// desktop draws with). Without an engine in the build, or when it cannot start here (no WebGPU or WebGL2), the 2D floor
// (floor2d.ts) shows instead.
import '../styles/floor.css';
import { engineWanted, mountEngine } from './engine-mount';
import { mountFloor2d } from './floor2d';
import type { Cleanup } from './index';

export { agentHue, boardOf, roleHue } from './floor-common';
export { agentTally, gibText, gpuLines, machineFacts, machineSubtitle } from './floor-popup';

/** `#/floor` is the Machines view (the plaza when no fleet loads); `#/floor/<id>` is that machine's board. */
export function mount(root: HTMLElement, params: string[] = []): Cleanup {
  if (!engineWanted()) return mountFloor2d(root);
  let inner: Cleanup = () => undefined;
  inner = mountEngine(root, params[0] ?? null, () => {
    inner();
    inner = mountFloor2d(root);
  });
  return () => inner();
}
