// The Floor: the plaza as a 2.5D isometric scene on a canvas. The
// orchestrator stands on a dais in the middle; workers stand on rings around
// it, grouped by role. Each agent is a small robot whose light shows its
// state; working robots pulse and speak (what they're doing); delegations and
// messages fly between them as packets. Drag to pan, wheel to zoom, click a
// robot to open it.

import '../styles/floor.css';

import type { Agent, Conversation, Delta, Entry } from '../api';
import { go } from '../router';
import { agents, on } from '../store';
import { button, clip } from '../ui/components';
import { h } from '../ui/dom';
import type { Cleanup } from './index';

const TILE_W = 72; // an iso tile's width, px at zoom 1
const TILE_H = 36;
const BUBBLE_MS = 4500;
const PACKET_MS = 1400;

interface Bot {
  agent: Agent;
  gx: number; // grid position
  gy: number;
  sx: number; // screen position of the pod's centre (world px, before pan/zoom)
  sy: number;
  bubble: { text: string; until: number; tone: 'info' | 'ok' | 'danger' } | null;
}

interface Packet {
  from: number;
  to: number;
  start: number;
  tone: 'accent' | 'ok';
}

interface Palette {
  bg: string; floor: string; floorLine: string; podTop: string; podLeft: string; podRight: string;
  text: string; text2: string; text3: string; accent: string; ok: string; info: string; danger: string;
  surface: string; border: string;
}

export function mount(root: HTMLElement): Cleanup {
  const canvas = h('canvas.floor-canvas', { 'aria-label': 'The plaza floor: one robot per agent' });
  const tip = h('div.floor-tip', { hidden: true });
  const legend = h(
    'div.floor-legend',
    null,
    h('span', null, h('i.floor-key.working'), 'Working'),
    h('span', null, h('i.floor-key.idle'), 'Idle'),
    h('span', null, h('i.floor-key.offline'), 'Offline'),
  );
  const stage = h('div.floor-stage', null, canvas, tip, legend);
  root.append(
    h('div.view-head', null, h('h1', null, 'Floor'), h('div.actions', null, button('Reset view', () => resetView(), { kind: 'ghost', icon: 'refresh' }))),
    stage,
  );
  root.classList.add('floor-view');

  const ctx = canvas.getContext('2d')!;
  let bots: Bot[] = [];
  const byCid = new Map<number, Bot>();
  const packets: Packet[] = [];
  let palette = readPalette();
  let pan = { x: 0, y: 0 };
  let zoom = 1;
  let hovered: Bot | null = null;
  let frame = 0;
  let running = true;
  let dpr = 1;

  // ---- Layout ---------------------------------------------------------------

  function layout(list: Agent[]): void {
    const previous = new Map(bots.map((b) => [b.agent.cid, b]));
    const orch = list.filter((a) => a.tier === 'orchestrator');
    const workers = list.filter((a) => a.tier !== 'orchestrator').sort((a, b) => a.role.localeCompare(b.role) || a.cid - b.cid);
    const slots = ringSlots(workers.length);
    const placed: [Agent, number, number][] = [];
    orch.forEach((a, i) => placed.push([a, i, 0]));
    workers.forEach((a, i) => placed.push([a, slots[i]![0], slots[i]![1]]));
    bots = placed.map(([agent, gx, gy]) => {
      const old = previous.get(agent.cid);
      return { agent, gx, gy, sx: (gx - gy) * (TILE_W / 2), sy: (gx + gy) * (TILE_H / 2), bubble: old?.bubble ?? null };
    });
    bots.sort((a, b) => a.gx + a.gy - (b.gx + b.gy));
    byCid.clear();
    bots.forEach((b) => byCid.set(b.agent.cid, b));
  }

  function resetView(): void {
    zoom = 1;
    pan = { x: 0, y: 0 };
    fit();
  }

  /** Zooms so the whole plaza fits the stage. */
  function fit(): void {
    if (!bots.length) return;
    const xs = bots.map((b) => b.sx);
    const ys = bots.map((b) => b.sy);
    const w = Math.max(...xs) - Math.min(...xs) + TILE_W * 3;
    const hgt = Math.max(...ys) - Math.min(...ys) + TILE_H * 6;
    const rect = canvas.getBoundingClientRect();
    zoom = Math.max(0.4, Math.min(1.6, Math.min(rect.width / w, rect.height / hgt)));
    pan = { x: -((Math.max(...xs) + Math.min(...xs)) / 2) * zoom, y: -((Math.max(...ys) + Math.min(...ys)) / 2) * zoom + 20 };
  }

  // ---- Drawing --------------------------------------------------------------

  function resize(): void {
    const rect = canvas.getBoundingClientRect();
    dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(rect.width * dpr);
    canvas.height = Math.round(rect.height * dpr);
  }

  function toScreen(x: number, y: number): [number, number] {
    const rect = canvas.getBoundingClientRect();
    return [rect.width / 2 + pan.x + x * zoom, rect.height / 2 + pan.y + y * zoom];
  }

  function draw(now: number): void {
    const rect = canvas.getBoundingClientRect();
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, rect.width, rect.height);
    ctx.save();
    ctx.translate(rect.width / 2 + pan.x, rect.height / 2 + pan.y);
    ctx.scale(zoom, zoom);
    drawFloor();
    for (const bot of bots) drawBot(bot, now);
    drawPackets(now);
    for (const bot of bots) drawBubble(bot, now);
    ctx.restore();
  }

  function drawFloor(): void {
    const extent = Math.max(3, ...bots.map((b) => Math.max(Math.abs(b.gx), Math.abs(b.gy)))) + 1;
    for (let gx = -extent; gx <= extent; gx++) {
      for (let gy = -extent; gy <= extent; gy++) {
        const x = (gx - gy) * (TILE_W / 2);
        const y = (gx + gy) * (TILE_H / 2);
        diamond(x, y, TILE_W / 2, TILE_H / 2);
        const centre = gx === 0 && gy === 0;
        ctx.fillStyle = centre ? palette.podTop : palette.floor;
        ctx.globalAlpha = centre ? 0.9 : 0.55 + 0.1 * ((gx + gy) & 1);
        ctx.fill();
        ctx.globalAlpha = 1;
        ctx.strokeStyle = palette.floorLine;
        ctx.lineWidth = 1;
        ctx.stroke();
      }
    }
  }

  function diamond(x: number, y: number, hw: number, hh: number): void {
    ctx.beginPath();
    ctx.moveTo(x, y - hh);
    ctx.lineTo(x + hw, y);
    ctx.lineTo(x, y + hh);
    ctx.lineTo(x - hw, y);
    ctx.closePath();
  }

  function drawBot(bot: Bot, now: number): void {
    const { agent } = bot;
    const orch = agent.tier === 'orchestrator';
    const working = agent.status === 'working';
    const offline = agent.status === 'offline';
    const podH = orch ? 18 : 10;
    const hw = TILE_W * 0.34;
    const hh = TILE_H * 0.34;
    const { sx: x, sy: y } = bot;
    ctx.globalAlpha = offline ? 0.4 : 1;

    // Working: a pulse ring on the floor.
    if (working) {
      const t = (now % 1600) / 1600;
      diamond(x, y, hw * (1.2 + t * 0.6), hh * (1.2 + t * 0.6));
      ctx.strokeStyle = palette.info;
      ctx.globalAlpha = 0.6 * (1 - t);
      ctx.lineWidth = 2;
      ctx.stroke();
      ctx.globalAlpha = offline ? 0.4 : 1;
    }

    // The pod: an iso box.
    ctx.fillStyle = palette.podLeft;
    ctx.beginPath();
    ctx.moveTo(x - hw, y);
    ctx.lineTo(x, y + hh);
    ctx.lineTo(x, y + hh - podH);
    ctx.lineTo(x - hw, y - podH);
    ctx.closePath();
    ctx.fill();
    ctx.fillStyle = palette.podRight;
    ctx.beginPath();
    ctx.moveTo(x + hw, y);
    ctx.lineTo(x, y + hh);
    ctx.lineTo(x, y + hh - podH);
    ctx.lineTo(x + hw, y - podH);
    ctx.closePath();
    ctx.fill();
    diamond(x, y - podH, hw, hh);
    ctx.fillStyle = palette.podTop;
    ctx.fill();
    ctx.strokeStyle = orch ? palette.accent : palette.floorLine;
    ctx.lineWidth = orch ? 1.5 : 1;
    ctx.stroke();

    // The robot.
    const bob = working ? Math.sin(now / 220) * 2 : offline ? 0 : Math.sin(now / 900 + agent.cid) * 1;
    const top = y - podH - 4 + bob;
    const hue = roleHue(agent.role);
    const bodyW = orch ? 20 : 16;
    const bodyH = orch ? 22 : 18;
    // shadow
    ctx.fillStyle = 'rgba(0,0,0,0.25)';
    ctx.beginPath();
    ctx.ellipse(x, y - podH, bodyW * 0.6, bodyW * 0.25, 0, 0, Math.PI * 2);
    ctx.fill();
    // body
    const grad = ctx.createLinearGradient(x - bodyW / 2, 0, x + bodyW / 2, 0);
    grad.addColorStop(0, `hsl(${hue} 55% 62%)`);
    grad.addColorStop(1, `hsl(${hue} 50% 42%)`);
    ctx.fillStyle = grad;
    roundRect(x - bodyW / 2, top - bodyH, bodyW, bodyH, 6);
    ctx.fill();
    // head
    const headR = orch ? 10 : 8;
    const hy = top - bodyH - headR + 2;
    ctx.fillStyle = `hsl(${hue} 30% 88%)`;
    ctx.beginPath();
    ctx.arc(x, hy, headR, 0, Math.PI * 2);
    ctx.fill();
    // visor and eye light
    ctx.fillStyle = '#0d1117';
    roundRect(x - headR * 0.75, hy - headR * 0.35, headR * 1.5, headR * 0.7, headR * 0.35);
    ctx.fill();
    const light = offline ? palette.text3 : working ? palette.info : palette.ok;
    const blink = working ? 0.6 + 0.4 * Math.sin(now / 120) : 1;
    ctx.fillStyle = light;
    ctx.globalAlpha = (offline ? 0.4 : 1) * blink;
    ctx.beginPath();
    ctx.arc(x - headR * 0.3, hy, headR * 0.16, 0, Math.PI * 2);
    ctx.arc(x + headR * 0.3, hy, headR * 0.16, 0, Math.PI * 2);
    ctx.fill();
    ctx.globalAlpha = offline ? 0.4 : 1;
    // antenna
    ctx.strokeStyle = `hsl(${hue} 30% 80%)`;
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.moveTo(x, hy - headR);
    ctx.lineTo(x, hy - headR - 7);
    ctx.stroke();
    ctx.fillStyle = working ? palette.info : orch ? palette.accent : light;
    ctx.beginPath();
    ctx.arc(x, hy - headR - 8, orch ? 3 : 2.2, 0, Math.PI * 2);
    ctx.fill();

    // Hover outline
    if (hovered === bot) {
      ctx.strokeStyle = palette.accent;
      ctx.lineWidth = 2;
      diamond(x, y, hw * 1.35, hh * 1.35);
      ctx.stroke();
    }

    // Name plate
    ctx.globalAlpha = offline ? 0.5 : 1;
    ctx.font = `${orch ? 600 : 500} ${orch ? 12 : 11}px ui-sans-serif, system-ui, sans-serif`;
    ctx.textAlign = 'center';
    ctx.fillStyle = palette.text;
    ctx.fillText(agent.name, x, y + hh + 14);
    ctx.font = '10px ui-sans-serif, system-ui, sans-serif';
    ctx.fillStyle = palette.text3;
    ctx.fillText(orch ? 'orchestrator' : agent.role, x, y + hh + 26);
    ctx.globalAlpha = 1;
  }

  function drawBubble(bot: Bot, now: number): void {
    const b = bot.bubble;
    if (!b || now > b.until) return;
    const fade = Math.min(1, (b.until - now) / 600);
    const x = bot.sx;
    const y = bot.sy - (bot.agent.tier === 'orchestrator' ? 92 : 78);
    ctx.font = '11px ui-sans-serif, system-ui, sans-serif';
    const text = clip(b.text, 34);
    const w = ctx.measureText(text).width + 16;
    ctx.globalAlpha = fade;
    ctx.fillStyle = palette.surface;
    ctx.strokeStyle = b.tone === 'danger' ? palette.danger : b.tone === 'ok' ? palette.ok : palette.info;
    ctx.lineWidth = 1.2;
    roundRect(x - w / 2, y - 22, w, 20, 8);
    ctx.fill();
    ctx.stroke();
    ctx.beginPath();
    ctx.moveTo(x - 5, y - 2);
    ctx.lineTo(x, y + 4);
    ctx.lineTo(x + 5, y - 2);
    ctx.fill();
    ctx.fillStyle = palette.text;
    ctx.textAlign = 'center';
    ctx.fillText(text, x, y - 8);
    ctx.globalAlpha = 1;
  }

  function drawPackets(now: number): void {
    for (let i = packets.length - 1; i >= 0; i--) {
      const p = packets[i]!;
      const t = (now - p.start) / PACKET_MS;
      const from = byCid.get(p.from);
      const to = byCid.get(p.to);
      if (t >= 1 || !from || !to) {
        packets.splice(i, 1);
        continue;
      }
      const ax = from.sx, ay = from.sy - 40, bx = to.sx, by = to.sy - 40;
      const cx = (ax + bx) / 2, cy = Math.min(ay, by) - 60;
      const color = p.tone === 'ok' ? palette.ok : palette.accent;
      ctx.strokeStyle = color;
      ctx.globalAlpha = 0.25;
      ctx.lineWidth = 1.5;
      ctx.beginPath();
      ctx.moveTo(ax, ay);
      ctx.quadraticCurveTo(cx, cy, bx, by);
      ctx.stroke();
      const e = 1 - (1 - t) * (1 - t);
      const px = (1 - e) * (1 - e) * ax + 2 * (1 - e) * e * cx + e * e * bx;
      const py = (1 - e) * (1 - e) * ay + 2 * (1 - e) * e * cy + e * e * by;
      ctx.globalAlpha = 1;
      ctx.fillStyle = color;
      ctx.shadowColor = color;
      ctx.shadowBlur = 10;
      ctx.beginPath();
      ctx.arc(px, py, 4, 0, Math.PI * 2);
      ctx.fill();
      ctx.shadowBlur = 0;
    }
  }

  function roundRect(x: number, y: number, w: number, hgt: number, r: number): void {
    ctx.beginPath();
    ctx.roundRect(x, y, w, hgt, r);
  }

  // ---- Interaction ----------------------------------------------------------

  function botAt(clientX: number, clientY: number): Bot | null {
    const rect = canvas.getBoundingClientRect();
    for (let i = bots.length - 1; i >= 0; i--) {
      const bot = bots[i]!;
      const [x, y] = toScreen(bot.sx, bot.sy - 30);
      if (Math.hypot(clientX - rect.left - x, clientY - rect.top - y) < 30 * zoom) return bot;
    }
    return null;
  }

  let drag: { x: number; y: number; px: number; py: number; moved: boolean } | null = null;
  canvas.addEventListener('pointerdown', (e) => {
    drag = { x: e.clientX, y: e.clientY, px: pan.x, py: pan.y, moved: false };
    canvas.setPointerCapture(e.pointerId);
  });
  canvas.addEventListener('pointermove', (e) => {
    if (drag) {
      const dx = e.clientX - drag.x, dy = e.clientY - drag.y;
      if (Math.abs(dx) + Math.abs(dy) > 3) drag.moved = true;
      pan = { x: drag.px + dx, y: drag.py + dy };
    }
    hovered = drag?.moved ? null : botAt(e.clientX, e.clientY);
    canvas.style.cursor = drag?.moved ? 'grabbing' : hovered ? 'pointer' : 'grab';
    showTip(e);
  });
  canvas.addEventListener('pointerup', (e) => {
    const clicked = drag && !drag.moved ? botAt(e.clientX, e.clientY) : null;
    drag = null;
    if (clicked) go('agents', clicked.agent.cid);
  });
  canvas.addEventListener('pointerleave', () => {
    hovered = null;
    tip.hidden = true;
  });
  canvas.addEventListener(
    'wheel',
    (e) => {
      e.preventDefault();
      const rect = canvas.getBoundingClientRect();
      const mx = e.clientX - rect.left - rect.width / 2 - pan.x;
      const my = e.clientY - rect.top - rect.height / 2 - pan.y;
      const next = Math.max(0.35, Math.min(2.5, zoom * Math.exp(-e.deltaY * 0.0015)));
      pan = { x: pan.x - mx * (next / zoom - 1), y: pan.y - my * (next / zoom - 1) };
      zoom = next;
    },
    { passive: false },
  );

  function showTip(e: PointerEvent): void {
    if (!hovered) {
      tip.hidden = true;
      return;
    }
    const a = hovered.agent;
    tip.replaceChildren(
      h('strong', null, a.name),
      h('span', null, `${a.role} · ${a.status}`),
      h('span.mono', null, a.model),
      a.work ? h('span.faint', null, clip(a.work.prompt, 80)) : null,
    );
    const rect = stage.getBoundingClientRect();
    tip.style.left = `${e.clientX - rect.left + 14}px`;
    tip.style.top = `${e.clientY - rect.top + 14}px`;
    tip.hidden = false;
  }

  // ---- Live signals ---------------------------------------------------------

  function say(cid: number, text: string, tone: 'info' | 'ok' | 'danger', ms = BUBBLE_MS): void {
    const bot = byCid.get(cid);
    if (bot) bot.bubble = { text, until: performance.now() + ms, tone };
  }

  const offs = [
    agents.watch((list) => {
      const first = bots.length === 0;
      layout(list);
      if (first) fit();
    }),
    on('delta', (data) => {
      const d = data as Delta;
      if (d.delta.type === 'tool_use') say(d.cid, `→ ${d.delta.name}`, 'info');
      else if (d.delta.type === 'thinking') say(d.cid, 'thinking…', 'info', 1500);
      else say(d.cid, 'writing…', 'info', 1500);
    }),
    on('transcript', (data) => {
      const e = data as Entry;
      if (e.kind === 'tool_request') say(e.cid, `→ ${String(e.data['name'])}`, 'info');
      else if (e.kind === 'work_end') say(e.cid, e.data['status'] === 'done' ? '✓ done' : `✗ ${String(e.data['status'])}`, e.data['status'] === 'done' ? 'ok' : 'danger');
      else if (e.kind === 'error') say(e.cid, 'error', 'danger');
      else if (e.kind === 'work_start') {
        const origin = e.data['origin'] as { from_cid?: number | null } | undefined;
        if (origin?.from_cid) packets.push({ from: origin.from_cid, to: e.cid, start: performance.now(), tone: 'accent' });
        else if (e.data['kind'] === 'task') {
          const orch = bots.find((b) => b.agent.tier === 'orchestrator');
          if (orch && orch.agent.cid !== e.cid) packets.push({ from: orch.agent.cid, to: e.cid, start: performance.now(), tone: 'accent' });
        }
      }
    }),
    on('conversation', (data) => {
      const c = data as Conversation;
      packets.push({ from: c.from_cid, to: c.to_cid, start: performance.now(), tone: 'ok' });
    }),
  ];

  const observer = new ResizeObserver(() => resize());
  observer.observe(canvas);
  resize();
  const themeQuery = window.matchMedia('(prefers-color-scheme: light)');
  const onTheme = () => (palette = readPalette());
  themeQuery.addEventListener('change', onTheme);

  const loop = (now: number) => {
    if (!running) return;
    draw(now);
    frame = requestAnimationFrame(loop);
  };
  frame = requestAnimationFrame(loop);

  return () => {
    running = false;
    cancelAnimationFrame(frame);
    observer.disconnect();
    themeQuery.removeEventListener('change', onTheme);
    root.classList.remove('floor-view');
    offs.forEach((off) => off());
  };
}

/** Square rings of grid slots around the centre: 8 on the first, 16 on the next… */
export function ringSlots(n: number): [number, number][] {
  const out: [number, number][] = [];
  for (let ring = 2; out.length < n; ring += 2) {
    const r = ring;
    for (let i = -r; i < r && out.length < n; i += 2) out.push([i, -r]);
    for (let i = -r; i < r && out.length < n; i += 2) out.push([r, i]);
    for (let i = r; i > -r && out.length < n; i -= 2) out.push([i, r]);
    for (let i = r; i > -r && out.length < n; i -= 2) out.push([-r, i]);
  }
  return out;
}

/** A stable hue per role. */
export function roleHue(role: string): number {
  let hash = 0;
  for (const ch of role) hash = (hash * 31 + ch.charCodeAt(0)) >>> 0;
  return hash % 360;
}

function readPalette(): Palette {
  const css = getComputedStyle(document.documentElement);
  const v = (name: string) => css.getPropertyValue(name).trim();
  return {
    bg: v('--bg'), floor: v('--surface-2'), floorLine: v('--border'), podTop: v('--surface-3'),
    podLeft: v('--border'), podRight: v('--border-strong'), text: v('--text'), text2: v('--text-2'),
    text3: v('--text-3'), accent: v('--accent'), ok: v('--ok'), info: v('--info'), danger: v('--danger'),
    surface: v('--surface'), border: v('--border'),
  };
}
