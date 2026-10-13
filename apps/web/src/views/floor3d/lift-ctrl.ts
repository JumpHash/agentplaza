// The lift controller: one cab between two stops (0 = the board's PCB top, 1 = the plate top), its doors and its call
// queue. Pure logic: no three.js, no nav grid. The design is in docs/design/navigation.md ("Lifts").
//
import type { LiftPhase, LiftState, LiftTicket } from './nav-types';

// The robot side (nav-agents) calls, for a robot id:
//   request(id, fromY, toY)  it wants a ride from the stop at fromY to the stop at toY (once, when its path reaches the lift)
//   ticket(id)               'none' | 'queued' | 'boarding' | 'riding' | 'arrived'
//   board(id)                it stepped into the cab (only while ticket is 'boarding')
//   alight(id)               it stepped out at its stop (only while ticket is 'arrived')
//   cancel(id)               it has LEFT: its path no longer uses this lift, it was respawned, removed, or it gave up
//   queueIndex(id)           its place in the line at its stop (0 = nearest the cab; -1 when it has no place)
// A live request stays live until the robot alights or cancels. Nothing here times a request out: a robot that waits,
// yields or is held keeps its place, and the cab serves every live request. Each frame reads state(): cabY is the
// platform's surface height, riders' feet follow it exactly. The cab never moves with its doors open, and the doors never
// open between stops.

export interface LiftConfig {
  /** The stop heights: the board's PCB top (low) and the plate top (high). */
  low: number;
  high: number;
  /** Riders the cab takes at once. Default 3. */
  capacity?: number;
  /** Average travel speed in units per second (the motion is smoothstep, so the peak is 1.5 times this). Default 1.2. */
  speed?: number;
  /** Seconds for the doors to open, and again to close. Default 0.5. */
  doorTime?: number;
  /** The least time the doors stay open for boarding. Default 1.0. */
  dwell?: number;
  /** The most extra seconds the doors wait while an admitted robot is still walking in. Default 2.5. */
  holdMax?: number;
  /** Seconds riders need to leave the cab (the least), and the most before the rest are dropped. Defaults 0.6 and 2.5. */
  unloadTime?: number;
  unloadMax?: number;
  /** The cab opens at a stop this many times in a row while the head of the line waits at the other one. Default 2. */
  skipMax?: number;
}

interface Req {
  id: number;
  from: 0 | 1;
  to: 0 | 1;
  /** The arrival order: the queue is served by ascending seq. A robot that missed its door gets a new one (the back). */
  seq: number;
  state: 'queued' | 'boarding' | 'riding' | 'arrived';
}

/** The sub-step: the controller advances in steps of at most this (seconds), so a long frame cannot skip a phase. */
const STEP = 1 / 60;
/** A frame's dt is clamped to this (seconds), as the nav world does. */
const MAX_DT = 0.25;

/** Smoothstep: the eased position of the travel at u in [0, 1] (zero speed at both ends). */
export function smoothstep(u: number): number {
  const k = Math.min(1, Math.max(0, u));
  return k * k * (3 - 2 * k);
}

export class LiftController {
  readonly low: number;
  readonly high: number;
  private readonly cap: number;
  private readonly speed: number;
  private readonly doorTime: number;
  private readonly dwell: number;
  private readonly holdMax: number;
  private readonly unloadTime: number;
  private readonly unloadMax: number;
  private readonly skipMax: number;

  private readonly reqs = new Map<number, Req>();
  private seq = 0;
  private stop: 0 | 1 = 0;
  private phase: LiftPhase = 'idle';
  /** Seconds spent in the current phase (travel: since its start). */
  private t = 0;
  /** Doors: 0 shut, 1 open. */
  private door = 0;
  private cabY: number;
  private travelFrom: 0 | 1 = 0;
  private travelTo: 0 | 1 = 0;
  private travelDur = 1;
  /** Openings here in a row while the head of the line waited at the other stop (see decide). */
  private skipped = 0;

  constructor(cfg: LiftConfig) {
    if (!(cfg.high > cfg.low)) throw new Error('lift: high must be above low');
    this.low = cfg.low;
    this.high = cfg.high;
    this.cap = Math.max(1, Math.floor(cfg.capacity ?? 3));
    this.speed = cfg.speed ?? 1.2;
    this.doorTime = cfg.doorTime ?? 0.5;
    this.dwell = cfg.dwell ?? 1.0;
    this.holdMax = cfg.holdMax ?? 2.5;
    this.unloadTime = cfg.unloadTime ?? 0.6;
    this.unloadMax = cfg.unloadMax ?? 2.5;
    this.skipMax = Math.max(1, Math.floor(cfg.skipMax ?? 2));
    this.cabY = this.low;
  }

  /** The stop height of stop s. */
  private yOf(s: 0 | 1): number {
    return s === 0 ? this.low : this.high;
  }

  /** The stop nearest to height y. */
  stopOf(y: number): 0 | 1 {
    return Math.abs(y - this.low) <= Math.abs(y - this.high) ? 0 : 1;
  }

  /**
   * Robot `id` wants to ride from the stop at fromY to the stop at toY. Returns false (and changes nothing) when the robot
   * already has a request, or both heights are the same stop.
   */
  request(id: number, fromY: number, toY: number): boolean {
    if (this.reqs.has(id)) return false;
    const from = this.stopOf(fromY);
    const to = this.stopOf(toY);
    if (from === to) return false;
    this.reqs.set(id, { id, from, to, seq: ++this.seq, state: 'queued' });
    return true;
  }

  ticket(id: number): LiftTicket {
    return this.reqs.get(id)?.state ?? 'none';
  }

  /** Robot `id` steps into the cab. True when it may (the ticket was 'boarding'): it is then 'riding'. */
  board(id: number): boolean {
    const r = this.reqs.get(id);
    if (!r || r.state !== 'boarding' || this.phase !== 'boarding') return false;
    r.state = 'riding';
    return true;
  }

  /** Robot `id` steps out at its stop. True when its ticket was 'arrived'; the request ends. */
  alight(id: number): boolean {
    const r = this.reqs.get(id);
    if (!r || r.state !== 'arrived') return false;
    this.reqs.delete(id);
    return true;
  }

  /** Robot `id` has left: its request (waiting, admitted or riding) is dropped. Unknown ids are a no-op. */
  cancel(id: number): void {
    this.reqs.delete(id);
  }

  /** The robot's place in the line at its stop: 0 is the front (the next to board). -1 when it is not waiting. */
  queueIndex(id: number): number {
    const r = this.reqs.get(id);
    if (!r || (r.state !== 'queued' && r.state !== 'boarding')) return -1;
    return this.line(r.from).indexOf(r);
  }

  /** The robots aboard (riding or arrived), in boarding order. */
  riders(): number[] {
    return this.aboard().map((r) => r.id);
  }

  state(): LiftState {
    return {
      cabY: this.cabY,
      doorsOpen: this.door > 0,
      boarding: this.aboard().map((r) => r.id),
      doorT: this.door,
      phase: this.phase,
      stop: this.travelling() ? this.travelTo : this.stop,
      calls: [this.waiting(0), this.waiting(1)],
    };
  }

  /** True when no robot has a request and the cab is at rest with its doors shut. */
  isIdle(): boolean {
    return this.reqs.size === 0 && this.phase === 'idle' && this.door === 0;
  }

  /** Advances the cab by dt seconds (clamped to [0, MAX_DT]; NaN and negative give 0). */
  update(dt: number): void {
    const h = Number.isFinite(dt) ? Math.min(MAX_DT, Math.max(0, dt)) : 0;
    const n = Math.ceil(h / STEP);
    for (let i = 0; i < n; i++) this.tick(h / n);
  }

  private travelling(): boolean {
    return this.phase === 'travel';
  }

  private waiting(s: 0 | 1): number {
    let n = 0;
    for (const r of this.reqs.values()) if (r.from === s && (r.state === 'queued' || r.state === 'boarding')) n++;
    return n;
  }

  /** The line at a stop, front first (by arrival order): every live request there, waiting or admitted. */
  private line(s: 0 | 1): Req[] {
    return Array.from(this.reqs.values())
      .filter((r) => r.from === s && (r.state === 'queued' || r.state === 'boarding'))
      .sort((a, b) => a.seq - b.seq);
  }

  /** The waiting robots over all stops, in arrival order. */
  private queued(): Req[] {
    return Array.from(this.reqs.values())
      .filter((r) => r.state === 'queued')
      .sort((a, b) => a.seq - b.seq);
  }

  private aboard(): Req[] {
    return Array.from(this.reqs.values())
      .filter((r) => r.state === 'riding' || r.state === 'arrived')
      .sort((a, b) => a.seq - b.seq);
  }

  private tick(h: number): void {
    switch (this.phase) {
      case 'idle':
        this.decide();
        return;
      case 'opening':
        this.door = Math.min(1, this.door + h / this.doorTime);
        if (this.door >= 1) this.afterOpen();
        return;
      case 'unloading': {
        this.t += h;
        const leaving = this.aboard().filter((r) => r.to === this.stop);
        if (this.t >= this.unloadMax) {
          for (const r of leaving) this.reqs.delete(r.id); // a robot that never stepped out is dropped (the view steps it off)
          this.beginBoarding();
        } else if (leaving.length === 0 && this.t >= this.unloadTime) this.beginBoarding();
        return;
      }
      case 'boarding': {
        this.t += h;
        const pending = Array.from(this.reqs.values()).filter((r) => r.state === 'boarding');
        if ((this.t >= this.dwell && pending.length === 0) || this.t >= this.dwell + this.holdMax) {
          for (const r of pending) {
            r.state = 'queued'; // missed the door: the back of the line
            r.seq = ++this.seq;
          }
          this.phase = 'closing';
          this.t = 0;
        }
        return;
      }
      case 'closing':
        this.door = Math.max(0, this.door - h / this.doorTime);
        if (this.door <= 0) {
          this.door = 0;
          this.phase = 'idle';
          this.t = 0;
        }
        return;
      case 'travel': {
        this.t += h;
        const u = Math.min(1, this.t / this.travelDur);
        const y0 = this.yOf(this.travelFrom);
        const y1 = this.yOf(this.travelTo);
        this.cabY = u >= 1 ? y1 : y0 + (y1 - y0) * smoothstep(u);
        if (u >= 1) this.arrive();
        return;
      }
    }
  }

  /**
   * Idle with the doors shut: riders aboard go to their stop first (those bound here unload). Then the cab serves the
   * robots waiting at its own stop, so it does not run empty to fetch the head of the line. The head at the other stop is
   * fetched when nobody waits here, or after the cab has opened here skipMax times in a row (no starvation).
   */
  private decide(): void {
    const S = this.stop;
    const aboard = this.aboard();
    if (aboard.some((r) => r.to === S)) {
      this.phase = 'opening';
      this.t = 0;
      return;
    }
    const first = aboard[0];
    if (first) {
      this.startTravel(first.to);
      return;
    }
    const head = this.queued()[0];
    if (!head) return;
    if (head.from === S) {
      this.skipped = 0;
      this.phase = 'opening';
      this.t = 0;
      return;
    }
    const here = this.queued().some((r) => r.from === S);
    if (here && this.skipped < this.skipMax) {
      this.skipped++;
      this.phase = 'opening';
      this.t = 0;
      return;
    }
    this.skipped = 0;
    this.startTravel(head.from);
  }

  private afterOpen(): void {
    this.door = 1;
    this.t = 0;
    const leaving = this.aboard().filter((r) => r.to === this.stop);
    if (leaving.length > 0) {
      for (const r of leaving) if (r.state === 'riding') r.state = 'arrived'; // defensive: a rider at the stop is ready to leave
      this.phase = 'unloading';
    } else this.beginBoarding();
  }

  /** Admits the front of this stop's line up to the free places (riders aboard are the ones still in the cab). */
  private beginBoarding(): void {
    this.t = 0;
    const room = this.cap - this.aboard().length;
    const admit = this.line(this.stop).filter((r) => r.state === 'queued').slice(0, Math.max(0, room));
    if (admit.length === 0) {
      this.phase = 'closing';
      return;
    }
    for (const r of admit) r.state = 'boarding';
    this.phase = 'boarding';
  }

  private startTravel(to: 0 | 1): void {
    if (to === this.stop || this.door > 0) return;
    this.travelFrom = this.stop;
    this.travelTo = to;
    this.travelDur = Math.max(1e-6, Math.abs(this.high - this.low) / this.speed);
    this.phase = 'travel';
    this.t = 0;
  }

  private arrive(): void {
    this.stop = this.travelTo;
    this.cabY = this.yOf(this.stop);
    for (const r of this.aboard()) if (r.to === this.stop) r.state = 'arrived';
    this.phase = 'idle';
    this.t = 0;
  }
}
