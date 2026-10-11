// Society: the agents as a graph of who knows whom, and what each thinks of the
// others. Each agent keeps its place (by cid) across refreshes; the panel beside
// the graph shows the selected agent's opinions in both directions.

import '../styles/society.css';
import { api, type Bond, type Opinion, type Society, type SocietyAgent } from '../api';
import { agentName, roleLabel } from '../roles';
import { attempt, badge, button, clip, compact, empty, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import { agentHue } from './floor';
import type { Cleanup } from './index';

const W = 960;
const H = 600;
const PAD = 90;
const POLL_MS = 5000;
const FIRST_STEPS = 300;
const LATER_STEPS = 150;
const REST = 150;
const DISAGREE = 0.25;
const INK = '#2b2420';
const SVG = 'http://www.w3.org/2000/svg';
const COLOUR = { red: '#e5533d', amber: '#f2b84b', green: '#3fae6a' } as const;
type Band = keyof typeof COLOUR;
const KIND_TONE: Record<Bond['kind'], Tone> = { met: 'muted', hired: 'accent', 'was-hired': 'info', added: 'ok' };

interface Pt { x: number; y: number }
interface Body extends Pt { cid: number; vx: number; vy: number; fixed: boolean }
interface Actions { pick: (cid: number | null) => void; rate: (cid: number, value: number) => Promise<void> }
type Kid = SVGElement | null | undefined | false;

const clamp = (v: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, v));
const band = (e: number): Band => (e < 0.4 ? 'red' : e <= 0.6 ? 'amber' : 'green');
const reputation = (a: SocietyAgent): number => clamp(a.reputation / 100, 0, 1);
const side = (a: SocietyAgent): number => 46 + 12 * reputation(a);

export function mount(root: HTMLElement): Cleanup {
  const graph = h('div.soc-graph');
  const aside = h('aside.soc-panel', { 'aria-label': 'Selected agent' });
  root.append(h('div.view-head', null, h('h1', null, 'Society')), h('div.soc', null, graph, aside));

  const pos = new Map<number, Pt>();
  let data: Society | null = null;
  let loaded = false;
  let alive = true;
  let selected: number | null = null;

  const render = (): void => {
    if (!loaded || !alive) return;
    const d = data;
    if (!d) {
      replace(graph, empty('alert', 'Could not load the society.'));
      replace(aside);
      return;
    }
    if (!d.agents.length) {
      replace(graph, empty('agents', 'No agents yet.', 'Spawn some and they will show up here, knowing one another.'));
      replace(aside);
      return;
    }
    if (selected !== null && !d.agents.some((a) => a.cid === selected)) selected = null;
    layout(d.agents, d.bonds, pos);
    replace(graph, drawGraph(d, selected, pos, pick));
    replace(aside, detail(d, selected, { pick, rate }));
  };

  const pick = (cid: number | null): void => {
    selected = cid;
    render();
  };

  const load = async (): Promise<void> => {
    try {
      data = await api.society();
    } catch {
      // Keep the last good view; the next poll tries again.
    }
    loaded = true;
    render();
  };

  const rate = async (cid: number, value: number): Promise<void> => {
    await attempt(() => api.rateAgent(cid, value), value > 0 ? 'Trusted more' : 'Trusted less');
    await load();
  };

  void load();
  const timer = window.setInterval(() => void load(), POLL_MS);
  return () => {
    alive = false;
    window.clearInterval(timer);
  };
}

// ---- Layout ----------------------------------------------------------------

/** Places the agents. Agents already placed stay where they are; only new ones
 *  move, under the springs and repulsion of the whole graph. Departed agents are dropped. */
function layout(agents: SocietyAgent[], bonds: Bond[], pos: Map<number, Pt>): void {
  const live = new Set(agents.map((a) => a.cid));
  for (const cid of Array.from(pos.keys())) if (!live.has(cid)) pos.delete(cid);
  const fresh = agents.filter((a) => !pos.has(a.cid)).map((a) => a.cid);
  if (!fresh.length) return;

  const isFresh = new Set(fresh);
  const placed = pos.size > 0;
  fresh.forEach((cid, i) => {
    const angle = i * 2.39996; // golden angle, so a crowd fans out evenly
    const r = placed ? 40 : 70 + 26 * Math.sqrt(i);
    pos.set(cid, { x: W / 2 + Math.cos(angle) * r, y: H / 2 + Math.sin(angle) * r });
  });

  const bodies: Body[] = agents.map((a) => {
    const p = pos.get(a.cid) ?? { x: W / 2, y: H / 2 };
    return { cid: a.cid, x: p.x, y: p.y, vx: 0, vy: 0, fixed: !isFresh.has(a.cid) };
  });
  const index = new Map(bodies.map((b, i) => [b.cid, i] as const));

  // One spring per unordered pair; stronger contacts pull harder.
  const springs = new Map<string, { a: number; b: number; k: number }>();
  for (const bond of bonds) {
    const a = index.get(bond.from);
    const b = index.get(bond.to);
    if (a === undefined || b === undefined || a === b) continue;
    const lo = Math.min(a, b);
    const hi = Math.max(a, b);
    const key = `${lo}:${hi}`;
    const k = 0.01 + 0.02 * clamp(bond.strength / 10, 0, 1);
    springs.set(key, { a: lo, b: hi, k: Math.max(k, springs.get(key)?.k ?? 0) });
  }

  const steps = fresh.length === agents.length ? FIRST_STEPS : LATER_STEPS;
  for (let step = 0; step < steps; step++) {
    for (let i = 0; i < bodies.length; i++) {
      for (let j = i + 1; j < bodies.length; j++) {
        const p = bodies[i];
        const q = bodies[j];
        if (!p || !q) continue;
        const dx = q.x - p.x;
        const dy = q.y - p.y;
        const d = Math.max(Math.hypot(dx, dy), 1);
        const f = 16000 / (d * d);
        const fx = (f * dx) / d;
        const fy = (f * dy) / d;
        p.vx -= fx;
        p.vy -= fy;
        q.vx += fx;
        q.vy += fy;
      }
    }
    for (const { a, b, k } of springs.values()) {
      const p = bodies[a];
      const q = bodies[b];
      if (!p || !q) continue;
      const dx = q.x - p.x;
      const dy = q.y - p.y;
      const d = Math.max(Math.hypot(dx, dy), 1);
      const f = k * (d - REST);
      const fx = (f * dx) / d;
      const fy = (f * dy) / d;
      p.vx += fx;
      p.vy += fy;
      q.vx -= fx;
      q.vy -= fy;
    }
    for (const p of bodies) {
      // Centring, then damping; placed agents keep their coordinates.
      p.vx = clamp((p.vx + (W / 2 - p.x) * 0.004) * 0.8, -25, 25);
      p.vy = clamp((p.vy + (H / 2 - p.y) * 0.004) * 0.8, -25, 25);
      if (p.fixed) continue;
      p.x = clamp(p.x + p.vx, PAD, W - PAD);
      p.y = clamp(p.y + p.vy, PAD, H - PAD);
    }
  }
  for (const p of bodies) pos.set(p.cid, { x: p.x, y: p.y });
}

// ---- Graph -----------------------------------------------------------------

/** An SVG element. Attributes are set, never parsed from markup. */
function s<K extends keyof SVGElementTagNameMap>(
  tag: K,
  attrs: Record<string, string | number> = {},
  ...kids: Kid[]
): SVGElementTagNameMap[K] {
  const node = document.createElementNS(SVG, tag);
  for (const [key, value] of Object.entries(attrs)) node.setAttribute(key, String(value));
  for (const kid of kids) if (kid) node.appendChild(kid);
  return node;
}

function drawGraph(d: Society, selected: number | null, pos: Map<number, Pt>, pick: (cid: number | null) => void): SVGSVGElement {
  const agent = new Map(d.agents.map((a) => [a.cid, a] as const));
  const bondOf = new Map(d.bonds.map((b) => [`${b.from}>${b.to}`, b] as const));
  const mutual = (b: Bond): boolean => bondOf.has(`${b.to}>${b.from}`);

  const bg = s('rect', { class: 'soc-bg', x: 0, y: 0, width: W, height: H });
  bg.addEventListener('click', () => pick(null));
  const dots = s('rect', { class: 'soc-dots', x: 0, y: 0, width: W, height: H, fill: 'url(#soc-dots)' });
  const defs = s(
    'defs',
    {},
    s('pattern', { id: 'soc-dots', width: 24, height: 24, patternUnits: 'userSpaceOnUse' }, s('circle', { cx: 2, cy: 2, r: 1.3, class: 'soc-dot' })),
    ...(Object.keys(COLOUR) as Band[]).map((c) =>
      s(
        'marker',
        { id: `soc-arrow-${c}`, viewBox: '0 0 10 10', refX: 9, refY: 5, markerWidth: 12, markerHeight: 12, markerUnits: 'userSpaceOnUse', orient: 'auto' },
        s('path', { d: 'M0 0 L10 5 L0 10 z', fill: COLOUR[c] }),
      ),
    ),
  );

  const edges = s('g');
  for (const b of d.bonds) {
    const from = pos.get(b.from);
    const to = pos.get(b.to);
    const a = agent.get(b.from);
    const z = agent.get(b.to);
    if (!from || !to || !a || !z || b.from === b.to) continue;
    const path = s('path', { d: edgePath(from, to, side(a) / 2 + 8, side(z) / 2 + 8) });
    if (selected === null) {
      path.setAttribute('class', mutual(b) ? 'soc-edge soc-mutual' : 'soc-edge');
    } else if (b.from === selected || b.to === selected) {
      const out = b.from === selected;
      const c = band(b.opinion.e);
      path.setAttribute('class', out ? 'soc-edge soc-out' : 'soc-edge soc-in');
      path.style.stroke = COLOUR[c];
      path.style.strokeWidth = String(1.5 + 4 * clamp(b.strength / 10, 0, 1));
      if (out) path.setAttribute('marker-end', `url(#soc-arrow-${c})`);
    } else {
      path.setAttribute('class', 'soc-edge soc-faded');
    }
    edges.appendChild(path);
  }

  const nodes = s('g');
  for (const a of d.agents) {
    const p = pos.get(a.cid);
    if (!p) continue;
    const sz = side(a);
    const half = sz / 2;
    const hue = agentHue(a.cid);
    const name = agentName(a);
    const label = clip(name, 16);
    const w = Math.max(56, label.length * 7.4 + 16);
    const classes = ['soc-node'];
    if (selected !== null && selected !== a.cid) classes.push('soc-dim');
    if (a.cid === selected) classes.push('soc-sel');

    const node = s('g', {
      class: classes.join(' '),
      transform: `translate(${p.x.toFixed(1)} ${p.y.toFixed(1)})`,
      tabindex: 0,
      role: 'button',
      'aria-label': name,
    });
    const body = s(
      'g',
      { class: 'soc-body' },
      s('rect', { class: 'soc-shadow', x: -half + 3, y: -half + 3, width: sz, height: sz, rx: 12 }),
      s('line', { x1: 0, y1: -half, x2: 0, y2: -half - 12, stroke: INK, 'stroke-width': 3 }),
      s('circle', { cx: 0, cy: -half - 13, r: 4.5, fill: `hsl(${hue} 70% 55%)`, stroke: INK, 'stroke-width': 2.5 }),
      s('rect', { x: -half, y: -half, width: sz, height: sz, rx: 12, fill: `hsl(${hue} 60% 64%)`, stroke: INK, 'stroke-width': 3 }),
      ...eyes(sz),
      s('path', {
        d: `M${(-sz * 0.16).toFixed(1)} ${(sz * 0.22).toFixed(1)} Q0 ${(sz * 0.34).toFixed(1)} ${(sz * 0.16).toFixed(1)} ${(sz * 0.22).toFixed(1)}`,
        fill: 'none',
        stroke: INK,
        'stroke-width': 2.5,
        'stroke-linecap': 'round',
      }),
    );
    const tag = s('rect', { class: 'soc-tag', x: -w / 2, y: half + 8, width: w, height: 22, rx: 6 });
    const text = s('text', { class: 'soc-name', x: 0, y: half + 23, 'text-anchor': 'middle' });
    text.textContent = label;

    node.append(body, tag, text);
    node.addEventListener('click', (ev) => {
      ev.stopPropagation();
      pick(a.cid);
    });
    node.addEventListener('keydown', (ev) => {
      if (ev.key === 'Enter' || ev.key === ' ') {
        ev.preventDefault();
        pick(a.cid);
      }
    });
    nodes.appendChild(node);
  }

  return s('svg', { class: 'soc-svg', viewBox: `0 0 ${W} ${H}`, preserveAspectRatio: 'xMidYMid meet', 'aria-label': 'Society graph' }, defs, bg, dots, edges, nodes);
}

/** A curved path from a's edge to z's edge, bowed to one side. Reversed bonds bow apart. */
function edgePath(a: Pt, z: Pt, ra: number, rz: number): string {
  const dx = z.x - a.x;
  const dy = z.y - a.y;
  const len = Math.max(Math.hypot(dx, dy), 1);
  const ux = dx / len;
  const uy = dy / len;
  const x1 = a.x + ux * ra;
  const y1 = a.y + uy * ra;
  const x2 = z.x - ux * rz;
  const y2 = z.y - uy * rz;
  const cx = (x1 + x2) / 2 - uy * len * 0.16;
  const cy = (y1 + y2) / 2 + ux * len * 0.16;
  return `M${x1.toFixed(1)} ${y1.toFixed(1)} Q${cx.toFixed(1)} ${cy.toFixed(1)} ${x2.toFixed(1)} ${y2.toFixed(1)}`;
}

/** The two eyes: white sclera with an ink pupil each. */
function eyes(sz: number): SVGElement[] {
  const out: SVGElement[] = [];
  for (const dir of [-1, 1]) {
    const x = dir * sz * 0.2;
    const y = -sz * 0.08;
    out.push(
      s('circle', { cx: x, cy: y, r: sz * 0.14, fill: '#fff', stroke: INK, 'stroke-width': 2 }),
      s('circle', { cx: x + 1.5, cy: y + 1.5, r: sz * 0.06, fill: INK }),
    );
  }
  return out;
}

// ---- Panel -----------------------------------------------------------------

function detail(d: Society, selected: number | null, act: Actions): HTMLElement {
  const me = d.agents.find((a) => a.cid === selected);
  if (selected === null || !me) return legend();

  const agent = new Map(d.agents.map((a) => [a.cid, a] as const));
  const bondOf = new Map(d.bonds.map((b) => [`${b.from}>${b.to}`, b] as const));
  const nameOf = (cid: number): string => {
    const a = agent.get(cid);
    return a ? agentName(a) : `agent ${cid}`;
  };
  // Two holders disagree when their opinions of each other differ by more than DISAGREE.
  const gap = (peer: number): boolean => {
    const ab = bondOf.get(`${me.cid}>${peer}`);
    const ba = bondOf.get(`${peer}>${me.cid}`);
    return !!ab && !!ba && Math.abs(ab.opinion.e - ba.opinion.e) > DISAGREE;
  };

  const name = agentName(me);
  const rep = clamp(me.reputation, 0, 100);
  const fill = h('span.soc-meter-fill');
  fill.style.width = `${rep}%`;
  const meter = h(
    'div.soc-meter',
    { role: 'meter', 'aria-label': 'Reputation', 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': Math.round(rep) },
    fill,
  );

  const outgoing = d.bonds
    .filter((b) => b.from === me.cid && agent.has(b.to))
    .map((b) => row(b.to, nameOf(b.to), b, gap(b.to), act.pick));
  const incoming = d.bonds
    .filter((b) => b.to === me.cid && agent.has(b.from))
    .map((b) => row(b.from, nameOf(b.from), b, gap(b.from), act.pick));

  return h(
    'div.soc-detail',
    null,
    h('header.soc-head', null, h('h2', null, name), h('p.soc-role', null, roleLabel(me.role))),
    h(
      'div.soc-stats',
      null,
      h('div.soc-stat', null, h('span.soc-k', null, 'Reputation'), h('span.soc-v', null, `${Math.round(rep)} / 100`)),
      meter,
      h('div.soc-stat', null, h('span.soc-k', null, 'Confidence'), h('span.soc-v', null, `${Math.round(clamp(me.confidence, 0, 1) * 100)}%`)),
      h('div.soc-stat', null, h('span.soc-k', null, 'Wallet'), h('span.soc-v', null, compact(me.wallet))),
    ),
    h(
      'div.soc-owner',
      null,
      button('Trust more', () => void act.rate(me.cid, 1), { kind: 'primary', size: 'sm' }),
      button('Trust less', () => void act.rate(me.cid, -1), { size: 'sm' }),
    ),
    h('h3.soc-h', null, `How ${name} sees others`),
    outgoing.length ? h('ul.soc-list', null, outgoing) : h('p.soc-none', null, 'No contacts yet.'),
    h('h3.soc-h', null, `How others see ${name}`),
    incoming.length ? h('ul.soc-list', null, incoming) : h('p.soc-none', null, 'Nobody has met them yet.'),
  );
}

function row(peer: number, peerName: string, bond: Bond, gap: boolean, pick: (cid: number) => void): HTMLElement {
  const who = h('button.soc-peer', { type: 'button' }, peerName);
  who.addEventListener('click', () => pick(peer));
  return h(
    'li.soc-row',
    null,
    h('div.soc-row-top', null, who, badge(bond.kind, KIND_TONE[bond.kind]), gap ? badge('they disagree', 'warn') : null),
    bar(bond.opinion),
    bond.note ? h('p.soc-note', null, bond.note) : null,
  );
}

/** An opinion: the filled part is the expected trust e; the hatched tail runs on by the uncertainty u. */
function bar(o: Opinion): HTMLElement {
  const e = clamp(o.e, 0, 1);
  const u = clamp(o.u, 0, 1);
  const fill = h('span.soc-bar-fill');
  fill.style.width = `${e * 100}%`;
  fill.style.background = COLOUR[band(e)];
  const tail = h('span.soc-bar-tail');
  tail.style.left = `${e * 100}%`;
  tail.style.width = `${Math.min(u, 1 - e) * 100}%`;
  return h(
    'div.soc-bar-row',
    { title: `trust ${e.toFixed(2)}, uncertainty ${u.toFixed(2)}` },
    h('div.soc-bar', null, fill, tail),
    h('span.soc-bar-v', null, e.toFixed(2)),
  );
}

function legend(): HTMLElement {
  return h(
    'div.soc-detail',
    null,
    h('header.soc-head', null, h('h2', null, 'Nothing selected'), h('p.soc-role', null, 'Pick an agent to see how it sees the others, and how they see it.')),
    h('h3.soc-h', null, 'Reading the graph'),
    h(
      'ul.soc-legend',
      null,
      h('li', null, h('span.soc-sw.soc-sw-red'), 'trust below 0.4'),
      h('li', null, h('span.soc-sw.soc-sw-amber'), 'trust from 0.4 to 0.6'),
      h('li', null, h('span.soc-sw.soc-sw-green'), 'trust above 0.6'),
      h('li', null, h('span.soc-sw'), 'solid, with an arrowhead: its own opinions'),
      h('li', null, h('span.soc-sw.soc-sw-dash'), 'dashed: the opinions held of it'),
      h('li', null, h('span.soc-sw.soc-sw-thin'), 'thicker line: a stronger contact'),
      h('li', null, h('span.soc-sw.soc-sw-mutual'), 'darker grey: a mutual pair'),
    ),
    h('p.soc-note', null, 'Trusting an agent more or less moves the opinions held of it.'),
  );
}
