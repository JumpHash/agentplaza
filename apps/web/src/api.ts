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
export interface ModelInfo { provider: string; id: string; name: string | null }
export interface ModelsView { models: ModelInfo[]; errors: { provider: string; error: string }[] }

export interface Settings {
  default_model: string; orchestrator_model: string; role_models: Record<string, string>;
  model_policy: { orchestrator: string[]; worker: string[] };
  effort: string | null; max_turns: number; max_tokens: number; fetch_allow: string[];
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
export interface BoardComment { id: string; post: string; agent_cid: number | null; author: string; body: string; created_at: number; attachments: string[] }
export interface BoardPost { id: string; agent_cid: number; role: string; subject: string; body: string; created_at: number; task: string | null; comments: BoardComment[]; attachments: string[] }
export interface Conversation { id: string; from_cid: number; to_cid: number; from_role: string; to_role: string; text: string; created_at: number; delivered: boolean; attachments?: string[] }
export interface Motion { id: string; kind: string; text: string; state: string; proposer_cid: number; tally: { yes: number; no: number; abstain: number; voted: number; seats: number }; votes?: { cid: number; role: string; vote: string; reason: string }[]; created_at: number }
export interface Account { id: number; kind: string; owner_ref: string; balance: number; credit_limit: number; created_at: number }
export interface Treasury { accounts: Account[]; totals: { accounts: number; entries: number; credits: number; debits: number; balance: number }; spent: number; plaza_budget: number; note: string }
export interface Posting { account: string; amount: number }
export interface JournalEntry { id: number; kind: string; at: number; postings: Posting[]; memo: string; task: string | null }
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
  kind: 'server' | 'workstation' | 'edge' | 'apple';
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
  models: () => get<ModelsView>('/v1/models'),
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
  motions: () => get<{ motions: Motion[] }>('/v1/motions').then((r) => r.motions),
  decide: (id: string, decision: 'approve' | 'reject', reason: string) => post<Motion>(`/v1/motions/${id}/decide`, { decision, reason }),
  treasury: () => get<Treasury>('/v1/treasury'),
  treasuryJournal: () => get<{ entries: JournalEntry[] } | JournalEntry[]>('/v1/treasury/journal').then((r) => (Array.isArray(r) ? r : r.entries)),
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
  runs: () => get<{ runs: Run[] }>('/v1/runs').then((r) => r.runs),
  run: (id: string) => get<Run & { vms: (Vm & { serial?: string })[] }>(`/v1/runs/${id}`),
  createRun: (n: number, role?: string) => post<Run>('/v1/runs', { n, vcpus: 1, memory_mib: 256, role }),
  killRun: (id: string) => del<Run>(`/v1/runs/${id}`),

  files: (kind?: 'deliverable' | 'shared') => get<Deliverable[]>(`/v1/files${kind ? `?kind=${kind}` : ''}`),
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
};

export type ApprovalState = 'pending' | 'approved' | 'denied' | 'used';
/** A tool call an agent asked to make, waiting on (or recording) the owner's decision. */
export interface Approval { id: string; cid: number; tool: string; input: unknown; created_ms: number; state: ApprovalState; reason: string | null }
/** `autonomous` acts freely, `ask` queues changes for approval, `read_only` refuses them. */
export type PermissionMode = 'autonomous' | 'ask' | 'read_only';
