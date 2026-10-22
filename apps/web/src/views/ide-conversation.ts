// The IDE's conversation workspace (`#/talk/conv-<n>` and `#/ide/conv-<n>`): the owner's conversation with the
// orchestrator, the project it drafts beside it, and Start. On the left, the conversations; in the centre, the chat
// (answers stream in as they are written); on the right, the draft: its goal, tasks, team, budget and settings, with
// what still stops Start, the owner's edits, and Start. Once started, the conversation stays the project's line to the
// orchestrator. The markup is the prototype's talk screen (loom: m-workspace's talk, m-ide-chat's conversation and draft).

import { api, type Delta, type OwnerConversation, type OwnerConversationSummary, type OwnerMessage, type ProjectDraft } from '../api';
import { afterText, budgetText, settingsLine, startBlock, streamed, teamLine } from '../ide/conversation-model';
import { consoleOrigin } from '../ide/open';
import { hrefIn } from '../router';
import { agentName, roleLabel } from '../roles';
import { projectTitle, refreshProjects } from '../scope';
import { agents, on } from '../store';
import { ago, attempt, badge, button, field, input, modal, textarea, toast } from '../ui/components';
import { attachBar, maySend } from '../ui/attach-bar';
import { contextBar, loadModelChoices, modelChip } from '../ui/chat-tools';
import { contextApi } from '../api-context';
import { sizeText } from '../ui/file-drop';
import { h, lu, replace } from '../ui/dom';
import { renderMarkdown } from '../ui/markdown';
import { openNewProject } from '../ui/new-project';
import type { Cleanup } from './index';

/** The orchestrator's name, from the roster. */
function orchestratorName(cid: number): string {
  const agent = agents.get().find((a) => a.cid === cid);
  return agent ? agentName(agent) : 'The orchestrator';
}

/** The files a message carries: each a link to its bytes, opened in a new tab. */
function filesOf(files: NonNullable<OwnerMessage['attachments']>): HTMLElement {
  return h(
    'div.ic-chips',
    null,
    files.map((a) => h('a.ic-chip', { href: api.fileUrl(a.id), target: '_blank', rel: 'noopener', title: a.name }, `${a.name} · ${sizeText(a.size)}`)),
  );
}

/** One message of the chat: the owner's words as a bubble, the orchestrator's as its reply, a plaza note as a notice. */
function bubble(m: OwnerMessage, who: string): HTMLElement {
  if (m.from === 'plaza') return h('div.ic-notice', null, renderMarkdown(m.text));
  const failed = m.status === 'failed' || m.status === 'stopped';
  return h(
    m.from === 'owner' ? 'article.ic-msg.ic-owner' : 'article.ic-msg',
    null,
    h('div.ic-who', null, h('b', null, m.from === 'owner' ? 'You' : who), ` · ${ago(m.at_ms)}`),
    m.text ? (m.from === 'owner' ? h('p', null, m.text) : h('div.ic-md', null, renderMarkdown(m.text))) : null,
    m.attachments?.length ? filesOf(m.attachments) : null,
    failed ? h('div.ic-sys.ic-err', null, m.status === 'failed' ? 'This turn failed.' : 'This turn was stopped.') : null,
  );
}

/** The owner edits the draft's words and budget, on the revision on screen. */
function openEdit(id: string, draft: ProjectDraft): void {
  const title = input({ value: draft.title });
  const goal = textarea({ rows: 4 });
  goal.value = draft.goal;
  const repo = input({ value: draft.repo ?? '', placeholder: 'agentplaza, a new branch release-0.9' });
  const budget = input({ type: 'number', min: 0, step: 1, value: draft.budget ?? '', placeholder: 'From the plaza’s budget' });
  const save = button(
    'Save',
    () => {
      const credits = budget.value.trim() === '' ? undefined : Math.trunc(Number(budget.value));
      void (async () => {
        const ok = await attempt(
          () => api.editDraft(id, { revision: draft.revision, title: title.value.trim(), goal: goal.value.trim(), repo: repo.value.trim(), budget: credits }),
          'Draft saved: the orchestrator sees your change with your next message',
        );
        if (ok) close();
      })();
    },
    { kind: 'primary' },
  );
  const close = modal(
    'Edit the draft',
    h('div.ic-form', null, field('Title', title), field('Goal', goal), field('Where the work goes', repo), field('Budget (credits)', budget)),
    undefined,
    [save],
  );
}

export function mountConversation(root: HTMLElement, id: string): Cleanup {
  root.classList.add('ic-talk-host');
  let alive = true;
  let conversation: OwnerConversation | null = null;
  let list: OwnerConversationSummary[] = [];
  let deltas: { work: number; delta: { type: string; text?: string } }[] = [];
  let sending = false;

  const side = h('nav.ic-talk-side', { 'aria-label': 'Conversations' });
  const log = h('div.ic-log.ic-conv-log', { role: 'log', 'aria-live': 'polite' });
  /** The answer that is still streaming in: the last thing in the log, rebuilt as the words arrive. */
  const liveWrap = h('div.ic-live');
  const draftEl = h('aside.ic-draft.ic-talk-draft', { 'aria-label': 'The project draft' });
  const head = h('div.ic-talk-head');

  const composerInput = h('textarea.ic-input', { rows: 2, placeholder: 'Write to the orchestrator… (Enter sends, Shift+Enter breaks the line)' }) as HTMLTextAreaElement;
  const sendBtn = button('Send', () => void send(), { kind: 'primary' });
  // Files: picked, dropped on the conversation or pasted into it. Each uploads as it is taken and goes with the next message.
  const bar = attachBar({ onChange: () => syncSend() });
  bar.button.className = 'btn ghost';
  bar.button.append('Attach');
  const syncSend = (): void => {
    sendBtn.disabled = sending || !conversation || conversation.archived || bar.busy() || bar.failed();
  };
  composerInput.addEventListener('keydown', (ev) => {
    if (ev.key === 'Enter' && !ev.shiftKey && !ev.isComposing) {
      ev.preventDefault();
      void send();
    }
  });

  // The model chip joins the tool row; the context line sits above the composer; a save error shows under it.
  const chip = modelChip({ label: 'Orchestrator model', save: (value) => contextApi.setConversationModel(id, value || null) });
  const ctx = contextBar({ cid: () => conversation?.cid ?? 0, source: () => contextApi.conversation(id) });
  void loadModelChoices('orchestrator').then((c) => chip.setChoices(c), () => undefined);

  const composer = h(
    'div.ic-composer',
    null,
    bar.el,
    h('div.ic-row', null, h('span.ic-sr', null, 'Message to the orchestrator'), composerInput, h('div.ic-acts', null, chip.el, bar.button, sendBtn)),
  );
  const main = h(
    'div.ic-talk-main',
    null,
    h('div.ws-main', null, h('section.ic-conv', { 'aria-label': 'Conversation' }, log, ctx.el, composer, chip.note)),
  );
  const top = h(
    'header.ic-talk-top',
    null,
    button('Console', () => (location.hash = consoleOrigin()), { size: 'sm', kind: 'ghost', title: 'Back to the console (Escape)' }),
    head,
  );
  root.append(h('div.ic-talk', null, top, side, main, draftEl));
  const unbind = bar.bind(main);
  log.append(liveWrap);

  const onKey = (e: KeyboardEvent): void => {
    if (e.key === 'Escape' && !document.querySelector('.scrim')) location.hash = consoleOrigin();
  };
  document.addEventListener('keydown', onKey);

  function drawSide(): void {
    replace(
      side,
      h('div.itk-sec', null, h('span', null, 'Talking it through'), h('span.sp'), button('New', () => openNewProject(), { size: 'sm', icon: 'plus', title: 'A new conversation, or a project opened directly' })),
      list.length
        ? list.map((c) =>
            h(
              'a.itk-t',
              { href: hrefIn(null, 'ide', c.id), 'aria-current': c.id === id ? 'true' : null, title: c.title },
              lu(c.draft ? 'file-pen-line' : 'messages-square', 14),
              h('span.n', null, c.title),
              h(
                'span.m',
                null,
                [c.project ? projectTitle(c.project) : c.draft ? `draft r${c.draft.revision}` : '', c.pending ? 'answering…' : '', ago(c.updated_ms)]
                  .filter(Boolean)
                  .join(' · '),
              ),
            ),
          )
        : h('p.ic-none', null, 'No conversations yet.'),
    );
  }

  function drawHead(): void {
    const c = conversation;
    const chip = c?.project
      ? badge(`Project ${projectTitle(c.project)}`, 'accent')
      : c?.draft
        ? badge(`Draft · revision ${c.draft.revision}`, startBlock(c) ? 'warn' : 'ok')
        : null;
    replace(head, h('span.ic-talk-title', null, c?.title ?? id), chip);
  }

  function drawLog(): void {
    const c = conversation;
    if (!c) return;
    const near = log.scrollHeight - log.scrollTop - log.clientHeight < 80;
    const who = orchestratorName(c.cid);
    replace(log, c.messages.map((m) => bubble(m, who)), liveWrap);
    if (near) log.scrollTop = log.scrollHeight;
    drawLive();
  }

  function drawLive(): void {
    const c = conversation;
    if (!c || !c.pending.length) {
      replace(liveWrap);
      return;
    }
    const text = streamed(deltas, c.pending);
    replace(
      liveWrap,
      h(
        'article.ic-msg.ic-live-msg',
        null,
        h('div.ic-who', null, h('b', null, orchestratorName(c.cid)), ' · answering…'),
        text ? h('div.ic-md', null, renderMarkdown(text)) : h('span.ic-dots', { role: 'img', 'aria-label': 'Thinking' }, h('i'), h('i'), h('i')),
      ),
    );
  }

  function drawDraft(): void {
    const c = conversation;
    if (!c) return;
    const d = c.draft;
    if (!d) {
      replace(
        draftEl,
        h('h2.ic-draft-h', null, 'The project draft'),
        h('p.ic-muted', null, 'The orchestrator drafts the project as you talk, and the draft appears here.'),
      );
      return;
    }
    const roster = agents.get();
    const block = startBlock(c);
    const started = !!c.project;
    const facts: [string, string | Node][] = [];
    if (d.repo) facts.push(['Repo', d.repo]);
    facts.push(['Team', d.team.length ? teamLine(d.team, roster) : 'Not drafted yet']);
    facts.push(['Budget', budgetText(d)]);
    const settings = settingsLine(d.settings);
    replace(
      draftEl,
      h('div', null, h('h2.ic-draft-h', null, d.title), h('p.ic-muted', null, `revision ${d.revision} · ${d.updated_by === 'owner' ? 'your edit' : 'the orchestrator'} · ${ago(d.updated_ms)}`)),
      h('p.ic-goal', null, d.goal),
      h('dl.kv', null, facts.map(([k, v]) => h('div', null, h('dt', null, k), h('dd', null, v)))),
      settings
        ? h('div.ic-settings', null, h('span.ic-k', null, 'Settings'), h('ul', null, settings.split(' · ').map((p) => h('li', null, p))))
        : null,
      h('h3.ic-h3', null, `Tasks (${d.tasks.length})`),
      d.tasks.length
        ? h(
            'ol.ic-tasks',
            null,
            d.tasks.map((t) =>
              h(
                'li',
                null,
                h('span.ic-mono', null, t.key),
                ` ${t.goal} `,
                h('span.ic-muted', null, t.role ? roleLabel(t.role) : 'any role'),
                afterText(t) ? h('span.ic-muted', null, ` ${afterText(t)}`) : null,
              ),
            ),
          )
        : h('p.ic-muted', null, 'No tasks yet.'),
      started
        ? h(
            'div.ic-block',
            null,
            h(
              'p.ic-started',
              null,
              'Started as ',
              h('a', { href: hrefIn(c.project!, 'projects', c.project!) }, projectTitle(c.project)),
              '. This conversation is its line to the orchestrator.',
            ),
            h(
              'div.ic-acts',
              null,
              h('a.btn', { href: hrefIn(c.project!, 'work', c.project!) }, 'Open in Work'),
              h('a.btn', { href: hrefIn(c.project!, 'sessions') }, 'Its agents'),
            ),
          )
        : h(
            'div.ic-block',
            null,
            block ? h('p.ic-stop', null, block) : h('p.ic-ready', null, 'Nothing stops Start.'),
            h(
              'div.ic-acts',
              null,
              button('Edit…', () => openEdit(id, d), { size: 'sm', kind: 'ghost' }),
              button('Start project', () => void start(d.revision), { kind: 'primary', disabled: !!block, title: block ?? `Open, staff and plan it at revision ${d.revision}` }),
            ),
          ),
    );
  }

  function draw(): void {
    if (!alive) return;
    drawSide();
    drawHead();
    drawLog();
    drawDraft();
    syncSend();
    syncChip();
  }

  /** The chip shows the conversation's own model and the orchestrator's model in use (from the agents store). */
  function syncChip(): void {
    const c = conversation;
    const orchestrator = c ? agents.get().find((a) => a.cid === c.cid) : undefined;
    chip.set({ value: (c as { model?: string | null } | null)?.model ?? '', shown: orchestrator?.model ?? '' });
  }

  async function load(): Promise<void> {
    const [c, l] = await Promise.all([attempt(() => api.ownerConversation(id)), api.ownerConversations().catch(() => list)]);
    if (!alive) return;
    if (c) conversation = c;
    list = l;
    if (!c) {
      replace(log, h('p.muted', null, `No conversation ${id}.`), liveWrap);
      return;
    }
    draw();
    ctx.refresh();
    log.scrollTop = log.scrollHeight;
  }

  async function send(): Promise<void> {
    const text = composerInput.value.trim();
    const ids = bar.ids();
    if (!conversation || !maySend(text, ids, sending || bar.busy() || bar.failed())) return;
    sending = true;
    draw();
    const c = await attempt(() => api.sayInConversation(id, text, ids));
    sending = false;
    if (!alive) return;
    if (c) {
      conversation = c;
      composerInput.value = '';
      bar.clear();
      log.scrollTop = log.scrollHeight;
      ctx.refresh();
    }
    draw();
  }

  async function start(revision: number): Promise<void> {
    const done = await attempt(() => api.startDraft(id, revision));
    if (!done || !alive) return;
    conversation = done;
    await refreshProjects();
    const s = done.started;
    const fresh = s.spawned.reduce((n, x) => n + x.count, 0);
    toast(`${projectTitle(s.project)} started: ${s.tasks.length} tasks, ${s.joined.length} agents from the bench, ${fresh} new`);
    draw();
  }

  const offs = [
    on('owner_conversation', (data) => {
      const c = data as OwnerConversation;
      if (c.id === id) {
        // An answer that arrived ends its stream.
        deltas = deltas.filter((d) => c.pending.includes(d.work));
        conversation = c;
      }
      const row = list.findIndex((x) => x.id === c.id);
      if (row < 0) void api.ownerConversations().then((l) => ((list = l), drawSide()));
      draw();
    }),
    on('delta', (raw) => {
      const d = raw as Delta;
      const c = conversation;
      if (!c || d.cid !== c.cid || !c.pending.includes(d.work)) return;
      deltas.push({ work: d.work, delta: d.delta as { type: string; text?: string } });
      drawLive();
    }),
    agents.watch(() => {
      drawDraft();
      syncChip();
    }),
  ];
  void load();

  return () => {
    alive = false;
    offs.forEach((off) => off());
    document.removeEventListener('keydown', onKey);
    unbind();
    ctx.dispose();
    chip.dispose();
    root.classList.remove('ic-talk-host');
  };
}
