// The IDE's conversation workspace (`#/ide/conv-<n>`): the owner's conversation with the orchestrator, the project it
// drafts beside it, and Start. On the left, the conversations; in the centre, the chat (answers stream in as they are
// written); on the right, the draft: its goal, tasks, team, budget and settings, with what still stops Start, the
// owner's edits, and Start. Once started, the conversation stays the project's line to the orchestrator.

import convCss from '../styles/ide-conversation.css';
import { api, type Delta, type OwnerConversation, type OwnerConversationSummary, type OwnerMessage, type ProjectDraft } from '../api';
import { afterText, budgetText, settingsLine, startBlock, streamed, teamLine } from '../ide/conversation-model';
import { consoleOrigin } from '../ide/open';
import { hrefIn } from '../router';
import { roleLabel } from '../roles';
import { projectTitle, refreshProjects } from '../scope';
import { agents, on } from '../store';
import { ago, attempt, badge, button, field, input, modal, textarea, toast } from '../ui/components';
import { attachBar, maySend } from '../ui/attach-bar';
import { sizeText } from '../ui/file-drop';
import { h, replace } from '../ui/dom';
import { renderMarkdown } from '../ui/markdown';
import { openNewProject } from '../ui/new-project';
import type { Cleanup } from './index';

const STYLE_ID = 'apz-ide-conv-style';

function ensureStyle(): void {
  if (document.getElementById(STYLE_ID)) return;
  const style = document.createElement('style');
  style.id = STYLE_ID;
  style.textContent = convCss;
  document.head.appendChild(style);
}

/** The orchestrator's name, from the roster. */
function orchestratorName(cid: number): string {
  const agent = agents.get().find((a) => a.cid === cid);
  return agent?.name || 'The orchestrator';
}

/** The files a message carries: each a link to its bytes, opened in a new tab. */
function filesOf(files: NonNullable<OwnerMessage['attachments']>): HTMLElement {
  return h(
    'div.attach-chips.cv-files',
    null,
    files.map((a) => h('a.attach-chip', { href: api.fileUrl(a.id), target: '_blank', rel: 'noopener', title: a.name }, h('span.name', null, a.name), h('span', null, sizeText(a.size)))),
  );
}

/** One message of the chat. */
function bubble(m: OwnerMessage, who: string): HTMLElement {
  if (m.from === 'plaza') return h('div.cv-note', null, renderMarkdown(m.text));
  const failed = m.status === 'failed' || m.status === 'stopped';
  return h(
    `div.cv-msg.${m.from === 'owner' ? 'cv-owner' : 'cv-orch'}`,
    { class: failed ? 'cv-failed' : null },
    h('div.cv-who', null, m.from === 'owner' ? 'You' : who, h('span.cv-when', null, ago(m.at_ms))),
    m.text ? h('div.cv-text', null, m.from === 'owner' ? h('p', null, m.text) : renderMarkdown(m.text)) : null,
    m.attachments?.length ? filesOf(m.attachments) : null,
    failed ? h('div.cv-why', null, m.status === 'failed' ? 'This turn failed.' : 'This turn was stopped.') : null,
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
    h('div.pj-form', null, field('Title', title), field('Goal', goal), field('Where the work goes', repo), field('Budget (credits)', budget), h('div.pj-form-actions', null, save)),
  );
}

export function mountConversation(root: HTMLElement, id: string): Cleanup {
  ensureStyle();
  root.classList.add('cv-host');
  let alive = true;
  let conversation: OwnerConversation | null = null;
  let list: OwnerConversationSummary[] = [];
  let deltas: { work: number; delta: { type: string; text?: string } }[] = [];
  let sending = false;

  const side = h('nav.cv-side', { 'aria-label': 'Conversations' });
  const log = h('div.cv-log', { role: 'log', 'aria-live': 'polite' });
  const live = h('div.cv-live');
  const draftEl = h('aside.cv-draft', { 'aria-label': 'The project draft' });
  const head = h('div.cv-head');

  const composerInput = h('textarea.cv-input', { rows: 2, placeholder: 'Write to the orchestrator… (Enter sends, Shift+Enter breaks the line)' }) as HTMLTextAreaElement;
  const sendBtn = button('Send', () => void send(), { kind: 'primary' });
  // Files: picked, dropped on the conversation or pasted into it. Each uploads as it is taken and goes with the next message.
  const bar = attachBar({ onChange: () => syncSend() });
  const syncSend = (): void => {
    sendBtn.disabled = sending || !conversation || conversation.archived || bar.busy() || bar.failed();
  };
  composerInput.addEventListener('keydown', (ev) => {
    if (ev.key === 'Enter' && !ev.shiftKey && !ev.isComposing) {
      ev.preventDefault();
      void send();
    }
  });

  const main = h('main.cv-main', null, log, live, h('div.cv-composer', null, bar.el, composerInput, bar.button, sendBtn));
  root.append(
    h(
      'div.cv-frame',
      null,
      h(
        'header.cv-top',
        null,
        button('Console', () => (location.hash = consoleOrigin()), { size: 'sm', kind: 'ghost', title: 'Back to the console (Escape)' }),
        head,
      ),
      side,
      main,
      draftEl,
    ),
  );
  const unbind = bar.bind(main);

  const onKey = (e: KeyboardEvent): void => {
    if (e.key === 'Escape' && !document.querySelector('.modal-backdrop')) location.hash = consoleOrigin();
  };
  document.addEventListener('keydown', onKey);

  function drawSide(): void {
    replace(
      side,
      h('div.cv-side-head', null, h('span', null, 'Conversations'), button('New', () => openNewProject(), { size: 'sm', icon: 'plus', title: 'A new conversation, or a project opened directly' })),
      list.length
        ? list.map((c) =>
            h(
              'a.cv-side-row',
              { href: hrefIn(null, 'ide', c.id), 'aria-current': c.id === id ? 'page' : null },
              h('span.cv-side-title', null, c.title),
              h(
                'span.cv-side-meta',
                null,
                c.project ? `${projectTitle(c.project)} · ` : c.draft ? `draft r${c.draft.revision} · ` : '',
                c.pending ? 'answering… · ' : '',
                ago(c.updated_ms),
              ),
            ),
          )
        : h('p.cv-none', null, 'No conversations yet.'),
    );
  }

  function drawHead(): void {
    const c = conversation;
    replace(head, h('strong.cv-title', null, c?.title ?? id), c?.project ? badge(`Project ${projectTitle(c.project)}`, 'accent') : null);
  }

  function drawLog(): void {
    const c = conversation;
    if (!c) return;
    const near = log.scrollHeight - log.scrollTop - log.clientHeight < 80;
    const who = orchestratorName(c.cid);
    replace(log, c.messages.map((m) => bubble(m, who)));
    if (near) log.scrollTop = log.scrollHeight;
    drawLive();
  }

  function drawLive(): void {
    const c = conversation;
    if (!c || !c.pending.length) {
      replace(live);
      return;
    }
    const text = streamed(deltas, c.pending);
    replace(
      live,
      h(
        'div.cv-msg.cv-orch.cv-streaming',
        null,
        h('div.cv-who', null, orchestratorName(c.cid), h('span.cv-when', null, 'answering…')),
        text ? h('div.cv-text', null, renderMarkdown(text)) : h('div.cv-dots', { 'aria-label': 'Thinking' }, '•••'),
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
        h('h2.cv-draft-title', null, 'The project draft'),
        h('p.cv-none', null, 'As you talk, the orchestrator drafts the project here: its goal, its tasks, its team and its budget. You start it when it looks right.'),
      );
      return;
    }
    const roster = agents.get();
    const block = startBlock(c);
    const started = !!c.project;
    replace(
      draftEl,
      h(
        'div.cv-draft-head',
        null,
        h('h2.cv-draft-title', null, d.title),
        h('span.cv-rev', null, `revision ${d.revision} · ${d.updated_by === 'owner' ? 'your edit' : 'the orchestrator'} · ${ago(d.updated_ms)}`),
      ),
      h('p.cv-goal', null, d.goal),
      h(
        'dl.cv-facts',
        null,
        d.repo ? [h('dt', null, 'Repo'), h('dd', null, d.repo)] : null,
        h('dt', null, 'Team'),
        h('dd', null, d.team.length ? teamLine(d.team, roster) : 'Not drafted yet'),
        h('dt', null, 'Budget'),
        h('dd', null, budgetText(d)),
        settingsLine(d.settings) ? [h('dt', null, 'Settings'), h('dd', null, settingsLine(d.settings))] : null,
      ),
      h('h3.cv-sub', null, `Tasks (${d.tasks.length})`),
      d.tasks.length
        ? h(
            'ol.cv-tasks',
            null,
            d.tasks.map((t) =>
              h('li', null, h('span.cv-key', null, t.key), h('span.cv-goal-line', null, t.goal), h('span.cv-role', null, t.role ? roleLabel(t.role) : 'any role'), h('span.cv-after', null, afterText(t))),
            ),
          )
        : h('p.cv-none', null, 'No tasks yet.'),
      started
        ? h(
            'div.cv-started',
            null,
            h('p', null, 'Started as ', h('a', { href: hrefIn(c.project!, 'projects', c.project!) }, projectTitle(c.project)), '. This conversation is its line to the orchestrator.'),
            h('div.row', null, h('a.btn', { href: hrefIn(c.project!, 'work', c.project!) }, 'Open in Work'), h('a.btn', { href: hrefIn(c.project!, 'agents') }, 'Its agents')),
          )
        : h(
            'div.cv-start',
            null,
            block ? h('p.cv-block', null, block) : h('p.cv-ready', null, 'Nothing stops Start.'),
            h(
              'div.row',
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
  }

  async function load(): Promise<void> {
    const [c, l] = await Promise.all([attempt(() => api.ownerConversation(id)), api.ownerConversations().catch(() => list)]);
    if (!alive) return;
    if (c) conversation = c;
    list = l;
    if (!c) {
      replace(log, h('p.cv-none', null, `No conversation ${id}.`));
      return;
    }
    draw();
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
    agents.watch(() => drawDraft()),
  ];
  void load();

  return () => {
    alive = false;
    offs.forEach((off) => off());
    document.removeEventListener('keydown', onKey);
    unbind();
    root.classList.remove('cv-host');
  };
}
