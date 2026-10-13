# Skill library

Owner's request (2026-10-14): a skill library, like the prompt library. Skills can be imported, exported, built in
the app, and imported from GitHub.

## What a skill is

A skill is a folder in the open SKILL.md format (the format Claude's Agent Skills use, so existing collections import
unchanged):

```
pdf-forms/
  SKILL.md           frontmatter (name, description, optional license, allowed-tools, metadata) and the body
  scripts/fill.py    optional: code the agent may run in its own VM
  reference.md       optional: more instructions the body links to
  assets/...         optional: templates, data
```

```markdown
---
name: pdf-forms
description: Fill and flatten PDF forms. Use when the task involves a PDF with form fields.
---
# PDF forms
…
```

**How agents use a skill (progressive disclosure):**
- An agent's system prompt lists the skills available to it under `## Skills`, by name and description only.
- When a task matches one, the agent calls `plaza_skill_read` for the body, then for any file it needs.
- Scripts run only inside the agent's own VM, which the agent writes them into. Nothing from a skill runs on the hub
  or the host.

**Which skills an agent gets (bindings):**
- `role:<role>`: the agents of that role.
- `plaza`: every agent.
- None: the skill stays in the library but is not handed to any agent.

A skill reaches agents only once it is published. A role can hold several skills, unlike a prompt binding.

## Library model (as the prompt library)

- **A skill** has:
  - an id (a slug);
  - the frontmatter name;
  - a title, description and tags;
  - its bindings;
  - the live version;
  - its source: `built` in the app, `upload`, or `github`. A GitHub source keeps the repo, the ref, the path and the
    commit it was taken from.
- **A version** is immutable: the SKILL.md text, the files (path, sha256, size, executable), a note, the author, and
  the check result.
- **Publishing** points the skill at a version. A version with an error-level finding cannot be published.
- **History:** every create, version, publish, unpublish, edit, archive and restore is an event row.

## Checks (`crates/apz-skill`)

`check(skill_md, files) -> Report`. It returns the same Report shape as `apz-prompt`, so the same score, the same best
version and the same UI apply. The body also goes through `apz_prompt::check(Kind::Fragment, body)`, which brings
secrets, fences, outline and vague wording. Skill rules:

| Rule | Severity | What it catches |
|---|---|---|
| `frontmatter` | error | no `---` frontmatter, or YAML that does not parse |
| `name-format` | error | the name is missing, longer than 64 characters, not lower-case letters, digits and hyphens, or contains "anthropic" or "claude" |
| `description-missing` | error | the description is missing or empty |
| `description-length` | error | the description is longer than 1024 characters |
| `description-when` | warning | the description does not say when to use the skill ("Use when …") |
| `body-long` | warning | the body is over 500 lines; move detail into referenced files |
| `missing-file` | error | the body links to a file the skill does not contain |
| `unused-file` | info | a file nothing in the body or the other text files mentions |
| `script` | warning | the skill contains code (a shebang, an executable bit, or `.py`, `.sh`, `.js`, `.ts`, `.rb` and similar): agents may run it in their VMs; review it before publishing |
| `large-file` | warning | a file over 1 MB |
| `binary-file` | info | a file that is not UTF-8 text (shown, not previewed) |
| `size-limit` | error | more than 200 files, a file over 5 MB, or more than 20 MB in all |

`template()` gives a starter SKILL.md that scores 100. `parse(skill_md)` returns the frontmatter fields and the body.

## Files and storage (migration 0007)

```
skills           id, name, title, description, tags[], active_version, latest_version, source_kind, source_repo,
                 source_ref, source_path, source_commit, created, updated, archived_at
skill_bindings   (skill_id, binding) PK
skill_versions   (skill_id, version) PK, skill_md, sha256, note, author, score, errors, warnings, infos, rules_version,
                 file_count, total_bytes, created                                           append-only
skill_files      (skill_id, version, path) PK, sha256, size, executable                     append-only
skill_findings   (skill_id, version, seq) PK, rule, severity, path, line, message           append-only
skill_events     id, skill_id, action, version, actor, at                                   append-only
```

**File contents** are blobs and stay out of Postgres (plaza rule). They are content-addressed by sha256:
- in Garage, with the same client the Library uses;
- without Garage, under `<state_dir>/skills/blobs/<sha256>`.

Without Postgres, `MemorySkills` keeps the rows in `<state_dir>/skills.json`, as `MemoryPrompts` does.

## Import, export, GitHub

**Import a file** (`.zip` or `.skill`):
- `POST /v1/skills/import/preview` with the zip bytes. The hub unpacks the zip in memory within the size limits, finds
  every folder with a SKILL.md, checks each one, and returns a preview: an id, plus per skill its path, name and
  report.
- `POST /v1/skills/import/commit` with `{preview_id, picks: [path…], binding?}` creates the picked skills, unpublished,
  with the source `upload`. A preview lives 15 minutes.

**Import from GitHub:**
- `POST /v1/skills/github/preview {url}` takes `https://github.com/<owner>/<repo>` with an optional
  `/tree/<ref>/<path>`.
- The hub resolves the ref to a commit (the GitHub API), downloads that commit's zipball within the same limits, and
  returns the same preview.
- A token improves the rate limit and opens private repos. If the vault holds a `github_token` secret, the hub sends it;
  the token never reaches a client.
- `POST /v1/skills/{id}/github/check` says whether the ref now points at a newer commit.
  `POST /v1/skills/{id}/github/update` imports that commit as a new version. Publishing it stays the owner's decision.

**Export:**
- `GET /v1/skills/{id}/versions/{v}/export` returns a zip with the skill's folder.
- `GET /v1/skills/export?ids=a,b` returns several skills in one zip.

**Desktop:** in addition to the zip upload, a folder on disk can be imported. The desktop zips the folder and sends it.

**Safety:** imported content is data, never instructions.
- Nothing in a skill runs on the hub.
- Scripts are flagged (`script` rule) and shown before publishing.
- Secrets block publishing (`secret` rule).
- Zip entries with `..`, absolute paths or links are refused.

## API (apid, owner only)

The prompt library's routes, with skills:
- list, create, get, meta, versions, version, publish, unpublish, archive, restore, diff, history, check, rules;
- the import, GitHub and export routes above.

Two routes differ from the prompt library:
- `GET /v1/skills/{id}/versions/{v}/files/{path}` returns one file.
- `POST /v1/skills/{id}/versions` takes `{skill_md, files: [{path, content_b64 | sha256, executable}], base_version,
  note}`. An unchanged file is sent by its sha256 alone.

## Agents and the hub MCP server

**Agent tools:**
- `plaza_skills` lists the agent's skills (name, description).
- `plaza_skill_read {name, path?}` returns the SKILL.md, or one file.

The tools return text. A binary file comes back as base64 with its size.

**Hub MCP tools:**
- `skills_list` and `skill_get` (observe);
- `skill_propose` (operate): adds a version and never publishes;
- `skill_publish` (govern).

## Screens (web and desktop, 1:1)

- **Skills** (under Agents, next to Prompts):
  - the list, with filters by binding, source and tags, and a search;
  - a score badge;
  - a source chip, with GitHub's repo@commit;
  - New skill (from the template), Import (a zip, plus a folder on the desktop), From GitHub (a URL, then the preview
    with checks, then pick and import), Export selected.
- **Skill editor:**
  - the file tree with add, rename and remove;
  - SKILL.md and text files in an editor;
  - the markdown preview;
  - findings as you type, each one jumping to its file and line;
  - Save version;
  - Publish and Unpublish;
  - Bindings: roles, and every agent;
  - Export;
  - for GitHub skills, Check for updates and Update.
- **Versions, compare, history, rules:** as in the prompt editor. Compare diffs SKILL.md and lists the added, removed
  and changed files.
