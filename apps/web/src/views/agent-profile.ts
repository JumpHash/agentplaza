// An agent's profile: its portrait (its own robot, posed and photographed in the 3D studio) on a
// cartoon ID card, with its role, money, standing with the others, conduct under the laws, work
// record, contacts and memories.

import '../styles/profile.css';
import { api, type Agent, type AgentProfile } from '../api';
import { agentName, roleLabel } from '../roles';
import { ago, badge, compact } from '../ui/components';
import { h, replace } from '../ui/dom';
import { agentHue, roleHue } from './floor';
import { portrait } from './floor3d/portrait';

/** The agent's portrait as an element: the photographed robot, or its initial on its colour. */
export function avatar(a: Agent, size = 40): HTMLElement {
  const hue = agentHue(a.cid);
  const url = portrait({
    cid: a.cid,
    roleHue: a.tier === 'orchestrator' ? 25 : roleHue(a.role),
    bodyHue: hue,
    orchestrator: a.tier === 'orchestrator',
    state: a.status === 'offline' ? 'offline' : 'idle',
  });
  const frame = h('span.avatar', { style: `width:${size}px;height:${size}px;--hue:${hue}` });
  if (url) frame.append(h('img', { src: url, alt: '', width: String(size), height: String(size) }));
  else frame.append(h('span.avatar-initial', null, agentName(a).slice(0, 1).toUpperCase()));
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
  const el = h('section.profile-card', null, pic, h('div.profile-main', null, ident, tiles, record));
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

  render();
  void refresh();
  return {
    el,
    update: (next) => {
      agent = next;
      render();
    },
    refresh,
  };
}
