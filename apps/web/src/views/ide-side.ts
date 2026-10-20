// The IDE's change views: the per-file diffs of the review (the Diff view in the centre, and the dock's Diff tab) and the
// checkpoints to restore (the Files tab). Pure rendering: the screen (views/ide.ts) owns the data and passes callbacks.

import type { IdeCheckpoint } from '../api';
import { diffMark, isViewed, lineCounts, type Viewed } from '../ide/model';
import type { FileChange, Hunk, Line, Stats } from '../ide/types';
import { ago, badge, card, empty, type Tone } from '../ui/components';
import { h } from '../ui/dom';

const STATUS: Record<FileChange['status'], { label: string; tone: Tone }> = {
  added: { label: 'added', tone: 'ok' },
  modified: { label: 'modified', tone: 'info' },
  deleted: { label: 'deleted', tone: 'danger' },
  renamed: { label: 'renamed', tone: 'warn' },
};

const SIGN: Record<Line['kind'], string> = { context: ' ', add: '+', remove: '-', no_newline: '\\' };

export interface ChangesProps {
  files: FileChange[];
  stats: Stats;
  /** The selected base's value: `work_start` or `checkpoint:<n>`. */
  base: string;
  bases: { value: string; label: string }[];
  marks: Viewed;
  onBase(value: string): void;
  onOpen(path: string): void;
  onMark(path: string, mark: string, on: boolean): void;
}

/** The changes since the base: the base picker and the totals, then one block per file (a viewed block starts shut). */
export function changesPanel(p: ChangesProps): HTMLElement {
  const viewed = p.files.filter((f) => isViewed(p.marks, f.path, diffMark(f))).length;
  const picker = h(
    'select.select',
    { 'aria-label': 'Compare against' },
    p.bases.map((b) => h('option', { value: b.value, selected: b.value === p.base }, b.label)),
  );
  picker.addEventListener('change', () => p.onBase(picker.value));
  return h(
    'div.ided-review-body',
    null,
    h('div.ided-cmp', null, picker, h('span.ided-totals', null, `${p.stats.files} files · +${p.stats.additions} −${p.stats.deletions}`), h('span.ided-totals', null, `${viewed} of ${p.files.length} viewed`)),
    p.files.length === 0
      ? empty('check', 'Nothing changed', 'No file differs from this base.')
      : h('div.ided-files', null, p.files.map((f) => fileBlock(f, p))),
  );
}

function fileBlock(file: FileChange, p: ChangesProps): HTMLElement {
  const mark = diffMark(file);
  const seen = isViewed(p.marks, file.path, mark);
  const { add, del } = lineCounts(file);
  const status = STATUS[file.status];
  const name = file.status === 'renamed' && file.old_path ? `${file.old_path} → ${file.path}` : file.path;
  // The name opens the file in the editor; the rest of the row toggles the block.
  const open = h('span.ided-fname', { title: 'Open in the editor', 'data-path-open': file.path }, name);
  const summary = h('summary', null, badge(status.label, status.tone), open, h('span.ided-add', null, `+${add}`), h('span.ided-del', null, `−${del}`));
  summary.addEventListener('click', (e) => {
    if (e.target === open) {
      e.preventDefault();
      p.onOpen(file.path);
    }
  });
  const box = h('input', { type: 'checkbox', checked: seen });
  const toggle = h('label.ided-mark', null, box, h('span', null, 'Viewed'));
  box.addEventListener('change', () => p.onMark(file.path, mark, box.checked));
  const body: (Node | string)[] = [toggle];
  if (file.binary) body.push(h('p.muted', null, 'Binary file: no text diff.'));
  else if (file.hunks.length === 0) body.push(h('p.muted', null, 'No text change.'));
  for (const hunk of file.hunks) body.push(hunkBlock(hunk));
  return h('details.ided-file', { open: !seen, 'data-path': file.path }, summary, h('div.ided-body', null, body));
}

function hunkBlock(hunk: Hunk): HTMLElement {
  return h(
    'details.ided-hunk',
    { open: true },
    h('summary', null, h('code', null, hunk.header)),
    h('div.ide-lines', null, hunk.lines.map((line) => h('div.ided-ln', { class: line.kind === 'add' ? 'add' : line.kind === 'remove' ? 'del' : '' }, h('span', null, line.old_no ?? ''), h('span', null, line.new_no ?? ''), h('span', null, SIGN[line.kind]), h('span', null, line.text)))),
  );
}

export interface CheckpointsProps {
  checkpoints: IdeCheckpoint[];
  canRestore: boolean;
  onRestore(cp: IdeCheckpoint): void;
}

/** The checkpoints of the workspace, as a card under the tree; each one restores after a confirm. */
export function checkpointsPanel(p: CheckpointsProps): HTMLElement {
  const body = p.checkpoints.length === 0
    ? empty('clock', 'No checkpoints', 'A checkpoint is a saved point in this workspace. Restore one to bring its files back.')
    : p.checkpoints.map((cp) =>
        h(
          'div.ided-cp',
          { 'data-key': String(cp.n) },
          h('div.ided-cp-main', null, h('strong', null, `#${cp.n}`), ` ${cp.label || 'checkpoint'} · ${ago(cp.created_ms)} · ${cp.files} files`),
          h('button.btn.sm', { type: 'button', disabled: !p.canRestore, onclick: () => p.onRestore(cp) }, 'Restore'),
        ),
      );
  return card('Checkpoints', body, { flush: true });
}
