// An agent's profile: its portrait (its own robot, posed and photographed in the 3D studio) on a
// cartoon ID card, with its role, money, standing with the others, conduct under the laws, work
// record, contacts and memories.

import '../styles/profile.css';
import { api, type Agent, type AgentProfile, type Infraction } from '../api';
import { agentName, roleLabel } from '../roles';
import { ago, badge, compact } from '../ui/components';
import { h, replace } from '../ui/dom';
import { agentHue, roleHue } from './floor';
import { cachedPortrait, portraitKey, portraitSpecJson, rememberPortrait, type PortraitSpec } from './portrait-spec';
import { engineWanted, startThumbEngine, type EngineStage } from './engine-mount';

/** The page's portrait engine: one per page, made when a portrait waits and destroyed a second after none waits. */
let portraitEngine: EngineStage | null = null;
let portraitWaiting = 0;
let portraitIdle = 0;
const pending = new Map<string, Promise<string | null>>();
/** The portrait's size in pixels, as the engine draws it. */
const PORTRAIT_PX = 384;

/** RGBA8 pixels as a PNG data URL, or null where the canvas cannot draw. */
function pngOf(rgba: Uint8Array, w: number, h: number): string | null {
  const cv = document.createElement('canvas');
  cv.width = w;
  cv.height = h;
  const g = cv.getContext('2d');
  if (!g) return null;
  g.putImageData(new ImageData(new Uint8ClampedArray(rgba), w, h), 0, 0);
  return cv.toDataURL('image/png');
}

/** The portrait as a PNG from the engine, or null when the engine cannot draw it. Concurrent asks for one agent share a job. */
function enginePortrait(spec: PortraitSpec): Promise<string | null> {
  const key = portraitKey(spec);
  const running = pending.get(key);
  if (running) return running;
  portraitEngine ??= startThumbEngine();
  window.clearTimeout(portraitIdle);
  portraitWaiting++;
  const job = portraitEngine
    .portrait(portraitSpecJson(spec), PORTRAIT_PX, PORTRAIT_PX)
    .then((rgba) => {
      const url = pngOf(rgba, PORTRAIT_PX, PORTRAIT_PX);
      if (url) rememberPortrait(key, url);
      return url;
    })
    .catch(() => null)
    .finally(() => {
      pending.delete(key);
      portraitWaiting--;
      if (portraitWaiting === 0) {
        portraitIdle = window.setTimeout(() => {
          portraitEngine?.destroy();
          portraitEngine = null;
        }, 1000);
      }
    });
  pending.set(key, job);
  return job;
}

/** The agent's portrait as an element: the photographed robot, or its initial on its colour. */
export function avatar(a: Agent, size = 40): HTMLElement {
  const hue = agentHue(a.cid);
  const spec: PortraitSpec = {
    species: a.species,
    cid: a.cid,
    roleHue: a.tier === 'orchestrator' ? 25 : roleHue(a.role),
    bodyHue: hue,
    orchestrator: a.tier === 'orchestrator',
    state: a.status === 'offline' ? 'offline' : 'idle',
  };
  const frame = h('span.avatar', { style: `width:${size}px;height:${size}px;--hue:${hue}` });
  const picture = (url: string): HTMLElement => h('img', { src: url, alt: '', width: String(size), height: String(size) });
  const letter = (): HTMLElement => h('span.avatar-initial', null, agentName(a).slice(0, 1).toUpperCase());
  // The engine draws the portrait: the letter shows until its picture comes back (and stays without an engine).
  const hit = cachedPortrait(portraitKey(spec));
  if (hit) frame.append(picture(hit));
  else {
    frame.append(letter());
    if (engineWanted()) {
      void enginePortrait(spec).then((url) => {
        if (url) frame.replaceChildren(picture(url));
      });
    }
  }
  if (a.status === 'offline') frame.classList.add('offline');
  return frame;
}

/** Conduct as a word: how law-abiding the agent has been lately. */
export function conductLabel(score: number): { text: string; tone: 'ok' | 'info' | 'warn' | 'danger' } {
  if (score >= 90) return { text: 'Exemplary', tone: 'ok' };
  if (score >= 70) return { text: 'Good standing', tone: 'info' };
  if (score >= 40) return { text: 'Under watch', tone: 'warn' };
  return { text: 'In trouble', tone: 'danger' };
}

function meter(value: number, max: number, tone: string, caption: string): HTMLElement {
  const pct = Math.max(0, Math.min(100, (value / max) * 100));
  return h(
    'div.pmeter',
    null,
    h('div.pmeter-track', null, h('div.pmeter-fill', { class: tone, style: `width:${pct}%` })),
    h('span.pmeter-caption', null, caption),
  );
}

function tile(label: string, value: Node | string, sub?: string): HTMLElement {
  return h('div.ptile', null, h('span.ptile-label', null, label), h('div.ptile-value', null, value), sub ? h('span.ptile-sub', null, sub) : null);
}

/** The profile card. It fills in its record from `/v1/agents/{cid}/profile` and refreshes with `update`. */
export function profileCard(a: Agent): { el: HTMLElement; update: (a: Agent) => void; refresh: () => Promise<void> } {
  const pic = h('div.profile-portrait');
  const ident = h('div.profile-ident');
  const tiles = h('div.profile-tiles');
  const record = h('div.profile-record');
  const inf = h('section.prof-inf', { 'aria-label': 'Infractions' });
  const el = h('section.profile-card', null, pic, h('div.profile-main', null, ident, tiles, record, inf));
  let agent = a;
  let profile: AgentProfile | null = null;

  const render = (): void => {
    replace(pic, avatar(agent, 168), h('span.profile-stamp', null, agent.tier === 'orchestrator' ? 'ORCHESTRATOR' : 'CITIZEN'));
    const status = agent.status === 'working' ? badge('Working', 'info', true) : agent.status === 'idle' ? badge('Idle', 'ok', true) : badge('Offline', 'muted', true);
    replace(
      ident,
      h('div.profile-name', null, h('h1', null, agentName(agent)), status),
      h(
        'div.profile-tags',
        null,
        h('span.role-chip', { style: `--hue:${agent.tier === 'orchestrator' ? 25 : roleHue(agent.role)}` }, roleLabel(agent.role)),
        h('span.mono.faint', null, agent.model),
        h('span.faint', null, `agent #${agent.cid}`),
      ),
    );
    const p = profile;
    const conduct = p ? conductLabel(p.conduct) : null;
    replace(
      tiles,
      tile('Balance', p ? h('span.money', null, `${compact(p.wallet)} cr`) : '—', p ? `earned ${compact(p.earned)} · spent ${compact(p.spent)}` : undefined),
      tile(
        'Reputation',
        // With no evidence anywhere, the computed score means nothing yet.
        p && p.confidence >= 0.05 ? meter(p.reputation, 100, 'rep', `${Math.round(p.reputation)} / 100`) : p ? h('span.faint', null, 'Unrated') : '—',
        p ? (p.confidence >= 0.05 ? `${Math.round(p.confidence * 100)}% confident` : 'no rated work yet') : undefined,
      ),
      tile('Conduct', p && conduct ? meter(p.conduct, 100, conduct.tone, conduct.text) : '—', p ? `${p.violations.length} violations · ${p.denials} refusals` : undefined),
      tile('Tasks', p ? `${p.tasks.verified} verified` : '—', p ? `${p.tasks.done} done · ${p.tasks.failed} failed · ${p.tasks.posted} posted` : undefined),
      tile('Network', p ? `${p.contacts} contacts` : '—', p ? `${p.memory_notes} memories` : undefined),
      tile('Tokens', compact(agent.usage.input_tokens + agent.usage.output_tokens), p ? `joined ${ago(p.joined_ms)}` : undefined),
    );
    if (p && (p.violations.length || p.fines.count)) {
      replace(
        record,
        h('h3', null, 'Record'),
        h(
          'ul.profile-violations',
          null,
          p.violations.slice(0, 5).map((v) => h('li', null, badge(v.law, 'danger'), h('span', null, v.detail), h('span.faint', null, ago(v.at_ms)))),
          p.fines.count ? h('li', null, badge('fines', 'warn'), h('span', null, `${p.fines.count} fines, ${compact(p.fines.total)} cr in total`)) : null,
        ),
      );
    } else {
      replace(record, p ? h('p.faint.profile-clean', null, 'A clean record: no violations, no fines.') : null);
    }
  };

  const refresh = async (): Promise<void> => {
    try {
      profile = await api.profile(agent.cid);
    } catch {
      profile = null;
    }
    render();
  };

  // Infractions: loaded on creation, then refreshed by `update` at most once every 30 s.
  const INF_SHOWN = 10;
  let infractions: Infraction[] | null = null;
  let infFailed = false;
  let infAt = 0;
  let infAll = false;

  const infRow = (r: Infraction): HTMLElement =>
    h(
      'li.prof-inf-row',
      { title: r.statement },
      h(
        'div.prof-inf-main',
        null,
        h('a.prof-inf-law', { href: `#/rules/${encodeURIComponent(r.law)}` }, r.law_title || r.law),
        h('span.mono.faint.prof-inf-id', null, r.law),
      ),
      h('span.mono.prof-inf-tool', null, r.tool),
      h('span.prof-inf-detail', null, r.detail),
      h('span.faint.prof-inf-ago', null, ago(r.at_ms)),
    );

  const renderInfractions = (): void => {
    const list = infractions ? [...infractions].sort((x, y) => y.at_ms - x.at_ms) : null;
    const n = list ? list.length : 0;
    const headline =
      list === null
        ? h('span.faint', null, infFailed ? 'Infractions unavailable' : 'Loading…')
        : n === 0
          ? badge('No infractions', 'ok', true)
          : badge(`${n} infraction${n === 1 ? '' : 's'}`, 'danger', true);
    const shown = list ? (infAll ? list : list.slice(0, INF_SHOWN)) : [];
    replace(
      inf,
      h('h3', null, 'Infractions'),
      headline,
      shown.length ? h('ul.prof-inf-list', null, shown.map(infRow)) : null,
      n > INF_SHOWN
        ? h(
            'button.prof-inf-more',
            {
              type: 'button',
              onclick: () => {
                infAll = !infAll;
                renderInfractions();
              },
            },
            infAll ? 'Show fewer' : 'Show all',
          )
        : null,
    );
  };

  const loadInfractions = async (): Promise<void> => {
    const cid = agent.cid;
    infAt = Date.now();
    try {
      const list = await api.agentInfractions(cid);
      if (cid !== agent.cid) return;
      infractions = list;
      infFailed = false;
    } catch {
      if (cid !== agent.cid) return;
      infFailed = true;
    }
    renderInfractions();
  };

  render();
  renderInfractions();
  void refresh();
  void loadInfractions();
  return {
    el,
    update: (next) => {
      const switched = next.cid !== agent.cid;
      agent = next;
      if (switched) {
        infractions = null;
        infFailed = false;
        infAll = false;
        renderInfractions();
      }
      render();
      if (switched || Date.now() - infAt >= 30_000) void loadInfractions();
    },
    refresh,
  };
}
