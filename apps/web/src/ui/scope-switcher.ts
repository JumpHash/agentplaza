// The top bar's project switcher: All projects, each live project, and New project…. The console shows what the
// choice scopes (scope.ts); the route carries it.

import { liveProjects, projects, projectTitle, scope, switchTo } from '../scope';
import { h } from './dom';
import { openNewProject } from './new-project';

const NEW = '__new';

export function scopeSwitcher(): HTMLElement {
  const sel = h('select.select.scope-switcher', { 'aria-label': 'Project', title: 'The project the console shows' }) as HTMLSelectElement;
  const draw = (): void => {
    const current = scope.get();
    const live = liveProjects(projects.get());
    const options: HTMLElement[] = [h('option', { value: '' }, 'All projects'), ...live.map((p) => h('option', { value: p.id }, p.title))];
    // A scope on a project that is over, or not loaded yet, still shows: the switcher says where the console is.
    if (current && !live.some((p) => p.id === current)) options.push(h('option', { value: current }, projectTitle(current)));
    options.push(h('option', { value: NEW }, 'New project…'));
    sel.replaceChildren(...options);
    sel.value = current ?? '';
  };
  sel.addEventListener('change', () => {
    if (sel.value === NEW) {
      sel.value = scope.get() ?? '';
      openNewProject();
      return;
    }
    switchTo(sel.value || null);
  });
  projects.watch(draw);
  scope.watch(draw);
  return sel;
}
