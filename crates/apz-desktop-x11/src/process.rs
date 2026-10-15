//! The processes the backend started, found again by pid and start time. A pid is reused once its process ends,
//! so a launch names its process by both: the start time is field 22 of `/proc/<pid>/stat`, in clock ticks since
//! boot, and it does not repeat for a live process. Ending a launched app goes only to its launched process and
//! the processes beneath it, never to the display's server or window manager, or to anything else on the machine.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::thread;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(20);

/// A process as `/proc/<pid>/stat` describes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stat {
    state: char,
    parent: u32,
    started: u64,
}

/// A process the backend started: its pid, and its start time when it was started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Started {
    pub pid: u32,
    pub started: u64,
}

/// Parses one `/proc/<pid>/stat` line. The command name is in parentheses and may hold spaces and parentheses, so
/// the fields are read after the last `)`: the state is field 3, the parent field 4 and the start time field 22.
pub(crate) fn parse_stat(text: &str) -> Option<Stat> {
    let rest = &text[text.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // Field 3 is the first field after the command, so field N is at index N - 3.
    Some(Stat {
        state: fields.first()?.chars().next()?,
        parent: fields.get(1)?.parse().ok()?,
        started: fields.get(19)?.parse().ok()?,
    })
}

fn stat_of(pid: u32) -> Option<Stat> {
    parse_stat(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// The start time of a process, or None once it is gone.
pub(crate) fn started_at(pid: u32) -> Option<u64> {
    stat_of(pid).map(|stat| stat.started)
}

/// Whether a process is still running: it exists and is neither a zombie nor dead.
fn is_running(pid: u32) -> bool {
    stat_of(pid).is_some_and(|stat| stat.state != 'Z' && stat.state != 'X')
}

/// Every process on the machine, by pid. A process that ends while the table is read is left out.
pub(crate) fn table() -> HashMap<u32, Stat> {
    let mut table = HashMap::new();
    let Ok(read) = fs::read_dir("/proc") else {
        return table;
    };
    for entry in read.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if let Some(stat) = stat_of(pid) {
            table.insert(pid, stat);
        }
    }
    table
}

/// Whether a launch's process is still the one it was: the same pid, with the same start time.
pub(crate) fn is_still(started: Started, table: &HashMap<u32, Stat>) -> bool {
    table
        .get(&started.pid)
        .is_some_and(|stat| stat.started == started.started)
}

/// The processes of `roots` that are still the ones they were, and every process beneath them.
pub(crate) fn tree(roots: &[Started], table: &HashMap<u32, Stat>) -> BTreeSet<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&pid, stat) in table {
        children.entry(stat.parent).or_default().push(pid);
    }
    let mut out = BTreeSet::new();
    let mut queue: Vec<u32> = roots
        .iter()
        .filter(|root| is_still(**root, table))
        .map(|root| root.pid)
        .collect();
    while let Some(pid) = queue.pop() {
        if out.insert(pid) {
            queue.extend(children.get(&pid).into_iter().flatten().copied());
        }
    }
    out
}

/// Ends the processes: SIGTERM to each, then SIGKILL to those still running after `grace`. Only the pids given are
/// signalled. Pid 0 and 1, and this process, are never among them.
pub(crate) fn terminate(pids: &BTreeSet<u32>, grace: Duration) {
    let own = std::process::id();
    let targets: Vec<u32> = pids
        .iter()
        .copied()
        .filter(|&pid| pid > 1 && pid != own)
        .collect();
    for &pid in &targets {
        signal(pid, libc::SIGTERM);
    }
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline && targets.iter().any(|&pid| is_running(pid)) {
        thread::sleep(POLL);
    }
    for &pid in &targets {
        if is_running(pid) {
            signal(pid, libc::SIGKILL);
        }
    }
}

fn signal(pid: u32, sig: libc::c_int) {
    if let Ok(pid) = libc::pid_t::try_from(pid) {
        // SAFETY: kill with a positive pid signals that one process. ESRCH (the process is already gone) is
        // expected and ignored.
        unsafe { libc::kill(pid, sig) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /// A `/proc/<pid>/stat` line with the given state, parent and start time, the other fields zero.
    fn stat_line(pid: u32, state: &str, parent: u32, started: u64) -> String {
        let mut fields = vec!["0".to_string(); 52];
        fields[0] = state.to_string();
        fields[1] = parent.to_string();
        fields[19] = started.to_string();
        format!("{pid} (apz app) {}", fields.join(" "))
    }

    #[test]
    fn the_stat_fields_are_read_after_a_command_with_spaces_and_brackets() {
        let stat = parse_stat(&stat_line(7, "S", 42, 999)).expect("parses");
        assert_eq!(
            stat,
            Stat {
                state: 'S',
                parent: 42,
                started: 999
            }
        );
        assert!(parse_stat("7 (short) S").is_none(), "too few fields");
    }

    fn entry(pid: u32, parent: u32, started: u64) -> (u32, Stat) {
        (
            pid,
            Stat {
                state: 'S',
                parent,
                started,
            },
        )
    }

    #[test]
    fn the_tree_is_the_live_launched_process_and_what_runs_beneath_it() {
        // The tree is the launched process and its descendants, not unrelated processes.
        {
            let table: HashMap<u32, Stat> = [
                entry(100, 1, 5),   // the launched app
                entry(101, 100, 6), // its child
                entry(102, 101, 7), // a grandchild
                entry(200, 1, 8),   // something else on the machine
                entry(300, 1, 9),   // the display's server, started by the daemon
            ]
            .into_iter()
            .collect();
            let launch = Started {
                pid: 100,
                started: 5,
            };
            assert_eq!(
                tree(&[launch], &table),
                [100, 101, 102].into_iter().collect()
            );
        }
        // A pid that was reused by another process (different start time) is not the launched
        // process, so it is not live and nothing beneath it is ours.
        {
            let table: HashMap<u32, Stat> =
                [entry(100, 1, 6), entry(101, 100, 7)].into_iter().collect();
            let stale = Started {
                pid: 100,
                started: 5,
            };
            assert!(!is_still(stale, &table), "the start time differs");
            assert!(
                tree(&[stale], &table).is_empty(),
                "nothing beneath a reused pid is ours"
            );
        }
    }

    #[test]
    fn terminate_ends_only_the_pids_it_is_given() {
        let mut ours = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sleep starts");
        let mut kept = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sleep starts");
        let only_ours: BTreeSet<u32> = [ours.id()].into_iter().collect();
        terminate(&only_ours, Duration::from_millis(200));
        let status = ours.wait().expect("the process is reaped");
        assert!(
            !status.success(),
            "the terminated process did not exit cleanly"
        );
        assert!(
            matches!(kept.try_wait(), Ok(None)),
            "the process not given is still running"
        );
        let _ = kept.kill();
        let _ = kept.wait();
    }
}
