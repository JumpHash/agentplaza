// The test harness: the repo has no JS test runner, so `npm test` bundles
// the assertion entry with esbuild (already the build tool) and executes it
// with node. The store's DOM-free paths are exercised directly.

import { build } from 'esbuild';
import { rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const out = join(tmpdir(), `apz-web-tests-${process.pid}.mjs`);

await build({
  entryPoints: [join(root, 'test/tests.ts')],
  bundle: true,
  platform: 'node',
  format: 'esm',
  outfile: out,
  // Views import their stylesheets; the logic under test needs none of them.
  loader: { '.css': 'empty' },
  logLevel: 'silent',
});
try {
  await import(pathToFileURL(out).href);
} finally {
  rmSync(out, { force: true });
}
