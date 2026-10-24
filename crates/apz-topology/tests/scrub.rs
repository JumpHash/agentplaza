//! The scrubber over SQLite snapshots (PLAN 22.5, 10.13's done-when): a seeded personal field in a production snapshot
//! never reaches the lab; with the rule it is scrubbed, passes the scan, and reaches the data stage; fakes keep joins; a
//! refused copy writes nothing where the lab reads; the copy is deleted at the run's end; the scan's report never names a
//! value. The Postgres source's URL rules are tested here; a live server is not run.

use std::path::{Path, PathBuf};

use apz_topology::data::{self, DataStage, Read};
use apz_topology::scrub::{self, Refusal, Rules, Scrubber};
use apz_topology::source::{self, Sqlite};
use rusqlite::Connection;

/// A production snapshot in `dir`: users with emails and names, orders with card numbers, contacts with the same emails.
fn snapshot(dir: &Path) -> PathBuf {
    let path = dir.join("production-snapshot.sqlite");
    let conn = Connection::open(&path).expect("the snapshot opens");
    conn.execute_batch(
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT, name TEXT);
         CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER, card TEXT, total INTEGER);
         CREATE TABLE contacts (id INTEGER PRIMARY KEY, email TEXT);
         INSERT INTO users VALUES (1, 'ana.keller@example.com', 'Ana Keller'), (2, 'bo@example.org', 'Bo Lind');
         INSERT INTO orders VALUES (10, 1, '4539 5787 6362 1486', 4200), (11, 2, '4539578763621486', 990);
         INSERT INTO contacts VALUES (1, 'ana.keller@example.com'), (2, 'bo@example.org');",
    )
    .expect("the snapshot is seeded");
    path
}

/// A fresh directory the test owns: the run's work directory, and inside it the lab's input directory.
fn run_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("apz-scrub-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a run directory");
    dir
}

fn copy_with(rules: &str, snapshot: &Path, dir: &Path) -> Result<scrub::Copy, Refusal> {
    let mut source = Sqlite::open(snapshot).expect("the snapshot opens read-only");
    let rules = Rules::parse(rules).expect("the rules parse");
    let mut scrubber = Scrubber::new(rules, [3; 32]).expect("a scrubber");
    scrub::copy(&mut source, &mut scrubber, dir)
}

fn count(path: &Path, sql: &str) -> i64 {
    let conn = Connection::open(path).expect("the copy opens");
    conn.query_row(sql, [], |row| row.get(0))
        .expect("the query runs")
}

const EMAIL_AND_NAME_RULES: &str = "names = [\"Keller\"]\n\
[[column]]\ntable = \"users\"\ncolumn = \"email\"\naction = \"fake\"\n\
[[column]]\ntable = \"users\"\ncolumn = \"name\"\naction = \"remove\"\n\
[[column]]\ntable = \"orders\"\ncolumn = \"card\"\naction = \"fake\"\n\
[[column]]\ntable = \"contacts\"\ncolumn = \"email\"\naction = \"fake\"\n";

#[test]
fn an_unruled_email_column_stops_the_copy_and_nothing_reaches_the_lab() {
    let dir = run_dir("unruled");
    let source = snapshot(&dir);
    let lab_input = dir.join("lab-input");
    std::fs::create_dir_all(&lab_input).expect("the lab's input directory");
    let refused =
        copy_with("", &source, &dir.join("copy")).expect_err("an unruled email stops the copy");
    let Refusal::Scan(report) = &refused else {
        panic!("expected the scan to refuse, got {refused}");
    };
    let email = report
        .findings
        .iter()
        .find(|f| f.table == "users" && f.column == "email")
        .expect("the finding names the column");
    assert_eq!(email.kind, "email");
    assert_eq!(email.samples, 2, "the sample count, not the values");
    let shown = refused.to_string();
    assert!(
        !shown.contains("example.com"),
        "the report names no value: {shown}"
    );
    assert!(
        !shown.contains("Keller"),
        "the report names no value: {shown}"
    );
    assert!(!dir.join("copy").join(scrub::COPY_FILE).exists(), "no copy");
    assert_eq!(
        std::fs::read_dir(&lab_input).expect("listed").count(),
        0,
        "nothing in the lab's input"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn with_the_rules_the_copy_passes_the_scan_and_the_data_stage_runs_over_it() {
    let dir = run_dir("ruled");
    let source = snapshot(&dir);
    let copy = copy_with(EMAIL_AND_NAME_RULES, &source, &dir.join("copy"))
        .expect("the copy passes the scan");
    assert!(copy.report.clean(), "{}", copy.report);
    let copy_path = copy.path().to_path_buf();
    assert!(copy_path.exists());
    assert_eq!(
        count(
            &copy_path,
            "SELECT count(*) FROM users WHERE email = 'ana.keller@example.com'"
        ),
        0
    );
    assert_eq!(
        count(&copy_path, "SELECT count(*) FROM users WHERE name IS NULL"),
        2
    );
    assert_eq!(
        count(&copy_path, "SELECT count(*) FROM users"),
        2,
        "every row is kept"
    );

    let stage = DataStage {
        previous: vec![],
        production: Some("prod-db".into()),
        migrations: vec![],
        down: vec![],
        reads: vec![Read {
            id: "users".into(),
            sql: "SELECT id, email FROM users".into(),
            min_rows: 2,
        }],
        down_reads: vec![],
        writers: None,
        invariants: vec![],
        backup: None,
    };
    stage.check().expect("the stage checks");
    let run = data::run_from(&stage, Path::new("."), &dir.join("data"), Some(&copy_path));
    assert!(
        run.notes.iter().all(|n| n == "no backup is declared"),
        "{:?}",
        run.notes
    );
    let rows = run
        .metrics
        .iter()
        .find(|m| m.name == "data.read.users.rows")
        .expect("the read ran");
    assert_eq!(rows.value, 2.0);
    let previous = run
        .metrics
        .iter()
        .find(|m| m.name == "data.previous.failed")
        .expect("recorded");
    assert_eq!(previous.value, 0.0);
    drop(copy);
    assert!(!copy_path.exists(), "the copy is gone when the run ends");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_fake_keeps_joins_across_tables_and_the_source_is_never_written() {
    let dir = run_dir("joins");
    let source = snapshot(&dir);
    let before = std::fs::read(&source).expect("the snapshot bytes");
    let copy =
        copy_with(EMAIL_AND_NAME_RULES, &source, &dir.join("copy")).expect("the copy passes");
    let join = "SELECT count(*) FROM users u JOIN contacts c ON u.email = c.email";
    assert_eq!(
        count(copy.path(), join),
        2,
        "the same email in two tables is the same fake"
    );
    assert_eq!(
        count(
            copy.path(),
            "SELECT count(*) FROM orders o JOIN users u ON o.user_id = u.id"
        ),
        2
    );
    assert_eq!(
        std::fs::read(&source).expect("the snapshot bytes"),
        before,
        "the source is untouched"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_card_number_in_an_unruled_column_stops_the_copy_and_a_bad_rule_names_nothing() {
    let dir = run_dir("card");
    let source = snapshot(&dir);
    let refused = copy_with(
        "[[column]]\ntable = \"users\"\ncolumn = \"email\"\naction = \"fake\"\n",
        &source,
        &dir.join("copy"),
    )
    .expect_err("the card column is unruled");
    let Refusal::Scan(report) = refused else {
        panic!("expected a scan refusal");
    };
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.table == "orders" && f.column == "card" && f.kind == "card")
    );
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.table == "users" && f.column == "name" && f.kind == "name")
    );
    let missing = copy_with(
        "[[column]]\ntable = \"users\"\ncolumn = \"phone\"\naction = \"remove\"\n",
        &source,
        &dir.join("copy2"),
    )
    .expect_err("a rule on a column the source lacks");
    assert!(matches!(missing, Refusal::Failed(_)), "{missing}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_blob_the_rules_do_not_remove_stops_the_copy() {
    let dir = run_dir("blob");
    let path = dir.join("blob.sqlite");
    let conn = Connection::open(&path).expect("opens");
    conn.execute_batch(
        "CREATE TABLE files (id INTEGER, body BLOB); INSERT INTO files VALUES (1, x'0102');",
    )
    .expect("seeded");
    drop(conn);
    let refused = copy_with("", &path, &dir.join("copy")).expect_err("a blob cannot be scanned");
    let Refusal::Scan(report) = refused else {
        panic!("expected a scan refusal");
    };
    assert_eq!(report.findings[0].kind, "unscannable-blob");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_source_that_is_missing_is_not_created_and_is_read_only() {
    let dir = run_dir("missing");
    let missing = dir.join("nope.sqlite");
    assert!(
        Sqlite::open(&missing).is_err(),
        "a missing source is an error"
    );
    assert!(!missing.exists(), "and no file is created");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_data_stage_declares_one_source_or_one_previous_schema() {
    let stage = |production: Option<&str>, previous: Vec<String>| DataStage {
        previous,
        production: production.map(str::to_string),
        migrations: vec![],
        down: vec![],
        reads: vec![],
        down_reads: vec![],
        writers: None,
        invariants: vec![],
        backup: None,
    };
    assert!(stage(None, vec![]).check().is_err());
    assert!(
        stage(Some("prod-db"), vec!["db/previous.sql".into()])
            .check()
            .is_err()
    );
    assert!(stage(Some("Not A Label"), vec![]).check().is_err());
    assert!(stage(Some("prod-db"), vec![]).check().is_ok());
    let run = data::run_from(
        &stage(Some("prod-db"), vec![]),
        Path::new("."),
        &run_dir("nocopy"),
        None,
    );
    assert!(
        run.notes.iter().any(|n| n.contains("was not given")),
        "{:?}",
        run.notes
    );
    assert!(
        run.metrics
            .iter()
            .all(|m| m.value == apz_topology::stage::UNREADABLE)
    );
}

#[test]
fn a_postgres_url_reaches_loopback_plain_and_any_other_host_over_verified_tls() {
    let plain = |url: &str| source::transport_for(&url.parse().expect("parses"), None);
    assert_eq!(
        plain("postgres://app@localhost/app"),
        Ok(source::Transport::Plain)
    );
    assert_eq!(
        plain("postgres://app@127.0.0.1:5432/app"),
        Ok(source::Transport::Plain)
    );
    assert_eq!(
        plain("postgres://app@[::1]/app"),
        Ok(source::Transport::Plain)
    );
    // A socket is a Unix path: only a Unix build of the driver reads the host as one.
    #[cfg(unix)]
    assert_eq!(
        plain("postgres://app@%2Ftmp/app"),
        Ok(source::Transport::Plain)
    );
    assert_eq!(
        plain("postgres://app@db.example.com/app"),
        Ok(source::Transport::Verified { ca: None })
    );
    assert_eq!(
        plain("postgres://app@10.0.0.7/app"),
        Ok(source::Transport::Verified { ca: None })
    );
    // One remote host among local ones is remote: the copy does not split its connection.
    assert_eq!(
        plain("postgres://app@localhost,db.example.com/app"),
        Ok(source::Transport::Verified { ca: None })
    );
    assert!(
        plain("postgres://app@db.example.com/app?sslmode=disable").is_err(),
        "no clear text to a remote host"
    );
    assert_eq!(
        plain("postgres://app@localhost/app?sslmode=disable"),
        Ok(source::Transport::Plain),
        "loopback may stay plain"
    );
    assert!(
        !source::POSTGRES_COLUMNS
            .to_ascii_uppercase()
            .contains("INSERT")
    );
}

#[test]
fn a_connector_ca_must_hold_a_certificate_and_the_public_roots_build_a_connector() {
    let remote: postgres::Config = "postgres://app@db.example.com/app".parse().expect("parses");
    assert!(source::transport_for(&remote, Some("not a certificate")).is_err());
    assert!(
        source::transport_for(&remote, Some("")).is_err(),
        "an empty CA is refused"
    );
    assert!(
        source::tls_connector(None).is_ok(),
        "the public roots and the ring provider build"
    );
    assert!(source::tls_connector(Some("garbage")).is_err());
}
