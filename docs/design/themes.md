# Themes

A theme sets how the consoles look: the shape and type of controls, every colour, and the background behind the
page. The same theme works in the web console and the desktop app. The rules live in one crate,
`crates/apz-theme`, and both consoles read its output. This file describes the parts, how they combine, where the
code is, and how the owner's choice is stored and applied.

## 1. What a theme is

A theme has three parts. Each one is chosen on its own, so any combination is valid.

| Part | What it sets | Type |
|---|---|---|
| Skin | Radii, row height, label style, shadows, how primary actions are drawn | `crates/apz-theme/src/skin.rs` |
| Palette | Every colour token, for light and dark | `crates/apz-theme/src/tokens.rs` |
| Background | The pattern and motion behind the page | `crates/apz-theme/src/background.rs` |

A theme file (`.apztheme.json`) saves one combination. The mode sits on top of it: **System**, **Light** or
**Dark**. System follows the operating system. A file with only a light palette resolves in light whatever the mode
is, and the reverse for a dark-only file (`resolve` in `crates/apz-theme/src/lib.rs`).

### Skins

Two skins are built in. Both use Geist for text and Geist Mono for code (`assets/fonts/`).

| Setting | Loom | Graphite |
|---|---|---|
| About | Warm paper, mono capital labels, ink for actions | Cool neutrals, sans labels, cobalt for actions |
| Text size | 13.5 px | 13.5 px |
| Radius, large radius | 8 px, 12 px | 8 px, 12 px |
| List row height | 32 px | 32 px |
| Section labels | Geist Mono, capitals, 11 px, weight 500, tracking 0.08 em | Geist, as written, 12 px, weight 500, no tracking |
| Primary action | Ink fill, sheet-coloured text (`action` = `ink`, `on-action` = `sheet`) | Signal fill, on-signal text (`action` = `signal-fill`, `on-action` = `on-signal`) |
| Card shadow | One light layer (dark: stronger) | None |
| Popover shadow | Two layers | One layer |
| Palette and background it names as default | `loom-clay`, `loom-paper` | `graphite-cobalt`, `graphite-grid` |

A skin never names a colour, with one exception: it says which token a primary action takes. The desktop reads the
skin with `apps/desktop/src/theme.rs` `skin()`. Density is a separate scale applied to the skin: Compact 0.875,
Default 1.0, Comfortable 1.125 (`Density::factor` in `crates/apz-theme/src/appearance.rs`).

### Built-in themes

Six themes ship as files in `apps/shared/themes/`. Each has a light and a dark palette.

| Theme | id | File |
|---|---|---|
| Loom · Clay (default) | `loom-clay` | `apps/shared/themes/loom-clay.apztheme.json` |
| Loom · Moss | `loom-moss` | `apps/shared/themes/loom-moss.apztheme.json` |
| Loom · Indigo | `loom-indigo` | `apps/shared/themes/loom-indigo.apztheme.json` |
| Graphite · Cobalt | `graphite-cobalt` | `apps/shared/themes/graphite-cobalt.apztheme.json` |
| Graphite · Violet | `graphite-violet` | `apps/shared/themes/graphite-violet.apztheme.json` |
| Graphite · Teal | `graphite-teal` | `apps/shared/themes/graphite-teal.apztheme.json` |

`DEFAULT_THEME` in `crates/apz-theme/src/builtin.rs` is `loom-clay`. `DEFAULT_SKIN` in `skin.rs` is `loom`. A new hub
starts in Loom · Clay with the System mode. The first-run wizard has a "look" step that offers Loom and Graphite:
`apps/desktop/src/console/onboard.rs` (the look step, around line 690) and `apps/web/src/ui/onboarding.ts`
(around line 165). Choosing one there saves it the same way a change in Appearance does.

## 2. The token list

`crates/apz-theme/src/tokens.rs` holds the list. It has 82 colour tokens in 11 groups. Each token has a name, a
group and a one-line description that the theme editor shows.

| Group | Tokens | Used for |
|---|---|---|
| Ground (7) | `paper`, `paper-side`, `sheet`, `sheet-2`, `hover`, `press`, `pattern` | The page, sidebar, cards, recessed areas, row states, background pattern |
| Lines (3) | `rule`, `rule-strong`, `rule-faint` | Hairlines, input borders, faint separators |
| Text (3) | `ink`, `ink-2`, `ink-3` | Body, secondary and meta text |
| Accent (5) | `signal`, `signal-fill`, `signal-ink`, `on-signal`, `signal-soft` | The owner's colour: marks, filled controls, links, tints |
| Action (2) | `action`, `on-action` | Primary buttons (set by the skin, see section 1) |
| States (10) | `ok`, `warn`, `crit`, `info`, `review`, each with a `-soft` tint | Finished, watching, failed, working, in review |
| Diff (6) | `add-bg`, `add-fg`, `add-bar`, `del-bg`, `del-fg`, `del-bar` | Added and removed lines, change bars |
| Code (7) | `syn-keyword`, `syn-string`, `syn-type`, `syn-number`, `syn-comment`, `syn-function`, `syn-variable` | Syntax colours |
| Terminal (18) | `term-bg`, `term-fg`, `ansi-0` to `ansi-15` | The terminal's ground, text and 16 ANSI colours |
| Data (18) | `chart-1` to `chart-8`, `chart-other`, `chart-grid`, `chart-axis`, `hue-0` to `hue-5`, `on-hue` | Chart series, grid and axes; project and agent hues |
| Overlay (3) | `toast-bg`, `toast-fg`, `scrim` | Toasts and the veil behind a dialog |

Names use lower case and `-`. The web reads each one as a CSS custom property with the same name (`--paper`,
`--ink-3`). The desktop reads them through `theme::palette()`, whose fields are the same names with `_` for `-`
(`ink_3`). Two tests keep both sides to the list:

- `crates/apz-theme/src/tokens.rs`, `names_are_unique_and_css_safe` and `checks_name_real_tokens`: names are unique,
  use only `a-z 0-9 -`, and every name a contrast rule uses is in the list.
- `apps/desktop/src/theme.rs`, `palette_fields_are_the_token_list`: the desktop's palette struct has exactly the
  fields of the list. A name the desktop reads that the list lacks fails the test, and so does a name the list has
  that the desktop does not read.

The web side has the same guard through the generated CSS (section 5) and `crates/apz-theme/tests/gen.rs`.

## 3. Resolving a theme

`crates/apz-theme/src/lib.rs` `resolve(file, skin, mode)` turns a file, a skin and a mode into a full set of
tokens. It runs five steps.

1. **Mode.** If the file has a palette for the mode, use it. Otherwise use light if the file has one, else dark.
2. **Given tokens.** Read the file's palette for that mode. Unknown names and values that are not colours are
   ignored here; `ThemeFile::problems` reports them when the file is loaded.
3. **Derive.** `crates/apz-theme/src/derive.rs` `derive` fills every token the palette does not give. Derivation
   is in OKLab and OKLCH, from tokens already known, so an imported theme with ten colours still gets all 82 in its
   own hues. Seeds `paper`, `ink` and `signal` come from the skin's default palette if the file lacks them
   (`builtin::seed`). Translucent given colours are composited onto `paper` first. Examples of the rules:

   | Token | Rule |
   |---|---|
   | `paper-side` | `paper` mixed 2 % toward `ink` |
   | `sheet` | Light: `paper` mixed 60 % toward white. Dark: `paper` mixed 3.5 % toward `ink` |
   | `hover`, `press`, `rule`, `rule-strong` | `paper` mixed 5.5 %, 9 %, 10 %, 18 % toward `ink` |
   | `ink-2`, `ink-3` | `ink` mixed 33 % and 46 % toward `paper` |
   | `signal-soft` | `paper` mixed 13 % (light) or 18 % (dark) toward `signal` |
   | `ok`, `warn`, `crit`, `info`, `review` | Fixed hues (152, 70, 25, 250, 295), chroma 0.13, lightness 0.50 on light and 0.76 on dark |
   | `on-signal` | White, or a dark ink at lightness 0.18, whichever contrasts more with `signal-fill` |
   | `syn-function` | `info` hue turned by +330° |
   | `ansi-9` to `ansi-14` | The base colour (`ansi-1` to `ansi-6`) moved 0.06 in lightness toward the bright end |
   | `scrim` | Black at alpha 0.32 (light) or 0.56 (dark) |

   Then `apply_skin` sets `action` and `on-action` from the skin. The palette cannot change them.
4. **Overrides.** The file's `overrides` for the mode are applied after derivation, composited over `paper` when
   translucent. They are how a theme adjusts a few tokens without giving the whole palette.
5. **Contrast pass.** `contrast_pass` (in `lib.rs`) runs the rules in `tokens::CHECKS` in order. Each rule says a
   foreground token must reach a minimum ratio on each ground it names. Text needs **4.5:1** (`TEXT`) and marks need
   **3:1** (`MARK`). A failing foreground is moved to the nearest colour that passes (`color::fix_contrast`). A token
   moved by an earlier rule is checked again by later rules that name it. The rules, in short:

   | Rule group | Minimum | Checked against |
   |---|---|---|
   | `ink` | 4.5 | paper, paper-side, sheet, sheet-2, hover, press |
   | `ink-2`, `ink-3` | 4.5 | paper, paper-side, sheet, sheet-2, hover |
   | `signal-ink` | 4.5 | paper, sheet, hover, signal-soft |
   | `signal` | 3 | paper, sheet |
   | `on-signal`, `on-action` | 4.5 | `signal-fill`, `action` |
   | `ok`, `warn`, `crit`, `info`, `review` | 4.5 | paper, sheet and their `-soft` tint |
   | `add-fg`, `del-fg` | 4.5 | their `-bg` |
   | `syn-*` | 4.5 | sheet, sheet-2 |
   | `term-fg` | 4.5 | term-bg |
   | `ansi-1` to `ansi-14` (the marks) | 3 | term-bg |
   | `chart-1` to `chart-6` | 3 | sheet |
   | `chart-axis` | 4.5 | sheet |
   | `hue-0` to `hue-5` | 3 | on-hue |
   | `toast-fg` | 4.5 | toast-bg |

   The pass returns a **report** (`Report`): `taken` (tokens from the file), `derived`, `adjusted` (each moved token
   with its old and new colour, the ground it failed on, the ratio it had and the minimum), and `failing` (checks
   that still fail after the move, which happens only when a colour cannot reach the ratio on all its grounds).
   `Report::summary` gives the line the importer and the editor show, for example
   "41 colours taken, 19 derived, 3 adjusted for contrast".

The built-in themes pass as written. The test `builtins_pass_contrast_with_every_skin` in
`crates/apz-theme/tests/gen.rs` resolves each built-in theme with each skin and both modes and fails if any check
is left failing. `fix_sources_on_request` in the same file covers the fix
path for imports.

The resolved result (`Resolved`) carries the id, name, skin, mode, tokens, background and report. Both consoles
apply that struct and nothing else.

## 4. The file format: `.apztheme.json`

Schema 1 (`SCHEMA` in `lib.rs`). `ThemeFile::parse` reads a file and fails with every problem it finds.

| Field | Required | Meaning |
|---|---|---|
| `schema` | yes | Must be `1`. Other values are refused. |
| `id` | yes | 1 to 64 characters of `a-z`, `0-9` and `-`. Built-in ids are reserved. |
| `name` | yes | 1 to 80 characters. |
| `about` | no | One line for the Appearance card. |
| `skin` | yes | `loom` or `graphite`. |
| `palette` | yes (light, dark or both) | `light` and/or `dark`: an object from token name to a colour text. |
| `background` | no | A preset id (`loom-paper`, `loom-linen`, `graphite-grid`, `graphite-blueprint`, `contours`) or a full background object. Default: the skin's preset. |
| `overrides` | no | Same shape as `palette`, applied after derivation (section 3, step 4). |
| `source` | no | Where it came from, for example `vscode: One Dark Pro`. Shown in Appearance. |

Colour text is what `Rgba::parse` accepts (hex in practice). Unknown token names and non-colour values are problems.

Example, a theme that changes three tokens and keeps the rest derived:

```json
{
  "schema": 1,
  "id": "my-clay",
  "name": "My clay",
  "about": "Clay orange on warm paper",
  "skin": "loom",
  "palette": {
    "light": { "paper": "#f6f4ee", "ink": "#1b1a17", "signal": "#d0602c" },
    "dark": { "paper": "#121110", "ink": "#ece8df", "signal": "#e8794a" }
  },
  "background": "loom-paper",
  "overrides": { "dark": { "rule": "#2a2824" } }
}
```

The six built-in files are full palettes with `background` set (`loom-clay.apztheme.json` is the reference: both
palettes list every token).

## 5. Generated files

Two files are generated from the built-in themes and committed:

| File | Read by | Contents |
|---|---|---|
| `apps/shared/themes.gen.json` | `apps/web/src/ui/theme.ts` (`BUILTINS`) | Tokens with their groups, skins, background presets, the built-in themes resolved for both modes, the default theme id |
| `apps/web/src/styles/themes.gen.css` | The web stylesheet | The font stacks (`FONT_STACK`, `MONO_STACK` in `generate.rs`), each skin's radii, row height, labels and shadows, and one block of token declarations per built-in theme and mode |

They are written by `crates/apz-theme/src/generate.rs` (`gen_json`, `gen_css`). The test
`crates/apz-theme/tests/gen.rs` compares each file with a fresh generation (`generated_json_is_current`,
`generated_css_is_current`) and fails when they differ. To regenerate after a change to a built-in file, a token,
a skin or a background:

```
APZ_REGEN=1 cargo test -p apz-theme --test gen
```

Commit the regenerated files with the change. `every_builtin_parses` also runs in the same test file.

## 6. Web runtime

| Piece | Path | Role |
|---|---|---|
| Boot | `apps/web/src/boot-theme.js` | Runs before the first paint. Reads the cached hub choice (`apz.appearance`) and the device override (`apz.appearance.device`), resolves the mode, and sets `data-skin`, `data-palette`, `data-mode` and `data-density` on the root. `data-density` is removed when the density is default. It also follows the system mode change. |
| Runtime | `apps/web/src/ui/theme.ts` | The owner's appearance: `startAppearance`, `setHub`, `setMode`, `setDeviceOverride`, `apply`, `current`, `resolvedMode`, `tokenValue`, and the theme switch control. `apply` paints the tokens, caches the hub document in `apz.appearance`, and dispatches `apz:appearance`, which the backdrop and other views listen to. |
| WebAssembly | `apps/web/src/theme/wasm.ts`, `crates/apz-theme-web/src/` (`lib.rs`, `bindings.rs`, `model.rs`) | Wraps the Rust crate compiled to WebAssembly. It exposes resolve, validate, import and contrast checks with typed shapes, and the backdrop size and points. |

The wasm module is loaded only when it is needed: for a saved or imported theme, for the theme editor, and for the
backdrop. The built-in themes need nothing beyond the generated CSS and JSON, so a page using a built-in theme does
not wait for the module. The cache key lets the next page load paint the right look before the hub answers.

## 7. Desktop runtime

`apps/desktop/src/theme.rs` holds the desktop side.

| Item | Role |
|---|---|
| `apply(appearance, saved)` | Resolves the choice and stores a `Look` (read back with `look()`, `appearance()`, `resolved()`). |
| `palette()`, `skin()`, `theme()` | The colours, the skin and the iced theme for the current look. |
| `is_light()`, `choose(light)`, `follow_system(mode)` | The mode, including the system mode. |
| Style functions (`paper`, `index`, `sheet`, `pop`, `margin`, `primary_button`, `secondary_button`, `input`, `pick_list`, `rule`, `notice`, `stamp`, …) | Each takes the theme and returns the style for one kind of surface. Controls never pick a colour directly. |
| `label_font()`, `label_text()` | Section labels as the skin sets them: capitals for Loom, as written for Graphite. |

Fonts: Geist (`SANS` and its weights 500, 600, 700 and italic) and Geist Mono (`MONO`, `MONO_MEDIUM`) are embedded by
the console (`console::run`). Geist is the default font of the app. Geist and Geist Mono are licensed under the SIL
Open Font License (`assets/fonts/Geist-LICENSE.txt`).

Type scale in `theme.rs`: page 22, section 15, item 13.5, prose 14.5 (line height 1.6), UI 13, meta 12, badge 11,
code 12.5 in Geist Mono.

## 8. The hub: storage and API

The hub keeps the owner's choice and saved themes, so every console and device sees one appearance. The code is
`crates/apid/src/appearance.rs`.

| Method and path | Effect |
|---|---|
| `GET /v1/appearance` | The document: the choice and the saved themes. |
| `PUT /v1/appearance` | Saves the choice. It is checked against the saved themes: an unknown theme is refused. |
| `PUT /v1/appearance/themes/{id}` | Saves a theme the owner made. Built-in ids are refused. |
| `DELETE /v1/appearance/themes/{id}` | Deletes a saved theme. A choice that used it goes back to the default. |

`Store::load(state_dir)` reads the document from the hub's state directory. Writes are checked (`check`) and
replace the document as a whole. There is one global choice per hub.

A device may override the hub's choice. The override is local and does not reach the hub.

| Console | Where the override is kept |
|---|---|
| Web | `localStorage`, key `apz.appearance.device` (`apps/web/src/ui/theme.ts`) |
| Desktop | The desktop settings file (`desktop.json`), key `appearance_device` (`apps/desktop/src/console/screens/appearance.rs`) |

When the override is set, it wins on that device until it is cleared; then the hub's choice shows again.

## 9. The Appearance screen, the editor, import and export

**Appearance** lists the built-in and saved themes, the skin, the mode, the density, the background and the device
override. The desktop screen is `apps/desktop/src/console/screens/appearance.rs`.

**The theme editor** edits one theme for one mode. Each token row shows its colour, its group and its one-line
description, and a contrast badge for each rule it takes part in. A failing badge names the ratio and the minimum.
**Fix** runs the contrast pass on the current palette and writes back the moved colours, with the same report as
section 3. The desktop editor is `apps/desktop/src/console/screens/theme_editor.rs`.

**Export and import** use the `.apztheme.json` file (section 4). An imported file is validated before it is saved;
a file with problems is refused with each problem listed.

**VS Code import** reads a VS Code colour theme and turns it into a theme file. The code is
`crates/apz-theme/src/vscode.rs`, and its tests are `crates/apz-theme/tests/vscode.rs`. The flow:

1. The theme file is read as JSON with comments (`//` and `/* */`) and trailing commas allowed (`parse_jsonc`).
   Text inside strings is left alone.
2. Its `include` chain is followed. Each file is read through a lookup supplied by the caller, and the child theme
   overrides its parents. The chain is refused when it has a cycle, a missing file, or more than 8 files
   (`MAX_INCLUDE_DEPTH`).
3. The mode comes from `type`, or `uiTheme` if `type` is absent. `light`, `hcLight`, `vs` and `hc-light` give light;
   every other name gives dark.
4. The page colour is `editor.background`. Without it, the default is `#1e1e1e` for dark and white for light.
5. Each AgentPlaza token takes the first VS Code key that parses. Colours with alpha are composited over the page,
   so the stored palette is opaque.
6. Tokens with no key are left out. `resolve` derives them.
7. The file's name is the theme's name, its id is `vscode-` and a slug of the name, and its `source` is
   `vscode: <name>`. The result is checked with `ThemeFile::problems` like any other file.

The key mapping, copied from `vscode.rs`:

| Token | VS Code keys (first one present wins) |
|---|---|
| `paper` | `editor.background` (set separately, with the default above) |
| `paper-side` | `sideBar.background`, `activityBar.background` |
| `sheet` | `editorWidget.background`, `dropdown.background`, `input.background` |
| `hover` | `list.hoverBackground` |
| `press` | `list.activeSelectionBackground` |
| `rule` | `panel.border`, `sideBar.border`, `editorGroup.border`, `widget.border` |
| `ink` | `editor.foreground`, `foreground` |
| `ink-2` | `sideBar.foreground`, `descriptionForeground` |
| `ink-3` | `disabledForeground`, `editorLineNumber.foreground` |
| `signal` | `focusBorder`, `button.background`, `textLink.foreground` |
| `signal-fill` | `button.background` |
| `on-signal` | `button.foreground` |
| `signal-ink` | `textLink.foreground` |
| `ok` | `editorGutter.addedBackground`, `terminal.ansiGreen` |
| `warn` | `editorWarning.foreground`, `terminal.ansiYellow` |
| `crit` | `editorError.foreground`, `terminal.ansiRed` |
| `info` | `editorInfo.foreground`, `terminal.ansiBlue` |
| `add-bg` | `diffEditor.insertedTextBackground` |
| `del-bg` | `diffEditor.removedTextBackground` |
| `term-bg` | `terminal.background` (falls back to the page) |
| `term-fg` | `terminal.foreground` |
| `ansi-0` to `ansi-15` | `terminal.ansiBlack`, `ansiRed`, `ansiGreen`, `ansiYellow`, `ansiBlue`, `ansiMagenta`, `ansiCyan`, `ansiWhite`, then the `ansiBright…` eight, in that order |
| `syn-keyword` | token scopes `keyword`, `storage` |
| `syn-string` | `string` |
| `syn-type` | `entity.name.type`, `support.type`, `storage.type` |
| `syn-number` | `constant.numeric`, `constant` |
| `syn-comment` | `comment` |
| `syn-function` | `entity.name.function`, `support.function` |
| `syn-variable` | `variable` |
| `syn-*` (override) | `semanticTokenColors` keys `keyword`, `string`, `type`, `number`, `comment`, `function`, `variable`, which win over the scopes |
| `chart-1` | the `signal` colour |
| `chart-2` to `chart-8` | ANSI indices 4, 2, 5, 3, 1, 6, 12 (blue, green, magenta, yellow, red, cyan, bright blue) |

A token scope matches when it equals the key or is more specific under it (`keyword.control` matches `keyword`). The
first rule that names the scope and has a foreground colour wins.

**`.vsix` import.** A `.vsix` is the package VS Code extensions ship in. `crates/apz-theme/src/vsix.rs` reads the
theme files inside it and passes each one through the same import. Tests are in `crates/apz-theme/tests/vsix.rs`.

**The report.** The import returns the file and the resolved report. The line shown to the owner has the form
"N colours taken, N derived, N adjusted for contrast" (section 3). `taken` counts the colours the VS Code theme
supplied; `adjusted` counts the tokens the contrast pass moved, so the owner can see how far the import was changed.

## 10. Backgrounds

A background is a pattern, a motion, a set of colours and an optional ornament. Presets are in
`crates/apz-theme/src/background.rs` (`presets()`, `preset_by_id()`). Each preset can be opened in the editor and
changed. Colours are palette token names, or hex.

| Id | Name | Pattern | Motion | Ornament |
|---|---|---|---|---|
| `plain` | Plain (default) | None | Still | None |
| `loom-paper` | Paper | Dots | Slow | Threads |
| `loom-linen` | Linen | Weave | Still | None |
| `graphite-grid` | Grid | Grid | Slow | Orbits |
| `graphite-blueprint` | Blueprint | Blueprint | Still | None |
| `contours` | Contours | Contours | Slow | None |

Patterns are `None`, `Dots`, `Weave`, `Grid`, `Blueprint` and `Contours`. Ornaments are `None`, `Threads` and
`Orbits` (`crates/apz-theme/src/backdrop.rs`).

**What a background draws.** The pattern is laid over the ground in its colour: Loom's Paper draws dots, and
Graphite's Grid draws a grid that fades out from the top of the window. Up to three colour fields sit behind it as
soft discs, and with `motion: slow` each one drifts to a far place and back over 46 to 73 seconds. The
pattern lights under the pointer when `pointer` is set. Film grain of up to 0.08 is re-seeded twelve times a second,
inverted on dark grounds. `strength` and `strength_dark` set how strongly the fields show in each mode, which is how
the Loom and Graphite skins keep their own atmospheres. The skins default to Paper (Loom) and Grid (Graphite); the
owner can pick another preset or edit one.

**The shader.** One shader draws every background: `crates/apz-theme/src/backdrop.wgsl`. Its inputs are built by
`uniforms` in `crates/apz-theme/src/backdrop.rs` (a 40-float block, `Uniforms::floats`). The same maths exists on
the CPU as `render_cpu` in that file, so a still frame can be drawn without a GPU. `animates(bg)` says whether a
background moves at all.

**Desktop layer.** `apps/desktop/src/console/backdrop.rs` draws the background behind the console. Its subscription
ticks every `1000 / MAX_FPS` milliseconds only when the background moves. The engine's copy is
`crates/apz-engine/src/backdrop.rs`, which re-exports the same items for the desktop.

**Web canvas.** `apps/web/src/view/backdrop.ts` (`startBackdrop`) draws in a fixed canvas behind the shell, in this
order of preference:

1. WebGPU, with `backdrop.wgsl` (`createWebGPU`).
2. WebGL2, with the GLSL that naga writes from the same shader: `backdrop.vert.glsl` and `backdrop.frag.glsl`. The
   build step `apz-pack web-backdrop` writes them. The uniform block is bound to binding point 0.
3. CSS gradients that draw the same pattern, when neither is available.

The canvas follows the `apz:appearance` event and the window size.

**Budgets.** The most frames a moving background is drawn in one second is 30 (`MAX_FPS` in
`crates/apz-theme/src/backdrop.rs`, the same constant in `backdrop.ts`). A background that does not move is drawn
once after each change, not every frame. The web canvas stops while the page is hidden (`document.hidden`) and
restarts when it is shown. With the reduced-motion preference (`prefers-reduced-motion: reduce`) it draws one still
frame at time 0 and does not animate.

## 11. Fonts

| Font | Use | Licence |
|---|---|---|
| Geist | Interface and reading text on both consoles | SIL Open Font License, `assets/fonts/Geist-LICENSE.txt` |
| Geist Mono | Code, and Loom's section labels | SIL Open Font License, `assets/fonts/Geist-LICENSE.txt` |

The web stylesheet names them in `FONT_STACK` and `MONO_STACK` (`crates/apz-theme/src/generate.rs`), with system
fallbacks. The desktop embeds them (section 7).

## Where the code is

| Area | Paths |
|---|---|
| Crate | `crates/apz-theme/src/` (`lib.rs`, `tokens.rs`, `derive.rs`, `color.rs`, `skin.rs`, `background.rs`, `builtin.rs`, `appearance.rs`, `generate.rs`, `vscode.rs`, `vsix.rs`, `backdrop.rs`, `backdrop.wgsl`) |
| Crate tests | `crates/apz-theme/tests/gen.rs`, `vscode.rs`, `vsix.rs` |
| WebAssembly | `crates/apz-theme-web/src/` |
| Built-in themes | `apps/shared/themes/*.apztheme.json` |
| Generated | `apps/shared/themes.gen.json`, `apps/web/src/styles/themes.gen.css` |
| Web | `apps/web/src/boot-theme.js`, `apps/web/src/ui/theme.ts`, `apps/web/src/theme/wasm.ts`, `apps/web/src/view/backdrop.ts`, `apps/web/src/ui/onboarding.ts` |
| Desktop | `apps/desktop/src/theme.rs`, `apps/desktop/src/console/backdrop.rs`, `apps/desktop/src/console/screens/appearance.rs`, `apps/desktop/src/console/screens/theme_editor.rs`, `apps/desktop/src/console/onboard.rs` |
| Engine | `crates/apz-engine/src/backdrop.rs` |
| Hub | `crates/apid/src/appearance.rs` |
| Fonts | `assets/fonts/` (Geist, Geist Mono, licence) |
