//! `GET /metrics`: the guest's resource use, as envd reports it (E2B's `Metrics`): the CPU count
//! and how busy the CPUs were since the last sample, the memory in use and in the page cache, and
//! the disk the sandbox's home lives on.
#![allow(unsafe_code)]

use std::path::Path;
use std::sync::Mutex;

use serde_json::{Value, json};

/// The last `/proc/stat` CPU totals sampled: `(busy, all)` jiffies.
static LAST_CPU: Mutex<Option<(u64, u64)>> = Mutex::new(None);

/// The aggregate `cpu` line of `/proc/stat` as `(busy, all)` jiffies: everything but idle and
/// iowait is busy. Only the first eight numbers count (user nice system idle iowait irq softirq
/// steal); guest and guest_nice are already inside user and nice.
pub fn parse_cpu(stat: &str) -> Option<(u64, u64)> {
    let line = stat.lines().find(|line| line.starts_with("cpu "))?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|field| field.parse().ok())
        .take(8)
        .collect();
    if fields.len() < 4 {
        return None;
    }
    let all: u64 = fields.iter().sum();
    let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
    Some((all.saturating_sub(idle), all))
}

/// The CPU lines of `/proc/stat` (`cpu0`, `cpu1`, ...).
pub fn cpu_count(stat: &str) -> usize {
    stat.lines()
        .filter(|line| {
            line.starts_with("cpu") && line.as_bytes().get(3).is_some_and(u8::is_ascii_digit)
        })
        .count()
        .max(1)
}

/// `(total, used, cache)` in bytes from `/proc/meminfo`: used is the total less what is available.
pub fn parse_meminfo(meminfo: &str) -> Option<(u64, u64, u64)> {
    let field = |name: &str| {
        meminfo.lines().find_map(|line| {
            let rest = line.strip_prefix(name)?.strip_prefix(':')?;
            rest.split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
                .map(|kib| kib * 1024)
        })
    };
    let total = field("MemTotal")?;
    let available = field("MemAvailable").or_else(|| field("MemFree"))?;
    Some((
        total,
        total.saturating_sub(available),
        field("Cached").unwrap_or(0),
    ))
}

/// How busy the CPUs were between two samples, in percent.
pub fn busy_pct(before: Option<(u64, u64)>, now: (u64, u64)) -> f64 {
    let (busy, all) = match before {
        Some((busy, all)) if now.1 > all => (now.0.saturating_sub(busy), now.1 - all),
        // No earlier sample (or the counters went back): the average since boot.
        _ => now,
    };
    if all == 0 {
        0.0
    } else {
        (busy as f64 * 100.0 / all as f64).clamp(0.0, 100.0)
    }
}

/// `(used, total)` bytes of the filesystem holding `path`.
fn disk(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let mut bytes = path.as_os_str().as_bytes().to_vec();
    bytes.push(0);
    // SAFETY: `statvfs` reads the NUL-terminated path and writes the struct it is given, both of
    // which outlive the call.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::statvfs(bytes.as_ptr().cast(), &mut stat) };
    if result != 0 {
        return None;
    }
    let block = stat.f_frsize as u64;
    let total = stat.f_blocks as u64 * block;
    let free = stat.f_bfree as u64 * block;
    Some((total.saturating_sub(free), total))
}

/// One sample, as E2B's `Metrics`.
pub fn sample(home: &Path) -> Value {
    let stat = std::fs::read_to_string("/proc/stat").unwrap_or_default();
    let now = parse_cpu(&stat).unwrap_or((0, 0));
    let before = LAST_CPU
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .replace(now);
    let (mem_total, mem_used, mem_cache) =
        parse_meminfo(&std::fs::read_to_string("/proc/meminfo").unwrap_or_default())
            .unwrap_or((0, 0, 0));
    let (disk_used, disk_total) = disk(home).unwrap_or((0, 0));
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    json!({
        "ts": ts,
        "cpu_count": cpu_count(&stat),
        "cpu_used_pct": busy_pct(before, now),
        "mem_total": mem_total,
        "mem_used": mem_used,
        "mem_cache": mem_cache,
        "disk_used": disk_used,
        "disk_total": disk_total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_time_ignores_the_guest_fields() {
        // user nice system idle iowait irq softirq steal guest guest_nice
        let stat = "cpu  10 0 5 80 2 1 1 0 100 50\n";
        assert_eq!(parse_cpu(stat), Some((17, 99)));
    }

    #[test]
    fn a_sample_has_every_field() {
        let sample = sample(Path::new("/"));
        for key in [
            "ts",
            "cpu_count",
            "cpu_used_pct",
            "mem_total",
            "mem_used",
            "disk_total",
        ] {
            assert!(sample.get(key).is_some(), "{key}");
        }
    }
}
