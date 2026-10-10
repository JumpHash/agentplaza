// Build: TypeScript + CSS sources -> apps/web/dist, plain static files.
//
// Tooling choice (boring on purpose): tsc typechecks (`npm run typecheck`),
// esbuild bundles. No framework, no dev server, no runtime dependencies beyond
// three.js. The output is exactly what an S3 bucket would serve: an index.html
// plus hashed, minified js/css at the ROOT of dist (noded serves bare file
// names only — no subdirectories).
//
// Deterministic: same sources -> same bundle bytes -> same content hash and
// same file names, so `npm run build` twice produces an identical dist.

import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync, rmSync, mkdirSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import * as esbuild from 'esbuild';

const root = dirname(fileURLToPath(import.meta.url));
const dist = join(root, 'dist');

rmSync(dist, { recursive: true, force: true });
mkdirSync(dist, { recursive: true });

const result = await esbuild.build({
  entryPoints: [join(root, 'src/main.ts')],
  bundle: true,
  minify: true,
  sourcemap: false,
  target: ['es2022'],
  format: 'esm',
  splitting: true, // the dynamic three.js import becomes an on-demand chunk
  chunkNames: 'chunk-[hash]',
  outdir: dist,
  entryNames: '[name]-[hash]',
  metafile: true,
  logLevel: 'silent',
});

let jsFile = null;
let cssFile = null;
for (const [name, entry] of Object.entries(result.metafile.outputs)) {
  if (entry.entryPoint?.endsWith('main.ts')) jsFile = name;
  // esbuild emits a sibling CSS bundle for the CSS imported by main.ts.
  if (!entry.entryPoint && name.endsWith('.css') && entry.imports.length === 0) cssFile = cssFile ?? name;
}
// The CSS bundle is named after the importing entry; fall back to any css.
if (!cssFile) cssFile = Object.keys(result.metafile.outputs).find((n) => n.endsWith('.css'));
if (!jsFile || !cssFile) {
  console.error('build: expected one js and one css bundle, got', Object.keys(result.metafile.outputs));
  process.exit(1);
}

const jsBase = basename(jsFile);
const cssBase = basename(cssFile);

// The bundle hash the colophon shows: sha256 over the exact bytes being served.
const bundleHash = createHash('sha256')
  .update(readFileSync(join(root, jsFile)))
  .update(readFileSync(join(root, cssFile)))
  .digest('hex');

const template = readFileSync(join(root, 'src/index.html'), 'utf8');
const html = template
  .replaceAll('{{JS}}', jsBase)
  .replaceAll('{{CSS}}', cssBase)
  .replaceAll('{{BUNDLE_HASH}}', bundleHash);
writeFileSync(join(dist, 'index.html'), html);

const kb = (n) => (n / 1024).toFixed(1) + ' KiB';
console.log(`dist/index.html  ${kb(html.length)}`);
console.log(`dist/${jsBase}  ${kb(readFileSync(join(root, jsFile)).length)}`);
console.log(`dist/${cssBase}  ${kb(readFileSync(join(root, cssFile)).length)}`);
console.log(`bundle sha256:${bundleHash.slice(0, 12)}…`);
