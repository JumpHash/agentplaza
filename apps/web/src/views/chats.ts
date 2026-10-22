// Chats: the chats (`#/chats`) with the plaza's orchestrator, with each project's planner and with each task's seats; and
// one chat (`#/chats/<id>`): its last messages, the questions the plaza asks (answered in place, as in the inbox), the
// rename and archive actions, and a composer. Reloads on `chat` frames of that chat, on `card` frames and on RESYNC.

import { workApi, type Card, type Chat, type ChatMessage } from '../api-work';
import { go, href } from '../router';
import { liveProjects, projectTitle, projects, refreshProjects, scope } from '../scope';
import { on, RESYNC } from '../store';
import { ago, attempt, badge, button, card, empty, field, input, modal, select, table, textarea, toast, type Column } from '../ui/components';
import { h, replace } from '../ui/dom';
import { Answers, amountUnits, choiceButtons, kindTone, prose, throttle } from './inbox';
import { readExpert, switchBox, writeExpert } from './work-expert';
import { authorLabel, cardKindLabel, chatKindLabel, clockText, frameChat, messageExpertLine, nonBlank, preselectProject, recentMessages, settledText } from './chat-model';
import type { Cleanup } from './index';

const EXPERT_KEY = 'apz.chats.expert';
/** The messages a chat screen reads: the last of them. */
const MESSAGES = 200;
/** How far, in pixels, from the end a reader still counts as reading the latest messages. */
const AT_END_PX = 48;

export function mount(root: HTMLElement, params: string[]): Cleanup {
  const id = params[0];
  return id ? chatScreen(root, id) : chatList(root);
}

/** The chats of the scope's project, or every chat under All projects. */
function chatList(root: HTMLElement): Cleanup {
  const inProject = scope.get();
  let chats: Chat[] = [];
  let loaded = false;
  let failed = false;
  let archived = false;
  let expert = readExpert(EXPERT_KEY);
  let alive = true;
  let seq = 0;
  const head = h('div.view-head');
  const body = h('div');
  root.append(head, body);

  const columns = (): Column<Chat>[] => {
    const base: Column<Chat>[] = [
      {
        label: 'Chat',
        cell: (c) =>
          h(
            'div',
            null,
            h('a', { href: href('chats', c.id) }, c.title),
            c.kind !== 'main' && c.project ? h('small.muted', { style: 'display: block' }, projectTitle(c.project)) : null,
            c.archived ? badge('Archived') : null,
          ),
      },
      { label: 'Kind', cell: (c) => chatKindLabel(c.kind) },
      { label: 'Last activity', cell: (c) => ago(c.updated_ms), num: true },
    ];
    return expert ? [...base, { label: 'Id', cell: (c) => h('code', null, c.id) }] : base;
  };

  const draw = (): void => {
    replace(
      head,
      h('div', null, h('h1', null, 'Chats')),
      h(
        'div.actions',
        null,
        switchBox('Show archived', archived, (checked) => {
          archived = checked;
          draw();
          void load();
        }),
        switchBox('Expert', expert, (checked) => {
          expert = checked;
          writeExpert(EXPERT_KEY, checked);
          draw();
        }),
        button('New chat', () => void newChat(), { kind: 'primary', icon: 'plus' }),
      ),
    );
    if (failed) {
      replace(body, h('p.muted', { role: 'alert' }, 'Could not load the chats.'));
      return;
    }
    if (!loaded) {
      replace(body);
      return;
    }
    if (!chats.length) {
      replace(body, empty('conversations', inProject ? 'No chats in this project yet' : 'No chats yet', 'Start one with New chat.'));
      return;
    }
    replace(body, table(columns(), chats, (c) => go('chats', c.id), (c) => c.id));
  };
  const redraw = (): void => {
    if (alive) draw();
  };

  // Each load takes a number; a load that a newer one has overtaken writes nothing.
  const load = async (): Promise<void> => {
    const mine = ++seq;
    try {
      const list = await workApi.chats(inProject ?? undefined, archived);
      if (mine !== seq) return;
      chats = list;
      failed = false;
    } catch {
      if (mine !== seq) return;
      failed = true;
    }
    loaded = true;
    redraw();
  };

  const later = throttle(() => void load());
  const offs: (() => unknown)[] = [on('chat', later.run), on(RESYNC, later.run), projects.watch(redraw)];
  void load();

  return () => {
    alive = false;
    later.stop();
    offs.forEach((off) => off());
  };
}

/** The New chat dialog: a live project to talk with its planner, and an optional title. The new chat then opens. */
async function newChat(): Promise<void> {
  // The projects may still be loading: the dialog says there is no live project only once they have loaded.
  if (projects.get().length === 0) await refreshProjects();
  const live = liveProjects(projects.get());
  if (!live.length) {
    toast('There is no live project to start a chat in.');
    return;
  }
  const selected = preselectProject(
    live.map((p) => p.id),
    scope.get(),
  );
  const pick = select(live.map((p) => ({ value: p.id, label: p.title || p.id })), selected ?? undefined);
  const title = input({ placeholder: "Title (the project's title when empty)", 'aria-label': 'Title' });
  let close: () => void = () => undefined;
  const start = button('Start chat', () => void submit(), { kind: 'primary' });
  const cancel = button('Cancel', () => close());
  const submit = async (): Promise<void> => {
    start.disabled = true;
    const chat = await attempt(() => workApi.openChat(pick.value, nonBlank(title.value) ?? undefined));
    if (!chat) {
      start.disabled = false;
      return;
    }
    close();
    go('chats', chat.id);
  };
  close = modal(
    'New chat',
    h('div.stack', null, field('Project', pick), field('Title', title, 'Leave it empty to use the project’s title.')),
    undefined,
    [cancel, start],
  );
}

/** One chat: its messages, questions answered in place, the rename and archive actions, and the composer. */
function chatScreen(root: HTMLElement, id: string): Cleanup {
  let chat: Chat | null = null;
  let missing: string | null = null; // the hub's words when the chat cannot be read
  let cards = new Map<string, Card>();
  let renaming = false;
  let renameText = '';
  let expert = readExpert(EXPERT_KEY);
  let sending = false;
  let placed = false; // the thread has been drawn once: that draw goes to its end
  let alive = true;
  let chatSeq = 0; // the latest read of the chat wins
  let cardsSeq = 0; // the latest read of its questions wins
  const head = h('div.view-head');
  const thread = h('div', { style: 'display: flex; flex-direction: column; gap: 16px; max-height: 60vh; overflow-y: auto; padding-right: 4px' });
  const text = textarea({
    rows: 2,
    placeholder: 'Write a message. Enter sends; Shift+Enter starts a new line.',
    'aria-label': 'Message',
    style: 'flex: 1 1 auto; width: auto; min-height: 0',
  });
  const send = button('Send', () => void submit(), { kind: 'primary' });
  const panel = card(null, h('div.stack', null, thread, h('div.row', { style: 'align-items: flex-end' }, text, send)));
  const body = h('div');
  root.append(head, body);

  const redraw = (): void => {
    if (alive) draw();
  };
  const answers = new Answers(
    redraw,
    (answered) => {
      cards.set(answered.id, answered);
    },
    () => void loadCards(),
  );

  const startRename = (): void => {
    renaming = true;
    renameText = chat?.title ?? '';
    draw();
    head.querySelector<HTMLInputElement>('input:not([type=checkbox])')?.focus();
  };

  const saveTitle = async (): Promise<void> => {
    const title = nonBlank(renameText);
    if (title === null) {
      toast('A chat needs a title.', 'error');
      return;
    }
    const saved = await attempt(() => workApi.editChat(id, { title }), 'Chat renamed');
    if (saved && alive) {
      renaming = false;
      void loadChat();
    }
  };

  const toggleArchive = async (): Promise<void> => {
    if (!chat) return;
    const archive = !chat.archived;
    const done = await attempt(() => workApi.editChat(id, { archived: archive }), archive ? 'Chat archived' : 'Chat restored');
    if (done && alive) void loadChat();
  };

  const drawHead = (): void => {
    if (!chat) {
      replace(head);
      return;
    }
    const c = chat;
    let title: HTMLElement;
    if (renaming) {
      const titleInput = input({ 'aria-label': 'Chat title', value: renameText });
      titleInput.addEventListener('input', () => (renameText = titleInput.value));
      titleInput.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') void saveTitle();
        if (e.key === 'Escape') {
          renaming = false;
          draw();
        }
      });
      title = titleInput;
    } else {
      title = h('h1', null, c.title);
    }
    const project = c.kind !== 'main' && c.project ? projectTitle(c.project) : null;
    const subtitle = [chatKindLabel(c.kind), project].filter(Boolean).join(' · ');
    const actions: (Node | null)[] = renaming
      ? [
          button('Save', () => void saveTitle(), { kind: 'primary' }),
          button('Cancel', () => {
            renaming = false;
            draw();
          }),
        ]
      : [
          button('Rename', startRename),
          c.kind === 'main' ? null : button(c.archived ? 'Restore' : 'Archive', () => void toggleArchive()),
          switchBox('Expert', expert, (checked) => {
            expert = checked;
            writeExpert(EXPERT_KEY, checked);
            draw();
          }),
        ];
    replace(
      head,
      h('div', null, title, h('p.muted', { style: 'margin: 0' }, subtitle, expert ? [' · ', h('code', null, c.id)] : null)),
      h('div.actions', null, actions),
    );
  };

  const drawThread = (): void => {
    if (!chat) return;
    const atEnd = thread.scrollHeight - thread.scrollTop - thread.clientHeight < AT_END_PX;
    const messages = recentMessages(chat.messages, MESSAGES);
    replace(
      thread,
      messages.length ? messages.map((m) => messageView(m)) : h('p.muted', null, 'No messages yet. Write below to start.'),
    );
    if (!placed || atEnd) thread.scrollTop = thread.scrollHeight;
    placed = true;
  };

  /** One message: a question as its card (buttons while it is open), a notice muted, talk with its author. */
  const messageView = (m: ChatMessage): HTMLElement => {
    if (m.kind === 'question') {
      const c = m.card ? cards.get(m.card) : undefined;
      return card(
        badge(c ? cardKindLabel(c.kind) : 'Question', c ? kindTone(c.kind) : 'muted'),
        h(
          'div.stack',
          null,
          prose(m.text),
          c?.state === 'open' ? choiceButtons(c, answers) : c ? h('p.muted', { style: 'margin: 0' }, settledText(c)) : null,
          expert ? h('code', { style: 'overflow-wrap: anywhere' }, messageExpertLine(m)) : null,
        ),
      );
    }
    return h(
      'div',
      { class: m.kind === 'notice' ? 'muted' : undefined, style: 'display: flex; flex-direction: column; gap: 4px' },
      h('div', { style: 'display: flex; gap: 8px; align-items: baseline' }, h('b', null, authorLabel(m)), h('small.muted', null, clockText(m.at_ms))),
      prose(m.text),
      expert ? h('code', { style: 'overflow-wrap: anywhere' }, messageExpertLine(m)) : null,
    );
  };

  const draw = (): void => {
    if (!alive) return;
    drawHead();
    if (missing !== null) {
      body.replaceChildren(empty('conversations', 'Could not open this chat', missing, h('a', { href: href('chats') }, 'All chats')));
    } else if (chat) {
      if (body.firstChild !== panel) body.replaceChildren(panel);
      drawThread();
    } else {
      body.replaceChildren();
    }
  };

  const loadChat = async (): Promise<void> => {
    const mine = ++chatSeq;
    try {
      const next = await workApi.chat(id, MESSAGES);
      if (mine !== chatSeq) return;
      chat = next;
      missing = null;
    } catch (err) {
      if (mine !== chatSeq) return;
      if (!chat) missing = err instanceof Error ? err.message : String(err);
    }
    draw();
  };

  // A card whose budget did not read still updates; only its amount is refused.
  const loadCards = async (): Promise<void> => {
    const mine = ++cardsSeq;
    try {
      const list = await workApi.cards(undefined, id);
      if (mine !== cardsSeq) return;
      const units = await amountUnits(list);
      if (mine !== cardsSeq) return;
      answers.units = units;
      cards = new Map<string, Card>(list.map((c): [string, Card] => [c.id, c]));
    } catch {
      if (mine !== cardsSeq) return;
      // The questions keep what they showed.
    }
    redraw();
  };

  const submit = async (): Promise<void> => {
    const said = nonBlank(text.value);
    if (said === null || sending) return;
    sending = true;
    text.disabled = true;
    send.disabled = true;
    const posted = await attempt(() => workApi.say(id, said));
    sending = false;
    if (!alive) return;
    text.disabled = false;
    send.disabled = false;
    if (posted) {
      text.value = '';
      void loadChat();
    }
    text.focus();
  };
  text.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      void submit();
    }
  });

  const reloadChat = throttle(() => void loadChat());
  const reloadCards = throttle(() => void loadCards());
  const offs: (() => unknown)[] = [
    on('chat', (data) => {
      if (frameChat(data) === id) reloadChat.run();
    }),
    on('card', reloadCards.run),
    on(RESYNC, () => {
      reloadChat.run();
      reloadCards.run();
    }),
    projects.watch(redraw),
  ];
  void loadChat();
  void loadCards();

  return () => {
    alive = false;
    reloadChat.stop();
    reloadCards.stop();
    offs.forEach((off) => off());
  };
}
