//! The C functions the backend calls that the `core-graphics` and `core-foundation` crates do not wrap: the
//! Accessibility API (ApplicationServices), the screen-permission and idle-time calls of CoreGraphics,
//! `CGImageGetBitmapInfo`, and `CFGetTypeID`, which checks a value's type before it is read. The declarations are raw
//! FFI; the rest of the crate calls them through the helpers below.

use std::ffi::c_void;

use core_foundation::array::CFArrayRef;
use core_foundation::base::{Boolean, CFTypeID, CFTypeRef};
use core_foundation::dictionary::CFDictionaryRef;
use core_foundation::string::CFStringRef;

/// An `AXError`. Zero is success.
pub type AXError = i32;
pub const AX_ERROR_SUCCESS: AXError = 0;
/// `kAXErrorAttributeUnsupported`.
pub const AX_ERROR_ATTRIBUTE_UNSUPPORTED: AXError = -25205;
/// `kAXErrorActionUnsupported`.
pub const AX_ERROR_ACTION_UNSUPPORTED: AXError = -25206;
/// `kAXErrorNotImplemented`.
pub const AX_ERROR_NOT_IMPLEMENTED: AXError = -25208;
/// `kAXErrorAPIDisabled`: the process is not trusted for Accessibility.
pub const AX_ERROR_API_DISABLED: AXError = -25211;

/// `kAXValueCGPointType`.
pub const AX_VALUE_CG_POINT: u32 = 1;
/// `kAXValueCGSizeType`.
pub const AX_VALUE_CG_SIZE: u32 = 2;

/// `kCGEventSourceStateHIDSystemState`.
pub const HID_SYSTEM_STATE: i32 = 1;
/// `kCGAnyInputEventType`: any input event.
pub const ANY_INPUT_EVENT: u32 = u32::MAX;

#[link(name = "ApplicationServices", kind = "framework")]
#[allow(non_snake_case)]
unsafe extern "C" {
    pub fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> Boolean;
    /// Returns a new (retained) element, or null.
    pub fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
    /// Returns a new (retained) value through `value`.
    pub fn AXUIElementCopyAttributeValue(
        element: CFTypeRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    pub fn AXUIElementIsAttributeSettable(
        element: CFTypeRef,
        attribute: CFStringRef,
        settable: *mut Boolean,
    ) -> AXError;
    /// Returns a new (retained) array of action names through `names`.
    pub fn AXUIElementCopyActionNames(element: CFTypeRef, names: *mut CFArrayRef) -> AXError;
    pub fn AXUIElementPerformAction(element: CFTypeRef, action: CFStringRef) -> AXError;
    pub fn AXUIElementSetAttributeValue(
        element: CFTypeRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    /// Copies an `AXValue`'s CoreGraphics struct (`value_type` is `AX_VALUE_CG_POINT` or `AX_VALUE_CG_SIZE`).
    /// Only call it on a value that `AXValueGetTypeID` and `AXValueGetType` have confirmed is an `AXValue` of that type.
    pub fn AXValueGetValue(value: CFTypeRef, value_type: u32, output: *mut c_void) -> Boolean;
    /// The `CFTypeID` of `AXValue`. Compare a value's type ID with it before reading the value as one.
    pub fn AXValueGetTypeID() -> CFTypeID;
    /// The `AXValueType` of an `AXValue` (`AX_VALUE_CG_POINT`, `AX_VALUE_CG_SIZE`, ...). Only call it on an `AXValue`.
    pub fn AXValueGetType(value: CFTypeRef) -> u32;
}

#[link(name = "CoreFoundation", kind = "framework")]
#[allow(non_snake_case)]
unsafe extern "C" {
    /// The `CFTypeID` of any CoreFoundation object. A value's type is compared with `AXValueGetTypeID` through it.
    pub fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;
}

#[link(name = "CoreGraphics", kind = "framework")]
#[allow(non_snake_case)]
unsafe extern "C" {
    /// Whether the process may capture the screen (Screen Recording). Does not prompt.
    pub fn CGPreflightScreenCaptureAccess() -> bool;
    pub fn CGImageGetBitmapInfo(image: *const c_void) -> u32;
    /// Seconds since the last event of `event_type` in the source state.
    pub fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
}

/// Whether the process is trusted for Accessibility. Passing no options means it never prompts.
pub fn accessibility_trusted() -> bool {
    // SAFETY: a null `options` is what AXIsProcessTrustedWithOptions accepts for a check that never prompts. No pointer
    // of ours is passed, and the call returns a plain Boolean that we do not own.
    unsafe { AXIsProcessTrustedWithOptions(std::ptr::null()) != 0 }
}

/// Whether the process may capture the screen.
pub fn screen_recording_granted() -> bool {
    // SAFETY: CGPreflightScreenCaptureAccess takes no arguments and returns a plain bool. It does not prompt and hands
    // back no object, so there is no pointer or ownership to uphold.
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// Seconds since any input event reached the machine, from the owner or from us.
pub fn seconds_since_input() -> f64 {
    // SAFETY: both arguments are plain integers (the HID system state source and the any-input event type). The call
    // reads the source's idle counter and returns a double; no pointer or ownership is involved.
    unsafe { CGEventSourceSecondsSinceLastEventType(HID_SYSTEM_STATE, ANY_INPUT_EVENT) }
}
