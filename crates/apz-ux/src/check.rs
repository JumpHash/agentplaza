//! One check run (PLAN 22.8, 22.9): a page at one size, theme and locale, read over the browser's DevTools session. The viewport,
//! the colour scheme and the locale are emulated, the page is loaded, and its picture is compared with the baseline
//! picture (the previous release's, when the caller has one). Then the page's snapshot is judged by the audit and the
//! layout rules. Pure over [`Devtools`]: the caller owns the browser, and the findings are its [`Issue`]s.

use base64::Engine as _;
use serde_json::json;

use crate::audit::audit;
use crate::layout::overflow;
use crate::probe::{self, Devtools};
use crate::spec::{Check, Spec};
use crate::visual::{self, Diff};
use crate::{Issue, Kind};

/// What one check found, and the pictures it made.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub label: String,
    /// The screenshot the check took (a PNG), kept as the page's picture for the next release's baseline.
    pub screenshot: Vec<u8>,
    /// The comparison with the baseline, when one was given.
    pub visual: Option<Diff>,
    /// The diff picture, when the pictures were compared and differ in size or content.
    pub diff: Option<Vec<u8>>,
    pub issues: Vec<Issue>,
}

impl Report {
    /// Whether the check passed: no issue of any kind.
    pub fn passed(&self) -> bool {
        self.issues.is_empty()
    }

    /// The issues of one kind.
    pub fn of(&self, kind: Kind) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(move |i| i.kind == kind)
    }
}

/// Runs `check` on the browser: emulates its size, theme and locale, loads its page, and judges it. `baseline` is the
/// previous release's picture of the same check (a PNG); without one there is no visual comparison.
pub fn run(
    dt: &mut dyn Devtools,
    spec: &Spec,
    check: &Check,
    baseline: Option<&[u8]>,
) -> Result<Report, String> {
    emulate(dt, check)?;
    dt.load(&check.url)?;
    let shot = dt.call("Page.captureScreenshot", json!({ "format": "png" }))?;
    let data = shot["data"].as_str().ok_or("the browser sent no picture")?;
    let screenshot = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| format!("the browser's picture: {e}"))?;
    let label = check.label();
    let mut issues = Vec::new();
    let mut visual_diff = None;
    let mut diff_picture = None;
    if let Some(base) = baseline {
        let tolerance = spec.tolerance();
        let reference = visual::decode(base)?;
        let candidate = visual::decode(&screenshot)?;
        let diff = visual::compare(&reference, &candidate, tolerance, &spec.visual.mask);
        if !diff.passed {
            issues.push(Issue::new(
                Kind::Visual,
                "pixel-diff",
                label.clone(),
                diff.reason.clone(),
            ));
            diff_picture =
                visual::diff_image(&reference, &candidate, tolerance, &spec.visual.mask).ok();
        }
        visual_diff = Some(diff);
    }
    let snapshot = probe::snapshot(dt)?;
    issues.extend(audit(&snapshot));
    issues.extend(overflow(&snapshot));
    Ok(Report {
        label,
        screenshot,
        visual: visual_diff,
        diff: diff_picture,
        issues,
    })
}

/// The emulation a check runs under: its size, its colour scheme, its locale.
fn emulate(dt: &mut dyn Devtools, check: &Check) -> Result<(), String> {
    dt.call(
        "Emulation.setDeviceMetricsOverride",
        json!({
            "width": check.device.width,
            "height": check.device.height,
            "deviceScaleFactor": 1,
            "mobile": false,
        }),
    )?;
    dt.call(
        "Emulation.setEmulatedMedia",
        json!({ "media": "", "features": [{ "name": "prefers-color-scheme", "value": check.theme }] }),
    )?;
    dt.call(
        "Emulation.setLocaleOverride",
        json!({ "locale": check.locale }),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    use crate::spec::Spec;
    use crate::visual::{Raster, Rect};

    /// A browser that shows a fixed picture and a fixed tree, and keeps the commands it was sent.
    struct Browser {
        picture: Vec<u8>,
        tree: Value,
        sent: Vec<(String, Value)>,
    }

    impl Devtools for Browser {
        fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
            self.sent.push((method.to_string(), params));
            Ok(match method {
                "Page.captureScreenshot" => json!({
                    "data": base64::engine::general_purpose::STANDARD.encode(&self.picture)
                }),
                "Runtime.evaluate" => json!({ "result": { "value": "en" } }),
                "Accessibility.getFullAXTree" => self.tree.clone(),
                _ => json!({}),
            })
        }

        fn load(&mut self, _url: &str) -> Result<(), String> {
            Ok(())
        }
    }

    /// A white `w` by `h` picture with a dark block painted at `block` (or none).
    fn picture(w: u32, h: u32, block: Option<Rect>) -> Vec<u8> {
        let pixels = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                match block {
                    Some(r) if r.contains(x, y) => [20, 20, 20, 255],
                    _ => [255, 255, 255, 255],
                }
            })
            .collect();
        visual::encode(&Raster {
            width: w,
            height: h,
            pixels,
        })
        .unwrap()
    }

    fn spec() -> Spec {
        Spec::parse(
            r#"
[visual]
channel_threshold = 8
max_changed_ratio = 0.001

[[device]]
name = "phone"
width = 64
height = 48

[[page]]
name = "checkout"
path = "/checkout"
devices = ["phone"]
themes = ["dark"]
locales = ["de"]
"#,
        )
        .unwrap()
    }

    fn check(spec: &Spec) -> Check {
        spec.checks("http://127.0.0.1:9000").remove(0)
    }

    /// A page with one control, named or not.
    fn tree(name: &str) -> Value {
        json!({ "nodes": [
            { "backendDOMNodeId": 2, "role": {"value": "button"}, "name": {"value": name}, "ignored": false },
        ]})
    }

    #[test]
    fn the_check_emulates_its_size_theme_and_locale_before_it_looks() {
        let spec = spec();
        let mut browser = Browser {
            picture: picture(64, 48, None),
            tree: tree("Pay"),
            sent: Vec::new(),
        };
        let report = run(&mut browser, &spec, &check(&spec), None).unwrap();
        assert_eq!(report.label, "checkout phone dark de");
        assert!(report.visual.is_none(), "no baseline, no comparison");
        assert!(report.passed(), "{:?}", report.issues);
        let metrics = browser
            .sent
            .iter()
            .find(|(m, _)| m == "Emulation.setDeviceMetricsOverride")
            .unwrap();
        assert_eq!(
            (metrics.1["width"].as_u64(), metrics.1["height"].as_u64()),
            (Some(64), Some(48))
        );
        let media = browser
            .sent
            .iter()
            .find(|(m, _)| m == "Emulation.setEmulatedMedia")
            .unwrap();
        assert_eq!(media.1["features"][0]["value"], "dark");
        let locale = browser
            .sent
            .iter()
            .find(|(m, _)| m == "Emulation.setLocaleOverride")
            .unwrap();
        assert_eq!(locale.1["locale"], "de");
    }

    #[test]
    fn an_identical_picture_passes_the_visual_check() {
        let spec = spec();
        let baseline = picture(
            64,
            48,
            Some(Rect {
                x: 4,
                y: 4,
                w: 20,
                h: 10,
            }),
        );
        let mut browser = Browser {
            picture: baseline.clone(),
            tree: tree("Pay"),
            sent: Vec::new(),
        };
        let report = run(&mut browser, &spec, &check(&spec), Some(&baseline)).unwrap();
        assert!(report.passed(), "{:?}", report.issues);
        assert!(report.visual.as_ref().unwrap().passed);
        assert!(report.diff.is_none());
    }

    #[test]
    fn a_layout_regression_fails_the_image_diff_and_leaves_a_diff_picture() {
        let spec = spec();
        // The baseline has the block at (4, 4); the candidate moved it to (7, 4): a layout regression.
        let baseline = picture(
            64,
            48,
            Some(Rect {
                x: 4,
                y: 4,
                w: 20,
                h: 10,
            }),
        );
        let moved = picture(
            64,
            48,
            Some(Rect {
                x: 7,
                y: 4,
                w: 20,
                h: 10,
            }),
        );
        let mut browser = Browser {
            picture: moved,
            tree: tree("Pay"),
            sent: Vec::new(),
        };
        let report = run(&mut browser, &spec, &check(&spec), Some(&baseline)).unwrap();
        assert!(!report.passed());
        let visual: Vec<&Issue> = report.of(Kind::Visual).collect();
        assert_eq!(visual.len(), 1, "{:?}", report.issues);
        assert_eq!(visual[0].rule, "pixel-diff");
        assert_eq!(visual[0].element, "checkout phone dark de");
        assert!(
            report.diff.is_some(),
            "the diff picture is kept as evidence"
        );
        let marked = visual::decode(report.diff.as_ref().unwrap()).unwrap();
        assert!(
            marked.pixels.contains(&[255, 0, 0, 255]),
            "changed pixels are red"
        );
    }

    #[test]
    fn the_audit_and_the_layout_rules_run_on_the_same_page() {
        let spec = spec();
        let mut browser = Browser {
            picture: picture(64, 48, None),
            tree: tree(""),
            sent: Vec::new(),
        };
        let report = run(&mut browser, &spec, &check(&spec), None).unwrap();
        assert_eq!(
            report.of(Kind::Accessibility).count(),
            1,
            "{:?}",
            report.issues
        );
        assert_eq!(report.issues[0].rule, "control-name");
        assert_eq!(report.of(Kind::Layout).count(), 0);
    }
}
