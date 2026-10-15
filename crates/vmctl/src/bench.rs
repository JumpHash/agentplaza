//! `vmctl bench` — how fast the AgentPlaza microVMs boot, snapshot and restore, and what a host
//! pays for them in memory.
//!
//! ```text
//! vmctl bench --kernel target/guest/x86_64/vmlinux --initrd target/guest/x86_64/initrd.gz \
//!             --n 10 --concurrency 5 --json
//! ```
//!
//! The run boots `--n` VMs, `--concurrency` at a time, each to the guest's first heartbeat and then
//! to one console round trip. With all of them alive it reads the host's memory, then snapshots each
//! VM in full and kills it. It restores `--n` VMs from those snapshots the same way (under other CIDs
//! and sockets), reads the memory again with all of them alive, and diff-snapshots and kills each.
//! Every VM is killed when the bench returns, on an error path too, and the scratch directory goes.
//!
//! The VMs run as threads of this process: the bench pins [`SpawnMode::InProcess`], because its
//! memory figures are this process's `VmRSS` and `Pss` (from `/proc/self`), which a VM in a child
//! process would not show in.

use std::env;
use std::fmt::Display;
use std::fs;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::process::{self, Command, ExitCode};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use vmm::spawner::{
    MIN_GUEST_CID, SnapshotKind, SpawnMode, VmHandle, VmSpec, VsockConfig, create_uds_dir,
    materialize_workspaces, set_spawn_mode, spawn,
};

/// The command line every bench VM boots with: the harness keeps the guest alive as PID 1, and the
/// serial console is the only channel the bench talks to it through.
const CMDLINE: &str = "console=ttyS0 plaza.keepalive=1 plaza.vm=0";

/// The serial line the guest prints at its first heartbeat: the boot is over when it appears.
const FIRST_BEAT: &str = "heartbeat 1";

/// How long a VM has to beat for the first time, and to answer a console command.
const BOOT_TIMEOUT: Duration = Duration::from_secs(30);
const ANSWER_TIMEOUT: Duration = Duration::from_secs(15);

/// How often a VM's serial capture is polled while the bench waits on it.
const POLL: Duration = Duration::from_millis(10);

/// Size of each VM's workspace volume: a copy of one ext4 image made with `mkfs.ext4`.
const WORKSPACE_BYTES: u64 = 64 << 20;

/// How much of a VM's serial output an error message quotes.
const SERIAL_TAIL_BYTES: usize = 800;

const DEFAULT_COUNT: usize = 10;
const DEFAULT_CONCURRENCY: usize = 1;
const DEFAULT_MEMORY_MIB: usize = 256;
const DEFAULT_VCPUS: u8 = 1;

const USAGE: &str = "\
usage: vmctl bench --kernel PATH --initrd PATH [--n N] [--concurrency C]
                   [--mem-mib M] [--vcpus V] [--json]

  --kernel PATH      guest kernel image (uncompressed vmlinux or bzImage)
  --initrd PATH      guest initrd with the harness as PID 1 (scripts/mk-initrd.sh)
  --n N              VMs to boot, and then to restore (default 10)
  --concurrency C    VMs started at a time, on C threads (default 1)
  --mem-mib M        guest memory per VM in MiB (default 256)
  --vcpus V          vCPUs per VM (default 1)
  --json             print the results as JSON instead of a table
  -h, --help         print this help

Each VM gets a vsock device and a workspace volume of its own (a copy of a 64 MiB ext4 image).
Scratch files live under the temp directory and are removed when the bench ends.";

/// Everything `vmctl bench` accepts on its command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchConfig {
    kernel: PathBuf,
    initrd: PathBuf,
    count: usize,
    concurrency: usize,
    memory_mib: usize,
    vcpus: u8,
    json: bool,
}

impl BenchConfig {
    /// Parses the arguments after `vmctl bench`.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut kernel = None;
        let mut initrd = None;
        let mut count = DEFAULT_COUNT;
        let mut concurrency = DEFAULT_CONCURRENCY;
        let mut memory_mib = DEFAULT_MEMORY_MIB;
        let mut vcpus = DEFAULT_VCPUS;
        let mut json = false;

        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let mut value = |name: &str| -> Result<String, String> {
                args.next()
                    .cloned()
                    .ok_or_else(|| format!("{name} needs a value"))
            };
            match arg.as_str() {
                "--kernel" => kernel = Some(PathBuf::from(value("--kernel")?)),
                "--initrd" => initrd = Some(PathBuf::from(value("--initrd")?)),
                "--n" => count = number("--n", &value("--n")?)?,
                "--concurrency" => concurrency = number("--concurrency", &value("--concurrency")?)?,
                "--mem-mib" => memory_mib = number("--mem-mib", &value("--mem-mib")?)?,
                "--vcpus" => vcpus = number("--vcpus", &value("--vcpus")?)?,
                "--json" => json = true,
                other => return Err(format!("unknown argument: {other}")),
            }
        }

        if count == 0 {
            return Err("--n must be at least 1".to_string());
        }
        if concurrency == 0 {
            return Err("--concurrency must be at least 1".to_string());
        }
        if memory_mib == 0 {
            return Err("--mem-mib must be at least 1".to_string());
        }
        if vcpus == 0 {
            return Err("--vcpus must be at least 1".to_string());
        }

        Ok(BenchConfig {
            kernel: kernel.ok_or("--kernel is required")?,
            initrd: initrd.ok_or("--initrd is required")?,
            count,
            concurrency,
            memory_mib,
            vcpus,
            json,
        })
    }
}

/// Parses a numeric flag's value, naming the flag in the error.
fn number<T>(name: &str, text: &str) -> Result<T, String>
where
    T: FromStr,
    T::Err: Display,
{
    text.parse().map_err(|err| format!("invalid {name}: {err}"))
}

/// Runs `vmctl bench` with the arguments after the subcommand name.
pub fn run(args: &[String]) -> ExitCode {
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let config = match BenchConfig::parse(args) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("vmctl bench: {err}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match bench(&config) {
        Ok(report) => {
            if config.json {
                println!("{}", report.to_json());
            } else {
                print!("{}", report.table());
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("vmctl bench: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Summary statistics of one measured quantity over the bench's VMs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

impl Summary {
    /// The summary of `samples` (in any order); `None` when there are none.
    pub fn of(samples: &[f64]) -> Option<Self> {
        let mut sorted = samples.to_vec();
        sorted.sort_by(f64::total_cmp);
        Some(Summary {
            p50: percentile(&sorted, 50)?,
            p95: percentile(&sorted, 95)?,
            p99: percentile(&sorted, 99)?,
            max: *sorted.last()?,
        })
    }
}

/// The nearest-rank percentile `pct` (0 to 100) of `sorted`, which must be ascending: the smallest
/// sample at or above which `pct` percent of the samples lie. `None` for no samples.
///
/// The rank is computed in integers, so `pct` of exactly 95 over 100 samples is the 95th sample,
/// not the 96th because of a float rounding error.
pub fn percentile(sorted: &[f64], pct: usize) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (pct * sorted.len()).div_ceil(100).clamp(1, sorted.len());
    Some(sorted[rank - 1])
}

/// One measured quantity: its summary over the bench's VMs, `None` when nothing was measured.
struct Metric {
    name: &'static str,
    unit: &'static str,
    summary: Option<Summary>,
}

impl Metric {
    fn new(name: &'static str, unit: &'static str, samples: &[f64]) -> Self {
        Metric {
            name,
            unit,
            summary: Summary::of(samples),
        }
    }
}

/// The memory of this process as the kernel reports it, in KiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Memory {
    /// `VmRSS` from `/proc/self/status`: every page this process maps and has resident.
    rss_kib: u64,
    /// `Pss` from `/proc/self/smaps_rollup`: resident pages, each divided among the mappings
    /// that share it. Restored VMs map their snapshot's memory privately, so pages they share
    /// with each other (or with the snapshot file) count once here and once per VM in `VmRSS`.
    pss_kib: u64,
}

/// What the bench measured.
struct Report {
    kernel: PathBuf,
    initrd: PathBuf,
    count: usize,
    concurrency: usize,
    memory_mib: usize,
    vcpus: u8,
    metrics: Vec<Metric>,
    /// `(phase, memory)`, in the order the phases ran.
    memory: Vec<(&'static str, Memory)>,
}

impl Report {
    /// The results as a compact table, one row per quantity.
    fn table(&self) -> String {
        let mut out = format!(
            "vmctl bench: {} VMs, concurrency {}, {} vCPU, {} MiB each\nkernel {}\ninitrd {}\n\n",
            self.count,
            self.concurrency,
            self.vcpus,
            self.memory_mib,
            self.kernel.display(),
            self.initrd.display(),
        );
        out.push_str(&format!(
            "{:<32} {:<5} {:>9} {:>9} {:>9} {:>9}\n",
            "metric", "unit", "p50", "p95", "p99", "max"
        ));
        for metric in &self.metrics {
            let cells: Vec<String> = match metric.summary {
                Some(s) => [s.p50, s.p95, s.p99, s.max]
                    .iter()
                    .map(|v| format!("{v:.1}"))
                    .collect(),
                None => vec!["-".to_string(); 4],
            };
            out.push_str(&format!(
                "{:<32} {:<5} {:>9} {:>9} {:>9} {:>9}\n",
                metric.name, metric.unit, cells[0], cells[1], cells[2], cells[3]
            ));
        }
        out.push_str(
            "\nhost process memory (MiB): baseline before any VM; booted and restored with all \
             the VMs alive\n",
        );
        out.push_str(&format!("{:<32} {:>9} {:>9}\n", "phase", "VmRSS", "Pss"));
        for (phase, memory) in &self.memory {
            out.push_str(&format!(
                "{:<32} {:>9.1} {:>9.1}\n",
                phase,
                kib_to_mib(memory.rss_kib),
                kib_to_mib(memory.pss_kib)
            ));
        }
        out
    }

    /// The results as one JSON object: the configuration, each metric's summary (`null` when
    /// nothing was measured) and each phase's memory in KiB.
    fn to_json(&self) -> String {
        let metrics: Vec<String> = self
            .metrics
            .iter()
            .map(|metric| {
                let body = match metric.summary {
                    Some(s) => format!(
                        "{{\"unit\":{},\"p50\":{:.3},\"p95\":{:.3},\"p99\":{:.3},\"max\":{:.3}}}",
                        json_string(metric.unit),
                        s.p50,
                        s.p95,
                        s.p99,
                        s.max
                    ),
                    None => "null".to_string(),
                };
                format!("{}:{}", json_string(metric.name), body)
            })
            .collect();
        let memory: Vec<String> = self
            .memory
            .iter()
            .map(|(phase, m)| {
                format!(
                    "{}:{{\"vmrss_kib\":{},\"pss_kib\":{}}}",
                    json_string(phase),
                    m.rss_kib,
                    m.pss_kib
                )
            })
            .collect();
        format!(
            "{{\"kernel\":{},\"initrd\":{},\"n\":{},\"concurrency\":{},\"mem_mib\":{},\"vcpus\":{},\
             \"metrics\":{{{}}},\"memory_kib\":{{{}}}}}",
            json_string(&self.kernel.to_string_lossy()),
            json_string(&self.initrd.to_string_lossy()),
            self.count,
            self.concurrency,
            self.memory_mib,
            self.vcpus,
            metrics.join(","),
            memory.join(",")
        )
    }
}

/// A JSON string literal for `text`.
fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn kib_to_mib(kib: u64) -> f64 {
    kib as f64 / 1024.0
}

/// A running VM the bench owns. Dropping it kills the VM (a `VmHandle` alone does not stop its VM),
/// so no path out of the bench leaves a guest running: an error in one phase drops the VMs of the
/// others.
struct KillOnDrop(VmHandle);

impl Deref for KillOnDrop {
    type Target = VmHandle;

    fn deref(&self) -> &VmHandle {
        &self.0
    }
}

impl DerefMut for KillOnDrop {
    fn deref_mut(&mut self) -> &mut VmHandle {
        &mut self.0
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        // Killing a VM that already stopped is a no-op, so this is only the error path's work.
        if let Err(err) = self.0.kill() {
            eprintln!("vmctl bench: VM {}: {err}", self.0.id());
        }
    }
}

/// A VM booted to its first heartbeat, after one console round trip.
struct Booted {
    vm: KillOnDrop,
    boot_ms: f64,
    round_trip_ms: f64,
}

/// A VM restored from a snapshot, after its console answered.
struct Restored {
    vm: KillOnDrop,
    spawn_ms: f64,
    answer_ms: f64,
}

/// Removes the scratch directory when dropped.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        match fs::remove_dir_all(&self.0) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                eprintln!("vmctl bench: cannot remove {}: {err}", self.0.display());
            }
            _ => {}
        }
    }
}

/// Runs every phase of the bench. The VMs and the scratch directory are released on every path.
fn bench(config: &BenchConfig) -> Result<Report, String> {
    // The memory figures are this process's: every VM has to be a thread of it.
    set_spawn_mode(SpawnMode::InProcess);

    let root = env::temp_dir().join(format!("apz-bench-{}", process::id()));
    let _ = fs::remove_dir_all(&root);
    // The scratch directory goes last: it is declared first, so it drops after the VMs.
    let _scratch = Scratch(root.clone());
    create_uds_dir(&root).map_err(|err| format!("cannot create {}: {err}", root.display()))?;
    measure(config, &root)
}

fn measure(config: &BenchConfig, root: &Path) -> Result<Report, String> {
    let count = config.count;
    let base = root.join("base.ext4");
    make_image(&base)?;
    let workspaces = materialize_workspaces(&base, &root.join("ws"), count)
        .map_err(|err| format!("cannot copy the workspace image: {err}"))?;

    let baseline = process_memory()?;

    // Boot every VM to its first heartbeat and one console round trip, and keep them all alive.
    let mut booted = fan_out(count, config.concurrency, |i| {
        boot(config, root, &workspaces[i], i)
    })?;
    let booted_memory = process_memory()?;

    // Snapshot each booted VM in full, then stop it: the restores come after, with the booted VMs
    // gone, so the memory they used is not counted in the restored figures.
    let mut full_paused = Vec::with_capacity(count);
    let mut full_wall = Vec::with_capacity(count);
    let mut full_memory = Vec::with_capacity(count);
    for (i, vm) in booted.iter_mut().enumerate() {
        let dir = root.join(format!("vm{i}")).join("snap");
        let started = Instant::now();
        let info = vm
            .vm
            .snapshot(&dir, SnapshotKind::Full)
            .map_err(|err| format!("VM {i}: full snapshot: {err}"))?;
        full_wall.push(ms(started.elapsed()));
        full_paused.push(info.paused_ms as f64);
        full_memory.push(kib(info.memory_written));
        vm.vm
            .kill()
            .map_err(|err| format!("VM {i}: stop after its snapshot: {err}"))?;
    }
    let boot_ms: Vec<f64> = booted.iter().map(|b| b.boot_ms).collect();
    let round_trip_ms: Vec<f64> = booted.iter().map(|b| b.round_trip_ms).collect();
    drop(booted);

    // Restore every VM from its snapshot, under another CID and socket, and keep them all alive.
    let mut restored = fan_out(count, config.concurrency, |i| {
        restore(config, root, &workspaces[i], i)
    })?;
    let restored_memory = process_memory()?;
    let spawn_ms: Vec<f64> = restored.iter().map(|r| r.spawn_ms).collect();
    let answer_ms: Vec<f64> = restored.iter().map(|r| r.answer_ms).collect();

    // A diff snapshot of each restored VM, then stop it.
    let mut diff_paused = Vec::with_capacity(count);
    let mut diff_memory = Vec::with_capacity(count);
    for (i, vm) in restored.iter_mut().enumerate() {
        let dir = root.join(format!("vm{i}")).join("diff");
        let info = vm
            .vm
            .snapshot(&dir, SnapshotKind::Diff)
            .map_err(|err| format!("VM {i}: diff snapshot of the restored VM: {err}"))?;
        diff_paused.push(info.paused_ms as f64);
        diff_memory.push(kib(info.memory_written));
        vm.vm
            .kill()
            .map_err(|err| format!("VM {i}: stop after its diff snapshot: {err}"))?;
    }
    drop(restored);

    Ok(Report {
        kernel: config.kernel.clone(),
        initrd: config.initrd.clone(),
        count,
        concurrency: config.concurrency,
        memory_mib: config.memory_mib,
        vcpus: config.vcpus,
        metrics: vec![
            Metric::new("boot_to_heartbeat", "ms", &boot_ms),
            Metric::new("boot_console_round_trip", "ms", &round_trip_ms),
            Metric::new("snapshot_full_paused", "ms", &full_paused),
            Metric::new("snapshot_full_wall", "ms", &full_wall),
            Metric::new("snapshot_full_memory_written", "KiB", &full_memory),
            Metric::new("restore_spawn", "ms", &spawn_ms),
            Metric::new("restore_to_console_answer", "ms", &answer_ms),
            Metric::new("snapshot_diff_paused", "ms", &diff_paused),
            Metric::new("snapshot_diff_memory_written", "KiB", &diff_memory),
        ],
        memory: vec![
            ("baseline", baseline),
            ("booted", booted_memory),
            ("restored", restored_memory),
        ],
    })
}

/// Boots VM `i` with its own vsock device and workspace, waits for its first heartbeat and makes
/// one console round trip. The VM is returned alive.
fn boot(config: &BenchConfig, root: &Path, workspace: &Path, i: usize) -> Result<Booted, String> {
    let dir = vm_dir(root, i)?;
    let spec = vm_spec(
        config,
        MIN_GUEST_CID + i as u64,
        dir.join("boot.sock"),
        workspace,
        None,
    );
    let started = Instant::now();
    let vm = KillOnDrop(spawn(spec).map_err(|err| format!("VM {i}: boot: {err}"))?);
    wait_for_output(&vm, FIRST_BEAT, BOOT_TIMEOUT)
        .map_err(|err| format!("VM {i} did not boot: {err}"))?;
    let boot_ms = ms(started.elapsed());
    let round_trip_ms = round_trip(&vm, &format!("bench-boot-{i}"))
        .map_err(|err| format!("VM {i}: console after boot: {err}"))?;
    Ok(Booted {
        vm,
        boot_ms,
        round_trip_ms,
    })
}

/// Restores VM `i` from the snapshot its boot wrote, under the CID and socket its restore gets
/// (`n` above the boot CIDs, so the two sets never meet), and waits for the console to answer.
fn restore(
    config: &BenchConfig,
    root: &Path,
    workspace: &Path,
    i: usize,
) -> Result<Restored, String> {
    let dir = vm_dir(root, i)?;
    let cid = MIN_GUEST_CID + (config.count + i) as u64;
    let spec = vm_spec(
        config,
        cid,
        dir.join("restore.sock"),
        workspace,
        Some(dir.join("snap")),
    );
    let started = Instant::now();
    let vm = KillOnDrop(spawn(spec).map_err(|err| format!("VM {i}: restore: {err}"))?);
    let spawn_ms = ms(started.elapsed());
    let answer_ms = spawn_ms
        + round_trip(&vm, &format!("bench-restore-{i}"))
            .map_err(|err| format!("VM {i}: console after restore: {err}"))?;
    Ok(Restored {
        vm,
        spawn_ms,
        answer_ms,
    })
}

/// The scratch directory of VM `i`, created.
fn vm_dir(root: &Path, i: usize) -> Result<PathBuf, String> {
    let dir = root.join(format!("vm{i}"));
    fs::create_dir_all(&dir).map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
    Ok(dir)
}

/// The spec of one bench VM: the kernel and initrd of a boot, or only the snapshot of a restore.
fn vm_spec(
    config: &BenchConfig,
    cid: u64,
    uds_path: PathBuf,
    workspace: &Path,
    restore: Option<PathBuf>,
) -> VmSpec {
    VmSpec {
        kernel: config.kernel.clone(),
        initrd: Some(config.initrd.clone()),
        cmdline: CMDLINE.to_string(),
        vcpus: config.vcpus,
        memory_mib: config.memory_mib,
        vsock: Some(VsockConfig {
            guest_cid: cid,
            uds_path,
        }),
        workspace: Some(workspace.to_path_buf()),
        restore,
        ..VmSpec::default()
    }
}

/// Sends `token` to the guest's console as an `echo` command and waits for the answer. Returns the
/// time it took, in milliseconds.
///
/// The wait is for the token at the start of a line: the console echoes the typed command, which
/// has the token mid-line after `echo `, and only the guest's answer begins a line with it.
fn round_trip(vm: &VmHandle, token: &str) -> Result<f64, String> {
    let started = Instant::now();
    vm.serial_input(format!("echo {token}\n").as_bytes())
        .map_err(|err| format!("console input: {err}"))?;
    wait_for_output(vm, &format!("\n{token}"), ANSWER_TIMEOUT)?;
    Ok(ms(started.elapsed()))
}

/// Waits until the VM's serial output contains `needle`, or `timeout` passes. The error quotes the
/// end of the output, which is what a failed boot or a silent console leaves behind.
fn wait_for_output(vm: &VmHandle, needle: &str, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        let serial = vm.serial();
        if serial.contains(needle) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "no {needle:?} within {}s; serial output ends:\n{}",
                timeout.as_secs(),
                tail(&serial, SERIAL_TAIL_BYTES)
            ));
        }
        thread::sleep(POLL);
    }
}

/// The last `bytes` bytes of `text`, cut at a character boundary.
fn tail(text: &str, bytes: usize) -> &str {
    let mut start = text.len().saturating_sub(bytes);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// Runs `job` for every index in `0..n`, on `concurrency` threads, and returns the results in
/// index order. After the first failure no new job starts. On a failure every result already
/// produced is dropped, which is what kills the VMs among them.
fn fan_out<T: Send>(
    n: usize,
    concurrency: usize,
    job: impl Fn(usize) -> Result<T, String> + Sync,
) -> Result<Vec<T>, String> {
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let slots: Mutex<Vec<Option<Result<T, String>>>> = Mutex::new((0..n).map(|_| None).collect());
    thread::scope(|scope| {
        for _ in 0..concurrency.min(n) {
            scope.spawn(|| {
                loop {
                    if failed.load(Ordering::SeqCst) {
                        break;
                    }
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= n {
                        break;
                    }
                    let result = job(i);
                    if result.is_err() {
                        failed.store(true, Ordering::SeqCst);
                    }
                    slots.lock().unwrap_or_else(PoisonError::into_inner)[i] = Some(result);
                }
            });
        }
    });

    let slots = slots.into_inner().unwrap_or_else(PoisonError::into_inner);
    let mut values = Vec::with_capacity(n);
    let mut first_error = None;
    for slot in slots.into_iter().flatten() {
        match slot {
            Ok(value) => values.push(value),
            Err(err) => {
                first_error.get_or_insert(err);
            }
        }
    }
    match first_error {
        Some(err) => Err(err),
        None => Ok(values),
    }
}

/// Creates the 64 MiB ext4 image every VM's workspace is copied from.
fn make_image(path: &Path) -> Result<(), String> {
    let file =
        fs::File::create(path).map_err(|err| format!("cannot create {}: {err}", path.display()))?;
    file.set_len(WORKSPACE_BYTES)
        .map_err(|err| format!("cannot size {}: {err}", path.display()))?;
    let status = Command::new("mkfs.ext4")
        .args(["-q", "-F"])
        .arg(path)
        .status()
        .map_err(|err| format!("cannot run mkfs.ext4: {err}"))?;
    if !status.success() {
        return Err(format!("mkfs.ext4 failed on {}", path.display()));
    }
    Ok(())
}

/// This process's memory, from `/proc/self`.
fn process_memory() -> Result<Memory, String> {
    let status = fs::read_to_string("/proc/self/status")
        .map_err(|err| format!("cannot read /proc/self/status: {err}"))?;
    let rollup = fs::read_to_string("/proc/self/smaps_rollup")
        .map_err(|err| format!("cannot read /proc/self/smaps_rollup: {err}"))?;
    Ok(Memory {
        rss_kib: kib_field(&status, "VmRSS:").ok_or("no VmRSS in /proc/self/status")?,
        pss_kib: kib_field(&rollup, "Pss:").ok_or("no Pss in /proc/self/smaps_rollup")?,
    })
}

/// The value of a `Key:   <n> kB` line of a `/proc` file, in KiB.
fn kib_field(text: &str, key: &str) -> Option<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix(key))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.parse().ok())
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn kib(bytes: u64) -> f64 {
    bytes as f64 / 1024.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// The required flags plus `extra`.
    fn with_required(extra: &[&str]) -> Vec<String> {
        let mut argv = args(&["--kernel", "/k/vmlinux", "--initrd", "/k/initrd.gz"]);
        argv.extend(extra.iter().map(|s| s.to_string()));
        argv
    }

    #[test]
    fn the_arguments_parse_and_refuse_what_cannot_run() {
        // The defaults apply when no flags are given, and every flag takes its value.
        {
            let config = BenchConfig::parse(&with_required(&[])).expect("defaults");
            assert_eq!(config.count, 10);
            assert_eq!(config.concurrency, 1);
            assert_eq!(config.memory_mib, 256);
            assert_eq!(config.vcpus, 1);
            assert!(!config.json);

            let config = BenchConfig::parse(&with_required(&[
                "--n",
                "3",
                "--concurrency",
                "2",
                "--mem-mib",
                "512",
                "--vcpus",
                "2",
                "--json",
            ]))
            .expect("all flags");
            assert_eq!(config.count, 3);
            assert_eq!(config.concurrency, 2);
            assert_eq!(config.memory_mib, 512);
            assert_eq!(config.vcpus, 2);
            assert!(config.json);
            assert_eq!(config.kernel, PathBuf::from("/k/vmlinux"));
            assert_eq!(config.initrd, PathBuf::from("/k/initrd.gz"));
        }

        // Without --kernel or --initrd, or with a zero, oversized, non-numeric,
        // unknown or valueless flag, the arguments are refused.
        {
            let no_kernel = args(&["--initrd", "/k/initrd.gz"]);
            assert!(
                BenchConfig::parse(&no_kernel)
                    .unwrap_err()
                    .contains("--kernel")
            );
            let no_initrd = args(&["--kernel", "/k/vmlinux"]);
            assert!(
                BenchConfig::parse(&no_initrd)
                    .unwrap_err()
                    .contains("--initrd")
            );
            assert!(BenchConfig::parse(&with_required(&["--n", "0"])).is_err());
            assert!(BenchConfig::parse(&with_required(&["--concurrency", "0"])).is_err());
            assert!(BenchConfig::parse(&with_required(&["--mem-mib", "0"])).is_err());
            assert!(BenchConfig::parse(&with_required(&["--vcpus", "0"])).is_err());
            assert!(BenchConfig::parse(&with_required(&["--vcpus", "256"])).is_err());
            assert!(BenchConfig::parse(&with_required(&["--n", "many"])).is_err());
            assert!(BenchConfig::parse(&with_required(&["--bogus"])).is_err());
            assert!(BenchConfig::parse(&with_required(&["--n"])).is_err());
        }
    }

    #[test]
    fn a_proc_field_reads_its_kib_value() {
        let status = "Name:\tvmctl\nVmPeak:\t  900 kB\nVmRSS:\t  123456 kB\nRssAnon:\t 1 kB\n";
        assert_eq!(kib_field(status, "VmRSS:"), Some(123_456));
        let rollup = "Rss:  200 kB\nPss:  98 kB\nPss_Anon:  50 kB\n";
        assert_eq!(kib_field(rollup, "Pss:"), Some(98));
        assert_eq!(kib_field(rollup, "Swap:"), None);
    }

    #[test]
    fn a_tail_cuts_at_a_character_boundary() {
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("abc", 10), "abc");
        // "é" is two bytes: a cut that lands inside it moves forward to the next character.
        assert_eq!(tail("aéb", 2), "b");
    }

    #[test]
    fn json_strings_escape_what_json_requires() {
        assert_eq!(json_string("a\"b\\c\n"), "\"a\\\"b\\\\c\\u000a\"");
    }
}
