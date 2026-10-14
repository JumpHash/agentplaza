//! Screen capture: XShm `GetImage` of the root window, with plain `GetImage` as the fallback, converted to RGBA.
//!
//! The conversion ([`zpixmap_to_frame`]) is pure and tested here. The X calls take a connection and the root.

use apz_desktop::{DesktopError, Frame, Result};
use x11rb::connection::Connection;
use x11rb::protocol::shm::ConnectionExt as _;
use x11rb::protocol::xproto::{ConnectionExt as _, Window};
use x11rb::rust_connection::RustConnection;

/// `ZPixmap`, the image format that carries one pixel per `bits_per_pixel` bits.
const Z_PIXMAP: u8 = 2;

/// The byte order of the pixels of a ZPixmap image (the server's `image_byte_order`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ByteOrder {
    Lsb,
    Msb,
}

/// The channel masks of the root visual: which bits of a pixel hold each colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChannelMasks {
    pub red: u32,
    pub green: u32,
    pub blue: u32,
}

/// How the root window's pixels are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PixelFormat {
    pub bits_per_pixel: u32,
    pub order: ByteOrder,
    pub masks: ChannelMasks,
}

/// The bytes of one scanline of a ZPixmap image: each row is padded to a multiple of 32 bits.
pub(crate) fn bytes_per_line(width: u32, bits_per_pixel: u32) -> usize {
    (width as usize * bits_per_pixel as usize).div_ceil(32) * 4
}

/// Converts a ZPixmap image of `width` by `height` pixels to RGBA. Only 24- and 32-bit screens with 8-bit
/// channels are supported; the alpha channel is opaque.
pub(crate) fn zpixmap_to_frame(
    data: &[u8],
    width: u32,
    height: u32,
    format: PixelFormat,
) -> Result<Frame> {
    let bytes = match format.bits_per_pixel {
        24 => 3usize,
        32 => 4usize,
        other => {
            return Err(DesktopError::Unsupported(format!(
                "a {other}-bit screen (only 24- and 32-bit screens are captured)"
            )));
        }
    };
    for mask in [format.masks.red, format.masks.green, format.masks.blue] {
        let shift = mask.trailing_zeros();
        if mask == 0 || (mask >> shift) != 0xff {
            return Err(DesktopError::Unsupported(format!(
                "a channel mask of {mask:#x} (only 8-bit channels are captured)"
            )));
        }
    }
    let stride = bytes_per_line(width, format.bits_per_pixel);
    let needed = if height == 0 {
        0
    } else {
        stride * (height as usize - 1) + width as usize * bytes
    };
    if data.len() < needed {
        return Err(DesktopError::Platform(format!(
            "the image holds {} bytes, a {width}x{height} image needs {needed}",
            data.len()
        )));
    }
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height as usize {
        let row = &data[y * stride..];
        for x in 0..width as usize {
            let pixel = &row[x * bytes..(x + 1) * bytes];
            let value = match format.order {
                ByteOrder::Lsb => pixel
                    .iter()
                    .rev()
                    .fold(0u32, |acc, &byte| (acc << 8) | u32::from(byte)),
                ByteOrder::Msb => pixel
                    .iter()
                    .fold(0u32, |acc, &byte| (acc << 8) | u32::from(byte)),
            };
            for mask in [format.masks.red, format.masks.green, format.masks.blue] {
                rgba.push(((value & mask) >> mask.trailing_zeros()) as u8);
            }
            rgba.push(0xff);
        }
    }
    Frame::new(width, height, rgba)
}

/// A System V shared memory segment the X server writes captures into. The segment is removed as soon as the
/// server has attached it, so it lives until both sides detach.
pub(crate) struct ShmSegment {
    addr: usize,
    len: usize,
    seg: u32,
}

impl ShmSegment {
    /// Creates a segment of `len` bytes and attaches it to the server.
    fn attach(conn: &RustConnection, len: usize) -> Result<Self> {
        // SAFETY: plain System V calls. The id and the address are checked before use, and the segment is
        // private (IPC_PRIVATE), so no other process can collide with it.
        let id = unsafe { libc::shmget(libc::IPC_PRIVATE, len, libc::IPC_CREAT | 0o600) };
        if id < 0 {
            return Err(platform("shmget", std::io::Error::last_os_error()));
        }
        // SAFETY: as above; a failed attach returns (void*)-1, checked below.
        let addr = unsafe { libc::shmat(id, std::ptr::null(), 0) };
        if addr as isize == -1 {
            let error = std::io::Error::last_os_error();
            // SAFETY: the id is ours and no longer needed.
            unsafe { libc::shmctl(id, libc::IPC_RMID, std::ptr::null_mut()) };
            return Err(platform("shmat", error));
        }
        let segment = conn.generate_id().map_err(platform_error);
        let attached = segment.and_then(|seg| {
            conn.shm_attach(seg, id as u32, false)
                .map_err(platform_error)?
                .check()
                .map_err(platform_error)?;
            Ok(seg)
        });
        match attached {
            Ok(seg) => {
                // SAFETY: the id is ours; the server has attached, so removing it only frees the key.
                unsafe { libc::shmctl(id, libc::IPC_RMID, std::ptr::null_mut()) };
                Ok(Self {
                    addr: addr as usize,
                    len,
                    seg,
                })
            }
            Err(error) => {
                // SAFETY: the address and id came from the calls above and are released exactly once here.
                unsafe {
                    libc::shmdt(addr);
                    libc::shmctl(id, libc::IPC_RMID, std::ptr::null_mut());
                }
                Err(error)
            }
        }
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: the address is a live attachment of `len` bytes, owned by this value until Drop detaches it.
        unsafe { std::slice::from_raw_parts(self.addr as *const u8, self.len) }
    }
}

impl Drop for ShmSegment {
    fn drop(&mut self) {
        // SAFETY: detaches the attachment made in `attach`, exactly once.
        unsafe { libc::shmdt(self.addr as *const libc::c_void) };
    }
}

/// Captures the root window. It uses a shared memory segment while the server accepts one and falls back to
/// plain `GetImage` for good when it does not.
#[derive(Default)]
pub(crate) struct Grabber {
    shm: Option<ShmSegment>,
    shm_refused: bool,
}

impl Grabber {
    /// The root window of `width` by `height` pixels in `format`.
    pub(crate) fn grab(
        &mut self,
        conn: &RustConnection,
        root: Window,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<Frame> {
        if !self.shm_refused {
            match self.grab_shm(conn, root, width, height, format) {
                Ok(frame) => return Ok(frame),
                Err(_) => {
                    self.shm = None;
                    self.shm_refused = true;
                }
            }
        }
        grab_plain(conn, root, width, height, format)
    }

    fn grab_shm(
        &mut self,
        conn: &RustConnection,
        root: Window,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<Frame> {
        let len = bytes_per_line(width, format.bits_per_pixel) * height as usize;
        if self.shm.as_ref().is_none_or(|shm| shm.len != len) {
            self.shm = None;
            self.shm = Some(ShmSegment::attach(conn, len)?);
        }
        let segment = self.shm.as_ref().expect("a segment was just attached");
        // The reply means the server has written the pixels into the segment.
        conn.shm_get_image(
            root,
            0,
            0,
            width as u16,
            height as u16,
            !0,
            Z_PIXMAP,
            segment.seg,
            0,
        )
        .map_err(platform_error)?
        .reply()
        .map_err(platform_error)?;
        zpixmap_to_frame(segment.bytes(), width, height, format)
    }
}

fn grab_plain(
    conn: &RustConnection,
    root: Window,
    width: u32,
    height: u32,
    format: PixelFormat,
) -> Result<Frame> {
    let reply = conn
        .get_image(
            x11rb::protocol::xproto::ImageFormat::Z_PIXMAP,
            root,
            0,
            0,
            width as u16,
            height as u16,
            !0,
        )
        .map_err(platform_error)?
        .reply()
        .map_err(platform_error)?;
    zpixmap_to_frame(&reply.data, width, height, format)
}

fn platform(what: &str, error: std::io::Error) -> DesktopError {
    DesktopError::Platform(format!("{what} failed: {error}"))
}

pub(crate) fn platform_error(error: impl std::fmt::Display) -> DesktopError {
    DesktopError::Platform(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RGB: ChannelMasks = ChannelMasks {
        red: 0x00ff_0000,
        green: 0x0000_ff00,
        blue: 0x0000_00ff,
    };

    fn format(bits_per_pixel: u32, order: ByteOrder) -> PixelFormat {
        PixelFormat {
            bits_per_pixel,
            order,
            masks: RGB,
        }
    }

    #[test]
    fn scanlines_are_padded_to_four_bytes() {
        assert_eq!(bytes_per_line(2, 32), 8);
        assert_eq!(bytes_per_line(3, 24), 12, "9 bytes padded to 12");
        assert_eq!(bytes_per_line(4, 24), 12);
        assert_eq!(bytes_per_line(1, 24), 4);
    }

    #[test]
    fn a_little_endian_bgrx_pixel_becomes_rgba() {
        // One pixel, blue 10, green 20, red 30, padding byte 0, stored least significant byte first.
        let data = [10u8, 20, 30, 0];
        let frame = zpixmap_to_frame(&data, 1, 1, format(32, ByteOrder::Lsb)).expect("converts");
        assert_eq!(frame.rgba, vec![30, 20, 10, 255]);
    }

    #[test]
    fn a_big_endian_pixel_becomes_the_same_rgba() {
        // The same colour, stored most significant byte first: padding, red, green, blue.
        let data = [0u8, 30, 20, 10];
        let frame = zpixmap_to_frame(&data, 1, 1, format(32, ByteOrder::Msb)).expect("converts");
        assert_eq!(frame.rgba, vec![30, 20, 10, 255]);
    }

    #[test]
    fn a_24_bit_screen_skips_to_the_padded_next_row() {
        // Three pixels of three bytes per row: nine bytes, padded with three to 12 per row, for two rows.
        let row0 = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 0, 0, 0];
        let row1 = [10u8, 11, 12, 13, 14, 15, 16, 17, 18, 0, 0, 0];
        let data: Vec<u8> = row0.iter().chain(row1.iter()).copied().collect();
        let frame = zpixmap_to_frame(&data, 3, 2, format(24, ByteOrder::Lsb)).expect("converts");
        assert_eq!(
            frame.rgba,
            vec![
                3, 2, 1, 255, 6, 5, 4, 255, 9, 8, 7, 255, //
                12, 11, 10, 255, 15, 14, 13, 255, 18, 17, 16, 255,
            ]
        );
    }

    #[test]
    fn a_short_image_or_an_odd_screen_is_refused() {
        assert!(zpixmap_to_frame(&[0; 3], 1, 1, format(32, ByteOrder::Lsb)).is_err());
        assert!(zpixmap_to_frame(&[0; 64], 1, 1, format(16, ByteOrder::Lsb)).is_err());
        let wide = PixelFormat {
            bits_per_pixel: 32,
            order: ByteOrder::Lsb,
            masks: ChannelMasks {
                red: 0xf800,
                green: 0x07e0,
                blue: 0x001f,
            },
        };
        assert!(zpixmap_to_frame(&[0; 8], 1, 1, wide).is_err());
    }
}
