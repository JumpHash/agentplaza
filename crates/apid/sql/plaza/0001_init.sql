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

