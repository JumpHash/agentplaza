// Writes assets/rooms/fixtures.json: 200 golden cases that the web and the desktop must both reproduce (the layout
// hash of each). Deterministic: case k uses role roles[k % 17], w and d from a fixed size list, door
// doors[(k >> 2) & 3], and the seed is the next draw of mulberry32(12345). Run `npm run rooms:fixtures` only when
// the algorithm or the catalogue changes on purpose. The file is written, not committed.

import { writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { cap, loadRooms } from './rooms-load.mjs';

const { generate, layoutHash, mulberry32, computeMinSize, catalog, roles } = await loadRooms();

const web = dirname(dirname(fileURLToPath(import.meta.url)));
const out = join(web, '../../assets/rooms/fixtures.json');
const minOut = join(web, '../../assets/rooms/minsize.json');
const ROLE_IDS = Object.keys(roles.roles);
const SIZES = [2.5, 3, 3.5, 4, 4.5, 5, 6, 7, 8];
const DOORS = ['n', 'e', 's', 'w'];
const COUNT = 200;

// minsize.json: role -> the smallest side for 1..6 desks, computed fresh by generation (computeMinSize, not the
// table). Sorted by role; Infinity (no side up to 8 qualifies) is written as null, the table's "none".
const minsize = {};
// 'other' is the family of a role with no entry (the board's role-less agents): it has a row too.
for (const role of [...ROLE_IDS, 'other'].sort()) {
  minsize[role] = [1, 2, 3, 4, 5, 6].map((agents) => computeMinSize(role, agents, catalog, roles));
}
writeFileSync(minOut, JSON.stringify(minsize, null, 1) + '\n');
console.log(`rooms:fixtures: ${ROLE_IDS.length} roles x 6 desk counts written to ${minOut}`);

const rng = mulberry32(12345);
const cases = [];
for (let k = 0; k < COUNT; k++) {
  const role = ROLE_IDS[k % ROLE_IDS.length];
  const w = SIZES[(k * 5) % SIZES.length];
  const d = SIZES[(k * 7 + 3) % SIZES.length];
  const door = DOORS[(k >> 2) & 3];
  const seed = Math.floor(rng() * 4294967296);
  const agents = cap(w, d);
  const layout = generate({ role, w, d, door, seed, agents }, catalog, roles);
  cases.push({
    role,
    w,
    d,
    door,
    seed,
    agents,
    hash: layoutHash(layout),
    skipped: layout.skipped.length,
    dropped: layout.dropped.length,
  });
}

writeFileSync(out, JSON.stringify({ schema: 1, cases }, null, 1) + '\n');
const dropped = cases.filter((c) => c.dropped > 0).length;
console.log(`rooms:fixtures: ${cases.length} cases written to ${out} (${dropped} with a dropped item)`);
