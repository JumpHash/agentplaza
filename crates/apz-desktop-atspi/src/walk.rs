//! Reading the live accessibility tree: applications, the focused one, and what each control is.

use apz_desktop::{DesktopError, Result};
use atspi::connection::AccessibilityConnection;
use atspi::proxy::accessible::ObjectRefExt;
use atspi::proxy::action::ActionProxy;
use atspi::{Interface, InterfaceSet, ObjectRefOwned, State, StateSet};

use crate::actions::{Facts, advertised};
use crate::role::apz_role;

/// How far a name or role search goes below an application. A deeper control needs a path.
pub(crate) const MAX_SEARCH_DEPTH: u8 = 32;

/// The most characters of a text control's contents a tree carries; a long document is cut here.
pub(crate) const MAX_TEXT_CHARS: i32 = 4096;

pub(crate) fn platform(error: impl std::fmt::Display) -> DesktopError {
    DesktopError::Platform(error.to_string())
}

/// The proxy of interface `$ty` on the object `$obj`, over the connection `$conn`.
macro_rules! proxy_on {
    ($ty:ty, $conn:expr, $obj:expr) => {{
        let destination: zbus::names::BusName<'static> = match $obj.name_as_str() {
            Some(name) => {
                zbus::names::BusName::try_from(name.to_owned()).map_err(crate::walk::platform)?
            }
            None => {
                return Err(crate::walk::platform(
                    "an accessible object has no bus name",
                ));
            }
        };
        <$ty>::builder($conn)
            .destination(destination)
            .map_err(crate::walk::platform)?
            .path($obj.path().clone())
            .map_err(crate::walk::platform)?
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await
            .map_err(crate::walk::platform)
    }};
}
pub(crate) use proxy_on;

/// What a control is, and what it can do: its normalised role and name, its states and interfaces, and the
/// AT-SPI names of the actions it lists.
pub(crate) struct Probe {
    pub(crate) role: String,
    pub(crate) name: Option<String>,
    pub(crate) states: StateSet,
    pub(crate) interfaces: InterfaceSet,
    pub(crate) actions: Vec<String>,
}

impl Probe {
    /// The apz action names the control advertises.
    pub(crate) fn advertised(&self) -> Vec<&'static str> {
        advertised(&Facts {
            role: &self.role,
            actions: &self.actions,
            editable: self.interfaces.contains(Interface::EditableText),
            value: self.interfaces.contains(Interface::Value),
            component: self.interfaces.contains(Interface::Component),
            selectable: self.states.contains(State::Selectable),
        })
    }
}

/// The normalised role and the name of a control: the cheap part of a probe, used while searching.
pub(crate) async fn identity(
    bus: &AccessibilityConnection,
    obj: &ObjectRefOwned,
) -> Result<(String, Option<String>)> {
    let accessible = obj
        .as_accessible_proxy(bus.connection())
        .await
        .map_err(platform)?;
    let role = accessible.get_role().await.map_err(platform)?;
    let name = accessible.name().await.map_err(platform)?;
    Ok((apz_role(role.name()), (!name.is_empty()).then_some(name)))
}

/// Everything about a control that the tree and the actions need.
pub(crate) async fn probe(bus: &AccessibilityConnection, obj: &ObjectRefOwned) -> Result<Probe> {
    let conn = bus.connection();
    let (role, name) = identity(bus, obj).await?;
    let accessible = obj.as_accessible_proxy(conn).await.map_err(platform)?;
    let states = accessible.get_state().await.map_err(platform)?;
    let interfaces = accessible.get_interfaces().await.map_err(platform)?;
    let actions = if interfaces.contains(Interface::Action) {
        let action = proxy_on!(ActionProxy, conn, obj)?;
        action
            .get_actions()
            .await
            .map_err(platform)?
            .into_iter()
            .map(|listed| listed.name)
            .collect()
    } else {
        Vec::new()
    };
    Ok(Probe {
        role,
        name,
        states,
        interfaces,
        actions,
    })
}

/// The states of a control.
pub(crate) async fn states_of(
    bus: &AccessibilityConnection,
    obj: &ObjectRefOwned,
) -> Result<StateSet> {
    let accessible = obj
        .as_accessible_proxy(bus.connection())
        .await
        .map_err(platform)?;
    accessible.get_state().await.map_err(platform)
}

/// The children of a control, in order. Objects a toolkit has since destroyed are returned too; a caller that
/// finds one unusable skips it.
pub(crate) async fn children(
    bus: &AccessibilityConnection,
    obj: &ObjectRefOwned,
) -> Result<Vec<ObjectRefOwned>> {
    let accessible = obj
        .as_accessible_proxy(bus.connection())
        .await
        .map_err(platform)?;
    accessible.get_children().await.map_err(platform)
}

/// The applications on the bus, from the registry root.
pub(crate) async fn applications(bus: &AccessibilityConnection) -> Result<Vec<ObjectRefOwned>> {
    let root = bus.root_accessible_on_registry().await.map_err(platform)?;
    root.get_children().await.map_err(platform)
}

/// The name an application or window goes by.
pub(crate) async fn name_of(bus: &AccessibilityConnection, obj: &ObjectRefOwned) -> Result<String> {
    let accessible = obj
        .as_accessible_proxy(bus.connection())
        .await
        .map_err(platform)?;
    accessible.name().await.map_err(platform)
}

/// The executable name of the process that owns an object's bus name, from `/proc`. None when it cannot be read.
async fn process_name(bus: &AccessibilityConnection, obj: &ObjectRefOwned) -> Option<String> {
    let name = obj.name_as_str()?.to_owned();
    let dbus = zbus::fdo::DBusProxy::new(bus.connection()).await.ok()?;
    let destination = zbus::names::BusName::try_from(name).ok()?;
    let pid = dbus
        .get_connection_unix_process_id(destination)
        .await
        .ok()?;
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(comm.trim().to_owned())
}

/// The application called `wanted`, by its name or by its process name.
pub(crate) async fn find_app(
    bus: &AccessibilityConnection,
    wanted: &str,
) -> Result<ObjectRefOwned> {
    for app in applications(bus).await? {
        if app.is_null() {
            continue;
        }
        let named = name_of(bus, &app).await.is_ok_and(|name| name == wanted);
        if named || process_name(bus, &app).await.as_deref() == Some(wanted) {
            return Ok(app);
        }
    }
    Err(DesktopError::NotFound(format!(
        "no application named {wanted}"
    )))
}

/// The application with a focused window: the first whose window holds the Active state.
pub(crate) async fn focused_app(bus: &AccessibilityConnection) -> Result<ObjectRefOwned> {
    for app in applications(bus).await? {
        if app.is_null() {
            continue;
        }
        for window in children(bus, &app).await.unwrap_or_default() {
            if window.is_null() {
                continue;
            }
            if states_of(bus, &window)
                .await
                .is_ok_and(|states| states.contains(State::Active))
            {
                return Ok(app);
            }
        }
    }
    Err(DesktopError::NotFound(
        "no application has a focused window; name the application".into(),
    ))
}

/// The application a request names, or the focused one when it names none.
pub(crate) async fn resolve_app(
    bus: &AccessibilityConnection,
    app: Option<&str>,
) -> Result<ObjectRefOwned> {
    match app {
        Some(name) => find_app(bus, name).await,
        None => focused_app(bus).await,
    }
}
