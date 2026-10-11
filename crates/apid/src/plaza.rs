//! The plaza's domain state (apz-plaza) and its REST routes: tasks, treasury,
//! agora, board, conversations, council.
//!
//! OWNER: slice "plaza-api". The handlers port noded's former ones — see
//! `git show 82d6286:crates/noded/src/lib.rs` (fn list_tasks … fn withdraw_task).

use std::sync::Arc;

use apz_http::Router;
use apz_plaza::agora::Agora;
use apz_plaza::council::Council;
use apz_plaza::events::EventBus;
use apz_plaza::social::Social;
use apz_plaza::tasks::TaskLedger;
use apz_plaza::treasury::Treasury;

use crate::state::App;

pub struct Plaza {
    pub events: Arc<EventBus>,
    pub tasks: TaskLedger,
    pub treasury: Arc<Treasury>,
    pub agora: Agora,
    pub social: Social,
    pub council: Council,
}

impl Plaza {
    pub fn new() -> Self {
        let events = EventBus::new();
        let tasks = TaskLedger::new(Arc::clone(&events));
        let treasury = Arc::clone(tasks.treasury());
        Self {
            agora: Agora::new(Arc::clone(&events)),
            social: Social::new(Arc::clone(&events)),
            council: Council::new(Arc::clone(&events)),
            tasks,
            treasury,
            events,
        }
    }
}

/// `/v1/tasks…`, `/v1/treasury…`, `/v1/agora`, `/v1/board…`,
/// `/v1/conversations…`, `/v1/motions…`.
pub fn routes(router: Router<App>) -> Router<App> {
    // TODO(plaza-api slice)
    router
}
