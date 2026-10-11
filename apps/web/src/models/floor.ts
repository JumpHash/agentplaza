/* The floor's pure derivations — the station layout, the state each robot
   wears, the walk a conversation (or a hiring) causes, and the floor read as
   one sentence. No canvas, no DOM: `views/floor.ts` draws exactly what these
   return, and the harness tests them with no daemon and no browser.

   The layout rule (deterministic — the front end derives every position, no
   server geometry):

     1. The roles on the floor are the distinct roles of the agents present
        (the register's role VMs joined to the hub's connected agents, by
        guest CID), sorted ascending by code unit.
     2. Station r of n stands in a surveyor's grid of columns = ceil(sqrt(n))
        blocks, at column r mod columns and row floor(r / columns), at a fixed
        pitch of 240 x (block height + 40) floor units.
     3. Inside its station, a role's agents stand in a sub-grid of three per
        row at a 48 x 54 pitch, ascending by guest CID (an agent the register
        gives no CID ranks last), centred in the block. Every block is as tall
        as the tallest role's rows need, so no agent overflows its plate.

   A robot's home is therefore a pure function of (its role's index among the
   roles, its CID's rank among that role's agents). Nothing depends on the
   canvas' width: the view scales the whole plan to fit, so resizing the
   window never moves a robot and never fakes a walk.

   The state precedence is fixed and stated once: unreachable (the hub, which
   can be read, does not list the agent) outranks talking (an active
   conversation) outranks working (serial lines past the quiet threshold)
   outranks idle. */

import type { BoardPost, ConversationsView } from './social';
import { activePairs, pairKey } from './social';
import type { Task } from './task';

/** The four states a robot wears at a glance. */
export type RobotState = 'working' | 'talking' | 'idle' | 'unreachable';

/** The state order the floor reads states in, for labels and legends. */
export const ROBOT_STATES: readonly RobotState[] = ['talking', 'working', 'idle', 'unreachable'];

/** The state each robot's ring is drawn for, as the register writes it. */
export const ROBOT_STATE_LABEL: Readonly<Record<RobotState, string>> = {
  working: 'working',
  talking: 'talking',
  idle: 'idle',
  unreachable: 'unreachable',
};

/** Serial lines per second past which an agent is working, not idle. The same
    quiet threshold the map and the run table read. */
export const WORKING_LPS = 0.02;

/* --------------------------------------------------------------- geometry */
const STATION_W = 208;
const STATION_PITCH_X = 240; // STATION_W + 32 of ruling between blocks
const STATION_PITCH_GAP_Y = 40;
const PLATE_H = 24; // the role's plate band across the block's top
const AGENT_PITCH_X = 48;
const AGENT_PITCH_Y = 54;
const STATION_PAD_BOTTOM = 16;
const PER_ROW = 3;
/** How far apart a gathered pair stands: adjacent, with one glyph of room. */
export const PAIR_GAP = 34;

/** The floor's plan in its own units: every position below is a floor
    coordinate, never a pixel. */
export interface Station {
  role: string;
  index: number;
  /** The block's top-left corner. */
  x: number;
  y: number;
  width: number;
  height: number;
  /** How many of the role's agents stand in it. */
  agents: number;
}

/** One agent's own facts, as the view collected them from the store. */
export interface FloorSource {
  /** The guest CID — the join key against the hub's connections. Null when
      the register named no vsock CID for it. */
  cid: number | null;
  role: string;
  /** The register's entry, when the register has one for this agent. */
  runId: string | null;
  vmId: number | null;
  lines: number;
  lps: number;
  /** The register's VM state (running/exited/killed), when it has one. */
  vmState: 'running' | 'exited' | 'killed' | null;
}

/** A robot on the plan: its facts, its station, its home, and where it
    stands now (home, or the gathering it walked to). */
export interface Robot {
  /** Stable across renders: the CID when the register gives one, else the
      role and its rank among that role's CID-less rows. Hover, focus,
      selection and the walk all key on it, so a re-laid plan never moves a
      robot's identity. */
  key: string;
  source: FloorSource;
  state: RobotState;
  station: Station;
  /** The rank among its role's agents (ascending CID), and its row and column
      inside the station's sub-grid. */
  slot: number;
  row: number;
  col: number;
  home: { x: number; y: number };
  /** Where the robot stands now — its home unless a gathering moved it. */
  x: number;
  y: number;
  /** The gathering it walked to, or null. */
  gathered: GatherKind | null;
}

/** A conversation the plaza is holding, as the floor draws it. */
export interface Gathering {
  kind: GatherKind;
  /** The CIDs gathered, ascending. */
  cids: number[];
  /** The meeting point the members walked to (their centroid). */
  x: number;
  y: number;
}

export type GatherKind = 'conversation' | 'hire';

/** One link that draws two agents together. */
export interface GatherLink {
  a: number;
  b: number;
  kind: GatherKind;
  at: number;
}

/** A hiring the board records: the award's own note names the task, the
    ledger's copy of that task names the contractor, and the note's author is
    the agent that awarded it. `subject: "hiring for <task>"` is the agora's
    template (docs/agents/social.md), so this derives from the board and the
    ledger alone — no endpoint is asked for it twice. */
export interface HireLink {
  task: string;
  author_cid: number;
  contractor_cid: number;
  at: number;
}

const HIRING_SUBJECT = /^hiring[ \t]+for[ \t]+(\S+)[ \t]*$/;

/** The hirings the board records. A post names a hiring when it carries the
    agora's subject template (or a task link and that template's words); the
    ledger's task then names the contractor. A pair of one — the note's author
    is the contractor — is not a pair, and is dropped rather than drawn as a
    robot walking to itself. */
export function hireLinks(posts: readonly BoardPost[], tasks: readonly Task[]): HireLink[] {
  const byId = new Map<string, Task>();
  for (const t of tasks) byId.set(t.id, t);
  const seen = new Set<string>();
  const links: HireLink[] = [];
  for (const post of posts) {
    const match = HIRING_SUBJECT.exec(post.subject);
    const taskId = post.task ?? (match ? match[1] : null);
    if (taskId == null) continue;
    const task = byId.get(taskId);
    if (!task || task.agent_cid == null) continue;
    if (task.agent_cid === post.agent_cid) continue;
    const key = `${pairKey(post.agent_cid, task.agent_cid)}\u0000${taskId}`;
    if (seen.has(key)) continue;
    seen.add(key);
    links.push({
      task: taskId,
      author_cid: post.agent_cid,
      contractor_cid: task.agent_cid,
      at: post.created_at,
    });
  }
  return links.sort((x, y) => x.at - y.at || x.author_cid - y.author_cid);
}

/** Fold the links the floor draws into one link per pair: a conversation
    outranks a hiring (the pair is talking, not merely hired), and the newest
    wins within a kind. Ascending by pair, so the plan is deterministic. */
export function gatherLinks(pairs: readonly { a: number; b: number; last_at: number }[], hires: readonly HireLink[]): GatherLink[] {
  const byPair = new Map<string, GatherLink>();
  const offer = (a: number, b: number, kind: GatherKind, at: number): void => {
    const lo = Math.min(a, b);
    const hi = Math.max(a, b);
    const key = pairKey(lo, hi);
    const held = byPair.get(key);
    if (!held) {
      byPair.set(key, { a: lo, b: hi, kind, at });
      return;
    }
    if (held.kind === 'conversation' && kind === 'hire') return;
    if (held.kind === 'hire' && kind === 'conversation') {
      byPair.set(key, { a: lo, b: hi, kind, at });
      return;
    }
    if (at > held.at) byPair.set(key, { a: lo, b: hi, kind, at });
  };
  for (const h of hires) offer(h.author_cid, h.contractor_cid, 'hire', h.at);
  for (const p of pairs) offer(p.a, p.b, 'conversation', p.last_at);
  return [...byPair.values()].sort((x, y) => x.a - y.a || x.b - y.b);
}

/** The meeting point and the standing order for each group of agents linked
    (directly or transitively) by the links: members stand in a row, ascending
    by CID, centred on the group's own centroid. A pair therefore stands
    adjacent at the midpoint of the two homes. */
export function gatherPlacements(
  links: readonly GatherLink[],
  homes: ReadonlyMap<number, { x: number; y: number }>,
): { targets: Map<number, { x: number; y: number; kind: GatherKind }>; gatherings: Gathering[] } {
  const parent = new Map<number, number>();
  const find = (cid: number): number => {
    let root = cid;
    while (parent.get(root) !== root) root = parent.get(root) ?? root;
    let walk = cid;
    while (parent.get(walk) !== walk) {
      const next = parent.get(walk) ?? walk;
      parent.set(walk, root);
      walk = next;
    }
    return root;
  };
  for (const cid of homes.keys()) parent.set(cid, cid);
  const members = new Map<number, number[]>(); // root -> cids
  const kinds = new Map<number, GatherKind>();
  for (const link of links) {
    if (!homes.has(link.a) || !homes.has(link.b)) continue;
    const ra = find(link.a);
    const rb = find(link.b);
    if (ra !== rb) parent.set(ra, rb);
  }
  for (const cid of homes.keys()) {
    const root = find(cid);
    const list = members.get(root);
    if (list) list.push(cid);
    else members.set(root, [cid]);
  }
  for (const link of links) {
    if (!homes.has(link.a) || !homes.has(link.b)) continue;
    const root = find(link.a);
    const held = kinds.get(root);
    if (held == null || link.kind === 'conversation') kinds.set(root, held === 'conversation' ? held : link.kind);
  }

  const targets = new Map<number, { x: number; y: number; kind: GatherKind }>();
  const gatherings: Gathering[] = [];
  for (const [root, list] of members) {
    if (list.length < 2) continue;
    const kind = kinds.get(root) ?? 'conversation';
    const cids = list.slice().sort((a, b) => a - b);
    let sx = 0;
    let sy = 0;
    for (const cid of cids) {
      const home = homes.get(cid);
      sx += home?.x ?? 0;
      sy += home?.y ?? 0;
    }
    const cx = sx / cids.length;
    const cy = sy / cids.length;
    cids.forEach((cid, i) => {
      targets.set(cid, { x: cx + (i - (cids.length - 1) / 2) * PAIR_GAP, y: cy, kind });
    });
    gatherings.push({ kind, cids, x: cx, y: cy });
  }
  const byCid = (a: Gathering, b: Gathering): number => (a.cids[0] ?? 0) - (b.cids[0] ?? 0);
  return { targets, gatherings: gatherings.sort(byCid) };
}

/** The floor read as a plan: its stations, its robots, its gatherings and the
    plan's own extent in floor units. */
export interface FloorRead {
  stations: Station[];
  robots: Robot[];
  gatherings: Gathering[];
  width: number;
  height: number;
  /** How many robots wear each state. */
  counts: Record<RobotState, number>;
  /** Whether the hub could be read: without it no robot reads unreachable. */
  hubReadable: boolean;
}

/** The two words a role's plate carries: the initials of its first two words,
    else its first two letters. Cosmetic — the plate and the card name the
    role in full — but it tells twenty-one stations apart at a glance. */
export function roleMonogram(role: string): string {
  const words = role.split(/[^A-Za-z0-9]+/).filter((w) => w !== '');
  const [first, second] = words;
  if (first == null) return '??';
  if (second == null) return first.slice(0, 2).toUpperCase();
  return (first.charAt(0) + second.charAt(0)).toUpperCase();
}

/** Read the floor: assign states (precedence above), lay the stations out by
    role index, place each robot at its home, then walk the gathered ones to
    their meeting point. */
export function readFloor(
  sources: readonly FloorSource[],
  connected: ReadonlySet<number> | null,
  social: Pick<ConversationsView, 'conversations' | 'pairs' | 'active_secs'> | null,
  hires: readonly HireLink[],
  now: number,
): FloorRead {
  const pairList = activePairs(social ?? {}, now);
  const talking = new Set<number>();
  for (const p of pairList) {
    talking.add(p.a);
    talking.add(p.b);
  }

  const robots: Robot[] = sources.map((source) => {
    const unreachable =
      connected != null && (source.cid == null || !connected.has(source.cid));
    const talkingNow = source.cid != null && talking.has(source.cid);
    const state: RobotState = unreachable
      ? 'unreachable'
      : talkingNow
        ? 'talking'
        : source.lps > WORKING_LPS
          ? 'working'
          : 'idle';
    return {
      source,
      key: source.cid != null ? `cid:${source.cid}` : `role:${source.role}`,
      state,
      station: { role: source.role, index: 0, x: 0, y: 0, width: STATION_W, height: 0, agents: 0 },
      slot: 0,
      row: 0,
      col: 0,
      home: { x: 0, y: 0 },
      x: 0,
      y: 0,
      gathered: null,
    };
  });

  /* the roles present, ascending by code unit — the layout's only ordering */
  const roles = [...new Set(robots.map((r) => r.source.role))].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
  const columns = Math.max(1, Math.ceil(Math.sqrt(roles.length)));

  /* each role's agents, ascending by CID (a row with none ranks last) */
  const byRole = new Map<string, Robot[]>();
  for (const role of roles) byRole.set(role, []);
  for (const robot of robots) byRole.get(robot.source.role)?.push(robot);
  let maxRows = 1;
  for (const list of byRole.values()) {
    list.sort((a, b) => {
      const ac = a.source.cid ?? Number.MAX_SAFE_INTEGER;
      const bc = b.source.cid ?? Number.MAX_SAFE_INTEGER;
      return ac - bc;
    });
    /* a row the register gives no CID keys by its rank among its role's own
       CID-less rows — stable across renders, and never colliding with a CID */
    let rank = 0;
    for (const robot of list) {
      if (robot.source.cid == null) robot.key = `role:${robot.source.role}#${rank++}`;
    }
    maxRows = Math.max(maxRows, Math.ceil(list.length / PER_ROW));
  }
  const stationH = PLATE_H + maxRows * AGENT_PITCH_Y + STATION_PAD_BOTTOM;
  const stationPitchY = stationH + STATION_PITCH_GAP_Y;

  const stations: Station[] = roles.map((role, index) => {
    const col = index % columns;
    const row = Math.floor(index / columns);
    return {
      role,
      index,
      x: col * STATION_PITCH_X,
      y: row * stationPitchY,
      width: STATION_W,
      height: stationH,
      agents: byRole.get(role)?.length ?? 0,
    };
  });

  for (const list of byRole.values()) {
    const station = stations.find((s) => s.role === list[0]?.source.role);
    if (!station) continue;
    const rows = Math.max(1, Math.ceil(list.length / PER_ROW));
    list.forEach((robot, slot) => {
      const cols = Math.min(PER_ROW, list.length - Math.floor(slot / PER_ROW) * PER_ROW);
      const col = slot % PER_ROW;
      const row = Math.floor(slot / PER_ROW);
      robot.station = station;
      robot.slot = slot;
      robot.row = row;
      robot.col = col;
      const x = station.x + STATION_W / 2 + (col - (cols - 1) / 2) * AGENT_PITCH_X;
      /* the sub-grid is vertically centred in the block's own body, so a
         role with one row of agents and a role with three stand alike */
      const y =
        station.y + PLATE_H + (stationH - PLATE_H - STATION_PAD_BOTTOM) / 2 +
        (row - (rows - 1) / 2) * AGENT_PITCH_Y;
      robot.home = { x, y };
      robot.x = x;
      robot.y = y;
    });
  }

  const homes = new Map<number, { x: number; y: number }>();
  for (const robot of robots) if (robot.source.cid != null) homes.set(robot.source.cid, robot.home);
  const links = gatherLinks(pairList, hires);
  const { targets, gatherings } = gatherPlacements(links, homes);
  for (const robot of robots) {
    const target = robot.source.cid == null ? undefined : targets.get(robot.source.cid);
    if (!target) continue;
    robot.x = target.x;
    robot.y = target.y;
    robot.gathered = target.kind;
  }

  const width = columns > 0 ? columns * STATION_PITCH_X - (STATION_PITCH_X - STATION_W) : STATION_W;
  const rows = Math.max(1, Math.ceil(roles.length / columns));
  const height = rows * stationPitchY - STATION_PITCH_GAP_Y;

  const counts: Record<RobotState, number> = { talking: 0, working: 0, idle: 0, unreachable: 0 };
  for (const robot of robots) counts[robot.state]++;

  return { stations, robots, gatherings, width, height, counts, hubReadable: connected != null };
}

/** The newest post one agent wrote, or null. The board is newest first, so
    the first match is the last thing it said. */
export function lastPostBy(posts: readonly BoardPost[], cid: number | null): BoardPost | null {
  if (cid == null) return null;
  return posts.find((p) => p.agent_cid === cid) ?? null;
}

/** The agents a CID is holding an active conversation with, ascending. */
export function peersOf(pairs: readonly { a: number; b: number }[], cid: number | null): number[] {
  if (cid == null) return [];
  const peers = new Set<number>();
  for (const p of pairs) {
    if (p.a === cid) peers.add(p.b);
    else if (p.b === cid) peers.add(p.a);
  }
  return [...peers].sort((a, b) => a - b);
}

/* -------------------------------------------------------------- the camera
   The floor is watched the way a real-time strategy game is: a fixed camera,
   never an orbit. The tilt (from the horizon) and the yaw are constants, the
   camera is ORTHOGRAPHIC so the plaza's parallel rulings stay parallel on
   screen — a surveyor looks down at a plan, not through a lens — and the
   frustum is sized to hold the whole plan at zoom 1. The owner pans by
   dragging and zooms with the wheel; nothing else moves the camera.
   Everything here is in world units, where the plan's own floor units are
   1:1 and the plan's centre is the origin. */

/** Degrees of tilt above the horizon: the classic RTS rake. */
export const RTS_TILT_DEG = 57;
/** Degrees of yaw: a slight turn, so the plaza reads as a plan in space
    rather than a diagram head-on. */
export const RTS_YAW_DEG = -12;
/** World units of headroom above the ground for plinths, robots, plates. */
export const RTS_ROOM = 46;
/** How far the plan may be panned off centre, as a fraction of its extent:
    the viewer may push a corner closer, never lose the plaza. */
export const RTS_PAN_FRACTION = 0.45;
export const RTS_ZOOM_MIN = 0.6;
export const RTS_ZOOM_MAX = 3.2;

export interface RtsFraming {
  pos: [number, number, number];
  target: [number, number, number];
  /** The frustum's height in world units at this zoom. */
  view_h: number;
}

export function clampZoom(zoom: number): number {
  return Math.max(RTS_ZOOM_MIN, Math.min(RTS_ZOOM_MAX, zoom));
}

/** The orthographic view height that holds the whole plan at zoom 1: the
    plan's depth foreshortens by sin(tilt) under the camera, its width does
    not, and the room above the ground covers the standing robots. */
export function rtsViewH(width: number, height: number, aspect: number): number {
  const tilt = (RTS_TILT_DEG * Math.PI) / 180;
  const needW = Math.max(1, width) * 1.06;
  const needH = Math.max(1, height) * Math.sin(tilt) * 1.06 + RTS_ROOM;
  return Math.max(needH, needW / Math.max(0.2, aspect));
}

/** Where the camera stands and what it aims at, for a plan at a zoom. The
    distance is far enough that the tilted frustum never clips the plan's
    near edge. */
export function rtsFraming(width: number, height: number, aspect: number, zoom: number): RtsFraming {
  const viewH = rtsViewH(width, height, aspect) / clampZoom(zoom);
  const tilt = (RTS_TILT_DEG * Math.PI) / 180;
  const yaw = (RTS_YAW_DEG * Math.PI) / 180;
  const dist = viewH * 1.6 + Math.max(1, height);
  return {
    pos: [Math.sin(yaw) * Math.cos(tilt) * dist, Math.sin(tilt) * dist, Math.cos(yaw) * Math.cos(tilt) * dist],
    target: [0, 0, 0],
    view_h: viewH,
  };
}

/** A drag's own effect on the camera's aim, on the ground: the content
    follows the pointer, so the target moves against it. `worldPerPx` is the
    view height divided by the canvas' height. */
export function rtsPanOffset(dx: number, dy: number, worldPerPx: number): [number, number] {
  const yaw = (RTS_YAW_DEG * Math.PI) / 180;
  const rightX = Math.cos(yaw);
  const rightZ = -Math.sin(yaw);
  const farX = Math.sin(yaw);
  const farZ = Math.cos(yaw);
  return [(-dx * rightX - dy * farX) * worldPerPx, (-dx * rightZ - dy * farZ) * worldPerPx];
}

/** Hold a panned target inside the plan: the viewer may look past an edge,
    never lose the plaza behind them. */
export function clampPan(
  target: { x: number; z: number },
  width: number,
  height: number,
): { x: number; z: number } {
  const maxX = (Math.max(1, width) / 2) * RTS_PAN_FRACTION;
  const maxZ = (Math.max(1, height) / 2) * RTS_PAN_FRACTION;
  return {
    x: Math.max(-maxX, Math.min(maxX, target.x)),
    z: Math.max(-maxZ, Math.min(maxZ, target.z)),
  };
}

/** How many agents the floor names, for the view's own marginalia. */
export function floorCountLine(read: FloorRead): string {
  const total = read.robots.length;
  if (total === 0) return 'no agents on the floor';
  const parts = ROBOT_STATES.filter((s) => read.counts[s] > 0).map((s) => `${read.counts[s]} ${ROBOT_STATE_LABEL[s]}`);
  return `${total} ${total === 1 ? 'agent' : 'agents'} · ${parts.join(' · ')}`;
}

/** The floor's live label: the fleet by state and the conversations being
    held, for the canvas' `aria-label`. Bounded — the first few pairs are
    named and the rest counted — so a plaza-wide chatter does not write an
    essay into the accessibility tree. */
export function floorLabel(read: FloorRead, named: Readonly<Record<number, string>>): string {
  const fleet = floorCountLine(read);
  if (read.gatherings.length === 0) return `Plaza floor. ${fleet}. No conversation is active.`;
  const said = read.gatherings.slice(0, 3).map((g) => {
    const who = g.cids.map((cid) => named[cid] ?? `cid ${cid}`);
    const verb = g.kind === 'hire' ? 'hired by' : 'with';
    return who.length === 2 ? `${who[0]} ${verb} ${who[1]}` : who.join(', ');
  });
  const more = read.gatherings.length - said.length;
  return `Plaza floor. ${fleet}. ${said.join('; ')}${more > 0 ? `; and ${more} more` : ''}.`;
}
