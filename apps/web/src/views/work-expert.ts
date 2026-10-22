// The work engine's views share one Expert switch: a checkbox kept in this browser under each view's own key, off until
// it is turned on. The dense detail (ids, micro-dollars, the tokens split, nodes, CIDs) shows only while it is on.

import { h } from '../ui/dom';

/** Whether a view's Expert switch was left on; off when nothing was kept, or when storage is refused. */
export function readExpert(key: string): boolean {
  try {
    return localStorage.getItem(key) === '1';
  } catch {
    return false;
  }
}

/** Keeps a view's Expert switch. A private window keeps no storage: the switch then lasts this visit. */
export function writeExpert(key: string, value: boolean): void {
  try {
    localStorage.setItem(key, value ? '1' : '0');
  } catch {
    // No storage: the switch still works for this visit.
  }
}

/** A checkbox with its label, for a view's switches (Expert, Archived); `change` gets the new state. */
export function switchBox(label: string, checked: boolean, change: (checked: boolean) => void): HTMLElement {
  return h(
    'label.check',
    null,
    h('input', { type: 'checkbox', checked, onchange: (ev: Event) => change((ev.target as HTMLInputElement).checked) }),
    h('span', null, label),
  );
}
