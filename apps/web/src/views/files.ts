// Library: the deliverables agents hand in with deliver_file. A grid of cards
// (each previewed by its type), a search and a type filter, and a detail modal
// with a large preview, the metadata, Download and Delete. The bytes live in the
// plaza's object store; the list refreshes live on the `file` frame.

import '../styles/files.css';
import { api, type Deliverable } from '../api';
import { attempt, button, empty, input, modal, tabs } from '../ui/components';
import { h, replace } from '../ui/dom';
import { on } from '../store';
import { renderMarkdown } from '../ui/markdown';
import type { Cleanup } from './index';

type Group = 'Video' | 'Image' | 'Audio' | 'Documents' | 'Other';
const GROUPS: Group[] = ['Video', 'Image', 'Audio', 'Documents', 'Other'];
const ALL = 'All';

/** The intro line under the tabs, per tab. */
const INTRO: Record<'deliverable' | 'shared', string> = {
  deliverable: "What the agents hand in. Stored in the plaza's object store.",
  shared: 'Files the agents share with each other and on the board.',
};

/** Text files up to this size are read into the detail view; larger ones are download-only. */
const TEXT_LIMIT = 200 * 1024;

function groupOf(d: Deliverable): Group {
  const t = d.content_type.toLowerCase();
  if (t.startsWith('video/')) return 'Video';
  if (t.startsWith('image/')) return 'Image';
  if (t.startsWith('audio/')) return 'Audio';
  if (
    t === 'application/pdf' ||
    t.startsWith('text/') ||
    t.includes('json') ||
    t.includes('xml') ||
    t.includes('word') ||
    t.includes('officedocument') ||
    t.includes('opendocument') ||
    t.includes('rtf')
  ) {
    return 'Documents';
  }
  return 'Other';
}

/** Plain text a viewer can show as-is. */
function isTextual(d: Deliverable): boolean {
  const t = d.content_type.toLowerCase();
  return t.startsWith('text/') || t.includes('json') || t.includes('xml');
}

function humanSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 ** 2) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 ** 3) return `${(n / 1024 ** 2).toFixed(1)} MB`;
  return `${(n / 1024 ** 3).toFixed(1)} GB`;
}

function ago(ms: number): string {
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (s < 60) return 'just now';
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} min ago`;
  const hours = Math.floor(m / 60);
  if (hours < 24) return `${hours} h ago`;
  return `${Math.floor(hours / 24)} d ago`;
}

/** The file's extension in capitals, for a file with no preview of its own. */
function monogram(name: string): string {
  const dot = name.lastIndexOf('.');
  const ext = dot >= 0 ? name.slice(dot + 1) : '';
  return (ext || '?').slice(0, 4).toUpperCase();
}

/** The card's preview tile: the video (plays on hover), the image, an audio glyph, or the type's monogram. */
function tile(d: Deliverable): HTMLElement {
  const url = api.fileUrl(d.id);
  const group = groupOf(d);
  if (group === 'Video') {
    const v = h('video', { src: url, preload: 'metadata' }) as HTMLVideoElement;
    v.muted = true;
    const t = h('div.lib-tile', null, v);
    t.addEventListener('mouseenter', () => void v.play().catch(() => undefined));
    t.addEventListener('mouseleave', () => v.pause());
    return t;
  }
  if (group === 'Image') return h('div.lib-tile', null, h('img', { src: url, loading: 'lazy', alt: '' }));
  if (group === 'Audio') return h('div.lib-tile', null, h('span.lib-glyph', { 'aria-hidden': 'true' }, '♪'));
  return h('div.lib-tile', null, h('span.lib-mono', null, monogram(d.name)));
}

/** The large preview in the detail modal. */
function bigPreview(d: Deliverable): HTMLElement {
  const url = api.fileUrl(d.id);
  const name = d.title || d.name;
  const group = groupOf(d);
  if (group === 'Video') {
    const v = h('video.lib-big', { src: url, preload: 'metadata' }) as HTMLVideoElement;
    v.controls = true;
    return v;
  }
  if (group === 'Image') return h('img.lib-big', { src: url, alt: name });
  if (group === 'Audio') {
    const a = h('audio.lib-audio', { src: url, preload: 'metadata' }) as HTMLAudioElement;
    a.controls = true;
    return a;
  }
  if (d.content_type.toLowerCase() === 'application/pdf') return h('iframe.lib-big', { src: url, title: name });
  if (!isTextual(d)) return h('p.faint', null, 'No preview for this type. Download it to open it.');
  const markdown = /\.md$/i.test(d.name) || d.content_type.toLowerCase() === 'text/markdown';
  const pre = (markdown ? h('div.lib-file.lib-text', null, 'Loading…') : h('pre.lib-file.lib-text', null, 'Loading…')) as HTMLElement;
  if (d.size > TEXT_LIMIT) pre.textContent = `Too large to preview (${humanSize(d.size)}). Download it to read it.`;
  else void loadText(d, pre, markdown);
  return pre;
}

async function loadText(d: Deliverable, pre: HTMLElement, markdown: boolean): Promise<void> {
  try {
    const res = await fetch(api.fileUrl(d.id));
    if (!res.ok) throw new Error(String(res.status));
    const text = await res.text();
    const shown = text.length > TEXT_LIMIT ? `${text.slice(0, TEXT_LIMIT)}\n…` : text;
    if (markdown) pre.replaceChildren(renderMarkdown(shown));
    else pre.textContent = shown;
  } catch {
    pre.textContent = 'Could not read this file.';
  }
}

function facts(d: Deliverable): HTMLElement {
  const row = (label: string, value: string, mono = false): HTMLElement =>
    h('li', null, h('span.faint', null, label), h(mono ? 'span.mono' : 'span', null, value));
  return h(
    'ul.lib-facts',
    null,
    row('Name', d.name, true),
    row('Content type', d.content_type, true),
    row('Size', `${humanSize(d.size)} (${d.size.toLocaleString()} bytes)`),
    row('sha256', d.sha256, true),
    row('Handed in by', d.author),
    row('When', new Date(d.created_ms).toLocaleString()),
    row('Store', d.store),
    d.task ? row('Task', d.task, true) : null,
  );
}

function downloadFile(d: Deliverable): void {
  const a = document.createElement('a');
  a.href = api.fileUrl(d.id, true);
  a.download = d.name;
  a.click();
}

export function mount(root: HTMLElement, params: string[] = []): Cleanup {
  let files: Deliverable[] = [];
  let opening = params[0] ?? '';

  const search = input({ type: 'search', placeholder: 'Search by title or name', autocomplete: 'off' });
  const typeSelect = h(
    'select.lib-select',
    null,
    [ALL, ...GROUPS].map((g) => h('option', { value: g }, g)),
  ) as HTMLSelectElement;
  const grid = h('div.lib-grid', null);

  let kind: 'deliverable' | 'shared' = 'deliverable';
  const intro = h('p.faint', null, INTRO[kind]);
  const tabHost = h('div', null);
  const drawTabs = (): void => {
    replace(
      tabHost,
      tabs(
        [
          { key: 'deliverable', label: 'Deliverables' },
          { key: 'shared', label: 'Shared' },
        ],
        kind,
        (key) => {
          if (key === kind) return;
          kind = key === 'shared' ? 'shared' : 'deliverable';
          intro.textContent = INTRO[kind];
          drawTabs();
          void refresh();
        },
      ),
    );
  };
  drawTabs();

  root.append(
    h('div.view-head', null, h('h1', null, 'Library')),
    h(
      'div.stack',
      null,
      tabHost,
      intro,
      h('div.lib-toolbar', null, search, typeSelect),
      grid,
    ),
  );

  const card = (d: Deliverable): HTMLElement => {
    const el = h(
      'article.prov-card.lib-card',
      { tabindex: '0', title: d.name, 'data-key': `file-${d.id}` },
      h('div.lib-tile-wrap', null, tile(d), h('span.lib-tag', null, d.store)),
      h('div.prov-title', null, h('h3.truncate', null, d.title || d.name), h('span.mono.faint.truncate', null, d.name)),
      h(
        'div.prov-meta',
        null,
        h('span.faint', null, `by ${d.author} · ${ago(d.created_ms)}`),
        h('span.faint', null, humanSize(d.size)),
        d.kind === 'shared' && d.audience === 'private' ? h('span.faint', null, `private · ${d.allowed.length} agents`) : null,
      ),
      d.task ? h('span.mono.faint.truncate', { title: 'Task' }, d.task) : null,
    );
    el.addEventListener('click', () => openDetail(d));
    el.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') openDetail(d);
    });
    return el;
  };

  const draw = (): void => {
    if (!files.length) {
      replace(grid, empty('board', 'Nothing handed in yet. Agents deliver finished work with deliver_file.'));
      return;
    }
    const q = search.value.trim().toLowerCase();
    const type = typeSelect.value;
    const shown = files.filter(
      (d) =>
        (type === ALL || groupOf(d) === type) &&
        (!q || (d.title || '').toLowerCase().includes(q) || d.name.toLowerCase().includes(q)),
    );
    replace(grid, ...(shown.length ? shown.map(card) : [h('p.faint', null, 'Nothing matches that search.')]));
  };

  const render = (): void => {
    // A redraw keeps the card that had the focus (the arrow keys pick cards), so a refresh does not drop it.
    const focused = document.activeElement;
    const kept = focused instanceof HTMLElement && grid.contains(focused) ? focused.dataset.key : undefined;
    draw();
    if (kept) grid.querySelector<HTMLElement>(`[data-key="${CSS.escape(kept)}"]`)?.focus({ preventScroll: true });
  };

  /** Arrow keys move the focus card by card in the order drawn, as the desktop's Library does. Enter opens the focused card. */
  const onArrow = (e: KeyboardEvent): void => {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    if (document.querySelector('.modal-backdrop') || (e.target instanceof Element && e.target.closest('input, textarea, select, [contenteditable="true"]'))) return;
    const cards = Array.from(grid.querySelectorAll<HTMLElement>('.lib-card'));
    if (!cards.length) return;
    const focusedCard = (document.activeElement as Element | null)?.closest<HTMLElement>('.lib-card');
    const at = focusedCard ? cards.indexOf(focusedCard) : -1;
    // Nothing picked yet: the first card, whichever arrow. The ends hold.
    const next = at < 0 ? 0 : e.key === 'ArrowDown' ? at + 1 : at - 1;
    e.preventDefault();
    cards[Math.min(Math.max(next, 0), cards.length - 1)]?.focus();
  };

  const refresh = async (): Promise<void> => {
    const list = await attempt(() => api.files(kind));
    if (list) files = list.slice().sort((a, b) => b.created_ms - a.created_ms);
    render();
    // A link to one item (`#/library/<id>`, from a rich reference in agent text) opens its detail once it is listed.
    if (opening) {
      const d = files.find((f) => f.id === opening);
      if (d) {
        opening = '';
        openDetail(d);
      }
    }
  };

  const openDelete = (d: Deliverable, onDone: () => void): void => {
    let close = () => {};
    const remove = button(
      'Delete',
      async () => {
        remove.disabled = true;
        const done = await attempt(async () => {
          await api.removeFile(d.id);
          return true;
        }, `${d.title || d.name} deleted`);
        remove.disabled = false;
        if (!done) return;
        close();
        onDone();
        await refresh();
      },
      { kind: 'primary' },
    );
    close = modal(
      `Delete ${d.title || d.name}?`,
      h(
        'div.stack',
        null,
        h('p.faint', null, "Removes the file from the plaza's object store. This cannot be undone."),
        h('div.prov-row', null, h('span.prov-spacer'), button('Cancel', () => close(), { kind: 'ghost' }), remove),
      ),
    );
  };

  const openDetail = (d: Deliverable): void => {
    let close = () => {};
    close = modal(
      d.title || d.name,
      h(
        'div.stack',
        null,
        bigPreview(d),
        facts(d),
        d.description ? h('p.faint', null, d.description) : null,
        h(
          'div.prov-row',
          null,
          h('span.prov-spacer'),
          button('Delete', () => openDelete(d, () => close()), { kind: 'ghost' }),
          button('Download', () => downloadFile(d), { kind: 'primary' }),
        ),
      ),
    );
  };

  search.addEventListener('input', render);
  typeSelect.addEventListener('change', render);
  const timer = setInterval(render, 60_000);
  const off = on('file', () => void refresh());
  document.addEventListener('keydown', onArrow);
  void refresh();

  return () => {
    off();
    clearInterval(timer);
    document.removeEventListener('keydown', onArrow);
  };
}
