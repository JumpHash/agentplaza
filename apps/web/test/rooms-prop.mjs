// The property test of the room generator (floor3d/rooms). Every role, at every size from its minSize up to 8
// (the squares 2.5 to 8, and the w != d pairs below), with all four doors and --seeds seeds (default 500):
// zero violations and zero dropped items. The desks are cap(w, d). A size below minSize(role, cap) is not checked.
// Prints per role and size: cases, violations, dropped, skipped (preferred stations that did not fit), and the lab
// URL of the first failure. Exits 1 on any failure. A full run is slow (about an hour at 500 seeds); a quick one:
//   npm run rooms:prop -- --seeds=20 --role=architect

import { cap, labUrl, loadRooms } from './rooms-load.mjs';

const args = Object.fromEntries(process.argv.slice(2).map((a) => a.replace(/^--/, '').split('=')));
const SEEDS = Number(args.seeds ?? 500);
const ONLY = args.role;
const DOORS = ['n', 'e', 's', 'w'];
const SQUARES = Array.from({ length: 12 }, (_, i) => 2.5 + i * 0.5).map((s) => [s, s]);
const PAIRS = [
  [2.5, 3.5],
  [3, 4],
  [4, 3.5],
  [4.5, 6],
  [5, 4],
  [6, 4.5],
];
const SIZES = [...SQUARES, ...PAIRS];

const { generate, verify, minSize, catalog, roles } = await loadRooms();
if (ONLY !== undefined && !roles.roles[ONLY]) {
  console.error(`rooms:prop: no role "${ONLY}"`);
  process.exit(2);
}
const roleIds = ONLY !== undefined ? [ONLY] : Object.keys(roles.roles);

const minimums = new Map();
function minimum(role, agents) {
  const key = `${role}/${agents}`;
  if (!minimums.has(key)) minimums.set(key, minSize(role, agents, catalog, roles));
  return minimums.get(key);
}

const started = Date.now();
let failures = 0;
let firstFailure = null;
console.log(`rooms:prop: ${roleIds.length} role(s), ${SIZES.length} sizes, ${SEEDS} seed(s), 4 doors`);
for (const role of roleIds) {
  for (const [w, d] of SIZES) {
    const agents = cap(w, d);
    const min = minimum(role, agents);
    if (Math.min(w, d) < min) {
      console.log(`${role} ${w}x${d}: below minSize ${min} (not checked)`);
      continue;
    }
    let cases = 0;
    let violations = 0;
    let dropped = 0;
    let skipped = 0;
    let first = null;
    for (let seed = 1; seed <= SEEDS; seed++) {
      for (const door of DOORS) {
        const input = { role, w, d, door, seed, agents };
        const layout = generate(input, catalog, roles);
        const found = verify(layout, input, catalog, roles);
        cases++;
        violations += found.length;
        dropped += layout.dropped.length;
        skipped += layout.skipped.length;
        if ((found.length > 0 || layout.dropped.length > 0) && first === null) {
          first = { url: labUrl(input), what: found[0] ?? `dropped ${layout.dropped.join(', ')}` };
        }
      }
    }
    const bad = violations > 0 || dropped > 0;
    if (bad) failures++;
    if (bad && firstFailure === null) firstFailure = first;
    const tail = first !== null && bad ? `  FIRST ${first.url}  ${first.what}` : '';
    console.log(`${role} ${w}x${d}: cases ${cases}, violations ${violations}, dropped ${dropped}, skipped ${skipped}${tail}`);
  }
}

const seconds = ((Date.now() - started) / 1000).toFixed(1);
if (failures > 0) {
  console.error(`rooms:prop: FAILED in ${failures} role-size group(s) after ${seconds} s. First failure: ${firstFailure.url} (${firstFailure.what})`);
  process.exit(1);
}
console.log(`rooms:prop: ok after ${seconds} s`);
