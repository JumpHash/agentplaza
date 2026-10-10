/* Per-VM serial activity: lines-per-second from poll deltas.
   The map and the 3D view read this to pulse running VMs; the run page
   shows it as a number. Fed by the store on every detail merge. */

const EMA_ALPHA = 0.4;

interface Sample {
  lines: number;
  at: number;
  /** Smoothed lines/sec; decays toward 0 as polls see no growth. */
  lps: number;
}

export class ActivityTracker {
  private samples = new Map<string, Sample>();

  /** Record a line count for `key` ("r3/0"); returns the smoothed lines/sec. */
  observe(key: string, lines: number, at: number): number {
    const prev = this.samples.get(key);
    if (!prev) {
      this.samples.set(key, { lines, at, lps: 0 });
      return 0;
    }
    const dl = Math.max(0, lines - prev.lines);
    const dt = Math.max(1, (at - prev.at) / 1000);
    const inst = dl / dt;
    const lps = prev.lps + EMA_ALPHA * (inst - prev.lps);
    this.samples.set(key, { lines, at, lps });
    return lps;
  }

  /** The smoothed lines/sec for a VM; 0 when never observed. */
  get(key: string): number {
    return this.samples.get(key)?.lps ?? 0;
  }

  /** Forget a run's VMs (the run left the register). */
  dropRun(runId: string): void {
    const prefix = runId + '/';
    for (const key of this.samples.keys()) {
      if (key.startsWith(prefix)) this.samples.delete(key);
    }
  }
}

export function vmKey(runId: string, vmId: number): string {
  return `${runId}/${vmId}`;
}
