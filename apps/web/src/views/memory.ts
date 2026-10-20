// Agent memory, as the prototype's Memory tab: a card with the note count and a find box, then Graph / List
// and the notes beside the reader (the List table with the reader under it). Framework-free. DOM via h();
// text only ever reaches the page as textContent.

import { api, type MemoryDoc, type MemoryNote } from '../api';
import { on } from '../store';
import { ago, badge, button, empty, input, table, type Column, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import { renderMarkdown } from '../ui/markdown';
import { createMemoryGraph, type MemoryGraph } from './memory-graph';

type Kind = MemoryNote['kind'];
type View = 'graph' | 'list';
/** What the body holds: the empty state, the no-match line, or one of the two views. */
type Shape = 'empty' | 'none' | View;

const FALLBACK_REFRESH_MS = 30_000;
const LIST_KEY = 'apz.memory.view';
const KIND_TONE: Record<Kind, Tone> = { note: 'info', person: 'accent', journal: 'warn' };

/** The view the user last chose; graph by default. Storage can be blocked, so it is read and written in try/catch. */
function readView(): View {
  try {
    return localStorage.getItem(LIST_KEY) === 'list' ? 'list' : 'graph';
  } catch {
    return 'graph';
  }
}

function saveView(view: View): void {
  try {
    localStorage.setItem(LIST_KEY, view);
  } catch {
    // Not kept: the choice lasts this visit only.
  }
}

/** Mirrors the server's slug rule: lowercase, runs of non [a-z0-9] become '-', trimmed, at most 60. */
function slugify(text: string): string {
  return text.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 60).replace(/-+$/, '');
}

/** The tags as grey badges, spaced as the prototype spaces them. */
function tagChips(tags: string[]): Node[] {
  return tags.flatMap((t, i) => {
    const chip = h('span.badge-t.t-muted', null, `#${t}`);
    return i ? [document.createTextNode(' '), chip] : [chip];
  });
}

export function mountMemory(root: HTMLElement, cid: number): () => void {
  let alive = true;
  let loaded = false;
  let notes: MemoryNote[] = [];
  let selected: string | null = null;
  let current: MemoryDoc | null = null;
  let readSeq = 0;
  let graph: MemoryGraph | null = null;
  let view: View = readView();
  let shape: Shape | null = null; // what the body currently holds

  const count = h('span.muted');
  const search = input({ type: 'search', placeholder: 'Find a note…', 'aria-label': 'Find a note' });
  const graphBtn = button('Graph', () => setView('graph'), { kind: 'ghost', size: 'sm' });
  const listBtn = button('List', () => setView('list'), { kind: 'ghost', size: 'sm' });
  const toolbar = h('div.cluster.mem-tools', { role: 'group', 'aria-label': 'View' }, graphBtn, listBtn);
  const canvas = h('div');
  const listBox = h('div');
  const reader = h('article.mem-read', { 'aria-live': 'polite' }, h('p.muted', null, 'Pick a note to read it.'));
  const body = h('div.ag-mem-body', null, h('p.muted', null, 'Loading memory…'));

  /** The notes the find box lets through, newest first; the first of them is what Enter opens. */
  const matches = (): MemoryNote[] => {
    const q = search.value.trim().toLowerCase();
    return notes.filter((n) => n.title.toLowerCase().includes(q)).sort((a, b) => b.updated_ms - a.updated_ms);
  };

  search.addEventListener('input', () => paint());
  search.addEventListener('keydown', (e) => {
    if (e.key !== 'Enter' || !search.value.trim()) return;
    e.preventDefault();
    const first = matches()[0];
    if (first) open(first.slug);
  });

  /** The note a `[[ref]]` names, by slug or by title. */
  const findNote = (ref: string): MemoryNote | null => {
    const want = slugify(ref);
    return notes.find((n) => n.slug === ref || n.slug === want || slugify(n.title) === want) ?? null;
  };
  const wikiChip = (title: string): HTMLElement => {
    const target = findNote(title);
    return target
      ? h('button.chip.t-accent', { type: 'button', onclick: () => open(target.slug) }, target.title)
      : h('span.chip.t-muted', { title: 'No note by this name yet' }, title.trim());
  };

  const renderReader = (doc: MemoryDoc): void => {
    const prov = doc.provenance;
    const provText = [prov.task ? `task ${prov.task}` : '', prov.work !== null ? `work #${prov.work}` : ''].filter(Boolean).join(' · ');
    const linked = doc.backlinks.map((b) => {
      const target = findNote(b);
      return target ? h('button.chip.t-info', { type: 'button', onclick: () => open(target.slug) }, target.title) : h('span.chip.t-muted', null, b);
    });
    reader.setAttribute('aria-label', doc.title);
    replace(
      reader,
      h('div.cluster', null, badge(doc.kind, KIND_TONE[doc.kind]), h('strong', null, doc.title)),
      h('p.muted', { title: new Date(doc.updated_ms).toLocaleString() }, `Updated ${ago(doc.updated_ms)}${provText ? ` · ${provText}` : ''}`),
      doc.tags.length ? h('div.cluster', null, tagChips(doc.tags)) : null,
      h('div.prose', null, renderMarkdown(doc.body, { wikiLink: wikiChip })),
      doc.backlinks.length ? [h('h4', null, 'Linked from'), h('div.cluster', null, linked)] : null,
    );
  };

  /** Opens a note in the reader and marks it in the graph. */
  const open = (slug: string): void => {
    selected = slug;
    graph?.select(slug);
    const seq = ++readSeq;
    api.memoryNote(cid, slug).then(
      (doc) => {
        if (!alive || seq !== readSeq) return;
        current = doc;
        renderReader(doc);
      },
      (err: unknown) => {
        if (!alive || seq !== readSeq) return;
        replace(reader, empty('alert', 'Could not open this note', err instanceof Error ? err.message : undefined));
      },
    );
  };

  const refresh = async (): Promise<void> => {
    let list: MemoryNote[];
    try {
      list = await api.memory(cid);
    } catch (err) {
      if (!alive || loaded) return;
      replace(body, empty('alert', 'Could not load memory', err instanceof Error ? err.message : undefined));
      return;
    }
    if (!alive) return;
    loaded = true;
    notes = list;
    count.textContent = `${list.length} ${list.length === 1 ? 'note' : 'notes'}`;
    if (list.length === 0) {
      graph?.destroy();
      graph = null;
      shape = 'empty';
      replace(body, empty('brain', 'No memories yet', 'Notes the agent writes appear here, linked like a little brain.'));
      return;
    }
    paint();
    const newest = list.reduce((best, n) => (n.updated_ms > best.updated_ms ? n : best));
    const target = list.find((n) => n.slug === selected) ?? newest;
    const stale = !current || current.slug !== target.slug || current.updated_ms !== target.updated_ms;
    if (stale) open(target.slug);
  };

  const listColumns: Column<MemoryNote>[] = [
    { label: 'Title', cell: (n) => n.title },
    { label: 'Kind', cell: (n) => badge(n.kind, KIND_TONE[n.kind]) },
    { label: 'Tags', cell: (n) => h('span', null, tagChips(n.tags)) },
    { label: 'Updated', cell: (n) => ago(n.updated_ms) },
  ];

  /** Draws the body for the current view and find text. The graph is kept while its shape holds, so its layout survives a refresh. */
  const paint = (): void => {
    if (!notes.length) return;
    const rows = matches();
    graphBtn.setAttribute('aria-pressed', String(view === 'graph'));
    listBtn.setAttribute('aria-pressed', String(view === 'list'));
    const want: Shape = rows.length === 0 ? 'none' : view;
    if (want !== shape) {
      shape = want;
      graph?.destroy();
      graph = null;
      if (want === 'graph') {
        graph = createMemoryGraph(canvas, (slug) => open(slug));
        replace(
          body,
          toolbar,
          h('div.mem-split', null, h('div', null, canvas, h('p.hint', null, 'Click a note to read it. Solid lines are links; dashed lines join notes that share a tag.')), reader),
        );
      } else if (want === 'list') {
        replace(body, toolbar, listBox, h('div.mem-read-under', null, reader));
      } else {
        replace(body, toolbar, h('p.muted', null, 'No notes match.'));
      }
    }
    if (graph) {
      graph.update(rows);
      graph.select(selected);
    } else if (want === 'list') {
      replace(listBox, table(listColumns, rows, (n) => open(n.slug), (n) => n.slug));
    }
  };

  const setView = (next: View): void => {
    if (next === view) return;
    view = next;
    saveView(next);
    paint();
  };

  // Live: a note written by this agent refreshes at once (debounced); polling is only a fallback.
  let debounce = 0;
  const off = on('memory', (raw) => {
    const d = raw as { cid: number; slug: string };
    if (d.cid !== cid) return;
    window.clearTimeout(debounce);
    debounce = window.setTimeout(() => void refresh(), 250);
  });

  replace(root, h('section.card', null, h('div.card-b', null, h('div.cluster', null, count, search), body)));
  void refresh();
  const timer = window.setInterval(() => void refresh(), FALLBACK_REFRESH_MS);

  return () => {
    alive = false;
    off();
    window.clearInterval(timer);
    window.clearTimeout(debounce);
    graph?.destroy();
    replace(root);
  };
}
