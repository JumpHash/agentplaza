// An artifact as a page: a head (title, sub, kind, version) and its panels on the Metrics grid. Metrics bodies draw
// through the Metrics panel renderer; the artifact-only bodies draw in a card of their own, with the panel's title.
// Every panel is a Metrics card (mt-card on mt-grid, as metrics/panels.ts draws them), so the cards never double up.

import '../styles/view.css';
import '../styles/proto/metrics.css';
import { h } from '../ui/dom';
import { HoverBus } from '../metrics/hover';
import { renderPanel, type PanelCtx } from '../metrics/panels';
import type { Body, Panel } from '../metrics/model';
import { renderArtifactBody, type ViewCtx } from './bodies';
import type { Artifact, ArtifactBody } from './model';

const ARTIFACT_TYPES = new Set<string>(['image', 'graph', 'dataset', 'document', 'code', 'html']);

/** True for the bodies only an artifact has (drawn by renderArtifactBody). */
function isArtifactBody(body: Body): body is ArtifactBody {
  return ARTIFACT_TYPES.has(body.type);
}

/** The Metrics panel context for an artifact page: charts and tables work, every action is a no-op. */
function panelContext(ctx: ViewCtx): PanelCtx {
  const noop = (): void => {};
  return {
    hover: new HoverBus(),
    annotations: [],
    showAnnotations: false,
    compact: false,
    // Minutes east of UTC, as the Metrics panels read it.
    tz: -new Date().getTimezoneOffset(),
    onZoom: noop,
    onPoint: noop,
    go: noop,
    filter: noop,
    mute: noop,
    ack: noop,
    toggleRule: noop,
    newRule: noop,
    action: noop,
    page: () => Promise.reject(new Error('Artifact tables do not page through the Metrics route.')),
    pin: noop,
    isPinned: () => false,
    about: noop,
    copyLink: noop,
    liveNow: false,
    artifact: ctx,
  };
}

/** One panel's card: an artifact body in a Metrics card with its title, or a Metrics panel through its renderer. */
/**
 * A panel with the metrics renderer's defaults filled in. An artifact's chart names only what it draws (its series),
 * while the metrics renderer reads every list (ghosts, markers) and flag of a metrics chart.
 */
function normalised(p: Panel): Panel {
  const b = p.body as { type?: string } & Record<string, unknown>;
  if (b.type !== 'chart') return p;
  const series = Array.isArray(b.series)
    ? (b.series as Record<string, unknown>[]).map((s) => ({ dash: false, ...s }))
    : [];
  return {
    ...p,
    body: { stacked: false, area: false, annotate: false, height: 240, ...b, series, ghosts: b.ghosts ?? [], markers: b.markers ?? [] } as Panel['body'],
  };
}

function panelNode(p: Panel, ctx: ViewCtx, pctx: PanelCtx): HTMLElement {
  if (!isArtifactBody(p.body)) return renderPanel(normalised(p), pctx).el;
  const card = h(
    'section.mt-card.mt-c',
    { 'data-panel': p.id },
    h('div.mt-card-h', null, h('div.mt-ttl', null, p.title), p.right ? h('span.mt-sub', null, p.right) : null),
    p.sub ? h('div.mt-sub', null, p.sub) : null,
    renderArtifactBody(p.id, p.body, ctx),
  );
  card.style.gridColumn = `span ${p.span}`;
  return card;
}

/** The artifact's head: its title and sub, and its kind and version. */
export function artifactHead(a: Artifact): HTMLElement {
  return h(
    'header.view-head',
    null,
    h('div', null, h('h1', null, a.title), a.sub ? h('p.lede', null, a.sub) : null),
    h('div.cluster', null, h('span.chip', null, a.kind), h('span.chip', null, `v${a.version}`)),
  );
}

/**
 * Renders an artifact. `firstOnly` shows just its first panel, for an inline embed. `head: false` leaves out the head,
 * for a screen that draws its own page head. `ctx.metrics` is accepted for callers that pass their Metrics context
 * along; the page does not read it.
 */
export function renderArtifact(
  a: Artifact,
  ctx: ViewCtx & { metrics?: unknown },
  opts: { firstOnly?: boolean; head?: boolean } = {},
): HTMLElement {
  const pctx = panelContext(ctx);
  const panels = opts.firstOnly ? a.panels.slice(0, 1) : a.panels;
  const grid = h('div.mt-grid.view-grid', null, panels.map((p) => panelNode(p, ctx, pctx)));
  return h('section.view-artifact', null, opts.head === false ? null : artifactHead(a), grid);
}
