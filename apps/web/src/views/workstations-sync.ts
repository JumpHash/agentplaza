// The work folder of one workstation: the sync's state (and a paused sync's decision), its conflicts, and a browser of the
// hub's mirror of the folder, where files can be downloaded, uploaded, deleted and delivered to the Library. The folder
// syncs by itself; this page only reads and writes the hub's side of it (docs/design/workstations.md section 8).

import { api, type StationSync, type SyncFile } from '../api';
import { ago, attempt, button, card, empty, field, input, modal, table, textarea, toast, type Column } from '../ui/components';
import { h, replace } from '../ui/dom';
import { bytesText, crumbsOf, joinPath, pendingPaths, parentDir, treeOf, uploadProblem, type Section } from './workstations-model';

/** How many held deletions the page lists; the rest are counted. */
const PENDING_SHOWN = 50;

const fileName = (path: string): string => path.slice(path.lastIndexOf('/') + 1);

/** The work folder: its state, the conflicts, and the mirror's files, folder by folder. */
export function syncCard(stationId: string): Section {
  let alive = true;
  let state: StationSync | null = null;
  let stateFailed = false;
  // All of the mirror's live files, or those under the folder being looked at (the hub lists by prefix).
  let files: SyncFile[] | null = null;
  let filesFailed = false;
  let dir = '';

  const body = h('div.stack', null, empty('clock', 'Loading…'));
  const fileInput = h('input', { type: 'file', hidden: true, 'aria-label': 'File to put in the work folder' });

  const statusLine = (s: StationSync): string => {
    const last = s.last_sync_ms ? `last sync ${ago(s.last_sync_ms)}` : 'never synced';
    return `${s.files} files · ${bytesText(s.bytes)} · ${s.linked ? 'linked' : 'not linked'} · ${last}`;
  };

  /** A paused sync: why, what it holds for deletion, and the owner's two ways on. */
  const pausedBox = (s: StationSync): HTMLElement => {
    const held = pendingPaths(s.pending_deletions);
    const shown = held.slice(0, PENDING_SHOWN);
    const more = held.length - shown.length;
    return h(
      'div.ws-banner.is-paused',
      null,
      h('p', null, 'Syncing is paused', s.reason ? `: ${s.reason}` : '.', held.length ? ` ${held.length} deletion${held.length === 1 ? '' : 's'} wait${held.length === 1 ? 's' : ''} for your decision.` : ''),
      shown.length ? h('ul.ws-pending', null, shown.map((p) => h('li', null, h('code', null, p)))) : null,
      more > 0 ? h('p.faint', null, `And ${more} more.`) : null,
      h(
        'div.ws-buttons',
        null,
        held.length
          ? button('Resume and apply the deletions', () => confirmResume(held.length), { kind: 'danger' })
          : null,
        button(held.length ? 'Resume and keep the files' : 'Resume', () => void resume(false), { kind: 'primary' }),
      ),
    );
  };

  const resume = async (applyDeletions: boolean): Promise<void> => {
    const done = await attempt(() => api.resumeStationSync(stationId, applyDeletions), 'Sync resumed');
    if (done !== undefined) void refresh();
  };

  const confirmResume = (count: number): void => {
    const pane = h(
      'div.stack',
      null,
      h('p', null, `The hub deletes ${count} file${count === 1 ? '' : 's'} from the work folder on the machine. The copies on the hub are not removed.`),
      h(
        'div.ws-buttons',
        null,
        button('Cancel', () => close()),
        button(
          'Resume and delete',
          () => {
            close();
            void resume(true);
          },
          { kind: 'danger' },
        ),
      ),
    );
    const close = modal('Resume and apply the deletions?', pane);
  };

  const conflictsBox = (s: StationSync): HTMLElement | null => {
    if (!s.conflicts.length) return null;
    const columns: Column<SyncFile>[] = [
      { label: 'Conflict copy', cell: (f) => h('strong', null, f.path) },
      { label: 'Of', cell: (f) => f.conflict_of ?? '—' },
      { label: 'Modified', cell: (f) => ago(f.mtime_ms) },
      {
        label: '',
        cell: (f) => h('span.ws-row-actions', null, download(f), button('Show folder', () => openFolder(parentDir(f.path)), { kind: 'ghost', size: 'sm' })),
      },
    ];
    return h(
      'div.ws-conflicts',
      null,
      h('p', null, `${s.conflicts.length} conflict${s.conflicts.length === 1 ? '' : 's'}: both copies are kept. The copy named "conflict" is the machine's version.`),
      h('div.ws-scroll', null, table(columns, s.conflicts)),
    );
  };

  const openFolder = (path: string): void => {
    dir = path;
    // The listing of the folder left is not the one of this folder: the page shows its loading state until it is in.
    files = null;
    filesFailed = false;
    render();
    void loadFiles();
  };

  const download = (f: SyncFile): HTMLElement => h('a.btn.ghost.sm', { href: api.stationFileUrl(stationId, f.path), download: fileName(f.path) }, 'Download');

  const confirmDelete = (f: SyncFile): void => {
    const pane = h(
      'div.stack',
      null,
      h('p', null, `${f.path} is removed from the work folder on the machine at the next sync. Its copy in the hub is kept as a deleted version.`),
      h(
        'div.ws-buttons',
        null,
        button('Cancel', () => close()),
        button(
          'Delete',
          () => {
            close();
            void remove(f);
          },
          { kind: 'danger' },
        ),
      ),
    );
    const close = modal('Delete this file?', pane);
  };

  const remove = async (f: SyncFile): Promise<void> => {
    const done = await attempt(() => api.deleteStationFile(stationId, f.path), `${fileName(f.path)} deleted`);
    if (done !== undefined) void refresh();
  };

  const confirmDeliver = (f: SyncFile): void => {
    const title = input({ value: fileName(f.path), maxlength: 200, 'aria-label': 'Title', autocomplete: 'off' });
    const description = textarea({ rows: 3, maxlength: 2000, 'aria-label': 'Description' });
    const pane = h(
      'div.stack',
      null,
      field('Title', title, 'How the file is listed in the Library.'),
      field('Description', description, 'Optional.'),
      h(
        'div.ws-buttons',
        null,
        button('Cancel', () => close()),
        button(
          'Deliver to Library',
          async () => {
            const done = await attempt(
              () => api.deliverStationFile(stationId, f.path, { title: title.value.trim() || undefined, description: description.value.trim() || undefined }),
              'Delivered to the Library',
            );
            if (done !== undefined) close();
          },
          { kind: 'primary' },
        ),
      ),
    );
    const close = modal('Deliver to the Library', pane);
  };

  const upload = async (file: File): Promise<void> => {
    const problem = uploadProblem(file.name, file.size);
    if (problem) {
      toast(problem, 'error');
      return;
    }
    const done = await attempt(() => api.putStationFile(stationId, joinPath(dir, file.name), file), `${file.name} is in the work folder`);
    if (done !== undefined) void refresh();
  };

  fileInput.addEventListener('change', () => {
    const file = fileInput.files?.[0];
    fileInput.value = '';
    if (file) void upload(file);
  });

  const rowsOf = (): { folder?: { name: string; path: string }; file?: SyncFile }[] => {
    if (!files) return [];
    const tree = treeOf(files, dir);
    return [...tree.folders.map((folder) => ({ folder })), ...tree.files.map((file) => ({ file }))];
  };

  const browser = (): HTMLElement => {
    const crumbs = crumbsOf(dir);
    const trail = h(
      'nav.ws-crumbs',
      { 'aria-label': 'Folder' },
      crumbs.map((c, i) =>
        i === crumbs.length - 1
          ? h('span.ws-crumb.is-here', null, c.name)
          : h('button.ws-crumb', { type: 'button', onclick: () => openFolder(c.path) }, c.name),
      ),
    );
    const toolbar = h('div.ws-buttons', null, button('Upload file', () => fileInput.click(), { kind: 'primary' }));
    if (files === null) {
      return h('div.stack', null, trail, toolbar, filesFailed ? empty('alert', 'Could not load the work folder.') : empty('file', 'Loading…'));
    }
    const rows = rowsOf();
    const columns: Column<{ folder?: { name: string; path: string }; file?: SyncFile }>[] = [
      {
        label: 'Name',
        cell: (r) =>
          r.folder
            ? h('button.ws-crumb', { type: 'button', onclick: () => openFolder(r.folder?.path ?? '') }, `${r.folder.name}/`)
            : h('strong', null, r.file ? fileName(r.file.path) : ''),
      },
      { label: 'Size', cell: (r) => (r.file ? bytesText(r.file.size) : '—'), num: true },
      { label: 'Modified', cell: (r) => (r.file ? ago(r.file.mtime_ms) : '—') },
      {
        label: '',
        cell: (r) =>
          r.file
            ? h(
                'span.ws-row-actions',
                null,
                download(r.file),
                button('Deliver', () => confirmDeliver(r.file as SyncFile), { kind: 'ghost', size: 'sm', title: 'Copy into the Library' }),
                button('Delete', () => confirmDelete(r.file as SyncFile), { kind: 'danger', size: 'sm' }),
              )
            : null,
      },
    ];
    return h(
      'div.stack',
      null,
      trail,
      toolbar,
      rows.length
        ? h('div.ws-scroll', null, table(columns, rows))
        : empty('file', dir ? 'This folder is empty' : 'The work folder is empty', 'Files the agents write here sync to the hub, and files you upload sync to the machine.'),
    );
  };

  const render = (): void => {
    if (!alive) return;
    if (state === null && stateFailed) {
      replace(body, empty('alert', 'Could not load the work folder.'));
      return;
    }
    if (state === null) return;
    const paused = state.paused === true;
    replace(
      body,
      h('p.faint', null, statusLine(state)),
      paused ? pausedBox(state) : null,
      conflictsBox(state),
      h('p.faint', null, 'The mirror on the hub. The machine has the same files; a change on either side reaches the other at the next sync.'),
      browser(),
      fileInput,
    );
  };

  async function loadFiles(): Promise<void> {
    try {
      // The folder's own files: the hub lists by prefix, so the root gets the whole mirror.
      files = await api.stationSyncFiles(stationId, dir ? `${dir}/` : '');
      filesFailed = false;
    } catch {
      filesFailed = true;
    }
    if (alive) render();
  }

  async function refresh(): Promise<void> {
    const [s] = await Promise.all([api.stationSync(stationId).catch((): null => null), loadFiles()]);
    if (!alive) return;
    if (s) {
      state = s;
      stateFailed = false;
    } else if (state === null) {
      stateFailed = true;
    }
    render();
  }

  void refresh();
  return {
    el: card('Work folder', body),
    refresh,
    dispose(): void {
      alive = false;
    },
  };
}
