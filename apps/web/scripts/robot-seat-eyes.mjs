// Seats the eyes of the robot species: each socket on the head front under the style rules, lifted the least it
// takes to stay visible from the portrait cameras in the rest, walk and wave poses (seat.ts, audit.ts). Only the
// `at` of the eye entries change; the rest of each file is kept byte for byte (a span replace, not a re-serialise).
//
//   node apps/web/scripts/robot-seat-eyes.mjs --species owl,ufo   seat these species
//   node apps/web/scripts/robot-seat-eyes.mjs --all               seat every valid species
//   node apps/web/scripts/robot-seat-eyes.mjs --dry-run ...       print the old and new positions, write nothing
//
// Prints per species: each eye's old and new socket z, the notes (a blocked eye names the parts to shrink or move),
// and the eye-occluded issues before and after.

import { build } from 'esbuild';
import { readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const out = join(tmpdir(), `apz-robot-seat-${process.pid}.mjs`);
const args = process.argv.slice(2);
const flag = (name) => args.includes(name);
const value = (name) => (args.includes(name) ? args[args.indexOf(name) + 1] : null);
const dryRun = flag('--dry-run');

const entry = [
  `export { auditSpecies } from ${JSON.stringify(join(root, 'src/views/floor3d/robots/audit.ts'))};`,
  `export { seatEyes } from ${JSON.stringify(join(root, 'src/views/floor3d/robots/seat.ts'))};`,
  `export { registry } from ${JSON.stringify(join(root, 'src/views/floor3d/robots/files.ts'))};`,
].join('\n');

await build({
  stdin: { contents: entry, resolveDir: root, loader: 'ts', sourcefile: 'seat-entry.ts' },
  bundle: true,
  platform: 'node',
  format: 'esm',
  outfile: out,
  loader: { '.css': 'empty' },
  logLevel: 'silent',
});

let mod;
try {
  mod = await import(pathToFileURL(out).href);
} finally {
  rmSync(out, { force: true });
}

const reg = mod.registry();
if (reg.errors.length) {
  console.error(`registry: ${reg.errors.length} validation error(s):`);
  for (const e of reg.errors) console.error(`  ${e}`);
}

let ids;
if (flag('--all')) ids = Object.keys(reg.species);
else if (value('--species')) ids = value('--species').split(',');
else {
  console.error('usage: robot-seat-eyes.mjs (--all | --species a,b) [--dry-run]');
  process.exit(2);
}

const fmt = (v) => {
  const r = Number(v.toFixed(4));
  return String(r === 0 ? 0 : r);
};

/**
 * The eyes block of a species file with each eye's `at` replaced where it changed (the other text is kept).
 * Eye objects have no nested braces, so each `{...}` in the eyes array is one eye, in order.
 */
function replaceEyeAts(text, oldAts, newAts) {
  const key = text.indexOf('"eyes"');
  if (key < 0) throw new Error('no "eyes" key');
  const open = text.indexOf('[', key);
  let depth = 0;
  let close = -1;
  for (let i = open; i < text.length; i++) {
    if (text[i] === '[') depth++;
    else if (text[i] === ']' && --depth === 0) {
      close = i;
      break;
    }
  }
  if (close < 0) throw new Error('unclosed "eyes" array');
  let k = 0;
  const block = text.slice(open, close + 1).replace(/\{[^{}]*\}/g, (obj) => {
    const i = k++;
    const next = newAts[i];
    const old = oldAts[i];
    if (!next || !old || next.every((v, j) => Number(v.toFixed(4)) === old[j])) return obj;
    return obj.replace(/"at"\s*:\s*\[[^\]]*\]/, `"at": [${next.map(fmt).join(', ')}]`);
  });
  return text.slice(0, open) + block + text.slice(close + 1);
}

const eyeIssues = (list) => list.filter((i) => i.kind.startsWith('eye-'));
const occludedEyes = (list) => new Set(list.filter((i) => i.kind === 'eye-occluded').map((i) => i.path)).size;

for (const id of ids) {
  const s = reg.species[id];
  if (!s) {
    console.error(`${id}: not a valid species in the registry`);
    process.exitCode = 1;
    continue;
  }
  const opts = { tools: reg.tools, clips: reg.clips };
  const before = eyeIssues(mod.auditSpecies(s, opts));
  const seat = mod.seatEyes(s, reg.clips);
  const oldAts = s.eyes.map((e) => e.at);
  const newAts = seat.eyes.map((e) => e.at);
  const file = join(root, '..', '..', 'assets', 'robots', 'species', `${id}.json`);
  const text = readFileSync(file, 'utf8');
  const next = replaceEyeAts(text, oldAts, newAts);
  const changed = next !== text;

  console.log(`\n${id} (${s.eyes.length} eyes): occluded before ${occludedEyes(before)}, eye issues before ${before.length}`);
  for (const n of seat.notes) console.log(`  ${n.blocked ? 'BLOCKED' : 'ok     '} eyes[${n.k}] ${n.text}`);
  if (!changed) console.log('  no change');
  if (changed && !dryRun) writeFileSync(file, next);

  // The after-state, from the new positions in memory (the file on disk is what the next run reads).
  const after = eyeIssues(mod.auditSpecies({ ...s, eyes: seat.eyes }, opts));
  console.log(`  occluded after ${occludedEyes(after)}, eye issues after ${after.length}${dryRun ? ' (dry run: not written)' : changed ? ' (written)' : ''}`);
  for (const i of after) console.log(`    [${i.severity}] ${i.kind} ${i.pose} ${i.path}: ${i.message}`);
}
