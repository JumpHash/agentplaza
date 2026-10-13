// The robots' work on the board: the behaviour engine (behaviour.ts) decides each robot's activity every decision tick
// (scoring.decision_s of simulation time). This module turns those decisions into what the floor needs:
//   - goalFor(bot): where the robot should walk now, with a key that changes when the goal does (the floor sends the
//     goal to the nav world at that point, the one navigateTo call site);
//   - arrived(bot): called every frame after the pose is read; once the robot has reached its goal, it starts the
//     activity's clip, tool and carried item (and shows the walk clip while it walks);
//   - facing(bot): the heading a station or a partner wants, or null (the floor keeps the nav pose's heading).
// The floor owns one RobotWork: noteTool reports tool calls, frame(dt, bots) runs once per frame before the poses are
// read. Nothing here calls navigateTo or uses wall-clock time or Math.random: the engine's clock is the frame time.

import behaviourJson from '../../../../../assets/robots/behaviour.json';
import { Behaviour, type BehaviourConfig, type BotIn, type BotState, type Decision, type EventIn, type SpotIn } from './behaviour';
import type { BoardDistrict, BoardLayout, TraceGraph } from './board-layout';
import type { Model, Params } from './hardware';
import type { NavGoal } from './nav';
import { boardSpots, deskSpots, districtKey, regionOf, roomSpots, type RoomFrame, type RoomItem, type RoomSpot } from './robot-spots';
import { registry } from './robots/files';
import type { Robot } from './robot';

const CONFIG = behaviourJson as unknown as BehaviourConfig;
/** A mover stands this far beside its partner (plaza units). */
const BESIDE = 1.2;
/** A robot has arrived when it stands this close to its goal and is not moving (plaza units). */
const ARRIVE = 0.6;
/** Seconds a chat message keeps its recipient as talkingTo (the engine's huddle partner), in simulation time. */
const TALK_S = 8;
/** The board slots a failure can be responded to at (a failed work item names no component). */
const FAILURE_SLOTS = ['cpu', 'memory', 'network', 'io', 'bmc', 'storage'];

/** The floor's robot as the work sees it: the agent's identity and status, its pose and its robot. */
export interface WorkBot {
  agent: { cid: number; status: string; role: string; tier: string };
  x: number;
  y: number;
  heading: number;
  moving: boolean;
  robot: Robot;
}

/** Where a robot should walk now: the nav goal, and a key that changes exactly when the goal does. */
export interface WorkGoal {
  key: string;
  goal: NavGoal;
  activity: string;
}

/** What the work keeps per robot. `seq` counts the decisions the engine changed; `started` is the seq whose activity is on. */
interface Track {
  dec: Decision | null;
  seq: number;
  started: number;
  /** The goal of the current decision (set by goalFor), and whether there is one. */
  target: { x: number; z: number };
  hasTarget: boolean;
  /** A mover's side of its partner: the unit direction from the partner, fixed when the decision changes. */
  side: { x: number; z: number };
  sideSeq: number;
  /** The clip the robot plays by hold (null: its state's clip), the tool in its grip and the item it carries. */
  shown: string | null;
  hand: string | null;
  carried: string | null;
  lastTool: string | null;
  talkingTo: number | null;
  talkUntil: number;
  /** Spot keys the nav world could not reach for this robot: sent as its avoid list until its next changed decision. */
  unreach: Set<string>;
}

function facePoint(b: WorkBot, p: WorkBot): number {
  return Math.atan2(p.x - b.x, p.y - b.y);
}

/** The straight-line walk speed (plaza units per second), the floor's fallback when it has no nav world. */
const STRAIGHT_SPEED = 1.6;

/**
 * The fallback walk: one frame of a straight line toward (gx, gz), through nothing (the nav world is what avoids
 * obstacles). Sets the robot's x, y and moving; returns the travel yaw, or null when it stands still.
 */
export function straightStep(b: { x: number; y: number; moving: boolean }, gx: number, gz: number, dt: number): number | null {
  const dx = gx - b.x;
  const dz = gz - b.y;
  const dist = Math.hypot(dx, dz);
  b.moving = dist > 0.02;
  if (!b.moving) return null;
  const step = Math.min(STRAIGHT_SPEED * dt, dist);
  b.x += (dx / dist) * step;
  b.y += (dz / dist) * step;
  return Math.atan2(dx, dz);
}

export class RobotWork {
  private readonly engine = new Behaviour(CONFIG);
  private readonly tracks = new Map<number, Track>();
  private readonly rooms = new Map<string, RoomFrame>();
  /** The board's own spots and the desks; `spotList` is what the engine reads. */
  private boardList: SpotIn[] = [];
  private homes: BoardLayout['homes'] | null = null;
  private desks: SpotIn[] = [];
  private spotList: SpotIn[] = [];
  private readonly deskKeys = new Set<string>();
  private readonly byCid = new Map<number, WorkBot>();
  private readonly prevStatus = new Map<number, string>();
  private pending: EventIn[] = [];
  /** The last stage seen of each project (a stand-up is a stage change). */
  private readonly stages = new Map<string, string>();
  private clock = 0;
  private nextTick = 0;
  private held = false;
  /** The nav component of a point (-1 off the walk), from the floor; each spot's component is cached for a second. */
  private reach: ((x: number, z: number) => number) | null = null;
  private readonly spotComp = new Map<string, number>();
  private readonly avoidDbg = new Map<number, { comp: number; local: number; avoid: number }>();
  private spotCompAt = -Infinity;

  /** The rooms are about to rebuild: their regions start again. */
  beginRooms(): void {
    this.rooms.clear();
  }

  /** A room was built on district `d` (its group turned by `turn`): its stations become the room's region. */
  addRoom(d: BoardDistrict, turn: number, spots: RoomSpot[], items: RoomItem[]): void {
    const region = this.rooms.size + 1;
    this.rooms.set(districtKey(d), { region, d, spots: roomSpots(region, d, turn, spots, items) });
    this.recombine();
  }

  /** The board's spots (components, commons, docks, traces, anywhere) for this board and layout. Call after the rooms. */
  setBoard(model: Model, params: Params, graph: TraceGraph, layout: BoardLayout): void {
    this.boardList = boardSpots(model, params, graph, layout);
    this.recombine();
  }

  /** The desks of the layout's agents. Call on every layout update (agents come and go without a rebuild). */
  setLayout(layout: BoardLayout): void {
    this.homes = layout.homes;
    this.recombine();
  }

  /** The floor reports a tool call an agent made (its transcript's tool_request). */
  noteTool(cid: number, tool: string, inp: Record<string, unknown>): void {
    const tr = this.track(cid);
    tr.lastTool = tool;
    if (tool === 'plaza_send_message' && typeof inp.to_cid === 'number') {
      tr.talkingTo = inp.to_cid;
      tr.talkUntil = this.clock + TALK_S;
    } else {
      tr.talkingTo = null;
    }
    if (tool === 'plaza_pay' && typeof inp.to === 'number') this.pending.push({ type: 'payment', to: inp.to });
  }

  /**
   * A bus frame from the hub (the floor subscribes): a project's stage or lead (a stand-up), a shared or delivered file
   * (a delivery to the orchestrator), or a failed work item (a failure to respond to). Queued for the next tick.
   */
  busFrame(kind: 'project' | 'file' | 'work', data: unknown): void {
    if (typeof data !== 'object' || data === null) return;
    const d = data as Record<string, unknown>;
    if (kind === 'project') {
      const id = typeof d.id === 'string' ? d.id : null;
      const lead = typeof d.lead_cid === 'number' ? d.lead_cid : null;
      const stage = typeof d.stage === 'string' ? d.stage : '';
      if (id === null || lead === null) return;
      if (this.stages.get(id) === stage) return;
      this.stages.set(id, stage);
      // The members: the lowest-numbered robots that are on the board, other than the lead (the feed names no members).
      const members = [...this.byCid.keys()].filter((c) => c !== lead).sort((x, y) => x - y);
      this.pending.push({ type: 'standup', conductor: lead, members: members.slice(0, CONFIG.events.standup.max) });
    } else if (kind === 'file') {
      if (d.deleted === true || typeof d.author_cid !== 'number') return;
      const to = this.orchestratorCid();
      if (to !== null && to !== d.author_cid) this.pending.push({ type: 'deliver', from: d.author_cid, to, item: 'crate' });
    } else if (kind === 'work') {
      if (d.state !== 'ended' || d.status !== 'failed' || typeof d.work !== 'number') return;
      // The feed names no component: the failure's slot is picked by its work id (a stable spread over the board's slots).
      this.pending.push({ type: 'respond', slot: FAILURE_SLOTS[d.work % FAILURE_SLOTS.length]! });
    }
  }

  /** The orchestrator's cid (the lead of the plaza's work), or null when it is not on the board. */
  private orchestratorCid(): number | null {
    for (const b of this.byCid.values()) if (b.agent.tier === 'orchestrator') return b.agent.cid;
    return null;
  }

  /** One frame, before the poses are read: the engine's clock, the status events, and a decision tick when due. `held` is the plaza's STOP ALL state. */
  frame(dt: number, bots: WorkBot[], held: boolean): void {
    this.clock += dt;
    this.held = held;
    this.byCid.clear();
    for (const b of bots) this.byCid.set(b.agent.cid, b);
    for (const cid of this.tracks.keys()) if (!this.byCid.has(cid)) this.tracks.delete(cid);
    for (const cid of this.prevStatus.keys()) if (!this.byCid.has(cid)) this.prevStatus.delete(cid);
    for (const b of bots) {
      const cid = b.agent.cid;
      if (this.prevStatus.get(cid) === 'working' && b.agent.status === 'idle') this.pending.push({ type: 'celebrate', cid });
      this.prevStatus.set(cid, b.agent.status);
    }
    if (this.clock >= this.nextTick) {
      this.nextTick = this.clock + CONFIG.scoring.decision_s;
      this.tick(bots, held);
    }
  }

  /**
   * The goal the robot walks to now, or null when it has none yet. The key changes exactly when the goal does: a
   * station's key changes with its decision; a mover's with its side of the partner (quantised, so a moving partner does
   * not re-send the goal every frame).
   */
  goalFor(b: WorkBot): WorkGoal | null {
    const tr = this.track(b.agent.cid);
    const d = tr.dec;
    if (!d) {
      tr.hasTarget = false;
      return null;
    }
    const p = d.partner !== null ? (this.byCid.get(d.partner) ?? null) : null;
    if (d.goal) {
      tr.target.x = d.goal.x;
      tr.target.z = d.goal.z;
      tr.hasTarget = true;
      return { key: `${tr.seq}`, goal: { x: d.goal.x, z: d.goal.z, face: d.goal.face ?? undefined }, activity: d.activity };
    }
    if (!p) {
      tr.hasTarget = false;
      return null;
    }
    if (tr.sideSeq !== tr.seq) {
      tr.sideSeq = tr.seq;
      const dx = b.x - p.x;
      const dz = b.y - p.y;
      const len = Math.hypot(dx, dz);
      tr.side.x = len > 1e-6 ? dx / len : -1;
      tr.side.z = len > 1e-6 ? dz / len : 0;
    }
    const x = p.x + tr.side.x * BESIDE;
    const z = p.y + tr.side.z * BESIDE;
    tr.target.x = x;
    tr.target.z = z;
    tr.hasTarget = true;
    const qx = Math.round(x * 2);
    const qz = Math.round(z * 2);
    return { key: `${tr.seq}:${qx},${qz}`, goal: { x, z }, activity: d.activity };
  }

  /**
   * Called every frame after the robot's pose is read. Walking shows the walk clip and puts down the last activity's tool
   * and item; once the robot has reached its goal, the decision's activity starts (its clip, tool and carried item).
   */
  arrived(b: WorkBot): void {
    const tr = this.track(b.agent.cid);
    const d = tr.dec;
    if (!d || tr.started === tr.seq) return;
    const reached = tr.hasTarget && !b.moving && Math.hypot(b.x - tr.target.x, b.y - tr.target.z) < ARRIVE;
    if (!reached) {
      if (tr.shown !== null) {
        tr.shown = null;
        b.robot.play(null);
      }
      if (tr.hand !== null) {
        tr.hand = null;
        b.robot.hold(null);
      }
      if (tr.carried !== null) {
        tr.carried = null;
        b.robot.carry(null);
      }
      return;
    }
    tr.started = tr.seq;
    const clip = d.clip === '@walk' ? null : d.clip;
    tr.shown = clip;
    b.robot.play(clip, { loop: true });
    tr.hand = d.tool;
    b.robot.hold(d.tool);
    tr.carried = d.carry;
    b.robot.carry(d.carry);
  }

  /**
   * The heading a station or a partner wants (the floor eases to it), or null (the floor keeps the nav pose's heading):
   * a host faces its partner; a mover faces its partner once it stands beside it; a station faces its arrival yaw.
   */
  facing(b: WorkBot): number | null {
    const tr = this.track(b.agent.cid);
    const d = tr.dec;
    if (!d) return null;
    const p = d.partner !== null ? (this.byCid.get(d.partner) ?? null) : null;
    if (p) {
      if (d.goal) return facePoint(b, p);
      return tr.started === tr.seq ? facePoint(b, p) : null;
    }
    if (tr.started === tr.seq && d.goal && d.goal.face !== null) return d.goal.face;
    return null;
  }

  /** Whether a robot is in a chat or a huddle right now (its label shows the talk dots). */
  talking(cid: number): boolean {
    const d = this.tracks.get(cid)?.dec;
    return !!d && d.partner !== null && (d.activity === 'chat' || d.activity === 'huddle');
  }

  /** The nav component of a point (-1 off the walk), from the nav world: a robot's and a spot's reachability. */
  setReach(component: (x: number, z: number) => number): void {
    this.reach = component;
    this.spotComp.clear();
  }

  /**
   * The nav world gave up a robot's walk (no progress for a while; its goal is cleared). The robot's spot goes on its
   * avoid list until its next changed decision, so the engine picks another spot. The decision counts as arrived where
   * the robot stands, so a new decision's clock starts when the robot reaches its new goal. Its tool and item are put down.
   */
  unreachable(cid: number): void {
    const tr = this.tracks.get(cid);
    const b = this.byCid.get(cid);
    if (!tr || !tr.dec) return;
    if (tr.dec.spot !== null) tr.unreach.add(tr.dec.spot);
    tr.hasTarget = false;
    tr.started = tr.seq;
    if (tr.shown !== null) {
      tr.shown = null;
      b?.robot.play(null);
    }
    if (tr.hand !== null) {
      tr.hand = null;
      b?.robot.hold(null);
    }
    if (tr.carried !== null) {
      tr.carried = null;
      b?.robot.carry(null);
    }
  }

  /** Debug (window.__apzBots): what the work holds for a robot, and the engine clock and held flag. Read-only. */
  debugOf(cid: number): Record<string, unknown> {
    const tr = this.tracks.get(cid);
    const d = tr?.dec ?? null;
    return {
      clock: this.clock,
      held: this.held,
      seq: tr?.seq ?? null,
      started: tr?.started ?? null,
      hasTarget: tr?.hasTarget ?? null,
      unreach: tr ? [...tr.unreach] : null,
      avoidDbg: this.avoidDbg.get(cid) ?? null,
      target: tr ? { x: tr.target.x, z: tr.target.z } : null,
      shown: tr?.shown ?? null,
      hand: tr?.hand ?? null,
      carried: tr?.carried ?? null,
      dec: d ? { activity: d.activity, clip: d.clip, tool: d.tool, carry: d.carry, spot: d.spot, goal: d.goal, partner: d.partner, until: d.until, changed: d.changed } : null,
    };
  }

  private track(cid: number): Track {
    let tr = this.tracks.get(cid);
    if (!tr) {
      tr = {
        dec: null,
        seq: 0,
        started: -1,
        target: { x: 0, z: 0 },
        hasTarget: false,
        side: { x: 0, z: 0 },
        sideSeq: -1,
        shown: null,
        hand: null,
        carried: null,
        lastTool: null,
        talkingTo: null,
        talkUntil: 0,
        unreach: new Set(),
      };
      this.tracks.set(cid, tr);
    }
    return tr;
  }

  /** Rebuilds the spot list. Desks are recomputed here too: a room built after the layout gives its desks their region. */
  private recombine(): void {
    this.desks = this.homes ? deskSpots(this.homes, (d) => this.rooms.get(districtKey(d))?.region ?? 0) : [];
    this.deskKeys.clear();
    for (const s of this.desks) this.deskKeys.add(s.key);
    const all: SpotIn[] = [...this.boardList, ...this.desks];
    for (const r of this.rooms.values()) all.push(...r.spots);
    this.spotList = all;
  }

  /** The engine's input from the floor's robots, then its decisions (a changed decision starts a new activity). */
  private tick(bots: WorkBot[], held: boolean): void {
    const reg = registry();
    // A spot's nav component is read once a second (spots do not move); a robot's own is read every tick.
    if (this.clock - this.spotCompAt >= 1) {
      this.spotCompAt = this.clock;
      this.spotComp.clear();
    }
    const robots: BotIn[] = [];
    for (const b of bots) {
      const cid = b.agent.cid;
      const tr = this.track(cid);
      const sp = reg.species[b.robot.species] ?? reg.species.rover!;
      robots.push({
        cid,
        role: b.agent.tier === 'orchestrator' ? 'orchestrator' : b.agent.role,
        personality: sp.personality,
        state: b.agent.status as BotState,
        lastTool: tr.lastTool,
        talkingTo: tr.talkingTo !== null && this.clock < tr.talkUntil ? tr.talkingTo : null,
        at: { x: b.x, z: b.y, region: regionOf(this.rooms.values(), b.x, b.y) },
        desk: this.deskKeys.has(`desk:${cid}`) ? `desk:${cid}` : null,
        quirks: sp.quirks,
        arrived: tr.started === tr.seq || !tr.hasTarget,
        avoid: this.avoidFor(b, tr),
      });
    }
    const events = this.pending;
    this.pending = [];
    const decisions = this.engine.step({ now: this.clock, robots, spots: this.spotList, held, events });
    for (const d of decisions) {
      const tr = this.track(d.cid);
      if (d.changed) {
        tr.seq += 1;
        tr.unreach.clear(); // the avoid list was used for this decision; the next one starts clean
      }
      tr.dec = d;
    }
  }

  /**
   * The spots a robot cannot be sent to: its unreachable spots, and, while it has a local spot free (one in its nav
   * component, not unreachable), every spot outside that component. A robot with no local spot keeps only its unreachable
   * ones (so it falls back to the rest, and never walks at a wall trying to reach an island).
   */
  private avoidFor(b: WorkBot, tr: Track): string[] {
    const avoid = new Set<string>(tr.unreach);
    const rc = this.reach ? this.reach(b.x, b.y) : -1;
    if (rc < 0) return this.noteAvoid(b.agent.cid, rc, 0, [...avoid]);
    let local = 0;
    let free = false;
    const far: string[] = [];
    for (const s of this.spotList) {
      if (this.spotCompOf(s) === rc) {
        local += 1;
        if (!tr.unreach.has(s.key)) free = true;
      } else far.push(s.key);
    }
    if (free) for (const k of far) avoid.add(k);
    return this.noteAvoid(b.agent.cid, rc, local, [...avoid]);
  }

  /** Debug (window.__apzBots): a robot's component, its local spots and its avoid list length, as of the last tick. */
  private noteAvoid(cid: number, comp: number, local: number, avoid: string[]): string[] {
    this.avoidDbg.set(cid, { comp, local, avoid: avoid.length });
    return avoid;
  }

  /** A spot's nav component (-1 off the walk), cached until the next second's refresh. */
  private spotCompOf(s: SpotIn): number {
    let c = this.spotComp.get(s.key);
    if (c === undefined) {
      c = this.reach ? this.reach(s.x, s.z) : -1;
      this.spotComp.set(s.key, c);
    }
    return c;
  }
}
