//! The UI tree and the UI actions through UI Automation. The walk uses the control view, and a child index path
//! follows the same view, so a path read from the tree names the control [`locate`] finds. Actions run through
//! the control's patterns and never move the owner's pointer.

use apz_desktop::{DesktopError, Result, UiNode, action_name};
use apz_station::{Selector, UiAction};
use windows::Win32::UI::Accessibility::{
    IUIAutomation, IUIAutomationElement, IUIAutomationExpandCollapsePattern,
    IUIAutomationInvokePattern, IUIAutomationSelectionItemPattern, IUIAutomationTogglePattern,
    IUIAutomationTreeWalker, IUIAutomationValuePattern, UIA_ExpandCollapsePatternId,
    UIA_InvokePatternId, UIA_PATTERN_ID, UIA_SelectionItemPatternId, UIA_TogglePatternId,
    UIA_ValuePatternId,
};
use windows::core::{BSTR, Interface};

use super::platform_error;
use crate::geometry::rect_bounds;
use crate::roles::{
    Capabilities, ExpandState, actions_for, role_name, selector_matches, toggle_value,
};

/// The most nodes one tree call returns: a browser or an IDE window can hold far more.
const MAX_NODES: usize = 2_000;
/// The most children one node lists.
const MAX_CHILDREN: usize = 500;
/// The most controls a selector search looks at.
const MAX_SEARCH: usize = 20_000;

/// A walker over the control view, with the budget left for one call.
struct Walk {
    walker: IUIAutomationTreeWalker,
    budget: usize,
    next_id: u64,
}

/// What one control reports, read once.
struct Facts {
    role: &'static str,
    name: Option<String>,
    value: Option<String>,
    bounds: (i32, i32, u32, u32),
    actions: Vec<&'static str>,
}

impl Walk {
    fn new(uia: &IUIAutomation) -> Result<Self> {
        // SAFETY: ControlViewWalker takes no pointers.
        let walker = unsafe { uia.ControlViewWalker() }.map_err(platform_error)?;
        Ok(Self {
            walker,
            budget: MAX_NODES,
            next_id: 0,
        })
    }

    /// The tree below `element`, `depth` levels down. A control that cannot be read fails the call; a child that
    /// cannot be read is left out.
    fn node(&mut self, element: &IUIAutomationElement, depth: u8) -> Result<UiNode> {
        self.next_id += 1;
        let id = self.next_id;
        let facts = describe(element)?;
        let children = if depth > 0 {
            self.children_below(element, depth - 1)
        } else {
            Vec::new()
        };
        Ok(UiNode {
            id,
            role: facts.role.to_string(),
            name: facts.name,
            value: facts.value,
            bounds: facts.bounds,
            actions: facts
                .actions
                .iter()
                .map(|action| action.to_string())
                .collect(),
            children,
        })
    }

    fn children_below(&mut self, parent: &IUIAutomationElement, depth: u8) -> Vec<UiNode> {
        let mut nodes = Vec::new();
        for child in self.children(parent) {
            if self.budget == 0 {
                break;
            }
            self.budget -= 1;
            if let Ok(node) = self.node(&child, depth) {
                nodes.push(node);
            }
        }
        nodes
    }

    /// The control-view children of `parent`, in order.
    fn children(&self, parent: &IUIAutomationElement) -> Vec<IUIAutomationElement> {
        let mut out = Vec::new();
        // SAFETY: the walker and the element are live; a null result means no child, and is filtered out.
        let mut next = unsafe { self.walker.GetFirstChildElement(parent) }
            .ok()
            .filter(|element| !element.as_raw().is_null());
        while let Some(child) = next {
            if out.len() >= MAX_CHILDREN {
                break;
            }
            // SAFETY: as above, for the sibling of `child`.
            next = unsafe { self.walker.GetNextSiblingElement(&child) }
                .ok()
                .filter(|element| !element.as_raw().is_null());
            out.push(child);
        }
        out
    }

    /// The first control under `parent`, in document order, that the selector's role and name match.
    fn search(
        &self,
        parent: &IUIAutomationElement,
        selector: &Selector,
        budget: &mut usize,
    ) -> Option<IUIAutomationElement> {
        for child in self.children(parent) {
            if *budget == 0 {
                return None;
            }
            *budget -= 1;
            let role = role_of(&child);
            let name = name_of(&child);
            if selector_matches(
                selector.role.as_deref(),
                selector.name.as_deref(),
                role,
                name.as_deref(),
            ) {
                return Some(child);
            }
            if let Some(found) = self.search(&child, selector, budget) {
                return Some(found);
            }
        }
        None
    }
}

/// The tree below `root`, `depth` levels down.
pub fn snapshot(uia: &IUIAutomation, root: &IUIAutomationElement, depth: u8) -> Result<UiNode> {
    let mut walk = Walk::new(uia)?;
    walk.node(root, depth)
}

/// The control a selector picks under `root`. A path walks child indices from the root; otherwise the first
/// control in document order whose role and name match is taken.
pub fn locate(
    uia: &IUIAutomation,
    root: &IUIAutomationElement,
    selector: &Selector,
) -> Result<IUIAutomationElement> {
    let walk = Walk::new(uia)?;
    if let Some(path) = &selector.path {
        let mut current = root.clone();
        for &index in path {
            current = walk
                .children(&current)
                .into_iter()
                .nth(index as usize)
                .ok_or_else(|| DesktopError::NotFound(format!("no control at path {path:?}")))?;
        }
        return Ok(current);
    }
    if selector.role.is_none() && selector.name.is_none() {
        return Err(DesktopError::Invalid(
            "a selector needs a path, a role or a name".into(),
        ));
    }
    let mut budget = MAX_SEARCH;
    walk.search(root, selector, &mut budget)
        .ok_or_else(|| DesktopError::NotFound(format!("no control matches {selector:?}")))
}

/// Runs an action on a control through the pattern that offers it. A control without the pattern refuses.
pub fn act(element: &IUIAutomationElement, action: &UiAction) -> Result<()> {
    let outcome: Option<windows::core::Result<()>> = match action {
        UiAction::Invoke => pattern::<IUIAutomationInvokePattern>(element, UIA_InvokePatternId)
            // SAFETY: the pattern is live for the call.
            .map(|pattern| unsafe { pattern.Invoke() }),
        UiAction::SetValue { value } => {
            pattern::<IUIAutomationValuePattern>(element, UIA_ValuePatternId).map(|pattern| {
                // SAFETY: the pattern is live; the value is copied into a BSTR for the call.
                unsafe { pattern.SetValue(&BSTR::from(value.as_str())) }
            })
        }
        // SAFETY: the element is live.
        UiAction::Focus => Some(unsafe { element.SetFocus() }),
        UiAction::Select => {
            pattern::<IUIAutomationSelectionItemPattern>(element, UIA_SelectionItemPatternId)
                // SAFETY: the pattern is live for the call.
                .map(|pattern| unsafe { pattern.Select() })
        }
        UiAction::Toggle => pattern::<IUIAutomationTogglePattern>(element, UIA_TogglePatternId)
            // SAFETY: as above.
            .map(|pattern| unsafe { pattern.Toggle() }),
        UiAction::Expand => {
            pattern::<IUIAutomationExpandCollapsePattern>(element, UIA_ExpandCollapsePatternId)
                // SAFETY: as above.
                .map(|pattern| unsafe { pattern.Expand() })
        }
        UiAction::Collapse => {
            pattern::<IUIAutomationExpandCollapsePattern>(element, UIA_ExpandCollapsePatternId)
                // SAFETY: as above.
                .map(|pattern| unsafe { pattern.Collapse() })
        }
    };
    match outcome {
        Some(result) => result.map_err(platform_error),
        None => Err(DesktopError::Unsupported(format!(
            "the control takes no {}",
            action_name(action)
        ))),
    }
}

/// What one control reports, read once.
fn describe(element: &IUIAutomationElement) -> Result<Facts> {
    // SAFETY: `element` is borrowed for the whole call, so the UIA interface stays alive. The getter writes
    // its result into an out value the windows wrapper owns; a failed read is returned as an error HRESULT.
    let control = unsafe { element.CurrentControlType() }.map_err(platform_error)?;
    // SAFETY: `element` is borrowed for the whole call. RECT is written to a wrapper-owned out value, and a
    // failed read (for example a provider that has gone away) is mapped to an error below.
    let rect = unsafe { element.CurrentBoundingRectangle() }.map_err(platform_error)?;
    // SAFETY: `element` is borrowed for the whole call. BOOL is written to a wrapper-owned out value, and a
    // failed read becomes `false` through `is_ok_and`.
    let focusable = unsafe { element.CurrentIsKeyboardFocusable() }.is_ok_and(|b| b.as_bool());
    // SAFETY: as for the focus flag above. A failed read leaves the control not treated as a password field.
    let password = unsafe { element.CurrentIsPassword() }.is_ok_and(|b| b.as_bool());
    let value_pattern = pattern::<IUIAutomationValuePattern>(element, UIA_ValuePatternId);
    let toggle_pattern = pattern::<IUIAutomationTogglePattern>(element, UIA_TogglePatternId);
    let expand_pattern =
        pattern::<IUIAutomationExpandCollapsePattern>(element, UIA_ExpandCollapsePatternId);
    let caps = Capabilities {
        focusable,
        invoke: pattern::<IUIAutomationInvokePattern>(element, UIA_InvokePatternId).is_some(),
        value_writable: value_pattern.as_ref().is_some_and(|pattern| {
            // SAFETY: the pattern is live for the call.
            unsafe { pattern.CurrentIsReadOnly() }.is_ok_and(|read_only| !read_only.as_bool())
        }),
        selectable: pattern::<IUIAutomationSelectionItemPattern>(
            element,
            UIA_SelectionItemPatternId,
        )
        .is_some(),
        toggle: toggle_pattern.is_some(),
        expand: expand_pattern
            .as_ref()
            // SAFETY: as above.
            .and_then(|pattern| unsafe { pattern.CurrentExpandCollapseState() }.ok())
            .map_or(ExpandState::Leaf, |state| ExpandState::from_raw(state.0)),
    };
    // A toggle reports its state as its value; a password box never reports its text.
    let value = if let Some(toggle) = &toggle_pattern {
        // SAFETY: as above.
        unsafe { toggle.CurrentToggleState() }
            .ok()
            .map(|state| toggle_value(state.0).to_string())
    } else if let (Some(pattern), false) = (&value_pattern, password) {
        // SAFETY: as above.
        text(unsafe { pattern.CurrentValue() }).filter(|value| !value.is_empty())
    } else {
        None
    };
    Ok(Facts {
        role: role_name(control.0),
        name: name_of(element),
        value,
        bounds: rect_bounds(rect.left, rect.top, rect.right, rect.bottom),
        actions: actions_for(caps),
    })
}

fn role_of(element: &IUIAutomationElement) -> &'static str {
    // SAFETY: the property read takes no pointers beyond its out value.
    unsafe { element.CurrentControlType() }.map_or("unknown", |control| role_name(control.0))
}

fn name_of(element: &IUIAutomationElement) -> Option<String> {
    // SAFETY: as above.
    text(unsafe { element.CurrentName() }).filter(|name| !name.is_empty())
}

fn text(result: windows::core::Result<BSTR>) -> Option<String> {
    result.ok().map(|value| value.to_string())
}

/// The pattern `id` of an element, as the interface `T`. A null or missing pattern is `None`.
fn pattern<T: Interface>(element: &IUIAutomationElement, id: UIA_PATTERN_ID) -> Option<T> {
    // SAFETY: the element is live; the result is an owned IUnknown or an error.
    let unknown = unsafe { element.GetCurrentPattern(id) }.ok()?;
    if unknown.as_raw().is_null() {
        return None;
    }
    unknown.cast::<T>().ok()
}
