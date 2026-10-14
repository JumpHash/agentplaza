// The Floor's shared helpers, with no 3D: the agents' and roles' hues, and the board a machine's plaza is built on. The
// 3D itself is the Rust engine (engine-mount.ts); the 2D floor (floor2d.ts) and the console's lists read these.
import type { FleetApple } from '../api';

/** The district hues: a curated cartoon set (coral, tangerine, butter, lime, mint, teal, sky, periwinkle, lavender, rose). */
const HUES = [8, 28, 46, 88, 140, 172, 198, 222, 262, 340];

/** An agent's own colour: hues spread by the golden angle, so neighbours never match. */
export function agentHue(cid: number): number {
  return (cid * 137.508 + 20) % 360;
}

/** A role's district hue, from the curated set by the role's hash. */
export function roleHue(role: string): number {
  let hash = 0;
  for (const ch of role) hash = (hash * 31 + ch.charCodeAt(0)) >>> 0;
  return HUES[hash % HUES.length]!;
}

/** A board model's numeric params (`gpus`, `studio`, the Apple chip's cores and memory). */
export type BoardParams = Record<string, number>;

/** The board model a machine's kind and vendor name: Apple machines share the Apple SoC board (hardware.rs models_for). */
export function boardModel(machine: { kind: string; vendor?: string | null }): string {
  return machine.vendor === 'apple' ? 'board/apple-soc' : `board/${machine.kind}`;
}

/** The board a machine's plaza is built on, and the params its model takes (as the Machines view builds them). */
export function boardOf(m: { kind: string; vendor?: string | null; model?: string | null; apple?: FleetApple | null; gpus: unknown[] }): { name: string; params: BoardParams } {
  const params: BoardParams = { gpus: m.gpus.length, studio: m.model?.includes('Studio') ? 1 : 0 };
  if (m.apple) {
    params.p_cores = m.apple.p_cores;
    params.e_cores = m.apple.e_cores;
    params.gpu_cores = m.apple.gpu_cores;
    params.memory_gib = m.apple.unified_memory_gib;
  }
  return { name: boardModel(m), params };
}
