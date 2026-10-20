// The first-run wizard (six steps, in a modal), the tour offer, and the tour itself: a callout next to each
// sidebar screen's link. The words and steps come from apps/shared/onboarding.json (flow.ts), the same as the
// desktop app, and so do the flags. The markup is the prototype's (m-onboard.js); its rules are in
// styles/proto/onboard.css.

import '../styles/proto/onboard.css';
import { api, type Machine, type Overview, type Provider } from '../api';
import { weft } from '../api-weft';
import { go } from '../router';
import { TOUR_STEPS, WIZARD_STEPS, browserStore, COPY, fill, keysSet, moveStep, readFlag, writeFlag } from '../onboarding/flow';
import { chooseLook } from '../views/appearance';
import { badge, button, dot, type Tone } from './components';
import { h, lu, replace } from './dom';
import { current } from './theme';

/**
 * What the hub says about the plaza's requirements; null where it could not be read. The browser cannot
 * see Docker or the containers: those are checked on the host (`agentplaza setup`).
 */
export interface Deps {
  services: boolean | null;
  hypervisor: boolean | null;
  weft: boolean | null;
}

/** What the checks found. `keys` is null when the providers could not be read. */
export interface Status {
  reachable: boolean;
  hub: boolean;
  node: boolean;
  keys: { n: number; m: number } | null;
  providers: Provider[] | null;
  deps: Deps;
}

/** The checks the wizard's steps show: the hub's overview, providers, machine and Weft. */
export async function loadStatus(): Promise<Status> {
  let overview: Overview | null = null;
  let providers: Provider[] | null = null;
  let machine: Machine | null = null;
  let weftStatus: { running: boolean } | null = null;
  try {
    overview = await api.overview();
  } catch {
    // unreachable: the rows say so
  }
  try {
    providers = await api.providers();
  } catch {
    // unknown keys: the row says so
  }
  try {
    machine = await api.machine();
  } catch {
    // unknown hypervisor: the row says so
  }
  try {
    weftStatus = await weft.status();
  } catch {
    // unknown Weft: the row says so
  }
  return {
    reachable: overview !== null,
    hub: overview?.hub ?? false,
    node: overview?.noded ?? false,
    keys: providers ? keysSet(providers) : null,
    providers,
    deps: {
      services: overview ? overview.hub : null,
      hypervisor: machine ? Boolean(machine.backend) : null,
      weft: weftStatus ? weftStatus.running : null,
    },
  };
}

/** One dependency row: its state, the words beside it and, when missing, what to run on the host. */
interface DepRow {
  label: string;
  tone: Tone;
  detail: string;
  hint?: string;
  command?: string;
}

/**
 * The dependencies step's rows. The browser cannot install anything: a missing one shows the command to
 * run on the host (the CLI's `agentplaza setup --fix <need>`), or the desktop app for the rest.
 */
export function depRows(s: Status | null): DepRow[] {
  const D = COPY.wizard.deps;
  const unread = (label: string): DepRow => ({ label, tone: 'muted', detail: COPY.wizard.checking });
  const d: Deps = s?.deps ?? { services: null, hypervisor: null, weft: null };
  return [
    d.services === null
      ? unread(D.rows.services)
      : d.services
        ? { label: D.rows.services, tone: 'ok', detail: 'Running' }
        : { label: D.rows.services, tone: 'danger', detail: 'Not running', hint: D.web_services },
    d.hypervisor === null
      ? unread(D.rows.hypervisor)
      : d.hypervisor
        ? { label: D.rows.hypervisor, tone: 'ok', detail: 'Available' }
        : { label: D.rows.hypervisor, tone: 'warn', detail: 'Not available', hint: D.web_hypervisor },
    d.weft === null
      ? unread(D.rows.weft)
      : d.weft
        ? { label: D.rows.weft, tone: 'ok', detail: 'Running' }
        : { label: D.rows.weft, tone: 'warn', detail: 'Not running', hint: D.web_weft },
    { label: D.rows.host, tone: 'muted', detail: D.web_host, command: 'agentplaza setup' },
  ];
}

/** One dependency as a row: its label, Ready or Not ready, and the hint or command for what is missing. */
function depRow(r: DepRow): HTMLElement {
  const state = r.command || r.tone === 'muted' ? null : r.tone === 'ok' ? badge('Ready', 'ok', true) : badge('Not ready', 'warn', true);
  let note: HTMLElement | null;
  if (r.command) note = h('small.muted', null, r.detail, ' ', h('span.mono', null, r.command));
  else if (r.tone === 'ok') note = null;
  else if (r.tone === 'muted') note = h('small.muted', null, r.detail);
  else note = h('small.muted', null, r.hint ?? '');
  return h('li.onb-row', null, h('div', null, h('b', null, r.label), note), state);
}

/** The checks step's rows: null where the check has not been read yet. */
function checkRows(s: Status | null): { label: string; ok: boolean | null }[] {
  const L = COPY.wizard.checks_labels;
  return [
    { label: L.hub, ok: s ? s.reachable && s.hub : null },
    { label: L.node, ok: s ? s.node : null },
    { label: L.keys, ok: s ? s.keys !== null && s.keys.n > 0 : null },
    { label: L.local, ok: s ? s.deps.services === true && s.deps.hypervisor === true && s.deps.weft === true : null },
  ];
}

// ---- the wizard ----------------------------------------------------------

let wizard: HTMLElement | null = null;
let wizardKey: ((e: KeyboardEvent) => void) | null = null;
let lookError = '';

function closeWizard(): void {
  wizard?.remove();
  wizard = null;
  if (wizardKey) document.removeEventListener('keydown', wizardKey);
  wizardKey = null;
}

/**
 * The look step: Loom or Graphite as the owner's look (stored the same way as any change in Appearance), and a link
 * to the Appearance screen for everything else. `redraw` runs after a choice; `toAppearance` ends the wizard and opens it.
 */
function lookStep(redraw: () => void, toAppearance: () => void): HTMLElement {
  const L = COPY.wizard.look;
  const choose = (palette: string): void => {
    lookError = '';
    chooseLook(palette).then(redraw, (err: unknown) => {
      lookError = err instanceof Error ? err.message : 'The look could not be saved.';
      redraw();
    });
  };
  const picked = current().appearance.theme;
  const pick = (palette: string, name: string, about: string): HTMLElement =>
    h('button.look-pick', { type: 'button', 'aria-pressed': String(picked === palette), onclick: () => choose(palette) }, h('strong', null, name), h('span', null, about));
  return h(
    'div.look-step',
    null,
    h('div.look-choices', null, pick('loom-clay', L.loom, L.loom_about), pick('graphite-cobalt', L.graphite, L.graphite_about)),
    lookError ? h('p.onboard-error', { role: 'alert' }, lookError) : null,
    h('div.onboard-actions', null, button(L.open_appearance, toAppearance, { kind: 'ghost', size: 'sm' })),
  );
}

/**
 * Shows the wizard. `onEnd` runs after Finish or Skip setup (the flag is set first). The close button and a
 * click on the scrim close it without setting the flag, so it opens again on the next start.
 * Once per first start, and again from the Setup screen.
 */
export function openWizard(onEnd: () => void): void {
  closeWizard();
  const store = browserStore();
  const sheet = h('div.sheet.onboard', { role: 'dialog', 'aria-modal': 'true' });
  const scrim = h('div.scrim', null, sheet);
  document.body.appendChild(scrim);
  wizard = scrim;
  let index = 0;
  let status: Status | null = null;

  const dismiss = (): void => closeWizard();
  const finish = (): void => {
    writeFlag(store, 'setup');
    closeWizard();
    onEnd();
  };
  wizardKey = (e: KeyboardEvent): void => {
    if (e.key === 'Escape') dismiss();
  };
  document.addEventListener('keydown', wizardKey);
  scrim.addEventListener('mousedown', (e) => {
    if (e.target === scrim) dismiss();
  });

  const rail = (): HTMLElement =>
    h(
      'ol.onb-rail',
      { style: `--steps:${WIZARD_STEPS.length}`, 'aria-label': 'Setup steps' },
      ...WIZARD_STEPS.map((s, i) =>
        h(
          'li',
          { class: i === index ? 'on' : i < index ? 'done' : '', 'aria-current': i === index ? 'step' : null },
          h('i.onb-dot', { role: 'img', 'aria-label': `${i + 1} / ${WIZARD_STEPS.length}` }),
          h('span', null, s.title),
        ),
      ),
    );

  const body = (): Node[] => {
    const step = WIZARD_STEPS[index]!;
    const nodes: Node[] = [rail(), h('p.lede', null, step.body)];
    if (step.id === 'hub') {
      const reach = status === null ? null : status.reachable && status.hub;
      nodes.push(
        h(
          'div.onb-box',
          null,
          h('span', null, `${COPY.wizard.connected_to} `, h('b.mono', null, location.host)),
          reach === null ? badge(COPY.wizard.checking, 'muted', true) : reach ? badge(COPY.wizard.reachable, 'ok', true) : badge(COPY.wizard.unreachable, 'danger', true),
        ),
      );
    } else if (step.id === 'look') {
      nodes.push(
        lookStep(render, () => {
          finish();
          go('appearance');
        }),
      );
    } else if (step.id === 'deps') {
      const rows = depRows(status);
      const missing = rows.some((r) => r.tone === 'warn' || r.tone === 'danger');
      if (missing) nodes.push(h('p.muted', null, COPY.wizard.deps.web_lead));
      nodes.push(h('ul.onb-list', null, ...rows.map(depRow)));
      if (status !== null) {
        if (missing) nodes.push(h('p.muted', null, COPY.wizard.deps.web_desktop), h('p.muted', null, COPY.wizard.deps.not_ready));
        else nodes.push(h('p.muted', null, COPY.wizard.deps.ready));
      }
    } else if (step.id === 'keys') {
      const count = status?.keys ? fill(COPY.wizard.keys_count, { n: status.keys.n, m: status.keys.m }) : COPY.wizard.checking;
      nodes.push(h('p', null, count));
      if (status?.providers) {
        nodes.push(h('ul.onb-list', null, ...status.providers.map((p) => h('li.onb-row', null, h('span.mono', null, p.id), dot(p.credential !== 'missing' ? 'ok' : 'muted')))));
      }
      nodes.push(
        h(
          'div.onboard-actions',
          null,
          button(
            COPY.wizard.open_models,
            () => {
              closeWizard();
              go('models');
            },
            { kind: 'ghost', size: 'sm' },
          ),
        ),
      );
    } else if (step.id === 'checks') {
      const rows = checkRows(status);
      nodes.push(
        h(
          'ul.onb-list',
          null,
          ...rows.map((r) => h('li.onb-row', null, h('span', null, r.label), r.ok === null ? badge(COPY.wizard.checking, 'muted', true) : badge(r.ok ? 'Ready' : 'Not ready', r.ok ? 'ok' : 'warn', true))),
        ),
      );
      if (status) nodes.push(h('p.muted', null, fill(COPY.wizard.checks_ready, { n: rows.filter((r) => r.ok).length, m: rows.length })));
    }
    return nodes;
  };

  const render = (): void => {
    const step = WIZARD_STEPS[index]!;
    const last = index === WIZARD_STEPS.length - 1;
    sheet.setAttribute('aria-label', step.title);
    replace(
      sheet,
      h(
        'header.sheet-h',
        null,
        h('h2', null, step.title),
        button(COPY.wizard.skip, finish, { kind: 'ghost', size: 'sm' }),
        h('button.iconbtn', { type: 'button', title: 'Close', 'aria-label': 'Close', onclick: dismiss }, lu('x', 16)),
      ),
      h('div.sheet-b', null, ...body()),
      h(
        'footer.sheet-f',
        null,
        button(
          COPY.wizard.back,
          () => {
            index = moveStep(index, -1, WIZARD_STEPS.length);
            render();
          },
          { kind: 'ghost', disabled: index === 0 },
        ),
        h('span.sp'),
        button(
          last ? COPY.wizard.finish : COPY.wizard.next,
          () => {
            if (last) {
              finish();
              return;
            }
            index = moveStep(index, 1, WIZARD_STEPS.length);
            render();
          },
          { kind: 'primary' },
        ),
      ),
    );
  };

  render();
  void loadStatus().then((s) => {
    status = s;
    if (wizard === scrim) render();
  });
}

// ---- the tour -------------------------------------------------------------

/** Where each stop's callout points: the sidebar link of its screen, or the nearest thing that stands for it. */
const TOUR_AT: Record<string, string[]> = {
  overview: ['nav.nav a.nrow[href$="/overview"]'],
  agents: ['nav.nav a.nrow[href$="/agents"]'],
  floor: ['nav.nav a.nrow[href$="/machines"]'],
  projects: ['.side .proj', '.side-search'],
  approvals: ['nav.nav a.nrow[href$="/approvals"]'],
  conversations: ['nav.nav a.nrow[href$="/conversations"]', '.side .thread[href^="#/talk/"]', 'nav.nav'],
  models: ['.side-foot a[href^="#/settings"]'],
  setup: ['.side-foot .hub', '.side-foot'],
};

let tourCard: HTMLElement | null = null;
let tourIdx = -1;
let tourObs: MutationObserver | null = null;

function tourTarget(screen: string): HTMLElement | null {
  for (const sel of TOUR_AT[screen] ?? []) {
    const el = document.querySelector<HTMLElement>(sel);
    if (el) return el;
  }
  return null;
}

/**
 * Puts the callout beside its stop's element (14 px away, centred on it, with an arrow), or centres it when the
 * element is not on screen (a closed drawer). The element gets the soft ring.
 */
function tourPlace(): void {
  const card = tourCard;
  if (!card) return;
  const el = tourTarget(TOUR_STEPS[tourIdx]!.screen);
  document.querySelectorAll<HTMLElement>('.tour-on').forEach((x) => {
    if (x !== el) x.classList.remove('tour-on');
  });
  if (el && !el.classList.contains('tour-on')) el.classList.add('tour-on');
  const r = el ? el.getBoundingClientRect() : null;
  const seen = !!r && r.width > 0 && r.height > 0 && r.right > 0 && r.left < innerWidth;
  card.classList.toggle('centred', !seen);
  card.classList.remove('flip');
  if (!r || !seen) {
    card.style.left = '';
    card.style.top = '';
    return;
  }
  const cw = card.offsetWidth || 320;
  const ch = card.offsetHeight || 170;
  let left = r.right + 14;
  if (left + cw > innerWidth - 12) {
    left = r.left - 14 - cw;
    card.classList.add('flip');
  }
  left = Math.max(12, Math.min(left, innerWidth - cw - 12));
  const mid = r.top + r.height / 2;
  const top = Math.max(12, Math.min(mid - ch / 2, innerHeight - ch - 12));
  card.style.left = `${left}px`;
  card.style.top = `${top}px`;
  card.style.setProperty('--arrow-y', `${Math.max(16, Math.min(ch - 16, mid - top))}px`);
}

function tourMove(delta: number): void {
  tourIdx = moveStep(tourIdx, delta, TOUR_STEPS.length);
  tourPaint();
}

/** Writes the stop's words when its index changes, then places the callout. */
function tourPaint(): void {
  const card = tourCard;
  if (!card) return;
  const list = TOUR_STEPS;
  const s = list[tourIdx]!;
  const last = tourIdx === list.length - 1;
  if (card.dataset.at !== String(tourIdx)) {
    card.dataset.at = String(tourIdx);
    replace(
      card,
      h('span.tour-count', null, fill(COPY.tour.count, { n: tourIdx + 1, m: list.length })),
      h('b.tour-t', null, s.title),
      h('p.tour-b', null, s.body),
      h(
        'footer.tour-f',
        null,
        button(COPY.tour.skip, () => stopTour(true), { kind: 'ghost', size: 'sm' }),
        h('span.sp'),
        button(COPY.tour.back, () => tourMove(-1), { kind: 'ghost', size: 'sm', disabled: tourIdx === 0 }),
        button(last ? COPY.tour.done : COPY.tour.next, () => (last ? stopTour(true) : tourMove(1)), { kind: 'primary', size: 'sm' }),
      ),
    );
  }
  tourPlace();
}

/** Escape ends the tour (and sets the flag); captured, so the palette and the sheets do not also take it. */
function tourKey(e: KeyboardEvent): void {
  if (e.key === 'Escape' && tourCard) {
    e.preventDefault();
    e.stopImmediatePropagation();
    stopTour(true);
  }
}

/** Starts the tour at its first step. It ends (and sets the flag) at Done, Skip tour or Escape. */
export function startTour(): void {
  stopTour(false);
  dropOffer();
  const card = h('div.tour-card', { role: 'dialog', 'aria-live': 'polite', 'aria-label': TOUR_STEPS[0]!.title });
  document.body.appendChild(card);
  tourCard = card;
  tourIdx = 0;
  // The sidebar and the page are redrawn in place: the ring and the callout follow each redraw.
  const app = document.getElementById('app');
  if (app) {
    tourObs = new MutationObserver(() => tourPaint());
    tourObs.observe(app, { childList: true, subtree: true, attributes: true, attributeFilter: ['class'] });
  }
  document.addEventListener('keydown', tourKey, true);
  window.addEventListener('resize', tourPlace);
  tourPaint();
}

/** Ends the running tour; `persist` sets the flag (Done, Skip tour, Escape). */
function stopTour(persist: boolean): void {
  if (!tourCard) return;
  if (persist) writeFlag(browserStore(), 'tour');
  document.querySelectorAll<HTMLElement>('.tour-on').forEach((x) => x.classList.remove('tour-on'));
  tourObs?.disconnect();
  tourObs = null;
  document.removeEventListener('keydown', tourKey, true);
  window.removeEventListener('resize', tourPlace);
  tourCard.remove();
  tourCard = null;
  tourIdx = -1;
}

// ---- the offer -------------------------------------------------------------

const OFFER_ID = 'onb-offer';

function dropOffer(): void {
  document.getElementById(OFFER_ID)?.remove();
}

/** The "Take a 1-minute tour?" offer in the corner, once the wizard ends (unless the tour was already done). */
export function offerTour(): void {
  const store = browserStore();
  if (readFlag(store, 'tour')) return;
  dropOffer();
  document.body.appendChild(
    h(
      'div.tour-card.offer',
      { id: OFFER_ID, role: 'dialog', 'aria-label': COPY.offer.title },
      h('b.tour-t', null, COPY.offer.title),
      h(
        'footer.tour-f',
        null,
        button(
          COPY.offer.skip,
          () => {
            writeFlag(store, 'tour');
            dropOffer();
          },
          { kind: 'ghost', size: 'sm' },
        ),
        h('span.sp'),
        button(COPY.offer.start, () => startTour(), { kind: 'primary', size: 'sm' }),
      ),
    ),
  );
}

/** On the first start (the setup flag not set): the wizard, then the tour offer. */
export function runFirstStart(): void {
  if (readFlag(browserStore(), 'setup')) return;
  openWizard(offerTour);
}
