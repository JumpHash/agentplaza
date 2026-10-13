// The Lab's breeders (`#/lab/breeders`): the agents with the breeder role, each with the GPU-hours
// its jobs have used and the per-job cap S-006c sets, its jobs (linked to their
// detail pages) and its recent Foundry tool calls. The calls come from the agent's own transcript,
// read as the agent's page reads it, and kept to the foundry_* tools and invoke_model. A job belongs
// to a breeder when its `agent` is the breeder's CID (the hub's job summary carries it).

import '../styles/lab-breeders.css';
import { api, listJobs, type Agent, type Entry, type JobSummary } from '../api';
import { href } from '../router';
import { clip } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { errorText, fmtGpuHours, fmtNumber, fmtTime, labEmpty, labRequest, pollEvery } from './lab-common';
import { dataTable } from './lab-models';

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

const JOB_TONE: Record<string, string> = { queued: 'muted', running: 'info', done: 'ok', failed: 'danger', cancelled: 'warn' };
const STATUS_TONE: Record<string, string> = { working: 'accent', idle: 'ok', offline: 'muted' };

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
      replace(root, h('p.lab-error', { role: 'alert' }, failure));
      return;
    }
    if (breeders === null) {
      replace(root, h('p.lab-muted', null, 'Loading breeders'));
      return;
    }
    if (breeders.length === 0) {
      replace(root, labEmpty('wrenchcog', 'No breeders yet.', 'A breeder is an agent with the breeder role; create one from Agents.'));
      return;
    }
    const jobs = breeders.reduce((n, b) => n + b.jobs.length, 0);
    const used = breeders.reduce((n, b) => n + b.used, 0);
    const summary = h('p.lab-muted', null, `${fmtNumber(breeders.length)} breeders · ${fmtNumber(jobs)} jobs · ${fmtGpuHours(used)} used`);
    replace(
      root,
      h('div.lab-page-head', null, h('h2', null, 'Breeders'), h('div.lab-actions', null, summary)),
      failure ? h('p.lab-error', { role: 'alert' }, failure) : null,
      h('div.lab-stack', null, breeders.map(breederCard)),
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

/** One breeder: its head (name, status), budget use, its jobs and its recent Foundry calls. */
function breederCard(b: Breeder): HTMLElement {
  const a = b.agent;
  const name = a.name.trim() === '' ? `agent ${a.cid}` : a.name;
  const jobRows = b.jobs.map((j) => [
    h('a.lab-mono.lab-nowrap', { href: href('lab', 'jobs', j.id) }, j.id),
    h('span.lab-badge', { class: JOB_TONE[j.status] ?? 'muted' }, j.status),
    h('span.lab-mono.lab-nowrap', null, j.recipe),
    h('span.lab-nowrap', null, fmtGpuHours(j.gpu_hours)),
    h('span.lab-nowrap', null, fmtTime(j.created_ms)),
  ]);
  const callRows = b.calls.map((c) => [
    h('span.lab-nowrap', null, fmtTime(c.at_ms)),
    h('span.lab-mono.lab-nowrap', null, c.name),
    c.detail ? h('span.lab-mono.lab-muted', null, clip(c.detail, 48)) : '—',
    h('span', { class: c.refused ? 'lab-error' : '' }, c.result ? clip(c.result.replace(/\s+/g, ' '), 140) : c.refused ? 'refused' : '—'),
  ]);
  return h(
    'section.lab-card.lab-breeder',
    null,
    h('header.lab-card-head', null, h('h3', null, h('a', { href: href('agents', a.cid) }, name)), h('span.lab-badge', { class: STATUS_TONE[a.status] ?? 'muted' }, a.status)),
    h(
      'div.lab-card-body',
      null,
      h(
        'div.lab-stack-tight',
        null,
        h(
          'div.lab-budget',
          null,
          h('div.lab-budget-head', null, h('span.lab-section-title', null, 'GPU-hours used'), h('span.lab-mono', null, fmtGpuHours(b.used))),
          h('span.lab-muted', null, `per-job cap ${fmtNumber(b.cap)} GPU-h (S-006c)`),
        ),
        b.note ? h('p.lab-error', null, b.note) : null,
        h('h4.lab-section-title', null, `Jobs (${fmtNumber(b.jobs.length)})`),
        dataTable(['Job', 'Status', 'Recipe', 'GPU-h', 'Created'], jobRows, 'No jobs yet.'),
        h('h4.lab-section-title', null, 'Recent Foundry calls'),
        dataTable(['Time', 'Tool', 'Input', 'Result'], callRows, 'No Foundry tool calls yet.'),
      ),
    ),
  );
}
