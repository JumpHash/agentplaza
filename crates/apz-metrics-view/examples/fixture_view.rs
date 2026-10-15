//! Prints a tab of the fixture as JSON: `cargo run -p apz-metrics-view --example fixture_view -- <tab> <scope>
//! [range]`. For looking at what the consoles will be given.

use apz_metrics_view::ViewQuery;
use apz_metrics_view::compose::compose;
use apz_metrics_view::fixture::Fixture;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let tab = args.first().map(String::as_str).unwrap_or("overview");
    let scope = args.get(1).map(String::as_str).unwrap_or("global");
    let range = args.get(2).map(String::as_str).unwrap_or("24h");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mut q = ViewQuery::new(tab, scope, range);
    q.fixture = true;
    let view = compose(&q, &Fixture::new(now));
    println!(
        "{}",
        serde_json::to_string_pretty(&view).unwrap_or_default()
    );
}
