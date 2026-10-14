//! Registry reads for the catalogue and the product type: a string value, the names of a key's subkeys, and
//! whether a key exists.

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, KEY_READ, RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
};
use windows::core::{HSTRING, PWSTR};

/// An open key, closed when dropped.
struct OpenKey(HKEY);

impl Drop for OpenKey {
    fn drop(&mut self) {
        // SAFETY: the handle came from RegOpenKeyExW and is closed once, here.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn open(root: HKEY, path: &str) -> Option<OpenKey> {
    let mut key = HKEY::default();
    let path = HSTRING::from(path);
    // SAFETY: `path` outlives the call and `key` is a valid out pointer.
    let status = unsafe { RegOpenKeyExW(root, &path, 0, KEY_READ, &mut key) };
    (status == ERROR_SUCCESS).then_some(OpenKey(key))
}

/// Whether the key at `path` under `root` exists.
pub fn key_exists(root: HKEY, path: &str) -> bool {
    open(root, path).is_some()
}

/// The string value `value` of the key at `path` under `root`; the empty name is the default value.
pub fn string_value(root: HKEY, path: &str, value: &str) -> Option<String> {
    let path = HSTRING::from(path);
    let value = HSTRING::from(value);
    let mut bytes = 0u32;
    // SAFETY: a null buffer asks only for the size, which `bytes` receives.
    let status = unsafe {
        RegGetValueW(
            root,
            &path,
            &value,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut bytes),
        )
    };
    if status != ERROR_SUCCESS || bytes == 0 {
        return None;
    }
    let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
    // SAFETY: `buffer` holds `bytes` bytes (rounded up to whole units) and `bytes` is its size on entry.
    let status = unsafe {
        RegGetValueW(
            root,
            &path,
            &value,
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let text = String::from_utf16_lossy(&buffer);
    Some(text.trim_end_matches('\0').to_string())
}

/// The names of the subkeys of the key at `path` under `root`. A key that cannot be opened has none.
pub fn subkeys(root: HKEY, path: &str) -> Vec<String> {
    let Some(key) = open(root, path) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for index in 0.. {
        let mut buffer = [0u16; 256];
        let mut len = buffer.len() as u32;
        // SAFETY: `buffer` has `len` units and no class name is asked for.
        let status = unsafe {
            RegEnumKeyExW(
                key.0,
                index,
                PWSTR(buffer.as_mut_ptr()),
                &mut len,
                None,
                PWSTR::null(),
                None,
                None,
            )
        };
        if status != ERROR_SUCCESS {
            break;
        }
        names.push(String::from_utf16_lossy(&buffer[..len as usize]));
    }
    names
}
