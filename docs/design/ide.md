# Open IDE

Status: built (2026-10-14), with the differences below. Phases 1 to 5 are on the hub, in the web and in the desktop app;
the CLI has `agentplaza ide`, including `ide term`. The layout is the agent-first one in `workspace.md` (section 2),
which supersedes section 5's layout. Section 1 gains a third workspace kind, the station. The gaps listed in section 4
are closed, except that checkpoints exist only for agent workspaces (section 6.3).

What differs from this plan:

- **Terminal.** The web uses xterm.js, not Ghostty's wasm build. The desktop draws `apz-vt` (a VT screen over `vte`) on
  an iced canvas. The PTY is always in the VM or on the workstation, never on the desktop (open question 9).
- **Repo writes.** A repo edit is a commit made through the Forgejo contents API as the admin token, the token the Git
  screen already uses (open question 2, kept as built).
- **Owner actions skip the laws classifier** (open question 4: no). Each owner write, branch, pull request, restore,
  run and stop is written to the audit (`ide_audit`).
- **Restore** is for agent workspaces. A repo uses git revert, and a station has no checkpoints.
- **Window and shell.** The desktop IDE is in the same window (open question 10). No local shell (open question 7 is
  unchanged).
- **Phase 6.** The command palette (Ctrl/Cmd+K) is built on both apps. Nothing else from phase 6 is claimed here.

Known limits: a station workspace has no checkpoints, changes, restore or branch, and STOP ALL refuses every operation
on it (`crates/apid/src/ide.rs`).

Owner's request: an "Open IDE" button in the top bar of the web console (`apps/web`) and the desktop app
(`apps/desktop`). It opens an IDE view modelled on t3code (https://github.com/pingdotgg/t3code). We take what fits
the plaza and adapt it to our workflow and styling. Where we can, the logic is Rust: natively in iced on the
desktop, and shared with the web through wasm.

## 1. Goal and scope

"Open IDE" opens a workspace. A workspace is one of two kinds:

- **Agent workspace.** The `/workspace` of one agent's VM. This is the live working copy of an agent. The agent may
  be writing to it while the owner looks. Its work is saved as checkpoints under `<workspace>/.plaza/checkpoints/`.
- **Project repo.** The Forgejo repo of a project (org `agentplaza`), at one branch. Edits become commits and pull
  requests.
- **Station.** An online workstation's work folder (`station:<id>`, see `docs/design/workstations.md`). Its files,
  runs and terminals go to the workstation's daemon. It has no checkpoints, so no changes, restore or branch.

Per-agent and per-project both work. The IDE state is keyed by the workspace id: `agent:<cid>`, `repo:<name>@<ref>` or
`station:<id>`.

Agents work in VMs. The IDE never reaches the host. It goes through the hub to the VM (section 6.4).

In scope: view and edit files, see what changed (since a checkpoint, or against a base branch), approve an agent's
pending writes, run a bounded command, and (last) a terminal in the VM.

Out of scope: multi-user live editing, extensions, language servers, debuggers, remote hosts outside the hub, a host
shell, and any second store for plaza data. The IDE does not replace agent memory or tools.

Entry points: the top bar button; agent detail "Open workspace"; project page "Open repo"; task page "Review changes"
(the diff since the checkpoint taken when that work began); Git screen "Open in IDE" on a file or pull request diff.

## 2. What we take from t3code

What t3code is. It is an "agent harness control surface": a GUI and a server that run on the owner's machine and
drive coding agents (Claude Code, Codex, Cursor, Grok Build, OpenCode, Antigravity) through their own CLIs. It has a
web app, an Electron desktop app and mobile apps. The README says we can fork it "to build the editor that you want".
The repo says it is early and mostly does not take outside contributions.

License. MIT, "Copyright (c) 2026 T3 Tools Inc." (LICENSE, read on 2026-10-14). The MIT license lets us use, copy,
modify, merge, publish, distribute, sublicense and sell the code. The condition: the copyright notice and the
permission notice stay in every copy or substantial portion. This plan copies no code. It takes ideas and behaviour.
If we port a file, keep the notice in that file's header and list it in our third-party notices. Their dependencies
have their own licences: `@pierre/diffs`, `@pierre/trees`, the Ghostty wasm build, tiptap. Check each before use.

Architecture fact we keep (`docs/internals/overview.md`): the server owns provider processes, terminals, Git and project
files, and clients control it over authenticated RPC. A client never substitutes its own filesystem. This matches our
rule: the IDE edits only what the hub owns. We do not copy their event-sourced orchestrator. We keep `/v1/ws`.

Feature by feature:

| t3code feature | Take? | What we change |
|---|---|---|
| Project and thread sidebar | Take the idea | Our sidebar already lists projects, agents and tasks. The IDE adds a workspace picker, not a second sidebar. |
| Chat with an agent | Skip as new work | Agents already have transcripts and conversations. The IDE shows the agent's transcript in a side panel and sends steer notes through `/v1/agents/{cid}/control`. |
| Per-turn diff ("review turn changes") | Take | Our unit is the checkpoint. The default diff is "since this work began", the same as the agent's `checkpoint_diff` tool. |
| Diff panel with "viewed" marks and find | Take | Diffs come from the hub as hunks. Viewed marks are kept in the browser (per viewer, so `localStorage` is fine there). |
| File tree with changed-file filter | Take | Tree from the hub. Filter to files changed since the checkpoint. |
| Editor (tiptap composer in t3) | Take, differently | A code editor, not a rich-text composer. Web: CodeMirror 6. Desktop: iced `text_editor` with its highlighter. |
| Terminal (Ghostty wasm in t3) | Take in phase 5 | Needs a PTY inside the VM, which we do not have (section 4). |
| Git actions (commit, push, PR) | Take partly | Commit, push and PR creation. Merge and review stay on the Git screen. |
| Model picker | Skip | Models are set on the agent (`PATCH /v1/agents/{cid}`). Not an IDE concern. |
| Approvals inline | Take | Reuse `/v1/approvals`. Show each pending agent write with its diff. |
| Command palette (Cmd+K) | Take later | Phase 6. Commands, files, agents, workspaces. |
| Keybindings file, remote access, mobile, provider sign-in, event store, VS Code themes, browser pane | Skip | Fixed defaults first. The hub is the only server. We have our own checkpoints and bus. Our palette only. |

## 3. What we already have

Reuse these. Do not build a second copy.

- **Agents, conversations, tasks, projects.** `/v1/agents/{cid}` (`/profile`, `/transcript`, `/control`, `/permissions`),
  `/v1/conversations`, `/v1/tasks/{id}`, `/v1/projects/{id}`. Each has a web view (`views/agent-detail.ts`,
  `views/tasks.ts`, `views/projects.ts`) and a desktop screen (`screens/agent.rs`, `screens/tasks.rs`,
  `screens/projects.rs`). Each project gets a Forgejo repo when opened.
- **Git.** `/v1/git/repos/{repo}` with `tree`, `file`, `branches`, `commits`, `pulls`, review and merge. A pull's
  `diff` is in its detail (`crates/apid/src/git.rs` around line 778). Web: `views/git.ts`. Desktop: `screens/git.rs`,
  which already browses trees and files read-only.
- **Approvals.** `crates/apid/src/approvals.rs`: a queue of held tool calls, with per-agent modes `autonomous`, `ask`,
  `read_only`. Web: `views/approvals.ts`. Desktop: `screens/approvals.rs`.
- **Checkpoints.** `crates/agent-harness/src/checkpoints.rs`: snapshot, diff, restore, list. Stored by content hash.
  The agent reaches them through `checkpoint_diff`, `checkpoint_list` and `checkpoint_restore`
  (`crates/apid/src/tools.rs` around lines 320 to 334). The owner cannot reach them through the hub yet.
- **Agent file tools.** `crates/agent-harness/src/agent.rs` (around lines 643 to 660): `bash`, `glob`, `grep`,
  `edit_file`, `checkpoint_*`. Their path checks are the base for the owner's file API.
- **Event bus.** `GET /v1/ws` (`crates/apid/src/events.rs`). It is read-only: a client message closes the socket.
  Socket caps per peer and in total.
- **Files.** `/v1/files` is the Library of deliverables. It is not a workspace. Keep the two apart.
- **Visual system.** `apps/web/src/styles/tokens.css` (palette, fonts, square corners, print shadow). Desktop:
  `apps/desktop/src/theme.rs` (`Palette`) and `apps/desktop/src/console/ui.rs` (helpers: `mono`, `btn`, `tabs`,
  `seg`, `sheet`, `card`, `view_head`, `empty`, `markdown`).

## 4. Gaps

1. **No owner file API into a workspace** (list, read, write, search). Only the agent's own tools reach its files.
2. **No owner checkpoint API.** Diff, list and restore exist inside the harness only.
3. **Git has no compare or single-commit diff, and no writes** apart from review and merge. Needed in phase 3.
4. **No PTY.** `/v1/runs/{id}/input` proxies to noded, and the harness `bash` tool is one-shot. A terminal needs a PTY
   in the guest and a two-way socket. `/v1/ws` is one-way (a client message closes it).
5. **Desktop highlighting is off.** iced 0.14.0 has a `highlighter` feature (syntect based). Our
   `apps/desktop/Cargo.toml` does not enable it.
6. **No shortcut layer** in either app. The desktop binds one key (`[`, the sidebar).

## 5. UX

Superseded: the layout below is the first plan. The built layout is the agent-first one in `workspace.md`, section 2.
The button and the entry points below still hold.

**The button.**

- Web: in `topbar()` (`apps/web/src/main.ts` around lines 54 to 65), before the `health` group. Label "OPEN IDE",
  Mono uppercase like the other top-bar text. Icon: lucide `square-code` (check it exists in `ui/icons.gen.ts`).
- Desktop: in `top_bar()` (`apps/desktop/src/console/shell.rs` around line 529), in the right cluster before the
  theme toggle, using `top_btn`-style chrome. Same label and icon (check `console/icons.rs`).
- Narrow widths: icon only, with the label as the tooltip.
- With no workspace chosen, the button opens the picker: agents that have a VM, and projects that have a repo. It
  remembers the last workspace per app.
- Shortcut: `mod+shift+e` (VS Code uses it for Explorer). Neither app binds it now. Check it against browser
  shortcuts on Linux, Windows and macOS before we commit to it (open question 5).

**The IDE layout.** One screen in the same window as the console.

- Top strip: workspace picker (kind, name, branch or checkpoint), changed-files count, state badge (`idle`,
  `agent working`, `saved`, `conflict`), and actions (Save, Commit and Open PR in phase 3).
- Left: file tree, with a filter to "changed since checkpoint".
- Centre: editor tabs. Read-only, and labelled as such, while the agent is writing that file.
- Right (bottom on narrow widths): tabs for **Changes** (diff, default base: since this work began), **Agent**
  (live transcript, steer box), **Approvals** (held agent writes with diff; approve or reject), **Run** (phase 4) and
  **Terminal** (phase 5).
- Keys: Cmd/Ctrl+S save, Cmd/Ctrl+P quick open (phase 2), Cmd/Ctrl+F find, Escape back to the origin screen.
- Review mode replaces the centre with the diff view. A task or an approval opens in review mode.

## 6. Architecture

### 6.1 Shared model

A new crate `crates/apz-ide`. Pure Rust, no I/O, no iced, no wasm-bindgen. Dependencies: `serde`, `serde_json`.

- `Workspace`: id, kind (`agent` or `repo`), owner (cid or repo name), ref, root, writable.
- `Entry`: path, kind, size, content hash, binary flag.
- `Buffer`: path, base hash, text, dirty. Save sends the base hash so the hub can detect a conflict.
- `ChangeSet`: files with status (added, modified, deleted, renamed), and hunks for each.
- `UnifiedDiff` parser: the hub returns a unified diff; the crate turns it into hunks. The hub does the diffing.
- Path rules: relative paths only, no `..`, no absolute paths, no writes under `.plaza/`.
- `IdeState` reducer: which tabs are open, the active panel, the base for the diff. Pure functions, so both apps
  behave the same.

Tests: plain unit tests in the crate. Parser fixtures from real `git diff` output.

### 6.2 Where the code goes

- `crates/apz-ide`: the model above.
- `crates/apz-ide-web`: a wasm-bindgen crate, built the way `crates/apz-engine-web` is (`cdylib` and `rlib`,
  wasm-bindgen pinned, `=0.2.129` there). It exports the parser, the tree helpers and the reducer to TypeScript.
- `crates/apid/src/ide.rs`: the routes below. Phase 5 adds `crates/apid/src/ide_term.rs` for the terminal socket.
- `crates/agent-harness`: handles the new workspace frames. It reuses `checkpoints.rs` and the path checks in
  `agent.rs`. The new frames go into `harness_control.rs`'s `Control` set (`apz-proto` wire), so no new vsock port.
- Desktop: `apps/desktop/src/console/screens/ide.rs` (screen), `ide_editor.rs` (text_editor with highlighter),
  `ide_diff.rs` (diff view), `ide_tree.rs` (tree widget), and later `ide_terminal.rs`. The screen joins the `Id`
  routing in `screens/mod.rs`, and the button joins `shell.rs`.
- Web: `apps/web/src/views/ide.ts`, `apps/web/src/styles/ide.css`, and an `ide` section in `api.ts`.

### 6.3 Hub API (proposed; the names follow the `/v1/<area>` style)

Owner-only, behind the console session like the other `/v1` routes.

- `GET  /v1/ide/workspaces`: agent workspaces (agents with a VM) and repo workspaces (projects with a repo).
- `GET  /v1/ide/workspaces/{id}/tree?path=&cursor=`: one page of entries (500 per page).
- `GET  /v1/ide/workspaces/{id}/file?path=`: text, hash, size. Refuses binary and files over 1 MiB.
- `PUT  /v1/ide/workspaces/{id}/file`: `{path, text, expected_hash}`. 409 on a hash mismatch.
- `GET  /v1/ide/workspaces/{id}/changes?base=checkpoint:<n>|branch:<name>`: changed files and the unified diff.
- `GET  /v1/ide/workspaces/{id}/checkpoints`, `POST .../checkpoints/{n}/restore`.
- `GET  /v1/ide/workspaces/{id}/search?q=&glob=`: bounded.
- `POST /v1/ide/workspaces/{id}/run`: `{command, timeout_s}` (phase 4). Output is read by polling, then streamed on
  `/v1/ws` if that is enough.
- Repo kind (phase 3): `GET .../compare/{base}...{head}`, `GET .../commits/{sha}/diff`, `POST .../commit`
  (`{branch, message, files}`), `POST .../pulls`.
- `WS /v1/ide/terminal/{id}` (phase 5): two-way.
- Approvals: reuse `/v1/approvals` and `/v1/approvals/{id}/decide`.

### 6.4 Data path for agent workspaces

Hub (`apid`) to the VM. For a VM on a node, the call goes into the node's command queue (`node_link.rs`, `nodes.rs`).
`apz-link` long-polls that queue and runs the command against the local `noded` and `vsockd`. The harness then
answers over its one vsock session (port 5000). Consequences:

- Every IDE call is a round trip through a long poll. Keep payloads small and page the tree.
- Harness file ops must not block the agent's turn. Serve them on their own lane inside the harness. If that is not
  ready in phase 1, the IDE is read-only while the agent works.
- Cache the tree per checkpoint in the hub, not in the browser.

### 6.5 Security

- **Only owned workspaces.** Agent `/workspace` and project repos. No host paths.
- **Paths.** Relative, no `..`, no symlink that resolves outside the root (checked in the harness, where the real
  filesystem is). `.plaza/` is readable, never writable.
- **Secrets.** A deny list for known secret paths (`.env*`, key and credential files). Enforced in the hub and the
  harness, listed in `apz-ide`.
- **Agents never reach the host.** The terminal and runs go into the VM. No local shell on desktop in v1.
- **Owner writes.** No approval, since the owner is the principal. Each write takes an automatic checkpoint labelled
  "owner edit" first, and is logged (who, path, hash before and after) in an append-only table.
- **Agent writes in `ask` mode** stay held in `/v1/approvals`. The IDE shows the held diff. Approving releases the
  call; the IDE gets no extra write path.
- **Laws.** `crates/apid/src/laws.rs` classifies tools. Owner runs should pass the same classifier (open question 4).
- **Transport.** Console session on every route. The terminal socket checks the origin like `/v1/ws` (`SameHost`) and
  has socket caps like `events.rs`. WebSockets only, no SSE (project rule). Runs have a timeout and an output cap.

### 6.6 Desktop iced pieces

- **Editor.** `iced::widget::text_editor` (already used by `screens/prompt_editor.rs`) with the `highlighter` feature.
  The syntax theme is ours, mapped from the palette. No LSP and no multi-cursor in v1.
- **Diff viewer and file tree.** Custom widgets. The hub sends the diff, so the desktop needs no diff engine. iced 0.14
  core has no tree widget.
- **Terminal (phase 5).** The PTY lives in the VM, so no `portable-pty` on the desktop. It needs a VT grid and parser,
  drawn on an iced `canvas`. Candidates, not yet checked for licence or wasm: `alacritty_terminal`, `vte`,
  `wezterm-term`, `libghostty-vt`.

### 6.7 Web pieces

- Plain TypeScript with the existing `h()` helpers. No React, which t3code uses; we add no framework.
- Editor: CodeMirror 6 (MIT). Monaco is too large for this console. Diff and tree data come from `apz-ide-web`.
- Terminal (phase 5): xterm.js (MIT) or Ghostty's wasm build, after a licence check.

## 7. Styling

Use the existing tokens. Add no new colours.

- Web (`apps/web/src/styles/tokens.css`): `--paper` and `--sheet` for the editor surface, `--ink` for text, `--ink-3`
  for line numbers, `--rule` for gutters, `--signal` for the active line, `--ok-ink` and `--crit` for add and remove.
  Syntax: keyword `--signal-ink`, string `--ok-ink`, number `--warn-ink`, type `--info-ink`, comment `--ink-3`. Dark
  theme follows the existing `[data-theme]` blocks. Code in `--mono`, prose in `--serif`, square corners.
- Desktop (`apps/desktop/src/theme.rs` `Palette`, `console/ui.rs` helpers): the same roles from `Palette`. Chrome from
  `view_head`, `tabs`, `seg`, `btn`, `card`, `sheet`, `empty`, `badge`. Icons from the lucide set; check
  `console/icons.rs` for `square-code`.

## 8. Phases

Each phase ships on its own. The top-bar button appears in phase 1, once the read-only view works. Until then it is
hidden behind a setting.

**Phase 1. Read-only IDE.** Spike first (two or three days): the desktop `text_editor` with the highlighter on one
file, a CodeMirror page on one file, and a build of `apz-ide` for wasm. Then: `apz-ide` model and parser; hub routes
`workspaces`, `tree`, `file`, `changes`, `checkpoints` (GET only); harness frames for list, read and diff; the button
and the entry points in both apps.
Done: an owner opens any agent workspace on web and on desktop, browses the tree, reads files, and sees the changes
since the agent's last checkpoint. The button is in both top bars.
Tests: `apz-ide` unit tests (diff fixtures, path rules, deny list); apid route tests against an apz-proto fixture;
harness tests beside `checkpoints.rs`; an `iced_test` screen test on desktop; a fixture view test on web. Visual check
with the app's own snapshot (`APZ_SNAPSHOT`), never a grab of the owner's screen.

**Phase 2. Owner edits and checkpoints.** `PUT file` with `expected_hash`; conflict handling (409 shows a banner
with "reload" and "keep mine"); automatic "owner edit" checkpoint before each write; checkpoint restore from the
IDE; search and quick open; the audit log of owner writes.
Done: the owner edits and saves a file in an agent workspace. A save over an agent's newer write returns 409 and the
UI shows the conflict. Restore brings back a checkpoint.
Tests: stale-hash and concurrent-write conflict tests; a restore round trip; an audit entry test.

**Phase 3. Repos.** Read through the existing git routes plus compare and single-commit diff. Commit and push with
the Forgejo contents API as the chosen identity (open question 2). Open a PR from the IDE. Merge and review stay on the
Git screen.
Done: the owner edits a project file on a branch, commits, opens a PR, and sees it on the Git screen.
Tests: `git.rs` unit tests with a mock Forgejo, and an integration run against the local Forgejo in Docker
(`127.0.0.1:3300`). A test checks the commit author.

**Phase 4. Approvals and runs.** The Approvals tab shows held agent writes with diffs; approve and reject from the IDE.
The Run tab runs one bounded command in the workspace (timeout, output cap, exit code, stop). The Agent tab shows the
live transcript and a steer box.
Done: an owner approves an agent's held edit from the IDE and sees the file change. The owner runs a test command in an
agent workspace and sees its output and exit code.
Tests: approval queue tests (`approvals.rs`) extended for the IDE path. Run tests in the harness: timeout kills the
process group, output cap holds, a run does not block an agent turn.

**Phase 5. Terminal.** A PTY service in the guest (inside the harness or a small guest binary, decided in the spike).
A two-way route `WS /v1/ide/terminal/{id}` with origin and socket-cap checks. The terminal renderer on web (xterm.js or
Ghostty wasm) and on desktop (VT grid plus iced canvas). Resize and reconnect. Closing the terminal does not stop the
agent's processes; a separate "kill session" does.
Done: the owner opens a shell in an agent VM, types, resizes, and reconnects after a reload.
Tests: PTY round trip in the harness; socket tests in apid; renderer tests on the grid crate. Desktop Windows build
checked the way our Windows notes describe (cargo check for windows-msvc, and Windows tests in CI on a branch).

**Later (phase 6, optional).** Command palette (Cmd+K), a keybindings file, split panes, tab persistence across
reloads, theme polish.

## 9. Risks

- **Harness concurrency.** File ops during a turn could slow the agent. Mitigation: a separate lane, or read-only
  during a turn until the lane exists.
- **Owner and agent write the same file.** Mitigation: hash check on save, automatic checkpoint, a clear conflict UI.
- **Remote node latency.** Each call waits on the node long poll. Mitigation: page the tree, cache per checkpoint.
- **Scope creep into a full IDE.** Hold to the non-goals in section 1.
- **Desktop highlighting cost on big files.** Mitigation: the 1 MiB read cap; check in the spike whether iced's
  highlighter handles visible lines only.
- **Unchecked licences and builds.** `alacritty_terminal`, `libghostty-vt`, Ghostty's wasm build, `@pierre/diffs`,
  `@pierre/trees`, CodeMirror 6, xterm.js. Check licences in the phase 1 and phase 5 spikes, and the wasm build of
  the terminal grid in phase 5.
- **Laws.** An owner command could hit a law the agents do not. See open question 4.

## 10. Open questions

1. Owner writes to an agent workspace: allowed silently with a checkpoint and audit (proposed), or does the owner
   have to add a note?
2. Repo commits: which Forgejo identity? The admin token (as the Git screen uses it now), or a dedicated owner user?
3. Should the IDE open the AgentPlaza source repo itself? Proposed: no. Only agent workspaces and project repos.
4. Should owner runs and writes pass the laws classifier (`laws.rs`)?
5. Shortcut `mod+shift+e`: check it against browser and OS shortcuts before we commit.
6. Icon `square-code`: confirm it is in the web icon set and in `console/icons.rs`.
7. Desktop shell: a local shell is out of scope for v1. Revisit later?
8. "Changes" default base: the checkpoint at the start of the current work (proposed, as the agent's own tool does),
   or the branch base?
9. Terminal library: pick after the phase 5 spike (alacritty_terminal, libghostty-vt, wezterm-term, or xterm.js on the
   web side).
10. Same window or a separate window on desktop?
