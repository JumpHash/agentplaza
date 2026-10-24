// The skill editor (docs/design/skill-library.md, "Screens"): the file tree, SKILL.md and text files with a live
// preview and findings, versions, compare, publish, bindings, export, and GitHub updates.

import '../styles/skill-editor.css';
import '../styles/proto/library.css';
import {
  ApiError,
  api,
  skillsApi,
  type PromptDiffLine,
  type RoleView,
  type SkillDetail,
  type SkillDiff,
  type SkillEvent,
  type SkillFile,
  type SkillFileIn,
  type SkillFinding,
  type SkillGithubCheck,
  type SkillReport,
  type SkillRules,
  type SkillSeverity,
  type SkillVersion,
  type SkillVersionMeta,
} from '../api';
import { on } from '../store';
import { ago, badge, button, field, input, modal, select, table, tabs, toast, type Column, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import { renderMarkdown } from '../ui/markdown';
import type { Cleanup } from './index';

const CHECK_MS = 400;
const LIVE_MS = 250;
const SKILL_MD = 'SKILL.md';
const PLAZA = 'plaza';
type TabKey = 'edit' | 'versions' | 'history' | 'rules';
const TABS: { key: TabKey; label: string }[] = [
  { key: 'edit', label: 'Edit' },
  { key: 'versions', label: 'Versions' },
  { key: 'history', label: 'History' },
  { key: 'rules', label: 'Rules' },
];
const SEVERITIES: SkillSeverity[] = ['error', 'warning', 'info'];
const SEVERITY_NAME: Record<SkillSeverity, string> = { error: 'Error', warning: 'Warning', info: 'Info' };
const SEVERITY_GROUP: Record<SkillSeverity, string> = { error: 'Errors', warning: 'Warnings', info: 'Infos' };
const SEVERITY_TONE: Record<SkillSeverity, Tone> = { error: 'danger', warning: 'warn', info: 'info' };
const SEVERITY_MARK: Record<SkillSeverity, string> = { error: '✕', warning: '▲', info: '●' };
const DIFF_SIGN: Record<PromptDiffLine['op'], string> = { same: ' ', add: '+', del: '−' };
const enc = new TextEncoder();
const dec = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });

/** A file in the editor. Text files are edited as a string; binary files are kept as bytes. */
interface EditFile {
  path: string;
  executable: boolean;
  /** The executable bit in the latest version. */
  savedExecutable: boolean;
  /** The stored content's hash, sent instead of the bytes while the content is unchanged. */
  hash: string | null;
  text: string | null;
  bytes: Uint8Array | null;
  /** Not in the latest version. */
  isNew: boolean;
  /** The content differs from the latest version. */
  edited: boolean;
}

/** The live pieces of the text editor, rebuilt whenever the tab is drawn. */
interface EditView {
  path: string;
  ta: HTMLTextAreaElement;
  gutter: HTMLElement;
  preview: HTMLElement | null;
}

const messageOf = (err: unknown): string => (err instanceof Error ? err.message : String(err));
const scoreTone = (score: number | null | undefined): Tone => (score == null ? 'muted' : score >= 90 ? 'ok' : score >= 60 ? 'warn' : 'danger');
const newestFirst = (a: SkillVersionMeta, b: SkillVersionMeta): number => b.version - a.version;
const byPath = (a: EditFile, b: EditFile): number => a.path.localeCompare(b.path);
const showErr = (el: HTMLElement, message: string): void => {
  el.hidden = false;
  el.textContent = message;
};

function contentOf(f: EditFile): Uint8Array {
  return f.text != null ? enc.encode(f.text) : (f.bytes ?? new Uint8Array(0));
}

function decodeText(bytes: Uint8Array): string | null {
  try {
    const text = dec.decode(bytes);
    return text.includes('\u0000') ? null : text;
  } catch {
    return null;
  }
}

function toEdit(
  path: string,
  bytes: Uint8Array,
  executable: boolean,
  hash: string | null,
  savedExecutable: boolean,
  isNew: boolean,
  edited: boolean,
): EditFile {
  const text = decodeText(bytes);
  return { path, executable, savedExecutable, hash, text, bytes: text == null ? bytes : null, isNew, edited };
}

function toBase64(bytes: Uint8Array): string {
  let bin = '';
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(bin);
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

/** Why `path` cannot name a file in the skill, or null. `current` is the file's own path when renaming. */
function pathProblem(path: string, taken: string[], current?: string): string | null {
  if (!path) return 'Enter a path.';
  if (path === SKILL_MD) return 'SKILL.md is the skill itself. Edit it in the editor.';
  if (path.startsWith('/')) return 'The path must be relative to the skill folder.';
  if (path.includes('\\')) return 'Use / between folders.';
  if (path.split('/').some((part) => part === '' || part === '.' || part === '..')) return 'The path cannot contain empty, "." or ".." parts.';
  if (path !== current && taken.includes(path)) return 'A file with that path already exists.';
  return null;
}

/** Start and end offsets of the 1-based line `n` in `text`. */
function lineBounds(text: string, n: number): { start: number; end: number } {
  let start = 0;
  for (let line = 1; line < n; line++) {
    const nl = text.indexOf('\n', start);
    if (nl < 0) break;
    start = nl + 1;
  }
  const nl = text.indexOf('\n', start);
  return { start, end: nl < 0 ? text.length : nl };
}

const sourceName = (source: SkillDetail['skill']['source']): string =>
  source.kind === 'built' ? 'Built in' : source.kind === 'upload' ? 'Uploaded' : 'GitHub';
const bindingName = (b: string): string => (b === PLAZA ? 'Every agent' : b.startsWith('role:') ? `Role ${b.slice(5)}` : b);

/** Tags as typed: comma-separated, each trimmed, empty ones dropped. */
export function splitTags(text: string): string[] {
  return text.split(',').map((t) => t.trim()).filter(Boolean);
}

/** The hub's rules for a skill's title and tags, as a sentence for the first failure, or null. */
export function skillMetaProblem(title: string, tags: string[]): string | null {
  const len = (s: string): number => [...s].length;
  if (len(title) === 0 || len(title) > 120) return 'The title must be 1 to 120 characters.';
  if (tags.length > 16 || tags.some((t) => len(t) > 32)) return 'Use at most 16 tags, each of 1 to 32 characters.';
  return null;
}

/** Renders the editor for skill `id` into `root`; `onBack` returns to the list. */
export function mountEditor(root: HTMLElement, id: string, onBack: () => void): Cleanup {
  let alive = true;
  let detail: SkillDetail | null = null;
  let latest: SkillVersion | null = null; // the latest version, as loaded
  let rules: SkillRules | null = null;
  let roles: RoleView[] = [];
  let loadError = '';
  let tab: TabKey = 'edit';
  let skillMd = ''; // the editor's SKILL.md text
  let savedSkillMd = ''; // SKILL.md in the latest version
  let files: EditFile[] = []; // every file but SKILL.md
  let selected = SKILL_MD; // SKILL.md, or a file's path
  let revision = 0; // bumped on every edit; a check response for an older revision is dropped
  let report: SkillReport | null = null;
  let checkSeq = 0;
  let checkTimer: number | undefined;
  let closeModal: (() => void) | null = null;
  let github: SkillGithubCheck | null = null;
  let history: SkillEvent[] | null = null;
  let viewing: SkillVersion | null = null;
  let picks: number[] = [];
  let diff: SkillDiff | null = null;
  let edit: EditView | null = null;
  let summaryEl: HTMLElement | null = null;
  let findingsEl: HTMLElement | null = null;
  let dirtyFlag: HTMLElement | null = null;
  let stale = false; // the hub changed this skill while the editor holds unsaved edits
  let liveTimer: number | undefined;

  const headEl = h('div.se-head');
  const tabsEl = h('div.se-tabs');
  const bodyEl = h('div.se-body');
  const uploadInput = h('input', { type: 'file', hidden: true, 'aria-hidden': 'true' });
  uploadInput.addEventListener('change', () => {
    const file = uploadInput.files?.[0];
    uploadInput.value = '';
    if (file) void addUpload(file);
  });
  root.replaceChildren(h('div.skill-editor', null, headEl, tabsEl, bodyEl, uploadInput));

  const latestNo = (): number => detail?.skill.latest_version ?? 0;
  const savedFile = (path: string): SkillFile | undefined => latest?.files.find((s) => s.path === path);
  const hasFile = (path: string): boolean => path === SKILL_MD || files.some((f) => f.path === path);
  const takenPaths = (): string[] => files.map((f) => f.path);

  const dirty = (): boolean => {
    if (skillMd !== savedSkillMd) return true;
    if (files.length !== (latest?.files.length ?? 0)) return true;
    return files.some((f) => f.isNew || f.edited || f.executable !== f.savedExecutable);
  };
  const syncDirty = (): void => {
    if (dirtyFlag) dirtyFlag.hidden = !dirty();
  };
  const onBeforeUnload = (e: BeforeUnloadEvent): void => {
    if (!dirty()) return;
    e.preventDefault();
    e.returnValue = '';
  };
  window.addEventListener('beforeunload', onBeforeUnload);

  const goBack = (): void => {
    if (dirty() && !window.confirm('Discard your unsaved edits?')) return;
    onBack();
  };

  function openModal(title: string, form: HTMLElement): void {
    closeModal?.();
    closeModal = modal(title, form, () => {
      closeModal = null;
    });
  }

  /** The files as a save sends them: changed or new ones with their bytes, unchanged ones by hash. */
  function filesIn(): SkillFileIn[] {
    return files.map((f): SkillFileIn =>
      f.edited || f.isNew || f.hash == null
        ? { path: f.path, executable: f.executable, content_b64: toBase64(contentOf(f)) }
        : { path: f.path, executable: f.executable, hash: f.hash },
    );
  }

  async function fetchBytes(v: number, path: string): Promise<Uint8Array> {
    const res = await fetch(skillsApi.fileUrl(id, v, path));
    if (!res.ok) throw new Error(`Could not load ${path}: ${res.status}`);
    return new Uint8Array(await res.arrayBuffer());
  }

  // Checks: debounced, and a response is dropped when a newer check or newer edit has superseded it.
  function scheduleCheck(): void {
    window.clearTimeout(checkTimer);
    checkTimer = window.setTimeout(() => void runCheck(), CHECK_MS);
  }

  async function runCheck(): Promise<void> {
    const seq = ++checkSeq;
    const rev = revision;
    const text = skillMd;
    const sent = files.map((f) => {
      const size = contentOf(f).length;
      return f.text != null
        ? { path: f.path, size, executable: f.executable, text: f.text }
        : { path: f.path, size, executable: f.executable };
    });
    try {
      const next = await skillsApi.check(text, sent);
      if (!alive || seq !== checkSeq || rev !== revision) return;
      report = next;
    } catch (err) {
      if (!alive || seq !== checkSeq) return;
      if (summaryEl) replace(summaryEl, h('p.se-error', { role: 'alert' }, `Check failed: ${messageOf(err)}`));
      return;
    }
    paintLines();
    paintReport();
  }

  function paintPreview(): void {
    if (edit?.preview) replace(edit.preview, renderMarkdown(edit.ta.value));
  }

  function paintLines(): void {
    if (!edit) return;
    const { ta, gutter, path } = edit;
    const worst = new Map<number, SkillSeverity>();
    for (const f of report?.findings ?? []) {
      if (f.path !== path || f.line < 1) continue;
      const cur = worst.get(f.line);
      if (!cur || SEVERITIES.indexOf(f.severity) < SEVERITIES.indexOf(cur)) worst.set(f.line, f.severity);
    }
    const count = Math.max(1, ta.value.split('\n').length);
    replace(
      gutter,
      Array.from({ length: count }, (_, i) => {
        const sev = worst.get(i + 1);
        return h('span', { class: sev ? `sev-${sev}` : null, title: sev ? SEVERITY_NAME[sev] : null }, String(i + 1));
      }),
    );
    gutter.scrollTop = ta.scrollTop;
  }

  function paintReport(): void {
    if (!summaryEl || !findingsEl) return;
    const r = report;
    if (!r) {
      replace(summaryEl, h('p.se-muted', null, 'Checking...'));
      replace(findingsEl);
      return;
    }
    const hasScripts = r.findings.some((f) => f.rule === 'script');
    replace(
      summaryEl,
      hasScripts
        ? h('div.se-banner', { role: 'note' }, 'This skill contains code. Agents may run it in their own VMs. Read it before publishing.')
        : null,
      h(
        'div.se-score',
        null,
        badge(`Score ${r.score}`, scoreTone(r.score)),
        badge(`${r.errors} errors`, r.errors ? 'danger' : 'muted'),
        badge(`${r.warnings} warnings`, r.warnings ? 'warn' : 'muted'),
        badge(`${r.infos} infos`, r.infos ? 'info' : 'muted'),
        h('span.se-stats', null, `${r.stats.files} files, ${r.stats.body_lines} body lines, about ${r.stats.est_tokens} tokens`),
      ),
    );
    const paths = Array.from(new Set(r.findings.map((f) => f.path))).sort((a, b) =>
      a === SKILL_MD ? -1 : b === SKILL_MD ? 1 : a.localeCompare(b),
    );
    replace(
      findingsEl,
      h('h2', null, 'Findings'),
      r.findings.length
        ? paths.map((p) =>
            h(
              'section.se-group',
              null,
              h('h3', null, p),
              h(
                'ol.se-findings-list',
                null,
                r.findings.filter((f) => f.path === p).map((f) => h('li', null, findingButton(f))),
              ),
            ),
          )
        : h('p.se-muted', null, 'No findings. This skill passes every check.'),
    );
  }

  function findingButton(f: SkillFinding): HTMLButtonElement {
    const where = f.line > 0 ? `Line ${f.line}` : 'Whole file';
    const el = h(
      'button.se-finding',
      { type: 'button', title: f.rule },
      h('span.se-sev', { class: SEVERITY_TONE[f.severity] }, `${SEVERITY_MARK[f.severity]} ${SEVERITY_NAME[f.severity]}`),
      h('span.se-where', null, where),
      h('span.se-msg', null, f.message),
      h('code.se-rule-id', null, `${f.path}: ${f.rule}`),
    );
    if (hasFile(f.path)) el.addEventListener('click', () => openFinding(f));
    else el.disabled = true;
    return el;
  }

  /** Selects the finding's file and, for a line, moves the editor to it. */
  function openFinding(f: SkillFinding): void {
    selected = f.path;
    drawTab();
    if (f.line > 0) selectLine(f.line);
  }

  function selectLine(n: number): void {
    if (!edit) return;
    const { ta, gutter } = edit;
    const { start, end } = lineBounds(ta.value, n);
    ta.focus();
    ta.setSelectionRange(start, end);
    const lineHeight = parseFloat(getComputedStyle(ta).lineHeight) || 20;
    ta.scrollTop = Math.max(0, (n - 2) * lineHeight);
    gutter.scrollTop = ta.scrollTop;
  }

  // The Edit tab: the file tree on the left; the selected file, its findings and summary on the right.

  function buildEdit(): void {
    edit = null;
    summaryEl = h('div.se-summary');
    findingsEl = h('div.se-findings');
    if (!hasFile(selected)) selected = SKILL_MD;
    const sel = selected === SKILL_MD ? null : (files.find((f) => f.path === selected) ?? null);
    const pane = sel && sel.text == null ? binaryPane(sel) : textPane(selected, selected === SKILL_MD ? skillMd : (sel?.text ?? ''));
    replace(
      bodyEl,
      h(
        'div.se-edit',
        null,
        h(
          'aside.se-tree',
          null,
          h('h2', null, 'Files'),
          treeTools(sel),
          treeList(),
        ),
        h('div.se-editor-col', null, fileHead(sel), pane, summaryEl, findingsEl),
      ),
    );
    paintPreview();
    paintLines();
    paintReport();
  }

  function treeList(): HTMLElement {
    return h(
      'nav.se-tree-list',
      { 'aria-label': 'Skill files' },
      treeItem(SKILL_MD, null),
      [...files].sort(byPath).map((f) => treeItem(f.path, f)),
    );
  }

  function treeItem(path: string, f: EditFile | null): HTMLElement {
    const active = path === selected;
    const size = f ? contentOf(f).length : enc.encode(skillMd).length;
    const changed = f ? f.isNew || f.edited || f.executable !== f.savedExecutable : skillMd !== savedSkillMd;
    return h(
      'button.se-file',
      {
        type: 'button',
        class: active ? 'active' : null,
        'aria-current': active ? 'true' : null,
        onclick: () => {
          selected = path;
          drawTab();
        },
      },
      h('span.se-file-name', null, path),
      h(
        'span.se-file-meta',
        null,
        f?.executable ? badge('exec') : null,
        changed ? badge('changed', 'warn') : null,
        h('span', null, formatBytes(size)),
      ),
    );
  }

  function treeTools(sel: EditFile | null): HTMLElement {
    return h(
      'div.se-tree-tools',
      null,
      button('Add file', openAddFile, { size: 'sm' }),
      button('Upload', () => uploadInput.click(), { size: 'sm' }),
      sel ? fileTools(sel) : null,
    );
  }

  function fileTools(f: EditFile): HTMLElement[] {
    return [
      button('Rename', () => openRename(f), { size: 'sm' }),
      button(f.executable ? 'Not executable' : 'Make executable', () => toggleExecutable(f), { size: 'sm' }),
      button('Remove', () => removeFile(f), { kind: 'ghost', size: 'sm' }),
    ];
  }

  function fileHead(sel: EditFile | null): HTMLElement {
    return h(
      'div.se-file-head',
      null,
      h('h2', null, selected),
      selected === SKILL_MD ? badge('Skill', 'accent') : null,
      sel?.isNew ? badge('new', 'accent') : null,
      sel?.executable ? badge('executable') : null,
      sel ? h('span.se-stats', null, formatBytes(contentOf(sel).length)) : null,
    );
  }

  function textPane(path: string, text: string): HTMLElement {
    const isMd = path === SKILL_MD || path.toLowerCase().endsWith('.md');
    const ta = h('textarea.se-src', {
      spellcheck: 'false',
      wrap: 'off',
      'aria-label': `Source of ${path}`,
      placeholder: 'Write the file',
    });
    ta.value = text;
    const gutter = h('div.se-gutter', { 'aria-hidden': 'true' });
    const preview = isMd ? h('div.se-preview') : null;
    edit = { path, ta, gutter, preview };
    ta.addEventListener('input', () => {
      if (path === SKILL_MD) {
        skillMd = ta.value;
      } else {
        const f = files.find((x) => x.path === path);
        if (f) {
          f.text = ta.value;
          f.edited = true;
        }
      }
      revision++;
      syncDirty();
      paintPreview();
      paintLines();
      scheduleCheck();
    });
    ta.addEventListener('scroll', () => {
      gutter.scrollTop = ta.scrollTop;
    });
    return h(
      'div.se-panes',
      null,
      h('div.se-pane', null, h('div.se-label', null, 'Source'), h('div.se-code', null, gutter, ta)),
      preview ? h('div.se-pane', null, h('div.se-label', null, 'Preview'), preview) : null,
    );
  }

  function binaryPane(f: EditFile): HTMLElement {
    const onServer = !!savedFile(f.path);
    return h(
      'div.se-binary',
      null,
      h('p', null, `${f.path} is a binary file, ${formatBytes(contentOf(f).length)}. It is not shown in the editor.`),
      onServer
        ? h('a.btn', { href: skillsApi.fileUrl(id, latestNo(), f.path), download: f.path.split('/').pop() ?? f.path }, 'Download')
        : h('p.se-muted', null, 'Save a version to keep this file.'),
    );
  }

  function openAddFile(): void {
    const path = input({ type: 'text', maxlength: 512, placeholder: 'scripts/fill.py', autocomplete: 'off', spellcheck: 'false' });
    const err = h('div.se-error', { role: 'alert', hidden: true });
    const add: HTMLButtonElement = button(
      'Add file',
      () => {
        const p = path.value.trim();
        const problem = pathProblem(p, takenPaths());
        if (problem) {
          showErr(err, problem);
          return;
        }
        files.push({ path: p, executable: false, savedExecutable: false, hash: null, text: '', bytes: null, isNew: true, edited: false });
        selected = p;
        revision++;
        closeModal?.();
        syncDirty();
        drawTab();
        scheduleCheck();
      },
      { kind: 'primary' },
    );
    openModal(
      'Add file',
      h(
        'div.se-form',
        null,
        field('Path', path, 'Relative to the skill folder, with / between folders.'),
        err,
        h('div.se-actions', null, add),
      ),
    );
  }

  function openRename(f: EditFile): void {
    const path = input({ type: 'text', maxlength: 512, value: f.path, autocomplete: 'off', spellcheck: 'false' });
    const err = h('div.se-error', { role: 'alert', hidden: true });
    const go: HTMLButtonElement = button(
      'Rename',
      () => {
        const p = path.value.trim();
        const problem = pathProblem(p, takenPaths(), f.path);
        if (problem) {
          showErr(err, problem);
          return;
        }
        closeModal?.();
        if (p !== f.path) {
          f.path = p;
          f.isNew = !savedFile(p);
          selected = p;
          revision++;
          syncDirty();
          drawTab();
          scheduleCheck();
        }
      },
      { kind: 'primary' },
    );
    openModal('Rename file', h('div.se-form', null, field('Path', path), err, h('div.se-actions', null, go)));
  }

  function toggleExecutable(f: EditFile): void {
    f.executable = !f.executable;
    revision++;
    syncDirty();
    drawTab();
    scheduleCheck();
  }

  function removeFile(f: EditFile): void {
    if (!window.confirm(`Remove ${f.path} from the skill? Older versions keep it.`)) return;
    files = files.filter((x) => x !== f);
    selected = SKILL_MD;
    revision++;
    syncDirty();
    drawTab();
    scheduleCheck();
  }

  /** Adds an uploaded file; an uploaded SKILL.md replaces the editor's SKILL.md. */
  async function addUpload(file: File): Promise<void> {
    let bytes: Uint8Array;
    try {
      bytes = new Uint8Array(await file.arrayBuffer());
    } catch (err) {
      toast(messageOf(err), 'error');
      return;
    }
    if (!alive) return;
    const path = file.name;
    const text = decodeText(bytes);
    if (path === SKILL_MD) {
      if (text == null) {
        toast('SKILL.md must be text', 'error');
        return;
      }
      skillMd = text;
    } else {
      const problem = pathProblem(path, []);
      if (problem) {
        toast(problem, 'error');
        return;
      }
      const existing = files.find((f) => f.path === path);
      if (existing) {
        existing.text = text;
        existing.bytes = text == null ? bytes : null;
        existing.edited = true;
      } else {
        const was = savedFile(path);
        files.push(toEdit(path, bytes, was?.executable ?? false, null, was?.executable ?? false, !was, true));
      }
    }
    selected = path;
    revision++;
    syncDirty();
    drawTab();
    scheduleCheck();
    toast(`Uploaded ${path}`);
  }

  // Versions

  function buildVersions(): void {
    if (!detail) return;
    const s = detail.skill;
    const live = s.active_version;
    const best = s.best_version;
    const currentRules = rules?.rules_version ?? 0;
    const cols: Column<SkillVersionMeta>[] = [
      {
        label: 'Version',
        cell: (v) =>
          h(
            'span.se-vers',
            null,
            h('strong', null, `v${v.version}`),
            v.version === live ? badge('Live', 'ok', true) : null,
            v.version === best ? badge('Best', 'accent') : null,
            v.rules_version < currentRules ? badge('stale', 'warn') : null,
          ),
      },
      { label: 'Note', cell: (v) => v.note || '—' },
      { label: 'Author', cell: (v) => v.author },
      { label: 'Created', cell: (v) => ago(v.created_ms) },
      { label: 'Score', num: true, cell: (v) => badge(String(v.score), scoreTone(v.score)) },
      { label: 'Errors / warnings / infos', cell: (v) => `${v.errors} / ${v.warnings} / ${v.infos}` },
      { label: '', cell: (v) => versionActions(v, live) },
    ];
    const sorted = [...detail.versions].sort(newestFirst);
    replace(
      bodyEl,
      h(
        'div.se-versions',
        null,
        viewing ? viewPanel(viewing) : null,
        picks.length ? pickBar() : null,
        diff ? comparePanel(diff) : null,
        sorted.length ? table(cols, sorted, undefined, (v) => String(v.version)) : h('p.se-muted', null, 'No versions yet.'),
      ),
    );
  }

  function versionActions(v: SkillVersionMeta, live: number | null): HTMLElement {
    const picked = picks.includes(v.version);
    const label = v.version === live ? 'Live' : live != null && v.version < live ? 'Roll back' : 'Publish';
    return h(
      'div.se-row-actions',
      null,
      button('View', () => void viewVersion(v.version), { size: 'sm' }),
      button(picked ? 'Unpick' : 'Compare', () => togglePick(v.version), { size: 'sm' }),
      button(label, () => void act(() => skillsApi.publish(id, v.version), `Published v${v.version}`), {
        size: 'sm',
        disabled: v.version === live,
      }),
      button('Restore as new', () => void restoreIntoEditor(v.version), { size: 'sm' }),
    );
  }

  function pickBar(): HTMLElement {
    const text = picks.length === 1 ? `v${picks[0]} picked. Pick one more version to compare.` : '';
    return h(
      'div.se-pickbar',
      null,
      text ? h('span.se-muted', null, text) : null,
      button(
        'Clear',
        () => {
          picks = [];
          diff = null;
          drawTab();
        },
        { kind: 'ghost', size: 'sm' },
      ),
    );
  }

  function togglePick(n: number): void {
    picks = picks.includes(n) ? picks.filter((x) => x !== n) : [...picks, n].slice(-2);
    diff = null;
    if (picks.length === 2) void loadDiff();
    else drawTab();
  }

  async function loadDiff(): Promise<void> {
    const from = Math.min(...picks);
    const to = Math.max(...picks);
    try {
      const next = await skillsApi.diff(id, from, to);
      if (!alive || picks.length !== 2) return;
      diff = next;
    } catch (err) {
      if (!alive) return;
      picks = [];
      toast(messageOf(err), 'error');
    }
    if (alive && tab === 'versions') drawTab();
  }

  async function viewVersion(n: number): Promise<void> {
    try {
      const v = await skillsApi.version(id, n);
      if (!alive) return;
      viewing = v;
      if (tab === 'versions') drawTab();
    } catch (err) {
      toast(messageOf(err), 'error');
    }
  }

  function viewPanel(v: SkillVersion): HTMLElement {
    return h(
      'section.se-view',
      null,
      h(
        'div.se-view-head',
        null,
        h('h2', null, `v${v.version}, read only`),
        v.stale ? badge('stale', 'warn') : null,
        badge(`Score ${v.score}`, scoreTone(v.score)),
        button(
          'Close',
          () => {
            viewing = null;
            drawTab();
          },
          { kind: 'ghost', size: 'sm' },
        ),
      ),
      renderMarkdown(v.skill_md),
      v.files.length
        ? h(
            'ul.se-file-list',
            null,
            v.files.map((f) =>
              h(
                'li',
                null,
                h('code', null, f.path),
                ` ${formatBytes(f.size)} `,
                h('a', { href: skillsApi.fileUrl(id, v.version, f.path), download: f.path.split('/').pop() ?? f.path }, 'Download'),
              ),
            ),
          )
        : h('p.se-muted', null, 'No other files.'),
    );
  }

  function comparePanel(d: SkillDiff): HTMLElement {
    const side = (v: SkillVersion): HTMLElement =>
      h(
        'div.se-side',
        null,
        h('h3', null, `v${v.version}`),
        h(
          'div.se-score',
          null,
          badge(`Score ${v.score}`, scoreTone(v.score)),
          badge(`${v.errors} errors`, v.errors ? 'danger' : 'muted'),
          badge(`${v.warnings} warnings`, v.warnings ? 'warn' : 'muted'),
          badge(`${v.infos} infos`, v.infos ? 'info' : 'muted'),
        ),
      );
    const list = (label: string, items: string[]): HTMLElement =>
      h(
        'div.se-changed',
        null,
        h('h3', null, `${label} (${items.length})`),
        items.length ? h('ul', null, items.map((p) => h('li', null, h('code', null, p)))) : h('p.se-muted', null, 'None'),
      );
    return h(
      'section.se-compare',
      null,
      h(
        'div.se-view-head',
        null,
        h('h2', null, `Compare v${d.from.version} and v${d.to.version}`),
        button(
          'Close',
          () => {
            picks = [];
            diff = null;
            drawTab();
          },
          { kind: 'ghost', size: 'sm' },
        ),
      ),
      h('div.se-sides', null, side(d.from), side(d.to)),
      h('div.se-files', null, list('Added', d.files.added), list('Removed', d.files.removed), list('Changed', d.files.changed)),
      h(
        'div.se-diff',
        null,
        d.skill_md.map((l) =>
          h(
            'div',
            { class: `se-line ${l.op}` },
            h('span.se-ln', null, l.old == null ? '' : String(l.old)),
            h('span.se-ln', null, l.new == null ? '' : String(l.new)),
            h('span.se-sign', null, DIFF_SIGN[l.op]),
            h('code', null, l.text),
          ),
        ),
      ),
    );
  }

  /** Loads version `n` into the editor (its files too) as unsaved edits. */
  async function restoreIntoEditor(n: number): Promise<void> {
    if (dirty() && !window.confirm('Replace your unsaved edits with this version?')) return;
    try {
      const v = await skillsApi.version(id, n);
      const loaded = await Promise.all(
        v.files.map(async (sf) => {
          const bytes = await fetchBytes(n, sf.path);
          const was = savedFile(sf.path);
          return toEdit(sf.path, bytes, sf.executable, null, was?.executable ?? false, !was, true);
        }),
      );
      if (!alive) return;
      skillMd = v.skill_md;
      files = loaded;
      selected = SKILL_MD;
      tab = 'edit';
      revision++;
      drawTabs();
      drawTab();
      syncDirty();
      void runCheck();
      toast(`Loaded v${n} into the editor. Save a version to keep it.`);
    } catch (err) {
      toast(messageOf(err), 'error');
    }
  }

  // History and rules

  async function buildHistory(): Promise<void> {
    let events = history;
    if (!events) {
      replace(bodyEl, h('p.se-muted', null, 'Loading history...'));
      try {
        events = (await skillsApi.history(id)).events;
      } catch (err) {
        if (alive && tab === 'history') replace(bodyEl, h('p.se-error', { role: 'alert' }, messageOf(err)));
        return;
      }
      history = events;
      if (!alive || tab !== 'history') return;
    }
    const cols: Column<SkillEvent>[] = [
      { label: 'Action', cell: (e) => badge(e.action) },
      { label: 'Version', cell: (e) => (e.version == null ? '—' : `v${e.version}`) },
      { label: 'Actor', cell: (e) => e.actor },
      { label: 'When', cell: (e) => ago(e.at_ms) },
    ];
    replace(bodyEl, events.length ? table(cols, events, undefined, (e) => String(e.id)) : h('p.se-muted', null, 'No history yet.'));
  }

  function buildRules(): void {
    if (!rules) {
      replace(bodyEl, h('p.se-muted', null, 'Loading the rules...'));
      return;
    }
    const all = rules.rules;
    replace(
      bodyEl,
      h(
        'div.se-rules',
        null,
        h('p.se-muted', null, 'The checks every skill is scored against. Errors block publishing. Warnings and infos lower the score.'),
        SEVERITIES.map((sev) => {
          const list = all.filter((r) => r.severity === sev);
          return h(
            'section.se-rule-group',
            null,
            h('h2', null, `${SEVERITY_GROUP[sev]} (${list.length})`),
            list.length
              ? list.map((r) =>
                  h(
                    'article.se-rule-card',
                    null,
                    h('div.se-rule-head', null, h('h3', null, r.title), h('code', null, r.id)),
                    h('p', null, r.why),
                  ),
                )
              : h('p.se-muted', null, 'None.'),
          );
        }),
      ),
    );
  }

  // Modals and actions

  function openSave(): void {
    if (!detail) return;
    const note = input({ type: 'text', maxlength: 200, placeholder: 'What changed, in a few words', autocomplete: 'off' });
    const err = h('div.se-error', { role: 'alert', hidden: true });
    const save: HTMLButtonElement = button(
      'Save version',
      async () => {
        save.disabled = true;
        try {
          const res = await skillsApi.addVersion(id, {
            skill_md: skillMd,
            files: filesIn(),
            note: note.value.trim(),
            base_version: latestNo(),
          });
          if (!alive) return;
          closeModal?.();
          if (!res.created) {
            toast('No changes');
            return;
          }
          toast(`Saved version ${res.version.version}`);
          await loadLatest();
        } catch (e) {
          if (!alive) return;
          if (e instanceof ApiError && e.status === 409) {
            showErr(err, 'Someone saved a newer version while you were editing. Reloading replaces your text with the latest version.');
            err.append(
              button(
                'Reload latest',
                () => {
                  closeModal?.();
                  void loadLatest();
                },
                { size: 'sm' },
              ),
            );
          } else {
            showErr(err, messageOf(e));
          }
          save.disabled = false;
        }
      },
      { kind: 'primary' },
    );
    openModal(
      'Save version',
      h('div.se-form', null, field('Note', note, 'Optional. Shown in the version list.'), err, h('div.se-actions', null, save)),
    );
  }

  function openPublish(): void {
    if (!detail) return;
    const d = detail;
    const which = select(
      [...d.versions].sort(newestFirst).map((v) => ({
        value: String(v.version),
        label: `v${v.version}, score ${v.score}${v.version === d.skill.active_version ? ' (live)' : ''}`,
      })),
      String(d.skill.latest_version),
    );
    const err = h('div.se-error', { role: 'alert', hidden: true });
    const go: HTMLButtonElement = button(
      'Publish',
      async () => {
        go.disabled = true;
        const v = Number(which.value);
        try {
          await skillsApi.publish(id, v);
          closeModal?.();
          toast(`Published v${v}`);
          await refreshDetail();
        } catch (e) {
          showErr(err, messageOf(e));
          go.disabled = false;
        }
      },
      { kind: 'primary' },
    );
    openModal(
      'Publish',
      h(
        'div.se-form',
        null,
        field('Version', which, 'Publishing an older version rolls the skill back. Versions with errors cannot be published.'),
        err,
        h('div.se-actions', null, go),
      ),
    );
  }

  function openBindings(): void {
    if (!detail) return;
    const have = new Set(detail.skill.bindings);
    const options = [
      { value: PLAZA, label: 'Every agent' },
      ...roles.map((r) => ({ value: `role:${r.id}`, label: `Role: ${r.title}` })),
    ];
    // A binding to a role that no longer exists stays listed, so saving does not drop it silently.
    const extra = detail.skill.bindings
      .filter((b) => !options.some((o) => o.value === b))
      .map((b) => ({ value: b, label: bindingName(b) }));
    const boxes: HTMLInputElement[] = [];
    const rows = [...options, ...extra].map((o) => {
      const cb = h('input', { type: 'checkbox', value: o.value });
      cb.checked = have.has(o.value);
      boxes.push(cb);
      return h('label.se-check', null, cb, ` ${o.label}`);
    });
    const err = h('div.se-error', { role: 'alert', hidden: true });
    const save: HTMLButtonElement = button(
      'Save bindings',
      async () => {
        save.disabled = true;
        try {
          await skillsApi.setBindings(
            id,
            boxes.filter((cb) => cb.checked).map((cb) => cb.value),
          );
          closeModal?.();
          toast('Bindings saved');
          await refreshDetail();
        } catch (e) {
          showErr(err, messageOf(e));
          save.disabled = false;
        }
      },
      { kind: 'primary' },
    );
    openModal(
      'Bindings',
      h(
        'div.se-form',
        null,
        h('p.se-muted', null, 'A published skill reaches the agents it is bound to: a role, or every agent.'),
        rows,
        err,
        h('div.se-actions', null, save),
      ),
    );
  }

  /** The details dialog: title and tags, saved with PUT /v1/skills/{id}. */
  function openDetails(): void {
    if (!detail) return;
    const s = detail.skill;
    const titleIn = input({ type: 'text', required: true, autocomplete: 'off', value: s.title });
    const tagsIn = input({ type: 'text', autocomplete: 'off', value: s.tags.join(', '), placeholder: 'comma, separated' });
    const err = h('div.se-error', { role: 'alert', hidden: true });
    const save: HTMLButtonElement = button('Save details', () => void saveDetails(), { kind: 'primary' });

    async function saveDetails(): Promise<void> {
      const title = titleIn.value.trim();
      const tags = splitTags(tagsIn.value);
      const problem = skillMetaProblem(title, tags);
      if (problem) {
        showErr(err, problem);
        return;
      }
      save.disabled = true;
      try {
        await skillsApi.saveMeta(id, { title, tags });
        if (!alive) return;
        closeModal?.();
        toast('Details saved');
        await refreshDetail();
      } catch (e) {
        if (!alive) return;
        showErr(err, messageOf(e));
        save.disabled = false;
      }
    }

    openModal(
      'Details',
      h(
        'div.se-form',
        null,
        field('Title', titleIn, 'Shown in the list and the header.'),
        field('Tags', tagsIn, 'Separate them with commas.'),
        err,
        h('div.se-actions', null, save),
      ),
    );
  }

  /** Reloads the latest version from the hub; unsaved edits are replaced only after a confirm. */
  function reloadFromHub(): void {
    if (dirty() && !window.confirm('Replace your unsaved edits with the latest version from the hub?')) return;
    loadLatest().catch((err: unknown) => toast(messageOf(err), 'error'));
  }

  /** The hub changed this skill: reload when nothing is unsaved, otherwise say so and keep the edits. */
  function onHubChange(): void {
    if (!alive || !detail) return;
    if (!dirty()) {
      reloadFromHub();
      return;
    }
    if (!stale) {
      stale = true;
      drawHead();
    }
  }

  // The hub publishes a `skill` frame for every change to a skill; only this skill's frames matter here.
  const offSkill = on('skill', (raw) => {
    if ((raw as { id?: unknown } | null)?.id !== id) return;
    window.clearTimeout(liveTimer);
    liveTimer = window.setTimeout(onHubChange, LIVE_MS);
  });

  async function checkGithub(): Promise<void> {
    try {
      const res = await skillsApi.githubCheck(id);
      if (!alive) return;
      github = res;
      drawHead();
    } catch (err) {
      toast(messageOf(err), 'error');
    }
  }

  async function updateGithub(): Promise<void> {
    if (dirty() && !window.confirm('Replace your unsaved edits with the GitHub update?')) return;
    try {
      const res = await skillsApi.githubUpdate(id);
      if (!alive) return;
      if (!res.created) {
        github = null;
        toast('No changes');
        drawHead();
        return;
      }
      toast(`Updated to v${res.version.version}`);
      await loadLatest();
    } catch (err) {
      toast(messageOf(err), 'error');
    }
  }

  /** Runs a skill action, then reloads the skill's detail. */
  async function act(run: () => Promise<unknown>, done: string): Promise<void> {
    try {
      await run();
      toast(done);
      await refreshDetail();
    } catch (err) {
      toast(messageOf(err), 'error');
    }
  }

  async function refreshDetail(): Promise<void> {
    const next = await skillsApi.get(id);
    if (!alive) return;
    detail = next;
    history = null;
    drawHead();
    drawTab();
  }

  /** Loads the latest version into the editor, replacing what is typed. */
  async function loadLatest(): Promise<void> {
    const next = await skillsApi.get(id);
    const ver = await skillsApi.version(id, next.skill.latest_version);
    const loaded = await Promise.all(
      ver.files.map(async (sf) => toEdit(sf.path, await fetchBytes(ver.version, sf.path), sf.executable, sf.hash, sf.executable, false, false)),
    );
    if (!alive) return;
    detail = next;
    latest = ver;
    skillMd = ver.skill_md;
    savedSkillMd = ver.skill_md;
    files = loaded;
    if (!hasFile(selected)) selected = SKILL_MD;
    github = null;
    history = null;
    stale = false;
    drawHead();
    drawTabs();
    drawTab();
    void runCheck();
  }

  // Header

  function drawHead(): void {
    if (!detail) {
      replace(
        headEl,
        h(
          'header.page-head',
          null,
          h('div.page-head-row', null, h('h1', null, 'Skill'), h('span.sp'), h('div.page-actions', null, button('Back to skills', goBack, { kind: 'ghost' }))),
          h('p.lede', null, loadError || 'Loading...'),
        ),
      );
      return;
    }
    const s = detail.skill;
    const archived = s.archived_ms != null;
    dirtyFlag = h('span', { hidden: !dirty() }, badge('Unsaved changes', 'warn'));
    replace(
      headEl,
      h(
        'header.page-head',
        null,
        h(
          'div.page-head-row',
          null,
          h('h1', null, s.title || s.name),
          h('span.sp'),
          h(
            'div.page-actions',
            null,
            button('Back to skills', goBack, { kind: 'ghost' }),
            button('Save version', openSave, { kind: 'primary' }),
            button('Details…', openDetails, { kind: 'ghost' }),
            button('Publish…', openPublish, { kind: 'ghost' }),
            s.active_version != null ? button('Unpublish', () => void act(() => skillsApi.unpublish(id), 'Unpublished'), { kind: 'ghost' }) : null,
            button('Bindings…', openBindings, { kind: 'ghost' }),
            h('a.btn.ghost', { href: skillsApi.exportUrl(id, s.latest_version), download: `${s.name}.zip` }, 'Export'),
            button(archived ? 'Restore' : 'Archive', () =>
              void act(() => (archived ? skillsApi.restore(id) : skillsApi.archive(id)), archived ? 'Restored' : 'Archived'),
              { kind: 'ghost' },
            ),
          ),
        ),
        h('p.lede', null, s.description || s.name),
      ),
      h(
        'div.pr-bar',
        null,
        badge(sourceName(s.source)),
        s.active_version != null ? badge(`Live v${s.active_version}`, 'ok') : badge('Not published', 'muted'),
        badge(`Latest v${s.latest_version}`, 'muted'),
        s.score != null ? badge(`Score ${s.score}`, scoreTone(s.score)) : null,
        s.bindings.map((b) => badge(bindingName(b), 'accent')),
        s.tags.map((t) => badge(t)),
        archived ? badge('Archived', 'muted') : null,
        dirtyFlag,
      ),
      sourceLine(s),
      stale
        ? h('div.pr-bar', { role: 'status' }, badge('Changed on the hub', 'warn'), button('Reload', reloadFromHub, { kind: 'ghost', size: 'sm' }))
        : null,
    );
  }

  function sourceLine(s: SkillDetail['skill']): HTMLElement | null {
    const src = s.source;
    if (src.kind !== 'github') return null;
    const result = github
      ? github.update_available
        ? h(
            'span.se-update',
            null,
            `New commit ${github.latest.slice(0, 7)} `,
            button('Update', () => void updateGithub(), { kind: 'primary', size: 'sm' }),
          )
        : h('span.se-muted', null, 'Up to date')
      : null;
    return h(
      'div.se-source',
      null,
      h('code', null, `${src.repo}@${src.commit.slice(0, 7)}`),
      button('Check for updates', () => void checkGithub(), { size: 'sm' }),
      result,
    );
  }

  function drawTabs(): void {
    replace(
      tabsEl,
      tabs(TABS, tab, (key) => {
        tab = key as TabKey;
        drawTabs();
        drawTab();
      }),
    );
  }

  function drawTab(): void {
    edit = null;
    if (!detail) {
      replace(bodyEl, h('p.se-muted', null, loadError || 'Loading...'));
      return;
    }
    switch (tab) {
      case 'edit':
        buildEdit();
        break;
      case 'versions':
        buildVersions();
        break;
      case 'history':
        void buildHistory();
        break;
      case 'rules':
        buildRules();
        break;
    }
  }

  async function init(): Promise<void> {
    try {
      [rules, roles] = await Promise.all([skillsApi.rules(), api.roles()]);
      // Every bindings picker lists roles by title (skills.ts, the desktop's).
      roles = [...roles].sort((a, b) => a.title.localeCompare(b.title));
      if (!alive) return;
      await loadLatest();
    } catch (err) {
      if (!alive) return;
      loadError = messageOf(err);
      drawHead();
      drawTab();
    }
  }

  drawHead();
  drawTabs();
  drawTab();
  void init();

  return () => {
    alive = false;
    window.clearTimeout(checkTimer);
    window.clearTimeout(liveTimer);
    offSkill();
    window.removeEventListener('beforeunload', onBeforeUnload);
    closeModal?.();
    root.replaceChildren();
  };
}
