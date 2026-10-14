# Walk surfaces: one model for what robots stand on

Status (2026-10-14): built. The surfaces and links are `crates/apz-board/src/walk.rs`; the layout emits them
(`layout/surfaces.rs`); the grid is rasterised from them (`grid.rs`). The drawn-support and z-fight checks are the
audit (`audit.rs`) and the z-fight lint (`zfight.rs`).

## The problem this fixes

Robots walk a grid graph: cells with a surface height, edges between neighbours within a step, one-way drops,
and portals for ramps and lifts. A* plans over it, and connected components say what is reachable. The graph
itself is sound. What was wrong is where it came from.

The grid was built from the layout's own numbers: slot rectangles, plate heights, landing points and room
footprints. The board people see is drawn separately, from the hardware models and the generators. The two
drifted apart:

- A lift landing was walkable on the grid with nothing drawn under it, so robots floated to the entrance.
- A wall top or a room-slab edge counted as support, so robots walked on walls.
- Plates and columns rested on numbers rather than on drawn tops, so things floated.

Fixing these one case at a time could not converge.

## The rule

**Robots walk only on surfaces that are drawn, and every drawn floor robots may use is walkable.** Both the
picture and the graph come from one list of walk surfaces.

```
WalkSurface {
  id, kind: "board" | "plate" | "room-floor" | "ramp" | "stair" | "bridge" | "landing" | "cab",
  shape: polygon in x/z (convex pieces) + height function (flat y, or a plane for a ramp),
  owner: the part or generator that draws it (so the drawn top IS this surface),
  edges: which sides are open (walkable to a neighbour surface), walled, or a drop (with its height)
}
WalkLink { from, to, kind: "step" | "ramp" | "drop" (one way) | "lift" (via cab) | "door" }
```

- **The generators produce walk surfaces first.** That covers board layout, plates, rooms, ramps, gangways,
  landings and the lift cab. Each surface's mesh is then built from that surface, so the drawn top and the walk
  height are the same numbers by construction. Hardware tops that robots stand on (the board's PCB top, a chip
  top that carries a plate) are walk surfaces of kind `board`, taken from the hardware model's own top faces.
- **The nav grid is rasterised from the walk surfaces, not from layout numbers.** A cell is walkable only if
  its centre and its footprint corners lie on one surface, or on two linked surfaces within a step. Its height
  is that surface's height at the point. Walls, slabs, furniture and anything not in the walk list are never
  support.
- **Links become graph edges:** steps and ramps are two-way, drops are one-way downhill, and a lift link joins
  the bottom landing to the top landing through the cab. A landing is a real surface: a slab drawn on the
  board, or the end of a bridge, at the cab's floor height. The cab's floor is flush with each landing it stops
  at, with no gap and no overlap.
- **Robots stand on the surface under their centre.** Feet height is the surface height at the centre. Corners
  only confirm footing. A robot's centre is never off a walk surface; if a push would move it off, it slides
  along the edge.

## Checks that keep it true (tests, every board, n = 1, 4, 8, 20)

1. **Drawn support:** for every walkable cell, a ray down from just above the cell's height hits a drawn mesh
   belonging to that cell's surface within 0.01. No walk cell sits over air.
2. **No hidden floor:** for every walk surface, its mesh exists and is visible.
3. **Lifts connect:** each lift's two landings are walk surfaces in the main component, joined by the lift
   link, and the cab floor is flush with both within 0.005.
4. **Reachability:** every seat, home and landing is reachable from every other, using lifts where needed.
5. **No wall walking:** in a 60 s run, no robot frame has its centre off a walk surface or its feet above
   the surface under its centre while grounded.
6. **No z-fighting:** no two drawn surfaces are coplanar and overlapping.

## The desktop port

There is no separate desktop mirror any more. The desktop and the web both draw through `apz-engine`, which reads
the one walk-surface list and rasterisation in `apz-board`. The shared nav fixtures (`assets/nav/fixtures.json`) are not yet read by a Rust test.
