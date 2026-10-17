# Machine models: the kit

The machines of the Machines view and the machine view are one model kit, in `kit/`: ten glTF binaries and their
textures. `tools/apz-kit` builds them from code and writes this folder; the engine (`crates/apz-engine/src/kit.rs`)
embeds and loads them on the web (wasm) and the desktop. The kit is never edited by hand; the graphics cards
(`gpus/`) are hand-written data.

## Files

| path | what it is |
|---|---|
| `kit/<model>.glb` | one machine: `hub`, `server`, `rack`, `gpu`, `tower`, `edge`, `mini`, `studio`, `laptop`, `phone` |
| `kit/grille.ktx2` | the perforated grille, RGBA8 with its mip chain, zstd-supercompressed |
| `kit/manifest.json` | each model's kind, size and triangle counts per level of detail |
| `gpus/specs.json`, `gpus/match.json` | the graphics cards: reference specs, and the rules that match a detected GPU to one (below) |

Each `.glb` has three roots, `lod0` (full), `lod1` (simple) and `lod2` (block). Its scene extras give `kind` and
`size` (`[w, h, d]`, 1 unit is about 10 cm; the base stands on y = 0 and the front faces +z).

## The contract

The engine finds parts by name; the full list is in `tools/apz-kit/src/lib.rs`.

- Materials are named (`graphite`, `aluminium`, `grille`, `glass`, `status`, `screen`, …). The `.glb` carries a
  neutral colour for each; the engine colours them per style (`crates/apz-engine/src/style.rs`: Ink, Blueprint and
  Night, each in a light and a dark theme).
- `status` is the light strip, coloured by the machine's state (working, idle, draining, offline). `fan` is a rotor,
  spun about its own z axis. Both run on the GPU from the fleet's tables.
- `slot:<n>` are the VM slot lights, `screen` and `plate` the surfaces the hosts paint, `anchor:label` and
  `anchor:port` where the name tag sits and where a cable ends.

Which model a machine draws as: `kit_model_of` in `crates/apz-engine/src/fleet.rs`. The hub is `hub`; a workstation
picks by its form and OS (`rack` for a VM, `laptop`, `server`, `mini` for a Mac, else `tower`); an Apple machine is a
`laptop`, a `studio` or a `mini`; a server or workstation with GPUs is `gpu`; the rest follow their kind.

## Changing a model

1. Edit the model in `tools/apz-kit/src/models/`.
2. Write the kit: `cargo run -p apz-kit -- build`. `cargo run -p apz-kit -- check` (and the test
   `the_committed_kit_is_up_to_date`) fails while the committed files differ from what the code writes.
3. `cargo test -p apz-engine --lib kit` parses every model and instantiates each level in every style.
4. Look at it in the lab.

## The lab

The hardware lab draws the kit on the machines' ground, in a style, at a level of detail. On the web:

```
#/hardware-lab?model=all&lod=0&style=ink&theme=dark&status=working&cam=iso
```

- `model`: a kit model id, or `all` for the whole line-up.
- `lod`: 0, 1 or 2.
- `style`: `ink`, `blueprint` or `night`; `theme`: `light` or `dark`.
- `status`: `working`, `idle`, `strained` or `offline`.
- `cam`: `iso`, `front`, `side`, `back` or `top`; `ortho=1` draws it with the orthographic camera, as the 2D views do.

On the desktop the same options are the `APZ_HW_*` variables (`APZ_SCREEN=HardwareLab`), listed in
`crates/apz-engine/src/hardware_lab.rs`.

## Graphics cards

`gpus/specs.json` holds one entry per reference card, module, baseboard, edge box or integrated GPU family (`{ "schema": 1, "specs": [...] }`). `gpus/match.json` maps what hardware detection reports to a card id (`{ "schema": 1, "rules": [...] }`). The types, the validator and the matcher are `crates/apz-engine/src/gpus.rs`. The source research is `docs/design/gpu-cards.md`.

Units are millimetres. The kit is built at 1 unit = 10 cm, so a 304 mm card is 3.04 units.

Each spec has `id`, `name`, `vendor` (`nvidia`, `amd`, `intel`, `apple`, or `generic` for the class fallbacks), `segment` (`consumer`, `workstation`, `datacenter`, `edge`, `laptop`, `integrated`), `form` (`pcie`, `sxm`, `oam`, `module`, `bga`, `integrated`), `reference` (true only for a manufacturer's own board), `size_mm` (`length`, `height`, `thickness`; `null` only for integrated parts), `slots`, `cooling` (`type`, `fans` with `d_mm`, `x_mm` from the bracket end and `side`, `fin_direction`), `shroud`, `backplate`, `power`, `bracket` (ports), `lighting`, plus `layout` (baseboards: `rows`, `cols`, `pitch_mm`), `die` (laptop BGA: die size and VRAM chips), `sources` and `approx`. `approx` lists the top-level fields that are estimates or unverified; a `null` size must be listed there. Fields the research could not confirm are left out or set generic, not invented.

Match rules are tried in order and the first match wins: `vendor` (`*` for any), `name_regex` (case-insensitive), optional `machine_kind` (`server`, `workstation`, `laptop`, `edge`, `phone`), optional `memory_mib` (`min`, `max`), `card`, and `fallback: true` for the generic rules. Every specific rule must come before every fallback; `validate_match` (in `gpus.rs`) checks this and that each `card` is a spec.

To add a card: add a spec, add its rule before the fallbacks, and add a case for it to the table in the test `match_card_agrees_with_the_ts_matcher` (`crates/apz-engine/src/gpus.rs`). `cargo test -p apz-engine --release --lib` checks every spec and rule (`embedded_files_parse_and_validate`).
