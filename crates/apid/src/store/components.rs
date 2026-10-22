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
        Component {
            name: "tasks",
            def: apz_plaza::tasks::def,
            rows: |app| app.plaza.tasks.rows(),
            load: |app, conn| app.plaza.tasks.load(conn),
        },
        Component {
            name: "projects",
            def: crate::projects::def,
            rows: |app| app.projects.rows(),
            load: |app, conn| app.projects.load(conn),
        },
        Component {
            name: "project_settings",
            def: crate::project_settings::def,
            rows: |app| app.project_settings.rows(),
            load: |app, conn| app.project_settings.load(conn),
        },
        Component {
            name: "maintenance",
            def: crate::maintenance::def,
            rows: |app| app.maintenance.rows(),
            load: |app, conn| app.maintenance.load(conn),
        },
        Component {
            name: "environments",
            def: crate::environments::def,
            rows: |app| app.environments.rows(),
            load: |app, conn| app.environments.load(conn),
        },
        // batch C2
        // batch C3
        Component {
            name: "connectors",
            def: crate::connectors::def,
            rows: |app| app.connectors.rows(),
            load: |app, conn| app.connectors.load(conn),
        },
        Component {
            name: "settings",
            def: crate::settings::def,
            rows: |app| app.settings.rows(),
            load: |app, conn| app.settings.load(conn),
        },
        Component {
            name: "appearance",
            def: crate::appearance::def,
            rows: |app| app.appearance.rows(),
            load: |app, conn| app.appearance.load(conn),
        },
        Component {
            name: "providers",
            def: crate::providers::def,
            rows: |app| app.providers.rows(),
            load: |app, conn| app.providers.load(conn),
        },
        // batch C4
        Component {
            name: "spawned",
            def: crate::spawned::def,
            rows: |app| app.spawned.rows(),
            load: |app, conn| app.spawned.load(conn),
        },
        Component {
            name: "rules",
            def: crate::laws::def,
            rows: |app| app.laws.rows(),
            load: |app, conn| app.laws.load(conn),
        },
        Component {
            name: "work",
            def: crate::scheduler::def,
            rows: |app| app.work.rows(),
            load: |app, conn| app.work.load(conn, app.store.db(crate::store::PLAZA)),
        },
        Component {
            name: "control",
            def: crate::control::def,
            rows: |app| app.control.rows(),
            load: |app, conn| app.control.load(conn),
        },
        Component {
            name: "custom_roles",
            def: crate::roles_custom::def,
            rows: |app| app.custom_roles.rows(),
            load: |app, conn| app.custom_roles.load(conn),
        },
        Component {
            name: "agent_proposals",
            def: crate::agent_proposals::def,
            rows: |app| app.agent_proposals.rows(),
            load: |app, conn| app.agent_proposals.load(conn),
        },
        Component {
            name: "nodes",
            def: crate::nodes::def,
            rows: |app| app.nodes.rows(),
            load: |app, conn| app.nodes.load(conn),
        },
        Component {
            name: "stations",
            def: crate::stations::store_def,
            rows: |app| app.stations.rows(),
            load: |app, conn| app.stations.load(conn),
        },
        Component {
            name: "social",
            def: apz_plaza::social::def,
            rows: |app| app.plaza.social.rows(),
            load: |app, conn| app.plaza.social.load(conn),
        },
        // batch C5
        Component {
            name: "prices",
            def: crate::prices::def,
            rows: |app| app.prices.rows(),
            load: |app, conn| app.prices.load(conn),
        },
        Component {
            name: "usage",
            def: crate::usage::def,
            rows: |app| app.usage.rows(),
            load: |app, conn| app.usage.load(conn),
        },
        Component {
            name: "foundry",
            def: crate::foundry::store::def,
            rows: |app| app.foundry.rows(),
            load: |app, conn| app.foundry.load(conn),
        },
        Component {
            name: "models",
            def: crate::foundry::registry::def,
            rows: |app| app.models.rows(),
            load: |app, conn| app.models.load(conn),
        },
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
            rows: |app| {
                app.providers
                    .listings
                    .rows(&app.providers.gateway().providers())
            },
            load: |app, conn| app.providers.listings.load(conn),
        },
    ]
}

/// Tables written through as things happen rather than by the saver (append-only ledgers and logs); they are in
/// `0001_init.sql` beside the components' and the same checks hold for them.
pub fn written_through() -> Vec<(&'static str, fn() -> &'static TableDef)> {
    vec![
        ("budget ledger", crate::budgets::entries_def),
        ("vault", crate::vault::def),
        ("vault audit", crate::vault::audit::def),
        ("usage calls", crate::usage::call_def),
        ("usage hours", crate::usage::hour_def),
        ("usage works", crate::usage::work_def),
    ]
}
