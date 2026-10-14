//! Window control on one X display through EWMH and ICCCM: the windows below the root, a window's pid, class and
//! title, activating a window, and asking one to close. With a window manager the requests go to it, as EWMH
//! client messages on the root. Without one they go to the window itself, because nothing else would act on them:
//! the window is raised and given the keyboard, or it is sent WM_DELETE_WINDOW when it asks for that.

use std::collections::BTreeSet;

use apz_desktop::Result;
use x11rb::CURRENT_TIME;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, EventMask,
    GetPropertyReply, InputFocus, StackMode, Window,
};
use x11rb::rust_connection::RustConnection;

use crate::capture::platform_error;

/// How many levels below the root `windows` looks: a window manager reparents a client under a frame.
const DEPTH: usize = 4;
/// The EWMH source indication these requests carry: 2, the value the requests ask for.
const SOURCE: u32 = 2;

/// The atoms the calls use, interned once per call.
pub(crate) struct Atoms {
    active: Atom,
    close: Atom,
    pid: Atom,
    net_name: Atom,
    utf8: Atom,
    wm_check: Atom,
    protocols: Atom,
    delete: Atom,
}

impl Atoms {
    pub(crate) fn intern(conn: &RustConnection) -> Result<Self> {
        Ok(Self {
            active: intern(conn, b"_NET_ACTIVE_WINDOW")?,
            close: intern(conn, b"_NET_CLOSE_WINDOW")?,
            pid: intern(conn, b"_NET_WM_PID")?,
            net_name: intern(conn, b"_NET_WM_NAME")?,
            utf8: intern(conn, b"UTF8_STRING")?,
            wm_check: intern(conn, b"_NET_SUPPORTING_WM_CHECK")?,
            protocols: intern(conn, b"WM_PROTOCOLS")?,
            delete: intern(conn, b"WM_DELETE_WINDOW")?,
        })
    }
}

fn intern(conn: &RustConnection, name: &[u8]) -> Result<Atom> {
    let reply = conn
        .intern_atom(false, name)
        .map_err(platform_error)?
        .reply()
        .map_err(platform_error)?;
    Ok(reply.atom)
}

/// The names an app is known by. `ids` are matched against a window's class: the desktop id and the program its
/// entry runs. `titles` are matched against the window's title: the entry's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Names {
    pub ids: Vec<String>,
    pub titles: Vec<String>,
}

/// Whether a window's class or title is one of the app's names, ignoring case.
pub(crate) fn matches_names(names: &Names, class: &[String], title: Option<&str>) -> bool {
    class
        .iter()
        .any(|part| names.ids.iter().any(|id| part.eq_ignore_ascii_case(id)))
        || title.is_some_and(|title| {
            names
                .titles
                .iter()
                .any(|name| title.eq_ignore_ascii_case(name))
        })
}

/// A window of an app: the window, and whether its process is one the backend started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Found {
    pub window: Window,
    pub launched: bool,
}

/// Reads a window's property, or None when the window or the property is not there.
fn property(
    conn: &RustConnection,
    window: Window,
    name: Atom,
    ty: impl Into<Atom>,
) -> Option<GetPropertyReply> {
    conn.get_property(false, window, name, ty, 0, 1024)
        .ok()?
        .reply()
        .ok()
}

fn children(conn: &RustConnection, window: Window) -> Vec<Window> {
    conn.query_tree(window)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .map(|reply| reply.children)
        .unwrap_or_default()
}

/// The windows below `root`, `DEPTH` levels down, top-level ones first.
pub(crate) fn windows(conn: &RustConnection, root: Window) -> Vec<Window> {
    let mut out = Vec::new();
    let mut level = vec![root];
    for _ in 0..DEPTH {
        let next: Vec<Window> = level
            .iter()
            .flat_map(|&window| children(conn, window))
            .collect();
        if next.is_empty() {
            break;
        }
        out.extend(next.iter().copied());
        level = next;
    }
    out
}

/// The pid a window names in `_NET_WM_PID`, if it names one.
pub(crate) fn pid(conn: &RustConnection, atoms: &Atoms, window: Window) -> Option<u32> {
    property(conn, window, atoms.pid, AtomEnum::CARDINAL)?
        .value32()?
        .next()
}

/// A window's `WM_CLASS`: the instance name, then the class name.
pub(crate) fn class(conn: &RustConnection, window: Window) -> Vec<String> {
    property(conn, window, AtomEnum::WM_CLASS.into(), AtomEnum::STRING)
        .map(|reply| {
            reply
                .value
                .split(|byte| *byte == 0)
                .filter(|part| !part.is_empty())
                .map(|part| String::from_utf8_lossy(part).into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// A window's title: `_NET_WM_NAME` when it has one, else `WM_NAME`.
pub(crate) fn title(conn: &RustConnection, atoms: &Atoms, window: Window) -> Option<String> {
    property(conn, window, atoms.net_name, atoms.utf8)
        .filter(|reply| !reply.value.is_empty())
        .or_else(|| property(conn, window, AtomEnum::WM_NAME.into(), AtomEnum::STRING))
        .filter(|reply| !reply.value.is_empty())
        .map(|reply| String::from_utf8_lossy(&reply.value).into_owned())
}

/// Whether a window manager runs on the display: the root names a supporting window in `_NET_SUPPORTING_WM_CHECK`.
pub(crate) fn has_window_manager(conn: &RustConnection, atoms: &Atoms, root: Window) -> bool {
    property(conn, root, atoms.wm_check, AtomEnum::ANY).is_some_and(|reply| !reply.value.is_empty())
}

/// The windows of an app on a display: those whose process the backend started (`launched`, a set of pids), and,
/// when `by_name` is set, those whose class or title is one of the app's names.
pub(crate) fn find(
    conn: &RustConnection,
    atoms: &Atoms,
    root: Window,
    names: &Names,
    launched: &BTreeSet<u32>,
    by_name: bool,
) -> Vec<Found> {
    windows(conn, root)
        .into_iter()
        .filter_map(|window| {
            let owned = pid(conn, atoms, window).is_some_and(|pid| launched.contains(&pid));
            let named = by_name
                && matches_names(
                    names,
                    &class(conn, window),
                    title(conn, atoms, window).as_deref(),
                );
            (owned || named).then_some(Found {
                window,
                launched: owned,
            })
        })
        .collect()
}

fn send_to_root(
    conn: &RustConnection,
    root: Window,
    message: Atom,
    window: Window,
    data: [u32; 5],
) -> Result<()> {
    let event = ClientMessageEvent::new(32, window, message, data);
    conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        event,
    )
    .map_err(platform_error)?
    .check()
    .map_err(platform_error)
}

/// Brings `window` to the front and gives it the keyboard. With a window manager the request is `_NET_ACTIVE_WINDOW`
/// to the root, and the window is raised here too. Without one the window is raised and given the keyboard
/// directly, which fails when the window is not mapped.
pub(crate) fn activate(
    conn: &RustConnection,
    atoms: &Atoms,
    root: Window,
    window: Window,
    managed: bool,
) -> Result<()> {
    if managed {
        send_to_root(
            conn,
            root,
            atoms.active,
            window,
            [SOURCE, CURRENT_TIME, 0, 0, 0],
        )?;
    }
    conn.configure_window(
        window,
        &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
    )
    .map_err(platform_error)?
    .check()
    .map_err(platform_error)?;
    if !managed {
        conn.set_input_focus(InputFocus::POINTER_ROOT, window, CURRENT_TIME)
            .map_err(platform_error)?
            .check()
            .map_err(platform_error)?;
    }
    Ok(())
}

fn supports_delete(conn: &RustConnection, atoms: &Atoms, window: Window) -> bool {
    property(conn, window, atoms.protocols, AtomEnum::ATOM)
        .and_then(|reply| {
            reply
                .value32()
                .map(|mut protocols| protocols.any(|atom| atom == atoms.delete))
        })
        .unwrap_or(false)
}

/// Asks `window` to close, gracefully: `_NET_CLOSE_WINDOW` to the root when a window manager runs, else
/// `WM_DELETE_WINDOW` to the window when it asks for that. Returns false when neither can be sent.
pub(crate) fn request_close(
    conn: &RustConnection,
    atoms: &Atoms,
    root: Window,
    window: Window,
    managed: bool,
) -> Result<bool> {
    if managed {
        send_to_root(
            conn,
            root,
            atoms.close,
            window,
            [CURRENT_TIME, SOURCE, 0, 0, 0],
        )?;
        return Ok(true);
    }
    if !supports_delete(conn, atoms, window) {
        return Ok(false);
    }
    let event = ClientMessageEvent::new(
        32,
        window,
        atoms.protocols,
        [atoms.delete, CURRENT_TIME, 0, 0, 0],
    );
    conn.send_event(false, window, EventMask::NO_EVENT, event)
        .map_err(platform_error)?
        .check()
        .map_err(platform_error)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Names {
        Names {
            ids: vec!["test-window".into(), "xmessage".into()],
            titles: vec!["Test Window".into()],
        }
    }

    #[test]
    fn a_window_matches_by_class_or_by_title_ignoring_case() {
        let class = vec!["Xmessage".to_string(), "xmessage".to_string()];
        assert!(matches_names(&names(), &class, None), "the class matches");
        assert!(
            matches_names(&names(), &[], Some("test window")),
            "the title matches"
        );
        assert!(
            !matches_names(&names(), &[], Some("Editor")),
            "neither matches"
        );
        assert!(
            !matches_names(&names(), &["Other".into()], None),
            "an unrelated class is not the app"
        );
    }
}
