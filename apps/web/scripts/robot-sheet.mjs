#!/usr/bin/env node
// Contact sheets for the robot species (assets/robots). For each species, one PNG with two rows (dark, light
// background) and six columns: front, q34, side, top (the four lab cameras), walk (iso) and tool (q34: the
// wrench in the grip, the species idle). Also an overview per background: every species' q34 view at the same scale.
//
// Usage: node apps/web/scripts/robot-sheet.mjs --out <dir> [--species id,id,...|all] [--chrome /usr/bin/chromium]
// Writes <out>/sheets/<id>.png, <out>/overview-dark.png and <out>/overview-light.png.
//
// How it works: this script serves apps/web/dist on a free local port and starts its own headless Chromium with a
// fresh profile (<out>/profile-<n>, removed at the end). It drives the robot lab (#/robot-lab) over CDP, one tile
// per URL, and screenshots the canvas. The lab's parameters that make the tiles: sheet=1 (the canvas alone), bg=dark
// or light, t=<seconds> (a frozen clock: the pose at that time), cam=front|q34|side|top|iso, state, clip, tool, hue.
//
// Frozen poses (the lab's t): an idle tile is taken at t = 1.3 s (a fixed time, not t = 0). A walk tile takes the
// clip walk.<locomotion> at half a cycle of its first looping channel (the first channel with a freq, resolved
// through the species' gait): t = (0.5 - phase/2π) / freq, with the phase wrapped into one cycle. A clip with no
// looping channel (such as wave, whose channels are constant offsets) has no cycle, so its pose is the same at any t.
//
// Hues: species are numbered in assets/robots/index.json order; hue = [25, 200, 290][index mod 3] (the same every run).
// Requires Node with a global WebSocket (Node 22+). No npm dependencies.

import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { createServer as createHttpServer } from 'node:http';
import { createServer as createNetServer } from 'node:net';
import { existsSync } from 'node:fs';
import { mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { dirname, extname, join, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { setTimeout as sleep } from 'node:timers/promises';

const HERE = dirname(fileURLToPath(import.meta.url));
const WEB = resolve(HERE, '..');
const DIST = join(WEB, 'dist');
const ROBOTS = resolve(WEB, '../../assets/robots');

const HUES = [25, 200, 290];
const IDLE_T = 1.3;
const TOOL = 'wrench';
const BGS = ['dark', 'light'];
const VIEWS = ['front', 'q34', 'side', 'top', 'walk', 'tool'];
const VIEW_LABEL = { front: 'front', q34: 'q34', side: 'side', top: 'top', walk: 'walk (iso)', tool: 'tool (q34)' };
const TILE = { w: 400, h: 520 };
// Sheet layout (CSS px at 1x): margins, the label column, tile size, gaps, caption and header heights.
const SHEET = { m: 20, lbl: 118, gap: 12, cap: 76, hdr: 30 };
const OVERVIEW = { scale: 0.6, cols: 6, gap: 14, m: 24, head: 60, label: 30 };
// Lab messages that mean a tile is wrong (not the app's own noise: its polls and offline states).
const PROBLEM = /no (?:body |pose )?clip|WebGL|THREE\.|shader|robot-lab|floor3d|robot\.ts/i;
const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
  '.ttf': 'font/ttf',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
  '.ico': 'image/x-icon',
  '.wasm': 'application/wasm',
};

function usage() {
  console.log('usage: node apps/web/scripts/robot-sheet.mjs --out <dir> [--species id,id,...|all] [--chrome /usr/bin/chromium]');
}

function parseArgs(argv) {
  const opts = { out: null, species: 'all', chrome: '/usr/bin/chromium' };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const value = () => {
      const v = argv[++i];
      if (v === undefined) throw new Error(`${a} needs a value`);
      return v;
    };
    if (a === '--out') opts.out = value();
    else if (a === '--species') opts.species = value();
    else if (a === '--chrome') opts.chrome = value();
    else if (a === '--help' || a === '-h') {
      usage();
      process.exit(0);
    } else throw new Error(`unknown argument ${a}`);
  }
  if (!opts.out) throw new Error('--out <dir> is required');
  return opts;
}

/** A number field, or the species' gait value for "gait.<name>" (as the web app resolves it). */
function gaitValue(v, gait, fallback = 0) {
  if (v === undefined) return fallback;
  if (typeof v === 'number') return v;
  const g = gait?.[v.slice(5)];
  return typeof g === 'number' ? g : fallback;
}

/** Half a cycle of a clip's first looping channel: the time at which that channel's wave is at c = 0.5. */
function halfCycle(clip, gait) {
  for (const ch of clip.channels ?? []) {
    if (ch.rate !== undefined) continue;
    const freq = gaitValue(ch.freq, gait, 0);
    if (freq > 0) {
      const c0 = ((((0.5 - (ch.phase ?? 0) / (2 * Math.PI)) % 1) + 1) % 1);
      return { seconds: c0 / freq, channel: `${ch.bone} ${ch.prop}`, freq };
    }
  }
  return null;
}

/** A free TCP port on 127.0.0.1 (bound and released once). */
function freePort() {
  return new Promise((res, rej) => {
    const srv = createNetServer();
    srv.on('error', rej);
    srv.listen(0, '127.0.0.1', () => {
      const { port } = srv.address();
      srv.close(() => res(port));
    });
  });
}

/** A static server for the built app (dist): the hash router needs only index.html and the bundles. */
function startStatic(root, port) {
  const srv = createHttpServer(async (req, res) => {
    try {
      const url = new URL(req.url ?? '/', 'http://127.0.0.1');
      let rel = decodeURIComponent(url.pathname);
      if (rel === '/' || rel === '') rel = '/index.html';
      const file = resolve(root, `.${rel}`);
      if (!file.startsWith(root + sep)) {
        res.writeHead(403).end();
        return;
      }
      const body = await readFile(file);
      res.writeHead(200, { 'content-type': MIME[extname(file)] ?? 'application/octet-stream', 'cache-control': 'no-store' });
      res.end(body);
    } catch {
      res.writeHead(404).end('not found');
    }
  });
  return new Promise((res, rej) => {
    srv.on('error', rej);
    srv.listen(port, '127.0.0.1', () => res(srv));
  });
}

function startChrome(bin, port, profileDir, extraFlags) {
  const args = [
    '--headless=new',
    '--use-angle=gl-egl',
    `--remote-debugging-port=${port}`,
    `--user-data-dir=${profileDir}`,
    '--no-first-run',
    '--no-default-browser-check',
    '--disable-extensions',
    '--disable-background-networking',
    '--hide-scrollbars',
    '--mute-audio',
    `--window-size=${TILE.w},${TILE.h}`,
    ...extraFlags,
    'about:blank',
  ];
  const proc = spawn(bin, args, { stdio: ['ignore', 'ignore', 'pipe'] });
  let stderr = '';
  proc.stderr.on('data', (d) => {
    if (stderr.length < 20000) stderr += d.toString();
  });
  return { proc, stderr: () => stderr.slice(-1500) };
}

async function waitForDevtools(port, proc) {
  const end = Date.now() + 30000;
  while (Date.now() < end) {
    if (proc.exitCode !== null) throw new Error(`chromium exited with code ${proc.exitCode}`);
    try {
      const r = await fetch(`http://127.0.0.1:${port}/json/version`);
      if (r.ok) return;
    } catch {
      // not up yet
    }
    await sleep(200);
  }
  throw new Error('chromium did not open its DevTools port');
}

async function pageTarget(port) {
  const end = Date.now() + 10000;
  while (Date.now() < end) {
    const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
    const page = list.find((t) => t.type === 'page' && t.webSocketDebuggerUrl);
    if (page) return page;
    await sleep(200);
  }
  throw new Error('no page target');
}

/** A minimal CDP client over the global WebSocket: send(method, params), and a listener for events. */
function connectCdp(url) {
  return new Promise((ok, fail) => {
    const ws = new WebSocket(url);
    const pending = new Map();
    const listeners = new Set();
    let seq = 0;
    const api = {
      send(method, params = {}) {
        return new Promise((res, rej) => {
          const id = ++seq;
          pending.set(id, { res, rej, method });
          ws.send(JSON.stringify({ id, method, params }));
        });
      },
      on(fn) {
        listeners.add(fn);
      },
      close() {
        ws.close();
      },
    };
    ws.onmessage = (ev) => {
      const msg = JSON.parse(String(ev.data));
      if (msg.id !== undefined) {
        const p = pending.get(msg.id);
        if (!p) return;
        pending.delete(msg.id);
        if (msg.error) p.rej(new Error(`${p.method}: ${msg.error.message}`));
        else p.res(msg.result);
      } else {
        for (const fn of listeners) fn(msg);
      }
    };
    ws.onopen = () => ok(api);
    ws.onerror = () => fail(new Error('websocket to chromium failed'));
  });
}

async function withTimeout(promise, ms, label) {
  let timer;
  const limit = new Promise((_, rej) => {
    timer = setTimeout(() => rej(new Error(`${label} timed out after ${ms} ms`)), ms);
  });
  try {
    return await Promise.race([promise, limit]);
  } finally {
    clearTimeout(timer);
  }
}

async function evaluate(cdp, expression, awaitPromise = false) {
  const r = await cdp.send('Runtime.evaluate', { expression, awaitPromise, returnByValue: true });
  if (r.exceptionDetails) {
    const d = r.exceptionDetails;
    throw new Error(`evaluate: ${d.exception?.description ?? d.text}`.split('\n')[0]);
  }
  return r.result.value;
}

/** One tile: point the lab at the URL (a hash change: the lab takes the state), wait for two frames, screenshot. */
async function captureTile(cdp, hash) {
  await evaluate(cdp, `(location.hash = ${JSON.stringify(hash)}, true)`);
  await evaluate(
    cdp,
    'new Promise((r) => setTimeout(() => requestAnimationFrame(() => requestAnimationFrame(() => r(true))), 60))',
    true,
  );
  const shot = await cdp.send('Page.captureScreenshot', { format: 'png', fromSurface: true });
  return shot.data;
}

/** The PNG's width and height, read from its IHDR chunk. */
function pngSize(b64) {
  const head = Buffer.from(b64.slice(0, 48), 'base64');
  return { w: head.readUInt32BE(16), h: head.readUInt32BE(20) };
}

function planTile(job, view, bg) {
  const q = new URLSearchParams({ species: job.id, hue: String(job.hue), bg, sheet: '1' });
  switch (view) {
    case 'front':
    case 'q34':
    case 'side':
    case 'top':
      q.set('cam', view);
      q.set('state', 'idle');
      q.set('t', String(IDLE_T));
      break;
    case 'walk':
      if (!job.clipOk) return { error: `no clip ${job.clipName} for ${job.id}` };
      q.set('cam', 'iso');
      q.set('state', 'walking');
      q.set('clip', job.clipName);
      q.set('t', String(job.walkT));
      break;
    case 'tool':
      q.set('cam', 'q34');
      q.set('state', 'idle');
      q.set('tool', TOOL);
      q.set('t', String(IDLE_T));
      break;
  }
  return { hash: `#/robot-lab?${q.toString()}` };
}

/** Runs in the page: draws the ops on one canvas and returns it as a PNG data URL. Self-contained (no closures). */
async function composeInPage(spec) {
  const load = (src) =>
    new Promise((ok, fail) => {
      const im = new Image();
      im.onload = () => ok(im);
      im.onerror = () => fail(new Error('a tile did not decode'));
      im.src = src;
    });
  const cv = document.createElement('canvas');
  cv.width = spec.width;
  cv.height = spec.height;
  const g = cv.getContext('2d');
  g.textBaseline = 'middle';
  for (const op of spec.ops) {
    if (op.op === 'rect') {
      g.fillStyle = op.fill;
      g.fillRect(op.x, op.y, op.w, op.h);
    } else if (op.op === 'img') {
      g.drawImage(await load(op.src), op.x, op.y, op.w, op.h);
    } else if (op.op === 'text') {
      g.font = `${op.weight ?? 400} ${op.size}px sans-serif`;
      g.fillStyle = op.colour;
      g.textAlign = op.align ?? 'left';
      g.fillText(op.text, op.x, op.y);
    }
  }
  return cv.toDataURL('image/png');
}

async function compose(cdp, spec) {
  const url = await evaluate(cdp, `(${composeInPage.toString()})(${JSON.stringify(spec)})`, true);
  if (typeof url !== 'string' || !url.startsWith('data:image/png;base64,')) throw new Error('compose returned no PNG');
  return Buffer.from(url.slice('data:image/png;base64,'.length), 'base64');
}

function wrapText(text, width) {
  const lines = [];
  let line = '';
  for (const word of text.split(/\s+/)) {
    if ((line + ' ' + word).trim().length > width) {
      if (line) lines.push(line);
      line = word;
    } else {
      line = (line + ' ' + word).trim();
    }
  }
  if (line) lines.push(line);
  return lines;
}

/** A grey cell with the reason, where a tile did not render. */
function placeholder(x, y, w, h, reason, palette) {
  const ops = [{ op: 'rect', x, y, w, h, fill: palette.rule }];
  wrapText(`not rendered: ${reason}`, 40)
    .slice(0, 6)
    .forEach((ln, i) => ops.push({ op: 'text', x: x + 12, y: y + 24 + i * 22, text: ln, size: 16, colour: palette.ink }));
  return ops;
}

function cellOps(cell, x, y, w, h, palette) {
  if (cell?.src) return [{ op: 'img', src: cell.src, x, y, w, h }];
  return placeholder(x, y, w, h, cell?.error ?? 'missing', palette);
}

const TILE_W = TILE.w;
const TILE_H = TILE.h;

/** One species: a row per background, a column per view, a caption on top. */
function sheetSpec(job, cells, palette) {
  const { m, lbl, gap, cap, hdr } = SHEET;
  const width = m + lbl + VIEWS.length * TILE_W + (VIEWS.length - 1) * gap + m;
  const height = m + cap + hdr + BGS.length * TILE_H + (BGS.length - 1) * gap + m;
  const ops = [{ op: 'rect', x: 0, y: 0, w: width, h: height, fill: palette.paper }];
  ops.push({ op: 'text', x: m, y: m + 22, text: job.id, size: 30, weight: 700, colour: palette.ink });
  const note = job.walkNote ? ` (${job.walkNote})` : '';
  ops.push({
    op: 'text',
    x: m,
    y: m + 56,
    text: `${job.name} · locomotion ${job.locomotion} · walk ${job.clipName} at t = ${job.walkT} s${note} · hue ${job.hue}`,
    size: 17,
    colour: palette.inkMuted,
  });
  VIEWS.forEach((view, c) => {
    ops.push({
      op: 'text',
      x: m + lbl + c * (TILE_W + gap) + TILE_W / 2,
      y: m + cap + hdr / 2,
      text: VIEW_LABEL[view],
      size: 17,
      weight: 600,
      colour: palette.ink,
      align: 'center',
    });
  });
  BGS.forEach((bg, r) => {
    const rowTop = m + cap + hdr + r * (TILE_H + gap);
    ops.push({ op: 'text', x: m, y: rowTop + TILE_H / 2 - 12, text: bg, size: 20, weight: 700, colour: palette.ink });
    ops.push({
      op: 'text',
      x: m,
      y: rowTop + TILE_H / 2 + 14,
      text: bg === 'dark' ? '#1b1a17' : '#f3efe6',
      size: 14,
      colour: palette.inkMuted,
    });
    VIEWS.forEach((view, c) => {
      const x = m + lbl + c * (TILE_W + gap);
      ops.push(...cellOps(cells[bg][view], x, rowTop, TILE_W, TILE_H, palette));
    });
  });
  return { width, height, ops };
}

/** The overview of one background: every species' q34 view, the same scale, the id under each. */
function overviewSpec(jobs, cellsById, bg, palette) {
  const { scale, cols, gap, m, head, label } = OVERVIEW;
  const tw = Math.round(TILE_W * scale);
  const th = Math.round(TILE_H * scale);
  const rows = Math.ceil(jobs.length / cols);
  const width = m + cols * tw + (cols - 1) * gap + m;
  const height = m + head + rows * (th + label) + (rows - 1) * gap + m;
  const ops = [{ op: 'rect', x: 0, y: 0, w: width, h: height, fill: palette.paper }];
  ops.push({
    op: 'text',
    x: m,
    y: m + 22,
    text: `Robot species, q34 view, ${bg} background (one scale, one camera)`,
    size: 22,
    weight: 700,
    colour: palette.ink,
  });
  ops.push({
    op: 'text',
    x: m,
    y: m + 46,
    text: 'hue = [25, 200, 290] by index mod 3 in index.json order',
    size: 15,
    colour: palette.inkMuted,
  });
  jobs.forEach((job, i) => {
    const col = i % cols;
    const row = Math.floor(i / cols);
    const x = m + col * (tw + gap);
    const y = m + head + row * (th + label + gap);
    ops.push(...cellOps(cellsById[job.id][bg].q34, x, y, tw, th, palette));
    ops.push({ op: 'text', x: x + tw / 2, y: y + th + label / 2, text: job.id, size: 17, weight: 700, colour: palette.ink, align: 'center' });
  });
  return { width, height, ops };
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  const started = Date.now();
  const outRoot = resolve(opts.out);
  const sheetsDir = join(outRoot, 'sheets');
  await mkdir(sheetsDir, { recursive: true });

  const index = JSON.parse(await readFile(join(ROBOTS, 'index.json'), 'utf8'));
  const palette = JSON.parse(await readFile(join(ROBOTS, 'palette.json'), 'utf8'));
  const shared = JSON.parse(await readFile(join(ROBOTS, 'clips.json'), 'utf8')).clips;
  const tool = JSON.parse(await readFile(join(ROBOTS, 'tools', `${TOOL}.json`), 'utf8'));
  if (tool.grip !== 'one') throw new Error(`${TOOL} is not a one-handed tool (grip ${tool.grip})`);

  const wanted = opts.species === 'all' ? index.species : opts.species.split(',').map((s) => s.trim()).filter(Boolean);
  for (const id of wanted) if (!index.species.includes(id)) throw new Error(`unknown species ${id}`);

  const jobs = [];
  for (const id of wanted) {
    const sp = JSON.parse(await readFile(join(ROBOTS, 'species', `${id}.json`), 'utf8'));
    const hue = HUES[index.species.indexOf(id) % HUES.length];
    const clipName = `walk.${sp.locomotion}`;
    const clip = sp.clips?.[clipName] ?? shared[clipName];
    const walk = clip ? halfCycle(clip, sp.gait ?? {}) : null;
    jobs.push({
      id,
      name: sp.name ?? id,
      locomotion: sp.locomotion,
      hue,
      clipName,
      clipOk: Boolean(clip),
      walkT: walk ? Number(walk.seconds.toFixed(4)) : 0,
      walkNote: walk ? `${walk.channel} at ${walk.freq} Hz` : 'no looping channel, t = 0',
    });
  }

  const port = await freePort();
  const serverPort = await freePort();
  let server = null;
  let browser = null;
  const profiles = [];
  const logs = [];
  const failures = [];
  let otherErrors = 0;
  const cellsById = {};
  try {
    server = await startStatic(DIST, serverPort);
    const attempts = [[], ['--enable-unsafe-swiftshader', '--use-angle=swiftshader']];
    const firstHash = `#/robot-lab?species=${wanted[0]}&hue=${jobs[0].hue}&bg=dark&sheet=1&cam=front&state=idle&t=${IDLE_T}`;
    for (const [n, extra] of attempts.entries()) {
      const profileDir = nextProfile(outRoot, profiles);
      profiles.push(profileDir);
      const chrome = startChrome(opts.chrome, port, profileDir, extra);
      await waitForDevtools(port, chrome.proc);
      const target = await pageTarget(port);
      const cdp = await connectCdp(target.webSocketDebuggerUrl);
      cdp.on((msg) => {
        if (logs.length >= 5000) return;
        if (msg.method === 'Runtime.consoleAPICalled') {
          const text = msg.params.args.map((a) => (a.value !== undefined ? String(a.value) : a.description ?? a.type)).join(' ');
          logs.push({ level: msg.params.type, text: text.slice(0, 300) });
        } else if (msg.method === 'Runtime.exceptionThrown') {
          const d = msg.params.exceptionDetails;
          logs.push({ level: 'exception', text: String(d.exception?.description ?? d.text ?? 'exception').split('\n')[0].slice(0, 300) });
        }
      });
      await cdp.send('Page.enable');
      await cdp.send('Runtime.enable');
      await cdp.send('Emulation.setDeviceMetricsOverride', { width: TILE.w, height: TILE.h, deviceScaleFactor: 1, mobile: false });
      browser = { proc: chrome.proc, cdp, stderr: chrome.stderr };
      await cdp.send('Page.navigate', { url: `http://127.0.0.1:${serverPort}/index.html${firstHash}` });
      const ready = await waitForLab(cdp, 60000);
      if (ready === 'canvas') break;
      await closeBrowser(browser);
      browser = null;
      if (n === attempts.length - 1) {
        throw new Error(`the robot lab did not render (${ready || 'no canvas'}); chromium said: ${chrome.stderr()}`);
      }
      console.log(`no WebGL with the default flags (${ready || 'no canvas'}): retrying with SwiftShader`);
    }

    const cdp = browser.cdp;
    for (const job of jobs) {
      const t0 = Date.now();
      const cells = { dark: {}, light: {} };
      let ok = 0;
      for (const bg of BGS) {
        for (const view of VIEWS) {
          const plan = planTile(job, view, bg);
          const mark = logs.length;
          let cell;
          try {
            if (plan.error) throw new Error(plan.error);
            const b64 = await withTimeout(captureTile(cdp, plan.hash), 45000, `${job.id} ${bg} ${view}`);
            const size = pngSize(b64);
            if (size.w !== TILE.w || size.h !== TILE.h) throw new Error(`tile is ${size.w}x${size.h}, not ${TILE.w}x${TILE.h}`);
            const problem = logs.slice(mark).find((l) => PROBLEM.test(l.text) && ['error', 'exception', 'warning'].includes(l.level));
            if (problem) throw new Error(`lab said: ${problem.text}`);
            cell = { src: `data:image/png;base64,${b64}` };
            ok++;
          } catch (err) {
            cell = { error: String(err.message ?? err).slice(0, 200) };
            failures.push(`${job.id} ${bg} ${view}: ${cell.error}`);
          }
          otherErrors += logs.slice(mark).filter((l) => ['error', 'exception'].includes(l.level) && !PROBLEM.test(l.text)).length;
          cells[bg][view] = cell;
        }
      }
      cellsById[job.id] = cells;
      const png = await compose(cdp, sheetSpec(job, cells, palette));
      const file = join(sheetsDir, `${job.id}.png`);
      await writeFile(file, png);
      console.log(`${job.id} (${job.name}, hue ${job.hue}): ${ok}/${BGS.length * VIEWS.length} tiles, ${((Date.now() - t0) / 1000).toFixed(1)} s -> ${file}`);
    }

    for (const bg of BGS) {
      const png = await compose(cdp, overviewSpec(jobs, cellsById, bg, palette));
      const file = join(outRoot, `overview-${bg}.png`);
      await writeFile(file, png);
      console.log(`overview ${bg}: ${jobs.length} species -> ${file}`);
    }
  } finally {
    await closeBrowser(browser);
    server?.close();
    // Chromium's child processes can still be writing for a moment after the main process exits: retry the removal.
    for (const dir of profiles) {
      for (let tries = 0; tries < 10 && existsSync(dir); tries++) {
        await rm(dir, { recursive: true, force: true }).catch(() => {});
        if (existsSync(dir)) await sleep(300);
      }
    }
  }

  if (failures.length) {
    console.log(`\n${failures.length} tile(s) did not render:`);
    for (const f of failures) console.log(`  ${f}`);
  } else {
    console.log('\nall tiles rendered');
  }
  if (otherErrors) console.log(`(${otherErrors} other console error(s) from the app, not counted as tile failures)`);
  console.log(`done in ${((Date.now() - started) / 1000).toFixed(1)} s`);
}

/** A fresh profile directory under the output root: the first profile-<n> not on disk and not already used. */
function nextProfile(outRoot, used) {
  let n = 1;
  while (existsSync(join(outRoot, `profile-${n}`)) || used.includes(join(outRoot, `profile-${n}`))) n++;
  return join(outRoot, `profile-${n}`);
}

/** Waits for the lab's canvas (or its no-WebGL message): 'canvas', 'nowebgl' or '' on timeout. */
async function waitForLab(cdp, ms) {
  const end = Date.now() + ms;
  const probe =
    "(() => { if (document.querySelector('.view canvas')) return 'canvas'; const v = document.querySelector('.view'); return v && v.textContent.includes('WebGL is not available') ? 'nowebgl' : ''; })()";
  while (Date.now() < end) {
    const state = await evaluate(cdp, probe).catch(() => '');
    if (state) return state;
    await sleep(300);
  }
  return '';
}

async function closeBrowser(b) {
  if (!b) return;
  try {
    b.cdp.close();
  } catch {
    // already closed
  }
  if (b.proc.exitCode === null) {
    b.proc.kill('SIGTERM');
    await Promise.race([once(b.proc, 'exit'), sleep(3000)]);
    if (b.proc.exitCode === null) b.proc.kill('SIGKILL');
  }
}

main().catch((err) => {
  console.error(`robot-sheet: ${err.message}`);
  process.exitCode = 1;
});
