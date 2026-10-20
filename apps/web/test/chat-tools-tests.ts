// The chats' model chip and context bar: the number text, the bar's tone and text, the part order and colours, and the
// model value split. The DOM is not touched here. Bundled with the other suites by test/tests.ts; it prints
// "chat tools tests: ok".

import { barText, orderedParts, PART_COLOUR, shareText, splitModel, tokensText, usageTone, withEffort } from '../src/ui/chat-tools';

let checks = 0;

function eq<T>(actual: T, expected: T, what: string): void {
  checks++;
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) throw new Error(`${what}: expected ${b}, got ${a}`);
}

// ---- Numbers ---------------------------------------------------------------------------------------------------------

eq(tokensText(0), '0', 'no tokens reads 0');
eq(tokensText(950), '950', 'under a thousand is the whole number');
eq(tokensText(1500), '1.5k', 'a thousand and a half takes one decimal');
eq(tokensText(41200), '41.2k', 'tens of thousands take one decimal');
eq(tokensText(99_960), '100k', 'just under a hundred thousand rounds to 100k, not 0.1M');
eq(tokensText(200_000), '200k', 'whole thousands from a hundred thousand up');
eq(tokensText(1_234_567), '1.2M', 'millions take one decimal');
eq(tokensText(999_600), '1.0M', 'a rounding up to a thousand thousands becomes millions');
eq(tokensText(Number.NaN), '0', 'a non-number reads 0');
eq(shareText(21, 100), '21%', 'a share is a whole percentage');
eq(shareText(1, 0), '0%', 'a zero total reads 0%');

// ---- Tone and text --------------------------------------------------------------------------------------------------

eq(usageTone(0.5), 'ok', 'half full is ok');
eq(usageTone(0.8), 'ok', 'exactly 80 % is still ok');
eq(usageTone(0.81), 'warn', 'over 80 % is warn');
eq(usageTone(0.95), 'warn', 'exactly 95 % is still warn');
eq(usageTone(0.951), 'crit', 'over 95 % is crit');
eq(usageTone(null), 'ok', 'an unknown window is ok');
eq(barText(41_200, 200_000, 0.206), '41.2k / 200k · 21%', 'the bar text with the window');
eq(barText(41_200, null, null), '41.2k used', 'the bar text without the window');
eq(barText(41_200, 0, null), '41.2k used', 'a zero window counts as unknown');

// ---- Parts: order and colours ----------------------------------------------------------------------------------------

const parts = orderedParts([
  { part: 'history', tokens: 5 },
  { part: 'system', tokens: 3 },
  { part: 'not_a_part', tokens: 9 },
]);
eq(parts.map((p) => p.part), ['system', 'tools', 'history', 'tool_results', 'attachments', 'summary'], 'parts in the breakdown order, unknown dropped');
eq(parts.map((p) => p.tokens), [3, 0, 5, 0, 0, 0], 'a part the hub did not send counts zero');
eq(parts.map((p) => p.name), ['System', 'Tools', 'History', 'Tool results', 'Attachments', 'Summary'], 'the part names');
eq(parts[0]?.colour, 'var(--ink-3)', 'system is ink-3');
eq(parts[2]?.colour, 'var(--signal)', 'history is the signal colour');
eq(parts[5]?.colour, 'var(--warn)', 'summary is warn');
eq(new Set(Object.values(PART_COLOUR)).size, 6, 'each part has its own colour');

// ---- Model values ----------------------------------------------------------------------------------------------------

eq(splitModel('anthropic/claude@high'), { base: 'anthropic/claude', effort: 'high' }, 'an effort is the suffix after @');
eq(splitModel('openrouter/vendor/name'), { base: 'openrouter/vendor/name', effort: '' }, 'a model with a slash and no effort');
eq(withEffort('anthropic/claude', 'max'), 'anthropic/claude@max', 'an effort pins onto the base');
eq(withEffort('anthropic/claude', ''), 'anthropic/claude', 'no effort leaves the base');
eq(withEffort('', 'high'), '', 'no model means the default, whatever the effort');

console.log(`chat tools tests: ok (${checks} checks)`);
