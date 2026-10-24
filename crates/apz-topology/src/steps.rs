//! The guest's side of a resilience run (PLAN 22.8): what a scenario's actions do to a lab VM's processes, fault proxies,
//! volumes, bridge and clock. A fault is set on the dependency's proxy, or netem on the bridge; a process is killed by its
//! pid file and started again; a volume is filled up to what it leaves free, bounded by its declared size; a clock is
//! moved. Each action is available only where the guest has what it needs (a Linux guest and root for netem and the clock,
//! a proxy, a pid file, a declared volume), and says why where it does not.
//!
//! The guest's processes are a [`Processes`] (the runner's own, or an in-process stand-in in tests); its host and its
//! commands are traits too, so the rules are tested with fakes. Nothing here installs a tool.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::fault::{Fault, Steer};
use crate::netem;
use crate::resilience::Actuator;
use crate::spec::{Action, Scenario};

/// The file a `fill_disk` action writes into its volume.
pub const FILL_FILE: &str = "apz-fill.bin";
/// One MiB, in bytes.
const MIB: u64 = 1024 * 1024;
/// The largest declared volume a fill may write (matches the topology's limit).
pub const MAX_FILL_MIB: u64 = crate::spec::MAX_VOLUME_MIB;

/// What the guest offers: its OS, its privilege, its tools on the PATH, and its paths.
pub trait Host {
    fn is_linux(&self) -> bool;
    fn is_root(&self) -> bool;
    fn tool(&self, name: &str) -> bool;
    fn exists(&self, path: &Path) -> bool;
}

/// The host the bin runs on: the OS, `id -u`, the PATH, and the paths.
pub struct System {
    pub path: String,
}

impl Host for System {
    fn is_linux(&self) -> bool {
        cfg!(target_os = "linux")
    }

    fn is_root(&self) -> bool {
        Command::new("id")
            .arg("-u")
            .output()
            .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).trim() == "0")
    }

    fn tool(&self, name: &str) -> bool {
        crate::load::find_on(&self.path, name).is_some()
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

/// How the guest runs a command to completion (netem, the clock, a kill).
pub trait Runner {
    fn run(&mut self, command: &[String]) -> Result<(), String>;
}

/// The guest's own commands.
pub struct Commands;

impl Runner for Commands {
    fn run(&mut self, command: &[String]) -> Result<(), String> {
        let status = Command::new(&command[0])
            .args(&command[1..])
            .status()
            .map_err(|e| format!("{}: {e}", command.join(" ")))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("{} ended with {status}", command.join(" ")))
        }
    }
}

/// The processes of a run: their fault proxies (steered by service), their pid files, their restarts, and their volumes.
pub trait Processes {
    /// Whether a fault proxy sits in front of `service`.
    fn has_proxy(&self, service: &str) -> bool;
    /// Sets the fault of the proxies in front of `service`.
    fn steer(&self, service: &str, fault: Fault) -> Result<(), String>;
    /// The pid file of `service`'s process, if the run keeps one.
    fn pid_file(&self, service: &str) -> Option<PathBuf>;
    /// Starts `service`'s process again, writing its new pid to its pid file.
    fn restart(&mut self, service: &str) -> Result<(), String>;
    /// The directory and the declared size in MiB of a volume `service` mounts.
    fn volume(&self, service: &str, volume: &str) -> Option<(PathBuf, u64)>;
}

/// The bytes a fill writes to leave `leave_mib` free in a volume of `declared_mib` that holds `used` bytes. Refused when the
/// declared size is out of range or there is no room above the reserve.
pub fn fill_bytes(declared_mib: u64, used: u64, leave_mib: u64) -> Result<u64, String> {
    if declared_mib == 0 || declared_mib > MAX_FILL_MIB {
        return Err(format!(
            "the volume is {declared_mib} MiB: a fill runs on a declared volume of 1 to {MAX_FILL_MIB} MiB"
        ));
    }
    let declared = declared_mib * MIB;
    let reserve = leave_mib * MIB;
    if used + reserve >= declared {
        return Err(format!(
            "the volume has at most {} MiB free already",
            declared.saturating_sub(used) / MIB
        ));
    }
    Ok(declared - used - reserve)
}

/// The bytes in `dir` (recursively), not counting the fill file itself.
pub fn used_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                used_bytes(&path)
            } else if path.file_name().is_some_and(|name| name == FILL_FILE) {
                0
            } else {
                entry.metadata().map_or(0, |m| m.len())
            }
        })
        .sum()
}

/// Writes `bytes` of zeros to `path`, in MiB chunks.
fn write_fill(path: &Path, bytes: u64) -> Result<(), String> {
    let mut file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let chunk = vec![0u8; MIB as usize];
    let mut left = bytes;
    while left > 0 {
        let n = left.min(MIB) as usize;
        file.write_all(&chunk[..n])
            .map_err(|e| format!("{}: {e}", path.display()))?;
        left -= n as u64;
    }
    Ok(())
}

/// The command that moves the guest's clock by `delta` seconds from now.
pub fn clock_command(now_secs: i64, delta: i64) -> Vec<String> {
    vec!["date".into(), "-s".into(), format!("@{}", now_secs + delta)]
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The guest's actions: its host, its commands, its processes, and the bridge netem shapes.
pub struct Guest<H: Host, R: Runner, P: Processes> {
    pub host: H,
    pub runner: R,
    pub procs: P,
    pub device: String,
}

impl<H: Host, R: Runner, P: Processes> Guest<H, R, P> {
    fn kill(&mut self, service: &str) -> Result<(), String> {
        let path = self.pid_file(service)?;
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let pid = text.trim().to_string();
        if pid.parse::<u32>().is_err() {
            return Err(format!("{service}'s pid file does not hold a pid"));
        }
        self.runner.run(&["kill".into(), "-9".into(), pid])
    }

    fn pid_file(&self, service: &str) -> Result<PathBuf, String> {
        self.procs
            .pid_file(service)
            .ok_or_else(|| format!("the guest keeps no pid file for {service}"))
    }

    fn volume(&self, service: &str, volume: &Option<String>) -> Result<(PathBuf, u64), String> {
        let name = volume.clone().unwrap_or_default();
        self.procs
            .volume(service, &name)
            .ok_or_else(|| format!("the guest has no volume {name} of {service}"))
    }
}

impl<H: Host, R: Runner, P: Processes> Actuator for Guest<H, R, P> {
    fn available(&self, scenario: &Scenario) -> Result<(), String> {
        match scenario.action {
            Action::Fault => match scenario.fault {
                Some(crate::fault::Fault::Netem { .. }) => netem::check(&self.host, &self.device),
                _ if self.procs.has_proxy(&scenario.target) => Ok(()),
                _ => Err(format!("no fault proxy in front of {}", scenario.target)),
            },
            Action::Kill | Action::Restart => {
                let path = self.pid_file(&scenario.target)?;
                if !self.host.exists(&path) {
                    return Err(format!(
                        "{} has no pid file in the guest yet",
                        scenario.target
                    ));
                }
                Ok(())
            }
            Action::FillDisk => {
                let (dir, declared) = self.volume(&scenario.target, &scenario.volume)?;
                fill_bytes(declared, used_bytes(&dir), scenario.leave_mib).map(|_| ())
            }
            Action::ClockSkew => {
                if !self.host.is_linux() || !self.host.tool("date") {
                    return Err("a clock skew needs date in a Linux guest".into());
                }
                if !self.host.is_root() {
                    return Err("a clock skew needs root in the guest".into());
                }
                Ok(())
            }
        }
    }

    fn act(&mut self, scenario: &Scenario, on: bool) -> Result<(), String> {
        match scenario.action {
            Action::Fault => match scenario.fault {
                Some(crate::fault::Fault::Netem {
                    delay_ms,
                    loss_percent,
                }) => {
                    let command = if on {
                        netem::shaping(&self.device, delay_ms, loss_percent)
                    } else {
                        netem::lifting(&self.device)
                    };
                    self.runner.run(&command)
                }
                Some(fault) => {
                    let set = if on { fault } else { Fault::Healthy };
                    self.procs.steer(&scenario.target, set)
                }
                None => Err("a fault action names no fault".into()),
            },
            Action::Kill => {
                if on {
                    self.kill(&scenario.target)
                } else {
                    self.procs.restart(&scenario.target)
                }
            }
            Action::Restart => {
                if on {
                    self.kill(&scenario.target)?;
                    self.procs.restart(&scenario.target)
                } else {
                    Ok(())
                }
            }
            Action::FillDisk => {
                let (dir, declared) = self.volume(&scenario.target, &scenario.volume)?;
                let file = dir.join(FILL_FILE);
                if on {
                    let bytes = fill_bytes(declared, used_bytes(&dir), scenario.leave_mib)?;
                    write_fill(&file, bytes)
                } else {
                    match std::fs::remove_file(&file) {
                        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                            Err(format!("{}: {e}", file.display()))
                        }
                        _ => Ok(()),
                    }
                }
            }
            Action::ClockSkew => {
                let skew = scenario.secs.unwrap_or(0);
                let delta = if on { skew } else { -skew };
                self.runner.run(&clock_command(now_secs(), delta))
            }
        }
    }
}

/// Processes that run in this process: fault proxies only, with no pid files, restarts or volumes (the in-process test
/// stand-in for a run's processes).
pub struct InProcess {
    pub proxies: BTreeMap<String, Vec<Box<dyn Steer>>>,
}

impl Processes for InProcess {
    fn has_proxy(&self, service: &str) -> bool {
        self.proxies.contains_key(service)
    }

    fn steer(&self, service: &str, fault: Fault) -> Result<(), String> {
        let proxies = self
            .proxies
            .get(service)
            .ok_or_else(|| format!("no fault proxy in front of {service}"))?;
        for proxy in proxies {
            proxy.steer(fault)?;
        }
        Ok(())
    }

    fn pid_file(&self, _: &str) -> Option<PathBuf> {
        None
    }

    fn restart(&mut self, service: &str) -> Result<(), String> {
        Err(format!(
            "no process of {service} runs in this process to restart"
        ))
    }

    fn volume(&self, _: &str, _: &str) -> Option<(PathBuf, u64)> {
        None
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::spec::{Expect, Probe};

    /// A host with what the test says it has.
    #[derive(Default)]
    pub(crate) struct FakeHost {
        pub linux: bool,
        pub root: bool,
        pub tools: Vec<String>,
        pub paths: Vec<String>,
    }

    impl Host for FakeHost {
        fn is_linux(&self) -> bool {
            self.linux
        }
        fn is_root(&self) -> bool {
            self.root
        }
        fn tool(&self, name: &str) -> bool {
            self.tools.iter().any(|t| t == name)
        }
        fn exists(&self, path: &Path) -> bool {
            let text = path.to_string_lossy();
            self.paths.iter().any(|p| *p == text)
        }
    }

    /// A runner that records what it would run, and runs nothing.
    #[derive(Default)]
    pub(crate) struct Recorder {
        pub commands: Vec<Vec<String>>,
    }

    impl Runner for Recorder {
        fn run(&mut self, command: &[String]) -> Result<(), String> {
            self.commands.push(command.to_vec());
            Ok(())
        }
    }

    /// A run's processes for the tests: a pid file and a volume it knows, and restarts that write a new pid.
    #[derive(Default)]
    pub(crate) struct FakeProcs {
        pub pid_file: Option<PathBuf>,
        pub volume_name: String,
        pub volume: Option<(PathBuf, u64)>,
        pub proxied: bool,
        pub restarts: Vec<String>,
    }

    impl Processes for FakeProcs {
        fn has_proxy(&self, _: &str) -> bool {
            self.proxied
        }
        fn steer(&self, service: &str, _: Fault) -> Result<(), String> {
            if self.proxied {
                Ok(())
            } else {
                Err(format!("no fault proxy in front of {service}"))
            }
        }
        fn pid_file(&self, _: &str) -> Option<PathBuf> {
            self.pid_file.clone()
        }
        fn restart(&mut self, service: &str) -> Result<(), String> {
            self.restarts.push(service.to_string());
            if let Some(path) = &self.pid_file {
                std::fs::write(path, "4343").map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        fn volume(&self, _: &str, name: &str) -> Option<(PathBuf, u64)> {
            if name == self.volume_name {
                self.volume.clone()
            } else {
                None
            }
        }
    }

    fn scenario(action: Action, target: &str) -> Scenario {
        Scenario {
            id: "s".into(),
            action,
            target: target.into(),
            fault: None,
            volume: None,
            leave_mib: 0,
            secs: None,
            at_secs: 0,
            for_secs: 1,
            recover_within_secs: 1,
            probe: Probe {
                service: "api".into(),
                path: "/".into(),
            },
            expect: Expect {
                max_error_percent: 100.0,
                max_p95_ms: None,
            },
        }
    }

    fn guest(host: FakeHost, procs: FakeProcs) -> Guest<FakeHost, Recorder, FakeProcs> {
        Guest {
            host,
            runner: Recorder::default(),
            procs,
            device: "apz0".into(),
        }
    }

    #[test]
    fn a_fill_writes_what_the_volume_has_left_above_its_reserve() {
        // A 4 MiB volume holding 1 MiB, keeping 1 MiB free: 2 MiB are written.
        assert_eq!(fill_bytes(4, MIB, 1), Ok(2 * MIB));
        let big = fill_bytes(2048, 0, 0).unwrap_err();
        assert!(big.contains("1 to 1024"), "{big}");
        let full = fill_bytes(4, 3 * MIB, 1).unwrap_err();
        assert!(full.contains("already"), "{full}");
    }

    #[test]
    fn a_fill_is_written_into_the_volume_bounded_by_its_declared_size_and_removed_after() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("data.txt"), vec![1u8; 1024]).unwrap();
        let mut s = scenario(Action::FillDisk, "cache");
        s.volume = Some("cache-data".into());
        s.leave_mib = 1;
        let mut guest = guest(
            FakeHost::default(),
            FakeProcs {
                volume: Some((dir.path().to_path_buf(), 2)),
                volume_name: "cache-data".into(),
                ..FakeProcs::default()
            },
        );
        assert_eq!(guest.available(&s), Ok(()));
        guest.act(&s, true).unwrap();
        let fill = std::fs::metadata(dir.path().join(FILL_FILE)).unwrap().len();
        // 2 MiB declared, 1 KiB of data, 1 MiB kept free: the fill is what is left.
        assert_eq!(fill, 2 * MIB - 1024 - MIB);
        guest.act(&s, false).unwrap();
        assert!(!dir.path().join(FILL_FILE).exists());
        assert!(dir.path().join("data.txt").exists(), "the data is kept");

        let mut unknown = s.clone();
        unknown.volume = Some("logs".into());
        assert!(
            guest
                .available(&unknown)
                .unwrap_err()
                .contains("no volume logs")
        );
    }

    #[test]
    fn a_fault_is_set_on_its_proxy_and_lifted_after() {
        let upstream: std::net::SocketAddr = "127.0.0.1:9".parse().unwrap();
        let proxy =
            crate::fault::Proxy::start("127.0.0.1:0".parse().unwrap(), upstream, Fault::Healthy)
                .unwrap();
        let handle = proxy.handle();
        let processes = InProcess {
            proxies: BTreeMap::from([(
                "cache".to_string(),
                vec![Box::new(handle) as Box<dyn Steer>],
            )]),
        };
        let mut guest = Guest {
            host: FakeHost::default(),
            runner: Recorder::default(),
            procs: processes,
            device: "apz0".into(),
        };
        let mut s = scenario(Action::Fault, "cache");
        s.fault = Some(Fault::Partition);
        assert_eq!(guest.available(&s), Ok(()));
        guest.act(&s, true).unwrap();
        assert_eq!(proxy.fault(), Fault::Partition);
        guest.act(&s, false).unwrap();
        assert_eq!(proxy.fault(), Fault::Healthy);

        let missing = scenario(Action::Fault, "queue");
        assert!(
            guest
                .available(&missing)
                .unwrap_err()
                .contains("no fault proxy in front of queue")
        );
    }

    #[test]
    fn a_kill_stops_the_pid_in_its_file_and_a_restart_starts_it_again() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("api.pid");
        std::fs::write(&pid_file, "4242\n").unwrap();
        let mut guest = guest(
            FakeHost {
                paths: vec![pid_file.display().to_string()],
                ..FakeHost::default()
            },
            FakeProcs {
                pid_file: Some(pid_file.clone()),
                ..FakeProcs::default()
            },
        );
        let kill = scenario(Action::Kill, "api");
        assert_eq!(guest.available(&kill), Ok(()));
        guest.act(&kill, true).unwrap();
        assert_eq!(guest.runner.commands[0], ["kill", "-9", "4242"]);
        guest.act(&kill, false).unwrap();
        assert_eq!(guest.procs.restarts, ["api"]);
        assert_eq!(std::fs::read_to_string(&pid_file).unwrap(), "4343");

        guest.act(&scenario(Action::Restart, "api"), true).unwrap();
        assert_eq!(guest.runner.commands[1], ["kill", "-9", "4343"]);
        assert_eq!(guest.procs.restarts, ["api", "api"]);
    }

    #[test]
    fn a_kill_without_a_pid_file_is_not_available() {
        let guest = guest(FakeHost::default(), FakeProcs::default());
        assert!(
            guest
                .available(&scenario(Action::Kill, "api"))
                .unwrap_err()
                .contains("no pid file")
        );
    }

    #[test]
    fn netem_needs_a_linux_guest_with_tc_root_and_the_bridge() {
        let mut s = scenario(Action::Fault, "cache");
        s.fault = Some(Fault::Netem {
            delay_ms: 50,
            loss_percent: 2,
        });
        let mac = guest(FakeHost::default(), FakeProcs::default());
        assert!(mac.available(&s).unwrap_err().contains("Linux"));

        let mut ready = guest(
            FakeHost {
                linux: true,
                root: true,
                tools: vec!["tc".into()],
                paths: vec!["/sys/class/net/apz0".into()],
            },
            FakeProcs::default(),
        );
        assert_eq!(ready.available(&s), Ok(()));
        ready.act(&s, true).unwrap();
        assert_eq!(
            ready.runner.commands[0].join(" "),
            "tc qdisc replace dev apz0 root netem delay 50ms loss 2%"
        );
    }

    #[test]
    fn a_clock_skew_needs_root_in_a_linux_guest_and_moves_the_clock_back_after() {
        let mut s = scenario(Action::ClockSkew, "api");
        s.secs = Some(3600);
        let unprivileged = guest(
            FakeHost {
                linux: true,
                tools: vec!["date".into()],
                ..FakeHost::default()
            },
            FakeProcs::default(),
        );
        assert!(unprivileged.available(&s).unwrap_err().contains("root"));
        let mut root = guest(
            FakeHost {
                linux: true,
                root: true,
                tools: vec!["date".into()],
                ..FakeHost::default()
            },
            FakeProcs::default(),
        );
        assert_eq!(root.available(&s), Ok(()));
        root.act(&s, true).unwrap();
        root.act(&s, false).unwrap();
        let at = |c: &[String]| c[2].trim_start_matches('@').parse::<i64>().unwrap();
        let (on, off) = (&root.runner.commands[0], &root.runner.commands[1]);
        // The skew is one hour forward, then one hour back: the two commands are two hours apart.
        assert!(((at(on) - at(off)) - 7200).abs() <= 5, "{on:?} {off:?}");
    }

    #[test]
    fn the_clock_command_moves_from_now_by_its_delta() {
        assert_eq!(clock_command(1_000, 3_600), ["date", "-s", "@4600"]);
    }
}
