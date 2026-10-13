# Web and desktop parity tracker

Owner's rule: the web console and the desktop app have 1:1 parity, in full. This file tracks what one side has and the
other lacks, from an audit on 2026-10-13. The audit compared the hub API calls each screen makes, plus the shared UI
features. Tick an item off when it lands, with its commit.

The 3D views are a separate effort: the shared Rust engine replaces three.js on the web and serves the desktop
(`docs/design/engine.md`). Once the engine has them, both apps get the same Floor, Machines thumbnails, portraits and
labs.

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

## Out of scope

**Hidden labs.** The hardware, robot and room labs are developer tools behind hidden routes on the web and environment
switches on the desktop. They move into the engine, where both apps get them at once.
