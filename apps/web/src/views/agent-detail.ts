// Agent detail: one agent's transcript, its composer, and its model, details and queue.

import '../styles/agent.css';
import { api, ApiError, type Agent, type Block, type Delta, type Entry, type ModelInfo, type Settings } from '../api';
import { href } from '../router';
import { agents, on } from '../store';
import { ago, attempt, badge, button, card, clip, compact, empty, field, tabs, textarea } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { mountMemory } from './memory';
import { profileCard } from './agent-profile';
import { choicesFrom, modelPicker, type ModelChoice } from '../ui/model-picker';

const NEAR_BOTTOM_PX = 80;

type Tier = 'orchestrator' | 'worker';

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
  return out;
}

type Section = { root: HTMLElement; head: HTMLElement };
type LiveTurn = { work: number; el: HTMLElement; text: Text; thinking: Text };
type Tab = 'transcript' | 'memory';

export function mountAgentDetail(root: HTMLElement, cid: number): Cleanup {
  const head = h('header.agent-head');
  const transcript = h('div.transcript');
  const composer = textarea({ placeholder: 'Message this agent…', rows: 3 });
  const send = button('Send', () => void submit(), { kind: 'primary', icon: 'send', title: 'Send (Ctrl+Enter)' });
  // The agent's model override ('' = the default), offered by its tier.
  const picker = modelPicker({
    choices: [],
    label: 'Change model',
    placeholder: 'Use the default',
    onChange: (value) => {
      void attempt(() => api.setAgentModel(cid, value || null), 'Model updated').then((ok) => {
        if (ok) appliedOverride = value;
        else picker.value = appliedOverride ?? '';
      });
    },
  });
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
    const history = await attempt(() => api.transcript(cid));
    if (!alive) return;
    for (const e of (history ?? []).slice().sort((x, y) => x.seq - y.seq)) renderEntry(e);
    loaded = true;
    for (const run of pending.splice(0)) run();
    transcript.scrollTop = transcript.scrollHeight;
  };

  const populateModels = (models: ModelInfo[], settings: Settings): void => {
    const current = appliedOverride ?? '';
    picker.setChoices(tierChoices(agent?.tier ?? 'worker', settings, models, { label: 'Use the default' }, current));
    picker.value = current;
  };

  const renderAgent = (): void => {
    const a = agent;
    if (!a) return;
    if (!profile) {
      profile = profileCard(a);
      profileHost.replaceChildren(profile.el);
    } else profile.update(a);
    const offline = a.status === 'offline';
    replace(
      head,
      h('a.back', { href: href('agents') }, 'Agents'),
      h('div.agent-title', null),
      h(
        'div.row.agent-actions',
        null,
        a.status === 'working' ? button('Cancel work', () => void attempt(() => api.cancel(cid), 'Cancelled'), { icon: 'ban' }) : null,
        button('Stop', () => void attempt(() => api.stop(cid), 'Stopped'), { kind: 'danger', icon: 'stop' }),
      ),
    );

    composer.disabled = offline;
    send.disabled = offline || sending;

    const source = a.model_source === 'agent' ? 'set for this agent' : a.model_source === 'role' ? 'from role' : 'default';
    replace(modelLine, h('strong', null, a.model), h('span.faint', null, source));
    const want = a.model_override ?? '';
    if (want !== appliedOverride) {
      appliedOverride = want;
      picker.value = want;
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
          card('Model', h('div.stack', null, modelLine, field('Change model', picker.el))),
          card('Permissions', permissionHost),
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
    void Promise.all([api.models(), api.settings()])
      .then(([view, settings]) => alive && populateModels(view.models, settings))
      .catch(() => undefined);
    await loadHistory();
  };
  void start();

  return () => {
    alive = false;
    window.clearInterval(profileTimer);
    offs.forEach((off) => off());
    live.clear();
    unmountMemory?.();
    unmountMemory = null;
  };
}

function responseNodes(data: Record<string, unknown>): Node[] {
  const blocks = Array.isArray(data['content']) ? (data['content'] as Block[]) : [];
  const out: Node[] = [];
  for (const b of blocks) {
    if (b.type === 'text') out.push(h('p.prose', null, b.text));
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
