//! Owner-only access to the station's token file.
//!
//! The enrolment record holds the station token. On Unix it is written mode 0600 (`apz_os::private`). A Windows file
//! has no mode bits, so the file gets an explicit DACL: the current user and SYSTEM, full access, and no inherited
//! entries (the DACL is protected, so the profile's ACL does not add anyone).

use std::path::Path;

/// Gives the file at `path` an owner-only ACL on Windows. On Unix the file is already 0600, so this does nothing.
#[cfg(not(windows))]
pub fn restrict_to_owner(path: &Path) -> Result<(), String> {
    let _ = path;
    Ok(())
}

/// Gives the file at `path` a protected DACL that grants only the current user and SYSTEM.
#[cfg(windows)]
pub fn restrict_to_owner(path: &Path) -> Result<(), String> {
    win::restrict(path)
}

#[cfg(windows)]
mod win {
    use std::ffi::{OsStr, c_void};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr;

    use windows::Win32::Foundation::{BOOL, CloseHandle, HANDLE, HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1, SE_FILE_OBJECT, SetNamedSecurityInfoW,
    };
    use windows::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetTokenInformation,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::core::{PCWSTR, PWSTR};

    /// Closes a token handle when it goes out of scope.
    struct Token(HANDLE);

    impl Drop for Token {
        fn drop(&mut self) {
            // SAFETY: the handle came from OpenProcessToken and is closed once, here.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    /// Frees a buffer that a Win32 call allocated with LocalAlloc, when it goes out of scope.
    struct Local(*mut c_void);

    impl Drop for Local {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: the pointer came from a Win32 call that documents LocalFree as its release.
                unsafe {
                    LocalFree(HLOCAL(self.0));
                }
            }
        }
    }

    fn wide(text: &OsStr) -> Vec<u16> {
        text.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn fail(what: &str, err: impl std::fmt::Display) -> String {
        format!("cannot {what}: {err}")
    }

    /// The current user's SID in string form (`S-1-5-...`).
    fn current_user_sid() -> Result<String, String> {
        // SAFETY: each call gets the handles and buffers this function owns, sized as the call needs.
        unsafe {
            let mut handle = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle)
                .map_err(|e| fail("open the process token", e))?;
            let token = Token(handle);
            // A TOKEN_USER is a SID_AND_ATTRIBUTES and a SID of at most 68 bytes: 256 bytes always holds it.
            let mut buffer = vec![0u64; 32];
            let mut needed = 0u32;
            GetTokenInformation(
                token.0,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                (buffer.len() * std::mem::size_of::<u64>()) as u32,
                &mut needed,
            )
            .map_err(|e| fail("read the user from the token", e))?;
            let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
            let mut text = PWSTR::null();
            ConvertSidToStringSidW(user.User.Sid, &mut text)
                .map_err(|e| fail("format the user's SID", e))?;
            let text = Local(text.0.cast());
            let sid = PWSTR(text.0.cast())
                .to_string()
                .map_err(|e| fail("read the user's SID", e))?;
            Ok(sid)
        }
    }

    /// The protected DACL: the user and SYSTEM with full access, and nothing else.
    fn owner_only_sddl(user_sid: &str) -> String {
        format!("D:P(A;;FA;;;{user_sid})(A;;FA;;;SY)")
    }

    /// Replaces the file's DACL with the owner-only one.
    pub fn restrict(path: &Path) -> Result<(), String> {
        let sid = current_user_sid()?;
        let sddl = wide(OsStr::new(&owner_only_sddl(&sid)));
        let target = wide(path.as_os_str());
        // SAFETY: the SDDL and path buffers are NUL-terminated and live for each call; the descriptor the
        // conversion allocates is freed by `Local`, and the DACL pointer is used only while it is alive.
        unsafe {
            let mut descriptor = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
            .map_err(|e| fail("build the owner-only ACL", e))?;
            let descriptor = Local(descriptor.0);
            let mut present = BOOL(0);
            let mut dacl: *mut ACL = ptr::null_mut();
            let mut defaulted = BOOL(0);
            GetSecurityDescriptorDacl(
                PSECURITY_DESCRIPTOR(descriptor.0),
                &mut present,
                &mut dacl,
                &mut defaulted,
            )
            .map_err(|e| fail("read the owner-only ACL", e))?;
            if !present.as_bool() || dacl.is_null() {
                return Err("the owner-only ACL has no DACL".to_string());
            }
            SetNamedSecurityInfoW(
                PCWSTR(target.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(dacl.cast_const()),
                None,
            )
            .ok()
            .map_err(|e| fail("set the file's ACL", e))?;
        }
        Ok(())
    }

    /// The file's DACL in SDDL form, read back from the file.
    #[cfg(test)]
    pub fn read_back(path: &Path) -> Result<String, String> {
        use windows::Win32::Security::Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW,
        };
        let target = wide(path.as_os_str());
        // SAFETY: the path is NUL-terminated for the call; the descriptor and string are freed by `Local`.
        unsafe {
            let mut descriptor = PSECURITY_DESCRIPTOR::default();
            GetNamedSecurityInfoW(
                PCWSTR(target.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                None,
                None,
                &mut descriptor,
            )
            .ok()
            .map_err(|e| fail("read the file's ACL", e))?;
            let descriptor = Local(descriptor.0);
            let mut text = PWSTR::null();
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                PSECURITY_DESCRIPTOR(descriptor.0),
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut text,
                None,
            )
            .map_err(|e| fail("format the file's ACL", e))?;
            let text = Local(text.0.cast());
            PWSTR(text.0.cast())
                .to_string()
                .map_err(|e| fail("read the file's ACL text", e))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_token_file_grants_only_the_user_and_system() {
            let dir = std::env::temp_dir().join(format!(
                "apz-acl-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).expect("a scratch directory");
            let file = dir.join("record.json");
            std::fs::write(&file, b"{}").expect("a scratch file");

            restrict(&file).expect("the ACL is set");
            let sddl = read_back(&file).expect("the ACL reads back");
            let sid = current_user_sid().expect("the user's SID");

            assert!(sddl.starts_with("D:P"), "the DACL is protected: {sddl}");
            assert_eq!(sddl.matches("(A;").count(), 2, "two entries: {sddl}");
            assert!(sddl.contains(&format!("(A;;FA;;;{sid})")), "{sddl}");
            assert!(sddl.contains("(A;;FA;;;SY)"), "{sddl}");

            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

#[cfg(all(test, not(windows)))]
mod tests {}
