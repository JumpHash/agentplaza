// The Metrics charts: a time (or labelled) line chart with stacked areas, a percentile band, comparison overlays,
// markers, a limit line, annotations, shaded gaps, a shared crosshair, drag-to-zoom and click-for-what-changed.
// SVG for the lines and text; long series are thinned to the pixel width first.

import { format, ticks, timeLabel } from './format';
import type { HoverBus } from './hover';
import { toneVar, type Annotation, type Chart, type Series } from './model';
import { h } from '../ui/dom';

export interface ChartCtx {
  hover: HoverBus;
  /** The view's annotations (drawn when the chart's `annotate` is set and annotations are on). */
  annotations: Annotation[];
  showAnnotations: boolean;
  /** Compact density draws charts 0.8 times as tall. */
  compact: boolean;
  /** Minutes from UTC, for time labels. */
  tz: number;
  /** A drag across a time chart: the start and end as fractions (0..1) of the chart's x range. */
  onZoom(from: number, to: number): void;
  /** A click on a point: its index and its time (null for a labelled axis). */
  onPoint(index: number, t: number | null): void;
}

export interface ChartHandle {
  /** Replaces the data (a live frame) and redraws on the next animation frame. */
  update(c: Chart): void;
  /** Redraws at the current width (after a resize or entering full screen). */
  redraw(): void;
  /** The data as a table element (the panel menu's "View as table"). */
  table(): HTMLElement;
  /** The data as CSV text (`time,<series…>`), oldest point first. */
  csv(): string;
  destroy(): void;
}

const NS = 'http://www.w3.org/2000/svg';
/** Room for the y labels: "1.4 MiB/s" and "$1,264" fit at 11 px. The desktop uses the same gutter. */
const PAD_L = 64;
const PAD_R = 10;
const PAD_T = 8;
const PAD_B = 20;

/** What a draw keeps for the crosshair: the scales and the live elements of the current SVG. */
interface Geometry {
  X: (i: number) => number;
  Y: (v: number) => number;
  n: number;
  root: SVGElement;
  xh: SVGElement;
  dots: SVGElement;
}

/** The first point of each pixel bucket when `n` points are drawn across `width` pixels (one bucket per point when they fit). */
export function bucketStarts(n: number, width: number): number[] {
  if (n <= 0) return [];
  const m = Math.max(1, Math.min(n, Math.floor(width)));
  return Array.from({ length: m }, (_, j) => Math.floor((j * n) / m));
}

/** One point per bucket: the largest value of each bucket (the smallest when `keep` is 'min'), null when the bucket has no value. */
export function thin(values: readonly (number | null | undefined)[], starts: readonly number[], keep: 'max' | 'min' = 'max'): (number | null)[] {
  return starts.map((from, j) => {
    const to = starts[j + 1] ?? values.length;
    let best: number | null = null;
    for (let i = from; i < to; i++) {
      const v = values[i];
      if (v == null) continue;
      if (best == null || (keep === 'max' ? v > best : v < best)) best = v;
    }
    return best;
  });
}

/** The outlines of stacked layers: each layer's top edge forward and the layer below's top edge back, so neighbours
 *  share one edge (no seam, no wedge). A gap counts as zero. */
export function stackedPaths(
  layers: readonly (readonly (number | null)[])[],
  x: (j: number) => number,
  y: (v: number) => number,
): string[] {
  let base: number[] = (layers[0] ?? []).map(() => 0);
  return layers.map((vals) => {
    const top = vals.map((v, j) => (base[j] ?? 0) + (v ?? 0));
    const d = bandPath(top, base, x, y);
    base = top;
    return d;
  });
}

/** The outline of a percentile band: the high edge left to right, then the low edge right to left (each point with
 * its own value), closed. A gap counts as 0, as on the desktop. */
export function bandPath(
  hi: readonly (number | null)[],
  lo: readonly (number | null)[],
  x: (j: number) => number,
  y: (v: number) => number,
): string {
  const top = hi.map((v, j) => `${j ? 'L' : 'M'}${x(j).toFixed(1)},${y(v ?? 0).toFixed(1)}`).join('');
  const bottom = lo
    .map((_, q) => {
      const j = lo.length - 1 - q;
      return `L${x(j).toFixed(1)},${y(lo[j] ?? 0).toFixed(1)}`;
    })
    .join('');
  return top + bottom + 'Z';
}

/** The indexes of the x-axis labels: the first, the last and evenly spaced between them (three labels under 460 px, five from there). */
export function xLabelIndexes(n: number, width: number): number[] {
  if (n <= 0) return [];
  const steps = width < 460 ? 2 : 4;
  const out: number[] = [];
  for (let k = 0; k <= steps; k++) {
    const i = Math.round((k / steps) * (n - 1));
    if (out[out.length - 1] !== i) out.push(i);
  }
  return out;
}

/** The number of points a chart draws: the longest series, or the label count of a labelled axis. */
function pointCount(c: Chart): number {
  let n = c.x.type === 'labels' ? c.x.labels.length : 0;
  for (const s of [...c.series, ...c.ghosts]) n = Math.max(n, s.values.length);
  if (c.band) n = Math.max(n, c.band.hi.length, c.band.lo.length);
  return n;
}

const clamp01 = (t: number): number => Math.min(1, Math.max(0, t));

/** The largest value in a list (0 when there is none). */
function peak(vals: readonly (number | null | undefined)[]): number {
  let m = 0;
  for (const v of vals) if (v != null && v > m) m = v;
  return m;
}

/** The runs of nulls in a list, as [from, to) index pairs. */
function nullRuns(vals: readonly (number | null)[]): [number, number][] {
  const runs: [number, number][] = [];
  let i = 0;
  while (i < vals.length) {
    if (vals[i] != null) {
      i++;
      continue;
    }
    const from = i;
    while (i < vals.length && vals[i] == null) i++;
    runs.push([from, i]);
  }
  return runs;
}

/** A polyline through `vals` at the x positions `xs`; a null breaks the line. */
function linePath(vals: readonly (number | null)[], xs: readonly number[], Y: (v: number) => number): string {
  let d = '';
  let pen = false;
  vals.forEach((v, j) => {
    if (v == null) {
      pen = false;
      return;
    }
    d += (pen ? 'L' : 'M') + (xs[j] ?? 0).toFixed(1) + ',' + Y(v).toFixed(1);
    pen = true;
  });
  return d;
}

/** An SVG element. Colours go through the style, so that the CSS variables resolve. */
function svgNode(tag: string, attrs: Record<string, string | number | null | undefined> = {}, css: Record<string, string> = {}): SVGElement {
  const el = document.createElementNS(NS, tag) as SVGElement;
  for (const [key, value] of Object.entries(attrs)) if (value != null) el.setAttribute(key, String(value));
  for (const [key, value] of Object.entries(css)) el.style.setProperty(key, value);
  return el;
}

function svgText(x: number, y: number, text: string, anchor: 'start' | 'middle' | 'end' = 'start', css: Record<string, string> = {}): SVGElement {
  const el = svgNode('text', { x, y, 'text-anchor': anchor }, css);
  el.textContent = text;
  return el;
}

function svgTitle(text: string): SVGElement {
  const el = svgNode('title');
  el.textContent = text;
  return el;
}

/** A CSV field, quoted only when it has a comma, a quote or a line break. */
function csvCell(s: string): string {
  return /[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

function chartLabel(c: Chart): string {
  const names = c.series.map((s) => s.name);
  return names.length ? `Line chart of ${names.join(', ')}` : 'Line chart';
}

export function lineChart(host: HTMLElement, c: Chart, ctx: ChartCtx): ChartHandle {
  let current = c;
  let destroyed = false;
  let frame: number | null = null;
  let geo: Geometry | null = null;

  const box = h('div.mx-chart');
  const legend = h('div.mx-legend');
  const tip = h('div.mx-tip', { style: 'display:none' });
  const svgWrap = h('div');
  box.append(legend, tip, svgWrap);
  host.appendChild(box);

  // One hint under the time charts of a host: the first time chart adds it.
  let hint: HTMLElement | null = null;
  if (c.x.type === 'time' && !host.querySelector('.mx-hint')) {
    hint = h('div.mx-hint', null, 'drag to zoom · click a point for what changed');
    host.appendChild(hint);
  }

  const unsubscribe = ctx.hover.on((frac, source) => cross(frac, source));

  /** The label of point `i` on the x axis. */
  function xText(i: number): string {
    const ax = current.x;
    return ax.type === 'time' ? timeLabel(ax.start_ms + i * ax.step_ms, ax.step_ms, ctx.tz) : (ax.labels[i] ?? '');
  }

  /** The time of point `i`, or null on a labelled axis. */
  function timeOf(i: number): number | null {
    const ax = current.x;
    return ax.type === 'time' ? ax.start_ms + i * ax.step_ms : null;
  }

  function legendItem(s: Series): HTMLElement {
    const colour = toneVar(s.tone);
    const swatch = h('i', { class: s.dash ? 'mx-dash' : null, style: `background:${colour};border-color:${colour}` });
    const scope = s.scope;
    if (!scope) return h('span', null, swatch, s.name);
    // A series with a scope navigates: the view listens for this event on the host.
    return h(
      'button.mx-scope',
      { type: 'button', onclick: () => host.dispatchEvent(new CustomEvent<string>('mx-scope', { detail: scope, bubbles: true })) },
      swatch,
      s.name,
    );
  }

  function paintLegend(): void {
    const cur = current;
    const items: Node[] = cur.series.map((s) => legendItem(s));
    for (const g of cur.ghosts) {
      items.push(h('span.mx-mute', null, h('i.mx-dash', { style: `border-color:${toneVar(g.tone)}` }), g.name));
    }
    if (cur.band) {
      items.push(h('span', null, h('i', { style: `background:${toneVar(cur.band.tone)};opacity:.3;height:8px` }), cur.band.name));
    }
    legend.replaceChildren(...items);
  }

  function draw(): void {
    const cur = current;
    const W = Math.max(240, svgWrap.clientWidth || host.clientWidth || 600);
    const H = Math.round(cur.height * (ctx.compact ? 0.8 : 1));
    const iw = W - PAD_L - PAD_R;
    const ih = H - PAD_T - PAD_B;
    const n = pointCount(cur);
    const X = (i: number): number => PAD_L + (n <= 1 ? 0 : (i / (n - 1)) * iw);

    // Long series are thinned to one point per pixel bucket; every drawn list shares these starts.
    const starts = bucketStarts(n, iw);
    const xs = starts.map((i) => X(i));
    const xAt = (j: number): number => xs[j] ?? 0;
    const series = cur.series.map((s) => ({ s, vals: thin(s.values, starts) }));
    const ghosts = cur.ghosts.map((g) => ({ g, vals: thin(g.values, starts) }));
    const bandSpec = cur.band;
    const band = bandSpec ? { tone: bandSpec.tone, lo: thin(bandSpec.lo, starts, 'min'), hi: thin(bandSpec.hi, starts) } : null;
    const totals = starts.map((_, j) => series.reduce((sum, { vals }) => sum + (vals[j] ?? 0), 0));

    let max = cur.stacked ? peak(totals) : Math.max(0, ...series.map(({ vals }) => peak(vals)));
    for (const { vals } of ghosts) max = Math.max(max, peak(vals));
    if (band) max = Math.max(max, peak(band.hi));
    // A chart with a ceiling (shares, percentages) ends its axis exactly there while the data fits under it.
    // (A stacked total of shares can land a rounding error above its ceiling.)
    const capped = cur.y_max != null && max <= cur.y_max * 1.001 && !(cur.limit && cur.limit.value > cur.y_max);
    if (cur.y_max != null) max = capped ? cur.y_max : Math.max(max, cur.y_max);
    if (cur.limit) max = Math.max(max, cur.limit.value);
    const tk = capped ? [0, max / 4, max / 2, (max * 3) / 4, max] : ticks(max);
    const ym = tk[4] ?? 1;
    const Y = (v: number): number => PAD_T + ih - (v / ym) * ih;

    const root = svgNode('svg', { width: W, height: H, role: 'img', 'aria-label': chartLabel(cur) });

    // Grid and axes.
    for (const v of tk) {
      const y = Y(v);
      root.append(svgNode('line', { x1: PAD_L, x2: W - PAD_R, y1: y, y2: y }, { stroke: 'var(--chart-grid)' }), svgText(PAD_L - 6, y + 3, format(cur.unit, v), 'end'));
    }
    for (const i of xLabelIndexes(n, W)) {
      root.append(svgText(X(i), H - 4, xText(i), i === 0 ? 'start' : i === n - 1 ? 'end' : 'middle'));
    }

    // Gaps: a run of nulls in a line is shaded and named.
    if (!cur.stacked) {
      for (const { s, vals } of series) {
        for (const [from, to] of nullRuns(vals)) {
          const x0 = xAt(Math.max(0, from - 1));
          const x1 = xAt(Math.min(starts.length - 1, to));
          const reason = s.gap_reason ?? 'no data';
          const rect = svgNode('rect', { x: x0, y: PAD_T, width: Math.max(2, x1 - x0), height: ih, opacity: 0.7 }, { fill: 'var(--rule-faint)' });
          rect.append(svgTitle(`${s.name}: ${reason}`));
          root.append(rect, svgText((x0 + x1) / 2, PAD_T + 12, reason, 'middle'));
        }
      }
    }

    // The percentile band: the high edge forward, the low edge back.
    if (band && starts.length) {
      root.append(svgNode('path', { d: bandPath(band.hi, band.lo, xAt, Y), opacity: 0.18 }, { fill: toneVar(band.tone) }));
    }

    if (cur.stacked) {
      const outlines = stackedPaths(series.map(({ vals }) => vals), xAt, Y);
      series.forEach(({ s }, k) => root.append(svgNode('path', { d: outlines[k] ?? '', opacity: 0.78 }, { fill: toneVar(s.tone) })));
    } else {
      for (const { s, vals } of series) {
        const d = linePath(vals, xs, Y);
        if (cur.area && starts.length && !vals.includes(null)) {
          const floor = Y(0).toFixed(1);
          root.append(svgNode('path', { d: `${d}L${xAt(starts.length - 1).toFixed(1)},${floor}L${xAt(0).toFixed(1)},${floor}Z`, opacity: 0.12 }, { fill: toneVar(s.tone) }));
        }
        root.append(
          svgNode('path', { d, fill: 'none', 'stroke-width': s.dash ? 1.2 : 1.6, 'stroke-dasharray': s.dash ? '4 3' : null }, { stroke: toneVar(s.tone) }),
        );
      }
    }

    for (const { g, vals } of ghosts) {
      root.append(svgNode('path', { d: linePath(vals, xs, Y), fill: 'none', 'stroke-width': 1.2, 'stroke-dasharray': '5 4', opacity: 0.75 }, { stroke: toneVar(g.tone) }));
    }

    // Markers and the limit line.
    for (const mk of cur.markers) {
      if (mk.index < 0 || mk.index >= n) continue;
      const x = X(mk.index);
      root.append(
        svgNode('line', { x1: x, x2: x, y1: PAD_T, y2: PAD_T + ih, 'stroke-dasharray': '2 3' }, { stroke: toneVar('signal') }),
        svgText(x + 3, PAD_T + 9, mk.label, 'start', { fill: toneVar('signal') }),
      );
    }
    if (cur.limit) {
      const y = Y(cur.limit.value);
      root.append(
        svgNode('line', { x1: PAD_L, x2: W - PAD_R, y1: y, y2: y, 'stroke-dasharray': '5 3' }, { stroke: toneVar('crit') }),
        // Above the line, or under it when the line runs along the top (as the desktop places it).
        svgText(W - PAD_R, y - 16 < 0 ? y + 13 : y - 4, cur.limit.label, 'end', { fill: toneVar('crit') }),
      );
    }

    // Annotations: a dotted line and a triangle on the axis, on a time axis only.
    const ax = cur.x;
    if (ax.type === 'time' && ax.step_ms > 0 && cur.annotate && ctx.showAnnotations) {
      for (const a of ctx.annotations) {
        const i = (a.t_ms - ax.start_ms) / ax.step_ms;
        if (!(i >= 0 && i <= n - 1)) continue;
        const x = X(i);
        const y = PAD_T + ih;
        const group = svgNode('g', { class: 'mx-ann' });
        group.append(
          svgNode('line', { x1: x, x2: x, y1: PAD_T, y2: y, 'stroke-dasharray': '1 3' }, { stroke: 'var(--ink-3)' }),
          svgNode('path', { d: `M${x - 5},${y} L${x + 5},${y} L${x},${y - 7}Z` }, { fill: 'var(--signal)' }),
          svgTitle(a.label),
        );
        root.append(group);
      }
    }

    // The crosshair, its dots and the drag brush; the view moves them through the hover bus.
    const xh = svgNode('line', { x1: 0, x2: 0, y1: PAD_T, y2: PAD_T + ih, visibility: 'hidden' }, { stroke: 'var(--ink-3)' });
    const dots = svgNode('g');
    const brush = svgNode('rect', { x: 0, y: PAD_T, width: 0, height: ih, opacity: 0.12, visibility: 'hidden' }, { fill: 'var(--signal)' });
    root.append(xh, dots, brush);
    svgWrap.replaceChildren(root);
    geo = { X, Y, n, root, xh, dots };

    let drag: number | null = null;
    const px = (e: MouseEvent): number => e.clientX - root.getBoundingClientRect().left;
    root.addEventListener('mousedown', (e) => {
      if (e.button !== 0) return;
      drag = px(e);
      e.preventDefault();
    });
    root.addEventListener('mousemove', (e) => {
      const x = px(e);
      const f = (x - PAD_L) / iw;
      if (f >= 0 && f <= 1) ctx.hover.set(f, box);
      if (drag != null && ax.type === 'time') {
        const a = Math.max(PAD_L, Math.min(drag, x));
        const b = Math.min(PAD_L + iw, Math.max(drag, x));
        brush.setAttribute('x', String(a));
        brush.setAttribute('width', String(Math.max(0, b - a)));
        brush.setAttribute('visibility', 'visible');
      }
    });
    root.addEventListener('mouseleave', () => {
      ctx.hover.set(null, box);
      drag = null;
      brush.setAttribute('visibility', 'hidden');
    });
    root.addEventListener('mouseup', (e) => {
      if (drag == null) return;
      const x0 = drag;
      const x1 = px(e);
      drag = null;
      brush.setAttribute('visibility', 'hidden');
      if (Math.abs(x1 - x0) < 6) {
        if (n === 0) return;
        const i = Math.round(clamp01((x1 - PAD_L) / iw) * (n - 1));
        ctx.onPoint(i, timeOf(i));
        return;
      }
      if (ax.type !== 'time') return;
      ctx.onZoom(clamp01((Math.min(x0, x1) - PAD_L) / iw), clamp01((Math.max(x0, x1) - PAD_L) / iw));
    });
  }

  /** Moves the crosshair to `frac` of the x range; the chart the mouse is over (`source` is its box) also shows its tip. */
  function cross(frac: number | null, source: unknown): void {
    const g = geo;
    if (!g || !g.root.isConnected) return;
    if (frac == null || g.n === 0) {
      g.xh.setAttribute('visibility', 'hidden');
      g.dots.replaceChildren();
      tip.style.display = 'none';
      return;
    }
    const cur = current;
    const i = Math.round(frac * (g.n - 1));
    const x = g.X(i);
    g.xh.setAttribute('x1', String(x));
    g.xh.setAttribute('x2', String(x));
    g.xh.setAttribute('visibility', 'visible');
    let acc = 0;
    const dotEls: SVGElement[] = [];
    for (const s of cur.series) {
      const v = s.values[i] ?? null;
      acc += v ?? 0;
      if (v != null) dotEls.push(svgNode('circle', { cx: x, cy: g.Y(cur.stacked ? acc : v), r: 2.8 }, { fill: toneVar(s.tone) }));
    }
    g.dots.replaceChildren(...dotEls);

    if (source !== box) {
      tip.style.display = 'none';
      return;
    }
    const val = (v: number | null | undefined): string => (v == null ? '—' : format(cur.unit, v));
    const rows: Node[] = [h('div.mx-t', null, xText(i))];
    for (const s of cur.series) {
      rows.push(h('div', null, h('i', { style: `background:${toneVar(s.tone)}` }), ` ${s.name} `, h('b.mx-num', null, val(s.values[i]))));
    }
    for (const gh of cur.ghosts) {
      rows.push(h('div.mx-mute', null, `┄ ${gh.name} `, h('span.mx-num', null, val(gh.values[i]))));
    }
    if (cur.band) rows.push(h('div.mx-mute', null, `${cur.band.name}: ${val(cur.band.lo[i])} – ${val(cur.band.hi[i])}`));
    tip.replaceChildren(...rows);
    tip.style.display = 'block';
    const left = x + 14 + 200 > box.clientWidth ? x - 210 : x + 14;
    tip.style.left = `${Math.max(0, left)}px`;
    tip.style.top = '28px';
  }

  /** Draws on the next animation frame; several updates in one frame draw once. */
  function scheduleDraw(): void {
    if (destroyed || frame != null) return;
    frame = requestAnimationFrame(() => {
      frame = null;
      draw();
    });
  }

  paintLegend();
  scheduleDraw();

  return {
    update(next: Chart): void {
      if (destroyed) return;
      current = next;
      paintLegend();
      scheduleDraw();
    },
    redraw(): void {
      if (destroyed) return;
      if (frame != null) {
        cancelAnimationFrame(frame);
        frame = null;
      }
      draw();
    },
    table(): HTMLElement {
      const cur = current;
      const n = pointCount(cur);
      const cols = [...cur.series, ...cur.ghosts];
      const head = h('tr', null, h('th', null, cur.x.type === 'time' ? 'Time' : 'Label'), ...cols.map((s) => h('th', null, s.name)));
      const rows: HTMLElement[] = [];
      for (let i = n - 1; i >= 0; i--) {
        const cells = cols.map((s, k) => {
          const v = s.values[i];
          return h('td', { class: k >= cur.series.length ? 'mx-mute mx-num' : 'mx-num' }, v == null ? '—' : format(cur.unit, v));
        });
        rows.push(h('tr', null, h('td', null, xText(i)), ...cells));
      }
      return h('table.mx-t', null, h('thead', null, head), h('tbody', null, ...rows));
    },
    csv(): string {
      const cur = current;
      const n = pointCount(cur);
      const lines = [['time', ...cur.series.map((s) => s.name)].map(csvCell).join(',')];
      for (let i = 0; i < n; i++) {
        const when = cur.x.type === 'time' ? new Date(cur.x.start_ms + i * cur.x.step_ms).toISOString() : (cur.x.labels[i] ?? '');
        const values = cur.series.map((s) => {
          const v = s.values[i];
          return v == null ? '' : String(v);
        });
        lines.push([csvCell(when), ...values].join(','));
      }
      return lines.join('\n');
    },
    destroy(): void {
      destroyed = true;
      if (frame != null) cancelAnimationFrame(frame);
      frame = null;
      unsubscribe();
      box.remove();
      hint?.remove();
      geo = null;
    },
  };
}
