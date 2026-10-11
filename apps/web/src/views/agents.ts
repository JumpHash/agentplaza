// Agents: the roster (`#/agents`) or one agent (`#/agents/<cid>`).

import { mountAgentDetail } from './agent-detail';
import { mountAgentList } from './agent-list';
import type { Cleanup } from './index';

export function mount(root: HTMLElement, params: string[]): Cleanup {
  const cid = Number(params[0]);
  return params[0] && Number.isFinite(cid) ? mountAgentDetail(root, cid) : mountAgentList(root);
}
