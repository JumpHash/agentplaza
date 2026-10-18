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
| 19 | Work | The project as one flow (`apps/web/src/views/work.ts`, `apps/desktop/src/console/screens/work.rs`): the plan column, the tasks laid out by `crates/apz-graph` in the order their `after` links allow, delivery; Waiting, Blocked and Needs you on the cards; the task drawer; "Only what needs me"; state columns when no task has a link. Beside it: the Overview's flow lines; on the Tasks screen, "waits on" chips, the Ready filter and the ready tasks nobody has claimed, by role | done in both apps (Graph 5/6 and 6/6): the desktop lays the graph out natively with the same crate and the web's card sizes, and a test lays the fixtures out both ways. The CLI prints a project's columns (`agentplaza tasks graph`) |

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

## Fourth pass (2026-10-14): workstations, the IDE workspace, rich content and the board

What landed on 2026-10-14, per surface. "Done" means the surface has the feature and the commit is listed. The CLI
column is the CLI or TUI, which the owner's 1:1 rule does not cover, so its gaps are shown for the record.

| Area | What | Web | Desktop | CLI or TUI | State |
|---|---|---|---|---|---|
| Workstations | List, enrol with a join code, facts, leases and the queue, settings (takeover policy, rate, recording), drain, revoke, audit | `apps/web/src/views/workstations.ts` | `apps/desktop/src/console/screens/workstations.rs` | `apps/cli/src/cmd/stations.rs` (list, show, codes, leases, settings, drain, revoke, audit); TUI tab `w` in `apps/cli/src/tui/workstations.rs` | done: `55317f9`, `42aa328`, `4e2ded9`, `f2376a1` |
| Workstations | Live view, take over and give back | `apps/web/src/views/workstations-live.ts` | `apps/desktop/src/console/screens/workstations_live.rs` | `stations screenshot`, `stations watch`; `stations take` (the commands on stdin, give-back ends it) and `stations input` (one action); TUI `T` on the Workstations tab | web and desktop done (`5d1e3f9`, `0fb259a`); CLI done in `cli: take over a workstation from the terminal, one-shot input, and VM workstations` |
| Workstations | Workstation VMs: list, New VM (node, OS, size, GPU, backend), delete | `apps/web/src/views/workstations-vms.ts` | `apps/desktop/src/console/screens/workstations_vms.rs` | `agentplaza stations vms` (list, create, delete) | web and desktop done (`53ea125`); CLI done (`6dcd7a9`) |
| Workstations | Recordings: list, playback, settings | `apps/web/src/views/workstations-recordings.ts` | `apps/desktop/src/console/screens/workstations_rec.rs` | `stations recordings` (list, show, frames, delete, defaults); playback in the apps | done (`a9e0b95`, `edef6b3`, `f2376a1`) |
| Workstations | Work folder: files, sync state, conflicts, pause and resume | `apps/web/src/views/workstations-sync.ts` | `apps/desktop/src/console/screens/workstations_files.rs` | `stations sync`, `stations files` | done (`ebc360a`, `c789528`) |
| Machines and the Floor | Workstation kind with its live thumbnail, OS models on the Floor, screen glow by state | `apps/web/src/views/station-machine.ts`, `machine-list.ts` | `apps/desktop/src/console/screens/machines.rs`, `floor.rs` | n/a | done (`0cd7a64`) |
| Setup | Add as a workstation | not applicable: the web is served by a hub | `apps/desktop/src/console/boot.rs` (`Choice::Workstation`), `workstation_setup.rs` | `agentplaza workstation join`, `status`, `leave` | done (`2b0d692`) |
| IDE workspace | Agent-first layout: projects and agents, the agent's chat, terminal, editor and diff, the board, files, tasks, approvals and activity | `apps/web/src/ide/`, `apps/web/src/views/ide*.ts` | `apps/desktop/src/console/screens/ide*.rs` | TUI Workspace tab: agent tree, transcript with composer, project board | done (`7c29f76`, `a010773`, `c668a71`) |
| IDE workspace | Owner file edits with the conflict check, search, checkpoints and restore (agent workspaces) | `apps/web/src/ide/` | `apps/desktop/src/console/screens/ide*.rs` | `agentplaza ide` (`ls`, `cat`, `edit`, `put`, `diff`, `checkpoints`, `restore`, `search`) | done (`235963d`, `33af468`) |
| IDE workspace | Terminal in a VM or on a workstation | `apps/web/src/views/ide-term-host.ts` (xterm.js) | `apps/desktop/src/console/screens/ide_terminal.rs` (`apz-vt`) | `agentplaza ide term` | done (`728ff11`, `9af2849`, `1719fcf`, `b52d8b4`) |
| IDE workspace | Runs with timeout and output cap | `apps/web/src/ide/` | `apps/desktop/src/console/screens/ide*.rs` | `agentplaza ide run` | done |
| IDE workspace | Command palette (Ctrl/Cmd+K): agents, files, tasks, commands | `apps/web/src/views/ide-palette.ts` | `apps/desktop/src/console/screens/ide_palette.rs` | n/a | done |
| IDE workspace | Station workspaces (`station:<id>`) | `apps/web/src/ide/types.ts` | `apps/desktop/src/console/screens/ide.rs` | `agentplaza ide` with `station:<id>` | done (`33af468`) |
| Rich content | Chips, hover cards, inline images, video and HTML cards, the unfurl | `apps/web/src/ui/rich.ts`, `ui/embeds.ts` | `apps/desktop/src/console/rich.rs` | `apps/cli/src/rich.rs` (short labels, terminal hyperlinks, plain text when piped); `agentplaza unfurl` | done (`3f00303`, `2212f74`, `7592dce`, `0f5c0e4`, `7b0cad8`); the desktop takes the plaza git host from the overview (`rich.rs`), so plaza git references are chips there too |
| Board (owner actions) | Post with `@name` and `@all`, pause and resume deliveries, export a thread or the board | `apps/web/src/board/owner.ts` | `apps/desktop/src/console/screens/board.rs` | `agentplaza board post`, `comment`, `pause`, `export` | done (`ddc4951`) |

## Fifth pass (2026-10-15): VM snapshots, egress credentials and E2B

The owner surfaces the snapshot, sleep, egress credential and E2B work added. "Done" means the surface has the
feature, in the file or command named.

| Surface | Web | Desktop | CLI | State |
|---|---|---|---|---|
| Sleep and wake an agent | `apps/web/src/views/agent-detail.ts` (Sleep, Wake) | `apps/desktop/src/console/screens/agent.rs` (`Act::Sleep`, `Act::Wake`) | `agentplaza agents sleep`, `wake` (`apps/cli/src/cmd/agents/lifecycle.rs`) | Done in all three |
| Machine checkpoints: list, take, restore, delete | `agent-detail.ts` (Machine checkpoints) | `agent.rs` (the checkpoints card) | `agentplaza agents checkpoints`, `checkpoint`, `rollback`, `checkpoint-delete` | Done in all three |
| Fork an agent | `agent-detail.ts` (Fork…) | `agent.rs` (Fork) | `agentplaza agents fork` | Done in all three |
| Move an agent to another node | `agent-detail.ts` (Move…) | `agent.rs` (Move…) | `agentplaza agents move` | Done in all three |
| Drain and undrain a node | `apps/web/src/views/nodes.ts` | `apps/desktop/src/console/screens/nodes.rs` | `agentplaza nodes drain`, `undrain` | Done in all three |
| Sleep and checkpoint settings: `idle_sleep_min`, `sleep_never_roles`, `vm_auto_checkpoints` | `apps/web/src/views/models-policy.ts` | `apps/desktop/src/console/screens/models.rs` (Limits) | `agentplaza models policy set --idle-sleep-min --sleep-never-roles --vm-auto-checkpoints` | Done in all three |
| Egress credential rules: list, create, enable, delete | `apps/web/src/views/egress-credentials.ts` | `apps/desktop/src/console/screens/egress.rs` | `agentplaza egress list`, `add`, `enable`, `disable`, `rm` | Done in all three |
| E2B keys, sandboxes and templates | `apps/web/src/views/e2b.ts` | `apps/desktop/src/console/screens/e2b.rs` | `agentplaza e2b keys …`, `e2b sandboxes`, `kill`, `pause`, `resume`, `e2b templates`, `delete-template` | Done in all three |
| Warm templates: list and remove | none | none | `agentplaza templates list`, `rm` | CLI and API (`GET/DELETE /v1/templates`): templates build and rebuild themselves; listing them is a diagnostic |
| Data volumes: list, create, remove | `apps/web/src/views/volumes.ts` | `apps/desktop/src/console/screens/volumes.rs` | `agentplaza volumes ls`, `create`, `rm` | Done in all three (`GET/POST /v1/volumes`, `DELETE /v1/volumes/{name}`) |
| Per-role volumes (`role_volumes`) | `volumes.ts` (Roles) | `volumes.rs` (Roles) | `agentplaza volumes roles`, `mount`, `unmount` | Done in all three |
| Sleep and wake a workstation VM | apps/web/src/views/workstations-vms.ts (Sleep, Wake) | apps/desktop/src/console/screens/workstations_vms.rs (Sleep, Wake) | agentplaza stations vms sleep, wake | Done in all three (plaza VMs) |

## Sixth pass (2026-10-18): projects and conversations

The project scope (`plaza-tv/PLAN-projects.md`): an agent works only on its project, the console switches between
projects, and a project is set up in a conversation with the orchestrator. "Done" means the surface has the feature, in
the file or command named.

| Surface | Web | Desktop | CLI or TUI | State |
|---|---|---|---|---|
| The project switcher: All projects, each live project, New project…; remembered | `apps/web/src/ui/scope-switcher.ts`, `scope.ts` (the route carries it, `#/p/<id>/…`) | `apps/desktop/src/console/shell.rs` (the top bar), `scope.rs` (remembered per hub) | TUI `p` and `P`; `--project` on `agents list`, `tasks list` and `tasks create` | web Scope 5/8, desktop Scope 7/8, CLI Scope 8/8 |
| New project: a conversation with the orchestrator, or a project opened directly | `apps/web/src/ui/new-project.ts` | `apps/desktop/src/console/new_project.rs` | `agentplaza chat`, `agentplaza projects create` | web 5/8, desktop 7/8, CLI 8/8 |
| Under a project: Overview, Agents, Tasks, Work, Board and Approvals show it; the plaza's setup screens say its own settings override them | `apps/web/src/views/overview.ts`, `agent-list.ts`, `tasks.ts`, `work.ts`, `board.ts`, `approvals.ts`, `main.ts` | `apps/desktop/src/console/screens/overview.rs`, `agents.rs`, `tasks.rs`, `work.rs`, `board.rs`, `approvals.rs`, `shell.rs` (the note) | `agents list --project`, `tasks list --project`; the TUI's Agents and Tasks tabs | web 5/8, desktop 7/8, CLI 8/8 |
| The bench: who works where, Put on…, Back to the bench; an agent's page says where it works and since when | `agent-list.ts`, `agent-detail.ts` | `agents.rs` (the Project column, the bench card), `agent.rs` | `agents list --bench`, `agents show`, `projects members add`, `remove` | web 5/8, desktop 7/8, CLI 8/8 |
| Projects: Pause, Resume, Archive; the Team tab (members, the bench, the agents from before, new agents for it); the Settings tab (each setting inherited or overridden, models by role, the budget) | `projects.ts`, `project-team.ts`, `project-settings.ts` | `projects.rs`, `project_team.rs`, `project_settings.rs` | `projects pause`, `resume`, `archive`, `members`, `settings`, `budget` | web 5/8, desktop 7/8, CLI 8/8 |
| Tasks: a new task names its project, its assignee one of that project's agents; a task from before projects moves into one | `tasks.ts` | `tasks.rs` | `tasks create --project`, `tasks move` | web 5/8, desktop 7/8, CLI 8/8 |
| The IDE: conversations with the orchestrator, the draft beside the chat, Edit and Start; project mode; the chat composer talks to an idle agent and steers a working one | `apps/web/src/views/ide-conversation.ts`, `ide-sidebar.ts`, `ide-chat.ts`, `apps/web/src/ide/conversation-model.ts` | `apps/desktop/src/console/screens/ide_conversation.rs`, `ide_sidebar.rs`, `ide_panels.rs`, `ide_model.rs` | `agentplaza chat` (`/draft`, `/start`); the TUI's Chat tab | web 6/8, desktop 7/8, CLI 8/8 |

## Out of scope

**Hidden labs.** The hardware, robot and room labs are developer tools behind hidden routes on the web and environment
switches on the desktop. They move into the engine, where both apps get them at once.
