//! Local provider children must be reaped even when the hub exits without running destructors.
//! Weak handles include in-flight calls without keeping idle children alive after their adapter drops.

use std::path::PathBuf;
use std::process::{Child, ExitStatus};
use std::sync::{Arc, Mutex, Weak};

use crate::GatewayError;

#[derive(Default)]
pub(crate) struct Registry {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    stopped: bool,
    children: Vec<Weak<Mutex<Process>>>,
}

struct Process {
    child: Child,
    /// Only a fresh directory created and owned by the adapter, never a configured working directory.
    owned_directory: Option<PathBuf>,
}

pub(crate) struct ManagedChild(Arc<Mutex<Process>>);

impl Registry {
    pub(crate) fn track(&self, child: Child) -> Result<ManagedChild, GatewayError> {
        self.track_process(Process {
            child,
            owned_directory: None,
        })
    }

    /// Transfers cleanup ownership of an adapter-created temporary directory. The caller must not
    /// pass a user directory; it must also stop cleaning this path independently after transfer.
    pub(crate) fn track_owned(
        &self,
        child: Child,
        owned_directory: PathBuf,
    ) -> Result<ManagedChild, GatewayError> {
        self.track_process(Process {
            child,
            owned_directory: Some(owned_directory),
        })
    }

    fn track_process(&self, mut process: Process) -> Result<ManagedChild, GatewayError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.stopped {
            process.stop();
            return Err(GatewayError::Config(
                "local providers are shutting down".into(),
            ));
        }
        state.children.retain(|child| child.strong_count() > 0);
        let child = Arc::new(Mutex::new(process));
        state.children.push(Arc::downgrade(&child));
        Ok(ManagedChild(child))
    }

    pub(crate) fn shutdown(&self) {
        let children = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.stopped = true;
            std::mem::take(&mut state.children)
        };
        for child in children.into_iter().filter_map(|child| child.upgrade()) {
            stop(&child);
        }
    }
}

fn stop(child: &Mutex<Process>) {
    child.lock().unwrap_or_else(|e| e.into_inner()).stop();
}

impl Process {
    fn stop(&mut self) {
        let _ = self.child.kill();
        // Do not remove a live process's files if reaping fails. A later shutdown/drop can retry.
        if self.child.wait().is_ok()
            && let Some(directory) = &self.owned_directory
            && (std::fs::remove_dir_all(directory).is_ok() || !directory.exists())
        {
            self.owned_directory = None;
        }
    }
}

impl ManagedChild {
    pub(crate) fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .child
            .try_wait()
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        stop(&self.0);
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn explicit_shutdown_stops_a_child_held_by_an_in_flight_call() {
        let registry = Registry::default();
        let mut child = registry
            .track(Command::new("sleep").arg("30").spawn().unwrap())
            .unwrap();
        assert!(child.try_wait().unwrap().is_none());
        registry.shutdown();
        assert!(child.try_wait().unwrap().is_some());
        assert!(
            registry
                .track(Command::new("sleep").arg("30").spawn().unwrap())
                .is_err()
        );
    }

    #[test]
    fn explicit_shutdown_removes_only_the_registered_owned_directory_after_reaping() {
        let root = std::env::temp_dir().join(format!(
            "apz-owned-process-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let owned = root.join("owned");
        let configured = root.join("configured-cwd");
        std::fs::create_dir_all(&owned).unwrap();
        std::fs::create_dir(&configured).unwrap();
        std::fs::write(owned.join("transcript"), "private task text").unwrap();
        std::fs::write(configured.join("keep"), "user file").unwrap();
        let registry = Registry::default();
        let mut child = registry
            .track_owned(
                Command::new("sleep")
                    .arg("30")
                    .current_dir(&configured)
                    .spawn()
                    .unwrap(),
                owned.clone(),
            )
            .unwrap();
        registry.shutdown();
        assert!(child.try_wait().unwrap().is_some());
        assert!(!owned.exists());
        assert_eq!(
            std::fs::read_to_string(configured.join("keep")).unwrap(),
            "user file"
        );
        drop(child);
        std::fs::remove_dir_all(root).unwrap();
    }
}
