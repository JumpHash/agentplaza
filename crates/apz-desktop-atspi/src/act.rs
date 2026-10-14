//! Accessibility actions on a control: finding the control a selector picks, then the AT-SPI call that performs
//! the action. None of them moves the owner's pointer or takes focus, except `focus`, which asks for it.
//!
//! - `invoke` runs the control's click, press or activate action.
//! - `toggle` runs its toggle action, or its click for a check box.
//! - `select` runs its select action, or selects it in its parent through Selection.
//! - `expand` and `collapse` run its expand-or-contract action, and do nothing when it is already in the state.
//! - `set_value` sets the text of an editable control (EditableText), or the number of a Value control.
//! - `focus` grabs keyboard focus (Component).

use std::future::Future;
use std::pin::Pin;

use apz_desktop::{DesktopError, Result, action_name};
use apz_station::{Selector, UiAction};
use atspi::connection::AccessibilityConnection;
use atspi::proxy::accessible::ObjectRefExt;
use atspi::proxy::action::ActionProxy;
use atspi::proxy::component::ComponentProxy;
use atspi::proxy::editable_text::EditableTextProxy;
use atspi::proxy::selection::SelectionProxy;
use atspi::proxy::value::ValueProxy;
use atspi::{Interface, ObjectRefOwned, State};

use crate::actions::action_index;
use crate::select::selects;
use crate::walk::{
    MAX_SEARCH_DEPTH, Probe, children, identity, platform, probe, proxy_on, resolve_app,
};

/// Performs `action` on the control `selector` picks, within the application the selector names, or the focused
/// application when it names none. The control must advertise the action.
pub(crate) async fn act(
    bus: &AccessibilityConnection,
    selector: &Selector,
    action: &UiAction,
) -> Result<()> {
    if selector.path.is_none() && selector.role.is_none() && selector.name.is_none() {
        return Err(DesktopError::Invalid(
            "a selector needs a path, a role or a name".into(),
        ));
    }
    let app = resolve_app(bus, selector.app.as_deref()).await?;
    let target = locate(bus, &app, selector).await?;
    let described = probe(bus, &target).await?;
    let wanted = action_name(action);
    if !described.advertised().contains(&wanted) {
        return Err(DesktopError::Invalid(format!(
            "a {} takes no {wanted} action",
            described.role
        )));
    }
    perform(bus, &target, &described, action).await
}

/// The control a selector picks. A path is taken from the application's object, one child index at a time (its
/// windows first). Without a path the first control in document order whose role and name match is taken.
async fn locate(
    bus: &AccessibilityConnection,
    app: &ObjectRefOwned,
    selector: &Selector,
) -> Result<ObjectRefOwned> {
    let Some(path) = &selector.path else {
        return search(bus, app.clone(), selector, MAX_SEARCH_DEPTH)
            .await
            .ok_or_else(|| DesktopError::NotFound(format!("no control matches {selector:?}")));
    };
    let mut node = app.clone();
    for &index in path {
        let child = i32::try_from(index)
            .map_err(|_| DesktopError::Invalid(format!("path index {index} is too large")))?;
        node = {
            let accessible = node
                .as_accessible_proxy(bus.connection())
                .await
                .map_err(platform)?;
            accessible
                .get_child_at_index(child)
                .await
                .map_err(|_| DesktopError::NotFound(format!("no control at path {path:?}")))?
        };
    }
    if selector.role.is_some() || selector.name.is_some() {
        let (role, name) = identity(bus, &node).await?;
        if !selects(selector, &role, name.as_deref()) {
            return Err(DesktopError::NotFound(format!(
                "the control at path {path:?} is not {selector:?}"
            )));
        }
    }
    Ok(node)
}

/// The first control at or below `obj`, in document order, that the selector picks.
fn search<'a>(
    bus: &'a AccessibilityConnection,
    obj: ObjectRefOwned,
    selector: &'a Selector,
    depth: u8,
) -> Pin<Box<dyn Future<Output = Option<ObjectRefOwned>> + 'a>> {
    Box::pin(async move {
        if obj.is_null() {
            return None;
        }
        if let Ok((role, name)) = identity(bus, &obj).await
            && selects(selector, &role, name.as_deref())
        {
            return Some(obj);
        }
        if depth == 0 {
            return None;
        }
        for child in children(bus, &obj).await.unwrap_or_default() {
            if let Some(found) = search(bus, child, selector, depth - 1).await {
                return Some(found);
            }
        }
        None
    })
}

async fn perform(
    bus: &AccessibilityConnection,
    target: &ObjectRefOwned,
    described: &Probe,
    action: &UiAction,
) -> Result<()> {
    let conn = bus.connection();
    match action {
        UiAction::Invoke => do_listed(conn, target, described, "invoke").await,
        UiAction::Toggle => do_listed(conn, target, described, "toggle").await,
        UiAction::Expand => {
            if described.states.contains(State::Expanded) {
                Ok(())
            } else {
                do_listed(conn, target, described, "expand").await
            }
        }
        UiAction::Collapse => {
            if described.states.contains(State::Expanded) {
                do_listed(conn, target, described, "collapse").await
            } else {
                Ok(())
            }
        }
        UiAction::Select => {
            if action_index(&described.actions, "select").is_some() {
                do_listed(conn, target, described, "select").await
            } else {
                select_in_parent(bus, target).await
            }
        }
        UiAction::Focus => {
            let component = proxy_on!(ComponentProxy, conn, target)?;
            if component.grab_focus().await.map_err(platform)? {
                Ok(())
            } else {
                Err(DesktopError::Platform("the control refused focus".into()))
            }
        }
        UiAction::SetValue { value } => set_value(conn, target, described, value).await,
    }
}

/// Runs the control's AT-SPI action for the apz action `wanted`.
async fn do_listed(
    conn: &zbus::Connection,
    target: &ObjectRefOwned,
    described: &Probe,
    wanted: &str,
) -> Result<()> {
    let index = action_index(&described.actions, wanted).ok_or_else(|| {
        DesktopError::Invalid(format!("a {} takes no {wanted} action", described.role))
    })?;
    let index = i32::try_from(index).map_err(platform)?;
    let listed = proxy_on!(ActionProxy, conn, target)?;
    if listed.do_action(index).await.map_err(platform)? {
        Ok(())
    } else {
        Err(DesktopError::Platform(format!(
            "the control did not perform {wanted}"
        )))
    }
}

/// Selects the control in its parent by index, for a parent that implements Selection.
async fn select_in_parent(bus: &AccessibilityConnection, target: &ObjectRefOwned) -> Result<()> {
    let conn = bus.connection();
    let (index, parent) = {
        let accessible = target.as_accessible_proxy(conn).await.map_err(platform)?;
        (
            accessible.get_index_in_parent().await.map_err(platform)?,
            accessible.parent().await.map_err(platform)?,
        )
    };
    if parent.is_null() {
        return Err(DesktopError::Unsupported(
            "the control has no parent to select it in".into(),
        ));
    }
    let parent_interfaces = {
        let accessible = parent.as_accessible_proxy(conn).await.map_err(platform)?;
        accessible.get_interfaces().await.map_err(platform)?
    };
    if !parent_interfaces.contains(Interface::Selection) {
        return Err(DesktopError::Unsupported(
            "the control's parent does not select its children".into(),
        ));
    }
    let selection = proxy_on!(SelectionProxy, conn, &parent)?;
    if selection.select_child(index).await.map_err(platform)? {
        Ok(())
    } else {
        Err(DesktopError::Platform(
            "the parent refused the selection".into(),
        ))
    }
}

/// Sets a text control's contents, or a value control's number.
async fn set_value(
    conn: &zbus::Connection,
    target: &ObjectRefOwned,
    described: &Probe,
    value: &str,
) -> Result<()> {
    if described.interfaces.contains(Interface::EditableText) {
        let editable = proxy_on!(EditableTextProxy, conn, target)?;
        return if editable.set_text_contents(value).await.map_err(platform)? {
            Ok(())
        } else {
            Err(DesktopError::Platform(
                "the control refused the text".into(),
            ))
        };
    }
    if described.interfaces.contains(Interface::Value) {
        let number: f64 = value.trim().parse().map_err(|_| {
            DesktopError::Invalid(format!("{value:?} is not a number for this control"))
        })?;
        let range = proxy_on!(ValueProxy, conn, target)?;
        return range.set_current_value(number).await.map_err(platform);
    }
    Err(DesktopError::Unsupported(
        "the control takes no value".into(),
    ))
}
