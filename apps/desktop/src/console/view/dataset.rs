//! The dataset artifact body: a columnar table with a filter, sortable heads and 100 rows a page. Inline rows are
//! filtered, sorted and paged here ([`local_page`]). A dataset in a blob is paged by the hub: the screen fetches each
//! page with the state's offset, sort and filter, and stores it in [`DatasetState::page`].

use crate::theme::{Palette, TYPE_META, TYPE_UI, palette, skin};
use apz_view::{ColumnKind, DatasetBody, DatasetColumn, Unit};
use iced::alignment::{Horizontal, Vertical};
use iced::widget::{button, column, container, row, scrollable, text, text_input};
use iced::{Element, Length};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cmp::Ordering;

/// The rows a page shows.
pub const PAGE: u64 = 100;

/// One page of rows, the shape the hub's rows route answers with.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RowsPage {
    pub columns: Vec<DatasetColumn>,
    /// The rows of the page, each a list of values in column order.
    pub rows: Vec<Vec<Value>>,
    /// The rows that match the filter, across every page.
    pub total: u64,
    /// The index of the page's first row.
    pub offset: u64,
}

/// What the table shows: where its page starts, how it is sorted and filtered, and the page the hub gave for a blob.
#[derive(Debug, Clone, Default)]
pub struct DatasetState {
    /// The index of the first row of the page; a multiple of [`PAGE`] once paged.
    pub offset: u64,
    /// The sort column's key and whether it runs ascending (`true`); nothing when the rows keep their order.
    pub sort: Option<(String, bool)>,
    /// The text the rows must contain, in any cell, ignoring case.
    pub filter: String,
    /// The page the hub gave for a blob-backed dataset.
    pub page: Option<RowsPage>,
    /// Whether a page is on its way from the hub.
    pub loading: bool,
    /// The reason the last page did not load.
    pub error: Option<String>,
}

/// What a press in the table asks for.
#[derive(Debug, Clone)]
pub enum DatasetMsg {
    /// The filter text changed.
    Filter(String),
    /// A column head was pressed: sort by it, or flip the direction when it is already the sort.
    Sort(String),
    /// Move one page back (-1) or forward (+1).
    Page(i64),
}

/// The dataset: a toolbar with the filter and the row count, the column heads, a page of rows and the pager. The
/// rows come from the state's page for a blob, else from [`local_page`].
pub fn dataset<'a, M: Clone + 'a>(
    body: &'a DatasetBody,
    state: &'a DatasetState,
    on: impl Fn(DatasetMsg) -> M + Clone + 'a,
) -> Element<'a, M> {
    let p = *palette();
    let sk = skin();
    let remote = body.blob.is_some();
    let local = (!remote).then(|| local_page(body, state));
    let page: Option<&RowsPage> = if remote {
        state.page.as_ref()
    } else {
        local.as_ref()
    };
    let total = page.map_or(0, |pg| pg.total);
    let offset = page.map_or(state.offset, |pg| pg.offset);

    let count = if state.filter.trim().is_empty() {
        format!("{total} rows")
    } else {
        format!("{total} of {} rows", body.total)
    };
    let filter_on = on.clone();
    let filter = text_input("Filter rows", &state.filter)
        .on_input(move |s| filter_on(DatasetMsg::Filter(s)))
        .size(TYPE_UI)
        .style(crate::theme::input)
        .width(Length::Fill);
    let toolbar_parts: Vec<Element<'a, M>> = vec![
        filter.into(),
        text(count).size(TYPE_META).color(p.ink_3).into(),
    ];
    let toolbar = row(toolbar_parts).spacing(12).align_y(Vertical::Center);

    let mut head_cells: Vec<Element<'a, M>> = Vec::with_capacity(body.columns.len());
    for col in &body.columns {
        let arrow = match &state.sort {
            Some((key, ascending)) if *key == col.key => {
                if *ascending {
                    " ↑"
                } else {
                    " ↓"
                }
            }
            _ => "",
        };
        let on_sort = on.clone();
        let key = col.key.clone();
        let head = button(
            text(format!("{}{arrow}", sk.label_text(&col.label)))
                .font(sk.label_font())
                .size(sk.label_size)
                .color(p.ink_2),
        )
        .padding(0)
        .style(button::text)
        .on_press(on_sort(DatasetMsg::Sort(key)));
        head_cells.push(
            container(head)
                .width(Length::FillPortion(1))
                .align_x(align_of(col.kind))
                .into(),
        );
    }
    let heads = row(head_cells).spacing(12).padding([6, 0]);

    let mut rows: Vec<Element<'a, M>> = Vec::new();
    if let Some(pg) = page {
        for values in &pg.rows {
            rows.push(record(&body.columns, values, &p));
        }
    }
    let notice: Option<Element<'a, M>> = if let Some(error) = &state.error {
        Some(text(error.clone()).size(TYPE_META).color(p.crit).into())
    } else if remote && page.is_none() {
        let words = if state.loading {
            "Loading rows"
        } else {
            "No rows yet"
        };
        Some(text(words).size(TYPE_META).color(p.ink_3).into())
    } else if page.is_some_and(|pg| pg.rows.is_empty()) {
        Some(
            text("No rows match the filter")
                .size(TYPE_META)
                .color(p.ink_3)
                .into(),
        )
    } else {
        None
    };
    let list: Element<'a, M> = scrollable(column(rows).spacing(0))
        .height(Length::Fixed(480.0))
        .width(Length::Fill)
        .into();

    let range = if total == 0 {
        "No rows".to_string()
    } else {
        format!("{}–{} of {total}", offset + 1, (offset + PAGE).min(total))
    };
    let on_prev = on.clone();
    let on_next = on.clone();
    let prev = button(text("‹").size(TYPE_UI))
        .style(crate::theme::secondary_button)
        .on_press_maybe((offset > 0).then(|| on_prev(DatasetMsg::Page(-1))));
    let next = button(text("›").size(TYPE_UI))
        .style(crate::theme::secondary_button)
        .on_press_maybe((offset + PAGE < total).then(|| on_next(DatasetMsg::Page(1))));
    let pager_parts: Vec<Element<'a, M>> = vec![
        prev.into(),
        text(range).size(TYPE_UI).color(p.ink_2).into(),
        next.into(),
    ];
    let pager = row(pager_parts).spacing(8).align_y(Vertical::Center);

    let mut parts: Vec<Element<'a, M>> = vec![toolbar.into(), heads.into()];
    if let Some(notice) = notice {
        parts.push(notice);
    }
    parts.push(list);
    parts.push(pager.into());
    column(parts).spacing(8).into()
}

/// Applies a message to the state. Returns true when the screen must fetch a page from the hub, which is so for a
/// blob-backed dataset (the state is then marked loading); an inline dataset re-renders from its rows.
pub fn update(body: &DatasetBody, state: &mut DatasetState, msg: DatasetMsg) -> bool {
    let remote = body.blob.is_some();
    match msg {
        DatasetMsg::Filter(filter) => {
            state.filter = filter;
            state.offset = 0;
        }
        DatasetMsg::Sort(key) => {
            state.sort = Some(match state.sort.take() {
                Some((current, ascending)) if current == key => (current, !ascending),
                _ => (key, true),
            });
            state.offset = 0;
        }
        DatasetMsg::Page(step) => {
            let total = if remote {
                state.page.as_ref().map_or(0, |pg| pg.total)
            } else {
                matching(body, state).len() as u64
            };
            let moved = state.offset as i64 + step * PAGE as i64;
            state.offset = (moved.max(0) as u64).min(last_start(total));
        }
    }
    if remote {
        state.loading = true;
        state.error = None;
    }
    remote
}

/// The inline rows filtered, sorted and paged by the state, as the hub pages a blob. The filter is a case-insensitive
/// substring match over every cell. A sort is numeric only when every non-empty value in its column parses as a
/// number; blanks sort last in either direction.
pub fn local_page(body: &DatasetBody, state: &DatasetState) -> RowsPage {
    let matched = matching(body, state);
    let total = matched.len() as u64;
    let offset = state.offset.min(last_start(total));
    let rows = matched
        .iter()
        .skip(offset as usize)
        .take(PAGE as usize)
        .map(|&i| body.rows[i].clone())
        .collect();
    RowsPage {
        columns: body.columns.clone(),
        rows,
        total,
        offset,
    }
}

/// The indices of the rows that pass the filter, in the sort's order (the rows' own order when there is no sort).
fn matching(body: &DatasetBody, state: &DatasetState) -> Vec<usize> {
    let needle = state.filter.trim().to_lowercase();
    let mut rows: Vec<usize> = (0..body.rows.len())
        .filter(|&i| {
            needle.is_empty()
                || body.rows[i]
                    .iter()
                    .any(|v| cell_text(v).to_lowercase().contains(needle.as_str()))
        })
        .collect();
    let Some((key, ascending)) = &state.sort else {
        return rows;
    };
    let Some(col) = body.columns.iter().position(|c| &c.key == key) else {
        return rows;
    };
    let cell = |i: usize| body.rows[i].get(col);
    let numeric = body
        .rows
        .iter()
        .all(|r| blank(r.get(col)) || r.get(col).and_then(number).is_some());
    rows.sort_by(|&a, &b| {
        let (va, vb) = (cell(a), cell(b));
        match (blank(va), blank(vb)) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => {
                let ord = if numeric {
                    let x = va.and_then(number).unwrap_or(0.0);
                    let y = vb.and_then(number).unwrap_or(0.0);
                    x.total_cmp(&y)
                } else {
                    cell_text_of(va)
                        .to_lowercase()
                        .cmp(&cell_text_of(vb).to_lowercase())
                };
                if *ascending { ord } else { ord.reverse() }
            }
        }
    });
    rows
}

/// The first row index of the last page of `total` rows.
fn last_start(total: u64) -> u64 {
    total.saturating_sub(1) / PAGE * PAGE
}

/// A cell as a number, when it is one or text that reads as one.
fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok().filter(|n| n.is_finite()),
        _ => None,
    }
}

/// Whether a cell is missing, null or blank text.
fn blank(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.trim().is_empty(),
        Some(_) => false,
    }
}

/// A cell's text: the string itself, or the JSON text of any other value (null is empty).
fn cell_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The text of an optional cell, empty when it is missing.
fn cell_text_of(v: Option<&Value>) -> String {
    v.map(cell_text).unwrap_or_default()
}

/// A cell as it reads: a number in the column's unit when it has one, else its text.
fn display(v: &Value, unit: Option<Unit>) -> String {
    match (unit, number(v)) {
        (Some(unit), Some(n)) => apz_view::format::format(unit, n),
        _ => cell_text(v),
    }
}

/// Numbers sit right, everything else left.
fn align_of(kind: ColumnKind) -> Horizontal {
    if matches!(kind, ColumnKind::Number) {
        Horizontal::Right
    } else {
        Horizontal::Left
    }
}

/// One row of the table: a cell per column, a null as an em dash in the secondary ink.
fn record<'a, M: 'a>(columns: &[DatasetColumn], values: &[Value], p: &Palette) -> Element<'a, M> {
    let mut cells: Vec<Element<'a, M>> = Vec::with_capacity(columns.len());
    for (i, col) in columns.iter().enumerate() {
        let content: Element<'a, M> = match values.get(i) {
            Some(v) if !v.is_null() => text(display(v, col.unit)).size(TYPE_UI).color(p.ink).into(),
            _ => text("—").size(TYPE_UI).color(p.ink_3).into(),
        };
        cells.push(
            container(content)
                .width(Length::FillPortion(1))
                .align_x(align_of(col.kind))
                .into(),
        );
    }
    row(cells).spacing(12).padding([6, 0]).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn body(rows: Vec<Vec<Value>>) -> DatasetBody {
        let total = rows.len() as u64;
        DatasetBody {
            columns: vec![
                DatasetColumn {
                    key: "name".into(),
                    label: "Name".into(),
                    kind: ColumnKind::Text,
                    unit: None,
                },
                DatasetColumn {
                    key: "ms".into(),
                    label: "Milliseconds".into(),
                    kind: ColumnKind::Number,
                    unit: None,
                },
            ],
            rows,
            blob: None,
            total,
        }
    }

    fn names(page: &RowsPage) -> Vec<String> {
        page.rows.iter().map(|r| cell_text(&r[0])).collect()
    }

    #[test]
    fn filter_matches_any_cell_ignoring_case() {
        let b = body(vec![
            vec![json!("Ada"), json!(3)],
            vec![json!("Bob"), json!(12)],
            vec![json!("ada lovelace"), Value::Null],
        ]);
        let state = DatasetState {
            filter: "ADA".into(),
            ..DatasetState::default()
        };
        let page = local_page(&b, &state);
        assert_eq!(page.total, 2);
        assert_eq!(names(&page), vec!["Ada", "ada lovelace"]);
        let by_number = DatasetState {
            filter: "12".into(),
            ..DatasetState::default()
        };
        assert_eq!(names(&local_page(&b, &by_number)), vec!["Bob"]);
    }

    #[test]
    fn numeric_sort_puts_blanks_last_in_both_directions() {
        let b = body(vec![
            vec![json!("a"), json!(12)],
            vec![json!("b"), json!(3)],
            vec![json!("c"), Value::Null],
            vec![json!("d"), json!("7")],
        ]);
        let up = DatasetState {
            sort: Some(("ms".into(), true)),
            ..DatasetState::default()
        };
        assert_eq!(names(&local_page(&b, &up)), vec!["b", "d", "a", "c"]);
        let down = DatasetState {
            sort: Some(("ms".into(), false)),
            ..DatasetState::default()
        };
        assert_eq!(names(&local_page(&b, &down)), vec!["a", "d", "b", "c"]);
    }

    #[test]
    fn text_sort_ignores_case() {
        let b = body(vec![
            vec![json!("bob"), json!(1)],
            vec![json!("Ada"), json!(2)],
            vec![json!("cy"), json!(3)],
        ]);
        let state = DatasetState {
            sort: Some(("name".into(), true)),
            ..DatasetState::default()
        };
        assert_eq!(names(&local_page(&b, &state)), vec!["Ada", "bob", "cy"]);
    }

    #[test]
    fn paging_moves_by_hundreds_and_clamps_to_the_last_page() {
        let rows: Vec<Vec<Value>> = (0..250)
            .map(|i| vec![json!(format!("r{i}")), json!(i)])
            .collect();
        let b = body(rows);
        let mut state = DatasetState::default();
        assert!(!update(&b, &mut state, DatasetMsg::Page(1)));
        assert_eq!(state.offset, 100);
        let second = local_page(&b, &state);
        assert_eq!(
            (second.offset, second.total, second.rows.len()),
            (100, 250, 100)
        );
        update(&b, &mut state, DatasetMsg::Page(1));
        let third = local_page(&b, &state);
        assert_eq!((third.offset, third.rows.len()), (200, 50));
        update(&b, &mut state, DatasetMsg::Page(1));
        assert_eq!(
            state.offset, 200,
            "the last page is the furthest a page goes"
        );
        update(&b, &mut state, DatasetMsg::Page(-1));
        update(&b, &mut state, DatasetMsg::Page(-1));
        update(&b, &mut state, DatasetMsg::Page(-1));
        assert_eq!(state.offset, 0);
    }

    #[test]
    fn filter_and_sort_go_back_to_the_first_page() {
        let rows: Vec<Vec<Value>> = (0..250)
            .map(|i| vec![json!(format!("r{i}")), json!(i)])
            .collect();
        let b = body(rows);
        let mut state = DatasetState {
            offset: 200,
            ..DatasetState::default()
        };
        update(&b, &mut state, DatasetMsg::Filter("r1".into()));
        assert_eq!(state.offset, 0);
        state.offset = 100;
        update(&b, &mut state, DatasetMsg::Sort("ms".into()));
        assert_eq!(state.offset, 0);
        assert_eq!(state.sort, Some(("ms".into(), true)));
        update(&b, &mut state, DatasetMsg::Sort("ms".into()));
        assert_eq!(state.sort, Some(("ms".into(), false)));
    }

    #[test]
    fn a_blob_dataset_asks_the_screen_for_pages() {
        let mut b = body(Vec::new());
        b.blob = Some(apz_view::BlobRef {
            id: "b1".into(),
            content_type: "text/csv".into(),
            bytes: 10,
        });
        let mut state = DatasetState::default();
        assert!(update(&b, &mut state, DatasetMsg::Sort("name".into())));
        assert!(state.loading);
    }
}
