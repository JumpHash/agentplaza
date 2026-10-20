// Models: providers and their keys (`#/settings/models/models`), and the model policy and defaults
// (`#/settings/models/models/policy`). The head holds the screen's action; the body is the tab's.

import '../styles/proto/settings-a.css';
import { h } from '../ui/dom';
import type { Cleanup } from './index';
import { mountModelPolicy } from './models-policy';
import { mountProviders } from './models-providers';

export function mount(root: HTMLElement, params: string[]): Cleanup {
  const tab = params[0] === 'policy' ? 'policy' : 'providers';
  const actions = h('div.mdl-head-a');
  const body = h('div.mdl-body');
  const link = (key: string, label: string, href: string): HTMLElement =>
    h('a.mdl-tab', { class: tab === key ? 'on' : '', href, 'aria-current': tab === key ? 'page' : null }, label);
  root.append(
    h(
      'div.mdl-page',
      null,
      h('div.view-head', null, h('div', null, h('h1', null, 'Models')), actions),
      h('nav.mdl-tabs', null, link('providers', 'Providers', '#/settings/models/models'), link('policy', 'Policy & defaults', '#/settings/models/models/policy')),
      body,
    ),
  );
  return tab === 'policy' ? mountModelPolicy(body, actions) : mountProviders(body, actions);
}
