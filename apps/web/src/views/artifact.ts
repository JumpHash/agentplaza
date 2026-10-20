// The artifact screen (`#/artifact/<id>?v=<n>`), and the inline embed and compact chip that rich text uses for
// `artifact:<id>`. The screen draws the artifact with the owner's context and offers its versions, Share, Export and
// Delete. An embed draws the artifact's first panel live under its title, with Open and Share.

import '../styles/artifact.css';
import { ApiError } from '../api';
import { artifactHref, cachedArtifact, deleteArtifact, downloadExport, getArtifact, listVersions } from '../api-artifacts';
import { go } from '../router';
import { ago, attempt, badge, button, modal } from '../ui/components';
import { h, replace } from '../ui/dom';
import { openShareDialog } from '../ui/share';
import { renderArtifact } from '../view/artifact';
import type { Artifact, VersionInfo } from '../view/model';
import { ownerCtx } from '../view/owner-ctx';
import type { Cleanup } from './index';

/** An artifact the rich text names: its id, and the version when the text gives one (`artifact:<id>@<n>`). */
export interface ArtifactRef {
  id: string;
  version: number | null;
}

type PanelOf = Artifact['panels'][number];

const muted = (text: string): HTMLElement => h('p.hint', null, text);

/** The panel's body type (`dataset`, `html` …), read without assuming the metrics body union carries it. */
const bodyType = (p: PanelOf): string | undefined => (p.body as { type?: string }).type;

/** The version in the route's `?v=`, or undefined for the newest. */
function versionParam(): number | undefined {
  const v = Number(new URLSearchParams(location.hash.split('?')[1] ?? '').get('v'));
  return Number.isInteger(v) && v > 0 ? v : undefined;
}

/** The inline card for an artifact a message names: its first panel live, its kind and version, Open and Share. */
export function artifactEmbed(ref: ArtifactRef): HTMLElement {
  const card = h('section.artifact-embed', { 'data-artifact': ref.id }, muted('Loading artifact…'));
  cachedArtifact(ref.id, ref.version ?? undefined).then(
    (a) => replace(card, embedHead(a, ref), renderArtifact(a, ownerCtx(a), { firstOnly: true, head: false }), embedFoot(a, ref)),
    () => replace(card, muted(`Artifact ${ref.id} is not available.`)),
  );
  return card;
}

/** The embed's head: its title, the kind and version, and a chip to the newer version when the text names an older one. */
function embedHead(a: Artifact, ref: ArtifactRef): HTMLElement {
  const newer = ref.version !== null && a.latest > a.version;
  return h(
    'header.artifact-embed-head',
    null,
    h('h3', null, a.title),
    badge(a.kind),
    h('span.hint', null, `v${a.version}`),
    newer ? h('a.chip.t-warn', { href: artifactHref(a.id) }, 'Newer version') : null,
  );
}

function embedFoot(a: Artifact, ref: ArtifactRef): HTMLElement {
  return h(
    'footer.cluster.artifact-embed-foot',
    null,
    button('Open', () => {
      location.hash = artifactHref(a.id, ref.version);
    }, { kind: 'ghost', size: 'sm' }),
    button('Share', () => openShareDialog({ kind: 'artifact', id: a.id, title: a.title }), { kind: 'ghost', size: 'sm' }),
  );
}

/** A compact chip for a further artifact in a message: its title and kind once fetched. A click expands the embed. */
export function artifactChip(ref: ArtifactRef): HTMLElement {
  const label = h('span.artifact-chip-label', null, `artifact ${ref.id}`);
  const chip = h('button.chip.artifact-chip', { type: 'button', title: 'Show this artifact' }, label);
  chip.addEventListener('click', () => chip.replaceWith(artifactEmbed(ref)));
  cachedArtifact(ref.id, ref.version ?? undefined).then(
    (a) => {
      label.textContent = `${a.title} · ${a.kind}`;
    },
    () => {
      chip.disabled = true;
      label.textContent = `artifact ${ref.id} (not available)`;
    },
  );
  return chip;
}

/** The artifact screen: a page head with its versions, Share, Export and Delete, and the artifact drawn below. */
export function mount(root: HTMLElement, params: string[] = []): Cleanup {
  const id = params[0] ?? '';
  let current: Artifact | null = null;

  const title = h('h1', null, 'Artifact');
  const lede = h('p.lede');
  const meta = h('div.cluster.artifact-meta');
  const versions = h('select.select.artifact-version', { 'aria-label': 'Version' }) as HTMLSelectElement;
  const exportHost = h('div.artifact-export-host', null);
  const body = h('div.artifact-body', null, muted('Loading…'));

  /** The version list, newest first. It is refetched on every show so a new version appears. */
  const fillVersions = async (a: Artifact): Promise<void> => {
    const list: VersionInfo[] = (await attempt(() => listVersions(id))) ?? [{ version: a.version, created_ms: a.updated_ms }];
    replace(
      versions,
      list
        .slice()
        .sort((x, y) => y.version - x.version)
        .map((v) => h('option', { value: String(v.version) }, `v${v.version} · ${ago(v.created_ms)}`)),
    );
    versions.value = String(a.version);
  };

  const show = async (version: number | undefined): Promise<void> => {
    try {
      const a = await getArtifact(id, version);
      current = a;
      title.textContent = a.title;
      replace(lede, a.sub ?? '');
      replace(meta, badge(a.kind, 'accent'), h('span.chip', null, `v${a.version}`), h('span.hint', null, `by ${a.author}`), h('span.hint', null, `updated ${ago(a.updated_ms)}`));
      replace(exportHost, exportMenu(a));
      replace(body, renderArtifact(a, ownerCtx(a), { head: false }));
      void fillVersions(a);
    } catch (e) {
      current = null;
      const missing = e instanceof ApiError && e.status === 404;
      if (!missing) console.error('artifact', id, e);
      replace(body, muted(missing ? 'This artifact does not exist, or it is not yours.' : 'This artifact could not be loaded.'));
    }
  };

  versions.addEventListener('change', () => {
    const n = Number(versions.value);
    const newest = current !== null && current.latest === n;
    history.replaceState(null, '', artifactHref(id, newest ? null : n));
    void show(newest ? undefined : n);
  });

  const actions = h(
    'div.page-actions',
    null,
    versions,
    button('Share', () => {
      if (current) openShareDialog({ kind: 'artifact', id, title: current.title });
    }, { kind: 'ghost' }),
    exportHost,
    button('Delete', () => {
      if (current) confirmDelete(current);
    }, { kind: 'ghost' }),
  );

  const head = h(
    'header.page-head',
    null,
    h('div.kicker', null, h('a', { href: '#/library' }, 'Library'), h('span', null, '/'), h('span', null, 'Artifact')),
    h('div.page-head-row', null, title, h('span.sp'), actions),
    lede,
    meta,
  );

  root.append(h('div.artifact-screen', null, head, body));
  void show(versionParam());
  return () => {};
}

/** The export menu: CSV and JSON for each table (dataset) panel, and PNG for each chart, bars and tiles panel. */
function exportMenu(a: Artifact): HTMLElement {
  const items: HTMLElement[] = [];
  a.panels.forEach((p, n) => {
    const label = p.title || `Panel ${n + 1}`;
    const type = bodyType(p);
    if (type === 'dataset') {
      for (const format of ['csv', 'json'] as const) {
        items.push(button(`${format.toUpperCase()} · ${label}`, () => void exportFile(a, format, p.id), { kind: 'ghost' }));
      }
    } else if (type === 'chart' || type === 'bars' || type === 'tiles') {
      items.push(h('a.btn.ghost', { href: pngHref(a, p.id), download: '' }, `PNG · ${label}`));
    }
  });
  if (!items.length) items.push(muted('Nothing to export: no table, chart or tile panels.'));
  return h(
    'details.artifact-export',
    null,
    h('summary.btn.ghost.sm.no-mark', null, h('span', null, 'Export')),
    h('div.menu.kit-pop.artifact-export-list', null, items),
  );
}

/** The hub's PNG render of one panel at the artifact's version. */
function pngHref(a: Artifact, panel: string): string {
  return `/v1/artifacts/${encodeURIComponent(a.id)}/export?format=png&panel=${encodeURIComponent(panel)}&v=${a.version}`;
}

async function exportFile(a: Artifact, format: 'csv' | 'json', panel: string): Promise<void> {
  const stem = a.title.replace(/[^\w.-]+/g, '-') || a.id;
  await attempt(() => downloadExport(a.id, format, panel, `${stem}-v${a.version}-${panel}.${format}`), 'Export ready');
}

/** Asks in the sheet before deleting. A deleted artifact leaves the screen for the Library. */
function confirmDelete(a: Artifact): void {
  let close = (): void => {};
  const cancel = button('Cancel', () => close(), { kind: 'ghost' });
  const remove = button(
    'Delete',
    async () => {
      remove.disabled = true;
      const done = await attempt(async () => {
        await deleteArtifact(a.id);
        return true;
      }, `${a.title} deleted`);
      if (!done) {
        remove.disabled = false;
        return;
      }
      close();
      go('library');
    },
    { kind: 'danger' },
  );
  close = modal(
    `Delete ${a.title}?`,
    h('p', null, 'Removes the artifact and all its versions. Its share links stop working. This cannot be undone.'),
    undefined,
    [cancel, remove],
  );
}
