/* Serial text handling: the append-only per-VM line cache, line kinds for the
   transcript's narrow column, and the guest's command set. Guest bytes are
   untrusted — this module only ever produces strings for textContent. */

export interface SerialCache {
  raw: string;
  lines: string[];
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
    cache.set(key, { raw: text, lines });
    return lines;
  }
  /* append-only growth; the last line may still be on its way (no \n yet) */
  const lines = cached.lines;
  const parts = text.slice(cached.raw.length).replace(/\r/g, '').split('\n');
  let i = 0;
  if (cached.raw !== '' && !cached.raw.endsWith('\n') && lines.length) {
    lines[lines.length - 1] = (lines[lines.length - 1] ?? '') + (parts[0] ?? '');
    i = 1;
  }
  for (; i < parts.length; i++) lines.push(parts[i] ?? '');
  if (text.endsWith('\n')) lines.pop();
  cached.raw = text;
  return lines;
}

/** The transcript's kind column: boot lines, heartbeats, guest chatter, replies. */
export function kindOf(line: string): string {
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

/** The guest's command set: quick buttons beside the serial input. */
export const QUICK_COMMANDS: ReadonlyArray<readonly [string, string]> = [
  ['help', 'list the guest’s commands'],
  ['uname', 'kernel banner'],
  ['cpu', 'vCPU count and model'],
  ['mem', 'first lines of /proc/meminfo'],
  ['uptime', '/proc/uptime'],
  ['poweroff', 'the guest exits through ACPI S5 (slept); the VM leaves the running state'],
];
