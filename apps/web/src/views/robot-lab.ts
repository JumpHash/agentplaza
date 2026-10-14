// The robot lab: a developer screen for the robot species under assets/robots/. The wasm engine draws it on
// its canvas: one species (or a line-up of all of them) under the Floor's lighting, with any clip, tool,
// carried item and state.
// Route: #/robot-lab?species=rover&clip=type&tool=wrench&carry=crate&state=working&hue=200&orch=1&cam=iso
//   species: a species id, or all (the line-up). clip: a shared or species clip (empty: the state's own).
//   tool: a tool id for the grip; carry: an item id for the carry bone. state: idle | working | walking | offline.
//   hue: 0–360. orch=1: the rover as the orchestrator. cam: front | iso | side | q34 (a 3/4 view) | top.
//   t: seconds, a frozen clock (each robot takes its pose at that time). bg: dark | light (the scene background).
// The engine validates these options itself: an unknown value falls back to the engine's default.
// A control change writes the route and restarts the engine with the new options. The engine has no option for
// the prop screen or the effects, so the lab has no controls for them.

import { button, field } from '../ui/components';
import { h } from '../ui/dom';
import { mountLabEngine, startLabEngine, type LabOptions } from './engine-mount';
import type { Cleanup } from './index';
import { ROBOT_SPECIES, ROBOT_TOOLS, robotCarryTools, robotClipOptions } from './lab-assets';

const STATES = ['idle', 'working', 'walking', 'offline'];
const CAMS = ['front', 'iso', 'side', 'q34', 'top'];

interface LabState {
  species: string;
  clip: string;
  tool: string;
  carry: string;
  state: string;
  hue: number;
  orch: boolean;
  cam: string;
  /** A frozen clock, in seconds; undefined for the live clock. */
  t: number | undefined;
  /** The scene background (dark | light), or '' for the engine's default. */
  bg: string;
}

function readState(hash: string): LabState {
  const q = new URLSearchParams(hash.split('?')[1] ?? '');
  const state = q.get('state') ?? '';
  const cam = q.get('cam') ?? '';
  const hue = Number(q.get('hue') ?? 200);
  const tRaw = q.get('t') ?? '';
  return {
    species: q.get('species') ?? 'all',
    clip: q.get('clip') ?? '',
    tool: q.get('tool') ?? '',
    carry: q.get('carry') ?? '',
    state: STATES.includes(state) ? state : 'idle',
    hue: Number.isFinite(hue) ? ((hue % 360) + 360) % 360 : 200,
    orch: q.get('orch') === '1',
    cam: CAMS.includes(cam) ? cam : 'front',
    t: tRaw !== '' && Number.isFinite(Number(tRaw)) ? Math.max(0, Number(tRaw)) : undefined,
    bg: q.get('bg') ?? '',
  };
}

function writeRoute(s: LabState): void {
  const q = new URLSearchParams();
  q.set('species', s.species);
  if (s.clip) q.set('clip', s.clip);
  if (s.tool) q.set('tool', s.tool);
  if (s.carry) q.set('carry', s.carry);
  q.set('state', s.state);
  q.set('hue', String(Math.round(s.hue)));
  if (s.orch) q.set('orch', '1');
  q.set('cam', s.cam);
  if (s.t !== undefined) q.set('t', String(s.t));
  if (s.bg) q.set('bg', s.bg);
  history.replaceState(null, '', `#/robot-lab?${q.toString()}`);
}

function toOptions(s: LabState): LabOptions {
  return { species: s.species, clip: s.clip, tool: s.tool, carry: s.carry, state: s.state, hue: s.hue, orch: s.orch, cam: s.cam, t: s.t, bg: s.bg };
}

/** A list with the current value in it when the list does not name it (a route can name anything; the engine decides). */
const withCurrent = (list: string[], value: string): string[] => (value === '' || list.includes(value) ? list : [value, ...list]);

/** The options of a select: `none`, when given, is the empty value's label; the list's own values follow. */
function choices(values: string[], none?: string): HTMLOptionElement[] {
  return [...(none === undefined ? [] : [h('option', { value: '' }, none)]), ...values.map((v) => h('option', { value: v }, v))];
}

/** The lab: the controls over the engine's canvas. The route is read once, when the lab mounts. */
export function mount(root: HTMLElement): Cleanup {
  const s = readState(location.hash);
  const bar = 'display:flex;flex-wrap:wrap;gap:10px 16px;align-items:center;padding:8px 0';
  const stats = h('div', { style: 'font-family:var(--mono);font-size:12px;color:var(--ink-2);padding:4px 0', 'aria-live': 'polite' });

  const apply = (): void => {
    writeRoute(s);
    host.restart(toOptions(s));
    paint();
  };
  const paint = (): void => {
    camBtns.forEach((b, i) => (b.style.outline = CAMS[i] === s.cam ? '2px solid var(--accent)' : ''));
  };
  const clipChoices = (): HTMLOptionElement[] => choices(withCurrent(robotClipOptions(s.species), s.clip), '(state clip)');

  const speciesSel = h(
    'select.select',
    {
      onchange: () => {
        s.species = speciesSel.value;
        s.clip = '';
        clipSel.replaceChildren(...clipChoices());
        apply();
      },
    },
    ...choices(withCurrent(['all', ...ROBOT_SPECIES], s.species)),
  );
  speciesSel.value = s.species;
  const clipSel = h('select.select', { onchange: () => { s.clip = clipSel.value; apply(); } }, ...clipChoices());
  clipSel.value = s.clip;
  const toolSel = h('select.select', { onchange: () => { s.tool = toolSel.value; apply(); } }, ...choices(withCurrent(ROBOT_TOOLS, s.tool), '(no tool)'));
  toolSel.value = s.tool;
  const carrySel = h('select.select', { onchange: () => { s.carry = carrySel.value; apply(); } }, ...choices(withCurrent(robotCarryTools(), s.carry), '(nothing)'));
  carrySel.value = s.carry;
  const stateSel = h('select.select', { onchange: () => { s.state = stateSel.value; apply(); } }, ...choices(STATES));
  stateSel.value = s.state;
  // The hue applies when the slider is let go: a restart per pixel of drag would start the engine many times.
  const hueIn = h('input', { type: 'range', min: 0, max: 360, step: 1, value: String(s.hue), onchange: () => { s.hue = Number(hueIn.value); apply(); } });
  const orchIn = h('input', { type: 'checkbox', onchange: () => { s.orch = orchIn.checked; apply(); } });
  orchIn.checked = s.orch;
  const camBtns = CAMS.map((c) => button(c.toUpperCase(), () => { s.cam = c; apply(); }));

  const panel = h(
    'div',
    { style: 'padding:12px 16px' },
    h(
      'div',
      { style: bar },
      field('Species', speciesSel),
      field('Clip', clipSel),
      field('Tool', toolSel),
      field('Carry', carrySel),
      field('State', stateSel),
      field('Hue', hueIn),
      field('Orchestrator', orchIn),
      ...camBtns,
    ),
    stats,
  );
  root.append(panel);
  paint();

  const host = mountLabEngine(root, startLabEngine, toOptions(s));
  const tick = window.setInterval(() => {
    const live = host.engine();
    stats.textContent = live ? `draw calls (last frame): ${live.draw_calls()}` : '';
  }, 500);

  return () => {
    window.clearInterval(tick);
    host.close();
    panel.remove();
  };
}
