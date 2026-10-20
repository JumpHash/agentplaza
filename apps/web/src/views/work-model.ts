// The Work screen's pure parts: what a task is to the screen, the line each card says, a project's flow line, the
// plan's cards, the tasks "Only what needs me" keeps, and where the plan, the graph and delivery stand on the map.
// The view (work.ts) draws from these; test/tests.ts checks them.

import { PROJECT_STAGES, type AgentStep, type GraphTask, type Project, type ProjectStage, type Task } from '../api';
import type { Layout } from '../graph/wasm';
import { roleLabel } from '../roles';

/** The planning stages, in order: their cards make the Plan column. */
export const PLAN_STAGES: ProjectStage[] = ['brief', 'research', 'proposal', 'critique', 'approval'];

/** What a task is to the screen: the hub's state, an open task split by its links, a claim by its agent's step. */
export type WorkState = 'ready' | 'waiting' | 'blocked' | 'working' | 'needs-you' | 'review' | 'done' | 'failed' | 'withdrawn';

export const STATE_LABEL: Record<WorkState, string> = {
  ready: 'Ready',
  waiting: 'Waiting',
  blocked: 'Blocked',
  working: 'Working',
  'needs-you': 'Needs you',
  review: 'In review',
  done: 'Done',
  failed: 'Failed',
  withdrawn: 'Withdrawn',
};

/** The task's state on the screen. An open task is ready, waiting on its links, or blocked by one that failed; a claim
 * whose agent waits on the owner's decision needs you. */
export function workState(t: { state: string; ready: boolean; blocked_by: string[] }, step?: AgentStep | null): WorkState {
  switch (t.state) {
    case 'open':
      return t.blocked_by.length ? 'blocked' : t.ready ? 'ready' : 'waiting';
    case 'claimed':
      return step?.stage === 'approval' ? 'needs-you' : 'working';
    case 'awaiting_review':
      return 'review';
    case 'done':
      return 'done';
    case 'failed':
      return 'failed';
    case 'withdrawn':
      return 'withdrawn';
    default:
      return 'working';
  }
}

/** An agent's current step by its cid, as far as the screen knows it. */
export type StepOf = (cid: number) => AgentStep | null | undefined;

/** Whether a state is over: its card dims in place. */
export const ended = (s: WorkState): boolean => s === 'done' || s === 'failed' || s === 'withdrawn';

/** A goal's first line: the card's title. */
export const titleOf = (goal: string): string => goal.split('\n').find((l) => l.trim())?.trim() ?? 'Untitled task';

/** Items as a sentence lists them: `a`, `a and b`, `a, b and c`. */
export function listed(items: string[]): string {
  if (items.length <= 1) return items.join('');
  return `${items.slice(0, -1).join(', ')} and ${items[items.length - 1]}`;
}

/** A span said short: `40s`, `12m`, `3h`, `2d`. */
export function shortSpan(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86_400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86_400)}d`;
}

/** What an agent's step says on a card: `Running bash · 2m`. Empty without a step. */
export function stepText(step: AgentStep | null | undefined, now: number): string {
  if (!step) return '';
  const what = {
    thinking: 'Thinking',
    streaming: 'Writing',
    tool: step.tool ? `Running ${step.tool}` : 'Running tools',
    approval: step.tool ? `Waits for you to approve ${step.tool}` : 'Waits for your approval',
    idle: 'Idle',
    asleep: 'Asleep',
  }[step.stage];
  return `${what} · ${shortSpan(now - step.since_ms)}`;
}

/** A moment as the Work and Tasks cards say it: `just now`, `4 min ago`, `3 h ago`, `2 d ago`; `—` for none. */
export function agoLabel(ms: number | null | undefined, now = Date.now()): string {
  if (!ms) return '—';
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 45) return 'just now';
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))} min ago`;
  if (s < 86_400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86_400)} d ago`;
}

/** The tasks `id` runs after, from the links, as `[from, to]` pairs. */
export const upstreamOf = (id: string, edges: [string, string][]): string[] => edges.filter(([, to]) => to === id).map(([from]) => from);

/** Who can claim a task: `QA agent`, or `agent` when it names no role. Role labels are names (Finance,
 * Incident), so the wording never puts an article or a plural on them. */
export const claimant = (role: string | null): string => (role ? `${roleLabel(role)} agent` : 'agent');

/** What a card needs to say who has the task or what it waits for. */
export interface LineContext {
  edges: [string, string][];
  /** A task's state by id, for the links it waits on. */
  stateOf: (id: string) => string | undefined;
  /** How the line names another task: a card its id (that card is on the same map), the drawer its title. */
  taskName: (id: string) => string;
  /** When a ready task became ready (see `readySince`), when the screen knows. */
  readyAt?: (id: string) => number | undefined;
  nameOf: (cid: number) => string;
  stepOf: (cid: number) => AgentStep | null | undefined;
  now: number;
}

/** The line under a card's title: who has the task and what they do now, or what it waits for. */
export function cardLine(t: GraphTask, state: WorkState, ctx: LineContext): string {
  switch (state) {
    case 'ready': {
      const at = ctx.readyAt?.(t.id);
      return at === undefined ? `Any ${claimant(t.role)} can claim it` : `Unclaimed for ${shortSpan(ctx.now - at)}`;
    }
    case 'waiting': {
      const open = upstreamOf(t.id, ctx.edges).filter((id) => ctx.stateOf(id) !== 'done');
      return open.length ? `After ${listed(open.map(ctx.taskName))}` : 'Waiting';
    }
    case 'blocked':
      return `Blocked: ${listed(t.blocked_by.map(ctx.taskName))} did not finish`;
    case 'working': {
      if (t.agent_cid === null) return STATE_LABEL[state];
      const step = stepText(ctx.stepOf(t.agent_cid), ctx.now);
      return step ? `${ctx.nameOf(t.agent_cid)} · ${step}` : ctx.nameOf(t.agent_cid);
    }
    case 'needs-you': {
      // The decision first: the state already says it waits on the owner, and a long line is cut at its end.
      if (t.agent_cid === null) return STATE_LABEL[state];
      const step = ctx.stepOf(t.agent_cid);
      const what = step?.tool ? `Approve ${step.tool}` : 'Your approval';
      return `${what} · ${ctx.nameOf(t.agent_cid)}${step ? ` · ${shortSpan(ctx.now - step.since_ms)}` : ''}`;
    }
    case 'review': {
      if (t.reviewer_cid === null) return 'Waits for a reviewer';
      const step = stepText(ctx.stepOf(t.reviewer_cid), ctx.now);
      return `Reviewed by ${ctx.nameOf(t.reviewer_cid)}${step ? ` · ${step}` : ''}`;
    }
    default:
      return t.agent_cid === null ? STATE_LABEL[state] : `${STATE_LABEL[state]} · ${ctx.nameOf(t.agent_cid)}`;
  }
}

/** One square of a flow line. */
export interface Seg {
  kind: 'plan' | 'task' | 'end';
  /** `done`, `current` or `later` for the plan and the end; a WorkState for a task. */
  state: string;
  label: string;
}

/** The most task squares a flow line draws; the rest are counted. */
export const FLOW_TASKS = 24;

/** A project's flow line: its five planning stages, a square per task, then review and delivery. For tasks outside
 * projects (`project` null), the task squares alone. With the agents' steps, a task whose agent waits on the owner is
 * the owner's square. */
export function flowSegments(project: Project | null, tasks: Task[], stepOf?: StepOf): Seg[] {
  const segs: Seg[] = [];
  const at = project ? PROJECT_STAGES.indexOf(project.stage) : -1;
  const place = (i: number): string => (i < at ? 'done' : i === at ? 'current' : 'later');
  if (project) {
    for (const s of PLAN_STAGES) {
      const yours = s === 'approval' && planWaitsForOwner(project);
      segs.push({ kind: 'plan', state: yours ? 'you' : place(PROJECT_STAGES.indexOf(s)), label: yours ? 'approval · waits for you' : s });
    }
  }
  for (const t of tasks.slice(0, FLOW_TASKS)) {
    const state = workState(t, t.agent_cid !== null ? stepOf?.(t.agent_cid) : null);
    segs.push({ kind: 'task', state, label: `${t.id} · ${STATE_LABEL[state]}` });
  }
  if (project) {
    segs.push({ kind: 'end', state: place(PROJECT_STAGES.indexOf('review')), label: 'review' });
    segs.push({ kind: 'end', state: project.stage === 'delivered' ? 'done' : 'later', label: 'delivered' });
  }
  return segs;
}

/** Whether a project's plan waits on the owner: it asked for the owner's approval and is at critique or approval. */
export const planWaitsForOwner = (p: Project): boolean => !p.approved_by && p.owner_approval && (p.stage === 'critique' || p.stage === 'approval');

/** Whether a project (or, with `null`, the tasks outside projects) waits on the owner: the plan's approval, or a task
 * whose agent waits on the owner's decision. */
export const waitsForOwner = (p: Project | null, tasks: Task[], stepOf?: StepOf): boolean =>
  (p !== null && planWaitsForOwner(p)) || tasks.some((t) => t.agent_cid !== null && workState(t, stepOf?.(t.agent_cid)) === 'needs-you');

/** The ready tasks nobody has claimed, for one role (`null`: any agent): how many, and the longest wait. */
export interface RoleWait {
  role: string | null;
  count: number;
  waited: number;
}

/** When a ready task became ready: when it was posted, or when the last task it runs after finished. */
export const readySince = (t: Task, byId: Map<string, Task>): number =>
  Math.max(t.created_at, ...t.after.map((id) => byId.get(id)?.updated_at ?? 0));

/** The ready tasks nobody has claimed, by the role they wait for, longest wait first: where the plaza is short of an
 * agent. */
export function waitsByRole(tasks: Task[], now: number): RoleWait[] {
  const byId = new Map(tasks.map((t) => [t.id, t]));
  const groups = new Map<string | null, RoleWait>();
  for (const t of tasks) {
    if (t.state !== 'open' || !t.ready) continue;
    const since = readySince(t, byId);
    const group = groups.get(t.role) ?? { role: t.role, count: 0, waited: 0 };
    group.count += 1;
    group.waited = Math.max(group.waited, now - since);
    groups.set(t.role, group);
  }
  return [...groups.values()].sort((a, b) => b.waited - a.waited || b.count - a.count);
}

/** The tasks "Only what needs me" keeps: the ones that wait on the owner, and every task that runs after them. */
export function needsMe(states: Map<string, WorkState>, edges: [string, string][]): Set<string> {
  const kept = new Set<string>();
  const next = [...states].filter(([, s]) => s === 'needs-you').map(([id]) => id);
  while (next.length) {
    const id = next.pop() as string;
    if (kept.has(id)) continue;
    kept.add(id);
    for (const [from, to] of edges) if (from === id) next.push(to);
  }
  return kept;
}

/** One card of the Plan column. `you`: the plan waits on the owner's approval. */
export interface PlanCard {
  stage: ProjectStage;
  state: 'done' | 'current' | 'later' | 'you';
  title: string;
  line: string;
}

const stageLabel = (s: string): string => s.charAt(0).toUpperCase() + s.slice(1);

/** The Plan column's five cards, each with what its stage holds. */
export function planCards(p: Project): PlanCard[] {
  const at = PROJECT_STAGES.indexOf(p.stage);
  const kinds = (k: string[]): Project['docs'] => p.docs.filter((d) => k.includes(d.kind));
  return PLAN_STAGES.map((stage) => {
    const i = PROJECT_STAGES.indexOf(stage);
    const state: PlanCard['state'] = i < at ? 'done' : i === at ? 'current' : 'later';
    switch (stage) {
      case 'brief': {
        const doc = kinds(['brief'])[0];
        return { stage, state, title: 'Brief', line: doc ? doc.title : 'The orchestrator writes it' };
      }
      case 'research': {
        const n = kinds(['research']).length;
        return { stage, state, title: 'Research', line: n ? `${n} ${n === 1 ? 'finding' : 'findings'}` : 'Researchers attach what they find' };
      }
      case 'proposal': {
        const docs = kinds(['proposal', 'design', 'spec', 'tasks', 'revision']);
        const what = Array.from(new Set(docs.map((d) => d.kind))).join(', ');
        return { stage, state, title: p.revision > 1 ? `Proposal · rev ${p.revision}` : 'Proposal', line: what || 'The planner writes it' };
      }
      case 'critique': {
        const approve = p.critiques.filter((c) => c.verdict === 'approve').length;
        const revise = p.critiques.filter((c) => c.verdict === 'revise').length;
        const line = p.critiques.length ? `${approve} approve${revise ? ` · ${revise} revise` : ''}` : 'Agents other than the author critique it';
        return { stage, state, title: 'Critique', line };
      }
      default: {
        if (p.approved_by) return { stage, state: 'done', title: p.approved_by === 'owner' ? 'Approved by you' : `Approved by ${p.approved_by}`, line: 'The tasks were posted from it' };
        return { stage, state: planWaitsForOwner(p) ? 'you' : state, title: stageLabel(stage), line: p.owner_approval ? 'Waits for your approval' : 'The mayor approves' };
      }
    }
  });
}

/** A box on the map. */
export interface Box {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Where the plan, the graph and delivery stand on the map, and the links that join them to the graph. */
export interface MapGeometry {
  width: number;
  height: number;
  plan: Box | null;
  /** Where the layout's origin goes. */
  graph: { x: number; y: number };
  delivery: Box | null;
  /** The approved plan to each task that runs after no other (dashed). */
  posted: string[];
  /** Each task that no other runs after, to delivery (dashed). */
  delivered: string[];
}

/** An elbow from `a` to `b`, bending at `x`: an SVG path. */
const elbow = (a: [number, number], x: number, b: [number, number]): string => `M${a[0]} ${a[1]}H${x}V${b[1]}H${b[0]}`;

/** The map: the plan (its `anchor` is the height its links leave from), the graph laid out, then delivery, each
 * centred on the tallest, `gap` apart. */
export function compose(
  layout: Layout,
  parts: { plan: { width: number; height: number; anchor: number } | null; delivery: { width: number; height: number } | null; gap: number },
): MapGeometry {
  const { plan, delivery, gap } = parts;
  const height = Math.max(layout.height, plan?.height ?? 0, delivery?.height ?? 0);
  const graph = { x: plan ? plan.width + gap : 0, y: (height - layout.height) / 2 };
  const planBox = plan ? { x: 0, y: (height - plan.height) / 2, width: plan.width, height: plan.height } : null;
  const deliveryBox = delivery
    ? { x: graph.x + layout.width + gap, y: (height - delivery.height) / 2, width: delivery.width, height: delivery.height }
    : null;
  const at = (id: string) => layout.cards.find((c) => c.id === id);
  const posted =
    planBox && plan
      ? (layout.columns[0] ?? []).flatMap((id) => {
          const c = at(id);
          return c ? [elbow([planBox.width, planBox.y + plan.anchor], planBox.width + gap / 2, [graph.x + c.x, graph.y + c.y + c.height / 2])] : [];
        })
      : [];
  const sources = new Set(layout.routes.map((r) => r.from));
  const delivered = deliveryBox
    ? layout.cards
        .filter((c) => !sources.has(c.id))
        .map((c) =>
          elbow(
            [graph.x + c.x + c.width, graph.y + c.y + c.height / 2],
            graph.x + layout.width + gap / 2,
            [deliveryBox.x, deliveryBox.y + deliveryBox.height / 2],
          ),
        )
    : [];
  return {
    width: deliveryBox ? deliveryBox.x + deliveryBox.width : graph.x + layout.width,
    height,
    plan: planBox,
    graph,
    delivery: deliveryBox,
    posted,
    delivered,
  };
}

/** A route's corners as an SVG path, moved by `dx`, `dy`. */
export const pathOf = (points: [number, number][], dx: number, dy: number): string =>
  points.map(([x, y], i) => `${i ? 'L' : 'M'}${x + dx} ${y + dy}`).join('');

/** The state columns a project's tasks stand in when none of them has a link. */
export const STATE_COLUMNS: { label: string; states: WorkState[] }[] = [
  { label: 'Open', states: ['ready', 'waiting', 'blocked'] },
  { label: 'Working', states: ['working', 'needs-you'] },
  { label: 'In review', states: ['review'] },
  { label: 'Ended', states: ['done', 'failed', 'withdrawn'] },
];

/** A graph built from tasks outside projects (`GET /v1/tasks`): their links among themselves become its edges. */
export function looseGraph(tasks: Task[]): { tasks: GraphTask[]; edges: [string, string][] } {
  const loose = tasks.filter((t) => !t.project);
  const ids = new Set(loose.map((t) => t.id));
  return {
    tasks: loose.map((t) => ({
      id: t.id,
      goal: t.goal,
      role: t.role,
      depth: 0,
      state: t.state,
      ready: t.ready,
      blocked_by: t.blocked_by,
      agent_cid: t.agent_cid,
      reviewer_cid: null,
      step: null,
    })),
    edges: loose.flatMap((t) => t.after.filter((id) => ids.has(id)).map((id): [string, string] => [id, t.id])),
  };
}
