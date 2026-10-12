//! `floor.ts` lines ~700–945: each frame — robots wander, visit, chat and stroll (with ground
//! height, slopes, falling, collisions), packets fly, dust drifts, props and rooms animate.

use serde_json::Value;

use super::Floor;

impl Floor {
    /// One frame: `dt` seconds since the last, `now_ms` the wall clock.
    pub fn step(&mut self, dt: f32, now_ms: f64) {
        self.time += dt;
        self.now_ms = now_ms; // contract: W2 (the rest of the frame)
    }

    /// `sendPacket(from, to, tone)`: ok = the ok tone, else the accent.
    pub fn send_packet(&mut self, from: u64, to: u64, ok: bool) {
        let _ = (from, to, ok); // contract: W2
    }

    /// One event from the console's stream (`GET /v1/events`: `kind`, JSON `data`): tool calls
    /// set activities and send packets (`recipientOf`, `toolActivity`), work start/end too.
    pub fn on_event(&mut self, kind: &str, data: &Value) {
        let _ = (kind, data); // contract: W2
    }
}
