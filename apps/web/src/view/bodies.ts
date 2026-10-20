// The bodies of an artifact's panels: the artifact-only types (image, document, and the rest through the context).
// Code, dataset, graph and html are drawn by renderers the context provides; without one, each shows a plain fallback.

import '../styles/view.css';
import { h, lu } from '../ui/dom';
import { renderMarkdown } from '../ui/markdown';
import type { ArtifactBody, CodeBody, DatasetBody, GraphBody, HtmlBody, ImageBody, RowsPage } from './model';

/** What an artifact body needs from its host: where blobs live, how to page rows, and the optional renderers. */
export interface ViewCtx {
  /** URL of a blob of the artifact being shown (owner: /v1/artifacts/{id}/blobs/{blob}; share viewer: its own). */
  blobUrl(blobId: string): string;
  rows(panelId: string, q: { offset: number; limit: number; sort?: string; desc?: boolean; filter?: string }): Promise<RowsPage>;
  /** Builds the live frame for an html panel (another agent provides it); absent = show a still card. */
  frame?(panelId: string, body: HtmlBody): HTMLElement;
  /** Renderers other agents provide; when absent the body shows a plain fallback. */
  code?(body: CodeBody): HTMLElement;
  dataset?(panelId: string, body: DatasetBody): HTMLElement;
  graph?(body: GraphBody): HTMLElement;
}

/** How many inline rows the plain dataset fallback shows. */
const FALLBACK_ROWS = 50;

/** Draws an artifact body into a new element. */
export function renderArtifactBody(panelId: string, body: ArtifactBody, ctx: ViewCtx): HTMLElement {
  switch (body.type) {
    case 'image':
      return imageBody(body, ctx);
    case 'document':
      return h('div.prose.view-doc', null, renderMarkdown(body.markdown));
    case 'code':
      return ctx.code ? ctx.code(body) : codeFallback(body);
    case 'dataset':
      return ctx.dataset ? ctx.dataset(panelId, body) : datasetFallback(body);
    case 'graph':
      return ctx.graph ? ctx.graph(body) : graphFallback(body);
    case 'html':
      return htmlBody(panelId, body, ctx);
  }
}

/** A picture in its own block, with a caption below. A click opens it full size in a lightbox. */
function imageBody(body: ImageBody, ctx: ViewCtx): HTMLElement {
  const src = ctx.blobUrl(body.blob.id);
  const img = h('img', { src, alt: body.alt, loading: 'lazy', width: body.width ?? null, height: body.height ?? null });
  const figure = h(
    'figure.view-image',
    { role: 'button', tabindex: 0, title: 'Open full size', onclick: () => openLightbox(src, body.alt) },
    img,
    body.caption ? h('figcaption.hint', null, body.caption) : null,
  );
  figure.addEventListener('keydown', (e: KeyboardEvent) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      openLightbox(src, body.alt);
    }
  });
  return figure;
}

/** The full image over the page, on the prototype's scrim. A click on the backdrop or Escape closes it. */
function openLightbox(src: string, alt: string): void {
  const onKey = (e: KeyboardEvent): void => {
    if (e.key === 'Escape') close();
  };
  const close = (): void => {
    document.removeEventListener('keydown', onKey);
    overlay.remove();
  };
  const overlay = h(
    'div.scrim.view-lightbox',
    { role: 'dialog', 'aria-modal': 'true', 'aria-label': alt || 'Image', onclick: close },
    h('img', { src, alt }),
    h('button.iconbtn.view-lightbox-close', { type: 'button', title: 'Close', 'aria-label': 'Close', onclick: close }, lu('x', 16)),
  );
  document.addEventListener('keydown', onKey);
  document.body.appendChild(overlay);
}

/** Plain code in a `pre`, labelled with its file name or language, as the prototype's code block. */
function codeFallback(body: CodeBody): HTMLElement {
  const label = body.filename || body.language;
  return h(
    'div.view-code',
    null,
    label ? h('div.dfile-h', null, h('span.mono', null, label)) : null,
    h('pre.code', body.language ? { 'data-lang': body.language } : null, h('code', null, body.text)),
  );
}

/** The first inline rows as a plain table. Rows in a blob need the dataset renderer. */
function datasetFallback(body: DatasetBody): HTMLElement {
  const head = body.columns.map((c) => h('th', { scope: 'col', class: c.kind === 'number' ? 'num' : null }, c.label));
  const rows = body.rows.slice(0, FALLBACK_ROWS).map((row) =>
    h(
      'tr',
      null,
      body.columns.map((c, k) => h('td', { class: c.kind === 'number' ? 'num' : null }, cellText(row[k]))),
    ),
  );
  const note = body.total > rows.length ? `${rows.length} of ${body.total} rows` : `${body.total} rows`;
  return h(
    'div.view-dataset',
    null,
    h('p.hint', null, note),
    h('div.tbl-wrap', null, h('table.tbl', null, h('thead', null, h('tr', null, head)), h('tbody', null, rows))),
  );
}

/** A value as text. Nested values are written as JSON; nothing here is markup. */
function cellText(value: unknown): string {
  if (value == null) return '';
  if (typeof value === 'object') return JSON.stringify(value);
  return String(value);
}

/** The nodes as a list, with their edges written under each. */
function graphFallback(body: GraphBody): HTMLElement {
  const labels = new Map(body.nodes.map((n) => [n.id, n.label]));
  const arrow = body.directed === false ? ' - ' : ' → ';
  return h(
    'div.view-graph',
    null,
    h(
      'ul.notes',
      null,
      body.nodes.map((n) => h('li', null, h('strong', null, n.label), n.sub ? h('span.muted', null, ` ${n.sub}`) : null)),
    ),
    body.edges.length
      ? h(
          'ul.notes.view-edges',
          null,
          body.edges.map((e) =>
            h('li', null, `${labels.get(e.from) ?? e.from}${arrow}${labels.get(e.to) ?? e.to}`, e.label ? ` (${e.label})` : null),
          ),
        )
      : null,
  );
}

/**
 * A live page. Where the context can run frames, the frame card itself (its still and its Run button); elsewhere a
 * still card: the hub's picture, the entry and its libraries.
 */
function htmlBody(panelId: string, body: HtmlBody, ctx: ViewCtx): HTMLElement {
  const frame = ctx.frame?.(panelId, body);
  if (frame) return frame;
  const still = body.still ? h('img', { src: ctx.blobUrl(body.still.id), alt: '', loading: 'lazy' }) : null;
  const card = h(
    'div.view-still',
    null,
    still ? h('div.view-still-img', null, still) : null,
    h('div.view-still-meta', null, h('code', null, body.entry), ...(body.libs ?? []).map((lib) => h('span.chip', null, lib))),
  );
  card.style.minHeight = `${body.height}px`;
  return card;
}
