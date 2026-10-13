// The long navigation checks (docs/design/navigation.md, Verification): 200 random (start, goal) pairs for each board and
// room layout (property), and 120 simulated seconds of 20 robots on each board (simulation). Prints one JSON line per
// board and exits 1 when a criterion fails. Not part of the suite: test/tests.ts runs the short variants. Run `npm run nav:sim`.

import { build } from 'esbuild';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const web = dirname(dirname(fileURLToPath(import.meta.url)));
const floor = join(web, 'src/views/floor3d');
const assets = join(web, '../../assets');

const BOARD_NAMES = ['workstation', 'laptop', 'edge', 'phone', 'server', 'apple-soc'];
const LAYOUT_COUNTS = [1, 4, 8];
const LAYOUT_OFFSETS = [0, 5, 10];
const PAIRS = 200;
const ROBOTS = 20;
const SECONDS = 120;
const DT = 1 / 30;
/** Invariant 2 as the sim reads it: a stuck event opens when non-legit time without progress exceeds EVENT_S; no robot
 *  may be stuck for STUCK_MAX_S seconds. */
const EVENT_S = 1;
const STUCK_MAX_S = 3;
/** Progress: the distance to the current waypoint shrinks by PROGRESS_M. A turn in place is legitimate for TURN_WAIT_S s. */
const PROGRESS_M = 0.05;
const TURN_WAIT_S = 1.2;

/** The navigation modules, the room generator, the roles and the six boards, bundled in memory (nothing is written). */
async function loadNav() {
  const entry = [
    "import * as nav from './nav.ts';",
    "import * as scene from './nav-scene.ts';",
    "import * as layout from './board-layout.ts';",
    "import * as gen from './rooms/gen.ts';",
    `import roles from ${JSON.stringify(join(assets, 'rooms/roles.json'))};`,
    ...BOARD_NAMES.map((name, i) => `import board${i} from ${JSON.stringify(join(assets, 'hardware/board', `${name}.json`))};`),
    `export const boards = { ${BOARD_NAMES.map((name, i) => `${JSON.stringify(name)}: board${i}`).join(', ')} };`,
    'export { nav, scene, layout, gen, roles };',
  ].join('\n');
  const result = await build({
    stdin: { contents: entry, resolveDir: floor, loader: 'ts', sourcefile: 'nav-entry.ts' },
    bundle: true,
    platform: 'node',
    format: 'esm',
    write: false,
    logLevel: 'silent',
    loader: { '.css': 'empty' },
  });
  return import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
}

const roleIds = (roles) => Object.keys(roles.roles);

function agentsFor(count, offset, ids) {
  return Array.from({ length: count }, (_, i) => ({ cid: i + 1, name: '', role: ids[(offset + i) % ids.length], tier: 'worker' }));
}

/** The navigation scene of a board with `count` agents in its rooms (the same inputs as test/tests.ts). */
function sceneFor(api, board, count, offset) {
  const model = api.boards[board];
  const agents = agentsFor(count, offset, roleIds(api.roles));
  return api.scene.navScene(model, {}, api.layout.layoutOnBoard(agents, model, {}), null);
}

/** Walkable world points: the spots and the centres of the portal cells on the a side. */
function walkable(api, grid) {
  const pts = grid.spots.map((s) => ({ x: s.x, z: s.z }));
  for (const p of grid.portals) {
    const c = p.a;
    pts.push({ x: grid.x0 + ((c % grid.cols) + 0.5) * grid.cell, z: grid.z0 + (Math.floor(c / grid.cols) + 0.5) * grid.cell });
  }
  return pts.filter((p) => grid.walk[api.nav.cellAt(grid, p.x, p.z)] === 1);
}

/**
 * Random pairs of walkable points in one connected component: the planner returns a path (null is counted) and no path
 * cell is blocked or inflated.
 */
function propertyPairs(api, grid, seed, pairs) {
  const pts = walkable(api, grid);
  if (pts.length === 0) return { nulls: pairs, bad: 0 };
  // The grid's own components (rule 12 with the height rule, nav-grid.ts walkComponents): a pair in two of them has no route.
  const comp = grid.component;
  const compOf = (p) => comp[api.nav.cellAt(grid, p.x, p.z)];
  const rng = api.gen.mulberry32(seed);
  let nulls = 0;
  let bad = 0;
  let made = 0;
  for (let tries = 0; made < pairs && tries < pairs * 100; tries++) {
    const a = pts[Math.floor(rng() * pts.length)];
    const b = pts[Math.floor(rng() * pts.length)];
    if (compOf(a) !== compOf(b)) continue;
    made++;
    const path = api.nav.findPath(grid, a, b);
    if (!path) {
      nulls++;
      continue;
    }
    const hit = path.points.some((p) => {
      const c = api.nav.cellAt(grid, p.x, p.z);
      return c < 0 || grid.blocked[c] === 1 || grid.inflated[c] === 1;
    });
    if (hit) bad++;
  }
  return { nulls: nulls + (pairs - made), bad };
}

/** The heading error of a robot toward a point, in radians (0 to pi). */
function headingError(p, target) {
  const want = Math.atan2(target.x - p.x, target.z - p.z);
  return Math.abs(Math.atan2(Math.sin(want - p.heading), Math.cos(want - p.heading)));
}

/** True while a robot heads for a jump or drop point (the next point of its path is flagged edge). */
function inEdge(world, id) {
  const head = world.path(id)?.[0];
  return head !== undefined && (head.edge === 'jump' || head.edge === 'drop');
}

/**
 * Robots on random walkable cells, each given a reachable random goal; a robot idle for 2 s takes a new one.
 * The sim's own reading of invariant 2 (independent of the engine's timer):
 *  - progress: the distance to the current waypoint shrinks by PROGRESS_M below its reference; a new waypoint (crossed or
 *    the reference without counting as progress; the arrival and a finished path close the event;
 *  - legitimate waits pause the clock: on a lift or waiting at a lift front, yielding, turning in place (up to
 *    TURN_WAIT_S s), and a finished path;
 *  - a stuck event opens when the non-legit time without progress exceeds EVENT_S; its duration is that time minus
 *    EVENT_S when progress, the arrival or the unreachable handler closes it.
 * Counts the robot-steps whose centre is in a blocked or inflated cell, or whose height differs from the ground of its cell
 * by more than NAV_STEP + 0.05 while it is not on a lift and not on a jump or drop (its path's next point is flagged edge),
 * and the pair-seconds of robots closer than 0.5.
 */
function simulate(api, grid, seed, robots, seconds) {
  const { NavWorld, cellAt, findPath } = api.nav;
  const rng = api.gen.mulberry32(seed);
  const cands = walkable(api, grid);
  const world = new NavWorld(grid);
  const rs = [];
  const used = new Set();
  for (let tries = 0; rs.length < robots && tries < 100000; tries++) {
    const p = cands[Math.floor(rng() * cands.length)];
    const c = cellAt(grid, p.x, p.z);
    if (used.has(c)) continue;
    used.add(c);
    const id = rs.length + 1;
    world.add({ id, x: p.x, z: p.z, speed: 0.8, turnRate: 2.5, priority: 1 });
    rs.push({ id, goal: null, active: false, idle: 0, nop: 0, open: false, ref: Infinity, wp: '', turnT: 0 });
  }
  const pickGoal = (from) => {
    for (let t = 0; t < 30; t++) {
      const q = cands[Math.floor(rng() * cands.length)];
      if (findPath(grid, from, q)) return q;
    }
    return null;
  };
  let robotSteps = 0;
  let bad = 0;
  let pairSeconds = 0;
  let events = 0;
  let resolvedMax = 0;
  let goalsIssued = 0;
  let goalsReached = 0;
  let unreachable = 0;
  const close = (r) => {
    if (r.open) resolvedMax = Math.max(resolvedMax, r.nop - EVENT_S);
    r.open = false;
    r.nop = 0;
  };
  world.onUnreachable((id) => {
    const r = rs[id - 1];
    if (!r) return;
    close(r);
    r.active = false;
    r.idle = 0;
    unreachable++;
  });
  const steps = Math.round(seconds / DT);
  for (let s = 0; s < steps; s++) {
    for (const r of rs) {
      const p = world.pose(r.id);
      robotSteps++;
      const c = cellAt(grid, p.x, p.z);
      if (c < 0 || grid.blocked[c] === 1 || grid.inflated[c] === 1) bad++;
      else if (!p.onLift && Math.abs(p.y - grid.y[c]) > api.nav.NAV_STEP + 0.05 && !inEdge(world, r.id)) bad++;
      if (r.active) {
        const path = world.path(r.id);
        const target = path && path.length ? path[0] : null;
        const pathDone = path !== null && path.length === 0;
        if (!p.moving && Math.hypot(p.x - r.goal.x, p.z - r.goal.z) < 0.35) {
          r.active = false;
          r.idle = 0;
          goalsReached++;
          close(r);
          continue;
        }
        const turning = target !== null && !p.moving && headingError(p, target) > Math.PI / 6;
        r.turnT = turning ? r.turnT + DT : 0;
        const legit = pathDone || p.onLift || p.yielding || p.waiting || (turning && r.turnT <= TURN_WAIT_S);
        let progressed = pathDone;
        if (target) {
          const dist = Math.hypot(target.x - p.x, target.z - p.z);
          const key = `${target.x},${target.z}`;
          // A new waypoint (crossed, or a replan) re-bases the reference without progress.
          if (key !== r.wp) {
            r.wp = key;
            r.ref = dist;
          } else if (dist <= r.ref - PROGRESS_M) {
            r.ref = dist;
            progressed = true;
          }
        }
        if (progressed) close(r);
        else if (!legit) {
          r.nop += DT;
          if (!r.open && r.nop > EVENT_S) {
            r.open = true;
            events++;
          }
        }
      } else {
        r.idle += DT;
        if (r.idle >= 2) {
          r.idle = 0;
          const q = pickGoal(p);
          if (q) {
            world.setGoal(r.id, { x: q.x, z: q.z });
            r.goal = q;
            r.active = true;
            r.ref = Infinity;
            r.wp = '';
            r.turnT = 0;
            goalsIssued++;
          }
        }
      }
    }
    world.step(DT);
    const pos = rs.map((r) => world.pose(r.id));
    for (let i = 0; i < pos.length; i++) {
      for (let j = i + 1; j < pos.length; j++) {
        if (Math.hypot(pos[i].x - pos[j].x, pos[i].z - pos[j].z) < 0.5) pairSeconds += DT;
      }
    }
  }
  let openAtEnd = 0;
  let openMax = 0;
  for (const r of rs) {
    if (!r.open) continue;
    openAtEnd++;
    openMax = Math.max(openMax, r.nop - EVENT_S);
  }
  return { robotSteps, bad, pairSeconds, stuckEvents: events, resolvedMax, openAtEnd, openMax, goalsIssued, goalsReached, unreachable };
}

const api = await loadNav();
let failed = false;
for (const [bi, board] of BOARD_NAMES.entries()) {
  let propOk = true;
  const layouts = [];
  for (let l = 0; l < LAYOUT_COUNTS.length; l++) {
    const s = sceneFor(api, board, LAYOUT_COUNTS[l], LAYOUT_OFFSETS[l]);
    const r = propertyPairs(api, s.grid, 1000 + bi * 3 + l, PAIRS);
    layouts.push({ agents: LAYOUT_COUNTS[l], pairs: PAIRS, nulls: r.nulls, badPaths: r.bad });
    if (r.nulls > 0 || r.bad > 0) propOk = false;
  }
  const s = sceneFor(api, board, LAYOUT_COUNTS[2], LAYOUT_OFFSETS[2]);
  const sim = simulate(api, s.grid, 2000 + bi, ROBOTS, SECONDS);
  const overlapPct = (100 * sim.pairSeconds) / (ROBOTS * SECONDS);
  const ok = propOk && sim.bad === 0 && overlapPct < 0.5 && sim.resolvedMax < STUCK_MAX_S && sim.openMax < STUCK_MAX_S;
  if (!ok) failed = true;
  console.log(JSON.stringify({
    board,
    layouts,
    robotSteps: sim.robotSteps,
    badSteps: sim.bad,
    stuckEvents: sim.stuckEvents,
    resolvedMaxSeconds: Number(sim.resolvedMax.toFixed(2)),
    openAtEnd: sim.openAtEnd,
    openMaxSeconds: Number(sim.openMax.toFixed(2)),
    overlapPct: Number(overlapPct.toFixed(3)),
    goalsReached: sim.goalsReached,
    goalsIssued: sim.goalsIssued,
    unreachable: sim.unreachable,
    ok,
  }));
}
process.exit(failed ? 1 : 0);
