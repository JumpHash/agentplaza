//! The AT-SPI connection of a session, and its UI tree.

use std::future::Future;
use std::pin::Pin;

use apz_desktop::{DesktopError, Result, UiNode};
use atspi::connection::AccessibilityConnection;
use atspi::proxy::bus::BusProxy;
use atspi::proxy::component::ComponentProxy;
use atspi::proxy::text::TextProxy;
use atspi::proxy::value::ValueProxy;
use atspi::{CoordType, Interface, ObjectRefOwned};

use crate::walk::{
    MAX_TEXT_CHARS, applications, children, name_of, platform, probe, proxy_on, resolve_app,
};

/// A session's accessibility tree, read and driven over its AT-SPI bus.
///
/// The bus is the one the session bus advertises (`org.a11y.Bus.GetAddress`); the session is named by its bus
/// address, so a daemon can point this at a private session. Each call blocks until the bus answers. A bus call
/// that a frozen application never answers fails after zbus's method timeout.
pub struct AtspiTree {
    bus: AccessibilityConnection,
}

impl AtspiTree {
    /// Connects to the accessibility bus of the session at `session_bus`, a D-Bus address such as
    /// `unix:path=/run/user/1000/bus`.
    ///
    /// Fails with [`DesktopError::Unsupported`] when the session does not advertise an accessibility bus.
    pub fn connect(session_bus: &str) -> Result<Self> {
        async_io::block_on(async {
            let session = zbus::connection::Builder::address(session_bus)
                .map_err(platform)?
                .build()
                .await
                .map_err(platform)?;
            let address = BusProxy::new(&session)
                .await
                .map_err(platform)?
                .get_address()
                .await
                .map_err(platform)?;
            if address.is_empty() {
                return Err(DesktopError::Unsupported(
                    "the session has no accessibility bus (accessibility is off)".into(),
                ));
            }
            let address: zbus::Address = address.parse().map_err(platform)?;
            let bus = AccessibilityConnection::from_address(address)
                .await
                .map_err(platform)?;
            Ok(Self { bus })
        })
    }

    /// The names of the applications on the accessibility bus.
    pub fn apps(&self) -> Result<Vec<String>> {
        async_io::block_on(async {
            let mut names = Vec::new();
            for app in applications(&self.bus).await? {
                if app.is_null() {
                    continue;
                }
                if let Ok(name) = name_of(&self.bus, &app).await {
                    names.push(name);
                }
            }
            Ok(names)
        })
    }

    /// The UI tree of the application `app` names (by its name or its process name), or of the focused
    /// application when `app` is `None`, down to `depth` levels below the application.
    ///
    /// Node ids are numbered in document order within this tree. A child the toolkit has destroyed is left out.
    pub fn tree(&self, app: Option<&str>, depth: u8) -> Result<UiNode> {
        async_io::block_on(async {
            let app = resolve_app(&self.bus, app).await?;
            let mut next_id = 0;
            build(&self.bus, app, depth, &mut next_id).await
        })
    }

    /// Performs an accessibility action on the control `selector` picks, within the application the selector
    /// names (or the focused one). Each action maps onto an AT-SPI interface; the module `act` documents the map.
    pub fn act(
        &self,
        selector: &apz_station::Selector,
        action: &apz_station::UiAction,
    ) -> Result<()> {
        async_io::block_on(crate::act::act(&self.bus, selector, action))
    }
}

/// The node for `obj` and, down to `depth` levels, its children.
fn build<'a>(
    bus: &'a AccessibilityConnection,
    obj: ObjectRefOwned,
    depth: u8,
    next_id: &'a mut u64,
) -> Pin<Box<dyn Future<Output = Result<UiNode>> + 'a>> {
    Box::pin(async move {
        let id = *next_id;
        *next_id += 1;
        let described = probe(bus, &obj).await?;
        let conn = bus.connection();

        let value = if described.interfaces.contains(Interface::Text) {
            let text = proxy_on!(TextProxy, conn, &obj)?;
            let count = text
                .character_count()
                .await
                .map_err(platform)?
                .min(MAX_TEXT_CHARS);
            if count > 0 {
                let contents = text.get_text(0, count).await.map_err(platform)?;
                (!contents.is_empty()).then_some(contents)
            } else {
                None
            }
        } else if described.interfaces.contains(Interface::Value) {
            let number = proxy_on!(ValueProxy, conn, &obj)?;
            Some(number.current_value().await.map_err(platform)?.to_string())
        } else {
            None
        };

        let bounds = if described.interfaces.contains(Interface::Component) {
            let component = proxy_on!(ComponentProxy, conn, &obj)?;
            let (x, y, width, height) = component
                .get_extents(CoordType::Screen)
                .await
                .map_err(platform)?;
            (
                x,
                y,
                u32::try_from(width).unwrap_or(0),
                u32::try_from(height).unwrap_or(0),
            )
        } else {
            (0, 0, 0, 0)
        };

        let actions = described
            .advertised()
            .into_iter()
            .map(str::to_owned)
            .collect();
        let mut children_nodes = Vec::new();
        if depth > 0 {
            for child in children(bus, &obj).await? {
                if child.is_null() {
                    continue;
                }
                // A child destroyed since the list was read cannot be probed; the rest of the tree still stands.
                if let Ok(node) = build(bus, child, depth - 1, next_id).await {
                    children_nodes.push(node);
                }
            }
        }

        Ok(UiNode {
            id,
            role: described.role,
            name: described.name,
            value,
            bounds,
            actions,
            children: children_nodes,
        })
    })
}
