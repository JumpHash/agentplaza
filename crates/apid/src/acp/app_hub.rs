//! The plaza as the ACP methods see it: [`super::agent::Hub`] over the hub's [`App`].

use std::sync::mpsc::Receiver;

use serde_json::Value;

use super::agent::Hub;
use crate::events::Frame;
use crate::state::App;
use crate::transcript::Entry;

/// The hub's [`Hub`].
pub struct AppHub<'a> {
    pub app: &'a App,
}

impl Hub for AppHub<'_> {
    fn agent(&self, name: Option<&str>) -> Result<u64, String> {
        let _ = (self.app, name);
        Err("not built yet".to_string())
    }

    fn subscribe(&self) -> Receiver<Frame> {
        self.app.bus.subscribe()
    }

    fn chat(&self, cid: u64, thread: &str, text: &str) -> Result<u64, String> {
        let _ = (cid, thread, text);
        Err("not built yet".to_string())
    }

    fn cancel(&self, cid: u64, work: u64) -> bool {
        let _ = (cid, work);
        false
    }

    fn entries(&self, cid: u64, works: &[u64]) -> Vec<Entry> {
        let _ = (cid, works);
        Vec::new()
    }

    fn pending_approvals(&self, cid: u64, thread: &str) -> Vec<Value> {
        let _ = (cid, thread);
        Vec::new()
    }

    fn decide(&self, id: &str, approve: bool, reason: &str) -> Result<(), String> {
        let _ = (id, approve, reason);
        Err("not built yet".to_string())
    }

    fn mode(&self, cid: u64) -> String {
        let _ = cid;
        "autonomous".to_string()
    }

    fn set_mode(&self, cid: u64, mode: &str) -> Result<(), String> {
        let _ = (cid, mode);
        Err("not built yet".to_string())
    }

    fn now_ms(&self) -> u64 {
        apz_plaza::now_ms()
    }
}
