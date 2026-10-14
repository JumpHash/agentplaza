# Web and desktop parity tracker

Owner's rule: the web console and the desktop app have 1:1 parity, in full. This file tracks what one side has and the
other lacks, from an audit on 2026-10-13. The audit compared the hub API calls each screen makes, plus the shared UI
features. Tick an item off when it lands, with its commit.

The 3D views are one Rust engine on both apps. three.js is deleted from the web (`987394f`, the last phase of
`docs/design/engine.md`, done), so the web's 3D is the engine too. Both apps get the same Floor, Machines thumbnails,
portraits and labs.

## Desktop gaps (web has it)

Audited on 2026-10-13 against the code (each row checked by the hub endpoints both apps call).

| # | Screen | What | State |
|---|---|---|---|
| 1 | Vault | Unseal and seal, slots (passphrase, key file), recovery code, rekey | done |
| 2 | Vault | Secret create, update and remove, rollback, test | done |
| 3 | Vault | `.env` import, audit log with chain verify, consumers per node matrix | done; verify names the first broken entry in both apps (845a960) |
| 4 | MCP | Catalogue, servers list and detail, start, stop, test, tools, logs | done |
| 5 | MCP | Add and edit wizard, with secrets and the test step | done |
| 6 | MCP | *This hub as a server*: modes, tokens, call log | done |
| 7 | Git | Code browser: branches, commits, tree, file viewer | done |
| 8 | Git | PR list and detail | done |
| 9 | Nodes | Node link: status, join by code, leave; per-node machine and runs | done: status, join and leave through `/v1/node-link` (845a960); per-node detail from `/v1/machines`, as on the web. on a remote hub Start and Stop are left to the node's own machine |
| 10 | Sign-in | Remote hub: owner-token paste, hub identity, sign-out, identity pinned on first use | done in both apps (845a960) |
| 11 | Machines | Row expands into the live 3D orbit view (engine) | done |
| 12 | Memory | Memory graph view | done (845a960); both apps seed the layout alike |
| 13 | Approvals | Action kinds `lift`, `acknowledge`, `retire` | done |
| 14 | Lab | Model lifecycle `shadow`, `promote`, `retire` | done |
| 15 | Agent profile | 3D portrait (engine) | done |
| 16 | Shell | Toasts, a theme switch, conversation search and pagination, task withdraw, connector secret clear | done |
| 17 | Prompts | The prompt library: list, editor, versions, compare, publish, history, rules | done in both apps (e91d418); the desktop checks locally with the same `apz-prompt` crate |
| 18 | Skills | The skill library: list, New, Import (zip, .skill; a folder on the desktop), From GitHub, Export, the editor (file tree, preview, findings, versions, compare, history, rules, bindings, publish, GitHub update) | done in both apps (8fb8bab); the desktop checks locally with `apz-skill` |

## Web gaps (desktop has it)

| # | Screen | What | State |
|---|---|---|---|
| 1 | Setup | First-run checks | done: `#/setup` runs them through the hub API (845a960) |
| 2 | Setup | The start-up chooser: start locally, host, join, connect | not applicable: the web console is served by a running hub, so there is nothing to start or choose |

## Screen-level audit (2026-10-13, second pass)

The first audit compared the hub endpoints each app calls. This pass compared what each screen offers: actions,
filters, fields, tabs, form checks, keyboard, empty and error states, and live updates. Every row is being closed.

| Area | Gap | Side to change |
|---|---|---|
| Live updates | Projects, tasks (transcripts), approvals, conversations, board, council, treasury, lab datasets, recipes and HF imports refresh on hub events on the web; the desktop waited for its poll. Each desktop screen now has a `subscription` hook | desktop |
| Projects | "Open" on a document's file | desktop |
| Tasks, Library | Enter opens the selected row | desktop |
| Library | video tiles (the desktop opens them in the system player) | desktop |
| Git | the disabled Merge button says why | desktop |
| Prompts | clear filters, try again, unsaved-changes guard, reload latest on a conflict, download .md, unpick and clear in compare | desktop |
| New agent | approve or reject the orchestrator's proposals, a reason required to reject | desktop |
| Memory | find filters the graph, keyboard on nodes, drag to pin, clickable backlinks with missing chips, live note refresh; a Fit control on the web | both |
| Treasury | "Where the money goes", task-id filter, clawback kind, sortable tables | desktop |
| Conversations | clear search, Escape closes a thread | desktop |
| Board | Ctrl/Cmd+Enter sends a reply on both | desktop |
| Council | role display names | web |
| Rules | the Tools and Other groups | web |
| Society | mean trust in and out | web |
| Usage, lab | empty states, breeder sorts, model-detail fields, form validation messages | both, as verified |
| Shell | `[` toggles the sidebar; STOP ALL and RESUME ask first | desktop |
| Vault | copy-failure hint for the recovery code | desktop |
| MCP | loading and error states; catalogue, access scopes and hub server wording made identical | both, as verified |
| Machines | the desktop's diagnostics and live stats on the web | web |
| Sign-in | sign out on the web | web |

## Third pass (2026-10-14)

What landed on 2026-10-14. Each row gives the file and line where the feature lives in each app.

| Screen | What | Web | Desktop | Commit |
|---|---|---|---|---|
| Usage | "Used by" column: where each model is used (default, orchestrator, role) | `apps/web/src/views/usage.ts:675` (column), `:412` (`settingsUses`) | `apps/desktop/src/console/screens/usage.rs:1217` (header), `:1232` (value), `:598` (`settings_uses`); the desktop had it already | bf6db01 (web) |
| Usage | An unpriced model links to Models to set a price | `apps/web/src/views/usage.ts:686` (link "set a price under Models") | `apps/desktop/src/console/screens/usage.rs:1235` (the same link, `Msg::Go(Id::Models)`) | bf6db01 (desktop) |
| Models | Each model's price on a provider's model list | `apps/web/src/views/models-providers.ts:144` (`api.price`), `:145` (rates line); the key is encoded by `apps/web/src/api.ts:579` | `apps/desktop/src/console/screens/models.rs:424` (`model_prices`), `:565-566` (path from `price_path`), `:572-580` (`Msg::Price`), `:1164` (`price_path`, percent-encodes the key), `:1500-1501` (the line) | bf6db01, 2816382 |
| Treasury | "see usage" link on the real model cost | `apps/web/src/views/treasury.ts:699` (the link under the cost tile) | `apps/desktop/src/console/screens/treasury.rs:687` (`Msg::Go(Id::Usage)` on the cost tile), `:761` (the link) | bf6db01 |
| Rules | Edit button on an owner rule | `apps/web/src/views/rules.ts:537` (Edit button per rule) | `apps/desktop/src/console/screens/rules.rs:1401` (Edit button in `rule_row`) | bf6db01 |
| Tools | Connector Docs link opens the docs. The desktop opens https only (`openable`, `apps/desktop/src/console/ui.rs:983`); the web opens http too | `apps/web/src/views/tools.ts:274` (connector link), `:427` (proposal link, http or https) | `apps/desktop/src/console/screens/tools.rs:935` (connector link), `:1053` (proposal link), `:614` (`Msg::OpenDocs`) | bf6db01 |
| Robot lab | Clips list (shared and species clips), draw-call readout | `apps/web/src/views/robot-lab.ts:106` (`clipsOf`), `:144` (clip choices), `:203` (readout) | `apps/desktop/src/console/screens/engine_lab.rs:352-357` (clip list), `:365` (Clip select), `:431-432` (readout); `apps/desktop/src/console/floor_engine.rs:16`, `:108` (`LAST_DRAW_CALLS`) | bf6db01, 37902f8 |
| Room lab | Width, depth and seed commit on Enter, and clamp | `apps/web/src/views/room-lab.ts:143-144`, `:152` (commit on change: Enter or blur; an out-of-range value keeps the last one, `readNum` at `:71`, rather than clamping) | `apps/desktop/src/console/screens/engine_lab.rs:380-383` (fields), `:294-300` (`number`, `on_submit`), `:208-215` (`number_value`, clamps to 1 to 12 and rounds to halves) | bf6db01 |
| Hardware lab | GPU buttons "0 GPUs / 1 GPU / 2 GPUs", open slider step 0.01, params sorted, readout red on errors | `apps/web/src/views/hardware-lab.ts:148` (GPU buttons), `:155` (open step), `:165` (params sorted), `:220` (red on errors) | `apps/desktop/src/console/screens/engine_lab.rs:393` (GPU buttons), `:396` (open step), `:402` (params sorted), `:440` (error flag), `:450` (summary red) | bf6db01 |

The desktop screens were checked by self-snapshot (APZ_SNAPSHOT) against a stand-in hub. The robot lab reads 117 draw calls on the default all-species view.

## Out of scope

**Hidden labs.** The hardware, robot and room labs are developer tools behind hidden routes on the web and environment
switches on the desktop. They move into the engine, where both apps get them at once.
