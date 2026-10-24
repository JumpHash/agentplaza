use super::*;

fn big(n: u32, seed: u32) -> Vec<u8> {
    (0..n)
        .map(|i| (i.wrapping_add(seed).wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect()
}

#[test]
fn blobs_round_trip_and_survive_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let text = b"fn memory_mib() -> Value {\n    Value::Null\n}\n".repeat(50);
    let h = {
        let blobs = Blobs::open(dir.path()).unwrap();
        let h = blobs.put(&text).unwrap();
        assert_eq!(blobs.put(&text).unwrap(), h, "a second put is a no-op");
        h
    };
    let blobs = Blobs::open(dir.path()).unwrap();
    assert_eq!(blobs.get(&h).unwrap(), text);
    assert_eq!(blobs.missing(&[h, Hash::of(b"nope")]).len(), 1);
}

#[test]
fn a_large_blob_is_chunked_and_a_small_edit_shares_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = Blobs::open(dir.path()).unwrap();
    let mut data = big(12_000_000, 0);
    let h1 = blobs.put(&data).unwrap();
    let (_, after_first) = blobs.stats();
    data[6_000_000] ^= 0xff;
    let h2 = blobs.put(&data).unwrap();
    let (_, after_second) = blobs.stats();
    assert_ne!(h1, h2);
    assert_eq!(blobs.get(&h2).unwrap(), data);
    assert!(
        after_second - after_first < 5 << 20,
        "an edit stores a few chunks, not the whole blob again"
    );
}

#[test]
fn an_upload_that_does_not_match_its_hash_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = Blobs::open(dir.path()).unwrap();
    assert!(blobs.put_checked(&Hash::of(b"a"), b"b").is_err());
    assert!(blobs.put_checked(&Hash::of(b"a"), b"a").is_ok());
}

#[test]
fn a_streamed_put_stores_what_a_whole_put_would() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = Blobs::open(dir.path()).unwrap();
    let data = big(10_000_000, 7);
    let (streamed, size) = blobs
        .put_reader(std::io::Cursor::new(data.clone()))
        .unwrap();
    assert_eq!(size, data.len() as u64);
    assert_eq!(streamed, Hash::of(&data));
    let (_, before) = blobs.stats();
    assert_eq!(
        blobs.put(&data).unwrap(),
        streamed,
        "the same chunks: nothing new stored"
    );
    assert_eq!(blobs.stats().1, before);
    let mut back = Vec::new();
    blobs
        .reader(&streamed)
        .unwrap()
        .read_to_end(&mut back)
        .unwrap();
    assert_eq!(back, data);
    // A small stream is stored whole.
    let (small, n) = blobs.put_reader(&b"hello"[..]).unwrap();
    assert_eq!((small, n), (Hash::of(b"hello"), 5));
}

#[test]
fn a_range_read_reads_only_the_chunks_it_covers() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = Blobs::open(dir.path()).unwrap();
    let data = big(40_000_000, 3);
    let h = blobs.put(&data).unwrap();
    let chunks = blobs.chunks(&h).unwrap().expect("chunked").len();
    assert!(chunks > 8, "{chunks} chunks");
    let before = blobs.entry_reads();
    let part = blobs.read_range(&h, 20_000_000, 1000).unwrap();
    assert_eq!(part, &data[20_000_000..20_001_000]);
    let reads = blobs.entry_reads() - before;
    // The manifest (twice: once to find the chunks, once inside `chunks`) and the one or two chunks the range spans.
    assert!(
        reads <= 4,
        "{reads} entries read for a 1000-byte range of a {chunks}-chunk blob"
    );
    let tail = blobs.read_range(&h, data.len() as u64 - 10, 100).unwrap();
    assert_eq!(tail, &data[data.len() - 10..]);
}

#[test]
fn a_sweep_frees_the_unreferenced_and_keeps_what_is_written_during_it() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = Blobs::open(dir.path()).unwrap();
    let kept = blobs.put(b"referenced").unwrap();
    let dropped = blobs.put(b"unreferenced").unwrap();
    std::thread::sleep(Duration::from_millis(20));
    let during = std::sync::Mutex::new(None);
    let report = blobs
        .sweep(
            &|h: &Hash| {
                // A put that lands while the sweep marks: written after the sweep's cutoff, so it is kept.
                let mut d = during.lock().unwrap();
                if d.is_none() {
                    *d = Some(blobs.put(b"written during the sweep").unwrap());
                }
                *h == kept
            },
            SweepOptions {
                grace: Duration::from_millis(10),
                compact_below: 0.5,
            },
        )
        .unwrap();
    let during = during.into_inner().unwrap().unwrap();
    assert!(blobs.has(&kept));
    assert!(!blobs.has(&dropped), "{report:?}");
    assert!(
        blobs.has(&during),
        "a blob written during the sweep survives it"
    );
    assert_eq!(report.dead, 1);
}

#[test]
fn a_sweep_compacts_a_mostly_dead_pack_and_removes_it() {
    let dir = tempfile::tempdir().unwrap();
    let keep;
    {
        let blobs = Blobs::open(dir.path()).unwrap();
        keep = blobs.put(&big(100_000, 1)).unwrap();
        for i in 0..20 {
            blobs.put(&big(100_000, 100 + i)).unwrap();
        }
    }
    // Start a second pack, so the first is no longer the one appended to.
    let path = pack_path(dir.path(), 2);
    File::create(&path).unwrap();
    let blobs = Blobs::open(dir.path()).unwrap();
    let old_size = fs::metadata(pack_path(dir.path(), 1)).unwrap().len();
    std::thread::sleep(Duration::from_millis(20));
    let report = blobs
        .sweep(
            &|h: &Hash| *h == keep,
            SweepOptions {
                grace: Duration::from_millis(10),
                compact_below: 0.5,
            },
        )
        .unwrap();
    assert_eq!(report.dead, 20);
    assert_eq!(report.packs_compacted, 1, "{report:?}");
    assert!(
        !pack_path(dir.path(), 1).exists(),
        "the compacted pack is gone"
    );
    assert!(
        report.freed_bytes > old_size / 2,
        "{report:?} of a {old_size}-byte pack"
    );
    assert_eq!(
        blobs.get(&keep).unwrap(),
        big(100_000, 1),
        "the live blob moved forward and still reads"
    );
    drop(blobs);
    let blobs = Blobs::open(dir.path()).unwrap();
    assert_eq!(
        blobs.get(&keep).unwrap(),
        big(100_000, 1),
        "and after a reopen"
    );
}

#[test]
fn entries_written_past_the_index_are_indexed_on_open_and_a_torn_tail_is_cut() {
    let dir = tempfile::tempdir().unwrap();
    let h = {
        let blobs = Blobs::open(dir.path()).unwrap();
        blobs.put(b"before").unwrap()
    };
    // An entry the index never heard of (a crash between the pack write and the index row), then a torn entry.
    let pack = pack_path(dir.path(), 1);
    let stored = b"orphan";
    let orphan = Hash::of(stored);
    let mut entry = Vec::new();
    entry.extend_from_slice(&(stored.len() as u32).to_le_bytes());
    entry.extend_from_slice(&orphan.0);
    entry.push(0);
    entry.extend_from_slice(&(stored.len() as u64).to_le_bytes());
    entry.extend_from_slice(stored);
    let mut f = OpenOptions::new().append(true).open(&pack).unwrap();
    f.write_all(&entry).unwrap();
    let good = f.metadata().unwrap().len();
    f.write_all(&[9, 0, 0, 0, 1, 2, 3]).unwrap();
    drop(f);
    let blobs = Blobs::open(dir.path()).unwrap();
    assert_eq!(blobs.get(&h).unwrap(), b"before");
    assert_eq!(blobs.get(&orphan).unwrap(), b"orphan");
    assert_eq!(
        fs::metadata(&pack).unwrap().len(),
        good,
        "the torn entry was cut off"
    );
}

#[test]
fn opening_takes_the_same_time_at_any_size() {
    let dir = tempfile::tempdir().unwrap();
    let h = {
        let blobs = Blobs::open(dir.path()).unwrap();
        blobs.put(b"small").unwrap()
    };
    // Grow the pack to 100 GB (sparse) and checkpoint the index past it, as a store that size would be.
    let pack = pack_path(dir.path(), 1);
    let size: u64 = 100 << 30;
    OpenOptions::new()
        .write(true)
        .open(&pack)
        .unwrap()
        .set_len(size)
        .unwrap();
    {
        let conn = apz_store::rusqlite::Connection::open(dir.path().join("index.db")).unwrap();
        conn.execute(
            "UPDATE pack SET indexed_len = ?1 WHERE no = 1",
            [size as i64],
        )
        .unwrap();
    }
    let started = std::time::Instant::now();
    let blobs = Blobs::open(dir.path()).unwrap();
    let took = started.elapsed();
    assert!(
        took < Duration::from_secs(1),
        "opened a 100 GB store in {took:?}"
    );
    assert_eq!(blobs.get(&h).unwrap(), b"small");
}

#[test]
fn a_second_process_on_the_folder_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let _first = Blobs::open(dir.path()).unwrap();
    assert!(Blobs::open(dir.path()).is_err());
}
