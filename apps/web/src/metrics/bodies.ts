// The list-like panel bodies: tables, bars, splits, stats and flags. panels.ts calls these; the alerting and tracing
// bodies are in bodies-ops.ts, charts and figures in charts.ts and figures.ts.

import type { PanelCtx } from './panels';
import type { BarRow, Flag, LegendItem, SplitRow, StatLine, Table } from './model';

export function table(host: HTMLElement, t: Table, ctx: PanelCtx): void {
  void host;
  void t;
  void ctx;
  throw new Error('table: not written yet');
}

export function flags(host: HTMLElement, list: Flag[], ctx: PanelCtx): void {
  void host;
  void list;
  void ctx;
  throw new Error('flags: not written yet');
}

export function bars(host: HTMLElement, rows: BarRow[], note: string): void {
  void host;
  void rows;
  void note;
  throw new Error('bars: not written yet');
}

export function split(host: HTMLElement, rows: SplitRow[], legend: LegendItem[]): void {
  void host;
  void rows;
  void legend;
  throw new Error('split: not written yet');
}

export function stats(host: HTMLElement, list: StatLine[]): void {
  void host;
  void list;
  throw new Error('stats: not written yet');
}
