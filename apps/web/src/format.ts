/* Number and time formatting — the register's voice (en-GB, tabular). */

export function pad(n: number | string, w: number): string {
  let s = String(n);
  while (s.length < w) s = '0' + s;
  return s;
}

export function fmtNum(n: number | null | undefined): string {
  return Number(n ?? 0).toLocaleString('en-GB');
}

export function fmtTime(ms: number | null | undefined): string {
  if (ms == null) return '—';
  const d = new Date(ms);
  return pad(d.getHours(), 2) + ':' + pad(d.getMinutes(), 2) + ':' + pad(d.getSeconds(), 2);
}

/** A self-contained timestamp for documents that leave the console:
    `2026-10-10 21:50:05`, the node's local clock like every other time here. */
export function fmtDateTime(ms: number | null | undefined): string {
  if (ms == null) return '—';
  const d = new Date(ms);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1, 2)}-${pad(d.getDate(), 2)} ${fmtTime(ms)}`;
}

export function fmtDur(ms: number): string {
  let s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return s + ' s';
  const m = Math.floor(s / 60); s %= 60;
  if (m < 60) return m + ' m ' + pad(s, 2) + ' s';
  const h = Math.floor(m / 60); const mm = m % 60;
  return h + ' h ' + pad(mm, 2) + ' m';
}
