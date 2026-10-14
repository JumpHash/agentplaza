// The JSON shapes of apz-ide's serde types, as apz-ide-web returns and reads them. Enums are snake_case strings or
// single-key objects, as serde writes them: a unit variant is its name, a tuple or struct variant is `{ name: payload }`.
// Keep in step with crates/apz-ide (workspace.rs, tree.rs, diff.rs, buffer.rs, state.rs).

/**
 * `agent:<cid>` for an agent's /workspace, `station:w<seq>` for a workstation's work folder, `repo:<name>@<ref>` for a
 * project repo at one branch (apz-ide's canonical form).
 */
export type WorkspaceId = `agent:${number}` | `station:w${number}` | `repo:${string}@${string}`;

/** workspace.rs `Kind`. */
export type WorkspaceKind = 'agent' | 'repo' | 'station';

/** workspace.rs `Workspace`. `ref` is null for an agent. */
export interface Workspace {
  id: WorkspaceId;
  kind: WorkspaceKind;
  owner: string;
  ref: string | null;
  root: string;
  writable: boolean;
}

/** tree.rs `EntryKind`. Symlinks are listed, never followed. */
export type EntryKind = 'file' | 'dir' | 'symlink';

/** tree.rs `Entry`. `hash` is set for files only. */
export interface Entry {
  path: string;
  kind: EntryKind;
  size: number;
  hash: string | null;
  binary: boolean;
}

/** tree.rs `TreePage`. */
export interface TreePage {
  entries: Entry[];
  next: string | null;
}

/** diff.rs `Status`. */
export type Status = 'added' | 'modified' | 'deleted' | 'renamed';

/** diff.rs `LineKind`. */
export type LineKind = 'context' | 'add' | 'remove' | 'no_newline';

/** diff.rs `Line`, without its prefix character. The numbers are null on the side the line is not on. */
export interface Line {
  kind: LineKind;
  old_no: number | null;
  new_no: number | null;
  text: string;
}

/** diff.rs `Hunk`: one `@@` block. */
export interface Hunk {
  old_start: number;
  old_lines: number;
  new_start: number;
  new_lines: number;
  header: string;
  lines: Line[];
}

/** diff.rs `FileChange`. A binary file has no hunks. */
export interface FileChange {
  path: string;
  old_path: string | null;
  status: Status;
  binary: boolean;
  hunks: Hunk[];
}

/** diff.rs `ChangeSet`: what `parseUnified` returns is this plus `stats`. */
export interface ChangeSet {
  files: FileChange[];
}

/** diff.rs `Stats`: totals over a change set. */
export interface Stats {
  files: number;
  additions: number;
  deletions: number;
}

/** What `parseUnified` returns: the ChangeSet, with its totals. */
export interface ParsedDiff extends ChangeSet {
  stats: Stats;
}

/** state.rs `Panel`. */
export type Panel = 'changes' | 'agent' | 'approvals' | 'run' | 'terminal';

/** state.rs `Base`: what the Changes panel compares against. */
export type Base = { checkpoint: number } | 'work_start' | { branch: string };

/** state.rs `IdeState`. `active` is one of `open`, or null when no tab is open. */
export interface IdeState {
  workspace: WorkspaceId | null;
  open: string[];
  active: string | null;
  panel: Panel;
  base: Base;
  review: boolean;
  filter_changed: boolean;
}

/** state.rs `Action`. */
export type Action =
  | { open: string }
  | { close: string }
  | { activate: string }
  | { set_panel: Panel }
  | { set_base: Base }
  | 'toggle_review'
  | 'toggle_changed_filter'
  | { set_workspace: WorkspaceId };
