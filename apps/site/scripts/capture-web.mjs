#!/usr/bin/env node
// Captures the console's screens for the site, through a headless Chromium driven over the
// DevTools protocol (no extra packages). Motion for the site comes from the desktop app itself.
//
//   node apps/site/scripts/capture-web.mjs --base http://127.0.0.1:7700 --out apps/site/assets \
//        [--screens floor,agents/209,board] [--wait 6] [--css '…']
//
// Screens are the console's hash routes (`#/floor`, `#/agents/209`). Point it at a plaza held by STOP ALL: it only reads.
// `--css` is injected into every page; the default hides the hold's banner and buttons, so the
// pictures show the console as it looks while the plaza runs.

import { spawn } from 'node:child_process';
import { mkdirSync, mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => (a.startsWith('--') ? [...acc, [a.slice(2), all[i + 1]]] : acc), []),
);
const BASE = args.base ?? 'http://127.0.0.1:7700';
const OUT = args.out ?? 'apps/site/assets';
const SCREENS = (args.screens ?? 'floor,agents,society,board,nodes').split(',').filter(Boolean);
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
  writeFileSync(join(OUT, `capture-${screen.replaceAll('/', '-')}.png`), Buffer.from(data, 'base64'));
  console.log(`capture-${screen.replaceAll('/', '-')}.png`);
}

ws.close();
const exited = new Promise((r) => chrome.once('exit', r));
chrome.kill();
await exited;
try { rmSync(profile, { recursive: true, force: true }); } catch {}
