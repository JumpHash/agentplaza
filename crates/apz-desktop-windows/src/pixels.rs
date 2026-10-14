//! Pixel order. Desktop Duplication and GDI both hand out 32-bit BGRA rows; a frame is RGBA.

/// Appends the RGBA form of one BGRA row to `out`. Alpha is forced to opaque: GDI leaves it zero, and a frame
/// with zero alpha shows as transparent.
pub fn bgra_row_to_rgba(row: &[u8], out: &mut Vec<u8>) {
    for pixel in row.chunks_exact(4) {
        out.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blue_and_red_swap_and_alpha_is_opaque() {
        let mut out = Vec::new();
        bgra_row_to_rgba(&[1, 2, 3, 0, 4, 5, 6, 9], &mut out);
        assert_eq!(out, vec![3, 2, 1, 255, 6, 5, 4, 255]);
    }

    #[test]
    fn a_partial_pixel_is_ignored() {
        let mut out = Vec::new();
        bgra_row_to_rgba(&[1, 2, 3], &mut out);
        assert!(out.is_empty());
    }
}
