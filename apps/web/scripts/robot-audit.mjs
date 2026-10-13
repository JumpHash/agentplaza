// Runs the robot design audit (src/views/floor3d/robots/audit.ts) over every valid species and prints it.
//
//   node scripts/robot-audit.mjs                  counts per species, the worst ten, the medians
//   node scripts/robot-audit.mjs --issues         also every issue, with its JSON path and label
//   node scripts/robot-audit.mjs --species owl,ufo only these species (the medians still use all of them)
//   node scripts/robot-audit.mjs --json           the raw result: { metrics, medians, issues } by species
//   node scripts/robot-audit.mjs --md             a markdown report: metrics and issues per species
//
// The audit is bundled with esbuild (already the build tool), as test/run.mjs does, and run with node.

import { build } from 'esbuild';
import { rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const out = join(tmpdir(), `apz-robot-audit-${process.pid}.mjs`);
const args = process.argv.slice(2);
const flag = (name) => args.includes(name);
const only = args.includes('--species') ? args[args.indexOf('--species') + 1].split(',') : null;

const entry = [
  `export { auditAll, metrics, medians } from ${JSON.stringify(join(root, 'src/views/floor3d/robots/audit.ts'))};`,
  `export { registry } from ${JSON.stringify(join(root, 'src/views/floor3d/robots/files.ts'))};`,
].join('\n');

await build({
  stdin: { contents: entry, resolveDir: root, loader: 'ts', sourcefile: 'audit-entry.ts' },
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
const all = Object.values(reg.species);
const metricsOf = Object.fromEntries(all.map((s) => [s.id, mod.metrics(s)]));
const med = mod.medians(all.map((s) => metricsOf[s.id]));
const issues = mod.auditAll(all.filter((s) => !only || only.includes(s.id)), { tools: reg.tools, clips: reg.clips, medians: med });

if (flag('--json')) {
  const result = {};
  for (const [id, list] of Object.entries(issues)) result[id] = { metrics: metricsOf[id], issues: list };
  console.log(JSON.stringify({ medians: med, species: result }, null, 2));
  process.exit(0);
}

const score = (list) => list.reduce((n, i) => n + (i.severity === 'error' ? 2 : 1), 0);
const counts = (list) => {
  const by = {};
  for (const i of list) by[i.kind] = (by[i.kind] ?? 0) + 1;
  return Object.entries(by).map(([k, v]) => `${k}:${v}`).join(' ');
};
const rows = Object.entries(issues).map(([id, list]) => ({ id, list, errors: list.filter((i) => i.severity === 'error').length, warns: list.filter((i) => i.severity === 'warn').length, score: score(list) }));
rows.sort((a, b) => b.score - a.score || a.id.localeCompare(b.id));

if (flag('--md')) {
  const f = (v, d = 3) => (typeof v === 'number' ? v.toFixed(d) : '-');
  console.log('| species | parts | eyes | height | width | head/body | head/height | eye/head | eye spacing | eye height | ink | errors | warns |');
  console.log('|---|---|---|---|---|---|---|---|---|---|---|---|---|');
  for (const r of rows) {
    const m = metricsOf[r.id];
    console.log(`| ${r.id} | ${m.parts} | ${m.eyes} | ${f(m.height)} | ${f(m.width)} | ${f(m.headToBody)} | ${f(m.headToHeight)} | ${f(m.eyeRelHead)} | ${f(m.eyeSpacing)} | ${f(m.eyeHeight)} | ${f(m.inkCoverage, 2)} | ${r.errors} | ${r.warns} |`);
  }
  console.log('');
  for (const r of rows) {
    if (!r.list.length) continue;
    console.log(`### ${r.id}\n`);
    for (const i of r.list) console.log(`- **${i.severity}** \`${i.kind}\` (${i.pose}) \`${i.path}\` "${i.label}": ${i.message}`);
    console.log('');
  }
  process.exit(0);
}

console.log('species          errors warns  kinds');
for (const r of rows) console.log(`${r.id.padEnd(16)} ${String(r.errors).padStart(6)} ${String(r.warns).padStart(5)}  ${counts(r.list)}`);
const total = rows.reduce((n, r) => n + r.errors + r.warns, 0);
console.log(`total: ${total} issue(s) over ${rows.length} species; ${rows.filter((r) => r.errors === 0).length} species with no errors`);
console.log(`worst ten: ${rows.slice(0, 10).map((r) => `${r.id} (${r.score})`).join(', ')}`);
console.log(`medians: ${Object.entries(med).map(([k, v]) => `${k} ${v.toFixed(3)}`).join(', ')}`);

if (flag('--issues')) {
  for (const r of rows) {
    if (!r.list.length) continue;
    console.log(`\n${r.id}`);
    for (const i of r.list) console.log(`  [${i.severity}] ${i.kind} ${i.pose} ${i.path} "${i.label}": ${i.message}`);
  }
}
