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
  cid: number; name: string; role: string; tier: Tier; status: AgentStatus;
  model: string; model_override: string | null; model_source: 'agent' | 'role' | 'default';
  connected_at_ms: number; last_seen_ms: number; run: string | null; vm: number | null;
  usage: Usage; credits: number; work: WorkItem | null; queue: number;
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

export interface BoardPost { id: string; agent_cid: number; role: string; subject: string; body: string; created_at: number; task: string | null }
export interface Conversation { id: string; from_cid: number; to_cid: number; from_role: string; to_role: string; text: string; created_at: number; delivered: boolean }
export interface Motion { id: string; kind: string; text: string; state: string; proposer_cid: number; tally: { yes: number; no: number; abstain: number; voted: number; seats: number }; votes?: { cid: number; role: string; vote: string; reason: string }[]; created_at: number }
export interface Account { id: number; kind: string; owner_ref: string; balance: number; credit_limit: number; created_at: number }
export interface Treasury { accounts: Account[]; totals: { accounts: number; entries: number; credits: number; debits: number; balance: number }; spent: number; note: string }
export interface Machine { name: string; arch: string; backend: string; irqchip: string; class: string; cpus: number; memory_mib: number; vsock_mode: string; started_at_ms: number }
export interface Vm { index: number; state: string; guest_cid: number | null; role?: string | null }
export interface Run { id: string; state: string; vcpus: number; memory_mib: number; beats: number | null; role: string | null; created_at: number; ended_at: number | null; vms: Vm[] }

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
  conversations: () => get<{ conversations: Conversation[] }>('/v1/conversations').then((r) => r.conversations),
  motions: () => get<{ motions: Motion[] }>('/v1/motions').then((r) => r.motions),
  decide: (id: string, decision: 'approve' | 'reject', reason: string) => post<Motion>(`/v1/motions/${id}/decide`, { decision, reason }),
  treasury: () => get<Treasury>('/v1/treasury'),
  agora: () => get<{ calls: unknown[] }>('/v1/agora').then((r) => r.calls),

  machine: () => get<Machine>('/v1/machine'),
  runs: () => get<{ runs: Run[] }>('/v1/runs').then((r) => r.runs),
  run: (id: string) => get<Run & { vms: (Vm & { serial?: string })[] }>(`/v1/runs/${id}`),
  createRun: (n: number, role?: string) => post<Run>('/v1/runs', { n, vcpus: 1, memory_mib: 256, role }),
  killRun: (id: string) => del<Run>(`/v1/runs/${id}`),
};
