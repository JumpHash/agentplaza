// Agent memory: the agent's notes as a little brain (left) and a reader (right).
// Framework-free. DOM via h(); text only ever reaches the page as textContent.

import '../styles/memory.css';
import { api, type MemoryDoc, type MemoryNote } from '../api';
import { ago, badge, clip, empty, input, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';

type Kind = MemoryNote['kind'];
interface GNode { slug: string; title: string; kind: Kind; r: number; x: number; y: number; vx: number; vy: number }
interface GPair { a: GNode; b: GNode }
type Resolve = (ref: string) => string | null;
type Open = (slug: string) => void;

const NS = 'http://www.w3.org/2000/svg';
const W = 640;
const H = 520;
const CX = W / 2;
const CY = H / 2;
const BLOB_RX = 292;
const BLOB_RY = 232;
const NODE_RX = 262; // node centres stay inside the blob outline
const NODE_RY = 204;
const REFRESH_MS = 10_000;

const KIND_COLOR: Record<Kind, string> = { note: '#8fb8ff', person: '#f2a6c8', journal: '#f7d154' };
const KIND_TONE: Record<Kind, Tone> = { note: 'info', person: 'accent', journal: 'warn' };

const FENCE = /^\s*```/;
const HEADING = /^(#{1,3})\s+(.*)$/;
const ITEM = /^\s*[-*]\s+/;
const INLINE = /\[\[([^\]\n]+)\]\]|\*\*([^*\n]+)\*\*|\*([^*\s][^*\n]*)\*|`([^`\n]+)`/g;

/** Mirrors the server's slug rule: lowercase, runs of non [a-z0-9] become '-', trimmed, at most 60. */
function slugify(text: string): string {
  return text.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 60).replace(/-+$/, '');
}

function svgEl<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string | number> = {}): SVGElementTagNameMap[K] {
  const el = document.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, String(v));
  return el;
}

const radiusOf = (size: number): number => Math.min(24, 9 + 2.2 * Math.log10(size + 10));

/** A closed, slightly lumpy outline through Catmull-Rom curves: the cartoon brain. */
function blobPath(): string {
  const n = 10;
  const pts = Array.from({ length: n }, (_, i) => {
    const t = (i / n) * Math.PI * 2;
    const k = 1 + 0.07 * Math.sin(3 * t + 1) + 0.04 * Math.cos(5 * t);
    return { x: CX + Math.cos(t) * BLOB_RX * k, y: CY + Math.sin(t) * BLOB_RY * k };
  });
  const at = (i: number) => pts[(i + n) % n]!;
  let d = `M${at(0).x.toFixed(1)},${at(0).y.toFixed(1)}`;
  for (let i = 0; i < n; i++) {
    const p0 = at(i - 1);
    const p1 = at(i);
    const p2 = at(i + 1);
    const p3 = at(i + 2);
    d += ` C${(p1.x + (p2.x - p0.x) / 6).toFixed(1)},${(p1.y + (p2.y - p0.y) / 6).toFixed(1)} ${(p2.x - (p3.x - p1.x) / 6).toFixed(1)},${(p2.y - (p3.y - p1.y) / 6).toFixed(1)} ${p2.x.toFixed(1)},${p2.y.toFixed(1)}`;
  }
  return `${d}Z`;
}

/** A gently bent quadratic curve between two nodes. */
function curve(a: GNode, b: GNode): string {
  const qx = (a.x + b.x) / 2 - (b.y - a.y) * 0.16;
  const qy = (a.y + b.y) / 2 + (b.x - a.x) * 0.16;
  return `M${a.x.toFixed(1)},${a.y.toFixed(1)} Q${qx.toFixed(1)},${qy.toFixed(1)} ${b.x.toFixed(1)},${b.y.toFixed(1)}`;
}

/** Repulsion between all nodes, springs along links, a pull to the centre, kept inside the blob. */
function simulate(nodes: GNode[], pairs: GPair[], ticks: number): void {
  for (let t = 0; t < ticks; t++) {
    const heat = 1 - t / ticks;
    for (const a of nodes) {
      for (const b of nodes) {
        if (a === b) continue;
        const dx = a.x - b.x;
        const dy = a.y - b.y;
        const d2 = Math.max(1, dx * dx + dy * dy);
        const d = Math.sqrt(d2);
        // Repulsion never cools off, or the centre pull would fold every note onto one spot.
        const f = (7000 / d2) * (0.5 + 0.5 * heat);
        a.vx += (dx / d) * f;
        a.vy += (dy / d) * f;
      }
    }
    for (const p of pairs) {
      const dx = p.b.x - p.a.x;
      const dy = p.b.y - p.a.y;
      const d = Math.hypot(dx, dy) || 1;
      const f = (d - 110) * 0.03 * heat;
      const fx = (dx / d) * f;
      const fy = (dy / d) * f;
      p.a.vx += fx;
      p.a.vy += fy;
      p.b.vx -= fx;
      p.b.vy -= fy;
    }
    for (const n of nodes) {
      n.vx = (n.vx + (CX - n.x) * 0.006) * 0.7;
      n.vy = (n.vy + (CY - n.y) * 0.006) * 0.7;
      const speed = Math.hypot(n.vx, n.vy);
      if (speed > 14) {
        n.vx = (n.vx / speed) * 14;
        n.vy = (n.vy / speed) * 14;
      }
      n.x += n.vx;
      n.y += n.vy;
      const k = Math.hypot((n.x - CX) / (NODE_RX - n.r), (n.y - CY) / (NODE_RY - n.r));
      if (k > 1) {
        n.x = CX + (n.x - CX) / k;
        n.y = CY + (n.y - CY) / k;
      }
    }
  }
}

/** Block markdown to DOM: headings, paragraphs, lists, fenced code, with inline spans. */
function markdown(src: string, resolve: Resolve, open: Open): Node[] {
  const lines = src.replace(/\r\n?/g, '\n').split('\n');
  const at = (j: number): string => lines[j] ?? '';
  const out: Node[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = at(i);
    if (FENCE.test(line)) {
      const code: string[] = [];
      for (i++; i < lines.length && !FENCE.test(at(i)); i++) code.push(at(i));
      i++;
      out.push(h('pre.mem-code', null, h('code', null, code.join('\n'))));
      continue;
    }
    const heading = HEADING.exec(line);
    if (heading) {
      const level = heading[1]?.length ?? 1;
      const tag: 'h1' | 'h2' | 'h3' = level === 1 ? 'h1' : level === 2 ? 'h2' : 'h3';
      out.push(h(tag, null, ...inline(heading[2] ?? '', resolve, open)));
      i++;
      continue;
    }
    if (ITEM.test(line)) {
      const items: Node[] = [];
      while (i < lines.length && ITEM.test(at(i))) {
        items.push(h('li', null, inline(at(i).replace(ITEM, ''), resolve, open)));
        i++;
      }
      out.push(h('ul', null, items));
      continue;
    }
    if (!line.trim()) {
      i++;
      continue;
    }
    const para: string[] = [];
    while (i < lines.length && at(i).trim() && !FENCE.test(at(i)) && !HEADING.test(at(i)) && !ITEM.test(at(i))) {
      para.push(at(i));
      i++;
    }
    out.push(h('p', null, inline(para.join(' '), resolve, open)));
  }
  return out;
}

/** Inline spans: [[Title]] chips, **bold**, *italic*, `code`. */
function inline(text: string, resolve: Resolve, open: Open): (Node | string)[] {
  const out: (Node | string)[] = [];
  let last = 0;
  for (const m of text.matchAll(INLINE)) {
    const start = m.index ?? 0;
    const [whole = '', wiki, bold, italic, code] = m;
    if (start > last) out.push(text.slice(last, start));
    if (wiki !== undefined) {
      const title = wiki.trim();
      const target = resolve(title);
      out.push(
        target
          ? h('button.chip.link', { type: 'button', onclick: () => open(target) }, title)
          : h('span.chip.missing', { title: 'No such note yet' }, title),
      );
    } else if (bold !== undefined) {
      out.push(h('strong', null, bold));
    } else if (italic !== undefined) {
      out.push(h('em', null, italic));
    } else if (code !== undefined) {
      out.push(h('code', null, code));
    }
    last = start + whole.length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

export function mountMemory(root: HTMLElement, cid: number): () => void {
  let alive = true;
  let loaded = false;
  let notes: MemoryNote[] = [];
  let nodes: GNode[] = [];
  let linkEls: { a: string; b: string; el: SVGPathElement }[] = [];
  let nodeEls = new Map<string, SVGGElement>();
  const positions = new Map<string, { x: number; y: number }>();
  let selected: string | null = null;
  let current: MemoryDoc | null = null;
  let backlinkSlugs = new Set<string>();
  let query = '';
  let readSeq = 0;

  const count = h('span.mem-count');
  const search = input({ type: 'search', placeholder: 'Search titles', 'aria-label': 'Search titles' });
  const brain = h('div.mem-brain');
  const reader = h('article.mem-reader', { 'aria-live': 'polite' });
  const body = h('div.mem-body', null, h('p.faint', null, 'Loading memory…'));

  search.addEventListener('input', () => {
    query = search.value.trim().toLowerCase();
    paint();
  });

  const resolveRef: Resolve = (ref) => {
    const want = slugify(ref);
    const hit = notes.find((n) => n.slug === ref || n.slug === want || slugify(n.title) === want);
    return hit ? hit.slug : null;
  };
  const titleOf = (ref: string): string => notes.find((n) => n.slug === resolveRef(ref))?.title ?? ref;

  /** Sets the classes that show the selection, its links and backlinks, and the search filter. */
  const paint = (): void => {
    const sel = selected;
    const hood = new Set<string>();
    if (sel !== null) {
      hood.add(sel);
      for (const l of linkEls) {
        if (l.a === sel) hood.add(l.b);
        if (l.b === sel) hood.add(l.a);
      }
      for (const s of backlinkSlugs) hood.add(s);
    }
    for (const n of nodes) {
      const g = nodeEls.get(n.slug);
      if (!g) continue;
      const cls = ['mem-node'];
      if (sel === n.slug) cls.push('sel');
      else if (sel !== null && !hood.has(n.slug)) cls.push('faded');
      if (query && !n.title.toLowerCase().includes(query)) cls.push('off');
      g.setAttribute('class', cls.join(' '));
    }
    for (const l of linkEls) {
      const touches = sel !== null && (l.a === sel || l.b === sel);
      const cls = ['mem-link'];
      if (touches) cls.push('on');
      else if (sel !== null) cls.push('faded');
      l.el.setAttribute('class', cls.join(' '));
    }
  };

  const renderReader = (doc: MemoryDoc): void => {
    const prov = doc.provenance;
    const stamp = [`Updated ${ago(doc.updated_ms)}`];
    if (prov.task) stamp.push(`task ${prov.task}`);
    if (prov.work !== null) stamp.push(`work #${prov.work}`);
    replace(
      reader,
      h(
        'header.mem-title',
        null,
        h('div.mem-meta', null, badge(doc.kind, KIND_TONE[doc.kind]), doc.tags.map((t) => h('span.chip', null, `#${t}`))),
        h('h1', null, doc.title),
      ),
      h('p.mem-stamp', { title: new Date(doc.updated_ms).toLocaleString() }, stamp.join(' · ')),
      h('div.mem-prose', null, markdown(doc.body, resolveRef, open)),
      doc.backlinks.length
        ? h(
            'footer.mem-linked',
            null,
            h('h3', null, 'Linked from'),
            h(
              'div.mem-chips',
              null,
              doc.backlinks.map((b) => {
                const slug = resolveRef(b);
                return slug
                  ? h('button.chip.link', { type: 'button', onclick: () => open(slug) }, titleOf(b))
                  : h('span.chip', null, b);
              }),
            ),
          )
        : null,
    );
  };

  const open = (slug: string): void => {
    selected = slug;
    backlinkSlugs = new Set();
    paint();
    const seq = ++readSeq;
    api.memoryNote(cid, slug).then(
      (doc) => {
        if (!alive || seq !== readSeq) return;
        current = doc;
        backlinkSlugs = new Set(doc.backlinks.map((b) => resolveRef(b)).filter((s): s is string => s !== null));
        renderReader(doc);
        paint();
      },
      (err: unknown) => {
        if (!alive || seq !== readSeq) return;
        replace(reader, empty('alert', 'Could not open this note', err instanceof Error ? err.message : undefined));
      },
    );
  };

  /** Lays the notes out as nodes; positions carry over between refreshes so the brain does not jump. */
  const drawGraph = (list: MemoryNote[]): void => {
    nodes = list.map((n, i) => {
      const prev = positions.get(n.slug);
      const angle = (i / Math.max(1, list.length)) * Math.PI * 2;
      return {
        slug: n.slug,
        title: n.title,
        kind: n.kind,
        r: radiusOf(n.size),
        x: prev?.x ?? CX + Math.cos(angle) * 80,
        y: prev?.y ?? CY + Math.sin(angle) * 80,
        vx: 0,
        vy: 0,
      };
    });
    const bySlug = new Map(nodes.map((n) => [n.slug, n] as const));
    const pairs: GPair[] = [];
    const seen = new Set<string>();
    for (const n of list) {
      const from = bySlug.get(n.slug);
      for (const ref of n.links) {
        const to = bySlug.get(ref) ?? bySlug.get(slugify(ref));
        if (!from || !to || to === from) continue;
        const key = from.slug < to.slug ? `${from.slug}|${to.slug}` : `${to.slug}|${from.slug}`;
        if (seen.has(key)) continue;
        seen.add(key);
        pairs.push({ a: from, b: to });
      }
    }
    simulate(nodes, pairs, 240);
    positions.clear();
    for (const n of nodes) positions.set(n.slug, { x: n.x, y: n.y });

    const svg = svgEl('svg', { viewBox: `0 0 ${W} ${H}`, role: 'img', 'aria-label': 'Memory graph' });
    svg.append(svgEl('path', { class: 'mem-blob', d: blobPath() }));
    const links = svgEl('g');
    linkEls = pairs.map((p) => {
      const el = svgEl('path', { class: 'mem-link', d: curve(p.a, p.b) });
      links.append(el);
      return { a: p.a.slug, b: p.b.slug, el };
    });
    const dots = svgEl('g');
    nodeEls = new Map();
    for (const n of nodes) {
      const g = svgEl('g', {
        class: 'mem-node',
        tabindex: 0,
        role: 'button',
        transform: `translate(${n.x.toFixed(1)} ${n.y.toFixed(1)})`,
      });
      g.setAttribute('aria-label', n.title);
      const dot = svgEl('circle', { r: n.r.toFixed(1), fill: KIND_COLOR[n.kind] });
      const label = svgEl('text', { y: (n.r + 14).toFixed(1), 'text-anchor': 'middle' });
      label.textContent = clip(n.title, 22);
      g.append(dot, label);
      g.addEventListener('click', () => open(n.slug));
      g.addEventListener('keydown', (e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          open(n.slug);
        }
      });
      dots.append(g);
      nodeEls.set(n.slug, g);
    }
    svg.append(links, dots);
    brain.replaceChildren(svg);
  };

  const refresh = async (): Promise<void> => {
    let list: MemoryNote[];
    try {
      list = await api.memory(cid);
    } catch (err) {
      if (!alive || loaded) return;
      replace(body, empty('alert', 'Could not load memory', err instanceof Error ? err.message : undefined));
      return;
    }
    if (!alive) return;
    loaded = true;
    notes = list;
    count.textContent = `${list.length} ${list.length === 1 ? 'note' : 'notes'}`;
    if (list.length === 0) {
      selected = null;
      current = null;
      nodes = [];
      linkEls = [];
      nodeEls = new Map();
      positions.clear();
      replace(body, empty('memory', 'No memories yet', 'Notes the agent writes appear here, linked like a little brain.'));
      return;
    }
    replace(body, brain, reader);
    drawGraph(list);
    const newest = list.reduce((best, n) => (n.updated_ms > best.updated_ms ? n : best));
    const target = list.find((n) => n.slug === selected) ?? newest;
    if (current && current.slug === target.slug && current.updated_ms === target.updated_ms && current.size === target.size) {
      selected = target.slug;
      paint();
    } else {
      open(target.slug);
    }
  };

  replace(root, h('div.mem-view', null, h('header.mem-head', null, count, search), body));
  void refresh();
  const timer = window.setInterval(() => void refresh(), REFRESH_MS);

  return () => {
    alive = false;
    window.clearInterval(timer);
    replace(root);
  };
}
