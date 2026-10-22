// The Workstations screen's view-model: the labels, tones, checks and the settings form's
// serialization the screen draws from. No DOM here, so the tests cover it directly.

import type {
  InputAction,
  InputButton,
  RecordingDefaults,
  StationAccount,
  StationCodeState,
  StationFacts,
  StationForm,
  StationGpu,
  StationLease,
  StationOs,
  StationSettings,
  StationStatus,
  StationView,
  Takeover,
} from '../api';
import type { Tone } from '../ui/components';

/** A block of a page that loads its own data: its element, a reload, and the teardown when the page is left. */
export interface Section {
  el: HTMLElement;
  refresh(): Promise<void>;
  dispose(): void;
}

/** The longest name a workstation can have (the hub's limit, in characters). */
export const MAX_NAME_CHARS = 64;
/** The join code's lifetimes the screen offers, in minutes; the hub caps a code at 60. */
export const TTL_OPTIONS = [15, 30, 60] as const;
/** The bounds the settings form checks before it sends (the hub stores any u8 and u32). */
export const RETENTION_DAYS = { min: 1, max: 3650 } as const;
/** The frames a recording keeps a second, for a workstation and the hub-wide defaults: the hub's sampler keeps at most 5. */
export const RECORD_FPS = { min: 1, max: 5 } as const;
/** The hub-wide defaults' frames a second: the same bounds as a workstation's. */
export const DEFAULT_FPS = RECORD_FPS;
export const WORK_FOLDER_MAX = 512;
/** The largest file the work folder syncs (the daemon's and the hub's cap). */
export const UPLOAD_MAX_BYTES = 512 * 1024 * 1024;

const clamp = (n: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, n));
const isRec = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
const isPosInt = (v: unknown): v is number => typeof v === 'number' && Number.isSafeInteger(v) && v > 0;
const textOf = (v: unknown): string => (typeof v === 'string' ? v : '');
function parseObject(text: string): Record<string, unknown> | null {
  try {
    const v: unknown = JSON.parse(text);
    return isRec(v) ? v : null;
  } catch {
    return null;
  }
}

const STATUS: Record<StationStatus, { label: string; tone: Tone }> = {
  online: { label: 'online', tone: 'ok' },
  offline: { label: 'offline', tone: 'muted' },
  draining: { label: 'draining', tone: 'warn' },
  revoked: { label: 'revoked', tone: 'danger' },
};

/** The status chip: the state the hub reports for a workstation. */
export function statusChip(status: StationStatus): { label: string; tone: Tone } {
  return STATUS[status];
}

const OS_LABEL: Record<StationOs, string> = { macos: 'macOS', windows: 'Windows', linux: 'Linux' };
const FORM_LABEL: Record<StationForm, string> = { laptop: 'Laptop', desktop: 'Desktop', mini: 'Mini', server: 'Server', vm: 'VM' };

export function osText(facts: Pick<StationFacts, 'os' | 'os_version'>): string {
  return [OS_LABEL[facts.os], facts.os_version].filter(Boolean).join(' ');
}

export function formLabel(form: StationForm): string {
  return FORM_LABEL[form];
}

/** The account badge on every row: the agents act as the owner, so it is flagged. Null for a separate agent account. */
export function accountBadge(account: StationAccount): { text: string; tone: Tone } | null {
  return account.mode === 'owner' ? { text: `agents act as ${account.name}`, tone: 'warn' } : null;
}

/** The account in words, for the facts: whose OS account the agents use on this machine. */
export function accountText(account: StationAccount): string {
  return account.mode === 'owner' ? `the owner's own account (${account.name}): agents act as the owner` : `a separate agent account (${account.name})`;
}

export function ramText(ramMb: number): string {
  if (!ramMb) return '—';
  const gib = ramMb / 1024;
  return `${gib >= 10 ? Math.round(gib) : gib.toFixed(1)} GB RAM`;
}

function vramText(mb: number): string {
  return mb >= 1024 ? `${Math.round(mb / 1024)} GB` : `${mb} MB`;
}

/** One GPU in words: its name, then its memory (a GPU that shares system memory reports 0). */
export function gpuLine(gpu: StationGpu): string {
  return gpu.vram_mb > 0 ? `${gpu.name} · ${vramText(gpu.vram_mb)}` : `${gpu.name} · shared memory`;
}

/** The GPUs' names for a row, or "no GPU". */
export function gpuSummary(gpus: StationGpu[]): string {
  return gpus.length ? gpus.map((g) => g.name).join(', ') : 'no GPU';
}

/** The displays' sizes in points, or "none". */
export function displaysText(displays: StationFacts['displays']): string {
  return displays.length ? displays.map((d) => `${d.w}×${d.h}`).join(', ') : 'none';
}

/** A row's one-line summary: the OS, the kind of machine, the memory and the GPUs. */
export function stationSummary(facts: StationFacts): string {
  return [osText(facts), formLabel(facts.form), ramText(facts.ram_mb), gpuSummary(facts.gpus)].filter((part) => part && part !== '—').join(' · ');
}

/** What a row's picture box says when there is no picture: why there is none. */
export function pictureNote(s: { online: boolean; revoked: boolean }): string {
  if (s.revoked) return 'Revoked';
  if (!s.online) return 'Offline';
  return 'No screen open';
}

/** The leases that hold a machine (opening or active) and those waiting for one. */
export function leaseCount(leases: StationLease[]): { holding: number; waiting: number } {
  const waiting = leases.filter((l) => l.state === 'queued').length;
  return { holding: leases.length - waiting, waiting };
}

/** A lease's state as a chip: queued leases show their place in the queue. */
export function leaseStateChip(lease: StationLease): { label: string; tone: Tone } {
  if (lease.state === 'queued') return { label: lease.position ? `queued #${lease.position}` : 'queued', tone: 'warn' };
  if (lease.state === 'opening') return { label: 'opening', tone: 'info' };
  return { label: 'active', tone: 'ok' };
}

export function leaseKindLabel(kind: StationLease['kind']): string {
  return kind === 'pixel' ? 'Pixel session' : 'Shared';
}

/** The takeover policies, with the one-line explanation each shows under the select (plan section 7). */
export const TAKEOVER_OPTIONS: { value: Takeover; label: string; explain: string }[] = [
  {
    value: 'agent_waits',
    label: 'Agent waits',
    explain: 'When you take over a screen session, the agent waits and resumes when you give the session back.',
  },
  {
    value: 'release_lease',
    label: 'Release the lease',
    explain: "When you take over, the agent's lease ends and its task goes back to the queue.",
  },
  {
    value: 'keep_working_pointer_paused',
    label: 'Keep working, pause the pointer',
    explain: 'When you take over, the agent keeps working through shells, files and the UI tree; only its pointer pauses.',
  },
];

export function takeoverText(value: Takeover): string {
  return TAKEOVER_OPTIONS.find((o) => o.value === value)?.explain ?? '';
}

function isTakeover(value: unknown): value is Takeover {
  return TAKEOVER_OPTIONS.some((o) => o.value === value);
}

/** The banner while the owner has the screen: what the agents do meanwhile, under the policy in force. */
export function takeoverBanner(policy: Takeover): string {
  if (policy === 'release_lease') return "You have the screen. Agents' leases end when you take over, and their tasks go back to the queue.";
  if (policy === 'keep_working_pointer_paused') return 'You have the screen. Agents keep working through shells, files and the UI tree; only their pointer is paused.';
  return 'You have the screen. Agents wait until you give it back.';
}

// ---- Live view and takeover ----------------------------------------------------------------------------------------

export const SCREEN_PROTOCOL = 'apz.station.screen.v1';
export const CONTROL_PROTOCOL = 'apz.station.control.v1';
/** The fps a live view may ask for (the hub takes 1 to 15). */
export const LIVE_FPS = { min: 1, max: 15, default: 5 } as const;
/** The scales a live view offers (the hub takes 0.1 to 1). */
export const LIVE_SCALES = [0.25, 0.5, 0.75, 1] as const;
export const LIVE_SCALE_DEFAULT = 0.5;
/** How often a socket is pinged; the hub closes one that stays silent for 45 s. */
export const PING_MS = 15_000;
/** Mouse moves go out at most this often; the newest point wins. */
export const MOVE_MS = 40;
/** Two Escapes this close together give the session back. */
export const ESCAPE_TWICE_MS = 1_500;
const BACKOFF_MS = [1_000, 2_000, 5_000, 10_000] as const;

/** The wait before reconnect `attempt` (0 first); the last wait repeats. */
export function backoffMs(attempt: number): number {
  return BACKOFF_MS[clamp(attempt, 0, BACKOFF_MS.length - 1)] ?? 10_000;
}

export interface Loc {
  protocol: string;
  host: string;
}

function socketUrl(path: string, query: Record<string, string | number>, loc: Loc): string {
  const scheme = loc.protocol === 'https:' ? 'wss:' : 'ws:';
  const qs = Object.entries(query)
    .map(([k, v]) => `${k}=${encodeURIComponent(String(v))}`)
    .join('&');
  return `${scheme}//${loc.host}${path}?${qs}`;
}

/** The live view's socket: `session` is a session id or `shared`. */
export function screenSocketUrl(station: string, view: { session: string; fps: number; scale: number }, loc: Loc): string {
  return socketUrl(`/v1/stations/${encodeURIComponent(station)}/screen`, { session: view.session, fps: view.fps, scale: view.scale }, loc);
}

/** The takeover socket for a session the live view names by its number. */
export function controlSocketUrl(station: string, session: number, loc: Loc): string {
  return socketUrl(`/v1/stations/${encodeURIComponent(station)}/control`, { session }, loc);
}

export interface FrameHeader {
  w: number;
  h: number;
  format: 'png' | 'jpeg';
  ts: number;
  seq: number;
}
export interface ScreenFrame extends FrameHeader {
  image: Uint8Array<ArrayBuffer>;
}

/**
 * One binary frame of the live view: a 4-byte big-endian header length, the JSON header `{w, h, format, ts, seq}`, then
 * the image. Null for anything that is not a frame of that shape.
 */
export function parseFrame(buf: ArrayBuffer): ScreenFrame | null {
  if (buf.byteLength < 4) return null;
  const headLen = new DataView(buf).getUint32(0, false);
  if (headLen === 0 || 4 + headLen > buf.byteLength) return null;
  const head = parseObject(new TextDecoder().decode(new Uint8Array(buf, 4, headLen)));
  if (!head) return null;
  const format = head.format === 'png' ? 'png' : head.format === 'jpeg' ? 'jpeg' : null;
  if (!format || !isPosInt(head.w) || !isPosInt(head.h) || !isPosInt(head.seq) || typeof head.ts !== 'number') return null;
  return { w: head.w, h: head.h, format, ts: head.ts, seq: head.seq, image: new Uint8Array(buf, 4 + headLen) };
}

/** The live view's text messages. */
export type ScreenMessage = { t: 'started'; session: number } | { t: 'ended'; reason: string } | { t: 'error'; error: string } | { t: 'pong' };

export function parseScreenText(text: string): ScreenMessage | null {
  const v = parseObject(text);
  if (!v) return null;
  if (v.t === 'started') return typeof v.session === 'number' ? { t: 'started', session: v.session } : null;
  if (v.t === 'ended') return { t: 'ended', reason: textOf(v.reason) };
  if (v.t === 'error') return { t: 'error', error: textOf(v.error) };
  return v.t === 'pong' ? { t: 'pong' } : null;
}

/** The takeover socket's messages. */
export type ControlMessage =
  | { t: 'taken'; session: number; policy: Takeover }
  | { t: 'given_back' }
  | { t: 'error'; error: string }
  | { t: 'pong' }
  | { t: 'ended'; reason: string };

export function parseControlText(text: string): ControlMessage | null {
  const v = parseObject(text);
  if (!v) return null;
  switch (v.t) {
    case 'taken':
      return typeof v.session === 'number' && isTakeover(v.policy) ? { t: 'taken', session: v.session, policy: v.policy } : null;
    case 'given_back':
      return { t: 'given_back' };
    case 'error':
      return { t: 'error', error: textOf(v.error) };
    case 'pong':
      return { t: 'pong' };
    case 'ended':
      return { t: 'ended', reason: textOf(v.reason) };
    default:
      return null;
  }
}

/** The client's frames on the takeover socket and the live view. */
export const takeFrame = (): string => '{"t":"take"}';
export const giveBackFrame = (): string => '{"t":"give_back"}';
export const pingFrame = (): string => '{"t":"ping"}';
export const actionFrame = (action: InputAction): string => JSON.stringify({ t: 'input', action });

export interface Point {
  x: number;
  y: number;
}
export interface Rect {
  left: number;
  top: number;
  width: number;
  height: number;
}

/**
 * A pointer position (client pixels) as a point of the shown frame, which is `frame` pixels wide and high and drawn
 * into `rect`. Clamped to the frame. Null when the picture has no size yet.
 */
export function framePoint(clientX: number, clientY: number, rect: Rect, frame: { w: number; h: number }): Point | null {
  if (rect.width <= 0 || rect.height <= 0) return null;
  const x = Math.round(((clientX - rect.left) * frame.w) / rect.width);
  const y = Math.round(((clientY - rect.top) * frame.h) / rect.height);
  return { x: clamp(x, 0, frame.w - 1), y: clamp(y, 0, frame.h - 1) };
}

/** A DOM mouse button (0 left, 1 middle, 2 right) as the daemon's; null for the others. */
export function inputButton(button: number): InputButton | null {
  if (button === 0) return 'left';
  if (button === 1) return 'middle';
  if (button === 2) return 'right';
  return null;
}

export const moveAction = (p: Point): InputAction => ({ action: 'move', x: p.x, y: p.y });
/** A press or a release of a button, at the pointer's place as the last move put it. */
export const buttonAction = (button: InputButton, down: boolean): InputAction => (down ? { action: 'down', button } : { action: 'up', button });

const NOTCH_PX = 50;
const MAX_NOTCHES = 20;

/**
 * A wheel delta as notches. `mode` is the DOM's deltaMode: 0 pixels, 1 lines, 2 pages. The sign is kept: a positive
 * delta (the wheel turned toward the user) is a positive count, which scrolls the content down.
 */
export function scrollNotches(delta: number, mode: number): number {
  if (!Number.isFinite(delta) || delta === 0) return 0;
  const size = Math.abs(delta);
  const units = mode === 1 ? size : mode === 2 ? size * 10 : size / NOTCH_PX;
  const count = clamp(Math.round(units), 1, MAX_NOTCHES);
  return Math.sign(delta) * count;
}

/** A wheel turn at `p`: `dy > 0` scrolls the content down, `dx > 0` scrolls it right. */
export function wheelAction(p: Point, deltaX: number, deltaY: number, mode: number): InputAction {
  return { action: 'scroll', x: p.x, y: p.y, dx: scrollNotches(deltaX, mode), dy: scrollNotches(deltaY, mode) };
}

/** The keyboard event's fields the mapping reads. */
export interface KeyEventLike {
  key: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}

/** DOM key names the daemon's chord names differ from. Single characters are their own names, lower case. */
const KEY_NAMES: Record<string, string> = {
  Enter: 'enter',
  Tab: 'tab',
  ' ': 'space',
  Escape: 'escape',
  Backspace: 'backspace',
  Delete: 'delete',
  Insert: 'insert',
  Home: 'home',
  End: 'end',
  PageUp: 'pageup',
  PageDown: 'pagedown',
  ArrowUp: 'up',
  ArrowDown: 'down',
  ArrowLeft: 'left',
  ArrowRight: 'right',
  '-': 'minus',
  '=': 'equal',
  '[': 'bracketleft',
  ']': 'bracketright',
  ';': 'semicolon',
  "'": 'quote',
  '\\': 'backslash',
  ',': 'comma',
  '.': 'period',
  '/': 'slash',
  '`': 'grave',
  '+': 'plus',
};

function chordName(key: string): string | null {
  const named = KEY_NAMES[key];
  if (named) return named;
  if (/^F([1-9]|1\d|2[0-4])$/.test(key)) return key.toLowerCase();
  if (/^[a-zA-Z0-9]$/.test(key)) return key.toLowerCase();
  return null;
}

/**
 * A key press as an owner action. A printable character with no ctrl, alt or meta is typed as text; anything else is
 * a chord (`ctrl+shift+s`, `shift+tab`, `enter`). Null for a key the daemon has no name for.
 */
export function keyAction(ev: KeyEventLike): InputAction | null {
  const printable = [...ev.key].length === 1;
  if (printable && !ev.ctrlKey && !ev.altKey && !ev.metaKey) return { action: 'type', text: ev.key };
  const name = chordName(ev.key);
  if (!name) return null;
  const mods: string[] = [];
  if (ev.ctrlKey) mods.push('ctrl');
  if (ev.altKey) mods.push('alt');
  if (ev.shiftKey) mods.push('shift');
  if (ev.metaKey) mods.push('meta');
  return { action: 'key', combo: [...mods, name].join('+') };
}

export interface SessionChoice {
  value: string;
  label: string;
}

/** The sessions a live view can watch: the station's shared one, and each active pixel lease's session. */
export function sessionChoices(leases: StationLease[], nameOf: (cid: number) => string): SessionChoice[] {
  const pixel = leases.filter((l): l is StationLease & { session: number } => l.kind === 'pixel' && l.state !== 'queued' && l.session !== null);
  return [
    { value: 'shared', label: "Shared: the owner's screen" },
    ...pixel.map((l) => ({ value: String(l.session), label: `Session ${l.session}: ${nameOf(l.agent)}${l.task ? ` · ${l.task}` : ''}` })),
  ];
}

// ---- Recordings ----------------------------------------------------------------------------------------------------

/**
 * The span of a recording in ms: from its start to its end, or while it is open, to its last action (its start when it
 * has none yet).
 */
export function recordingSpan(rec: { started_ms: number; ended_ms: number | null }, lastAtMs: number | null): number {
  const end = rec.ended_ms ?? Math.max(rec.started_ms, lastAtMs ?? rec.started_ms);
  return Math.max(0, end - rec.started_ms);
}

/** A frame's capture time as the hub keeps it: its seq (1-based) and when it was captured (epoch ms). */
export interface FrameStamp {
  seq: number;
  ts_ms: number;
}

/**
 * When each frame of a recording was kept, in ms into the recording: index `seq - 1` of the result, for `frames` frames.
 * The hub keeps each frame's time (`stamps`, in any order), so those are the real times. A frame without one (the
 * stamps stop at the hub's cap, or a recording kept before frames had times) is placed between the known ones, or past
 * the last at their average spacing, and never after the end. With no stamps the frames are spread evenly from the
 * start to the end. Times never go back.
 */
export function frameTimes(frames: number, span: number, startMs: number, stamps: FrameStamp[]): number[] {
  if (frames <= 0) return [];
  const end = Math.max(0, span);
  const known = stamps
    .filter((s) => Number.isInteger(s.seq) && s.seq >= 1 && s.seq <= frames)
    .sort((a, b) => a.seq - b.seq)
    .map((s) => ({ seq: s.seq, at: Math.max(0, s.ts_ms - startMs) }))
    .filter((s, i, all) => i === 0 || s.seq > (all[i - 1]?.seq ?? 0));
  if (known.length === 0) {
    return Array.from({ length: frames }, (_, i) => (frames <= 1 || end <= 0 ? 0 : (i / (frames - 1)) * end));
  }
  const first = known[0];
  const last = known[known.length - 1];
  if (!first || !last) return [];
  // The spacing of the known frames, for the ones past their ends (or the even spacing when only one is known).
  const step = known.length >= 2 ? (last.at - first.at) / (last.seq - first.seq) : end / frames;
  const cap = Math.max(end, last.at);
  const out: number[] = [];
  // `k` is the first known frame at or after `seq`: one pass over the frames.
  let k = 0;
  for (let seq = 1; seq <= frames; seq++) {
    while (k < known.length && (known[k]?.seq ?? Infinity) < seq) k++;
    const next = known[k];
    const prev = k > 0 ? known[k - 1] : undefined;
    let t: number;
    if (next && next.seq === seq) t = next.at;
    else if (prev && next) t = prev.at + ((seq - prev.seq) * (next.at - prev.at)) / (next.seq - prev.seq);
    else if (prev) t = prev.at + (seq - prev.seq) * step;
    else t = first.at - (first.seq - seq) * step;
    out.push(Math.max(clamp(t, 0, cap), out[seq - 2] ?? 0));
  }
  return out;
}

/** The frame on screen (1-based) at `rel` ms: the last one kept by then, the first before it. 0 with no frames. */
export function frameAt(rel: number, times: number[]): number {
  if (times.length === 0) return 0;
  if (rel < (times[0] ?? 0)) return 1;
  let lo = 0;
  let hi = times.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if ((times[mid] ?? 0) <= rel) lo = mid;
    else hi = mid - 1;
  }
  return lo + 1;
}

/** When frame `seq` (1-based) was kept, in ms into the recording: the inverse of `frameAt`. 0 with no frames. */
export function frameTime(seq: number, times: number[]): number {
  if (times.length === 0) return 0;
  return times[clamp(seq, 1, times.length) - 1] ?? 0;
}

/** The index of the newest action at or before `rel` ms (`events` in time order), or -1 before the first. */
export function activeAction(events: { at_ms: number }[], startMs: number, rel: number): number {
  let index = -1;
  for (let i = 0; i < events.length; i++) {
    const at = events[i]?.at_ms;
    if (at === undefined || at - startMs > rel) break;
    index = i;
  }
  return index;
}

/** m:ss, or h:mm:ss from an hour. Never negative. */
export function clockText(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const ss = String(s).padStart(2, '0');
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`;
}

/** An action in words: its kind, then the fields it was asked with. Cut to 160 characters. */
export function actionText(event: { kind: string; summary: unknown }): string {
  const fields = isRec(event.summary)
    ? Object.entries(event.summary)
        .filter(([k, v]) => k !== 'kind' && v !== null && v !== undefined)
        .map(([k, v]) => `${k} ${typeof v === 'string' ? v : JSON.stringify(v)}`)
    : [];
  const text = `${event.kind.replace(/_/g, ' ')}${fields.length ? `: ${fields.join(', ')}` : ''}`;
  return text.length > 160 ? `${text.slice(0, 159)}…` : text;
}

/** The playback speeds, as multipliers. */
export const PLAYBACK_SPEEDS = [0.5, 1, 2, 4, 8] as const;

// ---- The work folder -----------------------------------------------------------------------------------------------

/** The folders and files directly inside `dir` ('' is the root), out of a list of paths below it. */
export function treeOf<T extends { path: string }>(files: T[], dir: string): { folders: { name: string; path: string }[]; files: T[] } {
  const prefix = dir ? `${dir}/` : '';
  const folders = new Set<string>();
  const direct: T[] = [];
  for (const file of files) {
    if (!file.path.startsWith(prefix)) continue;
    const rest = file.path.slice(prefix.length);
    if (!rest) continue;
    const slash = rest.indexOf('/');
    if (slash === -1) direct.push(file);
    else folders.add(rest.slice(0, slash));
  }
  return {
    folders: [...folders].sort((a, b) => a.localeCompare(b)).map((name) => ({ name, path: `${prefix}${name}` })),
    files: direct.sort((a, b) => a.path.localeCompare(b.path)),
  };
}

/** The folder above `dir`, or '' at the root. */
export function parentDir(dir: string): string {
  const slash = dir.lastIndexOf('/');
  return slash === -1 ? '' : dir.slice(0, slash);
}

/** The path of `name` inside `dir`. */
export function joinPath(dir: string, name: string): string {
  return dir ? `${dir}/${name}` : name;
}

/** The trail to `dir` for the page's links: the root, then each folder. */
export function crumbsOf(dir: string): { name: string; path: string }[] {
  const crumbs = [{ name: 'Work folder', path: '' }];
  let at = '';
  for (const part of dir ? dir.split('/') : []) {
    at = joinPath(at, part);
    crumbs.push({ name: part, path: at });
  }
  return crumbs;
}

/** The paths a paused sync holds for deletion: each entry a string, or an object with its `path`. */
export function pendingPaths(raw: unknown): string[] {
  if (!Array.isArray(raw)) return [];
  return raw.flatMap((item): string[] => (typeof item === 'string' ? [item] : isRec(item) && typeof item.path === 'string' ? [item.path] : []));
}

/** Why a file cannot be uploaded into the work folder under `name`, or null. */
export function uploadProblem(name: string, size: number): string | null {
  if (!name || name === '.' || name === '..' || /[/\\\u0000-\u001f\u007f]/.test(name)) return 'That file name cannot be kept in the work folder.';
  if (size > UPLOAD_MAX_BYTES) return `The work folder takes files up to ${bytesText(UPLOAD_MAX_BYTES)}.`;
  return null;
}

/** A byte count in words, in binary units. */
export function bytesText(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let value = n / 1024;
  let i = 0;
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024;
    i++;
  }
  return `${value >= 10 ? Math.round(value) : value.toFixed(1)} ${units[i] ?? 'TB'}`;
}

// ---- Settings ------------------------------------------------------------------------------------------------------

/** The settings form as the controls hold them: text for the numbers, so a half-typed value is still a value. */
export interface SettingsForm {
  takeover: Takeover;
  record: boolean;
  retention_days: string;
  record_fps: string;
  record_takeovers: boolean;
  work_folder: string;
  snapshot_reset: boolean;
}

export function settingsForm(s: StationSettings): SettingsForm {
  return {
    takeover: s.takeover,
    record: s.record,
    retention_days: String(s.retention_days),
    record_fps: String(s.record_fps),
    record_takeovers: s.record_takeovers,
    work_folder: s.work_folder,
    snapshot_reset: s.snapshot_reset,
  };
}

export type SettingsRead = { ok: true; settings: StationSettings } | { ok: false; error: string };

/** A whole number from `text` within [min, max], or null. */
function wholeIn(text: string, min: number, max: number): number | null {
  const t = text.trim();
  if (!/^\d+$/.test(t)) return null;
  const n = Number(t);
  return n >= min && n <= max ? n : null;
}

/** The form's values as settings to send, or the first problem to show. The work folder is trimmed. */
export function readSettings(f: SettingsForm): SettingsRead {
  if (!TAKEOVER_OPTIONS.some((o) => o.value === f.takeover)) return { ok: false, error: 'Pick a takeover policy.' };
  const days = wholeIn(f.retention_days, RETENTION_DAYS.min, RETENTION_DAYS.max);
  if (days === null) return { ok: false, error: `Keep recordings for whole days, from ${RETENTION_DAYS.min} to ${RETENTION_DAYS.max}.` };
  const fps = wholeIn(f.record_fps, RECORD_FPS.min, RECORD_FPS.max);
  if (fps === null) return { ok: false, error: `Record whole frames per second, from ${RECORD_FPS.min} to ${RECORD_FPS.max}.` };
  const folder = f.work_folder.trim();
  if (!folder) return { ok: false, error: 'The work folder cannot be empty.' };
  if (folder.length > WORK_FOLDER_MAX || hasControl(folder)) return { ok: false, error: `The work folder is a path of up to ${WORK_FOLDER_MAX} characters.` };
  return {
    ok: true,
    settings: {
      takeover: f.takeover,
      record: f.record,
      retention_days: days,
      record_fps: fps,
      record_takeovers: f.record_takeovers,
      work_folder: folder,
      snapshot_reset: f.snapshot_reset,
    },
  };
}

/** The hub-wide recording defaults as the form holds them. */
export interface DefaultsForm {
  record: boolean;
  record_takeovers: boolean;
  retention_days: string;
  record_fps: string;
}

export function defaultsForm(d: RecordingDefaults): DefaultsForm {
  return {
    record: d.record,
    record_takeovers: d.record_takeovers,
    retention_days: String(d.retention_days),
    record_fps: String(d.record_fps),
  };
}

export type DefaultsRead = { ok: true; defaults: RecordingDefaults } | { ok: false; error: string };

export function readDefaults(f: DefaultsForm): DefaultsRead {
  const days = wholeIn(f.retention_days, RETENTION_DAYS.min, RETENTION_DAYS.max);
  if (days === null) return { ok: false, error: `Keep recordings for whole days, from ${RETENTION_DAYS.min} to ${RETENTION_DAYS.max}.` };
  const fps = wholeIn(f.record_fps, DEFAULT_FPS.min, DEFAULT_FPS.max);
  if (fps === null) return { ok: false, error: `Record whole frames per second, from ${DEFAULT_FPS.min} to ${DEFAULT_FPS.max}.` };
  return { ok: true, defaults: { record: f.record, retention_days: days, record_fps: fps, record_takeovers: f.record_takeovers } };
}

function hasControl(text: string): boolean {
  return [...text].some((c) => c.charCodeAt(0) < 32 || c.charCodeAt(0) === 127);
}

/** Why a name cannot be saved, or null. The same rule the hub applies. */
export function nameProblem(name: string): string | null {
  const t = name.trim();
  if (!t) return 'Give the workstation a name.';
  if ([...t].length > MAX_NAME_CHARS) return `The name is up to ${MAX_NAME_CHARS} characters.`;
  if (hasControl(t)) return 'The name has no control characters.';
  return null;
}

/** The command a person runs on the machine to enroll it. */
export function joinCommand(token: string): string {
  return `agentplaza workstation join ${token}`;
}

const CODE_STATE: Record<StationCodeState, { label: string; tone: Tone }> = {
  active: { label: 'Active', tone: 'ok' },
  used: { label: 'Used', tone: 'info' },
  expired: { label: 'Expired', tone: 'muted' },
  revoked: { label: 'Revoked', tone: 'danger' },
};

export function codeChip(state: StationCodeState): { label: string; tone: Tone } {
  return CODE_STATE[state];
}

/** mm:ss left until a deadline; 00:00 once it has passed. */
export function countdown(leftMs: number): string {
  const s = Math.max(0, Math.floor(leftMs / 1000));
  return `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`;
}

const AUDIT_LABEL: Record<string, string> = {
  join: 'Enrolled',
  join_refused: 'Join refused',
  code_revoke: 'Join code revoked',
  rename: 'Renamed',
  settings: 'Settings changed',
  drain: 'Draining',
  undrain: 'Back in service',
  revoke: 'Revoked',
  lease_granted: 'Lease granted',
  lease_failed: 'Lease failed',
  lease_ended: 'Lease ended',
  lease_released: 'Lease released',
};

/** An audit row's event in words; an unknown kind shows as its words. */
export function auditLabel(kind: string): string {
  return AUDIT_LABEL[kind] ?? kind.replace(/_/g, ' ');
}

/** Workstations in name order, the list's order. */
export function sortStations(list: StationView[]): StationView[] {
  return [...list].sort((a, b) => a.name.localeCompare(b.name));
}
