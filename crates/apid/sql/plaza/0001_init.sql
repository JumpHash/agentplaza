-- plaza.db, schema 1 (the clean break, W3): the hub's state of record. Every table is STRICT with typed columns and no
-- JSON (W69). The components' tables below are generated from their definitions (derive(apz_store::Table)): print
-- them with `cargo run -p apid -- schema`; the store's tests check that this file matches.

-- Full-text indexes (FTS5, store/search.rs): the journal's notes and the project memory's entries, searched by word and
-- ranked by bm25. They are not components: the saver rebuilds each one in the same transaction as the rows it indexes
-- (store/saver.rs), so a search reads what is saved and a restart finds the same index.
CREATE VIRTUAL TABLE "journal_note_search" USING fts5("task" UNINDEXED, "seq" UNINDEXED, "text", "reason");
CREATE VIRTUAL TABLE "memory_entry_search" USING fts5("key" UNINDEXED, "title", "body");

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
    "role" TEXT,
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
    "github" TEXT NOT NULL,
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
    "full_suite_minutes" INTEGER CHECK ("full_suite_minutes" >= 0),
    "team_max" INTEGER CHECK ("team_max" >= 0),
    "team_target" INTEGER CHECK ("team_target" >= 0),
    "queue_batch" INTEGER CHECK ("queue_batch" >= 0),
    "land_approval" INTEGER NOT NULL CHECK ("land_approval" IN (0, 1)),
    "release_continuous" INTEGER NOT NULL CHECK ("release_continuous" IN (0, 1)),
    "hook_defaults" INTEGER NOT NULL CHECK ("hook_defaults" IN (0, 1)),
    PRIMARY KEY ("project")
) STRICT;
CREATE TABLE "project_setting_role_models" (
    "project_setting_project" TEXT NOT NULL,
    "role_models_key" TEXT NOT NULL,
    "value" TEXT NOT NULL,
    PRIMARY KEY ("project_setting_project", "role_models_key"),
    FOREIGN KEY ("project_setting_project") REFERENCES "project_setting" ("project") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "project_setting_hooks" (
    "project_setting_project" TEXT NOT NULL,
    "hooks_pos" INTEGER NOT NULL CHECK ("hooks_pos" >= 0),
    "on" TEXT NOT NULL CHECK ("on" IN ('edit', 'run', 'land')),
    "action" TEXT NOT NULL CHECK ("action" IN ('format', 'lint', 'rule', 'secret_scan')),
    "rule_id" TEXT,
    "mode" TEXT NOT NULL CHECK ("mode" IN ('report', 'block')),
    PRIMARY KEY ("project_setting_project", "hooks_pos"),
    FOREIGN KEY ("project_setting_project") REFERENCES "project_setting" ("project") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "project_setting_hooks_paths" (
    "project_setting_project" TEXT NOT NULL,
    "hooks_pos" INTEGER NOT NULL CHECK ("hooks_pos" >= 0),
    "paths_pos" INTEGER NOT NULL CHECK ("paths_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("project_setting_project", "hooks_pos", "paths_pos"),
    FOREIGN KEY ("project_setting_project", "hooks_pos") REFERENCES "project_setting_hooks" ("project_setting_project", "hooks_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "project_setting_hooks_languages" (
    "project_setting_project" TEXT NOT NULL,
    "hooks_pos" INTEGER NOT NULL CHECK ("hooks_pos" >= 0),
    "languages_pos" INTEGER NOT NULL CHECK ("languages_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("project_setting_project", "hooks_pos", "languages_pos"),
    FOREIGN KEY ("project_setting_project", "hooks_pos") REFERENCES "project_setting_hooks" ("project_setting_project", "hooks_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "project_setting_intake" (
    "project_setting_project" TEXT NOT NULL,
    "intake_pos" INTEGER NOT NULL CHECK ("intake_pos" >= 0),
    "source" TEXT NOT NULL,
    "field" TEXT NOT NULL,
    "value" TEXT NOT NULL,
    "recipe" TEXT NOT NULL,
    "allow_public" INTEGER NOT NULL CHECK ("allow_public" IN (0, 1)),
    "status_property" TEXT NOT NULL,
    "started" TEXT NOT NULL,
    "done" TEXT NOT NULL,
    PRIMARY KEY ("project_setting_project", "intake_pos"),
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

-- component toolbox
CREATE TABLE "discovery" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "name" TEXT NOT NULL,
    "kind" TEXT NOT NULL,
    "what_for" TEXT NOT NULL,
    "how" TEXT NOT NULL,
    "verdict" TEXT NOT NULL,
    "url" TEXT,
    "by_cid" INTEGER NOT NULL CHECK ("by_cid" >= 0),
    "by" TEXT NOT NULL,
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    "flagged" TEXT,
    PRIMARY KEY ("id")
) STRICT;
CREATE TABLE "discovery_tags" (
    "discovery_id" TEXT NOT NULL,
    "tags_pos" INTEGER NOT NULL CHECK ("tags_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("discovery_id", "tags_pos"),
    FOREIGN KEY ("discovery_id") REFERENCES "discovery" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "discovery_endorsed_by" (
    "discovery_id" TEXT NOT NULL,
    "endorsed_by_pos" INTEGER NOT NULL CHECK ("endorsed_by_pos" >= 0),
    "value" INTEGER NOT NULL CHECK ("value" >= 0),
    PRIMARY KEY ("discovery_id", "endorsed_by_pos"),
    FOREIGN KEY ("discovery_id") REFERENCES "discovery" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component harness
CREATE TABLE "harness" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "harness_loops" (
    "harness_pos" INTEGER NOT NULL CHECK ("harness_pos" >= 0),
    "loops_pos" INTEGER NOT NULL CHECK ("loops_pos" >= 0),
    "id" TEXT NOT NULL,
    "cid" INTEGER NOT NULL CHECK ("cid" >= 0),
    "prompt" TEXT NOT NULL,
    "every_secs" INTEGER NOT NULL CHECK ("every_secs" >= 0),
    "max_runs" INTEGER NOT NULL CHECK ("max_runs" >= 0),
    "runs" INTEGER NOT NULL CHECK ("runs" >= 0),
    "next_at_ms" INTEGER NOT NULL CHECK ("next_at_ms" >= 0),
    "state" TEXT NOT NULL,
    "by" TEXT NOT NULL,
    "by_cid" INTEGER NOT NULL CHECK ("by_cid" >= 0),
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "last_work" INTEGER CHECK ("last_work" >= 0),
    "last_summary" TEXT,
    PRIMARY KEY ("harness_pos", "loops_pos"),
    FOREIGN KEY ("harness_pos") REFERENCES "harness" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "harness_limits" (
    "harness_pos" INTEGER NOT NULL CHECK ("harness_pos" >= 0),
    "limits_key" INTEGER NOT NULL CHECK ("limits_key" >= 0),
    "max_turns" INTEGER CHECK ("max_turns" >= 0),
    "compress_at" INTEGER CHECK ("compress_at" >= 0),
    "keep_recent" INTEGER CHECK ("keep_recent" >= 0),
    PRIMARY KEY ("harness_pos", "limits_key"),
    FOREIGN KEY ("harness_pos") REFERENCES "harness" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component approvals
CREATE TABLE "approval_queue" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "seq" INTEGER NOT NULL CHECK ("seq" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "approval_queue_modes" (
    "approval_queue_pos" INTEGER NOT NULL CHECK ("approval_queue_pos" >= 0),
    "modes_key" INTEGER NOT NULL CHECK ("modes_key" >= 0),
    "value" TEXT NOT NULL CHECK ("value" IN ('autonomous', 'ask', 'read_only')),
    PRIMARY KEY ("approval_queue_pos", "modes_key"),
    FOREIGN KEY ("approval_queue_pos") REFERENCES "approval_queue" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "approval_queue_items" (
    "approval_queue_pos" INTEGER NOT NULL CHECK ("approval_queue_pos" >= 0),
    "items_pos" INTEGER NOT NULL CHECK ("items_pos" >= 0),
    "id" TEXT NOT NULL,
    "cid" INTEGER NOT NULL CHECK ("cid" >= 0),
    "tool" TEXT NOT NULL,
    "input_content" TEXT,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "state" TEXT NOT NULL CHECK ("state" IN ('pending', 'approved', 'denied', 'used', 'withdrawn')),
    "reason" TEXT,
    "kind" TEXT NOT NULL,
    "subject" TEXT NOT NULL,
    "detail_content" TEXT,
    "thread" TEXT,
    PRIMARY KEY ("approval_queue_pos", "items_pos"),
    FOREIGN KEY ("approval_queue_pos") REFERENCES "approval_queue" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "approval_queue_items_waiting" (
    "approval_queue_pos" INTEGER NOT NULL CHECK ("approval_queue_pos" >= 0),
    "items_pos" INTEGER NOT NULL CHECK ("items_pos" >= 0),
    "waiting_pos" INTEGER NOT NULL CHECK ("waiting_pos" >= 0),
    "work" INTEGER NOT NULL CHECK ("work" >= 0),
    "call" TEXT NOT NULL,
    PRIMARY KEY ("approval_queue_pos", "items_pos", "waiting_pos"),
    FOREIGN KEY ("approval_queue_pos", "items_pos") REFERENCES "approval_queue_items" ("approval_queue_pos", "items_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component acp_sessions
CREATE TABLE "acp_session" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "cid" INTEGER NOT NULL CHECK ("cid" >= 0),
    "cwd" TEXT NOT NULL,
    "client" TEXT NOT NULL,
    "title" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "updated_ms" INTEGER NOT NULL CHECK ("updated_ms" >= 0),
    "closed" INTEGER NOT NULL CHECK ("closed" IN (0, 1)),
    "opener" TEXT NOT NULL,
    PRIMARY KEY ("id")
) STRICT;
CREATE TABLE "acp_session_works" (
    "acp_session_id" TEXT NOT NULL,
    "works_pos" INTEGER NOT NULL CHECK ("works_pos" >= 0),
    "value" INTEGER NOT NULL CHECK ("value" >= 0),
    PRIMARY KEY ("acp_session_id", "works_pos"),
    FOREIGN KEY ("acp_session_id") REFERENCES "acp_session" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "acp_session_repo" (
    "acp_session_id" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "branch" TEXT NOT NULL,
    "commit" TEXT NOT NULL,
    "dirty" INTEGER NOT NULL CHECK ("dirty" IN (0, 1)),
    PRIMARY KEY ("acp_session_id"),
    FOREIGN KEY ("acp_session_id") REFERENCES "acp_session" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component library
CREATE TABLE "library_entry" (
    "name" TEXT NOT NULL,
    "version" INTEGER NOT NULL CHECK ("version" >= 0),
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "author_cid" INTEGER NOT NULL CHECK ("author_cid" >= 0),
    "author" TEXT NOT NULL,
    "description" TEXT NOT NULL,
    "input_schema_content" TEXT,
    "state" TEXT NOT NULL,
    "hash" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "calls" INTEGER NOT NULL CHECK ("calls" >= 0),
    "review_note" TEXT NOT NULL,
    "project" TEXT NOT NULL,
    "connector" TEXT NOT NULL,
    "replaces" TEXT NOT NULL,
    "stage" TEXT NOT NULL,
    "trial_project" TEXT NOT NULL,
    "promoted" INTEGER NOT NULL CHECK ("promoted" IN (0, 1)),
    "tests" TEXT NOT NULL,
    "failures" INTEGER NOT NULL CHECK ("failures" >= 0),
    "latency_ms" INTEGER NOT NULL CHECK ("latency_ms" >= 0),
    "stage_from" INTEGER NOT NULL CHECK ("stage_from" >= 0),
    PRIMARY KEY ("name", "version")
) STRICT;
CREATE TABLE "library_entry_command" (
    "library_entry_name" TEXT NOT NULL,
    "library_entry_version" INTEGER NOT NULL CHECK ("library_entry_version" >= 0),
    "command_pos" INTEGER NOT NULL CHECK ("command_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("library_entry_name", "library_entry_version", "command_pos"),
    FOREIGN KEY ("library_entry_name", "library_entry_version") REFERENCES "library_entry" ("name", "version") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "library_entry_requires" (
    "library_entry_name" TEXT NOT NULL,
    "library_entry_version" INTEGER NOT NULL CHECK ("library_entry_version" >= 0),
    "requires_pos" INTEGER NOT NULL CHECK ("requires_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("library_entry_name", "library_entry_version", "requires_pos"),
    FOREIGN KEY ("library_entry_name", "library_entry_version") REFERENCES "library_entry" ("name", "version") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "library_entry_files" (
    "library_entry_name" TEXT NOT NULL,
    "library_entry_version" INTEGER NOT NULL CHECK ("library_entry_version" >= 0),
    "files_pos" INTEGER NOT NULL CHECK ("files_pos" >= 0),
    "path" TEXT NOT NULL,
    "size" INTEGER NOT NULL CHECK ("size" >= 0),
    "executable" INTEGER NOT NULL CHECK ("executable" IN (0, 1)),
    PRIMARY KEY ("library_entry_name", "library_entry_version", "files_pos"),
    FOREIGN KEY ("library_entry_name", "library_entry_version") REFERENCES "library_entry" ("name", "version") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "library_entry_comparisons" (
    "library_entry_name" TEXT NOT NULL,
    "library_entry_version" INTEGER NOT NULL CHECK ("library_entry_version" >= 0),
    "comparisons_pos" INTEGER NOT NULL CHECK ("comparisons_pos" >= 0),
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    "builtin_ran" INTEGER NOT NULL CHECK ("builtin_ran" IN (0, 1)),
    "builtin_ok" INTEGER NOT NULL CHECK ("builtin_ok" IN (0, 1)),
    "builtin_ms" INTEGER NOT NULL CHECK ("builtin_ms" >= 0),
    "candidate_ok" INTEGER NOT NULL CHECK ("candidate_ok" IN (0, 1)),
    "candidate_ms" INTEGER NOT NULL CHECK ("candidate_ms" >= 0),
    "same" INTEGER NOT NULL CHECK ("same" IN (0, 1)),
    PRIMARY KEY ("library_entry_name", "library_entry_version", "comparisons_pos"),
    FOREIGN KEY ("library_entry_name", "library_entry_version") REFERENCES "library_entry" ("name", "version") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component deliverables
CREATE TABLE "deliverable" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "name" TEXT NOT NULL,
    "title" TEXT NOT NULL,
    "description" TEXT,
    "content_type" TEXT NOT NULL,
    "size" INTEGER NOT NULL CHECK ("size" >= 0),
    "author_cid" INTEGER NOT NULL CHECK ("author_cid" >= 0),
    "author" TEXT NOT NULL,
    "task" TEXT,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "hash" TEXT NOT NULL,
    "kind" TEXT NOT NULL,
    "audience" TEXT NOT NULL,
    PRIMARY KEY ("id")
) STRICT;
CREATE TABLE "deliverable_allowed" (
    "deliverable_id" TEXT NOT NULL,
    "allowed_pos" INTEGER NOT NULL CHECK ("allowed_pos" >= 0),
    "value" INTEGER NOT NULL CHECK ("value" >= 0),
    PRIMARY KEY ("deliverable_id", "allowed_pos"),
    FOREIGN KEY ("deliverable_id") REFERENCES "deliverable" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
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
CREATE TABLE "connector_manifest" (
    "connectors_pos" INTEGER NOT NULL CHECK ("connectors_pos" >= 0),
    "manifests_pos" INTEGER NOT NULL CHECK ("manifests_pos" >= 0),
    "id" TEXT NOT NULL,
    "version" TEXT NOT NULL,
    "webhook" INTEGER NOT NULL CHECK ("webhook" IN (0, 1)),
    "auth_header" TEXT NOT NULL,
    "auth_format" TEXT NOT NULL,
    "state" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    PRIMARY KEY ("connectors_pos", "manifests_pos"),
    FOREIGN KEY ("connectors_pos") REFERENCES "connectors" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "connector_manifest_kind" (
    "connectors_pos" INTEGER NOT NULL CHECK ("connectors_pos" >= 0),
    "manifests_pos" INTEGER NOT NULL CHECK ("manifests_pos" >= 0),
    "kind_pos" INTEGER NOT NULL CHECK ("kind_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("connectors_pos", "manifests_pos", "kind_pos"),
    FOREIGN KEY ("connectors_pos", "manifests_pos") REFERENCES "connector_manifest" ("connectors_pos", "manifests_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "connector_manifest_scopes" (
    "connectors_pos" INTEGER NOT NULL CHECK ("connectors_pos" >= 0),
    "manifests_pos" INTEGER NOT NULL CHECK ("manifests_pos" >= 0),
    "scopes_pos" INTEGER NOT NULL CHECK ("scopes_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("connectors_pos", "manifests_pos", "scopes_pos"),
    FOREIGN KEY ("connectors_pos", "manifests_pos") REFERENCES "connector_manifest" ("connectors_pos", "manifests_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "connector_manifest_egress" (
    "connectors_pos" INTEGER NOT NULL CHECK ("connectors_pos" >= 0),
    "manifests_pos" INTEGER NOT NULL CHECK ("manifests_pos" >= 0),
    "egress_pos" INTEGER NOT NULL CHECK ("egress_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("connectors_pos", "manifests_pos", "egress_pos"),
    FOREIGN KEY ("connectors_pos", "manifests_pos") REFERENCES "connector_manifest" ("connectors_pos", "manifests_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "connector_manifest_secrets" (
    "connectors_pos" INTEGER NOT NULL CHECK ("connectors_pos" >= 0),
    "manifests_pos" INTEGER NOT NULL CHECK ("manifests_pos" >= 0),
    "secrets_pos" INTEGER NOT NULL CHECK ("secrets_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("connectors_pos", "manifests_pos", "secrets_pos"),
    FOREIGN KEY ("connectors_pos", "manifests_pos") REFERENCES "connector_manifest" ("connectors_pos", "manifests_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "connector_manifest_events" (
    "connectors_pos" INTEGER NOT NULL CHECK ("connectors_pos" >= 0),
    "manifests_pos" INTEGER NOT NULL CHECK ("manifests_pos" >= 0),
    "events_pos" INTEGER NOT NULL CHECK ("events_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("connectors_pos", "manifests_pos", "events_pos"),
    FOREIGN KEY ("connectors_pos", "manifests_pos") REFERENCES "connector_manifest" ("connectors_pos", "manifests_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "connector_manifest_actions" (
    "connectors_pos" INTEGER NOT NULL CHECK ("connectors_pos" >= 0),
    "manifests_pos" INTEGER NOT NULL CHECK ("manifests_pos" >= 0),
    "actions_pos" INTEGER NOT NULL CHECK ("actions_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("connectors_pos", "manifests_pos", "actions_pos"),
    FOREIGN KEY ("connectors_pos", "manifests_pos") REFERENCES "connector_manifest" ("connectors_pos", "manifests_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "connector_manifest_checks" (
    "connectors_pos" INTEGER NOT NULL CHECK ("connectors_pos" >= 0),
    "manifests_pos" INTEGER NOT NULL CHECK ("manifests_pos" >= 0),
    "checks_pos" INTEGER NOT NULL CHECK ("checks_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("connectors_pos", "manifests_pos", "checks_pos"),
    FOREIGN KEY ("connectors_pos", "manifests_pos") REFERENCES "connector_manifest" ("connectors_pos", "manifests_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "connector_kv" (
    "connectors_pos" INTEGER NOT NULL CHECK ("connectors_pos" >= 0),
    "kv_pos" INTEGER NOT NULL CHECK ("kv_pos" >= 0),
    "connector" TEXT NOT NULL,
    "key" TEXT NOT NULL,
    "value" TEXT NOT NULL,
    PRIMARY KEY ("connectors_pos", "kv_pos"),
    FOREIGN KEY ("connectors_pos") REFERENCES "connectors" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
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

-- component spawned
CREATE TABLE "spawned" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "spawned_entry" (
    "spawned_pos" INTEGER NOT NULL CHECK ("spawned_pos" >= 0),
    "spawned_pos_2" INTEGER NOT NULL CHECK ("spawned_pos_2" >= 0),
    "cid" INTEGER NOT NULL CHECK ("cid" >= 0),
    "role" TEXT NOT NULL,
    "model" TEXT,
    PRIMARY KEY ("spawned_pos", "spawned_pos_2"),
    FOREIGN KEY ("spawned_pos") REFERENCES "spawned" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "spawned_model" (
    "spawned_pos" INTEGER NOT NULL CHECK ("spawned_pos" >= 0),
    "models_key" INTEGER NOT NULL CHECK ("models_key" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("spawned_pos", "models_key"),
    FOREIGN KEY ("spawned_pos") REFERENCES "spawned" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component rules
CREATE TABLE "rules" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "owner_rule" (
    "rules_pos" INTEGER NOT NULL CHECK ("rules_pos" >= 0),
    "rules_pos_2" INTEGER NOT NULL CHECK ("rules_pos_2" >= 0),
    "id" TEXT NOT NULL,
    "title" TEXT NOT NULL,
    "source" TEXT NOT NULL,
    "enabled" INTEGER NOT NULL CHECK ("enabled" IN (0, 1)),
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "updated_ms" INTEGER NOT NULL CHECK ("updated_ms" >= 0),
    "author" TEXT NOT NULL,
    PRIMARY KEY ("rules_pos", "rules_pos_2"),
    FOREIGN KEY ("rules_pos") REFERENCES "rules" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "law_override" (
    "rules_pos" INTEGER NOT NULL CHECK ("rules_pos" >= 0),
    "overrides_pos" INTEGER NOT NULL CHECK ("overrides_pos" >= 0),
    "law" TEXT NOT NULL,
    "disabled_by" TEXT NOT NULL,
    "reason" TEXT NOT NULL,
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    PRIMARY KEY ("rules_pos", "overrides_pos"),
    FOREIGN KEY ("rules_pos") REFERENCES "rules" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "law_change" (
    "rules_pos" INTEGER NOT NULL CHECK ("rules_pos" >= 0),
    "history_pos" INTEGER NOT NULL CHECK ("history_pos" >= 0),
    "law" TEXT NOT NULL,
    "title" TEXT NOT NULL,
    "tier" TEXT NOT NULL,
    "disabled" INTEGER NOT NULL CHECK ("disabled" IN (0, 1)),
    "by" TEXT NOT NULL,
    "reason" TEXT NOT NULL,
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    "confirmed" INTEGER NOT NULL CHECK ("confirmed" IN (0, 1)),
    PRIMARY KEY ("rules_pos", "history_pos"),
    FOREIGN KEY ("rules_pos") REFERENCES "rules" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component standing_rules
CREATE TABLE "standing_rules" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "standing_rule" (
    "standing_rules_pos" INTEGER NOT NULL CHECK ("standing_rules_pos" >= 0),
    "versions_pos" INTEGER NOT NULL CHECK ("versions_pos" >= 0),
    "seq" INTEGER NOT NULL CHECK ("seq" >= 0),
    "text" TEXT NOT NULL,
    "author" TEXT NOT NULL,
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    PRIMARY KEY ("standing_rules_pos", "versions_pos"),
    FOREIGN KEY ("standing_rules_pos") REFERENCES "standing_rules" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component work
CREATE TABLE "work" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "next_id" INTEGER NOT NULL CHECK ("next_id" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "work_lane" (
    "work_pos" INTEGER NOT NULL CHECK ("work_pos" >= 0),
    "lanes_key" INTEGER NOT NULL CHECK ("lanes_key" >= 0),
    PRIMARY KEY ("work_pos", "lanes_key"),
    FOREIGN KEY ("work_pos") REFERENCES "work" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "work_lane_queue" (
    "work_pos" INTEGER NOT NULL CHECK ("work_pos" >= 0),
    "lanes_key" INTEGER NOT NULL CHECK ("lanes_key" >= 0),
    "queue_pos" INTEGER NOT NULL CHECK ("queue_pos" >= 0),
    "id" INTEGER NOT NULL CHECK ("id" >= 0),
    "cid" INTEGER NOT NULL CHECK ("cid" >= 0),
    "kind" TEXT NOT NULL CHECK ("kind" IN ('task', 'chat', 'message')),
    "thread" TEXT NOT NULL,
    "prompt" TEXT NOT NULL,
    "origin_task" TEXT,
    "origin_from_cid" INTEGER CHECK ("origin_from_cid" >= 0),
    "origin_owner" INTEGER NOT NULL CHECK ("origin_owner" IN (0, 1)),
    "origin_mcp" INTEGER NOT NULL CHECK ("origin_mcp" IN (0, 1)),
    "origin_acp" INTEGER NOT NULL CHECK ("origin_acp" IN (0, 1)),
    "origin_lane_present" INTEGER NOT NULL CHECK ("origin_lane_present" IN (0, 1)),
    "origin_lane_change" TEXT,
    "origin_lane_repo" TEXT,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "started_ms" INTEGER CHECK ("started_ms" >= 0),
    PRIMARY KEY ("work_pos", "lanes_key", "queue_pos"),
    FOREIGN KEY ("work_pos", "lanes_key") REFERENCES "work_lane" ("work_pos", "lanes_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "work_lane_current" (
    "work_pos" INTEGER NOT NULL CHECK ("work_pos" >= 0),
    "lanes_key" INTEGER NOT NULL CHECK ("lanes_key" >= 0),
    "id" INTEGER NOT NULL CHECK ("id" >= 0),
    "cid" INTEGER NOT NULL CHECK ("cid" >= 0),
    "kind" TEXT NOT NULL CHECK ("kind" IN ('task', 'chat', 'message')),
    "thread" TEXT NOT NULL,
    "prompt" TEXT NOT NULL,
    "origin_task" TEXT,
    "origin_from_cid" INTEGER CHECK ("origin_from_cid" >= 0),
    "origin_owner" INTEGER NOT NULL CHECK ("origin_owner" IN (0, 1)),
    "origin_mcp" INTEGER NOT NULL CHECK ("origin_mcp" IN (0, 1)),
    "origin_acp" INTEGER NOT NULL CHECK ("origin_acp" IN (0, 1)),
    "origin_lane_present" INTEGER NOT NULL CHECK ("origin_lane_present" IN (0, 1)),
    "origin_lane_change" TEXT,
    "origin_lane_repo" TEXT,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "started_ms" INTEGER CHECK ("started_ms" >= 0),
    PRIMARY KEY ("work_pos", "lanes_key"),
    FOREIGN KEY ("work_pos", "lanes_key") REFERENCES "work_lane" ("work_pos", "lanes_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component control
CREATE TABLE "control" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "paused" INTEGER NOT NULL CHECK ("paused" IN (0, 1)),
    "since_ms" INTEGER NOT NULL CHECK ("since_ms" >= 0),
    "by" TEXT NOT NULL,
    "reason" TEXT NOT NULL,
    "cancelled" INTEGER NOT NULL CHECK ("cancelled" >= 0),
    PRIMARY KEY ("pos")
) STRICT;

-- component custom_roles
CREATE TABLE "custom_role" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "title" TEXT NOT NULL,
    "purpose" TEXT NOT NULL,
    "collaboration" TEXT NOT NULL,
    "model" TEXT NOT NULL,
    "prompt" TEXT NOT NULL,
    "created_by" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    PRIMARY KEY ("id")
) STRICT;
CREATE TABLE "custom_role_responsibilities" (
    "custom_role_id" TEXT NOT NULL,
    "responsibilities_pos" INTEGER NOT NULL CHECK ("responsibilities_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("custom_role_id", "responsibilities_pos"),
    FOREIGN KEY ("custom_role_id") REFERENCES "custom_role" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "custom_role_deliverables" (
    "custom_role_id" TEXT NOT NULL,
    "deliverables_pos" INTEGER NOT NULL CHECK ("deliverables_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("custom_role_id", "deliverables_pos"),
    FOREIGN KEY ("custom_role_id") REFERENCES "custom_role" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "custom_role_boundaries" (
    "custom_role_id" TEXT NOT NULL,
    "boundaries_pos" INTEGER NOT NULL CHECK ("boundaries_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("custom_role_id", "boundaries_pos"),
    FOREIGN KEY ("custom_role_id") REFERENCES "custom_role" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "custom_role_tools" (
    "custom_role_id" TEXT NOT NULL,
    "tools_pos" INTEGER NOT NULL CHECK ("tools_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("custom_role_id", "tools_pos"),
    FOREIGN KEY ("custom_role_id") REFERENCES "custom_role" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component agent_proposals
CREATE TABLE "agent_proposal" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "new_role" INTEGER NOT NULL CHECK ("new_role" IN (0, 1)),
    "count" INTEGER NOT NULL CHECK ("count" >= 0),
    "model" TEXT NOT NULL,
    "reason" TEXT NOT NULL,
    "instruction" TEXT NOT NULL,
    "by_cid" INTEGER NOT NULL CHECK ("by_cid" >= 0),
    "by" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "state" TEXT NOT NULL,
    "decided_ms" INTEGER NOT NULL CHECK ("decided_ms" >= 0),
    "note" TEXT NOT NULL,
    "project" TEXT,
    PRIMARY KEY ("id")
) STRICT;
CREATE TABLE "agent_proposal_role" (
    "agent_proposal_id" TEXT NOT NULL,
    "id" TEXT NOT NULL,
    "title" TEXT NOT NULL,
    "purpose" TEXT NOT NULL,
    "collaboration" TEXT NOT NULL,
    "model" TEXT NOT NULL,
    "prompt" TEXT NOT NULL,
    "created_by" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    PRIMARY KEY ("agent_proposal_id"),
    FOREIGN KEY ("agent_proposal_id") REFERENCES "agent_proposal" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "agent_proposal_role_responsibilities" (
    "agent_proposal_id" TEXT NOT NULL,
    "responsibilities_pos" INTEGER NOT NULL CHECK ("responsibilities_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("agent_proposal_id", "responsibilities_pos"),
    FOREIGN KEY ("agent_proposal_id") REFERENCES "agent_proposal_role" ("agent_proposal_id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "agent_proposal_role_deliverables" (
    "agent_proposal_id" TEXT NOT NULL,
    "deliverables_pos" INTEGER NOT NULL CHECK ("deliverables_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("agent_proposal_id", "deliverables_pos"),
    FOREIGN KEY ("agent_proposal_id") REFERENCES "agent_proposal_role" ("agent_proposal_id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "agent_proposal_role_boundaries" (
    "agent_proposal_id" TEXT NOT NULL,
    "boundaries_pos" INTEGER NOT NULL CHECK ("boundaries_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("agent_proposal_id", "boundaries_pos"),
    FOREIGN KEY ("agent_proposal_id") REFERENCES "agent_proposal_role" ("agent_proposal_id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "agent_proposal_role_tools" (
    "agent_proposal_id" TEXT NOT NULL,
    "tools_pos" INTEGER NOT NULL CHECK ("tools_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("agent_proposal_id", "tools_pos"),
    FOREIGN KEY ("agent_proposal_id") REFERENCES "agent_proposal_role" ("agent_proposal_id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "agent_proposal_spawned" (
    "agent_proposal_id" TEXT NOT NULL,
    "spawned_pos" INTEGER NOT NULL CHECK ("spawned_pos" >= 0),
    "value" INTEGER NOT NULL CHECK ("value" >= 0),
    PRIMARY KEY ("agent_proposal_id", "spawned_pos"),
    FOREIGN KEY ("agent_proposal_id") REFERENCES "agent_proposal" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component memory_entries
CREATE TABLE "memory_entry" (
    "key" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "project" TEXT NOT NULL,
    "id" TEXT NOT NULL,
    "kind" TEXT NOT NULL,
    "status" TEXT NOT NULL,
    "title" TEXT NOT NULL,
    "body" TEXT NOT NULL,
    "path" TEXT NOT NULL,
    "verified_commit" TEXT NOT NULL,
    "verified_date" TEXT NOT NULL,
    "provenance_task" TEXT NOT NULL,
    "provenance_session" TEXT NOT NULL,
    "provenance_change" TEXT NOT NULL,
    PRIMARY KEY ("key")
) STRICT;
CREATE TABLE "memory_entry_anchors" (
    "memory_entry_key" TEXT NOT NULL,
    "anchors_pos" INTEGER NOT NULL CHECK ("anchors_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("memory_entry_key", "anchors_pos"),
    FOREIGN KEY ("memory_entry_key") REFERENCES "memory_entry" ("key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "memory_entry_supersedes" (
    "memory_entry_key" TEXT NOT NULL,
    "supersedes_pos" INTEGER NOT NULL CHECK ("supersedes_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("memory_entry_key", "supersedes_pos"),
    FOREIGN KEY ("memory_entry_key") REFERENCES "memory_entry" ("key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "memory_entry_links" (
    "memory_entry_key" TEXT NOT NULL,
    "links_pos" INTEGER NOT NULL CHECK ("links_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("memory_entry_key", "links_pos"),
    FOREIGN KEY ("memory_entry_key") REFERENCES "memory_entry" ("key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component memory_proposals
CREATE TABLE "memory_proposal" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "project" TEXT NOT NULL,
    "kind" TEXT NOT NULL,
    "text" TEXT NOT NULL,
    "why" TEXT NOT NULL,
    "by_cid" INTEGER NOT NULL CHECK ("by_cid" >= 0),
    "by" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    PRIMARY KEY ("id")
) STRICT;
CREATE TABLE "memory_proposal_anchors" (
    "memory_proposal_id" TEXT NOT NULL,
    "anchors_pos" INTEGER NOT NULL CHECK ("anchors_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("memory_proposal_id", "anchors_pos"),
    FOREIGN KEY ("memory_proposal_id") REFERENCES "memory_proposal" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component memory_curations
CREATE TABLE "memory_curation" (
    "project" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "started_ms" INTEGER NOT NULL CHECK ("started_ms" >= 0),
    PRIMARY KEY ("project")
) STRICT;
CREATE TABLE "memory_curation_tasks" (
    "memory_curation_project" TEXT NOT NULL,
    "tasks_pos" INTEGER NOT NULL CHECK ("tasks_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("memory_curation_project", "tasks_pos"),
    FOREIGN KEY ("memory_curation_project") REFERENCES "memory_curation" ("project") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "memory_curation_reading" (
    "memory_curation_project" TEXT NOT NULL,
    "reading_pos" INTEGER NOT NULL CHECK ("reading_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("memory_curation_project", "reading_pos"),
    FOREIGN KEY ("memory_curation_project") REFERENCES "memory_curation" ("project") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component nodes
CREATE TABLE "nodes" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "local_stopped" INTEGER NOT NULL CHECK ("local_stopped" IN (0, 1)),
    "local_test_share" INTEGER NOT NULL CHECK ("local_test_share" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "node" (
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "nodes_pos_2" INTEGER NOT NULL CHECK ("nodes_pos_2" >= 0),
    "id" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "public_key" TEXT NOT NULL,
    "cid_base" INTEGER NOT NULL CHECK ("cid_base" >= 0),
    "cid_count" INTEGER NOT NULL CHECK ("cid_count" >= 0),
    "joined_ms" INTEGER NOT NULL CHECK ("joined_ms" >= 0),
    "last_seen_ms" INTEGER NOT NULL CHECK ("last_seen_ms" >= 0),
    "draining" INTEGER NOT NULL CHECK ("draining" IN (0, 1)),
    "revoked" INTEGER NOT NULL CHECK ("revoked" IN (0, 1)),
    "address" TEXT NOT NULL,
    "label" TEXT NOT NULL,
    "test_share" INTEGER NOT NULL CHECK ("test_share" >= 0),
    PRIMARY KEY ("nodes_pos", "nodes_pos_2"),
    FOREIGN KEY ("nodes_pos") REFERENCES "nodes" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "node_machine" (
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "nodes_pos_2" INTEGER NOT NULL CHECK ("nodes_pos_2" >= 0),
    "cpus" INTEGER CHECK ("cpus" >= 0),
    "memory_mib" INTEGER CHECK ("memory_mib" >= 0),
    "host" TEXT,
    "version" TEXT,
    "name" TEXT,
    "arch" TEXT,
    "os" TEXT,
    "backend" TEXT,
    "cpu_baseline" TEXT,
    "clones" INTEGER CHECK ("clones" IN (0, 1)),
    "storage_free_gib" INTEGER CHECK ("storage_free_gib" >= 0),
    "irqchip" TEXT,
    "class" TEXT,
    "kind" TEXT,
    "vendor" TEXT,
    "model" TEXT,
    "apple_present" INTEGER NOT NULL CHECK ("apple_present" IN (0, 1)),
    "apple_chip" TEXT,
    "apple_p_cores" INTEGER CHECK ("apple_p_cores" >= 0),
    "apple_e_cores" INTEGER CHECK ("apple_e_cores" >= 0),
    "apple_gpu_cores" INTEGER CHECK ("apple_gpu_cores" >= 0),
    "apple_neural_cores" INTEGER CHECK ("apple_neural_cores" >= 0),
    "apple_unified_memory_gib" INTEGER CHECK ("apple_unified_memory_gib" >= 0),
    "cpu_present" INTEGER NOT NULL CHECK ("cpu_present" IN (0, 1)),
    "cpu_model" TEXT,
    "cpu_cores" INTEGER CHECK ("cpu_cores" >= 0),
    "cpu_threads" INTEGER CHECK ("cpu_threads" >= 0),
    "trainer_present" INTEGER NOT NULL CHECK ("trainer_present" IN (0, 1)),
    "trainer_runtime" TEXT,
    "trainer_ok" INTEGER CHECK ("trainer_ok" IN (0, 1)),
    "trainer_detail" TEXT,
    "storage_gib" INTEGER CHECK ("storage_gib" >= 0),
    "vsock_mode" TEXT,
    "started_at_ms" INTEGER CHECK ("started_at_ms" >= 0),
    "stats_present" INTEGER NOT NULL CHECK ("stats_present" IN (0, 1)),
    "stats_cpu_pct" REAL,
    "stats_mem_used_mib" INTEGER CHECK ("stats_mem_used_mib" >= 0),
    "mac_present" INTEGER NOT NULL CHECK ("mac_present" IN (0, 1)),
    "mac_builds" INTEGER CHECK ("mac_builds" >= 0),
    "mac_simulators" INTEGER CHECK ("mac_simulators" >= 0),
    "mac_builds_running" INTEGER CHECK ("mac_builds_running" >= 0),
    "mac_simulators_running" INTEGER CHECK ("mac_simulators_running" >= 0),
    "traffic_shaping" INTEGER CHECK ("traffic_shaping" IN (0, 1)),
    PRIMARY KEY ("nodes_pos", "nodes_pos_2"),
    FOREIGN KEY ("nodes_pos", "nodes_pos_2") REFERENCES "node" ("nodes_pos", "nodes_pos_2") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "node_machine_bases" (
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "nodes_pos_2" INTEGER NOT NULL CHECK ("nodes_pos_2" >= 0),
    "bases_pos" INTEGER NOT NULL CHECK ("bases_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("nodes_pos", "nodes_pos_2", "bases_pos"),
    FOREIGN KEY ("nodes_pos", "nodes_pos_2") REFERENCES "node_machine" ("nodes_pos", "nodes_pos_2") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "node_machine_gpu" (
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "nodes_pos_2" INTEGER NOT NULL CHECK ("nodes_pos_2" >= 0),
    "gpus_pos" INTEGER NOT NULL CHECK ("gpus_pos" >= 0),
    "vendor" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "memory_mib" INTEGER CHECK ("memory_mib" >= 0),
    PRIMARY KEY ("nodes_pos", "nodes_pos_2", "gpus_pos"),
    FOREIGN KEY ("nodes_pos", "nodes_pos_2") REFERENCES "node_machine" ("nodes_pos", "nodes_pos_2") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "node_machine_accelerator" (
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "nodes_pos_2" INTEGER NOT NULL CHECK ("nodes_pos_2" >= 0),
    "accelerators_pos" INTEGER NOT NULL CHECK ("accelerators_pos" >= 0),
    "index" INTEGER NOT NULL CHECK ("index" >= 0),
    "vendor" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "memory_mib" INTEGER CHECK ("memory_mib" >= 0),
    "backend" TEXT NOT NULL,
    "capability" TEXT,
    "link" TEXT NOT NULL,
    "busy" INTEGER NOT NULL CHECK ("busy" IN (0, 1)),
    "util_pct" INTEGER CHECK ("util_pct" >= 0),
    "mem_used_mib" INTEGER CHECK ("mem_used_mib" >= 0),
    PRIMARY KEY ("nodes_pos", "nodes_pos_2", "accelerators_pos"),
    FOREIGN KEY ("nodes_pos", "nodes_pos_2") REFERENCES "node_machine" ("nodes_pos", "nodes_pos_2") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "node_machine_fabric" (
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "nodes_pos_2" INTEGER NOT NULL CHECK ("nodes_pos_2" >= 0),
    "fabric_pos" INTEGER NOT NULL CHECK ("fabric_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("nodes_pos", "nodes_pos_2", "fabric_pos"),
    FOREIGN KEY ("nodes_pos", "nodes_pos_2") REFERENCES "node_machine" ("nodes_pos", "nodes_pos_2") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "node_machine_workstation_backend" (
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "nodes_pos_2" INTEGER NOT NULL CHECK ("nodes_pos_2" >= 0),
    "workstation_backends_pos" INTEGER NOT NULL CHECK ("workstation_backends_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("nodes_pos", "nodes_pos_2", "workstation_backends_pos"),
    FOREIGN KEY ("nodes_pos", "nodes_pos_2") REFERENCES "node_machine" ("nodes_pos", "nodes_pos_2") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "node_machine_browser" (
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "nodes_pos_2" INTEGER NOT NULL CHECK ("nodes_pos_2" >= 0),
    "browsers_pos" INTEGER NOT NULL CHECK ("browsers_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("nodes_pos", "nodes_pos_2", "browsers_pos"),
    FOREIGN KEY ("nodes_pos", "nodes_pos_2") REFERENCES "node_machine" ("nodes_pos", "nodes_pos_2") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "node_machine_phone" (
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "nodes_pos_2" INTEGER NOT NULL CHECK ("nodes_pos_2" >= 0),
    "phones_pos" INTEGER NOT NULL CHECK ("phones_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("nodes_pos", "nodes_pos_2", "phones_pos"),
    FOREIGN KEY ("nodes_pos", "nodes_pos_2") REFERENCES "node_machine" ("nodes_pos", "nodes_pos_2") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "node_code" (
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "codes_pos" INTEGER NOT NULL CHECK ("codes_pos" >= 0),
    "id" TEXT NOT NULL,
    "label" TEXT NOT NULL,
    "secret_sha256" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "expires_ms" INTEGER NOT NULL CHECK ("expires_ms" >= 0),
    "used_by" TEXT,
    "revoked" INTEGER NOT NULL CHECK ("revoked" IN (0, 1)),
    "failures" INTEGER NOT NULL CHECK ("failures" >= 0),
    PRIMARY KEY ("nodes_pos", "codes_pos"),
    FOREIGN KEY ("nodes_pos") REFERENCES "nodes" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "node_override" (
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "overrides_key" INTEGER NOT NULL CHECK ("overrides_key" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("nodes_pos", "overrides_key"),
    FOREIGN KEY ("nodes_pos") REFERENCES "nodes" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component test_lab
CREATE TABLE "test_lab" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "test_lab_run" (
    "test_lab_pos" INTEGER NOT NULL CHECK ("test_lab_pos" >= 0),
    "runs_pos" INTEGER NOT NULL CHECK ("runs_pos" >= 0),
    "id" TEXT NOT NULL,
    "project" TEXT NOT NULL,
    "channel" TEXT NOT NULL,
    "state" TEXT NOT NULL,
    "cpus" INTEGER NOT NULL CHECK ("cpus" >= 0),
    "memory_mib" INTEGER NOT NULL CHECK ("memory_mib" >= 0),
    "enqueued_ms" INTEGER NOT NULL CHECK ("enqueued_ms" >= 0),
    "started_ms" INTEGER CHECK ("started_ms" >= 0),
    "ended_ms" INTEGER CHECK ("ended_ms" >= 0),
    "outcome" TEXT,
    PRIMARY KEY ("test_lab_pos", "runs_pos"),
    FOREIGN KEY ("test_lab_pos") REFERENCES "test_lab" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "test_lab_run_need" (
    "test_lab_pos" INTEGER NOT NULL CHECK ("test_lab_pos" >= 0),
    "runs_pos" INTEGER NOT NULL CHECK ("runs_pos" >= 0),
    "needs_pos" INTEGER NOT NULL CHECK ("needs_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("test_lab_pos", "runs_pos", "needs_pos"),
    FOREIGN KEY ("test_lab_pos", "runs_pos") REFERENCES "test_lab_run" ("test_lab_pos", "runs_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "test_lab_reservation" (
    "test_lab_pos" INTEGER NOT NULL CHECK ("test_lab_pos" >= 0),
    "runs_pos" INTEGER NOT NULL CHECK ("runs_pos" >= 0),
    "reservations_pos" INTEGER NOT NULL CHECK ("reservations_pos" >= 0),
    "node" TEXT NOT NULL,
    "cpus" INTEGER NOT NULL CHECK ("cpus" >= 0),
    "memory_mib" INTEGER NOT NULL CHECK ("memory_mib" >= 0),
    PRIMARY KEY ("test_lab_pos", "runs_pos", "reservations_pos"),
    FOREIGN KEY ("test_lab_pos", "runs_pos") REFERENCES "test_lab_run" ("test_lab_pos", "runs_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component stations
CREATE TABLE "stations" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "station" (
    "stations_pos" INTEGER NOT NULL CHECK ("stations_pos" >= 0),
    "stations_pos_2" INTEGER NOT NULL CHECK ("stations_pos_2" >= 0),
    "id" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "version" TEXT NOT NULL,
    "code" TEXT NOT NULL,
    "token_sha256" TEXT NOT NULL,
    "joined_ms" INTEGER NOT NULL CHECK ("joined_ms" >= 0),
    "last_seen_ms" INTEGER NOT NULL CHECK ("last_seen_ms" >= 0),
    "settings_takeover" TEXT NOT NULL CHECK ("settings_takeover" IN ('agent_waits', 'release_lease', 'keep_working_pointer_paused')),
    "settings_record" INTEGER NOT NULL CHECK ("settings_record" IN (0, 1)),
    "settings_retention_days" INTEGER NOT NULL CHECK ("settings_retention_days" >= 0),
    "settings_record_fps" INTEGER NOT NULL CHECK ("settings_record_fps" >= 0),
    "settings_record_takeovers" INTEGER NOT NULL CHECK ("settings_record_takeovers" IN (0, 1)),
    "settings_work_folder" TEXT NOT NULL,
    "settings_snapshot_reset" INTEGER NOT NULL CHECK ("settings_snapshot_reset" IN (0, 1)),
    "draining" INTEGER NOT NULL CHECK ("draining" IN (0, 1)),
    "revoked" INTEGER NOT NULL CHECK ("revoked" IN (0, 1)),
    "address" TEXT NOT NULL,
    PRIMARY KEY ("stations_pos", "stations_pos_2"),
    FOREIGN KEY ("stations_pos") REFERENCES "stations" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "station_facts" (
    "stations_pos" INTEGER NOT NULL CHECK ("stations_pos" >= 0),
    "stations_pos_2" INTEGER NOT NULL CHECK ("stations_pos_2" >= 0),
    "os" TEXT NOT NULL CHECK ("os" IN ('macos', 'windows', 'linux')),
    "os_version" TEXT NOT NULL,
    "arch" TEXT NOT NULL,
    "hostname" TEXT NOT NULL,
    "form" TEXT NOT NULL CHECK ("form" IN ('laptop', 'desktop', 'mini', 'server', 'vm')),
    "cpus" INTEGER NOT NULL CHECK ("cpus" >= 0),
    "ram_mb" INTEGER NOT NULL CHECK ("ram_mb" >= 0),
    "account_kind" TEXT NOT NULL CHECK ("account_kind" IN ('agent_user', 'owner')),
    "account_agent_user_name" TEXT,
    "account_owner_name" TEXT,
    "apps" INTEGER NOT NULL CHECK ("apps" >= 0),
    "virtual_displays" INTEGER NOT NULL CHECK ("virtual_displays" IN (0, 1)),
    "ui_tree" INTEGER NOT NULL CHECK ("ui_tree" IN (0, 1)),
    "machine_id" TEXT,
    PRIMARY KEY ("stations_pos", "stations_pos_2"),
    FOREIGN KEY ("stations_pos", "stations_pos_2") REFERENCES "station" ("stations_pos", "stations_pos_2") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "station_facts_gpus" (
    "stations_pos" INTEGER NOT NULL CHECK ("stations_pos" >= 0),
    "stations_pos_2" INTEGER NOT NULL CHECK ("stations_pos_2" >= 0),
    "gpus_pos" INTEGER NOT NULL CHECK ("gpus_pos" >= 0),
    "vendor" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "vram_mb" INTEGER NOT NULL CHECK ("vram_mb" >= 0),
    PRIMARY KEY ("stations_pos", "stations_pos_2", "gpus_pos"),
    FOREIGN KEY ("stations_pos", "stations_pos_2") REFERENCES "station_facts" ("stations_pos", "stations_pos_2") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "station_facts_displays" (
    "stations_pos" INTEGER NOT NULL CHECK ("stations_pos" >= 0),
    "stations_pos_2" INTEGER NOT NULL CHECK ("stations_pos_2" >= 0),
    "displays_pos" INTEGER NOT NULL CHECK ("displays_pos" >= 0),
    "id" INTEGER NOT NULL CHECK ("id" >= 0),
    "w" INTEGER NOT NULL CHECK ("w" >= 0),
    "h" INTEGER NOT NULL CHECK ("h" >= 0),
    "scale" REAL NOT NULL,
    PRIMARY KEY ("stations_pos", "stations_pos_2", "displays_pos"),
    FOREIGN KEY ("stations_pos", "stations_pos_2") REFERENCES "station_facts" ("stations_pos", "stations_pos_2") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "station_sync_pause" (
    "stations_pos" INTEGER NOT NULL CHECK ("stations_pos" >= 0),
    "stations_pos_2" INTEGER NOT NULL CHECK ("stations_pos_2" >= 0),
    "reason" TEXT NOT NULL,
    "since_ms" INTEGER NOT NULL CHECK ("since_ms" >= 0),
    PRIMARY KEY ("stations_pos", "stations_pos_2"),
    FOREIGN KEY ("stations_pos", "stations_pos_2") REFERENCES "station" ("stations_pos", "stations_pos_2") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "station_sync_pause_deletion" (
    "stations_pos" INTEGER NOT NULL CHECK ("stations_pos" >= 0),
    "stations_pos_2" INTEGER NOT NULL CHECK ("stations_pos_2" >= 0),
    "deletions_pos" INTEGER NOT NULL CHECK ("deletions_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("stations_pos", "stations_pos_2", "deletions_pos"),
    FOREIGN KEY ("stations_pos", "stations_pos_2") REFERENCES "station_sync_pause" ("stations_pos", "stations_pos_2") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "station_code" (
    "stations_pos" INTEGER NOT NULL CHECK ("stations_pos" >= 0),
    "codes_pos" INTEGER NOT NULL CHECK ("codes_pos" >= 0),
    "id" TEXT NOT NULL,
    "label" TEXT NOT NULL,
    "secret_sha256" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "expires_ms" INTEGER NOT NULL CHECK ("expires_ms" >= 0),
    "used_by" TEXT,
    "revoked" INTEGER NOT NULL CHECK ("revoked" IN (0, 1)),
    "failures" INTEGER NOT NULL CHECK ("failures" >= 0),
    PRIMARY KEY ("stations_pos", "codes_pos"),
    FOREIGN KEY ("stations_pos") REFERENCES "stations" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "station_vm" (
    "stations_pos" INTEGER NOT NULL CHECK ("stations_pos" >= 0),
    "vms_pos" INTEGER NOT NULL CHECK ("vms_pos" >= 0),
    "id" TEXT NOT NULL,
    "node" TEXT NOT NULL,
    "vm" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "os" TEXT NOT NULL,
    "cpus" INTEGER NOT NULL CHECK ("cpus" >= 0),
    "ram_mb" INTEGER NOT NULL CHECK ("ram_mb" >= 0),
    "disk_gb" INTEGER NOT NULL CHECK ("disk_gb" >= 0),
    "gpu" TEXT NOT NULL,
    "image" TEXT,
    "backend" TEXT NOT NULL,
    "code" TEXT NOT NULL,
    "station" TEXT,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    PRIMARY KEY ("stations_pos", "vms_pos"),
    FOREIGN KEY ("stations_pos") REFERENCES "stations" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component social
CREATE TABLE "social" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "posts_seq" INTEGER NOT NULL CHECK ("posts_seq" >= 0),
    "comment_seq" INTEGER NOT NULL CHECK ("comment_seq" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "social_post" (
    "social_pos" INTEGER NOT NULL CHECK ("social_pos" >= 0),
    "posts_pos" INTEGER NOT NULL CHECK ("posts_pos" >= 0),
    "id" TEXT NOT NULL,
    "agent_cid" INTEGER NOT NULL CHECK ("agent_cid" >= 0),
    "role" TEXT NOT NULL,
    "subject" TEXT NOT NULL,
    "body" TEXT NOT NULL,
    "task" TEXT,
    "posted_by" TEXT NOT NULL,
    "created_at" INTEGER NOT NULL CHECK ("created_at" >= 0),
    "owner" INTEGER NOT NULL CHECK ("owner" IN (0, 1)),
    "project" TEXT,
    PRIMARY KEY ("social_pos", "posts_pos"),
    FOREIGN KEY ("social_pos") REFERENCES "social" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "social_post_comment" (
    "social_pos" INTEGER NOT NULL CHECK ("social_pos" >= 0),
    "posts_pos" INTEGER NOT NULL CHECK ("posts_pos" >= 0),
    "comments_pos" INTEGER NOT NULL CHECK ("comments_pos" >= 0),
    "id" TEXT NOT NULL,
    "post" TEXT NOT NULL,
    "agent_cid" INTEGER CHECK ("agent_cid" >= 0),
    "author" TEXT NOT NULL,
    "body" TEXT NOT NULL,
    "created_at" INTEGER NOT NULL CHECK ("created_at" >= 0),
    PRIMARY KEY ("social_pos", "posts_pos", "comments_pos"),
    FOREIGN KEY ("social_pos", "posts_pos") REFERENCES "social_post" ("social_pos", "posts_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "social_comment_attachment" (
    "social_pos" INTEGER NOT NULL CHECK ("social_pos" >= 0),
    "posts_pos" INTEGER NOT NULL CHECK ("posts_pos" >= 0),
    "comments_pos" INTEGER NOT NULL CHECK ("comments_pos" >= 0),
    "attachments_pos" INTEGER NOT NULL CHECK ("attachments_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("social_pos", "posts_pos", "comments_pos", "attachments_pos"),
    FOREIGN KEY ("social_pos", "posts_pos", "comments_pos") REFERENCES "social_post_comment" ("social_pos", "posts_pos", "comments_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "social_post_attachment" (
    "social_pos" INTEGER NOT NULL CHECK ("social_pos" >= 0),
    "posts_pos" INTEGER NOT NULL CHECK ("posts_pos" >= 0),
    "attachments_pos" INTEGER NOT NULL CHECK ("attachments_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("social_pos", "posts_pos", "attachments_pos"),
    FOREIGN KEY ("social_pos", "posts_pos") REFERENCES "social_post" ("social_pos", "posts_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "social_post_mention" (
    "social_pos" INTEGER NOT NULL CHECK ("social_pos" >= 0),
    "posts_pos" INTEGER NOT NULL CHECK ("posts_pos" >= 0),
    "mentions_pos" INTEGER NOT NULL CHECK ("mentions_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("social_pos", "posts_pos", "mentions_pos"),
    FOREIGN KEY ("social_pos", "posts_pos") REFERENCES "social_post" ("social_pos", "posts_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "social_cursor" (
    "social_pos" INTEGER NOT NULL CHECK ("social_pos" >= 0),
    "cursors_key" INTEGER NOT NULL CHECK ("cursors_key" >= 0),
    "value" INTEGER NOT NULL CHECK ("value" >= 0),
    PRIMARY KEY ("social_pos", "cursors_key"),
    FOREIGN KEY ("social_pos") REFERENCES "social" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component prices
CREATE TABLE "price_catalog" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "fetched_at_ms" INTEGER NOT NULL CHECK ("fetched_at_ms" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "price_catalog_openrouter" (
    "price_catalog_pos" INTEGER NOT NULL CHECK ("price_catalog_pos" >= 0),
    "openrouter_key" TEXT NOT NULL,
    "input" REAL NOT NULL,
    "output" REAL NOT NULL,
    "cache_read" REAL,
    "cache_write" REAL,
    PRIMARY KEY ("price_catalog_pos", "openrouter_key"),
    FOREIGN KEY ("price_catalog_pos") REFERENCES "price_catalog" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "price_catalog_litellm" (
    "price_catalog_pos" INTEGER NOT NULL CHECK ("price_catalog_pos" >= 0),
    "litellm_key" TEXT NOT NULL,
    "input" REAL NOT NULL,
    "output" REAL NOT NULL,
    "cache_read" REAL,
    "cache_write" REAL,
    PRIMARY KEY ("price_catalog_pos", "litellm_key"),
    FOREIGN KEY ("price_catalog_pos") REFERENCES "price_catalog" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "price_catalog_sources" (
    "price_catalog_pos" INTEGER NOT NULL CHECK ("price_catalog_pos" >= 0),
    "sources_pos" INTEGER NOT NULL CHECK ("sources_pos" >= 0),
    "name" TEXT NOT NULL,
    "url" TEXT NOT NULL,
    "models" INTEGER NOT NULL CHECK ("models" >= 0),
    "ok" INTEGER NOT NULL CHECK ("ok" IN (0, 1)),
    "error" TEXT,
    PRIMARY KEY ("price_catalog_pos", "sources_pos"),
    FOREIGN KEY ("price_catalog_pos") REFERENCES "price_catalog" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component usage
CREATE TABLE "usage_state" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "next_id" INTEGER NOT NULL CHECK ("next_id" >= 0),
    PRIMARY KEY ("pos")
) STRICT;

-- component foundry
CREATE TABLE "foundry" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "foundry_recipes" (
    "foundry_pos" INTEGER NOT NULL CHECK ("foundry_pos" >= 0),
    "recipes_key" TEXT NOT NULL,
    "id" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "repo" TEXT NOT NULL,
    "commit" TEXT NOT NULL,
    "schema_content" TEXT,
    "vetted_by" TEXT,
    "vetted_ms" INTEGER CHECK ("vetted_ms" >= 0),
    "local_dev" INTEGER NOT NULL CHECK ("local_dev" IN (0, 1)),
    PRIMARY KEY ("foundry_pos", "recipes_key"),
    FOREIGN KEY ("foundry_pos") REFERENCES "foundry" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "foundry_datasets" (
    "foundry_pos" INTEGER NOT NULL CHECK ("foundry_pos" >= 0),
    "datasets_key" TEXT NOT NULL,
    "id" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "kind" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "from_ms" INTEGER NOT NULL CHECK ("from_ms" >= 0),
    "to_ms" INTEGER NOT NULL CHECK ("to_ms" >= 0),
    "rows" INTEGER NOT NULL CHECK ("rows" >= 0),
    "frozen_rows" INTEGER NOT NULL CHECK ("frozen_rows" >= 0),
    "provenance_content" TEXT,
    "file" TEXT NOT NULL,
    "frozen_file" TEXT NOT NULL,
    "sha256" TEXT NOT NULL,
    PRIMARY KEY ("foundry_pos", "datasets_key"),
    FOREIGN KEY ("foundry_pos") REFERENCES "foundry" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "foundry_datasets_label_balance" (
    "foundry_pos" INTEGER NOT NULL CHECK ("foundry_pos" >= 0),
    "datasets_key" TEXT NOT NULL,
    "label_balance_key" TEXT NOT NULL,
    "value" INTEGER NOT NULL CHECK ("value" >= 0),
    PRIMARY KEY ("foundry_pos", "datasets_key", "label_balance_key"),
    FOREIGN KEY ("foundry_pos", "datasets_key") REFERENCES "foundry_datasets" ("foundry_pos", "datasets_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "foundry_datasets_taint" (
    "foundry_pos" INTEGER NOT NULL CHECK ("foundry_pos" >= 0),
    "datasets_key" TEXT NOT NULL,
    "taint_pos" INTEGER NOT NULL CHECK ("taint_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("foundry_pos", "datasets_key", "taint_pos"),
    FOREIGN KEY ("foundry_pos", "datasets_key") REFERENCES "foundry_datasets" ("foundry_pos", "datasets_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "foundry_jobs" (
    "foundry_pos" INTEGER NOT NULL CHECK ("foundry_pos" >= 0),
    "jobs_key" TEXT NOT NULL,
    "id" TEXT NOT NULL,
    "status" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "updated_ms" INTEGER NOT NULL CHECK ("updated_ms" >= 0),
    "recipe" TEXT NOT NULL,
    "mode" TEXT NOT NULL,
    "budget_gpu_hours" REAL NOT NULL,
    "gpu_hours" REAL NOT NULL,
    "agent" INTEGER CHECK ("agent" >= 0),
    "doc_content" TEXT,
    "plan_content" TEXT,
    "seq" INTEGER NOT NULL CHECK ("seq" >= 0),
    "outputs_remote" INTEGER NOT NULL CHECK ("outputs_remote" IN (0, 1)),
    "metrics_content" TEXT,
    "error" TEXT,
    PRIMARY KEY ("foundry_pos", "jobs_key"),
    FOREIGN KEY ("foundry_pos") REFERENCES "foundry" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "foundry_jobs_datasets" (
    "foundry_pos" INTEGER NOT NULL CHECK ("foundry_pos" >= 0),
    "jobs_key" TEXT NOT NULL,
    "datasets_pos" INTEGER NOT NULL CHECK ("datasets_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("foundry_pos", "jobs_key", "datasets_pos"),
    FOREIGN KEY ("foundry_pos", "jobs_key") REFERENCES "foundry_jobs" ("foundry_pos", "jobs_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "foundry_jobs_runs" (
    "foundry_pos" INTEGER NOT NULL CHECK ("foundry_pos" >= 0),
    "jobs_key" TEXT NOT NULL,
    "runs_pos" INTEGER NOT NULL CHECK ("runs_pos" >= 0),
    "node_job" TEXT NOT NULL,
    "node" TEXT NOT NULL,
    "island" TEXT NOT NULL,
    "started_ms" INTEGER NOT NULL CHECK ("started_ms" >= 0),
    "ended_ms" INTEGER CHECK ("ended_ms" >= 0),
    "state" TEXT NOT NULL,
    "exit_code" INTEGER,
    "next" INTEGER NOT NULL CHECK ("next" >= 0),
    "gpu_hours" REAL NOT NULL,
    PRIMARY KEY ("foundry_pos", "jobs_key", "runs_pos"),
    FOREIGN KEY ("foundry_pos", "jobs_key") REFERENCES "foundry_jobs" ("foundry_pos", "jobs_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "foundry_jobs_runs_devices" (
    "foundry_pos" INTEGER NOT NULL CHECK ("foundry_pos" >= 0),
    "jobs_key" TEXT NOT NULL,
    "runs_pos" INTEGER NOT NULL CHECK ("runs_pos" >= 0),
    "devices_pos" INTEGER NOT NULL CHECK ("devices_pos" >= 0),
    "value" INTEGER NOT NULL CHECK ("value" >= 0),
    PRIMARY KEY ("foundry_pos", "jobs_key", "runs_pos", "devices_pos"),
    FOREIGN KEY ("foundry_pos", "jobs_key", "runs_pos") REFERENCES "foundry_jobs_runs" ("foundry_pos", "jobs_key", "runs_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "foundry_jobs_series" (
    "foundry_pos" INTEGER NOT NULL CHECK ("foundry_pos" >= 0),
    "jobs_key" TEXT NOT NULL,
    "series_pos" INTEGER NOT NULL CHECK ("series_pos" >= 0),
    "kind" TEXT NOT NULL CHECK ("kind" IN ('step', 'eval', 'round', 'trial')),
    "step_island" TEXT,
    "step_step" INTEGER CHECK ("step_step" >= 0),
    "step_loss" REAL,
    "step_lr" REAL,
    "step_samples" INTEGER CHECK ("step_samples" >= 0),
    "step_t" INTEGER CHECK ("step_t" >= 0),
    "eval_island" TEXT,
    "eval_step" INTEGER CHECK ("eval_step" >= 0),
    "eval_split" TEXT,
    "eval_metrics_content" TEXT,
    "eval_t" INTEGER CHECK ("eval_t" >= 0),
    "round_island" TEXT,
    "round_round" INTEGER CHECK ("round_round" >= 0),
    "round_accepted" INTEGER CHECK ("round_accepted" IN (0, 1)),
    "round_hub_round" INTEGER CHECK ("round_hub_round" >= 0),
    "round_step" INTEGER CHECK ("round_step" >= 0),
    "round_t" INTEGER CHECK ("round_t" >= 0),
    "trial_island" TEXT,
    "trial_trial" INTEGER CHECK ("trial_trial" >= 0),
    "trial_objective" REAL,
    "trial_t" INTEGER CHECK ("trial_t" >= 0),
    PRIMARY KEY ("foundry_pos", "jobs_key", "series_pos"),
    FOREIGN KEY ("foundry_pos", "jobs_key") REFERENCES "foundry_jobs" ("foundry_pos", "jobs_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "foundry_jobs_outputs" (
    "foundry_pos" INTEGER NOT NULL CHECK ("foundry_pos" >= 0),
    "jobs_key" TEXT NOT NULL,
    "outputs_pos" INTEGER NOT NULL CHECK ("outputs_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("foundry_pos", "jobs_key", "outputs_pos"),
    FOREIGN KEY ("foundry_pos", "jobs_key") REFERENCES "foundry_jobs" ("foundry_pos", "jobs_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "foundry_jobs_candidates" (
    "foundry_pos" INTEGER NOT NULL CHECK ("foundry_pos" >= 0),
    "jobs_key" TEXT NOT NULL,
    "candidates_pos" INTEGER NOT NULL CHECK ("candidates_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("foundry_pos", "jobs_key", "candidates_pos"),
    FOREIGN KEY ("foundry_pos", "jobs_key") REFERENCES "foundry_jobs" ("foundry_pos", "jobs_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component models
CREATE TABLE "model_registry" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "model_registry_records" (
    "model_registry_pos" INTEGER NOT NULL CHECK ("model_registry_pos" >= 0),
    "records_key" TEXT NOT NULL,
    "state" TEXT NOT NULL CHECK ("state" IN ('trained', 'shadow', 'promoted', 'retired', 'imported')),
    "source_job" TEXT,
    "agent_cid" INTEGER CHECK ("agent_cid" >= 0),
    "provenance_content" TEXT,
    "calibration_content" TEXT,
    "size_bytes" INTEGER NOT NULL CHECK ("size_bytes" >= 0),
    "parameters" INTEGER NOT NULL CHECK ("parameters" >= 0),
    "shadow_runs" INTEGER NOT NULL CHECK ("shadow_runs" >= 0),
    "shadow_agreed" INTEGER NOT NULL CHECK ("shadow_agreed" >= 0),
    "shadow_cost_saved_usd" REAL NOT NULL,
    "shadow_updated_ms" INTEGER NOT NULL CHECK ("shadow_updated_ms" >= 0),
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "state_changed_ms" INTEGER NOT NULL CHECK ("state_changed_ms" >= 0),
    "source_present" INTEGER NOT NULL CHECK ("source_present" IN (0, 1)),
    "source_hf" TEXT,
    "source_revision" TEXT,
    "source_as_" TEXT,
    "source_path" TEXT,
    "source_servable_reason" TEXT,
    "source_task" TEXT,
    PRIMARY KEY ("model_registry_pos", "records_key"),
    FOREIGN KEY ("model_registry_pos") REFERENCES "model_registry" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "model_registry_records_card" (
    "model_registry_pos" INTEGER NOT NULL CHECK ("model_registry_pos" >= 0),
    "records_key" TEXT NOT NULL,
    "id" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "version" TEXT NOT NULL,
    "onnx_sha256" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "metrics_content" TEXT,
    "serving_content" TEXT,
    PRIMARY KEY ("model_registry_pos", "records_key"),
    FOREIGN KEY ("model_registry_pos", "records_key") REFERENCES "model_registry_records" ("model_registry_pos", "records_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "model_registry_records_card_input" (
    "model_registry_pos" INTEGER NOT NULL CHECK ("model_registry_pos" >= 0),
    "records_key" TEXT NOT NULL,
    "kind" TEXT NOT NULL,
    PRIMARY KEY ("model_registry_pos", "records_key"),
    FOREIGN KEY ("model_registry_pos", "records_key") REFERENCES "model_registry_records_card" ("model_registry_pos", "records_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "model_registry_records_card_input_properties" (
    "model_registry_pos" INTEGER NOT NULL CHECK ("model_registry_pos" >= 0),
    "records_key" TEXT NOT NULL,
    "properties_key" TEXT NOT NULL,
    "kind" TEXT NOT NULL CHECK ("kind" IN ('number', 'integer', 'string', 'boolean', 'array', 'object')),
    PRIMARY KEY ("model_registry_pos", "records_key", "properties_key"),
    FOREIGN KEY ("model_registry_pos", "records_key") REFERENCES "model_registry_records_card_input" ("model_registry_pos", "records_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "model_registry_records_card_input_properties_allowed" (
    "model_registry_pos" INTEGER NOT NULL CHECK ("model_registry_pos" >= 0),
    "records_key" TEXT NOT NULL,
    "properties_key" TEXT NOT NULL,
    PRIMARY KEY ("model_registry_pos", "records_key", "properties_key"),
    FOREIGN KEY ("model_registry_pos", "records_key", "properties_key") REFERENCES "model_registry_records_card_input_properties" ("model_registry_pos", "records_key", "properties_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "model_registry_records_card_input_properties_allowed_values" (
    "model_registry_pos" INTEGER NOT NULL CHECK ("model_registry_pos" >= 0),
    "records_key" TEXT NOT NULL,
    "properties_key" TEXT NOT NULL,
    "values_pos" INTEGER NOT NULL CHECK ("values_pos" >= 0),
    "kind" TEXT NOT NULL CHECK ("kind" IN ('bool', 'int', 'real', 'text')),
    "bool" INTEGER CHECK ("bool" IN (0, 1)),
    "int" INTEGER,
    "real" REAL,
    "text" TEXT,
    PRIMARY KEY ("model_registry_pos", "records_key", "properties_key", "values_pos"),
    FOREIGN KEY ("model_registry_pos", "records_key", "properties_key") REFERENCES "model_registry_records_card_input_properties_allowed" ("model_registry_pos", "records_key", "properties_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "model_registry_records_card_input_required" (
    "model_registry_pos" INTEGER NOT NULL CHECK ("model_registry_pos" >= 0),
    "records_key" TEXT NOT NULL,
    "required_pos" INTEGER NOT NULL CHECK ("required_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("model_registry_pos", "records_key", "required_pos"),
    FOREIGN KEY ("model_registry_pos", "records_key") REFERENCES "model_registry_records_card_input" ("model_registry_pos", "records_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "model_registry_records_card_output" (
    "model_registry_pos" INTEGER NOT NULL CHECK ("model_registry_pos" >= 0),
    "records_key" TEXT NOT NULL,
    "kind" TEXT NOT NULL,
    PRIMARY KEY ("model_registry_pos", "records_key"),
    FOREIGN KEY ("model_registry_pos", "records_key") REFERENCES "model_registry_records_card" ("model_registry_pos", "records_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "model_registry_records_card_output_properties" (
    "model_registry_pos" INTEGER NOT NULL CHECK ("model_registry_pos" >= 0),
    "records_key" TEXT NOT NULL,
    "properties_key" TEXT NOT NULL,
    "kind" TEXT NOT NULL CHECK ("kind" IN ('number', 'integer', 'string', 'boolean', 'array', 'object')),
    PRIMARY KEY ("model_registry_pos", "records_key", "properties_key"),
    FOREIGN KEY ("model_registry_pos", "records_key") REFERENCES "model_registry_records_card_output" ("model_registry_pos", "records_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "model_registry_records_card_output_properties_allowed" (
    "model_registry_pos" INTEGER NOT NULL CHECK ("model_registry_pos" >= 0),
    "records_key" TEXT NOT NULL,
    "properties_key" TEXT NOT NULL,
    PRIMARY KEY ("model_registry_pos", "records_key", "properties_key"),
    FOREIGN KEY ("model_registry_pos", "records_key", "properties_key") REFERENCES "model_registry_records_card_output_properties" ("model_registry_pos", "records_key", "properties_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "model_registry_records_card_output_properties_allowed_values" (
    "model_registry_pos" INTEGER NOT NULL CHECK ("model_registry_pos" >= 0),
    "records_key" TEXT NOT NULL,
    "properties_key" TEXT NOT NULL,
    "values_pos" INTEGER NOT NULL CHECK ("values_pos" >= 0),
    "kind" TEXT NOT NULL CHECK ("kind" IN ('bool', 'int', 'real', 'text')),
    "bool" INTEGER CHECK ("bool" IN (0, 1)),
    "int" INTEGER,
    "real" REAL,
    "text" TEXT,
    PRIMARY KEY ("model_registry_pos", "records_key", "properties_key", "values_pos"),
    FOREIGN KEY ("model_registry_pos", "records_key", "properties_key") REFERENCES "model_registry_records_card_output_properties_allowed" ("model_registry_pos", "records_key", "properties_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "model_registry_records_card_output_required" (
    "model_registry_pos" INTEGER NOT NULL CHECK ("model_registry_pos" >= 0),
    "records_key" TEXT NOT NULL,
    "required_pos" INTEGER NOT NULL CHECK ("required_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("model_registry_pos", "records_key", "required_pos"),
    FOREIGN KEY ("model_registry_pos", "records_key") REFERENCES "model_registry_records_card_output" ("model_registry_pos", "records_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "model_registry_records_card_features" (
    "model_registry_pos" INTEGER NOT NULL CHECK ("model_registry_pos" >= 0),
    "records_key" TEXT NOT NULL,
    "features_pos" INTEGER NOT NULL CHECK ("features_pos" >= 0),
    "name" TEXT NOT NULL,
    "kind" TEXT NOT NULL CHECK ("kind" IN ('numeric', 'onehot', 'hash')),
    "buckets" INTEGER CHECK ("buckets" >= 0),
    PRIMARY KEY ("model_registry_pos", "records_key", "features_pos"),
    FOREIGN KEY ("model_registry_pos", "records_key") REFERENCES "model_registry_records_card" ("model_registry_pos", "records_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "model_registry_records_card_features_values" (
    "model_registry_pos" INTEGER NOT NULL CHECK ("model_registry_pos" >= 0),
    "records_key" TEXT NOT NULL,
    "features_pos" INTEGER NOT NULL CHECK ("features_pos" >= 0),
    "values_pos" INTEGER NOT NULL CHECK ("values_pos" >= 0),
    "kind" TEXT NOT NULL CHECK ("kind" IN ('bool', 'int', 'real', 'text')),
    "bool" INTEGER CHECK ("bool" IN (0, 1)),
    "int" INTEGER,
    "real" REAL,
    "text" TEXT,
    PRIMARY KEY ("model_registry_pos", "records_key", "features_pos", "values_pos"),
    FOREIGN KEY ("model_registry_pos", "records_key", "features_pos") REFERENCES "model_registry_records_card_features" ("model_registry_pos", "records_key", "features_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "model_registry_records_card_features_scale" (
    "model_registry_pos" INTEGER NOT NULL CHECK ("model_registry_pos" >= 0),
    "records_key" TEXT NOT NULL,
    "features_pos" INTEGER NOT NULL CHECK ("features_pos" >= 0),
    "mean" REAL NOT NULL,
    "std" REAL NOT NULL,
    PRIMARY KEY ("model_registry_pos", "records_key", "features_pos"),
    FOREIGN KEY ("model_registry_pos", "records_key", "features_pos") REFERENCES "model_registry_records_card_features" ("model_registry_pos", "records_key", "features_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "model_registry_records_card_labels" (
    "model_registry_pos" INTEGER NOT NULL CHECK ("model_registry_pos" >= 0),
    "records_key" TEXT NOT NULL,
    PRIMARY KEY ("model_registry_pos", "records_key"),
    FOREIGN KEY ("model_registry_pos", "records_key") REFERENCES "model_registry_records_card" ("model_registry_pos", "records_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "model_registry_records_card_labels_names" (
    "model_registry_pos" INTEGER NOT NULL CHECK ("model_registry_pos" >= 0),
    "records_key" TEXT NOT NULL,
    "names_pos" INTEGER NOT NULL CHECK ("names_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("model_registry_pos", "records_key", "names_pos"),
    FOREIGN KEY ("model_registry_pos", "records_key") REFERENCES "model_registry_records_card_labels" ("model_registry_pos", "records_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "model_registry_records_card_allowed_roles" (
    "model_registry_pos" INTEGER NOT NULL CHECK ("model_registry_pos" >= 0),
    "records_key" TEXT NOT NULL,
    "allowed_roles_pos" INTEGER NOT NULL CHECK ("allowed_roles_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("model_registry_pos", "records_key", "allowed_roles_pos"),
    FOREIGN KEY ("model_registry_pos", "records_key") REFERENCES "model_registry_records_card" ("model_registry_pos", "records_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component mcp
CREATE TABLE "mcp_server" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "name" TEXT NOT NULL,
    "enabled" INTEGER NOT NULL CHECK ("enabled" IN (0, 1)),
    "limits_timeout_s" INTEGER NOT NULL CHECK ("limits_timeout_s" >= 0),
    "limits_max_output_kb" INTEGER NOT NULL CHECK ("limits_max_output_kb" >= 0),
    "limits_idle_stop_min" INTEGER NOT NULL CHECK ("limits_idle_stop_min" >= 0),
    "allow_private" INTEGER NOT NULL CHECK ("allow_private" IN (0, 1)),
    "storage_volume" INTEGER NOT NULL CHECK ("storage_volume" IN (0, 1)),
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "updated_ms" INTEGER NOT NULL CHECK ("updated_ms" >= 0),
    PRIMARY KEY ("id")
) STRICT;
CREATE TABLE "mcp_server_transport" (
    "mcp_server_id" TEXT NOT NULL,
    "kind" TEXT NOT NULL CHECK ("kind" IN ('stdio', 'http')),
    "workdir" TEXT,
    "url" TEXT,
    PRIMARY KEY ("mcp_server_id"),
    FOREIGN KEY ("mcp_server_id") REFERENCES "mcp_server" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "mcp_server_transport_package" (
    "mcp_server_id" TEXT NOT NULL,
    "kind" TEXT NOT NULL CHECK ("kind" IN ('npm', 'pypi', 'binary')),
    "npm_name" TEXT,
    "npm_version" TEXT,
    "pypi_name" TEXT,
    "pypi_version" TEXT,
    "pypi_entry" TEXT,
    "binary_url" TEXT,
    "binary_sha256" TEXT,
    "binary_entry" TEXT,
    PRIMARY KEY ("mcp_server_id"),
    FOREIGN KEY ("mcp_server_id") REFERENCES "mcp_server_transport" ("mcp_server_id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "mcp_server_transport_args" (
    "mcp_server_id" TEXT NOT NULL,
    "args_pos" INTEGER NOT NULL CHECK ("args_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("mcp_server_id", "args_pos"),
    FOREIGN KEY ("mcp_server_id") REFERENCES "mcp_server_transport" ("mcp_server_id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "mcp_server_env" (
    "mcp_server_id" TEXT NOT NULL,
    "env_key" TEXT NOT NULL,
    "kind" TEXT NOT NULL CHECK ("kind" IN ('secret', 'value')),
    "secret_secret" TEXT,
    "secret_format" TEXT,
    "value_value" TEXT,
    PRIMARY KEY ("mcp_server_id", "env_key"),
    FOREIGN KEY ("mcp_server_id") REFERENCES "mcp_server" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "mcp_server_headers" (
    "mcp_server_id" TEXT NOT NULL,
    "headers_key" TEXT NOT NULL,
    "kind" TEXT NOT NULL CHECK ("kind" IN ('secret', 'value')),
    "secret_secret" TEXT,
    "secret_format" TEXT,
    "value_value" TEXT,
    PRIMARY KEY ("mcp_server_id", "headers_key"),
    FOREIGN KEY ("mcp_server_id") REFERENCES "mcp_server" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "mcp_server_tools" (
    "mcp_server_id" TEXT NOT NULL,
    PRIMARY KEY ("mcp_server_id"),
    FOREIGN KEY ("mcp_server_id") REFERENCES "mcp_server" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "mcp_server_tools_allow" (
    "mcp_server_id" TEXT NOT NULL,
    "all" INTEGER NOT NULL CHECK ("all" IN (0, 1)),
    PRIMARY KEY ("mcp_server_id"),
    FOREIGN KEY ("mcp_server_id") REFERENCES "mcp_server_tools" ("mcp_server_id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "mcp_server_tools_allow_names" (
    "mcp_server_id" TEXT NOT NULL,
    "names_pos" INTEGER NOT NULL CHECK ("names_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("mcp_server_id", "names_pos"),
    FOREIGN KEY ("mcp_server_id") REFERENCES "mcp_server_tools_allow" ("mcp_server_id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "mcp_server_tools_risk" (
    "mcp_server_id" TEXT NOT NULL,
    "risk_key" TEXT NOT NULL,
    "value" TEXT NOT NULL CHECK ("value" IN ('read', 'write', 'irreversible')),
    PRIMARY KEY ("mcp_server_id", "risk_key"),
    FOREIGN KEY ("mcp_server_id") REFERENCES "mcp_server_tools" ("mcp_server_id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "mcp_server_access" (
    "mcp_server_id" TEXT NOT NULL,
    PRIMARY KEY ("mcp_server_id"),
    FOREIGN KEY ("mcp_server_id") REFERENCES "mcp_server" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "mcp_server_access_roles" (
    "mcp_server_id" TEXT NOT NULL,
    "all" INTEGER NOT NULL CHECK ("all" IN (0, 1)),
    PRIMARY KEY ("mcp_server_id"),
    FOREIGN KEY ("mcp_server_id") REFERENCES "mcp_server_access" ("mcp_server_id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "mcp_server_access_roles_names" (
    "mcp_server_id" TEXT NOT NULL,
    "names_pos" INTEGER NOT NULL CHECK ("names_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("mcp_server_id", "names_pos"),
    FOREIGN KEY ("mcp_server_id") REFERENCES "mcp_server_access_roles" ("mcp_server_id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "mcp_server_access_nodes" (
    "mcp_server_id" TEXT NOT NULL,
    "all" INTEGER NOT NULL CHECK ("all" IN (0, 1)),
    PRIMARY KEY ("mcp_server_id"),
    FOREIGN KEY ("mcp_server_id") REFERENCES "mcp_server_access" ("mcp_server_id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "mcp_server_access_nodes_names" (
    "mcp_server_id" TEXT NOT NULL,
    "names_pos" INTEGER NOT NULL CHECK ("names_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("mcp_server_id", "names_pos"),
    FOREIGN KEY ("mcp_server_id") REFERENCES "mcp_server_access_nodes" ("mcp_server_id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "mcp_server_access_projects" (
    "mcp_server_id" TEXT NOT NULL,
    "all" INTEGER NOT NULL CHECK ("all" IN (0, 1)),
    PRIMARY KEY ("mcp_server_id"),
    FOREIGN KEY ("mcp_server_id") REFERENCES "mcp_server_access" ("mcp_server_id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "mcp_server_access_projects_names" (
    "mcp_server_id" TEXT NOT NULL,
    "names_pos" INTEGER NOT NULL CHECK ("names_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("mcp_server_id", "names_pos"),
    FOREIGN KEY ("mcp_server_id") REFERENCES "mcp_server_access_projects" ("mcp_server_id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "mcp_server_offered" (
    "mcp_server_id" TEXT NOT NULL,
    "offered_pos" INTEGER NOT NULL CHECK ("offered_pos" >= 0),
    "name" TEXT NOT NULL,
    "description" TEXT NOT NULL,
    "input_schema_content" TEXT,
    "annotations_title" TEXT,
    "annotations_read_only_hint" INTEGER CHECK ("annotations_read_only_hint" IN (0, 1)),
    "annotations_destructive_hint" INTEGER CHECK ("annotations_destructive_hint" IN (0, 1)),
    "annotations_idempotent_hint" INTEGER CHECK ("annotations_idempotent_hint" IN (0, 1)),
    "annotations_open_world_hint" INTEGER CHECK ("annotations_open_world_hint" IN (0, 1)),
    PRIMARY KEY ("mcp_server_id", "offered_pos"),
    FOREIGN KEY ("mcp_server_id") REFERENCES "mcp_server" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
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

-- component recipe_runs
CREATE TABLE "recipe_run" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "recipe" TEXT NOT NULL,
    "version" INTEGER NOT NULL CHECK ("version" >= 0),
    "source" TEXT NOT NULL,
    "toml_content" TEXT,
    "trigger" TEXT NOT NULL,
    "task" TEXT,
    "project" TEXT,
    "subject" TEXT,
    "state" TEXT NOT NULL CHECK ("state" IN ('running', 'owner', 'passed', 'failed', 'cancelled')),
    "outcome" TEXT,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "updated_ms" INTEGER NOT NULL CHECK ("updated_ms" >= 0),
    PRIMARY KEY ("id")
) STRICT;
CREATE INDEX "recipe_run_task" ON "recipe_run" ("task");
CREATE INDEX "recipe_run_project" ON "recipe_run" ("project");
CREATE INDEX "recipe_run_subject" ON "recipe_run" ("subject");
CREATE TABLE "recipe_run_steps" (
    "recipe_run_id" TEXT NOT NULL,
    "steps_pos" INTEGER NOT NULL CHECK ("steps_pos" >= 0),
    "step" TEXT NOT NULL,
    "state" TEXT NOT NULL CHECK ("state" IN ('waiting', 'ready', 'running', 'passed', 'failed', 'skipped')),
    "visits" INTEGER NOT NULL CHECK ("visits" >= 0),
    "items" INTEGER NOT NULL CHECK ("items" >= 0),
    "reported" INTEGER NOT NULL CHECK ("reported" >= 0),
    "gate" TEXT NOT NULL,
    PRIMARY KEY ("recipe_run_id", "steps_pos"),
    FOREIGN KEY ("recipe_run_id") REFERENCES "recipe_run" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "recipe_run_edges" (
    "recipe_run_id" TEXT NOT NULL,
    "edges_pos" INTEGER NOT NULL CHECK ("edges_pos" >= 0),
    "index" INTEGER NOT NULL CHECK ("index" >= 0),
    "from" TEXT NOT NULL,
    "to" TEXT NOT NULL,
    "state" TEXT NOT NULL CHECK ("state" IN ('open', 'taken', 'dead')),
    "taken" INTEGER NOT NULL CHECK ("taken" >= 0),
    PRIMARY KEY ("recipe_run_id", "edges_pos"),
    FOREIGN KEY ("recipe_run_id") REFERENCES "recipe_run" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "recipe_run_checks" (
    "recipe_run_id" TEXT NOT NULL,
    "checks_pos" INTEGER NOT NULL CHECK ("checks_pos" >= 0),
    "step" TEXT NOT NULL,
    "passed" INTEGER NOT NULL CHECK ("passed" IN (0, 1)),
    "commands" TEXT NOT NULL,
    "summary" TEXT NOT NULL,
    PRIMARY KEY ("recipe_run_id", "checks_pos"),
    FOREIGN KEY ("recipe_run_id") REFERENCES "recipe_run" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component recipe_triggers
CREATE TABLE "recipe_trigger" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "recipe" TEXT NOT NULL,
    "project" TEXT,
    "action" TEXT NOT NULL CHECK ("action" IN ('run', 'full_suite', 'engine_watch')),
    "source" TEXT NOT NULL,
    "event" TEXT NOT NULL,
    "label" TEXT NOT NULL,
    "cron" TEXT NOT NULL,
    "every_minutes" INTEGER NOT NULL CHECK ("every_minutes" >= 0),
    "window_ms" INTEGER NOT NULL CHECK ("window_ms" >= 0),
    "enabled" INTEGER NOT NULL CHECK ("enabled" IN (0, 1)),
    "next_ms" INTEGER CHECK ("next_ms" >= 0),
    "last_ms" INTEGER CHECK ("last_ms" >= 0),
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    PRIMARY KEY ("id")
) STRICT;

-- component recipe_trigger_pending
CREATE TABLE "recipe_trigger_pending" (
    "trigger" TEXT NOT NULL,
    "item" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "recipe" TEXT NOT NULL,
    "project" TEXT,
    "source" TEXT NOT NULL,
    "fire_at" INTEGER NOT NULL CHECK ("fire_at" >= 0),
    "deliveries" INTEGER NOT NULL CHECK ("deliveries" >= 0),
    PRIMARY KEY ("trigger", "item")
) STRICT;

-- component recipe_trigger_fired
CREATE TABLE "recipe_trigger_fired" (
    "trigger" TEXT NOT NULL,
    "identity" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "fired_ms" INTEGER NOT NULL CHECK ("fired_ms" >= 0),
    PRIMARY KEY ("trigger", "identity")
) STRICT;

-- component recipe_versions
CREATE TABLE "recipe_version" (
    "recipe" TEXT NOT NULL,
    "version" INTEGER NOT NULL CHECK ("version" >= 0),
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "toml_content" TEXT,
    "saved_ms" INTEGER NOT NULL CHECK ("saved_ms" >= 0),
    PRIMARY KEY ("recipe", "version")
) STRICT;

-- component releases
CREATE TABLE "release" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "project" TEXT NOT NULL,
    "repo" TEXT NOT NULL,
    "version" TEXT NOT NULL,
    "channel" TEXT NOT NULL,
    "base" TEXT,
    "lane" TEXT NOT NULL,
    "from" TEXT NOT NULL,
    "state" TEXT NOT NULL CHECK ("state" IN ('draft', 'candidate', 'qualifying', 'qualified', 'published', 'promoted', 'failed', 'abandoned', 'superseded', 'yanked')),
    "number" INTEGER CHECK ("number" >= 0),
    "notes_content" TEXT,
    "run" TEXT,
    "sbom" TEXT NOT NULL,
    "provenance" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "updated_ms" INTEGER NOT NULL CHECK ("updated_ms" >= 0),
    PRIMARY KEY ("id")
) STRICT;
CREATE INDEX "release_project" ON "release" ("project");
CREATE TABLE "release_change" (
    "release_id" TEXT NOT NULL,
    "changes_pos" INTEGER NOT NULL CHECK ("changes_pos" >= 0),
    "change" TEXT NOT NULL,
    "title" TEXT NOT NULL,
    "review_run" TEXT,
    PRIMARY KEY ("release_id", "changes_pos"),
    FOREIGN KEY ("release_id") REFERENCES "release" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "release_artifact" (
    "release_id" TEXT NOT NULL,
    "artifacts_pos" INTEGER NOT NULL CHECK ("artifacts_pos" >= 0),
    "target" TEXT NOT NULL,
    "artifact" TEXT NOT NULL,
    "digest" TEXT NOT NULL,
    "size" INTEGER NOT NULL CHECK ("size" >= 0),
    "signatures" TEXT NOT NULL,
    "pool" TEXT NOT NULL,
    "base" TEXT NOT NULL,
    "run" TEXT NOT NULL,
    PRIMARY KEY ("release_id", "artifacts_pos"),
    FOREIGN KEY ("release_id") REFERENCES "release" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "release_verdict" (
    "release_id" TEXT NOT NULL,
    "verdicts_pos" INTEGER NOT NULL CHECK ("verdicts_pos" >= 0),
    "source" TEXT NOT NULL,
    "verdict" TEXT NOT NULL,
    "reason" TEXT NOT NULL,
    "seq" INTEGER NOT NULL CHECK ("seq" >= 0),
    PRIMARY KEY ("release_id", "verdicts_pos"),
    FOREIGN KEY ("release_id") REFERENCES "release" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "release_promotion" (
    "release_id" TEXT NOT NULL,
    "promotions_pos" INTEGER NOT NULL CHECK ("promotions_pos" >= 0),
    "environment" TEXT NOT NULL,
    "seq" INTEGER NOT NULL CHECK ("seq" >= 0),
    "health" TEXT NOT NULL,
    PRIMARY KEY ("release_id", "promotions_pos"),
    FOREIGN KEY ("release_id") REFERENCES "release" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "release_sync" (
    "release_id" TEXT NOT NULL,
    "sync_pos" INTEGER NOT NULL CHECK ("sync_pos" >= 0),
    "platform" TEXT NOT NULL,
    "state" TEXT NOT NULL,
    "url" TEXT,
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    PRIMARY KEY ("release_id", "sync_pos"),
    FOREIGN KEY ("release_id") REFERENCES "release" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
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
    "disk_quota_bytes" INTEGER CHECK ("disk_quota_bytes" >= 0),
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
    "kind" TEXT NOT NULL CHECK ("kind" IN ('start', 'budget', 'allowance', 'disk', 'connector', 'tool', 'engine', 'recipe', 'promote', 'memory', 'review', 'land', 'adopt', 'yank', 'seat', 'release', 'deploy', 'tool_call', 'incident', 'proposal')),
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
    "model" TEXT,
    "draft_seen" INTEGER NOT NULL CHECK ("draft_seen" >= 0),
    PRIMARY KEY ("id")
) STRICT;
CREATE INDEX "chat_project" ON "chat" ("project");
CREATE INDEX "chat_task" ON "chat" ("task");
CREATE TABLE "chat_draft" (
    "chat_id" TEXT NOT NULL,
    "revision" INTEGER NOT NULL CHECK ("revision" >= 0),
    "title" TEXT NOT NULL,
    "goal" TEXT NOT NULL,
    "kind" TEXT NOT NULL,
    "repo" TEXT,
    "github" TEXT NOT NULL,
    "disk_quota_bytes" INTEGER CHECK ("disk_quota_bytes" >= 0),
    "updated_by" TEXT NOT NULL,
    "updated_ms" INTEGER NOT NULL CHECK ("updated_ms" >= 0),
    "started_ms" INTEGER CHECK ("started_ms" >= 0),
    PRIMARY KEY ("chat_id"),
    FOREIGN KEY ("chat_id") REFERENCES "chat" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "chat_draft_tasks" (
    "chat_id" TEXT NOT NULL,
    "tasks_pos" INTEGER NOT NULL CHECK ("tasks_pos" >= 0),
    "key" TEXT NOT NULL,
    "goal" TEXT NOT NULL,
    "acceptance" TEXT,
    "role" TEXT,
    PRIMARY KEY ("chat_id", "tasks_pos"),
    FOREIGN KEY ("chat_id") REFERENCES "chat_draft" ("chat_id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "chat_draft_tasks_after" (
    "chat_id" TEXT NOT NULL,
    "tasks_pos" INTEGER NOT NULL CHECK ("tasks_pos" >= 0),
    "after_pos" INTEGER NOT NULL CHECK ("after_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("chat_id", "tasks_pos", "after_pos"),
    FOREIGN KEY ("chat_id", "tasks_pos") REFERENCES "chat_draft_tasks" ("chat_id", "tasks_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "chat_draft_budget" (
    "chat_id" TEXT NOT NULL,
    "unit" TEXT NOT NULL CHECK ("unit" IN ('usd', 'tokens')),
    "cap" INTEGER NOT NULL CHECK ("cap" >= 0),
    "monthly_cap" INTEGER CHECK ("monthly_cap" >= 0),
    PRIMARY KEY ("chat_id"),
    FOREIGN KEY ("chat_id") REFERENCES "chat_draft" ("chat_id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "chat_draft_settings" (
    "chat_id" TEXT NOT NULL,
    "default_model" TEXT,
    "effort" TEXT,
    "max_tokens" INTEGER CHECK ("max_tokens" >= 0),
    "max_turns" INTEGER CHECK ("max_turns" >= 0),
    "compress_at" INTEGER CHECK ("compress_at" >= 0),
    "keep_recent" INTEGER CHECK ("keep_recent" >= 0),
    "task_budget" INTEGER CHECK ("task_budget" >= 0),
    "permission_mode" TEXT CHECK ("permission_mode" IN ('autonomous', 'ask', 'read_only')),
    "review_timeout_minutes" INTEGER CHECK ("review_timeout_minutes" >= 0),
    "full_suite_minutes" INTEGER CHECK ("full_suite_minutes" >= 0),
    "team_max" INTEGER CHECK ("team_max" >= 0),
    "team_target" INTEGER CHECK ("team_target" >= 0),
    "queue_batch" INTEGER CHECK ("queue_batch" >= 0),
    "land_approval" INTEGER NOT NULL CHECK ("land_approval" IN (0, 1)),
    "release_continuous" INTEGER NOT NULL CHECK ("release_continuous" IN (0, 1)),
    "hook_defaults" INTEGER NOT NULL CHECK ("hook_defaults" IN (0, 1)),
    PRIMARY KEY ("chat_id"),
    FOREIGN KEY ("chat_id") REFERENCES "chat_draft" ("chat_id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "chat_draft_settings_role_models" (
    "chat_id" TEXT NOT NULL,
    "role_models_key" TEXT NOT NULL,
    "value" TEXT NOT NULL,
    PRIMARY KEY ("chat_id", "role_models_key"),
    FOREIGN KEY ("chat_id") REFERENCES "chat_draft_settings" ("chat_id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "chat_draft_settings_hooks" (
    "chat_id" TEXT NOT NULL,
    "hooks_pos" INTEGER NOT NULL CHECK ("hooks_pos" >= 0),
    "on" TEXT NOT NULL CHECK ("on" IN ('edit', 'run', 'land')),
    "action" TEXT NOT NULL CHECK ("action" IN ('format', 'lint', 'rule', 'secret_scan')),
    "rule_id" TEXT,
    "mode" TEXT NOT NULL CHECK ("mode" IN ('report', 'block')),
    PRIMARY KEY ("chat_id", "hooks_pos"),
    FOREIGN KEY ("chat_id") REFERENCES "chat_draft_settings" ("chat_id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "chat_draft_settings_hooks_paths" (
    "chat_id" TEXT NOT NULL,
    "hooks_pos" INTEGER NOT NULL CHECK ("hooks_pos" >= 0),
    "paths_pos" INTEGER NOT NULL CHECK ("paths_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("chat_id", "hooks_pos", "paths_pos"),
    FOREIGN KEY ("chat_id", "hooks_pos") REFERENCES "chat_draft_settings_hooks" ("chat_id", "hooks_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "chat_draft_settings_hooks_languages" (
    "chat_id" TEXT NOT NULL,
    "hooks_pos" INTEGER NOT NULL CHECK ("hooks_pos" >= 0),
    "languages_pos" INTEGER NOT NULL CHECK ("languages_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("chat_id", "hooks_pos", "languages_pos"),
    FOREIGN KEY ("chat_id", "hooks_pos") REFERENCES "chat_draft_settings_hooks" ("chat_id", "hooks_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "chat_draft_settings_intake" (
    "chat_id" TEXT NOT NULL,
    "intake_pos" INTEGER NOT NULL CHECK ("intake_pos" >= 0),
    "source" TEXT NOT NULL,
    "field" TEXT NOT NULL,
    "value" TEXT NOT NULL,
    "recipe" TEXT NOT NULL,
    "allow_public" INTEGER NOT NULL CHECK ("allow_public" IN (0, 1)),
    "status_property" TEXT NOT NULL,
    "started" TEXT NOT NULL,
    "done" TEXT NOT NULL,
    PRIMARY KEY ("chat_id", "intake_pos"),
    FOREIGN KEY ("chat_id") REFERENCES "chat_draft_settings" ("chat_id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "chat_works" (
    "chat_id" TEXT NOT NULL,
    "works_pos" INTEGER NOT NULL CHECK ("works_pos" >= 0),
    "value" INTEGER NOT NULL CHECK ("value" >= 0),
    PRIMARY KEY ("chat_id", "works_pos"),
    FOREIGN KEY ("chat_id") REFERENCES "chat" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
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
CREATE TABLE "chat_messages_attachments" (
    "chat_id" TEXT NOT NULL,
    "messages_pos" INTEGER NOT NULL CHECK ("messages_pos" >= 0),
    "attachments_pos" INTEGER NOT NULL CHECK ("attachments_pos" >= 0),
    "id" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "size" INTEGER NOT NULL CHECK ("size" >= 0),
    "mime" TEXT NOT NULL,
    PRIMARY KEY ("chat_id", "messages_pos", "attachments_pos"),
    FOREIGN KEY ("chat_id", "messages_pos") REFERENCES "chat_messages" ("chat_id", "messages_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
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

-- component journal
CREATE TABLE "journal" (
    "task" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    PRIMARY KEY ("task")
) STRICT;
CREATE TABLE "journal_notes" (
    "journal_task" TEXT NOT NULL,
    "notes_pos" INTEGER NOT NULL CHECK ("notes_pos" >= 0),
    "seq" INTEGER NOT NULL CHECK ("seq" >= 0),
    "kind" TEXT NOT NULL CHECK ("kind" IN ('decision', 'dead_end', 'open_question')),
    "session" TEXT,
    "text_content" TEXT,
    "reason_content" TEXT,
    "thread" TEXT,
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    PRIMARY KEY ("journal_task", "notes_pos"),
    FOREIGN KEY ("journal_task") REFERENCES "journal" ("task") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "journal_handovers" (
    "journal_task" TEXT NOT NULL,
    "handovers_pos" INTEGER NOT NULL CHECK ("handovers_pos" >= 0),
    "seq" INTEGER NOT NULL CHECK ("seq" >= 0),
    "session" TEXT NOT NULL,
    "stands_content" TEXT,
    "done_content" TEXT,
    "next_content" TEXT,
    "check_first_content" TEXT,
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    PRIMARY KEY ("journal_task", "handovers_pos"),
    FOREIGN KEY ("journal_task") REFERENCES "journal" ("task") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component decisions
CREATE TABLE "decisions_default" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "provider" TEXT NOT NULL,
    "model" TEXT NOT NULL,
    PRIMARY KEY ("pos")
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

-- component engines
CREATE TABLE "engine_instance" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "engine" TEXT NOT NULL,
    "label" TEXT,
    "accent" TEXT,
    "enabled" INTEGER NOT NULL CHECK ("enabled" IN (0, 1)),
    "home" TEXT,
    "route" TEXT,
    PRIMARY KEY ("id")
) STRICT;
CREATE TABLE "engine_install" (
    "engine_instance_id" TEXT NOT NULL,
    "kind" TEXT NOT NULL CHECK ("kind" IN ('npm', 'pip', 'archive', 'command')),
    "package" TEXT,
    "version" TEXT,
    "integrity" TEXT,
    "file" TEXT,
    "sha256" TEXT,
    "command" TEXT,
    PRIMARY KEY ("engine_instance_id"),
    FOREIGN KEY ("engine_instance_id") REFERENCES "engine_instance" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "engine_asset" (
    "engine_instance_id" TEXT NOT NULL,
    "assets_key" TEXT NOT NULL,
    "url" TEXT NOT NULL,
    "sha256" TEXT NOT NULL,
    "cmd" TEXT NOT NULL,
    PRIMARY KEY ("engine_instance_id", "assets_key"),
    FOREIGN KEY ("engine_instance_id") REFERENCES "engine_install" ("engine_instance_id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "engine_asset_arg" (
    "engine_instance_id" TEXT NOT NULL,
    "assets_key" TEXT NOT NULL,
    "args_pos" INTEGER NOT NULL CHECK ("args_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("engine_instance_id", "assets_key", "args_pos"),
    FOREIGN KEY ("engine_instance_id", "assets_key") REFERENCES "engine_asset" ("engine_instance_id", "assets_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "engine_arg" (
    "engine_instance_id" TEXT NOT NULL,
    "args_pos" INTEGER NOT NULL CHECK ("args_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("engine_instance_id", "args_pos"),
    FOREIGN KEY ("engine_instance_id") REFERENCES "engine_instance" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "engine_env" (
    "engine_instance_id" TEXT NOT NULL,
    "env_key" TEXT NOT NULL,
    "value" TEXT NOT NULL,
    PRIMARY KEY ("engine_instance_id", "env_key"),
    FOREIGN KEY ("engine_instance_id") REFERENCES "engine_instance" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "engine_about" (
    "engine_instance_id" TEXT NOT NULL,
    "name" TEXT,
    "description" TEXT,
    "icon" TEXT,
    "website" TEXT,
    "repository" TEXT,
    "version" TEXT,
    PRIMARY KEY ("engine_instance_id"),
    FOREIGN KEY ("engine_instance_id") REFERENCES "engine_instance" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component review_graph
CREATE TABLE "review_graph" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "next" INTEGER NOT NULL CHECK ("next" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "review_graph_run" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "runs_key" TEXT NOT NULL,
    "id" TEXT NOT NULL,
    "task" TEXT NOT NULL,
    "project" TEXT NOT NULL,
    "worker" INTEGER NOT NULL CHECK ("worker" >= 0),
    "change" TEXT NOT NULL,
    "revision" TEXT NOT NULL,
    "recipe" TEXT NOT NULL,
    "round" INTEGER NOT NULL CHECK ("round" >= 0),
    "rounds" INTEGER NOT NULL CHECK ("rounds" >= 0),
    "kind" TEXT NOT NULL,
    "state" TEXT NOT NULL CHECK ("state" IN ('running', 'passed', 'rework', 'owner', 'failed')),
    "owner_asked" INTEGER NOT NULL CHECK ("owner_asked" IN (0, 1)),
    "tree_blob" TEXT,
    "summary" TEXT NOT NULL,
    "purpose" TEXT NOT NULL,
    "repo" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "updated_ms" INTEGER NOT NULL CHECK ("updated_ms" >= 0),
    PRIMARY KEY ("review_graph_pos", "runs_key"),
    FOREIGN KEY ("review_graph_pos") REFERENCES "review_graph" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_run_nodes" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "runs_key" TEXT NOT NULL,
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "parent" TEXT,
    "state" TEXT NOT NULL CHECK ("state" IN ('waiting', 'running', 'passed', 'failed', 'skipped')),
    "assignee" INTEGER CHECK ("assignee" >= 0),
    "work" INTEGER CHECK ("work" >= 0),
    "tries" INTEGER NOT NULL CHECK ("tries" >= 0),
    "notes" TEXT NOT NULL,
    "record" INTEGER CHECK ("record" >= 0),
    "started_ms" INTEGER NOT NULL CHECK ("started_ms" >= 0),
    "ended_ms" INTEGER NOT NULL CHECK ("ended_ms" >= 0),
    "spawned" INTEGER NOT NULL CHECK ("spawned" >= 0),
    PRIMARY KEY ("review_graph_pos", "runs_key", "nodes_pos"),
    FOREIGN KEY ("review_graph_pos", "runs_key") REFERENCES "review_graph_run" ("review_graph_pos", "runs_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_run_nodes_def" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "runs_key" TEXT NOT NULL,
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "id" TEXT NOT NULL,
    "kind" TEXT NOT NULL,
    "tool" TEXT,
    "role" TEXT,
    "sub_reviews" INTEGER NOT NULL CHECK ("sub_reviews" >= 0),
    "constraints" INTEGER NOT NULL CHECK ("constraints" IN (0, 1)),
    "ask" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "runs_key", "nodes_pos"),
    FOREIGN KEY ("review_graph_pos", "runs_key", "nodes_pos") REFERENCES "review_graph_run_nodes" ("review_graph_pos", "runs_key", "nodes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_run_nodes_def_axes" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "runs_key" TEXT NOT NULL,
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "axes_pos" INTEGER NOT NULL CHECK ("axes_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "runs_key", "nodes_pos", "axes_pos"),
    FOREIGN KEY ("review_graph_pos", "runs_key", "nodes_pos") REFERENCES "review_graph_run_nodes_def" ("review_graph_pos", "runs_key", "nodes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_run_nodes_def_commands" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "runs_key" TEXT NOT NULL,
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "commands_pos" INTEGER NOT NULL CHECK ("commands_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "runs_key", "nodes_pos", "commands_pos"),
    FOREIGN KEY ("review_graph_pos", "runs_key", "nodes_pos") REFERENCES "review_graph_run_nodes_def" ("review_graph_pos", "runs_key", "nodes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_run_nodes_def_after" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "runs_key" TEXT NOT NULL,
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "after_pos" INTEGER NOT NULL CHECK ("after_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "runs_key", "nodes_pos", "after_pos"),
    FOREIGN KEY ("review_graph_pos", "runs_key", "nodes_pos") REFERENCES "review_graph_run_nodes_def" ("review_graph_pos", "runs_key", "nodes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_run_nodes_scores" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "runs_key" TEXT NOT NULL,
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "scores_pos" INTEGER NOT NULL CHECK ("scores_pos" >= 0),
    "symbol" TEXT NOT NULL,
    "lineage" TEXT,
    "axis" TEXT NOT NULL,
    "level" INTEGER NOT NULL,
    "confidence" REAL NOT NULL,
    "basis" TEXT NOT NULL,
    "evidence" TEXT NOT NULL,
    "node" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "runs_key", "nodes_pos", "scores_pos"),
    FOREIGN KEY ("review_graph_pos", "runs_key", "nodes_pos") REFERENCES "review_graph_run_nodes" ("review_graph_pos", "runs_key", "nodes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_run_nodes_findings" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "runs_key" TEXT NOT NULL,
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "findings_pos" INTEGER NOT NULL CHECK ("findings_pos" >= 0),
    "symbol" TEXT NOT NULL,
    "lineage" TEXT,
    "text" TEXT NOT NULL,
    "severity" TEXT NOT NULL,
    "node" TEXT NOT NULL,
    "round" INTEGER NOT NULL CHECK ("round" >= 0),
    "id" TEXT NOT NULL,
    "scenario" TEXT NOT NULL,
    "kind" TEXT NOT NULL,
    "verdict" TEXT NOT NULL CHECK ("verdict" IN ('pending', 'confirmed', 'dropped')),
    "proof" TEXT NOT NULL,
    "reason" TEXT NOT NULL,
    "contest" TEXT NOT NULL,
    "ruling" TEXT NOT NULL,
    "ruling_reason" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "runs_key", "nodes_pos", "findings_pos"),
    FOREIGN KEY ("review_graph_pos", "runs_key", "nodes_pos") REFERENCES "review_graph_run_nodes" ("review_graph_pos", "runs_key", "nodes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_run_nodes_findings_by" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "runs_key" TEXT NOT NULL,
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "findings_pos" INTEGER NOT NULL CHECK ("findings_pos" >= 0),
    "seat" TEXT NOT NULL,
    "model" TEXT NOT NULL,
    "session" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "runs_key", "nodes_pos", "findings_pos"),
    FOREIGN KEY ("review_graph_pos", "runs_key", "nodes_pos", "findings_pos") REFERENCES "review_graph_run_nodes_findings" ("review_graph_pos", "runs_key", "nodes_pos", "findings_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_run_nodes_findings_verified_by" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "runs_key" TEXT NOT NULL,
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "findings_pos" INTEGER NOT NULL CHECK ("findings_pos" >= 0),
    "seat" TEXT NOT NULL,
    "model" TEXT NOT NULL,
    "session" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "runs_key", "nodes_pos", "findings_pos"),
    FOREIGN KEY ("review_graph_pos", "runs_key", "nodes_pos", "findings_pos") REFERENCES "review_graph_run_nodes_findings" ("review_graph_pos", "runs_key", "nodes_pos", "findings_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_run_nodes_touches" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "runs_key" TEXT NOT NULL,
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "touches_pos" INTEGER NOT NULL CHECK ("touches_pos" >= 0),
    "from" TEXT NOT NULL,
    "to" TEXT NOT NULL,
    "how" TEXT NOT NULL,
    "note" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "runs_key", "nodes_pos", "touches_pos"),
    FOREIGN KEY ("review_graph_pos", "runs_key", "nodes_pos") REFERENCES "review_graph_run_nodes" ("review_graph_pos", "runs_key", "nodes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_run_rule" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "runs_key" TEXT NOT NULL,
    "rules_pos" INTEGER NOT NULL CHECK ("rules_pos" >= 0),
    "when" TEXT NOT NULL,
    "then" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "runs_key", "rules_pos"),
    FOREIGN KEY ("review_graph_pos", "runs_key") REFERENCES "review_graph_run" ("review_graph_pos", "runs_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_run_outcome" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "runs_key" TEXT NOT NULL,
    "decision" TEXT NOT NULL,
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    "by" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "runs_key"),
    FOREIGN KEY ("review_graph_pos", "runs_key") REFERENCES "review_graph_run" ("review_graph_pos", "runs_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_run_outcome_reasons" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "runs_key" TEXT NOT NULL,
    "reasons_pos" INTEGER NOT NULL CHECK ("reasons_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "runs_key", "reasons_pos"),
    FOREIGN KEY ("review_graph_pos", "runs_key") REFERENCES "review_graph_run_outcome" ("review_graph_pos", "runs_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_run_outcome_replan" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "runs_key" TEXT NOT NULL,
    "replan_pos" INTEGER NOT NULL CHECK ("replan_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "runs_key", "replan_pos"),
    FOREIGN KEY ("review_graph_pos", "runs_key") REFERENCES "review_graph_run_outcome" ("review_graph_pos", "runs_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_run_model" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "runs_key" TEXT NOT NULL,
    "models_pos" INTEGER NOT NULL CHECK ("models_pos" >= 0),
    "node" TEXT NOT NULL,
    "cid" INTEGER NOT NULL CHECK ("cid" >= 0),
    "model" TEXT NOT NULL,
    "session" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "runs_key", "models_pos"),
    FOREIGN KEY ("review_graph_pos", "runs_key") REFERENCES "review_graph_run" ("review_graph_pos", "runs_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_recipe" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "recipes_pos" INTEGER NOT NULL CHECK ("recipes_pos" >= 0),
    "id" TEXT NOT NULL,
    "version" INTEGER NOT NULL CHECK ("version" >= 0),
    "title" TEXT NOT NULL,
    "rounds" INTEGER NOT NULL CHECK ("rounds" >= 0),
    PRIMARY KEY ("review_graph_pos", "recipes_pos"),
    FOREIGN KEY ("review_graph_pos") REFERENCES "review_graph" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_recipe_applies" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "recipes_pos" INTEGER NOT NULL CHECK ("recipes_pos" >= 0),
    PRIMARY KEY ("review_graph_pos", "recipes_pos"),
    FOREIGN KEY ("review_graph_pos", "recipes_pos") REFERENCES "review_graph_recipe" ("review_graph_pos", "recipes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_recipe_applies_kinds" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "recipes_pos" INTEGER NOT NULL CHECK ("recipes_pos" >= 0),
    "kinds_pos" INTEGER NOT NULL CHECK ("kinds_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "recipes_pos", "kinds_pos"),
    FOREIGN KEY ("review_graph_pos", "recipes_pos") REFERENCES "review_graph_recipe_applies" ("review_graph_pos", "recipes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_recipe_applies_roles" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "recipes_pos" INTEGER NOT NULL CHECK ("recipes_pos" >= 0),
    "roles_pos" INTEGER NOT NULL CHECK ("roles_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "recipes_pos", "roles_pos"),
    FOREIGN KEY ("review_graph_pos", "recipes_pos") REFERENCES "review_graph_recipe_applies" ("review_graph_pos", "recipes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_recipe_axes" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "recipes_pos" INTEGER NOT NULL CHECK ("recipes_pos" >= 0),
    "axes_pos" INTEGER NOT NULL CHECK ("axes_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "recipes_pos", "axes_pos"),
    FOREIGN KEY ("review_graph_pos", "recipes_pos") REFERENCES "review_graph_recipe" ("review_graph_pos", "recipes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_recipe_first" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "recipes_pos" INTEGER NOT NULL CHECK ("recipes_pos" >= 0),
    "first_pos" INTEGER NOT NULL CHECK ("first_pos" >= 0),
    "when" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "recipes_pos", "first_pos"),
    FOREIGN KEY ("review_graph_pos", "recipes_pos") REFERENCES "review_graph_recipe" ("review_graph_pos", "recipes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_recipe_first_only" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "recipes_pos" INTEGER NOT NULL CHECK ("recipes_pos" >= 0),
    "first_pos" INTEGER NOT NULL CHECK ("first_pos" >= 0),
    "only_pos" INTEGER NOT NULL CHECK ("only_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "recipes_pos", "first_pos", "only_pos"),
    FOREIGN KEY ("review_graph_pos", "recipes_pos", "first_pos") REFERENCES "review_graph_recipe_first" ("review_graph_pos", "recipes_pos", "first_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_recipe_first_add" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "recipes_pos" INTEGER NOT NULL CHECK ("recipes_pos" >= 0),
    "first_pos" INTEGER NOT NULL CHECK ("first_pos" >= 0),
    "add_pos" INTEGER NOT NULL CHECK ("add_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "recipes_pos", "first_pos", "add_pos"),
    FOREIGN KEY ("review_graph_pos", "recipes_pos", "first_pos") REFERENCES "review_graph_recipe_first" ("review_graph_pos", "recipes_pos", "first_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_recipe_nodes" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "recipes_pos" INTEGER NOT NULL CHECK ("recipes_pos" >= 0),
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "id" TEXT NOT NULL,
    "kind" TEXT NOT NULL,
    "tool" TEXT,
    "role" TEXT,
    "sub_reviews" INTEGER NOT NULL CHECK ("sub_reviews" >= 0),
    "constraints" INTEGER NOT NULL CHECK ("constraints" IN (0, 1)),
    "ask" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "recipes_pos", "nodes_pos"),
    FOREIGN KEY ("review_graph_pos", "recipes_pos") REFERENCES "review_graph_recipe" ("review_graph_pos", "recipes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_recipe_nodes_axes" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "recipes_pos" INTEGER NOT NULL CHECK ("recipes_pos" >= 0),
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "axes_pos" INTEGER NOT NULL CHECK ("axes_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "recipes_pos", "nodes_pos", "axes_pos"),
    FOREIGN KEY ("review_graph_pos", "recipes_pos", "nodes_pos") REFERENCES "review_graph_recipe_nodes" ("review_graph_pos", "recipes_pos", "nodes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_recipe_nodes_commands" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "recipes_pos" INTEGER NOT NULL CHECK ("recipes_pos" >= 0),
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "commands_pos" INTEGER NOT NULL CHECK ("commands_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "recipes_pos", "nodes_pos", "commands_pos"),
    FOREIGN KEY ("review_graph_pos", "recipes_pos", "nodes_pos") REFERENCES "review_graph_recipe_nodes" ("review_graph_pos", "recipes_pos", "nodes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_recipe_nodes_after" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "recipes_pos" INTEGER NOT NULL CHECK ("recipes_pos" >= 0),
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "after_pos" INTEGER NOT NULL CHECK ("after_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "recipes_pos", "nodes_pos", "after_pos"),
    FOREIGN KEY ("review_graph_pos", "recipes_pos", "nodes_pos") REFERENCES "review_graph_recipe_nodes" ("review_graph_pos", "recipes_pos", "nodes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_recipe_rules" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "recipes_pos" INTEGER NOT NULL CHECK ("recipes_pos" >= 0),
    "rules_pos" INTEGER NOT NULL CHECK ("rules_pos" >= 0),
    "when" TEXT NOT NULL,
    "then" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "recipes_pos", "rules_pos"),
    FOREIGN KEY ("review_graph_pos", "recipes_pos") REFERENCES "review_graph_recipe" ("review_graph_pos", "recipes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_recipe_post_land" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "recipes_pos" INTEGER NOT NULL CHECK ("recipes_pos" >= 0),
    "post_land_pos" INTEGER NOT NULL CHECK ("post_land_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "recipes_pos", "post_land_pos"),
    FOREIGN KEY ("review_graph_pos", "recipes_pos") REFERENCES "review_graph_recipe" ("review_graph_pos", "recipes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_proposal" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    "id" TEXT NOT NULL,
    "by" INTEGER NOT NULL CHECK ("by" >= 0),
    "why" TEXT NOT NULL,
    "state" TEXT NOT NULL,
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    PRIMARY KEY ("review_graph_pos", "proposals_pos"),
    FOREIGN KEY ("review_graph_pos") REFERENCES "review_graph" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_proposal_recipe" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    "id" TEXT NOT NULL,
    "version" INTEGER NOT NULL CHECK ("version" >= 0),
    "title" TEXT NOT NULL,
    "rounds" INTEGER NOT NULL CHECK ("rounds" >= 0),
    PRIMARY KEY ("review_graph_pos", "proposals_pos"),
    FOREIGN KEY ("review_graph_pos", "proposals_pos") REFERENCES "review_graph_proposal" ("review_graph_pos", "proposals_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_proposal_recipe_applies" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    PRIMARY KEY ("review_graph_pos", "proposals_pos"),
    FOREIGN KEY ("review_graph_pos", "proposals_pos") REFERENCES "review_graph_proposal_recipe" ("review_graph_pos", "proposals_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_proposal_recipe_applies_kinds" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    "kinds_pos" INTEGER NOT NULL CHECK ("kinds_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "proposals_pos", "kinds_pos"),
    FOREIGN KEY ("review_graph_pos", "proposals_pos") REFERENCES "review_graph_proposal_recipe_applies" ("review_graph_pos", "proposals_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_proposal_recipe_applies_roles" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    "roles_pos" INTEGER NOT NULL CHECK ("roles_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "proposals_pos", "roles_pos"),
    FOREIGN KEY ("review_graph_pos", "proposals_pos") REFERENCES "review_graph_proposal_recipe_applies" ("review_graph_pos", "proposals_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_proposal_recipe_axes" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    "axes_pos" INTEGER NOT NULL CHECK ("axes_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "proposals_pos", "axes_pos"),
    FOREIGN KEY ("review_graph_pos", "proposals_pos") REFERENCES "review_graph_proposal_recipe" ("review_graph_pos", "proposals_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_proposal_recipe_first" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    "first_pos" INTEGER NOT NULL CHECK ("first_pos" >= 0),
    "when" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "proposals_pos", "first_pos"),
    FOREIGN KEY ("review_graph_pos", "proposals_pos") REFERENCES "review_graph_proposal_recipe" ("review_graph_pos", "proposals_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_proposal_recipe_first_only" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    "first_pos" INTEGER NOT NULL CHECK ("first_pos" >= 0),
    "only_pos" INTEGER NOT NULL CHECK ("only_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "proposals_pos", "first_pos", "only_pos"),
    FOREIGN KEY ("review_graph_pos", "proposals_pos", "first_pos") REFERENCES "review_graph_proposal_recipe_first" ("review_graph_pos", "proposals_pos", "first_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_proposal_recipe_first_add" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    "first_pos" INTEGER NOT NULL CHECK ("first_pos" >= 0),
    "add_pos" INTEGER NOT NULL CHECK ("add_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "proposals_pos", "first_pos", "add_pos"),
    FOREIGN KEY ("review_graph_pos", "proposals_pos", "first_pos") REFERENCES "review_graph_proposal_recipe_first" ("review_graph_pos", "proposals_pos", "first_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_proposal_recipe_nodes" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "id" TEXT NOT NULL,
    "kind" TEXT NOT NULL,
    "tool" TEXT,
    "role" TEXT,
    "sub_reviews" INTEGER NOT NULL CHECK ("sub_reviews" >= 0),
    "constraints" INTEGER NOT NULL CHECK ("constraints" IN (0, 1)),
    "ask" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "proposals_pos", "nodes_pos"),
    FOREIGN KEY ("review_graph_pos", "proposals_pos") REFERENCES "review_graph_proposal_recipe" ("review_graph_pos", "proposals_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_proposal_recipe_nodes_axes" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "axes_pos" INTEGER NOT NULL CHECK ("axes_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "proposals_pos", "nodes_pos", "axes_pos"),
    FOREIGN KEY ("review_graph_pos", "proposals_pos", "nodes_pos") REFERENCES "review_graph_proposal_recipe_nodes" ("review_graph_pos", "proposals_pos", "nodes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_proposal_recipe_nodes_commands" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "commands_pos" INTEGER NOT NULL CHECK ("commands_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "proposals_pos", "nodes_pos", "commands_pos"),
    FOREIGN KEY ("review_graph_pos", "proposals_pos", "nodes_pos") REFERENCES "review_graph_proposal_recipe_nodes" ("review_graph_pos", "proposals_pos", "nodes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_proposal_recipe_nodes_after" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "after_pos" INTEGER NOT NULL CHECK ("after_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "proposals_pos", "nodes_pos", "after_pos"),
    FOREIGN KEY ("review_graph_pos", "proposals_pos", "nodes_pos") REFERENCES "review_graph_proposal_recipe_nodes" ("review_graph_pos", "proposals_pos", "nodes_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_proposal_recipe_rules" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    "rules_pos" INTEGER NOT NULL CHECK ("rules_pos" >= 0),
    "when" TEXT NOT NULL,
    "then" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "proposals_pos", "rules_pos"),
    FOREIGN KEY ("review_graph_pos", "proposals_pos") REFERENCES "review_graph_proposal_recipe" ("review_graph_pos", "proposals_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "review_graph_proposal_recipe_post_land" (
    "review_graph_pos" INTEGER NOT NULL CHECK ("review_graph_pos" >= 0),
    "proposals_pos" INTEGER NOT NULL CHECK ("proposals_pos" >= 0),
    "post_land_pos" INTEGER NOT NULL CHECK ("post_land_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("review_graph_pos", "proposals_pos", "post_land_pos"),
    FOREIGN KEY ("review_graph_pos", "proposals_pos") REFERENCES "review_graph_proposal_recipe" ("review_graph_pos", "proposals_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component usage_providers
CREATE TABLE "usage_provider" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "kind" TEXT NOT NULL,
    "url" TEXT NOT NULL,
    "label" TEXT,
    PRIMARY KEY ("id")
) STRICT;

-- component hub_config
CREATE TABLE "hub_config" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "owner_token_sha256" TEXT NOT NULL,
    "public_url" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    PRIMARY KEY ("pos")
) STRICT;

-- component platforms
CREATE TABLE "platforms" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "next" INTEGER NOT NULL CHECK ("next" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "platforms_registrations" (
    "platforms_pos" INTEGER NOT NULL CHECK ("platforms_pos" >= 0),
    "registrations_pos" INTEGER NOT NULL CHECK ("registrations_pos" >= 0),
    "id" TEXT NOT NULL,
    "kind" TEXT NOT NULL CHECK ("kind" IN ('github', 'gitlab', 'forgejo')),
    "host" TEXT NOT NULL,
    "client_id" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    PRIMARY KEY ("platforms_pos", "registrations_pos"),
    FOREIGN KEY ("platforms_pos") REFERENCES "platforms" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "platforms_registrations_app" (
    "platforms_pos" INTEGER NOT NULL CHECK ("platforms_pos" >= 0),
    "registrations_pos" INTEGER NOT NULL CHECK ("registrations_pos" >= 0),
    "id" INTEGER NOT NULL CHECK ("id" >= 0),
    "slug" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "owner" TEXT NOT NULL,
    "html_url" TEXT NOT NULL,
    PRIMARY KEY ("platforms_pos", "registrations_pos"),
    FOREIGN KEY ("platforms_pos", "registrations_pos") REFERENCES "platforms_registrations" ("platforms_pos", "registrations_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "platforms_integrations" (
    "platforms_pos" INTEGER NOT NULL CHECK ("platforms_pos" >= 0),
    "integrations_pos" INTEGER NOT NULL CHECK ("integrations_pos" >= 0),
    "id" TEXT NOT NULL,
    "kind" TEXT NOT NULL CHECK ("kind" IN ('github', 'gitlab', 'forgejo')),
    "host" TEXT NOT NULL,
    "via" TEXT NOT NULL CHECK ("via" IN ('app', 'oauth', 'token')),
    "level" TEXT NOT NULL CHECK ("level" IN ('user', 'org', 'repo')),
    "registration" TEXT,
    "installation" INTEGER CHECK ("installation" >= 0),
    "repository_selection" TEXT,
    "manage_url" TEXT,
    "target" TEXT,
    "account_id" INTEGER NOT NULL CHECK ("account_id" >= 0),
    "account_login" TEXT NOT NULL,
    "account_name" TEXT NOT NULL,
    "account_email" TEXT NOT NULL,
    "account_avatar_url" TEXT NOT NULL,
    "expires_ms" INTEGER CHECK ("expires_ms" >= 0),
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "checked_ms" INTEGER NOT NULL CHECK ("checked_ms" >= 0),
    "error" TEXT,
    PRIMARY KEY ("platforms_pos", "integrations_pos"),
    FOREIGN KEY ("platforms_pos") REFERENCES "platforms" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "platforms_integrations_scopes" (
    "platforms_pos" INTEGER NOT NULL CHECK ("platforms_pos" >= 0),
    "integrations_pos" INTEGER NOT NULL CHECK ("integrations_pos" >= 0),
    "scopes_pos" INTEGER NOT NULL CHECK ("scopes_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("platforms_pos", "integrations_pos", "scopes_pos"),
    FOREIGN KEY ("platforms_pos", "integrations_pos") REFERENCES "platforms_integrations" ("platforms_pos", "integrations_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "platforms_integrations_grants" (
    "platforms_pos" INTEGER NOT NULL CHECK ("platforms_pos" >= 0),
    "integrations_pos" INTEGER NOT NULL CHECK ("integrations_pos" >= 0),
    PRIMARY KEY ("platforms_pos", "integrations_pos"),
    FOREIGN KEY ("platforms_pos", "integrations_pos") REFERENCES "platforms_integrations" ("platforms_pos", "integrations_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "platforms_integrations_grants_namespaces" (
    "platforms_pos" INTEGER NOT NULL CHECK ("platforms_pos" >= 0),
    "integrations_pos" INTEGER NOT NULL CHECK ("integrations_pos" >= 0),
    "namespaces_key" TEXT NOT NULL,
    "all" INTEGER NOT NULL CHECK ("all" IN (0, 1)),
    PRIMARY KEY ("platforms_pos", "integrations_pos", "namespaces_key"),
    FOREIGN KEY ("platforms_pos", "integrations_pos") REFERENCES "platforms_integrations_grants" ("platforms_pos", "integrations_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "platforms_integrations_grants_namespaces_repos" (
    "platforms_pos" INTEGER NOT NULL CHECK ("platforms_pos" >= 0),
    "integrations_pos" INTEGER NOT NULL CHECK ("integrations_pos" >= 0),
    "namespaces_key" TEXT NOT NULL,
    "value" TEXT NOT NULL,
    PRIMARY KEY ("platforms_pos", "integrations_pos", "namespaces_key", "value"),
    FOREIGN KEY ("platforms_pos", "integrations_pos", "namespaces_key") REFERENCES "platforms_integrations_grants_namespaces" ("platforms_pos", "integrations_pos", "namespaces_key") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component intake
CREATE TABLE "intake" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "next" INTEGER NOT NULL CHECK ("next" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "intake_connections" (
    "intake_pos" INTEGER NOT NULL CHECK ("intake_pos" >= 0),
    "connections_pos" INTEGER NOT NULL CHECK ("connections_pos" >= 0),
    "source" TEXT NOT NULL,
    "enabled" INTEGER NOT NULL CHECK ("enabled" IN (0, 1)),
    "base_url" TEXT NOT NULL,
    "integration" TEXT NOT NULL,
    "cursor" TEXT NOT NULL,
    "polled_ms" INTEGER NOT NULL CHECK ("polled_ms" >= 0),
    "error" TEXT NOT NULL,
    "updated_ms" INTEGER NOT NULL CHECK ("updated_ms" >= 0),
    PRIMARY KEY ("intake_pos", "connections_pos"),
    FOREIGN KEY ("intake_pos") REFERENCES "intake" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "intake_items" (
    "intake_pos" INTEGER NOT NULL CHECK ("intake_pos" >= 0),
    "items_pos" INTEGER NOT NULL CHECK ("items_pos" >= 0),
    "id" TEXT NOT NULL,
    "source" TEXT NOT NULL,
    "external_id" TEXT NOT NULL,
    "key" TEXT NOT NULL,
    "scope" TEXT NOT NULL,
    "title" TEXT NOT NULL,
    "body" TEXT NOT NULL,
    "author" TEXT NOT NULL,
    "public" INTEGER NOT NULL CHECK ("public" IN (0, 1)),
    "status" TEXT NOT NULL,
    "project" TEXT NOT NULL,
    "recipe" TEXT NOT NULL,
    "task" TEXT NOT NULL,
    "change" TEXT NOT NULL,
    "error" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "updated_ms" INTEGER NOT NULL CHECK ("updated_ms" >= 0),
    PRIMARY KEY ("intake_pos", "items_pos"),
    FOREIGN KEY ("intake_pos") REFERENCES "intake" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "intake_items_labels" (
    "intake_pos" INTEGER NOT NULL CHECK ("intake_pos" >= 0),
    "items_pos" INTEGER NOT NULL CHECK ("items_pos" >= 0),
    "labels_pos" INTEGER NOT NULL CHECK ("labels_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("intake_pos", "items_pos", "labels_pos"),
    FOREIGN KEY ("intake_pos", "items_pos") REFERENCES "intake_items" ("intake_pos", "items_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "intake_items_links" (
    "intake_pos" INTEGER NOT NULL CHECK ("intake_pos" >= 0),
    "items_pos" INTEGER NOT NULL CHECK ("items_pos" >= 0),
    "links_pos" INTEGER NOT NULL CHECK ("links_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("intake_pos", "items_pos", "links_pos"),
    FOREIGN KEY ("intake_pos", "items_pos") REFERENCES "intake_items" ("intake_pos", "items_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "intake_items_writes" (
    "intake_pos" INTEGER NOT NULL CHECK ("intake_pos" >= 0),
    "items_pos" INTEGER NOT NULL CHECK ("items_pos" >= 0),
    "writes_pos" INTEGER NOT NULL CHECK ("writes_pos" >= 0),
    "kind" TEXT NOT NULL,
    "ok" INTEGER NOT NULL CHECK ("ok" IN (0, 1)),
    "detail" TEXT NOT NULL,
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    PRIMARY KEY ("intake_pos", "items_pos", "writes_pos"),
    FOREIGN KEY ("intake_pos", "items_pos") REFERENCES "intake_items" ("intake_pos", "items_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component hf_imports
CREATE TABLE "hf_imports" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "next" INTEGER NOT NULL CHECK ("next" >= 0),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "hf_import" (
    "hf_imports_pos" INTEGER NOT NULL CHECK ("hf_imports_pos" >= 0),
    "imports_pos" INTEGER NOT NULL CHECK ("imports_pos" >= 0),
    "n" INTEGER NOT NULL CHECK ("n" >= 0),
    "id" TEXT NOT NULL,
    "revision" TEXT NOT NULL,
    "sha" TEXT NOT NULL,
    "as_" TEXT NOT NULL,
    "state" TEXT NOT NULL,
    "dir" TEXT NOT NULL,
    "bytes" INTEGER NOT NULL CHECK ("bytes" >= 0),
    "total" INTEGER NOT NULL CHECK ("total" >= 0),
    "model_id" TEXT,
    "servable" INTEGER CHECK ("servable" IN (0, 1)),
    "reason" TEXT,
    "error" TEXT,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "updated_ms" INTEGER NOT NULL CHECK ("updated_ms" >= 0),
    PRIMARY KEY ("hf_imports_pos", "imports_pos"),
    FOREIGN KEY ("hf_imports_pos") REFERENCES "hf_imports" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "hf_import_file" (
    "hf_imports_pos" INTEGER NOT NULL CHECK ("hf_imports_pos" >= 0),
    "imports_pos" INTEGER NOT NULL CHECK ("imports_pos" >= 0),
    "files_pos" INTEGER NOT NULL CHECK ("files_pos" >= 0),
    "path" TEXT NOT NULL,
    "size" INTEGER NOT NULL CHECK ("size" >= 0),
    "sha256" TEXT,
    "bytes" INTEGER NOT NULL CHECK ("bytes" >= 0),
    "state" TEXT NOT NULL,
    PRIMARY KEY ("hf_imports_pos", "imports_pos", "files_pos"),
    FOREIGN KEY ("hf_imports_pos", "imports_pos") REFERENCES "hf_import" ("hf_imports_pos", "imports_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component egress_rules
CREATE TABLE "egress_rule" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "host" TEXT NOT NULL,
    "port" INTEGER NOT NULL CHECK ("port" >= 0),
    "path_prefix" TEXT NOT NULL,
    "header" TEXT NOT NULL,
    "format" TEXT NOT NULL,
    "secret" TEXT NOT NULL,
    "enabled" INTEGER NOT NULL CHECK ("enabled" IN (0, 1)),
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    PRIMARY KEY ("id")
) STRICT;
CREATE TABLE "egress_rule_roles" (
    "egress_rule_id" TEXT NOT NULL,
    "roles_pos" INTEGER NOT NULL CHECK ("roles_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("egress_rule_id", "roles_pos"),
    FOREIGN KEY ("egress_rule_id") REFERENCES "egress_rule" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "egress_rule_methods" (
    "egress_rule_id" TEXT NOT NULL,
    "methods_pos" INTEGER NOT NULL CHECK ("methods_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("egress_rule_id", "methods_pos"),
    FOREIGN KEY ("egress_rule_id") REFERENCES "egress_rule" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component e2b_sandboxes
CREATE TABLE "e2b_sandboxes" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "template" TEXT NOT NULL,
    "run" TEXT NOT NULL,
    "cid" INTEGER NOT NULL CHECK ("cid" >= 0),
    "started_ms" INTEGER NOT NULL CHECK ("started_ms" >= 0),
    "end_ms" INTEGER NOT NULL CHECK ("end_ms" >= 0),
    "phase" TEXT NOT NULL CHECK ("phase" IN ('running', 'paused')),
    "auto_pause" INTEGER NOT NULL CHECK ("auto_pause" IN (0, 1)),
    "allow_internet" INTEGER NOT NULL CHECK ("allow_internet" IN (0, 1)),
    "cold" INTEGER NOT NULL CHECK ("cold" IN (0, 1)),
    "access_token" TEXT,
    "retired" INTEGER NOT NULL CHECK ("retired" IN (0, 1)),
    PRIMARY KEY ("id")
) STRICT;
CREATE TABLE "e2b_sandbox_metadata" (
    "e2b_sandboxes_id" TEXT NOT NULL,
    "metadata_key" TEXT NOT NULL,
    "value" TEXT NOT NULL,
    PRIMARY KEY ("e2b_sandboxes_id", "metadata_key"),
    FOREIGN KEY ("e2b_sandboxes_id") REFERENCES "e2b_sandboxes" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "e2b_sandbox_env" (
    "e2b_sandboxes_id" TEXT NOT NULL,
    "env_vars_key" TEXT NOT NULL,
    "value" TEXT NOT NULL,
    PRIMARY KEY ("e2b_sandboxes_id", "env_vars_key"),
    FOREIGN KEY ("e2b_sandboxes_id") REFERENCES "e2b_sandboxes" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "e2b_sandbox_snapshot" (
    "e2b_sandboxes_id" TEXT NOT NULL,
    "snapshots_pos" INTEGER NOT NULL CHECK ("snapshots_pos" >= 0),
    "id" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "checkpoint" TEXT NOT NULL,
    "taken_ms" INTEGER NOT NULL CHECK ("taken_ms" >= 0),
    "memory" INTEGER NOT NULL CHECK ("memory" IN (0, 1)),
    PRIMARY KEY ("e2b_sandboxes_id", "snapshots_pos"),
    FOREIGN KEY ("e2b_sandboxes_id") REFERENCES "e2b_sandboxes" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "e2b_sandbox_allow_out" (
    "e2b_sandboxes_id" TEXT NOT NULL,
    "allow_out_pos" INTEGER NOT NULL CHECK ("allow_out_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("e2b_sandboxes_id", "allow_out_pos"),
    FOREIGN KEY ("e2b_sandboxes_id") REFERENCES "e2b_sandboxes" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "e2b_sandbox_deny_out" (
    "e2b_sandboxes_id" TEXT NOT NULL,
    "deny_out_pos" INTEGER NOT NULL CHECK ("deny_out_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("e2b_sandboxes_id", "deny_out_pos"),
    FOREIGN KEY ("e2b_sandboxes_id") REFERENCES "e2b_sandboxes" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component e2b_templates
CREATE TABLE "e2b_template" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "name" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "updated_ms" INTEGER NOT NULL CHECK ("updated_ms" >= 0),
    PRIMARY KEY ("id")
) STRICT;
CREATE TABLE "e2b_template_build" (
    "e2b_template_id" TEXT NOT NULL,
    "builds_pos" INTEGER NOT NULL CHECK ("builds_pos" >= 0),
    "id" TEXT NOT NULL,
    "status" TEXT NOT NULL CHECK ("status" IN ('waiting', 'building', 'ready', 'error')),
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "finished_ms" INTEGER CHECK ("finished_ms" >= 0),
    "vcpus" INTEGER NOT NULL CHECK ("vcpus" >= 0),
    "memory_mib" INTEGER NOT NULL CHECK ("memory_mib" >= 0),
    "snapshot" TEXT,
    "user" TEXT NOT NULL,
    "workdir" TEXT NOT NULL,
    "plan_hash" TEXT,
    PRIMARY KEY ("e2b_template_id", "builds_pos"),
    FOREIGN KEY ("e2b_template_id") REFERENCES "e2b_template" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "e2b_template_build_wanted_tag" (
    "e2b_template_id" TEXT NOT NULL,
    "builds_pos" INTEGER NOT NULL CHECK ("builds_pos" >= 0),
    "wanted_tags_pos" INTEGER NOT NULL CHECK ("wanted_tags_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("e2b_template_id", "builds_pos", "wanted_tags_pos"),
    FOREIGN KEY ("e2b_template_id", "builds_pos") REFERENCES "e2b_template_build" ("e2b_template_id", "builds_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "e2b_template_build_tag" (
    "e2b_template_id" TEXT NOT NULL,
    "builds_pos" INTEGER NOT NULL CHECK ("builds_pos" >= 0),
    "tags_pos" INTEGER NOT NULL CHECK ("tags_pos" >= 0),
    "tag" TEXT NOT NULL,
    "assigned_ms" INTEGER NOT NULL CHECK ("assigned_ms" >= 0),
    PRIMARY KEY ("e2b_template_id", "builds_pos", "tags_pos"),
    FOREIGN KEY ("e2b_template_id", "builds_pos") REFERENCES "e2b_template_build" ("e2b_template_id", "builds_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "e2b_template_build_reason" (
    "e2b_template_id" TEXT NOT NULL,
    "builds_pos" INTEGER NOT NULL CHECK ("builds_pos" >= 0),
    "message" TEXT NOT NULL,
    "step" TEXT NOT NULL,
    PRIMARY KEY ("e2b_template_id", "builds_pos"),
    FOREIGN KEY ("e2b_template_id", "builds_pos") REFERENCES "e2b_template_build" ("e2b_template_id", "builds_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component test_maps
CREATE TABLE "test_map" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "format" INTEGER NOT NULL CHECK ("format" >= 0),
    "project" TEXT NOT NULL,
    "commit" TEXT NOT NULL,
    "source" TEXT NOT NULL,
    "runner" TEXT NOT NULL,
    "recorded_ms" INTEGER NOT NULL CHECK ("recorded_ms" >= 0),
    PRIMARY KEY ("id")
) STRICT;
CREATE TABLE "test_map_package" (
    "test_map_id" TEXT NOT NULL,
    "packages_pos" INTEGER NOT NULL CHECK ("packages_pos" >= 0),
    "name" TEXT NOT NULL,
    "dir" TEXT NOT NULL,
    PRIMARY KEY ("test_map_id", "packages_pos"),
    FOREIGN KEY ("test_map_id") REFERENCES "test_map" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "test_map_dep" (
    "test_map_id" TEXT NOT NULL,
    "packages_pos" INTEGER NOT NULL CHECK ("packages_pos" >= 0),
    "deps_pos" INTEGER NOT NULL CHECK ("deps_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("test_map_id", "packages_pos", "deps_pos"),
    FOREIGN KEY ("test_map_id", "packages_pos") REFERENCES "test_map_package" ("test_map_id", "packages_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "test_map_test" (
    "test_map_id" TEXT NOT NULL,
    "tests_pos" INTEGER NOT NULL CHECK ("tests_pos" >= 0),
    "id" TEXT NOT NULL,
    "package" TEXT NOT NULL,
    "file" TEXT NOT NULL,
    "smoke" INTEGER NOT NULL CHECK ("smoke" IN (0, 1)),
    PRIMARY KEY ("test_map_id", "tests_pos"),
    FOREIGN KEY ("test_map_id") REFERENCES "test_map" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "test_map_definition" (
    "test_map_id" TEXT NOT NULL,
    "definitions_pos" INTEGER NOT NULL CHECK ("definitions_pos" >= 0),
    "file" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "start" INTEGER NOT NULL CHECK ("start" >= 0),
    "end" INTEGER NOT NULL CHECK ("end" >= 0),
    PRIMARY KEY ("test_map_id", "definitions_pos"),
    FOREIGN KEY ("test_map_id") REFERENCES "test_map" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "test_map_cover" (
    "test_map_id" TEXT NOT NULL,
    "definitions_pos" INTEGER NOT NULL CHECK ("definitions_pos" >= 0),
    "covered_by_pos" INTEGER NOT NULL CHECK ("covered_by_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("test_map_id", "definitions_pos", "covered_by_pos"),
    FOREIGN KEY ("test_map_id", "definitions_pos") REFERENCES "test_map_definition" ("test_map_id", "definitions_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "test_map_file" (
    "test_map_id" TEXT NOT NULL,
    "files_pos" INTEGER NOT NULL CHECK ("files_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("test_map_id", "files_pos"),
    FOREIGN KEY ("test_map_id") REFERENCES "test_map" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- component test_misses
CREATE TABLE "test_miss" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "project" TEXT NOT NULL,
    "commit" TEXT NOT NULL,
    "test" TEXT NOT NULL,
    "package" TEXT NOT NULL,
    "selection" TEXT NOT NULL,
    "recorded_ms" INTEGER NOT NULL CHECK ("recorded_ms" >= 0),
    PRIMARY KEY ("id")
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

-- vault (written through)
CREATE TABLE "vault" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "format" INTEGER NOT NULL CHECK ("format" >= 0),
    "vault_id" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "index_mac" TEXT NOT NULL,
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "vault_slot" (
    "vault_pos" INTEGER NOT NULL CHECK ("vault_pos" >= 0),
    "slots_pos" INTEGER NOT NULL CHECK ("slots_pos" >= 0),
    "id" TEXT NOT NULL,
    "kind" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "kdf_present" INTEGER NOT NULL CHECK ("kdf_present" IN (0, 1)),
    "kdf_alg" TEXT,
    "kdf_salt" TEXT,
    "kdf_memory_kib" INTEGER CHECK ("kdf_memory_kib" >= 0),
    "kdf_passes" INTEGER CHECK ("kdf_passes" >= 0),
    "kdf_lanes" INTEGER CHECK ("kdf_lanes" >= 0),
    "wrap_nonce" TEXT NOT NULL,
    "wrap_ct" TEXT NOT NULL,
    PRIMARY KEY ("vault_pos", "slots_pos"),
    FOREIGN KEY ("vault_pos") REFERENCES "vault" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "vault_entry" (
    "vault_pos" INTEGER NOT NULL CHECK ("vault_pos" >= 0),
    "entries_pos" INTEGER NOT NULL CHECK ("entries_pos" >= 0),
    "id" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "kind" TEXT NOT NULL,
    "label" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "updated_ms" INTEGER NOT NULL CHECK ("updated_ms" >= 0),
    "created_by" TEXT NOT NULL,
    "current" INTEGER NOT NULL CHECK ("current" >= 0),
    "last_used_ms" INTEGER NOT NULL CHECK ("last_used_ms" >= 0),
    "use_count" INTEGER NOT NULL CHECK ("use_count" >= 0),
    PRIMARY KEY ("vault_pos", "entries_pos"),
    FOREIGN KEY ("vault_pos") REFERENCES "vault" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "vault_entry_scope" (
    "vault_pos" INTEGER NOT NULL CHECK ("vault_pos" >= 0),
    "entries_pos" INTEGER NOT NULL CHECK ("entries_pos" >= 0),
    "kind" TEXT NOT NULL CHECK ("kind" IN ('plaza', 'nodes')),
    PRIMARY KEY ("vault_pos", "entries_pos"),
    FOREIGN KEY ("vault_pos", "entries_pos") REFERENCES "vault_entry" ("vault_pos", "entries_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "vault_scope_node" (
    "vault_pos" INTEGER NOT NULL CHECK ("vault_pos" >= 0),
    "entries_pos" INTEGER NOT NULL CHECK ("entries_pos" >= 0),
    "nodes_pos" INTEGER NOT NULL CHECK ("nodes_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("vault_pos", "entries_pos", "nodes_pos"),
    FOREIGN KEY ("vault_pos", "entries_pos") REFERENCES "vault_entry_scope" ("vault_pos", "entries_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "vault_entry_binding" (
    "vault_pos" INTEGER NOT NULL CHECK ("vault_pos" >= 0),
    "entries_pos" INTEGER NOT NULL CHECK ("entries_pos" >= 0),
    "bindings_pos" INTEGER NOT NULL CHECK ("bindings_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("vault_pos", "entries_pos", "bindings_pos"),
    FOREIGN KEY ("vault_pos", "entries_pos") REFERENCES "vault_entry" ("vault_pos", "entries_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "vault_version" (
    "vault_pos" INTEGER NOT NULL CHECK ("vault_pos" >= 0),
    "entries_pos" INTEGER NOT NULL CHECK ("entries_pos" >= 0),
    "versions_pos" INTEGER NOT NULL CHECK ("versions_pos" >= 0),
    "v" INTEGER NOT NULL CHECK ("v" >= 0),
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "nonce" TEXT NOT NULL,
    "ct" TEXT NOT NULL,
    "dek_nonce" TEXT NOT NULL,
    "dek_ct" TEXT NOT NULL,
    "fp" TEXT NOT NULL,
    "hint_nonce" TEXT,
    "hint_ct" TEXT,
    "legacy_hint" TEXT,
    PRIMARY KEY ("vault_pos", "entries_pos", "versions_pos"),
    FOREIGN KEY ("vault_pos", "entries_pos") REFERENCES "vault_entry" ("vault_pos", "entries_pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "vault_tombstone" (
    "vault_pos" INTEGER NOT NULL CHECK ("vault_pos" >= 0),
    "tombstones_pos" INTEGER NOT NULL CHECK ("tombstones_pos" >= 0),
    "id" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    PRIMARY KEY ("vault_pos", "tombstones_pos"),
    FOREIGN KEY ("vault_pos") REFERENCES "vault" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- vault audit (written through)
CREATE TABLE "vault_audit" (
    "seq" INTEGER NOT NULL CHECK ("seq" >= 0),
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "ts_ms" INTEGER NOT NULL CHECK ("ts_ms" >= 0),
    "prev" TEXT NOT NULL,
    "mac" TEXT NOT NULL,
    "event" TEXT NOT NULL,
    "actor" TEXT NOT NULL,
    "entry" TEXT,
    "name" TEXT,
    "binding" TEXT,
    "node" TEXT,
    "cid" INTEGER CHECK ("cid" >= 0),
    "detail" TEXT,
    PRIMARY KEY ("seq")
) STRICT;

-- usage calls (written through)
CREATE TABLE "usage_call" (
    "id" INTEGER NOT NULL CHECK ("id" >= 0),
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    "cid" INTEGER NOT NULL CHECK ("cid" >= 0),
    "role" TEXT NOT NULL,
    "model" TEXT NOT NULL,
    "provider" TEXT NOT NULL,
    "tokens_input" INTEGER NOT NULL CHECK ("tokens_input" >= 0),
    "tokens_output" INTEGER NOT NULL CHECK ("tokens_output" >= 0),
    "tokens_cache_read" INTEGER NOT NULL CHECK ("tokens_cache_read" >= 0),
    "tokens_cache_write" INTEGER NOT NULL CHECK ("tokens_cache_write" >= 0),
    "cost_usd" REAL,
    "est_usd" REAL NOT NULL,
    "task" TEXT,
    "work" INTEGER NOT NULL CHECK ("work" >= 0),
    "via" TEXT NOT NULL,
    "rung" TEXT,
    "escalated" INTEGER NOT NULL CHECK ("escalated" IN (0, 1)),
    PRIMARY KEY ("id")
) STRICT;
CREATE INDEX "usage_call_at_ms" ON "usage_call" ("at_ms");
CREATE INDEX "usage_call_task" ON "usage_call" ("task");

-- usage hours (written through)
CREATE TABLE "usage_hour" (
    "hour_ms" INTEGER NOT NULL CHECK ("hour_ms" >= 0),
    "model" TEXT NOT NULL,
    "cid" INTEGER NOT NULL CHECK ("cid" >= 0),
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "provider" TEXT NOT NULL,
    "role" TEXT NOT NULL,
    "totals_calls" INTEGER NOT NULL CHECK ("totals_calls" >= 0),
    "totals_tokens_input" INTEGER NOT NULL CHECK ("totals_tokens_input" >= 0),
    "totals_tokens_output" INTEGER NOT NULL CHECK ("totals_tokens_output" >= 0),
    "totals_tokens_cache_read" INTEGER NOT NULL CHECK ("totals_tokens_cache_read" >= 0),
    "totals_tokens_cache_write" INTEGER NOT NULL CHECK ("totals_tokens_cache_write" >= 0),
    "totals_cost_usd" REAL NOT NULL,
    "totals_est_usd" REAL NOT NULL,
    "totals_usd" REAL NOT NULL,
    PRIMARY KEY ("hour_ms", "model", "cid")
) STRICT;

-- usage works (written through)
CREATE TABLE "usage_work" (
    "cid" INTEGER NOT NULL CHECK ("cid" >= 0),
    "work" INTEGER NOT NULL CHECK ("work" >= 0),
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "totals_calls" INTEGER NOT NULL CHECK ("totals_calls" >= 0),
    "totals_tokens_input" INTEGER NOT NULL CHECK ("totals_tokens_input" >= 0),
    "totals_tokens_output" INTEGER NOT NULL CHECK ("totals_tokens_output" >= 0),
    "totals_tokens_cache_read" INTEGER NOT NULL CHECK ("totals_tokens_cache_read" >= 0),
    "totals_tokens_cache_write" INTEGER NOT NULL CHECK ("totals_tokens_cache_write" >= 0),
    "totals_cost_usd" REAL NOT NULL,
    "totals_est_usd" REAL NOT NULL,
    "totals_usd" REAL NOT NULL,
    PRIMARY KEY ("cid", "work")
) STRICT;
CREATE INDEX "usage_work_work" ON "usage_work" ("work");

-- ide audit (written through)
CREATE TABLE "ide_audit" (
    "seq" INTEGER NOT NULL CHECK ("seq" >= 0),
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    "actor" TEXT NOT NULL,
    "workspace" TEXT NOT NULL,
    "path" TEXT,
    "action_kind" TEXT NOT NULL CHECK ("action_kind" IN ('write', 'restore', 'run', 'run_stop', 'term_open', 'term_kill')),
    "action_write_hash_before" TEXT,
    "action_write_hash_after_content" TEXT,
    "action_write_checkpoint_content" TEXT,
    "action_write_change_content" TEXT,
    "action_restore_restored" INTEGER CHECK ("action_restore_restored" >= 0),
    "action_restore_checkpoint_content" TEXT,
    "action_run_run_content" TEXT,
    "action_run_command" TEXT,
    "action_run_timeout_s" INTEGER CHECK ("action_run_timeout_s" >= 0),
    "action_run_stop_run" TEXT,
    "action_term_open_term" TEXT,
    "action_term_open_cols" INTEGER CHECK ("action_term_open_cols" >= 0),
    "action_term_open_rows" INTEGER CHECK ("action_term_open_rows" >= 0),
    "action_term_open_cwd" TEXT,
    "action_term_kill_term" TEXT,
    PRIMARY KEY ("seq")
) STRICT;
CREATE INDEX "ide_audit_workspace_at_ms" ON "ide_audit" ("workspace", "at_ms");

-- station audit (written through)
CREATE TABLE "station_audit" (
    "seq" INTEGER NOT NULL CHECK ("seq" >= 0),
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    "actor" TEXT NOT NULL,
    "station" TEXT,
    "agent" INTEGER CHECK ("agent" >= 0),
    "event_kind" TEXT NOT NULL CHECK ("event_kind" IN ('code_issue', 'code_revoke', 'join', 'join_refused', 'lease_ended', 'revoke', 'drain', 'undrain', 'rename', 'settings', 'lease_open', 'lease_released', 'lease_granted', 'lease_failed', 'freeze', 'unfreeze', 'freeze_failed', 'owner_input', 'agent_op', 'takeover', 'give_back', 'recording_delete', 'recording_defaults', 'sync_conflict', 'sync_paused', 'sync_resumed', 'sync_resume', 'sync_put', 'sync_delete', 'sync_deliver', 'vm_reset', 'vm_reset_failed', 'vm_create', 'vm_remove', 'vm_sleep', 'vm_wake', 'owner_ide')),
    "event_code_issue_code" TEXT,
    "event_code_issue_label" TEXT,
    "event_code_issue_expires_ms" INTEGER CHECK ("event_code_issue_expires_ms" >= 0),
    "event_code_revoke_code" TEXT,
    "event_join_name" TEXT,
    "event_join_code" TEXT,
    "event_join_address" TEXT,
    "event_join_hostname" TEXT,
    "event_join_refused_address" TEXT,
    "event_lease_ended_lease" TEXT,
    "event_lease_ended_session" INTEGER CHECK ("event_lease_ended_session" >= 0),
    "event_lease_ended_reason" TEXT,
    "event_revoke_name" TEXT,
    "event_rename_name" TEXT,
    "event_settings_settings_takeover" TEXT CHECK ("event_settings_settings_takeover" IN ('agent_waits', 'release_lease', 'keep_working_pointer_paused')),
    "event_settings_settings_record" INTEGER CHECK ("event_settings_settings_record" IN (0, 1)),
    "event_settings_settings_retention_days" INTEGER CHECK ("event_settings_settings_retention_days" >= 0),
    "event_settings_settings_record_fps" INTEGER CHECK ("event_settings_settings_record_fps" >= 0),
    "event_settings_settings_record_takeovers" INTEGER CHECK ("event_settings_settings_record_takeovers" IN (0, 1)),
    "event_settings_settings_work_folder" TEXT,
    "event_settings_settings_snapshot_reset" INTEGER CHECK ("event_settings_settings_snapshot_reset" IN (0, 1)),
    "event_lease_open_lease" TEXT,
    "event_lease_open_lease_kind" TEXT CHECK ("event_lease_open_lease_kind" IN ('shared', 'pixel')),
    "event_lease_open_task" TEXT,
    "event_lease_released_lease" TEXT,
    "event_lease_released_session" INTEGER CHECK ("event_lease_released_session" >= 0),
    "event_lease_released_close_error" TEXT,
    "event_lease_released_reason" TEXT,
    "event_lease_granted_lease" TEXT,
    "event_lease_granted_session" INTEGER CHECK ("event_lease_granted_session" >= 0),
    "event_lease_failed_lease" TEXT,
    "event_lease_failed_reason" TEXT,
    "event_freeze_failed_reason" TEXT,
    "event_owner_input_lease" TEXT,
    "event_owner_input_session" INTEGER CHECK ("event_owner_input_session" >= 0),
    "event_owner_input_source" TEXT,
    "event_owner_input_action" TEXT,
    "event_agent_op_op_content" TEXT,
    "event_agent_op_lease" TEXT,
    "event_agent_op_ok" INTEGER CHECK ("event_agent_op_ok" IN (0, 1)),
    "event_agent_op_error" TEXT,
    "event_takeover_by" TEXT,
    "event_takeover_source" TEXT,
    "event_takeover_policy" TEXT CHECK ("event_takeover_policy" IN ('agent_waits', 'release_lease', 'keep_working_pointer_paused')),
    "event_takeover_session" INTEGER CHECK ("event_takeover_session" >= 0),
    "event_give_back_by" TEXT,
    "event_give_back_reason" TEXT,
    "event_give_back_session" INTEGER CHECK ("event_give_back_session" >= 0),
    "event_recording_delete_recording" TEXT,
    "event_recording_delete_lease" TEXT,
    "event_recording_delete_frames" INTEGER CHECK ("event_recording_delete_frames" >= 0),
    "event_recording_defaults_defaults_record" INTEGER CHECK ("event_recording_defaults_defaults_record" IN (0, 1)),
    "event_recording_defaults_defaults_retention_days" INTEGER CHECK ("event_recording_defaults_defaults_retention_days" >= 0),
    "event_recording_defaults_defaults_record_fps" INTEGER CHECK ("event_recording_defaults_defaults_record_fps" >= 0),
    "event_recording_defaults_defaults_record_takeovers" INTEGER CHECK ("event_recording_defaults_defaults_record_takeovers" IN (0, 1)),
    "event_sync_conflict_path" TEXT,
    "event_sync_conflict_copy" TEXT,
    "event_sync_paused_reason" TEXT,
    "event_sync_paused_deletions" INTEGER CHECK ("event_sync_paused_deletions" >= 0),
    "event_sync_resumed_by" TEXT,
    "event_sync_resume_apply_deletions" INTEGER CHECK ("event_sync_resume_apply_deletions" IN (0, 1)),
    "event_sync_resume_reason" TEXT,
    "event_sync_resume_deletions" INTEGER CHECK ("event_sync_resume_deletions" >= 0),
    "event_sync_put_path" TEXT,
    "event_sync_put_hash" TEXT,
    "event_sync_put_size" INTEGER CHECK ("event_sync_put_size" >= 0),
    "event_sync_delete_path" TEXT,
    "event_sync_deliver_path" TEXT,
    "event_sync_deliver_file" TEXT,
    "event_vm_reset_vm" TEXT,
    "event_vm_reset_failed_vm" TEXT,
    "event_vm_reset_failed_reason" TEXT,
    "event_vm_create_vm" TEXT,
    "event_vm_create_node" TEXT,
    "event_vm_create_node_vm" TEXT,
    "event_vm_create_os" TEXT,
    "event_vm_create_code" TEXT,
    "event_vm_remove_vm" TEXT,
    "event_vm_remove_node" TEXT,
    "event_vm_remove_node_vm" TEXT,
    "event_vm_remove_station" TEXT,
    "event_vm_sleep_vm" TEXT,
    "event_vm_sleep_node" TEXT,
    "event_vm_sleep_node_vm" TEXT,
    "event_vm_wake_vm" TEXT,
    "event_vm_wake_node" TEXT,
    "event_vm_wake_node_vm" TEXT,
    "event_owner_ide_action_kind" TEXT CHECK ("event_owner_ide_action_kind" IN ('write', 'run', 'run_stop', 'term_open', 'term_kill')),
    "event_owner_ide_action_write_path" TEXT,
    "event_owner_ide_action_write_hash_before" TEXT,
    "event_owner_ide_action_write_hash_after_content" TEXT,
    "event_owner_ide_action_run_run_content" TEXT,
    "event_owner_ide_action_run_command" TEXT,
    "event_owner_ide_action_run_timeout_s" INTEGER CHECK ("event_owner_ide_action_run_timeout_s" >= 0),
    "event_owner_ide_action_run_stop_run" TEXT,
    "event_owner_ide_action_term_open_term" TEXT,
    "event_owner_ide_action_term_open_cols" INTEGER CHECK ("event_owner_ide_action_term_open_cols" >= 0),
    "event_owner_ide_action_term_open_rows" INTEGER CHECK ("event_owner_ide_action_term_open_rows" >= 0),
    "event_owner_ide_action_term_open_cwd" TEXT,
    "event_owner_ide_action_term_kill_term" TEXT,
    PRIMARY KEY ("seq")
) STRICT;
CREATE INDEX "station_audit_station_at_ms" ON "station_audit" ("station", "at_ms");

-- station recordings (written through)
CREATE TABLE "station_recording" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "station" TEXT NOT NULL,
    "lease" TEXT NOT NULL,
    "agent" INTEGER NOT NULL CHECK ("agent" >= 0),
    "task" TEXT,
    "started_ms" INTEGER NOT NULL CHECK ("started_ms" >= 0),
    "ended_ms" INTEGER CHECK ("ended_ms" >= 0),
    "frames" INTEGER NOT NULL CHECK ("frames" >= 0),
    "actions" INTEGER NOT NULL CHECK ("actions" >= 0),
    "bytes" INTEGER NOT NULL CHECK ("bytes" >= 0),
    "takeover" INTEGER NOT NULL CHECK ("takeover" IN (0, 1)),
    "width" INTEGER CHECK ("width" >= 0),
    "height" INTEGER CHECK ("height" >= 0),
    PRIMARY KEY ("id")
) STRICT;
CREATE INDEX "station_recording_station_started_ms" ON "station_recording" ("station", "started_ms");

-- station recording events (written through)
CREATE TABLE "station_recording_event" (
    "recording" TEXT NOT NULL,
    "seq" INTEGER NOT NULL CHECK ("seq" >= 0),
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "event_at_ms" INTEGER NOT NULL CHECK ("event_at_ms" >= 0),
    "event_kind" TEXT NOT NULL,
    "event_ok" INTEGER NOT NULL CHECK ("event_ok" IN (0, 1)),
    "event_summary_content" TEXT,
    PRIMARY KEY ("recording", "seq")
) STRICT;

-- station recording frames (written through)
CREATE TABLE "station_recording_frame" (
    "recording" TEXT NOT NULL,
    "seq" INTEGER NOT NULL CHECK ("seq" >= 0),
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "ts_ms" INTEGER NOT NULL CHECK ("ts_ms" >= 0),
    "bytes" INTEGER NOT NULL CHECK ("bytes" >= 0),
    "hash" TEXT NOT NULL,
    PRIMARY KEY ("recording", "seq")
) STRICT;

-- station sync files (written through)
CREATE TABLE "station_sync_file" (
    "station" TEXT NOT NULL,
    "path" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "hash" TEXT,
    "size" INTEGER NOT NULL CHECK ("size" >= 0),
    "mtime_ms" INTEGER NOT NULL CHECK ("mtime_ms" >= 0),
    "version" INTEGER NOT NULL CHECK ("version" >= 0),
    "base" TEXT,
    "conflict_of" TEXT,
    "synced_ms" INTEGER NOT NULL CHECK ("synced_ms" >= 0),
    PRIMARY KEY ("station", "path")
) STRICT;

-- eval runs (written through)
CREATE TABLE "eval_run" (
    "id" INTEGER NOT NULL CHECK ("id" >= 0),
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "session" TEXT NOT NULL,
    "project" TEXT NOT NULL,
    "task" TEXT,
    "recipe" TEXT NOT NULL,
    "step" TEXT NOT NULL,
    "seat" TEXT NOT NULL,
    "round" INTEGER NOT NULL CHECK ("round" >= 0),
    "engine" TEXT NOT NULL,
    "engine_version" TEXT NOT NULL,
    "model" TEXT,
    "provider" TEXT,
    "harness" TEXT NOT NULL,
    "node" TEXT,
    "pool" TEXT,
    "tokens_in" INTEGER NOT NULL CHECK ("tokens_in" >= 0),
    "tokens_out" INTEGER NOT NULL CHECK ("tokens_out" >= 0),
    "tokens_cached" INTEGER NOT NULL CHECK ("tokens_cached" >= 0),
    "cost_micros" INTEGER NOT NULL CHECK ("cost_micros" >= 0),
    "model_calls" INTEGER NOT NULL CHECK ("model_calls" >= 0),
    "tool_calls" INTEGER NOT NULL CHECK ("tool_calls" >= 0),
    "tool_ms" INTEGER NOT NULL CHECK ("tool_ms" >= 0),
    "wall_ms" INTEGER NOT NULL CHECK ("wall_ms" >= 0),
    "verdict" INTEGER CHECK ("verdict" IN (0, 1)),
    "outcome" TEXT NOT NULL,
    "ok" INTEGER NOT NULL CHECK ("ok" IN (0, 1)),
    "landed" INTEGER CHECK ("landed" IN (0, 1)),
    "reverted" INTEGER CHECK ("reverted" IN (0, 1)),
    "ended_ms" INTEGER NOT NULL CHECK ("ended_ms" >= 0),
    PRIMARY KEY ("id")
) STRICT;
CREATE INDEX "eval_run_project" ON "eval_run" ("project");
CREATE INDEX "eval_run_task" ON "eval_run" ("task");
CREATE INDEX "eval_run_seat" ON "eval_run" ("seat");
CREATE INDEX "eval_run_model" ON "eval_run" ("model");
CREATE INDEX "eval_run_ended_ms" ON "eval_run" ("ended_ms");

-- unfurl cache (written through)
CREATE TABLE "unfurl_cache" (
    "key" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "ok" INTEGER NOT NULL CHECK ("ok" IN (0, 1)),
    "card_content" TEXT,
    "error" TEXT,
    "fetched_ms" INTEGER NOT NULL CHECK ("fetched_ms" >= 0),
    "ttl_ms" INTEGER NOT NULL CHECK ("ttl_ms" >= 0),
    PRIMARY KEY ("key")
) STRICT;

-- prompt (written through)
CREATE TABLE "prompt" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "kind" TEXT NOT NULL CHECK ("kind" IN ('system', 'agents_md', 'fragment')),
    "title" TEXT NOT NULL,
    "description" TEXT NOT NULL,
    "binding" TEXT,
    "active_version" INTEGER CHECK ("active_version" >= 0),
    "latest_version" INTEGER NOT NULL CHECK ("latest_version" >= 0),
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "updated_ms" INTEGER NOT NULL CHECK ("updated_ms" >= 0),
    "archived_ms" INTEGER CHECK ("archived_ms" >= 0),
    PRIMARY KEY ("id")
) STRICT;
CREATE TABLE "prompt_tags" (
    "prompt_id" TEXT NOT NULL,
    "tags_pos" INTEGER NOT NULL CHECK ("tags_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("prompt_id", "tags_pos"),
    FOREIGN KEY ("prompt_id") REFERENCES "prompt" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- prompt version (written through)
CREATE TABLE "prompt_version" (
    "prompt_id" TEXT NOT NULL,
    "version" INTEGER NOT NULL CHECK ("version" >= 0),
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "note" TEXT NOT NULL,
    "author" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "score" INTEGER NOT NULL CHECK ("score" >= 0),
    "errors" INTEGER NOT NULL CHECK ("errors" >= 0),
    "warnings" INTEGER NOT NULL CHECK ("warnings" >= 0),
    "infos" INTEGER NOT NULL CHECK ("infos" >= 0),
    "rules_version" INTEGER NOT NULL CHECK ("rules_version" >= 0),
    "sha256" TEXT NOT NULL,
    "body" TEXT NOT NULL,
    PRIMARY KEY ("prompt_id", "version")
) STRICT;
CREATE TABLE "prompt_version_findings" (
    "prompt_version_prompt_id" TEXT NOT NULL,
    "prompt_version_version" INTEGER NOT NULL CHECK ("prompt_version_version" >= 0),
    "findings_pos" INTEGER NOT NULL CHECK ("findings_pos" >= 0),
    "rule" TEXT NOT NULL,
    "severity" TEXT NOT NULL CHECK ("severity" IN ('error', 'warning', 'info')),
    "line" INTEGER NOT NULL CHECK ("line" >= 0),
    "message" TEXT NOT NULL,
    PRIMARY KEY ("prompt_version_prompt_id", "prompt_version_version", "findings_pos"),
    FOREIGN KEY ("prompt_version_prompt_id", "prompt_version_version") REFERENCES "prompt_version" ("prompt_id", "version") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- prompt event (written through)
CREATE TABLE "prompt_event" (
    "id" INTEGER NOT NULL CHECK ("id" >= 0),
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "prompt_id" TEXT NOT NULL,
    "action" TEXT NOT NULL CHECK ("action" IN ('create', 'version', 'publish', 'unpublish', 'edit', 'archive', 'restore')),
    "version" INTEGER CHECK ("version" >= 0),
    "actor" TEXT NOT NULL,
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    PRIMARY KEY ("id")
) STRICT;
CREATE INDEX "prompt_event_prompt_id" ON "prompt_event" ("prompt_id");

-- hub mcp token (written through)
CREATE TABLE "hub_mcp_token" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "name" TEXT NOT NULL,
    "scope" TEXT NOT NULL CHECK ("scope" IN ('observe', 'operate', 'govern')),
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "expires_ms" INTEGER CHECK ("expires_ms" >= 0),
    "last_used_ms" INTEGER CHECK ("last_used_ms" >= 0),
    "calls" INTEGER NOT NULL CHECK ("calls" >= 0),
    "revoked_at_ms" INTEGER CHECK ("revoked_at_ms" >= 0),
    PRIMARY KEY ("id")
) STRICT;
CREATE TABLE "hub_mcp_token_allowlist" (
    "hub_mcp_token_id" TEXT NOT NULL,
    "allowlist_pos" INTEGER NOT NULL CHECK ("allowlist_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("hub_mcp_token_id", "allowlist_pos"),
    FOREIGN KEY ("hub_mcp_token_id") REFERENCES "hub_mcp_token" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- hub mcp token digest (written through)
CREATE TABLE "hub_mcp_token_digest" (
    "token_id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "digest" BLOB NOT NULL,
    PRIMARY KEY ("token_id")
) STRICT;
CREATE UNIQUE INDEX "hub_mcp_token_digest_digest" ON "hub_mcp_token_digest" ("digest");

-- hub mcp call (written through)
CREATE TABLE "hub_mcp_call" (
    "seq" INTEGER NOT NULL CHECK ("seq" >= 0),
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "call_at_ms" INTEGER NOT NULL CHECK ("call_at_ms" >= 0),
    "call_token_id" TEXT,
    "call_client_ip" TEXT,
    "call_tool" TEXT NOT NULL,
    "call_args_sha256" TEXT NOT NULL,
    "call_ok" INTEGER NOT NULL CHECK ("call_ok" IN (0, 1)),
    "call_error" TEXT,
    "call_duration_ms" INTEGER NOT NULL CHECK ("call_duration_ms" >= 0),
    PRIMARY KEY ("seq")
) STRICT;

-- hub mcp settings (written through)
CREATE TABLE "hub_mcp_settings" (
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "enabled" INTEGER NOT NULL CHECK ("enabled" IN (0, 1)),
    "bind" TEXT NOT NULL CHECK ("bind" IN ('local', 'remote')),
    "auth_mode" TEXT NOT NULL CHECK ("auth_mode" IN ('secure', 'open')),
    "open_scope" TEXT NOT NULL CHECK ("open_scope" IN ('observe', 'operate', 'govern')),
    PRIMARY KEY ("pos")
) STRICT;
CREATE TABLE "hub_mcp_settings_allowed_origins" (
    "hub_mcp_settings_pos" INTEGER NOT NULL CHECK ("hub_mcp_settings_pos" >= 0),
    "allowed_origins_pos" INTEGER NOT NULL CHECK ("allowed_origins_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("hub_mcp_settings_pos", "allowed_origins_pos"),
    FOREIGN KEY ("hub_mcp_settings_pos") REFERENCES "hub_mcp_settings" ("pos") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- share (written through)
CREATE TABLE "share" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "target_kind" TEXT NOT NULL,
    "target_id" TEXT NOT NULL,
    "version" INTEGER CHECK ("version" >= 0),
    "password_hash" TEXT,
    "expires_ms" INTEGER CHECK ("expires_ms" >= 0),
    "allow_download" INTEGER NOT NULL CHECK ("allow_download" IN (0, 1)),
    "created_by" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "revoked_ms" INTEGER CHECK ("revoked_ms" >= 0),
    "title" TEXT NOT NULL,
    PRIMARY KEY ("id")
) STRICT;

-- share event (written through)
CREATE TABLE "share_event" (
    "seq" INTEGER NOT NULL CHECK ("seq" >= 0),
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "share" TEXT NOT NULL,
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    "kind" TEXT NOT NULL,
    "via" TEXT NOT NULL,
    PRIMARY KEY ("seq")
) STRICT;
CREATE INDEX "share_event_share_at_ms" ON "share_event" ("share", "at_ms");

-- skill (written through)
CREATE TABLE "skill" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "name" TEXT NOT NULL,
    "title" TEXT NOT NULL,
    "description" TEXT NOT NULL,
    "active_version" INTEGER CHECK ("active_version" >= 0),
    "latest_version" INTEGER NOT NULL CHECK ("latest_version" >= 0),
    "source_kind" TEXT NOT NULL CHECK ("source_kind" IN ('built', 'upload', 'github')),
    "source_github_repo" TEXT,
    "source_github_git_ref" TEXT,
    "source_github_path" TEXT,
    "source_github_commit" TEXT,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "updated_ms" INTEGER NOT NULL CHECK ("updated_ms" >= 0),
    "archived_ms" INTEGER CHECK ("archived_ms" >= 0),
    PRIMARY KEY ("id")
) STRICT;
CREATE TABLE "skill_tags" (
    "skill_id" TEXT NOT NULL,
    "tags_pos" INTEGER NOT NULL CHECK ("tags_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("skill_id", "tags_pos"),
    FOREIGN KEY ("skill_id") REFERENCES "skill" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "skill_bindings" (
    "skill_id" TEXT NOT NULL,
    "bindings_pos" INTEGER NOT NULL CHECK ("bindings_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("skill_id", "bindings_pos"),
    FOREIGN KEY ("skill_id") REFERENCES "skill" ("id") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- skill version (written through)
CREATE TABLE "skill_version" (
    "skill_id" TEXT NOT NULL,
    "version" INTEGER NOT NULL CHECK ("version" >= 0),
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "note" TEXT NOT NULL,
    "author" TEXT NOT NULL,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "score" INTEGER NOT NULL CHECK ("score" >= 0),
    "errors" INTEGER NOT NULL CHECK ("errors" >= 0),
    "warnings" INTEGER NOT NULL CHECK ("warnings" >= 0),
    "infos" INTEGER NOT NULL CHECK ("infos" >= 0),
    "rules_version" INTEGER NOT NULL CHECK ("rules_version" >= 0),
    "hash" TEXT NOT NULL,
    "file_count" INTEGER NOT NULL CHECK ("file_count" >= 0),
    "total_bytes" INTEGER NOT NULL CHECK ("total_bytes" >= 0),
    "source_commit" TEXT,
    "skill_md" TEXT NOT NULL,
    PRIMARY KEY ("skill_id", "version")
) STRICT;

-- skill file (written through)
CREATE TABLE "skill_file" (
    "skill_id" TEXT NOT NULL,
    "version" INTEGER NOT NULL CHECK ("version" >= 0),
    "path" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "hash" TEXT NOT NULL,
    "size" INTEGER NOT NULL CHECK ("size" >= 0),
    "executable" INTEGER NOT NULL CHECK ("executable" IN (0, 1)),
    PRIMARY KEY ("skill_id", "version", "path")
) STRICT;

-- skill finding (written through)
CREATE TABLE "skill_finding" (
    "skill_id" TEXT NOT NULL,
    "version" INTEGER NOT NULL CHECK ("version" >= 0),
    "seq" INTEGER NOT NULL CHECK ("seq" >= 0),
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "finding_rule" TEXT NOT NULL,
    "finding_severity" TEXT NOT NULL CHECK ("finding_severity" IN ('error', 'warning', 'info')),
    "finding_path" TEXT NOT NULL,
    "finding_line" INTEGER NOT NULL CHECK ("finding_line" >= 0),
    "finding_message" TEXT NOT NULL,
    PRIMARY KEY ("skill_id", "version", "seq")
) STRICT;

-- skill event (written through)
CREATE TABLE "skill_event" (
    "id" INTEGER NOT NULL CHECK ("id" >= 0),
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "skill_id" TEXT NOT NULL,
    "action" TEXT NOT NULL CHECK ("action" IN ('create', 'version', 'publish', 'unpublish', 'edit', 'bind', 'archive', 'restore')),
    "version" INTEGER CHECK ("version" >= 0),
    "actor" TEXT NOT NULL,
    "at_ms" INTEGER NOT NULL CHECK ("at_ms" >= 0),
    PRIMARY KEY ("id")
) STRICT;

-- artifact (written through)
CREATE TABLE "artifact" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "author_cid" INTEGER CHECK ("author_cid" >= 0),
    "author" TEXT NOT NULL,
    "project" TEXT,
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "updated_ms" INTEGER NOT NULL CHECK ("updated_ms" >= 0),
    "shared" INTEGER NOT NULL CHECK ("shared" IN (0, 1)),
    PRIMARY KEY ("id")
) STRICT;

-- artifact version (written through)
CREATE TABLE "artifact_version" (
    "artifact_id" TEXT NOT NULL,
    "version" INTEGER NOT NULL CHECK ("version" >= 0),
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "title" TEXT NOT NULL,
    "kind" TEXT NOT NULL,
    "spec_bytes" INTEGER NOT NULL CHECK ("spec_bytes" >= 0),
    "bytes" INTEGER NOT NULL CHECK ("bytes" >= 0),
    "spec_hash" TEXT NOT NULL,
    PRIMARY KEY ("artifact_id", "version")
) STRICT;
CREATE TABLE "artifact_version_blobs" (
    "artifact_version_artifact_id" TEXT NOT NULL,
    "artifact_version_version" INTEGER NOT NULL CHECK ("artifact_version_version" >= 0),
    "blobs_pos" INTEGER NOT NULL CHECK ("blobs_pos" >= 0),
    "value" TEXT NOT NULL,
    PRIMARY KEY ("artifact_version_artifact_id", "artifact_version_version", "blobs_pos"),
    FOREIGN KEY ("artifact_version_artifact_id", "artifact_version_version") REFERENCES "artifact_version" ("artifact_id", "version") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE "artifact_version_scripts" (
    "artifact_version_artifact_id" TEXT NOT NULL,
    "artifact_version_version" INTEGER NOT NULL CHECK ("artifact_version_version" >= 0),
    "scripts_pos" INTEGER NOT NULL CHECK ("scripts_pos" >= 0),
    "panel" TEXT NOT NULL,
    "path" TEXT NOT NULL,
    "hash" TEXT NOT NULL,
    PRIMARY KEY ("artifact_version_artifact_id", "artifact_version_version", "scripts_pos"),
    FOREIGN KEY ("artifact_version_artifact_id", "artifact_version_version") REFERENCES "artifact_version" ("artifact_id", "version") ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

-- artifact blob (written through)
CREATE TABLE "artifact_blob" (
    "id" TEXT NOT NULL,
    "pos" INTEGER NOT NULL CHECK ("pos" >= 0),
    "cid" INTEGER CHECK ("cid" >= 0),
    "content_type" TEXT NOT NULL,
    "bytes" INTEGER NOT NULL CHECK ("bytes" >= 0),
    "created_ms" INTEGER NOT NULL CHECK ("created_ms" >= 0),
    "hash" TEXT NOT NULL,
    PRIMARY KEY ("id")
) STRICT;

