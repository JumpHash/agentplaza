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
    (reason) => {
      console.warn('the 3D engine did not start:', reason);
      bindReset(null);
      inner();
      inner = showEmpty(root, reason);
    },
    bindReset,
  );
  return () => {
    bindReset(null);
    inner();
  };
}

function showEmpty(root: HTMLElement, reason?: string): Cleanup {
  const link = document.createElement('a');
  link.href = `${href('machines')}?view=list`;
  link.textContent = 'Show the list';
  // Only a missing adapter means the browser cannot draw 3D; anything else is the engine's own failure, said as it is.
  const noGpu = reason === undefined || /adapter|WebGPU|WebGL/i.test(reason);
  root.replaceChildren(
    noGpu
      ? empty('floor', '3D needs WebGPU or WebGL2', 'This browser cannot draw the machines in 3D. Their list is one click away.', link)
      : empty('floor', 'The 3D view did not start', `${reason}. Their list is one click away.`, link),
  );
  return () => root.replaceChildren();
}
