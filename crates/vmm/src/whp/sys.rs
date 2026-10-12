// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port. The types and constants come from `windows-sys`
// (generated from the Windows SDK metadata, so the structure layouts are the
// SDK's, not typed from memory); only the function pointers are resolved here.

//! `WinHvPlatform.dll` and `WinHvEmulation.dll`, loaded at run time.
//!
//! Both DLLs exist only where the optional "Windows Hypervisor Platform"
//! feature is installed. Importing them would make every binary that links
//! this crate fail to start on a machine without the feature, so they are
//! opened with `LoadLibraryExW(.., LOAD_LIBRARY_SEARCH_SYSTEM32)` on the first
//! VM boot and their entry points resolved with `GetProcAddress`. A missing
//! DLL, a missing entry point or an absent hypervisor is a boot error that
//! names what to enable.

#![allow(non_snake_case)]
// FFI boundary (see the allowlist in `lib.rs`): the library loading, the
// transmutes of the resolved entry points to their SDK signatures, and the
// thin `call!` wrapper through which every WHP call is made.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::Hypervisor::*;
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::core::HRESULT;

/// Errors of the WHP boundary.
#[derive(Debug, thiserror::Error)]
pub enum WhpError {
    /// The DLL could not be loaded: the optional feature is not installed.
    #[error(
        "{dll} could not be loaded (Windows error {code}): enable \"Windows Hypervisor \
         Platform\" in Windows Features (optionalfeatures.exe) and restart"
    )]
    NotInstalled {
        /// The DLL that is missing.
        dll: &'static str,
        /// `GetLastError` of the failed load.
        code: u32,
    },
    /// An entry point is missing from an installed DLL (an older Windows).
    #[error("{dll} has no {symbol}: this Windows is too old for the WHP backend")]
    MissingSymbol {
        /// The DLL that was loaded.
        dll: &'static str,
        /// The entry point that is missing.
        symbol: &'static str,
    },
    /// The platform is installed but no hypervisor is running.
    #[error(
        "the Windows Hypervisor Platform reports no hypervisor: enable virtualization in the \
         firmware and \"Windows Hypervisor Platform\" in Windows Features"
    )]
    NoHypervisor,
    /// A WHP call failed.
    #[error("{call} failed: HRESULT {hresult:#010x}")]
    Call {
        /// The WHP function that failed.
        call: &'static str,
        /// Its result.
        hresult: u32,
    },
    /// The instruction emulator could not complete an access.
    #[error("the WHP instruction emulator could not complete a {what} access (status {status:#x})")]
    Emulation {
        /// "port I/O" or "MMIO".
        what: &'static str,
        /// The `WHV_EMULATOR_STATUS` bits.
        status: u32,
    },
}

/// Maps an `HRESULT` to `Ok` or [`WhpError::Call`].
pub fn check(call: &'static str, hresult: HRESULT) -> Result<(), WhpError> {
    if hresult >= 0 {
        Ok(())
    } else {
        Err(WhpError::Call {
            call,
            hresult: hresult as u32,
        })
    }
}

type WHvGetCapabilityFn =
    unsafe extern "system" fn(WHV_CAPABILITY_CODE, *mut c_void, u32, *mut u32) -> HRESULT;
type WHvCreatePartitionFn = unsafe extern "system" fn(*mut WHV_PARTITION_HANDLE) -> HRESULT;
type WHvPartitionFn = unsafe extern "system" fn(WHV_PARTITION_HANDLE) -> HRESULT;
type WHvSetPartitionPropertyFn = unsafe extern "system" fn(
    WHV_PARTITION_HANDLE,
    WHV_PARTITION_PROPERTY_CODE,
    *const c_void,
    u32,
) -> HRESULT;
type WHvMapGpaRangeFn = unsafe extern "system" fn(
    WHV_PARTITION_HANDLE,
    *const c_void,
    u64,
    u64,
    WHV_MAP_GPA_RANGE_FLAGS,
) -> HRESULT;
type WHvUnmapGpaRangeFn = unsafe extern "system" fn(WHV_PARTITION_HANDLE, u64, u64) -> HRESULT;
type WHvCreateVirtualProcessorFn =
    unsafe extern "system" fn(WHV_PARTITION_HANDLE, u32, u32) -> HRESULT;
type WHvDeleteVirtualProcessorFn = unsafe extern "system" fn(WHV_PARTITION_HANDLE, u32) -> HRESULT;
type WHvRunVirtualProcessorFn =
    unsafe extern "system" fn(WHV_PARTITION_HANDLE, u32, *mut c_void, u32) -> HRESULT;
type WHvCancelRunVirtualProcessorFn =
    unsafe extern "system" fn(WHV_PARTITION_HANDLE, u32, u32) -> HRESULT;
type WHvGetVirtualProcessorRegistersFn = unsafe extern "system" fn(
    WHV_PARTITION_HANDLE,
    u32,
    *const WHV_REGISTER_NAME,
    u32,
    *mut WHV_REGISTER_VALUE,
) -> HRESULT;
type WHvSetVirtualProcessorRegistersFn = unsafe extern "system" fn(
    WHV_PARTITION_HANDLE,
    u32,
    *const WHV_REGISTER_NAME,
    u32,
    *const WHV_REGISTER_VALUE,
) -> HRESULT;
type WHvRequestInterruptFn =
    unsafe extern "system" fn(WHV_PARTITION_HANDLE, *const WHV_INTERRUPT_CONTROL, u32) -> HRESULT;
type WHvTranslateGvaFn = unsafe extern "system" fn(
    WHV_PARTITION_HANDLE,
    u32,
    u64,
    WHV_TRANSLATE_GVA_FLAGS,
    *mut WHV_TRANSLATE_GVA_RESULT,
    *mut u64,
) -> HRESULT;
type WHvEmulatorCreateEmulatorFn =
    unsafe extern "system" fn(*const WHV_EMULATOR_CALLBACKS, *mut *mut c_void) -> HRESULT;
type WHvEmulatorDestroyEmulatorFn = unsafe extern "system" fn(*const c_void) -> HRESULT;
type WHvEmulatorTryIoEmulationFn = unsafe extern "system" fn(
    *const c_void,
    *const c_void,
    *const WHV_VP_EXIT_CONTEXT,
    *const WHV_X64_IO_PORT_ACCESS_CONTEXT,
    *mut WHV_EMULATOR_STATUS,
) -> HRESULT;
type WHvEmulatorTryMmioEmulationFn = unsafe extern "system" fn(
    *const c_void,
    *const c_void,
    *const WHV_VP_EXIT_CONTEXT,
    *const WHV_MEMORY_ACCESS_CONTEXT,
    *mut WHV_EMULATOR_STATUS,
) -> HRESULT;

/// The resolved entry points of both DLLs.
///
/// The modules are never freed: the table is process-global and lives as long
/// as the process does.
pub struct Api {
    pub WHvGetCapability: WHvGetCapabilityFn,
    pub WHvCreatePartition: WHvCreatePartitionFn,
    pub WHvSetupPartition: WHvPartitionFn,
    pub WHvDeletePartition: WHvPartitionFn,
    pub WHvSetPartitionProperty: WHvSetPartitionPropertyFn,
    pub WHvMapGpaRange: WHvMapGpaRangeFn,
    pub WHvUnmapGpaRange: WHvUnmapGpaRangeFn,
    pub WHvCreateVirtualProcessor: WHvCreateVirtualProcessorFn,
    pub WHvDeleteVirtualProcessor: WHvDeleteVirtualProcessorFn,
    pub WHvRunVirtualProcessor: WHvRunVirtualProcessorFn,
    pub WHvCancelRunVirtualProcessor: WHvCancelRunVirtualProcessorFn,
    pub WHvGetVirtualProcessorRegisters: WHvGetVirtualProcessorRegistersFn,
    pub WHvSetVirtualProcessorRegisters: WHvSetVirtualProcessorRegistersFn,
    pub WHvRequestInterrupt: WHvRequestInterruptFn,
    pub WHvTranslateGva: WHvTranslateGvaFn,
    pub WHvEmulatorCreateEmulator: WHvEmulatorCreateEmulatorFn,
    pub WHvEmulatorDestroyEmulator: WHvEmulatorDestroyEmulatorFn,
    pub WHvEmulatorTryIoEmulation: WHvEmulatorTryIoEmulationFn,
    pub WHvEmulatorTryMmioEmulation: WHvEmulatorTryMmioEmulationFn,
}

const PLATFORM_DLL: &str = "WinHvPlatform.dll";
const EMULATION_DLL: &str = "WinHvEmulation.dll";

fn load(dll: &'static str) -> Result<HMODULE, WhpError> {
    let wide: Vec<u16> = dll.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call;
    // the search is restricted to System32, so no DLL is taken from the
    // current directory or PATH.
    let module = unsafe {
        LoadLibraryExW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    if module.is_null() {
        // SAFETY: reads the calling thread's last-error value, set by the
        // failed load just above.
        let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        return Err(WhpError::NotInstalled { dll, code });
    }
    Ok(module)
}

/// Resolves `symbol` (NUL-terminated) in `module` as a function of type `F`.
///
/// # Safety
///
/// `F` must be the exact `extern "system"` signature the DLL exports under
/// that name.
unsafe fn symbol<F: Copy>(
    module: HMODULE,
    dll: &'static str,
    symbol: &'static str,
) -> Result<F, WhpError> {
    debug_assert!(symbol.ends_with('\0'));
    // SAFETY: `module` is a loaded module handle and `symbol` a NUL-terminated
    // ASCII string.
    let address = unsafe { GetProcAddress(module, symbol.as_ptr()) };
    match address {
        // SAFETY: function pointers are pointer-sized; the caller guarantees
        // `F` is the export's signature.
        Some(function) => Ok(unsafe { std::mem::transmute_copy::<_, F>(&function) }),
        None => Err(WhpError::MissingSymbol {
            dll,
            symbol: symbol.trim_end_matches('\0'),
        }),
    }
}

impl Api {
    fn load() -> Result<Api, WhpError> {
        let platform = load(PLATFORM_DLL)?;
        let emulation = load(EMULATION_DLL)?;
        macro_rules! resolve {
            ($module:expr, $dll:expr, $name:literal) => {
                // SAFETY: each field's type is the SDK signature of the export
                // with the same name (the `windows-sys` declarations).
                unsafe { symbol($module, $dll, concat!($name, "\0"))? }
            };
        }
        Ok(Api {
            WHvGetCapability: resolve!(platform, PLATFORM_DLL, "WHvGetCapability"),
            WHvCreatePartition: resolve!(platform, PLATFORM_DLL, "WHvCreatePartition"),
            WHvSetupPartition: resolve!(platform, PLATFORM_DLL, "WHvSetupPartition"),
            WHvDeletePartition: resolve!(platform, PLATFORM_DLL, "WHvDeletePartition"),
            WHvSetPartitionProperty: resolve!(platform, PLATFORM_DLL, "WHvSetPartitionProperty"),
            WHvMapGpaRange: resolve!(platform, PLATFORM_DLL, "WHvMapGpaRange"),
            WHvUnmapGpaRange: resolve!(platform, PLATFORM_DLL, "WHvUnmapGpaRange"),
            WHvCreateVirtualProcessor: resolve!(platform, PLATFORM_DLL, "WHvCreateVirtualProcessor"),
            WHvDeleteVirtualProcessor: resolve!(platform, PLATFORM_DLL, "WHvDeleteVirtualProcessor"),
            WHvRunVirtualProcessor: resolve!(platform, PLATFORM_DLL, "WHvRunVirtualProcessor"),
            WHvCancelRunVirtualProcessor: resolve!(
                platform,
                PLATFORM_DLL,
                "WHvCancelRunVirtualProcessor"
            ),
            WHvGetVirtualProcessorRegisters: resolve!(
                platform,
                PLATFORM_DLL,
                "WHvGetVirtualProcessorRegisters"
            ),
            WHvSetVirtualProcessorRegisters: resolve!(
                platform,
                PLATFORM_DLL,
                "WHvSetVirtualProcessorRegisters"
            ),
            WHvRequestInterrupt: resolve!(platform, PLATFORM_DLL, "WHvRequestInterrupt"),
            WHvTranslateGva: resolve!(platform, PLATFORM_DLL, "WHvTranslateGva"),
            WHvEmulatorCreateEmulator: resolve!(emulation, EMULATION_DLL, "WHvEmulatorCreateEmulator"),
            WHvEmulatorDestroyEmulator: resolve!(
                emulation,
                EMULATION_DLL,
                "WHvEmulatorDestroyEmulator"
            ),
            WHvEmulatorTryIoEmulation: resolve!(emulation, EMULATION_DLL, "WHvEmulatorTryIoEmulation"),
            WHvEmulatorTryMmioEmulation: resolve!(
                emulation,
                EMULATION_DLL,
                "WHvEmulatorTryMmioEmulation"
            ),
        })
    }
}

static API: OnceLock<Result<Api, String>> = OnceLock::new();

/// The WHP entry points, loaded on first use. A load failure is remembered
/// (as its message) so every later boot reports the same cause.
pub fn api() -> Result<&'static Api, WhpError> {
    match API.get_or_init(|| Api::load().map_err(|err| err.to_string())) {
        Ok(api) => Ok(api),
        // Re-run the load for a typed error: the first failure's cause (a
        // missing DLL or entry point) does not change while the process runs.
        Err(_) => Err(Api::load().err().unwrap_or(WhpError::NoHypervisor)),
    }
}

/// Calls a WHP entry point through [`api`] and checks its `HRESULT`.
///
/// `call!(api, WHvSetupPartition(handle))` expands to the call and a
/// [`check`] naming the function. The expansion is `unsafe`: every caller
/// states the call's own safety conditions in a `// SAFETY:` comment.
macro_rules! call {
    ($api:expr, $name:ident($($arg:expr),* $(,)?)) => {
        $crate::whp::sys::check(stringify!($name), ($api.$name)($($arg),*))
    };
}
pub(crate) use call;

/// Reads a fixed-size WHP capability.
pub fn capability<T: Copy + Default>(api: &Api, code: WHV_CAPABILITY_CODE) -> Result<T, WhpError> {
    let mut value = T::default();
    let mut written = 0u32;
    // SAFETY: `value` is a writable `T` whose size is passed as the buffer
    // size; the call writes at most that many bytes and the written count.
    unsafe {
        call!(
            api,
            WHvGetCapability(
                code,
                (&raw mut value).cast(),
                std::mem::size_of::<T>() as u32,
                &mut written,
            )
        )?;
    }
    Ok(value)
}

/// Whether a hypervisor is present (`WHvCapabilityCodeHypervisorPresent`).
pub fn hypervisor_present(api: &Api) -> Result<bool, WhpError> {
    let present: i32 = capability(api, WHvCapabilityCodeHypervisorPresent)?;
    Ok(present != 0)
}
