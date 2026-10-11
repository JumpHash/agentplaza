/* The store — the single copy of daemon state. Every screen renders from
   this; no view keeps its own divergent copy. Runs arrive whole from the
   daemon; serial text is append-only per VM; activity is derived from poll
   deltas. Rendering is coalesced through a microtask: the paint path never
   depends on animation frames, so hidden tabs still paint what they have. */

import { getBoard, getContacts, getConversationThread, getConversations, getAgora, getHub, getMachine, getMotions, getRun, getTask, getTreasury, getTreasuryJournal, listRuns, listTasks, readTaskArtifact, decideMotion as decideMotionApi, stopAgent as stopAgentApi, tellAgent } from './api';
import { ActivityTracker, vmKey } from './models/activity';
import type { Call } from './models/agora';
import type { ArtifactReadState } from './models/artifact';
import { readStateOf } from './models/artifact';
import type { DecisionState, Motion, MotionState } from './models/council';
import { decisionStateOf, motionSeq } from './models/council';
import type { RunSlice } from './models/history';
import { SessionHistory } from './models/history';
import type { HubStatus } from './models/hub';
import type { Machine } from './models/machine';
import type { Run, RunState, VmState } from './models/run';
import { isOpen, seqOf } from './models/run';
import type { BoardPost, Contact, Conversation, ConversationPair, UnreadCount } from './models/social';
import { ACTIVE_SECS, socialSeq } from './models/social';
import type { Task, TaskState } from './models/task';
import { isFinishedTask, seqOf as taskSeq } from './models/task';
import type { StopState } from './models/stop';
import { stopStateOf } from './models/stop';
import type { TellState } from './models/tell';
import { tellStateOf } from './models/tell';
import type { JournalEntry, Treasury } from './models/treasury';
import type { SerialCache } from './serial';
import { linesOf, serialDelta } from './serial';
import { toast } from './dom';

export type ViewName = 'register' | 'run' | 'machine' | 'map' | 'floor' | 'compare' | 'telemetry' | 'roster' | 'ledger' | 'treasury' | 'market' | 'board' | 'talk' | 'council';
export type Theme = 'system' | 'light' | 'dark';
export type Health = 'live' | 'stale' | 'down';
/** The event stream's own liveness — shown in the colophon, never feeds daemon health. */
export type StreamState = 'connecting' | 'live' | 'reconnecting';

declare global {
  interface Window {
    /** Verification hook: a harness may set this before boot to slow the
        poll clock, proving the event stream — not the polls — carries
        liveness. Production never sets it. */
    __PLAZA_TICK_MS__?: number;
    /** Read-only counters for the verification harness; the app never reads them. */
    __PLAZA_DIAG__?: {
      readonly renders: number;
      readonly wire: { readonly run: number; readonly vm: number; readonly serial: number; readonly task: number; readonly treasury: number; readonly agora: number; readonly board: number; readonly conversation: number; readonly motion: number };
      readonly stream: StreamState;
      /** What the hub line reads from: whether the hub is reachable, its
          agent and session counts, and the error when it is not. */
      readonly hub: { readonly reachable: boolean; readonly agents: number; readonly sessions: number; readonly error: string | null };
      /** The ledger as the store holds it: how many tasks, how many still in
          flight, which one is pinned, and the error when it is unreadable. */
      readonly tasks: {
        readonly count: number;
        readonly open: number;
        readonly detail: string | null;
        readonly error: string | null;
      };
      /** The artifact the ledger's sheet last read, if any: which task and
          path, and how the read ended. */
      readonly artifact: { readonly task: string; readonly path: string; readonly state: string } | null;
      /** The tool console's last drive, if any: which agent, which tool, and
          how it ended. */
      readonly tell: { readonly cid: number; readonly name: string; readonly state: string; readonly status: number | null } | null;
      /** The console's last stop, if any: which agent, how it ended, the hub's
          own verdict (null when it never answered) and its status. */
      readonly stop: { readonly cid: number; readonly state: string; readonly stopped: boolean | null; readonly status: number | null } | null;
      /** The treasury as the store holds it: how many accounts, how many
          journal entries, the totals line's credits, and the error when either
          half is unreadable. */
      readonly treasury: {
        readonly accounts: number;
        readonly entries: number;
        readonly total: number;
        readonly credits: number;
        readonly error: string | null;
        readonly journalError: string | null;
      };
      /** The market as the store holds it: how many calls the board carries,
          how many still offer (stand unclaimed), how many bids and awards they
          hold, and the error when the board is unreadable. */
      readonly market: {
        readonly calls: number;
        readonly open: number;
        readonly bids: number;
        readonly awarded: number;
        readonly total: number;
        readonly error: string | null;
      };
      /** The floor as the scene holds it: the robots on the plan, the
          stations, the gatherings being walked, the states they wear, the
          walks in flight and the fixed camera's own frame. */
      readonly floor: {
        readonly robots: number;
        readonly stations: number;
        readonly gatherings: number;
        readonly states: Record<string, number>;
        readonly selected: number | null;
        readonly walks: number;
        readonly positions: ReadonlyArray<{
          readonly cid: number | null;
          readonly x: number;
          readonly z: number;
          readonly gathering: string | null;
        }>;
        readonly tint_cycle: number;
        readonly camera: { readonly zoom: number; readonly pan: readonly number[]; readonly view_h: number };
        readonly plan: { readonly width: number; readonly height: number };
        readonly label: string | null;
      } | null;
      /** The public board as the store holds it: how many posts the console
          carries, the board's own total, the read cursors it reports, and the
          error (with its status) when the board is unreadable. */
      readonly board: {
        readonly posts: number;
        readonly total: number;
        readonly truncated: boolean;
        readonly unread: number;
        readonly error: string | null;
        readonly errorStatus: number | null;
      };
      /** The social layer: the conversations, the node's own pair rows, the
          contacts the console has read, and the thread it is showing. */
      readonly social: {
        readonly conversations: number;
        readonly pairs: number;
        readonly active: number;
        readonly activeSecs: number;
        readonly contacts: readonly number[];
        readonly thread: { readonly a: number; readonly b: number; readonly state: string } | null;
        readonly talkPair: { readonly a: number; readonly b: number } | null;
        readonly error: string | null;
        readonly errorStatus: number | null;
      };
      /** The council as the store holds it: how many motions the console
          carries, the bounding line, the state the sheet is narrowed to, and
          the error (with its status) when the council is unreadable. */
      readonly council: {
        readonly motions: number;
        readonly total: number;
        readonly truncated: boolean;
        readonly open: number;
        readonly awaiting: number;
        readonly ties: number;
        readonly filter: string | null;
        readonly error: string | null;
        readonly errorStatus: number | null;
        /** The owner's last decision, if any: which motion, how it ended, and
            the status. */
        readonly decision: { readonly id: string; readonly state: string; readonly status: number | null } | null;
      };
    };
  }
}

/** The pair a #compare route names: run a's vm avm against run b's vm bvm. */
export interface CompareTarget {
  a: string;
  avm: number;
  b: string;
  bvm: number;
}

export interface Route {
  name: ViewName;
  param: string | null;
  compare: CompareTarget | null;
}

/** One artifact the ledger's sheet has read — or is reading — for a task. */
export interface ArtifactRead {
  /** The task whose agent read it. */
  task: string;
  /** The path asked for, as the sheet sent it. */
  path: string;
  state: ArtifactReadState;
  /** The text, once read; empty otherwise. */
  content: string;
  /** The file's size in bytes, as the agent reported it. */
  size: number;
  /** Whether the agent's read cap, not the file's end, ended the content. */
  truncated: boolean;
  /** The guest CID of the agent that read it, or 0 before it answers. */
  cid: number;
  /** A refusal or failure, in whoever wrote it — verbatim, never paraphrased. */
  error: string;
  /** The HTTP status, or null when the node never answered. */
  status: number | null;
}

/** One tool call the console drove on a connected agent — the agent's own
    answer, or the refusal, kept as it came. */
export interface TellRead {
  /** The guest CID of the agent the call was driven on. */
  cid: number;
  /** The tool asked for, as the console sent it. */
  name: string;
  /** The arguments sent: a JSON object (the daemon refuses anything else). */
  args: Record<string, unknown>;
  /** How long the console asked the agent to take, in ms. */
  timeoutMs: number;
  state: TellState;
  /** The agent's own `ok`, when it answered (state `sent`). */
  ok: boolean;
  /** The agent's own output (state `sent`), else empty. */
  output: string;
  /** Whoever refused, in their own words — verbatim, never paraphrased. */
  error: string;
  /** The HTTP status, or null when the node never answered. */
  status: number | null;
  /** When the call settled (or started driving), in the console's clock. */
  at: number;
}

/** One stop the console asked for — the hub's own answer, or the refusal, kept
    as it came. Like a driven call, a stop is the owner's hand alone: it ends a
    real session, so it is called from a click and never from a poll, and the
    console reads its result from here. */
export interface StopRead {
  /** The guest CID of the agent whose session was addressed. */
  cid: number;
  /** The reason the console sent ('' when it sent none — the hub's default). */
  reason: string;
  state: StopState;
  /** The hub's own verdict: `true`/`false` when it answered 200; `null` when it
      never did (the state says how the stop ended instead). */
  stopped: boolean | null;
  /** The ended session's call count, when the hub said the session ended. */
  calls: number | null;
  /** The hub's own reason a session still stands, or whoever refused, in their
      own words — verbatim, never paraphrased. */
  detail: string;
  /** The HTTP status, or null when the node never answered. */
  status: number | null;
  /** When the stop settled (or started), in the console's clock. */
  at: number;
}

/** One owner's decision the console took on a motion — the motion it names,
    whether the owner ratified or vetoed, and the node's own answer or refusal,
    kept as it came. Like a driven call and a stop it is the owner's hand alone,
    set only by a click and never by a poll. */
export interface DecisionRead {
  /** The motion the owner decided. */
  id: string;
  /** Whether the owner approved it (`true` ratified, `false` vetoed). */
  approve: boolean;
  /** The note the console sent ('' when the box was empty). */
  note: string;
  state: DecisionState;
  /** Whoever refused, in their own words — verbatim, never paraphrased. */
  detail: string;
  /** The HTTP status, or null when the node never answered. */
  status: number | null;
  /** When the decision settled (or started), in the console's clock. */
  at: number;
}

/** One agent's contact list as the console last read it. Asked for when a
    card opens and never polled: the list is the agent's own, and the node
    emits no frame when it changes (docs/agents/social.md). */
export interface ContactsRead {
  cid: number;
  state: 'loading' | 'read' | 'error';
  contacts: Contact[];
  /** How many entries the agent's list holds (may exceed the bounded copy). */
  total: number;
  truncated: boolean;
  /** The refusal, in whoever wrote it — verbatim, never paraphrased. */
  error: string;
}

/** One pair's thread as the console last read it, newest first (the node
    sends it oldest first; the card reads the way the register reads). Only
    ever set by the owner's own click — a thread is asked for, not prefetched. */
export interface ThreadRead {
  a: number;
  b: number;
  state: 'loading' | 'read' | 'error';
  exchanges: Conversation[];
  total: number;
  truncated: boolean;
  error: string;
}

const TICK_MS =
  typeof window !== 'undefined' && typeof window.__PLAZA_TICK_MS__ === 'number' && window.__PLAZA_TICK_MS__ > 0
    ? window.__PLAZA_TICK_MS__
    : 1000; // detail cadence; the list polls every 2 ticks
const MACHINE_EVERY = 15; // ticks between machine polls
const HUB_EVERY = 15; // ticks between hub polls (the hub's status is frozen and slow)
const TASKS_EVERY = 2; // ticks between task-ledger polls (the ledger is small and changes fast)
const TREASURY_EVERY = 2; // ticks between treasury polls (accounts + journal ride the task cadence)
const AGORA_EVERY = 2; // ticks between agora polls (the board is small and changes fast — bids land in bursts)
const BOARD_EVERY = 2; // ticks between public-board polls (posts land in bursts; the cursor column moves with them)
const CONVERSATIONS_EVERY = 2; // ticks between conversation polls (the floor's proximity derives from them)
const MOTIONS_EVERY = 2; // ticks between council polls (a motion's votes land in bursts — the seats vote as they read)
const LIVE_WINDOW_MS = 4000; // a success older than this reads "stale"
const EVENT_REFRESH_MS = 150; // a burst of run events collapses into one list GET
const LIVE_RENDER_MS = 100; // the live lane's bound: at most this often under a serial burst
const VM_PUSH_MIN_MS = 1000; // a VM's sparkline ring gains at most one point per this window
const TELL_HISTORY = 5; // the tool console keeps the last few calls per agent, no more

export class Store {
  /* daemon state */
  machine: Machine | null = null;
  machineError: string | null = null;
  /** The hub's frozen status (through the daemon's proxy), or the reason it
      is unreadable. Its own upstream: a hub down is NOT the daemon down, so
      neither success nor failure here feeds `health`. */
  hub: HubStatus | null = null;
  hubError: string | null = null;
  runs: Run[] = []; // newest first
  runsError: string | null = null;
  /** The ledger's own copy of every task, newest first (the daemon lists them
      oldest first; the console reads them the way the register reads runs).
      The SSE `task` events patch these in place, exactly as the run events
      patch `runs`. */
  tasks: Task[] = []; // newest first
  tasksError: string | null = null;
  /** The treasury's own copy of its accounts, totals and zero-spend note. */
  treasury: Treasury | null = null;
  treasuryError: string | null = null;
  /** The double-entry journal, newest first (the daemon lists it oldest first,
      the way the register reads runs). The `treasury` SSE frames patch these
      in, exactly as the run events patch `runs`. */
  journal: JournalEntry[] = []; // newest first
  /** How many entries the journal holds (may exceed the bounded copy). */
  journalTotal = 0;
  /** Whether the daemon's response was cut to its own limit. */
  journalTruncated = false;
  journalError: string | null = null;
  /** The market's own copy of the board, newest first (the daemon lists the
      calls oldest first, the way the register reads runs). The SSE `agora`
      frames ask for it, exactly as the task events ask for the ledger. */
  calls: Call[] = []; // newest first
  /** How many calls the board holds (may exceed the bounded copy). */
  callsTotal = 0;
  /** Whether the daemon's response was cut to its own limit. */
  callsTruncated = false;
  callsError: string | null = null;
  /** The public board, newest first (the daemon lists posts newest first; the
      console reads them the way the register reads runs). The `board` SSE
      frames ask for it, exactly as the task events ask for the ledger. */
  board: BoardPost[] = []; // newest first
  /** How many posts the board holds (may exceed the bounded copy). */
  boardTotal = 0;
  boardLimit = 0;
  boardTruncated = false;
  /** Every connected agent's unread count — how "who has read what" is read. */
  boardUnread: UnreadCount[] = [];
  boardError: string | null = null;
  /** The status behind `boardError`, so a node without the social layer is
      told apart from a node that is merely unreachable (a 404). */
  boardErrorStatus: number | null = null;
  /** The conversations the plaza is holding, newest first, with the node's own
      pair rows and the window it calls a pair active. The floor's proximity
      derives from these; the `conversation` SSE frames ask for them. */
  conversations: Conversation[] = []; // newest first
  convPairs: ConversationPair[] = [];
  /** The window the node itself calls a pair active, in seconds. */
  convActiveSecs = ACTIVE_SECS;
  convTotal = 0;
  convLimit = 0;
  convTruncated = false;
  conversationsError: string | null = null;
  conversationsErrorStatus: number | null = null;
  /** The council's motions, newest first (the daemon lists them newest first;
      the console reads them the way the register reads runs). The `motion` SSE
      frames ask for them: a frame carries the motion's id, kind and state and
      NO text, so the motion's own row — its text and its votes' reasons — is
      read from the node and never patched from a frame. */
  motions: Motion[] = []; // newest first
  /** How many motions the council holds (may exceed the bounded copy). */
  motionsTotal = 0;
  motionsLimit = 0;
  motionsTruncated = false;
  motionsError: string | null = null;
  /** The status behind `motionsError`, so a node without a council is told
      apart from a node that is merely unreachable (a 404). */
  motionsErrorStatus: number | null = null;
  /** The state the council sheet is narrowed to, or null for every motion. A
      view preference: the sheet filters the motions it reads from `motions`. */
  motionFilter: MotionState | null = null;
  /** Per-agent contact lists, by CID — read on demand, never polled. */
  readonly contacts = new Map<number, ContactsRead>();
  /** The one pair's thread the card is reading, or null. */
  thread: ThreadRead | null = null;
  /** The pair the talk sheet is narrowed to, or null for the whole log. A view
      preference: the sheet reads the thread it asks for from `thread`. */
  talkPair: { a: number; b: number } | null = null;
  /** The task the ledger sheet is pinned to (`#ledger/{id}`), or null. */
  taskDetailId: string | null = null;
  /** The artifact the ledger's sheet last asked for — the one read in flight,
      or the last one it answered. Only ever set by the owner's own click: the
      read-through drives the agent's session, so it is never polled. */
  artifact: ArtifactRead | null = null;
  /** The tool console's own history, per agent CID: the last few tool calls
      the owner drove, newest first. Bounded ([`TELL_HISTORY`]) and dropped
      when the agent leaves the fleet. Only ever set by the owner's own click:
      a drive runs a real tool in the agent's session, so it is never polled. */
  readonly tells = new Map<number, TellRead[]>();
  /** The newest drive, whatever agent — the console's own readback of the last
      call it made. */
  lastTell: TellRead | null = null;
  /** The last stop the console asked for, whatever agent. Like a drive it is
      the owner's hand alone, set only by a click, and the console reads its
      result from here for the agent it names (its own `cid`). */
  lastStop: StopRead | null = null;
  /** The last owner's decision the console took on a motion, whatever motion —
      the node's own answer or the refusal, kept as it came. The owner's hand
      alone, set only by a click. */
  lastDecision: DecisionRead | null = null;
  /** Every run fetched with serial, by id (map/floor poll each open run). */
  readonly details = new Map<string, Run>();
  detailError: string | null = null;

  /* view state */
  detailId: string | null = null;
  vm = 0; // selected VM index within the open run
  follow = true; // transcript tail-follow
  theme: Theme = 'system';
  route: Route = { name: 'register', param: null, compare: null };
  /** The telemetry view's run pick; null is the whole fleet. */
  telemetryRun: string | null = null;

  /* derived caches */
  readonly serial = new Map<string, SerialCache>(); // "r3/0" -> lines
  readonly activity = new ActivityTracker();
  readonly history = new SessionHistory();

  /* health */
  private lastOkAt = 0;
  private lastErrAt = 0;

  /* the live wire (GET /api/v0/events): its own liveness and tallies —
     informational only; daemon health stays with the polls */
  stream: StreamState = 'connecting';
  readonly wire = { run: 0, vm: 0, serial: 0, task: 0, treasury: 0, agora: 0, board: 0, conversation: 0, motion: 0 };
  /** Render passes flushed; the harness asserts a burst cannot storm the paint path. */
  renders = 0;

  /* polling internals */
  private busy = false;
  private ticks = 0;
  private timer: number | null = null;
  private renderQueued = false;
  private readonly renderers: Array<() => void> = [];

  /* live-wire internals */
  private liveRenderAt = 0;
  private liveRenderTimer: number | null = null;
  private listRefreshTimer: number | null = null;
  private tasksRefreshTimer: number | null = null;
  private treasuryRefreshTimer: number | null = null;
  private callsRefreshTimer: number | null = null;
  private boardRefreshTimer: number | null = null;
  private conversationsRefreshTimer: number | null = null;
  private motionsRefreshTimer: number | null = null;
  private threadRefreshTimer: number | null = null;
  private hubRefreshTimer: number | null = null;
  private readonly detailRefreshPending = new Set<string>();
  private readonly vmPushAt = new Map<string, number>();
  /** The newest artifact ask's serial: an older answer never overwrites it. */
  private artifactReq = 0;
  /** The newest drive's serial, per agent: an older answer never overwrites it. */
  private readonly tellReq = new Map<number, number>();
  /** The newest stop's serial, per agent: an older answer never overwrites it. */
  private readonly stopReq = new Map<number, number>();
  /** The newest decision's serial, per motion: an older answer never
      overwrites it (the owner decides a motion once, but a refusal and a fresh
      try must not cross in flight). */
  private readonly decisionReq = new Map<string, number>();

  get health(): Health {
    if (this.lastOkAt === 0) return this.lastErrAt > 0 ? 'down' : 'stale';
    if (this.lastErrAt > this.lastOkAt && Date.now() - this.lastOkAt > LIVE_WINDOW_MS) return 'down';
    return Date.now() - this.lastOkAt <= LIVE_WINDOW_MS ? 'live' : 'stale';
  }

  /* ------------------------------------------------------------ rendering */
  onRender(render: () => void): void {
    this.renderers.push(render);
  }

  scheduleRender(): void {
    if (this.renderQueued) return;
    this.renderQueued = true;
    queueMicrotask(() => {
      this.renderQueued = false;
      this.renders++;
      for (const render of this.renderers) render();
    });
  }

  /** The live lane, for the event stream's serial frames: immediate while
      quiet, at most one paint per LIVE_RENDER_MS under a burst, and always
      a trailing paint so the last frame of a burst is never lost. A guest
      spewing serial can never storm the renderers. */
  private scheduleLiveRender(): void {
    if (this.liveRenderTimer != null) return; // a trailing paint is already queued
    const since = Date.now() - this.liveRenderAt;
    if (since >= LIVE_RENDER_MS) {
      this.liveRenderAt = Date.now();
      this.scheduleRender();
    } else {
      this.liveRenderTimer = setTimeout(() => {
        this.liveRenderTimer = null;
        this.liveRenderAt = Date.now();
        this.scheduleRender();
      }, LIVE_RENDER_MS - since);
    }
  }

  /* -------------------------------------------------------------- polling
     The interval pauses while the tab is hidden — except that showing a view
     always fetches once (ensurePolled), so a view opened in a hidden tab still
     paints. Visibility flips back to visible restart the clock. */
  startPolling(): void {
    if (this.timer != null) return;
    void this.tick();
    this.timer = setInterval(() => {
      if (!document.hidden) void this.tick();
    }, TICK_MS);
    document.addEventListener('visibilitychange', () => {
      if (!document.hidden) void this.tick();
    });
  }

  /** The one fetch every view gets even while hidden. */
  ensurePolled(): void {
    void this.pollFor(this.route, true);
  }

  private async tick(): Promise<void> {
    if (this.busy) return;
    this.busy = true;
    try {
      await this.pollFor(this.route, false);
    } finally {
      this.busy = false;
    }
  }

  private async pollFor(route: Route, first: boolean): Promise<void> {
    const jobs: Array<Promise<void>> = [];
    const wantList = first || this.ticks % 2 === 0 || this.runsError != null || this.runs.length === 0;
    if (wantList) jobs.push(this.pollList());
    const wantTasks = first || this.ticks % TASKS_EVERY === 0 || this.tasksError != null || this.tasks.length === 0;
    if (wantTasks) jobs.push(this.pollTasks());
    const wantTreasury =
      first || this.ticks % TREASURY_EVERY === 0 || this.treasuryError != null || this.treasury == null;
    if (wantTreasury) {
      jobs.push(this.pollTreasury());
      jobs.push(this.pollJournal());
    }
    const wantCalls =
      first || this.ticks % AGORA_EVERY === 0 || this.callsError != null || this.calls.length === 0;
    if (wantCalls) jobs.push(this.pollCalls());
    const wantBoard =
      first || this.ticks % BOARD_EVERY === 0 || this.boardError != null || this.board.length === 0;
    if (wantBoard) jobs.push(this.pollBoard());
    const wantConversations =
      first || this.ticks % CONVERSATIONS_EVERY === 0 || this.conversationsError != null || this.conversations.length === 0;
    if (wantConversations) jobs.push(this.pollConversations());
    const wantMotions =
      first || this.ticks % MOTIONS_EVERY === 0 || this.motionsError != null || this.motions.length === 0;
    if (wantMotions) jobs.push(this.pollMotions());
    if (first || this.ticks % MACHINE_EVERY === 0 || route.name === 'machine') jobs.push(this.pollMachine());
    if (first || this.ticks % HUB_EVERY === 0) jobs.push(this.pollHub());

    if (route.name === 'ledger' && route.param) {
      /* the pinned task's own GET keeps it fresh even when the list poll lags */
      jobs.push(this.pollTaskDetail(route.param));
    }
    if (route.name === 'run' && this.detailId) {
      jobs.push(this.pollDetail(this.detailId));
    } else if (route.name === 'compare' && route.compare) {
      /* both sides of the pair, with serial; quiet — a run that left the
         register shows in its pane, never a redirect */
      jobs.push(this.pollDetail(route.compare.a, true));
      if (route.compare.b !== route.compare.a) jobs.push(this.pollDetail(route.compare.b, true));
    } else if (
      route.name === 'floor' || route.name === 'map' || route.name === 'telemetry' || route.name === 'roster'
    ) {
      /* The floor, the map, telemetry and the roster read serial activity,
         which only the single-run GET carries: poll every open run's detail. */
      for (const run of this.runs) {
        if (run.state === 'starting' || run.state === 'running') jobs.push(this.pollDetail(run.id, true));
      }
    }
    this.ticks++;
    const okBefore = this.lastOkAt;
    await Promise.all(jobs);
    /* one fleet sample per poll that actually observed something — a poll
       that only failed writes nothing, so unobserved time stays un-inked */
    if (this.lastOkAt !== okBefore) this.sampleHistory();
  }

  private noteOk(): void {
    this.lastOkAt = Date.now();
  }

  private noteErr(): void {
    this.lastErrAt = Date.now();
  }

  private async pollList(): Promise<void> {
    try {
      const body = await listRuns();
      const runs = (body.runs ?? []).slice();
      runs.sort((a, b) => seqOf(b.id) - seqOf(a.id));
      /* identity is (id, created_at): a restarted daemon hands the same id
         out again — a changed created_at is a different run wearing it */
      for (const run of runs) {
        const prev = this.runs.find((r) => r.id === run.id);
        if (prev && prev.created_at !== run.created_at) this.forgetRun(run.id);
        const detail = this.details.get(run.id);
        if (detail && detail.created_at !== run.created_at) this.forgetRun(run.id);
      }
      /* forget caches of runs that left the register */
      const alive = new Set(runs.map((r) => r.id));
      for (const id of new Set([...this.runs.map((r) => r.id), ...this.details.keys()])) {
        if (!alive.has(id)) this.forgetRun(id);
      }
      this.runs = runs;
      this.runsError = null;
      this.noteOk();
    } catch (err) {
      this.runsError = err instanceof Error ? err.message : String(err);
      this.noteErr();
    }
    this.pruneTells();
    this.scheduleRender();
  }

  private async pollMachine(): Promise<void> {
    try {
      this.machine = await getMachine();
      this.machineError = null;
      this.noteOk();
    } catch (err) {
      this.machineError = err instanceof Error ? err.message : String(err);
      this.noteErr();
    }
    this.scheduleRender();
  }

  /** Poll the hub's status through the daemon's proxy. The hub is its own
      upstream: an unreachable hub is recorded as `hubError` for the hub
      line, and never feeds the daemon-health mark (the polls own that) — a
      hub down leaves the register exactly as it was. */
  private async pollHub(): Promise<void> {
    try {
      this.hub = await getHub();
      this.hubError = null;
    } catch (err) {
      this.hubError = err instanceof Error ? err.message : String(err);
    }
    this.pruneTells();
    this.scheduleRender();
  }

  /** Poll the task ledger. The list is the whole copy — a task carries every
      fact the console shows, so the ledger's table and its detail pane both
      read the one array. */
  private async pollTasks(): Promise<void> {
    try {
      const tasks = (await listTasks()).slice();
      tasks.sort((a, b) => taskSeq(b.id) - taskSeq(a.id)); // newest first
      this.tasks = tasks;
      this.tasksError = null;
      this.noteOk();
    } catch (err) {
      this.tasksError = err instanceof Error ? err.message : String(err);
      this.noteErr();
    }
    this.scheduleRender();
  }

  /** Poll the treasury's accounts, totals and zero-spend note. */
  private async pollTreasury(): Promise<void> {
    try {
      this.treasury = await getTreasury();
      this.treasuryError = null;
      this.noteOk();
    } catch (err) {
      this.treasuryError = err instanceof Error ? err.message : String(err);
      this.noteErr();
    }
    this.scheduleRender();
  }

  /** Poll the double-entry journal, newest first. The daemon lists it oldest
      first and bounds it to its own newest entries; the copy reads the way the
      register does, and the total says what the bound left behind. */
  private async pollJournal(): Promise<void> {
    try {
      const view = await getTreasuryJournal();
      const entries = view.entries.slice();
      entries.sort((a, b) => b.id - a.id); // newest first
      this.journal = entries;
      this.journalTotal = view.total;
      this.journalTruncated = view.truncated;
      this.journalError = null;
      this.noteOk();
    } catch (err) {
      this.journalError = err instanceof Error ? err.message : String(err);
      this.noteErr();
    }
    this.scheduleRender();
  }

  /** Poll the agora's board. The daemon lists the calls oldest first and bounds
      them to its own newest window; the copy reads newest first, the way the
      register does, and the total says what the bound left behind. */
  private async pollCalls(): Promise<void> {
    try {
      const view = await getAgora();
      const calls = view.calls.slice();
      calls.sort((a, b) => taskSeq(b.task) - taskSeq(a.task)); // newest first
      this.calls = calls;
      this.callsTotal = view.total;
      this.callsTruncated = view.truncated;
      this.callsError = null;
      this.noteOk();
    } catch (err) {
      this.callsError = err instanceof Error ? err.message : String(err);
      this.noteErr();
    }
    this.scheduleRender();
  }

  /** Poll the public board. The daemon lists posts newest first and bounds
      them to its own window; the copy keeps that order and the total says what
      the bound left behind. The unread column rides the same response, so
      "who has read what" moves with the posts. */
  private async pollBoard(): Promise<void> {
    try {
      const view = await getBoard();
      this.board = view.posts.slice();
      this.boardTotal = view.total;
      this.boardLimit = view.limit;
      this.boardTruncated = view.truncated;
      this.boardUnread = view.unread.slice();
      this.boardError = null;
      this.boardErrorStatus = null;
      this.noteOk();
    } catch (err) {
      this.boardError = err instanceof Error ? err.message : String(err);
      this.boardErrorStatus = err && typeof err === 'object' && 'status' in err ? (err.status as number | null) : null;
      this.noteErr();
    }
    this.scheduleRender();
  }

  /** Poll the plaza's conversations. The daemon lists the exchanges newest
      first and hands back its own pair rows with the activity window: the
      floor's proximity derives from exactly those, never from a guess. */
  private async pollConversations(): Promise<void> {
    try {
      const view = await getConversations();
      const exchanges = view.conversations.slice();
      exchanges.sort((a, b) => socialSeq(b.id) - socialSeq(a.id)); // newest first
      this.conversations = exchanges;
      this.convPairs = view.pairs.slice();
      this.convActiveSecs = view.active_secs;
      this.convTotal = view.total;
      this.convLimit = view.limit;
      this.convTruncated = view.truncated;
      this.conversationsError = null;
      this.conversationsErrorStatus = null;
      this.noteOk();
    } catch (err) {
      this.conversationsError = err instanceof Error ? err.message : String(err);
      this.conversationsErrorStatus =
        err && typeof err === 'object' && 'status' in err ? (err.status as number | null) : null;
      this.noteErr();
    }
    this.scheduleRender();
  }

  /** Poll the council's motions. The daemon lists them newest first and bounds
      them to its own window; the copy keeps that order and the total says what
      the bound left behind. The node derives every motion's state and gate, so
      the console reads the whole motion — nothing is patched in from a frame. */
  private async pollMotions(): Promise<void> {
    try {
      const view = await getMotions();
      const motions = view.motions.slice();
      motions.sort((a, b) => motionSeq(b.id) - motionSeq(a.id)); // newest first
      this.motions = motions;
      this.motionsTotal = view.total;
      this.motionsLimit = view.limit;
      this.motionsTruncated = view.truncated;
      this.motionsError = null;
      this.motionsErrorStatus = null;
      this.noteOk();
    } catch (err) {
      this.motionsError = err instanceof Error ? err.message : String(err);
      this.motionsErrorStatus =
        err && typeof err === 'object' && 'status' in err ? (err.status as number | null) : null;
      this.noteErr();
    }
    this.scheduleRender();
  }

  /** Read one agent's own contact list. A read, asked for when its card opens
      (the node emits no frame when a contact is added), so it is never polled
      — and an answer for an agent whose card has closed is still kept: the
      list is the agent's, not the card's. */
  async loadContacts(cid: number): Promise<void> {
    const held = this.contacts.get(cid);
    if (held != null && held.state === 'loading') return;
    this.contacts.set(cid, { cid, state: 'loading', contacts: [], total: 0, truncated: false, error: '' });
    this.scheduleRender();
    try {
      const view = await getContacts(cid);
      this.contacts.set(cid, {
        cid,
        state: 'read',
        contacts: view.contacts.slice(),
        total: view.total,
        truncated: view.truncated,
        error: '',
      });
      this.noteOk();
    } catch (err) {
      this.contacts.set(cid, {
        cid,
        state: 'error',
        contacts: [],
        total: 0,
        truncated: false,
        error: err instanceof Error ? err.message : String(err),
      });
      this.noteErr();
    }
    this.scheduleRender();
  }

  /** Read one pair's thread. Asked for by the owner's own click (the card's
      "read the thread"), so it is never prefetched; the newest ask wins, and
      the node's oldest-first answer is turned round for the card's tail. */
  async loadThread(a: number, b: number): Promise<void> {
    this.thread = { a, b, state: 'loading', exchanges: [], total: 0, truncated: false, error: '' };
    this.scheduleRender();
    try {
      const view = await getConversationThread(a, b);
      const exchanges = view.messages.slice().reverse();
      this.thread = {
        a: view.a,
        b: view.b,
        state: 'read',
        exchanges,
        total: view.total,
        truncated: view.truncated,
        error: '',
      };
      this.noteOk();
    } catch (err) {
      this.thread = {
        a,
        b,
        state: 'error',
        exchanges: [],
        total: 0,
        truncated: false,
        error: err instanceof Error ? err.message : String(err),
      };
      this.noteErr();
    }
    this.scheduleRender();
  }

  /** Narrow the talk sheet to one pair: remember the pair (ascending) and ask
      the node for its own thread, rather than sifting the bounded copy. The
      sheet reads the answer from `thread`. */
  setTalkPair(a: number, b: number): void {
    this.talkPair = { a: Math.min(a, b), b: Math.max(a, b) };
    void this.loadThread(a, b);
  }

  /** Widen the talk sheet back to the whole log. The thread last read is kept
      — it is the node's answer for its pair, not the sheet's filter. */
  clearTalkPair(): void {
    this.talkPair = null;
    this.scheduleRender();
  }

  /** Narrow the council sheet to one state — the sheet's filter, answering the
      same question its tally strip asks. */
  setMotionFilter(state: MotionState): void {
    this.motionFilter = state;
    this.scheduleRender();
  }

  /** Widen the council sheet back to every motion it carries. */
  clearMotionFilter(): void {
    this.motionFilter = null;
    this.scheduleRender();
  }

  /** A board frame landed: a post was written, or an agent's read cursor
      advanced. The frame carries the post (or the cursor it moved to) but not
      the board's totals, its bounding line or the other agents' unread counts,
      so the board is asked once — no row and no count is invented from a
      frame. */
  applyBoardEvent(): void {
    this.wire.board++;
    this.queueBoardRefresh();
  }

  /** A conversation frame landed: an agent spoke. The pair's own activity (and
      therefore the floor's walk) comes from the conversations GET; a thread the
      card is showing for that pair is refreshed with it. */
  applyConversationEvent(from: number, to: number): void {
    this.wire.conversation++;
    this.queueConversationsRefresh();
    const thread = this.thread;
    if (thread != null && thread.state === 'read') {
      const same = (thread.a === from && thread.b === to) || (thread.a === to && thread.b === from);
      if (same) this.queueThreadRefresh(from, to);
    }
  }

  /** One board GET shortly after board frames land; a burst collapses into it. */
  private queueBoardRefresh(): void {
    if (this.boardRefreshTimer != null) return;
    this.boardRefreshTimer = setTimeout(() => {
      this.boardRefreshTimer = null;
      void this.pollBoard();
    }, EVENT_REFRESH_MS);
  }

  /** One conversations GET shortly after an exchange lands. */
  private queueConversationsRefresh(): void {
    if (this.conversationsRefreshTimer != null) return;
    this.conversationsRefreshTimer = setTimeout(() => {
      this.conversationsRefreshTimer = null;
      void this.pollConversations();
    }, EVENT_REFRESH_MS);
  }

  /** One thread GET, for the pair the card is reading. */
  private queueThreadRefresh(a: number, b: number): void {
    if (this.threadRefreshTimer != null) return;
    this.threadRefreshTimer = setTimeout(() => {
      this.threadRefreshTimer = null;
      void this.loadThread(a, b);
    }, EVENT_REFRESH_MS);
  }

  /** Fetch one task by id and merge it into the ledger's copy. A pinned task
      that has left the ledger (a daemon that no longer holds it) unpins the
      sheet and says so, rather than showing a task that is not there. */
  async pollTaskDetail(id: string): Promise<void> {
    try {
      this.mergeTask(await getTask(id));
      this.noteOk();
    } catch (err) {
      const status = err && typeof err === 'object' && 'status' in err ? err.status : null;
      if (status === 404 && this.taskDetailId === id) {
        toast(`Task ${id} is not on the ledger`);
        this.taskDetailId = null;
        if (location.hash !== '#ledger') location.hash = '#ledger';
      }
      this.noteErr();
    }
    this.scheduleRender();
  }

  /** Merge a freshly fetched task into the ledger's copy, newest first. */
  mergeTask(task: Task): void {
    const idx = this.tasks.findIndex((t) => t.id === task.id);
    if (idx >= 0) {
      this.tasks[idx] = task;
      return;
    }
    this.tasks.push(task);
    this.tasks.sort((a, b) => taskSeq(b.id) - taskSeq(a.id));
  }

  /** The store's copy of a task, or null. */
  taskOf(id: string | null | undefined): Task | null {
    return id ? (this.tasks.find((t) => t.id === id) ?? null) : null;
  }

  /** Read one of a task's artifacts through the daemon's read-through. This is
      the owner's hand alone: it drives the task agent's own `read_file` — a
      tool call with a cost and a refusal mode — so it is called from a click
      and never from a poll. Only the newest ask lands; an older answer that
      arrives after a newer click is dropped. */
  async readArtifact(task: string, path: string): Promise<void> {
    const req = ++this.artifactReq;
    this.artifact = { task, path, state: 'loading', content: '', size: 0, truncated: false, cid: 0, error: '', status: null };
    this.scheduleRender();
    try {
      const read = await readTaskArtifact(task, path);
      if (req === this.artifactReq) {
        this.artifact = {
          task,
          path,
          state: 'read',
          content: read.content,
          size: read.size,
          truncated: read.truncated,
          cid: read.agent_cid,
          error: '',
          status: 200,
        };
      }
      this.noteOk();
    } catch (err) {
      /* a refusal is the daemon's, the hub's or the agent's own words — kept
         verbatim; the status tells the three apart (a 503 is a node never
         configured with the hub's admin surface, a 502 a hub out of reach) */
      const status = err && typeof err === 'object' && 'status' in err ? (err.status as number | null) : null;
      const message = err instanceof Error ? err.message : String(err);
      const state = readStateOf(status);
      if (req === this.artifactReq) {
        this.artifact = { task, path, state, content: '', size: 0, truncated: false, cid: 0, error: message, status };
      }
      this.noteErr();
    }
    this.scheduleRender();
  }

  /** Drive one tool call on the agent connected as `cid`, through the daemon's
      `/tell` proxy. This is the owner's hand alone: it runs a real tool in the
      agent's session — a call with a cost and a refusal mode — so it is called
      from a click and never from a poll. The newest drive for an agent wins; an
      older answer that arrives after a newer click is dropped. */
  async driveTool(
    cid: number,
    name: string,
    args: Record<string, unknown>,
    timeoutMs: number,
  ): Promise<void> {
    const req = (this.tellReq.get(cid) ?? 0) + 1;
    this.tellReq.set(cid, req);
    this.recordTell({
      cid,
      name,
      args,
      timeoutMs,
      state: 'driving',
      ok: false,
      output: '',
      error: '',
      status: null,
      at: Date.now(),
    });
    this.scheduleRender();
    try {
      const answer = await tellAgent(cid, name, args, timeoutMs);
      if (this.tellReq.get(cid) !== req) return;
      this.recordTell({
        cid,
        name,
        args,
        timeoutMs,
        state: 'sent',
        ok: answer.ok,
        output: answer.output,
        error: '',
        status: 200,
        at: Date.now(),
      });
      this.noteOk();
    } catch (err) {
      /* a refusal is the daemon's, the hub's or the agent's own words — kept
         verbatim; the status tells them apart (403 the role policy, 409 not
         connected, 504 a timeout, 503 a node never configured, 502 a hub out
         of reach) */
      if (this.tellReq.get(cid) !== req) return;
      const status = err && typeof err === 'object' && 'status' in err ? (err.status as number | null) : null;
      const message = err instanceof Error ? err.message : String(err);
      this.recordTell({
        cid,
        name,
        args,
        timeoutMs,
        state: tellStateOf(status),
        ok: false,
        output: '',
        error: message,
        status,
        at: Date.now(),
      });
      this.noteErr();
    }
    this.scheduleRender();
  }

  /** Record a driven call for one agent: newest first, bounded, and at most one
      call in flight per agent (a fresh drive supersedes a pending one). */
  private recordTell(entry: TellRead): void {
    const kept = (this.tells.get(entry.cid) ?? []).filter((t) => t.state !== 'driving');
    kept.unshift(entry);
    this.tells.set(entry.cid, kept.slice(0, TELL_HISTORY));
    this.lastTell = entry;
  }

  /** End the session of the agent connected as `cid`, through the daemon's
      `/stop` proxy. This is the owner's hand alone: it ends a real session (the
      hub delivers its stop, waits for the agent's `bye`, and answers), so it is
      called from a click and never from a poll. The hub's verdict is kept as it
      came — `stopped: true` with the ended session's call count, `stopped:
      false` with the hub's own reason it stands — and a refusal is whoever
      wrote it, verbatim. The newest stop for an agent wins; an older answer
      that arrives after a newer click is dropped.

      After any answered stop the fleet views are refreshed (coalesced) — the
      hub's connection is exactly what a stop changes, so the roster's hub
      column and the console itself catch up without a manual reload. */
  async stopSession(cid: number, reason?: string): Promise<void> {
    const req = (this.stopReq.get(cid) ?? 0) + 1;
    this.stopReq.set(cid, req);
    this.lastStop = {
      cid,
      reason: reason ?? '',
      state: 'stopping',
      stopped: null,
      calls: null,
      detail: '',
      status: null,
      at: Date.now(),
    };
    this.scheduleRender();
    try {
      const view = await stopAgentApi(cid, reason);
      if (this.stopReq.get(cid) !== req) return;
      this.lastStop = {
        cid: view.cid,
        reason: reason ?? '',
        state: view.stopped ? 'ended' : 'standing',
        stopped: view.stopped,
        calls: view.calls,
        detail: view.reason ?? '',
        status: 200,
        at: Date.now(),
      };
      this.noteOk();
    } catch (err) {
      /* a refusal is the daemon's, the hub's or the admin surface's own words —
         kept verbatim; the status tells them apart (404 a CID the hub does not
         serve, 409 an agent not connected, 502 a hub out of reach, 503 a node
         never configured with the admin surface, 400 a body outside the
         contract) */
      if (this.stopReq.get(cid) !== req) return;
      const status = err && typeof err === 'object' && 'status' in err ? (err.status as number | null) : null;
      const message = err instanceof Error ? err.message : String(err);
      this.lastStop = {
        cid,
        reason: reason ?? '',
        state: stopStateOf(status),
        stopped: null,
        calls: null,
        detail: message,
        status,
        at: Date.now(),
      };
      this.noteErr();
    }
    /* the hub's connection is what a stop acts on, so the fleet is asked once
       (coalesced) whatever the answer — an ended session reads not-connected
       now, and even a refusal about the hub is news about its connection */
    this.queueFleetRefresh();
    this.scheduleRender();
  }

  /** One coalesced register + hub GET shortly after a stop lands, so the
      roster's hub column and connection state catch up without a reload. */
  private queueFleetRefresh(): void {
    if (this.hubRefreshTimer != null) return;
    this.hubRefreshTimer = setTimeout(() => {
      this.hubRefreshTimer = null;
      void this.pollList();
      void this.pollHub();
    }, EVENT_REFRESH_MS);
  }

  /** Decide a motion the count gated to the owner, through the daemon's decide
      endpoint. This is the owner's hand alone: it records the decision on the
      register (the owner decides a motion once), so it is called from a click
      and never from a poll. The node's own answer is the motion, decided, and
      it replaces the row the console holds; the council is then asked once more
      (coalesced) — the same refresh a `motion` frame asks for, so a frame and
      the POST's answer converge on one read rather than racing. A refusal is
      whoever wrote it, verbatim (the two 409s above all: not owner-gated, and
      already decided). The newest decision per motion wins; an older answer
      that arrives after a newer click is dropped. */
  async decideMotion(id: string, approve: boolean, note?: string): Promise<void> {
    const req = (this.decisionReq.get(id) ?? 0) + 1;
    this.decisionReq.set(id, req);
    this.lastDecision = {
      id,
      approve,
      note: note ?? '',
      state: 'deciding',
      detail: '',
      status: null,
      at: Date.now(),
    };
    this.scheduleRender();
    try {
      const motion = await decideMotionApi(id, approve, note);
      if (this.decisionReq.get(id) !== req) return;
      this.lastDecision = {
        id,
        approve,
        note: note ?? '',
        state: decisionStateOf(200),
        detail: '',
        status: 200,
        at: Date.now(),
      };
      /* the node's own answer is the motion with its `owner` set: replace the
         row the console already holds (the id and its place are unchanged),
         and let the coalesced refresh reconcile anything else */
      this.motions = this.motions.map((m) => (m.id === id ? motion : m));
      this.noteOk();
    } catch (err) {
      /* a refusal is the daemon's own words — kept verbatim; the status tells
         them apart (404 no such motion, 409 not owner-gated or already decided,
         400 a body outside the contract) */
      if (this.decisionReq.get(id) !== req) return;
      const status = err && typeof err === 'object' && 'status' in err ? (err.status as number | null) : null;
      const message = err instanceof Error ? err.message : String(err);
      this.lastDecision = {
        id,
        approve,
        note: note ?? '',
        state: decisionStateOf(status),
        detail: message,
        status,
        at: Date.now(),
      };
      this.noteErr();
    }
    this.queueMotionsRefresh();
    this.scheduleRender();
  }

  /** The CIDs the console can still drive: the register's role VMs (when the
      register can be read) union the hub's connected agents (when the hub can).
      `null` when neither source shows anything — a history is then left alone,
      since a source we cannot read must not be read as an absence. */
  private liveAgentCids(): Set<number> | null {
    const known = new Set<number>();
    let readable = false;
    if (this.runsError == null && this.runs.length > 0) {
      readable = true;
      for (const run of this.runs) {
        for (const vm of this.detailOf(run).vms) if (vm.guest_cid != null) known.add(vm.guest_cid);
      }
    }
    if (this.hubError == null && this.hub != null) {
      readable = true;
      for (const agent of this.hub.agents) known.add(agent.cid);
    }
    return readable ? known : null;
  }

  /** Drop the tool console's history for agents that have left the fleet: a
      register role row or a hub connection no longer names their CID. Called
      after every register and hub observation — the console's calls have no
      other owner, so the store reconciles them with the fleet here. */
  pruneTells(): void {
    const live = this.liveAgentCids();
    if (live == null) return;
    for (const cid of [...this.tells.keys()]) {
      if (!live.has(cid)) {
        this.tells.delete(cid);
        this.tellReq.delete(cid);
      }
    }
  }

  /** One fleet observation, derived from what the client already holds: the
      register (live VMs, committed RAM) and the activity tracker (chatter). */
  private sampleHistory(): void {
    let vms = 0;
    let ramMib = 0;
    let lps = 0;
    const runs: Record<string, RunSlice> = {};
    for (const run of this.runs) {
      let live = 0;
      let runLps = 0;
      for (const v of run.vms) {
        if (v.state !== 'running') continue;
        live++;
        /* chatter counts while the VM is alive; an exited VM's EMA would
           otherwise read as fresh throughput forever after the run closes */
        runLps += this.activity.get(vmKey(run.id, v.id));
      }
      runs[run.id] = { vms: live, ramMib: live * run.memory_mib, lps: runLps };
      vms += live;
      ramMib += live * run.memory_mib;
      lps += runLps;
    }
    this.history.push({ at: Date.now(), vms, ramMib, lps, runs });
  }

  /** The run the run-detail view is open on, with serial when fetched. */
  get openRun(): Run | null {
    return this.detailId ? (this.details.get(this.detailId) ?? null) : null;
  }

  /** The freshest copy the store holds of a register entry: its fetched
      detail when it is the same incarnation of the id, else the list entry.
      Only the single-run GET carries serial, so a view that reads lines or
      activity must go through this. */
  detailOf(run: Run): Run {
    const detail = this.details.get(run.id);
    return detail && detail.created_at === run.created_at ? detail : run;
  }

  /** Fetch one run with serial. `quiet` polls (map/floor) skip the 404 redirect. */
  async pollDetail(id: string, quiet = false): Promise<void> {
    try {
      const run = await getRun(id);
      this.mergeDetail(run);
      if (this.detailId === id) this.detailError = null;
      this.noteOk();
    } catch (err) {
      const status = err && typeof err === 'object' && 'status' in err ? err.status : null;
      if (status === 404 && !quiet && this.detailId === id) {
        toast(`Run ${id} is not on the register`);
        this.details.delete(id);
        this.detailError = null;
        this.detailId = null;
        if (location.hash !== '#register') location.hash = '#register';
      } else if (this.detailId === id) {
        this.detailError = err instanceof Error ? err.message : String(err);
      }
      this.noteErr();
    }
    this.scheduleRender();
  }

  /** Merge a freshly fetched run: keep serial a response did not carry
      (DELETE answers without it), feed the activity tracker. */
  mergeDetail(run: Run): void {
    let old = this.details.get(run.id);
    if (old && old.created_at !== run.created_at) {
      /* the daemon began this id again: the old run is not this run */
      this.forgetRun(run.id);
      old = undefined;
    }
    const now = Date.now();
    run.vms.forEach((v, i) => {
      const kept = old?.vms[i]?.serial;
      if (v.serial == null) {
        if (kept != null) v.serial = kept;
      } else if (kept != null && kept.length > v.serial.length && kept.startsWith(v.serial)) {
        /* the event stream already carried bytes past this response's
           snapshot — the capture only grows, so the longer copy is newer */
        v.serial = kept;
      }
      const key = vmKey(run.id, v.id);
      const lps = this.activity.observe(key, linesOf(this.serial, key, v.serial).length, now);
      this.history.pushVm(key, lps);
      this.vmPushAt.set(key, now);
    });
    this.details.set(run.id, run);
  }

  /* ---------------------------------------------------------- the live wire
     The SSE consumer (events.ts) drives these. Every event lands in the
     same caches the polls fill, so every view updates through the one
     render path — and the polls reconcile anything the stream missed (the
     bus has no backlog; a reconnect's lost window heals on the GETs that
     each (re)open triggers). */

  setStreamState(state: StreamState): void {
    if (this.stream === state) return;
    this.stream = state;
    this.scheduleRender();
  }

  /** A run changed state (or was registered, as `starting`). Patch the
      copies the views read; the register's full row (shape, closed time)
      comes from a debounced list GET — a burst of transitions collapses
      into one fetch. A closed run's `ended_at` comes from its detail.
      An event that would wake an ENDED run is never believed: ids restart
      with the daemon, so that is a new run wearing the id — the register
      is asked at once (its answer carries every run's created_at). */
  applyRunEvent(id: string, state: RunState): void {
    this.wire.run++;
    const listed = this.runs.find((r) => r.id === id);
    const detail = this.details.get(id);
    const known = detail ?? listed;
    if (known && !isOpen(known) && (state === 'starting' || state === 'running')) {
      this.queueListRefresh(true);
      return;
    }
    if (listed) listed.state = state;
    if (detail) detail.state = state;
    this.scheduleRender();
    this.queueListRefresh();
    if (state !== 'starting' && state !== 'running') this.queueDetailRefresh(id);
  }

  /** A VM changed state. The mark, the tallies and the stamps all read the
      store's copies — patch both (the register's list row and the detail).
      A `running` event for an ended run is the restarted-id case above. */
  applyVmEvent(runId: string, vmId: number, state: VmState): void {
    this.wire.vm++;
    const listed = this.runs.find((r) => r.id === runId);
    const detail = this.details.get(runId);
    const known = detail ?? listed;
    if (known && !isOpen(known) && state === 'running') {
      this.queueListRefresh(true);
      return;
    }
    for (const run of [listed, detail]) {
      const vm = run?.vms.find((v) => v.id === vmId);
      if (vm) vm.state = state;
    }
    this.scheduleRender();
  }

  /** A VM's serial capture grew. `offset` is the capture byte the text
      starts at; the merge is offset-idempotent — append the new stretch,
      clip a re-delivery, and let a detail GET reconcile a gap (a missed
      frame or poll; rare by construction). The delta feeds the line cache
      and the activity EMA exactly as a poll merge does, so the transcript,
      the lines/s figures, the map's pulse and the floor's halo all go live
      between polls. */
  applySerialEvent(runId: string, vmId: number, offset: number, text: string): void {
    this.wire.serial++;
    const run = this.details.get(runId);
    const vm = run?.vms.find((v) => v.id === vmId);
    if (!vm || vm.serial == null) {
      /* no baseline to offset against — the detail GET carries the capture */
      this.queueDetailRefresh(runId);
      return;
    }
    const key = vmKey(runId, vmId);
    linesOf(this.serial, key, vm.serial); // sync the cache before the offset math
    const have = this.serial.get(key)?.bytes ?? 0;
    const delta = serialDelta(have, offset, text);
    if (delta.kind === 'gap') {
      this.queueDetailRefresh(runId);
      return;
    }
    if (delta.kind === 'dup') return;
    vm.serial += delta.text;
    const now = Date.now();
    const lps = this.activity.observe(key, linesOf(this.serial, key, vm.serial).length, now);
    /* the sparkline ring keeps its one-observation-per-poll timebase:
       between polls it gains at most one point per VM_PUSH_MIN_MS window */
    if (now - (this.vmPushAt.get(key) ?? 0) >= VM_PUSH_MIN_MS) {
      this.vmPushAt.set(key, now);
      this.history.pushVm(key, lps);
    }
    this.scheduleLiveRender();
  }

  /** A task changed state (or was created, as `open`). The event carries only
      the id, the state and the assignee; the ledger's full row (its outcome,
      note, agent CID and updated_at) comes from a debounced list GET, exactly
      as a run's closed time comes from its detail.

      Task ids restart with the daemon, so the register's identity guard
      applies in spirit: an event for a task the console has never seen asks
      the ledger for the truth rather than guessing at a row, and an event that
      would wake an ENDED task is never believed — that is a new task wearing
      the id, so the ledger is asked at once. */
  applyTaskEvent(id: string, state: TaskState, assignee: string | null): void {
    this.wire.task++;
    const task = this.tasks.find((t) => t.id === id);
    if (!task) {
      this.queueTasksRefresh();
      return;
    }
    if (isFinishedTask(task) && (state === 'open' || state === 'claimed')) {
      this.queueTasksRefresh(true);
      return;
    }
    task.state = state;
    task.assignee_id = assignee;
    this.scheduleRender();
    this.queueTasksRefresh();
  }

  /** One ledger GET shortly after task events land; a burst collapses into it.
      An identity question (an ended task supposedly waking) asks NOW. */
  private queueTasksRefresh(immediate = false): void {
    if (immediate) {
      if (this.tasksRefreshTimer != null) {
        clearTimeout(this.tasksRefreshTimer);
        this.tasksRefreshTimer = null;
      }
      void this.pollTasks();
      return;
    }
    if (this.tasksRefreshTimer != null) return;
    this.tasksRefreshTimer = setTimeout(() => {
      this.tasksRefreshTimer = null;
      void this.pollTasks();
    }, EVENT_REFRESH_MS);
  }

  /** A treasury entry was posted: the double-entry journal grew, so the mint
      or an escrow moved. The frame carries only the entry's id, kind, size and
      task; the journal's full row (its postings, memo, poster and stamp) and
      the accounts' new balances come from the GETs, exactly as a task event's
      omitted fields come from the ledger.

      The journal is append-only, so an entry the console already holds is
      complete — nothing more is needed. An id it has never seen (the first
      frame after connect, or one the console fell behind on) asks the journal
      once, coalesced: no row is invented from a frame, and a burst of posts
      collapses into one refresh of both the book and the journal. */
  applyTreasuryEvent(entry: number): void {
    this.wire.treasury++;
    if (this.journal.some((e) => e.id === entry)) return;
    this.queueTreasuryRefresh();
  }

  /** One refresh of the treasury and its journal shortly after posts land; a
      burst collapses into it. */
  private queueTreasuryRefresh(): void {
    if (this.treasuryRefreshTimer != null) return;
    this.treasuryRefreshTimer = setTimeout(() => {
      this.treasuryRefreshTimer = null;
      void this.pollTreasury();
      void this.pollJournal();
    }, EVENT_REFRESH_MS);
  }

  /** A call moved on the agora: an agent bid on a roled open task's call, or
      the call was awarded to a bidder. Unlike a treasury post — whose entry id
      the journal is append-only for, so a row the console holds is complete —
      an `agora` frame carries NO row: not the bids list the bidder's own bid
      joins, not the award's contractor or the escrow entry that funded it. The
      board's GET is the only source of a call's row, so the console asks it
      once, coalesced, and never invents a bid or an award from a frame. An
      event for a call the console has never seen asks the board the same way. */
  applyAgoraEvent(): void {
    this.wire.agora++;
    this.queueCallsRefresh();
  }

  /** One board GET shortly after agora events land; a burst of bids collapses
      into it. */
  private queueCallsRefresh(): void {
    if (this.callsRefreshTimer != null) return;
    this.callsRefreshTimer = setTimeout(() => {
      this.callsRefreshTimer = null;
      void this.pollCalls();
    }, EVENT_REFRESH_MS);
  }

  /** A motion moved on the council: a seat proposed one, or a seat voted on it.
      The frame carries the motion's id, its kind and the state it is now in —
      and NO text at all: a motion's text and its votes' reasons live only on
      the motions endpoint, the one source for them, exactly as the exchange
      record is for a conversation's words. So the console asks the council
      once, coalesced, and patches nothing from a frame: a motion the console
      has never seen (or one that just changed) is read whole from the node.
      Even the frame's own state is not written in — the node's row is the
      authority, and reading it keeps the console from ever disagreeing. */
  applyMotionEvent(): void {
    this.wire.motion++;
    this.queueMotionsRefresh();
  }

  /** One council GET shortly after motion frames land; a burst of votes
      collapses into it. */
  private queueMotionsRefresh(): void {
    if (this.motionsRefreshTimer != null) return;
    this.motionsRefreshTimer = setTimeout(() => {
      this.motionsRefreshTimer = null;
      void this.pollMotions();
    }, EVENT_REFRESH_MS);
  }

  /** One list GET shortly after run events land; a burst collapses into it.
      An identity question (an ended run supposedly waking) asks NOW — the
      register's answer carries every run's created_at. */
  private queueListRefresh(immediate = false): void {
    if (immediate) {
      if (this.listRefreshTimer != null) {
        clearTimeout(this.listRefreshTimer);
        this.listRefreshTimer = null;
      }
      void this.pollList();
      return;
    }
    if (this.listRefreshTimer != null) return;
    this.listRefreshTimer = setTimeout(() => {
      this.listRefreshTimer = null;
      void this.pollList();
    }, EVENT_REFRESH_MS);
  }

  /** Forget everything held for a run. Run ids RESTART with the daemon:
      after a restart the first run is r1 again — a different run wearing
      the same id. Identity is (id, created_at); when it changes, the old
      run's captures, activity and history must not bleed into the new one. */
  private forgetRun(id: string): void {
    this.details.delete(id);
    const prefix = id + '/';
    for (const key of [...this.serial.keys()]) {
      if (key.startsWith(prefix)) {
        this.serial.delete(key);
        this.vmPushAt.delete(key);
      }
    }
    this.activity.dropRun(id);
    this.history.dropRun(id);
  }

  /** A detail GET reconciles what an event could not carry (a serial gap,
      a run the register has not listed yet, a closed run's ended_at).
      One in flight per run. */
  private queueDetailRefresh(id: string): void {
    if (this.detailRefreshPending.has(id)) return;
    this.detailRefreshPending.add(id);
    void this.pollDetail(id, true).finally(() => this.detailRefreshPending.delete(id));
  }

  /** Lines for a VM of the open run, from the append-only cache. */
  linesFor(runId: string, vmId: number, serialRaw: string | undefined): string[] {
    return linesOf(this.serial, vmKey(runId, vmId), serialRaw);
  }
}

export const store = new Store();
