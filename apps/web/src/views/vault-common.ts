// Shared by the Vault screen's sections (vault.ts, vault-status.ts, vault-secrets.ts, vault-matrix.ts,
// vault-audit.ts, vault-import.ts, vault-form.ts). Pure helpers only: no DOM mutation outside what they return.
import type { VaultConsumer, VaultEntry, VaultNodeRef, VaultScope, VaultState, VaultStatus } from '../api';

/** This hub's own node id (the consumer matrix's first column). */
export const THIS_HUB = 'n0';

/** Env-style secret names: upper-case, digits and underscores, 2 to 64 characters. */
export const VAULT_NAME_RE = /^[A-Z][A-Z0-9_]{1,63}$/;

/** What the page loads and hands to every section. Replaced wholesale on each reload. */
export interface VaultData {
  status: VaultStatus;
  entries: VaultEntry[];
  consumers: VaultConsumer[];
}

export interface VaultCtx {
  data: VaultData;
  /** True unless the vault is unsealed: secret writes are then disabled and the sections say why. */
  locked: boolean;
  /** Re-fetches status, entries and consumers, then redraws the page. */
  reload: () => Promise<void>;
}

/** The current version of an entry (the one that resolves). */
export function currentVersion(entry: VaultEntry) {
  return entry.versions.find((v) => v.v === entry.current) ?? entry.versions[entry.versions.length - 1];
}

/** The fingerprint shown in tables and the matrix; empty when the entry has no versions. */
export function fingerprint(entry: VaultEntry): string {
  return currentVersion(entry)?.fp ?? '';
}

/** The last four characters of the current value, as the hub shows them. */
export function lastFour(entry: VaultEntry): string {
  return currentVersion(entry)?.hint ?? '';
}

/** Node name for a node id: `n0` is "this hub"; an unknown id is shown as-is. */
export function nodeName(id: string, nodes: VaultNodeRef[]): string {
  if (id === THIS_HUB) return 'this hub';
  return nodes.find((n) => n.id === id)?.name ?? id;
}

/** `plaza`, or the node ids of a `nodes` scope. */
export function scopeNodeIds(scope: VaultScope): string[] {
  return scope.kind === 'nodes' ? scope.nodes : [];
}

/** A short human label for a binding: the consumer's label when known, else a plain reading of its name. */
export function bindingLabel(binding: string, consumers: VaultConsumer[]): string {
  const known = consumers.find((c) => c.binding === binding);
  if (known?.label) return known.label;
  const [kind, rest = ''] = binding.split(/:(.*)/s);
  switch (kind) {
    case 'provider':
      return `${rest} provider`;
    case 'mcp':
      return `MCP ${rest}`;
    case 'connector':
      return `${rest} connector`;
    case 'tool':
      return `tool ${rest}`;
    case 'hf':
      return 'Hugging Face';
    default:
      return binding;
  }
}

/** The state's badge tone and the words the header shows. */
export const STATE_TONE: Record<VaultState, 'ok' | 'warn' | 'danger' | 'muted'> = {
  unsealed: 'ok',
  sealed: 'warn',
  tampered: 'danger',
  uninitialized: 'muted',
};

export const STATE_LABEL: Record<VaultState, string> = {
  unsealed: 'Unsealed',
  sealed: 'Sealed',
  tampered: 'Tampered',
  uninitialized: 'Uninitialized',
};

/** The message for an API error, never a value: the hub's own `error` text, or a fallback. */
export function errorText(err: unknown): string {
  return err instanceof Error && err.message ? err.message : 'The request failed.';
}

/** A local clock time for an epoch in ms: "14:32" today, "13 Oct 14:32" on another day. */
export function clockText(ms: number): string {
  const d = new Date(ms);
  const time = d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false });
  if (d.toDateString() === new Date().toDateString()) return time;
  return `${d.toLocaleDateString([], { day: 'numeric', month: 'short' })} ${time}`;
}

/**
 * An audit detail as the owner reads it. The hub writes epochs into some details: "4 uses in minute 29864994"
 * is epoch minutes; epoch seconds and milliseconds appear too. Each shows as a local clock time.
 */
export function auditDetail(detail: string): string {
  return detail
    .replace(/\b(\d+) uses? in minute (\d{7,9})\b/g, (_, n: string, m: string) => `${n} ${n === '1' ? 'use' : 'uses'} at ${clockText(Number(m) * 60_000)}`)
    .replace(/\bminute (\d{7,9})\b/g, (_, m: string) => `at ${clockText(Number(m) * 60_000)}`)
    .replace(/\b(\d{13})\b/g, (_, ms: string) => clockText(Number(ms)))
    .replace(/\b(\d{10})\b/g, (_, sec: string) => clockText(Number(sec) * 1000));
}
