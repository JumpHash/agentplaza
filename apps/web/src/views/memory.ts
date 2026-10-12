// Agent memory: the agent's notes as a live cartoon graph (left) and a reader (right).
// Framework-free. DOM via h(); text only ever reaches the page as textContent.

import '../styles/memory.css';
import { api, type MemoryDoc, type MemoryNote } from '../api';
import { on } from '../store';
import { ago, badge, empty, input, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import { renderMarkdown } from '../ui/markdown';
import { createMemoryGraph, type MemoryGraph } from './memory-graph';

type Kind = MemoryNote['kind'];
type Resolve = (ref: string) => string | null;

const FALLBACK_REFRESH_MS = 30_000;
const KIND_TONE: Record<Kind, Tone> = { note: 'info', person: 'accent', journal: 'warn' };


/** Mirrors the server's slug rule: lowercase, runs of non [a-z0-9] become '-', trimmed, at most 60. */
function slugify(text: string): string {
  return text.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 60).replace(/-+$/, '');
}

export function mountMemory(root: HTMLElement, cid: number): () => void {
  let alive = true;
  let loaded = false;
  let notes: MemoryNote[] = [];
  let selected: string | null = null;
  let current: MemoryDoc | null = null;
  let readSeq = 0;
  let graph: MemoryGraph | null = null;

  const count = h('span.mem-count');
  const search = input({ type: 'search', placeholder: 'Find a note…', 'aria-label': 'Find a note' });
  const canvas = h('div.mem-graph');
  const reader = h('article.mem-reader', { 'aria-live': 'polite' });
  const body = h('div.mem-body', null, h('p.faint', null, 'Loading memory…'));

  search.addEventListener('input', () => graph?.filter(search.value));
  search.addEventListener('keydown', (e) => {
    if (e.key !== 'Enter') return;
    const q = search.value.trim().toLowerCase();
    const hit = notes.find((n) => n.title.toLowerCase().includes(q));
    if (hit) open(hit.slug, true);
  });

  const resolveRef: Resolve = (ref) => {
    const want = slugify(ref);
    const hit = notes.find((n) => n.slug === ref || n.slug === want || slugify(n.title) === want);
    return hit ? hit.slug : null;
  };
  const titleOf = (ref: string): string => notes.find((n) => n.slug === resolveRef(ref))?.title ?? ref;
  const wikiChip = (title: string): HTMLElement => {
    const target = resolveRef(title);
    return target
      ? h('button.chip.link', { type: 'button', onclick: () => open(target, true) }, title)
      : h('span.chip.missing', { title: 'No such note yet' }, title);
  };

  const renderReader = (doc: MemoryDoc): void => {
    const prov = doc.provenance;
    const stamp = [`Updated ${ago(doc.updated_ms)}`];
    if (prov.task) stamp.push(`task ${prov.task}`);
    if (prov.work !== null) stamp.push(`work #${prov.work}`);
    replace(
      reader,
      h(
        'header.mem-title',
        null,
        h('div.mem-meta', null, badge(doc.kind, KIND_TONE[doc.kind]), doc.tags.map((t) => h('span.chip', { 'data-key': `tag-${t}` }, `#${t}`))),
        h('h1', null, doc.title),
      ),
      h('p.mem-stamp', { title: new Date(doc.updated_ms).toLocaleString() }, stamp.join(' · ')),
      h('div.mem-prose', null, renderMarkdown(doc.body, { wikiLink: wikiChip })),
      doc.backlinks.length
        ? h(
            'footer.mem-linked',
            null,
            h('h3', null, 'Linked from'),
            h(
              'div.mem-chips',
              null,
              doc.backlinks.map((b) => {
                const slug = resolveRef(b);
                return slug ? h('button.chip.link', { type: 'button', onclick: () => open(slug, true), 'data-key': `backlink-${b}` }, titleOf(b)) : h('span.chip', { 'data-key': `backlink-${b}` }, b);
              }),
            ),
          )
        : null,
    );
  };

  /** Opens a note in the reader and highlights it; `fly` also moves the camera to it. */
  const open = (slug: string, fly = false): void => {
    selected = slug;
    graph?.select(slug);
    if (fly) graph?.focus(slug);
    const seq = ++readSeq;
    api.memoryNote(cid, slug).then(
      (doc) => {
        if (!alive || seq !== readSeq) return;
        current = doc;
        renderReader(doc);
        graph?.select(slug, new Set(doc.backlinks.map((b) => resolveRef(b)).filter((s): s is string => s !== null)));
      },
      (err: unknown) => {
        if (!alive || seq !== readSeq) return;
        replace(reader, empty('alert', 'Could not open this note', err instanceof Error ? err.message : undefined));
      },
    );
  };

  const refresh = async (changed = new Set<string>()): Promise<void> => {
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
      replace(body, empty('memory', 'No memories yet', 'Notes the agent writes appear here, linked like a little brain.'));
      return;
    }
    if (!graph) {
      replace(body, canvas, reader);
      graph = createMemoryGraph(canvas, (slug) => open(slug));
    }
    graph.update(list, changed);
    const newest = list.reduce((best, n) => (n.updated_ms > best.updated_ms ? n : best));
    const target = list.find((n) => n.slug === selected) ?? newest;
    const stale = !current || current.slug !== target.slug || current.updated_ms !== target.updated_ms;
    if (stale) open(target.slug);
    else graph.select(target.slug);
  };

  // Live: a note written by this agent refreshes at once (debounced); polling is only a fallback.
  let pendingChanged = new Set<string>();
  let debounce = 0;
  const off = on('memory', (raw) => {
    const d = raw as { cid: number; slug: string };
    if (d.cid !== cid) return;
    pendingChanged.add(d.slug);
    window.clearTimeout(debounce);
    debounce = window.setTimeout(() => {
      const changed = pendingChanged;
      pendingChanged = new Set();
      void refresh(changed);
    }, 250);
  });

  replace(root, h('div.mem-view', null, h('header.mem-head', null, count, h('span.mem-hint', null, 'Scroll to zoom · drag to pan · drag a note to move it'), search), body));
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
