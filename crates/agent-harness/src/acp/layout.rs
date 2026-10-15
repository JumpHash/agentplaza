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

use std::fs;
use std::io;
use std::os::unix::fs::{PermissionsExt, lchown};
use std::path::{Path, PathBuf};

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
    fn id(self) -> u32 {
        match self {
            Owner::Root => 0,
            Owner::Engine => ENGINE_UID,
        }
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

/// Applies one step (a root-only operation; a missing path is skipped).
fn apply(step: &Step) -> io::Result<()> {
    if step.itself {
        return apply_entry(&step.path, step);
    }
    if step.tree {
        return apply_children(&step.path, step);
    }
    Ok(())
}

fn apply_children(dir: &Path, step: &Step) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        apply_entry(&entry?.path(), step)?;
    }
    Ok(())
}

fn apply_entry(path: &Path, step: &Step) -> io::Result<()> {
    if step.skip.iter().any(|skip| skip == path) {
        return Ok(());
    }
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    let id = step.owner.id();
    // `lchown` and no mode change on a symlink: the link itself is what changes, never its target.
    lchown(path, Some(id), Some(id))?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if meta.is_dir() {
        if let Some(mode) = step.mode {
            fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
        }
        if step.tree {
            apply_children(path, step)?;
        }
    } else if let (Owner::Root, Some(_)) = (step.owner, step.mode) {
        let mode = meta.permissions().mode() & !0o022;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
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
    bash::ensure_engine_user()?;
    let home = home_in(root, id);
    fs::create_dir_all(install_cache(&home)).map_err(describe)?;
    for step in plan(root, id, version, job) {
        apply(&step).map_err(describe)?;
    }
    Ok(())
}

/// Runs `install` while `dir` is the engine's to write, then gives it back to root: the install
/// writes into it as the engine user, so its install scripts never run as root. Root owns the
/// folder afterwards, read-only to the engine. Not root: runs `install` as it is.
pub(crate) fn install_in<T>(
    dir: &Path,
    install: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    if !bash::runs_as_root()? {
        return install();
    }
    fs::create_dir_all(dir).map_err(describe)?;
    apply(&Step::engine(dir, None)).map_err(describe)?;
    let result = install();
    // The folder goes back to root even when the install failed.
    let closed = apply(&Step::root_tree(dir, 0o755)).map_err(describe);
    let value = result?;
    closed?;
    Ok(value)
}

/// Hands `path` and what is under it to the engine (root only): the files the harness writes for
/// the engine (an ACP write, a config) must stay editable by it.
pub(crate) fn give_to_engine(path: &Path) -> Result<(), String> {
    if !bash::runs_as_root()? {
        return Ok(());
    }
    apply(&Step::engine(path, None)).map_err(describe)
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
    use super::*;

    const ROOT: &str = "/workspace";

    fn step_at<'a>(steps: &'a [Step], path: &str) -> &'a Step {
        steps
            .iter()
            .find(|step| step.path == Path::new(path))
            .unwrap_or_else(|| panic!("a step for {path}"))
    }

    #[test]
    fn the_job_root_hands_the_engine_its_entries_but_never_the_root_or_the_harness_folders() {
        let steps = plan(Path::new(ROOT), "goose", "1.52.0", Path::new(ROOT));
        let job = steps
            .iter()
            .find(|step| step.owner == Owner::Engine && step.tree && step.path == Path::new(ROOT))
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

    #[test]
    fn a_subfolder_job_is_the_engines_alone_with_nothing_skipped() {
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

    #[test]
    fn the_harness_state_and_the_installed_engines_stay_root_read_only() {
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

    #[test]
    fn each_engine_has_a_private_home_of_its_own() {
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
}
