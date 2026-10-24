-- Orchestrator core data model, draft (0121 pattern: runs on PostgreSQL 16, tested in tests/).
-- Decisions: 0019 machines, 0044-0047 identity, 0084 transcript, 0091 snapshots, 0093 events,
-- 0094 approvals, 0102 fencing, 0116 idempotency and nonces, 0123 this model and the task lifecycle.
-- Rule of thumb: an invariant the jhrl machines check (crates/jhrl/specs/machines/) is also enforced here, so a bug in
-- orchestrator code fails a transaction instead of corrupting state.

CREATE SCHEMA core;

-- ================================================================= owner, devices, machines
CREATE TABLE core.devices (                         -- owner devices; all equal (0095)
  id           text PRIMARY KEY,
  name         text NOT NULL,
  kind         text NOT NULL CHECK (kind IN ('cli', 'desktop', 'web', 'telegram')),
  public_key   bytea NOT NULL,                     -- device key (0047); passkeys live with the relay account
  enrolled_at  timestamptz NOT NULL DEFAULT now(),
  last_seen_at timestamptz,
  revoked_at   timestamptz
);

CREATE TABLE core.enrollment_codes (                -- one-time codes for devices and machines (0047)
  code_hash   bytea PRIMARY KEY,                    -- never store the code itself
  purpose     text NOT NULL CHECK (purpose IN ('device', 'machine')),
  expires_at  timestamptz NOT NULL,
  used_at     timestamptz,
  CHECK (used_at IS NULL OR used_at <= expires_at)
);

CREATE TABLE core.machines (                        -- 0019, 0101-0104
  id               text PRIMARY KEY,
  name             text NOT NULL UNIQUE,
  arch             text NOT NULL CHECK (arch IN ('x86_64', 'aarch64')),
  backend          text NOT NULL CHECK (backend IN ('kvm', 'hvf')),
  irqchip          text,                            -- e.g. vgicv2 on Pi 4/5
  page_size        integer NOT NULL,
  cpus             integer NOT NULL CHECK (cpus > 0),
  memory_mib       integer NOT NULL CHECK (memory_mib > 0),
  disk_gib         integer NOT NULL,
  labels           text[] NOT NULL DEFAULT '{}',    -- attached hardware: gpu, camera, ...
  host_reserve_mib integer NOT NULL DEFAULT 1024,
  class            text NOT NULL CHECK (class IN ('server', 'apple', 'pi5', 'pi4')),
  overcommit       numeric(4,2) NOT NULL CHECK (overcommit >= 1),   -- effective vCPUs per core; lowered under contention (0133)
  overcommit_cap   numeric(4,2) CHECK (overcommit_cap >= 1),        -- owner's cap; null = none
  cert_fingerprint bytea NOT NULL UNIQUE,
  state            text NOT NULL DEFAULT 'online' CHECK (state IN ('online', 'unreachable', 'draining', 'removed')),
  last_heartbeat_at timestamptz,
  CONSTRAINT hvf_on_aarch64 CHECK (backend <> 'hvf' OR arch = 'aarch64'),    -- HVF only on Apple Silicon (0007)
  CONSTRAINT overcommit_within_cap CHECK (overcommit_cap IS NULL OR overcommit <= overcommit_cap)
);

-- Starting overcommit per machine class (0133); the scheduler lowers it in steps of 0.5 under contention.
CREATE FUNCTION core.default_overcommit(class text) RETURNS numeric LANGUAGE sql IMMUTABLE AS $$
  SELECT CASE class WHEN 'server' THEN 4.0 WHEN 'apple' THEN 4.0 WHEN 'pi5' THEN 2.0 ELSE 1.5 END
$$;
CREATE FUNCTION core.set_overcommit() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NEW.overcommit IS NULL THEN NEW.overcommit := least(core.default_overcommit(NEW.class), coalesce(NEW.overcommit_cap, 99)); END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER machines_overcommit BEFORE INSERT ON core.machines FOR EACH ROW EXECUTE FUNCTION core.set_overcommit();

-- ================================================================= agents
CREATE TABLE core.agents (                          -- the registry card (identity.md (not in the repo)); keys live in the vault
  id           text PRIMARY KEY CHECK (id ~ '^[0-9A-HJKMNP-TV-Z]{26}$'),   -- ULID
  role         text NOT NULL,
  parent_id    text REFERENCES core.agents(id),     -- lineage: successor of (0046)
  config_hash  bytea NOT NULL,                      -- model + prompt + role + toolsets + grants
  codex_hash   bytea NOT NULL,                      -- laws in force at spawn
  public_key   bytea NOT NULL,                      -- signatures stay verifiable after retirement
  status       text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'hibernated', 'retired')),
  created_at   timestamptz NOT NULL DEFAULT now(),
  retired_at   timestamptz,
  CHECK ((status = 'retired') = (retired_at IS NOT NULL)),
  CHECK (parent_id IS NULL OR parent_id <> id)
);

-- ================================================================= tasks and change sets (0123)
CREATE TABLE core.change_sets (                     -- a re-plan of one task tree (0081)
  id           bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  root_id      text NOT NULL,
  reason       text NOT NULL,
  proposed_by  text NOT NULL,                       -- agent id or 'owner'
  state        text NOT NULL DEFAULT 'proposed' CHECK (state IN ('proposed', 'applied', 'rejected')),
  created_at   timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE core.tasks (
  id             text PRIMARY KEY,
  root_id        text NOT NULL,                     -- the chain for the margin cap (0111)
  parent_id      text REFERENCES core.tasks(id),
  title          text NOT NULL,
  spec_ref       text,                              -- artifact or Charter spec (0052)
  state          text NOT NULL DEFAULT 'draft',
  outcome        text CHECK (outcome IN ('accepted', 'failed', 'cancelled', 'superseded')),
  assignee_id    text REFERENCES core.agents(id),
  verifier_id    text REFERENCES core.agents(id),
  attempts       integer NOT NULL DEFAULT 0,
  max_attempts   integer NOT NULL DEFAULT 3 CHECK (max_attempts BETWEEN 1 AND 10),
  created_by_cs  bigint REFERENCES core.change_sets(id),
  superseded_by  bigint REFERENCES core.change_sets(id),
  created_at     timestamptz NOT NULL DEFAULT now(),
  updated_at     timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT verifier_not_worker CHECK (verifier_id IS NULL OR verifier_id IS DISTINCT FROM assignee_id),   -- verification by someone else (0053)
  CONSTRAINT attempts_bounded CHECK (attempts <= max_attempts),
  CONSTRAINT superseded_needs_change_set CHECK ((state = 'superseded' OR coalesce(outcome, '') = 'superseded') = (superseded_by IS NOT NULL)),
  CHECK ((state = 'closed') = (outcome IS NOT NULL) OR state IN ('accepted', 'failed', 'cancelled', 'superseded')),
  CHECK ((parent_id IS NULL) = (root_id = id))
);

CREATE TABLE core.task_deps (
  task_id     text NOT NULL REFERENCES core.tasks(id),
  depends_on  text NOT NULL REFERENCES core.tasks(id),
  PRIMARY KEY (task_id, depends_on),
  CHECK (task_id <> depends_on)
);

-- The task state machine (design/task-lifecycle.md (not in the repo)). Anything not listed is refused.
CREATE TABLE core.task_transitions (from_state text, to_state text, PRIMARY KEY (from_state, to_state));
INSERT INTO core.task_transitions VALUES
  ('draft', 'ready'), ('draft', 'cancelled'), ('draft', 'superseded'),
  ('ready', 'bidding'), ('ready', 'assigned'), ('ready', 'cancelled'), ('ready', 'superseded'),
  ('bidding', 'assigned'), ('bidding', 'ready'), ('bidding', 'cancelled'), ('bidding', 'superseded'),
  ('assigned', 'running'), ('assigned', 'cancelled'), ('assigned', 'superseded'),
  ('running', 'blocked'), ('running', 'verifying'), ('running', 'failed'), ('running', 'cancelled'), ('running', 'superseded'),
  ('blocked', 'running'), ('blocked', 'failed'), ('blocked', 'cancelled'), ('blocked', 'superseded'),
  ('verifying', 'accepted'), ('verifying', 'rework'), ('verifying', 'failed'),
  ('rework', 'running'), ('rework', 'failed'), ('rework', 'cancelled'), ('rework', 'superseded'),
  ('accepted', 'closed'), ('failed', 'closed'), ('cancelled', 'closed'), ('superseded', 'closed');

CREATE FUNCTION core.check_task() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE pending text;
BEGIN
  IF TG_OP = 'INSERT' THEN
    IF NEW.state <> 'draft' THEN RAISE EXCEPTION 'task % must start as draft', NEW.id; END IF;
    RETURN NEW;
  END IF;
  IF NEW.id <> OLD.id OR NEW.root_id <> OLD.root_id OR NEW.parent_id IS DISTINCT FROM OLD.parent_id THEN
    RAISE EXCEPTION 'task % may not move in the tree', OLD.id;
  END IF;
  IF OLD.state = 'closed' THEN RAISE EXCEPTION 'task % is closed', OLD.id; END IF;
  IF NEW.state <> OLD.state THEN
    IF NOT EXISTS (SELECT 1 FROM core.task_transitions WHERE from_state = OLD.state AND to_state = NEW.state) THEN
      RAISE EXCEPTION 'task % may not go from % to %', OLD.id, OLD.state, NEW.state;
    END IF;
    -- A task becomes ready only when everything it depends on was accepted.
    IF NEW.state = 'ready' AND OLD.state = 'draft' THEN
      SELECT d.depends_on INTO pending FROM core.task_deps d JOIN core.tasks t ON t.id = d.depends_on
      WHERE d.task_id = NEW.id AND coalesce(t.outcome, t.state) <> 'accepted' LIMIT 1;
      IF FOUND THEN RAISE EXCEPTION 'task % depends on %, which is not accepted', NEW.id, pending; END IF;
    END IF;
    IF NEW.state IN ('assigned', 'running') AND NEW.assignee_id IS NULL THEN
      RAISE EXCEPTION 'task % has no assignee', NEW.id;
    END IF;
    IF NEW.state = 'verifying' AND NEW.verifier_id IS NULL THEN
      RAISE EXCEPTION 'task % has no verifier', NEW.id;
    END IF;
    IF NEW.state = 'running' AND OLD.state = 'rework' THEN
      NEW.attempts := OLD.attempts + 1;               -- CHECK (attempts <= max_attempts) stops endless rework
    END IF;
    IF NEW.state = 'running' AND OLD.state = 'assigned' THEN NEW.attempts := 1; END IF;
    IF NEW.state = 'closed' THEN NEW.outcome := OLD.state; END IF;
    NEW.updated_at := now();
    INSERT INTO core.task_events (task_id, from_state, to_state) VALUES (NEW.id, OLD.state, NEW.state);
  ELSIF NEW.attempts <> OLD.attempts OR NEW.outcome IS DISTINCT FROM OLD.outcome THEN
    RAISE EXCEPTION 'task % attempts and outcome change only with its state', OLD.id;
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER tasks_lifecycle BEFORE INSERT OR UPDATE ON core.tasks FOR EACH ROW EXECUTE FUNCTION core.check_task();

CREATE TABLE core.task_events (
  id         bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  task_id    text NOT NULL REFERENCES core.tasks(id),
  from_state text NOT NULL,
  to_state   text NOT NULL,
  at         timestamptz NOT NULL DEFAULT now()
);

-- No dependency cycles, and dependencies stay inside one task tree.
CREATE FUNCTION core.check_dep() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF (SELECT root_id FROM core.tasks WHERE id = NEW.task_id) <> (SELECT root_id FROM core.tasks WHERE id = NEW.depends_on) THEN
    RAISE EXCEPTION 'dependency % -> % crosses task trees', NEW.task_id, NEW.depends_on;
  END IF;
  IF EXISTS (
    WITH RECURSIVE reach(id) AS (
      SELECT NEW.depends_on
      UNION SELECT d.depends_on FROM core.task_deps d JOIN reach r ON d.task_id = r.id)
    SELECT 1 FROM reach WHERE id = NEW.task_id) THEN
    RAISE EXCEPTION 'dependency % -> % would make a cycle', NEW.task_id, NEW.depends_on;
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER task_deps_acyclic BEFORE INSERT ON core.task_deps FOR EACH ROW EXECUTE FUNCTION core.check_dep();

-- ================================================================= runs, VMs, sessions
CREATE TABLE core.runs (
  id              text PRIMARY KEY,
  agent_id        text NOT NULL REFERENCES core.agents(id),
  task_id         text REFERENCES core.tasks(id),
  machine_id      text REFERENCES core.machines(id),
  state           text NOT NULL DEFAULT 'starting',
  model_rung      text NOT NULL CHECK (model_rung IN ('cheap', 'mid', 'frontier')),
  vcpus           integer NOT NULL DEFAULT 1 CHECK (vcpus BETWEEN 1 AND 64),
  memory_mib      integer NOT NULL DEFAULT 512 CHECK (memory_mib >= 128),
  idempotency_key text UNIQUE,                     -- StartRun retries never start two runs (0116)
  created_at      timestamptz NOT NULL DEFAULT now(),
  ended_at        timestamptz,
  CHECK ((state IN ('done', 'failed', 'killed')) = (ended_at IS NOT NULL))
);

-- Run states follow daemon.proto Run.State, plus 'stalled' for a lost machine (0102).
CREATE TABLE core.run_transitions (from_state text, to_state text, PRIMARY KEY (from_state, to_state));
INSERT INTO core.run_transitions VALUES
  ('starting', 'running'), ('starting', 'failed'), ('starting', 'killed'), ('starting', 'stalled'),
  ('running', 'waiting_approval'), ('running', 'hibernated'), ('running', 'stalled'),
  ('running', 'done'), ('running', 'failed'), ('running', 'killed'),
  ('waiting_approval', 'running'), ('waiting_approval', 'hibernated'), ('waiting_approval', 'stalled'),
  ('waiting_approval', 'failed'), ('waiting_approval', 'killed'),
  ('hibernated', 'starting'), ('hibernated', 'killed'),
  ('stalled', 'running'), ('stalled', 'starting'), ('stalled', 'failed'), ('stalled', 'killed');

CREATE FUNCTION core.check_run() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF TG_OP = 'INSERT' THEN
    IF NEW.state <> 'starting' THEN RAISE EXCEPTION 'run % must start as starting', NEW.id; END IF;
    RETURN NEW;
  END IF;
  IF NEW.agent_id <> OLD.agent_id OR NEW.task_id IS DISTINCT FROM OLD.task_id THEN
    RAISE EXCEPTION 'run % may not change agent or task', OLD.id;
  END IF;
  IF NEW.state <> OLD.state AND NOT EXISTS
     (SELECT 1 FROM core.run_transitions WHERE from_state = OLD.state AND to_state = NEW.state) THEN
    RAISE EXCEPTION 'run % may not go from % to %', OLD.id, OLD.state, NEW.state;
  END IF;
  -- Leaving a VM behind (move, hibernate, end) fences every session it had (0102).
  IF NEW.state IN ('done', 'failed', 'killed', 'hibernated') OR (OLD.state = 'stalled' AND NEW.state = 'starting') THEN
    UPDATE core.sessions SET fenced_at = now() WHERE run_id = NEW.id AND fenced_at IS NULL;
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER runs_lifecycle BEFORE INSERT OR UPDATE ON core.runs FOR EACH ROW EXECUTE FUNCTION core.check_run();

CREATE TABLE core.vms (
  id           text PRIMARY KEY,
  run_id       text NOT NULL REFERENCES core.runs(id),
  machine_id   text NOT NULL REFERENCES core.machines(id),
  trust_class  text NOT NULL CHECK (trust_class IN ('standard', 'low-trust', 'device')),   -- 0088, 0057
  created_at   timestamptz NOT NULL DEFAULT now(),
  destroyed_at timestamptz
);

CREATE TABLE core.sessions (                        -- per-boot session token (0044); epoch = token_epoch (0116)
  id          text PRIMARY KEY,
  run_id      text NOT NULL REFERENCES core.runs(id),
  vm_id       text NOT NULL REFERENCES core.vms(id),
  epoch       integer NOT NULL,
  token_hash  bytea NOT NULL UNIQUE,                -- the token itself never touches the database
  opened_at   timestamptz NOT NULL DEFAULT now(),
  fenced_at   timestamptz,
  UNIQUE (run_id, epoch)
);
-- At most one live session per run: the database form of "At most one copy of the agent is accepted" (crates/jhrl/specs/machines/agent_session.jhrl).
CREATE UNIQUE INDEX sessions_one_live ON core.sessions (run_id) WHERE fenced_at IS NULL;

CREATE FUNCTION core.check_session() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF TG_OP = 'UPDATE' THEN
    IF OLD.fenced_at IS NOT NULL AND NEW.fenced_at IS DISTINCT FROM OLD.fenced_at THEN
      RAISE EXCEPTION 'session % is fenced for good', OLD.id;
    END IF;
    IF NEW.id <> OLD.id OR NEW.run_id <> OLD.run_id OR NEW.epoch <> OLD.epoch OR NEW.token_hash <> OLD.token_hash THEN
      RAISE EXCEPTION 'session % is immutable apart from fencing', OLD.id;
    END IF;
    RETURN NEW;
  END IF;
  IF NEW.epoch <= coalesce((SELECT max(epoch) FROM core.sessions WHERE run_id = NEW.run_id), 0) THEN
    RAISE EXCEPTION 'session epoch % for run % is not newer than the last', NEW.epoch, NEW.run_id;
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER sessions_fencing BEFORE INSERT OR UPDATE ON core.sessions FOR EACH ROW EXECUTE FUNCTION core.check_session();

-- ================================================================= transcript (0084)
CREATE TABLE core.transcript (
  run_id      text NOT NULL REFERENCES core.runs(id),
  seq         bigint NOT NULL CHECK (seq >= 1),
  kind        text NOT NULL CHECK (kind IN ('user_turn', 'model_turn', 'tool_request', 'tool_decision',
                                            'tool_result', 'state_note', 'owner_message', 'system')),
  session_id  text REFERENCES core.sessions(id),   -- set for everything the harness caused
  call_id     text,
  decision    text CHECK (decision IN ('allow', 'deny', 'pending_approval')),
  law_id      text,                                 -- on denials (0038)
  taint       text NOT NULL DEFAULT 'trusted' CHECK (taint IN ('trusted', 'untrusted')),
  body        jsonb,                                -- small payloads inline
  blob        bytea,                                -- or a BLAKE3 reference to Garage
  entry_hash  bytea NOT NULL,                       -- BLAKE3 of the canonical entry (0135)
  prev_hash   bytea NOT NULL,                       -- entry_hash of seq - 1 ('\x00' for seq 1): the chain
  seal        bytea NOT NULL,                       -- orchestrator signature over run, seq, epoch, prev_hash, entry_hash
  at          timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (run_id, seq),
  CHECK ((kind IN ('tool_request', 'tool_decision', 'tool_result')) = (call_id IS NOT NULL)),
  CHECK ((kind = 'tool_decision') = (decision IS NOT NULL)),
  CONSTRAINT denial_names_law CHECK (decision IS DISTINCT FROM 'deny' OR law_id IS NOT NULL)
);
CREATE UNIQUE INDEX transcript_one_decision ON core.transcript (run_id, call_id) WHERE kind = 'tool_decision';
CREATE UNIQUE INDEX transcript_one_result   ON core.transcript (run_id, call_id) WHERE kind = 'tool_result';

CREATE FUNCTION core.check_transcript() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE last bigint;
BEGIN
  -- Gapless, in order. Writers serialise per run (SELECT ... FOR UPDATE on the run row).
  SELECT coalesce(max(seq), 0) INTO last FROM core.transcript WHERE run_id = NEW.run_id;
  IF NEW.seq <> last + 1 THEN
    RAISE EXCEPTION 'transcript of run % expects seq %, got %', NEW.run_id, last + 1, NEW.seq;
  END IF;
  -- The seal chain (0135): each entry names the hash of the one before it.
  IF NEW.prev_hash <> coalesce((SELECT entry_hash FROM core.transcript WHERE run_id = NEW.run_id AND seq = last), '\x00'::bytea) THEN
    RAISE EXCEPTION 'transcript of run % breaks the seal chain at seq %', NEW.run_id, NEW.seq;
  END IF;
  -- Only the run's live session writes ("Only the current session writes the transcript", crates/jhrl/specs/machines/agent_session.jhrl).
  IF NEW.session_id IS NOT NULL AND NOT EXISTS
     (SELECT 1 FROM core.sessions WHERE id = NEW.session_id AND run_id = NEW.run_id AND fenced_at IS NULL) THEN
    RAISE EXCEPTION 'session % is not the live session of run %', NEW.session_id, NEW.run_id;
  END IF;
  IF NEW.kind IN ('model_turn', 'tool_request', 'tool_result') AND NEW.session_id IS NULL THEN
    RAISE EXCEPTION '% entries must name the session that caused them', NEW.kind;
  END IF;
  -- A decision answers a request; a result follows an allow (NoStaleEffect: nothing runs undecided).
  IF NEW.kind = 'tool_decision' AND NOT EXISTS
     (SELECT 1 FROM core.transcript WHERE run_id = NEW.run_id AND call_id = NEW.call_id AND kind = 'tool_request') THEN
    RAISE EXCEPTION 'decision for unknown call %', NEW.call_id;
  END IF;
  IF NEW.kind = 'tool_result' AND NOT EXISTS
     (SELECT 1 FROM core.transcript WHERE run_id = NEW.run_id AND call_id = NEW.call_id
        AND kind = 'tool_decision' AND decision = 'allow') THEN
    RAISE EXCEPTION 'result for call % that was never allowed', NEW.call_id;
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER transcript_order BEFORE INSERT ON core.transcript FOR EACH ROW EXECUTE FUNCTION core.check_transcript();

-- ================================================================= snapshots (0029, 0091)
CREATE TABLE core.snapshots (
  id            text PRIMARY KEY,
  run_id        text NOT NULL REFERENCES core.runs(id),
  parent_id     text REFERENCES core.snapshots(id),
  seq           bigint NOT NULL,                    -- transcript seq the snapshot is consistent with
  arch          text NOT NULL,
  backend       text NOT NULL,
  kernel_build  text NOT NULL,
  manifest_hash bytea NOT NULL,
  bytes_written bigint NOT NULL,
  created_at    timestamptz NOT NULL DEFAULT now(),
  uploaded_at   timestamptz                         -- null while only in the machine's local cache
);
CREATE TABLE core.chunks (hash bytea PRIMARY KEY, size integer NOT NULL CHECK (size BETWEEN 1 AND 262144));
CREATE TABLE core.snapshot_chunks (
  snapshot_id text NOT NULL REFERENCES core.snapshots(id),
  chunk_hash  bytea NOT NULL REFERENCES core.chunks(hash),
  PRIMARY KEY (snapshot_id, chunk_hash)
);

CREATE FUNCTION core.check_snapshot() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE p core.snapshots;
BEGIN
  IF NEW.seq > coalesce((SELECT max(seq) FROM core.transcript WHERE run_id = NEW.run_id), 0) THEN
    RAISE EXCEPTION 'snapshot % claims seq % beyond the transcript', NEW.id, NEW.seq;
  END IF;
  IF NEW.parent_id IS NOT NULL THEN
    SELECT * INTO p FROM core.snapshots WHERE id = NEW.parent_id;
    IF p.run_id <> NEW.run_id OR p.seq > NEW.seq OR p.arch <> NEW.arch OR p.backend <> NEW.backend THEN
      RAISE EXCEPTION 'snapshot % does not continue its parent %', NEW.id, NEW.parent_id;
    END IF;
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER snapshots_chain BEFORE INSERT ON core.snapshots FOR EACH ROW EXECUTE FUNCTION core.check_snapshot();

-- A restore can only use an uploaded snapshot (MoveRun after machine loss, 0102).
CREATE VIEW core.restorable_snapshots AS
  SELECT DISTINCT ON (run_id) * FROM core.snapshots WHERE uploaded_at IS NOT NULL ORDER BY run_id, seq DESC, created_at DESC;

-- Chunk GC: chunks no snapshot of a live run refers to (closes the vmm.md (not in the repo) open point on chunk GC).
CREATE VIEW core.collectable_chunks AS
  SELECT c.hash, c.size FROM core.chunks c
  WHERE NOT EXISTS (
    SELECT 1 FROM core.snapshot_chunks sc JOIN core.snapshots s ON s.id = sc.snapshot_id
    JOIN core.runs r ON r.id = s.run_id
    WHERE sc.chunk_hash = c.hash AND r.state NOT IN ('done', 'failed', 'killed'));

-- ================================================================= approvals (0094, 0116)
CREATE TABLE core.approvals (
  id            text PRIMARY KEY,
  run_id        text NOT NULL REFERENCES core.runs(id),
  kind          text NOT NULL,
  summary       text NOT NULL,                      -- written by the orchestrator, never the agent
  risk_class    text NOT NULL,
  c004          boolean NOT NULL DEFAULT false,     -- irreversible / external / money-moving (0051)
  chain         text[] NOT NULL CHECK (cardinality(chain) >= 1),
  position      integer NOT NULL DEFAULT 1,
  step_deadline timestamptz NOT NULL,
  nonce         bytea NOT NULL,                     -- fresh per step; a decision must echo it
  status        text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'approved', 'denied', 'expired')),
  decided_as    text,                               -- the authority, e.g. 'owner'
  decided_by    text,                               -- the principal: device id or agent id
  decided_at    timestamptz,
  CONSTRAINT c004_owner_only CHECK (NOT c004 OR chain = ARRAY['owner']),       -- "C-004 is approved only by the owner" (crates/jhrl/specs/machines/approval.jhrl)
  CHECK (position BETWEEN 1 AND cardinality(chain)),
  CHECK ((status IN ('approved', 'denied')) = (decided_as IS NOT NULL AND decided_by IS NOT NULL)),
  CONSTRAINT decided_by_chain CHECK (decided_as IS NULL OR decided_as = chain[position])   -- DecidedByChain
);
CREATE TABLE core.used_nonces (nonce bytea PRIMARY KEY, approval_id text NOT NULL REFERENCES core.approvals(id), used_at timestamptz NOT NULL DEFAULT now());

CREATE FUNCTION core.check_approval() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF OLD.status <> 'pending' THEN RAISE EXCEPTION 'approval % is final (%)', OLD.id, OLD.status; END IF;   -- FinalStays
  IF NEW.chain <> OLD.chain OR NEW.c004 <> OLD.c004 OR NEW.run_id <> OLD.run_id THEN
    RAISE EXCEPTION 'approval % chain is fixed', OLD.id;
  END IF;
  IF NEW.position < OLD.position THEN RAISE EXCEPTION 'approval % cannot step back', OLD.id; END IF;
  IF NEW.position > OLD.position AND NEW.nonce = OLD.nonce THEN
    RAISE EXCEPTION 'approval % needs a fresh nonce for the next step', OLD.id;
  END IF;
  IF NEW.status = 'expired' AND OLD.position <> cardinality(OLD.chain) THEN
    RAISE EXCEPTION 'approval % expires only after its last step', OLD.id;
  END IF;
  IF NEW.status IN ('approved', 'denied') THEN
    INSERT INTO core.used_nonces (nonce, approval_id) VALUES (OLD.nonce, OLD.id);   -- a decision is used once
    NEW.decided_at := now();
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER approvals_flow BEFORE UPDATE ON core.approvals FOR EACH ROW EXECUTE FUNCTION core.check_approval();

-- ================================================================= events, idempotency, audit
CREATE TABLE core.events (                          -- the resumable event log; id is the cursor (0093)
  id      bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  at      timestamptz NOT NULL DEFAULT now(),
  run_id  text REFERENCES core.runs(id),
  kind    text NOT NULL CHECK (kind IN ('run', 'transcript', 'approval', 'vm', 'egress', 'task', 'machine',
                                    'inbox', 'codex', 'vault', 'restore')),   -- matches daemon.proto EventKind
  payload jsonb NOT NULL
);

CREATE TABLE core.idempotency (                     -- daemon API retries (0116)
  key          text PRIMARY KEY,
  method       text NOT NULL,
  principal    text NOT NULL,
  request_hash bytea NOT NULL,
  response     bytea,
  created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE FUNCTION core.check_idempotency() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NEW.key <> OLD.key OR NEW.method <> OLD.method OR NEW.principal <> OLD.principal OR NEW.request_hash <> OLD.request_hash THEN
    RAISE EXCEPTION 'idempotency key % was used for a different request', OLD.key;
  END IF;
  IF OLD.response IS NOT NULL AND NEW.response IS DISTINCT FROM OLD.response THEN
    RAISE EXCEPTION 'idempotency key % already has a response', OLD.key;
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER idempotency_stable BEFORE UPDATE ON core.idempotency FOR EACH ROW EXECUTE FUNCTION core.check_idempotency();

-- Audit log with a hash chain (C-001: log tampering must be evident). Each row's hash covers the
-- previous hash, so editing or deleting a row breaks every hash after it.
CREATE TABLE core.audit (
  id         bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  at         timestamptz NOT NULL DEFAULT now(),
  principal  text NOT NULL,                         -- owner device, agent, machine, or service
  action     text NOT NULL,                         -- Cedar action
  resource   text NOT NULL,
  decision   text NOT NULL CHECK (decision IN ('allow', 'deny')),
  law_id     text,
  detail     jsonb NOT NULL DEFAULT '{}',
  prev_hash  bytea NOT NULL,
  hash       bytea NOT NULL
);
CREATE FUNCTION core.audit_row_hash(prev bytea, a core.audit) RETURNS bytea LANGUAGE sql IMMUTABLE AS $$
  SELECT sha256(prev || convert_to(concat_ws('|', a.id, a.at, a.principal, a.action, a.resource, a.decision,
                                             coalesce(a.law_id, ''), a.detail::text), 'UTF8'))
$$;
CREATE FUNCTION core.chain_audit() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  PERFORM pg_advisory_xact_lock(hashtext('core.audit'));   -- one writer at a time keeps the chain linear
  NEW.prev_hash := coalesce((SELECT hash FROM core.audit ORDER BY id DESC LIMIT 1), '\x00'::bytea);
  NEW.hash := core.audit_row_hash(NEW.prev_hash, NEW);
  RETURN NEW;
END $$;
CREATE TRIGGER audit_chain BEFORE INSERT ON core.audit FOR EACH ROW EXECUTE FUNCTION core.chain_audit();

-- Returns the first row whose hash doesn't verify, or null if the whole chain is intact.
CREATE FUNCTION core.verify_audit() RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE r core.audit; prev bytea := '\x00'::bytea;
BEGIN
  FOR r IN SELECT * FROM core.audit ORDER BY id LOOP
    IF r.prev_hash <> prev OR r.hash <> core.audit_row_hash(prev, r) THEN RETURN r.id; END IF;
    prev := r.hash;
  END LOOP;
  RETURN NULL;
END $$;

-- ================================================================= append-only records
CREATE FUNCTION core.forbid_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION '% is append-only', TG_TABLE_NAME; END $$;
CREATE TRIGGER transcript_append_only  BEFORE UPDATE OR DELETE ON core.transcript  FOR EACH ROW EXECUTE FUNCTION core.forbid_change();
CREATE TRIGGER events_append_only      BEFORE UPDATE OR DELETE ON core.events      FOR EACH ROW EXECUTE FUNCTION core.forbid_change();
CREATE TRIGGER audit_append_only       BEFORE UPDATE OR DELETE ON core.audit       FOR EACH ROW EXECUTE FUNCTION core.forbid_change();
CREATE TRIGGER task_events_append_only BEFORE UPDATE OR DELETE ON core.task_events FOR EACH ROW EXECUTE FUNCTION core.forbid_change();
CREATE TRIGGER nonces_append_only      BEFORE UPDATE OR DELETE ON core.used_nonces FOR EACH ROW EXECUTE FUNCTION core.forbid_change();
