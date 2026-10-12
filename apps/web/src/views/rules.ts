// Rules: the plaza's laws (JHRL), explained for people — what each rule allows or forbids,
// for whom, when, why, how it is enforced and how often it has been — plus the latest
// infractions.

import '../styles/rules.css';
import { api, type Infraction, type LawInfo } from '../api';
import { href } from '../router';
import { on } from '../store';
import { ago, badge, button, field, input, select, tabs, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';

const DAY_MS = 86_400_000;
const INFRACTION_FETCH = 200;
const LATEST_SHOWN = 20;

interface Tier { key: string; label: string; desc: string }

const TIERS: Tier[] = [
  { key: 'constitution', label: 'Constitution', desc: "The plaza's fundamental rules; they override everything else." },
  { key: 'statutes', label: 'Statutes', desc: 'Standing rules for roles and everyday work.' },
  { key: 'tools', label: 'Tools', desc: 'Who may use which tool.' },
  { key: 'operations', label: 'Operations', desc: 'The kinds of actions the laws talk about.' },
  { key: 'other', label: 'Other', desc: '' },
];
const TIER_TABS = TIERS.slice(0, 4);
const EFFECTS: [string, string][] = [['all', 'All'], ['permit', 'Allows'], ['forbid', 'Forbids']];
const SEVERITIES = ['info', 'minor', 'major', 'critical'];

type Effect = 'all' | 'permit' | 'forbid';

function cap(s: string): string {
  return s ? s.charAt(0).toUpperCase() + s.slice(1) : s;
}

/** The tier a law belongs to, read from its tier field, then from the file it lives in. */
function tierOf(l: LawInfo): string {
  for (const probe of [l.tier, l.file].map((s) => (s || '').toLowerCase())) {
    if (probe.includes('const')) return 'constitution';
    if (probe.includes('statut')) return 'statutes';
    if (probe.includes('tool')) return 'tools';
    if (probe.includes('operat')) return 'operations';
  }
  return 'other';
}

function enforcementLabel(e: string): string {
  if (e === 'preventive') return 'Stops it before it happens';
  if (e === 'detective') return 'Recorded after the fact';
  return cap(e) || 'Enforcement not stated';
}

function severityTone(s: string): Tone {
  switch (s) {
    case 'critical': return 'danger';
    case 'major': return 'warn';
    case 'minor': return 'info';
    default: return 'muted';
  }
}

function cell(label: string, value: string): HTMLElement {
  return h('div.rules-stat', null, h('span.rules-stat-label', null, label), h('span.rules-stat-value', null, value));
}

export function mount(root: HTMLElement, params: string[]): Cleanup {
  const state: { q: string; effect: Effect; sev: string; tier: string } = { q: '', effect: 'all', sev: 'any', tier: 'all' };
  let status: 'loading' | 'ready' | 'error' = 'loading';
  let laws: LawInfo[] = [];
  let recent: Infraction[] = [];
  let alive = true;
  let seq = 0;
  let timer = 0;

  const search = input({ type: 'search', placeholder: 'Search by title, statement, id or operation', 'aria-label': 'Search rules' });
  const sevSel = select([{ value: 'any', label: 'Any severity' }, ...SEVERITIES.map((s) => ({ value: s, label: cap(s) }))], 'any');
  const chipBox = h('div.rules-chips', { role: 'group', 'aria-label': 'Effect' });
  const tierBox = h('div.rules-tabs');
  const strip = h('div.rules-strip');
  const countLine = h('p.rules-count');
  const listBox = h('div.rules-groups');
  const infrBox = h('div.infr-box');

  function renderChips(): void {
    replace(chipBox, EFFECTS.map(([key, label]) => h('button.rules-chip', {
      type: 'button',
      'aria-pressed': String(state.effect === key),
      onclick: () => {
        state.effect = key as Effect;
        renderChips();
        renderList();
      },
    }, label)));
  }

  function renderTabs(): void {
    replace(tierBox, tabs([{ key: 'all', label: 'All' }, ...TIER_TABS.map((t) => ({ key: t.key, label: t.label }))], state.tier, (key) => {
      state.tier = key;
      renderTabs();
      renderList();
    }));
  }

  function resetFilters(): void {
    state.q = '';
    state.effect = 'all';
    state.sev = 'any';
    state.tier = 'all';
    search.value = '';
    sevSel.value = 'any';
    renderChips();
    renderTabs();
    renderList();
  }

  function visible(l: LawInfo): boolean {
    if (state.effect !== 'all' && l.effect !== state.effect) return false;
    if (state.sev !== 'any' && l.severity !== state.sev) return false;
    if (state.tier !== 'all' && tierOf(l) !== state.tier) return false;
    const q = state.q.trim().toLowerCase();
    if (q && ![l.title, l.statement, l.id, ...l.ops].join(' ').toLowerCase().includes(q)) return false;
    return true;
  }

  function renderStrip(): void {
    const ready = status === 'ready';
    const forbids = laws.filter((l) => l.effect === 'forbid').length;
    const cut = Date.now() - 30 * DAY_MS;
    const n30 = recent.filter((i) => i.at_ms >= cut).length;
    const n30Text = recent.length >= INFRACTION_FETCH ? `${n30}+` : String(n30);
    replace(
      strip,
      cell('Laws', ready ? String(laws.length) : '—'),
      cell('Allows', ready ? String(laws.length - forbids) : '—'),
      cell('Forbids', ready ? String(forbids) : '—'),
      cell('Infractions, 30 days', ready ? n30Text : '—'),
    );
  }

  function renderGroup(t: Tier, items: LawInfo[]): HTMLElement {
    return h(
      'section.rules-group',
      { id: `tier-${t.key}`, 'aria-labelledby': `tier-${t.key}-h` },
      h(
        'header.rules-group-head',
        null,
        h('h2', { id: `tier-${t.key}-h` }, t.label),
        h('span.rules-group-count', null, String(items.length)),
        t.desc ? h('p.rules-group-desc', null, t.desc) : null,
      ),
      items.map(renderLaw),
    );
  }

  function renderLaw(l: LawInfo): HTMLElement {
    const forbid = l.effect === 'forbid';
    const usage = l.refusals > 0
      ? h(
        'p.law-usage',
        null,
        `Enforced ${l.refusals} ${l.refusals === 1 ? 'time' : 'times'}`,
        l.last_at_ms ? ` · last ${ago(l.last_at_ms)}` : '',
        l.agents.length ? ' · by ' : '',
        l.agents.flatMap((a, i) => [i ? ', ' : '', h('a', { href: href('agents', a.cid) }, a.name), ` (${a.count})`]),
      )
      : null;

    return h(
      'article.law',
      { id: `law-${l.id}`, 'data-key': l.id, class: forbid ? 'forbid' : 'permit' },
      h(
        'header.law-head',
        null,
        h('div.law-title', null, h('h3', null, l.title), h('span.law-id', null, l.id)),
        h(
          'div.law-pills',
          null,
          badge(forbid ? 'Forbids' : 'Allows', forbid ? 'danger' : 'ok'),
          badge(`Severity: ${cap(l.severity)}`, severityTone(l.severity)),
          badge(enforcementLabel(l.enforcement), l.enforcement === 'preventive' ? 'info' : 'muted'),
        ),
      ),
      h('p.law-statement', null, l.statement),
      h('p.law-short', null, h('strong', null, 'In short: '), l.explanation),
      l.rationale ? h('p.law-why', null, h('strong', null, 'Why: '), l.rationale) : null,
      h(
        'div.law-applies',
        null,
        h('span.law-label', null, 'Applies to'),
        h('span.law-scope', null, `${l.principal || 'any principal'} → ${l.resource || 'any resource'}`),
        l.ops.length ? h('span.law-ops', null, l.ops.map((o) => h('code.op', null, o))) : null,
      ),
      usage,
      h(
        'details.law-how',
        null,
        h('summary', null, 'How it is written (JHRL)'),
        h('p.law-file', null, `File: ${l.file || '—'}`),
        l.conditions.length
          ? h('ul.law-conds', null, l.conditions.map((c) => h('li', null, c)))
          : h('p.law-file', null, 'No extra conditions.'),
        h('pre.rules-src', null, l.source || '—'),
      ),
    );
  }

  function renderList(): void {
    if (status === 'loading') {
      countLine.textContent = '';
      replace(listBox, h('p.rules-empty', null, 'Loading the rules…'));
      return;
    }
    if (status === 'error') {
      countLine.textContent = '';
      replace(listBox, h('p.rules-empty', null, 'The rules could not be loaded just now.'));
      return;
    }
    const shown = laws.filter(visible);
    countLine.textContent = `Showing ${shown.length} of ${laws.length} ${laws.length === 1 ? 'rule' : 'rules'}`;
    if (!shown.length) {
      replace(listBox, h('div.rules-empty', null, h('p', null, 'No rules match these filters.'), button('Clear filters', () => resetFilters())));
      return;
    }
    replace(
      listBox,
      TIERS.map((t) => {
        const items = shown.filter((l) => tierOf(l) === t.key);
        return items.length ? renderGroup(t, items) : null;
      }),
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
    if (status !== 'ready') {
      replace(infrBox, h('p.rules-empty', null, status === 'error' ? 'Infractions could not be loaded.' : 'Loading…'));
      return;
    }
    const latest = [...recent].sort((a, b) => b.at_ms - a.at_ms).slice(0, LATEST_SHOWN);
    replace(
      infrBox,
      latest.length
        ? h('ol.infr-list', null, latest.map(renderInfraction))
        : h('p.rules-empty', null, 'No infractions yet.'),
    );
  }

  function renderAll(): void {
    renderStrip();
    renderList();
    renderInfractions();
  }

  /** Brings one law into view and highlights it briefly, clearing filters that hide it. */
  function focusLaw(id: string): void {
    const law = laws.find((l) => l.id === id);
    if (!law) return;
    if (!visible(law)) resetFilters();
    const el = document.getElementById(`law-${id}`);
    if (!el) return;
    el.scrollIntoView({ behavior: 'smooth', block: 'center' });
    el.classList.add('flash');
    window.setTimeout(() => el.classList.remove('flash'), 2200);
  }

  async function load(): Promise<void> {
    const mine = ++seq;
    try {
      const [l, r] = await Promise.all([api.laws(), api.infractions(INFRACTION_FETCH)]);
      if (!alive || mine !== seq) return;
      laws = l;
      recent = r;
      status = 'ready';
    } catch {
      if (!alive || mine !== seq) return;
      if (status !== 'ready') status = 'error';
    }
    renderAll();
  }

  search.addEventListener('input', () => {
    state.q = search.value;
    renderList();
  });
  sevSel.addEventListener('change', () => {
    state.sev = sevSel.value;
    renderList();
  });

  const schedule = (): void => {
    window.clearTimeout(timer);
    timer = window.setTimeout(() => void load(), 1000);
  };
  const offs = [on('violation', schedule), on('agent', schedule)];

  const onHash = (): void => {
    const [screen, id] = location.hash.replace(/^#\/?/, '').split('/');
    if (screen === 'rules' && id) focusLaw(decodeURIComponent(id));
  };
  window.addEventListener('hashchange', onHash);

  root.append(
    h(
      'div.rules-view',
      null,
      h('div.view-head', null, h('h1', null, 'Rules')),
      h(
        'div.rules-intro',
        null,
        h(
          'p',
          null,
          "These are the plaza's laws. Every action an agent takes is checked against them before it happens; a law that forbids an action stops it, and the attempt is recorded as an infraction.",
        ),
      ),
      strip,
      h(
        'div.rules-toolbar',
        null,
        h('div.rules-row', null, search, field('Severity', sevSel)),
        h('div.rules-row', null, chipBox, tierBox),
      ),
      h(
        'div.rules-grid',
        null,
        h('div.rules-main', null, countLine, listBox),
        h('aside.rules-side', null, h('section.infr-panel', null, h('h2', null, 'Latest infractions'), infrBox)),
      ),
    ),
  );

  renderChips();
  renderTabs();
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
