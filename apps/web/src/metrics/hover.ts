// The shared crosshair: hovering one time chart moves a crosshair on every time chart of the view.
// Also the tooltip row of a chart: a series swatch and name, then its value.

import { h } from '../ui/dom';

type Listener = (frac: number | null, source: unknown) => void;

export class HoverBus {
  private listeners = new Set<Listener>();

  /** Sets the hovered fraction of the x range (null: nothing hovered), naming the chart that set it. */
  set(frac: number | null, source: unknown): void {
    for (const fn of this.listeners) fn(frac, source);
  }

  on(fn: Listener): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  clear(): void {
    this.listeners.clear();
  }
}

/**
 * One row of a chart tooltip. `color` is the series colour (a CSS colour); a ghost row (the compared scope's series)
 * draws a dashed swatch and a muted value.
 */
export function tipRow(name: string, color: string, value: string, ghost = false): HTMLElement {
  const swatch = h('i.mt-sw', { style: ghost ? `background:repeating-linear-gradient(90deg,${color} 0 3px,transparent 3px 6px)` : `background:${color}` });
  return h('div.mt-tip-r', null, h('span', null, swatch, name), h('b.num', { class: ghost ? 'mt-tip-g' : null }, value));
}
