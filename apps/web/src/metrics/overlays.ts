// Overlays of the Metrics view: the panel and sibling menus, dialogs, the find palette, the keyboard help, the
// "what changed" drawer and toasts. One overlay at a time; Esc closes it and focus returns where it was.

import type { Around } from './model';

export interface MenuItem {
  label: string;
  key?: string;
  run(): void;
}

/** A menu under `anchor`; closes on a choice, Esc or a click outside. */
export function menu(anchor: HTMLElement, items: MenuItem[]): void {
  void anchor;
  void items;
  throw new Error('menu: not written yet');
}

export interface DialogAction {
  label: string;
  primary?: boolean;
  /** Return false to keep the dialog open (a validation failed). */
  run?(dialog: HTMLElement): boolean | void;
}

/** A modal dialog; returns a function that closes it. */
export function dialog(title: string, body: HTMLElement, actions: DialogAction[]): () => void {
  void title;
  void body;
  void actions;
  throw new Error('dialog: not written yet');
}

export interface PaletteItem {
  group: string;
  label: string;
  run(): void;
}

/** The find palette: a filter box over `items`, ↑ ↓ to move, Enter to run, Esc to close. */
export function palette(items: PaletteItem[], placeholder: string): void {
  void items;
  void placeholder;
  throw new Error('palette: not written yet');
}

/** The keyboard and mouse help. */
export function help(): void {
  throw new Error('help: not written yet');
}

export interface DrawerCtx {
  go(target: { scope?: string; tab?: string }): void;
  zoom(z: [number, number]): void;
  traces(): void;
}

/** "What changed around then": events, the metrics that moved most, kept traces, a zoom button. */
export function drawer(a: Around, ctx: DrawerCtx): void {
  void a;
  void ctx;
  throw new Error('drawer: not written yet');
}

/** Closes whatever overlay is open; true when one was. */
export function closeOverlay(): boolean {
  throw new Error('closeOverlay: not written yet');
}

/** Whether an overlay is open. */
export function overlayOpen(): boolean {
  return document.querySelector('.mx-scrim, .mx-menu, .mx-drawer') !== null;
}
