//! Workstations: machines agents operate through the hub (docs/design/workstations.md). This is the hub's core:
//! the enrollment (join codes, the registry, drain, revoke), the link socket each station's daemon holds open,
//! leases with the pixel-session queue, the station audit, and STOP ALL's freeze.
//!
//! - `registry`: the stations and the join codes (the `stations` component of plaza_state), the link table.
//! - `link`: one station's live link: requests and their answers by id, event fan-out, the loop that serves it.
//! - `leases`: an agent's claim on a station, the pixel capacity of a machine, and the queue.
//! - `ops`: the actions with the rest of the hub: the audit, the hello and events, grants, STOP ALL, the ticker.
//! - `audit`: the station audit, one typed entry per kind, in the `station_audit` table of `plaza.db`.
//! - `recordings`: a lease's recording (its actions, its frames in the hub's object store), the sampler and the
//!   retention sweep.
//! - `api`: the routes, listed in its module docs.
//! - `screen`: the live view (`WS .../screen`) and the thumbnails; the viewers of a session share one stream.
//! - `control`: the owner's takeover socket (`WS .../control`): take, input, give back.
//! - `takeover`: the takeover policies (agent waits, release the lease, keep working with the pointer paused).
//! - `act`: the choke point every workstation action passes (the checks, the laws, the audit), and `hooks` after it.
//! - `tools`: the workstation tools agents and the owner's MCP client are offered, parsed into operations for `act`.
//! - `vms`: the workstation VMs the hub makes on its nodes (a VM enrols with its join code; its leases snapshot and
//!   revert its disk through the node).
//! - `sync`: the work folder's two-way sync: the hub's mirror (rows in `station_sync_file`, content in the object
//!   store), the reconcile pass with a station's manifest, conflicts (kept on both sides), and the owner's file routes.
//!
//! The containment rule holds: a station is never the hub's own machine (refused at join and at every hello), and
//! agents reach a station only through the hub.

mod act;
mod api;
pub mod audit;
mod control;
mod hooks;
mod leases;
pub(crate) mod link;
mod ops;
pub mod recordings;
mod registry;
mod screen;
pub mod sync;
mod takeover;
pub mod tools;
mod vms;

use crate::state::App;

pub use api::routes;
pub use ops::{freeze_all, spawn};
pub use registry::{Store, def as store_def};
// The agents' pixel path (stations/act.rs) asks these before a screenshot or an input.
#[allow(unused_imports)]
pub use takeover::{pixel_paused, pointer_paused};

/// Every blob the station rows name, each once: each frame's JPEG and each sync row's content (the hub's copy of a path,
/// and the version both sides last agreed). These are the stations' roots in the hub's object store: the sweep keeps
/// them and frees what no row names (`storage::roots`).
pub fn blob_hashes(app: &App) -> Result<Vec<apz_blobs::Hash>, String> {
    blob_hashes_in(app.store.db(crate::store::PLAZA))
}

/// [`blob_hashes`] over the plaza's store, where the station rows are.
fn blob_hashes_in(db: &apz_store::Db) -> Result<Vec<apz_blobs::Hash>, String> {
    let mut texts = recordings::frame_hashes(db)?;
    texts.extend(sync::content_hashes(db)?);
    let mut hashes = texts
        .iter()
        .map(|text| text.parse::<apz_blobs::Hash>())
        .collect::<Result<Vec<_>, _>>()?;
    hashes.sort_unstable();
    hashes.dedup();
    Ok(hashes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_hashes_lists_each_frame_and_sync_content_once() {
        let dir = std::env::temp_dir().join(format!("apid-stations-roots-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = crate::store::open(&dir).expect("the store opens");
        let db = store.db(crate::store::PLAZA);
        let jpeg = apz_blobs::Hash::of(b"a frame");
        let file = apz_blobs::Hash::of(b"a file");
        let older = apz_blobs::Hash::of(b"an older version");
        // Two frames of the same JPEG name one blob.
        for (seq, ts_ms) in [(1, 1000), (2, 1100)] {
            let frame = recordings::FrameTime { seq, ts_ms };
            recordings::put_frame(db, "r1", frame, &jpeg.to_string(), 7).unwrap();
        }
        let row = |path: &str, hash: apz_blobs::Hash, base: apz_blobs::Hash| sync::Row {
            station: "ws-1".into(),
            path: path.into(),
            hash: Some(hash.to_string()),
            size: 7,
            mtime_ms: 1000,
            version: 2,
            base: Some(base.to_string()),
            conflict_of: None,
            synced_ms: 1000,
        };
        sync::save_rows(
            db,
            "ws-1",
            &[row("a.txt", file, older), row("b.txt", file, file)],
        )
        .unwrap();

        let mut want = vec![jpeg, file, older];
        want.sort();
        assert_eq!(
            blob_hashes_in(db).unwrap(),
            want,
            "the frame, the file and the version it agreed, each once"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
