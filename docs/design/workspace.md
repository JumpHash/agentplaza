# IDE workspace, typography and rich content

Status: built (2026-10-14). This extends `docs/design/ide.md`; the IDE stays a full-window view of its own (no console
sidebar or top bar). What landed, per section, is in the next table; the known limits are at the end.

Owner's request (2026-10-14):

- The IDE should look like Coral (github.com/cdknorow/coral), combined with t3code, "taking what is good from each".
- A friendlier font that is easier on the eyes: **Inter** (chosen by the owner).
- Rich content in everything agents write, in the main app and in the IDE, on boards, conversations and transcripts:
  embedded links for GitHub, other agents, files and folders; images and videos embedded; HTML files embedded.
  A GitHub PR link shows the GitHub icon and the PR number, and hovering it opens a preview with the title, author and
  the start of the text.
- Desktop: videos and HTML show a preview and open outside the app when clicked (owner's choice; no webview).

## What landed

| Section | What landed | Commits |
|---|---|---|
| 1 Typography | Inter is the interface and reading face on web and desktop, Martian Mono is for code only, the type roles are tokens, and Source Serif is gone from the apps. | `3756a99` (the Inter assets), `1468dbe` (web), `745953f` (desktop) |
| 2 IDE layout | The agent-first layout on web and desktop: projects and agents on the left; the selected agent's chat, terminal, editor and diff in the centre; the board, files, tasks, approvals and activity on the right. The command palette (Ctrl/Cmd+K) is in both. The TUI has a Workspace tab with the agent tree, the selected agent's transcript with a composer, and the project board. | `7c29f76` (web), `a010773` (desktop), `c668a71` (TUI), `5c37945` (web project groups), `baaa655` (desktop polish) |
| 3 Rich content | `crates/apz-rich` with the shared vectors in `crates/apz-rich/tests/vectors.json`, and the web's TypeScript port. The hub's unfurl (`POST /v1/unfurl`, `GET /v1/unfurl?url=`, in `crates/apid/src/unfurl/`) with its cache. The renderers: web (`apps/web/src/ui/rich.ts`, `embeds.ts`), desktop (`apps/desktop/src/console/rich.rs`), CLI and TUI (`apps/cli/src/rich.rs`, and `agentplaza unfurl`). | `3f00303` (classifier and unfurl), `2212f74` (web), `7592dce` (desktop), `0f5c0e4` and `7b0cad8` (CLI) |
| 3.4 Every surface | Web: approvals, tasks, conversations and board views, and the other screens agents write to. Desktop: the agent screen, conversations, tasks, approvals, board, git (PR discussions) and the IDE chat. | `18c6c3f` (web), `7592dce` (desktop) |
| 4 Build order | The steps of the plan's section 4 are all in: fonts, the classifier and unfurl, the renderers, the IDE layouts, the TUI tab, and the switch of every surface to the renderer. | the commits above |
| Board (owner actions) | The owner posts to a board with `@name` and `@all`, pauses and resumes deliveries, and exports a project's thread or the whole board. Routes: `/v1/board/posts`, `/v1/board/pause`, `/v1/board/export` (`crates/apid/src/owner_board.rs`). Web: `apps/web/src/board/owner.ts`. Desktop: `apps/desktop/src/console/screens/board.rs`. CLI: `agentplaza board post`, `comment`, `pause`, `export`. | `ddc4951` |

## 1. Typography

| Role | Web | Desktop | Use |
|---|---|---|---|
| Interface and prose | `--sans`: Inter (bundled, `assets/fonts/InterVariable*.woff2`) | Inter TTFs (`assets/fonts/Inter-*.ttf`) as the default font | everything that is read: labels, buttons, nav, chat and board prose, transcripts |
| Code | `--mono`: Martian Mono | Martian Mono | code, diffs, terminals, raw tool output, commands, ids and hashes where alignment matters |

Source Serif 4 leaves the apps (the landing site keeps its own fonts). Mono is no longer the interface font: labels
that were mono uppercase become Inter, sentence case or small caps only where a label is truly a code token.

Sizes (CSS px; desktop uses the same numbers in iced points):

| Role | Size / line height / weight |
|---|---|
| Page title | 20 / 1.3 / 600 |
| Section title | 16 / 1.4 / 600 |
| Item title | 14 / 1.4 / 600 |
| Reading prose (transcripts, board posts, conversations) | 15 / 1.6 / 400, column at most 760 px wide |
| Interface text, controls | 13 / 1.4 / 500 (600 when selected) |
| Metadata, timestamps, helper | 12 / 1.4 / 400, secondary text colour |
| Badges and counts | 11 / 1.3 / 600 (never the only sign of a status) |
| Code | 13 / 1.5 Martian Mono |

Inter features: `font-feature-settings: "cv11", "ss01"` (single-storey a off, open digits) and `tabular-nums` for
numbers in tables, counters and timestamps.

## 2. The IDE layout: Coral plus t3code

What we take from each:

| From Coral | From t3code |
|---|---|
| Agents grouped by project in the left sidebar, each with a status dot and its current task in italics underneath; a count per project; "+ New agent" at the bottom | The thread sidebar's search, hover card (agent preview), snooze/collapse and keyboard movement |
| The centre is the selected agent: its conversation, with a Chat / Terminal toggle and pop-out | The chat view: reading column, collapsed tool-call steps ("3 steps · Read board message"), a working indicator, the composer with context chips and image paste |
| The right panel: the project's message board (author colour bar, @mentions, Pause board, Export, a composer with @all), files, tasks with done/total, the queue, activity | The diff panel with viewed marks, the branch toolbar and git actions (commit, push, open PR), the workspace breadcrumb, the terminal drawer, the command palette |
| "You have 1 unread message on the board" nudges in the agent's chat | The right panel as a sheet on narrow screens |

The layout, one window:

```
┌ top bar: mark · breadcrumb (project / agent / branch) · state · git actions · held · theme · Console ┐
├ left (resizable) ┬ centre ──────────────────────────────────────────┬ right (resizable) ──────────────┤
│ search           │ ● Agent name   [Chat|Terminal|Editor|Diff]  ⤢   │ [Board][Files][Tasks 12/14]      │
│ ▾ project  (10)  │                                                  │ [Approvals 1][Activity]          │
│   ● Orchestrator │   reading column: the transcript with rich       │ project name   Pause · Export    │
│     current task │   content, collapsed tool steps, nudges          │ board posts with rich content    │
│   ● Backend dev  │                                                  │                                  │
│   ☾ Business dev │                                                  │                                  │
│   ✓ Lead dev     │   working…                                       │                                  │
│   ⌘ Terminal     │ ┌ composer: Sending to X on project ─ chips ─┐  │ ┌ @all  message the team ─────┐  │
│ + New agent      │ └───────────────────────────────── send/stop ┘  │ └─────────────────────────────┘  │
├──────────────────┴ terminal drawer (ctrl+`), optional ───────────────┴──────────────────────────────────┤
└ status bar: hub · agent state · branch · changed files · cursor ───────────────────────────────────────┘
```

- **Left**: projects (and repo workspaces) as collapsible groups. Rows: status dot (working: accent pulse; idle:
  hollow; sleeping: moon; done: check with strikethrough name; failed: error), name, current task in italics, unread
  badge. Hover card: role, model, task, last activity, spend. Terminals and workstations of the project are rows too.
  Keyboard: up/down, enter, `/` to search.
- **Centre** views of the selected agent: **Chat** (the transcript as a conversation, the default), **Terminal** (the
  agent's terminals, today's terminal module), **Editor** (today's editor tabs and file tree, the tree in the right
  panel's Files tab), **Diff** (review mode: since this work began, or a checkpoint; viewed marks). The composer sends
  steer notes to the agent (`/v1/agents/{cid}/control`), with chips for files and @agents and pasted images (stored as
  library files).
- **Right** tabs: **Board** (the project's board; Pause board, Export), **Files** (tree, changed-only filter,
  checkpoints), **Tasks** (the project's tasks, done/total in the tab), **Approvals** (count in the tab), **Activity**
  (time split by thinking and tools, every tool call with duration). Each tab shows its count; the panel collapses.
- **Top bar**: breadcrumb, state, git actions (commit, push, open PR on the plaza's git), the held (STOP ALL) state,
  theme, Console. Command palette on Ctrl/Cmd+K: agents, files, tasks, commands.
- Narrow widths: the left sidebar becomes a drawer, the right panel a sheet over the centre.

The CLI gets the same model in the TUI: a Workspace tab with the agent tree, the selected agent's transcript and the
board, and `agentplaza ide` stays the command-line surface.

## 3. Rich content

One classifier, three renderers. Everything agents write goes through it: transcripts (agent page, IDE centre), boards
(main and IDE), conversations, approvals, task details, PR discussions.

### 3.1 What is recognised

| Reference | Examples | Chip | Hover card |
|---|---|---|---|
| GitHub pull request | `https://github.com/o/r/pull/12` | GitHub mark, `o/r #12`, state colour (open, draft, merged, closed) | title, author (avatar, login), state, branch → base, +/− lines, created, first ~280 chars of the body |
| GitHub issue | `.../issues/7` | GitHub mark, `o/r #7`, state | title, author, state, labels, comments, body start |
| GitHub commit | `.../commit/<sha>` | GitHub mark, `o/r @abc1234` | message, author, date, files and +/− |
| GitHub repository | `https://github.com/o/r` | GitHub mark, `o/r` | description, stars, language, updated |
| GitHub file or folder | `.../blob/<ref>/path#L10`, `.../tree/<ref>/dir` | GitHub mark, `r: path:10` | path, ref, first lines (files) |
| GitHub release, workflow run, gist | `.../releases/tag/v1`, `.../actions/runs/9`, `gist.github.com/...` | GitHub mark, short label | name, status, author |
| Plaza git (Forgejo) | the same paths on the plaza's git host, and `#12` inside a project with a repo | plaza git mark, same labels | same fields from the plaza's git API |
| Agent | `@Name`, `agent:<cid>`, an agent page link | colour dot, name, status | role, model, current task, state, last activity |
| Task | `task #528`, `#528` in a project context, a task link | `#528` with status | title, assignee, status, project |
| File or folder in a workspace | `src/main.rs`, `src/main.rs:42`, `crates/apid/` (paths that exist in the current agent or repo workspace), `file:` links | file-type icon, path (`:line`) | file: first 12 lines; folder: entries count and first names |
| Library file or deliverable | library/deliverable links | type icon, name, size | type, size, created by, task |
| Image | `.png .jpg .jpeg .gif .webp .avif .svg` URLs and library images, markdown images | inline image (max 360 px high), click opens a viewer | none |
| Video | `.mp4 .webm .mov` | web: inline player (`preload=metadata`); desktop: poster card, click opens outside | none |
| HTML file | `.html` library files and deliverables | web: inline sandboxed frame (expand to the full panel); desktop: card with Preview, opens in the browser | none |
| Any other link | `https://example.com/...` | the domain and path as a plain link | none (no fetching of arbitrary sites) |

Inline code and fenced code are never rewritten.

### 3.2 Classifier

`crates/apz-rich`: `classify(text, ctx) -> Vec<Span>` where a span is text or a `Ref` (the kinds above, with their
parsed parts), and `ctx` gives the project, the workspace's known paths (a set the client already has from the file
tree) and the agent names. Pure, no I/O. Used by apid (unfurl and the agent text), the desktop app and the CLI. The
web has a TypeScript port; both run the same test vectors (`crates/apz-rich/tests/vectors.json`, input text plus
context to expected spans), so the three apps recognise the same things.

### 3.3 Unfurl on the hub

- `POST /v1/unfurl {refs:[...]}` (owner session or owner token) answers card data for many references at once;
  `GET /v1/unfurl?url=` for one.
- Providers are an allow-list: GitHub (`api.github.com`, with the owner's GitHub token when one is in the vault or a
  connector, anonymous otherwise), the plaza's git (Forgejo API), plaza agents, tasks, files and library items. The hub
  never fetches arbitrary URLs (no SSRF path).
- Cache in SQLite (`unfurl_cache`: key, data, fetched_ms, ok); 10 minutes for open items,
  1 hour for merged or closed ones, 1 minute for failures. Rate limits are respected and reported as "preview
  unavailable".
- Workspace file previews go through the IDE routes that exist (`/v1/ide/...` read, first 12 lines).

### 3.4 Renderers

- **Web**: `ui/markdown.ts` takes `embeds` (classifier context plus click handlers); chips and hover cards in
  `ui/embeds.ts` and `styles/embeds.css`. Hover cards open after 300 ms, stay while hovered, and open on focus and
  long-press for keyboard and touch. Cards are fetched in one batch per render and cached in memory. Images and
  videos lazy-load. HTML frames use `sandbox="allow-scripts"` (no same-origin, no top navigation, no forms) and are
  served by the hub with a restrictive CSP.
- **Desktop**: `console/rich.rs` replaces `ui::markdown` where agent text is shown: chips as styled buttons with
  icons, hover cards with iced `tooltip` holding the card, images inline through the image widget, videos and HTML
  as cards that open outside (`open::that`).
- **CLI**: chips become short labels (`GH o/r#12`, `@Name`, `#528`, `src/main.rs:42`) wrapped in OSC 8 hyperlinks
  when the terminal supports them. A pipe, a file or `--plain` prints the raw text, so scripts get the URLs as the
  agent wrote them. The TUI uses the same labels.

## 4. Build order

1. Inter on web and desktop, with the type roles (two agents, the IDE files excepted).
2. `apz-rich` with the vectors, and the hub unfurl routes with the cache.
3. The renderers: web, desktop, CLI and TUI.
4. The IDE layout on web and desktop, then the TUI Workspace tab.
5. Every surface switched to the rich renderer (transcripts, boards, conversations, approvals, tasks, git).
6. Checks: headless web screenshots of each surface in both themes and at phone width, desktop self-snapshots
   (`APZ_SNAPSHOT`), CLI output, then `docs/design/parity.md` updated.

## Known limits

- Desktop videos and HTML files show a card whose Open button leaves the app, by design (no webview). The web plays
  video inline and runs HTML in a sandboxed frame.
- External images load from their own hosts, with no referrer. The hub does not proxy them.
- The desktop's classifier context takes the plaza git host from the hub's overview (`git_host`), as the web does
  (`apps/desktop/src/console/rich.rs`, kept by the shell's overview load).
