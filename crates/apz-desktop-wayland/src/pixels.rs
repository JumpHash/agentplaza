//! Pixel formats of the PipeWire frames: conversion to the RGBA frames the desktop trait returns.

use apz_desktop::{DesktopError, Result};

/// The raw formats the stream is negotiated to, by the order of their bytes in memory. The `x` formats have a
/// padding byte where the others have alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Bgrx,
    Bgra,
    Rgbx,
    Rgba,
}

impl PixelFormat {
    /// Whether the first byte of a pixel is blue (BGRx, BGRA) rather than red (RGBx, RGBA).
    fn blue_first(self) -> bool {
        matches!(self, Self::Bgrx | Self::Bgra)
    }
}

/// Converts a frame of `width` by `height` pixels to RGBA, top row first, with every alpha set to 255 (a screen
/// has no transparency, and the padding byte of an `x` format is not alpha). `stride` is the number of bytes from
/// the start of one row to the next, which may be more than four bytes a pixel.
pub fn to_rgba(
    format: PixelFormat,
    width: u32,
    height: u32,
    stride: usize,
    data: &[u8],
) -> Result<Vec<u8>> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 {
        return Err(DesktopError::Invalid("a frame has no pixels".into()));
    }
    let row = w
        .checked_mul(4)
        .ok_or_else(|| DesktopError::Invalid("a frame row is too wide".into()))?;
    if stride < row {
        return Err(DesktopError::Invalid(format!(
            "a row of {row} bytes is shorter than its stride of {stride}"
        )));
    }
    let needed = stride
        .checked_mul(h - 1)
        .and_then(|rows| rows.checked_add(row))
        .ok_or_else(|| DesktopError::Invalid("a frame is too large".into()))?;
    if data.len() < needed {
        return Err(DesktopError::Invalid(format!(
            "a {width}x{height} frame needs {needed} bytes, not {}",
            data.len()
        )));
    }
    let (red, blue) = if format.blue_first() { (2, 0) } else { (0, 2) };
    let mut rgba = vec![0u8; row * h];
    for (y, out_row) in rgba.chunks_exact_mut(row).enumerate() {
        let in_row = &data[y * stride..y * stride + row];
        for (src, dst) in in_row.chunks_exact(4).zip(out_row.chunks_exact_mut(4)) {
            dst[0] = src[red];
            dst[1] = src[1];
            dst[2] = src[blue];
            dst[3] = 255;
        }
    }
    Ok(rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgrx_becomes_rgba_with_full_alpha() {
        // One pixel: blue 1, green 2, red 3, padding 9.
        let rgba = to_rgba(PixelFormat::Bgrx, 1, 1, 4, &[1, 2, 3, 9]).expect("converts");
        assert_eq!(rgba, vec![3, 2, 1, 255]);
    }

    #[test]
    fn rgba_keeps_its_channels_and_forces_alpha() {
        let rgba = to_rgba(PixelFormat::Rgba, 1, 1, 4, &[10, 20, 30, 0]).expect("converts");
        assert_eq!(rgba, vec![10, 20, 30, 255]);
    }

    #[test]
    fn bgra_and_rgbx_swap_the_same_way() {
        assert_eq!(
            to_rgba(PixelFormat::Bgra, 1, 1, 4, &[1, 2, 3, 4]).expect("converts"),
            vec![3, 2, 1, 255]
        );
        assert_eq!(
            to_rgba(PixelFormat::Rgbx, 1, 1, 4, &[1, 2, 3, 4]).expect("converts"),
            vec![1, 2, 3, 255]
        );
    }

    #[test]
    fn row_padding_is_skipped() {
        // Two rows of one pixel, with a stride of 8: four padding bytes after each pixel.
        let data = [1, 2, 3, 0, 9, 9, 9, 9, 4, 5, 6, 0];
        let rgba = to_rgba(PixelFormat::Rgbx, 1, 2, 8, &data).expect("converts");
        assert_eq!(rgba, vec![1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn a_short_buffer_and_a_bad_stride_are_refused() {
        assert!(to_rgba(PixelFormat::Bgrx, 2, 2, 8, &[0; 15]).is_err());
        assert!(to_rgba(PixelFormat::Bgrx, 2, 1, 4, &[0; 8]).is_err());
        assert!(to_rgba(PixelFormat::Bgrx, 0, 1, 0, &[]).is_err());
        assert!(to_rgba(PixelFormat::Bgrx, 1, 0, 4, &[]).is_err());
    }

    #[test]
    fn a_frame_of_the_right_size_is_exactly_the_pixels_times_four() {
        let data = vec![0u8; 3 * 2 * 4];
        let rgba = to_rgba(PixelFormat::Bgrx, 3, 2, 12, &data).expect("converts");
        assert_eq!(rgba.len(), 3 * 2 * 4);
    }
}
