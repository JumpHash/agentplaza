//! Who owns what in a coding engine's world, so the engine (the `apz-engine` user, see
//! [`crate::bash::Privilege::Engine`]) can write only its job folder and its own home.
//!
//! - `/workspace/.plaza` (the harness's state) is root's, mode 0700: the engine cannot read it.
//! - The workspace root is root's and sticky (mode 1777, like `/tmp`), so the engine can create
//!   files in it but cannot rename or delete `.plaza`, `.npm-global` or `.engines`, which root owns.
//!   The root is never handed to the engine: its owner could rename `.plaza` away.
//! - The npm prefix (`.npm-global`) and each installed engine (`.engines/<id>-<version>`) are
//!   root's and read-only to the engine (dirs 0755, files not writable by others).
//! - The engine's home, `.engines/home/<id>`, is the engine's, mode 0700, with its install cache.
//! - The job folder (the call's `cwd`) is the engine's, recursively. When it is the workspace root,
//!   the three harness folders above are left out.
//!
//! Every change here needs root and is a no-op otherwise (tests, dev). The plan is pure and tested;
//! the syscalls only run in a root guest.
//!
//! The walk that applies a step is race-free against the folders other users can write while it runs
//! (a `bash` command's own dev server, an engine that left its process group). It opens each folder
//! from a trusted anchor with `O_NOFOLLOW`, changes entries only through file descriptors and `*at`
//! calls, never follows a link, never leaves the step's device, and never hands the engine a file with
//! another hard link. Stray engine processes are killed before a walk (see [`bash::kill_engine_processes`]).

use std::ffi::{CStr, CString, OsStr};
use std::io;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use rustix::fs::{self as rfs, AtFlags, Dir, FileType, Mode, OFlags, Stat};
use rustix::process::{Gid, Uid};

use crate::bash::{self, ENGINE_UID};
use crate::files::Workspace;

/// The folders of the workspace the engine never owns: the harness's state, the npm prefix and the
/// installed engines.
const RESERVED: [&str; 3] = [".plaza", ".npm-global", ".engines"];

/// Who owns a path in the layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Owner {
    /// Root, which the engine cannot write.
    Root,
    /// The engine user: uid and gid [`ENGINE_UID`].
    Engine,
}

impl Owner {
    /// The uid and gid of the owner, for `fchown` and `chownat`.
    fn ids(self) -> (Option<Uid>, Option<Gid>) {
        let id = match self {
            Owner::Root => 0,
            Owner::Engine => ENGINE_UID,
        };
        (Some(Uid::from_raw(id)), Some(Gid::from_raw(id)))
    }
}

/// One change to the layout: the owner of `path` (and of what is under it when `tree` is set, except
/// what `skip` names), and the mode of its directories. Under a root owner, the files below lose the
/// group and other write bits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Step {
    pub path: PathBuf,
    pub owner: Owner,
    /// Whether the entries under `path` take the owner too.
    pub tree: bool,
    /// Whether `path` itself takes the owner (the workspace root never does).
    pub itself: bool,
    /// The mode of directories, `path` included, when set.
    pub mode: Option<u32>,
    /// Paths under `path` left alone, with everything under them.
    pub skip: Vec<PathBuf>,
}

impl Step {
    /// `path` and everything under it, owned by the engine.
    fn engine(path: &Path, mode: Option<u32>) -> Self {
        Self {
            path: path.to_path_buf(),
            owner: Owner::Engine,
            tree: true,
            itself: true,
            mode,
            skip: Vec::new(),
        }
    }

    /// `path` and everything under it, owned by root, with directories at `mode`.
    fn root_tree(path: &Path, mode: u32) -> Self {
        Self {
            mode: Some(mode),
            owner: Owner::Root,
            ..Self::engine(path, None)
        }
    }

    /// `path` alone, owned by root, at `mode`.
    fn root_dir(path: &Path, mode: u32) -> Self {
        Self {
            tree: false,
            ..Self::root_tree(path, mode)
        }
    }
}

/// The engine's own folder: `<workspace>/.engines/home/<id>`, where its `HOME` is.
pub(crate) fn engine_home(workspace: &Workspace, id: &str) -> PathBuf {
    home_in(workspace.dir(), id)
}

fn home_in(root: &Path, id: &str) -> PathBuf {
    root.join(".engines").join("home").join(id)
}

/// The engine's download cache for installs (its npm packs and release archives), inside its home so
/// the engine writes nothing outside its job folder and its home.
pub(crate) fn install_cache(home: &Path) -> PathBuf {
    home.join("install-cache")
}

/// The job folder must not be one of the harness's folders, which the engine may not own.
fn check_job(root: &Path, job: &Path) -> Result<(), String> {
    if job != root && RESERVED.iter().any(|name| job.starts_with(root.join(name))) {
        return Err(format!(
            "invalid input: {} is a harness folder an engine cannot work in",
            job.display()
        ));
    }
    Ok(())
}

/// The layout steps for engine `id` at `version` (its installed folder) working in `job`, under the
/// workspace `root`.
pub(crate) fn plan(root: &Path, id: &str, version: &str, job: &Path) -> Vec<Step> {
    let plaza = root.join(".plaza");
    let npm = root.join(".npm-global");
    let engines = root.join(".engines");
    let installed = engines.join(format!("{id}-{version}"));
    let mut steps = vec![
        Step::root_dir(&plaza, 0o700),
        Step::root_tree(&npm, 0o755),
        Step::root_dir(&engines, 0o755),
        Step::root_tree(&installed, 0o755),
        Step::engine(&home_in(root, id), Some(0o700)),
    ];
    if job == root {
        steps.push(Step {
            path: root.to_path_buf(),
            owner: Owner::Engine,
            tree: true,
            itself: false,
            mode: None,
            skip: vec![plaza, npm, engines],
        });
        steps.push(Step::root_dir(root, 0o1777));
    } else {
        // Back to root's and closed, in case an earlier job was the whole workspace.
        steps.push(Step::root_dir(root, 0o755));
        steps.push(Step::engine(job, None));
    }
    steps
}

/// Opens a folder without following a link: the flags of every step of a walk.
fn dir_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

/// Applies one step under `anchor`, a folder root owns and the engine cannot write (the workspace root,
/// or the parent of an install's folder). A missing path is skipped. Every folder on the way is opened
/// from the anchor with `O_NOFOLLOW`, and every change is made through a descriptor or a name in an
/// open folder, so a folder swapped for a link while the walk runs is never followed.
fn apply(anchor: &Path, step: &Step) -> io::Result<()> {
    if step.path == anchor {
        let Some(dir) = open_folder(anchor, anchor)? else {
            return Ok(());
        };
        let dev = rfs::fstat(&dir)?.st_dev;
        if step.itself {
            own_folder(dir.as_fd(), step)?;
        }
        if step.tree {
            walk_children(dir.as_fd(), dev, step, &skip_names(step))?;
        }
        return Ok(());
    }
    let (Some(parent), Some(name)) = (step.path.parent(), step.path.file_name()) else {
        return Err(io::Error::other(format!(
            "{} has no parent folder",
            step.path.display()
        )));
    };
    let Some(parent) = open_folder(anchor, parent)? else {
        return Ok(());
    };
    let name = CString::new(name.as_bytes()).map_err(io::Error::other)?;
    apply_entry(parent.as_fd(), &name, None, step, step.itself)
}

/// The folder `path` under `anchor` (`anchor` itself included), opened one component at a time without
/// following a link. A missing component is `None`; a link or a file on the way is an error.
fn open_folder(anchor: &Path, path: &Path) -> io::Result<Option<OwnedFd>> {
    let rest = path.strip_prefix(anchor).map_err(|_| {
        io::Error::other(format!(
            "{} is not under {}",
            path.display(),
            anchor.display()
        ))
    })?;
    let mut dir = rfs::open(
        anchor,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    for name in rest.iter() {
        let Some(next) = vanished(rfs::openat(&dir, name, dir_flags(), Mode::empty()))? else {
            return Ok(None);
        };
        dir = next;
    }
    Ok(Some(dir))
}

/// Creates the folders of `path` under `anchor`, which must exist and be root's (`anchor` itself is not
/// created). Each component is made and then opened from the folder before it with `O_NOFOLLOW`, so a
/// link planted in the path is never followed: a component that is a link or a file is an error, and
/// nothing is created through a link. The new folders are root's; the walks give them their owners and
/// modes afterwards.
pub(crate) fn create_beneath(anchor: &Path, path: &Path) -> io::Result<()> {
    let rest = path.strip_prefix(anchor).map_err(|_| {
        io::Error::other(format!(
            "{} is not under {}",
            path.display(),
            anchor.display()
        ))
    })?;
    let mut dir = rfs::open(
        anchor,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    for name in rest.iter() {
        if let Err(err) = rfs::mkdirat(&dir, name, Mode::from_raw_mode(0o755)) {
            // An entry that already exists is fine: what it is gets checked by the open below.
            if err != rustix::io::Errno::EXIST {
                return Err(err.into());
            }
        }
        dir = rfs::openat(&dir, name, dir_flags(), Mode::empty())?;
    }
    Ok(())
}

/// The names of the step's skipped folders: they are direct children of its path.
fn skip_names(step: &Step) -> Vec<&OsStr> {
    step.skip
        .iter()
        .filter_map(|path| path.file_name())
        .collect()
}

/// `result`, with a path that is gone read as nothing to do (the job's own processes can remove it
/// while the walk runs).
fn vanished<T, E: Into<io::Error>>(result: Result<T, E>) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(err) => {
            let err = err.into();
            if err.kind() == io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(err)
            }
        }
    }
}

/// `result` inside a walk, with an entry that is gone or became something else (a link where a folder
/// was, a file where a folder was) read as nothing to do. The job's own processes keep working while
/// the walk runs; an entry that changed under it is left alone, never followed.
fn moved<T, E: Into<io::Error>>(result: Result<T, E>) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(err) => {
            let err = err.into();
            let gone = err.kind() == io::ErrorKind::NotFound
                || matches!(err.raw_os_error(), Some(libc::ELOOP | libc::ENOTDIR));
            if gone { Ok(None) } else { Err(err) }
        }
    }
}

/// Applies `step` to what is directly under the open folder `dir`, except the names in `skip`.
fn walk_children(dir: BorrowedFd<'_>, dev: u64, step: &Step, skip: &[&OsStr]) -> io::Result<()> {
    for entry in Dir::read_from(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let bytes = name.to_bytes();
        if matches!(bytes, b"." | b"..") || skip.iter().any(|skipped| skipped.as_bytes() == bytes) {
            continue;
        }
        apply_entry(dir, name, Some(dev), step, true)?;
    }
    Ok(())
}

/// Applies `step` to the entry `name` of the open folder `parent`: its owner when `owned`, and what is
/// under it when it is a folder and `step.tree`. `dev` is the device the walk stays on, or `None` when
/// the entry starts the walk. An entry on another device is left alone. No link is followed: a symlink
/// is only re-owned, as itself. A file with another hard link is never handed to the engine, since the
/// other link is outside the folder.
fn apply_entry(
    parent: BorrowedFd<'_>,
    name: &CStr,
    dev: Option<u64>,
    step: &Step,
    owned: bool,
) -> io::Result<()> {
    let Some(st) = moved(rfs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW))? else {
        return Ok(());
    };
    if dev.is_some_and(|dev| dev != st.st_dev) {
        return Ok(());
    }
    let kind = FileType::from_raw_mode(st.st_mode);
    if kind == FileType::Directory {
        let Some(dir) = moved(rfs::openat(parent, name, dir_flags(), Mode::empty()))? else {
            return Ok(());
        };
        let opened = rfs::fstat(&dir)?;
        if (opened.st_dev, opened.st_ino) != (st.st_dev, st.st_ino) {
            // Replaced since the stat: not the folder the walk decided on.
            return Ok(());
        }
        if owned {
            own_folder(dir.as_fd(), step)?;
        }
        if step.tree {
            walk_children(dir.as_fd(), opened.st_dev, step, &[])?;
        }
        return Ok(());
    }
    if !owned || (step.owner == Owner::Engine && st.st_nlink > 1) {
        return Ok(());
    }
    if kind == FileType::RegularFile {
        return own_file(parent, name, &st, step);
    }
    // A link, a fifo or a device: its own owner, never followed and never a mode change.
    let (uid, gid) = step.owner.ids();
    moved(rfs::chownat(
        parent,
        name,
        uid,
        gid,
        AtFlags::SYMLINK_NOFOLLOW,
    ))
    .map(|_| ())
}

/// Gives the open folder `dir` to `step`'s owner, at `step`'s mode when it has one.
fn own_folder(dir: BorrowedFd<'_>, step: &Step) -> io::Result<()> {
    let (uid, gid) = step.owner.ids();
    rfs::fchown(dir, uid, gid)?;
    if let Some(mode) = step.mode {
        rfs::fchmod(dir, Mode::from_raw_mode(mode))?;
    }
    Ok(())
}

/// Gives the regular file `name` (statted as `st`) to `step`'s owner, through its descriptor. Under a
/// root owner the group and other write bits go. For the engine, a file that gains another link after
/// the check is given back to its old owner: the engine never keeps a file a link outside reaches.
fn own_file(parent: BorrowedFd<'_>, name: &CStr, st: &Stat, step: &Step) -> io::Result<()> {
    let flags =
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC;
    let Some(file) = moved(rfs::openat(parent, name, flags, Mode::empty()))? else {
        return Ok(());
    };
    let opened = rfs::fstat(&file)?;
    if (opened.st_dev, opened.st_ino) != (st.st_dev, st.st_ino)
        || FileType::from_raw_mode(opened.st_mode) != FileType::RegularFile
    {
        // Replaced since the stat: not the file the walk decided on.
        return Ok(());
    }
    let engine = step.owner == Owner::Engine;
    if engine && opened.st_nlink > 1 {
        return Ok(());
    }
    let (uid, gid) = step.owner.ids();
    rfs::fchown(&file, uid, gid)?;
    if engine && rfs::fstat(&file)?.st_nlink > 1 {
        rfs::fchown(
            &file,
            Some(Uid::from_raw(opened.st_uid)),
            Some(Gid::from_raw(opened.st_gid)),
        )?;
        return Ok(());
    }
    if !engine && step.mode.is_some() {
        rfs::fchmod(&file, Mode::from_raw_mode(opened.st_mode & 0o7777 & !0o022))?;
    }
    Ok(())
}

fn describe(err: io::Error) -> String {
    format!("unavailable: cannot prepare the engine's folders: {err}")
}

/// Lays out the folders for engine `id` at `version` working in `job`, before it is launched or
/// installed (root only). Creates its home and install cache, then applies the plan.
pub(crate) fn prepare_engine_dirs(
    workspace: &Workspace,
    id: &str,
    version: &str,
    job: &Path,
) -> Result<(), String> {
    let root = workspace.dir();
    check_job(root, job)?;
    if !bash::runs_as_root()? {
        return Ok(());
    }
    // Before any walk: a process an earlier call left behind could swap a folder the walk is in.
    bash::kill_engine_processes()?;
    bash::ensure_engine_user()?;
    let home = home_in(root, id);
    create_beneath(root, &install_cache(&home)).map_err(describe)?;
    for step in plan(root, id, version, job) {
        apply(root, &step).map_err(describe)?;
    }
    Ok(())
}

/// Runs `install` while `dir` is the engine's to write, then gives it back to root: the install
/// writes into it as the engine user, so its install scripts never run as root. Root owns the
/// folder afterwards, read-only to the engine. Not root: runs `install` as it is. The walks are
/// anchored at `dir`'s parent, which is root's (the npm prefix's, or the installed engines').
pub(crate) fn install_in<T>(
    dir: &Path,
    install: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    if !bash::runs_as_root()? {
        return install();
    }
    let anchor = dir
        .parent()
        .ok_or_else(|| format!("internal: {} has no parent folder", dir.display()))?;
    create_beneath(anchor, dir).map_err(describe)?;
    apply(anchor, &Step::engine(dir, None)).map_err(describe)?;
    let result = install();
    // Nothing the install started may outlive it: a process that left its group could still write
    // into the folder once root has it back.
    let stopped = bash::kill_engine_processes();
    // The folder goes back to root even when the install failed.
    let closed = apply(anchor, &Step::root_tree(dir, 0o755)).map_err(describe);
    let value = result?;
    stopped?;
    closed?;
    Ok(value)
}

/// Hands `path` and what is under it to the engine (root only), walking from the workspace root:
/// the files the harness writes for the engine (an ACP write, a config) must stay editable by it.
pub(crate) fn give_to_engine(workspace: &Path, path: &Path) -> Result<(), String> {
    if !bash::runs_as_root()? {
        return Ok(());
    }
    apply(workspace, &Step::engine(path, None)).map_err(describe)
}

/// The top-most of `path`'s ancestors that do not exist yet, if any: the folder a `create_dir_all`
/// of `path` is about to create first.
pub(crate) fn first_missing(path: &Path) -> Option<PathBuf> {
    let mut top = None;
    for ancestor in path.ancestors() {
        if ancestor.exists() {
            break;
        }
        top = Some(ancestor.to_path_buf());
    }
    top
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    use super::*;

    const ROOT: &str = "/workspace";

    fn step_at<'a>(steps: &'a [Step], path: &str) -> &'a Step {
        steps
            .iter()
            .find(|step| step.path == Path::new(path))
            .unwrap_or_else(|| panic!("a step for {path}"))
    }

    #[test]
    fn the_engine_plan_hands_each_owner_its_own_folders() {
        // Checked: the job root hands the engine its entries but never the root or the harness folders.
        {
            let steps = plan(Path::new(ROOT), "goose", "1.52.0", Path::new(ROOT));
            let job = steps
                .iter()
                .find(|step| {
                    step.owner == Owner::Engine && step.tree && step.path == Path::new(ROOT)
                })
                .expect("the job tree");
            assert!(
                !job.itself,
                "the root stays root's: its owner could rename .plaza"
            );
            assert_eq!(
                job.skip,
                vec![
                    PathBuf::from("/workspace/.plaza"),
                    PathBuf::from("/workspace/.npm-global"),
                    PathBuf::from("/workspace/.engines"),
                ]
            );
            let sticky = steps
                .iter()
                .find(|step| step.owner == Owner::Root && step.path == Path::new(ROOT))
                .expect("the root's own step");
            assert_eq!(sticky.mode, Some(0o1777));
            assert!(!sticky.tree);
        }
        // Checked: a subfolder job is the engine's alone with nothing skipped.
        {
            let steps = plan(
                Path::new(ROOT),
                "goose",
                "1.52.0",
                Path::new("/workspace/app"),
            );
            let job = step_at(&steps, "/workspace/app");
            assert_eq!(job.owner, Owner::Engine);
            assert!(job.tree && job.itself && job.skip.is_empty() && job.mode.is_none());
            let root = step_at(&steps, ROOT);
            assert_eq!(
                (root.owner, root.tree, root.mode),
                (Owner::Root, false, Some(0o755)),
                "the root is closed to the engine again"
            );
        }
        // Checked: the harness state and the installed engines stay root read-only.
        {
            let steps = plan(
                Path::new(ROOT),
                "goose",
                "1.52.0",
                Path::new("/workspace/app"),
            );
            let plaza = step_at(&steps, "/workspace/.plaza");
            assert_eq!(
                (plaza.owner, plaza.mode, plaza.tree),
                (Owner::Root, Some(0o700), false)
            );
            let npm = step_at(&steps, "/workspace/.npm-global");
            assert_eq!(
                (npm.owner, npm.mode, npm.tree),
                (Owner::Root, Some(0o755), true)
            );
            let installed = step_at(&steps, "/workspace/.engines/goose-1.52.0");
            assert_eq!(
                (installed.owner, installed.mode, installed.tree),
                (Owner::Root, Some(0o755), true)
            );
            let engines = step_at(&steps, "/workspace/.engines");
            assert_eq!((engines.owner, engines.tree), (Owner::Root, false));
        }
        // Checked: each engine has a private home of its own.
        {
            let steps = plan(
                Path::new(ROOT),
                "goose",
                "1.52.0",
                Path::new("/workspace/app"),
            );
            let home = step_at(&steps, "/workspace/.engines/home/goose");
            assert_eq!(
                (home.owner, home.mode, home.tree),
                (Owner::Engine, Some(0o700), true)
            );
            assert_eq!(
                install_cache(&home_in(Path::new(ROOT), "goose")),
                PathBuf::from("/workspace/.engines/home/goose/install-cache")
            );
            assert_ne!(
                home_in(Path::new(ROOT), "goose"),
                home_in(Path::new(ROOT), "vibe")
            );
        }
    }

    #[test]
    fn a_job_in_a_harness_folder_is_refused_before_anything_changes() {
        let root = Path::new(ROOT);
        assert!(check_job(root, root).is_ok());
        assert!(check_job(root, Path::new("/workspace/app")).is_ok());
        for reserved in [".plaza", ".npm-global", ".engines/home/goose"] {
            let job = root.join(reserved);
            assert!(check_job(root, &job).is_err(), "{}", job.display());
        }
    }

    #[test]
    fn create_beneath_makes_nested_folders_and_never_creates_through_a_link_or_a_file() {
        let scratch = Scratch::new("beneath");
        let anchor = scratch.0.join("anchor");
        let outside = scratch.0.join("outside");
        fs::create_dir_all(&anchor).expect("the anchor");
        fs::create_dir_all(&outside).expect("a folder outside");
        fs::write(anchor.join("file"), "kept").expect("a file");
        std::os::unix::fs::symlink(&outside, anchor.join("link")).expect("a link out");

        create_beneath(&anchor, &anchor.join("a/b/c")).expect("nested folders");
        assert!(anchor.join("a/b/c").is_dir());
        assert!(create_beneath(&anchor, &anchor.join("link/x")).is_err());
        assert!(create_beneath(&anchor, &anchor.join("file/x")).is_err());
        assert!(
            !outside.join("x").exists(),
            "nothing is created through the link"
        );
        assert_eq!(fs::read(anchor.join("file")).expect("the file"), b"kept");
    }

    /// A scratch folder the root tests remove when they end, however they end.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("apz-root-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("a scratch folder");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The owner and the mode bits of `path`, not following a link.
    fn owner_and_mode(path: &Path) -> (u32, u32) {
        let meta = fs::symlink_metadata(path).expect("the path");
        (meta.uid(), meta.mode() & 0o7777)
    }

    fn require_root() {
        assert!(
            bash::runs_as_root().expect("the uid"),
            "needs root: run scripts/test-harness-root.sh"
        );
    }

    #[test]
    #[ignore = "needs root: scripts/test-harness-root.sh"]
    fn root_prepare_engine_dirs_refuses_a_link_where_the_engines_folder_goes() {
        require_root();
        let scratch = Scratch::new("beneath-root");
        let root = scratch.0.join("ws");
        let outside = scratch.0.join("outside");
        let job = root.join("app");
        fs::create_dir_all(&job).expect("the job");
        fs::create_dir_all(&outside).expect("a folder outside");
        std::os::unix::fs::symlink(&outside, root.join(".engines")).expect("a link out");

        let result = prepare_engine_dirs(&Workspace::at(root.clone()), "goose", "1.52.0", &job);
        assert!(result.is_err(), "a link where .engines goes is refused");
        assert_eq!(
            fs::read_dir(&outside).expect("the folder outside").count(),
            0,
            "nothing is created through the link"
        );
    }

    #[test]
    #[ignore = "needs root: scripts/test-harness-root.sh"]
    fn root_prepare_engine_dirs_gives_the_job_to_the_engine_and_nothing_else() {
        require_root();
        let scratch = Scratch::new("prepare");
        let root = scratch.0.join("ws");
        let outside = scratch.0.join("outside");
        let job = root.join("app");
        fs::create_dir_all(job.join("sub")).expect("the job");
        fs::create_dir_all(root.join(".plaza")).expect("the harness state");
        fs::create_dir_all(&outside).expect("a folder outside");
        fs::write(job.join("main.rs"), "fn main() {}").expect("a file");
        fs::write(outside.join("target"), "kept").expect("the link's target");
        fs::set_permissions(outside.join("target"), fs::Permissions::from_mode(0o640))
            .expect("its mode");
        std::os::unix::fs::symlink(outside.join("target"), job.join("link")).expect("a link out");
        fs::write(outside.join("shared"), "kept").expect("a file outside");
        fs::hard_link(outside.join("shared"), job.join("shared")).expect("a hard link in");

        prepare_engine_dirs(&Workspace::at(root.clone()), "goose", "1.52.0", &job)
            .expect("prepared");

        let engine = ENGINE_UID;
        assert_eq!(owner_and_mode(&job).0, engine);
        assert_eq!(owner_and_mode(&job.join("sub")).0, engine);
        assert_eq!(owner_and_mode(&job.join("main.rs")).0, engine);
        assert_eq!(
            owner_and_mode(&job.join("link")).0,
            engine,
            "the link itself"
        );
        assert_eq!(
            owner_and_mode(&outside.join("target")),
            (0, 0o640),
            "the link's target keeps its owner and mode"
        );
        assert_eq!(
            owner_and_mode(&job.join("shared")).0,
            0,
            "a hard link stays root's"
        );
        assert_eq!(owner_and_mode(&outside.join("shared")).0, 0);
        assert_eq!(owner_and_mode(&root.join(".plaza")), (0, 0o700));
        assert_eq!(
            owner_and_mode(&root.join(".engines/home/goose")),
            (engine, 0o700)
        );
    }

    #[test]
    #[ignore = "needs root: scripts/test-harness-root.sh"]
    fn root_the_walk_never_follows_a_folder_swapped_for_a_link() {
        require_root();
        let scratch = Scratch::new("race");
        let root = scratch.0.join("ws");
        let outside = scratch.0.join("outside");
        let swapped = root.join("app/a");
        fs::create_dir_all(&swapped).expect("the job's folder");
        fs::create_dir_all(&outside).expect("a folder outside");
        fs::write(outside.join("file"), "kept").expect("a file outside");
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o750)).expect("its mode");
        fs::set_permissions(outside.join("file"), fs::Permissions::from_mode(0o640))
            .expect("its mode");

        let stop = Arc::new(AtomicBool::new(false));
        let swapper = {
            let (stop, swapped, outside) = (Arc::clone(&stop), swapped.clone(), outside.clone());
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let _ = fs::remove_dir(&swapped);
                    let _ = std::os::unix::fs::symlink(&outside, &swapped);
                    let _ = fs::remove_file(&swapped);
                    let _ = fs::create_dir(&swapped);
                }
            })
        };
        let workspace = Workspace::at(root.clone());
        for _ in 0..200 {
            // A walk that loses the race may fail; either way it must not reach outside.
            let _ = prepare_engine_dirs(&workspace, "goose", "1.52.0", &root.join("app"));
        }
        stop.store(true, Ordering::Relaxed);
        swapper.join().expect("the swapper ends");
        assert_eq!(owner_and_mode(&outside), (0, 0o750));
        assert_eq!(owner_and_mode(&outside.join("file")), (0, 0o640));
    }

    #[test]
    #[ignore = "needs root: scripts/test-harness-root.sh"]
    fn root_install_in_hands_the_folder_back_to_root_and_leaves_no_engine_process() {
        require_root();
        let scratch = Scratch::new("install");
        let workspace = Workspace::at(scratch.0.clone());
        fs::create_dir_all(scratch.0.join(".engines")).expect("the engines folder");
        let dir = scratch.0.join(".engines/goose-1.52.0");
        let home = engine_home(&workspace, "goose");
        install_in(&dir, || {
            bash::execute(
                &dir,
                "/bin/sh",
                &[
                    "-c",
                    "touch made; setsid sleep 60 </dev/null >/dev/null 2>&1 &",
                ],
                20,
                bash::Privilege::Engine(home),
            )
            .map(|_| ())
        })
        .expect("installed");
        assert_eq!(owner_and_mode(&dir).0, 0);
        let (owner, mode) = owner_and_mode(&dir.join("made"));
        assert_eq!(owner, 0);
        assert_eq!(mode & 0o022, 0, "no group or other write bit");
        let left = bash::engine_pids().expect("the scan");
        assert!(left.is_empty(), "an engine process is left: {left:?}");
    }
}
