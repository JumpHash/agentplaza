/* Session telemetry: a rolling buffer of what this console observed — one
   fleet sample per successful poll, plus a short lines/s series per VM for
   the run entries' sparklines. Everything in it derives from data the client
   already polls (the runs register and the serial-activity tracker); nothing
   is fetched for it. The buffer is capped, so a long session never grows
   without bound, and a run's per-VM series die with it — the activity
   cache's lifecycle. Samples are kept once written: a run that leaves the
   register still contributed to the fleet's past, and the past stays ink. */

/** Samples kept, fleet and per-VM alike — about two minutes at the poll cadence. */
export const HISTORY_CAP = 120;

/** One run's contribution to the fleet at one poll. */
export interface RunSlice {
  /** Its VMs in the running state. */
  vms: number;
  /** Committed guest RAM: running VMs × the run's memory_mib. */
  ramMib: number;
  /** Serial chatter: Σ its VMs' lines/s EMA. */
  lps: number;
}

/** The fleet at one poll. */
export interface FleetSample {
  /** Wall-clock ms of the observation. */
  at: number;
  /** This sample's entry number in the session (survives the cap). */
  no: number;
  vms: number;
  ramMib: number;
  lps: number;
  /** Per-run decomposition, by run id. */
  runs: Record<string, RunSlice>;
}

export class SessionHistory {
  readonly fleet: FleetSample[] = [];
  private readonly vmLps = new Map<string, number[]>(); // "r3/0" -> recent lines/s
  private total = 0;

  /** Write one fleet observation. */
  push(sample: Omit<FleetSample, 'no'>): void {
    this.total++;
    this.fleet.push({ ...sample, no: this.total });
    if (this.fleet.length > HISTORY_CAP) this.fleet.splice(0, this.fleet.length - HISTORY_CAP);
  }

  /** Append one lines/s observation for a VM, one per detail poll. */
  pushVm(key: string, lps: number): void {
    let s = this.vmLps.get(key);
    if (!s) {
      s = [];
      this.vmLps.set(key, s);
    }
    s.push(lps);
    if (s.length > HISTORY_CAP) s.splice(0, s.length - HISTORY_CAP);
  }

  /** A VM's recent lines/s, oldest first; empty when never observed. */
  vmSeries(key: string): readonly number[] {
    return this.vmLps.get(key) ?? [];
  }

  /** The newest fleet sample, if any. */
  get latest(): FleetSample | null {
    return this.fleet[this.fleet.length - 1] ?? null;
  }

  /** Observations written this session (≥ fleet.length; the cap trims, this doesn't). */
  get pushed(): number {
    return this.total;
  }

  /** Forget a run's per-VM series (the run left the register). Fleet samples
      are history and stay — they age out of the window on their own. */
  dropRun(runId: string): void {
    const prefix = runId + '/';
    for (const key of [...this.vmLps.keys()]) {
      if (key.startsWith(prefix)) this.vmLps.delete(key);
    }
  }
}
