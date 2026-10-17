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
import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync, rmSync, mkdirSync, readdirSync, copyFileSync, renameSync, existsSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import * as esbuild from 'esbuild';

const root = dirname(fileURLToPath(import.meta.url));
// The build goes to dist.next and replaces dist only once it is complete, so a hub serving dist
// never sees it empty or half written (and a failed build leaves the last good one in place).
const live = join(root, 'dist');
const dist = join(root, 'dist.next');

rmSync(dist, { recursive: true, force: true });
mkdirSync(dist, { recursive: true });

// The faces the console names (styles/tokens.css) sit at the root of dist too: noded serves bare file names only. Inter
// (woff2, variable, upright and italic) is the interface and reading face; Martian Mono (ttf) is for code. The Source
// Serif 4 and Inter TTF files in assets/fonts are the desktop's (it embeds them), so the web does not copy them.
const fontsDir = join(root, '../../assets/fonts');
const webFace = /^(MartianMono-.*\.ttf|InterVariable(-Italic)?\.woff2)$/;
for (const file of readdirSync(fontsDir).filter((f) => webFace.test(f))) {
  copyFileSync(join(fontsDir, file), join(dist, file));
}

// The engine is the web's 3D (crates/apz-engine-web): without the crate the names are null and the app falls back to
// its 2D floor. With it, apz-pack (tools/apz-pack, Rust) builds the engine for wasm, binds it
// with wasm-bindgen at the version Cargo.lock pins (and, built with --features wasm-opt, runs wasm-opt -Oz, kept only
// when it compresses smaller), and writes the glue and the wasm to dist.next under content-hashed names. It prints one
// JSON line with those names and the sizes.
const repoRoot = join(root, '../..');
// apz-pack <subcommand> --out dist: builds one browser crate for wasm, binds it and writes its files; returns its JSON line.
function packWeb(subcommand) {
  const out = execFileSync('cargo', ['run', '-q', '-p', 'apz-pack', '--release', '--', subcommand, '--out', dist], {
    cwd: repoRoot,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'inherit'],
  });
  const line = out.split('\n').map((l) => l.trim()).filter((l) => l.startsWith('{')).pop();
  if (!line) throw new Error(`build: apz-pack ${subcommand} printed no JSON line`);
  return JSON.parse(line);
}
let engineJsName = null;
let engineWasmName = null;
let engineSizes = null;
if (existsSync(join(repoRoot, 'crates/apz-engine-web/Cargo.toml'))) {
  const engine = packWeb('web-engine');
  engineJsName = engine.js;
  engineWasmName = engine.wasm;
  engineSizes = { bytes: engine.bytes, gzip: engine.gzip, brotli: engine.brotli, wasmOpt: engine.wasm_opt };
} else {
  console.log('engine: crates/apz-engine-web is not there yet, skipped');
}
// CI sets APZ_REQUIRE_ENGINE=1: a console without the engine ships with no 3D, so that build must fail.
if (process.env.APZ_REQUIRE_ENGINE === '1' && !(engineJsName && engineWasmName)) {
  throw new Error('build: APZ_REQUIRE_ENGINE=1 but the wasm engine is not in dist (crates/apz-engine-web missing or not built)');
}

// The IDE model's bindings (crates/apz-ide-web), packed the same way and kept beside the engine in dist. Nothing loads
// them with the console: the loader (src/ide/wasm.ts) imports the glue on first use, so only its name is defined here.
let ideJsName = null;
let ideWasmName = null;
let ideSizes = null;
if (existsSync(join(repoRoot, 'crates/apz-ide-web/Cargo.toml'))) {
  const ide = packWeb('web-ide');
  ideJsName = ide.js;
  ideWasmName = ide.wasm;
  ideSizes = { bytes: ide.bytes, gzip: ide.gzip, brotli: ide.brotli, wasmOpt: ide.wasm_opt };
} else {
  console.log('ide: crates/apz-ide-web is not there yet, skipped');
}

// The IDE screen (views/ide.ts) is loaded on first visit, with CodeMirror: esbuild splits it into a chunk beside main
// (`splitting`, esm). Its stylesheet is not in the start CSS either: this plugin makes `styles/ide.css` a string the view
// inserts when it first opens. The IDE terminal (views/ide-terminal.ts) is in that chunk too, so its stylesheet and
// xterm's own are text the same way.
const ideStylesheetAsText = {
  name: 'ide-stylesheet-as-text',
  setup(b) {
    b.onLoad({ filter: /([\\/]styles[\\/]ide(-[a-z]+)*\.css|[\\/]@xterm[\\/]xterm[\\/]css[\\/]xterm\.css)$/ }, (args) => ({
      contents: readFileSync(args.path, 'utf8'),
      loader: 'text',
    }));
  },
};

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
  splitting: true,
  plugins: [ideStylesheetAsText],
  // The @font-face urls name the copied font files; leave them as written.
  external: ['*.ttf', '*.woff2'],
  // The engine's fingerprinted glue and wasm names, for the app to import the engine by.
  define: {
    __APZ_ENGINE_JS__: JSON.stringify(engineJsName),
    __APZ_ENGINE_WASM__: JSON.stringify(engineWasmName),
    __APZ_IDE_JS__: JSON.stringify(ideJsName),
  },
  entryNames: '[name]-[hash]',
  metafile: true,
  logLevel: 'silent',
});

let jsFile = null;
for (const [name, entry] of Object.entries(result.metafile.outputs)) {
  if (entry.entryPoint?.endsWith('main.ts')) jsFile = name;
}
// The start stylesheet is the main entry's own CSS bundle: esbuild names it on the entry's js output (`cssBundle`). Not the
// first CSS output: a lazily loaded chunk that imports CSS (the IDE chunk, through ui/markdown.ts) gets a CSS file of its own,
// and linking that one would leave the console unstyled. Fall back to the output named main-*.css; fail if neither exists.
let cssFile = (jsFile && result.metafile.outputs[jsFile].cssBundle) || null;
if (!cssFile) cssFile = Object.keys(result.metafile.outputs).find((n) => basename(n).startsWith('main-') && n.endsWith('.css')) ?? null;
if (!jsFile || !cssFile || !result.metafile.outputs[cssFile]) {
  console.error('build: expected the main entry js and its css bundle, got', Object.keys(result.metafile.outputs));
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

// Precompressed .br and .gz siblings of the js, css, wasm and html, for the hub to serve by Accept-Encoding
// (apz-pack compress, Rust). The .br files are committed with dist; the .gz ones are rebuilt (see .gitignore).
if (existsSync(join(repoRoot, 'tools/apz-pack/Cargo.toml'))) {
  execFileSync('cargo', ['run', '-q', '-p', 'apz-pack', '--release', '--', 'compress', dist], {
    cwd: repoRoot,
    stdio: ['ignore', 'ignore', 'inherit'],
  });
}

const old = join(root, 'dist.old');
rmSync(old, { recursive: true, force: true });
if (existsSync(live)) renameSync(live, old);
renameSync(dist, live);
rmSync(old, { recursive: true, force: true });

const kb = (n) => (n / 1024).toFixed(1) + ' KiB';
console.log(`dist/index.html  ${kb(html.length)}`);
console.log(`dist/${jsBase}  ${kb(readFileSync(join(live, jsBase)).length)}`);
console.log(`dist/${cssBase}  ${kb(readFileSync(join(live, cssBase)).length)}`);
// The chunks: the ones main imports load with the start payload (modules the console and the IDE share); the rest load on
// the IDE's first visit (CodeMirror and the IDE screen).
const startChunks = new Set(result.metafile.outputs[jsFile].imports.filter((i) => i.kind === 'import-statement').map((i) => basename(i.path)));
let startBytes = readFileSync(join(live, jsBase)).length;
for (const name of Object.keys(result.metafile.outputs)) {
  const base = basename(name);
  if (!name.endsWith('.js') || base === jsBase) continue;
  const size = readFileSync(join(live, base)).length;
  if (startChunks.has(base)) {
    startBytes += size;
    console.log(`dist/${base}  ${kb(size)}  (start payload)`);
  } else console.log(`dist/${base}  ${kb(size)}  (on first visit)`);
}
console.log(`start payload js  ${kb(startBytes)}`);
console.log(`bundle sha256:${bundleHash.slice(0, 12)}…`);
if (engineSizes) {
  const opt = { kept: 'wasm-opt -Oz', larger: 'wasm-opt dropped: it compressed larger', off: 'no wasm-opt' }[engineSizes.wasmOpt];
  console.log(`dist/${engineWasmName}  ${kb(engineSizes.bytes)}  br ${kb(engineSizes.brotli)}  gzip -9 ${kb(engineSizes.gzip)}  (${opt})`);
}
if (ideSizes) {
  const opt = { kept: 'wasm-opt -Oz', larger: 'wasm-opt dropped: it compressed larger', off: 'no wasm-opt' }[ideSizes.wasmOpt];
  console.log(`dist/${ideWasmName}  ${kb(ideSizes.bytes)}  br ${kb(ideSizes.brotli)}  gzip -9 ${kb(ideSizes.gzip)}  (${opt})`);
}
