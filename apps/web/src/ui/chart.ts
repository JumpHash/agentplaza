// Hand-built SVG charts: stacked area, vertical bars, sparkline, and a number tween.
// No dependencies. Colours are CSS strings applied through element style (var() works there).

export interface ChartLayer { key: string; label: string; color: string; values: number[] }
export interface AreaData { labels: string[]; layers: ChartLayer[] }
export interface AreaOptions {
  format: (v: number) => string;
  tooltip?: (i: number) => Node | string;
  height?: number;
  ariaLabel?: string;
}
export interface ChartHandle<D> { update(data: D, opts?: Partial<AreaOptions>): void; destroy(): void }

export interface BarItem { key: string; label: string; value: number; color?: string }
export interface BarOptions { format: (v: number) => string; height?: number; ariaLabel?: string }

export interface SparkOptions { color?: string; height?: number; ariaLabel?: string }

const NS = 'http://www.w3.org/2000/svg';
const GRID = 'var(--chart-grid, rgba(128, 128, 128, 0.25))';
const AXIS = 'var(--chart-axis, currentColor)';
const DURATION = 400;

// ---- pure helpers ----

const clamp01 = (t: number): number => Math.min(1, Math.max(0, t));
const lerp = (a: number, b: number, k: number): number => a + (b - a) * k;
const zeros = (n: number): number[] => new Array<number>(n).fill(0);
const fit = (v: number[], n: number): number[] => Array.from({ length: n }, (_, i) => v[i] ?? 0);

export function niceScale(max: number, count = 4): number[] {
  if (!(max > 0) || !Number.isFinite(max)) return [0, 1];
  const raw = max / Math.max(1, count);
  const mag = 10 ** Math.floor(Math.log10(raw));
  const f = raw / mag;
  const step = (f <= 1 ? 1 : f <= 2 ? 2 : f <= 2.5 ? 2.5 : f <= 5 ? 5 : 10) * mag;
  const steps = Math.max(1, Math.ceil(max / step - 1e-9));
  return Array.from({ length: steps + 1 }, (_, k) => Number((k * step).toPrecision(12)));
}

export function resample(values: number[], n: number): number[] {
  if (n <= 0) return [];
  if (values.length === 0) return zeros(n);
  if (values.length === 1) return new Array<number>(n).fill(values[0] ?? 0);
  const last = values.length - 1;
  return Array.from({ length: n }, (_, i) => {
    const pos = n === 1 ? 0 : (i * last) / (n - 1);
    const lo = Math.floor(pos);
    const hi = Math.min(lo + 1, last);
    const f = pos - lo;
    return (values[lo] ?? 0) * (1 - f) + (values[hi] ?? 0) * f;
  });
}

export function tweenArray(from: number[], to: number[], t: number): number[] {
  const k = clamp01(t);
  const len = Math.max(from.length, to.length);
  return Array.from({ length: len }, (_, i) => {
    const a = from[i] ?? 0;
    return a + ((to[i] ?? 0) - a) * k;
  });
}

export function easeOut(t: number): number {
  return 1 - (1 - clamp01(t)) ** 3;
}

// Largest stacked total over the first n buckets.
function stackMax(rows: number[][], n: number): number {
  let m = 0;
  for (let i = 0; i < n; i++) {
    let s = 0;
    for (const r of rows) s += r[i] ?? 0;
    m = Math.max(m, s);
  }
  return m;
}

// Keeps next's order; removed items are re-inserted at their previous index and flagged gone.
function mergeKeys<T extends { key: string }>(prev: T[], next: T[]): { item: T; gone: boolean }[] {
  const keep = new Set(next.map((x) => x.key));
  const out: { item: T; gone: boolean }[] = next.map((item) => ({ item, gone: false }));
  prev.forEach((p, i) => {
    if (!keep.has(p.key)) out.splice(Math.min(i, out.length), 0, { item: p, gone: true });
  });
  return out;
}

// ---- DOM helpers ----

function svgEl<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string | number> = {}): SVGElementTagNameMap[K] {
  const node = document.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, String(v));
  return node;
}

function paint<T extends SVGElement>(node: T, style: Partial<CSSStyleDeclaration>): T {
  Object.assign(node.style, style);
  return node;
}

function txt(x: number, y: number, s: string, anchor: string): SVGTextElement {
  const t = svgEl('text', { x, y, 'text-anchor': anchor });
  t.textContent = s;
  return paint(t, { fill: AXIS, fontSize: '11px' });
}

const reduced = (): boolean =>
  typeof window !== 'undefined' && typeof window.matchMedia === 'function' &&
  window.matchMedia('(prefers-reduced-motion: reduce)').matches;

// Calls step(eased progress) each frame for ms, then done(). Returns a cancel function.
function animate(ms: number, step: (k: number) => void, done: () => void): () => void {
  if (reduced() || ms <= 0) {
    step(1);
    done();
    return () => {};
  }
  let id = 0;
  const t0 = performance.now();
  const tick = (now: number): void => {
    const t = clamp01((now - t0) / ms);
    step(easeOut(t));
    if (t < 1) id = requestAnimationFrame(tick);
    else done();
  };
  id = requestAnimationFrame(tick);
  return () => cancelAnimationFrame(id);
}

// Calls onChange when the element's width changes. Returns a disconnect function.
function watchWidth(el: HTMLElement, onChange: () => void): () => void {
  if (typeof ResizeObserver === 'undefined') return () => {};
  let last = el.clientWidth;
  const ro = new ResizeObserver(() => {
    if (el.clientWidth !== last) {
      last = el.clientWidth;
      onChange();
    }
  });
  ro.observe(el);
  return () => ro.disconnect();
}

// ---- countTo ----

interface CountEl extends HTMLElement { chartShown?: number; chartStop?: () => void }

export function countTo(el: HTMLElement, value: number, format: (v: number) => string, ms = 600): void {
  const e = el as CountEl;
  const from = e.chartShown ?? value;
  e.chartStop?.();
  e.chartStop = undefined;
  const show = (v: number): void => {
    e.chartShown = v;
    el.textContent = format(v);
  };
  if (from === value) {
    show(value);
    return;
  }
  e.chartStop = animate(ms, (k) => show(k >= 1 ? value : lerp(from, value, k)), () => show(value));
}

// ---- stackedArea ----

interface AreaGeo {
  W: number; n: number; x0: number; pw: number; pt: number; py1: number;
  x: (i: number) => number; y: (v: number) => number;
  tops: { color: string; upper: number[] }[];
}

export function stackedArea(el: HTMLElement, data: AreaData, opts: AreaOptions): ChartHandle<AreaData> {
  let o = opts;
  let labels: string[] = [];
  let state: { meta: ChartLayer; vals: number[] }[] = [];
  let maxRaw = 0;
  let hover = -1;
  let geo: AreaGeo | null = null;
  let cancel = (): void => {};

  const root = document.createElement('div');
  root.className = 'chart chart-area';
  const svg = svgEl('svg', { class: 'chart-svg', role: 'img' });
  const gBase = svgEl('g');
  const gHover = svgEl('g');
  svg.append(gBase, gHover);
  root.append(svg);
  const tip = document.createElement('div');
  tip.className = 'chart-tip';
  Object.assign(tip.style, { position: 'absolute', display: 'none', top: '8px', left: '0px', pointerEvents: 'none' });
  if (getComputedStyle(el).position === 'static') el.style.position = 'relative';
  el.append(root, tip);

  function paintHover(i: number): void {
    const g = geo;
    if (!g) return;
    const xi = g.x(i);
    const kids: SVGElement[] = [
      paint(svgEl('line', { class: 'chart-cross', x1: xi, x2: xi, y1: g.pt, y2: g.py1 }),
        { stroke: AXIS, strokeDasharray: '3 3', strokeWidth: '1px' }),
    ];
    for (const t of g.tops) {
      kids.push(paint(svgEl('circle', { cx: xi, cy: g.y(t.upper[i] ?? 0), r: 3 }),
        { fill: t.color, stroke: 'var(--chart-bg, #fff)', strokeWidth: '1.5px' }));
    }
    gHover.replaceChildren(...kids);
  }

  function draw(): void {
    const W = el.clientWidth || 640;
    const H = o.height ?? 260;
    const n = labels.length;
    const pl = 48, pr = 12, pt = 12, pb = 26;
    const px1 = W - pr, py1 = H - pb, pw = px1 - pl, ph = py1 - pt;
    const ticks = niceScale(maxRaw);
    const top = ticks[ticks.length - 1] ?? 1;
    const x = (i: number): number => (n <= 1 ? (pl + px1) / 2 : pl + (i * pw) / (n - 1));
    const y = (v: number): number => py1 - (v / top) * ph;
    svg.setAttribute('viewBox', `0 0 ${W} ${H}`);
    svg.setAttribute('aria-label', o.ariaLabel ?? 'Stacked area chart');
    Object.assign(svg.style, { width: '100%', height: `${H}px`, display: 'block' });

    const nodes: SVGElement[] = [];
    for (const t of ticks) {
      nodes.push(
        paint(svgEl('line', { x1: pl, x2: px1, y1: y(t), y2: y(t) }), { stroke: t === 0 ? AXIS : GRID, strokeWidth: '1px' }),
        txt(pl - 8, y(t) + 4, o.format(t), 'end'),
      );
    }

    const tops: { color: string; upper: number[] }[] = [];
    if (n > 0 && maxRaw > 0) {
      let base = zeros(n);
      for (const s of state) {
        const lower = base;
        const upper = base.map((b, i) => b + (s.vals[i] ?? 0));
        tops.push({ color: s.meta.color, upper });
        const edge = upper.map((v, i) => `${i ? 'L' : 'M'}${x(i)},${y(v)}`).join('');
        let back = '';
        for (let i = n - 1; i >= 0; i--) back += `L${x(i)},${y(lower[i] ?? 0)}`;
        nodes.push(
          paint(svgEl('path', { d: `${edge}${back}Z` }), { fill: s.meta.color, fillOpacity: '0.85' }),
          paint(svgEl('path', { d: edge }), { fill: 'none', stroke: s.meta.color, strokeWidth: '1px' }),
        );
        base = upper;
      }
    }

    if (n > 0) {
      const idx = new Set<number>();
      for (let j = 0; j <= 5; j++) idx.add(n <= 1 ? 0 : Math.round((j * (n - 1)) / 5));
      for (const i of idx) {
        const anchor = n <= 1 || (i !== 0 && i !== n - 1) ? 'middle' : i === 0 ? 'start' : 'end';
        nodes.push(txt(x(i), H - 8, labels[i] ?? '', anchor));
      }
    }
    gBase.replaceChildren(...nodes);
    geo = { W, n, x0: pl, pw, pt, py1, x, y, tops };
    if (hover >= 0 && hover < n) {
      paintHover(hover);
    } else {
      hover = -1;
      gHover.replaceChildren();
      tip.style.display = 'none';
    }
  }

  function onMove(e: PointerEvent): void {
    const g = geo;
    if (!g || g.n === 0) return;
    const r = svg.getBoundingClientRect();
    const rx = (e.clientX - r.left) * (g.W / (r.width || g.W));
    const i = g.n <= 1 ? 0 : Math.min(g.n - 1, Math.max(0, Math.round(((rx - g.x0) / g.pw) * (g.n - 1))));
    hover = i;
    paintHover(i);
    const body = o.tooltip?.(i);
    if (body === undefined) {
      tip.style.display = 'none';
      return;
    }
    tip.replaceChildren();
    if (typeof body === 'string') tip.textContent = body;
    else tip.append(body);
    tip.style.display = 'block';
    const boxW = el.clientWidth || g.W;
    const tw = tip.offsetWidth;
    const cx = e.clientX - el.getBoundingClientRect().left;
    let left = cx + 12;
    if (left + tw > boxW) left = cx - tw - 12;
    tip.style.left = `${Math.max(0, Math.min(left, boxW - tw))}px`;
  }

  function onLeave(): void {
    hover = -1;
    gHover.replaceChildren();
    tip.style.display = 'none';
  }
  root.addEventListener('pointermove', onMove);
  root.addEventListener('pointerleave', onLeave);

  function update(d: AreaData, next?: Partial<AreaOptions>): void {
    if (next) {
      o = {
        format: next.format ?? o.format,
        tooltip: next.tooltip ?? o.tooltip,
        height: next.height ?? o.height,
        ariaLabel: next.ariaLabel ?? o.ariaLabel,
      };
    }
    cancel();
    const n = d.labels.length;
    labels = d.labels;
    const prev = state;
    const fromMax = maxRaw;
    const rows = mergeKeys(prev.map((s) => s.meta), d.layers).map(({ item, gone }) => ({
      meta: item,
      gone,
      from: resample(prev.find((s) => s.meta.key === item.key)?.vals ?? [], n),
      to: gone ? zeros(n) : fit(item.values, n),
    }));
    const toMax = stackMax(rows.filter((r) => !r.gone).map((r) => r.to), n);
    cancel = animate(DURATION, (k) => {
      state = rows.map((r) => ({ meta: r.meta, vals: tweenArray(r.from, r.to, k) }));
      maxRaw = lerp(fromMax, toMax, k);
      draw();
    }, () => {
      state = rows.filter((r) => !r.gone).map((r) => ({ meta: r.meta, vals: r.to }));
      maxRaw = toMax;
      draw();
    });
  }

  const unwatch = watchWidth(el, draw);
  update(data);
  return {
    update,
    destroy(): void {
      cancel();
      unwatch();
      root.remove();
      tip.remove();
    },
  };
}

// ---- bars ----

export function bars(el: HTMLElement, items: BarItem[], opts: BarOptions): ChartHandle<BarItem[]> {
  const H = opts.height ?? 200;
  const root = document.createElement('div');
  root.className = 'chart chart-bars';
  const svg = svgEl('svg', { class: 'chart-svg', role: 'img', 'aria-label': opts.ariaLabel ?? 'Bar chart' });
  root.append(svg);
  el.append(root);
  let state: { meta: BarItem; h: number }[] = [];
  let maxRaw = 0;
  let cancel = (): void => {};

  function draw(): void {
    const W = el.clientWidth || 640;
    const pt = 22, pb = 22;
    const py1 = H - pb, ph = py1 - pt;
    const ticks = niceScale(maxRaw);
    const top = ticks[ticks.length - 1] ?? 1;
    const slot = (W - 16) / Math.max(1, state.length);
    const bw = Math.min(48, 0.6 * slot);
    const maxChars = Math.max(3, Math.floor(slot / 6.5));
    svg.setAttribute('viewBox', `0 0 ${W} ${H}`);
    Object.assign(svg.style, { width: '100%', height: `${H}px`, display: 'block' });

    const nodes: SVGElement[] = [paint(svgEl('line', { x1: 8, x2: W - 8, y1: py1, y2: py1 }), { stroke: AXIS, strokeWidth: '1px' })];
    state.forEach((s, i) => {
      const cx = 8 + slot * (i + 0.5);
      const hp = Math.max(0, (s.h / top) * ph);
      const yTop = py1 - hp;
      nodes.push(paint(svgEl('rect', { x: cx - bw / 2, y: yTop, width: bw, height: hp, rx: 2 }),
        { fill: s.meta.color ?? 'var(--chart-1)' }));
      nodes.push(txt(cx, yTop - 6, opts.format(s.h), 'middle'));
      const full = s.meta.label;
      const short = full.length > maxChars ? `${full.slice(0, maxChars - 1)}…` : full;
      const t = txt(cx, H - 6, short, 'middle');
      const title = svgEl('title');
      title.textContent = full;
      t.append(title);
      nodes.push(t);
    });
    svg.replaceChildren(...nodes);
  }

  function update(next: BarItem[]): void {
    cancel();
    const prev = state;
    const fromMax = maxRaw;
    const rows = mergeKeys(prev.map((s) => s.meta), next).map(({ item, gone }) => ({
      meta: item,
      gone,
      from: prev.find((s) => s.meta.key === item.key)?.h ?? 0,
      to: gone ? 0 : item.value,
    }));
    const toMax = Math.max(0, ...rows.map((r) => r.to));
    cancel = animate(DURATION, (k) => {
      state = rows.map((r) => ({ meta: r.meta, h: lerp(r.from, r.to, k) }));
      maxRaw = lerp(fromMax, toMax, k);
      draw();
    }, () => {
      state = rows.filter((r) => !r.gone).map((r) => ({ meta: r.meta, h: r.to }));
      maxRaw = toMax;
      draw();
    });
  }

  const unwatch = watchWidth(el, draw);
  update(items);
  return {
    update,
    destroy(): void {
      cancel();
      unwatch();
      root.remove();
    },
  };
}

// ---- sparkline ----

export function sparkline(el: HTMLElement, values: number[], opts: SparkOptions = {}): ChartHandle<number[]> {
  const H = opts.height ?? 40;
  const color = opts.color ?? 'var(--chart-1)';
  const root = document.createElement('div');
  root.className = 'chart chart-spark';
  const svg = svgEl('svg', {
    class: 'chart-svg', role: 'img', 'aria-label': opts.ariaLabel ?? 'Sparkline',
    viewBox: `0 0 100 ${H}`, preserveAspectRatio: 'none',
  });
  Object.assign(svg.style, { width: '100%', height: `${H}px`, display: 'block' });
  root.append(svg);
  el.append(root);

  const draw = (vals: number[]): void => {
    if (vals.length === 0) {
      svg.replaceChildren();
      return;
    }
    // A single value is drawn as a flat line across the full width.
    const pts = vals.length === 1 ? [vals[0] ?? 0, vals[0] ?? 0] : vals;
    const top = Math.max(0, ...pts) || 1;
    const step = 100 / (pts.length - 1);
    const yOf = (v: number): number => H - 2 - (v / top) * (H - 4);
    const line = `M${pts.map((v, i) => `${(i * step).toFixed(2)},${yOf(v).toFixed(2)}`).join('L')}`;
    svg.replaceChildren(
      paint(svgEl('path', { d: `${line}L100,${H}L0,${H}Z` }), { fill: color, fillOpacity: '0.15' }),
      paint(svgEl('path', { d: line, 'vector-effect': 'non-scaling-stroke' }), { fill: 'none', stroke: color, strokeWidth: '1.5px' }),
    );
  };

  draw(values);
  return {
    update(data: number[]): void {
      draw(data);
    },
    destroy(): void {
      root.remove();
    },
  };
}
