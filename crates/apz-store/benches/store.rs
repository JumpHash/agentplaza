//! Micro-benchmarks of the store's per-request path: one write through the writer thread (its savepoint and its
//! commit) and one read on a pooled reader (its read transaction). The file is scratch, so no time goes to fsync and
//! what is measured is the store's own work.
//!
//! Run with `cargo bench -p apz-store`.

use std::hint::black_box;

use apz_store::{Durability, FileSpec, Migration, Store};
use criterion::{Criterion, criterion_group, criterion_main};

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "create item",
    sql: "CREATE TABLE item (id INTEGER PRIMARY KEY, body TEXT NOT NULL) STRICT;",
}];

const FILES: &[FileSpec] = &[FileSpec {
    name: "plaza",
    file: "plaza.db",
    durability: Durability::Scratch,
    migrations: MIGRATIONS,
}];

fn benches(c: &mut Criterion) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path(), FILES).expect("open store");
    let db = store.db("plaza");
    db.write(|conn| {
        conn.execute("INSERT INTO item (id, body) VALUES (1, 'x')", [])?;
        Ok(())
    })
    .expect("seed row");

    c.bench_function("store: one write", |b| {
        b.iter(|| {
            black_box(
                db.write(|conn| {
                    conn.execute("INSERT OR REPLACE INTO item (id, body) VALUES (1, 'x')", [])?;
                    Ok(())
                })
                .unwrap(),
            )
        })
    });

    c.bench_function("store: one read", |b| {
        b.iter(|| {
            black_box(
                db.read(|conn| {
                    let body = conn.query_row("SELECT body FROM item WHERE id = 1", [], |r| {
                        r.get::<_, String>(0)
                    })?;
                    Ok(body)
                })
                .unwrap(),
            )
        })
    });
}

criterion_group!(store, benches);
criterion_main!(store);
