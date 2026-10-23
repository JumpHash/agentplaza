//! Secret scanning of a change (PLAN 21.3, the CI recipe's `scan` step): the lines a change adds are read for credentials
//! of the kinds below. A finding names the file, the line and the kind, never the value. The scan is textual and small on
//! purpose: it stops the common leaks (a key pasted into code, a private key), and a project that wants more runs its own
//! scanner as a command.

/// A line a change adds: its file, its number in the file as it is after the change, and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    pub path: String,
    pub line: u32,
    pub text: String,
}

/// A credential the scan found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: String,
    pub line: u32,
    pub kind: &'static str,
}

/// Every credential among the added lines, in order.
pub fn scan(added: &[Added]) -> Vec<Finding> {
    added
        .iter()
        .filter_map(|a| {
            kind_of(&a.text).map(|kind| Finding {
                path: a.path.clone(),
                line: a.line,
                kind,
            })
        })
        .collect()
}

/// The kind of credential a line holds, if it holds one.
pub fn kind_of(line: &str) -> Option<&'static str> {
    if line.contains("-----BEGIN") && line.contains("PRIVATE KEY-----") {
        return Some("private key");
    }
    if run_after(line, "AKIA", 16, |c| {
        c.is_ascii_uppercase() || c.is_ascii_digit()
    }) {
        return Some("AWS access key");
    }
    let github = ["ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"];
    if github
        .iter()
        .any(|p| run_after(line, p, 30, |c| c.is_ascii_alphanumeric() || c == '_'))
    {
        return Some("GitHub token");
    }
    let slack = ["xoxb-", "xoxp-", "xoxa-", "xoxr-", "xoxs-"];
    if slack
        .iter()
        .any(|p| run_after(line, p, 10, |c| c.is_ascii_alphanumeric() || c == '-'))
    {
        return Some("Slack token");
    }
    if assigned_secret(line) {
        return Some("credential assignment");
    }
    None
}

/// Whether some occurrence of `prefix` is followed by at least `min` characters that all satisfy `ok`.
fn run_after(line: &str, prefix: &str, min: usize, ok: impl Fn(char) -> bool) -> bool {
    line.match_indices(prefix).any(|(at, _)| {
        line[at + prefix.len()..]
            .chars()
            .take_while(|c| ok(*c))
            .count()
            >= min
    })
}

/// A credential-named key set to a quoted value of at least 16 characters with letters and digits, such as
/// `password = "..."` or `api_key: '...'`. Placeholders (`${...}`, `<...>`) are not values.
fn assigned_secret(line: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "api_key", "apikey", "api-key", "secret", "token", "password", "passwd",
    ];
    let lower = line.to_ascii_lowercase();
    KEYWORDS.iter().any(|keyword| {
        lower.match_indices(keyword).any(|(at, _)| {
            let rest = &line[at + keyword.len()..];
            let rest = rest.trim_start_matches(|c: char| {
                c.is_ascii_alphanumeric()
                    || c == '_'
                    || c == '-'
                    || c == '"'
                    || c == '\''
                    || c == ' '
            });
            let Some(rest) = rest.strip_prefix(['=', ':']) else {
                return false;
            };
            let value: String = rest
                .trim_start_matches([' ', '"', '\''])
                .chars()
                .take_while(|c| !matches!(c, ' ' | '"' | '\'' | ',' | ';' | ')' | '`'))
                .collect();
            value.len() >= 16
                && value.chars().any(|c| c.is_ascii_alphabetic())
                && value.chars().any(|c| c.is_ascii_digit())
                && !value.contains("${")
                && !value.contains('<')
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_common_credentials_are_found_and_ordinary_code_is_not() {
        assert_eq!(
            kind_of("let key = \"AKIAIOSFODNN7EXAMPLE\";"),
            Some("AWS access key")
        );
        assert_eq!(
            kind_of("-----BEGIN OPENSSH PRIVATE KEY-----"),
            Some("private key")
        );
        let github = format!("const T: &str = \"ghp_{}\";", "x".repeat(36));
        assert_eq!(kind_of(&github), Some("GitHub token"));
        assert_eq!(
            kind_of("password = \"hunter2hunter2hunter2\""),
            Some("credential assignment")
        );
        assert_eq!(
            kind_of("api_key: 'sk-test-0123456789abcd'"),
            Some("credential assignment")
        );
        assert_eq!(kind_of("token: &str,"), None);
        assert_eq!(kind_of("fn token(&self) -> String {"), None);
        assert_eq!(kind_of("password = \"${DB_PASSWORD_FROM_ENV}\""), None);
        assert_eq!(kind_of("let n = AKIA;"), None);
    }

    #[test]
    fn a_finding_names_the_line_and_never_the_value() {
        let added = vec![
            Added {
                path: "src/a.rs".into(),
                line: 7,
                text: "let key = \"AKIAIOSFODNN7EXAMPLE\";".into(),
            },
            Added {
                path: "src/a.rs".into(),
                line: 8,
                text: "let x = 1;".into(),
            },
        ];
        let found = scan(&added);
        assert_eq!(
            found,
            vec![Finding {
                path: "src/a.rs".into(),
                line: 7,
                kind: "AWS access key"
            }]
        );
        assert!(!format!("{found:?}").contains("IOSFOD"));
    }
}
