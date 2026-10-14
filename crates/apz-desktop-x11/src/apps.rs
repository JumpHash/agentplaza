//! Apps from the XDG `.desktop` entries: the directories, the entry parser, the `Exec` line with its field
//! codes, and the lookup by name or id. Pure apart from reading the directories.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use apz_desktop::{DesktopError, Result};

/// An application that can be launched: an application entry with a name and an `Exec` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DesktopEntry {
    /// The file name without `.desktop`; stable across launches.
    pub id: String,
    pub name: String,
    pub exec: String,
    pub path: PathBuf,
}

/// The application directories in precedence order: `$XDG_DATA_HOME/applications` (default
/// `~/.local/share`), then each of `$XDG_DATA_DIRS` (default `/usr/local/share:/usr/share`).
pub(crate) fn application_dirs(
    home: Option<&Path>,
    xdg_data_home: Option<&str>,
    xdg_data_dirs: Option<&str>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    match xdg_data_home.filter(|value| !value.is_empty()) {
        Some(value) => dirs.push(PathBuf::from(value).join("applications")),
        None => {
            if let Some(home) = home {
                dirs.push(home.join(".local/share/applications"));
            }
        }
    }
    let system = xdg_data_dirs
        .filter(|value| !value.is_empty())
        .unwrap_or("/usr/local/share:/usr/share");
    dirs.extend(
        system
            .split(':')
            .filter(|dir| !dir.is_empty())
            .map(|dir| Path::new(dir).join("applications")),
    );
    dirs
}

/// Reads the entry of a `.desktop` file. Only the `[Desktop Entry]` group counts. Hidden entries, entries that
/// are not applications, and entries without a name or an `Exec` line are not launchable and give `None`.
pub(crate) fn parse_desktop_entry(id: &str, path: &Path, text: &str) -> Option<DesktopEntry> {
    let mut in_entry = false;
    let mut kind = None;
    let mut name = None;
    let mut exec = None;
    let mut hidden = false;
    let mut no_display = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "Type" => kind = Some(value),
            "Name" => name = Some(value),
            "Exec" => exec = Some(value),
            "Hidden" => hidden = value == "true",
            "NoDisplay" => no_display = value == "true",
            _ => {}
        }
    }
    if kind != Some("Application") || hidden || no_display {
        return None;
    }
    let name = name.filter(|value| !value.is_empty())?;
    let exec = exec.filter(|value| !value.is_empty())?;
    Some(DesktopEntry {
        id: id.to_string(),
        name: name.to_string(),
        exec: exec.to_string(),
        path: path.to_path_buf(),
    })
}

/// Scans the directories for launchable entries. A file of a higher-precedence directory shadows the same
/// id in a lower one, even when the shadowing file is hidden.
pub(crate) fn scan(dirs: &[PathBuf]) -> Vec<DesktopEntry> {
    let mut seen = BTreeSet::new();
    let mut entries = Vec::new();
    for dir in dirs {
        let Ok(read) = fs::read_dir(dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = read
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "desktop"))
            .collect();
        files.sort();
        for path in files {
            let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if !seen.insert(id.to_string()) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            if let Some(entry) = parse_desktop_entry(id, &path, &text) {
                entries.push(entry);
            }
        }
    }
    entries
}

/// The entry an app name picks: an exact id, else a name that matches ignoring case, else an id that matches
/// ignoring case.
pub(crate) fn find<'a>(entries: &'a [DesktopEntry], app: &str) -> Option<&'a DesktopEntry> {
    entries
        .iter()
        .find(|entry| entry.id == app)
        .or_else(|| {
            entries
                .iter()
                .find(|entry| entry.name.eq_ignore_ascii_case(app))
        })
        .or_else(|| {
            entries
                .iter()
                .find(|entry| entry.id.eq_ignore_ascii_case(app))
        })
}

/// Splits an `Exec` line into words: spaces separate them, double quotes group them, and a backslash escapes
/// the next character.
pub(crate) fn split_exec(exec: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut has_word = false;
    let mut quoted = false;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let next = chars
                    .next()
                    .ok_or_else(|| invalid(exec, "a dangling backslash"))?;
                word.push(next);
                has_word = true;
            }
            '"' => {
                quoted = !quoted;
                has_word = true;
            }
            ' ' | '\t' if !quoted => {
                if has_word {
                    words.push(std::mem::take(&mut word));
                    has_word = false;
                }
            }
            other => {
                word.push(other);
                has_word = true;
            }
        }
    }
    if quoted {
        return Err(invalid(exec, "an unclosed quote"));
    }
    if has_word {
        words.push(word);
    }
    Ok(words)
}

/// The argv an `Exec` line launches with `files` (file paths or URLs). Field codes: `%f` and `%u` take the
/// first file, `%F` and `%U` take all of them, `%%` is a percent sign, and the rest (`%i`, `%c`, `%k`, …) are
/// dropped. When the line has no file code, the files are appended.
pub(crate) fn expand_exec(exec: &str, files: &[String]) -> Result<Vec<String>> {
    let mut argv = Vec::new();
    let mut saw_file_code = false;
    for word in split_exec(exec)? {
        if word == "%F" || word == "%U" {
            saw_file_code = true;
            argv.extend(files.iter().cloned());
            continue;
        }
        let mut expanded = String::new();
        let mut chars = word.chars();
        while let Some(c) = chars.next() {
            if c != '%' {
                expanded.push(c);
                continue;
            }
            match chars.next() {
                Some('%') => expanded.push('%'),
                Some(code @ ('f' | 'u' | 'F' | 'U')) => {
                    saw_file_code = true;
                    if let Some(first) = files.first() {
                        if code == 'f' || code == 'u' {
                            expanded.push_str(first);
                        } else {
                            expanded.push_str(&files.join(" "));
                        }
                    }
                }
                _ => {}
            }
        }
        if !expanded.is_empty() {
            argv.push(expanded);
        }
    }
    if !saw_file_code {
        argv.extend(files.iter().cloned());
    }
    if argv.is_empty() {
        return Err(invalid(exec, "no program to run"));
    }
    Ok(argv)
}

fn invalid(exec: &str, reason: &str) -> DesktopError {
    DesktopError::Invalid(format!("the Exec line \"{exec}\": {reason}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTRY: &str = "[Desktop Entry]\nType=Application\nName=Text Editor\nExec=editor --new %U\nIcon=edit\n\n[Desktop Action New]\nName=Ignored\nExec=ignored\n";

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apz-x11-apps-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    #[test]
    fn an_application_entry_is_launchable() {
        let entry =
            parse_desktop_entry("editor", Path::new("/x/editor.desktop"), ENTRY).expect("parses");
        assert_eq!(entry.id, "editor");
        assert_eq!(entry.name, "Text Editor");
        assert_eq!(entry.exec, "editor --new %U");
    }

    #[test]
    fn only_the_main_group_counts() {
        let entry = parse_desktop_entry("editor", Path::new("/x"), ENTRY).expect("parses");
        assert_eq!(entry.name, "Text Editor", "the action's Name is not taken");
    }

    #[test]
    fn hidden_non_application_and_incomplete_entries_are_not_launchable() {
        let p = Path::new("/x");
        assert!(
            parse_desktop_entry("a", p, "[Desktop Entry]\nType=Link\nName=A\nExec=a\n").is_none()
        );
        assert!(
            parse_desktop_entry(
                "a",
                p,
                "[Desktop Entry]\nType=Application\nName=A\nExec=a\nHidden=true\n"
            )
            .is_none()
        );
        assert!(
            parse_desktop_entry(
                "a",
                p,
                "[Desktop Entry]\nType=Application\nName=A\nExec=a\nNoDisplay=true\n"
            )
            .is_none()
        );
        assert!(
            parse_desktop_entry("a", p, "[Desktop Entry]\nType=Application\nExec=a\n").is_none()
        );
        assert!(
            parse_desktop_entry("a", p, "[Desktop Entry]\nType=Application\nName=A\n").is_none()
        );
    }

    #[test]
    fn a_localised_name_does_not_replace_the_name() {
        let text =
            "[Desktop Entry]\nType=Application\nName[de]=Editor\nName=Text Editor\nExec=ed\n";
        let entry = parse_desktop_entry("ed", Path::new("/x"), text).expect("parses");
        assert_eq!(entry.name, "Text Editor");
    }

    #[test]
    fn exec_splits_quoted_words_and_escapes() {
        assert_eq!(
            split_exec(r#"sh -c "echo \"hi there\"  done""#).expect("splits"),
            vec!["sh", "-c", r#"echo "hi there"  done"#]
        );
        assert_eq!(split_exec("a\tb   c").expect("splits"), vec!["a", "b", "c"]);
        assert!(split_exec(r#"a "unclosed"#).is_err());
    }

    #[test]
    fn field_codes_take_the_files_or_drop_out() {
        let files = vec!["/a.txt".to_string(), "/b.txt".to_string()];
        assert_eq!(
            expand_exec("editor --new %U", &files).expect("expands"),
            vec!["editor", "--new", "/a.txt", "/b.txt"]
        );
        assert_eq!(
            expand_exec("viewer %f %i %c", &files).expect("expands"),
            vec!["viewer", "/a.txt"]
        );
        assert_eq!(
            expand_exec("tool --name=%k%%", &[]).expect("expands"),
            vec!["tool", "--name=%"]
        );
    }

    #[test]
    fn files_are_appended_when_the_line_has_no_file_code() {
        let files = vec!["/a.txt".to_string()];
        assert_eq!(
            expand_exec("plain", &files).expect("expands"),
            vec!["plain", "/a.txt"]
        );
        assert_eq!(expand_exec("plain", &[]).expect("expands"), vec!["plain"]);
        assert!(
            expand_exec("%i", &[]).is_err(),
            "an empty command is refused"
        );
    }

    #[test]
    fn the_directories_follow_the_xdg_order() {
        let dirs = application_dirs(
            Some(Path::new("/home/u")),
            None,
            Some("/opt/share:/usr/share"),
        );
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/home/u/.local/share/applications"),
                PathBuf::from("/opt/share/applications"),
                PathBuf::from("/usr/share/applications"),
            ]
        );
        let explicit = application_dirs(Some(Path::new("/home/u")), Some("/data"), None);
        assert_eq!(explicit[0], PathBuf::from("/data/applications"));
        assert!(explicit.contains(&PathBuf::from("/usr/local/share/applications")));
    }

    #[test]
    fn a_hidden_user_entry_shadows_the_system_one() {
        let user = scratch("user");
        let system = scratch("system");
        fs::write(
            user.join("editor.desktop"),
            "[Desktop Entry]\nType=Application\nName=Mine\nExec=mine\nHidden=true\n",
        )
        .expect("writes");
        fs::write(system.join("editor.desktop"), ENTRY).expect("writes");
        fs::write(
            system.join("calc.desktop"),
            "[Desktop Entry]\nType=Application\nName=Calc\nExec=calc\n",
        )
        .expect("writes");
        let entries = scan(&[user.clone(), system.clone()]);
        assert_eq!(
            entries.len(),
            1,
            "the hidden override removes the system editor"
        );
        assert_eq!(entries[0].name, "Calc");
        let _ = fs::remove_dir_all(&user);
        let _ = fs::remove_dir_all(&system);
    }

    #[test]
    fn an_app_is_found_by_id_or_name() {
        let entries = vec![DesktopEntry {
            id: "firefox".into(),
            name: "Firefox Web Browser".into(),
            exec: "firefox".into(),
            path: PathBuf::from("/x"),
        }];
        assert!(find(&entries, "firefox").is_some());
        assert!(find(&entries, "firefox web browser").is_some());
        assert!(find(&entries, "FIREFOX").is_some());
        assert!(find(&entries, "chrome").is_none());
    }
}
