// Build: TypeScript + CSS sources -> apps/web/dist, plain static files.
//
// Tooling choice (boring on purpose): tsc typechecks (`npm run typecheck`),
// esbuild bundles. No framework, no dev server, no runtime dependencies.
// The output is exactly what an S3 bucket would serve: an index.html
// plus hashed, minified js/css at the ROOT of dist (noded serves bare file
// names only — no subdirectories).
//
// Deterministic: same sources -> same bundle bytes -> same content hash and
// same file names, so `npm run build` twice produces an identical dist.

import { createHash } from 'node:crypto';
import { execFileSync, spawnSync } from 'node:child_process';
import { readFileSync, writeFileSync, rmSync, mkdirSync, readdirSync, copyFileSync, renameSync, existsSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { gzipSync } from 'node:zlib';
import * as esbuild from 'esbuild';

const root = dirname(fileURLToPath(import.meta.url));
// The build goes to dist.next and replaces dist only once it is complete, so a hub serving dist
// never sees it empty or half written (and a failed build leaves the last good one in place).
const live = join(root, 'dist');
const dist = join(root, 'dist.next');

rmSync(dist, { recursive: true, force: true });
mkdirSync(dist, { recursive: true });

// The fonts (assets/fonts, OFL) sit at the root of dist too: noded serves bare file names only.
const fontsDir = join(root, '../../assets/fonts');
for (const file of readdirSync(fontsDir).filter((f) => f.endsWith('.ttf'))) {
  copyFileSync(join(fontsDir, file), join(dist, file));
}

// The engine wasm (crates/apz-engine-web): build it, bind it with wasm-bindgen, shrink it with
// wasm-opt when present, then write it and its glue to the dist root under content-hashed names.
const repoRoot = join(root, '../..');
const engineLib = 'apz_engine_web';
const engineScratch = join(root, 'dist.engine-tmp');

// Runs a tool with its output inherited; throws on failure so the build stops.
function run(cmd, args, opts = {}) {
  const r = spawnSync(cmd, args, { stdio: 'inherit', ...opts });
  if (r.error) throw r.error;
  if (r.status !== 0) throw new Error(`build: ${cmd} ${args.join(' ')} exited with ${r.status}`);
}

// The engine is optional until crates/apz-engine-web exists: without it the app builds as before (the names are null
// and the app keeps its three.js floor).
let engineJsName = null;
let engineWasmName = null;
let engineWasm = null;
if (existsSync(join(repoRoot, 'crates/apz-engine-web/Cargo.toml'))) {
  // The wasm-bindgen CLI must be the exact version Cargo.lock pins, or the glue and the wasm disagree.
  const lockText = readFileSync(join(repoRoot, 'Cargo.lock'), 'utf8');
  const pinnedBindgen = lockText.match(/^name = "wasm-bindgen"\r?\nversion = "([^"]+)"/m)?.[1];
  if (!pinnedBindgen) throw new Error('build: wasm-bindgen is not pinned in Cargo.lock');
  const cliBindgen = execFileSync('wasm-bindgen', ['--version'], { encoding: 'utf8' }).trim().split(/\s+/).pop();
  if (cliBindgen !== pinnedBindgen) {
    throw new Error(`build: wasm-bindgen CLI is ${cliBindgen} but Cargo.lock pins ${pinnedBindgen}`);
  }

  run('cargo', ['build', '-p', 'apz-engine-web', '--target', 'wasm32-unknown-unknown', '--profile', 'wasm-release'], { cwd: repoRoot });

  rmSync(engineScratch, { recursive: true, force: true });
  try {
    run('wasm-bindgen', [
      '--target', 'web',
      '--out-dir', engineScratch,
      join(repoRoot, 'target/wasm32-unknown-unknown/wasm-release', `${engineLib}.wasm`),
    ]);
    // wasm-opt works on the wasm-bindgen output (the bytes the glue loads), not the raw cargo output.
    const bindgenWasm = join(engineScratch, `${engineLib}_bg.wasm`);
    engineWasm = readFileSync(bindgenWasm);
    let hasWasmOpt = true;
    try {
      execFileSync('which', ['wasm-opt'], { stdio: 'ignore' });
    } catch {
      hasWasmOpt = false;
    }
    if (hasWasmOpt) {
      const optimised = join(engineScratch, `${engineLib}_opt.wasm`);
      run('wasm-opt', ['-Oz', '-o', optimised, bindgenWasm]);
      engineWasm = readFileSync(optimised);
    } else {
      console.log('wasm-opt not found, skipped');
    }

    const engineHash = createHash('sha256').update(engineWasm).digest('hex').slice(0, 8);
    engineWasmName = `${engineLib}-${engineHash}_bg.wasm`;
    engineJsName = `${engineLib}-${engineHash}.js`;

    // The glue loads its wasm by bare file name; point it at the fingerprinted copy, exactly once.
    const glueRaw = readFileSync(join(engineScratch, `${engineLib}.js`), 'utf8');
    const wasmRef = `'${engineLib}_bg.wasm'`;
    if (glueRaw.split(wasmRef).length !== 2) {
      throw new Error(`build: expected ${wasmRef} exactly once in the wasm-bindgen glue`);
    }
    writeFileSync(join(dist, engineWasmName), engineWasm);
    writeFileSync(join(dist, engineJsName), glueRaw.replace(wasmRef, `'${engineWasmName}'`));
  } finally {
    rmSync(engineScratch, { recursive: true, force: true });
  }
} else {
  console.log('engine: crates/apz-engine-web is not there yet, skipped');
}

const result = await esbuild.build({
  // Output paths in the metafile are relative to this, so the build works from any working directory.
  absWorkingDir: root,
  entryPoints: [join(root, 'src/main.ts')],
  bundle: true,
  minify: true,
  sourcemap: false,
  target: ['es2022'],
  format: 'esm',
  outdir: dist,
  // The @font-face urls name the copied font files; leave them as written.
  external: ['*.ttf'],
  // The engine's fingerprinted glue and wasm names, for the app to import the engine by.
  define: {
    __APZ_ENGINE_JS__: JSON.stringify(engineJsName),
    __APZ_ENGINE_WASM__: JSON.stringify(engineWasmName),
  },
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

const old = join(root, 'dist.old');
rmSync(old, { recursive: true, force: true });
if (existsSync(live)) renameSync(live, old);
renameSync(dist, live);
rmSync(old, { recursive: true, force: true });

const kb = (n) => (n / 1024).toFixed(1) + ' KiB';
console.log(`dist/index.html  ${kb(html.length)}`);
console.log(`dist/${jsBase}  ${kb(readFileSync(join(live, jsBase)).length)}`);
console.log(`dist/${cssBase}  ${kb(readFileSync(join(live, cssBase)).length)}`);
console.log(`bundle sha256:${bundleHash.slice(0, 12)}…`);
if (engineWasm) console.log(`dist/${engineWasmName}  ${kb(engineWasm.length)}  gzip -9 ${kb(gzipSync(engineWasm, { level: 9 }).length)}`);
