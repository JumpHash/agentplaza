// The IDE terminal's logic with no DOM and no xterm: the socket's frames (crates/apid/src/ide_term.rs), base64 that
// carries UTF-8 bytes safely, the offset a client resumes from, the reconnect schedule and the session keys. The
// terminal view (ide-terminal.ts) builds on these; the tests load this file alone.

/** The subprotocol the terminal socket is asked for. */
export const PROTOCOL = 'apz.ide.terminal.v1';
/** Terminal sockets one workspace may hold open (the hub's cap). */
export const MAX_TERMINALS = 4;
/** The waits before a reconnect; the last one repeats. */
export const RECONNECT_DELAYS_MS = [1_000, 2_000, 5_000, 10_000] as const;
/** How often a socket sends a ping, well inside the hub's ten-minute idle close. */
export const PING_MS = 30_000;
/** Typed input goes out in frames of at most this many bytes (the hub's write chunk is 32 KiB). */
export const INPUT_CHUNK_BYTES = 16 * 1024;

// ---- base64 over bytes -------------------------------------------------------------------------------------------

/** Base64 of raw bytes. Input is bytes, never a JS string, so multi-byte UTF-8 survives the round trip. */
export function bytesToBase64(bytes: Uint8Array): string {
  const CHUNK = 0x8000;
  let binary = '';
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(binary);
}

/** The bytes of a base64 string. Throws when the text is not base64. */
export function base64ToBytes(text: string): Uint8Array {
  const binary = atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

/** Splits input bytes into frames of at most `size` bytes. */
export function splitInput(bytes: Uint8Array, size: number = INPUT_CHUNK_BYTES): Uint8Array[] {
  const chunks: Uint8Array[] = [];
  for (let i = 0; i < bytes.length; i += size) chunks.push(bytes.subarray(i, i + size));
  return chunks;
}

// ---- the socket ---------------------------------------------------------------------------------------------------

/** The terminal socket for a workspace on this console's origin (the same scheme rule as the live layer's /v1/ws). */
export function terminalSocketUrl(workspace: string, loc: { protocol: string; host: string }): string {
  const scheme = loc.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${scheme}//${loc.host}/v1/ide/terminal/${encodeURIComponent(workspace)}`;
}

/** The client frames, as JSON text. `cwd` is sent only when it names a directory. */
export function openFrame(cols: number, rows: number, cwd?: string): string {
  const frame: Record<string, unknown> = { t: 'open', cols, rows };
  if (cwd) frame.cwd = cwd;
  return JSON.stringify(frame);
}
export function attachFrame(term: string, offset: number): string {
  return JSON.stringify({ t: 'attach', term, offset });
}
export function inputFrame(bytes: Uint8Array): string {
  return JSON.stringify({ t: 'in', data: bytesToBase64(bytes) });
}
export function resizeFrame(cols: number, rows: number): string {
  return JSON.stringify({ t: 'resize', cols, rows });
}
export function killFrame(): string {
  return '{"t":"kill"}';
}
export function pingFrame(): string {
  return '{"t":"ping"}';
}

/** A frame from the hub. */
export type ServerFrame =
  | { t: 'opened'; term: string }
  | { t: 'out'; data: Uint8Array; offset: number }
  | { t: 'lost'; bytes: number }
  | { t: 'exit'; code: number | null }
  | { t: 'error'; message: string }
  | { t: 'pong' };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
function isCount(value: unknown): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;
}

/** Reads one hub frame. Anything that is not a frame of the protocol is null (the caller ignores it). */
export function parseServerFrame(text: string): ServerFrame | null {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  if (!isRecord(value)) return null;
  switch (value.t) {
    case 'opened':
      return typeof value.term === 'string' && value.term !== '' ? { t: 'opened', term: value.term } : null;
    case 'out': {
      if (typeof value.data !== 'string' || !isCount(value.offset)) return null;
      try {
        return { t: 'out', data: base64ToBytes(value.data), offset: value.offset };
      } catch {
        return null;
      }
    }
    case 'lost':
      return isCount(value.bytes) ? { t: 'lost', bytes: value.bytes } : null;
    case 'exit':
      return { t: 'exit', code: typeof value.code === 'number' ? value.code : null };
    case 'error':
      return typeof value.message === 'string' ? { t: 'error', message: value.message } : null;
    case 'pong':
      return { t: 'pong' };
    default:
      return null;
  }
}

/** The offset a client resumes from once it holds an `out` frame: that frame's offset plus its byte count. */
export function nextOffset(out: { offset: number; data: Uint8Array }): number {
  return out.offset + out.data.length;
}

/** The wait before reconnect attempt `attempt` (0 is the first): 1 s, 2 s, 5 s, then 10 s for good. */
export function reconnectDelay(attempt: number): number {
  const step = Math.min(Math.max(0, Math.floor(attempt)), RECONNECT_DELAYS_MS.length - 1);
  return RECONNECT_DELAYS_MS[step] ?? RECONNECT_DELAYS_MS[RECONNECT_DELAYS_MS.length - 1] ?? 10_000;
}

/** Text safe to print in the terminal: control characters (escape sequences included) are dropped. */
export function plainText(text: string): string {
  return text.replace(/[\u0000-\u001f\u007f]/g, '').trim();
}

// ---- session storage ----------------------------------------------------------------------------------------------

/** What a tab keeps so a reload re-attaches: the guest terminal's id and the output offset it holds. */
export interface StoredSession {
  term: string;
  offset: number;
}

/** The part of Storage the terminal uses, so tests can pass a plain object. */
export interface KeyValueStore {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

/** The page's sessionStorage, or undefined where it is absent or blocked. */
export function browserSession(): KeyValueStore | undefined {
  try {
    return typeof sessionStorage === 'undefined' ? undefined : sessionStorage;
  } catch {
    return undefined;
  }
}

/** One key per workspace and tab slot (slots are "0" to "3"), so two tabs of one workspace never share a session. */
export function sessionKey(workspace: string, slot: string): string {
  return `apz.ide.terminal:${encodeURIComponent(workspace)}:${slot}`;
}

/** The stored session, or null when there is none, it is malformed, or storage cannot be read. */
export function readSession(store: KeyValueStore | undefined, key: string): StoredSession | null {
  if (!store) return null;
  try {
    const raw = store.getItem(key);
    if (raw === null) return null;
    const value: unknown = JSON.parse(raw);
    if (!isRecord(value) || typeof value.term !== 'string' || value.term === '' || !isCount(value.offset)) return null;
    return { term: value.term, offset: value.offset };
  } catch {
    return null;
  }
}

/** Stores a session. A storage that refuses the write leaves the terminal working without a reload re-attach. */
export function writeSession(store: KeyValueStore | undefined, key: string, session: StoredSession): void {
  try {
    store?.setItem(key, JSON.stringify(session));
  } catch {
    // Quota or blocked storage: the terminal still works in this page.
  }
}

export function clearSession(store: KeyValueStore | undefined, key: string): void {
  try {
    store?.removeItem(key);
  } catch {
    // Nothing stored, or storage is blocked: nothing to clear.
  }
}

/**
 * The terminals the stored sessions hold for one workspace: one per slot that has a session. This is the list the
 * terminal module keeps (a session is stored while its shell lives, and cleared when the shell ends or is killed).
 */
export function storedTerminalCount(store: KeyValueStore | undefined, workspace: string): number {
  let count = 0;
  for (let slot = 0; slot < MAX_TERMINALS; slot++) {
    if (readSession(store, sessionKey(workspace, String(slot)))) count++;
  }
  return count;
}
