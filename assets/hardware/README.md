# Hardware models: authoring guide

The models under this folder are the machines of the Machines view (`exterior/`) and their insides
(`board/`), plus reusable pieces (`parts/`) and the shared colours (`palette.json`). Both renderers
read them: the web (`apps/web/src/views/floor3d/hardware.ts`) and the desktop. The format is
specified in `docs/design/hardware-plazas.md` ("Schema"). This guide is the working version.

## Files

| path | what it is |
|---|---|
| `exterior/<kind>.json` | the machine as seen in the Machines view (`view: "exterior"`) |
| `board/<kind>.json` | its inside, the plaza's board (`view: "board"`) |
| `exterior/apple-laptop.json`, `exterior/apple-desktop.json`, `board/apple-soc.json` | Apple Silicon family (`vendor: "apple"`) |
| `parts/<name>.json` | a reusable piece (`view: "part"`), placed with `"use": "parts/<name>"` |
| `palette.json` | named materials (`{ "schema": 1, "colors": { name: material } }`) |

Kinds: `workstation`, `laptop`, `edge`, `phone`, `server`.

## Adding a model

1. Write the JSON file in the right folder. Start from a placeholder of the same kind.
2. Register it in `apps/web/src/views/floor3d/hardware-index.ts`: one static import and one line in
   `MODELS` (the key is the path without `.json`, e.g. `"exterior/apple-laptop"`).
3. Run `bash -c 'cd /mnt/fast/AgentPlaza/apps/web && npm run check'`. The test suite validates every
   file in `MODELS` and fails when a file is missing from the index.
4. Open the lab (below) and look at it.

Which files a machine uses: `modelsFor(machine)` in `hardware.ts`. An Apple machine (`vendor: "apple"`)
uses the Apple models (`exterior/apple-laptop` for kind `laptop`, else `exterior/apple-desktop`;
`board/apple-soc`). Any other machine uses `exterior/<kind>` and `board/<kind>`.

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
- `repeat`: `{ "count": 4, "step": [0, 0, 1.2] }`. The count is a number (1 to 64) or the name of a numeric param, e.g. `"p_cores"`.
- `when`: shown only when the expression holds: `"gpus"`, `"!gpus"`, `"gpus >= 2"`, `"gpu_cores >= 30"`. Any lower-case param name works; the value comes from the build's params.
- `anim`: `spin` (`{axis, rps}`), `pulse` (`{period}`), `blink` (`{period, phase}`), `slide` (`{to, on: "enter"}`, `to` is the open position in the part's parent frame), `swing` (`{axis, to, on: "enter"}`, `to` in degrees). `slide` and `swing` follow the `open` state (0 to 1, eased).
- `tag`: `status` (the LED takes the machine's status colour), `activity` (blinks while agents work), `fan` (spins while online, faster under load), `screen` (glows while online).
- `label`: silkscreen text on the part's top: `{ "text": "CPU0", "size": 0.6, "color": "#e8eadf" }`.
- `children`: parts transformed by this one.

Geometry (`geo.type`): `box` (`w h d`), `rbox` (`w h d r`), `cyl` (`rt rb h`, optional `seg`), `cone` (`r h`), `sphere` (`r`), `torus` (`r tube`, optional `arc`; lies in the XY plane), `plane` (`w d`, flat, up), `ring` (`ri ro`, flat, up).

### Boards

- `slots`: `{ "id", "on": "<part id>", "type": "cpu|gpu|memory|storage|network|power|io|soc|modem|bmc|npu", "top": y, "rect": [x, z, w, d], "capacity": n }`.
- `traces`: `{ "path": [[x, z], ...], "y": 0.06, "width": 0.25, "from": "<part id>", "to": "<part id>" }`. Drawn as copper; current pulses travel along them.

### Palette and materials

`palette.json` holds the named materials. Each entry is a material (`color` is required). The status
LEDs (`led-ok`, `led-warn`, `led-off`, `led-crit`) must exist: the `status` tag uses them.

## Validation

`validate(model, name)` in `hardware.ts` checks the file: schema and view, unknown fields, geo types
and dimensions, colours, palette names, duplicate ids, `use` targets, `when` syntax, `repeat`
counts, animations, anchors, slots and traces (references to parts must resolve). It returns the
error messages, each prefixed with the file name and a path such as `parts[2].children[0].geo`.
The check suite runs it on every model. The lab lists the same errors in red.

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

The controls keep the URL in step, so a link to the view is the URL itself. Screenshots:

```
node <scratchpad>/preview/shot.mjs out.png "http://127.0.0.1:7700/#/hardware-lab?model=board/workstation" 8
```

The third argument is the wait in seconds before the capture.
