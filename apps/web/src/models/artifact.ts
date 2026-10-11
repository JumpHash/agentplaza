/* A task's artifact as the daemon reports it (the `POST
   /api/v0/tasks/{id}/artifact` read-through, crates/noded ArtifactView), read
   through a defensive parse: the relay between the console and the node could
   be hostile, so this is never trusted. Every field is checked and a missing
   or mistyped one is an error that NAMES its path; unknown extra fields are
   tolerated, since the view may grow columns the console does not yet read.

   The content is a file the task's AGENT wrote — guest-derived text, and it is
   rendered as text, never as markup. */

export interface TaskArtifact {
  /** The task whose agent read the file. */
  task: string;
  /** The guest CID of the agent that read it (the task's assignee at claim time). */
  agent_cid: number;
  /** The path the console asked for, as it sent it. */
  path: string;
  /** The text the agent read. */
  content: string;
  /** The file's size on disk, in bytes, as the agent reported it. */
  size: number;
  /** Whether the agent's read cap, rather than the file's end, ended the content. */
  truncated: boolean;
}

/** How a task's artifact read ended. The states are distinct on purpose — a
    node that was never told where the hub's admin surface is (`unconfigured`,
    a 503) is not the same as a hub that could not be reached (`unreachable`,
    a 502 or no answer at all) nor as a refusal (`refused` — the daemon's, the
    hub's own policy, or the agent's own words). The console shows each one as
    what it is, and never dresses a failure up as a read. */
export type ArtifactReadState = 'loading' | 'read' | 'refused' | 'unconfigured' | 'unreachable';

/** The state a failed read wears, from the status the daemon answered with —
    or from no answer at all (`null`: the node was never reached). The mapping
    is the one place that reads the endpoint's status table
    (crates/noded `read_artifact`): every 4xx is a refusal, a 502 is a hub out
    of reach, and a 503 is a daemon never told where the hub's admin surface
    is. */
export function readStateOf(status: number | null): ArtifactReadState {
  if (status === 503) return 'unconfigured';
  if (status == null || status === 502) return 'unreachable';
  return 'refused';
}

/** A missing or mistyped required field; the message names its path. */
export class ArtifactParseError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ArtifactParseError';
  }
}

function obj(v: unknown, path: string): Record<string, unknown> {
  if (v == null || typeof v !== 'object' || Array.isArray(v)) {
    throw new ArtifactParseError(`${path} is missing or not an object`);
  }
  return v as Record<string, unknown>;
}

function str(o: Record<string, unknown>, key: string, path: string): string {
  const v = o[key];
  if (typeof v !== 'string') throw new ArtifactParseError(`${path}.${key} is missing or not a string`);
  return v;
}

/** A byte count or a guest CID: a whole, non-negative number. */
function count(o: Record<string, unknown>, key: string, path: string): number {
  const v = o[key];
  if (typeof v !== 'number' || !Number.isInteger(v) || v < 0) {
    throw new ArtifactParseError(`${path}.${key} is missing or not a whole, non-negative number`);
  }
  return v;
}

/** Parse one artifact answer defensively; unknown extras are tolerated. */
export function parseArtifact(raw: unknown): TaskArtifact {
  const o = obj(raw, 'artifact');
  const truncated = o['truncated'];
  if (typeof truncated !== 'boolean') {
    throw new ArtifactParseError('artifact.truncated is missing or not a boolean');
  }
  return {
    task: str(o, 'task', 'artifact'),
    agent_cid: count(o, 'agent_cid', 'artifact'),
    path: str(o, 'path', 'artifact'),
    content: str(o, 'content', 'artifact'),
    size: count(o, 'size', 'artifact'),
    truncated,
  };
}
