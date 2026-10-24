// The tool library's pure rules: the breadcrumb of a path, the rows a group shows, the schema's parameters, the seats a
// policy line reads, the tones of a cost and an access. The DOM is not touched here (the view renders these).

import type { JsonSchema, ToolAccess, ToolCost, ToolEntry, ToolNode, ToolSeat } from '../api-tool-library';

export type Tone = 'ok' | 'warn' | 'danger' | 'info' | 'accent' | 'muted';

/** The path's crumbs from the root: each one a group's path and its name. The root has none. */
export function crumbs(path: string): { name: string; path: string }[] {
  if (!path) return [];
  const parts = path.split('.');
  return parts.map((name, i) => ({ name, path: parts.slice(0, i + 1).join('.') }));
}

/** The parent of a path: `deploy.rollback` is in `deploy`, `deploy` in the root (""). */
export function parentOf(path: string): string {
  const cut = path.lastIndexOf('.');
  return cut < 0 ? '' : path.slice(0, cut);
}

/** A group's rows, in the order the hub gave them, filtered by a words-only text (every word must appear in the path or summary). */
export function visibleNodes(nodes: ToolNode[], filter: string): ToolNode[] {
  const words = filter.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return nodes;
  return nodes.filter((node) => {
    const text = `${node.path} ${node.summary}`.toLowerCase();
    return words.every((word) => text.includes(word));
  });
}

/** A node's label: its last segment, and its count when it is a group. */
export function nodeLabel(node: ToolNode): string {
  const name = node.path.slice(node.path.lastIndexOf('.') + 1);
  return node.kind === 'group' ? `${name} (${node.count})` : name;
}

/** The parameters of an input schema as rows: name, type, whether required, and what it is for. */
export function schemaRows(schema: JsonSchema): { name: string; type: string; required: boolean; description: string }[] {
  const required = new Set(schema.required ?? []);
  return Object.entries(schema.properties ?? {}).map(([name, property]) => ({
    name,
    type: property.type ?? 'any',
    required: required.has(name),
    description: property.description ?? '',
  }));
}

/** The words a cost hint reads as. */
export function costLabel(cost: ToolCost): string {
  return { free: 'free', cheap: 'a read or a small write', metered: 'billed: a model or a provider', slow: 'takes minutes or more' }[cost];
}

/** The tone of a cost hint. */
export function costTone(cost: ToolCost): Tone {
  return { free: 'ok', cheap: 'info', metered: 'warn', slow: 'accent' }[cost] as Tone;
}

/** The words an access reads as. */
export function accessLabel(access: ToolAccess): string {
  return access === 'operator' ? 'orchestrator only' : 'any seat its laws allow';
}

/** Where the entry runs, in words. */
export function runsLabel(entry: Pick<ToolEntry, 'runs'>): string {
  return entry.runs === 'guest' ? 'in the agent\'s VM' : 'on the hub';
}

/** The seats line: how many seats may use the entry, and which ones may not. */
export function seatsLine(seats: ToolSeat[]): string {
  const allowed = seats.filter((seat) => seat.allowed).map((seat) => seat.seat);
  const denied = seats.length - allowed.length;
  if (seats.length === 0) return 'No seat is listed.';
  return `${allowed.length} of ${seats.length} seats may use it${denied ? `; not: ${seats.filter((s) => !s.allowed).map((s) => s.seat).join(', ')}` : ''}.`;
}

/** The reason an entry is unavailable to the seat it was described for, or '' when it is available. */
export function unavailableText(entry: Pick<ToolEntry, 'available' | 'reason'>): string {
  return entry.available ? '' : entry.reason ?? 'not available to this seat';
}

/** A search box's query, trimmed; empty means no search. */
export function queryOf(text: string): string {
  return text.trim().replace(/\s+/g, ' ');
}
