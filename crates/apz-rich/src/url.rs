//! URL recognition: which http(s) URLs are references. GitHub, the plaza's git host (the same route shapes,
//! Forgejo's `src/branch/...` as well as GitHub's `blob/...`), the hub's own library and file routes, and media
//! by extension. Everything else is a plain link. Pure.

use crate::{Ctx, Ref};

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "avif", "svg"];
const VIDEO_EXTENSIONS: &[&str] = &["mp4", "webm", "mov"];
const HTML_EXTENSIONS: &[&str] = &["html"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Forge {
    GitHub,
    Plaza,
}

/// The lowercase host of a URL or of a bare host, without userinfo or port. "" when there is none.
pub fn host_of(text: &str) -> String {
    let rest = strip_scheme(text).unwrap_or(text);
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    host_port
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// The URL without its scheme, when it has an http or https one (any case).
pub fn strip_scheme(url: &str) -> Option<&str> {
    for scheme in ["https://", "http://"] {
        let n = scheme.len();
        if url.len() >= n && url.is_char_boundary(n) && url[..n].eq_ignore_ascii_case(scheme) {
            return Some(&url[n..]);
        }
    }
    None
}

/// The URL without its query and fragment.
pub fn strip_query(url: &str) -> &str {
    let end = url.find(['?', '#']).unwrap_or(url.len());
    &url[..end]
}

/// What an http(s) URL refers to. `Link` for anything not recognised. None when `url` is not an http(s) URL with
/// a host.
pub fn classify_url(url: &str, ctx: &Ctx) -> Option<Ref> {
    let rest = strip_scheme(url)?;
    let (rest, fragment) = match rest.find('#') {
        Some(i) => (&rest[..i], Some(&rest[i + 1..])),
        None => (rest, None),
    };
    let location = match rest.find('?') {
        Some(i) => &rest[..i],
        None => rest,
    };
    let (authority, path) = match location.find('/') {
        Some(i) => (&location[..i], &location[i..]),
        None => (location, ""),
    };
    let host = host_of(authority);
    if host.is_empty() {
        return None;
    }
    let line = fragment.and_then(line_of_fragment);
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let trailing_slash = path.ends_with('/');
    let plaza = host_of(ctx.plaza_git_host.as_deref().unwrap_or(""));
    let hub = host_of(ctx.hub_host.as_deref().unwrap_or(""));

    // The first route that recognises the URL wins; the host decides which routes apply.
    let routed = (host == "gist.github.com")
        .then(|| gist(&segments))
        .flatten()
        .or_else(|| {
            (host == "github.com" || host == "www.github.com")
                .then(|| forge(&segments, Forge::GitHub, line, trailing_slash))
                .flatten()
        })
        .or_else(|| {
            (!plaza.is_empty() && host == plaza)
                .then(|| forge(&segments, Forge::Plaza, line, trailing_slash))
                .flatten()
        })
        .or_else(|| {
            (!hub.is_empty() && host == hub)
                .then(|| hub_route(&segments))
                .flatten()
        });
    Some(
        routed
            .or_else(|| media(url, &segments))
            .unwrap_or_else(|| Ref::Link {
                url: url.to_string(),
            }),
    )
}

/// A hub route of the library or the files: `/v1/files/<id>` and `/v1/library/<name>/<version>`, from the path's
/// segments (no host).
pub(crate) fn hub_route(segments: &[&str]) -> Option<Ref> {
    match segments {
        ["v1", "library", name, version, ..] if valid_name(name) => {
            version.parse::<u32>().ok().map(|version| Ref::LibraryTool {
                name: (*name).to_string(),
                version,
            })
        }
        ["v1", "files", id, ..] if valid_name(id) => Some(Ref::Library {
            id: (*id).to_string(),
        }),
        _ => None,
    }
}

fn gist(segments: &[&str]) -> Option<Ref> {
    let id = match segments {
        [id] => *id,
        [_user, id] => *id,
        _ => return None,
    };
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric()))
        .then(|| Ref::Gist { id: id.to_string() })
}

fn forge(segments: &[&str], host: Forge, line: Option<u32>, trailing_slash: bool) -> Option<Ref> {
    let (owner, repo, rest) = match segments {
        [owner, repo, rest @ ..] => (*owner, *repo, rest),
        _ => return None,
    };
    if !valid_name(owner) || !valid_name(repo) {
        return None;
    }
    let plaza = host == Forge::Plaza;
    let (owner, repo) = (owner.to_string(), repo.to_string());
    match rest {
        [] => Some(if plaza {
            Ref::PlazaGitRepo { owner, repo }
        } else {
            Ref::GithubRepo { owner, repo }
        }),
        ["pull" | "pulls", n] => number(n).map(|number| {
            if plaza {
                Ref::PlazaGitPull {
                    owner,
                    repo,
                    number,
                }
            } else {
                Ref::GithubPull {
                    owner,
                    repo,
                    number,
                }
            }
        }),
        ["issues" | "issue", n] => number(n).map(|number| {
            if plaza {
                Ref::PlazaGitIssue {
                    owner,
                    repo,
                    number,
                }
            } else {
                Ref::GithubIssue {
                    owner,
                    repo,
                    number,
                }
            }
        }),
        ["commit", sha] if is_hex(sha, 7, 40) => Some(if plaza {
            Ref::PlazaGitCommit {
                owner,
                repo,
                sha: (*sha).to_string(),
            }
        } else {
            Ref::GithubCommit {
                owner,
                repo,
                sha: (*sha).to_string(),
            }
        }),
        ["releases", "tag", tag] if !tag.is_empty() => Some(if plaza {
            Ref::PlazaGitRelease {
                owner,
                repo,
                tag: percent_decode(tag),
            }
        } else {
            Ref::GithubRelease {
                owner,
                repo,
                tag: percent_decode(tag),
            }
        }),
        ["actions", "runs", id] => number(id).map(|id| {
            if plaza {
                Ref::PlazaGitRun { owner, repo, id }
            } else {
                Ref::GithubRun { owner, repo, id }
            }
        }),
        ["blob", reference, path @ ..] if !plaza && !path.is_empty() => {
            Some(file(owner, repo, reference, path, false, line, plaza))
        }
        ["tree", reference, path @ ..] if !plaza => {
            Some(file(owner, repo, reference, path, true, None, plaza))
        }
        ["src", "branch" | "commit" | "tag", reference, path @ ..] if plaza => {
            let dir = path.is_empty() || trailing_slash;
            Some(file(
                owner,
                repo,
                reference,
                path,
                dir,
                if dir { None } else { line },
                plaza,
            ))
        }
        _ => None,
    }
}

fn file(
    owner: String,
    repo: String,
    reference: &str,
    path: &[&str],
    dir: bool,
    line: Option<u32>,
    plaza: bool,
) -> Ref {
    let reference = percent_decode(reference);
    let path = percent_decode(&path.join("/"));
    if plaza {
        Ref::PlazaGitFile {
            owner,
            repo,
            reference,
            path,
            line,
            dir,
        }
    } else {
        Ref::GithubFile {
            owner,
            repo,
            reference,
            path,
            line,
            dir,
        }
    }
}

/// Image, video and HTML files by the extension of the last path segment.
fn media(url: &str, segments: &[&str]) -> Option<Ref> {
    let name = segments.last()?;
    let (_, ext) = name.rsplit_once('.')?;
    let ext = ext.to_ascii_lowercase();
    let url = url.to_string();
    if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        Some(Ref::Image { url })
    } else if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        Some(Ref::Video { url })
    } else if HTML_EXTENSIONS.contains(&ext.as_str()) {
        Some(Ref::Html { url })
    } else {
        None
    }
}

/// `L10` or `L10-L20` (the fragment of a line link) gives 10.
fn line_of_fragment(fragment: &str) -> Option<u32> {
    let digits = fragment.strip_prefix('L')?;
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    digits[..end].parse().ok()
}

fn number(text: &str) -> Option<u64> {
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

fn is_hex(text: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A GitHub or Forgejo owner, repository or library name.
pub(crate) fn valid_name(text: &str) -> bool {
    !text.is_empty()
        && text != "."
        && text != ".."
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// Percent-decodes ASCII escapes (`%20`); a malformed escape is kept as it is.
pub(crate) fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match escape_at(bytes, i) {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The byte of a `%XX` escape starting at `i`, if there is one.
fn escape_at(bytes: &[u8], i: usize) -> Option<u8> {
    if bytes[i] != b'%' || i + 2 >= bytes.len() {
        return None;
    }
    Some(hex_value(bytes[i + 1])? * 16 + hex_value(bytes[i + 2])?)
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(url: &str) -> Ref {
        classify_url(url, &Ctx::default()).expect("an http(s) URL")
    }

    #[test]
    fn url_classification_routes() {
        // github_routes_and_their_fields: GitHub pull, file and directory routes carry their fields.
        {
            assert_eq!(
                plain("https://github.com/o/r/pull/12?diff=split#discussion_r1"),
                Ref::GithubPull {
                    owner: "o".into(),
                    repo: "r".into(),
                    number: 12
                }
            );
            assert_eq!(
                plain("https://github.com/o/r/blob/main/src/a.rs#L10-L20"),
                Ref::GithubFile {
                    owner: "o".into(),
                    repo: "r".into(),
                    reference: "main".into(),
                    path: "src/a.rs".into(),
                    line: Some(10),
                    dir: false
                }
            );
            assert_eq!(
                plain("https://github.com/o/r/tree/main/crates/"),
                Ref::GithubFile {
                    owner: "o".into(),
                    repo: "r".into(),
                    reference: "main".into(),
                    path: "crates".into(),
                    line: None,
                    dir: true
                }
            );
        }
        // host_and_scheme_are_case_insensitive_but_paths_are_not: only host and scheme ignore case.
        {
            assert!(matches!(
                plain("HTTPS://WWW.GitHub.com/o/r/pull/3"),
                Ref::GithubPull { number: 3, .. }
            ));
            assert!(matches!(
                plain("https://github.com/o/r/PULL/3"),
                Ref::Link { .. }
            ));
            assert_eq!(host_of("https://Git.Plaza.test:3000/x"), "git.plaza.test");
        }
        // media_and_hub_routes: media extensions, raw GitHub images and hub library tools.
        {
            assert!(matches!(plain("https://x.io/a.PNG?v=2"), Ref::Image { .. }));
            assert!(matches!(plain("https://x.io/a.mp4"), Ref::Video { .. }));
            assert!(matches!(plain("https://x.io/a.html#top"), Ref::Html { .. }));
            assert!(matches!(
                plain("https://raw.githubusercontent.com/o/r/main/a.png"),
                Ref::Image { .. }
            ));
            assert!(matches!(
                plain("https://github.com/o/r/blob/main/a.png"),
                Ref::GithubFile { .. }
            ));
            let ctx = Ctx {
                hub_host: Some("hub.test".into()),
                ..Ctx::default()
            };
            assert_eq!(
                classify_url("https://hub.test/v1/library/search/3/files", &ctx),
                Some(Ref::LibraryTool {
                    name: "search".into(),
                    version: 3
                })
            );
            assert_eq!(
                classify_url("https://other.test/v1/files/abc", &ctx),
                Some(Ref::Link {
                    url: "https://other.test/v1/files/abc".into()
                })
            );
        }
        // plaza_routes_use_the_configured_host_only: Plaza git routes need the configured host.
        {
            let ctx = Ctx {
                plaza_git_host: Some("git.plaza.test".into()),
                ..Ctx::default()
            };
            assert_eq!(
                classify_url(
                    "https://git.plaza.test/agentplaza/pr-1/src/branch/main/a.rs#L4",
                    &ctx
                ),
                Some(Ref::PlazaGitFile {
                    owner: "agentplaza".into(),
                    repo: "pr-1".into(),
                    reference: "main".into(),
                    path: "a.rs".into(),
                    line: Some(4),
                    dir: false
                })
            );
            assert!(matches!(
                plain("https://git.plaza.test/o/r/pulls/2"),
                Ref::Link { .. }
            ));
        }
        // invalid_names_and_non_http_urls_are_not_routes: bad schemes, empty hosts and odd names are not routes.
        {
            assert!(classify_url("ftp://github.com/o/r", &Ctx::default()).is_none());
            assert!(classify_url("https://", &Ctx::default()).is_none());
            assert!(matches!(
                plain("https://github.com/../r/pull/1"),
                Ref::Link { .. }
            ));
            assert!(matches!(
                plain("https://github.com/o/r/commit/xyz"),
                Ref::Link { .. }
            ));
        }
    }
}
