# Robot species: authoring guide

The format is in `docs/design/robots.md` ("Species format", "Clips", "Tools and props"). This page is the
working version for the species authors: what to write, what the renderer does with it, and how to see it.

## Files

```
assets/robots/
  index.json        the lists, in order: species, tools, props (the species order is the deal, see below)
  palette.json      the eight fixed robot colours (ink, paper, rule, signal, ok, info, inkMuted, sheet)
  clips.json        the shared clips (idle, type, walk.<locomotion>, offline, blink, wave, proud, think, cheer)
  species/<id>.json one per species, schema 1
  tools/<id>.json   held tools and carried items (grip one | two | carry)
```

Code: `apps/web/src/views/floor3d/robots/` (`types.ts` the formats, `validate.ts` the checks, `files.ts` the
static imports and the registry, `geometry.ts` the part geometry, `clips.ts` the channel evaluation, `rig.ts`
the baked geometry and the per-robot meshes, `fx.ts` the effects pool). The API is `floor3d/robot.ts`.

## Registering a species

1. Write `species/<id>.json` (copy `rover.json` or `can.json`; `"id"` must equal the file name).
2. Add `<id>` to `index.json` `species`. The client's fallback deal is `species[cid % n]` (the hub deals species in
   phase 2): appending changes who gets what, so add species in batches.
3. In `floor3d/robots/files.ts`, import the file and add it to `FILES.species`. The test "every species file is in the
   index" fails until both are done. Tools are the same: `tools/<id>.json`, `index.json` `tools`, `FILES.tools`.
4. `cd apps/web && npm run check` (types, build, tests). The registry lists every error by file path; a species with
   errors is left out of the floor, and the rover stands in for it.

## Species (`species/<id>.json`)

- **Bones**: `{ "parent": "root" | <bone>, "at": [x, y, z] }`. Required: `base`, `body`, `head`, `armL`, `armR`,
  `gripL`, `gripR`, `carry`. A species without arms still has the arm bones (tentacles, stalks, a beam). Add
  extra bones freely (a rotor, a tail, feet). A tool shows in `gripR`, a crate at `carry`.
- **Parts**: `{ "bone", "geo", "mat", "at", "rot" (degrees), "scale", "outline" }`. The geometry types are the
  hardware ones: `box`, `rbox` (`w h d r`), `cyl` (`rt rb h`), `cone` (`r h`), `sphere` (`r`), `torus` (`r tube arc`),
  `plane`, `ring`, `shape`. Robots add optional `seg` (rbox `[n]`, cyl/cone `[radial]`, sphere `[w, h]`,
  torus `[radial, tubular]`) and `half: "top"` (a dome). Parts on a bone are merged with the bone's other parts:
  a species with a dozen parts still makes one mesh per bone.
- **Materials** (`mat`): `shell` (the agent's hue), `shell2` (its darker shade), `panel` (the district's hue), `paper`,
  `trim`, `ink`, `signal`, `glass` (dark, never glows), `accent` (the species' own colour, `accent`), `eye` (state
  colour, glows), `light` (signal glow), `thruster` (unlit, info or offline colour), `clear` (translucent). An inline
  material `{ "color": "#hex", "emissive", "glow", "opacity", "flat" }` also works. The highlight glow
  (`setHighlighted`) applies to shell, shell2, paper, trim, ink, signal and accent, not to glass, eye, light,
  thruster or clear.
- **Ink**: the cartoon line is a hull per bone over its inked parts. Parts of material `ink` are not inked (they
  already are the ink colour); parts smaller than 0.03 radius are not inked; `"outline": false` drops it for any
  part (a flat decal: a belly disc, a cheek). Use `outline: false` on small parts that would cost a draw call.
- **Eyes**: `{ "bone", "at", "r" (glass socket radius), "pupil", "shape", "glass": false?, "stretch"? }`. Shapes:
  `round`, `bar`, `visor`, `ring`, `heart`, `arc`. The pupil sits 0.004 in front of its socket's `at.z`, blinks
  (its lids scale in y) and takes the state colour. Eyes on the same bone at the same height share one mesh.
- **spin**: `{ "bone", "axis": "x|y|z", "walk", "idle", "offline"? }` rad/s, accumulated: a rotor (`axis: y`), a
  ball wheel's stripe (`axis: x`). A spinning part must sit on its own bone (its axis is that bone's origin).
- **feet**: bones that step while walking, alternately (sin in z, half-sine in y).
- **floats**: `true` bobs the base and body (2.2 rad/s), and sets them down when offline (hover, lamp, ufo).
- **gait**: `bob`, `sway`, `lean` are absolute values that the shared walk clips read ("gait.bob" and so on). `freq`,
  `speed` and `sideways` are reserved for the behaviour engine (phase 3) and do nothing yet.
- **personality**: integers 0–9 (`social energy focus curiosity play tidy`). **quirks**: `[{ "clip", "weight" }]`
  (clips that exist, own or shared). Both are read by the behaviour engine in phase 3.
- **clips**: this species' own clips (same format as `clips.json`, naming this species' bones). **overrides**: a
  shared clip id → an own clip id (for example `"type": "owl.peck-type"`).
- **orchestrator**: `{ "parts": [...] }` extra parts only the orchestrator wears (the rover's antenna and trim).
  The orchestrator is always the rover, so the extras go in `rover.json`.

Validation (`validate.ts`) names the path of every error: an unknown field, a bone that is not on the species, a
material name that is not in the list, a missing required bone, a parent cycle, a clip that names a missing bone.

## Clips (`clips.json`, and a species' own)

A clip is a list of channels, evaluated every frame and blended toward its targets with `k = min(1, dt·8)`:

```json
"type": { "loop": true, "channels": [
  { "bone": "armL", "prop": "rot.x", "base": -0.9, "amp": 0.2, "freq": 1.7507 },
  { "bone": "head", "prop": "lid", "base": 1, "amp": -0.9, "freq": 0.2778, "wave": "pulse", "duty": 0.0389, "snap": true }
] }
```

- `prop`: `rot.x|y|z` (radians), `pos.x|y|z` (offset from the bone's rest), `scale` (rest 1), `lid` (the eye lids,
  rest 1: a blink is a `pulse` on `lid`, with `snap: true` so it does not blend).
- `value = base + amp × wave(2π·freq·t + phase)`. `freq` is in **Hz** (rad/s ÷ 2π). `t` is the global clock, so a
  blink lands where it always did.
- `wave`: `sin` (default), `abs`, `halfsin`, `tri`, `saw`, `square` and `pulse` (both with `duty`), `hold`.
- `base`, `amp` and `freq` can be `"gait.bob"` (and so on): the species' gait value.
- `rate` (rad/s) accumulates instead of blending: a rolling wheel that is not a species spin.
- `env: "in-out"` with `dur`: a one-shot's amplitude follows a sine envelope.
- `loop: false` with `dur`: a one-shot that returns to the state's clip.
- `fx`: `[{ "kind", "at", "every" }]` spawns effects while the clip plays. Built now: `sparks`, `zzz`, `dots`,
  `hearts`, `keys`. The others (`steam`, `bubbles`, `notes`, `ink`, `glow`, `trail`, `confetti`) are logged as no-ops.

Shared clips may name only the required bones. A species' own clips may name its bones. A species that has no
`walk.<locomotion>` falls back to `walk.treads` (with one warning). Walk clips exist for `treads`, `wheels`, `ball`,
`walk4` and `hover`; the other locomotions (`hop`, `waddle`, `bounce`, …) are phase-2 content.

The state's clip: idle → `idle`, working → `type`, walking → `walk.<locomotion>`, offline → `offline`. A held clip
(`play(id, {loop: true})`, `pose`) replaces it until `play(null)`.

## Tools (`tools/<id>.json`)

`{ "schema": 1, "id", "name", "grip": "one" | "two" | "carry", "parts": [...], "tip": [x, y, z] }`. The parts have
no `bone`; they are built at the tool's origin, which is the grip (`one`: `gripR`; `two`: `gripR`, aimed at
`gripL`; `carry`: the `carry` bone). A held tool adds two draw calls (its fill and its ink).

## The lab

`#/robot-lab` shows the species under the Floor's lighting. The query parameters:

```
#/robot-lab?species=rover&clip=type&tool=wrench&carry=crate&state=working&hue=200&orch=1&cam=iso&fx=sparks
  species   a species id, or all (the line-up)        clip    a shared or own clip (empty: the state's)
  tool      a tool id, held in the grip                carry   an item id, carried
  state     idle | working | walking | offline         hue     0–360 (shell and panel)
  orch=1    the rover as the orchestrator              cam     front | iso | side
  fx        an effect, spawned every 0.7 s
```

The line shows the draw calls per robot (the main pass, shadows off, the floor hidden). Target: 12 or fewer.

## Budget

Per robot: one matte mesh per bone (only if it has parts), one ink hull per bone (only if it has inked parts), one
mesh per solo material (`eye` parts, `light`, `thruster`, `clear`, inline), one pupil mesh per eye row. The rover is
11 (12 as orchestrator), hover 12, tv 12, can 12, kitty 11. A held tool or carried item adds 2. Each extra part
that needs its own bone or its own material adds a draw: keep parts on the bone that already has them.

## Checks

- `cd apps/web && npm run check`: the types, the build, and the tests. The tests load every listed file, check the
  required bones, the clips' bones, the draw budget (as a count of meshes), the floor's API, and the blink.
- `cd apps/web && node test/run.mjs`: the tests alone.
- Look at it: `#/robot-lab?species=<id>` in the console (`npm run build` first, or the dev server).
