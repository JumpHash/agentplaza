// Library: the deliverables agents hand in with deliver_file, and the files shared on the board. The Loom prototype's
// layout: a page head, the Deliverables and Shared tabs with their counts, a search and a type filter, a grid of tiles
// (each previewed by its type) and, beside the grid, the selected file's panel with its preview, facts and actions.
// The bytes live in the plaza's object store; the list refreshes live on the `file` frame. Folders, uploads, renames and
// version history are not in the plaza's file API, so the prototype's controls for them are not drawn.

import '../styles/proto/library.css';
import { api, type Deliverable } from '../api';
import { attempt, badge, button, empty, input } from '../ui/components';
import { h, lu, replace } from '../ui/dom';
import type { LucideName } from '../ui/icons.gen';
import { on } from '../store';
import { renderMarkdown } from '../ui/markdown';
import { go } from '../router';
import { listArtifacts } from '../api-artifacts';
import { openShareDialog } from '../ui/share';
import type { ArtifactSummary } from '../view/model';
import type { Cleanup } from './index';

type Group = 'Video' | 'Image' | 'Audio' | 'Documents' | 'Other' | 'Artifacts';
const GROUPS: Group[] = ['Video', 'Image', 'Audio', 'Documents', 'Other', 'Artifacts'];
const ALL = 'All';
/** The group that lists the owner's artifacts (`/v1/artifacts`) instead of the deliverables. */
const ARTIFACTS: Group = 'Artifacts';
type Kind = 'deliverable' | 'shared';

/** The intro line under the tabs, per tab (the prototype's wording). */
const INTRO: Record<Kind, string> = {
  deliverable: 'Files agents hand in when their work is finished.',
  shared: 'Files the owner shares with the plaza.',
};

/** Text files up to this size are read into the detail panel; larger ones are download-only. */
const TEXT_LIMIT = 200 * 1024;

/** The thumbnail tint and icon by file extension; anything not listed gets the neutral file tile. */
const THUMB: Record<string, [string, LucideName]> = {
  md: ['info', 'file-text'],
  txt: ['info', 'file-text'],
  json: ['review', 'braces'],
  png: ['ok', 'image'],
  jpg: ['ok', 'image'],
  jpeg: ['ok', 'image'],
  svg: ['ok', 'image'],
  csv: ['warn', 'sheet'],
};

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

function extOf(name: string): string {
  const dot = name.lastIndexOf('.');
  return dot >= 0 ? name.slice(dot + 1).toLowerCase() : '';
}

/** The tile's thumbnail: the video (plays on hover), the image, an audio glyph, or the type's icon and monogram. */
function thumb(d: Deliverable): HTMLElement {
  const url = api.fileUrl(d.id);
  const group = groupOf(d);
  if (group === 'Video') {
    const v = h('video', { src: url, preload: 'metadata' }) as HTMLVideoElement;
    v.muted = true;
    const box = h('span.lib-card-thumb.lib-th-none', null, v);
    box.addEventListener('mouseenter', () => void v.play().catch(() => undefined));
    box.addEventListener('mouseleave', () => v.pause());
    return box;
  }
  if (group === 'Image') return h('span.lib-card-thumb.lib-th-none', null, h('img', { src: url, loading: 'lazy', alt: '' }));
  if (group === 'Audio') return h('span.lib-card-thumb.lib-th-none', null, h('span.lib-glyph', { 'aria-hidden': 'true' }, '♪'));
  const [tone, glyph] = THUMB[extOf(d.name)] ?? ['none', 'file' as LucideName];
  return h('span.lib-card-thumb', { class: `lib-th-${tone}` }, lu(glyph, 28), monogram(d.name));
}

/** The selected file's large preview. */
function preview(d: Deliverable): HTMLElement {
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
    const a = h('audio', { src: url, preload: 'metadata', style: 'width:100%' }) as HTMLAudioElement;
    a.controls = true;
    return a;
  }
  if (d.content_type.toLowerCase() === 'application/pdf') return h('iframe.lib-big', { src: url, title: name, style: 'height:480px;width:100%' });
  if (!isTextual(d)) return h('p.lib-faint', null, 'No preview for this type. Download it to open it.');
  const markdown = /\.md$/i.test(d.name) || d.content_type.toLowerCase() === 'text/markdown';
  const box = (markdown ? h('div.lib-md', null, 'Loading…') : h('pre.lib-pre', null, 'Loading…')) as HTMLElement;
  if (d.size > TEXT_LIMIT) box.textContent = `Too large to preview (${humanSize(d.size)}). Download it to read it.`;
  else void loadText(d, box, markdown);
  return box;
}

async function loadText(d: Deliverable, box: HTMLElement, markdown: boolean): Promise<void> {
  try {
    const res = await fetch(api.fileUrl(d.id));
    if (!res.ok) throw new Error(String(res.status));
    const text = await res.text();
    const shown = text.length > TEXT_LIMIT ? `${text.slice(0, TEXT_LIMIT)}\n…` : text;
    if (markdown) box.replaceChildren(renderMarkdown(shown));
    else box.textContent = shown;
  } catch {
    box.textContent = 'Could not read this file.';
  }
}

/** The facts of a file, as the prototype's definition list. */
function facts(d: Deliverable): HTMLElement {
  const pairs: [string, Node | string][] = [
    ['Name', h('span.mono', null, d.name)],
    ['Content type', h('span.mono', null, d.content_type)],
    ['Size', `${humanSize(d.size)} (${d.size.toLocaleString()} bytes)`],
    ['Hash', h('span.mono', null, d.hash)],
    ['Handed in by', d.author],
    ['When', new Date(d.created_ms).toLocaleString()],
    ['Audience', d.audience + (d.audience === 'private' ? ` · ${d.allowed.length} agents` : '')],
  ];
  if (d.task) pairs.push(['Task', h('span.mono', null, d.task)]);
  return h(
    'dl.kv',
    null,
    pairs.map(([k, v]) => h('div', null, h('dt', null, k), h('dd', null, v))),
  );
}

function downloadFile(d: Deliverable): void {
  const a = document.createElement('a');
  a.href = api.fileUrl(d.id, true);
  a.download = d.name;
  a.click();
}

/** A tab strip with the count after each label, the prototype's `.tabs` markup. */
function tabBar(items: [Kind, string, number][], current: Kind, onPick: (key: Kind) => void): HTMLElement {
  return h(
    'div.tabs',
    { role: 'tablist' },
    items.map(([key, label, n]) => {
      const b = h('button.tab', { type: 'button', role: 'tab', 'aria-selected': String(key === current) }, label, ' ', h('span.count.num', null, String(n)));
      b.addEventListener('click', () => onPick(key));
      return b;
    }),
  );
}

export function mount(root: HTMLElement, params: string[] = []): Cleanup {
  let byKind: Record<Kind, Deliverable[]> = { deliverable: [], shared: [] };
  let kind: Kind = 'deliverable';
  let selected: string | null = null;
  let confirmDelete = false;
  let opening = params[0] ?? '';
  let artifacts: ArtifactSummary[] | null = null;

  const search = input({ type: 'search', placeholder: 'Search by title or name', 'aria-label': 'Search files', autocomplete: 'off' });
  search.classList.add('lib-q');
  const typeSelect = h(
    'select.select',
    { 'aria-label': 'Type' },
    [ALL, ...GROUPS].map((g) => h('option', { value: g }, g)),
  ) as HTMLSelectElement;
  const tabHost = h('div');
  const intro = h('p.lib-faint', { style: 'margin-top:12px' }, INTRO[kind]);
  const grid = h('div.lib-grid');
  const detailHost = h('div');
  const cols = h('div.lib-cols', null, h('div', null, grid), detailHost);

  root.append(
    h(
      'header.page-head',
      null,
      h('div.page-head-row', null, h('h1', null, 'Library'), h('span.sp'), h('div.page-actions')),
      h('p.lede', null, 'Files agents hand in, and the files the owner shares.'),
    ),
    h(
      'div.lib-main',
      null,
      tabHost,
      intro,
      h('div.lib-tools', null, search, typeSelect),
      cols,
    ),
  );

  const drawTabs = (): void => {
    replace(
      tabHost,
      tabBar(
        [
          ['deliverable', 'Deliverables', byKind.deliverable.length],
          ['shared', 'Shared', byKind.shared.length],
        ],
        kind,
        (key) => {
          if (key === kind) return;
          kind = key;
          selected = null;
          confirmDelete = false;
          intro.textContent = INTRO[kind];
          drawTabs();
          draw();
        },
      ),
    );
  };

  /** The rows the search and the type filter leave, in the tab's order (newest first). */
  function visible(): Deliverable[] {
    const q = search.value.trim().toLowerCase();
    const type = typeSelect.value;
    return byKind[kind].filter(
      (d) => (type === ALL || groupOf(d) === type) && (!q || (d.title || '').toLowerCase().includes(q) || d.name.toLowerCase().includes(q)),
    );
  }

  const tileFor = (d: Deliverable): HTMLElement => {
    const b = h(
      'button.lib-tile',
      { type: 'button', title: d.name, 'data-key': `file-${d.id}`, class: selected === d.id ? 'is-on' : null },
      thumb(d),
      h('span.lib-tile-t', null, d.title || d.name),
      h('span.lib-clip.lib-card-name', null, d.name),
      h('span.lib-clip.lib-card-meta', null, `by ${d.author} · ${ago(d.created_ms)}`),
      h(
        'span.lib-clip.lib-card-meta',
        null,
        humanSize(d.size),
        d.task ? ' · ' : null,
        d.task ? h('span.mono', { title: 'Task' }, d.task) : null,
        d.kind === 'shared' && d.audience === 'private' ? ` · private · ${d.allowed.length} agents` : null,
      ),
    );
    b.addEventListener('click', () => {
      selected = d.id;
      confirmDelete = false;
      draw();
    });
    return b;
  };

  /** The artifacts group: the owner's artifacts, filtered by the search box. A click opens the artifact's screen. */
  const artifactTile = (s: ArtifactSummary): HTMLElement => {
    const b = h(
      'button.lib-tile',
      { type: 'button', title: s.title, 'data-key': `artifact-${s.id}` },
      h('span.lib-card-thumb.lib-th-none', null, h('span', null, s.kind.slice(0, 4).toUpperCase())),
      h('span.lib-tile-t', null, s.title),
      badge(s.kind),
      h('span.lib-clip.lib-card-meta', null, `by ${s.author} · ${ago(s.updated_ms)}`),
      h('span.lib-clip.lib-card-meta', null, `v${s.latest}`, s.shared ? ' · Shared' : null),
      s.project ? h('span.lib-clip.lib-card-name', { title: 'Project' }, s.project) : null,
    );
    b.addEventListener('click', () => go('artifact', s.id));
    return b;
  };

  const drawArtifacts = (): void => {
    if (artifacts === null) {
      replace(grid, h('p.lib-faint', null, 'Loading artifacts…'));
      return;
    }
    if (!artifacts.length) {
      replace(grid, empty('library', 'No artifacts yet', 'Agents make them with artifact_create.'));
      return;
    }
    const q = search.value.trim().toLowerCase();
    const shown = artifacts.filter((s) => !q || s.title.toLowerCase().includes(q) || s.kind.toLowerCase().includes(q));
    replace(grid, ...(shown.length ? shown.map(artifactTile) : [empty('search', 'Nothing matches that search.')]));
  };

  const drawFiles = (): void => {
    const all = byKind[kind];
    if (!all.length) {
      replace(grid, empty('library', 'Nothing handed in yet.', 'Agents deliver finished work with deliver_file.'));
      return;
    }
    const shown = visible();
    replace(grid, ...(shown.length ? shown.map(tileFor) : [empty('search', 'Nothing matches that search.')]));
  };

  /** The selected file's panel: its preview, facts and actions, and the delete confirmation when asked for. */
  const drawDetail = (): void => {
    const d = selected ? byKind[kind].find((f) => f.id === selected) : undefined;
    cols.classList.toggle('has-sel', !!d);
    if (!d) {
      replace(detailHost);
      return;
    }
    const name = d.title || d.name;
    const close = h('button.iconbtn', { type: 'button', title: 'Close', 'aria-label': 'Close' }, lu('x', 16));
    close.addEventListener('click', () => {
      selected = null;
      confirmDelete = false;
      draw();
    });
    const share = button('Share', () => openShareDialog({ kind: 'file', id: d.id, title: name }), { kind: 'ghost', size: 'sm' });
    const remove = button('Delete', () => {
      confirmDelete = true;
      draw();
    }, { kind: 'ghost', size: 'sm' });
    const download = button('Download', () => downloadFile(d), { kind: 'primary', size: 'sm' });
    const sure = button('Delete', () => void deleteFile(d), { kind: 'primary' });
    replace(
      detailHost,
      h(
        'aside.card.lib-det',
        null,
        h('div.card-h', null, h('strong', null, name), h('span.sp'), close),
        h(
          'div.card-b',
          null,
          preview(d),
          d.description ? h('p.lib-faint', null, d.description) : null,
          facts(d),
          h('div.lib-acts', { style: 'margin-top:16px' }, share, remove, download),
          confirmDelete
            ? h(
                'div.lib-sec',
                null,
                h('h3', null, `Delete ${name}?`),
                h(
                  'div.lib-form',
                  null,
                  h('p.lib-faint', null, "Removes the file from the plaza's object store. This cannot be undone."),
                  h('div.lib-acts', null, button('Cancel', () => {
                    confirmDelete = false;
                    draw();
                  }, { kind: 'ghost' }), sure),
                ),
              )
            : null,
        ),
      ),
    );
  };

  const draw = (): void => {
    // A redraw keeps the tile that had the focus (the arrow keys pick tiles), so a refresh does not drop it.
    const focused = document.activeElement;
    const kept = focused instanceof HTMLElement && grid.contains(focused) ? focused.dataset.key : undefined;
    if (typeSelect.value === ARTIFACTS) drawArtifacts();
    else drawFiles();
    drawDetail();
    if (kept) grid.querySelector<HTMLElement>(`[data-key="${CSS.escape(kept)}"]`)?.focus({ preventScroll: true });
  };

  async function deleteFile(d: Deliverable): Promise<void> {
    const done = await attempt(async () => {
      await api.removeFile(d.id);
      return true;
    }, `${d.title || d.name} deleted`);
    if (!done) return;
    selected = null;
    confirmDelete = false;
    await refresh();
  }

  /** Arrow keys move the focus tile by tile in the order drawn. Enter opens the focused tile (a button, so it clicks). */
  const onArrow = (e: KeyboardEvent): void => {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    if (document.querySelector('.scrim') || (e.target instanceof Element && e.target.closest('input, textarea, select, [contenteditable="true"]'))) return;
    const tiles = Array.from(grid.querySelectorAll<HTMLElement>('.lib-tile'));
    if (!tiles.length) return;
    const focusedTile = (document.activeElement as Element | null)?.closest<HTMLElement>('.lib-tile');
    const at = focusedTile ? tiles.indexOf(focusedTile) : -1;
    // Nothing picked yet: the first tile, whichever arrow. The ends hold.
    const next = at < 0 ? 0 : e.key === 'ArrowDown' ? at + 1 : at - 1;
    e.preventDefault();
    tiles[Math.min(Math.max(next, 0), tiles.length - 1)]?.focus();
  };

  const refresh = async (): Promise<void> => {
    const got = await attempt(() => Promise.all([api.files('deliverable'), api.files('shared')]));
    if (got) {
      const byNewest = (a: Deliverable, b: Deliverable): number => b.created_ms - a.created_ms;
      byKind = { deliverable: got[0].slice().sort(byNewest), shared: got[1].slice().sort(byNewest) };
    }
    // A link to one item (`#/library/<id>`, from a rich reference in agent text) selects it once it is listed.
    if (opening) {
      const d = [...byKind.deliverable, ...byKind.shared].find((f) => f.id === opening);
      if (d) {
        opening = '';
        kind = d.kind;
        selected = d.id;
        drawTabs();
        intro.textContent = INTRO[kind];
      }
    }
    drawTabs();
    draw();
  };

  const refreshArtifacts = async (): Promise<void> => {
    const list = await attempt(() => listArtifacts());
    if (list) artifacts = list;
    draw();
  };

  /** Arrow keys and the artifacts drawn on the minute are the only live parts; the file frame refreshes the list. */
  search.addEventListener('input', draw);
  typeSelect.addEventListener('change', () => {
    if (typeSelect.value === ARTIFACTS && artifacts === null) void refreshArtifacts();
    draw();
  });
  const timer = setInterval(() => (typeSelect.value === ARTIFACTS ? void refreshArtifacts() : draw()), 60_000);
  const off = on('file', () => void refresh());
  document.addEventListener('keydown', onArrow);
  drawTabs();
  draw();
  void refresh();

  return () => {
    off();
    clearInterval(timer);
    document.removeEventListener('keydown', onArrow);
  };
}
