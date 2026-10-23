// The chats' context and model routes on the hub: how full a chat's context window is (`GET .../context`), the
// model a chat runs on (a `PATCH` of an agent, or of a chat, with `provider/model[@effort]` or null for the default)
// and the compress control. Same origin and the shared `call` helper; the types follow the hub's real answers
// (crates/apid/src/context_track.rs). This module only fetches; it draws nothing.

import { call, type Agent } from './api';
import type { Chat } from './api-work';

/** The six parts of a chat's context, in the order the hub lists them (and the breakdown shows them). */
export const CONTEXT_PARTS = ['system', 'tools', 'history', 'tool_results', 'attachments', 'summary'] as const;
export type ContextPart = (typeof CONTEXT_PARTS)[number];

export interface ContextPartRow { part: ContextPart; tokens: number }

/** One tool's calls so far in the work: its errors, and the tokens its results took. */
export interface ContextToolRow { tool: string; calls: number; errors: number; result_tokens: number }

/** A chat's context (`GET /v1/agents/{cid}/context` or `GET /v1/chats/{id}/context`). */
export interface ContextView {
  cid: number;
  work: number;
  /** The model the chat runs on, `provider/model[@effort]`. */
  model: string;
  /** The model's context window in tokens; null when the window is unknown. */
  window: number | null;
  /** Tokens the context holds now. */
  used: number;
  output: number;
  /** used / window, 0..1; null when the window is unknown. */
  fill: number | null;
  /** The parts, in CONTEXT_PARTS order. */
  parts: ContextPartRow[];
  /** The tools, largest result first. */
  tools: ContextToolRow[];
  /** Model calls so far. */
  calls: number;
  compactions: number;
  updated_ms: number;
}

/** The context of an agent's latest work, or of the work `work` when given. */
export const agentContext = (cid: number, work?: number): Promise<ContextView> =>
  call<ContextView>('GET', `/v1/agents/${cid}/context${work === undefined ? '' : `?work=${work}`}`);

/** The context of a chat's turns: the orchestrator's (or its planner's) work on it. */
export const chatContext = (id: string): Promise<ContextView> =>
  call<ContextView>('GET', `/v1/chats/${encodeURIComponent(id)}/context`);

/** Sets an agent's model (`provider/model[@effort]`); null goes back to the default. A model outside the tier's policy is refused with 400. */
export const setAgentModel = (cid: number, model: string | null): Promise<Agent> =>
  call<Agent>('PATCH', `/v1/agents/${cid}`, { model });

/** Sets a chat's model: its turns run on it from the next message; null goes back to the agent's own. */
export const setChatModel = (id: string, model: string | null): Promise<Chat> =>
  call<Chat>('PATCH', `/v1/chats/${encodeURIComponent(id)}`, { model });

/** Compresses an agent's conversation (its running work's history). */
export const compressContext = (cid: number): Promise<unknown> =>
  call<unknown>('POST', `/v1/agents/${cid}/control`, { action: 'compress' });

export const contextApi = {
  agent: agentContext,
  chat: chatContext,
  setAgentModel,
  setChatModel,
  compress: compressContext,
};
