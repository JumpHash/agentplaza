#!/usr/bin/env node
// Captures the console for the site: screenshots of screens and a short recording of the floor,
// through a headless Chromium driven over the DevTools protocol (no extra packages).
//
//   node apps/site/scripts/capture-web.mjs --base http://127.0.0.1:7700 --out apps/site/assets \
//        [--screens floor,agents,society,board,nodes] [--record floor:12] [--wait 6] [--css '…']
//
// Screens are the console's hash routes (`#/floor`). The recording is rendered frame by frame on
// virtual time and encoded by ffmpeg to VP9 WebM. Point it at a plaza held by STOP ALL: it only reads.
// `--css` is injected into every page; the default hides the hold's banner and buttons, so the
// pictures show the console as it looks while the plaza runs.

import { spawn, spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => (a.startsWith('--') ? [...acc, [a.slice(2), all[i + 1]]] : acc), []),
);
const BASE = args.base ?? 'http://127.0.0.1:7700';
const OUT = args.out ?? 'apps/site/assets';
const SCREENS = (args.screens ?? 'floor,agents,society,board,nodes').split(',').filter(Boolean);
const RECORD = args.record ?? 'floor:12';
const WAIT = Number(args.wait ?? 6);
const CSS = args.css ?? '.held-banner, .plaza-control { display: none !important; }';
const W = 1600, H = 1000, SCALE = 2;
const PORT = 9333;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
mkdirSync(OUT, { recursive: true });

const profile = mkdtempSync(join(tmpdir(), 'apz-capture-'));
const chrome = spawn('chromium', [
  '--headless=new', '--no-sandbox', '--hide-scrollbars', '--mute-audio', '--no-first-run',
  `--remote-debugging-port=${PORT}`, `--user-data-dir=${profile}`,
  `--window-size=${W},${H}`, '--enable-unsafe-swiftshader', '--ignore-gpu-blocklist',
  // Extra Chromium flags, if any (APZ_CHROME_FLAGS).
  ...(process.env.APZ_CHROME_FLAGS ?? '').split(' ').filter(Boolean),
  'about:blank',
], { stdio: 'ignore' });

async function target() {
  for (let i = 0; i < 50; i++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
      const page = list.find((t) => t.type === 'page');
      if (page) return page.webSocketDebuggerUrl;
    } catch {}
    await sleep(200);
  }
  throw new Error('chromium did not start');
}

const ws = new WebSocket(await target());
await new Promise((r) => ws.addEventListener('open', r, { once: true }));
let seq = 0;
const pending = new Map();
const listeners = [];
ws.addEventListener('message', (ev) => {
  const msg = JSON.parse(ev.data);
  if (msg.id && pending.has(msg.id)) {
    const { resolve, reject } = pending.get(msg.id);
    pending.delete(msg.id);
    msg.error ? reject(new Error(msg.error.message)) : resolve(msg.result);
  } else if (msg.method) {
    for (const l of listeners) l(msg);
  }
});
const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = ++seq;
    pending.set(id, { resolve, reject });
    ws.send(JSON.stringify({ id, method, params }));
  });

await send('Page.enable');
await send('Emulation.setDeviceMetricsOverride', { width: W, height: H, deviceScaleFactor: SCALE, mobile: false });

async function open(screen) {
  await send('Page.navigate', { url: `${BASE}/#/${screen}` });
  await sleep(1500);
  await send('Runtime.evaluate', { expression: `document.head.insertAdjacentHTML('beforeend', ${JSON.stringify(`<style>${CSS}</style>`)})` });
  await sleep(WAIT * 1000);
}

for (const screen of SCREENS) {
  await open(screen);
  const { data } = await send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
  writeFileSync(join(OUT, `capture-${screen}.png`), Buffer.from(data, 'base64'));
  console.log(`capture-${screen}.png`);
}

if (RECORD) {
  // Frame by frame on virtual time: the page's clock advances exactly 1/FPS per frame and each
  // frame is captured whole, so a slow (software) renderer still yields a smooth, real-time video.
  const [screen, secs] = RECORD.split(':');
  const FPS = 30;
  const total = Math.round(Number(secs || 12) * FPS);
  const frames = mkdtempSync(join(tmpdir(), 'apz-frames-'));
  await open(screen);
  await send('Emulation.setVirtualTimePolicy', { policy: 'pause' });
  for (let n = 0; n < total; n++) {
    const expired = new Promise((r) => {
      const l = (msg) => {
        if (msg.method === 'Emulation.virtualTimeBudgetExpired') {
          listeners.splice(listeners.indexOf(l), 1);
          r();
        }
      };
      listeners.push(l);
    });
    await send('Emulation.setVirtualTimePolicy', { policy: 'advance', budget: 1000 / FPS });
    await expired;
    const { data } = await send('Page.captureScreenshot', { format: 'jpeg', quality: 94 });
    writeFileSync(join(frames, `f${String(n).padStart(5, '0')}.jpg`), Buffer.from(data, 'base64'));
    if (n % FPS === 0) process.stdout.write(`\r  frame ${n}/${total}`);
  }
  process.stdout.write('\n');
  const out = join(OUT, `capture-${screen}.webm`);
  const enc = spawnSync('ffmpeg', ['-y', '-loglevel', 'error', '-framerate', String(FPS), '-i', join(frames, 'f%05d.jpg'),
    '-vf', `scale=${W}:-2:flags=lanczos`, '-c:v', 'libvpx-vp9', '-b:v', '0', '-crf', '33', '-row-mt', '1', '-pix_fmt', 'yuv420p', '-an', out], { stdio: 'inherit' });
  rmSync(frames, { recursive: true, force: true });
  console.log(enc.status === 0 ? `capture-${screen}.webm (${total} frames, ${FPS} fps)` : 'ffmpeg failed');
}

ws.close();
const exited = new Promise((r) => chrome.once('exit', r));
chrome.kill();
await exited;
try { rmSync(profile, { recursive: true, force: true }); } catch {}
