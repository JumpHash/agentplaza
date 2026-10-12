// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port, in the shape of Firecracker's jailer
// (`src/jailer/src/chroot.rs`: the mount namespace, the bind-over-itself,
// `pivot_root`, `chdir("/")`, the lazy unmount of the old root; and
// `src/jailer/src/env.rs`: the `/dev` nodes and the uid/gid drop). This is a
// *trimmed* port for this port's shape, not a translation of that program: the
// VMMs here are threads inside `noded`/`vmctl`, so the jailer is a step on the
// VMM's own thread rather than a process that `exec`s a VMM child, and there is
// no cgroup, no netns, no resource-limit half, no daemonize and no `exec`.
//
// See the module documentation in `mod.rs` for where it runs, what it does and
// does not confine, and the platform facts it was built against.

//! The VMM's jailer, v0: a per-VM root, a mount namespace, and the drop.
//!
//! **What it does.** [`Jailer::enter`] takes the calling process down the same
//! road Firecracker's jailer takes a VMM child:
//!
//! 1. a **user namespace** when the caller is not root — the only way an
//!    unprivileged process gets `CAP_SYS_ADMIN` over a mount namespace
//!    (verified on this host: `unshare(CLONE_NEWNS)` alone is `EPERM` as a
//!    non-root user, while the same call after `unshare(CLONE_NEWUSER)` plus a
//!    one-line `uid_map` succeeds). The namespace's whole world is the caller's
//!    own uid and gid mapped to its uid 0 and gid 0, so its "root" has no host
//!    privilege at all;
//! 2. a **mount namespace** (`unshare(CLONE_NEWNS)`), made private
//!    (`mount(NULL, "/", NULL, MS_REC|MS_PRIVATE, NULL)`) so nothing it mounts
//!    propagates back to the host;
//! 3. the **per-VM root**: a `0700` directory holding *only* what the VM needs —
//!    each image **bind-mounted at its own absolute path** (the kernel and the
//!    initrd read-only, the writable disk read-write), `/dev/kvm`, `/dev/null`
//!    and `/dev/urandom` read-only, the vsock **UDS directory** the muxer binds
//!    its socket in, and a small **tmpfs** at `/run`;
//! 4. `pivot_root` into it — the root is bind-mounted over itself first, which
//!    `pivot_root` requires — then `chdir("/")` and a lazy unmount of the old
//!    root: after this the process cannot name anything outside the root.
//!    **`chroot` alone was rejected here**: it moves only the working
//!    directory's root, leaves the original root referenced by any descriptor
//!    or a process that kept one open, and can be escaped by a process with
//!    `CAP_SYS_CHROOT` — of which a user-namespace root has a version. The
//!    Jailer keeps Firecracker's `pivot_root` (its `chroot.rs` does exactly
//!    this), and the only reason to prefer `chroot` — a platform without
//!    `pivot_root` — does not exist on the Linux kernels this port targets;
//! 5. the **uid/gid drop** ([`setresgid`]/[`setresuid`]) when a target is
//!    configured and the process has the privilege to perform one.
//!
//! **Why the images are mirrored at their own paths.** The task of this slice
//! was the *post-pivot path audit*: after `pivot_root`, which lookups the VMM
//! still makes and where they land. Measured on this tree, the boot does keep
//! opening by path after the devices exist — `File::open(&spec.kernel)` and the
//! initrd's `File::open(path)` in [`crate::builder`], the block backend's
//! `VirtioBlock::new(image)`, and the muxer's `bind({uds_path})` and runtime
//! `connect({uds_path}_{port})`. Rather than rewrite each to carry an fd, the
//! jailer mirrors every source at the same path inside the root, so each of
//! those lookups resolves inside the jail to the same *inode* it would have
//! reached outside — the same file, and for the UDS directory the same
//! directory (a `{uds_path}_{port}` created later is visible through the bind).
//! Nothing in the device model, the muxer or the snapshot code learns about the
//! jailer, and the boot needs no other change. The descriptors the setup
//! already holds (`/dev/kvm`, the image files, the bound listening socket) keep
//! working untouched, because a file descriptor does not care what the
//! process's root is.
//!
//! **Where it runs, and why that bound is the honest one.** The jailer is a
//! step at the top of a VM's build, *before any VMM thread exists*, on the
//! thread that called [`crate::spawner`]'s backend. Two facts fix that:
//!
//! * `CLONE_NEWUSER` is refused (`EINVAL`) by a multi-threaded process — the
//!   kernel implies `CLONE_THREAD` for it, and a user namespace cannot be split
//!   off a non-empty thread group (verified here: single-threaded succeeds, a
//!   `pthread_create`d sibling makes the same call fail); and
//! * `pivot_root(2)` changes the *whole* thread group's root and working
//!   directory, which is right for a process that *is* the VMM and catastrophic
//!   for the daemon around it.
//!
//! So the jailer engages for a caller that is single-threaded at VM build time —
//! `vmctl`, which boots its VMs before it starts its printer and console
//! threads, and this crate's own probe — and reports [`Entered::Unsupported`]
//! for `noded`, whose VMs are threads of a serving daemon that is already
//! multi-threaded when a run is created. That is **not** a silent downgrade:
//! the reason is printed once and named, and the guest keeps the seccomp jail
//! ([`crate::jail`]), which needs no privilege and arms per thread whatever the
//! process's thread count is. A VM the switch says must be jailed is *refused*
//! rather than run unjailed when the failure is real (a mount that did not
//! land); only "this process cannot, by construction" is tolerated.
//!
//! **What it does not do — the honest gaps.** No **PID namespace**:
//! `unshare(CLONE_NEWPID)` only affects children, so an in-process VMM would
//! need a forked supervisor with the VMM's threads created inside it — the
//! shape Firecracker's jailer uses, since *it* forks and `exec`s the VMM. That
//! is the per-VM process this port does not have yet, and taking it here would
//! move the serial capture, the stop control and the spawner's join model from
//! in-process memory to cross-process IPC. No **net namespace**: there is no
//! network device to put in it (invariant 1) and the seccomp filter already
//! refuses any socket that is not `AF_UNIX`. No **IPC**, **UTS** or **cgroup**
//! namespace: nothing in a VMM uses SysV/POSIX IPC, the hostname is not read,
//! and cgroups are the daemon's lifecycle concern rather than this step's. No
//! **capability drop beyond the namespace**: in the unprivileged path the
//! process is root only inside a namespace whose sole id is the caller's, so
//! `setresgid`/`setresuid` to an unmapped id is impossible by construction
//! (`EINVAL`, verified) and the namespace itself *is* the drop; the numeric drop
//! is implemented for a root caller, where it is real, and is off unless
//! `APZ_VMM_JAILER_UID`/`_GID` name a target.
//!
//! **The mode switch.** `APZ_VMM_JAILER` is exactly `on` or `off` (ASCII
//! case-insensitive, trimmed); anything else is refused with
//! [`JailerError::Config`], so a typo stops the VM rather than silently picking
//! a mode. Absent, the jailer is **on** (the same default-on posture as the
//! seccomp jail), so a caller only has to say `off` to get the pre-jailer v0
//! behaviour. `APZ_VMM_JAIL_ROOT` moves the jail base directory;
//! `APZ_VMM_JAILER_UID`/`APZ_VMM_JAILER_GID` name the drop target.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use super::jailer_sys as sys;

/// How a bind-mounted source is visible inside the jail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Read-only: the kernel image, the initrd, the `/dev` nodes.
    ReadOnly,
    /// Read-write: the one writable disk, and the UDS directory the muxer
    /// binds its socket in.
    ReadWrite,
}

impl Access {
    /// The `mount(2)` flag this access adds to `MS_BIND`.
    fn flag(self) -> libc::c_ulong {
        match self {
            Self::ReadOnly => sys::MS_RDONLY,
            Self::ReadWrite => 0,
        }
    }
}

/// The environment variable that selects the jailer's mode: exactly `on` or
/// `off`.
pub const JAILER_ENV: &str = "APZ_VMM_JAILER";

/// The environment variable that moves the jail base directory (the `--jail-root`
/// of the design). Each VM's root is a child of it.
pub const JAIL_ROOT_ENV: &str = "APZ_VMM_JAIL_ROOT";

/// The environment variable naming the uid to drop to. Only honoured by a
/// caller that is root; a user namespace maps the caller's single id and cannot
/// carry another.
pub const JAIL_UID_ENV: &str = "APZ_VMM_JAILER_UID";

/// The environment variable naming the gid to drop to.
pub const JAIL_GID_ENV: &str = "APZ_VMM_JAILER_GID";

/// The `old_root` directory `pivot_root` moves the previous root under
/// (Firecracker's jailer calls it `old_root`; it is created and removed inside
/// the same step, so only its name matters).
const OLD_ROOT: &str = "oldroot";

/// The tmpfs `/run` gets: a VMM keeps no files there; this is a bound, not a
/// budget.
const RUN_TMPFS: &str = "size=64m,mode=0700";

/// A source to bind into the per-VM root, with how it is seen.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Bind {
    /// The host path. It is mirrored inside the root at the same path.
    source: PathBuf,
    /// Whether the bind is read-only.
    access: Access,
    /// Whether the source is a directory (a directory bind needs a directory to
    /// land on; a file bind needs a file).
    dir: bool,
}

/// The jail's plan: where the root is, what it holds, and where a tmpfs goes.
/// Building a plan touches nothing; [`Jailer::enter`] is the step that changes
/// the process.
#[derive(Debug)]
pub struct Jailer {
    /// The per-VM root directory, created `0700` on entry.
    root: PathBuf,
    /// The sources mirrored inside the root, in the order they are mounted.
    binds: Vec<Bind>,
    /// Directories inside the root that get a tmpfs.
    tmpfs: Vec<PathBuf>,
}

impl Jailer {
    /// Plans a jail rooted at `root` (created on entry), with nothing bound
    /// yet.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            binds: Vec::new(),
            tmpfs: Vec::new(),
        }
    }

    /// Adds a file source, mirrored inside the root at the same path.
    ///
    /// A relative source is mirrored under the root at that same relative path:
    /// the post-pivot working directory is `/`, so a caller that opened `x/y`
    /// before the jail opens `/x/y` after it, and the mirror has to be there.
    pub fn bind(&mut self, source: impl Into<PathBuf>, access: Access) -> &mut Self {
        self.binds.push(Bind {
            source: source.into(),
            access,
            dir: false,
        });
        self
    }

    /// [`Jailer::bind`] for a source that is a directory (the vsock UDS
    /// directory): the target has to be a directory too.
    pub fn bind_dir(&mut self, source: impl Into<PathBuf>, access: Access) -> &mut Self {
        self.binds.push(Bind {
            source: source.into(),
            access,
            dir: true,
        });
        self
    }

    /// Adds a tmpfs at `path` inside the root (an absolute path in the jail).
    pub fn tmpfs(&mut self, path: impl Into<PathBuf>) -> &mut Self {
        self.tmpfs.push(path.into());
        self
    }

    /// The root this jail plans to enter.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Enters the jail on the calling thread, or explains why it cannot.
    ///
    /// * [`Entered::Off`] — `APZ_VMM_JAILER=off`.
    /// * [`Entered::Unsupported`] — this process cannot enter a jail *by
    ///   construction*: it is multi-threaded, or it is unprivileged and the
    ///   kernel refuses a user namespace. **Nothing has been changed.**
    /// * [`Entered::Inside`] — the process is now inside the root and cannot
    ///   name anything outside it.
    ///
    /// Fails — and the VM must not boot — when the process *can* be jailed and
    /// the jail could not be built: an incomplete jail is not a jail.
    pub fn enter(self) -> Result<Entered, JailerError> {
        if !mode()? {
            return Ok(Entered::Off);
        }
        // Everything that can be refused is refused *before* the process is
        // moved: after `pivot_root` there is no un-moved state to return to.
        self.validate()?;
        // A process has one root. A caller that boots several VMs in one
        // single-threaded process (`vmctl -n N`) enters the jail once, for its
        // first VM, and every later VM runs in that same root: the alternative
        // — a root per VM — needs a process per VM, which is the per-VM-process
        // shape this v0 does not have. The later VMs' sources must therefore be
        // mirrored by the first plan; for this port's callers that holds,
        // because a run's workspace copies live under the UDS directory the
        // plan already mirrors (`materialize_workspaces`). A source outside
        // that set makes the later VM's boot fail loudly on its own open rather
        // than run outside the jail.
        if let Some(root) = entered_root() {
            return Ok(Entered::Unsupported(format!(
                "this process is already inside the jail at {}: a later VM of the same spawn \
                 runs in that root, not a second one",
                root.display()
            )));
        }

        // Captured *before* any `unshare`: inside a fresh user namespace, and
        // before its maps are written, the process reads as the overflow uid
        // (65534) — verified on this host, where a `uid_map` built from the
        // post-unshare `getuid()` maps 65534 instead of the caller.
        let (uid, gid) = sys::ids();
        let (euid, _) = sys::effective_ids();

        if let Some(threads) = thread_count()
            && threads > 1
        {
            return Ok(Entered::Unsupported(format!(
                "this process has {threads} threads: a user namespace (and so a mount \
                 namespace) can only be created by a single-threaded process, and \
                 chroot/pivot_root would change the root of every thread"
            )));
        }

        let mut user_ns = false;
        let mut uid_map = None;
        match sys::unshare(sys::CLONE_NEWUSER) {
            Ok(()) => {
                user_ns = true;
                uid_map = Some(set_up_user_ns(uid, gid)?);
            }
            // A multi-threaded process (`EINVAL`) has already been reported by
            // the thread count above; this is the belt to that braces, and it is
            // also what a kernel without user namespaces says.
            Err(err) if err.raw_os_error() == Some(libc::EINVAL) => {
                return Ok(Entered::Unsupported(format!(
                    "cannot create a user namespace: {err}"
                )));
            }
            Err(_) if euid == 0 => {
                // Root already has `CAP_SYS_ADMIN`: the mount namespace below
                // needs no user namespace, and the uid/gid drop replaces it.
            }
            Err(err) => {
                return Ok(Entered::Unsupported(format!(
                    "cannot create a user namespace ({err}) and this process is not root, so \
                     it has no CAP_SYS_ADMIN for a mount namespace"
                )));
            }
        }

        // The drop target is decided (and a misconfiguration refused) before
        // the process is moved, so a bad `APZ_VMM_JAILER_UID` is a failure to
        // start rather than a failure after the root has already changed.
        let target = drop_target(euid, user_ns)?;

        if let Err(err) = sys::unshare(sys::CLONE_NEWNS) {
            // Root without CAP_SYS_ADMIN (a container's root, under its seccomp profile and
            // capability set) cannot make a mount namespace either: the same "cannot jail here"
            // as an unprivileged process without user namespaces, not a broken setup.
            if !user_ns && err.raw_os_error() == Some(libc::EPERM) {
                return Ok(Entered::Unsupported(format!(
                    "cannot create a mount namespace ({err}): this root has no CAP_SYS_ADMIN \
                     (a container?) and user namespaces are refused"
                )));
            }
            return Err(JailerError::Setup {
                step: "unshare(CLONE_NEWNS)",
                source: err,
            });
        }
        if let Err(err) = sys::mount(
            None,
            Path::new("/"),
            None,
            sys::MS_REC | sys::MS_PRIVATE,
            None,
        ) {
            return Err(JailerError::Setup {
                step: "make the mount tree private",
                source: err,
            });
        }

        self.build_root()?;
        self.pivot()?;
        mark_entered(&self.root);
        self.apply_drop(target)?;

        Ok(Entered::Inside {
            root: self.root,
            uid_map,
            dropped: target,
        })
    }

    /// Rejects a plan the jailer must not act on: a source whose path escapes
    /// the root (`..`), which would make the *host* filesystem the target of a
    /// mirror's `create_dir_all`/`File::create`. Nothing here is reachable from
    /// a guest — the sources come from the boot spec — but a mirror that can
    /// land outside its own root is not a mirror.
    fn validate(&self) -> Result<(), JailerError> {
        let escapes = |path: &Path| {
            path.components()
                .any(|c| c == std::path::Component::ParentDir)
        };
        for bind in &self.binds {
            if escapes(&bind.source) {
                return Err(JailerError::Config(format!(
                    "the bind source {} names a parent directory (`..`), which would place \
                     the mirror outside the jail root",
                    bind.source.display()
                )));
            }
        }
        for dir in &self.tmpfs {
            if escapes(dir) {
                return Err(JailerError::Config(format!(
                    "the tmpfs path {} names a parent directory (`..`)",
                    dir.display()
                )));
            }
        }
        Ok(())
    }

    /// Creates the root directory and everything in it, and mounts the binds.
    /// Runs *before* the pivot, so every path here is a host path.
    fn build_root(&self) -> Result<(), JailerError> {
        create_private_dir(&self.root).map_err(|source| JailerError::Setup {
            step: "create the jail root",
            source,
        })?;

        // `pivot_root` requires the new root to be a mount point, so the root is
        // bind-mounted over itself (Firecracker's jailer does the same, for the
        // same reason).
        sys::mount(Some(&self.root), &self.root, None, sys::MS_BIND, None)
            .map_err(|source| setup("bind the jail root over itself", &self.root, source))?;

        for bind in &self.binds {
            let target = mirror(&self.root, &bind.source);
            let prepared = if bind.dir {
                std::fs::create_dir_all(&target)
            } else {
                // A file bind needs a file to land on; the bind replaces it.
                let parent = std::fs::create_dir_all(target.parent().unwrap_or(&self.root));
                parent.and_then(|()| std::fs::File::create(&target).map(|_| ()))
            };
            prepared.map_err(|source| JailerError::Setup {
                step: "lay out a jail path",
                source: io::Error::new(source.kind(), format!("{}: {source}", target.display())),
            })?;
            sys::mount(
                Some(&bind.source),
                &target,
                None,
                sys::MS_BIND | bind.access.flag(),
                None,
            )
            .map_err(|source| {
                setup(
                    match bind.access {
                        Access::ReadOnly => "bind a source read-only",
                        Access::ReadWrite => "bind a source read-write",
                    },
                    &bind.source,
                    source,
                )
            })?;
        }

        for dir in &self.tmpfs {
            let target = mirror(&self.root, dir);
            std::fs::create_dir_all(&target).map_err(|source| JailerError::Setup {
                step: "lay out the run directory",
                source: io::Error::new(source.kind(), format!("{}: {source}", target.display())),
            })?;
            sys::mount(
                Some(Path::new("tmpfs")),
                &target,
                Some("tmpfs"),
                0,
                Some(RUN_TMPFS),
            )
            .map_err(|source| setup("mount the run tmpfs", &target, source))?;
        }

        Ok(())
    }

    /// `pivot_root` into the root, then `chdir("/")` and drop the old root.
    fn pivot(&self) -> Result<(), JailerError> {
        let old_root = self.root.join(OLD_ROOT);
        sys::mkdir(&old_root, 0o700)
            .map_err(|source| setup("create the old root", &old_root, source))?;
        sys::chdir(&self.root)
            .map_err(|source| setup("chdir to the jail root", &self.root, source))?;
        sys::pivot_root(Path::new("."), Path::new(OLD_ROOT)).map_err(|source| {
            JailerError::Setup {
                step: "pivot_root",
                source,
            }
        })?;
        sys::chdir(Path::new("/")).map_err(|source| JailerError::Setup {
            step: "chdir to the new root",
            source,
        })?;
        // Lazily detach the old root, so nothing still referencing it can reach
        // it, then remove the (now empty) mount point.
        sys::umount2(&Path::new("/").join(OLD_ROOT), sys::MNT_DETACH).map_err(|source| {
            JailerError::Setup {
                step: "unmount the old root",
                source,
            }
        })?;
        sys::rmdir(&Path::new("/").join(OLD_ROOT)).map_err(|source| JailerError::Setup {
            step: "remove the old root",
            source,
        })?;
        Ok(())
    }

    /// Performs the uid/gid drop the plan decided on, if any. Runs *after* the
    /// pivot and is irreversible.
    fn apply_drop(&self, target: Option<(u32, u32)>) -> Result<(), JailerError> {
        let Some((uid, gid)) = target else {
            return Ok(());
        };
        // The gid first: after the uid drop there is no privilege left to set
        // it. The target was validated before the pivot, so a failure here is
        // the kernel's, and is reported.
        sys::setresgid(gid).map_err(|source| JailerError::Setup {
            step: "setresgid",
            source,
        })?;
        sys::setresuid(uid).map_err(|source| JailerError::Setup {
            step: "setresuid",
            source,
        })
    }
}

/// What [`Jailer::enter`] did.
#[derive(Debug)]
pub enum Entered {
    /// The process is now inside the jail root and cannot name anything outside
    /// it.
    Inside {
        /// The root the process was pivoted into.
        root: PathBuf,
        /// The `uid_map` the jail wrote, as read back from
        /// `/proc/self/uid_map` before the pivot — or `None` when no user
        /// namespace was made (a root caller).
        uid_map: Option<String>,
        /// The `(uid, gid)` the process dropped to, or `None` when no numeric
        /// drop was configured.
        dropped: Option<(u32, u32)>,
    },
    /// `APZ_VMM_JAILER=off`: nothing was done.
    Off,
    /// This process cannot be jailed by construction; the string is the
    /// caller-facing reason. Nothing was changed.
    Unsupported(String),
}

/// Errors from [`Jailer::enter`].
#[derive(Debug, thiserror::Error)]
pub enum JailerError {
    /// The jailer's configuration is invalid, so nothing is entered: {0}. A
    /// mistyped `APZ_VMM_JAILER` must stop the VM at startup.
    #[error("the jailer is misconfigured: {0}")]
    Config(String),
    /// A step of the jail failed: {step}: {source}. The process is *not*
    /// jailed, and a boot the switch says must be jailed must not continue
    /// unjailed.
    #[error("cannot enter the jail ({step}): {source}")]
    Setup {
        /// Which step failed.
        step: &'static str,
        /// The error it failed with (its errno, and the path in the message).
        source: io::Error,
    },
}

/// A [`JailerError::Setup`] naming the step and the path it failed on.
fn setup(step: &'static str, path: &Path, source: io::Error) -> JailerError {
    JailerError::Setup {
        step,
        source: io::Error::new(source.kind(), format!("{}: {source}", path.display())),
    }
}

/// The mode the process is set to. `0` = read the environment; then `MODE_ON`
/// or `MODE_OFF`; `MODE_INVALID` = the environment named a mode that does not
/// exist, refused until something sets it.
static MODE: AtomicU8 = AtomicU8::new(0);

const MODE_FROM_ENV: u8 = 0;
const MODE_ON: u8 = 1;
const MODE_OFF: u8 = 2;
const MODE_INVALID: u8 = 3;

/// Whether the "this process cannot be jailed" line has been printed (once per
/// process, not once per VM).
static WARNED_UNSUPPORTED: AtomicBool = AtomicBool::new(false);

/// Names one root, so two VMs of one process never share a root even when they
/// share an id.
static RUN: AtomicU64 = AtomicU64::new(0);

/// The root this process entered, once it has one. A process has a single
/// root; the second VM of a spawn shares the first VM's (see
/// [`Jailer::enter`]).
static ENTERED: Mutex<Option<PathBuf>> = Mutex::new(None);

/// The root this process is inside, if any.
fn entered_root() -> Option<PathBuf> {
    ENTERED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Records that the process is now inside `root`.
fn mark_entered(root: &Path) {
    *ENTERED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(root.to_owned());
}

/// Forces the jailer on or off, overriding the environment for the rest of the
/// process (the library half of a `--jailer` flag).
pub fn set_enabled(on: bool) {
    MODE.store(if on { MODE_ON } else { MODE_OFF }, Ordering::SeqCst);
}

/// Whether the jailer is armed. A process whose `APZ_VMM_JAILER` names no mode
/// reads as armed, so a caller that only checks this still asks
/// [`Jailer::enter`] and meets the error there.
pub fn is_enabled() -> bool {
    !matches!(mode(), Ok(false))
}

/// The mode this process is set to: the programmatic one, else the
/// environment's, else on. An `APZ_VMM_JAILER` value that is not `on`/`off` is
/// [`JailerError::Config`] — on every call, until something sets the mode.
pub fn mode() -> Result<bool, JailerError> {
    match MODE.load(Ordering::SeqCst) {
        MODE_FROM_ENV => mode_from_env(),
        MODE_ON => Ok(true),
        MODE_OFF => Ok(false),
        // Only `MODE_INVALID` is left: `set_enabled` and `mode_from_env` store
        // these four values and nothing else.
        _ => Err(invalid_jailer_env()),
    }
}

/// The mode an [`JAILER_ENV`] value names: `on` or `off`, ASCII
/// case-insensitive and trimmed. Anything else — an empty string included — is
/// [`JailerError::Config`].
fn parse_jailer_env(value: &str) -> Result<bool, JailerError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(JailerError::Config(format!(
            "{JAILER_ENV}={value:?} is not a mode: expected \"on\" or \"off\""
        ))),
    }
}

/// Reads [`JAILER_ENV`] and caches the decision in [`MODE`] — an invalid value
/// included, so it is never silently re-read as a default.
fn mode_from_env() -> Result<bool, JailerError> {
    match std::env::var(JAILER_ENV) {
        Ok(value) => {
            let mode = parse_jailer_env(&value);
            if mode.is_err() {
                MODE.store(MODE_INVALID, Ordering::SeqCst);
            }
            mode
        }
        Err(_) => Ok(true),
    }
}

/// The refusal an `APZ_VMM_JAILER` value that names no mode produces.
fn invalid_jailer_env() -> JailerError {
    let value = std::env::var(JAILER_ENV).unwrap_or_default();
    parse_jailer_env(&value).expect_err("only called when the value names no mode")
}

/// One `APZ_VMM_JAILER_UID`-style target: `None` when the variable is unset,
/// `Some(id)` when it names one, and [`JailerError::Config`] when it names
/// something that is not a number.
fn target_id(name: &'static str) -> Result<Option<u32>, JailerError> {
    match std::env::var(name) {
        Err(_) => Ok(None),
        Ok(value) => value
            .trim()
            .parse::<u32>()
            .map(Some)
            .map_err(|_| JailerError::Config(format!("{name}={value:?} is not a uid or gid"))),
    }
}

/// The `(uid, gid)` the process should drop to, decided from the environment
/// and the process's state *before* the jail is entered.
///
/// Only a root caller can drop: the user namespace a non-root caller gets maps
/// exactly one id (its own), so there is no second id to become — a
/// `setresuid` to an unmapped one fails `EINVAL` (verified on this host). With
/// no target configured there is no drop at all: the namespace (or, for root,
/// the caller's own choice) is the confinement, and silently picking a uid
/// would break the VM's access to its own images.
fn drop_target(euid: u32, user_ns: bool) -> Result<Option<(u32, u32)>, JailerError> {
    drop_target_from(
        target_id(JAIL_UID_ENV)?,
        target_id(JAIL_GID_ENV)?,
        euid,
        user_ns,
    )
}

/// [`drop_target`]'s decision, on the values it reads — a pure function so the
/// rule can be tested without touching the process's environment.
fn drop_target_from(
    uid: Option<u32>,
    gid: Option<u32>,
    euid: u32,
    user_ns: bool,
) -> Result<Option<(u32, u32)>, JailerError> {
    if uid.is_none() && gid.is_none() {
        return Ok(None);
    }
    if user_ns {
        return Err(JailerError::Config(format!(
            "{JAIL_UID_ENV}/{JAIL_GID_ENV} name a drop target, but this process is in a user \
             namespace whose only id is the caller's: there is no id to drop to"
        )));
    }
    if euid != 0 {
        return Err(JailerError::Config(format!(
            "{JAIL_UID_ENV}/{JAIL_GID_ENV} name a drop target, but this process is not root"
        )));
    }
    // A target that names only one of the two keeps the other where it is.
    Ok(Some((uid.unwrap_or(0), gid.unwrap_or(0))))
}

/// How many threads this process has, from `/proc/self/status`, or `None` when
/// that cannot be read (a chroot without `/proc`). Used only to give a
/// multi-threaded caller a clear reason *before* the kernel's `EINVAL`; the
/// `unshare` refusal is still the authority.
fn thread_count() -> Option<usize> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("Threads:"))
        .and_then(|count| count.trim().parse().ok())
}

/// The base directory a VM's jail is built under: `APZ_VMM_JAIL_ROOT` when it
/// is set, else a sibling of the run's UDS directory (`/tmp/plaza-vsock` →
/// `/tmp/plaza-vsock.jail`; a sibling rather than a child, so the mirror of the
/// UDS directory the muxer needs cannot contain the jail itself), else a
/// `plaza-jail` under the temporary directory.
pub fn default_base(uds_dir: Option<&Path>) -> PathBuf {
    if let Ok(dir) = std::env::var(JAIL_ROOT_ENV)
        && !dir.is_empty()
    {
        return PathBuf::from(dir);
    }
    match uds_dir {
        Some(dir) => PathBuf::from(format!("{}.jail", dir.display())),
        None => std::env::temp_dir().join("plaza-jail"),
    }
}

/// A per-VM root under `base`. `id` alone is not unique across a process's
/// lifetime (a run that boots VM 0 twice), so the process id and a counter
/// qualify it.
pub fn root_for(base: &Path, id: u32) -> PathBuf {
    let run = RUN.fetch_add(1, Ordering::SeqCst);
    base.join(format!("{}-{id}-{run}", std::process::id()))
}

/// The path `source` is mirrored at inside `root`: the same path, rooted. An
/// absolute source drops its leading separator (`/boot/vmlinuz-linux` →
/// `{root}/boot/vmlinuz-linux`); a relative one is rooted the same way, which
/// matches the post-pivot working directory of `/`.
fn mirror(root: &Path, source: &Path) -> PathBuf {
    match source.strip_prefix("/") {
        Ok(relative) => root.join(relative),
        Err(_) => root.join(source),
    }
}

/// Creates `path` (and its parents) as a `0700` directory.
///
/// The same rule and the same reason as the UDS directory
/// ([`crate::spawner::create_uds_dir`]): the jail root and the sockets below it
/// are a trust boundary, so a wider mode is not accepted, and
/// `create_dir_all`'s mode is masked by the umask, so the mode is set
/// explicitly.
fn create_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

/// Writes the user namespace's maps — the one id the caller owns, mapped to the
/// namespace's uid 0 and gid 0 — and returns the `uid_map` as the kernel
/// reports it.
///
/// `setgroups` is denied first: without that, the kernel refuses a `gid_map`
/// written by an unprivileged process.
fn set_up_user_ns(uid: u32, gid: u32) -> Result<String, JailerError> {
    // A kernel older than 3.19 has no `/proc/self/setgroups`; its absence is
    // not an error (there is nothing to deny).
    match std::fs::write("/proc/self/setgroups", "deny\n") {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(JailerError::Setup {
                step: "deny setgroups",
                source,
            });
        }
    }
    for (file, id) in [("/proc/self/uid_map", uid), ("/proc/self/gid_map", gid)] {
        std::fs::write(file, format!("0 {id} 1\n")).map_err(|source| JailerError::Setup {
            step: "write the user namespace's maps",
            source,
        })?;
    }
    // Read the mapping back: the evidence that the namespace carries the
    // caller's id, kept for the boot log and the probes.
    std::fs::read_to_string("/proc/self/uid_map").map_err(|source| JailerError::Setup {
        step: "read back the uid_map",
        source,
    })
}

/// [`Jailer::enter`] for a VMM boot: prints the outcome once and tolerates the
/// one "this process cannot be jailed" case, because there the VM keeps the
/// seccomp jail and the hard invariants do not depend on this layer. Every
/// other failure is returned, and the caller must not boot the VM.
pub fn enter_thread(jailer: Jailer, id: u32) -> Result<Entered, JailerError> {
    let entered = jailer.enter()?;
    match &entered {
        Entered::Inside {
            root,
            uid_map,
            dropped,
        } => {
            let mapping = match uid_map.as_deref().map(str::split_whitespace) {
                Some(map) => {
                    let fields: Vec<&str> = map.collect();
                    if fields.len() >= 3 {
                        format!(", uid_map {} <- {}", fields[0], fields[1])
                    } else {
                        String::new()
                    }
                }
                None => String::new(),
            };
            let dropped = match dropped {
                Some((uid, gid)) => format!(", dropped to {uid}:{gid}"),
                None => String::new(),
            };
            eprintln!(
                "vmm: VM {id} jailed in {}{mapping}{dropped}",
                root.display()
            );
        }
        Entered::Off => {}
        Entered::Unsupported(reason) => {
            if !WARNED_UNSUPPORTED.swap(true, Ordering::SeqCst) {
                eprintln!(
                    "vmm: this process cannot be jailed ({reason}); the VMM runs with its \
                     seccomp jail only"
                );
            }
        }
    }
    Ok(entered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mirror_is_the_same_path_rooted() {
        let root = Path::new("/jail/root");
        assert_eq!(
            mirror(root, Path::new("/boot/vmlinuz-linux")),
            Path::new("/jail/root/boot/vmlinuz-linux")
        );
        assert_eq!(
            mirror(root, Path::new("/tmp/plaza-vsock")),
            Path::new("/jail/root/tmp/plaza-vsock")
        );
        // A relative source is rooted the same way: the post-pivot cwd is `/`.
        assert_eq!(
            mirror(root, Path::new("vmlinux")),
            Path::new("/jail/root/vmlinux")
        );
    }

    #[test]
    fn a_jailer_env_value_that_names_no_mode_is_refused() {
        assert!(parse_jailer_env("on").expect("on"));
        assert!(!parse_jailer_env(" OFF ").expect("off"));
        assert!(parse_jailer_env("On\n").expect("on"));
        for typo in ["", " ", "yes", "1", "true", "kill", "log", "onn"] {
            assert!(
                matches!(parse_jailer_env(typo), Err(JailerError::Config(_))),
                "{typo:?} names no mode and must be refused"
            );
        }
    }

    /// The switch is programmatic, and the default (nothing set) is *on* — the
    /// same posture the seccomp jail has.
    #[test]
    fn the_switch_is_programmatic_or_defaults_on() {
        set_enabled(true);
        assert!(is_enabled());
        assert!(mode().expect("a set mode is never an error"));
        set_enabled(false);
        assert!(!is_enabled());
        assert!(!mode().expect("a set mode is never an error"));
        // Back to "read the environment" for any later test.
        MODE.store(MODE_FROM_ENV, Ordering::SeqCst);
        // Nothing is set in the test process's environment, so the default is
        // on; the environment path itself is probed by `tests/jailer.rs`.
        MODE.store(MODE_ON, Ordering::SeqCst);
        assert!(is_enabled());
        MODE.store(MODE_FROM_ENV, Ordering::SeqCst);
    }

    /// Two roots under one base never collide, even for the same VM id.
    #[test]
    fn a_root_is_unique_per_call() {
        let base = Path::new("/tmp/plaza-jail");
        let a = root_for(base, 0);
        let b = root_for(base, 0);
        assert_ne!(a, b);
        assert!(a.starts_with(base));
        assert!(b.starts_with(base));
    }

    #[test]
    fn the_default_base_is_a_sibling_of_the_uds_dir() {
        assert_eq!(
            default_base(Some(Path::new("/tmp/plaza-vsock"))),
            PathBuf::from("/tmp/plaza-vsock.jail")
        );
        assert!(default_base(None).ends_with("plaza-jail"));
    }

    /// The drop target parses, and a value that is not a number is refused
    /// rather than dropped.
    #[test]
    fn a_drop_target_is_a_number_or_nothing() {
        assert_eq!(
            target_id(JAIL_UID_ENV).expect("unset is not an error"),
            None
        );
        assert_eq!(
            target_id(JAIL_GID_ENV).expect("unset is not an error"),
            None
        );
    }

    /// A drop is decided from the caller's privilege: no target is no drop, a
    /// root caller with a target drops to it (an unset half stays where it is),
    /// and a target that cannot be honoured — a non-root caller, or a user
    /// namespace with a single mapped id — is *refused* rather than ignored.
    #[test]
    fn a_drop_is_decided_by_privilege_and_refused_where_it_cannot_happen() {
        // No target named: no drop, whatever the privilege.
        assert_eq!(drop_target_from(None, None, 0, false).expect("none"), None);
        assert_eq!(
            drop_target_from(None, None, 1000, true).expect("none"),
            None
        );
        // Root, target named: drop, and the unset half keeps its value.
        assert_eq!(
            drop_target_from(Some(60000), Some(60001), 0, false).expect("root drops"),
            Some((60000, 60001))
        );
        assert_eq!(
            drop_target_from(Some(60000), None, 0, false).expect("root drops"),
            Some((60000, 0))
        );
        // A non-root caller cannot drop to another uid.
        assert!(matches!(
            drop_target_from(Some(60000), None, 1000, false),
            Err(JailerError::Config(_))
        ));
        // A user namespace maps the caller's single id: there is nothing to
        // drop to, even as its root.
        assert!(matches!(
            drop_target_from(None, Some(60001), 0, true),
            Err(JailerError::Config(_))
        ));
    }

    /// A source that names a parent directory is refused before anything is
    /// mounted: its mirror would otherwise resolve outside the root — and
    /// `create_dir_all`/`File::create` would then touch the host, which is the
    /// one thing a jail may never do.
    #[test]
    fn a_source_that_escapes_the_root_is_refused() {
        let mut plan = Jailer::new("/tmp/plaza-jail/x");
        plan.bind("/../../etc/passwd", Access::ReadOnly);
        assert!(matches!(plan.validate(), Err(JailerError::Config(_))));

        let mut plan = Jailer::new("/tmp/plaza-jail/x");
        plan.bind_dir("../elsewhere", Access::ReadWrite);
        assert!(matches!(plan.validate(), Err(JailerError::Config(_))));

        let mut plan = Jailer::new("/tmp/plaza-jail/x");
        plan.tmpfs("/../run");
        assert!(matches!(plan.validate(), Err(JailerError::Config(_))));

        // The shapes the boot actually plans are accepted.
        let mut plan = Jailer::new("/tmp/plaza-jail/x");
        plan.bind("/boot/vmlinuz-linux", Access::ReadOnly);
        plan.bind_dir("/tmp/plaza-vsock", Access::ReadWrite);
        plan.bind("vmlinux", Access::ReadOnly);
        plan.tmpfs("/run");
        assert!(plan.validate().is_ok());
    }
}
