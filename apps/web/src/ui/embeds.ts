// Rich content in the web (docs/design/workspace.md, section 3.4): the chips and hover cards for the references that
// ui/rich.ts recognises, and inline media for images, videos and HTML files. DOM calls only, never innerHTML.
//
// The hub answers card data for references in one call (POST /v1/unfurl). The references of a rendered block are
// queued and sent together on the next microtask, 50 at most per request. Answers are kept in memory for five minutes
// (the hub sends no TTL) and failures for one. A chip or card that is waiting shows its neutral state and upgrades
// when its answer lands.

import '../styles/embeds.css';
import { api, type UnfurlAnswer, type UnfurlCard } from '../api';
import { go } from '../router';
import { agents as roster, overview } from '../store';
import { agentHue } from '../views/floor-common';
import { BRAND_ICONS } from './brands.gen';
import { ago } from './components';
import { h, icon, replace } from './dom';
import { label, targetOf, type AgentName, type Ref, type RefSpan, type RichCtx } from './rich';

/** What markdown needs to render rich content: the classifier's context and the handlers for clicks. */
export interface EmbedContext extends RichCtx {
  /** An agent was chosen: its cid when the text gives one or the name resolves to one, else its name. */
  openAgent?: (who: number | string) => void;
  /** A task by its number (`task #528`). */
  openTask?: (number: number) => void;
  /** A workspace file at a line (null when the reference names none). */
  openPath?: (path: string, line: number | null) => void;
  /** A library item: a deliverable's id, or `name/version` for a library tool. */
  openLibrary?: (id: string) => void;
  /** The first lines of a workspace file, for the hover preview of a path (the IDE's read API). Absent: no preview. */
  readHead?: (path: string) => Promise<string[] | null>;
}

/** The lines a path's hover preview shows. */
export const HEAD_LINES = 12;

const HOVER_MS = 300;
const LONG_PRESS_MS = 500;
const HIDE_MS = 150;
const CACHE_MS = 5 * 60_000;
const FAILURE_MS = 60_000;
const CACHE_MAX = 1000;
const BATCH_MAX = 50;
const SVG_NS = 'http://www.w3.org/2000/svg';

// ---- Data: one batch per render, answers cached in memory -------------------------------------------------------

type Listener = (answer: UnfurlAnswer) => void;
interface Held {
  at: number;
  answer: UnfurlAnswer;
}
interface Pending {
  ref: Ref;
  listeners: Set<Listener>;
}

const held = new Map<string, Held>();
const pending = new Map<string, Pending>();
let queue: string[] = [];
let scheduled = false;

const keyOf = (ref: Ref): string => JSON.stringify(ref);

/** Calls `fn` with the answer for `ref`: now when a fresh one is held, else when its batch lands. Returns the function
 * that stops the calls. */
export function subscribe(ref: Ref, fn: Listener): () => void {
  const key = keyOf(ref);
  const have = held.get(key);
  if (have && Date.now() - have.at < (have.answer.ok ? CACHE_MS : FAILURE_MS)) {
    fn(have.answer);
    return () => undefined;
  }
  let entry = pending.get(key);
  if (!entry) {
    entry = { ref, listeners: new Set() };
    pending.set(key, entry);
    queue.push(key);
    if (!scheduled) {
      scheduled = true;
      queueMicrotask(sendQueued);
    }
  }
  entry.listeners.add(fn);
  return () => {
    pending.get(key)?.listeners.delete(fn);
  };
}

function sendQueued(): void {
  scheduled = false;
  const keys = queue;
  queue = [];
  for (let i = 0; i < keys.length; i += BATCH_MAX) void send(keys.slice(i, i + BATCH_MAX));
}

async function send(keys: string[]): Promise<void> {
  const refs = keys.flatMap((key) => {
    const entry = pending.get(key);
    return entry ? [entry.ref] : [];
  });
  let cards: unknown[] = [];
  try {
    cards = (await api.unfurl(refs)).cards;
  } catch {
    cards = [];
  }
  keys.forEach((key, i) => settle(key, asAnswer(cards[i])));
}

/** An answer as the hub sends it, or an error when the entry is missing or malformed. */
function asAnswer(value: unknown): UnfurlAnswer {
  const o = value as { ok?: unknown; card?: unknown; error?: unknown } | null | undefined;
  if (o && o.ok === true && o.card && typeof o.card === 'object') return { ok: true, card: o.card as UnfurlCard };
  return { ok: false, error: typeof o?.error === 'string' ? o.error : 'unavailable' };
}

function settle(key: string, answer: UnfurlAnswer): void {
  const entry = pending.get(key);
  pending.delete(key);
  held.set(key, { at: Date.now(), answer });
  if (held.size > CACHE_MAX) {
    const oldest = held.keys().next().value;
    if (oldest !== undefined) held.delete(oldest);
  }
  entry?.listeners.forEach((fn) => {
    try {
      fn(answer);
    } catch (err) {
      console.error(err);
    }
  });
}

/** The kinds the hub has a card for (the others are text, links and files read from the workspace). */
function carded(kind: string): boolean {
  return (
    kind.startsWith('github_') ||
    kind.startsWith('plaza_git_') ||
    kind === 'gist' ||
    kind === 'agent' ||
    kind === 'task' ||
    kind === 'library' ||
    kind === 'library_tool'
  );
}

// ---- Card fields: the hub's JSON, read with type checks ----------------------------------------------------------

const str = (c: UnfurlCard, k: string): string | null => {
  const v = c[k];
  return typeof v === 'string' && v !== '' ? v : null;
};
const num = (c: UnfurlCard, k: string): number | null => {
  const v = c[k];
  return typeof v === 'number' && Number.isFinite(v) ? v : null;
};
const strs = (c: UnfurlCard, k: string): string[] => {
  const v = c[k];
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [];
};
/** A text or a number field as text (an assignee id may be either). */
const textOf = (c: UnfurlCard, k: string): string | null => {
  const v = c[k];
  return typeof v === 'number' ? String(v) : str(c, k);
};

interface Person {
  name: string;
  avatar: string | null;
}

/** An author: a user object (login, avatar_url, name) or a plain name. Avatars are https only. */
function person(c: UnfurlCard, k: string): Person | null {
  const v = c[k];
  if (typeof v === 'string' && v !== '') return { name: v, avatar: null };
  if (!v || typeof v !== 'object') return null;
  const o = v as Record<string, unknown>;
  const login = typeof o.login === 'string' && o.login !== '' ? o.login : null;
  const name = login ?? (typeof o.name === 'string' && o.name !== '' ? o.name : null);
  if (name === null) return null;
  const avatar = typeof o.avatar_url === 'string' && o.avatar_url.startsWith('https://') ? o.avatar_url : null;
  return { name, avatar };
}

const when = (ms: number | null): string | null => (ms === null ? null : ago(ms));
const bytesText = (n: number | null): string | null => {
  if (n === null) return null;
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
};
const plural = (n: number, word: string): string => `${n} ${word}${n === 1 ? '' : 's'}`;

// ---- Card bodies -----------------------------------------------------------------------------------------------

type Block = Node | null;

const kicker = (text: string): HTMLElement => h('div.embed-kicker', null, text);
const titleOf = (text: string): HTMLElement => h('div.embed-title', null, text);
const excerpt = (text: string | null): Block => (text ? h('p.embed-excerpt', null, text) : null);
const meta = (...parts: Array<string | null>): Block => {
  const shown = parts.filter((p): p is string => !!p);
  return shown.length ? h('div.embed-meta', null, shown.join(' · ')) : null;
};
const badge = (state: string): Block => (state ? h('span.embed-state', { 'data-state': state }, state) : null);
function byline(p: Person | null): Block {
  if (!p) return null;
  return h('span.embed-byline', null, p.avatar ? h('img.embed-avatar', { src: p.avatar, alt: '', width: 20, height: 20, loading: 'lazy', referrerpolicy: 'no-referrer' }) : null, p.name);
}

function bodyOf(r: Ref, answer: UnfurlAnswer | undefined): Block[] {
  if (r.kind === 'path') {
    return [kicker(r.dir ? 'Workspace folder' : 'Workspace file'), titleOf(r.path), meta(r.line !== null ? `Line ${r.line}` : null)];
  }
  if (answer === undefined) return [h('p.embed-muted', null, 'Loading preview…')];
  if (!answer.ok) return [h('p.embed-muted', null, 'Preview unavailable')];
  const c = answer.card;
  switch (r.kind) {
    case 'github_pull':
    case 'plaza_git_pull':
    case 'github_issue':
    case 'plaza_git_issue':
      return issueBody(label(r), c, r.kind.endsWith('pull'));
    case 'github_commit':
    case 'plaza_git_commit': {
      const files = num(c, 'files');
      const added = num(c, 'additions');
      const removed = num(c, 'deletions');
      return [
        kicker(label(r)),
        titleOf(str(c, 'title') ?? 'Commit'),
        h('div.embed-row', null, byline(person(c, 'author'))),
        meta(when(num(c, 'date_ms')), files === null ? null : plural(files, 'file'), diffText(added, removed)),
      ];
    }
    case 'github_repo':
    case 'plaza_git_repo': {
      const stars = num(c, 'stars');
      return [
        kicker('Repository'),
        titleOf(str(c, 'name') ?? label(r)),
        excerpt(str(c, 'description')),
        h('div.embed-row', null, stars === null ? null : `★ ${stars}`, str(c, 'language')),
        meta(when(num(c, 'updated_ms')), str(c, 'default_branch')),
      ];
    }
    case 'github_file':
    case 'plaza_git_file':
      return fileBody(label(r), c);
    case 'github_release':
    case 'plaza_git_release':
      return [
        kicker(label(r)),
        titleOf(str(c, 'name') ?? str(c, 'tag') ?? label(r)),
        h('div.embed-row', null, byline(person(c, 'author')), flag(c, 'prerelease') ? h('span.embed-label', null, 'pre-release') : null, flag(c, 'draft') ? h('span.embed-label', null, 'draft') : null),
        excerpt(str(c, 'body_excerpt')),
        meta(when(num(c, 'created_ms'))),
      ];
    case 'github_run':
    case 'plaza_git_run':
      return [
        kicker(label(r)),
        titleOf(str(c, 'title') ?? str(c, 'name') ?? label(r)),
        h('div.embed-row', null, badge(str(c, 'conclusion') ?? str(c, 'state') ?? ''), str(c, 'branch')),
        meta(str(c, 'actor'), when(num(c, 'created_ms'))),
      ];
    case 'gist': {
      const files = num(c, 'files');
      return [kicker('Gist'), titleOf(str(c, 'title') ?? 'Gist'), h('div.embed-row', null, byline(person(c, 'author'))), meta(files === null ? null : plural(files, 'file'), when(num(c, 'updated_ms')))];
    }
    case 'agent':
      return agentBody(c, r.name ?? '');
    case 'task': {
      const state = str(c, 'status') ?? '';
      return [
        kicker(label(r)),
        titleOf(str(c, 'title') ?? 'Task'),
        h('div.embed-row', null, badge(state), textOf(c, 'assignee') ? `Assignee ${textOf(c, 'assignee')}` : null),
        meta(str(c, 'project'), when(num(c, 'updated_ms'))),
      ];
    }
    case 'library':
      return [
        kicker('Library file'),
        titleOf(str(c, 'title') ?? str(c, 'name') ?? r.id),
        h('div.embed-row', null, str(c, 'content_type'), byline(person(c, 'author'))),
        meta(bytesText(num(c, 'size')), str(c, 'task'), when(num(c, 'created_ms'))),
      ];
    case 'library_tool': {
      const files = c.files;
      return [
        kicker(label(r)),
        titleOf(str(c, 'name') ?? r.name),
        excerpt(str(c, 'description')),
        h('div.embed-row', null, badge(str(c, 'state') ?? ''), byline(person(c, 'author'))),
        meta(typeof files === 'number' ? plural(files, 'file') : null, when(num(c, 'created_ms'))),
      ];
    }
    default:
      return [];
  }
}

function flag(c: UnfurlCard, k: string): boolean {
  return c[k] === true;
}

function diffText(added: number | null, removed: number | null): string | null {
  if (added === null && removed === null) return null;
  return `+${added ?? 0} −${removed ?? 0}`;
}

function issueBody(title: string, c: UnfurlCard, pull: boolean): Block[] {
  const labels = strs(c, 'labels');
  const comments = num(c, 'comments');
  return [
    kicker(title),
    titleOf(str(c, 'title') ?? 'Untitled'),
    h('div.embed-row', null, byline(person(c, 'author')), badge(str(c, 'state') ?? '')),
    pull
      ? h('div.embed-row.embed-mono', null, `${str(c, 'head') ?? '?'} → ${str(c, 'base') ?? '?'}`, h('span.embed-add', null, `+${num(c, 'additions') ?? 0}`), h('span.embed-del', null, `−${num(c, 'deletions') ?? 0}`))
      : labels.length
        ? h('div.embed-row', null, labels.map((name) => h('span.embed-label', null, name)))
        : null,
    excerpt(str(c, 'body_excerpt')),
    meta(when(num(c, 'created_ms')), comments && comments > 0 ? plural(comments, 'comment') : null),
  ];
}

function fileBody(title: string, c: UnfurlCard): Block[] {
  const path = str(c, 'path') ?? '';
  const ref = str(c, 'ref');
  if (c.dir === true) {
    const entries = num(c, 'entries');
    const names = strs(c, 'names');
    return [
      kicker(title),
      h('div.embed-title.embed-mono-title', null, path),
      meta(entries === null ? null : plural(entries, 'entry'), ref ? `on ${ref}` : null),
      names.length ? h('div.embed-row.embed-mono', null, names.join('  ')) : null,
    ];
  }
  const lines = Array.isArray(c.lines) ? (c.lines as unknown[]).filter((x): x is string => typeof x === 'string') : [];
  return [
    kicker(title),
    h('div.embed-title.embed-mono-title', null, path),
    meta(bytesText(num(c, 'size')), ref ? `on ${ref}` : null),
    c.binary === true ? h('p.embed-muted', null, 'Binary file') : null,
    lines.length ? h('pre.embed-code', null, lines.join('\n')) : null,
    c.binary !== true && lines.length === 0 ? h('p.embed-muted', null, 'No preview') : null,
  ];
}

function agentBody(c: UnfurlCard, fallback: string): Block[] {
  const name = str(c, 'name') ?? fallback;
  const status = str(c, 'status') ?? '';
  const t = c.task as { number?: unknown; title?: unknown } | null | undefined;
  const current = t && typeof t === 'object' && typeof t.number === 'number' ? `#${t.number} ${typeof t.title === 'string' ? t.title : ''}`.trim() : 'No current task';
  const active = when(num(c, 'last_active_ms'));
  return [
    kicker('Agent'),
    titleOf(name ? `@${name}` : 'Agent'),
    h('div.embed-row', null, status ? h('span.embed-status-text', { 'data-status': status }, status) : null, str(c, 'role'), str(c, 'model')),
    h('div.embed-row', null, h('span.embed-muted-inline', null, 'Task'), current),
    meta(active ? `Last active ${active}` : null),
  ];
}

// ---- Chips -------------------------------------------------------------------------------------------------------

function brandMark(name: string): SVGSVGElement | null {
  const d = BRAND_ICONS[name]?.path;
  if (!d) return null;
  const svg = document.createElementNS(SVG_NS, 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('class', 'embed-icon');
  svg.setAttribute('aria-hidden', 'true');
  const path = document.createElementNS(SVG_NS, 'path');
  path.setAttribute('d', d);
  path.setAttribute('fill', 'currentColor');
  svg.append(path);
  return svg;
}

/** The leading mark of a chip: the GitHub or plaza git mark, the agent's dot, a file or library icon. */
function leading(r: Ref): Node | null {
  if (r.kind.startsWith('github_') || r.kind === 'gist') return brandMark('github');
  if (r.kind.startsWith('plaza_git_')) return icon('git', 'embed-icon');
  switch (r.kind) {
    case 'agent':
      return h('span.embed-dot', { 'aria-hidden': 'true' });
    case 'path':
    case 'library':
      return icon('file', 'embed-icon');
    case 'library_tool':
      return icon('library', 'embed-icon');
    default:
      return null;
  }
}

function cidOf(r: Extract<Ref, { kind: 'agent' }>, embeds: EmbedContext): number | null {
  if (r.cid !== null) return r.cid;
  if (r.name === null) return null;
  const known = embeds.agents?.find((a: AgentName) => a.name === r.name) ?? roster.get().find((a) => a.name === r.name);
  return known ? known.cid : null;
}

/** What a click on the reference does, when the embedding page handles it. */
function actionFor(r: Ref, embeds: EmbedContext): (() => void) | null {
  const { openAgent, openTask, openPath, openLibrary } = embeds;
  switch (r.kind) {
    case 'agent': {
      if (!openAgent) return null;
      const who: number | string = cidOf(r, embeds) ?? r.name ?? '';
      return () => openAgent(who);
    }
    case 'task':
      return openTask ? () => openTask(r.number) : null;
    case 'path':
      return openPath ? () => openPath(r.path, r.line) : null;
    case 'library':
      return openLibrary ? () => openLibrary(r.id) : null;
    case 'library_tool':
      return openLibrary ? () => openLibrary(`${r.name}/${r.version}`) : null;
    default:
      return null;
  }
}

/** The state a chip shows once its card lands: pull and issue states, a task's or an agent's status. */
function paint(el: HTMLElement, r: Ref, card: UnfurlCard): void {
  if (r.kind === 'github_pull' || r.kind === 'plaza_git_pull' || r.kind === 'github_issue' || r.kind === 'plaza_git_issue') {
    const state = str(card, 'state');
    if (state) el.dataset.state = state;
  } else if (r.kind === 'task' || r.kind === 'agent') {
    const status = str(card, 'status');
    if (status) el.dataset.status = status;
  }
}

function chip(r: Ref, raw: string, text: string | null, embeds: EmbedContext): HTMLElement {
  const forge = r.kind.startsWith('github_') || r.kind.startsWith('plaza_git_') || r.kind === 'gist';
  const target = targetOf(raw);
  const href = forge && /^https?:\/\//i.test(target) ? target : null;
  const agentCid = r.kind === 'agent' ? cidOf(r, embeds) : null;
  const agentName = r.kind === 'agent' ? (r.name ?? embeds.agents?.find((a) => a.cid === agentCid)?.name ?? null) : null;
  const shown = text || (agentName ? `@${agentName}` : label(r));
  const action = actionFor(r, embeds);
  const hasCard = carded(r.kind) || previewable(r, embeds);

  const body: Node[] = [];
  const lead = leading(r);
  if (lead) body.push(lead);
  const labelEl = h('span.embed-chip-label', null, shown);
  body.push(labelEl);
  if (r.kind === 'agent') body.push(h('span.embed-status', { 'aria-hidden': 'true' }));

  const kind = { 'data-kind': r.kind };
  const el: HTMLElement = href
    ? h('a.embed-chip', { ...kind, href, target: '_blank', rel: 'noopener noreferrer' }, body)
    : hasCard || action
      ? h('button.embed-chip', { ...kind, type: 'button' }, body)
      : h('span.embed-chip', kind, body);

  if (r.kind === 'agent' && agentCid !== null) {
    el.style.setProperty('--hue', String(agentHue(agentCid)));
    const status = roster.get().find((a) => a.cid === agentCid)?.status;
    if (status) el.dataset.status = status;
  }

  let webUrl: string | null = null;
  el.addEventListener('click', (e) => {
    if (pressed.has(el)) {
      pressed.delete(el);
      e.preventDefault();
      return;
    }
    if (href) return;
    e.preventDefault();
    if (action) action();
    else if (webUrl) window.open(webUrl, '_blank', 'noopener,noreferrer');
  });

  if (hasCard) wireHover(el, r, embeds);
  if (carded(r.kind)) {
    subscribe(r, (answer) => {
      if (!answer.ok) return;
      webUrl = str(answer.card, 'url');
      paint(el, r, answer.card);
      // A library file is named by its id until its card says what it is.
      const name = r.kind === 'library' && !text ? str(answer.card, 'title') ?? str(answer.card, 'name') : null;
      if (name) labelEl.textContent = name;
    });
  }
  return el;
}

/** A workspace file (not a folder) the page can preview: the reference is a path and the page can read the file. */
function previewable(r: Ref, embeds: EmbedContext): boolean {
  return r.kind === 'path' && !r.dir && !!embeds.readHead;
}

// ---- Media ---------------------------------------------------------------------------------------------------------

function imageNode(url: string, alt: string): HTMLElement {
  const button: HTMLElement = h('button.embed-media-btn', { type: 'button', 'aria-label': `Open image: ${alt}` }, h('img.embed-img', { src: url, alt, loading: 'lazy', referrerpolicy: 'no-referrer' }));
  button.addEventListener('click', () => openLightbox(url, alt, button));
  return h('span.embed-block', null, button);
}

function videoNode(url: string, name: string): HTMLElement {
  return h('span.embed-block', null, h('video.embed-video', { src: url, controls: true, preload: 'metadata', playsinline: true, 'aria-label': name, title: name }));
}

/** An HTML file in a frame that cannot reach the console: no same-origin, no forms, no top navigation. It loads when the
 * reader asks for the preview, and "Expand" moves the card to a panel over the page. */
function htmlNode(src: string, name: string): HTMLElement {
  const frame = h('iframe.embed-frame', { title: name, sandbox: 'allow-scripts', referrerpolicy: 'no-referrer', hidden: true });
  const preview = h('button.embed-btn', { type: 'button', 'aria-expanded': 'false' }, 'Preview');
  const expand = h('button.embed-btn', { type: 'button', hidden: true }, 'Expand');
  const card: HTMLElement = h('div.embed-html', null, h('div.embed-html-head', null, icon('file', 'embed-icon'), h('span.embed-html-name', { title: name }, name), preview, expand), frame);
  let loaded = false;
  preview.addEventListener('click', () => {
    if (frame.hidden) {
      if (!loaded) {
        frame.src = src;
        loaded = true;
      }
      frame.hidden = false;
      preview.textContent = 'Hide';
      preview.setAttribute('aria-expanded', 'true');
      expand.hidden = false;
    } else {
      if (panel?.card === card) closePanel();
      frame.hidden = true;
      preview.textContent = 'Preview';
      preview.setAttribute('aria-expanded', 'false');
      expand.hidden = true;
    }
  });
  expand.addEventListener('click', () => (panel?.card === card ? closePanel() : openPanel(card, expand)));
  return card;
}

/** A library file: a chip until its card says what it is, then the image, video or HTML it holds. */
function librarySlot(r: Extract<Ref, { kind: 'library' }>, raw: string, text: string | null, embeds: EmbedContext): HTMLElement {
  const slot = h('span.embed-slot');
  slot.append(chip(r, raw, text, embeds));
  subscribe(r, (answer) => {
    if (!answer.ok) return;
    const type = str(answer.card, 'content_type') ?? '';
    const name = str(answer.card, 'title') ?? str(answer.card, 'name') ?? r.id;
    const src = api.fileUrl(r.id);
    if (type.startsWith('image/')) replace(slot, imageNode(src, name));
    else if (type.startsWith('video/')) replace(slot, videoNode(src, name));
    else if (type.startsWith('text/html')) replace(slot, htmlNode(src, name));
  });
  return slot;
}

let lightbox: { box: HTMLElement; opener: HTMLElement } | null = null;

function openLightbox(src: string, alt: string, opener: HTMLElement): void {
  closeLightbox();
  const close = h('button.embed-btn.embed-lightbox-close', { type: 'button', onclick: () => closeLightbox() }, 'Close');
  const box: HTMLElement = h(
    'div.embed-lightbox',
    {
      role: 'dialog',
      'aria-modal': 'true',
      'aria-label': alt || 'Image',
      onclick: (e: Event) => {
        if (e.target === box) closeLightbox();
      },
    },
    close,
    h('img', { src, alt, referrerpolicy: 'no-referrer' }),
  );
  document.body.append(box);
  lightbox = { box, opener };
  close.focus();
}

function closeLightbox(): void {
  if (!lightbox) return;
  const { box, opener } = lightbox;
  lightbox = null;
  box.remove();
  opener.focus();
}

/** An HTML card moved to a panel over the page; it returns to its place when the panel closes. */
let panel: { card: HTMLElement; marker: Element; button: HTMLElement } | null = null;

function openPanel(card: HTMLElement, button: HTMLElement): void {
  closePanel();
  const marker = document.createElement('span');
  marker.hidden = true;
  card.replaceWith(marker);
  document.body.append(card);
  card.classList.add('is-panel');
  button.textContent = 'Close panel';
  panel = { card, marker, button };
}

function closePanel(): void {
  if (!panel) return;
  const { card, marker, button } = panel;
  panel = null;
  card.classList.remove('is-panel');
  button.textContent = 'Expand';
  if (marker.isConnected) marker.replaceWith(card);
  else card.remove();
  button.focus();
}

// ---- Hover cards -----------------------------------------------------------------------------------------------------

interface Shown {
  anchor: HTMLElement;
  pop: HTMLElement;
  unsub: () => void;
}

let active: Shown | null = null;
let hideTimer: ReturnType<typeof setTimeout> | undefined;
let popSeq = 0;
let globalsInstalled = false;
/** Chips whose long press opened a card: the click that ends the press does nothing. */
const pressed = new WeakSet<Element>();

function scheduleHide(): void {
  clearTimeout(hideTimer);
  hideTimer = setTimeout(hideCard, HIDE_MS);
}

function cancelHide(): void {
  clearTimeout(hideTimer);
  hideTimer = undefined;
}

function hideCard(): void {
  cancelHide();
  if (!active) return;
  const { anchor, pop, unsub } = active;
  active = null;
  unsub();
  pop.remove();
  anchor.removeAttribute('aria-describedby');
}

/** Under the anchor, above it when it would leave the viewport, kept inside the viewport sideways. */
function place(pop: HTMLElement, anchor: HTMLElement): void {
  if (!anchor.isConnected) {
    hideCard();
    return;
  }
  const a = anchor.getBoundingClientRect();
  const p = pop.getBoundingClientRect();
  const gap = 8;
  const below = a.bottom + gap;
  const above = a.top - gap - p.height;
  const top = below + p.height > window.innerHeight - gap && above >= gap ? above : below;
  const left = Math.max(gap, Math.min(a.left, window.innerWidth - p.width - gap));
  pop.style.top = `${Math.max(gap, top)}px`;
  pop.style.left = `${left}px`;
}

function showCard(anchor: HTMLElement, r: Ref, embeds: EmbedContext): void {
  cancelHide();
  if (active?.anchor === anchor) return;
  hideCard();
  installGlobals();
  popSeq += 1;
  const pop = h('div.embed-pop', { role: 'tooltip', id: `embed-pop-${popSeq}` });
  pop.addEventListener('pointerenter', cancelHide);
  pop.addEventListener('pointerleave', (e) => {
    if (e.pointerType === 'mouse') scheduleHide();
  });
  replace(pop, bodyOf(r, undefined));
  document.body.append(pop);
  anchor.setAttribute('aria-describedby', pop.id);
  const shown: Shown = { anchor, pop, unsub: () => undefined };
  active = shown;
  place(pop, anchor);
  if (carded(r.kind)) {
    shown.unsub = subscribe(r, (answer) => {
      replace(pop, bodyOf(r, answer));
      place(pop, anchor);
    });
  } else if (r.kind === 'path' && embeds.readHead) {
    // A workspace file: its first lines, read by the page. The card stays what it was if the reader moved on.
    const read = embeds.readHead(r.path);
    void read
      .catch(() => null)
      .then((lines) => {
        if (active?.pop !== pop) return;
        const head = lines ? lines.slice(0, HEAD_LINES) : null;
        replace(pop, [
          ...bodyOf(r, undefined),
          head === null ? h('p.embed-muted', null, 'No preview') : head.length ? h('pre.embed-code', null, head.join('\n')) : h('p.embed-muted', null, 'Empty file'),
        ]);
        place(pop, anchor);
      });
  }
}

/** Pointer and keyboard behaviour of a chip with a card: hover opens after a short wait and stays while hovered; focus
 * (keyboard) and a long press (touch) open it at once. */
function wireHover(el: HTMLElement, r: Ref, embeds: EmbedContext): void {
  let showTimer: ReturnType<typeof setTimeout> | undefined;
  let pressTimer: ReturnType<typeof setTimeout> | undefined;
  const stopPress = (): void => clearTimeout(pressTimer);
  el.addEventListener('pointerenter', (e) => {
    if (e.pointerType !== 'mouse') return;
    cancelHide();
    clearTimeout(showTimer);
    showTimer = setTimeout(() => showCard(el, r, embeds), HOVER_MS);
  });
  el.addEventListener('pointerleave', (e) => {
    clearTimeout(showTimer);
    if (e.pointerType === 'mouse') scheduleHide();
  });
  el.addEventListener('focus', () => {
    if (el.matches(':focus-visible')) showCard(el, r, embeds);
  });
  el.addEventListener('blur', scheduleHide);
  el.addEventListener('pointerdown', (e) => {
    pressed.delete(el);
    if (e.pointerType === 'mouse') return;
    stopPress();
    pressTimer = setTimeout(() => {
      pressed.add(el);
      showCard(el, r, embeds);
    }, LONG_PRESS_MS);
  });
  el.addEventListener('pointerup', stopPress);
  el.addEventListener('pointercancel', stopPress);
}

/** Once per page: Escape closes the open layer, a tap outside closes a card, and cards follow their anchors. */
function installGlobals(): void {
  if (globalsInstalled || typeof document === 'undefined') return;
  globalsInstalled = true;
  document.addEventListener('keydown', (e) => {
    if (e.key !== 'Escape') return;
    if (lightbox) closeLightbox();
    else if (panel) closePanel();
    else if (active) hideCard();
  });
  document.addEventListener(
    'pointerdown',
    (e) => {
      if (!active) return;
      const target = e.target as Node;
      if (active.pop.contains(target) || active.anchor.contains(target)) return;
      hideCard();
    },
    true,
  );
  const follow = (): void => {
    if (active) place(active.pop, active.anchor);
  };
  window.addEventListener('resize', follow);
  window.addEventListener('scroll', follow, { capture: true, passive: true });
}

// ---- Entry points ------------------------------------------------------------------------------------------------------

/** The node for one reference span: a chip, a link, or inline media (images, videos, HTML files). */
export function refNode(value: RefSpan['value'], embeds: EmbedContext): HTMLElement {
  const { raw, r, text } = value;
  switch (r.kind) {
    case 'image':
      return imageNode(r.url, text || label(r));
    case 'video':
      return videoNode(r.url, text || label(r));
    case 'html':
      return htmlNode(r.url, text || label(r));
    case 'link':
      return h('a', { href: targetOf(raw), target: '_blank', rel: 'noopener noreferrer' }, text || label(r));
    case 'library':
      return librarySlot(r, raw, text, embeds);
    default:
      return chip(r, raw, text, embeds);
  }
}

/** An agent by its cid, or by its name (the roster's). Opens its page; a name nobody holds opens nothing. */
function openAgentPage(who: number | string): void {
  if (typeof who === 'number') {
    go('agents', who);
    return;
  }
  const known = roster.get().find((a) => a.name === who);
  if (known) go('agents', known.cid);
}

/** A library item opens on the Library (a deliverable by its id); a library tool (`name/version`) on the Tools screen. */
function openLibraryPage(id: string): void {
  if (id.includes('/')) go('tools');
  else go('library', id);
}

/** The page's rich-content context: the agent roster (names to cids), the plaza's git host from the overview, and the
 * handlers that open an agent's page, a task and a library item. A page passes what it knows in `extra`: the project,
 * its repository and the workspace paths (the IDE), and its own handlers. */
export function embedContextFromStore(extra: Partial<EmbedContext> = {}): EmbedContext {
  const names = roster
    .get()
    .filter((a) => a.name !== '')
    .map((a) => ({ name: a.name, cid: a.cid }));
  return {
    project: null,
    project_repo: null,
    plaza_git_host: overview.get()?.git_host ?? null,
    hub_host: typeof location === 'undefined' ? null : location.host,
    paths: [],
    agents: names,
    openAgent: openAgentPage,
    openTask: (number) => go('tasks', number),
    openLibrary: openLibraryPage,
    ...extra,
  };
}
