//! The loopback exemption an AppContainer needs to reach the hub's egress proxy (Windows only).
//!
//! An AppContainer reaches loopback only while its SID is on the exemption list, and only an administrator can
//! change that list. The exemption opens every loopback port, so a persistent WFP filter per container also blocks
//! its loopback connections to every port but the one asked for. Windows Firewall rules do not reach AppContainer
//! loopback, so the filter is what narrows it. Reading the list and checking a SID need no administrator.
//!
//! [`container_sid`] and [`is_exempt`] run as the hub does. [`grant`] and [`revoke`] change the list and the filters,
//! so the hub runs them in an elevated helper (`apid windows-loopback`), started by [`run_elevated`].

use std::io;
use std::mem::{size_of, zeroed};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr::{null, null_mut};
use std::slice;

use windows_sys::Win32::Foundation::{ERROR_CANCELLED, FALSE, HANDLE};
use windows_sys::Win32::NetworkManagement::WindowsFilteringPlatform::{
    FWP_ACTION_BLOCK, FWP_EMPTY, FWP_MATCH_EQUAL, FWP_MATCH_NOT_EQUAL, FWP_SID, FWP_UINT16,
    FWP_V4_ADDR_AND_MASK, FWP_V4_ADDR_MASK, FWP_V6_ADDR_AND_MASK, FWP_V6_ADDR_MASK, FWP_VALUE0,
    FWPM_ACTION0, FWPM_CONDITION_ALE_PACKAGE_ID, FWPM_CONDITION_IP_REMOTE_ADDRESS_V4,
    FWPM_CONDITION_IP_REMOTE_ADDRESS_V6, FWPM_CONDITION_IP_REMOTE_PORT, FWPM_DISPLAY_DATA0,
    FWPM_FILTER_CONDITION0, FWPM_FILTER_FLAG_PERSISTENT, FWPM_FILTER0,
    FWPM_LAYER_ALE_AUTH_CONNECT_V4, FWPM_LAYER_ALE_AUTH_CONNECT_V6, FWPM_SUBLAYER_UNIVERSAL,
    FwpmEngineClose0, FwpmEngineOpen0, FwpmFilterAdd0, FwpmFilterDeleteByKey0,
};
use windows_sys::Win32::NetworkManagement::WindowsFirewall::{
    NetworkIsolationGetAppContainerConfig, NetworkIsolationSetAppContainerConfig,
};
use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSidToSidW};
use windows_sys::Win32::Security::{EqualSid, PSID, SID_AND_ATTRIBUTES};
use windows_sys::Win32::System::RemoteDesktop::{
    ProcessIdToSessionId, WTSGetActiveConsoleSessionId,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, GetExitCodeProcess, INFINITE, WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows_sys::core::{GUID, PWSTR};

use crate::SandboxError;
use crate::windows::{Local, os_error, sid_of, wide, win32_error};

/// The prefix of every AppContainer SID.
const APP_CONTAINER: &str = "S-1-15-2-";
/// `RPC_C_AUTHN_WINNT`: the engine is opened over the Windows authentication service.
const RPC_C_AUTHN_WINNT: u32 = 10;
/// `FWP_E_FILTER_NOT_FOUND`: there is no filter with the key to delete.
const FILTER_NOT_FOUND: u32 = 0x8032_0003;
/// `::1`, the IPv6 loopback address.
const LOOPBACK_V6: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
/// `127.0.0.0/8`, the IPv4 loopback network, in host order as WFP takes addresses.
const LOOPBACK_V4: u32 = 0x7F00_0000;
const LOOPBACK_V4_MASK: u32 = 0xFF00_0000;

/// The AppContainer SID that `workdir` runs under, as text (`S-1-15-2-...`). The profile is created on first use,
/// as the sandbox creates it on spawn.
pub fn container_sid(workdir: &Path) -> Result<String, SandboxError> {
    let dir = std::fs::canonicalize(workdir)
        .map_or_else(|_| workdir.to_path_buf(), crate::strip_verbatim);
    text_of(sid_of(&dir)?.0)
}

/// Whether the container `sid` is on the exemption list, so it may reach loopback. Needs no administrator.
pub fn is_exempt(sid: &str) -> Result<bool, SandboxError> {
    let sid = parse(sid)?;
    exempt(sid.0)
}

/// Gives the container `sid` the loopback exemption, and a filter that keeps its loopback connections to `port`.
/// Needs an administrator.
pub fn grant(sid: &str, port: u16) -> Result<(), SandboxError> {
    let psid = parse(sid)?;
    // The filters go first: the exemption alone would open every loopback port.
    let engine = Engine::open()?;
    for family in [Family::V4, Family::V6] {
        let key = filter_key(sid, family);
        remove_filter(&engine, key)?;
        add_block(&engine, psid.0, sid, family, port, key)?;
    }
    set_exempt(psid.0, true)
}

/// Takes the exemption and both filters away from the container `sid`. Needs an administrator.
pub fn revoke(sid: &str) -> Result<(), SandboxError> {
    let psid = parse(sid)?;
    set_exempt(psid.0, false)?;
    let engine = Engine::open()?;
    for family in [Family::V4, Family::V6] {
        remove_filter(&engine, filter_key(sid, family))?;
    }
    Ok(())
}

/// Whether Windows' administrator prompt can reach the owner from here: this process runs in a user's desktop
/// session (session 0 is where services run), and a user is logged on at the console.
pub fn has_desktop() -> bool {
    let mut session: u32 = 0;
    // SAFETY: `session` is a valid out pointer. A failed call leaves it at 0, which is no desktop.
    unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) };
    // SAFETY: a plain query with no pointers.
    session != 0 && unsafe { WTSGetActiveConsoleSessionId() } != u32::MAX
}

/// Runs `exe` with `args` under Windows' elevation prompt (the `runas` verb), hidden, and waits for it. Returns its
/// exit code. A declined prompt is an error.
pub fn run_elevated(exe: &Path, args: &[String]) -> Result<u32, SandboxError> {
    let verb = wide("runas");
    let file = wide(exe.as_os_str());
    let params = wide(join(args));
    // SAFETY: the struct is plain data, and all-zero is a valid value for it.
    let mut info: SHELLEXECUTEINFOW = unsafe { zeroed() };
    info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.nShow = SW_HIDE;
    // SAFETY: `info` and the strings it points at live through the call.
    if unsafe { ShellExecuteExW(&mut info) } == FALSE {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(ERROR_CANCELLED as i32) {
            return Err(SandboxError::Io(io::Error::other(
                "the administrator prompt was declined",
            )));
        }
        return Err(SandboxError::Io(io::Error::new(
            err.kind(),
            format!("ShellExecuteExW: {err}"),
        )));
    }
    if info.hProcess.is_null() {
        return Err(SandboxError::Io(io::Error::other(
            "the elevated helper did not start",
        )));
    }
    // SAFETY: ShellExecuteExW gave us this process handle to own.
    let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    // SAFETY: the handle is valid until `process` drops.
    if unsafe { WaitForSingleObject(process.as_raw_handle(), INFINITE) } == u32::MAX {
        return Err(os_error("WaitForSingleObject"));
    }
    let mut code: u32 = 0;
    // SAFETY: `code` is a valid out pointer for the call.
    if unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) } == FALSE {
        return Err(os_error("GetExitCodeProcess"));
    }
    Ok(code)
}

/// Whether `sid` is on the exemption list.
pub(crate) fn exempt(sid: PSID) -> Result<bool, SandboxError> {
    let (entries, _list) = read_list()?;
    // SAFETY: both SIDs are valid for the call; the entries live in `_list`.
    Ok(entries
        .iter()
        .any(|entry| unsafe { EqualSid(entry.Sid, sid) } != FALSE))
}

/// The SID as text, e.g. `S-1-15-2-...`.
fn text_of(sid: PSID) -> Result<String, SandboxError> {
    let mut text: PWSTR = null_mut();
    // SAFETY: `text` is a valid out pointer. The API allocates the string, freed by `_text`.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == FALSE {
        return Err(os_error("ConvertSidToStringSidW"));
    }
    let _text = Local(text.cast());
    // SAFETY: the API returns a NUL-terminated string, alive until `_text` is freed.
    let units = unsafe {
        let mut len = 0;
        while *text.add(len) != 0 {
            len += 1;
        }
        slice::from_raw_parts(text, len)
    };
    Ok(String::from_utf16_lossy(units))
}

/// The SID for `text`, freed on drop. Only AppContainer SIDs are accepted: the helper changes nothing else.
fn parse(text: &str) -> Result<Local, SandboxError> {
    if !text.starts_with(APP_CONTAINER) {
        return Err(SandboxError::Invalid(format!(
            "not an AppContainer SID: {text}"
        )));
    }
    let wide_text = wide(text);
    let mut sid: PSID = null_mut();
    // SAFETY: the string is NUL-terminated for the call. The API allocates `sid`, freed by the guard.
    if unsafe { ConvertStringSidToSidW(wide_text.as_ptr(), &mut sid) } == FALSE {
        return Err(os_error("ConvertStringSidToSidW"));
    }
    Ok(Local(sid))
}

/// The exemption list as the API gives it, with the block that holds its entries (freed on drop).
fn read_list() -> Result<(Vec<SID_AND_ATTRIBUTES>, Local), SandboxError> {
    let mut count: u32 = 0;
    let mut first: *mut SID_AND_ATTRIBUTES = null_mut();
    // SAFETY: both out pointers are valid for writes.
    let rc = unsafe { NetworkIsolationGetAppContainerConfig(&mut count, &mut first) };
    if rc != 0 {
        return Err(win32_error("NetworkIsolationGetAppContainerConfig", rc));
    }
    let block = Local(first.cast());
    let entries = if first.is_null() {
        Vec::new()
    } else {
        // SAFETY: the API wrote `count` entries at `first`, alive until `block` is freed.
        unsafe { slice::from_raw_parts(first, count as usize) }.to_vec()
    };
    Ok((entries, block))
}

/// Adds or removes `sid` on the exemption list, keeping every other entry: the API replaces the whole list.
fn set_exempt(sid: PSID, on: bool) -> Result<(), SandboxError> {
    let (mut entries, _list) = read_list()?;
    // SAFETY: both SIDs are valid for the call.
    let at = entries
        .iter()
        .position(|entry| unsafe { EqualSid(entry.Sid, sid) } != FALSE);
    match (at, on) {
        (Some(_), true) | (None, false) => return Ok(()),
        (Some(index), false) => {
            entries.remove(index);
        }
        (None, true) => entries.push(SID_AND_ATTRIBUTES {
            Sid: sid,
            Attributes: 0,
        }),
    }
    // SAFETY: each entry points at a SID that lives through the call (in `_list`, or `sid`).
    let rc =
        unsafe { NetworkIsolationSetAppContainerConfig(entries.len() as u32, entries.as_ptr()) };
    if rc != 0 {
        return Err(win32_error("NetworkIsolationSetAppContainerConfig", rc));
    }
    Ok(())
}

/// An open WFP engine, closed on drop.
struct Engine(HANDLE);

impl Engine {
    fn open() -> Result<Self, SandboxError> {
        let mut handle: HANDLE = null_mut();
        // SAFETY: a null server, identity and session mean the local engine and the default session. The out
        // pointer is valid for the call.
        let rc = unsafe { FwpmEngineOpen0(null(), RPC_C_AUTHN_WINNT, null(), null(), &mut handle) };
        if rc != 0 {
            return Err(win32_error("FwpmEngineOpen0", rc));
        }
        Ok(Self(handle))
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // SAFETY: the engine was opened by `open` and is closed once.
        unsafe { FwpmEngineClose0(self.0) };
    }
}

/// Deletes the filter with `key`, if it is there.
fn remove_filter(engine: &Engine, key: u128) -> Result<(), SandboxError> {
    let key = GUID::from_u128(key);
    // SAFETY: the engine is open and the key lives through the call.
    match unsafe { FwpmFilterDeleteByKey0(engine.0, &key) } {
        0 | FILTER_NOT_FOUND => Ok(()),
        rc => Err(win32_error("FwpmFilterDeleteByKey0", rc)),
    }
}

/// A persistent filter that blocks the container's loopback connections to every port but `port`.
fn add_block(
    engine: &Engine,
    sid: PSID,
    sid_text: &str,
    family: Family,
    port: u16,
    key: u128,
) -> Result<(), SandboxError> {
    let mut v4 = FWP_V4_ADDR_AND_MASK {
        addr: LOOPBACK_V4,
        mask: LOOPBACK_V4_MASK,
    };
    let mut v6 = FWP_V6_ADDR_AND_MASK {
        addr: LOOPBACK_V6,
        prefixLength: 128,
    };
    let mut conditions = [FWPM_FILTER_CONDITION0::default(); 3];
    // The container's package SID.
    conditions[0].fieldKey = FWPM_CONDITION_ALE_PACKAGE_ID;
    conditions[0].matchType = FWP_MATCH_EQUAL;
    conditions[0].conditionValue.r#type = FWP_SID;
    conditions[0].conditionValue.Anonymous.sid = sid.cast();
    // The loopback network of the filter's family.
    conditions[1].fieldKey = family.address();
    conditions[1].matchType = FWP_MATCH_EQUAL;
    match family {
        Family::V4 => {
            conditions[1].conditionValue.r#type = FWP_V4_ADDR_MASK;
            conditions[1].conditionValue.Anonymous.v4AddrMask = &mut v4;
        }
        Family::V6 => {
            conditions[1].conditionValue.r#type = FWP_V6_ADDR_MASK;
            conditions[1].conditionValue.Anonymous.v6AddrMask = &mut v6;
        }
    }
    // Every remote port but the proxy's.
    conditions[2].fieldKey = FWPM_CONDITION_IP_REMOTE_PORT;
    conditions[2].matchType = FWP_MATCH_NOT_EQUAL;
    conditions[2].conditionValue.r#type = FWP_UINT16;
    conditions[2].conditionValue.Anonymous.uint16 = port;

    let name = wide(format!("apz loopback {} {sid_text}", family.name()));
    let filter = FWPM_FILTER0 {
        filterKey: GUID::from_u128(key),
        displayData: FWPM_DISPLAY_DATA0 {
            name: name.as_ptr().cast_mut(),
            description: null_mut(),
        },
        flags: FWPM_FILTER_FLAG_PERSISTENT,
        layerKey: family.layer(),
        subLayerKey: FWPM_SUBLAYER_UNIVERSAL,
        weight: FWP_VALUE0 {
            r#type: FWP_EMPTY,
            ..Default::default()
        },
        numFilterConditions: conditions.len() as u32,
        filterCondition: conditions.as_mut_ptr(),
        action: FWPM_ACTION0 {
            r#type: FWP_ACTION_BLOCK,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut id: u64 = 0;
    // SAFETY: the engine is open. The filter, its conditions and the values they point at live through the call.
    let rc = unsafe { FwpmFilterAdd0(engine.0, &filter, null_mut(), &mut id) };
    if rc != 0 {
        return Err(win32_error("FwpmFilterAdd0", rc));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum Family {
    V4,
    V6,
}

impl Family {
    fn name(self) -> &'static str {
        match self {
            Family::V4 => "v4",
            Family::V6 => "v6",
        }
    }

    fn layer(self) -> GUID {
        match self {
            Family::V4 => FWPM_LAYER_ALE_AUTH_CONNECT_V4,
            Family::V6 => FWPM_LAYER_ALE_AUTH_CONNECT_V6,
        }
    }

    fn address(self) -> GUID {
        match self {
            Family::V4 => FWPM_CONDITION_IP_REMOTE_ADDRESS_V4,
            Family::V6 => FWPM_CONDITION_IP_REMOTE_ADDRESS_V6,
        }
    }
}

/// The key of the filter of `family` on the container `sid`: two hashes of the text make the 128 bits, so the same
/// container always has the same keys, and revoke finds them.
fn filter_key(sid: &str, family: Family) -> u128 {
    let text = format!("apz loopback {} {sid}", family.name());
    let high = fnv(0xcbf2_9ce4_8422_2325, text.as_bytes());
    let low = fnv(0x0000_0100_0000_01b3, text.as_bytes());
    (u128::from(high) << 64) | u128::from(low)
}

/// FNV-1a over `bytes`, from `seed`.
fn fnv(seed: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(seed, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The arguments as one command line, each in quotes when it is empty or has a space.
fn join(args: &[String]) -> String {
    args.iter()
        .map(|arg| {
            if arg.is_empty() || arg.contains(' ') {
                format!("\"{arg}\"")
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_container_and_family_has_its_own_filter_key() {
        let sid = "S-1-15-2-1";
        assert_eq!(filter_key(sid, Family::V4), filter_key(sid, Family::V4));
        assert_ne!(filter_key(sid, Family::V4), filter_key(sid, Family::V6));
        assert_ne!(
            filter_key(sid, Family::V4),
            filter_key("S-1-15-2-2", Family::V4)
        );
    }

    #[test]
    fn arguments_with_a_space_or_nothing_are_quoted() {
        let args = ["grant".to_owned(), "a b".to_owned(), String::new()];
        assert_eq!(join(&args), "grant \"a b\" \"\"");
    }

    #[test]
    fn only_appcontainer_sids_are_accepted() {
        assert!(matches!(parse("S-1-5-18"), Err(SandboxError::Invalid(_))));
    }
}
