// TODO: built by its own slice.

import { empty } from '../ui/components';
import type { Cleanup } from './index';

export function mountAgentDetail(root: HTMLElement, _cid: number): Cleanup {
  root.appendChild(empty('sparkles', 'Coming soon'));
  return () => {};
}
