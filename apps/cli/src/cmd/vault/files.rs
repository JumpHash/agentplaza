//! A `.env` file's keys, as the hub reads them, and the removal of such a file. Shredding never
//! follows a link: a symbolic link is refused, and the file is opened with `O_NOFOLLOW`.

#[cfg(unix)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::io::Write;
use std::path::Path;

/// The name of every `NAME=value` pair in `text`, in order (repeats included). The rules are the
/// hub's (`apid`'s dotenv reader): an optional `export ` prefix, `#` comments and blank lines, and
/// quoted values that run over several lines. No value is kept. An error names a line number only.
pub fn dotenv_names(text: &str) -> Result<Vec<String>, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<&str> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    let mut names = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let number = i + 1;
        let trimmed = lines[i].trim();
        i += 1;
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let body = match trimmed.strip_prefix("export") {
            Some(rest) if rest.starts_with([' ', '\t']) => rest.trim_start(),
            _ => trimmed,
        };
        let Some((name, value_text)) = body.split_once('=') else {
            return Err(format!("line {number} is not NAME=value"));
        };
        names.push(name.trim().to_string());
        let value_text = value_text.trim_start();
        if let Some(quote @ ('"' | '\'')) = value_text.chars().next() {
            i = skip_quoted(&lines, i, &value_text[1..], quote == '"', number)?;
        }
    }
    Ok(names)
}

/// Skips a quoted value that starts in `first` and may continue on the lines from `next` on.
/// Returns the index of the next unread line. Only the layout is read; the value is dropped.
fn skip_quoted(
    lines: &[&str],
    next: usize,
    first: &str,
    double: bool,
    number: usize,
) -> Result<usize, String> {
    let mut next = next;
    let mut current = first.to_string();
    loop {
        let mut chars = current.chars();
        let mut tail = None;
        while let Some(c) = chars.next() {
            if double && c == '\\' {
                chars.next();
            } else if (double && c == '"') || (!double && c == '\'') {
                tail = Some(chars.as_str().trim().to_string());
                break;
            }
        }
        if let Some(tail) = tail {
            if !tail.is_empty() && !tail.starts_with('#') {
                return Err(format!("line {number}: text after the closing quote"));
            }
            return Ok(next);
        }
        let Some(line) = lines.get(next) else {
            return Err(format!("line {number}: the quote is never closed"));
        };
        current = line.to_string();
        next += 1;
    }
}

/// Refuses what `shred` must not touch: a symbolic link, anything that is not a regular file, and
/// (off Unix, where a link cannot be opened without following it) every file.
#[cfg(unix)]
pub fn check_shreddable(path: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.file_type().is_symlink() {
        return Err(format!(
            "{} is a symbolic link; refusing to shred it",
            path.display()
        ));
    }
    if !meta.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn check_shreddable(path: &Path) -> Result<(), String> {
    Err(format!(
        "--shred needs Unix (the file is opened without following links); {} was left alone",
        path.display()
    ))
}

/// Overwrites the file with zero bytes of its own length, syncs it to disk, and removes it.
pub fn shred(path: &Path) -> Result<(), String> {
    check_shreddable(path)?;
    overwrite_with_zeros(path)?;
    std::fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Zero bytes over the whole file, in place, then `fsync`. Opened without truncation and without
/// following a link.
#[cfg(unix)]
fn overwrite_with_zeros(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let fail = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut file = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(fail)?;
    let meta = file.metadata().map_err(fail)?;
    if !meta.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    let zeros = [0u8; 64 * 1024];
    let mut left = meta.len();
    while left > 0 {
        let n = left.min(zeros.len() as u64) as usize;
        file.write_all(&zeros[..n]).map_err(fail)?;
        left -= n as u64;
    }
    file.sync_all().map_err(fail)
}

#[cfg(not(unix))]
fn overwrite_with_zeros(path: &Path) -> Result<(), String> {
    check_shreddable(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A unique directory under the system temp directory, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            static COUNT: AtomicU64 = AtomicU64::new(0);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let dir = std::env::temp_dir().join(format!(
                "apz-vault-{name}-{}-{nanos}-{}",
                std::process::id(),
                COUNT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).expect("create the scratch directory");
            Scratch(dir)
        }

        fn file(&self, name: &str, text: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, text).expect("write the scratch file");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const ENV_FILE: &str = "\u{feff}# the keys\n\nexport ANTHROPIC_API_KEY=sk-ant-1\nHF_TOKEN = hf_x  # trailing comment\nTITLE=\"two\nwords\"\nSINGLE='a \\n literal'\nEMPTY=\nHASH=#not-a-value\n";

    #[test]
    fn a_file_on_disk_yields_each_key_once_in_order() {
        let scratch = Scratch::new("keys");
        let path = scratch.file("keys.env", ENV_FILE.as_bytes());
        let text = std::fs::read_to_string(&path).expect("read the scratch file");
        let names = dotenv_names(&text).expect("a valid file");
        assert_eq!(
            names,
            [
                "ANTHROPIC_API_KEY",
                "HF_TOKEN",
                "TITLE",
                "SINGLE",
                "EMPTY",
                "HASH"
            ]
        );
        assert_eq!(names.len(), 6);
    }

    #[test]
    fn dotenv_names_follow_the_quoting_and_line_rules() {
        for (case, input, want) in [
            (
                "repeated names are all listed so the caller can count them",
                "A=1\nA=2\nB=3\n",
                vec!["A", "A", "B"],
            ),
            (
                "escaped quotes do not end a double-quoted value early",
                "A=\"x\\\"y\"\nB=1\n",
                vec!["A", "B"],
            ),
        ] {
            assert_eq!(dotenv_names(input).expect("valid"), want, "{case}");
        }
        for (case, input, needle) in [
            (
                "a line without equals is an error with its line number",
                "OK=1\nnot a pair\n",
                "line 2",
            ),
            (
                "an unclosed quote is an error",
                "A=\"open\nstill open",
                "never closed",
            ),
            (
                "text after a closing quote is an error",
                "A=\"x\" trailing\n",
                "after the closing quote",
            ),
        ] {
            let err = dotenv_names(input).unwrap_err();
            assert!(err.contains(needle), "{case}: {err}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn shred_overwrites_with_zeros_of_the_same_length_then_removes_the_file() {
        let scratch = Scratch::new("shred");
        let path = scratch.file("secrets.env", b"SECRET_ONE=alpha\nSECRET_TWO=beta\n");
        let len = std::fs::metadata(&path).expect("metadata").len();
        overwrite_with_zeros(&path).expect("overwrite");
        let after = std::fs::read(&path).expect("read back");
        assert_eq!(after.len() as u64, len);
        assert!(after.iter().all(|b| *b == 0), "every byte is zero");
        shred(&path).expect("shred");
        assert!(!path.exists(), "the file is gone");
    }

    /// Off Unix the product refuses `--shred` on purpose: the file must be left exactly as it was.
    #[cfg(not(unix))]
    #[test]
    fn shred_refuses_off_unix_and_leaves_the_file_alone() {
        let scratch = Scratch::new("refuse");
        let path = scratch.file("secrets.env", b"KEEP=me\n");
        let err = shred(&path).unwrap_err();
        assert!(err.contains("needs Unix"), "{err}");
        assert_eq!(
            std::fs::read(&path).expect("file still there"),
            b"KEEP=me\n"
        );
    }

    #[test]
    fn shred_fails_on_a_missing_file_and_refuses_a_directory() {
        // Shredding a missing file is an error.
        {
            let scratch = Scratch::new("missing");
            assert!(shred(&scratch.0.join("nope.env")).is_err());
        }
        // Shred refuses a directory and leaves it in place.
        {
            let scratch = Scratch::new("dir");
            assert!(shred(&scratch.0).is_err());
            assert!(scratch.0.is_dir());
        }
    }

    #[cfg(unix)]
    #[test]
    fn shred_refuses_a_symlink_and_leaves_its_target_alone() {
        let scratch = Scratch::new("link");
        let target = scratch.file("target.env", b"KEEP=me\n");
        let link = scratch.0.join("link.env");
        std::os::unix::fs::symlink(&target, &link).expect("make the link");
        let err = shred(&link).unwrap_err();
        assert!(err.contains("symbolic link"), "{err}");
        assert_eq!(
            std::fs::read(&target).expect("target still there"),
            b"KEEP=me\n"
        );
        assert!(
            std::fs::symlink_metadata(&link)
                .expect("link still there")
                .file_type()
                .is_symlink()
        );
    }
}
