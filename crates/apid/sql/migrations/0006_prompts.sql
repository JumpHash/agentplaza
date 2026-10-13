-- 0006 prompts: the prompt library (docs/design/prompt-library.md). prompts holds one row per system prompt, AGENTS.md
-- or fragment, with its binding and live version; prompt_versions the immutable bodies with their check scores;
-- prompt_findings each check finding of a version, under the rule set the version was scored with; prompt_events the
-- history of every create, version, publish, unpublish, edit, archive and restore. The last three are append-only.
CREATE TABLE prompts (
    id text PRIMARY KEY CHECK (id ~ '^[a-z0-9][a-z0-9-]{0,62}$'),
    kind text NOT NULL CHECK (kind IN ('system', 'agents_md', 'fragment')),
    title text NOT NULL CHECK (char_length(btrim(title)) BETWEEN 1 AND 120),
    description text NOT NULL DEFAULT '' CHECK (char_length(description) <= 500),
    tags text[] NOT NULL DEFAULT '{}' CHECK (cardinality(tags) <= 16),
    binding text CHECK (
        binding IS NULL
        OR (kind = 'system' AND binding ~ '^role:[a-z0-9-]{1,64}$')
        OR (kind = 'agents_md' AND binding = 'plaza')
    ),
    active_version integer CHECK (active_version >= 1),
    latest_version integer NOT NULL DEFAULT 1 CHECK (latest_version >= 1),
    created timestamptz NOT NULL,
    updated timestamptz NOT NULL,
    archived_at timestamptz
);
-- One live prompt per binding.
CREATE UNIQUE INDEX prompts_binding_live ON prompts (binding) WHERE binding IS NOT NULL AND archived_at IS NULL;

CREATE TABLE prompt_versions (
    prompt_id text NOT NULL REFERENCES prompts (id),
    version integer NOT NULL CHECK (version >= 1),
    body text NOT NULL CHECK (char_length(body) <= 200000),
    body_sha256 bytea NOT NULL CHECK (octet_length(body_sha256) = 32),
    note text NOT NULL DEFAULT '' CHECK (char_length(note) <= 500),
    author text NOT NULL CHECK (char_length(author) BETWEEN 1 AND 80),
    score smallint NOT NULL CHECK (score BETWEEN 0 AND 100),
    errors integer NOT NULL CHECK (errors >= 0),
    warnings integer NOT NULL CHECK (warnings >= 0),
    infos integer NOT NULL CHECK (infos >= 0),
    rules_version integer NOT NULL CHECK (rules_version >= 1),
    created timestamptz NOT NULL,
    PRIMARY KEY (prompt_id, version)
);
CREATE TRIGGER prompt_versions_no_update_delete BEFORE UPDATE OR DELETE ON prompt_versions
    FOR EACH ROW EXECUTE FUNCTION forbid_mutation();
CREATE TRIGGER prompt_versions_no_truncate BEFORE TRUNCATE ON prompt_versions
    FOR EACH STATEMENT EXECUTE FUNCTION forbid_mutation();

-- The live version must be one of the prompt's versions (no check when it is null).
ALTER TABLE prompts ADD CONSTRAINT prompts_active_version_fk
    FOREIGN KEY (id, active_version) REFERENCES prompt_versions (prompt_id, version)
    DEFERRABLE INITIALLY DEFERRED;

CREATE TABLE prompt_findings (
    prompt_id text NOT NULL,
    version integer NOT NULL,
    seq integer NOT NULL CHECK (seq >= 0),
    rule text NOT NULL CHECK (rule ~ '^[a-z][a-z0-9-]{1,40}$'),
    severity text NOT NULL CHECK (severity IN ('error', 'warning', 'info')),
    line integer NOT NULL CHECK (line >= 0),
    message text NOT NULL CHECK (char_length(message) BETWEEN 1 AND 300),
    PRIMARY KEY (prompt_id, version, seq),
    FOREIGN KEY (prompt_id, version) REFERENCES prompt_versions (prompt_id, version)
);
CREATE INDEX prompt_findings_rule ON prompt_findings (rule);
CREATE TRIGGER prompt_findings_no_update_delete BEFORE UPDATE OR DELETE ON prompt_findings
    FOR EACH ROW EXECUTE FUNCTION forbid_mutation();
CREATE TRIGGER prompt_findings_no_truncate BEFORE TRUNCATE ON prompt_findings
    FOR EACH STATEMENT EXECUTE FUNCTION forbid_mutation();

CREATE TABLE prompt_events (
    id bigserial PRIMARY KEY,
    prompt_id text NOT NULL REFERENCES prompts (id),
    action text NOT NULL CHECK (action IN ('create', 'version', 'publish', 'unpublish', 'edit', 'archive', 'restore')),
    version integer CHECK (version >= 1),
    actor text NOT NULL CHECK (char_length(actor) BETWEEN 1 AND 80),
    at timestamptz NOT NULL
);
CREATE INDEX prompt_events_prompt ON prompt_events (prompt_id, id);
CREATE TRIGGER prompt_events_no_update_delete BEFORE UPDATE OR DELETE ON prompt_events
    FOR EACH ROW EXECUTE FUNCTION forbid_mutation();
CREATE TRIGGER prompt_events_no_truncate BEFORE TRUNCATE ON prompt_events
    FOR EACH STATEMENT EXECUTE FUNCTION forbid_mutation();
