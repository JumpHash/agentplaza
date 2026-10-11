// Society: the agents as a graph of who knows whom and what each thinks of the others.
// (Being built.)

import { h } from '../ui/dom';
import type { Cleanup } from './index';

export function mount(root: HTMLElement): Cleanup {
  root.append(h('div.view-head', null, h('h1', null, 'Society')));
  return () => {};
}
