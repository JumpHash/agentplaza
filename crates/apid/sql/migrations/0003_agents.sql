-- 0003 agents: the roster rows (agents), the custom roles (roles) and the proposals that spawn
-- agents (agent_proposals, proposal_roles, agent_proposal_spawns). docs/design/database.md 2.1.
-- Built-in roles come from code, so roles holds only custom roles and tasks.role stays plain text.
-- A proposal's role snapshot and its spawned agents are child tables: each is filtered by proposal.

CREATE TABLE agents (
    cid bigint PRIMARY KEY CHECK (cid >= 0),
    name text NOT NULL,
    role text NOT NULL,
    model text,
    species text NOT NULL DEFAULT '',
    species_at timestamptz,
    state text NOT NULL DEFAULT 'active' CHECK (state IN ('active', 'retired')),
    created_at timestamptz NOT NULL DEFAULT now(),
    retired_at timestamptz,
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX agents_state_cid ON agents (state, cid);

CREATE TABLE roles (
    id text PRIMARY KEY CHECK (id ~ '^[a-z0-9-]{2,32}$'),
    title text NOT NULL,
    purpose text NOT NULL,
    collaboration text NOT NULL DEFAULT '',
    budget bigint NOT NULL DEFAULT 0 CHECK (budget >= 0),
    model text NOT NULL DEFAULT '',
    prompt text NOT NULL DEFAULT '',
    created_by text NOT NULL DEFAULT '',
    created_at timestamptz NOT NULL DEFAULT now(),
    responsibilities text[] NOT NULL DEFAULT '{}',
    deliverables text[] NOT NULL DEFAULT '{}',
    boundaries text[] NOT NULL DEFAULT '{}',
    tools text[] NOT NULL DEFAULT '{}'
);

CREATE TABLE agent_proposals (
    id text PRIMARY KEY CHECK (id ~ '^ap[0-9]+$'),
    new_role boolean NOT NULL,
    count integer NOT NULL CHECK (count BETWEEN 1 AND 8),
    model text NOT NULL DEFAULT '',
    reason text NOT NULL,
    instruction text NOT NULL,
    by_cid bigint NOT NULL REFERENCES agents (cid),
    by text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    state text NOT NULL CHECK (state IN ('pending', 'approved', 'rejected')),
    decided_at timestamptz,
    note text
);
CREATE INDEX agent_proposals_state_created ON agent_proposals (state, created_at);

CREATE TABLE proposal_roles (
    proposal_id text PRIMARY KEY REFERENCES agent_proposals (id) ON DELETE CASCADE,
    id text NOT NULL,
    title text NOT NULL DEFAULT '',
    purpose text NOT NULL DEFAULT '',
    collaboration text NOT NULL DEFAULT '',
    budget bigint NOT NULL DEFAULT 0 CHECK (budget >= 0),
    model text NOT NULL DEFAULT '',
    prompt text NOT NULL DEFAULT '',
    created_by text NOT NULL DEFAULT '',
    created_at timestamptz NOT NULL DEFAULT now(),
    responsibilities text[] NOT NULL DEFAULT '{}',
    deliverables text[] NOT NULL DEFAULT '{}',
    boundaries text[] NOT NULL DEFAULT '{}',
    tools text[] NOT NULL DEFAULT '{}'
);

CREATE TABLE agent_proposal_spawns (
    proposal_id text NOT NULL REFERENCES agent_proposals (id) ON DELETE CASCADE,
    cid bigint NOT NULL REFERENCES agents (cid),
    PRIMARY KEY (proposal_id, cid)
);
CREATE INDEX agent_proposal_spawns_cid ON agent_proposal_spawns (cid);
