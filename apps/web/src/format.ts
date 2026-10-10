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
  /* a fleet's agents run for hours, and a role fleet runs for days: past a
     day the hours roll into days rather than reading as "72 h 00 m", which
     is not a duration the register would ever write */
  if (h < 24) return h + ' h ' + pad(mm, 2) + ' m';
  const d = Math.floor(h / 24); const hh = h % 24;
  return d + ' d ' + pad(hh, 2) + ' h ' + pad(mm, 2) + ' m';
}

/** An amount of credits as money: the ledger's own unit and the dollar spend
    it stands for. Amounts are whole credits and 1 credit = $0.001 of real
    spend (docs/treasury-draft/schema.sql, statute 0109), so a figure is always
    shown both ways and no rate is invented. */
export function fmtMoney(credits: number | null | undefined): string {
  const c = Number(credits ?? 0);
  const dollars = (c / 1000).toLocaleString('en-GB', {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  });
  return `${fmtNum(c)} cr · $${dollars}`;
}

/** A moment as the register stamps it in a ruled row: the clock time when it
    falls on `today`'s day, else the date and time — so a fleet opened
    yesterday never reads as though it opened this morning. The year joins
    only when it differs. `today` is a seam for the harness. */
export function fmtStamp(ms: number | null | undefined, today: number = Date.now()): string {
  if (ms == null) return '—';
  const d = new Date(ms);
  const t = new Date(today);
  const sameDay =
    d.getFullYear() === t.getFullYear() && d.getMonth() === t.getMonth() && d.getDate() === t.getDate();
  if (sameDay) return fmtTime(ms);
  const date =
    d.getFullYear() === t.getFullYear()
      ? `${pad(d.getMonth() + 1, 2)}-${pad(d.getDate(), 2)}`
      : `${d.getFullYear()}-${pad(d.getMonth() + 1, 2)}-${pad(d.getDate(), 2)}`;
  return `${date} ${pad(d.getHours(), 2)}:${pad(d.getMinutes(), 2)}`;
}
