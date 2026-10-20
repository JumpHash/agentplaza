// The prompt editor (docs/design/prompt-library.md, "Screens"), in the Loom prototype's layout: the page head with the
// prompt's actions, the status badges, the tabs (Edit, Versions, History, Rules). Edit is the markdown source beside a
// line diff against the live version; the checks (score and findings) and the preview sit under it. Versions mark the live
// and the best one; compare shows a line diff; publish, history and the rules are as before.

import '../styles/prompt-editor.css';
import '../styles/proto/library.css';
import {
  ApiError,
  promptsApi,
  type PromptDetail,
  type PromptDiff,
  type PromptEvent,
  type PromptFinding,
  type PromptKind,
  type PromptMeta,
  type PromptReport,
  type PromptRules,
  type PromptSeverity,
  type PromptVersion,
  type PromptVersionMeta,
} from '../api';
import { on } from '../store';
import { ago, badge, button, card, field, input, modal, select, table, toast, type Column, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import { renderMarkdown } from '../ui/markdown';
import type { Cleanup } from './index';

const CHECK_MS = 400;
const LIVE_MS = 250;
type TabKey = 'edit' | 'versions' | 'history' | 'rules';
const SEVERITIES: PromptSeverity[] = ['error', 'warning', 'info'];
const SEVERITY_NAME: Record<PromptSeverity, string> = { error: 'Error', warning: 'Warning', info: 'Info' };
const SEVERITY_TONE: Record<PromptSeverity, Tone> = { error: 'danger', warning: 'warn', info: 'info' };
const SEVERITY_MARK: Record<PromptSeverity, string> = { error: '✕', warning: '▲', info: '●' };
const KIND_NAME: Record<PromptKind, string> = { system: 'System prompt', agents_md: 'AGENTS.md', fragment: 'Fragment' };
const DIFF_SIGN: Record<'same' | 'add' | 'del', string> = { same: ' ', add: '+', del: '−' };

const messageOf = (err: unknown): string => (err instanceof Error ? err.message : String(err));
const scoreTone = (score: number | null | undefined): Tone => (score == null ? 'muted' : score >= 90 ? 'ok' : score >= 60 ? 'warn' : 'danger');
const newestFirst = (a: PromptVersionMeta, b: PromptVersionMeta): number => b.version - a.version;

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

/** A line diff of `a` against `b` (the longest common subsequence of lines): same, del and add rows, in order. */
function lineDiff(a: string, b: string): { op: 'same' | 'add' | 'del'; text: string }[] {
  const A = a.split('\n');
  const B = b.split('\n');
  const n = A.length;
  const m = B.length;
  const dp: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) dp[i]![j] = A[i] === B[j] ? dp[i + 1]![j + 1]! + 1 : Math.max(dp[i + 1]![j]!, dp[i]![j + 1]!);
  }
  const out: { op: 'same' | 'add' | 'del'; text: string }[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (A[i] === B[j]) {
      out.push({ op: 'same', text: A[i]! });
      i++;
      j++;
    } else if (dp[i + 1]![j]! >= dp[i]![j + 1]!) {
      out.push({ op: 'del', text: A[i++]! });
    } else {
      out.push({ op: 'add', text: B[j++]! });
    }
  }
  while (i < n) out.push({ op: 'del', text: A[i++]! });
  while (j < m) out.push({ op: 'add', text: B[j++]! });
  return out;
}

/** The diff lines as the prototype draws them: a `.diff` box of `.dl` rows with a sign. */
function diffBox(rows: { op: 'same' | 'add' | 'del'; text: string }[]): HTMLElement {
  return h(
    'div.diff',
    null,
    rows.map((l) => h('div', { class: l.op === 'same' ? 'dl' : `dl ${l.op}` }, `${DIFF_SIGN[l.op]} ${l.text}`)),
  );
}

/** A tab strip with the count after a label, the prototype's `.tabs` markup. */
function tabBar(items: { key: TabKey; label: string; count?: number }[], current: TabKey, onPick: (key: TabKey) => void): HTMLElement {
  return h(
    'div.tabs',
    { role: 'tablist' },
    items.map((t) => {
      const b = h('button.tab', { type: 'button', role: 'tab', 'aria-selected': String(t.key === current) }, t.label, t.count != null ? ' ' : null, t.count != null ? h('span.count.num', null, String(t.count)) : null);
      b.addEventListener('click', () => onPick(t.key));
      return b;
    }),
  );
}

/** A tag list as typed: comma-separated, each trimmed, empty ones dropped. */
export function parseTags(text: string): string[] {
  return text.split(',').map((t) => t.trim()).filter(Boolean);
}

/** The binding box for `kind`, set to `binding`: a role for system prompts, Every agent for AGENTS.md, none for fragments. */
export function bindingField(kind: PromptKind, binding: string | null): { node: HTMLElement; value: () => string | null } {
  if (kind === 'system') {
    const roleIn = input({ type: 'text', autocomplete: 'off', placeholder: 'e.g. planner' });
    roleIn.value = binding?.startsWith('role:') ? binding.slice('role:'.length) : '';
    return {
      node: field('Role', roleIn, 'The agent role this prompt is bound to. Leave empty for none.'),
      value: () => (roleIn.value.trim() ? `role:${roleIn.value.trim()}` : null),
    };
  }
  if (kind === 'agents_md') {
    const sharedIn = h('input', { type: 'checkbox' }) as HTMLInputElement;
    sharedIn.checked = binding === 'plaza';
    return { node: h('label.check', null, sharedIn, ' Apply to every agent'), value: () => (sharedIn.checked ? 'plaza' : null) };
  }
  return { node: h('p.pr-note', null, 'Fragments are not bound to a role or to the plaza.'), value: () => null };
}

/** The hub's rules for a prompt's details (title, description, tags, binding), as a sentence for the first failure, or null. */
export function metaProblem(kind: PromptKind, meta: PromptMeta): string | null {
  const len = (s: string): number => [...s].length;
  if (len(meta.title) === 0 || len(meta.title) > 120) return 'The title must be 1 to 120 characters.';
  if (len(meta.description) > 500) return 'The description is longer than 500 characters.';
  if (meta.tags.length > 16 || meta.tags.some((t) => len(t) > 32)) return 'Use at most 16 tags, each of 1 to 32 characters.';
  if (kind === 'system' && meta.binding != null && !/^role:[a-z0-9-]{1,64}$/.test(meta.binding)) {
    return 'A role is lower-case letters, digits and dashes, at most 64 characters.';
  }
  return null;
}

/** Renders the editor for prompt `id` into `root`; `onBack` returns to the list. */
export function mountEditor(root: HTMLElement, id: string, onBack: () => void): Cleanup {
  let alive = true;
  let detail: PromptDetail | null = null;
  let rules: PromptRules | null = null;
  let kind: PromptKind = 'system';
  let loadError = '';
  let tab: TabKey = 'edit';
  let body = ''; // the editor's text
  let savedBody = ''; // the text of the latest version
  let liveBody = ''; // the text of the live version (the latest, when none is live): what the diff compares with
  let baseVersion = 0; // the latest version the editor started from
  let report: PromptReport | null = null;
  let checkSeq = 0;
  let checkTimer: number | undefined;
  let closeModal: (() => void) | null = null;
  let history: PromptEvent[] | null = null;
  let viewing: PromptVersion | null = null;
  let picks: number[] = [];
  let diff: PromptDiff | null = null;
  let edit: EditView | null = null;
  let dirtyFlag: HTMLElement | null = null;
  let stale = false; // the hub changed this prompt while the editor holds unsaved edits
  let liveTimer: number | undefined;

  const headEl = h('div');
  const barEl = h('div.pr-bar');
  const tabsEl = h('div');
  const bodyEl = h('div');
  root.replaceChildren(h('div.prompt-editor', null, headEl, barEl, tabsEl, bodyEl));

  const dirty = (): boolean => body !== savedBody;
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

  // Checks: debounced, and a response is dropped when a newer check or newer text has superseded it.
  function scheduleCheck(): void {
    window.clearTimeout(checkTimer);
    checkTimer = window.setTimeout(() => void runCheck(), CHECK_MS);
  }

  async function runCheck(): Promise<void> {
    const seq = ++checkSeq;
    const text = body;
    try {
      const next = await promptsApi.check(kind, text);
      if (!alive || seq !== checkSeq || text !== body) return;
      report = next;
    } catch (err) {
      if (!alive || seq !== checkSeq) return;
      if (edit) replace(edit.summary, h('p.pe-error', { role: 'alert' }, `Check failed: ${messageOf(err)}`));
      return;
    }
    paintReport();
  }

  function paintPreview(): void {
    if (edit) replace(edit.preview, renderMarkdown(body));
  }

  function paintDiff(): void {
    if (edit) replace(edit.diffEl, diffBox(lineDiff(liveBody, body)));
  }

  function paintReport(): void {
    if (!edit) return;
    const r = report;
    if (!r) {
      replace(edit.summary, h('p.pe-muted', null, 'Checking...'));
      replace(edit.findings);
      return;
    }
    replace(
      edit.summary,
      h(
        'div.pe-score',
        null,
        badge(`Score ${r.score}`, scoreTone(r.score)),
        badge(`${r.errors} errors`, r.errors ? 'danger' : 'muted'),
        badge(`${r.warnings} warnings`, r.warnings ? 'warn' : 'muted'),
        badge(`${r.infos} infos`, r.infos ? 'info' : 'muted'),
        h('span.pe-stats', null, `${r.stats.words} words, about ${r.stats.est_tokens} tokens, ${r.stats.lines} lines, ${r.stats.headings} headings`),
      ),
    );
    replace(
      edit.findings,
      r.findings.length
        ? h('ol.pe-findings-list', null, r.findings.map((f) => h('li', null, findingButton(f))))
        : h('p.pe-muted', null, 'No findings. This prompt passes every check.'),
    );
  }

  function findingButton(f: PromptFinding): HTMLElement {
    const where = f.line > 0 ? `Line ${f.line}` : 'Whole document';
    const el = h(
      'button.pe-finding',
      { type: 'button', title: f.rule },
      h('span.pe-sev', { class: SEVERITY_TONE[f.severity] }, `${SEVERITY_MARK[f.severity]} ${SEVERITY_NAME[f.severity]}`),
      h('span.pe-where', null, where),
      h('span.pe-msg', null, f.message),
      h('code.pe-rule-id', null, f.rule),
    );
    if (f.line > 0) el.addEventListener('click', () => selectLine(f.line));
    return el;
  }

  function selectLine(n: number): void {
    if (!edit) return;
    const { ta } = edit;
    const { start, end } = lineBounds(ta.value, n);
    ta.focus();
    ta.setSelectionRange(start, end);
    const lineHeight = parseFloat(getComputedStyle(ta).lineHeight) || 20;
    ta.scrollTop = Math.max(0, (n - 2) * lineHeight);
  }

  /** The Edit tab: the source beside the diff against the live version, with the note and the save; the checks and preview below. */
  function buildEdit(): void {
    const ta = h('textarea.textarea.pr-src', { spellcheck: 'false', wrap: 'off', 'aria-label': 'Markdown', placeholder: 'Write the prompt in markdown' }) as HTMLTextAreaElement;
    ta.value = body;
    const noteIn = input({ type: 'text', maxlength: 200, placeholder: 'What changed, in a few words', autocomplete: 'off' });
    const saveBtn: HTMLButtonElement = button(`Save as version ${(detail?.prompt.latest_version ?? 0) + 1}`, () => void saveVersion(noteIn, saveBtn), { kind: 'primary' });
    const diffEl = h('div.pr-diff');
    const summary = h('div');
    const findings = h('div');
    const preview = h('div.lib-md');
    edit = { ta, diffEl, summary, findings, preview, saveErr: h('div.pe-error', { role: 'alert', hidden: true }) };
    ta.addEventListener('input', () => {
      body = ta.value;
      syncDirty();
      paintDiff();
      paintPreview();
      scheduleCheck();
    });
    replace(
      bodyEl,
      h(
        'div.pr-cols',
        null,
        card(null, [
          ta,
          h('div.pr-bar', null, field('Note', noteIn), saveBtn),
          edit.saveErr,
        ]),
        card('Against the live version', diffEl),
      ),
      h('div.pr-stack', { style: 'margin-top:16px' }, card('Checks', [summary, findings]), card('Preview', preview)),
    );
    paintDiff();
    paintPreview();
    paintReport();
  }

  /** Saves the editor's text as the next version. A version saved meanwhile is said so, with a reload to take it. */
  async function saveVersion(noteIn: HTMLInputElement, saveBtn: HTMLButtonElement): Promise<void> {
    if (!detail || !edit) return;
    const err = edit.saveErr;
    err.hidden = true;
    err.textContent = '';
    saveBtn.disabled = true;
    try {
      const res = await promptsApi.addVersion(id, body, noteIn.value.trim(), baseVersion);
      if (!alive) return;
      if (!res.created) {
        toast('No changes');
        return;
      }
      toast(`Saved version ${res.version.version}`);
      await loadLatest();
    } catch (e) {
      if (!alive) return;
      err.hidden = false;
      if (e instanceof ApiError && e.status === 409) {
        err.append('Someone saved a newer version while you were editing. Reloading replaces your text with the latest version. ', button('Reload latest', () => void loadLatest(), { kind: 'ghost', size: 'sm' }));
      } else {
        err.textContent = messageOf(e);
      }
    } finally {
      saveBtn.disabled = false;
    }
  }

  function buildVersions(): void {
    if (!detail) return;
    const d = detail;
    const live = d.prompt.active_version;
    const best = d.prompt.best_version;
    const current = rules?.rules_version ?? 0;
    const cols: Column<PromptVersionMeta>[] = [
      {
        label: 'Version',
        cell: (v) =>
          h(
            'span',
            null,
            h('strong', null, `v${v.version}`),
            ' ',
            v.version === live ? badge('Live', 'ok') : null,
            v.version === best ? badge('Best', 'accent') : null,
            v.rules_version < current ? badge('stale', 'warn') : null,
          ),
      },
      { label: 'Note', cell: (v) => v.note || '—' },
      { label: 'Author', cell: (v) => v.author },
      { label: 'Created', cell: (v) => ago(v.created_ms) },
      { label: 'Score', num: true, cell: (v) => badge(String(v.score), scoreTone(v.score)) },
      { label: 'E / W / I', cell: (v) => h('span.mono', null, `${v.errors} / ${v.warnings} / ${v.infos}`) },
      { label: '', cell: (v) => versionActions(v, live) },
    ];
    const sorted = [...d.versions].sort(newestFirst);
    replace(
      bodyEl,
      viewing ? viewPanel(viewing) : null,
      sorted.length ? h('div.tbl-wrap', null, table(cols, sorted, undefined, (v) => String(v.version))) : h('p.pr-note', null, 'No versions yet.'),
      picks.length ? pickBar() : null,
      diff ? comparePanel(diff) : null,
    );
  }

  function versionActions(v: PromptVersionMeta, live: number | null): HTMLElement {
    const picked = picks.includes(v.version);
    const publishLabel = v.version === live ? 'Live' : live != null && v.version < live ? 'Roll back' : 'Publish';
    return h(
      'div.lib-acts',
      null,
      button('View', () => void viewVersion(v.version), { kind: 'ghost', size: 'sm' }),
      button(picked ? 'Unpick' : 'Compare', () => togglePick(v.version), { kind: 'ghost', size: 'sm' }),
      button(publishLabel, () => void act(() => promptsApi.publish(id, v.version), `Published v${v.version}`), { kind: 'ghost', size: 'sm', disabled: v.version === live }),
      button('Restore as new', () => void restoreIntoEditor(v.version), { kind: 'ghost', size: 'sm' }),
    );
  }

  function pickBar(): HTMLElement {
    const text = picks.length === 1 ? `v${picks[0]} picked. Pick one more version to compare.` : '';
    return h(
      'div.pr-bar',
      null,
      text ? h('span.pr-note', null, text) : null,
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
      const next = await promptsApi.diff(id, from, to);
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
      const v = await promptsApi.version(id, n);
      if (!alive) return;
      viewing = v;
      if (tab === 'versions') drawTab();
    } catch (err) {
      toast(messageOf(err), 'error');
    }
  }

  function viewPanel(v: PromptVersion): HTMLElement {
    return card(
      `v${v.version}, read only`,
      h('pre.lib-pre', null, v.body),
      {
        actions: [
          button(
            'Close',
            () => {
              viewing = null;
              drawTab();
            },
            { kind: 'ghost', size: 'sm' },
          ),
        ],
      },
    );
  }

  function comparePanel(d: PromptDiff): HTMLElement {
    return card(
      `Compare v${d.from.version} and v${d.to.version}`,
      h(
        'div.diff',
        null,
        d.lines.map((l) => h('div', { class: l.op === 'same' ? 'dl' : `dl ${l.op}` }, `${DIFF_SIGN[l.op]} ${l.text}`)),
      ),
      {
        actions: [
          button(
            'Close',
            () => {
              picks = [];
              diff = null;
              drawTab();
            },
            { kind: 'ghost', size: 'sm' },
          ),
        ],
      },
    );
  }

  async function restoreIntoEditor(n: number): Promise<void> {
    if (dirty() && !window.confirm('Replace your unsaved edits with this version?')) return;
    try {
      const v = await promptsApi.version(id, n);
      if (!alive) return;
      body = v.body;
      tab = 'edit';
      drawTabs();
      drawTab();
      syncDirty();
      void runCheck();
      toast(`Loaded v${n} into the editor. Save a version to keep it.`);
    } catch (err) {
      toast(messageOf(err), 'error');
    }
  }

  async function buildHistory(): Promise<void> {
    let events = history;
    if (!events) {
      replace(bodyEl, h('p.pr-note', null, 'Loading history...'));
      try {
        events = (await promptsApi.history(id)).events;
      } catch (err) {
        if (alive && tab === 'history') replace(bodyEl, h('p.pr-note', { role: 'alert' }, messageOf(err)));
        return;
      }
      history = events;
      if (!alive || tab !== 'history') return;
    }
    const cols: Column<PromptEvent>[] = [
      { label: 'Action', cell: (e) => badge(e.action, 'muted') },
      { label: 'Version', cell: (e) => (e.version == null ? '—' : `v${e.version}`) },
      { label: 'Actor', cell: (e) => e.actor },
      { label: 'When', cell: (e) => ago(e.at_ms) },
    ];
    replace(bodyEl, events.length ? h('div.tbl-wrap', null, table(cols, events, undefined, (e) => String(e.id))) : h('p.pr-note', null, 'No history yet.'));
  }

  function buildRules(): void {
    if (!rules) {
      replace(bodyEl, h('p.pr-note', null, 'Loading the rules...'));
      return;
    }
    const current = kind;
    const applies = rules.rules.filter((r) => r.kinds.length === 0 || r.kinds.includes(current));
    const ordered = SEVERITIES.flatMap((sev) => applies.filter((r) => r.severity === sev));
    replace(
      bodyEl,
      h('p.pr-note', null, `The checks that apply to ${KIND_NAME[current]}. Errors block publishing and cost 25 points each; warnings cost 8 and infos 2.`),
      h(
        'div.pr-rules',
        null,
        ordered.map((r) => h('div.row', null, badge(r.severity, SEVERITY_TONE[r.severity]), h('span', null, r.title, h('small', null, r.why)))),
      ),
    );
  }

  function openPublish(): void {
    if (!detail) return;
    const d = detail;
    const which = select(
      [...d.versions].sort(newestFirst).map((v) => ({
        value: String(v.version),
        label: `v${v.version}, score ${v.score}${v.version === d.prompt.active_version ? ' (live)' : ''}`,
      })),
      String(d.prompt.latest_version),
    );
    const err = h('div.pe-error', { role: 'alert', hidden: true });
    const go: HTMLButtonElement = button(
      'Publish',
      async () => {
        go.disabled = true;
        const v = Number(which.value);
        try {
          await promptsApi.publish(id, v);
          closeModal?.();
          toast(`Published v${v}`);
          await refreshDetail();
        } catch (e) {
          err.hidden = false;
          err.textContent = messageOf(e);
          go.disabled = false;
        }
      },
      { kind: 'primary' },
    );
    openModal(
      'Publish',
      h(
        'div.pe-form',
        null,
        field('Version', which, 'Publishing an older version rolls the prompt back. Versions with errors cannot be published.'),
        err,
        h('div.pe-actions', null, go),
      ),
    );
  }

  /** The details dialog: title, description, tags and binding, saved with PUT /v1/prompts/{id}. */
  function openDetails(): void {
    if (!detail) return;
    const p = detail.prompt;
    const titleIn = input({ type: 'text', required: true, autocomplete: 'off', value: p.title });
    const descIn = input({ type: 'text', autocomplete: 'off', value: p.description, placeholder: 'A line on what it is for' });
    const tagsIn = input({ type: 'text', autocomplete: 'off', value: p.tags.join(', '), placeholder: 'comma, separated' });
    const bind = bindingField(p.kind, p.binding);
    const err = h('div.pe-error', { role: 'alert', hidden: true });
    const save: HTMLButtonElement = button('Save details', () => void saveDetails(), { kind: 'primary' });

    async function saveDetails(): Promise<void> {
      const meta: PromptMeta = {
        title: titleIn.value.trim(),
        description: descIn.value.trim(),
        tags: parseTags(tagsIn.value),
        binding: bind.value(),
      };
      const problem = metaProblem(p.kind, meta);
      if (problem) {
        err.hidden = false;
        err.textContent = problem;
        return;
      }
      save.disabled = true;
      try {
        await promptsApi.saveMeta(id, meta);
        if (!alive) return;
        closeModal?.();
        toast('Details saved');
        await refreshDetail();
      } catch (e) {
        if (!alive) return;
        err.hidden = false;
        err.textContent = e instanceof ApiError && e.status === 409 ? `${messageOf(e)} (the binding is already taken)` : messageOf(e);
        save.disabled = false;
      }
    }

    openModal(
      'Details',
      h(
        'div.pe-form',
        null,
        field('Title', titleIn),
        field('Description', descIn),
        field('Tags', tagsIn, 'Separate them with commas.'),
        bind.node,
        err,
        h('div.pe-actions', null, save),
      ),
    );
  }

  /** Reloads the latest version from the hub; unsaved edits are replaced only after a confirm. */
  function reloadFromHub(): void {
    if (dirty() && !window.confirm('Replace your unsaved edits with the latest version from the hub?')) return;
    loadLatest().catch((err: unknown) => toast(messageOf(err), 'error'));
  }

  /** The hub changed this prompt: reload when nothing is unsaved, otherwise say so and keep the edits. */
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

  // The hub publishes a `prompt` frame for every change to a prompt; only this prompt's frames matter here.
  const offPrompt = on('prompt', (raw) => {
    if ((raw as { id?: unknown } | null)?.id !== id) return;
    window.clearTimeout(liveTimer);
    liveTimer = window.setTimeout(onHubChange, LIVE_MS);
  });

  /** Runs a prompt action, then reloads the prompt's detail. */
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
    const next = await promptsApi.get(id);
    if (!alive) return;
    detail = next;
    history = null;
    drawHead();
    drawTab();
  }

  /** Loads the latest version's body into the editor (replacing what is typed), and the live version's for the diff. */
  async function loadLatest(): Promise<void> {
    const next = await promptsApi.get(id);
    const latest = await promptsApi.version(id, next.prompt.latest_version);
    const liveN = next.prompt.active_version ?? next.prompt.latest_version;
    const live = liveN === latest.version ? latest : await promptsApi.version(id, liveN);
    if (!alive) return;
    detail = next;
    kind = next.prompt.kind;
    baseVersion = next.prompt.latest_version;
    body = latest.body;
    savedBody = latest.body;
    liveBody = live.body;
    history = null;
    stale = false;
    drawHead();
    drawTabs();
    drawTab();
    void runCheck();
  }

  function drawHead(): void {
    if (!detail) {
      replace(
        headEl,
        h(
          'header.page-head',
          null,
          h('div.page-head-row', null, h('h1', null, 'Prompt'), h('span.sp'), h('div.page-actions', null, button('Back to prompts', goBack, { kind: 'ghost' }))),
          h('p.lede', null, loadError || 'Loading...'),
        ),
      );
      return;
    }
    const p = detail.prompt;
    const archived = p.archived_ms != null;
    dirtyFlag = h('span', { hidden: !dirty() }, badge('Unsaved changes', 'warn'));
    replace(
      headEl,
      h(
        'header.page-head',
        null,
        h(
          'div.page-head-row',
          null,
          h('h1', null, p.title || 'Untitled prompt'),
          h('span.sp'),
          h(
            'div.page-actions',
            null,
            h('a.btn.ghost', { href: promptsApi.rawUrl(id, p.latest_version), download: `${p.id}.md` }, 'Download .md'),
            button('Details…', openDetails, { kind: 'ghost' }),
            button('Publish…', openPublish, { kind: 'ghost' }),
            p.active_version != null ? button('Unpublish', () => void act(() => promptsApi.unpublish(id), 'Unpublished'), { kind: 'ghost' }) : null,
            button(archived ? 'Restore' : 'Archive', () => void act(() => (archived ? promptsApi.restore(id) : promptsApi.archive(id)), archived ? 'Restored' : 'Archived'), { kind: 'ghost' }),
          ),
        ),
        h('p.lede', null, `${KIND_NAME[p.kind]} · ${p.id}`),
      ),
      stale
        ? h('div.pr-bar', { role: 'status' }, badge('Changed on the hub', 'warn', true), button('Reload', reloadFromHub, { kind: 'ghost', size: 'sm' }))
        : null,
    );
    replace(
      barEl,
      p.active_version != null ? badge(`Live v${p.active_version}`, 'ok') : badge('Not published', 'muted'),
      badge(`Latest v${p.latest_version}`, 'muted'),
      p.score != null ? badge(`Score ${p.score}`, scoreTone(p.score)) : null,
      p.binding ? badge(`Binds ${p.binding}`, 'accent') : null,
      archived ? badge('Archived', 'muted') : null,
      dirtyFlag,
    );
  }

  function drawTabs(): void {
    replace(
      tabsEl,
      tabBar(
        [
          { key: 'edit', label: 'Edit' },
          { key: 'versions', label: 'Versions', count: detail?.versions.length ?? 0 },
          { key: 'history', label: 'History' },
          { key: 'rules', label: 'Rules' },
        ],
        tab,
        (key) => {
          tab = key;
          drawTabs();
          drawTab();
        },
      ),
    );
  }

  function drawTab(): void {
    edit = null;
    if (!detail) {
      replace(bodyEl, h('p.pr-note', null, loadError || 'Loading...'));
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
      rules = await promptsApi.rules();
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
    offPrompt();
    window.removeEventListener('beforeunload', onBeforeUnload);
    closeModal?.();
    root.replaceChildren();
  };
}

interface EditView {
  ta: HTMLTextAreaElement;
  diffEl: HTMLElement;
  preview: HTMLElement;
  summary: HTMLElement;
  findings: HTMLElement;
  saveErr: HTMLElement;
}
