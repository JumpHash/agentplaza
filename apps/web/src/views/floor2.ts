// The Floor: districts of robots on an isometric plaza.

import '../styles/floor.css';

import { api, type Agent, type Conversation, type Entry } from '../api';
import { go } from '../router';
import { agents, on } from '../store';
import { attempt, button, clip, input } from '../ui/components';
import { h } from '../ui/dom';
import type { Cleanup } from './index';

const TW = 64;
const TH = 32;
const BUBBLE_MS = 4000;
const PACKET_MS = 1500;

interface District {
  role: string;
  cx: number; // centre, grid units
  cy: number;
  r: number; // half-size, grid units
  hue: number;
}

interface Bot {
  agent: Agent;
  district: District;
  home: [number, number]; // console position
  x: number; // current position, grid units
  y: number;
  tx: number; // walk target
  ty: number;
  facing: number; // -1 left, 1 right
  walk: number; // walk phase
  nextWander: number;
  bubble: { text: string; until: number; tone: 'info' | 'ok' | 'danger' } | null;
}

interface Packet { from: number; to: number; start: number; tone: 'accent' | 'ok' }

export function roleHue(role: string): number {
  let hash = 0;
  for (const ch of role) hash = (hash * 31 + ch.charCodeAt(0)) >>> 0;
  return hash % 360;
}

/** World (grid) → screen px, before pan and zoom. */
function iso(x: number, y: number, z = 0): [number, number] {
  return [(x - y) * (TW / 2), (x + y) * (TH / 2) - z];
}

/** Districts by role around a central dais; each agent gets a console slot. */
export function layoutDistricts(list: Agent[]): { districts: District[]; homes: Map<number, [District, number, number]> } {
  const groups = new Map<string, Agent[]>();
  for (const a of list) {
    const key = a.tier === 'orchestrator' ? '' : a.role;
    groups.set(key, [...(groups.get(key) ?? []), a]);
  }
  const districts: District[] = [];
  const homes = new Map<number, [District, number, number]>();
  const place = (d: District, members: Agent[]) => {
    districts.push(d);
    const side = Math.max(1, Math.ceil(Math.sqrt(members.length)));
    members.sort((a, b) => a.cid - b.cid).forEach((a, i) => {
      const gx = d.cx - (side - 1) + (i % side) * 2;
      const gy = d.cy - (side - 1) + Math.floor(i / side) * 2;
      homes.set(a.cid, [d, gx, gy]);
    });
  };
  const centre = groups.get('');
  if (centre) place({ role: 'orchestrator', cx: 0, cy: 0, r: 2, hue: 25 }, centre);
  const roles = [...groups.keys()].filter((k) => k).sort();
  const ringR = 6 + Math.floor(roles.length / 6) * 2;
  roles.forEach((role, i) => {
    const members = groups.get(role)!;
    const side = Math.max(1, Math.ceil(Math.sqrt(members.length)));
    const angle = (i / roles.length) * Math.PI * 2 - Math.PI / 2;
    place(
      { role, cx: Math.round(Math.cos(angle) * ringR), cy: Math.round(Math.sin(angle) * ringR), r: side + 1, hue: roleHue(role) },
      members,
    );
  });
  return { districts, homes };
}

export function mount(root: HTMLElement): Cleanup {
  const canvas = h('canvas.floor-canvas', { 'aria-label': 'The plaza floor' });
  const card = h('div.floor-card', { hidden: true });
  const legend = h(
    'div.floor-legend',
    null,
    h('span', null, h('i.floor-key.working'), 'Working'),
    h('span', null, h('i.floor-key.idle'), 'Idle'),
    h('span', null, h('i.floor-key.offline'), 'Offline'),
  );
  const stage = h('div.floor-stage', null, canvas, card, legend);
  root.classList.add('floor-view');
  root.append(h('div.view-head', null, h('h1', null, 'Floor'), h('div.actions', null, button('Reset view', () => fit(), { kind: 'ghost', icon: 'refresh' }))), stage);

  const ctx = canvas.getContext('2d')!;
  let districts: District[] = [];
  let bots: Bot[] = [];
  const byCid = new Map<number, Bot>();
  const packets: Packet[] = [];
  let pan = { x: 0, y: 0 };
  let zoom = 1;
  let dpr = 1;
  let hovered: Bot | null = null;
  let selected: Bot | null = null;
  let running = true;
  let frame = 0;
  let last = performance.now();
  let palette = readPalette();

  function sync(list: Agent[]): void {
    const laid = layoutDistricts(list);
    districts = laid.districts;
    const next: Bot[] = [];
    for (const agent of list) {
      const slot = laid.homes.get(agent.cid);
      if (!slot) continue;
      const [district, hx, hy] = slot;
      const old = byCid.get(agent.cid);
      next.push(
        old
          ? Object.assign(old, { agent, district, home: [hx, hy] as [number, number] })
          : { agent, district, home: [hx, hy], x: hx, y: hy, tx: hx, ty: hy, facing: 1, walk: 0, nextWander: performance.now() + Math.random() * 3000, bubble: null },
      );
    }
    bots = next;
    byCid.clear();
    bots.forEach((b) => byCid.set(b.agent.cid, b));
    if (selected) selected = byCid.get(selected.agent.cid) ?? null;
  }
