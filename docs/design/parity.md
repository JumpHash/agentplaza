# Web and desktop parity tracker

Owner's rule: the web console and the desktop app have 1:1 parity, in full. This file tracks what one side has and the
other lacks, from an audit on 2026-10-13. The audit compared the hub API calls each screen makes, plus the shared UI
features. Tick an item off when it lands, with its commit.

The 3D views are a separate effort: the shared Rust engine replaces three.js on the web and serves the desktop
(`docs/design/engine.md`). Once the engine has them, both apps get the same Floor, Machines thumbnails, portraits and
labs.

## Desktop gaps (web has it)

| # | Screen | Missing on desktop | Size |
|---|---|---|---|
| 1 | Vault | Unseal and seal, slots (passphrase, key file), recovery code, rekey | M |
| 2 | Vault | Secret create, update and remove (the secret form), rollback, test | M |
| 3 | Vault | `.env` import, audit log with chain verify, consumers per node matrix | M |
| 4 | MCP | The whole screen: catalogue, servers list and detail, start, stop, test, tools, logs | L |
| 5 | MCP | Add and edit wizard, with secrets and the test step | L |
| 6 | MCP | *This hub as a server*: modes, tokens, call log | M |
| 7 | Git | Code browser: branches, commits, tree, file viewer | M |
| 8 | Git | PR list and detail (to verify) | S-M |
| 9 | Nodes | Node link: status, join by code, leave; per-node machine and runs | M |
| 10 | Sign-in | Connecting to a remote hub: owner-token paste, hub identity, sign-out | M |
| 11 | Machines | Row expands into the live 3D orbit view (engine) | M |
| 12 | Memory | Memory graph view | M |
| 13 | Approvals | Action kinds `lift`, `acknowledge`, `retire` (to verify) | S |
| 14 | Lab | Model lifecycle `shadow`, `promote`, `retire` (to verify) | S |
| 15 | Agent profile | 3D portrait (engine) | S |
| 16 | Shell | Toasts, a theme switch, conversation search and pagination, task withdraw, connector secret clear | S |

## Web gaps (desktop has it)

| # | Screen | Missing on the web | Size |
|---|---|---|---|
| 1 | Setup | First-run checks and boot screen (`setup/checks.rs`, `boot.rs`) as a web first-run page | S |

## Out of scope

**Hidden labs.** The hardware, robot and room labs are developer tools behind hidden routes on the web and environment
switches on the desktop. They move into the engine, where both apps get them at once.
