/* Serial text handling: the append-only per-VM line cache, line kinds for the
   transcript's narrow column, and the guest's command set. Guest bytes are
   untrusted — this module only ever produces strings for textContent. */

export interface SerialCache {
  raw: string;
  lines: string[];
  /** The UTF-8 byte length of `raw` — the unit the daemon's serial offsets speak. */
  bytes: number;
}

/* ---------------------------------------------------------- byte offsets
   The event stream's `serial` frames name a byte offset into the VM's
   capture (a Vec<u8> daemon-side), not a string index: for ASCII the two
   agree, but any multibyte guest byte would drift a naive .length merge.
   The line cache therefore tracks the byte length of what it holds, and a
   re-delivery is clipped by bytes. */

const encoder = new TextEncoder();
const decoder = new TextDecoder();
const ASCII_ONLY = /^[\x00-\x7F]*$/;

/** The UTF-8 byte length of `text`. */
export function utf8ByteLength(text: string): number {
  return ASCII_ONLY.test(text) ? text.length : encoder.encode(text).length;
}

/**
 * `text` with its first `bytes` UTF-8 bytes dropped — a re-delivered stretch
 * of a serial event. The store's copy and the daemon's capture decode the
 * same buffer, so the clip point always lands on a character boundary; a
 * capture that went through a lossy decode misaligns instead and heals
 * through the gap path (a detail GET), never through a wrong append.
 */
export function clipLeadingBytes(text: string, bytes: number): string {
  if (bytes <= 0) return text;
  if (ASCII_ONLY.test(text)) return text.slice(bytes); // ascii: bytes are indices; past the end is ''
  const buf = encoder.encode(text);
  if (bytes >= buf.length) return '';
  return decoder.decode(buf.subarray(bytes));
}

/** What one serial event means against what the store already holds. */
export type SerialDelta =
  | { kind: 'append'; text: string } // the unseen stretch, ready to append
  | { kind: 'dup' } // a re-delivery carrying nothing new
  | { kind: 'gap' }; // bytes were missed — a detail GET reconciles

/**
 * The offset-idempotent merge decision, pure so the harness can test it
 * directly: `have` is the byte length of the store's copy, `offset` the
 * capture byte the event's `text` starts at. offset == have appends;
 * offset < have clips the re-delivered head and keeps only what is new;
 * offset > have is a gap (a missed poll or frame), never an append.
 */
export function serialDelta(have: number, offset: number, text: string): SerialDelta {
  if (offset > have) return { kind: 'gap' };
  const kept = clipLeadingBytes(text, have - offset);
  return kept === '' ? { kind: 'dup' } : { kind: 'append', text: kept };
}

/**
 * Split a VM's serial capture into lines, incrementally: the daemon's capture
 * only ever grows, so a seen prefix is never re-split. The guest's console
 * speaks CRLF; the \r is stripped. The final partial line (no \n yet) is kept.
 */
export function linesOf(cache: Map<string, SerialCache>, key: string, raw: string | undefined): string[] {
  const text = raw == null ? '' : String(raw);
  const cached = cache.get(key);
  if (cached && cached.raw === text) return cached.lines;
  if (!cached || !text.startsWith(cached.raw)) {
    /* fresh vm or the daemon replaced the buffer: re-split wholesale */
    const lines = text === '' ? [] : text.replace(/\r\n/g, '\n').replace(/\n$/, '').split('\n');
    cache.set(key, { raw: text, lines, bytes: utf8ByteLength(text) });
    return lines;
  }
  /* append-only growth; the last line may still be on its way (no \n yet) */
  const lines = cached.lines;
  const grown = text.slice(cached.raw.length);
  const parts = grown.replace(/\r/g, '').split('\n');
  let i = 0;
  if (cached.raw !== '' && !cached.raw.endsWith('\n') && lines.length) {
    lines[lines.length - 1] = (lines[lines.length - 1] ?? '') + (parts[0] ?? '');
    i = 1;
  }
  for (; i < parts.length; i++) lines.push(parts[i] ?? '');
  if (text.endsWith('\n')) lines.pop();
  cached.raw = text;
  cached.bytes += utf8ByteLength(grown);
  return lines;
}

/** The transcript's kind column: boot lines, heartbeats, guest chatter, replies,
    and the vsock wire's two directions (the frozen session's own lines). */
export function kindOf(line: string): string {
  if (line.startsWith('[vsock->]')) return 'out';
  if (line.startsWith('[vsock<-]')) return 'in';
  if (line.startsWith('plaza guest alive: heartbeat')) return 'beat';
  if (line.startsWith('plaza guest:')) return 'guest';
  if (line.startsWith('[')) return 'boot';
  if (/^(cpus:|Linux plaza-vm|commands:|MemTotal:|MemFree:|MemAvailable:|unknown command)/.test(line)) return 'reply';
  if (/^\d+\.\d+ +\d+\.\d+$/.test(line)) return 'reply';
  return 'line';
}

/** Lines that mark a VM's way out, flagged in the transcript margin. */
export function isExitLine(line: string): boolean {
  return /rebooting|parking|Power down/.test(line);
}

/** The harness's own note that a session closed well — the margin marks it
    like an exit, but in the register's green: the wire's work is done. */
export function isSessionMark(line: string): boolean {
  return line.startsWith('agent-harness: session complete');
}

/** The guest's command set: quick buttons beside the serial input. */
export const QUICK_COMMANDS: ReadonlyArray<readonly [string, string]> = [
  ['help', 'list the guest’s commands'],
  ['uname', 'kernel banner'],
  ['cpu', 'vCPU count and model'],
  ['mem', 'first lines of /proc/meminfo'],
  ['uptime', '/proc/uptime'],
  ['poweroff', 'the guest exits through ACPI S5 (slept); the VM leaves the running state'],
];
