// Clip evaluation: a channel's value at time t. Both renderers read clips.json the same way: value =
// base + amp × wave(2π·freq·t + phase), with `gait.<name>` strings resolved from the species' gait.

import type { Channel, Clip, Gait, Wave } from './types';

/** The wave at a cycle position: `c` in cycles (c = freq·t + phase/2π). `duty` is for square and pulse. */
export function waveAt(wave: Wave, c: number, duty = 0.5): number {
  const f = c - Math.floor(c);
  switch (wave) {
    case 'sin': return Math.sin(2 * Math.PI * c);
    case 'abs': return Math.abs(Math.sin(2 * Math.PI * c));
    case 'halfsin': return Math.max(0, Math.sin(2 * Math.PI * c));
    case 'tri': return 1 - 4 * Math.abs(f - 0.5);
    case 'saw': return 2 * f - 1;
    case 'square': return f < duty ? 1 : -1;
    case 'pulse': return f < duty ? 1 : 0;
    case 'hold': return 0;
  }
}

/** A number field, or the species' gait value for "gait.<name>". */
export function resolve(v: number | string | undefined, gait: Gait, fallback: number): number {
  if (v === undefined) return fallback;
  if (typeof v === 'number') return v;
  const k = v.slice(5) as keyof Gait;
  const g = gait[k];
  return typeof g === 'number' ? g : fallback;
}

/**
 * A channel's value at the global time `t` (seconds), and `elapsed` into a one-shot of length `dur`
 * (for the `in-out` envelope). Clips are periodic in the global time, so a blink lands where it always did.
 */
export function channelValue(ch: Channel, t: number, gait: Gait, elapsed = 0, dur = 0): number {
  const base = resolve(ch.base, gait, 0);
  const amp = resolve(ch.amp, gait, 0);
  const freq = resolve(ch.freq, gait, 0);
  let env = 1;
  if (ch.env === 'in-out' && dur > 0) env = Math.sin(Math.PI * Math.min(1, Math.max(0, elapsed / dur)));
  const w = waveAt(ch.wave ?? 'sin', freq * t + (ch.phase ?? 0) / (2 * Math.PI), ch.duty);
  return base + amp * env * w;
}

/** The rest value of a prop: rotations and offsets at 0, a scale at 1, the eye lids open (1). */
export function restOf(prop: string): number {
  return prop === 'scale' || prop === 'lid' ? 1 : 0;
}

/** All the channels of a clip evaluated at `t`: key "bone|prop" → value (rate channels are skipped). */
export function evalClip(clip: Clip, t: number, gait: Gait, elapsed = 0): Map<string, { value: number; snap: boolean }> {
  const out = new Map<string, { value: number; snap: boolean }>();
  for (const ch of clip.channels) {
    if (ch.rate !== undefined) continue;
    const key = ch.prop === 'lid' ? 'lid' : `${ch.bone}|${ch.prop}`;
    out.set(key, { value: channelValue(ch, t, gait, elapsed, clip.dur ?? 0), snap: ch.snap === true });
  }
  return out;
}
