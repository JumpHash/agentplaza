// The Machines screen: the machines in two views. The Grid (floor.ts) is the fleet drawn in 3D and is the default; the
// List (machine.ts) is one row per machine, each opening into its live view. `#/machines?view=list` is the List, and
// `#/machines/<id>` is the List with that machine's row open. The switch changes the hash only, so the header stays.
// The markup is the prototype's: the page head with the 3D | List switch, the 3D box with its reset control.
import '../styles/proto/machines.css';
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
  const threeD = h('button', { type: 'button' }, '3D');
  threeD.addEventListener('click', () => {
    location.hash = viewHash('grid');
  });
  const listBtn = h('button', { type: 'button' }, 'List');
  listBtn.addEventListener('click', () => {
    location.hash = viewHash('list');
  });
  const resetBtn = button('Reset', () => reset?.(), { icon: 'refresh', size: 'sm' });
  resetBtn.hidden = true;
  // The Reset control sits over the 3D box, bottom right (the prototype's `.f3d-ctl`); it shows while the Grid has an engine.
  const ctl = h('div.f3d-ctl', null, resetBtn);
  ctl.hidden = true;
  const switchBox = h('div.seg.mt-views', { role: 'group', 'aria-label': 'View' }, threeD, listBtn);
  const lookSwitch = styleSwitch(); // the machines' drawing style: the Grid, the List and the machine view draw in it
  const head = h('div.view-head.mch-head', null, h('div.mch-head-l', null, h('h1', null, 'Machines')), h('div.mt-actions', null, switchBox, lookSwitch.el));
  root.append(head, bodyBox);

  // The Reset control belongs to the Grid, and shows while the Grid has an engine to reset.
  const bindReset = (fn: (() => void) | null): void => {
    reset = fn;
    const on = fn !== null && view === 'grid';
    resetBtn.hidden = !on;
    ctl.hidden = !on;
  };

  // The Grid: the 3D box holds the engine's stage; its reset control sits over it.
  const mountFleet = (): Cleanup => {
    const frame = h('div.f3d.f3d-fleet');
    bodyBox.append(frame);
    const stop = mountGrid(frame, bindReset);
    frame.append(ctl);
    return () => {
      stop();
      frame.remove();
    };
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
    ctl.hidden = true;
    root.classList.toggle('floor-view', view === 'grid');
    head.hidden = view === 'machine';
    threeD.setAttribute('aria-pressed', String(view === 'grid'));
    listBtn.setAttribute('aria-pressed', String(view === 'list'));
    body = view === 'grid' ? mountFleet() : route.id !== null ? mountMachineView(bodyBox, route.id) : mountList(bodyBox);
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
