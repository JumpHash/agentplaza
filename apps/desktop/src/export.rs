//! Transcript export: where a transcript goes, what it holds, and the one
//! `std::fs` write that puts it there.
//!
//! The register has no file dialog (iced offers none, and the crate takes no
//! dependency for one), so the destination is a path in a text box: the model
//! derives a default in the process's temp directory, the operator may type
//! another, and the write is a plain `std::fs::write` whose error is kept as
//! text for the notice line.
//!
//! Nothing here knows about iced: the path arithmetic, the transcript bytes
//! and the write are all unit-tested, and the reducer decides *when* to ask
//! for a write while `app`'s runtime is the only place that performs one.

use std::path::{Path, PathBuf};

use crate::api::{Run, Vm, VmState};
use crate::exit::exit_kind;
use crate::model::{clock_label, duration_label, instant_label};
use crate::session::{self, SessionRead};

/// The first line of every exported transcript, so a file found later says
/// what it is.
pub const TRANSCRIPT_HEADER: &str = "# AgentPlaza transcript";

/// The first line of every exported run report, which is not a transcript: it
/// carries the run's facts and each VM's derived session summary over the
/// captures.
pub const REPORT_HEADER: &str = "# AgentPlaza report";

/// What an export covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportTarget {
    /// The VM the console is aimed at.
    Vm,
    /// Every VM of the run, one labelled section each.
    Run,
    /// The whole run as a report: its facts, every transcript, and each VM's
    /// derived session summary.
    Report,
}

impl ExportTarget {
    /// Every target, in the order the switch offers them.
    pub const ALL: [ExportTarget; 3] = [ExportTarget::Vm, ExportTarget::Run, ExportTarget::Report];

    /// The word the switch shows.
    pub fn label(self) -> &'static str {
        match self {
            ExportTarget::Vm => "this vm",
            ExportTarget::Run => "all vms",
            ExportTarget::Report => "report",
        }
    }
}

/// The export box: which transcript, and where it goes.
///
/// The path is derived from the console's target until the operator types one
/// of their own; an edit sticks, so a path the operator chose is never
/// silently replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    /// What an export covers.
    pub target: ExportTarget,
    /// The path in the box.
    pub path: String,
    /// Whether the operator has typed a path of their own.
    pub edited: bool,
}

impl Default for Export {
    fn default() -> Self {
        Self {
            target: ExportTarget::Vm,
            path: String::new(),
            edited: false,
        }
    }
}

impl Export {
    /// Derives the box's path for the run and VM in hand, unless the operator
    /// has typed a path of their own.
    pub fn derive(&mut self, run: &str, vm: u32, dir: &Path) {
        if self.edited {
            return;
        }
        self.path = export_path(dir, run, vm, self.target);
    }

    /// What an export will write to: the box, less the surrounding space a
    /// paste tends to carry.
    pub fn to_write(&self) -> &str {
        self.path.trim()
    }
}

/// One transcript write the runtime has to make: the bytes, and where they go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportCall {
    /// What the write covers: one VM, the run's transcripts, or its report.
    pub target: ExportTarget,
    /// The run the transcript is of.
    pub run: String,
    /// The VM it is of, or `None` for every VM of the run.
    pub vm: Option<u32>,
    /// The path to write it to.
    pub path: String,
    /// The bytes to write.
    pub contents: String,
}

/// The session's telemetry export box: the CSV's path, and whether the
/// operator typed one.
///
/// The same shape as the transcript's box and the same one write at the end of
/// it: a path in a text box, derived from the temp directory until the
/// operator types their own, and the disk's own words when it cannot land.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvExport {
    /// The path in the box.
    pub path: String,
    /// Whether the operator has typed a path of their own.
    pub edited: bool,
}

impl Default for CsvExport {
    fn default() -> Self {
        Self {
            path: String::new(),
            edited: false,
        }
    }
}

impl CsvExport {
    /// Fills an empty box with the session's default path, stamped where the
    /// session's first observation was taken; a path the operator typed is
    /// never replaced.
    pub fn derive(&mut self, dir: &Path, at_ms: u64) {
        if self.edited || !self.to_write().is_empty() {
            return;
        }
        self.path = telemetry_path(dir, at_ms);
    }

    /// What an export will write to: the box, less the surrounding space a
    /// paste tends to carry.
    pub fn to_write(&self) -> &str {
        self.path.trim()
    }
}

/// One session CSV the runtime has to write: the bytes, and where they go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvCall {
    /// The path to write it to.
    pub path: String,
    /// The bytes to write.
    pub contents: String,
}

/// The directory a derived path is built in: the process's temp directory, so
/// an export never lands in the current directory by accident.
pub fn default_dir() -> PathBuf {
    std::env::temp_dir()
}

/// The path a transcript of `run`/`vm` gets in `dir`.
///
/// The id becomes a filename token first (see [`file_token`]): it arrives from
/// the daemon, and a name that can contain a separator could otherwise steer
/// the write out of `dir`.
pub fn export_path(dir: &Path, run: &str, vm: u32, target: ExportTarget) -> String {
    let run = file_token(run);
    let name = match target {
        ExportTarget::Vm => format!("plaza-{run}-{vm}.txt"),
        ExportTarget::Run => format!("plaza-{run}-all.txt"),
        ExportTarget::Report => format!("plaza-{run}-report.txt"),
    };
    dir.join(name).to_string_lossy().into_owned()
}

/// The path a session's telemetry CSV gets in `dir`.
///
/// The name carries the wall clock it was derived at, in milliseconds since
/// the Unix epoch, so two sessions in the same directory sort by the time they
/// started and never overwrite one another's file by accident.
pub fn telemetry_path(dir: &Path, at_ms: u64) -> String {
    dir.join(format!("plaza-telemetry-{at_ms}.csv"))
        .to_string_lossy()
        .into_owned()
}

/// A run id as a filename token: ASCII letters, digits, `-` and `_` are kept,
/// everything else becomes `_`, and an id with nothing kept becomes `run`.
pub fn file_token(run: &str) -> String {
    let token: String = run
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect();
    if token.is_empty() {
        "run".to_string()
    } else {
        token
    }
}

/// One VM's transcript: the header, a line naming the run and the VM, then the
/// capture exactly as the daemon served it (bytes and line endings untouched:
/// the export is evidence, not a rendering).
pub fn vm_transcript(run: &Run, vm: &Vm, at_ms: u64) -> String {
    let mut out = String::new();
    out.push_str(TRANSCRIPT_HEADER);
    out.push('\n');
    out.push_str(&format!(
        "# run {} · vm {} · {}\n",
        run.id,
        vm.id,
        vm_facts(vm)
    ));
    out.push_str(&format!("# exported {}\n", clock_label(at_ms)));
    push_capture(&mut out, vm);
    out
}

/// A whole run's transcript: the header, the run's own facts, then one
/// labelled section per VM.
pub fn run_transcript(run: &Run, at_ms: u64) -> String {
    let mut out = String::new();
    out.push_str(TRANSCRIPT_HEADER);
    out.push('\n');
    out.push_str(&format!(
        "# run {} · {} · {} vm(s) · {} up\n",
        run.id,
        run.state.label(),
        run.vms.len(),
        run.live_vms()
    ));
    out.push_str(&format!("# exported {}\n", clock_label(at_ms)));
    for vm in &run.vms {
        out.push('\n');
        out.push_str(&format!("# vm {} · {}\n", vm.id, vm_facts(vm)));
        push_capture(&mut out, vm);
    }
    out
}

/// A run's report: the run's facts, then every VM's transcript under the same
/// labelled section rule the run export uses, with the session its capture
/// shows summarised under the label.
///
/// The report is a document about the run rather than a concatenation of its
/// captures, so it adds two kinds of bytes the transcript export does not: the
/// header's facts (state, shape, the instants read against the run's own
/// clock, and each VM's identity), and one `session:` line per VM derived from
/// the capture alone (see [`crate::session`]). Everything between a section's
/// label and the next one is the capture exactly as served.
pub fn run_report(run: &Run, at_ms: u64) -> String {
    let reference = run.ended_at.unwrap_or(at_ms);
    let sessions: Vec<SessionRead> = run
        .vms
        .iter()
        .map(|vm| session::read(vm.serial.as_deref().unwrap_or("")))
        .collect();

    let mut out = String::new();
    out.push_str(REPORT_HEADER);
    out.push('\n');
    out.push_str(&format!(
        "# run {} · {} · {} vm(s) · {} up\n",
        run.id,
        run.state.label(),
        run.vms.len(),
        run.live_vms()
    ));
    out.push_str(&format!("# shape: {}\n", shape_facts(run)));
    out.push_str(&format!(
        "# created {} · ended {} · elapsed {}\n",
        instant_label(run.created_at, reference),
        match run.ended_at {
            Some(ended) => instant_label(ended, reference),
            None => "—".to_string(),
        },
        duration_label(reference.saturating_sub(run.created_at))
    ));
    out.push_str(&format!("# exported {}\n", clock_label(at_ms)));
    out.push_str(&format!("# sessions: {}\n", session_tally(&sessions)));
    for vm in &run.vms {
        out.push_str(&format!("# vm {} · {}\n", vm.id, vm_facts(vm)));
    }
    for (vm, session) in run.vms.iter().zip(&sessions) {
        out.push('\n');
        out.push_str(&format!("# vm {} · {}\n", vm.id, vm_facts(vm)));
        out.push_str(&format!("session: {}\n", session.summary()));
        push_capture(&mut out, vm);
    }
    out
}

/// The run's shape as the report's facts header writes it: how many vCPUs each
/// guest was given, how much memory, and whether a guest reboots itself after
/// a count of heartbeats or runs long-lived.
fn shape_facts(run: &Run) -> String {
    format!(
        "{} {} · {} MiB each · beats {}",
        run.vcpus,
        if run.vcpus == 1 { "vcpu" } else { "vcpus" },
        run.memory_mib,
        match run.beats {
            Some(beats) => beats.to_string(),
            None => "long-lived".to_string(),
        }
    )
}

/// The report header's ledger of what the captures showed: how many of the
/// run's guests completed the frozen session, how many are keepalive sessions,
/// and how many showed none. It is the total of the `session:` lines below it,
/// stated once so the report's first lines say what the document holds.
fn session_tally(sessions: &[SessionRead]) -> String {
    let count = |of: fn(&SessionRead) -> bool| sessions.iter().filter(|s| of(s)).count();
    let complete = count(|s| matches!(s, SessionRead::Complete { .. }));
    let keepalive = count(|s| matches!(s, SessionRead::Keepalive { .. }));
    let none = sessions.len() - complete - keepalive;
    format!("{complete} complete · {keepalive} keepalive · {none} none seen")
}

/// Appends a VM's capture, terminated: an export concatenates captures, and
/// one that ended mid-line would run into the next label.
fn push_capture(out: &mut String, vm: &Vm) {
    let Some(serial) = vm.serial.as_deref() else {
        return;
    };
    if serial.is_empty() {
        return;
    }
    out.push_str(serial);
    if !serial.ends_with('\n') {
        out.push('\n');
    }
}

/// A VM's facts as a transcript header writes them: the role its guest runs
/// as when it ran as one, how it is, its guest CID and, once it has left the
/// running state, how it left.
///
/// The role leads because an exported transcript is evidence read later: a
/// file of an agent's serial that does not say which agent wrote it is a
/// transcript of nobody in particular.
fn vm_facts(vm: &Vm) -> String {
    let mut facts = match vm.role.as_deref() {
        Some(role) => format!("role {role} · {}", vm.state.label()),
        None => vm.state.label().to_string(),
    };
    match vm.guest_cid {
        Some(cid) => facts.push_str(&format!(" · cid {cid}")),
        None => facts.push_str(" · no vsock"),
    }
    if vm.state != VmState::Running {
        if let Some(serial) = vm.serial.as_deref() {
            facts.push_str(&format!(" · exit {}", exit_kind(serial).label()));
        }
    }
    facts
}

/// Writes an export — a transcript or a session CSV — and answers how many
/// bytes it wrote, or the operating system's own words for why it could not.
///
/// This is the export path's only side effect, and there is exactly one of it:
/// one `std::fs::write` to the path in the box, with no directory created and
/// no file removed first.
pub fn write_export(path: &str, contents: &str) -> Result<usize, String> {
    std::fs::write(path, contents)
        .map(|()| contents.len())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::RunState;

    fn vm(id: u32, state: VmState, cid: Option<u64>, serial: Option<&str>) -> Vm {
        Vm {
            id,
            guest_cid: cid,
            role: None,
            state,
            serial: serial.map(str::to_string),
        }
    }

    fn run() -> Run {
        Run {
            id: "r1".to_string(),
            state: RunState::Done,
            vcpus: 2,
            memory_mib: 512,
            beats: None,
            created_at: 1_760_000_000_000,
            ended_at: Some(1_760_000_060_000),
            vms: vec![
                vm(
                    0,
                    VmState::Exited,
                    Some(40),
                    Some("$ poweroff\r\n[   12.4] reboot: Power down\r\n"),
                ),
                vm(1, VmState::Killed, Some(41), Some("$ reboot\nmachine restart")),
            ],
        }
    }

    #[test]
    fn a_derived_path_names_the_run_and_the_vm_in_the_directory_it_was_given() {
        let dir = Path::new("/tmp/plaza-test");
        assert_eq!(
            export_path(dir, "r1", 0, ExportTarget::Vm),
            "/tmp/plaza-test/plaza-r1-0.txt"
        );
        assert_eq!(
            export_path(dir, "r7", 13, ExportTarget::Vm),
            "/tmp/plaza-test/plaza-r7-13.txt"
        );
        assert_eq!(
            export_path(dir, "r1", 0, ExportTarget::Run),
            "/tmp/plaza-test/plaza-r1-all.txt"
        );
    }

    #[test]
    fn a_run_id_cannot_steer_the_write_out_of_the_directory() {
        let dir = Path::new("/tmp/plaza-test");
        for id in ["../../etc/passwd", "a/b", "r1\n", "…", " ", "/"] {
            let path = export_path(dir, id, 0, ExportTarget::Vm);
            let name = Path::new(&path)
                .file_name()
                .expect("a file name")
                .to_string_lossy()
                .into_owned();
            assert!(!name.contains('/'), "{id:?} -> {name:?}");
            assert!(path.starts_with("/tmp/plaza-test/"), "{id:?} -> {path}");
            assert_eq!(Path::new(&path).parent().expect("a parent"), dir, "{id:?}");
        }
        assert_eq!(file_token("../../etc/passwd"), "______etc_passwd");
        assert_eq!(file_token(""), "run");
        assert_eq!(file_token("r1"), "r1");
    }

    #[test]
    fn a_vm_transcript_is_a_header_a_fact_line_and_the_capture_byte_for_byte() {
        let run = run();
        let transcript = vm_transcript(&run, &run.vms[1], 1_760_000_000_000);
        assert_eq!(
            transcript,
            "# AgentPlaza transcript\n\
             # run r1 · vm 1 · killed · cid 41 · exit reset\n\
             # exported 08:53:20 UTC\n\
             $ reboot\nmachine restart\n"
        );
        // The capture's own line endings are not rewritten: the CRLF capture
        // is kept as served.
        let crlf = vm_transcript(&run, &run.vms[0], 0);
        assert!(crlf.ends_with("[   12.4] reboot: Power down\r\n"), "{crlf:?}");
        assert!(crlf.contains("· exited · cid 40 · exit slept\n"), "{crlf:?}");
    }

    #[test]
    fn a_vm_that_never_wrote_a_line_exports_its_header_and_nothing_else() {
        let mut run = run();
        run.vms[0].serial = None;
        let transcript = vm_transcript(&run, &run.vms[0], 0);
        assert_eq!(
            transcript,
            "# AgentPlaza transcript\n\
             # run r1 · vm 0 · exited · cid 40\n\
             # exported 00:00:00 UTC\n"
        );
    }

    #[test]
    fn a_role_agents_transcript_names_its_role_on_the_facts_line() {
        let mut run = run();
        run.vms[0].role = Some("builder".to_string());
        let one = vm_transcript(&run, &run.vms[0], 0);
        assert!(
            one.contains("# run r1 · vm 0 · role builder · exited · cid 40 · exit slept\n"),
            "{one:?}"
        );
        // A VM with no role gets no role on its line: nothing is invented.
        assert!(
            !one.contains("# vm 1 · role"),
            "the run's other VM carries no role: {one:?}"
        );
        let two = vm_transcript(&run, &run.vms[1], 0);
        assert!(two.contains("# run r1 · vm 1 · killed · cid 41 · exit reset\n"), "{two:?}");

        // The run export labels each of its sections the same way.
        let all = run_transcript(&run, 0);
        assert!(all.contains("# vm 0 · role builder · exited · cid 40 · exit slept\n"), "{all:?}");
        assert!(all.contains("# vm 1 · killed · cid 41 · exit reset\n"), "{all:?}");
    }

    #[test]
    fn a_run_transcript_labels_every_vm_in_order() {
        let run = run();
        let transcript = run_transcript(&run, 1_760_000_000_000);
        assert_eq!(
            transcript,
            "# AgentPlaza transcript\n\
             # run r1 · done · 2 vm(s) · 0 up\n\
             # exported 08:53:20 UTC\n\
             \n\
             # vm 0 · exited · cid 40 · exit slept\n\
             $ poweroff\r\n[   12.4] reboot: Power down\r\n\
             \n\
             # vm 1 · killed · cid 41 · exit reset\n\
             $ reboot\nmachine restart\n"
        );
        // An export is a file: its last line ends.
        assert!(transcript.ends_with('\n'));
    }

    #[test]
    fn a_capture_that_ends_mid_line_does_not_run_into_the_next_label() {
        let mut run = run();
        run.vms[0].serial = Some("$ poweroff".to_string());
        let transcript = run_transcript(&run, 0);
        assert!(
            transcript.contains("$ poweroff\n\n# vm 1 · killed"),
            "the label would have been glued to the capture: {transcript:?}"
        );
    }

    #[test]
    fn a_run_without_a_vsock_device_says_so_on_its_facts_line() {
        let mut run = run();
        run.vms = vec![vm(0, VmState::Running, None, Some("login: plaza\n"))];
        run.state = RunState::Running;
        run.ended_at = None;
        let transcript = vm_transcript(&run, &run.vms[0], 0);
        assert!(transcript.contains("vm 0 · running · no vsock\n"), "{transcript:?}");
        // A VM that is still up has no exit to read.
        assert!(!transcript.contains("exit "), "{transcript:?}");
    }

    #[test]
    fn the_box_derives_until_the_operator_types_and_then_keeps_their_path() {
        let dir = Path::new("/tmp/plaza-test");
        let mut export = Export::default();
        export.derive("r1", 2, dir);
        assert_eq!(export.path, "/tmp/plaza-test/plaza-r1-2.txt");

        // Another target, another default.
        export.target = ExportTarget::Run;
        export.derive("r1", 2, dir);
        assert_eq!(export.path, "/tmp/plaza-test/plaza-r1-all.txt");

        // The operator's own path is never replaced by the derivation.
        export.path = "  ./mine.txt  ".to_string();
        export.edited = true;
        export.derive("r9", 0, dir);
        assert_eq!(export.to_write(), "./mine.txt");
    }

    #[test]
    fn a_transcript_is_written_and_the_bytes_come_back_off_the_disk() {
        let path = std::env::temp_dir().join(format!(
            "plaza-export-unit-{}-{:?}.txt",
            std::process::id(),
            std::thread::current().id()
        ));
        let path = path.to_string_lossy().into_owned();
        let run = run();
        let contents = vm_transcript(&run, &run.vms[1], 0);

        let written = write_export(&path, &contents).expect("the write lands");
        assert_eq!(written, contents.len());
        let read_back = std::fs::read_to_string(&path).expect("the file is there");
        assert_eq!(read_back, contents);
        std::fs::remove_file(&path).expect("the probe file is removed");
    }

    #[test]
    fn a_write_that_cannot_land_answers_with_the_systems_own_words() {
        let path = format!(
            "/nonexistent-plaza-dir-{}/plaza-r1-0.txt",
            std::process::id()
        );
        let error = write_export(&path, "hello").expect_err("the directory cannot exist");
        assert!(!error.is_empty(), "the system's words are carried, not swallowed");
        assert!(!Path::new(&path).exists(), "nothing was written");
    }

    #[test]
    fn the_sessions_path_is_stamped_in_the_directory_it_was_given() {
        let dir = Path::new("/tmp/plaza-test");
        assert_eq!(
            telemetry_path(dir, 1_760_000_500_000),
            "/tmp/plaza-test/plaza-telemetry-1760000500000.csv"
        );
        // Two sessions in the same directory do not collide, and the names
        // sort by the time each began.
        assert!(
            telemetry_path(dir, 1_760_000_500_000) < telemetry_path(dir, 1_760_000_502_000)
        );
    }

    #[test]
    fn the_sessions_box_is_derived_until_the_operator_types_a_path() {
        let dir = Path::new("/tmp/plaza-test");
        let mut csv = CsvExport::default();
        csv.derive(dir, 1_760_000_500_000);
        assert_eq!(csv.path, "/tmp/plaza-test/plaza-telemetry-1760000500000.csv");
        assert!(!csv.edited);

        // A later stamp never overwrites what is already in the box.
        csv.derive(dir, 1_760_000_999_000);
        assert_eq!(csv.path, "/tmp/plaza-test/plaza-telemetry-1760000500000.csv");

        // A path the operator typed, whitespace and all, is what is written.
        csv.path = "  /tmp/mine.csv  ".to_string();
        csv.edited = true;
        csv.derive(dir, 1_760_000_999_000);
        assert_eq!(csv.to_write(), "/tmp/mine.csv");

        // Emptying the box hands it back to the derived default.
        csv.path = "   ".to_string();
        csv.edited = false;
        csv.derive(dir, 1_760_000_500_000);
        assert_eq!(csv.path, "/tmp/plaza-test/plaza-telemetry-1760000500000.csv");
    }

    /// A capture as the console really carries it: the guest init's own lines
    /// and the harness's transcript around a frozen session that completed.
    const SESSION: &str = "plaza guest: plaza.harness=1: loading the vsock modules\r\n\
                           [    0.417190] virtio_vsock: vsock ready\r\n\
                           [vsock->] {\"type\":\"hello\",\"vm\":0,\"guest_cid\":40}\r\n\
                           [vsock<-] {\"type\":\"welcome\",\"echo\":\"plaza-orchestrator\"}\r\n\
                           agent-harness: handshake complete (host echo \"plaza-orchestrator\")\r\n\
                           [vsock->] {\"type\":\"tool_call\",\"call\":1,\"name\":\"echo\",\"args\":{\"text\":\"plaza-guest-40-hello\"}}\r\n\
                           [vsock<-] {\"type\":\"tool_result\",\"call\":1,\"ok\":true,\"output\":\"plaza-guest-40-hello\"}\r\n\
                           [vsock->] {\"type\":\"tool_call\",\"call\":2,\"name\":\"uptime\",\"args\":{}}\r\n\
                           [vsock<-] {\"type\":\"tool_result\",\"call\":2,\"ok\":true,\"output\":\"12.34 45.67\"}\r\n\
                           [vsock->] {\"type\":\"bye\",\"calls\":2}\r\n\
                           agent-harness: session complete after 2 tool calls\r\n";

    /// A keepalive guest's capture: it has beaten once and served one tool
    /// call, and the hub has not stopped it, so there is no `bye`.
    const KEEPALIVE: &str = "[vsock->] {\"type\":\"hello\",\"vm\":1,\"guest_cid\":41,\"keepalive\":true}\r\n\
                             [vsock<-] {\"type\":\"welcome\",\"echo\":\"plaza-orchestrator\",\"keepalive\":true}\r\n\
                             [vsock->] {\"type\":\"heartbeat\",\"seq\":1}\r\n\
                             [vsock<-] {\"type\":\"tool_call\",\"call\":1,\"name\":\"uptime\",\"args\":{}}\r\n\
                             [vsock->] {\"type\":\"tool_result\",\"call\":1,\"ok\":true,\"output\":\"1.0 2.0\"}\r\n";

    /// A live run of three guests: one completed the frozen session, one is a
    /// keepalive agent, and one has written nothing at all.
    fn report_run() -> Run {
        Run {
            id: "r1".to_string(),
            state: RunState::Running,
            vcpus: 1,
            memory_mib: 256,
            beats: Some(3),
            created_at: 1_760_000_000_000,
            ended_at: None,
            vms: vec![
                Vm {
                    id: 0,
                    guest_cid: Some(40),
                    role: Some("builder".to_string()),
                    state: VmState::Running,
                    serial: Some(SESSION.to_string()),
                },
                Vm {
                    id: 1,
                    guest_cid: Some(41),
                    role: None,
                    state: VmState::Running,
                    serial: Some(KEEPALIVE.to_string()),
                },
                Vm {
                    id: 2,
                    guest_cid: None,
                    role: None,
                    state: VmState::Running,
                    serial: None,
                },
            ],
        }
    }

    #[test]
    fn a_reports_path_names_the_run_and_cannot_steer_the_write_out_of_the_directory() {
        let dir = Path::new("/tmp/plaza-test");
        assert_eq!(
            export_path(dir, "r1", 0, ExportTarget::Report),
            "/tmp/plaza-test/plaza-r1-report.txt"
        );
        // A report is not one VM's transcript, so no VM index reaches its name.
        assert_ne!(
            export_path(dir, "r1", 3, ExportTarget::Report),
            export_path(dir, "r1", 3, ExportTarget::Vm)
        );
        // The run id becomes the same filename token as everywhere else.
        assert_eq!(
            export_path(dir, "../../etc/passwd", 0, ExportTarget::Report),
            "/tmp/plaza-test/plaza-______etc_passwd-report.txt"
        );
    }

    #[test]
    fn a_run_report_carries_its_facts_every_transcript_and_each_derived_session() {
        let report = run_report(&report_run(), 1_760_000_500_000);
        // The captures are carried byte for byte, under the label rule.
        let expected = format!(
            "# AgentPlaza report\n\
             # run r1 · running · 3 vm(s) · 3 up\n\
             # shape: 1 vcpu · 256 MiB each · beats 3\n\
             # created 08:53:20 UTC · ended — · elapsed 8m 20s\n\
             # exported 09:01:40 UTC\n\
             # sessions: 1 complete · 1 keepalive · 1 none seen\n\
             # vm 0 · role builder · running · cid 40\n\
             # vm 1 · running · cid 41\n\
             # vm 2 · running · no vsock\n\
             \n\
             # vm 0 · role builder · running · cid 40\n\
             session: hello→welcome→echo✓→uptime✓→bye, 2 tool calls\n\
             {SESSION}\
             \n\
             # vm 1 · running · cid 41\n\
             session: keepalive, 1 heartbeat, 1 tool call, no bye seen\n\
             {KEEPALIVE}\
             \n\
             # vm 2 · running · no vsock\n\
             session: none seen\n"
        );
        assert_eq!(report, expected);
        // A report is a file: its last line ends.
        assert!(report.ends_with('\n'));
    }

    #[test]
    fn a_reports_instants_are_read_against_the_runs_own_clock() {
        // A run that ended three days and seven hours after it was created:
        // the creation reads as its day rather than as today's clock, and the
        // elapsed time is in the day tier.
        let mut run = report_run();
        run.state = RunState::Done;
        run.ended_at = Some(1_760_000_060_000);
        let created = run.ended_at.expect("just set") - 3 * crate::model::DAY_MS - 7 * 3_600_000;
        run.created_at = created;
        let report = run_report(&run, 1_760_000_500_000);
        assert!(
            report.contains("# created 2025-10-06 · ended 08:54:20 UTC · elapsed 3d 07h\n"),
            "{report:?}"
        );
        // A run with no VMs and no end is still a document: the facts, and no
        // section at all.
        let mut bare = report_run();
        bare.vms.clear();
        let bare = run_report(&bare, 1_760_000_500_000);
        assert!(bare.ends_with("# sessions: 0 complete · 0 keepalive · 0 none seen\n"), "{bare:?}");
        assert!(!bare.contains("# vm "), "{bare:?}");
    }

    #[test]
    fn a_reports_bytes_are_written_and_read_back_with_the_same_one_write() {
        let path = std::env::temp_dir().join(format!(
            "plaza-report-unit-{}-{:?}.txt",
            std::process::id(),
            std::thread::current().id()
        ));
        let path = path.to_string_lossy().into_owned();
        let contents = run_report(&report_run(), 1_760_000_500_000);
        let written = write_export(&path, &contents).expect("the write lands");
        assert_eq!(written, contents.len());
        assert_eq!(std::fs::read_to_string(&path).expect("the file is there"), contents);
        std::fs::remove_file(&path).expect("the probe file is removed");
    }
}
