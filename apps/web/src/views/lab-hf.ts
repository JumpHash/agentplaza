// The Hugging Face panel on the Models tab (contract: docs/design/foundry.md, "Hugging Face"): the
// owner's token (its state only, never its value), a search with filters and sorting, a detail
// drawer per model (files, sizes against the cap, what it can be imported as), and the import queue
// with live progress frames, cancel and resume. Every route is owner-only. Terms are never accepted
// here: a gated model links to its page on huggingface.co.

import '../styles/lab-hf.css';
import { on } from '../store';
import { button, input, select } from '../ui/components';
import { h } from '../ui/dom';
import { errorText, fmtNumber, fmtSize, fmtTime, labRequest, pollEvery } from './lab-common';

const POLL_MS = 5_000;
const GIB = 1024 ** 3;
const LIBRARIES = ['any', 'onnx', 'safetensors', 'transformers', 'gguf'];
const SORTS: { value: string; label: string }[] = [
  { value: 'downloads', label: 'Downloads' },
  { value: 'likes', label: 'Likes' },
  { value: 'lastModified', label: 'Updated' },
  { value: 'trending', label: 'Trending' },
];
/** The default file sets, as the contract names them (the hub checks the real files at import). */
const BASE_FILES = ['config.json', 'tokenizer.json', 'tokenizer_config.json', 'vocab.json', 'vocab.txt', 'merges.txt', 'special_tokens_map.json', 'sentencepiece.bpe.model', 'generation_config.json', 'pytorch_model.bin'];
const BASE_SUFFIXES = ['.safetensors', '.safetensors.index.json'];
const SERVABLE_FILES = ['config.json', 'model_card.json', 'onnx/model.onnx', 'model.onnx', 'tokenizer.json'];

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
const STATE_TONE: Record<string, string> = { queued: 'muted', downloading: 'info', verifying: 'info', registering: 'info', done: 'ok', failed: 'danger', cancelled: 'muted', interrupted: 'warn' };

/** A model id as a path: its slash stays a slash, every segment encoded. */
const modelPath = (id: string): string => id.split('/').map(encodeURIComponent).join('/');

/** Whether a gated field is set: the hub says `false`, or a gate kind such as `auto` or `manual`. */
export const isGated = (gated: boolean | string): boolean => gated !== false && gated !== '' && gated !== null && gated !== undefined;

/** The files an import of this kind takes by default (the contract's default sets). */
export function defaultFiles(files: HfFile[], as: 'base' | 'servable'): HfFile[] {
  if (as === 'servable') return files.filter((f) => SERVABLE_FILES.includes(f.path));
  return files.filter((f) => BASE_FILES.includes(f.path) || BASE_SUFFIXES.some((s) => f.path.endsWith(s)));
}

const sum = (files: HfFile[]): number => files.reduce((n, f) => n + f.size, 0);

/** A date from the hub as `2026-10-13 14:05`, or a dash. */
const dateOf = (iso: string | null): string => (iso ? fmtTime(Date.parse(iso)) : '—');

/** A value with a label above it, as the Lab's definition lists. */
function kv(pairs: [string, Node | string][]): HTMLElement {
  return h('dl.lab-kv', null, pairs.flatMap(([label, value]) => [h('dt', null, label), h('dd', null, value)]));
}

/** An external link that opens in a new tab and carries no referrer. */
const extLink = (url: string, text: string): HTMLElement => h('a', { href: url, target: '_blank', rel: 'noopener noreferrer' }, text);

/** A model id as a button that opens its drawer. */
const idButton = (id: string, open: (id: string) => void): HTMLElement => {
  const b = h('button.lab-linkbtn', { type: 'button' }, id);
  b.addEventListener('click', () => open(id));
  return b;
};

/** The state badge of an import, as the models list's badges. */
const stateChip = (state: string): HTMLElement => h('span.lab-badge', { class: STATE_TONE[state] ?? 'muted' }, state);

/** The panel: returns its element and the function that stops it. `changed` runs when a registration may have changed the registry. */
export function hfPanel(changed: () => void): { el: HTMLElement; stop: () => void } {
  let status: HfStatus | null = null;
  let statusError = '';
  let tokenError = '';
  let tokenBusy = false;
  let query = { search: '', task: '', library: 'any', sort: 'downloads' };
  let items: HfItem[] = [];
  let next: string | null = null;
  let searchError = '';
  let searched = false;
  let sortKey: SortKey = 'downloads';
  let sortAsc = false;
  let detail: HfDetail | null = null;
  let detailError = '';
  let importError = '';
  let imports: ImportRecord[] = [];
  let importsError = '';

  const el = h('div.lab-hf');

  const draw = (): void => {
    el.replaceChildren(...[tokenBlock(), searchBlock(), detail ? drawer(detail) : null, importsBlock()].filter((n): n is HTMLElement => n !== null));
  };

  // ---- Token -------------------------------------------------------------

  function tokenBlock(): HTMLElement {
    const set = status?.hf_token_set === true;
    const value = input({ type: 'password', autocomplete: 'new-password', spellcheck: false, placeholder: 'Token value' });
    const save = button('Save token', () => {
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
    }, { kind: 'primary', size: 'sm', disabled: tokenBusy });
    const remove = set
      ? button('Remove token', () => {
          labRequest<unknown>('DELETE', '/v1/foundry/hf/token')
            .then(() => refreshStatus())
            .catch((err: unknown) => {
              tokenError = errorText(err);
              draw();
            });
        }, { kind: 'danger', size: 'sm' })
      : null;
    return h(
      'div.lab-hf-block',
      null,
      h('h4.lab-section-title', null, 'Hugging Face token'),
      h(
        'div.lab-row',
        null,
        h('span.lab-badge', { class: status === null ? 'muted' : set ? 'ok' : 'muted' }, status === null ? 'Token state unknown' : set ? 'Token set' : 'Token not set'),
        status
          ? h('span.lab-muted', null, `Max import ${fmtNumber(status.hf_max_gb)} GB · free disk ${status.free_bytes === null ? 'unknown' : fmtSize(status.free_bytes)}`)
          : h('span.lab-muted', null, statusError || 'Reading the token state'),
      ),
      h(
        'div.lab-row',
        null,
        value,
        save,
        remove,
      ),
      h('p.lab-muted', null, 'The token is stored on this machine and never shown back. Create one at ', extLink('https://huggingface.co/settings/tokens', 'huggingface.co/settings/tokens'), '.'),
      tokenError ? h('p.lab-error', { role: 'alert' }, tokenError) : null,
    );
  }

  // ---- Search ------------------------------------------------------------

  function searchBlock(): HTMLElement {
    const search = input({ placeholder: 'Search models', value: query.search, spellcheck: false });
    const task = input({ placeholder: 'Task, e.g. text-classification', value: query.task, spellcheck: false });
    const library = select(LIBRARIES.map((l) => ({ value: l, label: l === 'any' ? 'Any library' : l })), query.library);
    const sort = select(SORTS, query.sort);
    const run = (): void => {
      query = { search: search.value.trim(), task: task.value.trim(), library: library.value, sort: sort.value };
      items = [];
      next = null;
      void loadPage(true);
    };
    const form = h(
      'form.lab-row',
      { onsubmit: (e: Event) => { e.preventDefault(); run(); } },
      search,
      task,
      library,
      sort,
      button('Search', run, { kind: 'primary', size: 'sm', icon: 'search' }),
    );
    return h('div.lab-hf-block', null, h('h4.lab-section-title', null, 'Search'), form, resultsTable(), searchError ? h('p.lab-error', { role: 'alert' }, searchError) : null);
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

  function sortHead(label: string, key: SortKey): HTMLElement {
    const mark = sortKey === key ? (sortAsc ? ' ▲' : ' ▼') : '';
    const b = h('button.lab-linkbtn', { type: 'button' }, label + mark);
    b.addEventListener('click', () => {
      sortAsc = sortKey === key ? !sortAsc : false;
      sortKey = key;
      draw();
    });
    return h('th', null, b);
  }

  function resultsTable(): HTMLElement {
    if (!searched) return h('p.lab-muted', null, 'Search the Hub, or leave the box empty for the most downloaded models.');
    if (items.length === 0) return h('p.lab-muted', null, 'No models match.');
    const sorted = items.slice().sort((a, b) => {
      const x = sortValue(a, sortKey);
      const y = sortValue(b, sortKey);
      const order = x < y ? -1 : x > y ? 1 : 0;
      return sortAsc ? order : -order;
    });
    const rows = sorted.map((m) =>
      h(
        'tr',
        { 'data-link': m.id },
        h('td', null, idButton(m.id, openDetail), h('div.lab-muted', null, m.author)),
        h('td', null, m.task ?? '—'),
        h('td', null, m.library ?? '—'),
        h('td.num', null, fmtNumber(m.downloads)),
        h('td.num', null, fmtNumber(m.likes)),
        h('td', null, dateOf(m.last_modified)),
        h('td', null, m.license ?? '—'),
        h('td', null, isGated(m.gated) ? h('span.lab-badge', { class: 'warn' }, 'gated') : h('span.lab-muted', null, 'open')),
      ),
    );
    return h(
      'div.lab-stack-tight',
      null,
      h(
        'div.lab-table-wrap',
        null,
        h(
          'table.lab-table.lab-hf-table',
          null,
          h(
            'thead',
            null,
            h('tr', null, h('th', null, 'Model'), h('th', null, 'Task'), h('th', null, 'Library'), sortHead('Downloads', 'downloads'), sortHead('Likes', 'likes'), sortHead('Updated', 'updated'), sortHead('License', 'license'), sortHead('Access', 'gated')),
          ),
          h('tbody', null, rows),
        ),
      ),
      next ? button('Load more', () => void loadPage(false), { size: 'sm' }) : null,
    );
  }

  async function loadPage(first: boolean): Promise<void> {
    const params = new URLSearchParams({ sort: query.sort, limit: '30' });
    if (query.search) params.set('search', query.search);
    if (query.task) params.set('task', query.task);
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

  // ---- Drawer ------------------------------------------------------------

  function openDetail(id: string): void {
    detail = null;
    detailError = '';
    importError = '';
    labRequest<HfDetail>('GET', `/v1/foundry/hf/models/${modelPath(id)}`)
      .then((d) => {
        detail = d;
        draw();
      })
      .catch((err: unknown) => {
        detailError = errorText(err);
        draw();
      });
  }

  function drawer(d: HfDetail): HTMLElement {
    const close = button('', () => {
      detail = null;
      draw();
    }, { kind: 'ghost', icon: 'x', title: 'Close' });
    const capGb = status?.hf_max_gb ?? 0;
    const cap = capGb * GIB;
    const baseSize = sum(defaultFiles(d.files, 'base'));
    const servableSize = sum(defaultFiles(d.files, 'servable'));
    const gated = isGated(d.gated);
    const sizes = [
      ['Base set', fmtSize(baseSize), baseSize > cap],
      ['Servable set', fmtSize(servableSize), servableSize > cap],
    ] as const;
    const fileRows = d.files.map((f) => [f.path, fmtSize(f.size), f.lfs ? 'LFS' : 'git']);
    return h(
      'section.lab-card.lab-hf-drawer',
      null,
      h(
        'header.lab-card-head',
        null,
        h('h3', null, extLink(d.url, d.id)),
        close,
      ),
      h(
        'div.lab-card-body',
        null,
        h('div.lab-stack-tight', null,
          gated
            ? h(
                'p.lab-error',
                null,
                'Gated: this model needs its terms accepted on its Hub page. Accept them there yourself; this console never accepts terms for you. ',
                extLink(d.url, 'Open its page on huggingface.co'),
              )
            : null,
          kv([
            ['Author', d.author],
            ['Revision', h('span.lab-mono', null, d.sha.slice(0, 12))],
            ['Task', d.task ?? '—'],
            ['Library', d.library ?? '—'],
            ['License', d.license ?? '—'],
            ['Downloads', fmtNumber(d.downloads)],
            ['Likes', fmtNumber(d.likes)],
            ['Updated', dateOf(d.last_modified)],
            ['Servable', d.importable.servable ? 'yes' : `no: ${d.importable.servable_reason ?? 'not a servable model'}`],
            ['Base', d.importable.base ? 'yes' : `no: ${d.importable.base_reason ?? 'not a base model'}`],
          ]),
          d.summary ? h('p.lab-note', null, d.summary) : null,
          h('h4.lab-section-title', null, 'Default selections against the cap'),
          h('div.lab-row', null, ...sizes.map(([label, size, over]) =>
            h('span', { class: over ? 'lab-error' : 'lab-mono' }, `${label}: ${size}${over ? ' (over the cap)' : ''}`),
          )),
          h('span.lab-muted', null, `Cap: ${fmtNumber(capGb)} GB`),
          h('h4.lab-section-title', null, `Files (${fmtNumber(d.files.length)})`),
          d.files.length === 0 ? h('p.lab-muted', null, 'No files listed.') : dataTable(['File', 'Size', 'Storage'], fileRows),
          h('div.lab-row', null,
            button('Import as base', () => startImport(d, 'base'), { kind: 'primary', size: 'sm', icon: 'plus', disabled: busyImport() }),
            button('Import as servable', () => startImport(d, 'servable'), { size: 'sm', icon: 'plus', disabled: busyImport() }),
          ),
          importError ? h('p.lab-error', { role: 'alert' }, importError) : null,
          detailError ? h('p.lab-error', { role: 'alert' }, detailError) : null,
        ),
      ),
    );
  }

  const busyImport = (): boolean => false;

  function startImport(d: HfDetail, as: 'base' | 'servable'): void {
    importError = '';
    labRequest<{ import: ImportRecord }>('POST', '/v1/foundry/hf/import', { id: d.id, revision: d.sha, as })
      .then(() => refreshImports())
      .catch((err: unknown) => {
        importError = errorText(err);
        draw();
      });
  }

  // ---- Imports -----------------------------------------------------------

  function importsBlock(): HTMLElement {
    const head = h('h4.lab-section-title', null, `Imports (${fmtNumber(imports.length)})`);
    if (imports.length === 0) {
      return h('div.lab-hf-block', null, head, h('p.lab-muted', null, 'No imports yet.'), importsError ? h('p.lab-error', { role: 'alert' }, importsError) : null);
    }
    const rows = imports.map((r) => {
      const percent = r.total > 0 ? r.bytes / r.total : 0;
      const bar = h('div.lab-meter.signal', { role: 'meter', 'aria-valuemin': 0, 'aria-valuemax': 1, 'aria-valuenow': percent }, h('span', { style: `width:${Math.min(1, percent) * 100}%` }));
      const action = ACTIVE.has(r.state)
        ? button('Cancel', () => cancelImport(r.n), { size: 'sm', kind: 'danger' })
        : button('Remove', () => cancelImport(r.n), { size: 'sm' });
      const resume = r.state === 'interrupted' ? button('Resume', () => resumeImport(r), { size: 'sm', kind: 'primary' }) : null;
      const model = r.model_id ? h('a', { href: `#/lab/models/${encodeURIComponent(r.model_id)}` }, r.model_id) : h('span.lab-muted', null, r.id);
      return h(
        'tr',
        null,
        h('td.num', null, `#${r.n}`),
        h('td', null, model, h('div.lab-muted', null, r.id)),
        h('td.lab-nowrap', null, r.as),
        h('td', null, stateChip(r.state), r.reason ? h('div.lab-muted', null, r.reason) : null, r.error ? h('div.lab-error', null, r.error) : null),
        h('td', null, bar, h('span.lab-mono', null, `${fmtSize(r.bytes)} of ${fmtSize(r.total)}`)),
        h('td', null, fmtTime(r.updated_ms)),
        h('td', null, h('div.lab-row', null, resume, action)),
      );
    });
    return h(
      'div.lab-hf-block',
      null,
      head,
      h('div.lab-table-wrap', null, h('table.lab-table', null,
        h('thead', null, h('tr', null, ['No.', 'Model', 'As', 'State', 'Progress', 'Updated', ''].map((t) => h('th', null, t)))),
        h('tbody', null, rows),
      )),
      importsError ? h('p.lab-error', { role: 'alert' }, importsError) : null,
    );
  }

  function dataTable(heads: string[], rows: string[][]): HTMLElement {
    return h('div.lab-table-wrap', null, h('table.lab-table', null,
      h('thead', null, h('tr', null, heads.map((t) => h('th', null, t)))),
      h('tbody', null, rows.map((cells) => h('tr', null, cells.map((c) => h('td.lab-wrap', null, c))))),
    ));
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

  let known = new Map<number, string>();
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
  // The first draw shows the section before the first answers arrive.
  draw();
  return {
    el,
    stop: () => {
      unsubscribe();
      stopPolling();
    },
  };
}
