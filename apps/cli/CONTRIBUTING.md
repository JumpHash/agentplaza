# Writing `agentplaza` commands

`agentplaza` (apps/cli) does everything the web console and the desktop app do, over the same hub
API. One module per console area in `src/cmd/<area>.rs`; `src/cmd/mod.rs` lists them and `src/main.rs`
dispatches to them.

## A command module

```rust
#[derive(clap::Subcommand, Debug)]
pub enum Cmd { /* one variant per action, documented: the doc comment is the help text */ }
pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String>
```

Keep those two names and that signature (`main.rs` calls them). Everything else in the file is
yours.

- **The API**: exactly what the web console calls — read `apps/web/src/api.ts` (paths, bodies, the
  TypeScript types of the answers) and the matching view in `apps/web/src/views/` for which fields
  matter and how actions are confirmed. Never invent endpoints.
- **The client** (`src/ctx.rs`): `ctx.client.get(path)`, `.post(path, &json!({…}))`, `.put`,
  `.patch`, `.delete` → `Result<serde_json::Value, String>` (a non-2xx answer is `Err` with the
  hub's message). Encode ids in paths with `ctx::seg(id)`.
- **Output**: `ctx.out(&value, |v| { …human output… })` — with `--json` it prints the hub's JSON
  as is. Human output: `ctx::table(&["id", "name", …], &rows)` for lists, `ctx::fields(&[("name",
  …), …])` for one thing, plain sentences for actions ("stopped agent 203"). Field helpers:
  `ctx::s(v, "key")` (a string or ""), `ctx::text(v, "key")` (any value as text).
- **Shape**: lists are `list` (with filters as flags), one thing is `show <id>`, actions are verbs
  (`create`, `stop`, `approve`, …). Long text goes in `--text` or from stdin when the flag is `-`.
  Destructive actions ask `Proceed? [y/N]` on a terminal unless `--yes`.
- No `unwrap()` on hub data; errors are `Err(String)` sentences.

## Build and check

`cargo build -p agentplaza-cli` and `cargo test -p agentplaza-cli`; no warnings from your files.
Never start the plaza to try a command.
