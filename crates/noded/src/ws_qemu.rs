//! QEMU/KVM workstation VMs: the second launch backend of a node (docs/design/workstations.md, section 10).
//!
//! `apz-vmm` boots Linux by direct boot only and attaches no PCI device, so a Windows workstation VM (UEFI) and a VFIO
//! GPU go to QEMU when the node has it. A Linux VM goes to QEMU when asked for, and boots the node's desktop kernel
//! directly, as the plaza backend does.
//!
//! Kept here: no NIC (`-nic none`, rule 0010), so the only channel out is the vsock device. The process is the noded
//! user, never root, under QEMU's own `-sandbox`. Each disk is a qcow2 overlay, so the base image is never written. The
//! control channel is QMP over a unix socket in the VM's directory.
//!
//! The argv and the `qemu-img` commands are pure and unit-tested. No test launches QEMU or touches `/dev/kvm`.

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The longest a unix socket path may be, in bytes: `sun_path` holds 108 bytes with its terminator.
const SOCKET_PATH_MAX: usize = 107;
/// The lowest guest CID a vsock device may have: 0 to 2 are the reserved CIDs (hypervisor, local, host).
const VSOCK_FIRST_GUEST_CID: u32 = 3;
/// How long a stopping VM has to power itself down before QEMU is told to quit.
pub const POWERDOWN_GRACE: Duration = Duration::from_secs(60);
/// How long QEMU has to exit after `quit`, before it is killed.
const QUIT_GRACE: Duration = Duration::from_secs(5);
/// How long a started VM has to answer QMP, which means its machine is up and its firmware files are read.
pub const READY_WAIT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(100);
/// The most lines one QMP reply may run to before it is taken as broken.
const MAX_REPLY_LINES: usize = 1_000;

/// The QMP socket in a VM's directory.
pub const QMP_SOCKET: &str = "qmp.sock";
/// The pidfile QEMU writes in a VM's directory.
pub const PID_FILE: &str = "qemu.pid";
/// The serial console's log in a VM's directory.
pub const SERIAL_LOG: &str = "serial.log";
/// QEMU's own stderr log in a VM's directory.
pub const LOG: &str = "qemu.log";

/// The magic at the start of a qcow2 file.
const QCOW2_MAGIC: [u8; 4] = *b"QFI\xfb";
/// The OVMF pairs for x86_64, as (code, variables) under the root. The first pair whose two files both exist wins.
const X86_FIRMWARE: [(&str, &str); 6] = [
    (
        "usr/share/edk2/x64/OVMF_CODE.4m.fd",
        "usr/share/edk2/x64/OVMF_VARS.4m.fd",
    ),
    (
        "usr/share/edk2/x64/OVMF_CODE.fd",
        "usr/share/edk2/x64/OVMF_VARS.fd",
    ),
    (
        "usr/share/OVMF/OVMF_CODE_4M.fd",
        "usr/share/OVMF/OVMF_VARS_4M.fd",
    ),
    ("usr/share/OVMF/OVMF_CODE.fd", "usr/share/OVMF/OVMF_VARS.fd"),
    (
        "usr/share/edk2/ovmf/OVMF_CODE.fd",
        "usr/share/edk2/ovmf/OVMF_VARS.fd",
    ),
    (
        "usr/share/qemu/edk2-x86_64-code.fd",
        "usr/share/qemu/edk2-i386-vars.fd",
    ),
];

/// The guest architectures a node can run under QEMU: the host's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    /// The architecture this daemon runs on.
    pub fn host() -> Self {
        if cfg!(target_arch = "aarch64") {
            Self::Aarch64
        } else {
            Self::X86_64
        }
    }

    fn program(self) -> &'static str {
        match self {
            Self::X86_64 => "qemu-system-x86_64",
            Self::Aarch64 => "qemu-system-aarch64",
        }
    }
}

/// UEFI firmware: the read-only code, and the variable store each VM copies its own from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Firmware {
    pub code: PathBuf,
    pub vars: PathBuf,
}

/// What a node has for QEMU: the program for its architecture, `qemu-img`, and on x86_64 the OVMF firmware.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Available {
    pub program: PathBuf,
    pub img: PathBuf,
    pub arch: Arch,
    /// Present on x86_64 (the Windows path boots UEFI); none on aarch64, which boots Linux directly.
    pub firmware: Option<Firmware>,
}

/// The directories a program is looked for in: `PATH`, then the usual binary directories.
pub fn path_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    dirs.push(PathBuf::from("/usr/bin"));
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs
}

/// The first of `dirs` holding `name` as an executable file.
pub fn find_program(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    dirs.iter()
        .map(|dir| dir.join(name))
        .find(|path| is_executable(path))
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// The OVMF pair under `root` (the file system root in production), or `None` when no complete pair is there.
pub fn find_firmware(root: &Path) -> Option<Firmware> {
    X86_FIRMWARE.iter().find_map(|(code, vars)| {
        let code = root.join(code);
        let vars = root.join(vars);
        (code.is_file() && vars.is_file()).then_some(Firmware { code, vars })
    })
}

/// What this node has for QEMU, reading devices and files under `root`: `/dev/kvm` and `/dev/vhost-vsock` (the
/// vsock device the hub relay reaches a VM by), the program for `arch`, `qemu-img`, and on x86_64 the OVMF pair.
/// `None` when any of them is missing.
pub fn detect_in(root: &Path, dirs: &[PathBuf], arch: Arch) -> Option<Available> {
    if !(root.join("dev/kvm").exists() && root.join("dev/vhost-vsock").exists()) {
        return None;
    }
    let program = find_program(dirs, arch.program())?;
    let img = find_program(dirs, "qemu-img")?;
    let firmware = match arch {
        Arch::X86_64 => Some(find_firmware(root)?),
        Arch::Aarch64 => None,
    };
    Some(Available {
        program,
        img,
        arch,
        firmware,
    })
}

/// This node's QEMU, when it has one. QEMU here runs on KVM, so only Linux nodes report it.
pub fn detect() -> Option<Available> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    detect_in(Path::new("/"), &path_dirs(), Arch::host())
}

/// How a VM boots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Boot<'a> {
    /// UEFI: the read-only code, and this VM's own variable store. The join config is not here: the guest reads it over its
    /// vsock from the node's relay, as a Linux guest does.
    Uefi { code: &'a Path, vars: &'a Path },
    /// Direct boot of the desktop guest's kernel and initramfs, with the command line its daemon reads.
    Direct {
        kernel: &'a Path,
        initrd: &'a Path,
        cmdline: &'a str,
    },
}

/// One VM's launch: what it is, what it boots, and where its disk and its state are.
#[derive(Debug, Clone, Copy)]
pub struct Plan<'a> {
    /// The workstation's name (a DNS label).
    pub name: &'a str,
    pub arch: Arch,
    pub cpus: u8,
    pub ram_mb: u32,
    /// A Windows guest: it boots UEFI, from a SATA disk, with the standard VGA.
    pub windows: bool,
    pub boot: Boot<'a>,
    /// The qcow2 overlay the VM boots from.
    pub disk: &'a Path,
    /// The VM's directory: the QMP socket, the pidfile and the logs go here.
    pub dir: &'a Path,
    /// The guest CID of the vsock device, for a VM with a hub (the relay reaches it there); `None` attaches no vsock device.
    pub cid: Option<u32>,
    /// A PCI address (`DDDD:BB:DD.F`) passed through with VFIO; already checked against sysfs.
    pub vfio: Option<&'a str>,
}

fn pair(args: &mut Vec<String>, flag: &str, value: impl Into<String>) {
    args.push(flag.to_string());
    args.push(value.into());
}

/// The QEMU argv of a VM. Refused, with the reason, for a shape QEMU here cannot launch: a path with a comma (QEMU
/// reads commas as option separators), a QMP socket path too long for a unix socket, a Windows guest that does not
/// boot UEFI, or one on an architecture without its firmware.
pub fn argv(plan: &Plan) -> Result<Vec<String>, String> {
    let sock = plan.dir.join(QMP_SOCKET);
    if sock.as_os_str().len() > SOCKET_PATH_MAX {
        return Err(format!(
            "the workstation's directory is too long for its QMP socket: {}",
            plan.dir.display()
        ));
    }
    if let Some(cid) = plan.cid
        && cid < VSOCK_FIRST_GUEST_CID
    {
        return Err(format!(
            "a workstation's guest CID must be at least {VSOCK_FIRST_GUEST_CID}, not {cid}"
        ));
    }
    let mut paths: Vec<&Path> = vec![plan.disk, plan.dir];
    match plan.boot {
        Boot::Uefi { code, vars } => paths.extend([code, vars]),
        Boot::Direct { kernel, initrd, .. } => paths.extend([kernel, initrd]),
    }
    if let Some(bad) = paths
        .iter()
        .find(|path| path.to_string_lossy().contains(','))
    {
        return Err(format!(
            "a path with a comma cannot be given to QEMU: {}",
            bad.display()
        ));
    }
    match (plan.windows, plan.boot, plan.arch) {
        (true, Boot::Direct { .. }, _) => {
            return Err("a Windows guest boots with UEFI, not by direct boot".into());
        }
        (true, _, Arch::Aarch64) => {
            return Err("a Windows guest under QEMU runs on an x86_64 node".into());
        }
        _ => {}
    }

    let (machine, display) = match plan.arch {
        Arch::X86_64 if plan.windows => ("q35,accel=kvm", vec!["-vga", "std"]),
        Arch::X86_64 => ("q35,accel=kvm", vec!["-device", "virtio-vga"]),
        Arch::Aarch64 => ("virt,accel=kvm", vec!["-device", "virtio-gpu-pci"]),
    };
    let mut args: Vec<String> = [
        "-name",
        plan.name,
        "-machine",
        machine,
        "-cpu",
        "host",
        "-no-user-config",
        "-nodefaults",
        // No network card, no netdev: the vsock device below is the only channel out (rule 0010).
        "-nic",
        "none",
        "-display",
        "none",
    ]
    .iter()
    .map(|arg| arg.to_string())
    .collect();
    pair(&mut args, "-smp", plan.cpus.to_string());
    pair(&mut args, "-m", format!("{}M", plan.ram_mb));
    args.extend(display.iter().map(|arg| arg.to_string()));

    match plan.boot {
        Boot::Uefi { code, vars } => {
            pair(
                &mut args,
                "-drive",
                format!(
                    "if=pflash,format=raw,unit=0,readonly=on,file={}",
                    code.display()
                ),
            );
            pair(
                &mut args,
                "-drive",
                format!("if=pflash,format=raw,unit=1,file={}", vars.display()),
            );
        }
        Boot::Direct {
            kernel,
            initrd,
            cmdline,
        } => {
            pair(&mut args, "-kernel", kernel.display().to_string());
            pair(&mut args, "-initrd", initrd.display().to_string());
            pair(&mut args, "-append", cmdline);
        }
    }

    let disk = plan.disk.display();
    if plan.windows {
        pair(&mut args, "-device", "ahci,id=ahci");
        pair(
            &mut args,
            "-drive",
            format!("if=none,id=disk0,format=qcow2,file={disk}"),
        );
        pair(&mut args, "-device", "ide-hd,drive=disk0,bus=ahci.0");
    } else {
        pair(
            &mut args,
            "-drive",
            format!("if=none,id=disk0,format=qcow2,discard=unmap,file={disk}"),
        );
        pair(&mut args, "-device", "virtio-blk-pci,drive=disk0");
    }

    if let Some(cid) = plan.cid {
        pair(
            &mut args,
            "-device",
            format!("vhost-vsock-pci,guest-cid={cid}"),
        );
    }
    if let Some(pci) = plan.vfio {
        pair(&mut args, "-device", format!("vfio-pci,host={pci}"));
    }
    pair(
        &mut args,
        "-serial",
        format!("file:{}", plan.dir.join(SERIAL_LOG).display()),
    );
    pair(
        &mut args,
        "-qmp",
        format!("unix:{},server,nowait", sock.display()),
    );
    pair(
        &mut args,
        "-pidfile",
        plan.dir.join(PID_FILE).display().to_string(),
    );
    pair(
        &mut args,
        "-sandbox",
        "on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny",
    );
    Ok(args)
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The format of a base image: qcow2 by its magic, else raw. The containers QEMU would probe for are refused, since
/// the format is given, never probed.
pub fn base_format(path: &Path) -> Result<&'static str, String> {
    let mut magic = [0_u8; 4];
    let read = fs::File::open(path)
        .and_then(|mut file| file.read(&mut magic))
        .map_err(|err| format!("cannot read the image {}: {err}", path.display()))?;
    if read == magic.len() && magic == QCOW2_MAGIC {
        return Ok("qcow2");
    }
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "vhd" | "vhdx" | "vmdk" | "vdi" | "vpc" | "qcow" | "qed" => Err(format!(
            "the image {} is a {ext} image: convert it to raw or qcow2",
            path.display()
        )),
        _ => Ok("raw"),
    }
}

/// `qemu-img create`: a qcow2 overlay of `backing` (in `format`), `size_gb` GiB, or the backing's size when none is given.
pub fn overlay_args(
    overlay: &Path,
    backing: &Path,
    format: &str,
    size_gb: Option<u32>,
) -> Vec<String> {
    let mut args = vec![
        "create".to_string(),
        "-f".into(),
        "qcow2".into(),
        "-b".into(),
        path_arg(backing),
        "-F".into(),
        format.into(),
        path_arg(overlay),
    ];
    if let Some(gb) = size_gb {
        args.push(format!("{gb}G"));
    }
    args
}

/// `qemu-img convert`: writes `src`, with its whole backing chain, as one self-contained qcow2 file.
pub fn flatten_args(src: &Path, dst: &Path) -> Vec<String> {
    vec![
        "convert".to_string(),
        "-f".into(),
        "qcow2".into(),
        "-O".into(),
        "qcow2".into(),
        path_arg(src),
        path_arg(dst),
    ]
}

/// Runs `qemu-img` with `args`, and names its stderr when it fails.
pub fn run_qemu_img(img: &Path, args: &[String]) -> Result<(), String> {
    let out = Command::new(img)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("cannot run {}: {err}", img.display()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "qemu-img failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Freezes a stopped VM's disk as its snapshot. The disk, with its backing chain, is flattened into the snapshot file,
/// and the lease overlay starts again over that file. Chains never grow past one overlay.
pub fn snapshot(img: &Path, disk: &Path, snapshot: &Path) -> Result<(), String> {
    let tmp = snapshot.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    if let Err(err) = run_qemu_img(img, &flatten_args(disk, &tmp)) {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    if let Err(err) = fs::rename(&tmp, snapshot) {
        let _ = fs::remove_file(&tmp);
        return Err(format!("cannot replace {}: {err}", snapshot.display()));
    }
    revert(img, disk, snapshot)
}

/// Starts the lease overlay again over the snapshot: the disk becomes what the snapshot holds. The old overlay, and
/// whatever a lease wrote to it, is discarded.
pub fn revert(img: &Path, disk: &Path, snapshot: &Path) -> Result<(), String> {
    let tmp = disk.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    if let Err(err) = run_qemu_img(img, &overlay_args(&tmp, snapshot, "qcow2", None)) {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    fs::rename(&tmp, disk).map_err(|err| {
        let _ = fs::remove_file(&tmp);
        format!("cannot replace {}: {err}", disk.display())
    })
}

/// A QMP reply, once its line is read.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Return(Value),
    Error(String),
    /// An asynchronous event (`SHUTDOWN`, `STOP`, ...): not a reply.
    Event,
}

/// The line that asks QMP for `command`.
pub fn command_line(command: &str) -> String {
    format!("{}\n", json!({ "execute": command }))
}

/// Reads one QMP line. `None` for a line that is not a reply or an event.
pub fn parse_reply(line: &str) -> Option<Reply> {
    let value: Value = serde_json::from_str(line).ok()?;
    if let Some(ret) = value.get("return") {
        return Some(Reply::Return(ret.clone()));
    }
    if let Some(err) = value.get("error") {
        let desc = err
            .get("desc")
            .and_then(Value::as_str)
            .unwrap_or("QMP command failed");
        return Some(Reply::Error(desc.to_string()));
    }
    value.get("event").map(|_| Reply::Event)
}

/// A QMP session over any stream: the greeting is read, `qmp_capabilities` is sent, then commands are sent one at a time.
pub struct Qmp<S> {
    io: BufReader<S>,
}

impl<S: Read + Write> Qmp<S> {
    /// Reads QEMU's greeting and enables the commands.
    pub fn open(stream: S) -> Result<Self, String> {
        let mut qmp = Self {
            io: BufReader::new(stream),
        };
        let greeting = qmp.line()?;
        let greeted = serde_json::from_str::<Value>(&greeting)
            .ok()
            .and_then(|value| value.get("QMP").cloned());
        if greeted.is_none() {
            return Err(format!("not a QMP greeting: {}", greeting.trim()));
        }
        qmp.execute("qmp_capabilities")?;
        Ok(qmp)
    }

    /// Sends `command` and returns its reply, skipping events. A reply that is an error becomes the error text.
    pub fn execute(&mut self, command: &str) -> Result<Value, String> {
        self.io
            .get_mut()
            .write_all(command_line(command).as_bytes())
            .map_err(|err| format!("QMP write: {err}"))?;
        for _ in 0..MAX_REPLY_LINES {
            let line = self.line()?;
            match parse_reply(&line) {
                Some(Reply::Return(value)) => return Ok(value),
                Some(Reply::Error(desc)) => return Err(desc),
                Some(Reply::Event) | None => {}
            }
        }
        Err(format!("no reply to {command} in {MAX_REPLY_LINES} lines"))
    }

    fn line(&mut self) -> Result<String, String> {
        let mut line = String::new();
        let read = self
            .io
            .read_line(&mut line)
            .map_err(|err| format!("QMP read: {err}"))?;
        if read == 0 {
            return Err("QMP closed the connection".into());
        }
        Ok(line)
    }
}

/// Sends one command to the QMP socket at `socket`, over a new connection.
#[cfg(unix)]
pub fn send(socket: &Path, command: &str) -> Result<Value, String> {
    let stream = std::os::unix::net::UnixStream::connect(socket)
        .map_err(|err| format!("cannot reach QMP at {}: {err}", socket.display()))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .and_then(|()| stream.set_write_timeout(Some(Duration::from_secs(10))))
        .map_err(|err| err.to_string())?;
    Qmp::open(stream)?.execute(command)
}

/// Sends one command to the QMP socket at `socket`. Off unix there is no socket to reach, so it is refused.
#[cfg(not(unix))]
pub fn send(_socket: &Path, _command: &str) -> Result<Value, String> {
    Err("QEMU workstation VMs need a unix socket for QMP, which this platform does not have".into())
}

/// The pid in a VM's pidfile, when that process is still a QEMU. It is only read: nothing is signalled.
pub fn running_pid(dir: &Path) -> Option<u32> {
    let pid: u32 = fs::read_to_string(dir.join(PID_FILE))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let cmdline = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    String::from_utf8_lossy(&cmdline)
        .contains("qemu-system")
        .then_some(pid)
}

/// The pid a QEMU VM left running by a previous noded can be taken back by, from what the node reads: its pidfile's text, its
/// `/proc` command line (`None` when there is no such process), and whether its QMP socket answers. A VM is taken back only
/// when all three say it is a running QEMU that this node can still reach.
pub fn reattach_pid(
    pidfile: Option<&str>,
    cmdline: Option<&[u8]>,
    qmp_answers: bool,
) -> Option<u32> {
    let pid = parse_pid(pidfile?)?;
    (is_qemu_cmdline(cmdline?) && qmp_answers).then_some(pid)
}

fn parse_pid(text: &str) -> Option<u32> {
    text.trim().parse().ok()
}

/// Whether a `/proc/<pid>/cmdline` names a QEMU system emulator.
fn is_qemu_cmdline(cmdline: &[u8]) -> bool {
    String::from_utf8_lossy(cmdline).contains("qemu-system")
}

/// A QEMU VM this node did not start, found running after a restart of noded. It is not this process's child, so it is
/// watched by its pid and stopped through QMP.
#[derive(Debug)]
pub struct Attached {
    pid: u32,
    socket: PathBuf,
}

/// Takes back the QEMU VM in `dir` if a previous noded left it running (see [`reattach_pid`]). Nothing is signalled.
pub fn reattach(dir: &Path) -> Option<Attached> {
    let pidfile = fs::read_to_string(dir.join(PID_FILE)).ok();
    let pid = pidfile.as_deref().and_then(parse_pid);
    let cmdline = pid.and_then(|pid| fs::read(format!("/proc/{pid}/cmdline")).ok());
    let socket = dir.join(QMP_SOCKET);
    let qmp_answers = cmdline.is_some() && send(&socket, "query-status").is_ok();
    reattach_pid(pidfile.as_deref(), cmdline.as_deref(), qmp_answers)
        .map(|pid| Attached { pid, socket })
}

impl Attached {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Whether the VM's QEMU has gone. Its pid is only checked, never signalled.
    pub fn exited(&self) -> bool {
        let cmdline = fs::read(format!("/proc/{}/cmdline", self.pid)).ok();
        !cmdline.is_some_and(|text| is_qemu_cmdline(&text))
    }

    /// Powers the guest down through QMP and waits `grace` for QEMU to exit, then quits it and waits briefly. An error when
    /// QEMU is still there: the VM stays recorded as running, and nothing is killed.
    pub fn shut_down(&mut self, grace: Duration) -> Result<(), String> {
        if self.exited() {
            return Ok(());
        }
        let _ = send(&self.socket, "system_powerdown");
        if self.wait_gone(grace) {
            return Ok(());
        }
        let _ = send(&self.socket, "quit");
        if self.wait_gone(QUIT_GRACE) {
            return Ok(());
        }
        Err(format!("QEMU (pid {}) did not exit after quit", self.pid))
    }

    fn wait_gone(&self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            if self.exited() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(POLL);
        }
    }
}

/// A running QEMU process for one VM.
pub struct Process {
    child: Child,
    socket: PathBuf,
}

impl Process {
    /// Starts QEMU with `argv`, its stderr going to `log`.
    pub fn start(program: &Path, argv: &[String], log: File, socket: PathBuf) -> io::Result<Self> {
        let child = Command::new(program)
            .args(argv)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(log))
            .spawn()?;
        Ok(Self { child, socket })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Whether QEMU has exited. Reaps it when it has.
    pub fn exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    /// Waits until QMP answers, or `limit` passes, or QEMU exits. True when it answered.
    pub fn wait_ready(&mut self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            if self.exited() {
                return false;
            }
            if send(&self.socket, "query-status").is_ok() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(POLL);
        }
    }

    /// Powers the guest down and waits `grace` for QEMU to exit. Then it quits QEMU and waits briefly. Then it kills
    /// the process. A VM that is already gone is stopped.
    pub fn shut_down(&mut self, grace: Duration) -> Result<(), String> {
        if self.exited() {
            return Ok(());
        }
        let _ = send(&self.socket, "system_powerdown");
        if self.wait_exit(grace) {
            return Ok(());
        }
        let _ = send(&self.socket, "quit");
        if self.wait_exit(QUIT_GRACE) {
            return Ok(());
        }
        self.child.kill().map_err(|err| err.to_string())?;
        self.child.wait().map(|_| ()).map_err(|err| err.to_string())
    }

    fn wait_exit(&mut self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            if self.exited() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(POLL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch(name: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "noded-ws-qemu-{}-{}-{name}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
        fs::write(path, b"x").expect("a file");
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("a mode");
    }

    fn has_pair(args: &[String], flag: &str, value: &str) -> bool {
        args.windows(2).any(|w| w[0] == flag && w[1] == value)
    }

    fn dir_plan() -> PathBuf {
        PathBuf::from("/home/u/.local/state/agentplaza/workstations/wv3")
    }

    fn linux_plan<'a>(dir: &'a Path, disk: &'a Path, cmdline: &'a str) -> Plan<'a> {
        Plan {
            name: "desk-1",
            arch: Arch::X86_64,
            cpus: 4,
            ram_mb: 8192,
            windows: false,
            boot: Boot::Direct {
                kernel: Path::new("/g/vmlinux"),
                initrd: Path::new("/g/initrd.gz"),
                cmdline,
            },
            disk,
            dir,
            cid: Some(4099),
            vfio: None,
        }
    }

    // QEMU runs only on a Linux node (`detect` refuses elsewhere): this asserts the KVM command line and its unix QMP
    // socket, which only exist there. A Windows host has no `-qmp unix:` socket to build.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_linux_plaza_disk_boots_directly_on_kvm_with_vsock_and_no_nic() {
        let dir = dir_plan();
        let disk = dir.join("disk.qcow2");
        let cmdline = "console=ttyS0 plaza.workstation=1";
        let args = argv(&linux_plan(&dir, &disk, cmdline)).expect("a Linux VM launches");

        assert!(has_pair(&args, "-machine", "q35,accel=kvm"));
        assert!(has_pair(&args, "-cpu", "host"));
        assert!(has_pair(&args, "-smp", "4"));
        assert!(has_pair(&args, "-m", "8192M"));
        assert!(has_pair(&args, "-kernel", "/g/vmlinux"));
        assert!(has_pair(&args, "-initrd", "/g/initrd.gz"));
        assert!(has_pair(&args, "-append", cmdline));
        assert!(
            !args
                .iter()
                .any(|a| a == "pflash" || a.contains("if=pflash"))
        );
        assert!(has_pair(&args, "-device", "virtio-blk-pci,drive=disk0"));
        assert!(has_pair(&args, "-device", "virtio-vga"));
        assert!(has_pair(&args, "-device", "vhost-vsock-pci,guest-cid=4099"));
        assert!(args.contains(&"-no-user-config".to_string()));
        assert!(args.contains(&"-nodefaults".to_string()));
        assert!(has_pair(&args, "-display", "none"));
        assert!(has_pair(
            &args,
            "-qmp",
            &format!("unix:{}/qmp.sock,server,nowait", dir.display())
        ));
        assert!(has_pair(
            &args,
            "-sandbox",
            "on,obsolete=deny,elevateprivileges=deny,spawn=deny,resourcecontrol=deny"
        ));
        assert!(has_pair(
            &args,
            "-pidfile",
            &format!("{}/qemu.pid", dir.display())
        ));
        assert!(has_pair(
            &args,
            "-serial",
            &format!("file:{}/serial.log", dir.display())
        ));
        assert!(!args.iter().any(|a| a.contains("vfio")));
        assert!(!args.iter().any(|a| a.contains("join")));
    }

    #[test]
    fn a_windows_guest_boots_uefi_from_its_own_variables_and_a_sata_disk() {
        let dir = dir_plan();
        let disk = dir.join("disk.qcow2");
        let vars = dir.join("vars.fd");
        let plan = Plan {
            name: "win-1",
            arch: Arch::X86_64,
            cpus: 6,
            ram_mb: 8192,
            windows: true,
            boot: Boot::Uefi {
                code: Path::new("/usr/share/OVMF/OVMF_CODE_4M.fd"),
                vars: &vars,
            },
            disk: &disk,
            dir: &dir,
            cid: Some(4100),
            vfio: None,
        };
        let args = argv(&plan).expect("a Windows VM launches");

        assert!(has_pair(
            &args,
            "-drive",
            "if=pflash,format=raw,unit=0,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd"
        ));
        assert!(has_pair(
            &args,
            "-drive",
            &format!("if=pflash,format=raw,unit=1,file={}", vars.display())
        ));
        assert!(
            !args.iter().any(|a| a == "-fw_cfg"),
            "the join config comes over vsock, never through fw_cfg"
        );
        assert!(has_pair(&args, "-device", "vhost-vsock-pci,guest-cid=4100"));
        assert!(has_pair(&args, "-device", "ide-hd,drive=disk0,bus=ahci.0"));
        assert!(has_pair(&args, "-device", "ahci,id=ahci"));
        assert!(has_pair(&args, "-vga", "std"));
        assert!(!args.iter().any(|a| a.contains("virtio-blk")));
        assert!(!args.iter().any(|a| a == "-kernel" || a == "-append"));
        assert!(args.contains(&"-nic".to_string()));
    }

    #[test]
    fn a_vfio_gpu_is_passed_through_only_when_the_plan_names_one() {
        let dir = dir_plan();
        let disk = dir.join("disk.qcow2");
        let mut plan = linux_plan(&dir, &disk, "console=ttyS0");
        assert!(!argv(&plan).unwrap().iter().any(|a| a.contains("vfio-pci")));
        plan.vfio = Some("0000:01:00.0");
        let args = argv(&plan).unwrap();
        assert!(has_pair(&args, "-device", "vfio-pci,host=0000:01:00.0"));
    }

    #[test]
    fn no_argv_carries_a_network_card_or_a_netdev() {
        let dir = dir_plan();
        let disk = dir.join("disk.qcow2");
        let vars = dir.join("vars.fd");
        let uefi = Plan {
            name: "win-1",
            arch: Arch::X86_64,
            cpus: 2,
            ram_mb: 4096,
            windows: true,
            boot: Boot::Uefi {
                code: Path::new("/c.fd"),
                vars: &vars,
            },
            disk: &disk,
            dir: &dir,
            cid: Some(4100),
            vfio: Some("0000:01:00.0"),
        };
        let arm = Plan {
            arch: Arch::Aarch64,
            windows: false,
            ..linux_plan(&dir, &disk, "console=ttyS0")
        };
        for plan in [linux_plan(&dir, &disk, "x"), uefi, arm] {
            let args = argv(&plan).expect("a launch");
            let nics: Vec<&String> = args.iter().filter(|a| *a == "-nic").collect();
            assert_eq!(nics.len(), 1, "exactly one -nic");
            assert!(has_pair(&args, "-nic", "none"));
            assert!(
                !args.iter().any(|a| {
                    a.contains("netdev")
                        || a.contains("virtio-net")
                        || a.contains("e1000")
                        || a.contains("rtl8139")
                        || a.contains("vmxnet")
                }),
                "{args:?}"
            );
        }
    }

    #[test]
    fn an_aarch64_linux_guest_uses_the_virt_machine_and_virtio_display() {
        let dir = dir_plan();
        let disk = dir.join("disk.qcow2");
        let plan = Plan {
            arch: Arch::Aarch64,
            ..linux_plan(&dir, &disk, "console=ttyS0")
        };
        let args = argv(&plan).unwrap();
        assert!(has_pair(&args, "-machine", "virt,accel=kvm"));
        assert!(has_pair(&args, "-device", "virtio-gpu-pci"));
    }

    #[test]
    fn argv_refuses_unsafe_paths_and_a_windows_guest_without_uefi_x86_64() {
        // a_comma_in_a_path_and_a_long_socket_path_are_refused: a comma in the path and an over-long socket path are refused.
        {
            let dir = PathBuf::from("/srv/a,b/wv1");
            let disk = dir.join("disk.qcow2");
            let err = argv(&linux_plan(&dir, &disk, "x")).unwrap_err();
            assert!(err.contains("comma"), "{err}");

            let long = PathBuf::from(format!("/{}", "d".repeat(SOCKET_PATH_MAX)));
            let disk = long.join("disk.qcow2");
            let err = argv(&linux_plan(&long, &disk, "x")).unwrap_err();
            assert!(err.contains("too long"), "{err}");
        }

        // a_windows_guest_must_boot_uefi_on_x86_64: a Windows guest needs UEFI, and on aarch64 it is refused.
        {
            let dir = dir_plan();
            let disk = dir.join("disk.qcow2");
            let direct_windows = Plan {
                windows: true,
                ..linux_plan(&dir, &disk, "x")
            };
            assert!(argv(&direct_windows).unwrap_err().contains("UEFI"));

            let vars = dir.join("vars.fd");
            let arm_windows = Plan {
                arch: Arch::Aarch64,
                windows: true,
                boot: Boot::Uefi {
                    code: Path::new("/c.fd"),
                    vars: &vars,
                },
                ..linux_plan(&dir, &disk, "x")
            };
            assert!(argv(&arm_windows).unwrap_err().contains("x86_64"));
        }
    }

    #[test]
    fn the_firmware_is_the_first_code_and_vars_pair_present_under_the_root() {
        let root = scratch("firmware");
        assert_eq!(find_firmware(&root), None, "no firmware yet");

        // A code file with no matching variables is not a pair.
        touch(&root.join("usr/share/OVMF/OVMF_CODE_4M.fd"));
        assert_eq!(
            find_firmware(&root),
            None,
            "an unpaired code file is refused"
        );

        touch(&root.join("usr/share/OVMF/OVMF_VARS_4M.fd"));
        assert_eq!(
            find_firmware(&root),
            Some(Firmware {
                code: root.join("usr/share/OVMF/OVMF_CODE_4M.fd"),
                vars: root.join("usr/share/OVMF/OVMF_VARS_4M.fd"),
            })
        );

        // The Arch layout is searched before Debian's, so it wins when both are present.
        touch(&root.join("usr/share/edk2/x64/OVMF_CODE.4m.fd"));
        touch(&root.join("usr/share/edk2/x64/OVMF_VARS.4m.fd"));
        assert_eq!(
            find_firmware(&root).map(|f| f.code),
            Some(root.join("usr/share/edk2/x64/OVMF_CODE.4m.fd"))
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_program_must_be_an_executable_file_and_detection_needs_every_part() {
        let root = scratch("detect");
        let bin = root.join("bin");
        touch(&bin.join("qemu-system-x86_64"));
        assert_eq!(
            find_program(std::slice::from_ref(&bin), "qemu-system-x86_64"),
            None,
            "a file without the execute bit is not a program"
        );
        make_executable(&bin.join("qemu-system-x86_64"));
        touch(&bin.join("qemu-img"));
        make_executable(&bin.join("qemu-img"));
        touch(&root.join("usr/share/OVMF/OVMF_CODE_4M.fd"));
        touch(&root.join("usr/share/OVMF/OVMF_VARS_4M.fd"));
        let dirs = vec![bin.clone()];

        // No /dev/kvm and no vhost-vsock: this node has no QEMU.
        assert_eq!(detect_in(&root, &dirs, Arch::X86_64), None);
        touch(&root.join("dev/kvm"));
        assert_eq!(
            detect_in(&root, &dirs, Arch::X86_64),
            None,
            "no vhost-vsock"
        );
        touch(&root.join("dev/vhost-vsock"));

        let found = detect_in(&root, &dirs, Arch::X86_64).expect("every part is here");
        assert_eq!(found.program, bin.join("qemu-system-x86_64"));
        assert_eq!(found.img, bin.join("qemu-img"));
        assert_eq!(found.arch, Arch::X86_64);
        assert!(found.firmware.is_some());

        let no_img = vec![root.join("nowhere")];
        assert_eq!(detect_in(&root, &no_img, Arch::X86_64), None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_qmp_frame_is_one_command_line_and_a_reply_or_an_event_parses() {
        assert_eq!(
            command_line("query-status"),
            "{\"execute\":\"query-status\"}\n"
        );
        assert_eq!(
            parse_reply("{\"return\": {\"status\": \"running\"}}\n"),
            Some(Reply::Return(json!({"status": "running"})))
        );
        assert_eq!(
            parse_reply("{\"error\": {\"class\": \"GenericError\", \"desc\": \"nope\"}}"),
            Some(Reply::Error("nope".into()))
        );
        assert_eq!(
            parse_reply("{\"event\": \"SHUTDOWN\", \"data\": {}}"),
            Some(Reply::Event)
        );
        assert_eq!(parse_reply("not json"), None);
    }

    #[cfg(unix)]
    #[test]
    fn qmp_execute_skips_events_and_returns_the_reply_or_the_error_text() {
        // qmp_skips_events_and_returns_the_reply_to_a_command: events are skipped and the command reply is returned.
        {
            use std::os::unix::net::UnixStream;
            let (client, server) = UnixStream::pair().expect("a connected pair");
            let fake = std::thread::spawn(move || {
                let mut writer = server.try_clone().expect("a second end");
                let mut reader = BufReader::new(server);
                writer
                    .write_all(b"{\"QMP\": {\"version\": {}, \"capabilities\": []}}\n")
                    .unwrap();
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert!(line.contains("qmp_capabilities"), "{line}");
                writer.write_all(b"{\"return\": {}}\n").unwrap();
                line.clear();
                reader.read_line(&mut line).unwrap();
                assert!(line.contains("system_powerdown"), "{line}");
                writer
                    .write_all(
                        b"{\"event\": \"SHUTDOWN\", \"data\": {}}\n{\"return\": {\"status\": \"ok\"}}\n",
                    )
                    .unwrap();
            });
            let mut qmp = Qmp::open(client).expect("a greeted session");
            let value = qmp.execute("system_powerdown").expect("the reply");
            assert_eq!(value["status"], "ok");
            fake.join().expect("the fake server ends cleanly");
        }

        // a_qmp_error_reply_is_the_error_text: a QMP error reply becomes its desc text.
        {
            use std::os::unix::net::UnixStream;
            let (client, server) = UnixStream::pair().expect("a connected pair");
            let fake = std::thread::spawn(move || {
                let mut writer = server.try_clone().expect("a second end");
                let mut reader = BufReader::new(server);
                writer.write_all(b"{\"QMP\": {}}\n").unwrap();
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                writer.write_all(b"{\"return\": {}}\n").unwrap();
                line.clear();
                reader.read_line(&mut line).unwrap();
                writer
                    .write_all(
                        b"{\"error\": {\"class\": \"GenericError\", \"desc\": \"device busy\"}}\n",
                    )
                    .unwrap();
            });
            let mut qmp = Qmp::open(client).expect("a greeted session");
            assert_eq!(qmp.execute("quit"), Err("device busy".into()));
            fake.join().expect("the fake server ends cleanly");
        }
    }

    #[test]
    fn the_overlay_and_flatten_commands_name_their_backing_and_size() {
        let overlay = Path::new("/v/disk.qcow2");
        let base = Path::new("/img/base.img");
        assert_eq!(
            overlay_args(overlay, base, "raw", Some(40)),
            [
                "create",
                "-f",
                "qcow2",
                "-b",
                "/img/base.img",
                "-F",
                "raw",
                "/v/disk.qcow2",
                "40G"
            ]
        );
        assert_eq!(
            overlay_args(overlay, Path::new("/v/snapshot.qcow2"), "qcow2", None),
            [
                "create",
                "-f",
                "qcow2",
                "-b",
                "/v/snapshot.qcow2",
                "-F",
                "qcow2",
                "/v/disk.qcow2"
            ]
        );
        assert_eq!(
            flatten_args(overlay, Path::new("/v/snapshot.tmp")),
            [
                "convert",
                "-f",
                "qcow2",
                "-O",
                "qcow2",
                "/v/disk.qcow2",
                "/v/snapshot.tmp"
            ]
        );
    }

    #[test]
    fn a_base_image_is_qcow2_by_its_magic_and_refused_in_other_containers() {
        let dir = scratch("format");
        let raw = dir.join("base.img");
        fs::write(&raw, vec![0_u8; 64]).unwrap();
        assert_eq!(base_format(&raw), Ok("raw"));

        let qcow = dir.join("win.img");
        fs::write(&qcow, [&QCOW2_MAGIC[..], &[0_u8; 60][..]].concat()).unwrap();
        assert_eq!(base_format(&qcow), Ok("qcow2"));

        let vhdx = dir.join("win.vhdx");
        fs::write(&vhdx, b"vhdxfile").unwrap();
        assert!(base_format(&vhdx).unwrap_err().contains("convert"));
        assert!(base_format(&dir.join("missing.img")).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_vm_with_no_vsock_device_attaches_none_and_a_reserved_cid_is_refused() {
        let dir = dir_plan();
        let disk = dir.join("disk.qcow2");
        let no_hub = Plan {
            cid: None,
            ..linux_plan(&dir, &disk, "x")
        };
        let args = argv(&no_hub).expect("a VM with no hub launches");
        assert!(!args.iter().any(|a| a.contains("vhost-vsock")), "{args:?}");
        assert!(args.contains(&"-nic".to_string()) && args.contains(&"none".to_string()));

        for reserved in [0, 1, 2] {
            let plan = Plan {
                cid: Some(reserved),
                ..linux_plan(&dir, &disk, "x")
            };
            assert!(
                argv(&plan).unwrap_err().contains("at least 3"),
                "CID {reserved} is reserved"
            );
        }
    }

    #[test]
    fn a_qemu_left_running_is_taken_back_only_when_its_pid_is_qemu_and_qmp_answers() {
        let qemu = b"/usr/bin/qemu-system-x86_64\0-name\0desk-1\0-nic\0none\0";
        assert_eq!(reattach_pid(Some("4242\n"), Some(qemu), true), Some(4242));
        assert_eq!(
            reattach_pid(Some("4242"), Some(qemu), false),
            None,
            "a QEMU whose QMP does not answer is not taken back"
        );
        let sleep = b"/usr/bin/sleep\x00600\x00";
        assert_eq!(
            reattach_pid(Some("4242"), Some(sleep), true),
            None,
            "a pid reused by another process is not a VM"
        );
        assert_eq!(
            reattach_pid(Some("4242"), None, true),
            None,
            "no such process"
        );
        assert_eq!(reattach_pid(None, Some(qemu), true), None, "no pidfile");
        assert_eq!(reattach_pid(Some("not a pid"), Some(qemu), true), None);
        assert_eq!(reattach_pid(Some(""), Some(qemu), true), None);
    }

    #[test]
    fn a_pidfile_is_a_running_qemu_only_when_its_process_is_one() {
        let dir = scratch("pid");
        assert_eq!(running_pid(&dir), None, "no pidfile");
        // This test process is not QEMU, so its pid is not reported, and nothing is signalled.
        fs::write(dir.join(PID_FILE), std::process::id().to_string()).unwrap();
        assert_eq!(running_pid(&dir), None);
        fs::write(dir.join(PID_FILE), "not a pid").unwrap();
        assert_eq!(running_pid(&dir), None);
        let _ = fs::remove_dir_all(&dir);
    }
}
