# Robots: 30 species, role tools, and a life on the board

Status: design (2026-10-13). Replaces the five hard-coded robot variants (`robot.ts` / `robot.rs`) with
data-driven species shared by the web console and the desktop app, and the floor's ad-hoc wander/chat code
with a behaviour engine. Builds on the board plazas (`hardware-plazas.md`): agents live in rooms on a
machine's chips and now also work *on* the chips.

## Goals

- **30 cute species**, each with its own silhouette, gait, idle quirks and personality, so a plaza of 40
  agents looks like a crowd of individuals, not clones.
- **Assigned at random when an agent is created, kept for life, balanced**: the hub deals species like a
  shuffled deck, so every species shows up a reasonable number of times.
- **They live their jobs**: what an agent is doing right now (its role, its state, its latest tool call,
  who it is talking to) decides where it goes, what it holds and what it does: typing at a desk, drafting
  at an easel, soldering a chip, probing memory with a multimeter, pouring flasks in a lab, carrying a
  crate to the release room, huddling with a reviewer, napping on a charging pad.
- **One source of truth**: species, clips, tools, props and behaviour tables are JSON under
  `assets/robots/`, read by both renderers. Parity is by data, not by duplicated hand code.

## Files

```
assets/robots/
  index.json          the lists (species, tools, props) in order; both apps and apid read it
  palette.json        fixed robot materials (graphite, brushed trim, glass, eye, light, …)
  clips.json          the shared animation clips (type, draw, solder, carry, nap, …)
  behaviour.json      roles → toolboxes and work, tool categories, activities, scoring weights
  species/<id>.json   one per species (30) + oracle.json (model agents, never dealt)
  tools/<id>.json     held tools and carried items
  props/<id>.json     station props placed in rooms (easel, lab bench, workbench, …)
```

Web: `apps/web/src/views/floor3d/robots/` (loader, rig, clips, engine) and `#/robot-lab`.
Desktop: `apps/desktop/src/console/floor3d/robots/` (the same modules, by the same names).
Hub: `crates/apid/src/looks.rs` (dealing and keeping species).

## Species format (`species/<id>.json`, schema 1)

Parts use the hardware part format exactly (`geo` types box, rbox, cyl, cone, sphere, torus, plane, ring,
shape; `at`, `rot` in degrees, `scale`, `repeat`, `children`, `outline`), so both apps reuse the
hardware geometry builders. A part hangs on a **bone** instead of a model.

```jsonc
{
  "schema": 1,
  "id": "owl",
  "name": "Hoot",                       // the species' nickname, shown in the agent card
  "blurb": "Sees everything twice. Swivels its head all the way round when it thinks.",
  "accent": "#ffb84d",                  // the species' own fixed colour (mat "accent")
  "height": 1.0,                        // robot units, before the floor's scale (about 1)
  "label_top": 1.12,                    // label anchor height above the root
  "locomotion": "hop",                  // see Locomotion
  "gait": { "speed": 0.9, "freq": 3.2, "bob": 0.12, "sway": 0.04, "lean": 0.1, "sideways": false },
  "bones": {                            // pivots: parent and offset; "root" is implicit
    "base":  { "parent": "root", "at": [0, 0, 0] },
    "body":  { "parent": "root", "at": [0, 0.1, 0] },
    "head":  { "parent": "body", "at": [0, 0.55, 0] },
    "armL":  { "parent": "body", "at": [-0.3, 0.45, 0] },
    "armR":  { "parent": "body", "at": [0.3, 0.45, 0] },
    "gripL": { "parent": "armL", "at": [0, -0.28, 0.02] },
    "gripR": { "parent": "armR", "at": [0, -0.28, 0.02] },
    "carry": { "parent": "body", "at": [0, 0.4, 0.32] },   // where a carried crate sits
    "footL": { "parent": "base", "at": [-0.12, 0, 0] },
    "footR": { "parent": "base", "at": [0.12, 0, 0] },
    "tail":  { "parent": "body", "at": [0, 0.2, -0.3] }    // any extra bones a species wants
  },
  "parts": [
    { "bone": "body", "geo": { "type": "sphere", "r": 0.34 }, "mat": "shell", "scale": [1, 1.15, 0.95] },
    { "bone": "head", "geo": { "type": "rbox", "w": 0.5, "h": 0.36, "d": 0.4, "r": 0.14 }, "mat": "shell" }
  ],
  "eyes": [                             // glowing eyes: glass socket + pupil, blink and state colour
    { "bone": "head", "at": [-0.11, 0.04, 0.2], "r": 0.085, "pupil": 0.05, "shape": "round" },
    { "bone": "head", "at": [0.11, 0.04, 0.2], "r": 0.085, "pupil": 0.05, "shape": "round" }
  ],
  "spin":  [{ "bone": "wheelL", "axis": "x", "walk": 8, "idle": 0 }],   // rad/s while walking / idle
  "feet":  ["footL", "footR"],          // step while walking (alternating)
  "floats": false,                      // hovers (bobs; sets down when offline)
  "personality": { "social": 3, "energy": 3, "focus": 9, "curiosity": 7, "play": 2, "tidy": 6 },
  "quirks": [{ "clip": "owl.swivel", "weight": 3 }, { "clip": "owl.slowblink", "weight": 2 }],
  "clips": { "owl.swivel": { "...": "species clips, same format as clips.json" } },
  "overrides": { "type": "owl.peck-type" } // replaces a shared clip for this species
}
```

**Required bones**: `base`, `body`, `head`, `armL`, `armR`, `gripL`, `gripR`, `carry`. A species without
arms (snail, jelly, ghost, ufo) still has the arm bones: tentacles, eye stalks, a tractor beam, sheet
"hands". A tool must look held in `gripR` for every species, and a crate must look carried at `carry`.

**Materials** (`mat`): `shell` (the agent's own hue), `shell2` (the same hue, darker), `panel` (the
room's hue), `paper` (white), `trim` (brushed metal), `ink` (graphite), `signal` (orange), `glass` (dark
lens), `eye` (state colour, glowing), `light` (signal glow), `thruster` (info, unlit), `accent` (the
species' accent), `clear` (translucent: domes, jelly bells), or an inline material as in the hardware
format. The highlight glow applies to `shell`, `shell2`, `paper`, `trim`, `ink`, `accent`.

**Eye shapes**: `round`, `bar` (a horizontal slit), `visor` (one wide band; one entry), `ring`, `heart`
(when happy: cheer, celebrate), `arc` (closed, smiling: napping). Pupils scale y to blink; colour follows
state (idle mint, working cyan, offline grey).

**Locomotion** (picks the walk clip and how the base moves): `treads`, `wheels`, `ball`, `unicycle`,
`hover`, `fly` (rises 0.4 while travelling), `float` (drifts, no feet), `hop` (arcs: `gait.bob` is the hop
height), `bounce` (always bouncing), `waddle` (sway + side-to-side), `walk2`, `walk4`, `walk6`,
`scuttle` (walk6 sideways), `slither` (slow, no bob), `tumble` (rolls over its edges by 90°).

## Clips (`clips.json` + species `clips`)

A clip is a set of procedural channels; both renderers evaluate it the same way.

```jsonc
"type": {
  "loop": true,
  "channels": [
    { "bone": "armL", "prop": "rot.x", "base": -0.9, "amp": 0.2, "freq": 11, "wave": "sin" },
    { "bone": "armR", "prop": "rot.x", "base": -0.9, "amp": 0.2, "freq": 11, "phase": 3.1416 },
    { "bone": "head", "prop": "rot.x", "base": 0.14, "amp": 0.05, "freq": 5 }
  ],
  "fx": [{ "kind": "keys", "at": "gripR" }]
}
```

- `prop`: `rot.x|y|z` (radians), `pos.x|y|z` (offset from the bone's rest), `scale` (uniform factor).
- `wave`: `sin` (default), `abs` (|sin|, bounces), `tri`, `saw`, `square`, `pulse` (`duty` 0..1), `hold`
  (just `base`). Value = `base + amp * wave(2π·freq·t + phase)`; `freq` in Hz.
- One-shot clips (`"loop": false, "dur": 1.6`) play once, then return to the activity's clip; channels may
  use `env: "in-out"` (a sine envelope over `dur`).
- Bones a clip doesn't name ease back to rest. Every change of clip blends: each channel moves toward its
  target with `k = min(1, dt·8)`, as today.
- `fx` (effects, small and cheap): `sparks` (solder), `steam`, `bubbles` (lab), `zzz` (nap), `notes`
  (music), `hearts` (cheer), `dots` (talk, the "· · ·"), `keys` (typing glints), `ink` (octopus), `glow`
  (a light cone or beam: lamp, ufo), `trail` (snail), `confetti` (celebrate). Each is a few sprites or
  meshes from a shared pool, at a bone or at the station.

Shared clips (at least): `idle`, `walk` (per locomotion family: `walk.treads`, `walk.hop`, …), `type`,
`write`, `draw`, `read`, `think`, `talk`, `listen`, `point`, `wave`, `cheer`, `celebrate`, `nap`,
`stretch`, `dance`, `sweep`, `carry`, `hold`, `solder`, `screw`, `hammer`, `probe`, `inspect` (magnifier
up), `pour`, `microscope`, `water` (watering can), `count` (coins), `stamp`, `conduct` (baton), `speech`
(megaphone/gavel), `spray` (extinguisher), `patrol` (flashlight sweep), `lay-cable`, `shelve`, `sticky`
(put a note on a board), `measure`, `highfive`, `sit`, `gaze`, `offline` (slumped, eyes dim).

## Tools and props

`tools/<id>.json`: `{ schema: 1, id, name, grip: "one" | "two" | "carry", parts: [...], tip: [x,y,z], fx? }`.
Parts in the hardware format with robot material names; `grip: one` hangs at `gripR`, `two` spans both
grips (anchored at `gripR`, aimed at `gripL`), `carry` sits at `carry`.

Tools (at least): baton, gavel, megaphone, scroll, blueprint, pencil, t-square, tape-measure, wrench,
screwdriver, soldering-iron, hammer, magnifier, multimeter, clipboard, red-pen, crate, flag, cable-spool,
antenna-wand, extinguisher, flashlight, padlock, book, book-stack, feather-duster, flask, test-tube,
notebook, sticky-notes, marker, coin-bag, calculator, abacus, ledger, stamp, seedling, watering-can,
dna-vial, coffee-mug, broom, ball.

`props/<id>.json`: `{ schema: 1, id, name, footprint: [w, d], parts: [...], spots: [{ at, face, clip }] }`,
station furniture with the spots a robot stands at and the clip it plays there: drafting-table, easel,
whiteboard, lab-bench (flasks, burner), microscope-desk, workbench (vise, parts tray), book-cart,
counter (coins, scale), podium, crate-stack, server-rack, alarm-post, guard-post, plant-bed (the
breeder's nursery: saplings that glow brighter as a job trains), meeting-table, charging-dock, bench.

## The 30 species

Personality is 0–9: **so**cial, **en**ergy, **fo**cus, **cu**riosity, **pl**ay, **ti**dy.

| # | id | name | look | moves | quirks | so en fo cu pl ti |
|---|---|---|---|---|---|---|
| 1 | rover | Treads | boxy body on tank treads, binocular eyes on a neck, claw hands (the original; always the orchestrator) | treads | binocular zoom (eyes telescope out), compacts into a cube for a moment | 5 5 7 6 3 8 |
| 2 | hover | Bubble | a round bubble on a glowing thruster | hover | barrel roll, thruster puff | 7 7 4 7 7 3 |
| 3 | tv | Telly | a CRT head on a ball wheel | ball | the screen flips to static, then a heart; antenna wiggle | 8 5 5 6 6 4 |
| 4 | can | Dome | a can with a glass dome head | wheels | the dome spins round; the lid pops | 4 4 8 4 3 7 |
| 5 | kitty | Whiskers | a cat with pointed ears and a tail | walk4 | big stretch, tail flick, ear twitch, curls up to nap | 5 5 5 8 8 2 |
| 6 | owl | Hoot | an egg body, huge round eyes, wing arms | hop | swivels its head all the way round, slow blink | 3 3 9 7 2 6 |
| 7 | crab | Pinch | a wide low shell on six legs, big claws | scuttle | claw clack, side-shuffle dance | 6 8 5 5 7 5 |
| 8 | octo | Inky | a dome head with four curling tentacles | float | a ripple down the tentacles, puffs an ink ring | 7 5 6 8 6 4 |
| 9 | bee | Buzz | a striped round drone on four rotors | fly | figure-eight loop, pollen sparkle | 8 9 3 6 8 3 |
| 10 | snail | Shelly | a spiral shell on a soft slug, eye stalks | slither | hides in its shell, leaves a glowing trail | 3 1 9 5 2 7 |
| 11 | penguin | Waddles | a tuxedo body, flippers | waddle | belly slide, flipper flap | 9 6 5 4 7 5 |
| 12 | frog | Ribbit | a squat body, big eyes on top | hop | tongue flick (catches a spark), throat-bubble croak | 5 8 4 7 8 3 |
| 13 | mole | Digger | a drill nose, goggles, treads | treads | drill spin, burrows and pops up | 2 7 8 6 3 6 |
| 14 | lamp | Lumen | a desk lamp hopping on its base, eyes on the shade | hop | shines its light on whoever it looks at, curious tilt | 6 7 6 9 6 4 |
| 15 | teapot | Earl | a round pot, a spout arm, a lid hat | waddle | lid pops with steam, pours a cup | 8 3 6 4 4 8 |
| 16 | jelly | Glim | a translucent bell with glowing tendrils | float | glow pulse down the bell, slow drift spin | 5 2 6 6 5 4 |
| 17 | bunny | Hops | long floppy ears, round tail | hop | ear flop, nose twitch, a happy binky jump | 7 9 3 7 9 3 |
| 18 | turtle | Shellby | a hex-plated shell on four stumpy legs | walk4 | tucks its head in, sunbathes under the atom | 4 2 8 3 2 8 |
| 19 | pup | Sprocket | a four-legged dog with a wagging antenna tail | walk4 | zoomies, sits and begs, fetches a ball | 9 9 3 7 9 2 |
| 20 | ufo | Saucy | a saucer with a glass cockpit, a tractor beam | hover | beams up a small thing and puts it down, wobble spin | 5 6 5 9 6 5 |
| 21 | cube | Qubit | a cube with a face screen | tumble | twists a face like a puzzle cube, stack-hop | 4 6 8 5 5 9 |
| 22 | duck | Quackers | a rubber duck on a hidden wheel | waddle | squeaks (squash and stretch), head bob | 8 6 4 5 8 4 |
| 23 | mushroom | Morel | a spotted cap on stubby legs | walk2 | puffs glowing spores, tips its cap | 5 3 6 6 5 6 |
| 24 | astro | Cosmo | a fishbowl helmet and a jetpack | hop (low-g) | jetpack boost, plants a tiny flag, floats upside down | 6 7 6 9 7 5 |
| 25 | ghost | Boo | a sheet with a wavy hem | float | peek-a-boo (fades out and back), giggle bounce | 6 5 4 7 8 3 |
| 26 | unicycle | Wobbly | a tall slim body on one wheel, arms out | unicycle | wobbles and catches itself, wheelie spin | 6 8 5 6 7 4 |
| 27 | ladybug | Dot | a spotted dome on six legs, wing covers | walk6 | wing covers open, a short flight, spots glow | 6 6 6 7 6 6 |
| 28 | pogo | Boing | a body on a spring | bounce | super bounce with a flip, squashes before a jump | 7 9 3 6 9 2 |
| 29 | lifty | Forky | a little forklift: mast, forks for arms | wheels | forks up and down, reverses with a beep light | 5 6 8 3 3 9 |
| 30 | sprout | Sprig | a flowerpot body, leaf antennae, a bud on top | waddle | leaves open toward the atom, waters itself, blooms | 6 4 6 6 5 7 |
| — | oracle | — | a faceted crystal on a plinth (model agents, never dealt) | none | pulses when invoked; data rings orbit it | — |

## Dealing species (the hub)

- `looks.rs` keeps `cid → { species, dealt_ms }`, persisted as the `looks` component.
- An agent gets a species when the hub first sees it (spawn, hello, or load without a look): among the
  dealable species (all but `oracle`), take those with the **fewest agents now** (agents that exist;
  retired agents free their species), and pick one of them at random. With 30 species and 40 agents,
  every species has one or two. The orchestrator is always `rover`, outside the count; a model agent is
  always `oracle`.
- `Agent` JSON gains `species`. `GET /v1/robots/species` returns `[{ id, name, count }]`. The owner may
  change one: `PUT /v1/agents/{cid}/species { species }` (an unknown or reserved id is a 400).
- The species list comes from `assets/robots/index.json` (`include_str!`); a test checks every listed
  file exists.
- Clients fall back to `species[cid % 30]` when the hub sends none (older hubs).

## What they do: the behaviour engine

A small utility AI per robot, the same rules in both apps (`behaviour.ts` / `behaviour.rs`) and its tables
in `behaviour.json`. It runs on the client from what the client already sees: agent status, the latest
tool call (the activity line), conversations (who messages whom), tasks and projects events, the board
layout. It decides an **activity** (a clip, a tool, a place, a duration, maybe a partner); the floor walks
the robot there (along the traces) and plays it.

### Places (stations)

- **desk**: the agent's home slot in its room (typing, writing, reading, thinking).
- **room props**: each room places the station props its role uses, on its free floor (`behaviour.json`
  `roles.<role>.props`), each with spots: an architect's room has a drafting table and an easel, a
  researcher's a lab bench and a microscope desk, a builder's a workbench, …
- **components**: spots around every slot of the board model (`hardware` slots: cpu, gpu, memory, storage,
  network, power, io, soc, npu, bmc, modem), at the component's height, on its edges, facing it. A spot
  belongs to the component's type, so jobs pick the right chips (a QA bot probes memory, an edge operator
  tunes the network chip, a breeder tends the GPU).
- **commons**: a ring around the atom with a meeting table and benches (stand-ups, gazing, chats).
- **docks**: charging pads at room edges (naps, offline).
- **anywhere**: a free point on the board or along a trace (wander, patrol, zoomies).

Every spot is reserved by one robot at a time; a busy spot is skipped.

### Activities

| activity | clip | tool | place | when |
|---|---|---|---|---|
| type | type | — | desk | code: write/edit/patch/exec |
| write | write | pencil / notebook | desk | docs, memory writes, posts |
| draw | draw | pencil / marker | drafting-table, easel | architect and planner work |
| sticky | sticky | sticky-notes | whiteboard | plans, tasks, assignments |
| read | read | book / clipboard | desk, book-cart | memory reads, reviews, research reading |
| research | pour → microscope | flask / test-tube | lab-bench, microscope-desk | web search, fetch, research |
| tinker | solder / screw / hammer | soldering-iron / screwdriver / wrench | component | builds, tests, exec on the machine |
| probe | probe / inspect | multimeter / magnifier | component | QA, audits, security checks |
| lay-cable | lay-cable | cable-spool | trace or network/io component | edge operations, deploys |
| deliver | carry | crate / scroll / book | walks to a recipient's room or the release room | share_file, publish, releases, handoffs |
| review | read + point | clipboard + red-pen | beside another agent's desk | verdicts, reviews |
| huddle | talk / listen | — | a table, or beside a partner | messages between agents, pairing |
| conduct | conduct | baton | commons (the atom) | the orchestrator while others work |
| speech | speech | megaphone / gavel | podium | council, motions, votes |
| count | count | coin-bag / abacus | counter | payments, treasury |
| tend | water | watering-can / seedling | plant-bed, gpu/npu component | breeders: datasets, jobs, training |
| respond | spray / run | extinguisher | the hottest component | incidents, failures |
| patrol | patrol | flashlight | traces, bmc/network components | security reviews, idle security bots |
| think | think | the role's first tool | desk | working, no tool call yet |
| wander, gaze, chat, nap, play, stretch, tidy, sit, quirk | … | ball / broom / mug | anywhere, commons, dock | idle |
| powered-down | offline | — | dock | offline |

### Choosing

- **Working** agents do work. The latest tool call's **category** (`behaviour.json` `tools`: tool name
  patterns → code, docs, recall, research, test, ship, talk, review, plan, money, train, govern) and the
  role pick the activity (`roles.<role>.work.<category>`, else `roles.<role>.work.default`). It holds for
  at least `min_s` (no flicker); a new category switches at the next beat. Variation within a category is
  weighted (a builder's `test` is 60 % tinker on a component, 40 % probe).
- **Idle** agents pick among idle activities by score: `base × trait fit × novelty × distance`.
  Trait fit: chat and visit scale with `social`, wander and play with `energy` and `play`, gaze and
  explore with `curiosity`, tidy with `tidy`, nap with `(9 - energy)` and how long since the last nap;
  quirks with their weights. Novelty halves the score of what it did last. The pick is weighted random
  (seeded per agent), so two of the same species still differ.
- **Social rules**: a chat needs a willing idle partner (a roll against the partner's `social`); robots
  remember who they chatted with and prefer friends; groups cap at 4. When agent A messages B, A walks
  to B and they huddle (both work). A reviewer reviewing B's work stands at B's desk with a clipboard.
- **Events**: a task done → its agent celebrates (hearts, confetti) and idle neighbours cheer; a project
  stand-up (project created or advanced) → its members gather at the commons and the orchestrator
  conducts; a failure → incident bots respond; a payment → a finance bot carries coins; a deliverable
  shared → a crate travels from the author to the requester.
- **Offline**: walks to its dock and powers down. **Held plaza** (STOP ALL): everyone idles; nobody works.

### Tools by role (`behaviour.json` `roles`)

| role | toolbox | work (default first) | room props |
|---|---|---|---|
| orchestrator | baton, clipboard | conduct, huddle, review, type | podium, meeting-table |
| mayor, mayor-office | gavel, megaphone, scroll, stamp | speech, write, stamp, huddle | podium, counter |
| architect | blueprint, pencil, t-square, tape-measure | draw, measure on components, type | drafting-table, easel |
| builder | wrench, screwdriver, soldering-iron, hammer | type, tinker, deliver | workbench |
| qa | multimeter, magnifier, clipboard | probe, tinker, type | workbench |
| reviewer | clipboard, red-pen | review, read, type | — (visits) |
| release | crate, flag | deliver, type, cheer | crate-stack |
| edge-operator | cable-spool, antenna-wand, wrench | lay-cable, tinker, type | server-rack |
| incident | extinguisher, flashlight | respond, type, patrol | alarm-post |
| security-reviewer | flashlight, padlock, magnifier | patrol, probe, read | guard-post |
| librarian | book-stack, book, feather-duster | shelve, read, deliver books | book-cart |
| researcher | flask, test-tube, notebook, magnifier | research, read, write | lab-bench, microscope-desk |
| planner | sticky-notes, marker, scroll | sticky, draw, huddle | whiteboard |
| finance | coin-bag, calculator, abacus | count, deliver coins, type | counter |
| auditor | ledger, magnifier, stamp | probe, read, stamp | counter |
| breeder | watering-can, seedling, dna-vial, flask | tend, research, type | plant-bed, lab-bench |
| (other) | coffee-mug, notebook | type, read, write | — |

Idle extras by trait: a ball (play ≥ 7), a broom (tidy ≥ 7), a coffee mug (focus ≥ 7).

## Rendering notes

- Species geometry is built once per species and shared by every robot of it; each robot has its own
  materials (shell hue, eye state). Outlines as today.
- Effects come from a shared pool; at most ~60 live sprites per plaza.
- The engine ticks every robot at most every 0.25 s (decisions staggered by cid); clips evaluate every
  frame. 100 robots must hold 60 fps on the board view.

## Phases

1. **Rig and lab (web)**: the loader, bones, clips, eyes, tools at grips, carry, effects; the five
   current designs ported to JSON so they look exactly as today; `#/robot-lab` (one species or a
   line-up, any clip, tool, carry, state). The floor keeps working unchanged.
2. **Content, in parallel**: species 6–30 (five per author), tools, props, `clips.json` complete;
   desktop loader with the same lab (`APZ_ROBOT_LAB`); the hub deals species.
3. **Behaviour**: the engine in both apps, stations on the board (room props, component spots,
   commons, docks), the floor plays it; agent card shows the species.
4. **Polish**: effects, sounds off, performance pass, screenshots in both themes, landing hero video.

## Verification

- Validation: every file in `index.json` parses and validates (web `npm run check`, a Rust test); every
  species has the required bones; every clip names existing bones; every tool named by `behaviour.json`
  exists.
- Lab screenshots of every species in both themes, in idle, walk, type, carry and its quirks.
- Engine unit tests: dealing balance (40 agents → every species 1–2), activity choice per role and tool
  category, spot reservation, social pairing caps, offline goes to a dock, held plaza → no work.
- The floor with 16 real agents and with `fake=60` agents at 60 fps.
