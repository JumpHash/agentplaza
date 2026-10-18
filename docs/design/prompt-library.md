# Prompt library: system prompts and AGENTS.md

Status (2026-10-14): built. The hub is `crates/apid/src/prompts/` (routes, store in the database and in memory, seed,
tests), the checks are `crates/apz-prompt`, the storage is migration `0006_prompts.sql`, and the screens are
`apps/web/src/views/prompts.ts` and `prompt-editor.ts`, and `apps/desktop/src/console/screens/prompts.rs` and `prompt_editor.rs`.

Owner's request (2026-10-13): a library for system prompts and AGENTS.md files. Prompts are written in markdown, with
best-practice checks, a way to compare versions and see which is best, validation, a markdown preview, and versioning.
Everything is stored in the database and managed properly.

## What it is

- **A prompt** has an id (a slug), a kind, a title, a description, tags, an optional binding, and the version that is
  live.
  - **Kinds:**
    - `system`: the system prompt for a role;
    - `agents_md`: an AGENTS.md, the plaza's house rules for every agent, written to the agents.md convention;
    - `fragment`: a reusable piece of text.
  - **Bindings:**
    - `role:<role>` (system kind only): the published version *replaces* that role's built-in or custom prompt;
    - `plaza` (agents_md kind only): the published version is appended to every agent's system prompt, under
      `## House rules (AGENTS.md)`;
    - `project:<id>` (agents_md kind only): the published version is appended to the system prompt of the agents
      working on that project, under `## Project rules (AGENTS.md)`.
  - At most one live (not archived) prompt holds a given binding.
- **A version** is immutable once written: the body, its sha256, a note, the author, and the check result (score and
  findings) under the rule set it was scored with. Saving a body equal to the latest version does not create a new
  version.
- **Publishing** points the prompt at one version. Rolling back is publishing an older version. A version with any
  error-level finding cannot be published.
- **History:** every create, version, publish, unpublish, edit, archive and restore is an event row (append-only), with
  the actor and the time.

## Checks (`crates/apz-prompt`)

Checks are pure Rust and deterministic. The same crate runs in apid (for every API call and for stored versions) and
natively in the desktop (for live checks while typing). The web asks the hub (`POST /v1/prompts/check`, debounced).

`check(kind, body) -> Report` returns the findings (rule, severity, 1-based line or 0 for the whole document, message),
counts, stats (characters, words, lines, headings, estimated tokens at 4 characters per token), and a score:
`100 - 25 × errors - 8 × warnings - 2 × infos`, floored at 0. `RULES_VERSION` changes whenever a rule changes. A stored
version scored under an older rule set is shown as *stale*, and its live re-check is shown next to it.

**The best version** of a prompt is the one with the highest score, ties going to the newer version. The version list
marks it, and the compare view puts any two versions side by side with their scores, finding counts and a line diff.

### Rules

| Rule | Severity | Kinds | What it catches |
|---|---|---|---|
| `empty` | error | all | the body is blank |
| `too-long` | warning above 32,000 characters, error above 200,000 | all | long prompts dilute attention and cost tokens on every turn |
| `secret` | error | all | something that looks like a credential: `sk-…`, `ghp_…`, `github_pat_…`, `AKIA…`, `xox?-…`, `apz_mcp_…`, a PEM private key, or a long high-entropy value after `key`, `token`, `secret` or `password` |
| `unclosed-fence` | error | all | an odd number of ``` fences |
| `template-var` | error | all | a `{{name}}` that is not one of the known variables (`name`, `role`, `cid`, `tier`, `plaza`) |
| `heading-first` | warning | system, agents_md | the document does not open with a `#` heading |
| `heading-skip` | warning | all | a heading level jumps by more than one (`#` then `###`) |
| `empty-section` | warning | all | a heading with nothing under it before the next heading of the same or higher level |
| `duplicate-heading` | warning | all | the same heading text twice at the same level |
| `duplicate-line` | warning | all | the same non-blank line twice outside code fences |
| `shouting` | warning | all | emphasis inflation: more than 5 of MUST, NEVER, ALWAYS, IMPORTANT, CRITICAL in capitals, or more than 3 other all-caps words of four or more letters that are not known acronyms |
| `vague` | info | all | hedges that leave the model guessing: "try to", "if possible", "maybe", "etc.", "and so on", "as needed", "as appropriate", "various" |
| `negative-only` | info | all | a section whose list items mostly say what not to do; say what to do instead |
| `long-line` | info | all | a paragraph line over 400 characters; split it or make it a list |
| `fence-language` | info | all | a code fence without a language |
| `trailing-space` | info | all | trailing whitespace |
| `system-sections` | warning | system | a missing house section: Purpose, Responsibilities, Deliverables, Boundaries (the template `roles_custom::prompt_of` writes) |
| `second-person` | info | system | the prompt talks about "the agent" or "the assistant" instead of addressing it as "you" |
| `agents-md-sections` | warning for Setup or Build, and for Testing; info for the rest | agents_md | the agents.md convention's sections: Project overview, Setup or Build commands, Testing, Code style, Commit or PR guidelines, Security |
| `bare-command` | warning | agents_md | a shell command (`cargo`, `npm`, `pnpm`, `yarn`, `make`, `pytest`, `go`, `docker`, `git`, or a line starting with `$ `) outside backticks or a code fence |

`rules()` returns the catalogue (id, severity, kinds, title, why). The UI shows it as the in-app best-practices guide, and
each finding links to its rule.

### Other helpers

- `diff(old, new)`: a line diff (LCS) with old and new line numbers.
- `render(body, vars)`: substitutes `{{name}}`-style variables. Unknown names are left as written; `template-var`
  already reports them.
- `slug(title)`: the id derived from a title.

## Storage (migration 0006)

```
prompts          id (slug), kind, title, description, tags[], binding, active_version, created, updated, archived_at
prompt_versions  (prompt_id, version) PK, body, body_sha256, note, author, score, errors, warnings, infos,
                 rules_version, created                                         append-only
prompt_findings  (prompt_id, version, seq) PK, rule, severity, line, message    append-only
prompt_events    id, prompt_id, action, version, actor, at                      append-only
```

- `prompts.binding` is unique among live prompts (a partial unique index).
- `(id, active_version)` references `prompt_versions`.
- The append-only tables use the `forbid_mutation()` triggers from 0002.
- All SQL lives in `sql/queries/prompt_*.sql`.

**Without Postgres** (`APZ_DATABASE_URL` unset), the same `PromptStore` trait is served by `MemoryPrompts`. It is
persisted to `<state_dir>/prompts.json` by atomic rename, with the same rules.

**Built-in prompts:**
- At start, each built-in role prompt (`<prompts>/<role>/prompt.md`) is imported as the prompt `role-<role>`, kind
  system, bound to `role:<role>`, author `builtin`, and published.
- When a release changes a built-in file, a new `builtin` version is added. It is published only if the live version
  is itself a `builtin` one: the owner's edits are never overwritten.

## API (apid)

| Method and path | What |
|---|---|
| `GET /v1/prompts?kind=&archived=` | the list: id, kind, title, description, tags, binding, active_version, latest_version, best_version, score of the live version, updated, archived |
| `POST /v1/prompts` | create: `{id?, kind, title, description?, tags?, binding?, body, note?}`; writes version 1. 201 with the prompt |
| `GET /v1/prompts/{id}` | the prompt, its versions (no bodies, with score and counts), the live version's body and findings |
| `PUT /v1/prompts/{id}` | edit the metadata: title, description, tags, binding |
| `GET /v1/prompts/{id}/versions/{v}` | one version: body, findings, stats, `stale` |
| `GET /v1/prompts/{id}/versions/{v}/raw` | the body as `text/markdown`, for downloading an AGENTS.md |
| `POST /v1/prompts/{id}/versions` | `{body, note?, base_version}`. 201 with the new version; 200 with the latest if the body is unchanged; 409 if `base_version` is not the latest |
| `POST /v1/prompts/{id}/publish` | `{version}`. 422 if the version has errors |
| `POST /v1/prompts/{id}/unpublish` | no version is live; the binding falls back to the built-in prompt |
| `POST /v1/prompts/{id}/archive`, `/restore` | archive and restore. Archiving releases the binding |
| `GET /v1/prompts/{id}/diff?from=&to=` | the line diff between two versions, with both reports |
| `GET /v1/prompts/{id}/history` | the events, newest first |
| `POST /v1/prompts/check` | `{kind, body}`: the report, nothing stored (for the live editor) |
| `GET /v1/prompts/rules` | the rule catalogue, `RULES_VERSION`, the template variables, and the starter template for each kind (each scores 100) |

The owner's console token may do everything. Agents never write prompts through these routes. The hub MCP server
exposes the following tools:
- `prompts_list` and `prompt_get` (scope observe);
- `prompt_propose` (operate): adds a version, never publishes;
- `prompt_publish` (govern).

## Screens (web and desktop, 1:1)

- **Prompts** (under Agents in the nav):
  - the list, with filters by kind and tags and a search;
  - the live version's score as a badge, plus binding chips;
  - "New prompt" (kind, title, binding, a starter template per kind).
- **Prompt editor:**
  - a markdown source pane and a live preview pane (the existing renderers: `ui/markdown.ts` on the web,
    `ui::markdown` on the desktop);
  - findings under the editor as you type, each one jumping to its line and naming its rule;
  - the score, the counts and the token estimate;
  - Save version (with a note), Publish, Unpublish, Archive.
- **Versions:**
  - the list, marked with the live version and the best version;
  - compare any two: scores side by side and a line diff;
  - restore an old version as a new one;
  - publish an old version (rollback);
  - history.
- **Rules:** the catalogue, as the in-app best-practices guide.
