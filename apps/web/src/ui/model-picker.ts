// The model picker: the popover behind every model choice (agent page, spawn dialog, model policy, decisions).
// It offers only the choices its caller passes, so nothing listed is refused. A rail filters the list: the
// favourites (the owner's `favourite_models`, shared across devices) or one API instance's models. An instance
// whose health is crit lists no models, only its message and a way to its provider setup.

import '../styles/proto/model-picker.css';
import { api } from '../api';
import { providersApi, type ApiInstance } from '../api-providers';
import { brandLogo, button, dot, toast, trainsBadge } from './components';
import { h, lu } from './dom';

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

/** The rail's favourites button. A filter key no provider id can take (ids are `[a-z0-9_-]`). */
const FAVOURITES = '*';

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

/** The choices a rail filter keeps: all of them (''), the favourites, or one API instance's models. Pure. */
export function filterChoices(choices: ModelChoice[], filter: string, favourites: ReadonlySet<string>): ModelChoice[] {
  if (!filter) return choices;
  if (filter === FAVOURITES) return choices.filter((c) => !!c.provider && favourites.has(splitValue(c.value).base));
  return choices.filter((c) => c.provider === filter);
}

/** What the rail offers: an enabled API instance, or (when the hub did not answer) a provider the caller passed. */
interface Source {
  id: string;
  label: string;
  brand: string;
  accent: string | null;
  crit: boolean;
  message: string;
}

/** An accent the hub stores: a hex colour. Anything else is not used in a style. */
const HEX = /^#[0-9a-f]{3,8}$/i;

export function modelPicker(opts: ModelPickerOptions): ModelPicker {
  let choices = opts.choices.slice();
  let value = opts.value ?? '';
  let disabled = false;
  let open = false;
  let query = '';
  // The rail's filter: '' (every choice), FAVOURITES, or an instance id.
  let filter = '';
  // Group keys whose rows are shown past SHOW_FIRST; cleared when the panel opens.
  let expanded = new Set<string>();
  let timer: ReturnType<typeof setTimeout> | undefined;
  // The options currently rendered, in visual order (inherit first, then provider groups).
  let flat: ModelChoice[] = [];
  let optEls: HTMLElement[] = [];
  let active = -1;
  // The hub's view, loaded each time the popover opens: the instances (null until it answers) and the favourites.
  let instances: ApiInstance[] | null = null;
  let favourites = new Set<string>();
  // Favourite changes are saved one at a time, each on the settings as they are then.
  let saving: Promise<void> = Promise.resolve();

  const root = h('div.mp');
  const trigger = h('button.mp-trigger', {
    type: 'button',
    'aria-haspopup': 'listbox',
    'aria-expanded': 'false',
    'aria-label': opts.label,
  });
  root.appendChild(trigger);

  let pop: HTMLElement | null = null;
  let rail: HTMLElement | null = null;
  let search: HTMLInputElement | null = null;
  let list: HTMLElement | null = null;

  const instanceOf = (id: string): ApiInstance | undefined => instances?.find((i) => i.id === id);
  const brandOf = (id: string): string => instanceOf(id)?.kind ?? id;
  const labelOf = (id: string): string => instanceOf(id)?.label || id;
  const isCrit = (id: string): boolean => instanceOf(id)?.health.status === 'crit';

  function sources(): Source[] {
    if (instances) {
      return instances
        .filter((i) => i.enabled)
        .map((i) => ({
          id: i.id,
          label: i.label || i.id,
          brand: i.kind,
          accent: i.accent,
          crit: i.health.status === 'crit',
          message: i.health.message,
        }));
    }
    const ids = [...new Set(choices.map((c) => c.provider).filter(Boolean))];
    return ids.map((id) => ({ id, label: id, brand: id, accent: null, crit: false, message: '' }));
  }

  function renderTrigger(): void {
    const cur = choices.find((c) => c.value === value);
    trigger.replaceChildren();
    if (cur) {
      if (cur.provider) {
        trigger.appendChild(brandLogo(brandOf(cur.provider)));
        trigger.appendChild(h('span.mp-label.mp-mono', { title: cur.value }, splitValue(cur.value).model || cur.label));
      } else {
        trigger.appendChild(h('span.mp-label', null, cur.label));
      }
      if (cur.hint) trigger.appendChild(h('span.mp-hint', null, cur.hint));
      if (cur.trains) trigger.appendChild(trainsBadge());
    } else {
      trigger.appendChild(h('span.mp-placeholder', null, opts.placeholder ?? 'Choose a model'));
    }
    trigger.appendChild(lu('chevron-down', 14, 'mp-chev'));
  }

  function renderRail(): void {
    if (!rail) return;
    const star = h(
      'button.mp-rail-btn',
      {
        type: 'button',
        'data-filter': FAVOURITES,
        'aria-pressed': filter === FAVOURITES ? 'true' : 'false',
        title: 'Favourites',
        'aria-label': 'Favourites',
      },
      lu('star', 16),
    );
    const items = sources().map((s) =>
      h(
        'button.mp-rail-btn',
        {
          type: 'button',
          'data-filter': s.id,
          'aria-pressed': filter === s.id ? 'true' : 'false',
          title: s.label,
          'aria-label': s.crit ? `${s.label}, needs setup` : s.label,
          style: s.accent && HEX.test(s.accent) ? `--acc:${s.accent}` : null,
        },
        brandLogo(s.brand),
        s.crit ? h('i.mp-rail-crit', { 'aria-hidden': 'true' }) : null,
      ),
    );
    rail.replaceChildren(star, ...items);
  }

  function paint(): void {
    optEls.forEach((el, i) => el.classList.toggle('is-active', i === active));
    optEls[active]?.scrollIntoView({ block: 'nearest' });
  }

  /** The star that toggles a model's favourite. Its click never reaches the row, so it does not pick the model. */
  function starButton(base: string, on: boolean): HTMLElement {
    return h(
      'button.mp-star',
      {
        type: 'button',
        'aria-pressed': on ? 'true' : 'false',
        'aria-label': on ? `Remove ${base} from favourites` : `Add ${base} to favourites`,
        title: on ? 'Remove from favourites' : 'Add to favourites',
        onclick: (e: Event) => {
          e.stopPropagation();
          toggleFavourite(base);
        },
      },
      lu('star', 14),
    );
  }

  function addOption(c: ModelChoice, nested = false): void {
    const index = flat.length;
    flat.push(c);
    const { base, model } = splitValue(c.value);
    const el = h(
      'div.mp-opt',
      {
        role: 'option',
        // The full id, since an openrouter row shows only its name.
        title: c.provider ? base : null,
        'aria-selected': c.value === value ? 'true' : 'false',
        onclick: () => pick(c),
        onmouseenter: () => {
          active = index;
          paint();
        },
      },
      c.provider
        ? h('span.mp-id', null, c.provider === 'openrouter' ? rowName(c) : model)
        : h('span.mp-label', null, c.label),
      c.provider ? h('span.mp-inst', null, labelOf(c.provider)) : null,
      c.hint ? h('span.mp-hint', null, c.hint) : null,
      c.trains ? trainsBadge() : null,
      c.provider ? starButton(base, favourites.has(base)) : null,
    );
    if (nested) el.classList.add('mp-nested');
    optEls.push(el);
    list?.appendChild(el);
  }

  /** The notice in place of an instance's models: its health message and the way to its provider setup. */
  function critNotice(id: string): HTMLElement {
    const inst = instanceOf(id);
    const setup = button(
      'Open provider setup',
      () => {
        close();
        location.hash = `#/settings/models/models?instance=${encodeURIComponent(id)}`;
      },
      { kind: 'ghost', size: 'sm' },
    );
    setup.classList.add('link');
    return h(
      'div.mp-crit',
      { role: 'status' },
      h(
        'div.mp-crit-msg',
        null,
        dot('danger'),
        h('span', null, h('b', null, labelOf(id)), ' ', inst?.health.message || 'Not ready to list models.'),
      ),
      setup,
    );
  }

  /** Re-renders the rows for the rail filter and the search text. A group or vendor subgroup shows SHOW_FIRST rows until expanded. */
  function renderList(): void {
    const el = list;
    if (!el) return;
    el.replaceChildren();
    flat = [];
    optEls = [];

    if (filter && filter !== FAVOURITES && isCrit(filter)) {
      el.appendChild(critNotice(filter));
      return;
    }

    const showRows = (key: string, items: ModelChoice[], nested = false): void => {
      const full = expanded.has(key);
      for (const c of full ? items : items.slice(0, SHOW_FIRST)) addOption(c, nested);
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

    // An instance whose health is crit lists no models: its choices are left out and it gets a notice instead.
    const pool = filterChoices(choices, filter, favourites).filter((c) => !c.provider || !isCrit(c.provider));
    const { inherit, groups } = groupChoices(pool, query);
    for (const c of inherit) addOption(c);
    for (const g of groups) {
      if (g.key === 'presets') {
        el.appendChild(h('div.mp-group', null, h('span.mp-name', null, 'Presets'), h('span.mp-count', null, String(g.count))));
        showRows(g.key, g.items);
        continue;
      }
      if (!g.subgroups.length) {
        showRows(g.key, g.items);
        continue;
      }
      for (const s of g.subgroups) {
        el.appendChild(h('div.mp-sub', null, h('span.mp-name', null, s.vendor), h('span.mp-count', null, String(s.items.length))));
        showRows(`${g.key}/${s.vendor}`, s.items, true);
      }
    }

    let notices = 0;
    if (!filter) {
      for (const inst of instances ?? []) {
        if (!inst.enabled || inst.health.status !== 'crit' || !choices.some((c) => c.provider === inst.id)) continue;
        el.appendChild(critNotice(inst.id));
        notices += 1;
      }
    }
    if (flat.length === 0 && notices === 0) {
      el.appendChild(
        filter === FAVOURITES && !favourites.size && !query
          ? h('div.mp-empty', null, h('b', null, 'No favourites yet'), h('span', null, 'Star a model to list it here.'))
          : h('div.mp-empty', null, 'No models match'),
      );
    }
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

  /** Stars or unstars a model. The list shows the change at once; the save follows, and a failed save puts it back. */
  function toggleFavourite(base: string): void {
    const on = !favourites.has(base);
    if (on) favourites.add(base);
    else favourites.delete(base);
    renderList();
    paint();
    saving = saving
      .then(async () => {
        const fresh = await api.settings();
        const next = new Set(fresh.favourite_models ?? []);
        if (on) next.add(base);
        else next.delete(base);
        await api.saveSettings({ ...fresh, favourite_models: [...next] });
      })
      .catch((err: unknown) => {
        if (on) favourites.delete(base);
        else favourites.add(base);
        if (open) {
          renderList();
          paint();
        }
        toast(err instanceof Error ? err.message : String(err), 'error');
      });
  }

  // composedPath keeps the popover's own clicks inside it even when a redraw has removed their target.
  const onDocClick = (e: MouseEvent): void => {
    if (!e.composedPath().includes(root)) close();
  };

  /** Applies the search text to the list (run after the debounce). */
  function applyQuery(): void {
    timer = undefined;
    query = search?.value ?? '';
    renderList();
    active = flat.length ? 0 : -1;
    paint();
  }

  /** A rail button sets the filter; the pressed one again clears it (every choice). */
  function onRailClick(e: Event): void {
    const b = (e.target as Element).closest<HTMLElement>('[data-filter]');
    if (!b) return;
    const key = b.dataset.filter ?? '';
    filter = filter === key ? '' : key;
    expanded = new Set<string>();
    renderRail();
    renderList();
    active = flat.length ? 0 : -1;
    paint();
    search?.focus();
  }

  /** Opens the popover upwards when there is more room above the trigger. */
  function placePop(): void {
    if (!pop) return;
    const r = trigger.getBoundingClientRect();
    const below = window.innerHeight - r.bottom;
    pop.classList.toggle('up', below < 320 && r.top > below);
  }

  /** Loads the instances (for the rail and the crit notices) and the favourites; redraws what changed. */
  function loadSources(): void {
    void Promise.all([providersApi.list().catch(() => null), api.settings().catch(() => null)]).then(([list, settings]) => {
      if (list) instances = list;
      if (settings) favourites = new Set(settings.favourite_models ?? []);
      if (!open) return;
      renderTrigger();
      renderRail();
      renderList();
      paint();
    });
  }

  function openPop(): void {
    if (disabled || open) return;
    open = true;
    query = '';
    filter = '';
    expanded = new Set<string>();
    rail = h('div.mp-rail', { role: 'toolbar', 'aria-label': 'Filter models' });
    rail.addEventListener('click', onRailClick);
    search = h('input.mp-search-in', {
      type: 'search',
      placeholder: 'Search models…',
      'aria-label': 'Search models',
      autocomplete: 'off',
      oninput: () => {
        // A light debounce: a fast typist redraws the list once, not per key.
        if (timer !== undefined) clearTimeout(timer);
        timer = setTimeout(applyQuery, 80);
      },
    });
    list = h('div.mp-list', { role: 'listbox', 'aria-label': opts.label });
    pop = h('div.mp-pop', null, rail, h('div.mp-main', null, h('label.mp-search', null, lu('search', 15), search), list));
    root.appendChild(pop);
    placePop();
    trigger.setAttribute('aria-expanded', 'true');
    renderRail();
    renderList();
    active = flat.findIndex((c) => c.value === value);
    paint();
    document.addEventListener('click', onDocClick);
    search.focus();
    loadSources();
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
    rail = null;
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
        // A focused button (a star, a rail button, "Show more") acts on its own.
        if ((e.target as Element).closest('button')) break;
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

  const api_: ModelPicker = {
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
        renderRail();
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
  return api_;
}
