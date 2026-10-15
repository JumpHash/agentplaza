//! `filesystem.Filesystem/*` (E2B's `spec/envd/filesystem/filesystem.proto`): stat, list, make a
//! directory, move, remove, and watch a directory.
//!
//! A relative path, and `~`, resolve against the sandbox's home; nothing else is confined. Watchers
//! poll: a stat snapshot every [`POLL`], diffed against the previous one. That walks the tree on
//! each tick, but it needs no inotify binding and yields the events the SDKs surface (create, write,
//! remove, chmod). A rename shows as a remove and a create.

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use super::{Ctx, RpcError, StreamSink};

/// How often a watcher, and a `WatchDir` stream, takes a fresh snapshot.
pub(super) const POLL: Duration = Duration::from_millis(200);
/// How long a `WatchDir` stream stays silent before a keepalive.
const KEEPALIVE: Duration = Duration::from_secs(50);
/// Events a watcher keeps between drains; the oldest go first beyond this.
const MAX_QUEUED_EVENTS: usize = 10_000;

const CREATE: &str = "EVENT_TYPE_CREATE";
const WRITE: &str = "EVENT_TYPE_WRITE";
const REMOVE: &str = "EVENT_TYPE_REMOVE";
const CHMOD: &str = "EVENT_TYPE_CHMOD";

/// One change a watcher saw: the path relative to the watched directory, and the event type.
type Change = (String, &'static str);

/// A poll-based watcher: the changes not yet drained, and the flag that stops its thread.
struct Watcher {
    events: Arc<Mutex<VecDeque<Change>>>,
    stop: Arc<AtomicBool>,
}

static WATCHERS: Mutex<BTreeMap<String, Watcher>> = Mutex::new(BTreeMap::new());
static NEXT_WATCHER: AtomicU64 = AtomicU64::new(1);

/// A unary method (`Stat`, `MakeDir`, `Move`, `ListDir`, `Remove`, `CreateWatcher`,
/// `GetWatcherEvents`, `RemoveWatcher`).
pub fn unary(method: &str, request: &Value, ctx: &Ctx) -> Result<Value, RpcError> {
    match method {
        "Stat" => {
            let path = path_arg(request, "path", ctx)?;
            Ok(json!({"entry": entry_info(&path, &Names::load())?}))
        }
        "ListDir" => {
            let path = path_arg(request, "path", ctx)?;
            // The default depth is 1: the directory's own entries.
            let depth = match int_field(request, "depth")? {
                0 => 1,
                depth => depth,
            };
            let mut entries = Vec::new();
            list(&path, depth, &Names::load(), &mut entries)?;
            Ok(json!({"entries": entries}))
        }
        "MakeDir" => {
            let path = path_arg(request, "path", ctx)?;
            if fs::symlink_metadata(&path).is_ok() {
                return Err(RpcError::new(
                    "already_exists",
                    format!("mkdir {}: already exists", path.display()),
                ));
            }
            create_dirs(&path, ctx.uid, ctx.gid)?;
            Ok(json!({"entry": entry_info(&path, &Names::load())?}))
        }
        "Move" => {
            let source = path_arg(request, "source", ctx)?;
            let destination = path_arg(request, "destination", ctx)?;
            fs::rename(&source, &destination).map_err(|err| RpcError::io(&err, "move"))?;
            Ok(json!({"entry": entry_info(&destination, &Names::load())?}))
        }
        "Remove" => {
            let path = path_arg(request, "path", ctx)?;
            // A symlink is removed as a link, never followed into its target.
            let meta = fs::symlink_metadata(&path).map_err(|err| RpcError::io(&err, "remove"))?;
            let removed = if meta.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            };
            removed.map_err(|err| RpcError::io(&err, "remove"))?;
            Ok(json!({}))
        }
        "CreateWatcher" => {
            let path = path_arg(request, "path", ctx)?;
            let recursive = bool_field(request, "recursive");
            let meta = fs::metadata(&path).map_err(|err| RpcError::io(&err, "watch"))?;
            if !meta.is_dir() {
                return Err(RpcError::new(
                    "invalid_argument",
                    format!("watch {}: not a directory", path.display()),
                ));
            }
            let id = format!("w{}", NEXT_WATCHER.fetch_add(1, Ordering::Relaxed));
            let events = Arc::new(Mutex::new(VecDeque::new()));
            let stop = Arc::new(AtomicBool::new(false));
            // The baseline is taken before the call returns, so a change made right after it is seen.
            let baseline = snapshot(&path, recursive);
            spawn_poller(
                path,
                recursive,
                baseline,
                Arc::clone(&events),
                Arc::clone(&stop),
            );
            lock(&WATCHERS).insert(id.clone(), Watcher { events, stop });
            Ok(json!({"watcherId": id}))
        }
        "GetWatcherEvents" => {
            let id = string_field(request, "watcherId");
            let events = match lock(&WATCHERS).get(&id) {
                Some(watcher) => Arc::clone(&watcher.events),
                None => return Err(no_watcher(&id)),
            };
            let drained: Vec<Value> = lock(&events)
                .drain(..)
                .map(|(name, kind)| json!({"name": name, "type": kind}))
                .collect();
            Ok(json!({"events": drained}))
        }
        "RemoveWatcher" => {
            let id = string_field(request, "watcherId");
            match lock(&WATCHERS).remove(&id) {
                Some(watcher) => {
                    watcher.stop.store(true, Ordering::Relaxed);
                    Ok(json!({}))
                }
                None => Err(no_watcher(&id)),
            }
        }
        _ => Err(RpcError::new(
            "unimplemented",
            format!("filesystem.Filesystem/{method}"),
        )),
    }
}

/// A server-streaming method (`WatchDir`): `start`, then the changes as they happen, a keepalive
/// after a quiet [`KEEPALIVE`], until a write to the sink fails (the client went away).
pub fn stream(
    method: &str,
    request: &Value,
    ctx: &Ctx,
    sink: &mut dyn StreamSink,
) -> Result<(), RpcError> {
    if method != "WatchDir" {
        return Err(RpcError::new(
            "unimplemented",
            format!("filesystem.Filesystem/{method}"),
        ));
    }
    let path = path_arg(request, "path", ctx)?;
    let recursive = bool_field(request, "recursive");
    let meta = fs::metadata(&path).map_err(|err| RpcError::io(&err, "watch"))?;
    if !meta.is_dir() {
        return Err(RpcError::new(
            "invalid_argument",
            format!("watch {}: not a directory", path.display()),
        ));
    }
    let mut last_sent = Instant::now();
    if sink.message(&json!({"start": {}})).is_err() {
        return Ok(());
    }
    let mut previous = snapshot(&path, recursive);
    loop {
        thread::sleep(POLL);
        let current = snapshot(&path, recursive);
        let changes = diff(&path, &previous, &current);
        previous = current;
        for (name, kind) in changes {
            let message = json!({"filesystem": {"name": name, "type": kind}});
            if sink.message(&message).is_err() {
                return Ok(());
            }
            last_sent = Instant::now();
        }
        if last_sent.elapsed() >= KEEPALIVE {
            if sink.message(&json!({"keepalive": {}})).is_err() {
                return Ok(());
            }
            last_sent = Instant::now();
        }
    }
}

/// A user from `/etc/passwd`: the ids that own what it writes, and the home that resolves its
/// relative paths.
pub(super) struct User {
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
}

/// The guest's user called `name`, if `/etc/passwd` has one.
pub(super) fn lookup_user(name: &str) -> Option<User> {
    let passwd = fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() < 7 || fields[0] != name {
            return None;
        }
        Some(User {
            uid: fields[2].parse().ok()?,
            gid: fields[3].parse().ok()?,
            home: PathBuf::from(fields[5]),
        })
    })
}

/// Resolves a request path: `~` and a relative path go under `home`, an absolute one stays.
pub(super) fn resolve(raw: &str, home: &Path) -> PathBuf {
    if raw == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home.join(rest);
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        home.join(path)
    }
}

/// Creates `dir` and its missing parents, each one created owned by `uid:gid`, so a file the
/// sandbox's user uploads into a new directory leaves the directory theirs too.
pub(super) fn create_dirs(dir: &Path, uid: u32, gid: u32) -> Result<(), RpcError> {
    let mut missing = Vec::new();
    let mut cursor = Some(dir);
    while let Some(path) = cursor {
        if path.as_os_str().is_empty() || fs::symlink_metadata(path).is_ok() {
            break;
        }
        missing.push(path);
        cursor = path.parent();
    }
    for path in missing.into_iter().rev() {
        match fs::create_dir(path) {
            Ok(()) => std::os::unix::fs::lchown(path, Some(uid), Some(gid))
                .map_err(|err| RpcError::io(&err, "chown"))?,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(RpcError::io(&err, "mkdir")),
        }
    }
    Ok(())
}

/// The path a request names, resolved against the sandbox's home; an empty one is an error.
fn path_arg(request: &Value, key: &str, ctx: &Ctx) -> Result<PathBuf, RpcError> {
    let raw = string_field(request, key);
    if raw.is_empty() {
        return Err(RpcError::new(
            "invalid_argument",
            format!("{key} is required"),
        ));
    }
    Ok(resolve(&raw, &ctx.home))
}

fn string_field(request: &Value, key: &str) -> String {
    request
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn bool_field(request: &Value, key: &str) -> bool {
    request.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// A 64-bit integer field: protobuf's JSON writes it as a string, but a number is accepted too.
fn int_field(request: &Value, key: &str) -> Result<u64, RpcError> {
    let invalid = || RpcError::new("invalid_argument", format!("{key} must be an integer"));
    match request.get(key) {
        None | Some(Value::Null) => Ok(0),
        Some(Value::Number(number)) => number.as_u64().ok_or_else(invalid),
        Some(Value::String(text)) => text.parse().map_err(|_| invalid()),
        Some(_) => Err(invalid()),
    }
}

fn no_watcher(id: &str) -> RpcError {
    RpcError::new("not_found", format!("watcher {id} not found"))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A poisoned lock still holds consistent data here (a queue or a map), so keep using it.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The guest's uid and gid names, read per request so a user added since start is seen.
struct Names {
    users: BTreeMap<u32, String>,
    groups: BTreeMap<u32, String>,
}

impl Names {
    fn load() -> Self {
        Self {
            users: id_names("/etc/passwd"),
            groups: id_names("/etc/group"),
        }
    }

    fn user(&self, uid: u32) -> String {
        self.users
            .get(&uid)
            .cloned()
            .unwrap_or_else(|| uid.to_string())
    }

    fn group(&self, gid: u32) -> String {
        self.groups
            .get(&gid)
            .cloned()
            .unwrap_or_else(|| gid.to_string())
    }
}

/// `name:password:id:…` lines (both `passwd` and `group` have the id third); the first entry for
/// an id wins.
fn id_names(path: &str) -> BTreeMap<u32, String> {
    let mut names = BTreeMap::new();
    for line in fs::read_to_string(path).unwrap_or_default().lines() {
        let fields: Vec<&str> = line.split(':').collect();
        if let (Some(name), Some(Ok(id))) =
            (fields.first(), fields.get(2).map(|f| f.parse::<u32>()))
        {
            names.entry(id).or_insert_with(|| name.to_string());
        }
    }
    names
}

/// The `EntryInfo` of one path. A symlink reports its target's type and size, as E2B does, and
/// names the target; a dangling link reports itself.
fn entry_info(path: &Path, names: &Names) -> Result<Value, RpcError> {
    let link = fs::symlink_metadata(path).map_err(|err| RpcError::io(&err, "stat"))?;
    let is_symlink = link.file_type().is_symlink();
    let followed = if is_symlink {
        fs::metadata(path).ok()
    } else {
        None
    };
    let target = followed.as_ref().unwrap_or(&link);
    let file_type = if is_symlink && followed.is_none() {
        "FILE_TYPE_SYMLINK"
    } else if target.is_dir() {
        "FILE_TYPE_DIRECTORY"
    } else if target.is_file() {
        "FILE_TYPE_FILE"
    } else {
        "FILE_TYPE_UNSPECIFIED"
    };
    let kind = if is_symlink {
        'l'
    } else if target.is_dir() {
        'd'
    } else {
        '-'
    };
    let mode = target.mode() & 0o7777;
    let mut entry = json!({
        "name": path.file_name().map_or_else(|| path.to_string_lossy().into_owned(), |name| name.to_string_lossy().into_owned()),
        "type": file_type,
        "path": path.to_string_lossy(),
        "size": target.len().to_string(),
        "mode": mode,
        "permissions": permissions(kind, mode),
        "owner": names.user(target.uid()),
        "group": names.group(target.gid()),
        "modifiedTime": rfc3339(target.modified().unwrap_or(UNIX_EPOCH)),
        "isSymlink": is_symlink,
    });
    if is_symlink {
        let destination = fs::read_link(path)
            .map(|target| target.to_string_lossy().into_owned())
            .unwrap_or_default();
        entry["symlinkTarget"] = json!(destination);
    }
    Ok(entry)
}

/// `ls -l` style: the type letter, then the nine permission bits.
fn permissions(kind: char, mode: u32) -> String {
    const BITS: [(u32, char); 9] = [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o40, 'r'),
        (0o20, 'w'),
        (0o10, 'x'),
        (0o4, 'r'),
        (0o2, 'w'),
        (0o1, 'x'),
    ];
    let mut out = String::from(kind);
    for (bit, letter) in BITS {
        out.push(if mode & bit != 0 { letter } else { '-' });
    }
    out
}

/// An RFC 3339 UTC timestamp with nanoseconds, the form protobuf's JSON uses. Times before the
/// epoch read as the epoch.
fn rfc3339(time: SystemTime) -> String {
    let (secs, nanos) = match time.duration_since(UNIX_EPOCH) {
        Ok(since) => (
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
            since.subsec_nanos(),
        ),
        Err(_) => (0, 0),
    };
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    // Howard Hinnant's civil-from-days: the proleptic Gregorian date of a count of days since 1970.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{nanos:09}Z",
        secs_of_day / 3_600,
        secs_of_day % 3_600 / 60,
        secs_of_day % 60,
    )
}

/// The entries under `dir`, sorted by path, down to `depth` levels (1 is `dir`'s own entries).
/// Symlinks are listed but not followed.
fn list(dir: &Path, depth: u64, names: &Names, out: &mut Vec<Value>) -> Result<(), RpcError> {
    let mut children: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|err| RpcError::io(&err, "list"))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    children.sort();
    for child in children {
        // An entry that vanished between the read and the stat is skipped.
        let Ok(info) = entry_info(&child, names) else {
            continue;
        };
        let is_dir = fs::symlink_metadata(&child).is_ok_and(|meta| meta.is_dir());
        out.push(info);
        // A subdirectory that cannot be read does not hide its siblings.
        if depth > 1 && is_dir {
            let _ = list(&child, depth - 1, names, out);
        }
    }
    Ok(())
}

/// What a poll saw of one path: enough to tell a create, a write, and a chmod apart.
#[derive(Clone, PartialEq)]
struct Stamp {
    dir: bool,
    len: u64,
    mtime: SystemTime,
    mode: u32,
}

/// The paths under `root` (its direct entries, or all of them when `recursive`), without following
/// symlinks. An unreadable directory contributes nothing.
fn snapshot(root: &Path, recursive: bool) -> BTreeMap<PathBuf, Stamp> {
    let mut out = BTreeMap::new();
    collect(root, recursive, &mut out);
    out
}

fn collect(dir: &Path, recursive: bool, out: &mut BTreeMap<PathBuf, Stamp>) {
    let Ok(read) = fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        let is_dir = meta.is_dir();
        out.insert(
            path.clone(),
            Stamp {
                dir: is_dir,
                len: meta.len(),
                mtime: meta.modified().unwrap_or(UNIX_EPOCH),
                mode: meta.mode() & 0o7777,
            },
        );
        if recursive && is_dir {
            collect(&path, true, out);
        }
    }
}

/// The changes from `before` to `after`, in path order: a path new in `after` is a create, one gone
/// is a remove, and a surviving path whose mode changed is a chmod, whose size or mtime changed a
/// write (a directory's mtime only means its entries changed, which their own events report).
fn diff(
    root: &Path,
    before: &BTreeMap<PathBuf, Stamp>,
    after: &BTreeMap<PathBuf, Stamp>,
) -> Vec<Change> {
    let mut changes = Vec::new();
    for (path, now) in after {
        let name = relative(root, path);
        match before.get(path) {
            None => changes.push((name, CREATE)),
            Some(old) => {
                if old.mode != now.mode {
                    changes.push((name.clone(), CHMOD));
                }
                if !now.dir && (old.len != now.len || old.mtime != now.mtime) {
                    changes.push((name, WRITE));
                }
            }
        }
    }
    for path in before.keys() {
        if !after.contains_key(path) {
            changes.push((relative(root, path), REMOVE));
        }
    }
    changes
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Starts the thread that polls `root` until `stop` is set, queueing its changes on `events`.
fn spawn_poller(
    root: PathBuf,
    recursive: bool,
    baseline: BTreeMap<PathBuf, Stamp>,
    events: Arc<Mutex<VecDeque<Change>>>,
    stop: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let mut previous = baseline;
        while !stop.load(Ordering::Relaxed) {
            thread::sleep(POLL);
            let current = snapshot(&root, recursive);
            let changes = diff(&root, &previous, &current);
            previous = current;
            if changes.is_empty() {
                continue;
            }
            let mut queue = lock(&events);
            for change in changes {
                if queue.len() == MAX_QUEUED_EVENTS {
                    queue.pop_front();
                }
                queue.push_back(change);
            }
        }
    });
}

/// Scratch directories and a context for the tests, shared with `files`' tests.
#[cfg(test)]
pub(super) mod test_support {
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::super::Ctx;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    /// A fresh directory under the system temp dir, removed when dropped. It is owned by the
    /// running user, so the context's uid and gid are the current ids.
    pub struct Scratch(pub PathBuf);

    impl Scratch {
        pub fn new(label: &str) -> Self {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("apz-envd-{label}-{}-{n}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        pub fn ctx(&self) -> Ctx {
            let meta = fs::metadata(&self.0).unwrap();
            Ctx {
                home: self.0.clone(),
                uid: meta.uid(),
                gid: meta.gid(),
                user_uid: meta.uid(),
                user_gid: meta.gid(),
                workdir: None,
                env: Vec::new(),
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::os::unix::fs::symlink;
    use std::thread;

    use super::test_support::Scratch;
    use super::*;

    fn code_of(result: Result<Value, RpcError>) -> &'static str {
        result.unwrap_err().code
    }

    #[test]
    fn stat_reports_files_directories_and_links() {
        let scratch = Scratch::new("stat");
        let ctx = scratch.ctx();
        fs::write(scratch.0.join("a.txt"), "hello").unwrap();
        fs::create_dir(scratch.0.join("d")).unwrap();
        symlink("a.txt", scratch.0.join("link")).unwrap();
        symlink("missing", scratch.0.join("dangling")).unwrap();

        let file = unary("Stat", &json!({"path": "a.txt"}), &ctx).unwrap()["entry"].clone();
        assert_eq!(file["name"].as_str(), Some("a.txt"));
        assert_eq!(file["type"].as_str(), Some("FILE_TYPE_FILE"));
        assert_eq!(file["size"].as_str(), Some("5"));
        assert_eq!(
            file["path"].as_str(),
            Some(scratch.0.join("a.txt").to_string_lossy().as_ref())
        );
        let permissions = file["permissions"].as_str().unwrap();
        assert_eq!(permissions.len(), 10);
        assert!(permissions.starts_with('-'));
        assert!(file["modifiedTime"].as_str().unwrap().ends_with('Z'));
        assert_eq!(file["isSymlink"].as_bool(), Some(false));
        assert!(file.get("symlinkTarget").is_none());

        let dir = unary("Stat", &json!({"path": "d"}), &ctx).unwrap()["entry"].clone();
        assert_eq!(dir["type"].as_str(), Some("FILE_TYPE_DIRECTORY"));
        assert!(dir["permissions"].as_str().unwrap().starts_with('d'));

        let link = unary("Stat", &json!({"path": "link"}), &ctx).unwrap()["entry"].clone();
        assert_eq!(link["type"].as_str(), Some("FILE_TYPE_FILE"));
        assert_eq!(link["isSymlink"].as_bool(), Some(true));
        assert!(link["symlinkTarget"].as_str().unwrap().ends_with("a.txt"));
        assert!(link["permissions"].as_str().unwrap().starts_with('l'));

        let dangling = unary("Stat", &json!({"path": "dangling"}), &ctx).unwrap()["entry"].clone();
        assert_eq!(dangling["type"].as_str(), Some("FILE_TYPE_SYMLINK"));

        assert_eq!(
            code_of(unary("Stat", &json!({"path": "nope"}), &ctx)),
            "not_found"
        );
        assert_eq!(
            code_of(unary("Stat", &json!({"path": ""}), &ctx)),
            "invalid_argument"
        );
    }

    #[test]
    fn relative_and_tilde_paths_resolve_against_home() {
        let scratch = Scratch::new("resolve");
        let ctx = scratch.ctx();
        fs::write(scratch.0.join("a.txt"), "x").unwrap();
        let expected = scratch.0.join("a.txt").to_string_lossy().into_owned();
        for raw in ["a.txt", "~/a.txt"] {
            let entry = unary("Stat", &json!({"path": raw}), &ctx).unwrap()["entry"].clone();
            assert_eq!(entry["path"].as_str(), Some(expected.as_str()), "{raw}");
        }
        assert_eq!(resolve("~", Path::new("/h")), PathBuf::from("/h"));
        assert_eq!(resolve("/abs/x", Path::new("/h")), PathBuf::from("/abs/x"));
    }

    #[test]
    fn list_dir_lists_to_the_requested_depth() {
        let scratch = Scratch::new("list");
        let ctx = scratch.ctx();
        fs::create_dir_all(scratch.0.join("d/e")).unwrap();
        fs::write(scratch.0.join("d/e/f.txt"), "x").unwrap();
        fs::write(scratch.0.join("a.txt"), "x").unwrap();
        let names = |depth: Option<u64>| -> Vec<String> {
            let request = match depth {
                Some(depth) => json!({"path": ".", "depth": depth}),
                None => json!({"path": "."}),
            };
            unary("ListDir", &request, &ctx).unwrap()["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| entry["name"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(names(None), ["a.txt", "d"]);
        assert_eq!(names(Some(2)), ["a.txt", "d", "e"]);
        assert_eq!(names(Some(3)), ["a.txt", "d", "e", "f.txt"]);
    }

    #[test]
    fn make_dir_creates_parents_and_refuses_an_existing_path() {
        let scratch = Scratch::new("mkdir");
        let ctx = scratch.ctx();
        let entry = unary("MakeDir", &json!({"path": "x/y/z"}), &ctx).unwrap()["entry"].clone();
        assert_eq!(entry["type"].as_str(), Some("FILE_TYPE_DIRECTORY"));
        assert!(scratch.0.join("x/y/z").is_dir());
        assert_eq!(
            code_of(unary("MakeDir", &json!({"path": "x/y"}), &ctx)),
            "already_exists"
        );
    }

    #[test]
    fn move_renames_and_reports_the_destination() {
        let scratch = Scratch::new("move");
        let ctx = scratch.ctx();
        fs::write(scratch.0.join("a.txt"), "x").unwrap();
        let entry = unary(
            "Move",
            &json!({"source": "a.txt", "destination": "b.txt"}),
            &ctx,
        )
        .unwrap()["entry"]
            .clone();
        assert_eq!(entry["name"].as_str(), Some("b.txt"));
        assert!(!scratch.0.join("a.txt").exists());
        assert!(scratch.0.join("b.txt").is_file());
        assert_eq!(
            code_of(unary(
                "Move",
                &json!({"source": "a.txt", "destination": "c.txt"}),
                &ctx
            )),
            "not_found"
        );
    }

    #[test]
    fn remove_deletes_files_and_directories_recursively() {
        let scratch = Scratch::new("remove");
        let ctx = scratch.ctx();
        fs::create_dir_all(scratch.0.join("d/e")).unwrap();
        fs::write(scratch.0.join("d/e/f.txt"), "x").unwrap();
        fs::write(scratch.0.join("a.txt"), "x").unwrap();
        unary("Remove", &json!({"path": "d"}), &ctx).unwrap();
        unary("Remove", &json!({"path": "a.txt"}), &ctx).unwrap();
        assert!(!scratch.0.join("d").exists());
        assert!(!scratch.0.join("a.txt").exists());
        assert_eq!(
            code_of(unary("Remove", &json!({"path": "a.txt"}), &ctx)),
            "not_found"
        );
    }

    /// Polls the watcher until an event `kind` for `name` has been seen, keeping all seen events.
    fn wait_for(ctx: &Ctx, id: &str, seen: &mut Vec<(String, String)>, name: &str, kind: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let events = unary("GetWatcherEvents", &json!({"watcherId": id}), ctx).unwrap();
            for event in events["events"].as_array().unwrap() {
                seen.push((
                    event["name"].as_str().unwrap().to_string(),
                    event["type"].as_str().unwrap().to_string(),
                ));
            }
            if seen.iter().any(|(n, k)| n == name && k == kind) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "no {kind} for {name}; saw {seen:?}"
            );
            thread::sleep(POLL);
        }
    }

    #[test]
    fn watcher_reports_create_write_and_remove() {
        let scratch = Scratch::new("watcher");
        let ctx = scratch.ctx();
        let id = unary(
            "CreateWatcher",
            &json!({"path": ".", "recursive": false}),
            &ctx,
        )
        .unwrap()["watcherId"]
            .as_str()
            .unwrap()
            .to_string();
        let mut seen = Vec::new();

        fs::write(scratch.0.join("w.txt"), "a").unwrap();
        wait_for(&ctx, &id, &mut seen, "w.txt", CREATE);
        fs::write(scratch.0.join("w.txt"), "a longer body").unwrap();
        wait_for(&ctx, &id, &mut seen, "w.txt", WRITE);
        fs::remove_file(scratch.0.join("w.txt")).unwrap();
        wait_for(&ctx, &id, &mut seen, "w.txt", REMOVE);

        unary("RemoveWatcher", &json!({"watcherId": id}), &ctx).unwrap();
        assert_eq!(
            code_of(unary("GetWatcherEvents", &json!({"watcherId": id}), &ctx)),
            "not_found"
        );
    }

    /// A sink that records what it is sent and fails once a `s.txt` event arrives (or after ten
    /// seconds, which marks the test as timed out).
    struct Collect {
        messages: Vec<Value>,
        deadline: Instant,
        timed_out: bool,
    }

    impl StreamSink for Collect {
        fn message(&mut self, message: &Value) -> io::Result<()> {
            self.messages.push(message.clone());
            if message["filesystem"]["name"].as_str() == Some("s.txt") {
                return Err(io::Error::other("client went away"));
            }
            if Instant::now() > self.deadline {
                self.timed_out = true;
                return Err(io::Error::other("timed out"));
            }
            Ok(())
        }
    }

    #[test]
    fn watch_dir_streams_start_then_events_until_the_sink_fails() {
        let scratch = Scratch::new("watch-stream");
        let ctx = scratch.ctx();
        let root = scratch.0.clone();
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(600));
            fs::write(root.join("s.txt"), "x").unwrap();
        });
        let mut sink = Collect {
            messages: Vec::new(),
            deadline: Instant::now() + Duration::from_secs(10),
            timed_out: false,
        };
        let path = scratch.0.to_string_lossy().into_owned();
        stream("WatchDir", &json!({"path": path}), &ctx, &mut sink).unwrap();
        writer.join().unwrap();
        assert!(!sink.timed_out);
        assert!(sink.messages[0].get("start").is_some());
        assert!(sink.messages.iter().any(|message| {
            message["filesystem"]["name"].as_str() == Some("s.txt")
                && message["filesystem"]["type"].as_str() == Some(CREATE)
        }));
    }
}
