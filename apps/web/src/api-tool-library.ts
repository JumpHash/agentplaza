// The tool library: the hub's tree of every capability (crates/apid/src/tool_library/). The owner browses the tree, searches
// it, and reads an entry's schema and the seats that may use it. The routes are read-only; the answers follow the hub's.

import { call } from './api';

/** A child of a group, as the tree shows it: a group (with how many entries are under it) or an entry. */
export type ToolKind = 'group' | 'entry';
export interface ToolNode {
  path: string;
  kind: ToolKind;
  summary: string;
  /** The entries under a group; 0 for an entry. */
  count: number;
}

/** Where an entry runs: in the agent's VM, or on the hub. */
export type ToolRuns = 'guest' | 'hub';
/** Who may use an entry beyond its own tool or connector: anyone its laws allow, or the orchestrator only. */
export type ToolAccess = 'any' | 'operator';
/** A cost hint. */
export type ToolCost = 'free' | 'cheap' | 'metered' | 'slow';

/** One entry in full: what it does, its schema, its policy. */
export interface ToolEntry {
  path: string;
  summary: string;
  description: string;
  runs: ToolRuns;
  source: string;
  input_schema: JsonSchema;
  output_schema: JsonSchema;
  access: ToolAccess;
  approval: boolean;
  cost: ToolCost;
  exec: string;
  available: boolean;
  reason: string | null;
}

/** A JSON Schema as the entries carry it: the parts the owner reads. */
export interface JsonSchema {
  type?: string;
  description?: string;
  properties?: Record<string, { type?: string; description?: string }>;
  required?: string[];
  [key: string]: unknown;
}

/** The entry's answer on the owner's page: the entry, and the seats that may use it. */
export interface ToolSeat { seat: string; allowed: boolean; reason: string | null }
export interface ToolEntryDetail extends ToolEntry { seats: ToolSeat[] }

/** A group's listing: its children, and (at the root) the route groups the library leaves out and why. */
export interface ToolListing {
  path: string;
  summary: string;
  children: ToolNode[];
  internal: { group: string; reason: string }[];
}

export interface ToolHit { path: string; summary: string; runs: ToolRuns; score: number }

export const toolLibraryApi = {
  listing(path: string): Promise<ToolListing> {
    return call<ToolListing>('GET', `/v1/tool-library?path=${encodeURIComponent(path)}`);
  },
  search(query: string, under = ''): Promise<{ query: string; hits: ToolHit[] }> {
    const scope = under ? `&under=${encodeURIComponent(under)}` : '';
    return call('GET', `/v1/tool-library/search?q=${encodeURIComponent(query)}${scope}`);
  },
  entry(path: string): Promise<ToolEntryDetail> {
    return call<ToolEntryDetail>('GET', `/v1/tool-library/entry?path=${encodeURIComponent(path)}`);
  },
};
