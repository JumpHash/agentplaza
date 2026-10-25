// A project's Previews tab: the lanes of the project that run an app on a node. Each preview says what it is (its lane, its
// node and port, awake, asleep or closed), opens its app in a new tab, and can be put to sleep, woken or closed. A preview
// that has not ended can be shared: a link with a scope, a label and an expiry, shown with its address and its QR code.
// Each preview lists its links (a revoke on the owner's word), and each link lists its openings. The rules are in
// previews-model.ts. Archived projects are read only. The API is api-projects.ts (previewsApi).

import { previewsApi, type Preview, type PreviewLink, type PreviewOpening, type PreviewScope, type PreviewShare } from '../api-projects';
import type { Project } from '../api';
import { ago, attempt, badge, button, field, input, modal, select, toast } from '../ui/components';
import { h, lu, replace } from '../ui/dom';
import { DAYS_DEFAULT, DAYS_MAX, DAYS_MIN, LABEL_MAX, LINK_STATE, SCOPE_CHOICES, SCOPE_LABEL, STATUS, linkSeconds } from './previews-model';

/** A moment as the owner reads it: a date and a time in the browser's locale. */
const when = (ms: number): string => new Date(ms).toLocaleString();

/** A yes or no in a dialog. `onYes` runs only on the yes. */
function confirmModal(title: string, text: string, label: string, onYes: () => void): void {
  const acts = h('div.pv-dlg-acts');
  let close = (): void => {};
  replace(
    acts,
    button('Keep', () => close(), { kind: 'ghost' }),
    button(
      label,
      () => {
        close();
        onYes();
      },
      { kind: 'danger' },
    ),
  );
  close = modal(title, h('p', null, text), undefined, [acts]);
}

/** Copies a link's address. Where the browser refuses, the address is selected for copying by hand. */
async function copyLink(url: HTMLInputElement): Promise<void> {
  try {
    await navigator.clipboard.writeText(url.value);
    toast('Link copied');
  } catch {
    url.select();
    toast('The browser would not copy it: the address is selected, copy it by hand', 'error');
  }
}

/** Makes a link to `pv` and shows it: the address (read only, with a Copy button) and its QR code. `onMade` runs once the link exists. */
function openShare(pv: Preview, onMade: () => void): void {
  const scope = select(SCOPE_CHOICES, 'view');
  const label = input({ maxlength: LABEL_MAX, placeholder: 'For review', autocomplete: 'off' });
  const days = input({ type: 'number', min: DAYS_MIN, max: DAYS_MAX, value: DAYS_DEFAULT });
  const body = h('div.pv-share');
  const acts = h('div.pv-dlg-acts');
  let close = (): void => {};

  const showResult = (share: PreviewShare): void => {
    const url = input({ value: share.url, readonly: true, 'aria-label': 'Address of the link', autocomplete: 'off' });
    const copy = button('Copy', () => void copyLink(url), { kind: 'ghost', size: 'sm' });
    const who = share.link.scope === 'use' ? 'use the app' : 'watch the app';
    const until = share.link.expires_ms ? ` until ${when(share.link.expires_ms)}` : '';
    replace(
      body,
      h('p', null, `Anyone with this address can ${who}${until}. Revoke the link to stop it.`),
      field('Address', h('div.pv-share-row', null, url, copy)),
      h('img.pv-qr', { src: share.qr, alt: 'QR code of the preview link', width: 200, height: 200 }),
    );
    replace(acts, button('Done', () => close(), { kind: 'primary' }));
    onMade();
  };

  const create = async (): Promise<void> => {
    const seconds = linkSeconds(Number(days.value));
    if (seconds === undefined) {
      toast(`The link lasts from ${DAYS_MIN} to ${DAYS_MAX} whole days`, 'error');
      return;
    }
    const share = await attempt(() =>
      previewsApi.share(pv.id, { scope: scope.value as PreviewScope, label: label.value.trim(), expires_in_s: seconds }),
    );
    if (share) showResult(share);
  };

  replace(
    body,
    field('Scope', scope, 'Watch lets the link view the app and nothing it changes. Use lets the link use the app.'),
    field('Label', label, 'Optional. Shown in the list of links.'),
    field('Expires in days', days, `From ${DAYS_MIN} to ${DAYS_MAX} days.`),
  );
  replace(acts, button('Cancel', () => close(), { kind: 'ghost' }), button('Create link', () => void create(), { kind: 'primary' }));
  close = modal(`Share ${pv.lane}`, body, undefined, [acts]);
}

/** The line of a link's state and time: when it was revoked or ends, or when it was made. */
function linkWhen(l: PreviewLink): string {
  if (l.state === 'revoked' && l.revoked_ms) return `revoked ${ago(l.revoked_ms)}`;
  if (l.expires_ms) return `${l.state === 'expired' ? 'expired' : 'expires'} ${when(l.expires_ms)}`;
  return `made ${ago(l.created_ms)}`;
}

/** The openings of a link: a disclosure that loads them the first time it opens. The hub's order is kept (newest first). */
function openingsBox(l: PreviewLink): HTMLElement {
  const body = h('div.pv-ops-b');
  let loaded = false;
  const details = h('details.pv-ops', null, h('summary', null, 'Openings'), body);
  details.addEventListener('toggle', () => {
    if (!details.open || loaded) return;
    loaded = true;
    void (async () => {
      const got: PreviewOpening[] | undefined = await attempt(() => previewsApi.openings(l.id));
      if (got === undefined) {
        loaded = false;
        replace(body, h('p.pj-faint', null, 'The openings could not be loaded.'));
        return;
      }
      replace(
        body,
        got.length
          ? got.map((o) => h('div.pv-op', null, h('time', null, when(o.at_ms)), h('code', null, o.method), h('span', null, o.from || 'unknown'), h('code', null, o.path)))
          : h('p.pj-faint', null, 'No one has opened this link yet.'),
      );
    })();
  });
  return details;
}

/** The links of a preview: a disclosure that loads them the first time it opens. `reload` brings them up to date. */
function linksBox(pv: Preview, writable: boolean): { el: HTMLElement; reload: () => void } {
  const body = h('div.pv-links-b');
  let links: PreviewLink[] | undefined;
  let failed = false;
  const details = h('details.pv-links', null, h('summary', null, 'Share links'), body);

  const draw = (): void => {
    if (links === undefined) {
      replace(body, h('p.pj-faint', null, failed ? 'The links could not be loaded.' : 'Loading the links…'));
      return;
    }
    if (!links.length) {
      replace(body, h('p.pj-faint', null, 'No links yet.'));
      return;
    }
    replace(body, links.map(linkRow));
  };

  const load = async (): Promise<void> => {
    const got = await attempt(() => previewsApi.links(pv.id));
    failed = got === undefined;
    if (got) links = got;
    draw();
  };

  const linkRow = (l: PreviewLink): HTMLElement => {
    const state = LINK_STATE[l.state];
    const revoke = (): void =>
      confirmModal('Revoke this link?', 'Its address stops working at once, for everyone it was sent to.', 'Revoke', () => {
        void (async () => {
          if (await attempt(() => previewsApi.revoke(l.id), 'Link revoked')) void load();
        })();
      });
    return h(
      'div.pv-link',
      null,
      h(
        'div.pv-link-h',
        null,
        h('strong', null, l.label || 'No label'),
        badge(SCOPE_LABEL[l.scope], 'muted'),
        badge(state.label, state.tone),
        h('span.pj-faint', null, linkWhen(l)),
      ),
      h('div.pv-link-a', null, writable && l.state === 'active' ? button('Revoke', revoke, { kind: 'danger', size: 'sm' }) : null, openingsBox(l)),
    );
  };

  details.addEventListener('toggle', () => {
    if (details.open && links === undefined) void load();
  });
  return {
    el: details,
    // Open now: fetch again. Shut: forget the copy, so the next opening fetches it.
    reload: () => {
      if (details.open) void load();
      else links = undefined;
    },
  };
}

/** The Previews tab of project `p`. It loads the previews and redraws after each action; the links load when opened. */
export function previewsTab(p: Project): HTMLElement {
  const root = h('div.pj-tab');
  const writable = !p.archived;
  /** Undefined until the first answer. A failed reload keeps the list it had. */
  let list: Preview[] | undefined;
  let failed = false;

  const reload = async (): Promise<void> => {
    const got = await attempt(() => previewsApi.list(p.id));
    failed = got === undefined;
    if (got) list = got;
    draw();
  };

  /** Runs a preview's action, then brings the list up to date; `done` is the toast of its success. */
  const act = (run: () => Promise<Preview>, done: string): void => {
    void (async () => {
      if (await attempt(run, done)) await reload();
    })();
  };

  const previewCard = (pv: Preview): HTMLElement => {
    const status = STATUS[pv.status];
    const ended = pv.status === 'closed';
    const links = linksBox(pv, writable);
    const actions: Node[] = [
      button('Sleep', () => act(() => previewsApi.sleep(pv.id), `${pv.lane} is asleep`), { kind: 'ghost', size: 'sm' }),
      button('Wake', () => act(() => previewsApi.wake(pv.id), `${pv.lane} is awake`), { kind: 'ghost', size: 'sm' }),
      button(
        'Close',
        () =>
          confirmModal(`Close ${pv.lane}?`, 'Its app stops and the preview ends now, before its lane does. Its share links stop working.', 'Close', () =>
            act(() => previewsApi.close(pv.id), `${pv.lane} is closed`),
          ),
        { kind: 'danger', size: 'sm' },
      ),
      button('Share', () => openShare(pv, links.reload), { kind: 'primary', size: 'sm' }),
    ];
    return h(
      'article.pv-card',
      null,
      h('header.pv-h', null, h('h2', null, pv.lane), h('span.pv-chips', null, badge(status.label, status.tone, pv.status === 'awake'))),
      h(
        'dl.pv-kvg',
        null,
        h('dt', null, 'Node'),
        h('dd', null, pv.node),
        h('dt', null, 'Port'),
        h('dd', null, String(pv.port)),
        h('dt', null, 'Started'),
        h('dd', null, ago(pv.created_ms)),
        h('dt', null, 'Last used'),
        h('dd', null, ago(pv.used_ms)),
        h('dt', null, 'Ends'),
        h('dd', null, pv.ends_ms ? when(pv.ends_ms) : 'No end set'),
      ),
      ended ? null : h('a.pv-open', { href: pv.path, target: '_blank', rel: 'noopener' }, lu('external-link', 14), h('span', null, `Open ${pv.path}`)),
      h('div.pv-body', null, links.el),
      writable && !ended ? h('footer.pv-acts', null, ...actions) : null,
    );
  };

  const intro = (): HTMLElement =>
    h('div.pv-intro', null, h('p', null, 'The lanes of this project that run an app on a node. Open one, put it to sleep or wake it, or share it by a link that watches it or uses it.'));

  function draw(): void {
    let rest: Node[];
    if (list === undefined) {
      rest = [h('p.pj-faint', null, failed ? 'The previews could not be loaded. Try again in a moment.' : 'Loading the previews…')];
    } else if (!list.length) {
      rest = [h('div.empty', null, lu('box', 22), h('b', null, 'No previews yet'), h('span', null, 'A preview is a lane’s app running on a node. It shows here once a lane runs one.'))];
    } else {
      rest = [h('div.pv-list', null, list.map(previewCard))];
    }
    replace(root, intro(), rest);
  }

  draw();
  void reload();
  return root;
}
