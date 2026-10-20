// Size budget for the console's engine bundle. For each file the budget file lists, the gzip (level 9) size must stay
// within its budget; a file over budget makes the script exit 1. Plain node, no dependencies.
//
//   node apps/web/scripts/size-budget.mjs <dist dir> [budget file]
//
// The budget file defaults to apps/web/size-budget.json. A pattern that matches no file fails too, so a bundle that was
// renamed or no longer built cannot pass silently.

import { gzipSync } from 'node:zlib';
import { readFileSync, readdirSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const MB = 1_000_000; // the budgets are decimal megabytes

const distDir = process.argv[2];
if (!distDir) {
  console.error('usage: node apps/web/scripts/size-budget.mjs <dist dir> [budget file]');
  process.exit(2);
}
const budgetPath = process.argv[3] ?? join(dirname(fileURLToPath(import.meta.url)), '..', 'size-budget.json');
const { budgets } = JSON.parse(readFileSync(budgetPath, 'utf8'));

const fmt = (n) => n.toLocaleString('en-US');
const problems = [];
console.log('size budget (gzip -9; 1 MB = 1,000,000 bytes)');

for (const { pattern, max_gzip_mb: maxMb } of budgets) {
  // A file name pattern: `*` matches any run of characters within one name.
  const folder = dirname(pattern);
  const re = new RegExp('^' + basename(pattern).replace(/[.+?^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '[^/]*') + '$');
  let names = [];
  try {
    names = readdirSync(join(distDir, folder)).filter((n) => re.test(n)).sort();
  } catch {
    // the folder is missing, so nothing matches
  }
  if (names.length === 0) {
    problems.push(`${pattern}: no file in ${distDir} matches it (renamed, or not built)`);
    continue;
  }
  const limit = Math.round(maxMb * MB);
  for (const name of names) {
    const file = join(folder, name);
    const bytes = readFileSync(join(distDir, file));
    const gzip = gzipSync(bytes, { level: 9 }).length;
    const over = gzip > limit;
    console.log(`  ${file}  raw ${fmt(bytes.length)} B  gzip ${fmt(gzip)} B  budget ${fmt(limit)} B  ${over ? 'OVER' : 'ok'}`);
    if (over) problems.push(`${file}: gzip ${fmt(gzip)} B is over its budget of ${fmt(limit)} B`);
  }
}

if (problems.length > 0) {
  for (const p of problems) console.error(`size budget: ${p}`);
  console.error('size budget: raise a budget in apps/web/size-budget.json only together with the reason, in the same commit');
  process.exit(1);
}
console.log('size budget: ok');
