-- The object store's index (apz-blobs): where every blob lives in the packs. The packs are the truth; this index is
-- a checkpoint of them, so opening scans only what was appended since (`pack.indexed_len`).

CREATE TABLE blob (
    hash BLOB PRIMARY KEY NOT NULL CHECK (length(hash) = 32),
    pack INTEGER NOT NULL CHECK (pack > 0),
    "offset" INTEGER NOT NULL CHECK ("offset" >= 0),
    stored INTEGER NOT NULL CHECK (stored >= 0),
    flags INTEGER NOT NULL CHECK (flags >= 0),
    raw INTEGER NOT NULL CHECK (raw >= 0),
    -- When it was last written or put again: a sweep spares blobs touched within its grace window.
    touched_ms INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE INDEX blob_pack ON blob (pack);

CREATE TABLE pack (
    no INTEGER PRIMARY KEY NOT NULL CHECK (no > 0),
    -- The bytes at the start of the pack whose entries are all in `blob`.
    indexed_len INTEGER NOT NULL CHECK (indexed_len >= 0)
) STRICT;
