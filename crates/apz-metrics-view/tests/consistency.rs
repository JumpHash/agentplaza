//! The Metrics view's numbers agree with each other. Every check composes the fixture plaza, reads the formatted
//! values back into numbers, and compares them with a tolerance of 1 % plus half of the last printed digit of each
//! formatted value (the rounding the text itself carries). Summed panels compare against the tile they add up to.

use apz_metrics_view::compose::compose;
use apz_metrics_view::fixture::Fixture;
use apz_metrics_view::source::Source;
use apz_metrics_view::tabs::TABS;
use apz_metrics_view::{Body, Panel, Series, Tile, View, ViewQuery};

/// A fixed "now", so every run composes the same windows.
const NOW_MS: u64 = 1_790_000_000_000;

const RANGES: [&str; 7] = ["15m", "1h", "6h", "24h", "7d", "30d", "90d"];

/// The tolerance of every comparison, as a fraction of the larger value.
const TOLERANCE: f64 = 0.01;

/// A formatted value read back: its number, and half of the last printed digit (the rounding it carries).
#[derive(Clone, Copy, Debug)]
struct Shown {
    value: f64,
    half_step: f64,
}

/// Parses text such as `$39.5`, `44.0M`, `8.9k`, `-$1.2`, `75%`, `12 ms` or `3` into its number. A `k`, `M` or `B`
/// suffix scales the number; any other unit word is kept as it is.
fn read(text: &str) -> Shown {
    let trimmed = text.trim();
    let (negative, body) = match trimmed.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, trimmed),
    };
    let body = body.strip_prefix('$').unwrap_or(body);
    let number_end = body
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ','))
        .unwrap_or(body.len());
    let (number, rest) = body.split_at(number_end);
    assert!(!number.is_empty(), "no number in {text:?}");
    let digits: String = number.chars().filter(|c| *c != ',').collect();
    let decimals = digits.split_once('.').map_or(0, |(_, f)| f.len());
    let mantissa: f64 = digits
        .parse()
        .unwrap_or_else(|_| panic!("{text:?} is not a number"));
    let scale = match rest {
        "k" => 1e3,
        "M" => 1e6,
        "B" => 1e9,
        _ => 1.0,
    };
    let sign = if negative { -1.0 } else { 1.0 };
    Shown {
        value: sign * mantissa * scale,
        half_step: 0.5 * 10f64.powi(-(decimals as i32)) * scale,
    }
}

/// Like `read`, but an em dash (no data in the window) reads as zero. Only the hub box has no model calls.
fn read_or_zero(text: &str) -> Shown {
    if text.trim() == "—" {
        Shown {
            value: 0.0,
            half_step: 0.0,
        }
    } else {
        read(text)
    }
}

/// The sum of several formatted values, with their rounding added up.
fn sum(texts: &[&str]) -> Shown {
    texts.iter().map(|t| read_or_zero(t)).fold(
        Shown {
            value: 0.0,
            half_step: 0.0,
        },
        |acc, s| Shown {
            value: acc.value + s.value,
            half_step: acc.half_step + s.half_step,
        },
    )
}

/// The slack a comparison of `a` and `b` may differ by: 1 % of the larger value and both roundings.
fn slack(a: Shown, b: Shown) -> f64 {
    TOLERANCE * a.value.abs().max(b.value.abs()) + a.half_step + b.half_step
}

/// Two numbers agree within the tolerance.
fn assert_agrees(what: &str, a: Shown, b: Shown) {
    assert!(
        (a.value - b.value).abs() <= slack(a, b),
        "{what}: {} and {} differ by more than the tolerance",
        a.value,
        b.value
    );
}

/// `a` is at most `b`, within the tolerance.
fn assert_at_most(what: &str, a: Shown, b: Shown) {
    assert!(
        a.value <= b.value + slack(a, b),
        "{what}: {} is larger than {}",
        a.value,
        b.value
    );
}

fn compose_view(tab: &str, scope: &str, range: &str) -> View {
    let mut q = ViewQuery::new(tab, scope, range);
    q.fixture = true;
    compose(&q, &Fixture::new(NOW_MS))
}

fn panel<'a>(view: &'a View, id: &str) -> &'a Panel {
    view.panels
        .iter()
        .find(|p| p.id == id)
        .unwrap_or_else(|| panic!("the {} tab has no panel {id}", view.tab))
}

/// The first tile with this label, in any panel of the view.
fn tile<'a>(view: &'a View, label: &str) -> &'a Tile {
    view.panels
        .iter()
        .find_map(|p| match &p.body {
            Body::Tiles { tiles } => tiles.iter().find(|t| t.label == label),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the {} tab has no {label} tile", view.tab))
}

fn tile_shown(view: &View, label: &str) -> Shown {
    read(&tile(view, label).value)
}

/// The empty panels a view may show with the fixture's data: an intro that asks for pins on an unpinned dashboard,
/// and the infractions list, which is empty in a short window when no agent broke a rule in it.
const EXPECTED_EMPTY: &[(&str, &str)] = &[("dash", "intro"), ("agents", "infractions")];

/// The series of a chart panel, whichever way the panel carries them.
fn chart_series<'a>(panel: &'a Panel) -> &'a [Series] {
    match &panel.body {
        Body::Chart(chart) => &chart.series,
        other => panic!("panel {} is not a chart: {other:?}", panel.id),
    }
}

/// The total of a chart's series over every step, leaving out the series `keep` rejects.
fn chart_total(panel: &Panel, keep: impl Fn(&Series) -> bool) -> f64 {
    chart_series(panel)
        .iter()
        .filter(|s| keep(s))
        .flat_map(|s| s.values.iter().flatten())
        .sum()
}

/// Whether `word` appears in `text` as a whole word, so `inf` does not match `info`.
fn has_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + word.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// Every string in a JSON value (keys and values), checked for text that a broken number would print.
fn assert_no_broken_text(json: &serde_json::Value, where_: &str) {
    match json {
        serde_json::Value::String(s) => {
            for word in ["NaN", "inf", "Infinity", "undefined"] {
                assert!(!has_word(s, word), "{where_} prints {s:?}");
            }
        }
        serde_json::Value::Array(items) => {
            items.iter().for_each(|v| assert_no_broken_text(v, where_))
        }
        serde_json::Value::Object(map) => map.iter().for_each(|(k, v)| {
            assert_no_broken_text(&serde_json::Value::String(k.clone()), where_);
            assert_no_broken_text(v, where_)
        }),
        _ => {}
    }
}

/// The Tokens tab's tiles, treemap and chart agree with each other and with the Overview.
#[test]
fn the_tokens_tab_agrees_with_the_overview_and_with_itself() {
    let tokens = compose_view("tokens", "global", "24h");
    let overview = compose_view("overview", "global", "24h");

    // The Spend tile is the same number on both tabs.
    assert_agrees(
        "Spend tile, Tokens against Overview",
        tile_shown(&tokens, "Spend"),
        tile_shown(&overview, "Spend"),
    );

    // The spend share treemap's items add up to the Spend tile.
    let spend = tile_shown(&tokens, "Spend");
    let items = match &panel(&tokens, "spend-share").body {
        Body::Treemap { items, .. } => items,
        other => panic!("spend-share is not a treemap: {other:?}"),
    };
    let treemap_total: f64 = items.iter().map(|i| i.value).sum();
    assert_agrees(
        "spend share items against the Spend tile",
        Shown {
            value: treemap_total,
            half_step: 0.0,
        },
        spend,
    );

    // The input series of "Tokens by provider" add up to the Input tokens tile. The output series and the dashed
    // comparison total are not input.
    let input_total = chart_total(panel(&tokens, "tokens-by-group"), |s| {
        s.name != "output" && !s.name.starts_with("total ·")
    });
    assert_agrees(
        "input series over every step against the Input tokens tile",
        Shown {
            value: input_total,
            half_step: 0.0,
        },
        tile_shown(&tokens, "Input tokens"),
    );
}

/// The Overview's Spend by provider chart adds up to the Spend tile, and its header total says the same.
#[test]
fn the_overview_spend_by_provider_chart_adds_up_to_the_spend_tile() {
    let overview = compose_view("overview", "global", "24h");
    let spend_panel = panel(&overview, "spend-by-group");
    let chart_total_spend = chart_total(spend_panel, |_| true);
    let spend = tile_shown(&overview, "Spend");
    assert_agrees(
        "Spend by provider over every step against the Spend tile",
        Shown {
            value: chart_total_spend,
            half_step: 0.0,
        },
        spend,
    );
    let header = spend_panel.right.split(' ').next().unwrap_or_default();
    assert_agrees(
        "the Spend by provider header total against the Spend tile",
        read(header),
        spend,
    );
}

/// Each node's spend and calls are at most the global's, and the three nodes' calls add up to the global calls.
#[test]
fn the_node_scopes_fit_inside_the_global_scope() {
    let node_ids: Vec<String> = Fixture::new(NOW_MS)
        .scopes()
        .into_iter()
        .map(|s| s.id)
        .filter(|id| id.starts_with("node:"))
        .collect();
    assert_eq!(
        node_ids.len(),
        3,
        "the fixture plaza has three nodes: {node_ids:?}"
    );

    let global = compose_view("tokens", "global", "24h");
    let global_spend = tile_shown(&global, "Spend");
    let global_calls = tile_shown(&global, "Calls");

    let mut node_calls = Vec::new();
    for id in &node_ids {
        let node = compose_view("tokens", id, "24h");
        // A node with no model calls shows an em dash, which reads as zero here.
        let spend = read_or_zero(&tile(&node, "Spend").value);
        let calls = read_or_zero(&tile(&node, "Calls").value);
        assert_at_most(&format!("{id} spend against global"), spend, global_spend);
        assert_at_most(&format!("{id} calls against global"), calls, global_calls);
        node_calls.push(tile(&node, "Calls").value.clone());
    }
    let texts: Vec<&str> = node_calls.iter().map(String::as_str).collect();
    assert_agrees(
        "the nodes' calls summed against the global calls",
        sum(&texts),
        global_calls,
    );
}

/// Cold wakes are at most all wakes, and the waiting-queue peak is a whole, non-negative count.
#[test]
fn the_lifecycle_and_hub_counts_are_whole_and_ordered() {
    let lifecycle = compose_view("lifecycle", "global", "24h");
    assert_at_most(
        "cold wakes against wakes",
        tile_shown(&lifecycle, "Cold wakes"),
        tile_shown(&lifecycle, "Wakes"),
    );

    let hub = compose_view("hub", "global", "24h");
    let peak = &tile(&hub, "Wakes waiting, peak").value;
    assert!(
        peak.chars().all(|c| c.is_ascii_digit()),
        "the waiting peak is a whole count, not {peak:?}"
    );
    assert!(read(peak).value >= 0.0, "the waiting peak is not negative");
}

// The Agents table is not checked against the global spend: its cost column is "Cost / task", the P50 of each
// agent's cost per task (`work.cost_usd`), not a per-agent total, so the column cannot be summed to the window's
// spend. The table is still composed, and its text checked, by `every_tab_at_every_range_composes_without_broken_text`.

/// Every tab at every range composes for the global scope and for a node, with no panic, no NaN or infinity in any
/// text, and no panel body left empty when the fixture has data.
#[test]
fn every_tab_at_every_range_composes_without_broken_text() {
    for &(tab, _, _) in TABS {
        for range in RANGES {
            for scope in ["global", "node:n2"] {
                let view = compose_view(tab, scope, range);
                let json = serde_json::to_value(&view).expect("a view serializes");
                assert_no_broken_text(&json, &format!("{tab} {scope} {range}"));
                if scope != "global" {
                    continue;
                }
                assert_eq!(view.tab, tab, "the global {tab} tab falls back at {range}");
                for p in &view.panels {
                    if matches!(p.body, Body::Empty { .. }) {
                        assert!(
                            EXPECTED_EMPTY.contains(&(tab, p.id.as_str())),
                            "the {tab} tab's {} panel is empty at {range} with the fixture's data",
                            p.id
                        );
                    }
                }
            }
        }
    }
}
