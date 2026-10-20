// The memory graph: an agent's notes as a force graph, drawn as the prototype's SVG. Each note is a ring
// with a glyph for its kind and its title beneath; `[[links]]` are solid lines, and notes that share a tag
// are joined by dashed ones. The layout is the desktop's: 240 cooling steps of repulsion, springs along links
// and shared tags, and a pull to the middle. It runs once per change, starting from where each note sat before,
// so a new note does not reshuffle the picture. Clicking (or Enter / Space on) a note opens it.

import type { MemoryNote } from '../api';

type Kind = MemoryNote['kind'];
interface Spot { x: number; y: number }
interface Pt extends Spot { slug: string; fx: number; fy: number }
interface Edge { a: string; b: string; kind: 'link' | 'tag' }

export interface MemoryGraph {
  /** Replaces the notes and lays them out again; notes already drawn keep their place. */
  update(notes: MemoryNote[]): void;
  /** Marks the note being read (`null` clears it). */
  select(slug: string | null): void;
  destroy(): void;
}

const NS = 'http://www.w3.org/2000/svg';
const W = 640;
const H = 360;
const STEPS = 240;
const KG: Record<Kind, string> = { note: '¶', person: '@', journal: '#' };

function el<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string | number> = {}, ...kids: Node[]): SVGElementTagNameMap[K] {
  const e = document.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
  e.append(...kids);
  return e;
}

function text<K extends 'text' | 'title'>(tag: K, attrs: Record<string, string | number>, value: string): SVGElementTagNameMap[K] {
  const e = el(tag, attrs);
  e.textContent = value;
  return e;
}

/** Mirrors the server's slug rule, so a `[[link]]` written as a title still finds its note. */
function slugOf(notes: MemoryNote[], ref: string): string | null {
  if (notes.some((n) => n.slug === ref)) return ref;
  const want = ref.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 60);
  return notes.find((n) => n.slug === want)?.slug ?? null;
}

function edgesOf(notes: MemoryNote[]): Edge[] {
  const edges: Edge[] = [];
  const seen = new Set<string>();
  const add = (a: string, b: string, kind: Edge['kind']): void => {
    const key = a < b ? `${a}|${b}` : `${b}|${a}`;
    if (a === b || seen.has(key)) return;
    seen.add(key);
    edges.push({ a, b, kind });
  };
  for (const n of notes) {
    for (const ref of n.links) {
      const target = slugOf(notes, ref);
      if (target) add(n.slug, target, 'link');
    }
  }
  notes.forEach((x, i) => {
    for (const y of notes.slice(i + 1)) if (x.tags.some((t) => y.tags.includes(t))) add(x.slug, y.slug, 'tag');
  });
  return edges;
}

/** The force layout, run once for the set: repulsion between notes, springs along links (long) and shared tags (short). */
function layout(notes: MemoryNote[], edges: Edge[], prev: Map<string, Spot>): Map<string, Pt> {
  const pts: Pt[] = notes.map((n, i) => {
    const old = prev.get(n.slug);
    const t = (i / Math.max(notes.length, 1)) * Math.PI * 2;
    return { slug: n.slug, x: old?.x ?? W / 2 + Math.cos(t) * 130, y: old?.y ?? H / 2 + Math.sin(t) * 100, fx: 0, fy: 0 };
  });
  const at = new Map(pts.map((p) => [p.slug, p]));
  for (let it = 0; it < STEPS; it += 1) {
    const cool = 1 - it / 260;
    for (const p of pts) {
      p.fx = (W / 2 - p.x) * 0.012;
      p.fy = (H / 2 - p.y) * 0.012;
    }
    for (let i = 0; i < pts.length; i += 1) {
      for (let j = i + 1; j < pts.length; j += 1) {
        const a = pts[i]!;
        const b = pts[j]!;
        const dx = a.x - b.x;
        const dy = a.y - b.y;
        const d = Math.max(Math.hypot(dx, dy), 1);
        const f = 3200 / (d * d);
        a.fx += (dx / d) * f;
        a.fy += (dy / d) * f;
        b.fx -= (dx / d) * f;
        b.fy -= (dy / d) * f;
      }
    }
    for (const e of edges) {
      const a = at.get(e.a)!;
      const b = at.get(e.b)!;
      const dx = b.x - a.x;
      const dy = b.y - a.y;
      const d = Math.max(Math.hypot(dx, dy), 1);
      const f = (d - (e.kind === 'link' ? 140 : 190)) * 0.035;
      a.fx += (dx / d) * f;
      a.fy += (dy / d) * f;
      b.fx -= (dx / d) * f;
      b.fy -= (dy / d) * f;
    }
    for (const p of pts) {
      p.x = Math.min(W - 70, Math.max(70, p.x + p.fx * cool));
      p.y = Math.min(H - 48, Math.max(48, p.y + p.fy * cool));
    }
  }
  return at;
}

export function createMemoryGraph(host: HTMLElement, onOpen: (slug: string) => void): MemoryGraph {
  const svg = el('svg', { class: 'mg', viewBox: `0 0 ${W} ${H}`, role: 'img', 'aria-label': 'Memory graph' });
  host.replaceChildren(svg);
  const spots = new Map<string, Spot>();
  const nodeEls = new Map<string, SVGGElement>();
  let selected: string | null = null;

  const update = (notes: MemoryNote[]): void => {
    const edges = edgesOf(notes);
    const at = layout(notes, edges, spots);
    for (const p of at.values()) spots.set(p.slug, { x: p.x, y: p.y });
    nodeEls.clear();

    const lines = edges.map((e) => {
      const a = at.get(e.a)!;
      const b = at.get(e.b)!;
      return el('line', {
        x1: a.x.toFixed(1),
        y1: a.y.toFixed(1),
        x2: b.x.toFixed(1),
        y2: b.y.toFixed(1),
        class: e.kind === 'link' ? 'mg-link' : 'mg-tag',
      });
    });
    const dots = notes.map((n) => {
      const p = at.get(n.slug)!;
      const x = p.x.toFixed(1);
      const y = p.y.toFixed(1);
      const g = el('g', { class: n.slug === selected ? 'mg-node on' : 'mg-node', 'data-slug': n.slug, role: 'button', tabindex: 0, 'aria-label': n.title });
      g.append(
        text('title', {}, n.title),
        el('circle', { cx: x, cy: y, r: 18 }),
        text('text', { class: 'mg-glyph', x, y: (p.y + 5).toFixed(1) }, KG[n.kind] ?? '•'),
        text('text', { class: 'mg-name', x, y: (p.y + 34).toFixed(1) }, n.title),
      );
      g.addEventListener('click', () => onOpen(n.slug));
      g.addEventListener('keydown', (e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          onOpen(n.slug);
        }
      });
      nodeEls.set(n.slug, g);
      return g;
    });
    svg.setAttribute('aria-label', `Memory graph, ${notes.length} notes`);
    svg.replaceChildren(...lines, ...dots);
  };

  return {
    update,
    select(slug) {
      selected = slug;
      for (const [s, g] of nodeEls) g.classList.toggle('on', s === slug);
    },
    destroy() {
      host.replaceChildren();
    },
  };
}
