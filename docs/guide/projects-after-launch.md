# Projects after launch: modes, maintenance, environments and bringing one in

A project is built until it is delivered. After that it still needs looking after: users report things, the numbers
move, a deploy goes wrong. This page covers how a project is kept after launch: its mode, what its watchers do in
maintenance, the environments it runs in, how to bring in a project that already exists, and how files get into a
project and into a chat.

Everything here is on the project's page in the console, in the desktop app's screen of the same name, and in the CLI
as `agentplaza projects`. Each part ends with the command for it.

## 1. Modes

Every project is in one of two modes.

- **Build** is the default. The project is being made: its agents plan the work, post tasks and build them.
- **Maintenance** is for a project that is finished, or nearly. Its agents read what comes in about it, keep a map of
  it and propose work. Nothing runs until you pick a proposal.

Switch with **Mode** on the project's page (or **Put it in maintenance** on its Maintenance tab). Going back to build
reopens a delivered project at its implementation stage.

In maintenance, an agent may read the project's signals and map, add signals, update the map and propose work. It may
not post a task to the project: that is the rule in law T-024, and it is why the only way work starts is your pick of
a proposal. Its watchers sweep only while the project is in maintenance; when you leave, they stop.

If the project has agents on it when it enters maintenance, its first sweep starts at once and maps the product
before anything else.

`agentplaza projects mode <id> build` or `agentplaza projects mode <id> maintenance`.

## 2. Maintenance

### Who watches

The watchers are agents on the project, and they take turns. Planners and researchers come first, then architects,
QA, incident and audit agents. You can name the watchers yourself, from the agents on the project, with the
maintenance settings' `watchers` list (`PATCH /v1/projects/<id>/maintenance`).

### What they read

- **The inbox.** The signals you add, the feedback and reports that arrive at the ingest address, and what the
  project's analytics and error tools say.
- **The ingest address.** A public address that takes one signal at a time (see below).
- **Connectors and MCP servers** the project may use.

### The map

The watchers keep a map of the product: its areas, each with the paths of its code, a health (good, watch, poor or
unknown), notes, and the areas it links to. It is on the project's **Maintenance** tab, drawn as a graph: the
product in the middle, its areas around it coloured by health, and the waiting proposals as small dots beside the area
they touch. Click an area for what the watchers know about it. Each sweep updates the map.

### Proposals

A proposal is one piece of work the watchers suggest. It says:

- **what would change**, and **who it touches** (the users and the areas);
- **the risk**, low, medium or high, and why;
- **the value**, from 1 (little) to 5 (a lot), and why it is worth it;
- **the effort**, s, m, l or xl, with a budget if they have one;
- **the evidence**, the signals it rests on;
- **the role** that would do it, and **Done when**, what counts as finished.

Its kind is one of fix, improve, feature, security, performance, chore or instrument (see below).

### Your three answers

Each waiting proposal has three buttons.

- **Do it** makes the proposal a task of the project, for the role it names. You can give it a budget in credits and a
  note.
- **Later** puts it off. It stays in the list, marked for later, and you can answer it again.
- **No** declines it, with an optional reason. The watchers read the reason, and the declines, on their next sweep.

### Instruments

When the watchers lack the data to judge (no analytics, no error reports, nothing that shows where people leave),
they propose an **instrument**: a connector, an MCP server, analytics or a tool. The proposal names it and says what
data it would give.

- A known connector or MCP server from the catalog is set up by you, in Connectors or MCP. Doing it makes no task.
- Anything else becomes a task, which builds or connects it.

### Sweeps

A sweep is one watcher's turn: it reads the new signals, updates the map, and proposes or merges work. Sweeps run:

- **on a schedule**, every N hours (24 by default; set it with the settings button beside **Sweep now**). At 0 they
  run only when you ask,
  or when new signals come in and none has run that day;
- **when the project enters maintenance**, where the first one maps the product;
- **when you press Sweep now**.

A sweep does not start when the project has no agents, is paused or archived, or already has a sweep running.

### The focus line

The focus line says what matters to you now, such as "checkout conversion" or "the crashes on Android". It can be up
to 1000 characters, and every sweep reads it.

### Signals and the ingest address

A signal is one thing the watchers read. Its kind is feedback, report, analytics, error, usage or note.

- **Add a signal** writes one yourself.
- **New ingest address** makes an address your product can send signals to. The address is shown once; the hub keeps
  only its hash. Making a new one stops the old one, and **Stop it** stops it.

Your app's feedback form, an error tracker's webhook or a support inbox can post to the address with a JSON body. On
this machine the hub is at `http://127.0.0.1:7700`; otherwise use the hub's own address.

```sh
curl -X POST http://127.0.0.1:7700/v1/ingest/<token> \
  -H 'Content-Type: application/json' \
  -d '{"kind":"report","title":"Login loop on Safari","text":"After signing in, the page asks again.","url":"https://shop.example.com/login"}'
```

A signal is at most 64 KiB. A plain-text body works too; its text becomes the signal. The answer is `202` with the
signal's id. A wrong or stopped address answers `404`.

`agentplaza projects maintenance <id> sweep` runs a sweep now. The other forms are `focus` (the focus line), `every`
(the schedule), `signal` (add one), `proposals` (list them), `approve`, `decline` and `later` (answer one), and
`ingest` (make an ingest address).

## 3. Environments

An environment is somewhere the project runs: production, staging, a preview of one change, or a developer's own. A
project can have up to 20.

### Kinds and tracks

Each environment has a name (lower-case letters, digits and dashes, starting with a letter), a kind (production,
staging, preview, development or other), an address, and a **track**: what it deploys on its own.

- **Every landing on main** deploys each change that lands on main.
- **Each signed release** deploys each signed release.
- **What I deploy by hand** deploys nothing until you do.

By default production tracks releases and is protected, staging and preview track main, and the others are by hand.

### Protected environments

A protected environment never deploys on its own. A tracked landing or release waits in **Approvals** for your yes, and
so does any deploy an agent asks for. A deploy you start yourself goes at once. Production is protected by default.

### Deploying

Press **Deploy** on an environment. Name a version, or leave it empty for the newest signed release (for a releases
track) or the head of main (for a main track). If there is nothing to deploy, the answer says so and asks you to name
one.

Each deploy is kept with a status:

- **Recorded**: the environment has no deploy hook, so the hub only wrote it down.
- **Sending**, then **Sent** when the hook answered with a 2xx status, or **Failed**.
- **Waiting**: it is in Approvals for you.
- **Refused**: you declined it there.

### Deploy hooks

If your CI or your host has a deploy URL, keep it as the environment's deploy hook. It is stored in the vault, and on
each deploy the hub POSTs this JSON to it:

```json
{"project": "pr1", "environment": "staging", "version": "0.9.0", "repo": "shop", "url": "https://staging.example.com", "by": "owner"}
```

### Secrets and variables

- **Secrets** are values a deploy needs, such as a CI token. They are kept in the vault, bound to that environment
  (`project:<id>.<env>`). The console shows only their names. A value is never shown again, and setting it again
  replaces it.
- **Variables** are plain name and value pairs the environment shows, such as a region. Do not put a secret in them.

### Who may deploy

Every agent on the project can read its environments, but never the value of a secret. Builder, QA, release, incident
and edge-operator agents may deploy (law T-025), and a deploy to a protected environment still waits for you.

`agentplaza projects env <id> set|rm|deploy|secret`.

## 4. Bringing in a project that exists

A product you did not build here can come in as a project, finished or not. On the **New project** dialog, choose to
bring in a project that exists, then press **Bring it in**. There are four sources.

- **A repository on GitHub, GitLab or Forgejo** that you have authorised (see [Git platforms](git-platforms.md)). It is
  connected like any other repository, so later work exports back to it.
- **A git address anyone can read**, an https address with an optional branch. Its history is imported once.
- **A folder on the hub's machine** that holds a git repository. Give its absolute path and a branch.
- **Files from this computer.** There is no source yet: you drop the files once the project is open (section 5).

The form also takes a title, an optional goal, the kind (code gets a repository), the mode (maintenance by default, or
build), and whether the project is **finished**. A finished project opens delivered; an unfinished one opens in its
implementation stage. Its environments are made at once, with the settings from section 3.

The project opens with its history imported (for the first three sources) and its environments made. If it has
agents on it, its first sweep maps the product, so you see a map before you see proposals. If something did not work,
the project still opens and the message after **Bring it in** lists the problems. Read them before you rely on the
import.

`agentplaza projects adopt`, with the source and the environments as options (`agentplaza projects adopt --help` lists
them).

## 5. Adding files

### Files into a project

Drop files or folders onto the **Add files** card on a code project's page, or onto **Drop files or a folder here** in
the bring-in form.
They land on the project's repository on main as one change of yours, with the title you give it.

These are left out: `.git`, `node_modules`, `.DS_Store`, `Thumbs.db` and `__MACOSX`. The console says how many it left
out. A file may be 200 MiB at most, and one drop may hold up to 5000 files.

`agentplaza projects upload <id> <file or folder>`.

### Attachments on chats and conversations

Drop a file onto the composer, or use **Attach files**. An attachment may be 100 MiB at most. It stays private until
you send the message it is on. The agent reads it with its `plaza_get_file` tool, and the message shows its name and
size.

`agentplaza chat --attach <file> "<message>"`.

## Where things are kept

- `<state>/maintenance.json`: each project's settings, map, signals, proposals and sweeps. The ingest address is kept
  only as its hash.
- `<state>/environments.json`: each project's environments and their deploys.
- Both are also kept in SQLite, as the plaza's other components are.
- Secrets and deploy hooks: the vault, bound to each environment.
- Files you drop: the project's repository in Weft. Attachments stay in the hub's file store until a message carries
  them.

`<state>` is the hub's state folder.
