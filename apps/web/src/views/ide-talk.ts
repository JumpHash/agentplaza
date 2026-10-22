// The IDE's talk workspace (`#/ide/chat:<id>`): one chat of the plaza's, the owner's line to the orchestrator (`main`) or
// to a project's planner, with the project draft its chat holds and Start. On the left, the chats; in the centre, the
// chat (answers stream in as they are written, and files go with a message); on the right, the draft: its goal, tasks,
// budget and settings, with what still stops Start, the owner's edits, and Start (which answers the draft's start card).
// Once started, the chat stays the project's line to its planner. The markup is the prototype's talk screen (loom:
// m-workspace's talk, m-ide-chat's conversation and draft).

import { api, type Delta } from '../api';
import type { Attachment } from '../api-projects';
import { workApi, type Card, type Chat, type ChatMessage, type ProjectDraft } from '../api-work';
import { contextApi, type ContextView } from '../api-context';
import { afterText, budgetText, draftBudgetOf, settingsLine, startBlock, streamed, talkWorkspace } from '../ide/talk-model';
import { consoleOrigin } from '../ide/open';
import { hrefIn } from '../router';
import { agentName, roleLabel } from '../roles';
import { projectTitle, refreshProjects } from '../scope';
import { agents, on, RESYNC } from '../store';
import { ago, attempt, badge, button, field, input, modal, textarea, toast } from '../ui/components';
import { attachBar, maySend } from '../ui/attach-bar';
import { contextBar, loadModelChoices, modelChip } from '../ui/chat-tools';
import { sizeText } from '../ui/file-drop';
import { h, lu, replace } from '../ui/dom';
import { renderMarkdown } from '../ui/markdown';
import { openNewProject } from '../ui/new-project';
import { capInput } from './budget-model';
import { Answers, amountUnits, choiceButtons, prose, throttle } from './inbox';
import { awaitingReply, doneText, draftPending, openStartCard, settledText } from './chat-model';
import type { Cleanup } from './index';

/** The name a chat's session answers under: its agent's, from the roster, else the chat's kind. */
function speakerName(chat: Chat, cid: number): string {
  const agent = agents.get().find((a) => a.cid === cid);
  if (agent) return agentName(agent);
  return chat.kind === 'main' ? 'The orchestrator' : 'The planner';
}

/** The files a message carries: each a link to its bytes, opened in a new tab. */
function filesOf(files: Attachment[]): HTMLElement {
  return h(
    'div.ic-chips',
    null,
    files.map((a) => h('a.ic-chip', { href: api.fileUrl(a.id), target: '_blank', rel: 'noopener', title: a.name }, `${a.name} · ${sizeText(a.size)}`)),
  );
}

/** One message of the chat: the owner's words as a bubble, a session's answer as its reply, a question as its card, a notice as a note. */
function bubble(m: ChatMessage, who: string, card: Card | undefined, answers: Answers): HTMLElement {
  if (m.kind === 'question') {
    return h(
      'article.ic-msg',
      null,
      h('div.ic-who', null, h('b', null, who), ` · ${ago(m.at_ms)}`),
      prose(m.text),
      card?.state === 'open' ? choiceButtons(card, answers) : card ? h('p.ic-muted', null, settledText(card)) : null,
    );
  }
  if (m.author === 'system' || m.kind === 'notice') return h('div.ic-notice', null, renderMarkdown(m.text));
  const files = m.attachments?.length ? filesOf(m.attachments) : null;
  if (m.author === 'owner') {
    return h('article.ic-msg.ic-owner', null, h('div.ic-who', null, h('b', null, 'You'), ` · ${ago(m.at_ms)}`), m.text ? h('p', null, m.text) : null, files);
  }
  return h('article.ic-msg', null, h('div.ic-who', null, h('b', null, who), ` · ${ago(m.at_ms)}`), h('div.ic-md', null, renderMarkdown(m.text)), files);
}

export function mountTalk(root: HTMLElement, chatId: string): Cleanup {
  root.classList.add('ic-talk-host');
  let alive = true;
  let seq = 0; // the latest load wins
  let chat: Chat | null = null;
  let draft: ProjectDraft | null = null;
  let cards = new Map<string, Card>();
  let list: Chat[] = [];
  /** The chat's session, from its context: the agent whose turns the chat shows and whose stream it is. */
  let cid = 0;
  let deltas: { work: number; delta: { type: string; text?: string } }[] = [];
  let sending = false;

  const side = h('nav.ic-talk-side', { 'aria-label': 'Chats' });
  const log = h('div.ic-log.ic-conv-log', { role: 'log', 'aria-live': 'polite' });
  /** The answer that is still streaming in: the last thing in the log, rebuilt as the words arrive. */
  const liveWrap = h('div.ic-live');
  const draftEl = h('aside.ic-draft.ic-talk-draft', { 'aria-label': 'The project draft' });
  const head = h('div.ic-talk-head');

  const composerInput = h('textarea.ic-input', { rows: 2, placeholder: 'Write a message… (Enter sends, Shift+Enter breaks the line)' }) as HTMLTextAreaElement;
  const sendBtn = button('Send', () => void send(), { kind: 'primary' });
  // Files: picked, dropped on the chat or pasted into it. Each uploads as it is taken and goes with the next message.
  const bar = attachBar({ onChange: () => syncSend() });
  bar.button.className = 'btn ghost';
  bar.button.append('Attach');
  const syncSend = (): void => {
    sendBtn.disabled = sending || !chat || chat.archived || bar.busy() || bar.failed();
  };
  composerInput.addEventListener('keydown', (ev) => {
    if (ev.key === 'Enter' && !ev.shiftKey && !ev.isComposing) {
      ev.preventDefault();
      void send();
    }
  });

  // The model chip joins the tool row; the context line sits above the composer; a save error shows under it. The
  // context names the chat's session (its cid), which the chip and the compress control then act on.
  const chip = modelChip({ label: 'Model', save: (value) => contextApi.setChatModel(chatId, value || null) });
  const fetchContext = async (): Promise<ContextView> => {
    const view = await contextApi.chat(chatId);
    cid = view.cid;
    syncChip();
    return view;
  };
  const ctx = contextBar({ cid: () => cid, source: () => fetchContext() });
  void loadModelChoices('orchestrator').then((c) => chip.setChoices(c), () => undefined);

  const composer = h(
    'div.ic-composer',
    null,
    bar.el,
    h('div.ic-row', null, h('span.ic-sr', null, 'Message'), composerInput, h('div.ic-acts', null, chip.el, bar.button, sendBtn)),
  );
  const main = h(
    'div.ic-talk-main',
    null,
    h('div.ws-main', null, h('section.ic-conv', { 'aria-label': 'Chat' }, log, ctx.el, composer, chip.note)),
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

  const answers = new Answers(
    () => redraw(),
    (answered) => {
      cards.set(answered.id, answered);
    },
    () => void load(),
  );

  const onKey = (e: KeyboardEvent): void => {
    if (e.key === 'Escape' && !document.querySelector('.scrim')) location.hash = consoleOrigin();
  };
  document.addEventListener('keydown', onKey);

  const redraw = (): void => {
    if (alive) draw();
  };

  function drawSide(): void {
    replace(
      side,
      h('div.itk-sec', null, h('span', null, 'Talking it through'), h('span.sp'), button('New', () => openNewProject(), { size: 'sm', icon: 'plus', title: 'A new chat with the orchestrator, or a project opened directly' })),
      list.length
        ? list.map((c) =>
            h(
              'a.itk-t',
              { href: hrefIn(null, 'ide', talkWorkspace(c.id)), 'aria-current': c.id === chatId ? 'true' : null, title: c.title },
              lu(c.draft ? 'file-pen-line' : 'messages-square', 14),
              h('span.n', null, c.title),
              h(
                'span.m',
                null,
                [draftPending(c) && c.draft ? `draft r${c.draft.revision}` : c.kind === 'project' && c.project ? projectTitle(c.project) : '', ago(c.updated_ms)]
                  .filter(Boolean)
                  .join(' · '),
              ),
            ),
          )
        : h('p.ic-none', null, 'No chats yet.'),
    );
  }

  function drawHead(): void {
    const c = chat;
    const chipText = !c
      ? null
      : draft && draftPending(c)
        ? badge(`Draft · revision ${draft.revision}`, startBlock(c, draft) ? 'warn' : 'ok')
        : c.kind === 'project' && c.project
          ? badge(`Project ${projectTitle(c.project)}`, 'accent')
          : null;
    replace(head, h('span.ic-talk-title', null, c?.title ?? chatId), chipText);
  }

  function drawLog(): void {
    const c = chat;
    if (!c) return;
    const near = log.scrollHeight - log.scrollTop - log.clientHeight < 80;
    const who = speakerName(c, cid);
    replace(
      log,
      c.messages.map((m) => bubble(m, m.author === 'owner' ? 'You' : who, m.card ? cards.get(m.card) : undefined, answers)),
      liveWrap,
    );
    if (near) log.scrollTop = log.scrollHeight;
    drawLive();
  }

  function drawLive(): void {
    const c = chat;
    if (!c || !awaitingReply(c.messages)) {
      replace(liveWrap);
      return;
    }
    const text = streamed(deltas, c.works);
    replace(
      liveWrap,
      h(
        'article.ic-msg.ic-live-msg',
        null,
        h('div.ic-who', null, h('b', null, speakerName(c, cid)), ' · answering…'),
        text ? h('div.ic-md', null, renderMarkdown(text)) : h('span.ic-dots', { role: 'img', 'aria-label': 'Thinking' }, h('i'), h('i'), h('i')),
      ),
    );
  }

  /** The owner edits the draft's words and budget, on the revision on screen. */
  function openEdit(d: ProjectDraft): void {
    const unit = d.budget?.unit ?? 'usd';
    const title = input({ value: d.title });
    const goal = textarea({ rows: 4 });
    goal.value = d.goal;
    const repo = input({ value: d.repo ?? '', placeholder: 'agentplaza, a new branch release-0.9' });
    const budget = input({ value: capInput(unit, d.budget?.cap), placeholder: unit === 'usd' ? 'From the plaza’s budget, in dollars' : 'From the plaza’s budget, in tokens' });
    const save = button(
      'Save',
      () => {
        const next = draftBudgetOf(unit, budget.value, d.budget?.monthly_cap);
        if (!next.ok) {
          toast(next.problem, 'error');
          return;
        }
        void (async () => {
          const ok = await attempt(
            () => workApi.editDraft(chatId, { revision: d.revision, title: title.value.trim(), goal: goal.value.trim(), repo: repo.value.trim(), budget: next.value }),
            'Draft saved: the orchestrator sees your change with your next message',
          );
          if (ok) {
            close();
            void load();
          }
        })();
      },
      { kind: 'primary' },
    );
    const close = modal(
      'Edit the draft',
      h('div.ic-form', null, field('Title', title), field('Goal', goal), field('Where the work goes', repo), field('Budget', budget, unit === 'usd' ? 'Dollars.' : 'Tokens.')),
      undefined,
      [save],
    );
  }

  /** Starts the draft: answers its open start card (`start`), which opens the project and posts its tasks. */
  async function start(): Promise<void> {
    const card = openStartCard([...cards.values()]);
    if (!card) return;
    const done = await attempt(() => workApi.answerCard(card.id, 'start'));
    if (!done || !alive) return;
    toast(doneText(done.done));
    await refreshProjects();
    void load();
  }

  function drawDraft(): void {
    const c = chat;
    if (!c) return;
    const d = draft;
    if (!d) {
      replace(
        draftEl,
        h('h2.ic-draft-h', null, 'The project draft'),
        h('p.ic-muted', null, 'The orchestrator drafts the project as you talk, and the draft appears here.'),
      );
      return;
    }
    const block = startBlock(c, d);
    const started = d.started_ms !== undefined && c.project ? c.project : null;
    const startable = openStartCard([...cards.values()]) !== null;
    const facts: [string, string | Node][] = [];
    if (d.repo) facts.push(['Repo', d.repo]);
    facts.push(['Budget', budgetText(d.budget)]);
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
              h('a', { href: hrefIn(started, 'projects', started) }, projectTitle(started)),
              '. This chat is its line to the planner.',
            ),
            h(
              'div.ic-acts',
              null,
              h('a.btn', { href: hrefIn(started, 'work', started) }, 'Open in Work'),
              h('a.btn', { href: hrefIn(started, 'sessions') }, 'Its agents'),
            ),
          )
        : h(
            'div.ic-block',
            null,
            block ? h('p.ic-stop', null, block) : h('p.ic-ready', null, 'Nothing stops Start.'),
            h(
              'div.ic-acts',
              null,
              button('Edit…', () => openEdit(d), { size: 'sm', kind: 'ghost' }),
              button('Start project', () => void start(), {
                kind: 'primary',
                disabled: !!block || !startable,
                title: block ?? (startable ? `Open, staff and plan it at revision ${d.revision}` : 'The draft’s start card is not open yet'),
              }),
            ),
          ),
    );
  }

  /** The chip shows the chat's own model and the model in use by its session (from the agents store). */
  function syncChip(): void {
    const session = cid ? agents.get().find((a) => a.cid === cid) : undefined;
    chip.set({ value: chat?.model ?? '', shown: session?.model ?? '' });
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

  async function load(): Promise<void> {
    const mine = ++seq;
    const [c, l, cs] = await Promise.all([
      workApi.chat(chatId).catch(() => null),
      workApi.chats().catch(() => null),
      workApi.cards(undefined, chatId).catch(() => null),
    ]);
    if (!alive || mine !== seq) return;
    if (l) list = l.filter((x) => x.kind !== 'task');
    if (cs) {
      cards = new Map<string, Card>(cs.map((x): [string, Card] => [x.id, x]));
      answers.units = await amountUnits(cs);
    }
    if (!c) {
      chat = null;
      draft = null;
      replace(log, h('p.muted', null, `No chat ${chatId}.`), liveWrap);
      drawSide();
      return;
    }
    chat = c;
    // A chat whose last message is not the owner's has answered: what streamed for it is no longer pending.
    if (!awaitingReply(c.messages)) deltas = [];
    draft = c.draft ? await workApi.draft(chatId).catch(() => null) : null;
    if (!alive || mine !== seq) return;
    draw();
    ctx.refresh();
    log.scrollTop = log.scrollHeight;
  }

  async function send(): Promise<void> {
    const text = composerInput.value.trim();
    const ids = bar.ids();
    if (!chat || !maySend(text, ids, sending || bar.busy() || bar.failed())) return;
    sending = true;
    draw();
    const posted = await attempt(() => workApi.say(chatId, text, ids));
    sending = false;
    if (!alive) return;
    if (posted) {
      composerInput.value = '';
      bar.clear();
      log.scrollTop = log.scrollHeight;
      void load();
    }
    draw();
  }

  const reload = throttle(() => void load());
  const offs: (() => unknown)[] = [
    // Any chat's change refreshes the list; this chat's messages and draft come with the same reload.
    on('chat', reload.run),
    on('card', reload.run),
    on(RESYNC, reload.run),
    on('delta', (raw) => {
      const d = raw as Delta;
      if (!chat || d.cid !== cid || !chat.works.includes(d.work)) return;
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
    reload.stop();
    offs.forEach((off) => off());
    document.removeEventListener('keydown', onKey);
    unbind();
    ctx.dispose();
    chip.dispose();
    root.classList.remove('ic-talk-host');
  };
}
