// Agent detail: one agent's transcript, its composer, and its model, harness, permissions, conduct,
// checkpoints, details and queue, under its profile. The markup is the prototype's agent page.

import { api, ApiError, isContributor, type Agent, type AgentStatus, type Block, type Delta, type Entry, type ModelInfo, type MoveResult, type NodeView, type Sanction, type SanctionsView, type Settings, type Suspension, type VmCheckpoint } from '../api';
import { go, href } from '../router';
import { openIde } from '../ide/open';
import { prewake } from '../prewake';
import { agentName } from '../roles';
import { liveProjects, onBench, projects, projectTitle } from '../scope';
import { agents, on, refreshAgents } from '../store';
import { ago, attempt, badge, button, card, clip, compact, empty, field, input, modal, tabs, textarea, toast, type Tone } from '../ui/components';
import { h, icon, lu, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { mountMemory } from './memory';
import { profileCard } from './agent-profile';
import { harnessCard } from './agent-harness';
import { bytesText } from './workstations-model';
import { choicesFrom, modelPicker, type ModelChoice } from '../ui/model-picker';
import { contextBar, followAgentModel, modelChip } from '../ui/chat-tools';
import { contextApi } from '../api-context';
import { embedContextFromStore } from '../ui/embeds';
import { attachBar, maySend } from '../ui/attach-bar';
import { renderMarkdown } from '../ui/markdown';

const NEAR_BOTTOM_PX = 80;

type Tier = 'orchestrator' | 'worker';

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

/**
 * The model choices for a tier. A worker may run any model: the policy's worker list comes
 * first as presets, then every model the providers list. An orchestrator may run only its
 * policy list. `current` (an override no longer offered) is kept as a choice.
 */
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
type LiveTurn = { work: number; el: HTMLElement; text: Text; caret: HTMLElement };
type Tab = 'transcript' | 'memory';
type PermissionMode = 'autonomous' | 'ask' | 'read_only';

/** The nodes an agent's VM can move to: those in service, so not stopped and not revoked. */
export function movableNodes(nodes: NodeView[]): NodeView[] {
  return nodes.filter((n) => !n.draining && !n.revoked);
}

/** The outcome of a move as one line: where the VM went, the MB it carried and the seconds it took (`ms` in). */
export function moveText(r: Pick<MoveResult, 'from' | 'to' | 'bytes' | 'ms'>, nameOf: (id: string) => string): string {
  return `Moved from ${nameOf(r.from)} to ${nameOf(r.to)}: ${(r.bytes / 1048576).toFixed(1)} MB in ${(r.ms / 1000).toFixed(1)} s.`;
}

/**
 * The prototype's menu popover (.kit-pop): under `anchor`, aligned to its end. A pick, Escape or a click
 * outside closes it. Local to this view: the shell's popover loads the window's media queries.
 */
function menuBeside(anchor: HTMLElement, menu: HTMLElement): void {
  document.querySelectorAll('.kit-pop').forEach((p) => p.remove());
  const pop = h('div.kit-pop', { role: 'presentation' }, menu);
  pop.style.position = 'fixed';
  document.body.append(pop);
  const r = anchor.getBoundingClientRect();
  pop.style.top = `${r.bottom + 6}px`;
  pop.style.right = `${Math.max(8, window.innerWidth - r.right)}px`;
  const close = (): void => {
    pop.remove();
    document.removeEventListener('mousedown', onDown);
    document.removeEventListener('keydown', onKey);
  };
  const onDown = (e: MouseEvent): void => {
    if (!pop.contains(e.target as Node) && !anchor.contains(e.target as Node)) close();
  };
  const onKey = (e: KeyboardEvent): void => {
    if (e.key === 'Escape') close();
  };
  window.setTimeout(() => document.addEventListener('mousedown', onDown), 0);
  document.addEventListener('keydown', onKey);
  menu.addEventListener('click', (e) => {
    if ((e.target as Element).closest('[role="menuitem"]')) close();
  });
}

/** A row of the prototype's side cards: a label and its value, in the .ag-kv grid. */
const kvPairs = (pairs: [string, Node | string][]): Node[] => pairs.flatMap(([k, v]) => [h('dt', null, k), h('dd', null, v)]);

export function mountAgentDetail(root: HTMLElement, cid: number): Cleanup {
  const head = h('header.ag-hd');
  const transcript = h('div.ag-tr');
  const composer = textarea({ placeholder: 'Message this agent…', rows: 3, 'aria-label': 'Message this agent' });
  const send = button('Send', () => void submit(), { kind: 'primary', icon: 'send', title: 'Send (Ctrl+Enter)' });
  // Files: picked, dropped on the composer or pasted into it. Each uploads as it is taken and goes with the next message.
  const attach = attachBar({
    onChange: () => {
      send.disabled = isOffline() || sending || attach.busy() || attach.failed();
    },
  });
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
  const modelLine = h('p');
  const details = h('dl.ag-kv');
  const queue = h('div');

  // The transcript and the memory library share one space; a tab picks which shows.
  const memoryHost = h('div', { style: 'display:none' });
  const transcriptCard = card(null, transcript, { flush: true });
  // The model chip sits in a tool row under the composer; the context line sits above it; a save error shows under the row.
  const chip = modelChip({ label: 'Model', save: (value) => contextApi.setAgentModel(cid, value || null) });
  const ctx = contextBar({ cid: () => cid, source: (next) => contextApi.agent(next) });
  const unfollow = followAgentModel(chip, () => cid);
  const composerRow = h(
    'div',
    null,
    ctx.el,
    attach.el,
    h('div.ag-compose', null, attach.button, composer, send),
    h('div.ct-tools', null, chip.el),
    chip.note,
  );
  const unbindAttach = attach.bind(composerRow);
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
  /** The last tool row of each work item, waiting for its permission decision to join it. */
  const toolRows = new Map<number, HTMLElement>();
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
    const sectionRoot = h('section.ag-sec', { 'data-work': String(work) }, sectionHead);
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
          replace(section.head, badge(String(d['kind'] ?? 'work'), 'info'), h('p', null, String(d['prompt'] ?? '')));
          break;
        case 'work_end': {
          const s = String(d['status'] ?? '');
          const tone = /fail|error/.test(s) ? 'danger' : /cancel|stop/.test(s) ? 'warn' : 'ok';
          section.root.append(badge(s || 'done', tone));
          break;
        }
        case 'model_response':
          removeLive(e.work);
          section.root.append(...responseNodes(d, (summary) => toolRows.set(e.work, summary)));
          break;
        case 'tool_decision': {
          const line = decisionBadge(d);
          if (!line) break;
          // A decision on a tool row that is waiting joins that row; otherwise it stands on its own.
          const row = toolRows.get(e.work);
          if (row) {
            row.append(line);
            toolRows.delete(e.work);
          } else section.root.append(line);
          break;
        }
        case 'tool_result': {
          const failed = d['is_error'] === true;
          const body = String(d['content'] ?? d['output'] ?? '');
          section.root.append(
            h('details', null, h('summary', null, badge(failed ? 'Error' : 'Output', failed ? 'danger' : 'ok')), h('pre.code.ag-out', null, body)),
          );
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
        const caret = h('span.muted', null, '▍');
        const el = h('div.ag-live', null, h('div.prose', null, text), caret);
        turn = { work: d.work, el, text, caret };
        live.set(key, turn);
        sectionOf(d.work).root.append(el);
      }
      const part = d.delta;
      if (part.type === 'tool_use') turn.el.insertBefore(h('span.badge-t.t-info', null, `→ ${part.name}`), turn.caret);
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
    head.querySelectorAll<HTMLButtonElement>('.ag-acts-grp button').forEach((b) => (b.disabled = on));
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

  /** A confirmation dialog: the prototype's question, a Cancel and the go button; the go button runs `onGo` once. */
  const confirmBox = (title: string, text: string, goLabel: string, danger: boolean, onGo: () => void): void => {
    let close: () => void = () => {};
    close = modal(
      title,
      h(
        'div.ag-stack',
        null,
        h('p', null, text),
        h(
          'div.cluster',
          { style: 'justify-content:flex-end' },
          button('Cancel', () => close(), { kind: 'ghost' }),
          button(goLabel, () => {
            close();
            onGo();
          }, { kind: danger ? 'danger' : 'primary' }),
        ),
      ),
    );
  };

  const restart = (a: Agent): void =>
    confirmBox(`Restart`, `Restart ${agentName(a)}? Its VM restarts.`, 'Restart', false, () => void act(() => attempt(() => api.restartAgent(cid), 'Restarting')));
  const retire = (a: Agent): void =>
    confirmBox(
      'Retire',
      `Retire ${agentName(a)}? Its VM stops for good and it leaves the roster.`,
      'Retire',
      true,
      () =>
        void act(async () => {
          const ok = await attempt(async () => {
            await api.retireAgent(cid);
            return true;
          }, 'Retired');
          if (ok) go('agents');
        }),
    );
  // One labelled checkbox for the wipe dialog: the box (read on confirm) and its row.
  const checkRow = (label: string, checked: boolean, disabled = false): [HTMLInputElement, HTMLElement] => {
    const box = h('input', { type: 'checkbox' }) as HTMLInputElement;
    box.checked = checked;
    box.disabled = disabled;
    return [box, h('label.cluster', null, box, h('span', null, label))];
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
        'div.ag-stack',
        null,
        h('p.muted', null, "The agent's VM restarts with a fresh workspace. This cannot be undone."),
        workspaceRow,
        memoryRow,
        transcriptRow,
        identityRow,
        h('div.cluster', { style: 'justify-content:flex-end' }, confirmBtn),
      ),
    );
  };

  // Move: the VM goes to another node that takes work. The dialog lists those nodes and posts the move. A 409 (the
  // agent is working, or the node is stopped or revoked) is shown as the hub says it. The move can take seconds.
  const moveDialog = (a: Agent): void => {
    const list = h('div.ag-stack', null, h('p.muted', null, 'Loading the nodes...'));
    const problem = h('p.danger');
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
        'div.ag-stack',
        null,
        h('p.muted', null, "The agent's VM moves to the node you pick. It can take seconds, and an agent that is working cannot move."),
        list,
        problem,
        outcome,
        h('div.cluster', { style: 'justify-content:flex-end' }, button('Close', () => close(), { kind: 'ghost' }), moveBtn),
      ),
    );
    api
      .nodes()
      .then((all) => {
        nodeNames = new Map(all.map((n) => [n.id, n.name || n.id]));
        const nodes = movableNodes(all);
        if (!nodes.length) {
          replace(list, h('p.muted', null, 'No node takes work right now: each one is stopped or revoked.'));
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
            return h('label.cluster', null, el, h('span', null, n.name || n.id, n.online ? null : ' (offline)'));
          }),
        );
      })
      .catch((err: unknown) => replace(list, h('p.danger', null, err instanceof Error ? err.message : 'The nodes could not load.')));
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
  const checkpointHost = h('div.ag-stack');
  const checkpointLabel = input({ placeholder: 'Label (optional)', 'aria-label': 'Checkpoint label' });
  const takeBtn = button('Take checkpoint', () => takeCheckpoint(), { kind: 'primary', icon: 'plus' });
  const checkpointCard = card(
    'Machine checkpoints',
    h('div.ag-stack', null, h('div.cluster', null, checkpointLabel, takeBtn), checkpointHost),
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
    const body: Node | Node[] = checkpointsFailed
      ? h('p.danger', null, 'Could not load the checkpoints')
      : checkpoints === null
        ? h('p.muted', null, 'Loading…')
        : list.length === 0
          ? h('p.muted', null, 'No checkpoints yet')
          : list.map(checkpointRow);
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
  const restoreCheckpoint = (cp: VmCheckpoint): void =>
    confirmBox(
      'Restore checkpoint',
      `Roll back ${checkpointName(cp)} to this checkpoint? Its VM — memory, disk and running processes — returns to that moment. A safety checkpoint is taken first, so you can undo this.`,
      'Roll back',
      false,
      () => void checkpointAct(() => attempt(() => api.restoreVmCheckpoint(cid, cp.name), 'Rolled back')),
    );
  const deleteCheckpoint = (cp: VmCheckpoint): void =>
    confirmBox(
      'Delete checkpoint',
      `Delete the checkpoint ${checkpointName(cp)}? It cannot be restored afterwards.`,
      'Delete',
      true,
      () => void checkpointAct(() => attempt(() => api.deleteVmCheckpoint(cid, cp.name), 'Checkpoint deleted')),
    );
  const checkpointRow = (cp: VmCheckpoint): HTMLElement =>
    h(
      'div.ag-row',
      { 'data-key': cp.name },
      h(
        'div',
        { style: 'min-width:0' },
        h('div.ag-name', { title: cp.name }, checkpointName(cp)),
        badge(cp.kind, 'muted'),
        h('div.ag-row-sub', null, `${ago(cp.taken_at_ms)} · ${bytesText(cp.memory_written)} memory written · ${bytesText(cp.disk_bytes)} disk · paused ${cp.paused_ms} ms`),
      ),
      h(
        'div.cluster',
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
    const result = h('div');
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
        return h('li', null, link);
      });
      replace(
        result,
        h('p', null, `${made.forks.length === 1 ? 'The new agent starts' : 'The new agents start'} from a checkpoint of ${agentName(a)}:`),
        h('ul', null, ...links),
      );
    };
    close = modal(
      `Fork ${agentName(a)}`,
      h(
        'div.ag-stack',
        null,
        h('p', null, "Each fork gets a copy of this agent's VM, taken from a fresh checkpoint, and its own name. The forks then work apart from this agent and from each other."),
        field(`Number of forks (1–${FORK_MAX})`, count),
        h('div.cluster', { style: 'justify-content:flex-end' }, forkBtn),
        result,
      ),
    );
  };

  /** Where the agent works, why it waits if it does, and the move: onto a project from the bench, or back to it. */
  const projectLine = (a: Agent): HTMLElement | null => {
    if (a.tier === 'orchestrator') return h('p.ag-row-sub', null, 'Works on every project: it opens them and runs them.');
    if (a.kind === 'model') return null;
    if (a.project) {
      const pid = a.project;
      return h(
        'p.ag-row-sub',
        null,
        'Works on ',
        h('a', { href: href('projects', pid) }, projectTitle(pid)),
        a.joined_ms ? ` since ${ago(a.joined_ms)}` : null,
        a.held ? ` · ${a.held}` : null,
        ' ',
        button('Back to the bench', () => void act(() => attempt(() => api.removeMember(pid, cid), `${agentName(a)} is back on the bench`).then(() => void refreshAgents())), {
          size: 'sm',
          kind: 'ghost',
          disabled: busy,
        }),
      );
    }
    if (!onBench(a)) return null;
    const live = liveProjects(projects.get());
    const put = live.length
      ? (() => {
          const sel = h('select.select', { 'aria-label': 'Put on a project' }, h('option', { value: '' }, 'Put on a project…'), ...live.map((p) => h('option', { value: p.id }, p.title))) as HTMLSelectElement;
          sel.addEventListener('change', () => {
            if (!sel.value) return;
            const to = sel.value;
            void act(() => attempt(() => api.addMember(to, cid), `${agentName(a)} works on ${projectTitle(to)} now`).then(() => void refreshAgents()));
          });
          return sel;
        })()
      : null;
    return h('p.ag-row-sub', null, `${agentName(a)} is on the bench: it does nothing until it works on a project. `, put);
  };

  /** The More menu: Sleep, Fork, Move and Restart, then a rule and the rare destructive Wipe and Retire. */
  const openMore = (anchor: HTMLElement, a: Agent): void => {
    const w = a.status === 'working';
    const off = a.status === 'offline';
    const asleep = a.status === 'asleep' || a.status === 'dozing';
    const item = (iconEl: Element, label: string, note: string | null, onPick: () => void, o: { disabled?: boolean; danger?: boolean } = {}): HTMLButtonElement => {
      const b = h(
        'button',
        { type: 'button', role: 'menuitem', disabled: !!o.disabled, class: o.danger ? 'danger' : null },
        iconEl,
        h('span', null, label, note ? h('small', null, note) : null),
      );
      b.addEventListener('click', onPick);
      return b;
    };
    const menu = h(
      'div.menu.ag-more',
      { role: 'menu' },
      asleep ? null : item(icon('moon', '', 15), 'Sleep', 'Pause the VM until a chat or task wakes it', () => sleepAgent(), { disabled: busy || off || w }),
      item(icon('copy', '', 15), 'Fork…', null, () => fork(a), { disabled: busy }),
      item(icon('send', '', 15), 'Move…', 'Move the VM to another node', () => moveDialog(a), { disabled: busy || w }),
      item(icon('refresh', '', 15), 'Restart', null, () => restart(a), { disabled: busy }),
      h('hr'),
      item(icon('trash', '', 15), 'Wipe…', null, () => wipe(a), { disabled: busy, danger: true }),
      item(lu('archive', 15), 'Retire', null, () => retire(a), { disabled: busy, danger: true }),
    );
    menuBeside(anchor, menu);
  };

  /** The head's More button: the prototype's ellipsis, opening the More menu beside it. */
  const moreButton = (a: Agent): HTMLButtonElement => {
    const more = button('More', () => openMore(more, a), { disabled: busy });
    more.prepend(lu('ellipsis', 15));
    return more;
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
    const asleep = a.status === 'asleep' || a.status === 'dozing';
    replace(
      head,
      h('a.ag-back', { href: href('agents') }, '← Agents'),
      h(
        'div.ag-hd-top',
        null,
        h('div.cluster', null, h('h1', null, agentName(a)), statusBadge(a.status)),
        h(
          'div.ag-head-actions',
          null,
          h(
            'div.ag-acts-grp',
            null,
            button('Open workspace', () => {
              prewake(cid, a.status);
              openIde(`agent:${cid}`);
            }, { kind: 'primary', icon: 'square-code', title: "Open this agent's workspace in the IDE" }),
            a.status === 'working' ? button('Cancel work', () => void act(() => attempt(() => api.cancel(cid), 'Cancelled')), { icon: 'ban', disabled: busy }) : null,
            asleep ? button('Wake', () => wakeAgent(), { icon: 'sun', disabled: busy }) : null,
            moreButton(a),
            button('Stop', () => void act(() => attempt(() => api.stop(cid), 'Stopped')), { kind: 'danger', icon: 'stop', disabled: busy }),
          ),
        ),
      ),
      projectLine(a),
    );

    composer.disabled = offline;
    attach.button.disabled = offline;
    send.disabled = offline || sending || attach.busy() || attach.failed();

    const source =
      a.model_source === 'agent' ? 'set for this agent' : a.model_source === 'project' ? 'from its project' : a.model_source === 'role' ? 'from role' : 'default';
    replace(modelLine, h('strong.mono', null, a.model), ' ', h('span.muted', null, source));
    const want = a.model_override ?? '';
    if (want !== appliedOverride) {
      appliedOverride = want;
      syncModel(want);
    }

    replace(
      details,
      kvPairs([
        ['CID', h('span.mono', null, String(a.cid))],
        ['Run', h('span.mono', null, a.run ?? '—')],
        ['VM', h('span.mono', null, a.vm === null ? '—' : String(a.vm))],
        ['Connected', ago(a.connected_at_ms)],
        ['Last seen', ago(a.last_seen_ms)],
        ['Tokens in', h('span.mono', null, compact(a.usage.input_tokens))],
        ['Tokens out', h('span.mono', null, compact(a.usage.output_tokens))],
      ]),
    );

    replace(
      queue,
      a.work ? h('div', null, badge(a.work.kind, 'info'), ' ', clip(a.work.prompt, 160)) : h('span.muted', null, 'Nothing running'),
      h('p.muted', null, `${a.queue} queued`),
    );
  };

  const isOffline = (): boolean => agent?.status === 'offline';

  const submit = async (): Promise<void> => {
    const text = composer.value.trim();
    const ids = attach.ids();
    if (isOffline() || !maySend(text, ids, sending || attach.busy() || attach.failed())) return;
    sending = true;
    send.disabled = true;
    const ok = await attempt(() => api.chat(cid, text, ids));
    sending = false;
    send.disabled = isOffline() || attach.busy() || attach.failed();
    if (ok) {
      composer.value = '';
      attach.clear();
      ctx.refresh();
    }
  };

  composer.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      void submit();
    }
  });

  // The agent's permission mode: loaded on mount, saved on click.
  const permissionOptions: { mode: PermissionMode; label: string; hint: string }[] = [
    { mode: 'autonomous', label: 'Autonomous', hint: 'acts on its own' },
    { mode: 'ask', label: 'Ask first', hint: 'write and irreversible actions wait for your approval in Approvals' },
    { mode: 'read_only', label: 'Read-only', hint: 'can look, not change' },
  ];
  const permissionHost = h('div.ag-stack');
  const harness = harnessCard(cid);
  const conduct = conductCard(cid);
  let permission: PermissionMode | null = null;
  let permissionFailed = false;
  const renderPermission = (): void => {
    // One full-width choice per mode, its hint under its name, so the card fits the side column.
    replace(
      permissionHost,
      h(
        'div.ag-stack',
        { role: 'radiogroup', 'aria-label': 'Permission mode' },
        ...permissionOptions.map((o) => {
          const choice = h(
            'button.ag-choice',
            { type: 'button', role: 'radio', 'aria-checked': String(o.mode === permission) },
            h('strong', null, o.label),
            h('span.muted', null, o.hint),
          );
          choice.addEventListener('click', () => choosePermission(o.mode));
          return choice;
        }),
      ),
      permissionFailed ? h('p.muted', null, 'Could not load this setting') : permission === null ? h('p.muted', null, 'Loading…') : null,
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
    head,
    profileHost,
    h(
      'div.ag-page',
      null,
      h('div.ag-main', null, tabBar, transcriptCard, memoryHost, composerRow),
      h(
        'aside.ag-side',
        null,
        card('Model', h('div.ag-stack', null, modelLine, field('Change model', picker.el), field('Reasoning effort', effortSel))),
        card('Harness', harness.el),
        card('Permissions', permissionHost),
        card('Conduct', conduct.el),
        checkpointCard,
        card('Details', details),
        card('Queue', queue),
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
          ? empty('alert', 'No such agent', undefined, h('a.btn', { href: href('agents') }, 'All agents'))
          : empty('alert', 'Could not load this agent', err instanceof Error ? err.message : undefined),
      );
      return;
    }
    if (!alive) return;
    renderAgent();
    ctx.refresh();
    void loadCheckpoints();
    void Promise.all([api.models(), api.settings()])
      .then(([view, settings]) => alive && populateModels(view.models, settings))
      .catch(() => undefined);
    await loadHistory();
  };
  void start();

  return () => {
    alive = false;
    unbindAttach();
    ctx.dispose();
    chip.dispose();
    unfollow();
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
  const body = h('div.ag-stack');
  let view: SanctionsView | null = null;
  let failed = false;
  let error: string | null = null;
  let busy = false;
  let alive = true;

  const sanctionRow = (r: Sanction): HTMLElement =>
    h(
      'div.ag-row',
      { title: r.title },
      h(
        'div',
        { style: 'min-width:0' },
        h('strong', null, r.sanction),
        ' ',
        h('code', null, r.law),
        ' ',
        badge(r.severity, severityTone(r.severity)),
        h('div.ag-row-sub', null, `offence ${r.count} · ${r.outcome} · ${r.fine > 0 ? `fined ${compact(r.fine)} cr` : 'no fine'} · ${ago(r.at_ms)}`),
        r.lifted ? h('div.ag-row-sub', null, `lifted by owner: ${r.lifted.why}`) : null,
        r.owner_action
          ? h('div.ag-row-sub', null, `${r.owner_action.action === 'retire' ? 'Retired' : 'Acknowledged'} by the owner, ${ago(r.owner_action.at_ms)}`)
          : null,
      ),
    );

  const suspensionRow = (s: Suspension, now: number): HTMLElement =>
    h(
      'div.ag-row',
      null,
      h('div', { style: 'min-width:0' }, h('code', null, s.tool), ' ', h('span.muted', null, s.law), h('div.ag-row-sub', null, timeLeft(s.until_ms, now))),
      button('Lift', () => liftSuspension(s), { size: 'sm', disabled: busy }),
    );

  const render = (): void => {
    if (!view) {
      replace(body, failed ? h('p.danger', null, 'Could not load conduct.') : h('p.muted', null, 'Loading…'));
      return;
    }
    const now = Date.now();
    const suspensions = view.suspensions.filter((s) => s.cid === cid && s.active).sort((x, y) => x.until_ms - y.until_ms);
    const sanctions = view.sanctions.filter((r) => r.cid === cid).sort((x, y) => y.at_ms - x.at_ms);
    replace(
      body,
      h('h4', null, 'Suspended now'),
      suspensions.length ? suspensions.map((s) => suspensionRow(s, now)) : h('p.muted', null, 'Not suspended.'),
      error ? h('p.danger', null, error) : null,
      h('h4', null, 'Sanctions'),
      sanctions.length ? sanctions.map((r) => sanctionRow(r)) : h('p.muted', null, 'No sanctions on record.'),
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

  /** The lift dialog: the suspension named, a reason box, then Cancel and Lift. An empty reason is refused and the box stays open. */
  const liftSuspension = (s: Suspension): void => {
    const reason = input({ placeholder: 'Why it can be lifted', 'aria-label': 'Reason' });
    let close: () => void = () => {};
    const lift = async (): Promise<void> => {
      const text = reason.value.trim();
      if (!text) {
        toast('Give a reason to lift a suspension.', 'error');
        reason.focus();
        return;
      }
      close();
      error = null;
      busy = true;
      render();
      try {
        await api.liftSanction(s.id, text);
        toast('Suspension lifted');
      } catch (err) {
        error = err instanceof Error ? err.message : String(err);
      }
      busy = false;
      await refresh();
    };
    close = modal(
      'Lift suspension',
      h(
        'div.ag-stack',
        null,
        h('p', null, `Lift the suspension of ${s.tool} (${s.law})? Give a reason.`),
        reason,
        h(
          'div.cluster',
          { style: 'justify-content:flex-end' },
          button('Cancel', () => close(), { kind: 'ghost' }),
          button('Lift', () => void lift(), { kind: 'primary' }),
        ),
      ),
    );
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

/**
 * The status word in the head. Asleep is muted, not a warning: the agent is paused on purpose and wakes on a chat
 * or task. A dozing agent reads as asleep (light): paused after a short idle, and it wakes in a moment.
 */
function statusBadge(status: AgentStatus): HTMLElement {
  if (status === 'working') return badge('Working', 'info', true);
  if (status === 'idle') return badge('Idle', 'ok', true);
  if (status === 'asleep') return badge('Asleep', 'muted', true);
  if (status === 'dozing') return badge('Asleep (light)', 'muted', true);
  return badge('Offline', 'muted', true);
}

/** Time left until `untilMs`, as "2h 5m left". */
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

/**
 * A model response as the prototype's transcript rows: its text as prose, each tool call as a disclosure
 * (the name in mono, the input in a pre). The last tool row of a response is kept for its decision to join.
 */
function responseNodes(data: Record<string, unknown>, waitingRow: (summary: HTMLElement) => void): Node[] {
  const blocks = Array.isArray(data['content']) ? (data['content'] as Block[]) : [];
  const last = blocks.map((b) => b.type).lastIndexOf('tool_use');
  const out: Node[] = [];
  blocks.forEach((b, i) => {
    if (b.type === 'text') out.push(h('div.prose', null, renderMarkdown(b.text, { embeds: embedContextFromStore() })));
    else if (b.type === 'tool_use') {
      const summary = h('summary', null, h('span.ag-tn', null, `→ ${b.name}`));
      out.push(h('details', null, summary, h('pre.code.ag-out', null, JSON.stringify(b.input, null, 2))));
      if (i === last) waitingRow(summary);
    }
  });
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
