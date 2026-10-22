// The chats' model chip and context bar. The chip shows the model a chat runs on (the brand, the model id in mono, the
// effort) and opens the model picker with an effort row under it; choosing saves at once. The bar is a thin line of
// the context window's parts, with the count and the share at its right; clicking it opens the breakdown. Both are
// mounted in the four chats: the IDE agent chat, the owner conversation, the Overview orchestrator composer and the
// agent page composer. Pure helpers (number text, part order and colours, tone) are exported for the tests.

import '../styles/proto/chat-tools.css';
import { api, type Tier } from '../api';
import { contextApi, CONTEXT_PARTS, type ContextPart, type ContextView } from '../api-context';
import { providersApi } from '../api-providers';
import { tierChoices } from '../views/agent-detail';
import { agents, on } from '../store';
import { brandLogo, button, toast } from './components';
import { h, lu, replace } from './dom';
import { modelPicker, type ModelChoice, type ModelPicker } from './model-picker';

// ---- Pure helpers ------------------------------------------------------------------------------------------------

export const PART_NAME: Record<ContextPart, string> = {
  system: 'System',
  tools: 'Tools',
  history: 'History',
  tool_results: 'Tool results',
  attachments: 'Attachments',
  summary: 'Summary',
};

/** Each part's colour: a palette token, so the bar follows the skin and the mode. */
export const PART_COLOUR: Record<ContextPart, string> = {
  system: 'var(--ink-3)',
  tools: 'var(--info)',
  history: 'var(--signal)',
  tool_results: 'var(--review)',
  attachments: 'var(--ok)',
  summary: 'var(--warn)',
};

export interface PartStyle {
  part: ContextPart;
  name: string;
  colour: string;
  tokens: number;
}

/** The parts in the breakdown's order (system first, summary last), each with its name and colour. A part the hub did not send counts 0; a part this console does not know is dropped. */
export function orderedParts(parts: { part: string; tokens: number }[]): PartStyle[] {
  return CONTEXT_PARTS.map((part) => {
    const row = parts.find((p) => p.part === part);
    return { part, name: PART_NAME[part], colour: PART_COLOUR[part], tokens: Math.max(0, row?.tokens ?? 0) };
  });
}

/**
 * A token count in the console's compact form: 950, 41.2k, 200k, 1.2M. One decimal below 100k, whole thousands
 * up to a million, then millions with one decimal (999,600 rounds up to 1.0M, never 1000k).
 */
export function tokensText(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return '0';
  if (n < 1000) return String(Math.round(n));
  const k = n < 100_000 ? Math.round(n / 100) / 10 : Math.round(n / 1000);
  if (k < 1000) return `${k}k`;
  return `${(Math.round(n / 100_000) / 10).toFixed(1)}M`;
}

/** The share of a total as a whole percentage: "21%". A zero total reads 0%. */
export function shareText(part: number, total: number): string {
  if (!(total > 0)) return '0%';
  return `${Math.round((part / total) * 100)}%`;
}

/** The bar's tone: warn over 80 % of the window, crit over 95 %, otherwise ok (also when the window is unknown). */
export function usageTone(fill: number | null): 'ok' | 'warn' | 'crit' {
  if (fill === null || !Number.isFinite(fill)) return 'ok';
  if (fill > 0.95) return 'crit';
  if (fill > 0.8) return 'warn';
  return 'ok';
}

/** The bar's text: "41.2k / 200k · 21%" when the window is known, "41.2k used" when it is not. */
export function barText(used: number, window: number | null, fill: number | null): string {
  if (!window || window <= 0) return `${tokensText(used)} used`;
  const share = fill ?? used / window;
  return `${tokensText(used)} / ${tokensText(window)} · ${Math.round(share * 100)}%`;
}

/** A model value split into its base (`provider/model`) and its effort (`@effort`, '' when none). */
export function splitModel(value: string): { base: string; effort: string } {
  const at = value.lastIndexOf('@');
  return at < 0 ? { base: value, effort: '' } : { base: value.slice(0, at), effort: value.slice(at + 1) };
}

/** A base with an effort pinned to it (`provider/model@high`); '' stays '' (the default). */
export function withEffort(base: string, effort: string): string {
  return base && effort ? `${base}@${effort}` : base;
}

/** The effort row's levels: the default, then low to max. */
export const EFFORT_LEVELS: [string, string][] = [
  ['', 'Default'],
  ['low', 'Low'],
  ['medium', 'Medium'],
  ['high', 'High'],
  ['max', 'Max'],
];

// ---- Model choices -----------------------------------------------------------------------------------------------

const CHOICE_TTL_MS = 60_000;
const choiceCache = new Map<Tier, { at: number; load: Promise<ModelChoice[]> }>();
// A model listing refreshed in the background: the pickers read the models again, not the minute-old choices.
on('models', () => choiceCache.clear());

/**
 * The models a tier may run, as the chip's picker lists them (the policy first for a worker, every model after it;
 * the default first). Cached for a minute, so the four chats share one request.
 */
export function loadModelChoices(tier: Tier): Promise<ModelChoice[]> {
  const hit = choiceCache.get(tier);
  if (hit && Date.now() - hit.at < CHOICE_TTL_MS) return hit.load;
  const load = Promise.all([api.models(), api.settings()]).then(([view, settings]) =>
    tierChoices(tier, settings, view.models, { label: 'Use the default' }),
  );
  choiceCache.set(tier, { at: Date.now(), load });
  load.catch(() => choiceCache.delete(tier));
  return load;
}

/** Provider id to its brand (the instance's kind), loaded once. A provider not listed draws its own id. */
let brands: Promise<Map<string, string>> | null = null;
function brandsOf(): Promise<Map<string, string>> {
  brands ??= providersApi.list().then(
    (list) => new Map(list.map((i) => [i.id, i.kind] as [string, string])),
    () => new Map<string, string>(),
  );
  return brands;
}

// ---- The model chip ----------------------------------------------------------------------------------------------

export interface ModelChipOpts {
  /** The chip's accessible name, also the picker's. */
  label: string;
  /** Saves a value (`provider/model[@effort]`, '' for the default). A rejection's message shows under the composer. */
  save: (value: string) => Promise<unknown>;
}

export interface ModelChipState {
  /** The chat's own override (`provider/model[@effort]`); '' when it runs the default. */
  value: string;
  /** The model the chat runs on now, as the hub resolves it. */
  shown: string;
}

export interface ModelChip {
  /** The chip, its popover and the picker. */
  el: HTMLElement;
  /** Where a save error shows: place it under the composer. Empty when there is none. */
  note: HTMLElement;
  set(state: ModelChipState): void;
  setChoices(choices: ModelChoice[]): void;
  dispose(): void;
}

/** The chip in the composer's tool row: the brand, the model id in mono, the effort, a chevron; it opens the picker and the effort row. */
export function modelChip(opts: ModelChipOpts): ModelChip {
  let value = '';
  let shown = '';
  let choices: ModelChoice[] = [];
  let brandOf = new Map<string, string>();
  let open = false;

  const note = h('p.ct-note', { role: 'alert' });

  // The picker lists base models only: the effort is the row under it, not a part of the choice.
  const pickerChoices = (): ModelChoice[] => {
    const seen = new Set<string>();
    const out: ModelChoice[] = [];
    for (const c of choices) {
      const base = splitModel(c.value).base;
      if (seen.has(base)) continue;
      seen.add(base);
      out.push({ ...c, value: base });
    }
    const current = splitModel(value).base;
    if (current && !seen.has(current)) out.push({ value: current, label: current, provider: current.split('/')[0] ?? '' });
    return out;
  };

  const picker: ModelPicker = modelPicker({
    choices: [],
    label: opts.label,
    placeholder: 'Choose a model',
    onChange: (base) => {
      void commit(base ? withEffort(base, splitModel(value).effort) : '');
    },
  });

  const chipBtn = h('button.ct-chip', { type: 'button', 'aria-haspopup': 'dialog', 'aria-expanded': 'false', 'aria-label': `${opts.label}: ${shown || 'default'}` });
  const effortRow = h('div.ct-eff-row', { role: 'radiogroup', 'aria-label': 'Reasoning effort' }, h('span.ct-eff-l', null, 'Effort'));
  const pop = h(
    'div.ct-pop.ct-model-pop',
    { role: 'dialog', 'aria-label': opts.label, hidden: true },
    h('div.ct-picker', null, picker.el),
    effortRow,
  );
  const wrap = h('div.ct-chip-wrap', null, chipBtn, pop);

  /** Pins an effort on the model in use (or the picker's pick); the default model is the one the chat runs on. */
  const pickEffort = (effort: string): void => {
    const base = picker.value || splitModel(shown).base;
    if (!base) return;
    void commit(withEffort(base, effort));
  };

  for (const [effort, label] of EFFORT_LEVELS) {
    const b = h(
      'button.ct-seg',
      { type: 'button', role: 'radio', 'data-effort': effort, onclick: () => pickEffort(effort) },
      label,
    );
    effortRow.append(b);
  }

  const paint = (): void => {
    const { base, effort } = splitModel(value || shown);
    const provider = base.split('/')[0] ?? '';
    const id = base.includes('/') ? base.slice(base.indexOf('/') + 1) : base;
    const brand = brandOf.get(provider) ?? provider;
    const { effort: pinned } = splitModel(value);
    const shownEffort = pinned || effort;
    chipBtn.replaceChildren(
      ...[
        base ? brandLogo(brand) : h('span.ct-model', null, '…'),
        base ? h('span.ct-model', { title: base }, id) : null,
        shownEffort ? h('span.ct-eff', null, `· ${shownEffort}`) : null,
        lu('chevron-down', 14, 'ct-chev'),
      ].filter((n): n is HTMLElement | SVGSVGElement => n !== null),
    );
    chipBtn.setAttribute('aria-label', `${opts.label}: ${base || 'default'}${shownEffort ? `, ${shownEffort} effort` : ''}`);
    picker.setChoices(pickerChoices());
    picker.value = splitModel(value).base;
    for (const b of effortRow.querySelectorAll<HTMLButtonElement>('.ct-seg')) {
      b.setAttribute('aria-checked', String(b.dataset.effort === pinned));
    }
  };

  /** Saves at once; the chip shows the new value and goes back when the hub refuses it. */
  async function commit(next: string): Promise<void> {
    if (next === value) return;
    const prev = value;
    value = next;
    note.textContent = '';
    paint();
    try {
      await opts.save(next);
    } catch (err) {
      value = prev;
      note.textContent = err instanceof Error ? err.message : String(err);
      paint();
    }
  }

  const onDown = (e: MouseEvent): void => {
    if (!wrap.contains(e.target as Node)) setOpen(false);
  };
  const onKey = (e: KeyboardEvent): void => {
    if (e.key === 'Escape') setOpen(false);
  };
  function setOpen(next: boolean): void {
    if (next === open) return;
    open = next;
    pop.hidden = !next;
    chipBtn.setAttribute('aria-expanded', String(next));
    if (next) {
      window.setTimeout(() => document.addEventListener('mousedown', onDown), 0);
      document.addEventListener('keydown', onKey);
    } else {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    }
  }
  chipBtn.addEventListener('click', () => setOpen(!open));

  void brandsOf().then((m) => {
    brandOf = m;
    paint();
  });
  paint();

  return {
    el: wrap,
    note,
    set(state) {
      value = state.value;
      shown = state.shown;
      paint();
    },
    setChoices(next) {
      choices = next;
      paint();
    },
    dispose() {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    },
  };
}

/**
 * Keeps a chip on the agent `cid()`: its override, the model it runs on, and the choices its tier offers. Follows the
 * agents store, so a model the hub changes shows at once.
 */
export function followAgentModel(chip: ModelChip, cid: () => number): () => void {
  let tier: Tier | '' = '';
  return agents.watch((list) => {
    const a = list.find((x) => x.cid === cid());
    if (!a) return;
    if (a.tier !== tier) {
      tier = a.tier;
      void loadModelChoices(a.tier).then(
        (c) => chip.setChoices(c),
        () => undefined,
      );
    }
    chip.set({ value: a.model_override ?? '', shown: a.model });
  });
}

// ---- The context bar ---------------------------------------------------------------------------------------------

/** A `context` event arrives at most once a second per agent; the bar refetches once a second at most. */
const EVENT_DEBOUNCE_MS = 1_000;
/** A fallback refetch while the bar shows a view, in case the event stream dropped a frame. */
const POLL_MS = 10_000;

export interface ContextBarOpts {
  /** The chat's agent. A `context` event for this cid refetches the bar; 0 means not known yet. */
  cid: () => number;
  /** Fetches the chat's context: an agent's (given its cid), or an owner conversation's. */
  source: (cid: number) => Promise<ContextView>;
}

export interface ContextBar {
  el: HTMLElement;
  /** Refetches now (after a send, or once the chat knows its agent). */
  refresh(): void;
  dispose(): void;
}

/** The thin context line above the composer, with its breakdown popover and the Compress button at warn and crit. */
export function contextBar(opts: ContextBarOpts): ContextBar {
  let alive = true;
  let view: ContextView | null = null;
  let seq = 0;
  let debounce: ReturnType<typeof setTimeout> | undefined;
  let open = false;

  const hit = h('button.ct-ctx-hit', { type: 'button', 'aria-haspopup': 'dialog', 'aria-expanded': 'false', title: 'Context breakdown' });
  const pop = h('div.ct-pop.ct-breakdown', { role: 'dialog', 'aria-label': 'Context breakdown', hidden: true });
  const compress = button('Compress', () => void doCompress(), { kind: 'ghost', size: 'sm', title: 'Compress the conversation' });
  compress.hidden = true;
  const el = h('div.ct-ctx', { 'data-tone': 'ok' }, hit, compress, pop);

  const onDown = (e: MouseEvent): void => {
    if (!el.contains(e.target as Node)) setOpen(false);
  };
  const onKey = (e: KeyboardEvent): void => {
    if (e.key === 'Escape') setOpen(false);
  };
  function setOpen(next: boolean): void {
    if (next === open) return;
    open = next;
    pop.hidden = !next;
    hit.setAttribute('aria-expanded', String(next));
    if (next) {
      drawBreakdown();
      placeBreakdown();
      window.setTimeout(() => document.addEventListener('mousedown', onDown), 0);
      document.addEventListener('keydown', onKey);
    } else {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    }
  }
  hit.addEventListener('click', () => setOpen(!open));

  function drawBar(): void {
    if (!view) return;
    const used = view.used;
    const windowTokens = view.window && view.window > 0 ? view.window : null;
    const fill = view.fill ?? (windowTokens ? used / windowTokens : null);
    const tone = usageTone(fill);
    el.dataset.tone = tone;
    compress.hidden = tone === 'ok';
    // The track is the window (or what is used, when the window is unknown); each part takes its share of it.
    const total = Math.max(windowTokens ?? 0, used);
    const segs = orderedParts(view.parts)
      .filter((p) => p.tokens > 0 && total > 0)
      .map((p) => h('i.ct-part', { style: `width:${((p.tokens / total) * 100).toFixed(2)}%;background:${p.colour}`, title: `${p.name} · ${tokensText(p.tokens)}` }));
    const text = barText(used, windowTokens, fill);
    replace(hit, h('span.ct-track', { 'aria-hidden': 'true' }, segs), h('span.ct-ctx-txt', null, text));
    hit.setAttribute('aria-label', `Context: ${text}. Show the breakdown`);
  }

  /**
   * Opens the breakdown above the bar, or below it when the room above (under the top bar) is smaller than the
   * popover. Its height is capped to the room it gets, and it scrolls past that.
   */
  function placeBreakdown(): void {
    const gap = 8;
    const topBar = parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--bar-h')) || 0;
    const r = el.getBoundingClientRect();
    const roomAbove = r.top - topBar - gap;
    const roomBelow = window.innerHeight - r.bottom - gap;
    // The popover's own height is measured with no height cap, so the test is about its content.
    pop.style.maxHeight = '';
    const height = pop.offsetHeight;
    const below = height > roomAbove && roomBelow > roomAbove;
    pop.classList.toggle('down', below);
    pop.style.maxHeight = `${Math.max(120, Math.floor(below ? roomBelow : roomAbove))}px`;
  }

  function drawBreakdown(): void {
    if (!view) return;
    const v = view;
    const used = v.used;
    const parts = orderedParts(v.parts);
    replace(
      pop,
      h(
        'div.ct-bd-top',
        null,
        h('b.mono', { title: v.model }, v.model || 'Model unknown'),
        h('span', null, v.window ? `${tokensText(v.window)} window` : 'Window unknown'),
      ),
      h('div.ct-bd-sec', null, 'Context'),
      h(
        'ul.ct-rows',
        null,
        parts.map((p) =>
          h(
            'li.ct-row.ct-row-part',
            { 'data-part': p.part },
            h('i.ct-sw', { style: `background:${p.colour}`, 'aria-hidden': 'true' }),
            h('span.ct-cell', null, p.name),
            h('span.ct-num', null, tokensText(p.tokens)),
            h('span.ct-num.ct-share', null, shareText(p.tokens, used)),
          ),
        ),
      ),
      h('div.ct-bd-sec', null, 'Tool calls'),
      v.tools.length
        ? h(
            'ul.ct-rows',
            null,
            v.tools.map((t) =>
              h(
                'li.ct-row.ct-row-tool',
                { 'data-tool': t.tool },
                h('span.ct-cell.mono', { title: t.tool }, t.tool),
                h('span.ct-num', null, `${t.calls} ${t.calls === 1 ? 'call' : 'calls'}`),
                h('span.ct-num', { class: t.errors > 0 ? 'ct-crit' : '' }, `${t.errors} ${t.errors === 1 ? 'error' : 'errors'}`),
                h('span.ct-num', null, `${tokensText(t.result_tokens)} result`),
              ),
            ),
          )
        : h('p.ct-empty', null, 'No tool calls yet'),
      h('div.ct-bd-foot', null, `${v.calls} model ${v.calls === 1 ? 'call' : 'calls'} · ${v.compactions} ${v.compactions === 1 ? 'compaction' : 'compactions'}`),
    );
  }

  async function fetchNow(): Promise<void> {
    const my = ++seq;
    try {
      const next = await opts.source(opts.cid());
      if (!alive || my !== seq) return;
      view = next;
      drawBar();
      if (open) {
        drawBreakdown();
        placeBreakdown();
      }
    } catch {
      // Keep the last view: a failed refetch is not a reason to blank the line.
    }
  }

  function schedule(): void {
    if (debounce !== undefined) return;
    debounce = setTimeout(() => {
      debounce = undefined;
      if (alive) void fetchNow();
    }, EVENT_DEBOUNCE_MS);
  }

  async function doCompress(): Promise<void> {
    const cid = opts.cid();
    if (!cid) return;
    try {
      await contextApi.compress(cid);
      toast('Compressing the conversation');
      schedule();
    } catch (err) {
      toast(err instanceof Error ? err.message : String(err), 'error');
    }
  }

  const off = on('context', (raw) => {
    const cid = opts.cid();
    if (!cid || (raw as { cid?: unknown } | null)?.cid !== cid) return;
    schedule();
  });
  const poll = setInterval(() => {
    if (view && alive) void fetchNow();
  }, POLL_MS);

  return {
    el,
    refresh: () => void fetchNow(),
    dispose() {
      alive = false;
      off();
      if (debounce !== undefined) clearTimeout(debounce);
      clearInterval(poll);
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    },
  };
}
