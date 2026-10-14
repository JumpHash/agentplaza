-- 0012 station_recording_frames: the time each frame of a recording was captured (stations/recordings.rs). One row per
-- frame, written when the frame is stored (its JPEG is the object recordings/<station>/<recording>/<seq>.jpg), so the
-- playback runs on the real times and a takeover's gap shows. The rows go with their recording (ON DELETE CASCADE).
-- A recording from before this migration has no rows: its frames play at even spacing.
CREATE TABLE station_recording_frames (
    recording text NOT NULL REFERENCES station_recordings (id) ON DELETE CASCADE,
    seq bigint NOT NULL CHECK (seq >= 1),
    ts_ms bigint NOT NULL,
    bytes bigint NOT NULL DEFAULT 0 CHECK (bytes >= 0),
    PRIMARY KEY (recording, seq)
);
