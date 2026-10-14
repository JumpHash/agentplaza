// The first-run wizard (four steps, in a modal), the tour offer, and the tour itself: a callout
// next to each sidebar screen's link. The words and steps come from apps/shared/onboarding.json
// (flow.ts); the same flags and steps as the desktop app.

import '../styles/onboarding.css';
import { api, type Machine, type Overview, type Provider } from '../api';
import { go } from '../router';
import { TOUR_STEPS, WIZARD_STEPS, browserStore, COPY, fill, keysSet, moveStep, readFlag, writeFlag } from '../onboarding/flow';
import { button, dot, type Tone } from './components';
import { h, replace } from './dom';

/**
 * What the hub says about the plaza's requirements; null where it could not be read. The browser cannot
 * see Docker or the containers: those are checked on the host (`agentplaza setup`).
 */
export interface Deps {
  services: boolean | null;
  hypervisor: boolean | null;
  git: boolean | null;
}

/** What the checks found. `keys` is null when the providers could not be read. */
export interface Status {
  reachable: boolean;
  hub: boolean;
  node: boolean;
  keys: { n: number; m: number } | null;
  deps: Deps;
}

/** One check row: label, tone and the words beside it. */
interface Row {
  label: string;
  tone: Tone;
  detail: string;
}

/** The checks the wizard's steps show: the hub's overview, providers, machine and git host. */
export async function loadStatus(): Promise<Status> {
  let overview: Overview | null = null;
  let providers: Provider[] | null = null;
  let machine: Machine | null = null;
  let git: { configured: boolean } | null = null;
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
    git = await api.git();
  } catch {
    // unknown git host: the row says so
  }
  return {
    reachable: overview !== null,
    hub: overview?.hub ?? false,
    node: overview?.noded ?? false,
    keys: providers ? keysSet(providers) : null,
    deps: {
      services: overview ? overview.hub : null,
      hypervisor: machine ? Boolean(machine.backend) : null,
      git: git ? git.configured : null,
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
  const d: Deps = s?.deps ?? { services: null, hypervisor: null, git: null };
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
    d.git === null
      ? unread(D.rows.git)
      : d.git
        ? { label: D.rows.git, tone: 'ok', detail: 'Configured' }
        : { label: D.rows.git, tone: 'warn', detail: 'Not configured', command: 'agentplaza setup --fix forgejo' },
    { label: D.rows.host, tone: 'muted', detail: D.web_host, command: 'agentplaza setup' },
  ];
}

/** The dependencies step's list: a dot, the label, the words and the command to run on the host. */
function depsList(s: Status | null): HTMLElement {
  return h(
    'ul.onboard-checks.onboard-deps',
    null,
    ...depRows(s).map((r) =>
      h(
        'li',
        null,
        dot(r.tone),
        h('span', null, r.label),
        h('span.detail', null, r.detail),
        r.hint ? h('span.detail', null, r.hint) : null,
        r.command ? h('code', null, r.command) : null,
      ),
    ),
  );
}

function rows(s: Status | null): Row[] {
  const L = COPY.wizard.checks_labels;
  if (!s) return [{ label: L.hub, tone: 'muted', detail: COPY.wizard.checking }];
  return [
    { label: L.hub, tone: s.reachable && s.hub ? 'ok' : 'danger', detail: s.reachable && s.hub ? COPY.wizard.reachable : COPY.wizard.unreachable },
    { label: L.node, tone: s.node ? 'ok' : 'warn', detail: s.node ? 'Running' : 'Not running' },
    {
      label: L.keys,
      tone: !s.keys ? 'muted' : s.keys.n > 0 ? 'ok' : 'warn',
      detail: s.keys ? fill(COPY.wizard.keys_count, { n: s.keys.n, m: s.keys.m }) : 'Unknown',
    },
  ];
}

/** The check rows as a list: a dot, the label and the detail. */
export function checkList(s: Status | null): HTMLElement {
  return h(
    'ul.onboard-checks',
    null,
    ...rows(s).map((r) => h('li', null, dot(r.tone), h('span', null, r.label), h('span.detail', null, r.detail))),
  );
}

// ---- the wizard ----------------------------------------------------------

let wizard: HTMLElement | null = null;

function closeWizard(): void {
  wizard?.remove();
  wizard = null;
}

/**
 * Shows the wizard. `onEnd` runs after Finish or Skip setup (the flag is set first).
 * Once per first start, and again from the Setup screen.
 */
export function openWizard(onEnd: () => void): void {
  closeWizard();
  const store = browserStore();
  const backdrop = h('div.modal-backdrop');
  const card = h('div.modal.onboard', { role: 'dialog', 'aria-modal': 'true', 'aria-label': COPY.wizard.steps[0]!.title });
  backdrop.appendChild(card);
  document.body.appendChild(backdrop);
  wizard = backdrop;
  let index = 0;
  let status: Status | null = null;

  const finish = (): void => {
    writeFlag(store, 'setup');
    closeWizard();
    onEnd();
  };

  const body = (): Node[] => {
    const step = WIZARD_STEPS[index]!;
    const nodes: Node[] = [h('h2.onboard-title', null, step.title), h('p.onboard-lead', null, step.body)];
    if (step.id === 'hub') {
      nodes.push(
        h('p', null, `${COPY.wizard.connected_to} ${location.host}`),
        h('p.onboard-status', null, dot(status?.reachable ? 'ok' : status ? 'danger' : 'muted'), status ? (status.reachable ? COPY.wizard.reachable : COPY.wizard.unreachable) : COPY.wizard.checking),
      );
    }
    if (step.id === 'keys') {
      const count = status?.keys ? fill(COPY.wizard.keys_count, { n: status.keys.n, m: status.keys.m }) : COPY.wizard.checking;
      nodes.push(
        h('p.onboard-status', null, dot(status?.keys && status.keys.n > 0 ? 'ok' : 'warn'), count),
        h('div.onboard-actions', null, button(COPY.wizard.open_models, () => go('models'), { kind: 'ghost', size: 'sm' })),
      );
    }
    if (step.id === 'deps') nodes.push(h('p', null, COPY.wizard.deps.web_lead), depsList(status), h('p.muted', null, COPY.wizard.deps.web_desktop));
    if (step.id === 'checks') nodes.push(checkList(status));
    return nodes;
  };

  const render = (): void => {
    const last = index === WIZARD_STEPS.length - 1;
    const dots = WIZARD_STEPS.map((_, i) => h('span', { class: i === index ? 'on' : '' }));
    replace(
      card,
      h('header.card-head', null, h('div.onboard-dots', { 'aria-label': `${index + 1} / ${WIZARD_STEPS.length}` }, ...dots), h('span.spacer'), button(COPY.wizard.skip, finish, { kind: 'ghost', size: 'sm' })),
      h('div.card-body', null, ...body()),
      h(
        'footer.onboard-foot',
        null,
        button(COPY.wizard.back, () => {
          index = moveStep(index, -1, WIZARD_STEPS.length);
          render();
        }, { kind: 'ghost', size: 'sm', disabled: index === 0 }),
        h('span.spacer'),
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
          { kind: 'primary', size: 'sm' },
        ),
      ),
    );
  };

  render();
  void loadStatus().then((s) => {
    status = s;
    if (wizard === backdrop) render();
  });
}

// ---- the tour -------------------------------------------------------------

let tourCard: HTMLElement | null = null;
let tourTeardown: (() => void) | null = null;

/** The sidebar link of a screen (the sidebar is `nav.nav`, each link `a[href="#/<screen>"]`). */
function anchorOf(screen: string): HTMLElement | null {
  return document.querySelector<HTMLElement>(`nav.nav a[href="#/${screen}"]`);
}

/** Puts the callout beside its anchor, or centred when the anchor is not on screen (a closed drawer). */
function place(card: HTMLElement, anchor: HTMLElement | null): void {
  const rect = anchor?.getBoundingClientRect();
  const width = card.offsetWidth;
  const height = card.offsetHeight;
  if (!rect || rect.width === 0) {
    card.classList.add('centered');
    card.style.left = `${Math.max(8, (innerWidth - width) / 2)}px`;
    card.style.top = `${Math.max(8, (innerHeight - height) / 2)}px`;
    return;
  }
  card.classList.remove('centered');
  const centre = rect.top + rect.height / 2;
  const top = Math.min(Math.max(8, centre - 24), innerHeight - height - 8);
  card.style.left = `${Math.min(rect.right + 14, innerWidth - width - 8)}px`;
  card.style.top = `${top}px`;
  card.style.setProperty('--arrow-y', `${centre - top - 6}px`);
}

/** Starts the tour at its first step. It ends (and sets the flag) at Done, Skip tour or Escape. */
export function startTour(): void {
  stopTour(false);
  const card = h('div.tour-card', { role: 'dialog', 'aria-label': COPY.tour.steps[0]!.title });
  document.body.appendChild(card);
  tourCard = card;
  let index = 0;
  let anchor: HTMLElement | null = null;

  const highlight = (next: HTMLElement | null): void => {
    anchor?.classList.remove('tour-on');
    anchor = next;
    anchor?.classList.add('tour-on');
  };
  const onKey = (e: KeyboardEvent): void => {
    if (e.key === 'Escape') stopTour(true);
  };
  const onResize = (): void => place(card, anchor);
  addEventListener('resize', onResize);
  document.addEventListener('keydown', onKey);
  tourTeardown = () => {
    removeEventListener('resize', onResize);
    document.removeEventListener('keydown', onKey);
    highlight(null);
  };

  const show = (): void => {
    const step = TOUR_STEPS[index]!;
    const last = index === TOUR_STEPS.length - 1;
    highlight(anchorOf(step.screen));
    replace(
      card,
      h('div.tour-count', null, fill(COPY.tour.count, { n: index + 1, m: TOUR_STEPS.length })),
      h('h3', null, step.title),
      h('p', null, step.body),
      h(
        'footer.tour-foot',
        null,
        button(COPY.tour.skip, () => stopTour(true), { kind: 'ghost', size: 'sm' }),
        h('span.spacer'),
        button(COPY.tour.back, () => {
          index = moveStep(index, -1, TOUR_STEPS.length);
          show();
        }, { kind: 'ghost', size: 'sm', disabled: index === 0 }),
        button(last ? COPY.tour.done : COPY.tour.next, () => {
          if (last) stopTour(true);
          else {
            index = moveStep(index, 1, TOUR_STEPS.length);
            show();
          }
        }, { kind: 'primary', size: 'sm' }),
      ),
    );
    place(card, anchor);
  };

  show();
}

/** Ends the running tour; `persist` sets the flag (Done, Skip tour, Escape). */
function stopTour(persist: boolean): void {
  if (!tourCard) return;
  if (persist) writeFlag(browserStore(), 'tour');
  tourTeardown?.();
  tourTeardown = null;
  tourCard.remove();
  tourCard = null;
}

/** The "Take a 1-minute tour?" offer, once the wizard ends (unless the tour was already done). */
export function offerTour(): void {
  const store = browserStore();
  if (readFlag(store, 'tour')) return;
  const box = h('div.modal-backdrop');
  const close = (): void => box.remove();
  const card = h(
    'div.modal.onboard',
    { role: 'dialog', 'aria-modal': 'true', 'aria-label': COPY.offer.title },
    h('div.card-body', null, h('h2.onboard-title', null, COPY.offer.title)),
    h(
      'footer.onboard-foot',
      null,
      button(COPY.offer.skip, () => {
        writeFlag(store, 'tour');
        close();
      }, { kind: 'ghost', size: 'sm' }),
      h('span.spacer'),
      button(COPY.offer.start, () => {
        close();
        startTour();
      }, { kind: 'primary', size: 'sm' }),
    ),
  );
  box.appendChild(card);
  document.body.appendChild(box);
}

/** On the first start (the setup flag not set): the wizard, then the tour offer. */
export function runFirstStart(): void {
  if (readFlag(browserStore(), 'setup')) return;
  openWizard(offerTour);
}
