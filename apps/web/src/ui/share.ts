// The Share dialog, for an artifact or a Library file. One switch makes the target reachable by
// link. While it is shared, the dialog shows both links, the expiry, password, pinned version and
// download settings, and the activity. Agents ask to share through Approvals; approving creates
// the share on the hub, so nothing here is needed for that.

import '../styles/share.css';
import { shares, type ShareEvent, type ShareTarget, type ShareView } from '../api-shares';
import { ago, button, card, field, input, modal, select } from './components';
import { h, replace } from './dom';

/** What the dialog shares: an artifact or a Library file, and the version on screen, if there is one. */
export interface ShareSubject {
  kind: 'artifact' | 'file';
  id: string;
  title: string;
  version?: number;
}

const DAY_MS = 86_400_000;

/** The expiry choices; a relative one counts from the moment it is picked. */
const EXPIRY: { value: string; label: string; days?: number }[] = [
  { value: 'never', label: 'Never' },
  { value: '1', label: '1 day', days: 1 },
  { value: '7', label: '7 days', days: 7 },
  { value: '30', label: '30 days', days: 30 },
  { value: 'date', label: 'On a date…' },
];

const EVENT_LABEL: Record<ShareEvent['kind'], string> = { view: 'Viewed', download: 'Downloaded', unlock_failed: 'Wrong password' };
const VIA_LABEL: Record<ShareEvent['via'], string> = { relay: 'agentplaza.io', network: 'your network' };

const pad = (n: number): string => String(n).padStart(2, '0');

/** A local calendar date as a date input reads it: `YYYY-MM-DD`. */
const dateValue = (ms: number): string => {
  const d = new Date(ms);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
};

/** The last millisecond of a picked local date, so the share lasts through that whole day. */
const endOfDay = (value: string): number => {
  const [y = 1970, m = 1, d = 1] = value.split('-').map(Number);
  return new Date(y, m - 1, d, 23, 59, 59, 999).getTime();
};

const messageOf = (err: unknown): string => (err instanceof Error ? err.message : String(err));

/**
 * Copies a link from the click itself, which the browser requires. A refused copy selects the link
 * in its box, so the owner can copy it by hand.
 */
function copyLink(value: string, box: HTMLInputElement, btn: HTMLButtonElement, label: string): void {
  const manual = (): void => {
    box.focus();
    box.select();
  };
  const flash = (): void => {
    btn.textContent = 'Copied';
    window.setTimeout(() => {
      btn.textContent = label;
    }, 1500);
  };
  try {
    void navigator.clipboard.writeText(value).then(flash, manual);
  } catch {
    manual();
  }
}

/** Keeps Tab inside the open dialog, so focus does not wander onto the page behind the backdrop. */
function trapFocus(root: HTMLElement, e: KeyboardEvent): void {
  const box = root.closest<HTMLElement>('.sheet');
  if (e.key !== 'Tab' || !box) return;
  const items = [...box.querySelectorAll<HTMLElement>('button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled])')];
  const first = items[0];
  const last = items[items.length - 1];
  if (!first || !last) return;
  const active = document.activeElement;
  if (e.shiftKey && (active === first || !box.contains(active))) {
    e.preventDefault();
    last.focus();
  } else if (!e.shiftKey && (active === last || !box.contains(active))) {
    e.preventDefault();
    first.focus();
  }
}

/** Opens the Share dialog. It reads the target's live share, and every change it makes is the hub's answer. */
export function openShareDialog(subject: ShareSubject): void {
  const target: ShareTarget = { kind: subject.kind, id: subject.id };
  const root = h('div.share-dialog', { 'aria-busy': 'true' }, h('p.hint', null, 'Loading…'));
  // The live share (null while private), its recent events, and the drafts the owner has typed.
  let current: ShareView | null = null;
  let events: ShareEvent[] = [];
  let expired = false;
  let expiryChoice: string | null = null;
  let password = '';
  let loaded = false;
  let busy = false;
  let closed = false;
  let focusSwitch = true;
  let error = '';

  const onKey = (e: KeyboardEvent): void => trapFocus(root, e);
  modal(`Share ${subject.title}`, root, () => {
    closed = true;
    document.removeEventListener('keydown', onKey);
  });
  document.addEventListener('keydown', onKey);

  const load = async (): Promise<void> => {
    const all = await shares.list(target);
    current = all.find((s) => s.state === 'live') ?? null;
    expired = current === null && all.some((s) => s.state === 'expired');
    events = current ? await shares.events(current.id, 20) : [];
    expiryChoice = null;
  };

  const paint = (): void => {
    if (closed || !loaded) return;
    replace(root, body());
    root.setAttribute('aria-busy', String(busy));
    if (focusSwitch) {
      focusSwitch = false;
      root.querySelector<HTMLElement>('.tgl')?.focus();
    }
  };

  // One change to the share: it runs, then the dialog reloads what the hub now holds. A failure shows inline.
  const run = async (action: () => Promise<unknown>): Promise<void> => {
    if (busy) return;
    busy = true;
    error = '';
    paint();
    try {
      await action();
      await load();
    } catch (err) {
      error = messageOf(err);
    }
    busy = false;
    paint();
  };

  const linkRow = (label: string, value: string, copyLabel: string): HTMLElement => {
    const box = input({ readonly: true, 'aria-label': label });
    box.value = value;
    const copy = button(copyLabel, () => copyLink(value, box, copy, copyLabel), { kind: 'ghost', size: 'sm' });
    return h('div.field', null, h('span.field-l', null, label), h('div.cluster', null, box, copy));
  };

  const settings = (share: ShareView): HTMLElement => {
    const pinned = share.version ?? subject.version ?? null;
    const choice = expiryChoice ?? (share.expires_ms === null ? 'never' : 'date');

    const expiry = select(EXPIRY, choice);
    expiry.addEventListener('change', () => {
      expiryChoice = expiry.value;
      const days = EXPIRY.find((o) => o.value === expiry.value)?.days;
      if (days) void run(() => shares.update(share.id, { expires_ms: Date.now() + days * DAY_MS }));
      else if (expiry.value === 'never') void run(() => shares.update(share.id, { expires_ms: null }));
      else paint();
    });

    const dateBox = input({ type: 'date', min: dateValue(Date.now()), 'aria-label': 'Expiry date' });
    dateBox.value = share.expires_ms === null ? '' : dateValue(share.expires_ms);
    dateBox.addEventListener('change', () => {
      if (!dateBox.value) return;
      const ms = endOfDay(dateBox.value);
      if (ms <= Date.now()) {
        error = 'Pick a date after today.';
        paint();
        return;
      }
      void run(() => shares.update(share.id, { expires_ms: ms }));
    });

    const pw = input({ type: 'password', autocomplete: 'new-password', 'aria-label': 'Password', placeholder: share.has_password ? 'New password' : 'Password' });
    pw.value = password;
    pw.addEventListener('input', () => {
      password = pw.value;
    });
    const setPassword = button(share.has_password ? 'Change' : 'Set password', () => {
      if (!password) {
        error = 'Type a password first.';
        paint();
        return;
      }
      const next = password;
      void run(async () => {
        await shares.update(share.id, { password: next });
        password = '';
      });
    }, { kind: 'ghost', size: 'sm' });
    const removePassword = share.has_password ? button('Remove', () => void run(() => shares.update(share.id, { password: null })), { kind: 'ghost', size: 'sm' }) : null;

    const version = select(
      pinned === null ? [{ value: 'latest', label: 'Latest' }] : [{ value: 'latest', label: 'Latest' }, { value: 'pinned', label: `This version (v${pinned})` }],
      share.version === null ? 'latest' : 'pinned',
    );
    version.addEventListener('change', () => void run(() => shares.update(share.id, { version: version.value === 'pinned' ? pinned : null })));

    const download = h('input', { type: 'checkbox' });
    download.checked = share.allow_download;
    download.addEventListener('change', () => void run(() => shares.update(share.id, { allow_download: download.checked })));

    return h(
      'div.stack',
      null,
      field('Expires', expiry),
      choice === 'date' ? field('Date', dateBox, 'The link stops working at the end of this day.') : null,
      field(
        'Password',
        h('div.cluster', null, pw, setPassword, removePassword),
        share.has_password ? 'Visitors need this password to open the link.' : 'Anyone with the link can open it.',
      ),
      field('Version', version, 'Latest shows the newest version of the target.'),
      h('label.check', null, download, h('span', null, 'Allow downloading data and files')),
    );
  };

  const activity = (share: ShareView): HTMLElement =>
    card('Activity', [
      h('p.hint.num', null, `${share.views} views · ${share.downloads} downloads`),
      events.length
        ? h('ol.notes', null, events.map((e) => h('li', null, `${EVENT_LABEL[e.kind]} · via ${VIA_LABEL[e.via]} · ${ago(e.at_ms)}`)))
        : h('p.hint', null, 'Nobody has opened this link yet.'),
    ]);

  const body = (): (Node | null)[] => {
    const share = current;
    const toggle = h(
      'button.tgl',
      { type: 'button', role: 'switch', 'aria-checked': share ? 'true' : 'false', 'aria-label': share ? 'Anyone with the link (can view)' : 'Private (you only)', disabled: busy },
    );
    toggle.addEventListener('click', () => void run(() => (share ? shares.revoke(share.id) : shares.create({ target, version: null, allow_download: false }))));
    const note = share
      ? 'Turning this off stops sharing and revokes both links at once.'
      : expired
        ? 'This link has expired. Turning sharing on makes a new link.'
        : 'Only you can open this.';
    const top = h(
      'div.stack',
      null,
      h('div.cluster', null, toggle, h('span', null, share ? 'Anyone with the link (can view)' : 'Private (you only)')),
      h('p.hint', null, note),
    );
    const errorLine = error ? h('p.notice', { role: 'alert' }, error) : null;
    if (!share) return [top, errorLine];
    return [
      top,
      h('div.stack', null, linkRow('Link', share.links.public, 'Copy link'), linkRow('Network link', share.links.network, 'Copy network link')),
      settings(share),
      activity(share),
      errorLine,
    ];
  };

  void (async () => {
    try {
      await load();
    } catch (err) {
      error = messageOf(err);
    }
    loaded = true;
    root.setAttribute('aria-busy', 'false');
    paint();
  })();
}
