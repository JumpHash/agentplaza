// The Floor without WebGL: a top-down 2D board. Each room is a rectangle on its slot of the machine's board
// (the same layout the 3D plaza uses), and each agent is a dot coloured by its role hue, with a ring while working.

import '../styles/floor.css';

import { api, type Agent, type Conversation, type Entry, type FleetResponse } from '../api';
import { floorName } from '../roles';
import { go } from '../router';
import { agents, on } from '../store';
import { button, clip } from '../ui/components';
import { h } from '../ui/dom';
import { requestLayout } from './engine-mount';
import { boardOf, type BoardParams as Params } from './floor-common';
import type { Cleanup } from './index';

/** A room as the 2D floor draws it: its role, centre (cx, cy), cell size (w by d), hue and shape. */
interface BoardDistrict {
  role: string;
  cx: number;
  cy: number;
  w: number;
  d: number;
  hue: number;
  shape: 'round' | 'square';
}

/** The board's footprint, for framing. */
interface Bounds {
  cx: number;
  cz: number;
  w: number;
  d: number;
}

/** The shared layout's JSON (apz-board's Layout): the parts the 2D floor reads. */
interface SharedLayout {
  bounds: Bounds;
  rooms: { role: string; round: boolean; hue: number; centre: [number, number]; half: [number, number] }[];
  homes: { agent: string; room: number; at: [number, number] }[];
}

const BUBBLE_MS = 4000;
const PACKET_MS = 1500;
const SPEED = 1.6; // grid units per second
const DOT = 0.45; // a robot's radius, grid units
const CLEARANCE = 0.9; // robots keep a 0.45 radius each, so centres stay this far apart
const AVOID_DEG = [0, 30, -30, 60, -60]; // headings tried when a step would clash
const MAX_PACKETS = 60;
const ZOOM_MIN = 0.2;
const ZOOM_MAX = 14;
const POLL_MS = 5000;
const DEFAULT_BOARD: { name: string; params: Params } = { name: 'board/workstation', params: { gpus: 1, studio: 0 } };

interface Bot {
  agent: Agent;
  district: BoardDistrict;
  home: [number, number]; // console position, grid units
  x: number; // current position, grid units
  y: number;
  tx: number; // wander target
  ty: number;
  moving: boolean;
  nextWander: number;
  bubble: { text: string; until: number; tone: 'info' | 'ok' | 'danger' } | null;
  activity: string; // what the agent is doing right now, shown on its card
  talk: number | null; // cid of the robot this one is walking over to
  talkUntil: number; // give up walking over after this time
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
  return Math.min(Math.max(v, lo), hi);
}

/** Robots keep clear of each other: a step that clashes is retried at a few headings. */
function clashes(b: Bot, x: number, y: number, all: Bot[]): boolean {
  return all.some((o) => o !== b && Math.hypot(o.x - x, o.y - y) < CLEARANCE);
}

/** The nearest idle robot in the same room, for a chat. */
function neighbourOf(b: Bot, all: Bot[]): Bot | null {
  let best: Bot | null = null;
  let bestDist = 6;
  for (const o of all) {
    if (o === b || o.district !== b.district || o.agent.status !== 'idle') continue;
    const dist = Math.hypot(o.x - b.x, o.y - b.y);
    if (dist < bestDist) {
      bestDist = dist;
      best = o;
    }
  }
  return best;
}

/** Shows the machine's board as a top-down plan: rooms as rectangles, agents as dots. */
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
  let alive = true;
  let lastList: Agent[] = [];
  let members = new Set<number>(); // the machine's agents (the fleet's); empty until a fleet read names them: no dots before
  let fleetRead = false;
  let board: { key: string; kind: string; params: Params } | null = null;
  let bounds: Bounds | null = null;
  let laying = 0; // the latest layout request: an older answer is dropped
  let refit = false; // a new board frames itself once its layout lands
  let districts: BoardDistrict[] = [];
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
  let running = true;
  let raf = 0;
  let last = performance.now();
  const palette = readPalette();

  /**
   * Lays the agents on the board and places their dots: homes follow the layout, robots keep their positions. The layout
   * is the engine's (apz-board's, from the layout worker), so the 2D and the 3D floors seat everyone alike.
   */
  function sync(list: Agent[]): void {
    lastList = list;
    if (!board) return; // the board comes with the machine; until then there is nothing to lay out
    const mine = list.filter((a) => members.has(a.cid));
    const agents = mine
      .map((a): [number, string] => [a.cid, a.tier === 'orchestrator' ? 'orchestrator' : a.role])
      .sort((x, y) => x[0] - y[0]);
    const seq = ++laying;
    void requestLayout({ kind: board.kind, gpus: board.params.gpus ?? 0, agents }).then((json) => {
      if (alive && seq === laying && json) place(mine, json as SharedLayout);
    });
  }

  /** The shared layout's rooms and homes as the dots and rooms the 2D floor draws. */
  function place(mine: Agent[], laid: SharedLayout): void {
    districts = laid.rooms.map((r) => ({
      role: r.role || 'other',
      cx: r.centre[0],
      cy: r.centre[1],
      w: r.half[0] * 2,
      d: r.half[1] * 2,
      hue: r.hue,
      shape: r.round ? 'round' : 'square',
    }));
    bounds = laid.bounds;
    if (refit) {
      refit = false;
      fit();
    }
    const homes = new Map(laid.homes.map((h) => [Number(h.agent), h] as const));
    const next: Bot[] = [];
    for (const agent of mine) {
      const home = homes.get(agent.cid);
      const district = home ? districts[home.room] : undefined;
      if (!home || !district) continue;
      const [hx, hy] = home.at;
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
    byCid.clear();
    bots.forEach((b) => byCid.set(b.agent.cid, b));
    if (selected) selected = byCid.get(selected.agent.cid) ?? null;
    if (hovered) hovered = byCid.get(hovered.agent.cid) ?? null;
    renderCard();
  }

  /** Screen px of a grid point, with pan and zoom applied. */
  function project(gx: number, gy: number): [number, number] {
    return [width / 2 + pan.x + gx * zoom, height / 2 + pan.y + gy * zoom];
  }

  /** Frames the whole board in the view. */
  function fit(): void {
    if (!bounds) {
      pan = { x: 0, y: 0 };
      zoom = 1;
      return;
    }
    zoom = clamp(Math.min((width * 0.85) / Math.max(1, bounds.w), (height * 0.85) / Math.max(1, bounds.d)), ZOOM_MIN, ZOOM_MAX);
    pan = { x: -bounds.cx * zoom, y: -bounds.cz * zoom };
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
          // Wander within the room only.
          const { cx, cy, w, d } = b.district;
          b.tx = cx + (Math.random() * 2 - 1) * Math.max(0.5, w / 2 - 0.7);
          b.ty = cy + (Math.random() * 2 - 1) * Math.max(0.5, d / 2 - 0.7);
          b.nextWander = now + 3000 + Math.random() * 4000;
        }
      }
      // Working and offline bots head for their console; idle bots wander or chat.
      const partner = idle && b.talk !== null && now < b.talkUntil ? byCid.get(b.talk) : undefined;
      if (b.talk !== null && !partner) b.talk = null;
      const [hx, hy] = b.home;
      const target: [number, number] = partner ? [partner.x + 0.9, partner.y] : idle ? [b.tx, b.ty] : [hx, hy];
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
      // Never leave the room.
      const { cx, cy, w, d } = b.district;
      b.x = clamp(b.x, cx - w / 2 + DOT, cx + w / 2 - DOT);
      b.y = clamp(b.y, cy - d / 2 + DOT, cy + d / 2 - DOT);
      if (partner && !b.moving) {
        // Arrived: show the chat bubble.
        b.bubble = { text: '· · ·', until: now + 3000, tone: 'info' };
        b.talk = null;
        b.nextWander = now + 2500 + Math.random() * 2000;
      }
    }
  }

  function fillPlain(fill: string): void {
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

  /** A room on its slot: a rectangle (or ellipse for round rooms) with its role's name in the corner. */
  function drawRoom(d: BoardDistrict): void {
    const [x0, y0] = project(d.cx - d.w / 2, d.cy - d.d / 2);
    const [x1, y1] = project(d.cx + d.w / 2, d.cy + d.d / 2);
    const boss = d.role === 'orchestrator';
    ctx.beginPath();
    if (d.shape === 'round') ctx.ellipse((x0 + x1) / 2, (y0 + y1) / 2, (x1 - x0) / 2, (y1 - y0) / 2, 0, 0, Math.PI * 2);
    else ctx.rect(x0, y0, x1 - x0, y1 - y0);
    fillPlain(`hsl(${d.hue} 35% 55% / 0.2)`);
    ctx.strokeStyle = boss ? palette.signal : `hsl(${d.hue} 40% 42%)`;
    ctx.lineWidth = boss ? 2 : 1;
    ctx.stroke();
    ctx.font = '10px ui-monospace, SFMono-Regular, Menlo, monospace';
    ctx.fillStyle = palette.ink2;
    ctx.textAlign = 'left';
    ctx.textBaseline = 'top';
    ctx.fillText(d.role.toUpperCase(), x0 + 4, y0 + 4);
  }

  /** A robot as a dot: its role hue, a pulsing ring while working, grey when offline. */
  function drawBot(b: Bot, now: number): void {
    const [fx, fy] = project(b.x, b.y);
    const status = b.agent.status;
    const r = Math.max(3, DOT * zoom);
    ctx.save();
    ctx.globalAlpha = status === 'offline' ? 0.4 : 1;
    if (status === 'working') {
      const pulse = 0.5 + 0.5 * Math.sin(now / 300);
      ctx.beginPath();
      ctx.arc(fx, fy, r + 2 + pulse * 2, 0, Math.PI * 2);
      ctx.strokeStyle = palette.info;
      ctx.lineWidth = 1.5;
      ctx.stroke();
    }
    ctx.beginPath();
    ctx.arc(fx, fy, r, 0, Math.PI * 2);
    ctx.fillStyle = status === 'offline' ? palette.ink3 : `hsl(${b.district.hue} 60% 50%)`;
    ctx.fill();
    ctx.strokeStyle = palette.sheet;
    ctx.lineWidth = 1;
    ctx.stroke();
    ctx.restore();
  }

  function drawOverlay(b: Bot, now: number): void {
    const [fx, fy] = project(b.x, b.y);
    if (b.bubble && now < b.bubble.until) {
      const alpha = clamp((b.bubble.until - now) / 800, 0, 1);
      const text = clip(b.bubble.text, 30);
      ctx.font = '11px ui-sans-serif, system-ui, sans-serif';
      const w = ctx.measureText(text).width + 14;
      const bh = 18;
      const bx = fx;
      const by = fy - 12;
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
    if (b === hovered || b === selected || zoom > 1.2) {
      ctx.font = '600 11px ui-sans-serif, system-ui, sans-serif';
      ctx.textAlign = 'center';
      ctx.textBaseline = 'top';
      ctx.lineJoin = 'round';
      ctx.lineWidth = 3;
      ctx.strokeStyle = palette.paper;
      ctx.strokeText(floorName(b.agent), fx, fy + 8);
      ctx.fillStyle = palette.ink;
      ctx.fillText(floorName(b.agent), fx, fy + 8);
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
      const [x0, y0] = project(a.x, a.y);
      const [x2, y2] = project(b.x, b.y);
      const mx = (x0 + x2) / 2;
      const my = (y0 + y2) / 2 - 1.5 * zoom;
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

    // The board itself: its footprint under the rooms.
    if (bounds) {
      const [x0, y0] = project(bounds.cx - bounds.w / 2, bounds.cz - bounds.d / 2);
      const [x1, y1] = project(bounds.cx + bounds.w / 2, bounds.cz + bounds.d / 2);
      ctx.fillStyle = palette.sheet;
      ctx.fillRect(x0, y0, x1 - x0, y1 - y0);
      ctx.strokeStyle = palette.rule;
      ctx.lineWidth = 1;
      ctx.strokeRect(x0, y0, x1 - x0, y1 - y0);
    }

    for (const d of districts) drawRoom(d);
    for (const b of bots) drawBot(b, now);
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
    let bestDist = Math.max(14, DOT * zoom * 2);
    for (const b of bots) {
      const [fx, fy] = project(b.x, b.y);
      const dist = Math.hypot(mx - fx, my - fy);
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
    const [sx, sy] = project(selected.x, selected.y);
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
      const gx = (mx - ox) / zoom;
      const gy = (my - oy) / zoom;
      zoom = clamp(zoom * Math.exp(-ev.deltaY * 0.0015), ZOOM_MIN, ZOOM_MAX);
      pan = { x: mx - gx * zoom - width / 2, y: my - gy * zoom - height / 2 };
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

  /** Shows the named board with its params, building it unless it is the one already shown, then lays the agents on it. */
  function showBoard(name: string, params: Params): void {
    const key = `${name}|${JSON.stringify(params)}`;
    const changed = board?.key !== key;
    if (changed) board = { key, kind: name.replace(/^board\//, ''), params };
    if (changed) refit = true;
    sync(lastList);
  }

  /** The machine the Floor shows: the hub, with its agents and board; the workstation board when the fleet cannot be read. */
  async function refresh(): Promise<void> {
    let fleet: FleetResponse | null = null;
    try {
      fleet = await api.fleet();
    } catch {
      fleet = null; // the machines list below
    }
    const m = fleet?.machines.find((x) => x.role === 'hub') ?? fleet?.machines[0];
    if (!alive) return;
    if (m) {
      members = new Set(m.agents); // the members only change on a fleet read: a failed read keeps the last known ones
      fleetRead = true;
      const { name, params } = boardOf(m);
      showBoard(name, params);
      return;
    }
    if (fleetRead) return; // the last known machine stays
    try {
      const nodes = await api.machines();
      const node = nodes.find((n) => n.id === 'n0') ?? nodes[0];
      if (alive && node && !fleetRead) {
        const { name, params } = boardOf(node);
        showBoard(name, params);
        return;
      }
    } catch {
      // the workstation board below
    }
    if (alive && !fleetRead) showBoard(DEFAULT_BOARD.name, DEFAULT_BOARD.params);
  }

  // Live data.
  const offAgents = agents.watch((list) => sync(list));
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

  void refresh();
  const poll = window.setInterval(() => void refresh(), POLL_MS);

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
    alive = false;
    cancelAnimationFrame(raf);
    window.clearInterval(poll);
    ro.disconnect();
    offAgents();
    offTranscript();
    offDelta();
    offConversation();
    root.classList.remove('floor-view');
    root.replaceChildren();
  };
}
