# Navigation: robots that walk around things

Status: design (2026-10-13). The owner: "the robots walk towards walls, items, props, etc and look clunky walking into
those. We need path mapping/tracing for them." Today a robot walks straight at its goal (or along the trace graph between
rooms) and slides through furniture and walls. This adds a navigation grid, path planning, path following and local
avoidance — the same algorithm in both apps (`floor3d/nav.ts`, `floor3d/world/nav.rs`), proven by shared fixtures.

## The nav grid

One grid over the board in world space, cell = 0.25 units. Each cell: `walk` (bool), `y` (floor height), `cost`,
`region` (0 = the board's PCB, else a room id).

1. **Board**: every cell on the PCB is walkable at the PCB top, unless a hardware part stands on it taller than the step
   height (0.25): those cells are blocked. Part footprints come from the model's flattened world AABBs (the same walk the
   z-fight lint does: `zfight.ts`), so every chip, connector, heatsink and slot blocks correctly; decals, traces and parts
   lower than the step don't. Trace cells cost 0.7 (robots like walking on copper), open PCB 1.0.
2. **Rooms**: each room's own generator grid (rooms.md) placed in world space at its terrace top: solid cells and walls
   blocked, the rest walkable. A room connects to the board only through its **door portal**.
3. **Portals**: a door portal joins the room's door cell to the board cell in front of the terrace below it. If the
   terrace top is ≤ 1.5 above the board, the portal is a **ramp** (cost = length + 2·rise); higher (the GPU's top edge,
   tall heatsinks), it is a **lift**: a small platform with rails rendered at the terrace edge next to the door; the robot
   rides it (cost = 4 + rise; one robot at a time).
4. **Clearance**: robots are discs of radius 0.3. Blocked cells are inflated by one cell for planning; cells within one
   more cell of a blocked cell cost +0.5 (paths keep off walls and furniture when they can).

The grid is rebuilt when the board, its params or any room layout changes (cheap: ~60k cells for a large board).

## Planning

- **A\*** on the grid, 8-connected (no corner cutting past blocked cells), octile heuristic, ties broken by cell index
  (deterministic); portals are extra edges. Goals are spots (room stations, desks), board component spots, or free points.
- **Smoothing**: string-pulling — from each kept point, skip ahead to the farthest later point visible by a supercover
  line over walkable, same-region cells; then one pass of corner cutting (Chaikin, 0.25) limited so no sample enters a
  blocked or inflated cell. Portals stay as fixed waypoints.
- Paths are cached per (start cell, goal cell) for 2 s.

## Following

- A robot **turns before it walks**: it rotates toward the next segment at its species' turn rate and only accelerates
  once within 30° of it; speed follows the gait's speed, eases out at corners (by the turn angle ahead) and eases in at
  arrival, ending on the spot facing the spot's `face`.
- Heights follow the cells (`y`), interpolated; on a ramp the body leans with the slope; on a lift the robot stands still
  while the platform moves.
- If the path becomes blocked (a layout change), it replans.

## Avoiding each other

- Robots are discs; each tick a robot looks 1.2 units ahead along its path. If another robot is there:
  the one with lower priority (idle < walking < working with a deadline; ties by cid) **yields**: it steps aside to the
  nearest free cell off the path (or waits), and the other passes. A robot waiting more than 2 s replans with the
  blocking cells penalised (+5).
- A small separation force (within 0.7) keeps standing robots from overlapping; it never pushes a robot into a blocked
  cell.
- Spots are reserved (robots.md), so two robots never share a destination; lifts and doors admit one robot at a time
  (a queue cell in front of each).

## Verification

- **Unit**: grid build (a part blocks its footprint; a decal doesn't; a room's walls block except its door), A\* optimality
  on small grids, smoothing never enters blocked cells, follower turns before walking.
- **Property** (both apps): every board × 3 layouts × 200 random (start, goal) pairs among spots and doors → a path exists
  when the goal is reachable; no path sample is in a blocked or inflated cell; every room's spots are reachable from every
  other room's door.
- **Simulation** (headless, both apps): 20 robots, 120 simulated seconds of random errands on each board → no robot centre
  ever inside an inflated-blocked cell; robot–robot overlap (distance < 0.5) under 0.5 % of robot-seconds; no robot stuck
  (no progress) for more than 5 s.
- **Fixtures** (`assets/nav/fixtures.json`): (board, params, seeds, start, goal) → path hash; both apps match.
- **Debug overlay**: `?nav=1` on the web floor (desktop `APZ_NAV=1`): walkable/blocked/inflated cells, portals, each
  robot's current path and its yield state.
