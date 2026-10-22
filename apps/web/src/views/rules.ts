// Rules: the plaza's laws (JHRL), as the prototype's Rules screen. The summary strip; the strict permits
// switch; the built-in statutes, each with its override and its refusals; the owner's rules, with an
// editor (check, save, enable, delete).

import { agentLabelIn, agentName } from '../roles';
import '../styles/proto/social.css';
import { api, ApiError, type Diagnostic, type LawCheck, type LawInfo, type LawOverride, type LawRule, type WouldRefuse } from '../api';
import { href } from '../router';
import { agents, on } from '../store';
import { attempt, badge, button, field, input, modal, select, stat, table, tabs, toast, type Column } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import {
  EXAMPLE_LAW,
  SEVERITIES,
  STRICT_HELP,
  TIER_DESCRIPTIONS,
  TIER_FILTERS,
  TIER_LABELS,
  TIER_ORDER,
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
  tierKey,
  withStrictPermits,
  type TierKey,
} from './rules-model';

const RELOAD_MS = 1000;
const EFFECTS: [LawFilter['effect'], string][] = [['all', 'All'], ['permit', 'Allows'], ['forbid', 'Forbids']];
const BUILTIN_DESC = 'The laws the plaza ships with. Disabling one asks for a reason, and the reason stays with the override until the statute is enabled again.';
const OWNER_DESC = 'Laws you write in JHRL. A rule takes effect once its source compiles; its laws stop applying when the rule is disabled or deleted.';
/** The tier of the constitution: switching one of its laws off needs the law's id typed in. */
const CONSTITUTION = 'constitution';

type Status = 'loading' | 'ready' | 'error';
type TierFilter = 'all' | TierKey;

const plural = (n: number, one: string, many: string): string => `${n} ${n === 1 ? one : many}`;

/** A time as the prototype says it: "just now", "4 min ago", "3 h ago", "2 d ago". */
function ago(ms: number | null | undefined, now = Date.now()): string {
  if (!ms) return '—';
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 45) return 'just now';
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86400)} d ago`;
}

/** A line count for a source: at least sixteen rows, and one more than its lines, as the prototype sizes it. */
const rowsFor = (text: string): number => Math.max(16, text.split('\n').length + 1);

export function mount(root: HTMLElement, params: string[]): Cleanup {
  let alive = true;
  let seq = 0;
  let timer = 0;
  const filter: LawFilter = { q: '', effect: 'all', sev: 'any', tier: 'all' };
  let laws: LawInfo[] = [];
  let lawsStatus: Status = 'loading';
  let lawsError = '';
  let rules: LawRule[] = [];
  let rulesStatus: Status = 'loading';
  let rulesError = '';
  let strict: boolean | null = null;
  let strictBusy = false;

  // Strict permits: the switch, its state and its help text.
  const strictInput = h('input', { type: 'checkbox', role: 'switch', 'aria-describedby': 'rules-strict-help' }) as HTMLInputElement;
  const strictText = h('span', null, 'Unknown');
  const strictFail = h('p.rl-err', { role: 'alert', hidden: true });
  const strictSec = h(
    'section.rl-card',
    { id: 'rules-strict' },
    h('div.rl-head', null, h('h2', null, 'Strict permits'), h('label.rules-switch', null, strictInput, ' ', strictText)),
    h('p.hint', { id: 'rules-strict-help' }, STRICT_HELP),
    strictFail,
  );

  // The summary strip and the statutes' filters.
  const strip = h('div.rl-strip');
  const search = input({ type: 'search', placeholder: 'Search by title, statement, id or operation', 'aria-label': 'Search statutes', autocomplete: 'off' });
  const sevSel = select([{ value: 'any', label: 'Any severity' }, ...SEVERITIES.map((s) => ({ value: s, label: cap(s) }))], 'any');
  sevSel.setAttribute('aria-label', 'Severity');
  const chipBox = h('div.rl-chips', { role: 'group', 'aria-label': 'Effect' });

  // Built-in statutes.
  const tierTabs = h('div.rl-slot');
  const builtinCount = h('p.hint');
  const builtinBox = h('div.rl-slot');
  const builtinSec = h(
    'section.rl-card',
    { id: 'rules-builtin' },
    h('h2', null, 'Built-in statutes'),
    h('p.hint', null, BUILTIN_DESC),
    h('div.rl-bar', null, search, sevSel),
    h('div.rl-bar', null, chipBox, tierTabs),
    builtinCount,
    builtinBox,
  );

  // Owner rules: the list, and the editor above it while one is open.
  const editorBox = h('div.rl-slot');
  const ownerBox = h('div.rl-slot');
  const newButton = button('New rule', () => openEditor(null), { kind: 'primary', size: 'sm' });
  const ownerSec = h(
    'section.rl-card',
    { id: 'rules-owner' },
    h('div.rl-head', null, h('h2', null, 'Owner rules'), newButton),
    h('p.hint', null, OWNER_DESC),
    editorBox,
    ownerBox,
  );

  // The banner for constitutional laws that are switched off; it stays while any is.
  const constitutionBanner = h('div.rl-banner', { role: 'status', hidden: true });

  const offs = [on('agent', schedule), on('law', schedule)];

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
          'button.btn.sm',
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
      replace(builtinBox, h('p.muted', null, 'Loading the statutes…'));
      return;
    }
    // The statutes in their tiers' groups, in the order the desktop groups them.
    const groups: HTMLElement[] = [];
    for (const key of TIER_ORDER) {
      const items = shown.filter((l) => tierKey(l) === key);
      if (items.length === 0) continue;
      groups.push(
        h(
          'section.rl-group',
          null,
          h('h3', null, TIER_LABELS[key], ' ', h('span.muted', null, String(items.length))),
          TIER_DESCRIPTIONS[key] ? h('p.hint', null, TIER_DESCRIPTIONS[key]) : null,
          items.map(lawRow),
        ),
      );
    }
    let empty: HTMLElement | null = null;
    if (lawsStatus === 'ready' && shown.length === 0) {
      empty = all.length
        ? h('div.empty', null, h('p', null, 'No statutes match these filters.'), button('Clear filters', () => resetFilters(), { kind: 'ghost', size: 'sm' }))
        : h('p.muted', null, 'No built-in statutes.');
    }
    replace(builtinBox, failure(lawsStatus, lawsError, 'The statutes could not be loaded'), groups, empty);
  }

  function renderStrip(): void {
    const ready = lawsStatus === 'ready';
    const forbids = laws.filter((l) => l.effect === 'forbid').length;
    replace(
      strip,
      stat('Laws', ready ? String(laws.length) : '—'),
      stat('Allows', ready ? String(laws.length - forbids) : '—'),
      stat('Forbids', ready ? String(forbids) : '—'),
    );
  }

  function renderOwner(): void {
    if (rulesStatus === 'loading') {
      replace(ownerBox, h('p.muted', null, 'Loading the owner rules…'));
      return;
    }
    const empty = rulesStatus === 'ready' && rules.length === 0 ? h('p.muted', null, 'No owner rules yet. Write one with New rule.') : null;
    replace(ownerBox, failure(rulesStatus, rulesError, 'The owner rules could not be loaded'), rules.map(ruleCard), empty);
  }

  function renderAll(): void {
    renderStrict();
    renderConstitutionBanner();
    renderStrip();
    renderBuiltin();
    renderOwner();
  }

  /** The banner while any constitutional law is switched off: each one a button that brings it into view. */
  function renderConstitutionBanner(): void {
    const off = laws.filter((l) => l.constitutional_off);
    constitutionBanner.hidden = off.length === 0;
    replace(
      constitutionBanner,
      off.length === 0
        ? null
        : [
            h('p', null, `${plural(off.length, 'constitutional law', 'constitutional laws')} ${off.length === 1 ? 'is' : 'are'} switched off`),
            ...off.map((l) => button(l.id, () => focusLaw(l.id), { kind: 'ghost', size: 'sm' })),
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
    const [l, r] = await Promise.allSettled([api.laws(), api.lawRules()]);
    if (!alive || mine !== seq) return;
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
      'div',
      { role: 'alert' },
      h('p.rl-err', null, `${what}: ${message}`),
      button('Try again', () => void load(), { size: 'sm' }),
    );
  }

  // ---- Statutes and owner laws ------------------------------------------

  /** One law as a card. Built-in statutes carry their override; owner laws follow their rule. */
  function lawRow(l: LawInfo): HTMLElement {
    const owner = isOwnerLaw(l);
    const forbid = l.effect === 'forbid';
    const off = !l.enabled || l.constitutional_off;
    const pills: HTMLElement[] = [
      badge(effectLabel(l.effect), forbid ? 'danger' : 'ok'),
      badge(`Severity: ${cap(l.severity)}`, severityTone(l.severity)),
      badge(enforcementLabel(l.enforcement), l.enforcement === 'preventive' ? 'info' : 'muted'),
    ];
    if (off) pills.push(badge(owner ? 'Rule disabled' : 'Disabled', 'warn'));
    return h(
      'article.rl-law',
      { id: `law-${l.id}`, 'data-key': l.id, class: off ? 'off' : '' },
      h('div.rl-head', null, h('h3', null, l.title), h('span.law-id', null, l.id)),
      h('div.rl-pills', null, pills),
      h('p', null, l.statement),
      l.explanation ? h('p.hint', null, h('b', null, 'In short:'), ' ', l.explanation) : null,
      l.rationale ? h('p.hint', null, h('b', null, 'Why:'), ' ', l.rationale) : null,
      h(
        'p.hint',
        null,
        `Applies to ${l.principal || 'any principal'} → ${l.resource || 'any resource'} `,
        l.ops.flatMap((o, i) => [i ? ' ' : '', h('code', null, o)]),
      ),
      refusalLine(l),
      owner ? null : overrideBlock(l),
      h(
        'details',
        null,
        h('summary', null, 'How it is written (JHRL)'),
        h('p.hint', null, `File: ${l.file || '—'}`),
        l.conditions.length ? h('ul', null, l.conditions.map((c) => h('li', null, h('code', null, c)))) : h('p.hint', null, 'No extra conditions.'),
        h('pre.code', null, l.source || '—'),
      ),
    );
  }

  function refusalLine(l: LawInfo): HTMLElement {
    const n = l.refusals;
    return h(
      'p.hint',
      null,
      n ? `Refused ${plural(n, 'time', 'times')}` : 'Not refused yet',
      l.last_at_ms ? ` · last ${ago(l.last_at_ms)}` : '',
      l.agents.length ? ' · by ' : '',
      l.agents.flatMap((a, i) => [i ? ', ' : '', h('a', { href: href('sessions', a.cid) }, agentLabelIn(agents.get(), a.cid)), ` (${a.count})`]),
    );
  }

  /** A built-in statute's override: the reason it is off and who turned it off, with Enable; or Disable. */
  function overrideBlock(l: LawInfo): HTMLElement[] {
    const o: LawOverride | null = l.overridden;
    if (!o) return [h('div.rl-acts', null, button('Disable', () => askDisable(l), { size: 'sm' }))];
    return [
      h('p.hint', null, `Disabled by ${o.disabled_by || 'the owner'}, ${ago(o.at_ms)}: ${o.reason || 'no reason given'}`),
      h('div.rl-acts', null, button('Enable', () => void enableStatute(l), { size: 'sm' })),
    ];
  }

  function askDisable(l: LawInfo): void {
    if (l.tier === CONSTITUTION) return askConstitutional(l);
    const reason = h('input.input', { type: 'text', id: 'rl-reason', maxlength: 300, autocomplete: 'off', placeholder: 'Why this statute is off', 'aria-label': 'Reason' }) as HTMLInputElement;
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
      h('div.stack', null, h('p', null, `${l.id} stops applying until it is enabled again. The reason is kept with the override.`), field('Reason (required)', reason)),
      undefined,
      [button('Cancel', () => close(), { kind: 'ghost' }), confirm],
    );
  }

  /** Switching a constitutional law off: what it protects, its title and summary, a reason, and the id typed in. */
  function askConstitutional(l: LawInfo): void {
    const reason = h('input.input', { type: 'text', maxlength: 300, autocomplete: 'off', placeholder: 'Why this law is off', 'aria-label': 'Reason' }) as HTMLInputElement;
    const typed = h('input.input', { type: 'text', autocomplete: 'off', spellcheck: 'false', placeholder: l.id, 'aria-label': `Type ${l.id} to confirm` }) as HTMLInputElement;
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
        h('blockquote', null, l.statement || l.title),
        l.rationale ? h('p.hint', null, l.rationale) : null,
        h('p', null, 'While it is off, agents may do what it forbids. The change is recorded with the time, and this screen flags it until the law is switched back on.'),
        field('Reason (required)', reason),
        field('Type the law’s id to confirm', typed, `Exactly ${l.id}, as written`),
      ),
      undefined,
      [button('Cancel', () => close(), { kind: 'ghost' }), confirm],
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
      'article.rl-law.rl-rule',
      { id: `rule-${rule.id}`, 'data-key': rule.id, class: rule.enabled ? '' : 'off' },
      h('div.rl-head', null, h('h3', null, rule.title), h('span.law-id', null, rule.id)),
      h('div.rl-pills', null, badge(rule.enabled ? 'Enabled' : 'Disabled', rule.enabled ? 'ok' : 'muted'), badge(declared.length === 1 ? '1 law' : `${declared.length} laws`)),
      h('p.hint', null, `Updated ${ago(rule.updated_ms)}${rule.author ? ` by ${rule.author}` : ''}`),
      h(
        'div.rl-acts',
        null,
        button('Edit', () => openEditor(rule), { size: 'sm' }),
        button(rule.enabled ? 'Disable' : 'Enable', () => void toggleRule(rule), { size: 'sm' }),
        button('Delete', () => askDelete(rule), { size: 'sm', kind: 'danger' }),
      ),
      declared.length ? declared.map(lawRow) : h('p.hint', null, 'Declares no laws.'),
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
        if (editorBox.querySelector('.rl-ed')?.getAttribute('data-rule') === rule.id) closeEditor();
        void load();
      } else {
        confirm.disabled = false;
      }
    }
    close = modal(
      `Delete ${rule.title}`,
      h('p', null, `Delete ${rule.title} (${rule.id})? Its laws stop applying. This cannot be undone.`),
      undefined,
      [button('Cancel', () => close(), { kind: 'ghost' }), confirm],
    );
  }

  function openEditor(rule: LawRule | null): void {
    replace(editorBox, buildEditor(rule));
    editorBox.querySelector<HTMLElement>('.rl-ed')?.scrollIntoView({ behavior: 'smooth', block: 'center' });
    editorBox.querySelector<HTMLInputElement>('input')?.focus();
  }

  function closeEditor(): void {
    replace(editorBox);
  }

  /** The editor: a title, the JHRL source with line numbers, the reference, and Check / Save. */
  function buildEditor(rule: LawRule | null): HTMLElement {
    const titleIn = h('input.input', { type: 'text', maxlength: 120, autocomplete: 'off', placeholder: 'What the rule is for', 'aria-label': 'Rule title', value: rule?.title ?? '' }) as HTMLInputElement;
    const gutter = h('pre.rl-gut', { 'aria-hidden': 'true' });
    const src = h('textarea.rl-ta', { id: 'rl-src', wrap: 'off', spellcheck: 'false', 'aria-label': 'Rule source (JHRL)', placeholder: 'law "O-001" "Title" { ... }' }) as HTMLTextAreaElement;
    src.value = rule?.source ?? '';
    const fail = h('p.rl-err', { role: 'alert', hidden: true });
    const result = h('div', { role: 'status' });
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
    /** The gutter's numbers, the lines with a problem marked, and the rows the source needs. */
    const paint = (): void => {
      const marks = diagnosticsByLine(diags);
      const count = rowsFor(src.value);
      src.rows = count;
      const nodes: (Node | string)[] = [];
      for (let i = 0; i < count; i++) {
        const line = i + 1;
        const msg = marks.get(line);
        if (i) nodes.push('\n');
        nodes.push(h('span', { class: msg ? 'bad' : undefined, title: msg }, String(line)));
      }
      replace(gutter, nodes);
    };
    const pick = (d: Diagnostic): void => {
      const { start, end } = lineSpan(src.value, d.line);
      src.focus();
      src.setSelectionRange(start, end);
    };
    const showDiags = (list: Diagnostic[]): void => {
      diags = list;
      paint();
      replace(result, list.length ? [h('h4', null, `Does not compile: ${plural(list.length, 'problem', 'problems')}`), diagnosticList(list, pick)] : null);
    };

    src.addEventListener('input', () => {
      // Results describe the source they were checked on; an edit clears them.
      diags = [];
      result.replaceChildren();
      paint();
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
      'section.rl-ed',
      { id: 'rules-editor', 'data-rule': rule?.id ?? null },
      h('h3', null, rule ? [`Edit ${rule.title} `, h('span.law-id', null, rule.id)] : 'New rule'),
      field('Title', titleIn),
      field('Source (JHRL)', h('div.rl-src', null, gutter, src)),
      jhrlReference(),
      fail,
      h('div.rl-acts', null, checkButton, saveButton, button('Cancel', () => closeEditor(), { kind: 'ghost' })),
      result,
    );
  }

  // ---- Strict permits, the switch -------------------------------------------

  strictInput.addEventListener('change', () => void setStrict(strictInput.checked));

  root.append(
    h('header.page-head', null, h('div.page-head-row', null, h('h1', null, 'Rules'), h('span.sp'), h('div.page-actions')), h('p.lede', null, 'The laws the plaza ships with, and the owner’s own JHRL rules.')),
    constitutionBanner,
    strip,
    h('div.rl-main', null, strictSec, builtinSec, ownerSec),
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

function diagnosticList(diags: Diagnostic[], pick: (d: Diagnostic) => void): HTMLElement {
  return h(
    'ol.rl-diag',
    null,
    diags.map((d) =>
      h(
        'li',
        null,
        h('button.btn.ghost.sm', { type: 'button', title: diagnosticText(d), onclick: () => pick(d) }, `Line ${d.line}, column ${d.column}`),
        ` ${d.message}`,
      ),
    ),
  );
}

const REFUSAL_COLUMNS: Column<WouldRefuse>[] = [
  { label: 'When', cell: (w) => ago(w.at_ms) },
  { label: 'Agent', cell: (w) => h('a', { href: href('sessions', w.cid) }, agentName({ cid: w.cid, role: w.role })) },
  { label: 'Role', cell: (w) => w.role },
  { label: 'Tool', cell: (w) => h('code', null, w.tool) },
  { label: 'Laws', cell: (w) => w.laws.join(', ') },
];

function checkOk(r: LawCheck): Node[] {
  return [
    h('h4', null, 'Compiles'),
    h('p', null, 'Declares: ', r.laws.length ? r.laws.flatMap((id, i) => [i ? ' ' : '', h('code', null, id)]) : 'no laws'),
    h('h4', null, 'Would have refused, recently'),
    r.would_refuse.length
      ? h('div.tbl-wrap', null, table(REFUSAL_COLUMNS, r.would_refuse))
      : h('p.muted', null, 'None of the recent calls would be refused.'),
  ];
}

function jhrlReference(): HTMLElement {
  return h(
    'details',
    { open: true },
    h('summary', null, 'JHRL reference'),
    h(
      'ul.hint',
      null,
      h('li', null, h('code', null, 'law "ID" "Title" { ... }'), ': one law per block, with a unique ID'),
      h('li', null, h('code', null, 'tier statute'), ' (or constitution, operations)'),
      h('li', null, h('code', null, 'enforcement preventive'), ' or detective'),
      h('li', null, h('code', null, 'severity info, minor, major or critical')),
      h('li', null, h('code', null, 'statement "what the law says, in plain words"')),
      h('li', null, h('code', null, 'permit OP'), ' or ', h('code', null, 'forbid OP'), ', where OP is a declared dotted operation such as tool.call'),
      h('li', null, h('code', null, 'principal is Agent and resource is Tool')),
      h('li', null, h('code', null, 'when <condition>'), ' (optional)'),
      h('li', null, h('code', null, 'rationale "why the law exists"'), ' (optional)'),
    ),
    h('pre.code', null, EXAMPLE_LAW),
  );
}
