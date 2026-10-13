-- 0007 skills: the skill library (docs/design/skill-library.md). skills holds one row per skill with its source;
-- skill_bindings the roles (or the whole plaza) it is handed to; skill_versions the immutable SKILL.md texts with
-- their check scores; skill_files each version's files (the bytes are content-addressed blobs outside Postgres, in
-- Garage or the state dir); skill_findings each check finding; skill_events the history. The last four are append-only.
CREATE TABLE skills (
    id text PRIMARY KEY CHECK (id ~ '^[a-z0-9][a-z0-9-]{0,62}$'),
    name text NOT NULL CHECK (name ~ '^[a-z0-9][a-z0-9-]{0,63}$'),
    title text NOT NULL CHECK (char_length(btrim(title)) BETWEEN 1 AND 120),
    description text NOT NULL DEFAULT '' CHECK (char_length(description) <= 1024),
    tags text[] NOT NULL DEFAULT '{}' CHECK (cardinality(tags) <= 16),
    active_version integer CHECK (active_version >= 1),
    latest_version integer NOT NULL DEFAULT 1 CHECK (latest_version >= 1),
    source_kind text NOT NULL CHECK (source_kind IN ('built', 'upload', 'github')),
    source_repo text CHECK (source_repo IS NULL OR source_repo ~ '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$'),
    source_ref text CHECK (char_length(source_ref) <= 200),
    source_path text CHECK (char_length(source_path) <= 500),
    source_commit text CHECK (source_commit IS NULL OR source_commit ~ '^[0-9a-f]{40}$'),
    created timestamptz NOT NULL,
    updated timestamptz NOT NULL,
    archived_at timestamptz,
    CHECK ((source_kind = 'github') = (source_repo IS NOT NULL))
);
-- Agents call a skill by its name: one live skill per name.
CREATE UNIQUE INDEX skills_name_live ON skills (name) WHERE archived_at IS NULL;

CREATE TABLE skill_bindings (
    skill_id text NOT NULL REFERENCES skills (id),
    binding text NOT NULL CHECK (binding = 'plaza' OR binding ~ '^role:[a-z0-9-]{1,64}$'),
    PRIMARY KEY (skill_id, binding)
);
CREATE INDEX skill_bindings_binding ON skill_bindings (binding);

CREATE TABLE skill_versions (
    skill_id text NOT NULL REFERENCES skills (id),
    version integer NOT NULL CHECK (version >= 1),
    skill_md text NOT NULL CHECK (char_length(skill_md) <= 200000),
    sha256 bytea NOT NULL CHECK (octet_length(sha256) = 32),
    note text NOT NULL DEFAULT '' CHECK (char_length(note) <= 500),
    author text NOT NULL CHECK (char_length(author) BETWEEN 1 AND 80),
    score smallint NOT NULL CHECK (score BETWEEN 0 AND 100),
    errors integer NOT NULL CHECK (errors >= 0),
    warnings integer NOT NULL CHECK (warnings >= 0),
    infos integer NOT NULL CHECK (infos >= 0),
    rules_version integer NOT NULL CHECK (rules_version >= 1),
    file_count integer NOT NULL CHECK (file_count BETWEEN 0 AND 200),
    total_bytes bigint NOT NULL CHECK (total_bytes BETWEEN 0 AND 20971520),
    source_commit text CHECK (source_commit IS NULL OR source_commit ~ '^[0-9a-f]{40}$'),
    created timestamptz NOT NULL,
    PRIMARY KEY (skill_id, version)
);
CREATE TRIGGER skill_versions_no_update_delete BEFORE UPDATE OR DELETE ON skill_versions
    FOR EACH ROW EXECUTE FUNCTION forbid_mutation();
CREATE TRIGGER skill_versions_no_truncate BEFORE TRUNCATE ON skill_versions
    FOR EACH STATEMENT EXECUTE FUNCTION forbid_mutation();

ALTER TABLE skills ADD CONSTRAINT skills_active_version_fk
    FOREIGN KEY (id, active_version) REFERENCES skill_versions (skill_id, version)
    DEFERRABLE INITIALLY DEFERRED;

CREATE TABLE skill_files (
    skill_id text NOT NULL,
    version integer NOT NULL,
    path text NOT NULL CHECK (char_length(path) BETWEEN 1 AND 300 AND path !~ '(^/|\.\.|\\)' AND path <> 'SKILL.md'),
    sha256 bytea NOT NULL CHECK (octet_length(sha256) = 32),
    size bigint NOT NULL CHECK (size BETWEEN 0 AND 5242880),
    executable boolean NOT NULL DEFAULT false,
    PRIMARY KEY (skill_id, version, path),
    FOREIGN KEY (skill_id, version) REFERENCES skill_versions (skill_id, version)
);
CREATE TRIGGER skill_files_no_update_delete BEFORE UPDATE OR DELETE ON skill_files
    FOR EACH ROW EXECUTE FUNCTION forbid_mutation();
CREATE TRIGGER skill_files_no_truncate BEFORE TRUNCATE ON skill_files
    FOR EACH STATEMENT EXECUTE FUNCTION forbid_mutation();

CREATE TABLE skill_findings (
    skill_id text NOT NULL,
    version integer NOT NULL,
    seq integer NOT NULL CHECK (seq >= 0),
    rule text NOT NULL CHECK (rule ~ '^[a-z][a-z0-9-]{1,40}$'),
    severity text NOT NULL CHECK (severity IN ('error', 'warning', 'info')),
    path text NOT NULL CHECK (char_length(path) BETWEEN 1 AND 300),
    line integer NOT NULL CHECK (line >= 0),
    message text NOT NULL CHECK (char_length(message) BETWEEN 1 AND 300),
    PRIMARY KEY (skill_id, version, seq),
    FOREIGN KEY (skill_id, version) REFERENCES skill_versions (skill_id, version)
);
CREATE TRIGGER skill_findings_no_update_delete BEFORE UPDATE OR DELETE ON skill_findings
    FOR EACH ROW EXECUTE FUNCTION forbid_mutation();
CREATE TRIGGER skill_findings_no_truncate BEFORE TRUNCATE ON skill_findings
    FOR EACH STATEMENT EXECUTE FUNCTION forbid_mutation();

CREATE TABLE skill_events (
    id bigserial PRIMARY KEY,
    skill_id text NOT NULL REFERENCES skills (id),
    action text NOT NULL CHECK (action IN ('create', 'version', 'publish', 'unpublish', 'edit', 'bind', 'archive', 'restore')),
    version integer CHECK (version >= 1),
    actor text NOT NULL CHECK (char_length(actor) BETWEEN 1 AND 80),
    at timestamptz NOT NULL
);
CREATE INDEX skill_events_skill ON skill_events (skill_id, id);
CREATE TRIGGER skill_events_no_update_delete BEFORE UPDATE OR DELETE ON skill_events
    FOR EACH ROW EXECUTE FUNCTION forbid_mutation();
CREATE TRIGGER skill_events_no_truncate BEFORE TRUNCATE ON skill_events
    FOR EACH STATEMENT EXECUTE FUNCTION forbid_mutation();
