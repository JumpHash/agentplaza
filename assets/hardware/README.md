# Hardware models: authoring guide

The models under this folder are the machines of the Machines view (`exterior/`) and their insides
(`board/`), plus reusable pieces (`parts/`) and the shared colours (`palette.json`). The Rust engine
(`crates/apz-engine/src/hardware.rs`) draws them on the web (wasm) and the desktop, and `apz-board` reads them for
the layout. The format is
specified in `docs/design/hardware-plazas.md` ("Schema"). This guide is the working version.

## Files

| path | what it is |
|---|---|
| `exterior/<kind>.json` | the machine as seen in the Machines view (`view: "exterior"`) |
| `board/<kind>.json` | its inside, the plaza's board (`view: "board"`) |
| `exterior/apple-laptop.json`, `exterior/apple-desktop.json`, `board/apple-soc.json` | Apple Silicon family (`vendor: "apple"`) |
| `parts/<name>.json` | a reusable piece (`view: "part"`), placed with `"use": "parts/<name>"` |
| `workstation/<name>.json` | a workstation (a station the agents operate) as an exterior, by OS family and form: `mac-mini`, `windows-laptop`, `linux-tower`, `vm-rack` (`hardware::station_model`). Key: `workstation/mac-mini` |
| `palette.json` | named materials (`{ "schema": 1, "colors": { name: material } }`) |

Kinds: `workstation`, `laptop`, `edge`, `phone`, `server`.

A workstation's model reads the build param `screen_state` (0 dark: offline, 1 dim: up with no active lease, 2 lit:
a lease is active). Its screen is three parts, one per state, each shown by `"when": "screen_state == N"`.

## Adding a model

1. Write the JSON file in the right folder. Start from a placeholder of the same kind.
2. Nothing to register: the build embeds every file under this folder (`crates/apz-engine/build.rs` lists them; the key
   is the path without `.json`, e.g. `"exterior/apple-laptop"`).
3. Run `cargo test -p apz-engine -p apz-board --release --lib`: the tests validate and build every file.
4. Open the lab (below) and look at it.

Which files a machine uses: `models_for(kind, vendor)` in `crates/apz-engine/src/hardware.rs`. An Apple machine
(`vendor: "apple"`) uses the Apple models (`exterior/apple-laptop` for kind `laptop`, else `exterior/apple-desktop`;
`board/apple-soc`). Any other machine uses `exterior/<kind>` and `board/<kind>`, and an unknown kind takes the
workstation. A station's exterior is `station_model(os, form)` (one of the `workstation/` files).

## The file

```jsonc
{
  "schema": 1,
  "kind": "workstation",
  "view": "exterior",                 // "exterior" | "board" | "part"
  "size": [4, 6, 4],                  // bounding box, used to frame the camera
  "params": { "gpus": 0 },            // defaults for the inputs `when` and `repeat` read
  "parts": [ ... ],
  "anchors": {
    "camera": { "at": [6, 5, 6], "look": [0, 3, 0] },  // exterior: the ENTER fly-in; board: the default view (required)
    "label": [0, 7, 0],               // exterior: where the name tag sits
    "port": [0, 1, -2],               // exterior: where cables attach
    "open": "side_panel",             // exterior: the part that slides/swings away on ENTER
    "atom": [0, 3, 0]                 // the plaza's atom (required on boards: over the main processor)
  },
  "slots": [ ... ],                   // board only: surfaces rooms are built on
  "traces": [ ... ]                   // board only: copper paths
}
```

Units: one unit is about one robot's width. Y is up, the origin is the centre of the footprint on
the ground. Exteriors are 6 to 14 units tall, boards 40 to 80 units across. Angles are degrees.

### Parts

Each part has `geo` (or `use`, not both), and optionally:

- `id`: unique in the file. Referenced by `anchors`, `slots`, `traces`.
- `at`, `rot` (degrees), `scale` (a number or `[sx, sy, sz]`).
- `mat`: a palette name (`"pcb"`) or an inline material `{ "color": "#1f6b3a", "emissive": "#000", "glow": 1.5, "opacity": 0.4, "additive": false, "flat": false }`.
- `outline`: the ink outline (default on for solids, off for `plane`/`ring` decals).
- `repeat`: `{ "count": 4, "step": [0, 0, 1.2] }`. Copy `i` sits at `step * i`. The count is a number (1 to 64), the name of a numeric param (`"p_cores"`), or `"<param>/<n>"` for the param over `n`, rounded up (`"p_cores/4"`: 10 cores give 3 copies). Add `"wrap": w` and `"row": [dx, dy, dz]` to lay the copies in rows of `w`: copy `i` sits at `step * (i % w) + row * floor(i / w)`. So `{ "count": 10, "wrap": 4, "step": [1, 0, 0], "row": [0, 0, 1] }` makes rows of four, then two. `wrap` is a whole number 1 or more; `row` is required with `wrap` and needs it.
- `when`: shown only when the expression holds: `"gpus"`, `"!gpus"`, `"gpus >= 2"`, `"gpu_cores >= 30"`. Any lower-case param name works; the value comes from the build's params.
- `anim`: `spin` (`{axis, rps}`), `pulse` (`{period}`), `blink` (`{period, phase}`), `slide` (`{to, on: "enter"}`, `to` is the open position in the part's parent frame), `swing` (`{axis, to, on: "enter"}`, `to` in degrees). `slide` and `swing` follow the `open` state (0 to 1, eased).
- `tag`: `status` (the LED takes the machine's status colour), `activity` (blinks while agents work), `fan` (spins while online, faster under load), `screen` (glows while online).
- `label`: silkscreen text on the part's top: `{ "text": "CPU0", "size": 0.6, "color": "#e8eadf" }`.
- `detail`: `1` (medium) or `2` (fine). The part is shown only while the camera is near: detail 1 within 160 units of the part's box, detail 2 within 90 (in the model's frame, with a 5% band either way). Use it for the small things (SMD parts, pins, screws, fins) so a far camera skips them. Set it on a group and every part under it inherits it.
- `cutaway`: `true` on a tall part that hides what is behind it (a GPU card over the board). While it stands between the camera and the focus point (the board's atom, or the orbit target) it fades to a quarter opacity, and back.
- `attached`: `true` for a solid that is fixed by something other than resting on what is under it: a fan on its shroud, a cable, a glow, a light. The grounding check (below) does not test its bottom. Use it only for those; a resting part that floats is moved.
- `children`: parts transformed by this one.

Geometry (`geo.type`): `box` (`w h d`), `rbox` (`w h d r`), `cyl` (`rt rb h`, optional `seg`), `cone` (`r h`), `sphere` (`r`), `torus` (`r tube`, optional `arc`; lies in the XY plane), `plane` (`w d`, flat, up), `ring` (`ri ro`, flat, up).

### Boards

- `slots`: `{ "id", "on": "<part id>", "type": "cpu|gpu|memory|storage|network|power|io|soc|modem|bmc|npu", "top": y, "rect": [x, z, w, d], "capacity": n }`.
- `traces`: `{ "path": [[x, z], ...], "y": 0.06, "width": 0.25, "from": "<part id>", "to": "<part id>" }`. Drawn as copper; current pulses travel along them.

### Palette and materials

`palette.json` holds the named materials. Each entry is a material (`color` is required). The status
LEDs (`led-ok`, `led-warn`, `led-off`, `led-crit`) must exist: the `status` tag uses them.

## Grounding

Every solid rests on what is directly under it. The rule, as the audit checks it (the grounding audit in `crates/apz-engine/src/world/ground_audit.rs`):

- The bottom of a solid is at the top of the highest solid under it, within 0.02, at its centre and four inset corners of its bottom face. A bottom more than 0.02 above that is **floating**.
- A solid whose bottom lies inside a solid beneath it, overlapping by more than 0.02 on every axis, is **sinking**. So is a bottom below the ground (y = 0).
- Exempt: parts with `"attached": true`, flat `plane` and `ring` decals (silkscreen, pads, labels), the copper traces, additive glows. None of these holds anything up.
- Boards: a slot's `top` is the top of its `on` part plus the plate thickness, 0.2 (`PLATE` in `crates/apz-engine/src/world/slab.rs`). The plate's underside rests on the highest solid under it; where it stands over lower ground, a column (cells of 0.5 units) goes down to what is under. A slot top is never lower than its part's top.
- Check it: `cargo test -p apz-engine --release --lib shared_layout_seats_every_agent_on_every_board_kind` seats every agent on every board kind at 1, 4, 8 and 20 agents (the test runs in release only). The live board can be audited from the console with `window.__apzGroundAudit()` (the floor's board, its layout and its rooms).

Every fix is at the source: move the part in its board file, or set `attached` where it really is attached. Do not lower a slot top to hide a plate.

## Validation

`validate(model, name)` in `crates/apz-engine/src/hardware.rs` checks the file: schema and view, unknown fields, geo types
and dimensions, colours, palette names, duplicate ids, `use` targets, `when` syntax, `repeat`
counts, animations, anchors, slots and traces (references to parts must resolve). It returns the
error messages, each prefixed with the file name and a path such as `parts[2].children[0].geo`.
The test `every_embedded_model_parses_and_validates` runs it on every model. The lab's readout lists the same errors in red.

## The lab

Open the lab at `#/hardware-lab` (it is not in the sidebar). Query parameters, all optional:

```
#/hardware-lab?model=exterior/workstation&gpus=1&status=online&working=0&open=0&traces=1&cam=iso
```

- `model`: a key of `MODELS`, e.g. `board/apple-soc`.
- `gpus`: 0, 1 or 2 (the `gpus` param).
- `status`: `online`, `draining`, `offline`, `revoked`.
- `working`: 0 to 6 (agents working; drives `activity` tags and the current pulses).
- `open`: 0 to 1 (the ENTER progress for `slide` and `swing`).
- `traces`: 1 or 0 (boards: show the copper and pulses).
- `cam`: `iso` (default), `top`, `front`, `side`, or `anchor` (the file's `anchors.camera`). The camera frames the model's `size`.
- Any other key whose name is a param name (lower-case, digits, `_`) and whose value is a number is a model param, and overrides the model's `params` default: `&p_cores=16&e_cores=8&gpu_cores=76&studio=1`. Keys the lab uses itself (`model`, `gpus`, `status`, `working`, `open`, `traces`, `cam`) cannot be params.

The model's params (except `gpus`, which has its buttons) show as number inputs in the controls, and changing one rebuilds the model. Changing the model clears the param overrides, since each model has its own defaults.

The controls keep the URL in step, so a link to the view is the URL itself. Screenshots:

```
node <scratchpad>/preview/shot.mjs out.png "http://127.0.0.1:7700/#/hardware-lab?model=board/workstation" 8
```

The third argument is the wait in seconds before the capture.

## Graphics cards

`gpus/specs.json` holds one entry per reference card, module, baseboard, edge box or integrated GPU family (`{ "schema": 1, "specs": [...] }`). `gpus/match.json` maps what hardware detection reports to a card id (`{ "schema": 1, "rules": [...] }`). The types, the validator and the matcher are `crates/apz-engine/src/gpus.rs`. The source research is `docs/design/gpu-cards.md`.

Units are millimetres. A GPU model is built at 1 unit = 10 cm (the owner's brief), so 304 mm is 3.0 units. The rest of this folder uses one unit as about one robot's width, so GPU parts will not match that scale until the modeller resolves it (research doc, section 0).

Each spec has `id`, `name`, `vendor` (`nvidia`, `amd`, `intel`, `apple`, or `generic` for the class fallbacks), `segment` (`consumer`, `workstation`, `datacenter`, `edge`, `laptop`, `integrated`), `form` (`pcie`, `sxm`, `oam`, `module`, `bga`, `integrated`), `reference` (true only for a manufacturer's own board), `size_mm` (`length`, `height`, `thickness`; `null` only for integrated parts), `slots`, `cooling` (`type`, `fans` with `d_mm`, `x_mm` from the bracket end and `side`, `fin_direction`), `shroud`, `backplate`, `power`, `bracket` (ports), `lighting`, plus `layout` (baseboards: `rows`, `cols`, `pitch_mm`), `die` (laptop BGA: die size and VRAM chips), `sources` and `approx`. `approx` lists the top-level fields that are estimates or unverified; a `null` size must be listed there. Fields the research could not confirm are left out or set generic, not invented.

Match rules are tried in order and the first match wins: `vendor` (`*` for any), `name_regex` (case-insensitive), optional `machine_kind` (`server`, `workstation`, `laptop`, `edge`, `phone`), optional `memory_mib` (`min`, `max`), `card`, and `fallback: true` for the generic rules. Every specific rule must come before every fallback; `validate_match` (in `gpus.rs`) checks this and that each `card` is a spec.

To add a card: add a spec, add its rule before the fallbacks, and add a case for it to the table in the test `match_card_agrees_with_the_ts_matcher` (`crates/apz-engine/src/gpus.rs`). `cargo test -p apz-engine --release --lib` checks every spec and rule (`embedded_files_parse_and_validate`).
