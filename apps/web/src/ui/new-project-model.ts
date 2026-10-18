// The new-project dialog's rules, without the DOM: what the bring-in form sends (adoptInput), the line an adoption
// reports (importLine), the line about picked files (filesLine), and the authorizations and repositories the form offers.
// The hub's answers are typed in api-projects.ts and api-platforms.ts.

import { ENV_KINDS, type Adopted, type AdoptInput, type AdoptSource, type EnvKind, type EnvironmentInput, type ProjectMode } from '../api-projects';
import type { PlatformIntegration, PlatformRepo } from '../api-platforms';
import { KIND_LABEL, accountText } from '../views/platforms-model';
import { keepFiles, sizeText, type PickedFile } from './file-drop';

/** Where a project that exists is: a repository on the owner's git platform, a git address, a folder, or files. */
export type WhereKind = 'platform' | 'git' | 'path' | 'files';
export const WHERE_KINDS: { value: WhereKind; label: string }[] = [
  { value: 'platform', label: 'A repository on GitHub, GitLab or Forgejo' },
  { value: 'git', label: 'A git address anyone can read' },
  { value: 'path', label: "A folder on the hub's machine" },
  { value: 'files', label: 'Files from this computer' },
];

export const MAX_ENVIRONMENTS = 3;
const ENV_NAME = /^[a-z][a-z0-9-]{0,39}$/;

/** One environment row as the form holds it. A row with no name and no address is ignored. */
export interface EnvRowForm { name: string; kind: string; url: string }

/** The bring-in form as the owner left it. Only the fields of the chosen `where` are read. */
export interface BringForm {
  title: string;
  goal: string;
  where: WhereKind;
  /** The authorization's id (platform), and the repository's `owner/name` (platform). */
  platform: string;
  repo: string;
  sign: boolean;
  /** An https address (git), an absolute folder on the hub (path). */
  git: string;
  folder: string;
  branch: string;
  mode: ProjectMode;
  finished: boolean;
  environments: EnvRowForm[];
}

/** True when `text` is an absolute http or https address. */
function isHttpUrl(text: string): boolean {
  try {
    const url = new URL(text);
    return (url.protocol === 'https:' || url.protocol === 'http:') && url.host !== '';
  } catch {
    return false;
  }
}

function sourceOf(form: BringForm): { source?: AdoptSource; error?: string } {
  const branch = form.branch.trim() || 'main';
  if (form.where === 'platform') {
    const repo = form.repo.trim();
    if (!form.platform) return { error: 'Pick the git platform account the repository is on' };
    if (!repo) return { error: 'Pick a repository' };
    return { source: form.sign ? { platform: form.platform, repo, sign: true } : { platform: form.platform, repo } };
  }
  if (form.where === 'git') {
    const url = form.git.trim();
    if (!isHttpUrl(url)) return { error: 'The git address starts with https://' };
    return { source: { git: url, branch } };
  }
  if (form.where === 'path') {
    const folder = form.folder.trim();
    if (!folder.startsWith('/')) return { error: 'The folder is an absolute path on the hub, so it starts with /' };
    return { source: { path: folder, branch } };
  }
  // Files come after the adoption, by upload: there is no source yet.
  return {};
}

function environmentsOf(rows: EnvRowForm[]): { environments: ({ name: string } & EnvironmentInput)[]; error?: string } {
  const used = rows.map((r) => ({ name: r.name.trim(), kind: r.kind, url: r.url.trim() })).filter((r) => r.name || r.url);
  if (used.length > MAX_ENVIRONMENTS) return { environments: [], error: `A project has up to ${MAX_ENVIRONMENTS} environments here` };
  const out: ({ name: string } & EnvironmentInput)[] = [];
  for (const r of used) {
    if (!ENV_NAME.test(r.name)) {
      return { environments: [], error: 'An environment name is lower-case letters, digits and dashes, and starts with a letter' };
    }
    if (out.some((e) => e.name === r.name)) return { environments: [], error: `Two environments are called ${r.name}` };
    if (!(ENV_KINDS as string[]).includes(r.kind)) return { environments: [], error: `Pick a kind for ${r.name}` };
    if (r.url && !isHttpUrl(r.url)) return { environments: [], error: `${r.name} needs an https address` };
    out.push({ name: r.name, kind: r.kind as EnvKind, ...(r.url ? { url: r.url } : {}) });
  }
  return { environments: out };
}

/** The adoption the form asks for (`POST /v1/projects/adopt`), or the first thing that is wrong with it. */
export function adoptInput(form: BringForm): { input?: AdoptInput; error?: string } {
  const title = form.title.trim();
  if (!title) return { error: 'Give the project a title' };
  const source = sourceOf(form);
  if (source.error) return { error: source.error };
  const envs = environmentsOf(form.environments);
  if (envs.error) return { error: envs.error };
  const input: AdoptInput = { title, kind: 'code', mode: form.mode, finished: form.finished };
  const goal = form.goal.trim();
  if (goal) input.goal = goal;
  if (source.source) input.source = source.source;
  if (envs.environments.length) input.environments = envs.environments;
  return { input };
}

function importedOf(answer: Record<string, unknown> | null): Record<string, unknown> | undefined {
  const imported = answer?.imported;
  return imported && typeof imported === 'object' ? (imported as Record<string, unknown>) : undefined;
}

/** The changes an adoption imported: the import's own count, or the platform connection's. 0 when there are none. */
function commitsOf(answer: Record<string, unknown> | null): number {
  const direct = answer?.commits;
  if (typeof direct === 'number') return direct;
  const nested = importedOf(answer)?.commits;
  return typeof nested === 'number' ? nested : 0;
}

/**
 * What an adoption reports, as one line: "Imported 12 changes", then what did not work, joined by full stops. Empty when
 * there is nothing to say.
 */
export function importLine(adopted: Pick<Adopted, 'import' | 'problems'>): string {
  const answer = adopted.import;
  const parts: string[] = [];
  const commits = commitsOf(answer);
  if (commits > 0) parts.push(`Imported ${commits} ${commits === 1 ? 'change' : 'changes'}`);
  else if (importedOf(answer)?.empty === true) parts.push('The repository is empty');
  const problems = [...adopted.problems];
  const failed = importedOf(answer)?.error;
  if (typeof failed === 'string') problems.push(failed);
  for (const problem of problems) {
    const text = problem.trim().replace(/\.+$/, '');
    if (text && !parts.includes(text)) parts.push(text);
  }
  return parts.join('. ');
}

/** The line under a files box: how many files go in, how big they are, and what is left out (.git, node_modules). */
export function filesLine(files: PickedFile[]): string {
  if (files.length === 0) return '';
  const { kept, skipped } = keepFiles(files);
  const left = skipped ? `${skipped} left out (.git, node_modules)` : '';
  if (kept.length === 0) return ['No files to send', left].filter(Boolean).join(' · ');
  const bytes = kept.reduce((sum, f) => sum + f.file.size, 0);
  const head = `${kept.length} ${kept.length === 1 ? 'file' : 'files'}, ${sizeText(bytes)}`;
  return [head, left].filter(Boolean).join(' · ');
}

/** An authorization as the form lists it: its kind, and the account or target it covers. */
export function authorizationLabel(i: Pick<PlatformIntegration, 'kind' | 'level' | 'target' | 'account' | 'error'>): string {
  const text = `${KIND_LABEL[i.kind]} · ${accountText(i)}`;
  return i.error ? `${text} · needs you` : text;
}

/** The repositories the form offers: the granted ones only, by name, each once, in order. */
export function grantedRepos(repos: Pick<PlatformRepo, 'full_name' | 'granted'>[]): string[] {
  return [...new Set(repos.filter((r) => r.granted).map((r) => r.full_name))].sort();
}
