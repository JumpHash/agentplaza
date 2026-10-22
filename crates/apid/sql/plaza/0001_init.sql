-- plaza.db, schema 1 (the clean break, W3): the hub's state of record. Every table is STRICT with typed columns and no
-- JSON (W69). The components' tables below are generated from their definitions (derive(apz_store::Table)): print
-- them with `cargo run -p apid -- schema`; the store's tests check that this file matches.

-- component tasks
CREATE TABLE "task_ledger" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "next_seq" INTEGER NOT NULL CHECK ("next_seq" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "ledger_task" (
    "task_ledger_pos" INTEGER NOT NULL CHECK ("task_ledger_pos" >= 0),
    "tasks_pos" INTEGER NOT NULL CHECK ("tasks_pos" >= 0),
    "id" TEXT NOT NULL,
    "goal" TEXT NOT NULL,
    "acceptance" TEXT,
    "budget" INTEGER NOT NULL CHECK ("budget" >= 0),
    "role" TEXT,
    "escrow_entry" INTEGER CHECK ("escrow_entry" >= 0),
    "created_at" INTEGER NOT NULL CHECK ("created_at" >= 0),
    "state" TEXT NOT NULL CHECK ("state" IN ('open', 'claimed', 'awaiting_review', 'rework', 'done', 'failed', 'withdrawn')),
    "assignee_id" TEXT,
    "verifier_id" TEXT,
    "agent_cid" INTEGER CHECK ("agent_cid" >= 0),
    "note" TEXT,
    "updated_at" INTEGER NOT NULL CHECK ("updated_at" >= 0),
    "project" TEXT,
    PRIMARY KEY ("task_ledger_pos", "tasks_pos"),
    FOREIGN KEY ("task_ledger_pos") REFERENCES "task_ledger" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "ledger_task_after" (
    "task_ledger_pos" INTEGER NOT NULL CHECK ("task_ledger_pos" >= 0),
    "tasks_pos" INTEGER NOT NULL CHECK ("tasks_pos" >= 0),
    "after_pos" INTEGER NOT NULL CHECK ("after_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("task_ledger_pos", "tasks_pos", "after_pos"),
    FOREIGN KEY ("task_ledger_pos", "tasks_pos") REFERENCES "ledger_task" ("task_ledger_pos", "tasks_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component projects
CREATE TABLE "project" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "title" TEXT NOT NULL,
    "goal" TEXT NOT NULL,
    "kind" TEXT NOT NULL,
    "stage" TEXT NOT NULL,
    "lead_cid" INTEGER NOT NULL CHECK ("lead_cid" >= 0),
    "owner_approval" INTEGER NOT NULL CHECK ("owner_approval" IN (0, 1)),
    "repo" TEXT,
    "revision" INTEGER NOT NULL CHECK ("revision" >= 0),
    "approved_by" TEXT,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "updated_ms" INTEGER NOT NULL CHECK ("updated_ms" >= 0),
    "paused" INTEGER NOT NULL CHECK ("paused" IN (0, 1)),
    "archived" INTEGER NOT NULL CHECK ("archived" IN (0, 1)),
    "mode" TEXT NOT NULL,
    PRIMARY KEY ("id")
) STRICT;
CREATE TABLE "project_docs" (
    "project_id" TEXT NOT NULL,
    "docs_pos" INTEGER NOT NULL CHECK ("docs_pos" >= 0),
    "kind" TEXT NOT NULL,
    "title" TEXT NOT NULL,
    "file" TEXT,
    "repo_path" TEXT,
    "commit" TEXT,
    "summary" TEXT NOT NULL,
    "by_cid" INTEGER NOT NULL CHECK ("by_cid" >= 0),
    "by" TEXT NOT NULL,
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    PRIMARY KEY ("project_id", "docs_pos"),
    FOREIGN KEY ("project_id") REFERENCES "project" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "project_critiques" (
    "project_id" TEXT NOT NULL,
    "critiques_pos" INTEGER NOT NULL CHECK ("critiques_pos" >= 0),
    "by_cid" INTEGER NOT NULL CHECK ("by_cid" >= 0),
    "by" TEXT NOT NULL,
    "verdict" TEXT NOT NULL,
    "text" TEXT NOT NULL,
    "revision" INTEGER NOT NULL CHECK ("revision" >= 0),
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    PRIMARY KEY ("project_id", "critiques_pos"),
    FOREIGN KEY ("project_id") REFERENCES "project" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "project_tasks" (
    "project_id" TEXT NOT NULL,
    "tasks_pos" INTEGER NOT NULL CHECK ("tasks_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("project_id", "tasks_pos"),
    FOREIGN KEY ("project_id") REFERENCES "project" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "project_history" (
    "project_id" TEXT NOT NULL,
    "history_pos" INTEGER NOT NULL CHECK ("history_pos" >= 0),
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    "by_cid" INTEGER NOT NULL CHECK ("by_cid" >= 0),
    "by" TEXT NOT NULL,
    "what" TEXT NOT NULL,
    "note" TEXT NOT NULL,
    PRIMARY KEY ("project_id", "history_pos"),
    FOREIGN KEY ("project_id") REFERENCES "project" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component project_settings
CREATE TABLE "project_setting" (
    "project" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "default_model" TEXT,
    "effort" TEXT,
    "max_tokens" INTEGER CHECK ("max_tokens" >= 0),
    "max_turns" INTEGER CHECK ("max_turns" >= 0),
    "compress_at" INTEGER CHECK ("compress_at" >= 0),
    "keep_recent" INTEGER CHECK ("keep_recent" >= 0),
    "task_budget" INTEGER CHECK ("task_budget" >= 0),
    "permission_mode" TEXT CHECK ("permission_mode" IN ('autonomous', 'ask', 'read_only')),
    "review_timeout_minutes" INTEGER CHECK ("review_timeout_minutes" >= 0),
    PRIMARY KEY ("project")
) STRICT;
CREATE TABLE "project_setting_role_models" (
    "project_setting_project" TEXT NOT NULL,
    "role_models_key" TEXT NOT NULL,
    "value" TEXT NOT NULL,
    PRIMARY KEY ("project_setting_project", "role_models_key"),
    FOREIGN KEY ("project_setting_project") REFERENCES "project_setting" ("project") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component maintenance
CREATE TABLE "maintenance_book" (
    "project" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "sweep_runs" INTEGER NOT NULL CHECK ("sweep_runs" >= 0),
    "sweep_last_ms" INTEGER NOT NULL CHECK ("sweep_last_ms" >= 0),
    "sweep_next_ms" INTEGER NOT NULL CHECK ("sweep_next_ms" >= 0),
    "sweep_running_present" INTEGER NOT NULL CHECK ("sweep_running_present" IN (0, 1)),
    "sweep_running_work" INTEGER CHECK ("sweep_running_work" >= 0),
    "sweep_running_cid" INTEGER CHECK ("sweep_running_cid" >= 0),
    "sweep_running_started_ms" INTEGER CHECK ("sweep_running_started_ms" >= 0),
    "sweep_running_why" TEXT,
    "sweep_last_summary" TEXT NOT NULL,
    "sweep_last_cid" INTEGER NOT NULL CHECK ("sweep_last_cid" >= 0),
    "sweep_seen_seq" INTEGER NOT NULL CHECK ("sweep_seen_seq" >= 0),
    "seq" INTEGER NOT NULL CHECK ("seq" >= 0),
    "ingest" TEXT,
    PRIMARY KEY ("project")
) STRICT;
CREATE TABLE "maintenance_book_config" (
    "maintenance_book_project" TEXT NOT NULL,
    "every_hours" INTEGER NOT NULL CHECK ("every_hours" >= 0),
    "focus" TEXT NOT NULL,
    PRIMARY KEY ("maintenance_book_project"),
    FOREIGN KEY ("maintenance_book_project") REFERENCES "maintenance_book" ("project") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "maintenance_book_config_watchers" (
    "maintenance_book_project" TEXT NOT NULL,
    "watchers_pos" INTEGER NOT NULL CHECK ("watchers_pos" >= 0),
    "value" INTEGER NOT NULL CHECK ("value" >= 0),
    PRIMARY KEY ("maintenance_book_project", "watchers_pos"),
    FOREIGN KEY ("maintenance_book_project") REFERENCES "maintenance_book_config" ("maintenance_book_project") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "maintenance_book_map" (
    "maintenance_book_project" TEXT NOT NULL,
    "summary" TEXT NOT NULL,
    "updated_ms" INTEGER NOT NULL CHECK ("updated_ms" >= 0),
    "by" TEXT NOT NULL,
    PRIMARY KEY ("maintenance_book_project"),
    FOREIGN KEY ("maintenance_book_project") REFERENCES "maintenance_book" ("project") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "maintenance_book_map_areas" (
    "maintenance_book_project" TEXT NOT NULL,
    "areas_pos" INTEGER NOT NULL CHECK ("areas_pos" >= 0),
    "id" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "summary" TEXT NOT NULL,
    "health" TEXT NOT NULL,
    "notes" TEXT NOT NULL,
    "updated_ms" INTEGER NOT NULL CHECK ("updated_ms" >= 0),
    "by" TEXT NOT NULL,
    PRIMARY KEY ("maintenance_book_project", "areas_pos"),
    FOREIGN KEY ("maintenance_book_project") REFERENCES "maintenance_book_map" ("maintenance_book_project") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "maintenance_book_map_areas_paths" (
    "maintenance_book_project" TEXT NOT NULL,
    "areas_pos" INTEGER NOT NULL CHECK ("areas_pos" >= 0),
    "paths_pos" INTEGER NOT NULL CHECK ("paths_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("maintenance_book_project", "areas_pos", "paths_pos"),
    FOREIGN KEY ("maintenance_book_project", "areas_pos") REFERENCES "maintenance_book_map_areas" ("maintenance_book_project", "areas_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "maintenance_book_map_areas_links" (
    "maintenance_book_project" TEXT NOT NULL,
    "areas_pos" INTEGER NOT NULL CHECK ("areas_pos" >= 0),
    "links_pos" INTEGER NOT NULL CHECK ("links_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("maintenance_book_project", "areas_pos", "links_pos"),
    FOREIGN KEY ("maintenance_book_project", "areas_pos") REFERENCES "maintenance_book_map_areas" ("maintenance_book_project", "areas_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "maintenance_book_signals" (
    "maintenance_book_project" TEXT NOT NULL,
    "signals_pos" INTEGER NOT NULL CHECK ("signals_pos" >= 0),
    "id" TEXT NOT NULL,
    "seq" INTEGER NOT NULL CHECK ("seq" >= 0),
    "kind" TEXT NOT NULL,
    "title" TEXT NOT NULL,
    "text" TEXT NOT NULL,
    "from" TEXT NOT NULL,
    "url" TEXT,
    "environment" TEXT,
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    PRIMARY KEY ("maintenance_book_project", "signals_pos"),
    FOREIGN KEY ("maintenance_book_project") REFERENCES "maintenance_book" ("project") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "maintenance_book_proposals" (
    "maintenance_book_project" TEXT NOT NULL,
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    "id" TEXT NOT NULL,
    "kind" TEXT NOT NULL,
    "title" TEXT NOT NULL,
    "summary" TEXT NOT NULL,
    "area" TEXT,
    "worth_score" INTEGER NOT NULL CHECK ("worth_score" >= 0),
    "worth_why" TEXT NOT NULL,
    "effort_size" TEXT NOT NULL,
    "effort_budget" INTEGER CHECK ("effort_budget" >= 0),
    "effort_note" TEXT NOT NULL,
    "instrument_present" INTEGER NOT NULL CHECK ("instrument_present" IN (0, 1)),
    "instrument_kind" TEXT,
    "instrument_name" TEXT,
    "instrument_catalog" TEXT,
    "instrument_data" TEXT,
    "role" TEXT NOT NULL,
    "acceptance" TEXT NOT NULL,
    "state" TEXT NOT NULL,
    "by_cid" INTEGER NOT NULL CHECK ("by_cid" >= 0),
    "by" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "decided_ms" INTEGER NOT NULL CHECK ("decided_ms" >= 0),
    "note" TEXT NOT NULL,
    "task" TEXT,
    PRIMARY KEY ("maintenance_book_project", "proposals_pos"),
    FOREIGN KEY ("maintenance_book_project") REFERENCES "maintenance_book" ("project") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "maintenance_book_proposals_impact" (
    "maintenance_book_project" TEXT NOT NULL,
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    "changes" TEXT NOT NULL,
    "users" TEXT NOT NULL,
    "risk" TEXT NOT NULL,
    "risk_note" TEXT NOT NULL,
    PRIMARY KEY ("maintenance_book_project", "proposals_pos"),
    FOREIGN KEY ("maintenance_book_project", "proposals_pos") REFERENCES "maintenance_book_proposals" ("maintenance_book_project", "proposals_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "maintenance_book_proposals_impact_areas" (
    "maintenance_book_project" TEXT NOT NULL,
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    "areas_pos" INTEGER NOT NULL CHECK ("areas_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("maintenance_book_project", "proposals_pos", "areas_pos"),
    FOREIGN KEY ("maintenance_book_project", "proposals_pos") REFERENCES "maintenance_book_proposals_impact" ("maintenance_book_project", "proposals_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "maintenance_book_proposals_evidence" (
    "maintenance_book_project" TEXT NOT NULL,
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    "evidence_pos" INTEGER NOT NULL CHECK ("evidence_pos" >= 0),
    "signal" TEXT,
    "note" TEXT NOT NULL,
    PRIMARY KEY ("maintenance_book_project", "proposals_pos", "evidence_pos"),
    FOREIGN KEY ("maintenance_book_project", "proposals_pos") REFERENCES "maintenance_book_proposals" ("maintenance_book_project", "proposals_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component environments
CREATE TABLE "project_environment" (
    "project" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "seq" INTEGER NOT NULL CHECK ("seq" >= 0),
    PRIMARY KEY ("project")
) STRICT;
CREATE TABLE "environment" (
    "project_environment_project" TEXT NOT NULL,
    "environments_pos" INTEGER NOT NULL CHECK ("environments_pos" >= 0),
    "name" TEXT NOT NULL,
    "kind" TEXT NOT NULL,
    "url" TEXT,
    "track" TEXT NOT NULL,
    "protected" INTEGER NOT NULL CHECK ("protected" IN (0, 1)),
    "notes" TEXT NOT NULL,
    "version" TEXT,
    "deployed_ms" INTEGER NOT NULL CHECK ("deployed_ms" >= 0),
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "updated_ms" INTEGER NOT NULL CHECK ("updated_ms" >= 0),
    PRIMARY KEY ("project_environment_project", "environments_pos"),
    FOREIGN KEY ("project_environment_project") REFERENCES "project_environment" ("project") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "environment_variable" (
    "project_environment_project" TEXT NOT NULL,
    "environments_pos" INTEGER NOT NULL CHECK ("environments_pos" >= 0),
    "variables_key" TEXT NOT NULL,
    "value" TEXT NOT NULL,
    PRIMARY KEY ("project_environment_project", "environments_pos", "variables_key"),
    FOREIGN KEY ("project_environment_project", "environments_pos") REFERENCES "environment" ("project_environment_project", "environments_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "deploy" (
    "project_environment_project" TEXT NOT NULL,
    "environments_pos" INTEGER NOT NULL CHECK ("environments_pos" >= 0),
    "deploys_pos" INTEGER NOT NULL CHECK ("deploys_pos" >= 0),
    "id" TEXT NOT NULL,
    "version" TEXT NOT NULL,
    "by" TEXT NOT NULL,
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    "status" TEXT NOT NULL,
    "detail" TEXT NOT NULL,
    "approval" TEXT,
    PRIMARY KEY ("project_environment_project", "environments_pos", "deploys_pos"),
    FOREIGN KEY ("project_environment_project", "environments_pos") REFERENCES "environment" ("project_environment_project", "environments_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component connectors
CREATE TABLE "connectors" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "connectors_settings" (
    "connectors_pos" INTEGER NOT NULL CHECK ("connectors_pos" >= 0),
    "settings_key" TEXT NOT NULL,
    "enabled" INTEGER NOT NULL CHECK ("enabled" IN (0, 1)),
    PRIMARY KEY ("connectors_pos", "settings_key"),
    FOREIGN KEY ("connectors_pos") REFERENCES "connectors" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "connectors_settings_roles" (
    "connectors_pos" INTEGER NOT NULL CHECK ("connectors_pos" >= 0),
    "settings_key" TEXT NOT NULL,
    "roles_pos" INTEGER NOT NULL CHECK ("roles_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("connectors_pos", "settings_key", "roles_pos"),
    FOREIGN KEY ("connectors_pos", "settings_key") REFERENCES "connectors_settings" ("connectors_pos", "settings_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "connectors_settings_projects" (
    "connectors_pos" INTEGER NOT NULL CHECK ("connectors_pos" >= 0),
    "settings_key" TEXT NOT NULL,
    "projects_pos" INTEGER NOT NULL CHECK ("projects_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("connectors_pos", "settings_key", "projects_pos"),
    FOREIGN KEY ("connectors_pos", "settings_key") REFERENCES "connectors_settings" ("connectors_pos", "settings_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "connector_proposal" (
    "connectors_pos" INTEGER NOT NULL CHECK ("connectors_pos" >= 0),
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    "id" TEXT NOT NULL,
    "proposer_cid" INTEGER NOT NULL CHECK ("proposer_cid" >= 0),
    "proposer" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "state" TEXT NOT NULL,
    "note" TEXT,
    PRIMARY KEY ("connectors_pos", "proposals_pos"),
    FOREIGN KEY ("connectors_pos") REFERENCES "connectors" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "connector_proposal_checks" (
    "connectors_pos" INTEGER NOT NULL CHECK ("connectors_pos" >= 0),
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    "checks_pos" INTEGER NOT NULL CHECK ("checks_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("connectors_pos", "proposals_pos", "checks_pos"),
    FOREIGN KEY ("connectors_pos", "proposals_pos") REFERENCES "connector_proposal" ("connectors_pos", "proposals_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "connector_proposal_spec" (
    "connectors_pos" INTEGER NOT NULL CHECK ("connectors_pos" >= 0),
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    "id" TEXT NOT NULL,
    "label" TEXT NOT NULL,
    "category" TEXT NOT NULL,
    "description" TEXT NOT NULL,
    "docs_url" TEXT NOT NULL,
    "base_url" TEXT NOT NULL,
    "auth_kind" TEXT NOT NULL,
    "auth_name" TEXT,
    PRIMARY KEY ("connectors_pos", "proposals_pos"),
    FOREIGN KEY ("connectors_pos", "proposals_pos") REFERENCES "connector_proposal" ("connectors_pos", "proposals_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "connector_proposal_spec_fields" (
    "connectors_pos" INTEGER NOT NULL CHECK ("connectors_pos" >= 0),
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    "fields_pos" INTEGER NOT NULL CHECK ("fields_pos" >= 0),
    "name" TEXT NOT NULL,
    "label" TEXT NOT NULL,
    "env" TEXT NOT NULL,
    "hint" TEXT NOT NULL,
    "secret" INTEGER NOT NULL CHECK ("secret" IN (0, 1)),
    "required" INTEGER NOT NULL CHECK ("required" IN (0, 1)),
    PRIMARY KEY ("connectors_pos", "proposals_pos", "fields_pos"),
    FOREIGN KEY ("connectors_pos", "proposals_pos") REFERENCES "connector_proposal_spec" ("connectors_pos", "proposals_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "connector_proposal_spec_actions" (
    "connectors_pos" INTEGER NOT NULL CHECK ("connectors_pos" >= 0),
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    "actions_pos" INTEGER NOT NULL CHECK ("actions_pos" >= 0),
    "name" TEXT NOT NULL,
    "description" TEXT NOT NULL,
    "risk" TEXT NOT NULL,
    "method" TEXT NOT NULL,
    "path" TEXT NOT NULL,
    "input_schema_content" TEXT,
    PRIMARY KEY ("connectors_pos", "proposals_pos", "actions_pos"),
    FOREIGN KEY ("connectors_pos", "proposals_pos") REFERENCES "connector_proposal_spec" ("connectors_pos", "proposals_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component settings
CREATE TABLE "settings" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "default_model" TEXT NOT NULL,
    "orchestrator_model" TEXT NOT NULL,
    "effort" TEXT,
    "max_turns" INTEGER NOT NULL CHECK ("max_turns" >= 0),
    "max_tokens" INTEGER NOT NULL CHECK ("max_tokens" >= 0),
    "memory_autoscale" INTEGER NOT NULL CHECK ("memory_autoscale" IN (0, 1)),
    "vm_auto_checkpoints" INTEGER NOT NULL CHECK ("vm_auto_checkpoints" IN (0, 1)),
    "retention_posts_max" INTEGER NOT NULL CHECK ("retention_posts_max" >= 0),
    "retention_posts_days" INTEGER NOT NULL CHECK ("retention_posts_days" >= 0),
    "retention_usage_calls_max" INTEGER NOT NULL CHECK ("retention_usage_calls_max" >= 0),
    "retention_usage_days" INTEGER NOT NULL CHECK ("retention_usage_days" >= 0),
    "laws_strict_permits" INTEGER NOT NULL CHECK ("laws_strict_permits" IN (0, 1)),
    "payments_approve_over" INTEGER NOT NULL CHECK ("payments_approve_over" >= 0),
    "payments_charge_models" INTEGER NOT NULL CHECK ("payments_charge_models" IN (0, 1)),
    "foundry_hf_max_gb" INTEGER NOT NULL CHECK ("foundry_hf_max_gb" >= 0),
    "allow_training_models" INTEGER NOT NULL CHECK ("allow_training_models" IN (0, 1)),
    "review_timeout_minutes" INTEGER NOT NULL CHECK ("review_timeout_minutes" >= 0),
    "workstation_recordings_record" INTEGER NOT NULL CHECK ("workstation_recordings_record" IN (0, 1)),
    "workstation_recordings_retention_days" INTEGER NOT NULL CHECK ("workstation_recordings_retention_days" >= 0),
    "workstation_recordings_record_fps" INTEGER NOT NULL CHECK ("workstation_recordings_record_fps" >= 0),
    "workstation_recordings_record_takeovers" INTEGER NOT NULL CHECK ("workstation_recordings_record_takeovers" IN (0, 1)),
    "engines_mode" TEXT NOT NULL,
    "acp_enabled" INTEGER NOT NULL CHECK ("acp_enabled" IN (0, 1)),
    "acp_write_back" INTEGER NOT NULL CHECK ("acp_write_back" IN (0, 1)),
    "acp_v2" INTEGER NOT NULL CHECK ("acp_v2" IN (0, 1)),
    "idle_sleep_min" INTEGER NOT NULL CHECK ("idle_sleep_min" >= 0),
    "doze_idle_s" INTEGER NOT NULL CHECK ("doze_idle_s" >= 0),
    "health_interval_s" INTEGER NOT NULL CHECK ("health_interval_s" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "settings_role_models" (
    "settings_pos" INTEGER NOT NULL CHECK ("settings_pos" >= 0),
    "role_models_key" TEXT NOT NULL,
    "value" TEXT NOT NULL,
    PRIMARY KEY ("settings_pos", "role_models_key"),
    FOREIGN KEY ("settings_pos") REFERENCES "settings" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "settings_model_policy" (
    "settings_pos" INTEGER NOT NULL CHECK ("settings_pos" >= 0),
    PRIMARY KEY ("settings_pos"),
    FOREIGN KEY ("settings_pos") REFERENCES "settings" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "settings_model_policy_orchestrator" (
    "settings_pos" INTEGER NOT NULL CHECK ("settings_pos" >= 0),
    "orchestrator_pos" INTEGER NOT NULL CHECK ("orchestrator_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("settings_pos", "orchestrator_pos"),
    FOREIGN KEY ("settings_pos") REFERENCES "settings_model_policy" ("settings_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "settings_model_policy_worker" (
    "settings_pos" INTEGER NOT NULL CHECK ("settings_pos" >= 0),
    "worker_pos" INTEGER NOT NULL CHECK ("worker_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("settings_pos", "worker_pos"),
    FOREIGN KEY ("settings_pos") REFERENCES "settings_model_policy" ("settings_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "settings_fetch_block" (
    "settings_pos" INTEGER NOT NULL CHECK ("settings_pos" >= 0),
    "fetch_block_pos" INTEGER NOT NULL CHECK ("fetch_block_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("settings_pos", "fetch_block_pos"),
    FOREIGN KEY ("settings_pos") REFERENCES "settings" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "settings_prices" (
    "settings_pos" INTEGER NOT NULL CHECK ("settings_pos" >= 0),
    "prices_key" TEXT NOT NULL,
    "input" REAL NOT NULL,
    "output" REAL NOT NULL,
    PRIMARY KEY ("settings_pos", "prices_key"),
    FOREIGN KEY ("settings_pos") REFERENCES "settings" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "settings_board_pauses" (
    "settings_pos" INTEGER NOT NULL CHECK ("settings_pos" >= 0),
    "all" INTEGER NOT NULL CHECK ("all" IN (0, 1)),
    PRIMARY KEY ("settings_pos"),
    FOREIGN KEY ("settings_pos") REFERENCES "settings" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "settings_board_pauses_projects" (
    "settings_pos" INTEGER NOT NULL CHECK ("settings_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("settings_pos", "value"),
    FOREIGN KEY ("settings_pos") REFERENCES "settings_board_pauses" ("settings_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "settings_engines_enabled" (
    "settings_pos" INTEGER NOT NULL CHECK ("settings_pos" >= 0),
    "engines_enabled_pos" INTEGER NOT NULL CHECK ("engines_enabled_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("settings_pos", "engines_enabled_pos"),
    FOREIGN KEY ("settings_pos") REFERENCES "settings" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "settings_sleep_never_roles" (
    "settings_pos" INTEGER NOT NULL CHECK ("settings_pos" >= 0),
    "sleep_never_roles_pos" INTEGER NOT NULL CHECK ("sleep_never_roles_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("settings_pos", "sleep_never_roles_pos"),
    FOREIGN KEY ("settings_pos") REFERENCES "settings" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "settings_role_volumes" (
    "settings_pos" INTEGER NOT NULL CHECK ("settings_pos" >= 0),
    "role_volumes_key" TEXT NOT NULL,
    PRIMARY KEY ("settings_pos", "role_volumes_key"),
    FOREIGN KEY ("settings_pos") REFERENCES "settings" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "settings_role_volumes_volumes" (
    "settings_pos" INTEGER NOT NULL CHECK ("settings_pos" >= 0),
    "role_volumes_key" TEXT NOT NULL,
    "volumes_pos" INTEGER NOT NULL CHECK ("volumes_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("settings_pos", "role_volumes_key", "volumes_pos"),
    FOREIGN KEY ("settings_pos", "role_volumes_key") REFERENCES "settings_role_volumes" ("settings_pos", "role_volumes_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "settings_favourite_models" (
    "settings_pos" INTEGER NOT NULL CHECK ("settings_pos" >= 0),
    "favourite_models_pos" INTEGER NOT NULL CHECK ("favourite_models_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("settings_pos", "favourite_models_pos"),
    FOREIGN KEY ("settings_pos") REFERENCES "settings" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component appearance
CREATE TABLE "appearance" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "appearance_choice" (
    "appearance_pos" INTEGER NOT NULL CHECK ("appearance_pos" >= 0),
    "theme" TEXT NOT NULL,
    "skin" TEXT,
    "background_preset" TEXT,
    "mode" TEXT NOT NULL CHECK ("mode" IN ('system', 'light', 'dark')),
    "density" TEXT NOT NULL CHECK ("density" IN ('compact', 'default', 'comfortable')),
    "code_size" REAL NOT NULL,
    "chosen" INTEGER NOT NULL CHECK ("chosen" IN (0, 1)),
    PRIMARY KEY ("appearance_pos"),
    FOREIGN KEY ("appearance_pos") REFERENCES "appearance" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "appearance_choice_background" (
    "appearance_pos" INTEGER NOT NULL CHECK ("appearance_pos" >= 0),
    "id" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "pattern" TEXT NOT NULL CHECK ("pattern" IN ('none', 'dots', 'weave', 'grid', 'blueprint', 'contours')),
    "scale" REAL NOT NULL,
    "strength" REAL NOT NULL,
    "strength_dark" REAL,
    "motion" TEXT NOT NULL CHECK ("motion" IN ('still', 'slow')),
    "pointer" INTEGER NOT NULL CHECK ("pointer" IN (0, 1)),
    "ornament" TEXT NOT NULL CHECK ("ornament" IN ('none', 'threads', 'orbits')),
    "grain" REAL NOT NULL,
    "grain_dark" REAL,
    "scope" TEXT NOT NULL CHECK ("scope" IN ('pages', 'all')),
    PRIMARY KEY ("appearance_pos"),
    FOREIGN KEY ("appearance_pos") REFERENCES "appearance_choice" ("appearance_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "appearance_choice_background_fields" (
    "appearance_pos" INTEGER NOT NULL CHECK ("appearance_pos" >= 0),
    "fields_pos" INTEGER NOT NULL CHECK ("fields_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("appearance_pos", "fields_pos"),
    FOREIGN KEY ("appearance_pos") REFERENCES "appearance_choice_background" ("appearance_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "appearance_theme" (
    "appearance_pos" INTEGER NOT NULL CHECK ("appearance_pos" >= 0),
    "themes_pos" INTEGER NOT NULL CHECK ("themes_pos" >= 0),
    "schema" INTEGER NOT NULL CHECK ("schema" >= 0),
    "id" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "about" TEXT NOT NULL,
    "skin" TEXT NOT NULL,
    "background_preset" TEXT,
    "source" TEXT,
    PRIMARY KEY ("appearance_pos", "themes_pos"),
    FOREIGN KEY ("appearance_pos") REFERENCES "appearance" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "appearance_theme_background" (
    "appearance_pos" INTEGER NOT NULL CHECK ("appearance_pos" >= 0),
    "themes_pos" INTEGER NOT NULL CHECK ("themes_pos" >= 0),
    "id" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "pattern" TEXT NOT NULL CHECK ("pattern" IN ('none', 'dots', 'weave', 'grid', 'blueprint', 'contours')),
    "scale" REAL NOT NULL,
    "strength" REAL NOT NULL,
    "strength_dark" REAL,
    "motion" TEXT NOT NULL CHECK ("motion" IN ('still', 'slow')),
    "pointer" INTEGER NOT NULL CHECK ("pointer" IN (0, 1)),
    "ornament" TEXT NOT NULL CHECK ("ornament" IN ('none', 'threads', 'orbits')),
    "grain" REAL NOT NULL,
    "grain_dark" REAL,
    "scope" TEXT NOT NULL CHECK ("scope" IN ('pages', 'all')),
    PRIMARY KEY ("appearance_pos", "themes_pos"),
    FOREIGN KEY ("appearance_pos", "themes_pos") REFERENCES "appearance_theme" ("appearance_pos", "themes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "appearance_theme_background_fields" (
    "appearance_pos" INTEGER NOT NULL CHECK ("appearance_pos" >= 0),
    "themes_pos" INTEGER NOT NULL CHECK ("themes_pos" >= 0),
    "fields_pos" INTEGER NOT NULL CHECK ("fields_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("appearance_pos", "themes_pos", "fields_pos"),
    FOREIGN KEY ("appearance_pos", "themes_pos") REFERENCES "appearance_theme_background" ("appearance_pos", "themes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "appearance_theme_palette" (
    "appearance_pos" INTEGER NOT NULL CHECK ("appearance_pos" >= 0),
    "themes_pos" INTEGER NOT NULL CHECK ("themes_pos" >= 0),
    PRIMARY KEY ("appearance_pos", "themes_pos"),
    FOREIGN KEY ("appearance_pos", "themes_pos") REFERENCES "appearance_theme" ("appearance_pos", "themes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "appearance_theme_palette_light" (
    "appearance_pos" INTEGER NOT NULL CHECK ("appearance_pos" >= 0),
    "themes_pos" INTEGER NOT NULL CHECK ("themes_pos" >= 0),
    PRIMARY KEY ("appearance_pos", "themes_pos"),
    FOREIGN KEY ("appearance_pos", "themes_pos") REFERENCES "appearance_theme_palette" ("appearance_pos", "themes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "appearance_theme_palette_light_entries" (
    "appearance_pos" INTEGER NOT NULL CHECK ("appearance_pos" >= 0),
    "themes_pos" INTEGER NOT NULL CHECK ("themes_pos" >= 0),
    "entries_key" TEXT NOT NULL,
    "value" TEXT NOT NULL,
    PRIMARY KEY ("appearance_pos", "themes_pos", "entries_key"),
    FOREIGN KEY ("appearance_pos", "themes_pos") REFERENCES "appearance_theme_palette_light" ("appearance_pos", "themes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "appearance_theme_palette_dark" (
    "appearance_pos" INTEGER NOT NULL CHECK ("appearance_pos" >= 0),
    "themes_pos" INTEGER NOT NULL CHECK ("themes_pos" >= 0),
    PRIMARY KEY ("appearance_pos", "themes_pos"),
    FOREIGN KEY ("appearance_pos", "themes_pos") REFERENCES "appearance_theme_palette" ("appearance_pos", "themes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "appearance_theme_palette_dark_entries" (
    "appearance_pos" INTEGER NOT NULL CHECK ("appearance_pos" >= 0),
    "themes_pos" INTEGER NOT NULL CHECK ("themes_pos" >= 0),
    "entries_key" TEXT NOT NULL,
    "value" TEXT NOT NULL,
    PRIMARY KEY ("appearance_pos", "themes_pos", "entries_key"),
    FOREIGN KEY ("appearance_pos", "themes_pos") REFERENCES "appearance_theme_palette_dark" ("appearance_pos", "themes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "appearance_theme_overrides" (
    "appearance_pos" INTEGER NOT NULL CHECK ("appearance_pos" >= 0),
    "themes_pos" INTEGER NOT NULL CHECK ("themes_pos" >= 0),
    PRIMARY KEY ("appearance_pos", "themes_pos"),
    FOREIGN KEY ("appearance_pos", "themes_pos") REFERENCES "appearance_theme" ("appearance_pos", "themes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "appearance_theme_overrides_light" (
    "appearance_pos" INTEGER NOT NULL CHECK ("appearance_pos" >= 0),
    "themes_pos" INTEGER NOT NULL CHECK ("themes_pos" >= 0),
    PRIMARY KEY ("appearance_pos", "themes_pos"),
    FOREIGN KEY ("appearance_pos", "themes_pos") REFERENCES "appearance_theme_overrides" ("appearance_pos", "themes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "appearance_theme_overrides_light_entries" (
    "appearance_pos" INTEGER NOT NULL CHECK ("appearance_pos" >= 0),
    "themes_pos" INTEGER NOT NULL CHECK ("themes_pos" >= 0),
    "entries_key" TEXT NOT NULL,
    "value" TEXT NOT NULL,
    PRIMARY KEY ("appearance_pos", "themes_pos", "entries_key"),
    FOREIGN KEY ("appearance_pos", "themes_pos") REFERENCES "appearance_theme_overrides_light" ("appearance_pos", "themes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "appearance_theme_overrides_dark" (
    "appearance_pos" INTEGER NOT NULL CHECK ("appearance_pos" >= 0),
    "themes_pos" INTEGER NOT NULL CHECK ("themes_pos" >= 0),
    PRIMARY KEY ("appearance_pos", "themes_pos"),
    FOREIGN KEY ("appearance_pos", "themes_pos") REFERENCES "appearance_theme_overrides" ("appearance_pos", "themes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "appearance_theme_overrides_dark_entries" (
    "appearance_pos" INTEGER NOT NULL CHECK ("appearance_pos" >= 0),
    "themes_pos" INTEGER NOT NULL CHECK ("themes_pos" >= 0),
    "entries_key" TEXT NOT NULL,
    "value" TEXT NOT NULL,
    PRIMARY KEY ("appearance_pos", "themes_pos", "entries_key"),
    FOREIGN KEY ("appearance_pos", "themes_pos") REFERENCES "appearance_theme_overrides_dark" ("appearance_pos", "themes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component providers
CREATE TABLE "providers" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "provider" (
    "providers_pos" INTEGER NOT NULL CHECK ("providers_pos" >= 0),
    "providers_pos_2" INTEGER NOT NULL CHECK ("providers_pos_2" >= 0),
    "id" TEXT NOT NULL,
    "kind" TEXT NOT NULL CHECK ("kind" IN ('anthropic', 'openai', 'openai-responses', 'azure-openai', 'gemini', 'bedrock', 'vertex-anthropic', 'vertex-gemini', 'openai-compatible', 'openai-decisions', 'systemone', 'azure-decisions', 'mock')),
    "label" TEXT,
    "accent" TEXT,
    "enabled" INTEGER NOT NULL CHECK ("enabled" IN (0, 1)),
    "base_url" TEXT,
    "auth_kind" TEXT NOT NULL CHECK ("auth_kind" IN ('none', 'env', 'secret', 'command')),
    "auth_env" TEXT,
    "auth_secret" TEXT,
    "auth_command" TEXT,
    "extra_body_content" TEXT,
    PRIMARY KEY ("providers_pos", "providers_pos_2"),
    FOREIGN KEY ("providers_pos") REFERENCES "providers" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "provider_extra_headers" (
    "providers_pos" INTEGER NOT NULL CHECK ("providers_pos" >= 0),
    "providers_pos_2" INTEGER NOT NULL CHECK ("providers_pos_2" >= 0),
    "extra_headers_key" TEXT NOT NULL,
    "value" TEXT NOT NULL,
    PRIMARY KEY ("providers_pos", "providers_pos_2", "extra_headers_key"),
    FOREIGN KEY ("providers_pos", "providers_pos_2") REFERENCES "provider" ("providers_pos", "providers_pos_2") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "provider_models" (
    "providers_pos" INTEGER NOT NULL CHECK ("providers_pos" >= 0),
    "providers_pos_2" INTEGER NOT NULL CHECK ("providers_pos_2" >= 0),
    "models_pos" INTEGER NOT NULL CHECK ("models_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("providers_pos", "providers_pos_2", "models_pos"),
    FOREIGN KEY ("providers_pos", "providers_pos_2") REFERENCES "provider" ("providers_pos", "providers_pos_2") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "provider_options" (
    "providers_pos" INTEGER NOT NULL CHECK ("providers_pos" >= 0),
    "providers_pos_2" INTEGER NOT NULL CHECK ("providers_pos_2" >= 0),
    "region" TEXT,
    "project" TEXT,
    "location" TEXT,
    "path" TEXT,
    "max_tokens_field" TEXT,
    "models_cache_secs" INTEGER CHECK ("models_cache_secs" >= 0),
    "websocket" INTEGER CHECK ("websocket" IN (0, 1)),
    "replay_reasoning" INTEGER CHECK ("replay_reasoning" IN (0, 1)),
    "mistral_compat" INTEGER CHECK ("mistral_compat" IN (0, 1)),
    "reasoning_effort" INTEGER CHECK ("reasoning_effort" IN (0, 1)),
    "first_party" INTEGER CHECK ("first_party" IN (0, 1)),
    "refusal_fallbacks" INTEGER CHECK ("refusal_fallbacks" IN (0, 1)),
    PRIMARY KEY ("providers_pos", "providers_pos_2"),
    FOREIGN KEY ("providers_pos", "providers_pos_2") REFERENCES "provider" ("providers_pos", "providers_pos_2") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "provider_options_trains_on_data" (
    "providers_pos" INTEGER NOT NULL CHECK ("providers_pos" >= 0),
    "providers_pos_2" INTEGER NOT NULL CHECK ("providers_pos_2" >= 0),
    "trains_on_data_pos" INTEGER NOT NULL CHECK ("trains_on_data_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("providers_pos", "providers_pos_2", "trains_on_data_pos"),
    FOREIGN KEY ("providers_pos", "providers_pos_2") REFERENCES "provider_options" ("providers_pos", "providers_pos_2") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "provider_offered" (
    "providers_pos" INTEGER NOT NULL CHECK ("providers_pos" >= 0),
    "offered_pos" INTEGER NOT NULL CHECK ("offered_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("providers_pos", "offered_pos"),
    FOREIGN KEY ("providers_pos") REFERENCES "providers" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component sessions
CREATE TABLE "session" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "project" TEXT NOT NULL,
    "task" TEXT,
    "seat" TEXT,
    "kind" TEXT NOT NULL CHECK ("kind" IN ('step', 'long', 'helper')),
    "parent" TEXT,
    "role" TEXT NOT NULL,
    "engine" TEXT NOT NULL,
    "model" TEXT,
    "state" TEXT NOT NULL CHECK ("state" IN ('queued', 'placing', 'forking', 'running', 'paused', 'compacting', 'handing_over', 'ending', 'ended', 'failed')),
    "paused_by" TEXT CHECK ("paused_by" IN ('budget', 'allowance', 'disk', 'card', 'node')),
    "node" TEXT,
    "cid" INTEGER CHECK ("cid" >= 0),
    "run" TEXT,
    "work" INTEGER CHECK ("work" >= 0),
    "usage_tokens_in" INTEGER NOT NULL CHECK ("usage_tokens_in" >= 0),
    "usage_tokens_out" INTEGER NOT NULL CHECK ("usage_tokens_out" >= 0),
    "usage_tokens_cached" INTEGER NOT NULL CHECK ("usage_tokens_cached" >= 0),
    "usage_cost_micros" INTEGER NOT NULL CHECK ("usage_cost_micros" >= 0),
    "usage_model_calls" INTEGER NOT NULL CHECK ("usage_model_calls" >= 0),
    "usage_tool_calls" INTEGER NOT NULL CHECK ("usage_tool_calls" >= 0),
    "context_bytes" INTEGER NOT NULL CHECK ("context_bytes" >= 0),
    "compactions" INTEGER NOT NULL CHECK ("compactions" >= 0),
    "handover_content" TEXT,
    "outcome" TEXT,
    "attempt" INTEGER NOT NULL CHECK ("attempt" >= 0),
    "verdict" INTEGER CHECK ("verdict" IN (0, 1)),
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "started_ms" INTEGER CHECK ("started_ms" >= 0),
    "ended_ms" INTEGER CHECK ("ended_ms" >= 0),
    PRIMARY KEY ("id")
) STRICT;
CREATE INDEX "session_project" ON "session" ("project");
CREATE INDEX "session_task" ON "session" ("task");
CREATE INDEX "session_cid" ON "session" ("cid");

-- component seats
CREATE TABLE "seat" (
    "task" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "recipe" TEXT NOT NULL,
    "step" TEXT NOT NULL,
    "role" TEXT NOT NULL,
    "model_policy" TEXT NOT NULL CHECK ("model_policy" IN ('any', 'differ')),
    "state" TEXT NOT NULL CHECK ("state" IN ('waiting', 'ready', 'filled', 'done', 'failed')),
    "session" TEXT,
    "last_model" TEXT,
    "attempts" INTEGER NOT NULL CHECK ("attempts" >= 0),
    "round" INTEGER NOT NULL CHECK ("round" >= 0),
    "returned_with_content" TEXT,
    "updated_ms" INTEGER NOT NULL CHECK ("updated_ms" >= 0),
    PRIMARY KEY ("task", "name")
) STRICT;

-- component budgets
CREATE TABLE "budget" (
    "owner_kind" TEXT NOT NULL CHECK ("owner_kind" IN ('project', 'task')),
    "owner" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "project" TEXT NOT NULL,
    "unit" TEXT NOT NULL CHECK ("unit" IN ('usd', 'tokens')),
    "cap" INTEGER CHECK ("cap" >= 0),
    "warn_at_pct" INTEGER NOT NULL CHECK ("warn_at_pct" >= 0),
    "monthly_cap" INTEGER CHECK ("monthly_cap" >= 0),
    "month" INTEGER NOT NULL CHECK ("month" >= 0),
    "month_spent" INTEGER NOT NULL CHECK ("month_spent" >= 0),
    "reserved" INTEGER NOT NULL CHECK ("reserved" >= 0),
    "drawn" INTEGER NOT NULL CHECK ("drawn" >= 0),
    "spent" INTEGER NOT NULL CHECK ("spent" >= 0),
    "held" INTEGER NOT NULL CHECK ("held" >= 0),
    "paused" INTEGER NOT NULL CHECK ("paused" IN (0, 1)),
    "warned" INTEGER NOT NULL CHECK ("warned" IN (0, 1)),
    "updated_ms" INTEGER NOT NULL CHECK ("updated_ms" >= 0),
    PRIMARY KEY ("owner_kind", "owner")
) STRICT;

-- component cards
CREATE TABLE "card" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "kind" TEXT NOT NULL CHECK ("kind" IN ('start', 'budget', 'allowance', 'disk', 'connector', 'tool', 'engine', 'recipe', 'promote', 'memory', 'review', 'land', 'adopt', 'yank', 'seat', 'release', 'deploy', 'tool_call', 'incident')),
    "chat" TEXT,
    "project" TEXT,
    "task" TEXT,
    "session" TEXT,
    "subject" TEXT NOT NULL,
    "title" TEXT NOT NULL,
    "body_content" TEXT,
    "state" TEXT NOT NULL CHECK ("state" IN ('open', 'answered', 'withdrawn')),
    "answer" TEXT,
    "amount" INTEGER CHECK ("amount" >= 0),
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "answered_ms" INTEGER CHECK ("answered_ms" >= 0),
    PRIMARY KEY ("id")
) STRICT;
CREATE INDEX "card_chat" ON "card" ("chat");
CREATE TABLE "card_choices" (
    "card_id" TEXT NOT NULL,
    "choices_pos" INTEGER NOT NULL CHECK ("choices_pos" >= 0),
    "id" TEXT NOT NULL,
    "label" TEXT NOT NULL,
    "detail" TEXT,
    "takes_amount" INTEGER NOT NULL CHECK ("takes_amount" IN (0, 1)),
    PRIMARY KEY ("card_id", "choices_pos"),
    FOREIGN KEY ("card_id") REFERENCES "card" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component chats
CREATE TABLE "chat" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "kind" TEXT NOT NULL CHECK ("kind" IN ('main', 'project', 'task')),
    "project" TEXT,
    "task" TEXT,
    "title" TEXT NOT NULL,
    "archived" INTEGER NOT NULL CHECK ("archived" IN (0, 1)),
    "next_seq" INTEGER NOT NULL CHECK ("next_seq" >= 0),
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "updated_ms" INTEGER NOT NULL CHECK ("updated_ms" >= 0),
    PRIMARY KEY ("id")
) STRICT;
CREATE INDEX "chat_project" ON "chat" ("project");
CREATE INDEX "chat_task" ON "chat" ("task");
CREATE TABLE "chat_messages" (
    "chat_id" TEXT NOT NULL,
    "messages_pos" INTEGER NOT NULL CHECK ("messages_pos" >= 0),
    "seq" INTEGER NOT NULL CHECK ("seq" >= 0),
    "author" TEXT NOT NULL CHECK ("author" IN ('owner', 'session', 'system')),
    "session" TEXT,
    "seat" TEXT,
    "kind" TEXT NOT NULL CHECK ("kind" IN ('message', 'question', 'notice')),
    "text_content" TEXT,
    "card" TEXT,
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    PRIMARY KEY ("chat_id", "messages_pos"),
    FOREIGN KEY ("chat_id") REFERENCES "chat" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component threads
CREATE TABLE "thread" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "task" TEXT NOT NULL,
    "subject" TEXT,
    "state" TEXT NOT NULL CHECK ("state" IN ('open', 'closed')),
    "opened_by" TEXT NOT NULL,
    "next_seq" INTEGER NOT NULL CHECK ("next_seq" >= 0),
    "since_output" INTEGER NOT NULL CHECK ("since_output" >= 0),
    "reported" INTEGER NOT NULL CHECK ("reported" IN (0, 1)),
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "closed_ms" INTEGER CHECK ("closed_ms" >= 0),
    PRIMARY KEY ("id")
) STRICT;
CREATE INDEX "thread_task" ON "thread" ("task");
CREATE TABLE "thread_messages" (
    "thread_id" TEXT NOT NULL,
    "messages_pos" INTEGER NOT NULL CHECK ("messages_pos" >= 0),
    "seq" INTEGER NOT NULL CHECK ("seq" >= 0),
    "from_seat" TEXT NOT NULL,
    "from_session" TEXT,
    "to_seat" TEXT NOT NULL,
    "text_content" TEXT,
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    "delivered_session" TEXT,
    "delivered_ms" INTEGER CHECK ("delivered_ms" >= 0),
    PRIMARY KEY ("thread_id", "messages_pos"),
    FOREIGN KEY ("thread_id") REFERENCES "thread" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component provider_listings
CREATE TABLE "provider_listing" (
    "provider" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "fetched_ms" INTEGER NOT NULL CHECK ("fetched_ms" >= 0),
    PRIMARY KEY ("provider")
) STRICT;
CREATE TABLE "provider_listing_models" (
    "provider_listing_provider" TEXT NOT NULL,
    "models_pos" INTEGER NOT NULL CHECK ("models_pos" >= 0),
    "id" TEXT NOT NULL,
    "name" TEXT,
    "context" INTEGER CHECK ("context" >= 0),
    PRIMARY KEY ("provider_listing_provider", "models_pos"),
    FOREIGN KEY ("provider_listing_provider") REFERENCES "provider_listing" ("provider") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- budget ledger (written through)
CREATE TABLE "budget_entry" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "owner_kind" TEXT NOT NULL CHECK ("owner_kind" IN ('project', 'task')),
    "owner" TEXT NOT NULL,
    "kind" TEXT NOT NULL CHECK ("kind" IN ('set', 'reserve', 'hold', 'charge', 'release', 'raise')),
    "amount" INTEGER NOT NULL CHECK ("amount" >= 0),
    "micros" INTEGER NOT NULL CHECK ("micros" >= 0),
    "tokens" INTEGER NOT NULL CHECK ("tokens" >= 0),
    "session" TEXT,
    "model" TEXT,
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE INDEX "budget_entry_owner" ON "budget_entry" ("owner");

