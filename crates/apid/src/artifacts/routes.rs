//! The owner's artifact routes: `/v1/artifacts…`.

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_view::artifact::Artifact;
use apz_view::{Body, Cell, XAxis};
use serde_json::json;

use crate::deliverables::served;
use crate::state::App;

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/artifacts", list_route)
        .doc(RouteDoc::new("List the artifacts, newest first (?project=, ?q=)"))
        .get("/v1/artifacts/{id}", get_route)
        .doc(RouteDoc::new("Get an artifact, or one of its versions (?v=)").errors(&[400, 404]))
        .get("/v1/artifacts/{id}/versions", versions_route)
        .doc(RouteDoc::new("List an artifact's versions").errors(&[404]))
        .get("/v1/artifacts/{id}/blobs/{blob}", blob_route)
        .doc(RouteDoc::new("Get a data blob that an artifact references").errors(&[404]))
        .get("/v1/artifacts/{id}/rows", rows_route)
        .doc(
            RouteDoc::new("Page a dataset's rows (?panel=&offset=&limit=&sort=&desc=1&filter=)")
                .errors(&[400, 404]),
        )
        .get("/v1/artifacts/{id}/export", export_route)
        .doc(
            RouteDoc::new("Export a table, dataset or chart as CSV, a chart, bars or tiles panel as PNG (?panel=), or the artifact as JSON")
                .errors(&[400, 404]),
        )
        .delete("/v1/artifacts/{id}", delete_route)
        .doc(RouteDoc::new("Delete an artifact and the data only it uses").errors(&[404]))
}

/// `GET /v1/artifacts?project=&q=`: the summaries, newest first.
fn list_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let project = query(req, "project").filter(|p| !p.is_empty());
    let q = query(req, "q").filter(|q| !q.is_empty());
    Response::json(200, &app.artifacts.list(project.as_deref(), q.as_deref()))
}

/// `GET /v1/artifacts/{id}?v=`: the artifact at version `v` (the newest when absent).
fn get_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let version = match version_param(req) {
        Ok(version) => version,
        Err(e) => return Response::error(400, &e),
    };
    match app.artifacts.get(p.get("id"), version) {
        Some(artifact) => Response::json(200, &artifact),
        None => Response::error(404, "no such artifact or version"),
    }
}

/// `GET /v1/artifacts/{id}/versions`: the versions, oldest first.
fn versions_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    if !app.artifacts.exists(id) {
        return Response::error(404, "no such artifact");
    }
    Response::json(200, &app.artifacts.versions(id))
}

/// `GET /v1/artifacts/{id}/blobs/{blob}`: a data blob one of the artifact's versions references, served as the
/// deliverables serve their files (sandboxed when it is HTML).
fn blob_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let blob = p.get("blob");
    match app.artifacts.artifact_blob(p.get("id"), blob) {
        Some((content_type, bytes)) => {
            let (content_type, headers) = served(&content_type, blob, false);
            headers.into_iter().fold(
                Response::new(200, &content_type, bytes),
                |response, (name, value)| response.with_header(name, &value),
            )
        }
        None => Response::error(404, "no such blob"),
    }
}

/// `GET /v1/artifacts/{id}/rows?panel=&offset=&limit=&sort=&desc=1&filter=&v=`: one page of a dataset's rows.
fn rows_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    if !app.artifacts.exists(id) {
        return Response::error(404, "no such artifact");
    }
    let version = match version_param(req) {
        Ok(version) => version,
        Err(e) => return Response::error(400, &e),
    };
    let number = |key: &str, default: u64| {
        query(req, key)
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(default)
    };
    let sort = query(req, "sort").filter(|s| !s.is_empty());
    let desc = query(req, "desc").as_deref() == Some("1");
    let filter = query(req, "filter").filter(|f| !f.is_empty());
    let panel = query(req, "panel").unwrap_or_default();
    let sort = sort.as_deref().map(|key| (key, desc));
    match app.artifacts.rows(
        id,
        version,
        &panel,
        number("offset", 0),
        number("limit", 100),
        sort,
        filter.as_deref(),
    ) {
        Ok(page) => Response::json(200, &page),
        Err(e) => Response::error(400, &e),
    }
}

/// `GET /v1/artifacts/{id}/export?format=csv|json&panel=&v=`: a table, dataset or chart panel as CSV, or the whole
/// artifact as JSON, sent as an attachment with a file name made from its title.
fn export_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let version = match version_param(req) {
        Ok(version) => version,
        Err(e) => return Response::error(400, &e),
    };
    let Some(artifact) = app.artifacts.get(p.get("id"), version) else {
        return Response::error(404, "no such artifact or version");
    };
    let panel = query(req, "panel").filter(|s| !s.is_empty());
    match query(req, "format").as_deref().unwrap_or("json") {
        "json" => attachment(
            Response::json(200, &artifact),
            &format!("{}.json", file_stem(&artifact.title, None)),
        ),
        "csv" => {
            let Some(panel) = panel else {
                return Response::error(400, "a CSV export needs the panel (?panel=)");
            };
            match panel_table(app, &artifact, &panel) {
                Ok((header, rows)) => attachment(
                    Response::new(200, "text/csv; charset=utf-8", to_csv(&header, &rows)),
                    &format!("{}.csv", file_stem(&artifact.title, Some(&panel))),
                ),
                Err(e) => Response::error(400, &e),
            }
        }
        "png" => {
            let Some(panel) = panel else {
                return Response::error(400, "a PNG export needs the panel (?panel=)");
            };
            let dark = query(req, "mode").as_deref() == Some("dark");
            match panel_png(app, &artifact, &panel, dark) {
                Ok(bytes) => attachment(
                    Response::new(200, "image/png", bytes),
                    &format!("{}.png", file_stem(&artifact.title, Some(&panel))),
                ),
                Err(e) => Response::error(400, &e),
            }
        }
        other => Response::error(400, &format!("format is csv, json or png, not {other:?}")),
    }
}

/// A chart, bars or tiles panel as a PNG in the owner's current theme (light unless `dark`), drawn once in Rust by
/// `apz_view::raster` so both consoles and the share viewer download the same image. 960 by 540 CSS px at 2×.
pub(crate) fn panel_png(
    app: &App,
    artifact: &apz_view::artifact::Artifact,
    panel: &str,
    dark: bool,
) -> Result<Vec<u8>, String> {
    let p = artifact
        .panels
        .iter()
        .find(|p| p.id == panel)
        .ok_or_else(|| format!("no panel {panel:?}"))?;
    let doc = app.appearance.get();
    let tokens = doc.choice.resolve(&doc.themes, !dark).tokens;
    apz_view::raster::panel_png(p, &tokens, 960, 540, 2.0)
}

/// `DELETE /v1/artifacts/{id}`.
fn delete_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    if app.artifacts.delete(id) {
        Response::json(200, &json!({ "deleted": id }))
    } else {
        Response::error(404, "no such artifact")
    }
}

/// The header and the rows of a table, dataset or chart panel, as text. A chart's first column is its x axis.
pub(crate) fn panel_table(
    app: &App,
    artifact: &Artifact,
    panel: &str,
) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    let found = artifact
        .panels
        .iter()
        .find(|p| p.id == panel)
        .ok_or_else(|| format!("no panel {panel:?}"))?;
    match &found.body {
        Body::Dataset(_) => {
            let (columns, rows) = app.artifacts.dataset_rows(
                &artifact.id,
                Some(artifact.version),
                panel,
                u64::MAX,
            )?;
            Ok((
                columns.iter().map(|c| c.label.clone()).collect(),
                rows.iter()
                    .map(|r| r.iter().map(crate::artifacts::value_text).collect())
                    .collect(),
            ))
        }
        Body::Table(t) => Ok((
            t.columns.iter().map(|c| c.head.clone()).collect(),
            t.rows
                .iter()
                .map(|r| r.cells.iter().map(cell_text).collect())
                .collect(),
        )),
        Body::Chart(c) => {
            let n = c.series.iter().map(|s| s.values.len()).max().unwrap_or(0);
            let xs: Vec<String> = match &c.x {
                XAxis::Time { start_ms, step_ms } => (0..n)
                    .map(|i| {
                        start_ms
                            .saturating_add(step_ms.saturating_mul(i as u64))
                            .to_string()
                    })
                    .collect(),
                XAxis::Labels { labels } => (0..n)
                    .map(|i| labels.get(i).cloned().unwrap_or_default())
                    .collect(),
            };
            let mut header = vec!["x".to_string()];
            header.extend(c.series.iter().map(|s| s.name.clone()));
            let rows = (0..n)
                .map(|i| {
                    let mut row = vec![xs[i].clone()];
                    row.extend(c.series.iter().map(|s| {
                        s.values
                            .get(i)
                            .copied()
                            .flatten()
                            .map_or_else(String::new, |v| v.to_string())
                    }));
                    row
                })
                .collect();
            Ok((header, rows))
        }
        _ => Err(format!(
            "panel {panel:?} is not a table, dataset or chart, so it has no CSV"
        )),
    }
}

/// A table cell's text.
fn cell_text(cell: &Cell) -> String {
    match cell {
        Cell::Text { text }
        | Cell::Mono { text }
        | Cell::Num { text }
        | Cell::Status { text, .. }
        | Cell::Pill { text, .. }
        | Cell::Muted { text } => text.clone(),
        Cell::Named { name, id } => format!("{name} {id}"),
        Cell::Spark { values, .. } => values
            .iter()
            .map(f64::to_string)
            .collect::<Vec<_>>()
            .join(" "),
        Cell::Bar { parts } => parts
            .iter()
            .map(|(share, _, label)| format!("{label} {share}%"))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// RFC 4180 CSV with CRLF line ends: a field with a comma, quote or line break is quoted.
pub(crate) fn to_csv(header: &[String], rows: &[Vec<String>]) -> String {
    std::iter::once(header)
        .chain(rows.iter().map(Vec::as_slice))
        .map(|fields| {
            fields
                .iter()
                .map(|f| csv_field(f))
                .collect::<Vec<_>>()
                .join(",")
                + "\r\n"
        })
        .collect()
}

/// A CSV field, quoted when it holds a comma, quote or line break. A field that a spreadsheet would read as a formula
/// (it starts with `=`, `+`, `-`, `@`, a tab or a carriage return) gets a `'` in front, so an export cannot run one.
fn csv_field(field: &str) -> String {
    let guarded = if field.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{field}")
    } else {
        field.to_string()
    };
    if guarded.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", guarded.replace('"', "\"\""))
    } else {
        guarded
    }
}

/// The attachment headers around `response`.
fn attachment(response: Response, file: &str) -> Response {
    response.with_header(
        "content-disposition",
        &format!("attachment; filename=\"{file}\""),
    )
}

/// A file name from a title (and a panel): lower-case ASCII words joined by dashes, `artifact` when nothing is left.
fn file_stem(title: &str, panel: Option<&str>) -> String {
    let mut stem = slug(title);
    if let Some(panel) = panel {
        stem.push('-');
        stem.push_str(&slug(panel));
    }
    stem
}

fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out: String = out.trim_end_matches('-').chars().take(60).collect();
    if out.is_empty() {
        "artifact".to_string()
    } else {
        out
    }
}

/// The `v` query parameter: a version number, or none for the newest.
fn version_param(req: &Request) -> Result<Option<u32>, String> {
    match query(req, "v") {
        None => Ok(None),
        Some(v) => v
            .parse::<u32>()
            .map(Some)
            .map_err(|_| format!("v is a version number, not {v:?}")),
    }
}

/// A percent-decoded query parameter.
fn query(req: &Request, key: &str) -> Option<String> {
    req.query_param(key).map(decode)
}

/// Decodes `%XX` and `+` (a space) in a query value; a malformed escape is kept as it is.
fn decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cell_a_spreadsheet_would_read_as_a_formula_is_guarded() {
        assert_eq!(csv_field("=1+1"), "'=1+1");
        assert_eq!(csv_field("+SUM(A1)"), "'+SUM(A1)");
        assert_eq!(csv_field("-2"), "'-2");
        assert_eq!(csv_field("@cmd"), "'@cmd");
        assert_eq!(csv_field("\tx"), "'\tx");
        assert_eq!(csv_field("=\"a,b\""), "\"'=\"\"a,b\"\"\"");
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
    }
}
