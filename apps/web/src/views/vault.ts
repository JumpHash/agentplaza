// Vault screen: the page head, the status card, a toolbar (Add secret, Import .env) and three tabs
// (Secrets, Per node, Audit). Each reload re-fetches everything and rebuilds the page from the same data;
// the sections return fresh elements on every call. The page's children are the screen's children.

import '../styles/proto/settings-c.css';
import { vaultApi } from '../api';
import { button, card, tabs } from '../ui/components';
import { h, replace } from '../ui/dom';
import { on } from '../store';
import { errorText, type VaultCtx, type VaultData } from './vault-common';
import { vaultStatusSection } from './vault-status';
import { type SecretScope, vaultSecretsSection } from './vault-secrets';
import { vaultMatrixSection } from './vault-matrix';
import { vaultAuditSection } from './vault-audit';
import { openImportDialog } from './vault-import';
import { addSecretDialog } from './vault-form';
import { dim, pageHead } from './vault-ui';
import type { Cleanup } from './index';

type VaultTab = 'secrets' | 'matrix' | 'audit';

const TABS: { key: VaultTab; label: string }[] = [
  { key: 'secrets', label: 'Secrets' },
  { key: 'matrix', label: 'Per node' },
  { key: 'audit', label: 'Audit' },
];

/** One change can emit several live events; a single reload covers the burst. */
const RELOAD_DEBOUNCE_MS = 250;

function isTab(key: string): key is VaultTab {
  return TABS.some((t) => t.key === key);
}

/** Status first. Entries and consumers are fetched only when the vault is unsealed. */
async function fetchData(): Promise<VaultData> {
  const status = await vaultApi.status();
  if (status.state !== 'unsealed') return { status, entries: [], consumers: [] };
  const [entries, consumers] = await Promise.all([vaultApi.entries(), vaultApi.consumers()]);
  return { status, entries, consumers };
}

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  let alive = true;
  let tab: VaultTab = 'secrets';
  // Kept here, not in the section, because every draw rebuilds the section.
  let secretScope: SecretScope = { kind: 'all' };
  let ctx: VaultCtx | undefined;
  let failure: string | undefined;
  let loads = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;

  function toolbar(c: VaultCtx): HTMLElement {
    const title = c.locked ? 'Unseal the vault first.' : undefined;
    const add = button('Add secret', () => addSecretDialog({}, () => void c.reload()), {
      kind: 'primary',
      title,
      disabled: c.locked,
    });
    const imp = button('Import .env', () => openImportDialog(c), {
      kind: 'ghost',
      title,
      disabled: c.locked,
    });
    return h('div.vlt-bar', null, add, imp);
  }

  function tabBar(): HTMLElement {
    return tabs(TABS, tab, (key) => {
      if (!isTab(key)) return;
      tab = key;
      draw();
    });
  }

  function tabContent(c: VaultCtx): Node[] {
    if (tab === 'matrix') return [vaultMatrixSection(c)];
    if (tab === 'audit') return [vaultAuditSection(c)];
    return vaultSecretsSection(c, secretScope, (next) => {
      secretScope = next;
      draw();
    });
  }

  function draw(): void {
    if (!alive) return;
    if (failure !== undefined) {
      replace(
        root,
        pageHead('Vault'),
        card('', [
          h('div.vlt-err', { role: 'alert' }, `The vault could not load. ${failure}`),
          button('Try again', () => void reload(), { kind: 'primary' }),
        ]),
      );
      return;
    }
    if (!ctx) {
      replace(root, pageHead('Vault'), card('', dim('Loading the vault...')));
      return;
    }
    const c = ctx;
    replace(root, pageHead('Vault'), vaultStatusSection(c), toolbar(c), tabBar(), ...tabContent(c));
  }

  async function reload(): Promise<void> {
    const seq = ++loads;
    try {
      const data = await fetchData();
      if (!alive || seq !== loads) return;
      ctx = { data, locked: data.status.state !== 'unsealed', reload };
      failure = undefined;
    } catch (err) {
      if (!alive || seq !== loads) return;
      failure = errorText(err);
    }
    draw();
  }

  function scheduleReload(): void {
    if (timer !== undefined) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = undefined;
      void reload();
    }, RELOAD_DEBOUNCE_MS);
  }

  const offs = [on('vault.changed', scheduleReload), on('vault.audit_broken', scheduleReload)];

  draw();
  void reload();

  return () => {
    alive = false;
    if (timer !== undefined) clearTimeout(timer);
    for (const off of offs) off();
  };
}
