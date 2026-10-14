//! The PipeWire side of the owner's screen. A thread connects to the portal's PipeWire remote, negotiates raw
//! video from the screen's stream and keeps the latest frame, which `latest` hands to the caller.
//!
//! PipeWire's objects are not `Send`, so they live on that thread only. The thread runs until the [`Capture`] is
//! dropped.

use std::io::Cursor;
use std::os::fd::OwnedFd;
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use apz_desktop::{DesktopError, Frame, Result};
use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use pw::stream::{StreamBox, StreamFlags, StreamState};
use spa::buffer::DataType;
use spa::param::ParamType;
use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use spa::param::format_utils;
use spa::param::video::{VideoFormat, VideoInfoRaw};
use spa::pod::serialize::PodSerializer;
use spa::pod::{self, Pod, Value};
use spa::utils::{Direction, Fraction, Rectangle, SpaTypes};

use crate::pixels::{PixelFormat, to_rgba};

/// How long the first frame may take to arrive. The compositor sends one as soon as the stream starts.
const FIRST_FRAME: Duration = Duration::from_secs(5);

/// How long starting the stream may take before the start is reported as failed.
const CONNECT: Duration = Duration::from_secs(10);

/// The latest frame, and the failure that ended the stream if one did.
#[derive(Default)]
struct Latest {
    frame: Option<Frame>,
    failure: Option<String>,
}

/// State shared between the PipeWire thread and the caller.
#[derive(Default)]
struct Shared {
    latest: Mutex<Latest>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Latest> {
        self.latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn publish(&self, frame: Frame) {
        self.lock().frame = Some(frame);
        self.changed.notify_all();
    }

    fn fail(&self, message: String) {
        self.lock().failure = Some(message);
        self.changed.notify_all();
    }
}

/// A running capture of one screen stream.
pub struct Capture {
    shared: Arc<Shared>,
    quit: pw::channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    /// Connects to the PipeWire remote `fd` and starts the stream `node`, returning once the stream is connected.
    pub fn start(fd: OwnedFd, node: u32) -> Result<Self> {
        let shared = Arc::new(Shared::default());
        let (quit, quit_rx) = pw::channel::channel::<()>();
        let (ready, connected) = mpsc::sync_channel::<std::result::Result<(), String>>(1);
        let thread = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("apz-wayland-capture".into())
                .spawn(move || run(fd, node, shared, quit_rx, ready))
                .map_err(|error| {
                    DesktopError::Platform(format!("cannot start the capture thread: {error}"))
                })?
        };
        match connected.recv_timeout(CONNECT) {
            Ok(Ok(())) => Ok(Self {
                shared,
                quit,
                thread: Some(thread),
            }),
            Ok(Err(message)) => {
                let _ = thread.join();
                Err(DesktopError::Platform(message))
            }
            Err(_) => {
                // The thread is stuck before its loop, so it is left to finish on its own rather than joined.
                Err(DesktopError::Platform(
                    "the PipeWire stream did not start in time".into(),
                ))
            }
        }
    }

    /// The latest frame, at the stream's pixel size. Waits up to [`FIRST_FRAME`] when no frame has arrived yet.
    pub fn latest(&self) -> Result<Frame> {
        let guard = self.shared.lock();
        let (guard, _timed_out) = self
            .shared
            .changed
            .wait_timeout_while(guard, FIRST_FRAME, |latest| {
                latest.frame.is_none() && latest.failure.is_none()
            })
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(message) = &guard.failure {
            return Err(DesktopError::Platform(format!(
                "the screen stream stopped: {message}"
            )));
        }
        guard
            .frame
            .clone()
            .ok_or_else(|| DesktopError::Platform("the screen stream has sent no frame yet".into()))
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.quit.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Reports a setup failure to the caller and ends the thread.
macro_rules! or_fail {
    ($ready:expr, $what:expr, $result:expr) => {
        match $result {
            Ok(value) => value,
            Err(error) => {
                let _ = $ready.send(Err(format!("{}: {error}", $what)));
                return;
            }
        }
    };
}

/// The thread body: sets up PipeWire on this thread, reports whether the stream connected, then runs the loop
/// until the quit channel fires.
fn run(
    fd: OwnedFd,
    node: u32,
    shared: Arc<Shared>,
    quit: pw::channel::Receiver<()>,
    ready: SyncSender<std::result::Result<(), String>>,
) {
    pw::init();
    let mainloop = or_fail!(
        ready,
        "PipeWire main loop",
        pw::main_loop::MainLoopRc::new(None)
    );
    let context = or_fail!(
        ready,
        "PipeWire context",
        pw::context::ContextRc::new(&mainloop, None)
    );
    let core = or_fail!(ready, "PipeWire remote", context.connect_fd_rc(fd, None));
    let props = properties! {
        *pw::keys::MEDIA_TYPE => "Video",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Screen",
    };
    let stream = or_fail!(
        ready,
        "PipeWire stream",
        StreamBox::new(&core, "apz-desktop-capture", props)
    );
    let state = Callbacks::new(Arc::clone(&shared));
    let _listener = or_fail!(
        ready,
        "PipeWire listener",
        stream
            .add_local_listener_with_user_data(state)
            .state_changed(|_, state, _old, new| {
                if let StreamState::Error(message) = new {
                    state.shared.fail(message);
                }
            })
            .param_changed(|_, state, id, param| {
                let Some(param) = param else {
                    state.negotiated = None;
                    return;
                };
                if id != ParamType::Format.as_raw() {
                    return;
                }
                let Ok((media_type, subtype)) = format_utils::parse_format(param) else {
                    return;
                };
                if media_type != MediaType::Video || subtype != MediaSubtype::Raw {
                    return;
                }
                if state.info.parse(param).is_err() {
                    state
                        .shared
                        .fail("the stream's video format cannot be read".into());
                    return;
                }
                let size = state.info.size();
                let Some(format) = pixel_format(state.info.format()) else {
                    state
                        .shared
                        .fail("the stream offered a pixel format the backend does not read".into());
                    return;
                };
                state.negotiated = Some(Negotiated {
                    format,
                    width: size.width,
                    height: size.height,
                });
            })
            .process(|stream, state| {
                let Some(negotiated) = state.negotiated else {
                    return;
                };
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let Some(plane) = buffer.datas_mut().first_mut() else {
                    return;
                };
                let row = negotiated.width as usize * 4;
                let stride = usize::try_from(plane.chunk().stride())
                    .ok()
                    .filter(|stride| *stride > 0)
                    .unwrap_or(row);
                let Some(bytes) = plane.data() else {
                    return;
                };
                if let Ok(rgba) = to_rgba(
                    negotiated.format,
                    negotiated.width,
                    negotiated.height,
                    stride,
                    bytes,
                ) && let Ok(frame) = Frame::new(negotiated.width, negotiated.height, rgba)
                {
                    state.shared.publish(frame);
                }
            })
            .register()
    );
    let format = or_fail!(ready, "PipeWire format", serialize(format_object()));
    let buffers = or_fail!(ready, "PipeWire buffers", serialize(buffers_object()));
    let (Some(format_pod), Some(buffers_pod)) =
        (Pod::from_bytes(&format), Pod::from_bytes(&buffers))
    else {
        let _ = ready.send(Err("PipeWire rejected the stream parameters".into()));
        return;
    };
    let mut params = [format_pod, buffers_pod];

    // Quitting is a message on the loop, so the thread wakes at once when the capture is dropped.
    let _attached = quit.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |_| mainloop.quit()
    });
    or_fail!(
        ready,
        "PipeWire connect",
        stream.connect(
            Direction::Input,
            Some(node),
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
            &mut params,
        )
    );
    let _ = ready.send(Ok(()));
    mainloop.run();
}

/// The state the stream callbacks update.
struct Callbacks {
    shared: Arc<Shared>,
    info: VideoInfoRaw,
    negotiated: Option<Negotiated>,
}

impl Callbacks {
    fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            info: VideoInfoRaw::new(),
            negotiated: None,
        }
    }
}

/// The format the stream settled on.
#[derive(Clone, Copy)]
struct Negotiated {
    format: PixelFormat,
    width: u32,
    height: u32,
}

/// The raw formats the backend reads, from what the stream can send.
fn pixel_format(format: VideoFormat) -> Option<PixelFormat> {
    [
        (VideoFormat::BGRx, PixelFormat::Bgrx),
        (VideoFormat::BGRA, PixelFormat::Bgra),
        (VideoFormat::RGBx, PixelFormat::Rgbx),
        (VideoFormat::RGBA, PixelFormat::Rgba),
    ]
    .into_iter()
    .find(|(raw, _)| raw.as_raw() == format.as_raw())
    .map(|(_, pixel)| pixel)
}

/// The format the backend asks for: one of the four raw RGB layouts, at any size up to 16384 pixels a side.
fn format_object() -> pod::Object {
    pod::object!(
        SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,
        pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA
        ),
        pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle {
                width: 1920,
                height: 1080
            },
            Rectangle {
                width: 1,
                height: 1
            },
            Rectangle {
                width: 16384,
                height: 16384
            }
        ),
        pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: 30, denom: 1 },
            Fraction { num: 0, denom: 1 },
            Fraction {
                num: 1000,
                denom: 1
            }
        ),
    )
}

/// The buffers the backend accepts: shared memory only (MemFd or MemPtr), so every frame can be read without
/// mapping a DMA-BUF.
fn buffers_object() -> pod::Object {
    let data_types = (1u32 << DataType::MemPtr.as_raw()) | (1u32 << DataType::MemFd.as_raw());
    pod::Object {
        type_: SpaTypes::ObjectParamBuffers.as_raw(),
        id: ParamType::Buffers.as_raw(),
        properties: vec![pod::Property::new(
            spa::sys::SPA_PARAM_BUFFERS_dataType,
            Value::Int(data_types as i32),
        )],
    }
}

/// Encodes a pod object for `connect`.
fn serialize(object: pod::Object) -> std::result::Result<Vec<u8>, String> {
    PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(object))
        .map(|(cursor, _)| cursor.into_inner())
        .map_err(|error| format!("the parameters cannot be encoded: {error}"))
}
