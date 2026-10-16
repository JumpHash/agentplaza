# Metrics

The Metrics system records what the plaza does and what it costs: model calls, tokens and spend, turn and tool
latency, the time agents take to wake, boot and fork, the resources of every node, VM, workstation and sandbox, the
work queue and approvals, and the hub's own health. The owner reads it in one view, the same in both consoles.

The code is in four places:

- `crates/apz-metrics`: the core. Series, histograms, compression, rollups, queries, alert rules, forecasts, traces
  and the catalog. No I/O and no clocks; callers pass `now_ms`.
- `crates/apz-metrics-view`: the view model. The hub composes every tab as a list of typed panels, and the formatter.
- `crates/apid/src/metrics`: the hub. It records its own numbers, ingests the nodes' batches, stores, alerts and serves
  `/v1/metrics/*`.
- `crates/noded/src/metrics.rs`: the node side. It collects a node's numbers and holds them until the node link
  carries them to the hub.

The consoles are `apps/web/src/metrics/` with the page in `apps/web/src/views/metrics.ts`, and
`apps/desktop/src/console/screens/metrics/`. The architecture page has a short [Metrics](../architecture.md#metrics)
section that points here.

## What is measured

A **scope** is a place in the tree the view can open. Each tab applies only to some scopes (`apz-metrics-view/src/tabs.rs`).

| Scope | What it is |
|---|---|
| Global | The whole plaza. |
| Node | One machine. `node:n0` is the hub's own machine. |
| Run | A group of VMs on one node. |
| VM | One microVM run by `vmm`. |
| Workstation, sandbox | A workstation (QEMU) or an E2B-style sandbox. They stay their own kinds because they behave differently. |
| Agent | One agent, keyed by its guest CID. |
| Group | A role, a run or a law group, summed, with the agents ranked inside it. |
| Provider, model | A provider and the models behind it. |

The tabs, in order, with their groups:

| Group | Tabs |
|---|---|
| Health | Overview, Alerts |
| Models | Tokens & spend, Context, Latency, Traces, Agents & work |
| Fleet | Compute, Lifecycle, Storage & IO, Hub |
| Plan | Capacity & forecast |
| (none) | Events, My dashboard |

The **catalog** (`CATALOG` in `crates/apz-metrics/src/catalog.rs`) defines every metric the plaza records: 100
definitions. Each has an id (dotted, such as `llm.calls`, `llm.errors` or `llm.ttft_ms`), a title, a unit, a kind
(counter, gauge or histogram), the lowest collection level that records it, its entity kind, the labels it may carry,
its source, a note on its cost and a description. Both consoles read the catalog through `GET /v1/metrics/catalog`, and
the "About this metric" text comes from it. Units decide how a value reads (`Count`, `Percent`, `Ms`, `Bytes`,
`Usd`, `Tokens` and others), and the formatter in `apz-metrics-view` prints every unit the same way on both consoles.

The families cover:

- model calls, tokens, cost and time to first token, with the engine that made each call;
- turn and tool durations and outcomes, and the context each request is made of;
- lifecycle timings (boot, resume, wake, pause, snapshot, template fork, sandbox create, agent move);
- host, VM, workstation and sandbox CPU, memory, disk, IO and network;
- agent time by state, the work queue, approvals and law infractions;
- the hub's API routes, Postgres, event bus, node links and outside services;
- forecasts, budgets and capacity headroom.

## How a value travels

1. **Record.** A daemon registers each handle once (`crates/apz-metrics/src/registry.rs`). A record is one atomic
   operation with no lookup by name and no allocation. A series is keyed by metric id, entity kind and entity id. A
   hard cap per entity kind folds any overflow into an `other` series, so a runaway label cannot grow memory.
2. **Scrape.** A registry is scraped every 10 s, or every 2 s while a console is watching a scope live. The hub's own
   collectors (`crates/apid/src/metrics/collect.rs`) run on its tick: the API route timer, the database timer, bus lag,
   the queue and approval gauges, agent states, the treasury and store sizes, and the metrics system's own cost.
3. **Node batch.** noded keeps what it collected in a buffer. The buffer holds up to 15 minutes when nothing collects
   it, and older samples are dropped first. The node link (`apz-link`) takes the buffer with each heartbeat
   (`GET /v1/metrics/batch` on noded). While the hub has asked for live data, noded answers every 2 s with
   `GET /v1/metrics/batch?live=1`, and answers 204 when nobody is asking. A batch is an `apz_metrics::wire::Batch`:
   delta-encoded columns, with a skip for a value that has not changed.
4. **Ingest.** The hub receives the batch at `POST /v1/nodes/{id}/metrics` (`nodes.rs`), and `crates/apid/src/metrics/ingest.rs` measures each node's
   clock offset from the heartbeat timing, smooths it (0.8 old, 0.2 new) and shifts the batch's times by it. The
   offset is recorded as `link.clock_offset_ms`.
5. **Rollup.** The hub's in-memory `Rollup` (`crates/apz-metrics/src/agg.rs`) builds 10-second, 1-minute and 1-hour
   windows as points arrive. A closed window goes to the store once. Nothing reads raw data back to build an aggregate.
6. **Store.** The store writes in batches every 10 s (see Storage).
7. **Read.** A query picks the coarsest tier that still gives at least one point per two pixels of chart width
   (`query::pick_tier`). The server downsamples to that width with LTTB (`query::lttb`), so the consoles never receive
   more points than they can draw.

**Live.** A console holds a lease (`POST /v1/metrics/live`) for each scope on screen. A lease lapses 15 s after its last
renewal. While any lease is held, the hub scrapes at 2 s and asks each node under a leased scope for 2 s batches. The
hub asks at most every 10 s, and each ask keeps the node streaming for 15 s. Every 2 s it recomposes the live panels of
each leased tab and sends them as a `metrics` event on `/v1/ws`.

**Clock.** Nodes stamp points with their own clock. The hub corrects every batch with the measured offset, so the
timelines of all machines line up.

## Storage

The tables are in `crates/apid/sql/migrations/0014_metrics.sql`. The store's own connection writes them, so metric
writes never wait behind the hub's state writes (`crates/apid/src/metrics/pg.rs`).

| Table | Contents | Default retention |
|---|---|---|
| `metric_chunk_10s` | One row per series per 10 minutes. Points are Gorilla-style compressed (delta-of-delta times, XOR'd values). Histograms are not kept at 10 s. | 48 h |
| `metric_1m` | One row per series per hour: the hour's 1-minute aggregates as JSON, deflate-compressed. | 14 days |
| `metric_1h` | One row per series per day: the same, for 1-hour aggregates. | 400 days |
| `metric_events` | Lifecycle, alert and other events, with duration and entity. | 90 days |
| `model_calls` | One row per model call: tokens, cost, timings, provider, model, agent, role, run and node. | 90 days |
| `trace_spans` | Kept traces, spans as JSON. | 7 days, capped at 2048 MB |
| `metric_kv` | Saved values: settings, alert state, dashboards. The only table that is not partitioned. | none |

- Every other table is partitioned by day (UTC). Retention drops whole partitions, so there are no large deletes.
- The open 10-minute chunk of each series is held in memory and written when it closes.
- Retention is part of the owner's settings (`crates/apid/src/metrics/settings.rs`).
- When there is no Postgres, `crates/apid/src/metrics/mem.rs` keeps the same data in memory, and the saved values in
  files.

Sizing, as the design estimated it for 3 nodes, 100 VMs and about 2,000 series: about 26 MB a day of 10-second chunks,
about 30 MB a day of 1-minute aggregates, about 1 MB a day of hourly aggregates and about 6 MB a day of model calls.
These are estimates. Packing points into rows matters more than compressing them, because each Postgres row carries a
fixed header.

## API

Every route is owner-only, like `/v1/usage`. Agents never reach them. Node numbers stay with the owner.

| Route | Purpose |
|---|---|
| `GET /v1/metrics/catalog` | Metric definitions. |
| `GET /v1/metrics/scopes` | The scope tree. |
| `GET /v1/metrics/view` | A composed tab, from `tab`, `scope`, `range`, `shift`, `zoom`, `compare`, `group`, `trace`, `filter`, `width`, `tz` and `fixture`. |
| `GET /v1/metrics/around` | "What changed around then" for a scope and a time. |
| `POST /v1/metrics/query` | One read, returned as grouped series (the CLI and tests use it). |
| `GET /v1/metrics/events` | Events, by scope, time, kind. |
| `GET /v1/metrics/calls` | Paged model calls. |
| `GET /v1/metrics/traces`, `GET /v1/metrics/traces/{id}` | Kept traces, and one with its spans. |
| `GET /v1/metrics/export` | The chart series of a view as CSV. |
| `POST /v1/metrics/live`, `POST /v1/metrics/open/{cid}` | A live lease, and an agent open in a console. |
| `GET`, `PUT /v1/metrics/settings` | The owner's settings. |
| `GET`, `PUT /v1/metrics/dashboards` | Pinned panels and scopes, so both consoles show the same dashboard. |
| `GET /v1/metrics/alerts`, `PUT /v1/metrics/alerts/rules`, `DELETE /v1/metrics/alerts/rules/{id}` | Alert state, and rule upsert and removal. |
| `POST /v1/metrics/alerts/{id}/mute`, `…/ack`, `…/on` | Mute for a time, acknowledge, enable or disable. |
| `GET /v1/metrics/slos` | Service levels, with compliance and error budget left. |
| `GET /v1/metrics/forecast` | Spend projection, budget burn and headroom. |
| `GET /metrics` | OpenMetrics text, off unless enabled (see Outside endpoints). |

The node daemon serves `GET /v1/metrics/batch` and `POST /v1/metrics/live`. Like its other routes, they need the
machine's internal token.

## View model

`crates/apz-metrics-view/src/model.rs` defines the view. A `View` is a list of `Panel`s. Each panel has a typed `Body`
(tiles, a chart, a table, a heatmap and so on) with its numbers already chosen, and a title and a label. Colours are
`Tone` tokens (`chart-1` to `chart-8`, `ink-3`, `rule`, `ok`, `warn`, `crit`, `info`, `signal`). Each console maps the
tokens to its own palette, so both follow the theme.

- **Composition.** The hub composes each tab with one composer in `crates/apz-metrics-view/src/compose/`, reading
  through the `Source` trait (`source.rs`). The hub implements `Source` over its store
  (`crates/apid/src/metrics/source.rs`). Other composers share the panel helpers in `compose/mod.rs`.
- **Parity.** The consoles are renderers. Neither decides what a tab shows, so the two cannot drift apart. Zoom,
  compare, the "what changed" drawer, the panel menu, pins and the find palette are each console's own interaction,
  driven by the same fields.
- **Formatting.** `apz_metrics_view::format` is the one formatter the hub and the desktop use. The web has its own
  `apps/web/src/metrics/format.ts`. Both are checked against the same golden file,
  `crates/apz-metrics-view/tests/format_golden.json`: the Rust test is `tests/format_golden.rs`, and the web test
  suite (`apps/web/test/tests.ts`) checks the TypeScript formatter against the same file. A change to a format changes
  the golden file, and both sides must pass it.
- **Comparison and movers.** `compare.rs` builds overlays and change percentages. `movers.rs` ranks what moved most
  between two windows.
- **Forecasts.** `crates/apz-metrics/src/forecast.rs` projects a least-squares line with a p10 to p90 band, the time a
  series crosses a limit (for example a volume filling), and a month-end spend. Forecasts are labelled as estimates.

## Alerts and service levels

A **rule** (`crates/apz-metrics/src/alerts.rs`) names one metric, an aggregate, a scope (or every node, agent, run or
volume), a comparison, a threshold, a duration (`for`), a severity and an on or off switch. A rule moves through ok,
pending, firing and resolved. It fires only after its condition has held for its duration.

The starter rules are on by default and can be switched off:

- first-token p95 over 2 s;
- a volume over 90 percent used;
- an agent's context window over 90 percent full;
- wake p95 over 1 s;
- daily spend over $60;
- a node not reporting for 60 s.

The starter service levels are: wake p95 under 1 s, first-token p95 under 3 s, hub API p95 under 200 ms and heartbeat
lag under 30 s. `slo_status` reports compliance and the error budget left over the window.

Evaluation runs on the hub as windows update (`crates/apid/src/metrics/alerting.rs`). It reads the open windows and no
table. A transition is published on the bus as `metrics.alert`, which the consoles show as a banner and a desktop
notification, and it is recorded as an `alert` event. A rule may also post to the board. Mute, acknowledge and
enable or disable are owner actions, kept in a history of up to 500 entries.

Spend caps stay with the laws, which enforce them. Metrics reports budgets and can warn before a cap is reached, but it
never blocks anything.

## Traces and tail sampling

A **trace** is one agent turn. Its spans are kept in a per-agent ring of 256 spans (`RING` in
`crates/apz-metrics/src/trace.rs`), and the oldest are dropped past that. The hub hands out span ids
(`Traces::span_id` in `crates/apid/src/metrics/traces.rs`). Nothing is written until the turn ends.

When a turn ends, the tail sampler (`TailSampler::decide`) decides whether to keep the whole trace:

- every turn that errored;
- every turn of an agent open in a console right now;
- every turn slower than the p95 of its role, once that role has 20 finished turns;
- 1 percent of the rest, chosen by a deterministic random generator;
- everything, at the Full level.

Every finished turn feeds its role's duration histogram, so the aggregates stay exact even though most traces are not
kept. Kept traces go to `trace_spans` and are read through `GET /v1/metrics/traces`.

## Cost budget, levels and the governor

The metrics system measures its own cost (`collect.rs` records it with the hub's own numbers). The design budgets are:

| Where | Budget |
|---|---|
| noded sampler | under 0.5 percent of one core with 50 VMs, under 8 MB resident |
| Hub ingest, rollups and query cache | under 1 percent of one core steady, under 64 MB resident |
| Hot path (one model call, one block request) | under 1 µs added, no allocation, lock or syscall |
| Harness, per agent | under 100 KB |
| Heartbeat payload | under 4 KB per beat on average |
| Postgres | under 100 MB per day at the estimate above |
| Console | under 2 percent CPU while visible and live, nothing while hidden |

These are targets. The benchmark that would check them in CI is not in the tree yet.

The **collection level** (`crates/apz-metrics/src/level.rs`) is the owner's choice in Settings:

| Level | Collects |
|---|---|
| Off | Only model calls, which billing needs. Alert rules on spend still run. |
| Basic | Host and VM CPU and memory at 60 s, lifecycle events, latency histograms, hub timers, alerts and forecasts. |
| Standard (default) | Plus disk, IO, network and pressure at 10 s, the context split, agent state time and sampled traces. |
| Full | Plus 2 s everywhere, every trace for 24 h and per-process guest statistics. It reverts to Standard after 24 h. |

`Level::sample_ms` gives the interval for host and VM gauges: 2 s when a scope is live or the level is Full, 60 s for
Basic or an idle VM (idle for more than 5 minutes), 10 s otherwise, and none at Off.

The **governor** (`Governor` in the same file) compares the metrics system's cost with its budget. Over budget for
5 minutes, it steps the effective level down one step. Under budget for 30 minutes, it steps back up, never above the
level the owner chose.

Settings (`crates/apid/src/metrics/settings.rs`) hold the level, the retention, the spend budgets, the forecast basis
and the outside endpoints. The owner edits them in the Metrics view's settings dialog.

## Privacy

- Only numbers and names are stored. Prompts, completions, tool arguments and tool output never enter metrics. Traces
  keep timings and sizes.
- Series keys are entity ids and catalog labels, never free text from a request. The OpenMetrics output allows only the
  labels the catalog lists.
- Agents never read metrics. Node numbers stay with the owner, and nodes report only their own numbers.
- Outside endpoints are owner actions, off by default.

## Outside endpoints

- **OpenMetrics.** `GET /metrics` returns the latest 1-minute window of every series, in OpenMetrics text
  (`export::openmetrics`, `crates/apz-metrics/src/openmetrics.rs`). It returns 404 unless the owner enables it. When
  enabled, it requires `Authorization: Bearer` with a token the owner's settings hold, and the token is issued when the
  endpoint is turned on.
- **OTLP.** Kept traces can be exported as OTLP/HTTP JSON (`export.rs`) to a collector URL the owner names. The export is
  off by default and sends nothing until the owner has confirmed that collector.
- **CSV.** `GET /v1/metrics/export` writes the chart series of any view, comparison series included.

## The fixture

`fixture=1` on `GET /v1/metrics/view` composes the view from
`apz_metrics_view::fixture`, a `Source` of generated data, instead of the store. The desktop screen starts in fixture
mode when the `APZ_METRICS_FIXTURE` environment variable is set at launch
(`apps/desktop/src/console/screens/metrics/mod.rs`). The web view state carries the same flag. The fixture is for
design review and for the composers' tests, and it never reads the store.

## The CLI

`apz metrics` (`apps/cli/src/cmd/metrics.rs`) reads the same routes as the consoles:

- `apz metrics top [--metric llm.spend_usd] [--scope global] [--window 24h] [--by agent|node|provider|model|role]
  [--limit 10]` prints the
  groups with the largest totals (counters) or means (everything else) over the window.
- `apz metrics query --metric <id> [--scope …] [--window 24h] [--step 1h] [--agg …] [--by agent] [--csv]` prints one metric as a
  row per step and a column per group, or CSV with epoch milliseconds and raw values.
- `apz metrics watch [--scope global] [--interval 2]` prints the overview tiles again every interval until Ctrl-C.
- `apz metrics alerts` lists the alert rules and the subjects firing now.

The `/v1/usage/*` routes are unchanged.

## Not yet built

- The CI benchmark that runs a fake node with 50 VMs and checks the budgets above.
