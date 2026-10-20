// A live HTML artifact in the web console. The card shows a still (the hub's picture, or a quiet panel) and a Run
// button. Run turns the card into a sandboxed frame that loads the artifact's page. The frame is an opaque origin: the
// sandbox never includes allow-same-origin, so the page reaches the console only through the bridge's messages
// (content_bridge.js). This file accepts a message only from a live frame's own window and only with the apz:1 tag.
// At most LIVE_LIMIT frames run across the page: the most recently focused or opened ones. Opening one more turns the
// least recent back into its still.

import '../styles/view-frame.css';
import { h } from '../ui/dom';
import { TOKENS, tokenValue } from '../ui/theme';
import type { HtmlBody } from './model';

/** What a live frame needs besides its body: which artifact and panel it belongs to, and how it is named. */
export interface FrameOpts {
  artifactId: string;
  panelId: string;
  title?: string;
  stillUrl?: string;
}

const LIVE_LIMIT = 2;
const MIN_HEIGHT = 120;
const MAX_HEIGHT = 2000;
const DEFAULT_HEIGHT = 480;
/** The sandbox a frame runs in. Never allow-same-origin: the page is an opaque origin. */
const SANDBOX = 'allow-scripts allow-popups allow-downloads';

/** The handles the manager and the bridge messages need from one card. */
interface Card {
  el: HTMLElement;
  /** The running frame, or null while the card shows its still. */
  iframe: HTMLIFrameElement | null;
  /** Orders the live cards: the larger the stamp, the more recent the use. */
  stamp: number;
  /** Turns a live card back into its still. */
  stop: () => void;
  sendTheme: () => void;
  setHeight: (px: number) => void;
  askOpen: (raw: string) => void;
  askSave: (name: string, mime: string, data: string) => void;
}

/** The cards whose frames are running. Never more than LIVE_LIMIT. */
const live: Card[] = [];
let clock = 0;
let listening = false;

/** How many frames are running across the page. Tests read it. */
export function liveCount(): number {
  return live.length;
}

function clampHeight(px: number): number {
  if (!Number.isFinite(px)) return DEFAULT_HEIGHT;
  return Math.min(MAX_HEIGHT, Math.max(MIN_HEIGHT, Math.round(px)));
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

/** The URL when it is an http or https address, else null. */
function webUrl(raw: string): URL | null {
  try {
    const url = new URL(raw);
    return url.protocol === 'http:' || url.protocol === 'https:' ? url : null;
  } catch {
    return null;
  }
}

/** A file name the page asked for, with path separators and control characters removed. */
function fileName(name: string): string {
  const clean = name.replace(/[\\/:*?"<>|\u0000-\u001f]/g, '_').trim().slice(0, 120);
  return clean === '' ? 'download' : clean;
}

/** The theme the frame is given: every built-in token, the two fonts, and the mode in force. */
function themeVars(): Record<string, string> {
  const vars: Record<string, string> = {};
  const put = (name: string): void => {
    const value = tokenValue(name);
    if (value !== '') vars[`--${name}`] = value;
  };
  for (const token of TOKENS) put(token.name);
  put('font');
  put('font-mono');
  return vars;
}

/** Saves text the page made. Markup and script types are saved as opaque bytes, so a saved file never runs as a page. */
function saveText(name: string, mime: string, text: string): void {
  const type = mime === '' || /html|svg|xml|javascript/i.test(mime) ? 'application/octet-stream' : mime;
  const href = URL.createObjectURL(new Blob([text], { type }));
  const link = h('a', { href, download: name });
  document.body.append(link);
  link.click();
  link.remove();
  window.setTimeout(() => URL.revokeObjectURL(href), 1000);
}

function touch(card: Card): void {
  card.stamp = ++clock;
}

/** Makes a card's frame live, and turns the least recent other live frames back into stills. */
function admit(card: Card): void {
  touch(card);
  if (!live.includes(card)) live.push(card);
  while (live.length > LIVE_LIMIT) {
    let least: Card | undefined;
    for (const other of live) {
      if (other !== card && (least === undefined || other.stamp < least.stamp)) least = other;
    }
    if (least === undefined) break;
    live.splice(live.indexOf(least), 1);
    least.stop();
  }
}

/** One message from a frame. Only a live frame's own window is heard. */
function onMessage(event: MessageEvent): void {
  const data: unknown = event.data;
  if (!isRecord(data) || data.apz !== 1) return;
  const card = live.find((c) => c.iframe !== null && c.iframe.contentWindow === event.source);
  if (card === undefined) return;
  if (data.type === 'ready') {
    card.sendTheme();
  } else if (data.type === 'height' && typeof data.height === 'number') {
    card.setHeight(data.height);
  } else if (data.type === 'open' && typeof data.url === 'string') {
    card.askOpen(data.url);
  } else if (
    data.type === 'download' &&
    typeof data.name === 'string' &&
    typeof data.mime === 'string' &&
    typeof data.data === 'string'
  ) {
    card.askSave(data.name, data.mime, data.data);
  }
}

/** Registers the page-wide listeners once: messages from frames, and the appearance changes. */
function listen(): void {
  if (listening) return;
  listening = true;
  window.addEventListener('message', onMessage);
  window.addEventListener('apz:appearance', () => {
    for (const card of live) card.sendTheme();
  });
}

/**
 * A still card that becomes a live, sandboxed frame on Run. `url` gives the frame's address: the owner's capability
 * URL, or a share's relative frame path. The owner's absolute URL also gets an "Open in a new tab" link.
 */
export function liveFrame(url: () => Promise<string>, body: HtmlBody, opts: FrameOpts): HTMLElement {
  listen();
  const title = opts.title ?? body.entry;
  const startHeight = clampHeight(body.height);
  let busy = false;
  let pending: (() => void) | null = null;

  // The address is asked for once, when first needed. A failed ask is retried on the next need.
  let address: Promise<string> | null = null;
  const frameUrl = (): Promise<string> => {
    if (address === null) {
      address = url().catch((err: unknown) => {
        address = null;
        throw err;
      });
    }
    return address;
  };

  // The still: the hub's picture when there is one, else a quiet panel naming the entry, its libraries and its size.
  const panel = h(
    'div.vf-panel',
    null,
    h('div.vf-entry', null, body.entry),
    h('div.vf-libs', null, ...(body.libs ?? []).map((lib) => h('span.chip', null, lib))),
    h('div.vf-size', null, `${startHeight} px tall`),
  );
  const face: HTMLElement = opts.stillUrl
    ? h('img.vf-still', { src: opts.stillUrl, alt: title, loading: 'lazy' })
    : panel;
  const stage = h('div.vf-stage', null, face);

  // The confirm bar answers the frame's requests to open a link or save a file. One question at a time.
  const question = h('span.vf-q', { role: 'status' });
  const yes = h('button.btn.sm', { type: 'button' });
  const no = h('button.btn.sm.ghost', { type: 'button' }, 'Cancel');
  const confirm = h('div.notice.vf-confirm', null, question, yes, no);
  confirm.hidden = true;

  const clearAsk = (): void => {
    pending = null;
    confirm.hidden = true;
  };
  const ask = (text: string, action: string, go: () => void): void => {
    question.textContent = text;
    yes.textContent = action;
    pending = go;
    confirm.hidden = false;
  };
  yes.addEventListener('click', () => {
    const go = pending;
    clearAsk();
    if (go !== null) go();
  });
  no.addEventListener('click', clearAsk);

  const note = h('span.hint.vf-note', { role: 'status' });
  const link = h('a.btn.sm.ghost.vf-open', { target: '_blank', rel: 'noopener noreferrer' }, 'Open in a new tab');
  link.hidden = true;
  const runBtn = h('button.btn.sm.primary.vf-run', { type: 'button' }, 'Run');
  const foot = h('div.vf-foot', null, h('span.mono.muted.vf-title', null, title), note, link, runBtn);

  const el = h('section.vf', { 'aria-label': title }, stage, confirm, foot);
  el.dataset.artifact = opts.artifactId;
  el.dataset.panel = opts.panelId;
  el.dataset.state = 'still';

  // Only the owner's capability URL is absolute; a share's frame path is relative and gets no link. The link appears
  // once the address is known, which the pointer or the keyboard reaching the card asks for.
  const primeLink = (): void => {
    frameUrl().then(
      (u) => {
        if (/^[a-z][a-z0-9+.-]*:/i.test(u) && webUrl(u) !== null) {
          link.href = u;
          link.hidden = false;
        }
      },
      () => undefined,
    );
  };
  el.addEventListener('pointerenter', primeLink);
  el.addEventListener('focusin', primeLink);

  const focused = (): void => {
    if (card.iframe !== null) touch(card);
  };
  el.addEventListener('pointerdown', focused);
  el.addEventListener('focusin', focused);

  const mount = (src: string): void => {
    const frame = h('iframe.vf-frame', {
      sandbox: SANDBOX,
      referrerpolicy: 'no-referrer',
      loading: 'lazy',
      title,
    });
    frame.src = src;
    frame.style.height = `${startHeight}px`;
    card.iframe = frame;
    el.dataset.state = 'live';
    face.hidden = true;
    runBtn.hidden = true;
    stage.append(frame);
    admit(card);
  };

  const run = async (): Promise<void> => {
    if (card.iframe !== null || busy) return;
    busy = true;
    runBtn.disabled = true;
    note.textContent = '';
    try {
      mount(await frameUrl());
    } catch {
      note.textContent = 'The frame could not be started.';
    } finally {
      busy = false;
      runBtn.disabled = false;
    }
  };
  runBtn.addEventListener('click', () => {
    void run();
  });

  const stop = (): void => {
    clearAsk();
    card.iframe?.remove();
    card.iframe = null;
    el.dataset.state = 'still';
    face.hidden = false;
    runBtn.hidden = false;
  };

  const sendTheme = (): void => {
    const win = card.iframe?.contentWindow;
    if (!win) return;
    // The frame is an opaque origin, so the message goes to any origin; it carries styles only.
    win.postMessage(
      { apz: 1, type: 'theme', vars: themeVars(), mode: document.documentElement.dataset.mode },
      '*',
    );
  };

  const setHeight = (px: number): void => {
    if (card.iframe === null || !Number.isFinite(px)) return;
    card.iframe.style.height = `${clampHeight(px)}px`;
  };

  const askOpen = (raw: string): void => {
    const target = webUrl(raw);
    if (target === null) return;
    ask(`Open ${target.host}?`, 'Open', () => {
      window.open(target.href, '_blank', 'noopener,noreferrer');
    });
  };

  const askSave = (name: string, mime: string, data: string): void => {
    const file = fileName(name);
    ask(`Save ${file}?`, 'Save', () => saveText(file, mime, data));
  };

  const card: Card = { el, iframe: null, stamp: 0, stop, sendTheme, setHeight, askOpen, askSave };
  return el;
}
