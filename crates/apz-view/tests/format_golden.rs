use apz_metrics::catalog::Unit;
use apz_view::format::{format, nice, time_label};
use serde_json::Value;

const GOLDEN: &str = include_str!("format_golden.json");

fn golden() -> Value {
    serde_json::from_str(GOLDEN).expect("format_golden.json parses as JSON")
}

fn rows<'a>(golden: &'a Value, key: &str) -> &'a Vec<Value> {
    golden[key]
        .as_array()
        .unwrap_or_else(|| panic!("format_golden.json has an array named {key}"))
}

#[test]
fn every_format_case_in_the_golden_file_prints_its_expected_text() {
    let golden = golden();
    for row in rows(&golden, "format") {
        let unit: Unit = serde_json::from_value(row[0].clone()).expect("a unit name");
        let value = row[1].as_f64().expect("a number");
        let expected = row[2].as_str().expect("expected text");
        assert_eq!(format(unit, value), expected, "{unit:?} of {value}");
    }
}

#[test]
fn a_non_finite_value_prints_an_em_dash_in_every_unit() {
    let units = [
        Unit::Count,
        Unit::Percent,
        Unit::Ms,
        Unit::Bytes,
        Unit::BytesPerSec,
        Unit::Mib,
        Unit::Usd,
        Unit::Tokens,
        Unit::TokensPerSec,
        Unit::Ratio,
    ];
    for unit in units {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(format(unit, value), "—", "{unit:?} of {value}");
        }
    }
}

#[test]
fn nice_returns_the_golden_axis_ceiling_for_each_maximum() {
    let golden = golden();
    for row in rows(&golden, "nice") {
        let max = row[0].as_f64().expect("a maximum");
        let expected = row[1].as_f64().expect("a ceiling");
        assert!(
            (nice(max) - expected).abs() < 1e-9,
            "nice({max}) is {expected}"
        );
    }
}

#[test]
fn time_labels_follow_the_golden_step_classes_at_each_offset() {
    let golden = golden();
    for row in rows(&golden, "time") {
        let t_ms = row[0].as_u64().expect("a time in milliseconds");
        let step_ms = row[1].as_u64().expect("a step in milliseconds");
        let offset_min = row[2].as_i64().expect("an offset in minutes") as i32;
        let expected = row[3].as_str().expect("expected label");
        assert_eq!(
            time_label(t_ms, step_ms, offset_min),
            expected,
            "time_label({t_ms}, {step_ms}, {offset_min})"
        );
    }
}
