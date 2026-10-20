// The share viewer: the page a link opens at /artifact/<share>, on a hub or on agentplaza.io. It is read-only and draws the
// artifact through the console's own renderers (view/), so it carries no console code: no router, no store, no API client.
// Every URL is absolute on the page's own origin, so the same bundle works on both origins.

import './styles/tokens.css';
import './styles/proto/base.css';
import './styles/proto/skins.css';
import './styles/viewer.css';
import { renderArtifact } from './view/artifact';
import { renderCode } from './view/code';
import { renderDataset } from './view/dataset';
import { liveFrame } from './view/frame';
import { renderGraph } from './view/graph';
import type { ViewCtx } from './view/bodies';
import type { Artifact, DatasetBody, HtmlBody, RowsPage } from './view/model';
import { h, replace } from './ui/dom';

interface ShareInfo {
  id: string;
  title: string;
  kind: string;
  allow_download: boolean;
  version: number | null;
  latest: number;
}

interface ThemeInfo {
  skin: string;
  palette: string;
  mode: 'light' | 'dark';
  css?: string;
}

interface ViewerData {
  share: ShareInfo;
  artifact: Artifact;
  theme: ThemeInfo;
}

interface RowsQuery {
  offset: number;
  limit: number;
  sort?: string;
  desc?: boolean;
  filter?: string;
}

const PUBLIC_HOST = 'agentplaza.io';
const RELAY_REPORT_URL = 'https://agentplaza.io/relay/report';
const CUSTOM_STYLE_ID = 'apz-viewer-theme';

const MESSAGES = {
  notFound: ['Link not found', 'This link does not exist or was turned off.'],
  expired: ['Link expired', 'This link has expired.'],
  offline: ['Plaza offline', 'The plaza that shared this is offline right now.'],
  failed: ['Could not load', 'Something went wrong loading this link. Try again in a moment.'],
} as const;

// The share id is the segment after /artifact/. A page without one shows the not-found state.
const match = /^\/artifact\/([^/]+)\/?$/.exec(location.pathname);
const base = match ? `/artifact/${match[1]}` : '';
const app = document.getElementById('app') as HTMLElement;

function show(node: Node): void {
  replace(app, node);
}

function message(text: readonly [string, string]): HTMLElement {
  return h('div.empty', null, h('b', null, text[0]), h('span', null, text[1]));
}

/** Sets the theme on the root (the tokens' selectors read it) and injects the owner's custom CSS when there is some. */
function applyTheme(theme: ThemeInfo): void {
  const root = document.documentElement;
  root.dataset.skin = theme.skin;
  root.dataset.palette = theme.palette;
  root.dataset.mode = theme.mode === 'light' ? 'light' : 'dark';
  let style = document.getElementById(CUSTOM_STYLE_ID);
  if (theme.css) {
    if (!style) {
      style = document.createElement('style');
      style.id = CUSTOM_STYLE_ID;
      document.head.append(style);
    }
    style.textContent = theme.css;
  } else {
    style?.remove();
  }
}

/** The rows of a dataset panel, one page at a time. */
async function fetchRows(panelId: string, q: RowsQuery): Promise<RowsPage> {
  const params = new URLSearchParams({ panel: panelId, offset: String(q.offset), limit: String(q.limit) });
  if (q.sort) params.set('sort', q.sort);
  if (q.desc) params.set('desc', '1');
  if (q.filter) params.set('filter', q.filter);
  const res = await fetch(`${base}/rows?${params.toString()}`, { credentials: 'same-origin', cache: 'no-store' });
  if (!res.ok) throw new Error(`Rows failed with HTTP ${res.status}.`);
  return (await res.json()) as RowsPage;
}

/**
 * An HTML panel's frame, drawn by the console's frame manager: the share's own frame path, and its still when it has one.
 * The frame is an opaque origin (no allow-same-origin), so it reaches this page only through the bridge's messages.
 */
function frameFor(panelId: string, body: HtmlBody): HTMLElement {
  // The page is opened by name so that its relative paths resolve under the panel (the hub serves the entry there).
  return liveFrame(() => Promise.resolve(`${base}/frame/${encodeURIComponent(panelId)}/index.html`), body, {
    artifactId: match?.[1] ?? '',
    panelId,
    stillUrl: body.still ? `${base}/blobs/${encodeURIComponent(body.still.id)}` : undefined,
  });
}

function viewCtx(): ViewCtx {
  return {
    blobUrl: (blobId) => `${base}/blobs/${encodeURIComponent(blobId)}`,
    rows: fetchRows,
    frame: frameFor,
    code: renderCode,
    dataset: (panelId: string, body: DatasetBody) => renderDataset(panelId, body, fetchRows),
    graph: renderGraph,
  };
}

/** One download link per dataset panel and format, when the owner allowed downloads. */
function exportLinks(panels: Artifact['panels']): HTMLElement[] {
  const datasets = panels.filter((p) => p.body.type === 'dataset');
  const many = datasets.length > 1;
  const links: HTMLElement[] = [];
  for (const p of datasets) {
    for (const format of ['csv', 'json'] as const) {
      const label = many ? `${p.title} ${format.toUpperCase()}` : `Download ${format.toUpperCase()}`;
      links.push(
        h('a.btn.sm.ghost', {
          href: `${base}/export?format=${format}&panel=${encodeURIComponent(p.id)}`,
          download: '',
        }, label),
      );
    }
  }
  return links;
}

/** The report form, shown only on agentplaza.io: the hub has no relay to send it to. */
function reportBlock(shareId: string): HTMLElement {
  const reason = h('textarea.textarea', {
    name: 'reason',
    required: true,
    maxlength: 500,
    rows: 3,
    'aria-label': 'Why are you reporting this link?',
  }) as HTMLTextAreaElement;
  const send = h('button.btn.sm.primary', { type: 'submit' }, 'Send report');
  const note = h('p.hint', { role: 'status' });
  const form = h('form.stack', null, reason, h('div.cluster', null, send), note);
  form.addEventListener('submit', (e) => {
    e.preventDefault();
    const text = reason.value.trim();
    if (!text) return;
    send.disabled = true;
    fetch(RELAY_REPORT_URL, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ id: shareId, reason: text }),
    })
      .then((res) => {
        if (!res.ok) throw new Error(`Report failed with HTTP ${res.status}.`);
        reason.value = '';
        note.textContent = 'Thanks. The report was sent.';
      })
      .catch(() => {
        note.textContent = 'The report could not be sent. Try again later.';
        send.disabled = false;
      });
  });
  return h('details.viewer-report.card', null, h('summary', null, 'Report this link'), h('div.card-b', null, form));
}

function render(data: ViewerData): void {
  applyTheme(data.theme);
  const { share, artifact } = data;
  document.title = `${share.title} · AgentPlaza`;
  const version = share.version ?? artifact.version;
  const exports = share.allow_download ? exportLinks(artifact.panels) : [];
  show(
    h(
      'div.page.viewer-page',
      null,
      h(
        'header.page-head',
        null,
        h(
          'div.page-head-row',
          null,
          h('span.viewer-mark', null, 'AgentPlaza'),
          h('span.hint', null, `Shared from a plaza · v${version}`),
          h('span.sp'),
          exports.length ? h('nav.page-actions', null, exports) : null,
        ),
      ),
      h('main.viewer-main', null, renderArtifact(artifact, viewCtx())),
      location.host === PUBLIC_HOST ? h('footer.viewer-foot', null, reportBlock(share.id)) : null,
    ),
  );
}

function passwordView(): HTMLElement {
  const input = h('input.input', {
    type: 'password',
    name: 'password',
    autocomplete: 'current-password',
    required: true,
    'aria-label': 'Password',
  }) as HTMLInputElement;
  const button = h('button.btn.primary', { type: 'submit' }, 'Unlock');
  const error = h('p.hint.err', { role: 'alert' });
  const form = h('form.card.viewer-password', null, h('div.card-b', null, h('p', null, 'This link is protected by a password.'), h('div.field', null, h('span.field-l', null, 'Password'), input), h('div.cluster', null, button), error));
  form.addEventListener('submit', (e) => {
    e.preventDefault();
    button.disabled = true;
    error.textContent = '';
    fetch(`${base}/unlock`, {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ password: input.value }),
    })
      .then((res) => {
        if (res.status === 204) return boot();
        error.textContent = res.status === 401 || res.status === 403
          ? 'That password is not right.'
          : 'The link could not be unlocked. Try again.';
        button.disabled = false;
        input.select();
        return undefined;
      })
      .catch(() => {
        error.textContent = 'The link could not be unlocked. Try again.';
        button.disabled = false;
      });
  });
  return form;
}

/** Reads the share's data and shows it, or the state the hub answered with. */
async function boot(): Promise<void> {
  if (!match) return show(message(MESSAGES.notFound));
  show(h('p.hint', null, 'Loading the artifact…'));
  let res: Response;
  try {
    res = await fetch(`${base}/data`, { credentials: 'same-origin', cache: 'no-store' });
  } catch {
    return show(message(MESSAGES.offline));
  }
  if (res.status === 401) return show(passwordView());
  if (res.status === 404) return show(message(MESSAGES.notFound));
  if (res.status === 410) return show(message(MESSAGES.expired));
  if (res.status === 503) return show(message(MESSAGES.offline));
  if (!res.ok) return show(message(MESSAGES.failed));
  let data: ViewerData;
  try {
    data = (await res.json()) as ViewerData;
  } catch {
    return show(message(MESSAGES.failed));
  }
  render(data);
}

void boot();
