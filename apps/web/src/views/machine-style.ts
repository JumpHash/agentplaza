// The machines' drawing style: Ink, Blueprint or Night. The owner chooses it once (the Style switch in the Machines header
// and in the machine view's); every engine that draws the machines uses it, in the console's light or dark theme.
import { h } from '../ui/dom';

export type MachineStyle = 'ink' | 'blueprint' | 'night';

/** The styles in the switch's order, with their labels. */
export const STYLES: readonly { style: MachineStyle; label: string }[] = [
  { style: 'ink', label: 'Ink' },
  { style: 'blueprint', label: 'Blueprint' },
  { style: 'night', label: 'Night' },
];

const STORAGE_KEY = 'apz.machines.style';

/** The style a stored value names; a missing or unknown value is Ink. */
export function parseStyle(raw: string | null): MachineStyle {
  return STYLES.find((s) => s.style === raw)?.style ?? 'ink';
}

// The style chosen on this page: it holds here even when the browser will not store it (a private window).
let chosen: MachineStyle | null = null;
const subscribers = new Set<(style: MachineStyle) => void>();

/** The owner's machine style. */
export function machineStyle(): MachineStyle {
  if (chosen !== null) return chosen;
  try {
    return parseStyle(localStorage.getItem(STORAGE_KEY));
  } catch {
    return 'ink';
  }
}

/** Stores the owner's machine style, then tells every subscriber. */
export function setMachineStyle(style: MachineStyle): void {
  chosen = style;
  try {
    localStorage.setItem(STORAGE_KEY, style);
  } catch {
    // not stored: the choice holds for this page
  }
  for (const fn of [...subscribers]) fn(style);
}

/** Calls `fn` with each new machine style; the returned function unsubscribes. */
export function onMachineStyle(fn: (style: MachineStyle) => void): () => void {
  subscribers.add(fn);
  return () => {
    subscribers.delete(fn);
  };
}

/** The console is dark unless the page says light. */
export function isDark(): boolean {
  return document.documentElement.dataset.theme !== 'light';
}

/** The key of the look a picture is drawn in: its style and the console's theme. */
export function styleKey(): string {
  return machineStyle() + '-' + (isDark() ? 'dark' : 'light');
}

/** The Ink | Blueprint | Night segmented control (the prototype's `.seg`). `dispose` unsubscribes it. */
export function styleSwitch(): { el: HTMLElement; dispose: () => void } {
  const buttons = STYLES.map(({ style, label }) => {
    const el = h('button', { type: 'button' }, label);
    el.addEventListener('click', () => setMachineStyle(style));
    return { style, el };
  });
  const paint = (current: MachineStyle): void => {
    for (const b of buttons) b.el.setAttribute('aria-pressed', String(b.style === current));
  };
  paint(machineStyle());
  const off = onMachineStyle(paint);
  const el = h('div.seg', { role: 'group', 'aria-label': 'Machine style' }, ...buttons.map((b) => b.el));
  return { el, dispose: off };
}
