// Agent detail: one agent's transcript, its composer, and its model, details and queue.

import '../styles/agent.css';
import { api, ApiError, type Agent, type Block, type Delta, type Entry, type ModelInfo } from '../api';
import { href } from '../router';
import { agents, on } from '../store';
import { ago, attempt, badge, button, card, clip, compact, empty, field, textarea } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';

const NEAR_BOTTOM_PX = 80;

type Section = { root: HTMLElement; head: HTMLElement };
type LiveTurn = { work: number; el: HTMLElement; text: Text; thinking: Text };

export function mountAgentDetail(root: HTMLElement, cid: number): Cleanup {
  const head = h('header.agent-head');
  const transcript = h('div.transcript');
  const composer = textarea({ placeholder: 'Message this agent…', rows: 3 });
  const send = button('Send', () => void submit(), { kind: 'primary', icon: 'send', title: 'Send (Ctrl+Enter)' });
  const modelSelect = h('select.select');
  const modelLine = h('div.model-line');
  const details = h('div');
  const queue = h('div.stack');

  const sections = new Map<number, Section>();
  const live = new Map<string, LiveTurn>();
  const pending: (() => void)[] = [];
  let agent: Agent | undefined;
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

  const populateModels = (models: ModelInfo[]): void => {
    const current = appliedOverride ?? '';
    const options = models.map((m) => ({ value: `${m.provider}/${m.id}`, label: m.name ?? m.id }));
    if (current && !options.some((o) => o.value === current)) options.push({ value: current, label: current });
    modelSelect.replaceChildren(h('option', { value: '' }, 'Use the default'), ...options.map((o) => h('option', { value: o.value }, o.label)));
    modelSelect.value = current;
  };

  const renderAgent = (): void => {
    const a = agent;
    if (!a) return;
    const offline = a.status === 'offline';
    const label = a.status === 'working' ? 'Working' : a.status === 'idle' ? 'Idle' : 'Offline';
    const tone = a.status === 'working' ? 'info' : a.status === 'idle' ? 'ok' : 'muted';
    replace(
      head,
      h('a.back', { href: href('agents') }, 'Agents'),
      h(
        'div.agent-title',
        null,
        h('h1', null, a.name),
        badge(label, tone, true),
        badge(a.tier === 'orchestrator' ? 'Orchestrator' : 'Worker', a.tier === 'orchestrator' ? 'accent' : 'muted'),
        h('span.muted', null, a.role),
      ),
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
      modelSelect.value = want;
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
        h('dt', null, 'Credits'),
        h('dd.mono', null, String(a.credits)),
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

  modelSelect.addEventListener('change', () => {
    const value = modelSelect.value;
    void attempt(() => api.setAgentModel(cid, value || null), 'Model updated').then((ok) => {
      if (ok) appliedOverride = value;
      else modelSelect.value = appliedOverride ?? '';
    });
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
      h(
        'div.agent-grid',
        null,
        h(
          'div.agent-main',
          null,
          card(null, transcript, { flush: true }),
          h('div.composer', null, composer, h('div.row', null, h('span', { style: 'flex:1' }), send)),
        ),
        h(
          'aside.agent-side',
          null,
          card('Model', h('div.stack', null, modelLine, field('Change model', modelSelect))),
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
    void api.models().then((view) => alive && populateModels(view.models)).catch(() => undefined);
    await loadHistory();
  };
  void start();

  return () => {
    alive = false;
    offs.forEach((off) => off());
    live.clear();
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
