// The robot lab: a developer screen for the robot species under assets/robots/. The wasm engine draws it on
// its canvas: one species (or a line-up of all of them) under the Floor's lighting, with any clip, tool,
// carried item and state.
// Route: #/robot-lab?species=rover&clip=type&tool=wrench&carry=crate&state=working&hue=200&orch=1&cam=iso
//   species: a species id, or all (the line-up). clip: a shared or species clip (empty: the state's own).
//   tool: a tool id for the grip; carry: an item id for the carry bone. state: idle | working | walking | offline.
//   hue: 0–360. orch=1: the rover as the orchestrator. cam: front | iso | side | q34 (a 3/4 view) | top.
//   t: seconds, a frozen clock (each robot takes its pose at that time). bg: dark | light (the scene background).
// The engine validates these options itself: an unknown value falls back to the engine's default.

import { h } from '../ui/dom';
import { startLabEngine, type LabOptions } from './engine-mount';
import type { Cleanup } from './index';

/** The lab on the wasm engine, its canvas filling the root. The route's options are read once. */
export function mount(root: HTMLElement): Cleanup {
  const route = new URLSearchParams(location.hash.split('?')[1] ?? '');
  const canvas = document.createElement('canvas');
  canvas.style.cssText = 'display:block;width:100%;height:70vh;min-height:420px;touch-action:none';
  root.append(canvas);
  const tRaw = route.get('t') ?? '';
  const lab: LabOptions = {
    species: route.get('species') ?? 'all',
    clip: route.get('clip') ?? '',
    tool: route.get('tool') ?? '',
    carry: route.get('carry') ?? '',
    state: route.get('state') ?? 'idle',
    hue: Number(route.get('hue') ?? 200),
    orch: route.get('orch') === '1',
    cam: route.get('cam') ?? 'front',
    t: tRaw !== '' && Number.isFinite(Number(tRaw)) ? Math.max(0, Number(tRaw)) : undefined,
    bg: route.get('bg') ?? '',
  };
  let engine: { destroy: () => void } | null = null;
  let closed = false;
  startLabEngine(canvas, lab).then(
    (started) => {
      if (closed) started.destroy();
      else engine = started;
    },
    (err: unknown) => {
      const msg = err instanceof Error ? err.message : String(err);
      canvas.replaceWith(h('p', { style: 'color:var(--ink-2)' }, `The engine did not start: ${msg}`));
    },
  );
  return () => {
    closed = true;
    engine?.destroy();
    canvas.remove();
  };
}
