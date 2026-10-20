// A project's Maintenance tab: what its watchers saw and propose, and the owner's answers.
//
// Baseline first: one line says what waits for the owner, then the product's map as a graph (the product in the
// middle, its areas around it coloured by health, the proposals as small dots beside the area they touch), and the
// proposals ranked, each with what would change, its value, risk and size, and three answers: Do it, Later, No.
// Words come on demand: a click on an area or a proposal opens it beside the map. The inbox, the ingest address,
// what the watchers read and the settings sit below. Expert shows every field and the decided proposals.

import '../styles/project-maintenance.css';
import { api, type Agent, type Project } from '../api';
import { projectsApi, SIGNAL_KINDS, type Area, type Maintenance, type Proposal, type Signal, type SignalKind } from '../api-projects';
import { on } from '../store';
import { ago, attempt, badge, button, card, empty, field, input, modal, select, textarea, toast } from '../ui/components';
import { h, replace } from '../ui/dom';
import { href } from '../router';
import {
  decided,
  everyText,
  headline,
  healthTone,
  KIND_LABEL,
  mapLinks,
  outcomeLine,
  proposalsByArea,
  ringLayout,
  signalLine,
  sweepLine,
  waitingFirst,
  weighLine,
} from './maintenance-model';

const SVG_NS = 'http://www.w3.org/2000/svg';
const MAP = 420;
const REFRESH_MS = 400;

type Selection = { kind: 'none' } | { kind: 'area'; id: string } | { kind: 'proposal'; id: string };

/** What the tab remembers per project while the page lives: the selection, Expert, the open inbox form. */
const memory = new Map<string, { sel: Selection; expert: boolean; shownAreas: Set<string> }>();

function svg<T extends SVGElement>(tag: string, attrs: Record<string, string | number>, cls?: string): T {
  const el = document.createElementNS(SVG_NS, tag) as T;
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, String(v));
  if (cls) el.setAttribute('class', cls);
  return el;
}

/** The Maintenance tab of project `p`. */
export function maintenanceTab(p: Project, onModeChange: () => void): HTMLElement {
  const root = h('div.mt', null, h('p.mt-faint', null, 'Loading…'));
  const mem = memory.get(p.id) ?? { sel: { kind: 'none' } as Selection, expert: false, shownAreas: new Set<string>() };
  memory.set(p.id, mem);
  let data: Maintenance | null = null;
  let names = new Map<number, string>();
  let timer = 0;

  const nameOf = (cid: number): string => names.get(cid) ?? `agent ${cid}`;

  const load = async (): Promise<void> => {
    const [m, agents] = await Promise.all([
      attempt(() => projectsApi.maintenance(p.id)),
      api.agents().catch(() => [] as Agent[]),
    ]);
    if (shown && !root.isConnected) return;
    if (m) data = m;
    names = new Map(agents.map((a) => [a.cid, a.name || `agent ${a.cid}`]));
    draw();
  };
  const schedule = (): void => {
    window.clearTimeout(timer);
    timer = window.setTimeout(() => void load(), REFRESH_MS);
  };
  // Live: the watchers' signals, maps, proposals and sweeps, and the tasks that picked proposals became.
  const offs = [
    on('maintenance', (ev) => {
      if ((ev as { project?: string }).project === p.id) schedule();
    }),
    on('task', schedule),
  ];
  // Once the tab has been on the page and leaves it (the page redrew, or the owner went elsewhere), it stops listening.
  let shown = false;
  const stop = new MutationObserver(() => {
    if (root.isConnected) {
      shown = true;
      return;
    }
    if (!shown) return;
    offs.forEach((off) => off());
    stop.disconnect();
    window.clearTimeout(timer);
  });
  stop.observe(document.body, { childList: true, subtree: true });

  const select = (sel: Selection): void => {
    mem.sel = sel;
    draw();
  };

  // ---- The owner's answers ---------------------------------------------------------------------------------------

  const doIt = (pr: Proposal): void => {
    const note = textarea({ rows: 2, placeholder: 'Anything the team should know (optional)' });
    const budget = input({ type: 'number', min: 200, step: 100, placeholder: pr.effort.budget ? String(pr.effort.budget) : 'The project’s default' });
    const knownTool = pr.instrument?.catalog;
    const close = modal(
      'Do it',
      h(
        'div.mt-form',
        null,
        h('p', null, h('strong', null, pr.title)),
        h('p.mt-faint', null, knownTool
          ? `${pr.instrument!.name} is something you set up: it is picked, and you add it in Connectors or MCP.`
          : `It becomes a task of this project for a ${pr.role}. ${weighLine(pr)}.`),
        knownTool ? null : field('Budget (credits)', budget),
        field('Note', note),
        h(
          'div.actions',
          null,
          button('Cancel', () => close(), { kind: 'ghost' }),
          button('Do it', () => {
            const credits = Number(budget.value);
            void attempt(
              () => projectsApi.approveProposal(p.id, pr.id, { note: note.value.trim() || undefined, budget: credits >= 200 ? credits : undefined }),
              knownTool ? `${pr.instrument!.name}: set it up in Connectors or MCP` : 'Picked: it is a task now',
            ).then((done) => {
              if (!done) return;
              close();
              void load();
            });
          }, { kind: 'primary' }),
        ),
      ),
    );
  };

  const later = (pr: Proposal): void => {
    void attempt(() => projectsApi.laterProposal(p.id, pr.id), 'Put off for later').then(() => void load());
  };

  const no = (pr: Proposal): void => {
    const reason = textarea({ rows: 3, placeholder: 'Why not: the watchers read it, and propose it again only with new evidence' });
    const close = modal(
      'Not this',
      h(
        'div.mt-form',
        null,
        h('p', null, h('strong', null, pr.title)),
        field('Reason (optional)', reason),
        h(
          'div.actions',
          null,
          button('Cancel', () => close(), { kind: 'ghost' }),
          button('Decline', () => {
            void attempt(() => projectsApi.declineProposal(p.id, pr.id, reason.value.trim()), 'Declined').then((done) => {
              if (!done) return;
              close();
              void load();
            });
          }, { kind: 'danger' }),
        ),
      ),
    );
  };

  const answers = (pr: Proposal): HTMLElement =>
    h(
      'div.mt-answers',
      null,
      button('Do it', () => doIt(pr), { kind: 'primary', size: 'sm' }),
      pr.state === 'later' ? null : button('Later', () => later(pr), { size: 'sm' }),
      button('No', () => no(pr), { size: 'sm', kind: 'ghost' }),
    );

  // ---- Pieces ----------------------------------------------------------------------------------------------------

  const valueDots = (score: number): HTMLElement =>
    h('span.mt-value', { title: `Value ${score} of 5` }, [1, 2, 3, 4, 5].map((i) => h('i', { class: i <= score ? 'on' : '' })));

  const proposalCard = (pr: Proposal, full: boolean): HTMLElement => {
    const open = full || (mem.sel.kind === 'proposal' && mem.sel.id === pr.id);
    const area = pr.area ?? pr.impact.areas[0];
    return h(
      'article.mt-proposal',
      { class: `${pr.kind} ${pr.state}${open ? ' open' : ''}`, 'data-key': pr.id },
      h(
        'div.mt-proposal-head',
        null,
        badge(KIND_LABEL[pr.kind] ?? pr.kind, pr.kind === 'security' ? 'danger' : pr.kind === 'instrument' ? 'info' : 'muted'),
        h('button.mt-title', { type: 'button', onclick: () => select(open && !full ? { kind: 'none' } : { kind: 'proposal', id: pr.id }) }, pr.title),
        valueDots(pr.value.score),
      ),
      h('p.mt-changes', null, pr.impact.changes || pr.summary),
      h(
        'div.mt-weigh',
        null,
        h('span', { class: `risk ${pr.impact.risk}` }, `${pr.impact.risk} risk`),
        h('span', null, pr.effort.size.toUpperCase()),
        area ? h('span.mt-faint', null, area) : null,
        pr.state === 'later' ? badge('later', 'warn') : null,
      ),
      open
        ? h(
            'div.mt-detail',
            null,
            h('p', null, pr.summary),
            pr.impact.users ? h('p', null, h('span.mt-label', null, 'Who it touches'), pr.impact.users) : null,
            h('p', null, h('span.mt-label', null, 'Why it is worth it'), pr.value.why),
            pr.impact.risk_note ? h('p', null, h('span.mt-label', null, 'Risk'), pr.impact.risk_note) : null,
            pr.instrument
              ? h('p', null, h('span.mt-label', null, 'Data it would give'), `${pr.instrument.name} (${pr.instrument.kind}): ${pr.instrument.data}`)
              : null,
            pr.evidence.length
              ? h(
                  'ul.mt-evidence',
                  null,
                  pr.evidence.map((e) => h('li', null, e.signal ? h('code', null, e.signal) : null, e.signal && e.note ? ' ' : null, e.note)),
                )
              : null,
            pr.acceptance ? h('p', null, h('span.mt-label', null, 'Done when'), pr.acceptance) : null,
            h('p.mt-faint', null, `Proposed by ${pr.by} ${ago(pr.created_ms)} · for a ${pr.role}`),
          )
        : null,
      pr.state === 'proposed' || pr.state === 'later' ? answers(pr) : h('p.mt-outcome', null, outcomeLine(pr)),
    );
  };

  /** The map: the product in the middle, its areas around it, the waiting proposals beside their areas. */
  const mapGraph = (m: Maintenance): Node => {
    const areas = m.map.areas;
    if (areas.length === 0) {
      return h(
        'div.mt-map-empty',
        null,
        h('div.mt-core-dot', { class: m.sweep.running ? 'busy' : '' }),
        h('p', null, m.sweep.running ? 'The watchers are mapping the project…' : 'No map yet: the first sweep maps the project.'),
      );
    }
    const at = ringLayout(areas, MAP);
    const byArea = proposalsByArea(m.proposals);
    const c = MAP / 2;
    const root = svg<SVGSVGElement>('svg', { viewBox: `0 0 ${MAP} ${MAP}`, role: 'group', 'aria-label': 'The product map' }, 'mt-svg');
    root.addEventListener('click', () => select({ kind: 'none' }));
    const lines = svg<SVGGElement>('g', {}, 'mt-lines');
    const nodes = svg<SVGGElement>('g', {}, 'mt-nodes');
    root.append(lines, nodes);
    // Spokes from the product to each area, then the areas' own links.
    areas.forEach((a, i) => {
      const pt = at.get(a.id)!;
      lines.append(svg('line', { x1: c, y1: c, x2: pt.x, y2: pt.y, style: `--i:${i}` }, 'mt-spoke'));
    });
    for (const [a, b] of mapLinks(areas)) {
      const pa = at.get(a)!;
      const pb = at.get(b)!;
      lines.append(svg('line', { x1: pa.x, y1: pa.y, x2: pb.x, y2: pb.y }, 'mt-link'));
    }
    // The product.
    const core = svg<SVGGElement>('g', { transform: `translate(${c} ${c})` }, `mt-core${m.unseen ? ' heard' : ''}`);
    core.append(svg('circle', { r: 16 }, 'mt-core-ring'), svg('circle', { r: 9 }, 'mt-core-fill'));
    const coreTitle = svg('title', {});
    coreTitle.textContent = `${p.title}${m.unseen ? `: ${m.unseen} new signals` : ''}`;
    core.append(coreTitle);
    nodes.append(core);
    // The areas, each in its place; one the page has not shown before eases out from the product (a CSS animation,
    // so it also lands where it belongs in a background tab).
    areas.forEach((a, i) => {
      const pt = at.get(a.id)!;
      const selected = mem.sel.kind === 'area' && mem.sel.id === a.id;
      const g = svg<SVGGElement>('g', { tabindex: 0, role: 'button', transform: `translate(${pt.x} ${pt.y})` }, `mt-area ${healthTone(a.health)}${selected ? ' on' : ''}`);
      const fresh = !mem.shownAreas.has(a.id);
      mem.shownAreas.add(a.id);
      const body = svg<SVGGElement>('g', fresh ? { style: `--dx:${c - pt.x}px;--dy:${c - pt.y}px;--i:${i}` } : {}, fresh ? 'mt-area-in fresh' : 'mt-area-in');
      g.append(body);
      const waiting = byArea.get(a.id) ?? [];
      body.append(svg('circle', { r: selected ? 15 : 12 }, 'mt-area-dot'));
      // The waiting proposals as small dots around the area, one per proposal (at most 8).
      waiting.slice(0, 8).forEach((pr, j) => {
        const angle = -Math.PI / 2 + (j * Math.PI) / 4;
        body.append(svg('circle', { cx: 21 * Math.cos(angle), cy: 21 * Math.sin(angle), r: 3.5 }, `mt-prop-dot ${pr.kind}`));
      });
      const label = svg<SVGTextElement>('text', { y: 32, 'text-anchor': 'middle' }, 'mt-area-label');
      label.textContent = a.name.length > 18 ? `${a.name.slice(0, 17)}…` : a.name;
      body.append(label);
      const title = svg('title', {});
      title.textContent = `${a.name}: ${a.health}${waiting.length ? ` · ${waiting.length} waiting` : ''}`;
      g.append(title);
      const pick = (ev: Event): void => {
        ev.stopPropagation();
        select(selected ? { kind: 'none' } : { kind: 'area', id: a.id });
      };
      g.addEventListener('click', pick);
      g.addEventListener('keydown', (ev) => {
        if (ev.key === 'Enter' || ev.key === ' ') pick(ev);
      });
      nodes.append(g);
    });
    return h('div.mt-map', null, root);
  };

  /** Beside the map: the selected area or proposal, else what waits for the owner. */
  const side = (m: Maintenance): Node => {
    if (mem.sel.kind === 'area') {
      const id = mem.sel.id;
      const a = m.map.areas.find((x) => x.id === id);
      if (a) return areaPanel(a, m);
    }
    if (mem.sel.kind === 'proposal') {
      const id = mem.sel.id;
      const pr = m.proposals.find((x) => x.id === id);
      if (pr) return h('div.mt-side', null, proposalCard(pr, true));
    }
    // Nothing chosen: the product in a paragraph and the waiting proposals as one line each (their cards are below).
    const waiting = waitingFirst(m.proposals);
    return h(
      'div.mt-side',
      null,
      h('h3.mt-side-head', null, 'The product'),
      m.map.summary ? h('p.mt-summary', null, m.map.summary) : null,
      waiting.length
        ? h(
            'ol.mt-brief',
            null,
            waiting.slice(0, 6).map((pr) =>
              h(
                'li',
                null,
                h('button.mt-brief-item', { type: 'button', onclick: (ev: Event) => { ev.stopPropagation(); select({ kind: 'proposal', id: pr.id }); } }, h('span.mt-brief-title', null, pr.title), valueDots(pr.value.score)),
              ),
            ),
          )
        : null,
      h('p.mt-faint', null, m.map.areas.length ? 'Click an area or a proposal to see more.' : ''),
    );
  };

  const areaPanel = (a: Area, m: Maintenance): Node => {
    const waiting = proposalsByArea(m.proposals).get(a.id) ?? [];
    return h(
      'div.mt-side',
      null,
      h('h3.mt-side-head', null, h('span', { class: `mt-health ${healthTone(a.health)}` }), a.name),
      a.summary ? h('p', null, a.summary) : null,
      a.notes ? h('p.mt-faint', null, a.notes) : null,
      a.paths.length ? h('div.mt-paths', null, a.paths.map((x) => h('code', null, x))) : null,
      a.links.length
        ? h('p.mt-faint', null, 'Talks to ', a.links.flatMap((l, i) => [i ? ', ' : '', h('button.mt-link-btn', { type: 'button', onclick: () => select({ kind: 'area', id: l }) }, m.map.areas.find((x) => x.id === l)?.name ?? l)]))
        : null,
      waiting.length ? waiting.map((pr) => proposalCard(pr, false)) : h('p.mt-faint', null, 'Nothing proposed here.'),
      h('p.mt-faint', null, `Updated ${ago(a.updated_ms)} by ${a.by || 'a watcher'}`),
    );
  };

  const signalRow = (s: Signal): HTMLElement =>
    h(
      'li.mt-signal',
      { 'data-key': s.id },
      badge(s.kind, s.kind === 'error' ? 'danger' : s.kind === 'report' ? 'warn' : 'muted'),
      h('span.mt-signal-text', { title: s.text }, signalLine(s)),
      h('span.mt-faint', null, `${s.from}${s.environment ? ` · ${s.environment}` : ''} · ${ago(s.at_ms)}`),
      s.url ? h('a', { href: s.url, target: '_blank', rel: 'noopener' }, 'open') : null,
      button('', () => void attempt(() => projectsApi.deleteSignal(p.id, s.id)).then(() => void load()), { icon: 'x', size: 'sm', kind: 'ghost', title: 'Delete this signal' }),
    );

  const addSignal = (): void => {
    const kind = select_(SIGNAL_KINDS.map((k) => ({ value: k, label: k })), 'feedback');
    const title = input({ placeholder: 'Checkout fails on Safari' });
    const text = textarea({ rows: 4, placeholder: 'What a user said, a figure from your analytics, an error report…' });
    const from = input({ placeholder: 'support, a customer, your analytics' });
    const close = modal(
      'Add a signal',
      h(
        'div.mt-form',
        null,
        field('Kind', kind),
        field('Title (optional)', title),
        field('What it says', text),
        field('From (optional)', from),
        h(
          'div.actions',
          null,
          button('Cancel', () => close(), { kind: 'ghost' }),
          button('Add', () => {
            if (!text.value.trim()) {
              toast('Say what the signal says', 'error');
              return;
            }
            void attempt(() => projectsApi.addSignal(p.id, { kind: kind.value as SignalKind, title: title.value, text: text.value, from: from.value || undefined }), 'Signal added').then((done) => {
              if (!done) return;
              close();
              void load();
            });
          }, { kind: 'primary' }),
        ),
      ),
    );
  };

  const ingest = (m: Maintenance): void => {
    void attempt(() => projectsApi.newIngest(p.id)).then((made) => {
      if (!made) return;
      const url = `${location.origin}${made.path}`;
      const example = `curl -X POST ${url} \\\n  -H 'content-type: application/json' \\\n  -d '{"kind": "report", "title": "Checkout fails", "text": "The pay button does nothing on Safari"}'`;
      modal(
        'The ingest address',
        h(
          'div.mt-form',
          null,
          h('p', null, 'Send user reports, feedback and webhooks here. It is shown once: copy it now. Making a new one stops this one.'),
          h('pre.mt-code', null, url),
          h('p.mt-faint', null, 'JSON with text (and kind, title, from, url, environment), or plain text:'),
          h('pre.mt-code', null, example),
        ),
      );
      void m;
      void load();
    });
  };

  const settings = (m: Maintenance): void => {
    const every = select_(
      [0, 1, 6, 12, 24, 48, 168].map((n) => ({ value: String(n), label: everyText(n) })),
      String(m.config.every_hours),
    );
    const close = modal(
      'Sweeps',
      h(
        'div.mt-form',
        null,
        field('Sweep', every, 'A sweep is one watcher reading what came in, updating the map and proposing.'),
        h('p.mt-faint', null, m.watchers.length
          ? `Watchers, in turn: ${m.watchers.map(nameOf).join(', ')}.`
          : 'No agent watches it yet: add a planner or a researcher on the Team tab.'),
        h(
          'div.actions',
          null,
          button('Cancel', () => close(), { kind: 'ghost' }),
          button('Save', () => {
            void attempt(() => projectsApi.setMaintenance(p.id, { every_hours: Number(every.value) }), 'Saved').then((done) => {
              if (!done) return;
              close();
              void load();
            });
          }, { kind: 'primary' }),
        ),
      ),
    );
  };

  // ---- The page --------------------------------------------------------------------------------------------------

  const notInMaintenance = (): Node[] => [
    h(
      'div.mt-intro',
      null,
      h('h3', null, 'This project is being built'),
      h('p', null, 'In maintenance, its agents read its analytics, user reports and feedback, keep a map of it and propose work: what would change, for whom, its value and its risk. Nothing runs until you pick a proposal.'),
      button('Put it in maintenance', () => {
        void attempt(() => projectsApi.setMode(p.id, 'maintenance'), `${p.title} is in maintenance`).then((done) => {
          if (done) onModeChange();
        });
      }, { kind: 'primary' }),
    ),
  ];

  const draw = (): void => {
    if (!data) return;
    const m = data;
    if (p.mode !== 'maintenance' && m.sweep.runs === 0 && m.proposals.length === 0 && m.signals_total === 0) {
      replace(root, notInMaintenance());
      return;
    }
    const now = Date.now();
    const focus = input({ value: m.config.focus, placeholder: 'What matters now: checkout conversion, the mobile crashes…' });
    const saveFocus = (): void => {
      if (focus.value.trim() === m.config.focus) return;
      void attempt(() => projectsApi.setMaintenance(p.id, { focus: focus.value.trim() }), 'The watchers will read it').then(() => void load());
    };
    focus.addEventListener('change', saveFocus);
    const waiting = waitingFirst(m.proposals);
    const done = decided(m.proposals);
    const instruments = waiting.filter((x) => x.kind === 'instrument');
    // A redraw replaces the tab's content, which pulls the window up; the owner stays where they were reading.
    const y = window.scrollY;
    replace(
      root,
      p.mode !== 'maintenance'
        ? h('p.mt-note', null, 'This project is in build mode now: its watchers no longer sweep. ', h('button.mt-link-btn', { type: 'button', onclick: () => void attempt(() => projectsApi.setMode(p.id, 'maintenance')).then((x) => x && onModeChange()) }, 'Back to maintenance'))
        : null,
      h(
        'div.mt-strip',
        null,
        h('div.mt-verdict', { class: waiting.length ? 'needs' : '' }, headline(m)),
        h('div.mt-sweep', null, sweepLine(m, now, nameOf), ' · ', everyText(m.config.every_hours)),
        h(
          'div.mt-strip-actions',
          null,
          button('Sweep now', () => void attempt(() => projectsApi.sweep(p.id), 'A watcher is sweeping').then(() => void load()), { size: 'sm', icon: 'refresh', disabled: !!m.sweep.running }),
          button('', () => settings(m), { size: 'sm', icon: 'settings', kind: 'ghost', title: 'How often it sweeps, and who watches' }),
          h('label.mt-expert', null, h('input', { type: 'checkbox', checked: mem.expert, onchange: (ev: Event) => { mem.expert = (ev.target as HTMLInputElement).checked; draw(); } }), 'Expert'),
        ),
      ),
      h('div.mt-focus', null, h('span.mt-label', null, 'Focus'), focus),
      h('div.mt-stage', null, mapGraph(m), side(m)),
      m.sweep.last_summary && m.sweep.runs > 0
        ? h('details.mt-last', null, h('summary', null, 'What the last sweep said'), h('p', null, m.sweep.last_summary))
        : null,
      card(
        `Proposals${waiting.length ? ` (${waiting.length})` : ''}`,
        waiting.length
          ? h('div.mt-proposals', null, waiting.map((pr) => proposalCard(pr, mem.expert)))
          : empty('lightbulb', 'Nothing proposed yet', m.watchers.length ? 'The watchers propose after they read what comes in.' : 'Add an agent to the project (a planner or a researcher) so someone watches it.'),
      ),
      instruments.length
        ? card('Data the watchers lack', h('ul.mt-instruments', null, instruments.map((pr) => h('li', null, h('strong', null, pr.instrument?.name ?? pr.title), ` — ${pr.instrument?.data ?? pr.summary}`))))
        : null,
      mem.expert && done.length
        ? card(`Decided (${done.length})`, h('div.mt-proposals', null, done.map((pr) => proposalCard(pr, true))))
        : done.length
          ? h('p.mt-faint', null, `${done.length} decided — `, h('button.mt-link-btn', { type: 'button', onclick: () => { mem.expert = true; draw(); } }, 'show them'))
          : null,
      card(
        `Inbox${m.signals_total ? ` (${m.signals_total}${m.unseen ? `, ${m.unseen} new` : ''})` : ''}`,
        m.signals.length
          ? h('ul.mt-signals', null, m.signals.slice(0, mem.expert ? 200 : 15).map(signalRow))
          : h('p.mt-faint', null, 'Nothing has come in yet. Add what users told you, or give your app or error tracker the ingest address.'),
        {
          actions: [
            button('Add a signal', addSignal, { size: 'sm', icon: 'plus' }),
            button(m.sources.ingest ? 'New ingest address' : 'Ingest address', () => ingest(m), { size: 'sm', title: 'An address your app, support inbox or error tracker posts to' }),
            m.sources.ingest ? button('Stop it', () => void attempt(() => projectsApi.stopIngest(p.id), 'The ingest address stopped').then(() => void load()), { size: 'sm', kind: 'ghost' }) : null,
          ].filter((x): x is HTMLButtonElement => x !== null),
        },
      ),
      card(
        'What the watchers read',
        h(
          'ul.mt-sources',
          null,
          h('li', null, `The inbox: ${m.sources.inbox} signals${m.sources.ingest ? ', and its ingest address' : ''}`),
          p.repo ? h('li', null, 'The code: ', h('a', { href: href('history', p.repo) }, p.repo)) : null,
          m.sources.connectors.map((c) => h('li', null, `${c.label} (${c.category})`)),
          m.sources.mcp.map((s) => h('li', null, `The MCP server ${s.name}`)),
          m.sources.connectors.length + m.sources.mcp.length === 0
            ? h('li.mt-faint', null, 'No analytics, error or support tools are connected yet. ', h('a', { href: href('mcp') }, 'Add an MCP server'), ' or ', h('a', { href: href('tools') }, 'a connector'), ', or let the watchers propose one.')
            : null,
        ),
      ),
    );
    if (window.scrollY !== y) window.scrollTo({ top: y });
  };

  void load();
  return root;
}

// The components' select, renamed so it does not clash with the selection above.
const select_ = select;
