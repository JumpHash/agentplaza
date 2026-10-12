// Projects: every goal's flow from brief to delivery (brief → research → proposal →
// critique → approval → implementation → review → delivered), with its documents,
// critiques, approval and tasks; the owner approves or sends a project back.

import { h } from '../ui/dom';
import type { Cleanup } from './index';

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  // contract: W1a
  root.append(h('div.view-head', null, h('h1', null, 'Projects')));
  return () => {};
}
