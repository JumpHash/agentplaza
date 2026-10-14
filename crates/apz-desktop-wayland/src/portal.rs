//! The desktop portals: one RemoteDesktop session that also carries the ScreenCast stream of the owner's screen,
//! so the owner approves one dialog for the screen, the pointer and the keyboard together.
//!
//! The daemon's request loop is synchronous, so every portal call here blocks on a private single-threaded tokio
//! runtime. The restore token the portal returns is passed back on the next session, so the owner approves once
//! and not again until they revoke the access.

use std::future::Future;
use std::os::fd::OwnedFd;

use apz_desktop::{DesktopError, Frame, Result};
use ashpd::desktop::remote_desktop::{
    Axis, DeviceType, KeyState, RemoteDesktop, SelectDevicesOptions,
};
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::{PersistMode, Session};
use ashpd::enumflags2::BitFlags;

use crate::capture::Capture;

/// A started portal session: the input and screen it gives, and the token that starts the next one unasked.
pub struct Opened {
    pub portal: Portal,
    pub restore_token: Option<String>,
}

/// A live portal session with its PipeWire capture of the screen.
pub struct Portal {
    runtime: tokio::runtime::Runtime,
    remote: RemoteDesktop,
    session: Session<RemoteDesktop>,
    node: u32,
    capture: Capture,
    logical: Option<(u32, u32)>,
}

/// The parts the async setup hands back to the blocking side.
struct Started {
    remote: RemoteDesktop,
    session: Session<RemoteDesktop>,
    fd: OwnedFd,
    node: u32,
    logical: Option<(u32, u32)>,
    restore_token: Option<String>,
}

impl Portal {
    /// Starts the session. The portal shows the owner its dialog unless `restore` (the token of an earlier
    /// approval) lets it skip the dialog.
    pub fn open(restore: Option<&str>) -> Result<Opened> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| {
                DesktopError::Platform(format!("cannot start the runtime: {error}"))
            })?;
        let started = runtime.block_on(start(restore)).map_err(portal_error)?;
        let capture = match Capture::start(started.fd, started.node) {
            Ok(capture) => capture,
            Err(error) => {
                // The screen never started, so the portal session is closed rather than left open for the owner.
                let _ = runtime.block_on(started.session.close());
                return Err(error);
            }
        };
        Ok(Opened {
            portal: Self {
                runtime,
                remote: started.remote,
                session: started.session,
                node: started.node,
                capture,
                logical: started.logical,
            },
            restore_token: started.restore_token,
        })
    }

    /// The size of the screen in logical (point) pixels, when the portal reports it. The frames are in device
    /// pixels, which differ on a scaled display.
    pub fn logical_size(&self) -> Option<(u32, u32)> {
        self.logical
    }

    /// The latest frame, in device pixels.
    pub fn latest_frame(&self) -> Result<Frame> {
        self.capture.latest()
    }

    /// Moves the pointer to a point of the screen, in logical pixels from the top-left corner.
    pub fn motion(&self, x: i32, y: i32) -> Result<()> {
        self.call(self.remote.notify_pointer_motion_absolute(
            &self.session,
            self.node,
            f64::from(x),
            f64::from(y),
            Default::default(),
        ))
    }

    /// Presses or releases a pointer button, by its evdev code.
    pub fn button(&self, code: i32, pressed: bool) -> Result<()> {
        self.call(self.remote.notify_pointer_button(
            &self.session,
            code,
            key_state(pressed),
            Default::default(),
        ))
    }

    /// Scrolls by `steps` notches along an axis. Positive is down or right.
    pub fn scroll(&self, axis: Axis, steps: i32) -> Result<()> {
        self.call(self.remote.notify_pointer_axis_discrete(
            &self.session,
            axis,
            steps,
            Default::default(),
        ))
    }

    /// Presses or releases a key, by its X keysym.
    pub fn key(&self, keysym: u32, pressed: bool) -> Result<()> {
        let keysym = i32::try_from(keysym)
            .map_err(|_| DesktopError::Invalid(format!("keysym {keysym:#x} is out of range")))?;
        self.call(self.remote.notify_keyboard_keysym(
            &self.session,
            keysym,
            key_state(pressed),
            Default::default(),
        ))
    }

    /// Closes the portal session and stops the capture. The owner's screen is no longer shared after this.
    pub fn close(self) {
        // `_capture` lives to the end of this function: it stops after the session has closed and while the runtime
        // is still alive, the same order as an explicit drop.
        let Self {
            runtime,
            session,
            capture: _capture,
            ..
        } = self;
        let _ = runtime.block_on(session.close());
    }

    fn call<T>(
        &self,
        future: impl Future<Output = std::result::Result<T, ashpd::Error>>,
    ) -> Result<T> {
        self.runtime.block_on(future).map_err(portal_error)
    }
}

/// Sets the session up: the screen source, the devices, the start (which shows the dialog), and the PipeWire
/// remote with the stream's node.
async fn start(restore: Option<&str>) -> std::result::Result<Started, ashpd::Error> {
    let remote = RemoteDesktop::new().await?;
    let screencast = Screencast::new().await?;
    let session = remote.create_session(Default::default()).await?;

    screencast
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Embedded)
                .set_sources(BitFlags::from(SourceType::Monitor))
                .set_multiple(false)
                .set_persist_mode(PersistMode::ExplicitlyRevoked)
                .set_restore_token(restore),
        )
        .await?
        .response()?;

    remote
        .select_devices(
            &session,
            SelectDevicesOptions::default()
                .set_devices(DeviceType::Keyboard | DeviceType::Pointer)
                .set_persist_mode(PersistMode::ExplicitlyRevoked)
                .set_restore_token(restore),
        )
        .await?
        .response()?;

    let selected = remote
        .start(&session, None, Default::default())
        .await?
        .response()?;
    let stream = selected.streams().first().ok_or(ashpd::Error::ParseError(
        "the portal returned no screen stream",
    ))?;
    let node = stream.pipe_wire_node_id();
    let logical = stream
        .size()
        .and_then(|(w, h)| Some((u32::try_from(w).ok()?, u32::try_from(h).ok()?)))
        .filter(|(w, h)| *w > 0 && *h > 0);
    let restore_token = selected.restore_token().map(str::to_owned);

    let fd = screencast
        .open_pipe_wire_remote(&session, Default::default())
        .await?;

    Ok(Started {
        remote,
        session,
        fd,
        node,
        logical,
        restore_token,
    })
}

fn key_state(pressed: bool) -> KeyState {
    if pressed {
        KeyState::Pressed
    } else {
        KeyState::Released
    }
}

/// A portal error as the desktop trait reports it. A dialog the owner dismissed is "not permitted", since
/// the owner chose that.
fn portal_error(error: ashpd::Error) -> DesktopError {
    match error {
        ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled) => {
            DesktopError::NotPermitted(
                "the owner did not approve sharing the screen and input".into(),
            )
        }
        other => DesktopError::Platform(format!("the desktop portal: {other}")),
    }
}
