-- Local provider transports: expand the provider-kind check and add typed process options.
-- Preserve version 1 verbatim: existing hubs verify its recorded hash before upgrading.
-- SQLite cannot alter a CHECK constraint. Copy the provider subtree, rebuild it with the
-- generated definitions, then restore every row under the same foreign keys. All of this
-- runs in the store's migration transaction; foreign-key enforcement remains enabled.

CREATE TEMP TABLE "_local_upgrade_provider" AS SELECT * FROM "provider";
CREATE TEMP TABLE "_local_upgrade_provider_extra_headers" AS SELECT * FROM "provider_extra_headers";
CREATE TEMP TABLE "_local_upgrade_provider_models" AS SELECT * FROM "provider_models";
CREATE TEMP TABLE "_local_upgrade_provider_options" AS SELECT * FROM "provider_options";
CREATE TEMP TABLE "_local_upgrade_provider_options_trains_on_data" AS SELECT * FROM "provider_options_trains_on_data";

DROP TABLE "provider_options_trains_on_data";
DROP TABLE "provider_options";
DROP TABLE "provider_models";
DROP TABLE "provider_extra_headers";
DROP TABLE "provider";

CREATE TABLE "provider" (
    "providers_pos" INTEGER NOT NULL CHECK ("providers_pos" >= 0),
    "providers_pos_2" INTEGER NOT NULL CHECK ("providers_pos_2" >= 0),
    "id" TEXT NOT NULL,
    "kind" TEXT NOT NULL CHECK ("kind" IN ('anthropic', 'openai', 'openai-responses', 'codex-app-server', 'opencode-local', 'azure-openai', 'gemini', 'bedrock', 'vertex-anthropic', 'vertex-gemini', 'openai-compatible', 'openai-decisions', 'systemone', 'azure-decisions', 'mock')),
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
    "executable" TEXT,
    "working_directory" TEXT,
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

INSERT INTO "provider" ("providers_pos", "providers_pos_2", "id", "kind", "label", "accent", "enabled", "base_url", "auth_kind", "auth_env", "auth_secret", "auth_command", "extra_body_content")
SELECT "providers_pos", "providers_pos_2", "id", "kind", "label", "accent", "enabled", "base_url", "auth_kind", "auth_env", "auth_secret", "auth_command", "extra_body_content" FROM "_local_upgrade_provider";
INSERT INTO "provider_extra_headers" ("providers_pos", "providers_pos_2", "extra_headers_key", "value")
SELECT "providers_pos", "providers_pos_2", "extra_headers_key", "value" FROM "_local_upgrade_provider_extra_headers";
INSERT INTO "provider_models" ("providers_pos", "providers_pos_2", "models_pos", "value")
SELECT "providers_pos", "providers_pos_2", "models_pos", "value" FROM "_local_upgrade_provider_models";
INSERT INTO "provider_options" ("providers_pos", "providers_pos_2", "region", "project", "location", "path", "max_tokens_field", "models_cache_secs", "websocket", "replay_reasoning", "mistral_compat", "reasoning_effort", "first_party", "refusal_fallbacks")
SELECT "providers_pos", "providers_pos_2", "region", "project", "location", "path", "max_tokens_field", "models_cache_secs", "websocket", "replay_reasoning", "mistral_compat", "reasoning_effort", "first_party", "refusal_fallbacks" FROM "_local_upgrade_provider_options";
INSERT INTO "provider_options_trains_on_data" ("providers_pos", "providers_pos_2", "trains_on_data_pos", "value")
SELECT "providers_pos", "providers_pos_2", "trains_on_data_pos", "value" FROM "_local_upgrade_provider_options_trains_on_data";

DROP TABLE "_local_upgrade_provider_options_trains_on_data";
DROP TABLE "_local_upgrade_provider_options";
DROP TABLE "_local_upgrade_provider_models";
DROP TABLE "_local_upgrade_provider_extra_headers";
DROP TABLE "_local_upgrade_provider";
