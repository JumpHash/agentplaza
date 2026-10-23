# Robot species: authoring guide

The format is in `docs/design/robots.md` ("Species format", "Clips", "Tools and props"). This page is the
working version for the species authors: what to write, what the renderer does with it, and how to see it.

## Files

```
assets/robots/
  index.json        the lists, in order: species, tools, props (the species order is the deal, see below)
  palette.json      the eight fixed robot colours (ink, paper, rule, signal, ok, info, inkMuted, sheet)
  clips.json        the shared clips (idle, type, offline, blink, wave, proud, think, cheer, the walk.<locomotion>
                    clips, and the work and gesture clips)
  species/<id>.json one per species, schema 1
  tools/<id>.json   held tools and carried items (grip one | two | carry)
  props/<id>.json   station props: furniture a robot works at, with its spots (schema 1)
```

Code: `crates/apz-engine/src/robots/` (`types.rs` the formats, `validate.rs` the checks, `files.rs` the embedded
files and the registry, `geometry.rs` the part geometry, `clips.rs` the clip evaluation, `rig.rs` the baked geometry and
the per-robot meshes, `props.rs` the station props, `fx.rs` the effects pool, `audit.rs` the design audit, `lab.rs` the
robot lab). The API is `crates/apz-engine/src/robot.rs`.

## Registering a species

1. Write `species/<id>.json` (copy `rover.json` or `can.json`; `"id"` must equal the file name).
2. Add `<id>` to `index.json` `species`. A cid's fallback species is the list in order without `oracle`, dealt by
   `cid % n` (`robot_variant` in `crates/apz-engine/src/robot.rs`); a hub record that names a species takes it instead.
   Appending changes who gets what, so add species in batches.
3. Nothing to import: the build embeds every file under this folder (`crates/apz-engine/build.rs` lists them). Tools
   are the same: `tools/<id>.json` and `index.json` `tools`.
4. `cargo test -p apz-engine --release --lib` (the registry and audit tests). The registry lists every error by file
   path; a species with errors is left out of the floor, and the rover stands in for it.

## Species (`species/<id>.json`)

- **Identity**: `id` (the file name), `name` (the nickname on the agent card), `blurb`, `accent` (the species' colour),
  `height`, `label_top` (the label's height above the root), and `locomotion`, which picks the walk clip
  (`walk.<locomotion>`).
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
  (`set_highlighted`) applies to shell, shell2, paper, trim, ink, signal and accent, not to glass, eye, light,
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

Validation (`crates/apz-engine/src/robots/validate.rs`) names the path of every error: an unknown field, a bone that is not on the species, a
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
- `fx`: `[{ "kind", "at", "every" }]` spawns effects while the clip plays. Built: `sparks`, `zzz`, `dots`,
  `hearts`, `keys`, `steam`, `ink`, `glow`, `notes`, `bubbles`, `confetti`, `trail` (`robots/fx.rs`). Any other kind is not built.

Shared clips may name only the required bones. A species' own clips may name its bones. A species that has no
`walk.<locomotion>` falls back to `walk.treads` (with one warning). Every `walk.<locomotion>` in `clips.json` is a shared
walk clip (`treads`, `wheels`, `ball`, `walk4`, `hover`, `hop`, `waddle`, `bounce`, and the others listed there).

The state's clip: idle → `idle`, working → `type`, walking → `walk.<locomotion>`, offline → `offline`. A held clip
(`play(id, {loop: true})`, `pose`) replaces it until `play(null)`.

## Tools (`tools/<id>.json`)

`{ "schema": 1, "id", "name", "grip": "one" | "two" | "carry", "aim"?, "parts": [...], "tip": [x, y, z] }`. The parts have
no `bone`; they are built at the tool's origin, which is the grip (`one`: `gripR`; `two`: `gripR`, aimed at
`gripL`; `carry`: the `carry` bone). `aim` sets how a one-hand tool sits in the fist: `forward` (the default), `down` or
`up`. A held tool adds two draw calls (its fill and its ink).

## The lab

`#/robot-lab` shows the species under the Floor's lighting. The query parameters:

```
#/robot-lab?species=rover&clip=type&tool=wrench&carry=crate&state=working&hue=200&orch=1&cam=iso&fx=sparks
  species   a species id, or all (the line-up)        clip    a shared or own clip (empty: the state's)
  tool      a tool id, held in the grip                carry   an item id, carried
  state     idle | working | walking | offline         hue     0–360 (shell and panel)
  orch=1    the rover as the orchestrator              cam     front | iso | side | q34 | top
  prop      a station prop id, or all (each at its spots)  t      seconds: a frozen clock
  bg        dark | light (the scene background)
  fx        an effect, spawned every 0.7 s
```

The line shows the draw calls per robot (the main pass, shadows off, the floor hidden). The budget test allows 14
(`each_species_builds_within_the_draw_budget`).

## Budget

Per robot: one matte mesh per bone (only if it has parts), one ink hull per bone (only if it has inked parts), one
mesh per solo material (`eye` parts, `light`, `thruster`, `clear`, inline), one pupil mesh per eye row. The rover is
11 (12 as orchestrator), hover 12, tv 12, can 12, kitty 11. A held tool or carried item adds 2. Each extra part
that needs its own bone or its own material adds a draw: keep parts on the bone that already has them.

## Checks

- `cargo test -p apz-engine --release --lib`: the robot tests (`robots/tests.rs`) load every listed file, check the
  required bones, the clip evaluation, the validation paths, and the draw budget (as a count of meshes). The design
  audit (`robots/audit.rs`) runs on every species in its own tests. The eye checks are strict: an eye error
  (`eye-fit`, `eye-occluded`, `eye-pair`, `eye-overlap`, `eye-silhouette`) in any species fails the suite, in every
  pose (`eyes_are_strict_in_every_pose`). The rest of the audit is a report.
- `cd apps/web && npm run check`: the types, the build and the web tests. The web tests do not cover robots.
- `cd apps/web && node test/run.mjs`: the web tests alone.
- Look at it: `#/robot-lab?species=<id>` in the console (`npm run build` first, or the dev server).

## Style (the rules a species is judged against)

The audit (`crates/apz-engine/src/robots/audit.rs`, run by its tests) checks
the geometry; these rules say what it should look like. The numbers come from the 31 species (medians in
`scratchpad/robots/baseline.md`) and the best-looking species: rover, tv, kitty, ufo, cube, duck, bunny, lifty.
Where the two disagree, the rule wins and the species is fixed. Units are robot units (a species is about 1 high).

**Eyes (the face is the front of the head, symmetric about x = 0)**
- **Position**: eyes sit on the front face, symmetric about x = 0 (a mirror pair at ±x). Never on the side of the
  head toward +x. Spacing is 0.35–0.45 of head width (centre to centre; median 0.41). Height is 45–60% up the head
  (median 0.49). Eyes face forward, or outward by at most 12°.
- **Size**: socket radius `r` is 0.14–0.19 of head width (median 0.165). A pupil is 0.6–0.9 of `r`; its footprint
  stays inside the socket (`eye-pupil`).
- **Seating**: a socket is a flat disc, and the eye format has no rotation, so it faces +z. The rim lies flush with the
  head: the head surface is behind the disc at every rim point (the centre and eight points at `r`), plus about 0.005
  (`eye-fit`: a rim point with the surface in front of it is clipped). The centre may float off a curved head by up to
  50% of `r` (a flat disc sags onto a curve at its rim). Never sink a socket into the head: the disc is never behind
  the surface at any rim point.
- **Visibility** (`eye-occluded`, an error in the rest, walk and wave poses): from the front and the three-quarter
  camera at portrait distance (the lab's `one` line-up, `apps/web/src/views/robot-lab.ts`), the pupil must be the
  first thing hit, with no part of any bone in front of it, and at least four of five probes (the pupil centre and four
  rim points at 85% of `r`) must be clear. The rays run against the real triangles of every part, so a part that
  covers an eye names itself. A clear (`clear`) part does not hide. The rim stays on the head silhouette
  (`eye-silhouette`). Two eyes never overlap (`eye-pair`), and no face feature lies under a socket's footprint
  (`eye-overlap`).
- **Seat**: `seat_eyes` in `crates/apz-engine/src/robots/audit.rs` computes where a species' eyes go. It is pure: it
  returns the eyes and notes, and writes no file. It places each socket
  on the head front in the style's band (spacing 0.35–0.45 of head width, height 45–60%, kept where the eye already is
  inside it), tries the flattest spots in the band when the current one sags, and lifts the socket the least that leaves
  the pupil and rim visible in every pose. A mirror partner copies its twin's seat. A part that hides an eye, or a rim
  that sags past the float limit, is reported as blocked and the eye is left as it was: shrink or move the named part
  by hand, then run the tests again.
- **Style**: one shape per species, the same on both sides. A cyclops or a one-eyed sprout sets `"asym": true`.

**Face features (noses, beaks, cheeks, mouths, antennae, plates)**
- Each sits on the head surface: touching, not floating (`feature-float`), and at most 30% of its size buried
  (`feature-buried`). Place it along the surface normal at the feature, and use `rot` so its front faces outward.
- A feature that is not on the centre line needs an `"asym": true` flag, or a mirror partner (`symmetry`).
- A face plate is a `paper` disc on the head. It is a plate, not a socket: keep it under the eyes and centred
  (overlap with a plate is not counted, but the eyes must sit within its edge).

**Proportions (per family; median head-to-body height ratio, from the audit)**
- biped and waddle (penguin, duck, teapot, sprout): head/body 0.73.
- hop (owl, frog, lamp, bunny, astro): 0.91. Hoppers have larger heads.
- wheels and treads (can, lifty, rover, mole): 0.63–0.66. A wheeled or treaded body is tall and the head small.
- walk4 (kitty, turtle, pup): 1.00.
- floating (octo, jelly, ghost, oracle): no body. The head is the whole top, and the eyes are on it.
- Overall: head/body 0.83 and head/height 0.41 (median). A species more than 25% from the median is flagged
  (`proportion`). Explain it in the blurb or fix it.
- Eye radius relative to head width: 0.14–0.19 (median 0.165). Hovers and cyclops-style robots run larger (0.28–0.36)
  and need a reason.

**Limbs**
- Arms and legs: 0.08–0.12 thick (rbox width, owl and bunny arms are 0.1). A limb never sinks more than 0.03 into the
  body (`limb-through-body`, a warning in every pose, walk and wave included).
- Hands and claws touch the arm they belong to, within 0.02 (`attach`). A grip (`gripL`, `gripR`) sits at the end of
  its arm part.
- A held tool does not pierce the head or body by more than 0.01 (`tool`), in the one-hand and two-hand grips.

**Outline (the ink line)**
- Inked parts: any matte material except `ink` and `outline: false` parts. A part under 0.03 radius is not inked.
- Use `outline: false` for flat decals (a belly disc, a cheek, a face plate) and for small parts that would cost a
  draw call. Use the `ink` material for linework that is already ink.
- Ink coverage (area share of the surface carrying the line) is 0.54–0.72 for most species (median 0.63). The
  ladybug and cube have none or nearly all; decide per species and say why.

**Curved surfaces**
- Place a feature along the surface normal at its point of contact: the normal of a sphere is the direction from its
  centre. Do not put it at a fixed `z`.
- Use `rot` so a feature faces outward: a cone or cylinder pointing away from the head centre, a disc flat on the
  surface.
- Sink a socket or feature about 30% of its size into the surface, so it reads as seated, not stuck on.
