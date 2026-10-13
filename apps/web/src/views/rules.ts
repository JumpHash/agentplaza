// Rules: the plaza's laws (JHRL). Strict permits; the built-in statutes, each with its
// override and its recent refusals; the owner's rules, with an editor (check, save, enable,
// delete); and the refusals of every law.

import '../styles/rules.css';
import { api, ApiError, type Diagnostic, type Infraction, type LawCheck, type LawInfo, type LawOverride, type LawRule, type WouldRefuse } from '../api';
import { href } from '../router';
import { on } from '../store';
import { ago, attempt, badge, button, modal, field, input, select, table, tabs, toast, type Column } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import {
  EXAMPLE_LAW,
  SEVERITIES,
  STRICT_HELP,
  TIER_FILTERS,
  builtinLaws,
  cap,
  matchesFilter,
  type LawFilter,
  diagnosticsByLine,
  diagnosticText,
  effectLabel,
  enforcementLabel,
  errorText,
  isBlank,
  isOwnerLaw,
  lawsOfRule,
  lineSpan,
  severityTone,
  withStrictPermits,
  type TierKey,
} from './rules-model';

const VIOLATION_LIMIT = 20;
const RELOAD_MS = 1000;
const INFRACTION_FETCH = 200;
const LATEST_SHOWN = 20;
const DAY_MS = 86_400_000;
const EFFECTS: [LawFilter['effect'], string][] = [['all', 'All'], ['permit', 'Allows'], ['forbid', 'Forbids']];
const INTRO = "These are the plaza's laws. Every action an agent takes is checked against them before it happens; a law that forbids an action stops it, and the attempt is recorded as an infraction.";
const BUILTIN_DESC = 'The laws the plaza ships with. Disabling one asks for a reason, and the reason stays with the override until the statute is enabled again.';
const OWNER_DESC = 'Laws you write in JHRL. A rule takes effect once its source compiles; its laws stop applying when the rule is disabled or deleted.';
/** The tier of the constitution: switching one of its laws off needs the law's id typed in. */
const CONSTITUTION = 'constitution';
const REFERENCE = [
  'law "ID" "Title" { ... }: one law per block, with a unique ID',
  'tier statute (or constitution, operations)',
  'enforcement preventive or detective',
  'severity info, minor, major or critical',
  'statement "what the law says, in plain words"',
  'permit OP or forbid OP, where OP is a declared dotted operation such as tool.call',
  'principal is Agent and resource is Tool',
  'when <condition> (optional)',
  'rationale "why the law exists" (optional)',
];

type Status = 'loading' | 'ready' | 'error';
type TierFilter = 'all' | TierKey;

const plural = (n: number, one: string, many: string): string => `${n} ${n === 1 ? one : many}`;

export function mount(root: HTMLElement, params: string[]): Cleanup {
  let alive = true;
  let seq = 0;
  let timer = 0;
  const filter: LawFilter = { q: '', effect: 'all', sev: 'any', tier: 'all' };
  let infractions: Infraction[] = [];
  let infrStatus: Status = 'loading';
  let infrError = '';
  let laws: LawInfo[] = [];
  let lawsStatus: Status = 'loading';
  let lawsError = '';
  let rules: LawRule[] = [];
  let rulesStatus: Status = 'loading';
  let rulesError = '';
  let strict: boolean | null = null;
  let strictBusy = false;

  // Strict permits: the switch, its state and its help text.
  const strictInput = h('input', { type: 'checkbox', 'aria-describedby': 'rules-strict-help' }) as HTMLInputElement;
  const strictText = h('span', null, 'Unknown');
  const strictFail = h('p.rules-error', { role: 'alert', hidden: true });
  const strictBox = h(
    'section.rules-strict',
    { 'aria-labelledby': 'rules-strict-h' },
    h('h2', { id: 'rules-strict-h' }, 'Strict permits'),
    h('label.rules-switch', null, strictInput, strictText),
    h('p.rules-help', { id: 'rules-strict-help' }, STRICT_HELP),
    strictFail,
  );

  // The summary strip, the statutes' filters and the latest infractions.
  const strip = h('div.rules-strip');
  const search = input({ type: 'search', placeholder: 'Search by title, statement, id or operation', 'aria-label': 'Search statutes', autocomplete: 'off' });
  const sevSel = select([{ value: 'any', label: 'Any severity' }, ...SEVERITIES.map((s) => ({ value: s, label: cap(s) }))], 'any');
  const chipBox = h('div.rules-chips', { role: 'group', 'aria-label': 'Effect' });
  const infrBox = h('div.infr-box');

  // Built-in statutes.
  const tierTabs = h('div.rules-tabs');
  const builtinCount = h('p.rules-count');
  const builtinBox = h('div.rules-list');

  // Owner rules: the list, and the editor above it while one is open.
  const editorBox = h('div.rules-editor-box');
  const ownerBox = h('div.rules-list');
  const newButton = button('New rule', () => openEditor(null), { kind: 'primary' });

  // The warning banner for constitutional laws that are switched off; it stays while any is.
  const constitutionBanner = h('div.banner.warn.rules-constitution', { role: 'status', hidden: true });

  const offs = [on('violation', schedule), on('agent', schedule), on('law', schedule)];

  function renderStrict(): void {
    strictInput.checked = strict === true;
    strictInput.disabled = strict === null || strictBusy;
    strictText.textContent = strict === null ? 'Unknown' : strict ? 'On' : 'Off';
  }

  function renderTierTabs(): void {
    replace(
      tierTabs,
      tabs(TIER_FILTERS.map((t) => ({ key: t.key, label: t.label })), filter.tier, (key) => {
        filter.tier = key as TierFilter;
        renderTierTabs();
        renderBuiltin();
      }),
    );
  }

  function renderChips(): void {
    replace(
      chipBox,
      EFFECTS.map(([key, label]) =>
        h(
          'button.rules-chip',
          {
            type: 'button',
            'aria-pressed': String(filter.effect === key),
            onclick: () => {
              filter.effect = key;
              renderChips();
              renderBuiltin();
            },
          },
          label,
        ),
      ),
    );
  }

  function resetFilters(): void {
    filter.q = '';
    filter.effect = 'all';
    filter.sev = 'any';
    filter.tier = 'all';
    search.value = '';
    sevSel.value = 'any';
    renderChips();
    renderTierTabs();
    renderBuiltin();
  }

  function renderBuiltin(): void {
    const all = builtinLaws(laws, 'all');
    const shown = all.filter((l) => matchesFilter(l, filter));
    builtinCount.textContent = lawsStatus === 'ready' ? `Showing ${shown.length} of ${plural(all.length, 'statute', 'statutes')}` : '';
    if (lawsStatus === 'loading') {
      replace(builtinBox, h('p.rules-empty', null, 'Loading the statutes…'));
      return;
    }
    const empty = lawsStatus === 'ready' && shown.length === 0
      ? h(
        'div.rules-empty',
        null,
        h('p', null, all.length ? 'No statutes match these filters.' : 'No built-in statutes.'),
        all.length ? button('Clear filters', () => resetFilters(), { size: 'sm' }) : null,
      )
      : null;
    replace(builtinBox, failure(lawsStatus, lawsError, 'The statutes could not be loaded'), shown.map(lawRow), empty);
  }

  function renderStrip(): void {
    const ready = lawsStatus === 'ready';
    const forbids = laws.filter((l) => l.effect === 'forbid').length;
    const cut = Date.now() - 30 * DAY_MS;
    const n30 = infractions.filter((i) => i.at_ms >= cut).length;
    const n30Text = infractions.length >= INFRACTION_FETCH ? `${n30}+` : String(n30);
    replace(
      strip,
      cell('Laws', ready ? String(laws.length) : '—'),
      cell('Allows', ready ? String(laws.length - forbids) : '—'),
      cell('Forbids', ready ? String(forbids) : '—'),
      cell('Infractions, 30 days', infrStatus === 'ready' ? n30Text : '—'),
    );
  }

  function renderInfraction(i: Infraction): HTMLElement {
    return h(
      'li.infr-row',
      null,
      h(
        'div.infr-line',
        null,
        h('a.infr-agent', { href: href('agents', i.cid) }, i.agent_name || `agent ${i.cid}`),
        ' violated ',
        h('a', { href: href('rules', i.law) }, i.law_title || i.law),
      ),
      h(
        'div.infr-meta',
        null,
        i.tool ? h('code', null, i.tool) : null,
        i.detail ? h('span.infr-detail', null, i.detail) : null,
        h('time', null, ago(i.at_ms)),
      ),
    );
  }

  function renderInfractions(): void {
    if (infrStatus === 'loading') {
      replace(infrBox, h('p.rules-empty', null, 'Loading…'));
      return;
    }
    const latest = [...infractions].sort((a, b) => b.at_ms - a.at_ms).slice(0, LATEST_SHOWN);
    replace(
      infrBox,
      failure(infrStatus, infrError, 'Infractions could not be loaded'),
      latest.length ? h('ol.infr-list', null, latest.map(renderInfraction)) : (infrStatus === 'ready' ? h('p.rules-empty', null, 'No infractions yet.') : null),
    );
  }

  function renderOwner(): void {
    if (rulesStatus === 'loading') {
      replace(ownerBox, h('p.rules-empty', null, 'Loading the owner rules…'));
      return;
    }
    const empty = rulesStatus === 'ready' && rules.length === 0
      ? h('p.rules-empty', null, 'No owner rules yet. Write one with New rule.')
      : null;
    replace(ownerBox, failure(rulesStatus, rulesError, 'The owner rules could not be loaded'), rules.map(ruleCard), empty);
  }

  function renderAll(): void {
    renderStrict();
    renderConstitutionBanner();
    renderStrip();
    renderBuiltin();
    renderOwner();
    renderInfractions();
  }

  /** The banner while any constitutional law is switched off: each one a link to its card. */
  function renderConstitutionBanner(): void {
    const off = laws.filter((l) => l.constitutional_off);
    constitutionBanner.hidden = off.length === 0;
    replace(
      constitutionBanner,
      off.length === 0
        ? null
        : [
            h('strong', null, `${plural(off.length, 'constitutional law', 'constitutional laws')} ${off.length === 1 ? 'is' : 'are'} switched off`),
            h('span.rules-banner-links', null, off.map((l) => button(l.id, () => focusLaw(l.id), { kind: 'ghost', size: 'sm' }))),
          ],
    );
  }

  /** Brings one law into view and highlights it briefly; filters that hide a statute are cleared first. */
  function focusLaw(id: string): void {
    const law = laws.find((l) => l.id === id);
    if (!law) return;
    if (!isOwnerLaw(law) && !matchesFilter(law, filter)) resetFilters();
    const el = document.getElementById(`law-${id}`);
    if (!el) return;
    el.scrollIntoView({ behavior: 'smooth', block: 'center' });
    el.classList.add('flash');
    window.setTimeout(() => el.classList.remove('flash'), 2200);
  }

  async function load(): Promise<void> {
    const mine = ++seq;
    const [l, r, i] = await Promise.allSettled([api.laws(), api.lawRules(), api.infractions(INFRACTION_FETCH)]);
    if (!alive || mine !== seq) return;
    if (i.status === 'fulfilled') {
      infractions = i.value;
      infrStatus = 'ready';
      infrError = '';
    } else {
      infrStatus = 'error';
      infrError = errorText(i.reason);
    }
    if (l.status === 'fulfilled') {
      laws = l.value;
      lawsStatus = 'ready';
      lawsError = '';
    } else {
      lawsStatus = 'error';
      lawsError = errorText(l.reason);
    }
    if (r.status === 'fulfilled') {
      rules = r.value.rules;
      strict = r.value.strict_permits;
      rulesStatus = 'ready';
      rulesError = '';
    } else {
      rulesStatus = 'error';
      rulesError = errorText(r.reason);
    }
    renderAll();
  }

  function schedule(): void {
    window.clearTimeout(timer);
    timer = window.setTimeout(() => void load(), RELOAD_MS);
  }

  async function setStrict(on: boolean): Promise<void> {
    strictBusy = true;
    strictFail.hidden = true;
    renderStrict();
    try {
      // The whole settings document is saved, so the current one is read first.
      const current = await api.settings();
      await api.saveSettings(withStrictPermits(current, on));
      strict = on;
      toast(on ? 'Strict permits on' : 'Strict permits off');
    } catch (err) {
      strictFail.hidden = false;
      strictFail.textContent = `Strict permits were not changed: ${errorText(err)}`;
    } finally {
      strictBusy = false;
      if (alive) renderStrict();
    }
  }

  function failure(status: Status, message: string, what: string): HTMLElement | null {
    if (status !== 'error') return null;
    return h(
      'div.rules-error',
      { role: 'alert' },
      h('p', null, `${what}: ${message}`),
      button('Try again', () => void load(), { size: 'sm' }),
    );
  }

  // ---- Statutes and owner laws ------------------------------------------

  /** One law as a card. Built-in statutes carry their override; owner laws follow their rule. */
  function lawRow(l: LawInfo): HTMLElement {
    const owner = isOwnerLaw(l);
    const forbid = l.effect === 'forbid';
    const pills: HTMLElement[] = [
      badge(effectLabel(l.effect), forbid ? 'danger' : 'ok'),
      badge(`Severity: ${cap(l.severity)}`, severityTone(l.severity)),
      badge(enforcementLabel(l.enforcement), l.enforcement === 'preventive' ? 'info' : 'muted'),
    ];
    if (!l.enabled) pills.push(badge(owner ? 'Rule disabled' : 'Disabled', 'warn'));
    return h(
      'article.law',
      { id: `law-${l.id}`, 'data-key': l.id, class: `${forbid ? 'forbid' : 'permit'}${owner ? ' owner' : ''}${l.enabled ? '' : ' off'}` },
      h('header.law-head', null, h('div.law-title', null, h('h3', null, l.title), h('span.law-id', null, l.id)), h('div.law-pills', null, pills)),
      h('p.law-statement', null, l.statement),
      l.explanation ? h('p.law-short', null, h('strong', null, 'In short: '), l.explanation) : null,
      l.rationale ? h('p.law-why', null, h('strong', null, 'Why: '), l.rationale) : null,
      h(
        'div.law-applies',
        null,
        h('span.law-label', null, 'Applies to'),
        h('span.law-scope', null, `${l.principal || 'any principal'} → ${l.resource || 'any resource'}`),
        l.ops.length ? h('span.law-ops', null, l.ops.map((o) => h('code.op', null, o))) : null,
      ),
      refusalLine(l),
      owner ? null : overrideBox(l),
      recentBox(l),
      h(
        'details.law-how',
        null,
        h('summary', null, 'How it is written (JHRL)'),
        h('p.law-file', null, `File: ${l.file || '—'}`),
        l.conditions.length ? h('ul.law-conds', null, l.conditions.map((c) => h('li', null, c))) : h('p.law-file', null, 'No extra conditions.'),
        h('pre.law-source', null, l.source || '—'),
      ),
    );
  }

  function refusalLine(l: LawInfo): HTMLElement {
    if (!l.refusals) return h('p.law-usage', null, 'Not refused yet');
    return h(
      'p.law-usage',
      null,
      `Refused ${plural(l.refusals, 'time', 'times')}`,
      l.last_at_ms ? ` · last ${ago(l.last_at_ms)}` : '',
      l.agents.length ? ' · by ' : '',
      l.agents.flatMap((a, i) => [i ? ', ' : '', h('a', { href: href('agents', a.cid) }, a.name), ` (${a.count})`]),
    );
  }

  /** The recent refusals of one law, loaded when its expander opens. */
  function recentBox(l: LawInfo): HTMLElement {
    const body = h('div.law-recent-body', null, h('p.law-usage', null, 'Open to see the recent refusals.'));
    const box = h('details.law-how.law-recent', null, h('summary', null, `Recent refusals (${l.refusals})`), body) as HTMLDetailsElement;
    let loaded = false;
    box.addEventListener('toggle', () => {
      if (!box.open || loaded) return;
      loaded = true;
      replace(body, h('p.law-usage', null, 'Loading…'));
      api.lawViolations(l.id, VIOLATION_LIMIT).then(
        (rows) => {
          if (alive) replace(body, rows.length ? h('ol.infr-list', null, rows.map(violationRow)) : h('p.law-usage', null, 'No refusals recorded.'));
        },
        (err: unknown) => {
          loaded = false;
          if (alive) replace(body, h('p.rules-error', { role: 'alert' }, errorText(err)));
        },
      );
    });
    return box;
  }

  function violationRow(i: Infraction): HTMLElement {
    return h(
      'li.infr-row',
      null,
      h('div.infr-line', null, h('a.infr-agent', { href: href('agents', i.cid) }, i.agent_name || `agent ${i.cid}`), ' ', i.tool ? h('code', null, i.tool) : null),
      h('div.infr-meta', null, h('time', null, ago(i.at_ms)), h('span', null, i.law_title || i.law), i.detail ? h('span.infr-detail', null, i.detail) : null),
    );
  }

  function overrideBox(l: LawInfo): HTMLElement {
    const o: LawOverride | null = l.overridden;
    if (!o) return h('div.rules-override', null, button('Disable', () => askDisable(l), { size: 'sm' }));
    return h(
      'div.rules-override',
      null,
      h('p', null, h('strong', null, 'Disabled'), ` by ${o.disabled_by || 'the owner'}, ${ago(o.at_ms)}: ${o.reason || 'no reason given'}`),
      button('Enable', () => void enableStatute(l), { size: 'sm' }),
    );
  }

  function askDisable(l: LawInfo): void {
    if (l.tier === CONSTITUTION) return askConstitutional(l);
    const reason = h('input.input', { type: 'text', maxlength: 300, autocomplete: 'off', placeholder: 'Why this statute is off', 'aria-label': 'Reason' }) as HTMLInputElement;
    const confirm = button('Disable statute', () => void disable(), { kind: 'danger' });
    confirm.disabled = true;
    reason.addEventListener('input', () => {
      confirm.disabled = isBlank(reason.value);
    });
    let close: () => void = () => {};
    async function disable(): Promise<void> {
      if (isBlank(reason.value)) return;
      confirm.disabled = true;
      const done = await attempt(() => api.setLawOverride(l.id, true, reason.value.trim()), 'Statute disabled');
      if (done) {
        close();
        void load();
      } else {
        confirm.disabled = false;
      }
    }
    close = modal(
      `Disable ${l.title}`,
      h(
        'div.stack',
        null,
        h('p', null, `${l.id} stops applying until it is enabled again. The reason is kept with the override.`),
        field('Reason (required)', reason),
        h('div.rules-actions', null, button('Cancel', () => close(), { kind: 'ghost' }), confirm),
      ),
    );
  }

  /** Switching a constitutional law off: what it protects, its title and summary, a reason, and the id typed in. */
  function askConstitutional(l: LawInfo): void {
    const reason = h('input.input', { type: 'text', maxlength: 300, autocomplete: 'off', placeholder: 'Why this law is off', 'aria-label': 'Reason' }) as HTMLInputElement;
    const typed = h('input.input', { type: 'text', autocomplete: 'off', spellcheck: false, placeholder: l.id, 'aria-label': `Type ${l.id} to confirm` }) as HTMLInputElement;
    const confirm = button('Switch off constitutional law', () => void disable(), { kind: 'danger' });
    confirm.disabled = true;
    const ready = (): boolean => typed.value.trim() === l.id && !isBlank(reason.value);
    const sync = (): void => {
      confirm.disabled = !ready();
    };
    reason.addEventListener('input', sync);
    typed.addEventListener('input', sync);
    let close: () => void = () => {};
    async function disable(): Promise<void> {
      if (!ready()) return;
      confirm.disabled = true;
      const done = await attempt(() => api.setLawOverride(l.id, true, reason.value.trim(), typed.value.trim()), `${l.id} switched off`);
      if (done) {
        close();
        void load();
      } else {
        sync();
      }
    }
    close = modal(
      `Switch off "${l.title}"?`,
      h(
        'div.stack',
        null,
        h('p', null, `${l.id} is a constitutional law. It protects:`),
        h('blockquote.law-quote', null, l.statement || l.title),
        l.rationale ? h('p.law-why', null, l.rationale) : null,
        h('p', null, 'While it is off, agents may do what it forbids. The change is recorded with the time, and this screen flags it until the law is switched back on.'),
        field('Reason (required)', reason),
        field("Type the law's id to confirm", typed, `Exactly ${l.id}, as written`),
        h('div.rules-actions', null, button('Cancel', () => close(), { kind: 'ghost' }), confirm),
      ),
    );
  }

  async function enableStatute(l: LawInfo): Promise<void> {
    const done = await attempt(() => api.setLawOverride(l.id, false, ''), 'Statute enabled');
    if (done) void load();
  }

  // ---- Owner rules --------------------------------------------------------

  function ruleCard(rule: LawRule): HTMLElement {
    const declared = lawsOfRule(laws, rule.id);
    return h(
      'article.law.owner',
      { id: `rule-${rule.id}`, 'data-key': rule.id, class: rule.enabled ? '' : 'off' },
      h(
        'header.law-head',
        null,
        h('div.law-title', null, h('h3', null, rule.title), h('span.law-id', null, rule.id)),
        h('div.law-pills', null, badge(rule.enabled ? 'Enabled' : 'Disabled', rule.enabled ? 'ok' : 'muted'), badge(plural(declared.length, 'law', 'laws'))),
      ),
      h('p.law-usage', null, `Updated ${ago(rule.updated_ms)}${rule.author ? ` by ${rule.author}` : ''}`),
      h(
        'div.rules-actions',
        null,
        button('Edit', () => openEditor(rule), { size: 'sm' }),
        button(rule.enabled ? 'Disable' : 'Enable', () => void toggleRule(rule), { size: 'sm' }),
        button('Delete', () => askDelete(rule), { size: 'sm', kind: 'danger' }),
      ),
      declared.length ? h('div.law-declared', null, declared.map(lawRow)) : h('p.law-usage', null, 'Declares no laws.'),
    );
  }

  async function toggleRule(rule: LawRule): Promise<void> {
    const done = await attempt(() => api.setLawRuleEnabled(rule.id, !rule.enabled), rule.enabled ? 'Rule disabled' : 'Rule enabled');
    if (done) void load();
  }

  function askDelete(rule: LawRule): void {
    const confirm = button('Delete rule', () => void remove(), { kind: 'danger' });
    let close: () => void = () => {};
    async function remove(): Promise<void> {
      confirm.disabled = true;
      const done = await attempt(() => api.deleteLawRule(rule.id), 'Rule deleted');
      if (done) {
        close();
        if (editorBox.querySelector('.rules-editor')?.getAttribute('data-rule') === rule.id) closeEditor();
        void load();
      } else {
        confirm.disabled = false;
      }
    }
    close = modal(
      `Delete ${rule.title}`,
      h(
        'div.stack',
        null,
        h('p', null, `Delete ${rule.title} (${rule.id})? Its laws stop applying. This cannot be undone.`),
        h('div.rules-actions', null, button('Cancel', () => close(), { kind: 'ghost' }), confirm),
      ),
    );
  }

  function openEditor(rule: LawRule | null): void {
    replace(editorBox, buildEditor(rule));
    editorBox.scrollIntoView({ behavior: 'smooth', block: 'start' });
    editorBox.querySelector<HTMLInputElement>('input')?.focus();
  }

  function closeEditor(): void {
    replace(editorBox);
  }

  /** The editor: a title, the JHRL source with line numbers, the reference, and Check / Save. */
  function buildEditor(rule: LawRule | null): HTMLElement {
    const titleIn = h('input.input', { type: 'text', maxlength: 120, autocomplete: 'off', placeholder: 'What the rule is for', 'aria-label': 'Rule title', value: rule?.title ?? '' }) as HTMLInputElement;
    const gutter = h('div.rules-gutter', { 'aria-hidden': 'true' });
    const src = h('textarea.rules-src', { spellcheck: 'false', wrap: 'off', rows: 16, 'aria-label': 'Rule source (JHRL)', placeholder: 'law "O-001" "Title" { ... }' }) as HTMLTextAreaElement;
    src.value = rule?.source ?? '';
    const fail = h('p.rules-error', { role: 'alert', hidden: true });
    const result = h('div.rules-check', { role: 'status' });
    const checkButton = button('Check', () => void check(), {});
    const saveButton = button('Save', () => void save(), { kind: 'primary' });
    let diags: Diagnostic[] = [];

    const setBusy = (busy: boolean): void => {
      checkButton.disabled = busy;
      saveButton.disabled = busy;
    };
    const showFail = (message: string): void => {
      fail.hidden = message === '';
      fail.textContent = message;
    };
    const paint = (): void => {
      const marks = diagnosticsByLine(diags);
      const count = Math.max(16, src.value.split('\n').length);
      replace(
        gutter,
        Array.from({ length: count }, (_, i) => {
          const line = i + 1;
          const msg = marks.get(line);
          return h('span', { class: msg ? 'bad' : '', title: msg ?? null }, String(line));
        }),
      );
      gutter.scrollTop = src.scrollTop;
    };
    const pick = (d: Diagnostic): void => {
      const { start, end } = lineSpan(src.value, d.line);
      src.focus();
      src.setSelectionRange(start, end);
      const lineHeight = parseFloat(getComputedStyle(src).lineHeight) || 20;
      src.scrollTop = Math.max(0, (d.line - 2) * lineHeight);
      gutter.scrollTop = src.scrollTop;
    };
    const showDiags = (list: Diagnostic[]): void => {
      diags = list;
      paint();
      replace(
        result,
        list.length
          ? h('div.rules-check-bad', null, h('h4', null, `Does not compile: ${plural(list.length, 'problem', 'problems')}`), diagnosticList(list, pick))
          : null,
      );
    };

    src.addEventListener('input', () => {
      // Results describe the source they were checked on; an edit clears them.
      diags = [];
      result.replaceChildren();
      paint();
    });
    src.addEventListener('scroll', () => {
      gutter.scrollTop = src.scrollTop;
    });

    async function check(): Promise<void> {
      setBusy(true);
      showFail('');
      try {
        const r = await api.checkLaw(src.value, rule?.id);
        if (r.ok) {
          diags = [];
          paint();
          replace(result, checkOk(r));
        } else {
          showDiags(r.diagnostics);
        }
      } catch (err) {
        replace(result);
        showFail(errorText(err));
      } finally {
        setBusy(false);
      }
    }

    async function save(): Promise<void> {
      const title = titleIn.value.trim();
      if (isBlank(title)) {
        showFail('Give the rule a title.');
        titleIn.focus();
        return;
      }
      setBusy(true);
      showFail('');
      try {
        const body = { title, source: src.value };
        if (rule) await api.updateLawRule(rule.id, body);
        else await api.createLawRule(body);
        toast(rule ? 'Rule saved' : 'Rule created');
        closeEditor();
        void load();
      } catch (err) {
        showFail(errorText(err));
        if (err instanceof ApiError && err.diagnostics.length) showDiags(err.diagnostics);
      } finally {
        setBusy(false);
      }
    }

    paint();
    return h(
      'section.rules-editor',
      { 'data-rule': rule?.id ?? null, 'aria-label': rule ? `Edit ${rule.title}` : 'New rule' },
      h('header.rules-editor-head', null, h('h3', null, rule ? `Edit ${rule.title}` : 'New rule'), rule ? h('span.law-id', null, rule.id) : null),
      field('Title', titleIn),
      h('div.field', null, h('span', null, 'Source (JHRL)'), h('div.rules-code', null, gutter, src)),
      jhrlReference(),
      fail,
      h('div.rules-editor-actions', null, checkButton, saveButton, button('Cancel', () => closeEditor(), { kind: 'ghost' })),
      result,
    );
  }

  // ---- Strict permits, the switch -------------------------------------------

  strictInput.addEventListener('change', () => void setStrict(strictInput.checked));

  root.append(
    h(
      'div.rules-view',
      null,
      h('div.view-head', null, h('h1', null, 'Rules')),
      constitutionBanner,
      h('div.rules-intro', null, h('p', null, INTRO)),
      strip,
      h(
        'div.rules-grid',
        null,
        h(
          'div.rules-main',
          null,
          strictBox,
          h(
            'section.rules-section',
            { id: 'rules-builtin', 'aria-labelledby': 'rules-builtin-h' },
            h('header.rules-section-head', null, h('h2', { id: 'rules-builtin-h' }, 'Built-in statutes')),
            h('p.rules-section-desc', null, BUILTIN_DESC),
            h(
              'div.rules-toolbar',
              null,
              h('div.rules-row', null, search, field('Severity', sevSel)),
              h('div.rules-row', null, chipBox, tierTabs),
            ),
            builtinCount,
            builtinBox,
          ),
          h(
            'section.rules-section',
            { id: 'rules-owner', 'aria-labelledby': 'rules-owner-h' },
            h('header.rules-section-head', null, h('h2', { id: 'rules-owner-h' }, 'Owner rules'), h('span.rules-section-action', null, newButton)),
            h('p.rules-section-desc', null, OWNER_DESC),
            editorBox,
            ownerBox,
          ),
        ),
        h('aside.rules-side', null, h('section.infr-panel', null, h('h2', null, 'Latest infractions'), infrBox)),
      ),
    ),
  );

  search.addEventListener('input', () => {
    filter.q = search.value;
    renderBuiltin();
  });
  sevSel.addEventListener('change', () => {
    filter.sev = sevSel.value;
    renderBuiltin();
  });

  const onHash = (): void => {
    const [screen, id] = location.hash.replace(/^#\/?/, '').split('/');
    if (screen === 'rules' && id) focusLaw(decodeURIComponent(id));
  };
  window.addEventListener('hashchange', onHash);

  renderChips();
  renderTierTabs();
  renderAll();
  void load().then(() => {
    if (params[0]) focusLaw(params[0]);
  });

  return () => {
    alive = false;
    window.clearTimeout(timer);
    offs.forEach((off) => off());
    window.removeEventListener('hashchange', onHash);
  };
}

// ---- Pieces that need no state ---------------------------------------------

function cell(label: string, value: string): HTMLElement {
  return h('div.rules-stat', null, h('span.rules-stat-label', null, label), h('span.rules-stat-value', null, value));
}

function diagnosticList(diags: Diagnostic[], pick: (d: Diagnostic) => void): HTMLElement {
  return h(
    'ol.rules-diags',
    null,
    diags.map((d) => h('li', null, h('button.rules-diag', { type: 'button', title: diagnosticText(d), onclick: () => pick(d) }, `Line ${d.line}, column ${d.column}`), h('span', null, ` ${d.message}`))),
  );
}

const REFUSAL_COLUMNS: Column<WouldRefuse>[] = [
  { label: 'When', cell: (w) => ago(w.at_ms) },
  { label: 'Agent', cell: (w) => h('a', { href: href('agents', w.cid) }, `agent ${w.cid}`) },
  { label: 'Role', cell: (w) => w.role },
  { label: 'Tool', cell: (w) => h('code', null, w.tool) },
  { label: 'Laws', cell: (w) => w.laws.join(', ') },
];

function checkOk(r: LawCheck): HTMLElement {
  return h(
    'div.rules-check-ok',
    null,
    h('h4', null, 'Compiles'),
    h('p', null, 'Declares: ', r.laws.length ? r.laws.flatMap((id, i) => [i ? ', ' : '', h('code', null, id)]) : 'no laws'),
    h('h4', null, 'Would have refused, recently'),
    r.would_refuse.length
      ? h('div.rules-scroll', null, table(REFUSAL_COLUMNS, r.would_refuse))
      : h('p.law-usage', null, 'None of the recent calls would be refused.'),
  );
}

function jhrlReference(): HTMLElement {
  return h(
    'details.rules-ref',
    { open: true },
    h('summary', null, 'JHRL reference'),
    h('ul', null, REFERENCE.map((line) => h('li', null, line))),
    h('pre.law-source', null, EXAMPLE_LAW),
  );
}
