// Robot behaviour engine (pure logic: no three.js, no nav, no DOM). Decides, per robot and per tick, the activity,
// clip, tool, carried item, place (a spot, a partner or here) and until-time. The floor walks robots to the goal
// (nav.ts) and plays the clip. Rules: docs/design/robots.md ("What they do: the behaviour engine"); tables:
// assets/robots/behaviour.json. desktop/world/behaviour.rs is the same algorithm, proven equal by
// assets/robots/behaviour-fixtures.json (runScenario hashes must match in both apps).
//
// Determinism: every decision draws from mulberry32 seeded by (cid, the robot's decision counter), in a fixed order;
// robots are processed in ascending cid; events come before robots. Only + - * / and sqrt touch floats (no trig),
// so both apps compute bit-identical numbers.

export interface Personality {
  social: number;
  energy: number;
  focus: number;
  curiosity: number;
  play: number;
  tidy: number;
}

export type BotState = 'idle' | 'working' | 'offline';

export interface Pos {
  x: number;
  z: number;
  region: number;
}

/** A robot as the floor reports it each tick. `lastTool` is the latest tool call (null: none yet). */
export interface BotIn {
  cid: number;
  role: string;
  personality: Personality;
  state: BotState;
  lastTool?: string | null;
  /** The cid this robot is messaging (the floor's activity line), or null. */
  talkingTo?: number | null;
  at: Pos;
  /** The key of this robot's own desk spot, or null. */
  desk?: string | null;
  /** The species' quirks: clip ids (own or shared) with weights. */
  quirks?: { clip: string; weight: number }[];
  /** Whether the robot stands at its decision's goal. Undefined counts as arrived. A decision that has not arrived holds its
   *  `until` (its duration starts at arrival; see ARRIVE_CAP_S). */
  arrived?: boolean;
  /** Spot keys this robot cannot reach (the nav layer gave up on them). A decision that holds one is made again without it. */
  avoid?: string[];
}

/** A place a robot can stand at. `kind`: desk, prop:<id>, component:<slot>, commons, dock, trace, anywhere. */
export interface SpotIn {
  key: string;
  kind: string;
  region: number;
  x: number;
  z: number;
  face: number;
  clip: string | null;
}

export type EventIn =
  | { type: 'celebrate'; cid: number }
  | { type: 'standup'; conductor: number; members: number[] }
  | { type: 'respond'; slot: string }
  | { type: 'payment'; to: number }
  | { type: 'deliver'; from: number; to: number; item?: string };

export interface TickIn {
  /** Simulation seconds. */
  now: number;
  robots: BotIn[];
  spots: SpotIn[];
  /** STOP ALL: everyone idles; no work; no events. */
  held: boolean;
  events: EventIn[];
}

export interface Goal {
  x: number;
  z: number;
  region: number;
  /** World yaw to face on arrival; null: keep the current heading (or face the partner). */
  face: number | null;
}

export interface Decision {
  cid: number;
  activity: string;
  /** The clip id; "@walk" and "@quirk" are the species' walk clip and the quirk's clip (the floor resolves them). */
  clip: string;
  tool: string | null;
  carry: string | null;
  /** The reserved spot key, or null. */
  spot: string | null;
  /** Where to walk. Null: walk to `partner` (a mover), or stay (no goal and no partner). */
  goal: Goal | null;
  /** A partner robot: a mover walks beside it; a host (goal set, partner set) stays and faces it. */
  partner: number | null;
  until: number;
  /** True when this robot decided on this tick. */
  changed: boolean;
}

export interface WorkEntry {
  activity: string;
  w: number;
  slot?: string;
}

export interface RoleDef {
  toolbox: string[];
  props: string[];
  work: Record<string, WorkEntry[]>;
}

export type RoleEntry = RoleDef | { alias: string };

export interface ActivityDef {
  clip: string;
  tools: string[];
  carry?: string;
  place: string[];
  dur: number[];
  min_s: number;
  pairs?: string;
  host_clip?: string;
}

export interface IdleDef {
  activity: string;
  base: number;
  traits: Record<string, number>;
  gate?: { trait: string; min: number };
}

export interface BehaviourConfig {
  categories: string[];
  tools: { match: string; category: string }[];
  roles: Record<string, RoleEntry>;
  activities: Record<string, ActivityDef>;
  idle: IdleDef[];
  events: {
    celebrate: { activity: string; cheer: { activity: string; max: number; radius: number } };
    standup: { conduct: string; member: string; max: number };
    respond: { activity: string; role: string; max: number };
    payment: { activity: string; carry: string; role: string };
    deliver: { activity: string; carry: string };
  };
  scoring: {
    decision_s: number;
    trait_floor: number;
    novelty: number;
    distance_scale: number;
    region_penalty: number;
    rest_gap_s: number;
    group_max: number;
    friend_bonus: number;
  };
}

type Kind = 'work' | 'idle' | 'event' | 'pair' | 'dock';

/** A decision as the engine keeps it (the public fields plus what the rules need later). */
interface Dec extends Decision {
  kind: Kind;
  category: string;
  minUntil: number;
  held: boolean;
  state: string;
  novelty: string;
  /** The drawn duration, when the decision was made; `until` is now + dur once it has arrived. */
  dur: number;
  since: number;
  /** Still walking to its goal: `until` is held at Infinity. */
  pending: boolean;
}

interface Mem {
  counter: number;
  dec: Dec | null;
  last: string;
  lastNap: number;
  friends: Map<number, number>;
}

type Loc =
  | { kind: 'spot'; spot: SpotIn; d: number }
  | { kind: 'here'; d: number }
  | { kind: 'partner'; cid: number; d: number }
  | { kind: 'point'; goal: Goal; d: number };

const FNV_OFFSET = 0x811c9dc5;
const FNV_PRIME = 0x01000193;
/** Never-nap sentinel: a robot that has never napped is fully rested. */
const NEVER = -1e9;
/** A decision's duration starts at arrival, or when this long has passed since the decision (a robot that cannot get there). */
const ARRIVE_CAP_S = 12;
/** A decision's goal further than this (plaza units, planar) is walked to; the floor's stale arrival flag is not trusted for it. */
const ARRIVE_NEAR = 0.6;

/** FNV-1a (32 bit) over the UTF-16 code units of `s` (ASCII in practice), continuing from `h`. */
export function fnvAppend(h: number, s: string): number {
  let x = h >>> 0;
  for (let i = 0; i < s.length; i++) {
    x ^= s.charCodeAt(i);
    x = Math.imul(x, FNV_PRIME) >>> 0;
  }
  return x >>> 0;
}

export function hex8(h: number): string {
  return (h >>> 0).toString(16).padStart(8, '0');
}

/** mulberry32: a seeded generator of floats in [0, 1). */
export function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** The seed of a robot's n-th decision (n counts from 1). */
export function seedOf(cid: number, counter: number): number {
  return (Math.imul(cid >>> 0, 0x9e3779b1) + Math.imul(counter >>> 0, 0x85ebca77) + 0x27d4eb2f) >>> 0;
}

/** Index of the weighted pick for `u` in [0, 1); -1 when no weight is positive. */
export function pickIndex(weights: number[], u: number): number {
  let total = 0;
  for (const w of weights) total += w;
  if (!(total > 0)) return -1;
  let x = u * total;
  let last = -1;
  for (let i = 0; i < weights.length; i++) {
    const w = weights[i]!;
    if (w <= 0) continue;
    last = i;
    x -= w;
    if (x < 0) return i;
  }
  return last;
}

function hereGoal(b: BotIn): Goal {
  return { x: b.at.x, z: b.at.z, region: b.at.region, face: null };
}

/** Whether a goal is further than ARRIVE_NEAR from a position (planar). */
function farFrom(at: Pos, g: Goal): boolean {
  const dx = at.x - g.x;
  const dz = at.z - g.z;
  return Math.sqrt(dx * dx + dz * dz) > ARRIVE_NEAR;
}

function spotGoal(s: SpotIn): Goal {
  return { x: s.x, z: s.z, region: s.region, face: s.face };
}

export class Behaviour {
  private readonly cfg: BehaviourConfig;
  private readonly mems = new Map<number, Mem>();
  /** Spot key -> the cid holding it. */
  private readonly reserved = new Map<string, number>();
  private now = 0;
  private held = false;
  /** This tick's robots by cid (ascending). */
  private bots = new Map<number, BotIn>();
  /** This tick's spots sorted by key, and by key. */
  private spots: SpotIn[] = [];
  private spotByKey = new Map<string, SpotIn>();
  /** Robots whose decision was set this tick by an event or as a host: not decided again this tick. */
  private assigned = new Set<number>();
  private changed = new Set<number>();

  constructor(cfg: BehaviourConfig) {
    this.cfg = cfg;
  }

  /** The tool category ("think" for no tool call yet, "default" for an unknown tool). */
  categoryOf(tool: string | null | undefined): string {
    if (tool == null) return 'think';
    for (const t of this.cfg.tools) if (t.match === tool) return t.category;
    for (const t of this.cfg.tools) if (t.match.endsWith('*') && tool.startsWith(t.match.slice(0, -1))) return t.category;
    return 'default';
  }

  /** Advances every robot by one tick. Pure given the inputs and the engine's own state (the decision history). */
  step(input: TickIn): Decision[] {
    this.now = input.now;
    this.held = input.held;
    this.bots = new Map();
    for (const b of [...input.robots].sort((a, b) => a.cid - b.cid)) this.bots.set(b.cid, b);
    this.spots = [...input.spots].sort((a, b) => (a.key < b.key ? -1 : a.key > b.key ? 1 : 0));
    this.spotByKey = new Map(this.spots.map((s) => [s.key, s]));
    this.assigned = new Set();
    this.changed = new Set();
    for (const cid of [...this.mems.keys()]) if (!this.bots.has(cid)) this.mems.delete(cid);
    for (const [key, cid] of [...this.reserved]) if (!this.bots.has(cid) || !this.spotByKey.has(key)) this.reserved.delete(key);

    for (const ev of input.events) this.event(ev);
    // A decision that was walking starts its duration when its robot arrives (or at the cap). Robots in cid order.
    for (const b of this.bots.values()) {
      const d = this.mems.get(b.cid)?.dec;
      if (!d || !d.pending) continue;
      if (b.arrived !== false || this.now >= d.since + ARRIVE_CAP_S) {
        d.pending = false;
        d.until = this.now + d.dur;
        if (d.partner !== null) {
          const h = this.mems.get(d.partner)?.dec;
          if (h && h.partner === b.cid) h.until = d.until;
        }
      }
    }
    // Up to three passes: a decision can break a partner's link, and that robot then decides in the same tick.
    for (let pass = 0; pass < 3; pass++) {
      let any = false;
      for (const b of this.bots.values()) {
        if (this.assigned.has(b.cid)) continue;
        if (this.due(b, this.mem(b.cid))) {
          this.decide(b);
          any = true;
        }
      }
      if (!any) break;
    }
    return [...this.bots.keys()].map((cid) => {
      const d = this.mem(cid).dec!;
      return {
        cid,
        activity: d.activity,
        clip: d.clip,
        tool: d.tool,
        carry: d.carry,
        spot: d.spot,
        goal: d.goal,
        partner: d.partner,
        until: d.until,
        changed: this.changed.has(cid),
      };
    });
  }

  private mem(cid: number): Mem {
    let m = this.mems.get(cid);
    if (!m) {
      m = { counter: 0, dec: null, last: '', lastNap: NEVER, friends: new Map() };
      this.mems.set(cid, m);
    }
    return m;
  }

  private rngFor(cid: number, m: Mem): () => number {
    m.counter += 1;
    return mulberry32(seedOf(cid, m.counter));
  }

  /** The state the rules use: held reads an online robot as idle, and offline stays offline. */
  private effState(b: BotIn): string {
    return b.state === 'offline' ? 'offline' : this.held ? 'idle' : b.state;
  }

  private role(name: string): RoleDef {
    let e = this.cfg.roles[name] ?? this.cfg.roles.other!;
    if ('alias' in e) e = this.cfg.roles[e.alias]!;
    return e as RoleDef;
  }

  private distance(a: Pos, b: Pos): number {
    const dx = a.x - b.x;
    const dz = a.z - b.z;
    return Math.sqrt(dx * dx + dz * dz) + (a.region === b.region ? 0 : this.cfg.scoring.region_penalty);
  }

  private free(key: string, cid: number): boolean {
    const h = this.reserved.get(key);
    return h === undefined || h === cid;
  }

  /** Whether a robot is part of a pair or group (it has a partner, or another robot points at it). */
  private isLinked(cid: number): boolean {
    for (const [c, m] of this.mems) {
      if (!m.dec) continue;
      if (m.dec.partner === cid) return true;
      if (c === cid && m.dec.partner !== null) return true;
    }
    return false;
  }

  /** Free for an event: online, not held in a pair or group, not already decided this tick. */
  private eventFree(b: BotIn): boolean {
    return this.effState(b) !== 'offline' && !this.assigned.has(b.cid) && !this.isLinked(b.cid);
  }

  private due(b: BotIn, m: Mem): boolean {
    const d = m.dec;
    if (!d) return true;
    const eff = this.effState(b);
    if (eff !== d.state || d.held !== this.held) return true;
    if (d.spot !== null && (b.avoid ?? []).includes(d.spot)) return true;
    if (this.now >= d.until) return true;
    if (d.partner !== null) {
      const p = this.bots.get(d.partner);
      if (!p || this.effState(p) === 'offline') return true;
      // A pair (chat, talk, host) is mutual: when the other half has moved on, decide again. Events and reviews are not.
      const pairs = this.cfg.activities[d.activity]!.pairs;
      if (d.kind !== 'event' && pairs !== 'review' && this.mems.get(d.partner)?.dec?.partner !== b.cid) return true;
    }
    if (d.kind === 'work' && eff === 'working' && this.categoryOf(b.lastTool) !== d.category && this.now >= d.minUntil) return true;
    return false;
  }

  /** Frees a robot's spot and makes its partner decide again (it was the other half of a pair). */
  private release(b: BotIn, m: Mem): void {
    const d = m.dec;
    if (!d) return;
    if (d.spot !== null && this.reserved.get(d.spot) === b.cid) this.reserved.delete(d.spot);
    if (d.partner !== null) {
      const p = this.mems.get(d.partner);
      if (p?.dec && p.dec.partner === b.cid) p.dec.until = this.now;
    }
    m.dec = null;
  }

  private traitValue(b: BotIn, m: Mem, key: string): number {
    if (key === 'rest') return Math.min(9, (9 * (this.now - m.lastNap)) / this.cfg.scoring.rest_gap_s);
    if (key.startsWith('-')) return 9 - (b.personality as unknown as Record<string, number>)[key.slice(1)]!;
    return (b.personality as unknown as Record<string, number>)[key] ?? 0;
  }

  private fit(b: BotIn, m: Mem, traits: Record<string, number>): number {
    const keys = Object.keys(traits).sort();
    if (keys.length === 0) return 1;
    let f = this.cfg.scoring.trait_floor;
    for (const k of keys) f += traits[k]! * (this.traitValue(b, m, k) / 9);
    return f;
  }

  /** The location of activity `actId` for `b`: the first place token that has a spot (or a partner), else null. */
  private locate(b: BotIn, actId: string, slot: string | null, chat: { cid: number; d: number } | null): Loc | null {
    const act = this.cfg.activities[actId]!;
    const props = this.role(b.role).props;
    for (const tok of act.place) {
      if (tok === 'here') return { kind: 'here', d: 0 };
      if (tok === 'partner') {
        const p = this.partnerFor(b, act, chat);
        if (p) return { kind: 'partner', cid: p.cid, d: p.d };
        continue;
      }
      if (tok === 'desk') {
        const s = b.desk != null ? this.spotByKey.get(b.desk) : undefined;
        if (s && this.free(s.key, b.cid) && !(b.avoid ?? []).includes(s.key)) return { kind: 'spot', spot: s, d: this.distance(b.at, s) };
        continue;
      }
      if (tok.startsWith('prop:') && !props.includes(tok.slice(5))) continue;
      const want = tok === 'component' && slot ? `component:${slot}` : tok;
      let best: SpotIn | null = null;
      let bestD = Infinity;
      for (const s of this.spots) {
        if (!this.matches(s.kind, want) || !this.free(s.key, b.cid) || (b.avoid ?? []).includes(s.key)) continue;
        const d = this.distance(b.at, s);
        if (d < bestD) {
          best = s;
          bestD = d;
        }
      }
      if (best) return { kind: 'spot', spot: best, d: bestD };
    }
    return null;
  }

  private matches(kind: string, want: string): boolean {
    return want === 'component' ? kind.startsWith('component:') : kind === want;
  }

  private partnerFor(b: BotIn, act: ActivityDef, chat: { cid: number; d: number } | null): { cid: number; d: number } | null {
    if (act.pairs === 'chat') return chat;
    if (act.pairs === 'review') {
      let best: { cid: number; d: number; pref: number } | null = null;
      for (const o of this.bots.values()) {
        if (o.cid === b.cid || this.effState(o) !== 'working' || this.assigned.has(o.cid) || this.isLinked(o.cid)) continue;
        if (this.mems.get(o.cid)?.dec?.kind !== 'work') continue;
        const pref = this.categoryOf(o.lastTool) === 'ship' || this.categoryOf(o.lastTool) === 'review' ? 0 : 1;
        const d = this.distance(b.at, o.at);
        if (!best || pref < best.pref || (pref === best.pref && d < best.d)) best = { cid: o.cid, d, pref };
      }
      return best ? { cid: best.cid, d: best.d } : null;
    }
    if (act.pairs === 'talk') {
      if (b.talkingTo == null) return null;
      const o = this.bots.get(b.talkingTo);
      if (!o || o.cid === b.cid || this.effState(o) === 'offline' || this.assigned.has(o.cid) || this.isLinked(o.cid)) return null;
      const k = this.mems.get(o.cid)?.dec?.kind;
      if (k !== undefined && k !== 'idle' && k !== 'work') return null;
      return { cid: o.cid, d: this.distance(b.at, o.at) };
    }
    return null;
  }

  /** A willing idle partner for a chat: candidates ranked by friendship and distance; each rolls against its social. */
  private chatPick(b: BotIn, rng: () => number): { cid: number; d: number } | null {
    const cands: { cid: number; d: number; score: number }[] = [];
    const friends = this.mems.get(b.cid)?.friends;
    for (const o of this.bots.values()) {
      if (o.cid === b.cid || this.assigned.has(o.cid) || this.effState(o) !== 'idle') continue;
      const k = this.mems.get(o.cid)?.dec?.kind;
      if ((k !== undefined && k !== 'idle') || this.isLinked(o.cid)) continue;
      const d = this.distance(b.at, o.at);
      const f = friends?.get(o.cid) ?? 0;
      cands.push({ cid: o.cid, d, score: (1 + this.cfg.scoring.friend_bonus * f) / (1 + d / this.cfg.scoring.distance_scale) });
    }
    cands.sort((x, y) => (y.score !== x.score ? y.score - x.score : x.cid - y.cid));
    for (const c of cands) {
      if (rng() * 9 < this.bots.get(c.cid)!.personality.social) return { cid: c.cid, d: c.d };
    }
    return null;
  }

  private pickTool(b: BotIn, act: ActivityDef, rng: () => number): string | null {
    if (act.tools.length === 0) return null;
    const toolbox = this.role(b.role).toolbox;
    if (act.tools[0] === '$first') return toolbox[0] ?? null;
    const inBox = act.tools.filter((t) => toolbox.includes(t));
    if (inBox.length === 0) return act.tools[0]!;
    if (inBox.length === 1) return inBox[0]!;
    const u = rng();
    return inBox[Math.floor(u * inBox.length)]!;
  }

  private instantiate(
    b: BotIn,
    rng: () => number,
    kind: Kind,
    actId: string,
    loc: Loc,
    o: { clip?: string; carry?: string; category?: string; novelty?: string } = {},
  ): Dec {
    const act = this.cfg.activities[actId]!;
    const tool = this.pickTool(b, act, rng);
    const u = rng();
    const dur = act.dur[0]! + (act.dur[1]! - act.dur[0]!) * u;
    const goal = loc.kind === 'spot' ? spotGoal(loc.spot) : loc.kind === 'point' ? loc.goal : loc.kind === 'here' ? hereGoal(b) : null;
    // The input's arrival describes the robot's previous goal, so a new decision walks when its goal is far (the input's
    // undefined arrival keeps the old rule: no walk).
    const pending = b.arrived !== undefined && (b.arrived === false || (goal !== null && farFrom(b.at, goal)));
    return {
      cid: b.cid,
      activity: actId,
      clip: o.clip ?? act.clip,
      tool,
      carry: o.carry ?? act.carry ?? null,
      spot: loc.kind === 'spot' ? loc.spot.key : null,
      goal,
      partner: loc.kind === 'partner' ? loc.cid : null,
      until: pending ? Infinity : this.now + dur,
      changed: false,
      kind,
      category: o.category ?? 'idle',
      minUntil: this.now + act.min_s,
      held: this.held,
      state: this.effState(b),
      novelty: o.novelty ?? actId,
      dur,
      since: this.now,
      pending,
    };
  }

  private commit(b: BotIn, m: Mem, d: Dec): void {
    if (d.spot !== null) this.reserved.set(d.spot, b.cid);
    m.dec = d;
    m.last = d.novelty;
    this.changed.add(b.cid);
  }

  /** Makes the partner of a mover a host: it stays where it is and plays the pair's host clip until the same time. */
  private host(mover: BotIn, mm: Mem, d: Dec): void {
    const pid = d.partner!;
    const h = this.bots.get(pid)!;
    const hm = this.mem(pid);
    this.release(h, hm);
    const act = this.cfg.activities[d.activity]!;
    hm.dec = {
      cid: pid,
      activity: d.activity,
      clip: act.host_clip ?? act.clip,
      tool: null,
      carry: null,
      spot: null,
      goal: hereGoal(h),
      partner: mover.cid,
      until: d.until,
      changed: false,
      kind: 'pair',
      category: 'pair',
      minUntil: this.now,
      held: this.held,
      state: this.effState(h),
      novelty: d.novelty,
      dur: d.dur,
      since: this.now,
      pending: false,
    };
    this.changed.add(pid);
    this.assigned.add(pid);
    if (act.pairs === 'chat') {
      mm.friends.set(pid, (mm.friends.get(pid) ?? 0) + 1);
      hm.friends.set(mover.cid, (hm.friends.get(mover.cid) ?? 0) + 1);
    }
  }

  private locateOr(b: BotIn, actId: string, slot: string | null): Loc {
    return this.locate(b, actId, slot, null) ?? { kind: 'here', d: 0 };
  }

  private decide(b: BotIn): void {
    const m = this.mem(b.cid);
    this.release(b, m);
    const rng = this.rngFor(b.cid, m);
    const eff = this.effState(b);

    if (eff === 'offline') {
      this.commit(b, m, this.instantiate(b, rng, 'dock', 'powered-down', this.locateOr(b, 'powered-down', null)));
      return;
    }

    if (eff === 'working') {
      if (b.lastTool == null) {
        this.commit(b, m, this.instantiate(b, rng, 'work', 'think', this.locateOr(b, 'think', null), { category: 'think' }));
        return;
      }
      const cat = this.categoryOf(b.lastTool);
      const r = this.role(b.role);
      const table = r.work[cat] ?? r.work.default ?? [];
      const cands: { e: WorkEntry; loc: Loc }[] = [];
      for (const e of table) {
        const loc = this.locate(b, e.activity, e.slot ?? null, null);
        if (loc) cands.push({ e, loc });
      }
      const idx = pickIndex(
        cands.map((c) => c.e.w),
        rng(),
      );
      if (idx < 0) {
        this.commit(b, m, this.instantiate(b, rng, 'work', 'think', this.locateOr(b, 'think', null), { category: 'think' }));
        return;
      }
      const c = cands[idx]!;
      const d = this.instantiate(b, rng, 'work', c.e.activity, c.loc, { category: cat });
      this.commit(b, m, d);
      if (d.partner !== null && this.cfg.activities[d.activity]!.pairs !== 'review') this.host(b, m, d);
      return;
    }

    // Idle (including a held robot).
    const chat = this.chatPick(b, rng);
    const entries: { actId: string; clip?: string; key: string; loc: Loc; base: number; traits: Record<string, number> }[] = [];
    for (const def of this.cfg.idle) {
      if (def.gate && ((b.personality as unknown as Record<string, number>)[def.gate.trait] ?? 0) < def.gate.min) continue;
      if (def.activity === 'quirk') {
        for (const q of b.quirks ?? []) {
          const loc = this.locate(b, 'quirk', null, null);
          if (loc) entries.push({ actId: 'quirk', clip: q.clip, key: `quirk:${q.clip}`, loc, base: q.weight, traits: {} });
        }
        continue;
      }
      const loc = this.locate(b, def.activity, null, chat);
      if (loc) entries.push({ actId: def.activity, key: def.activity, loc, base: def.base, traits: def.traits });
    }
    const scores: number[] = [];
    for (const e of entries) {
      const nov = m.last === e.key ? this.cfg.scoring.novelty : 1;
      const df = 1 / (1 + e.loc.d / this.cfg.scoring.distance_scale);
      scores.push(e.base * this.fit(b, m, e.traits) * nov * df);
    }
    const idx = pickIndex(scores, rng());
    const e = entries[idx < 0 ? entries.findIndex((x) => x.actId === 'stand') : idx]!;
    const d = this.instantiate(b, rng, 'idle', e.actId, e.loc, { clip: e.clip, novelty: e.key });
    this.commit(b, m, d);
    if (e.actId === 'nap') m.lastNap = this.now;
    if (d.partner !== null) this.host(b, m, d);
  }

  private event(ev: EventIn): void {
    if (this.held) return;
    const cfg = this.cfg.events;
    if (ev.type === 'celebrate') {
      const c = this.bots.get(ev.cid);
      if (!c || !this.eventFree(c)) return;
      const cm = this.mem(c.cid);
      this.release(c, cm);
      this.commit(c, cm, this.instantiate(c, this.rngFor(c.cid, cm), 'event', cfg.celebrate.activity, { kind: 'here', d: 0 }));
      this.assigned.add(c.cid);
      const near = [...this.bots.values()]
        .filter(
          (o) =>
            o.cid !== c.cid &&
            this.effState(o) === 'idle' &&
            (this.mems.get(o.cid)?.dec?.kind ?? 'idle') === 'idle' &&
            !this.assigned.has(o.cid) &&
            !this.isLinked(o.cid) &&
            o.at.region === c.at.region &&
            this.distance(c.at, o.at) <= cfg.celebrate.cheer.radius,
        )
        .sort((x, y) => {
          const dx = this.distance(c.at, x.at);
          const dy = this.distance(c.at, y.at);
          return dx !== dy ? dx - dy : x.cid - y.cid;
        })
        .slice(0, cfg.celebrate.cheer.max);
      for (const o of near) {
        const om = this.mem(o.cid);
        this.release(o, om);
        const d = this.instantiate(o, this.rngFor(o.cid, om), 'event', cfg.celebrate.cheer.activity, { kind: 'here', d: 0 });
        d.partner = c.cid;
        this.commit(o, om, d);
        this.assigned.add(o.cid);
      }
      return;
    }
    if (ev.type === 'standup') {
      const c = this.bots.get(ev.conductor);
      if (!c || !this.eventFree(c)) return;
      const max = Math.min(cfg.standup.max, this.cfg.scoring.group_max - 1);
      const members = [...new Set(ev.members)]
        .sort((x, y) => x - y)
        .map((cid) => this.bots.get(cid))
        .filter((o): o is BotIn => !!o && o.cid !== c.cid && this.eventFree(o))
        .slice(0, max);
      if (members.length === 0) return;
      const cm = this.mem(c.cid);
      this.release(c, cm);
      this.commit(c, cm, this.instantiate(c, this.rngFor(c.cid, cm), 'event', cfg.standup.conduct, this.locateOr(c, cfg.standup.conduct, null)));
      this.assigned.add(c.cid);
      for (const o of members) {
        const om = this.mem(o.cid);
        const loc = this.locateOr(o, cfg.standup.member, null);
        this.release(o, om);
        const d = this.instantiate(o, this.rngFor(o.cid, om), 'event', cfg.standup.member, loc);
        d.partner = c.cid;
        this.commit(o, om, d);
        this.assigned.add(o.cid);
      }
      return;
    }
    if (ev.type === 'respond') {
      let n = 0;
      for (const o of [...this.bots.values()]) {
        if (n >= cfg.respond.max) break;
        if (o.role !== cfg.respond.role || !this.eventFree(o)) continue;
        const loc = this.locate(o, cfg.respond.activity, ev.slot, null);
        if (!loc) continue;
        const om = this.mem(o.cid);
        this.release(o, om);
        this.commit(o, om, this.instantiate(o, this.rngFor(o.cid, om), 'event', cfg.respond.activity, loc));
        this.assigned.add(o.cid);
        n++;
      }
      return;
    }
    if (ev.type === 'payment') {
      const to = this.bots.get(ev.to);
      if (!to) return;
      const f = [...this.bots.values()]
        .filter((o) => o.role === cfg.payment.role && this.eventFree(o))
        .sort((x, y) => {
          const dx = this.distance(to.at, x.at);
          const dy = this.distance(to.at, y.at);
          return dx !== dy ? dx - dy : x.cid - y.cid;
        })[0];
      if (!f) return;
      this.carryTo(f, to, cfg.payment.activity, cfg.payment.carry);
      return;
    }
    if (ev.type === 'deliver') {
      const from = this.bots.get(ev.from);
      const to = this.bots.get(ev.to);
      if (!from || !to || !this.eventFree(from)) return;
      this.carryTo(from, to, cfg.deliver.activity, ev.item ?? cfg.deliver.carry);
    }
  }

  /** A robot carries `item` to `to`'s desk (or to where `to` stands when it has no desk). */
  private carryTo(c: BotIn, to: BotIn, actId: string, item: string): void {
    const s = to.desk ? this.spotByKey.get(to.desk) : undefined;
    const goal: Goal = s ? spotGoal(s) : hereGoal(to);
    const cm = this.mem(c.cid);
    this.release(c, cm);
    const loc: Loc = { kind: 'point', goal, d: this.distance(c.at, goal) };
    this.commit(c, cm, this.instantiate(c, this.rngFor(c.cid, cm), 'event', actId, loc, { carry: item }));
    this.assigned.add(c.cid);
  }
}

// --- Scenario fixtures: 100 seeded scenarios, hashed. Both apps generate the same inputs from the seed.

export const SCENARIO_COUNT = 100;
export const SCENARIO_TICKS = 40;
export const SCENARIO_DT = 0.5;

const SCENARIO_KINDS = [
  'commons', 'dock', 'trace', 'anywhere', 'prop:drafting-table', 'prop:lab-bench', 'prop:whiteboard', 'prop:counter',
  'prop:podium', 'prop:crate-stack', 'prop:plant-bed', 'prop:book-cart', 'prop:microscope-desk', 'component:cpu',
  'component:gpu', 'component:memory', 'component:network', 'component:bmc', 'component:io',
];
const SCENARIO_ROLES = [
  'orchestrator', 'mayor', 'architect', 'builder', 'qa', 'reviewer', 'release', 'edge-operator', 'incident',
  'security-reviewer', 'librarian', 'researcher', 'planner', 'finance', 'auditor', 'breeder', 'other',
];
const SCENARIO_TOOLS = [
  'plaza_post_task', 'bash', 'memory_write', 'fetch_url', 'plaza_git', 'plaza_verdict', 'plaza_send_message',
  'bash_output', 'plaza_pay', 'read_file', 'plaza_vote', 'plaza_hire', 'share_file', 'plaza_money',
  'plaza_project_critique', 'todo_write', 'unknown_tool',
];
const SCENARIO_QUIRKS: { clip: string; weight: number }[][] = [
  [],
  [{ clip: 'owl.swivel', weight: 3 }],
  [
    { clip: 'pup.wag', weight: 4 },
    { clip: 'pup.beg', weight: 3 },
  ],
];
const SCENARIO_SLOTS = ['cpu', 'gpu', 'memory', 'network', 'bmc', 'io'];

/** Runs scenario `s` (0..SCENARIO_COUNT-1) through a fresh engine; the hash covers every decision of every tick. */
export function runScenario(cfg: BehaviourConfig, s: number): { hash: string; decisions: number } {
  const rng = mulberry32(0x0b5e0000 + s);
  const pick = (n: number): number => Math.floor(rng() * n);
  const spots: SpotIn[] = [];
  const nSpots = 40 + pick(30);
  for (let i = 0; i < nSpots; i++) {
    const kind = SCENARIO_KINDS[pick(SCENARIO_KINDS.length)]!;
    const x = rng() * 24 - 12;
    const z = rng() * 24 - 12;
    const region = pick(3);
    const face = pick(8) * 45;
    spots.push({ key: `k${String(i).padStart(3, '0')}`, kind, region, x, z, face, clip: null });
  }
  const nBots = 3 + pick(6);
  const bots: BotIn[] = [];
  for (let i = 0; i < nBots; i++) {
    const cid = 1 + 3 * i;
    const role = SCENARIO_ROLES[pick(SCENARIO_ROLES.length)]!;
    const personality: Personality = {
      social: pick(10),
      energy: pick(10),
      focus: pick(10),
      curiosity: pick(10),
      play: pick(10),
      tidy: pick(10),
    };
    const r0 = rng();
    const state: BotState = r0 < 0.15 ? 'offline' : r0 < 0.6 ? 'idle' : 'working';
    const lastTool = rng() < 0.2 ? null : SCENARIO_TOOLS[pick(SCENARIO_TOOLS.length)]!;
    const talkingTo = rng() < 0.2 ? 1 + 3 * pick(nBots) : null;
    const x = rng() * 24 - 12;
    const z = rng() * 24 - 12;
    const region = pick(3);
    const desk = `d${cid}`;
    const dx = rng() * 24 - 12;
    const dz = rng() * 24 - 12;
    const dr = pick(3);
    spots.push({ key: desk, kind: 'desk', region: dr, x: dx, z: dz, face: 0, clip: 'type' });
    const quirks = SCENARIO_QUIRKS[pick(SCENARIO_QUIRKS.length)]!;
    bots.push({ cid, role, personality, state, lastTool, talkingTo, at: { x, z, region }, desk, quirks });
  }

  const engine = new Behaviour(cfg);
  let h = FNV_OFFSET;
  let n = 0;
  for (let t = 0; t < SCENARIO_TICKS; t++) {
    const now = t * SCENARIO_DT;
    if (t > 0) {
      for (const b of bots) {
        if (rng() < 0.12) {
          const r = rng();
          b.state = r < 0.15 ? 'offline' : r < 0.6 ? 'idle' : 'working';
          b.lastTool = rng() < 0.2 ? null : SCENARIO_TOOLS[pick(SCENARIO_TOOLS.length)]!;
          b.talkingTo = rng() < 0.15 ? 1 + 3 * pick(nBots) : null;
        }
      }
    }
    const held = s % 5 === 0 && t >= 20;
    const events: EventIn[] = [];
    if (rng() < 0.2) {
      const kind = pick(5);
      if (kind === 0) {
        events.push({ type: 'celebrate', cid: bots[pick(nBots)]!.cid });
      } else if (kind === 1) {
        const conductor = bots[pick(nBots)]!.cid;
        const members: number[] = [];
        for (const b of bots) if (rng() < 0.5) members.push(b.cid);
        events.push({ type: 'standup', conductor, members });
      } else if (kind === 2) {
        events.push({ type: 'respond', slot: SCENARIO_SLOTS[pick(SCENARIO_SLOTS.length)]! });
      } else if (kind === 3) {
        events.push({ type: 'payment', to: bots[pick(nBots)]!.cid });
      } else {
        const from = bots[pick(nBots)]!.cid;
        const to = bots[pick(nBots)]!.cid;
        events.push({ type: 'deliver', from, to, item: rng() < 0.5 ? 'crate' : 'coin-bag' });
      }
    }
    const q = (x: number): number => Math.floor(x * 1000 + 0.5);
    for (const d of engine.step({ now, robots: bots, spots, held, events })) {
      const goal = d.goal ? `${q(d.goal.x)},${q(d.goal.z)},${d.goal.region},${d.goal.face === null ? '-' : q(d.goal.face)}` : '-';
      const line = [d.cid, d.activity, d.clip, d.tool ?? '-', d.carry ?? '-', d.spot ?? '-', goal, d.partner ?? '-', q(d.until), d.changed ? 1 : 0].join('|');
      h = fnvAppend(h, `${line}\n`);
      n++;
    }
  }
  return { hash: hex8(h), decisions: n };
}
