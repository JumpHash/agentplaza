// One session, at `#/sessions/<id>`, or at `#/sessions/<cid>`: the session on that VM (the newest one there). Its facts and a
// Stop button, then two tabs: the transcript (the log of its work, oldest first, following the end while the reader is
// there) and the context (what its last model call sent, by part and by tool). The expert view adds the node, the VM,
// the run, the engine, the attempt, the compactions, the raw log entries and the micro-dollars.

import '../styles/proto/sessions.css';
import { workApi, type Session, type SessionContext, type TranscriptEntry } from '../api-work';
import { projects, projectTitle } from '../scope';
import { href } from '../router';
import { on, RESYNC } from '../store';
import { barText, orderedParts, shareText, tokensText, usageTone } from '../ui/chat-tools';
import { ago, attempt, badge, button, card, clip, empty, modal, tabs } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { readExpert, switchBox, writeExpert } from './work-expert';
import {
  clockText,
  contextFill,
  costText,
  count,
  hasCall,
  headingText,
  isCidRef,
  isFinal,
  newestWithCid,
  PLAZA_PROJECT,
  pausedText,
  seatText,
  stateLabel,
  stateTone,
  toolsByResult,
  transcriptLine,
  usageText,
} from './session-model';

/** The expert switch's key, shared with the sessions list so both keep the same setting. */
export const EXPERT_KEY = 'apz.sessions.expert';
/** Log entries read per request (the hub's page is at most 1000). */
const LOG_PAGE = 500;
/** The newest log entries kept in memory while paging; older ones are dropped as new pages arrive. */
const LOG_HELD = 1000;
/** The newest log rows drawn. */
const LOG_ROWS = 300;
const REFRESH_MS = 300;
const HANDOVER_CHARS = 600;
const RAW_CHARS = 2000;

/** One session's log as read so far (`owner` is `<id>@<cid>`): the entries held, oldest first, the count of older ones
 * dropped, and the seq the next read starts after. */
interface Log {
  owner: string;
  entries: TranscriptEntry[];
  dropped: number;
  cursor: number;
}

/** The read that failed last (`step` names what it read: "the session", "the context", "the transcript"), and why. */
interface Failure {
  step: string;
  message: string;
}

/** One session, by its id (`s12`) or by the CID of the VM it runs on (`105`). */
export function mountSession(root: HTMLElement, ref: string): Cleanup {
  let session: Session | null = null;
  let loaded = false;
  let failure: Failure | null = null;
  let tab: 'transcript' | 'context' = 'transcript';
  let expert = readExpert(EXPERT_KEY);
  let stopping = false;
  let context: SessionContext | null = null;
  let log: Log | null = null;
  let alive = true;
  let seq = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;

  /** A throttle: a frame starts a refresh in REFRESH_MS unless one is already due, so a busy stream never starves it. */
  const schedule = (): void => {
    timer ??= setTimeout(() => void refresh(), REFRESH_MS);
  };

  const offs = [
    on('session', (data) => {
      // By id, only this session's frames count; by CID, any session may be the VM's newest.
      const id = (data as Session | null)?.id;
      if (isCidRef(ref) || (id !== undefined && id === session?.id)) schedule();
    }),
    on('transcript', (data) => {
      if (session && (data as { cid?: number } | null)?.cid === session.cid) schedule();
    }),
    on(RESYNC, schedule),
  ];

  /**
   * Reads one session's log after its last entry seen, a page at a time, until the end. Entries already held are skipped,
   * and only the newest LOG_HELD are kept. Returns early when the view is gone or the log now belongs to another session.
   */
  const readLog = async (id: string, current: Log): Promise<void> => {
    for (;;) {
      if (!alive || log !== current) return;
      const page = await workApi.transcript(id, current.cursor, LOG_PAGE);
      if (!alive || log !== current) return;
      let fresh = 0;
      for (const e of page) {
        if (e.seq <= current.cursor) continue;
        current.entries.push(e);
        current.cursor = e.seq;
        fresh++;
      }
      const over = current.entries.length - LOG_HELD;
      if (over > 0) {
        current.entries.splice(0, over);
        current.dropped += over;
      }
      if (page.length < LOG_PAGE || fresh === 0) return;
    }
  };

  /** Reads the session, its context on the Context tab, and its log; then draws the page. */
  const refresh = async (): Promise<void> => {
    clearTimeout(timer);
    timer = undefined;
    const mine = ++seq;
    let step = 'the session';
    try {
      const next = isCidRef(ref) ? newestWithCid(await workApi.sessions(), Number(ref)) : await workApi.session(ref);
      if (!alive || mine !== seq) return;
      session = next;
      failure = null;
      loaded = true;
      if (next) {
        // A new session on the VM (or a move to another VM) starts its own log and context.
        const owner = `${next.id}@${next.cid ?? ''}`;
        let current = log;
        if (!current || current.owner !== owner) {
          current = { owner, entries: [], dropped: 0, cursor: 0 };
          log = current;
          context = null;
        }
        if (tab === 'context') {
          // Its own read: a failed context is shown, and the transcript read below still runs.
          try {
            context = await workApi.context(next.id);
          } catch (err) {
            if (!alive || mine !== seq) return;
            failure = { step: 'the context', message: err instanceof Error ? err.message : String(err) };
          }
        }
        if (!alive || mine !== seq) return;
        draw();
        if (next.cid !== null) {
          step = 'the transcript';
          await readLog(next.id, current);
        }
      }
    } catch (err) {
      if (!alive || mine !== seq) return;
      failure = { step, message: err instanceof Error ? err.message : String(err) };
      loaded = true;
    }
    if (!alive || mine !== seq) return;
    draw();
  };

  const stop = async (s: Session): Promise<void> => {
    stopping = true;
    draw();
    const ended = await attempt(() => workApi.stopSession(s.id), 'Session stopped');
    if (!alive) return;
    stopping = false;
    if (ended) session = ended;
    draw();
  };

  /** Stop: a long-lived session (a planner or the orchestrator) is confirmed first; a step stops at once. */
  const askStop = (s: Session): void => {
    if (s.kind !== 'long') {
      void stop(s);
      return;
    }
    const close = modal(
      'Stop this session?',
      h('p', null, 'A planner or the orchestrator runs as one long-lived session. Stopping it ends its work now.'),
      undefined,
      [
        button('Keep running', () => close(), { kind: 'ghost' }),
        button(
          'Stop',
          () => {
            close();
            void stop(s);
          },
          { kind: 'danger' },
        ),
      ],
    );
  };

  const backLink = (): HTMLElement => h('a.btn', { href: href('sessions') }, 'All sessions');

  const head = (s: Session): HTMLElement => {
    const reason = s.state === 'paused' ? pausedText(s.paused_by) : null;
    return h(
      'header.view-head',
      null,
      h('div', null, h('span.se-id', null, s.id), h('h1', null, headingText(s))),
      h(
        'div.actions',
        null,
        badge(stateLabel(s.state), stateTone(s.state), true),
        reason ? h('span.se-reason', null, reason) : null,
        button('Stop', () => askStop(s), { kind: 'danger', disabled: isFinal(s.state) || stopping }),
        switchBox('Expert', expert, (checked) => {
          expert = checked;
          writeExpert(EXPERT_KEY, checked);
          draw();
        }),
      ),
    );
  };

  const facts = (s: Session): HTMLElement => {
    const u = s.usage;
    const link = (url: string, text: string): HTMLElement => h('a', { href: url }, text);
    const pairs: [string, Node | string, boolean][] = [
      ['Seat', seatText(s), false],
      ['Task', s.task ? link(href('tasks', s.task), s.task) : h('span.muted', null, 'No task'), false],
      ['Project', s.project === PLAZA_PROJECT ? 'The plaza' : link(href('projects', s.project), projectTitle(s.project)), false],
      ['Model', s.model ? h('span.mono', null, s.model) : '—', false],
      [
        'Usage',
        `${usageText(u)} · ${tokensText(u.tokens_cached)} cached · ${costText(u.cost_micros)} · ${count(u.tool_calls, 'tool call', 'tool calls')}`,
        false,
      ],
      ['Started', ago(s.started_ms), false],
      ['Ended', ago(s.ended_ms), false],
    ];
    if (s.verdict !== null) pairs.push(['Verdict', s.verdict ? 'Passed' : 'Not passed', false]);
    if (s.outcome) pairs.push(['Outcome', s.outcome, true]);
    if (s.handover && isFinal(s.state)) pairs.push(['Handover', h('p.se-text', null, clip(s.handover, HANDOVER_CHARS)), true]);
    if (expert) {
      pairs.push(
        ['Node', s.node ?? '—', false],
        ['VM', s.cid === null ? '—' : String(s.cid), false],
        ['Run', s.run ?? '—', false],
        ['Engine', s.engine, false],
        ['Attempt', String(s.attempt), false],
        ['Compactions', String(s.compactions), false],
        ['Model calls', String(u.model_calls), false],
        ['Cost, micro-dollars', `${u.cost_micros.toLocaleString('en-US')} µ$`, false],
      );
    }
    return h(
      'dl.kv',
      null,
      pairs.map(([k, v, wide]) => h('div', { class: wide ? 'se-wide' : null }, h('dt', null, k), h('dd', null, v))),
    );
  };

  /** One log row: its time, its label (with a note), its text, and in expert its sequence number and raw data. */
  const logRow = (e: TranscriptEntry): HTMLElement => {
    const line = transcriptLine(e);
    const labelClass = line.alert ? 'se-alert' : line.known ? '' : 'se-faint';
    return h(
      'div.se-row',
      null,
      h('time.se-time', null, clockText(e.at_ms)),
      h(
        'div',
        null,
        h(
          'div.se-head',
          null,
          h('span.se-label', { class: labelClass }, line.label),
          line.note ? h('span.se-reason', null, line.note) : null,
          expert ? h('span.se-id', null, `#${e.seq}`) : null,
        ),
        line.text ? (line.mono ? h('pre.se-out', null, line.text) : h('p.se-text', null, line.text)) : null,
        expert ? h('pre.se-out.se-raw', null, clip(JSON.stringify(e.data), RAW_CHARS)) : null,
      ),
    );
  };

  /** The log's scroll box: the newest rows, after a note for the entries that were read but are not drawn. */
  const logBoxOf = (s: Session): HTMLElement => {
    const entries = log?.entries ?? [];
    const rows = entries.slice(-LOG_ROWS);
    const earlier = (log?.dropped ?? 0) + entries.length - rows.length;
    const box = h('div.se-tr', { role: 'log' });
    if (earlier > 0) box.append(h('p.se-note', null, `${earlier} earlier entries are not shown.`));
    if (!entries.length) box.append(h('p.muted', null, s.cid === null ? 'Nothing has run in this session yet.' : 'No entries yet.'));
    box.append(...rows.map(logRow));
    return box;
  };

  /** The context card: the fill bar, the parts and the tools; before the first model call, a line saying so. */
  const contextCard = (): HTMLElement => {
    const ctx = context;
    if (!ctx) return card(null, h('p.muted', null, failure ? 'No context to show.' : 'Loading…'));
    if (!hasCall(ctx)) {
      return card(null, empty('agents', 'No model call yet', 'The context shows here once the session has made its first call.'));
    }
    const used = ctx.used;
    const windowTokens = ctx.window && ctx.window > 0 ? ctx.window : null;
    const fill = contextFill(ctx);
    const total = Math.max(windowTokens ?? 0, used);
    const parts = orderedParts(ctx.parts);
    const segs = parts
      .filter((p) => p.tokens > 0 && total > 0)
      .map((p) =>
        h('i.ct-part', { style: `width:${((p.tokens / total) * 100).toFixed(2)}%;background:${p.colour}`, title: `${p.name} · ${tokensText(p.tokens)}` }),
      );
    const bar = h(
      'div.ct-ctx',
      { 'data-tone': usageTone(fill) },
      h('span.ct-track', null, segs),
      h('span.ct-ctx-txt', null, barText(used, windowTokens, fill)),
    );
    const partRows = h(
      'div.ct-rows',
      null,
      parts.map((p) =>
        h(
          'div.ct-row.ct-row-part',
          null,
          h('i.ct-sw', { style: `background:${p.colour}` }),
          h('span.ct-cell', null, p.name),
          h('span.ct-num', null, tokensText(p.tokens)),
          h('span.ct-num.ct-share', null, shareText(p.tokens, used)),
        ),
      ),
    );
    const tools = toolsByResult(ctx.tools ?? []);
    const toolRows = tools.length
      ? h(
          'div.ct-rows',
          null,
          tools.map((t) =>
            h(
              'div.ct-row.ct-row-tool',
              null,
              h('span.ct-cell.mono', { title: t.tool }, t.tool),
              h('span.ct-num', null, count(t.calls, 'call', 'calls')),
              h('span.ct-num', { class: t.errors > 0 ? 'ct-crit' : null }, count(t.errors, 'error', 'errors')),
              h('span.ct-num', null, `${tokensText(t.result_tokens)} result`),
            ),
          ),
        )
      : h('p.ct-empty', null, 'No tool calls yet');
    return card(null, [
      bar,
      h('div.ct-bd-sec', null, 'Parts'),
      partRows,
      h('div.ct-bd-sec', null, 'Tools'),
      toolRows,
      h('p.se-note', null, `${count(ctx.calls ?? 0, 'model call', 'model calls')} · ${count(ctx.compactions ?? 0, 'compaction', 'compactions')}`),
    ]);
  };

  /** Draws the page. The log keeps the reader's place: at its end it follows new entries, scrolled up it stays put. */
  const draw = (): void => {
    const prev = root.querySelector<HTMLElement>('.se-tr');
    const atEnd = !prev || prev.scrollHeight - prev.scrollTop - prev.clientHeight < 40;
    const top = prev?.scrollTop ?? 0;
    if (!loaded) {
      replace(root, h('p.muted', null, 'Loading…'));
      return;
    }
    if (!session) {
      replace(
        root,
        failure
          ? empty('alert', `Could not load ${failure.step}`, failure.message, backLink())
          : empty('agents', `No session runs on VM ${ref} now`, undefined, backLink()),
      );
      return;
    }
    const s = session;
    const tabEls = tabs(
      [
        { key: 'transcript', label: 'Transcript' },
        { key: 'context', label: 'Context' },
      ],
      tab,
      (key) => {
        tab = key === 'context' ? 'context' : 'transcript';
        draw();
        void refresh();
      },
    );
    // The last read that failed, as one line above the tab strip; the session stays shown.
    const failed = failure ? h('p.hint.err', null, `Could not load ${failure.step}: ${failure.message}`) : null;
    const box = tab === 'transcript' ? logBoxOf(s) : null;
    replace(root, head(s), card('Details', facts(s)), failed, tabEls, box ? card(null, box) : contextCard());
    if (box) box.scrollTop = atEnd ? box.scrollHeight : top;
  };

  // The Project fact shows the projects' titles. A projects read sets them without a frame, so each set redraws the page.
  const offProjects = projects.watch(draw);
  void refresh();

  return () => {
    alive = false;
    clearTimeout(timer);
    offProjects();
    offs.forEach((off) => off());
  };
}
