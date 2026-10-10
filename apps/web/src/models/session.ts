/* The frozen vsock session, read back out of a VM's transcript. The guest's
   harness logs every protocol line it sends ([vsock->]) and receives
   ([vsock<-]) to the serial console — the only place a session is observable
   from outside the guest. This module is the one pure derivation of "where
   the session stands": the 3D district's live arcs and the run report's
   session summary both read it, so they can never disagree.
   The transcript is untrusted: every payload is JSON.parsed inside a guard
   and must match the frozen shapes (crates/agent-harness proto.rs); anything
   else is skipped, never believed. */

export type SessionPhase = 'active' | 'complete';

export interface SessionRead {
  /** active = hello sent, no bye yet; complete = the bye line landed. */
  phase: SessionPhase;
  /** The session's steps in observed order: hello, welcome, echo ✓, … */
  steps: string[];
  /** Tool calls made — the bye line's own count when it landed. */
  calls: number;
}

const OUT = '[vsock->] ';
const IN = '[vsock<-] ';
const COMPLETE_NOTE = 'agent-harness: session complete';

/** A payload parsed to a plain object with a string `type`. */
function payload(line: string, prefix: string): Record<string, unknown> | null {
  const text = line.slice(prefix.length);
  let obj: unknown;
  try {
    obj = JSON.parse(text);
  } catch {
    return null;
  }
  if (obj == null || typeof obj !== 'object' || Array.isArray(obj)) return null;
  const rec = obj as Record<string, unknown>;
  return typeof rec['type'] === 'string' ? rec : null;
}

/**
 * Where a VM's session stands, derived from its transcript lines alone.
 * Null when the transcript holds no session at all. A fresh `hello` restarts
 * the reading — a guest that reboots (the harness exits by rebooting) begins
 * a new session, and the summary describes the latest one.
 */
export function sessionOf(lines: readonly string[]): SessionRead | null {
  let steps: string[] | null = null; // null = no session under way
  let toolSteps: number[] = []; // step indexes of unresolved tool calls
  let resolved = 0; // tool calls answered (ok or not)
  let phase: SessionPhase = 'active';
  let calls = 0;
  for (const line of lines) {
    if (line.startsWith(OUT)) {
      const msg = payload(line, OUT);
      if (msg == null) continue;
      switch (msg['type']) {
        case 'hello':
          steps = ['hello'];
          toolSteps = [];
          resolved = 0;
          calls = 0;
          phase = 'active';
          break;
        case 'tool_call': {
          if (steps == null || phase === 'complete') break;
          const name = typeof msg['name'] === 'string' ? msg['name'] : '?';
          toolSteps.push(steps.length);
          steps.push(name);
          break;
        }
        case 'bye': {
          if (steps == null) break;
          steps.push('bye');
          phase = 'complete';
          calls = typeof msg['calls'] === 'number' && msg['calls'] >= 0
            ? Math.floor(msg['calls'])
            : toolSteps.length;
          break;
        }
      }
    } else if (line.startsWith(IN)) {
      const msg = payload(line, IN);
      if (msg == null || steps == null || phase === 'complete') continue;
      switch (msg['type']) {
        case 'welcome':
          if (!steps.includes('welcome')) steps.push('welcome');
          break;
        case 'tool_result': {
          /* answer the matching outstanding call, else the oldest one */
          const call = typeof msg['call'] === 'number' ? msg['call'] : null;
          let idx = -1;
          if (call != null) {
            /* the frozen session's calls are 1-based in order */
            const ordinal = call - 1 - resolved;
            if (ordinal >= 0 && ordinal < toolSteps.length) idx = ordinal;
          }
          if (idx < 0) idx = 0;
          const stepIdx = toolSteps.splice(idx, 1)[0];
          if (stepIdx == null) break;
          resolved++;
          const mark = msg['ok'] === false ? '✗' : '✓';
          steps[stepIdx] = (steps[stepIdx] ?? '?') + mark;
          break;
        }
      }
    } else if (line.startsWith(COMPLETE_NOTE)) {
      /* the harness's own closing note follows the bye line; if a partial
         capture lost the bye, the note still closes the session honestly,
         and it carries the call count the bye would have */
      if (steps != null) {
        phase = 'complete';
        const m = /after (\d+) tool call/.exec(line);
        if (m) calls = Number(m[1]);
      }
    }
  }
  if (steps == null) return null;
  return { phase, steps, calls: phase === 'complete' ? calls : resolved + toolSteps.length };
}

/** The one-line session summary the run report carries:
    `hello→welcome→echo✓→uptime✓→bye, 2 tool calls`. */
export function sessionSummary(read: SessionRead): string {
  return `${read.steps.join('→')}, ${read.calls} tool ${read.calls === 1 ? 'call' : 'calls'}`;
}
