// The Machines grid: the fleet drawn by the Rust engine (engine-mount.ts: the same engine the desktop draws with).
// When the engine is not in the build or cannot start here (no WebGPU or WebGL2), an empty state says so and offers the List.
import '../styles/floor.css';
import { engineWanted, mountEngine } from './engine-mount';
import { empty } from '../ui/components';
import { href } from '../router';
import type { Cleanup } from './index';

export { agentHue, roleHue } from './floor-common';
export { agentTally, gibText, gpuLines, machineFacts, machineSubtitle } from './floor-popup';

/** The Grid view of the Machines screen (machines.ts). `bindReset` gets the engine's reset, or null when there is none. */
export function mountGrid(root: HTMLElement, bindReset: (reset: (() => void) | null) => void): Cleanup {
  if (!engineWanted()) {
    bindReset(null);
    return showEmpty(root);
  }
  let inner: Cleanup = () => undefined;
  inner = mountEngine(
    root,
    () => {
      bindReset(null);
      inner();
      inner = showEmpty(root);
    },
    bindReset,
  );
  return () => {
    bindReset(null);
    inner();
  };
}

function showEmpty(root: HTMLElement): Cleanup {
  const link = document.createElement('a');
  link.href = `${href('machines')}?view=list`;
  link.textContent = 'Show the list';
  root.replaceChildren(
    empty(
      'floor',
      '3D needs WebGPU or WebGL2',
      'This browser cannot draw the machines in 3D. Their list is one click away.',
      link,
    ),
  );
  return () => root.replaceChildren();
}
