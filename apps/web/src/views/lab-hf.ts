// The Hugging Face panel on the Models tab, in the prototype's layout (contract: docs/design/foundry.md,
// "Hugging Face"): three cards, the owner's token (its state only, never its value), a search with
// filters and sorting, and the import queue with live progress frames, cancel and resume. A model
// opens a wide dialog with its files, its sizes against the cap and what it can be imported as. Every
// route is owner-only. Terms are never accepted here: a gated model links to its page on huggingface.co.

import { on } from '../store';
import { badge, card, modal, select, table as componentTable, type Column, type Tone } from '../ui/components';
import { h } from '../ui/dom';
import { agoText, errorText, fmtNumber, fmtSize, labButton, labRequest, pollEvery } from './lab-common';

const POLL_MS = 5_000;
const GIB = 1024 ** 3;
const TASKS: [string, string][] = [
  ['any', 'Any task'],
  ['text-generation', 'text-generation'],
  ['text-classification', 'text-classification'],
  ['feature-extraction', 'feature-extraction'],
];
const LIBRARIES: [string, string][] = [
  ['any', 'Any library'],
  ['onnx', 'onnx'],
  ['safetensors', 'safetensors'],
  ['transformers', 'transformers'],
  ['gguf', 'gguf'],
];
const SORTS: { value: string; label: string }[] = [
  { value: 'downloads', label: 'Downloads' },
  { value: 'likes', label: 'Likes' },
  { value: 'lastModified', label: 'Updated' },
  { value: 'trending', label: 'Trending' },
];
export interface HfStatus { hf_token_set: boolean; hf_max_gb: number; free_bytes: number | null; downloads_dir: string }
export interface HfItem {
  id: string; author: string; task: string | null; library: string | null; tags: string[]; downloads: number; likes: number;
  last_modified: string | null; gated: boolean | string; private: boolean; license: string | null;
}
interface HfPage { items: HfItem[]; next_cursor: string | null }
export interface HfFile { path: string; size: number; sha256: string | null; lfs: boolean }
interface HfDetail {
  id: string; author: string; sha: string; url: string; task: string | null; library: string | null; tags: string[];
  license: string | null; gated: boolean | string; private: boolean; downloads: number; likes: number; last_modified: string | null;
  summary: string; files: HfFile[];
  importable: { servable: boolean; servable_reason: string | null; base: boolean; base_reason: string | null };
  /** The default file sets the hub imports for each kind, by path. */
  import_files: { base: string[]; servable: string[] };
}
interface ImportFile { path: string; size: number; sha256: string | null; bytes: number; state: string }
export interface ImportRecord {
  n: number; id: string; revision: string; sha: string; as: 'base' | 'servable'; state: string; dir: string; files: ImportFile[];
  bytes: number; total: number; model_id: string | null; servable: boolean | null; reason: string | null; error: string | null;
  created_ms: number; updated_ms: number;
}
/** A `foundry` frame of kind `hf_import`: byte progress for one import (and file), or its state change. */
interface HfFrame { kind: 'hf_import'; import: number; id: string; file: string; bytes: number; total: number; state: string }

type SortKey = 'downloads' | 'likes' | 'updated' | 'license' | 'gated';

const ACTIVE = new Set(['queued', 'downloading', 'verifying', 'registering']);
const IMPORT_TONE: Record<string, Tone> = { queued: 'muted', downloading: 'info', verifying: 'info', registering: 'info', done: 'ok', failed: 'danger', cancelled: 'muted', interrupted: 'warn' };

/** A model id as a path: its slash stays a slash, every segment encoded. */
const modelPath = (id: string): string => id.split('/').map(encodeURIComponent).join('/');

/** Whether a gated field is set: the hub says `false`, or a gate kind such as `auto` or `manual`. */
export const isGated = (gated: boolean | string): boolean => gated !== false && gated !== '' && gated !== null && gated !== undefined;

/** The files an import takes by default: the hub's default set for that kind (`import_files`), out of the repo's files. */
export function defaultFiles(files: HfFile[], paths: string[]): HfFile[] {
  return files.filter((f) => paths.includes(f.path));
}

const sum = (files: HfFile[]): number => files.reduce((n, f) => n + f.size, 0);

/** A date from the hub, as the prototype's relative time, or a dash. */
const dateOf = (iso: string | null): string => (iso ? agoText(Date.parse(iso)) : '—');

/** A definition list (the prototype's `h.kv`). */
function kv(pairs: [string, Node | string][]): HTMLElement {
  return h(
    'dl.kv',
    null,
    pairs.map(([label, value]) => h('div', null, h('dt', null, label), h('dd', null, value))),
  );
}

/** The panel: returns its element and the function that stops it. `changed` runs when a registration may have changed the registry. */
export function hfPanel(changed: () => void): { el: HTMLElement; stop: () => void } {
  let status: HfStatus | null = null;
  let statusError = '';
  let tokenError = '';
  let tokenBusy = false;
  let query = { search: '', task: 'any', library: 'any', sort: 'downloads' };
  let items: HfItem[] = [];
  let next: string | null = null;
  let searchError = '';
  let searched = false;
  let sortKey: SortKey | null = null;
  let sortAsc = false;
  let detailClose: (() => void) | null = null;
  let importError = '';
  let imports: ImportRecord[] = [];
  let importsError = '';
  let known = new Map<number, string>();

  const el = h('div.lab2-form');

  const draw = (): void => {
    el.replaceChildren(tokenCard(), searchCard(), importsCard());
  };

  // ---- Token -------------------------------------------------------------

  function tokenCard(): HTMLElement {
    const set = status?.hf_token_set === true;
    const value = h('input.input', { type: 'password', id: 'lab-hf-token', autocomplete: 'off', placeholder: 'Access token', 'aria-label': 'Access token' });
    value.style.cssText = 'flex:1 1 220px;min-width:0';
    const save = labButton('Save', () => {
      const v = value.value.trim();
      if (!v) {
        tokenError = 'Paste the token first.';
        draw();
        return;
      }
      tokenBusy = true;
      tokenError = '';
      labRequest<unknown>('PUT', '/v1/foundry/hf/token', { value: v })
        .then(() => {
          value.value = '';
          return refreshStatus();
        })
        .catch((err: unknown) => {
          tokenError = errorText(err);
        })
        .finally(() => {
          tokenBusy = false;
          draw();
        });
    }, { kind: 'primary', sm: true });
    save.disabled = tokenBusy;
    const remove = labButton('Remove', () => {
      labRequest<unknown>('DELETE', '/v1/foundry/hf/token')
        .then(() => refreshStatus())
        .catch((err: unknown) => {
          tokenError = errorText(err);
          draw();
        });
    }, { kind: 'danger', sm: true });
    remove.disabled = !set;
    const state = status === null ? badge('token unknown', 'muted') : set ? badge('token set', 'ok') : badge('token not set', 'warn');
    const facts = status
      ? `${fmtNumber(status.hf_max_gb)} GB max per import · ${status.free_bytes === null ? 'unknown' : fmtSize(status.free_bytes)} free on disk`
      : statusError || 'Reading the token state';
    const body = h(
      'div',
      null,
      h('div.lab2-row', { style: 'justify-content:flex-start' }, state, h('span.lab2-note', null, facts)),
      h('div.lab2-row', { style: 'justify-content:flex-start;margin-top:8px' }, value, save, remove),
      h('div.lab2-err', { id: 'lab-hf-token-note', role: 'alert' }, tokenError),
    );
    return card('Hugging Face token', body);
  }

  // ---- Search ------------------------------------------------------------

  function searchCard(): HTMLElement {
    const search = h('input.input', { type: 'search', id: 'lab-hf-q', value: query.search, placeholder: 'Search models or authors', 'aria-label': 'Search models', spellcheck: 'false' });
    search.style.cssText = 'flex:1 1 220px;min-width:0';
    const task = select(TASKS.map(([value, label]) => ({ value, label })), query.task);
    task.setAttribute('aria-label', 'Task');
    const library = select(LIBRARIES.map(([value, label]) => ({ value, label })), query.library);
    library.setAttribute('aria-label', 'Library');
    const sort = select(SORTS, query.sort);
    sort.setAttribute('aria-label', 'Sort');
    const run = (): void => {
      query = { search: search.value.trim(), task: task.value, library: library.value, sort: sort.value };
      items = [];
      next = null;
      sortKey = null;
      void loadPage(true);
    };
    search.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') run();
    });
    const bar = h('div.lab2-row', { style: 'justify-content:flex-start' }, search, task, library, sort, labButton('Search', run, { kind: 'primary', icon: 'search' }));
    const body = h('div.lab2-form', null, bar, resultsBlock(), searchError ? h('p.lab2-err', { role: 'alert' }, searchError) : null);
    return card('Search Hugging Face', body);
  }

  function sortValue(item: HfItem, key: SortKey): number | string {
    switch (key) {
      case 'downloads': return item.downloads;
      case 'likes': return item.likes;
      case 'updated': return Date.parse(item.last_modified ?? '') || 0;
      case 'license': return item.license ?? '';
      case 'gated': return isGated(item.gated) ? 1 : 0;
    }
  }

  const SORT_COLS: [string, string, SortKey | null][] = [
    ['id', 'Model', null],
    ['task', 'Task', null],
    ['library', 'Library', null],
    ['downloads', 'Downloads', 'downloads'],
    ['likes', 'Likes', 'likes'],
    ['last_modified', 'Updated', 'updated'],
    ['license', 'License', 'license'],
    ['gated', 'Access', 'gated'],
  ];

  function resultsBlock(): HTMLElement {
    if (!searched) return h('p.lab2-note', null, 'Search the Hub, or leave the box empty for the most downloaded models.');
    if (items.length === 0) return h('p.lab2-note', null, 'No models match.');
    const sorted = sortKey
      ? items.slice().sort((a, b) => {
          const x = sortValue(a, sortKey as SortKey);
          const y = sortValue(b, sortKey as SortKey);
          const order = x < y ? -1 : x > y ? 1 : 0;
          return sortAsc ? order : -order;
        })
      : items;
    const head = h(
      'tr',
      null,
      SORT_COLS.map(([key, label, sortable]) => {
        if (!sortable) return h('th', null, label);
        const mark = sortKey === sortable ? (sortAsc ? ' ↑' : ' ↓') : '';
        const b = h('button.lab2-sortbtn', { type: 'button', 'aria-sort': sortKey === sortable ? (sortAsc ? 'ascending' : 'descending') : 'none' }, label + mark);
        b.addEventListener('click', () => {
          if (sortKey === sortable) sortAsc = !sortAsc;
          else {
            sortKey = sortable;
            sortAsc = ['id', 'task', 'library', 'license'].includes(key);
          }
          draw();
        });
        return h('th', null, b);
      }),
    );
    const rows = sorted.map((m) => {
      const tr = h(
        'tr',
        { class: 'clickable' },
        h('td', null, h('strong', null, m.id), h('div.lab2-note', null, m.author)),
        h('td', null, m.task ?? '—'),
        h('td', null, m.library ?? '—'),
        h('td', { class: 'num' }, fmtNumber(m.downloads)),
        h('td', { class: 'num' }, fmtNumber(m.likes)),
        h('td', null, dateOf(m.last_modified)),
        h('td', null, m.license ?? '—'),
        h('td', null, isGated(m.gated) ? badge('gated', 'warn') : badge('open', 'muted')),
      );
      tr.addEventListener('click', () => openDetail(m.id));
      return tr;
    });
    return h(
      'div.lab2-form',
      null,
      h('div.tbl-wrap', null, h('table.tbl', null, h('thead', null, head), h('tbody', null, rows))),
      next ? h('div.lab2-row', { style: 'justify-content:center' }, labButton('Load more', () => void loadPage(false), { kind: 'ghost' })) : null,
    );
  }

  async function loadPage(first: boolean): Promise<void> {
    const params = new URLSearchParams({ sort: query.sort, limit: '30' });
    if (query.search) params.set('search', query.search);
    if (query.task !== 'any') params.set('task', query.task);
    if (query.library !== 'any') params.set('library', query.library);
    if (!first && next) params.set('cursor', next);
    try {
      const page = await labRequest<HfPage>('GET', `/v1/foundry/hf/models?${params.toString()}`);
      items = first ? page.items : items.concat(page.items);
      next = page.next_cursor;
      searchError = '';
    } catch (err) {
      searchError = errorText(err);
    }
    searched = true;
    draw();
  }

  // ---- Detail dialog -----------------------------------------------------

  function openDetail(id: string): void {
    importError = '';
    labRequest<HfDetail>('GET', `/v1/foundry/hf/models/${modelPath(id)}`)
      .then((d) => showDetail(d))
      .catch((err: unknown) => {
        searchError = errorText(err);
        draw();
      });
  }

  function showDetail(d: HfDetail): void {
    detailClose?.();
    const capGb = status?.hf_max_gb ?? 0;
    const cap = capGb * GIB;
    const baseSize = sum(defaultFiles(d.files, d.import_files.base));
    const servableSize = sum(defaultFiles(d.files, d.import_files.servable));
    const gated = isGated(d.gated);
    const note = h('p.lab2-err', { role: 'alert' }, importError);
    const body = h(
      'div.lab2-form',
      null,
      gated ? h('p.lab2-err', null, 'Gated: accept the terms on huggingface.co yourself before you import.') : null,
      kv([
        ['Revision', h('span.mono', null, d.sha.slice(0, 12))],
        ['Servable', d.importable.servable ? 'Yes' : d.importable.servable_reason ?? 'No ONNX file in the repo.'],
        ['Base', d.importable.base ? 'Yes' : d.importable.base_reason ?? 'No safetensors weights in the repo.'],
        ['Default base set', h('span', { class: baseSize > cap ? 'lab2-over' : '' }, `${fmtSize(baseSize)} of ${fmtSize(cap)} cap`)],
        ['Default servable set', h('span', { class: servableSize > cap ? 'lab2-over' : '' }, `${fmtSize(servableSize)} of ${fmtSize(cap)} cap`)],
      ]),
      d.files.length === 0
        ? h('p.lab2-note', null, 'No files.')
        : h(
            'div.tbl-wrap',
            null,
            componentTable(
              [
                { label: 'File', cell: (f: HfFile) => h('span.mono', null, f.path) },
                { label: 'Size', cell: (f: HfFile) => fmtSize(f.size) },
                { label: 'Storage', cell: (f: HfFile) => (f.lfs ? 'LFS' : 'git') },
              ] as Column<HfFile>[],
              d.files,
            ),
          ),
      d.summary ? h('p.lab2-note', null, d.summary) : null,
      note,
    );
    const importAs = (as: 'base' | 'servable'): void => startImport(d, as);
    const foot = h(
      'div.lab-actions',
      null,
      labButton('Import as base', () => importAs('base'), { kind: 'ghost' }),
      labButton('Import as servable', () => importAs('servable'), { kind: 'primary' }),
    );
    detailClose = modal(d.id, h('div', null, body, foot));
  }

  function startImport(d: HfDetail, as: 'base' | 'servable'): void {
    importError = '';
    labRequest<{ import: ImportRecord }>('POST', '/v1/foundry/hf/import', { id: d.id, revision: d.sha, as })
      .then(() => {
        detailClose?.();
        detailClose = null;
        return refreshImports();
      })
      .catch((err: unknown) => {
        importError = errorText(err);
        const box = document.querySelector('.sheet .lab2-err[role="alert"]');
        if (box) box.textContent = importError;
      });
  }

  // ---- Imports -----------------------------------------------------------

  function importActions(r: ImportRecord): HTMLElement {
    const wrap = h('div.lab-row', null);
    if (r.state === 'interrupted') wrap.append(labButton('Resume', () => resumeImport(r), { kind: 'ghost', sm: true }));
    if (ACTIVE.has(r.state)) wrap.append(labButton('Cancel', () => cancelImport(r.n), { kind: 'ghost', sm: true }));
    else wrap.append(labButton('Remove', () => cancelImport(r.n), { kind: 'ghost', sm: true }));
    return wrap;
  }

  function importsCard(): HTMLElement {
    let body: Node;
    if (imports.length === 0) {
      body = h('p.lab2-note', null, 'No imports yet.');
    } else {
      body = h(
        'div.tbl-wrap',
        null,
        h(
          'table.tbl',
          null,
          h('thead', null, h('tr', null, ['No.', 'Model', 'As', 'State', 'Reason or error', 'Bytes', 'Updated', ''].map((t) => h('th', null, t)))),
          h(
            'tbody',
            null,
            imports.map((r) =>
              h(
                'tr',
                null,
                h('td', { class: 'num' }, String(r.n)),
                h('td', { class: 'mono' }, r.id),
                h('td', null, r.as),
                h('td', null, badge(r.state, IMPORT_TONE[r.state] ?? 'muted')),
                h('td', null, r.error ?? r.reason ?? '—'),
                h('td', { class: 'num' }, `${fmtSize(r.bytes)} of ${fmtSize(r.total)}`),
                h('td', null, agoText(r.updated_ms)),
                h('td', null, importActions(r)),
              ),
            ),
          ),
        ),
      );
    }
    return card('Import queue', h('div', null, body, importsError ? h('p.lab2-err', { role: 'alert' }, importsError) : null));
  }

  function cancelImport(n: number): void {
    labRequest<unknown>('DELETE', `/v1/foundry/hf/imports/${n}`)
      .then(() => refreshImports())
      .catch((err: unknown) => {
        importsError = errorText(err);
        draw();
      });
  }

  function resumeImport(r: ImportRecord): void {
    labRequest<unknown>('POST', '/v1/foundry/hf/import', { id: r.id, revision: r.revision, as: r.as })
      .then(() => refreshImports())
      .catch((err: unknown) => {
        importsError = errorText(err);
        draw();
      });
  }

  // ---- Loading and live frames ------------------------------------------

  async function refreshStatus(): Promise<void> {
    try {
      status = await labRequest<HfStatus>('GET', '/v1/foundry/hf/status');
      statusError = '';
    } catch (err) {
      statusError = errorText(err);
    }
    draw();
  }

  async function refreshImports(): Promise<void> {
    try {
      const list = await labRequest<{ imports: ImportRecord[] }>('GET', '/v1/foundry/hf/imports');
      imports = list.imports;
      importsError = '';
      // A registration may have added a model: tell the registry once per import that just finished.
      let finished = false;
      for (const r of imports) {
        const before = known.get(r.n);
        if (before !== undefined && before !== r.state && (r.state === 'done' || r.state === 'failed')) finished = true;
        known.set(r.n, r.state);
      }
      if (finished) changed();
    } catch (err) {
      importsError = errorText(err);
    }
    draw();
  }

  /** A progress frame: the import's state and bytes, and its file's. An import we do not know yet is fetched. */
  const onFrame = (raw: unknown): void => {
    const f = raw as HfFrame | null;
    if (!f || f.kind !== 'hf_import' || typeof f.import !== 'number') return;
    const r = imports.find((x) => x.n === f.import);
    if (!r) {
      void refreshImports();
      return;
    }
    const before = r.state;
    r.state = f.state;
    r.bytes = f.bytes ?? r.bytes;
    r.total = f.total ?? r.total;
    if (f.file) {
      const file = r.files.find((x) => x.path === f.file);
      if (file) file.bytes = f.bytes;
    }
    if (before !== f.state && (f.state === 'done' || f.state === 'failed')) {
      changed();
      void refreshImports();
      return;
    }
    draw();
  };

  const unsubscribe = on('foundry', onFrame);
  const stopPolling = pollEvery(POLL_MS, () => {
    void refreshStatus();
    void refreshImports();
  });
  // The first draw shows the panel before the first answers arrive.
  draw();
  return {
    el,
    stop: () => {
      unsubscribe();
      stopPolling();
      detailClose?.();
    },
  };
}
