//! Every metric the plaza records. Both consoles read this (through `GET /v1/metrics/catalog`) for units, titles
//! and the "About this metric" text; the OpenMetrics output allows only the labels listed here.

use crate::EntityKind::{self, *};
use crate::level::Level;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricKind {
    /// A running total; a scrape yields the increase since the last one.
    Counter,
    /// A level at a moment.
    Gauge,
    /// A distribution, in log-scale buckets.
    Histogram,
}

/// How a value reads. The consoles format by unit (apz-metrics-view `format`), so both print the same text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Count,
    Percent,
    Ms,
    Bytes,
    BytesPerSec,
    Mib,
    Usd,
    Tokens,
    TokensPerSec,
    Ratio,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct MetricDef {
    pub id: &'static str,
    pub title: &'static str,
    pub unit: Unit,
    pub kind: MetricKind,
    /// The lowest collection level that records it.
    pub level: Level,
    /// The entity kind its series are keyed by.
    pub entity: EntityKind,
    /// The labels it may carry (at most two).
    pub labels: &'static [&'static str],
    /// Where it comes from.
    pub source: &'static str,
    /// What recording it costs.
    pub cost: &'static str,
    /// What it measures, for people.
    pub description: &'static str,
}

/// Looks a metric up by id.
pub fn find(id: &str) -> Option<&'static MetricDef> {
    CATALOG.iter().find(|m| m.id == id)
}

const ATOMIC: &str = "one atomic operation per record; no allocation";
const PREAD: &str = "one pread of an already open file per tick";
const STATE: &str = "one counter add per state change";

macro_rules! m {
    ($id:literal, $title:literal, $unit:ident, $kind:ident, $level:ident, $entity:ident, [$($l:literal),*], $src:literal, $cost:expr, $desc:literal) => {
        MetricDef {
            id: $id,
            title: $title,
            unit: Unit::$unit,
            kind: MetricKind::$kind,
            level: Level::$level,
            entity: $entity,
            labels: &[$($l),*],
            source: $src,
            cost: $cost,
            description: $desc,
        }
    };
}

pub const CATALOG: &[MetricDef] = &[
    // Model calls: keyed by agent, labelled with provider and model; scopes resolve through the agent.
    m!(
        "llm.calls",
        "Model calls",
        Count,
        Counter,
        Off,
        Agent,
        ["model", "provider"],
        "hub LLM proxy and harness metering",
        ATOMIC,
        "Model calls made."
    ),
    m!(
        "llm.errors",
        "Model call errors",
        Count,
        Counter,
        Basic,
        Agent,
        ["model", "provider"],
        "hub LLM proxy",
        ATOMIC,
        "Model calls that ended in an error status."
    ),
    m!(
        "llm.retries",
        "Model call retries",
        Count,
        Counter,
        Basic,
        Agent,
        ["model", "provider"],
        "hub LLM proxy",
        ATOMIC,
        "Model calls repeated after a rate limit or overload."
    ),
    m!(
        "llm.ttft_ms",
        "Time to first token",
        Ms,
        Histogram,
        Basic,
        Agent,
        ["model", "provider"],
        "hub LLM proxy",
        ATOMIC,
        "Time from the request leaving the hub to the first streamed token arriving."
    ),
    m!(
        "llm.duration_ms",
        "Model call duration",
        Ms,
        Histogram,
        Basic,
        Agent,
        ["model", "provider"],
        "hub LLM proxy",
        ATOMIC,
        "Total time of each model call."
    ),
    m!(
        "llm.out_tps",
        "Output speed",
        TokensPerSec,
        Histogram,
        Basic,
        Agent,
        ["model", "provider"],
        "hub LLM proxy",
        ATOMIC,
        "Output tokens per second while streaming."
    ),
    m!(
        "llm.tokens_in",
        "Input tokens (fresh)",
        Tokens,
        Counter,
        Off,
        Agent,
        ["model", "provider"],
        "provider-reported usage",
        ATOMIC,
        "Input tokens that were not read from the provider's cache."
    ),
    m!(
        "llm.tokens_cache_read",
        "Input tokens (cache read)",
        Tokens,
        Counter,
        Off,
        Agent,
        ["model", "provider"],
        "provider-reported usage",
        ATOMIC,
        "Input tokens served from the provider's cache."
    ),
    m!(
        "llm.tokens_cache_write",
        "Input tokens (cache write)",
        Tokens,
        Counter,
        Off,
        Agent,
        ["model", "provider"],
        "provider-reported usage",
        ATOMIC,
        "Input tokens written into the provider's cache."
    ),
    m!(
        "llm.tokens_out",
        "Output tokens",
        Tokens,
        Counter,
        Off,
        Agent,
        ["model", "provider"],
        "provider-reported usage",
        ATOMIC,
        "Output tokens."
    ),
    m!(
        "llm.spend_usd",
        "Spend",
        Usd,
        Counter,
        Off,
        Agent,
        ["model", "provider"],
        "provider-reported cost, else the price catalog estimate",
        ATOMIC,
        "Cost of model calls: the provider-reported cost when it sends one, otherwise the catalog estimate."
    ),
    m!(
        "llm.engine_spend_usd",
        "Spend by engine",
        Usd,
        Counter,
        Off,
        Engine,
        [],
        "hub LLM proxy and harness metering",
        ATOMIC,
        "Spend split by the engine that made the call: the plaza harness, an ACP engine or hub MCP."
    ),
    m!(
        "llm.ratelimit_requests_left_pct",
        "Rate limit: requests left",
        Percent,
        Gauge,
        Standard,
        Provider,
        [],
        "provider rate-limit response headers",
        "read from headers already in hand",
        "Requests left in the provider's current rate-limit window."
    ),
    m!(
        "llm.ratelimit_tokens_left_pct",
        "Rate limit: tokens left",
        Percent,
        Gauge,
        Standard,
        Provider,
        [],
        "provider rate-limit response headers",
        "read from headers already in hand",
        "Tokens left in the provider's current rate-limit window."
    ),
    // Context.
    m!(
        "ctx.tokens",
        "Context composition",
        Tokens,
        Gauge,
        Standard,
        Agent,
        ["part"],
        "agent harness and ACP, per model call",
        "section lengths already known while building the request",
        "What fills each model request: system prompt, tool definitions, history, tool results, attachments and the compaction summary. The split is estimated from section sizes and scaled to the provider-reported input tokens."
    ),
    m!(
        "ctx.fill_pct",
        "Window fill",
        Percent,
        Gauge,
        Standard,
        Agent,
        [],
        "agent harness and ACP, per model call",
        ATOMIC,
        "Input tokens of the latest call as a share of the model's context window."
    ),
    m!(
        "ctx.window",
        "Context window",
        Tokens,
        Gauge,
        Standard,
        Agent,
        [],
        "model catalog",
        ATOMIC,
        "The context window of the model the agent uses."
    ),
    m!(
        "ctx.compactions",
        "Compactions",
        Count,
        Counter,
        Standard,
        Agent,
        [],
        "agent harness",
        ATOMIC,
        "History compactions."
    ),
    m!(
        "ctx.freed_tokens",
        "Tokens freed by compaction",
        Tokens,
        Counter,
        Standard,
        Agent,
        [],
        "agent harness",
        ATOMIC,
        "Tokens removed from the window by compactions."
    ),
    m!(
        "ctx.carried_tokens",
        "Carried context",
        Tokens,
        Counter,
        Standard,
        Agent,
        [],
        "agent harness",
        ATOMIC,
        "Input tokens that only re-send earlier turns."
    ),
    // Turns, tools, agents, work.
    m!(
        "turn.duration_ms",
        "Turn duration",
        Ms,
        Histogram,
        Basic,
        Agent,
        [],
        "agent harness and ACP turn boundaries",
        ATOMIC,
        "One agent turn, from prompt to final answer."
    ),
    m!(
        "tool.duration_ms",
        "Tool duration",
        Ms,
        Histogram,
        Basic,
        Tool,
        [],
        "hub tool and MCP relays",
        ATOMIC,
        "Time of each tool or MCP call."
    ),
    m!(
        "tool.calls",
        "Tool calls",
        Count,
        Counter,
        Basic,
        Tool,
        [],
        "hub tool and MCP relays",
        ATOMIC,
        "Tool and MCP calls."
    ),
    m!(
        "tool.errors",
        "Tool errors",
        Count,
        Counter,
        Basic,
        Tool,
        [],
        "hub tool and MCP relays",
        ATOMIC,
        "Tool and MCP calls that failed."
    ),
    m!(
        "agent.state_ms",
        "Where agent time goes",
        Ms,
        Counter,
        Standard,
        Agent,
        ["state"],
        "agent state changes",
        STATE,
        "Time each agent spends streaming from a model, waiting for the first token, running tools, waiting for owner approval, idle or asleep."
    ),
    m!(
        "work.queued",
        "Queued tasks",
        Count,
        Gauge,
        Basic,
        Agent,
        [],
        "scheduler lanes",
        ATOMIC,
        "Tasks waiting for an agent."
    ),
    m!(
        "work.wait_ms",
        "Task wait",
        Ms,
        Histogram,
        Basic,
        Agent,
        [],
        "scheduler lanes",
        ATOMIC,
        "How long a task waited before an agent picked it up."
    ),
    m!(
        "work.done",
        "Tasks finished",
        Count,
        Counter,
        Basic,
        Agent,
        [],
        "scheduler",
        ATOMIC,
        "Tasks that finished."
    ),
    m!(
        "work.failed",
        "Tasks failed",
        Count,
        Counter,
        Basic,
        Agent,
        [],
        "scheduler",
        ATOMIC,
        "Tasks that failed or were abandoned."
    ),
    m!(
        "work.turns",
        "Turns per task",
        Count,
        Histogram,
        Basic,
        Agent,
        [],
        "scheduler",
        ATOMIC,
        "Model turns a finished task took."
    ),
    m!(
        "work.cost_usd",
        "Cost per task",
        Usd,
        Histogram,
        Basic,
        Agent,
        [],
        "scheduler and usage",
        ATOMIC,
        "Spend of a finished task."
    ),
    m!(
        "tool.errors_by_agent",
        "Tool errors by agent",
        Count,
        Counter,
        Basic,
        Agent,
        [],
        "hub tool relays",
        ATOMIC,
        "Failed tool calls, per agent."
    ),
    m!(
        "approval.wait_ms",
        "Approval wait",
        Ms,
        Histogram,
        Standard,
        Agent,
        [],
        "approvals",
        ATOMIC,
        "How long an approval request waited for the owner."
    ),
    m!(
        "approval.pending",
        "Approvals waiting",
        Count,
        Gauge,
        Standard,
        Global,
        [],
        "approvals",
        ATOMIC,
        "Approval requests waiting on the owner now."
    ),
    m!(
        "law.infractions",
        "Law infractions",
        Count,
        Counter,
        Standard,
        Agent,
        ["law"],
        "the laws' infraction record",
        "a read of rows the laws already write",
        "What the laws stopped."
    ),
    // Hosts.
    m!(
        "host.cpu_pct",
        "CPU",
        Percent,
        Gauge,
        Basic,
        Node,
        [],
        "noded sampler: /proc/stat, host_statistics64, GetSystemTimes",
        PREAD,
        "Share of all host cores busy."
    ),
    m!(
        "host.mem_used_mib",
        "Memory",
        Mib,
        Gauge,
        Basic,
        Node,
        [],
        "noded sampler",
        PREAD,
        "Host memory in use (total minus available)."
    ),
    m!(
        "host.mem_total_mib",
        "Memory size",
        Mib,
        Gauge,
        Basic,
        Node,
        [],
        "noded sampler",
        PREAD,
        "Host memory installed."
    ),
    m!(
        "host.load1",
        "Load",
        Count,
        Gauge,
        Basic,
        Node,
        [],
        "noded sampler",
        PREAD,
        "One-minute load average."
    ),
    m!(
        "host.disk_read_bps",
        "Host disk read",
        BytesPerSec,
        Gauge,
        Standard,
        Node,
        [],
        "noded sampler: /proc/diskstats",
        PREAD,
        "Bytes read from host disks per second."
    ),
    m!(
        "host.disk_write_bps",
        "Host disk write",
        BytesPerSec,
        Gauge,
        Standard,
        Node,
        [],
        "noded sampler: /proc/diskstats",
        PREAD,
        "Bytes written to host disks per second."
    ),
    m!(
        "host.net_rx_bps",
        "Host network in",
        BytesPerSec,
        Gauge,
        Standard,
        Node,
        [],
        "noded sampler: /proc/net/dev",
        PREAD,
        "Bytes received per second, all interfaces but loopback."
    ),
    m!(
        "host.net_tx_bps",
        "Host network out",
        BytesPerSec,
        Gauge,
        Standard,
        Node,
        [],
        "noded sampler: /proc/net/dev",
        PREAD,
        "Bytes sent per second, all interfaces but loopback."
    ),
    m!(
        "host.psi_cpu_pct",
        "CPU pressure",
        Percent,
        Gauge,
        Standard,
        Node,
        [],
        "noded sampler: /proc/pressure",
        PREAD,
        "Share of time tasks waited for CPU (Linux pressure stall information)."
    ),
    m!(
        "host.psi_mem_pct",
        "Memory pressure",
        Percent,
        Gauge,
        Standard,
        Node,
        [],
        "noded sampler: /proc/pressure",
        PREAD,
        "Share of time tasks waited for memory."
    ),
    m!(
        "host.psi_io_pct",
        "IO pressure",
        Percent,
        Gauge,
        Standard,
        Node,
        [],
        "noded sampler: /proc/pressure",
        PREAD,
        "Share of time tasks waited for IO."
    ),
    m!(
        "host.gpu_util_pct",
        "GPU",
        Percent,
        Gauge,
        Standard,
        Node,
        ["gpu"],
        "noded sampler: vendor sysfs or nvidia-smi",
        "one read per 10 s, only on nodes with a GPU",
        "GPU utilisation."
    ),
    m!(
        "host.gpu_mem_pct",
        "GPU memory",
        Percent,
        Gauge,
        Standard,
        Node,
        ["gpu"],
        "noded sampler: vendor sysfs or nvidia-smi",
        "one read per 10 s, only on nodes with a GPU",
        "GPU memory in use."
    ),
    m!(
        "volume.used_pct",
        "Volume used",
        Percent,
        Gauge,
        Standard,
        Volume,
        [],
        "noded sampler: statvfs",
        "one statvfs per volume per tick",
        "Used share of a volume."
    ),
    m!(
        "volume.used_bytes",
        "Volume used bytes",
        Bytes,
        Gauge,
        Standard,
        Volume,
        [],
        "noded sampler: statvfs",
        "one statvfs per volume per tick",
        "Bytes used on a volume."
    ),
    m!(
        "volume.size_bytes",
        "Volume size",
        Bytes,
        Gauge,
        Standard,
        Volume,
        [],
        "noded sampler: statvfs",
        "one statvfs per volume per tick",
        "Size of a volume."
    ),
    m!(
        "svc.cpu_pct",
        "Service CPU",
        Percent,
        Gauge,
        Basic,
        Service,
        [],
        "noded sampler: /proc/<pid>/stat",
        PREAD,
        "CPU share of one of the plaza's own processes."
    ),
    m!(
        "svc.rss_mib",
        "Service memory",
        Mib,
        Gauge,
        Basic,
        Service,
        [],
        "noded sampler: /proc/<pid>/status",
        PREAD,
        "Resident memory of one of the plaza's own processes."
    ),
    m!(
        "vm.count",
        "VMs",
        Count,
        Gauge,
        Basic,
        Node,
        ["state"],
        "noded registry",
        ATOMIC,
        "VMs on a node, by state: running, paused, asleep."
    ),
    // VMs.
    m!(
        "vm.cpu_pct",
        "VM CPU",
        Percent,
        Gauge,
        Basic,
        Vm,
        [],
        "noded sampler: the VMM process",
        PREAD,
        "CPU share of a VM's monitor process."
    ),
    m!(
        "vm.rss_mib",
        "VMM memory",
        Mib,
        Gauge,
        Basic,
        Vm,
        [],
        "noded sampler: the VMM process",
        PREAD,
        "Resident memory of a VM's monitor process."
    ),
    m!(
        "vm.guest_used_mib",
        "Guest memory used",
        Mib,
        Gauge,
        Basic,
        Vm,
        [],
        "virtio-balloon statistics",
        ATOMIC,
        "Memory the guest reports in use."
    ),
    m!(
        "vm.guest_total_mib",
        "Guest memory size",
        Mib,
        Gauge,
        Basic,
        Vm,
        [],
        "virtio-balloon statistics",
        ATOMIC,
        "Memory the guest has."
    ),
    m!(
        "vm.blk_read_bps",
        "VM disk read",
        BytesPerSec,
        Gauge,
        Standard,
        Vm,
        [],
        "vmm virtio-blk counters",
        "counted on the existing request path; no extra syscalls",
        "Bytes the guest reads from its disks per second."
    ),
    m!(
        "vm.blk_write_bps",
        "VM disk write",
        BytesPerSec,
        Gauge,
        Standard,
        Vm,
        [],
        "vmm virtio-blk counters",
        "counted on the existing request path; no extra syscalls",
        "Bytes the guest writes to its disks per second."
    ),
    m!(
        "vm.blk_ops",
        "VM disk operations",
        Count,
        Counter,
        Standard,
        Vm,
        [],
        "vmm virtio-blk counters",
        ATOMIC,
        "Disk requests the guest made."
    ),
    m!(
        "vm.vsock_rx_bps",
        "vsock in",
        BytesPerSec,
        Gauge,
        Standard,
        Vm,
        [],
        "vmm vsock counters",
        ATOMIC,
        "Bytes from the hub to the guest per second."
    ),
    m!(
        "vm.vsock_tx_bps",
        "vsock out",
        BytesPerSec,
        Gauge,
        Standard,
        Vm,
        [],
        "vmm vsock counters",
        ATOMIC,
        "Bytes from the guest to the hub per second."
    ),
    m!(
        "vm.workspace_used_mib",
        "/workspace used",
        Mib,
        Gauge,
        Standard,
        Vm,
        [],
        "agent harness, every 60 s while running",
        "one statvfs per minute in the guest",
        "Space used on the guest's /workspace."
    ),
    // Lifecycle (keyed by node; a run label where there is one).
    m!(
        "life.boot_ms",
        "Boot",
        Ms,
        Histogram,
        Basic,
        Node,
        ["run"],
        "noded registry",
        ATOMIC,
        "Spawn until the harness says ready."
    ),
    m!(
        "life.resume_ms",
        "Resume snapshot",
        Ms,
        Histogram,
        Basic,
        Node,
        ["run"],
        "noded registry",
        ATOMIC,
        "Restoring a memory snapshot."
    ),
    m!(
        "life.wake_ms",
        "Wake",
        Ms,
        Histogram,
        Basic,
        Node,
        ["run"],
        "hub wake path and noded",
        ATOMIC,
        "Wake request until the agent is ready."
    ),
    m!(
        "life.wake_restore_ms",
        "Wake: restore",
        Ms,
        Histogram,
        Basic,
        Node,
        ["run"],
        "noded registry",
        ATOMIC,
        "The snapshot-restore part of a wake."
    ),
    m!(
        "life.wake_ready_ms",
        "Wake: harness ready",
        Ms,
        Histogram,
        Basic,
        Node,
        ["run"],
        "hub wake path",
        ATOMIC,
        "The harness-ready part of a wake."
    ),
    m!(
        "life.pause_ms",
        "Paused for snapshot",
        Ms,
        Histogram,
        Basic,
        Node,
        ["run"],
        "vmm snapshot",
        ATOMIC,
        "How long a VM was paused while its snapshot was taken."
    ),
    m!(
        "life.snapshot_ms",
        "Snapshot written",
        Ms,
        Histogram,
        Basic,
        Node,
        ["run"],
        "noded registry",
        ATOMIC,
        "Time to write a snapshot."
    ),
    m!(
        "life.snapshot_mib",
        "Snapshot size",
        Mib,
        Gauge,
        Basic,
        Node,
        ["run"],
        "noded registry",
        ATOMIC,
        "Size of the latest memory snapshot."
    ),
    m!(
        "life.fork_ms",
        "Template fork",
        Ms,
        Histogram,
        Basic,
        Node,
        [],
        "hub sandbox templates",
        ATOMIC,
        "Forking a sandbox from a template snapshot."
    ),
    m!(
        "life.sandbox_create_ms",
        "Sandbox create",
        Ms,
        Histogram,
        Basic,
        Node,
        [],
        "hub sandboxes",
        ATOMIC,
        "Creating a sandbox."
    ),
    m!(
        "life.move_ms",
        "Agent move",
        Ms,
        Histogram,
        Basic,
        Node,
        [],
        "hub placement",
        ATOMIC,
        "Moving an agent between nodes."
    ),
    // The hub.
    m!(
        "hub.http_ms",
        "API latency",
        Ms,
        Histogram,
        Basic,
        Route,
        [],
        "one timer around the hub router",
        ATOMIC,
        "Time to answer an API request, per route family."
    ),
    m!(
        "hub.http_requests",
        "API requests",
        Count,
        Counter,
        Basic,
        Route,
        [],
        "one timer around the hub router",
        ATOMIC,
        "API requests, per route family."
    ),
    m!(
        "hub.http_errors",
        "API errors",
        Count,
        Counter,
        Basic,
        Route,
        [],
        "one timer around the hub router",
        ATOMIC,
        "API requests answered with a 5xx status."
    ),
    m!(
        "hub.pg_ms",
        "Postgres query time",
        Ms,
        Histogram,
        Basic,
        Hub,
        [],
        "a timer around the hub's database client",
        ATOMIC,
        "Time of each database round trip."
    ),
    m!(
        "hub.wake_queue",
        "Wakes waiting",
        Count,
        Gauge,
        Basic,
        Hub,
        ["lane"],
        "the hub's wake queue, read each tick",
        ATOMIC,
        "Agent wakes waiting for a worker, by lane (interactive, background)."
    ),
    m!(
        "life.wake_cold",
        "Cold wakes",
        Count,
        Counter,
        Basic,
        Node,
        [],
        "noded's wake path",
        ATOMIC,
        "Wakes that could not restore their snapshot and booted the VM cold instead."
    ),
    m!(
        "vsock.sessions",
        "Guest sessions",
        Count,
        Gauge,
        Basic,
        Node,
        [],
        "vsockd's /v1/metrics on the node, read each tick",
        "one loopback request per tick",
        "Guest sessions open on the node's vsockd."
    ),
    m!(
        "vsock.session_wakeups",
        "Session wake-ups",
        Count,
        Counter,
        Basic,
        Node,
        [],
        "vsockd's /v1/metrics on the node, read each tick",
        "one loopback request per tick",
        "Times the node's keepalive sessions woke up: per open session, the idle cost of an agent."
    ),
    m!(
        "hub.pg_in_use",
        "Postgres connections in use",
        Count,
        Gauge,
        Basic,
        Hub,
        [],
        "the hub's database client",
        ATOMIC,
        "Database connections busy."
    ),
    m!(
        "hub.bus_lag_ms",
        "Event bus lag",
        Ms,
        Histogram,
        Basic,
        Hub,
        [],
        "the hub event bus",
        ATOMIC,
        "Time from an event being published to it being written to a console."
    ),
    m!(
        "hub.ws_clients",
        "Open console sockets",
        Count,
        Gauge,
        Basic,
        Hub,
        [],
        "the hub event bus",
        ATOMIC,
        "WebSocket subscribers."
    ),
    m!(
        "hub.events",
        "Events",
        Count,
        Counter,
        Basic,
        Hub,
        [],
        "the hub event bus",
        ATOMIC,
        "Events published."
    ),
    m!(
        "link.beat_lag_ms",
        "Heartbeat lag",
        Ms,
        Histogram,
        Basic,
        Node,
        [],
        "node heartbeats",
        ATOMIC,
        "Time between a node's heartbeats beyond the expected 15 s."
    ),
    m!(
        "link.rtt_ms",
        "Round trip",
        Ms,
        Gauge,
        Basic,
        Node,
        [],
        "node heartbeats",
        ATOMIC,
        "Round trip between the hub and a node."
    ),
    m!(
        "link.clock_offset_ms",
        "Clock offset",
        Ms,
        Gauge,
        Basic,
        Node,
        [],
        "node heartbeats",
        ATOMIC,
        "How far a node's clock is from the hub's."
    ),
    m!(
        "link.commands_queued",
        "Commands queued",
        Count,
        Gauge,
        Basic,
        Node,
        [],
        "node command queue",
        ATOMIC,
        "Commands waiting for a node to collect them."
    ),
    m!(
        "link.payload_bytes",
        "Heartbeat payload",
        Bytes,
        Histogram,
        Basic,
        Node,
        [],
        "node heartbeats",
        ATOMIC,
        "Size of each heartbeat body."
    ),
    m!(
        "ext.latency_ms",
        "Outside service latency",
        Ms,
        Histogram,
        Basic,
        Service,
        [],
        "the hub's calls to outside services",
        ATOMIC,
        "Latency of calls from the hub to a provider API, an MCP server or the object store."
    ),
    m!(
        "ext.errors",
        "Outside service errors",
        Count,
        Counter,
        Basic,
        Service,
        [],
        "the hub's calls to outside services",
        ATOMIC,
        "Failed calls to an outside service."
    ),
    m!(
        "store.bytes",
        "Store size",
        Bytes,
        Gauge,
        Standard,
        Service,
        [],
        "hourly size reads of Garage, Forgejo and Postgres",
        "one size query per store per hour",
        "Size of a plaza store: objects, git, database tables (the metrics tables listed apart)."
    ),
    m!(
        "treasury.credits",
        "Treasury credits",
        Count,
        Gauge,
        Basic,
        Global,
        [],
        "the treasury journal",
        ATOMIC,
        "Plaza credits left."
    ),
    // The metrics system's own cost.
    m!(
        "self.cpu_pct",
        "Metrics CPU",
        Percent,
        Gauge,
        Off,
        Service,
        [],
        "the metrics system measuring itself",
        ATOMIC,
        "CPU spent collecting, rolling up and answering metrics."
    ),
    m!(
        "self.mem_mb",
        "Metrics memory",
        Mib,
        Gauge,
        Off,
        Service,
        [],
        "the metrics system measuring itself",
        ATOMIC,
        "Memory the metrics system holds."
    ),
    m!(
        "self.points_in",
        "Points in",
        Count,
        Counter,
        Off,
        Hub,
        [],
        "the metrics system measuring itself",
        ATOMIC,
        "Points ingested."
    ),
    m!(
        "self.series",
        "Series",
        Count,
        Gauge,
        Off,
        Hub,
        [],
        "the metrics system measuring itself",
        ATOMIC,
        "Live series."
    ),
    m!(
        "self.cache_hit_pct",
        "Query cache hit",
        Percent,
        Gauge,
        Off,
        Hub,
        [],
        "the metrics system measuring itself",
        ATOMIC,
        "Queries answered from the cache."
    ),
    m!(
        "self.pg_bytes_day",
        "Metrics storage growth",
        Bytes,
        Gauge,
        Off,
        Hub,
        [],
        "the metrics system measuring itself",
        ATOMIC,
        "Bytes the metrics tables grew by in the last day."
    ),
    m!(
        "self.alert_eval_ms",
        "Alert evaluation",
        Ms,
        Histogram,
        Off,
        Hub,
        [],
        "the metrics system measuring itself",
        ATOMIC,
        "Time spent checking alert rules as points arrive."
    ),
    m!(
        "self.traces_bytes",
        "Kept traces",
        Bytes,
        Gauge,
        Off,
        Hub,
        [],
        "the metrics system measuring itself",
        ATOMIC,
        "Bytes of kept traces."
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_ids_are_unique_and_carry_at_most_two_labels() {
        let mut ids: Vec<&str> = CATALOG.iter().map(|m| m.id).collect();
        ids.sort();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate metric id");
        assert!(CATALOG.iter().all(|m| m.labels.len() <= crate::MAX_LABELS));
    }
}
