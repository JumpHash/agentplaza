// The Maintenance tab's logic, without the page: which proposals need the owner and in what order, how a proposal
// reads in a line, where each area of the map is drawn, and the words for a sweep. views/project-maintenance.ts draws
// with it; test/maintenance-tests.ts checks it.

import type { Area, Health, Maintenance, Proposal, ProposalKind, Risk, Signal, Size } from '../api-projects';

const RISK_ORDER: Record<Risk, number> = { low: 0, medium: 1, high: 2 };

/** Waiting for the owner: proposed, or put off for later. */
export const isWaiting = (p: Proposal): boolean => p.state === 'proposed' || p.state === 'later';

/**
 * The proposals waiting for the owner, best first: the most valuable, then the least risky, then the ones not put off,
 * then the newest.
 */
export function waitingFirst(list: Proposal[]): Proposal[] {
  return list
    .filter(isWaiting)
    .sort(
      (a, b) =>
        b.value.score - a.value.score ||
        RISK_ORDER[a.impact.risk] - RISK_ORDER[b.impact.risk] ||
        Number(a.state === 'later') - Number(b.state === 'later') ||
        b.created_ms - a.created_ms,
    );
}

/** The proposals the owner has answered (or that were replaced), newest decision first. */
export function decided(list: Proposal[]): Proposal[] {
  return list.filter((p) => !isWaiting(p)).sort((a, b) => b.decided_ms - a.decided_ms);
}

export const KIND_LABEL: Record<ProposalKind, string> = {
  fix: 'Fix',
  improve: 'Improve',
  feature: 'Feature',
  security: 'Security',
  performance: 'Performance',
  chore: 'Chore',
  instrument: 'More data',
};

export const SIZE_LABEL: Record<Size, string> = { s: 'small', m: 'medium', l: 'large', xl: 'very large' };

/** "Value 4/5 · low risk · small". */
export function weighLine(p: Proposal): string {
  return `Value ${p.value.score}/5 · ${p.impact.risk} risk · ${SIZE_LABEL[p.effort.size] ?? p.effort.size}`;
}

/** What became of a decided proposal, in words. */
export function outcomeLine(p: Proposal): string {
  switch (p.state) {
    case 'approved':
      if (!p.task) return p.instrument?.catalog ? `Picked: set up ${p.instrument.name} in Connectors or MCP` : 'Picked';
      return `Picked: task ${p.task}${p.task_state ? ` (${p.task_state.replace(/_/g, ' ')})` : ''}`;
    case 'declined':
      return p.note ? `Declined: ${p.note}` : 'Declined';
    case 'withdrawn':
      return p.note ? `Withdrawn: ${p.note}` : 'Withdrawn';
    default:
      return p.state;
  }
}

/** The headline of the tab: what waits for the owner. */
export function headline(m: Maintenance): string {
  const n = m.proposals.filter(isWaiting).length;
  if (n === 0) return m.sweep.runs === 0 && !m.sweep.running ? 'No sweep yet' : 'Nothing waits for you';
  return n === 1 ? 'One proposal waits for you' : `${n} proposals wait for you`;
}

const HOUR = 3600_000;

/** "every 24 h", "only when asked". */
export function everyText(hours: number): string {
  return hours === 0 ? 'only when you ask' : hours === 1 ? 'every hour' : `every ${hours} h`;
}

/** The sweep in a line: under way, or when it last ran and when the next is due. */
export function sweepLine(m: Maintenance, now: number, nameOf: (cid: number) => string): string {
  const s = m.sweep;
  if (s.running) return `${nameOf(s.running.cid)} is sweeping now`;
  if (s.runs === 0) return m.watchers.length ? 'The first sweep maps the project' : 'Add an agent to the project to watch it';
  const ago = Math.max(0, now - s.last_ms);
  const agoText = ago < HOUR ? `${Math.max(1, Math.round(ago / 60_000))} min ago` : `${Math.round(ago / HOUR)} h ago`;
  const every = m.config.every_hours * HOUR;
  let next = '';
  if (every > 0) {
    const left = s.last_ms + every - now;
    next = left <= 0 ? ' · the next is due' : ` · the next in ${left < HOUR ? `${Math.max(1, Math.round(left / 60_000))} min` : `${Math.round(left / HOUR)} h`}`;
  }
  return `Last sweep ${agoText} by ${nameOf(s.last_cid)}${next}`;
}

/** The health's tone for a dot: good, watch, poor or unknown. */
export const healthTone = (h: Health): 'ok' | 'warn' | 'danger' | 'muted' =>
  h === 'good' ? 'ok' : h === 'watch' ? 'warn' : h === 'poor' ? 'danger' : 'muted';

/** Where each area sits on a ring around the product, in a square of side `size`: evenly, starting at the top. */
export function ringLayout(areas: Area[], size: number): Map<string, { x: number; y: number }> {
  const c = size / 2;
  const r = areas.length <= 1 ? 0 : size * (areas.length > 10 ? 0.38 : 0.33);
  const out = new Map<string, { x: number; y: number }>();
  areas.forEach((a, i) => {
    const angle = -Math.PI / 2 + (2 * Math.PI * i) / Math.max(1, areas.length);
    out.set(a.id, { x: c + r * Math.cos(angle), y: c + r * Math.sin(angle) });
  });
  return out;
}

/** The links of the map, each once (a–b and b–a are one line). */
export function mapLinks(areas: Area[]): [string, string][] {
  const ids = new Set(areas.map((a) => a.id));
  const seen = new Set<string>();
  const out: [string, string][] = [];
  for (const a of areas) {
    for (const b of a.links) {
      if (!ids.has(b) || b === a.id) continue;
      const key = [a.id, b].sort().join('|');
      if (seen.has(key)) continue;
      seen.add(key);
      out.push([a.id, b]);
    }
  }
  return out;
}

/** The waiting proposals of each area (by the proposal's area, else the first of its impact's areas). */
export function proposalsByArea(list: Proposal[]): Map<string, Proposal[]> {
  const out = new Map<string, Proposal[]>();
  for (const p of waitingFirst(list)) {
    const area = p.area ?? p.impact.areas[0];
    if (!area) continue;
    out.set(area, [...(out.get(area) ?? []), p]);
  }
  return out;
}

/** A signal in one line: its title, else the start of its text. */
export function signalLine(s: Signal): string {
  const text = s.title.trim() || s.text.replace(/\s+/g, ' ').trim();
  return text.length > 120 ? `${text.slice(0, 119)}…` : text;
}
