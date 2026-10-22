// The console's copies of the hub's GET answers (PLAN 30.4). Every JSON GET goes through api.ts's `call`, which
// revalidates it: the kept ETag goes out as `If-None-Match`, a 304 answers with the kept body and a 200 replaces it. So
// nothing is served without asking the hub. A screen paints the kept body at once (`getCached`) and the revalidated one
// a moment later. A bounded copy also persists in localStorage, so the next visit paints at once too. The copies belong
// to one hub, named by its origin and by the version and store schema of its last `hello`: a new version or schema starts
// empty, since its bodies may have another shape.

/** The copies kept in memory, in characters of response text. The least used go first. */
const MEMORY_CAP = 8_000_000;
/** The copies written to localStorage, in characters of their JSON: about 2 MB. */
const PERSIST_CAP = 2_000_000;
/** How long after a change the copies are written, so a burst of reads writes once. */
const PERSIST_DELAY_MS = 500;
/** The hub the copies belong to: the origin, version and schema of its last `hello`. */
const STAMP_KEY = 'apz.cache.hub';
/** The prefix of every key that holds copies; the copies of a hub this page no longer talks to are removed. */
const COPIES_PREFIX = 'apz.cache|';

/**
 * Routes whose answers may hold secrets: the vault's values and audit, the owner's sign-in, the hub-MCP tokens and a
 * provider's key. Their copies stay in memory and are never written to localStorage: any script of this origin can
 * read localStorage, and it outlives the tab, so a secret written there would outlive the session that fetched it.
 */
const SECRET_ROUTES = [
  /^\/v1\/vault(?=[/?]|$)/,
  /^\/v1\/auth(?=[/?]|$)/,
  /^\/v1\/hub-mcp\/tokens(?=[/?]|$)/,
  /^\/v1\/providers\/[^/?]+\/key(?=[/?]|$)/,
  // Transcripts hold what tools printed and models wrote (and they are large): kept in memory only.
  /^\/v1\/(agents|sessions)\/[^/?]+\/transcript(?=[/?]|$)/,
  /^\/v1\/owner\/conversations\/[^/?]+\/transcript(?=[/?]|$)/,
];

/** Whether a GET of `path` may hold a secret, so its copy is never persisted. */
export function isSecretRoute(path: string): boolean {
  return SECRET_ROUTES.some((re) => re.test(path));
}

/** What a `hello` says of the hub: its version and the schema of its store. */
export interface Hub {
  version: string;
  schema: number;
}

interface HubStamp extends Hub {
  origin: string;
}

/** One kept answer: its tag, its body as the hub sent it, and the request that brought it. */
interface Copy {
  etag: string;
  text: string;
  request: number;
  /** The last use (or the restore): the least used go first. */
  used: number;
  persist: boolean;
}

/** The persisted form of a copy: the same fields, the key holds the origin and the path. */
interface Stored {
  etag: string;
  text: string;
  used: number;
}

const copies = new Map<string, Copy>();
let memoryChars = 0;
/** Numbers requests and uses in the order they happen, so a late answer never replaces a newer one. */
let counter = 0;
let stamp: HubStamp | null = null;
let restored = false;
let flushTimer: ReturnType<typeof setTimeout> | undefined;

const originNow = (): string => (typeof location === 'undefined' ? '' : location.origin);
const keyOf = (path: string): string => originNow() + path;
const sizeOf = (key: string, c: Copy): number => key.length + c.etag.length + c.text.length;
const copiesKey = (s: HubStamp): string => `${COPIES_PREFIX}${s.origin}|${s.version}|${s.schema}`;

/** The number of a request about to be sent. */
export function nextRequest(): number {
  return ++counter;
}

/** The ETag kept for `path`, to send as `If-None-Match`, or undefined when none is kept. */
export function storedTag(path: string): string | undefined {
  restore();
  return copies.get(keyOf(path))?.etag;
}

/**
 * The last answer kept for `path`, parsed afresh (the caller may change it), or undefined when none is kept. A screen
 * paints this at once; the revalidated answer follows.
 */
export function getCached<T>(path: string): T | undefined {
  restore();
  const c = copies.get(keyOf(path));
  if (c === undefined) return undefined;
  c.used = ++counter;
  try {
    return JSON.parse(c.text) as T;
  } catch {
    return undefined;
  }
}

/** Keeps a 200 answer with its tag. A copy of a newer request's answer that is already kept stays. */
export function keep(path: string, etag: string, text: string, request: number): void {
  restore();
  const key = keyOf(path);
  const old = copies.get(key);
  if (old !== undefined && old.request > request) return;
  put(key, { etag, text, request, used: ++counter, persist: !isSecretRoute(path) });
  scheduleFlush();
}

/** Drops the copy of `path`: the hub refused it, or answered it without a tag, so the copy is no longer what it says. */
export function forget(path: string): void {
  restore();
  const key = keyOf(path);
  const c = copies.get(key);
  if (c === undefined) return;
  copies.delete(key);
  memoryChars -= sizeOf(key, c);
  scheduleFlush();
}

/**
 * A `hello` names the hub this socket reaches. When its version or schema differs from the hub the copies were kept for
 * (or it is another origin), those copies go, from memory and from storage, and the new name is written. Returns true
 * when a hub that was known has changed, so the screens should ask again.
 */
export function noteHub(hub: Hub): boolean {
  restore();
  const next: HubStamp = { origin: originNow(), version: hub.version, schema: hub.schema };
  const prev = stamp ?? readStamp();
  const same = prev !== null && prev.origin === next.origin && prev.version === next.version && prev.schema === next.schema;
  if (!same && prev !== null) {
    copies.clear();
    memoryChars = 0;
    dropStoredCopies(copiesKey(next));
  }
  stamp = next;
  writeStamp(next);
  scheduleFlush();
  return !same && prev !== null;
}

/**
 * Writes the copies of the stamped hub now: the most recently used, up to PERSIST_CAP characters. The page calls it as it
 * hides, so the last change is kept.
 */
export function flush(): void {
  if (flushTimer !== undefined) {
    clearTimeout(flushTimer);
    flushTimer = undefined;
  }
  if (stamp === null) return;
  const rows: string[] = [];
  let total = 0;
  for (const [key, c] of [...copies].filter(([, c]) => c.persist).sort((a, b) => b[1].used - a[1].used)) {
    const row = `${JSON.stringify(key)}:${JSON.stringify({ etag: c.etag, text: c.text, used: c.used } satisfies Stored)}`;
    if (total + row.length > PERSIST_CAP) break;
    total += row.length + 1;
    rows.push(row);
  }
  try {
    localStorage.setItem(copiesKey(stamp), `{${rows.join(',')}}`);
  } catch {
    // No storage, or it is full: the copies stay in memory, and the screens paint from the hub as before.
  }
}

/** Test hook: a page with no copies, no stamp and no write pending. */
export function resetCacheForTest(): void {
  copies.clear();
  memoryChars = 0;
  counter = 0;
  stamp = null;
  restored = false;
  if (flushTimer !== undefined) clearTimeout(flushTimer);
  flushTimer = undefined;
}

/** Sets a copy and keeps the memory count true; the least used copies go while the cap is passed. */
function put(key: string, c: Copy): void {
  const old = copies.get(key);
  if (old !== undefined) memoryChars -= sizeOf(key, old);
  copies.set(key, c);
  memoryChars += sizeOf(key, c);
  while (memoryChars > MEMORY_CAP) {
    let least: [string, Copy] | undefined;
    for (const row of copies) if (least === undefined || row[1].used < least[1].used) least = row;
    if (least === undefined) break;
    copies.delete(least[0]);
    memoryChars -= sizeOf(least[0], least[1]);
  }
}

/** Reads the copies of the stamped hub, once a page, so that the first paint has them. */
function restore(): void {
  if (restored) return;
  restored = true;
  const saved = readStamp();
  if (saved === null || saved.origin !== originNow()) return;
  stamp = saved;
  try {
    const rows: unknown = JSON.parse(localStorage.getItem(copiesKey(saved)) ?? 'null');
    if (typeof rows !== 'object' || rows === null) return;
    const list = Object.entries(rows)
      .filter((row): row is [string, Stored] => isStored(row[1]))
      .sort((a, b) => a[1].used - b[1].used);
    // Oldest first, so the restored order of use holds; each gets a use number newer than any before this page.
    for (const [key, s] of list) put(key, { etag: s.etag, text: s.text, request: 0, used: ++counter, persist: true });
  } catch {
    // Unreadable copies: the page starts without them.
  }
}

function isStored(v: unknown): v is Stored {
  if (typeof v !== 'object' || v === null) return false;
  const s = v as Partial<Stored>;
  return typeof s.etag === 'string' && typeof s.text === 'string' && typeof s.used === 'number';
}

function readStamp(): HubStamp | null {
  try {
    const s = JSON.parse(localStorage.getItem(STAMP_KEY) ?? 'null') as Partial<HubStamp> | null;
    if (s && typeof s.origin === 'string' && typeof s.version === 'string' && typeof s.schema === 'number') {
      return { origin: s.origin, version: s.version, schema: s.schema };
    }
  } catch {
    // No storage, or a stamp this build cannot read: the page starts with no copies.
  }
  return null;
}

function writeStamp(s: HubStamp): void {
  try {
    localStorage.setItem(STAMP_KEY, JSON.stringify(s));
  } catch {
    // No storage: the copies stay in memory.
  }
}

/** Removes every copies key but `keep` from storage: the copies of a hub this page no longer talks to. */
function dropStoredCopies(keep: string): void {
  try {
    const stale: string[] = [];
    for (let i = 0; i < localStorage.length; i++) {
      const k = localStorage.key(i);
      if (k !== null && k.startsWith(COPIES_PREFIX) && k !== keep) stale.push(k);
    }
    for (const k of stale) localStorage.removeItem(k);
  } catch {
    // No storage: nothing persisted to drop.
  }
}

function scheduleFlush(): void {
  flushTimer ??= setTimeout(flush, PERSIST_DELAY_MS);
}
