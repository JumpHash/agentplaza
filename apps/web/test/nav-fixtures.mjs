// Writes assets/nav/fixtures.json: 100 golden cases that the web and the desktop must both reproduce (the path hash of
// each). Case i is seed i: board BOARD_NAMES[i % 6], a room layout of LAYOUT_COUNTS[floor(i / 6) % 3] agents whose roles
// start at offset i % roles, and the start drawn from the walkable spots by mulberry32(i). The goal is drawn the same way
// from the walkable spots of the start's component (rule 12, grid.component), so every case has a path. The
// layout follows from the seed, so a case is reproducible from (board, params, seed, start, goal). Run
// `npm run nav:fixtures` only when the grid, the planner or the layout changes on purpose. The file is written, not committed.

import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';

const web = dirname(dirname(fileURLToPath(import.meta.url)));
const floor = join(web, 'src/views/floor3d');
const assets = join(web, '../../assets');

const BOARD_NAMES = ['workstation', 'laptop', 'edge', 'phone', 'server', 'apple-soc'];
const LAYOUT_COUNTS = [1, 4, 8];

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

/** Walkable world points: the spots, and (unless spotsOnly) the centres of the portal cells on the a side. */
function walkable(api, grid, spotsOnly) {
  const pts = grid.spots.map((s) => ({ x: s.x, z: s.z }));
  if (!spotsOnly) {
    for (const p of grid.portals) {
      const c = p.a;
      pts.push({ x: grid.x0 + ((c % grid.cols) + 0.5) * grid.cell, z: grid.z0 + (Math.floor(c / grid.cols) + 0.5) * grid.cell });
    }
  }
  return pts.filter((p) => grid.walk[api.nav.cellAt(grid, p.x, p.z)] === 1);
}


const api = await loadNav();
const out = join(assets, 'nav/fixtures.json');
const cases = [];
for (let seed = 0; seed < 100; seed++) {
  const board = BOARD_NAMES[seed % 6];
  const count = LAYOUT_COUNTS[Math.floor(seed / 6) % 3];
  const s = sceneFor(api, board, count, seed % roleIds(api.roles).length);
  let pts = walkable(api, s.grid, true);
  if (pts.length < 2) pts = walkable(api, s.grid, false);
  const comp = s.grid.component; // rule 12 (nav-grid.ts): two-way links and portals; a drop never joins
  const compOf = (p) => comp[api.nav.cellAt(s.grid, p.x, p.z)];
  const rng = api.gen.mulberry32(seed);
  const i = Math.floor(rng() * pts.length);
  const start = pts[i];
  const peers = pts.filter((p, k) => k !== i && compOf(p) === compOf(start));
  const goal = peers.length > 0 ? peers[Math.floor(rng() * peers.length)] : start;
  const path = api.nav.findPath(s.grid, start, goal);
  cases.push({ board, params: {}, seed, start: [start.x, start.z], goal: [goal.x, goal.z], hash: path ? path.hash : 'none' });
}
mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, JSON.stringify(cases, null, 1) + '\n');
console.log(`nav:fixtures: ${cases.length} cases (${cases.filter((c) => c.hash === 'none').length} without a path) written to ${out}`);
