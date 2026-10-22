// The console's copies of the hub's GET answers (src/api-cache.ts) and the events client's resync (src/store.ts): a
// copy's tag goes out as If-None-Match and a 304 answers with the copy; a POST keeps nothing; a gap in seq, a reconnect
// and a hello with a new boot, version or schema ask the screens again; a new schema drops the persisted copies; the
// routes that can hold secrets are never persisted; the persisted copies stay within 2 MB. fetch, localStorage,
// location and WebSocket are fakes. Bundled with the other suites by test/tests.ts; it prints "cache tests: ok".

import { api, call, hubMcpApi, vaultApi } from '../src/api';
import { flush, getCached, isSecretRoute, noteHub, resetCacheForTest } from '../src/api-cache';
import { connect, on, RESYNC } from '../src/store';

let checks = 0;

function eq<T>(actual: T, expected: T, what: string): void {
  checks++;
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) throw new Error(`${what}: expected ${b}, got ${a}`);
}

// ---- Fakes -------------------------------------------------------------------------------------------------------

const ORIGIN = 'http://hub.test';
const STAMP_KEY = 'apz.cache.hub';
const copiesKeyOf = (version: string, schema: number): string => `apz.cache|${ORIGIN}|${version}|${schema}`;

const realLocation = Object.getOwnPropertyDescriptor(globalThis, 'location');
const realStorage = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
const realFetch = globalThis.fetch;
const realWebSocket = Object.getOwnPropertyDescriptor(globalThis, 'WebSocket');
const realSetTimeout = globalThis.setTimeout;
Object.defineProperty(globalThis, 'location', { value: { origin: ORIGIN, protocol: 'http:', host: 'hub.test' }, configurable: true, writable: true });

/** localStorage as a Map: the keys, the length and the removals the copies use. */
function useStorage(): Map<string, string> {
  const data = new Map<string, string>();
  const storage = {
    get length(): number {
      return data.size;
    },
    key: (i: number): string | null => [...data.keys()][i] ?? null,
    getItem: (k: string): string | null => data.get(k) ?? null,
    setItem: (k: string, v: string): void => {
      data.set(k, String(v));
    },
    removeItem: (k: string): void => {
      data.delete(k);
    },
  };
  Object.defineProperty(globalThis, 'localStorage', { value: storage, configurable: true, writable: true });
  return data;
}

/** What the fake hub answers, per path: its status, body and ETag (a GET whose If-None-Match names the tag gets a 304). */
interface Served {
  status: number;
  body?: unknown;
  etag?: string;
}
let served = new Map<string, Served>();
let requests: { method: string; path: string; ifNoneMatch: string | null; cache: string | null }[] = [];

globalThis.fetch = (async (input: string, init?: RequestInit) => {
  const method = init?.method ?? 'GET';
  const headers = (init?.headers ?? {}) as Record<string, string>;
  const ifNoneMatch = headers['if-none-match'] ?? null;
  requests.push({ method, path: input, ifNoneMatch, cache: init?.cache ?? null });
  const reply = served.get(input) ?? { status: 200, body: [] };
  if (method === 'GET' && reply.etag !== undefined && ifNoneMatch === reply.etag) {
    return new Response(null, { status: 304, headers: { etag: reply.etag } });
  }
  const out: Record<string, string> = { 'content-type': 'application/json' };
  if (reply.etag !== undefined) out['etag'] = reply.etag;
  return new Response(reply.body === undefined ? null : JSON.stringify(reply.body), { status: reply.status, headers: out });
}) as typeof fetch;

/** A WebSocket the store can open: the test calls its handlers as the hub would. */
class FakeSocket {
  static made: FakeSocket[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  constructor(
    readonly url: string,
    readonly protocol: string,
  ) {
    FakeSocket.made.push(this);
  }
  close(): void {
    // the test ends a socket by calling its onclose
  }
}

const frame = (event: string, seq: number, data: unknown): string => JSON.stringify({ event, seq, data });
const sleep = (ms: number): Promise<void> => new Promise((done) => realSetTimeout(done, ms));

// ---- Revalidation ------------------------------------------------------------------------------------------------

{
  resetCacheForTest();
  useStorage();
  requests = [];
  served = new Map([['/v1/projects', { status: 200, body: [{ id: 'p1' }], etag: 'W/"a1"' }]]);
  eq(await api.projects(), [{ id: 'p1' }], 'a GET answers with the body the hub sent');
  eq(requests[0]?.ifNoneMatch, null, 'a GET with no copy sends no If-None-Match');
  eq(await api.projects(), [{ id: 'p1' }], 'a GET whose copy is current answers with the copy');
  eq(requests[1]?.ifNoneMatch, 'W/"a1"', 'a GET with a copy sends the copy tag as If-None-Match');
  eq(requests[1]?.cache, 'no-store', 'the browser HTTP cache stays out of the revalidation');
  eq(getCached('/v1/projects'), [{ id: 'p1' }], 'the copy paints at once');

  served.set('/v1/projects', { status: 200, body: [{ id: 'p1' }, { id: 'p2' }], etag: 'W/"a2"' });
  eq((await api.projects()).length, 2, 'a changed body answers 200 with the new body');
  eq(getCached<unknown[]>('/v1/projects')?.length, 2, 'the copy follows the newest answer');

  // Each read is a fresh value: a view that sorts or edits what it was given does not change the copy.
  const painted = getCached<{ id: string }[]>('/v1/projects');
  painted?.push({ id: 'zz' });
  eq(getCached<unknown[]>('/v1/projects')?.length, 2, 'a read is a fresh value, not the copy itself');
}

// ---- What is never kept: writes, refusals -------------------------------------------------------------------------

{
  resetCacheForTest();
  useStorage();
  served = new Map([['/v1/tasks', { status: 200, body: { id: 't1' }, etag: 'W/"t1"' }]]);
  await call('POST', '/v1/tasks', { goal: 'x' });
  eq(getCached('/v1/tasks'), undefined, 'a POST answer is never kept');

  served.set('/v1/projects/p9', { status: 200, body: { id: 'p9' }, etag: 'W/"p9"' });
  await api.project('p9');
  eq(getCached('/v1/projects/p9') !== undefined, true, 'a GET answer is kept');
  served.set('/v1/projects/p9', { status: 404, body: { error: 'project not found' } });
  let refused = false;
  try {
    await api.project('p9');
  } catch {
    refused = true;
  }
  eq(refused, true, 'a refused GET throws as before');
  eq(getCached('/v1/projects/p9'), undefined, 'a refused GET drops its copy');
}

// ---- Persistence, the visit that paints at once, and a new hub --------------------------------------------------

{
  resetCacheForTest();
  const storage = useStorage();
  requests = [];
  served = new Map([['/v1/overview', { status: 200, body: { agents: 2 }, etag: 'W/"o1"' }]]);
  await api.overview();
  eq(noteHub({ version: '1.0.0', schema: 3 }), false, 'the first hello names no change');
  flush();
  eq(storage.get(copiesKeyOf('1.0.0', 3)) !== undefined, true, 'the copies persist under the origin, version and schema of the hello');

  // A visit: the page is loaded again and paints the persisted copy before any answer arrives.
  resetCacheForTest();
  eq(getCached('/v1/overview'), { agents: 2 }, 'a visit paints the persisted copy at once');
  eq(noteHub({ version: '1.0.0', schema: 3 }), false, 'the same hub again changes nothing');
  eq(getCached('/v1/overview') !== undefined, true, 'and its copies stay');

  eq(noteHub({ version: '1.0.0', schema: 4 }), true, 'a hello with a new schema is a change');
  eq(storage.get(copiesKeyOf('1.0.0', 3)), undefined, 'a hello with a new schema drops the copies persisted for the old one');
  eq(getCached('/v1/overview'), undefined, 'and the page holds none of them');
  eq(JSON.parse(storage.get(STAMP_KEY) ?? 'null'), { origin: ORIGIN, version: '1.0.0', schema: 4 }, 'the stamp names the new schema');
}

// ---- Secret routes stay in memory ----------------------------------------------------------------------------------

{
  resetCacheForTest();
  const storage = useStorage();
  served = new Map([
    ['/v1/projects', { status: 200, body: [{ id: 'p1' }], etag: 'W/"s1"' }],
    ['/v1/vault/secrets', { status: 200, body: [{ name: 'token' }], etag: 'W/"s2"' }],
    ['/v1/auth/check', { status: 200, body: { ok: true }, etag: 'W/"s3"' }],
    ['/v1/hub-mcp/tokens', { status: 200, body: { tokens: [] }, etag: 'W/"s4"' }],
    ['/v1/providers/p1/key', { status: 200, body: { key: 'k' }, etag: 'W/"s5"' }],
  ]);
  noteHub({ version: '1.0.0', schema: 3 });
  await api.projects();
  await vaultApi.entries();
  await api.authCheck();
  await hubMcpApi.tokens();
  await call('GET', '/v1/providers/p1/key');
  flush();
  const persisted = JSON.parse(storage.get(copiesKeyOf('1.0.0', 3)) ?? '{}') as Record<string, unknown>;
  eq(Object.keys(persisted), [`${ORIGIN}/v1/projects`], 'only the routes that can hold no secret are persisted');
  eq(getCached('/v1/vault/secrets') !== undefined, true, 'a secret route is still kept in memory and revalidated');
  eq(
    [isSecretRoute('/v1/vault/audit?limit=5'), isSecretRoute('/v1/auth/check'), isSecretRoute('/v1/hub-mcp/tokens'), isSecretRoute('/v1/providers/p1/key')],
    [true, true, true, true],
    'the deny list names the vault, the sign-in, the hub tokens and a provider key',
  );
  eq([isSecretRoute('/v1/vaultx'), isSecretRoute('/v1/providers/p1'), isSecretRoute('/v1/hub-mcp/status')], [false, false, false], 'and no other route');
}

// ---- The persisted copies stay within 2 MB, the most recently used first ------------------------------------------

{
  resetCacheForTest();
  const storage = useStorage();
  noteHub({ version: '1.0.0', schema: 3 });
  for (const n of [1, 2, 3]) {
    served.set(`/v1/big/${n}`, { status: 200, body: { blob: 'x'.repeat(900_000) }, etag: `W/"big${n}"` });
    await call('GET', `/v1/big/${n}`);
  }
  flush();
  const persisted = JSON.parse(storage.get(copiesKeyOf('1.0.0', 3)) ?? '{}') as Record<string, unknown>;
  eq(Object.keys(persisted).sort(), [`${ORIGIN}/v1/big/2`, `${ORIGIN}/v1/big/3`], 'the least recently used copy is left out of storage');
}

// ---- The events client: seq gaps, reconnects, a new hub, models --------------------------------------------------

{
  resetCacheForTest();
  const storage = useStorage();
  Object.defineProperty(globalThis, 'WebSocket', { value: FakeSocket, configurable: true, writable: true });
  // The store's stable-connection timer is a minute long: unref it, so the run does not wait for it.
  globalThis.setTimeout = ((fn: () => void, ms?: number, ...rest: unknown[]) => {
    const t = realSetTimeout(fn, ms, ...rest);
    if (ms === 60_000) t.unref();
    return t;
  }) as typeof setTimeout;

  served = new Map([['/v1/projects', { status: 200, body: [{ id: 'p1' }], etag: 'W/"p1"' }]]);
  requests = [];
  let resyncs = 0;
  let models = 0;
  // The reads a screen starts on a resync: they are awaited before the block ends, so none is left in flight.
  const reads: Promise<unknown>[] = [];
  const offResync = on(RESYNC, () => {
    resyncs++;
    // A screen asks again: its read revalidates against the copy it holds.
    reads.push(api.projects());
  });
  const offModels = on('models', () => {
    models++;
  });

  connect();
  const first = FakeSocket.made[0]!;
  first.onopen?.();
  first.onmessage?.({ data: frame('hello', 2, { boot: 'b1', version: '1.0.0', schema: 3, seq: 2 }) });
  await api.projects();
  flush();
  eq(storage.get(copiesKeyOf('1.0.0', 3)) !== undefined, true, 'the hello names the copies: they persist under its version and schema');

  first.onmessage?.({ data: frame('task', 3, {}) });
  eq(resyncs, 0, 'the next seq is no gap');
  first.onmessage?.({ data: frame('task', 5, {}) });
  eq(resyncs, 1, 'a gap in seq asks the screens again');
  eq(requests.filter((r) => r.path === '/v1/projects').at(-1)?.ifNoneMatch, 'W/"p1"', 'the gap revalidates a copy: the screen asks with its tag');
  first.onmessage?.({ data: frame('models', 6, {}) });
  eq([resyncs, models], [1, 1], 'a models event reaches its listeners and is no gap');

  // A reconnect: the socket closes, the store waits and opens another; its hello asks the screens again.
  first.onclose?.();
  await sleep(1100);
  const second = FakeSocket.made[1]!;
  second.onopen?.();
  second.onmessage?.({ data: frame('hello', 6, { boot: 'b1', version: '1.0.0', schema: 3, seq: 6 }) });
  eq(resyncs, 2, 'a reconnect asks the screens again');

  // A hub upgrade on the new socket: its schema differs, so the copies persisted for the old schema go.
  second.onmessage?.({ data: frame('hello', 6, { boot: 'b2', version: '1.1.0', schema: 4, seq: 6 }) });
  eq(resyncs, 3, 'a new version or schema asks the screens again');
  eq(storage.get(copiesKeyOf('1.0.0', 3)), undefined, 'a hello with a new schema drops the persisted copies');
  eq(getCached('/v1/projects'), undefined, 'and the page holds none of them');

  second.onmessage?.({ data: frame('hello', 6, { boot: 'b3', version: '1.1.0', schema: 4, seq: 6 }) });
  eq(resyncs, 4, 'a new boot asks the screens again');

  // A wire event after a hello continues from the hello's seq.
  second.onmessage?.({ data: frame('task', 7, {}) });
  eq(resyncs, 4, 'the seq after the hello is no gap');

  await Promise.allSettled(reads);
  offResync();
  offModels();
  globalThis.setTimeout = realSetTimeout;
}

// ---- Restore the page's globals ----------------------------------------------------------------------------------

if (realFetch) globalThis.fetch = realFetch;
if (realWebSocket) Object.defineProperty(globalThis, 'WebSocket', realWebSocket);
else delete (globalThis as { WebSocket?: unknown }).WebSocket;
if (realStorage) Object.defineProperty(globalThis, 'localStorage', realStorage);
else delete (globalThis as { localStorage?: unknown }).localStorage;
if (realLocation) Object.defineProperty(globalThis, 'location', realLocation);
else delete (globalThis as { location?: unknown }).location;
resetCacheForTest();

console.log(`cache tests: ok (${checks} checks)`);
