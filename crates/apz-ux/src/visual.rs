//! Image diffs of screenshots (PLAN 22.8): a candidate picture against its baseline, pixel by pixel. A pixel changed when a
//! channel moved by more than the tolerance's channel threshold; the picture passes when the changed share of its compared
//! pixels is within the tolerance's ratio. Rectangles of the mask are not compared (a clock, a rotating banner). The diff
//! picture marks what changed, in red, and keeps the rest faded, as the evidence a finding cites.

use serde::Deserialize;

/// A decoded picture: RGBA, row by row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[u8; 4]>,
}

/// A rectangle in picture pixels: an ignored area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn contains(&self, x: u32, y: u32) -> bool {
        x >= self.x && y >= self.y && x - self.x < self.w && y - self.y < self.h
    }
}

/// How far a candidate may differ: a channel may move by `channel` steps (0 to 255) before its pixel counts as changed, and
/// the picture passes when at most `ratio` of its compared pixels changed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tolerance {
    pub channel: u8,
    pub ratio: f64,
}

/// The result of comparing a candidate with its baseline.
#[derive(Debug, Clone, PartialEq)]
pub struct Diff {
    /// The pixels compared (the picture's, less the mask).
    pub compared: u64,
    pub changed: u64,
    pub ratio: f64,
    pub passed: bool,
    /// Why it did not pass, as people read it; empty when it passed.
    pub reason: String,
}

/// Decodes a PNG to RGBA. Palette, grey and 16-bit pictures are expanded to eight-bit RGBA.
pub fn decode(bytes: &[u8]) -> Result<Raster, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("the picture is not a PNG: {e}"))?;
    let size = reader
        .output_buffer_size()
        .ok_or("the picture is too large to decode")?;
    let mut buf = vec![0; size];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("the picture does not decode: {e}"))?;
    let data = &buf[..info.buffer_size()];
    let (color, _) = reader.output_color_type();
    let pixels = match color {
        png::ColorType::Rgba => data.as_chunks::<4>().0.to_vec(),
        png::ColorType::Rgb => data
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => data.iter().map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("the picture was not expanded to colour".into()),
    };
    Ok(Raster {
        width: info.width,
        height: info.height,
        pixels,
    })
}

/// Encodes a picture as an eight-bit RGBA PNG.
pub fn encode(raster: &Raster) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, raster.width, raster.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("the picture's header: {e}"))?;
    let flat: Vec<u8> = raster.pixels.iter().flatten().copied().collect();
    writer
        .write_image_data(&flat)
        .map_err(|e| format!("the picture's data: {e}"))?;
    writer
        .finish()
        .map_err(|e| format!("the picture's end: {e}"))?;
    Ok(out)
}

/// Whether a pixel changed: some channel moved by more than `channel` steps.
fn changed(a: [u8; 4], b: [u8; 4], channel: u8) -> bool {
    a.iter()
        .zip(b.iter())
        .any(|(x, y)| x.abs_diff(*y) > channel)
}

/// The picture's size as `WxH`, for the reasons.
fn size(raster: &Raster) -> String {
    format!("{}x{}", raster.width, raster.height)
}

/// Compares `candidate` with `baseline` under `tolerance`, leaving out the `mask`'s rectangles. Pictures of different sizes
/// do not pass.
pub fn compare(baseline: &Raster, candidate: &Raster, tolerance: Tolerance, mask: &[Rect]) -> Diff {
    if baseline.width != candidate.width || baseline.height != candidate.height {
        return Diff {
            compared: 0,
            changed: 0,
            ratio: 1.0,
            passed: false,
            reason: format!(
                "the screenshot is {}, its baseline is {}",
                size(candidate),
                size(baseline)
            ),
        };
    }
    let (mut compared, mut differing) = (0u64, 0u64);
    for (i, (a, b)) in baseline.pixels.iter().zip(&candidate.pixels).enumerate() {
        let (x, y) = position(i, baseline.width);
        if mask.iter().any(|r| r.contains(x, y)) {
            continue;
        }
        compared += 1;
        if changed(*a, *b, tolerance.channel) {
            differing += 1;
        }
    }
    let ratio = if compared == 0 {
        0.0
    } else {
        differing as f64 / compared as f64
    };
    let passed = ratio <= tolerance.ratio;
    let reason = if passed {
        String::new()
    } else {
        format!(
            "{differing} of {compared} pixels changed ({:.4} of the picture; the tolerance is {:.4})",
            ratio, tolerance.ratio
        )
    };
    Diff {
        compared,
        changed: differing,
        ratio,
        passed,
        reason,
    }
}

/// The diff picture of two pictures of the same size: changed pixels red, masked ones blue, the rest the candidate's grey
/// at half strength.
pub fn diff_image(
    baseline: &Raster,
    candidate: &Raster,
    tolerance: Tolerance,
    mask: &[Rect],
) -> Result<Vec<u8>, String> {
    if baseline.width != candidate.width || baseline.height != candidate.height {
        return Err("the pictures differ in size, so there is no diff picture".into());
    }
    let pixels = baseline
        .pixels
        .iter()
        .zip(&candidate.pixels)
        .enumerate()
        .map(|(i, (a, b))| {
            let (x, y) = position(i, baseline.width);
            if mask.iter().any(|r| r.contains(x, y)) {
                [40, 90, 255, 255]
            } else if changed(*a, *b, tolerance.channel) {
                [255, 0, 0, 255]
            } else {
                let grey =
                    (0.299 * f64::from(b[0]) + 0.587 * f64::from(b[1]) + 0.114 * f64::from(b[2]))
                        / 2.0
                        + 127.0;
                let v = grey.round() as u8;
                [v, v, v, 255]
            }
        })
        .collect();
    encode(&Raster {
        width: baseline.width,
        height: baseline.height,
        pixels,
    })
}

/// The column and row of pixel `i` in a picture `width` pixels wide.
fn position(i: usize, width: u32) -> (u32, u32) {
    let width = width.max(1) as usize;
    ((i % width) as u32, (i / width) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `w` by `h` picture of one colour, with the rectangle `at` painted in `paint`.
    fn picture(w: u32, h: u32, base: [u8; 4], at: Option<(Rect, [u8; 4])>) -> Raster {
        let mut pixels = vec![base; (w * h) as usize];
        if let Some((rect, paint)) = at {
            for y in 0..h {
                for x in 0..w {
                    if rect.contains(x, y) {
                        pixels[(y * w + x) as usize] = paint;
                    }
                }
            }
        }
        Raster {
            width: w,
            height: h,
            pixels,
        }
    }

    const TIGHT: Tolerance = Tolerance {
        channel: 0,
        ratio: 0.0,
    };

    fn block() -> Rect {
        Rect {
            x: 10,
            y: 10,
            w: 20,
            h: 20,
        }
    }

    #[test]
    fn an_identical_picture_passes_through_the_png_codec() {
        let base = picture(
            64,
            48,
            [240, 240, 240, 255],
            Some((block(), [20, 20, 20, 255])),
        );
        let bytes = encode(&base).unwrap();
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, base, "the codec keeps every pixel");
        let diff = compare(&base, &decoded, TIGHT, &[]);
        assert!(diff.passed, "{diff:?}");
        assert_eq!(diff.changed, 0);
        assert_eq!(diff.compared, 64 * 48);
    }

    #[test]
    fn a_layout_shift_fails_the_image_diff() {
        // The baseline: a dark block at (10, 10). The candidate: the same block moved 3 pixels right, as a layout
        // regression moves a control.
        let base = picture(
            120,
            80,
            [255, 255, 255, 255],
            Some((block(), [20, 20, 20, 255])),
        );
        let shifted = Rect { x: 13, ..block() };
        let candidate = picture(
            120,
            80,
            [255, 255, 255, 255],
            Some((shifted, [20, 20, 20, 255])),
        );
        let tolerance = Tolerance {
            channel: 8,
            ratio: 0.001,
        };
        let diff = compare(&base, &candidate, tolerance, &[]);
        assert!(!diff.passed, "{diff:?}");
        assert_eq!(
            diff.changed,
            3 * 20 * 2,
            "a 3-pixel strip on each of the block's 20 rows, in both places"
        );
        assert!(diff.reason.contains("pixels changed"), "{}", diff.reason);
        let image = diff_image(&base, &candidate, tolerance, &[]).unwrap();
        let marked = decode(&image).unwrap();
        assert_eq!(
            marked.pixels[(10 * 120 + 10) as usize],
            [255, 0, 0, 255],
            "a changed pixel is red"
        );
    }

    #[test]
    fn a_recoloured_candidate_fails_and_a_tolerated_change_passes() {
        let base = picture(60, 60, [255, 255, 255, 255], None);
        let dark = picture(60, 60, [0, 0, 0, 255], None);
        let tolerance = Tolerance {
            channel: 8,
            ratio: 0.5,
        };
        assert!(!compare(&base, &dark, tolerance, &[]).passed);
        // A colour nudged within the channel threshold is not a change at all.
        let nudged = picture(60, 60, [250, 251, 255, 255], None);
        let diff = compare(&base, &nudged, tolerance, &[]);
        assert!(diff.passed, "{diff:?}");
        assert_eq!(diff.changed, 0);
    }

    #[test]
    fn a_change_under_the_ratio_passes_and_over_it_fails() {
        // 20 by 20 pixels of 3600: 400 changed is 11.1 %.
        let base = picture(60, 60, [255, 255, 255, 255], None);
        let candidate = picture(
            60,
            60,
            [255, 255, 255, 255],
            Some((block(), [0, 0, 0, 255])),
        );
        let loose = Tolerance {
            channel: 0,
            ratio: 0.12,
        };
        let strict = Tolerance {
            channel: 0,
            ratio: 0.10,
        };
        assert!(compare(&base, &candidate, loose, &[]).passed);
        assert!(!compare(&base, &candidate, strict, &[]).passed);
    }

    #[test]
    fn a_masked_area_is_not_compared() {
        // The change sits in the mask (a clock in the corner): the picture passes at zero tolerance.
        let base = picture(60, 60, [255, 255, 255, 255], None);
        let candidate = picture(
            60,
            60,
            [255, 255, 255, 255],
            Some((block(), [0, 0, 0, 255])),
        );
        let mask = [Rect {
            x: 5,
            y: 5,
            w: 30,
            h: 30,
        }];
        let diff = compare(&base, &candidate, TIGHT, &mask);
        assert!(diff.passed, "{diff:?}");
        assert_eq!(diff.compared, 60 * 60 - 30 * 30);
        assert_eq!(diff.changed, 0);
        let image = decode(&diff_image(&base, &candidate, TIGHT, &mask).unwrap()).unwrap();
        assert_eq!(
            image.pixels[(6 * 60 + 6) as usize],
            [40, 90, 255, 255],
            "the mask is marked blue"
        );
    }

    #[test]
    fn pictures_of_different_sizes_do_not_pass() {
        let base = picture(60, 60, [255, 255, 255, 255], None);
        let other = picture(59, 60, [255, 255, 255, 255], None);
        let diff = compare(
            &base,
            &other,
            Tolerance {
                channel: 255,
                ratio: 1.0,
            },
            &[],
        );
        assert!(!diff.passed);
        assert!(diff.reason.contains("59x60"), "{}", diff.reason);
        assert!(diff_image(&base, &other, TIGHT, &[]).is_err());
    }

    #[test]
    fn bytes_that_are_not_a_png_are_refused() {
        assert!(decode(b"not a picture").is_err());
    }
}
