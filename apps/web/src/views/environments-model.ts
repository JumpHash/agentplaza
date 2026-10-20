// The Environments tab's pure rules: the order of the environments, what their track and their deploys say, the
// NAME=value lines of their variables, the names they and their secrets may take, the defaults a new one starts with,
// and the line that says what is deployed there now. No DOM here; project-environments.ts draws them.

import type { Deploy, DeployStatus, EnvKind, Environment, EnvTrack } from '../api-projects';
import { ago } from '../ui/components';

const KIND_ORDER: EnvKind[] = ['production', 'staging', 'preview', 'development', 'other'];

const KIND_LABELS: Record<EnvKind, string> = {
  production: 'Production',
  staging: 'Staging',
  preview: 'Preview',
  development: 'Development',
  other: 'Other',
};

export function kindLabel(kind: EnvKind): string {
  return KIND_LABELS[kind] ?? 'Other';
}

function kindRank(kind: EnvKind): number {
  const i = KIND_ORDER.indexOf(kind);
  return i < 0 ? KIND_ORDER.length : i;
}

/** Production first, then staging, preview, development and other; by name within a kind. A new list. */
export function sortEnvironments(list: Environment[]): Environment[] {
  return [...list].sort((a, b) => kindRank(a.kind) - kindRank(b.kind) || a.name.localeCompare(b.name));
}

const TRACK_TEXT: Record<EnvTrack, string> = {
  main: 'every landing on main',
  releases: 'each signed release',
  manual: 'what you deploy by hand',
};

/** What an environment runs, as the line after "Runs: ". */
export function trackText(track: EnvTrack): string {
  return TRACK_TEXT[track] ?? TRACK_TEXT.manual;
}

/** The choices of the "Runs" field, in the words the form uses. */
export const TRACK_CHOICES: { value: EnvTrack; label: string }[] = [
  { value: 'main', label: 'Every landing on main' },
  { value: 'releases', label: 'Each signed release' },
  { value: 'manual', label: 'What I deploy by hand' },
];

const STATUS_TEXT: Record<DeployStatus, string> = {
  recorded: 'Recorded',
  sending: 'Sending',
  sent: 'Sent',
  failed: 'Failed',
  waiting: 'Waiting',
  refused: 'Refused',
};

export function deployStatusText(status: DeployStatus): string {
  return STATUS_TEXT[status] ?? status;
}

/** The badge tone of a deploy: done is ok, still going or waiting is warn, failed or refused is danger. */
export function deployTone(status: DeployStatus): 'ok' | 'warn' | 'danger' {
  switch (status) {
    case 'recorded':
    case 'sent':
      return 'ok';
    case 'failed':
    case 'refused':
      return 'danger';
    default:
      return 'warn';
  }
}

/** What a deploy that has just been asked for says, and whether it is a failure to show as one. */
export function deployOutcome(env: string, status: DeployStatus): { text: string; error: boolean } {
  switch (status) {
    case 'sent':
      return { text: `Sent to ${env}.`, error: false };
    case 'recorded':
      return { text: `Recorded for ${env}.`, error: false };
    case 'sending':
      return { text: `Sending to ${env}.`, error: false };
    case 'waiting':
      return { text: `Waiting for an approval before it goes to ${env}.`, error: false };
    case 'failed':
      return { text: `The deploy to ${env} failed.`, error: true };
    case 'refused':
      return { text: `The deploy to ${env} was refused.`, error: true };
    default:
      return { text: `Deploy to ${env}: ${status}.`, error: false };
  }
}

/** The last `n` deploys, newest first. The hub keeps them oldest first. */
export function recentDeploys(deploys: Deploy[], n = 5): Deploy[] {
  if (n <= 0) return [];
  return deploys.slice(-n).reverse();
}

/** The deploy-hook line of a card. */
export function hookText(hook: boolean): string {
  return hook ? 'Deploy hook: set' : 'No deploy hook: deploys are recorded only';
}

/** What is deployed there now: its version and when it went, else that nothing is. */
export function nowText(env: Pick<Environment, 'version' | 'deployed_ms'>, now: number): string {
  return env.version ? `Now: ${env.version} · ${ago(env.deployed_ms, now)}` : 'Nothing deployed yet';
}

const VAR_NAME = /^[A-Za-z][A-Za-z0-9_]*$/;
/** The rule for a variable's or a secret's name. */
export const VAR_RULE = 'Letters, digits and _, starting with a letter.';

export function validVarName(name: string): boolean {
  return VAR_NAME.test(name);
}

const ENV_NAME = /^[a-z][a-z0-9-]{0,39}$/;
/** The rule for an environment's name. */
export const NAME_RULE = 'Lower-case letters, digits and dashes, starting with a letter. 40 characters at most.';

export function validName(name: string): boolean {
  return ENV_NAME.test(name);
}

/** True for an absolute http or https address. */
export function webUrl(url: string | undefined | null): boolean {
  if (!url) return false;
  try {
    const u = new URL(url);
    return u.protocol === 'http:' || u.protocol === 'https:';
  } catch {
    return false;
  }
}

/** A form's address: empty (none), or a web address. */
export function validUrl(url: string): boolean {
  return url === '' || webUrl(url);
}

/** The variables as NAME=value lines, by name. */
export function variablesText(vars: Record<string, string>): string {
  return Object.keys(vars)
    .sort()
    .map((k) => `${k}=${vars[k] ?? ''}`)
    .join('\n');
}

/** The variables as [name, value] pairs, by name. */
export function variableRows(vars: Record<string, string>): [string, string][] {
  return Object.keys(vars)
    .sort()
    .map((k): [string, string] => [k, vars[k] ?? '']);
}

/**
 * Reads NAME=value lines. Blank lines are skipped; the value is what follows the first `=`, trimmed. On a bad line the
 * result has no variables and `error` says which line and why.
 */
export function parseVariables(text: string): { vars: Record<string, string>; error: string } {
  const vars: Record<string, string> = {};
  const lines = text.split('\n');
  for (let i = 0; i < lines.length; i++) {
    const line = (lines[i] ?? '').trim();
    if (!line) continue;
    const at = line.indexOf('=');
    if (at < 0) return { vars: {}, error: `Line ${i + 1} is not NAME=value.` };
    const name = line.slice(0, at).trim();
    if (!name) return { vars: {}, error: `Line ${i + 1} has no name before the =.` };
    if (!validVarName(name)) return { vars: {}, error: `Line ${i + 1}: "${name}" is not a name. ${VAR_RULE}` };
    if (Object.prototype.hasOwnProperty.call(vars, name)) return { vars: {}, error: `Line ${i + 1}: ${name} is given twice.` };
    vars[name] = line.slice(at + 1).trim();
  }
  return { vars, error: '' };
}

/** The defaults a new environment of `kind` starts with: production releases, staging and preview follow main, others are by hand. */
export function defaultTrack(kind: EnvKind): EnvTrack {
  if (kind === 'production') return 'releases';
  if (kind === 'staging' || kind === 'preview') return 'main';
  return 'manual';
}

/** Production is protected by default. */
export function defaultProtected(kind: EnvKind): boolean {
  return kind === 'production';
}
