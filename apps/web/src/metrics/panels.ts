// A Metrics panel: its card (title, sub, right text, freshness stamp, pinned-from line, the ⋯ menu) and its body,
// one renderer per body type.

import type { ChartCtx } from './charts';
import type { Panel } from './model';

/** What a panel can ask the view to do. */
export interface PanelCtx extends ChartCtx {
  /** Narrow to a scope and/or open a tab (and a trace). */
  go(target: { scope?: string; tab?: string; trace?: string }): void;
  /** The Traces filter changed. */
  filter(f: string): void;
  /** Alert actions. */
  mute(rule: string): void;
  ack(rule: string, subject: string): void;
  toggleRule(id: string, on: boolean): void;
  newRule(): void;
  /** "Load older" in a call log. */
  older(panelId: string): void;
  /** The panel menu's actions. */
  pin(p: Panel): void;
  isPinned(p: Panel): boolean;
  about(p: Panel): void;
  copyLink(p: Panel): void;
  /** Live panels show "live · N s ago"; others "as of HH:MM:SS". */
  liveNow: boolean;
}

export interface PanelHandle {
  el: HTMLElement;
  /** Replaces the panel's body (a live frame). */
  update(p: Panel): void;
  /** Redraws charts at the current size. */
  redraw(): void;
  /** Updates the freshness stamp. */
  fresh(lastMs: number, live: boolean): void;
  destroy(): void;
}

/** The panel's card, spanning `p.span` twelfths of the grid. */
export function renderPanel(p: Panel, ctx: PanelCtx): PanelHandle {
  void p;
  void ctx;
  throw new Error('renderPanel: not written yet');
}
