/* Hash routing: #register (default) / #run/{id} / #machine / #map / #view3d /
   #compare/{a}/{avm}/{b}/{bvm} / #telemetry / #roster / #ledger/{id} /
   #treasury / #market. Keys 0–9 walk the ten index destinations; the compare
   and ledger addresses carry their subject so they can be bookmarked. */

import type { CompareTarget, Route, ViewName } from './store';

const NAMES: Readonly<Record<string, ViewName>> = {
  register: 'register',
  run: 'run',
  machine: 'machine',
  map: 'map',
  view3d: 'view3d',
  compare: 'compare',
  telemetry: 'telemetry',
  roster: 'roster',
  ledger: 'ledger',
  treasury: 'treasury',
  market: 'market',
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
  const param = name === 'run' || name === 'ledger' ? (parts[1] ?? null) : null;
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
  roster: 'roster',
  ledger: 'ledger',
  treasury: 'treasury',
  market: 'market',
};

/** Key destinations for 0–9. `run` resolves against what the store knows. */
export const KEY_ROUTES: Readonly<Record<string, ViewName>> = {
  '0': 'register',
  '1': 'run',
  '2': 'machine',
  '3': 'map',
  '4': 'view3d',
  '5': 'telemetry',
  '6': 'ledger',
  '7': 'roster',
  '8': 'treasury',
  '9': 'market',
};
