/* The 2D map: the node's fleet drawn as a surveyor's plan on canvas 2D.
   The machine is the plot (a bounded region labeled with its model), each run
   a lane, each VM a cell by run/VM index. State marks are outlined shapes —
   running is a filled circle, exited a square, killed a diamond; a vsock CID
   is the wire through the square. Running VMs pulse with their serial chatter
   (lines/sec from the store's poll deltas). Click or arrow-key+Enter selects
   a VM and opens its run entry. No chart library, no framework. */

import { byId } from '../dom';
import { fmtNum } from '../format';
import { vmKey } from '../models/activity';
import type { Run, Vm } from '../models/run';
import { seqOf } from '../models/run';
import { store } from '../store';

interface Cell {
  run: Run;
  vm: Vm;
  index: number;
  x: number;
  y: number;
}

interface Palette {
  paper: string; ink: string; ink2: string; ink3: string;
  rule: string; margin: string; signal: string; ok: string; crit: string; warn: string;
}

const LANE_H = 84;
const CELL = 30;
const CELL_GAP = 12;
const GUTTER = 108;
const TOP = 82;
const ROW_PITCH = 54; // a wrapped row of cells within one lane
const REDUCED = typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches;

let cells: Cell[] = [];
/* hover is a run/vm KEY, never a cell object: layout() rebuilds cells from
   the store every render, and a held object would go stale (a killed VM would
   keep reading "running"). The focused index stays valid because lane order
   (run seq, then VM index) is stable. */
let hoverKey: { runId: string; vmId: number } | null = null;
let focused = 0; // index into cells (keyboard cursor)
let phase = 0; // pulse phase 0..1
let phaseTimer: number | null = null;
let canvas: HTMLCanvasElement | null = null;
let ctx: CanvasRenderingContext2D | null = null;

function palette(): Palette {
  const cs = getComputedStyle(document.documentElement);
  const g = (name: string): string => cs.getPropertyValue(name).trim();
  return {
    paper: g('--paper'), ink: g('--ink'), ink2: g('--ink-2'), ink3: g('--ink-3'),
    rule: g('--rule'), margin: g('--margin'), signal: g('--signal'),
    ok: g('--ok'), crit: g('--crit'), warn: g('--warn'),
  };
}

/* ------------------------------------------------------------ layout */
interface Lane {
  run: Run;
  y: number;
  rows: number;
}

/* A lane wraps its cells at the plot's right edge onto as many rows as the
   run needs — a 50-VM run takes three rows at desktop width, ten on a phone,
   and every cell stays inside the plot. */
function layout(width: number): { height: number; lanes: Lane[] } {
  cells = [];
  const lanes: Lane[] = [];
  const runs = store.runs.slice().sort((a, b) => seqOf(a.id) - seqOf(b.id));
  const perRow = Math.max(1, Math.floor((width - GUTTER - 30) / (CELL + CELL_GAP)));
  let y = TOP;
  for (const run of runs) {
    const rows = Math.max(1, Math.ceil(run.vms.length / perRow));
    lanes.push({ run, y, rows });
    run.vms.forEach((vm, index) => {
      const row = Math.floor(index / perRow);
      const col = index % perRow;
      cells.push({ run, vm, index, x: GUTTER + col * (CELL + CELL_GAP), y: y + row * ROW_PITCH });
    });
    y += LANE_H + (rows - 1) * ROW_PITCH;
  }
  return { height: Math.max(420, y + 24), lanes };
}

/* ------------------------------------------------------------ drawing */
export function renderMap(): void {
  if (store.route.name !== 'map') return;
  if (!canvas) initCanvas();
  if (!canvas || !ctx) return;

  const notice = byId('map-notice');
  if (store.runsError != null && store.runs.length === 0) {
    notice.hidden = false;
    byId('map-notice-text').textContent =
      `The node daemon is not answering at ${location.origin} (${store.runsError}). Retrying.`;
  } else {
    notice.hidden = true;
  }

  const c = canvas;
  const cssWidth = c.clientWidth || c.parentElement?.clientWidth || 800;
  const { height: cssHeight, lanes } = layout(cssWidth);
  if (c.style.height !== `${cssHeight}px`) c.style.height = `${cssHeight}px`;
  const dpr = Math.min(2, window.devicePixelRatio || 1);
  const w = Math.round(cssWidth * dpr);
  const h = Math.round(cssHeight * dpr);
  if (c.width !== w || c.height !== h) {
    c.width = w;
    c.height = h;
  }
  const g = ctx;
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  const p = palette();
  const mono = '11px "Martian Mono", ui-monospace, monospace';
  const monoSm = '9.5px "Martian Mono", ui-monospace, monospace';

  g.fillStyle = p.paper;
  g.fillRect(0, 0, cssWidth, cssHeight);

  /* the machine plot: a bounded region with a double rule, labeled */
  g.strokeStyle = p.ink;
  g.lineWidth = 1;
  g.strokeRect(10.5, 10.5, cssWidth - 21, cssHeight - 21);
  g.strokeStyle = p.margin;
  g.strokeRect(14.5, 14.5, cssWidth - 29, cssHeight - 29);
  const m = store.machine;
  const label = m
    ? `${m.name} · ${m.class} · ${m.arch} ${m.backend} · ${fmtNum(m.cpus)} cpus · ${fmtNum(m.memory_mib)} MiB · vsock ${m.vsock_mode}`
    : store.machineError ? 'machine unreachable' : 'machine —';
  g.font = mono;
  g.fillStyle = p.ink2;
  g.textBaseline = 'middle';
  g.fillText(label, 24, 32);
  /* the brand corner mark: ink square, orange corner */
  g.fillStyle = p.ink;
  g.fillRect(cssWidth - 34, 22, 12, 12);
  g.fillStyle = p.signal;
  g.fillRect(cssWidth - 28, 28, 6, 6);

  /* lanes and cells */
  for (const lane of lanes) {
    const run = lane.run;
    g.font = mono;
    g.fillStyle = run.state === 'running' ? p.ink : p.ink3;
    g.fillText(run.id, 24, lane.y - 12);
    g.font = monoSm;
    g.fillStyle = p.ink3;
    g.fillText(`${run.vms.length}×${run.vcpus} vCPU`, 24, lane.y + 2);
    g.fillText(`${fmtNum(run.memory_mib)} MiB`, 24, lane.y + 16);
    g.strokeStyle = p.rule;
    const sepY = lane.y + (lane.rows - 1) * ROW_PITCH + 38.5;
    g.beginPath();
    g.moveTo(GUTTER - 12, sepY);
    g.lineTo(cssWidth - 24, sepY);
    g.stroke();
  }

  for (const cell of cells) drawCell(g, cell, p);

  byId('map-live').hidden = store.health !== 'live';
  byId('map-live').textContent = store.health;
  updateStatus();
}

function drawCell(g: CanvasRenderingContext2D, cell: Cell, p: Palette): void {
  const cx = cell.x + CELL / 2;
  const cy = cell.y;
  const key = vmKey(cell.run.id, cell.vm.id);
  const lps = store.activity.get(key);
  const isSel = store.detailId === cell.run.id && store.vm === cell.index;
  const isFocus = cells[focused] === cell && document.activeElement === canvas;
  const isHover = hoverKey != null && hoverKey.runId === cell.run.id && hoverKey.vmId === cell.vm.id;

  /* pulse: a ring whose reach and weight follow serial chatter */
  if (cell.vm.state === 'running' && lps > 0.02) {
    const reach = REDUCED ? Math.min(1, lps / 6) : phase;
    const alpha = Math.min(0.85, 0.25 + lps / 4) * (1 - reach * 0.7);
    g.strokeStyle = p.ok;
    g.globalAlpha = Math.max(0.15, alpha);
    g.lineWidth = 1.5;
    g.beginPath();
    g.arc(cx, cy, 10 + reach * (8 + Math.min(10, lps * 2)), 0, Math.PI * 2);
    g.stroke();
    g.globalAlpha = 1;
  }

  /* the state mark */
  g.lineWidth = 1.5;
  if (cell.vm.state === 'running') {
    g.fillStyle = p.ok;
    g.beginPath();
    g.arc(cx, cy, 7, 0, Math.PI * 2);
    g.fill();
  } else if (cell.vm.state === 'exited') {
    g.strokeStyle = p.ink2;
    g.strokeRect(cx - 6.5, cy - 6.5, 13, 13);
  } else {
    /* killed: the crit diamond */
    g.strokeStyle = p.crit;
    g.beginPath();
    g.moveTo(cx, cy - 8);
    g.lineTo(cx + 8, cy);
    g.lineTo(cx, cy + 8);
    g.lineTo(cx - 8, cy);
    g.closePath();
    g.stroke();
  }

  /* the vsock wire through the mark */
  if (cell.vm.guest_cid != null) {
    g.strokeStyle = p.ink2;
    g.lineWidth = 1;
    g.beginPath();
    g.moveTo(cx - 12, cy + 0.5);
    g.lineTo(cx + 12, cy + 0.5);
    g.stroke();
  }

  /* labels: vm id above, cid below */
  g.font = '9.5px "Martian Mono", ui-monospace, monospace';
  g.textAlign = 'center';
  g.fillStyle = p.ink3;
  g.fillText(`vm ${cell.vm.id}`, cx, cy - 22);
  if (cell.vm.guest_cid != null) {
    g.fillStyle = p.ink2;
    g.fillText(`cid ${cell.vm.guest_cid}`, cx, cy + 28);
  }
  g.textAlign = 'left';

  /* selection and keyboard focus: the signal outline */
  if (isSel || isFocus) {
    g.strokeStyle = p.signal;
    g.lineWidth = 2;
    g.strokeRect(cx - 14.5, cy - 14.5, 29, 29);
  }
  if (isHover) {
    g.strokeStyle = p.ink3;
    g.lineWidth = 1;
    g.strokeRect(cx - 17.5, cy - 17.5, 35, 35);
  }
}

function updateStatus(): void {
  const hovered = hoverKey
    ? (cells.find((c) => c.run.id === hoverKey?.runId && c.vm.id === hoverKey?.vmId) ?? null)
    : null;
  const target = hovered ?? cells[focused] ?? null;
  const status = byId('map-status');
  if (cells.length === 0) {
    status.textContent = store.runsError != null
      ? `The node daemon is not answering (${store.runsError}).`
      : 'The register is blank — no runs to draw. Enter one on the register page.';
    return;
  }
  if (!target) {
    status.textContent = `${cells.length} VMs on the plot. Arrow keys walk them; Enter opens the entry.`;
    return;
  }
  const lps = target.vm.state === 'running' ? store.activity.get(vmKey(target.run.id, target.vm.id)) : 0;
  status.textContent =
    `${target.run.id} · vm ${target.vm.id} · ${target.vm.state}` +
    (target.vm.guest_cid != null ? ` · cid ${target.vm.guest_cid}` : '') +
    (lps > 0.02 ? ` · ${lps.toFixed(1)} lines/s` : '') +
    ' — Enter or click opens the entry';
}

/* ------------------------------------------------------------ selection */
function openCell(cell: Cell): void {
  store.detailId = cell.run.id;
  store.vm = cell.index;
  if (location.hash !== `#run/${cell.run.id}`) location.hash = `#run/${cell.run.id}`;
  else store.scheduleRender();
}

function cellAt(mx: number, my: number): Cell | null {
  for (const cell of cells) {
    if (Math.abs(mx - (cell.x + CELL / 2)) <= 18 && Math.abs(my - cell.y) <= 18) return cell;
  }
  return null;
}

function moveFocus(dx: number, dy: number): void {
  if (cells.length === 0) return;
  const cur = cells[focused] ?? cells[0];
  let next = -1;
  if (!cur) {
    next = 0;
  } else if (dy !== 0) {
    /* nearest cell in the lane above/below */
    let best = Infinity;
    cells.forEach((c, i) => {
      if (Math.sign(c.y - cur.y) !== Math.sign(dy)) return;
      const d = Math.abs(c.x - cur.x) + Math.abs(c.y - cur.y);
      if (d < best) { best = d; next = i; }
    });
  } else {
    /* walk left/right within the flattened order, not wrapping lanes */
    const i = focused + dx;
    const cand = cells[i];
    if (cand && cand.run === cur.run) next = i;
  }
  if (next >= 0) {
    focused = next;
    hoverKey = null; // the keyboard cursor outranks a stale pointer hover
    const cell = cells[next];
    if (cell) byId('sr-status').textContent = `vm ${cell.vm.id} of run ${cell.run.id}, ${cell.vm.state}`;
    renderMap();
  }
}

function initCanvas(): void {
  canvas = byId<HTMLCanvasElement>('map-canvas');
  ctx = canvas.getContext('2d');

  canvas.addEventListener('click', (e) => {
    const rect = canvas?.getBoundingClientRect();
    if (!rect) return;
    const cell = cellAt(e.clientX - rect.left, e.clientY - rect.top);
    if (cell) {
      focused = cells.indexOf(cell);
      openCell(cell);
    }
  });
  canvas.addEventListener('mousemove', (e) => {
    const rect = canvas?.getBoundingClientRect();
    if (!rect) return;
    const cell = cellAt(e.clientX - rect.left, e.clientY - rect.top);
    const key = cell ? { runId: cell.run.id, vmId: cell.vm.id } : null;
    if (key?.runId !== hoverKey?.runId || key?.vmId !== hoverKey?.vmId) {
      hoverKey = key;
      canvas?.style.setProperty('cursor', cell ? 'pointer' : 'default');
      renderMap();
    }
  });
  canvas.addEventListener('mouseleave', () => {
    hoverKey = null;
    renderMap();
  });
  canvas.addEventListener('keydown', (e) => {
    switch (e.key) {
      case 'ArrowLeft': moveFocus(-1, 0); e.preventDefault(); break;
      case 'ArrowRight': moveFocus(1, 0); e.preventDefault(); break;
      case 'ArrowUp': moveFocus(0, -1); e.preventDefault(); break;
      case 'ArrowDown': moveFocus(0, 1); e.preventDefault(); break;
      case 'Enter': {
        const cell = cells[focused];
        if (cell) { openCell(cell); e.preventDefault(); }
        break;
      }
    }
  });

  /* pulse phase: a plain interval, not rAF — the paint path stays
     frame-independent; hidden tabs and reduced-motion sit it out */
  if (phaseTimer == null && !REDUCED) {
    phaseTimer = setInterval(() => {
      if (document.hidden || store.route.name !== 'map') return;
      phase = (phase + 0.08) % 1;
      if (cells.some((c) => c.vm.state === 'running' && store.activity.get(vmKey(c.run.id, c.vm.id)) > 0.02)) {
        renderMap();
      }
    }, 120);
  }
}

/** Showing the map re-measures the canvas (its width reads 0 while hidden);
    clientWidth forces layout synchronously — no animation frame needed. */
export function syncMapVisibility(): void {
  if (store.route.name === 'map') renderMap();
}
