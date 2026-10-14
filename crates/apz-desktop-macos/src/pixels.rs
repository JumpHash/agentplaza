//! Repacking a CoreGraphics image into the RGBA frame the trait returns.
//!
//! A display image's pixels are four bytes each, but their order depends on the image's bitmap info: a display is
//! usually BGRA (little-endian, alpha first), and other images may be RGBA or ARGB. [`layout`] reads that order from
//! the bitmap info, and [`repack`] copies the rows out of their padding into a tight RGBA buffer.

use apz_desktop::{DesktopError, Frame, Result};

/// `kCGBitmapByteOrderMask`.
const BYTE_ORDER_MASK: u32 = 0x7000;
/// `kCGBitmapByteOrder32Little`.
const BYTE_ORDER_32_LITTLE: u32 = 0x2000;
/// `kCGBitmapAlphaInfoMask`.
const ALPHA_MASK: u32 = 0x1F;

/// Where the red, green and blue bytes sit within each four-byte pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub red: usize,
    pub green: usize,
    pub blue: usize,
}

/// The layout of an image from its `CGBitmapInfo`.
///
/// Byte order and alpha position decide the order: a big-endian image stores the bytes of the pixel word from the
/// most significant down, a little-endian one from the least significant up.
pub fn layout(bitmap_info: u32) -> Layout {
    let little = bitmap_info & BYTE_ORDER_MASK == BYTE_ORDER_32_LITTLE;
    // kCGImageAlphaPremultipliedFirst (2), kCGImageAlphaFirst (4), kCGImageAlphaNoneSkipFirst (6).
    let alpha_first = matches!(bitmap_info & ALPHA_MASK, 2 | 4 | 6);
    match (little, alpha_first) {
        // Memory: A R G B. Word 0xAARRGGBB, big-endian.
        (false, true) => Layout {
            red: 1,
            green: 2,
            blue: 3,
        },
        // Memory: R G B A.
        (false, false) => Layout {
            red: 0,
            green: 1,
            blue: 2,
        },
        // Memory: B G R A. The usual display layout.
        (true, true) => Layout {
            red: 2,
            green: 1,
            blue: 0,
        },
        // Memory: A B G R.
        (true, false) => Layout {
            red: 3,
            green: 2,
            blue: 1,
        },
    }
}

/// Copies a `width` by `height` image out of `source`, whose rows are `bytes_per_row` apart, into an RGBA frame.
///
/// The alpha of every pixel is set to 255: a screen is opaque, and the alpha byte of a display image is not
/// meaningful (it is often padding).
pub fn repack(
    source: &[u8],
    width: usize,
    height: usize,
    bytes_per_row: usize,
    layout: Layout,
) -> Result<Frame> {
    let row = width
        .checked_mul(4)
        .ok_or_else(|| DesktopError::Invalid("the image is too wide".into()))?;
    if width == 0 || height == 0 {
        return Err(DesktopError::Invalid("the image has no pixels".into()));
    }
    if bytes_per_row < row {
        return Err(DesktopError::Invalid(format!(
            "a row of {width} pixels needs {row} bytes, not {bytes_per_row}"
        )));
    }
    let needed = bytes_per_row * (height - 1) + row;
    if source.len() < needed {
        return Err(DesktopError::Invalid(format!(
            "a {width}x{height} image needs {needed} bytes, not {}",
            source.len()
        )));
    }
    let mut rgba = Vec::with_capacity(row * height);
    for y in 0..height {
        let start = y * bytes_per_row;
        for pixel in source[start..start + row].chunks_exact(4) {
            rgba.extend_from_slice(&[
                pixel[layout.red],
                pixel[layout.green],
                pixel[layout.blue],
                255,
            ]);
        }
    }
    let width = u32::try_from(width).map_err(|_| DesktopError::Invalid("too wide".into()))?;
    let height = u32::try_from(height).map_err(|_| DesktopError::Invalid("too tall".into()))?;
    Frame::new(width, height, rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The usual display: little-endian, alpha first.
    const DISPLAY_INFO: u32 = BYTE_ORDER_32_LITTLE | 2;

    #[test]
    fn the_display_layout_is_bgra() {
        assert_eq!(
            layout(DISPLAY_INFO),
            Layout {
                red: 2,
                green: 1,
                blue: 0,
            }
        );
    }

    #[test]
    fn each_byte_order_and_alpha_position_has_its_layout() {
        let at = |red, green, blue| Layout { red, green, blue };
        assert_eq!(layout(2), at(1, 2, 3), "big-endian, alpha first: ARGB");
        assert_eq!(layout(1), at(0, 1, 2), "big-endian, alpha last: RGBA");
        assert_eq!(
            layout(BYTE_ORDER_32_LITTLE | 1),
            at(3, 2, 1),
            "little, alpha last: ABGR"
        );
        assert_eq!(
            layout(BYTE_ORDER_32_LITTLE | 6),
            at(2, 1, 0),
            "none-skip first is still BGRA"
        );
    }

    #[test]
    fn repacking_reorders_bgra_into_rgba_and_drops_row_padding() {
        // Two pixels per row, one row of padding after each row (bytes_per_row 12, rows 8 bytes of pixels).
        // Pixel values are B,G,R,A in memory.
        let source = [
            10, 20, 30, 0, 40, 50, 60, 0, 99, 99, 99, 99, //
            70, 80, 90, 0, 1, 2, 3, 0, 99, 99, 99, 99,
        ];
        let frame = repack(&source, 2, 2, 12, layout(DISPLAY_INFO)).expect("repacks");
        assert_eq!((frame.w, frame.h), (2, 2));
        assert_eq!(
            frame.rgba,
            vec![
                30, 20, 10, 255, 60, 50, 40, 255, //
                90, 80, 70, 255, 3, 2, 1, 255,
            ]
        );
    }

    #[test]
    fn a_short_buffer_or_a_bad_stride_is_refused() {
        let layout = layout(DISPLAY_INFO);
        assert!(repack(&[0; 7], 2, 1, 8, layout).is_err());
        assert!(
            repack(&[0; 16], 2, 2, 4, layout).is_err(),
            "a row narrower than its pixels"
        );
        assert!(repack(&[0; 16], 0, 2, 8, layout).is_err());
        // Two rows of 8 bytes with a stride of 12 need 20 bytes, not 16.
        assert!(repack(&[0; 16], 2, 2, 12, layout).is_err());
    }
}
