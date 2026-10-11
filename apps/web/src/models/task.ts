/* The task ledger's frozen shapes (crates/noded TaskView / TasksView), read
   through a defensive parse: the relay between the console and the node could
   be hostile, so this is never trusted. Every field is checked and a missing
   or mistyped one is an error that NAMES its path; unknown extra fields are
   tolerated, since the schema may grow columns the console does not yet read.

   The task's own text (`goal`, `acceptance`, `role`, `note`) is prose the
   caller wrote, and it is rendered as text, never markup. */

/** The states noded v0 gives a task (crates/noded tasks::TaskState). */
export type TaskState = 'open' | 'claimed' | 'done' | 'failed' | 'withdrawn';
/** The schema's outcome enum, set when a task ends. */
export type TaskOutcome = 'accepted' | 'failed' | 'cancelled';

export interface Task {
  /** `t<seq>`. */
  id: string;
  /** What the task is for (the schema's `title`). */
  goal: string;
  /** How it will be judged complete (the schema's `spec_ref`), or null. */
  acceptance: string | null;
  /** The task's budget, in whole credits (the schema's `budget_credits`). */
  budget: number;
  /** The role the task was created for, or null. */
  role: string | null;
  state: TaskState;
  /** How the task ended, or null while it lives. */
  outcome: TaskOutcome | null;
  /** The schema's `assignee_id`: the claiming agent's role, or `cid:<n>`. */
  assignee_id: string | null;
  /** The schema's `verifier_id` (always null at v0). */
  verifier_id: string | null;
  /** The claiming agent's guest CID, or null while nobody claimed it. */
  agent_cid: number | null;
  /** What the agent said when it finished, or null. */
  note: string | null;
  /** When the task was created, in milliseconds since the Unix epoch. */
  created_at: number;
  /** When the task last changed, in milliseconds since the Unix epoch. */
  updated_at: number;
}

/** A missing or mistyped required field; the message names its path. */
export class TaskParseError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'TaskParseError';
  }
}

/* Static membership tables: the states and outcomes noded v0 accepts. */
const STATES: Readonly<Record<string, true>> = {
  open: true,
  claimed: true,
  done: true,
  failed: true,
  withdrawn: true,
};
const OUTCOMES: Readonly<Record<string, true>> = { accepted: true, failed: true, cancelled: true };

function obj(v: unknown, path: string): Record<string, unknown> {
  if (v == null || typeof v !== 'object' || Array.isArray(v)) {
    throw new TaskParseError(`${path} is missing or not an object`);
  }
  return v as Record<string, unknown>;
}

function reqStr(o: Record<string, unknown>, key: string, path: string): string {
  const v = o[key];
  if (typeof v !== 'string') throw new TaskParseError(`${path}.${key} is missing or not a string`);
  return v;
}

/** A nullable string: present as `null`, or a string. An absent key is a
    missing field; the daemon always sends the column (serde writes `null`). */
function optStr(o: Record<string, unknown>, key: string, path: string): string | null {
  const v = o[key];
  if (v == null) {
    if (!(key in o)) throw new TaskParseError(`${path}.${key} is missing`);
    return null;
  }
  if (typeof v !== 'string') throw new TaskParseError(`${path}.${key} is not a string or null`);
  return v;
}

function int(o: Record<string, unknown>, key: string, path: string): number {
  const v = o[key];
  if (typeof v !== 'number' || !Number.isFinite(v)) {
    throw new TaskParseError(`${path}.${key} is missing or not a number`);
  }
  if (!Number.isInteger(v)) throw new TaskParseError(`${path}.${key} is not an integer`);
  return v;
}

function optInt(o: Record<string, unknown>, key: string, path: string): number | null {
  const v = o[key];
  if (v == null) {
    if (!(key in o)) throw new TaskParseError(`${path}.${key} is missing`);
    return null;
  }
  if (typeof v !== 'number' || !Number.isInteger(v)) {
    throw new TaskParseError(`${path}.${key} is not an integer or null`);
  }
  return v;
}

function stateOf(o: Record<string, unknown>, key: string, path: string): TaskState {
  const v = o[key];
  if (typeof v !== 'string' || STATES[v] !== true) {
    throw new TaskParseError(`${path}.${key} is missing or not a task state`);
  }
  return v as TaskState;
}

function outcomeOf(o: Record<string, unknown>, key: string, path: string): TaskOutcome | null {
  const v = o[key];
  if (v == null) {
    if (!(key in o)) throw new TaskParseError(`${path}.${key} is missing`);
    return null;
  }
  if (typeof v !== 'string' || OUTCOMES[v] !== true) {
    throw new TaskParseError(`${path}.${key} is not null or a task outcome`);
  }
  return v as TaskOutcome;
}

function readTask(t: Record<string, unknown>, path: string): Task {
  return {
    id: reqStr(t, 'id', path),
    goal: reqStr(t, 'goal', path),
    acceptance: optStr(t, 'acceptance', path),
    budget: int(t, 'budget', path),
    role: optStr(t, 'role', path),
    state: stateOf(t, 'state', path),
    outcome: outcomeOf(t, 'outcome', path),
    assignee_id: optStr(t, 'assignee_id', path),
    verifier_id: optStr(t, 'verifier_id', path),
    agent_cid: optInt(t, 'agent_cid', path),
    note: optStr(t, 'note', path),
    created_at: int(t, 'created_at', path),
    updated_at: int(t, 'updated_at', path),
  };
}

/** Parse one task as the API reports it; unknown extras are tolerated. */
export function parseTask(raw: unknown): Task {
  return readTask(obj(raw, 'task'), 'task');
}

/** Parse the list endpoint's body (`{"tasks": [...]}`), oldest first as the
    API returns it. A bad row is an error naming its index. */
export function parseTasksView(raw: unknown): Task[] {
  const root = obj(raw, 'ledger');
  const list = root['tasks'];
  if (!Array.isArray(list)) throw new TaskParseError('ledger.tasks is missing or not an array');
  return list.map((t, i) => readTask(obj(t, `ledger.tasks[${i}]`), `ledger.tasks[${i}]`));
}

/** The numeric folio of a task id (`t12` -> 12); 0 when it does not parse. */
export function seqOf(id: string | null | undefined): number {
  const m = /^t(\d+)$/.exec(id ?? '');
  return m && m[1] ? Number(m[1]) : 0;
}

/** A task that has not ended: it can still be claimed, finished or withdrawn. */
export function isOpenTask(task: Task | null | undefined): boolean {
  return !!task && (task.state === 'open' || task.state === 'claimed');
}

/** A task that has ended: `done`, `failed` or `withdrawn`, and is final. */
export function isFinishedTask(task: Task | null | undefined): boolean {
  return !!task && !isOpenTask(task);
}

/* The done note's artifact line. A task whose workflow wrote files is finished
   with `artifacts: <p1>, <p2> (N tool call(s))` — the paths its workspace
   holds, joined with ", ", the count of tool calls in parentheses. A read-only
   run carries its own one-line summary instead (`read-only: 3 match(es) (1
   tool call(s))`), and a note written by hand names none; both read as no
   artifacts. The parse is pure and total: a note this console does not
   recognise yields nothing, never a guessed path. */
const NOTE_ARTIFACTS = /^artifacts:[ \t]*(.*?)(?:[ \t]*\(\d+ tool call\(s\)\))?[ \t]*$/;

/** The artifact paths a finish note names, in the order the note lists them;
    empty when it names none — a summary note, a plain note, a null note, or an
    `artifacts:` line with no paths. */
export function artifactPaths(note: string | null | undefined): string[] {
  if (note == null) return [];
  const m = NOTE_ARTIFACTS.exec(note.trim());
  if (m == null) return [];
  return (m[1] ?? '')
    .split(',')
    .map((p) => p.trim())
    .filter((p) => p !== '');
}

/** How the ledger names a task's state in its own marginalia. */
export const STATE_LABEL: Readonly<Record<TaskState, string>> = {
  open: 'open · unclaimed',
  claimed: 'claimed',
  done: 'done · accepted',
  failed: 'failed',
  withdrawn: 'withdrawn · cancelled',
};
