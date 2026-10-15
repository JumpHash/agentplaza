// The list-like panel bodies: tables, alert rows, rules, traces (list, waterfall, span details), events, bars, splits,
// stats and flags. panels.ts calls these; charts and figures are in charts.ts and figures.ts.

import type { PanelCtx } from './panels';
import type { AlertRow, BarRow, EventRow, Flag, LegendItem, RuleRow, SplitRow, StatLine, Table, TracesBody } from './model';

export function table(host: HTMLElement, t: Table, ctx: PanelCtx): void {
  void host;
  void t;
  void ctx;
  throw new Error('table: not written yet');
}

export function alerts(host: HTMLElement, rows: AlertRow[], ctx: PanelCtx): void {
  void host;
  void rows;
  void ctx;
  throw new Error('alerts: not written yet');
}

export function rules(host: HTMLElement, rows: RuleRow[], ctx: PanelCtx): void {
  void host;
  void rows;
  void ctx;
  throw new Error('rules: not written yet');
}

export function traces(host: HTMLElement, t: TracesBody, ctx: PanelCtx): void {
  void host;
  void t;
  void ctx;
  throw new Error('traces: not written yet');
}

export function events(host: HTMLElement, kinds: string[], rows: EventRow[], ctx: PanelCtx): void {
  void host;
  void kinds;
  void rows;
  void ctx;
  throw new Error('events: not written yet');
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
