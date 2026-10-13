-- Treasury and Agora schema, draft (decisions 0039, 0109-0112). PostgreSQL 16.
-- Amounts are whole credits (1 credit = $0.001 of real spend, 0109), stored as bigint.

CREATE SCHEMA treasury;

-- ---------------------------------------------------------------- accounts
CREATE TYPE treasury.account_kind AS ENUM (
  'mint',            -- governance source of all credits; may go arbitrarily negative
  'treasury',        -- fines, filing fees, reclaimed balances
  'town_budget',     -- one per town: funded monthly by the mint; task budgets are drawn from it (0137)
  'task_escrow',     -- one per task: its budget; operating funds only (0039)
  'agent_earnings',  -- one per agent: accumulated margin; buys autonomy, never real money
  'external_spend'   -- real money leaving: model providers, tools, compute
);

CREATE TABLE treasury.accounts (
  id            bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  kind          treasury.account_kind NOT NULL,
  owner_ref     text NOT NULL,              -- task id, agent id (ULID), or a fixed name
  credit_limit  bigint NOT NULL DEFAULT 0,  -- earnings may go negative down to -credit_limit (set by reputation)
  created_at    timestamptz NOT NULL DEFAULT now(),
  UNIQUE (kind, owner_ref),
  CHECK (credit_limit >= 0)
);

-- ---------------------------------------------------------------- double-entry journal
CREATE TYPE treasury.entry_kind AS ENUM (
  'mint',            -- owner/council funds a task budget
  'spend',           -- metered real cost: escrow -> external_spend
  'subcontract',     -- award: parent escrow locks the price in the child task's escrow
  'payment',         -- verified work: child escrow -> contractor earnings (the margin; real cost was already spent)
  'refund',          -- unused escrow back to the parent escrow or mint
  'clawback',        -- warranty claim: contractor earnings -> escrow/treasury
  'fine',            -- sanction: earnings -> treasury (from earnings only, never escrow)
  'fee',             -- appeal filing fee (earnings -> treasury), refunded on success (treasury -> earnings)
  'budget_move',     -- owner or council moves budget between towns (0137); over 10% of the source needs the owner (0067)
  'model'            -- a model call's metered cost: task escrow (or the agent's wallet when no task) -> external_spend
);

-- Which account kinds each entry kind may debit (amount < 0) and credit (amount > 0).
-- Anything not listed is refused at commit, so e.g. a fine can never touch escrow.
CREATE TABLE treasury.posting_rules (
  entry_kind    treasury.entry_kind   NOT NULL,
  account_kind  treasury.account_kind NOT NULL,
  side          text NOT NULL CHECK (side IN ('debit', 'credit')),
  PRIMARY KEY (entry_kind, account_kind, side)
);
INSERT INTO treasury.posting_rules VALUES
  ('mint',        'mint',           'debit'),  ('mint',        'task_escrow',    'credit'),
  ('mint',        'town_budget',    'credit'),
  ('subcontract', 'town_budget',    'debit'),                                   -- a town funds a root task
  ('refund',      'town_budget',    'credit'),                                  -- a root task refunds its town
  ('budget_move', 'town_budget',    'debit'),  ('budget_move', 'town_budget',    'credit'),
  ('spend',       'task_escrow',    'debit'),  ('spend',       'external_spend', 'credit'),
  ('subcontract', 'task_escrow',    'debit'),  ('subcontract', 'task_escrow',    'credit'),
  ('payment',     'task_escrow',    'debit'),  ('payment',     'agent_earnings', 'credit'),
  ('refund',      'task_escrow',    'debit'),  ('refund',      'task_escrow',    'credit'),
  ('refund',      'mint',           'credit'),
  ('clawback',    'agent_earnings', 'debit'),  ('clawback',    'task_escrow',    'credit'),
  ('clawback',    'treasury',       'credit'),
  ('fine',        'agent_earnings', 'debit'),  ('fine',        'treasury',       'credit'),
  ('fee',         'agent_earnings', 'debit'),  ('fee',         'treasury',       'credit'),
  ('fee',         'treasury',       'debit'),  ('fee',         'agent_earnings', 'credit'),
  ('model',       'task_escrow',    'debit'),  ('model',       'agent_earnings', 'debit'),
  ('model',       'external_spend', 'credit');

-- Total margin per task chain, as a percentage of the chain's real cost (0111).
CREATE FUNCTION treasury.margin_cap_pct() RETURNS integer LANGUAGE sql IMMUTABLE AS 'SELECT 20';

CREATE TABLE treasury.entries (
  id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  kind        treasury.entry_kind NOT NULL,
  task_id     text,                         -- the task this entry belongs to, if any
  root_task   text,                         -- root of the task chain (margin cap, 0111)
  memo        text NOT NULL DEFAULT '',
  posted_by   text NOT NULL,                -- always orchestrator code, never an agent (0039)
  created_at  timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT posted_by_service CHECK (posted_by IN ('treasury-service', 'agora-service', 'tribunal-service')),
  CONSTRAINT task_money_names_chain CHECK (kind IN ('fine', 'fee', 'model') OR root_task IS NOT NULL)   -- money that moves for a task names its chain; a model call without a task pays from the wallet
);

CREATE TABLE treasury.postings (
  entry_id    bigint NOT NULL REFERENCES treasury.entries(id),
  account_id  bigint NOT NULL REFERENCES treasury.accounts(id),
  amount      bigint NOT NULL CHECK (amount <> 0),   -- credits; negative = debit, positive = credit
  PRIMARY KEY (entry_id, account_id)
);

-- Every entry must balance to zero, and no account may overdraw its limit.
-- Checked at commit time, so an entry's postings can be inserted one by one.
CREATE FUNCTION treasury.check_entry() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
  total bigint;
  bad   record;
  e     record;
  m     record;
BEGIN
  SELECT * INTO e FROM treasury.entries WHERE id = NEW.entry_id;

  SELECT a.kind AS account_kind, CASE WHEN p.amount < 0 THEN 'debit' ELSE 'credit' END AS side INTO bad
  FROM treasury.postings p JOIN treasury.accounts a ON a.id = p.account_id
  WHERE p.entry_id = NEW.entry_id
    AND NOT EXISTS (SELECT 1 FROM treasury.posting_rules r
                    WHERE r.entry_kind = e.kind AND r.account_kind = a.kind
                      AND r.side = CASE WHEN p.amount < 0 THEN 'debit' ELSE 'credit' END)
  LIMIT 1;
  IF FOUND THEN
    RAISE EXCEPTION '% entry may not % a % account', e.kind, bad.side, bad.account_kind;
  END IF;

  SELECT coalesce(sum(amount), 0) INTO total FROM treasury.postings WHERE entry_id = NEW.entry_id;
  IF total <> 0 THEN
    RAISE EXCEPTION 'entry % does not balance (sum = %)', NEW.entry_id, total;
  END IF;

  SELECT a.id, a.kind, b.balance, a.credit_limit INTO bad
  FROM treasury.accounts a
  JOIN treasury.balances b ON b.account_id = a.id
  WHERE a.id IN (SELECT account_id FROM treasury.postings WHERE entry_id = NEW.entry_id)
    AND a.kind IN ('task_escrow', 'agent_earnings', 'treasury', 'town_budget')
    AND b.balance < -a.credit_limit
  LIMIT 1;
  IF FOUND THEN
    RAISE EXCEPTION 'account % (%) would be at %, below its limit of -%', bad.id, bad.kind, bad.balance, bad.credit_limit;
  END IF;

  IF e.kind = 'payment' THEN
    SELECT * INTO m FROM treasury.chain_margin WHERE root_task = e.root_task;
    IF m.paid_to_agents * 100 > m.real_cost * treasury.margin_cap_pct() THEN
      RAISE EXCEPTION 'payment breaks the chain margin cap: % paid against % real cost in chain % (cap %%%)',
        m.paid_to_agents, m.real_cost, e.root_task, treasury.margin_cap_pct();
    END IF;
  END IF;
  RETURN NULL;
END $$;

CREATE CONSTRAINT TRIGGER postings_balanced
  AFTER INSERT ON treasury.postings
  DEFERRABLE INITIALLY DEFERRED
  FOR EACH ROW EXECUTE FUNCTION treasury.check_entry();

-- The journal is append-only: corrections are new entries, never edits.
CREATE FUNCTION treasury.forbid_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  RAISE EXCEPTION 'treasury journal is append-only';
END $$;

CREATE TRIGGER entries_append_only  BEFORE UPDATE OR DELETE ON treasury.entries
  FOR EACH ROW EXECUTE FUNCTION treasury.forbid_change();
CREATE TRIGGER postings_append_only BEFORE UPDATE OR DELETE ON treasury.postings
  FOR EACH ROW EXECUTE FUNCTION treasury.forbid_change();

CREATE VIEW treasury.balances AS
  SELECT a.id AS account_id, a.kind, a.owner_ref, coalesce(sum(p.amount), 0)::bigint AS balance
  FROM treasury.accounts a
  LEFT JOIN treasury.postings p ON p.account_id = a.id
  GROUP BY a.id, a.kind, a.owner_ref;

-- Margin cap per task chain (0111): contractors' total margin <= 20% of real cost in the chain.
-- Real cost is only money that left (spend); subcontract transfers are not cost, so hops can't stack margin.
-- Margin is net of clawbacks.
CREATE VIEW treasury.chain_margin AS
  SELECT e.root_task,
         sum(CASE WHEN e.kind = 'spend' AND p.amount > 0 THEN p.amount ELSE 0 END)::bigint AS real_cost,
         sum(CASE WHEN e.kind IN ('payment', 'clawback') AND a.kind = 'agent_earnings' THEN p.amount ELSE 0 END)::bigint AS paid_to_agents
  FROM treasury.entries e
  JOIN treasury.postings p ON p.entry_id = e.id
  JOIN treasury.accounts a ON a.id = p.account_id
  WHERE e.root_task IS NOT NULL
  GROUP BY e.root_task;

-- ---------------------------------------------------------------- Agora (0073, 0112)
CREATE SCHEMA agora;

CREATE TABLE agora.calls (
  id             bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  task_id        text NOT NULL,
  requester      text NOT NULL,              -- agent id
  capability     text NOT NULL,              -- what is needed, matched against agent cards
  spec_ref       text NOT NULL,              -- artifact with spec and acceptance criteria
  max_price      bigint NOT NULL CHECK (max_price > 0),
  deadline       timestamptz NOT NULL,
  invited        text[] NOT NULL,            -- at most 3 best-matching candidates (0112)
  state          text NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'awarded', 'cancelled', 'expired')),
  created_at     timestamptz NOT NULL DEFAULT now(),
  CHECK (cardinality(invited) BETWEEN 1 AND 3)
);

CREATE TABLE agora.bids (
  call_id     bigint NOT NULL REFERENCES agora.calls(id),
  bidder      text NOT NULL,
  price       bigint NOT NULL CHECK (price > 0),
  eta         interval NOT NULL,
  confidence  real NOT NULL CHECK (confidence BETWEEN 0 AND 1),
  created_at  timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (call_id, bidder)
);

CREATE TABLE agora.awards (
  call_id        bigint PRIMARY KEY REFERENCES agora.calls(id),
  contractor     text NOT NULL,
  price          bigint NOT NULL,
  escrow_entry   bigint REFERENCES treasury.entries(id),
  warranty_until timestamptz NOT NULL,       -- clawback window (0039)
  outcome        text CHECK (outcome IN ('passed', 'failed', 'clawed_back'))
);

-- Only invited candidates bid, on open calls, at or under the max price;
-- an award goes to a bidder at the price they bid (0112).
CREATE FUNCTION agora.check_bid() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE c agora.calls;
BEGIN
  SELECT * INTO c FROM agora.calls WHERE id = NEW.call_id;
  IF c.state <> 'open' THEN RAISE EXCEPTION 'call % is %, not open', c.id, c.state; END IF;
  IF NOT NEW.bidder = ANY (c.invited) THEN RAISE EXCEPTION 'bidder % was not invited to call %', NEW.bidder, c.id; END IF;
  IF NEW.price > c.max_price THEN RAISE EXCEPTION 'bid price % exceeds max_price %', NEW.price, c.max_price; END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER bids_valid BEFORE INSERT ON agora.bids FOR EACH ROW EXECUTE FUNCTION agora.check_bid();

CREATE FUNCTION agora.check_award() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM agora.bids WHERE call_id = NEW.call_id AND bidder = NEW.contractor AND price = NEW.price) THEN
    RAISE EXCEPTION 'award to % at % matches no bid on call %', NEW.contractor, NEW.price, NEW.call_id;
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER awards_valid BEFORE INSERT ON agora.awards FOR EACH ROW EXECUTE FUNCTION agora.check_award();

-- Reputation counters per agent (0039); derived scores are computed from these.
CREATE TABLE agora.reputation (
  agent_id        text PRIMARY KEY,
  verified        integer NOT NULL DEFAULT 0,
  failed          integer NOT NULL DEFAULT 0,
  reworked        integer NOT NULL DEFAULT 0,
  clawbacks       integer NOT NULL DEFAULT 0,
  quoted_credits  bigint  NOT NULL DEFAULT 0,  -- sum of bid prices on completed work
  actual_credits  bigint  NOT NULL DEFAULT 0,  -- sum of real cost on that work (bid accuracy)
  on_time         integer NOT NULL DEFAULT 0,
  late            integer NOT NULL DEFAULT 0
);
