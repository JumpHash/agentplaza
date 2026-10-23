# Work-engine parity: what each console offers

This file says what each console offers for the work engine: sessions, seats, the inbox, budgets, chats, projects, the
work and task flow, landing through Weft, the change recipe and reviews, nodes and the IDE workspace. It is taken from the
code as it stands on 2026-10-23 (branch eng/07). It replaces the audit of 2026-10-13 and its later passes.

The owner's rule is 1:1 parity between the web console and the desktop app. The CLI and the terminal UI (TUI) are shown
for the record; their gaps are not parity gaps.

## How to read it

Each feature has one row per console. offered: the console does it end to end. partial: some of it, or read-only; the
note says which. missing: no way to do it; the note names the route when there is one. The CLI/TUI row is offered only
when both the CLI and the TUI have the feature.

Sources are repo-relative. Routes are in `crates/apid/src/work_routes.rs` and the route files named in the notes;
`docs/api/openapi.json` lists the same paths. The web's screens are registered in `apps/web/src/views/index.ts` (the IDE
is at `#/ide`, not in the sidebar); the desktop's sidebar is `apps/desktop/src/console/shell.rs`; the CLI's subcommands
are in `apps/cli/src/main.rs`, and the TUI tabs in `apps/cli/src/tui/app.rs`.

## Sessions

A session is one run of a step of a task, a project's planner, the orchestrator, or a helper.

| Feature | Console | Status | Note | Source |
|---|---|---|---|---|
| Session list | Web | offered | The sessions of the project in scope, live ones first. No state or task filter. | `apps/web/src/views/sessions.ts` |
| | Desktop | offered | Live-only switch; the project comes from the top bar. | `apps/desktop/src/console/screens/sessions.rs` |
| | CLI/TUI | offered | CLI `sessions` takes `--project`, `--task`, `--state`, `--live`. TUI Sessions tab; `p` and `P` switch the project. | `apps/cli/src/cmd/sessions.rs`, `apps/cli/src/tui/sessions.rs` |
| Session page | Web | offered | Facts, model and usage. The expert view adds node, VM, run, engine, attempt. | `apps/web/src/views/session.ts` |
| | Desktop | offered | The same facts on the session screen. | `apps/desktop/src/console/screens/session.rs` |
| | CLI/TUI | partial | CLI `sessions show <id>`. TUI shows a details popup with no transcript or context. | `apps/cli/src/cmd/sessions.rs`, `apps/cli/src/tui/screens.rs` |
| Transcript | Web | offered | Oldest first; follows the end while the reader is there. | `apps/web/src/views/session.ts` |
| | Desktop | offered | Transcript tab. | `apps/desktop/src/console/screens/session.rs` |
| | CLI/TUI | partial | CLI `sessions transcript <id> --follow`. TUI has none. | `apps/cli/src/cmd/sessions.rs` |
| Context | Web | offered | What the last model call sent, by part and by tool. | `apps/web/src/views/session.ts` |
| | Desktop | offered | Context tab. | `apps/desktop/src/console/screens/session.rs` |
| | CLI/TUI | partial | CLI `sessions context <id>`. TUI has none. | `apps/cli/src/cmd/sessions.rs` |
| Stop a session | Web | offered | A step stops at once; a planner or the orchestrator asks first. | `apps/web/src/views/session.ts` |
| | Desktop | offered | The same rule. | `apps/desktop/src/console/screens/session.rs` |
| | CLI/TUI | partial | CLI `sessions stop <id>`, which asks unless `--yes`. TUI `s` stops the whole plaza (STOP ALL), not one session. | `apps/cli/src/cmd/sessions.rs`, `apps/cli/src/tui/app.rs` |

## Seats and threads

A seat is a role in a task (builder, reviewer, and so on). One session fills a seat at a time.

| Feature | Console | Status | Note | Source |
|---|---|---|---|---|
| Seats of a task | Web | offered | One row per seat: role, state, the session filling it, tries, rounds, last model, and what sent the task back. Shown on the task page. | `apps/web/src/views/task-seats.ts`, `apps/web/src/views/tasks.ts` |
| | Desktop | offered | Panel on the task page. | `apps/desktop/src/console/screens/task_seats.rs` |
| | CLI/TUI | partial | CLI `seats <task>`. No TUI view. | `apps/cli/src/cmd/seats.rs` |
| Seat threads (messages addressed to seats) | Web | missing | `/v1/tasks/{id}/threads` and `/v1/threads/{id}` exist in `crates/apid/src/work_routes.rs`; no console calls them. | `crates/apid/src/threads.rs` |
| | Desktop | missing | The same two routes, not called. | `crates/apid/src/threads.rs` |
| | CLI/TUI | missing | The same two routes, not called. | `crates/apid/src/threads.rs` |

## Inbox, cards and approvals

A card is a decision the plaza needs from the owner. The inbox lists the cards; a chat shows the ones it asked.

| Feature | Console | Status | Note | Source |
|---|---|---|---|---|
| Inbox (cards) | Web | offered | Each card with its choices. A choice that takes a budget amount has a field, in the budget's unit. | `apps/web/src/views/inbox.ts` |
| | Desktop | offered | The same, on the Inbox screen. | `apps/desktop/src/console/screens/inbox.rs` |
| | CLI/TUI | offered | CLI `inbox`, `inbox answer <card> <choice> [--amount]`. TUI Inbox tab; Enter answers after a confirm. | `apps/cli/src/cmd/inbox.rs`, `apps/cli/src/tui/inbox.rs` |
| Approvals (calls and agent proposals) | Web | offered | Calls agents ask to make and the orchestrator's proposed agents: approve, deny, reject, send back. | `apps/web/src/views/approvals.ts` |
| | Desktop | offered | The same list; a rejection asks for its reason in a dialog. | `apps/desktop/src/console/screens/approvals.rs` |
| | CLI/TUI | partial | CLI `approvals`, `approvals proposals`, `approve-proposal`, `reject-proposal` cover both. TUI Approvals tab decides calls only (`a`, `d`); proposals are not listed. | `apps/cli/src/cmd/approvals.rs`, `apps/cli/src/tui/app.rs` |

## Budgets

A budget caps a project or a task, in dollars (`usd`) or tokens. A task's budget is reserved from its project when the
task starts.

| Feature | Console | Status | Note | Source |
|---|---|---|---|---|
| Project budget | Web | offered | Cap, unit, warn share, monthly cap; what is spent, held and left. Set in a sheet. | `apps/web/src/views/budget.ts`, `apps/web/src/views/project-settings.ts` |
| | Desktop | offered | Set-budget form with the warn share and the monthly cap. | `apps/desktop/src/console/screens/budget.rs` |
| | CLI/TUI | partial | CLI `budget project <id> --set UNIT [CAP]`, with the warn and monthly flags. TUI has none. | `apps/cli/src/cmd/budget.rs` |
| Task budget | Web | offered | Budget card on the task page. | `apps/web/src/views/tasks.ts`, `apps/web/src/views/budget.ts` |
| | Desktop | offered | Budget panel on the task page. | `apps/desktop/src/console/screens/tasks.rs`, `apps/desktop/src/console/screens/budget.rs` |
| | CLI/TUI | partial | CLI `budget task <id> --set UNIT [CAP]`. TUI has none. | `apps/cli/src/cmd/budget.rs` |
| Ledger | Web | offered | Newest first; every column with the expert switch. | `apps/web/src/views/budget.ts` |
| | Desktop | offered | Ledger section of the budget panel. | `apps/desktop/src/console/screens/budget.rs` |
| | CLI/TUI | partial | CLI `budget ledger`. TUI has none. | `apps/cli/src/cmd/budget.rs` |
| Disk quota (project) | Web | missing | The project budget route takes `disk_quota_bytes`; the web never sends it. | `crates/apid/src/work_routes.rs` |
| | Desktop | missing | The same field, not sent. | `crates/apid/src/work_routes.rs` |
| | CLI/TUI | missing | The same field, not sent. | `crates/apid/src/work_routes.rs` |

## Chats

A chat is a conversation with the orchestrator (`main`), with a project's planner, or with a task's seats.

| Feature | Console | Status | Note | Source |
|---|---|---|---|---|
| Chat list | Web | offered | The orchestrator's, each project's and each task's; a switch shows the archived ones. | `apps/web/src/views/chats.ts` |
| | Desktop | offered | The same list, filtered by project, with archived. | `apps/desktop/src/console/screens/chats.rs` |
| | CLI/TUI | offered | CLI `chats` with `--project` and `--archived`. TUI Chat tab. | `apps/cli/src/cmd/chats.rs`, `apps/cli/src/tui/chat.rs` |
| Open a chat with a project's planner | Web | offered | New chat dialog: a live project and a title. | `apps/web/src/views/chats.ts` |
| | Desktop | offered | New chat dialog ("Start chat"). | `apps/desktop/src/console/screens/chats.rs` |
| | CLI/TUI | offered | CLI `chats open <project>`. TUI `n` opens one in the project the switcher shows. | `apps/cli/src/cmd/chats.rs`, `apps/cli/src/tui/chat.rs` |
| Messages | Web | offered | Composer. Files go with a message in the IDE conversation, not on this screen. | `apps/web/src/views/chats.ts`, `apps/web/src/views/ide-talk.ts` |
| | Desktop | offered | Text composer. | `apps/desktop/src/console/screens/chats.rs` |
| | CLI/TUI | offered | CLI `chats show <id>`, `chats post <id> <text>`. TUI `t`. | `apps/cli/src/cmd/chats.rs`, `apps/cli/src/tui/chat.rs` |
| Rename, archive, restore | Web | offered | Rename in place; archive and restore. | `apps/web/src/views/chats.ts` |
| | Desktop | offered | Rename, archive, restore. | `apps/desktop/src/console/screens/chats.rs` |
| | CLI/TUI | partial | CLI `chats archive <id>` with `--undo`; no rename. TUI has none. | `apps/cli/src/cmd/chats.rs` |
| Chat model | Web | partial | A model chip in the IDE conversation sets it. The Chats screen has none. | `apps/web/src/views/ide-talk.ts`, `apps/web/src/api-context.ts` |
| | Desktop | partial | Set from the IDE chat tools only. | `apps/desktop/src/console/chat_tools.rs` |
| | CLI/TUI | missing | No command or tab sets it. | `apps/cli/src/cmd/chats.rs` |
| Project draft and Start | Web | offered | In the IDE conversation: the draft beside the chat, edits to it, and Start, which answers the draft's start card. | `apps/web/src/views/ide-talk.ts` |
| | Desktop | partial | The New project dialog talks it through; the start card is answered in the Inbox. No draft view. | `apps/desktop/src/console/new_project.rs`, `apps/desktop/src/console/screens/inbox.rs` |
| | CLI/TUI | partial | `chats` talks it through; the start card is answered with `inbox answer` or the TUI Inbox tab. No draft view. | `apps/cli/src/cmd/projects.rs`, `apps/cli/src/cmd/inbox.rs` |
| Chat transcript (the turns the session took on a chat) | Web | missing | `GET /v1/chats/{id}/transcript` is not called. | `crates/apid/src/work_routes.rs` |
| | Desktop | missing | The same route, not called. | `crates/apid/src/work_routes.rs` |
| | CLI/TUI | missing | The same route, not called. | `crates/apid/src/work_routes.rs` |

## Projects

A project is a goal the plaza works on. It goes from brief to plan, then tasks, then delivery.

| Feature | Console | Status | Note | Source |
|---|---|---|---|---|
| Project list and New project | Web | offered | The list; New project talks it through with the orchestrator, opens one directly, or adopts an existing one. | `apps/web/src/views/projects.ts`, `apps/web/src/ui/new-project.ts` |
| | Desktop | offered | The list; the same three ways to start. | `apps/desktop/src/console/screens/projects.rs`, `apps/desktop/src/console/new_project.rs` |
| | CLI/TUI | partial | CLI `projects` (list, show, create, adopt). TUI has the project switcher only: no list, no create. | `apps/cli/src/cmd/projects.rs`, `apps/cli/src/tui/scope.rs` |
| Pause, resume, archive | Web | offered | Pause, resume, and archive. | `apps/web/src/views/projects.ts` |
| | Desktop | offered | Pause, resume; archive asks first. | `apps/desktop/src/console/screens/projects.rs` |
| | CLI/TUI | partial | CLI `projects pause`, `resume`, `archive`. TUI has none. | `apps/cli/src/cmd/projects.rs` |
| Plan: approve, send back | Web | offered | "Approve plan" and "Send back" to an earlier stage. | `apps/web/src/views/projects.ts` |
| | Desktop | offered | "Approve plan" and "Send back". | `apps/desktop/src/console/screens/projects.rs` |
| | CLI/TUI | partial | CLI `projects approve`, `projects send-back`. TUI has none. | `apps/cli/src/cmd/projects.rs` |
| Mode (build or maintenance) | Web | offered | Mode chooser on the project page. | `apps/web/src/views/projects.ts` |
| | Desktop | offered | Mode chip; a change calls the hub's mode route. | `apps/desktop/src/console/screens/projects.rs` |
| | CLI/TUI | partial | CLI `projects mode`, `projects modes`. TUI has none. | `apps/cli/src/cmd/projects.rs` |
| Settings (each inherited or overridden) | Web | offered | Models by role, the budget and the other settings; each one inherited or overridden. | `apps/web/src/views/project-settings.ts` |
| | Desktop | offered | Models by role, review timeout, task budget. | `apps/desktop/src/console/screens/project_settings.rs` |
| | CLI/TUI | partial | CLI `projects settings`. TUI has none. | `apps/cli/src/cmd/projects/settings.rs` |
| Maintenance (signals, proposals, sweeps) | Web | offered | Signals; proposals approved, declined or left for later; sweeps. | `apps/web/src/views/project-maintenance.ts` |
| | Desktop | offered | Signals; approve and decline proposals. | `apps/desktop/src/console/screens/project_maintenance.rs` |
| | CLI/TUI | partial | CLI `projects maintenance`. TUI has none. | `apps/cli/src/cmd/projects/maintenance.rs` |
| Environments and deploy hooks | Web | offered | Environments, deploy hooks, their secrets and runs. | `apps/web/src/views/project-environments.ts` |
| | Desktop | offered | The same, on the project's Environments tab. | `apps/desktop/src/console/screens/project_environments.rs` |
| | CLI/TUI | partial | CLI `projects env`. TUI has none. | `apps/cli/src/cmd/projects/environments.rs` |

## Work and tasks

A task belongs to one project and goes only to that project's agents. The work view orders a project's tasks by their
`after` links.

| Feature | Console | Status | Note | Source |
|---|---|---|---|---|
| Work (a project as one flow) | Web | offered | Plan, then the tasks in the order their links let them run, then delivery. "Only what needs me" filters the cards. The map is laid out by `apz-graph-web`. | `apps/web/src/views/work.ts`, `apps/web/src/views/work-model.ts`, `apps/web/src/graph/wasm.ts` |
| | Desktop | offered | The same flow, laid out natively by `apz-graph`. | `apps/desktop/src/console/screens/work.rs` |
| | CLI/TUI | partial | CLI `tasks graph` prints the columns. TUI Tasks tab shows the "Runs after" links; no map. | `apps/cli/src/cmd/tasks.rs`, `apps/cli/src/tui/app.rs` |
| Tasks board and one task | Web | offered | Four lanes. A task's goal, result, facts, and a timeline from the assignee's transcript. | `apps/web/src/views/tasks.ts` |
| | Desktop | offered | Four-lane board; the task page with its timeline. | `apps/desktop/src/console/screens/tasks.rs` |
| | CLI/TUI | partial | CLI `tasks list`, `tasks show`, `tasks timeline`. TUI Tasks tab, read-only. | `apps/cli/src/cmd/tasks.rs` |
| New task | Web | offered | "New task" dialog; the task names its project. | `apps/web/src/views/tasks.ts` |
| | Desktop | offered | "New task"; the task belongs to a project. | `apps/desktop/src/console/screens/tasks.rs` |
| | CLI/TUI | partial | CLI `tasks create --project`. TUI has none. | `apps/cli/src/cmd/tasks.rs` |
| Assign, move, withdraw | Web | offered | Assign to an idle agent of the project; move between lanes or into a project; withdraw. | `apps/web/src/views/tasks.ts` |
| | Desktop | offered | Assign to, Move into a project, Withdraw. | `apps/desktop/src/console/screens/tasks.rs` |
| | CLI/TUI | partial | CLI `tasks assign`, `tasks move`, `tasks withdraw`. TUI has none. | `apps/cli/src/cmd/tasks.rs` |
| Claim and finish (`/v1/tasks/{id}/claim`, `/finish`) | Web | missing | The routes exist; no console calls them. | `docs/api/openapi.json` |
| | Desktop | missing | The same routes, not called. | `docs/api/openapi.json` |
| | CLI/TUI | missing | The same routes, not called. | `docs/api/openapi.json` |

## Landing through Weft

Weft is the plaza's version control. A change is sealed for review, signed, then landed onto `main` from the landing queue.
The owner's Weft screen reads the repositories, changes and releases. The CLI passes its words to the Weft command
(`crates/weft/src/cli.rs`).

| Feature | Console | Status | Note | Source |
|---|---|---|---|---|
| Repositories, changes, change page | Web | offered | Repositories; changes as lanes off `main`; a change with its sign-off chain; files at `main` with blame. | `apps/web/src/views/history.ts`, `apps/web/src/views/history-model.ts` |
| | Desktop | offered | Repositories and changes; a change page with the sign-off dots. The sidebar calls it Weft. | `apps/desktop/src/console/screens/history.rs`, `apps/desktop/src/console/screens/history_map.rs` |
| | CLI/TUI | partial | CLI `weft ...` runs the Weft command (`log`, `show`, `tree`, `blame`). TUI has none. | `apps/cli/src/cmd/weft.rs` |
| Landing queue (queue, land) | Web | partial | The queue is read. No land, pause or clash action; the hub has no route for them. | `apps/web/src/views/history.ts` |
| | Desktop | partial | Read-only "Landing queue" card. | `apps/desktop/src/console/screens/history_ops.rs` |
| | CLI/TUI | partial | `weft queue`, `weft queue add`, `weft land` through the passthrough. TUI has none. | `apps/cli/src/cmd/weft.rs` |
| Sign-off and releases | Web | offered | Sign a change or a proposed release; list and propose releases. | `apps/web/src/views/history.ts` |
| | Desktop | offered | Sign-off chain; releases, with Sign for a proposed one. | `apps/desktop/src/console/screens/history.rs` |
| | CLI/TUI | partial | `weft release`, `weft release sign`. TUI has none. | `apps/cli/src/cmd/weft.rs` |
| Export to git, adopt | Web | offered | Export the landed changes to a connection, with a dry run first; adopt a repository. | `apps/web/src/views/history.ts`, `apps/web/src/api-weft.ts` |
| | Desktop | offered | Export to git; Adopt. | `apps/desktop/src/console/screens/history_ops.rs` |
| | CLI/TUI | partial | `weft export <repo> [--dry-run]`, `weft adopt <repo>` through the passthrough. TUI has none. | `apps/cli/src/cmd/weft.rs` |

## Change recipe and reviews

The change recipe names the steps a sealed change is reviewed with (for example code, docs, research). A review is one run
of those steps over one task's change.

| Feature | Console | Status | Note | Source |
|---|---|---|---|---|
| Change recipe (steps, edit, proposals) | Web | offered | The steps as a graph by layer; edit the recipe; approve or decline the orchestrator's proposals. | `apps/web/src/views/recipes.ts`, `apps/web/src/api-review.ts` |
| | Desktop | missing | No recipe screen; `/v1/recipes` is not called. | `apps/desktop/src/console/screens/mod.rs` |
| | CLI/TUI | partial | CLI `recipes` (list, show, put, proposals, approve, decline). TUI has none. | `apps/cli/src/cmd/recipes.rs` |
| Review of a task's change | Web | offered | The review graph: intents, the four readings, steps and gate. Shown on the task page and the Approvals screen; the expert view adds the step table and findings. | `apps/web/src/views/review-graph.ts`, `apps/web/src/views/review-model.ts` |
| | Desktop | partial | "Review changes" opens the change in the IDE diff. No rounds, steps or graph. | `apps/desktop/src/console/screens/tasks.rs` |
| | CLI/TUI | partial | CLI `tasks review` lists the rounds; `tasks rule` sends the owner's ruling (pass, rework, fail). No graph. TUI has none. | `apps/cli/src/cmd/tasks.rs` |

## Nodes

A node is a machine that runs agents for the plaza. The hub's own node can be stopped and resumed, not revoked.

| Feature | Console | Status | Note | Source |
|---|---|---|---|---|
| Node list and state | Web | offered | The machines with their state and runs. | `apps/web/src/views/nodes.ts` |
| | Desktop | offered | Each machine opens into its runs, VMs and technical details. | `apps/desktop/src/console/screens/nodes.rs` |
| | CLI/TUI | partial | CLI `nodes list`, `nodes show`. TUI Nodes tab, read-only. | `apps/cli/src/cmd/nodes.rs`, `apps/cli/src/tui/app.rs` |
| Stop and resume a node | Web | offered | Stop now or after their turns; resume. The agents move to other nodes. | `apps/web/src/views/nodes.ts` |
| | Desktop | offered | Stop (now or after their turns), Resume. | `apps/desktop/src/console/screens/nodes.rs` |
| | CLI/TUI | partial | CLI `nodes stop`, `nodes resume`. TUI has only the plaza-wide STOP ALL and resume. | `apps/cli/src/cmd/nodes.rs`, `apps/cli/src/tui/app.rs` |
| Rebalance | Web | offered | Rebalance spreads the agents over the nodes. | `apps/web/src/views/nodes.ts` |
| | Desktop | offered | Rebalance. | `apps/desktop/src/console/screens/nodes.rs` |
| | CLI/TUI | partial | CLI `nodes rebalance`. TUI has none. | `apps/cli/src/cmd/nodes.rs` |
| Join codes and revoke | Web | offered | Create join codes (each token shown once); revoke a node. | `apps/web/src/views/nodes.ts` |
| | Desktop | offered | Join codes; revoke. | `apps/desktop/src/console/screens/nodes.rs` |
| | CLI/TUI | partial | CLI `nodes code`, `nodes codes`, `nodes revoke`. TUI has none. | `apps/cli/src/cmd/nodes.rs` |
| This machine joins another network | Web | offered | Join card with a code; leave. | `apps/web/src/views/nodes.ts` |
| | Desktop | offered | Join and Leave through `/v1/node-link`. | `apps/desktop/src/console/screens/nodes.rs` |
| | CLI/TUI | partial | CLI `node join`, `node status`, `node leave`. TUI has none. | `apps/cli/src/cmd/node.rs` |

## Workspace (the IDE)

The IDE works in an agent's workspace, a project repository at one branch, or a workstation's folder.

| Feature | Console | Status | Note | Source |
|---|---|---|---|---|
| Browse, edit, diff, search | Web | offered | Tree, file editor with a conflict check on save, changes against a base, search. | `apps/web/src/views/ide.ts`, `apps/web/src/api.ts` (`ideApi`) |
| | Desktop | offered | Tree, editor, diff, files and search screens. | `apps/desktop/src/console/screens/ide.rs`, `apps/desktop/src/console/screens/ide_files.rs` |
| | CLI/TUI | partial | CLI `ide ls`, `cat`, `edit`, `put`, `diff`, `search`. TUI Workspace tab shows the agents, transcript and board; `o` opens the IDE; no file edits. | `apps/cli/src/cmd/ide.rs`, `apps/cli/src/tui/workspace.rs` |
| Checkpoints and restore | Web | offered | List checkpoints; restore one. | `apps/web/src/views/ide-data.ts` |
| | Desktop | offered | Same, in the IDE screens. | `apps/desktop/src/console/screens/ide_files.rs` |
| | CLI/TUI | partial | CLI `ide checkpoints`, `ide restore`. TUI has none. | `apps/cli/src/cmd/ide.rs` |
| Runs and terminal | Web | offered | Bounded runs with their output, stop; a terminal drawn with xterm.js. | `apps/web/src/views/ide-tabs.ts`, `apps/web/src/views/ide-term-host.ts` |
| | Desktop | offered | Terminal tabs; Run and Stop in the panels. | `apps/desktop/src/console/screens/ide_terminal.rs`, `apps/desktop/src/console/screens/ide_panels.rs` |
| | CLI/TUI | partial | CLI `ide run`, `ide term`. TUI has none. | `apps/cli/src/cmd/ide.rs` |
| Agent chat in the workspace (send, stop, approve or deny a tool call) | Web | offered | The IDE conversation: send, stop, approve or deny. | `apps/web/src/views/ide-chat.ts` |
| | Desktop | offered | The IDE chat: send, stop, approve or deny. | `apps/desktop/src/console/screens/ide_chat.rs` |
| | CLI/TUI | partial | CLI `agents tell` sends a message. TUI composer (`t`) on the Workspace tab; tool calls are decided on the Approvals tab. | `apps/cli/src/cmd/agents.rs`, `apps/cli/src/tui/app.rs` |
| Branches and pull requests (`/branch`, `/pulls`) | Web | missing | The IDE routes exist; no console calls them. | `docs/api/openapi.json` |
| | Desktop | missing | The same routes, not called. | `docs/api/openapi.json` |
| | CLI/TUI | missing | The same routes, not called. | `docs/api/openapi.json` |

## The wasm crates

- `apz-engine-web`: part of the web console (its 3D views, through `apps/web/src/views/engine-mount.ts`). Not the work engine.
- `apz-graph-web`: part of the web console's Work screen (`apps/web/src/graph/wasm.ts`). The desktop lays out the same
  graphs with `apz-graph` directly.
- `apz-ide-web`: part of the web IDE (`apps/web/src/ide/wasm.ts`).
- `apz-theme-web`: part of the web console's theme editor and appearance (`apps/web/src/theme/wasm.ts`).

## Gaps the owner should see

Web console, missing (the hub has the route or the data):

- Seat threads (`/v1/tasks/{id}/threads`, `/v1/threads/{id}`), the chat transcript, and task claim and finish: no console calls them.
- A disk quota on a project's budget: no console sends it.
- Branches and pull requests of an IDE workspace (`/branch`, `/pulls`): no console calls them.
- Landing actions: the landing queue is read-only on the web and the desktop; `weft land` is on the CLI only.

Desktop, beyond those: the change recipe has no screen (`/v1/recipes` is not called); the review graph is replaced by
the IDE diff ("Review changes"); a chat's model is set only from the IDE chat tools.

The TUI has the Overview, Sessions, Tasks, Approvals, Inbox, Board, Nodes, Machines, Workspace, Workstations, Chat and
Providers tabs (`apps/cli/src/tui/app.rs`). It reads and answers the work engine; it does not configure it. The CLI
has a subcommand for nearly every row above.

## Not in this file

The Board, Metrics, Lab, Models, Tools, MCP, Vault, Egress credentials, E2B, Volumes, Prompts, Skills, Library, Rules,
Git platforms, Appearance, Setup, Machines and Workstations screens are not work-engine features and are not listed.
