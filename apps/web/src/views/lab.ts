// The Lab (`#/lab`, `#/lab/<section>`, `#/lab/jobs/<id>`, `#/lab/models/<id>`): the training lab's
// sections under one sidebar entry. This file draws the page head and the section strip (the
// prototype's `h.head('Lab')` and `nav.tabs.lab-strip`), then hands the rest of the route to the
// section's own `render(content, params)`, where `params` is what follows the section (`[id]` for a
// job or a model detail). Each section module returns its cleanup.

import '../styles/proto/lab.css';
import { href } from '../router';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { render as renderOverview } from './lab-overview';
import { render as renderJobs } from './lab-jobs';
import { render as renderJobDetail } from './lab-job-detail';
import { render as renderRecipes } from './lab-recipes';
import { render as renderDatasets } from './lab-datasets';
import { render as renderModels } from './lab-models';
import { render as renderModelDetail } from './lab-model-detail';
import { render as renderBreeders } from './lab-breeders';

export const SECTIONS: { key: string; label: string }[] = [
  { key: 'overview', label: 'Overview' },
  { key: 'jobs', label: 'Jobs' },
  { key: 'recipes', label: 'Recipes' },
  { key: 'datasets', label: 'Datasets' },
  { key: 'models', label: 'Models' },
  { key: 'breeders', label: 'Breeders' },
];

export function mount(root: HTMLElement, params: string[]): Cleanup {
  const known = SECTIONS.find((s) => s.key === params[0]);
  const section = known ? known.key : 'overview';
  const rest = known ? params.slice(1) : [];
  const content = h('div.lab-body');
  const head = h(
    'header.page-head',
    null,
    h('div.page-head-row', null, h('h1', null, 'Lab'), h('span.sp'), h('div.page-actions')),
  );
  const strip = h(
    'nav.tabs.lab-strip',
    { role: 'tablist', 'aria-label': 'Lab sections' },
    SECTIONS.map((s) =>
      h(
        'a.tab',
        {
          role: 'tab',
          href: href('lab', s.key),
          'aria-selected': s.key === section ? 'true' : 'false',
          'aria-current': s.key === section ? 'page' : null,
        },
        s.label,
      ),
    ),
  );
  replace(root, head, strip, content);
  const cleanup = dispatch(section, rest, content);
  return () => cleanup();
}

function dispatch(section: string, rest: string[], content: HTMLElement): Cleanup {
  switch (section) {
    case 'jobs':
      return rest[0] ? renderJobDetail(content, rest) : renderJobs(content, rest);
    case 'recipes':
      return renderRecipes(content, rest);
    case 'datasets':
      return renderDatasets(content, rest);
    case 'models':
      return rest[0] ? renderModelDetail(content, rest) : renderModels(content, rest);
    case 'breeders':
      return renderBreeders(content, rest);
    default:
      return renderOverview(content, rest);
  }
}
