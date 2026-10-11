// TODO: built by its own slice.

import { empty } from '../ui/components';
import type { Cleanup } from './index';

export function mountAgentList(root: HTMLElement): Cleanup {
  root.appendChild(empty('sparkles', 'Coming soon'));
  return () => {};
}
