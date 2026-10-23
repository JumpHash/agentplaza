//! Previews (PLAN 18.1, 18.2): a lane's running app, reached through the hub, and the links that share it.
//!
//! - A preview is a fork VM that serves the lane's app on a port. The CI recipe's `preview` step starts it (`life.rs`),
//!   and registers the port with noded (6.1's inbound map). Its address on the hub is `/preview/<project>/<lane>/`.
//!   A request is carried through the node's tunnel (`nodes::Store::open_port`), so the hub never dials a node.
//! - It sleeps when idle (a noded snapshot) and wakes on the next request; it ends when its lane lands or closes, plus
//!   the project's grace, which the lifecycle sweep enforces (`life.rs`).
//! - Access: the owner's sign-in (`hub_auth::is_owner`) by default, or a link. A link is a token `<id>.<mac>`: `id` is
//!   a row of `preview_link` (scope `view` or `use`, an expiry, a label, revocation), `mac` is HMAC-SHA256 under the
//!   store's secret (`<state>/preview_secret`). A link grants its own preview only, and every opening of it is a row of
//!   `preview_opening` (when, from where, what).
//!
//! The rows are typed (`derive(apz_store::Table)`) in `plaza.db`, written through the store's writer like the shares.

pub mod life;
pub mod proxy;
pub mod route;
#[cfg(test)]
mod tests;

use std::path::Path;
use std::sync::{Arc, OnceLock};

use apz_store::rusqlite::{Connection, Params, params};
use apz_store::table::{Changes, Loaded, Row, Value, apply};
use apz_store::{Db, IndexDef, RowSet, StoreError, Table, TableDef};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::store::sql;

/// A preview's state: serving, asleep, or closed for good.
pub const AWAKE: &str = "awake";
pub const ASLEEP: &str = "asleep";
pub const CLOSED: &str = "closed";

/// A preview's kind: a web app on a fork's port (`web`), or a native app's station stream (`native`).
pub const WEB: &str = "web";
pub const NATIVE: &str = "native";

/// A link's scope: `view` reads (GET and HEAD); `use` makes any request the preview allows.
pub const VIEW: &str = "view";
pub const USE: &str = "use";

/// The query parameter that carries a link's token on its first request (the hub then sets it as a cookie).
pub const TOKEN_PARAM: &str = "preview_token";
/// The cookie a link's token is kept in, scoped to its preview's path.
pub const COOKIE: &str = "apz_preview";

/// One preview (a row of `preview`): the fork VM serving a lane's app.
#[derive(Clone, Debug, Serialize, Deserialize, Table)]
#[table(name = "preview")]
pub struct Preview {
    /// 32 lowercase hex characters.
    #[table(key)]
    pub id: String,
    pub project: String,
    /// The lane the preview shows: its change.
    pub lane: String,
    /// The node the fork runs on, the fork's noded run and its VM's CID.
    pub node: String,
    pub run: String,
    pub cid: u64,
    /// The port the app listens on in the fork.
    pub port: u32,
    /// `web` or `native`. A native preview's `station` is the workstation whose shared screen it shows.
    pub kind: String,
    pub station: String,
    /// `awake`, `asleep` or `closed`.
    pub status: String,
    pub created_ms: u64,
    /// The last request that used the preview: its idle clock.
    pub used_ms: u64,
    /// When the preview ends: its lane's landing or closing plus the grace. `None` while the lane is open.
    pub ends_ms: Option<u64>,
}

/// One share link of a preview (a row of `preview_link`). Its id is the token's id.
#[derive(Clone, Debug, Serialize, Deserialize, Table)]
#[table(name = "preview_link")]
pub struct Link {
    /// 32 lowercase hex characters: 128 random bits.
    #[table(key)]
    pub id: String,
    pub preview: String,
    /// `view` or `use`.
    pub scope: String,
    pub label: String,
    pub created_by: String,
    pub created_ms: u64,
    /// Unix milliseconds after which the link stops working.
    pub expires_ms: Option<u64>,
    /// Unix milliseconds when the link was revoked. A revoked link never works again.
    pub revoked_ms: Option<u64>,
}

/// One recorded opening of a link (a row of `preview_opening`). Its key is its place in the log.
#[derive(Clone, Debug, Serialize, Deserialize, Table)]
#[table(name = "preview_opening")]
pub struct Opening {
    #[serde(skip)]
    #[table(key)]
    pub seq: u64,
    pub link: String,
    pub preview: String,
    pub at_ms: u64,
    /// The peer's address, or empty when the hub does not know it.
    pub from: String,
    pub method: String,
    pub path: String,
}

/// The table of previews.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(Preview)
}

/// The table of links, keyed by id.
pub fn link_def() -> &'static TableDef {
    apz_store::root_def!(Link)
}

/// The table of openings, keyed by seq, with an index on (link, at_ms) for one link's openings.
pub fn opening_def() -> &'static TableDef {
    static DEF: OnceLock<TableDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = Opening::root();
        def.indexes.push(IndexDef {
            columns: vec!["link".to_string(), "at_ms".to_string()],
            unique: false,
        });
        def
    })
}

/// The previews, links and openings of the hub, as typed rows of `plaza.db`. Nothing is kept in memory: each call
/// reads or writes the database.
pub struct Store {
    db: Arc<Db>,
    secret: [u8; 32],
    /// A store made by `for_tests` keeps its folder and its hub's store alive with it.
    #[cfg(test)]
    _keep: Option<(tempfile::TempDir, apz_store::Store)>,
}

impl Store {
    /// The store over `db` (the hub's `plaza.db`). The token secret is `<state_dir>/preview_secret`, created on first use.
    pub fn open(db: Arc<Db>, state_dir: &Path) -> Store {
        Store {
            db,
            secret: load_secret(&state_dir.join("preview_secret")),
            #[cfg(test)]
            _keep: None,
        }
    }

    /// A store in a fresh folder, kept alive with the store.
    #[cfg(test)]
    pub fn for_tests() -> Store {
        let dir = tempfile::Builder::new()
            .prefix("apid-previews")
            .tempdir()
            .expect("a temporary folder");
        let store = crate::store::open(dir.path()).expect("the store opens");
        // The preview tables are not in `0001_init.sql` yet (the lead regenerates it after the merge): the test store
        // creates them from their definitions, as the schema dump does.
        let ddl = format!(
            "{}\n{}\n{}",
            def().ddl(),
            link_def().ddl(),
            opening_def().ddl()
        );
        store
            .db(crate::store::PLAZA)
            .write(move |conn| {
                conn.execute_batch(&ddl)?;
                Ok(())
            })
            .expect("the preview tables");
        let mut previews = Store::open(Arc::clone(store.db(crate::store::PLAZA)), dir.path());
        previews._keep = Some((dir, store));
        previews
    }

    /// Writes a new preview of `lane` on `node` (fork `run`, VM `cid`) serving `port`, awake, and returns it.
    pub fn add_preview(
        &self,
        project: &str,
        lane: &str,
        node: &str,
        run: &str,
        cid: u64,
        port: u32,
        now: u64,
    ) -> Result<Preview, StoreError> {
        let preview = Preview {
            id: random_id(),
            project: project.to_string(),
            lane: lane.to_string(),
            node: node.to_string(),
            run: run.to_string(),
            cid,
            port,
            kind: WEB.to_string(),
            station: String::new(),
            status: AWAKE.to_string(),
            created_ms: now,
            used_ms: now,
            ends_ms: None,
        };
        let stored = preview.clone();
        self.db
            .write(move |conn| write_row(conn, def(), sql::PREVIEW_POS, &stored.id, &stored))?;
        Ok(preview)
    }

    /// Writes a native preview of `lane`: the station `station`'s shared screen, awake, with no fork behind it.
    pub fn add_native_preview(
        &self,
        project: &str,
        lane: &str,
        station: &str,
        now: u64,
    ) -> Result<Preview, StoreError> {
        let preview = Preview {
            id: random_id(),
            project: project.to_string(),
            lane: lane.to_string(),
            node: String::new(),
            run: String::new(),
            cid: 0,
            port: 0,
            kind: NATIVE.to_string(),
            station: station.to_string(),
            status: AWAKE.to_string(),
            created_ms: now,
            used_ms: now,
            ends_ms: None,
        };
        let stored = preview.clone();
        self.db
            .write(move |conn| write_row(conn, def(), sql::PREVIEW_POS, &stored.id, &stored))?;
        Ok(preview)
    }

    /// Saves a changed preview at its place.
    pub fn save_preview(&self, preview: &Preview) -> Result<(), StoreError> {
        let stored = preview.clone();
        self.db
            .write(move |conn| write_row(conn, def(), sql::PREVIEW_POS, &stored.id, &stored))?;
        Ok(())
    }

    /// The preview `id`, whatever its state. A failed read is named on stderr and reads as no preview.
    pub fn preview(&self, id: &str) -> Option<Preview> {
        let id = id.to_string();
        self.read("preview", None, move |conn| {
            Ok(
                read_rows::<Preview>(conn, sql::PREVIEW_GET, def(), params![id])?
                    .into_iter()
                    .next(),
            )
        })
    }

    /// The newest preview of `lane` of `project`, whatever its state: a closed one says the lane's app has ended.
    pub fn lane_preview(&self, project: &str, lane: &str) -> Option<Preview> {
        let (project, lane) = (project.to_string(), lane.to_string());
        self.read("lane preview", None, move |conn| {
            Ok(
                read_rows::<Preview>(conn, sql::PREVIEW_LIST_LANE, def(), params![project, lane])?
                    .into_iter()
                    .next(),
            )
        })
    }

    /// Every preview, or the previews of `project` only, newest first.
    pub fn previews(&self, project: Option<&str>) -> Vec<Preview> {
        let project = project.map(str::to_string);
        self.read("previews", Vec::new(), move |conn| match project {
            Some(project) => read_rows(conn, sql::PREVIEW_LIST_PROJECT, def(), params![project]),
            None => read_rows(conn, sql::PREVIEW_LIST, def(), params![]),
        })
    }

    /// The previews that are not closed: the lifecycle sweep's list.
    pub fn open_previews(&self) -> Vec<Preview> {
        self.read("open previews", Vec::new(), move |conn| {
            read_rows(conn, sql::PREVIEW_OPEN, def(), params![])
        })
    }

    /// Writes a new link to `preview` with `scope`, a label and an expiry, and returns it. Its id is 128 random bits.
    pub fn add_link(
        &self,
        preview: &str,
        scope: &str,
        label: &str,
        expires_ms: Option<u64>,
        created_by: &str,
        now: u64,
    ) -> Result<Link, StoreError> {
        let link = Link {
            id: random_id(),
            preview: preview.to_string(),
            scope: scope.to_string(),
            label: label.to_string(),
            created_by: created_by.to_string(),
            created_ms: now,
            expires_ms,
            revoked_ms: None,
        };
        let stored = link.clone();
        self.db
            .write(move |conn| write_row(conn, link_def(), sql::LINK_POS, &stored.id, &stored))?;
        Ok(link)
    }

    /// The link `id`, whatever its state.
    pub fn link(&self, id: &str) -> Option<Link> {
        let id = id.to_string();
        self.read("link", None, move |conn| {
            Ok(
                read_rows::<Link>(conn, sql::LINK_GET, link_def(), params![id])?
                    .into_iter()
                    .next(),
            )
        })
    }

    /// The links of `preview`, newest first.
    pub fn links(&self, preview: &str) -> Vec<Link> {
        let preview = preview.to_string();
        self.read("links", Vec::new(), move |conn| {
            read_rows(conn, sql::LINK_LIST_PREVIEW, link_def(), params![preview])
        })
    }

    /// Revokes link `id`: a revoked link never works again. `Ok(None)` is an unknown id.
    pub fn revoke(&self, id: &str, now: u64) -> Result<Option<Link>, StoreError> {
        let id = id.to_string();
        self.db
            .write(move |conn| {
                let Some(mut link) =
                    read_rows::<Link>(conn, sql::LINK_GET, link_def(), params![id])?
                        .into_iter()
                        .next()
                else {
                    return Ok(None);
                };
                link.revoked_ms = link.revoked_ms.or(Some(now));
                write_row(conn, link_def(), sql::LINK_POS, &link.id, &link)?;
                Ok(Some(link))
            })
            .map(|written| written.value)
    }

    /// Records one opening of link `link` of `preview`: when, from `from`, what request.
    pub fn record_opening(
        &self,
        link: &str,
        preview: &str,
        from: &str,
        method: &str,
        path: &str,
        at_ms: u64,
    ) -> Result<(), StoreError> {
        let opening = Opening {
            seq: 0,
            link: link.to_string(),
            preview: preview.to_string(),
            at_ms,
            from: from.to_string(),
            method: method.to_string(),
            path: path.to_string(),
        };
        self.db.write(move |conn| {
            let last: i64 = conn
                .prepare_cached(sql::PREVIEW_OPENING_LAST_SEQ)?
                .query_row([], |r| r.get(0))?;
            let seq = u64::try_from(last).unwrap_or(0) + 1;
            let mut row = opening;
            row.seq = seq;
            save(conn, opening_def(), &row, seq)
        })?;
        Ok(())
    }

    /// The most recent openings of `link`, newest first, at most `limit`.
    pub fn openings(&self, link: &str, limit: u32) -> Vec<Opening> {
        let link = link.to_string();
        self.read("openings", Vec::new(), move |conn| {
            read_rows(
                conn,
                sql::PREVIEW_OPENING_PAGE,
                opening_def(),
                params![link, i64::from(limit)],
            )
        })
    }

    /// The token of link `id`: the id and its MAC.
    pub fn mint(&self, id: &str) -> String {
        format!("{id}.{}", hex(&self.mac(&format!("preview-link\n{id}"))))
    }

    /// The link id a token names, when the token's MAC is this store's. The row's own state is checked by the caller.
    pub fn verified_id(&self, token: &str) -> Option<String> {
        let (id, mac) = token.split_once('.')?;
        let well_formed = id.len() == 32
            && id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !well_formed {
            return None;
        }
        let expected = hex(&self.mac(&format!("preview-link\n{id}")));
        apz_trust::ct_eq(mac.as_bytes(), expected.as_bytes()).then(|| id.to_string())
    }

    fn mac(&self, message: &str) -> [u8; 32] {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.secret).expect("HMAC takes a key of any length");
        mac.update(message.as_bytes());
        mac.finalize().into_bytes().into()
    }

    fn read<T>(
        &self,
        what: &str,
        fallback: T,
        look: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> T {
        self.db.read(look).unwrap_or_else(|e| {
            eprintln!("previews: {what}: {e}");
            fallback
        })
    }
}

/// A fresh id: 128 random bits, as 32 lowercase hex characters.
fn random_id() -> String {
    let mut bytes = [0u8; 16];
    apz_os::random::fill(&mut bytes).expect("preview ids need OS randomness");
    hex(&bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The MAC secret: the file's 32 bytes, or a fresh secret written with mode 0600 when there is none.
fn load_secret(path: &Path) -> [u8; 32] {
    if let Ok(bytes) = std::fs::read(path)
        && let Ok(secret) = <[u8; 32]>::try_from(bytes.as_slice())
    {
        return secret;
    }
    let mut secret = [0u8; 32];
    apz_os::random::fill(&mut secret).expect("the preview secret needs OS randomness");
    if let Err(e) = write_private(path, &secret) {
        eprintln!("previews: write {}: {e}", path.display());
    }
    secret
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)
}

/// Writes a record at its place: its own when it is stored already, else the next after the highest. `pos_query` is the
/// statement that finds the place from the row's id.
fn write_row<T: Table>(
    conn: &Connection,
    def: &TableDef,
    pos_query: &str,
    id: &str,
    value: &T,
) -> Result<(), StoreError> {
    let pos: i64 = conn
        .prepare_cached(pos_query)?
        .query_row(params![id], |r| r.get(0))?;
    save(conn, def, value, u64::try_from(pos).unwrap_or(0))
}

/// Writes a record and its child rows as upserts: its root row at `pos`, then every child table's rows.
fn save<T: Table>(
    conn: &Connection,
    def: &TableDef,
    value: &T,
    pos: u64,
) -> Result<(), StoreError> {
    let mut rows = RowSet::new();
    value.put(def, vec![Value::Integer(int(pos))], &mut rows)?;
    let mut changes = Changes::default();
    for table in def.walk() {
        for row in rows.tables.remove(&table.name).unwrap_or_default() {
            changes.upserts.push((table.name.clone(), row));
        }
    }
    apply(conn, def, &changes)
}

/// Takes the rows `query` selects as `T`, in the table's column order (as `SELECT *` gives them).
fn read_rows<T: Table>(
    conn: &Connection,
    query: &str,
    def: &TableDef,
    params: impl Params,
) -> Result<Vec<T>, StoreError> {
    let width = def.key.len() + def.columns.len();
    let mut stmt = conn.prepare_cached(query)?;
    let mut rows = stmt.query(params)?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let mut values = Vec::with_capacity(width);
        for i in 0..width {
            values.push(r.get::<_, Value>(i)?);
        }
        let columns = values.split_off(def.key.len());
        let row = Row {
            key: values,
            columns,
        };
        out.push(T::take(def, &row, &Loaded::default())?);
    }
    Ok(out)
}

/// A count or a millisecond time as the SQL integer it is stored as.
fn int(value: impl TryInto<i64>) -> i64 {
    value.try_into().unwrap_or(i64::MAX)
}
