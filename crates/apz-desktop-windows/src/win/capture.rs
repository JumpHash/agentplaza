//! Screen capture of the primary display. DXGI Desktop Duplication first; GDI BitBlt when duplication is not
//! available (no output at the desktop origin, a secure desktop, a device that was lost). Windows.Graphics.Capture
//! is the next step and is not used yet.

use apz_desktop::{DesktopError, Frame, Result};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, IDXGIAdapter1,
    IDXGIFactory1, IDXGIOutput, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, HBITMAP, HDC, ReleaseDC, SRCCOPY,
    SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
use windows::core::Interface;

use super::platform_error;
use crate::pixels::bgra_row_to_rgba;

/// How long one duplication wait lasts. A static screen answers with a timeout, which means "nothing changed".
const FRAME_WAIT_MS: u32 = 100;

/// Captures one session's screen. It keeps the last frame, so a static screen returns it without a copy.
#[derive(Default)]
pub struct Capturer {
    duplicator: Option<Duplicator>,
    last: Option<Frame>,
}

impl Capturer {
    /// The screen now. Duplication is opened on first use and reopened after a failure.
    pub fn grab(&mut self) -> Result<Frame> {
        if self.duplicator.is_none() {
            self.duplicator = Duplicator::open().ok();
        }
        if let Some(duplicator) = self.duplicator.as_mut() {
            match duplicator.next_frame() {
                Ok(Some(frame)) => return Ok(self.keep(frame)),
                Ok(None) => {
                    if let Some(last) = self.last.clone() {
                        return Ok(last);
                    }
                }
                Err(_) => self.duplicator = None,
            }
        }
        let frame = gdi_grab()?;
        Ok(self.keep(frame))
    }

    fn keep(&mut self, frame: Frame) -> Frame {
        self.last = Some(frame.clone());
        frame
    }
}

/// One output's duplication, with the device it copies through.
struct Duplicator {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    staging: Option<ID3D11Texture2D>,
}

impl Duplicator {
    fn open() -> Result<Self> {
        // SAFETY: CreateDXGIFactory1 takes no pointers.
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.map_err(platform_error)?;
        let (adapter, output) = primary_output(&factory)?;
        let mut device = None;
        let mut context = None;
        // SAFETY: the adapter outlives the call; the out pointers point at locals.
        unsafe {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .map_err(platform_error)?;
        let (Some(device), Some(context)) = (device, context) else {
            return Err(DesktopError::Platform(
                "Direct3D gave no device for the display".into(),
            ));
        };
        let output: IDXGIOutput1 = output.cast().map_err(platform_error)?;
        // SAFETY: the device belongs to the adapter that owns `output`, as duplication requires.
        let duplication = unsafe { output.DuplicateOutput(&device) }.map_err(platform_error)?;
        Ok(Self {
            device,
            context,
            duplication,
            staging: None,
        })
    }

    /// The next changed frame: `None` when nothing changed within the wait.
    fn next_frame(&mut self) -> Result<Option<Frame>> {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        // SAFETY: the out pointers point at locals that outlive the call.
        match unsafe {
            self.duplication
                .AcquireNextFrame(FRAME_WAIT_MS, &mut info, &mut resource)
        } {
            Ok(()) => {}
            Err(error) if error.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
            Err(error) => return Err(platform_error(error)),
        }
        // A zero present time means only the pointer moved: the picture did not change.
        let copied = match resource {
            Some(resource) if info.LastPresentTime != 0 => self.copy(&resource).map(Some),
            _ => Ok(None),
        };
        // SAFETY: a frame was acquired above, and it is released exactly once, even when the copy failed.
        let released = unsafe { self.duplication.ReleaseFrame() }.map_err(platform_error);
        let frame = copied?;
        released?;
        Ok(frame)
    }

    fn copy(&mut self, resource: &IDXGIResource) -> Result<Frame> {
        let texture: ID3D11Texture2D = resource.cast().map_err(platform_error)?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `desc` is a valid out pointer.
        unsafe { texture.GetDesc(&mut desc) };
        if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return Err(DesktopError::Platform(format!(
                "the desktop is in format {:?}, not BGRA",
                desc.Format
            )));
        }
        let staging = self.staging_for(&desc)?;
        // SAFETY: both textures belong to this device and have the same size and format.
        unsafe { self.context.CopyResource(&staging, &texture) };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: the staging texture is CPU-readable; it is unmapped below.
        unsafe {
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .map_err(platform_error)?;
        let width = desc.Width as usize;
        let height = desc.Height as usize;
        let pitch = mapped.RowPitch as usize;
        let mut rgba = Vec::with_capacity(width * height * 4);
        for row in 0..height {
            // SAFETY: the mapping holds `pitch` bytes per row for `height` rows, and each row starts with
            // `width` BGRA pixels, so the `width * 4` bytes read here lie inside it.
            let line = unsafe {
                std::slice::from_raw_parts((mapped.pData as *const u8).add(row * pitch), width * 4)
            };
            bgra_row_to_rgba(line, &mut rgba);
        }
        // SAFETY: matches the Map above.
        unsafe { self.context.Unmap(&staging, 0) };
        Frame::new(desc.Width, desc.Height, rgba)
    }

    /// A CPU-readable copy target of the desktop's size, reused while the size holds.
    fn staging_for(&mut self, desc: &D3D11_TEXTURE2D_DESC) -> Result<ID3D11Texture2D> {
        if let Some(staging) = &self.staging {
            let mut current = D3D11_TEXTURE2D_DESC::default();
            // SAFETY: `current` is a valid out pointer.
            unsafe { staging.GetDesc(&mut current) };
            if current.Width == desc.Width && current.Height == desc.Height {
                return Ok(staging.clone());
            }
        }
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: desc.Width,
            Height: desc.Height,
            MipLevels: 1,
            ArraySize: 1,
            Format: desc.Format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut texture = None;
        // SAFETY: no initial data; the out pointer points at a local.
        unsafe {
            self.device
                .CreateTexture2D(&staging_desc, None, Some(&mut texture))
        }
        .map_err(platform_error)?;
        let texture = texture
            .ok_or_else(|| DesktopError::Platform("Direct3D gave no staging texture".into()))?;
        self.staging = Some(texture.clone());
        Ok(texture)
    }
}

/// The adapter and output at the desktop origin: the primary display.
fn primary_output(factory: &IDXGIFactory1) -> Result<(IDXGIAdapter1, IDXGIOutput)> {
    for adapter_index in 0.. {
        // SAFETY: EnumAdapters1 takes an index and returns an owned adapter.
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(adapter_index) }) else {
            break;
        };
        for output_index in 0.. {
            // SAFETY: as above, for outputs of this adapter.
            let Ok(output) = (unsafe { adapter.EnumOutputs(output_index) }) else {
                break;
            };
            // SAFETY: `GetDesc` fills in an owned struct.
            let desc = unsafe { output.GetDesc() }.map_err(platform_error)?;
            let at_origin = desc.DesktopCoordinates.left == 0 && desc.DesktopCoordinates.top == 0;
            if desc.AttachedToDesktop.as_bool() && at_origin {
                return Ok((adapter, output));
            }
        }
    }
    Err(DesktopError::NotFound(
        "no display at the desktop origin".into(),
    ))
}

/// The primary display through GDI BitBlt. It fails on a secure desktop, and for windows that draw outside GDI.
fn gdi_grab() -> Result<Frame> {
    // SAFETY: GetSystemMetrics takes an index and returns a value.
    let (width, height) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    if width <= 0 || height <= 0 {
        return Err(DesktopError::Platform(
            "the primary display has no size".into(),
        ));
    }
    // SAFETY: a null window asks for the screen's device context, released below.
    let screen = unsafe { GetDC(None) };
    if screen.0.is_null() {
        return Err(DesktopError::Platform(
            "GetDC gave no screen context".into(),
        ));
    }
    // SAFETY: the handles are created here and released below in every path.
    let (memory, bitmap, previous) = unsafe {
        let memory = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let previous = SelectObject(memory, bitmap);
        (memory, bitmap, previous)
    };
    // SAFETY: both device contexts are live.
    let pixels = unsafe { BitBlt(memory, 0, 0, width, height, screen, 0, 0, SRCCOPY) }
        .map_err(platform_error)
        .and_then(|()| read_bitmap(memory, bitmap, width, height));
    // SAFETY: undoes the selection and releases what was created above.
    unsafe {
        SelectObject(memory, previous);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
    }
    Frame::new(width as u32, height as u32, pixels?)
}

/// The bitmap's pixels as RGBA rows, top row first.
fn read_bitmap(memory: HDC, bitmap: HBITMAP, width: i32, height: i32) -> Result<Vec<u8>> {
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // Negative height: rows run top to bottom.
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let row_bytes = width as usize * 4;
    let mut bgra = vec![0u8; row_bytes * height as usize];
    // SAFETY: `bgra` holds exactly `height` rows of `row_bytes`, which is what GetDIBits writes for 32-bit rows.
    let lines = unsafe {
        GetDIBits(
            memory,
            bitmap,
            0,
            height as u32,
            Some(bgra.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        )
    };
    if lines != height {
        return Err(DesktopError::Platform(format!(
            "GetDIBits copied {lines} of {height} rows"
        )));
    }
    let mut rgba = Vec::with_capacity(bgra.len());
    for line in bgra.chunks_exact(row_bytes) {
        bgra_row_to_rgba(line, &mut rgba);
    }
    Ok(rgba)
}
