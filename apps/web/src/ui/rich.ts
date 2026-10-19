// The rich-content classifier: the TypeScript port of crates/apz-rich (docs/design/workspace.md, section 3). Pure: no
// I/O, no clock, no network. It recognises exactly what the Rust crate recognises. Both run the same vectors
// (crates/apz-rich/tests/vectors.json, checked by test/tests.ts), so a rule changed on one side changes on the other.
//
// Spans and refs use the crate's JSON shapes (snake_case kinds, every field present and null when absent), so a
// vector's expected spans compare with this output directly.
//
// Positions are UTF-16 code units here and bytes in Rust; the rules only look at characters, so the results agree.
// Numbers are JS numbers: an id above 2^53 is not exact (the hub's ids are far below that).

/** An agent `@Name` can name. */
export interface AgentName {
  name: string;
  cid: number;
}

/** What the classifier may match against (the crate's `Ctx`). Every field is optional; null is absent. */
export interface RichCtx {
  /** The project in view. Its presence makes `#N` a task. */
  project?: string | null;
  /** The workspace's known paths: files, and folders with a trailing `/`. A Set is used as it is (no copy). */
  paths?: ReadonlySet<string> | readonly string[] | null;
  /** The agents `@Name` may name. */
  agents?: readonly AgentName[] | null;
  /** The hub's own host. Its `/v1/files` and `/v1/library` links are library references. */
  hub_host?: string | null;
}

interface Repo {
  owner: string;
  repo: string;
}

/** A recognised reference. Every field of a kind is present. */
export type Ref =
  | (Repo & { kind: 'github_pull' | 'github_issue'; number: number })
  | (Repo & { kind: 'github_commit'; sha: string })
  | (Repo & { kind: 'github_repo' })
  | (Repo & { kind: 'github_file'; ref: string; path: string; line: number | null; dir: boolean })
  | (Repo & { kind: 'github_release'; tag: string })
  | (Repo & { kind: 'github_run'; id: number })
  | { kind: 'gist'; id: string }
  | { kind: 'agent'; name: string | null; cid: number | null }
  | { kind: 'mention_all' }
  | { kind: 'task'; number: number }
  | { kind: 'path'; path: string; line: number | null; dir: boolean }
  | { kind: 'library'; id: string }
  | { kind: 'library_tool'; name: string; version: number }
  | { kind: 'image' | 'video' | 'html' | 'link'; url: string };

/** A piece of a classified text. Concatenating every span's raw text gives the input back. */
export type Span =
  | { kind: 'text'; value: string }
  /** An inline code span, backticks included. Never rewritten. */
  | { kind: 'code'; value: string }
  /** `raw` is the matched text (the whole `[label](url)` for a markdown link); `text` is the label, else null. */
  | { kind: 'ref'; value: { raw: string; r: Ref; text: string | null } };

export type RefSpan = Extract<Span, { kind: 'ref' }>;

const U32_MAX = (1n << 32n) - 1n;
const U64_MAX = (1n << 64n) - 1n;
const IMAGE_EXTENSIONS = ['png', 'jpg', 'jpeg', 'gif', 'webp', 'avif', 'svg'];
const VIDEO_EXTENSIONS = ['mp4', 'webm', 'mov'];
const HTML_EXTENSIONS = ['html'];
/** Trailing characters of a bare URL that are text (the web renderer's rule). */
const URL_TRAILING = new Set(['.', ',', ';', ':', '!', '?', "'", '"', '*', '_', ')', ']']);
/** Characters around a word that are not part of it (`(src/a.rs),` is `src/a.rs`). */
const WORD_LEADING = new Set(['(', '[', '{', '"', "'", '<']);
const WORD_TRAILING = new Set(['.', ',', ';', ':', '!', '?', "'", '"', '*', '_', ')', ']', '}', '>']);
const RUST_WHITESPACE = new RegExp('^[\\t\\n\\v\\f\\r \\u0085\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000]$');

interface Cx {
  hasProject: boolean;
  paths: ReadonlySet<string>;
  agents: readonly AgentName[];
  hubHost: string;
}

const EMPTY_PATHS: ReadonlySet<string> = new Set();

/** Splits `text` into spans. The four passes match the crate: inline code, markdown links and images, bare URLs, then
 * words (`#528`, `@Name`, `task #528`, workspace paths, hub routes). Code is never split. */
export function classify(text: string, ctx: RichCtx = {}): Span[] {
  const cx = context(ctx);
  let spans = codeSpans(text);
  spans = mapText(spans, (t) => markdownSpans(t, cx));
  spans = mapText(spans, (t) => urlSpans(t, cx));
  spans = mapText(spans, (t) => wordSpans(t, cx));
  return merge(spans);
}

/** The short chip label (section 3.1): `o/r #12`, `o/r @abc1234`, `r: path:10`, `#528`, `@Name`, `src/main.rs:42`. */
export function label(r: Ref): string {
  switch (r.kind) {
    case 'github_pull':
    case 'github_issue':
      return `${r.owner}/${r.repo} #${r.number}`;
    case 'github_commit':
      return `${r.owner}/${r.repo} @${r.sha.slice(0, 7)}`;
    case 'github_repo':
      return `${r.owner}/${r.repo}`;
    case 'github_file': {
      const shown = folderPath(r.path, r.dir);
      return r.line !== null ? `${r.repo}: ${shown}:${r.line}` : `${r.repo}: ${shown}`;
    }
    case 'github_release':
      return `${r.owner}/${r.repo} release ${r.tag}`;
    case 'github_run':
      return `${r.owner}/${r.repo} run ${r.id}`;
    case 'gist':
      return `gist ${r.id}`;
    case 'agent':
      return r.name !== null ? `@${r.name}` : `agent:${r.cid ?? 0}`;
    case 'mention_all':
      return '@all';
    case 'task':
      return `#${r.number}`;
    case 'path': {
      const shown = folderPath(r.path, r.dir);
      return r.line !== null ? `${shown}:${r.line}` : shown;
    }
    case 'library':
      return `file ${r.id}`;
    case 'library_tool':
      return `${r.name} v${r.version}`;
    case 'image':
    case 'video':
    case 'html': {
      const path = stripQuery(r.url);
      const last = path.split('/').pop() ?? '';
      return last !== '' ? last : path;
    }
    case 'link': {
      const bare = stripQuery(r.url);
      return (stripScheme(bare) ?? bare).replace(/\/+$/, '');
    }
  }
}

// ---- Context and helpers ---------------------------------------------------

function context(ctx: RichCtx): Cx {
  const paths = ctx.paths;
  let set: ReadonlySet<string> = EMPTY_PATHS;
  if (paths instanceof Set) set = paths as ReadonlySet<string>;
  else if (paths) set = new Set(paths as readonly string[]);
  return {
    hasProject: ctx.project != null,
    paths: set,
    agents: ctx.agents ?? [],
    hubHost: hostOf(ctx.hub_host ?? ''),
  };
}

const asciiLower = (s: string): string => s.replace(/[A-Z]+/g, (m) => m.toLowerCase());
const isWs = (c: string): boolean => RUST_WHITESPACE.test(c);
const isAsciiWs = (code: number): boolean => code === 32 || code === 9 || code === 10 || code === 12 || code === 13;

/** The first index at or after `from` of any of `chars`, or the text length when none. */
function firstOf(text: string, from: number, chars: string[]): number {
  let best = text.length;
  for (const c of chars) {
    const at = text.indexOf(c, from);
    if (at >= 0 && at < best) best = at;
  }
  return best;
}

/** `text` as an unsigned integer no larger than `max`, digits only; null when it does not parse. */
function unsignedDigits(text: string, max: bigint): number | null {
  if (!/^[0-9]+$/.test(text)) return null;
  const value = BigInt(text);
  return value > max ? null : Number(value);
}

/** `version.parse::<u32>()`: digits with an optional `+`. */
function parseVersion(text: string): number | null {
  const m = /^\+?([0-9]+)$/.exec(text);
  if (!m) return null;
  const value = BigInt(m[1] ?? '');
  return value > U32_MAX ? null : Number(value);
}

/** The lowercase host of a URL or of a bare host, without userinfo or port. "" when there is none. */
export function hostOf(text: string): string {
  const rest = stripScheme(text) ?? text;
  const authority = rest.slice(0, firstOf(rest, 0, ['/', '?', '#']));
  const hostPort = authority.slice(authority.lastIndexOf('@') + 1);
  return asciiLower(hostPort.split(':')[0] ?? '');
}

/** The URL without its scheme, when it has an http or https one (any case). */
function stripScheme(url: string): string | null {
  for (const scheme of ['https://', 'http://']) {
    if (url.length >= scheme.length && asciiLower(url.slice(0, scheme.length)) === scheme) return url.slice(scheme.length);
  }
  return null;
}

/** The URL without its query and fragment. */
function stripQuery(url: string): string {
  return url.slice(0, firstOf(url, 0, ['?', '#']));
}

/** A GitHub owner, repository or library name. */
function validName(text: string): boolean {
  return /^[A-Za-z0-9._-]+$/.test(text) && text !== '.' && text !== '..';
}

function folderPath(path: string, dir: boolean): string {
  return dir && !path.endsWith('/') ? `${path}/` : path;
}

function percentDecode(text: string): string {
  const bytes = new TextEncoder().encode(text);
  const out: number[] = [];
  for (let i = 0; i < bytes.length; ) {
    const byte = escapeAt(bytes, i);
    if (byte !== null) {
      out.push(byte);
      i += 3;
    } else {
      out.push(bytes[i] ?? 0);
      i += 1;
    }
  }
  return new TextDecoder().decode(new Uint8Array(out));
}

/** The byte of a `%XX` escape starting at `i`, if there is one. */
function escapeAt(bytes: Uint8Array, i: number): number | null {
  if (bytes[i] !== 37 || i + 2 >= bytes.length) return null;
  const hi = hexValue(bytes[i + 1] ?? 0);
  const lo = hexValue(bytes[i + 2] ?? 0);
  return hi === null || lo === null ? null : hi * 16 + lo;
}

function hexValue(b: number): number | null {
  if (b >= 48 && b <= 57) return b - 48;
  if (b >= 97 && b <= 102) return b - 87;
  if (b >= 65 && b <= 70) return b - 55;
  return null;
}

function isHex(text: string, min: number, max: number): boolean {
  return text.length >= min && text.length <= max && /^[0-9a-fA-F]+$/.test(text);
}

// ---- Span plumbing ---------------------------------------------------------

function mapText(spans: Span[], f: (text: string) => Span[]): Span[] {
  return spans.flatMap((span) => (span.kind === 'text' ? f(span.value) : [span]));
}

/** Adjacent text spans become one; empty text spans go. */
function merge(spans: Span[]): Span[] {
  const out: Span[] = [];
  for (const span of spans) {
    if (span.kind === 'text') {
      if (span.value === '') continue;
      const last = out[out.length - 1];
      if (last?.kind === 'text') last.value += span.value;
      else out.push({ kind: 'text', value: span.value });
    } else out.push(span);
  }
  return out;
}

function pushText(out: Span[], text: string): void {
  if (text !== '') out.push({ kind: 'text', value: text });
}

// ---- Pass 1: inline code ---------------------------------------------------

/** A backtick, one or more characters that are not a backtick or a newline, and a backtick. */
function codeSpans(text: string): Span[] {
  const out: Span[] = [];
  let plainFrom = 0;
  let i = 0;
  let open = text.indexOf('`', i);
  while (open >= 0) {
    const body = open + 1;
    const lineEnd = firstOf(text, body, ['\n']);
    const k = text.slice(body, lineEnd).indexOf('`');
    if (k > 0) {
      const close = body + k;
      pushText(out, text.slice(plainFrom, open));
      out.push({ kind: 'code', value: text.slice(open, close + 1) });
      plainFrom = close + 1;
      i = close + 1;
    } else {
      i = body;
    }
    open = text.indexOf('`', i);
  }
  pushText(out, text.slice(plainFrom));
  return out;
}

// ---- Pass 2: markdown links and images -------------------------------------

interface ParsedLink {
  end: number;
  label: string;
  target: string;
}

/** `[label](target)` and `![alt](target)` where the target is an http(s) URL, a hub route or a workspace path. */
function markdownSpans(text: string, cx: Cx): Span[] {
  const out: Span[] = [];
  let plainFrom = 0;
  let i = 0;
  let open = text.indexOf('[', i);
  while (open >= 0) {
    const link = parseLink(text, open);
    const r = link ? targetRef(link.target, cx) : null;
    if (link && r) {
      const bang = open > 0 && text.charAt(open - 1) === '!';
      const start = bang ? open - 1 : open;
      pushText(out, text.slice(plainFrom, start));
      out.push({ kind: 'ref', value: { raw: text.slice(start, link.end), r, text: link.label } });
      plainFrom = link.end;
      i = link.end;
    } else {
      i = open + 1;
    }
    open = text.indexOf('[', i);
  }
  pushText(out, text.slice(plainFrom));
  return out;
}

/** Parses `[label](target)` at `open`. The label has no `]` or newline; the target has no whitespace and its
 * parentheses are balanced at one level. */
function parseLink(text: string, open: number): ParsedLink | null {
  const close = text.indexOf(']', open + 1);
  if (close < 0) return null;
  const label = text.slice(open + 1, close);
  if (label === '' || label.includes('\n') || text.charAt(close + 1) !== '(') return null;
  const targetStart = close + 2;
  let p = targetStart;
  for (;;) {
    if (p >= text.length) return null;
    const ch = text.charAt(p);
    if (ch === ')') break;
    if (ch === '(') {
      let q = p + 1;
      while (q < text.length && text.charAt(q) !== ')' && text.charAt(q) !== '(' && !isAsciiWs(text.charCodeAt(q))) q++;
      if (text.charAt(q) !== ')') return null;
      p = q + 1;
    } else if (isAsciiWs(text.charCodeAt(p))) {
      return null;
    } else {
      p++;
    }
  }
  return { end: p + 1, label, target: text.slice(targetStart, p) };
}

/** The reference a markdown target names: an http(s) URL, a hub route or a workspace path. */
function targetRef(target: string, cx: Cx): Ref | null {
  if (stripScheme(target) !== null) return classifyUrl(target, cx);
  return localRef(target, cx);
}

// ---- Pass 3: bare URLs -----------------------------------------------------

function urlSpans(text: string, cx: Cx): Span[] {
  const out: Span[] = [];
  let plainFrom = 0;
  let i = 0;
  let start = findScheme(text, i);
  while (start >= 0) {
    const end = firstUrlStop(text, start);
    let stop = end;
    while (stop > start && URL_TRAILING.has(text.charAt(stop - 1))) stop--;
    const r = classifyUrl(text.slice(start, stop), cx);
    if (r) {
      pushText(out, text.slice(plainFrom, start));
      out.push({ kind: 'ref', value: { raw: text.slice(start, stop), r, text: null } });
      plainFrom = stop;
    }
    i = end;
    start = findScheme(text, i);
  }
  pushText(out, text.slice(plainFrom));
  return out;
}

/** The first index at or after `from` that is whitespace, `<` or `>`; the text length when there is none. */
function firstUrlStop(text: string, from: number): number {
  for (let i = from; i < text.length; i++) {
    const c = text.charAt(i);
    if (c === '<' || c === '>' || isWs(c)) return i;
  }
  return text.length;
}

/** The next `http://` or `https://` at or after `from`, any case. */
function findScheme(text: string, from: number): number {
  for (let p = from; p < text.length; p++) {
    if (asciiLower(text.charAt(p)) !== 'h') continue;
    if (asciiLower(text.slice(p, p + 7)) === 'http://' || asciiLower(text.slice(p, p + 8)) === 'https://') return p;
  }
  return -1;
}

// ---- Pass 4: words ---------------------------------------------------------

interface Token {
  a: number;
  b: number;
}

/** Maximal runs of non-whitespace. */
function tokensOf(text: string): Token[] {
  const out: Token[] = [];
  let start = -1;
  for (let i = 0; i < text.length; i++) {
    if (isWs(text.charAt(i))) {
      if (start >= 0) {
        out.push({ a: start, b: i });
        start = -1;
      }
    } else if (start < 0) {
      start = i;
    }
  }
  if (start >= 0) out.push({ a: start, b: text.length });
  return out;
}

/** A token without the brackets and punctuation around it. */
function trimToken(text: string, a0: number, b0: number): Token {
  let a = a0;
  let b = b0;
  while (a < b && WORD_LEADING.has(text.charAt(a))) a++;
  while (b > a && WORD_TRAILING.has(text.charAt(b - 1))) b--;
  return { a, b };
}

/** Words: whitespace-separated tokens, with the punctuation around a word left as text. */
function wordSpans(text: string, cx: Cx): Span[] {
  const tokens = tokensOf(text);
  const out: Span[] = [];
  let plainFrom = 0;
  let i = 0;
  while (i < tokens.length) {
    const raw = tokens[i];
    if (!raw) break;
    const { a, b } = trimToken(text, raw.a, raw.b);
    if (a >= b) {
      i++;
      continue;
    }
    // Two words: `task #528`. `PR #12` and `pull #12` are text, and so is their `#12`. The gap is spaces and tabs only,
    // and the first word has no trailing punctuation.
    const next = tokens[i + 1];
    if (next && b === raw.b && /^[ \t]+$/.test(text.slice(raw.b, next.a))) {
      const { a: c, b: d } = trimToken(text, next.a, next.b);
      const word = asciiLower(text.slice(a, b));
      const number = hashNumber(text.slice(c, d));
      if (word === 'task' && number !== null) {
        pushText(out, text.slice(plainFrom, a));
        out.push({ kind: 'ref', value: { raw: text.slice(a, d), r: { kind: 'task', number }, text: null } });
        plainFrom = d;
        i += 2;
        continue;
      }
      const isPr = word === 'pr' || word === 'pull';
      if (isPr && number !== null) {
        i += 2;
        continue;
      }
    }
    const r = singleWord(text.slice(a, b), cx);
    if (r) {
      pushText(out, text.slice(plainFrom, a));
      out.push({ kind: 'ref', value: { raw: text.slice(a, b), r, text: null } });
      plainFrom = b;
    }
    i++;
  }
  pushText(out, text.slice(plainFrom));
  return out;
}

/** One word: `#N` (a task, in a project), `@all`, `@Name`, `agent:<cid>`, a hub route, or a workspace path. */
function singleWord(word: string, cx: Cx): Ref | null {
  const number = hashNumber(word);
  if (number !== null) return cx.hasProject ? { kind: 'task', number } : null;
  if (word.startsWith('@')) {
    const name = word.slice(1);
    if (asciiLower(name) === 'all') return { kind: 'mention_all' };
    return cx.agents.some((a) => a.name === name) ? { kind: 'agent', name, cid: null } : null;
  }
  if (word.startsWith('agent:')) {
    const digits = word.slice(6);
    const cid = digits !== '' ? unsignedDigits(digits, U64_MAX) : null;
    return cid === null ? null : { kind: 'agent', name: null, cid };
  }
  return localRef(word.startsWith('file:') ? word.slice(5) : word, cx);
}

/** A relative target: a hub route (`/v1/files/<id>`), or a workspace path with an optional line. */
function localRef(target: string, cx: Cx): Ref | null {
  if (target.startsWith('/v1/')) {
    const segments = target.slice(0, firstOf(target, 0, ['?', '#'])).split('/').filter((s) => s !== '');
    return hubRoute(segments);
  }
  const [path, line] = splitLine(target);
  if (path === '') return null;
  if (cx.paths.has(path)) {
    // A file as given, or a folder given with its `/`. A folder has no line.
    const dir = path.endsWith('/');
    if (dir && line !== null) return null;
    return { kind: 'path', path, line, dir };
  }
  // A folder given without its `/`.
  const folder = `${path}/`;
  if (line === null && !path.endsWith('/') && cx.paths.has(folder)) return { kind: 'path', path: folder, line: null, dir: true };
  return null;
}

/** `path:12` and `path#L12` give the line; anything else is the path itself. */
function splitLine(target: string): [string, number | null] {
  const hashAt = target.lastIndexOf('#L');
  if (hashAt >= 0) {
    const line = allDigits(target.slice(hashAt + 2));
    if (line !== null) return [target.slice(0, hashAt), line];
  }
  const colonAt = target.lastIndexOf(':');
  if (colonAt >= 0) {
    const line = allDigits(target.slice(colonAt + 1));
    if (line !== null) return [target.slice(0, colonAt), line];
  }
  return [target, null];
}

function allDigits(text: string): number | null {
  return unsignedDigits(text, U32_MAX);
}

/** `#528` gives 528; anything else null. */
function hashNumber(word: string): number | null {
  return word.startsWith('#') ? unsignedDigits(word.slice(1), U64_MAX) : null;
}

// ---- URLs ------------------------------------------------------------------

/** What an http(s) URL refers to. `link` for anything not recognised. Null when the text is not an http(s) URL with a
 * host. */
export function classifyUrl(url: string, cx: Cx): Ref | null {
  const afterScheme = stripScheme(url);
  if (afterScheme === null) return null;
  const hashAt = afterScheme.indexOf('#');
  const withoutFragment = hashAt >= 0 ? afterScheme.slice(0, hashAt) : afterScheme;
  const fragment = hashAt >= 0 ? afterScheme.slice(hashAt + 1) : null;
  const queryAt = withoutFragment.indexOf('?');
  const location = queryAt >= 0 ? withoutFragment.slice(0, queryAt) : withoutFragment;
  const slashAt = location.indexOf('/');
  const authority = slashAt >= 0 ? location.slice(0, slashAt) : location;
  const path = slashAt >= 0 ? location.slice(slashAt) : '';
  const host = hostOf(authority);
  if (host === '') return null;
  const line = fragment !== null ? lineOfFragment(fragment) : null;
  const segments = path.split('/').filter((s) => s !== '');

  // The first route that recognises the URL wins; the host decides which routes apply.
  let routed: Ref | null = null;
  if (host === 'gist.github.com') routed = gist(segments);
  if (routed === null && (host === 'github.com' || host === 'www.github.com')) routed = forge(segments, line);
  if (routed === null && cx.hubHost !== '' && host === cx.hubHost) routed = hubRoute(segments);
  return routed ?? media(url, segments) ?? { kind: 'link', url };
}

/** A hub route of the library or the files: `/v1/files/<id>` and `/v1/library/<name>/<version>`. */
function hubRoute(segments: string[]): Ref | null {
  if (segments[0] === 'v1' && segments[1] === 'library' && segments.length >= 4 && validName(segments[2] ?? '')) {
    const version = parseVersion(segments[3] ?? '');
    return version === null ? null : { kind: 'library_tool', name: segments[2] ?? '', version };
  }
  if (segments[0] === 'v1' && segments[1] === 'files' && segments.length >= 3 && validName(segments[2] ?? '')) {
    return { kind: 'library', id: segments[2] ?? '' };
  }
  return null;
}

function gist(segments: string[]): Ref | null {
  const id = segments.length === 1 ? segments[0] : segments.length === 2 ? segments[1] : undefined;
  return id !== undefined && /^[A-Za-z0-9]+$/.test(id) ? { kind: 'gist', id } : null;
}

/** GitHub's routes: the pull, issue, commit, release, run and file pages of a repository. */
function forge(segments: string[], line: number | null): Ref | null {
  if (segments.length < 2) return null;
  const owner = segments[0] ?? '';
  const repo = segments[1] ?? '';
  if (!validName(owner) || !validName(repo)) return null;
  const rest = segments.slice(2);
  const first = rest[0];
  const second = rest[1];
  const third = rest[2];

  if (rest.length === 0) return { kind: 'github_repo', owner, repo };
  if ((first === 'pull' || first === 'pulls') && rest.length === 2) {
    const number = unsignedDigits(second ?? '', U64_MAX);
    if (number === null) return null;
    return { kind: 'github_pull', owner, repo, number };
  }
  if ((first === 'issues' || first === 'issue') && rest.length === 2) {
    const number = unsignedDigits(second ?? '', U64_MAX);
    if (number === null) return null;
    return { kind: 'github_issue', owner, repo, number };
  }
  if (first === 'commit' && rest.length === 2 && isHex(second ?? '', 7, 40)) {
    return { kind: 'github_commit', owner, repo, sha: second ?? '' };
  }
  if (first === 'releases' && second === 'tag' && rest.length === 3 && (third ?? '') !== '') {
    return { kind: 'github_release', owner, repo, tag: percentDecode(third ?? '') };
  }
  if (first === 'actions' && second === 'runs' && rest.length === 3) {
    const id = unsignedDigits(third ?? '', U64_MAX);
    if (id === null) return null;
    return { kind: 'github_run', owner, repo, id };
  }
  if (first === 'blob' && rest.length >= 3) {
    return fileRef(owner, repo, second ?? '', rest.slice(2), false, line);
  }
  if (first === 'tree' && rest.length >= 2) {
    return fileRef(owner, repo, second ?? '', rest.slice(2), true, null);
  }
  return null;
}

function fileRef(owner: string, repo: string, reference: string, path: string[], dir: boolean, line: number | null): Ref {
  return {
    kind: 'github_file',
    owner,
    repo,
    ref: percentDecode(reference),
    path: percentDecode(path.join('/')),
    line,
    dir,
  };
}

/** Image, video and HTML files by the extension of the last path segment. */
function media(url: string, segments: string[]): Ref | null {
  const name = segments[segments.length - 1];
  if (name === undefined) return null;
  const dot = name.lastIndexOf('.');
  if (dot < 0) return null;
  const ext = asciiLower(name.slice(dot + 1));
  if (IMAGE_EXTENSIONS.includes(ext)) return { kind: 'image', url };
  if (VIDEO_EXTENSIONS.includes(ext)) return { kind: 'video', url };
  if (HTML_EXTENSIONS.includes(ext)) return { kind: 'html', url };
  return null;
}

/** `L10` or `L10-L20` (the fragment of a line link) gives 10. */
function lineOfFragment(fragment: string): number | null {
  const m = /^L([0-9]+)/.exec(fragment);
  return m ? unsignedDigits(m[1] ?? '', U32_MAX) : null;
}

// ---- Inline code and links, shared helpers ----------------------------------

/** The code span's raw text is `open .. close`, both backticks. */
export function codeText(raw: string): string {
  return raw.slice(1, -1);
}

/** A markdown link's target, read back from its raw text (`[label](target)` or `![alt](target)`). A bare URL is its
 * own target. */
export function targetOf(raw: string): string {
  if (!raw.startsWith('[') && !raw.startsWith('!')) return raw;
  const sep = raw.indexOf('](');
  return sep >= 0 ? raw.slice(sep + 2, -1) : raw;
}
