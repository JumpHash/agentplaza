/* The keyboard sheet: `?` toggles it, Escape (or `?` again, or a click on the
   backdrop, or the Close button) dismisses it. It is a real dialog —
   aria-modal, focus moved inside on open and returned on close, Tab trapped
   within while open. The shortcut list itself is static markup in index.html;
   this module is only the open/close discipline. */

import { byId } from '../dom';

let open = false;
let restoreFocus: HTMLElement | null = null;

export function keysOpen(): boolean {
  return open;
}

function focusables(sheet: HTMLElement): HTMLElement[] {
  return [...sheet.querySelectorAll<HTMLElement>('button, a[href], [tabindex]')]
    .filter((n) => !n.hasAttribute('disabled') && n.tabIndex >= 0);
}

/* capture phase, so the sheet's keys never reach the global bindings below */
function onKey(e: KeyboardEvent): void {
  if (e.key === 'Escape' || e.key === '?') {
    e.preventDefault();
    e.stopPropagation();
    toggleKeys(false);
    return;
  }
  if (e.key !== 'Tab') return;
  const sheet = byId('keys-sheet');
  const f = focusables(sheet);
  if (f.length === 0) {
    e.preventDefault();
    return;
  }
  const first = f[0];
  const last = f[f.length - 1];
  const active = document.activeElement;
  if (e.shiftKey && (active === first || !sheet.contains(active))) {
    e.preventDefault();
    last?.focus();
  } else if (!e.shiftKey && (active === last || !sheet.contains(active))) {
    e.preventDefault();
    first?.focus();
  }
}

export function toggleKeys(force?: boolean): void {
  const want = force ?? !open;
  if (want === open) return;
  open = want;
  const wrap = byId('keys');
  wrap.hidden = !open;
  if (open) {
    restoreFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    byId('keys-sheet').focus();
    document.addEventListener('keydown', onKey, true);
  } else {
    document.removeEventListener('keydown', onKey, true);
    restoreFocus?.focus();
    restoreFocus = null;
  }
}

export function initKeys(): void {
  byId('keys-close').addEventListener('click', () => toggleKeys(false));
  /* a click on the backdrop (not the sheet) closes */
  byId('keys').addEventListener('mousedown', (e) => {
    if (e.target === byId('keys')) toggleKeys(false);
  });
  /* the colophon's standing invitation */
  byId('colo-keys').addEventListener('click', () => toggleKeys(true));
}
