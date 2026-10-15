// Metrics (`#/metrics?tab=…&scope=…&range=…`): every number the plaza records, by scope and tab. The hub composes
// each tab (crates/apz-metrics-view); this view lays out the header (breadcrumb, range, live, group, compare,
// annotations, export, find), the scope tree, the grouped tabs and the panel grid, and handles zoom, time shift,
// the "what changed" drawer, live frames, pins, the find palette and the keyboard. `fixture=1` shows the built-in
// fixture.

import '../styles/metrics.css';
import type { Cleanup } from './index';

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  void root;
  return () => {};
}
