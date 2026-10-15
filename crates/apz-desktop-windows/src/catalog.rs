//! The rules of the app catalogue: which scripting bridges an app offers, how a query names an app or an
//! executable, and how launch arguments are quoted.

use apz_station::Bridge;

/// The ProgID an executable registers its COM server under, for the apps the general adapter drives through COM.
pub fn progid_for_exe(exe: &str) -> Option<&'static str> {
    match exe.to_ascii_lowercase().as_str() {
        "excel.exe" => Some("Excel.Application"),
        "winword.exe" => Some("Word.Application"),
        "powerpnt.exe" => Some("PowerPoint.Application"),
        "photoshop.exe" => Some("Photoshop.Application"),
        _ => None,
    }
}

/// The scripting bridges an app offers: PowerShell always, and COM when its ProgID is registered.
pub fn bridges(com_registered: bool) -> Vec<Bridge> {
    let mut bridges = vec![Bridge::PowerShell];
    if com_registered {
        bridges.push(Bridge::Com);
    }
    bridges
}

/// Whether a query names an app, by its name or its executable (ASCII case ignored).
pub fn names_app(query: &str, name: &str, id: &str) -> bool {
    query.eq_ignore_ascii_case(name) || query.eq_ignore_ascii_case(id)
}

/// The file name of a Windows path: what follows the last `\` or `/`.
pub fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Whether a window's executable (a path or a file name) is the one a query asks for. The query may give the
/// executable with or without `.exe`.
pub fn same_exe(query: &str, executable: &str) -> bool {
    strip_exe(query).eq_ignore_ascii_case(strip_exe(file_name(executable)))
}

fn strip_exe(name: &str) -> &str {
    let cut = name.len().saturating_sub(4);
    match name.get(cut..) {
        Some(tail) if name.len() >= 4 && tail.eq_ignore_ascii_case(".exe") => &name[..cut],
        _ => name,
    }
}

/// Joins launch arguments into one command line, each quoted as [`quote_arg`] quotes it.
pub fn command_line(args: &[String]) -> String {
    args.iter()
        .map(|arg| quote_arg(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Quotes one argument when it is empty or holds a space, a tab or a quote, so that CommandLineToArgvW reads it
/// back as one argument. Backslashes before a quote are doubled, and the closing quote follows any trailing
/// backslashes doubled.
pub fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn office_and_photoshop_have_their_progids() {
        assert_eq!(progid_for_exe("EXCEL.EXE"), Some("Excel.Application"));
        assert_eq!(progid_for_exe("winword.exe"), Some("Word.Application"));
        assert_eq!(
            progid_for_exe("powerpnt.exe"),
            Some("PowerPoint.Application")
        );
        assert_eq!(
            progid_for_exe("Photoshop.exe"),
            Some("Photoshop.Application")
        );
        assert_eq!(progid_for_exe("notepad.exe"), None);
    }

    #[test]
    fn powershell_is_always_offered_and_com_only_when_registered() {
        assert_eq!(bridges(false), vec![Bridge::PowerShell]);
        assert_eq!(bridges(true), vec![Bridge::PowerShell, Bridge::Com]);
    }

    #[test]
    fn quote_arg_and_command_line_follow_the_windows_rules() {
        // Plain arguments are left alone, empty or spaced arguments are quoted,
        // quotes and backslashes before a quote are escaped, and trailing
        // backslashes are doubled before the closing quote.
        for (case, arg, want) in [
            ("plain arguments are left alone", "plain", "plain"),
            (
                "paths with backslashes are left alone",
                r"C:\dir\file",
                r"C:\dir\file",
            ),
            ("an empty argument is quoted", "", "\"\""),
            ("an argument with a space is quoted", "a b", "\"a b\""),
            (
                "quotes inside an argument are escaped",
                r#"say "hi""#,
                r#""say \"hi\"""#,
            ),
            (
                "a backslash before a quote is escaped",
                r#"a\"b"#,
                r#""a\\\"b""#,
            ),
            (
                "trailing backslashes are doubled before the closing quote",
                r"C:\my dir\",
                r#""C:\my dir\\""#,
            ),
        ] {
            assert_eq!(quote_arg(arg), want, "{case}");
        }
        // A command line joins quoted arguments with spaces.
        assert_eq!(command_line(&[]), "");
        assert_eq!(
            command_line(&["a b".to_string(), "c".to_string()]),
            "\"a b\" c"
        );
    }
}
