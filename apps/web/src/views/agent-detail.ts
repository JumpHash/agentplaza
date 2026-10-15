// Agent detail: one agent's transcript, its composer, and its model, details and queue.

import '../styles/agent.css';
import { api, ApiError, isContributor, type Agent, type AgentStatus, type Block, type Delta, type Entry, type ModelInfo, type MoveResult, type NodeView, type Sanction, type SanctionsView, type Settings, type Suspension, type VmCheckpoint } from '../api';
import { go, href } from '../router';
import { openIde } from '../ide/open';
import { prewake } from '../prewake';
import { agentName } from '../roles';
import { agents, on, refreshAgents } from '../store';
import { ago, attempt, badge, button, card, clip, compact, empty, field, input, modal, tabs, textarea, toast, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { mountMemory } from './memory';
import { profileCard } from './agent-profile';
import { harnessCard } from './agent-harness';
import { bytesText } from './workstations-model';
import { choicesFrom, modelPicker, type ModelChoice } from '../ui/model-picker';
import { embedContextFromStore } from '../ui/embeds';
import { renderMarkdown } from '../ui/markdown';

const NEAR_BOTTOM_PX = 80;

type Tier = 'orchestrator' | 'worker';

/**
 * The model choices for a tier. A worker may run any model: the policy's worker list comes
 * first as presets, then every model the providers list. An orchestrator may run only its
 * policy list. `current` (an override no longer offered) is kept as a choice.
 */
/** Reasoning effort levels a model name can pin (`provider/model@effort`); '' = the model's default. */
const EFFORTS: [string, string][] = [
  ['', 'Model default'],
  ['none', 'None'],
  ['minimal', 'Minimal'],
  ['low', 'Low'],
  ['medium', 'Medium'],
  ['high', 'High'],
  ['xhigh', 'Extra high'],
  ['max', 'Max'],
];
const effortOf = (value: string): string => {
  const at = value.lastIndexOf('@');
  const e = at < 0 ? '' : value.slice(at + 1);
  return EFFORTS.some(([v]) => v === e) ? e : '';
};
const stripEffort = (value: string): string => (effortOf(value) ? value.slice(0, value.lastIndexOf('@')) : value);
const withEffort = (value: string, effort: string): string => (value ? (effort ? `${stripEffort(value)}@${effort}` : value) : '');

export function tierChoices(
  tier: Tier,
  settings: Settings,
  models: ModelInfo[],
  inherit?: { label: string },
  current?: string,
): ModelChoice[] {
  const names = new Map(models.map((m) => [`${m.provider}/${m.id}`, m.name ?? m.id] as [string, string]));
  const policy = settings.model_policy[tier];
  const out = choicesFrom(policy, names, inherit);
  if (tier === 'worker') {
    for (const c of out) if (c.value && policy.includes(c.value)) c.preset = true;
    const offered = new Set(out.map((c) => c.value));
    for (const m of models) {
      const value = `${m.provider}/${m.id}`;
      if (offered.has(value)) continue;
      offered.add(value);
      out.push({ value, label: m.name ?? m.id, provider: m.provider });
    }
  }
  if (current && !out.some((c) => c.value === current)) {
    out.push({ value: current, label: current, provider: current.split('/')[0] ?? '' });
  }
  const trainers = new Set(models.filter(isContributor).map((m) => `${m.provider}/${m.id}`));
  for (const c of out) c.trains = trainers.has(c.value.replace(/@.*$/, ''));
  return out;
}

type Section = { root: HTMLElement; head: HTMLElement };
type LiveTurn = { work: number; el: HTMLElement; text: Text; thinking: Text };
type Tab = 'transcript' | 'memory';

/** The nodes an agent's VM can move to: those that take work, so not draining and not revoked. */
export function movableNodes(nodes: NodeView[]): NodeView[] {
  return nodes.filter((n) => !n.draining && !n.revoked);
}

/** The outcome of a move as one line: where the VM went, the MB it carried and the seconds it took (`ms` in). */
export function moveText(r: Pick<MoveResult, 'from' | 'to' | 'bytes' | 'ms'>, nameOf: (id: string) => string): string {
  return `Moved from ${nameOf(r.from)} to ${nameOf(r.to)}: ${(r.bytes / 1048576).toFixed(1)} MB in ${(r.ms / 1000).toFixed(1)} s.`;
}

export function mountAgentDetail(root: HTMLElement, cid: number): Cleanup {
  const head = h('header.agent-head');
  const transcript = h('div.transcript');
  const composer = textarea({ placeholder: 'Message this agent…', rows: 3 });
  const send = button('Send', () => void submit(), { kind: 'primary', icon: 'send', title: 'Send (Ctrl+Enter)' });
  // The agent's model override ('' = the default), offered by its tier, and its reasoning
  // effort, stored together as `provider/model@effort`.
  const saveModel = (value: string): void => {
    void attempt(() => api.setAgentModel(cid, value || null), 'Model updated').then((ok) => {
      if (ok) appliedOverride = value;
      syncModel(appliedOverride ?? '');
    });
  };
  // The values the picker offers, so a pinned effort shows on its preset when there is one.
  let offered = new Set<string>();
  const picker = modelPicker({
    choices: [],
    label: 'Change model',
    placeholder: 'Use the default',
    onChange: (value) => saveModel(withEffort(value, effortSel.value)),
  });
  const effortSel = h(
    'select.select',
    { 'aria-label': 'Reasoning effort' },
    EFFORTS.map(([value, label]) => h('option', { value }, label)),
  ) as HTMLSelectElement;
  effortSel.addEventListener('change', () => {
    // Effort on the default model pins that model for this agent.
    const base = stripEffort(picker.value || (agent ? agent.model : ''));
    if (base) saveModel(withEffort(base, effortSel.value));
  });
  // Shows `value` in both controls: the picker gets the choice it matches (with or without
  // the effort pin), the effort control the pin.
  const syncModel = (value: string): void => {
    const effort = effortOf(value);
    picker.value = offered.has(value) ? value : stripEffort(value);
    effortSel.value = effort;
  };
  const modelLine = h('div.model-line');
  const details = h('div');
  const queue = h('div.stack');

  // The transcript and the memory library share one space; a tab picks which shows.
  const memoryHost = h('div.memory-host', { style: 'display:none' });
  const transcriptCard = card(null, transcript, { flush: true });
  const composerRow = h('div.composer', null, composer, h('div.row', null, h('span', { style: 'flex:1' }), send));
  const tabLabels: Record<Tab, string> = { transcript: 'Transcript', memory: 'Memory' };
  let tab: Tab = 'transcript';
  let unmountMemory: (() => void) | null = null;
  const showTab = (next: Tab): void => {
    if (next === tab) return;
    tab = next;
    const memory = next === 'memory';
    transcriptCard.style.display = memory ? 'none' : '';
    composerRow.style.display = memory ? 'none' : '';
    memoryHost.style.display = memory ? '' : 'none';
    unmountMemory?.();
    unmountMemory = memory ? mountMemory(memoryHost, cid) : null;
    tabBar.querySelectorAll('button').forEach((b) => b.setAttribute('aria-selected', String(b.textContent === tabLabels[next])));
  };
  const tabBar = tabs(
    [
      { key: 'transcript', label: tabLabels.transcript },
      { key: 'memory', label: tabLabels.memory },
    ],
    tab,
    (key) => showTab(key as Tab),
  );

  const sections = new Map<number, Section>();
  const live = new Map<string, LiveTurn>();
  const pending: (() => void)[] = [];
  let agent: Agent | undefined;
  // The profile card (portrait, money, standing, conduct), made once the agent is known.
  const profileHost = h('div');
  let profile: ReturnType<typeof profileCard> | null = null;
  const profileTimer = window.setInterval(() => void profile?.refresh(), 15_000);
  let alive = true;
  let loaded = false;
  let sending = false;
  let lastSeq = 0;
  let appliedOverride: string | undefined;

  const keepBottom = (change: () => void): void => {
    const stick = transcript.scrollHeight - transcript.scrollTop - transcript.clientHeight < NEAR_BOTTOM_PX;
    change();
    if (stick) transcript.scrollTop = transcript.scrollHeight;
  };

  const sectionOf = (work: number): Section => {
    const existing = sections.get(work);
    if (existing) return existing;
    const sectionHead = h('header.work-head');
    const sectionRoot = h('section.work', null, sectionHead);
    const created = { root: sectionRoot, head: sectionHead };
    sections.set(work, created);
    transcript.append(sectionRoot);
    return created;
  };

  const removeLive = (work: number): void => {
    for (const [key, turn] of live) {
      if (turn.work !== work) continue;
      turn.el.remove();
      live.delete(key);
    }
  };

  const renderEntry = (e: Entry): void => {
    if (e.seq <= lastSeq) return;
    lastSeq = e.seq;
    if (e.kind === 'model_request') return;
    const d = e.data;
    keepBottom(() => {
      const section = sectionOf(e.work);
      switch (e.kind) {
        case 'work_start':
          replace(section.head, badge(String(d['kind'] ?? 'work'), 'info'), h('p.work-prompt', null, String(d['prompt'] ?? '')));
          break;
        case 'work_end': {
          const s = String(d['status'] ?? '');
          const tone = /fail|error/.test(s) ? 'danger' : /cancel|stop/.test(s) ? 'warn' : 'ok';
          section.root.append(h('div.work-end', null, badge(s || 'done', tone)));
          break;
        }
        case 'model_response':
          removeLive(e.work);
          section.root.append(...responseNodes(d));
          break;
        case 'tool_decision': {
          const line = decisionBadge(d);
          if (line) section.root.append(h('div.decision', null, line));
          break;
        }
        case 'tool_result': {
          const failed = d['is_error'] === true;
          const body = String(d['content'] ?? d['output'] ?? '');
          section.root.append(h('details.result', null, h('summary', { class: failed ? 'danger' : 'ok' }, failed ? 'Error' : 'Output'), h('pre.pre', null, body)));
          break;
        }
        case 'error':
          section.root.append(h('p.danger', null, String(d['message'] ?? 'Error')));
          break;
        default:
          break;
      }
    });
  };

  const onDelta = (raw: unknown): void => {
    const d = raw as Delta;
    if (d.cid !== cid) return;
    const key = `${d.work}:${d.req}`;
    keepBottom(() => {
      let turn = live.get(key);
      if (!turn) {
        const text = new Text();
        const thinking = new Text();
        const el = h('div.live-turn', null, h('p.prose', null, text));
        turn = { work: d.work, el, text, thinking };
        live.set(key, turn);
        sectionOf(d.work).root.append(el);
      }
      const part = d.delta;
      if (part.type === 'tool_use') turn.el.append(h('span.chip', null, `→ ${part.name}`));
      else if (part.type === 'text') turn.text.appendData(part.text);

    });
  };

  const loadHistory = async (): Promise<void> => {
    const history = await attempt(() => api.transcriptLast(cid));
    if (!alive) return;
    for (const e of (history ?? []).slice().sort((x, y) => x.seq - y.seq)) renderEntry(e);
    loaded = true;
    for (const run of pending.splice(0)) run();
    transcript.scrollTop = transcript.scrollHeight;
  };

  const populateModels = (models: ModelInfo[], settings: Settings): void => {
    const current = appliedOverride ?? '';
    const choices = tierChoices(agent?.tier ?? 'worker', settings, models, { label: 'Use the default' }, current);
    offered = new Set(choices.map((c) => c.value));
    picker.setChoices(choices);
    syncModel(current);
  };

  // Head actions share one busy state: while a request runs every head button is disabled,
  // and a render during the request keeps them disabled.
  let busy = false;
  const setHeadBusy = (on: boolean): void => {
    head.querySelectorAll<HTMLButtonElement>('.agent-actions button').forEach((b) => (b.disabled = on));
  };
  const act = async (run: () => Promise<unknown>): Promise<void> => {
    busy = true;
    setHeadBusy(true);
    try {
      await run();
    } finally {
      busy = false;
      setHeadBusy(false);
    }
  };
  const restart = (a: Agent): void => {
    if (!window.confirm(`Restart ${agentName(a)}? Its VM restarts.`)) return;
    void act(() => attempt(() => api.restartAgent(cid), 'Restarting'));
  };
  const retire = (a: Agent): void => {
    if (!window.confirm(`Retire ${agentName(a)}? Its VM stops for good and it leaves the roster.`)) return;
    void act(async () => {
      const ok = await attempt(async () => {
        await api.retireAgent(cid);
        return true;
      }, 'Retired');
      if (ok) go('agents');
    });
  };
  // One labelled checkbox for the wipe dialog: the box (read on confirm) and its row.
  const checkRow = (label: string, checked: boolean, disabled = false): [HTMLInputElement, HTMLElement] => {
    const box = h('input', { type: 'checkbox' }) as HTMLInputElement;
    box.checked = checked;
    box.disabled = disabled;
    return [box, h('label.check', null, box, h('span', null, label))];
  };
  const wipe = (a: Agent): void => {
    const [, workspaceRow] = checkRow('Workspace (always)', true, true);
    const [memoryBox, memoryRow] = checkRow('Memory notes', false);
    const [transcriptBox, transcriptRow] = checkRow('Conversation transcript', false);
    const [identityBox, identityRow] = checkRow('Name and reputation', false);
    let close: () => void = () => {};
    const confirmBtn = button('Wipe', () => void run(), { kind: 'danger' });
    const run = async (): Promise<void> => {
      confirmBtn.disabled = true;
      const ok = await attempt(async () => {
        await api.wipeAgent(cid, { memory: memoryBox.checked, transcript: transcriptBox.checked, identity: identityBox.checked });
        return true;
      }, 'Wiped');
      confirmBtn.disabled = false;
      if (ok) close();
    };
    close = modal(
      `Wipe ${agentName(a)}`,
      h(
        'div.stack',
        null,
        h('p.muted', null, "The agent's VM restarts with a fresh workspace. This cannot be undone."),
        workspaceRow,
        memoryRow,
        transcriptRow,
        identityRow,
        h('div.row.modal-actions', null, confirmBtn),
      ),
    );
  };

  // Move: the VM goes to another node that takes work. The dialog lists those nodes and posts the move. A 409 (the
  // agent is working, or the node is draining or revoked) is shown as the hub says it. The move can take seconds.
  const moveDialog = (a: Agent): void => {
    const list = h('div.stack', null, h('p.muted', null, 'Loading the nodes...'));
    const problem = h('p.mcp-danger');
    const outcome = h('div');
    const moveBtn = button('Move', () => void run(), { kind: 'primary', icon: 'send', disabled: true });
    let chosen: string | null = null;
    let nodeNames = new Map<string, string>();
    let close: () => void = () => {};
    const run = async (): Promise<void> => {
      const target = chosen;
      if (target === null) return;
      moveBtn.disabled = true;
      replace(problem);
      try {
        const moved = await api.moveAgent(cid, target);
        replace(outcome, h('p', null, moveText(moved, (id) => nodeNames.get(id) ?? id)));
        void refreshAgents();
      } catch (err) {
        replace(problem, err instanceof Error ? err.message : 'The move failed.');
        moveBtn.disabled = false;
      }
    };
    close = modal(
      `Move ${agentName(a)}`,
      h(
        'div.stack',
        null,
        h('p.muted', null, "The agent's VM moves to the node you pick. It can take seconds, and an agent that is working cannot move."),
        list,
        problem,
        outcome,
        h('div.row.modal-actions', null, moveBtn, button('Close', () => close(), { kind: 'ghost' })),
      ),
    );
    api
      .nodes()
      .then((all) => {
        nodeNames = new Map(all.map((n) => [n.id, n.name || n.id]));
        const nodes = movableNodes(all);
        if (!nodes.length) {
          replace(list, h('p.muted', null, 'No node takes work right now: each one is draining or revoked.'));
          return;
        }
        replace(
          list,
          nodes.map((n) => {
            const el = input({ type: 'radio', name: `move-node-${cid}`, value: n.id });
            el.addEventListener('change', () => {
              if (!el.checked) return;
              chosen = n.id;
              moveBtn.disabled = false;
            });
            return h('label.check', null, el, h('span', null, n.name || n.id, n.online ? null : ' (offline)'));
          }),
        );
      })
      .catch((err: unknown) => replace(list, h('p.mcp-danger', null, err instanceof Error ? err.message : 'The nodes could not load.')));
  };

  // A sleep or a wake changes the agent's status, so the roster is read again straight away.
  const sleepAgent = (): void =>
    void act(async () => {
      await attempt(() => api.sleepAgent(cid), 'Agent asleep');
      void refreshAgents();
    });
  const wakeAgent = (): void =>
    void act(async () => {
      await attempt(() => api.wakeAgent(cid), 'Agent awake');
      void refreshAgents();
    });

  // The machine checkpoints: read on mount and after each change. While one request runs, every row action and the
  // take button wait; a failed read says so instead of showing an empty list.
  let checkpoints: VmCheckpoint[] | null = null;
  let checkpointsFailed = false;
  let checkpointBusy = false;
  const checkpointHost = h('div.stack');
  const checkpointLabel = input({ placeholder: 'Label (optional)', 'aria-label': 'Checkpoint label' });
  const takeBtn = button('Take checkpoint', () => takeCheckpoint(), { icon: 'plus' });
  const checkpointCard = card(
    'Machine checkpoints',
    h('div.stack', null, checkpointHost, h('div.row', null, checkpointLabel, takeBtn)),
  );
  checkpointLabel.addEventListener('keydown', (e) => {
    if (e.key !== 'Enter') return;
    e.preventDefault();
    takeCheckpoint();
  });
  // A checkpoint shows its label, or its name when it has none.
  const checkpointName = (cp: VmCheckpoint): string => cp.label || cp.name;

  const renderCheckpoints = (): void => {
    takeBtn.disabled = checkpointBusy;
    const list = checkpoints ?? [];
    const body: Node =
      checkpointsFailed
        ? h('span.faint', null, 'Could not load the checkpoints')
        : checkpoints === null
          ? h('span.faint', null, 'Loading…')
          : list.length === 0
            ? h('span.faint', null, 'No checkpoints yet')
            : h('div.stack', null, ...list.map(checkpointRow));
    replace(checkpointHost, body);
  };
  const loadCheckpoints = async (): Promise<void> => {
    try {
      checkpoints = (await api.vmCheckpoints(cid)).checkpoints;
      checkpointsFailed = false;
    } catch {
      checkpointsFailed = true;
    }
    if (alive) renderCheckpoints();
  };
  const checkpointAct = async (run: () => Promise<unknown>): Promise<void> => {
    checkpointBusy = true;
    renderCheckpoints();
    try {
      await run();
    } finally {
      checkpointBusy = false;
      await loadCheckpoints();
    }
  };
  const takeCheckpoint = (): void => {
    void checkpointAct(async () => {
      const made = await attempt(() => api.takeVmCheckpoint(cid, checkpointLabel.value.trim()), 'Checkpoint taken');
      if (made) checkpointLabel.value = '';
    });
  };
  const restoreCheckpoint = (cp: VmCheckpoint): void => {
    if (!window.confirm(`Roll back ${checkpointName(cp)} to this checkpoint? Its VM — memory, disk and running processes — returns to that moment. A safety checkpoint is taken first, so you can undo this.`)) return;
    void checkpointAct(() => attempt(() => api.restoreVmCheckpoint(cid, cp.name), 'Rolled back'));
  };
  const deleteCheckpoint = (cp: VmCheckpoint): void => {
    if (!window.confirm(`Delete the checkpoint ${checkpointName(cp)}? It cannot be restored afterwards.`)) return;
    void checkpointAct(() => attempt(() => api.deleteVmCheckpoint(cid, cp.name), 'Checkpoint deleted'));
  };
  const checkpointRow = (cp: VmCheckpoint): HTMLElement =>
    h(
      'div.stack.checkpoint',
      { 'data-key': cp.name },
      h('div.row', null, h('strong.truncate', { title: cp.name }, checkpointName(cp)), badge(cp.kind)),
      h(
        'span.faint',
        null,
        `${ago(cp.taken_at_ms)} · ${bytesText(cp.memory_written)} memory written · ${bytesText(cp.disk_bytes)} disk · paused ${cp.paused_ms} ms`,
      ),
      h(
        'div.row',
        null,
        button('Restore', () => restoreCheckpoint(cp), { size: 'sm', icon: 'refresh', disabled: checkpointBusy }),
        button('Delete', () => deleteCheckpoint(cp), { kind: 'danger', size: 'sm', icon: 'trash', disabled: checkpointBusy }),
      ),
    );

  // A fork takes a fresh checkpoint and starts each new agent from it. The count is 1 to FORK_MAX, whole.
  const FORK_MAX = 8;
  const fork = (a: Agent): void => {
    const count = input({ type: 'number', min: 1, max: FORK_MAX, step: 1, value: 1, 'aria-label': 'Number of forks' });
    const forkCount = (): number | null => {
      const n = Number(count.value);
      return count.value.trim() !== '' && Number.isInteger(n) && n >= 1 && n <= FORK_MAX ? n : null;
    };
    const forkBtn = button('Fork', () => void run(), { kind: 'primary', icon: 'copy' });
    const result = h('div.stack');
    let close: () => void = () => {};
    count.addEventListener('input', () => {
      forkBtn.disabled = forkCount() === null;
    });
    const run = async (): Promise<void> => {
      const n = forkCount();
      if (n === null) return;
      forkBtn.disabled = true;
      const made = await attempt(() => api.forkAgent(cid, n), 'Forked');
      forkBtn.disabled = forkCount() === null;
      if (!made) return;
      void refreshAgents();
      void loadCheckpoints();
      const links = made.forks.map((f) => {
        const link = h('a', { href: href('agents', f.cid) }, f.name || `agent ${f.cid}`);
        // The link opens the new agent's page, so the dialog closes with it.
        link.addEventListener('click', () => close());
        return h('div', null, link);
      });
      replace(
        result,
        h('p.muted', null, `${made.forks.length === 1 ? 'The new agent starts' : 'The new agents start'} from a checkpoint of ${agentName(a)}:`),
        h('div.stack', null, ...links),
      );
    };
    close = modal(
      `Fork ${agentName(a)}`,
      h(
        'div.stack',
        null,
        h('p.muted', null, "Each fork gets a copy of this agent's VM, taken from a fresh checkpoint, and its own name. The forks then work apart from this agent and from each other."),
        field(`Number of forks (1–${FORK_MAX})`, count),
        h('div.row.modal-actions', null, forkBtn),
        result,
      ),
    );
  };

  const renderAgent = (): void => {
    const a = agent;
    if (!a) return;
    if (!profile) {
      profile = profileCard(a);
      profileHost.replaceChildren(profile.el);
    } else profile.update(a);
    const offline = a.status === 'offline';
    harness.update(a.status === 'working');
    replace(
      head,
      h('a.back', { href: href('agents') }, 'Agents'),
      h('div.agent-title', null, statusBadge(a.status)),
      h(
        'div.row.agent-actions',
        null,
        button('Open workspace', () => { prewake(cid, a.status); openIde(`agent:${cid}`); }, { icon: 'square-code', title: "Open this agent's workspace in the IDE" }),
        a.status === 'working' ? button('Cancel work', () => void act(() => attempt(() => api.cancel(cid), 'Cancelled')), { icon: 'ban', disabled: busy }) : null,
        a.status === 'asleep'
          ? button('Wake', () => wakeAgent(), { icon: 'sun', disabled: busy })
          : button('Sleep', () => sleepAgent(), { icon: 'moon', title: 'Pause the VM until a chat or task wakes it', disabled: busy || offline || a.status === 'working' }),
        button('Fork…', () => fork(a), { icon: 'copy', disabled: busy }),
        button('Move…', () => moveDialog(a), { icon: 'send', title: 'Move the VM to another node', disabled: busy || a.status === 'working' }),
        button('Restart', () => restart(a), { disabled: busy }),
        button('Stop', () => void act(() => attempt(() => api.stop(cid), 'Stopped')), { kind: 'danger', icon: 'stop', disabled: busy }),
        button('Wipe…', () => wipe(a), { disabled: busy }),
        button('Retire', () => retire(a), { kind: 'danger', disabled: busy }),
      ),
    );

    composer.disabled = offline;
    send.disabled = offline || sending;

    const source = a.model_source === 'agent' ? 'set for this agent' : a.model_source === 'role' ? 'from role' : 'default';
    replace(modelLine, h('strong', null, a.model), h('span.faint', null, source));
    const want = a.model_override ?? '';
    if (want !== appliedOverride) {
      appliedOverride = want;
      syncModel(want);
    }

    replace(
      details,
      h(
        'dl.kv',
        null,
        h('dt', null, 'CID'),
        h('dd.mono', null, String(a.cid)),
        h('dt', null, 'Run'),
        h('dd.mono', null, a.run ?? '—'),
        h('dt', null, 'VM'),
        h('dd.mono', null, a.vm === null ? '—' : String(a.vm)),
        h('dt', null, 'Connected'),
        h('dd', null, ago(a.connected_at_ms)),
        h('dt', null, 'Last seen'),
        h('dd', null, ago(a.last_seen_ms)),
        h('dt', null, 'Tokens in'),
        h('dd.mono', null, compact(a.usage.input_tokens)),
        h('dt', null, 'Tokens out'),
        h('dd.mono', null, compact(a.usage.output_tokens)),
      ),
    );

    replace(
      queue,
      a.work ? h('div.row', null, badge(a.work.kind, 'info'), h('span.truncate', null, clip(a.work.prompt, 160))) : h('span.faint', null, 'Nothing running'),
      h('span.muted', null, `${a.queue} queued`),
    );
  };

  const isOffline = (): boolean => agent?.status === 'offline';

  const submit = async (): Promise<void> => {
    const text = composer.value.trim();
    if (!text || sending || isOffline()) return;
    sending = true;
    send.disabled = true;
    const ok = await attempt(() => api.chat(cid, text));
    sending = false;
    send.disabled = isOffline();
    if (ok) composer.value = '';
  };

  composer.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      void submit();
    }
  });

  // The agent's permission mode: loaded on mount, saved on click.
  type PermissionMode = 'autonomous' | 'ask' | 'read_only';
  const permissionOptions: { mode: PermissionMode; label: string; hint: string }[] = [
    { mode: 'autonomous', label: 'Autonomous', hint: 'acts on its own' },
    { mode: 'ask', label: 'Ask first', hint: 'write and irreversible actions wait for your approval in Approvals' },
    { mode: 'read_only', label: 'Read-only', hint: 'can look, not change' },
  ];
  const permissionHost = h('div.stack');
  const harness = harnessCard(cid);
  const conduct = conductCard(cid);
  let permission: PermissionMode | null = null;
  let permissionFailed = false;
  const renderPermission = (): void => {
    // One full-width choice per mode, its hint under its name, so the card fits the side column.
    replace(
      permissionHost,
      h(
        'div.perm-options',
        { role: 'radiogroup', 'aria-label': 'Permission mode' },
        ...permissionOptions.map((o) => {
          const choice = h(
            'button.perm-option',
            { type: 'button', role: 'radio', 'aria-checked': String(o.mode === permission) },
            h('span.perm-label', null, o.label),
            h('span.perm-hint', null, o.hint),
          );
          if (o.mode === permission) choice.classList.add('active');
          choice.addEventListener('click', () => choosePermission(o.mode));
          return choice;
        }),
      ),
      permissionFailed ? h('span.faint', null, 'Could not load this setting') : permission === null ? h('span.faint', null, 'Loading…') : null,
    );
  };
  const choosePermission = (next: PermissionMode): void => {
    if (permission === null || next === permission) return;
    void attempt(() => api.setPermissions(cid, next), 'Permissions updated').then((ok) => {
      if (!alive) return;
      if (ok) permission = next;
      renderPermission();
    });
  };
  renderPermission();
  void api
    .permissions(cid)
    .then((view) => {
      if (!alive) return;
      permission = view.mode;
      renderPermission();
    })
    .catch(() => {
      if (!alive) return;
      permissionFailed = true;
      renderPermission();
    });

  const offs = [
    agents.watch((list) => {
      const found = list.find((a) => a.cid === cid);
      if (!found) return;
      agent = found;
      renderAgent();
    }),
    on('transcript', (raw) => {
      const e = raw as Entry;
      if (e.cid !== cid) return;
      if (loaded) renderEntry(e);
      else pending.push(() => renderEntry(e));
    }),
    on('delta', (raw) => {
      if ((raw as Delta).cid !== cid) return;
      if (loaded) onDelta(raw);
      else pending.push(() => onDelta(raw));
    }),
  ];

  root.append(
    h(
      'div.agent-view',
      null,
      head,
      profileHost,
      h(
        'div.agent-grid',
        null,
        h(
          'div.agent-main',
          null,
          tabBar,
          transcriptCard,
          memoryHost,
          composerRow,
        ),
        h(
          'aside.agent-side',
          null,
          card('Model', h('div.stack', null, modelLine, field('Change model', picker.el), field('Reasoning effort', effortSel))),
          card('Harness', harness.el),
          card('Permissions', permissionHost),
          card('Conduct', conduct.el),
          checkpointCard,
          card('Details', details),
          card('Queue', queue),
        ),
      ),
    ),
  );

  const start = async (): Promise<void> => {
    try {
      agent = await api.agent(cid);
    } catch (err) {
      if (!alive) return;
      replace(
        root,
        err instanceof ApiError && err.status === 404
          ? empty('agents', 'No such agent', undefined, h('a.btn', { href: href('agents') }, 'All agents'))
          : empty('alert', 'Could not load this agent', err instanceof Error ? err.message : undefined),
      );
      return;
    }
    if (!alive) return;
    renderAgent();
    void loadCheckpoints();
    void Promise.all([api.models(), api.settings()])
      .then(([view, settings]) => alive && populateModels(view.models, settings))
      .catch(() => undefined);
    await loadHistory();
  };
  void start();

  return () => {
    alive = false;
    harness.destroy();
    conduct.destroy();
    window.clearInterval(profileTimer);
    offs.forEach((off) => off());
    live.clear();
    unmountMemory?.();
    unmountMemory = null;
  };
}

/**
 * The Conduct section: the agent's suspensions in force, each with a Lift, then its sanctions on the
 * register, newest first. Read from the sanctions register; refreshed every 15 s and after a lift.
 */
function conductCard(cid: number): { el: HTMLElement; destroy: () => void } {
  const body = h('div.stack');
  let view: SanctionsView | null = null;
  let failed = false;
  let error: string | null = null;
  let busy = false;
  let alive = true;

  const sanctionRow = (r: Sanction): HTMLElement =>
    h(
      'li.conduct-row',
      { title: r.title },
      h('div.conduct-row-head', null, h('strong', null, r.sanction), h('code.conduct-law', null, r.law), badge(r.severity, severityTone(r.severity))),
      h(
        'div.conduct-row-meta',
        null,
        h('span', null, `offence ${r.count}`),
        h('span', null, r.outcome),
        h('span', null, r.fine > 0 ? `fined ${compact(r.fine)} cr` : 'no fine'),
        h('time', null, ago(r.at_ms)),
      ),
      r.lifted ? h('p.conduct-lifted', null, `lifted by owner: ${r.lifted.why}`) : null,
      r.owner_action
        ? h('p.conduct-lifted', null, `${r.owner_action.action === 'retire' ? 'Retired' : 'Acknowledged'} by the owner, ${ago(r.owner_action.at_ms)}`)
        : null,
    );

  const suspensionRow = (s: Suspension, now: number): HTMLElement =>
    h(
      'li.conduct-susp',
      null,
      h('div.conduct-susp-main', null, h('code', null, s.tool), h('span.faint', null, s.law)),
      h(
        'div.conduct-susp-side',
        null,
        h('span.faint', null, timeLeft(s.until_ms, now)),
        button('Lift', () => void liftSuspension(s), { size: 'sm', disabled: busy }),
      ),
    );

  const render = (): void => {
    if (!view) {
      replace(body, failed ? h('p.danger', null, 'Could not load conduct.') : h('p.faint', null, 'Loading…'));
      return;
    }
    const now = Date.now();
    const suspensions = view.suspensions.filter((s) => s.cid === cid && s.active).sort((x, y) => x.until_ms - y.until_ms);
    const sanctions = view.sanctions.filter((r) => r.cid === cid).sort((x, y) => y.at_ms - x.at_ms);
    replace(
      body,
      h('h3.conduct-h', null, 'Suspended now'),
      suspensions.length ? h('ul.conduct-list', null, suspensions.map((s) => suspensionRow(s, now))) : h('p.faint', null, 'Not suspended.'),
      error ? h('p.danger', null, error) : null,
      h('h3.conduct-h', null, 'Sanctions'),
      sanctions.length ? h('ul.conduct-list', null, sanctions.map((r) => sanctionRow(r))) : h('p.faint', null, 'No sanctions on record.'),
    );
  };

  const refresh = async (): Promise<void> => {
    try {
      view = await api.sanctions({ cid });
      failed = false;
    } catch {
      failed = true;
    }
    if (alive) render();
  };

  const liftSuspension = async (s: Suspension): Promise<void> => {
    const answer = window.prompt(`Lift the suspension of ${s.tool} (${s.law})? Give a reason.`);
    if (answer === null) return;
    const reason = answer.trim();
    error = null;
    if (!reason) {
      error = 'Give a reason to lift a suspension.';
      render();
      return;
    }
    busy = true;
    render();
    try {
      await api.liftSanction(s.id, reason);
      toast('Suspension lifted');
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    }
    busy = false;
    await refresh();
  };

  render();
  void refresh();
  const timer = window.setInterval(() => void refresh(), 15_000);
  return {
    el: body,
    destroy: () => {
      alive = false;
      window.clearInterval(timer);
    },
  };
}

/** Time left until `untilMs`, as "2h 5m left". */
/** The status word in the head. Asleep is muted, not a warning: the agent is paused on purpose and wakes on a chat or task. */
function statusBadge(status: AgentStatus): HTMLElement {
  if (status === 'working') return badge('Working', 'info', true);
  if (status === 'idle') return badge('Idle', 'ok', true);
  if (status === 'asleep') return badge('Asleep', 'muted', true);
  return badge('Offline', 'muted', true);
}

function timeLeft(untilMs: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((untilMs - now) / 1000));
  if (s < 60) return `${s}s left`;
  if (s < 3600) return `${Math.floor(s / 60)}m left`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m left`;
  return `${Math.floor(s / 86400)}d ${Math.floor((s % 86400) / 3600)}h left`;
}

/** A sanction's severity as a badge tone: the serious ones red, the middling ones amber. */
function severityTone(severity: string): Tone {
  if (/crit|high|severe/i.test(severity)) return 'danger';
  if (/med|moderate/i.test(severity)) return 'warn';
  return 'muted';
}

function responseNodes(data: Record<string, unknown>): Node[] {
  const blocks = Array.isArray(data['content']) ? (data['content'] as Block[]) : [];
  const out: Node[] = [];
  for (const b of blocks) {
    if (b.type === 'text') out.push(h('div.prose', null, renderMarkdown(b.text, { embeds: embedContextFromStore() })));
    else if (b.type === 'tool_use') out.push(h('details.tool', null, h('summary.chip', null, `→ ${b.name}`), h('pre.pre', null, JSON.stringify(b.input, null, 2))));
  }
  return out;
}

function decisionBadge(d: Record<string, unknown>): HTMLElement | null {
  switch (d['decision']) {
    case 'allow':
      return badge('ran in VM', 'info');
    case 'done':
      return badge('ran on host', 'muted');
    case 'deny':
      return badge(`denied: ${String(d['reason'] ?? '')}`, 'danger');
    default:
      return null;
  }
}
