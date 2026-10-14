-- 0011 station_sync_files: the hub's mirror of each workstation's work folder (stations/sync.rs). One row per path the
-- sync knows, per station. `hash` is the hub's copy (NULL once the file is deleted: the row stays, so the deletion
-- reaches the station); `base` is the content both sides last agreed on (NULL when neither had the file); `version`
-- counts the hub's own changes to the path; `conflict_of` names the file a conflict copy was made from. Rewritten in
-- place on each sync pass, not append-only. The content lives in the object store under sync/<sha256>.
CREATE TABLE station_sync_files (
    station text NOT NULL,
    path text NOT NULL,
    hash text,
    size bigint NOT NULL DEFAULT 0 CHECK (size >= 0),
    mtime_ms bigint NOT NULL DEFAULT 0,
    version bigint NOT NULL DEFAULT 0 CHECK (version >= 0),
    base text,
    conflict_of text,
    synced_ms bigint NOT NULL DEFAULT 0,
    PRIMARY KEY (station, path)
);
CREATE INDEX station_sync_files_conflicts ON station_sync_files (station) WHERE conflict_of IS NOT NULL;
