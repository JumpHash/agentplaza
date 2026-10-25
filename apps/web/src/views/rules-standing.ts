// The owner's standing rules, on the Rules screen (PLAN 9.8): the text every session's prompt and every engine's
// instruction file carries. Each save is a new version; the newest is the text in force.

import { api, type StandingRules } from '../api';
import { ago, attempt, button, textarea } from '../ui/components';
import { h, replace } from '../ui/dom';
import { STANDING_MAX, errorText, standingChanged, standingProblem } from './rules-model';

const HELP =
  'Rules every agent works by. Each session’s prompt carries them, and so does every engine’s instruction file (CLAUDE.md, AGENTS.md, GEMINI.md). A save is a new version; the newest is the text in force.';

/** The standing rules card. It loads the text and its history once and saves a new version on request; `alive` says
 * whether the page is still shown. */
export function standingCard(alive: () => boolean): HTMLElement {
  let saved: StandingRules | null = null;
  let busy = false;
  const area = textarea({
    'aria-label': 'Standing rules',
    'aria-describedby': 'rules-standing-help',
    rows: 8,
    maxlength: STANDING_MAX,
    disabled: true,
    placeholder: 'Write the rules every agent follows.',
  });
  const count = h('small.hint');
  const meta = h('p.hint');
  const fail = h('p.rl-err', { role: 'alert', hidden: true });
  const history = h('ol.rl-history', { 'aria-label': 'Versions', hidden: true });
  const saveButton = button('Save as new version', () => void save(), { kind: 'primary', size: 'sm' });
  const revertButton = button(
    'Revert',
    () => {
      area.value = saved?.text ?? '';
      paint();
    },
    { kind: 'ghost', size: 'sm' },
  );
  area.addEventListener('input', paint);

  function changed(): boolean {
    return saved !== null && standingChanged(area.value, saved.text);
  }

  function paint(): void {
    const problem = standingProblem(area.value);
    count.textContent = problem ?? `${Array.from(area.value.trim()).length} of ${STANDING_MAX} characters`;
    saveButton.disabled = busy || !changed() || problem !== null;
    revertButton.disabled = busy || !changed();
    meta.textContent = saved?.version
      ? `Version ${saved.version}, saved by ${saved.author ?? 'nobody'} ${ago(saved.updated_ms)}.`
      : 'No standing rules yet.';
  }

  function paintHistory(): void {
    const versions = saved?.versions ?? [];
    history.hidden = versions.length === 0;
    replace(
      history,
      versions
        .slice()
        .reverse()
        .map((v) => h('li', null, `Version ${v.version} · ${v.author} · ${ago(v.at_ms)}`)),
    );
  }

  async function load(): Promise<void> {
    try {
      const next = await api.standingRules();
      if (!alive()) return;
      saved = next;
      area.value = next.text;
      area.disabled = false;
      fail.hidden = true;
    } catch (err) {
      if (!alive()) return;
      fail.textContent = errorText(err);
      fail.hidden = false;
    }
    paint();
    paintHistory();
  }

  async function save(): Promise<void> {
    const text = area.value;
    busy = true;
    paint();
    const next = await attempt(() => api.saveStandingRules(text), 'Standing rules saved');
    busy = false;
    if (alive() && next) {
      saved = next;
      fail.hidden = true;
      paintHistory();
    }
    paint();
  }

  const section = h(
    'section.rl-card',
    { id: 'rules-standing' },
    h('div.rl-head', null, h('h2', null, 'Standing rules')),
    h('p.hint', { id: 'rules-standing-help' }, HELP),
    area,
    count,
    meta,
    fail,
    h('div.rl-acts', null, saveButton, revertButton),
    history,
  );
  paint();
  void load();
  return section;
}
