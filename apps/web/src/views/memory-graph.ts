// The memory graph: an agent's notes as a live, cartoon force graph. Notes are inked bubbles with
// a glyph for their kind and a name tag; `[[links]]` are ink strokes, shared tags faint dashes.
// The layout runs continuously (repulsion, collision that respects the name tags, springs along
// links, a gentle pull to the middle) and settles; it reheats when notes change or a node is
// dragged. Wheel zooms around the pointer, dragging the paper pans, dragging a note moves it.

import type { MemoryNote } from '../api';

type Kind = MemoryNote['kind'];

interface GNode {
  slug: string; title: string; kind: Kind; tags: string[];
  r: number; tagW: number; x: number; y: number; vx: number; vy: number;
  fx: number | null; fy: number | null;
  el: SVGGElement; body: SVGGElement; label: SVGGElement;
}
interface Link { a: GNode; b: GNode; tag: boolean; el: SVGPathElement }

export interface MemoryGraph {
  /** Replaces the notes; positions carry over, new notes pop in, changed ones ping. */
  update(notes: MemoryNote[], changed?: Set<string>): void;
  /** Highlights a note and its neighbourhood (`null` clears). */
  select(slug: string | null, extra?: Set<string>): void;
  /** Dims notes whose title doesn't match. */
  filter(query: string): void;
  /** Moves the camera to a note. */
  focus(slug: string): void;
  fit(): void;
  destroy(): void;
}

const NS = 'http://www.w3.org/2000/svg';
const INK = '#2b2420';
export const KIND_COLOR: Record<Kind, string> = { note: '#8fb8ff', person: '#f5a3c7', journal: '#f7d154' };
const LINK_LEN = 150;
const TAG_LEN = 230;

function el<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string | number> = {}, ...kids: globalThis.Node[]): SVGElementTagNameMap[K] {
  const e = document.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
  for (const kid of kids) e.append(kid);
  return e;
}

const clip = (s: string, n: number): string => (s.length > n ? `${s.slice(0, n - 1)}…` : s);
const radiusOf = (size: number): number => 17 + 4 * Math.log10(size / 200 + 1);

/** A small ink glyph for the kind: a notebook, a person, or a page. */
function glyph(kind: Kind): SVGElement {
  const s = { fill: 'none', stroke: INK, 'stroke-width': 2.2, 'stroke-linecap': 'round', 'stroke-linejoin': 'round' };
  if (kind === 'journal') {
    return el('g', {}, el('rect', { ...s, x: -7, y: -8, width: 14, height: 16, rx: 2 }), el('path', { ...s, d: 'M-3 -3h6M-3 1h6M-3 5h4' }), el('path', { ...s, d: 'M-7 -8v16', 'stroke-width': 3.2 }));
  }
  if (kind === 'person') {
    return el('g', {}, el('circle', { ...s, cx: 0, cy: -3.5, r: 4 }), el('path', { ...s, d: 'M-7.5 8c1-5 4-6.5 7.5-6.5s6.5 1.5 7.5 6.5' }));
  }
  return el('g', {}, el('path', { ...s, d: 'M-6 -8h8l4 4v12h-12z' }), el('path', { ...s, d: 'M2 -8v4h4M-3 1h6M-3 4.5h6' }));
}

export function createMemoryGraph(host: HTMLElement, onOpen: (slug: string) => void): MemoryGraph {
  const svg = el('svg', { class: 'memg', role: 'img', 'aria-label': 'Memory graph' });
  const defs = el(
    'defs',
    {},
    el('pattern', { id: 'memg-dots', width: 26, height: 26, patternUnits: 'userSpaceOnUse' }, el('circle', { cx: 2, cy: 2, r: 1.4, fill: 'rgba(43,36,32,0.18)' })),
  );
  const paper = el('rect', { class: 'memg-paper', x: -5000, y: -5000, width: 10000, height: 10000, fill: 'url(#memg-dots)' });
  const view = el('g');
  const linkLayer = el('g');
  const nodeLayer = el('g');
  view.append(paper, linkLayer, nodeLayer);
  svg.append(defs, view);

  const controls = document.createElement('div');
  controls.className = 'memg-controls';
  const btn = (label: string, title: string, fn: () => void): HTMLButtonElement => {
    const b = document.createElement('button');
    b.type = 'button';
    b.textContent = label;
    b.title = title;
    b.addEventListener('click', fn);
    controls.append(b);
    return b;
  };
  const legend = document.createElement('div');
  legend.className = 'memg-legend';
  for (const [kind, colour] of Object.entries(KIND_COLOR)) {
    const item = document.createElement('span');
    const dot = document.createElement('i');
    dot.style.background = colour;
    item.append(dot, document.createTextNode(kind));
    legend.append(item);
  }
  host.replaceChildren(svg, controls, legend);

  let nodes: GNode[] = [];
  let links: Link[] = [];
  const bySlug = new Map<string, GNode>();
  const cam = { k: 1, x: 0, y: 0 };
  let touched = false; // the user moved the camera: stop auto-fitting
  let alpha = 1;
  let raf = 0;
  let selected: string | null = null;
  let hood = new Set<string>();
  let query = '';

  const size = (): { w: number; h: number } => {
    const r = svg.getBoundingClientRect();
    return { w: Math.max(1, r.width), h: Math.max(1, r.height) };
  };
  // Name tags keep a readable size when zoomed out: they counter-scale below zoom 1.
  let labelScale = 1;
  const applyCam = (): void => {
    view.setAttribute('transform', `translate(${cam.x.toFixed(1)} ${cam.y.toFixed(1)}) scale(${cam.k.toFixed(4)})`);
    const s = Math.min(1.8, Math.max(1, 1 / cam.k));
    if (Math.abs(s - labelScale) > 0.01) {
      labelScale = s;
      for (const p of nodes) p.label.setAttribute('transform', `translate(0 ${p.r}) scale(${s.toFixed(3)}) translate(0 ${-p.r})`);
    }
  };
  const toWorld = (cx: number, cy: number): { x: number; y: number } => {
    const r = svg.getBoundingClientRect();
    return { x: (cx - r.left - cam.x) / cam.k, y: (cy - r.top - cam.y) / cam.k };
  };

  // ---- Simulation --------------------------------------------------------------------------

  const tick = (): void => {
    const n = nodes.length;
    for (let i = 0; i < n; i++) {
      const a = nodes[i]!;
      for (let j = i + 1; j < n; j++) {
        const b = nodes[j]!;
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let d2 = dx * dx + dy * dy;
        if (d2 < 1) {
          dx = Math.random() - 0.5;
          dy = Math.random() - 0.5;
          d2 = 1;
        }
        const d = Math.sqrt(d2);
        // Repulsion: everything keeps its distance.
        const f = (14000 / d2) * alpha;
        a.vx -= (dx / d) * f;
        a.vy -= (dy / d) * f;
        b.vx += (dx / d) * f;
        b.vy += (dy / d) * f;
        // Collision on the bubble plus its name tag, so labels never pile up.
        const ra = Math.max(a.r, a.tagW / 2) + 14;
        const rb = Math.max(b.r, b.tagW / 2) + 14;
        const overlap = ra + rb - d;
        if (overlap > 0) {
          const push = (overlap / d) * 0.5;
          a.x -= dx * push;
          a.y -= dy * push * 0.6;
          b.x += dx * push;
          b.y += dy * push * 0.6;
        }
      }
    }
    for (const l of links) {
      const dx = l.b.x - l.a.x;
      const dy = l.b.y - l.a.y;
      const d = Math.hypot(dx, dy) || 1;
      const k = (l.tag ? 0.012 : 0.05) * alpha;
      const f = ((d - (l.tag ? TAG_LEN : LINK_LEN)) / d) * k;
      l.a.vx += dx * f;
      l.a.vy += dy * f;
      l.b.vx -= dx * f;
      l.b.vy -= dy * f;
    }
    for (const p of nodes) {
      p.vx -= p.x * 0.008 * alpha;
      p.vy -= p.y * 0.008 * alpha;
      p.vx *= 0.6;
      p.vy *= 0.6;
      if (p.fx !== null && p.fy !== null) {
        p.x = p.fx;
        p.y = p.fy;
        p.vx = p.vy = 0;
      } else {
        p.x += p.vx;
        p.y += p.vy;
      }
    }
  };

  const draw = (): void => {
    for (const p of nodes) p.el.setAttribute('transform', `translate(${p.x.toFixed(1)} ${p.y.toFixed(1)})`);
    for (const l of links) {
      const mx = (l.a.x + l.b.x) / 2;
      const my = (l.a.y + l.b.y) / 2;
      const dx = l.b.x - l.a.x;
      const dy = l.b.y - l.a.y;
      const bend = l.tag ? 0 : 0.12;
      l.el.setAttribute('d', `M${l.a.x.toFixed(1)} ${l.a.y.toFixed(1)} Q${(mx - dy * bend).toFixed(1)} ${(my + dx * bend).toFixed(1)} ${l.b.x.toFixed(1)} ${l.b.y.toFixed(1)}`);
    }
  };

  const loop = (): void => {
    raf = 0;
    tick();
    draw();
    alpha *= 0.985;
    if (!touched && alpha < 0.25 && alpha > 0.2) fit(false);
    if (alpha > 0.004) raf = requestAnimationFrame(loop);
    else if (!touched) fit(true);
  };
  const reheat = (to = 0.6): void => {
    alpha = Math.max(alpha, to);
    if (!raf) raf = requestAnimationFrame(loop);
  };

  // ---- Camera --------------------------------------------------------------------------------

  let tween = 0;
  const moveCam = (k: number, x: number, y: number, animate: boolean): void => {
    cancelAnimationFrame(tween);
    if (!animate) {
      Object.assign(cam, { k, x, y });
      applyCam();
      return;
    }
    const from = { ...cam };
    const t0 = performance.now();
    const step = (now: number): void => {
      const t = Math.min(1, (now - t0) / 380);
      const e = 1 - Math.pow(1 - t, 3);
      cam.k = from.k + (k - from.k) * e;
      cam.x = from.x + (x - from.x) * e;
      cam.y = from.y + (y - from.y) * e;
      applyCam();
      if (t < 1) tween = requestAnimationFrame(step);
    };
    tween = requestAnimationFrame(step);
  };

  function fit(animate = true): void {
    if (!nodes.length) return;
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (const p of nodes) {
      const half = Math.max(p.r, p.tagW / 2);
      x0 = Math.min(x0, p.x - half); x1 = Math.max(x1, p.x + half);
      y0 = Math.min(y0, p.y - p.r); y1 = Math.max(y1, p.y + p.r + 30);
    }
    const { w, h } = size();
    const k = Math.min(1.6, Math.max(0.25, Math.min((w - 60) / (x1 - x0 || 1), (h - 60) / (y1 - y0 || 1))));
    moveCam(k, w / 2 - ((x0 + x1) / 2) * k, h / 2 - ((y0 + y1) / 2) * k, animate);
  }

  const zoomAt = (factor: number, cx: number, cy: number): void => {
    const r = svg.getBoundingClientRect();
    const px = cx - r.left;
    const py = cy - r.top;
    const k = Math.min(3, Math.max(0.2, cam.k * factor));
    moveCam(k, px - ((px - cam.x) / cam.k) * k, py - ((py - cam.y) / cam.k) * k, false);
    touched = true;
  };

  btn('+', 'Zoom in', () => {
    const r = svg.getBoundingClientRect();
    zoomAt(1.3, r.left + r.width / 2, r.top + r.height / 2);
  });
  btn('−', 'Zoom out', () => {
    const r = svg.getBoundingClientRect();
    zoomAt(1 / 1.3, r.left + r.width / 2, r.top + r.height / 2);
  });
  btn('⤢', 'Fit all notes', () => {
    touched = false;
    fit(true);
  });

  svg.addEventListener('wheel', (ev) => {
    ev.preventDefault();
    zoomAt(Math.exp(-ev.deltaY * 0.0015), ev.clientX, ev.clientY);
  }, { passive: false });

  // Dragging: the paper pans, a note moves (a short press without movement opens it).
  let drag: { node: GNode | null; sx: number; sy: number; cx: number; cy: number; moved: boolean } | null = null;
  svg.addEventListener('pointerdown', (ev) => {
    const g = (ev.target as Element).closest('g.memg-node');
    const node = g ? bySlug.get(g.getAttribute('data-slug') ?? '') ?? null : null;
    drag = { node, sx: ev.clientX, sy: ev.clientY, cx: cam.x, cy: cam.y, moved: false };
    svg.setPointerCapture(ev.pointerId);
    svg.classList.add('grabbing');
  });
  svg.addEventListener('pointermove', (ev) => {
    if (!drag) return;
    const dx = ev.clientX - drag.sx;
    const dy = ev.clientY - drag.sy;
    if (!drag.moved && Math.hypot(dx, dy) < 4) return;
    drag.moved = true;
    if (drag.node) {
      const w = toWorld(ev.clientX, ev.clientY);
      drag.node.fx = w.x;
      drag.node.fy = w.y;
      reheat(0.3);
    } else {
      touched = true;
      moveCam(cam.k, drag.cx + dx, drag.cy + dy, false);
    }
  });
  const release = (ev: PointerEvent): void => {
    if (!drag) return;
    const { node, moved } = drag;
    drag = null;
    svg.classList.remove('grabbing');
    if (svg.hasPointerCapture(ev.pointerId)) svg.releasePointerCapture(ev.pointerId);
    if (node) {
      node.fx = node.fy = null;
      if (!moved) onOpen(node.slug);
    }
  };
  svg.addEventListener('pointerup', release);
  svg.addEventListener('pointercancel', release);

  // ---- Rendering -----------------------------------------------------------------------------

  const paint = (): void => {
    for (const p of nodes) {
      const cls = ['memg-node', `k-${p.kind}`];
      if (p.slug === selected) cls.push('sel');
      else if (selected && !hood.has(p.slug)) cls.push('dim');
      if (query && !p.title.toLowerCase().includes(query)) cls.push('off');
      p.el.setAttribute('class', cls.join(' '));
    }
    for (const l of links) {
      const on = selected !== null && (l.a.slug === selected || l.b.slug === selected);
      l.el.setAttribute('class', `memg-link${l.tag ? ' tag' : ''}${on ? ' on' : selected ? ' dim' : ''}`);
    }
  };

  const makeNode = (n: MemoryNote): GNode => {
    const r = radiusOf(n.size);
    const text = clip(n.title, 26);
    const tagW = text.length * 6.7 + 18;
    const body = el('g', { class: 'memg-body' });
    body.append(
      el('circle', { class: 'memg-shadow', cx: 3, cy: 3.5, r }),
      el('circle', { class: 'memg-dot', r, fill: KIND_COLOR[n.kind] }),
      el('circle', { class: 'memg-shine', cx: -r * 0.35, cy: -r * 0.4, r: r * 0.28 }),
      glyph(n.kind),
    );
    const label = el('text', { class: 'memg-text', y: r + 21, 'text-anchor': 'middle' });
    label.textContent = text;
    const tag = el('rect', { class: 'memg-tag', x: -tagW / 2, y: r + 7, width: tagW, height: 20, rx: 7 });
    const title = el('title');
    title.textContent = n.title;
    const g = el('g', { class: 'memg-node', 'data-slug': n.slug, tabindex: 0, role: 'button', 'aria-label': n.title });
    const labelGroup = el('g', { transform: `translate(0 ${r}) scale(${labelScale.toFixed(3)}) translate(0 ${-r})` });
    labelGroup.append(tag, label);
    g.append(title, body, labelGroup);
    g.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        onOpen(n.slug);
      }
    });
    return { slug: n.slug, title: n.title, kind: n.kind, tags: n.tags, r, tagW, x: 0, y: 0, vx: 0, vy: 0, fx: null, fy: null, el: g, body, label: labelGroup };
  };

  const resolve = (notes: MemoryNote[], ref: string): string | null => {
    if (bySlug.has(ref)) return ref;
    const want = ref.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 60);
    return notes.find((n) => n.slug === want)?.slug ?? null;
  };

  const ping = (p: GNode, cls: string): void => {
    p.body.classList.remove(cls);
    void p.body.getBoundingClientRect();
    p.body.classList.add(cls);
  };

  const update = (notes: MemoryNote[], changed = new Set<string>()): void => {
    const keep = new Set(notes.map((n) => n.slug));
    for (const p of nodes) if (!keep.has(p.slug)) {
      p.el.remove();
      bySlug.delete(p.slug);
    }
    const fresh: GNode[] = [];
    nodes = notes.map((n) => {
      let p = bySlug.get(n.slug);
      if (!p) {
        p = makeNode(n);
        bySlug.set(n.slug, p);
        nodeLayer.append(p.el);
        fresh.push(p);
      } else if (p.title !== n.title || p.kind !== n.kind) {
        const next = makeNode(n);
        Object.assign(next, { x: p.x, y: p.y });
        p.el.replaceWith(next.el);
        bySlug.set(n.slug, next);
        p = next;
      }
      p.tags = n.tags;
      return p;
    });
    // Links: [[links]] as ink, and a faint dash between notes sharing a tag.
    for (const l of links) l.el.remove();
    links = [];
    const seen = new Set<string>();
    const add = (a: GNode, b: GNode, tag: boolean): void => {
      const key = a.slug < b.slug ? `${a.slug}|${b.slug}` : `${b.slug}|${a.slug}`;
      if (a === b || seen.has(key)) return;
      seen.add(key);
      const path = el('path', { class: 'memg-link' });
      linkLayer.append(path);
      links.push({ a, b, tag, el: path });
    };
    for (const n of notes) {
      const a = bySlug.get(n.slug)!;
      for (const ref of n.links) {
        const slug = resolve(notes, ref);
        if (slug) add(a, bySlug.get(slug)!, false);
      }
    }
    const byTag = new Map<string, GNode[]>();
    for (const p of nodes) for (const t of p.tags) byTag.set(t, [...(byTag.get(t) ?? []), p]);
    for (const group of byTag.values()) for (let i = 1; i < group.length; i++) add(group[i - 1]!, group[i]!, true);
    // New notes start next to a note they link to (or near the middle) and pop in.
    for (const p of fresh) {
      const friend = links.find((l) => (l.a === p && !fresh.includes(l.b)) || (l.b === p && !fresh.includes(l.a)));
      const anchor = friend ? (friend.a === p ? friend.b : friend.a) : null;
      const a = Math.random() * Math.PI * 2;
      p.x = (anchor?.x ?? 0) + Math.cos(a) * 60;
      p.y = (anchor?.y ?? 0) + Math.sin(a) * 60;
      if (nodes.length > fresh.length) ping(p, 'pop');
    }
    for (const slug of changed) {
      const p = bySlug.get(slug);
      if (p && !fresh.includes(p)) ping(p, 'ping');
    }
    paint();
    draw();
    reheat(fresh.length ? 0.8 : 0.3);
  };

  const ro = new ResizeObserver(() => {
    if (!touched) fit(false);
  });
  ro.observe(svg);

  return {
    update,
    select(slug, extra = new Set()) {
      selected = slug;
      hood = new Set(extra);
      if (slug) {
        hood.add(slug);
        for (const l of links) {
          if (l.tag) continue;
          if (l.a.slug === slug) hood.add(l.b.slug);
          if (l.b.slug === slug) hood.add(l.a.slug);
        }
      }
      paint();
    },
    filter(q) {
      query = q.trim().toLowerCase();
      paint();
    },
    focus(slug) {
      const p = bySlug.get(slug);
      if (!p) return;
      const { w, h } = size();
      const k = Math.max(cam.k, 0.9);
      touched = true;
      moveCam(k, w / 2 - p.x * k, h / 2 - p.y * k, true);
    },
    fit() {
      touched = false;
      fit(true);
    },
    destroy() {
      cancelAnimationFrame(raf);
      cancelAnimationFrame(tween);
      ro.disconnect();
      host.replaceChildren();
    },
  };
}
