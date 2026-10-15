// The other Metrics figures: the latency heatmap (canvas), treemaps, gauges, lifecycle swimlanes and sparklines.

import type { ChartCtx } from './charts';
import { format, timeLabel } from './format';
import type { GaugeItem, Heatmap, Swimlanes, Tone, TreeItem, Unit } from './model';
import { statusVar, toneVar } from './model';
import { h as el } from '../ui/dom';

export interface FigureHandle {
  redraw(): void;
  destroy(): void;
}

/** A treemap cell in percent of the box: its origin and size, 0 to 100 each. */
export interface TreeCell {
  item: TreeItem;
  x: number;
  y: number;
  w: number;
  h: number;
}

const SVG_NS = 'http://www.w3.org/2000/svg';
const TREE_H = 210;
const TREE_TEXT_MIN_AREA = 2000;
const LANE_W = 70;
const LANE_H = 22;
const AXIS_H = 18;
const SWIM_MIN_W = 560;
const LANE_TEXT = 'font:400 11px var(--mono);fill:var(--ink-2)';
const AXIS_TEXT = 'font:400 10px var(--sans);fill:var(--ink-3)';

/** A handle whose redraw re-renders the figure, and whose destroy empties its host (nothing draws after that). */
function figureHandle(host: HTMLElement, redraw: () => void): FigureHandle {
  let gone = false;
  return {
    redraw: () => {
      if (!gone) redraw();
    },
    destroy: () => {
      gone = true;
      host.replaceChildren();
    },
  };
}

/** An SVG element with its attributes set as given. Colours go in `style`, where a token's var() resolves. */
function svgEl(tag: string, attrs: Record<string, string | number> = {}): SVGElement {
  const node = document.createElementNS(SVG_NS, tag);
  for (const [key, value] of Object.entries(attrs)) node.setAttribute(key, String(value));
  return node;
}

/** Counts per time column and log-scale duration row, on a canvas; hover shows the cell, click opens what changed. */
export function heatmap(host: HTMLElement, h: Heatmap, ctx: ChartCtx): FigureHandle {
  const wrap = el('div.mx-hm-wrap', { style: 'position:relative' });
  const cv = el('canvas.mx-hm', { role: 'img', 'aria-label': 'Latency heatmap: calls per time and duration bucket' });
  const tip = el('div.mx-tip', { style: 'position:absolute;display:none;pointer-events:none' });
  wrap.append(cv, tip);
  host.appendChild(wrap);

  const cols = h.counts.length;
  const rows = h.counts[0]?.length ?? 0;
  const height = Math.round(h.height * (ctx.compact ? 0.8 : 1));
  const padL = 50;
  const padB = 18;
  let layout = { width: 0, cw: 0, rh: 0 };

  const timeAt = (c: number): number => h.start_ms + c * h.step_ms;
  /** The duration at the edge of a row; the rows are log-spaced from lo to hi (linear when that is not possible). */
  const rowEdge = (r: number): number => (h.lo > 0 && h.hi > h.lo ? h.lo * Math.pow(h.hi / h.lo, r / rows) : h.lo + ((h.hi - h.lo) * r) / rows);
  const bucketRange = (r: number): string => `${format(h.unit, rowEdge(r))}–${format(h.unit, rowEdge(r + 1))}`;

  function cellAt(e: MouseEvent): { c: number; r: number } | null {
    if (!cols || !rows) return null;
    const box = cv.getBoundingClientRect();
    const x = e.clientX - box.left - padL;
    const y = e.clientY - box.top;
    const c = Math.floor(x / layout.cw);
    const r = rows - 1 - Math.floor(y / layout.rh);
    if (c < 0 || c >= cols || r < 0 || r >= rows) return null;
    return { c, r };
  }

  function draw(): void {
    const width = Math.max(240, wrap.clientWidth);
    const dpr = window.devicePixelRatio || 1;
    cv.width = Math.round(width * dpr);
    cv.height = Math.round(height * dpr);
    cv.style.width = `${width}px`;
    cv.style.height = `${height}px`;
    layout = { width, cw: (width - padL) / Math.max(1, cols), rh: (height - padB) / Math.max(1, rows) };
    const g = cv.getContext('2d');
    if (!g || !cols || !rows) return;
    g.scale(dpr, dpr);

    const css = getComputedStyle(document.documentElement);
    const cold = css.getPropertyValue('--chart-1').trim();
    const hot = css.getPropertyValue('--chart-3').trim();
    const axis = css.getPropertyValue('--chart-axis').trim();
    let max = 0;
    for (const column of h.counts) for (const v of column) max = Math.max(max, v);
    const top = max > 0 ? max : 1;

    for (let c = 0; c < cols; c++) {
      for (let r = 0; r < rows; r++) {
        const v = (h.counts[c]?.[r] ?? 0) / top;
        if (v < 0.02) continue;
        g.fillStyle = v > 0.6 ? hot : cold;
        g.globalAlpha = Math.min(1, 0.12 + v * 0.95);
        g.fillRect(padL + c * layout.cw, (rows - 1 - r) * layout.rh, Math.ceil(layout.cw), Math.ceil(layout.rh));
      }
    }

    g.globalAlpha = 1;
    g.fillStyle = axis;
    g.font = '10px system-ui';
    g.textAlign = 'right';
    const yLabels = [h.lo, rowEdge(rows / 2), h.hi];
    yLabels.forEach((v, k) => {
      const r = Math.round((k / (yLabels.length - 1)) * (rows - 1));
      g.fillText(format(h.unit, v), padL - 6, (rows - 1 - r) * layout.rh + layout.rh / 2 + 3);
    });
    const nt = width < 460 ? 2 : 4;
    for (let k = 0; k <= nt; k++) {
      const c = Math.round((k / nt) * (cols - 1));
      g.textAlign = k === 0 ? 'left' : k === nt ? 'right' : 'center';
      g.fillText(timeLabel(timeAt(c), h.step_ms, ctx.tz), padL + c * layout.cw + (k === nt ? layout.cw : 0), height - 4);
    }
  }

  cv.addEventListener('mousemove', (e) => {
    const at = cellAt(e);
    if (!at) {
      tip.style.display = 'none';
      return;
    }
    const box = cv.getBoundingClientRect();
    const n = h.counts[at.c]?.[at.r] ?? 0;
    tip.replaceChildren(`${timeLabel(timeAt(at.c), h.step_ms, ctx.tz)} · ${bucketRange(at.r)}: `, el('b.num', null, String(Math.round(n))), ' calls');
    tip.style.display = 'block';
    tip.style.left = `${Math.min(e.clientX - box.left + 12, layout.width - 170)}px`;
    tip.style.top = `${e.clientY - box.top + 10}px`;
  });
  cv.addEventListener('mouseleave', () => {
    tip.style.display = 'none';
  });
  cv.addEventListener('click', (e) => {
    const at = cellAt(e);
    if (at) ctx.onPoint(at.c, timeAt(at.c));
  });

  draw();
  return figureHandle(host, draw);
}

/**
 * Splits the items into cells that tile a 100 by 100 box (percent). The items are sorted by value, largest first;
 * each step cuts the box in two halves by value, along its longer side. Every item keeps a cell, and a cell's area
 * is its share of the total.
 */
export function splitTreemap(items: TreeItem[]): TreeCell[] {
  const weight = (it: TreeItem): number => (Number.isFinite(it.value) && it.value > 0 ? it.value : 0);
  const cells: TreeCell[] = [];
  const place = (list: TreeItem[], x: number, y: number, w: number, h: number): void => {
    if (list.length === 0) return;
    const [only] = list;
    if (list.length === 1 && only) {
      cells.push({ item: only, x, y, w, h });
      return;
    }
    const sum = list.reduce((s, it) => s + weight(it), 0);
    let k = 0;
    let f: number;
    if (sum > 0) {
      let acc = 0;
      while (k < list.length - 1 && acc + weight(list[k] as TreeItem) <= sum / 2) {
        acc += weight(list[k] as TreeItem);
        k++;
      }
      if (k === 0) {
        acc = weight(list[0] as TreeItem);
        k = 1;
      }
      f = acc / sum;
    } else {
      // Nothing has weight: halve the cells by count so they still tile the box.
      k = Math.ceil(list.length / 2);
      f = k / list.length;
    }
    if (w >= h) {
      place(list.slice(0, k), x, y, w * f, h);
      place(list.slice(k), x + w * f, y, w * (1 - f), h);
    } else {
      place(list.slice(0, k), x, y, w, h * f);
      place(list.slice(k), x, y + h * f, w, h * (1 - f));
    }
  };
  place([...items].sort((a, b) => weight(b) - weight(a)), 0, 0, 100, 100);
  return cells;
}

/** A squarified-by-halves treemap; small cells drop their text and keep a title. */
export function treemap(host: HTMLElement, unit: Unit, items: TreeItem[]): FigureHandle {
  const box = el('div.mx-treemap', { style: `position:relative;height:${TREE_H}px` });
  host.appendChild(box);
  const total = items.reduce((s, it) => s + Math.max(0, it.value), 0);

  const render = (): void => {
    const width = box.clientWidth || 600;
    box.replaceChildren(
      ...splitTreemap(items).map((c) => {
        const d = document.createElement('div');
        Object.assign(d.style, {
          position: 'absolute',
          left: `${c.x}%`,
          top: `${c.y}%`,
          width: `${c.w}%`,
          height: `${c.h}%`,
          background: toneVar(c.item.tone),
        });
        const share = total > 0 ? (Math.max(0, c.item.value) / total) * 100 : 0;
        const pct = `${share.toFixed(0)}%`;
        if ((c.w / 100) * width * ((c.h / 100) * TREE_H) > TREE_TEXT_MIN_AREA) {
          d.replaceChildren(el('b', null, c.item.name), el('br'), `${format(unit, c.item.value)} · ${pct}`);
        }
        d.title = `${c.item.name}: ${format(unit, c.item.value)} (${pct})`;
        return d;
      }),
    );
  };

  render();
  return figureHandle(host, render);
}

/** One half-circle gauge: the arc fills to `frac`, coloured by its status, with the label in the arc and the sub text under. */
function gaugeCell(g: GaugeItem): HTMLElement {
  const frac = Number.isFinite(g.frac) ? Math.min(1, Math.max(0, g.frac)) : 0;
  const r = 52;
  const cx = 64;
  const cy = 62;
  const a0 = Math.PI;
  const a1 = Math.PI + Math.PI * frac;
  const p = (a: number): string => `${(cx + r * Math.cos(a)).toFixed(2)},${(cy + r * Math.sin(a)).toFixed(2)}`;
  const svg = svgEl('svg', {
    width: 128,
    height: 78,
    role: 'img',
    'aria-label': `${g.label} ${g.sub}`,
    style: 'display:block;margin:0 auto',
  });
  svg.appendChild(svgEl('path', { d: `M${p(a0)} A${r},${r} 0 0 1 ${p(2 * Math.PI)}`, fill: 'none', 'stroke-width': 10, style: 'stroke:var(--rule-faint)' }));
  svg.appendChild(svgEl('path', { d: `M${p(a0)} A${r},${r} 0 0 1 ${p(a1)}`, fill: 'none', 'stroke-width': 10, style: `stroke:${statusVar(g.status)}` }));
  const label = svgEl('text', { x: cx, y: cy - 6, 'text-anchor': 'middle', style: 'font:600 17px var(--sans);fill:var(--ink)' });
  label.textContent = g.label;
  svg.appendChild(label);
  return el('div', null, svg, el('div', { style: 'text-align:center;font-size:11.5px;color:var(--ink-3)' }, g.sub));
}

/** Half-circle gauges side by side. */
export function gauges(host: HTMLElement, items: GaugeItem[]): FigureHandle {
  const box = el('div.mx-gauges', { style: 'display:grid;grid-template-columns:repeat(auto-fill,minmax(128px,1fr));gap:8px' });
  host.appendChild(box);
  const render = (): void => {
    box.replaceChildren(...items.map(gaugeCell));
  };
  render();
  return figureHandle(host, render);
}

/** One lane per VM, coloured segments by state, hover text per segment; a click on a lane name narrows to it. */
export function swimlanes(host: HTMLElement, s: Swimlanes, go: (scope: string) => void, tz: number): FigureHandle {
  const scroll = el('div.mx-swim', { style: 'overflow-x:auto' });
  const legend = el('div.mx-legend', { style: 'margin-top:8px' });
  host.append(scroll, legend);
  const span = Math.max(1, s.end_ms - s.start_ms);

  const render = (): void => {
    // The chart is drawn at its real pixel width, so its text keeps its size; under 560 px the wrapper scrolls.
    const width = Math.max(SWIM_MIN_W, scroll.clientWidth);
    const plot = width - LANE_W - 4;
    const height = s.lanes.length * LANE_H + AXIS_H;
    const x = (t: number): number => LANE_W + Math.min(1, Math.max(0, (t - s.start_ms) / span)) * plot;
    const svg = svgEl('svg', { width, height, role: 'img', 'aria-label': 'VM timeline' });

    s.lanes.forEach((lane, row) => {
      const y = row * LANE_H;
      const name = svgEl('text', { x: 0, y: y + 14, style: LANE_TEXT });
      name.textContent = lane.name;
      if (lane.scope) {
        const scope = lane.scope;
        name.setAttribute('style', `${LANE_TEXT};cursor:pointer`);
        name.setAttribute('role', 'button');
        name.setAttribute('tabindex', '0');
        name.addEventListener('click', () => go(scope));
        name.addEventListener('keydown', (e) => {
          if (e.key === 'Enter') go(scope);
        });
      }
      svg.appendChild(name);
      for (const seg of lane.segs) {
        const x0 = x(seg.start_ms);
        const x1 = x(seg.end_ms);
        if (x1 <= x0) continue;
        const rect = svgEl('rect', { x: x0, y: y + 3, width: x1 - x0, height: 15, style: `fill:${toneVar(seg.tone)}` });
        const title = svgEl('title');
        title.textContent = `${lane.name} · ${seg.state}${seg.detail ? ` · ${seg.detail}` : ''}`;
        rect.appendChild(title);
        svg.appendChild(rect);
      }
    });

    const axisY = s.lanes.length * LANE_H + 13;
    for (let k = 0; k <= 4; k++) {
      const label = svgEl('text', {
        x: LANE_W + (plot * k) / 4,
        y: axisY,
        'text-anchor': k === 0 ? 'start' : k === 4 ? 'end' : 'middle',
        style: AXIS_TEXT,
      });
      label.textContent = timeLabel(s.start_ms + (span * k) / 4, span / 4, tz);
      svg.appendChild(label);
    }

    scroll.replaceChildren(svg);
    legend.replaceChildren(...s.legend.map((item) => el('span', null, el('i', { style: `background:${toneVar(item.tone)};height:8px` }), item.label)));
  };

  render();
  return figureHandle(host, render);
}

/** A small inline sparkline (SVG), filled under the line. */
export function spark(values: number[], tone: Tone, width = 90, height = 22): SVGSVGElement {
  const svg = svgEl('svg', { width, height, 'aria-hidden': 'true', style: 'display:inline-block;vertical-align:middle' }) as SVGSVGElement;
  const n = values.length;
  if (n === 0) return svg;
  const lo = values.reduce((m, v) => Math.min(m, v), Infinity);
  const hi = values.reduce((m, v) => Math.max(m, v), -Infinity);
  const range = hi - lo;
  const pts = values.map((v, i) => {
    const x = n > 1 ? (i / (n - 1)) * width : width / 2;
    // A flat series sits in the middle of the box.
    const y = range > 0 ? height - 2 - ((v - lo) / range) * (height - 4) : height / 2;
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  });
  const color = toneVar(tone);
  svg.appendChild(svgEl('path', { d: `M0,${height} L${pts.join(' L')} L${width},${height}Z`, style: `fill:${color};opacity:0.15` }));
  svg.appendChild(svgEl('polyline', { points: pts.join(' '), fill: 'none', 'stroke-width': 1.3, style: `stroke:${color}` }));
  return svg;
}
