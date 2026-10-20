// The Environments tab's pure rules: the order and wording of the environments, what a deploy's status and outcome say,
// the NAME=value lines of the variables, the names, the URLs, the defaults, and the line of what is deployed now.
// The DOM is not touched here. Bundled with the other suites by test/tests.ts; it prints "environments tests: ok".

import type { Deploy, DeployStatus, EnvKind, Environment } from '../src/api-projects';
import {
  NAME_RULE,
  TRACK_CHOICES,
  VAR_RULE,
  defaultProtected,
  defaultTrack,
  deployOutcome,
  deployStatusText,
  deployTone,
  hookText,
  kindLabel,
  nowText,
  parseVariables,
  recentDeploys,
  sortEnvironments,
  trackText,
  validName,
  validUrl,
  validVarName,
  variableRows,
  variablesText,
  webUrl,
} from '../src/views/environments-model';

let checks = 0;

function eq<T>(actual: T, expected: T, what: string): void {
  checks++;
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) throw new Error(`${what}: expected ${b}, got ${a}`);
}

/** An environment as the hub lists it; only the fields the tab reads matter here. */
function env(over: Partial<Environment>): Environment {
  return {
    name: 'staging', kind: 'staging', track: 'main', protected: false, variables: {}, notes: '', deployed_ms: 0,
    deploys: [], created_ms: 1, updated_ms: 1, secrets: [], hook: false, ...over,
  };
}

/** A deploy as the hub lists it. */
function dep(over: Partial<Deploy>): Deploy {
  return { id: 'd1', version: 'v1', by: 'irfan', at_ms: 1, status: 'sent', detail: '', ...over };
}

const KINDS: EnvKind[] = ['production', 'staging', 'preview', 'development', 'other'];

// ---- Order and names ----------------------------------------------------------------

const mixed = [
  env({ name: 'b-dev', kind: 'development' }),
  env({ name: 'prod', kind: 'production' }),
  env({ name: 'zz', kind: 'other' }),
  env({ name: 'pr-2', kind: 'preview' }),
  env({ name: 'staging', kind: 'staging' }),
  env({ name: 'pr-1', kind: 'preview' }),
];
eq(sortEnvironments(mixed).map((e) => e.name), ['prod', 'staging', 'pr-1', 'pr-2', 'b-dev', 'zz'], 'production first, then staging, preview, development and other; by name within a kind');
const given = [env({ name: 'b', kind: 'staging' }), env({ name: 'a', kind: 'production' })];
eq(sortEnvironments(given).map((e) => e.name), ['a', 'b'], 'the sorted list puts production first');
eq(given.map((e) => e.name), ['b', 'a'], 'the list it is given is not reordered');
eq(sortEnvironments([env({ name: 'x', kind: 'mystery' as EnvKind }), env({ name: 'a', kind: 'production' })]).map((e) => e.name), ['a', 'x'], 'an unknown kind goes last');
eq(KINDS.map(kindLabel), ['Production', 'Staging', 'Preview', 'Development', 'Other'], 'the kinds in words');

// ---- Track and defaults -------------------------------------------------------------

eq(trackText('main'), 'every landing on main', 'main runs every landing');
eq(trackText('releases'), 'each signed release', 'releases run each signed release');
eq(trackText('manual'), 'what you deploy by hand', 'manual runs what is deployed by hand');
eq(TRACK_CHOICES.map((c) => c.label), ['Every landing on main', 'Each signed release', 'What I deploy by hand'], 'the Runs choices, in the form\'s words');
eq(TRACK_CHOICES.map((c) => c.value), ['main', 'releases', 'manual'], 'the Runs choices carry the tracks');
eq(KINDS.map(defaultTrack), ['releases', 'main', 'main', 'manual', 'manual'], 'production releases, staging and preview follow main, the rest are by hand');
eq(KINDS.map(defaultProtected), [true, false, false, false, false], 'only production is protected by default');

// ---- Deploys ------------------------------------------------------------------------

const statuses: DeployStatus[] = ['recorded', 'sending', 'sent', 'failed', 'waiting', 'refused'];
eq(statuses.map(deployTone), ['ok', 'warn', 'ok', 'danger', 'warn', 'danger'], 'done is ok, going or waiting is warn, failed or refused is danger');
eq(statuses.map(deployStatusText), ['Recorded', 'Sending', 'Sent', 'Failed', 'Waiting', 'Refused'], 'the status badges in words');
eq(deployOutcome('staging', 'sent'), { text: 'Sent to staging.', error: false }, 'a sent deploy says where it went');
eq(deployOutcome('staging', 'recorded'), { text: 'Recorded for staging.', error: false }, 'a recorded deploy is not an error');
eq(deployOutcome('staging', 'waiting'), { text: 'Waiting for an approval before it goes to staging.', error: false }, 'a waiting deploy waits for an approval');
eq(deployOutcome('production', 'refused'), { text: 'The deploy to production was refused.', error: true }, 'a refused deploy is an error');
eq(deployOutcome('production', 'failed').error, true, 'a failed deploy is an error');

const many = ['d1', 'd2', 'd3', 'd4', 'd5', 'd6', 'd7'].map((id) => dep({ id }));
eq(recentDeploys(many).map((d) => d.id), ['d7', 'd6', 'd5', 'd4', 'd3'], 'the last five deploys, newest first');
eq(recentDeploys(many.slice(0, 2)).map((d) => d.id), ['d2', 'd1'], 'fewer than five: all of them, newest first');
eq(recentDeploys([]), [], 'no deploys, none shown');
eq(recentDeploys(many, 0), [], 'zero shown when asked for none');
eq(many[0]?.id, 'd1', 'the hub\'s list is not reordered');

// ---- What is deployed now, and the hook --------------------------------------------

const now = Date.UTC(2026, 9, 18, 12, 0, 0);
eq(hookText(true), 'Deploy hook: set', 'a hook is set');
eq(hookText(false), 'No deploy hook: deploys are recorded only', 'no hook: deploys are only recorded');
eq(nowText(env({}), now), 'Nothing deployed yet', 'nothing deployed yet without a version');
eq(nowText(env({ version: 'v1.4.0', deployed_ms: now - 5 * 60_000 }), now), 'Now: v1.4.0 · 5 min ago', 'the version and how long ago it went');
eq(nowText(env({ version: 'main@ab12cd', deployed_ms: now - 2 * 3_600_000 }), now), 'Now: main@ab12cd · 2 h ago', 'a branch head shows too');

// ---- Variables ----------------------------------------------------------------------

eq(
  parseVariables('A=1\n\nB = two words \n URL=https://x.test/a?b=c'),
  { vars: { A: '1', B: 'two words', URL: 'https://x.test/a?b=c' }, error: '' },
  'NAME=value lines; blank lines skipped; the value is what follows the first =, trimmed',
);
eq(parseVariables('').vars, {}, 'no lines, no variables');
eq(parseVariables('A=1\nnope').error, 'Line 2 is not NAME=value.', 'a line without = is named');
eq(parseVariables('=x').error, 'Line 1 has no name before the =.', 'a line with no name is named');
eq(parseVariables('1A=x').error, `Line 1: "1A" is not a name. ${VAR_RULE}`, 'a name starting with a digit is refused, with the rule');
eq(parseVariables('MY-VAR=x').error, `Line 1: "MY-VAR" is not a name. ${VAR_RULE}`, 'a dash is not in a name');
eq(parseVariables('A=1\nA=2').error, 'Line 2: A is given twice.', 'a name given twice is refused');
eq(parseVariables('A=1\nB=2\nBAD LINE').vars, {}, 'one bad line gives no variables at all');
eq(parseVariables('constructor=1').vars, { constructor: '1' }, 'a name that is a prototype key is still an ordinary variable');
const vars = { ZETA: 'last', ALPHA: 'a=b', beta: '' };
eq(variablesText(vars), 'ALPHA=a=b\nZETA=last\nbeta=', 'the variables as NAME=value lines, by name');
eq(parseVariables(variablesText(vars)).vars, { ALPHA: 'a=b', ZETA: 'last', beta: '' }, 'the lines read back to the same variables');
eq(variablesText({}), '', 'no variables, no lines');
eq(variableRows({ ZETA: 'last', ALPHA: 'a' }), [['ALPHA', 'a'], ['ZETA', 'last']], 'the two-column rows, by name');

// ---- Names ----------------------------------------------------------------------------

eq(['production', 'pr-12', 'a', 'eu-west-1'].map(validName), [true, true, true, true], 'lower-case names with digits and dashes are fine');
eq(['Prod', '1prod', '', 'prod_1', 'prod name', '-prod'].map(validName), [false, false, false, false, false, false], 'other names are refused');
eq(validName('a'.repeat(40)), true, 'forty characters is the most a name may be');
eq(validName('a'.repeat(41)), false, 'forty-one characters is too many');
eq(NAME_RULE.includes('40'), true, 'the rule for a name says how long it may be');
eq(['STRIPE_KEY', 'k2', 'Token'].map(validVarName), [true, true, true], 'variable and secret names: letters, digits and _');
eq(['_k', '2k', 'a-b', ''].map(validVarName), [false, false, false, false], 'a name starts with a letter, and has no dash');

// ---- URLs -----------------------------------------------------------------------------

eq(validUrl(''), true, 'an empty address is none, which is fine');
eq(['https://example.test', 'http://127.0.0.1:3000/x'].map(validUrl), [true, true], 'an absolute web address is fine');
eq(['example.test', 'javascript:alert(1)', 'ftp://x.test'].map(validUrl), [false, false, false], 'anything else is refused');
eq(webUrl('https://a.test'), true, 'https is a web address');
eq([webUrl(undefined), webUrl(null), webUrl('not a url')], [false, false, false], 'missing or unparsable is not a web address');

console.log(`environments tests: ok (${checks} checks)`);
