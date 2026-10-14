# Porting a web console screen to the desktop app

> **Status: the port is done.** The desktop app (`apps/desktop`, iced 0.14 + wgpu) has a screen
> module for each web console screen and is at parity with the web console (`apps/web`). The
> brief below is kept as the reference for the conventions when a screen is added or changed. The
> per-agent rules at the end were for the porting work and are historical.

The desktop app is at parity with the web console: every screen does what its web view does, in
the same design language (`apps/web/DESIGN.md`). Port the behaviour and the layout faithfully; do
not invent features.

## A screen module (`src/console/screens/<name>.rs`)

Keep these signatures (the screen table in `screens/mod.rs` calls them):

```rust
pub struct State { /* the screen's data, its error, what is selected/open */ }   // Default
pub enum Msg { Go(Id), /* your messages */ }                                      // Debug, Clone
pub fn go(msg: &Msg) -> Option<Id>                 // navigation this message asks for
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg>       // on open and every 4 s poll
pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg>
pub fn view<'a>(state: &'a State, ctx: &'a Ctx) -> Element<'a, Msg>
```

- Data: `ctx.client` (`src/console/client.rs`): `get::<T>(path)`, `post::<T>(path, &json)`,
  `patch`, `delete`, `call(method, path, Option<&json>) -> Result<Value, String>`. They block: wrap
  every call in `client::task(move || client.get::<Value>("/v1/…"), Msg::Loaded)` (clone the
  client into the closure). Deserialize into `serde_json::Value` or small local structs.
- The endpoints and JSON shapes are exactly what the web view uses: read its calls in
  `apps/web/src/api.ts` (paths and TypeScript types) and how the view reads the fields.
  A screen may also export a `subscription(ctx)` for its own timers or events (as `agents.rs` does).
- Navigation: `Msg::Go(Id::Agent(cid))` etc. (`screens::Id`).
- Polling: `console/mod.rs` sends a tick every 4 s, which refreshes the current screen
  (`screens::refresh`) and the shell; keep inputs, selection and scroll state across reloads.

## The design kit (`src/console/ui.rs`, `src/theme.rs`)

`ui::sheet(head, body)` is the page; `ui::view_head(title, actions)`; `ui::card(title, body)`
for sections; `ui::btn(label, icon, Kind::{Primary,Secondary,Ghost,Danger}, Some(msg))`;
`ui::badge(text, Tone)`; `ui::mono` (titles, labels, figures, ids), `ui::prose` (reading text, Inter at the prose size),
`ui::faint`, `ui::label` (small uppercase labels); `ui::icon(name, size, colour)` (the web's lucide
names, `src/console/icons.rs`); `ui::brand_logo(provider, size)`; `ui::empty(icon, title, sentence)`;
`ui::error_line`; `ui::loading`; `ui::markdown(source)` for anything agents write; `ui::hairline`.
Colours: `ui::INK`, `ui::INK_2`, `ui::INK_3`, `ui::SIGNAL`, `ui::RULE`, `ui::SHEET`, `ui::PAPER`
(check `grep -n "pub const" src/console/ui.rs src/theme.rs`). Square corners, 1px rules, the print
shadow on boxed things. Tables: a `column` of `row`s with `Length::FillPortion` columns, a mono
header row and hairlines. Long text wraps at ~72ch (`max_width(620)`).

Shared widgets: the model picker (`src/console/picker.rs`), the agent wizard
(`src/console/screens/wizard.rs`), the agent side panels (`src/console/screens/agent_side.rs`).

## Rules for porting agents (historical)

- Work only in the files your brief names. Keep every `pub` signature you were given.
- Build with `cargo check` (fast), not `cargo build`:
  `CARGO_TARGET_DIR=<your target dir> cargo check --manifest-path <worktree>/Cargo.toml -p agentplaza-desktop`.
  No warnings from your files. Tests are optional (pure helpers only).
- No `unwrap()` on data from the hub; show errors with `ui::error_line`.
- Never start the plaza or the app. Never touch the main checkout.
