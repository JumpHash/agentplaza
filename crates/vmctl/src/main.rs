//! `vmctl` — spawn N AgentPlaza microVMs, stream their serial consoles and
//! report how they stopped.
//!
//! ```text
//! vmctl --kernel /boot/vmlinuz-linux -n 3 --mem-mib 512 \
//!       --cmdline 'console=ttyS0 panic=-1' --timeout-secs 90
//! ```
//!
//! Guest serial output is printed line by line, prefixed with `[vm <id>] `.
//! The exit status is 0 only if every VM stopped because the guest exited
//! (shutdown, reset or a kernel panic with `panic=-1`); a VM that had to be
//! killed — because it hit `--timeout-secs` or because a sibling failed to
//! boot — makes the exit status 1.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use vmm::spawner::{GicVersion, VmExit, VmHandle, VmSpec, spawn_n};

/// How often the printer polls the VMs' serial captures.
const PRINTER_INTERVAL: Duration = Duration::from_millis(20);

const USAGE: &str = "\
usage: vmctl --kernel PATH --cmdline STR [--initrd PATH] [-n N] [--vcpus V]
             [--mem-mib M] [--timeout-secs T] [--gic v2|v3]

  --kernel PATH      guest kernel image (uncompressed ELF vmlinux or bzImage)
  --cmdline STR      kernel command line
  --initrd PATH      optional initrd image
  -n N               number of VMs to spawn (default 1)
  --vcpus V          vCPUs per VM (default 1)
  --mem-mib M        guest memory per VM in MiB (default 512)
  --timeout-secs T   stop VMs still running after T seconds (default 120, 0 = no timeout)
  --gic v2|v3        aarch64 interrupt controller (ignored on x86_64)
  -h, --help         print this help

The VMM appends `pci=off loglevel=7` to --cmdline unless the command line
already sets those options itself (this port has no PCI device, and the
guest's only console is the serial capture this tool prints).";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    let config = match Config::parse(&args) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("vmctl: {err}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    run(config)
}

/// Everything `vmctl` accepts on its command line.
#[derive(Debug)]
struct Config {
    kernel: PathBuf,
    cmdline: String,
    initrd: Option<PathBuf>,
    count: usize,
    vcpus: u8,
    memory_mib: usize,
    /// `None` means "never time out".
    timeout: Option<Duration>,
    gic: Option<GicVersion>,
}

impl Config {
    fn parse(args: &[String]) -> Result<Self, String> {
        let defaults = VmSpec::default();
        let mut kernel: Option<PathBuf> = None;
        let mut cmdline: Option<String> = None;
        let mut initrd = None;
        let mut count = 1usize;
        let mut vcpus = defaults.vcpus;
        let mut memory_mib = defaults.memory_mib;
        let mut timeout = Some(Duration::from_secs(120));
        let mut gic = None;

        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let mut value = |name: &str| -> Result<String, String> {
                args.next()
                    .cloned()
                    .ok_or_else(|| format!("{name} needs a value"))
            };
            match arg.as_str() {
                "--kernel" => kernel = Some(PathBuf::from(value("--kernel")?)),
                "--cmdline" => cmdline = Some(value("--cmdline")?),
                "--initrd" => initrd = Some(PathBuf::from(value("--initrd")?)),
                "-n" => {
                    count = value("-n")?
                        .parse()
                        .map_err(|err| format!("invalid -n: {err}"))?
                }
                "--vcpus" => {
                    vcpus = value("--vcpus")?
                        .parse()
                        .map_err(|err| format!("invalid --vcpus: {err}"))?
                }
                "--mem-mib" => {
                    memory_mib = value("--mem-mib")?
                        .parse()
                        .map_err(|err| format!("invalid --mem-mib: {err}"))?
                }
                "--timeout-secs" => {
                    let secs: u64 = value("--timeout-secs")?
                        .parse()
                        .map_err(|err| format!("invalid --timeout-secs: {err}"))?;
                    timeout = (secs > 0).then(|| Duration::from_secs(secs));
                }
                "--gic" => {
                    gic = Some(match value("--gic")?.as_str() {
                        "v2" => GicVersion::V2,
                        "v3" => GicVersion::V3,
                        other => return Err(format!("invalid --gic: {other}")),
                    })
                }
                other => return Err(format!("unknown argument: {other}")),
            }
        }

        if count == 0 {
            return Err("-n must be at least 1".to_string());
        }
        if vcpus == 0 {
            return Err("--vcpus must be at least 1".to_string());
        }
        if memory_mib == 0 {
            return Err("--mem-mib must be at least 1".to_string());
        }

        Ok(Config {
            kernel: kernel.ok_or("--kernel is required")?,
            cmdline: cmdline.ok_or("--cmdline is required")?,
            initrd,
            count,
            vcpus,
            memory_mib,
            timeout,
            gic,
        })
    }
}

fn run(config: Config) -> ExitCode {
    let spec = VmSpec {
        kernel: config.kernel,
        cmdline: config.cmdline,
        vcpus: config.vcpus,
        memory_mib: config.memory_mib,
        initrd: config.initrd,
        gic: config.gic,
    };

    let mut vms = match spawn_n(spec, config.count) {
        Ok(vms) => vms,
        Err(err) => {
            eprintln!("vmctl: {err}");
            return ExitCode::FAILURE;
        }
    };

    // Stream the serial consoles while the VMs run.
    let sinks: Vec<Arc<Mutex<Vec<u8>>>> = vms.iter().map(VmHandle::serial_sink).collect();
    let printing = Arc::new(AtomicBool::new(true));
    let printer = {
        let printing = Arc::clone(&printing);
        thread::spawn(move || stream_serial(&sinks, &printing))
    };

    let deadline = config.timeout.map(|timeout| Instant::now() + timeout);
    let mut exits: Vec<Option<VmExit>> = vec![None; vms.len()];
    let mut failure: Option<String> = None;

    for (id, vm) in vms.iter_mut().enumerate() {
        let waited = match deadline {
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                vm.wait_timeout(remaining)
            }
            None => vm.wait().map(Some),
        };
        match waited {
            Ok(Some(exit)) => exits[id] = Some(exit),
            Ok(None) => {
                failure = Some(format!(
                    "timed out after {}s waiting for VM {id}",
                    config.timeout.map_or(0, |timeout| timeout.as_secs())
                ));
                break;
            }
            Err(err) => {
                failure = Some(format!("VM {id}: {err}"));
                break;
            }
        }
    }

    // Bring down whatever is still running (a straggler after the timeout, or
    // the siblings of a VM that failed).
    for vm in &mut vms {
        let id = vm.id();
        if let Err(err) = vm.kill() {
            eprintln!("vmctl: failed to stop VM {id}: {err}");
            failure.get_or_insert_with(|| format!("VM {id} did not stop when killed"));
        }
    }

    printing.store(false, Ordering::SeqCst);
    if printer.join().is_err() {
        eprintln!("vmctl: the serial printer thread panicked");
    }

    for (id, exit) in exits.iter().enumerate() {
        let exit = exit.unwrap_or(VmExit::Killed);
        eprintln!(
            "vmctl: vm {id}: {}",
            match exit {
                VmExit::GuestExited => "guest exited",
                VmExit::Killed => "killed",
            }
        );
        if exit != VmExit::GuestExited {
            failure.get_or_insert_with(|| format!("VM {id} was killed instead of exiting"));
        }
    }

    match failure {
        Some(message) => {
            eprintln!("vmctl: {message}");
            ExitCode::FAILURE
        }
        None => ExitCode::SUCCESS,
    }
}

/// Prints each VM's new serial output, line by line, as `[vm <id>] <line>`.
///
/// Runs until `printing` is cleared (after which it drains whatever is left,
/// including a trailing line without a newline).
fn stream_serial(sinks: &[Arc<Mutex<Vec<u8>>>], printing: &AtomicBool) {
    // Per VM: how much has been printed, and the partial line in flight.
    let mut state: Vec<(usize, Vec<u8>)> = sinks.iter().map(|_| (0, Vec::new())).collect();

    loop {
        let still_printing = printing.load(Ordering::SeqCst);
        for (id, sink) in sinks.iter().enumerate() {
            drain_serial(id, sink, &mut state[id]);
        }
        if !still_printing {
            break;
        }
        thread::sleep(PRINTER_INTERVAL);
    }

    for (id, (_, pending)) in state.iter_mut().enumerate() {
        if !pending.is_empty() {
            print_line(id, pending);
            pending.clear();
        }
    }
}

/// Appends the VM's unprinted serial bytes to `state` and prints the lines
/// that completed.
fn drain_serial(id: usize, sink: &Arc<Mutex<Vec<u8>>>, state: &mut (usize, Vec<u8>)) {
    let (printed, pending) = state;
    let sink = sink.lock().unwrap_or_else(PoisonError::into_inner);
    if sink.len() <= *printed {
        return;
    }
    let chunk: Vec<u8> = sink[*printed..].to_vec();
    *printed = sink.len();
    drop(sink);

    let mut rest = &chunk[..];
    while let Some(newline) = rest.iter().position(|&byte| byte == b'\n') {
        pending.extend_from_slice(&rest[..=newline]);
        print_line(id, pending);
        pending.clear();
        rest = &rest[newline + 1..];
    }
    pending.extend_from_slice(rest);
}

/// Prints `line` (which may end in a newline) with the VM's prefix.
fn print_line(id: usize, line: &[u8]) {
    let text = String::from_utf8_lossy(line);
    let text = text.strip_suffix('\n').unwrap_or(&text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    println!("[vm {id}] {text}");
}
