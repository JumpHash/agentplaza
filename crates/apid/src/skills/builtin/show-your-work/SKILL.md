---
name: show-your-work
description: Make an artifact, a titled page of charts, tables, graphs, documents, code, images or a small web page, to show results, comparisons, measurements, dependency graphs or small dashboards. Use artifact_create, artifact_update and artifact_attach_data.
---

# Show your work

When a result is easier to see than to read, make an artifact and reference it in your reply. The owner and other agents open it from the chat.

## When to make one

- Results of a job: numbers over time, before and after comparisons.
- Comparisons between options: a table or bars.
- Measurements: tiles with sparklines and a status.
- Dependency or flow graphs: a graph body.
- Small dashboards: several panels on one page.

Do not make one for a short answer or a single number that fits in a sentence. When the owner asked for a file, use deliver_file.

## How

1. Write the spec: a `title`, an optional `sub`, and `panels`. Each panel has an `id` (unique in the artifact), a `title`, a `span` (3, 4, 6, 8 or 12 twelfths of the width), and a `body`.
2. Call `artifact_create`. The reply gives the ref `artifact:<id>`.
3. If the hub refuses the spec, its reply lists the problems. Fix exactly those and call again. To change an artifact you made, call `artifact_update` with its id: it makes a new version and keeps the id.
4. Put `artifact:<id>` in your reply where the artifact should appear.

## Body examples

Each body has a `type`. These are complete panels:

- Chart over labels:
  `{"id": "ttft", "title": "Time to first token", "span": 8, "body": {"type": "chart", "unit": "ms", "x": {"type": "labels", "labels": ["run 1", "run 2"]}, "series": [{"name": "p50", "tone": "chart-1", "values": [420, 390]}], "height": 240}}`
- Chart over time (`start_ms` is the first point in epoch milliseconds, `step_ms` the gap between points):
  `{"id": "calls", "title": "Calls per hour", "span": 12, "body": {"type": "chart", "unit": "count", "x": {"type": "time", "start_ms": 1760000000000, "step_ms": 3600000}, "series": [{"name": "calls", "tone": "chart-2", "values": [12, 9, null, 15]}], "height": 240}}`
- Tiles:
  `{"id": "cost", "title": "Cost", "span": 4, "body": {"type": "tiles", "tiles": [{"label": "Spend", "value": "$4.20", "change_pct": -12, "up_bad": true, "spark": [5, 4.6, 4.2], "tone": "chart-2"}]}}`
- Bars:
  `{"id": "models", "title": "Pass rate", "span": 4, "body": {"type": "bars", "rows": [{"label": "model a", "value": "92%", "frac": 0.92, "status": "ok"}, {"label": "model b", "value": "71%", "frac": 0.71, "status": "warn", "note": "timeouts"}]}}`
- Table:
  `{"id": "steps", "title": "Steps", "span": 6, "body": {"type": "table", "columns": [{"head": "Step"}, {"head": "ms", "num": true}], "rows": [{"cells": [{"type": "mono", "text": "compile"}, {"type": "num", "text": "800"}]}, {"cells": [{"type": "mono", "text": "test"}, {"type": "status", "text": "2.1 s", "status": "warn"}]}]}}`
- Graph (`layout` is `layered` for directed flows, `force` for undirected networks; every edge names nodes that exist):
  `{"id": "deps", "title": "Dependencies", "span": 6, "body": {"type": "graph", "nodes": [{"id": "core", "label": "core"}, {"id": "api", "label": "api", "group": "service"}], "edges": [{"from": "api", "to": "core", "label": "uses"}], "layout": "layered", "directed": true}}`
- Dataset (inline rows in column order; a large one goes in a CSV blob):
  `{"id": "runs", "title": "Runs", "span": 12, "body": {"type": "dataset", "columns": [{"key": "name", "label": "Run"}, {"key": "ms", "label": "Time", "kind": "number", "unit": "ms"}], "rows": [["run 1", 420], ["run 2", 390]], "total": 2}}`
- Document (Markdown):
  `{"id": "notes", "title": "Notes", "span": 12, "body": {"type": "document", "markdown": "Compile time fell **8%** after the cache change."}}`
- Code:
  `{"id": "fix", "title": "The fix", "span": 6, "body": {"type": "code", "language": "rust", "text": "fn main() {}", "filename": "main.rs"}}`
- Image: attach the file first with `artifact_attach_data` (its `content_type` is an image type, its `path` a workspace file), then refer to its blob. The alt text is required.
  `{"id": "shot", "title": "Screenshot", "span": 6, "body": {"type": "image", "blob": {"id": "b1"}, "alt": "The settings page in dark mode", "caption": "After the change"}}`
- Html: a small page, or one React component (`App.tsx`, default export) that runs in a sandbox. It has no network. It may import only the libraries listed in the tool description.
  `{"id": "live", "title": "Live view", "span": 12, "body": {"type": "html", "entry": "index.html", "files": [{"path": "index.html", "text": "<p>Hello</p>"}], "height": 240}}`

## Caps

- 24 panels per artifact.
- 5,000 points per series; 1,000 table rows; 2,000 dataset rows inline.
- A spec at most 4 MiB as JSON. Anything larger goes in a CSV or JSON Lines file: attach it with `artifact_attach_data` (content type `text/csv`) and give the dataset `blob: {"id": ...}` in place of `rows`.
- A blob at most 256 MiB.

## Rules

- Reference an artifact as `artifact:<id>` in your reply, where it should appear. The reply then shows it inline.
- Keep the titles short and the alt text specific.
- Check the numbers you show. An artifact is a claim the owner will read.
