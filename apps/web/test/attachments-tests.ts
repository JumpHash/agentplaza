// The attachment rules of the composers: whether a message may go, a chip's text, and a file's name in a message. The
// DOM is not touched here. Bundled with the other suites by test/tests.ts; it prints "attachments tests: ok".

import { chipLabel, fileName, maySend } from '../src/ui/attach-bar';

let checks = 0;

function eq<T>(actual: T, expected: T, what: string): void {
  checks++;
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) throw new Error(`${what}: expected ${b}, got ${a}`);
}

// ---- Sending -----------------------------------------------------------------------------------------------------

eq(maySend('hello', [], false), true, 'text alone can go');
eq(maySend('', ['a1'], false), true, 'files alone can go, with no text');
eq(maySend('  hello  ', ['a1', 'b2'], false), true, 'text and files can go together');
eq(maySend('', [], false), false, 'an empty message does not go');
eq(maySend('   \n\t ', [], false), false, 'a blank message does not go');
eq(maySend('hello', [], true), false, 'nothing goes while a file is uploading');
eq(maySend('', ['a1'], true), false, 'files do not go while another is uploading');

// ---- Chips -------------------------------------------------------------------------------------------------------

eq(chipLabel(14 * 1024, 'ready'), '14 KB', 'a ready file shows its size');
eq(chipLabel(812, 'ready'), '812 B', 'a small file shows bytes');
eq(chipLabel(3355443, 'ready'), '3.2 MB', 'a megabyte file shows one decimal');
eq(chipLabel(14 * 1024, 'sending'), '14 KB · sending…', 'a file on its way says so');
eq(chipLabel(3355443, 'failed'), '3.2 MB · failed', 'a file that failed says so');

// ---- Names -------------------------------------------------------------------------------------------------------

eq(fileName('notes.md'), 'notes.md', 'a loose file keeps its name');
eq(fileName('src/main.rs'), 'main.rs', 'a file in a folder is named by its last part');
eq(fileName('a/b/c.txt'), 'c.txt', 'a deep path gives the last part');

console.log(`attachments tests: ok (${checks} checks)`);
