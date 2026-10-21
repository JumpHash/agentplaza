//! The directory a caller is confined to, and the path rules that keep operations inside it.
//!
//! The unix open and write paths make a few raw syscalls (`openat2`, `openat`, `mkdirat`, `renameat`,
//! `unlinkat`, `fcntl`). They are the only `unsafe` in the confinement. Each function that makes one carries
//! `#[allow(unsafe_code)]`, and each block states its invariant.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::ffi::{CString, OsStr, OsString};

use apz_ide::path::PathError;

use crate::error::OpError;

/// The confined directory. Every operation names its paths relative to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    dir: PathBuf,
}

impl Root {
    /// The root at `dir`, as given. It is canonicalized per operation, so it may be a symlink.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The directory as given.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The canonical root. A root that is not there is unavailable.
    pub fn canonical(&self) -> Result<PathBuf, OpError> {
        fs::canonicalize(&self.dir).map_err(|err| {
            OpError::Unavailable(format!(
                "the workspace {} is not mounted: {err}",
                self.dir.display()
            ))
        })
    }
}

/// A path an operation may touch: its cleaned name, and where it is.
pub(crate) struct Target {
    pub(crate) rel: String,
    pub(crate) abs: PathBuf,
}

/// The native path for a `/`-separated wire path. A verbatim path (`\\?\`, which a canonical root is on Windows)
/// takes no `/` as a separator, so the parts are joined with the OS separator here. Empty parts are dropped.
pub(crate) fn native(rel: &str) -> PathBuf {
    rel.split('/').filter(|part| !part.is_empty()).collect()
}

/// Checks `raw` with `lexical`, then the real filesystem: the deepest existing ancestor is
/// canonicalized and must land inside the canonical root, and the place it lands is checked with
/// `landed`.
pub(crate) fn locate(
    root: &Root,
    raw: &str,
    lexical: fn(&str) -> Result<String, PathError>,
    landed: fn(&str) -> Result<String, PathError>,
) -> Result<Target, OpError> {
    let rel = lexical(raw).map_err(|err| path_refusal(raw, err))?;
    let canonical = root.canonical()?;
    let mut ancestor = canonical.join(native(&rel));
    let mut walked_up = false;
    let resolved = loop {
        match fs::canonicalize(&ancestor) {
            Ok(resolved) => break resolved,
            Err(err) if err.kind() == io::ErrorKind::NotFound => match ancestor.parent() {
                Some(parent) => {
                    ancestor = parent.to_path_buf();
                    walked_up = true;
                }
                None => return Err(OpError::denied(format!("{raw:?} cannot be resolved"))),
            },
            Err(err) => return Err(OpError::io(err, format!("cannot resolve {raw:?}"))),
        }
    };
    // Nothing exists below a file. Unix says so while resolving (ENOTDIR, above); Windows answers "path not found",
    // as for a missing directory, so the walk up lands on the file: the same refusal here, not a missing file later.
    if walked_up && !resolved.is_dir() {
        return Err(OpError::io(
            io::Error::from(io::ErrorKind::NotADirectory),
            format!("cannot resolve {raw:?}"),
        ));
    }
    let Ok(inside) = resolved.strip_prefix(&canonical) else {
        return Err(OpError::denied(format!(
            "{raw:?} resolves to {} outside the workspace",
            resolved.display()
        )));
    };
    // The wire path is `/`-separated on every OS, so the components are joined with `/` rather
    // than taking the OS separator (a backslash on Windows, which `check_*` refuses).
    let inside = inside
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if !inside.is_empty() {
        landed(&inside).map_err(|err| path_refusal(raw, err))?;
    }
    Ok(Target {
        abs: canonical.join(native(&rel)),
        rel,
    })
}

/// The refusal for a path the rules name. Malformed input is invalid; the rest are denied.
fn path_refusal(raw: &str, err: PathError) -> OpError {
    match err {
        PathError::Empty | PathError::Nul | PathError::Control => {
            OpError::invalid(format!("{raw:?}: {err}"))
        }
        _ => OpError::denied(format!("{raw:?} refused: {err}")),
    }
}

/// How a path is shown in a reply: `.` for the root itself.
pub(crate) fn shown(rel: &str) -> &str {
    if rel.is_empty() { "." } else { rel }
}

/// Flags for reading a file: links refused, and a FIFO, device or socket never blocks the open.
#[cfg(unix)]
const FILE_FLAGS: libc::c_int =
    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_CLOEXEC;

/// Flags for a folder: a directory, with links refused.
#[cfg(unix)]
const DIR_FLAGS: libc::c_int =
    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

/// Opens `rel` under `root_dir` for reading, and returns only a regular file.
///
/// `rel` is plain names: a `..`, a root or a prefix is refused. The path is resolved through its links
/// once and must land inside the canonical root. The file is then opened from a handle to that root by
/// its resolved names with links refused, so a link swapped in after the check is refused rather than
/// followed. The kind is asked of the open handle, so a FIFO, device or socket swapped in is refused
/// without a read.
#[cfg(unix)]
pub(crate) fn open_confined(root_dir: &Path, rel: &Path) -> io::Result<fs::File> {
    let (root, inside) = resolve(root_dir, rel)?;
    if inside.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "names the root itself",
        ));
    }
    regular(open_under(&root, &inside, FILE_FLAGS)?)
}

/// Opens the folder `rel_dir` under `root_dir`, resolved and opened as [`open_confined`] opens a file. An
/// empty `rel_dir` is the root itself. A folder that is not there is `NotFound`.
#[cfg(unix)]
pub(crate) fn open_confined_dir(root_dir: &Path, rel_dir: &Path) -> io::Result<fs::File> {
    let (root, inside) = resolve(root_dir, rel_dir)?;
    if inside.as_os_str().is_empty() {
        return Ok(root);
    }
    open_under(&root, &inside, DIR_FLAGS)
}

/// Opens the folder `rel_dir` under `root_dir` for writing into, making each folder that is missing. It
/// returns the handle and the folders it made, outermost first, as absolute paths.
///
/// The part that exists is resolved and opened as [`open_confined_dir`] opens it. Each missing name is then
/// made with `mkdirat` from the handle before it and opened again with links refused, so a link swapped in
/// for a missing name is refused rather than followed.
#[cfg(unix)]
pub(crate) fn make_confined_dirs(
    root_dir: &Path,
    rel_dir: &Path,
) -> io::Result<(fs::File, Vec<PathBuf>)> {
    plain(rel_dir)?;
    let canonical = fs::canonicalize(root_dir)?;
    let mut existing = canonical.join(rel_dir);
    let mut missing: Vec<OsString> = Vec::new();
    let resolved = loop {
        match fs::canonicalize(&existing) {
            Ok(resolved) => break resolved,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                let parent = existing.parent().map(Path::to_path_buf);
                let name = existing.file_name().map(OsStr::to_os_string);
                match (parent, name) {
                    (Some(parent), Some(name)) => {
                        missing.push(name);
                        existing = parent;
                    }
                    _ => return Err(err),
                }
            }
            Err(err) => return Err(err),
        }
    };
    let inside = resolved
        .strip_prefix(&canonical)
        .map_err(|_| outside_root())?;
    let mut dir = open_confined_dir(&canonical, inside)?;
    let mut made = Vec::new();
    let mut abs = resolved.clone();
    missing.reverse();
    for name in missing {
        abs.push(&name);
        if mkdir_under(&dir, &name)? {
            made.push(abs.clone());
        }
        dir = openat_name(&dir, &name, DIR_FLAGS)?;
    }
    Ok((dir, made))
}

/// Creates `name` under `dir`, exclusively: a name that exists, a link included, is never opened, and a
/// link is never followed. The mode is `0o666` less the umask, as any new file gets.
#[cfg(unix)]
#[allow(unsafe_code)]
pub(crate) fn create_new_under(dir: &fs::File, name: &OsStr) -> io::Result<fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};

    let name = c_name(name)?;
    let flags = libc::O_CREAT | libc::O_EXCL | libc::O_WRONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    // SAFETY: `name` is NUL-terminated and `dir` is an open descriptor for the whole call. The mode is the
    // one variadic argument; an unsuffixed literal is passed as a `c_int`, which is how C promotes it.
    let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags, 0o666) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful `openat` returns a new descriptor that nothing else owns.
    Ok(unsafe { fs::File::from_raw_fd(fd) })
}

/// `renameat` within `dir`: `from` replaces `to` in one step, or nothing changes.
#[cfg(unix)]
#[allow(unsafe_code)]
pub(crate) fn rename_under(dir: &fs::File, from: &OsStr, to: &OsStr) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    let from = c_name(from)?;
    let to = c_name(to)?;
    let fd = dir.as_raw_fd();
    // SAFETY: both names are NUL-terminated, and `dir` is open for the whole call and is both the source and
    // the target folder.
    if unsafe { libc::renameat(fd, from.as_ptr(), fd, to.as_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// `unlinkat` of `name` under `dir`, for the temporary file a failed write leaves behind.
#[cfg(unix)]
#[allow(unsafe_code)]
pub(crate) fn remove_under(dir: &fs::File, name: &OsStr) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    let name = c_name(name)?;
    // SAFETY: `name` is NUL-terminated and `dir` is open for the whole call. No flags: the name is not a folder.
    if unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The root handle and the resolved names of `rel` under it. The path is resolved through its links once
/// and must land inside the canonical root, so the names it lands on hold no links to follow.
#[cfg(unix)]
fn resolve(root_dir: &Path, rel: &Path) -> io::Result<(fs::File, PathBuf)> {
    plain(rel)?;
    let canonical = fs::canonicalize(root_dir)?;
    let resolved = fs::canonicalize(canonical.join(rel))?;
    let inside = resolved
        .strip_prefix(&canonical)
        .map_err(|_| outside_root())?
        .to_path_buf();
    Ok((fs::File::open(&canonical)?, inside))
}

/// `rel` is plain names: a `..`, a root, a prefix or a `.` is refused.
#[cfg(unix)]
fn plain(rel: &Path) -> io::Result<()> {
    use std::path::Component;

    if rel
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a confined path is plain names only",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn outside_root() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "resolves outside the root")
}

/// Opens `inside` (plain names, none empty) under the root handle: the last name with `last` flags, and each
/// name before it as a folder. `openat2` is tried on Linux, and the walk is the portable path.
#[cfg(target_os = "linux")]
fn open_under(root: &fs::File, inside: &Path, last: libc::c_int) -> io::Result<fs::File> {
    match open_beneath(root, inside, last) {
        Some(opened) => opened,
        None => open_by_walk(root, inside, last),
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn open_under(root: &fs::File, inside: &Path, last: libc::c_int) -> io::Result<fs::File> {
    open_by_walk(root, inside, last)
}

/// `openat2` with `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS`. `None` when the kernel has no `openat2`.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn open_beneath(
    root: &fs::File,
    inside: &Path,
    flags: libc::c_int,
) -> Option<io::Result<fs::File>> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    /// The `struct open_how` of `openat2(2)`, spelled here so the build does not depend on the libc version.
    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }
    const RESOLVE_NO_SYMLINKS: u64 = 0x04;
    const RESOLVE_BENEATH: u64 = 0x08;

    let path = match CString::new(inside.as_os_str().as_bytes()) {
        Ok(path) => path,
        Err(err) => return Some(Err(io::Error::new(io::ErrorKind::InvalidInput, err))),
    };
    let how = OpenHow {
        flags: flags as u64,
        mode: 0,
        resolve: RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS,
    };
    // SAFETY: `path` is NUL-terminated, `how` is a live `open_how` of the size passed, and `root` is an
    // open directory for the whole call.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            root.as_raw_fd() as libc::c_long,
            path.as_ptr(),
            &how as *const OpenHow,
            std::mem::size_of::<OpenHow>(),
        )
    };
    if fd >= 0 {
        // SAFETY: a successful `openat2` returns a new descriptor that nothing else owns.
        return Some(Ok(unsafe { fs::File::from_raw_fd(fd as libc::c_int) }));
    }
    let err = io::Error::last_os_error();
    (err.raw_os_error() != Some(libc::ENOSYS)).then_some(Err(err))
}

/// The portable walk: each name is opened with `openat` from the handle before it. Every name but the last
/// is a folder, and no name is a link.
#[cfg(unix)]
fn open_by_walk(root: &fs::File, inside: &Path, last: libc::c_int) -> io::Result<fs::File> {
    let names: Vec<&OsStr> = inside.iter().collect();
    let Some((final_name, dirs)) = names.split_last() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "names the root itself",
        ));
    };
    let mut dir = root.try_clone()?;
    for name in dirs {
        dir = openat_name(&dir, name, DIR_FLAGS)?;
    }
    openat_name(&dir, final_name, last)
}

/// One `openat` of `name` under the directory `dir`, with `flags`.
#[cfg(unix)]
#[allow(unsafe_code)]
fn openat_name(dir: &fs::File, name: &OsStr, flags: libc::c_int) -> io::Result<fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};

    let name = c_name(name)?;
    // SAFETY: `name` is NUL-terminated and `dir` is an open descriptor for the whole call.
    let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful `openat` returns a new descriptor that nothing else owns.
    Ok(unsafe { fs::File::from_raw_fd(fd) })
}

/// `mkdirat` of `name` under `dir`: `true` when it made the folder, `false` when the name was already there.
#[cfg(unix)]
#[allow(unsafe_code)]
fn mkdir_under(dir: &fs::File, name: &OsStr) -> io::Result<bool> {
    use std::os::fd::AsRawFd;

    let name = c_name(name)?;
    // SAFETY: `name` is NUL-terminated and `dir` is an open descriptor for the whole call.
    if unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o777) } == 0 {
        return Ok(true);
    }
    let err = io::Error::last_os_error();
    if err.kind() == io::ErrorKind::AlreadyExists {
        Ok(false)
    } else {
        Err(err)
    }
}

/// A name as a C string, or a refusal when it holds a NUL.
#[cfg(unix)]
fn c_name(name: &OsStr) -> io::Result<CString> {
    use std::os::unix::ffi::OsStrExt;

    CString::new(name.as_bytes()).map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))
}

/// The file once its handle is known to be a regular file. The kind is asked of the handle, not the
/// path, and `O_NONBLOCK` is cleared so later reads block as they always did.
#[cfg(unix)]
#[allow(unsafe_code)]
fn regular(file: fs::File) -> io::Result<fs::File> {
    use std::os::fd::AsRawFd;

    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let fd = file.as_raw_fd();
    // SAFETY: `fd` belongs to `file`, which stays open for the whole call.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: as above; only the status flags change.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

/// Windows keeps the path as it was opened before: the canonical root joined with the relative path.
#[cfg(not(unix))]
pub(crate) fn open_confined(root_dir: &Path, rel: &Path) -> io::Result<fs::File> {
    fs::File::open(fs::canonicalize(root_dir)?.join(rel))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `path_refusal` for `raw` and `err` is invalid input that names the path in debug form, then the rule.
    fn invalid(raw: &str, err: PathError) {
        let rule = err.to_string();
        match path_refusal(raw, err) {
            OpError::InvalidInput(reason) => {
                assert!(reason.starts_with(&format!("{raw:?}: ")), "{reason}");
                assert!(reason.ends_with(&rule), "{reason}");
            }
            other => panic!("expected invalid input, got {other}"),
        }
    }

    /// `path_refusal` for `raw` and `err` is a denial that names the path in debug form, then the rule.
    fn denied(raw: &str, err: PathError) {
        let rule = err.to_string();
        match path_refusal(raw, err) {
            OpError::Denied(reason) => {
                assert!(
                    reason.starts_with(&format!("{raw:?} refused: ")),
                    "{reason}"
                );
                assert!(reason.ends_with(&rule), "{reason}");
            }
            other => panic!("expected a denial, got {other}"),
        }
    }

    #[test]
    fn an_empty_path_is_invalid_input() {
        invalid("", PathError::Empty);
    }

    #[test]
    fn a_nul_in_the_path_is_invalid_input() {
        invalid("a\0b", PathError::Nul);
    }

    #[test]
    fn a_control_character_in_the_path_is_invalid_input() {
        invalid("a\u{1}b", PathError::Control);
    }

    #[test]
    fn an_absolute_path_is_denied() {
        denied("/etc/passwd", PathError::Absolute);
    }

    #[test]
    fn a_backslash_path_is_denied() {
        denied("a\\b", PathError::Backslash);
    }

    #[test]
    fn a_path_with_a_parent_segment_is_denied() {
        denied("a/../../x", PathError::Escape);
    }

    #[test]
    fn a_path_under_the_plaza_directory_is_denied() {
        denied(".plaza/state", PathError::Reserved);
    }

    #[test]
    fn a_secret_path_is_denied() {
        denied(".env", PathError::Secret);
    }

    #[test]
    fn shown_writes_the_root_as_a_dot_and_every_other_path_as_it_is() {
        assert_eq!(shown(""), ".");
        assert_eq!(shown("src/lib.rs"), "src/lib.rs");
        assert_eq!(shown(" "), " ", "only the empty path is the root");
    }
}
