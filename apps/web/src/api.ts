// The apid client: one function per endpoint, typed by what apid returns.
// The console is served by apid, so every path is same-origin.

export class ApiError extends Error {
  constructor(public status: number, message: string) {
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
    throw new ApiError(res.status, message);
  }
  return data as T;
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

export type AgentStatus = 'offline' | 'idle' | 'working';
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
  usage: Usage; work: WorkItem | null; queue: number;
}

export interface Overview {
  agents: { total: number; idle: number; working: number; offline: number };
  tasks: Record<string, number>;
  default_model: string; orchestrator_model: string; dev_models: boolean; hub: boolean; noded: boolean;
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
  /** Whether a value is available, and where from (the vault wins over the environment). */
  set: boolean; source: 'vault' | 'env' | null;
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
export interface ModelInfo { provider: string; id: string; name: string | null; group?: ModelGroup }
export interface ModelsView { models: ModelInfo[]; errors: { provider: string; error: string }[] }

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
}

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
export interface GitPull { number: number; title: string; state: string; author: string; head: string; base: string; created_at: string; updated_at: string; comments: number; review: 'approved' | 'changes_requested' | 'pending'; mergeable: boolean }
export interface GitPullDetail extends GitPull { body: string; diff: string; reviews: { author: string; state: string; body: string; at: string }[]; comments_list?: never; files: { path: string; additions: number; deletions: number }[] }
export interface HarnessLimits { max_turns: number | null; compress_at: number | null; keep_recent: number | null }
export interface HarnessLoop { id: string; cid: number; prompt: string; every_secs: number; max_runs: number; runs: number; next_at_ms: number; state: 'running' | 'done' | 'stopped'; by: string; by_cid: number; created_ms: number; last_work: number | null; last_summary: string | null }
export interface LawInfo {
  id: string; title: string; tier: string; enforcement: string; severity: string; statement: string; rationale: string;
  effect: 'permit' | 'forbid'; ops: string[]; principal: string; resource: string; conditions: string[]; explanation: string;
  file: string; source: string; refusals: number; agents: { cid: number; name: string; count: number }[]; last_at_ms: number | null;
}
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
export interface BoardPost { id: string; agent_cid: number; role: string; subject: string; body: string; created_at: number; task: string | null; comments: BoardComment[]; attachments: string[] }
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
export interface Motion { id: string; kind: string; text: string; state: string; proposer_cid: number; tally: { yes: number; no: number; abstain: number; voted: number; seats: number }; votes?: { cid: number; role: string; vote: string; reason: string }[]; created_at: number }
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
  role: 'hub' | 'node';
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
}
export interface FleetLink { a: string; b: string; online: boolean }
export interface FleetResponse { machines: FleetMachine[]; links: FleetLink[] }

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

  tasks: () => get<{ tasks: Task[] }>('/v1/tasks').then((r) => r.tasks),
  task: (id: string) => get<Task>(`/v1/tasks/${id}`),
  createTask: (t: { goal: string; acceptance?: string; role?: string; budget?: number; assignee?: number }) => post<Task>('/v1/tasks', t),
  withdrawTask: (id: string) => del<Task>(`/v1/tasks/${id}`),

  board: () => get<{ posts: BoardPost[] }>('/v1/board').then((r) => r.posts),
  /** The owner comments on a board post. */
  commentOnPost: (postId: string, text: string) => post<BoardComment>(`/v1/board/${postId}/comments`, { text }),
  conversations: () => get<{ conversations: Conversation[] }>('/v1/conversations').then((r) => r.conversations),
  conversationsPage: (q: ConversationQuery = {}) => get<ConversationsPage>(`/v1/conversations${query({ limit: q.limit, before: q.before, q: q.q })}`),
  conversationThread: (a: number, b: number, q: ConversationQuery = {}) =>
    get<ThreadPage>(`/v1/conversations/${a}/${b}${query({ limit: q.limit, before: q.before, q: q.q })}`),
  motions: () => get<{ motions: Motion[] }>('/v1/motions').then((r) => r.motions),
  decide: (id: string, decision: 'approve' | 'reject', reason: string) => post<Motion>(`/v1/motions/${id}/decide`, { decision, reason }),
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
  gitPulls: (repo: string, state: 'open' | 'closed' | 'all' = 'open') => get<GitPull[]>(`/v1/git/repos/${repo}/pulls?state=${state}`),
  gitPull: (repo: string, n: number) => get<GitPullDetail>(`/v1/git/repos/${repo}/pulls/${n}`),
  gitReview: (repo: string, n: number, event: 'APPROVE' | 'REQUEST_CHANGES' | 'COMMENT', body: string) => post<unknown>(`/v1/git/repos/${repo}/pulls/${n}/review`, { event, body }),
  gitMerge: (repo: string, n: number, style: 'merge' | 'squash' | 'rebase' = 'merge') => post<unknown>(`/v1/git/repos/${repo}/pulls/${n}/merge`, { style }),
  loops: () => get<HarnessLoop[]>('/v1/loops'),
  startLoop: (cid: number, prompt: string, every_minutes: number, max_runs: number) => post<HarnessLoop>('/v1/loops', { cid, prompt, every_minutes, max_runs }),
  stopLoop: (id: string) => del<HarnessLoop>(`/v1/loops/${id}`),
  agentLimits: (cid: number) => get<HarnessLimits>(`/v1/agents/${cid}/limits`),
  agentControl: (cid: number, body: { action: 'compress' | 'steer' | 'extend_turns' | 'set_limits'; text?: string; turns?: number; max_turns?: number; compress_at?: number; keep_recent?: number }) => post<unknown>(`/v1/agents/${cid}/control`, body),
  laws: () => get<LawInfo[]>('/v1/laws'),
  infractions: (limit = 200) => get<Infraction[]>(`/v1/infractions?limit=${limit}`),
  agentInfractions: (cid: number) => get<Infraction[]>(`/v1/agents/${cid}/infractions`),
  plazaState: () => get<PlazaState>('/v1/plaza/state'),
  stopAll: (reason = '') => post<PlazaState>('/v1/plaza/stop', { reason }),
  resumeAll: () => post<PlazaState>('/v1/plaza/resume', {}),
  restartAgent: (cid: number) => post<unknown>(`/v1/agents/${cid}/restart`, {}),
  wipeAgent: (cid: number, opts: { memory?: boolean; transcript?: boolean; identity?: boolean }) => post<{ ok: boolean; wiped: string[] }>(`/v1/agents/${cid}/wipe`, opts),
  retireAgent: (cid: number) => post<unknown>(`/v1/agents/${cid}/retire`, {}),
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
  decideApproval: (id: string, approve: boolean, reason?: string) => post<unknown>(`/v1/approvals/${id}`, { approve, reason }),
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
  joined_ms: number; last_seen_ms: number; machine: { cpus?: number; memory_mib?: number; host?: string; version?: string } | null;
  draining: boolean; revoked: boolean; address: string; online: boolean; agents: number;
}
export type JoinCodeState = 'active' | 'used' | 'expired' | 'revoked';
export interface JoinCode { id: string; label: string; created_ms: number; expires_ms: number; state: JoinCodeState; used_by: string | null }

export type ApprovalState = 'pending' | 'approved' | 'denied' | 'used';
/** A tool call an agent asked to make, waiting on (or recording) the owner's decision. */
export interface Approval { id: string; cid: number; tool: string; input: unknown; created_ms: number; state: ApprovalState; reason: string | null }
/** `autonomous` acts freely, `ask` queues changes for approval, `read_only` refuses them. */
export type PermissionMode = 'autonomous' | 'ask' | 'read_only';
