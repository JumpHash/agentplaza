-- 0015 shares: the share links (crates/apid/src/shares, docs/design/sharing.md). This release the file store
-- (shares.json and share_events.jsonl in the state folder) is the source of truth; these tables are created for
-- the Postgres-backed store that the hub's DB snapshot mirrors into. Nothing writes them yet. A share's id is its
-- 128-bit random link token; password_hash is an argon2 PHC string; expires_ms and revoked_ms are unix milliseconds.
CREATE TABLE shares (
    id text PRIMARY KEY,
    target_kind text NOT NULL,
    target_id text NOT NULL,
    version int,
    password_hash text,
    expires_ms bigint,
    allow_download boolean NOT NULL,
    created_by text NOT NULL,
    created_ms bigint NOT NULL,
    revoked_ms bigint,
    title text NOT NULL
);

CREATE TABLE share_events (
    share text NOT NULL,
    at_ms bigint NOT NULL,
    kind text NOT NULL,
    via text NOT NULL
);
CREATE INDEX share_events_share_at ON share_events (share, at_ms);
