//! The test inventory of a Rust repository (PLAN 15.4): every `#[test]` function in its `.rs` files, with its file and its
//! package. The inventory is what a selection picks from and what its fraction is taken over.
//!
//! The scan is textual: a test attribute (`#[test]`, `#[tokio::test]`, `#[xxx::test(...)]`) followed by the `fn` it marks.
//! Build directories and hidden directories are skipped.

use std::fs;
use std::io;
use std::path::Path;

use crate::graph::package_of;
use crate::llvm::relative;
use crate::map::{Package, TestEntry};

/// Every test under `root`, sorted by id. A test's package is the one that owns its file ("" when none does).
pub fn scan_rust(root: &Path, packages: &[Package]) -> io::Result<Vec<TestEntry>> {
    let root_text = root.to_string_lossy().to_string();
    let mut files = Vec::new();
    collect(root, &mut files)?;
    let mut tests = Vec::new();
    for file in files {
        let text = fs::read_to_string(&file)?;
        let path = relative(&file.to_string_lossy(), &root_text);
        let package = package_of(packages, &path)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        for name in test_names(&text) {
            tests.push(TestEntry {
                id: format!("{path}::{name}"),
                package: package.clone(),
                file: path.clone(),
                smoke: false,
            });
        }
    }
    tests.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(tests)
}

/// The `.rs` files under `dir`, skipping build output and hidden directories.
fn collect(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> io::Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            if name == "target" || name == "node_modules" || name.starts_with('.') {
                continue;
            }
            collect(&path, out)?;
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
    Ok(())
}

/// The names of the test functions in one file's text, in order.
pub fn test_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut pending = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if pending && !trimmed.starts_with("#[") && !trimmed.is_empty() {
            if let Some(name) = fn_name(trimmed) {
                names.push(name);
            }
            pending = false;
            continue;
        }
        if is_test_attr(trimmed) {
            pending = true;
        }
    }
    names
}

/// Whether an attribute line is a test's: `#[test]`, or an attribute whose path ends in `test` (`#[tokio::test]`).
fn is_test_attr(trimmed: &str) -> bool {
    let Some(rest) = trimmed.strip_prefix("#[") else {
        return false;
    };
    let path = rest.split(['(', ']', ' ']).next().unwrap_or_default();
    path == "test" || path.ends_with("::test")
}

/// The name of the function a line declares (`pub async fn name(`), if it declares one.
fn fn_name(trimmed: &str) -> Option<String> {
    let at = trimmed.find("fn ")?;
    if at > 0 {
        let before = trimmed.as_bytes()[at - 1];
        if before.is_ascii_alphanumeric() || before == b'_' {
            return None;
        }
    }
    let name: String = trimmed[at + 3..]
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_plain_async_and_attributed_tests_and_skips_helpers() {
        // One line with escapes: the scanner reads this file too, and a literal's own lines must not look like tests.
        let text = "fn helper() {}\n#[test]\nfn plain() {}\n#[tokio::test]\nasync fn awaited() {}\n#[test]\n\
                    #[ignore = \"slow\"]\npub fn ignored_but_a_test() {}\n#[cfg(test)]\nmod t {\n    \
                    #[test(flavor = \"multi\")]\n    fn flavored() {}\n}\n";
        assert_eq!(
            test_names(text),
            vec!["plain", "awaited", "ignored_but_a_test", "flavored"]
        );
    }

    #[test]
    fn a_test_attribute_on_a_non_fn_is_dropped() {
        assert!(test_names("#[test]\nstruct NotAFn;\n").is_empty());
    }

    #[test]
    fn an_identifier_containing_fn_is_not_a_declaration() {
        assert_eq!(fn_name("let elfn = 1;"), None);
        assert_eq!(
            fn_name("pub(crate) fn go_now(x: u8)"),
            Some("go_now".into())
        );
    }
}
