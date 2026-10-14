// The first-run wizard and the tour, as pure logic: the steps, the shared copy
// (apps/shared/onboarding.json, the same words as the desktop app), the progress
// flags in localStorage. The DOM lives in ui/onboarding.ts.

import copy from '../../../shared/onboarding.json';

export const COPY = copy;

/** The wizard's steps, in order (at most five, one screen each). */
export const WIZARD_STEPS = copy.wizard.steps;

/** The tour's steps, in order: each is a callout next to a sidebar screen. */
export const TOUR_STEPS = copy.tour.steps;

/** localStorage keys for the two flags. The desktop app keeps the same flags under `setup_done` and `tour_done`. */
export const FLAGS = { setup: 'apz.setup.done', tour: 'apz.tour.done' } as const;
export type Flag = keyof typeof FLAGS;

/** The step index after moving by `delta`, kept inside the steps. */
export function moveStep(index: number, delta: number, count: number): number {
  return Math.min(count - 1, Math.max(0, index + delta));
}

/** Fills `{name}` placeholders of a copy string. */
export function fill(template: string, values: Record<string, string | number>): string {
  return template.replace(/\{(\w+)\}/g, (whole, name: string) => (name in values ? String(values[name]) : whole));
}

/** Whether a flag is set. Storage that refuses (private windows, blocked site data) reads as not set. */
export function readFlag(store: Pick<Storage, 'getItem'> | null, flag: Flag): boolean {
  try {
    return store?.getItem(FLAGS[flag]) === '1';
  } catch {
    return false;
  }
}

/** Sets a flag. A refused write is not an error: the flag then simply is not remembered. */
export function writeFlag(store: Pick<Storage, 'setItem'> | null, flag: Flag): void {
  try {
    store?.setItem(FLAGS[flag], '1');
  } catch {
    // not persisted; this page still knows
  }
}

/** The browser's localStorage, or null when the browser refuses to give it. */
export function browserStore(): Storage | null {
  try {
    return localStorage;
  } catch {
    return null;
  }
}

/** Providers with a key: every provider whose credential is not missing. */
export function keysSet(providers: { credential: string }[]): { n: number; m: number } {
  return { n: providers.filter((p) => p.credential !== 'missing').length, m: providers.length };
}
