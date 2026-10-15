// The other Metrics figures: the latency heatmap (canvas), treemaps, gauges, lifecycle swimlanes and sparklines.

import type { ChartCtx } from './charts';
import type { GaugeItem, Heatmap, Swimlanes, Tone, TreeItem, Unit } from './model';

export interface FigureHandle {
  redraw(): void;
  destroy(): void;
}

/** Counts per time column and log-scale duration row, on a canvas; hover shows the cell, click opens what changed. */
export function heatmap(host: HTMLElement, h: Heatmap, ctx: ChartCtx): FigureHandle {
  void host;
  void h;
  void ctx;
  throw new Error('heatmap: not written yet');
}

/** A squarified-by-halves treemap; small cells drop their text and keep a title. */
export function treemap(host: HTMLElement, unit: Unit, items: TreeItem[]): FigureHandle {
  void host;
  void unit;
  void items;
  throw new Error('treemap: not written yet');
}

/** Half-circle gauges side by side. */
export function gauges(host: HTMLElement, items: GaugeItem[]): FigureHandle {
  void host;
  void items;
  throw new Error('gauges: not written yet');
}

/** One lane per VM, coloured segments by state, hover text per segment; a click on a lane name narrows to it. */
export function swimlanes(host: HTMLElement, s: Swimlanes, go: (scope: string) => void, tz: number): FigureHandle {
  void host;
  void s;
  void go;
  void tz;
  throw new Error('swimlanes: not written yet');
}

/** A small inline sparkline (SVG), filled under the line. */
export function spark(values: number[], tone: Tone, width = 90, height = 22): SVGSVGElement {
  void values;
  void tone;
  void width;
  void height;
  throw new Error('spark: not written yet');
}
