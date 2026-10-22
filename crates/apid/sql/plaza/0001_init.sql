-- plaza.db, schema 1 (the clean break, W3): the hub's state of record. Every table is STRICT with typed columns and no
-- JSON (W69). The components' tables below are generated from their definitions (derive(apz_store::Table)): print
-- them with `cargo run -p apid -- schema`; the store's tests check that this file matches.

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

