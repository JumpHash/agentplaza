//! Pure helpers on frames: scaling, the coordinate grid, encoding, and mapping screenshot points to screen points.

use apz_station::ImageFormat;
use image::{ExtendedColorType, ImageEncoder, RgbaImage, imageops::FilterType};

use crate::desktop::{DesktopError, Frame, Result};

/// The largest side a scaled frame may have.
const MAX_SIDE: u32 = 16384;

/// Glyphs of the digits 0 to 9 for the grid labels: 3 by 5 pixels, one row per entry, the left
/// column in bit 2.
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b010, 0b010, 0b010],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

/// Scales a frame by `factor` (a positive, finite number). The result is at least one pixel on each side.
pub fn scale(frame: &Frame, factor: f32) -> Result<Frame> {
    frame.check()?;
    if !(factor.is_finite() && factor > 0.0) {
        return Err(DesktopError::Invalid(format!(
            "a scale must be a positive number, not {factor}"
        )));
    }
    let w = ((frame.w as f32 * factor).round() as u32).max(1);
    let h = ((frame.h as f32 * factor).round() as u32).max(1);
    if w > MAX_SIDE || h > MAX_SIDE {
        return Err(DesktopError::Invalid(format!(
            "a scaled frame of {w}x{h} is larger than {MAX_SIDE} pixels a side"
        )));
    }
    let source = RgbaImage::from_raw(frame.w, frame.h, frame.rgba.clone())
        .ok_or_else(|| DesktopError::Invalid("the frame buffer does not match its size".into()))?;
    let resized = image::imageops::resize(&source, w, h, FilterType::Triangle);
    Frame::new(w, h, resized.into_raw())
}

/// Draws a coordinate grid over a frame: a one-pixel line every `step` pixels, and a label with its
/// coordinate at each line (columns along the top, rows down the left). Each line pixel is black over
/// a light background and white over a dark one, and every label sits on a black box, so the grid reads
/// on any screen. Steps smaller than a label are allowed; the labels then overlap.
pub fn grid_overlay(frame: &Frame, step: u32) -> Result<Frame> {
    frame.check()?;
    if step == 0 {
        return Err(DesktopError::Invalid(
            "a grid step is at least one pixel".into(),
        ));
    }
    let (w, h) = (frame.w, frame.h);
    let source = &frame.rgba;
    let mut out = source.clone();
    for x in (0..w).step_by(step as usize) {
        for y in 0..h {
            paint_line_pixel(&mut out, source, w, x, y);
        }
    }
    for y in (0..h).step_by(step as usize) {
        for x in 0..w {
            paint_line_pixel(&mut out, source, w, x, y);
        }
    }
    for x in (0..w).step_by(step as usize) {
        draw_label(&mut out, w, h, i64::from(x) + 2, 1, &x.to_string());
    }
    for y in (0..h).step_by(step as usize) {
        draw_label(&mut out, w, h, 1, i64::from(y) + 2, &y.to_string());
    }
    Frame::new(w, h, out)
}

/// Encodes a frame as PNG (lossless) or JPEG (`quality` 1 to 100; other values are clamped).
/// JPEG drops the alpha channel.
pub fn encode(frame: &Frame, format: ImageFormat, quality: u8) -> Result<Vec<u8>> {
    frame.check()?;
    let mut out = Vec::new();
    match format {
        ImageFormat::Png => image::codecs::png::PngEncoder::new(&mut out)
            .write_image(&frame.rgba, frame.w, frame.h, ExtendedColorType::Rgba8)
            .map_err(encode_error)?,
        ImageFormat::Jpeg => {
            let rgb: Vec<u8> = frame
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
                .collect();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100))
                .encode(&rgb, frame.w, frame.h, ExtendedColorType::Rgb8)
                .map_err(encode_error)?;
        }
    }
    Ok(out)
}

/// Maps a point in screenshot pixels to screen points. A screenshot taken at `scale` has every
/// screen point times `scale` pixels, so the screen point is the pixel divided by `scale`, rounded.
/// The caller validates `scale` (it is in (0, 1] for a screenshot); for a non-positive or non-finite
/// scale the point is returned unchanged rather than dividing by it.
pub fn map_point(x: i32, y: i32, scale: f32) -> (i32, i32) {
    if !(scale.is_finite() && scale > 0.0) {
        return (x, y);
    }
    (
        (x as f32 / scale).round() as i32,
        (y as f32 / scale).round() as i32,
    )
}

fn encode_error(error: image::ImageError) -> DesktopError {
    DesktopError::Encode(error.to_string())
}

/// Paints one grid line pixel, black over a light source pixel and white over a dark one.
fn paint_line_pixel(out: &mut [u8], source: &[u8], w: u32, x: u32, y: u32) {
    let index = ((y as usize) * (w as usize) + x as usize) * 4;
    let luminance = 0.299 * f32::from(source[index])
        + 0.587 * f32::from(source[index + 1])
        + 0.114 * f32::from(source[index + 2]);
    let value = if luminance >= 128.0 { 0 } else { 255 };
    out[index] = value;
    out[index + 1] = value;
    out[index + 2] = value;
}

/// Draws `text` (decimal digits) at `(x, y)` on a black box with white glyphs, clipped to the frame.
fn draw_label(out: &mut [u8], w: u32, h: u32, x: i64, y: i64, text: &str) {
    let width = 4 * text.len() as i64 + 1;
    fill_rect(out, (w, h), (x, y, width, 7), [0, 0, 0, 255]);
    for (index, digit) in text.bytes().enumerate() {
        let glyph = DIGITS[usize::from(digit - b'0')];
        for (row, &bits) in glyph.iter().enumerate() {
            for column in 0..3i64 {
                if (bits >> (2 - column)) & 1 == 1 {
                    let px = x + 1 + 4 * index as i64 + column;
                    let py = y + 1 + row as i64;
                    fill_rect(out, (w, h), (px, py, 1, 1), [255, 255, 255, 255]);
                }
            }
        }
    }
}

/// Fills the rectangle `(x, y, width, height)` of a `(w, h)` frame, clipped to the frame.
fn fill_rect(
    out: &mut [u8],
    (w, h): (u32, u32),
    (x, y, width, height): (i64, i64, i64, i64),
    color: [u8; 4],
) {
    for py in y.max(0)..(y + height).min(i64::from(h)) {
        for px in x.max(0)..(x + width).min(i64::from(w)) {
            let index = ((py as usize) * (w as usize) + px as usize) * 4;
            out[index..index + 4].copy_from_slice(&color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, color: [u8; 4]) -> Frame {
        let rgba = color
            .iter()
            .copied()
            .cycle()
            .take((w * h * 4) as usize)
            .collect();
        Frame::new(w, h, rgba).expect("a valid frame")
    }

    const WHITE: [u8; 4] = [255, 255, 255, 255];

    #[test]
    fn scaling_refuses_a_bad_factor_or_an_absurd_size() {
        let frame = solid(4, 4, WHITE);
        assert!(scale(&frame, 0.0).is_err());
        assert!(scale(&frame, -1.0).is_err());
        assert!(scale(&frame, f32::NAN).is_err());
        assert!(scale(&frame, f32::INFINITY).is_err());
        assert!(scale(&frame, 1.0e9).is_err());
    }

    #[test]
    fn a_zero_grid_step_is_refused() {
        assert!(grid_overlay(&solid(4, 4, WHITE), 0).is_err());
    }

    #[test]
    fn png_is_lossless_and_jpeg_is_a_jpeg() {
        let frame = solid(8, 6, [40, 90, 200, 255]);
        let png = encode(&frame, ImageFormat::Png, 80).expect("encodes");
        assert_eq!(&png[..4], &[0x89, b'P', b'N', b'G']);
        let decoded = image::load_from_memory(&png).expect("decodes").to_rgba8();
        assert_eq!((decoded.width(), decoded.height()), (8, 6));
        assert_eq!(decoded.into_raw(), frame.rgba);

        let jpeg = encode(&frame, ImageFormat::Jpeg, 80).expect("encodes");
        assert_eq!(&jpeg[..3], &[0xFF, 0xD8, 0xFF]);
        let decoded = image::load_from_memory(&jpeg).expect("decodes");
        assert_eq!((decoded.width(), decoded.height()), (8, 6));
    }

    #[test]
    fn encoding_refuses_a_frame_whose_buffer_is_the_wrong_size() {
        let broken = Frame {
            w: 4,
            h: 4,
            rgba: vec![0; 10],
        };
        assert!(encode(&broken, ImageFormat::Png, 80).is_err());
        assert!(encode(&broken, ImageFormat::Jpeg, 80).is_err());
    }

    #[test]
    fn a_screenshot_point_maps_to_the_screen_point() {
        assert_eq!(map_point(200, 100, 0.5), (400, 200));
        assert_eq!(map_point(3, 3, 0.3), (10, 10));
        assert_eq!(map_point(-40, 20, 1.0), (-40, 20));
        assert_eq!(
            map_point(10, 10, 0.0),
            (10, 10),
            "a bad scale is not divided by"
        );
    }
}
