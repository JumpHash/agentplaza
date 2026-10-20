// The Lab's breeders (`#/lab/breeders`): the agents with the breeder role, each with the GPU-hours
// its jobs have used and the per-job cap S-006c sets, its jobs (linked to their
// detail pages) and its recent Foundry tool calls. The calls come from the agent's own transcript,
// read as the agent's page reads it, and kept to the foundry_* tools and invoke_model. A job belongs
// to a breeder when its `agent` is the breeder's CID (the hub's job summary carries it).

import { api, listJobs, type Agent, type Entry, type JobSummary } from '../api';
import { href } from '../router';
import { badge, card, clip, table as dataTable, type Column, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { agoText, errorText, fmtGpuHours, fmtNumber, labEmpty, labRequest, pollEvery } from './lab-common';

const POLL_MS = 5_000;
/** The per-job GPU-hour cap when the laws cannot be read: S-006c permits a breeder's job up to 8 GPU-hours (S-006d refuses more). */
const PER_JOB_CAP_FALLBACK = 8;
/** S-006c's cap, from its condition `context.gpu_hours <= N`. */
const CAP_PATTERN = /context\.gpu_hours\s*<=\s*(\d+(?:\.\d+)?)/;
/** The transcript page size the agent page reads with. */
const PAGE = 1000;
/** The newest tool calls shown per breeder, and the most a breeder keeps in memory. */
const RECENT = 12;
const KEPT = 200;
/** The job id in a submit call's output: the job view's `id`, or the refusal's "job <id> did not dispatch". */
const JOB_ID = /"id":"([^"]+)"/;

/** A job as the list gives it; `agent` is the submitting CID, when the hub says it. */
type JobRow = JobSummary & { agent?: number | null };

/** One Foundry tool call of a breeder: its request, what the decision or result said, and whether it was refused. */
interface ToolCall { call: string; at_ms: number; name: string; detail: string; result: string; refused: boolean }

/** A breeder's read position in its transcript, and what that read found. */
interface Cursor { after: number; names: Map<string, string>; calls: ToolCall[] }

interface Breeder { agent: Agent; jobs: JobRow[]; calls: ToolCall[]; used: number; cap: number; note: string }

/** S-006c's per-job cap, read from the laws; null when it cannot be read. */
async function readCap(): Promise<number | null> {
  try {
    const laws = await labRequest<{ id: string; conditions?: string[] }[]>('GET', '/v1/laws');
    const law = laws.find((l) => l.id === 'S-006c');
    const m = law ? CAP_PATTERN.exec((law.conditions ?? []).join(' ')) : null;
    return m ? Number(m[1]) : null;
  } catch {
    return null;
  }
}

const JOB_TONE: Record<string, Tone> = { queued: 'muted', running: 'info', done: 'ok', failed: 'danger', cancelled: 'warn' };
const STATUS_TONE: Record<string, Tone> = { working: 'info', idle: 'muted', offline: 'muted' };

const isFoundry = (name: string): boolean => name.startsWith('foundry_') || name === 'invoke_model';

/** The one input a call names: its model, its job's recipe, its job, or its name. */
function detailOf(input: unknown): string {
  const i = (input ?? {}) as Record<string, unknown>;
  const job = i['job'];
  const recipe = job !== null && typeof job === 'object' ? (job as Record<string, unknown>)['recipe'] : undefined;
  const picked = i['model'] ?? recipe ?? (typeof job === 'string' ? job : undefined) ?? i['name'] ?? i['kind'];
  return typeof picked === 'string' ? picked : '';
}

/** Folds one transcript entry into a breeder's cursor: the Foundry requests, then their decisions and results. */
function absorb(cur: Cursor, e: Entry): void {
  cur.after = Math.max(cur.after, e.seq);
  const d = e.data;
  const call = String(d['call'] ?? '');
  if (e.kind === 'tool_request') {
    const name = String(d['name'] ?? '');
    if (!isFoundry(name)) return;
    cur.names.set(call, name);
    cur.calls.push({ call, at_ms: e.at_ms, name, detail: detailOf(d['input']), result: '', refused: false });
    return;
  }
  const name = cur.names.get(call);
  if (name === undefined) return;
  const row = cur.calls.find((c) => c.call === call);
  if (!row) return;
  if (e.kind === 'tool_decision') {
    const output = String(d['output'] ?? '');
    row.refused = String(d['decision'] ?? '') === 'deny';
    if (output) row.result = output;
    if (name === 'foundry_submit_job') {
      const m = JOB_ID.exec(output);
      const id = m?.[1];
      if (id) row.result = `job ${id} ${/"status":"(\w+)"/.exec(output)?.[1] ?? 'submitted'}`;
    }
  } else if (e.kind === 'tool_result') {
    const output = String(d['output'] ?? '');
    if (output && !row.result) row.result = output;
    if (d['ok'] === false) row.refused = true;
  }
}

/** Reads the transcript from the cursor on, a page at a time, until the newest entry. */
async function readTranscript(cid: number, cur: Cursor): Promise<void> {
  for (;;) {
    const page = await api.transcript(cid, cur.after, PAGE);
    for (const e of page.slice().sort((a, b) => a.seq - b.seq)) absorb(cur, e);
    if (page.length < PAGE) break;
  }
  if (cur.calls.length > KEPT) cur.calls.splice(0, cur.calls.length - KEPT);
}

export function render(root: HTMLElement, _params: string[]): Cleanup {
  /** Each breeder's cursor, kept across polls so a poll reads only what is new. */
  const cursors = new Map<number, Cursor>();
  let breeders: Breeder[] | null = null;
  let failure = '';
  /** S-006c's cap, read once per visit (null until it is read). */
  let cap: number | null = null;

  const draw = (): void => {
    if (failure && breeders === null) {
      replace(root, h('p.lab-err', { role: 'alert' }, failure));
      return;
    }
    if (breeders === null) {
      replace(root, h('p.lab-note', null, 'Loading breeders'));
      return;
    }
    if (breeders.length === 0) {
      replace(root, labEmpty('bot', 'No breeders yet.', 'A breeder is an agent with the breeder role; create one from Agents.'));
      return;
    }
    const jobs = breeders.reduce((n, b) => n + b.jobs.length, 0);
    const used = breeders.reduce((n, b) => n + b.used, 0);
    const summary = h('span.lab2-note', null, `${fmtNumber(breeders.length)} breeders · ${fmtNumber(jobs)} jobs · ${fmtGpuHours(used)} used`);
    replace(
      root,
      h('div.lab2-form', null, h('div.lab2-row', { style: 'justify-content:flex-start' }, summary), failure ? h('p.lab-err', { role: 'alert' }, failure) : null, h('div.lab2-grid', null, breeders.map(breederCard)), jobsCard(breeders), callsCard(breeders)),
    );
  };

  const load = async (): Promise<void> => {
    try {
      if (cap === null) cap = await readCap();
      const [agents, jobs] = await Promise.all([api.agents(), listJobs() as Promise<JobRow[]>]);
      const mine = agents.filter((a) => a.role === 'breeder').sort((a, b) => a.cid - b.cid);
      breeders = await Promise.all(mine.map((agent) => breederOf(agent, jobs)));
      failure = '';
    } catch (err) {
      failure = errorText(err);
    }
    draw();
  };

  /** A breeder's row: its jobs and its calls. A transcript that cannot be read is noted, not fatal. */
  const breederOf = async (agent: Agent, jobs: JobRow[]): Promise<Breeder> => {
    const cur = cursors.get(agent.cid) ?? { after: 0, names: new Map<string, string>(), calls: [] };
    cursors.set(agent.cid, cur);
    let note = '';
    try {
      await readTranscript(agent.cid, cur);
    } catch (err) {
      note = `Transcript unreachable: ${errorText(err)}`;
    }
    // A breeder's jobs are the jobs the hub says were submitted with its CID.
    const own = jobs.filter((j) => j.agent === agent.cid);
    return {
      agent,
      jobs: own,
      calls: cur.calls.slice(-RECENT).reverse(),
      used: own.reduce((n, j) => n + j.gpu_hours, 0),
      cap: cap ?? PER_JOB_CAP_FALLBACK,
      note,
    };
  };

  const stop = pollEvery(POLL_MS, () => void load());
  return () => stop();
}

/** One breeder's card: its name and status, its role and CID, then the GPU-hours used, the jobs and the per-job cap. */
function breederCard(b: Breeder): HTMLElement {
  const a = b.agent;
  const name = a.name.trim() === '' ? `agent ${a.cid}` : a.name;
  const body = h(
    'div',
    null,
    h(
      'div.lab2-row',
      { style: 'justify-content:flex-start' },
      badge(a.status, STATUS_TONE[a.status] ?? 'muted'),
      h('span.lab2-note', null, `breeder · CID ${a.cid}`),
    ),
    h(
      'dl.kv',
      null,
      h('div', null, h('dt', null, 'GPU-h used'), h('dd', null, h('span.num', null, fmtGpuHours(b.used)))),
      h('div', null, h('dt', null, 'Jobs'), h('dd', null, fmtNumber(b.jobs.length))),
      h('div', null, h('dt', null, 'Per-job cap'), h('dd', null, `${fmtNumber(b.cap)} GPU-h`)),
    ),
    b.note ? h('p.lab-err', null, b.note) : null,
  );
  return card(name, body, { actions: [h('a.btn.ghost.sm', { href: href('agents', a.cid) }, h('span', null, 'Agent page'))] });
}

/** Every breeder's jobs in one table: each job links to its page. */
function jobsCard(breeders: Breeder[]): HTMLElement {
  const rows = breeders.flatMap((b) => b.jobs);
  const cols: Column<JobRow>[] = [
    { label: 'Job', cell: (j) => h('a.mono', { href: href('lab', 'jobs', j.id) }, j.id) },
    { label: 'Status', cell: (j) => badge(j.status, JOB_TONE[j.status] ?? 'muted') },
    { label: 'Recipe', cell: (j) => h('span.mono', null, j.recipe) },
    { label: 'GPU-h', num: true, cell: (j) => j.gpu_hours.toFixed(2) },
    { label: 'Created', cell: (j) => agoText(j.created_ms) },
  ];
  return card('Jobs', rows.length ? h('div.tbl-wrap', null, dataTable(cols, rows)) : h('p.lab-note', null, 'No jobs yet.'));
}

/** Each breeder's recent Foundry calls, newest first, read from its transcript. */
function callsCard(breeders: Breeder[]): HTMLElement {
  const cols: Column<ToolCall>[] = [
    { label: 'Time', cell: (c) => agoText(c.at_ms) },
    { label: 'Tool', cell: (c) => h('span.mono', null, c.name) },
    { label: 'Input', cell: (c) => h('span.mono', null, c.detail ? clip(c.detail, 80) : '—') },
    { label: 'Result', cell: (c) => (c.refused ? h('span.lab2-over', null, `Refused${c.result ? `: ${clip(c.result.replace(/\s+/g, ' '), 140)}` : ''}`) : c.result ? clip(c.result.replace(/\s+/g, ' '), 140) : '—') },
  ];
  const parts = breeders.map((b) => {
    if (b.note && b.calls.length === 0) return h('p.lab2-note', null, b.note);
    return b.calls.length ? h('div.tbl-wrap', null, dataTable(cols, b.calls)) : h('p.lab-note', null, 'No Foundry calls yet.');
  });
  return card('Recent Foundry calls', parts.length ? parts : h('p.lab-note', null, 'No Foundry calls yet.'));
}
