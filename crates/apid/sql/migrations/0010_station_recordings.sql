-- 0010 station_recordings: the index of the workstation recordings (crates/apid/src/stations/recordings.rs). One row per
-- recorded lease, written when it starts and again when it ends (with its counts). Its frames are JPEG objects in the object
-- store, under recordings/<station>/<recording>/<seq>.jpg; its actions are the rows of station_recording_events, which go
-- with the recording. Not append-only: the retention sweep and the owner's delete remove rows.
CREATE TABLE station_recordings (
    id text PRIMARY KEY,
    station text NOT NULL,
    lease text NOT NULL,
    agent bigint NOT NULL,
    task text,
    started_ms bigint NOT NULL,
    ended_ms bigint,
    frames bigint NOT NULL DEFAULT 0 CHECK (frames >= 0),
    actions bigint NOT NULL DEFAULT 0 CHECK (actions >= 0),
    bytes bigint NOT NULL DEFAULT 0 CHECK (bytes >= 0),
    takeover boolean NOT NULL DEFAULT false,
    width bigint,
    height bigint
);
CREATE INDEX station_recordings_station_started ON station_recordings (station, started_ms);

CREATE TABLE station_recording_events (
    seq bigserial PRIMARY KEY,
    recording text NOT NULL REFERENCES station_recordings (id) ON DELETE CASCADE,
    at_ms bigint NOT NULL,
    kind text NOT NULL,
    ok boolean NOT NULL,
    summary jsonb NOT NULL
);
CREATE INDEX station_recording_events_recording ON station_recording_events (recording, seq);
