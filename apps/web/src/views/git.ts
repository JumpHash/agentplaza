// Git: the plaza's repositories on its git host — code, commits, branches and pull
// requests with their reviews; the owner reviews and merges here.

import { h } from '../ui/dom';
import type { Cleanup } from './index';

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  // contract: W1b — params[0] = repo name (optional), params[1] = tab, params[2] = PR number
  root.append(h('div.view-head', null, h('h1', null, 'Git')));
  return () => {};
}
