// The prompt editor (docs/design/prompt-library.md, "Screens"): the markdown source with a live preview, the findings
// and score as you type, versions (live and best marked), compare with a line diff, publish, history and the rules.

import '../styles/prompt-editor.css';
import {
  ApiError,
  promptsApi,
  type PromptDetail,
  type PromptDiff,
  type PromptEvent,
  type PromptFinding,
  type PromptKind,
  type PromptReport,
  type PromptRules,
  type PromptSeverity,
  type PromptVersion,
  type PromptVersionMeta,
} from '../api';
import { ago, badge, button, field, input, modal, select, table, tabs, toast, type Column, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import { renderMarkdown } from '../ui/markdown';
import type { Cleanup } from './index';

const CHECK_MS = 400;
type TabKey = 'edit' | 'versions' | 'history' | 'rules';
const TABS: { key: TabKey; label: string }[] = [
  { key: 'edit', label: 'Edit' },
  { key: 'versions', label: 'Versions' },
  { key: 'history', label: 'History' },
  { key: 'rules', label: 'Rules' },
];
const SEVERITIES: PromptSeverity[] = ['error', 'warning', 'info'];
const SEVERITY_NAME: Record<PromptSeverity, string> = { error: 'Error', warning: 'Warning', info: 'Info' };
const SEVERITY_GROUP: Record<PromptSeverity, string> = { error: 'Errors', warning: 'Warnings', info: 'Infos' };
const SEVERITY_TONE: Record<PromptSeverity, Tone> = { error: 'danger', warning: 'warn', info: 'info' };
const SEVERITY_MARK: Record<PromptSeverity, string> = { error: '✕', warning: '▲', info: '●' };
const KIND_NAME: Record<PromptKind, string> = { system: 'System prompt', agents_md: 'AGENTS.md', fragment: 'Fragment' };
const DIFF_SIGN: Record<'same' | 'add' | 'del', string> = { same: ' ', add: '+', del: '−' };

/** The live pieces of the Edit tab, rebuilt whenever the tab is drawn. */
interface EditView {
  ta: HTMLTextAreaElement;
  gutter: HTMLElement;
  preview: HTMLElement;
  summary: HTMLElement;
  findings: HTMLElement;
}

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

  const headEl = h('div.pe-head');
  const tabsEl = h('div.pe-tabs');
  const bodyEl = h('div.pe-main');
  root.replaceChildren(h('div.prompt-editor', null, headEl, tabsEl, bodyEl));

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
    paintLines();
    paintReport();
  }

  function paintPreview(): void {
    if (edit) replace(edit.preview, renderMarkdown(body));
  }

  function paintLines(): void {
    if (!edit) return;
    const worst = new Map<number, PromptSeverity>();
    for (const f of report?.findings ?? []) {
      if (f.line < 1) continue;
      const cur = worst.get(f.line);
      if (!cur || SEVERITIES.indexOf(f.severity) < SEVERITIES.indexOf(cur)) worst.set(f.line, f.severity);
    }
    const count = Math.max(1, body.split('\n').length);
    replace(
      edit.gutter,
      Array.from({ length: count }, (_, i) => {
        const sev = worst.get(i + 1);
        return h('span', { class: sev ? `sev-${sev}` : null, title: sev ? SEVERITY_NAME[sev] : null }, String(i + 1));
      }),
    );
    edit.gutter.scrollTop = edit.ta.scrollTop;
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
      h('h2', null, 'Findings'),
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
    const { ta, gutter } = edit;
    const { start, end } = lineBounds(ta.value, n);
    ta.focus();
    ta.setSelectionRange(start, end);
    const lineHeight = parseFloat(getComputedStyle(ta).lineHeight) || 20;
    ta.scrollTop = Math.max(0, (n - 2) * lineHeight);
    gutter.scrollTop = ta.scrollTop;
  }

  function buildEdit(): void {
    const ta = h('textarea.pe-src', { spellcheck: 'false', wrap: 'off', 'aria-label': 'Prompt source in markdown', placeholder: 'Write the prompt in markdown' }) as HTMLTextAreaElement;
    ta.value = body;
    const gutter = h('div.pe-gutter', { 'aria-hidden': 'true' });
    const preview = h('div.pe-preview');
    const summary = h('div.pe-summary');
    const findings = h('div.pe-findings');
    edit = { ta, gutter, preview, summary, findings };
    ta.addEventListener('input', () => {
      body = ta.value;
      syncDirty();
      paintPreview();
      paintLines();
      scheduleCheck();
    });
    ta.addEventListener('scroll', () => {
      gutter.scrollTop = ta.scrollTop;
    });
    replace(
      bodyEl,
      h(
        'div.pe-edit',
        null,
        h(
          'div.pe-panes',
          null,
          h('div.pe-pane', null, h('div.pe-label', null, 'Markdown'), h('div.pe-code', null, gutter, ta)),
          h('div.pe-pane', null, h('div.pe-label', null, 'Preview'), preview),
        ),
        summary,
        findings,
      ),
    );
    paintPreview();
    paintLines();
    paintReport();
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
            'span.pe-vers',
            null,
            h('strong', null, `v${v.version}`),
            v.version === live ? badge('Live', 'ok', true) : null,
            v.version === best ? badge('Best', 'accent') : null,
            v.rules_version < current ? badge('stale', 'warn') : null,
          ),
      },
      { label: 'Note', cell: (v) => v.note || '—' },
      { label: 'Author', cell: (v) => v.author },
      { label: 'Created', cell: (v) => ago(v.created_ms) },
      { label: 'Score', num: true, cell: (v) => badge(String(v.score), scoreTone(v.score)) },
      { label: 'Errors / warnings / infos', cell: (v) => `${v.errors} / ${v.warnings} / ${v.infos}` },
      { label: '', cell: (v) => versionActions(v, live) },
    ];
    const sorted = [...d.versions].sort(newestFirst);
    replace(
      bodyEl,
      h(
        'div.pe-versions',
        null,
        viewing ? viewPanel(viewing) : null,
        picks.length ? pickBar() : null,
        diff ? comparePanel(diff) : null,
        sorted.length ? table(cols, sorted, undefined, (v) => String(v.version)) : h('p.pe-muted', null, 'No versions yet.'),
      ),
    );
  }

  function versionActions(v: PromptVersionMeta, live: number | null): HTMLElement {
    const picked = picks.includes(v.version);
    const publishLabel = v.version === live ? 'Live' : live != null && v.version < live ? 'Roll back' : 'Publish';
    return h(
      'div.pe-row-actions',
      null,
      button('View', () => void viewVersion(v.version), { size: 'sm' }),
      button(picked ? 'Unpick' : 'Compare', () => togglePick(v.version), { size: 'sm' }),
      button(publishLabel, () => void act(() => promptsApi.publish(id, v.version), `Published v${v.version}`), { size: 'sm', disabled: v.version === live }),
      button('Restore as new', () => void restoreIntoEditor(v.version), { size: 'sm' }),
    );
  }

  function pickBar(): HTMLElement {
    const text = picks.length === 1 ? `v${picks[0]} picked. Pick one more version to compare.` : '';
    return h(
      'div.pe-pickbar',
      null,
      text ? h('span.pe-muted', null, text) : null,
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
    return h(
      'section.pe-view',
      null,
      h(
        'div.pe-view-head',
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
      renderMarkdown(v.body),
    );
  }

  function comparePanel(d: PromptDiff): HTMLElement {
    const side = (v: PromptVersion): HTMLElement =>
      h(
        'div.pe-side',
        null,
        h('h3', null, `v${v.version}`),
        h(
          'div.pe-score',
          null,
          badge(`Score ${v.score}`, scoreTone(v.score)),
          badge(`${v.errors} errors`, v.errors ? 'danger' : 'muted'),
          badge(`${v.warnings} warnings`, v.warnings ? 'warn' : 'muted'),
          badge(`${v.infos} infos`, v.infos ? 'info' : 'muted'),
        ),
      );
    return h(
      'section.pe-compare',
      null,
      h(
        'div.pe-view-head',
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
      h('div.pe-sides', null, side(d.from), side(d.to)),
      h(
        'div.pe-diff',
        null,
        d.lines.map((l) =>
          h(
            'div',
            { class: `pe-line ${l.op}` },
            h('span.pe-ln', null, l.old == null ? '' : String(l.old)),
            h('span.pe-ln', null, l.new == null ? '' : String(l.new)),
            h('span.pe-sign', null, DIFF_SIGN[l.op]),
            h('code', null, l.text),
          ),
        ),
      ),
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
      replace(bodyEl, h('p.pe-muted', null, 'Loading history...'));
      try {
        events = (await promptsApi.history(id)).events;
      } catch (err) {
        if (alive && tab === 'history') replace(bodyEl, h('p.pe-error', { role: 'alert' }, messageOf(err)));
        return;
      }
      history = events;
      if (!alive || tab !== 'history') return;
    }
    const cols: Column<PromptEvent>[] = [
      { label: 'Action', cell: (e) => badge(e.action) },
      { label: 'Version', cell: (e) => (e.version == null ? '—' : `v${e.version}`) },
      { label: 'Actor', cell: (e) => e.actor },
      { label: 'When', cell: (e) => ago(e.at_ms) },
    ];
    replace(bodyEl, events.length ? table(cols, events, undefined, (e) => String(e.id)) : h('p.pe-muted', null, 'No history yet.'));
  }

  function buildRules(): void {
    if (!rules) {
      replace(bodyEl, h('p.pe-muted', null, 'Loading the rules...'));
      return;
    }
    const current = kind;
    const applies = rules.rules.filter((r) => r.kinds.length === 0 || r.kinds.includes(current));
    replace(
      bodyEl,
      h(
        'div.pe-rules',
        null,
        h(
          'p.pe-muted',
          null,
          `The checks that apply to ${KIND_NAME[current]}. Errors block publishing and cost 25 points each; warnings cost 8 and infos 2.`,
        ),
        SEVERITIES.map((sev) => {
          const list = applies.filter((r) => r.severity === sev);
          return h(
            'section.pe-rule-group',
            null,
            h('h2', null, `${SEVERITY_GROUP[sev]} (${list.length})`),
            list.length
              ? list.map((r) =>
                  h(
                    'article.pe-rule-card',
                    null,
                    h('div.pe-rule-head', null, h('h3', null, r.title), h('code', null, r.id)),
                    h('p', null, r.why),
                  ),
                )
              : h('p.pe-muted', null, 'None for this kind.'),
          );
        }),
      ),
    );
  }

  function openSave(): void {
    if (!detail) return;
    const note = input({ type: 'text', maxlength: 200, placeholder: 'What changed, in a few words', autocomplete: 'off' });
    const err = h('div.pe-error', { role: 'alert', hidden: true });
    const showError = (message: string, extra?: HTMLElement): void => {
      err.hidden = false;
      err.textContent = message;
      if (extra) err.append(extra);
    };
    const save: HTMLButtonElement = button(
      'Save version',
      async () => {
        save.disabled = true;
        try {
          const res = await promptsApi.addVersion(id, body, note.value.trim(), baseVersion);
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
            showError(
              'Someone saved a newer version while you were editing. Reloading replaces your text with the latest version.',
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
            showError(messageOf(e));
          }
          save.disabled = false;
        }
      },
      { kind: 'primary' },
    );
    openModal(
      'Save version',
      h('div.pe-form', null, field('Note', note, 'Optional. Shown in the version list.'), err, h('div.pe-actions', null, save)),
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

  /** Loads the latest version's body into the editor (replacing what is typed). */
  async function loadLatest(): Promise<void> {
    const next = await promptsApi.get(id);
    const latest = await promptsApi.version(id, next.prompt.latest_version);
    if (!alive) return;
    detail = next;
    kind = next.prompt.kind;
    baseVersion = next.prompt.latest_version;
    body = latest.body;
    savedBody = latest.body;
    history = null;
    drawHead();
    drawTabs();
    drawTab();
    void runCheck();
  }

  function drawHead(): void {
    if (!detail) {
      replace(headEl, h('div.pe-top', null, button('Back to prompts', goBack, { kind: 'ghost' })), h('p.pe-muted', null, loadError || 'Loading...'));
      return;
    }
    const p = detail.prompt;
    const archived = p.archived_ms != null;
    dirtyFlag = h('span.pe-dirty', { hidden: !dirty() }, badge('Unsaved changes', 'warn', true));
    replace(
      headEl,
      h(
        'div.pe-top',
        null,
        button('Back to prompts', goBack, { kind: 'ghost' }),
        h('a.btn', { href: promptsApi.rawUrl(id, p.latest_version), download: `${p.id}.md` }, 'Download .md'),
      ),
      h(
        'div.pe-title',
        null,
        h('h1', null, p.title || 'Untitled prompt'),
        h(
          'div.pe-chips',
          null,
          badge(KIND_NAME[p.kind]),
          p.binding ? badge(`Binds ${p.binding}`, 'accent') : null,
          p.active_version != null ? badge(`Live v${p.active_version}`, 'ok', true) : badge('Not published'),
          badge(`Latest v${p.latest_version}`),
          p.score != null ? badge(`Score ${p.score}`, scoreTone(p.score)) : null,
          archived ? badge('Archived') : null,
          dirtyFlag,
        ),
      ),
      h(
        'div.pe-actions',
        null,
        button('Save version', openSave, { kind: 'primary' }),
        button('Publish…', openPublish),
        p.active_version != null ? button('Unpublish', () => void act(() => promptsApi.unpublish(id), 'Unpublished')) : null,
        button(archived ? 'Restore' : 'Archive', () =>
          void act(() => (archived ? promptsApi.restore(id) : promptsApi.archive(id)), archived ? 'Restored' : 'Archived'),
        ),
      ),
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
      replace(bodyEl, h('p.pe-muted', null, loadError || 'Loading...'));
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
    window.removeEventListener('beforeunload', onBeforeUnload);
    closeModal?.();
    root.replaceChildren();
  };
}
