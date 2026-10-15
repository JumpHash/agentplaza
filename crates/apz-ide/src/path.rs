//! Path rules for the IDE's file API. A path is relative, `/`-separated, and never
//! climbs out of the workspace root. Files under `.plaza/` are readable and never
//! writable. Secret files (keys, `.env`, credentials) are neither read nor written.
//!
//! The hub and the harness both call these; the harness checks the real filesystem
//! as well (a symlink that resolves outside the root is not caught here).

use std::fmt;

/// Why a path was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathError {
    /// Nothing is left after cleaning (`""`, `.`, `./`). `check_dir` takes this as the root.
    Empty,
    /// Starts with `/` or `\`, or with a drive letter such as `C:`.
    Absolute,
    /// Contains a backslash. The separator is `/`.
    Backslash,
    /// Contains a NUL byte.
    Nul,
    /// Contains a control character.
    Control,
    /// Has a `..` segment.
    Escape,
    /// Under `.plaza/`: readable, never writable.
    Reserved,
    /// A secret file or a secret directory. Never read or written.
    Secret,
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PathError::Empty => "path is empty",
            PathError::Absolute => "path must be relative",
            PathError::Backslash => "path must use / as the separator",
            PathError::Nul => "path contains a NUL byte",
            PathError::Control => "path contains a control character",
            PathError::Escape => "path may not contain ..",
            PathError::Reserved => "the .plaza directory is read-only",
            PathError::Secret => "secret files are not available in the IDE",
        })
    }
}

impl std::error::Error for PathError {}

/// Cleans a relative path: drops `.` segments and empty segments (`./`, `//`,
/// a trailing `/`), and refuses everything else the rules name.
fn clean(p: &str) -> Result<String, PathError> {
    if p.contains('\0') {
        return Err(PathError::Nul);
    }
    let b = p.as_bytes();
    let drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
    if p.starts_with('/') || p.starts_with('\\') || drive {
        return Err(PathError::Absolute);
    }
    if p.contains('\\') {
        return Err(PathError::Backslash);
    }
    if p.chars().any(|c| c.is_control()) {
        return Err(PathError::Control);
    }
    let mut segs: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => return Err(PathError::Escape),
            s => segs.push(s),
        }
    }
    if segs.is_empty() {
        return Err(PathError::Empty);
    }
    Ok(segs.join("/"))
}

/// Cleans a relative file path. Returns the cleaned path, or why it was refused.
pub fn check_path(p: &str) -> Result<String, PathError> {
    clean(p)
}

/// Cleans a relative directory path. `""` (and `.`, `./`) is the root. Refuses secret directories.
pub fn check_dir(p: &str) -> Result<String, PathError> {
    match clean(p) {
        Err(PathError::Empty) => Ok(String::new()),
        Err(e) => Err(e),
        Ok(c) if is_secret(&c) => Err(PathError::Secret),
        Ok(c) => Ok(c),
    }
}

/// Cleans a path the IDE may read: a valid path that is not secret.
pub fn check_read(p: &str) -> Result<String, PathError> {
    let c = clean(p)?;
    if is_secret(&c) {
        return Err(PathError::Secret);
    }
    Ok(c)
}

/// Cleans a path the IDE may write: a readable path that is not under `.plaza/`.
/// Any segment named `.plaza` counts, not only one at the root.
pub fn check_write(p: &str) -> Result<String, PathError> {
    let c = check_read(p)?;
    if c.split('/').any(|s| s.eq_ignore_ascii_case(".plaza")) {
        return Err(PathError::Reserved);
    }
    Ok(c)
}

/// The secret deny list, matched case-insensitively on each segment's name.
/// Basenames `.env` and `.env.*`, `*.pem`, `*.key`, `id_rsa*`, `id_ed25519*`, `*.p12`,
/// `*.pfx`, `.netrc`, `.npmrc`, `.pypirc`, `credentials`, `credentials.json`; and any
/// path with a segment `.ssh`, `.aws` or `.gnupg`. Tree listings should drop these too.
pub fn is_secret(p: &str) -> bool {
    let segs: Vec<String> = p
        .split(['/', '\\'])
        .filter(|s| !s.is_empty() && *s != ".")
        .map(|s| s.to_ascii_lowercase())
        .collect();
    let Some(base) = segs.last() else {
        return false;
    };
    if segs
        .iter()
        .any(|s| matches!(s.as_str(), ".ssh" | ".aws" | ".gnupg"))
    {
        return true;
    }
    let b = base.as_str();
    b == ".env"
        || b.starts_with(".env.")
        || b.ends_with(".pem")
        || b.ends_with(".key")
        || b.ends_with(".p12")
        || b.ends_with(".pfx")
        || b.starts_with("id_rsa")
        || b.starts_with("id_ed25519")
        || matches!(
            b,
            ".netrc" | ".npmrc" | ".pypirc" | "credentials" | "credentials.json"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_path_normalises_clean_paths_and_refuses_bad_shapes() {
        // A clean path comes back normalised.
        for (input, want) in [
            ("a/b.txt", "a/b.txt"),
            ("./a//b/", "a/b"),
            ("a/./b", "a/b"),
            ("a/b/c.txt/", "a/b/c.txt"),
            ("café/naïve.txt", "café/naïve.txt"),
            // A name with dots is not a .. segment.
            ("..hidden/x", "..hidden/x"),
            ("a..b", "a..b"),
        ] {
            assert_eq!(check_path(input).unwrap(), want, "{input:?}");
        }

        // Every refusal of `clean` is a row.
        let cases = [
            ("", PathError::Empty),
            (".", PathError::Empty),
            ("./", PathError::Empty),
            ("./.", PathError::Empty),
            ("./././", PathError::Empty),
            ("/x", PathError::Absolute),
            ("/", PathError::Absolute),
            ("//", PathError::Absolute),
            ("/etc/passwd", PathError::Absolute),
            ("\\x", PathError::Absolute),
            ("\\\\server\\share", PathError::Absolute),
            ("C:\\x", PathError::Absolute),
            ("c:/x", PathError::Absolute),
            ("D:x", PathError::Absolute),
            ("C:", PathError::Absolute),
            ("..", PathError::Escape),
            ("../x", PathError::Escape),
            ("a/../b", PathError::Escape),
            ("a/..", PathError::Escape),
            ("a/../../x", PathError::Escape),
            ("./../x", PathError::Escape),
            ("a//../b", PathError::Escape),
            ("a\0b", PathError::Nul),
            ("a\nb", PathError::Control),
            ("a\tb", PathError::Control),
            ("a\u{7f}b", PathError::Control),
        ];
        for (p, want) in cases {
            assert_eq!(check_path(p), Err(want), "{p:?}");
        }

        // Backslash is refused as a separator.
        assert_eq!(check_path("a\\b"), Err(PathError::Backslash));
        assert_eq!(check_write("src\\main.rs"), Err(PathError::Backslash));
    }

    #[test]
    fn check_dir_takes_the_root_and_refuses_secret_dirs() {
        assert_eq!(check_dir("").unwrap(), "");
        assert_eq!(check_dir("./").unwrap(), "");
        assert_eq!(check_dir("src/").unwrap(), "src");
        assert_eq!(check_dir("a/..").unwrap_err(), PathError::Escape);
        assert_eq!(check_dir("/").unwrap_err(), PathError::Absolute);
        assert_eq!(check_dir("home/.ssh").unwrap_err(), PathError::Secret);
    }

    #[test]
    fn writes_refuse_plaza_and_secrets() {
        assert_eq!(check_write(".plaza").unwrap_err(), PathError::Reserved);
        assert_eq!(
            check_write(".plaza/checkpoints/1").unwrap_err(),
            PathError::Reserved
        );
        assert_eq!(check_write("./.plaza/x").unwrap_err(), PathError::Reserved);
        assert_eq!(
            check_write("sub/.plaza/x").unwrap_err(),
            PathError::Reserved
        );
        assert_eq!(check_write(".PLAZA/x").unwrap_err(), PathError::Reserved);
        assert_eq!(check_write(".env").unwrap_err(), PathError::Secret);
        assert_eq!(
            check_write("deploy/server.pem").unwrap_err(),
            PathError::Secret
        );
        assert_eq!(check_write("../x").unwrap_err(), PathError::Escape);
        assert_eq!(check_write("src/main.rs").unwrap(), "src/main.rs");
        assert_eq!(
            check_write(".plaza2/x").unwrap(),
            ".plaza2/x",
            "only the .plaza segment is reserved"
        );
    }

    #[test]
    fn reads_allow_plaza_and_refuse_secrets() {
        assert_eq!(
            check_read(".plaza/checkpoints/1").unwrap(),
            ".plaza/checkpoints/1"
        );
        assert_eq!(check_read(".env").unwrap_err(), PathError::Secret);
        assert_eq!(
            check_read("keys/private.pem").unwrap_err(),
            PathError::Secret
        );
        assert_eq!(check_read("/etc/passwd").unwrap_err(), PathError::Absolute);
        assert_eq!(check_read("").unwrap_err(), PathError::Empty);
    }

    #[test]
    fn deny_list_matches_the_secret_names_and_leaves_similar_names_alone() {
        for (p, want) in [
            // Secret names are refused.
            (".env", true),
            (".env.local", true),
            ("config/.env.production", true),
            ("certs/server.pem", true),
            ("keys/prod.key", true),
            ("id_rsa", true),
            ("home/id_rsa.pub", true),
            ("id_ed25519", true),
            ("x/id_ed25519.bak", true),
            ("a/b.p12", true),
            ("a/b.pfx", true),
            (".netrc", true),
            (".npmrc", true),
            (".pypirc", true),
            ("credentials", true),
            ("aws/credentials.json", true),
            ("home/.ssh/config", true),
            (".aws/config", true),
            ("repo/.gnupg/secring", true),
            ("PRIVATE.KEY", true),
            ("./.env", true),
            ("ENV/.Env.Dev", true),
            // Similar names are left alone.
            ("env.rs", false),
            ("envelope.txt", false),
            (".envrc", false),
            ("src/key_store.rs", false),
            ("keyboard.txt", false),
            ("credentials_helper.rs", false),
            ("sshd_config", false),
            ("notes/ssh.md", false),
            ("docs/aws.md", false),
            ("src/main.rs", false),
            ("a/b/c", false),
            ("", false),
        ] {
            assert_eq!(
                is_secret(p),
                want,
                "{p} should be {}",
                if want { "secret" } else { "not secret" }
            );
        }
    }
}
