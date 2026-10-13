// The model picker: a styled dropdown of models grouped by provider, used wherever a
// model is chosen (agent page, spawn dialog, model policy). Only the models a caller
// passes are offered — callers pass what the agent's tier allows, so nothing listed
// is refused.

import '../styles/model-picker.css';
import { h } from './dom';
import { brandLogo, trainsBadge } from './components';

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
  /** A contributor model: its vendor trains on the data sent to it. */
  trains?: boolean;
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

/** Rows shown per group or vendor subgroup before a "Show N more" button. */
const SHOW_FIRST = 12;

/** The upstream vendor of an openrouter model: the second segment of `openrouter/vendor/name`. */
function vendorOf(c: ModelChoice): string {
  const { model } = splitValue(c.value);
  const slash = model.indexOf('/');
  return slash <= 0 ? 'other' : model.slice(0, slash);
}

/** The row's name. For openrouter the vendor segment is dropped (the full id goes in the row's title). */
function rowName(c: ModelChoice): string {
  if (c.provider !== 'openrouter') return c.label;
  const { model } = splitValue(c.value);
  const slash = model.indexOf('/');
  if (slash < 0 || !c.label.startsWith(model.slice(0, slash + 1))) return c.label;
  return c.label.slice(slash + 1) || c.label;
}

export interface ChoiceSubgroup {
  /** The upstream vendor, or 'other' for an id without one. */
  vendor: string;
  items: ModelChoice[];
}

export interface ChoiceGroup {
  /** 'presets' for the Presets group, else `p:<provider>`. */
  key: string;
  /** The provider id ('' for Presets). */
  provider: string;
  /** Matching rows in the whole group, subgroups included. */
  count: number;
  /** Matching rows directly under the group header (every group but openrouter). */
  items: ModelChoice[];
  /** Vendor subgroups sorted by vendor, 'other' last (openrouter only). */
  subgroups: ChoiceSubgroup[];
}

export interface GroupedChoices {
  /** Matching inherit choices: shown first, ungrouped. */
  inherit: ModelChoice[];
  /** Presets first, then one group per provider in first-seen order. */
  groups: ChoiceGroup[];
}

/**
 * Filters and groups the picker's choices. `query` is split on whitespace and every term must
 * match a choice's model id or display name (case-insensitive). Pure: no DOM and no row cap.
 */
export function groupChoices(choices: ModelChoice[], query: string): GroupedChoices {
  const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
  const hits = terms.length
    ? choices.filter((c) => {
        const hay = `${splitValue(c.value).base}\n${c.label}`.toLowerCase();
        return terms.every((t) => hay.includes(t));
      })
    : choices;
  const inherit = hits.filter((c) => !c.provider);
  const groups: ChoiceGroup[] = [];
  const presets = hits.filter((c) => c.provider && c.preset);
  if (presets.length) groups.push({ key: 'presets', provider: '', count: presets.length, items: presets, subgroups: [] });

  const byProvider = new Map<string, ModelChoice[]>();
  for (const c of hits) {
    if (!c.provider) continue;
    const rows = byProvider.get(c.provider);
    if (rows) rows.push(c);
    else byProvider.set(c.provider, [c]);
  }
  for (const [provider, rows] of byProvider) {
    const key = `p:${provider}`;
    if (provider !== 'openrouter') {
      groups.push({ key, provider, count: rows.length, items: rows, subgroups: [] });
      continue;
    }
    const byVendor = new Map<string, ModelChoice[]>();
    for (const c of rows) {
      const vendor = vendorOf(c);
      const items = byVendor.get(vendor);
      if (items) items.push(c);
      else byVendor.set(vendor, [c]);
    }
    const rank = (v: string): number => (v === 'other' ? 1 : 0);
    const subgroups = [...byVendor]
      .sort(([a], [b]) => rank(a) - rank(b) || a.localeCompare(b))
      .map(([vendor, items]) => ({ vendor, items }));
    groups.push({ key, provider, count: rows.length, items: [], subgroups });
  }
  return { inherit, groups };
}

export function modelPicker(opts: ModelPickerOptions): ModelPicker {
  let choices = opts.choices.slice();
  let value = opts.value ?? '';
  let disabled = false;
  let open = false;
  let query = '';
  // Group keys whose rows are shown past SHOW_FIRST; cleared when the panel opens.
  let expanded = new Set<string>();
  let timer: ReturnType<typeof setTimeout> | undefined;
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
      if (cur.trains) trigger.appendChild(trainsBadge());
    } else {
      trigger.appendChild(h('span.mp-placeholder', null, opts.placeholder ?? 'Choose a model'));
    }
    trigger.appendChild(h('span.mp-chev', { 'aria-hidden': 'true' }));
  }

  function paint(): void {
    optEls.forEach((el, i) => el.classList.toggle('is-active', i === active));
    optEls[active]?.scrollIntoView({ block: 'nearest' });
  }

  function addOption(c: ModelChoice, inlineLogo = false, nested = false): void {
    const index = flat.length;
    flat.push(c);
    const { base, model } = splitValue(c.value);
    const router = c.provider === 'openrouter';
    const el = h(
      'div.mp-opt',
      {
        role: 'option',
        // The full id, since an openrouter row shows only its name.
        title: router ? base : null,
        'aria-selected': c.value === value ? 'true' : 'false',
        onclick: () => pick(c),
        onmouseenter: () => {
          active = index;
          paint();
        },
      },
      inlineLogo && c.provider ? brandLogo(c.provider) : null,
      h('span.mp-label', null, rowName(c)),
      c.provider && !router ? h('span.mp-id', null, model) : null,
      c.hint ? h('span.mp-hint', null, c.hint) : null,
      c.trains ? trainsBadge() : null,
    );
    if (nested) el.classList.add('mp-nested');
    optEls.push(el);
    list?.appendChild(el);
  }

  /** Re-renders the rows for the current query. A group or vendor subgroup shows SHOW_FIRST rows until expanded. */
  function renderList(): void {
    const el = list;
    if (!el) return;
    const { inherit, groups } = groupChoices(choices, query);
    el.replaceChildren();
    flat = [];
    optEls = [];

    const showRows = (key: string, items: ModelChoice[], opt: { logo?: boolean; nested?: boolean } = {}): void => {
      const full = expanded.has(key);
      for (const c of full ? items : items.slice(0, SHOW_FIRST)) addOption(c, !!opt.logo, !!opt.nested);
      if (items.length > SHOW_FIRST) {
        el.appendChild(
          h(
            'button.mp-more',
            {
              type: 'button',
              'data-key': key,
              onclick: (e: Event) => {
                // The list is redrawn below, so this click must not reach the document test that closes the panel.
                e.stopPropagation();
                toggle(key);
              },
            },
            full ? 'Show fewer' : `Show ${items.length - SHOW_FIRST} more`,
          ),
        );
      }
    };

    for (const c of inherit) addOption(c);
    for (const g of groups) {
      if (g.key === 'presets') {
        el.appendChild(h('div.mp-group', null, h('span.mp-name', null, 'Presets'), h('span.mp-count', null, String(g.count))));
        showRows(g.key, g.items, { logo: true });
        continue;
      }
      el.appendChild(
        h(
          'div.mp-group',
          null,
          brandLogo(g.provider),
          h('span.mp-name', null, g.provider),
          h('span.mp-count', null, String(g.count)),
        ),
      );
      if (!g.subgroups.length) {
        showRows(g.key, g.items);
        continue;
      }
      for (const s of g.subgroups) {
        el.appendChild(h('div.mp-sub', null, h('span.mp-name', null, s.vendor), h('span.mp-count', null, String(s.items.length))));
        showRows(`${g.key}/${s.vendor}`, s.items, { nested: true });
      }
    }
    if (flat.length === 0) el.appendChild(h('div.mp-empty', null, 'No models match'));
  }

  /** Expands or collapses one group's rows, then keeps keyboard focus on its button. */
  function toggle(key: string): void {
    if (expanded.has(key)) expanded.delete(key);
    else expanded.add(key);
    renderList();
    active = Math.min(active, flat.length - 1);
    paint();
    list?.querySelector<HTMLElement>(`.mp-more[data-key="${key}"]`)?.focus();
  }

  const onDocClick = (e: MouseEvent): void => {
    if (!root.contains(e.target as Node)) close();
  };

  /** Applies the search text to the list (run after the debounce). */
  function applyQuery(): void {
    timer = undefined;
    query = search?.value ?? '';
    renderList();
    active = flat.length ? 0 : -1;
    paint();
  }

  function openPop(): void {
    if (disabled || open) return;
    open = true;
    query = '';
    expanded = new Set<string>();
    pop = h('div.mp-pop', { role: 'listbox', 'aria-label': opts.label });
    search = h('input.mp-search', {
      type: 'search',
      placeholder: 'Search models',
      'aria-label': 'Search models',
      autocomplete: 'off',
      oninput: () => {
        // A light debounce: a fast typist redraws the list once, not per key.
        if (timer !== undefined) clearTimeout(timer);
        timer = setTimeout(applyQuery, 80);
      },
    });
    pop.appendChild(search);
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
    if (timer !== undefined) {
      clearTimeout(timer);
      timer = undefined;
    }
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
        // A focused "Show more" button activates itself natively.
        if ((e.target as HTMLElement).classList.contains('mp-more')) break;
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
