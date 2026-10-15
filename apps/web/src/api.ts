// The apid client: one function per endpoint, typed by what apid returns.
// The console is served by apid, so every path is same-origin.

import type { FileChange, Stats, TreePage, WorkspaceKind } from './ide/types';
import type { Ref } from './ui/rich';

export class ApiError extends Error {
  /** The response's JSON body when it had one (a 409 carries the server's `current_hash`). */
  constructor(public status: number, message: string, public diagnostics: Diagnostic[] = [], public body: unknown = null) {
    super(message);
  }
}

async function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(path, {
    method,
    headers: body === undefined ? {} : { 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await res.text();
  const data = text ? (JSON.parse(text) as unknown) : null;
  if (!res.ok) {
    // Off loopback the hub wants the owner's sign-in; the shell shows the sign-in screen.
    if (res.status === 401 && path !== '/v1/auth/session') window.dispatchEvent(new CustomEvent('apz-auth-required'));
    const message = (data as { error?: string } | null)?.error ?? `${method} ${path}: ${res.status}`;
    throw new ApiError(res.status, message, (data as { diagnostics?: Diagnostic[] } | null)?.diagnostics ?? [], data);
  }
  return data as T;
}

/**
 * A request whose answer is not JSON (a picture, a file's bytes) or whose body is raw bytes. Errors are thrown as
 * `call` throws them, so the callers see the same `ApiError`.
 */
async function raw(method: string, path: string, body?: BodyInit, contentType?: string): Promise<Response> {
  const res = await fetch(path, { method, headers: contentType ? { 'content-type': contentType } : {}, body });
  if (!res.ok) {
    if (res.status === 401) window.dispatchEvent(new CustomEvent('apz-auth-required'));
    const data: { error?: string } | null = await res.json().catch(() => null);
    throw new ApiError(res.status, data?.error ?? `${method} ${path}: ${res.status}`, [], data);
  }
  return res;
}

const get = <T>(path: string) => call<T>('GET', path);
const post = <T>(path: string, body: unknown = {}) => call<T>('POST', path, body);
const put = <T>(path: string, body: unknown) => call<T>('PUT', path, body);
const patch = <T>(path: string, body: unknown) => call<T>('PATCH', path, body);
const del = <T>(path: string) => call<T>('DELETE', path);

/** A query string from the defined parameters: `?limit=50&q=x`, or `''`. */
const query = (params: Record<string, string | number | undefined>): string => {
  const parts = Object.entries(params).filter(([, v]) => v !== undefined && v !== '').map(([k, v]) => `${k}=${encodeURIComponent(String(v))}`);
  return parts.length ? `?${parts.join('&')}` : '';
};

// ---- Types -----------------------------------------------------------------

/** A sleeping agent's VM is paused or stopped; a chat or a task wakes it. */
export type AgentStatus = 'offline' | 'idle' | 'working' | 'asleep';
export type Tier = 'orchestrator' | 'worker';
export type WorkKind = 'task' | 'chat' | 'message' | 'motion';

export interface Usage { input_tokens: number; output_tokens: number; cache_read_tokens: number; cache_write_tokens: number }

export interface WorkItem {
  id: number; cid: number; kind: WorkKind; thread: string; prompt: string;
  origin: { task: string | null; motion: string | null; from_cid: number | null };
  created_ms: number; started_ms: number | null;
}

export interface Agent {
  cid: number; name: string; role: string; role_label: string; tier: Tier; status: AgentStatus;
  model: string; model_override: string | null; model_source: 'agent' | 'role' | 'default';
  connected_at_ms: number; last_seen_ms: number; run: string | null; vm: number | null;
  usage: Usage; work: WorkItem | null; queue: number; species?: string;
  kind: string;
}

export interface Overview {
  agents: { total: number; idle: number; working: number; offline: number };
  tasks: Record<string, number>;
  default_model: string; orchestrator_model: string; dev_models: boolean; hub: boolean; noded: boolean;
  /** The plaza's git host's web origin, for the rich links in agent text (null when the forge is not configured). */
  git_host: string | null;
}

export type Auth = 'none' | { env: string } | { secret: string } | { command: string };

export interface ProviderKind {
  id: string; kind: string; label: string; brand: string; base_url: string | null;
  auth: Auth; notes: string; models: string[]; options?: Record<string, unknown>;
}

export interface Provider {
  id: string; kind: string; base_url: string | null; auth: Auth; models: string[];
  options: Record<string, unknown> | null; extra_headers: Record<string, string>;
  credential: 'env' | 'secret' | 'none' | 'missing' | 'command';
  group?: ModelGroup;
}

export interface ProviderInput {
  id: string; kind: string; base_url?: string | null; auth?: Auth; models?: string[];
  options?: Record<string, unknown>; extra_headers?: Record<string, string>; extra_body?: Record<string, unknown>;
}

export interface TestReport { ok: boolean; model: string; latency_ms: number; reply: string | null; error: string | null }

export type ConnectorCategory = 'research' | 'media' | 'infra' | 'messaging' | 'dev';
export type ConnectorRisk = 'read' | 'write' | 'irreversible';
export interface ConnectorAction { name: string; description: string; risk: ConnectorRisk }
/** A tool an agent can be lent through the plaza. Its key lives in the vault, never in a VM. */
/** One credential a connector needs: in the vault, or read from apid's environment (`env`). */
export interface ConnectorField {
  name: string; label: string; env: string; hint: string; secret: boolean; required: boolean;
  /**
   * Whether a value is available, and where from: the vault or the environment; `missing` when neither
   * holds it; `sealed` when the vault is sealed so it cannot tell; `out_of_scope` when only other nodes hold it.
   */
  set: boolean; source: 'vault' | 'env' | 'missing' | 'sealed' | 'out_of_scope' | null;
}
export interface Connector {
  id: string; label: string; brand: string; kind: 'api' | 'cli'; category: ConnectorCategory;
  description: string; docs_url: string; key_hint: string;
  /** Every required field is set. */
  configured: boolean; enabled: boolean; fields: ConnectorField[];
  /** Empty means every role may use it. */
  roles: string[]; actions: ConnectorAction[]; cli: { package: string; binary: string } | null;
}
export interface ConnectorTest { ok: boolean; latency_ms: number; detail: string }
/** `chat` models write text for agents; `decisions` models answer typed questions for the plaza. */
export type ModelGroup = 'chat' | 'decisions';
export interface ModelInfo { provider: string; id: string; name: string | null; group?: ModelGroup; trains_on_data?: boolean }
export interface ModelsView { models: ModelInfo[]; errors: { provider: string; error: string }[] }

/** Contributor models: their vendor trains future models on what is sent to and returned by them. */
export const isContributor = (m: { trains_on_data?: boolean }): boolean => m.trains_on_data === true;
/** The ids a provider kind lists as contributor models (`options.trains_on_data`). */
export const contributorIds = (options: Record<string, unknown> | null | undefined): string[] => {
  const ids = options?.trains_on_data;
  return Array.isArray(ids) ? ids.filter((id): id is string => typeof id === 'string') : [];
};

/** Provider kinds whose models are decision models (the kind list carries no group). */
const DECISION_KINDS = ['openai-decisions', 'systemone', 'azure-decisions'];
export const isDecisionKind = (kind: string): boolean => DECISION_KINDS.includes(kind);

// ---- Decisions: typed questions answered with calibrated probabilities ----
export interface DecisionChoice { value: string; description?: string }
export interface DecisionLevel { label: string; description?: string }
export type DecisionQuestion =
  | { type: 'predicate'; name: string; instructions: string }
  | { type: 'choice'; name: string; instructions: string; choices: DecisionChoice[] }
  | { type: 'score'; name: string; instructions: string; levels: DecisionLevel[] };
export interface DecisionRequest { provider: string; model: string; input: string; questions: DecisionQuestion[] }
export type DecisionAnswer =
  | { type: 'predicate'; name: string; probability: number }
  | { type: 'choice'; name: string; choice: string; probabilities: { value: string; probability: number }[]; confidence: number }
  | { type: 'score'; name: string; score: number; probabilities: { value: number; label: string; probability: number }[]; confidence: number }
  | { type: 'refusal'; name: string };
export interface DecisionResponse { provider: string; model: string; answers: DecisionAnswer[]; usage: { input_tokens: number } }
export interface DecisionDefault { provider: string | null; model: string | null }

/** The owner's retention limits (apid's `retention` block). `0` days keeps forever. */
export interface Retention { conversations_max: number; conversations_days: number; posts_max: number; posts_days: number }

export interface Settings {
  retention?: Retention;
  default_model: string; orchestrator_model: string; role_models: Record<string, string>;
  model_policy: { orchestrator: string[]; worker: string[] };
  effort: string | null; max_turns: number; max_tokens: number; fetch_block: string[];
  prices: Record<string, { input: number; output: number }>; dev_models?: boolean;
  /** Strict permits: when on, an operation with permit laws is refused unless one of them allows it. */
  laws?: { strict_permits: boolean };
  /** Contributor models (their vendor trains on the data sent to them) may be defaults, role models and policy entries. */
  allow_training_models?: boolean;
  /** Minutes an idle agent waits before its VM sleeps; 0 = never. */
  idle_sleep_min: number;
  /** Roles whose agents never sleep when idle. */
  sleep_never_roles: string[];
  /** Shared read-only data volumes per role: agents of the role mount each one at /data/<name>. */
  role_volumes: Record<string, string[]>;
  /** Take a checkpoint of the agent's VM before each irreversible tool call. */
  vm_auto_checkpoints: boolean;
}

/** One saved state of an agent's VM (memory, disk and running processes). `diff` builds on the one before it. */
export interface VmCheckpoint {
  name: string; cid: number; label: string; kind: 'full' | 'diff'; taken_at_ms: number;
  memory_written: number; disk_bytes: number; paused_ms: number;
}

export interface VmCheckpointList { checkpoints: VmCheckpoint[] }

export interface ForkedAgent { cid: number; run: string; name: string }

/** The forks made from one checkpoint of the agent's VM, which the forks start from. */
export interface ForkResult { forks: ForkedAgent[]; checkpoint: VmCheckpoint }

export interface VmRestoreResult { ok: boolean; safety: VmCheckpoint }

export interface Task {
  id: string; state: string; goal: string; acceptance: string | null; role: string | null; budget: number;
  assignee_id: string | null; agent_cid: number | null; note: string | null; outcome: string | null;
  created_at: number; updated_at: number;
}

export type Block =
  | { type: 'text'; text: string }
  | { type: 'thinking'; text: string }
  | { type: 'tool_use'; id: string; name: string; input: unknown }
  | { type: 'tool_result'; tool_use_id: string; content: string; is_error: boolean }
  | { type: 'opaque'; kind: string; raw: unknown };

export interface ChatResponse {
  model: string; content: Block[]; stop: string | { other: string };
  usage: Usage;
}

export type EntryKind = 'work_start' | 'model_request' | 'model_response' | 'tool_request' | 'tool_decision' | 'tool_result' | 'work_end' | 'error';
export interface Entry { seq: number; at_ms: number; cid: number; work: number; kind: EntryKind; data: Record<string, unknown> }

export interface Delta { cid: number; work: number; req: number; delta: { type: 'text' | 'thinking'; text: string } | { type: 'tool_use'; name: string } }

/** A comment in a board post's thread: by an agent (`agent_cid`), or by the owner (`agent_cid` null). */
export interface Discovery { id: string; name: string; kind: string; what_for: string; how: string; verdict: string; url: string | null; tags: string[]; by_cid: number; by: string; at_ms: number; endorsed_by: number[]; flagged: string | null }
export type ProjectStage = 'brief' | 'research' | 'proposal' | 'critique' | 'approval' | 'implementation' | 'review' | 'delivered';
export const PROJECT_STAGES: ProjectStage[] = ['brief', 'research', 'proposal', 'critique', 'approval', 'implementation', 'review', 'delivered'];
export interface ProjectDoc { kind: string; title: string; file: string | null; repo_path: string | null; commit: string | null; summary: string; by_cid: number; by: string; at_ms: number }
export interface ProjectCritique { by_cid: number; by: string; verdict: 'approve' | 'revise' | 'comment'; text: string; revision: number; at_ms: number }
export interface ProjectEvent { at_ms: number; by_cid: number; by: string; what: string; note: string }
export interface Project { id: string; title: string; goal: string; kind: 'code' | 'other'; stage: ProjectStage; lead_cid: number; owner_approval: boolean; repo: string | null; docs: ProjectDoc[]; critiques: ProjectCritique[]; revision: number; approved_by: string | null; tasks: string[]; history: ProjectEvent[]; created_ms: number; updated_ms: number }
export interface GitRepo { name: string; description: string; default_branch: string; updated_at: string; size_kb: number; open_prs: number; project: string | null }
export interface GitBranch { name: string; commit: { id: string; message: string; author: string; at: string } }
export interface GitCommit { id: string; short: string; message: string; author: string; at: string; additions: number; deletions: number }
export interface GitEntry { name: string; path: string; kind: 'file' | 'dir'; size: number }
export interface GitFile { path: string; ref: string; size: number; text: string | null; binary: boolean }
/** A pull request row; `comments` is the count of its discussion, which the detail carries in full. A merged pull request is closed, with `merged` true; older hubs omit `merged`. */
export interface GitPull { number: number; title: string; state: string; author: string; head: string; base: string; created_at: string; updated_at: string; comments: number; review: 'approved' | 'changes_requested' | 'pending'; mergeable: boolean; merged?: boolean; merged_at?: string | null }
export interface GitComment { author: string; body: string; at: string }
export interface GitPullDetail extends GitPull { body: string; diff: string; reviews: { author: string; state: string; body: string; at: string }[]; discussion: GitComment[]; files: { path: string; additions: number; deletions: number }[] }
export interface HarnessLimits { max_turns: number | null; compress_at: number | null; keep_recent: number | null }
export interface HarnessLoop { id: string; cid: number; prompt: string; every_secs: number; max_runs: number; runs: number; next_at_ms: number; state: 'running' | 'done' | 'stopped'; by: string; by_cid: number; created_ms: number; last_work: number | null; last_summary: string | null }
export interface LawInfo {
  id: string; title: string; tier: string; enforcement: string; severity: string; statement: string; rationale: string;
  effect: 'permit' | 'forbid'; ops: string[]; principal: string; resource: string; conditions: string[]; explanation: string;
  file: string; source: string; refusals: number; agents: { cid: number; name: string; count: number }[]; last_at_ms: number | null;
  origin: 'built-in' | 'owner'; enabled: boolean; rule_id: string; overridden: LawOverride | null;
  /** A constitutional law that is switched off now. */
  constitutional_off: boolean;
}
export interface LawOverride { law: string; disabled_by: string; reason: string; at_ms: number }
/** A change of state of a constitutional law, on record: who, when, and whether it was confirmed by typing the id. */
export interface LawChange { law: string; title: string; tier: string; disabled: boolean; by: string; reason: string; at_ms: number; confirmed: boolean }
export interface LawRule { id: string; title: string; source: string; enabled: boolean; created_ms: number; updated_ms: number; author: string }
export interface LawRules { rules: LawRule[]; overrides: LawOverride[]; strict_permits: boolean; constitutional_off: string[]; history: LawChange[] }
/** One compile error: `line` and `column` are 1-based positions in the source. */
export interface Diagnostic { file: string; line: number; column: number; message: string }
export interface WouldRefuse { cid: number; role: string; tool: string; at_ms: number; laws: string[] }
export interface LawCheck { ok: boolean; diagnostics: Diagnostic[]; laws: string[]; would_refuse: WouldRefuse[] }
export interface Infraction { cid: number; law: string; at_ms: number; tool: string; detail: string; law_title: string; statement: string; agent_name: string }
export interface RoleDef {
  id: string; title: string; purpose: string; responsibilities: string[]; deliverables: string[]; collaboration: string;
  boundaries: string[]; tools: string[]; budget: number; model: string; prompt: string; created_by: string; created_ms: number;
}
export interface RoleView { id: string; title: string; source: 'builtin' | 'custom'; def: RoleDef; agents: number }
export interface AgentProposal {
  id: string; role: RoleDef; new_role: boolean; count: number; model: string; reason: string; instruction: string;
  by_cid: number; by: string; created_ms: number; state: 'pending' | 'approved' | 'rejected'; decided_ms: number; note: string; spawned: number[];
}
export interface PlazaState { paused: boolean; since_ms: number; by: string; reason: string; cancelled: number }
export interface BoardComment { id: string; post: string; agent_cid: number | null; author: string; body: string; created_at: number; attachments: string[] }
export interface BoardPost { id: string; agent_cid: number; role: string; subject: string; body: string; created_at: number; task: string | null; comments: BoardComment[]; attachments: string[]; owner?: boolean; project?: string | null; mentions?: string[] }
/** The board as the list endpoint answers it: the posts (newest first), and whether its deliveries are paused. */
export interface BoardView { posts: BoardPost[]; total: number; limit: number; truncated: boolean; unread: { agent_cid: number; count: number }[]; project: string | null; paused: boolean; paused_all: boolean; paused_projects: string[] }
export interface Conversation { id: string; from_cid: number; to_cid: number; from_role: string; to_role: string; text: string; created_at: number; delivered: boolean; attachments?: string[] }
/** One pair's summary over the retained record: `last_at` and `count` are the whole record's, not the page's. */
export interface ConversationPair { a: number; b: number; active: boolean; count: number; last_at: number }
/** Paging and search for the conversation reads; `before` is the `next_before` of the page before. */
export interface ConversationQuery { limit?: number; before?: number; q?: string }
/** `GET /v1/conversations`: one page of exchanges, newest first. */
export interface ConversationsPage {
  conversations: Conversation[]; pairs: ConversationPair[]; active_secs: number; total: number;
  limit: number; truncated: boolean; next_before: number | null; pairs_total: number; pairs_truncated: boolean;
}
/** `GET /v1/conversations/{a}/{b}`: one page of a thread, oldest first within the page. */
export interface ThreadPage {
  a: number; b: number; active: boolean; active_secs: number; messages: Conversation[];
  total: number; limit: number; truncated: boolean; next_before: number | null;
}
/** The quorum a motion is counted under: the rule in words and its numbers. */
export interface MotionQuorum { rule: string; seats: number; filled: number; required: number; can_decide: boolean }
/** One seat's vote on a motion: the seat (its role), the agent that cast it, the word and the reason. */
export interface MotionVote { seat: string; agent_cid: number; vote: string; reason: string; at: number }
export interface Motion { id: string; kind: string; text: string; state: string; proposer_cid: number; tally: { yes: number; no: number; abstain: number; voted: number; seats: number }; votes?: MotionVote[]; quorum: MotionQuorum; created_at: number }
export interface Account { id: number; kind: string; owner_ref: string; balance: number; credit_limit: number; created_at: number }
export interface Treasury { accounts: Account[]; totals: { accounts: number; entries: number; credits: number; debits: number; balance: number }; spent: number; plaza_budget: number; note: string }
/** One leg of a journal entry: a debit is negative, a credit positive. `account` is the account id. */
export interface Posting { account: number; amount: number; kind: string; owner_ref: string }
/** A posted entry. `amount` is the size of the movement; `postings` say where it went. Newest first by `id`. */
export interface JournalEntry { id: number; kind: string; amount: number; created_at: number; memo: string; posted_by: string; postings: Posting[]; task_id: string | null; root_task: string | null }
export interface JournalPage { entries: JournalEntry[]; total: number; limit: number; truncated: boolean; next_before: number | null }
export interface JournalQuery { before?: number; limit?: number; kind?: string; wallet?: number; task?: string }
/** One bucket of the series: the flows inside [at_ms, end_ms) and the balances at its end. */
export interface TreasuryPoint {
  at_ms: number; end_ms: number; entries: number;
  minted: number; escrowed: number; paid: number; refunded: number; fined: number; spent: number;
  by_kind: Record<string, number>;
  plaza_budget_end: number; circulating_end: number; treasury_end: number;
}
export interface TreasurySeries { from_ms: number; to_ms: number; bucket_ms: number; points: TreasuryPoint[] }
export interface Wallet { cid: number; name: string; balance: number }
export interface Machine { name: string; arch: string; backend: string; irqchip: string; class: string; cpus: number; memory_mib: number; vsock_mode: string; started_at_ms: number }
export interface Vm { index: number; state: string; guest_cid: number | null; role?: string | null }
export interface Run { id: string; state: string; vcpus: number; memory_mib: number; beats: number | null; role: string | null; created_at: number; ended_at: number | null; vms: Vm[] }
export interface Gpu { vendor: string; name: string; memory_mib: number | null }
/** One process the plaza runs on a machine. `cpu_percent` is of the whole machine (all cores = 100). */
export interface ProcessUsage { name: string; pid: number; cpu_percent: number; rss_mib: number }
/** An agent VM's process, and its guest memory as the balloon reports it. */
export interface VmUsage extends ProcessUsage { run: string; vm: number; cid: number | null; role: string | null; usable_mib: number | null; guest_used_mib: number | null }
export interface PlazaUsage {
  /** apid, the hub (vsockd), noded, garage — whichever run on this machine. */
  services: ProcessUsage[];
  vms: VmUsage[];
  total: { cpu_percent: number; rss_mib: number; services_cpu_percent: number; services_rss_mib: number; vms_cpu_percent: number; vms_rss_mib: number };
}
export interface MachineNode extends Machine {
  id: string;
  kind: FleetKind;
  /** The maker in lower case (apple, dell, ...); null when unknown. */
  vendor?: string | null;
  /** The product's name, e.g. "MacBook Pro (M3 Max)"; null when unknown. */
  model?: string | null;
  /** Only on Macs. */
  apple?: FleetApple | null;
  gpus: Gpu[];
  noded_port?: number;
  runs?: Run[];
  agents?: number;
  reachable?: boolean;
  stats: {
    cpu_percent: number; load: [number, number, number]; mem_total_mib: number; mem_used_mib: number; uptime_s: number;
    vms_running: number; guest_mem_mib: number; history: { t_ms: number; cpu_percent: number; mem_used_mib: number }[];
    /** Where the machine's resources go: the plaza's services and its agents' VMs (absent on older nodes). */
    plaza?: PlazaUsage;
  } | null;
}

/** One agent's view of another (AP-072): decayed evidence r/s, belief, disbelief, uncertainty, expected trust. */
export interface Opinion { r: number; s: number; b: number; d: number; u: number; e: number; updated_ms: number }
/** A directed contact edge (AP-071) with the holder's opinion of the peer. `note` is the holder's private memory of the peer. */
export interface Bond {
  from: number; to: number; kind: 'met' | 'hired' | 'was-hired' | 'added'; since_ms: number;
  strength: number; interactions: number; last_ms: number; note: string | null; blocked: boolean;
  opinion: Opinion;
}
export interface SocietyAgent { cid: number; name: string; role: string; reputation: number; confidence: number; wallet: number }
export interface Society { agents: SocietyAgent[]; bonds: Bond[]; computed_ms: number }
export interface MemoryNote { slug: string; title: string; kind: 'note' | 'journal' | 'person'; links: string[]; tags: string[]; updated_ms: number; size: number }
export interface MemoryDoc extends MemoryNote { body: string; backlinks: string[]; provenance: { task: string | null; work: number | null } }

/** One agent's record (GET /v1/agents/{cid}/profile): money, work, standing and conduct. */
export interface AgentProfile {
  cid: number; joined_ms: number;
  wallet: number; earned: number; spent: number;
  tasks: { posted: number; done: number; verified: number; failed: number };
  reputation: number; confidence: number; contacts: number; memory_notes: number;
  /** 0–100: starts at 100, loses points for law violations (30 days), refused tool calls (7 days) and fines. */
  conduct: number;
  fines: { count: number; total: number };
  denials: number;
  violations: { law: string; at_ms: number; detail: string }[];
}

/** A connector an agent proposed: its declarative spec, waiting for (or past) the owner's review. */
export interface ConnectorProposal {
  id: string; proposer_cid: number; proposer: string; created_ms: number;
  state: 'pending' | 'approved' | 'rejected'; note: string | null;
  /** Problems the plaza found when checking the spec (empty = it passed). */
  checks: string[];
  spec: {
    id: string; label: string; category: ConnectorCategory; description: string; docs_url: string; base_url: string;
    auth: { kind: 'bearer' | 'header' | 'query' | 'path'; name?: string };
    fields: { name: string; label: string; env: string; secret: boolean; required: boolean; hint: string }[];
    actions: { name: string; description: string; risk: ConnectorRisk; method: string; path: string; input_schema: unknown }[];
  };
}

/** One unfurl answer: the card for a reference, or why there is none (the hub's `{ok, card}` or `{ok:false, error}`). */
export type UnfurlAnswer = { ok: true; card: UnfurlCard } | { ok: false; error: string };
/** A card as the hub's unfurl builds it (crates/apid/src/unfurl): `kind` names the reference, `url` is its web page
 * when the hub knows one. The fields depend on the kind; the web reads them with type checks (ui/embeds.ts). */
export type UnfurlCard = { kind: string; url?: string | null } & Record<string, unknown>;

/** A deliverable in the Library: a file an agent handed in with deliver_file. */
export interface Deliverable {
  id: string; name: string; title: string; description: string | null; content_type: string; size: number;
  author_cid: number; author: string; task: string | null; created_ms: number; sha256: string;
  /** Where it is stored: "garage" (S3) or "local". */
  store: string;
  /** A deliverable the owner asked for, or a file an agent shared. */
  kind: 'deliverable' | 'shared';
  /** public: every agent (and the board); private: its author, the agents in `allowed`, and the owner. */
  audience: 'public' | 'private';
  allowed: number[];
}

/** An agent-made tool in the plaza's library. */
export interface LibraryTool {
  name: string; version: number; author_cid: number; author: string; description: string;
  command: string[]; requires: string[]; input_schema: unknown;
  /** private: only its author; pending: awaiting review to publish; published: every agent; rejected. */
  state: 'private' | 'pending' | 'published' | 'rejected';
  files: { path: string; size: number }[]; created_ms: number; calls: number; review_note: string | null;
}

// ---- Endpoints -------------------------------------------------------------

/** `GET /v1/fleet`: the machines of the plaza (the hub and its nodes) and the links between them. */
export type FleetKind = 'workstation' | 'laptop' | 'edge' | 'phone' | 'server';
export type FleetStatus = 'online' | 'draining' | 'offline' | 'revoked';
export interface FleetApple { chip: string; p_cores: number; e_cores: number; gpu_cores: number; neural_cores: number; unified_memory_gib: number }
export interface FleetMachine {
  id: string;
  name: string;
  role: 'hub' | 'node' | 'station';
  kind: FleetKind;
  /** The maker in lower case (apple, dell, lenovo, raspberrypi, samsung, ...); null when unknown. */
  vendor: string | null;
  model: string | null;
  /** Only on Macs. */
  apple?: FleetApple | null;
  status: FleetStatus;
  arch: string;
  os: string;
  backend: string;
  cpu: { model: string; cores: number; threads: number };
  memory_mib: number;
  gpus: { name: string; vendor: string; memory_mib: number | null }[];
  storage_gib: number;
  /** The cids running there. */
  agents: number[];
  /** Of those, how many are working. */
  working: number;
  /** Live load; null when unknown. */
  stats: { cpu_pct: number; mem_used_mib: number } | null;
  last_seen_ms: number;
  /** Only on a workstation: the station's own summary. */
  station?: FleetStation | null;
}
export interface FleetStation { form: StationForm; os: StationOs; leases: number; active: number; account: 'agent_user' | 'owner'; account_name: string }
export interface FleetLink { a: string; b: string; online: boolean }
export interface FleetResponse { machines: FleetMachine[]; links: FleetLink[] }

// ---- Workstations: machines agents operate through the hub (docs/design/workstations.md) ----
export type StationStatus = 'online' | 'offline' | 'draining' | 'revoked';
export type StationOs = 'macos' | 'windows' | 'linux';
export type StationForm = 'laptop' | 'desktop' | 'mini' | 'server' | 'vm';
/** The OS account the agents work in: a separate account made for them, or the owner's own (the hub shows that). */
export type StationAccount = { mode: 'agent_user'; name: string } | { mode: 'owner'; name: string };
export interface StationGpu { vendor: string; name: string; vram_mb: number }
export interface StationDisplay { id: number; w: number; h: number; scale: number }
/** What the daemon reports about its machine (`apz-station` facts). */
export interface StationFacts {
  os: StationOs; os_version: string; arch: string; hostname: string; form: StationForm;
  cpus: number; ram_mb: number; gpus: StationGpu[]; displays: StationDisplay[]; account: StationAccount;
  apps: number; virtual_displays: boolean; ui_tree: boolean;
}
/** What happens when the owner takes over a pixel session. */
export type Takeover = 'agent_waits' | 'release_lease' | 'keep_working_pointer_paused';
export interface StationSettings {
  takeover: Takeover; record: boolean; retention_days: number; record_fps: number;
  record_takeovers: boolean; work_folder: string; snapshot_reset: boolean;
  /** What an hour of a lease costs the lease's task, in plaza credits (1 credit is $0.001); 0 is free. */
  rate_per_hour: number;
}
export type LeaseKind = 'shared' | 'pixel';
export type LeaseState = 'queued' | 'opening' | 'active';
/** A lease with its place in the queue while it waits (1 is next). */
export interface StationLease {
  id: string; station: string; agent: number; task: string | null; kind: LeaseKind;
  session: number | null; started_ms: number; state: LeaseState; position: number | null;
}
export interface StationView {
  id: string; name: string; status: StationStatus; online: boolean; draining: boolean; revoked: boolean;
  version: string; code: string; joined_ms: number; last_seen_ms: number; address: string;
  facts: StationFacts; settings: StationSettings; leases: StationLease[];
}
export type StationCodeState = 'active' | 'used' | 'expired' | 'revoked';
export interface StationCode { id: string; label: string; created_ms: number; expires_ms: number; state: StationCodeState; used_by: string | null }
/** One audit row: the common fields, then whatever the action recorded (a lease, a reason, the settings). */
export interface StationAuditRow {
  at_ms: number; actor: string; station: string | null; agent: number | null; kind: string;
  [detail: string]: unknown;
}
export interface StationSettingsAnswer { settings: StationSettings; pushed: boolean; push_error: string | null }

/** A workstation VM the plaza makes on a node (`/v1/stations/vms`, docs/design/workstations.md section 10). */
export type VmOs = 'linux' | 'windows' | 'macos';
export type VmBackend = 'plaza' | 'qemu';
export interface StationVm {
  id: string; node: string; vm: string; name: string; os: VmOs; cpus: number; ram_mb: number; disk_gb: number;
  /** `none`, `virtual` or `vfio:<pci-id>`. */
  gpu: string; image: string | null; backend: VmBackend; station: string | null; created_ms: number; online: boolean;
}
/** The `POST /v1/stations/vms` body. `auto` lets the hub pick the backend. */
export interface StationVmBody {
  node: string; os: VmOs; name: string; cpus: number; ram_mb: number; disk_gb: number;
  gpu: string; image?: string; backend: 'auto' | VmBackend;
}

/** A mouse button, as the daemon names it. */
export type InputButton = 'left' | 'right' | 'middle';
/**
 * One owner action on a screen session (apz-station's `InputAction`). Points are in the pixels of the frame the
 * owner sees; scroll amounts are notches, positive `dy` scrolls the content down.
 */
export type InputAction =
  | { action: 'move'; x: number; y: number }
  | { action: 'click'; x: number; y: number; button: InputButton; count: number }
  | { action: 'down'; button: InputButton }
  | { action: 'up'; button: InputButton }
  | { action: 'scroll'; x: number; y: number; dx: number; dy: number }
  | { action: 'type'; text: string }
  | { action: 'key'; combo: string }
  | { action: 'drag'; from: [number, number]; to: [number, number] };

/** A recorded lease (`recordings::Recording` on the hub). `ended_ms` is null while the lease is open. */
export interface StationRecording {
  id: string; station: string; lease: string; agent: number; task: string | null; started_ms: number; ended_ms: number | null;
  frames: number; actions: number; bytes: number; takeover: boolean; width: number | null; height: number | null;
}
/** One action of a recording: `summary` is the action as it was asked for. */
export interface StationRecordingEvent { at_ms: number; kind: string; ok: boolean; summary: unknown }
/** A frame's capture time: its seq (counts from 1) and when it was captured (epoch ms), in seq order, the first 20000. */
export interface StationRecordingFrameTime { seq: number; ts_ms: number }
export interface StationRecordingDetail extends StationRecording { events: StationRecordingEvent[]; frame_times: StationRecordingFrameTime[] }
/** The hub-wide recording defaults, for the stations that join from now on. */
export interface RecordingDefaults { record: boolean; retention_days: number; record_fps: number; record_takeovers: boolean }

/** A file of a work folder's mirror on the hub. */
export interface SyncFile { path: string; hash: string | null; size: number; mtime_ms: number; version: number; conflict_of: string | null; deleted: boolean }
/**
 * The work-folder sync's state. `paused`, `reason` and `pending_deletions` come when the sync holds its deletions for the
 * owner to decide (the hub adds them); `pending_deletions` lists paths, as strings or as `{path}` objects.
 */
export interface StationSync {
  station: string; linked: boolean; files: number; bytes: number; last_sync_ms: number; conflicts: SyncFile[];
  paused?: boolean; reason?: string | null; pending_deletions?: unknown[];
}
/** What the Library answers when a file is delivered into it. */
export interface StationDelivery { id: string; name: string; size: number; kind: string; audience: string }

/** One path segment, encoded. */
const seg = (text: string): string => encodeURIComponent(text);
/** A work-folder file's URL: each segment of its path encoded, the slashes kept. */
const filesUrl = (station: string, path: string): string => `/v1/stations/${seg(station)}/files/${path.split('/').map(seg).join('/')}`;

export const api = {
  overview: () => get<Overview>('/v1/overview'),

  providerKinds: () => get<ProviderKind[]>('/v1/provider-kinds'),
  providers: () => get<Provider[]>('/v1/providers'),
  addProvider: (p: ProviderInput) => post<Provider>('/v1/providers', p),
  updateProvider: (p: ProviderInput) => put<Provider>(`/v1/providers/${p.id}`, p),
  removeProvider: (id: string) => del<unknown>(`/v1/providers/${id}`),
  setSecret: (id: string, value: string) => put<unknown>(`/v1/providers/${id}/secret`, { value }),
  testProvider: (id: string, model?: string) => post<TestReport>(`/v1/providers/${id}/test`, model ? { model } : {}),
  providerModels: (id: string) => get<{ id: string; name: string | null }[]>(`/v1/providers/${id}/models`),
  /** Chat models only: every chat picker (agents, policy) is built from this, so decision models are never offered. */
  models: async (): Promise<ModelsView> => {
    const view = await get<ModelsView>('/v1/models');
    return { ...view, models: view.models.filter((m) => m.group !== 'decisions') };
  },
  /** Decision models only. */
  decisionModels: async (): Promise<ModelsView> => {
    const view = await get<ModelsView>('/v1/models');
    return { ...view, models: view.models.filter((m) => m.group === 'decisions') };
  },
  askDecision: (r: DecisionRequest) => post<DecisionResponse>('/v1/decisions', r),
  getDecisionDefault: () => get<DecisionDefault>('/v1/decisions/default'),
  setDecisionDefault: (d: { provider: string; model: string }) => put<DecisionDefault>('/v1/decisions/default', d),
  settings: () => get<Settings>('/v1/settings'),
  saveSettings: (s: Settings) => put<Settings>('/v1/settings', s),

  agents: () => get<Agent[]>('/v1/agents'),
  agent: (cid: number) => get<Agent>(`/v1/agents/${cid}`),
  spawnAgents: (role: string, count: number, model?: string) => post<unknown>('/v1/agents', { role, count, model }),
  setAgentModel: (cid: number, model: string | null) => patch<Agent>(`/v1/agents/${cid}`, { model }),
  renameAgent: (cid: number, name: string) => patch<Agent>(`/v1/agents/${cid}`, { name }),
  killAgent: (cid: number) => del<unknown>(`/v1/agents/${cid}`),
  chat: (cid: number, text: string) => post<{ work: number }>(`/v1/agents/${cid}/chat`, { text }),
  assign: (cid: number, task_id: string) => post<{ work: number }>(`/v1/agents/${cid}/assign`, { task_id }),
  cancel: (cid: number) => post<{ cancelled: boolean }>(`/v1/agents/${cid}/cancel`),
  stop: (cid: number) => post<unknown>(`/v1/agents/${cid}/stop`),
  transcript: (cid: number, after = 0, limit = 1000) => get<Entry[]>(`/v1/agents/${cid}/transcript?after=${after}&limit=${limit}`),
  /** The newest `last` entries, oldest first. The hub's `after` pages from the oldest, so a long log would hide its newest steps. */
  transcriptLast: (cid: number, last = 1000) => get<Entry[]>(`/v1/agents/${cid}/transcript?last=${last}`),

  tasks: () => get<{ tasks: Task[] }>('/v1/tasks').then((r) => r.tasks),
  task: (id: string) => get<Task>(`/v1/tasks/${id}`),
  createTask: (t: { goal: string; acceptance?: string; role?: string; budget?: number; assignee?: number }) => post<Task>('/v1/tasks', t),
  withdrawTask: (id: string) => del<Task>(`/v1/tasks/${id}`),

  board: () => get<{ posts: BoardPost[] }>('/v1/board').then((r) => r.posts),
  /** The board for one project (or the whole board), with its pause state. */
  boardView: (project?: string) => get<BoardView>(`/v1/board${query({ project })}`),
  /** The owner comments on a board post. */
  commentOnPost: (postId: string, text: string) => post<BoardComment>(`/v1/board/${postId}/comments`, { text }),
  /** The owner posts to the board. `mentions` are agent names or roles, and `all`. */
  postToBoard: (body: { body: string; subject?: string; project?: string; mentions?: string[] }) => post<BoardPost>('/v1/board/posts', body),
  /** Pauses (`on`) or resumes the deliveries of a project's board, or of the whole board when no project is given. */
  setBoardPause: (on: boolean, project?: string) =>
    post<{ project: string | null; on: boolean; changed: boolean; paused: boolean; paused_all: boolean; paused_projects: string[] }>('/v1/board/pause', { on, project }),
  /** The thread as a download: Markdown or JSON, for a project or the whole board. */
  boardExportUrl: (format: 'md' | 'json', project?: string) => `/v1/board/export${query({ format, project })}`,
  conversations: () => get<{ conversations: Conversation[] }>('/v1/conversations').then((r) => r.conversations),
  conversationsPage: (q: ConversationQuery = {}) => get<ConversationsPage>(`/v1/conversations${query({ limit: q.limit, before: q.before, q: q.q })}`),
  conversationThread: (a: number, b: number, q: ConversationQuery = {}) =>
    get<ThreadPage>(`/v1/conversations/${a}/${b}${query({ limit: q.limit, before: q.before, q: q.q })}`),
  motions: () => get<{ motions: Motion[] }>('/v1/motions').then((r) => r.motions),
  /** The owner's decision: `approve` ratifies the motion, `false` vetoes it. A blank `note` is sent as `null`, since the hub refuses an empty string. */
  decide: (id: string, approve: boolean, note?: string) => post<Motion>(`/v1/motions/${id}/decide`, { approve, note: note?.trim() ? note.trim() : null }),
  treasury: () => get<Treasury>('/v1/treasury'),
  /** Entries newest first. Pass the previous page's `next_before` to load older ones. */
  treasuryJournal: (q: JournalQuery = {}) =>
    get<JournalPage>(`/v1/treasury/journal${query({ before: q.before, limit: q.limit, kind: q.kind, wallet: q.wallet, task: q.task })}`),
  /** Flows per bucket over [from, to) in unix ms, empty buckets included. Day buckets above 2000 buckets are refused. */
  treasurySeries: (from: number, to: number, bucket: 'hour' | 'day') => get<TreasurySeries>(`/v1/treasury/series${query({ from, to, bucket })}`),
  treasuryWallets: () => get<Wallet[]>('/v1/treasury/wallets'),
  mintBudget: (amount: number) => post<unknown>('/v1/treasury/mint', { amount }),
  fineAgent: (cid: number, amount: number, memo: string) => post<unknown>('/v1/treasury/fine', { cid, amount, memo }),
  agora: () => get<{ calls: unknown[] }>('/v1/agora').then((r) => r.calls),

  profile: (cid: number) => get<AgentProfile>(`/v1/agents/${cid}/profile`),
  society: () => get<Society>('/v1/society'),
  /** The owner rates an agent (AP-072: the owner is the root of trust), value in [-1, 1]. */
  rateAgent: (cid: number, value: number) => post<Opinion>(`/v1/agents/${cid}/rating`, { value }),
  memory: (cid: number) => get<{ notes: MemoryNote[] }>(`/v1/agents/${cid}/memory`).then((r) => r.notes),
  memoryNote: (cid: number, slug: string) => get<MemoryDoc>(`/v1/agents/${cid}/memory/${encodeURIComponent(slug)}`),

  machine: () => get<Machine>('/v1/machine'),
  machines: () => get<MachineNode[]>('/v1/machines'),
  fleet: () => get<FleetResponse>('/v1/fleet'),
  runs: () => get<{ runs: Run[] }>('/v1/runs').then((r) => r.runs),
  run: (id: string) => get<Run & { vms: (Vm & { serial?: string })[] }>(`/v1/runs/${id}`),
  createRun: (n: number, role?: string) => post<Run>('/v1/runs', { n, vcpus: 1, memory_mib: 256, role }),
  killRun: (id: string) => del<Run>(`/v1/runs/${id}`),

  files: (kind?: 'deliverable' | 'shared') => get<Deliverable[]>(`/v1/files${kind ? `?kind=${kind}` : ''}`),
  projects: () => get<Project[]>('/v1/projects'),
  project: (id: string) => get<Project>(`/v1/projects/${id}`),
  approveProject: (id: string, note = '') => post<Project>(`/v1/projects/${id}/approve`, { note }),
  vetoProject: (id: string, stage: ProjectStage, reason: string) => post<Project>(`/v1/projects/${id}/veto`, { stage, reason }),
  git: () => get<{ configured: boolean; org: string }>('/v1/git'),
  gitRepos: () => get<GitRepo[]>('/v1/git/repos'),
  gitCreateRepo: (name: string, description: string) => post<GitRepo>('/v1/git/repos', { name, description }),
  gitBranches: (repo: string) => get<GitBranch[]>(`/v1/git/repos/${repo}/branches`),
  gitCommits: (repo: string, ref = '', limit = 30) => get<GitCommit[]>(`/v1/git/repos/${repo}/commits?ref=${encodeURIComponent(ref)}&limit=${limit}`),
  gitTree: (repo: string, ref = '', path = '') => get<GitEntry[]>(`/v1/git/repos/${repo}/tree?ref=${encodeURIComponent(ref)}&path=${encodeURIComponent(path)}`),
  gitFile: (repo: string, ref: string, path: string) => get<GitFile>(`/v1/git/repos/${repo}/file?ref=${encodeURIComponent(ref)}&path=${encodeURIComponent(path)}`),
  gitPulls: (repo: string, state: 'open' | 'closed' | 'all' = 'open', page = 1, limit = 50) => get<GitPull[]>(`/v1/git/repos/${repo}/pulls?state=${state}&page=${page}&limit=${limit}`),
  gitOpenPull: (repo: string, title: string, head: string, base: string, body = '') => post<GitPull>(`/v1/git/repos/${repo}/pulls`, { title, head, base, body }),
  gitPull: (repo: string, n: number) => get<GitPullDetail>(`/v1/git/repos/${repo}/pulls/${n}`),
  gitComment: (repo: string, n: number, body: string) => post<GitComment>(`/v1/git/repos/${repo}/pulls/${n}/comments`, { body }),
  gitSetPullState: (repo: string, n: number, state: 'open' | 'closed') => post<GitPull>(`/v1/git/repos/${repo}/pulls/${n}/state`, { state }),
  gitReview: (repo: string, n: number, event: 'APPROVE' | 'APPROVED' | 'REQUEST_CHANGES' | 'COMMENT', body: string) => post<unknown>(`/v1/git/repos/${repo}/pulls/${n}/review`, { event, body }),
  gitMerge: (repo: string, n: number, style: 'merge' | 'squash' | 'rebase' = 'merge') => post<unknown>(`/v1/git/repos/${repo}/pulls/${n}/merge`, { style }),
  loops: () => get<HarnessLoop[]>('/v1/loops'),
  startLoop: (cid: number, prompt: string, every_minutes: number, max_runs: number) => post<HarnessLoop>('/v1/loops', { cid, prompt, every_minutes, max_runs }),
  stopLoop: (id: string) => del<HarnessLoop>(`/v1/loops/${id}`),
  agentLimits: (cid: number) => get<HarnessLimits>(`/v1/agents/${cid}/limits`),
  agentControl: (cid: number, body: { action: 'compress' | 'steer' | 'extend_turns' | 'set_limits'; text?: string; turns?: number; max_turns?: number; compress_at?: number; keep_recent?: number }) => post<unknown>(`/v1/agents/${cid}/control`, body),
  laws: () => get<LawInfo[]>('/v1/laws'),
  lawRules: () => get<LawRules>('/v1/laws/rules'),
  createLawRule: (r: { title: string; source: string }) => post<LawRule>('/v1/laws/rules', r),
  updateLawRule: (id: string, r: { title: string; source: string }) => put<LawRule>(`/v1/laws/rules/${id}`, r),
  setLawRuleEnabled: (id: string, enabled: boolean) => post<LawRule>(`/v1/laws/rules/${id}/${enabled ? 'enable' : 'disable'}`),
  deleteLawRule: (id: string) => del<{ deleted: string }>(`/v1/laws/rules/${id}`),
  checkLaw: (source: string, id?: string) => post<LawCheck>('/v1/laws/check', id ? { source, id } : { source }),
  /** Switching a constitutional law off needs `confirm` set to its exact id (the API answers 409 without it). */
  setLawOverride: (law: string, disabled: boolean, reason: string, confirm?: string) => put<{ law: string; override: LawOverride | null }>(`/v1/laws/overrides/${law}`, { disabled, reason, confirm }),
  lawViolations: (law: string, limit = 20) => get<Infraction[]>(`/v1/laws/violations${query({ law, limit })}`),
  infractions: (limit = 200) => get<Infraction[]>(`/v1/infractions?limit=${limit}`),
  agentInfractions: (cid: number) => get<Infraction[]>(`/v1/agents/${cid}/infractions`),
  plazaState: () => get<PlazaState>('/v1/plaza/state'),
  stopAll: (reason = '') => post<PlazaState>('/v1/plaza/stop', { reason }),
  resumeAll: () => post<PlazaState>('/v1/plaza/resume', {}),
  restartAgent: (cid: number) => post<unknown>(`/v1/agents/${cid}/restart`, {}),
  wipeAgent: (cid: number, opts: { memory?: boolean; transcript?: boolean; identity?: boolean }) => post<{ ok: boolean; wiped: string[] }>(`/v1/agents/${cid}/wipe`, opts),
  retireAgent: (cid: number) => post<unknown>(`/v1/agents/${cid}/retire`, {}),
  /** Refused (409) while the agent works or has queued work. */
  sleepAgent: (cid: number) => post<{ ok: boolean }>(`/v1/agents/${cid}/sleep`),
  wakeAgent: (cid: number) => post<{ ok: boolean }>(`/v1/agents/${cid}/wake`),
  /** The agent's VM checkpoints, newest first. */
  vmCheckpoints: (cid: number) => get<VmCheckpointList>(`/v1/agents/${cid}/vm-checkpoints`),
  takeVmCheckpoint: (cid: number, label?: string) => post<VmCheckpoint>(`/v1/agents/${cid}/vm-checkpoints`, label ? { label } : {}),
  restoreVmCheckpoint: (cid: number, name: string) => post<VmRestoreResult>(`/v1/agents/${cid}/vm-checkpoints/${encodeURIComponent(name)}/restore`),
  deleteVmCheckpoint: (cid: number, name: string) => del<{ ok: boolean }>(`/v1/agents/${cid}/vm-checkpoints/${encodeURIComponent(name)}`),
  /** Forks the agent n times (1..8) from a fresh checkpoint of its VM. */
  forkAgent: (cid: number, n: number) => post<ForkResult>(`/v1/agents/${cid}/fork`, { n }),
  /** Moves the agent's VM to another node. 409 while it works or when the target is draining or revoked; it can take seconds. */
  moveAgent: (cid: number, node: string) => post<MoveResult>(`/v1/agents/${cid}/move`, { node }),
  roles: () => get<RoleView[]>('/v1/roles'),
  saveRole: (def: RoleDef) => post<RoleDef>('/v1/roles', def),
  deleteRole: (id: string) => del<unknown>(`/v1/roles/${id}`),
  agentProposals: () => get<AgentProposal[]>('/v1/agent-proposals'),
  approveAgentProposal: (id: string, edits: { role?: RoleDef; count?: number; model?: string } = {}) => post<AgentProposal>(`/v1/agent-proposals/${id}/approve`, edits),
  rejectAgentProposal: (id: string, reason: string) => post<AgentProposal>(`/v1/agent-proposals/${id}/reject`, { reason }),
  toolbox: () => get<Discovery[]>('/v1/toolbox'),
  removeDiscovery: (id: string) => del<Discovery>(`/v1/toolbox/${id}`),
  flagDiscovery: (id: string, reason: string | null) => post<Discovery>(`/v1/toolbox/${id}/flag`, { reason }),
  file: (id: string) => get<Deliverable>(`/v1/files/${id}`),
  /** The deliverable's bytes, inline (for previews) or as a download. */
  fileUrl: (id: string, download = false) => `/v1/files/${id}/content${download ? '?download=1' : ''}`,
  /** Card data for up to 50 references in one call, answered in order (the hub's `POST /v1/unfurl`). */
  unfurl: (refs: Ref[]) => post<{ cards: UnfurlAnswer[] }>('/v1/unfurl', { refs }),
  removeFile: (id: string) => del<unknown>(`/v1/files/${id}`),
  library: () => get<LibraryTool[]>('/v1/library'),
  libraryFile: (name: string, version: number, path: string) => get<{ path: string; content: string }>(`/v1/library/${name}/${version}/files?path=${encodeURIComponent(path)}`),
  reviewLibraryTool: (name: string, version: number, approve: boolean, note?: string) => post<LibraryTool>(`/v1/library/${name}/${version}/review`, { approve, note }),
  removeLibraryTool: (name: string, version: number) => del<unknown>(`/v1/library/${name}/${version}`),
  connectorProposals: () => get<ConnectorProposal[]>('/v1/connectors/proposals'),
  reviewConnectorProposal: (id: string, approve: boolean, note?: string) => post<ConnectorProposal>(`/v1/connectors/proposals/${id}/review`, { approve, note }),
  connectors: () => get<Connector[]>('/v1/connectors'),
  updateConnector: (id: string, patch: { enabled?: boolean; roles?: string[] }) => put<Connector>(`/v1/connectors/${id}`, patch),
  /** Stores one credential field in the vault (`field` omitted: the connector's main key). */
  setConnectorSecret: (id: string, value: string, field?: string) => put<unknown>(`/v1/connectors/${id}/secret`, field ? { field, value } : { value }),
  /** Removes one field from the vault (omitted: every field of the connector). */
  clearConnectorSecret: (id: string, field?: string) => del<unknown>(`/v1/connectors/${id}/secret${field ? `?field=${encodeURIComponent(field)}` : ''}`),
  testConnector: (id: string) => post<ConnectorTest>(`/v1/connectors/${id}/test`),

  approvals: () => get<Approval[]>('/v1/approvals'),
  /** `action` picks what a sanction escalation does; left out, approve and deny act as they always have. */
  decideApproval: (id: string, approve: boolean, reason?: string, action?: ApprovalAction) => post<unknown>(`/v1/approvals/${id}`, { approve, reason, action }),
  /** The sanctions register, newest first. `cid`, `law` and `limit` (default 100) narrow it. `active` answers only the suspensions in force (no sanctions, no limit). */
  sanctions: (q: { cid?: number; law?: string; limit?: number; active?: boolean } = {}) =>
    get<SanctionsView>(`/v1/sanctions${query({ cid: q.cid, law: q.law, limit: q.limit, active: q.active ? 1 : undefined })}`),
  /** Lifts an active suspension. 404 for an unknown id, 409 when it is not an active suspension. */
  liftSanction: (id: number, reason: string) => post<Sanction>(`/v1/sanctions/${id}/lift`, { reason }),
  permissions: (cid: number) => get<{ mode: PermissionMode }>(`/v1/agents/${cid}/permissions`),
  setPermissions: (cid: number, mode: PermissionMode) => put<{ mode: PermissionMode }>(`/v1/agents/${cid}/permissions`, { mode }),

  /** 200 when this browser may use the console (loopback, or signed in); 401 otherwise. */
  authCheck: () => get<{ ok: boolean }>('/v1/auth/check'),
  /** Trades the owner token for this browser's session cookie (HttpOnly; the token is not kept). */
  signIn: (token: string) => post<{ ok: boolean }>('/v1/auth/session', { token }),
  signOut: () => del<{ ok: boolean }>('/v1/auth/session'),
  hubIdentity: () => get<HubIdentity>('/v1/hub/identity'),
  /** A new owner token, shown once; every signed-in browser must sign in again. */
  rotateOwnerToken: () => post<{ token: string }>('/v1/hub/owner-token/rotate'),

  nodes: () => get<NodeView[]>('/v1/nodes'),
  /** A join code: its token is in this answer only. `ttl_minutes` ≤ 60 (0: the default 15). */
  issueJoinCode: (label: string, ttl_minutes: number) => post<{ id: string; token: string; expires_ms: number }>('/v1/nodes/codes', { label, ttl_minutes }),
  joinCodes: () => get<JoinCode[]>('/v1/nodes/codes'),
  revokeJoinCode: (id: string) => del<unknown>(`/v1/nodes/codes/${id}`),
  revokeNode: (id: string) => post<NodeView>(`/v1/nodes/${id}/revoke`),
  drainNode: (id: string, on: boolean) => post<NodeView>(`/v1/nodes/${id}/drain`, { on }),
  nodeMachine: (id: string) => get<Machine>(`/v1/nodes/${id}/machine`),
  nodeRuns: (id: string) => get<unknown>(`/v1/nodes/${id}/runs`),
  nodeLink: () => get<NodeLink>('/v1/node-link'),
  /** Joins the network whose Nodes screen issued `code`. 400 `{error}` on a bad code, 409 when already joined. */
  joinNodeLink: (code: string) => post<NodeLink>('/v1/node-link/join', { code }),
  /** Leaves that network; `forget` also drops its saved hub details. */
  leaveNodeLink: (forget: boolean) => post<NodeLink>('/v1/node-link/leave', { forget }),

  /** A join code for a workstation: its token is in this answer only. `ttl_minutes` ≤ 60 (0: the default 15). */
  issueStationCode: (label: string, ttl_minutes: number) => post<{ id: string; token: string; expires_ms: number }>('/v1/stations/codes', { label, ttl_minutes }),
  stationCodes: () => get<StationCode[]>('/v1/stations/codes'),
  revokeStationCode: (id: string) => del<unknown>(`/v1/stations/codes/${encodeURIComponent(id)}`),
  stations: () => get<StationView[]>('/v1/stations'),
  /** The workstation VMs the plaza made on its nodes. */
  stationVms: () => get<StationVm[]>('/v1/stations/vms'),
  createStationVm: (body: StationVmBody) => post<StationVm>('/v1/stations/vms', body),
  /** Removes the VM from its node; the station it enrolled as is revoked. */
  deleteStationVm: (id: string) => del<StationVm>(`/v1/stations/vms/${seg(id)}`),
  /** Keeps the plaza VM's memory and stops it; it wakes in a fraction of a second. */
  sleepStationVm: (id: string) => post<{ state: string }>(`/v1/stations/vms/${seg(id)}/sleep`, {}),
  wakeStationVm: (id: string) => post<{ state: string }>(`/v1/stations/vms/${seg(id)}/wake`, {}),
  /** A workstation's latest picture as an object URL (the caller revokes it), or null when it cannot be had. */
  stationThumbnailUrl: async (id: string): Promise<string | null> => {
    try {
      const res = await fetch(`/v1/stations/${encodeURIComponent(id)}/thumbnail`);
      if (res.status === 401) window.dispatchEvent(new CustomEvent('apz-auth-required'));
      if (!res.ok) return null;
      return URL.createObjectURL(await res.blob());
    } catch {
      return null;
    }
  },
  station: (id: string) => get<StationView>(`/v1/stations/${encodeURIComponent(id)}`),
  renameStation: (id: string, name: string) => patch<StationView>(`/v1/stations/${encodeURIComponent(id)}`, { name }),
  drainStation: (id: string, on: boolean) => post<StationView>(`/v1/stations/${encodeURIComponent(id)}/${on ? 'drain' : 'undrain'}`),
  revokeStation: (id: string) => post<StationView>(`/v1/stations/${encodeURIComponent(id)}/revoke`),
  /** Stores the settings and pushes them to the workstation when it is linked (`pushed`, `push_error`). */
  saveStationSettings: (id: string, settings: StationSettings) => put<StationSettingsAnswer>(`/v1/stations/${encodeURIComponent(id)}/settings`, settings),
  releaseStationLease: (id: string, lease: string) => del<unknown>(`/v1/stations/${encodeURIComponent(id)}/leases/${encodeURIComponent(lease)}`),
  /** The newest rows first; with `station` only that workstation's. */
  stationAudit: (station: string, limit = 100) => get<StationAuditRow[]>(`/v1/stations/audit${query({ station, limit })}`),

  /** One picture of a station's screen (its session, or the lease's when `session` is left out); null when none is open. */
  stationThumbnail: (id: string, session?: string) =>
    raw('GET', `/v1/stations/${seg(id)}/thumbnail${session ? `?session=${encodeURIComponent(session)}` : ''}`).then((r) => r.blob()).catch((): null => null),
  /** The work folder's state, and the mirror's files under `prefix` (a directory with its trailing slash, or none for all). */
  stationSync: (id: string) => get<StationSync>(`/v1/stations/${seg(id)}/sync`),
  stationSyncFiles: (id: string, prefix = '') => get<SyncFile[]>(`/v1/stations/${seg(id)}/files${query({ prefix: prefix || undefined })}`),
  /** The file's bytes, for a download (the page's cookie signs the request). */
  stationFileUrl: (id: string, path: string) => filesUrl(id, path),
  /** Writes a file into the mirror; the next pass pushes it to the station. */
  putStationFile: (id: string, path: string, body: Blob) => raw('PUT', filesUrl(id, path), body, 'application/octet-stream').then((r) => r.json() as Promise<SyncFile>),
  deleteStationFile: (id: string, path: string) => del<SyncFile>(filesUrl(id, path)),
  /** Copies a mirrored file into the Library as a deliverable. */
  deliverStationFile: (id: string, path: string, body: { title?: string; description?: string }) => post<StationDelivery>(`${filesUrl(id, path)}/deliver`, body),
  /** Resumes a paused sync; `applyDeletions` carries out the deletions it held. */
  resumeStationSync: (id: string, applyDeletions: boolean) => post<StationSync>(`/v1/stations/${seg(id)}/sync/resume`, { apply_deletions: applyDeletions }),
  /** The newest recordings first; `station` narrows them to one workstation, `before` pages back from a start in ms. */
  stationRecordings: (q: { station?: string; limit?: number; before?: number } = {}) =>
    get<StationRecording[]>(`/v1/stations/recordings${query({ station: q.station, limit: q.limit, before: q.before })}`),
  stationRecording: (id: string) => get<StationRecordingDetail>(`/v1/stations/recordings/${seg(id)}`),
  /** One frame's JPEG, seq counting from 1. */
  stationRecordingFrameUrl: (id: string, seq: number) => `/v1/stations/recordings/${seg(id)}/frames/${seq}`,
  deleteStationRecording: (id: string) => del<{ id: string; deleted: boolean }>(`/v1/stations/recordings/${seg(id)}`),
  stationRecordingDefaults: () => get<RecordingDefaults>('/v1/stations/recording-defaults'),
  saveStationRecordingDefaults: (d: RecordingDefaults) => put<RecordingDefaults>('/v1/stations/recording-defaults', d),

  /** The model-call ledger, totalled per model, agent, role and provider over [from, to) unix ms (default: the last 30 days). */
  usageSummary: (from?: number, to?: number) => get<UsageSummary>(`/v1/usage/summary${query({ from, to })}`),
  /** Buckets of the window, empty ones included. `by` picks the grouping key. */
  usageSeries: (from: number, to: number, bucket: UsageBucket, by: UsageGroup) => get<UsageSeries>(`/v1/usage/series${query({ from, to, bucket, by })}`),
  /** Calls newest first. Pass the previous page's `next_before` to load older calls. */
  usageCalls: (q: { before?: number; limit?: number; model?: string; cid?: number }) => get<UsageCallsPage>(`/v1/usage/calls${query({ ...q })}`),
  /** Every known model price (USD per million tokens). Large: prefer `price` for the few models in use. */
  prices: () => get<PricesView>('/v1/prices'),
  /** One model's price. Rejects with an ApiError (404) when no source prices the model. */
  price: (model: string) => get<ModelPrice>(`/v1/prices/${encodeURIComponent(model)}`),
  /** Fetches the price sources again now. */
  refreshPrices: () => post<unknown>('/v1/prices/refresh'),
};

// ---- Usage ledger ----------------------------------------------------------

export type UsageBucket = 'hour' | 'day';
export type UsageGroup = 'model' | 'agent' | 'role' | 'provider';

/** Token counts and money for a set of calls. `usd` is the provider-reported cost where known, else `est_usd`. */
export interface UsageTotals {
  calls: number; input: number; output: number; cache_read: number; cache_write: number;
  cost_usd: number; est_usd: number; usd: number; cache_hit_rate: number;
}
export interface UsageKeyed { key: string; totals: UsageTotals }
export interface UsageSummary {
  from_ms: number; to_ms: number; totals: UsageTotals;
  by_model: UsageKeyed[]; by_agent: UsageKeyed[]; by_role: UsageKeyed[]; by_provider: UsageKeyed[];
  /** Models that were called but have no price, so their estimate is zero. */
  unpriced: string[];
}
export interface UsagePoint { at_ms: number; groups: Record<string, UsageTotals>; totals: UsageTotals }
export interface UsageSeries { from_ms: number; to_ms: number; bucket_ms: number; by: UsageGroup; points: UsagePoint[] }
export interface UsageCall {
  id: number; at_ms: number; cid: number; role: string; model: string; provider: string;
  input: number; output: number; cache_read: number; cache_write: number;
  cost_usd: number | null; est_usd: number; task: string | null; via: string;
}
export interface UsageCallsPage { calls: UsageCall[]; total: number; next_before: number | null }

// ---- Prices ----------------------------------------------------------------

/** USD per million tokens. `cache_read`/`cache_write` are null when the source does not price them. */
export interface PriceRates { input: number; output: number; cache_read: number | null; cache_write: number | null; source: string }
export interface ModelPrice { model: string; price: PriceRates }
export interface PriceSource { name: string; url: string; models: number; ok: boolean; error?: string }
export interface PricesView { fetched_at_ms: number; sources: PriceSource[]; models: ModelPrice[] }

/** This hub as a node of another network. `cids` is the CID range its agents hold there, "first-last" (null until joined); `pid` is set while `running`. */
export interface NodeLink { joined: boolean; hub: string | null; node_id: string | null; cids: string | null; running: boolean; pid: number | null }

/** What the hub says about itself: anyone may ask. `fingerprint` is its TLS certificate's SHA-256 (base64url), null without TLS. */
export interface HubIdentity { name: string; version: string; url: string; fingerprint: string | null; nodes: number }
/** A machine in the plaza: `n0` is the hub's own. Agents on it have CIDs `cid_base` … `cid_base + cid_count - 1`. */
export interface NodeView {
  id: string; name: string; public_key: string; cid_base: number; cid_count: number;
  joined_ms: number; last_seen_ms: number;
  /** The machine summary the node reported. `workstation_backends` lists `plaza`, and `qemu` when QEMU and its firmware are here. */
  machine: { cpus?: number; memory_mib?: number; host?: string; version?: string; workstation_backends?: string[]; vendor?: string | null; apple?: unknown } | null;
  draining: boolean; revoked: boolean; address: string; online: boolean; agents: number;
}
export type JoinCodeState = 'active' | 'used' | 'expired' | 'revoked';
export interface JoinCode { id: string; label: string; created_ms: number; expires_ms: number; state: JoinCodeState; used_by: string | null }

export type ApprovalState = 'pending' | 'approved' | 'denied' | 'used' | 'withdrawn';
/**
 * A call waiting on (or recording) the owner's decision. `kind` is "tool" (an agent's tool call), "sanction"
 * (an escalation from the conduct laws) or "payment" (a held payment). For a tool call `subject` is the tool
 * and `detail` its input; a sanction's `detail` carries `sanction_id`.
 */
export interface Approval {
  id: string; cid: number; tool: string; input: unknown; created_ms: number; state: ApprovalState; reason: string | null;
  kind: string; subject: string; detail: unknown;
}
/** What the owner does on a sanction escalation: acknowledge it, lift the suspension, or record a retirement. */
export type ApprovalAction = 'acknowledge' | 'lift' | 'retire';

/** A sanction on the register. `lifted` is set once the owner has lifted it; `owner_action` is the owner's latest decision on the record. */
export interface Sanction {
  id: number; cid: number; law: string; title: string; severity: string; count: number; tool: string; at_ms: number;
  sanction: string; outcome: string; fine: number; entry: number | null; until_ms: number | null;
  lifted: { at_ms: number; by: 'owner'; why: string } | null; approval: string | null;
  owner_action: { action: 'acknowledge' | 'retire'; at_ms: number } | null;
}
/** A suspension: the agent may not use `tool` until `until_ms`. `active` is false once it has run out or been lifted. */
export interface Suspension { id: number; cid: number; tool: string; law: string; until_ms: number; active: boolean }
export interface SanctionsView { sanctions: Sanction[]; suspensions: Suspension[] }
/** `autonomous` acts freely, `ask` queues changes for approval, `read_only` refuses them. */
export type PermissionMode = 'autonomous' | 'ask' | 'read_only';

// ---- Foundry (the Lab) -----------------------------------------------------
// The training lab's apid routes (`/v1/foundry/...`): recipes, datasets, the device fleet, jobs and
// the model registry. Live progress arrives on the console socket as `training` frames (see
// `views/lab-common.ts`). A non-2xx answer throws an `ApiError` carrying the hub's `error` text.

export interface Recipe {
  id: string; name: string; repo: string; commit: string; schema: unknown;
  vetted_by: string | null; vetted_ms: number | null; local_dev: boolean;
}

export type DatasetKind = 'cascade' | 'tasks' | 'usage' | 'upload' | 'all';
export interface Dataset {
  id: string; name: string; kind: DatasetKind; created_ms: number; from_ms: number; to_ms: number;
  rows: number; frozen_rows: number; label_balance: Record<string, number>; taint: string[];
  provenance: Record<string, unknown>; file: string; frozen_file: string; sha256: string;
}
/** An export from the plaza's records; `since_ms` is inclusive and `until_ms` exclusive, both on a row's `ts`. */
export interface DatasetSpec { kind: Exclude<DatasetKind, 'upload'>; name: string; since_ms?: number; until_ms?: number }

export interface Device {
  index: number; vendor: string; name: string; backend: string; memory_mib: number; mem_used_mib: number;
  busy: boolean; util_pct: number; allocated_to: string | null;
}
export interface DeviceMachine { id: string; online: boolean; trainer_ok: boolean; devices: Device[] }
export interface DevicesView { machines: DeviceMachine[] }

export type FoundryMode = 'single' | 'tight' | 'loose';
/** The job document's `devices` block: what placement may use. Unknown keys are refused. */
export interface DeviceRequest {
  min_devices?: number; max_devices?: number; min_memory_mib?: number; backends?: string[];
  mode?: FoundryMode; machines?: string[]; allow_cpu?: boolean;
}
/** What the New job form sends. `recipe` is `name@version`. */
export interface JobDocument {
  job_id?: string; recipe: string; datasets: string[]; search?: unknown; budget_gpu_hours: number;
  target?: 'x86_64' | 'aarch64' | 'any'; export?: 'onnx'; devices?: DeviceRequest; mode?: FoundryMode;
  train?: Record<string, unknown>;
}

export type JobStatus = 'queued' | 'running' | 'done' | 'failed' | 'cancelled';
export interface JobSummary {
  id: string; status: JobStatus; mode: FoundryMode; recipe: string; datasets: string[];
  created_ms: number; updated_ms: number; gpu_hours: number; budget_gpu_hours: number;
  error: string | null; runs: number;
}
export interface JobIsland { id: string; backend: string; leader: string; rdzv: string; machines: { id: string; devices: number[] }[] }
export interface JobPlan {
  mode: FoundryMode; islands: JobIsland[]; inner_steps: number; staleness: number; devices_total: number;
  datasets: Record<string, string>; frozen: string | null; output_dir: string; train: Record<string, unknown>; rounds?: number;
}
export interface JobRun {
  node_job: string; node: string; island: string; devices: number[]; started_ms: number; ended_ms: number | null;
  state: string; exit_code: number | null; next: number; gpu_hours: number;
}
/** One point of a job's series. `t` is the time it was recorded, ms. */
export interface SeriesPoint {
  kind: 'step' | 'eval' | 'round' | 'trial'; island: string; t: number;
  step?: number; loss?: number; lr?: number; samples?: number; split?: string; metrics?: Record<string, number>;
  round?: number; accepted?: boolean; hub_round?: number; trial?: number; objective?: number;
}
/** A job in full. Its `runs` are the run records (the summary's `runs` is their count). */
export interface JobView extends Omit<JobSummary, 'runs'> {
  plan: JobPlan | null; runs: JobRun[]; series: SeriesPoint[]; metrics: Record<string, unknown> | null;
  outputs: string[]; outputs_remote: boolean; seq: number;
}
/** One stored event: `seq` orders them, `event` is the trainer's own line (its name is `ev`). */
export interface JobEvent { seq: number; island: string; node: string; event: { ev: string } & Record<string, unknown> }
export interface JobEventsPage { job: string; events: JobEvent[]; next: number }

export type ModelState = 'trained' | 'shadow' | 'promoted' | 'retired';
export interface ModelCard {
  id: string; name: string; version: string; input: unknown; output: unknown; features: unknown[];
  labels: string[] | null; allowed_roles: string[]; onnx_sha256: string; created_ms: number; metrics: unknown;
}
export interface ShadowStats { runs: number; agreed: number; cost_saved_usd: number; updated_ms: number }
export interface ModelView {
  id: string; state: ModelState; card: ModelCard; source_job: string | null; agent_cid: number | null;
  provenance: unknown; calibration: unknown; size_bytes: number; parameters: number; shadow: ShadowStats;
  agreement_rate: number | null; created_ms: number; state_changed_ms: number;
}

/** A refused dispatch: the job is recorded as `failed` and comes back with the refusal. */
export class JobDispatchError extends ApiError {
  constructor(message: string, public job: JobView) {
    super(422, message);
  }
}

// `@` stays raw in a segment (RFC 3986 allows it): a recipe id is `name@version`, and apid reads the raw form.
const foundryPath = (...parts: (string | number)[]): string => '/v1/foundry/' + parts.map((p) => encodeURIComponent(String(p)).replace(/%40/g, '@')).join('/');

/** A request with a raw body (the JSONL upload) or none; returns the status and the parsed answer. */
async function foundryFetch(method: string, path: string, body?: string, type = 'application/json'): Promise<{ ok: boolean; status: number; data: unknown }> {
  const res = await fetch(path, { method, headers: body === undefined ? {} : { 'content-type': type }, body });
  const text = await res.text();
  let data: unknown = null;
  try {
    data = text ? (JSON.parse(text) as unknown) : null;
  } catch {
    data = text || null;
  }
  if (res.status === 401) window.dispatchEvent(new CustomEvent('apz-auth-required'));
  return { ok: res.ok, status: res.status, data };
}

/** The answer of a request that must succeed: its JSON, or an `ApiError` with the hub's `error`. */
async function foundryOk<T>(method: string, path: string, body?: unknown, type?: string): Promise<T> {
  const raw = body === undefined || typeof body === 'string' ? body : JSON.stringify(body);
  const reply = await foundryFetch(method, path, raw, type);
  if (!reply.ok) {
    const error = (reply.data as { error?: string } | null)?.error ?? `${method} ${path}: ${reply.status}`;
    throw new ApiError(reply.status, error);
  }
  return reply.data as T;
}

// Recipes

export const listRecipes = () => foundryOk<Recipe[]>('GET', foundryPath('recipes'));
/** The owner vets a commit of a recipe: 400 unless `commit` is 7 to 64 hex characters (or `dev-...`), 403 on a local-dev pin mismatch. */
export const vetRecipe = (id: string, commit: string) => foundryOk<Recipe>('POST', foundryPath('recipes', id, 'vet'), { commit });

// Datasets

export const listDatasets = () => foundryOk<Dataset[]>('GET', foundryPath('datasets'));
export const createDataset = (spec: DatasetSpec) => foundryOk<Dataset>('POST', foundryPath('datasets'), spec);
/** Imports a JSONL body as a dataset. Each row needs `ts` (unix ms), `label` and the recipe's feature fields. */
export const uploadDataset = (name: string, jsonl: string) =>
  foundryOk<Dataset>('POST', `${foundryPath('datasets', 'upload')}?name=${encodeURIComponent(name)}`, jsonl, 'application/x-ndjson');

// Devices and planning

export const listDevices = () => foundryOk<DevicesView>('GET', foundryPath('devices'));
/** The dry run: the plan a job document would get. Nothing is written or dispatched. */
export const planJob = (doc: JobDocument) => foundryOk<JobPlan>('POST', foundryPath('plan'), doc);

// Jobs

/**
 * Submits a job document and dispatches it. Throws a `JobDispatchError` (422) when the job was recorded
 * but its dispatch failed; the job is on the error. Any other refusal is a plain `ApiError`.
 */
export async function submitJob(doc: JobDocument): Promise<JobView> {
  const reply = await foundryFetch('POST', foundryPath('jobs'), JSON.stringify(doc));
  if (reply.status === 422) {
    const body = reply.data as { error?: string; job: JobView };
    throw new JobDispatchError(body.error ?? 'dispatch failed', body.job);
  }
  if (!reply.ok) throw new ApiError(reply.status, (reply.data as { error?: string } | null)?.error ?? `POST jobs: ${reply.status}`);
  return reply.data as JobView;
}
/** Every job, newest first, without the series. */
export const listJobs = () => foundryOk<JobSummary[]>('GET', foundryPath('jobs'));
export const getJob = (id: string) => foundryOk<JobView>('GET', foundryPath('jobs', id));
/** The job's stored events after sequence `after`; keep `next` for the following call. */
export const jobEvents = (id: string, after = 0) => foundryOk<JobEventsPage>('GET', `${foundryPath('jobs', id, 'events')}?after=${after}`);
export const cancelJob = (id: string) => foundryOk<JobView>('POST', foundryPath('jobs', id, 'cancel'));

// Models

export const listModels = () => foundryOk<{ models: ModelView[] }>('GET', foundryPath('models')).then((r) => r.models);
export const getModel = (id: string) => foundryOk<ModelView>('GET', foundryPath('models', id));
/** Registers the model in a directory holding `model.onnx` and `model_card.json`; `path` is absolute. */
export const importModel = (path: string) => foundryOk<ModelView>('POST', foundryPath('models', 'import'), { path });
export const shadowModel = (id: string) => foundryOk<ModelView>('POST', foundryPath('models', id, 'shadow'));
export const promoteModel = (id: string) => foundryOk<ModelView>('POST', foundryPath('models', id, 'promote'));
export const retireModel = (id: string) => foundryOk<ModelView>('POST', foundryPath('models', id, 'retire'));

// ---- Vault (docs/design/vault.md, "Routes") --------------------------------
// Owner auth. Values go in on write and never come back: no call returns a secret value.

export type VaultState = 'uninitialized' | 'sealed' | 'unsealed' | 'tampered';
export type VaultSlotKind = 'passphrase' | 'keyfile' | 'recovery';
export interface VaultSlot {
  id: string; kind: VaultSlotKind; created_ms: number;
  /** True for the slot that unlocked this session. A rekey does not ask for its secret. */
  unsealed_with?: boolean;
}
/** `id` is `n0` for this hub, else the node's id. */
export interface VaultNodeRef { id: string; name: string }
export interface VaultStatus {
  state: VaultState;
  slots: VaultSlot[];
  entries: number;
  nodes: VaultNodeRef[];
  /** `null` until the audit chain has been checked (the first unlock or audit read). */
  audit_ok: boolean | null;
  rekeyed_ms: number | null;
  /** The reason the vault is tampered (state `tampered`), else null. */
  tampered: string | null;
  /** Keys still in the env file that the vault has not taken (the deprecation banner). Not yet served by apid. */
  env_file_keys?: number;
}
export type VaultKind = 'api_key' | 'token' | 'password' | 'generic';
export type VaultScope = { kind: 'plaza' } | { kind: 'nodes'; nodes: string[] };
export interface VaultVersion { v: number; created_ms: number; fp: string; hint: string }
export interface VaultEntry {
  id: string; name: string; kind: VaultKind; label: string; created_ms: number; updated_ms: number; created_by: string;
  scope: VaultScope; bindings: string[]; current: number; versions: VaultVersion[];
  /** 0 when never used. */
  last_used_ms: number; use_count: number;
}
export interface VaultCreate { name: string; kind: VaultKind; label?: string; value: string; scope: VaultScope; bindings: string[] }
export interface VaultPatch { label?: string; scope?: VaultScope; bindings?: string[] }
/** Per node (`n0` and the node ids): the entry id that resolves, `missing`, or `out_of_scope`. */
export interface VaultConsumer { binding: string; label: string; wants: string; per_node: Record<string, string> }
export interface VaultTest { ok: boolean; detail: string }
export interface VaultImportBody { format: 'dotenv'; text: string; scope: VaultScope; bindings?: Record<string, string[]> }
export interface VaultImportResult { imported: { name: string; id: string }[]; skipped: { name: string; reason: string }[] }
export interface VaultAuditLine {
  seq: number; ts_ms: number; event: string; entry?: string; name?: string; actor: string;
  binding?: string; node?: string; cid?: number; detail?: string; prev: string; mac: string;
}
/** The first line that breaks the chain: its 0-based position, its seq when it has one, and 12-character hash prefixes. */
export interface VaultBrokenEntry { index: number; seq?: number; expected: string; found: string }
export interface VaultAudit { entries: VaultAuditLine[]; chain_ok?: boolean; first_broken?: VaultBrokenEntry | null; verified?: number | null }

const vaultPath = (...parts: string[]) => ['/v1/vault', ...parts.map((p) => encodeURIComponent(p))].join('/');

export const vaultApi = {
  status: () => get<VaultStatus>('/v1/vault'),
  /** `{}` tries the key file; otherwise a passphrase or a recovery code (not both). */
  unseal: (body: { passphrase?: string; recovery_code?: string } = {}) => post<VaultStatus>('/v1/vault/unseal', body),
  seal: () => post<VaultStatus>('/v1/vault/seal'),
  addPassphrase: (passphrase: string) => post<VaultStatus>('/v1/vault/slots', { kind: 'passphrase', passphrase }),
  addKeyfile: () => post<VaultStatus>('/v1/vault/slots', { kind: 'keyfile' }),
  /** 409 when it is the last slot. */
  removeSlot: (id: string) => del<VaultStatus>(vaultPath('slots', id)),
  /** Shown once: the caller must not keep it. Replaces the old recovery slot. */
  recovery: () => post<{ recovery_code: string }>('/v1/vault/recovery'),
  /** `secrets` maps each OTHER passphrase/recovery slot id to its passphrase or code. */
  rekey: (secrets: Record<string, string> = {}) => post<VaultStatus>('/v1/vault/rekey', { secrets }),
  entries: () => get<VaultEntry[]>('/v1/vault/secrets'),
  createEntry: (body: VaultCreate) => post<VaultEntry>('/v1/vault/secrets', body),
  /** A new version of the value. */
  rotateEntry: (id: string, value: string) => put<VaultEntry>(vaultPath('secrets', id, 'value'), { value }),
  patchEntry: (id: string, body: VaultPatch) => patch<VaultEntry>(vaultPath('secrets', id), body),
  rollbackEntry: (id: string, v: number) => post<VaultEntry>(vaultPath('secrets', id, 'rollback'), { v }),
  /** Leaves a tombstone, so an env import never brings the name back. */
  deleteEntry: (id: string) => del<unknown>(vaultPath('secrets', id)),
  testEntry: (id: string, node?: string) => post<VaultTest>(vaultPath('secrets', id, 'test'), node ? { node } : {}),
  importEnv: (body: VaultImportBody) => post<VaultImportResult>('/v1/vault/import', body),
  consumers: () => get<VaultConsumer[]>('/v1/vault/consumers'),
  audit: (after = 0, limit = 100, verify = false) => get<VaultAudit>(`/v1/vault/audit?after=${after}&limit=${limit}${verify ? '&verify=1' : ''}`),
};

// ---- MCP servers (docs/design/mcp.md, "Routes") ----------------------------

export type McpTransport =
  | { kind: 'stdio'; image: string; command?: string[]; workdir?: string }
  | { kind: 'http'; url: string };
/** A vault secret by name, or a plain literal. A header's `format` wraps the secret, e.g. "Bearer {}". */
export type McpSetting = { secret: string; format?: string } | { value: string };
export type McpRisk = 'read' | 'write' | 'irreversible';
/** `"all"` or a list of names. */
export type McpPick = 'all' | string[];
export interface McpServerConfig {
  id: string; name: string; enabled: boolean; transport: McpTransport;
  env: Record<string, McpSetting>; headers: Record<string, McpSetting>;
  tools: { allow: McpPick; risk: Record<string, McpRisk> };
  access: { roles: McpPick; nodes: McpPick };
  limits: { timeout_s: number; max_output_kb: number; idle_stop_min: number };
  allow_private: boolean; storage: { volume: boolean };
  created_ms: number; updated_ms: number;
}
/** `starting` is in the design but apid does not report it yet (a start shows `running` or `stopped`). */
export type McpStatus = 'stopped' | 'starting' | 'running' | 'error';
export interface McpServerInfo { name: string; version: string; protocol: string }
/** The config, flattened, with the status over its instances. */
export interface McpServerView extends McpServerConfig {
  status: McpStatus; tools_count: number; last_error: string | null; last_used_ms: number; server_info: McpServerInfo | null;
}
export interface McpToolRow { name: string; tool: string; description: string; input_schema: unknown; annotations: unknown; risk: McpRisk; allowed: boolean }
export interface McpProbeTool { name: string; agent_name: string; description: string; annotations: unknown; auto_risk: McpRisk; risk: McpRisk }
export interface McpLogLine { ts_ms: number; stream: 'stdout' | 'stderr'; line: string }
/** `ok: false` comes back as a 200 with `error` and the stderr tail. */
export interface McpTestResult {
  ok: boolean; ms: number; server_info?: McpServerInfo; tools?: McpProbeTool[]; resources?: unknown[]; prompts?: unknown[];
  error?: string; stderr?: McpLogLine[];
}
export interface McpSpec { name: string; secret: boolean; required: boolean; help: string; format: string | null }
export interface McpPreset {
  id: string; name: string; description: string; transport: 'stdio' | 'http'; image: string | null; command: string[];
  url: string | null; env: McpSpec[]; headers: McpSpec[]; docs: string; status: string; note: string;
}

/** `POST /v1/agents/{cid}/move`: the node the VM left and the one it reached; `bytes` moved and `ms` taken. */
export interface MoveResult { ok: boolean; from: string; to: string; bytes: number; ms: number }

// ---- Egress credentials and the E2B API (owner screens) ---------------------

/**
 * A rule the hub uses to add a header to the agent's HTTPS requests to `host`. The secret is a vault secret's name
 * (bound to the consumer `egress`); the header value is `format` with `${SECRET}` replaced by the secret. The VM
 * never sees the secret. Empty `roles` or `methods` mean every role or every method.
 */
export interface EgressRule {
  id: string; roles: string[]; host: string; port: number; methods: string[]; path_prefix: string;
  header: string; format: string; secret: string; enabled: boolean; created_ms: number;
}
/** What a new rule or a change to one carries. The server fills the defaults a body leaves out. */
export type EgressRuleBody = Omit<EgressRule, 'id' | 'created_ms'>;

export interface E2bKey { id: string; label: string; created_ms: number }
/** A new key: `key` is shown once, in this answer only. */
export interface E2bKeyCreated extends E2bKey { key: string }
/**
 * A sandbox on the hub's E2B gateway. `cid` and `run` say where it runs (the agent's VM, when it is one); the
 * timestamps are RFC 3339 strings.
 */
export interface E2bSandbox {
  sandboxID: string; templateID: string; state: 'running' | 'paused'; startedAt: string; endAt: string;
  cpuCount: number; memoryMB: number; metadata: Record<string, string>; cid: number | null; run: string | null;
  allowInternetAccess: boolean | null; autoPause: boolean;
}

/** The egress credential rules (`/v1/egress/inject`). `rules` returns the list; a failure throws an `ApiError`. */
export const egressApi = {
  rules: () => get<{ rules: EgressRule[] }>('/v1/egress/inject').then((r) => r.rules),
  addRule: (rule: EgressRuleBody) => post<EgressRule>('/v1/egress/inject', rule),
  patchRule: (id: string, change: Partial<EgressRuleBody>) => patch<EgressRule>(`/v1/egress/inject/${seg(id)}`, change),
  deleteRule: (id: string) => del<{ ok: boolean }>(`/v1/egress/inject/${seg(id)}`),
};

/** The owner's E2B API: its keys (`/v1/e2b/keys`) and the sandboxes the gateway runs (`/v1/e2b/sandboxes`). */
export const e2bApi = {
  keys: () => get<{ keys: E2bKey[] }>('/v1/e2b/keys').then((r) => r.keys),
  createKey: (label: string) => post<E2bKeyCreated>('/v1/e2b/keys', { label }),
  revokeKey: (id: string) => del<{ ok: boolean }>(`/v1/e2b/keys/${seg(id)}`),
  sandboxes: () => get<{ sandboxes: E2bSandbox[] }>('/v1/e2b/sandboxes').then((r) => r.sandboxes),
  /** Ends the sandbox (204). */
  endSandbox: (id: string) => del<null>(`/v1/e2b/sandboxes/${seg(id)}`),
  /** Pauses the sandbox (204). */
  pauseSandbox: (id: string) => post<null>(`/v1/e2b/sandboxes/${seg(id)}/pause`, {}),
  /** Resumes a paused sandbox (200 or 201). */
  resumeSandbox: (id: string) => post<unknown>(`/v1/e2b/sandboxes/${seg(id)}/resume`, {}),
};

/** A shared read-only data volume on the hub machine (`/v1/volumes`): agents of a role mount it at /data/<name>. */
export interface DataVolume { name: string; size_bytes: number; created_ms: number; source: string; in_use: number[] }
export const volumesApi = {
  list: () => get<DataVolume[]>('/v1/volumes'),
  create: (name: string, source: string) => post<DataVolume>('/v1/volumes', { name, source }),
  remove: (name: string) => del<{ removed: boolean }>(`/v1/volumes/${seg(name)}`),
};

export const mcpApi = {
  catalog: () => get<McpPreset[]>('/v1/mcp/catalog'),
  servers: () => get<McpServerView[]>('/v1/mcp/servers'),
  server: (id: string) => get<McpServerView>(`/v1/mcp/servers/${encodeURIComponent(id)}`),
  /** 400 names the bad field (`{error, field}`); 409 on a duplicate id. */
  createServer: (config: McpServerConfig) => post<McpServerView>('/v1/mcp/servers', config),
  patchServer: (id: string, partial: Partial<McpServerConfig>) => patch<McpServerView>(`/v1/mcp/servers/${encodeURIComponent(id)}`, partial),
  /** Also removes the vault bindings `mcp:<id>`. `purge` also deletes the server's storage volume (?purge=1). */
  deleteServer: (id: string, purge = false) => del<unknown>(`/v1/mcp/servers/${encodeURIComponent(id)}${purge ? '?purge=1' : ''}`),
  testServer: (id: string, node?: string) => post<McpTestResult>(`/v1/mcp/servers/${encodeURIComponent(id)}/test`, node ? { node } : {}),
  startServer: (id: string) => post<McpServerView>(`/v1/mcp/servers/${encodeURIComponent(id)}/start`),
  stopServer: (id: string) => post<McpServerView>(`/v1/mcp/servers/${encodeURIComponent(id)}/stop`),
  serverTools: (id: string) => get<McpToolRow[]>(`/v1/mcp/servers/${encodeURIComponent(id)}/tools`),
  serverLogs: (id: string, limit = 100) => get<McpLogLine[]>(`/v1/mcp/servers/${encodeURIComponent(id)}/logs?limit=${limit}`),
};

// ---- This hub as an MCP server (docs/design/hub-mcp.md, "Hub API") ----------------------------------------------
export type HubBind = 'local' | 'remote';
export type HubScope = 'observe' | 'operate' | 'govern';
export type HubAuthMode = 'secure' | 'open';
export interface HubMcpStatus {
  enabled: boolean; bind: HubBind; allowed_origins: string[]; url: string; tokens: number; stored: boolean;
  /** Whether the endpoint serves TLS. A remote bind needs it. */
  tls: boolean;
  /** The origins the hub accepts in practice (the allowed ones, plus those it adds itself). Read-only. */
  effective_origins: string[];
  auth_mode: HubAuthMode; open_scope: HubScope;
  /** Set while open mode is on: the server's own warning. */
  warning?: string | null;
}
export interface HubMcpSettings {
  enabled: boolean; bind: HubBind; allowed_origins: string[];
  auth_mode: HubAuthMode; open_scope: HubScope;
  /** Required (true) for open + remote; the server refuses it otherwise. */
  confirm_open_remote?: boolean;
}
export interface HubToken {
  id: string; name: string; scope: HubScope; tools: string[]; created_ms: number;
  expires_ms: number | null; last_used_ms: number | null; calls: number; revoked_at_ms: number | null;
}
export interface HubTokenBody { name: string; scope: HubScope; tools?: string[]; expires_days?: number }
/** The token is in `token` once, at creation; the record never carries it. */
export interface HubTokenCreated { token: string; record: HubToken }
export interface HubCall { token_id: string; tool: string; args_sha256: string; ok: boolean; error: string | null; duration_ms: number; at_ms: number }
export interface HubCallsQuery { token?: string; tool?: string; limit?: number }

export const hubMcpApi = {
  status: () => get<HubMcpStatus>('/v1/hub-mcp'),
  save: (body: HubMcpSettings) => put<HubMcpStatus>('/v1/hub-mcp', body),
  tokens: () => get<{ tokens: HubToken[] }>('/v1/hub-mcp/tokens'),
  createToken: (body: HubTokenBody) => post<HubTokenCreated>('/v1/hub-mcp/tokens', body),
  revokeToken: (id: string) => del<unknown>(`/v1/hub-mcp/tokens/${encodeURIComponent(id)}`),
  calls: (q: HubCallsQuery = {}) => {
    const params = new URLSearchParams();
    if (q.token) params.set('token', q.token);
    if (q.tool) params.set('tool', q.tool);
    if (q.limit) params.set('limit', String(q.limit));
    const qs = params.toString();
    return get<{ calls: HubCall[] }>(`/v1/hub-mcp/calls${qs ? `?${qs}` : ''}`);
  },
};

// The prompt library (docs/design/prompt-library.md): system prompts, AGENTS.md and fragments, versioned and checked.
export type PromptKind = 'system' | 'agents_md' | 'fragment';
export type PromptSeverity = 'error' | 'warning' | 'info';
export interface PromptFinding { rule: string; severity: PromptSeverity; line: number; message: string }
export interface PromptStats { chars: number; words: number; lines: number; headings: number; est_tokens: number }
export interface PromptReport {
  kind: PromptKind; rules_version: number; findings: PromptFinding[]; score: number;
  errors: number; warnings: number; infos: number; stats: PromptStats;
}
/** One prompt. `score` is the live version's (null when unpublished); `best_version` the highest-scoring version. */
export interface PromptSummary {
  id: string; kind: PromptKind; title: string; description: string; tags: string[]; binding: string | null;
  active_version: number | null; latest_version: number; best_version: number | null; score: number | null;
  created_ms: number; updated_ms: number; archived_ms: number | null;
}
export interface PromptVersionMeta {
  version: number; note: string; author: string; created_ms: number; score: number;
  errors: number; warnings: number; infos: number; rules_version: number; sha256: string;
}
/** A whole version. `stale` when it was scored under an older rule set; `recheck` is then today's report. */
export interface PromptVersion extends PromptVersionMeta {
  body: string; findings: PromptFinding[]; stats: PromptStats; stale: boolean; recheck: PromptReport | null;
}
export interface PromptDetail { prompt: PromptSummary; versions: PromptVersionMeta[]; live: PromptVersion | null }
export interface PromptDiffLine { op: 'same' | 'add' | 'del'; old: number | null; new: number | null; text: string }
export interface PromptDiff { from: PromptVersion; to: PromptVersion; lines: PromptDiffLine[] }
export type PromptAction = 'create' | 'version' | 'publish' | 'unpublish' | 'edit' | 'archive' | 'restore';
export interface PromptEvent { id: number; prompt_id: string; action: PromptAction; version: number | null; actor: string; at_ms: number }
export interface PromptRule { id: string; severity: PromptSeverity; kinds: PromptKind[]; title: string; why: string }
/** `templates`: the starter body for each kind (each scores 100). */
export interface PromptRules { rules_version: number; rules: PromptRule[]; variables: string[]; templates: Record<PromptKind, string> }
export interface PromptCreate {
  id?: string; kind: PromptKind; title: string; description?: string; tags?: string[]; binding?: string | null;
  body: string; note?: string;
}
/** The editable parts of a prompt (PUT /v1/prompts/{id}): the kind and body are fixed by versions. */
export interface PromptMeta { title: string; description: string; tags: string[]; binding: string | null }

const promptPath = (id: string) => `/v1/prompts/${encodeURIComponent(id)}`;

export const promptsApi = {
  list: (q: { kind?: PromptKind; archived?: boolean } = {}) =>
    get<{ prompts: PromptSummary[] }>(`/v1/prompts${query({ kind: q.kind, archived: q.archived ? 'true' : undefined })}`),
  get: (id: string) => get<PromptDetail>(promptPath(id)),
  create: (body: PromptCreate) => post<{ prompt: PromptSummary }>('/v1/prompts', body),
  saveMeta: (id: string, meta: PromptMeta) => put<{ prompt: PromptSummary }>(promptPath(id), meta),
  version: (id: string, v: number) => get<PromptVersion>(`${promptPath(id)}/versions/${v}`),
  /** The body as text/markdown, for a download link. */
  rawUrl: (id: string, v: number) => `${promptPath(id)}/versions/${v}/raw`,
  /** 409 when `base_version` is no longer the latest; `created` is false when the body equals the latest. */
  addVersion: (id: string, body: string, note: string, base_version: number) =>
    post<{ version: PromptVersionMeta; created: boolean }>(`${promptPath(id)}/versions`, { body, note, base_version }),
  /** 422 when the version has error-level findings. */
  publish: (id: string, version: number) => post<{ prompt: PromptSummary }>(`${promptPath(id)}/publish`, { version }),
  unpublish: (id: string) => post<{ prompt: PromptSummary }>(`${promptPath(id)}/unpublish`),
  archive: (id: string) => post<{ prompt: PromptSummary }>(`${promptPath(id)}/archive`),
  restore: (id: string) => post<{ prompt: PromptSummary }>(`${promptPath(id)}/restore`),
  diff: (id: string, from: number, to: number) => get<PromptDiff>(`${promptPath(id)}/diff${query({ from, to })}`),
  history: (id: string) => get<{ events: PromptEvent[] }>(`${promptPath(id)}/history`),
  check: (kind: PromptKind, body: string) => post<PromptReport>('/v1/prompts/check', { kind, body }),
  rules: () => get<PromptRules>('/v1/prompts/rules'),
};

// The skill library (docs/design/skill-library.md): SKILL.md folders, versioned and checked, from files or GitHub.
export type SkillSeverity = PromptSeverity;
export interface SkillFinding { rule: string; severity: SkillSeverity; path: string; line: number; message: string }
export interface SkillStats { files: number; total_bytes: number; body_lines: number; est_tokens: number; scripts: number }
export interface SkillReport {
  rules_version: number; findings: SkillFinding[]; score: number; errors: number; warnings: number; infos: number;
  stats: SkillStats; name: string | null; description: string | null;
}
export type SkillSource =
  | { kind: 'built' }
  | { kind: 'upload' }
  | { kind: 'github'; repo: string; git_ref: string | null; path: string; commit: string };
/** One skill. `score` is the live version's (null when unpublished); `best_version` the highest-scoring version. */
export interface SkillSummary {
  id: string; name: string; title: string; description: string; tags: string[]; bindings: string[];
  active_version: number | null; latest_version: number; best_version: number | null; score: number | null;
  source: SkillSource; created_ms: number; updated_ms: number; archived_ms: number | null;
}
/** The editable parts of a skill (PUT /v1/skills/{id}): its title and tags. */
export interface SkillMeta { title: string; tags: string[] }
export interface SkillFile { path: string; sha256: string; size: number; executable: boolean }
export interface SkillVersionMeta {
  version: number; note: string; author: string; created_ms: number; score: number; errors: number; warnings: number;
  infos: number; rules_version: number; sha256: string; file_count: number; total_bytes: number; source_commit: string | null;
}
/** A whole version. `stale` when scored under an older rule set; `recheck` is then today's report. */
export interface SkillVersion extends SkillVersionMeta {
  skill_md: string; files: SkillFile[]; findings: SkillFinding[]; stale: boolean; recheck: SkillReport | null;
}
export interface SkillDetail { skill: SkillSummary; versions: SkillVersionMeta[]; live: SkillVersion | null }
export interface SkillDiff {
  from: SkillVersion; to: SkillVersion; skill_md: PromptDiffLine[];
  files: { added: string[]; removed: string[]; changed: string[] };
}
export type SkillAction = 'create' | 'version' | 'publish' | 'unpublish' | 'edit' | 'bind' | 'archive' | 'restore';
export interface SkillEvent { id: number; skill_id: string; action: SkillAction; version: number | null; actor: string; at_ms: number }
export interface SkillRule { id: string; severity: SkillSeverity; title: string; why: string }
export interface SkillRules { rules_version: number; rules: SkillRule[]; template: string }
/** A file to save: new bytes (base64), or an unchanged file by its sha256. */
export type SkillFileIn = { path: string; executable: boolean } & ({ content_b64: string } | { sha256: string });
/** An import preview (15 minutes): the skills found, each checked. `exists` names a live skill with the same name. */
export interface SkillPreview {
  preview_id: string; source: SkillSource; expires_ms: number;
  skills: { path: string; name: string | null; title: string; report: SkillReport; files: number; exists: string | null }[];
}
export interface SkillGithubCheck { current: string; latest: string; update_available: boolean }

const skillPath = (id: string) => `/v1/skills/${encodeURIComponent(id)}`;

export const skillsApi = {
  list: (q: { archived?: boolean } = {}) => get<{ skills: SkillSummary[] }>(`/v1/skills${query({ archived: q.archived ? 'true' : undefined })}`),
  get: (id: string) => get<SkillDetail>(skillPath(id)),
  create: (body: { id?: string; title: string; tags?: string[]; bindings?: string[]; skill_md: string; files?: SkillFileIn[]; note?: string }) =>
    post<{ skill: SkillSummary }>('/v1/skills', body),
  saveMeta: (id: string, meta: SkillMeta) => put<{ skill: SkillSummary }>(skillPath(id), meta),
  setBindings: (id: string, bindings: string[]) => put<{ skill: SkillSummary }>(`${skillPath(id)}/bindings`, { bindings }),
  version: (id: string, v: number) => get<SkillVersion>(`${skillPath(id)}/versions/${v}`),
  /** One file's bytes, for a download link or a fetch. */
  fileUrl: (id: string, v: number, path: string) => `${skillPath(id)}/versions/${v}/files/${path.split('/').map(encodeURIComponent).join('/')}`,
  /** 409 when `base_version` is not the latest; `created` false when nothing changed. */
  addVersion: (id: string, body: { skill_md: string; files: SkillFileIn[]; note: string; base_version: number }) =>
    post<{ version: SkillVersionMeta; created: boolean }>(`${skillPath(id)}/versions`, body),
  publish: (id: string, version: number) => post<{ skill: SkillSummary }>(`${skillPath(id)}/publish`, { version }),
  unpublish: (id: string) => post<{ skill: SkillSummary }>(`${skillPath(id)}/unpublish`),
  archive: (id: string) => post<{ skill: SkillSummary }>(`${skillPath(id)}/archive`),
  restore: (id: string) => post<{ skill: SkillSummary }>(`${skillPath(id)}/restore`),
  diff: (id: string, from: number, to: number) => get<SkillDiff>(`${skillPath(id)}/diff${query({ from, to })}`),
  history: (id: string) => get<{ events: SkillEvent[] }>(`${skillPath(id)}/history`),
  check: (skill_md: string, files: { path: string; size: number; executable: boolean; text?: string }[]) =>
    post<SkillReport>('/v1/skills/check', { skill_md, files }),
  rules: () => get<SkillRules>('/v1/skills/rules'),
  /** A .zip or .skill file's bytes. */
  importPreview: async (bytes: ArrayBuffer): Promise<SkillPreview> => {
    const res = await fetch('/v1/skills/import/preview', { method: 'POST', headers: { 'content-type': 'application/zip' }, body: bytes });
    const data = (await res.json().catch(() => null)) as (SkillPreview & { error?: string }) | null;
    if (!res.ok) throw new ApiError(res.status, data?.error ?? `import: ${res.status}`);
    return data as SkillPreview;
  },
  githubPreview: (url: string) => post<SkillPreview>('/v1/skills/github/preview', { url }),
  importCommit: (preview_id: string, picks: string[], bindings: string[] = []) =>
    post<{ skills: SkillSummary[] }>('/v1/skills/import/commit', { preview_id, picks, bindings }),
  githubCheck: (id: string) => post<SkillGithubCheck>(`${skillPath(id)}/github/check`),
  githubUpdate: (id: string) => post<{ version: SkillVersionMeta; created: boolean }>(`${skillPath(id)}/github/update`),
  /** Zip downloads. */
  exportUrl: (id: string, v: number) => `${skillPath(id)}/versions/${v}/export`,
  exportManyUrl: (ids: string[]) => `/v1/skills/export${query({ ids: ids.join(',') })}`,
};

// ---- IDE -------------------------------------------------------------------
// The IDE's owner-only routes (docs/design/ide.md §6.3). A workspace id (`agent:7`) goes in the path, encoded. The tree
// and diff shapes are apz-ide's (ide/types.ts).

/** One row of the workspace list. A station row carries its machine's OS and its status (only online stations are listed). */
export interface IdeWorkspaceInfo { id: string; kind: WorkspaceKind; owner: string; name: string; writable: boolean; status: string; os?: StationOs | null }
export interface IdeFile { path: string; size: number; hash: string; text: string }
export interface IdeSaved { path: string; hash: string; checkpoint: number | null }
/** `changes`: the files since `base` and the unified diff. `files` and `stats` are parsed from `diff` when absent. */
export interface IdeChanges { base: unknown; checkpoint: number | null; diff: string; files?: FileChange[]; stats?: Stats }
export interface IdeCheckpoint { n: number; label: string; created_ms: number; files: number }
export interface IdeMatch { path: string; line: number; text: string }
/** A run's id (the hub's `{run}`), then one poll's answer: the output from `offset` up to `next`, and the run's state. */
export interface IdeRunStarted { run: string }
export interface IdeRunPoll { run: string; offset: number; output: string; next: number; done: boolean; exit_code: number | null; timed_out: boolean }
export interface IdeRunStop { run: string; stopped: boolean; done: boolean }

const ideWs = (id: string) => `/v1/ide/workspaces/${encodeURIComponent(id)}`;

export const ideApi = {
  workspaces: () => get<IdeWorkspaceInfo[]>('/v1/ide/workspaces'),
  /** One page of a directory's entries (`path` `''` is the root); `next` is the cursor for the following page. */
  tree: (id: string, path: string, cursor?: string | null) => get<TreePage>(`${ideWs(id)}/tree${query({ path, cursor: cursor ?? undefined })}`),
  file: (id: string, path: string) => get<IdeFile>(`${ideWs(id)}/file${query({ path })}`),
  /** `expected_hash` is the hash the buffer was read at. 409 (`ApiError.status`) when the file has moved on: `body.current_hash`. */
  save: (id: string, body: { path: string; text: string; expected_hash: string | null }) => put<IdeSaved>(`${ideWs(id)}/file`, body),
  /** `base` is `work_start` or `checkpoint:<n>`. */
  changes: (id: string, base: string) => get<IdeChanges>(`${ideWs(id)}/changes${query({ base })}`),
  checkpoints: async (id: string): Promise<IdeCheckpoint[]> => {
    const r = await get<IdeCheckpoint[] | { checkpoints: IdeCheckpoint[] }>(`${ideWs(id)}/checkpoints`);
    return Array.isArray(r) ? r : r.checkpoints;
  },
  restore: (id: string, n: number) => post<{ restored: unknown; checkpoint: number | null }>(`${ideWs(id)}/checkpoints/${n}/restore`),
  search: (id: string, q: string, glob?: string) => get<{ matches: IdeMatch[]; truncated: boolean }>(`${ideWs(id)}/search${query({ q, glob })}`),
  /** A bounded command in the workspace (agent workspaces). `timeout_s` is the hub's bound; the run answers at once with its id. */
  runStart: (id: string, command: string, timeout_s?: number) => post<IdeRunStarted>(`${ideWs(id)}/runs`, { command, timeout_s }),
  /** The output from `offset`; poll again from `next` until `done`. */
  runPoll: (id: string, run: string, offset: number) => get<IdeRunPoll>(`${ideWs(id)}/runs/${encodeURIComponent(run)}${query({ offset })}`),
  runStop: (id: string, run: string) => post<IdeRunStop>(`${ideWs(id)}/runs/${encodeURIComponent(run)}/stop`),
};
