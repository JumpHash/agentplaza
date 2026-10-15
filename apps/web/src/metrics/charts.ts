// The Metrics charts: a time (or labelled) line chart with stacked areas, a percentile band, comparison overlays,
// markers, a limit line, annotations, shaded gaps, a shared crosshair, drag-to-zoom and click-for-what-changed.
// SVG for the lines and text; long series are thinned to the pixel width first.

import type { HoverBus } from './hover';
import type { Annotation, Chart } from './model';

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
  /** The data as CSV text (`time,<series…>`). */
  csv(): string;
  destroy(): void;
}

export function lineChart(host: HTMLElement, c: Chart, ctx: ChartCtx): ChartHandle {
  void host;
  void c;
  void ctx;
  throw new Error('lineChart: not written yet');
}
