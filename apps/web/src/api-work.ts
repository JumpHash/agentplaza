// The work engine's API: sessions (a fresh context working one seat's step, a project's planner, or the main
// orchestrator), a task's seats, the budgets of a project and of a task with their ledgers, the cards waiting for the
// owner, and the chats. Same origin and the same `call` as the rest of the console. The routes are listed in
// crates/apid/src/work_routes.rs; the shapes are the structs those handlers serialize (sessions.rs, seats.rs,
// budgets.rs, cards.rs, chats.rs, transcript.rs, context_track.rs). Amounts: micro-dollars for a `usd` budget, tokens
// for a `tokens` one.

import { call, readableAt } from './api';

const seg = (text: string): string => encodeURIComponent(text);

/** `?a=1&b=x`: the defined parameters only, `true` as `true`; `''` when there are none. */
const qs = (params: Record<string, string | number | boolean | null | undefined>): string => {
  const parts = Object.entries(params)
    .filter(([, v]) => v !== undefined && v !== null && v !== false && v !== '')
    .map(([k, v]) => `${k}=${encodeURIComponent(String(v))}`);
  return parts.length ? `?${parts.join('&')}` : '';
};

// ---- Sessions -----------------------------------------------------------------------------------------------------

/** What a session is for: one step of a task, a project's planner or the main orchestrator (long), or a helper. */
export type SessionKind = 'step' | 'long' | 'helper';
/** Where a session is in its life. `ended` and `failed` are final. */
export type SessionState =
  | 'queued'
  | 'placing'
  | 'forking'
  | 'running'
  | 'paused'
  | 'compacting'
  | 'handing_over'
  | 'ending'
  | 'ended'
  | 'failed';
/** Why a session is paused: its budget, its allowance, the disk, a card waiting for the owner, or its node. */
export type PausedBy = 'budget' | 'allowance' | 'disk' | 'card' | 'node';

/** What a session has used. Costs are micro-dollars. */
export interface SessionUsage {
  tokens_in: number;
  tokens_out: number;
  tokens_cached: number;
  cost_micros: number;
  model_calls: number;
  tool_calls: number;
}

/** One session. A session has no name: its seat, task and project say what it is for. */
export interface Session {
  /** `s<seq>`. */
  id: string;
  /** The project it works for (`plaza` for the main orchestrator). */
  project: string;
  /** The task it works on; none for a long-lived session. */
  task: string | null;
  /** The seat it fills (`builder`, `reviewer:1`, …). */
  seat: string | null;
  kind: SessionKind;
  /** For a helper, the session that started it. */
  parent: string | null;
  /** The seat template (role) it runs as. */
  role: string;
  /** `harness` (the hub's own loop) or a vendor engine's id. */
  engine: string;
  model: string | null;
  state: SessionState;
  paused_by: PausedBy | null;
  /** Its fork: the node it runs on, its VM's CID and noded's run. */
  node: string | null;
  cid: number | null;
  run: string | null;
  /** The work item in flight on its CID's lane. */
  work: number | null;
  usage: SessionUsage;
  context_bytes: number;
  compactions: number;
  /** What it handed over to the next session of its seat when it ended. */
  handover: string | null;
  /** Why it ended or failed, in a sentence. */
  outcome: string | null;
  /** Which try of its seat this is (1, 2, …). */
  attempt: number;
  /** A reviewer's verdict, once given (true: passed). */
  verdict: boolean | null;
  created_ms: number;
  started_ms: number | null;
  ended_ms: number | null;
}

export interface SessionFilter {
  project?: string;
  task?: string;
  state?: SessionState;
  live?: boolean;
}

/** One entry of a session's transcript: `kind` names what happened, `data` holds its fields as the hub wrote them. */
export interface TranscriptEntry {
  cid: number;
  seq: number;
  session?: string;
  /** The work item it ran under (0 for none). */
  work: number;
  at_ms: number;
  kind: string;
  data: Record<string, unknown>;
}

export interface ContextPart {
  part: string;
  tokens: number;
}

export interface ContextTool {
  tool: string;
  calls: number;
  errors: number;
  result_tokens: number;
}

/**
 * A session's context: what its last model call sent (`used`) against its model's `window`, by part and by tool. A
 * session that has made no call yet has no cid and no parts.
 */
export interface SessionContext {
  session: string;
  cid?: number | null;
  work?: number | null;
  model?: string | null;
  window?: number | null;
  used: number;
  output?: number;
  /** `used` over `window`, at most 1; null without a window. */
  fill?: number | null;
  parts: ContextPart[];
  tools?: ContextTool[];
  calls?: number;
  compactions?: number;
  updated_ms?: number;
}

// ---- Seats --------------------------------------------------------------------------------------------------------

/** Where a seat is: waiting for an earlier step, ready for a session, filled by one, done, or failed for good. */
export type SeatState = 'waiting' | 'ready' | 'filled' | 'done' | 'failed';
/** Which models may fill a seat: any the project allows, or one no other seat of the task used. */
export type ModelPolicy = 'any' | 'differ';

/** One seat of a task: a role in its recipe, filled by one session at a time. */
export interface Seat {
  task: string;
  /** The seat's name in its task (`builder`, `reviewer`, …). */
  name: string;
  recipe: string;
  step: string;
  role: string;
  model_policy: ModelPolicy;
  state: SeatState;
  /** The session filling it now. */
  session: string | null;
  /** The model of the last session that filled it. */
  last_model: string | null;
  /** Sessions started for it in this round. */
  attempts: number;
  /** The recipe's loop round it is in (1 at the start). */
  round: number;
  /** What the step that sent the task back said (the findings). */
  returned_with: string | null;
  updated_ms: number;
}

// ---- Budgets ------------------------------------------------------------------------------------------------------

export type BudgetUnit = 'usd' | 'tokens';
export type OwnerKind = 'project' | 'task';

/**
 * A budget as the hub reads it: its cap, what is spent, held and reserved, and the rest the hub adds (`left`, the share
 * spent). Before a budget is set the hub answers only `{owner, unit: null, cap: null, spent: 0}`, so every other field
 * may be missing.
 */
export interface Budget {
  owner: string;
  owner_kind?: OwnerKind;
  /** For a task, its project; for a project, itself. */
  project?: string;
  unit: BudgetUnit | null;
  /** In the unit (micro-dollars or tokens); null: no cap. */
  cap: number | null;
  /** The share of the cap at which the owner is told (80 by default). */
  warn_at_pct?: number;
  /** A project's cap per month for its long-lived sessions; null: none. */
  monthly_cap?: number | null;
  /** The month (`yyyymm`) `month_spent` counts. */
  month?: number;
  month_spent?: number;
  /** A project: what its running tasks may still spend. A task: what it reserved from its project (in the project's unit). */
  reserved?: number;
  /** A task: how much of its reservation it has drawn. */
  drawn?: number;
  spent: number;
  /** The worst case of the calls in flight. */
  held?: number;
  /** The owner paused it, or it reached its cap and waits for a raise. */
  paused?: boolean;
  warned?: boolean;
  updated_ms?: number;
  /** What is left to spend (cap less spent, held and reserved); null when uncapped. */
  left?: number | null;
  /** The share of the cap spent, in percent; null when uncapped. */
  spent_pct?: number | null;
}

/** A project's budget, with its tasks' budgets. */
export interface ProjectBudget extends Budget {
  tasks?: Budget[];
}

/** The body of a budget write. A task's takes only `unit` and `cap`. */
export interface BudgetBody {
  unit: BudgetUnit;
  cap?: number | null;
  warn_at_pct?: number;
  monthly_cap?: number | null;
}

/** What a ledger entry records: a budget set, a reservation, a call's hold, a charge, a release, or a raise. */
export type LedgerKind = 'set' | 'reserve' | 'hold' | 'charge' | 'release' | 'raise';

/** One ledger entry, newest first. `amount` is in the owner's unit; `micros` and `tokens` carry both. */
export interface LedgerEntry {
  owner_kind: OwnerKind;
  owner: string;
  kind: LedgerKind;
  amount: number;
  micros: number;
  tokens: number;
  session: string | null;
  model: string | null;
  at_ms: number;
}

// ---- Cards --------------------------------------------------------------------------------------------------------

/** What a card asks: a start, a budget, an allowance, a disk, a connector, a tool, a seat, a release, a tool call, … */
export type CardKind =
  | 'start'
  | 'budget'
  | 'allowance'
  | 'disk'
  | 'connector'
  | 'tool'
  | 'engine'
  | 'recipe'
  | 'promote'
  | 'memory'
  | 'review'
  | 'land'
  | 'adopt'
  | 'yank'
  | 'seat'
  | 'release'
  | 'deploy'
  | 'tool_call'
  | 'incident';

/** Where a card is. */
export type CardState = 'open' | 'answered' | 'withdrawn';

/** One button of a card. `takes_amount`: the owner types an amount to go with it. */
export interface CardChoice {
  id: string;
  label: string;
  detail: string | null;
  takes_amount: boolean;
}

/** A decision waiting for the owner, shown in the chat it belongs to. */
export interface Card {
  /** `c<seq>`. */
  id: string;
  kind: CardKind;
  /** The chat it is shown in. */
  chat: string | null;
  project: string | null;
  task: string | null;
  session: string | null;
  /** What it is about, for grouping cards of one subject. */
  subject: string;
  title: string;
  body: string | null;
  choices: CardChoice[];
  state: CardState;
  /** The choice the owner made. */
  answer: string | null;
  /** The amount given with the answer (micro-dollars or tokens, as the card says). */
  amount: number | null;
  created_ms: number;
  answered_ms: number | null;
}

/** An answer's reply: the card, and what the answer decided done (empty when nothing else follows). */
export interface CardAnswer {
  card: Card;
  done: string;
}

// ---- Chats --------------------------------------------------------------------------------------------------------

/** Whose chat it is: the main orchestrator's, a project's planner's, or a task's. */
export type ChatKind = 'main' | 'project' | 'task';
/** Who wrote a message: the owner, a session, or the hub. */
export type MessageAuthor = 'owner' | 'session' | 'system';
/** What a message is: talk, a card waiting for the owner (its buttons are the card's choices), or a notice. */
export type MessageKind = 'message' | 'question' | 'notice';

export interface ChatMessage {
  seq: number;
  author: MessageAuthor;
  session: string | null;
  /** The seat the writing session filled. */
  seat: string | null;
  kind: MessageKind;
  text: string;
  /** The card of a question. */
  card: string | null;
  at_ms: number;
}

/** A chat. The lists leave `messages` empty; a read has its last messages. */
export interface Chat {
  /** `main` for the orchestrator's chat. */
  id: string;
  kind: ChatKind;
  project: string | null;
  task: string | null;
  title: string;
  archived: boolean;
  next_seq: number;
  created_ms: number;
  updated_ms: number;
  messages: ChatMessage[];
}

/** The id of the main orchestrator's chat. */
export const MAIN_CHAT = 'main';

/** Micro-dollars in a dollar: a `usd` amount on the wire is dollars times this. */
export const MICROS_PER_DOLLAR = 1_000_000;

// ---- The calls ----------------------------------------------------------------------------------------------------

/** The work engine's reads (cached like every GET, see api.ts) and writes. */
export const workApi = {
  sessions: readableAt<[filter?: SessionFilter], Session[]>((filter = {}) =>
    `/v1/sessions${qs({ project: filter.project, task: filter.task, state: filter.state, live: filter.live })}`,
  ),
  session: readableAt<[id: string], Session>((id) => `/v1/sessions/${seg(id)}`),
  transcript: (id: string, after = 0, limit = 200) => call<TranscriptEntry[]>('GET', `/v1/sessions/${seg(id)}/transcript${qs({ after, limit })}`),
  context: readableAt<[id: string, work?: number], SessionContext>((id, work) => `/v1/sessions/${seg(id)}/context${qs({ work })}`),
  stopSession: (id: string) => call<Session>('POST', `/v1/sessions/${seg(id)}/stop`, {}),

  seats: readableAt<[task: string], Seat[]>((task) => `/v1/tasks/${seg(task)}/seats`),

  projectBudget: readableAt<[id: string], ProjectBudget>((id) => `/v1/projects/${seg(id)}/budget`),
  setProjectBudget: (id: string, body: BudgetBody) => call<ProjectBudget>('PUT', `/v1/projects/${seg(id)}/budget`, body),
  projectLedger: readableAt<[id: string, limit?: number], LedgerEntry[]>((id, limit) => `/v1/projects/${seg(id)}/budget/ledger${qs({ limit })}`),
  taskBudget: readableAt<[id: string], Budget>((id) => `/v1/tasks/${seg(id)}/budget`),
  setTaskBudget: (id: string, body: BudgetBody) => call<Budget>('PUT', `/v1/tasks/${seg(id)}/budget`, body),
  taskLedger: readableAt<[id: string, limit?: number], LedgerEntry[]>((id, limit) => `/v1/tasks/${seg(id)}/budget/ledger${qs({ limit })}`),

  cards: readableAt<[open?: boolean, chat?: string], Card[]>((open, chat) => `/v1/cards${qs({ open, chat })}`),
  card: readableAt<[id: string], Card>((id) => `/v1/cards/${seg(id)}`),
  answerCard: (id: string, choice: string, amount?: number) =>
    call<CardAnswer>('POST', `/v1/cards/${seg(id)}`, amount === undefined ? { choice } : { choice, amount }),

  chats: readableAt<[project?: string, archived?: boolean], Chat[]>((project, archived) => `/v1/chats${qs({ project, archived })}`),
  chat: readableAt<[id: string, limit?: number], Chat>((id, limit) => `/v1/chats/${seg(id)}${qs({ limit })}`),
  messages: (id: string, after = 0, limit = 200) => call<ChatMessage[]>('GET', `/v1/chats/${seg(id)}/messages${qs({ after, limit })}`),
  openChat: (project: string, title?: string) => call<Chat>('POST', '/v1/chats', title ? { project, title } : { project }),
  say: (id: string, text: string) => call<ChatMessage>('POST', `/v1/chats/${seg(id)}/messages`, { text }),
  editChat: (id: string, body: { title?: string; archived?: boolean }) => call<Chat>('PATCH', `/v1/chats/${seg(id)}`, body),
};
