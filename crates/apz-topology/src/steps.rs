//! The guest's side of a resilience run (PLAN 22.8): what a scenario's actions do inside a lab VM. A fault is set on its
//! fault proxy, or netem on the bridge; a process is killed by its pid file and started again; a volume is filled to what
//! it leaves free; a clock is moved. Each action is available only where the guest has what it needs (a Linux guest,
//! root, a tool, a dedicated volume), and says why where it does not.
//!
//! The guest's host and its commands are traits, so the rules are tested here with fakes; the lab VM's bin ([`System`],
//! [`Commands`]) is the one that touches the disk, the clock and the processes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::fault::{Fault, Proxy};
use crate::netem;
use crate::resilience::Actuator;
use crate::spec::{Action, Scenario};

/// The file a `fill_disk` action writes into its volume.
pub const FILL_FILE: &str = "apz-fill.bin";
/// The largest filesystem a fill may touch: a dedicated lab volume of at most 1 GiB, never a host disk.
pub const MAX_FILL_KIB: u64 = 1024 * 1024;

/// What the guest offers: its OS, its privilege, its tools on the PATH, its paths, and its filesystems' sizes.
pub trait Host {
    fn is_linux(&self) -> bool;
    fn is_root(&self) -> bool;
    fn tool(&self, name: &str) -> bool;
    fn exists(&self, path: &Path) -> bool;
    /// The total and the available KiB of the filesystem that holds `dir`.
    fn disk_kib(&self, dir: &Path) -> Option<(u64, u64)>;
}

/// The host the bin runs on: the OS, the PATH, `id -u`, the paths, and `df`.
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

    fn disk_kib(&self, dir: &Path) -> Option<(u64, u64)> {
        // The POSIX `df -P` columns on both systems: the filesystem, its 1024-byte blocks, used, available.
        let out = Command::new("df").arg("-Pk").arg(dir).output().ok()?;
        let line = String::from_utf8_lossy(&out.stdout)
            .lines()
            .nth(1)?
            .to_string();
        let fields: Vec<&str> = line.split_whitespace().collect();
        let total = fields.get(1)?.parse().ok()?;
        let avail = fields.get(3)?.parse().ok()?;
        Some((total, avail))
    }
}

/// How the guest runs a command: `run` waits for it, `spawn` starts it and leaves it running (a service).
pub trait Runner {
    fn run(&mut self, command: &[String]) -> Result<(), String>;
    fn spawn(&mut self, command: &[String]) -> Result<u32, String>;
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

    fn spawn(&mut self, command: &[String]) -> Result<u32, String> {
        let child = Command::new(&command[0])
            .args(&command[1..])
            .spawn()
            .map_err(|e| format!("{}: {e}", command.join(" ")))?;
        Ok(child.id())
    }
}

/// A service the guest can kill and start: its pid file, and the command that starts it (empty: it is not started again).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub pid_file: PathBuf,
    pub start: Vec<String>,
}

/// The bytes a fill writes to leave `leave_mib` free on a filesystem of `total_kib` with `avail_kib` available. A fill is
/// refused on any filesystem bigger than [`MAX_FILL_KIB`], and when there is no space above the reserve.
pub fn fill_bytes(total_kib: u64, avail_kib: u64, leave_mib: u64) -> Result<u64, String> {
    if total_kib > MAX_FILL_KIB {
        return Err(format!(
            "the volume is {} MiB: a fill runs only on a dedicated volume of at most 1 GiB",
            total_kib / 1024
        ));
    }
    let reserve = leave_mib * 1024;
    if avail_kib <= reserve {
        return Err(format!(
            "the volume has at most {} MiB free already",
            avail_kib / 1024
        ));
    }
    Ok((avail_kib - reserve) * 1024)
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

/// The guest's actions: its fault proxies by target, its services, its volumes by (service, volume), and its bridge.
pub struct Guest<H: Host, R: Runner> {
    pub host: H,
    pub runner: R,
    pub proxies: BTreeMap<String, Proxy>,
    pub services: BTreeMap<String, Service>,
    pub volumes: BTreeMap<(String, String), PathBuf>,
    pub device: String,
}

impl<H: Host, R: Runner> Guest<H, R> {
    fn kill(&mut self, target: &str) -> Result<(), String> {
        let service = self.service(target)?;
        let text = std::fs::read_to_string(&service.pid_file)
            .map_err(|e| format!("{}: {e}", service.pid_file.display()))?;
        let pid = text.trim().to_string();
        if pid.parse::<u32>().is_err() {
            return Err(format!("{target}'s pid file does not hold a pid"));
        }
        self.runner.run(&["kill".into(), "-9".into(), pid])
    }

    fn start(&mut self, target: &str) -> Result<(), String> {
        let service = self.service(target)?.clone();
        if service.start.is_empty() {
            return Ok(());
        }
        let pid = self.runner.spawn(&service.start)?;
        std::fs::write(&service.pid_file, pid.to_string())
            .map_err(|e| format!("{}: {e}", service.pid_file.display()))
    }

    fn service(&self, target: &str) -> Result<&Service, String> {
        self.services
            .get(target)
            .ok_or_else(|| format!("the guest knows no process of {target}"))
    }

    fn volume(&self, target: &str, volume: &Option<String>) -> Result<PathBuf, String> {
        let name = volume.clone().unwrap_or_default();
        self.volumes
            .get(&(target.to_string(), name.clone()))
            .cloned()
            .ok_or_else(|| format!("the guest has no volume {name} of {target}"))
    }
}

impl<H: Host, R: Runner> Actuator for Guest<H, R> {
    fn available(&self, scenario: &Scenario) -> Result<(), String> {
        match scenario.action {
            Action::Fault => match scenario.fault {
                Some(Fault::Netem { .. }) => netem::check(&self.host, &self.device),
                _ if self.proxies.contains_key(&scenario.target) => Ok(()),
                _ => Err(format!("no fault proxy in front of {}", scenario.target)),
            },
            Action::Kill | Action::Restart => {
                let service = self.service(&scenario.target)?;
                if !self.host.exists(&service.pid_file) {
                    return Err(format!("{} has no pid file in the guest", scenario.target));
                }
                if scenario.action == Action::Restart && service.start.is_empty() {
                    return Err(format!(
                        "a restart needs the start command of {}",
                        scenario.target
                    ));
                }
                Ok(())
            }
            Action::FillDisk => {
                let dir = self.volume(&scenario.target, &scenario.volume)?;
                if !self.host.is_linux() || !self.host.tool("fallocate") {
                    return Err("filling a disk needs fallocate in a Linux guest".into());
                }
                let (total, avail) = self
                    .host
                    .disk_kib(&dir)
                    .ok_or_else(|| format!("{} cannot be measured", dir.display()))?;
                fill_bytes(total, avail, scenario.leave_mib).map(|_| ())
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
                Some(Fault::Netem {
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
                    let proxy = self
                        .proxies
                        .get(&scenario.target)
                        .ok_or_else(|| format!("no fault proxy in front of {}", scenario.target))?;
                    proxy.set_fault(if on { fault } else { Fault::Healthy });
                    Ok(())
                }
                None => Err("a fault action names no fault".into()),
            },
            Action::Kill => {
                if on {
                    self.kill(&scenario.target)
                } else {
                    self.start(&scenario.target)
                }
            }
            Action::Restart => {
                if on {
                    self.kill(&scenario.target)?;
                    self.start(&scenario.target)
                } else {
                    Ok(())
                }
            }
            Action::FillDisk => {
                let dir = self.volume(&scenario.target, &scenario.volume)?;
                let file = dir.join(FILL_FILE);
                let file = file.display().to_string();
                if on {
                    let (total, avail) = self
                        .host
                        .disk_kib(&dir)
                        .ok_or_else(|| format!("{} cannot be measured", dir.display()))?;
                    let bytes = fill_bytes(total, avail, scenario.leave_mib)?;
                    self.runner
                        .run(&["fallocate".into(), "-l".into(), bytes.to_string(), file])
                } else {
                    self.runner.run(&["rm".into(), "-f".into(), file])
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
        pub disk: Option<(u64, u64)>,
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
        fn disk_kib(&self, _: &Path) -> Option<(u64, u64)> {
            self.disk
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
        fn spawn(&mut self, command: &[String]) -> Result<u32, String> {
            self.commands.push(command.to_vec());
            Ok(4343)
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

    fn guest(host: FakeHost) -> Guest<FakeHost, Recorder> {
        Guest {
            host,
            runner: Recorder::default(),
            proxies: BTreeMap::new(),
            services: BTreeMap::new(),
            volumes: BTreeMap::new(),
            device: "apz0".into(),
        }
    }

    #[test]
    fn a_fill_is_refused_on_a_host_disk_and_leaves_its_reserve() {
        // 512 MiB dedicated, 400 MiB available, 16 MiB kept: 384 MiB is written.
        assert_eq!(
            fill_bytes(512 * 1024, 400 * 1024, 16),
            Ok(384 * 1024 * 1024)
        );
        let host = fill_bytes(200 * 1024 * 1024, 1024, 0).unwrap_err();
        assert!(host.contains("dedicated"), "{host}");
        let reserve = fill_bytes(512 * 1024, 10 * 1024, 16).unwrap_err();
        assert!(reserve.contains("already"), "{reserve}");
    }

    #[test]
    fn a_clock_moves_by_its_delta_from_now() {
        let command = clock_command(1_000, 3_600);
        assert_eq!(command, ["date", "-s", "@4600"]);
    }

    #[test]
    fn a_fault_is_set_on_its_proxy_and_lifted_after() {
        let upstream: std::net::SocketAddr = "127.0.0.1:9".parse().unwrap();
        let proxy = Proxy::start("127.0.0.1:0".parse().unwrap(), upstream, Fault::Healthy).unwrap();
        let mut guest = guest(FakeHost::default());
        guest.proxies.insert("cache".into(), proxy);
        let mut s = scenario(Action::Fault, "cache");
        s.fault = Some(Fault::Partition);
        assert_eq!(guest.available(&s), Ok(()));
        guest.act(&s, true).unwrap();
        assert_eq!(guest.proxies["cache"].fault(), Fault::Partition);
        guest.act(&s, false).unwrap();
        assert_eq!(guest.proxies["cache"].fault(), Fault::Healthy);

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
        let pid_file = dir.path().join("cache.pid");
        std::fs::write(&pid_file, "4242\n").unwrap();
        let mut guest = guest(FakeHost {
            paths: vec![pid_file.display().to_string()],
            ..FakeHost::default()
        });
        guest.services.insert(
            "cache".into(),
            Service {
                pid_file: pid_file.clone(),
                start: vec!["/usr/bin/cache".into(), "--port".into(), "6379".into()],
            },
        );
        guest.act(&scenario(Action::Kill, "cache"), true).unwrap();
        assert_eq!(guest.runner.commands[0], ["kill", "-9", "4242"]);
        guest.act(&scenario(Action::Kill, "cache"), false).unwrap();
        assert_eq!(guest.runner.commands[1][0], "/usr/bin/cache");
        assert_eq!(std::fs::read_to_string(&pid_file).unwrap(), "4343");

        guest
            .act(&scenario(Action::Restart, "cache"), true)
            .unwrap();
        assert_eq!(guest.runner.commands[2], ["kill", "-9", "4343"]);
        assert_eq!(guest.runner.commands[3][0], "/usr/bin/cache");
        assert_eq!(guest.available(&scenario(Action::Restart, "cache")), Ok(()));
        assert!(guest.available(&scenario(Action::Kill, "queue")).is_err());
    }

    #[test]
    fn netem_needs_a_linux_guest_with_tc_root_and_the_bridge() {
        let mut s = scenario(Action::Fault, "cache");
        s.fault = Some(Fault::Netem {
            delay_ms: 50,
            loss_percent: 2,
        });
        let mac = guest(FakeHost::default());
        assert!(mac.available(&s).unwrap_err().contains("Linux"));

        let mut ready = guest(FakeHost {
            linux: true,
            root: true,
            tools: vec!["tc".into()],
            paths: vec!["/sys/class/net/apz0".into()],
            ..FakeHost::default()
        });
        assert_eq!(ready.available(&s), Ok(()));
        ready.act(&s, true).unwrap();
        assert_eq!(
            ready.runner.commands[0].join(" "),
            "tc qdisc replace dev apz0 root netem delay 50ms loss 2%"
        );
    }

    #[test]
    fn a_disk_fill_is_available_only_on_a_small_dedicated_linux_volume() {
        let dir = PathBuf::from("/tmp/apz-volume");
        let mut s = scenario(Action::FillDisk, "cache");
        s.volume = Some("cache-data".into());
        s.leave_mib = 16;
        let mut linux = guest(FakeHost {
            linux: true,
            tools: vec!["fallocate".into()],
            disk: Some((512 * 1024, 400 * 1024)),
            ..FakeHost::default()
        });
        linux
            .volumes
            .insert(("cache".into(), "cache-data".into()), dir.clone());
        assert_eq!(linux.available(&s), Ok(()));
        linux.act(&s, true).unwrap();
        let fill = linux.runner.commands[0].clone();
        assert_eq!(fill[0..2], ["fallocate", "-l"]);
        assert_eq!(fill[2], (384u64 * 1024 * 1024).to_string());
        linux.act(&s, false).unwrap();
        assert_eq!(linux.runner.commands[1][0..2], ["rm", "-f"]);

        linux.host.disk = Some((100 * 1024 * 1024, 1024));
        assert!(linux.available(&s).unwrap_err().contains("dedicated"));
        let mut mac = guest(FakeHost {
            disk: Some((512 * 1024, 400 * 1024)),
            ..FakeHost::default()
        });
        mac.volumes
            .insert(("cache".into(), "cache-data".into()), dir);
        assert!(mac.available(&s).unwrap_err().contains("Linux guest"));
    }

    #[test]
    fn a_clock_skew_needs_root_in_a_linux_guest_and_moves_the_clock_back_after() {
        let mut s = scenario(Action::ClockSkew, "api");
        s.secs = Some(3600);
        let unprivileged = guest(FakeHost {
            linux: true,
            tools: vec!["date".into()],
            ..FakeHost::default()
        });
        assert!(unprivileged.available(&s).unwrap_err().contains("root"));
        let mut root = guest(FakeHost {
            linux: true,
            root: true,
            tools: vec!["date".into()],
            ..FakeHost::default()
        });
        assert_eq!(root.available(&s), Ok(()));
        root.act(&s, true).unwrap();
        root.act(&s, false).unwrap();
        let (on, off) = (&root.runner.commands[0], &root.runner.commands[1]);
        let at = |c: &Vec<String>| c[2].trim_start_matches('@').parse::<i64>().unwrap();
        // The skew is one hour forward, then one hour back: the two commands are two hours apart.
        assert!(((at(on) - at(off)) - 7200).abs() <= 5, "{on:?} {off:?}");
    }
}
