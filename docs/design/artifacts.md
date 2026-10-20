# Artifacts

An artifact is a titled page of panels that an agent makes to show its work: charts, tables, graphs, documents,
code, images, datasets, or a small web page. The owner and other agents open it from the hub. This document covers
the model, how agents make artifacts, the hub's store and owner routes, where artifacts appear, and how HTML
artifacts run in a sandbox. Sharing a link outside the hub is in `docs/design/sharing.md`.

## 1. The model

An artifact is a title, an optional subtitle, and up to 24 panels on the 12-column grid. Each panel has a title and
one typed body. The types are shared by both consoles, so the web and the desktop draw the same numbers.

Crate `crates/apz-view`:

| File | What it holds |
|---|---|
| `src/model.rs` | The panel model (serde). `Panel`, `PanelAction`, `PinRef`, and `Body`. The web mirrors these types in `apps/web/src/view/model.ts`. |
| `src/artifact.rs` | `Spec` (what an agent sends), `Artifact` (a stored version), `Summary` (a list row), `Caps`, `validate`, `kind_of`, and the HTML checks. |
| `src/libs.rs` | The vendored libraries an HTML artifact may import, with pinned versions. |
| `src/highlight.rs` | The syntax highlighter for code panels. Its rules are data in `apps/shared/highlight.json`, which the web reads too. |
| `src/format.rs` | Value formatting by unit, axis ticks, and time labels. |

### Bodies

`Body` has these variants. The first group is the Metrics set. The second group is what artifacts add.

| Group | Variants |
|---|---|
| Metrics | `Health`, `Tiles`, `Chart`, `Heatmap`, `Treemap`, `Gauges`, `Table`, `Flags`, `Bars`, `Split`, `Stats`, `Swimlanes`, `Alerts`, `Rules`, `Traces`, `Events`, `Empty` |
| Artifact | `Image`, `Graph`, `Dataset`, `Document`, `Code`, `Html` |

`kind_of` names an artifact by its panels: `chart`, `table`, `dataset`, `image`, `graph`, `document`, `code`,
`html`, `figures` (tiles, gauges, or stats), `panel`, or `empty`.

### Limits

`Caps::default()` in `artifact.rs` holds the limits every artifact meets. `validate` returns one message per problem,
and each message names the panel it is about. The hub runs `validate` on every create and every update.

| Cap | Limit |
|---|---|
| Panels | 24 |
| Title | 120 characters |
| Series in a chart | 24 |
| Points in a series | 5,000 |
| Table rows | 1,000 |
| Inline dataset rows | 2,000 |
| Columns | 64 |
| Graph nodes | 1,000 |
| Graph edges | 4,000 |
| Document or code text | 400,000 bytes |
| HTML files per artifact | 48 |
| HTML bytes per artifact | 2 MiB |
| Spec as JSON | 4 MiB |
| One blob | 256 MiB |

Larger data goes into a CSV or JSON Lines file attached as a blob (section 2).

### Vendored libraries

An HTML artifact may import only the libraries in `libs.rs`. The hub serves them from `/_lib/` (section 5). The set
and the versions are:

| Name | Version |
|---|---|
| react | 18.3.1 |
| react-dom | 18.3.1 |
| prop-types | 15.8.1 |
| recharts | 2.15.4 |
| d3 | 7.9.0 |
| three | 0.160.1 |
| lodash | 4.17.21 |
| papaparse | 5.5.3 |
| lucide | 1.55.0 |
| tailwindcss (browser build) | 4.1.13 |

The web build copies these from `apps/web/node_modules` at the versions `apps/web/package.json` pins, and a test in
`libs.rs` checks the pins. `libs::closure` works out the files a page needs, so a page loads only what it uses.
recharts pulls in react, react-dom, and prop-types.

## 2. How agents make artifacts

### Tools

An agent makes an artifact through three tools. Their names are in `crates/apid/src/tools.rs` (lines 61 to 63), the
shared input schema is at line 450, and the descriptions start at line 560.

| Tool | Use |
|---|---|
| `artifact_create` | Make a new artifact from a title, a subtitle, and panels. Takes `project` as well. The reply gives the ref `artifact:<id>`. |
| `artifact_update` | Publish a new version of an artifact the agent made. |
| `artifact_attach_data` | Attach a file to an artifact: a CSV or JSON Lines dataset, an image, or a workspace file. The reply gives a blob id (`b1`, `b2`, ...). |

The tool descriptions tell the agent to use artifacts for results, comparisons, measurements, dependency graphs, and
small dashboards. A short answer stays text. Data beyond the inline caps goes into a CSV attached with
`artifact_attach_data`, and the dataset panel refers to it with `blob: {"id": ...}` in place of `rows`.

The built-in skill "Show your work" (`crates/apid/src/skills/builtin/show-your-work/SKILL.md`) teaches the steps:
build the spec, attach data first, call `artifact_create`, then put the returned `artifact:<id>` in the reply where
the artifact should appear. An image needs its file attached first, with alt text.

### Gateway routes

The agents' routes are in `crates/apid/src/artifacts/gateway.rs`, under `/internal/v1/agents/{cid}/gateway/`, where
`{cid}` is the agent's id. They sit behind the same authorisation as the agent's other gateway routes.

| Route | Handler | Purpose |
|---|---|---|
| `PUT /internal/v1/agents/{cid}/gateway/artifacts` | `create_route` | Create an artifact |
| `.../artifacts/{id}` | `update_route` | Publish a new version |
| `.../artifacts/blobs` | `blob_route` | Attach a blob. Registered before `{id}`, so `blobs` is never read as an id. |
| `.../artifacts/{id}` | `get_route` | Read an artifact back. An id the agent cannot read answers 404. |

### Checks on every write

Each create and update passes these checks, in this order. A refusal carries the reasons, and the agent fixes the
spec and calls again.

1. The budget. `over_budget` refuses a create when the agent's artifacts or bytes in the window reach
   `DAILY_ARTIFACTS` (200) or `DAILY_BYTES` (512 MiB). The counts come from `Store::stats`.
2. The laws. `refused_by_law` (`gateway.rs`, line 225) asks the laws engine (`crates/apid/src/laws.rs`) whether the
   agent may publish this spec, as the files route does for a file.
3. The validator. `apz_view::artifact::validate` checks the caps, the panels, the HTML files, and the blob ids. A blob
   that does not exist is refused, with a hint to attach it first.

A create writes the artifact to the store with the agent's id and the project, and `record` in `gateway.rs` logs it.

### Versions

Each update adds a version, and an earlier version is not changed. A ref without a version (`artifact:<id>`) means the
latest. A ref with one (`artifact:<id>@<n>`) pins that version. Owners list versions with
`GET /v1/artifacts/{id}/versions`.

### Blobs

A blob is a file attached to one artifact. Blobs are written to the deliverables store (`crates/apid/src/deliverables.rs`),
which keeps objects on Garage or on disk. `Store::put_blob` (`artifacts/mod.rs`) writes the bytes and returns a
`BlobRef` (`apz-view/src/model.rs`, line 187). A blob's id (`b1`, `b2`, ...) is local to its artifact, so `b1` in one
artifact is not `b1` in another. `Store::artifact_blob` reads a blob by artifact and id.

## 3. The store and the owner routes

### Store

`crates/apid/src/artifacts/mod.rs` holds `Store`. Its index is `artifacts.json` in the hub's state folder, and
`Store::load` reads it at start-up.

| Method | What it does |
|---|---|
| `create`, `update` | Validate, then write a new artifact or a new version. Return the validator's messages on refusal. |
| `get(id, version)` | Read one version, or the latest. |
| `list(project, query)` | List summaries, filtered by project and by text. |
| `versions(id)` | List the versions of one artifact. |
| `delete(id)` | Remove an artifact. |
| `rows(...)`, `dataset_rows(...)` | Read a dataset's rows in pages. A page is at most `MAX_PAGE` (500) rows; the default is `DEFAULT_PAGE` (100). |
| `compiled(id, version, path)` | Return the compiled JavaScript of one HTML file (section 5). |
| `set_shared(id, shared)` | Mark an artifact as shared. Sharing itself is in `docs/design/sharing.md`. |
| `stats(cid, since_ms)` | Count an agent's artifacts and bytes since a time, for the budget. |

### Owner routes

`crates/apid/src/artifacts/routes.rs` registers the owner's routes.

| Route | Handler | Purpose |
|---|---|---|
| `GET /v1/artifacts` | `list_route` | List summaries, filtered by project and query |
| `GET /v1/artifacts/{id}` | `get_route` | One artifact. A `version` parameter pins a version. |
| `GET /v1/artifacts/{id}/versions` | `versions_route` | The version list |
| `GET /v1/artifacts/{id}/blobs/{blob}` | `blob_route` | A blob's bytes |
| `GET /v1/artifacts/{id}/rows` | `rows_route` | A dataset's rows, paged |
| `GET /v1/artifacts/{id}/export` | `export_route` | Export a panel as CSV, JSON, or PNG |
| `DELETE /v1/artifacts/{id}` | `delete_route` | Delete the artifact |

`export_route` reads a panel's table with `panel_table` and writes CSV with `to_csv`, quoting fields with `csv_field`.
The file name comes from the title and the panel (`file_stem`, `slug`).

## 4. Where artifacts show

### References

The rich-text crate `crates/apz-rich` reads an artifact reference in three forms:

- `artifact:<id>` and `artifact:<id>@<n>`, as one whole word in text or in a markdown link target. The id is 8 to 40
  characters. A bad id or version stays plain text (`classify.rs`, line 156).
- A hub link `/artifact/<id>`.
- The `artifact` kind of the rich model, which carries the id and the version (`apz-rich/src/lib.rs`, line 102).

Public links are covered in `docs/design/sharing.md`.

### Embeds and chips

A message that names an artifact shows it inline. The first mention of an id and version in a message gets an embed,
and later mentions get a compact chip.

| Console | File |
|---|---|
| Web | `apps/web/src/ui/markdown.ts` (`artifactBlocks`, with a per-message `artifacts.seen` set), and `apps/web/src/views/artifact.ts` (`artifactEmbed`, `artifactChip`) |
| Desktop | `apps/desktop/src/console/rich.rs`, which builds the artifact embeds for the transcript |

### Viewer screens

| Console | Where |
|---|---|
| Web | Route `#/artifact/<id>?v=<n>`, in `apps/web/src/views/artifact.ts`. The screen draws the artifact with the owner's context and offers its versions, Share, and Export. |
| Desktop | The `Id::Artifact` screen in `apps/desktop/src/console/shell.rs`. Going from the Library to an artifact is listed at line 627. |

### The Library

The Library has an Artifacts group that lists the artifacts the agents made, newest first. The desktop loads it as
`Artifacts(Result<Vec<Summary>, String>)` in `apps/desktop/src/console/screens/library.rs` (lines 236 to 248).

### Shared renderers

Each panel kind has one renderer per console, so the two consoles draw the same thing.

| Console | Directory | Files |
|---|---|---|
| Web | `apps/web/src/view/` | `artifact.ts`, `bodies.ts`, `code.ts`, `dataset.ts`, `frame.ts`, `graph.ts`, `highlight.ts`, `model.ts`, `backdrop.ts`, `owner-ctx.ts` |
| Desktop | `apps/desktop/src/console/view/` | `code.rs`, `dataset.rs`, `document.rs`, `graph.rs`, `image.rs`, `mod.rs`, `webview.rs` |

Graphs are laid out by `crates/apz-graph` on both consoles, so a graph has the same card sizes on each (see
`docs/design/parity.md`).

## 5. HTML artifacts

An HTML artifact is a set of files: an entry page (`.html`), styles, and scripts in TypeScript, TSX, JSX, or
JavaScript. It runs in a sandboxed frame with no access to the console's origin.

### Compile

Scripts are compiled on the hub. `crates/apz-artifact/src/lib.rs` `compile(path, source)` parses each script with oxc,
checks it, strips the types, turns JSX into `React.createElement` calls, and emits an ES module. `Store::compiled`
returns the compiled JavaScript of one file.

- An import of a vendored library becomes a read of the global that the library sets on the page, so a page loads
  only what the hub vendored.
- A relative import stays as it is, and the file must be part of the artifact.
- Any other import is refused, and so is a network load (`network_loads` in `apz-view/src/artifact.rs`). A link is
  allowed; a load is not.
- `safe_path` and `is_script` keep file paths plain: no `..`, no backslashes, and only the script extensions
  `.ts`, `.tsx`, `.jsx`, `.js`, and `.mjs`.

The validator runs these checks on create and update, and a refusal names the file and the import.

### The content origin

Frames are served from a second origin, on a second port, so a page in a frame runs apart from the console. The
origin is set up in `crates/apid/src/content.rs`.

- The port comes from `--content-port`, parsed by `take_port`. Without the flag it is the API port plus one. The hub
  refuses to start when the API port is 65535 and no content port is given.
- A frame's URL carries its own capability: `/a/{artifact}/{version}/{panel}/{cap}/{path}`. `cap` is an HMAC over the
  artifact, the version, the panel, and an expiry, made with a secret in a private file (`frame_secret`,
  `write_private`). `cap_valid` checks it, so a URL stops working when its expiry passes.
- `/_lib/{file}` serves the vendored libraries. `/_bridge.js` serves `crates/apid/src/content_bridge.js`. `with_bridge`
  puts the bridge script before the page's own scripts, so the page can report its size before its code runs.

Every frame response carries the headers from `frame_headers`:

- One Content-Security-Policy with `sandbox allow-scripts allow-popups allow-downloads` and `default-src 'none'`. The
  sandbox has no `allow-same-origin`, so the page cannot read the console's storage or cookies.
- `Permissions-Policy: camera=(), microphone=(), geolocation=(), clipboard-read=()`.
- `frame-ancestors`, which limits which pages may embed the frame.

### The bridge

The page talks to the host through `window.postMessage` on the web and through the webview's IPC on the desktop. The
bridge in `content_bridge.js` sends these messages:

| Message | Direction | Meaning |
|---|---|---|
| `ready` | page to host | The page's scripts have run. Sent on `DOMContentLoaded`. |
| `height` | page to host | The page's height in pixels, sent when it changes. |
| `open` | page to host | A link to leave the page. Only web and mail links go. `window.open` takes the same route. |
| `download` | page to host | A file to save: a name and its contents. |
| `theme` | host to page | The palette: a map of custom properties (`--name`) and the mode, `light` or `dark`. |

A link or a download does not happen on the page's word alone. The host asks the owner first, in a confirm bar on the
web (`apps/web/src/view/frame.ts`, line 189) and in a confirmation on the desktop (`webview.rs`). One question is open
at a time. The host checks each link (`webUrl` in `frame.ts`, `is_openable` in `webview.rs`) and sanitises a download's
name (`fileName` in `frame.ts`, `safe_name` in `webview.rs`), so the page does not choose a path.

### Web frames

`apps/web/src/view/frame.ts` makes the frame. It loads the content origin's URL in an iframe with
`sandbox="allow-scripts allow-popups allow-downloads"`, and never `allow-same-origin`, so the frame is an opaque
origin. The console's start page allows that origin as a frame source: `console_page` in
`crates/apid/src/content.rs` adds it to the page's `frame-src 'self'`, because the content listener is another port.
The console takes a message only from a live frame's own window, and only with the `apz:1` tag. The height the page reports is clamped between 120 and 2,000
pixels, and the default is 480 until the page reports.

At most two frames run at once (`LIVE_LIMIT`, line 21). The frames that run are the ones the owner focused or opened
most recently, and the rule is set out in the comment at the top of the file. Other frames show their still.

### Desktop webviews

`apps/desktop/src/console/view/webview.rs` draws each frame in a native webview through `iced_wry`. The webview is the
platform's own engine: WebKitGTK on Linux x86_64, WebView2 on Windows, and WKWebView on macOS.

- `LIVE` (line 16) is true on Windows, macOS, and Linux x86_64. Elsewhere the frame is its still image.
- The arm64 Linux packages do not link a webview. An HTML artifact there shows its still and an "Open in browser"
  button, which opens the content origin's URL.
- The same two-live-frames rule applies, and the same bridge messages and confirmations.

The desktop gives the page the app's palette as `__apzTheme` (built by `theme_vars` and `theme_script`), so the page
follows the app's light and dark mode.

### Tests

`apz-view/src/artifact.rs` tests the import scan (`imports_are_found`), the refusal of network loads
(`network_loads_are_refused_but_links_are_not`), and the vendored-library check
(`html_checks_imports_against_the_vendored_set`).
