//! What the backend refuses, and why, in the words the daemon passes on.

/// A pixel session on a Windows client edition: those have one active desktop session, so an agent cannot have
/// its own screen there.
pub const CLIENT_PIXEL_UNSUPPORTED: &str = "one desktop session on Windows client editions; use the shared session or a Windows VM workstation";

/// A pixel session on Windows Server, where one session per agent is the design.
pub const SERVER_PIXEL_UNSUPPORTED: &str =
    "one session per agent on Windows Server needs the multi-session path, which is not built yet";

/// Whether a `ProductType` registry value (`WinNT`, `ServerNT`, `LanmanNT`) names a server edition, which can
/// run several sessions at once.
pub fn is_server_product_type(product_type: &str) -> bool {
    product_type.eq_ignore_ascii_case("ServerNT") || product_type.eq_ignore_ascii_case("LanmanNT")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_editions_are_recognised() {
        assert!(is_server_product_type("ServerNT"));
        assert!(is_server_product_type("LanmanNT"));
        assert!(is_server_product_type("servernt"));
        assert!(!is_server_product_type("WinNT"));
        assert!(!is_server_product_type(""));
    }
}
