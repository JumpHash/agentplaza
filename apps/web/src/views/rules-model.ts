// The rules screen's logic, free of the DOM: how laws are grouped and labelled, where a
// diagnostic points in a source, and the settings body that turns strict permits on or off.

import type { Diagnostic, LawInfo, Settings } from '../api';
import type { Tone } from '../ui/components';

export type TierKey = 'constitution' | 'statute' | 'tools' | 'operations' | 'other';

/** What each tier is called, as the desktop's tabs and group heads say it. */
export const TIER_LABELS: Record<TierKey, string> = {
  constitution: 'Constitution',
  statute: 'Statutes',
  tools: 'Tools',
  operations: 'Operations',
  other: 'Other',
};

/** The tiers in the order the desktop groups the built-in statutes. */
export const TIER_ORDER: TierKey[] = ['constitution', 'statute', 'tools', 'operations', 'other'];

/** The tab filters of the built-in statutes, as the desktop's tabs; Other is a group with no tab. */
export const TIER_FILTERS: { key: 'all' | TierKey; label: string }[] = [
  { key: 'all', label: 'All' },
  ...TIER_ORDER.filter((key) => key !== 'other').map((key) => ({ key, label: TIER_LABELS[key] })),
];

/** What each tier group holds, as the desktop's group heads say. */
export const TIER_DESCRIPTIONS: Record<TierKey, string> = {
  constitution: "The plaza's fundamental rules; they override everything else.",
  statute: 'Standing rules for roles and everyday work.',
  tools: 'Who may use which tool.',
  operations: 'The kinds of actions the laws talk about.',
  other: '',
};

export const STRICT_HELP = 'Strict permits: when on, an operation that has permit laws is refused unless one of those permits allows it. Off (the default): only forbids refuse, and a permit only allows.';

export const EXAMPLE_LAW = `law "O-001" "Agents do not delete posts" {
  tier statute
  enforcement preventive
  severity major
  statement "Agents do not delete posts on the board."
  forbid tool.call
  principal is Agent
  resource is Tool
  when resource.name == "delete_post"
}`;

export function cap(s: string): string {
  return s ? s.charAt(0).toUpperCase() + s.slice(1) : s;
}

/** The tier of a law, by the desktop's rule: the tier field, then the file it was written in; each is
 *  probed for "const", "statut", "tool" and "operat" in that order, and no match is other. */
export function tierKey(l: Pick<LawInfo, 'tier' | 'file'>): TierKey {
  for (const probe of [(l.tier || '').toLowerCase(), (l.file || '').toLowerCase()]) {
    if (probe.includes('const')) return 'constitution';
    if (probe.includes('statut')) return 'statute';
    if (probe.includes('tool')) return 'tools';
    if (probe.includes('operat')) return 'operations';
  }
  return 'other';
}

/** An owner law comes from an owner rule; a built-in one ships with the plaza. */
export function isOwnerLaw(l: Pick<LawInfo, 'origin' | 'rule_id'>): boolean {
  return l.origin === 'owner' || l.rule_id !== '';
}

/** The built-in statutes in one tier (or in every tier), in the order the server gave. */
export function builtinLaws(laws: LawInfo[], tier: 'all' | TierKey): LawInfo[] {
  return laws.filter((l) => !isOwnerLaw(l) && (tier === 'all' || tierKey(l) === tier));
}

export const SEVERITIES = ['info', 'minor', 'major', 'critical'];

/** The filters of the statutes list: a tier, an effect, a severity, and a search text. */
export interface LawFilter {
  q: string;
  effect: 'all' | 'permit' | 'forbid';
  sev: string;
  tier: 'all' | TierKey;
}

export function matchesFilter(l: LawInfo, f: LawFilter): boolean {
  if (f.tier !== 'all' && tierKey(l) !== f.tier) return false;
  if (f.effect !== 'all' && l.effect !== f.effect) return false;
  if (f.sev !== 'any' && l.severity !== f.sev) return false;
  const q = f.q.trim().toLowerCase();
  if (q && ![l.title, l.statement, l.id, ...l.ops].join(' ').toLowerCase().includes(q)) return false;
  return true;
}

/** The laws an owner rule declares. */
export function lawsOfRule(laws: LawInfo[], ruleId: string): LawInfo[] {
  return laws.filter((l) => isOwnerLaw(l) && l.rule_id === ruleId);
}

export function effectLabel(effect: string): string {
  return effect === 'forbid' ? 'Forbids' : 'Allows';
}

export function enforcementLabel(e: string): string {
  if (e === 'preventive') return 'Stops it before it happens';
  if (e === 'detective') return 'Recorded after the fact';
  return cap(e) || 'Enforcement not stated';
}

export function severityTone(s: string): Tone {
  switch (s) {
    case 'critical': return 'danger';
    case 'major': return 'warn';
    case 'minor': return 'info';
    default: return 'muted';
  }
}

/** Offsets of one 1-based line in `source`; a line past the end is the last one. */
export function lineSpan(source: string, line: number): { start: number; end: number } {
  const lines = source.split('\n');
  const n = Math.min(Math.max(line, 1), lines.length);
  const start = lines.slice(0, n - 1).reduce((sum, text) => sum + text.length + 1, 0);
  return { start, end: start + (lines[n - 1] ?? '').length };
}

/** The messages of the diagnostics, joined per line; the gutter marks these lines. */
export function diagnosticsByLine(diags: Diagnostic[]): Map<number, string> {
  const byLine = new Map<number, string>();
  for (const d of diags) {
    const prev = byLine.get(d.line);
    byLine.set(d.line, prev ? `${prev}\n${d.message}` : d.message);
  }
  return byLine;
}

export function diagnosticText(d: Diagnostic): string {
  return `Line ${d.line}, column ${d.column}: ${d.message}`;
}

/** A title or reason is required: blank (or only spaces) does not count. */
export function isBlank(text: string): boolean {
  return text.trim() === '';
}

/** The settings document with strict permits set; `dev_models` is apid's own field, so it is dropped before a save. */
export function withStrictPermits(settings: Settings, on: boolean): Settings {
  const next: Settings = { ...settings, laws: { ...settings.laws, strict_permits: on } };
  delete next.dev_models;
  return next;
}

export function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
