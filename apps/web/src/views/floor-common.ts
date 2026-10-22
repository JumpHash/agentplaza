// The Floor's shared helper, with no 3D: an agent's hue. The 3D itself is the Rust engine
// (engine-mount.ts); the console's lists read this.

/** An agent's own colour: hues spread by the golden angle, so neighbours never match. */
export function agentHue(cid: number): number {
  return (cid * 137.508 + 20) % 360;
}
