//! The node's collectors, run before every metrics scrape: host disk, IO, network and pressure, volumes, GPUs, the
//! plaza's own services and each VM (CPU, RSS, guest memory, disk and vsock traffic), and VM counts by state.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use apz_metrics::{EntityKind, SeriesKey};
use serde_json::{Value, json};
use vmm::spawner::IoStats;

use crate::Node;
use crate::metrics::NodeMetrics;
use crate::registry::Detail;
use crate::stats::{PlazaUsage, StatsView, VmProcess};

/// The entity of node-level series. The hub replaces it with the node the batch came from.
const NODE: &str = "node";

/// The VM states the registry reports, in the order of the `vm.count` series.
const VM_STATES: [&str; 3] = ["running", "paused", "asleep"];

const SECTOR_BYTES: u64 = 512;
const MIB: u64 = 1024 * 1024;

/// The byte totals of the last reading of the disks and of the network, and when they were read.
static DISK_TOTALS: Mutex<Option<Totals>> = Mutex::new(None);
static NET_TOTALS: Mutex<Option<Totals>> = Mutex::new(None);

/// Each running VM's I/O totals at their last reading, by entity (`<run>/<index>`).
static VM_IO_TOTALS: Mutex<BTreeMap<String, VmIoTotals>> = Mutex::new(BTreeMap::new());

/// Sets every gauge from fresh readings (series keys as in the catalog: `crates/apz-metrics/src/catalog.rs`).
pub fn sample(node: &Node, m: &NodeMetrics, now_ms: u64) {
    let fleet = fleet(node);
    set_vm_counts(m, &fleet.counts);
    if let Some(view) = node
        .stats
        .view(fleet.running, fleet.guest_mib, &fleet.processes)
    {
        set_host_samples(m, &view);
        if let Some(plaza) = &view.plaza {
            set_plaza(m, plaza);
        }
    }
    set_io_rates(m, now_ms);
    set_vm_io(m, &fleet.io, now_ms);
    set_pressure(m);
    set_gpus(m);
    set_volumes(m);
    set_cold_wakes(m);
    set_vsockd(m);
}

/// The cold-boot fallbacks counted so far, at the last tick.
static COLD_WAKES: Mutex<u64> = Mutex::new(0);

/// `life.wake_cold`: the wakes since the last tick that booted cold after a failed restore (the node's Prometheus
/// counter, `crate::metrics::WAKE_COLD`).
fn set_cold_wakes(m: &NodeMetrics) {
    let now = crate::metrics::WAKE_COLD.get();
    let mut last = lock(&COLD_WAKES);
    let delta = now.saturating_sub(*last);
    *last = now;
    if delta > 0 {
        m.counter(node_series("life.wake_cold")).add(delta as f64);
    }
}

/// Where this host's vsockd serves its HTTP surface, and the token it takes; set once at start.
static VSOCKD: std::sync::OnceLock<(u16, Option<(&'static str, String)>)> =
    std::sync::OnceLock::new();

/// Points the collector at the host's vsockd (`APZ_VSOCKD_HTTP_PORT`, default 19080), with the machine's token.
pub fn point_at_vsockd(port: u16, header: Option<(&'static str, String)>) {
    let _ = VSOCKD.set((port, header));
}

/// vsockd's session wake-ups at the last tick.
static SESSION_WAKEUPS: Mutex<Option<u64>> = Mutex::new(None);

/// `vsock.sessions` and `vsock.session_wakeups` from vsockd's `GET /v1/metrics` on loopback. A host with no vsockd
/// (or one that does not answer within 300 ms) records nothing.
fn set_vsockd(m: &NodeMetrics) {
    let Some((port, header)) = VSOCKD.get() else {
        return;
    };
    let headers: Vec<(&str, &str)> = header.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let Ok(reply) = apz_http::client::send_with(
        *port,
        "GET",
        "/v1/metrics",
        &headers,
        None,
        std::time::Duration::from_millis(300),
    ) else {
        return;
    };
    if reply.status != 200 {
        return;
    }
    let text = String::from_utf8_lossy(&reply.body);
    let (sessions, wakeups) = vsockd_numbers(&text);
    if let Some(sessions) = sessions {
        m.gauge(node_series("vsock.sessions")).set(sessions);
    }
    if let Some(total) = wakeups {
        let total = total as u64;
        let mut last = lock(&SESSION_WAKEUPS);
        if let Some(before) = *last {
            // A restarted vsockd counts from zero again: that tick adds nothing.
            if total >= before {
                m.counter(node_series("vsock.session_wakeups"))
                    .add((total - before) as f64);
            }
        }
        *last = Some(total);
    }
}

/// The open sessions and the session wake-ups in vsockd's Prometheus text.
fn vsockd_numbers(text: &str) -> (Option<f64>, Option<f64>) {
    let value = |name: &str| {
        text.lines().find_map(|line| {
            line.strip_prefix(name)?
                .strip_prefix(' ')?
                .trim()
                .parse::<f64>()
                .ok()
        })
    };
    (
        value("apz_vsockd_sessions"),
        value("apz_vsockd_session_wakeups_total"),
    )
}

/// The node's VMs: the running count and their guest memory (counted as `GET /v1/machine/stats` counts them), the
/// count of VMs in each state, and the processes of the running VMs.
struct Fleet {
    running: u64,
    guest_mib: u64,
    counts: [u64; 3],
    processes: Vec<VmProcess>,
    io: Vec<VmIo>,
}

fn fleet(node: &Node) -> Fleet {
    let mut fleet = Fleet {
        running: 0,
        guest_mib: 0,
        counts: [0; 3],
        processes: Vec::new(),
        io: Vec::new(),
    };
    for run in node.registry().list() {
        let view = run.view(Detail::Summary);
        let pids = run.process_ids();
        let io = run.io_stats();
        for vm in &view.vms {
            if let Some(index) = VM_STATES.iter().position(|state| *state == vm.state) {
                fleet.counts[index] += 1;
            }
            if vm.state != "running" {
                continue;
            }
            fleet.running += 1;
            fleet.guest_mib += u64::from(view.memory_mib);
            let Some(&(_, pid)) = pids.iter().find(|(index, _)| *index == vm.id) else {
                continue;
            };
            if let Some((_, stats)) = io.iter().find(|(index, _)| *index == vm.id) {
                fleet.io.push(VmIo {
                    entity: format!("{}/{}", view.id, vm.id),
                    pid,
                    stats: *stats,
                });
            }
            fleet.processes.push(VmProcess {
                pid,
                run: view.id.to_string(),
                vm: vm.id,
                cid: vm.guest_cid,
                role: vm.role.clone(),
                usable_mib: vm.memory.as_ref().map(|memory| memory.usable_mib),
                guest_used_mib: vm.memory.as_ref().and_then(|memory| {
                    let available = memory.available_mib?;
                    Some(u64::from(memory.max_mib).saturating_sub(available))
                }),
            });
        }
    }
    fleet
}

/// A node-level series.
fn node_series(metric: &str) -> SeriesKey {
    SeriesKey::new(metric, EntityKind::Node, NODE)
}

fn set_vm_counts(m: &NodeMetrics, counts: &[u64; 3]) {
    for (state, count) in VM_STATES.iter().zip(counts.iter()) {
        m.gauge(node_series("vm.count").label("state", state))
            .set(*count as f64);
    }
}

fn set_host_samples(m: &NodeMetrics, view: &StatsView) {
    m.gauge(node_series("host.cpu_pct")).set(view.cpu_percent);
    m.gauge(node_series("host.mem_used_mib"))
        .set(view.mem_used_mib as f64);
    m.gauge(node_series("host.mem_total_mib"))
        .set(view.mem_total_mib as f64);
    m.gauge(node_series("host.load1")).set(view.load[0]);
}

/// The plaza's services and the running VMs' processes. A VM's entity is `<run>/<index>`.
fn set_plaza(m: &NodeMetrics, plaza: &PlazaUsage) {
    for service in &plaza.services {
        let key = |metric: &str| SeriesKey::new(metric, EntityKind::Service, service.name);
        m.gauge(key("svc.cpu_pct")).set(service.cpu_percent);
        m.gauge(key("svc.rss_mib")).set(service.rss_mib as f64);
    }
    for usage in &plaza.vms {
        let entity = format!("{}/{}", usage.run, usage.vm);
        let key = |metric: &str| SeriesKey::new(metric, EntityKind::Vm, &entity);
        m.gauge(key("vm.cpu_pct")).set(usage.cpu_percent);
        m.gauge(key("vm.rss_mib")).set(usage.rss_mib as f64);
        if let Some(used) = usage.guest_used_mib {
            m.gauge(key("vm.guest_used_mib")).set(used as f64);
        }
        if let Some(total) = usage.usable_mib {
            m.gauge(key("vm.guest_total_mib")).set(f64::from(total));
        }
    }
}

/// The rates of the disks' and the network's byte totals since the previous call. The first call, and a call
/// that finds the clock unmoved or a counter gone backwards, sets nothing.
fn set_io_rates(m: &NodeMetrics, now_ms: u64) {
    if let Some(bytes) = read_file("/proc/diskstats").and_then(|text| disk_bytes(&text)) {
        if let Some([read, written]) = rates(&mut lock(&DISK_TOTALS), now_ms, bytes) {
            m.gauge(node_series("host.disk_read_bps")).set(read);
            m.gauge(node_series("host.disk_write_bps")).set(written);
        }
    }
    if let Some(bytes) = read_file("/proc/net/dev").and_then(|text| net_bytes(&text)) {
        if let Some([received, sent]) = rates(&mut lock(&NET_TOTALS), now_ms, bytes) {
            m.gauge(node_series("host.net_rx_bps")).set(received);
            m.gauge(node_series("host.net_tx_bps")).set(sent);
        }
    }
}

/// Volumes reported at most: the node's fixed disks, largest first.
const MAX_VOLUMES: usize = 8;

/// `volume.size_bytes`, `volume.used_bytes` and `volume.used_pct` of each fixed disk, keyed by its mount point (the
/// hub prefixes the node: `n1:/var/lib/plaza`).
fn set_volumes(m: &NodeMetrics) {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let found: Vec<(String, u64, u64)> = disks
        .list()
        .iter()
        .filter(|d| !d.is_removable())
        .map(|d| {
            (
                d.mount_point().to_string_lossy().into_owned(),
                d.total_space(),
                d.available_space(),
            )
        })
        .collect();
    for (mount, size, used) in volumes(found) {
        let key = |metric: &str| SeriesKey::new(metric, EntityKind::Volume, &mount);
        m.gauge(key("volume.size_bytes")).set(size as f64);
        m.gauge(key("volume.used_bytes")).set(used as f64);
        m.gauge(key("volume.used_pct"))
            .set(used as f64 / size as f64 * 100.0);
    }
}

/// (mount, size, used) of the volumes worth reporting, from (mount, total, available): sized ones, each mount once,
/// largest first, at most [`MAX_VOLUMES`].
fn volumes(found: Vec<(String, u64, u64)>) -> Vec<(String, u64, u64)> {
    let mut out: Vec<(String, u64, u64)> = Vec::new();
    for (mount, total, available) in found {
        if total == 0 || out.iter().any(|(m, ..)| *m == mount) {
            continue;
        }
        out.push((mount, total, total.saturating_sub(available)));
    }
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out.truncate(MAX_VOLUMES);
    out
}

fn set_pressure(m: &NodeMetrics) {
    for (metric, resource) in [
        ("host.psi_cpu_pct", "cpu"),
        ("host.psi_mem_pct", "memory"),
        ("host.psi_io_pct", "io"),
    ] {
        let path = format!("/proc/pressure/{resource}");
        if let Some(share) = read_file(path).and_then(|text| pressure_some_avg10(&text)) {
            m.gauge(node_series(metric)).set(share);
        }
    }
}

/// One GPU's sysfs readings (AMD's driver exposes them). A reading the card does not have is None.
#[derive(Debug, Clone, Copy, PartialEq)]
struct GpuReadings {
    index: u32,
    util_pct: Option<f64>,
    vram_used: Option<u64>,
    vram_total: Option<u64>,
}

impl GpuReadings {
    /// The VRAM in use, in percent of the total, when both are known and the total is not zero.
    fn vram_percent(&self) -> Option<f64> {
        let total = self.vram_total.filter(|total| *total > 0)?;
        Some((self.vram_used? as f64) * 100.0 / total as f64)
    }
}

/// The GPU gauges, and the `gpus` report the console lists (`name`, `util_pct`, `mem_used_mb`, `mem_total_mb`).
fn set_gpus(m: &NodeMetrics) {
    let gpus = read_gpus();
    for gpu in &gpus {
        let index = gpu.index.to_string();
        if let Some(util) = gpu.util_pct {
            m.gauge(node_series("host.gpu_util_pct").label("gpu", &index))
                .set(util);
        }
        if let Some(percent) = gpu.vram_percent() {
            m.gauge(node_series("host.gpu_mem_pct").label("gpu", &index))
                .set(percent);
        }
    }
    let report: Vec<Value> = gpus
        .iter()
        .map(|gpu| {
            json!({
                "name": format!("GPU {}", gpu.index),
                "util_pct": gpu.util_pct,
                "mem_used_mb": gpu.vram_used.map(|bytes| bytes / MIB),
                "mem_total_mb": gpu.vram_total.map(|bytes| bytes / MIB),
            })
        })
        .collect();
    m.report("gpus", Value::Array(report));
}

/// The cards in `/sys/class/drm` (`card0`, `card1`, …; connectors such as `card0-HDMI-A-1` are skipped).
fn read_gpus() -> Vec<GpuReadings> {
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };
    let mut gpus: Vec<GpuReadings> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let index: u32 = entry
                .file_name()
                .to_str()?
                .strip_prefix("card")?
                .parse()
                .ok()?;
            let device = entry.path().join("device");
            Some(GpuReadings {
                index,
                util_pct: read_file(device.join("gpu_busy_percent"))
                    .and_then(|text| parse_number(&text)),
                vram_used: read_file(device.join("mem_info_vram_used"))
                    .and_then(|text| parse_number(&text)),
                vram_total: read_file(device.join("mem_info_vram_total"))
                    .and_then(|text| parse_number(&text)),
            })
        })
        .collect();
    gpus.sort_by_key(|gpu| gpu.index);
    gpus
}

/// A byte total at the time it was read.
#[derive(Debug, Clone, Copy)]
struct Totals {
    at_ms: u64,
    bytes: [u64; 2],
}

/// The rate of each of two totals since the previous reading, in bytes per second, and stores this reading for the
/// next call. None on the first reading, when the clock has not moved, or when a total went backwards.
fn rates(previous: &mut Option<Totals>, now_ms: u64, bytes: [u64; 2]) -> Option<[f64; 2]> {
    let before = previous.replace(Totals {
        at_ms: now_ms,
        bytes,
    })?;
    rates_since(before, now_ms, bytes)
}

/// The rate of each of two totals between the reading `before` and the reading `bytes` at `now_ms`, in bytes per
/// second. None when the clock has not moved or a total went backwards.
fn rates_since(before: Totals, now_ms: u64, bytes: [u64; 2]) -> Option<[f64; 2]> {
    let elapsed_ms = now_ms.checked_sub(before.at_ms).filter(|ms| *ms > 0)?;
    let rate = |index: usize| -> Option<f64> {
        let delta = bytes[index].checked_sub(before.bytes[index])?;
        Some(delta as f64 * 1000.0 / elapsed_ms as f64)
    };
    Some([rate(0)?, rate(1)?])
}

/// One running VM's I/O counters, with the process that reported them.
struct VmIo {
    entity: String,
    pid: u32,
    stats: IoStats,
}

/// A VM's I/O totals at a reading, and the process that reported them. A new process starts its counters over, so a
/// reading from another process is never compared with these.
#[derive(Debug, Clone, Copy)]
struct VmIoTotals {
    pid: u32,
    blk: Totals,
    vsock: Totals,
    ops: u64,
}

/// The rates a VM's totals make between two readings of the same process.
#[derive(Debug, Clone, Copy, PartialEq)]
struct VmIoRates {
    blk_read_bps: f64,
    blk_write_bps: f64,
    vsock_rx_bps: f64,
    vsock_tx_bps: f64,
}

/// What one reading of a VM's counters makes: its new totals, the disk requests since the previous reading (all of
/// them on a first reading, or after a new process), and the rates when a previous reading of the same process exists.
#[derive(Debug, Clone, Copy)]
struct VmIoStep {
    totals: VmIoTotals,
    new_ops: u64,
    rates: Option<VmIoRates>,
}

fn vm_io_step(previous: Option<VmIoTotals>, pid: u32, now_ms: u64, stats: IoStats) -> VmIoStep {
    let blk = [stats.blk_read_bytes, stats.blk_write_bytes];
    let vsock = [stats.vsock_rx_bytes, stats.vsock_tx_bytes];
    let totals = VmIoTotals {
        pid,
        blk: Totals {
            at_ms: now_ms,
            bytes: blk,
        },
        vsock: Totals {
            at_ms: now_ms,
            bytes: vsock,
        },
        ops: stats.blk_ops,
    };
    let before = previous.filter(|before| before.pid == pid);
    let new_ops = match before {
        Some(before) => stats
            .blk_ops
            .checked_sub(before.ops)
            .unwrap_or(stats.blk_ops),
        None => stats.blk_ops,
    };
    let rates = before.and_then(|before| {
        let [blk_read_bps, blk_write_bps] = rates_since(before.blk, now_ms, blk)?;
        let [vsock_rx_bps, vsock_tx_bps] = rates_since(before.vsock, now_ms, vsock)?;
        Some(VmIoRates {
            blk_read_bps,
            blk_write_bps,
            vsock_rx_bps,
            vsock_tx_bps,
        })
    });
    VmIoStep {
        totals,
        new_ops,
        rates,
    }
}

/// Sets each running VM's disk and vsock rates, and adds its new disk requests to the request counter, from the
/// readings `fleet` took. A VM that is no longer running drops out of the totals.
fn set_vm_io(m: &NodeMetrics, readings: &[VmIo], now_ms: u64) {
    let mut totals = lock(&VM_IO_TOTALS);
    let mut next = BTreeMap::new();
    for reading in readings {
        let previous = totals.get(&reading.entity).copied();
        let step = vm_io_step(previous, reading.pid, now_ms, reading.stats);
        let key = |metric: &str| SeriesKey::new(metric, EntityKind::Vm, &reading.entity);
        m.counter(key("vm.blk_ops")).add(step.new_ops as f64);
        if let Some(rates) = step.rates {
            m.gauge(key("vm.blk_read_bps")).set(rates.blk_read_bps);
            m.gauge(key("vm.blk_write_bps")).set(rates.blk_write_bps);
            m.gauge(key("vm.vsock_rx_bps")).set(rates.vsock_rx_bps);
            m.gauge(key("vm.vsock_tx_bps")).set(rates.vsock_tx_bps);
        }
        next.insert(reading.entity.clone(), step.totals);
    }
    *totals = next;
}

/// The bytes read and written by the whole disks in `/proc/diskstats` text. Partitions and loop, RAM, zram and
/// device-mapper devices are left out, so a disk is not counted twice. None when the text lists no device.
fn disk_bytes(text: &str) -> Option<[u64; 2]> {
    let rows: Vec<(&str, u64, u64)> = text
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let name = *fields.get(2)?;
            let read_sectors: u64 = fields.get(5)?.parse().ok()?;
            let written_sectors: u64 = fields.get(9)?.parse().ok()?;
            Some((name, read_sectors, written_sectors))
        })
        .collect();
    if rows.is_empty() {
        return None;
    }
    let names: Vec<&str> = rows.iter().map(|row| row.0).collect();
    let mut total = [0_u64; 2];
    for (name, read_sectors, written_sectors) in &rows {
        if is_whole_disk(name, &names) {
            total[0] = total[0].saturating_add(read_sectors.saturating_mul(SECTOR_BYTES));
            total[1] = total[1].saturating_add(written_sectors.saturating_mul(SECTOR_BYTES));
        }
    }
    Some(total)
}

/// Whether `name` is a whole disk among the devices `names`: not a loop, RAM, zram or device-mapper device, and
/// not a partition of another listed device.
fn is_whole_disk(name: &str, names: &[&str]) -> bool {
    if ["loop", "ram", "zram", "dm-"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
    {
        return false;
    }
    !names
        .iter()
        .any(|parent| *parent != name && is_partition_of(name, parent))
}

/// Whether `name` is a partition of `parent`: `sda1` of `sda`, `nvme0n1p1` of `nvme0n1`.
fn is_partition_of(name: &str, parent: &str) -> bool {
    let Some(rest) = name.strip_prefix(parent) else {
        return false;
    };
    let all_digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    all_digits(rest)
        || (rest.strip_prefix('p').is_some_and(all_digits)
            && parent.ends_with(|c: char| c.is_ascii_digit()))
}

/// The bytes received and sent by every network interface but `lo`, in `/proc/net/dev` text.
fn net_bytes(text: &str) -> Option<[u64; 2]> {
    let mut total = [0_u64; 2];
    let mut interfaces = 0;
    for line in text.lines() {
        let Some((name, counters)) = line.split_once(':') else {
            continue;
        };
        if name.trim() == "lo" {
            continue;
        }
        let fields: Vec<&str> = counters.split_whitespace().collect();
        let (Some(received), Some(sent)) = (
            fields.first().and_then(|field| field.parse::<u64>().ok()),
            fields.get(8).and_then(|field| field.parse::<u64>().ok()),
        ) else {
            continue;
        };
        total[0] = total[0].saturating_add(received);
        total[1] = total[1].saturating_add(sent);
        interfaces += 1;
    }
    (interfaces > 0).then_some(total)
}

/// The `some avg10` share of a pressure-stall file (`/proc/pressure/cpu` and the like), in percent.
fn pressure_some_avg10(text: &str) -> Option<f64> {
    let line = text.lines().find(|line| line.starts_with("some "))?;
    line.split_whitespace()
        .find_map(|field| field.strip_prefix("avg10=")?.parse().ok())
}

/// The text of a file, or None when it cannot be read. Systems without the file (macOS and Windows have no
/// `/proc` or `/sys`) read as None, so nothing is set.
fn read_file(path: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn parse_number<T: std::str::FromStr>(text: &str) -> Option<T> {
    text.trim().parse().ok()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vsockd_sessions_and_wakeups_are_read_from_its_prometheus_text() {
        let text = "# HELP apz_vsockd_sessions Guest sessions open\n# TYPE apz_vsockd_sessions gauge\n\
                    apz_vsockd_sessions 12\napz_vsockd_sessions_refused_total 0\n\
                    apz_vsockd_session_wakeups_total 4410\n";
        assert_eq!(vsockd_numbers(text), (Some(12.0), Some(4410.0)));
        assert_eq!(vsockd_numbers(""), (None, None));
    }

    #[test]
    fn volumes_are_the_sized_mounts_once_each_largest_first() {
        let found = vec![
            ("/".to_string(), 100, 40),
            ("/boot".to_string(), 1, 1),
            ("/var/lib/plaza".to_string(), 500, 100),
            ("/".to_string(), 100, 40),
            ("/proc".to_string(), 0, 0),
        ];
        assert_eq!(
            volumes(found),
            vec![
                ("/var/lib/plaza".to_string(), 500, 400),
                ("/".to_string(), 100, 60),
                ("/boot".to_string(), 1, 0),
            ]
        );
    }

    const DISKSTATS: &str = "\
   7       0 loop0 1 0 8 0 0 0 0 0 0 0 0
   8       0 sda 100 0 2048 0 50 0 4096 0 0 0 0
   8       1 sda1 90 0 1024 0 40 0 2048 0 0 0 0
 259       0 nvme0n1 10 0 1000 0 5 0 3000 0 0 0 0
 259       1 nvme0n1p1 4 0 500 0 2 0 1500 0 0 0 0
   1       0 ram0 0 0 4000 0 0 0 0 0 0 0 0";

    const NET_DEV: &str = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo:  500 5 0 0 0 0 0 0   500 5 0 0 0 0 0 0
  eth0: 1000 10 0 0 0 0 0 0  2000 20 0 0 0 0 0 0
wlan0: 300 3 0 0 0 0 0 0  400 4 0 0 0 0 0 0";

    #[test]
    fn disk_totals_count_whole_disks_in_bytes_and_skip_partitions_and_loop_devices() {
        assert_eq!(
            disk_bytes(DISKSTATS),
            Some([(2048 + 1000) * 512, (4096 + 3000) * 512])
        );
    }

    #[test]
    fn net_totals_add_every_interface_but_loopback() {
        assert_eq!(net_bytes(NET_DEV), Some([1000 + 300, 2000 + 400]));
    }

    #[test]
    fn the_pressure_share_is_the_avg10_of_the_some_line() {
        let cpu = "some avg10=1.25 avg60=0.80 avg300=0.40 total=123456";
        let memory = "some avg10=0.00 avg60=0.00 avg300=0.00 total=0\nfull avg10=9.99 avg60=0.00 avg300=0.00 total=1";
        assert_eq!(pressure_some_avg10(cpu), Some(1.25));
        assert_eq!(pressure_some_avg10(memory), Some(0.0));
        assert_eq!(pressure_some_avg10("garbage"), None);
    }

    #[test]
    fn a_vm_io_rate_is_the_increase_per_second_from_the_same_process() {
        let first = IoStats {
            blk_read_bytes: 1000,
            blk_write_bytes: 0,
            blk_ops: 3,
            vsock_rx_bytes: 0,
            vsock_tx_bytes: 100,
        };
        let step = vm_io_step(None, 7, 0, first);
        assert_eq!(step.rates, None, "the first reading has no rate yet");
        assert_eq!(
            step.new_ops, 3,
            "the first reading counts every request since boot"
        );

        let second = IoStats {
            blk_read_bytes: 3000,
            blk_write_bytes: 500,
            blk_ops: 5,
            vsock_rx_bytes: 2000,
            vsock_tx_bytes: 100,
        };
        let step = vm_io_step(Some(step.totals), 7, 2000, second);
        assert_eq!(
            step.rates,
            Some(VmIoRates {
                blk_read_bps: 1000.0,
                blk_write_bps: 250.0,
                vsock_rx_bps: 1000.0,
                vsock_tx_bps: 0.0,
            })
        );
        assert_eq!(step.new_ops, 2, "two requests since the previous reading");
    }

    #[test]
    fn a_vm_whose_process_changed_starts_its_io_over() {
        let before = IoStats {
            blk_read_bytes: 9000,
            blk_write_bytes: 0,
            blk_ops: 40,
            vsock_rx_bytes: 0,
            vsock_tx_bytes: 0,
        };
        let step = vm_io_step(None, 7, 0, before);
        let restarted = IoStats {
            blk_read_bytes: 100,
            blk_write_bytes: 0,
            blk_ops: 1,
            vsock_rx_bytes: 0,
            vsock_tx_bytes: 0,
        };
        let step = vm_io_step(Some(step.totals), 8, 2000, restarted);
        assert_eq!(
            step.rates, None,
            "a new process has no reading of its own to compare with"
        );
        assert_eq!(step.new_ops, 1, "a new process's requests are all new");
    }

    #[test]
    fn a_rate_is_the_increase_per_second_since_the_previous_reading() {
        let mut previous = None;
        assert_eq!(rates(&mut previous, 1_000, [100, 0]), None);
        assert_eq!(rates(&mut previous, 3_000, [500, 40]), Some([200.0, 20.0]));
    }
}
