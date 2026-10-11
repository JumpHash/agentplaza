// The floor screen. TODO: built by its own slice.

import { empty } from '../ui/components';
import type { Cleanup } from './index';

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  root.appendChild(empty('floor', 'Coming soon'));
  return () => {};
}
