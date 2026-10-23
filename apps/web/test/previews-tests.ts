// The Previews tab's pure rules: a share's days become its seconds (whole days from 1 to 90, no others), the words and
// tones of a preview's status and of a link's scope and state. The DOM is not touched here. Bundled with the other suites
// by test/tests.ts; it prints "previews tests: ok".

import { DAYS_DEFAULT, DAYS_MAX, DAYS_MIN, LABEL_MAX, LINK_STATE, SCOPE_CHOICES, SCOPE_LABEL, STATUS, linkSeconds } from '../src/views/previews-model';

let checks = 0;

function eq<T>(actual: T, expected: T, what: string): void {
  checks++;
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) throw new Error(`${what}: expected ${b}, got ${a}`);
}

// ---- Share lengths ----------------------------------------------------------------------------------

eq([DAYS_MIN, DAYS_MAX, DAYS_DEFAULT, LABEL_MAX], [1, 90, 7, 80], 'the days range, the default and the label length');
eq(linkSeconds(1), 86400, 'one day is 86400 seconds');
eq(linkSeconds(DAYS_DEFAULT), 7 * 86400, 'the default week is 604800 seconds');
eq(linkSeconds(90), 90 * 86400, 'ninety days is the longest share');
eq([0, 91, -1, 1.5, NaN, Infinity].map((d) => linkSeconds(d)), [undefined, undefined, undefined, undefined, undefined, undefined], 'no share shorter than a day, longer than 90, or not whole days');

// ---- Words and tones --------------------------------------------------------------------------------

eq([STATUS.awake.label, STATUS.awake.tone], ['Awake', 'ok'], 'an awake preview is ok');
eq([STATUS.asleep.label, STATUS.closed.label], ['Asleep', 'Closed'], 'asleep and closed are named');
eq([SCOPE_LABEL.view, SCOPE_LABEL.use], ['Watch', 'Use'], 'the scopes are named Watch and Use');
eq(SCOPE_CHOICES.map((c) => c.value), ['view', 'use'], 'the scope choices are view then use');
eq([LINK_STATE.active.tone, LINK_STATE.revoked.tone, LINK_STATE.expired.tone], ['ok', 'danger', 'muted'], 'a link is ok, revoked (danger) or expired (muted)');

console.log(`previews tests: ok (${checks} checks)`);
