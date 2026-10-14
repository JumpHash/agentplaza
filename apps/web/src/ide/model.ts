// The IDE view's pure helpers: no DOM, no CodeMirror, no I/O. The view drives them and the tests check them.

import type { FileChange } from './types';

// ---- Languages -------------------------------------------------------------

export type Lang = 'typescript' | 'tsx' | 'javascript' | 'jsx' | 'rust' | 'python' | 'markdown' | 'json' | 'css' | 'html' | 'plain';

const LANGS: Record<string, Lang> = {
  ts: 'typescript', mts: 'typescript', cts: 'typescript', tsx: 'tsx',
  js: 'javascript', mjs: 'javascript', cjs: 'javascript', jsx: 'jsx',
  rs: 'rust', py: 'python', md: 'markdown', markdown: 'markdown',
  json: 'json', css: 'css', html: 'html', htm: 'html',
};

/** The language of a file by its extension (any case). Extensionless files and dotfiles such as `.gitignore` are plain. */
export function languageFor(path: string): Lang {
  const base = path.slice(path.lastIndexOf('/') + 1);
  const dot = base.lastIndexOf('.');
  if (dot <= 0) return 'plain';
  return LANGS[base.slice(dot + 1).toLowerCase()] ?? 'plain';
}

// ---- Buffers and conflicts -------------------------------------------------

/** One open file: the hash it was read at, whether it has edits, and the server's hash while a save conflicts. */
export interface Buffer {
  path: string;
  base: string;
  dirty: boolean;
  saving: boolean;
  conflict: string | null;
}

export type BufferEvent =
  | { kind: 'edit' }
  | { kind: 'save' }
  /** `edited`: the buffer changed while the save was in flight, so it is still dirty. */
  | { kind: 'saved'; hash: string; edited: boolean }
  | { kind: 'save_failed' }
  /** The hub refused the save (409): `current` is the file's hash on the hub now. */
  | { kind: 'conflict'; current: string }
  /** The buffer takes the hub's copy (a reload, or the user chose "Reload"). */
  | { kind: 'reload'; hash: string }
  /** The user chose "Keep mine": the buffer re-bases on the hub's copy and its edits stay. */
  | { kind: 'keep_mine'; current: string };

export function openBuffer(path: string, hash: string): Buffer {
  return { path, base: hash, dirty: false, saving: false, conflict: null };
}

export function applyBuffer(b: Buffer, e: BufferEvent): Buffer {
  switch (e.kind) {
    case 'edit':
      return { ...b, dirty: true };
    case 'save':
      return { ...b, saving: true };
    case 'saved':
      return { ...b, base: e.hash, dirty: e.edited, saving: false, conflict: null };
    case 'save_failed':
      return { ...b, saving: false };
    case 'conflict':
      return { ...b, saving: false, conflict: e.current };
    case 'reload':
      return { ...b, base: e.hash, dirty: false, saving: false, conflict: null };
    case 'keep_mine':
      return { ...b, base: e.current, conflict: null };
  }
}

/**
 * What a file's new hub hash asks of its open buffer: nothing when it is the hash already read, a silent reload when
 * the buffer has no edits, and the conflict state when the buffer has edits the hub does not have.
 */
export function onRemote(b: Buffer, hash: string): { action: 'none' | 'reload' | 'conflict'; buffer: Buffer } {
  if (hash === b.base) return { action: 'none', buffer: b.conflict === null ? b : { ...b, conflict: null } };
  if (!b.dirty) return { action: 'reload', buffer: b };
  return { action: 'conflict', buffer: { ...b, conflict: hash } };
}

// ---- The workspace's state badge -------------------------------------------

export type Badge = 'idle' | 'agent working' | 'saved' | 'conflict';

/** Conflict outranks the agent working, which outranks a save just made; nothing else is idle. */
export function badgeOf(s: { conflict: boolean; agentWorking: boolean; saved: boolean }): Badge {
  if (s.conflict) return 'conflict';
  if (s.agentWorking) return 'agent working';
  return s.saved ? 'saved' : 'idle';
}

// ---- Viewed marks ----------------------------------------------------------

export const LAST_WORKSPACE_KEY = 'apz.ide.workspace';

/** The localStorage key for one workspace's viewed marks. It lives in the browser, so each viewer keeps their own. */
export const viewedKey = (workspace: string): string => `apz.ide.viewed.${workspace}`;

/** Viewed marks: a path to the mark it was viewed at. A changed file's mark changes, so it is no longer viewed. */
export type Viewed = Record<string, string>;

/** Reads stored marks. Anything malformed reads as no marks. */
export function parseViewed(raw: string | null): Viewed {
  if (!raw) return {};
  try {
    const v: unknown = JSON.parse(raw);
    if (v === null || typeof v !== 'object' || Array.isArray(v)) return {};
    const out: Viewed = {};
    for (const [path, mark] of Object.entries(v)) if (typeof mark === 'string') out[path] = mark;
    return out;
  } catch {
    return {};
  }
}

export const isViewed = (marks: Viewed, path: string, mark: string): boolean => marks[path] === mark;

export function withViewed(marks: Viewed, path: string, mark: string, on: boolean): Viewed {
  const next = { ...marks };
  if (on) next[path] = mark;
  else delete next[path];
  return next;
}

/** A file's viewed mark: a 32-bit FNV-1a of its hunks, so any change to the diff changes the mark. */
export function diffMark(file: FileChange): string {
  const text = `${file.status}\u0000${file.binary}\u0000${JSON.stringify(file.hunks)}`;
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(16).padStart(8, '0');
}

/** The added and removed line counts of one file's hunks. */
export function lineCounts(file: FileChange): { add: number; del: number } {
  let add = 0;
  let del = 0;
  for (const hunk of file.hunks) {
    for (const line of hunk.lines) {
      if (line.kind === 'add') add++;
      else if (line.kind === 'remove') del++;
    }
  }
  return { add, del };
}

// ---- Quick open ------------------------------------------------------------

/**
 * How well `query` matches `path`: every query character must appear in order. A match at a word start, in a row, or in
 * the file name scores higher; a long gap and a long path score lower. null when a character is missing. Case-insensitive.
 */
export function fuzzyScore(query: string, path: string): number | null {
  const q = query.toLowerCase();
  if (q === '') return 0;
  const p = path.toLowerCase();
  const nameStart = p.lastIndexOf('/') + 1;
  let score = 0;
  let from = 0;
  let last = -2;
  for (const ch of q) {
    const at = p.indexOf(ch, from);
    if (at < 0) return null;
    score += 1;
    if (at === last + 1) score += 4;
    if (at === 0 || '/._- '.includes(p.charAt(at - 1))) score += 3;
    if (at >= nameStart) score += 2;
    score -= (at - from) * 0.1;
    last = at;
    from = at + 1;
  }
  return score - p.length * 0.01;
}

/** The paths that match `query`, best first (ties by path), at most `limit`. */
export function rankPaths(paths: string[], query: string, limit = 50): string[] {
  const scored: { path: string; score: number }[] = [];
  for (const path of paths) {
    const score = fuzzyScore(query, path);
    if (score !== null) scored.push({ path, score });
  }
  scored.sort((a, b) => b.score - a.score || (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
  return scored.slice(0, limit).map((s) => s.path);
}

// ---- Tree paths ------------------------------------------------------------

/** The directories above a file, root first: `a/b/c.txt` gives `['', 'a', 'a/b']`. The root is `''`. */
export function dirsOf(path: string): string[] {
  const parts = path.split('/');
  const out = [''];
  for (let i = 1; i < parts.length; i++) out.push(parts.slice(0, i).join('/'));
  return out;
}

/** The directory a path sits in (`''` for a top-level file). */
export function parentDir(path: string): string {
  const i = path.lastIndexOf('/');
  return i < 0 ? '' : path.slice(0, i);
}
