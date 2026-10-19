import { h, replace } from '../ui/dom';
import { attempt, badge, button, card, clip, empty } from '../ui/components';
import { vaultApi, type VaultAuditLine, type VaultBrokenEntry } from '../api';
import { auditDetail, bindingLabel, errorText, nodeName, type VaultCtx } from './vault-common';
import { agoText, dim, tableWrap } from './vault-ui';

const PAGE = 50;

/** The last verify result. Kept at module level so it survives the section rebuilds on each reload. */
let verified: { ok: boolean; message: string; broken: VaultBrokenEntry | null } | null = null;

/** The verify answer in words. The desktop's chain_message says the same thing. */
function chainMessage(ok: boolean, count: number | null | undefined, broken: VaultBrokenEntry | null): string {
  if (ok) return count == null ? 'Chain intact' : `Chain intact: ${count} entries verified`;
  if (!broken) return 'Chain broken';
  const at = broken.seq == null ? '' : ` (seq ${broken.seq})`;
  return `Chain broken at entry #${broken.index}${at}: ${count ?? broken.index} entries verified before it`;
}

export function vaultAuditSection(ctx: VaultCtx): HTMLElement {
  const verifySlot = h('span');
  const paintVerified = () => replace(verifySlot, verified ? badge(verified.message, verified.ok ? 'ok' : 'danger') : null);

  const verifyBtn = button('Verify chain', onVerify, {
    disabled: ctx.locked,
    title: ctx.locked ? 'Unseal the vault first.' : undefined,
  });

  async function onVerify() {
    verifyBtn.disabled = true;
    verifyBtn.textContent = 'Verifying';
    const res = await attempt(() => vaultApi.audit(0, PAGE, true));
    verifyBtn.textContent = 'Verify chain';
    verifyBtn.disabled = ctx.locked;
    if (!res) return;
    const ok = res.chain_ok === true;
    const broken = res.first_broken ?? null;
    verified = { ok, message: chainMessage(ok, res.verified, broken), broken };
    paintVerified();
    paint();
    body.querySelector<HTMLElement>('.vlt-broken')?.scrollIntoView({ block: 'center' });
  }

  const columns: { label: string; cell: (l: VaultAuditLine) => Node | string }[] = [
    { label: 'Time', cell: (l) => h('span', { title: new Date(l.ts_ms).toLocaleString() }, agoText(l.ts_ms)) },
    {
      label: 'Event',
      cell: (l) => {
        const isBroken = verified?.broken?.seq === l.seq;
        return h(
          'span',
          { class: isBroken ? 'vlt-broken' : null },
          l.event,
          ' ',
          h('span.vlt-sub.mono', null, `#${l.seq}`),
          isBroken ? ' ' : null,
          isBroken ? badge('broken', 'danger') : null,
        );
      },
    },
    { label: 'Actor', cell: (l) => l.actor || '—' },
    { label: 'Entry', cell: (l) => (l.name ? h('span.mono', null, l.name) : '—') },
    { label: 'Binding', cell: (l) => (l.binding ? bindingLabel(l.binding, ctx.data.consumers) : '—') },
    { label: 'Node', cell: (l) => (l.node ? nodeName(l.node, ctx.data.status.nodes) : '—') },
    { label: 'Cid', cell: (l) => (l.cid != null ? String(l.cid) : '—') },
    {
      label: 'Detail',
      cell: (l) => {
        const d = l.detail ? auditDetail(l.detail) : '';
        return d ? h('span', { title: d }, clip(d, 120)) : '—';
      },
    },
  ];

  const body = h('div');
  const section = card('Audit log', body, { actions: [verifySlot, verifyBtn] });
  let lines: VaultAuditLine[] = [];
  let after = 0;
  let hasMore = false;
  let loaded = false;
  let busy = false;
  let failed: string | null = null;

  const paint = () => {
    const nodes: Node[] = [];
    section.classList.remove('flush');
    if (ctx.locked) {
      nodes.push(dim('Unseal the vault to read the audit log.'));
    } else if (failed !== null) {
      nodes.push(h('div.vlt-err', { role: 'alert' }, `Could not read the audit log. ${failed}`), button('Try again', onRetry, { kind: 'ghost' }));
    } else if (!loaded) {
      nodes.push(dim('Loading the audit log.'));
    } else if (!lines.length) {
      nodes.push(empty('file', 'No audit events yet'));
    } else {
      section.classList.add('flush');
      const table = h(
        'table.tbl',
        null,
        h('thead', null, h('tr', null, columns.map((c) => h('th', null, c.label)))),
        h('tbody', null, lines.map((l) => h('tr', { 'data-key': String(l.seq) }, columns.map((c) => h('td', null, c.cell(l)))))),
      );
      nodes.push(tableWrap(table));
      if (hasMore) nodes.push(h('div.vlt-bar', null, button('Load more', onMore, { kind: 'ghost' })));
    }
    replace(body, nodes);
  };

  async function loadPage() {
    busy = true;
    try {
      const res = await vaultApi.audit(after, PAGE);
      const page = res.entries;
      lines = lines.concat(page);
      const last = page[page.length - 1];
      if (last) after = last.seq;
      hasMore = page.length === PAGE;
      failed = null;
    } catch (err) {
      failed = errorText(err);
    } finally {
      loaded = true;
      busy = false;
      paint();
    }
  }

  function onMore() {
    if (busy) return;
    void loadPage();
  }

  function onRetry() {
    if (busy) return;
    failed = null;
    loaded = false;
    paint();
    void loadPage();
  }

  paintVerified();
  if (ctx.locked) {
    paint();
  } else {
    paint();
    void loadPage();
  }

  return section;
}
