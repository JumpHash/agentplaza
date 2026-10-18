//! Golden: a real repository's history imported, every tree checked against git's.
//! Runs when `WEFT_GOLDEN_REPO` names a git repository (`WEFT_GOLDEN_BRANCH`, default main).

mod common;

use common::plaza;
use serde_json::json;

#[test]
fn a_real_history_imports_with_every_tree_matching_git() {
    let Ok(path) = std::env::var("WEFT_GOLDEN_REPO") else {
        eprintln!("WEFT_GOLDEN_REPO is not set: skipped");
        return;
    };
    let branch = std::env::var("WEFT_GOLDEN_BRANCH").unwrap_or_else(|_| "main".into());
    let p = plaza("golden", &[]);
    p.owner_says("repo.create", json!({ "repo": "golden" }))
        .unwrap();
    let start = std::time::Instant::now();
    let source = weft::import::Source::open(&path, None, &branch).unwrap();
    let report = weft::import::import(&p.store, "golden", &source, &branch, "main").unwrap();
    eprintln!(
        "imported {} commits, {} file changes, {} tree checks in {:?}",
        report.commits,
        report.files,
        report.checked,
        start.elapsed()
    );
    // Every commit's tree, not just every hundredth.
    let st = p.store.state();
    let chain = st.repos["golden"].refs["main"].clone();
    let gits: Vec<String> = chain
        .iter()
        .map(|r| {
            st.changes[&st.revisions[r].change]
                .import
                .as_ref()
                .unwrap()
                .git
                .clone()
        })
        .collect();
    drop(st);
    let repo = weft::git::Repo::open(std::path::Path::new(&path)).unwrap();
    for (rev, g) in chain.iter().zip(&gits) {
        let commit =
            weft::git::parse_commit(&repo.read(&weft::git::parse_oid(g).unwrap()).unwrap().1)
                .unwrap();
        weft::import::check_tree(&p.store, rev, &commit.tree).unwrap();
    }
    eprintln!("all {} trees match ({:?})", chain.len(), start.elapsed());
    assert!(p.store.verify_deep().is_ok());
    std::fs::remove_dir_all(&p.dir).ok();
}
