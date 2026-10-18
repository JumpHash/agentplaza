// The Git platforms screen's pure rules: the top line, the labels of a level and of how an authorization was given,
// the authorizations grouped by host, the identity line, the grants body built from the checkboxes, a connect result
// and a flow's outcome in plain words. No DOM here.

import type { ConnectResult, FlowStatus, GrantsBody, NamespaceGrant, PlatformConnection, PlatformIntegration, PlatformKind, PlatformLevel, PlatformRegistration, PlatformTokenBody, PlatformVia } from '../api-platforms';
import { connectionLabel } from './history-model';

export const KIND_ORDER: PlatformKind[] = ['github', 'gitlab', 'forgejo'];
export const KIND_LABEL: Record<PlatformKind, string> = { github: 'GitHub', gitlab: 'GitLab', forgejo: 'Forgejo' };
/** The scopes a GitLab OAuth app needs, as the owner ticks them when registering it. */
export const GITLAB_SCOPES = 'api, read_user';

/** `1 authorization`, `2 repositories`. */
export function count(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** The top line: what the owner has, what is connected to a project, and what waits for them. `connected` is null
 * when the Weft repos could not be read, so the line says nothing about connections. */
export function stateLine(s: { authorizations: number; connected: number | null; needsYou: number }): string {
  if (s.authorizations === 0) return 'Nothing authorized yet: add GitHub, GitLab or Forgejo.';
  const parts = [`${count(s.authorizations, 'authorization')}.`];
  if (s.connected !== null) parts.push(s.connected === 0 ? 'No repository connected.' : `${count(s.connected, 'repository', 'repositories')} connected.`);
  if (s.needsYou > 0) parts.push(`${count(s.needsYou, 'authorization')} ${s.needsYou === 1 ? 'needs' : 'need'} you.`);
  return parts.join(' ');
}

/** A level in the owner's words. GitLab's organisation is a group. */
export function levelLabel(level: PlatformLevel, kind: PlatformKind): string {
  if (level === 'user') return 'Your account';
  if (level === 'repo') return 'One repository';
  return kind === 'gitlab' ? 'Group' : 'Organisation';
}

/** A namespace's kind in the owner's words: the account, an organisation, or a GitLab group. */
export function namespaceLabel(kind: string): string {
  if (kind === 'user') return 'Your account';
  return kind === 'group' ? 'Group' : 'Organisation';
}

/** How an authorization was given. */
export function viaLabel(via: PlatformVia): string {
  if (via === 'app') return 'GitHub App';
  return via === 'oauth' ? 'Signed in' : 'Token';
}

/** A host without its scheme and trailing slash: `github.com`, `gitlab.example.org/tools`. */
export function hostName(host: string): string {
  return host.trim().replace(/^https?:\/\//i, '').replace(/\/+$/, '');
}

/** A host as the screen compares it: no case, no trailing slash. */
export function hostKey(host: string): string {
  return host.trim().replace(/\/+$/, '').toLowerCase();
}

export function sameHost(a: string, b: string): boolean {
  return hostKey(a) === hostKey(b);
}

/** The registration a card acts on for its host: the plaza's GitHub App on GitHub, an OAuth app on GitLab or Forgejo. */
export function registrationFor(regs: PlatformRegistration[], kind: PlatformKind, host: string): PlatformRegistration | undefined {
  return regs.find((r) => r.kind === kind && sameHost(r.host, host) && (kind === 'github' ? r.app !== null : r.app === null));
}

export interface HostGroup { host: string; kind: PlatformKind; label: string; items: PlatformIntegration[] }

/** The authorizations of each host, the hosts in the order GitHub, GitLab, Forgejo. */
export function groupByHost(items: PlatformIntegration[]): HostGroup[] {
  const groups = new Map<string, HostGroup>();
  for (const item of items) {
    const key = hostKey(item.host);
    let group = groups.get(key);
    if (!group) {
      group = { host: item.host, kind: item.kind, label: KIND_LABEL[item.kind], items: [] };
      groups.set(key, group);
    }
    group.items.push(item);
  }
  return [...groups.values()].sort((a, b) => KIND_ORDER.indexOf(a.kind) - KIND_ORDER.indexOf(b.kind) || hostKey(a.host).localeCompare(hostKey(b.host)));
}

/** Whom exports are authored by, or the warning that the owner has no verified email on that host. */
export function identityLine(identity: { name: string; login: string; email: string }, host: string): { text: string; warn: boolean } {
  if (!identity.email) return { text: `No verified email: sign in on ${hostName(host)}`, warn: true };
  return { text: `Exports are by ${identity.name || identity.login} <${identity.email}>`, warn: false };
}

/** A GitHub installation's choice on the platform, in words; null for any other authorization. */
export function repositorySelectionLabel(selection: string | null): string | null {
  if (selection === 'all') return 'All repositories';
  return selection === 'selected' ? 'Chosen repositories' : null;
}

/** An authorization's connection to a project, in one line: `→ pr1-notes · In step`. */
export function connectionText(c: PlatformConnection): string {
  return `→ ${c.weft_repo} · ${connectionLabel(c.state)}`;
}

/** The repositories connected to a platform, across the Weft repos: one connection each. */
export function connectedCount(repos: { connections: { platform: string }[] }[]): number {
  const kinds = new Set<string>(KIND_ORDER);
  return repos.reduce((n, r) => n + r.connections.filter((c) => kinds.has(c.platform)).length, 0);
}

// ---- Grants ------------------------------------------------------------------

/** One namespace's checkboxes: every repository of it, or the repositories named. */
export interface GrantDraft { all: boolean; repos: string[] }
export type GrantState = Record<string, GrantDraft>;

/** The checkboxes as the authorization holds them now (the start of an edit). */
export function grantState(grants: { namespaces: Record<string, NamespaceGrant> }): GrantState {
  const out: GrantState = {};
  for (const [name, g] of Object.entries(grants.namespaces)) out[name] = { all: g.all, repos: [...g.repos] };
  return out;
}

/** Whether anything at all is granted. */
export function hasGrants(grants: { namespaces: Record<string, NamespaceGrant> }): boolean {
  return Object.values(grants.namespaces).some((g) => g.all || g.repos.length > 0);
}

/** Ticks or unticks a namespace's "All repositories". The named repositories are kept, so unticking returns them. */
export function setAll(state: GrantState, namespace: string, on: boolean): void {
  const draft = (state[namespace] ??= { all: false, repos: [] });
  draft.all = on;
}

/** Ticks or unticks one repository of a namespace. */
export function setRepo(state: GrantState, namespace: string, repo: string, on: boolean): void {
  const draft = (state[namespace] ??= { all: false, repos: [] });
  draft.repos = on ? [...new Set([...draft.repos, repo])] : draft.repos.filter((r) => r !== repo);
}

/** The body of `PUT grants`: the whole map. A namespace with nothing ticked is left out (nothing of it is granted);
 * a namespace with "All repositories" sends no repository names. */
export function grantsBody(state: GrantState): GrantsBody {
  const namespaces: Record<string, NamespaceGrant> = {};
  for (const [name, draft] of Object.entries(state)) {
    if (draft.all) namespaces[name] = { all: true, repos: [] };
    else if (draft.repos.length > 0) namespaces[name] = { all: false, repos: [...new Set(draft.repos)].sort() };
  }
  return { namespaces };
}

// ---- Forms -------------------------------------------------------------------

/** The placeholder of the target field: a group path for GitLab, an organisation, or a repository. */
export function targetPlaceholder(level: PlatformLevel, kind: PlatformKind): string {
  if (level === 'repo') return 'namespace/repo';
  return kind === 'gitlab' ? 'group/path' : 'organisation';
}

/** The problem with a token form, or null when it can be sent. */
export function tokenProblem(token: string, level: PlatformLevel, target: string): string | null {
  if (!token.trim()) return 'Paste the token first.';
  if (level !== 'user' && !target.trim()) return `Name the ${level === 'repo' ? 'repository (namespace/repo)' : 'organisation or group'} it is for.`;
  return null;
}

/** The body of `POST tokens`: the target only for an organisation or a repository. */
export function tokenBody(f: { kind: PlatformKind; host: string; token: string; level: PlatformLevel; target: string }): PlatformTokenBody {
  const body: PlatformTokenBody = { kind: f.kind, host: f.host.trim(), token: f.token.trim(), level: f.level };
  if (f.level !== 'user') body.target = f.target.trim();
  return body;
}

/** The problem with an OAuth app form, or null when it can be sent. */
export function registrationProblem(clientId: string, clientSecret: string): string | null {
  return clientId.trim() && clientSecret.trim() ? null : 'Enter the client ID and the client secret.';
}

// ---- Outcomes ----------------------------------------------------------------

/** A flow's result in words, or null while it is still pending. */
export function flowOutcome(status: FlowStatus, label: string): { text: string; error: boolean } | null {
  switch (status.status) {
    case 'done': {
      const made = status.detail?.integrations?.length ?? 0;
      return { text: made > 0 ? `Authorized on ${label}.` : `${label} is set up.`, error: false };
    }
    case 'failed':
      return { text: status.error || `The ${label} step failed.`, error: true };
    case 'expired':
      return { text: 'It took longer than 15 minutes. Start again.', error: true };
    default:
      return null;
  }
}

/** What the first import brought, in words. */
function importText(imp: NonNullable<ConnectResult['imported']>): string {
  if (imp.empty) return "The repository is empty: the first export pushes Weft's history.";
  const changes = imp.commits ?? 0;
  const releases = imp.releases?.length ?? 0;
  if (changes === 0 && releases === 0) return 'Nothing new to import.';
  const parts: string[] = [];
  if (changes > 0) parts.push(count(changes, 'change'));
  if (releases > 0) parts.push(count(releases, 'release'));
  return `Imported ${parts.join(' and ')}.`;
}

/** A connect answer in plain words; `warn` when part of it failed (the import or the signing key). */
export function connectResultText(r: ConnectResult): { text: string; warn: boolean } {
  const parts = [`Connected to ${r.weft_repo}.`];
  let warn = false;
  if (r.imported?.error) {
    parts.push(`The import failed: ${r.imported.error}`);
    warn = true;
  } else if (r.imported) {
    parts.push(importText(r.imported));
  }
  if (r.signing_key && typeof r.signing_key === 'object') {
    parts.push(`The signing key could not be registered: ${r.signing_key.error}`);
    warn = true;
  }
  return { text: parts.join(' '), warn };
}

/** What an authorization is for: the owner's account, or the organisation, group or repository it is scoped to. */
export function accountText(i: Pick<PlatformIntegration, 'level' | 'target' | 'account'>): string {
  if (i.level === 'user') return `@${i.account.login || i.account.name}`;
  return i.target || i.account.login || '—';
}
