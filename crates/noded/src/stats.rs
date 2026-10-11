//! Live resource stats for this machine. A background sampler reads `/proc`
//! every [`SAMPLE_INTERVAL`] and keeps the latest sample and the last
//! [`HISTORY_LEN`] of them. It reads `/proc` only; no process is spawned.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// How often the host is sampled.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);

/// How many samples the history keeps.
pub const HISTORY_LEN: usize = 60;

/// One host sample.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Sample {
    /// When the sample was taken, in milliseconds since the Unix epoch.
    pub t_ms: u64,
    /// Busy CPU since the previous sample, in percent of all CPU time.
    pub cpu_percent: f64,
    /// The 1-, 5- and 15-minute load averages.
    pub load: [f64; 3],
    pub mem_total_mib: u64,
    /// `MemTotal` minus `MemAvailable`.
    pub mem_used_mib: u64,
    pub uptime_s: f64,
}

/// `GET /v1/machine/stats`.
#[derive(Debug, Serialize)]
pub struct StatsView {
    pub cpu_percent: f64,
    pub load: [f64; 3],
    pub mem_total_mib: u64,
    pub mem_used_mib: u64,
    pub uptime_s: f64,
    /// VMs in the `running` state.
    pub vms_running: u64,
    /// Guest memory those VMs were booted with, in MiB.
    pub guest_mem_mib: u64,
    pub history: Vec<HistoryPoint>,
}

/// One history entry.
#[derive(Debug, Serialize)]
pub struct HistoryPoint {
    pub t_ms: u64,
    pub cpu_percent: f64,
    pub mem_used_mib: u64,
}

/// The latest sample and the history, shared between the sampler and the API.
#[derive(Default)]
pub struct Stats {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    latest: Option<Sample>,
    history: VecDeque<Sample>,
}

impl Stats {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn push(&self, sample: Sample) {
        let mut state = self.lock();
        if state.history.len() == HISTORY_LEN {
            state.history.pop_front();
        }
        state.history.push_back(sample);
        state.latest = Some(sample);
    }

    /// The API body, with the VM counts the caller read from the registry,
    /// or `None` before the first sample has been taken.
    pub fn view(&self, vms_running: u64, guest_mem_mib: u64) -> Option<StatsView> {
        let state = self.lock();
        let latest = state.latest?;
        Some(StatsView {
            cpu_percent: latest.cpu_percent,
            load: latest.load,
            mem_total_mib: latest.mem_total_mib,
            mem_used_mib: latest.mem_used_mib,
            uptime_s: latest.uptime_s,
            vms_running,
            guest_mem_mib,
            history: state
                .history
                .iter()
                .map(|sample| HistoryPoint {
                    t_ms: sample.t_ms,
                    cpu_percent: sample.cpu_percent,
                    mem_used_mib: sample.mem_used_mib,
                })
                .collect(),
        })
    }
}

/// Starts the sampler thread, which pushes one host sample into `stats` every
/// [`SAMPLE_INTERVAL`] for the life of the process.
pub fn spawn(stats: Arc<Stats>) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("noded-stats".into())
        .spawn(move || run(&stats))
        .map(|_handle| ())
}

fn run(stats: &Stats) {
    let mut prev = cpu_line();
    loop {
        std::thread::sleep(SAMPLE_INTERVAL);
        let now = cpu_line();
        let cpu = match (&prev, &now) {
            (Some(before), Some(after)) => cpu_percent(before, after),
            _ => None,
        };
        prev = now;
        if let Some(sample) = cpu.and_then(read_host) {
            stats.push(sample);
        }
    }
}

/// The aggregate counters of a `cpu` line in `/proc/stat`: `total` is every
/// counted field, `busy` is `total` minus idle and iowait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CpuTimes {
    total: u64,
    busy: u64,
}

fn parse_cpu_line(line: &str) -> Option<CpuTimes> {
    let mut fields = line.split_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    // user nice system idle iowait irq softirq steal. The guest fields that
    // follow are already counted inside user, so they are left out.
    let values: Vec<u64> = fields.take(8).map(|field| field.parse().ok()).collect::<Option<_>>()?;
    if values.len() < 5 {
        return None;
    }
    let total: u64 = values.iter().sum();
    let idle = values[3] + values[4];
    Some(CpuTimes { total, busy: total - idle })
}

/// The busy share, in percent, between two `cpu` lines of `/proc/stat`. `None`
/// when a line does not parse, the counters went backwards, or no time passed.
pub fn cpu_percent(before: &str, after: &str) -> Option<f64> {
    let (before, after) = (parse_cpu_line(before)?, parse_cpu_line(after)?);
    let total = after.total.checked_sub(before.total)?;
    let busy = after.busy.checked_sub(before.busy)?;
    if total == 0 {
        return None;
    }
    Some(100.0 * busy as f64 / total as f64)
}

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn cpu_line() -> Option<String> {
    read("/proc/stat")?.lines().find(|line| line.starts_with("cpu ")).map(str::to_string)
}

/// `(MemTotal, MemAvailable)` in MiB, from `/proc/meminfo` text.
fn parse_meminfo(text: &str) -> Option<(u64, u64)> {
    let kib = |key: &str| {
        text.lines().find_map(|line| {
            line.strip_prefix(key)?.trim().strip_suffix("kB")?.trim().parse::<u64>().ok()
        })
    };
    Some((kib("MemTotal:")? / 1024, kib("MemAvailable:")? / 1024))
}

fn parse_loadavg(text: &str) -> Option<[f64; 3]> {
    let mut fields = text.split_whitespace().map(|field| field.parse::<f64>().ok());
    Some([fields.next()??, fields.next()??, fields.next()??])
}

fn parse_uptime(text: &str) -> Option<f64> {
    text.split_whitespace().next()?.parse().ok()
}

/// A full sample, given the CPU share already worked out from `/proc/stat`.
fn read_host(cpu_percent: f64) -> Option<Sample> {
    let (mem_total_mib, mem_available_mib) = parse_meminfo(&read("/proc/meminfo")?)?;
    Some(Sample {
        t_ms: now_ms(),
        cpu_percent,
        load: parse_loadavg(&read("/proc/loadavg")?)?,
        mem_total_mib,
        mem_used_mib: mem_total_mib.saturating_sub(mem_available_mib),
        uptime_s: parse_uptime(&read("/proc/uptime")?)?,
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_percent_is_the_busy_share_of_the_delta() {
        // user nice system idle iowait irq softirq steal (guest fields ignored)
        let before = "cpu  100 0 100 800 100 0 0 0 0 0";
        // 50 more user and 50 more idle: 50 busy of 100 counted
        let after = "cpu  150 0 100 850 100 0 0 0 0 0";
        assert_eq!(cpu_percent(before, after), Some(50.0));
        // iowait counts as idle, so 100 more iowait is 0 busy
        let iowait = "cpu  100 0 100 800 200 0 0 0";
        assert_eq!(cpu_percent(before, iowait), Some(0.0));
        // no time passed, or the counters went backwards
        assert_eq!(cpu_percent(before, before), None);
        assert_eq!(cpu_percent(after, before), None);
    }
}
