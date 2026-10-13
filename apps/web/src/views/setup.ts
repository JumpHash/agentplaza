// Setup: the first-run checks of the desktop app (apps/desktop/src/console/setup and boot.rs),
// run here through the hub's API. Rows a browser cannot see (Docker, the containers, the ports,
// the hypervisor device of this machine) are marked as checked by the desktop app.

import '../styles/setup.css';
import { api, ApiError, vaultApi, type VaultStatus } from '../api';
import { href } from '../router';
import { badge, button, card, type Tone } from '../ui/components';
import { h } from '../ui/dom';
import type { Cleanup } from './index';

type Status = 'checking' | 'ok' | 'warning' | 'failed' | 'desktop';
interface Fix { text: string; to?: string }
interface Row { label: string; status: Status; detail: string; fix?: Fix }

const STAMP: Record<Status, string> = { checking: 'CHECKING', ok: 'OK', warning: 'WARNING', failed: 'FAILED', desktop: 'DESKTOP' };
const TONE: Record<Status, Tone> = { checking: 'muted', ok: 'ok', warning: 'warn', failed: 'danger', desktop: 'info' };
const WEB_LABELS = ['Hub reachable', 'Hub identity', 'Hub service', 'Node here', 'Network', 'Machine and hypervisor', 'Vault', 'Model providers', 'Default model', 'Git host'];

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;

function desktop(label: string, detail: string): Row {
  return { label, status: 'desktop', detail: `${detail}. Checked by the AgentPlaza app here, since a browser cannot see this machine.` };
}

function settled<T>(result: PromiseSettledResult<T>): T | null {
  return result.status === 'fulfilled' ? result.value : null;
}

function vaultRow(v: VaultStatus): Row {
  const label = 'Vault';
  if (v.state === 'uninitialized') return { label, status: 'warning', detail: 'not created yet: secrets have no encrypted home', fix: { text: 'Create the vault', to: 'vault' } };
  if (v.state === 'sealed') return { label, status: 'warning', detail: 'sealed: agents cannot read secrets until it is unsealed', fix: { text: 'Unseal the vault', to: 'vault' } };
  if (v.state === 'tampered') return { label, status: 'failed', detail: `tampered${v.tampered ? `: ${v.tampered}` : ''}`, fix: { text: 'Read the audit log and recover', to: 'vault' } };
  const envKeys = v.env_file_keys ?? 0;
  if (envKeys > 0) return { label, status: 'warning', detail: `${plural(envKeys, 'key')} still in the env file, not in the vault`, fix: { text: 'Import them into the vault', to: 'vault' } };
  if (!v.audit_ok) return { label, status: 'warning', detail: 'unsealed, but the audit chain does not verify', fix: { text: 'Check the audit log', to: 'vault' } };
  return { label, status: 'ok', detail: `unsealed: ${plural(v.entries, 'secret')}, ${plural(v.slots.length, 'unlock slot')}` };
}

/** Asks the hub for every fact the web can check; never throws. */
async function collect(): Promise<Row[]> {
  const [over, ident, mach, vaultState, providers, git, link] = await Promise.allSettled([
    api.overview(),
    api.hubIdentity(),
    api.machine(),
    vaultApi.status(),
    api.providers(),
    api.git(),
    api.nodeLink(),
  ]);
  const overview = settled(over);
  const rows: Row[] = [];

  if (overview) {
    rows.push({ label: 'Hub reachable', status: 'ok', detail: `apid answers, ${plural(overview.agents.total, 'agent')} on this plaza` });
  } else if (over.status === 'rejected' && over.reason instanceof ApiError && over.reason.status === 401) {
    rows.push({ label: 'Hub reachable', status: 'warning', detail: 'the hub wants the owner signed in', fix: { text: 'Sign in with the owner token' } });
  } else {
    rows.push({ label: 'Hub reachable', status: 'failed', detail: 'apid does not answer', fix: { text: 'Start AgentPlaza on this machine (desktop app: Start locally), then run the checks again' } });
  }

  const identity = settled(ident);
  if (!identity) {
    rows.push({ label: 'Hub identity', status: 'failed', detail: 'the hub does not say who it is' });
  } else if (identity.fingerprint || identity.nodes === 0) {
    rows.push({ label: 'Hub identity', status: 'ok', detail: `${identity.name} ${identity.version}, ${identity.fingerprint ? 'HTTPS' : 'plain HTTP'}` });
  } else {
    rows.push({ label: 'Hub identity', status: 'warning', detail: `${identity.name} ${identity.version}, plain HTTP with ${plural(identity.nodes, 'node')} joined`, fix: { text: 'Other machines should join over HTTPS', to: 'nodes' } });
  }

  if (overview) {
    rows.push(overview.hub
      ? { label: 'Hub service', status: 'ok', detail: 'the hub serves the console and the API' }
      : { label: 'Hub service', status: 'failed', detail: 'the hub service is not running', fix: { text: 'Start the services (desktop app: Start locally)' } });
    rows.push(overview.noded
      ? { label: 'Node here', status: 'ok', detail: 'this machine runs agents' }
      : { label: 'Node here', status: 'warning', detail: 'no node on this machine: agents cannot run here', fix: { text: 'Add this machine as a node', to: 'nodes' } });
  }

  const net = settled(link);
  if (!net) {
    rows.push({ label: 'Network', status: 'warning', detail: 'could not read the node link', fix: { text: 'Open Nodes', to: 'nodes' } });
  } else if (!net.joined) {
    rows.push({ label: 'Network', status: 'ok', detail: 'this hub runs its own network' });
  } else {
    rows.push({ label: 'Network', status: net.running ? 'ok' : 'warning', detail: `joined ${net.hub ?? 'another hub'}, node ${net.running ? 'running' : 'stopped'}`, fix: { text: 'Start the node', to: 'nodes' } });
  }

  const machine = settled(mach);
  rows.push(machine
    ? { label: 'Machine and hypervisor', status: machine.backend ? 'ok' : 'warning', detail: `${machine.backend || 'no hypervisor backend'} on ${machine.arch}, ${plural(machine.cpus, 'CPU')}, ${machine.memory_mib} MiB`, fix: { text: 'Turn on virtualization in the firmware: agents run in VMs' } }
    : desktop('Machine and hypervisor', 'The hub did not report the machine; the hypervisor device (/dev/kvm, Hypervisor.framework or Windows Hypervisor Platform) is checked there'));

  const vault = settled(vaultState);
  rows.push(vault ? vaultRow(vault) : { label: 'Vault', status: 'warning', detail: 'could not read the vault', fix: { text: 'Open the Vault', to: 'vault' } });

  const list = settled(providers);
  if (!list) {
    rows.push({ label: 'Model providers', status: 'warning', detail: 'could not list the providers', fix: { text: 'Open Models', to: 'models' } });
  } else if (list.length === 0) {
    rows.push({ label: 'Model providers', status: 'failed', detail: 'none: agents cannot call a model', fix: { text: 'Add a provider and its key', to: 'models' } });
  } else {
    const missing = list.filter((p) => p.credential === 'missing').map((p) => p.id);
    rows.push(missing.length
      ? { label: 'Model providers', status: 'warning', detail: `${missing.join(', ')} without a key (of ${plural(list.length, 'provider')})`, fix: { text: 'Set the key for each one', to: 'models' } }
      : { label: 'Model providers', status: 'ok', detail: `${plural(list.length, 'provider')}, every key set` });
  }

  if (overview) {
    rows.push(overview.default_model
      ? { label: 'Default model', status: 'ok', detail: overview.default_model }
      : { label: 'Default model', status: 'warning', detail: 'none set: agents have no model by default', fix: { text: 'Pick a default model', to: 'models' } });
  }

  const gitInfo = settled(git);
  if (!gitInfo) {
    rows.push({ label: 'Git host', status: 'warning', detail: 'could not ask the git host', fix: { text: 'Open Git', to: 'git' } });
  } else if (gitInfo.configured) {
    rows.push({ label: 'Git host', status: 'ok', detail: `org ${gitInfo.org}` });
  } else {
    rows.push({ label: 'Git host', status: 'warning', detail: 'the git host is not configured', fix: { text: 'Set FORGEJO_URL and FORGEJO_TOKEN for the hub, then restart apid', to: 'git' } });
  }

  rows.push(
    desktop('AgentPlaza services', 'apid, vsockd and noded installed beside the app'),
    desktop('Docker and containers', 'Docker running, with the Postgres and Forgejo containers'),
    desktop('Ports', '7700, 8899 and 19080 free, or held by AgentPlaza itself'),
  );
  return rows;
}

function renderRow(row: Row): HTMLElement {
  const fix = row.fix && (row.status === 'warning' || row.status === 'failed') ? row.fix : null;
  return h('li.setup-row', { 'data-status': row.status },
    h('div.setup-main', null,
      h('div.setup-label', null, row.label),
      h('div.setup-detail', null, row.detail),
      fix ? (fix.to ? h('a.setup-fix', { href: href(fix.to) }, fix.text) : h('span.setup-fix', null, fix.text)) : null,
    ),
    badge(STAMP[row.status], TONE[row.status]),
  );
}

export function mount(root: HTMLElement): Cleanup {
  let alive = true;
  let seq = 0;
  const list = h('ol.setup-list');
  const summary = h('p.setup-summary');

  const paint = (rows: Row[]) => {
    list.replaceChildren(...rows.map(renderRow));
    if (rows.some((r) => r.status === 'checking')) {
      summary.textContent = 'Checking...';
      return;
    }
    const failed = rows.filter((r) => r.status === 'failed').length;
    const warned = rows.filter((r) => r.status === 'warning').length;
    summary.textContent = `${plural(failed, 'failure')}, ${plural(warned, 'item')} to set up`;
  };

  const run = async () => {
    const mine = ++seq;
    paint(WEB_LABELS.map((label) => ({ label, status: 'checking', detail: 'checking...' })));
    const rows = await collect();
    if (alive && mine === seq) paint(rows);
  };

  root.append(
    h('div.setup-view', null,
      h('div.view-head', null, h('h1', null, 'Setup')),
      h('p.setup-intro', null, 'What the plaza needs before agents can run. The checks ask this hub. Rows marked Desktop are checked by the AgentPlaza app on this machine.'),
      h('div.setup-bar', null, summary, button('Run checks again', () => void run())),
      card('Checks', list, { flush: true }),
    ),
  );
  void run();
  return () => {
    alive = false;
  };
}
