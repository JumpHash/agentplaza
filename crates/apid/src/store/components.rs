//! The components the saver keeps in `plaza.db`: each one's tables, its rows as its state is now, and how it loads
//! at start. Order is the load order (a component whose load reads another's state comes after it).

use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};

use crate::state::App;

/// One component of the hub's state.
pub struct Component {
    /// Its name in logs and in `0001_init.sql`'s comments.
    pub name: &'static str,
    /// Its tables (defined once).
    pub def: fn() -> &'static TableDef,
    /// Its rows as its state is now.
    pub rows: fn(&App) -> Result<RowSet, StoreError>,
    /// Replaces its state with what the store holds.
    pub load: fn(&App, &Connection) -> Result<(), StoreError>,
}

/// Every component, in load order.
pub fn all() -> Vec<Component> {
    vec![
        // batch C1
        // batch C2
        // batch C3
        // batch C4
        // batch C5
        // The work engine's records (phase 0).
        Component {
            name: "sessions",
            def: crate::sessions::def,
            rows: |app| app.sessions.rows(),
            load: |app, conn| app.sessions.load(conn),
        },
        Component {
            name: "seats",
            def: crate::seats::def,
            rows: |app| app.seats.rows(),
            load: |app, conn| app.seats.load(conn),
        },
        Component {
            name: "budgets",
            def: crate::budgets::def,
            rows: |app| app.budgets.rows(),
            load: |app, conn| app.budgets.load(conn),
        },
        Component {
            name: "cards",
            def: crate::cards::def,
            rows: |app| app.cards.rows(),
            load: |app, conn| app.cards.load(conn),
        },
        Component {
            name: "chats",
            def: crate::chats::def,
            rows: |app| app.chats.rows(),
            load: |app, conn| app.chats.load(conn),
        },
        Component {
            name: "threads",
            def: crate::threads::def,
            rows: |app| app.threads.rows(),
            load: |app, conn| app.threads.load(conn),
        },
        Component {
            name: "provider_listings",
            def: crate::provider_listings::def,
            rows: |app| app.providers.listings.rows(&app.providers.gateway().providers()),
            load: |app, conn| app.providers.listings.load(conn),
        },
    ]
}

/// Tables written through as things happen rather than by the saver (append-only ledgers and logs); they are in
/// `0001_init.sql` beside the components' and the same checks hold for them.
pub fn written_through() -> Vec<(&'static str, fn() -> &'static TableDef)> {
    vec![("budget ledger", crate::budgets::entries_def)]
}
