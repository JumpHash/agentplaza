// The Floor: districts of robots on an isometric plaza.

import '../styles/floor.css';

import { type Agent, type Conversation, type Entry } from '../api';
import { floorName } from '../roles';
import { go } from '../router';
import { agents, on } from '../store';
import { button, clip } from '../ui/components';
import { h } from '../ui/dom';
import type { Cleanup } from './index';

const TW = 64;
const TH = 32;
const BUBBLE_MS = 4000;
const PACKET_MS = 1500;
const SPEED = 1.6; // grid units per second
const PLATFORM_Z = 10; // platform top height, screen px before zoom
const BODY_Z = 24; // hover and packet anchor height above the floor
const MAX_PACKETS = 60;
const GAP = 2; // clear ground between districts, grid units
const CLEARANCE = 0.9; // robots keep a 0.45 radius each, so centres stay this far apart
const AVOID_DEG = [0, 30, -30, 60, -60]; // headings tried when a step would clash

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
  tx: number; // wander target
  ty: number;
  facing: number; // -1 left, 1 right
  walk: number; // walk phase
  moving: boolean;
  nextWander: number;
  bubble: { text: string; until: number; tone: 'info' | 'ok' | 'danger' } | null;
  activity: string; // what the agent is doing right now, shown on its card
  talk: number | null; // cid of the robot this one is walking over to
  talkUntil: number; // give up walking over after this time
}

interface Packet { from: number; to: number; start: number; tone: 'accent' | 'ok' }

type PropKind = 'desk' | 'plant' | 'crate' | 'lamp' | 'rack' | 'shelf' | 'board' | 'bench';

interface Prop {
  kind: PropKind;
  x: number; // grid units
  y: number;
  d?: District; // the district it furnishes; none for the central square
}

/** Half-size of a district for these members: a square of console slots plus a margin. */
function half(members: Agent[]): number {
  return 2 * (Math.max(1, Math.ceil(Math.sqrt(members.length))) + 1);
}

interface Palette {
  paper: string;
  sheet: string;
  rule: string;
  ink: string;
  ink2: string;
  ink3: string;
  signal: string;
  ok: string;
  info: string;
  danger: string;
}

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
      const gx = d.cx - (side - 1) * 2 + (i % side) * 4;
      const gy = d.cy - (side - 1) * 2 + Math.floor(i / side) * 4;
      homes.set(a.cid, [d, gx, gy]);
    });
  };
  const centre = groups.get('');
  if (centre) place({ role: 'orchestrator', cx: 0, cy: 0, r: half(centre), hue: 25 }, centre);
  const roles = [...groups.keys()].filter((k) => k).sort();
  // Ring radius: neighbouring footprints keep a gap, and the ring clears the dais.
  const maxHalf = Math.max(0, ...roles.map((role) => half(groups.get(role)!)));
  const bossHalf = centre ? half(centre) : 0;
  const ringR = Math.max(
    roles.length > 1 ? (2 * maxHalf * Math.SQRT2 + GAP) / (2 * Math.sin(Math.PI / roles.length)) : 0,
    (bossHalf + maxHalf) * Math.SQRT2 + GAP,
  );
  roles.forEach((role, i) => {
    const members = groups.get(role)!;
    const angle = (i / roles.length) * Math.PI * 2 - Math.PI / 2;
    place(
      { role, cx: Math.round(Math.cos(angle) * ringR), cy: Math.round(Math.sin(angle) * ringR), r: half(members), hue: roleHue(role) },
      members,
    );
  });
  return { districts, homes };
}

function readPalette(): Palette {
  const cs = getComputedStyle(document.documentElement);
  const get = (name: string, fallback: string): string => cs.getPropertyValue(`--${name}`).trim() || fallback;
  return {
    paper: get('paper', '#f5f3ee'),
    sheet: get('sheet', '#ffffff'),
    rule: get('rule', '#d6d2c8'),
    ink: get('ink', '#1a1a1a'),
    ink2: get('ink-2', '#4a4a4a'),
    ink3: get('ink-3', '#8a8a8a'),
    signal: get('signal', '#d9482b'),
    ok: get('ok', '#2f8f5b'),
    info: get('info', '#2f6fb0'),
    danger: get('danger', '#c0392b'),
  };
}

function str(v: unknown): string {
  return typeof v === 'string' ? v : '';
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, v));
}

/** Benches on the central square, clear of the dais. */
const BENCHES: [number, number][] = [
  [2.6, 2.6],
  [-2.6, -2.6],
  [2.6, -2.6],
  [-2.6, 2.6],
];

/** Role flavour: what stands against the back edge of a district. */
const FLAVOUR: Record<string, PropKind> = {
  builder: 'rack',
  qa: 'rack',
  release: 'rack',
  security: 'rack',
  researcher: 'shelf',
  librarian: 'shelf',
  planner: 'shelf',
  architect: 'board',
  reviewer: 'board',
};

/** Furniture for every district but the orchestrator's dais, plus the square's benches. */
function buildProps(ds: District[]): Prop[] {
  const out: Prop[] = BENCHES.map(([x, y]): Prop => ({ kind: 'bench', x, y }));
  for (const d of ds) {
    if (d.role === 'orchestrator') continue;
    const e = d.r - 0.7; // inset from the platform edge
    out.push(
      { kind: 'plant', x: d.cx - e, y: d.cy - e, d },
      { kind: 'desk', x: d.cx + e, y: d.cy - e, d },
      { kind: 'desk', x: d.cx - e, y: d.cy + e, d },
      { kind: 'lamp', x: d.cx + e, y: d.cy + e, d },
      { kind: 'crate', x: d.cx - e, y: d.cy, d },
      { kind: FLAVOUR[d.role] ?? 'crate', x: d.cx, y: d.cy - e, d },
    );
    if (d.r >= 4) out.push({ kind: 'desk', x: d.cx + e, y: d.cy, d });
  }
  return out;
}

/** True if a robot standing at (x, y) would sit within clearance of another robot. */
function clashes(b: Bot, x: number, y: number, all: Bot[]): boolean {
  return all.some((o) => o !== b && Math.hypot(o.x - x, o.y - y) < CLEARANCE);
}

/** A nearby idle robot in the same district to walk over and chat with, if any. */
function neighbourOf(b: Bot, all: Bot[]): Bot | null {
  const near = all.filter(
    (o) => o !== b && o.district === b.district && o.agent.status === 'idle' && Math.hypot(o.x - b.x, o.y - b.y) < 8,
  );
  return near[Math.floor(Math.random() * near.length)] ?? null;
}

export function mountFloor2d(root: HTMLElement): Cleanup {
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
  let props: Prop[] = [];
  const byCid = new Map<number, Bot>();
  const packets: Packet[] = [];
  let pan = { x: 0, y: 0 };
  let zoom = 1;
  let width = 0;
  let height = 0;
  let dpr = 1;
  let hovered: Bot | null = null;
  let selected: Bot | null = null;
  let cardSig = '';
  let drag: { x: number; y: number; moved: number } | null = null;
  let fitted = false;
  let running = true;
  let raf = 0;
  let last = performance.now();
  const palette = readPalette();

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
          : {
              agent,
              district,
              home: [hx, hy],
              x: hx,
              y: hy,
              tx: hx,
              ty: hy,
              facing: 1,
              walk: 0,
              moving: false,
              nextWander: performance.now() + Math.random() * 3000,
              bubble: null,
              activity: 'idle',
              talk: null,
              talkUntil: 0,
            },
      );
    }
    bots = next;
    props = buildProps(districts);
    byCid.clear();
    bots.forEach((b) => byCid.set(b.agent.cid, b));
    if (selected) selected = byCid.get(selected.agent.cid) ?? null;
    if (hovered) hovered = byCid.get(hovered.agent.cid) ?? null;
    renderCard();
  }

  /** Screen px of a world point, with pan and zoom applied. */
  function project(gx: number, gy: number, z = 0): [number, number] {
    const ox = width / 2 + pan.x;
    const oy = height / 2 + pan.y;
    const [ix, iy] = iso(gx, gy, z);
    return [ox + ix * zoom, oy + iy * zoom];
  }

  function fit(): void {
    if (!districts.length) {
      pan = { x: 0, y: 0 };
      zoom = 1;
      return;
    }
    let minX = Infinity;
    let maxX = -Infinity;
    let minY = Infinity;
    let maxY = -Infinity;
    for (const d of districts) {
      const corners: [number, number][] = [
        [d.cx - d.r, d.cy - d.r],
        [d.cx + d.r, d.cy - d.r],
        [d.cx + d.r, d.cy + d.r],
        [d.cx - d.r, d.cy + d.r],
      ];
      for (const [gx, gy] of corners) {
        const [ix, iy] = iso(gx, gy);
        minX = Math.min(minX, ix);
        maxX = Math.max(maxX, ix);
        minY = Math.min(minY, iy - 60); // room for robots and bubbles
        maxY = Math.max(maxY, iy + PLATFORM_Z);
      }
    }
    const w = Math.max(1, maxX - minX);
    const hgt = Math.max(1, maxY - minY);
    zoom = clamp(Math.min((width * 0.85) / w, (height * 0.85) / hgt), 0.4, 2.5);
    pan = { x: -((minX + maxX) / 2) * zoom, y: -((minY + maxY) / 2) * zoom };
  }

  function update(dt: number, now: number): void {
    for (const b of bots) {
      const idle = b.agent.status === 'idle';
      if (!idle) b.talk = null;
      if (idle && now >= b.nextWander) {
        const friend = Math.random() < 0.3 ? neighbourOf(b, bots) : null;
        if (friend) {
          // Walk over to a neighbour; the chat bubble shows on arrival.
          b.talk = friend.agent.cid;
          b.talkUntil = now + 9000;
          b.nextWander = now + 9000;
        } else {
          // Wander within the district only.
          const span = Math.max(0.5, b.district.r - 0.7);
          b.tx = b.district.cx + (Math.random() * 2 - 1) * span;
          b.ty = b.district.cy + (Math.random() * 2 - 1) * span;
          b.nextWander = now + 3000 + Math.random() * 4000;
        }
      }
      // Working and offline bots head for their console; idle bots wander or chat.
      const partner = idle && b.talk !== null && now < b.talkUntil ? byCid.get(b.talk) : undefined;
      if (b.talk !== null && !partner) b.talk = null;
      const [hx, hy] = b.home;
      const target: [number, number] = partner ? [partner.x + 0.9, partner.y + 0.9] : idle ? [b.tx, b.ty] : [hx, hy];
      const [gx, gy] = target;
      const dx = gx - b.x;
      const dy = gy - b.y;
      const dist = Math.hypot(dx, dy);
      const reach = Math.min(SPEED * dt, dist);
      const heading = Math.atan2(dy, dx);
      // Take the heading if it is clear, else swerve left or right; if every swerve clashes, wait a frame.
      const way = AVOID_DEG.map((deg) => heading + (deg * Math.PI) / 180).find(
        (a) => !clashes(b, b.x + Math.cos(a) * reach, b.y + Math.sin(a) * reach, bots),
      );
      b.moving = dist > 0.02 && way !== undefined;
      if (way !== undefined) {
        b.x += Math.cos(way) * reach;
        b.y += Math.sin(way) * reach;
      }
      // Never leave the district.
      const { cx, cy, r } = b.district;
      b.x = clamp(b.x, cx - r + 0.5, cx + r - 0.5);
      b.y = clamp(b.y, cy - r + 0.5, cy + r - 0.5);
      if (b.moving) {
        b.walk += dt * 9;
        const screenDx = dx - dy;
        if (Math.abs(screenDx) > 0.05) b.facing = screenDx > 0 ? 1 : -1;
      }
      if (partner && !b.moving) {
        // Arrived: turn to the neighbour and show the chat bubble.
        b.facing = partner.x - b.x - (partner.y - b.y) >= 0 ? 1 : -1;
        b.bubble = { text: '· · ·', until: now + 3000, tone: 'info' };
        b.talk = null;
        b.nextWander = now + 2500 + Math.random() * 2000;
      }
    }
  }

  function fillPoly(pts: [number, number][], fill: string): void {
    ctx.beginPath();
    pts.forEach(([x, y], i) => (i === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y)));
    ctx.closePath();
    ctx.fillStyle = fill;
    ctx.fill();
  }

  function rrect(x: number, y: number, w: number, hgt: number, r: number): void {
    ctx.beginPath();
    ctx.moveTo(x + r, y);
    ctx.arcTo(x + w, y, x + w, y + hgt, r);
    ctx.arcTo(x + w, y + hgt, x, y + hgt, r);
    ctx.arcTo(x, y + hgt, x, y, r);
    ctx.arcTo(x, y, x + w, y, r);
    ctx.closePath();
  }

  function drawPlatform(d: District): void {
    const x0 = d.cx - d.r;
    const x1 = d.cx + d.r;
    const y0 = d.cy - d.r;
    const y1 = d.cy + d.r;
    const top = (x: number, y: number): [number, number] => project(x, y, PLATFORM_Z);
    const base = (x: number, y: number): [number, number] => project(x, y, 0);
    // Side faces facing the viewer.
    fillPoly([top(x1, y0), top(x1, y1), base(x1, y1), base(x1, y0)], `hsl(${d.hue} 30% 32% / 0.5)`);
    fillPoly([top(x1, y1), top(x0, y1), base(x0, y1), base(x1, y1)], `hsl(${d.hue} 30% 22% / 0.6)`);
    const topFace: [number, number][] = [top(x0, y0), top(x1, y0), top(x1, y1), top(x0, y1)];
    fillPoly(topFace, palette.sheet);
    fillPoly(topFace, `hsl(${d.hue} 30% 50% / 0.12)`);
    const isBoss = d.role === 'orchestrator';
    ctx.beginPath();
    topFace.forEach(([x, y], i) => (i === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y)));
    ctx.closePath();
    ctx.strokeStyle = isBoss ? palette.signal : palette.rule;
    ctx.lineWidth = isBoss ? 2 : 1;
    ctx.stroke();
    // Mono plaque with the role, on the back edge.
    const [px, py] = top(x0, y0);
    const label = d.role.toUpperCase();
    ctx.font = '10px ui-monospace, SFMono-Regular, Menlo, monospace';
    const w = ctx.measureText(label).width + 12;
    const plaqueY = py + 10;
    rrect(px - w / 2, plaqueY, w, 16, 3);
    ctx.fillStyle = palette.sheet;
    ctx.fill();
    ctx.strokeStyle = isBoss ? palette.signal : palette.rule;
    ctx.lineWidth = 1;
    ctx.stroke();
    ctx.fillStyle = palette.ink2;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.fillText(label, px, plaqueY + 8.5);
  }

  function drawConsole(gx: number, gy: number, working: boolean): void {
    const s = 0.32;
    const s2 = 0.2;
    const top = (x: number, y: number): [number, number] => project(x, y, PLATFORM_Z + 12);
    const base = (x: number, y: number): [number, number] => project(x, y, PLATFORM_Z);
    const A: [number, number] = [gx - s, gy - s];
    const B: [number, number] = [gx + s, gy - s];
    const C: [number, number] = [gx + s, gy + s];
    const D: [number, number] = [gx - s, gy + s];
    fillPoly([top(B[0], B[1]), top(C[0], C[1]), base(C[0], C[1]), base(B[0], B[1])], palette.ink3);
    fillPoly([top(C[0], C[1]), top(D[0], D[1]), base(D[0], D[1]), base(C[0], C[1])], palette.ink2);
    fillPoly([top(A[0], A[1]), top(B[0], B[1]), top(C[0], C[1]), top(D[0], D[1])], palette.sheet);
    const screenPts: [number, number][] = [
      top(gx - s2, gy - s2),
      top(gx + s2, gy - s2),
      top(gx + s2, gy + s2),
      top(gx - s2, gy + s2),
    ];
    ctx.save();
    ctx.globalAlpha = working ? 1 : 0.6;
    fillPoly(screenPts, working ? palette.info : palette.rule);
    ctx.restore();
  }

  /** Paths from the square to each district, and the central square with its benches. */
  function drawGround(): void {
    ctx.save();
    ctx.lineCap = 'round';
    for (const d of districts) {
      if (d.role === 'orchestrator') continue;
      const [ax, ay] = project(0, 0);
      const [bx, by] = project(d.cx, d.cy);
      ctx.beginPath();
      ctx.moveTo(ax, ay);
      ctx.lineTo(bx, by);
      ctx.globalAlpha = 0.8;
      ctx.strokeStyle = palette.rule;
      ctx.lineWidth = 11 * zoom;
      ctx.stroke();
      ctx.globalAlpha = 0.9;
      ctx.strokeStyle = palette.sheet;
      ctx.lineWidth = 6 * zoom;
      ctx.stroke();
    }
    ctx.restore();

    const S = 3.5;
    const sq: [number, number][] = [project(-S, -S), project(S, -S), project(S, S), project(-S, S)];
    fillPoly(sq, palette.sheet);
    ctx.save();
    ctx.strokeStyle = palette.rule;
    ctx.lineWidth = 1;
    ctx.beginPath();
    sq.forEach(([x, y], i) => (i === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y)));
    ctx.closePath();
    ctx.stroke();
    // A dashed inner band, like paving joints.
    const I = 2.4;
    ctx.globalAlpha = 0.6;
    ctx.setLineDash([4, 4]);
    ctx.beginPath();
    ctx.moveTo(...project(-I, -I));
    ctx.lineTo(...project(I, -I));
    ctx.lineTo(...project(I, I));
    ctx.lineTo(...project(-I, I));
    ctx.closePath();
    ctx.stroke();
    ctx.restore();
  }

  /** Warm light on the platform under a lamp post. */
  function drawLampPool(gx: number, gy: number): void {
    const [sx, sy] = project(gx, gy, PLATFORM_Z);
    ctx.save();
    ctx.translate(sx, sy);
    ctx.scale(1, 0.5);
    const g = ctx.createRadialGradient(0, 0, 0, 0, 0, 36 * zoom);
    g.addColorStop(0, 'rgba(255, 214, 140, 0.5)');
    g.addColorStop(1, 'rgba(255, 214, 140, 0)');
    ctx.fillStyle = g;
    ctx.beginPath();
    ctx.arc(0, 0, 36 * zoom, 0, Math.PI * 2);
    ctx.fill();
    ctx.restore();
  }

  /** An axis-aligned box on the grid, drawn as its three visible faces. */
  function block(gx: number, gy: number, s: number, z: number, h: number, top: string, left: string, right: string): void {
    const at = (x: number, y: number, zz: number): [number, number] => project(x, y, zz);
    const lo = z;
    const hi = z + h;
    fillPoly([at(gx + s, gy - s, hi), at(gx + s, gy + s, hi), at(gx + s, gy + s, lo), at(gx + s, gy - s, lo)], right);
    fillPoly([at(gx + s, gy + s, hi), at(gx - s, gy + s, hi), at(gx - s, gy + s, lo), at(gx + s, gy + s, lo)], left);
    fillPoly([at(gx - s, gy - s, hi), at(gx + s, gy - s, hi), at(gx + s, gy + s, hi), at(gx - s, gy + s, hi)], top);
  }

  function drawDesk(gx: number, gy: number, working: boolean, now: number): void {
    const z = PLATFORM_Z;
    block(gx, gy, 0.42, z, 9, 'hsl(32 30% 74%)', 'hsl(32 25% 52%)', 'hsl(32 25% 60%)');
    // Monitor on the desk; its screen glows while the district works.
    const mx = gx;
    const my = gy - 0.12;
    const ms = 0.12;
    block(mx, my, ms, z + 9, 11, palette.ink2, palette.ink, palette.ink2);
    const f = (x: number, zz: number): [number, number] => project(x, my + ms, zz);
    ctx.save();
    if (working) {
      ctx.shadowColor = palette.info;
      ctx.shadowBlur = 8 + 4 * Math.sin(now / 400);
    }
    fillPoly(
      [f(mx - ms * 0.8, z + 11), f(mx + ms * 0.8, z + 11), f(mx + ms * 0.8, z + 18), f(mx - ms * 0.8, z + 18)],
      working ? palette.info : palette.rule,
    );
    ctx.restore();
  }

  function drawPlant(gx: number, gy: number): void {
    const z = PLATFORM_Z;
    block(gx, gy, 0.2, z, 6, 'hsl(18 35% 52%)', 'hsl(18 30% 38%)', 'hsl(18 30% 44%)');
    const [lx, ly] = project(gx, gy, z + 6);
    const leaves: [number, number, number][] = [
      [-4, -9, -0.6],
      [4, -9, 0.6],
      [0, -13, 0],
      [-2, -6, -1.2],
      [3, -6, 1.1],
    ];
    ctx.save();
    ctx.fillStyle = 'hsl(118 38% 40%)';
    for (const [ox, oy, rot] of leaves) {
      ctx.beginPath();
      ctx.ellipse(lx + ox * zoom, ly + oy * zoom, 5 * zoom, 2.8 * zoom, rot, 0, Math.PI * 2);
      ctx.fill();
    }
    ctx.restore();
  }

  function drawCrate(gx: number, gy: number): void {
    block(gx, gy, 0.22, PLATFORM_Z, 10, 'hsl(35 45% 64%)', 'hsl(35 40% 42%)', 'hsl(35 40% 50%)');
  }

  function drawLamp(gx: number, gy: number): void {
    const [bx, by] = project(gx, gy, PLATFORM_Z);
    const [hx, hy] = project(gx, gy, PLATFORM_Z + 44);
    ctx.save();
    ctx.strokeStyle = palette.ink2;
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.moveTo(bx, by);
    ctx.lineTo(hx, hy);
    ctx.stroke();
    ctx.shadowColor = 'hsl(42 95% 62%)';
    ctx.shadowBlur = 10;
    ctx.fillStyle = 'hsl(42 95% 74%)';
    ctx.beginPath();
    ctx.arc(hx, hy, 2.8, 0, Math.PI * 2);
    ctx.fill();
    ctx.restore();
  }

  /** Server rack for builder, qa, release and security: blinking LEDs on the front. */
  function drawRack(gx: number, gy: number, now: number): void {
    const z = PLATFORM_Z;
    block(gx, gy, 0.22, z, 26, 'hsl(220 10% 34%)', 'hsl(220 10% 16%)', 'hsl(220 10% 24%)');
    const front = gy + 0.22;
    for (let row = 0; row < 5; row++) {
      for (let col = 0; col < 2; col++) {
        const [lx, ly] = project(gx - 0.1 + col * 0.12, front, z + 4 + row * 4.5);
        const lit = Math.sin(now / 170 + row * 1.7 + col * 2.9) > 0.1;
        ctx.beginPath();
        ctx.arc(lx, ly, 1.3, 0, Math.PI * 2);
        ctx.fillStyle = lit ? (col ? palette.ok : palette.info) : palette.ink3;
        ctx.fill();
      }
    }
  }

  /** Bookshelf for researcher, librarian and planner: coloured spines. */
  function drawShelf(gx: number, gy: number): void {
    const z = PLATFORM_Z;
    block(gx, gy, 0.3, z, 24, 'hsl(28 30% 62%)', 'hsl(28 28% 40%)', 'hsl(28 28% 48%)');
    const front = gy + 0.3;
    for (let row = 0; row < 2; row++) {
      const top = z + 4 + row * 10;
      for (let i = 0; i < 6; i++) {
        const x0 = gx - 0.26 + i * 0.09;
        const quad: [number, number][] = [
          project(x0, front, top + 7),
          project(x0 + 0.07, front, top + 7),
          project(x0 + 0.07, front, top),
          project(x0, front, top),
        ];
        fillPoly(quad, `hsl(${(i * 53 + row * 140) % 360} 45% ${row ? 46 : 58}%)`);
      }
    }
  }

  /** Whiteboard for architect and reviewer, with a few marks on it. */
  function drawBoard(gx: number, gy: number, working: boolean): void {
    const z = PLATFORM_Z;
    ctx.save();
    ctx.strokeStyle = palette.ink2;
    ctx.lineWidth = 1.2;
    for (const x of [gx - 0.5, gx + 0.5]) {
      const [px, py] = project(x, gy, z);
      const [px2, py2] = project(x, gy, z + 28);
      ctx.beginPath();
      ctx.moveTo(px, py);
      ctx.lineTo(px2, py2);
      ctx.stroke();
    }
    const board: [number, number][] = [
      project(gx - 0.55, gy, z + 26),
      project(gx + 0.55, gy, z + 26),
      project(gx + 0.55, gy, z + 12),
      project(gx - 0.55, gy, z + 12),
    ];
    fillPoly(board, palette.sheet);
    ctx.strokeStyle = palette.ink3;
    ctx.lineWidth = 1;
    ctx.beginPath();
    board.forEach(([x, y], i) => (i === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y)));
    ctx.closePath();
    ctx.stroke();
    ctx.strokeStyle = working ? palette.info : palette.signal;
    ctx.beginPath();
    for (let i = 0; i < 3; i++) {
      const lineZ = z + 22 - i * 3.5;
      const [sx, sy] = project(gx - 0.4, gy, lineZ);
      const [ex, ey] = project(gx - 0.4 + 0.25 + i * 0.12, gy, lineZ);
      ctx.moveTo(sx, sy);
      ctx.lineTo(ex, ey);
    }
    ctx.stroke();
    ctx.restore();
  }

  /** A low bench on the square, on the ground. */
  function drawBench(gx: number, gy: number): void {
    block(gx, gy, 0.3, 0, 6, 'hsl(30 22% 66%)', 'hsl(30 20% 44%)', 'hsl(30 20% 52%)');
  }

  function paintProp(p: Prop, now: number): void {
    const working = bots.some((b) => b.district === p.d && b.agent.status === 'working');
    switch (p.kind) {
      case 'desk':
        drawDesk(p.x, p.y, working, now);
        break;
      case 'plant':
        drawPlant(p.x, p.y);
        break;
      case 'crate':
        drawCrate(p.x, p.y);
        break;
      case 'lamp':
        drawLamp(p.x, p.y);
        break;
      case 'rack':
        drawRack(p.x, p.y, now);
        break;
      case 'shelf':
        drawShelf(p.x, p.y);
        break;
      case 'board':
        drawBoard(p.x, p.y, working);
        break;
      case 'bench':
        drawBench(p.x, p.y);
        break;
    }
  }

  /** Slowly turning ring above the orchestrator's dais. */
  function drawHolo(now: number): void {
    const boss = districts.find((d) => d.role === 'orchestrator');
    if (!boss) return;
    const [hx, hy] = project(boss.cx, boss.cy, PLATFORM_Z + 60 + Math.sin(now / 900) * 2);
    const R = 22 * zoom;
    const t = now / 2600;
    ctx.save();
    ctx.globalAlpha = 0.7;
    ctx.strokeStyle = palette.signal;
    ctx.lineWidth = 1.4;
    ctx.setLineDash([6 * zoom, 5 * zoom]);
    ctx.lineDashOffset = -now / 60;
    ctx.beginPath();
    ctx.ellipse(hx, hy, R, R * 0.36, t, 0, Math.PI * 2);
    ctx.stroke();
    ctx.globalAlpha = 0.4;
    ctx.strokeStyle = palette.info;
    ctx.setLineDash([]);
    ctx.beginPath();
    ctx.ellipse(hx, hy, R * 0.66, R * 0.24, -t * 1.6, 0, Math.PI * 2);
    ctx.stroke();
    ctx.restore();
  }

  function drawBot(b: Bot, now: number): void {
    const [fx, fy] = project(b.x, b.y, PLATFORM_Z);
    const status = b.agent.status;
    const tone = status === 'working' ? palette.info : status === 'idle' ? palette.ok : palette.ink3;
    const hue = b.district.hue;
    const swing = b.moving ? Math.sin(b.walk) * 3 : 0;
    const tap = (side: number): number => (status === 'working' ? Math.sin(now / 110 + (side > 0 ? Math.PI : 0)) * 2 : 0);
    ctx.save();
    ctx.globalAlpha = status === 'offline' ? 0.4 : 1;
    ctx.translate(fx, fy);
    ctx.scale(zoom * b.facing, zoom);

    // Shadow.
    ctx.save();
    ctx.globalAlpha *= 0.18;
    ctx.fillStyle = palette.ink;
    ctx.beginPath();
    ctx.ellipse(0, 0, 9, 4.5, 0, 0, Math.PI * 2);
    ctx.fill();
    ctx.restore();

    // Legs swing while walking.
    ctx.strokeStyle = palette.ink2;
    ctx.lineWidth = 2.5;
    ctx.lineCap = 'round';
    for (const side of [-1, 1]) {
      const off = side === -1 ? swing : -swing;
      ctx.beginPath();
      ctx.moveTo(side * 3.5, -12);
      ctx.lineTo(side * 3.5 + off, 0);
      ctx.stroke();
    }

    // Body with a role-hue gradient and a chest panel.
    const grad = ctx.createLinearGradient(0, -30, 0, -12);
    grad.addColorStop(0, `hsl(${hue} 55% 66%)`);
    grad.addColorStop(1, `hsl(${hue} 50% 44%)`);
    rrect(-7, -30, 14, 18, 4);
    ctx.fillStyle = grad;
    ctx.fill();
    ctx.strokeStyle = palette.ink2;
    ctx.lineWidth = 0.8;
    ctx.stroke();
    rrect(-4, -25, 8, 7, 1.5);
    ctx.fillStyle = palette.sheet;
    ctx.fill();
    ctx.fillStyle = palette.ink;
    ctx.font = 'bold 7px ui-monospace, SFMono-Regular, Menlo, monospace';
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.fillText(b.agent.role.charAt(0).toUpperCase() || '?', 0, -21.5);

    // Arms, tapping while working.
    ctx.strokeStyle = `hsl(${hue} 45% 36%)`;
    ctx.lineWidth = 2;
    for (const side of [-1, 1]) {
      ctx.beginPath();
      ctx.moveTo(side * 7, -27);
      ctx.lineTo(side * 10, -19 + tap(side));
      ctx.stroke();
    }

    // Head with a dark visor and status eyes.
    rrect(-6, -41, 12, 10, 3);
    ctx.fillStyle = palette.sheet;
    ctx.fill();
    ctx.strokeStyle = palette.ink2;
    ctx.stroke();
    rrect(-4.5, -38.5, 9, 5, 1.5);
    ctx.fillStyle = palette.ink;
    ctx.fill();
    ctx.fillStyle = tone;
    for (const ex of [-2, 2]) {
      ctx.beginPath();
      ctx.arc(ex, -36, 1.1, 0, Math.PI * 2);
      ctx.fill();
    }

    // Antenna light.
    ctx.strokeStyle = palette.ink2;
    ctx.lineWidth = 0.8;
    ctx.beginPath();
    ctx.moveTo(0, -41);
    ctx.lineTo(0, -46);
    ctx.stroke();
    ctx.beginPath();
    ctx.arc(0, -47, 1.8, 0, Math.PI * 2);
    ctx.fillStyle = tone;
    ctx.fill();

    // A glowing token above the head while working.
    if (status === 'working') {
      const pulse = 0.5 + 0.5 * Math.sin(now / 300);
      ctx.shadowColor = palette.info;
      ctx.shadowBlur = 8 + 6 * pulse;
      ctx.beginPath();
      ctx.arc(0, -56, 2.6 + pulse * 0.6, 0, Math.PI * 2);
      ctx.fillStyle = palette.info;
      ctx.fill();
      ctx.shadowBlur = 0;
    }
    ctx.restore();
  }

  function drawOverlay(b: Bot, now: number): void {
    const [fx, fy] = project(b.x, b.y, PLATFORM_Z);
    if (b.bubble && now < b.bubble.until) {
      const [bx, by] = project(b.x, b.y, PLATFORM_Z + 52);
      const alpha = clamp((b.bubble.until - now) / 800, 0, 1);
      const text = clip(b.bubble.text, 30);
      ctx.font = '11px ui-sans-serif, system-ui, sans-serif';
      const w = ctx.measureText(text).width + 14;
      const bh = 18;
      ctx.save();
      ctx.globalAlpha = alpha;
      rrect(bx - w / 2, by - bh - 6, w, bh, 6);
      ctx.fillStyle = palette.sheet;
      ctx.fill();
      ctx.strokeStyle = b.bubble.tone === 'danger' ? palette.danger : b.bubble.tone === 'ok' ? palette.ok : palette.info;
      ctx.lineWidth = 1;
      ctx.stroke();
      ctx.fillStyle = palette.ink;
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      ctx.fillText(text, bx, by - bh / 2 - 6);
      ctx.restore();
    }
    if (b === hovered || b === selected || zoom > 0.9) {
      ctx.font = '600 11px ui-sans-serif, system-ui, sans-serif';
      ctx.textAlign = 'center';
      ctx.textBaseline = 'top';
      ctx.lineJoin = 'round';
      ctx.lineWidth = 3;
      ctx.strokeStyle = palette.paper;
      ctx.strokeText(floorName(b.agent), fx, fy + 14);
      ctx.fillStyle = palette.ink;
      ctx.fillText(floorName(b.agent), fx, fy + 14);
    }
  }

  function drawPackets(now: number): void {
    for (let i = packets.length - 1; i >= 0; i--) {
      const p = packets[i];
      if (!p) continue;
      const t = (now - p.start) / PACKET_MS;
      if (t >= 1) {
        packets.splice(i, 1);
        continue;
      }
      const a = byCid.get(p.from);
      const b = byCid.get(p.to);
      if (!a || !b) continue;
      const [x0, y0] = project(a.x, a.y, PLATFORM_Z + BODY_Z);
      const [x2, y2] = project(b.x, b.y, PLATFORM_Z + BODY_Z);
      const mx = (x0 + x2) / 2;
      const my = (y0 + y2) / 2 - 60 * zoom;
      const u = 1 - t;
      const x = u * u * x0 + 2 * u * t * mx + t * t * x2;
      const y = u * u * y0 + 2 * u * t * my + t * t * y2;
      const color = p.tone === 'ok' ? palette.ok : palette.signal;
      ctx.save();
      ctx.shadowColor = color;
      ctx.shadowBlur = 12;
      ctx.fillStyle = color;
      ctx.beginPath();
      ctx.arc(x, y, 3.5, 0, Math.PI * 2);
      ctx.fill();
      ctx.restore();
    }
  }

  function draw(now: number): void {
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.globalAlpha = 1;
    ctx.fillStyle = palette.paper;
    ctx.fillRect(0, 0, width, height);

    // Faint floor grid.
    let extent = 8;
    for (const d of districts) extent = Math.max(extent, Math.abs(d.cx) + d.r + 3, Math.abs(d.cy) + d.r + 3);
    ctx.save();
    ctx.globalAlpha = 0.3;
    ctx.strokeStyle = palette.rule;
    ctx.lineWidth = 1;
    ctx.beginPath();
    for (let g = -extent; g <= extent; g++) {
      const [ax, ay] = project(g, -extent);
      const [bx, by] = project(g, extent);
      ctx.moveTo(ax, ay);
      ctx.lineTo(bx, by);
      const [cx, cy] = project(-extent, g);
      const [dx, dy] = project(extent, g);
      ctx.moveTo(cx, cy);
      ctx.lineTo(dx, dy);
    }
    ctx.stroke();
    ctx.restore();

    drawGround();
    for (const d of districts) drawPlatform(d);
    for (const p of props) if (p.kind === 'lamp') drawLampPool(p.x, p.y);

    // Props, consoles and robots, back to front.
    const items: { depth: number; paint: () => void }[] = [];
    for (const p of props) items.push({ depth: p.x + p.y, paint: () => paintProp(p, now) });
    for (const b of bots) {
      const working = b.agent.status === 'working';
      const [hx, hy] = b.home;
      items.push({ depth: hx + hy - 0.01, paint: () => drawConsole(hx, hy, working) });
      items.push({ depth: b.x + b.y, paint: () => drawBot(b, now) });
    }
    items.sort((p, q) => p.depth - q.depth);
    for (const item of items) item.paint();

    drawHolo(now);
    drawPackets(now);
    for (const b of bots) drawOverlay(b, now);
    positionCard();
  }

  function sendPacket(from: number, to: number, tone: 'accent' | 'ok'): void {
    if (from === to) return;
    packets.push({ from, to, start: performance.now(), tone });
    if (packets.length > MAX_PACKETS) packets.shift();
  }

  function say(bot: Bot, text: string, tone: 'info' | 'ok' | 'danger'): void {
    bot.bubble = { text, until: performance.now() + BUBBLE_MS, tone };
  }

  function findBot(mx: number, my: number): Bot | null {
    let best: Bot | null = null;
    let bestDist = 22 * zoom;
    for (const b of bots) {
      const [fx, fy] = project(b.x, b.y, PLATFORM_Z);
      const dist = Math.hypot(mx - fx, my - (fy - 24 * zoom));
      if (dist < bestDist) {
        bestDist = dist;
        best = b;
      }
    }
    return best;
  }

  function select(bot: Bot | null): void {
    selected = bot;
    cardSig = '';
    renderCard();
  }

  /** What the card shows: offline overrides whatever the transcript last said. */
  const shown = (b: Bot): string => (b.agent.status === 'offline' ? 'offline' : b.activity);

  const nameOf = (cid: unknown): string => {
    const b = typeof cid === 'number' ? byCid.get(cid) : undefined;
    return b ? floorName(b.agent) : String(cid ?? '?');
  };

  function setActivity(bot: Bot, text: string): void {
    bot.activity = text;
    if (bot === selected) renderCard();
  }

  /** The cid a tool call hands a message or a task to, if any. */
  function recipientOf(tool: string, inp: Record<string, unknown>): number | null {
    const to = inp.to_cid ?? inp.assignee;
    const sends = tool === 'plaza_send_message';
    const assigns = tool === 'plaza_assign' || tool === 'plaza_post_task';
    return (sends || assigns) && typeof to === 'number' ? to : null;
  }

  /** The one line that says what a tool call is doing. */
  function toolActivity(tool: string, inp: Record<string, unknown>): string {
    if (tool === 'bash') return `$ ${clip(str(inp.command).split('\n')[0] ?? '', 120)}`;
    if (tool === 'plaza_send_message') return `✉ to ${nameOf(inp.to_cid)}: ${clip(str(inp.text), 100)}`;
    if (tool === 'plaza_assign' || (tool === 'plaza_post_task' && inp.assignee != null)) {
      return `→ task for ${nameOf(inp.assignee ?? inp.to_cid)}: ${str(inp.title)}`;
    }
    if (tool === 'share_file') return `⇪ shared ${str(inp.path)}`;
    const target = str(inp.path) || str(inp.url);
    return target ? `→ ${tool} ${clip(target, 80)}` : `→ ${tool}`;
  }

  function renderCard(): void {
    if (!selected) {
      card.hidden = true;
      cardSig = '';
      return;
    }
    const a = selected.agent;
    card.hidden = false;
    const line = shown(selected);
    const name = floorName(a);
    const sig = [name, line].join('|');
    if (sig === cardSig) return;
    cardSig = sig;
    card.replaceChildren(h('header.floor-card-head', null, h('strong', null, name)), h('div.floor-card-activity', null, line));
  }

  function positionCard(): void {
    if (!selected || card.hidden) return;
    const [sx, sy] = project(selected.x, selected.y, PLATFORM_Z + 40);
    const left = clamp(sx + 24, 8, Math.max(8, width - 308));
    const top = clamp(sy - 120, 8, Math.max(8, height - 260));
    card.style.left = `${left}px`;
    card.style.top = `${top}px`;
  }

  // Pointer: drag to pan, click to select, hover to highlight.
  const local = (ev: MouseEvent): [number, number] => {
    const r = canvas.getBoundingClientRect();
    return [ev.clientX - r.left, ev.clientY - r.top];
  };
  canvas.addEventListener('pointerdown', (ev) => {
    drag = { x: ev.clientX, y: ev.clientY, moved: 0 };
    canvas.setPointerCapture(ev.pointerId);
    canvas.style.cursor = 'grabbing';
  });
  canvas.addEventListener('pointermove', (ev) => {
    if (drag) {
      const dx = ev.clientX - drag.x;
      const dy = ev.clientY - drag.y;
      drag.moved += Math.abs(dx) + Math.abs(dy);
      drag.x = ev.clientX;
      drag.y = ev.clientY;
      pan = { x: pan.x + dx, y: pan.y + dy };
      return;
    }
    const [mx, my] = local(ev);
    hovered = findBot(mx, my);
    canvas.style.cursor = hovered ? 'pointer' : 'grab';
  });
  canvas.addEventListener('pointerup', (ev) => {
    if (!drag) return;
    const moved = drag.moved;
    drag = null;
    canvas.style.cursor = 'grab';
    if (moved < 4) {
      const [mx, my] = local(ev);
      select(findBot(mx, my));
    }
  });
  canvas.addEventListener('pointercancel', () => {
    drag = null;
    canvas.style.cursor = 'grab';
  });
  canvas.addEventListener('pointerleave', () => {
    hovered = null;
  });
  canvas.addEventListener(
    'wheel',
    (ev) => {
      ev.preventDefault();
      const [mx, my] = local(ev);
      const ox = width / 2 + pan.x;
      const oy = height / 2 + pan.y;
      const ix = (mx - ox) / zoom;
      const iy = (my - oy) / zoom;
      zoom = clamp(zoom * Math.exp(-ev.deltaY * 0.0015), 0.4, 2.5);
      pan = { x: mx - ix * zoom - width / 2, y: my - iy * zoom - height / 2 };
    },
    { passive: false },
  );

  const resize = (): void => {
    dpr = window.devicePixelRatio || 1;
    width = canvas.clientWidth;
    height = canvas.clientHeight;
    canvas.width = Math.round(width * dpr);
    canvas.height = Math.round(height * dpr);
  };
  const ro = new ResizeObserver(() => resize());
  ro.observe(canvas);
  resize();

  // Live data.
  const offAgents = agents.watch((list) => {
    sync(list);
    if (!fitted && list.length) {
      fit();
      fitted = true;
    }
  });
  const offTranscript = on('transcript', (data) => {
    const e = data as Entry;
    const bot = byCid.get(e.cid);
    switch (e.kind) {
      case 'tool_request': {
        const tool = str(e.data.tool) || str(e.data.name) || 'tool';
        const inp = (e.data.input ?? {}) as Record<string, unknown>;
        if (bot) {
          say(bot, `→ ${tool}`, 'info');
          setActivity(bot, toolActivity(tool, inp));
        }
        const to = recipientOf(tool, inp);
        if (to !== null) sendPacket(e.cid, to, 'accent');
        break;
      }
      case 'model_request':
        if (bot) setActivity(bot, 'thinking…');
        break;
      case 'work_end': {
        const status = str(e.data.status) || 'done';
        if (bot) {
          say(bot, status, /err|fail/i.test(status) ? 'danger' : 'ok');
          setActivity(bot, 'idle');
        }
        break;
      }
      case 'work_start': {
        const origin = (e.data.origin ?? {}) as { from_cid?: unknown };
        if (typeof origin.from_cid === 'number') sendPacket(origin.from_cid, e.cid, 'accent');
        else if (e.data.kind === 'task') {
          const boss = bots.find((b) => b.agent.tier === 'orchestrator');
          if (boss) sendPacket(boss.agent.cid, e.cid, 'accent');
        }
        break;
      }
      default:
        break;
    }
  });
  const offDelta = on('delta', (data) => {
    const bot = byCid.get((data as { cid: number }).cid);
    if (bot) setActivity(bot, 'thinking…');
  });
  canvas.addEventListener('dblclick', (ev) => {
    const [mx, my] = local(ev);
    const bot = findBot(mx, my);
    if (bot) go('agents', bot.agent.cid);
  });
  const offConversation = on('conversation', (data) => {
    const c = data as Conversation;
    sendPacket(c.from_cid, c.to_cid, 'ok');
  });

  // Loop.
  const tick = (now: number): void => {
    if (!running) return;
    const dt = Math.min(0.05, (now - last) / 1000);
    last = now;
    update(dt, now);
    draw(now);
    raf = requestAnimationFrame(tick);
  };
  raf = requestAnimationFrame(tick);

  return () => {
    running = false;
    cancelAnimationFrame(raf);
    ro.disconnect();
    offAgents();
    offTranscript();
    offDelta();
    offConversation();
    root.classList.remove('floor-view');
    root.replaceChildren();
  };
}
