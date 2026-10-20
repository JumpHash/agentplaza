//! The Accessibility tree of the machine: reading an element's attributes, and walking down from an app's windows
//! or from the desktop.
//!
//! Each walk keeps the element behind every node, in the order the nodes were visited, so `elements[node.id]` is the
//! element a [`ax::locate`] path leads to. A walk stops at [`MAX_NODES`].

use std::ffi::c_void;

use apz_desktop::{DesktopError, Result, UiNode};
use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{Boolean, CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::display::CGDisplay;
use core_graphics::geometry::{CGPoint, CGSize};

use crate::ax::{self, Raw, RawElement};
use crate::{sys, workspace};

/// The most nodes one walk reads. Some apps have very large trees; the cap keeps a walk bounded.
pub const MAX_NODES: usize = 4000;

/// A walked tree, and the element behind each node: `elements[node.id]`. The desktop root has no element.
pub struct Walk {
    pub root: UiNode,
    pub elements: Vec<Option<CFType>>,
}

/// The desktop: the running apps that have windows, each with its windows, down to `levels` below the root.
pub fn desktop(levels: u8) -> Result<Walk> {
    let mut walker = Walker {
        elements: vec![None],
    };
    let mut children = Vec::new();
    if levels > 0 {
        for app in workspace::running_apps() {
            if walker.full() {
                break;
            }
            let (Some(name), Some(element)) = (app.name, application_element(app.pid)) else {
                continue;
            };
            let windows = attr_items(&element, "AXWindows");
            if windows.is_empty() {
                continue;
            }
            children.push(walker.application(element, &name, windows, levels - 1));
        }
    }
    Ok(Walk {
        root: UiNode {
            id: 0,
            role: "desktop".into(),
            name: Some("Desktop".into()),
            value: None,
            bounds: main_display_bounds(),
            actions: Vec::new(),
            children,
        },
        elements: walker.elements,
    })
}

/// One app, from its process id: the application node, with its windows down to `levels` below the root.
pub fn app(pid: i32, name: &str, levels: u8) -> Result<Walk> {
    let element = application_element(pid).ok_or_else(|| {
        DesktopError::Platform(format!("could not open the accessibility tree of {name}"))
    })?;
    let windows = attr_items(&element, "AXWindows");
    let mut walker = Walker {
        elements: Vec::new(),
    };
    let root = walker.application(element, name, windows, levels);
    Ok(Walk {
        root,
        elements: walker.elements,
    })
}

struct Walker {
    elements: Vec<Option<CFType>>,
}

impl Walker {
    fn full(&self) -> bool {
        self.elements.len() >= MAX_NODES
    }

    fn push(&mut self, element: Option<CFType>) -> u64 {
        self.elements.push(element);
        (self.elements.len() - 1) as u64
    }

    /// An application node whose children are its windows. `levels` is how many levels of children it shows.
    fn application(
        &mut self,
        element: CFType,
        name: &str,
        windows: Vec<CFType>,
        levels: u8,
    ) -> UiNode {
        let id = self.push(Some(element));
        let mut children = Vec::new();
        if levels > 0 {
            for window in windows {
                if self.full() {
                    break;
                }
                children.push(self.node(window, levels - 1));
            }
        }
        UiNode {
            id,
            role: "application".into(),
            name: Some(name.to_string()),
            value: None,
            bounds: (0, 0, 0, 0),
            actions: Vec::new(),
            children,
        }
    }

    /// An element and its `AXChildren`, down to `levels` below it.
    fn node(&mut self, element: CFType, levels: u8) -> UiNode {
        let id = self.push(Some(element.clone()));
        let raw = read(&element);
        let mut children = Vec::new();
        if levels > 0 {
            for child in attr_items(&element, "AXChildren") {
                if self.full() {
                    break;
                }
                children.push(self.node(child, levels - 1));
            }
        }
        ax::node_from_raw(id, &raw, children)
    }
}

fn application_element(pid: i32) -> Option<CFType> {
    // SAFETY: AXUIElementCreateApplication takes a plain pid and borrows nothing from us. It returns a new (+1)
    // element, or null when the pid has no accessibility element.
    let element = unsafe { sys::AXUIElementCreateApplication(pid) };
    // SAFETY: the element is non-null, so it is the (+1) reference returned above. The create rule hands that one
    // reference to this CFType, which releases it on drop.
    (!element.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(element) })
}

fn main_display_bounds() -> (i32, i32, u32, u32) {
    let bounds = CGDisplay::main().bounds();
    (
        bounds.origin.x.round() as i32,
        bounds.origin.y.round() as i32,
        bounds.size.width.round() as u32,
        bounds.size.height.round() as u32,
    )
}

/// The attributes of one element, read from the platform.
fn read(element: &CFType) -> RawElement {
    let text = |attribute| {
        attr(element, attribute)
            .and_then(|value| value.downcast::<CFString>())
            .map(|text| text.to_string())
    };
    RawElement {
        role: text("AXRole"),
        title: text("AXTitle"),
        description: text("AXDescription"),
        value: raw_value(element),
        position: attr(element, "AXPosition").and_then(|value| point_of(&value)),
        size: attr(element, "AXSize").and_then(|value| size_of(&value)),
        actions: action_names(element),
        value_settable: settable(element, "AXValue"),
        focus_settable: settable(element, "AXFocused"),
        select_settable: settable(element, "AXSelected"),
    }
}

/// The value of an attribute, or `None` when the element does not have it.
fn attr(element: &CFType, name: &str) -> Option<CFType> {
    let key = CFString::new(name);
    let mut value: CFTypeRef = std::ptr::null();
    // SAFETY: `element` is a live AXUIElement that this call only borrows, and `key` is a CFString that the local `key`
    // keeps alive until the call returns. `value` is a local out-pointer: on success the callee writes a new (+1)
    // reference into it, and nothing reads it before the call has returned.
    let error = unsafe {
        sys::AXUIElementCopyAttributeValue(
            element.as_CFTypeRef(),
            key.as_concrete_TypeRef(),
            &mut value,
        )
    };
    if error != sys::AX_ERROR_SUCCESS || value.is_null() {
        return None;
    }
    // SAFETY: the call succeeded and `value` is non-null, so it is the new (+1) reference the callee wrote. The create
    // rule hands that reference to this CFType, which releases it on drop.
    Some(unsafe { CFType::wrap_under_create_rule(value) })
}

/// The items of an array attribute, each retained. Empty when the attribute is missing or not an array.
fn attr_items(element: &CFType, name: &str) -> Vec<CFType> {
    let Some(value) = attr(element, name) else {
        return Vec::new();
    };
    let Some(array) = value.downcast::<CFArray<*const c_void>>() else {
        return Vec::new();
    };
    // SAFETY: every item pointer was read from `array`, which is alive for this whole chain because `value` owns it
    // until this function returns. `get_all_values` copies the pointers out without retaining them. Under the get rule
    // each item is retained here, so every CFType returned owns its own reference and outlives `array` and `value`.
    array
        .get_all_values()
        .into_iter()
        .map(|item| unsafe { CFType::wrap_under_get_rule(item) })
        .collect()
}

fn raw_value(element: &CFType) -> Raw {
    let Some(value) = attr(element, "AXValue") else {
        return Raw::Absent;
    };
    if let Some(text) = value.downcast::<CFString>() {
        Raw::Text(text.to_string())
    } else if let Some(number) = value.downcast::<CFNumber>() {
        number.to_f64().map_or(Raw::Absent, Raw::Number)
    } else if let Some(flag) = value.downcast::<CFBoolean>() {
        Raw::Boolean(bool::from(flag))
    } else {
        Raw::Absent
    }
}

/// Whether `value` is an `AXValue` of the CoreGraphics type `value_type`. Attribute values come from other
/// applications, so each one is checked before `AXValueGetValue` reads it as that struct.
fn is_ax_value(value: &CFType, value_type: u32) -> bool {
    let reference = value.as_CFTypeRef();
    // SAFETY: `reference` is a live CF object for the whole call, borrowed from `value`. Both calls only read its type.
    // The type-ID comparison comes first, so AXValueGetType is only called on an AXValue, as it requires.
    unsafe {
        sys::CFGetTypeID(reference) == sys::AXValueGetTypeID()
            && sys::AXValueGetType(reference) == value_type
    }
}

fn point_of(value: &CFType) -> Option<(f64, f64)> {
    if !is_ax_value(value, sys::AX_VALUE_CG_POINT) {
        return None;
    }
    let mut point = CGPoint::new(0.0, 0.0);
    // SAFETY: `value` is an AXValue holding a CGPoint, as checked above, and it stays alive for the call. `point` is a
    // local CGPoint that only this call borrows, and the pointer passed is its own address, so the callee writes exactly
    // one CGPoint into valid memory.
    let ok = unsafe {
        sys::AXValueGetValue(
            value.as_CFTypeRef(),
            sys::AX_VALUE_CG_POINT,
            (&mut point as *mut CGPoint).cast::<c_void>(),
        )
    };
    (ok != 0).then_some((point.x, point.y))
}

fn size_of(value: &CFType) -> Option<(f64, f64)> {
    if !is_ax_value(value, sys::AX_VALUE_CG_SIZE) {
        return None;
    }
    let mut size = CGSize::new(0.0, 0.0);
    // SAFETY: `value` is an AXValue holding a CGSize, as checked above, and it stays alive for the call. `size` is a
    // local CGSize that only this call borrows, and the pointer passed is its own address, so the callee writes exactly
    // one CGSize into valid memory.
    let ok = unsafe {
        sys::AXValueGetValue(
            value.as_CFTypeRef(),
            sys::AX_VALUE_CG_SIZE,
            (&mut size as *mut CGSize).cast::<c_void>(),
        )
    };
    (ok != 0).then_some((size.width, size.height))
}

fn action_names(element: &CFType) -> Vec<String> {
    let mut names: CFArrayRef = std::ptr::null();
    // SAFETY: `element` is a live AXUIElement that this call only borrows, and `names` is a local out-pointer that the
    // callee writes a new (+1) array of action names into on success.
    let error = unsafe { sys::AXUIElementCopyActionNames(element.as_CFTypeRef(), &mut names) };
    if error != sys::AX_ERROR_SUCCESS || names.is_null() {
        return Vec::new();
    }
    // SAFETY: the call succeeded and `names` is non-null, so it is the (+1) array returned above. The create rule hands
    // it to this CFArray, which releases it on drop.
    let names = unsafe { CFArray::<CFString>::wrap_under_create_rule(names) };
    names.iter().map(|name| name.to_string()).collect()
}

fn settable(element: &CFType, name: &str) -> bool {
    let key = CFString::new(name);
    let mut settable: Boolean = 0;
    // SAFETY: `element` is a live AXUIElement that this call only borrows, `key` is a CFString that the local `key` keeps
    // alive until the call returns, and `settable` is a local Boolean that the callee writes through its pointer.
    let error = unsafe {
        sys::AXUIElementIsAttributeSettable(
            element.as_CFTypeRef(),
            key.as_concrete_TypeRef(),
            &mut settable,
        )
    };
    error == sys::AX_ERROR_SUCCESS && settable != 0
}

/// Sets an attribute of an element to a value (a string or a boolean, as a `CFType`).
pub fn set_attribute(element: &CFType, attribute: &str, value: &CFType) -> sys::AXError {
    let key = CFString::new(attribute);
    // SAFETY: element, key and value remain owned and live for the entire call.
    unsafe {
        sys::AXUIElementSetAttributeValue(
            element.as_CFTypeRef(),
            key.as_concrete_TypeRef(),
            value.as_CFTypeRef(),
        )
    }
}

/// Performs an accessibility action (such as `AXPress`) on an element.
pub fn perform_action(element: &CFType, action: &str) -> sys::AXError {
    let name = CFString::new(action);
    // SAFETY: element and name remain owned and live for the entire call.
    unsafe { sys::AXUIElementPerformAction(element.as_CFTypeRef(), name.as_concrete_TypeRef()) }
}
