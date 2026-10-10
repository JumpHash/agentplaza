/* Hash routing: #register (default) / #run/{id} / #machine / #map / #view3d /
   #compare/{a}/{avm}/{b}/{bvm} / #telemetry. Keys 0–5 walk the six index
   destinations; the compare address carries its pair so it can be bookmarked. */

import type { CompareTarget, Route, ViewName } from './store';

const NAMES: Readonly<Record<string, ViewName>> = {
  register: 'register',
  run: 'run',
  machine: 'machine',
  map: 'map',
  view3d: 'view3d',
  compare: 'compare',
  telemetry: 'telemetry',
};

export function parseRoute(hash: string): Route {
  const parts = hash.slice(1).split('/');
  const name = NAMES[parts[0] ?? ''] ?? 'register';
  if (name === 'compare') {
    const a = parts[1] ?? '';
    const avm = Number(parts[2]);
    const b = parts[3] ?? '';
    const bvm = Number(parts[4]);
    const ok =
      a !== '' && b !== '' &&
      Number.isInteger(avm) && avm >= 0 && String(avm) === parts[2] &&
      Number.isInteger(bvm) && bvm >= 0 && String(bvm) === parts[4];
    const compare: CompareTarget | null = ok ? { a, avm, b, bvm } : null;
    return { name, param: null, compare };
  }
  const param = name === 'run' ? (parts[1] ?? null) : null;
  return { name, param, compare: null };
}

/** The nav section a view belongs to (run detail and compare highlight "Run"). */
export const NAV_OF: Record<ViewName, ViewName> = {
  register: 'register',
  run: 'run',
  machine: 'machine',
  map: 'map',
  view3d: 'view3d',
  compare: 'run',
  telemetry: 'telemetry',
};

/** Key destinations for 0–5. `run` resolves against what the store knows. */
export const KEY_ROUTES: Readonly<Record<string, ViewName>> = {
  '0': 'register',
  '1': 'run',
  '2': 'machine',
  '3': 'map',
  '4': 'view3d',
  '5': 'telemetry',
};
