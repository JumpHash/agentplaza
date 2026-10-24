// A project's Maintenance tab: what its watchers saw and propose, and the owner's answers.
//
// Baseline first: one line says what waits for the owner, then the product's map as a graph (the product in the
// middle, its areas around it coloured by health, the proposals as small dots beside the area they touch), and the
// proposals ranked, each with what would change, its value, risk and size, and three answers: Do it, Later, No.
// Words come on demand: a click on an area or a proposal opens it beside the map. The inbox, the ingest address,
// what the watchers read and the settings sit below. Expert shows every field and the decided proposals. The markup is
// the prototype's Maintenance tab (styles/proto/projects.css); the rules are in maintenance-model.ts.

import { agentLabel, agentName } from '../roles';
import { api, type Agent, type Project } from '../api';
import { projectsApi, SIGNAL_KINDS, type Area, type Maintenance, type Proposal, type Signal, type SignalKind } from '../api-projects';
import { on } from '../store';
import { ago, attempt, badge, button, card, dot, empty, field, input, modal, select, textarea, toast } from '../ui/components';
import { h, lu, replace } from '../ui/dom';
import type { LucideName } from '../ui/icons.gen';
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

/** What the tab remembers per project while the page lives: the selection, Expert, the areas already shown. */
const memory = new Map<string, { sel: Selection; expert: boolean; shownAreas: Set<string> }>();

const KTONE = (k: Proposal['kind']): 'danger' | 'info' | 'muted' => (k === 'security' ? 'danger' : k === 'instrument' ? 'info' : 'muted');
const SIZE_SHORT: Record<Proposal['effort']['size'], string> = { s: 'S', m: 'M', l: 'L', xl: 'XL' };
const EVERY: number[] = [0, 1, 6, 12, 24, 48, 168];

function svg<T extends SVGElement>(tag: string, attrs: Record<string, string | number>, cls?: string): T {
  const el = document.createElementNS(SVG_NS, tag) as T;
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, String(v));
  if (cls) el.setAttribute('class', cls);
  return el;
}

/** A dialog as the prototype draws it: the body, and the actions in the sheet's own footer. */
function dialog(title: string, body: (Node | null)[], foot: (close: () => void) => Node[]): () => void {
  const close = modal(title, h('div.pj-dlg', null, body));
  const scrims = document.querySelectorAll<HTMLElement>('.scrim');
  scrims[scrims.length - 1]?.querySelector('.sheet')?.append(h('footer.sheet-f', null, foot(close)));
  return close;
}

/** A button with a lucide icon (button() takes the console's own icon names only). */
function lbtn(label: string, onClick: (ev: MouseEvent) => void, o: { kind?: 'primary' | 'danger' | 'ghost'; size?: 'sm'; icon?: LucideName; title?: string; disabled?: boolean } = {}): HTMLButtonElement {
  const b = button(label, onClick, { kind: o.kind, size: o.size, title: o.title, disabled: o.disabled });
  if (o.icon) b.insertBefore(lu(o.icon, o.size === 'sm' ? 14 : 15), b.firstChild);
  return b;
}

/** An icon-only button (the prototype's iconbtn). */
function iconBtn(icon: LucideName, title: string, onClick: (ev: MouseEvent) => void): HTMLButtonElement {
  const b = h('button.iconbtn', { type: 'button', title, 'aria-label': title }, lu(icon, 16));
  b.addEventListener('click', onClick);
  return b;
}

/** The value of a proposal as five dots, filled up to its score. */
function valueDots(score: number): HTMLElement {
  return h('span.mt-val', { title: `Value ${score} of 5` }, [1, 2, 3, 4, 5].map((i) => h('i', { class: i <= score ? 'on' : null })));
}

/** The Maintenance tab of project `p`. */
export function maintenanceTab(p: Project, onModeChange: () => void): HTMLElement {
  const root = h('div.pj-tab', null, h('p.pj-faint', null, 'Loading…'));
  const mem = memory.get(p.id) ?? { sel: { kind: 'none' } as Selection, expert: false, shownAreas: new Set<string>() };
  memory.set(p.id, mem);
  let data: Maintenance | null = null;
  let names = new Map<number, string>();
  let timer = 0;

  const nameOf = (cid: number): string => names.get(cid) ?? agentLabel(cid);

  const load = async (): Promise<void> => {
    const [m, agents] = await Promise.all([
      attempt(() => projectsApi.maintenance(p.id)),
      api.agents().catch(() => [] as Agent[]),
    ]);
    if (shown && !root.isConnected) return;
    if (m) data = m;
    names = new Map(agents.map((a) => [a.cid, agentName(a)]));
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

  const choose = (sel: Selection): void => {
    mem.sel = sel;
    draw();
  };

  // ---- The owner's answers ---------------------------------------------------------------------------------------

  const doIt = (pr: Proposal): void => {
    const note = textarea({ rows: 2, placeholder: 'Anything the agents should know (optional)' });
    const knownTool = pr.instrument?.catalog;
    dialog(
      'Do it',
      [
        h('p', null, h('strong', null, pr.title)),
        h('p.pj-faint', null, knownTool ? `${pr.instrument!.name} is something you set up: it is picked, and you add it in Connectors or MCP.` : `It becomes a task of this project for a ${pr.role}. ${weighLine(pr)}.`),
        field('Note', note, undefined),
      ],
      (close) => [
        lbtn('Cancel', () => close(), { kind: 'ghost' }),
        lbtn(
          'Do it',
          () => {
            void attempt(
              () => projectsApi.approveProposal(p.id, pr.id, { note: note.value.trim() || undefined }),
              knownTool ? `${pr.instrument!.name}: set it up in Connectors or MCP` : 'Picked: it is a task now',
            ).then((done) => {
              if (!done) return;
              close();
              void load();
            });
          },
          { kind: 'primary' },
        ),
      ],
    );
  };

  const later = (pr: Proposal): void => {
    void attempt(() => projectsApi.laterProposal(p.id, pr.id), 'Put off for later').then(() => void load());
  };

  const no = (pr: Proposal): void => {
    const reason = textarea({ rows: 3, placeholder: 'Why not: the watchers read it, and propose it again only with new evidence' });
    dialog('Not this', [h('p', null, h('strong', null, pr.title)), field('Reason (optional)', reason)], (close) => [
      lbtn('Cancel', () => close(), { kind: 'ghost' }),
      lbtn(
        'Decline',
        () => {
          void attempt(() => projectsApi.declineProposal(p.id, pr.id, reason.value.trim()), 'Declined').then((done) => {
            if (!done) return;
            close();
            void load();
          });
        },
        { kind: 'danger' },
      ),
    ]);
  };

  // ---- Pieces ----------------------------------------------------------------------------------------------------

  /** One proposal: its kind, title and value; what would change, its weight and its area; its details when open; the answers. */
  const proposalCard = (pr: Proposal, open: boolean): HTMLElement => {
    const waiting = pr.state === 'proposed' || pr.state === 'later';
    const area = pr.area ?? pr.impact.areas[0];
    const areaName = area ? (data?.map.areas.find((a) => a.id === area)?.name ?? '') : '';
    return h(
      'article.mt-prop',
      { 'data-id': pr.id },
      h(
        'div.mt-ptop',
        null,
        badge(KIND_LABEL[pr.kind] ?? pr.kind, KTONE(pr.kind)),
        h('button.mt-ptitle', { type: 'button', onclick: () => choose(mem.sel.kind === 'proposal' && mem.sel.id === pr.id ? { kind: 'none' } : { kind: 'proposal', id: pr.id }) }, pr.title),
        valueDots(pr.value.score),
      ),
      h('p.mt-changes', null, pr.impact.changes || pr.summary),
      h(
        'div.mt-weigh',
        null,
        h('span', { class: `risk ${pr.impact.risk}` }, `${pr.impact.risk} risk`),
        h('span', null, SIZE_SHORT[pr.effort.size] ?? pr.effort.size),
        h('span', null, areaName),
        pr.state === 'later' ? badge('later', 'warn') : null,
      ),
      open
        ? h(
            'div.mt-pbody',
            null,
            h('p.mt-psum', null, pr.summary),
            pr.impact.users ? h('div.mt-k', null, h('b', null, 'Who it touches'), h('span', null, pr.impact.users)) : null,
            h('div.mt-k', null, h('b', null, 'Why it is worth it'), h('span', null, pr.value.why)),
            pr.impact.risk_note ? h('div.mt-k', null, h('b', null, 'Risk'), h('span', null, pr.impact.risk_note)) : null,
            pr.instrument ? h('div.mt-k', null, h('b', null, 'Data it would give'), h('span', null, `${pr.instrument.name} (${pr.instrument.kind}): ${pr.instrument.data}`)) : null,
            pr.evidence.length
              ? h(
                  'ul.mt-ev',
                  null,
                  pr.evidence.map((e) => h('li', null, e.signal ? h('code', null, e.signal) : null, e.signal ? ' ' : null, e.note)),
                )
              : null,
            pr.acceptance ? h('div.mt-k', null, h('b', null, 'Done when'), h('span', null, pr.acceptance)) : null,
            h('div.mt-foot', null, `Proposed by ${pr.by} ${ago(pr.created_ms)} · for a ${pr.role}`),
          )
        : null,
      waiting
        ? h(
            'div.mt-ans',
            null,
            lbtn('Do it', () => doIt(pr), { kind: 'primary', size: 'sm' }),
            pr.state === 'later' ? null : lbtn('Later', () => later(pr), { size: 'sm' }),
            lbtn('No', () => no(pr), { kind: 'ghost', size: 'sm' }),
          )
        : h('div.mt-outcome', null, outcomeLine(pr)),
    );
  };

  /** The map: the product in the middle, its areas around it, the waiting proposals beside their areas. */
  const mapBody = (m: Maintenance): Node => {
    const areas = m.map.areas;
    if (!areas.length) {
      return h(
        'div.mt-empty',
        null,
        m.sweep.running ? h('span.spin') : null,
        m.sweep.running ? 'The watchers are mapping the project…' : 'No map yet: the first sweep maps the project.',
      );
    }
    const C = MAP / 2;
    const at = ringLayout(areas, MAP);
    const byArea = proposalsByArea(m.proposals);
    const el = svg<SVGSVGElement>('svg', { viewBox: `0 0 ${MAP} ${MAP}`, role: 'group', 'aria-label': 'The product map' });
    const clear = svg<SVGRectElement>('rect', { x: 0, y: 0, width: MAP, height: MAP, fill: 'transparent' });
    clear.addEventListener('click', () => choose({ kind: 'none' }));
    el.append(clear);
    // Spokes from the product to each area, then the areas' own links.
    for (const a of areas) {
      const pt = at.get(a.id)!;
      el.append(svg('line', { x1: C, y1: C, x2: pt.x.toFixed(1), y2: pt.y.toFixed(1) }, 'mt-spoke'));
    }
    for (const [a, b] of mapLinks(areas)) {
      const pa = at.get(a)!;
      const pb = at.get(b)!;
      el.append(svg('line', { x1: pa.x.toFixed(1), y1: pa.y.toFixed(1), x2: pb.x.toFixed(1), y2: pb.y.toFixed(1) }, 'mt-link'));
    }
    // The product.
    const prod = svg<SVGGElement>('g', {}, `mt-prodg${m.unseen ? ' heard' : ''}`);
    const prodTitle = svg('title', {});
    prodTitle.textContent = `${p.title}${m.unseen ? `: ${m.unseen} new signals` : ''}`;
    prod.append(prodTitle, svg('circle', { cx: C, cy: C, r: 16 }, 'mt-prod-r'), svg('circle', { cx: C, cy: C, r: 9 }, 'mt-prod-f'));
    el.append(prod);
    // The areas, each in its place; one the page has not shown before eases out from the product (a CSS animation).
    for (const a of areas) {
      const pt = at.get(a.id)!;
      const on = mem.sel.kind === 'area' && mem.sel.id === a.id;
      const fresh = !mem.shownAreas.has(a.id);
      mem.shownAreas.add(a.id);
      const waiting = byArea.get(a.id) ?? [];
      const g = svg<SVGGElement>('g', { style: `--dx:${(C - pt.x).toFixed(0)}px;--dy:${(C - pt.y).toFixed(0)}px` }, fresh ? 'mt-node fresh' : 'mt-node');
      const label = a.name.length > 18 ? `${a.name.slice(0, 17)}…` : a.name;
      const area = svg<SVGCircleElement>('circle', { cx: pt.x.toFixed(1), cy: pt.y.toFixed(1), r: on ? 15 : 12, tabindex: 0, role: 'button' }, `mt-area ${healthTone(a.health)}${on ? ' on' : ''}`);
      const areaTitle = svg('title', {});
      areaTitle.textContent = `${a.name}: ${a.health}${waiting.length ? ` · ${waiting.length} waiting` : ''}`;
      area.append(areaTitle);
      const pick = (ev: Event): void => {
        ev.stopPropagation();
        choose(on ? { kind: 'none' } : { kind: 'area', id: a.id });
      };
      area.addEventListener('click', pick);
      area.addEventListener('keydown', (ev) => {
        if (ev.key === 'Enter' || ev.key === ' ') {
          ev.preventDefault();
          pick(ev);
        }
      });
      g.append(area);
      // The waiting proposals as small dots around the area, one per proposal (at most 8).
      waiting.slice(0, 8).forEach((pr, k) => {
        const t = -Math.PI / 2 + (k * Math.PI) / 4;
        g.append(svg('circle', { cx: (pt.x + 21 * Math.cos(t)).toFixed(1), cy: (pt.y + 21 * Math.sin(t)).toFixed(1), r: 3.5 }, `mt-prop-dot ${pr.kind}`));
      });
      const text = svg<SVGTextElement>('text', { x: pt.x.toFixed(1), y: (pt.y + 28).toFixed(1), 'text-anchor': 'middle' }, 'mt-lab');
      text.textContent = label;
      g.append(text);
      el.append(g);
    }
    return el;
  };

  /** Beside the map: the selected area or proposal, else the product and what waits for the owner. */
  const sidePanel = (m: Maintenance): (Node | null)[] => {
    if (mem.sel.kind === 'area') {
      const id = mem.sel.id;
      const a = m.map.areas.find((x) => x.id === id);
      if (a) return areaPanel(a, m);
    }
    if (mem.sel.kind === 'proposal') {
      const id = mem.sel.id;
      const pr = m.proposals.find((x) => x.id === id);
      if (pr) return [proposalCard(pr, true)];
    }
    // Nothing chosen: the product in a paragraph and the best waiting proposals, one line each.
    const top = waitingFirst(m.proposals).slice(0, 6);
    return [
      h('h3', null, 'The product'),
      m.map.summary ? h('p', null, m.map.summary) : null,
      top.length
        ? h(
            'ol.mt-top',
            null,
            top.map((pr) => h('li', null, h('button.mt-linkbtn', { type: 'button', onclick: () => choose({ kind: 'proposal', id: pr.id }) }, pr.title), ' ', valueDots(pr.value.score))),
          )
        : null,
      m.map.areas.length ? h('p.pj-faint', null, 'Click an area or a proposal to see more.') : null,
    ];
  };

  const areaPanel = (a: Area, m: Maintenance): (Node | null)[] => {
    const waiting = waitingFirst(m.proposals).filter((x) => (x.area ?? x.impact.areas[0]) === a.id);
    const talks = a.links.map((l) => button(m.map.areas.find((x) => x.id === l)?.name ?? l, () => choose({ kind: 'area', id: l }), { kind: 'ghost', size: 'sm' }));
    return [
      h('h3', null, dot(healthTone(a.health)), a.name),
      h('p', null, a.summary),
      a.notes ? h('p.pj-faint', null, a.notes) : null,
      a.paths.length ? h('div.mt-chips', null, a.paths.map((x) => h('code', null, x))) : null,
      a.links.length ? h('div.mt-talks', null, 'Talks to ', talks) : null,
      ...(waiting.length ? waiting.map((pr) => proposalCard(pr, mem.expert)) : [h('p.pj-faint', null, 'Nothing proposed here.')]),
      h('p.pj-faint', null, `Updated ${ago(a.updated_ms)} by ${a.by || 'a watcher'}`),
    ];
  };

  const signalRow = (s: Signal): HTMLElement =>
    h(
      'li.mt-sig',
      { 'data-key': s.id },
      badge(s.kind, s.kind === 'error' ? 'danger' : s.kind === 'report' ? 'warn' : 'muted'),
      h(
        'div.mt-sigt',
        null,
        h('span', { title: s.text }, signalLine(s)),
        h('span.pj-faint', null, `${s.from}${s.environment ? ` · ${s.environment}` : ''} · ${ago(s.at_ms)}`),
      ),
      h(
        'span.mt-sigend',
        null,
        s.url ? h('a', { href: s.url, target: '_blank', rel: 'noopener' }, 'open') : null,
        iconBtn('trash-2', 'Delete this signal', () => void attempt(() => projectsApi.deleteSignal(p.id, s.id)).then(() => void load())),
      ),
    );

  const addSignal = (): void => {
    const kind = select(
      SIGNAL_KINDS.map((k) => ({ value: k, label: k })),
      'feedback',
    );
    const title = input({ placeholder: 'Checkout fails on Safari' });
    const text = textarea({ rows: 4, placeholder: 'What a user said, a figure from your analytics, an error report…' });
    const from = input({ placeholder: 'support, a customer, your analytics' });
    dialog('Add a signal', [field('Kind', kind), field('Title (optional)', title), field('What it says', text), field('From (optional)', from)], (close) => [
      lbtn('Cancel', () => close(), { kind: 'ghost' }),
      lbtn(
        'Add',
        () => {
          if (!text.value.trim()) {
            toast('Say what the signal says', 'error');
            return;
          }
          void attempt(() => projectsApi.addSignal(p.id, { kind: kind.value as SignalKind, title: title.value, text: text.value, from: from.value || undefined }), 'Signal added').then((done) => {
            if (!done) return;
            close();
            void load();
          });
        },
        { kind: 'primary' },
      ),
    ]);
  };

  const ingest = (): void => {
    void attempt(() => projectsApi.newIngest(p.id)).then((made) => {
      if (!made) return;
      const url = `${location.origin}${made.path}`;
      const example = `curl -X POST ${url} \\\n  -H 'content-type: application/json' \\\n  -d '{"kind": "report", "title": "Checkout fails", "text": "The pay button does nothing on Safari"}'`;
      modal('The ingest address', h('div.pj-dlg', null, h('p', null, 'Send user reports, feedback and webhooks here. It is shown once: copy it now. Making a new one stops this one.'), h('pre.mt-pre', null, url), h('p.pj-faint', null, 'JSON with text (and kind, title, from, url, environment), or plain text:'), h('pre.mt-pre', null, example)));
      void load();
    });
  };

  const sweeps = (m: Maintenance): void => {
    const every = select(
      EVERY.map((n) => ({ value: String(n), label: everyText(n) })),
      String(m.config.every_hours),
    );
    dialog(
      'Sweeps',
      [
        field('Sweep', every, 'A sweep is one watcher reading what came in, updating the map and proposing.'),
        h('p.pj-faint', null, m.watchers.length ? `Watchers, in turn: ${m.watchers.map(nameOf).join(', ')}.` : 'No agent watches it yet.'),
      ],
      (close) => [
        lbtn('Cancel', () => close(), { kind: 'ghost' }),
        lbtn(
          'Save',
          () => {
            void attempt(() => projectsApi.setMaintenance(p.id, { every_hours: Number(every.value) }), 'Saved').then((done) => {
              if (!done) return;
              close();
              void load();
            });
          },
          { kind: 'primary' },
        ),
      ],
    );
  };

  // ---- The page --------------------------------------------------------------------------------------------------

  const intro = (): HTMLElement =>
    empty(
      'search',
      'This project is being built',
      'In maintenance, its agents read its analytics, user reports and feedback, keep a map of it and propose work: what would change, for whom, its value and its risk. Nothing runs until you pick a proposal.',
      lbtn('Put it in maintenance', () => {
        void attempt(() => projectsApi.setMode(p.id, 'maintenance'), `${p.title} is in maintenance`).then((done) => {
          if (done) onModeChange();
        });
      }, { kind: 'primary' }),
    );

  const draw = (): void => {
    if (!data) return;
    const m = data;
    if (p.mode !== 'maintenance' && m.sweep.runs === 0 && !m.sweep.running && m.proposals.length === 0 && m.signals.length === 0) {
      replace(root, intro());
      return;
    }
    const now = Date.now();
    const waiting = waitingFirst(m.proposals);
    const done = decided(m.proposals);
    const instruments = waiting.filter((x) => x.instrument);
    const focus = input({ id: 'mt-focus', value: m.config.focus, placeholder: 'What matters now: checkout conversion, the mobile crashes…' });
    focus.addEventListener('change', () => {
      const v = focus.value.trim();
      if (v === (m.config.focus || '')) return;
      void attempt(() => projectsApi.setMaintenance(p.id, { focus: v }), 'The watchers will read it').then(() => void load());
    });
    // A redraw replaces the tab's content, which pulls the window up; the owner stays where they were reading.
    const y = window.scrollY;
    const sigs = m.signals.slice(0, mem.expert ? 200 : 15);
    const inboxActs: HTMLElement[] = [
      lbtn('Add a signal', addSignal, { size: 'sm', icon: 'plus' }),
      button(m.sources.ingest ? 'New ingest address' : 'Ingest address', ingest, { size: 'sm', title: 'An address your app, support inbox or error tracker posts to' }),
    ];
    if (m.sources.ingest) inboxActs.push(button('Stop it', () => void attempt(() => projectsApi.stopIngest(p.id), 'The ingest address stopped').then(() => void load()), { size: 'sm', kind: 'ghost' }));
    const reads: (Node | null)[] = [h('li', null, `The inbox: ${m.sources.inbox} signals${m.sources.ingest ? ', and its ingest address' : ''}`)];
    if (p.repo) reads.push(h('li', null, 'The code: ', h('a', { href: href('history', p.repo) }, p.repo)));
    m.sources.connectors.forEach((c) => reads.push(h('li', null, `${c.label} (${c.category})`)));
    m.sources.mcp.forEach((x) => reads.push(h('li', null, `The MCP server ${x.name}`)));
    if (!m.sources.connectors.length && !m.sources.mcp.length) {
      reads.push(h('li.pj-faint', null, 'No analytics, error or support tools are connected yet. ', h('a', { href: href('mcp') }, 'Add an MCP server'), ' or ', h('a', { href: href('tools') }, 'a connector'), ', or let the watchers propose one.'));
    }
    replace(
      root,
      p.mode !== 'maintenance'
        ? h('div.mt-note', null, h('span', null, 'This project is in build mode now: its watchers no longer sweep.'), lbtn('Back to maintenance', () => void attempt(() => projectsApi.setMode(p.id, 'maintenance')).then((x) => x && onModeChange()), { kind: 'ghost', size: 'sm' }))
        : null,
      h(
        'div.mt-strip',
        null,
        h(
          'div.mt-titles',
          null,
          h('div.mnt-head', { class: waiting.length ? 'needs' : null }, waiting.length ? h('span.mt-hdot', { 'aria-hidden': 'true' }) : null, headline(m)),
          h('div.mt-sweep', null, `${sweepLine(m, now, nameOf)} · ${everyText(m.config.every_hours)}`),
        ),
        h(
          'div.mt-acts',
          null,
          lbtn('Sweep now', () => void attempt(() => projectsApi.sweep(p.id), 'A watcher is sweeping').then(() => void load()), { size: 'sm', disabled: !!m.sweep.running }),
          iconBtn('settings', 'How often it sweeps, and who watches', () => sweeps(m)),
          h('label.check', null, h('input', { type: 'checkbox', checked: mem.expert, onchange: (ev: Event) => { mem.expert = (ev.target as HTMLInputElement).checked; draw(); } }), h('span', null, 'Expert')),
        ),
      ),
      h('div.mt-focus', null, h('span.field-l', null, 'Focus'), focus),
      h('div.mt-stage', null, h('div.mt-map', null, mapBody(m)), h('aside.mt-side', null, sidePanel(m))),
      m.sweep.last_summary && m.sweep.runs > 0 ? h('details.mt-disc', null, h('summary', null, 'What the last sweep said'), h('p', null, m.sweep.last_summary)) : null,
      card(
        'Proposals',
        waiting.length
          ? h('div.mt-props', null, waiting.map((pr) => proposalCard(pr, mem.expert)))
          : empty('search', 'Nothing proposed yet', m.watchers.length ? 'The watchers propose after they read what comes in.' : 'Add an agent to the project (a planner or a researcher) so someone watches it.'),
      ),
      instruments.length
        ? card('Data the watchers lack', h('ul.mt-lack', null, instruments.map((pr) => h('li', null, h('b', null, pr.instrument?.name ?? pr.title), ` — ${pr.instrument?.data ?? pr.summary}`))))
        : null,
      mem.expert && done.length
        ? card(`Decided (${done.length})`, done.map((pr) => proposalCard(pr, true)))
        : done.length
          ? h('p.pj-faint', null, `${done.length} decided — `, h('button.mt-linkbtn', { type: 'button', onclick: () => { mem.expert = true; draw(); } }, 'show them'))
          : null,
      card(
        `Inbox (${m.signals_total}${m.unseen ? `, ${m.unseen} new` : ''})`,
        sigs.length ? h('ul.mt-inbox', null, sigs.map(signalRow)) : empty('search', 'Nothing has come in yet.', 'Add what users told you, or give your app or error tracker the ingest address.'),
        { actions: inboxActs },
      ),
      card('What the watchers read', h('ul.mt-read', null, reads)),
    );
    if (window.scrollY !== y) window.scrollTo({ top: y });
  };

  void load();
  return root;
}
