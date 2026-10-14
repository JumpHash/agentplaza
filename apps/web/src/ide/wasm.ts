// The IDE model's wasm (crates/apz-ide-web), loaded on first use. The console never fetches it: nothing imports this
// module at start, and `loadIde()` imports the glue (and so the wasm) only when the IDE asks. Each wasm export takes
// and returns JSON text; the functions here wrap that, so callers see the shapes in ./types.

import type { Action, ChangeSet, Entry, IdeState, ParsedDiff, WorkspaceId } from './types';

/** The glue's exports, as wasm-bindgen generates them (`--target web`). Every value crosses as JSON text. */
export interface IdeGlue {
  default(): Promise<unknown>;
  parse_unified(text: string): string;
  check_path(p: string): string;
  check_read(p: string): string;
  check_write(p: string): string;
  check_dir(p: string): string;
  is_secret(p: string): boolean;
  initial_state(): string;
  reduce(state: string, action: string): string;
  children(entries: string, dir: string): string;
  changed_filter(entries: string, changes: string): string;
  parse_workspace_id(s: string): string;
  format_workspace_id(json: string): string;
}

/** The IDE model, typed. A refusal (a bad path, a bad diff, a bad id) throws an Error with apz-ide's message. */
export interface Ide {
  /** Parses `git diff` or `diff -u` text into hunks, with the totals. */
  parseUnified(text: string): ParsedDiff;
  /** Cleans a relative path: drops `.` and empty segments; refuses `..`, absolute and control-character paths. */
  checkPath(p: string): string;
  /** A path the IDE may read: valid and not a secret file. */
  checkRead(p: string): string;
  /** A path the IDE may write: readable and not under `.plaza/`. */
  checkWrite(p: string): string;
  /** A directory path; `''` is the root. */
  checkDir(p: string): string;
  isSecret(p: string): boolean;
  initialState(): IdeState;
  /** The next state after an action. Pure: the same inputs always give the same state. */
  reduce(state: IdeState, action: Action): IdeState;
  /** The direct children of `dir` among the entries, directories first. */
  children(entries: Entry[], dir: string): Entry[];
  /** The entries a change set touches, and their ancestor directories, sorted by path. */
  changedFilter(entries: Entry[], changes: ChangeSet): Entry[];
  /** Parses a workspace id in its canonical form. */
  parseWorkspaceId(s: string): WorkspaceId;
  formatWorkspaceId(id: WorkspaceId): string;
}

/** The typed wrapper over a glue. Exported so the wrapping is testable without the wasm. */
export function wrapIde(glue: IdeGlue): Ide {
  const parsed = <T>(text: string): T => JSON.parse(text) as T;
  return {
    parseUnified: (text) => parsed<ParsedDiff>(glue.parse_unified(text)),
    checkPath: (p) => glue.check_path(p),
    checkRead: (p) => glue.check_read(p),
    checkWrite: (p) => glue.check_write(p),
    checkDir: (p) => glue.check_dir(p),
    isSecret: (p) => glue.is_secret(p),
    initialState: () => parsed<IdeState>(glue.initial_state()),
    reduce: (state, action) => parsed<IdeState>(glue.reduce(JSON.stringify(state), JSON.stringify(action))),
    children: (entries, dir) => parsed<Entry[]>(glue.children(JSON.stringify(entries), dir)),
    changedFilter: (entries, changes) => parsed<Entry[]>(glue.changed_filter(JSON.stringify(entries), JSON.stringify(changes))),
    parseWorkspaceId: (s) => parsed<WorkspaceId>(glue.parse_workspace_id(s)),
    formatWorkspaceId: (id) => glue.format_workspace_id(JSON.stringify(id)),
  };
}

/** The glue's file name at the dist root: esbuild replaces this at build time (null when the IDE crate is missing). */
declare const __APZ_IDE_JS__: string | null;

let loading: Promise<Ide> | null = null;

/**
 * The IDE model, loaded and initialised on the first call; later calls share it. Rejects when this build has no IDE
 * model or the wasm does not load; a failed load is retried by the next call.
 */
export function loadIde(): Promise<Ide> {
  if (!loading) {
    loading = (async () => {
      const name = typeof __APZ_IDE_JS__ === 'string' ? __APZ_IDE_JS__ : '';
      if (name === '') throw new Error('this build has no IDE model');
      const glueUrl = new URL('./' + name, document.baseURI).href;
      const glue = (await import(/* @vite-ignore */ glueUrl)) as IdeGlue;
      await glue.default();
      return wrapIde(glue);
    })();
    loading.catch(() => {
      loading = null;
    });
  }
  return loading;
}
