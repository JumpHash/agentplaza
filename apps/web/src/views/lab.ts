// The Lab (`#/lab`, `#/lab/<section>`, `#/lab/jobs/<id>`, `#/lab/models/<id>`): the training lab's
// sections under one sidebar entry. This file draws the section strip and hands the rest of the
// route to the section's own `render(content, params)`, where `params` is what follows the
// section (`[id]` for a job or a model detail). Each section module returns its cleanup.

import '../styles/lab.css';
import '../styles/lab-strip.css';
import { href } from '../router';
import { h, replace } from '../ui/dom';
import { tabs } from '../ui/components';
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
  const content = h('div.lab-stack');
  const strip = tabs(SECTIONS, section, (key) => {
    location.hash = href('lab', key);
  });
  replace(root, h('div.lab-body', null, h('div.lab-strip', null, strip), content));
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
