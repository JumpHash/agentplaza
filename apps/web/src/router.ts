/* Hash routing: #register (default) / #run/{id} / #machine / #map / #view3d.
   Keys 0–4 walk the same five destinations. */

import type { Route, ViewName } from './store';

const NAMES: Readonly<Record<string, ViewName>> = {
  register: 'register',
  run: 'run',
  machine: 'machine',
  map: 'map',
  view3d: 'view3d',
};

export function parseRoute(hash: string): Route {
  const parts = hash.slice(1).split('/');
  const name = NAMES[parts[0] ?? ''] ?? 'register';
  const param = name === 'run' ? (parts[1] ?? null) : null;
  return { name, param };
}

/** The nav section a view belongs to (run detail highlights "Run"). */
export const NAV_OF: Record<ViewName, ViewName> = {
  register: 'register',
  run: 'run',
  machine: 'machine',
  map: 'map',
  view3d: 'view3d',
};

/** Key destinations for 0–4. `run` resolves against what the store knows. */
export const KEY_ROUTES: Readonly<Record<string, ViewName>> = {
  '0': 'register',
  '1': 'run',
  '2': 'machine',
  '3': 'map',
  '4': 'view3d',
};
