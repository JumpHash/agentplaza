// The Workstations screen's view-model: the labels, tones, checks and the settings form's
// serialization the screen draws from. No DOM here, so the tests cover it directly.

import type {
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

/** The longest name a workstation can have (the hub's limit, in characters). */
export const MAX_NAME_CHARS = 64;
/** The join code's lifetimes the screen offers, in minutes; the hub caps a code at 60. */
export const TTL_OPTIONS = [15, 30, 60] as const;
/** The bounds the settings form checks before it sends (the hub stores any u8 and u32). */
export const RETENTION_DAYS = { min: 1, max: 3650 } as const;
export const RECORD_FPS = { min: 1, max: 30 } as const;
export const WORK_FOLDER_MAX = 512;

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
