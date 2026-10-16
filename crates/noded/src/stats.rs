//! Live resource stats for this machine. A background sampler reads the host
//! every [`SAMPLE_INTERVAL`] and keeps the latest sample and the last
//! [`HISTORY_LEN`] of them, plus the latest reading of every process, so the
//! API can say where the machine's CPU and memory go. On Linux the sampler
//! reads `/proc`; on other systems it asks the OS through `sysinfo`. No process
//! is spawned.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
#[cfg(not(target_os = "linux"))]
use sysinfo::{Process, ProcessesToUpdate, System};

/// How often the host is sampled.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);

/// Bytes in a MiB, for the sysinfo readings (which are in bytes).
#[cfg(not(target_os = "linux"))]
const BYTES_PER_MIB: u64 = 1024 * 1024;

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
    /// Where the plaza's CPU and memory go: its services and agent VMs. `None`
    /// until the sampler has read the processes.
    pub plaza: Option<PlazaUsage>,
}

/// One history entry.
#[derive(Debug, Serialize)]
pub struct HistoryPoint {
    pub t_ms: u64,
    pub cpu_percent: f64,
    pub mem_used_mib: u64,
}

/// The plaza's processes: its services and its running agent VMs.
#[derive(Debug, PartialEq, Serialize)]
pub struct PlazaUsage {
    pub services: Vec<ServiceUsage>,
    pub vms: Vec<VmUsage>,
    pub total: TotalUsage,
}

/// One plaza service process.
#[derive(Debug, PartialEq, Serialize)]
pub struct ServiceUsage {
    /// `apid`, `hub` (the `vsockd` process), `noded` or `garage`.
    pub name: &'static str,
    pub pid: u32,
    /// Busy share since the previous sample, in percent of all CPU time.
    pub cpu_percent: f64,
    pub rss_mib: u64,
}

/// One running agent VM's process.
#[derive(Debug, PartialEq, Serialize)]
pub struct VmUsage {
    /// `vm <run>/<index>`.
    pub name: String,
    pub pid: u32,
    pub cpu_percent: f64,
    pub rss_mib: u64,
    pub run: String,
    pub vm: u32,
    pub cid: Option<u64>,
    pub role: Option<String>,
    pub usable_mib: Option<u32>,
    pub guest_used_mib: Option<u64>,
}

/// The plaza's totals: its services and its VMs together, and each apart.
#[derive(Debug, PartialEq, Serialize)]
pub struct TotalUsage {
    pub cpu_percent: f64,
    pub rss_mib: u64,
    pub services_cpu_percent: f64,
    pub services_rss_mib: u64,
    pub vms_cpu_percent: f64,
    pub vms_rss_mib: u64,
}

/// A running agent VM as the caller read it from the registry: its process and
/// the metadata the plaza's usage reports beside it.
#[derive(Debug, Clone, PartialEq)]
pub struct VmProcess {
    /// The VM's process id.
    pub pid: u32,
    /// The run the VM belongs to (`r<seq>`).
    pub run: String,
    /// The VM's index among the run's VMs.
    pub vm: u32,
    /// The VM's vsock guest CID, when it has one.
    pub cid: Option<u64>,
    /// The role the VM's guest runs as, when it runs as one.
    pub role: Option<String>,
    /// The VM's usable guest memory in MiB, when it has a balloon.
    pub usable_mib: Option<u32>,
    /// Guest memory in use in MiB (`max_mib` minus `available_mib`), once the
    /// guest has reported its available memory.
    pub guest_used_mib: Option<u64>,
}

/// What the sampler last read of one process.
#[derive(Debug, Clone, PartialEq)]
struct ProcSample {
    /// The command name (`/proc/<pid>/comm`).
    comm: String,
    /// The process's CPU ticks (`utime + stime`) when it was read.
    #[cfg(target_os = "linux")]
    ticks: u64,
    /// Busy share since the previous sample, in percent of all CPU time.
    cpu_percent: f64,
    rss_mib: u64,
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
    procs: Option<HashMap<u32, ProcSample>>,
}

impl Stats {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn push(&self, sample: Sample) {
        let mut state = self.lock();
        if state.history.len() == HISTORY_LEN {
            state.history.pop_front();
        }
        state.history.push_back(sample);
        state.latest = Some(sample);
    }

    fn set_procs(&self, procs: HashMap<u32, ProcSample>) {
        self.lock().procs = Some(procs);
    }

    /// The API body, with the VM counts and VM processes the caller read from
    /// the registry, or `None` before the first sample has been taken.
    pub fn view(
        &self,
        vms_running: u64,
        guest_mem_mib: u64,
        vms: &[VmProcess],
    ) -> Option<StatsView> {
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
            plaza: state.procs.as_ref().map(|procs| plaza_usage(procs, vms)),
        })
    }
}

/// Starts the sampler thread, which pushes one host sample into `stats` every
/// [`SAMPLE_INTERVAL`] and reads every process, for the life of the process.
pub fn spawn(stats: Arc<Stats>) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("noded-stats".into())
        .spawn(move || run(&stats))
        .map(|_handle| ())
}

#[cfg(target_os = "linux")]
fn run(stats: &Stats) {
    let mut prev_cpu = cpu_line();
    let mut prev_ticks: HashMap<u32, u64> = HashMap::new();
    loop {
        std::thread::sleep(SAMPLE_INTERVAL);
        let now_cpu = cpu_line();
        let cpu = match (&prev_cpu, &now_cpu) {
            (Some(before), Some(after)) => cpu_percent(before, after),
            _ => None,
        };
        let total = match (
            prev_cpu.as_deref().and_then(total_jiffies),
            now_cpu.as_deref().and_then(total_jiffies),
        ) {
            (Some(before), Some(after)) => after.checked_sub(before),
            _ => None,
        };
        let procs = read_processes(&prev_ticks, total);
        prev_ticks = procs
            .iter()
            .map(|(pid, sample)| (*pid, sample.ticks))
            .collect();
        prev_cpu = now_cpu;
        if let Some(sample) = cpu.and_then(read_host) {
            stats.push(sample);
        }
        stats.set_procs(procs);
    }
}

/// The sampler on systems without `/proc`: `sysinfo` reads the host's CPU and
/// memory, and every process, once per [`SAMPLE_INTERVAL`].
#[cfg(not(target_os = "linux"))]
fn run(stats: &Stats) {
    let mut system = System::new();
    // A CPU share is worked out between two refreshes, so refresh once before
    // the first sample.
    system.refresh_cpu_usage();
    system.refresh_processes(ProcessesToUpdate::All, true);
    loop {
        std::thread::sleep(SAMPLE_INTERVAL);
        system.refresh_cpu_usage();
        system.refresh_memory();
        system.refresh_processes(ProcessesToUpdate::All, true);
        stats.push(host_sample(&system));
        stats.set_procs(process_samples(&system));
    }
}

/// A full sample of the host, from the CPU, memory and uptime `system` holds.
/// The load averages are zeros on Windows, which has none.
#[cfg(not(target_os = "linux"))]
fn host_sample(system: &System) -> Sample {
    let load = System::load_average();
    Sample {
        t_ms: now_ms(),
        cpu_percent: f64::from(system.global_cpu_usage()),
        load: [load.one, load.five, load.fifteen],
        mem_total_mib: system.total_memory() / BYTES_PER_MIB,
        mem_used_mib: system.used_memory() / BYTES_PER_MIB,
        uptime_s: System::uptime() as f64,
    }
}

/// Every process `system` lists, keyed by pid. `sysinfo` reports a process's
/// CPU per core, so it is divided by the number of CPUs to give the share of
/// all CPU time that `/proc` gives on Linux.
#[cfg(not(target_os = "linux"))]
fn process_samples(system: &System) -> HashMap<u32, ProcSample> {
    let cpus = system.cpus().len().max(1) as f32;
    system
        .processes()
        .iter()
        .map(|(pid, process)| {
            (
                pid.as_u32(),
                ProcSample {
                    comm: process_name(process),
                    cpu_percent: f64::from(process.cpu_usage() / cpus),
                    rss_mib: process.memory() / BYTES_PER_MIB,
                },
            )
        })
        .collect()
}

/// The command name of a process. Windows names an executable with its
/// `.exe` extension, which the plaza's service names do not carry.
#[cfg(not(target_os = "linux"))]
fn process_name(process: &Process) -> String {
    let name = process.name().to_string_lossy();
    name.strip_suffix(".exe").unwrap_or(&name).to_string()
}

/// The aggregate counters of a `cpu` line in `/proc/stat`: `total` is every
/// counted field, `busy` is `total` minus idle and iowait.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CpuTimes {
    total: u64,
    busy: u64,
}

#[cfg(target_os = "linux")]
fn parse_cpu_line(line: &str) -> Option<CpuTimes> {
    let mut fields = line.split_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    // user nice system idle iowait irq softirq steal. The guest fields that
    // follow are already counted inside user, so they are left out.
    let values: Vec<u64> = fields
        .take(8)
        .map(|field| field.parse().ok())
        .collect::<Option<_>>()?;
    if values.len() < 5 {
        return None;
    }
    let total: u64 = values.iter().sum();
    let idle = values[3] + values[4];
    Some(CpuTimes {
        total,
        busy: total - idle,
    })
}

/// The machine's total CPU ticks (every counted field, every CPU) on a `cpu`
/// line of `/proc/stat`.
#[cfg(target_os = "linux")]
fn total_jiffies(line: &str) -> Option<u64> {
    parse_cpu_line(line).map(|times| times.total)
}

/// The busy share, in percent, between two `cpu` lines of `/proc/stat`. `None`
/// when a line does not parse, the counters went backwards, or no time passed.
#[cfg(target_os = "linux")]
pub fn cpu_percent(before: &str, after: &str) -> Option<f64> {
    let (before, after) = (parse_cpu_line(before)?, parse_cpu_line(after)?);
    let total = after.total.checked_sub(before.total)?;
    let busy = after.busy.checked_sub(before.busy)?;
    if total == 0 {
        return None;
    }
    Some(100.0 * busy as f64 / total as f64)
}

/// A process's busy share, in percent of all CPU time, between two samples: its
/// ticks over the machine's ticks (every CPU) in the same span. `None` when the
/// process's counter went backwards or no machine time passed.
#[cfg(target_os = "linux")]
fn process_cpu_percent(before_ticks: u64, after_ticks: u64, total_ticks: u64) -> Option<f64> {
    let ticks = after_ticks.checked_sub(before_ticks)?;
    if total_ticks == 0 {
        return None;
    }
    Some(100.0 * ticks as f64 / total_ticks as f64)
}

/// The command name and CPU ticks (`utime + stime`, fields 14 and 15) of a
/// `/proc/<pid>/stat` line. The name sits in parentheses and may hold spaces
/// and parentheses, so the fields are counted from the last `)`.
#[cfg(target_os = "linux")]
fn parse_proc_stat(line: &str) -> Option<(String, u64)> {
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    let comm = line.get(open + 1..close)?.to_string();
    // The state (field 3) is the first field after the `)`, so field 14 is
    // index 11 and field 15 is index 12.
    let fields: Vec<&str> = line.get(close + 1..)?.split_whitespace().collect();
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some((comm, utime.saturating_add(stime)))
}

/// The resident memory in MiB, from the `VmRSS` line (in kB) of a
/// `/proc/<pid>/status` text. `None` for a process with no resident memory,
/// such as a kernel thread.
#[cfg(target_os = "linux")]
fn parse_vm_rss_mib(text: &str) -> Option<u64> {
    let kib: u64 = text
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .trim()
        .strip_suffix("kB")?
        .trim()
        .parse()
        .ok()?;
    Some(kib / 1024)
}

/// The plaza service a process with command name `comm` is, under the name the
/// console shows it by.
fn service_name(comm: &str) -> Option<&'static str> {
    match comm {
        "apid" => Some("apid"),
        "vsockd" => Some("hub"),
        "noded" => Some("noded"),
        "garage" => Some("garage"),
        _ => None,
    }
}

/// Every process `/proc` lists that can be read, keyed by pid. A process's CPU
/// share is its ticks since the previous sample (`prev_ticks`) over `total`,
/// the machine's ticks in the same span; a process with no previous reading,
/// or no machine time to divide by, reports 0.
#[cfg(target_os = "linux")]
fn read_processes(prev_ticks: &HashMap<u32, u64>, total: Option<u64>) -> HashMap<u32, ProcSample> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return HashMap::new();
    };
    entries
        .filter_map(|entry| {
            let pid: u32 = entry.ok()?.file_name().to_str()?.parse().ok()?;
            let (comm, ticks) = parse_proc_stat(&read(&format!("/proc/{pid}/stat"))?)?;
            let rss_mib = parse_vm_rss_mib(&read(&format!("/proc/{pid}/status"))?)?;
            let cpu_percent = prev_ticks
                .get(&pid)
                .zip(total)
                .and_then(|(before, total)| process_cpu_percent(*before, ticks, total))
                .unwrap_or(0.0);
            Some((
                pid,
                ProcSample {
                    comm,
                    #[cfg(target_os = "linux")]
                    ticks,
                    cpu_percent,
                    rss_mib,
                },
            ))
        })
        .collect()
}

/// The plaza's usage: the service processes by pid, the VM processes in the
/// order the caller gave them (a VM whose process was not read is left out),
/// and the totals.
fn plaza_usage(procs: &HashMap<u32, ProcSample>, vms: &[VmProcess]) -> PlazaUsage {
    let mut services: Vec<ServiceUsage> = procs
        .iter()
        .filter_map(|(pid, sample)| {
            Some(ServiceUsage {
                name: service_name(&sample.comm)?,
                pid: *pid,
                cpu_percent: sample.cpu_percent,
                rss_mib: sample.rss_mib,
            })
        })
        .collect();
    services.sort_by_key(|service| service.pid);
    let vm_usage: Vec<VmUsage> = vms
        .iter()
        .filter_map(|vm| {
            let sample = procs.get(&vm.pid)?;
            Some(VmUsage {
                name: format!("vm {}/{}", vm.run, vm.vm),
                pid: vm.pid,
                cpu_percent: sample.cpu_percent,
                rss_mib: sample.rss_mib,
                run: vm.run.clone(),
                vm: vm.vm,
                cid: vm.cid,
                role: vm.role.clone(),
                usable_mib: vm.usable_mib,
                guest_used_mib: vm.guest_used_mib,
            })
        })
        .collect();
    let services_cpu_percent: f64 = services.iter().map(|service| service.cpu_percent).sum();
    let services_rss_mib: u64 = services.iter().map(|service| service.rss_mib).sum();
    let vms_cpu_percent: f64 = vm_usage.iter().map(|vm| vm.cpu_percent).sum();
    let vms_rss_mib: u64 = vm_usage.iter().map(|vm| vm.rss_mib).sum();
    PlazaUsage {
        services,
        vms: vm_usage,
        total: TotalUsage {
            cpu_percent: services_cpu_percent + vms_cpu_percent,
            rss_mib: services_rss_mib + vms_rss_mib,
            services_cpu_percent,
            services_rss_mib,
            vms_cpu_percent,
            vms_rss_mib,
        },
    }
}

#[cfg(target_os = "linux")]
fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

#[cfg(target_os = "linux")]
fn cpu_line() -> Option<String> {
    read("/proc/stat")?
        .lines()
        .find(|line| line.starts_with("cpu "))
        .map(str::to_string)
}

/// `(MemTotal, MemAvailable)` in MiB, from `/proc/meminfo` text.
#[cfg(target_os = "linux")]
fn parse_meminfo(text: &str) -> Option<(u64, u64)> {
    let kib = |key: &str| {
        text.lines().find_map(|line| {
            line.strip_prefix(key)?
                .trim()
                .strip_suffix("kB")?
                .trim()
                .parse::<u64>()
                .ok()
        })
    };
    Some((kib("MemTotal:")? / 1024, kib("MemAvailable:")? / 1024))
}

#[cfg(target_os = "linux")]
fn parse_loadavg(text: &str) -> Option<[f64; 3]> {
    let mut fields = text
        .split_whitespace()
        .map(|field| field.parse::<f64>().ok());
    Some([fields.next()??, fields.next()??, fields.next()??])
}

#[cfg(target_os = "linux")]
fn parse_uptime(text: &str) -> Option<f64> {
    text.split_whitespace().next()?.parse().ok()
}

/// A full sample, given the CPU share already worked out from `/proc/stat`.
#[cfg(target_os = "linux")]
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
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
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

    #[test]
    #[cfg(target_os = "linux")]
    fn total_jiffies_is_every_counted_field_of_the_cpu_line() {
        assert_eq!(
            total_jiffies("cpu  100 0 100 800 100 0 0 0 0 0"),
            Some(1100)
        );
        assert_eq!(total_jiffies("cpu0 100 0 100 800"), None);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn proc_stat_ticks_are_utime_plus_stime_counted_after_the_name() {
        // The name holds spaces and parentheses; utime is 30 and stime 12.
        let line =
            "1234 (my vm (x)) S 1 1234 1234 0 -1 4194560 100 0 0 0 30 12 0 0 20 0 1 0 500 0 0";
        assert_eq!(parse_proc_stat(line), Some(("my vm (x)".to_string(), 42)));
        assert_eq!(parse_proc_stat("1234 (apid) S 1"), None);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn vm_rss_is_read_from_the_status_text_in_mib() {
        let status = "Name:\tapid\nVmPeak:\t  9000 kB\nVmRSS:\t   2048 kB\nThreads:\t1\n";
        assert_eq!(parse_vm_rss_mib(status), Some(2));
        // A kernel thread has no VmRSS line.
        assert_eq!(parse_vm_rss_mib("Name:\tkthreadd\n"), None);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn process_cpu_percent_is_its_ticks_over_the_machine_ticks() {
        assert_eq!(process_cpu_percent(100, 150, 1000), Some(5.0));
        // the counter went backwards (pid reuse), or no machine time passed
        assert_eq!(process_cpu_percent(150, 100, 1000), None);
        assert_eq!(process_cpu_percent(100, 150, 0), None);
    }

    #[test]
    fn the_plaza_services_are_named_as_the_console_shows_them() {
        assert_eq!(service_name("apid"), Some("apid"));
        assert_eq!(service_name("vsockd"), Some("hub"));
        assert_eq!(service_name("noded"), Some("noded"));
        assert_eq!(service_name("garage"), Some("garage"));
        assert_eq!(service_name("bash"), None);
    }

    #[test]
    fn plaza_usage_names_the_services_and_sums_the_totals() {
        let sample = |comm: &str, cpu_percent: f64, rss_mib: u64| ProcSample {
            comm: comm.to_string(),
            #[cfg(target_os = "linux")]
            ticks: 0,
            cpu_percent,
            rss_mib,
        };
        let procs = HashMap::from([
            (10, sample("vsockd", 1.5, 4)),
            (11, sample("bash", 9.0, 1)),
            (12, sample("vmm", 20.0, 300)),
            (13, sample("apid", 0.5, 8)),
        ]);
        let vms = [
            VmProcess {
                pid: 12,
                run: "r3".to_string(),
                vm: 0,
                cid: Some(100),
                role: Some("builder".to_string()),
                usable_mib: Some(256),
                guest_used_mib: Some(100),
            },
            // no reading for this process: it is left out
            VmProcess {
                pid: 99,
                run: "r3".to_string(),
                vm: 1,
                cid: None,
                role: None,
                usable_mib: None,
                guest_used_mib: None,
            },
        ];
        let usage = plaza_usage(&procs, &vms);
        let services: Vec<(&str, u32)> = usage.services.iter().map(|s| (s.name, s.pid)).collect();
        assert_eq!(services, vec![("hub", 10), ("apid", 13)]);
        assert_eq!(usage.vms.len(), 1);
        assert_eq!(usage.vms[0].name, "vm r3/0");
        assert_eq!(usage.vms[0].rss_mib, 300);
        assert_eq!(usage.vms[0].guest_used_mib, Some(100));
        assert_eq!(usage.total.services_cpu_percent, 2.0);
        assert_eq!(usage.total.services_rss_mib, 12);
        assert_eq!(usage.total.vms_cpu_percent, 20.0);
        assert_eq!(usage.total.vms_rss_mib, 300);
        assert_eq!(usage.total.cpu_percent, 22.0);
        assert_eq!(usage.total.rss_mib, 312);
    }
}
