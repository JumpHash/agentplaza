// The Git platforms screen's pure rules: the top line, the labels, the authorizations grouped by host, the identity
// line, the grants body built from the checkboxes, the token and app forms, a flow's outcome and a connect result.
// The DOM is not touched here. Bundled with the other suites by test/tests.ts; it prints "platforms tests: ok".

import type { ConnectResult, FlowStatus, PlatformIntegration, PlatformRegistration } from '../src/api-platforms';
import {
  accountText,
  connectResultText,
  connectedCount,
  connectionText,
  count,
  flowOutcome,
  grantState,
  grantsBody,
  groupByHost,
  hasGrants,
  hostName,
  identityLine,
  levelLabel,
  namespaceLabel,
  registrationFor,
  registrationProblem,
  repositorySelectionLabel,
  sameHost,
  setAll,
  setRepo,
  stateLine,
  targetPlaceholder,
  tokenBody,
  tokenProblem,
  viaLabel,
} from '../src/views/platforms-model';

let checks = 0;

function eq<T>(actual: T, expected: T, what: string): void {
  checks++;
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) throw new Error(`${what}: expected ${b}, got ${a}`);
}

/** An authorization as the hub lists it; only the fields the screen reads matter here. */
function integ(over: Partial<PlatformIntegration>): PlatformIntegration {
  return {
    id: 'github-2', kind: 'github', host: 'https://github.com', via: 'app', level: 'org', registration: 'github-app-1',
    installation: 7, repository_selection: 'selected', manage_url: null, target: 'JumpHash',
    account: { id: 9, login: 'JumpHash', name: 'JumpHash', email: '', avatar_url: '' }, scopes: [], expires_ms: null,
    grants: { namespaces: {} }, created_ms: 1, checked_ms: 1, error: null, label: 'GitHub',
    identity: { id: 5, login: 'irfan', name: 'Irfan S', email: 'irfan@x.test', avatar_url: '' },
    ...over,
  };
}

const appReg: PlatformRegistration = {
  id: 'github-app-1', kind: 'github', host: 'https://github.com', client_id: 'Iv23li', created_ms: 1,
  app: { id: 42, slug: 'agentplaza-3f9c2a', name: 'AgentPlaza 3f9c2a', owner: 'irfan', html_url: 'https://github.com/apps/agentplaza-3f9c2a' },
};
const glReg: PlatformRegistration = { id: 'gitlab-app-4', kind: 'gitlab', host: 'https://gitlab.com', client_id: 'abc', app: null, created_ms: 2 };
const ghOauth: PlatformRegistration = { id: 'github-app-5', kind: 'github', host: 'https://ghe.test', client_id: 'x', app: null, created_ms: 3 };

// ---- The top line and labels ---------------------------------------------------

eq(stateLine({ authorizations: 0, connected: null, needsYou: 0 }), 'Nothing authorized yet: add GitHub, GitLab or Forgejo.', 'nothing authorized: the line says how to start');
eq(stateLine({ authorizations: 2, connected: 1, needsYou: 0 }), '2 authorizations. 1 repository connected.', 'the top line counts authorizations and connected repositories');
eq(stateLine({ authorizations: 1, connected: 0, needsYou: 0 }), '1 authorization. No repository connected.', 'no connection is said plainly');
eq(stateLine({ authorizations: 2, connected: 2, needsYou: 1 }), '2 authorizations. 2 repositories connected. 1 authorization needs you.', 'what needs the owner comes last');
eq(stateLine({ authorizations: 3, connected: null, needsYou: 2 }), '3 authorizations. 2 authorizations need you.', 'without Weft there is no connection count');
eq(count(1, 'repository', 'repositories'), '1 repository', 'one repository');
eq(count(0, 'change'), '0 changes', 'zero changes takes the plural');

eq(levelLabel('user', 'github'), 'Your account', 'the user level is the owner\'s account');
eq(levelLabel('org', 'github'), 'Organisation', 'an organisation on GitHub');
eq(levelLabel('org', 'gitlab'), 'Group', 'a GitLab organisation is a group');
eq(levelLabel('repo', 'forgejo'), 'One repository', 'the repository level');
eq([namespaceLabel('user'), namespaceLabel('org'), namespaceLabel('group')], ['Your account', 'Organisation', 'Group'], 'namespace kinds in words');
eq([viaLabel('app'), viaLabel('oauth'), viaLabel('token')], ['GitHub App', 'Signed in', 'Token'], 'how an authorization was given');
eq(hostName('https://gitlab.example.org/'), 'gitlab.example.org', 'a host shows without its scheme');
eq(hostName('http://127.0.0.1:3000'), '127.0.0.1:3000', 'a local host keeps its port');
eq(sameHost('https://GitHub.com/', 'https://github.com'), true, 'hosts compare without case or a trailing slash');
eq(sameHost('https://github.com', 'https://gitlab.com'), false, 'different hosts are different');
eq(accountText(integ({})), 'JumpHash', 'an organisation authorization is its organisation');
eq(accountText(integ({ level: 'user', target: null })), '@JumpHash', 'a user authorization is the owner\'s login');
eq(accountText(integ({ level: 'repo', target: 'irfan/notes', account: { id: 1, login: 'irfan', name: '', email: '', avatar_url: '' } })), 'irfan/notes', 'a repository authorization names the repository');
eq(accountText(integ({ level: 'user', target: null, account: { id: 1, login: '', name: 'Irfan S', email: '', avatar_url: '' } })), '@Irfan S', 'with no login the name stands in');

// ---- Registrations ------------------------------------------------------------------

const regs = [appReg, glReg, ghOauth];
eq(registrationFor(regs, 'github', 'https://github.com/')?.id, 'github-app-1', 'GitHub\'s card finds the App for its host');
eq(registrationFor(regs, 'github', 'https://ghe.test'), undefined, 'a GitHub registration without an App is not the App');
eq(registrationFor(regs, 'gitlab', 'https://gitlab.com')?.id, 'gitlab-app-4', 'GitLab\'s card finds its OAuth app');
eq(registrationFor(regs, 'gitlab', 'https://github.com'), undefined, 'an app on another host is not found');

// ---- Grouping and identity ----------------------------------------------------------------

const grouped = groupByHost([
  integ({ id: 'forgejo-1', kind: 'forgejo', host: 'https://codeberg.org', label: 'Forgejo' }),
  integ({ id: 'github-2' }),
  integ({ id: 'gitlab-3', kind: 'gitlab', host: 'https://gitlab.com', label: 'GitLab' }),
  integ({ id: 'github-4', level: 'user', target: null }),
]);
eq(grouped.map((g) => g.host), ['https://github.com', 'https://gitlab.com', 'https://codeberg.org'], 'hosts in the order GitHub, GitLab, Forgejo');
eq(grouped.map((g) => g.items.map((i) => i.id)), [['github-2', 'github-4'], ['gitlab-3'], ['forgejo-1']], 'each host keeps its authorizations in order');
eq(grouped.map((g) => g.label), ['GitHub', 'GitLab', 'Forgejo'], 'each group is labelled by its kind');

eq(identityLine({ name: 'Irfan S', login: 'irfan', email: 'irfan@x.test' }, 'https://github.com'), { text: 'Exports are by Irfan S <irfan@x.test>', warn: false }, 'exports are by the verified identity');
eq(identityLine({ name: '', login: 'irfan', email: 'irfan@x.test' }, 'https://github.com'), { text: 'Exports are by irfan <irfan@x.test>', warn: false }, 'the login stands in for a missing name');
eq(identityLine({ name: 'Irfan S', login: 'irfan', email: '' }, 'https://gitlab.com/'), { text: 'No verified email: sign in on gitlab.com', warn: true }, 'no verified email is a warning');
eq([repositorySelectionLabel('all'), repositorySelectionLabel('selected'), repositorySelectionLabel(null)], ['All repositories', 'Chosen repositories', null], 'GitHub\'s repository selection in words');
eq(connectionText({ weft_repo: 'pr1-notes', project: 'pr1', connection: 'github', state: 'in-step' }), '→ pr1-notes · In step', 'a connection in one line');
eq(connectedCount([{ connections: [{ platform: 'github' }, { platform: 'git' }] }, { connections: [{ platform: 'gitlab' }] }, { connections: [] }]), 2, 'connected repositories count the platform connections only');

// ---- Grants body -------------------------------------------------------------------------

const granted = {
  namespaces: {
    JumpHash: { all: true, repos: [] },
    irfan: { all: false, repos: ['notes'] },
  },
};
const draft = grantState(granted);
eq(grantsBody(draft), granted, 'the body holds every granted namespace, as it was');
eq(hasGrants(granted), true, 'an authorization with grants has something granted');
eq(hasGrants({ namespaces: { a: { all: false, repos: [] } } }), false, 'an empty namespace grant is nothing granted');
eq(hasGrants({ namespaces: {} }), false, 'no namespaces is nothing granted');

const copy = grantState(granted);
setRepo(copy, 'irfan', 'zeta', true);
setRepo(copy, 'irfan', 'notes', false);
eq(granted.namespaces.irfan, { all: false, repos: ['notes'] }, 'a draft does not touch the authorization it came from');
eq(grantsBody(copy).namespaces.irfan, { all: false, repos: ['zeta'] }, 'unticking a repository leaves the others');

setAll(copy, 'irfan', true);
eq(grantsBody(copy).namespaces.irfan, { all: true, repos: [] }, 'All repositories sends no repository names');
setAll(copy, 'irfan', false);
eq(grantsBody(copy).namespaces.irfan, { all: false, repos: ['zeta'] }, 'unticking All brings the named repositories back');

setAll(copy, 'JumpHash', false);
eq('JumpHash' in grantsBody(copy).namespaces, false, 'a namespace with nothing ticked is left out of the body');

const fresh = grantState({ namespaces: {} });
setRepo(fresh, 'new', 'alpha', true);
setRepo(fresh, 'new', 'alpha', true);
setRepo(fresh, 'new', 'zeta', true);
eq(grantsBody(fresh), { namespaces: { new: { all: false, repos: ['alpha', 'zeta'] } } }, 'a repository is named once, the names sorted');
setRepo(fresh, 'new', 'alpha', false);
setRepo(fresh, 'new', 'zeta', false);
eq(grantsBody(fresh), { namespaces: {} }, 'with nothing ticked the body is empty');

// ---- Forms ---------------------------------------------------------------------------

eq(targetPlaceholder('repo', 'github'), 'namespace/repo', 'a repository target');
eq(targetPlaceholder('org', 'gitlab'), 'group/path', 'a GitLab group target');
eq(targetPlaceholder('org', 'github'), 'organisation', 'an organisation target');
eq(tokenProblem('', 'user', ''), 'Paste the token first.', 'a token is needed');
eq(tokenProblem('glpat', 'org', '  '), 'Name the organisation or group it is for.', 'an organisation token names its target');
eq(tokenProblem('ghp', 'repo', ''), 'Name the repository (namespace/repo) it is for.', 'a repository token names its target');
eq(tokenProblem('ghp', 'user', ''), null, 'a personal token needs no target');
eq(tokenProblem('glpat', 'org', 'jumphash'), null, 'an organisation token with a target can be sent');
eq(
  tokenBody({ kind: 'gitlab', host: ' https://gitlab.com ', token: ' glpat ', level: 'user', target: 'ignored' }),
  { kind: 'gitlab', host: 'https://gitlab.com', token: 'glpat', level: 'user' },
  'a personal token body has no target',
);
eq(
  tokenBody({ kind: 'gitlab', host: 'https://gitlab.com', token: 'glpat', level: 'org', target: ' tools/platform ' }),
  { kind: 'gitlab', host: 'https://gitlab.com', token: 'glpat', level: 'org', target: 'tools/platform' },
  'a group token body names the group, trimmed',
);
eq(registrationProblem('', 'secret'), 'Enter the client ID and the client secret.', 'an OAuth app needs its client ID');
eq(registrationProblem('id', '  '), 'Enter the client ID and the client secret.', 'an OAuth app needs its client secret');
eq(registrationProblem('id', 'secret'), null, 'an OAuth app with both can be saved');

// ---- Outcomes -------------------------------------------------------------------------

const status = (s: Partial<FlowStatus> & Pick<FlowStatus, 'status'>): FlowStatus => s;
eq(flowOutcome(status({ status: 'pending' }), 'GitHub'), null, 'a flow still waiting has no outcome yet');
eq(flowOutcome(status({ status: 'done', detail: { integrations: ['github-2'] } }), 'GitHub'), { text: 'Authorized on GitHub.', error: false }, 'a finished flow that made an authorization');
eq(flowOutcome(status({ status: 'done', detail: {} }), 'GitHub'), { text: 'GitHub is set up.', error: false }, 'a finished flow with nothing to show');
eq(flowOutcome(status({ status: 'failed', error: 'uninstalled on GitHub' }), 'GitHub'), { text: 'uninstalled on GitHub', error: true }, 'a failed flow says why');
eq(flowOutcome(status({ status: 'failed' }), 'GitLab'), { text: 'The GitLab step failed.', error: true }, 'a failed flow with no reason');
eq(flowOutcome(status({ status: 'expired' }), 'GitLab'), { text: 'It took longer than 15 minutes. Start again.', error: true }, 'an expired flow');

const connected = (over: Partial<ConnectResult>): ConnectResult => ({ weft_repo: 'pr1-notes', connection: 'github', remote: 'https://github.com/irfan/notes.git', branch: 'main', author: 'Irfan S <irfan@x.test>', ...over });
eq(connectResultText(connected({})), { text: 'Connected to pr1-notes.', warn: false }, 'a connection with no import to report');
eq(connectResultText(connected({ imported: { commits: 12, releases: ['v0.1.0'] }, signing_key: 'registered' })), { text: 'Connected to pr1-notes. Imported 12 changes and 1 release.', warn: false }, 'the import in plain words');
eq(connectResultText(connected({ imported: { commits: 1, releases: [] } })), { text: 'Connected to pr1-notes. Imported 1 change.', warn: false }, 'one change, no release');
eq(connectResultText(connected({ imported: { commits: 3, releases: ['a', 'b'] } })), { text: 'Connected to pr1-notes. Imported 3 changes and 2 releases.', warn: false }, 'two releases');
eq(connectResultText(connected({ imported: { empty: true, commits: 0 } })), { text: 'Connected to pr1-notes. The repository is empty: the first export pushes Weft\'s history.', warn: false }, 'an empty repository says what happens next');
eq(connectResultText(connected({ imported: { commits: 0 } })), { text: 'Connected to pr1-notes. Nothing new to import.', warn: false }, 'an import with nothing new');
eq(connectResultText(connected({ imported: { error: 'boom' } })), { text: 'Connected to pr1-notes. The import failed: boom', warn: true }, 'a failed import is a warning');
eq(connectResultText(connected({ signing_key: { error: 'no scope' } })), { text: 'Connected to pr1-notes. The signing key could not be registered: no scope', warn: true }, 'a failed signing key is a warning');

console.log(`platforms tests: ok (${checks} checks)`);
