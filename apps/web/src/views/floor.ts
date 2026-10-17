// The Floor: the Machines view, drawn by the Rust engine (engine-mount.ts: the same engine the desktop draws with).
// When the engine is not in the build or cannot start here (no WebGPU or WebGL2), an empty state says so.
import '../styles/floor.css';
import { engineWanted, mountEngine } from './engine-mount';
import { empty } from '../ui/components';
import type { Cleanup } from './index';

export { agentHue, roleHue } from './floor-common';
export { agentTally, gibText, gpuLines, machineFacts, machineSubtitle } from './floor-popup';

/** `#/floor` is the Machines view (the plaza when no fleet loads). */
export function mount(root: HTMLElement): Cleanup {
  if (!engineWanted()) return showEmpty(root);
  let inner: Cleanup = () => undefined;
  inner = mountEngine(root, () => {
    inner();
    inner = showEmpty(root);
  });
  return () => inner();
}

function showEmpty(root: HTMLElement): Cleanup {
  const link = document.createElement('a');
  link.href = '#/machine';
  link.textContent = 'Go to Machines';
  root.replaceChildren(
    empty(
      'floor',
      '3D needs WebGPU or WebGL2',
      'This browser cannot draw the machines in 3D. Their list is under Setup › Machines.',
      link,
    ),
  );
  return () => root.replaceChildren();
}
