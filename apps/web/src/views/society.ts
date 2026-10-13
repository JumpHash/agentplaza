// Society: the agents as a graph of who knows whom, and what each thinks of the
// others. Each node keeps its place (by key) across refreshes; the panel beside
// the graph shows the selected agent's opinions in both directions, or a summary
// of a selected group. Past GROUP_AT agents the graph opens at the family level:
// a family is a box that pops open into its roles, and a role into its agents.

import '../styles/society.css';
import { api, type Bond, type Opinion, type Society, type SocietyAgent } from '../api';
import { agentName, FAMILIES, type Family, familyLabel, roleFamily, roleLabel } from '../roles';
import { attempt, badge, button, clip, compact, empty, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import { agentHue } from './floor';
import type { Cleanup } from './index';

// The layout's world box: the first layout spreads the nodes over all of it, and the view
// shows the box, so the graph grows and shrinks with the panel.
const W = 760;
const H = 700;
/** Clear space kept between two nodes' footprints. */
const GAP = 10;
/** Collision passes; a box too crowded to clear in this many keeps the overlap it has. */
const PASSES = 400;
const POLL_MS = 5000;
const FIRST_STEPS = 300;
const LATER_STEPS = 150;
const REST = 150;
const DISAGREE = 0.25;
/** Above this many agents the graph opens at the family level (unless the toggle says otherwise). */
export const GROUP_AT = 24;
/** How long the members of a group take to pop out of it. */
const POP_MS = 250;
const INK = '#2b2420';
const SVG = 'http://www.w3.org/2000/svg';
const COLOUR = { red: '#e5533d', amber: '#f2b84b', green: '#3fae6a' } as const;
type Band = keyof typeof COLOUR;
const KIND_TONE: Record<Bond['kind'], Tone> = { met: 'muted', hired: 'accent', 'was-hired': 'info', added: 'ok' };

/** A node's key: 'a:<cid>' an agent, 'f:<family>' a family, 'r:<family>/<role>' a role. */
export type Key = string;
/** The group the graph is open at. A null family is the top level; a null role is a whole family. */
export interface Open { family: Family | null; role: string | null }
/** One thing drawn on a level: an agent, or a box standing for a family or a role of agents. */
export interface Mark {
  key: Key;
  kind: 'agent' | 'family' | 'role';
  label: string;
  members: SocietyAgent[];
  /** The group a click opens; null for an agent. */
  opens: Open | null;
}
/** The contacts drawn between two marks. Flat: one bond, directed. Grouped: every bond between the two, merged. */
export interface Edge { from: Key; to: Key; bonds: Bond[]; strength: number; trust: number }
/** What the graph shows at one level. */
export interface Level { marks: Mark[]; edges: Edge[]; /** The mark each agent is drawn in. */ of: Map<number, Key>; grouped: boolean }

interface Pt { x: number; y: number }
/** What a node takes up around its centre, in world units. */
interface Foot {
  /** Half the width: the robot, or its tag when that is wider. */
  half: number;
  /** The room above the centre: the antenna and its tip. */
  up: number;
  /** The room below the centre: the name tag. */
  down: number;
}
interface Item { key: Key; fp: Foot }
interface Body extends Pt { key: Key; vx: number; vy: number; fixed: boolean; fp: Foot }
interface Enter { from: Pt; fresh: Set<Key> }
interface Actions {
  pick: (key: Key | null) => void;
  open: (mark: Mark) => void;
  to: (next: Open) => void;
  up: () => void;
  group: () => void;
  rate: (cid: number, value: number) => Promise<void>;
}

const TOP: Open = { family: null, role: null };

const clamp = (v: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, v));
const band = (e: number): Band => (e < 0.4 ? 'red' : e <= 0.6 ? 'amber' : 'green');
const reputation = (a: SocietyAgent): number => clamp(a.reputation / 100, 0, 1);
const side = (a: SocietyAgent): number => 46 + 12 * reputation(a);
const agentKey = (cid: number): Key => `a:${cid}`;
const familyKey = (family: Family): Key => `f:${family}`;
const roleKey = (family: Family, role: string): Key => `r:${family}/${role}`;
/** The key of the group an open level is at; null at the top. */
const openKey = (o: Open): Key | null => (o.family === null ? null : o.role === null ? familyKey(o.family) : roleKey(o.family, o.role));
/** A group box's side by member count: the square root, so a big group stays readable. */
const boxSide = (n: number): number => 58 + 24 * Math.sqrt(n);
const tagWidth = (label: string): number => Math.max(56, clip(label, 16).length * 7.4 + 16);

/** The agents a key stands for: one agent, a family's agents, or a role's agents. */
export function membersOf(agents: SocietyAgent[], key: Key): SocietyAgent[] {
  if (key.startsWith('a:')) {
    const cid = Number(key.slice(2));
    return agents.filter((a) => a.cid === cid);
  }
  if (key.startsWith('f:')) {
    const family = key.slice(2);
    return agents.filter((a) => roleFamily(a.role) === family);
  }
  if (key.startsWith('r:')) {
    const cut = key.indexOf('/');
    if (cut < 0) return [];
    const family = key.slice(2, cut);
    const role = key.slice(cut + 1);
    return agents.filter((a) => a.role === role && roleFamily(a.role) === family);
  }
  return [];
}

/** The level one step up from `open`. */
export function parentOf(open: Open): Open {
  return open.family !== null && open.role !== null ? { family: open.family, role: null } : TOP;
}

/** The breadcrumb: "All families", then the family and role the level is open at. */
export function trailOf(open: Open): Array<{ label: string; open: Open }> {
  const out: Array<{ label: string; open: Open }> = [{ label: 'All families', open: TOP }];
  if (open.family !== null) out.push({ label: familyLabel(open.family), open: { family: open.family, role: null } });
  if (open.family !== null && open.role !== null) out.push({ label: roleLabel(open.role), open });
  return out;
}

function summary(from: Key, to: Key, bonds: Bond[]): Edge {
  return {
    from,
    to,
    bonds,
    strength: bonds.reduce((sum, b) => sum + b.strength, 0),
    trust: bonds.reduce((sum, b) => sum + b.opinion.e, 0) / bonds.length,
  };
}

/** What the graph draws at `open`. Grouped: a family box, except the open family (its roles), and
 *  a role box, except the open role (its agents); a role with one agent is that agent. Flat: every
 *  agent. Edges: one per bond when flat; grouped, one per pair of marks, its strength summed and
 *  its trust averaged. A bond inside one mark is not drawn. */
export function level(agents: SocietyAgent[], bonds: Bond[], open: Open, grouped: boolean): Level {
  const marks: Mark[] = [];
  const of = new Map<number, Key>();
  const add = (key: Key, kind: Mark['kind'], label: string, members: SocietyAgent[], opens: Open | null): void => {
    marks.push({ key, kind, label, members, opens });
    for (const a of members) of.set(a.cid, key);
  };
  const addAgent = (a: SocietyAgent): void => add(agentKey(a.cid), 'agent', agentName(a), [a], null);

  if (!grouped) {
    for (const a of agents) addAgent(a);
  } else {
    const byFamily = new Map<Family, SocietyAgent[]>();
    for (const a of agents) {
      const family = roleFamily(a.role);
      const list = byFamily.get(family);
      if (list) list.push(a);
      else byFamily.set(family, [a]);
    }
    for (const family of FAMILIES) {
      const members = byFamily.get(family);
      if (!members) continue;
      if (family !== open.family) {
        add(familyKey(family), 'family', familyLabel(family), members, { family, role: null });
        continue;
      }
      const byRole = new Map<string, SocietyAgent[]>();
      for (const a of members) {
        const list = byRole.get(a.role);
        if (list) list.push(a);
        else byRole.set(a.role, [a]);
      }
      for (const [role, group] of byRole) {
        if (group.length === 1 || role === open.role) for (const a of group) addAgent(a);
        else add(roleKey(family, role), 'role', roleLabel(role), group, { family, role });
      }
    }
  }

  const edges: Edge[] = [];
  const pairs = new Map<string, { from: Key; to: Key; bonds: Bond[] }>();
  for (const b of bonds) {
    const p = of.get(b.from);
    const q = of.get(b.to);
    if (p === undefined || q === undefined || p === q) continue;
    if (!grouped) {
      edges.push(summary(p, q, [b]));
      continue;
    }
    const [lo, hi] = p < q ? [p, q] : [q, p];
    const id = `${lo}|${hi}`;
    const pair = pairs.get(id) ?? { from: lo, to: hi, bonds: [] };
    pair.bonds.push(b);
    pairs.set(id, pair);
  }
  for (const pair of pairs.values()) edges.push(summary(pair.from, pair.to, pair.bonds));
  return { marks, edges, of, grouped };
}

/** The open group, or the top level when its family or role has no agents now. */
function validOpen(agents: SocietyAgent[], o: Open): Open {
  if (o.family === null || !agents.some((a) => roleFamily(a.role) === o.family)) return TOP;
  if (o.role !== null && !agents.some((a) => a.role === o.role)) return { family: o.family, role: null };
  return o;
}

export function mount(root: HTMLElement): Cleanup {
  const graph = h('div.soc-graph');
  const aside = h('aside.soc-panel', { 'aria-label': 'Selected agent' });
  const soc = h('div.soc', null, graph, aside);
  root.append(h('div.view-head', null, h('h1', null, 'Society')), soc);
  // Like the Floor, the Society fills the window and never scrolls the page.
  document.body.classList.add('floor-mode');

  const pos = new Map<Key, Pt>();
  let data: Society | null = null;
  let loaded = false;
  let alive = true;
  let selected: Key | null = null;
  let open: Open = TOP;
  /** Null: grouping follows the size (GROUP_AT); true or false: the user's toggle. */
  let grouped: boolean | null = null;
  /** Where the next level's new marks pop out of: set by a click on a group. */
  let popFrom: Pt | null = null;
  /** The agents with an active suspension, from the sanctions register, read once per refresh. */
  let suspended: ReadonlySet<number> = new Set();

  const isGrouped = (d: Society | null): boolean => grouped ?? (d !== null && d.agents.length > GROUP_AT);

  const render = (): void => {
    if (!loaded || !alive) return;
    const d = data;
    // With no agents, one empty state replaces the two panels (and their borders).
    if (d && !d.agents.length) {
      replace(soc, empty('society', 'No agents yet', 'Spawn agents and they show up here, with what they think of one another.'));
      return;
    }
    replace(soc, graph, aside);
    if (!d) {
      replace(graph, empty('alert', 'Could not load the society.'));
      replace(aside);
      return;
    }
    const on = isGrouped(d);
    open = validOpen(d.agents, on ? open : TOP);
    if (selected !== null && !membersOf(d.agents, selected).length) selected = null;
    const lv = level(d.agents, d.bonds, open, on);
    const known = new Set(pos.keys());
    layout(
      lv.marks.map((m) => ({ key: m.key, fp: markFoot(m) })),
      lv.edges,
      pos,
      popFrom,
    );
    const enter: Enter | null = popFrom
      ? { from: popFrom, fresh: new Set(lv.marks.map((m) => m.key).filter((k) => !known.has(k))) }
      : null;
    popFrom = null;
    replace(graph, controls(on, open, act), drawGraph(d, lv, selected, pos, enter, act, suspended));
    replace(aside, panel(d, selected, act, suspended));
  };

  /** Selects a node; an agent is zoomed to (its family and role open), so it is on the graph. */
  const pick = (key: Key | null): void => {
    const d = data;
    if (key !== null && key.startsWith('a:') && d && isGrouped(d)) {
      const a = d.agents.find((x) => agentKey(x.cid) === key);
      if (a) open = { family: roleFamily(a.role), role: a.role };
    }
    selected = key;
    render();
  };

  const to = (next: Open): void => {
    open = next;
    selected = openKey(next);
    render();
  };

  const act: Actions = {
    pick,
    open: (mark) => {
      popFrom = pos.get(mark.key) ?? null;
      if (mark.opens) to(mark.opens);
    },
    to,
    up: () => {
      if (open.family === null) pick(null);
      else to(parentOf(open));
    },
    group: () => {
      grouped = !isGrouped(data);
      if (!grouped) {
        open = TOP;
        if (selected !== null && !selected.startsWith('a:')) selected = null;
      }
      render();
    },
    rate: async (cid, value) => {
      await attempt(() => api.rateAgent(cid, value), value > 0 ? 'Trusted more' : 'Trusted less');
      await load();
    },
  };

  const load = async (): Promise<void> => {
    try {
      data = await api.society();
    } catch {
      // Keep the last good view; the next poll tries again.
    }
    // One read of the sanctions register per refresh marks every node; a failed read keeps the last set.
    try {
      const register = await api.sanctions();
      suspended = new Set(register.suspensions.filter((s) => s.active).map((s) => s.cid));
    } catch {
      // Keep the last set; the next poll tries again.
    }
    loaded = true;
    render();
  };

  // Esc goes up one level (then clears the selection).
  const onKey = (ev: KeyboardEvent): void => {
    if (ev.key !== 'Escape' || (open.family === null && selected === null)) return;
    ev.preventDefault();
    act.up();
  };
  document.addEventListener('keydown', onKey);

  void load();
  const timer = window.setInterval(() => void load(), POLL_MS);
  return () => {
    alive = false;
    window.clearInterval(timer);
    document.removeEventListener('keydown', onKey);
    document.body.classList.remove('floor-mode');
  };
}

// ---- Layout ----------------------------------------------------------------

/** The room an agent takes up, from its centre; the name tag is part of it. */
const foot = (a: SocietyAgent): Foot => {
  const half = side(a) / 2;
  const tag = tagWidth(agentName(a));
  return { half: Math.max(half, tag / 2), up: half + 18, down: half + 30 };
};

/** The room a mark takes up: an agent's robot, or a group's box and its tag. */
function markFoot(m: Mark): Foot {
  const only = m.kind === 'agent' ? m.members[0] : undefined;
  if (only) return foot(only);
  const half = boxSide(m.members.length) / 2;
  return { half: Math.max(half, tagWidth(m.label) / 2), up: half + 6, down: half + 30 };
}

/** How far an edge stops short of a mark's centre. */
function radius(m: Mark): number {
  const only = m.kind === 'agent' ? m.members[0] : undefined;
  return only ? side(only) / 2 + 8 : boxSide(m.members.length) / 2 + 4;
}

/** How far two footprints overlap on each axis; they clash only when both are positive. */
function overlap(p: Pt, fp: Foot, q: Pt, fq: Foot): [number, number] {
  const dx = q.x - p.x;
  const dy = q.y - p.y;
  // Stacked vertically, the lower node's name tag must clear the upper node's antenna.
  const stack = dy >= 0 ? fp.down + fq.up : fq.down + fp.up;
  return [fp.half + fq.half + GAP - Math.abs(dx), stack + GAP - Math.abs(dy)];
}

/** The clearest spot for a newcomer: free room to the nearest node (capped, so any spot with
 *  room is as good as another), then the spot nearest its contacts among those. */
function clearest(item: Item, items: Item[], edges: Edge[], pos: Map<Key, Pt>): Pt {
  const fp = item.fp;
  const others: Array<Pt & { fp: Foot }> = [];
  for (const o of items) {
    const p = pos.get(o.key);
    if (p && o.key !== item.key) others.push({ x: p.x, y: p.y, fp: o.fp });
  }
  const peers: Pt[] = [];
  for (const e of edges) {
    const other = e.from === item.key ? e.to : e.to === item.key ? e.from : null;
    const p = other === null ? undefined : pos.get(other);
    if (p) peers.push(p);
  }
  const target = peers.length
    ? { x: peers.reduce((s, p) => s + p.x, 0) / peers.length, y: peers.reduce((s, p) => s + p.y, 0) / peers.length }
    : { x: W / 2, y: H / 2 };
  let best = target;
  let bestScore = -Infinity;
  for (let x = fp.half; x <= W - fp.half; x += 6) {
    for (let y = fp.up; y <= H - fp.down; y += 6) {
      let room = 40;
      for (const o of others) {
        const [ox, oy] = overlap({ x, y }, fp, o, o.fp);
        room = Math.min(room, Math.max(-ox, -oy));
      }
      const score = room - 0.02 * Math.hypot(x - target.x, y - target.y);
      if (score > bestScore) {
        bestScore = score;
        best = { x, y };
      }
    }
  }
  return best;
}

/** Stretches the nodes over the box, each axis on its own, so a first layout uses all of it. */
function stretch(bodies: Body[]): void {
  const xs = bodies.map((b) => b.x);
  const ys = bodies.map((b) => b.y);
  const x0 = Math.min(...xs);
  const x1 = Math.max(...xs);
  const y0 = Math.min(...ys);
  const y1 = Math.max(...ys);
  // A single row or column of nodes goes to the middle of its axis.
  const map = (v: number, lo: number, hi: number, outLo: number, outHi: number): number =>
    hi - lo < 1 ? (outLo + outHi) / 2 : outLo + ((v - lo) / (hi - lo)) * (outHi - outLo);
  for (const b of bodies) {
    b.x = map(b.x, x0, x1, 60, W - 60);
    b.y = map(b.y, y0, y1, 70, H - 70);
  }
}

/** Pushes footprints apart until none clash, or the passes run out. A clash moves both nodes
 *  along the axis where they overlap least; a placed node holds still. Nodes stay in the box. */
function separate(bodies: Body[]): void {
  for (let pass = 0; pass < PASSES; pass++) {
    let clashed = false;
    for (let i = 0; i < bodies.length; i++) {
      for (let j = i + 1; j < bodies.length; j++) {
        const p = bodies[i];
        const q = bodies[j];
        if (!p || !q) continue;
        const [ox, oy] = overlap(p, p.fp, q, q.fp);
        if (ox <= 0 || oy <= 0) continue;
        // Who gives way: a placed node holds still, so a free one takes the whole push.
        const [wi, wj]: [number, number] = p.fixed && q.fixed ? [0, 0] : p.fixed ? [0, 1] : q.fixed ? [1, 0] : [0.5, 0.5];
        if (wi === 0 && wj === 0) continue;
        clashed = true;
        if (ox < oy) {
          const s = q.x < p.x ? -1 : 1;
          q.x += s * (ox + 0.5) * wj;
          p.x -= s * (ox + 0.5) * wi;
        } else {
          const s = q.y < p.y ? -1 : 1;
          q.y += s * (oy + 0.5) * wj;
          p.y -= s * (oy + 0.5) * wi;
        }
      }
    }
    for (const b of bodies) {
      if (b.fixed) continue;
      b.x = clamp(b.x, b.fp.half, W - b.fp.half);
      b.y = clamp(b.y, b.fp.up, H - b.fp.down);
    }
    if (!clashed) break;
  }
}

/** Places the nodes. Nodes already placed stay where they are; only new ones move, under the
 *  springs and repulsion of the whole graph, and then clear of everyone's footprint. A new node
 *  starts around `anchor` when a group has just popped open there. Departed nodes are dropped. */
function layout(items: Item[], edges: Edge[], pos: Map<Key, Pt>, anchor: Pt | null): void {
  const live = new Set(items.map((i) => i.key));
  for (const key of Array.from(pos.keys())) if (!live.has(key)) pos.delete(key);
  const fresh = items.filter((i) => !pos.has(i.key)).map((i) => i.key);
  if (!fresh.length) return;

  const isFresh = new Set(fresh);
  const placed = pos.size > 0;
  if (anchor) {
    fresh.forEach((key, i) => {
      const angle = i * 2.39996; // golden angle, so the members fan out evenly
      const r = 20 + 22 * Math.sqrt(i);
      pos.set(key, { x: anchor.x + Math.cos(angle) * r, y: anchor.y + Math.sin(angle) * r });
    });
  } else if (placed) {
    // A newcomer takes the clearest spot near its contacts; the nodes already placed do not move.
    for (const item of items) if (isFresh.has(item.key)) pos.set(item.key, clearest(item, items, edges, pos));
  } else {
    fresh.forEach((key, i) => {
      const angle = i * 2.39996; // golden angle, so a crowd fans out evenly
      const r = 70 + 26 * Math.sqrt(i);
      pos.set(key, { x: W / 2 + Math.cos(angle) * r, y: H / 2 + Math.sin(angle) * r });
    });
  }

  const bodies: Body[] = items.map((item) => {
    const p = pos.get(item.key) ?? { x: W / 2, y: H / 2 };
    return { key: item.key, x: p.x, y: p.y, vx: 0, vy: 0, fixed: !isFresh.has(item.key), fp: item.fp };
  });
  const index = new Map(bodies.map((b, i) => [b.key, i] as const));

  // One spring per unordered pair; stronger contacts pull harder.
  const springs = new Map<string, { a: number; b: number; k: number }>();
  for (const e of edges) {
    const a = index.get(e.from);
    const b = index.get(e.to);
    if (a === undefined || b === undefined || a === b) continue;
    const lo = Math.min(a, b);
    const hi = Math.max(a, b);
    const key = `${lo}:${hi}`;
    const k = 0.01 + 0.02 * clamp(e.strength / 10, 0, 1);
    springs.set(key, { a: lo, b: hi, k: Math.max(k, springs.get(key)?.k ?? 0) });
  }

  const steps = fresh.length === items.length ? FIRST_STEPS : LATER_STEPS;
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
      // Centring, then damping; placed nodes keep their coordinates.
      p.vx = clamp((p.vx + (W / 2 - p.x) * 0.004) * 0.8, -25, 25);
      p.vy = clamp((p.vy + (H / 2 - p.y) * 0.004) * 0.8, -25, 25);
      if (p.fixed) continue;
      p.x = clamp(p.x + p.vx, p.fp.half, W - p.fp.half);
      p.y = clamp(p.y + p.vy, p.fp.up, H - p.fp.down);
    }
  }
  if (!placed) stretch(bodies);
  separate(bodies);
  for (const p of bodies) pos.set(p.key, { x: p.x, y: p.y });
}

// ---- Graph -----------------------------------------------------------------

/** An SVG element. Attributes are set, never parsed from markup. */
function s<K extends keyof SVGElementTagNameMap>(
  tag: K,
  attrs: Record<string, string | number> = {},
  ...kids: Array<SVGElement | null | undefined | false>
): SVGElementTagNameMap[K] {
  const node = document.createElementNS(SVG, tag);
  for (const [key, value] of Object.entries(attrs)) node.setAttribute(key, String(value));
  for (const kid of kids) if (kid) node.appendChild(kid);
  return node;
}

/** The level's toolbar, above the graph: the family toggle, then the breadcrumb and Back. */
function controls(on: boolean, at: Open, act: Actions): HTMLElement {
  const toggle = h(
    'button.soc-toggle',
    { type: 'button', 'aria-pressed': on ? 'true' : 'false', title: 'Draw the families as boxes that open into roles and agents' },
    'Group by family',
  );
  toggle.addEventListener('click', () => act.group());
  if (!on) return h('div.soc-top', null, toggle);

  const crumbs = h('nav.soc-crumbs', { 'aria-label': 'Groups' });
  trailOf(at).forEach((c, i, all) => {
    if (i) crumbs.append(h('span.soc-sep', { 'aria-hidden': 'true' }, '›'));
    if (i === all.length - 1) {
      crumbs.append(h('span.soc-crumb', { 'aria-current': 'page' }, c.label));
      return;
    }
    const link = h('button.soc-crumb', { type: 'button' }, c.label);
    link.addEventListener('click', () => act.to(c.open));
    crumbs.append(link);
  });
  return h('div.soc-top', null, toggle, crumbs, at.family !== null ? button('Back', () => act.up(), { size: 'sm' }) : null);
}

function drawGraph(d: Society, lv: Level, selected: Key | null, pos: Map<Key, Pt>, enter: Enter | null, act: Actions, suspended: ReadonlySet<number>): SVGSVGElement {
  const byKey = new Map(lv.marks.map((m) => [m.key, m] as const));
  const bondOf = new Set(d.bonds.map((b) => `${b.from}>${b.to}`));
  const mutual = (b: Bond): boolean => bondOf.has(`${b.to}>${b.from}`);
  const sel = selected !== null && selected.startsWith('a:') ? Number(selected.slice(2)) : null;
  const selMark = sel === null ? null : (lv.of.get(sel) ?? null);
  // A selected group (a family or role, open) lights the contacts that cross its edge.
  const group = selected !== null && !selected.startsWith('a:') ? new Set(membersOf(d.agents, selected).map((a) => a.cid)) : null;
  const inGroup = (m: Mark | undefined): boolean => !!group && !!m && m.members.every((a) => group.has(a.cid));
  const lit = (e: Edge): boolean => {
    if (group) return inGroup(byKey.get(e.from)) !== inGroup(byKey.get(e.to));
    return selMark !== null && (e.from === selMark || e.to === selMark);
  };
  // The selection's marks stay bright, with the marks at the far end of each lit contact.
  const bright = new Set<Key>();
  if (selected !== null) {
    if (selMark !== null) bright.add(selMark);
    for (const m of lv.marks) if (inGroup(m)) bright.add(m.key);
    for (const e of lv.edges) {
      if (!lit(e)) continue;
      bright.add(e.from);
      bright.add(e.to);
    }
  }

  const bg = s('rect', { class: 'soc-bg', x: -W, y: -H, width: 3 * W, height: 3 * H });
  bg.addEventListener('click', () => act.pick(null));
  const dots = s('rect', { class: 'soc-dots', x: -W, y: -H, width: 3 * W, height: 3 * H, fill: 'url(#soc-dots)' });
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
  for (const e of lv.edges) {
    const from = pos.get(e.from);
    const to = pos.get(e.to);
    const a = byKey.get(e.from);
    const z = byKey.get(e.to);
    if (!from || !to || !a || !z) continue;
    const path = s('path', { d: edgePath(from, to, radius(a), radius(z)) });
    const width = 1.5 + 4 * clamp(e.strength / (lv.grouped ? 30 : 10), 0, 1);
    const first = lv.grouped ? undefined : e.bonds[0];
    if (selected === null) {
      path.setAttribute('class', first && mutual(first) ? 'soc-edge soc-mutual' : 'soc-edge');
      if (lv.grouped) path.style.strokeWidth = String(width);
    } else if (!lit(e)) {
      path.setAttribute('class', 'soc-edge soc-faded');
      if (lv.grouped) path.style.strokeWidth = String(width);
    } else if (lv.grouped) {
      path.setAttribute('class', 'soc-edge soc-agg');
      path.style.stroke = COLOUR[band(e.trust)];
      path.style.strokeWidth = String(width);
    } else {
      // Flat, one bond: the selected agent's own opinions run out (arrowhead), the ones held of it run in.
      const b = e.bonds[0];
      if (b) {
        const out = b.from === sel;
        const c = band(b.opinion.e);
        path.setAttribute('class', out ? 'soc-edge soc-out' : 'soc-edge soc-in');
        path.style.stroke = COLOUR[c];
        path.style.strokeWidth = String(width);
        if (out) path.setAttribute('marker-end', `url(#soc-arrow-${c})`);
      }
    }
    edges.appendChild(path);
  }

  const nodes = s('g');
  for (const m of lv.marks) {
    const p = pos.get(m.key);
    const first = m.members[0];
    if (!p || !first) continue;
    const grp = m.kind !== 'agent';
    const sz = grp ? boxSide(m.members.length) : side(first);
    const half = sz / 2;
    const label = grp ? m.label : agentName(first);
    const w = tagWidth(label);
    const classes = ['soc-node'];
    if (grp) classes.push('soc-group');
    if (selected !== null && !bright.has(m.key)) classes.push('soc-dim');
    if (m.key === selected) classes.push('soc-sel');

    const node = s('g', {
      class: classes.join(' '),
      transform: `translate(${p.x.toFixed(1)} ${p.y.toFixed(1)})`,
      tabindex: 0,
      role: 'button',
      'aria-label': label,
    });
    const tag = s('rect', { class: 'soc-tag', x: -w / 2, y: half + 8, width: w, height: 22, rx: 6 });
    const text = s('text', { class: 'soc-name', x: 0, y: half + 23, 'text-anchor': 'middle' });
    text.textContent = clip(label, 16);
    // A node with a suspended agent in it gets the ring at its top-right corner, inside the body so it bounces with it.
    const hasSuspended = m.members.some((a) => suspended.has(a.cid));
    const body = grp ? groupBody(m, sz) : robotBody(first, sz);
    if (hasSuspended) {
      body.append(suspendedRing(half));
      node.setAttribute('aria-label', `${label}, suspended`);
    }
    node.append(body, tag, text);

    const activate = (): void => {
      if (m.opens) act.open(m);
      else act.pick(m.key);
    };
    node.addEventListener('click', (ev) => {
      ev.stopPropagation();
      activate();
    });
    node.addEventListener('keydown', (ev) => {
      if (ev.key === 'Enter' || ev.key === ' ') {
        ev.preventDefault();
        activate();
      }
    });
    // Members of a group that has just opened pop out of where the group was.
    if (enter && enter.fresh.has(m.key) && typeof node.animate === 'function') {
      node.animate(
        [
          { transform: `translate(${enter.from.x.toFixed(1)}px, ${enter.from.y.toFixed(1)}px) scale(0.3)`, opacity: 0 },
          { transform: `translate(${p.x.toFixed(1)}px, ${p.y.toFixed(1)}px) scale(1)`, opacity: 1 },
        ],
        { duration: POP_MS, easing: 'cubic-bezier(0.3, 1.2, 0.5, 1)' },
      );
    }
    nodes.appendChild(node);
  }

  // Frame the layout box: the agents are spread over all of it, footprints included.
  const box = `0 0 ${W} ${H}`;
  return s('svg', { class: 'soc-svg', viewBox: box, preserveAspectRatio: 'xMidYMid meet', 'aria-label': 'Society graph' }, defs, bg, dots, edges, nodes);
}

/** The ring segment at a node's top-right corner, for a suspended agent: red-orange, with a tooltip. */
function suspendedRing(half: number): SVGElement {
  const r = 9;
  const ring = s('path', {
    class: 'soc-suspended',
    d: `M${half} ${(-half - r).toFixed(1)} A${r} ${r} 0 0 1 ${half + r} ${-half}`,
    fill: 'none',
    'stroke-width': 3.5,
    'stroke-linecap': 'round',
  });
  ring.style.stroke = 'var(--danger)';
  const title = s('title', {});
  title.textContent = 'suspended';
  ring.append(title);
  return ring;
}

/** A cartoon robot head, drawn around its centre: the body of an agent node. */
function robotBody(a: SocietyAgent, sz: number): SVGElement {
  const half = sz / 2;
  const hue = agentHue(a.cid);
  return s(
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
}

/** A group's box: a rounded square, up to three of its members' colours stacked on top, and the count. */
function groupBody(m: Mark, sz: number): SVGElement {
  const half = sz / 2;
  const shown = m.members.slice(0, 3);
  const faces = shown.map((a, i) =>
    s('rect', {
      x: (i - (shown.length - 1) / 2) * 14 - 7,
      y: -sz * 0.24 - 7,
      width: 14,
      height: 14,
      rx: 4,
      fill: `hsl(${agentHue(a.cid)} 60% 64%)`,
      stroke: INK,
      'stroke-width': 2,
    }),
  );
  const count = s('text', { class: 'soc-count', x: 0, y: sz * 0.22, 'text-anchor': 'middle' });
  count.textContent = String(m.members.length);
  return s(
    'g',
    { class: 'soc-body' },
    s('rect', { class: 'soc-shadow', x: -half + 3, y: -half + 3, width: sz, height: sz, rx: 16 }),
    s('rect', { class: 'soc-group-box', x: -half, y: -half, width: sz, height: sz, rx: 16 }),
    ...faces,
    count,
  );
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

function panel(d: Society, selected: Key | null, act: Actions, suspended: ReadonlySet<number>): HTMLElement {
  if (selected === null) return legend();
  if (selected.startsWith('a:')) return detail(d, Number(selected.slice(2)), act, suspended);
  return groupDetail(d, selected, act);
}

function detail(d: Society, cid: number, act: Actions, suspended: ReadonlySet<number>): HTMLElement {
  const me = d.agents.find((a) => a.cid === cid);
  if (!me) return legend();

  const agent = new Map(d.agents.map((a) => [a.cid, a] as const));
  const bondOf = new Map(d.bonds.map((b) => [`${b.from}>${b.to}`, b] as const));
  const nameOf = (peer: number): string => {
    const a = agent.get(peer);
    return a ? agentName(a) : `agent ${peer}`;
  };
  // Two holders disagree when their opinions of each other differ by more than DISAGREE.
  const gap = (peer: number): boolean => {
    const ab = bondOf.get(`${me.cid}>${peer}`);
    const ba = bondOf.get(`${peer}>${me.cid}`);
    return !!ab && !!ba && Math.abs(ab.opinion.e - ba.opinion.e) > DISAGREE;
  };
  const pick = (peer: number): void => act.pick(agentKey(peer));

  const name = agentName(me);
  const rep = clamp(me.reputation, 0, 100);
  const fill = h('span.soc-meter-fill');
  fill.style.width = me.confidence >= 0.05 ? `${rep}%` : '0%'; // unrated: an empty bar
  const meter = h(
    'div.soc-meter',
    { role: 'meter', 'aria-label': 'Reputation', 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': Math.round(rep) },
    fill,
  );

  const outgoing = d.bonds
    .filter((b) => b.from === me.cid && agent.has(b.to))
    .map((b) => row(b.to, nameOf(b.to), b, gap(b.to), pick));
  const incoming = d.bonds
    .filter((b) => b.to === me.cid && agent.has(b.from))
    .map((b) => row(b.from, nameOf(b.from), b, gap(b.from), pick));

  return h(
    'div.soc-detail',
    null,
    h('header.soc-head', null, h('h2', null, name), h('p.soc-role', null, roleLabel(me.role)), suspended.has(me.cid) ? badge('suspended', 'danger') : null),
    h(
      'div.soc-stats',
      null,
      h('div.soc-stat', null, h('span.soc-k', null, 'Reputation'), h('span.soc-v', null, me.confidence >= 0.05 ? `${Math.round(rep)} / 100` : 'Unrated')),
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

/** The summary of a selected group: its members, its trust across the edge, and its strongest outside ties. */
function groupDetail(d: Society, key: Key, act: Actions): HTMLElement {
  const members = membersOf(d.agents, key);
  const first = members[0];
  if (!first) return legend();
  const inside = new Set(members.map((m) => m.cid));
  const agent = new Map(d.agents.map((a) => [a.cid, a] as const));
  const family = roleFamily(first.role);
  const isFamily = key.startsWith('f:');
  const title = isFamily ? familyLabel(family) : roleLabel(first.role);
  const sub = isFamily ? `${members.length} agents` : `${members.length} agents · ${familyLabel(family)}`;

  // The contacts that cross the group's edge: members' opinions of outsiders (out), and theirs of members (in).
  const outs: Bond[] = [];
  const ins: Bond[] = [];
  for (const b of d.bonds) {
    const f = inside.has(b.from);
    const t = inside.has(b.to);
    if (f === t || !agent.has(b.from) || !agent.has(b.to)) continue;
    if (f) outs.push(b);
    else ins.push(b);
  }
  const mean = (bs: Bond[]): number | null => (bs.length ? bs.reduce((sum, b) => sum + b.opinion.e, 0) / bs.length : null);
  const trust = (label: string, m: number | null): HTMLElement =>
    h('div.soc-trust', null, h('span.soc-k', null, label), m === null ? h('span.soc-none', null, 'No contacts') : bar({ e: m, u: 0 }));
  const nameOf = (cid: number): string => {
    const a = agent.get(cid);
    return a ? agentName(a) : `agent ${cid}`;
  };
  const ties = [...outs, ...ins].sort((x, y) => y.strength - x.strength).slice(0, 3);

  return h(
    'div.soc-detail',
    null,
    h('header.soc-head', null, h('h2', null, title), h('p.soc-role', null, sub)),
    h('div.soc-stats', null, trust('Trust out', mean(outs)), trust('Trust in', mean(ins))),
    h('h3.soc-h', null, 'Members'),
    h('ul.soc-list', null, members.map((m) => memberRow(m, act))),
    h('h3.soc-h', null, 'Strongest outside ties'),
    ties.length
      ? h(
          'ul.soc-list',
          null,
          ties.map((b) => {
            const peer = inside.has(b.from) ? b.to : b.from;
            return row(peer, nameOf(peer), b, false, (cid) => act.pick(agentKey(cid)));
          }),
        )
      : h('p.soc-none', null, 'No contacts across the edge.'),
  );
}

function memberRow(m: SocietyAgent, act: Actions): HTMLElement {
  const who = h('button.soc-peer', { type: 'button' }, agentName(m));
  who.addEventListener('click', () => act.pick(agentKey(m.cid)));
  return h('li.soc-row', { 'data-key': `member-${m.cid}` }, h('div.soc-row-top', null, who, h('span.soc-k', null, roleLabel(m.role))));
}

function row(peer: number, peerName: string, bond: Bond, gap: boolean, pick: (cid: number) => void): HTMLElement {
  const who = h('button.soc-peer', { type: 'button' }, peerName);
  who.addEventListener('click', () => pick(peer));
  return h(
    'li.soc-row',
    { 'data-key': `peer-${peer}` },
    h('div.soc-row-top', null, who, badge(bond.kind, KIND_TONE[bond.kind]), gap ? badge('they disagree', 'warn') : null),
    bar(bond.opinion),
    bond.note ? noteEl(bond.note) : null,
  );
}

/** An opinion: the filled part is the expected trust e; the hatched tail runs on by the uncertainty u. */
function bar(o: Pick<Opinion, 'e' | 'u'>): HTMLElement {
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
      h('li', null, h('span.soc-sw.soc-sw-box'), 'a box: a group of agents, sized by its count; click to open it'),
    ),
    h('p.soc-note', null, 'Trusting an agent more or less moves the opinions held of it.'),
  );
}

/** A private note, clamped to a few lines; a click shows all of it. */
function noteEl(text: string): HTMLElement {
  const el = h('p.soc-note.clamped', { title: 'Click to expand' }, text);
  el.addEventListener('click', () => el.classList.toggle('clamped'));
  return el;
}
