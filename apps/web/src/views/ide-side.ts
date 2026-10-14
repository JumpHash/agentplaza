// The IDE's right panel: the changes since a base (a collapsible diff per file, with viewed marks) and the checkpoints to
// restore. Pure rendering: the screen (views/ide.ts) owns the data and passes callbacks.

import type { IdeCheckpoint } from '../api';
import { diffMark, isViewed, lineCounts, type Viewed } from '../ide/model';
import type { FileChange, Hunk, Line, Stats } from '../ide/types';
import { ago, badge, empty, type Tone } from '../ui/components';
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

export function changesPanel(p: ChangesProps): HTMLElement {
  const viewed = p.files.filter((f) => isViewed(p.marks, f.path, diffMark(f))).length;
  const picker = h(
    'select.select',
    { 'aria-label': 'Compare against' },
    p.bases.map((b) => h('option', { value: b.value, selected: b.value === p.base }, b.label)),
  );
  picker.addEventListener('change', () => p.onBase(picker.value));
  return h(
    'div.ide-changes',
    null,
    h('div.ide-changes-head', null, picker, h('span.ide-totals', null, `${p.stats.files} files · +${p.stats.additions} −${p.stats.deletions}`), h('span.ide-totals', null, `${viewed} of ${p.files.length} viewed`)),
    p.files.length === 0
      ? empty('check', 'Nothing changed', 'No file differs from this base.')
      : h('div.ide-files', null, p.files.map((f) => fileBlock(f, p))),
  );
}

function fileBlock(file: FileChange, p: ChangesProps): HTMLElement {
  const mark = diffMark(file);
  const seen = isViewed(p.marks, file.path, mark);
  const { add, del } = lineCounts(file);
  const status = STATUS[file.status];
  const name = file.old_path ? `${file.old_path} → ${file.path}` : file.path;
  const open = h('span.ide-file-name', { title: 'Open in the editor' }, name);
  const summary = h('summary', null, badge(status.label, status.tone), open, h('span.ide-counts', null, h('span.add', null, `+${add}`), h('span.del', null, `−${del}`)));
  summary.addEventListener('click', (e) => {
    // The name opens the file; the rest of the row toggles the diff.
    if (e.target === open) {
      e.preventDefault();
      p.onOpen(file.path);
    }
  });
  const toggle = h('label.ide-viewed', null, h('input', { type: 'checkbox', checked: seen }), h('span', null, 'Viewed'));
  toggle.querySelector('input')?.addEventListener('change', (e) => p.onMark(file.path, mark, (e.target as HTMLInputElement).checked));
  const body: (Node | string)[] = [toggle];
  if (file.binary) body.push(h('p.ide-note', null, 'Binary file: no text diff.'));
  else if (file.hunks.length === 0) body.push(h('p.ide-note', null, 'No text change.'));
  for (const hunk of file.hunks) body.push(hunkBlock(hunk));
  return h('details.ide-file', { class: file.status, open: !seen }, summary, h('div.ide-file-body', null, body));
}

function hunkBlock(hunk: Hunk): HTMLElement {
  return h(
    'details.ide-hunk',
    { open: true },
    h('summary', null, h('code', null, hunk.header)),
    h('div.ide-lines', null, hunk.lines.map((line) => h('div.ide-line', { class: line.kind }, h('span.ln', null, line.old_no ?? ''), h('span.ln', null, line.new_no ?? ''), h('span.sign', null, SIGN[line.kind]), h('span.code', null, line.text)))),
  );
}

export interface CheckpointsProps {
  checkpoints: IdeCheckpoint[];
  canRestore: boolean;
  onRestore(cp: IdeCheckpoint): void;
}

export function checkpointsPanel(p: CheckpointsProps): HTMLElement {
  if (p.checkpoints.length === 0) return h('div.ide-changes', null, empty('clock', 'No checkpoints', 'A checkpoint is a saved point in this workspace. Restore one to bring its files back.'));
  return h(
    'div.ide-changes',
    null,
    h(
      'ul.ide-cps',
      null,
      p.checkpoints.map((cp) =>
        h(
          'li.ide-cp',
          { 'data-key': String(cp.n) },
          h('div.ide-cp-main', null, h('strong', null, `#${cp.n}`), h('span', null, cp.label || 'checkpoint'), h('span.ide-totals', null, `${ago(cp.created_ms)} · ${cp.files} files`)),
          h('button.btn', { type: 'button', class: 'sm', disabled: !p.canRestore, onclick: () => p.onRestore(cp) }, 'Restore'),
        ),
      ),
    ),
  );
}
