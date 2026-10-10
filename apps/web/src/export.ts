/* Transcript export: a VM's serial capture — or a whole run's, sectioned by
   VM — as a plain-text download. The bytes are untrusted guest text; a file
   download is inert by construction (Blob -> object URL -> a download anchor,
   revoked on use). Nothing here renders markup. */

import { fmtNum } from './format';
import type { Run, Vm } from './models/run';

/** One VM's transcript, exactly as captured: the lines joined, with a final
    newline. No headers, no reformatting — the file is the transcript. */
export function vmTranscriptText(lines: string[]): string {
  return lines.length === 0 ? '' : lines.join('\n') + '\n';
}

/** The whole run: every VM's transcript under its own labeled section rule,
    in VM order. Section labels are the only added bytes. */
export function runTranscriptText(run: Run, linesForVm: (vm: Vm) => string[]): string {
  const parts: string[] = [];
  for (const vm of run.vms) {
    const lines = linesForVm(vm);
    const cid = vm.guest_cid != null ? ` · cid ${vm.guest_cid}` : '';
    parts.push(
      `══ run ${run.id} · vm ${vm.id}${cid} · ${vm.state} · ` +
      `${fmtNum(lines.length)} ${lines.length === 1 ? 'line' : 'lines'} ══\n`,
    );
    parts.push(vmTranscriptText(lines));
    parts.push('\n');
  }
  return parts.join('');
}

/** Download `text` as a plain-text file. The object URL is revoked as soon as
    the browser has taken the download (a zero-timeout after the click). */
export function downloadText(filename: string, text: string): void {
  const blob = new Blob([text], { type: 'text/plain;charset=utf-8' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => { URL.revokeObjectURL(url); }, 0);
}
