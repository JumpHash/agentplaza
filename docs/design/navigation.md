# Navigation: robots that walk around things

Status: design (2026-10-13). The owner: "the robots walk towards walls, items, props, etc and look clunky walking into
those. We need path mapping/tracing for them." Today a robot walks straight at its goal (or along the trace graph between
rooms) and slides through furniture and walls. This adds a navigation grid, path planning, path following and local
avoidance — the same algorithm in both apps (`floor3d/nav.ts`, `floor3d/world/nav.rs`), proven by shared fixtures.

## The nav grid

One grid over the board in world space, cell = 0.25 units. Each cell: `walk` (bool), `y` (floor height), `cost`,
`region` (0 = the board's PCB, else a plate id or a room id). Plate ids come after the room ids (plate i has id
districts + i + 1), so no two regions share an id.

1. **Board**: every cell on the PCB is walkable at the PCB top, unless a hardware part stands on it taller than the step
   height (0.25): those cells are blocked. Part footprints come from the model's flattened world AABBs (the same walk the
   z-fight lint does: `zfight.ts`), so every chip, connector, heatsink and slot blocks correctly; decals, traces and parts
   lower than the step don't. Trace cells cost 0.7 (robots like walking on copper), open PCB 1.0.
2. **Plates**: each slot that a room sits on is a **plate**: its rectangle at the slot's top height, walkable at that
   height, with its own region id. A part under a plate is covered by it and does not block it; a part more than the step
   above the plate top does.
3. **Rooms on plates**: each room's own generator grid (rooms.md) placed in world space at its terrace top: its interior
   and its door cells are walkable; its solids and its walls (the slab margin) are blocked. The band round the room is
   blocked off its plate (a cliff buffer) and walkable on its plate. A room connects to the board only through its
   **door**: the cell outside the door is a plate cell (walkable at the plate top, the same height as the room), so the
   door is a plain step onto the plate, with no portal. A room whose door opens onto a non-plate cell is a layout bug and
   is reported as unconnected.
4. **Landings**: each plate has exactly one landing to the board. It is found once on the board-only grid (plates, PCB,
   parts and traces, no rooms): a multi-source BFS from the plate's perimeter cells, outward over **open** cells (not
   blocked, not inflated), until it reaches the board's main component (the largest connected component). A candidate
   is valid only with the rooms in place (rule 6 of the grid, `landingClearAt` in nav-grid.ts): its plate cell is outside
   every room slab, and its board cell (and a lift's queue cell) is outside every room slab and every cliff band (off
   every plate, within `NAV_RAMP_DEPTH` of a slab). The first valid board cell reached is the landing, and the plate's
   edge cell it leaves from is the portal's other end. A plate at most 1.5 above the board gets a **ramp** (cost =
   length + 2·rise); higher, a **lift**: a cab in a shaft beside the plate's edge, the robot rides it (cost =
   4 + rise; up to three riders, in queue order). A lift's queue cell, two cells beyond the board cell (the cell between them walk too), must be clear, or that cell is skipped. A
   plate with no valid landing is reported as unreached.
   **Layout fallback** (board-layout.ts, `layoutOnBoard`): the layout plans each slot's exit on the same board-only grid
   (the candidate plates included, the same open-cell walk). Once the rooms are built, it builds the nav scene and
   excludes every plate the grid reports unreached, and every plate whose room is unconnected (`connectivity`, rule 12:
   its door or desks cannot reach the main component). Each round excludes those candidates (a slot with no exit left is
   unusable, and its rooms move) and plans again, up to 64 rounds; only then does the board grow by BOARD_SCALE_STEP.
   Candidate exits are keyed (slot, configuration, exit), so a rejected exit is never picked again in the same scale.
   **Gangway** (the layout's landing where no plain one exists, kind "gangway"): its bridge cells (from the plate edge
   out to the lift's cell) become walk at the plate height, except a cell inside a room's slab (its walls). The last bridge
   cell is the lift door, and one lift portal joins it to the board cell (always a lift, whatever the rise).
   `GANGWAY_REACH` stays 3.25 (board-layout.ts).
5. **Clearance**: robots are discs of radius 0.3. A non-blocked cell next to a blocked cell (8-neighbours, or the grid's
   edge) is **inflated**: it is not walkable, so neither a plan nor a commit uses it. Cells exactly two cells from a
   blocked cell cost +0.5 (paths keep off walls and furniture when they can). Door cells, landing cells and spot cells,
   and their 8-neighbours, are never inflated, so a door, a landing or a spot stays reachable.
6. **Connectivity (rule 12)**: a walk cell joins its 8-neighbours (a diagonal only past two walk orthogonal cells) only when
   the height step between them is two-way: at most `NAV_JUMP_UP` (0.8), since a robot can then walk or jump up and walk
   or drop down. Each portal (ramp, lift, gangway lift) joins its two ends. A **one-way drop** (a step above 0.8 up to
   `NAV_DROP_DOWN`, 2.0) joins nothing: it is a directed edge that the planner takes downhill only (moveKind 2). The main
   component is the largest; every walk cell outside it is cut (not walk), so a robot can never fall into a place it
   cannot leave. `connectivity(grid)` reports the main component, the component count, the one-way drops (`oneWay`),
   `doorsOk`, `unconnected` and `unreached`. Test: every seat is reached from the cell the planner snaps it to in the main
   component, and on the 8-agent layout every pair of seats has a path both ways.


The grid is rebuilt when the board, its params or any room layout changes (cheap: ~60k cells for a large board).

## Layout notes (board-layout.ts)

- **Exits**: a slot's aisle leaves by its nearer aisle end, else by a spur (a 1.0 gap through a row, centred on a sample
  column, clear of tall parts, and leaving no room span narrower than one aisle width), else by a gangway. Each candidate
  is keyed (slot, configuration, exit) and is validated as in rule 4 once the rooms exist.
- **Room clearance**: a room keeps `NAV_RAMP_DEPTH` off a spur gap and off the aisle end its exit leaves by (`rowSpans`),
  so the room's cliff band (rule 7a) does not cover the landing. The room's span is then split as before.
- **Desks and doors**: after the furniture is placed, every desk of a room connects to the room's door both ways through
  walk cells. A room whose door or desks are enclosed takes its plate out (its exit is excluded, its rooms move to another
  plate, or the board grows); no seat is deleted. The clearance is the grid's own inflation (one cell, 0.25 units); a separate
  robot-width clearance (0.6 units) is not checked yet.
- **Invariants (per board and agent count, test/tests.ts)**: every seat is in the main component (rule 12); on the 8-agent
  layout every pair of seats has a path both ways; every plate that holds a room is reached; every desk connects to its
  room's door both ways.

**The desktop port must mirror** (apps/desktop/src/console/floor3d/world/):

1. The nav grid's rule 6 (landing search): the open-cell test (not blocked, not inflated) for the plate source, the BFS
   cells and the board cell; the clearance test for the plate source (outside every slab), the board cell and a lift's
   queue (outside every slab and every off-plate band within `NAV_RAMP_DEPTH`).
2. `board_layout.rs`: the board-only grid built with the candidate plates; an outward source needs its inside plate cell
   open; the exit search walks open cells; the room check and the excluded-key fallback (64 rounds, the unreached and
   unconnected exclusions, `slot|id` excluding a whole slot); the spur span rule (`AISLE`); `rowSpans` with the
   `NAV_RAMP_DEPTH` inset off a spur gap and off the exit end.
3. `layout.rs`: the scale loop that retries a pass before it grows the board, and the plate list (one plate per usable slot,
   in candidate order, id = districts + index + 1).
4. `motion.rs`: follows the grid's portals and components; it must not compute a landing of its own. The header of this
   doc names `world/nav.rs` for the grid, which is not in the tree under that name; the grid mirror is the file that holds
   rule 6.

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
- Heights: the pose's `y` is the body's (Gravity and heights); the cells give its ground, and an edge is walked, jumped or
  dropped by the body. On a ramp the body leans with the slope; on a lift the robot stands still while the platform moves.
- If the walk to the next point is no longer on walkable cells of the robot's region (a layout change), it replans from
  where it stands.
- **Proposals, one commit.** Within a step, the followers (the walk and the lift code) and the avoidance (yield, hold,
  separation) only propose a position for each robot. Once every proposal is final, one commit per robot, in id order,
  applies it. Nothing else writes a robot's position, so the invariants below hold for every move.

The invariants (numbered as in `nav-agents.ts`):

1. **Never enter a blocked cell.** The commit takes the proposed point only when the move from the robot's current point
   sweeps walkable cells only: every cell the centre line touches must have `walk` set (a supercover, Amanatides-Woo;
   at a corner the x axis steps first, so the result is deterministic). The robot's own start cell is not tested.
   **Portal exception:** a move whose ends lie within 0.05 of the two cell centres of one portal (either way) is taken,
   because the planner emits that transition. A lift move is taken as it is. If the move fails, the x-only move
   (`x`, current `z`) and then the z-only move are tried; if none passes, the robot stays where it is and
   `blockedSince` records the first failure.
2. **Never stay stuck.** Progress is the distance to the robot's current waypoint shrinking by at least 0.05 within the
   a waypoint crossed or a replan re-bases that distance without counting as progress. These waits stop the
   clock and are not stuck: a finished path (the arrival), a lift wait, a yield (invariant 3), and turning in place
   (heading more than 30° off with speed 0) for up to 1.2 s. Otherwise, after 1 s without progress the robot replans from
   its cell, with a crowd penalty of 1000 on the cells within 1.5 of another robot; a failed plan retries after 0.5 s.
   After 3 s without progress its goal is cleared and `onUnreachable` is called, so the behaviour layer picks its next
   activity.
3. **Every yield has a timeout.** A yield (a side-step or a hold) ends after 1.5 s at most, or sooner once the blocker has
   been clear for 0.5 s. The saved goal is restored, and the robot does not yield to that blocker again for 1.5 s. If the
   hold is still blocked after the rejoin, the robot proceeds: invariant 1 keeps it out of blocked cells, and separation
   pushes it apart within walkable cells.
4. **Snapping and components.** A spawn snaps to the nearest walkable cell. A goal not on a walkable cell (or off the grid)
   snaps to the nearest walkable cell of the robot's own component; with none, the robot keeps no goal, stays where it is,
   and `onUnreachable` is called at once. The walkable cells form connected
   components (8-neighbours, a diagonal only past two walkable orthogonal cells, and a portal whose two ends are
   walkable); they are computed when the grid is set. A plan needs its start and goal in one component, otherwise it
   fails at once (and is retried as in 2), so A* never searches a region it cannot leave.

## Avoiding each other

Each tick, the avoidance pass (`avoid`, then `separate`) reads the robots' paths and positions and proposes moves; the
rules below are numbered as in `apps/web/src/views/floor3d/nav-agents.ts` (its rule 10 covers the look-ahead, the
priority, the yield and the hold; its rule 12 the separation).

8. **Look-ahead.** Robots are discs. Each tick a robot looks 1.2 units ahead along its path. Another robot whose centre
   is within 0.6 of that point, or within 0.6 of the robot itself, is a conflict.
9. **Priority.** Of two conflicting robots, the one with the lower priority (idle < walking < working with a deadline)
   **yields**. Equal priority: the higher id yields. A robot on a lift neither yields nor is yielded to.
10. **Yield.** The yielding robot steps aside to the nearest walkable cell within 6 cells that is at least 0.6 from every
    other robot and not within one cell of its remaining path, and walks there. The yield ends once the blocker has been
    at least 0.6 away for 0.5 s, once the blocker is gone, or after 1.5 s at most. The saved goal is then restored and the
    robot replans from where it stands. After a time-out it does not yield to that blocker again for 1.5 s. If two robots
    yield to each other, the lower id proceeds.
11. **Hold.** A robot with no free cell holds on the spot under the same limits as a yield (rule 10). A hold does not
    replan, and a held robot is not stuck (invariant 2).
12. **Separation.** Robots closer than 0.7 are pushed apart by the full overlap each. If the full push does not land on a
    walkable cell, half of it is tried, else there is no push. Separation only proposes positions: the commit point
    (invariant 1) decides which move is taken, so a push never enters a blocked cell.
- **Crowd and give-up.** A stuck replan (invariant 2) adds a cost of 1000 to every cell within 1.5 units of another
  robot, so the new path keeps off the crowd where it can. A robot with no progress for 3 s has its goal cleared and
  `onUnreachable` is called, so the behaviour layer picks its next activity. A failed plan is retried after 0.5 s.

- Spots are reserved (robots.md), so two robots never share a destination; a lift's cab takes up to three riders in
  queue order (a queue cell in front of each; see Lifts).

## Gravity and heights

The robot's height is its body, not its cell. The pose's `y` is the body's feet height (`floor3d/nav-height.ts`). The nav
world plans on cells as before (rules 1 to 13). After each step's commit and stuck check, `NavWorld.step` steps every
robot's body once, in id order (`vertical`), and then audits it (`audit1`).

**Constants** (nav-types.ts, "Gravity and heights"): `NAV_GRAVITY` 20; `NAV_STEP` 0.3 (walked); `NAV_JUMP_UP` 0.8 (a
jump's rise limit); `NAV_DROP_DOWN` 2.0 (a fall's limit, and the respawn's); `NAV_JUMP_TIME` 0.45 (a jump, from launch to
landing); `NAV_CROUCH_TIME` 0.1; `NAV_LAND_TIME` 0.3 (the landing squash); `NAV_GLIDE_SPEED` 1.5; `NAV_RESPAWN_OUT` 0.25 and
`NAV_RESPAWN_IN` 0.35. The footprint radius is `NAV_RADIUS` 0.3; the audit's tolerance is `AUDIT_EPS` 0.02 (nav-agents.ts).

**Surfaces.** Under the robot (`surfacesAt`):

- `floorTop` (the support): the highest surface under the four corners of the footprint (x ± 0.3, z ± 0.3), over the cells
  that are not blocked, off the board or cut off. A cliff, a wall or a part is not a floor for a corner. The centre cell
  is always included.
- `walkTop`: the support when the centre cell is walk, else none (the robot is airborne).
- Off the board (`NavGrid.off`, set by buildGrid rule 1): no floor at all.

**Drawn tops and the walk invariant.** When the drawn scene is known (`NavWorld.setDrawn`, from the board and its district
rooms; the plates no district covers, the aisles, are not drawn yet, so their walk cells have no drawn top), `walkTop` is the drawn top under the centre, not the grid's height: `drawn-tops.ts` bakes, at 0.05 spacing over the
grid, the highest upward drawn face whose height is within NAV_STEP of the grid height of its walk cell (a part taller than
a step is blocked on the grid, so walls and furniture never enter), and the lookup is bilinear over the four samples around
the centre (NaN where none is drawn: the grid's height is used and the fallback is counted). The invariant the audit checks
(`audit()`): a grounded robot's centre is on a walk cell and its feet are on the drawn surface (`hover` 0: no feet more than
0.01 over the exact drawn top under the centre, a floating species exempt; `onWall` 0: no feet above the grid's surface).
Re-home on a layout change: the agent count scales the board, so `setHome` moves each robot's home with it (a respawn
returns there), and `setGrid` projects a robot standing off the new walk to the nearest walk cell and re-grounds it.

**Modes** (each robot's body; `d` = walkTop - y):

- `ground`: no walkTop: fall (glide for a floating species, when a floor is under it). `d` above NAV_STEP and up to
  NAV_JUMP_UP: crouch (a floating species jumps at once). Above NAV_JUMP_UP (never planned): snapped up. Below -NAV_STEP:
  fall (glide for a floating species). Otherwise y = walkTop exactly, every step.
- `crouch`: y holds for NAV_CROUCH_TIME, then `launch`. The crouch is the take-off, so the audit treats it as airborne.
- `jump`: launch from y0 with v0 = (walkTop - y0 + g T^2 / 2) / T, T = NAV_JUMP_TIME. The height at t from the anchor is
  y0 + v0 t - g t^2 / 2, the velocity v0 - g t. Lands when v <= 0 and y <= walkTop + 1e-4 (y = walkTop, mode `land`). With
  no walkTop in the air, it keeps falling.
- `fall`: ballistic from the anchor (v0 = 0 at the start of a fall). Lands like a jump. When y0 - y is more than
  NAV_DROP_DOWN, or there is no floor, it goes to `out`.
- `glide` (floating species): y = max(target, y - NAV_GLIDE_SPEED dt), target = walkTop, else floorTop. Back to `ground`
  once walkTop is within NAV_STEP above y. No floor: `out`.
- `land`: y = walkTop for NAV_LAND_TIME, then `ground` (no walkTop: `fall`).
- `out`: the fall goes on (ballistic). After NAV_RESPAWN_OUT the robot respawns: its goal, path, lift seat and avoidance are
  dropped; x, z = its home (the add position, snapped to a walk cell when it is not one); heading 0; the body is `in`.
  The floor's `onRespawn` hook makes the behaviour choose again (`RobotWork.unreachable`).
- `in`: NAV_RESPAWN_IN, then `ground`.
- `lift`: while riding, y is the cab floor (`surfaceY` of the lift), set every step by the lift code, not by the body.

**Fade.** The respawn fade scales the robot group, not its opacity (the materials are shared by species): `out` shrinks
from 1 to 0.02 with u^2 over NAV_RESPAWN_OUT (u = t / duration); `in` grows back from 0.02 to 1 with an ease-out-back
over NAV_RESPAWN_IN. The pose reports the scale.

**Ground audit (G5).** After every body step, each robot that is not riding is counted (`audit1`, `NavWorld.audit()`,
`window.__apzNavAudit()` in the floor). A grounded body (`ground`, `land`, `in`) is:

- *floating* when its feet are more than AUDIT_EPS (0.02) above its support, or it has no support;
- *sinking* when they are more than 0.02 below it.

The test (test/tests.ts) requires 0 floating and 0 sinking frames on every board for 60 s with 8 agents. Separation pushes
and wall slides re-ground in the same step: the body is stepped after the commit.

**Invariants** (tested in test/tests.ts, "Gravity and heights"):

- G1. A grounded body is on a walk cell: no robot is drawn above a non-walkable cell while grounded.
- G2. Nothing stays in the air: an air phase ends by a landing, a floor (glide) or the respawn. No air frame is stuck for
  more than 2 s.
- G3. Planned moves never respawn a robot: a planned drop is at most NAV_DROP_DOWN (rule 12). A respawn comes only from a
  fall longer than NAV_DROP_DOWN, or from a robot with no floor under it (off the board).
- G4. A forced push (`NavWorld.shove`, a test or debug move, or a layout change under the robot) that drops it more than
  NAV_DROP_DOWN respawns it at home within 1.5 s (0.25 s out, then the fall, then the fade in); a drop up to NAV_DROP_DOWN
  lands.
- G5. The ground audit above: no floating and no sinking grounded frame.

**Clips.** The body is a layer over the hold, the one-shot and the state's clip (`robot.setBody`). Shared clips
(assets/robots/clips.json, `jump.crouch`, `jump.air`, `fall.air`, `land`) name only bones every species has (the body,
the head, the arms): a crouch (body 0.92, down 0.06, arms down), a jump (body 1.04, arms up, head up), a fall (arms out, head
0.1) and a landing (body 0.86 and down 0.05, for NAV_LAND_TIME). The feet are the species' own (`sp.feet`): the state tucks
them by `AIR_FEET` in the air (jump +0.06, fall -0.03). Floating species (`floats`) skip the crouch and glide; wheeled
species (no feet) get the body channels only.

**The desktop port must mirror** (apps/desktop/src/console/floor3d/world/motion.rs, and robot.rs for the clips):

1. The constants above, with the same values.
2. The body's modes, transitions and formulas exactly as listed (the anchor, the launch velocity, the landing test with
   1e-4, the exact ground contact, the glide, the fade and its scale formulas).
3. The support: the footprint's corner rule (the highest non-blocked, on-board surface, the centre must be walk), and the
   off-board flag (the rust grid sets it in its board rule 1).
4. The order: after the commit and the stuck check, once per robot in ascending id; a riding robot's body is its cab floor.
5. The respawn: drop the walk, the lift seat and the avoidance; x, z = home (snapped); heading 0; the body `in`; the
   respawn hook.
6. The pose fields: y is the body's; the body mode and the scale drive the clip (`jump.crouch`, `jump.air`, `fall.air`,
   `land`, none on the ground or in `in`) and the group's scale; the species' feet tuck by `AIR_FEET`.
7. The audit counters (floating, sinking) so the port's simulation can report the same numbers.

## Lifts

A lift is a cab in a shaft beside a plate: it serves two stops, the board (stop 0, the PCB top `NavLift.low`) and the plate
(stop 1, `NavLift.high`). Robots ride it between a plate and the board, up to three at a time, in queue order. The shaft
and its look are `floor3d/lift.ts`; the cab's mechanics are `floor3d/lift-ctrl.ts` (one `LiftController` per lift, no
three.js, no grid); the robots' side is `nav-agents.ts` (rule 4). The gangway (`gangway.ts`) is only the bridge: it ends at
the cab's plate side, so the bridge's deck and the top stop meet flush.

**Geometry** (lift.ts, in the lift's frame: z = 0 is the front cell's centre, +z toward the walkway, -z toward the plate).
The cab is `CAB_W` 1.6 along the wall and 0.5 deep, from `CAB_BACK` -0.12 (the plate's edge plane) to `CAB_FRONT` 0.37. The queue is
two cells out from the board cell, so the deck covers only the front cell and the cell between, and robots queue beyond it. It is open at both landings: stop 1 at the plate side (the
door), stop 0 at the walkway side. Each stop has its own gate (two leaves, `GATE_H` 0.45 high) that slides apart as the
doors open, and its own call panel with a lamp that lights while robots wait there. The shaft is four posts from the board
to `high + GATE_H + 0.04` (the lintel over the gates), a base plate at each, X braces on both sides, and rails over the
gates. A counterweight rides a guide outside the frame and runs opposite the cab on a cable over a sheave beam; it never
goes below the board. Nothing floats: the posts stand on the PCB top and the top stop is the plate's top.

**The controller** (lift-ctrl.ts). Phases: `idle` (at a stop, doors shut) → `opening` (doors open, `doorTime` 0.5 s) →
`unloading` (riders bound here alight; at least `unloadTime` 0.6 s, at most `unloadMax` 2.5 s, then the rest are dropped)
→ `boarding` (the admitted robots step in: a dwell of `dwell` 1.0 s, extended up to `holdMax` 2.5 s while an admitted robot
is still walking in) → `closing` (0.5 s) → `travel` (smoothstep from one stop to the other at `speed` 1.2 units/s on
average, so the peak is 1.5 times that) → arrive (riders bound here become `arrived`) → `idle`.

- *Decision at an idle stop, doors shut:* riders bound here unload first; else riders aboard go to their stop; else the
  front of the queue decides: if it is here the doors open, else the cab goes to its stop. The queue is one line over both
  stops in arrival order, so a robot at the other end waits for the robots ahead of it and no more.
- *Admission:* at boarding, the queued robots at this stop are admitted in arrival order up to `capacity` 3 (less the
  riders still aboard). A robot admitted but not in the cab by the time the doors close goes to the back of the line.
- *Queue expiry:* a queued or admitted request not touched for `QUEUE_TTL` 3 s is dropped, so a robot that leaves its
  landing cannot keep the cab running for a ghost. The robot touches its request every step it stands at its landing.
- *Doors and motion:* the cab moves only from `idle` with the doors shut (door opening 0 in `travel`), and the doors only
  open at a stop. `doorT` is the opening (0 shut, 1 open); `doorsOpen` is `doorT > 0`.
- *Idle:* with nothing queued or aboard the cab is `idle` at its last stop with its doors shut and does not move.

**The interface** (nav-types.ts, "Lifts"):

```ts
type LiftTicket = 'none' | 'queued' | 'boarding' | 'riding' | 'arrived';
interface LiftState { cabY: number; doorsOpen: boolean; boarding: number[]; doorT: number; phase: LiftPhase; stop: 0 | 1; calls: [number, number] }
interface LiftHandle {
  request(fromY: number, toY: number, id: number): boolean; touch(id: number): void; ticket(id: number): LiftTicket;
  board(id: number): boolean; alight(id: number): boolean; cancel(id: number): void; state(): LiftState;
}
// NavWorld.lift(id): LiftHandle | null (null for an unknown lift). NavWorld.liftState(id) remains for debug readers.
```

- `cabY` is the cab floor's height: a rider's y is `cabY` every frame. `boarding` lists the robots aboard (riding or
  arrived). `stop` is the stop the cab is at (or going to); `calls` counts the robots waiting at each stop.
- A robot asks for a ride with `request(fromY, toY, id)` from the landing it stands at (fromY is that landing's height,
  toY the other). It `touch`es the request each step it stands at the landing. Its `ticket` tells it what to do: `queued`
  (wait at the queue), `boarding` (walk in and call `board(id)`), `riding` (its y is `cabY`, it does not walk), `arrived`
  (the cab is at its stop: once the doors are open it calls `alight(id)` and steps off at the far landing), `none` (no
  ride: walk on).
- nav-agents' rule 4 (the boarding, riding and step-off, and the queue) is built on these calls; the cab is never moved
  by it.

**Rules** (nav-agents.ts rule 4): a robot whose path reaches a lift portal from either landing requests the cab for that
landing and waits at the queue (a legitimate wait, not a stall); it boards when its ticket is `boarding`; it rides while
`riding` (or `arrived` with the doors still shutting), its position the platform centre; it steps off when `arrived` with
the doors open, and its remaining path is its plan to the goal. A plan takes a lift only for a change of level (the
`noLifts` option, as before).

**The desktop port must mirror** (apps/desktop/src/console/floor3d/world/, and its lift renderer):

1. `lift-ctrl.ts`: the constants (`QUEUE_TTL` 3, `MAX_DT` 0.25, the sub-step 1/60 s, the defaults above), the phase machine
   (`tick`, `decide`, `afterOpen`, `beginBoarding`, `startTravel`, `arrive`), the queue order (by arrival `seq`, a missed
   robot gets a new one), the admission and capacity, the smoothstep travel and its duration `|high - low| / speed`.
2. `nav-agents.ts` rule 4: the request, touch, board, aboard (riding; arrived with doorT < 1), alight, cancel and the
   step-off, and the removal of a robot from every cab (`remove`, respawn).
3. `lift.ts` geometry: the frame, the cab's planes (`CAB_W`, `CAB_BACK`, `CAB_FRONT`), gates, call lamps, counterweight and
   cables. `gangway.ts`: the bridge length `(bx - sx) n + (bz - sz) n + CAB_BACK`.
4. `NavWorld.lift(id)` and `liftState(id)`; the floor reads the handle's `state()` each frame (never a second cab).

Tests (test/tests.ts): the controller's "lift:" block (ordering, capacity, no move with the doors open, every request
served within a bound, two robots from opposite ends, idle when nothing is queued, the request rules), and "lift riders"
(six trips on the server board, the slowest within 90 s, and riders' bodies on the cab).

## Verification

- **Unit**: grid build (a part blocks its footprint; a part at most one step high blocks nothing), a route on an open
  board, a detour round a wall that avoids blocked and inflated cells, the path hash, and a follower that turns before it
  walks.
- **Property** (both apps): every board × 3 layouts × 200 random (start, goal) pairs of walkable points (spots and portal
  cells) in one component → no path sample is in a blocked or inflated cell, and at most one pair per layout has no path.
- **Simulation** (headless, both apps): 20 robots, 120 simulated seconds of random errands on each board (the 8-agent
  layout). The sim reads progress and the legitimate waits the same way invariant 2 does (from the public pose and
  path, not the engine's timer), and prints one JSON line per board:
  - `badSteps`: robot steps with the centre in a blocked or inflated cell. Must be 0 (invariant 1).
  - `stuckEvents`: stuck events opened (more than 1 s of non-legit time without progress).
  - `resolvedMaxSeconds`: the longest closed event, measured as its non-legit time after the first second. Must be under 3.
  - `openAtEnd`, `openMaxSeconds`: events still open at the end; the longest must be under 3.
  - `overlapPct`: robot–robot overlap (distance < 0.5) as a share of robot-seconds. Must be under 0.5 %.
  - `goalsReached`, `goalsIssued`, `unreachable`: arrivals, goals given, and goals cleared by the give-up in 2.
  A board passes when all four criteria hold and its property checks pass. The suite (`test/tests.ts`) runs a 15 s
  variant on three boards (workstation, server, apple-soc) with the same criteria.
- **Gravity** (`test/tests.ts`, "Gravity and heights" and "Lift riders"): the body's modes on flat surfaces (a walked step, a jump of 0.8 with
  its crouch and landing, a fall of 1.5, a fall over NAV_DROP_DOWN, no floor, a glide), the fade, a forced push off a 2.5 drop
  (respawn within 1.5 s), a 1.5 drop (lands), and 60 s with 8 agents on every board: no respawn, no air frame stuck for more
  than 2 s, no grounded robot off a walk cell, no robot in a blocked cell, no floating or sunk frame (the audit). Six lift trips on the
  server board arrive within 150 s, and a rider's y is its cab floor.
- **Fixtures** (`assets/nav/fixtures.json`): (board, params, seeds, start, goal) → path hash; both apps match.
- **Debug overlay**: `?nav=1` on the web floor (desktop `APZ_NAV=1`): walkable/blocked/inflated cells, portals, each
  robot's current path and its yield state.
