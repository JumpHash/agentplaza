// The owner's side of the board, shared by the board screen and the IDE's board panel: a composer
// that posts to the board (with @mentions, an @all chip and @-suggestions of agent names and roles),
// Pause board / Resume board, Export (Markdown), and the banner a paused board shows. The hub holds
// a paused board's posts and does not deliver their mentions until it resumes.

import '../styles/board-owner.css';
import { api, type Agent, type BoardPost } from '../api';
import { roleLabel } from '../roles';
import { agents } from '../store';
import { button, toast } from '../ui/components';
import { h, replace } from '../ui/dom';
import { mentionsOf } from '../ide/view';

const POST_MAX = 4000;
const SUGGESTIONS = 8;

export interface MentionOption {
  name: string;
  detail: string;
}

/** What an @-mention can name: `all`, then each agent by its name and by its role (no repeats). */
export function mentionOptions(list: Agent[]): MentionOption[] {
  const seen = new Set<string>();
  const out: MentionOption[] = [];
  const add = (name: string, detail: string): void => {
    const key = name.toLowerCase();
    if (!name || seen.has(key)) return;
    seen.add(key);
    out.push({ name, detail });
  };
  add('all', 'everyone');
  for (const agent of list) {
    add(agent.name.trim(), roleLabel(agent.role));
    add(agent.role, 'role');
  }
  return out;
}

/** The `@partial` word before the caret, as its start and its text after the `@`; null when the caret is not in one. */
export function mentionAt(text: string, caret: number): { start: number; query: string } | null {
  const match = /(?:^|[^A-Za-z0-9_])@([A-Za-z0-9._-]*)$/.exec(text.slice(0, caret));
  if (!match) return null;
  const query = match[1] ?? '';
  return { start: caret - query.length - 1, query };
}

/** The names a message mentions, without their `@`, for the post's `mentions`. */
export const mentionNames = (text: string): string[] => mentionsOf(text);

export interface ComposerOpts {
  /** The field's placeholder. */
  placeholder: string;
  /** The field's accessible name. */
  label: string;
  /** The project the post is for, read at send time; undefined is the whole board. */
  project: () => string | undefined;
  /** Called with the post once the hub has stored it. */
  onPosted: (post: BoardPost) => void;
  /** The Loom board's markup (the dock, bd-* classes) instead of the IDE panel's. */
  proto?: boolean;
}

/** The composer at the foot of a board: a textarea with @-suggestions, an @all chip, and Send. */
export function composer(opts: ComposerOpts): { el: HTMLElement; focus: () => void } {
  const proto = !!opts.proto;
  const ta = h(proto ? 'textarea.textarea' : 'textarea.board-owner-text', {
    rows: 2,
    maxlength: POST_MAX,
    placeholder: opts.placeholder,
    'aria-label': opts.label,
    'aria-autocomplete': 'list',
    autocomplete: 'off',
  }) as HTMLTextAreaElement;
  const list = proto
    ? h('div.bd-mlist', { role: 'listbox', 'aria-label': 'People to mention', hidden: true })
    : h('ul.board-owner-suggest', { role: 'listbox', 'aria-label': 'Mention suggestions', hidden: true });
  const meter = h(proto ? 'span.bd-meter' : 'span.board-owner-meter', null, `0/${POST_MAX}`);
  let options: MentionOption[] = [];
  let active = 0;
  let open = false;
  let busy = false;
  let partial: { start: number; query: string } | null = null;

  const sync = (): void => {
    meter.textContent = `${ta.value.length}/${POST_MAX}`;
    send.disabled = !ta.value.trim() || busy;
  };

  const close = (): void => {
    open = false;
    partial = null;
    list.hidden = true;
  };

  const pick = (index: number): void => {
    const option = options[index];
    if (!option) return;
    const caret = ta.selectionStart;
    const from = partial ? partial.start : caret;
    const insert = `@${option.name} `;
    ta.value = `${ta.value.slice(0, from)}${insert}${ta.value.slice(caret)}`;
    const at = from + insert.length;
    ta.setSelectionRange(at, at);
    close();
    sync();
    ta.focus();
  };

  const paint = (): void => {
    replace(
      list,
      options.map((option, i) =>
        proto
          ? h(
              'div.bd-mopt',
              {
                role: 'option',
                'aria-selected': i === active ? 'true' : 'false',
                onmousedown: (e: Event) => {
                  e.preventDefault();
                  pick(i);
                },
              },
              h('b', null, option.name),
              h('span.hint', null, option.detail),
            )
          : h(
              'li.board-owner-option',
              {
                role: 'option',
                'aria-selected': i === active ? 'true' : 'false',
                class: i === active ? 'active' : '',
                onmousedown: (e: Event) => {
                  e.preventDefault();
                  pick(i);
                },
              },
              h('span.board-owner-option-name', null, `@${option.name}`),
              h('span.board-owner-option-detail', null, option.detail),
            ),
      ),
    );
  };

  const suggest = (): void => {
    partial = mentionAt(ta.value, ta.selectionStart);
    if (!partial) return close();
    const needle = partial.query.toLowerCase();
    options = mentionOptions(agents.get())
      .filter((o) => o.name.toLowerCase().includes(needle))
      .slice(0, SUGGESTIONS);
    if (!options.length) return close();
    active = Math.min(active, options.length - 1);
    open = true;
    list.hidden = false;
    paint();
  };

  const send = button('Send', () => void submit(), { kind: 'primary', size: 'sm' });

  const submit = async (): Promise<void> => {
    const text = ta.value.trim();
    if (!text || busy) return;
    busy = true;
    sync();
    try {
      const post = await api.postToBoard({ body: text, project: opts.project(), mentions: mentionNames(text) });
      if (ta.value.trim() === text) ta.value = '';
      close();
      opts.onPosted(post);
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Could not post to the board.', 'error');
    } finally {
      busy = false;
      sync();
    }
  };

  const allChip = button('@all', () => {
    const caret = ta.selectionStart;
    const before = ta.value.slice(0, caret);
    const lead = before && !/\s$/.test(before) ? ' ' : '';
    const insert = `${lead}@all `;
    ta.value = `${before}${insert}${ta.value.slice(caret)}`;
    ta.setSelectionRange(before.length + insert.length, before.length + insert.length);
    ta.focus();
    sync();
  }, { kind: proto ? 'ghost' : undefined, size: 'sm', title: 'Mention everyone on the project (or every agent on the board)' });

  ta.addEventListener('input', () => {
    active = 0;
    suggest();
    sync();
  });
  ta.addEventListener('click', suggest);
  ta.addEventListener('blur', close);
  ta.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      void submit();
      return;
    }
    if (!open) return;
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      active = (active + (e.key === 'ArrowDown' ? 1 : options.length - 1)) % options.length;
      paint();
    } else if (e.key === 'Enter' || e.key === 'Tab') {
      e.preventDefault();
      pick(active);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      close();
    }
  });

  sync();
  const el = proto
    ? h(
        'section.bd-dock',
        { 'aria-label': 'Owner composer' },
        h('div.bd-comp', null, list, ta),
        h('div.bd-foot', null, allChip, meter, send),
      )
    : h(
        'div.board-owner-composer',
        null,
        h('div.board-owner-tools', null, allChip, h('span.board-owner-hint', null, '@ to mention')),
        h('div.board-owner-box', null, list, ta),
        h('div.board-owner-foot', null, meter, send),
      );
  return { el, focus: () => ta.focus() };
}

/** Pause/Resume and Export for one scope (the whole board, or a project), with the paused banner. */
export function boardOwnerBar(opts: { project: () => string | undefined; onChanged: () => void; proto?: boolean }): {
  actions: HTMLElement;
  banner: HTMLElement;
  setPaused: (paused: boolean) => void;
} {
  const proto = !!opts.proto;
  let paused = false;
  const pauseBtn = button('Pause board', () => void toggle(), { size: 'sm' });
  const exportLink = proto
    ? (h('a.btn.ghost.sm', { download: '', title: 'Export the thread as Markdown' }, 'Export') as HTMLAnchorElement)
    : (h('a.btn.sm.board-owner-export', { download: '', title: 'Export the thread as Markdown' }, 'Export') as HTMLAnchorElement);
  const banner = h(
    proto ? 'p.bd-banner' : 'p.board-owner-paused',
    { role: 'status', hidden: true },
    'Paused. Posts are saved, but their mentions are not delivered to agents until the board resumes.',
  );

  const paint = (): void => {
    pauseBtn.textContent = paused ? 'Resume board' : 'Pause board';
    pauseBtn.setAttribute('aria-pressed', paused ? 'true' : 'false');
    banner.hidden = !paused;
    exportLink.href = api.boardExportUrl('md', opts.project());
  };

  const toggle = async (): Promise<void> => {
    pauseBtn.disabled = true;
    try {
      const saved = await api.setBoardPause(!paused, opts.project());
      paused = saved.paused;
      opts.onChanged();
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Could not change the pause.', 'error');
    } finally {
      pauseBtn.disabled = false;
      paint();
    }
  };

  paint();
  return {
    actions: h(proto ? 'div.bd-owner' : 'div.board-owner-actions', null, pauseBtn, exportLink),
    banner,
    setPaused: (next: boolean) => {
      paused = next;
      paint();
    },
  };
}
