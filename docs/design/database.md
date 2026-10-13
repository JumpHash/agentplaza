# AgentPlaza hub: relational database design

Status: design only. No code changed. Written against the tree as it stands: `crates/apid/sql/`
has `migrations/0001_init.sql` and the query/test layout described in `docs/dev/sql.md`.

## 0. Rules this design follows

From the owner, via main (binding):

1. No domain entity is stored as a JSON blob. Agents, tasks, board posts and replies,
   conversations and messages, projects, the library, deliverables, treasury accounts and journal,
   payments, approvals, motions and votes, laws, rules, overrides and history, sanctions,
   violations, nodes, join codes, machines, usage calls, the foundry, MCP servers, connectors, the
   toolbox, harness, permissions and looks each get real tables with real columns.
2. Settings and the providers list are tables with typed columns. No settings document.
3. The vault is the one exception to the encrypted-document model. Its ciphertext and wrapped keys
   are `bytea` columns in `vault_versions` and `vault_slots`. Non-secret metadata (`vault_entries`:
   name, kind, scope, bindings, versions, fingerprints, created, last used, use count) is queryable.
   The MAC-chained audit is `vault_audit`.
4. `jsonb` only for genuinely open-ended payloads (section 2.4). Each `jsonb` column sits in a row
   whose key fields are real columns.
5. `plaza_state` is removed in a final migration (`0023`), after the data move is verified and
   after one release of retention.

Conventions used throughout:

- Ids: the API's ids stay as they are today. Numeric ids that are `u64` in Rust are `bigint`
  (CHECK `>= 0`) and keep their values (identity columns are advanced with `setval` after the
  data move). Text ids that the API uses (`t<seq>`, `n1`, motion ids, project ids, `name@version`)
  stay `text` primary keys. Where an id is `t<seq>`, the table stores `seq bigint` and derives
  `id` as a generated column, so ordering is numeric.
- Time: every `*_ms` field in Rust becomes `timestamptz`. The HTTP shape is unchanged: queries
  return `*_ms` as `(extract(epoch from ts) * 1000)::bigint`.
- Money: `bigint` credits, never float. Rust `u64` amounts are checked into `i64` on write
  (values above `i64::MAX` are refused). Treasury amounts are already `i64` in the code.
  The one exception is USD in usage and prices, see section 2.5.
- Enums: `text` with `CHECK (col IN (...))`, not Postgres `ENUM` types. Justification (once, for
  all domains): a Postgres enum cannot drop a value, and adding a value inside a transaction is
  restricted; a CHECK is replaced in the same migration that changes the set, and the data
  migration can rewrite values before the constraint is added. The CHECK lists are the Rust
  enum variants; each is re-checked against the source when its migration is written.
- Lists of short strings that are never filtered by value (role responsibilities, tools,
  boundaries, deliverables, library requirements): `text[]` columns, not child tables. Any list
  a route filters or joins on is a child table.
- Append-only tables (section 2.6) are enforced by a trigger and by revoking UPDATE, DELETE and
  TRUNCATE from the application role.

## 1. Inventory of today's persistence

### 1.1 What is stored today

`crates/apid/src/db.rs` lists 33 snapshot components in `COMPONENTS`, in restore order (treasury
first, because task escrows live in it). Every 2 seconds the writer thread (`spawn_writer`,
`db.rs:551`) calls `snapshot_of` for each component, hashes the result (FNV-1a) and upserts the
row into `plaza_state` when the hash changed. Transcripts and infractions are the only tables.

Abbreviations: "Blob" means a `plaza_state` JSON row, plus the file fallback. "Route" means the
registered `.route` or match-arm path (the list of 239 paths was taken from `crates/apid/src`).

| # | Component | Rust types (file:line) | Size bound | Written | Route queries |
|---|---|---|---|---|---|
| 1 | agents (spawned) | `spawned::Registry{spawned: Vec<Entry{cid, role, model, name}>, models}` (`spawned.rs:18,29`) | one per agent | spawn, retire, model change | `/v1/agents`, `/v1/agents/{cid}`, `/v1/agents/{cid}/profile`, `/v1/fleet` |
| 2 | names | `BTreeMap<u64,String>` (`agents.rs`, `names`) | = agents | rename | `PATCH /v1/agents/{cid}` |
| 3 | looks | `Look{species, dealt_ms}` by cid (`looks.rs:39`) | = agents | species set | `PUT /v1/agents/{cid}/species`, `/v1/robots/species` |
| 4 | custom_roles | `RoleDef{id,title,purpose,responsibilities,deliverables,collaboration,boundaries,tools,budget,model,prompt,created_by,created_ms}` (`roles_custom.rs:22`) | tens | create, edit, delete | `/v1/roles`, `/v1/roles/{id}` |
| 5 | agent_proposals | `AgentProposal{id, role: RoleDef, new_role, count, model, reason, instruction, by_cid, by, created_ms, state, decided_ms, note, spawned: Vec<u64>}` (`agent_proposals.rs:35`) | grows; decided kept | create, decide | `/v1/agent-proposals`, `/{id}/approve`, `/{id}/reject` |
| 6 | tasks | `TaskRecord{id, goal, acceptance, budget, role, escrow_entry, created_at, state, assignee_id, verifier_id, agent_cid, note, updated_at}` (`apz-plaza/src/tasks.rs:882`); cap `MAX_TASKS = 1024` (`tasks.rs:86`) | <= 1024 in memory | every transition (state machine) | `/v1/tasks`, `/v1/tasks/{id}`, `/{id}/bid`, `/award`, `/claim`, `/finish` |
| 7 | agora | `AgoraInner{bids: HashMap<task, Vec<BidView>>, awards}`, `CallView`, `BidView`, `AwardView` (`agora.rs`); `MAX_BIDS_PER_TASK=32`, `AGORA_LIMIT=256` | 256 calls | bid, award | `/v1/agora` |
| 8 | board (social) | `PostView{id, agent_cid, role, subject, body, task, posted_by, created_at, comments: Vec<CommentView>, attachments}` (`social.rs:356`); `MAX_POSTS=100_000`, `MAX_COMMENTS=200`, `BOARD_LIMIT=256` | 100k posts, comments embedded | post, comment, read cursor | `/v1/board`, `/v1/board/read`, `/v1/board/{id}/comments` |
| 9 | conversations (social) | `ConversationView{id, from_cid, to_cid, from_role, to_role, text, created_at, delivered, delivery, attachments}` (`social.rs:478`); `MAX_CONVERSATIONS=100_000`, `MAX_PAIRS=256` | 100k messages in one Vec | send, say | `/v1/conversations`, `/v1/conversations/{a}/{b}`, `/v1/agents/{cid}/chat`, `/v1/messages` |
| 10 | contacts (social) | `ContactView{peer_cid, role, since, kind}` per cid (`social.rs:445`); `MAX_CONTACTS_PER_AGENT=256` | 256 per agent | contact add | `/v1/agents/{cid}/contacts` |
| 11 | bonds | `Contact{kind, since_ms, last_ms, count, log: Vec<(i64,f64)>, note, blocked}`, `OpinionEdge{r, s, updated_ms, rated_at, ratings: Vec<(i64,f64)>}` (`apz-plaza/src/bonds.rs:133,172`); `MAX_LOG=200` | per pair | every interaction and rating | `/v1/agents/{cid}/rating`, society reads |
| 12 | society | JSON `{bonuses, posted, outreach}` maps (`society.rs:461`) | per agent | counters | `/v1/society` |
| 13 | council | `Motion{id, proposer_cid, proposer_role, kind, text, amount, needs_owner, votes: [Option<Cast>; 4], decision: Option<Decision>, counted: Option<Quorum>, created_at, updated_at}` (`council.rs:828`); `MAX_MOTIONS=256` | 256 | propose, vote, owner decide | `/v1/motions`, `/{id}`, `/{id}/vote`, `/{id}/decide` |
| 14 | projects | `Project{id, title, goal, kind, stage, lead_cid, owner_approval, repo, docs: Vec<Doc>, critiques: Vec<Critique>, revision, approved_by, tasks: Vec<String>, history: Vec<Event>, created_ms, updated_ms}` (`projects.rs:95`) | open | stage change, critique | `/v1/projects`, `/{id}`, `/{id}/approve`, `/{id}/veto` |
| 15 | toolbox | `Discovery{id, name, kind, what_for, how, verdict, url, tags, by_cid, by, at_ms, endorsed_by: Vec<u64>, flagged}` (`toolbox.rs:39`); `MAX_DISCOVERIES=500` | 500 | discover, endorse, flag | `/v1/toolbox`, `/{id}`, `/{id}/flag` |
| 16 | harness | `Saved{loops, limits}` (`harness_control.rs:73`) | per agent | loop start/stop, limits | `/v1/loops`, `/v1/loops/{id}`, `/v1/agents/{cid}/limits` |
| 17 | approvals | `Snapshot{modes, pending: queue, seq}` (`approvals.rs:399`); `QUEUE_CAP=500`; modes also in `permissions.json` (`approvals.rs:109`) | 500 pending | every approval-gated call | `/v1/approvals`, `/v1/approvals/{id}`, `/v1/agents/{cid}/permissions` |
| 18 | library | `Entry{name, version, author_cid, author, description, command, requires, input_schema: Value, state, files: Vec<FileInfo>, hash, created_ms, calls, review_note}` (`library.rs:43`); `MAX_FILE_BYTES=4 MiB` | packages | publish, review, call counter | `/v1/library`, `/{name}/{version}`, `/files`, `/review` |
| 19 | deliverables | `Deliverable{id, name, title, description, content_type, size, author_cid, author, task, created_ms, sha256, store, kind, audience, allowed: Vec<u64>}` (`deliverables.rs:28`); bytes in the store, not in the blob | grows | create, grant | `/v1/files`, `/v1/files/{id}`, `/{id}/content` |
| 20 | connectors | `Snapshot{settings: map<id, Setting{enabled, roles}>, proposals: Vec<Plan{connector, action, values: BTreeMap<String,String>}>}` (`connectors.rs:552,926,1207`) | small | toggle, propose, review | `/v1/connectors`, `/{id}`, `/{id}/secret`, `/{id}/test`, `/v1/connectors/proposals`, `/{id}/review` |
| 21 | settings | `Settings{default_model, orchestrator_model, role_models: map, model_policy, effort, max_turns, max_tokens, fetch_block: Vec, prices: map<String,Price>, outreach_minutes, memory_autoscale, retention, sanctions, laws, payments{approve_over, charge_models}, foundry, allow_training_models, review_timeout_minutes}` (`settings.rs:73,188`) | 1 doc | owner edits | `/v1/settings` (GET, PUT) |
| 22 | providers | gateway provider list; `snapshot` clears secret fields (`providers.rs:589`) | few | edit, secret set, test | `/v1/providers`, `/{id}`, `/{id}/models`, `/{id}/test`, `/{id}/secret`; `/v1/provider-kinds` |
| 23 | laws (violations) | `Violation{law, at_ms, detail}` in `BTreeMap<String cid, Vec<Violation>>` (`laws.rs:60,1138`) | per infraction | each violation | `/v1/laws/violations`, `/v1/laws/check` |
| 24 | rules (C-tier) | `OwnerRule{id, title, source, enabled, created_ms, updated_ms, author}` (`laws.rs:126`), `Override{law, disabled_by, reason, at_ms}` (`:138`), `LawChange{law, title, tier, disabled, by, reason, at_ms, confirmed}` history (`:148,168`) | history pruned oldest first (`laws.rs:610`) | owner edits | `/v1/laws/rules`, `/{id}`, `/{id}/enable`, `/{id}/disable`, `/v1/laws/overrides/{law}` |
| 25 | sanctions | `State{applied: Vec<Applied>, suspensions: Vec<Suspension>, next_id}` (`sanctions.rs:249`); `Applied{id, cid, law, title, severity, count, tool, at_ms, sanction, outcome, fine, entry, until_ms, approval, lifted: Option<Lifted>, owner_action: Option<OwnerAction>}` (`:190`) | grows | sanction, lift | `/v1/sanctions`, `/{id}/lift` |
| 26 | control | `PlazaState` (STOP ALL) (`control.rs:44`) | 1 | stop, resume | `/v1/plaza/stop`, `/v1/plaza/resume`, `/v1/plaza/state` |
| 27 | nodes (and join codes) | `Persisted{nodes: Vec<Node>, codes: Vec<Code>}` (`nodes.rs:146`); heartbeats mutate it (`nodes.rs:664`); `MAX_CODE_MINUTES=60` | fleet | heartbeat (every node, repeatedly), join, drain, revoke | `/v1/nodes`, `/v1/nodes/codes`, `/{id}/{commands,drain,heartbeat,machine,results,revoke,runs,stats}`, `/v1/node-link`, `/v1/machines` |
| 28 | prices | `CatalogState{fetched_at_ms, openrouter: BTreeMap<model, ModelPrice>, litellm, sources}` (`prices.rs:192`) | thousands of models | daily refresh | `/v1/prices`, `/{model}`, `/refresh` |
| 29 | usage | `LedgerState{next_id, calls: VecDeque<Call>, hours: BTreeMap<HourKey,HourRow>}` (`usage.rs:149`); `Call{id, at_ms, cid, role, model, provider, tokens, cost_usd: Option<f64>, est_usd: f64, task, via, rung, escalated}` (`usage.rs:40`); retention `calls_max` (`RETENTION_MAX=100_000`) | up to calls_max rows | every model call | `/v1/usage/calls` (page 50, limit 200), `/v1/usage/series`, `/v1/usage/summary` |
| 30 | models (registry) | `ModelRecord{card, state, source_job, agent_cid, provenance: Value, calibration: Value, size_bytes, parameters, shadow: ShadowStats, created_ms, state_changed_ms, source}` (`foundry/registry.rs:104`), map by id | per model | state transitions, shadow stats | `/v1/foundry/models`, `/{id}`, `/{id}/{promote,shadow,retire,rederive,invoke}`, `/v1/foundry/models/import` |
| 31 | foundry (jobs) | `Job{id, status, recipe, datasets: Vec<String>, mode, budget_gpu_hours: f64, gpu_hours: f64, doc: Value, plan: Value, runs: Vec<Run>, series: Vec<Value>, seq, outputs, outputs_remote, metrics: Value, error, candidates}` (`foundry/store.rs:59`); `state.json` in its dir | one row per job, `series` grows per step | every training event | `/v1/foundry/jobs`, `/{id}`, `/{id}/events`, `/{id}/cancel`, `/{id}/hub/*` |
| 32 | HF imports | `Saved`, `Inner`, `Job{Step, Progress}` (`foundry/hf_import.rs:70-99`), own file, not a component | per import | progress | `/v1/foundry/hf/imports`, `/{n}`, `/v1/foundry/hf/import`, `/v1/foundry/hf/token` |
| 33 | datasets | `CallRec` (`foundry/datasets.rs:130`), own files | per dataset | upload, calls | `/v1/foundry/datasets`, `/upload`, `/v1/foundry/devices`, `/v1/foundry/recipes` |
| 34 | mcp | `McpServer{id, name, enabled, transport, env: map<String, Setting>, headers: map<String, Setting>, tools: ToolPolicy, access, limits, allow_private, storage, created_ms, updated_ms}` (`mcp/config.rs:23`); `Inner{servers, tools}` (`:412`) | few | config, start, stop | `/v1/mcp/servers`, `/{id}`, `/{id}/{start,stop,test,tools,logs}`, `/v1/mcp/catalog` |
| 35 | vault | `Document{format, vault_id, created_ms, slots: Vec<Slot>, entries: Vec<Entry{id, name, kind, label, created_ms, updated_ms, created_by, scope, bindings: Vec<String>, current, versions: Vec<Version{v, created_ms, nonce, ct, dek_nonce, dek_ct, fp, hint_*, legacy_hint}>, last_used_ms, use_count}>, tombstones, index_mac}` (`vault/model.rs`, `vault/mod.rs:1225`); audit is a file, `Record{event, actor, entry, name, binding, node, cid, detail}` with `Head{seq, mac}` (`vault/audit.rs`) | per secret; audit grows | every secret read or write | `/v1/vault/secrets`, `/{id}`, `/{id}/value`, `/{id}/test`, `/{id}/rollback`, `/v1/vault/slots`, `/audit`, `/consumers`, `/seal`, `/unseal`, `/rekey`, `/recovery`, `/import` |
| 36 | work | `scheduler::Saved{next_id, lanes: queues}` (`scheduler.rs:755,765`) | in-flight queues | every lane change, hot | none (internal) |
| 37 | transcript (table) | `transcript.rs` `KEEP=5_000` in memory; table `transcript(cid, seq, work, kind, at_ms, data)` | per record | every agent record | `/v1/agents/{cid}/transcript` |
| 38 | infraction (table) | `infraction(id, cid, law, at_ms, tool, detail)` | per infraction | per violation | `/v1/infractions`, `/v1/agents/{cid}/infractions` |

Non-component state that must move too: `hub_auth.rs` sessions (`SignIn`, `:178`), `git-users.json`
(`git.rs:56`), approvals `permissions.json` (`approvals.rs:109`), the foundry `state.json`, HF
import state, dataset files, deliverable and library bytes (they stay in the object store; only
metadata moves), and the vault doc and audit files.

Mapping note: the snapshot calls name `app.foundry` and `app.models` without a file. This table
maps `foundry` to `foundry/store.rs` (jobs) and `models` to `foundry/registry.rs` (model records).
Confirm against `App` construction before writing the foundry migration.

### 1.2 How it is queried

- Lists are sorted and paged by the API today in memory (`BTreeMap` order, `limit`, `after`).
  Each list route needs an index that matches its sort (section 2).
- The hot filters are: tasks by state and assignee; board by task and agent; conversations by
  pair; usage by agent, task, time range; foundry jobs by status; approvals by state; vault by
  name and scope; nodes by status.

### 1.3 Growth and write frequency

- Usage: one row per model call. Today the whole `calls` deque (up to `calls_max`, default large)
  is rewritten into the blob on any call. Highest-volume write.
- Transcript: one row per agent record, written one statement at a time.
- Board, conversations, bonds: one row per post, message, or interaction. Each is a whole-blob
  rewrite today.
- Work queues: rewritten on every lane change.
- Heartbeats: every node heartbeat marks the nodes blob dirty.
- Foundry jobs: `series` grows per step inside one row.
- Tasks, agora, council: a few writes per task or motion.
- Settings, providers, prices, laws, roles, connectors, MCP: rare, owner-driven or daily.

## 2. Target schema

Each domain below lists its tables with columns, primary keys (PK), foreign keys (FK, with the ON
DELETE behaviour), unique and check constraints, and the indexes that match a real route.
"Retire" means agents, nodes and tasks are never deleted; they change state. That keeps every FK
valid (see 2.1).

### 2.0 Common (0002)

- `data_migrations` (domain text PK, status text CHECK in (pending, running, verified, failed),
  source text CHECK in (blob, file, none), source_sha256 text, rows_expected bigint,
  rows_loaded bigint, detail text, started_at, finished_at timestamptz). See section 4.
- `forbid_mutation()` trigger function (section 2.6).

### 2.1 agents (0003)

- `agents` (cid bigint PK CHECK cid >= 0, name text, role text, model text, species text,
  species_at timestamptz, state text CHECK in (active, retired), created_at, retired_at,
  updated_at). One row per cid ever seen. Usage, transcript, violations and sanctions reference
  it, and rows are never deleted. Index: `(state, cid)`.
- `roles` (id text PK, title, purpose, collaboration, budget bigint CHECK >= 0, model, prompt,
  created_by, created_at, responsibilities text[], deliverables text[], boundaries text[],
  tools text[]). Built-in roles come from code; `roles` holds only custom roles. So `tasks.role` is
  plain text (no FK).
- `agent_proposals` (id text PK, new_role bool, count int CHECK 1..N, model, reason, instruction,
  by_cid FK agents, by text, created_at, state text CHECK in (pending, approved, rejected),
  decided_at, note). Index: `(state, created_at)`.
- `proposal_roles` (proposal_id text PK FK agent_proposals ON DELETE CASCADE, the RoleDef columns
  of `roles`). One role snapshot per proposal.
- `agent_proposal_spawns` (proposal_id FK CASCADE, cid FK agents, PK (proposal_id, cid)).

### 2.2 nodes and machines (0004)

- `nodes` (id text PK, name, status text CHECK in (online, offline, draining, revoked) (verify
  against nodes.rs), joined_at, last_seen_at, drained bool, ...). Heartbeat sets `last_seen_at`
  with a 10 s throttle (see 5.3); it never rewrites other columns.
- `node_join_codes` (id text PK, code_hash bytea (store a hash, not the code), created_at,
  expires_at, used_at, node_id FK nodes ON DELETE SET NULL). CHECK expires_at > created_at.
- `node_commands` (id bigserial PK, node_id FK nodes ON DELETE CASCADE, kind text, payload jsonb
  (open-ended command body, see 2.4), state text CHECK in (queued, sent, done, failed), created_at,
  result_at). Index: `(node_id, state, id)`.
- `node_machine_stats` (node_id PK FK nodes CASCADE, sampled_at, cpu, mem, disk, gpu columns).
  One row per node, replaced on each sample.

### 2.3 vault (0005)

- `vault_meta` (singleton: id smallint PK CHECK id = 1, vault_id text, format int,
  created_at timestamptz, index_mac bytea).
- `vault_slots` (id text PK, kind text, wrapped_key bytea, params jsonb (KDF and wrap parameters,
  see 2.4), created_at).
- `vault_entries` (id text PK, name text UNIQUE, kind text NOT NULL CHECK in (api_key, token,
  password, generic) (decision 4), label text, scope text CHECK in (plaza, nodes),
  created_at, created_by text, updated_at, current_version int, last_used_at timestamptz NULL,
  use_count bigint NOT NULL DEFAULT 0 CHECK >= 0).
- `vault_entry_nodes` (entry_id FK vault_entries CASCADE, node_id FK nodes ON DELETE CASCADE,
  PK (entry_id, node_id)). Only when scope = nodes.
- `vault_entry_bindings` (entry_id FK CASCADE, binding text, PK (entry_id, binding)). Index
  `(binding)` for "which secrets does this consumer read" (`/v1/vault/consumers`).
- `vault_versions` (entry_id FK CASCADE, v int CHECK v >= 1, created_at, fingerprint text (public
  fingerprint, not secret), nonce bytea, ct bytea, dek_nonce bytea, dek_ct bytea, hint_nonce bytea
  NULL, hint_ct bytea NULL, legacy_hint text NULL, PK (entry_id, v)). Ciphertext is bytea. The
  current-version row is `current_version`. Rollback sets it.
- `vault_tombstones` (entry_id text PK (deleted id, no FK), name, deleted_at, deleted_by).
- `vault_audit` (seq bigint PK, at timestamptz, event text, actor text, entry_id text (no FK: the
  audit outlives the entry), name text, binding text, node_id text, cid bigint NULL (no FK),
  detail jsonb (see 2.4), mac bytea NOT NULL). Append-only (2.6).
- `vault_audit_head` (singleton: id smallint PK CHECK id = 1, seq bigint, mac bytea). Locked
  `FOR UPDATE` on each audit write, so the chain has one writer.

The vault's key material and MAC are unchanged. Rust still verifies the chain; the database only
stores bytes. The data move (section 4) decodes today's base64 `String`s into `bytea` and checks
that re-encoding gives the original text.

### 2.4 jsonb: the complete list (each one justified)

Every other column is typed. These are the only `jsonb` columns in the design:

| Column | Row key fields (real columns) | Why it stays jsonb |
|---|---|---|
| `transcript.data` (existing) | cid, seq, work, kind, at_ms | a tool call or model message payload whose shape depends on the tool |
| `approval_requests.detail` | id, cid, tool, state, created_at | the input of the tool call awaiting approval, open per tool |
| `approval_decisions.detail` | id, decided_at, decided_by, outcome | the tool's input and output as decided, open per tool |
| `vault_audit.detail` | seq, at, event, actor, entry_id | the audit's extra detail, varies by event |
| `vault_slots.params` | id, kind, created_at | KDF and wrap parameters, differ by slot kind |
| `node_commands.payload` | id, node_id, kind, state | the command body; kinds differ per command |
| `mcp_server_tools.input_schema` | server_id, name | the tool's JSON Schema, supplied by the MCP server |
| `library_packages.input_schema` | name, version | the package's JSON Schema, supplied by the author |
| `project_events.detail` | id, project_id, at, kind, actor | the event payload, varies by kind |
| `work_items.payload` | id, lane, seq, state, enqueued_at | the queued request body; the scheduler accepts several request kinds |
| `foundry_models.serving` | id, state, created_at | a model's serving config (allowed by the owner's rule) |
| `foundry_models.provenance` | id, source_job, created_at | pipeline provenance, keys differ per recipe |
| `foundry_models.calibration` | id, state, created_at | calibration output, keys differ per recipe |
| `foundry_jobs.plan` | id, status, recipe, created_at | open-ended trainer output: the plan the trainer ran, its keys differ per recipe (decision 8) |
| `foundry_jobs.doc` | id, status, recipe, created_at | open-ended trainer output: the document the trainer returned, keys differ per recipe (decision 8) |
| `foundry_jobs.metrics` | id, status, recipe | open-ended trainer output: summary metrics keyed by recipe (decision 8). The ranking metrics are typed on `foundry_candidates`; per-step values go to `foundry_job_points` |

Nothing else is `jsonb`: settings, providers, connectors, MCP config, usage, prices, nodes,
tasks, council, board and everything else are typed.

### 2.5 treasury (0006) and money

- `treasury_accounts` (id bigint PK, kind text CHECK in (mint, treasury, town_budget,
  task_escrow, agent_earnings, external_spend), owner_ref text, credit_limit bigint NOT NULL
  DEFAULT 0, created_at timestamptz, UNIQUE (kind, owner_ref)). One task-escrow account per task
  (owner_ref = the task id).
- `treasury_entries` (id bigint PK (identity; ids preserved), kind text CHECK in (mint, spend,
  subcontract, payment, refund, clawback, fine, fee, budget_move, transfer, model), task_id text
  NULL (FK added in 0007, see below), root_task text NULL, memo text NOT NULL DEFAULT '', posted_by
  text NOT NULL, created_at timestamptz NOT NULL). Append-only.
- `treasury_postings` (entry_id bigint FK treasury_entries ON DELETE RESTRICT, line int, account_id
  bigint FK treasury_accounts, amount bigint NOT NULL CHECK (amount <> 0), PK (entry_id, line)).
  Append-only. Index `(account_id, entry_id)` for balances.
- Balance view `treasury_balances` = `SUM(amount)` per account (no stored balance).
- Balance invariant: a `CONSTRAINT TRIGGER ... DEFERRABLE INITIALLY DEFERRED` on
  `treasury_entries` and `treasury_postings` checks that each entry's postings sum to zero. Money
  cannot be created or lost in one commit.
- `payments` is not a table. A payment is a `treasury_entries` row with kind `payment`. A payment
  held for approval is an `approval_requests` row (section 2.10).

Decision 3, applied everywhere: any money in USD is `*_usd_micros bigint` (USD x 10^6). Credits
stay a separate `bigint`. No float holds money anywhere. Usage stores `cost_usd_micros` (what the
provider reported) and `est_usd_micros` (the catalogue estimate). Where a call converts to credits,
the row also stores `credits bigint` and `credit_rate_usd_micros bigint` (the USD micros per credit
used, a column for the conversion rate). Both are NULL when the call does not convert. Prices store
per-million-token rates as `*_usd_micros_per_mtok bigint`.

### 2.6 Append-only tables and how they are enforced

Append-only (decision 15, the only four): `treasury_entries` and `treasury_postings` (the treasury
journal), `vault_audit`, `law_history` (C-tier history), and `approval_decisions` (decided
approvals). Nothing else is on this list. The other insert-mostly tables (`task_transitions`,
`bond_ratings`, `project_events`, `sanction_events`, `control_events`) are plain tables written by
convention, and `infraction` is a retention-eligible log.

Enforcement, both layers:

1. Trigger. `forbid_mutation()` (in 0002) raises an exception. It is a `BEFORE UPDATE OR DELETE`
   row trigger and a `BEFORE TRUNCATE` statement trigger. It applies to every role, so a bug or a
   manual SQL session cannot rewrite history.
2. Privileges. The application role has `INSERT, SELECT` only on these tables:
   `REVOKE UPDATE, DELETE, TRUNCATE ON <table> FROM apid_app`. The migration owner keeps the
   privilege, so a migration can still fix a broken row, by disabling the trigger on purpose.

Retention-eligible (decision 15): the scheduled retention job deletes rows, with configurable windows (section 4.7). C-tier history is not on this list (decision 2).

- `usage_calls`: pruned by `calls_max` and by days (`usage.rs:294-304`).
- `transcript`: pruned per cid.
- Retention windows are in section 4.7, not here.

### 2.7 tasks, agora and the board (0007, 0008)

Full worked example in section 5.4. The schema:

- `tasks` (seq bigint PK, id text GENERATED ALWAYS AS ('t' || seq) STORED UNIQUE, goal text NOT
  NULL CHECK length 1..4000, acceptance text, budget bigint NOT NULL CHECK >= 0, role text,
  escrow_entry bigint FK treasury_entries ON DELETE RESTRICT (set once, at creation), state text
  NOT NULL CHECK in (open, claimed, awaiting_review, done, failed, withdrawn) (the `TaskState`
  variants at `tasks.rs:98`), outcome text CHECK in (done, failed, withdrawn) NULL (verify
  against `tasks.rs`), assignee_id text, agent_cid bigint FK agents ON DELETE RESTRICT, verifier_id
  text, note text, created_at timestamptz, updated_at timestamptz).
  Indexes: `(state, seq)` for the list filter, `(agent_cid)` partial where not null for "my tasks".
- `task_transitions` (id bigint identity PK, task_seq FK tasks RESTRICT, from_state, to_state,
  agent_cid NULL, at timestamptz, note text). Plain table, insert-only by convention. The current code keeps no history; this
  adds the audit that the escrow refund story needs.
- `treasury_entries.task_id` gets its FK to `tasks(id)` in 0007, after both tables exist. The
  entries are created in 0006, so the FK cannot exist earlier (circular otherwise).
- `agora_calls` (task_seq bigint PK FK tasks ON DELETE RESTRICT, capability text, spec_ref text,
  max_price bigint CHECK >= 0, state text CHECK in (open, awarded, closed) (verify
  `agora.rs`), created_at, award_agent_cid bigint NULL FK agents, award_price bigint NULL,
  award_escrow bigint NULL FK treasury_entries, awarded_at). One award per call, as columns.
- `agora_bids` (task_seq FK agora_calls CASCADE, seq int, agent_cid FK agents, bidder text,
  price bigint CHECK >= 0, created_at, PK (task_seq, seq)). Index `(task_seq, price)`. CHECK
  count per call <= 32 is enforced in code and by a trigger (`MAX_BIDS_PER_TASK`).
- `board_posts` (seq bigint PK, id text GENERATED ALWAYS AS ('p' || seq) STORED UNIQUE (today's format,
  `p<seq>`, `social.rs:732`), agent_cid FK agents ON DELETE RESTRICT, role text, subject text
  NOT NULL CHECK <= 256, body text NOT NULL CHECK <= 4096, task_id text NULL FK tasks(id) ON DELETE
  RESTRICT, posted_by text CHECK <= 64, created_at timestamptz). Indexes: `(seq DESC)` for
  "newest first", `(task_id)`, `(agent_cid, seq DESC)`.
- `board_comments` (id text PK, post_seq FK board_posts ON DELETE CASCADE, seq int, agent_cid
  FK agents NULL, author text, body text CHECK <= 1000, created_at). `MAX_COMMENTS=200` enforced
  in code and a trigger. Index `(post_seq, seq)`.
- `board_read_cursors` (agent_cid FK agents CASCADE, PK agent_cid, last_seq bigint).
- `conversation_messages` (seq bigint PK, id text GENERATED ALWAYS AS ('c' || seq) STORED UNIQUE (today's format, `c<seq>`, `social.rs:969`),
  from_cid FK agents, to_cid FK agents, from_role text, to_role text, text text CHECK <= 2048,
  created_at timestamptz, delivered bool, delivery text NULL). Index `(from_cid, to_cid, seq)` and
  `(to_cid, from_cid, seq)` for both directions of a pair. A "pair" is derived, never stored.
  CHECK `from_cid <> to_cid` (verify).
- `agent_contacts` (agent_cid FK agents CASCADE, peer_cid FK agents CASCADE, role text, since
  timestamptz, kind text CHECK in (verify `social.rs`), PK (agent_cid, peer_cid)). CHECK <= 256 per
  agent in code.
- `attachment_refs` (owner_kind text CHECK in (board_post, board_comment, message), owner_id text,
  file_id text FK deliverables(id) ON DELETE RESTRICT, PK (owner_kind, owner_id, file_id)). One
  table for every attachment, so deliverables stay the only file store.

### 2.8 bonds and society (0009)

- `bond_contacts` (a bigint FK agents, b bigint FK agents, PK (a, b) with CHECK a < b (verify
  whether pairs are ordered), kind text CHECK, since timestamptz, last_at timestamptz, count
  bigint, note text CHECK <= 2000, blocked bool). Index on `(b, a)` for the other direction.
- `bond_ratings` (id bigint identity PK, a bigint, b bigint, at timestamptz, score double
  precision, FK (a, b) bond_contacts). Plain table, insert-only by convention. `MAX_LOG=200` becomes a retention window, not
  a row cap. The score is a double on purpose: it is a reputation score, not money.
- `bond_edges` (a bigint, b bigint, PK (a, b), r double precision, s double precision, updated_at
  timestamptz, rated_at timestamptz). One row per directed edge, so the eigen computation reads a
  table. Scores are doubles, documented as such.
- `society_counters` (agent_cid bigint PK FK agents, bonus_credits bigint, bonus_count bigint,
  bonus_last bigint, posted bigint, outreach bigint). The tuple `(u64, u64, i64)` for bonuses is
  unnamed in the code; the names above are a guess to confirm in `society.rs`.

### 2.9 council (0010)

- `council_motions` (id text PK, proposer_cid FK agents, proposer_role text (seat name from
  `SEATS`, `council.rs:190`), kind text CHECK in (verify `council.rs:308`), text text CHECK <= 4000,
  amount bigint NULL CHECK >= 0, needs_owner bool, state text CHECK in (verify `council.rs:375`),
  quorum_filled int, quorum_required int, decision_approve bool NULL, decision_note text NULL
  CHECK <= 1000, decision_at timestamptz NULL, created_at, updated_at). CHECK `decision_at IS NULL
  OR decision_approve IS NOT NULL`. Index `(state, created_at DESC)`.
- `council_votes` (motion_id text FK council_motions CASCADE, seat smallint CHECK in 0..3 (4 =
  `SEAT_COUNT`), agent_cid FK agents, vote text CHECK in (verify `council.rs:351`), reason text
  CHECK <= 2000, at timestamptz, PK (motion_id, seat)). A re-vote is an UPDATE of its seat row, so
  a seat counts once.

### 2.10 projects, library, deliverables (0011, 0012)

- `projects` (id text PK, title, goal, kind, stage text CHECK in (verify), lead_cid FK agents,
  owner_approval bool, repo text NULL, revision int, approved_by text NULL, created_at, updated_at).
  Index `(stage, updated_at DESC)`.
- `project_docs` (project_id FK CASCADE, name text, body text, revision int, PK (project_id, name)
  (verify whether `Doc` has a name key)).
- `project_critiques` (id bigint identity PK, project_id FK CASCADE, by_cid FK agents, text text,
  at timestamptz).
- `project_tasks` (project_id FK CASCADE, task_seq FK tasks, PK (project_id, task_seq)).
- `project_events` (id bigint identity PK, project_id FK CASCADE, at timestamptz, kind text,
  actor text, detail jsonb (see 2.4)). Plain table, insert-only by convention.
- `library_packages` (name text, version int CHECK >= 1, PK (name, version), author_cid FK agents,
  author text, description text, command text[], requires text[], input_schema jsonb (2.4), state
  text CHECK in (verify), hash text, created_at, calls bigint NOT NULL DEFAULT 0 CHECK >= 0,
  review_note text). Index `(state)`.
- `library_package_files` (name, version, path text, bytes bigint, sha256 text, PK (name, version,
  path), FK (name, version) CASCADE).
- `deliverables` (id text PK, name text, title text, description text, content_type text, size
  bigint CHECK >= 0, author_cid FK agents, author text, task_seq bigint NULL FK tasks (id is text,
  so `task_id text` referencing `tasks(id)`), created_at, sha256 text, store text (the object store
  key), kind text, audience text). Index `(task_id)`, `(author_cid, created_at DESC)`.
- `deliverable_grants` (deliverable_id FK CASCADE, cid FK agents, PK (deliverable_id, cid)). Replaces
  `allowed: Vec<u64>`.

### 2.11 laws, rules and violations (0013)

- `owner_rules` (id text PK, title text, source text, enabled bool, author text, created_at,
  updated_at). Index `(enabled)`.
- `law_overrides` (law text PK, disabled_by text, reason text, at timestamptz). One override per
  law, so the `retain` in `laws.rs:593` becomes an upsert.
- `law_history` (id bigint identity PK, law text, title text, tier text CHECK in (verify), disabled
  bool, by text, reason text, at timestamptz, confirmed bool). Append-only (2.6). Never pruned in the database (decision 2); the API view may cap at 500.
- Violations are not a table. A violation is an `infraction` row with `source = 'law_check'` (section 2.20, decision 1).

### 2.12 sanctions (0014)

- `sanctions` (id bigint PK (ids preserved), cid FK agents, law text, title text, severity text
  CHECK in (verify `sanctions.rs:68`), count int, tool text, at timestamptz, sanction text,
  outcome text, fine bigint CHECK >= 0, entry_id bigint NULL FK treasury_entries, until_at
  timestamptz NULL, approval_id text NULL, lifted_at timestamptz NULL, lifted_by text NULL,
  lifted_reason text NULL). Index `(cid, at DESC)`, `(outcome, at DESC)`.
- `suspensions` (id bigint PK, cid FK agents, tool text, law text, until_at timestamptz, lifted
  bool). Index `(cid, lifted)`.
- `sanction_events` (id bigint identity PK, sanction_id bigint FK sanctions, kind text CHECK in
  (applied, lifted, owner_action), at timestamptz, actor text, detail text). Plain table, insert-only by convention. This is
  the history that `owner_action` and `lifted` currently hold inside the blob.

### 2.13 approvals, permissions and decisions (0015)

- `agent_permission_modes` (cid bigint PK FK agents, mode text CHECK in (verify
  `approvals.rs`), updated_at). Replaces `modes` and `permissions.json`.
- `approval_requests` (id text PK, cid FK agents, tool text, kind text CHECK in (tool, payment, ...),
  state text CHECK in (pending, approved, denied, expired), detail jsonb (2.4), created_at,
  decided_at, amount bigint NULL CHECK >= 0 (payment amount, credits)). Partial index
  `(created_at) WHERE state = 'pending'` for the queue. `QUEUE_CAP=500` enforced in code.
- `approval_decisions` (id bigint identity PK, request_id FK approval_requests, decided_at, decided_by
  text, outcome text CHECK in (approved, denied), detail jsonb (2.4), note text). Append-only.
- `default_decisions` (singleton: id smallint PK CHECK id = 1, typed columns from
  `decisions.rs:43` `DefaultState`, updated_at). A row, not a document.

### 2.14 toolbox, harness and control (0016)

- `toolbox_discoveries` (id text PK, name text CHECK <= 80, kind text, what_for text CHECK <= 400,
  how text CHECK <= 2000, verdict text CHECK <= 1000, url text NULL, by_cid FK agents, by text,
  at timestamptz, flagged text NULL, flagged_at timestamptz NULL). Index `(kind, at DESC)`. Cap
  500 in code.
- `toolbox_tags` (discovery_id FK CASCADE, tag text CHECK <= 8 per discovery in code, PK
  (discovery_id, tag)). Index `(tag)` (the tag filter).
- `toolbox_endorsements` (discovery_id FK CASCADE, cid FK agents, at timestamptz, PK
  (discovery_id, cid)).
- `harness_loops` (id text PK, cid FK agents, kind text, state text CHECK in (verify
  `harness_control.rs`), started_at, stopped_at, keep_recent int CHECK >= 0 (default 6, `:24`)).
  Index `(cid, state)`.
- `harness_limits` (cid bigint PK FK agents, limits typed columns (turns, tokens, budget, ...,
  verify `harness_control.rs`), updated_at).
- `plaza_control` (singleton: id smallint PK CHECK id = 1, stopped bool, changed_at timestamptz, by
  text, reason text).
- `control_events` (id bigint identity PK, at, kind text CHECK in (stop, resume), by text, reason text). Plain table, insert-only by convention.

### 2.15 connectors and MCP (0017)

- `connectors` (id text PK, enabled bool, updated_at timestamptz). The connector catalogue stays
  in code.
- `connector_roles` (connector_id FK CASCADE, role text, PK (connector_id, role)). Replaces
  `Setting.roles`.
- `connector_proposals` (id text PK, connector_id FK connectors ON DELETE RESTRICT, action text,
  state text CHECK in (verify), by_cid FK agents, at, reviewed_at, reviewed_by, note text).
- `connector_proposal_values` (proposal_id FK CASCADE, key text, value text, PK (proposal_id, key)).
  Replaces `Plan.values: BTreeMap`. Secret values never go here (the vault holds them).
- `mcp_servers` (id text PK, name text, enabled bool, transport text CHECK in (stdio, http) (verify
  `mcp/config.rs`), command text NULL, url text NULL, allow_private bool, access text (verify
  enum), storage text (verify), limits typed columns (verify), created_at, updated_at, tool_policy
  text (verify)). Index `(enabled)`.
- `mcp_server_env` (server_id FK mcp_servers CASCADE, name text, source text CHECK in (plain,
  vault), plain_value text NULL, vault_entry_id text NULL FK vault_entries ON DELETE RESTRICT,
  PK (server_id, name), CHECK (source = 'plain' AND vault_entry_id IS NULL) OR (source = 'vault' AND
  plain_value IS NULL)). Decision 10: none may survive. Section 4.6 sweeps them into the vault.
- `mcp_server_headers`: same shape as `mcp_server_env`.
- `mcp_server_tools` (server_id FK CASCADE, name text, description text, input_schema jsonb (2.4),
  policy text CHECK in (verify), PK (server_id, name)).
- `mcp_server_logs` (id bigint identity PK, server_id FK CASCADE, at, stream text, line text).
  Retention by row count per server, not append-only.

### 2.16 settings and providers (0018)

Settings is a singleton row, typed. There is no key/value table and no JSON document.

- `settings` (singleton: id smallint PK CHECK id = 1, default_model text, orchestrator_model text,
  effort text NULL, max_turns int CHECK >= 1, max_tokens int CHECK >= 1, outreach_minutes int,
  memory_autoscale bool, allow_training_models bool, review_timeout_minutes int, model_policy text
  (verify the `ModelPolicy` variants), fetch_block_enabled bool, retention_calls_max int CHECK <=
  100000, retention_days int CHECK <= 3650, sanctions_* typed columns, laws_* typed columns,
  payments_approve_over bigint CHECK >= 0 (credits), payments_charge_models bool, foundry_* typed
  columns, updated_at, updated_by).
- `settings_role_models` (role text PK, model text). Replaces `role_models`.
- `settings_fetch_block` (host text PK). Replaces `fetch_block: Vec<String>`.
- `price_overrides` (model text PK, input_usd_micros_per_mtok bigint, output_usd_micros_per_mtok bigint,
  updated_at). Owner overrides, moved out of `Settings.prices`, which is dropped (decision 11). The catalogue stays a separate table.
- `providers` (id text PK, kind text CHECK in (verify `provider-kinds`), base_url text, enabled
  bool, auth_kind text CHECK in (none, key, command) (verify `Auth`), auth_vault_entry_id text NULL
  FK vault_entries ON DELETE RESTRICT, created_at, updated_at). The secret is in the vault; the
  snapshot already clears it.
- `provider_headers` (provider_id FK CASCADE, name text, source text CHECK in (plain, vault),
  plain_value text NULL, vault_entry_id text NULL FK, PK (provider_id, name)). Same CHECK as
  `mcp_server_env`.
- `provider_models` is not a table: `/v1/providers/{id}/models` is a live call to the provider.

### 2.17 prices and usage (0019)

- `price_sources` (id text PK, fetched_at timestamptz, status text CHECK in (verify), detail text).
  Replaces `CatalogState.sources`.
- `price_catalog` (source_id FK price_sources CASCADE, model text, input_usd_micros_per_mtok bigint,
  output_usd_micros_per_mtok bigint, PK (source_id, model)). Index `(model)` for `/v1/prices/{model}`.
  The rate in `ModelPrice` is a float today (`per_million`, `prices.rs`); it becomes a bigint
  micro-USD rate.
- `usage_calls` (id bigint identity PK (ids preserved via setval), at timestamptz, agent_cid FK
  agents ON DELETE RESTRICT, role text, model text, provider text, tokens_in bigint, tokens_out
  bigint, tokens_* typed (verify `Tokens`), cost_usd_micros bigint NULL CHECK >= 0 (provider-reported),
  est_usd_micros bigint NOT NULL CHECK >= 0, credits bigint NULL CHECK >= 0, credit_rate_usd_micros bigint NULL CHECK > 0 (both NULL when the call does not convert), task_seq bigint NULL FK tasks, via text, rung text NULL,
  escalated bool). Indexes: `(agent_cid, at DESC)`, `(task_seq)` partial, `(at)` for series and
  retention. Not append-only (retention).
- `usage_hour_rollups` (hour timestamptz, agent_cid FK agents, model text, PK (hour, agent_cid,
  model), calls bigint, tokens_in bigint, tokens_out bigint, cost_usd_micros bigint, est_usd_micros bigint, credits bigint). The `HourKey`/`HourRow` in `usage.rs:149` are the source; the key columns are a guess
  to confirm. Updated in the same transaction as its calls (batch, section 5.3).

### 2.18 foundry (0020)

- `foundry_datasets` (id text PK, name text, created_at, owner text, bytes bigint, sha256 text,
  store text). Index `(created_at DESC)`.
- `foundry_dataset_calls` (id bigint identity PK, dataset_id FK CASCADE, at, cid FK agents NULL,
  kind text, detail text (from `CallRec`, `datasets.rs:130`)).
- `foundry_jobs` (id text PK, status text CHECK in (verify `foundry/store.rs`), recipe text, mode
  text, budget_gpu_hours numeric(10,2) CHECK >= 0, gpu_hours numeric(10,2) CHECK >= 0, created_at,
  updated_at, seq bigint, outputs_remote bool, error text NULL, plan jsonb (2.4), doc jsonb (2.4),
  metrics jsonb (2.4)). Index `(status, updated_at DESC)`, `(recipe)`. The candidate list is `foundry_candidates`, not a column.
  GPU hours are not money, so `numeric` is fine.
- `foundry_job_datasets` (job_id FK CASCADE, dataset_id FK foundry_datasets, PK (job_id, dataset_id)).
- `foundry_job_runs` (job_id FK CASCADE, seq int, status text, started_at, ended_at, node_id text
  NULL, PK (job_id, seq)). Replaces `Job.runs`.
- `foundry_job_points` (id bigint identity PK, job_id FK CASCADE, step bigint, metric text,
  value double precision, at timestamptz). Index `(job_id, metric, step)`. Replaces the per-step
  `series: Vec<Value>`, so a training run no longer rewrites one growing row.
- `foundry_job_outputs` (job_id FK CASCADE, name text, remote bool, PK (job_id, name)).
- `foundry_candidates` (job_id FK foundry_jobs CASCADE, model_id text, island text NULL, status text CHECK in (verify `breeder.rs`), val_accuracy double precision NULL, val_nll double precision NULL, val_loss double precision NULL, created_at timestamptz, updated_at timestamptz, PK (job_id, model_id)). The ranking metrics are typed (decision 8). Index `(job_id, val_nll)` and `(job_id, status)`. `model_id` has no FK, because a candidate exists before its model record.
- `foundry_models` (id text PK, state text CHECK in (verify `registry.rs`), name text, base text,
  source_job text NULL FK foundry_jobs ON DELETE SET NULL, agent_cid FK agents NULL, size_bytes
  bigint CHECK >= 0, parameters bigint CHECK >= 0, created_at, state_changed_at, serving jsonb
  (2.4), provenance jsonb (2.4), calibration jsonb (2.4), shadow_* typed columns from
  `ShadowStats`, source_kind text NULL, source_ref text NULL). Index `(state)`.
- `foundry_hf_imports` (n bigint identity PK, repo text, file text NULL, status text CHECK in
  (verify `hf_import.rs`), step text, progress double precision CHECK between 0 and 1, bytes_total
  bigint, bytes_done bigint, error text NULL, created_at, updated_at). Index `(status)`.
- The HF token is a vault entry (`vault_entries` row, kind `token`), not a column.

### 2.19 auth and work (0021)

- `hub_sessions` (token_hash bytea PK (the token is never stored), subject text, created_at,
  expires_at, revoked_at timestamptz NULL). CHECK expires_at > created_at. Index `(expires_at)`.
- `owner_tokens` (id smallint PK CHECK id = 1, token_hash bytea, rotated_at). Hash only.
- `work_items` (id bigint identity PK, lane text, seq bigint, state text CHECK in (queued, running,
  done, failed), enqueued_at timestamptz, started_at NULL, payload jsonb (2.4), UNIQUE (lane, seq)).
  Index `(lane, state, seq)`. Rows are deleted when done, so this is a queue, not history.

### 2.20 transcript and infraction (0022, upgrade of existing tables)

- `transcript`: keep `PRIMARY KEY (cid, seq)`. Add `NOT NULL` on `cid`, `seq`, `kind`, `at_ms`
  (after a check that no rows violate them). Add FK `cid` to `agents` as `NOT VALID`, then
  `VALIDATE` after the data move (agents are created first, section 4). Index `(cid, at_ms)`. No
  change to `data jsonb` (the first entry of section 2.4).
- `infraction` (decision 1, violations merged in): `id` stays `bigserial`; `at_ms` becomes `at timestamptz`; `tool` becomes nullable (law-check rows have none); add `source text NOT NULL CHECK in (tool_call, law_check)`; `law` and `detail` are `NOT NULL`. FK `cid` to `agents` (NOT VALID, then VALIDATE). Index `(cid, at DESC)`. Rows from the `violations` blob map to `source = 'law_check'`; rows from transcript infractions map to `tool_call`. Retention-eligible log (decision 15), not on the append-only list.

### 2.21 Table count per domain

| Domain (migration) | New tables | Notes |
|---|---|---|
| common (0002) | 1 | `data_migrations`; plus the `forbid_mutation` function |
| agents (0003) | 5 | agents, roles, agent_proposals, proposal_roles, agent_proposal_spawns |
| nodes (0004) | 4 | nodes, node_join_codes, node_commands, node_machine_stats |
| vault (0005) | 9 | vault_meta, vault_slots, vault_entries, vault_entry_nodes, vault_entry_bindings, vault_versions, vault_tombstones, vault_audit, vault_audit_head |
| treasury (0006) | 3 | accounts, entries, postings (+ a balances view) |
| tasks and agora (0007) | 4 | tasks, task_transitions, agora_calls, agora_bids; also adds the `treasury_entries.task_id` FK (no new table) |
| board and conversations (0008) | 6 | board_posts, board_comments, board_read_cursors, conversation_messages, agent_contacts, attachment_refs |
| bonds and society (0009) | 4 | bond_contacts, bond_ratings, bond_edges, society_counters |
| council (0010) | 2 | council_motions, council_votes |
| projects (0011) | 5 | projects, project_docs, project_critiques, project_tasks, project_events |
| library and deliverables (0012) | 4 | library_packages, library_package_files, deliverables, deliverable_grants |
| laws (0013) | 3 | owner_rules, law_overrides, law_history (violations are `infraction` rows, 0022) |
| sanctions (0014) | 3 | sanctions, suspensions, sanction_events |
| approvals (0015) | 4 | agent_permission_modes, approval_requests, approval_decisions, default_decisions |
| toolbox, harness, control (0016) | 7 | toolbox_discoveries, toolbox_tags, toolbox_endorsements, harness_loops, harness_limits, plaza_control, control_events |
| connectors and MCP (0017) | 9 | connectors, connector_roles, connector_proposals, connector_proposal_values, mcp_servers, mcp_server_env, mcp_server_headers, mcp_server_tools, mcp_server_logs |
| settings and providers (0018) | 6 | settings, settings_role_models, settings_fetch_block, price_overrides, providers, provider_headers |
| prices and usage (0019) | 4 | price_sources, price_catalog, usage_calls, usage_hour_rollups |
| foundry (0020) | 10 | foundry_datasets, foundry_dataset_calls, foundry_jobs, foundry_job_datasets, foundry_job_runs, foundry_job_points, foundry_job_outputs, foundry_candidates, foundry_models, foundry_hf_imports |
| auth and work (0021) | 3 | hub_sessions, owner_tokens, work_items |
| transcript and infraction (0022) | 0 | upgrades only |
| drop plaza_state (0023) | 0 | drops `plaza_state` |

Total: 96 new tables, including `data_migrations` (0002), so 95 domain tables. Plus the 2
existing tables (`transcript`, `infraction`), which are upgraded, not replaced. `plaza_state` is
dropped in 0023. The per-domain numbers are the design's; the implementers confirm them against
the code they read when they write each migration.

## 3. Migrations plan

File numbers are given in landing order: 0004 went to the hub-MCP tables (2026-10-13), so every planned file from nodes on moves down by one (nodes is 0005, vault 0006, and so on); the table keeps the planned names with their original numbers below nodes. Numbering is by dependency, not by the examples in the brief. Example: vault must precede
treasury, and nodes must precede vault (vault scopes reference nodes). So `0003_tasks_board.sql`
from the brief becomes `0007_tasks_agora.sql` and `0008_board_conversations.sql`.

| File | Domain | Owner (implementer) | Depends on |
|---|---|---|---|
| `0001_init.sql` | existing (plaza_state, transcript, infraction) | done | none |
| `0002_common.sql` | data_migrations, forbid_mutation() | platform | 0001 |
| `0003_agents.sql` | agents, roles, proposals | identity | 0002 |
| `0004_hub_mcp.sql` | the hub as an MCP server: tokens, call log, settings (`docs/design/hub-mcp.md`) | platform | 0002 |
| `0005_nodes.sql` | nodes, join codes, commands, machine stats | infra | 0003 |
| `0005_vault.sql` | vault (metadata, bytea ciphertext, audit) | vault | 0004 |
| `0006_treasury.sql` | accounts, entries, postings, balances | ledger | 0003 |
| `0007_tasks_agora.sql` | tasks, transitions, agora calls and bids; adds treasury_entries.task_id FK | ledger | 0006 |
| `0008_board_conversations.sql` | board, comments, cursors, messages, contacts, attachments | social | 0007 |
| `0009_bonds_society.sql` | bond contacts, ratings, edges, society counters | social | 0003 |
| `0010_council.sql` | motions, votes | governance | 0003 |
| `0011_projects.sql` | projects, docs, critiques, tasks, events | projects | 0007 |
| `0012_library_deliverables.sql` | packages, files, deliverables, grants | projects | 0007 (deliverables.task) |
| `0013_laws.sql` | owner rules, overrides, law history | governance | 0003 |
| `0014_sanctions.sql` | sanctions, suspensions, sanction events | governance | 0006, 0013 |
| `0015_approvals.sql` | permission modes, requests, decisions, default decisions | governance | 0003, 0006 |
| `0016_toolbox_harness_control.sql` | toolbox, harness, plaza control | ops | 0003 |
| `0017_connectors_mcp.sql` | connectors, MCP servers, env, headers, tools, logs | ops | 0005 |
| `0018_settings_providers.sql` | settings, role models, fetch block, price overrides, providers, headers | ops | 0005 |
| `0019_prices_usage.sql` | price catalogue, usage calls, hour rollups | ledger | 0003, 0007 |
| `0020_foundry.sql` | datasets, jobs, runs, points, candidates, models, HF imports | foundry | 0003, 0005 |
| `0021_auth_work.sql` | hub sessions, owner token hash, work items | platform | 0003 |
| `0022_transcript_infraction.sql` | upgrades to the two existing tables; merges violations into `infraction` | platform | 0003 |
| `0023_drop_plaza_state.sql` | drops `plaza_state` (shipped one release after the data move) | platform | all, and data_migrations verified |

Owner groups (for parallel work): identity (0003), infra (0004), vault (0005), ledger (0006,
0007, 0019), social (0008, 0009), governance (0010, 0013, 0014, 0015), projects (0011, 0012),
ops (0016, 0017, 0018), foundry (0020), platform (0002, 0021, 0022, 0023).

Rules:

- A migration never edits an applied file (`docs/dev/sql.md`). Each migration is one change set.
- The FK order above is required by the numbering. A later migration may add a FK to an earlier
  table, but not the reverse, except the one case in 0007 (the `treasury_entries.task_id` FK).
- 0023 checks in SQL that every domain in `data_migrations` is `verified` before it drops
  `plaza_state`. If not, it raises an exception and the migration does not apply.

## 4. Data migration

### 4.1 Mechanism

- `data_migrations` (in 0002) has one row per domain: status (pending, running, verified,
  failed), source (blob, file, none), `source_sha256`, `rows_expected`, `rows_loaded`, `detail`,
  timestamps.
- The step is Rust, not SQL. It runs once at startup, after the SQL migrations have applied
  (`Db::connect`, after `migrate::migrate`). It is `crates/apid/src/db/data_migrate.rs`, with one
  function per domain.
- It runs under the existing `APZMIG` advisory lock (`docs/dev/sql.md`), taken again for the data step.
- Domains run in dependency order: agents, nodes, vault, treasury, tasks, agora, board, messages,
  contacts, bonds, society, council, projects, library, deliverables, laws, sanctions, approvals,
  toolbox, harness, control, connectors, MCP, settings, providers, prices, usage, foundry, auth,
  work, then transcript and infraction files.

### 4.2 One domain

1. Read the source. The source is the `plaza_state` row for the component, or the component's
   file (`<component>.json`) when the row is missing and the file exists. Hash it.
2. If `status = verified` and the hash matches `source_sha256`, skip. Idempotent.
3. In one transaction: set status `running`, truncate the domain's tables (the blob is the
   source of truth until the domain is verified), parse the blob into the existing Rust types
   (`serde`, the same types the code already has, kept for the migration only), insert with
   explicit ids, then `setval` the identity sequences to `max(id)`.
4. Verify in the same transaction, before commit:
   - row counts per table equal the counts derived from the parsed blob (arrays, maps, nested
     comments and versions);
   - for treasury, the postings of each entry sum to zero, and each account's balance equals the
     balance computed from the blob (the trial balance of the old state);
   - for tasks, every `escrow_entry` exists, and each task's budget matches its escrow account;
   - for the vault, each base64 `String` decodes and re-encodes to the same text;
   - for usage, the row count equals `calls.len()`.
5. On success: set `rows_expected`, `rows_loaded`, `status = verified`, `finished_at`, commit.
   Then rename the source. Rename is done after commit, in the same order as the data (for
   example `UPDATE plaza_state SET component = component || '.migrated'` in its own statement
   after commit, and the file renamed to `<component>.json.migrated`).
6. On failure: roll back the domain's transaction (the blob and any earlier verified domains are
   unaffected), set status `failed` with the detail in a separate statement, and stop apid at
   start with `db: data migration <domain> failed: <detail>`. Fail closed: a half-migrated hub
   must not start, because it would run on two sources of truth. Decision 5: refuse to start, naming the domain and the count mismatch (`db: data migration <domain> failed: expected <N> rows, loaded <M>: <detail>`). Never fall back silently.

### 4.3 Retention and removal

- `.migrated` blobs and files stay for one release, so a rollback to the previous release still
  has its data. The previous release cannot run against the new schema (the migration runner
  refuses an older build, `docs/dev/sql.md`), so this retention is for manual recovery, not
  for running the old binary.
- `0023_drop_plaza_state.sql` ships in the following release. It checks `data_migrations` and
  drops `plaza_state`. It does not drop the `.migrated` files; those are removed by hand.

### 4.4 Files and the fallback

- Transcripts and infractions in files (no-DB runs) are imported on the first run with a
  database, through the same idempotent step: `(cid, seq)` and `id` are the keys, so a second
  run adds nothing. Decision 14: they are imported in the data move.
- Every other domain's file fallback stays as it is today, for runs without Postgres. The file
  fallback is a per-domain JSON file store with the same repository trait (section 5.1).

### 4.5 Test

Fixtures are real snapshots, checked in under `crates/apid/testdata/` (the directory already
exists). The test runs each domain's migration on a throwaway database (`scripts/test-db.sh`,
section 5.5) and asserts the verification in 4.2 passes, plus a second run that changes nothing.

### 4.6 Secret sweep and the vault kinds (decisions 4 and 10)

No plaintext secret survives the data move. The sweep runs as the first step after the vault
domain (wave 1), before providers, connectors and MCP are written, and it scans the blob and the
file copies of every domain.

1. Detect. A value is a candidate when any of these holds:
   - the name matches `(?i)authorization|api[-_]?key|token|secret|passw|cookie` (provider and MCP
     headers and env);
   - the value matches a key or token pattern (`sk-`, `Bearer `, `ghp_`, `xox[abp]-`,
     `AKIA[0-9A-Z]{16}`), or is 20 or more characters with no space in a field the server spec
     flags as secret;
   - a `{value}` placeholder sits in a field the spec flags as secret.
   A value that already references the vault is skipped.
2. Move. Each candidate becomes a `vault_entries` row with a kind from the closed list
   (`api_key`, `token`, `password`, or `generic` when none fits), one `vault_versions` row written
   through the vault's own encryption path, a `vault_entry_bindings` row (`provider:<id>` or
   `mcp:<id>`), and a `vault_audit` row with `event = 'migrate_import'`. The vault must be unsealed.
   If it is sealed when a secret is found, the migration refuses to start:
   `db: data migration secrets failed: vault is sealed; unseal it and restart`.
3. Rewrite. The row in the table references the entry (`source = 'vault'`, `vault_entry_id`). In
   the retained `.migrated` blob and file, the value is replaced by `<moved-to-vault:ENTRY_ID>`
   before the rename, so no plaintext remains in any copy.
4. Log names only: for example `secrets: moved 3 values (provider openrouter header Authorization
   -> vault entry openrouter-authorization)`. Never a value, a length or a hash.
5. Verify. `data_migrations.detail` records the count moved, and a scan of the new tables finds no
   value that matches the detection rules.

The vault kind of each existing entry maps through a fixed table (key-like to `api_key`,
token-like to `token`, password-like to `password`, anything else to `generic`). A kind that falls
through to `generic` is logged by name. It is not silent.

### 4.7 Retention (decision 15)

A scheduled job, `retention`, runs hourly as a task inside apid, not as a cron. It deletes in
batches of 5,000 rows, one transaction per batch, oldest first. Windows are typed columns in
`settings`: `retention_usage_calls_days`, `retention_usage_calls_max`, `retention_transcript_days`,
`retention_transcript_per_cid`, `retention_infraction_days`, `retention_mcp_logs_per_server`. The
defaults proposed here are 90 days for usage calls, 30 days for transcripts, 365 days for
infractions, and 1,000 lines per MCP server. Each run logs counts and names only. The usage hour
rollups are not pruned, so totals and series beyond the window still answer from the rollups.

## 5. Repository pattern

### 5.1 Layout and traits

```
crates/apid/src/store/mod.rs          Stores: the set of domain repositories, wired into App
crates/apid/src/store/tx.rs           Tx: a database transaction, or a buffered file transaction
crates/apid/src/store/error.rs        StoreError
crates/apid/src/store/tasks.rs        trait TaskRepo + PgTaskRepo + FileTaskRepo
crates/apid/src/store/treasury.rs     trait LedgerRepo + Pg + File
... one module per domain
crates/apid/sql/queries/tasks_get.sql           one statement per file (docs/dev/sql.md)
crates/apid/sql/queries/tasks_page.sql
crates/apid/sql/queries/tasks_insert.sql
crates/apid/sql/queries/tasks_transition.sql    compare-and-set on state
crates/apid/sql/queries/treasury_post_entry.sql
...
crates/apid/src/db/sql.rs             one `pub const` per query file (existing rule)
```

Each domain trait:

```rust
pub trait TaskRepo: Send + Sync {
    fn get(&self, id: &str) -> Result<Option<TaskRecord>, StoreError>;
    fn page(&self, filter: &TaskFilter, page: Page) -> Result<Vec<TaskRecord>, StoreError>;
    fn insert(&self, tx: &mut Tx<'_>, new: &NewTask) -> Result<i64, StoreError>;      // returns seq
    fn set_escrow(&self, tx: &mut Tx<'_>, seq: i64, entry: i64) -> Result<(), StoreError>;
    fn transition(&self, tx: &mut Tx<'_>, id: &str, from: TaskState, to: TaskState,
                  at: DateTime<Utc>, note: Option<&str>) -> Result<(), StoreError>;  // WHERE state = from
}
```

- `PgTaskRepo` runs the query files through `Tx`.
- `FileTaskRepo` implements the same trait over the existing JSON file. Its `Tx` buffers the
  operations and applies them in one rename, so a multi-domain write is atomic in no-DB mode
  too (within one file; across domains, the buffered files are written in the same order as the
  database transaction, and a crash between renames is recovered on start, see 6).
- `Stores` holds `Arc<dyn TaskRepo>` etc. Selected once at start, from `Db::enabled()`.

### 5.2 Transactions and in-memory caches

- `Db::transact(|tx| { ... }) -> Result<T, StoreError>` opens one transaction, runs the closure,
  commits or rolls back. Multi-table writes (a task and its escrow, section 5.4) happen inside
  one closure.
- Write path, per command: validate in memory, then `transact`, then publish to memory. Memory is
  updated only after commit, through `tx.on_commit(|| cache.apply(delta))`. If commit fails,
  memory is unchanged and the API returns the error. Today the order is the opposite: mutate in
  memory, save the blob up to 2 seconds later.
- Caches: a domain that a route reads often and that is small stays in memory (tasks,
  roles, settings, providers, prices, connectors, MCP config, toolbox, default decisions). Large
  domains (board, conversations, usage, vault audit, transcript) are read from the database,
  paged, with no full-table cache.
- Coherence: one apid process writes (decision 12). Every multi-row write still uses a transaction, and the pool (section 6.1) is the only connection source. The cache is
  loaded at start, then kept by the write path. On a database error mid-run, the domain's cache
  is reloaded from the database, not patched.
- Append-only domains have no cache.

### 5.3 Hot paths

- Usage: one `INSERT` per call, in the transaction of the operation that made it, with the hour rollup `UPDATE` in the same transaction (the per-change rule, decision 12). This replaces the whole-deque rewrite. Batching is a fallback only if a benchmark shows the per-call cost is too high.
- Transcript: one `INSERT ... ON CONFLICT DO NOTHING` per record, in the transaction of the agent step that produced it (the file backend already dedups this way).
- Heartbeats: `UPDATE nodes SET last_seen_at = $2 WHERE id = $1 AND last_seen_at < $2 - 10 s`.
  At most one write per node per 10 seconds, and no other row changes.
- Scheduler work items: insert on enqueue, update on start, delete on done. Only the changed rows
  are written. The saver never snapshots the scheduler.
- Board and conversations: insert-only on the hot path, with `seq` from an identity column.
- Writer thread: the 2-second snapshot loop (`db.rs:551`) is retired per domain as each domain moves (decision 12, section 10). A migrated domain writes per change in its own transaction and leaves `COMPONENTS`. The loop keeps running only for unmigrated components, and goes with `plaza_state` in the final wave.

### 5.4 Worked example: create a task with its escrow and a board post

Behaviour today: a task is created in memory and its budget is minted into escrow by the treasury
(`TaskRecord.escrow_entry` points at the mint entry). The task blob and the treasury blob are
saved in two separate upserts. The new code does all three writes in one transaction: the task,
its mint journal entry with two postings, and the board post that announces it.

SQL, the statements the repo runs inside the transaction (a sketch, the queries become files):

```sql
-- tasks_insert.sql   $1 goal  $2 acceptance  $3 budget  $4 role  $5 created_at
INSERT INTO tasks (goal, acceptance, budget, role, state, created_at, updated_at)
VALUES ($1, $2, $3, $4, 'open', $5, $5)
RETURNING seq, id;

-- treasury_account_ensure.sql   $1 kind 'task_escrow'  $2 owner_ref (the task id)
INSERT INTO treasury_accounts (kind, owner_ref, created_at)
VALUES ($1, $2, now())
ON CONFLICT (kind, owner_ref) DO NOTHING;

-- treasury_entry_insert.sql   $1 kind 'mint'  $2 task_id  $3 memo  $4 posted_by  $5 created_at
INSERT INTO treasury_entries (kind, task_id, memo, posted_by, created_at)
VALUES ($1, $2, $3, $4, $5)
RETURNING id;

-- treasury_posting_insert.sql   $1 entry_id  $2 line  $3 account_id  $4 amount
INSERT INTO treasury_postings (entry_id, line, account_id, amount)
VALUES ($1, $2, $3, $4);       -- run twice: budget out of the treasury, into escrow

-- tasks_set_escrow.sql   $1 seq  $2 entry_id
UPDATE tasks SET escrow_entry = $2 WHERE seq = $1 AND escrow_entry IS NULL;

-- board_post_insert.sql   $1 agent_cid  $2 role  $3 subject  $4 body  $5 task_id  $6 posted_by  $7 created_at
INSERT INTO board_posts (agent_cid, role, subject, body, task_id, posted_by, created_at)
VALUES ($1, $2, $3, $4, $5, $6, $7)
RETURNING seq, id;
```

Constraints that make this safe:

- The `balanced` deferred constraint trigger on `treasury_entries` and `treasury_postings` fails
  the commit if the two postings do not net to zero.
- `treasury_postings.amount <> 0`, and `budget >= 0` on tasks. A failed insert rolls back the
  whole closure: no task without its escrow, no escrow without its task, no post without the task.
- `tasks.escrow_entry` is set once (`AND escrow_entry IS NULL`), so a replay cannot move it.
- `board_posts.task_id` FK: the post cannot name a task that does not exist.

Rust (sketch):

```rust
// crates/apid/src/store/plaza/task_create.rs
pub fn create_task(stores: &Stores, input: CreateTask, now: DateTime<Utc>) -> Result<TaskRecord, StoreError> {
    stores.db.transact(|tx| {
        let seq = stores.tasks.insert(tx, &NewTask::from(&input, now))?;
        let id = format!("t{seq}");

        if input.budget > 0 {
            stores.ledger.ensure_account(tx, AccountKind::TaskEscrow, &id)?;
            let entry = stores.ledger.post(tx, &PostEntry {
                kind: EntryKind::Mint,
                task_id: Some(&id),
                memo: &input.memo,
                posted_by: "plaza",
                created_at: now,
                postings: &[
                    Posting { account: TREASURY, amount: -input.budget },
                    Posting { account: escrow_of(&id), amount: input.budget },
                ],
            })?;   // the balance is checked by the deferred trigger at commit
            stores.tasks.set_escrow(tx, seq, entry)?;
        }

        let post = stores.board.insert_post(tx, &NewPost::announcement(&input, &id, now))?;
        Ok(TaskRecord::created(seq, post))  // memory is updated only after commit
    })
}
```

Memory publication: `Db::transact` runs the closure's `tx.on_commit` callbacks after the commit
succeeds, and the task cache, the treasury cache and the event bus (`EventBus`) get the new task
then. A rollback publishes nothing.

### 5.5 Tests

- `scripts/test-db.sh` (existing) starts a throwaway `postgres:17` container and sets
  `APZ_TEST_DATABASE_URL`. Each test creates its own database, `apz_test_<label>_<pid>`.
- Every repo trait gets one contract suite, run against both `FileTaskRepo` and `PgTaskRepo`
  (the pattern of `db::tests::every_query_behaves_the_same_on_both_backends`). The suite
  covers ordering, paging, filters, compare-and-set transitions, and the escrow closure above.
- Constraint tests: a deliberately unbalanced entry must fail to commit; a rolled-back closure
  must leave no task, entry, or post. An append-only test: `UPDATE` and `DELETE` on
  `treasury_postings` fail for the application role and for the owner (trigger).
- Migration tests: section 4.5.
- A test that a Rust enum variant list and the SQL CHECK list are equal, per enum, so a new
  variant cannot be added without a migration.

## 6. Risks, and what is fragile today

### 6.1 Risks of the new design

- Hot path cost: usage and transcript writes multiply with load. Mitigated by batching (5.3),
  not by per-row round trips.
- Connection: resolved by decision 6. A pool of 4 to 8 connections (`r2d2` with `r2d2_postgres`, sync, matching the `postgres` crate the code already uses) is in Wave 0, before any hot domain moves.
- Transition period: some domains are blobs and some are tables. The two must not both be
  authoritative. Rule: a domain is either a blob (old code) or a table (new code), switched by the
  data migration. The writer loop covers only blob domains.
- Foreign keys to `agents`: agents are never deleted, and the data migration creates a placeholder
  `agents` row for any cid that appears in usage, transcript, or violations but not in the agents
  blob. The placeholder count is recorded in `data_migrations.detail`.
- Timestamps: `u64` ms to `timestamptz` is lossless for ms. Values past 2^63 ms are rejected.
- Ordering: lists that the API sorts in memory (`BTreeMap` order) must sort by the same key in
  SQL. A mismatch changes the API order, so each list route has a test for its order.
- Index bloat: `tasks`, `foundry_jobs`, and `approval_requests` are update-heavy. Use
  `fillfactor = 80` on them and let autovacuum run.
- Vault: the audit chain has one writer (`vault_audit_head ... FOR UPDATE`). Vault writes are
  rare, so this does not contend in practice.

### 6.2 What is fragile today (from the code)

- Every 2 seconds the writer serialises all 33 components and hashes each one (`db.rs:551-570`),
  even when nothing changed. Any change rewrites a whole component. The board blob can hold 100k
  posts with 200 comments each; one new comment rewrites all of them.
- Cross-component consistency is not transactional. A task and its treasury escrow are two
  separate upserts that can land on different ticks. A crash between them leaves a task pointing
  at an entry that is not stored, or a minted budget with no task. The restore order (treasury
  first) does not fix a crash between writes.
- `Db::store` skips an empty snapshot (`db.rs:293-297`). A component whose snapshot is `""` can
  never clear its row. The scheduler returns `""` in its fresh state (`scheduler.rs:765`), so its
  queue can only reach the database through a non-empty snapshot.
- Connect failure at start ran the hub on files (fixed by decision 13). With `APZ_DATABASE_URL` set, the database is the only source of truth: apid retries for 30 s, then exits with an error. The file backend runs only when no URL is configured.
- Money as floats: `usage::Call.cost_usd: Option<f64>` and `est_usd: f64` (`usage.rs:40-54`).
  Prices are `f64` per-million rates (`prices.rs`). Totals sum floats.
- Vault: the document is saved twice, as the `vault` blob and as the vault's own file
  (`vault/mod.rs:253,291`). Which copy wins at start is not obvious from the code. Ciphertext is
  base64 `String`s, not bytes, inside JSON (`vault/model.rs` `Version`).
- MCP: `McpServer.env` and `headers` are maps of `Setting`. Whether a `Setting` can hold a secret
  in plain text is not clear from the code (decision 10). The blob is written to Postgres in
  full.
- Foundry: `Job.series` grows as a `Vec<Value>` in one row (`foundry/store.rs:59`). Every training
  step rewrites the whole series.
- Transcript: `transcript.rs` keeps 5,000 records per cid in memory (`KEEP`), but the restore
  loads up to 500 per cid (`docs/dev/sql.md`, "Restart"). A restart therefore loses records between 500 and
  5,000 from the in-memory view.
- Duplicated state: approvals modes live in the `approvals` blob and in `permissions.json`
  (`approvals.rs:109`). Violations and infractions recorded the same events twice (`laws.rs:60`, `infraction`). Decision 1 merges them.
- Heartbeats mutate the nodes blob (`nodes.rs:664`), so the fleet's heartbeat rate sets the
  rewrite rate of the nodes blob.
- Laws history was pruned oldest-first (`laws.rs:610`). Fixed by decision 2: the database never prunes C-tier history.
- Usage: the calls deque is in one blob, so its size, up to `calls_max`, sets the cost of every
  write.
- The writer cadence is 2 seconds (`db.rs:558`). The brief says "the 3 s saver". Confirm which
  is intended.

## 7. Open questions for main (all answered in section 9)

1. Violations vs infractions. Propose merging `violations` into `infraction`, which needs
   `law_id` and `severity`. Decide before 0013 and 0022.
2. C-tier history: the code prunes `law_history` (`laws.rs:610`). Is the history meant to be
   append-only (proposed here), or capped? If capped, the cap moves to a privileged archive
   function and the table is not append-only.
3. Usage cost unit: this design uses `cost_usd_micros bigint`. Does USD spend convert to credits
   for the treasury, and at what rate? That decides whether usage posts journal entries.
4. Vault `kind`: free text (proposed) or a closed CHECK list? The catalogue in code is the
   source of truth.
5. Failed data migration: refuse to start (proposed, fail closed), or run the old blob path for
   the failed domain for one run?
6. Connection: keep one `Client` behind a `Mutex`, or move to a pool (r2d2 or deadpool) before
   the first hot domain moves?
7. Migration numbering differs from the brief's examples (`0002_common.sql` precedes
   `0003_agents.sql`, and tasks are `0007`). OK?
8. Foundry `plan`, `doc` and `metrics` are kept as `jsonb` for now. Should they be typed
   columns, with the recipes' fields listed, once the recipes are stable?
9. Built-in roles: code only (proposed) or seeded rows? Affects whether `tasks.role` can have an FK.
10. MCP and provider headers: can a value be a plain secret today? If yes, the migration must move
    it to the vault before the data move, not after.
11. `settings.prices` duplicates `price_catalog` as overrides. Keep both, or one?
12. A second apid instance: if the hub ever runs two processes, the caches need invalidation
    (`LISTEN/NOTIFY`). Out of scope unless the owner plans it.
13. Connect-failure reconciliation (6.2): should a restart after an outage refuse to start, or
    prefer the newer source by timestamp?
14. Should file-fallback transcripts and infractions import into the database the first time it
    is reachable (section 4.4)?
15. Retention: `usage_calls` and `transcript` are pruned by DELETE. Is the retention job allowed
    to delete, given the append-only rule elsewhere?
16. The id formats for board posts, conversation messages and motions: confirm in `social.rs`
    and `council.rs` before the generated-column definitions are written.

## 8. What I read

`crates/apid/src/db.rs` (components, snapshot and restore dispatch, writer, store/load and
transcript appends), `docs/dev/sql.md`, `crates/apid/sql/migrations/0001_init.sql`; the snapshot
bodies of 30 apid stores (`toolbox`, `usage`, `projects`, `looks`, `prices`, `library`,
`settings`, `connectors`, `approvals`, `roles_custom`, `providers`, `society`, `agents`,
`scheduler`, `mcp/config`, `control`, `laws`, `foundry/registry`, `harness_control`,
`agent_proposals`, `deliverables`, `spawned`, `nodes`, `sanctions`, `foundry/store`, `vault`);
struct fields for `TaskRecord`, `Account`, `Entry`, `Posting`, `Motion`, `Cast`, `Contact`,
`OpinionEdge`, `PostView`, `CommentView`, `ConversationView`, `ContactView`, `BidView`, `AwardView`,
`CallView`, `Call`, `Settings`, `Project`, `Deliverable`, `Entry` (library), `Look`, `RoleDef`,
`AgentProposal`, `Discovery`, `Violation`, `OwnerRule`, `Override`, `LawChange`, `Applied`,
`Suspension`, `Job`, `ModelRecord`, `McpServer`, `Persisted`, `Entry`/`Version`/`Document`
(vault), `Record`/`Head` (audit), and the enum variants of `TaskState` and `EntryKind`/`AccountKind`;
every registered path literal under `crates/apid/src` (239 paths).

## 9. Decisions

Main's decisions on the 16 questions of section 7. Each is applied in the section named.

| # | Question | Decision | Applied in |
|---|---|---|---|
| 1 | Violations and infractions | Merge `violations` into `infraction`, with a `source` column (`tool_call`, `law_check`). | 2.11, 2.20 |
| 2 | C-tier law history | Append-only in the database, never pruned there. The in-memory or API view may cap at 500. | 2.6, 2.11 |
| 3 | Money | `*_usd_micros bigint` for every USD amount (usage, prices). Credits stay a separate `bigint`. Usage rows that convert also store `credits` and `credit_rate_usd_micros`. No floats for money. | 2.5, 2.17 |
| 4 | Vault kind | Closed list `api_key`, `token`, `password`, `generic`, with a CHECK. | 2.3, 4.6 |
| 5 | Failed data migration | Refuse to start, with an error that names the domain and the count mismatch. Never fall back silently. | 4.2 |
| 6 | Connections | A pool of 4 to 8 connections: `r2d2` with `r2d2_postgres`, which is sync, like the `postgres` crate. It lands in Wave 0, before any hot domain moves. | 5, 6.1, 10 |
| 7 | Renumbered migrations | Accepted. | 3 |
| 8 | Foundry | Typed ranking metrics (`val_accuracy`, `val_nll`, `val_loss`, `island`, `status`, timestamps) on `foundry_candidates`. The doc, plan and metrics bags on `foundry_jobs` stay `jsonb`, as open-ended trainer output. | 2.4, 2.18 |
| 9 | Roles | Built-in roles live in code. `tasks.role` is text with no FK. Custom roles have their own table, `roles`. | 2.1, 2.7 |
| 10 | Plaintext secrets | None may survive. The data move inspects provider and MCP headers and env values, and moves each secret into the vault with the right binding. The config is rewritten to reference the vault. Names are logged, values never. | 2.15, 4.6 |
| 11 | Prices | `settings.prices` is dropped. Owner overrides live in `price_overrides`. | 2.16 |
| 12 | Processes | One apid process. Transactions are still used for every multi-row write. The migration lock (`APZMIG`) already exists and is reused. | 4.1, 5.2 |
| 13 | Database unreachable at start | When `APZ_DATABASE_URL` is set, the database is the only source of truth. If it is unreachable, retry with backoff for 30 s, then exit with an error. Never run on files when a database is configured. The file backend is used only when no URL is configured. | 6.2, 10 |
| 14 | File-fallback transcripts and infractions | Imported during the data move. | 4.4 |
| 15 | Retention | Deletes in `usage_calls` and `transcript` are allowed; they are logs. Append-only applies to the treasury journal, vault audit, C-tier history and decided approvals. Retention is a documented scheduled job with configurable windows. | 2.6, 4.7 |
| 16 | Ids | Today's API id formats stay exactly, so URLs are stable. The formats are listed below. | 2.7, 2.8 |

Id formats, as the production code creates them today (checked against the source, and not
against test-only lines, so the URLs do not change):

| Entity | Format | Source |
|---|---|---|
| agent | `cid` (number) | agents |
| task | `t<seq>` | `tasks.rs:805` |
| board post | `p<seq>` | `social.rs:732` |
| board comment | `c<seq>` | `social.rs:786` |
| conversation message | `c<seq>` (separate table from comments) | `social.rs:969` |
| motion | `m<seq>` | `council.rs:1098` |
| node | `n<seq>` | `nodes.rs:278` |
| node join code | `c<n>` | `nodes.rs:200` |
| node command | `k<n>` (confirm in `nodes.rs:350`) | `nodes.rs:350` |
| harness loop | `l<n>` | `harness_control.rs:208` |
| project | `pr<seq>` | `projects.rs:245` |
| approval | `a<n>` | `approvals.rs:237` |
| toolbox discovery | `d<n>` | `toolbox.rs:200` |
| agent proposal | `ap<n>` | `agent_proposals.rs:81` |
| connector proposal | `cp<n>` in a test only (`connectors.rs:1739`); the production format is to confirm | `connectors.rs` |
| deliverable | `f<hex>` | `deliverables.rs:482` |
| foundry plan item | `i<n>` | `foundry/plan.rs:587` |

The generated-column expressions in the DDL above (`'t' || seq`, `'p' || seq`, `'c' || seq`) match
this table. Other tables keep a `text` primary key that the Rust side fills from the table's own
sequence, with the prefix above.

Two consequences of the decisions:

- Decision 13 changes the startup path: `Db::connect` no longer falls back to files when a URL is
  set. Section 6.2 records this as fixed.
- Decision 12 means the transaction boundary is per operation. Section 10 retires the saver per
  domain.

## 10. Waves

Ordering and file ownership, so parallel implementers do not collide.

Rule that drives the shape: the migration runner refuses a gap, and the CI check requires new
numbers to be the base maximum plus one. Waves therefore cannot ship migrations one at a time. All
schema files `0002` to `0022` are written and registered in Wave 0. Each later wave adds code,
queries and its data-move step only. `0023_drop_plaza_state.sql` is written in Wave 0 but
registered only in the final wave, one release later. Until a file is merged it can change; once
merged it is immutable (`docs/dev/sql.md`). A schema bug found in a later wave gets the next free
number, and the drop moves to the number after it.

### Wave 0: infrastructure

Must land first. No other wave starts coding until it is merged.

| File | Change |
|---|---|
| `crates/apid/Cargo.toml` | add `r2d2`, `r2d2_postgres` |
| `crates/apid/src/db.rs` | pool; the DB-required startup rule (decision 13: retry 30 s, then exit); no file fallback when a URL is set; `Db::transact(\|tx\| ...)` with `on_commit`; the saver keeps `COMPONENTS` for unmigrated components |
| `crates/apid/src/db/migrate.rs` | the `MIGRATIONS` list, with every entry `0002` to `0022`. This is the only edit point for the list, so later waves never touch it. `0023` is added in the final wave |
| `crates/apid/sql/migrations/0002_common.sql` | `data_migrations`, `forbid_mutation()` |
| `crates/apid/sql/migrations/0003_agents.sql` to `0022_transcript_infraction.sql` | schema for every domain, per section 3. Each owner reviews their domain's file |
| `crates/apid/sql/migrations/migrations.lock` | updated once, by the integrator, with `scripts/migrations.sh lock` |
| `crates/apid/src/db/data_migrate/mod.rs` | the data-migration framework: `data_migrations` rows, the advisory lock, the per-domain runner, the domain registry (empty in Wave 0) |
| `crates/apid/src/store/mod.rs`, `store/tx.rs`, `store/error.rs` | `Stores`, `Tx`, `StoreError`, the repository traits' shared pieces |
| `crates/apid/src/db/sql.rs` | consts for the common and migration queries |
| `crates/apid/src/state.rs` | wire `Stores` into `App` |
| `crates/apid/tests/store_contract.rs` (new) | the contract-suite helper, run against file and Postgres backends |
| `docs/dev/sql.md` | the pool, the startup rule, the data-migration framework |

Exit: `scripts/test-db.sh` passes, the migrations apply on an empty database and on a
`0001`-only database, and apid refuses to start with an unreachable URL after 30 seconds.

### Wave 1: agents, nodes, settings and providers, vault

Runs after Wave 0. Vault goes first in the wave, because the secret sweep (4.6) and providers need
it.

| File | Owner |
|---|---|
| `crates/apid/src/store/vault.rs`, `store/agents.rs`, `store/nodes.rs`, `store/settings.rs`, `store/providers.rs` (new) | each domain |
| `crates/apid/sql/queries/vault_*.sql`, `agents_*.sql`, `nodes_*.sql`, `settings_*.sql`, `providers_*.sql` (new) | each domain |
| `crates/apid/src/vault/mod.rs`, `vault/audit.rs`, `vault/model.rs` | vault: keep the encryption and the MAC chain; replace the blob and the file persistence with `store::vault`. `vault/disk.rs` retires for the migrated domain |
| `crates/apid/src/agents.rs`, `spawned.rs`, `looks.rs`, `roles_custom.rs`, `agent_proposals.rs` | agents |
| `crates/apid/src/nodes.rs`, `node_link.rs` | nodes (heartbeat goes to the 10 s throttled `UPDATE`) |
| `crates/apid/src/settings.rs`, `providers.rs` | settings and providers. `Settings.prices` moves to `price_overrides` |
| `crates/apid/src/db/data_migrate/{vault,agents,nodes,settings,providers,secrets}.rs` (new) | one data step per domain; `secrets.rs` is 4.6 |
| `crates/apid/src/db.rs` | remove these components from `COMPONENTS` |

Exit: the four domains round-trip their fixtures, the sweep moves every fixture secret and leaves
none behind, and a restart with a sealed vault refuses to start (4.6).

### Wave 2: treasury, tasks and agora, board and conversations

Runs after Wave 1 (tasks reference agents, and the escrow references the treasury).

| File | Owner |
|---|---|
| `crates/apid/src/store/treasury.rs`, `store/tasks.rs`, `store/agora.rs`, `store/board.rs` (new, includes contacts and attachments) | ledger and social |
| `crates/apid/sql/queries/treasury_*.sql`, `tasks_*.sql`, `agora_*.sql`, `board_*.sql`, `conversation_*.sql`, `contacts_*.sql` (new) | ledger and social |
| `crates/apz-plaza/src/treasury.rs`, `tasks.rs`, `agora.rs`, `social.rs` | only the snapshot and restore seams and the store calls. No change to the rules |
| `crates/apid/src/plaza.rs` | the bid, award, post and message routes call the store inside their own transaction |
| `crates/apid/src/db/data_migrate/{treasury,tasks,agora,board}.rs` (new) | one data step per domain; the trial balance check (4.2) runs on treasury |
| `crates/apid/src/db.rs` | remove `treasury`, `tasks`, `agora`, `social` from `COMPONENTS` |

Exit: the task-with-escrow transaction of 5.4 passes its contract tests, including the rollback
cases, and the trial balance matches the blob on the fixtures.

### Wave 3: everything else

Sub-owners work in parallel. Each owns a store module, a data step and its queries, and adds one
line to the domain registry in `data_migrate/mod.rs` (a trivial merge).

| Sub-owner | Store modules | Code they touch | Data step |
|---|---|---|---|
| governance | `store/council.rs`, `laws.rs`, `sanctions.rs`, `approvals.rs` | `apz-plaza/src/council.rs`, `apid/src/laws.rs`, `sanctions.rs`, `approvals.rs`, `decisions.rs` | council, laws (incl. violations to infraction), sanctions, approvals |
| ops | `store/toolbox.rs`, `harness.rs`, `control.rs`, `connectors.rs`, `mcp.rs` | `toolbox.rs`, `harness_control.rs`, `control.rs`, `connectors.rs`, `mcp/*.rs` | toolbox, harness, control, connectors, MCP (the sweep rules of 4.6 apply first) |
| projects | `store/projects.rs`, `library.rs`, `deliverables.rs` | `projects.rs`, `library.rs`, `deliverables.rs` | projects, library, deliverables |
| ledger | `store/usage.rs`, `prices.rs` | `usage.rs` (batch is not used; one insert per call), `prices.rs`, the call site in `llm_proxy.rs` | prices, usage (hot; see 5.3) |
| social and bonds | `store/bonds.rs`, `society.rs` | `apz-plaza/src/bonds.rs`, `apid/src/society.rs` | bonds, society |
| foundry | `store/foundry.rs` | `foundry/store.rs`, `registry.rs`, `hf_import.rs`, `datasets.rs`, `breeder.rs` (candidates) | datasets, jobs, candidates, models, HF imports |
| platform | `store/auth.rs`, `store/work.rs`, `store/transcript.rs` | `hub_auth.rs`, `scheduler.rs` (work items), `db/files.rs` (import only) | sessions, work items, transcript and infraction import (decision 14), retention job (4.7) |

Exit per domain: its contract suite passes on both backends (where a file backend exists), its
data step verifies the counts, and its component is removed from `COMPONENTS`. The saver is
removed when `COMPONENTS` is empty.

### Final: drop plaza_state (one release later)

| File | Change |
|---|---|
| `crates/apid/sql/migrations/0023_drop_plaza_state.sql` | checks that every domain in `data_migrations` is `verified`, then drops `plaza_state`. Raises an exception otherwise |
| `crates/apid/src/db/migrate.rs` | register `0023` in this release, and no sooner |
| `crates/apid/src/db.rs` | delete the saver loop, `snapshot_of`, `restore_into`, `COMPONENTS`, `Db::store` and `Db::load` |
| `docs/dev/sql.md` | remove the `plaza_state` description |

The `.migrated` rows and files live for the one release between the data move and this step. They
are removed by hand after 0023.

### Shared files and how the waves avoid collisions

- `db/migrate.rs` (the list): Wave 0 only. After that it is not edited, except for 0023.
- `migrations.lock`: the integrator only, through `scripts/migrations.sh lock`.
- `db.rs` (`COMPONENTS`, the saver): each wave removes its own names. Wave 0 keeps the list
  intact; no wave adds a name.
- `data_migrate/mod.rs` (the registry): one line per domain, added by the domain's owner.
- `apz-plaza/src/*`: only the snapshot and restore seams change, and only in Waves 2 and 3.
