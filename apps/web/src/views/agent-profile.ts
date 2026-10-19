// An agent's profile: its robot portrait, its role, model and number, its money, standing with the others,
// conduct under the laws, work record and infractions. The portrait is the prototype's robot, tinted by the
// role's hue; where the engine has drawn the agent's own portrait, that picture takes its place.

import { api, type Agent, type AgentProfile, type Infraction } from '../api';
import { agentName, roleLabel } from '../roles';
import { ago, badge, button, compact } from '../ui/components';
import { h, replace } from '../ui/dom';
import { agentHue } from './floor';
import { cachedPortrait, portraitKey, portraitSpecJson, rememberPortrait, type PortraitSpec } from './portrait-spec';
import { engineWanted, startThumbEngine, type EngineStage } from './engine-mount';

/** The page's portrait engine: one per page, made when a portrait waits and destroyed a second after none waits. */
let portraitEngine: EngineStage | null = null;
let portraitWaiting = 0;
let portraitIdle = 0;
let portraitWarned = false;
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
    .catch((e: unknown) => {
      // The robot stays; say why once per page, so a broken portrait path is not silent.
      if (!portraitWarned) {
        portraitWarned = true;
        console.warn('agent portrait: the engine did not draw it', e);
      }
      return null;
    })
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

const esc = (s: string): string =>
  s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;');

/** A role's hue: the prototype's hash of the role id, so one role keeps one colour. */
function roleHueOf(role: string): number {
  let hue = 0;
  for (const ch of String(role)) hue = (hue * 31 + ch.charCodeAt(0)) % 360;
  return hue;
}

/** The prototype's robot as an SVG string: a tinted head on a stalk, a dark visor, two eyes and a base; the orchestrator gets an O stamp. */
function robotSvg(a: Agent, size: number): string {
  const hue = roleHueOf(a.role);
  const c = `hsl(${hue} 55% 48%)`;
  const soft = `hsl(${hue} 60% 50% / .16)`;
  const off = a.status === 'offline';
  const stamp =
    a.tier === 'orchestrator'
      ? '<circle cx="34" cy="6" r="5" fill="var(--signal)" stroke="var(--panel)" stroke-width="1.5"/><text x="34" y="8.4" text-anchor="middle" font-size="6.5" font-weight="700" fill="var(--on-action)">O</text>'
      : '';
  return `<svg class="robot" width="${size}" height="${size}" viewBox="0 0 40 40" role="img" aria-label="${esc(agentName(a))} portrait" style="flex:none;opacity:${off ? 0.38 : 1};filter:${off ? 'grayscale(1)' : 'none'}"><line x1="20" y1="8" x2="20" y2="3" stroke="${c}" stroke-width="1.6"/><circle cx="20" cy="2.5" r="2" fill="${c}"/><rect x="7" y="8" width="26" height="24" rx="8" fill="${soft}" stroke="${c}" stroke-width="1.6"/><rect x="11" y="14" width="18" height="9" rx="4.5" fill="var(--action)"/><circle cx="16" cy="18.5" r="2" fill="${c}"/><circle cx="24" cy="18.5" r="2" fill="${c}"/><rect x="13" y="34" width="14" height="4" rx="2" fill="${c}"/>${stamp}</svg>`;
}

/** The agent's portrait as an element: the robot, or the engine's picture of the agent once it has drawn it. */
export function avatar(a: Agent, size = 40): HTMLElement {
  const hue = agentHue(a.cid);
  const spec: PortraitSpec = {
    species: a.species,
    cid: a.cid,
    roleHue: a.tier === 'orchestrator' ? 25 : roleHueOf(a.role),
    bodyHue: hue,
    orchestrator: a.tier === 'orchestrator',
    state: a.status === 'offline' ? 'offline' : 'idle',
  };
  const frame = h('span.ag-portrait', { style: `display:inline-flex;flex:none;line-height:0;width:${size}px;height:${size}px` });
  const picture = (url: string): HTMLElement => h('img', { src: url, alt: '', width: String(size), height: String(size) });
  frame.innerHTML = robotSvg(a, size);
  // The engine draws the agent's own portrait; the robot shows until its picture comes back (and stays without an engine).
  const hit = cachedPortrait(portraitKey(spec));
  if (hit) frame.replaceChildren(picture(hit));
  else if (engineWanted()) {
    void enginePortrait(spec).then((url) => {
      if (url) frame.replaceChildren(picture(url));
    });
  }
  return frame;
}

/** Conduct as a word: how law-abiding the agent has been lately. */
export function conductLabel(score: number): { text: string; tone: 'ok' | 'info' | 'warn' | 'danger' } {
  if (score >= 90) return { text: 'Exemplary', tone: 'ok' };
  if (score >= 70) return { text: 'Good standing', tone: 'info' };
  if (score >= 40) return { text: 'Under watch', tone: 'warn' };
  return { text: 'In trouble', tone: 'danger' };
}

/** A meter: the value as a share of max, in the prototype's .ag-meter track. */
function meter(value: number, max: number): HTMLElement {
  const pct = Math.max(0, Math.min(100, (value / max) * 100));
  return h('div.ag-meter', null, h('i', { style: `width:${pct}%` }));
}

function tile(label: string, value: Node | string, sub?: string): HTMLElement {
  return h('div.ag-tile', null, h('div.muted', null, label), h('div.num', null, value), sub ? h('div.ag-row-sub', null, sub) : null);
}

/** The profile card. It fills in its record from `/v1/agents/{cid}/profile` and refreshes with `update`. */
export function profileCard(a: Agent): { el: HTMLElement; update: (a: Agent) => void; refresh: () => Promise<void> } {
  const top = h('div.ag-prof-top');
  const tiles = h('div.ag-tiles');
  const record = h('div');
  const inf = h('section', { 'aria-label': 'Infractions' });
  const el = h('section.card', null, h('div.card-b', null, h('div.ag-prof', null, top, tiles, record, inf)));
  let agent = a;
  let profile: AgentProfile | null = null;

  const render = (): void => {
    const orch = agent.tier === 'orchestrator';
    replace(
      top,
      h('div', null, avatar(agent, 56)),
      h(
        'div',
        { style: 'min-width:0' },
        h('div.ag-prof-name', null, h('span.muted.ag-tier', null, orch ? 'ORCHESTRATOR' : 'CITIZEN')),
        h(
          'div.ag-prof-line',
          null,
          badge(roleLabel(agent.role), 'accent'),
          h('span.mono', null, agent.model),
          h('span.muted', null, `agent #${agent.cid}`),
        ),
      ),
    );
    const p = profile;
    const conduct = p ? conductLabel(p.conduct) : null;
    const tok = compact(agent.usage.input_tokens + agent.usage.output_tokens);
    replace(
      tiles,
      tile('Balance', p ? h('span', null, `${compact(p.wallet)} `, h('span.muted', null, 'cr')) : '—', p ? `earned ${compact(p.earned)} · spent ${compact(p.spent)}` : undefined),
      p && p.confidence >= 0.05
        ? tile('Reputation', h('span', null, meter(p.reputation, 100), `${Math.round(p.reputation)} / 100`), `${Math.round(p.confidence * 100)}% confident`)
        : tile('Reputation', h('span.muted', null, p ? 'Unrated' : '—'), p ? 'no rated work yet' : undefined),
      p && conduct
        ? tile('Conduct', h('span', null, meter(p.conduct, 100), conduct.text), `${p.violations.length} violations · ${p.denials} refusals`)
        : tile('Conduct', '—'),
      tile('Tasks', p ? `${p.tasks.verified} verified` : '—', p ? `${p.tasks.done} done · ${p.tasks.failed} failed · ${p.tasks.posted} posted` : undefined),
      tile('Network', p ? `${p.contacts} contacts` : '—', p ? `${p.memory_notes} memories` : undefined),
      tile('Tokens', tok, p ? `joined ${ago(p.joined_ms)}` : undefined),
    );
    if (p && (p.violations.length || p.fines.count)) {
      replace(
        record,
        p.violations
          .slice(0, 5)
          .map((v) => h('div.ag-row', null, h('div', null, badge(v.law, 'danger'), ' ', v.detail), h('span.muted', null, ago(v.at_ms)))),
        p.fines.count
          ? h('div.ag-row', null, h('div', null, badge('fines', 'warn'), ` ${p.fines.count} fines, ${compact(p.fines.total)} cr in total`))
          : null,
      );
    } else {
      replace(record, p ? h('p.muted', null, 'A clean record: no violations, no fines.') : null);
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
      'div.ag-row',
      { title: r.statement },
      h(
        'div',
        { style: 'min-width:0' },
        h('a', { href: `#/rules/${encodeURIComponent(r.law)}` }, r.law_title || r.law),
        h('div.ag-row-sub', null, r.law),
        h('div', null, h('code', null, r.tool), ' ', r.detail),
      ),
      h('span.muted', null, ago(r.at_ms)),
    );

  const renderInfractions = (): void => {
    const list = infractions ? [...infractions].sort((x, y) => y.at_ms - x.at_ms) : null;
    const n = list ? list.length : 0;
    const headline =
      list === null
        ? h('span.muted', null, infFailed ? 'Infractions unavailable' : 'Loading…')
        : n === 0
          ? badge('No infractions', 'ok')
          : badge(n === 1 ? '1 infraction' : `${n} infractions`, 'danger');
    const shown = list ? (infAll ? list : list.slice(0, INF_SHOWN)) : [];
    replace(
      inf,
      h('div.cluster', null, headline),
      shown.map(infRow),
      n > INF_SHOWN
        ? button(
            infAll ? 'Show fewer' : 'Show all',
            () => {
              infAll = !infAll;
              renderInfractions();
            },
            { kind: 'ghost', size: 'sm' },
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
