/* Transcript export: a VM's serial capture — or a whole run's, sectioned by
   VM — as a plain-text download. The bytes are untrusted guest text; a file
   download is inert by construction (Blob -> object URL -> a download anchor,
   revoked on use). Nothing here renders markup. */

import { fmtDateTime, fmtDur, fmtNum } from './format';
import type { FleetSample } from './models/history';
import type { Run, Vm } from './models/run';
import { cidRange, exitKind, exitKindText, modeText } from './models/run';
import { sessionOf, sessionSummary } from './models/session';

/** One VM's transcript, exactly as captured: the lines joined, with a final
    newline. No headers, no reformatting — the file is the transcript. */
export function vmTranscriptText(lines: string[]): string {
  return lines.length === 0 ? '' : lines.join('\n') + '\n';
}

/** The labeled section rule every run-level export puts over a VM's transcript. */
function sectionRule(run: Run, vm: Vm, lineCount: number): string {
  const cid = vm.guest_cid != null ? ` · cid ${vm.guest_cid}` : '';
  return (
    `══ run ${run.id} · vm ${vm.id}${cid} · ${vm.state} · ` +
    `${fmtNum(lineCount)} ${lineCount === 1 ? 'line' : 'lines'} ══\n`
  );
}

/** The whole run: every VM's transcript under its own labeled section rule,
    in VM order. Section labels are the only added bytes. */
export function runTranscriptText(run: Run, linesForVm: (vm: Vm) => string[]): string {
  const parts: string[] = [];
  for (const vm of run.vms) {
    const lines = linesForVm(vm);
    parts.push(sectionRule(run, vm, lines.length));
    parts.push(vmTranscriptText(lines));
    parts.push('\n');
  }
  return parts.join('');
}

/** The run's report: the register's facts about the run (state, shape,
    times, per-VM CIDs and exit kinds), then every VM's transcript under its
    section rule — with the session's summary line when the transcript shows
    a completed vsock session. Assembled here, byte for byte; the transcripts
    inside stay exactly as captured. */
export function runReportText(run: Run, linesForVm: (vm: Vm) => string[]): string {
  const n = run.vms.length;
  const parts: string[] = [];
  parts.push(`══ run ${run.id} · report ══\n`);
  parts.push(`state: ${run.state}\n`);
  parts.push(`opened: ${fmtDateTime(run.created_at)}\n`);
  parts.push(`closed: ${run.ended_at != null ? fmtDateTime(run.ended_at) : '—'}\n`);
  parts.push(`elapsed: ${fmtDur((run.ended_at ?? Date.now()) - run.created_at)}\n`);
  parts.push(
    `shape: ${n} ${n === 1 ? 'VM' : 'VMs'} × ${run.vcpus} ${run.vcpus === 1 ? 'vCPU' : 'vCPUs'} · ` +
    `${fmtNum(run.memory_mib)} MiB each · ${modeText(run)}\n`,
  );
  const cids = cidRange(run);
  parts.push(`vsock: ${cids != null ? `guest cids ${cids}` : 'none'}\n`);
  parts.push('guests:\n');
  for (const vm of run.vms) {
    const cid = vm.guest_cid != null ? ` · cid ${vm.guest_cid}` : '';
    const exit = vm.state === 'exited' ? ` · ${exitKindText(exitKind(linesForVm(vm)))}` : '';
    parts.push(`  vm ${vm.id}${cid} · ${vm.state}${exit}\n`);
  }
  parts.push('\n');
  for (const vm of run.vms) {
    const lines = linesForVm(vm);
    parts.push(sectionRule(run, vm, lines.length));
    const session = sessionOf(lines);
    if (session?.phase === 'complete') parts.push(`session: ${sessionSummary(session)}\n`);
    parts.push(vmTranscriptText(lines));
    parts.push('\n');
  }
  return parts.join('');
}

/** The telemetry session as long-form CSV: one row per observation for the
    fleet total, then one per run seen in that sample. Long form, because runs
    arrive and leave mid-session — a wide sheet would lie with empty cells.
    Run ids and figures are the node's own identifiers; no guest text here. */
export function telemetryCsv(samples: readonly FleetSample[]): string {
  const first = samples[0];
  const rows: string[] = ['observed_at,elapsed_s,scope,vms_live,ram_mib,lines_per_s'];
  for (const s of samples) {
    const at = new Date(s.at).toISOString();
    const elapsed = (first ? (s.at - first.at) / 1000 : 0).toFixed(1);
    rows.push(`${at},${elapsed},fleet,${s.vms},${s.ramMib},${s.lps.toFixed(2)}`);
    for (const [id, r] of Object.entries(s.runs)) {
      rows.push(`${at},${elapsed},${id},${r.vms},${r.ramMib},${r.lps.toFixed(2)}`);
    }
  }
  return rows.join('\n') + '\n';
}

/** Download `text` as a plain-text file. The object URL is revoked as soon as
    the browser has taken the download (a zero-timeout after the click). */
export function downloadText(filename: string, text: string, mime = 'text/plain;charset=utf-8'): void {
  const blob = new Blob([text], { type: mime });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => { URL.revokeObjectURL(url); }, 0);
}
