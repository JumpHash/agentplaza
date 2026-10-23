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
        Component {
            name: "toolbox",
            def: crate::toolbox::def,
            rows: |app| app.toolbox.rows(),
            load: |app, conn| app.toolbox.load(conn),
        },
        Component {
            name: "harness",
            def: crate::harness_control::def,
            rows: |app| app.harness.rows(),
            load: |app, conn| app.harness.load(conn),
        },
        Component {
            name: "approvals",
            def: crate::approvals::def,
            rows: |app| app.approvals.rows(),
            load: |app, conn| app.approvals.load(conn),
        },
        Component {
            name: "acp_sessions",
            def: crate::acp::store::def,
            rows: |app| app.acp_sessions.rows(),
            load: |app, conn| app.acp_sessions.load(conn),
        },
        Component {
            name: "library",
            def: crate::library::def,
            rows: |app| app.library.rows(),
            load: |app, conn| app.library.load(conn),
        },
        Component {
            name: "deliverables",
            def: crate::deliverables::def,
            rows: |app| app.deliverables.rows(),
            load: |app, conn| app.deliverables.load(conn),
        },
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
            name: "standing_rules",
            def: crate::standing_rules::def,
            rows: |app| app.standing_rules.rows(),
            load: |app, conn| app.standing_rules.load(conn),
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
            name: "memory_entries",
            def: crate::project_memory::entry_def,
            rows: |app| app.project_memory.entry_rows(),
            load: |app, conn| app.project_memory.load_entries(conn),
        },
        Component {
            name: "memory_proposals",
            def: crate::project_memory::proposal_def,
            rows: |app| app.project_memory.proposal_rows(),
            load: |app, conn| app.project_memory.load_proposals(conn),
        },
        Component {
            name: "memory_curations",
            def: crate::curator::def,
            rows: |app| app.curator.rows(),
            load: |app, conn| app.curator.load(conn),
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
        Component {
            name: "mcp",
            def: crate::mcp::config::def,
            rows: |app| app.mcp.store.rows(),
            load: |app, conn| app.mcp.store.load(conn),
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
            name: "recipe_runs",
            def: crate::recipes::runs::def,
            rows: |app| app.recipe_runs.rows(),
            load: |app, conn| app.recipe_runs.load(conn),
        },
        Component {
            name: "recipe_versions",
            def: crate::recipes::runs::versions_def,
            rows: |app| app.recipe_runs.version_rows(),
            load: |app, conn| app.recipe_runs.load_versions(conn),
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
            name: "journal",
            def: crate::journal::def,
            rows: |app| app.journal.rows(),
            load: |app, conn| app.journal.load(conn),
        },
        Component {
            name: "decisions",
            def: crate::decisions::def,
            rows: |app| app.providers.decisions.rows(),
            load: |app, conn| app.providers.decisions.load(conn),
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
        // The engine 0.11 records (state files of the hub folder, now in plaza.db).
        Component {
            name: "engines",
            def: crate::engine_instances::def,
            rows: |app| app.engines.rows(),
            load: |app, conn| app.engines.load(conn),
        },
        Component {
            name: "review_graph",
            def: crate::review_graph::def,
            rows: |app| app.reviews.rows(),
            load: |app, conn| app.reviews.load(conn),
        },
        Component {
            name: "usage_providers",
            def: crate::usage_providers::def,
            rows: |app| app.usage_providers.rows(),
            load: |app, conn| app.usage_providers.load(conn),
        },
        Component {
            name: "hub_config",
            def: crate::hub_auth::def,
            rows: |app| app.hub_auth.rows(),
            load: |app, conn| app.hub_auth.load(conn),
        },
        Component {
            name: "platforms",
            def: crate::platforms::def,
            rows: |app| app.platforms.rows(),
            load: |app, conn| app.platforms.load(conn),
        },
        Component {
            name: "hf_imports",
            def: crate::foundry::hf_import::def,
            rows: |app| app.hf.rows(),
            load: |app, conn| app.hf.load(conn),
        },
        Component {
            name: "egress_rules",
            def: crate::egress_inject::def,
            rows: |app| app.egress_rules.rows(),
            load: |app, conn| app.egress_rules.load(conn),
        },
        Component {
            name: "e2b_sandboxes",
            def: crate::e2b::def,
            rows: |app| crate::e2b::sandboxes(app).rows(),
            load: |app, conn| crate::e2b::sandboxes(app).load(conn),
        },
        Component {
            name: "e2b_templates",
            def: crate::e2b_tpl::def,
            rows: |app| app.e2b_templates.rows(),
            load: |app, conn| app.e2b_templates.load(conn),
        },
        Component {
            name: "test_maps",
            def: crate::test_select::map_def,
            rows: |app| app.test_select.map_rows(),
            load: |app, conn| app.test_select.load_maps(conn),
        },
        Component {
            name: "test_misses",
            def: crate::test_select::miss_def,
            rows: |app| app.test_select.miss_rows(),
            load: |app, conn| app.test_select.load_misses(conn),
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
        ("ide audit", crate::ide::audit_def),
        ("station audit", crate::stations::audit::def),
        ("station recordings", crate::stations::recordings::def),
        (
            "station recording events",
            crate::stations::recordings::event_def,
        ),
        (
            "station recording frames",
            crate::stations::recordings::frame_def,
        ),
        ("station sync files", crate::stations::sync::def),
        ("eval runs", crate::evals::def),
        ("unfurl cache", crate::unfurl::def),
        ("prompt", crate::prompts::store::def),
        ("prompt version", crate::prompts::store::version_def),
        ("prompt event", crate::prompts::store::event_def),
        ("hub mcp token", crate::hub_mcp::store_db::token_def),
        ("hub mcp token digest", crate::hub_mcp::store_db::digest_def),
        ("hub mcp call", crate::hub_mcp::store_db::call_def),
        ("hub mcp settings", crate::hub_mcp::store_db::settings_def),
        ("share", crate::shares::def),
        ("share event", crate::shares::event_def),
        ("preview", crate::previews::def),
        ("preview link", crate::previews::link_def),
        ("preview opening", crate::previews::opening_def),
        ("skill", crate::skills::store_db::def),
        ("skill version", crate::skills::store_db::version_def),
        ("skill file", crate::skills::store_db::file_def),
        ("skill finding", crate::skills::store_db::finding_def),
        ("skill event", crate::skills::store_db::event_def),
        ("artifact", crate::artifacts::def),
        ("artifact version", crate::artifacts::version_def),
        ("artifact blob", crate::artifacts::blob_def),
    ]
}
