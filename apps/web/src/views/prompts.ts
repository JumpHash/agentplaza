// Prompts: the prompt library (docs/design/prompt-library.md). `#/settings/agents/prompts` lists system prompts, AGENTS.md
// files and fragments, with a kind tab, an archived toggle and search, and opens the new-prompt dialog; `#/.../prompts/<id>`
// opens one in the editor (views/prompt-editor.ts). The Loom prototype's layout: a page head, the kind tabs with counts,
// the search bar and the table.

import '../styles/proto/library.css';
import { promptsApi, ApiError, type PromptCreate, type PromptKind, type PromptSummary } from '../api';
import { on } from '../store';
import { go, href } from '../router';
import { ago, badge, button, empty, field, input, modal, select, table, textarea, toast, type Column, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { bindingField, metaProblem, mountEditor, parseTags } from './prompt-editor';

const LIVE_MS = 250;

type Status = 'loading' | 'ready' | 'error';
type KindFilter = 'all' | PromptKind;

const INTRO = 'The prompt library: system prompts, AGENTS.md files and fragments. Every version is checked and scored.';
const EMPTY_TEXT = 'The library holds the system prompts your agents run on, the AGENTS.md files that guide them and the fragments you reuse across prompts. Every version is checked against the rules and scored, so you can see which one is best and roll back to it. Start with New prompt.';

const KIND_LABEL: Record<PromptKind, string> = { system: 'System prompt', agents_md: 'AGENTS.md', fragment: 'Fragment' };
const KIND_FILTERS: [KindFilter, string][] = [['all', 'All'], ['system', 'System prompts'], ['agents_md', 'AGENTS.md'], ['fragment', 'Fragments']];

/** Rules for a tone by score: 90 and up good, 70 and up warn, the rest bad. */
function scoreTone(score: number): Tone {
  return score >= 90 ? 'ok' : score >= 70 ? 'warn' : 'danger';
}

/** A binding as a chip: `role:x` reads "Role: x", `plaza` reads "Whole plaza". */
function bindingChip(binding: string | null): Node | string {
  if (!binding) return h('span.pr-note', null, '—');
  if (binding === 'plaza') return badge('Whole plaza', 'info');
  if (binding.startsWith('role:')) return badge(`Role: ${binding.slice('role:'.length)}`, 'info');
  return badge(binding, 'info');
}

/** A title as an id slug: lower case, runs of other characters become one dash. */
function slugOf(text: string): string {
  return text.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
}

let starterCache: Promise<Record<PromptKind, string>> | null = null;

/** The starter body for each kind, from the rules; fetched once and kept. */
function starters(): Promise<Record<PromptKind, string>> {
  if (!starterCache) {
    starterCache = promptsApi.rules().then(
      (r) => r.templates,
      (err: unknown) => {
        starterCache = null;
        throw err;
      },
    );
  }
  return starterCache;
}

export function mount(root: HTMLElement, params: string[]): Cleanup {
  if (params[0]) {
    return mountEditor(root, params[0], () => {
      location.hash = href('prompts');
    });
  }
  return mountList(root);
}

function mountList(root: HTMLElement): Cleanup {
  let alive = true;
  let seq = 0;
  let status: Status = 'loading';
  let failure = '';
  let prompts: PromptSummary[] = [];
  let kindFilter: KindFilter = 'all';
  let showArchived = false;
  let query = '';
  let closeDialog: (() => void) | null = null;
  let liveTimer: number | undefined;

  const search = input({ type: 'search', placeholder: 'Search by title, id, description or tag', 'aria-label': 'Search prompts', autocomplete: 'off' });
  search.classList.add('pr-q');
  const archivedIn = h('input', { type: 'checkbox' }) as HTMLInputElement;
  const tabBox = h('div');
  const countLine = h('p.pr-note', { 'aria-live': 'polite' });
  const listBox = h('div');
  const newButton = button('New prompt', () => openNew(), { kind: 'primary', icon: 'plus' });

  search.addEventListener('input', () => {
    query = search.value.trim().toLowerCase();
    renderList();
  });
  archivedIn.addEventListener('change', () => {
    showArchived = archivedIn.checked;
    void load();
  });

  /** The kind tabs, with the count of each kind among the rows the archived toggle leaves. */
  function renderTabs(): void {
    const live = prompts.filter((p) => showArchived || p.archived_ms == null);
    const count = (k: KindFilter): number => (k === 'all' ? live.length : live.filter((p) => p.kind === k).length);
    replace(
      tabBox,
      h(
        'div.tabs',
        { role: 'tablist' },
        KIND_FILTERS.map(([key, label]) => {
          const b = h('button.tab', { type: 'button', role: 'tab', 'aria-selected': String(kindFilter === key) }, label, ' ', h('span.count.num', null, String(count(key))));
          b.addEventListener('click', () => {
            kindFilter = key;
            renderTabs();
            renderList();
          });
          return b;
        }),
      ),
    );
  }

  /** The rows the filters leave: kind, the archived toggle (the API returns archived rows too), and the search. */
  function visible(): PromptSummary[] {
    return prompts.filter(
      (p) =>
        (showArchived || p.archived_ms == null) &&
        (kindFilter === 'all' || p.kind === kindFilter) &&
        (!query || [p.id, p.title, p.description, ...p.tags].join(' ').toLowerCase().includes(query)),
    );
  }

  function clearFilters(): void {
    kindFilter = 'all';
    query = '';
    search.value = '';
    renderTabs();
    renderList();
  }

  const columns: Column<PromptSummary>[] = [
    {
      label: 'Title',
      cell: (p) =>
        h(
          'span',
          null,
          h('a', { href: href('prompts', p.id), onclick: (e: Event) => e.stopPropagation() }, h('strong', null, p.title || p.id)),
          p.archived_ms != null ? ' ' : null,
          p.archived_ms != null ? badge('Archived') : null,
          h('div.mono.pr-note', null, p.id),
        ),
    },
    { label: 'Kind', cell: (p) => KIND_LABEL[p.kind] ?? p.kind },
    { label: 'Binding', cell: (p) => bindingChip(p.binding) },
    {
      label: 'Live',
      cell: (p) => (p.active_version != null ? `v${p.active_version}` : h('span.pr-note', null, 'Unpublished')),
    },
    {
      label: 'Score',
      cell: (p) => {
        const parts: Node[] = [p.score != null ? badge(String(p.score), scoreTone(p.score)) : h('span.pr-note', null, '—')];
        if (p.best_version != null && p.best_version !== p.active_version) parts.push(h('div.pr-note', null, `best: v${p.best_version}`));
        return h('div', null, parts);
      },
    },
    { label: 'Tags', cell: (p) => h('span', null, p.tags.map((t) => badge(t))) },
    { label: 'Updated', cell: (p) => ago(p.updated_ms) },
  ];

  function renderList(): void {
    if (status === 'loading') {
      countLine.textContent = '';
      replace(listBox, h('p.pr-note', null, 'Loading the prompts…'));
      return;
    }
    if (status === 'error') {
      countLine.textContent = '';
      replace(listBox, empty('alert', 'The prompts could not be loaded.', failure, button('Try again', () => void load(), { size: 'sm' })));
      return;
    }
    const shown = visible();
    const total = prompts.filter((p) => showArchived || p.archived_ms == null).length;
    countLine.textContent = total ? `Showing ${shown.length} of ${total}` : '';
    if (shown.length) {
      replace(listBox, h('div.tbl-wrap', null, table(columns, shown, (p) => go('prompts', p.id), (p) => p.id)));
      return;
    }
    if (total === 0 && kindFilter === 'all' && !query) {
      replace(listBox, empty('file', 'No prompts yet', EMPTY_TEXT, button('New prompt', () => openNew(), { kind: 'primary', icon: 'plus' })));
      return;
    }
    replace(listBox, empty('search', 'No prompts match these filters.', undefined, button('Clear filters', () => clearFilters(), { kind: 'ghost', size: 'sm' })));
  }

  /** The new-prompt dialog: kind, title, optional id, binding, description, tags and the body from the kind's starter. */
  function openNew(): void {
    const kindSel = select(
      (['system', 'agents_md', 'fragment'] as PromptKind[]).map((value) => ({ value, label: KIND_LABEL[value] })),
      'system',
    );
    const titleIn = input({ type: 'text', required: true, autocomplete: 'off', placeholder: 'What the prompt is for' });
    const idIn = input({ type: 'text', autocomplete: 'off', placeholder: 'generated from the title' });
    const bindingBox = h('div');
    let bind = bindingField(kindSel.value as PromptKind, null);
    const descIn = input({ type: 'text', autocomplete: 'off', placeholder: 'A line on what it is for' });
    const tagsIn = input({ type: 'text', autocomplete: 'off', placeholder: 'comma, separated' });
    const bodyIn = textarea({ rows: 12, required: true, placeholder: 'Loading the starter text…' }) as HTMLTextAreaElement;
    const submit = h('button.btn.primary', { type: 'submit' }, 'Create prompt') as HTMLButtonElement;

    // The body follows the kind's starter until the writer edits it.
    let autofilled = '';
    let templates: Record<PromptKind, string> | null = null;
    function fillBody(): void {
      if (!templates) return;
      if (bodyIn.value.trim() !== '' && bodyIn.value !== autofilled) return;
      autofilled = templates[kindSel.value as PromptKind];
      bodyIn.value = autofilled;
    }

    function renderBinding(): void {
      bind = bindingField(kindSel.value as PromptKind, null);
      replace(bindingBox, bind.node);
    }

    async function create(): Promise<void> {
      const kind = kindSel.value as PromptKind;
      const meta = {
        title: titleIn.value.trim(),
        description: descIn.value.trim(),
        tags: parseTags(tagsIn.value),
        binding: bind.value(),
      };
      const problem = metaProblem(kind, meta);
      if (problem) {
        toast(problem, 'error');
        return;
      }
      const body: PromptCreate = { id: idIn.value.trim() || undefined, kind, ...meta, body: bodyIn.value };
      submit.disabled = true;
      try {
        const { prompt } = await promptsApi.create(body);
        closeDialog?.();
        closeDialog = null;
        go('prompts', prompt.id);
      } catch (err) {
        submit.disabled = false;
        const text = err instanceof Error ? err.message : String(err);
        toast(err instanceof ApiError && err.status === 409 ? `${text} (the id or binding is already taken)` : text, 'error');
      }
    }

    kindSel.addEventListener('change', () => {
      renderBinding();
      fillBody();
    });
    titleIn.addEventListener('input', () => {
      idIn.placeholder = slugOf(titleIn.value) || 'generated from the title';
    });
    renderBinding();
    void starters().then(
      (t) => {
        templates = t;
        fillBody();
        bodyIn.placeholder = '';
      },
      (err: unknown) => {
        bodyIn.placeholder = '';
        toast(err instanceof Error ? err.message : String(err), 'error');
      },
    );

    const form = h(
      'form.lib-form',
      {
        onsubmit: (e: Event) => {
          e.preventDefault();
          void create();
        },
      },
      field('Kind', kindSel),
      field('Title', titleIn),
      field('Id', idIn, 'Optional. Leave empty to use the slug of the title.'),
      bindingBox,
      field('Description', descIn),
      field('Tags', tagsIn, 'Separate them with commas.'),
      field('Body', bodyIn),
      h('div.lib-acts', null, button('Cancel', () => closeDialog?.(), { kind: 'ghost' }), submit),
    );
    closeDialog = modal('New prompt', form);
  }

  /** Loads the list. A quiet load (a live refresh) keeps the rows on screen while it runs. */
  async function load(quiet = false): Promise<void> {
    const mine = ++seq;
    if (!quiet) {
      status = 'loading';
      renderList();
    }
    try {
      const res = await promptsApi.list({ archived: showArchived });
      if (!alive || mine !== seq) return;
      if (!Array.isArray(res?.prompts)) throw new Error('The hub did not answer with a prompt list (is it an older version?)');
      prompts = res.prompts;
      status = 'ready';
    } catch (err) {
      if (!alive || mine !== seq) return;
      failure = err instanceof Error ? err.message : String(err);
      status = 'error';
    }
    renderTabs();
    renderList();
  }

  replace(
    root,
    h(
      'header.page-head',
      null,
      h('div.page-head-row', null, h('h1', null, 'Prompts'), h('span.sp'), h('div.page-actions', null, newButton)),
      h('p.lede', null, INTRO),
    ),
    tabBox,
    h(
      'div.pr-bar',
      null,
      search,
      h('label.check', null, archivedIn, ' Show archived'),
    ),
    countLine,
    listBox,
  );
  renderTabs();
  renderList();
  void load();

  // The hub publishes a `prompt` frame for every change to a prompt: the list reloads at once (debounced).
  const offPrompt = on('prompt', () => {
    window.clearTimeout(liveTimer);
    liveTimer = window.setTimeout(() => void load(true), LIVE_MS);
  });

  return () => {
    alive = false;
    window.clearTimeout(liveTimer);
    offPrompt();
    closeDialog?.();
    closeDialog = null;
  };
}

