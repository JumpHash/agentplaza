// Models: providers and their keys (`#/models`), and the model policy and
// defaults (`#/models/policy`).

import { go } from '../router';
import { tabs } from '../ui/components';
import { h } from '../ui/dom';
import type { Cleanup } from './index';
import { mountModelPolicy } from './models-policy';
import { mountProviders } from './models-providers';

export function mount(root: HTMLElement, params: string[]): Cleanup {
  const tab = params[0] === 'policy' ? 'policy' : 'providers';
  const body = h('div.stack');
  root.append(
    h('div.view-head', null, h('h1', null, 'Models')),
    tabs([{ key: 'providers', label: 'Providers' }, { key: 'policy', label: 'Policy & defaults' }], tab, (key) => go('models', ...(key === 'policy' ? ['policy'] : []))),
    body,
  );
  return tab === 'policy' ? mountModelPolicy(body) : mountProviders(body);
}
