//! Per-VM respawn supervisor: the plaza node-daemon's VM lifecycle semantics on
//! the public spawner API. Keeps N slots alive; every VM that exits on its own
//! is respawned in its slot until the operator deadline; a VM the host kills is
//! not respawned. Usage:
//!
//! ```text
//! cargo run -p vmm --example fleet_supervisor -- \
//!   --kernel /boot/vmlinuz-linux --initrd target/guest/initrd.x86_64.gz \
//!   -n 4 --beats 2 --seconds 40
//! ```
//!
//! `--beats N` passes `plaza.beats=N` so each guest reboots itself after N
//! heartbeats (churn mode — required with `--seconds`, because the bounded run
//! retires slots when their incumbent reboots after the deadline; the watcher
//! threads own the handles, so long-lived guests are bounded-mode-unreachable).
//! Without `--seconds` the supervisor runs until the process is stopped
//! (SIGINT/SIGTERM); without `--beats` the guests run long-lived.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use vmm::spawner::{self, VmExit, VmSpec};

struct Args {
    kernel: PathBuf,
    initrd: Option<PathBuf>,
    n: usize,
    memory_mib: usize,
    beats: Option<u32>,
    seconds: Option<u64>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        kernel: PathBuf::new(),
        initrd: None,
        n: 4,
        memory_mib: 512,
        beats: None,
        seconds: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| it.next().ok_or_else(|| format!("missing value for {flag}"));
        match arg.as_str() {
            "--kernel" => args.kernel = PathBuf::from(value("--kernel")?),
            "--initrd" => args.initrd = Some(PathBuf::from(value("--initrd")?)),
            "-n" => args.n = value("-n")?.parse().map_err(|e| format!("-n: {e}"))?,
            "--mem-mib" => {
                args.memory_mib = value("--mem-mib")?
                    .parse()
                    .map_err(|e| format!("--mem-mib: {e}"))?;
            }
            "--beats" => {
                args.beats = Some(
                    value("--beats")?
                        .parse()
                        .map_err(|e| format!("--beats: {e}"))?,
                );
            }
            "--seconds" => {
                args.seconds = Some(
                    value("--seconds")?
                        .parse()
                        .map_err(|e| format!("--seconds: {e}"))?,
                );
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if args.kernel.as_os_str().is_empty() {
        return Err("--kernel is required".into());
    }
    if args.seconds.is_some() && args.beats.is_none() {
        return Err("--seconds requires --beats (bounded mode retires slots on churn)".into());
    }
    Ok(args)
}

fn spec_for(args: &Args) -> VmSpec {
    let cmdline = match args.beats {
        Some(beats) => format!("console=ttyS0 plaza.beats={beats}"),
        None => "console=ttyS0".to_string(),
    };
    VmSpec {
        kernel: args.kernel.clone(),
        cmdline,
        vcpus: 1,
        memory_mib: args.memory_mib,
        initrd: args.initrd.clone(),
        gic: None,
        vsock: None,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args()?;
    println!(
        "fleet-supervisor: {} slots, {} MiB each{}{}",
        args.n,
        args.memory_mib,
        args.beats
            .map(|b| format!(", churn every {b} heartbeats"))
            .unwrap_or_default(),
        args.seconds
            .map(|s| format!(", retiring after {s}s"))
            .unwrap_or_default()
    );

    let spec = spec_for(&args);
    let stop = Arc::new(AtomicBool::new(false));

    let mut watchers = Vec::with_capacity(args.n);
    for slot in 0..args.n {
        let handle = spawner::spawn(spec.clone())?;
        let spec = spec.clone();
        let stop = Arc::clone(&stop);
        watchers.push(thread::spawn(move || {
            let slot = slot;
            let mut handle = handle;
            let mut incarnation = 1u32;
            loop {
                match handle.wait() {
                    Ok(VmExit::GuestExited { reset }) => {
                        println!(
                            "fleet-supervisor: vm {slot} incarnation {incarnation} exited \
                             (reset={reset})"
                        );
                        if stop.load(Ordering::SeqCst) {
                            println!("fleet-supervisor: vm {slot}: deadline passed; retiring slot");
                            return;
                        }
                        match spawner::spawn(spec.clone()) {
                            Ok(next) => {
                                handle = next;
                                incarnation += 1;
                            }
                            Err(err) => {
                                println!(
                                    "fleet-supervisor: vm {slot} respawn failed: {err}; \
                                     retiring slot"
                                );
                                return;
                            }
                        }
                    }
                    Ok(VmExit::Killed) => {
                        println!(
                            "fleet-supervisor: vm {slot} incarnation {incarnation} killed \
                             by the host; retiring slot"
                        );
                        return;
                    }
                    Err(err) => {
                        println!("fleet-supervisor: vm {slot} wait failed: {err}; retiring slot");
                        return;
                    }
                }
            }
        }));
    }

    match args.seconds {
        Some(seconds) => {
            thread::sleep(Duration::from_secs(seconds));
            println!("fleet-supervisor: operator deadline; no more respawns");
            stop.store(true, Ordering::SeqCst);
            // Churn guests reboot within one beats window; allow two for slack.
            let deadline = std::time::Instant::now() + Duration::from_secs(60);
            for watcher in &watchers {
                while !watcher.is_finished() {
                    if std::time::Instant::now() > deadline {
                        println!("fleet-supervisor: a watcher did not retire in time; exiting");
                        return Ok(());
                    }
                    thread::sleep(Duration::from_millis(200));
                }
            }
            println!("fleet-supervisor: fleet drained");
        }
        None => {
            // Unbounded: run until the process is stopped (SIGINT/SIGTERM).
            for watcher in watchers {
                let _ = watcher.join();
            }
        }
    }
    Ok(())
}
