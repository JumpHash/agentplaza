//! Static serving of the web app's files: `apps/web/*` only.
//!
//! The guard has two layers, because the console directory is written by
//! another process: a request may only name a bare file (the characters
//! `[A-Za-z0-9._-]`, no separators, not `.` or `..`), and the path it resolves
//! to must still be inside the console directory once symlinks are followed.
//! Anything else is a 404, the same answer an unknown file gets - a refusal
//! that told a prober which of its attempts was interesting would be a hint it
//! did not earn.

use std::fs;
use std::path::Path;

use crate::http::Response;

/// The file `GET /` serves.
pub const INDEX: &str = "index.html";

/// Whether `name` is a bare console file name the daemon will serve.
pub fn is_servable_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Serves `console_dir/name`, or a 404 JSON error when the name is not a bare
/// console file, when the file is not there, or when it resolves outside the
/// console directory.
pub fn serve(console_dir: &Path, name: &str) -> Response {
    if !is_servable_name(name) {
        return Response::error(404, &format!("no such file: {name}"));
    }
    let Ok(directory) = fs::canonicalize(console_dir) else {
        return Response::error(404, &format!("no such file: {name}"));
    };
    let Ok(path) = fs::canonicalize(directory.join(name)) else {
        return Response::error(404, &format!("no such file: {name}"));
    };
    if !path.starts_with(&directory) || !path.is_file() {
        return Response::error(404, &format!("no such file: {name}"));
    }
    match fs::read(&path) {
        Ok(body) => Response::file(content_type(name), body),
        Err(err) => Response::error(500, &format!("cannot read {name}: {err}")),
    }
}

/// The content type of a console file, by extension: html, css, js, svg, png
/// and ico are served as themselves, anything else as bytes. The console's
/// files are built with lowercase extensions.
pub fn content_type(name: &str) -> &'static str {
    match name.rsplit_once('.').map_or("", |(_, extension)| extension) {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_bare_console_names_are_servable() {
        for name in ["index.html", "app.js", "style.css", "logo.svg", "a-b_c.png", ".hidden"] {
            assert!(is_servable_name(name), "{name}");
        }
        for name in [
            "",
            ".",
            "..",
            "../Cargo.toml",
            "sub/app.js",
            "/etc/passwd",
            "app.js/",
            "%2e%2e",
            "app js",
            "app.js\r\n",
            "café.html",
        ] {
            assert!(!is_servable_name(name), "{name}");
        }
    }

    #[test]
    fn extensions_map_to_content_types() {
        assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(content_type("app.css"), "text/css; charset=utf-8");
        assert_eq!(content_type("app.js"), "text/javascript; charset=utf-8");
        assert_eq!(content_type("logo.svg"), "image/svg+xml");
        assert_eq!(content_type("favicon.png"), "image/png");
        assert_eq!(content_type("favicon.ico"), "image/x-icon");
        assert_eq!(content_type("notes.txt"), "application/octet-stream");
        assert_eq!(content_type("no-extension"), "application/octet-stream");
    }

    #[test]
    fn a_missing_console_directory_is_a_404() {
        let missing = std::env::temp_dir().join(format!(
            "noded-no-such-console-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&missing);
        let response = serve(&missing, INDEX);
        assert_eq!(response.status(), 404);
    }
}
