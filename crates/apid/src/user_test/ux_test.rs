//! The UI and UX checks (PLAN 22.8, 22.9) on a real page: a local Chrome, driven through the web target's DevTools session
//! (`web::local`), checks a served page at a declared size, theme and locale. The test skips, printing why, when no Chrome
//! is installed (`CHROME` names one). The rules and the image diff are tested over fixtures in `apz-ux` itself.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::thread;

use apz_ux::check;
use apz_ux::spec::{Check, Spec};
use apz_ux::{Issue, Kind};

use super::web::{self, Web};

/// The clean page: a language, a named button and link, readable text that fits.
const CLEAN: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Shop</title></head>\
<body style=\"background:#fff;color:#000\"><h1>Your order</h1><p>Thanks for shopping with us.</p>\
<button>Pay</button> <a href=\"/dark\">Back to shop</a></body></html>";

/// The same page with the three seeded faults: an icon-only button with no name, faint text, and a line that runs past its
/// box in English.
const SEEDED: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Shop</title></head>\
<body style=\"background:#fff;color:#000\"><h1>Your order</h1><p style=\"color:#bbb\">Faint note</p>\
<button><span style=\"display:inline-block;width:12px;height:12px;background:#333\"></span></button>\
<div style=\"width:120px;white-space:nowrap\">A line that cannot wrap in its box</div></body></html>";

/// The clean page on a dark background: the picture a release's baseline would not match.
const DARK: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Shop</title></head>\
<body style=\"background:#000;color:#fff\"><h1>Your order</h1><p>Thanks for shopping with us.</p>\
<button>Pay</button> <a href=\"/\">Back to shop</a></body></html>";

/// Serves the pages on a local port (a thread per connection: a browser opens sockets it sends nothing on). Returns the
/// address.
fn serve(pages: &'static [(&'static str, &'static str)]) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            thread::spawn(move || answer(stream, pages));
        }
    });
    format!("http://{address}")
}

fn answer(mut stream: std::net::TcpStream, pages: &[(&str, &str)]) {
    let Ok(clone) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(clone);
    let mut request = String::new();
    if reader.read_line(&mut request).is_err() {
        return;
    }
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) | Err(_) => break,
            Ok(_) if header == "\r\n" || header == "\n" => break,
            Ok(_) => {}
        }
    }
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let (status, html) = match pages.iter().find(|(p, _)| *p == path) {
        Some((_, html)) => ("200 OK", *html),
        None => ("404 Not Found", "<!doctype html><title>Not found</title>"),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

/// The declaration the tests check against: one phone-sized desktop page in light and dark, English.
const DECLARED: &str = r#"
[visual]
channel_threshold = 8
max_changed_ratio = 0.001

[[device]]
name = "desktop"
width = 800
height = 600

[[page]]
name = "shop"
path = "/"
devices = ["desktop"]
themes = ["light"]
locales = ["en"]
"#;

/// Opens `url` in a local Chrome and runs the page's check on it. None when no Chrome is installed (the caller skips).
fn check_page(url: &str, baseline: Option<&[u8]>) -> Option<check::Report> {
    let chrome = match web::local::find_chrome() {
        Ok(chrome) => chrome,
        Err(why) => {
            eprintln!("skipped the UI checks' Chrome test: {why}");
            return None;
        }
    };
    let spec = Spec::parse(DECLARED).unwrap();
    let shop = Check {
        page: "shop".into(),
        url: url.into(),
        device: spec.devices[0].clone(),
        theme: "light".into(),
        locale: "en".into(),
    };
    let mut page: Web = web::local::open(&chrome, url).unwrap();
    Some(check::run(&mut page, &spec, &shop, baseline).expect("the check runs on the page"))
}

fn rules_of(issues: &[Issue]) -> Vec<&str> {
    issues.iter().map(|i| i.rule).collect()
}

#[test]
fn a_seeded_missing_label_is_found_on_a_real_page_and_the_clean_page_passes() {
    let clean_base = serve(&[("/", CLEAN), ("/dark", DARK)]);
    let Some(clean) = check_page(&format!("{clean_base}/"), None) else {
        return;
    };
    assert!(clean.passed(), "the clean page passes: {:?}", clean.issues);

    let seeded_base = serve(&[("/", SEEDED)]);
    let seeded = check_page(&format!("{seeded_base}/"), None).unwrap();
    let names: Vec<&Issue> = seeded
        .issues
        .iter()
        .filter(|i| i.rule == "control-name")
        .collect();
    assert_eq!(
        names.len(),
        1,
        "the unlabelled button is found: {:?}",
        seeded.issues
    );
    assert_eq!(names[0].kind, Kind::Accessibility);
    assert!(names[0].message.contains("button"), "{}", names[0].message);
}

#[test]
fn low_contrast_text_and_an_overflowing_line_are_found_on_a_real_page() {
    let base = serve(&[("/", SEEDED)]);
    let Some(report) = check_page(&format!("{base}/"), None) else {
        return;
    };
    let found = rules_of(&report.issues);
    assert!(found.contains(&"color-contrast"), "{:?}", report.issues);
    let contrast = report
        .issues
        .iter()
        .find(|i| i.rule == "color-contrast")
        .unwrap();
    assert!(
        contrast.message.contains("Faint note"),
        "{}",
        contrast.message
    );
    assert_eq!(contrast.kind, Kind::Accessibility);

    let overflow: Vec<&Issue> = report.of(Kind::Layout).collect();
    assert_eq!(overflow.len(), 1, "{:?}", report.issues);
    assert_eq!(overflow[0].rule, "text-overflow");
    assert!(
        overflow[0].message.contains("cannot wrap"),
        "{}",
        overflow[0].message
    );
}

#[test]
fn a_real_page_is_compared_with_its_own_picture_and_a_dark_one_fails() {
    let base = serve(&[("/", CLEAN), ("/dark", DARK)]);
    let Some(first) = check_page(&format!("{base}/"), None) else {
        return;
    };
    assert!(first.visual.is_none());
    let baseline = first.screenshot.clone();

    // The same page against its own picture: the screenshot is the browser's own, so it matches.
    let same = check_page(&format!("{base}/"), Some(&baseline)).unwrap();
    let diff = same.visual.as_ref().expect("compared");
    assert!(diff.passed, "the same page passes its picture: {diff:?}");
    assert!(same.passed(), "{:?}", same.issues);

    // The dark page against the light picture: the recoloured candidate fails the diff, and its diff picture is kept.
    let dark = check_page(&format!("{base}/dark"), Some(&baseline)).unwrap();
    assert!(!dark.visual.as_ref().unwrap().passed);
    let changed: Vec<&Issue> = dark.of(Kind::Visual).collect();
    assert_eq!(changed.len(), 1, "{:?}", dark.issues);
    assert_eq!(changed[0].rule, "pixel-diff");
    assert!(dark.diff.is_some(), "the diff picture is evidence");
}
