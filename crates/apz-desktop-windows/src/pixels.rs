//! Pixel order. Desktop Duplication and GDI both hand out 32-bit BGRA rows; a frame is RGBA.

/// Appends the RGBA form of one BGRA row to `out`. Alpha is forced to opaque: GDI leaves it zero, and a frame
/// with zero alpha shows as transparent.
pub fn bgra_row_to_rgba(row: &[u8], out: &mut Vec<u8>) {
    for pixel in row.as_chunks::<4>().0 {
        out.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgra_rows_become_rgba_with_opaque_alpha() {
        for (case, row, want) in [
            (
                "blue and red swap and alpha is opaque",
                vec![1, 2, 3, 0, 4, 5, 6, 9],
                vec![3, 2, 1, 255, 6, 5, 4, 255],
            ),
            ("a partial pixel is ignored", vec![1, 2, 3], vec![]),
        ] {
            let mut out = Vec::new();
            bgra_row_to_rgba(&row, &mut out);
            assert_eq!(out, want, "{case}");
        }
    }
}
