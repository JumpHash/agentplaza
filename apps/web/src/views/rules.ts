// Rules: the plaza's laws (JHRL), explained for people — what each rule allows or forbids,
// for whom, when, why, how it is enforced and how often it has been — plus the latest
// infractions.

import { h } from '../ui/dom';
import type { Cleanup } from './index';

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  // contract: R1 — params[0] = a law id to open (e.g. #/rules/S-001a)
  root.append(h('div.view-head', null, h('h1', null, 'Rules')));
  return () => {};
}
