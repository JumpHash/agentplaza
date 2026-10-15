// The operational panel bodies: firing alerts (with mute, acknowledge, open), the rules table (on/off, new rule),
// kept traces (filter, list, waterfall, span details) and the events feed (kind filter).

import type { PanelCtx } from './panels';
import type { AlertRow, EventRow, RuleRow, TracesBody } from './model';

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
