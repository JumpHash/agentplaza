import { h, replace } from '../ui/dom';
import { attempt, badge, button, card, clip, empty, table, ago, type Column } from '../ui/components';
import { vaultApi, type VaultAuditLine, type VaultBrokenEntry } from '../api';
import { auditDetail, bindingLabel, errorText, nodeName, type VaultCtx } from './vault-common';
import '../styles/vault-audit.css';

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
  const verifySlot = h('span.vault-audit-verify');
  const paintVerified = () => replace(verifySlot, verified ? badge(verified.message, verified.ok ? 'ok' : 'danger', true) : null);

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
    body.querySelector<HTMLElement>('.vault-audit-broken')?.scrollIntoView({ block: 'center' });
  }

  const columns: Column<VaultAuditLine>[] = [
    { label: 'Time', cell: (l) => h('span', { title: new Date(l.ts_ms).toLocaleString() }, ago(l.ts_ms)) },
    {
      label: 'Event',
      cell: (l) => {
        const isBroken = verified?.broken?.seq === l.seq;
        const parts = [h('span.vault-audit-event', null, l.event), h('small.vault-audit-seq', null, `#${l.seq}`), ...(isBroken ? [badge('broken', 'danger')] : [])];
        return isBroken ? h('div.vault-audit-broken', null, ...parts) : h('div', null, ...parts);
      },
    },
    { label: 'Actor', cell: (l) => l.actor || '—' },
    { label: 'Entry', cell: (l) => (l.name ? h('span.vault-audit-mono', null, l.name) : '—') },
    { label: 'Binding', cell: (l) => (l.binding ? bindingLabel(l.binding, ctx.data.consumers) : '—') },
    { label: 'Node', cell: (l) => (l.node ? nodeName(l.node, ctx.data.status.nodes) : '—') },
    { label: 'Cid', cell: (l) => l.cid ?? '—' },
    { label: 'Detail', cell: (l) => (l.detail ? h('span', { title: auditDetail(l.detail) }, clip(auditDetail(l.detail), 120)) : '—') },
  ];

  const body = h('div.vault-audit');
  let lines: VaultAuditLine[] = [];
  let after = 0;
  let hasMore = false;
  let loaded = false;
  let busy = false;
  let failed: string | null = null;

  const paint = () => {
    const nodes: Node[] = [];
    if (failed) nodes.push(empty('file', 'Could not read the audit log', failed));
    else if (!loaded) nodes.push(h('p.vault-audit-note', null, 'Loading the audit log.'));
    else if (!lines.length) nodes.push(empty('file', 'No audit events yet'));
    else {
      nodes.push(h('div.vault-audit-scroll', null, table(columns, lines, undefined, (l) => String(l.seq))));
      if (hasMore) nodes.push(h('div.vault-audit-more', null, button('Load more', onMore, { kind: 'ghost' })));
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

  paintVerified();
  if (ctx.locked) {
    replace(body, h('p.vault-audit-note', null, 'Unseal the vault to read the audit log.'));
  } else {
    paint();
    void loadPage();
  }

  const actions = h('span.vault-audit-actions', null, verifySlot, verifyBtn);
  return card('Audit log', body, { actions: [actions] });
}
