// This crate is safe Rust only: `forbid(unsafe_code)` makes any future
// `unsafe` block a compile error.
#![forbid(unsafe_code)]

//! `vmctl` — spawn N AgentPlaza microVMs, stream their serial consoles and
//! report how they stopped.
//!
//! ```text
//! vmctl --kernel /boot/vmlinuz-linux -n 3 --mem-mib 512 \
//!       --cmdline 'console=ttyS0 panic=-1' --timeout-secs 90
//! ```
//!
//! Guest serial output is printed line by line, prefixed with `[vm <id>] `.
//! With `--console` the tool also forwards its own standard input to VM 0's
//! serial line, which is how a guest whose `PID 1` (or shell) reads `ttyS0`
//! gets driven from the host.
//!
//! The exit status is 0 only if every VM stopped because the guest exited
//! (shutdown, reset or a kernel panic with `panic=-1`); a VM that had to be
//! stopped by the host — because it hit `--timeout-secs` or because a sibling
//! failed to boot — makes the exit status 1.

use std::io::{self, Read};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use vmm::spawner::{
    GicVersion, MAX_GUEST_CID, MIN_GUEST_CID, VmExit, VmHandle, VmRuntime, VmSpec, VsockConfig,
    spawn_each,
};

/// How often the printer polls the VMs' serial captures.
const PRINTER_INTERVAL: Duration = Duration::from_millis(20);

/// How many bytes one console-input read takes from stdin: the UART's input
/// queue is 64 bytes and `VmHandle::serial_input` refuses a slice it cannot
/// take whole, so a read never asks for more than that.
const CONSOLE_INPUT_CHUNK: usize = 64;

/// Where the per-VM vsock Unix sockets live unless `--uds-dir` says otherwise;
/// VM `i`'s muxer binds `{DEFAULT_UDS_DIR}/vm{cid}`.
const DEFAULT_UDS_DIR: &str = "/tmp/plaza-vsock";

/// How long the console has to stay quiet (no new guest output) before the
/// reader considers the guest's console live and starts forwarding its input.
///
/// A guest's serial driver reads and discards bytes while it brings the port
/// up (`serial8250`'s power-on reads of the receive register), exactly as a
/// real 16550 would hand them over, so input written into the port *before the
/// guest's console is up* can be eaten before any program ever sees it -
/// measured on this host: 3 of the first 8 bytes at 50 ms, none at 600 ms.
/// Waiting for a quiet console is what a person does before typing at a boot
/// log, and it is the only readiness signal a console tool has.
const CONSOLE_SETTLE: Duration = Duration::from_millis(300);

/// The longest the reader waits for that quiet moment: a guest that chatters
/// forever still gets its input, just later, instead of never.
const CONSOLE_SETTLE_CAP: Duration = Duration::from_secs(10);

/// How long the console-input thread waits before retrying a slice the guest
/// has not drained yet.
const CONSOLE_INPUT_RETRY: Duration = Duration::from_millis(10);

/// How many times in a row a console-input slice is retried before it is
/// reported and dropped: a guest that has stopped reading must not turn the
/// host's console stream into an unbounded backlog.
const CONSOLE_INPUT_ATTEMPTS: usize = 100;

const USAGE: &str = "\
usage: vmctl --kernel PATH --cmdline STR [--initrd PATH] [-n N] [--vcpus V]
             [--mem-mib M] [--timeout-secs T] [--gic v2|v3] [--vsock-cid BASE]
             [--console]

  --kernel PATH      guest kernel image (uncompressed ELF vmlinux or bzImage)
  --cmdline STR      kernel command line
  --initrd PATH      optional initrd image
  -n N               number of VMs to spawn (default 1)
  --vcpus V          vCPUs per VM (default 1)
  --mem-mib M        guest memory per VM in MiB (default 512)
  --timeout-secs T   stop VMs still running after T seconds (default 120, 0 = no timeout)
  --gic v2|v3        aarch64 interrupt controller (ignored on x86_64)
  --vsock-cid BASE   give the VMs vsock devices, VM i getting CID BASE+i (default:
                     no vsock device; BASE is a guest CID, so at least 3)
  --uds-dir DIR      directory for the per-VM vsock Unix sockets (default
                     /tmp/plaza-vsock; VM i's muxer binds DIR/vm{BASE+i})
  --console          forward this tool's stdin to VM 0's serial console
  -h, --help         print this help

The VMM appends `pci=off loglevel=7` to --cmdline unless the command line
already sets those options itself (this port has no PCI device, and the
guest's only console is the serial capture this tool prints).

--console drives VM 0 only, even with -n > 1: a console is a conversation, and
one stdin cannot be a conversation with several guests. Guest serial output for
every VM is printed either way.

Before its first write, --console waits for the guest's console to go quiet: a
guest's serial driver discards bytes that arrive while it is still bringing the
port up, exactly as a real UART's would, so input sent at a boot log can be
eaten before any program reads it. Input sent once the guest is up (the usual
case) is unaffected.

--vsock-cid gives each VM a virtio-vsock device so its guest can reach the
host's orchestrator over AF_VSOCK (CID 2). The tool assigns the CIDs (VM i gets
BASE+i) because the caller — the orchestrator — has to know which guest is
which; every VM needs its own CID, and CIDs 0, 1 and 2 belong to the vsock
stack. The device exists only if this option is given: a guest without it has
no way to reach the host at all, which is the point of the option.";

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
    /// Guest CID of VM 0; VM `i` gets `vsock_cid + i`. `None` spawns VMs with
    /// no vsock device.
    vsock_cid: Option<u64>,
    /// Directory holding the per-VM vsock Unix sockets; VM `i`'s muxer binds
    /// `{uds_dir}/vm{cid}`. Ignored when `vsock_cid` is `None`.
    uds_dir: PathBuf,
    /// Forward this process's stdin to VM 0's serial console.
    console: bool,
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
        let mut vsock_cid = None;
        let mut uds_dir = PathBuf::from(DEFAULT_UDS_DIR);
        let mut console = false;

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
                "--vsock-cid" => {
                    let base: u64 = value("--vsock-cid")?
                        .parse()
                        .map_err(|err| format!("invalid --vsock-cid: {err}"))?;
                    if base < MIN_GUEST_CID {
                        return Err(format!(
                            "--vsock-cid must be at least {MIN_GUEST_CID}: CIDs 0, 1 and 2 are \
                             the hypervisor, loopback and the host"
                        ));
                    }
                    if base > MAX_GUEST_CID {
                        return Err(format!(
                            "--vsock-cid must be at most {MAX_GUEST_CID}: the kernel refuses \
                             VMADDR_CID_ANY and above"
                        ));
                    }
                    vsock_cid = Some(base);
                }
                "--console" => console = true,
                "--uds-dir" => uds_dir = PathBuf::from(value("--uds-dir")?),
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

        if let Some(base) = vsock_cid {
            let last = base.checked_add(count as u64 - 1).ok_or_else(|| {
                "--vsock-cid and -n together run past the last guest CID".to_string()
            })?;
            if last > MAX_GUEST_CID {
                return Err("--vsock-cid and -n together run past the last guest CID".to_string());
            }
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
            vsock_cid,
            uds_dir,
            console,
        })
    }
}

fn run(config: Config) -> ExitCode {
    let base = VmSpec {
        kernel: config.kernel,
        cmdline: config.cmdline,
        vcpus: config.vcpus,
        memory_mib: config.memory_mib,
        initrd: config.initrd,
        gic: config.gic,
        vsock: None,
    };
    // Every VM's vsock device carries its own CID and its own host Unix socket
    // path: VM `id` gets the base CID plus its id, and the muxer binds
    // `{uds_dir}/vm{cid}`, because the host side (the orchestrator) identifies
    // guests by CID and reaches each through its own socket.
    let vsock_cid = config.vsock_cid;
    let uds_dir = config.uds_dir;
    let mut vms = match spawn_each(config.count, |id| {
        let mut spec = base.clone();
        spec.vsock = vsock_cid.map(|base| {
            let guest_cid = base + u64::from(id);
            VsockConfig {
                guest_cid,
                uds_path: uds_dir.join(format!("vm{guest_cid}")),
            }
        });
        spec
    }) {
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

    // Console input. The reader gets the runtime rather than the handle: the
    // handle is needed here, with `&mut self`, to wait for and stop the VMs.
    if config.console {
        let runtime = vms[0].runtime();
        thread::spawn(move || forward_console_input(runtime));
    }

    // A timeout so large that the deadline overflows `Instant` means "no
    // timeout" (as `VmHandle::wait_timeout` treats it), instead of panicking.
    let deadline = config
        .timeout
        .and_then(|timeout| Instant::now().checked_add(timeout));
    let mut exits: Vec<Option<VmExit>> = vec![None; vms.len()];
    // The VMs the deadline stopped, rather than the guest stopping itself.
    let mut stopped_after_deadline: Vec<u32> = Vec::new();
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
            // The VM outlived its deadline. That is what `--timeout-secs`
            // asks for, not a failure to wait: the VM is stopped below, so the
            // run ends in the state that matters - every VM joined - instead
            // of in a half-stopped one.
            Ok(None) => {
                eprintln!(
                    "vmctl: VM {id} was still running after {}s; stopping it",
                    config.timeout.map_or(0, |timeout| timeout.as_secs())
                );
                stopped_after_deadline.push(id as u32);
            }
            Err(err) => {
                failure = Some(format!("VM {id}: {err}"));
                break;
            }
        }
    }

    // Bring down whatever is still running (a straggler after the timeout, or
    // the siblings of a VM that failed). A VM that cannot be stopped within
    // the kill timeout is the failure this tool has to report: it is the one
    // state that leaves a guest running behind the caller's back.
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
                VmExit::GuestExited { reset: false } => "guest exited",
                VmExit::GuestExited { reset: true } => "guest exited (reset)",
                VmExit::Killed => "killed",
            }
        );
        if !matches!(exit, VmExit::GuestExited { .. }) {
            failure.get_or_insert_with(|| {
                if stopped_after_deadline.contains(&(id as u32)) {
                    format!("VM {id} was stopped by the {}s timeout, not by its guest",
                        config.timeout.map_or(0, |timeout| timeout.as_secs()))
                } else {
                    format!("VM {id} was killed instead of exiting")
                }
            });
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

/// Forwards this process's stdin to VM 0's serial console, until stdin ends or
/// the VM stops taking input.
///
/// The reader runs on its own thread because the VM's own wait (`--timeout-secs`
/// and `VmHandle::wait`) blocks, and because a console is interactive: bytes
/// have to reach the guest as they are typed, not once the VM has stopped.
/// Nothing here is buffered by line — the guest's terminal does the echoing and
/// the line editing, exactly as it would for a physical serial port — so the
/// thread forwards each read as it arrives.
///
/// A read is forwarded as one call: `VmHandle::serial_input` takes a slice
/// whole or not at all, so a slice the console has no room for yet is retried
/// (the bytes are still in the FIFO-free case *not* queued, so nothing is
/// duplicated) until the guest drains the queue. A guest that stopped reading
/// for [`CONSOLE_INPUT_ATTEMPTS`] tries is reported and the slice is dropped,
/// rather than turning this tool into an unbounded backlog of guest input.
fn forward_console_input(runtime: Arc<VmRuntime>) {
    wait_for_console(&runtime);
    let mut stdin = io::stdin().lock();
    let mut chunk = [0u8; CONSOLE_INPUT_CHUNK];
    loop {
        let count = match stdin.read(&mut chunk) {
            // End of input: a piped script ran out, or the terminal was
            // closed. The VM keeps running until it stops on its own.
            Ok(0) => return,
            Ok(count) => count,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => {
                eprintln!("vmctl: cannot read stdin for --console: {err}");
                return;
            }
        };

        let mut attempts = 0;
        loop {
            match runtime.serial_input(&chunk[..count]) {
                Ok(()) => break,
                Err(err) => {
                    // The VM has been asked to stop: this thread's console is
                    // over, and no further retry can be answered.
                    if runtime.stop_requested() {
                        return;
                    }
                    attempts += 1;
                    if attempts >= CONSOLE_INPUT_ATTEMPTS {
                        eprintln!("vmctl: dropped {count} bytes of console input: {err}");
                        break;
                    }
                    thread::sleep(CONSOLE_INPUT_RETRY);
                }
            }
        }
    }
}

/// Waits until the guest's console is live: it has produced output and then
/// stayed quiet for [`CONSOLE_SETTLE`] (or [`CONSOLE_SETTLE_CAP`] has passed).
///
/// Input written to a UART whose guest has not finished bringing the port up
/// is discarded by the guest's driver, so this is not politeness but the
/// difference between a piped script reaching the guest and being eaten by the
/// guest's power-on register reads.
fn wait_for_console(runtime: &VmRuntime) {
    let started = Instant::now();
    let mut last_len = runtime.serial.lock().unwrap_or_else(PoisonError::into_inner).len();
    let mut quiet_since = None;
    loop {
        thread::sleep(Duration::from_millis(20));
        let len = runtime.serial.lock().unwrap_or_else(PoisonError::into_inner).len();
        if len != last_len {
            last_len = len;
            quiet_since = None;
        } else if len > 0 {
            let since = *quiet_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= CONSOLE_SETTLE {
                return;
            }
        }
        if started.elapsed() >= CONSOLE_SETTLE_CAP {
            eprintln!(
                "vmctl: the guest's console is still producing output after {}s; \
                 forwarding stdin to it anyway",
                CONSOLE_SETTLE_CAP.as_secs()
            );
            return;
        }
        if runtime.stop_requested() {
            return;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses the required flags plus `extra` flag/value pairs.
    fn parse(extra: &[(&str, &str)]) -> Result<Config, String> {
        let mut argv: Vec<String> = vec![
            "--kernel".into(),
            "/boot/vmlinuz".into(),
            "--cmdline".into(),
            "console=ttyS0".into(),
        ];
        for (flag, value) in extra {
            argv.push((*flag).to_string());
            argv.push((*value).to_string());
        }
        Config::parse(&argv)
    }

    #[test]
    fn the_last_guest_cid_is_u32_max_minus_one() {
        assert_eq!(MAX_GUEST_CID, 4_294_967_294);
        assert!(MAX_GUEST_CID < u32::MAX as u64);
    }

    #[test]
    fn vsock_cid_upper_bound_is_enforced() {
        assert!(parse(&[("--vsock-cid", "3")]).is_ok());
        assert!(parse(&[("--vsock-cid", "4294967294")]).is_ok()); // u32::MAX - 1
        assert!(parse(&[("--vsock-cid", "2")]).is_err()); // below MIN_GUEST_CID
        assert!(parse(&[("--vsock-cid", "4294967295")]).is_err()); // u32::MAX
        assert!(parse(&[("--vsock-cid", "4294967296")]).is_err()); // u32::MAX + 1
    }

    #[test]
    fn vsock_cid_and_n_must_stay_under_the_transport_max() {
        assert!(parse(&[("-n", "1"), ("--vsock-cid", "4294967294")]).is_ok());
        assert!(parse(&[("-n", "2"), ("--vsock-cid", "4294967294")]).is_err());
    }
}
