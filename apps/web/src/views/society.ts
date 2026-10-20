// Society: the agents as a graph of who knows whom, and what each thinks of the
// others. Each node keeps its place (by key) across refreshes; the panel beside
// the graph shows the selected agent's opinions in both directions, or a summary
// of a selected group. Past GROUP_AT agents the graph opens at the family level:
// a family is a box that pops open into its roles, and a role into its agents.
// The markup is the prototype's (styles/proto/society.css): discs with initials,
// group boxes with member chips, and the panel's stats, rows and legend.

import '../styles/proto/society.css';
import { api, type Bond, type Opinion, type Society, type SocietyAgent } from '../api';
import { agentName, FAMILIES, type Family, familyLabel, roleFamily, roleLabel } from '../roles';
import { attempt, badge, button, clip, compact, empty, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';

// The layout's world box: the first layout places the nodes round all of it, and the view
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
/** An agent's disc radius, as drawn. */
const DISC = 20;
/** How far an agent's name tag reaches below its centre. */
const TAG_DOWN = 40;
const SVG = 'http://www.w3.org/2000/svg';
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
  /** Half the width: the disc, or its tag when that is wider. */
  half: number;
  /** The room above the centre: the disc, or a group's top edge. */
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
/** The band of an opinion: below 0.4 red (crit), up to 0.6 amber (warn), above green (ok). */
const band = (e: number): 'crit' | 'warn' | 'ok' => (e < 0.4 ? 'crit' : e <= 0.6 ? 'warn' : 'ok');
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
  const wrap = h('div.soc-wrap');
  const host = h('div');
  root.append(h('header.view-head', null, h('div', null, h('h1', null, 'Society'))), host);

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
  /** The contact notes the reader has opened, by 'from-to', kept across refreshes. */
  const expanded = new Set<string>();

  const isGrouped = (d: Society | null): boolean => grouped ?? (d !== null && d.agents.length > GROUP_AT);

  const render = (): void => {
    if (!loaded || !alive) return;
    const d = data;
    // With no agents, one empty state replaces the two panels (and their borders).
    if (d && !d.agents.length) {
      replace(host, empty('society', 'No agents yet', 'Spawn agents and they show up here, with what they think of one another.'));
      return;
    }
    replace(host, wrap);
    replace(wrap, graph, aside);
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
    replace(graph, toolbar(on, open, act), on ? crumbs(open, act) : null, drawGraph(d, lv, selected, pos, enter, act, suspended));
    replace(aside, panel(d, selected, act, suspended, expanded));
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
      const register = await api.sanctions({ active: true });
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
  };
}

// ---- Layout ----------------------------------------------------------------

/** The room a mark takes up: an agent's disc and its name tag, or a group's box and its tag. */
function markFoot(m: Mark): Foot {
  const only = m.kind === 'agent' ? m.members[0] : undefined;
  if (only) return { half: Math.max(DISC, tagWidth(agentName(only)) / 2), up: DISC, down: TAG_DOWN };
  const half = boxSide(m.members.length) / 2;
  return { half: Math.max(half, tagWidth(m.label) / 2), up: half, down: half + 20 };
}

/** How far two footprints overlap on each axis; they clash only when both are positive. */
function overlap(p: Pt, fp: Foot, q: Pt, fq: Foot): [number, number] {
  const dx = q.x - p.x;
  const dy = q.y - p.y;
  // Stacked vertically, the lower node's name tag must clear the upper node's disc.
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

/** Places the nodes. Nodes already placed stay where they are; only new ones move. The first
 *  layout puts them round a ring, as the prototype does; a newcomer takes the clearest spot near
 *  its contacts, and a group's members pop out round where the group was. Then the springs and
 *  repulsion of the whole graph settle the newcomers, and everyone is kept clear of each other.
 *  Departed nodes are dropped. */
function layout(items: Item[], edges: Edge[], pos: Map<Key, Pt>, anchor: Pt | null): void {
  const live = new Set(items.map((i) => i.key));
  for (const key of Array.from(pos.keys())) if (!live.has(key)) pos.delete(key);
  const fresh = items.filter((i) => !pos.has(i.key)).map((i) => i.key);
  if (!fresh.length) return;

  const isFresh = new Set(fresh);
  const placed = pos.size > 0;
  const ringed = !placed && !anchor;
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
    const n = items.length;
    const R = n <= 1 ? 0 : Math.min(280, 120 + n * 22);
    items.forEach((item, i) => {
      const ang = -Math.PI / 2 + (i * 2 * Math.PI) / Math.max(n, 1);
      pos.set(item.key, { x: W / 2 + R * Math.cos(ang), y: H / 2 + R * Math.sin(ang) * 0.85 });
    });
  }

  const bodies: Body[] = items.map((item) => {
    const p = pos.get(item.key) ?? { x: W / 2, y: H / 2 };
    return { key: item.key, x: p.x, y: p.y, vx: 0, vy: 0, fixed: !isFresh.has(item.key), fp: item.fp };
  });
  const index = new Map(bodies.map((b, i) => [b.key, i] as const));

  if (!ringed) {
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
  }
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

/** An SVG element holding text, set as text. */
function st<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string | number>, text: string): SVGElementTagNameMap[K] {
  const node = s(tag, attrs);
  node.textContent = text;
  return node;
}

/** The toolbar above the graph: the family toggle, and Back when a family is open. */
function toolbar(on: boolean, at: Open, act: Actions): HTMLElement {
  const toggle = button('Group by family', () => act.group(), {
    size: 'sm',
    title: 'Draw the families as boxes that open into roles and agents',
  });
  toggle.setAttribute('aria-pressed', on ? 'true' : 'false');
  return h('div.soc-toolbar', null, toggle, on && at.family !== null ? button('Back', () => act.up(), { size: 'sm' }) : null);
}

/** The breadcrumb, shown only when grouped: each step above the open one is a button back to it. */
function crumbs(at: Open, act: Actions): HTMLElement {
  const nav = h('nav.soc-crumbs', { 'aria-label': 'Breadcrumb' });
  const trail = trailOf(at);
  trail.forEach((c, i) => {
    if (i === trail.length - 1) nav.append(h('span', { 'aria-current': 'page' }, c.label));
    else nav.append(button(c.label, () => act.to(c.open), { kind: 'ghost', size: 'sm' }));
  });
  return nav;
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

  const bg = s('rect', { class: 'soc-bg', width: W, height: H, fill: 'transparent' });
  bg.addEventListener('click', () => act.pick(null));
  const defs = s(
    'defs',
    {},
    ...(['ok', 'warn', 'crit'] as const).map((t) =>
      s(
        'marker',
        { id: `soc-arr-${t}`, viewBox: '0 0 10 10', refX: 9, refY: 5, markerWidth: 6, markerHeight: 6, orient: 'auto' },
        s('path', { d: 'M0,0 L10,5 L0,10 z', style: `fill:var(--${t})` }),
      ),
    ),
  );

  const edges = s('g');
  lv.edges.forEach((e, k) => {
    const from = pos.get(e.from);
    const to = pos.get(e.to);
    if (!from || !to || !byKey.has(e.from) || !byKey.has(e.to)) return;
    const first = lv.grouped ? undefined : e.bonds[0];
    const out = first !== undefined && sel !== null && first.from === sel;
    const cls = ['soc-edge', `soc-e${k}`, `t-${band(e.trust)}`];
    if (selected === null) {
      if (first && mutual(first)) cls.push('soc-mutual');
    } else if (!lit(e)) {
      cls.push('soc-faded');
    } else {
      cls.push('soc-lit');
      if (first && !out) cls.push('soc-in');
    }
    const width = 1.5 + 4 * clamp(e.strength / (lv.grouped ? 30 : 10), 0, 1);
    const attrs: Record<string, string | number> = { class: cls.join(' '), d: edgePath(from, to), style: `--sw:${width}` };
    if (selected !== null && lit(e) && out) attrs['marker-end'] = `url(#soc-arr-${band(e.trust)})`;
    const path = s('path', attrs);
    path.append(st('title', {}, lv.grouped ? 'grouped contacts' : `trust ${e.trust.toFixed(2)}`));
    edges.appendChild(path);
  });

  const nodes = s('g');
  // Hovering a node lights the edges that touch it (the edges carry soc-e<index>).
  const hov = lv.marks
    .map((m, i) => {
      const ks = lv.edges.reduce<string[]>((o, e, k) => (e.from === m.key || e.to === m.key ? o.concat(`.soc-e${k}`) : o), []);
      return ks.length ? `.soc-svg:has(.soc-n${i}:hover) :is(${ks.join(',')}){opacity:1;stroke-width:2px}` : '';
    })
    .join('');
  nodes.appendChild(st('style', {}, hov));
  lv.marks.forEach((m, i) => {
    const p = pos.get(m.key);
    const first = m.members[0];
    if (!p || !first) return;
    const grp = m.kind !== 'agent';
    const label = grp ? m.label : agentName(first);
    const hasSuspended = m.members.some((a) => suspended.has(a.cid));
    const classes = ['soc-node', `soc-n${i}`];
    if (selected !== null && !bright.has(m.key)) classes.push('soc-dim');
    if (m.key === selected) classes.push('soc-sel');
    const node = s('g', {
      class: classes.join(' '),
      transform: `translate(${p.x.toFixed(1)} ${p.y.toFixed(1)})`,
      tabindex: 0,
      role: 'button',
      'aria-label': grp ? `${label}, ${m.members.length} agents${hasSuspended ? ', suspended' : ''}` : `${label}${hasSuspended ? ', suspended' : ''}`,
    });
    if (grp) node.append(...groupBody(m, boxSide(m.members.length), label, hasSuspended));
    else node.append(...agentBody(first, label, hasSuspended));

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
  });

  return s('svg', { class: 'soc-svg', viewBox: `0 0 ${W} ${H}`, role: 'img', 'aria-label': 'Society graph' }, defs, bg, edges, nodes);
}

/** The ring a suspended agent's node carries, as the prototype draws it. */
function suspendedDot(cx: number, cy: number): SVGElement {
  const dot = s('circle', { class: 'soc-susp', cx, cy, r: 5 });
  dot.append(st('title', {}, 'suspended'));
  return dot;
}

/** An agent's disc with its initials, the name below it, and a dot when it is suspended. */
function agentBody(a: SocietyAgent, label: string, suspendedNow: boolean): SVGElement[] {
  const w = a.name.trim().split(/\s+/);
  const ini = (w.length > 1 ? `${w[0]?.[0] ?? ''}${w[1]?.[0] ?? ''}` : (w[0] ?? '').slice(0, 2)).toUpperCase();
  const out: SVGElement[] = [
    s('circle', { class: 'soc-disc', r: DISC }),
    st('text', { class: 'soc-ini', 'text-anchor': 'middle', 'dominant-baseline': 'central' }, ini),
    st('text', { class: 'soc-lbl', y: DISC + 15, 'text-anchor': 'middle' }, clip(label, 16)),
  ];
  if (suspendedNow) out.push(suspendedDot(DISC * 0.7, -DISC * 0.7));
  return out;
}

/** A group's box: a rounded square with up to three of its members' chips, and the count; its tag below. */
function groupBody(m: Mark, side: number, label: string, suspendedNow: boolean): SVGElement[] {
  const half = side / 2;
  const chips = m.members.slice(0, 3).map((_, i) => s('rect', { class: 'soc-chip', x: 8 + i * 14, y: 8, width: 10, height: 10, rx: 2 }));
  const inner = s(
    'g',
    { transform: `translate(${-half} ${-half})` },
    s('rect', { class: 'soc-group', width: side, height: side, rx: 12 }),
    ...chips,
    st('text', { class: 'soc-tag', x: side / 2, y: side / 2 + 5, 'text-anchor': 'middle' }, String(m.members.length)),
    suspendedNow ? suspendedDot(side - 4, 4) : null,
  );
  return [inner, st('text', { class: 'soc-lbl', x: 0, y: half + 14, 'text-anchor': 'middle' }, clip(label, 16))];
}

/** The curve from one node's centre to another's, bowed to one side; reversed bonds bow apart. */
function edgePath(a: Pt, z: Pt): string {
  const mx = (a.x + z.x) / 2 - (z.y - a.y) * 0.15;
  const my = (a.y + z.y) / 2 + (z.x - a.x) * 0.15;
  return `M${a.x.toFixed(1)},${a.y.toFixed(1)} Q${mx.toFixed(1)},${my.toFixed(1)} ${z.x.toFixed(1)},${z.y.toFixed(1)}`;
}

// ---- Panel -----------------------------------------------------------------

/** The panel's head: the title, and under it the subtitle, as the prototype's view head. */
function viewHead(title: string, sub: string | null, extra: Node | null = null): HTMLElement {
  return h('div.view-head', null, h('div', null, h('h2', null, title, extra ? ' ' : null, extra), sub === null ? null : h('p.view-sub', null, sub)));
}

function panel(d: Society, selected: Key | null, act: Actions, suspended: ReadonlySet<number>, expanded: Set<string>): Node[] {
  if (selected === null) return legend();
  if (selected.startsWith('a:')) return detail(d, Number(selected.slice(2)), act, suspended, expanded);
  return groupDetail(d, selected, act);
}

function detail(d: Society, cid: number, act: Actions, suspended: ReadonlySet<number>, expanded: Set<string>): Node[] {
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
  const rated = me.confidence >= 0.05;
  const rep = clamp(me.reputation, 0, 100);
  const meter = h(
    'div.soc-bar',
    { role: 'meter', 'aria-label': 'Reputation', 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': Math.round(rated ? rep : 0) },
    h('i.soc-fill.t-ok', { style: `width:${rated ? rep : 0}%` }),
  );

  const outgoing = d.bonds
    .filter((b) => b.from === me.cid && agent.has(b.to))
    .map((b) => row(b.to, nameOf(b.to), b, gap(b.to), pick, expanded));
  const incoming = d.bonds
    .filter((b) => b.to === me.cid && agent.has(b.from))
    .map((b) => row(b.from, nameOf(b.from), b, gap(b.from), pick, expanded));

  return [
    viewHead(name, roleLabel(me.role), suspended.has(me.cid) ? badge('suspended', 'danger') : null),
    h(
      'div.soc-stats',
      null,
      h('div.stat', null, h('span.muted', null, 'Reputation'), h('b.num', null, rated ? `${Math.round(rep)} / 100` : 'Unrated'), meter),
      h('div.stat', null, h('span.muted', null, 'Confidence'), h('b.num', null, `${Math.round(clamp(me.confidence, 0, 1) * 100)}%`)),
      h('div.stat', null, h('span.muted', null, 'Wallet'), h('b.num', null, compact(me.wallet))),
    ),
    h(
      'div.cluster',
      null,
      button('Trust more', () => void act.rate(me.cid, 1), { kind: 'primary', size: 'sm' }),
      button('Trust less', () => void act.rate(me.cid, -1), { size: 'sm' }),
    ),
    h('h3', null, `How ${name} sees others`),
    ...(outgoing.length ? outgoing : [h('p.muted', null, 'No contacts yet.')]),
    h('h3', null, `How others see ${name}`),
    ...(incoming.length ? incoming : [h('p.muted', null, 'Nobody has met them yet.')]),
  ];
}

/** The summary of a selected group: its members, its trust across the edge, and its strongest outside ties. */
function groupDetail(d: Society, key: Key, act: Actions): Node[] {
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
    h('div.stat', null, h('span.muted', null, label), h('b.num', null, m === null ? 'none yet' : m.toFixed(2)), m === null ? null : bar({ e: m, u: 0 }));
  const nameOf = (cid: number): string => {
    const a = agent.get(cid);
    return a ? agentName(a) : `agent ${cid}`;
  };
  const ties = [...outs, ...ins].sort((x, y) => y.strength - x.strength).slice(0, 3);

  return [
    viewHead(title, sub),
    h('div.soc-stats', null, trust('Mean trust in', mean(ins)), trust('Mean trust out', mean(outs))),
    h('h3', null, 'Members'),
    ...members.map((m) => memberRow(m, act)),
    h('h3', null, 'Strongest outside ties'),
    ...(ties.length
      ? ties.map((b) => {
          const peer = inside.has(b.from) ? b.to : b.from;
          return tieRow(peer, nameOf(peer), b, act);
        })
      : [h('p.muted', null, 'No contacts across the edge.')]),
  ];
}

function memberRow(m: SocietyAgent, act: Actions): HTMLElement {
  return h(
    'div.soc-row',
    null,
    h('div', null, button(agentName(m), () => act.pick(agentKey(m.cid)), { kind: 'ghost', size: 'sm' })),
    h('span.muted', null, roleLabel(m.role)),
  );
}

/** A tie across a group's edge: the peer, the kind, the opinion and its note. */
function tieRow(peer: number, peerName: string, bond: Bond, act: Actions): HTMLElement {
  return h(
    'div.soc-row',
    null,
    h('div', null, button(peerName, () => act.pick(agentKey(peer)), { kind: 'ghost', size: 'sm' }), ' ', badge(bond.kind, 'muted')),
    h('span.num', null, bond.opinion.e.toFixed(2)),
    bar(bond.opinion),
    bond.note ? h('p.soc-note', null, bond.note) : null,
  );
}

function row(peer: number, peerName: string, bond: Bond, gap: boolean, pick: (cid: number) => void, expanded: Set<string>): HTMLElement {
  const key = `${bond.from}-${bond.to}`;
  return h(
    'div.soc-row',
    null,
    h(
      'div',
      null,
      button(peerName, () => pick(peer), { kind: 'ghost', size: 'sm' }),
      ' ',
      badge(bond.kind, KIND_TONE[bond.kind]),
      gap ? ' ' : null,
      gap ? badge('they disagree', 'warn') : null,
    ),
    h('span.num', null, bond.opinion.e.toFixed(2)),
    bar(bond.opinion),
    bond.note ? noteEl(bond.note, key, expanded) : null,
  );
}

/** A private note, clamped to two lines; a click shows all of it, and the click is remembered. */
function noteEl(text: string, key: string, expanded: Set<string>): HTMLElement {
  const el = h('p.soc-note', { title: 'Click to expand', class: expanded.has(key) ? 'open' : null }, text);
  el.addEventListener('click', () => {
    if (expanded.has(key)) expanded.delete(key);
    else expanded.add(key);
    el.classList.toggle('open', expanded.has(key));
  });
  return el;
}

/** An opinion: the filled part is the expected trust e; the hatched tail runs on by the uncertainty u. */
function bar(o: Pick<Opinion, 'e' | 'u'>): HTMLElement {
  const e = clamp(o.e, 0, 1);
  const u = clamp(o.u, 0, 1);
  const fill = h('i.soc-fill', { class: `t-${band(e)}` });
  fill.style.width = `${e * 100}%`;
  const tail = h('i.soc-tail');
  tail.style.left = `${e * 100}%`;
  tail.style.width = `${Math.min(u, 1 - e) * 100}%`;
  return h('div.soc-bar', { title: `trust ${e.toFixed(2)}, uncertainty ${u.toFixed(2)}` }, fill, tail);
}

function legend(): Node[] {
  return [
    viewHead('Nothing selected', 'Pick an agent to see how it sees the others, and how they see it.'),
    h('h3', null, 'Reading the graph'),
    h(
      'ul.soc-legend',
      null,
      legendItem('background:var(--crit)', 'trust below 0.4'),
      legendItem('background:var(--warn)', 'trust from 0.4 to 0.6'),
      legendItem('background:var(--ok)', 'trust above 0.6'),
      legendItem('', 'solid, with an arrowhead: its own opinions'),
      legendItem('background:repeating-linear-gradient(90deg,var(--ink-3) 0 6px,transparent 6px 10px)', 'dashed: the opinions held of it'),
      legendItem('height:8px', 'thicker line: a stronger contact'),
      legendItem('background:var(--ink-2)', 'darker grey: a mutual pair'),
      legendItem('height:14px;width:14px;border:1px dashed var(--rule-strong)', 'a box: a group of agents, sized by its count; click to open it'),
    ),
    h('p.hint', null, 'Trusting an agent more or less moves the opinions held of it.'),
  ];
}

function legendItem(style: string, text: string): HTMLElement {
  return h('li', null, h('span.soc-sw', style ? { style } : null), text);
}
