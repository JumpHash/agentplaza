// Prompts: the prompt library (docs/design/prompt-library.md). `#/prompts` lists system prompts, AGENTS.md files and
// fragments, with a kind filter, archived toggle and search, and opens the new-prompt dialog; `#/prompts/<id>` opens
// one in the editor (views/prompt-editor.ts).

import '../styles/prompts.css';
import { promptsApi, ApiError, type PromptCreate, type PromptKind, type PromptSummary } from '../api';
import { go, href } from '../router';
import { ago, badge, button, field, input, modal, select, table, textarea, toast, type Column, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { mountEditor } from './prompt-editor';

type Status = 'loading' | 'ready' | 'error';
type KindFilter = 'all' | PromptKind;

const INTRO = 'Versioned system prompts, AGENTS.md files and fragments, each checked against the plaza\'s rules as it is written. The live version is the one in use; the best version is the highest-scoring one kept.';
const EMPTY_TEXT = 'The library holds the system prompts your agents run on, the AGENTS.md files that guide them and the fragments you reuse across prompts. Every version is checked against the rules and scored, so you can see which one is best and roll back to it. Start with New prompt.';

const KIND_LABEL: Record<PromptKind, string> = { system: 'System prompt', agents_md: 'AGENTS.md', fragment: 'Fragment' };
const KIND_FILTERS: [KindFilter, string][] = [['all', 'All'], ['system', 'System prompts'], ['agents_md', 'AGENTS.md'], ['fragment', 'Fragments']];

/** Rules for a tone by score: 90 and up good, 70 and up warn, the rest bad. */
function scoreTone(score: number): Tone {
  return score >= 90 ? 'ok' : score >= 70 ? 'warn' : 'danger';
}

/** A binding as a chip: `role:x` reads "Role: x", `plaza` reads "Whole plaza". */
function bindingChip(binding: string | null): Node | string {
  if (!binding) return '—';
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
      location.hash = '#/prompts';
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

  const search = input({ type: 'search', placeholder: 'Search by title, id, description or tag', 'aria-label': 'Search prompts', autocomplete: 'off' });
  const archivedIn = h('input', { type: 'checkbox' }) as HTMLInputElement;
  const chipBox = h('div.prompts-chips', { role: 'group', 'aria-label': 'Kind' });
  const countLine = h('p.prompts-count', { 'aria-live': 'polite' });
  const listBox = h('div.prompts-list');
  const newButton = button('New prompt', () => openNew(), { kind: 'primary' });

  search.addEventListener('input', () => {
    query = search.value.trim().toLowerCase();
    renderList();
  });
  archivedIn.addEventListener('change', () => {
    showArchived = archivedIn.checked;
    void load();
  });

  function renderChips(): void {
    replace(
      chipBox,
      KIND_FILTERS.map(([key, label]) =>
        h(
          'button.prompts-chip',
          {
            type: 'button',
            'aria-pressed': String(kindFilter === key),
            onclick: () => {
              kindFilter = key;
              renderChips();
              renderList();
            },
          },
          label,
        ),
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
    renderChips();
    renderList();
  }

  const columns: Column<PromptSummary>[] = [
    {
      label: 'Title',
      cell: (p) =>
        h(
          'div.prompts-title',
          null,
          h('a', { href: href('prompts', p.id) }, p.title || p.id),
          h('code.prompts-id', null, p.id),
          p.archived_ms != null ? badge('Archived') : null,
        ),
    },
    { label: 'Kind', cell: (p) => KIND_LABEL[p.kind] ?? p.kind },
    { label: 'Binding', cell: (p) => bindingChip(p.binding) },
    {
      label: 'Live',
      cell: (p) => (p.active_version != null ? `v${p.active_version}` : h('span.prompts-muted', null, 'Unpublished')),
    },
    {
      label: 'Score',
      cell: (p) => {
        const parts: Node[] = [
          p.score != null ? badge(String(p.score), scoreTone(p.score)) : h('span.prompts-muted', null, '—'),
        ];
        if (p.best_version != null && p.best_version !== p.active_version) parts.push(h('span.prompts-best', null, `best: v${p.best_version}`));
        return h('div.prompts-score', null, parts);
      },
    },
    { label: 'Tags', cell: (p) => h('div.prompts-tags', null, p.tags.map((t) => h('span.prompts-tag', null, t))) },
    { label: 'Updated', cell: (p) => ago(p.updated_ms) },
  ];

  function renderList(): void {
    if (status === 'loading') {
      countLine.textContent = '';
      replace(listBox, h('p.prompts-muted', null, 'Loading the prompts…'));
      return;
    }
    if (status === 'error') {
      countLine.textContent = '';
      replace(
        listBox,
        h('div.prompts-error', { role: 'alert' }, h('p', null, `The prompts could not be loaded. ${failure}`), button('Try again', () => void load(), { size: 'sm' })),
      );
      return;
    }
    const shown = visible();
    const total = prompts.filter((p) => showArchived || p.archived_ms == null).length;
    countLine.textContent = total ? `Showing ${shown.length} of ${total}` : '';
    if (shown.length) {
      replace(listBox, h('div.prompts-table-wrap', null, table(columns, shown, undefined, (p) => p.id)));
      return;
    }
    if (total === 0 && kindFilter === 'all' && !query) {
      replace(
        listBox,
        h(
          'div.prompts-empty',
          null,
          h('h3', null, 'No prompts yet'),
          h('p', null, EMPTY_TEXT),
          button('New prompt', () => openNew(), { kind: 'primary' }),
        ),
      );
      return;
    }
    replace(
      listBox,
      h(
        'div.prompts-empty',
        null,
        h('p', null, 'No prompts match these filters.'),
        button('Clear filters', () => clearFilters(), { size: 'sm' }),
      ),
    );
  }

  /** The new-prompt dialog: kind, title, optional id, binding, description, tags and the body from the kind's starter. */
  function openNew(): void {
    const kindSel = select(
      (['system', 'agents_md', 'fragment'] as PromptKind[]).map((value) => ({ value, label: KIND_LABEL[value] })),
      'system',
    );
    const titleIn = input({ type: 'text', required: true, autocomplete: 'off', placeholder: 'What the prompt is for' });
    const idIn = input({ type: 'text', autocomplete: 'off', placeholder: 'generated from the title' });
    const roleIn = input({ type: 'text', autocomplete: 'off', placeholder: 'e.g. planner' });
    const sharedIn = h('input', { type: 'checkbox' }) as HTMLInputElement;
    const bindingBox = h('div.prompts-binding');
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
      const kind = kindSel.value as PromptKind;
      if (kind === 'system') replace(bindingBox, field('Role', roleIn, 'The agent role this prompt is bound to. Leave empty for none.'));
      else if (kind === 'agents_md') replace(bindingBox, h('label.prompts-check', null, sharedIn, ' Apply to every agent'));
      else replace(bindingBox, h('p.prompts-hint', null, 'Fragments are not bound to a role or to the plaza.'));
    }

    async function create(): Promise<void> {
      const kind = kindSel.value as PromptKind;
      const role = roleIn.value.trim();
      const binding = kind === 'system' ? (role ? `role:${role}` : null) : kind === 'agents_md' && sharedIn.checked ? 'plaza' : null;
      const body: PromptCreate = {
        id: idIn.value.trim() || undefined,
        kind,
        title: titleIn.value.trim(),
        description: descIn.value.trim(),
        tags: tagsIn.value.split(',').map((t) => t.trim()).filter(Boolean),
        binding,
        body: bodyIn.value,
      };
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
      'form.prompts-form',
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
      h('div.prompts-actions', null, button('Cancel', () => closeDialog?.(), { kind: 'ghost' }), submit),
    );
    closeDialog = modal('New prompt', form);
  }

  async function load(): Promise<void> {
    const mine = ++seq;
    status = 'loading';
    renderList();
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
    renderList();
  }

  const toolbar = h(
    'div.prompts-toolbar',
    null,
    h('div.prompts-row', null, chipBox, h('label.prompts-toggle', null, archivedIn, ' Show archived')),
    h('div.prompts-row', null, search, countLine),
  );
  replace(
    root,
    h(
      'section.prompts-view',
      null,
      h('header.prompts-head', null, h('div', null, h('h1', null, 'Prompts'), h('p.prompts-intro', null, INTRO)), newButton),
      toolbar,
      listBox,
    ),
  );
  renderChips();
  renderList();
  void load();

  return () => {
    alive = false;
    closeDialog?.();
    closeDialog = null;
  };
}
