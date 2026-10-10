/* The surveyor's-plan pen: the theme palette and mono fonts the canvases
   (map, telemetry) and the 3D scene read. Colours come from the CSS custom
   properties, re-read at every render, so a theme switch mid-session repaints
   every surface on the next store render — nothing is cached per theme. */

export interface Palette {
  paper: string; sheet: string; ink: string; ink2: string; ink3: string;
  rule: string; margin: string; signal: string; ok: string; crit: string; warn: string;
}

export const MONO = '11px "Martian Mono", ui-monospace, monospace';
export const MONO_SM = '9.5px "Martian Mono", ui-monospace, monospace';

export function palette(): Palette {
  const cs = getComputedStyle(document.documentElement);
  const g = (name: string): string => cs.getPropertyValue(name).trim();
  return {
    paper: g('--paper'), sheet: g('--sheet'), ink: g('--ink'), ink2: g('--ink-2'), ink3: g('--ink-3'),
    rule: g('--rule'), margin: g('--margin'), signal: g('--signal'),
    ok: g('--ok'), crit: g('--crit'), warn: g('--warn'),
  };
}
