-- 0009 unfurl_cache: the hub's unfurl answers (crates/apid/src/unfurl, docs/design/workspace.md section 3.3). One row
-- per reference: key is the reference's JSON, data the answer the route returns ({ok, card} or {ok, error}), and
-- fetched_ms when it was fetched. The TTL is read from the answer (open 10 min, merged or closed 1 h, failures 1 min).
-- Upserted: a refetch replaces the row.
CREATE TABLE unfurl_cache (
    key text PRIMARY KEY,
    data jsonb NOT NULL,
    ok boolean NOT NULL,
    fetched_ms bigint NOT NULL
);
