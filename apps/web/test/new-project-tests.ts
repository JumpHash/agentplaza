// The new-project dialog's pure rules: what the bring-in form sends, the line an adoption reports, the line about picked
// files, and the authorizations and repositories the form offers. The DOM is not touched here. Bundled with the other
// suites by test/tests.ts; it prints "new project tests: ok".

import type { Adopted } from '../src/api-projects';
import type { PlatformIntegration, PlatformRepo } from '../src/api-platforms';
import type { PickedFile } from '../src/ui/file-drop';
import { WHERE_KINDS, adoptInput, authorizationLabel, filesLine, grantedRepos, importLine, type BringForm } from '../src/ui/new-project-model';

let checks = 0;

function eq<T>(actual: T, expected: T, what: string): void {
  checks++;
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) throw new Error(`${what}: expected ${b}, got ${a}`);
}

/** A bring-in form as a fresh dialog leaves it (a git address, maintenance, finished), with `over` changed. */
function form(over: Partial<BringForm> = {}): BringForm {
  return {
    title: 'Shop',
    goal: '',
    where: 'git',
    platform: '',
    repo: '',
    sign: false,
    git: 'https://example.com/shop.git',
    folder: '',
    branch: 'main',
    mode: 'maintenance',
    finished: true,
    environments: [],
    ...over,
  };
}

const env = (name: string, url = '', kind = 'production') => ({ name, kind, url });
const picked = (path: string, size: number): PickedFile => ({ path, file: { size } as unknown as File });
const base = { title: 'Shop', kind: 'code', mode: 'maintenance', finished: true };

// ---- The kinds of place a project can be brought from.
eq(WHERE_KINDS.map((w) => w.value), ['platform', 'git', 'path', 'files'], 'a project is brought from a platform, a git address, a folder or files, in that order');

// ---- adoptInput: the title, the source, the environments.
eq(adoptInput(form({ title: '  ' })).error, 'Give the project a title', 'a project needs a title');
eq(adoptInput(form({ where: 'platform' })).error, 'Pick the git platform account the repository is on', 'a repository needs its authorization');
eq(adoptInput(form({ where: 'platform', platform: 'gh1' })).error, 'Pick a repository', 'a repository needs a repository');
eq(
  adoptInput(form({ where: 'platform', platform: 'gh1', repo: 'acme/shop', goal: '  The shop  ' })).input,
  { ...base, goal: 'The shop', source: { platform: 'gh1', repo: 'acme/shop' } } as never,
  'a platform repository: its source, the goal trimmed',
);
eq(
  adoptInput(form({ where: 'platform', platform: 'gh1', repo: 'acme/shop', sign: true })).input?.source,
  { platform: 'gh1', repo: 'acme/shop', sign: true } as never,
  'a signed export asks for the hub key',
);
eq(adoptInput(form({ git: 'ftp://example.com/shop.git' })).error, 'The git address starts with https://', 'a git address is http or https');
eq(adoptInput(form({ git: 'https://' })).error, 'The git address starts with https://', 'a git address needs a host');
eq(adoptInput(form({ branch: '' })).input?.source, { git: 'https://example.com/shop.git', branch: 'main' } as never, 'a git branch defaults to main');
eq(adoptInput(form({ where: 'path', folder: 'shop' })).error, 'The folder is an absolute path on the hub, so it starts with /', 'a folder on the hub is absolute');
eq(
  adoptInput(form({ where: 'path', folder: ' /Users/me/code/shop ', branch: 'trunk' })).input?.source,
  { path: '/Users/me/code/shop', branch: 'trunk' } as never,
  'a folder on the hub, with its branch',
);
eq(adoptInput(form({ where: 'path', folder: '/srv/shop', branch: '' })).input?.source, { path: '/srv/shop', branch: 'main' } as never, 'a folder defaults to main');
eq(adoptInput(form({ where: 'files' })).input, base as never, 'files come after the adoption, so no source yet');
eq(
  adoptInput(form({ where: 'files', mode: 'build', finished: false })).input,
  { ...base, mode: 'build', finished: false } as never,
  'the mode and finished go through',
);

// ---- adoptInput: environments. Empty rows are ignored; names are checked.
eq(adoptInput(form({ where: 'files', environments: [env('')] })).input?.environments, undefined, 'empty environment rows are ignored');
eq(
  adoptInput(form({ where: 'files', environments: [env('prod', 'https://shop.example.com'), env('')] })).input?.environments,
  [{ name: 'prod', kind: 'production', url: 'https://shop.example.com' }] as never,
  'an environment with its address',
);
eq(
  adoptInput(form({ where: 'files', environments: [env('staging', '', 'staging')] })).input?.environments,
  [{ name: 'staging', kind: 'staging' }] as never,
  'an environment with no address',
);
const nameProblem = 'An environment name is lower-case letters, digits and dashes, and starts with a letter';
eq(adoptInput(form({ where: 'files', environments: [env('Prod')] })).error, nameProblem, 'an environment name is lower case');
eq(adoptInput(form({ where: 'files', environments: [env('1prod')] })).error, nameProblem, 'an environment name starts with a letter');
eq(adoptInput(form({ where: 'files', environments: [env('a'.repeat(41))] })).error, nameProblem, 'an environment name is at most 40 characters');
eq(adoptInput(form({ where: 'files', environments: [env('a'.repeat(40))] })).error, undefined, 'a 40 character name is fine');
eq(adoptInput(form({ where: 'files', environments: [env('', 'https://x.example.com')] })).error, nameProblem, 'an address needs a name');
eq(adoptInput(form({ where: 'files', environments: [env('prod'), env('prod', '', 'staging')] })).error, 'Two environments are called prod', 'environment names are unique');
eq(
  adoptInput(form({ where: 'files', environments: [env('a1'), env('a2'), env('a3'), env('a4')] })).error,
  'A project has up to 3 environments here',
  'three environments at most',
);
eq(adoptInput(form({ where: 'files', environments: [env('prod', 'ftp://x')] })).error, 'prod needs an https address', 'an environment address is https');
eq(adoptInput(form({ where: 'files', environments: [env('prod', '', 'nowhere')] })).error, 'Pick a kind for prod', 'an environment has a known kind');

// ---- importLine: what an adoption reports, as one line.
eq(importLine({ import: { commits: 12 }, problems: [] } as unknown as Adopted), 'Imported 12 changes', 'an import counts its changes');
eq(importLine({ import: { commits: 1 }, problems: [] } as unknown as Adopted), 'Imported 1 change', 'one change is singular');
eq(
  importLine({ import: { imported: { commits: 3 } }, problems: ['The hook was not saved.'] } as unknown as Adopted),
  'Imported 3 changes. The hook was not saved',
  'a platform import counts its changes, then the problems',
);
eq(importLine({ import: null, problems: [] } as unknown as Adopted), '', 'nothing imported and no problems says nothing');
eq(importLine({ import: { commits: 0 }, problems: ['Two things failed.'] } as unknown as Adopted), 'Two things failed', 'no changes, only the problems');
eq(importLine({ import: { imported: { empty: true } }, problems: [] } as unknown as Adopted), 'The repository is empty', 'an empty repository says so');
eq(importLine({ import: { imported: { error: 'boom' } }, problems: ['boom'] } as unknown as Adopted), 'boom', 'a problem named twice is said once');

// ---- filesLine: the files that go in, their size, and what is left out.
eq(filesLine([]), '', 'no files, no line');
eq(filesLine([picked('a.txt', 512), picked('src/b.rs', 512)]), '2 files, 1 KB', 'two files and their size');
eq(filesLine([picked('a.txt', 812)]), '1 file, 812 B', 'one file is singular');
eq(
  filesLine([picked('a.txt', 1048576), picked('b.txt', 1048576), picked('c.txt', 1048576), picked('node_modules/x.js', 10), picked('.git/HEAD', 10)]),
  '3 files, 3.0 MB · 2 left out (.git, node_modules)',
  'the files left out are counted',
);
eq(filesLine([picked('.DS_Store', 10)]), 'No files to send · 1 left out (.git, node_modules)', 'only files left out');

// ---- The authorizations and repositories the form lists.
const integration = (over: Record<string, unknown>) =>
  ({ kind: 'github', level: 'user', target: null, account: { login: 'irfan', name: 'Irfan' }, error: null, ...over }) as unknown as PlatformIntegration;
eq(authorizationLabel(integration({})), 'GitHub · @irfan', 'an account authorization: kind and account');
eq(authorizationLabel(integration({ kind: 'gitlab', level: 'org', target: 'acme' })), 'GitLab · acme', 'an organisation authorization: kind and target');
eq(authorizationLabel(integration({ kind: 'forgejo', error: 'expired' })), 'Forgejo · @irfan · needs you', 'an authorization that needs the owner says so');
const repo = (full_name: string, granted: boolean) => ({ full_name, granted }) as unknown as PlatformRepo;
eq(
  grantedRepos([repo('acme/shop', true), repo('acme/tools', false), repo('acme/shop', true), repo('acme/api', true)]),
  ['acme/api', 'acme/shop'],
  'only the granted repositories, once each, in order',
);
eq(grantedRepos([repo('acme/tools', false)]), [], 'no granted repository offers none');

console.log(`new project tests: ok (${checks} checks)`);
export {};
