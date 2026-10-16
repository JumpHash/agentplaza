//! Zip import and export, finding skills in an archive, and GitHub URLs (docs/design/skill-library.md, "Import,
//! export, GitHub"). No network here: apid downloads, this reads and writes bytes.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Write};

/// The archive limits: entries, per-file and total bytes (uncompressed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_entries: usize,
    pub max_file: u64,
    pub max_total: u64,
}

impl Limits {
    /// A whole uploaded or downloaded archive (a repo may hold many skills).
    pub const ARCHIVE: Limits = Limits {
        max_entries: 5000,
        max_file: 5 * 1024 * 1024,
        max_total: 100 * 1024 * 1024,
    };
}

/// One file read from an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// `/`-separated, relative, no `..`, no leading `/`.
    pub path: String,
    pub bytes: Vec<u8>,
    pub executable: bool,
}

/// A skill found in an archive: the folder holding a SKILL.md, its SKILL.md text, and its other files with paths
/// relative to that folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundSkill {
    pub dir: String,
    pub skill_md: String,
    pub files: Vec<Entry>,
}

const S_IFMT: u32 = 0o170000;
const S_IFLNK: u32 = 0o120000;

/// Reads a zip within `limits`. Refuses entries with `..`, absolute paths, backslashes, drive letters, symlinks or
/// names that are not UTF-8, and stops at the first limit crossed (Err says which). Directories are skipped; a
/// GitHub zipball's single top folder (`<repo>-<sha>/`) is kept as part of the paths (find_skills handles any depth).
/// `./` segments are dropped. The per-file size is checked against the declared size and again while reading, so a
/// lying header cannot pass a zip bomb.
pub fn read_zip(bytes: &[u8], limits: Limits) -> Result<Vec<Entry>, String> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("not a zip: {e}"))?;
    if archive.len() > limits.max_entries {
        return Err(format!(
            "too many entries: more than {}",
            limits.max_entries
        ));
    }
    let mut out = Vec::new();
    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|e| format!("entry {index}: {e}"))?;
        let raw = std::str::from_utf8(file.name_raw())
            .map_err(|_| format!("entry {index}: the name is not UTF-8"))?
            .to_string();
        let mode = file.unix_mode();
        if mode.is_some_and(|m| m & S_IFMT == S_IFLNK) {
            return Err(format!("{raw}: symlinks are refused"));
        }
        let path = normalise(&raw)?;
        if file.is_dir() || path.is_empty() {
            continue;
        }
        if file.size() > limits.max_file {
            return Err(format!("{path}: larger than {} bytes", limits.max_file));
        }
        let cap = limits.max_file.min(limits.max_total - total);
        let mut data = Vec::new();
        (&mut file)
            .take(cap + 1)
            .read_to_end(&mut data)
            .map_err(|e| format!("{path}: {e}"))?;
        let got = data.len() as u64;
        if got > limits.max_file {
            return Err(format!("{path}: larger than {} bytes", limits.max_file));
        }
        if got > cap {
            return Err(format!(
                "the archive is larger than {} bytes",
                limits.max_total
            ));
        }
        total += got;
        out.push(Entry {
            path,
            bytes: data,
            executable: mode.is_some_and(|m| m & 0o111 != 0),
        });
    }
    Ok(out)
}

/// Every folder under `under` (a path prefix, "" for all) holding a `SKILL.md`, with the files inside that folder
/// (not those of a nested skill folder). Sorted by dir.
///
/// `under` is matched against the paths as they are, or, when every entry shares one top folder (a GitHub zipball's
/// `<repo>-<sha>/`), after that folder too. The returned `dir`s keep the top folder, so a caller picks by the dir it
/// was shown. A root skill (`SKILL.md` at the top of the archive) has dir "". A folder whose SKILL.md is not UTF-8 is
/// not returned, but its files still belong to it and not to the folder above.
pub fn find_skills(entries: &[Entry], under: &str) -> Vec<FoundSkill> {
    let under = under.trim_matches('/');
    let top = common_top(entries);
    let mut skill_mds: BTreeMap<&str, &Entry> = BTreeMap::new();
    for entry in entries {
        let dir = if entry.path == "SKILL.md" {
            Some("")
        } else {
            entry.path.strip_suffix("/SKILL.md")
        };
        if let Some(dir) = dir {
            skill_mds.insert(dir, entry);
        }
    }
    let mut wanted: BTreeSet<&str> = BTreeSet::new();
    for (dir, md) in &skill_mds {
        if in_scope(dir, under, top) && std::str::from_utf8(&md.bytes).is_ok() {
            wanted.insert(*dir);
        }
    }
    // Each file belongs to the nearest folder above it that holds a SKILL.md.
    let mut files: BTreeMap<&str, Vec<Entry>> = BTreeMap::new();
    for entry in entries {
        let Some(owner) = owner_of(&entry.path, &skill_mds) else {
            continue;
        };
        if !wanted.contains(owner) {
            continue;
        }
        let rel = if owner.is_empty() {
            entry.path.as_str()
        } else {
            &entry.path[owner.len() + 1..]
        };
        if rel == "SKILL.md" {
            continue;
        }
        files.entry(owner).or_default().push(Entry {
            path: rel.to_string(),
            bytes: entry.bytes.clone(),
            executable: entry.executable,
        });
    }
    wanted
        .into_iter()
        .map(|dir| FoundSkill {
            dir: dir.to_string(),
            skill_md: std::str::from_utf8(&skill_mds[dir].bytes)
                .unwrap_or("")
                .to_string(),
            files: files.remove(dir).unwrap_or_default(),
        })
        .collect()
}

/// A zip holding each skill as `<dir>/SKILL.md` plus `<dir>/<file>`, executables marked (unix mode 0755).
pub fn write_zip(skills: &[FoundSkill]) -> Result<Vec<u8>, String> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for skill in skills {
        let dir = normalise(&skill.dir)?;
        put(
            &mut writer,
            &join(&dir, "SKILL.md")?,
            skill.skill_md.as_bytes(),
            false,
        )?;
        for file in &skill.files {
            let rel = normalise(&file.path)?;
            if rel.is_empty() || rel == "SKILL.md" {
                continue;
            }
            put(
                &mut writer,
                &join(&dir, &rel)?,
                &file.bytes,
                file.executable,
            )?;
        }
    }
    let cursor = writer.finish().map_err(|e| e.to_string())?;
    Ok(cursor.into_inner())
}

/// A GitHub location: `https://github.com/<owner>/<repo>`, optionally `/tree/<ref>/<path…>` or `/blob/<ref>/<path…>`
/// (a SKILL.md link means its folder), `.git` suffix and trailing `/` allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubRef {
    pub owner: String,
    pub repo: String,
    /// None: the default branch.
    pub git_ref: Option<String>,
    /// The folder inside the repo, "" for the root.
    pub path: String,
}

/// Parses a github.com URL (https, or without a scheme; `github.com` or `www.github.com`). A ref containing `/` is
/// ambiguous in tree and blob URLs: the first segment after `tree`/`blob` is the ref and the rest is the path.
/// Refuses other hosts, other schemes, missing owner or repo, names outside `[A-Za-z0-9_.-]`, other URL shapes
/// (issues, pulls…) and `.`/`..` segments.
pub fn parse_github_url(url: &str) -> Result<GithubRef, String> {
    let url = url.trim();
    let rest = match url.split_once("://") {
        Some(("https", rest)) => rest,
        Some((scheme, _)) => return Err(format!("{scheme}:// links are not accepted, use https")),
        None => url,
    };
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    if !(host.eq_ignore_ascii_case("github.com") || host.eq_ignore_ascii_case("www.github.com")) {
        return Err(format!("not a github.com URL: host {host}"));
    }
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segs.len() < 2 {
        return Err("the URL needs an owner and a repo: https://github.com/<owner>/<repo>".into());
    }
    let (owner, repo_raw) = (segs[0], segs[1]);
    let repo = repo_raw
        .strip_suffix(".git")
        .filter(|r| !r.is_empty())
        .unwrap_or(repo_raw);
    if !valid_name(owner) || !valid_name(repo) {
        return Err(format!(
            "{owner}/{repo_raw}: names use letters, digits, '_', '.' and '-'"
        ));
    }
    let (git_ref, folder): (Option<String>, &[&str]) = match segs.get(2).copied() {
        None => (None, &[]),
        Some("tree") | Some("blob") => {
            let Some(git_ref) = segs.get(3) else {
                return Err("the URL names no ref after /tree/".into());
            };
            let mut folder = segs.get(4..).unwrap_or(&[]);
            if folder.last() == Some(&"SKILL.md") {
                folder = &folder[..folder.len() - 1];
            }
            (Some(git_ref.to_string()), folder)
        }
        Some(other) => {
            return Err(format!(
                "not a folder link: /{other}/ is not a tree or blob URL"
            ));
        }
    };
    if segs
        .iter()
        .skip(3)
        .any(|s| *s == "." || *s == ".." || s.contains('\\'))
    {
        return Err("the path has a '.', '..' or backslash segment".into());
    }
    Ok(GithubRef {
        owner: owner.to_string(),
        repo: repo.to_string(),
        git_ref,
        path: folder.join("/"),
    })
}

/// Normalises an archive path: refuses `..`, a leading `/`, a backslash or a drive letter; drops `.` and empty segments.
fn normalise(raw: &str) -> Result<String, String> {
    if raw.contains('\\') {
        return Err(format!("{raw}: backslashes are refused"));
    }
    if raw.starts_with('/') {
        return Err(format!("{raw}: absolute paths are refused"));
    }
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return Err(format!("{raw}: drive letters are refused"));
    }
    let mut parts = Vec::new();
    for seg in raw.split('/') {
        match seg {
            "" | "." => {}
            ".." => return Err(format!("{raw}: '..' is refused")),
            seg => parts.push(seg),
        }
    }
    Ok(parts.join("/"))
}

/// The one top folder every entry sits under, if there is one.
fn common_top(entries: &[Entry]) -> Option<&str> {
    let mut top: Option<&str> = None;
    for entry in entries {
        let first = entry.path.split_once('/')?.0;
        match top {
            None => top = Some(first),
            Some(t) if t != first => return None,
            Some(_) => {}
        }
    }
    top
}

fn in_scope(dir: &str, under: &str, top: Option<&str>) -> bool {
    under.is_empty()
        || within(dir, under)
        || top.is_some_and(|t| within(dir, &format!("{t}/{under}")))
}

fn within(path: &str, base: &str) -> bool {
    path == base
        || path
            .strip_prefix(base)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// The nearest folder above `path` that holds a SKILL.md, the root ("") included.
fn owner_of<'a>(path: &'a str, dirs: &BTreeMap<&str, &Entry>) -> Option<&'a str> {
    let mut end = path.len();
    while let Some(idx) = path[..end].rfind('/') {
        let dir = &path[..idx];
        if dirs.contains_key(dir) {
            return Some(dir);
        }
        end = idx;
    }
    dirs.contains_key("").then_some("")
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

fn join(dir: &str, rel: &str) -> Result<String, String> {
    let joined = if dir.is_empty() {
        rel.to_string()
    } else {
        format!("{dir}/{rel}")
    };
    normalise(&joined)
}

fn put(
    writer: &mut zip::ZipWriter<Cursor<Vec<u8>>>,
    path: &str,
    data: &[u8],
    executable: bool,
) -> Result<(), String> {
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(if executable { 0o755 } else { 0o644 });
    writer
        .start_file(path, options)
        .map_err(|e| format!("{path}: {e}"))?;
    writer.write_all(data).map_err(|e| format!("{path}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: Limits = Limits {
        max_entries: 100,
        max_file: 1024,
        max_total: 4096,
    };

    fn raw_zip(files: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in files {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(data.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn file(path: &str, data: &str, executable: bool) -> Entry {
        Entry {
            path: path.into(),
            bytes: data.as_bytes().to_vec(),
            executable,
        }
    }

    #[test]
    fn round_trip_finds_nested_skills() {
        let skills = vec![
            FoundSkill {
                dir: "a".into(),
                skill_md: "# a".into(),
                files: vec![
                    file("run.sh", "#!/bin/sh", true),
                    file("lib/x.txt", "x", false),
                ],
            },
            FoundSkill {
                dir: "a/b".into(),
                skill_md: "# b".into(),
                files: vec![file("y.txt", "y", false)],
            },
            FoundSkill {
                dir: "c".into(),
                skill_md: "# c".into(),
                files: vec![],
            },
        ];
        let zip = write_zip(&skills).unwrap();
        let entries = read_zip(&zip, Limits::ARCHIVE).unwrap();
        let found = find_skills(&entries, "");
        assert_eq!(
            found.iter().map(|s| s.dir.as_str()).collect::<Vec<_>>(),
            ["a", "a/b", "c"]
        );
        assert_eq!(found[0].skill_md, "# a");
        let mut names: Vec<(&str, bool)> = found[0]
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.executable))
            .collect();
        names.sort();
        assert_eq!(names, vec![("lib/x.txt", false), ("run.sh", true)]);
        assert_eq!(
            found[1]
                .files
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            ["y.txt"]
        );
    }

    #[test]
    fn find_skills_root_and_under_filter() {
        // root_skill_and_under_filter: a root SKILL.md is found with its own files, and the
        // `under` filter keeps only the matching subfolder.
        {
            let zip = raw_zip(&[
                ("SKILL.md", "# root"),
                ("tool.py", "print(1)"),
                ("skills/x/SKILL.md", "# x"),
            ]);
            let entries = read_zip(&zip, Limits::ARCHIVE).unwrap();
            let all = find_skills(&entries, "");
            assert_eq!(
                all.iter().map(|s| s.dir.as_str()).collect::<Vec<_>>(),
                ["", "skills/x"]
            );
            assert_eq!(
                all[0]
                    .files
                    .iter()
                    .map(|f| f.path.as_str())
                    .collect::<Vec<_>>(),
                ["tool.py"]
            );
            let under = find_skills(&entries, "skills");
            assert_eq!(
                under.iter().map(|s| s.dir.as_str()).collect::<Vec<_>>(),
                ["skills/x"]
            );
        }
        // github_zipball_top_folder_is_stripped_for_under: a GitHub zipball's top folder is
        // kept in the skill dirs but stripped when matching the `under` filter.
        {
            let zip = raw_zip(&[
                ("repo-abc/SKILL.md", "# top"),
                ("repo-abc/skills/x/SKILL.md", "# x"),
                ("repo-abc/skills/x/a.txt", "a"),
            ]);
            let entries = read_zip(&zip, Limits::ARCHIVE).unwrap();
            let all = find_skills(&entries, "");
            assert_eq!(
                all.iter().map(|s| s.dir.as_str()).collect::<Vec<_>>(),
                ["repo-abc", "repo-abc/skills/x"]
            );
            let under = find_skills(&entries, "skills/x");
            assert_eq!(
                under.iter().map(|s| s.dir.as_str()).collect::<Vec<_>>(),
                ["repo-abc/skills/x"]
            );
            assert_eq!(
                under[0]
                    .files
                    .iter()
                    .map(|f| f.path.as_str())
                    .collect::<Vec<_>>(),
                ["a.txt"]
            );
        }
    }

    #[test]
    fn zip_slip_and_bad_names_are_refused() {
        for bad in ["../x", "a/../../x", "/etc/passwd", "a\\b", "C:/x", "C:x"] {
            let zip = raw_zip(&[(bad, "evil")]);
            assert!(
                read_zip(&zip, Limits::ARCHIVE).is_err(),
                "{bad} must be refused"
            );
        }
        let ok = read_zip(&raw_zip(&[("./a/b.txt", "x")]), Limits::ARCHIVE).unwrap();
        assert_eq!(ok[0].path, "a/b.txt");
    }

    #[test]
    fn symlinks_are_refused() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .add_symlink("link", "target", zip::write::SimpleFileOptions::default())
            .unwrap();
        let zip = writer.finish().unwrap().into_inner();
        assert!(
            read_zip(&zip, Limits::ARCHIVE)
                .unwrap_err()
                .contains("link")
        );
    }

    #[test]
    fn limits_are_enforced() {
        let big = "x".repeat(50);
        let big_zip = raw_zip(&[("big.txt", big.as_str())]);
        let per_file = Limits {
            max_entries: 10,
            max_file: 10,
            max_total: 100,
        };
        assert!(
            read_zip(&big_zip, per_file)
                .unwrap_err()
                .contains("big.txt")
        );
        assert!(read_zip(&big_zip, SMALL).is_ok());

        let fifteen = "y".repeat(15);
        let two = raw_zip(&[("a.txt", fifteen.as_str()), ("b.txt", fifteen.as_str())]);
        let total = Limits {
            max_entries: 10,
            max_file: 100,
            max_total: 20,
        };
        assert!(read_zip(&two, total).is_err());
        assert!(
            read_zip(
                &two,
                Limits {
                    max_entries: 1,
                    ..total
                }
            )
            .is_err()
        );
        assert_eq!(
            read_zip(
                &two,
                Limits {
                    max_total: 30,
                    ..total
                }
            )
            .unwrap()
            .len(),
            2
        );
    }

    #[test]
    fn github_url_forms() {
        let ok = |url: &str, git_ref: Option<&str>, path: &str| {
            let r = parse_github_url(url).unwrap_or_else(|e| panic!("{url}: {e}"));
            assert_eq!((r.owner.as_str(), r.repo.as_str()), ("o", "r"), "{url}");
            assert_eq!(r.git_ref.as_deref(), git_ref, "{url}");
            assert_eq!(r.path, path, "{url}");
        };
        ok("https://github.com/o/r", None, "");
        ok("https://github.com/o/r.git", None, "");
        ok("https://github.com/o/r/", None, "");
        ok("https://www.github.com/o/r", None, "");
        ok("github.com/o/r", None, "");
        ok("https://github.com/o/r?tab=readme#x", None, "");
        ok("https://github.com/o/r/tree/main", Some("main"), "");
        ok(
            "https://github.com/o/r/tree/main/skills/x",
            Some("main"),
            "skills/x",
        );
        ok(
            "https://github.com/o/r/blob/v1.2/skills/x/SKILL.md",
            Some("v1.2"),
            "skills/x",
        );
        ok(
            "https://github.com/o/r/blob/main/SKILL.md",
            Some("main"),
            "",
        );
        ok(
            "https://github.com/o/r/tree/feat/x/skills/y",
            Some("feat"),
            "x/skills/y",
        );
        for bad in [
            "https://gitlab.com/o/r",
            "https://github.com.evil.com/o/r",
            "http://github.com/o/r",
            "https://github.com/o",
            "https://github.com/",
            "https://github.com/o/r$x",
            "https://github.com/o/..",
            "https://github.com/o/r/tree",
            "https://github.com/o/r/tree/main/../x",
            "https://github.com/o/r/issues/1",
        ] {
            assert!(parse_github_url(bad).is_err(), "{bad} must be refused");
        }
    }
}
