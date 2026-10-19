#![cfg(feature = "raster")]

use apz_theme::Tokens;
use apz_view::raster::{panel_pixmap, panel_png};
use apz_view::{
    BarRow, Body, Chart, Limit, Marker, Panel, Series, Status, Tile, Tone, Unit, XAxis,
};

fn tokens() -> Tokens {
    apz_theme::generate::resolved_builtins().remove(0).0.tokens
}

fn series(name: &str, tone: Tone, values: Vec<Option<f64>>) -> Series {
    Series {
        name: name.into(),
        tone,
        dash: false,
        values,
        gap_reason: None,
        scope: None,
    }
}

/// A three-series time chart with a gap, a marker and a limit line.
fn chart_panel() -> Panel {
    Panel {
        id: "latency".into(),
        title: "Latency".into(),
        sub: "p95 over the last hour".into(),
        right: "3 series".into(),
        span: 12,
        metric: None,
        live: false,
        pinned: None,
        actions: vec![],
        body: Body::Chart(Chart {
            unit: Unit::Ms,
            x: XAxis::Time {
                start_ms: 1_700_000_000_000,
                step_ms: 60_000,
            },
            series: vec![
                series(
                    "api",
                    Tone::C1,
                    vec![Some(120.0), Some(140.0), None, Some(180.0), Some(160.0)],
                ),
                series(
                    "worker",
                    Tone::C2,
                    vec![Some(80.0), Some(90.0), Some(95.0), Some(110.0), Some(105.0)],
                ),
                series(
                    "db",
                    Tone::C3,
                    vec![Some(40.0), Some(45.0), Some(50.0), Some(48.0), Some(52.0)],
                ),
            ],
            stacked: false,
            area: true,
            y_max: None,
            band: None,
            ghosts: vec![],
            markers: vec![Marker {
                index: 2,
                label: "deploy".into(),
            }],
            limit: Some(Limit {
                value: 150.0,
                label: "SLO".into(),
            }),
            height: 220,
            annotate: false,
            point_tab: None,
        }),
    }
}

fn with_body(body: Body) -> Panel {
    Panel {
        body,
        ..chart_panel()
    }
}

fn near(a: u8, b: u8) -> bool {
    (a as i32 - b as i32).abs() <= 1
}

#[test]
fn a_chart_panel_renders_a_png_at_its_width_and_height_times_the_scale() {
    let png = panel_png(&chart_panel(), &tokens(), 480, 280, 2.0).expect("a chart renders");
    assert_eq!(
        &png[..8],
        &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']
    );
    let w = u32::from_be_bytes(png[16..20].try_into().expect("four bytes"));
    let h = u32::from_be_bytes(png[20..24].try_into().expect("four bytes"));
    assert_eq!((w, h), (960, 560));
}

#[test]
fn a_pixel_in_the_ground_is_the_sheet_token() {
    let tokens = tokens();
    let pixmap = panel_pixmap(&chart_panel(), &tokens, 480, 280, 2.0).expect("a chart renders");
    let pixel = pixmap.pixel(2, 2).expect("the pixel is inside the pixmap");
    let sheet = tokens.get("sheet").to_u8();
    assert!(
        near(pixel.red(), sheet[0]),
        "red {} vs {}",
        pixel.red(),
        sheet[0]
    );
    assert!(
        near(pixel.green(), sheet[1]),
        "green {} vs {}",
        pixel.green(),
        sheet[1]
    );
    assert!(
        near(pixel.blue(), sheet[2]),
        "blue {} vs {}",
        pixel.blue(),
        sheet[2]
    );
    assert_eq!(pixel.alpha(), 255);
}

#[test]
fn a_body_other_than_chart_bars_or_tiles_is_refused() {
    let panel = with_body(Body::Empty {
        text: "nothing here".into(),
    });
    assert!(panel_png(&panel, &tokens(), 480, 280, 1.0).is_err());
}

#[test]
fn two_renders_of_one_panel_are_the_same_bytes() {
    let tokens = tokens();
    let a = panel_png(&chart_panel(), &tokens, 480, 280, 1.0).expect("renders");
    let b = panel_png(&chart_panel(), &tokens, 480, 280, 1.0).expect("renders");
    assert_eq!(a, b);
}

#[test]
fn bars_and_tiles_render_too() {
    let tokens = tokens();
    let bars = with_body(Body::Bars {
        rows: vec![
            BarRow {
                label: "api".into(),
                value: "82 %".into(),
                frac: 0.82,
                status: Status::Warn,
                note: "rising since 14:00".into(),
            },
            BarRow {
                label: "worker".into(),
                value: "41 %".into(),
                frac: 0.41,
                status: Status::Ok,
                note: String::new(),
            },
        ],
        note: "Share of the quota".into(),
    });
    assert!(panel_png(&bars, &tokens, 360, 200, 1.0).is_ok());

    let tiles = with_body(Body::Tiles {
        tiles: vec![Tile {
            label: "Requests".into(),
            value: "1.2 M".into(),
            change_pct: Some(-3.5),
            up_bad: false,
            note: "last 24 h".into(),
            spark: vec![1.0, 3.0, 2.0, 4.0],
            tone: Tone::C1,
            tab: None,
        }],
    });
    assert!(panel_png(&tiles, &tokens, 360, 200, 1.0).is_ok());
}
