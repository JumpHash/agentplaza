// The Floor's shared helpers, with no 3D: the agents' and roles' hues. The 3D itself is the Rust engine
// (engine-mount.ts); the console's lists read these.

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
