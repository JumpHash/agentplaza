// Files from the owner's machine: dropped on a zone (files, or whole folders) or picked in the browser's file dialog,
// then sent to a project's repository (one change) or attached to a message. Folders keep their layout; what never
// belongs in a repository (.git, node_modules, .DS_Store) is left out and counted.

import '../styles/file-drop.css';
import { projectsApi, type Attachment, type FilesAdded } from '../api-projects';

/** A file and its path inside what was dropped or picked (`src/main.rs`; just the name for a loose file). */
export interface PickedFile {
  path: string;
  file: File;
}

/** Parts of a path that never go into a repository. */
const IGNORED_PARTS = new Set(['.git', 'node_modules', '.DS_Store', 'Thumbs.db', '__MACOSX']);

/** True when `path` has a part that never goes into a repository. */
export function ignoredPath(path: string): boolean {
  return path.split('/').some((part) => IGNORED_PARTS.has(part));
}

/** Splits picked files into the ones to keep and how many were left out. */
export function keepFiles(files: PickedFile[]): { kept: PickedFile[]; skipped: number } {
  const kept = files.filter((f) => !ignoredPath(f.path));
  return { kept, skipped: files.length - kept.length };
}

/** A byte count as people read it: 812 B, 14 KB, 3.2 MB. */
export function sizeText(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  const mb = bytes / (1024 * 1024);
  return `${mb < 10 ? mb.toFixed(1) : Math.round(mb)} MB`;
}

// The browser's directory entries (webkitGetAsEntry), typed as far as this file uses them.
interface FsEntry {
  isFile: boolean;
  isDirectory: boolean;
  name: string;
  fullPath: string;
}
interface FsFileEntry extends FsEntry {
  file: (ok: (f: File) => void, fail: (e: unknown) => void) => void;
}
interface FsDirEntry extends FsEntry {
  createReader: () => { readEntries: (ok: (entries: FsEntry[]) => void, fail: (e: unknown) => void) => void };
}

async function walk(entry: FsEntry, out: PickedFile[]): Promise<void> {
  if (entry.isFile) {
    const file = await new Promise<File>((ok, fail) => (entry as FsFileEntry).file(ok, fail));
    out.push({ path: entry.fullPath.replace(/^\/+/, ''), file });
    return;
  }
  if (!entry.isDirectory || IGNORED_PARTS.has(entry.name)) return;
  const reader = (entry as FsDirEntry).createReader();
  // readEntries answers in batches until an empty one.
  for (;;) {
    const batch = await new Promise<FsEntry[]>((ok, fail) => reader.readEntries(ok, fail));
    if (batch.length === 0) break;
    for (const child of batch) await walk(child, out);
  }
}

/** The files of a drop, folders walked into. */
export async function filesOfDrop(data: DataTransfer): Promise<PickedFile[]> {
  const entries: FsEntry[] = [];
  for (const item of Array.from(data.items)) {
    if (item.kind !== 'file') continue;
    const entry = (item as DataTransferItem & { webkitGetAsEntry?: () => FsEntry | null }).webkitGetAsEntry?.();
    if (entry) entries.push(entry);
  }
  if (entries.length === 0) return Array.from(data.files).map((file) => ({ path: file.name, file }));
  const out: PickedFile[] = [];
  for (const entry of entries) await walk(entry, out);
  return out;
}

/** True when a drag carries files (not text or a link). */
function carriesFiles(ev: DragEvent): boolean {
  return Array.from(ev.dataTransfer?.types ?? []).includes('Files');
}

/**
 * Makes `el` take dropped files: while files are dragged over it, it has the class `drop-over` (styles/file-drop.css);
 * a drop hands them to `onFiles`. Returns the cleanup.
 */
export function dropZone(el: HTMLElement, onFiles: (files: PickedFile[]) => void): () => void {
  let depth = 0;
  const enter = (ev: DragEvent): void => {
    if (!carriesFiles(ev)) return;
    ev.preventDefault();
    depth++;
    el.classList.add('drop-over');
  };
  const over = (ev: DragEvent): void => {
    if (!carriesFiles(ev)) return;
    ev.preventDefault();
    if (ev.dataTransfer) ev.dataTransfer.dropEffect = 'copy';
  };
  const leave = (ev: DragEvent): void => {
    if (!carriesFiles(ev)) return;
    depth = Math.max(0, depth - 1);
    if (depth === 0) el.classList.remove('drop-over');
  };
  const drop = (ev: DragEvent): void => {
    if (!carriesFiles(ev) || !ev.dataTransfer) return;
    ev.preventDefault();
    ev.stopPropagation();
    depth = 0;
    el.classList.remove('drop-over');
    void filesOfDrop(ev.dataTransfer).then((files) => {
      if (files.length) onFiles(files);
    });
  };
  el.addEventListener('dragenter', enter);
  el.addEventListener('dragover', over);
  el.addEventListener('dragleave', leave);
  el.addEventListener('drop', drop);
  return () => {
    el.removeEventListener('dragenter', enter);
    el.removeEventListener('dragover', over);
    el.removeEventListener('dragleave', leave);
    el.removeEventListener('drop', drop);
  };
}

/** Opens the browser's file dialog: files (several), or one folder with everything in it. */
export function pickFiles(opts: { folder?: boolean; multiple?: boolean; accept?: string } = {}): Promise<PickedFile[]> {
  return new Promise((resolve) => {
    const input = document.createElement('input');
    input.type = 'file';
    input.multiple = opts.multiple ?? true;
    if (opts.accept) input.accept = opts.accept;
    if (opts.folder) (input as HTMLInputElement & { webkitdirectory: boolean }).webkitdirectory = true;
    input.style.display = 'none';
    input.addEventListener('change', () => {
      const files = Array.from(input.files ?? []).map((file) => ({ path: file.webkitRelativePath || file.name, file }));
      input.remove();
      resolve(files);
    });
    // A dialog closed with nothing chosen fires `cancel` (where the browser has it); the promise then answers [].
    input.addEventListener('cancel', () => {
      input.remove();
      resolve([]);
    });
    document.body.append(input);
    input.click();
  });
}

/** Progress of an upload: files sent so far of all, and bytes. */
export interface UploadProgress {
  done: number;
  total: number;
  bytes: number;
  totalBytes: number;
}

/**
 * Sends `files` into project `id`'s repository: each file's bytes (three at a time), then one change of the owner's
 * on main with all of them, under `folder` when given. Ignored paths are left out (the answer counts them).
 */
export async function uploadToProject(
  id: string,
  files: PickedFile[],
  opts: { folder?: string; title?: string; onProgress?: (p: UploadProgress) => void } = {},
): Promise<{ added: FilesAdded | null; skipped: number }> {
  const { kept, skipped } = keepFiles(files);
  if (kept.length === 0) return { added: null, skipped };
  const totalBytes = kept.reduce((n, f) => n + f.file.size, 0);
  const hashes: { path: string; hash: string }[] = new Array(kept.length);
  let next = 0;
  let done = 0;
  let bytes = 0;
  const worker = async (): Promise<void> => {
    while (next < kept.length) {
      const at = next++;
      const f = kept[at]!;
      const up = await projectsApi.upload(id, f.file);
      hashes[at] = { path: f.path, hash: up.hash };
      done++;
      bytes += f.file.size;
      opts.onProgress?.({ done, total: kept.length, bytes, totalBytes });
    }
  };
  await Promise.all([worker(), worker(), worker()]);
  const added = await projectsApi.addFiles(id, hashes, { folder: opts.folder, title: opts.title });
  return { added, skipped };
}

/** Attaches `files` (uploaded one by one) for a message; their ids go in the message's `attachments`. */
export async function attachFiles(files: PickedFile[]): Promise<Attachment[]> {
  const out: Attachment[] = [];
  for (const f of files) out.push(await projectsApi.attach(f.file, f.path.slice(f.path.lastIndexOf('/') + 1)));
  return out;
}
