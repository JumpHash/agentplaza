// The artifact graph body: a layered layout of cards with routed links, or a force layout of circles, drawn as SVG in a
// frame that pans and zooms. The placement comes from the task graph's wasm (graph/wasm.ts), loaded on first use: the
// frame is returned at once and filled when the layout arrives. Colours are theme tokens only.

import '../styles/view-graph.css';
import { loadGraph } from '../graph/wasm';
import type { GraphLayout as WasmGraph, Layout, LayoutRoute } from '../graph/wasm';
import type { GraphBody, GraphEdge, GraphNode } from './model';

const SVG_NS = 'http://www.w3.org/2000/svg';
/** The most nodes drawn; a larger graph shows the first of them with a note. */
const MAX_NODES = 1000;
const FRAME_HEIGHT = 420;
const MAX_HEIGHT = 640;
const CARD_HEIGHT = 44;
const CARD_MIN = 120;
const CARD_MAX = 240;
/** The width of one label character, in CSS pixels (the estimate the card widths use). */
const CHAR_WIDTH = 7;
const NODE_RADIUS = 7;
const HUES = 6;
const ZOOM_MIN = 0.3;
const ZOOM_MAX = 3;
const FORCE_WIDTH = 800;
const FORCE_HEIGHT = 420;
const EDGE_LABEL_CHARS = 40;
const FORCE_LABEL_CHARS = 48;

/** One drawn edge: its line and its label (when it has one), kept for the hover highlight. */
interface EdgeRef {
  from: string;
  to: string;
  els: SVGElement[];
}

/** What a layout drew: the content's size and the nodes and edges hover can reach. */
interface Scene {
  width: number;
  height: number;
  nodes: Map<string, SVGGElement>;
  edges: EdgeRef[];
}

/** The state one drawn graph shares between its layout and its hover. */
interface Ctx {
  root: HTMLElement;
  tip: HTMLElement;
  world: SVGGElement;
  /** The arrowhead's reference, `url(#id)`. */
  arrow: string;
  directed: boolean;
  nodes: GraphNode[];
  edges: GraphEdge[];
  groups: Map<string, number>;
}

let seq = 0;

/** A graph body as a frame: the layout is drawn into it once the wasm has loaded. */
export function renderGraph(body: GraphBody): HTMLElement {
  const root = document.createElement('div');
  root.className = 'view-graph';
  root.style.height = `${FRAME_HEIGHT}px`;

  const svg = document.createElementNS(SVG_NS, 'svg') as SVGSVGElement;
  svg.setAttribute('class', 'view-graph-canvas');
  const arrowId = `view-graph-arrow-${++seq}`;
  const defs = el('defs', {});
  const marker = el(
    'marker',
    { id: arrowId, viewBox: '0 0 8 8', refX: 7, refY: 4, markerWidth: 7, markerHeight: 7, orient: 'auto' },
    'view-graph-marker',
  );
  marker.append(el('path', { d: 'M0,0 L8,4 L0,8 z' }, 'view-graph-arrow'));
  defs.append(marker);
  const world = el('g', {}, 'view-graph-world') as SVGGElement;
  svg.append(defs, world);

  const bar = document.createElement('div');
  bar.className = 'cluster view-graph-bar';
  const fit = document.createElement('button');
  fit.type = 'button';
  fit.className = 'btn sm ghost view-graph-fit';
  fit.textContent = 'Fit';
  bar.append(fit);

  const note = document.createElement('div');
  note.className = 'hint view-graph-note';
  const tip = document.createElement('div');
  tip.className = 'view-graph-tip';
  tip.hidden = true;
  root.append(svg, bar, note, tip);

  const { nodes, edges } = capGraph(body);
  const kind = body.layout ?? 'layered';
  const ctx: Ctx = {
    root,
    tip,
    world,
    arrow: `url(#${arrowId})`,
    directed: body.directed ?? kind === 'layered',
    nodes,
    edges,
    groups: groupIndex(nodes),
  };
  if (body.nodes.length > nodes.length) {
    note.textContent = `Showing the first ${nodes.length} of ${body.nodes.length} nodes.`;
  }
  if (nodes.length === 0) {
    note.textContent = 'This graph has no nodes.';
    return root;
  }

  const view = { k: 1, x: 0, y: 0 };
  let scene: Scene | null = null;
  // Set once the reader pans or zooms, so a resize stops refitting the frame.
  let moved = false;

  const apply = (): void => {
    world.setAttribute('transform', `translate(${view.x} ${view.y}) scale(${view.k})`);
  };
  const fitView = (): void => {
    if (!scene) return;
    const box = svg.getBoundingClientRect();
    view.k = clamp(Math.min(1, box.width / scene.width, box.height / scene.height), ZOOM_MIN, 1);
    view.x = (box.width - scene.width * view.k) / 2;
    view.y = (box.height - scene.height * view.k) / 2;
    apply();
  };
  fit.addEventListener('click', () => {
    moved = false;
    fitView();
  });
  new ResizeObserver(() => {
    if (!moved) fitView();
  }).observe(root);

  svg.addEventListener(
    'wheel',
    (ev: WheelEvent) => {
      ev.preventDefault();
      const box = svg.getBoundingClientRect();
      const px = ev.clientX - box.left;
      const py = ev.clientY - box.top;
      const unit = ev.deltaMode === 1 ? 16 : 1;
      const next = clamp(view.k * Math.exp(-ev.deltaY * unit * 0.0015), ZOOM_MIN, ZOOM_MAX);
      // Keep the content under the pointer where it is.
      const wx = (px - view.x) / view.k;
      const wy = (py - view.y) / view.k;
      view.k = next;
      view.x = px - wx * next;
      view.y = py - wy * next;
      moved = true;
      apply();
    },
    { passive: false },
  );

  let drag: { x: number; y: number; vx: number; vy: number } | null = null;
  svg.addEventListener('pointerdown', (ev: PointerEvent) => {
    if (ev.button !== 0) return;
    drag = { x: ev.clientX, y: ev.clientY, vx: view.x, vy: view.y };
    svg.setPointerCapture(ev.pointerId);
    svg.classList.add('is-panning');
  });
  svg.addEventListener('pointermove', (ev: PointerEvent) => {
    if (!drag) return;
    view.x = drag.vx + ev.clientX - drag.x;
    view.y = drag.vy + ev.clientY - drag.y;
    moved = true;
    apply();
  });
  const release = (): void => {
    drag = null;
    svg.classList.remove('is-panning');
  };
  svg.addEventListener('pointerup', release);
  svg.addEventListener('pointercancel', release);

  loadGraph().then(
    (graph) => {
      try {
        scene = kind === 'force' ? forceScene(ctx, graph) : layeredScene(ctx, graph);
        root.style.height = `${Math.min(MAX_HEIGHT, Math.max(FRAME_HEIGHT, Math.ceil(scene.height)))}px`;
        fitView();
      } catch (err) {
        note.textContent = `Could not draw the graph: ${reason(err)}`;
      }
    },
    (err: unknown) => {
      note.textContent = `Could not draw the graph: ${reason(err)}`;
    },
  );
  return root;
}

/** The layered layout: cards in columns, each link routed as a polyline from its source to its target. */
function layeredScene(ctx: Ctx, graph: WasmGraph): Scene {
  const width = cardWidth(ctx.nodes);
  const layout: Layout = graph.layout(
    {
      tasks: ctx.nodes.map((n) => ({ id: n.id })),
      edges: ctx.edges.map((e): [string, string] => [e.from, e.to]),
    },
    { card_width: width, card_height: CARD_HEIGHT },
  );
  const cards = new Map(layout.cards.map((c) => [c.id, c]));
  const routes = new Map<string, LayoutRoute>(layout.routes.map((r) => [edgeKey(r.from, r.to), r]));
  const edgeLayer = el('g', {}, 'view-graph-edges') as SVGGElement;
  const nodeLayer = el('g', {}, 'view-graph-nodes') as SVGGElement;
  ctx.world.replaceChildren(edgeLayer, nodeLayer);

  const scene: Scene = { width: layout.width, height: layout.height, nodes: new Map(), edges: [] };
  for (const e of ctx.edges) {
    const route = routes.get(edgeKey(e.from, e.to));
    if (!route || route.points.length < 2) continue;
    const line = el(
      'polyline',
      {
        points: route.points.map(([x, y]) => `${x},${y}`).join(' '),
        ...(ctx.directed ? { 'marker-end': ctx.arrow } : {}),
      },
      'view-graph-edge',
    );
    edgeLayer.append(line);
    const els: SVGElement[] = [line];
    if (e.label) {
      const [mx, my] = midpoint(route.points);
      els.push(edgeLabel(edgeLayer, mx, my, e.label));
    }
    scene.edges.push({ from: e.from, to: e.to, els });
  }

  for (const n of ctx.nodes) {
    const c = cards.get(n.id);
    if (!c) continue;
    const g = el('g', {}, 'view-graph-node') as SVGGElement;
    g.append(el('rect', { x: c.x, y: c.y, width: c.width, height: c.height, rx: 8 }, 'view-graph-card'));
    if (n.tone || n.group !== undefined) {
      g.append(
        el(
          'line',
          {
            x1: c.x + 2,
            y1: c.y + 8,
            x2: c.x + 2,
            y2: c.y + c.height - 8,
            'stroke-width': 3,
            'stroke-linecap': 'round',
            style: `stroke: ${nodeColour(n, ctx.groups)}`,
          },
          'view-graph-stripe',
        ),
      );
    }
    const label = el('text', { x: c.x + 12, y: n.sub ? c.y + 19 : c.y + 27 }, 'view-graph-label');
    label.textContent = clip(n.label, Math.floor((c.width - 24) / CHAR_WIDTH));
    g.append(label);
    if (n.sub) {
      const sub = el('text', { x: c.x + 12, y: c.y + 35 }, 'view-graph-sub');
      sub.textContent = clip(n.sub, Math.floor((c.width - 24) / 6));
      g.append(sub);
    }
    wireNode(ctx, scene, n, g);
    nodeLayer.append(g);
  }
  return scene;
}

/** The force layout: circles placed by the wasm force solver, straight links between them, labels to the right. */
function forceScene(ctx: Ctx, graph: WasmGraph): Scene {
  const width = ctx.root.clientWidth || FORCE_WIDTH;
  const height = FORCE_HEIGHT;
  // The minimum gap the solver keeps between nodes (its default is 28). The circles themselves are drawn at NODE_RADIUS.
  const { positions } = graph.force(
    ctx.nodes.map((n) => n.id),
    ctx.edges.map((e): [string, string] => [e.from, e.to]),
    { width, height, node_size: 28 },
  );
  const at = (id: string): [number, number] => positions[id] ?? [width / 2, height / 2];

  const edgeLayer = el('g', {}, 'view-graph-edges') as SVGGElement;
  const nodeLayer = el('g', {}, 'view-graph-nodes') as SVGGElement;
  ctx.world.replaceChildren(edgeLayer, nodeLayer);

  const scene: Scene = { width, height, nodes: new Map(), edges: [] };
  for (const e of ctx.edges) {
    const [ax, ay] = at(e.from);
    const [bx, by] = at(e.to);
    const len = Math.hypot(bx - ax, by - ay);
    // The line stops just outside each circle, so an arrowhead lands on its edge.
    const ux = len > 0 ? (bx - ax) / len : 0;
    const uy = len > 0 ? (by - ay) / len : 0;
    const r = NODE_RADIUS + 1;
    const line = el(
      'line',
      {
        x1: ax + ux * r,
        y1: ay + uy * r,
        x2: bx - ux * r,
        y2: by - uy * r,
        ...(ctx.directed ? { 'marker-end': ctx.arrow } : {}),
      },
      'view-graph-edge',
    );
    edgeLayer.append(line);
    const els: SVGElement[] = [line];
    if (e.label) {
      els.push(edgeLabel(edgeLayer, (ax + bx) / 2, (ay + by) / 2, e.label));
    }
    scene.edges.push({ from: e.from, to: e.to, els });
  }

  for (const n of ctx.nodes) {
    const [x, y] = at(n.id);
    const g = el('g', { transform: `translate(${x} ${y})` }, 'view-graph-node') as SVGGElement;
    g.append(el('circle', { r: NODE_RADIUS, style: `fill: ${nodeColour(n, ctx.groups)}` }, 'view-graph-dot'));
    const label = el('text', { x: NODE_RADIUS + 5, y: 4 }, 'view-graph-label');
    label.textContent = clip(n.label, FORCE_LABEL_CHARS);
    g.append(label);
    wireNode(ctx, scene, n, g);
    nodeLayer.append(g);
  }
  return scene;
}

/** A node's hover: its edges and itself stay bright, the rest dims, and its sub shows in the tip. */
function wireNode(ctx: Ctx, scene: Scene, node: GraphNode, g: SVGGElement): void {
  scene.nodes.set(node.id, g);
  g.addEventListener('pointerenter', (ev: PointerEvent) => highlight(ctx, scene, node, ev));
  g.addEventListener('pointermove', (ev: PointerEvent) => placeTip(ctx, ev));
  g.addEventListener('pointerleave', () => highlight(ctx, scene, null));
}

function highlight(ctx: Ctx, scene: Scene, node: GraphNode | null, ev?: PointerEvent): void {
  const id = node ? node.id : null;
  ctx.root.classList.toggle('is-hovering', node !== null);
  for (const [nodeId, g] of scene.nodes) g.classList.toggle('is-hot', nodeId === id);
  for (const e of scene.edges) {
    const hot = id !== null && (e.from === id || e.to === id);
    for (const part of e.els) part.classList.toggle('is-hot', hot);
  }
  if (node && node.sub && ev) {
    const title = document.createElement('div');
    title.className = 'view-graph-tip-label';
    title.textContent = node.label;
    const sub = document.createElement('div');
    sub.className = 'view-graph-tip-sub';
    sub.textContent = node.sub;
    ctx.tip.replaceChildren(title, sub);
    ctx.tip.hidden = false;
    placeTip(ctx, ev);
  } else {
    ctx.tip.hidden = true;
  }
}

/** Places the tip beside the pointer, on the left when it would overflow the frame. */
function placeTip(ctx: Ctx, ev: PointerEvent): void {
  const box = ctx.root.getBoundingClientRect();
  const pointerX = ev.clientX - box.left;
  let left = pointerX + 12;
  if (left + ctx.tip.offsetWidth > box.width - 4) left = Math.max(4, pointerX - ctx.tip.offsetWidth - 12);
  ctx.tip.style.left = `${left}px`;
  ctx.tip.style.top = `${ev.clientY - box.top + 12}px`;
}

function edgeLabel(layer: SVGGElement, x: number, y: number, text: string): SVGElement {
  const label = el('text', { x, y: y - 6, 'text-anchor': 'middle' }, 'view-graph-edge-label');
  label.textContent = clip(text, EDGE_LABEL_CHARS);
  layer.append(label);
  return label;
}

/** The node's colour: its tone when it has one, else its group's hue, else the ink colour. */
function nodeColour(node: GraphNode, groups: Map<string, number>): string {
  if (node.tone) return toneColour(node.tone);
  if (node.group !== undefined) return `var(--hue-${(groups.get(node.group) ?? 0) % HUES})`;
  return 'var(--ink-3)';
}

function toneColour(tone: string): string {
  if (/^chart-[1-8]$/.test(tone) || ['ok', 'warn', 'crit', 'info', 'signal'].includes(tone)) {
    return `var(--${tone}, var(--ink-3))`;
  }
  if (tone === 'rule') return 'var(--rule-strong)';
  return 'var(--ink-3)';
}

/** The groups in order of first appearance, each with its index (the hue it takes). */
function groupIndex(nodes: GraphNode[]): Map<string, number> {
  const groups = new Map<string, number>();
  for (const n of nodes) {
    if (n.group !== undefined && !groups.has(n.group)) groups.set(n.group, groups.size);
  }
  return groups;
}

/** The first MAX_NODES nodes, with the links between them. */
function capGraph(body: GraphBody): { nodes: GraphNode[]; edges: GraphEdge[] } {
  const nodes = body.nodes.slice(0, MAX_NODES);
  const ids = new Set(nodes.map((n) => n.id));
  return { nodes, edges: body.edges.filter((e) => ids.has(e.from) && ids.has(e.to)) };
}

/** The card width for the widest label or sub, estimated at CHAR_WIDTH per character and kept within the card limits. */
function cardWidth(nodes: GraphNode[]): number {
  let widest = 0;
  for (const n of nodes) widest = Math.max(widest, n.label.length, n.sub?.length ?? 0);
  return Math.min(CARD_MAX, Math.max(CARD_MIN, widest * CHAR_WIDTH + 24));
}

/** The point halfway along a polyline's length. */
function midpoint(points: [number, number][]): [number, number] {
  const segments: [[number, number], [number, number]][] = [];
  for (let i = 1; i < points.length; i++) {
    const a = points[i - 1];
    const b = points[i];
    if (a && b) segments.push([a, b]);
  }
  const length = (s: [[number, number], [number, number]]): number => Math.hypot(s[1][0] - s[0][0], s[1][1] - s[0][1]);
  let half = segments.reduce((sum, s) => sum + length(s), 0) / 2;
  for (const s of segments) {
    const len = length(s);
    if (len > 0 && half <= len) {
      const t = half / len;
      const [a, b] = s;
      return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
    }
    half -= len;
  }
  return points[points.length - 1] ?? [0, 0];
}

function edgeKey(from: string, to: string): string {
  return `${from}\u0000${to}`;
}

function clip(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, Math.max(0, max - 1))}…` : text;
}

function clamp(value: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, value));
}

function reason(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** An SVG element with its attributes and class set; attribute values are strings or numbers, never HTML. */
function el(tag: string, attrs: Record<string, string | number>, cls?: string): SVGElement {
  const node = document.createElementNS(SVG_NS, tag);
  for (const [name, value] of Object.entries(attrs)) node.setAttribute(name, String(value));
  if (cls) node.setAttribute('class', cls);
  return node;
}
