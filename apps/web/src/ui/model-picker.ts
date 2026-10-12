// The model picker: a styled dropdown of models grouped by provider, used wherever a
// model is chosen (agent page, spawn dialog, model policy). Only the models a caller
// passes are offered — callers pass what the agent's tier allows, so nothing listed
// is refused.

import '../styles/model-picker.css';
import { h } from './dom';
import { brandLogo } from './components';

export interface ModelChoice {
  /** What is stored: `provider/model[@effort]`, or '' for an "inherit" choice. */
  value: string;
  /** The model's display name (falls back to its id). */
  label: string;
  /** The provider id the choice is grouped under; '' for the inherit choice (shown first, ungrouped). */
  provider: string;
  /** A short faint note, e.g. "low effort" or "current". */
  hint?: string;
  /** Listed in the "Presets" group (also still listed under its provider). */
  preset?: boolean;
}

export interface ModelPicker {
  /** The control to place in the page. */
  el: HTMLElement;
  /** The selected value. Setting it does not fire `onChange`. */
  value: string;
  /** Replaces the choices, keeping the value when it is still offered. */
  setChoices(choices: ModelChoice[]): void;
  disabled: boolean;
}

export interface ModelPickerOptions {
  choices: ModelChoice[];
  value?: string;
  /** Shown when nothing is selected and there is no inherit choice. */
  placeholder?: string;
  /** Called when the user picks a different choice. */
  onChange?: (value: string) => void;
  /** Accessible name. */
  label?: string;
}

/** Splits `provider/model[@effort]` into its parts. */
function splitValue(v: string): { base: string; effort: string; provider: string; model: string } {
  const at = v.indexOf('@');
  const base = at < 0 ? v : v.slice(0, at);
  const effort = at < 0 ? '' : v.slice(at + 1);
  const slash = base.indexOf('/');
  return {
    base,
    effort,
    provider: slash < 0 ? base : base.slice(0, slash),
    model: slash < 0 ? '' : base.slice(slash + 1),
  };
}

/** Builds choices from `provider/model[@effort]` strings, naming them from `names` (`provider/model` → display name). */
export function choicesFrom(values: string[], names: Map<string, string>, inherit?: { label: string }): ModelChoice[] {
  const out: ModelChoice[] = [];
  if (inherit) out.push({ value: '', provider: '', label: inherit.label });
  for (const value of values) {
    const { base, effort, provider, model } = splitValue(value);
    out.push({
      value,
      provider,
      label: names.get(base) ?? (model || base),
      hint: effort ? `${effort} effort` : undefined,
    });
  }
  return out;
}

export function modelPicker(opts: ModelPickerOptions): ModelPicker {
  let choices = opts.choices.slice();
  let value = opts.value ?? '';
  let disabled = false;
  let open = false;
  let query = '';
  // The options currently rendered, in visual order (inherit first, then provider groups).
  let flat: ModelChoice[] = [];
  let optEls: HTMLElement[] = [];
  let active = -1;

  const root = h('div.mp');
  const trigger = h('button.mp-trigger', {
    type: 'button',
    'aria-haspopup': 'listbox',
    'aria-expanded': 'false',
    'aria-label': opts.label,
  });
  root.appendChild(trigger);

  let pop: HTMLElement | null = null;
  let search: HTMLInputElement | null = null;
  let list: HTMLElement | null = null;

  function renderTrigger(): void {
    const cur = choices.find((c) => c.value === value);
    trigger.replaceChildren();
    if (cur) {
      if (cur.provider) trigger.appendChild(brandLogo(cur.provider));
      trigger.appendChild(h('span.mp-label', null, cur.label));
      if (cur.hint) trigger.appendChild(h('span.mp-hint', null, cur.hint));
    } else {
      trigger.appendChild(h('span.mp-placeholder', null, opts.placeholder ?? 'Choose a model'));
    }
    trigger.appendChild(h('span.mp-chev', { 'aria-hidden': 'true' }));
  }

  function paint(): void {
    optEls.forEach((el, i) => el.classList.toggle('is-active', i === active));
    optEls[active]?.scrollIntoView({ block: 'nearest' });
  }

  function addOption(c: ModelChoice, inlineLogo = false): void {
    const index = flat.length;
    flat.push(c);
    const { model } = splitValue(c.value);
    const el = h(
      'div.mp-opt',
      {
        role: 'option',
        'aria-selected': c.value === value ? 'true' : 'false',
        onclick: () => pick(c),
        onmouseenter: () => {
          active = index;
          paint();
        },
      },
      inlineLogo && c.provider ? brandLogo(c.provider) : null,
      h('span.mp-label', null, c.label),
      c.provider ? h('span.mp-id', null, model) : null,
      c.hint ? h('span.mp-hint', null, c.hint) : null,
    );
    optEls.push(el);
    list?.appendChild(el);
  }

  /** Re-renders the options for the current query. */
  function renderList(): void {
    if (!list) return;
    const q = query;
    const hits = choices.filter(
      (c) => !q || c.label.toLowerCase().includes(q) || c.value.toLowerCase().includes(q),
    );
    list.replaceChildren();
    flat = [];
    optEls = [];
    for (const c of hits) if (!c.provider) addOption(c);
    const presets = hits.filter((c) => c.preset);
    if (presets.length) {
      list.appendChild(h('div.mp-group', null, 'Presets'));
      for (const c of presets) addOption(c, true);
    }
    const groups = new Map<string, ModelChoice[]>();
    for (const c of hits) {
      if (!c.provider) continue;
      const g = groups.get(c.provider);
      if (g) g.push(c);
      else groups.set(c.provider, [c]);
    }
    for (const [provider, items] of groups) {
      list.appendChild(h('div.mp-group', null, brandLogo(provider), h('span', null, provider)));
      for (const c of items) addOption(c);
    }
    if (flat.length === 0) list.appendChild(h('div.mp-empty', null, 'No models match'));
  }

  const onDocClick = (e: MouseEvent): void => {
    if (!root.contains(e.target as Node)) close();
  };

  function openPop(): void {
    if (disabled || open) return;
    open = true;
    query = '';
    pop = h('div.mp-pop', { role: 'listbox', 'aria-label': opts.label });
    if (choices.length > 8) {
      search = h('input.mp-search', {
        type: 'search',
        placeholder: 'Search models',
        'aria-label': 'Search models',
        autocomplete: 'off',
        oninput: () => {
          query = (search?.value ?? '').trim().toLowerCase();
          renderList();
          active = flat.length ? 0 : -1;
          paint();
        },
      });
      pop.appendChild(search);
    }
    list = h('div.mp-list');
    pop.appendChild(list);
    root.appendChild(pop);
    trigger.setAttribute('aria-expanded', 'true');
    renderList();
    active = flat.findIndex((c) => c.value === value);
    paint();
    document.addEventListener('click', onDocClick);
    (search ?? trigger).focus();
  }

  function close(): void {
    if (!open) return;
    open = false;
    pop?.remove();
    pop = null;
    search = null;
    list = null;
    flat = [];
    optEls = [];
    active = -1;
    trigger.setAttribute('aria-expanded', 'false');
    document.removeEventListener('click', onDocClick);
  }

  function pick(c: ModelChoice): void {
    close();
    trigger.focus();
    if (c.value === value) return;
    value = c.value;
    renderTrigger();
    opts.onChange?.(value);
  }

  function move(delta: number): void {
    if (!flat.length) return;
    if (active < 0) active = delta > 0 ? 0 : flat.length - 1;
    else active = Math.min(flat.length - 1, Math.max(0, active + delta));
    paint();
  }

  trigger.addEventListener('click', () => (open ? close() : openPop()));

  root.addEventListener('keydown', (e: KeyboardEvent) => {
    if (disabled) return;
    if (!open) {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        openPop();
      }
      return;
    }
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        move(1);
        break;
      case 'ArrowUp':
        e.preventDefault();
        move(-1);
        break;
      case 'Enter': {
        const c = flat[active];
        if (c) {
          e.preventDefault();
          pick(c);
        }
        break;
      }
      case 'Escape':
        e.preventDefault();
        close();
        trigger.focus();
        break;
    }
  });

  renderTrigger();

  const api: ModelPicker = {
    el: root,
    get value(): string {
      return value;
    },
    set value(v: string) {
      value = v;
      renderTrigger();
    },
    setChoices(next: ModelChoice[]): void {
      choices = next.slice();
      // Keep the value only while it is still offered.
      if (!choices.some((c) => c.value === value)) value = '';
      renderTrigger();
      if (open) {
        renderList();
        paint();
      }
    },
    get disabled(): boolean {
      return disabled;
    },
    set disabled(d: boolean) {
      disabled = d;
      trigger.disabled = d;
      if (d) close();
    },
  };
  return api;
}
