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
const SPEED = 1.6; // grid units per second
const PLATFORM_Z = 10; // platform top height, screen px before zoom
const BODY_Z = 24; // hover and packet anchor height above the floor
const MAX_PACKETS = 60;

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
}

interface Packet { from: number; to: number; start: number; tone: 'accent' | 'ok' }

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
            },
      );
    }
    bots = next;
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
      if (idle && now >= b.nextWander) {
        const span = Math.max(0.5, b.district.r - 0.7);
        b.tx = b.district.cx + (Math.random() * 2 - 1) * span;
        b.ty = b.district.cy + (Math.random() * 2 - 1) * span;
        b.nextWander = now + 3000 + Math.random() * 4000;
      }
      // Working and offline bots head for their console; idle bots wander.
      const [gx, gy] = idle ? [b.tx, b.ty] : b.home;
      const dx = gx - b.x;
      const dy = gy - b.y;
      const dist = Math.hypot(dx, dy);
      const step = SPEED * dt;
      b.moving = dist > 0.02;
      if (dist > step) {
        b.x += (dx / dist) * step;
        b.y += (dy / dist) * step;
      } else {
        b.x = gx;
        b.y = gy;
      }
      if (b.moving) {
        b.walk += dt * 9;
        const screenDx = dx - dy;
        if (Math.abs(screenDx) > 0.05) b.facing = screenDx > 0 ? 1 : -1;
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
      ctx.strokeText(b.agent.name, fx, fy + 14);
      ctx.fillStyle = palette.ink;
      ctx.fillText(b.agent.name, fx, fy + 14);
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

    for (const d of districts) drawPlatform(d);

    // Consoles and robots, back to front.
    const items: { depth: number; paint: () => void }[] = [];
    for (const b of bots) {
      const working = b.agent.status === 'working';
      const [hx, hy] = b.home;
      items.push({ depth: hx + hy - 0.01, paint: () => drawConsole(hx, hy, working) });
      items.push({ depth: b.x + b.y, paint: () => drawBot(b, now) });
    }
    items.sort((p, q) => p.depth - q.depth);
    for (const item of items) item.paint();

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

  function renderCard(): void {
    if (!selected) {
      card.hidden = true;
      cardSig = '';
      return;
    }
    const a = selected.agent;
    card.hidden = false;
    const sig = [a.status, a.model, a.work?.id ?? '', a.name, a.role].join('|');
    if (sig === cardSig) return;
    cardSig = sig;
    const cid = a.cid;
    const chat = input({ placeholder: `Message ${a.name}`, 'aria-label': `Message ${a.name}` });
    const send = (): void => {
      const text = chat.value.trim();
      if (!text) return;
      chat.value = '';
      void attempt(() => api.chat(cid, text), 'Sent');
    };
    chat.addEventListener('keydown', (ev) => {
      if (ev.key === 'Enter') send();
    });
    const work = a.work ? clip(a.work.prompt, 90) : 'Nothing right now';
    card.replaceChildren(
      h(
        'header.floor-card-head',
        null,
        h('div', null, h('strong', null, a.name), h('div.floor-card-sub', null, `${a.role} · ${a.status}`)),
        button('', () => select(null), { kind: 'ghost', icon: 'x', title: 'Close' }),
      ),
      h('dl.floor-card-facts', null, h('dt', null, 'Model'), h('dd', null, a.model), h('dt', null, 'Work'), h('dd', null, work)),
      h('div.floor-card-chat', null, chat, button('Send', send, { kind: 'primary', icon: 'send' })),
      h(
        'div.floor-card-actions',
        null,
        button('Open', () => go('agents', cid)),
        a.status === 'working' ? button('Cancel work', () => void attempt(() => api.cancel(cid), 'Cancelled'), { icon: 'ban' }) : null,
        button('Stop', () => void attempt(() => api.stop(cid), 'Stopped'), { kind: 'danger', icon: 'stop' }),
      ),
    );
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
      case 'tool_request':
        if (bot) say(bot, `→ ${str(e.data.tool) || str(e.data.name) || 'tool'}`, 'info');
        break;
      case 'work_end': {
        const status = str(e.data.status) || 'done';
        if (bot) say(bot, status, /err|fail/i.test(status) ? 'danger' : 'ok');
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
    offConversation();
    root.classList.remove('floor-view');
    root.replaceChildren();
  };
}
