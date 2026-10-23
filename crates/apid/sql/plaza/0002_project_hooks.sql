-- plaza.db, migration 2 (Engine 1.7): a project's hooks (apz_proto::hooks), and whether it starts from the defaults.
-- The tables are the settings components' (project_settings, and the drafts' settings that become a project's): the
-- store's tests check that the migrated file matches their definitions (`apid schema` prints them). Each
-- `hook_defaults` is its table's last column, where the definition puts it, so the migration adds it at the end.

ALTER TABLE "project_setting" ADD COLUMN "hook_defaults" INTEGER NOT NULL DEFAULT 0 CHECK ("hook_defaults" IN (0, 1));
ALTER TABLE "chat_draft_settings" ADD COLUMN "hook_defaults" INTEGER NOT NULL DEFAULT 0 CHECK ("hook_defaults" IN (0, 1));

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
