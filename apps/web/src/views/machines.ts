// The Machines screen: the machines in two views. The Grid (floor.ts) is the fleet drawn in 3D and is the default; the
// List (machine.ts) is one row per machine, each opening into its live view. `#/machines?view=list` is the List, and
// `#/machines/<id>` is the List with that machine's row open. The switch changes the hash only, so the header stays.
import { button } from '../ui/components';
import { h } from '../ui/dom';
import { href, parse } from '../router';
import { mountGrid } from './floor';
import { styleSwitch } from './machine-style';
import { mountList } from './machine';
import { mountMachineView } from './machine-view';
import type { Cleanup } from './index';

export type MachinesView = 'grid' | 'list' | 'machine';

export interface MachinesRoute {
  view: MachinesView;
  /** The machine whose row the List opens, or null. */
  id: string | null;
}

/** The view a hash shows: the Grid by default, the List for `?view=list` or for a machine's id. */
export function machinesRoute(hash: string): MachinesRoute {
  const id = parse(hash).params[0] ?? null;
  const query = new URLSearchParams(hash.split('?')[1] ?? '');
  return { view: id !== null ? 'machine' : query.get('view') === 'list' ? 'list' : 'grid', id };
}

/** The hash of a view: the bare route is the Grid, the List carries `?view=list`. */
export function viewHash(view: MachinesView): string {
  return view === 'list' ? `${href('machines')}?view=list` : href('machines');
}

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  let view: MachinesView | null = null;
  let body: Cleanup = () => undefined;
  let reset: (() => void) | null = null;
  let shown: string | null = null; // the machine id the view shows

  const bodyBox = h('div.machines-body');
  const gridBtn = button('Grid', () => {
    location.hash = viewHash('grid');
  });
  const listBtn = button('List', () => {
    location.hash = viewHash('list');
  });
  const resetBtn = button('Reset view', () => reset?.(), { icon: 'refresh' });
  resetBtn.hidden = true;
  const switchBox = h('div.machines-switch', { role: 'group', 'aria-label': 'View' }, gridBtn, listBtn);
  const lookSwitch = styleSwitch(); // the machines' drawing style: the Grid, the List and the machine view draw in it
  const head = h('div.view-head', null, h('h1', null, 'Machines'), h('div.actions', null, switchBox, lookSwitch.el, resetBtn));
  root.append(head, bodyBox);

  // The Reset view button belongs to the Grid, and shows while the Grid has an engine to reset.
  const bindReset = (fn: (() => void) | null): void => {
    reset = fn;
    resetBtn.hidden = fn === null || view !== 'grid';
  };

  // Shows the body the hash asks for; the body it replaces releases its resources first.
  const paint = (): void => {
    const route = machinesRoute(location.hash);
    if (route.view === view && route.id === shown) return;
    view = route.view;
    shown = route.id;
    body();
    body = () => undefined;
    bodyBox.replaceChildren();
    reset = null;
    resetBtn.hidden = true;
    root.classList.toggle('floor-view', view === 'grid');
    head.hidden = view === 'machine';
    gridBtn.setAttribute('aria-pressed', String(view === 'grid'));
    gridBtn.classList.toggle('primary', view === 'grid');
    listBtn.setAttribute('aria-pressed', String(view === 'list'));
    listBtn.classList.toggle('primary', view === 'list');
    body = view === 'grid' ? mountGrid(bodyBox, bindReset) : route.id !== null ? mountMachineView(bodyBox, route.id) : mountList(bodyBox);
  };

  // The switch and the redirects change the hash within this screen; a different screen takes the page away.
  const onHash = (): void => {
    if (parse(location.hash).screen === 'machines') paint();
  };
  window.addEventListener('hashchange', onHash);
  paint();
  return () => {
    window.removeEventListener('hashchange', onHash);
    lookSwitch.dispose();
    body();
    body = () => undefined;
    root.classList.remove('floor-view');
  };
}
