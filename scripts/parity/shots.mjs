#!/usr/bin/env node
// Visual parity harness. For every theme, mode and screen in screens.json it captures the web console
// (headless Chromium, driven over the DevTools protocol) and the desktop app (its own APZ_SNAPSHOT frame),
// then compares the two with a perceptual diff (diff.mjs). Captures run one at a time.
//
// Usage:
//   node scripts/parity/shots.mjs --hub http://127.0.0.1:7700 --token-file <state>/owner-token --out <dir>
//     [--themes loom-clay,graphite-cobalt] [--modes light,dark] [--screens overview,agents]
//     [--max-ratio 0.08] [--binary <agentplaza-desktop>] [--screens-file <json>]
// Needs Node 22 or later (the global WebSocket), Chromium (CHROMIUM overrides /usr/bin/chromium) and,
// on Linux without a display, xvfb-run. Exits 1 when a screen's mismatch ratio is above --max-ratio
// or a capture fails. Never run this against the owner's own desktop session: the app snapshots itself.

import { spawn } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { diff } from './diff.mjs';
import { decode, encode } from './png.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, '..', '..');
const WIDTH = 1360;
const HEIGHT = 880;
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

function fail(message) {
  console.error(`shots: ${message}`);
  process.exit(2);
}

/** Parses `--key value` pairs; a bare `--flag` is true. */
function parseArgs(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i += 1) {
    const key = argv[i];
    if (!key.startsWith('--')) fail(`unexpected argument: ${key}`);
    const next = argv[i + 1];
    if (next === undefined || next.startsWith('--')) {
      out[key.slice(2)] = true;
    } else {
      out[key.slice(2)] = next;
      i += 1;
    }
  }
  return out;
}

/** A comma-separated option as a list, or null when it is absent. */
function listOf(value) {
  return typeof value === 'string' ? value.split(',').map((s) => s.trim()).filter(Boolean) : null;
}

/** The wanted names, checked against the known ones; all of them when none are wanted. */
function pick(wanted, known, what) {
  if (!wanted) return known;
  const missing = wanted.filter((name) => !known.includes(name));
  if (missing.length > 0) fail(`unknown ${what}: ${missing.join(', ')}`);
  return wanted;
}

/** Polls check() until it returns a value, or fails after ms. */
async function until(check, ms, what) {
  const end = Date.now() + ms;
  for (;;) {
    try {
      const value = await check();
      if (value) return value;
    } catch {
      // Not up yet: keep polling until the deadline.
    }
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(250);
  }
}

const args = parseArgs(process.argv.slice(2));
if (!args.hub || !args['token-file'] || !args.out) {
  fail('usage: shots.mjs --hub <url> --token-file <owner-token> --out <dir> [--themes ..] [--modes ..] [--screens ..]');
}
const hub = String(args.hub).replace(/\/+$/, '');
const token = readFileSync(String(args['token-file']), 'utf8').trim();
const outDir = resolve(String(args.out));
const maxRatio = Number(args['max-ratio'] ?? 0.08);
if (!Number.isFinite(maxRatio)) fail('--max-ratio must be a number');

const catalogue = JSON.parse(readFileSync(join(repo, 'apps/shared/themes.gen.json'), 'utf8'));
const builtins = catalogue.themes.map((t) => ({ id: t.id, skin: t.skin }));
const themes = pick(listOf(args.themes), builtins.map((t) => t.id), 'theme')
  .map((id) => builtins.find((t) => t.id === id));
const modes = pick(listOf(args.modes) ?? ['light', 'dark'], ['light', 'dark'], 'mode');
const allScreens = JSON.parse(readFileSync(String(args['screens-file'] ?? join(here, 'screens.json')), 'utf8'));
const screens = pick(listOf(args.screens), allScreens.map((s) => s.name), 'screen')
  .map((name) => allScreens.find((s) => s.name === name));

const targetDir = process.env.CARGO_TARGET_DIR ?? join(repo, 'target');
const binary = String(args.binary ?? [
  join(targetDir, 'release/agentplaza-desktop'),
  join(targetDir, 'debug/agentplaza-desktop'),
].find((path) => existsSync(path)) ?? '');
if (!binary || !existsSync(binary)) fail('no desktop binary: build agentplaza-desktop or pass --binary');

/** Trades the owner token for the session cookie the console uses (the token itself is not kept). */
async function signIn() {
  const res = await fetch(`${hub}/v1/auth/session`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ token }),
  });
  if (!res.ok) throw new Error(`sign-in to the hub failed (HTTP ${res.status})`);
  const cookies = res.headers.getSetCookie().map((line) => {
    const pair = line.split(';')[0];
    const eq = pair.indexOf('=');
    return { name: pair.slice(0, eq).trim(), value: pair.slice(eq + 1).trim() };
  });
  if (cookies.length === 0) throw new Error('the hub gave no session cookie');
  return cookies;
}

/** Starts headless Chromium with a DevTools port and returns its first page's socket URL. */
async function startChromium() {
  const bin = process.env.CHROMIUM ?? '/usr/bin/chromium';
  const profile = mkdtempSync(join(tmpdir(), 'apz-parity-'));
  const port = 9400 + Math.floor(Math.random() * 400);
  const proc = spawn(bin, [
    '--headless=new',
    '--no-sandbox',
    '--disable-gpu',
    '--hide-scrollbars',
    '--no-first-run',
    '--mute-audio',
    `--remote-debugging-port=${port}`,
    `--user-data-dir=${profile}`,
    `--window-size=${WIDTH},${HEIGHT}`,
    'about:blank',
  ], { stdio: 'ignore' });
  const page = await until(async () => {
    const tabs = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
    return tabs.find((tab) => tab.type === 'page' && tab.webSocketDebuggerUrl);
  }, 30000, 'Chromium');
  return { proc, profile, socketUrl: page.webSocketDebuggerUrl };
}

/** A minimal DevTools protocol client over Node's built-in WebSocket. */
class Cdp {
  constructor(url) {
    this.ws = new WebSocket(url);
    this.seq = 0;
    this.calls = new Map();
    this.waiters = [];
    this.ws.addEventListener('message', (ev) => {
      const msg = JSON.parse(String(ev.data));
      if (msg.id !== undefined) {
        const call = this.calls.get(msg.id);
        if (!call) return;
        this.calls.delete(msg.id);
        if (msg.error) call.reject(new Error(`${call.method}: ${msg.error.message}`));
        else call.resolve(msg.result ?? {});
      } else if (msg.method) {
        this.waiters = this.waiters.filter((waiter) => {
          if (waiter.method !== msg.method) return true;
          waiter.resolve(msg.params);
          return false;
        });
      }
    });
  }

  open() {
    return new Promise((ok, no) => {
      this.ws.addEventListener('open', ok, { once: true });
      this.ws.addEventListener('error', () => no(new Error('DevTools connection failed')), { once: true });
    });
  }

  send(method, params = {}) {
    const id = (this.seq += 1);
    return new Promise((resolveCall, rejectCall) => {
      this.calls.set(id, { method, resolve: resolveCall, reject: rejectCall });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }

  /** Resolves with the params of the next event called `method`; the caller registers before acting. */
  event(method, ms = 30000) {
    return new Promise((resolveEvent, rejectEvent) => {
      const timer = setTimeout(() => rejectEvent(new Error(`timed out waiting for ${method}`)), ms);
      this.waiters.push({
        method,
        resolve: (params) => {
          clearTimeout(timer);
          resolveEvent(params);
        },
      });
    });
  }

  close() {
    this.ws.close();
  }
}

/** The web console at this screen, in the look given, as a PNG. */
async function captureWeb(cdp, look, screen) {
  // A blank page first, so the console boots afresh with the device override in place.
  await cdp.send('Page.navigate', { url: 'about:blank' });
  await sleep(300);
  const { identifier } = await cdp.send('Page.addScriptToEvaluateOnNewDocument', {
    source: `try { localStorage.setItem('apz.appearance.device', ${JSON.stringify(JSON.stringify(look))}); } catch (e) {}`,
  });
  try {
    const loaded = cdp.event('Page.loadEventFired');
    loaded.catch(() => {});
    const nav = await cdp.send('Page.navigate', { url: `${hub}/#/${screen.web.replace(/^#\/?/, '')}` });
    if (nav.errorText) throw new Error(`navigation failed: ${nav.errorText}`);
    await loaded;
    await sleep(screen.wait_ms);
    const { data } = await cdp.send('Page.captureScreenshot', { format: 'png' });
    return Buffer.from(data, 'base64');
  } finally {
    await cdp.send('Page.removeScriptToEvaluateOnNewDocument', { identifier });
  }
}

/** The desktop app at this screen, in the look given, as the PNG its own snapshot writes. */
function captureDesktop(look, screen, outPng) {
  const afterSecs = Math.max(1, Math.round(screen.wait_ms / 1000));
  const timeoutMs = (afterSecs + 90) * 1000;
  const env = {
    ...process.env,
    APZ_CONNECT: hub,
    APZ_TOKEN: token,
    APZ_SCREEN: screen.desktop,
    APZ_APPEARANCE: JSON.stringify(look),
    APZ_SNAPSHOT: outPng,
    APZ_SNAPSHOT_AFTER: String(afterSecs),
    APZ_SNAPSHOT_SIZE: `${WIDTH}x${HEIGHT}`,
  };
  return new Promise((done, reject) => {
    const child = spawn(binary, [], { env, stdio: ['ignore', 'ignore', 'pipe'] });
    let stderr = '';
    let finished = false;
    let poll;
    let timer;
    const finish = (error) => {
      if (finished) return;
      finished = true;
      clearInterval(poll);
      clearTimeout(timer);
      if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
      if (error) reject(error);
      else done();
    };
    child.stderr.on('data', (chunk) => {
      stderr = (stderr + chunk).slice(-2000);
    });
    child.on('exit', (code) => {
      finish(new Error(`desktop exited (${code}) before its snapshot: ${stderr.trim().split('\n').slice(-3).join(' | ')}`));
    });
    // The app keeps running after it snapshots itself, so the file's arrival ends the capture.
    poll = setInterval(() => {
      if (!existsSync(outPng)) return;
      try {
        decode(readFileSync(outPng));
      } catch {
        return; // Still being written: try again.
      }
      finish(null);
    }, 500);
    timer = setTimeout(() => finish(new Error(`no snapshot after ${timeoutMs / 1000}s`)), timeoutMs);
  });
}

/** Captures one theme, mode and screen in both consoles and compares them. */
async function compare(cdp, theme, mode, screen) {
  const name = `${theme.id}-${mode}-${screen.name}`;
  const look = { theme: theme.id, skin: theme.skin, mode };
  const row = {
    theme: theme.id,
    mode,
    screen: screen.name,
    web_png: `${name}-web.png`,
    desktop_png: `${name}-desktop.png`,
    diff_png: `${name}-diff.png`,
  };
  console.log(`capturing ${name}`);
  try {
    const webBytes = await captureWeb(cdp, look, screen);
    writeFileSync(join(outDir, row.web_png), webBytes);
    const desktopPath = join(outDir, row.desktop_png);
    rmSync(desktopPath, { force: true });
    await captureDesktop(look, screen, desktopPath);
    const result = diff(decode(webBytes), decode(readFileSync(desktopPath)), { threshold: 0.1 });
    writeFileSync(join(outDir, row.diff_png), encode(result.image));
    return {
      ...row,
      sizes: result.sizes,
      mismatched: result.mismatched,
      total: result.total,
      ratio: result.ratio,
      status: result.ratio <= maxRatio ? 'pass' : 'fail',
    };
  } catch (error) {
    return { ...row, status: 'error', error: error.message };
  }
}

/** Writes report.json and report.md with every row so far (the run is saved even when it stops early). */
function writeReport(rows) {
  const failed = rows.filter((row) => row.status !== 'pass').length;
  writeFileSync(join(outDir, 'report.json'), JSON.stringify({
    hub,
    width: WIDTH,
    height: HEIGHT,
    max_ratio: maxRatio,
    failed,
    results: rows,
  }, null, 2));
  const lines = [
    '# Visual parity report',
    '',
    `Hub ${hub} at ${WIDTH}x${HEIGHT}. A screen fails above a mismatch ratio of ${maxRatio}.`,
    '',
    `${rows.length - failed} of ${rows.length} screens pass.`,
    '',
    '| Theme | Mode | Screen | Mismatched pixels | Ratio | Result |',
    '| --- | --- | --- | ---: | ---: | --- |',
  ];
  for (const row of rows) {
    const count = row.status === 'error' ? '-' : `${row.mismatched} of ${row.total}`;
    const ratio = row.status === 'error' ? '-' : row.ratio.toFixed(4);
    const result = row.status === 'error' ? `error: ${row.error.replace(/\|/g, '/')}` : row.status;
    lines.push(`| ${row.theme} | ${row.mode} | ${row.screen} | ${count} | ${ratio} | ${result} |`);
  }
  writeFileSync(join(outDir, 'report.md'), `${lines.join('\n')}\n`);
}

mkdirSync(outDir, { recursive: true });
const cookies = await signIn();
const chromium = await startChromium();
const cdp = new Cdp(chromium.socketUrl);
const rows = [];
try {
  await cdp.open();
  await cdp.send('Page.enable');
  await cdp.send('Network.enable');
  for (const cookie of cookies) {
    await cdp.send('Network.setCookie', { ...cookie, url: hub, path: '/' });
  }
  await cdp.send('Emulation.setDeviceMetricsOverride', {
    width: WIDTH,
    height: HEIGHT,
    deviceScaleFactor: 1,
    mobile: false,
  });
  for (const theme of themes) {
    for (const mode of modes) {
      for (const screen of screens) {
        rows.push(await compare(cdp, theme, mode, screen));
        writeReport(rows);
      }
    }
  }
} finally {
  cdp.close();
  chromium.proc.kill('SIGTERM');
  rmSync(chromium.profile, { recursive: true, force: true });
}

const failed = rows.filter((row) => row.status !== 'pass');
console.log(`${rows.length - failed.length} of ${rows.length} screens pass; report in ${join(outDir, 'report.md')}`);
process.exitCode = failed.length > 0 ? 1 : 0;
