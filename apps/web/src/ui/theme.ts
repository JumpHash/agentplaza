// The console's theme: dark (the default) or the light register, chosen with the switch in the
// sidebar's foot and remembered in localStorage. It is set on the root before the first paint.

import { h, icon, replace } from './dom';

export type Theme = 'dark' | 'light';

const KEY = 'apz.theme';

export function currentTheme(): Theme {
  return document.documentElement.dataset.theme === 'light' ? 'light' : 'dark';
}

function stored(): Theme {
  try {
    return localStorage.getItem(KEY) === 'light' ? 'light' : 'dark';
  } catch {
    return 'dark';
  }
}

export function applyTheme(theme: Theme): void {
  document.documentElement.dataset.theme = theme;
  try {
    localStorage.setItem(KEY, theme);
  } catch {
    // Storage blocked: the choice lasts until the page reloads.
  }
}

/** The switch: shows the theme it changes to. */
export function themeSwitch(): HTMLButtonElement {
  const btn = h('button.btn.ghost.sm.theme-switch', { type: 'button' });
  const paint = (): void => {
    const next = currentTheme() === 'dark' ? 'light' : 'dark';
    btn.title = `Switch to the ${next} theme`;
    btn.setAttribute('aria-label', btn.title);
    replace(btn, icon(next === 'light' ? 'sun' : 'moon'), h('span.lbl', null, next === 'light' ? 'Light theme' : 'Dark theme'));
  };
  btn.addEventListener('click', () => {
    applyTheme(currentTheme() === 'dark' ? 'light' : 'dark');
    paint();
  });
  paint();
  return btn;
}

// Before the first paint.
document.documentElement.dataset.theme = stored();
